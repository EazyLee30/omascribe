use crate::{Engine, Segment};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    process::Stdio,
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker: u32,
}
/// Only attribute a passage when one voice covers a clear majority of its voiced time.
pub fn dominant_speaker(segment: &Segment, turns: &[Turn]) -> Option<u32> {
    let mut overlap = BTreeMap::<u32, u64>::new();
    for t in turns {
        let n = segment
            .end_ms
            .min(t.end_ms)
            .saturating_sub(segment.start_ms.max(t.start_ms));
        *overlap.entry(t.speaker).or_default() += n;
    }
    let total: u64 = overlap.values().sum();
    let (&speaker, &best) = overlap.iter().max_by_key(|(_, n)| *n)?;
    let minimum = (segment.end_ms.saturating_sub(segment.start_ms) / 4).clamp(300, 1000);
    (best >= minimum && best.saturating_mul(100) >= total.saturating_mul(70)).then_some(speaker)
}
/// Relate new clusters to human-labelled examples. Ambiguous clusters keep a fresh identity.
fn anchored_names(segments: &[Segment], turns: &[Turn]) -> BTreeMap<u32, String> {
    let mut votes = BTreeMap::<u32, BTreeMap<String, u64>>::new();
    for s in segments
        .iter()
        .filter(|s| s.speaker_manual && s.speaker.is_some())
    {
        if let Some(cluster) = dominant_speaker(s, turns) {
            let weight = turns
                .iter()
                .filter(|t| t.speaker == cluster)
                .map(|t| {
                    s.end_ms
                        .min(t.end_ms)
                        .saturating_sub(s.start_ms.max(t.start_ms))
                })
                .sum::<u64>();
            *votes
                .entry(cluster)
                .or_default()
                .entry(s.speaker.clone().unwrap())
                .or_default() += weight;
        }
    }
    let mut result = BTreeMap::new();
    for (cluster, names) in votes {
        let total = names.values().sum::<u64>();
        if let Some((name, weight)) = names.into_iter().max_by_key(|(_, n)| *n) {
            if weight >= 1000 && weight * 100 >= total * 85 {
                result.insert(cluster, name);
            }
        }
    }
    result
}
pub fn labeled_text(s: &Segment, m: &crate::Meeting) -> String {
    match s.speaker.as_ref().and_then(|id| m.speaker_names.get(id)) {
        Some(name) => format!("{name}: {}", s.text),
        None => s.text.clone(),
    }
}
impl Engine {
    pub fn set_meeting_diarization(&self, id: &str, enabled: bool) -> Result<()> {
        let _gate = self.config_lock.lock().unwrap();
        if self.status.lock().unwrap().diarization_busy {
            bail!("Speaker analysis is running");
        }
        let db = self.store.lock().unwrap();
        let mut m = db.meeting(id)?;
        m.diarization = enabled;
        if enabled {
            self.diarization_cancelled.lock().unwrap().remove(id);
        }
        db.save_meeting(&m)
    }
    pub fn rename_speaker(&self, meeting: &str, id: Option<String>, name: &str) -> Result<String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 60 || name.contains(['\n', '\r']) {
            bail!("Use a speaker name of 1–60 characters");
        }
        let mut db = self.store.lock().unwrap();
        let mut m = db.meeting(meeting)?;
        if id
            .as_ref()
            .is_some_and(|id| !m.speaker_names.contains_key(id))
        {
            bail!("Unknown speaker");
        }
        let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if m.speaker_names.len() >= 64 && !m.speaker_names.contains_key(&id) {
            bail!("Too many speakers");
        }
        m.speaker_names.insert(id.clone(), name.into());
        let segments = db.segments(&m)?;
        let tx = db.conn.transaction()?;
        tx.execute(
            "UPDATE meetings SET json=?2 WHERE id=?1",
            rusqlite::params![m.id, serde_json::to_string(&m)?],
        )?;
        for mut s in segments
            .into_iter()
            .filter(|s| s.speaker.as_deref() == Some(&id))
        {
            tx.execute(
                "INSERT OR IGNORE INTO segment_history VALUES(?1,?2,?3)",
                rusqlite::params![s.id, s.revision, serde_json::to_string(&s)?],
            )?;
            s.speaker_manual = true;
            s.revision += 1;
            tx.execute(
                "UPDATE segments SET json=?2 WHERE id=?1",
                rusqlite::params![s.id, serde_json::to_string(&s)?],
            )?;
        }
        tx.commit()?;
        for mut s in db.suggestions(meeting)? {
            s.stale = true;
            db.save_suggestion(&s)?;
        }
        drop(db);
        self.sync_meeting_files(meeting);
        Ok(id)
    }
    pub fn assign_speaker(
        &self,
        meeting: &str,
        segment: &str,
        speaker: Option<String>,
        revision: u32,
    ) -> Result<()> {
        self.assign_speaker_group(meeting, segment, speaker, revision, false)
    }
    pub fn assign_speaker_group(
        &self,
        meeting: &str,
        segment: &str,
        speaker: Option<String>,
        revision: u32,
        all_matching: bool,
    ) -> Result<()> {
        let mut db = self.store.lock().unwrap();
        let m = db.meeting(meeting)?;
        if speaker
            .as_ref()
            .is_some_and(|id| !m.speaker_names.contains_key(id))
        {
            bail!("Unknown speaker");
        }
        let s = db
            .segments(&m)?
            .into_iter()
            .find(|s| s.id == segment)
            .context("Unknown transcript passage")?;
        if s.revision != revision {
            bail!("Passage changed; refresh before editing");
        }
        let group = s.speaker.clone();
        let targets: Vec<_> = db
            .segments(&m)?
            .into_iter()
            .filter(|p| {
                p.id == segment
                    || all_matching && group.is_some() && p.speaker == group && !p.speaker_manual
            })
            .collect();
        let suggestions = db.suggestions(meeting)?;
        let tx = db.conn.transaction()?;
        let mut changed = std::collections::HashSet::new();
        for mut passage in targets {
            tx.execute(
                "INSERT OR IGNORE INTO segment_history VALUES(?1,?2,?3)",
                rusqlite::params![
                    passage.id,
                    passage.revision,
                    serde_json::to_string(&passage)?
                ],
            )?;
            changed.insert(passage.id.clone());
            passage.speaker = speaker.clone();
            passage.speaker_manual = true;
            passage.revision += 1;
            tx.execute(
                "UPDATE segments SET json=?2 WHERE id=?1",
                rusqlite::params![passage.id, serde_json::to_string(&passage)?],
            )?;
        }
        for mut suggestion in suggestions {
            if suggestion
                .sources
                .iter()
                .any(|q| changed.contains(&q.segment_id))
            {
                suggestion.stale = true;
                tx.execute(
                    "UPDATE suggestions SET json=?2 WHERE id=?1",
                    rusqlite::params![suggestion.id, serde_json::to_string(&suggestion)?],
                )?;
            }
        }
        tx.commit()?;
        drop(db);
        self.sync_meeting_files(meeting);
        Ok(())
    }
    pub fn cancel_diarization(&self) {
        let _gate = self.config_lock.lock().unwrap();
        let status = self.status.lock().unwrap().clone();
        if !status.diarization_busy {
            return;
        }
        if let Some(id) = status.diarization_meeting_id {
            self.diarization_cancelled.lock().unwrap().insert(id);
        }
        crate::platform::stop_process(self.diarization_pid.load(Ordering::SeqCst), false);
    }
    pub fn diarize(self: &Arc<Self>, id: &str) -> Result<()> {
        self.diarize_with_count(id, None)
    }
    pub fn diarize_with_count(self: &Arc<Self>, id: &str, count: Option<u32>) -> Result<()> {
        self.diarize_with_admission_readers(
            id,
            count,
            || self.verified_model("speaker-diarization"),
            crate::platform::available_memory_mib,
        )
    }
    // The public entry always uses the catalog verifier and platform memory
    // reader. Injection only makes their existing admission order observable.
    fn diarize_with_admission_readers(
        self: &Arc<Self>,
        id: &str,
        count: Option<u32>,
        verify_model: impl FnOnce() -> Result<crate::model_integrity::Receipt>,
        available_memory: impl Fn() -> Option<u64>,
    ) -> Result<()> {
        if count.is_some_and(|n| n > 16) {
            bail!("Speaker count must be 0–16");
        }
        let verified = verify_model()?;
        let _gate = self.config_lock.lock().unwrap();
        self.require_available()?;
        self.configuration_idle()?;
        verified.check_current()?;
        let before = self.snapshot(id)?;
        if before.segments.is_empty() {
            bail!("Transcribe audio before separating speakers");
        }
        if before.meeting.duration_ms > 7_200_000 {
            bail!("Speaker analysis supports recordings up to two hours; split longer recordings first");
        }
        let required = 700 + before.meeting.duration_ms * 64 / 1024 / 1024;
        if available_memory().is_none_or(|m| m < required) {
            bail!("Not enough memory for speaker analysis");
        }
        let dir = verified.directory();
        let prefs = self.preferences.lock().unwrap().clone();
        let job = uuid::Uuid::new_v4().to_string();
        let output = self.root.join(format!("speaker-{job}.json"));
        let mut command = crate::platform::child_command(self.worker_path());
        command
            .arg("--diarize")
            .arg(dir.join("segmentation.onnx"))
            .arg(dir.join("embedding.onnx"))
            .arg(self.audio_dir(id)?)
            .arg(&output)
            .arg(count.unwrap_or(prefs.speaker_count).to_string())
            .arg(prefs.threads.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(self.root.join("diarization.log"))?);
        verified.check_current()?;
        let mut child = command
            .spawn()
            .context("Unable to start speaker analysis")?;
        if let Err(error) = verified.check_current() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        self.diarization_pid.store(child.id(), Ordering::SeqCst);
        self.diarization_cancelled.lock().unwrap().remove(id);
        {
            let mut s = self.status.lock().unwrap();
            s.diarization_busy = true;
            s.diarization_status = "running".into();
            s.diarization_meeting_id = Some(id.into());
        }
        let engine = self.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<()> {
                let deadline = Instant::now();
                loop {
                    if let Some(status) = child.try_wait()? {
                        if !status.success() {
                            bail!("Speaker analysis stopped; transcript is preserved");
                        }
                        break;
                    }
                    if deadline.elapsed() > Duration::from_secs(1800) {
                        let _ = child.kill();
                        let _ = child.wait();
                        bail!("Speaker analysis timed out");
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                verified.check_current()?;
                if std::fs::metadata(&output)?.len() > 16 * 1024 * 1024 {
                    bail!("Speaker result too large");
                }
                let turns: Vec<Turn> = serde_json::from_slice(&std::fs::read(&output)?)?;
                let _gate = engine.config_lock.lock().unwrap();
                if engine
                    .diarization_cancelled
                    .lock()
                    .unwrap()
                    .contains(&before.meeting.id)
                {
                    bail!("Speaker analysis cancelled; transcript is preserved");
                }
                let mut db = engine.store.lock().unwrap();
                let mut m = db.meeting(&before.meeting.id)?;
                let mut segments = db.segments(&m)?;
                if m.run != before.meeting.run
                    || segments.len() != before.segments.len()
                    || segments
                        .iter()
                        .zip(&before.segments)
                        .any(|(a, b)| a.id != b.id || a.revision != b.revision)
                {
                    bail!("Transcript changed during speaker analysis; run it again");
                }
                let anchors = anchored_names(&segments, &turns);
                let mut names = BTreeMap::new();
                for segment in &mut segments {
                    if segment.speaker_manual {
                        continue;
                    }
                    segment.speaker = dominant_speaker(segment, &turns).map(|speaker| {
                        if let Some(id) = anchors.get(&speaker) {
                            return id.clone();
                        }
                        let id = format!("{job}-{speaker}");
                        names.insert(id.clone(), format!("Speaker {}", speaker + 1));
                        id
                    });
                    segment.revision += 1;
                }
                // Only human-labelled audio anchors can carry names across reanalysis.
                m.speaker_names
                    .retain(|id, _| segments.iter().any(|s| s.speaker.as_ref() == Some(id)));
                m.speaker_names.extend(names);
                m.diarization_cursor = segments.last().map_or(0, |s| s.seq);
                let tx = db.conn.transaction()?;
                for s in &before.segments {
                    tx.execute(
                        "INSERT OR IGNORE INTO segment_history VALUES(?1,?2,?3)",
                        rusqlite::params![s.id, s.revision, serde_json::to_string(s)?],
                    )?;
                }
                for s in &segments {
                    tx.execute(
                        "UPDATE segments SET json=?2 WHERE id=?1",
                        rusqlite::params![s.id, serde_json::to_string(s)?],
                    )?;
                }
                tx.execute(
                    "UPDATE meetings SET json=?2 WHERE id=?1",
                    rusqlite::params![m.id, serde_json::to_string(&m)?],
                )?;
                tx.commit()?;
                for mut suggestion in db.suggestions(&m.id)? {
                    suggestion.stale = true;
                    db.save_suggestion(&suggestion)?;
                }
                drop(db);
                engine.sync_meeting_files(&m.id);
                Ok(())
            })();
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(output);
            engine.diarization_pid.store(0, Ordering::SeqCst);
            let mut s = engine.status.lock().unwrap();
            s.diarization_busy = false;
            s.diarization_status = match result {
                Ok(()) => "done".into(),
                Err(e) => format!("{e:#}"),
            };
        });
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn admission_model(engine: &Engine) -> Result<crate::models::Model> {
        let mut model = crate::models::model("speaker-diarization")?.clone();
        let directory = engine.root.join("model-store").join(&model.id);
        std::fs::create_dir_all(&directory)?;
        for asset in &mut model.files {
            asset.bytes = 4;
            asset.sha256 = format!("{:x}", Sha256::digest(b"good"));
            std::fs::write(directory.join(&asset.name), b"good")?;
        }
        Ok(model)
    }

    fn admission_source(engine: &Engine, meeting: &crate::Meeting) -> Result<()> {
        engine.store.lock().unwrap().save_segment(&Segment {
            id: format!("{}:1:1", meeting.id),
            meeting_id: meeting.id.clone(),
            run: meeting.run,
            seq: 1,
            start_ms: 0,
            end_ms: 1000,
            text: "录音仍待核对。".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        })
    }

    fn assert_admission_never_started(engine: &Engine) {
        assert_eq!(engine.diarization_pid.load(Ordering::SeqCst), 0);
        let status = engine.status.lock().unwrap();
        assert!(!status.diarization_busy);
        assert!(status.diarization_meeting_id.is_none());
        assert!(!status.notes_busy);
        drop(status);
        assert!(!engine.note_busy.load(Ordering::SeqCst));
        assert_eq!(engine.inference_pool.status().starts, 0);
        assert!(!engine.root.join("diarization.log").exists());
    }

    fn admission_asset_hash(path: &std::path::Path, expected: u64) -> Result<String> {
        use std::io::Read;
        let mut input = std::fs::File::open(path)?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut bytes = 0u64;
        loop {
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            bytes = bytes.checked_add(n as u64).context("Asset size overflow")?;
            anyhow::ensure!(bytes <= expected, "Asset grew during fixture hashing");
            hash.update(&buffer[..n]);
        }
        anyhow::ensure!(bytes == expected, "Asset shrank during fixture hashing");
        Ok(format!("{:x}", hash.finalize()))
    }

    #[test]
    fn cold_speaker_admission_rejections_do_not_hash_model_assets() -> Result<()> {
        // These are valid catalog-descriptor fixtures, not executable ONNX
        // models. Each path must reach its existing admission error without
        // constructing a worker, so native model execution is impossible.
        let mut observations = Vec::new();
        for rejection in ["recording", "empty", "too_long", "memory"] {
            let root = tempfile::tempdir()?;
            let engine = Engine::open(root.path().into())?;
            let mut meeting = engine.create_meeting("Speaker admission")?;
            let model = admission_model(&engine)?;
            if rejection != "empty" {
                admission_source(&engine, &meeting)?;
            }
            if rejection == "recording" {
                engine.status.lock().unwrap().recording = true;
            }
            if rejection == "too_long" {
                meeting.duration_ms = 7_200_001;
                engine.store.lock().unwrap().save_meeting(&meeting)?;
            }
            let expected = match rejection {
                "recording" => "Finish recording, transcription and writing before changing models",
                "empty" => "Transcribe audio before separating speakers",
                "too_long" => "Speaker analysis supports recordings up to two hours; split longer recordings first",
                _ => "Not enough memory for speaker analysis",
            };
            assert_eq!(engine.model_integrity.hashed_bytes(), 0);
            let error = engine
                .diarize_with_admission_readers(
                    &meeting.id,
                    None,
                    || engine.verify_model_spec(&model),
                    || Some(if rejection == "memory" { 0 } else { u64::MAX }),
                )
                .expect_err("known ineligible actions cannot start speaker analysis");
            assert_eq!(error.to_string(), expected, "{rejection}");
            assert_admission_never_started(&engine);
            let bytes = engine.model_integrity.hashed_bytes();
            println!("SPEAKER_PREFLIGHT_REJECTION={rejection} MODEL_BYTES_HASHED={bytes}");
            observations.push((rejection, bytes));
        }
        assert!(
            observations.iter().all(|(_, bytes)| *bytes == 0),
            "cold admission refusals must not read model weights: {observations:?}"
        );
        Ok(())
    }

    #[test]
    #[ignore = "explicit pinned local speaker asset copy/hash control; never runs an ONNX model"]
    fn live_public_recording_speaker_rejection_does_not_hash_assets() -> Result<()> {
        use std::{fs, path::PathBuf};
        let original = PathBuf::from(std::env::var("OMASCRIBE_TEST_SPEAKER_MODELS")?);
        let model = crate::models::model("speaker-diarization")?;
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let directory = root.path().join("model-store").join(&model.id);
        fs::create_dir_all(&directory)?;
        let mut originals = Vec::new();
        for asset in &model.files {
            let path = original.join(&asset.name);
            let before = fs::symlink_metadata(&path)?;
            anyhow::ensure!(before.file_type().is_file() && before.len() == asset.bytes);
            let before_hash = admission_asset_hash(&path, asset.bytes)?;
            assert_eq!(before_hash, asset.sha256);
            let owned = directory.join(&asset.name);
            #[cfg(target_os = "macos")]
            {
                let mut command = std::process::Command::new("/bin/cp");
                command.arg("-c").arg(&path).arg(&owned);
                crate::probe::capture(&mut command, Duration::from_secs(30), 4096)?;
            }
            #[cfg(not(target_os = "macos"))]
            fs::copy(&path, &owned)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let copy = fs::symlink_metadata(&owned)?;
                assert_ne!((before.dev(), before.ino()), (copy.dev(), copy.ino()));
            }
            assert_eq!(admission_asset_hash(&owned, asset.bytes)?, asset.sha256);
            originals.push((path, before, before_hash));
        }
        let meeting = engine.create_meeting("Public speaker admission")?;
        admission_source(&engine, &meeting)?;
        engine.status.lock().unwrap().recording = true;
        assert_eq!(engine.model_integrity.hashed_bytes(), 0);
        let error = engine.diarize(&meeting.id).unwrap_err();
        assert_admission_never_started(&engine);
        let bytes = engine.model_integrity.hashed_bytes();
        println!("PUBLIC_SPEAKER_PREFLIGHT_MODEL_BYTES_HASHED={bytes}");
        // Windows cached receipts must be released before owned file cleanup.
        drop(engine);
        fs::remove_dir_all(&directory)?;
        assert!(!directory.exists());
        for (path, before, before_hash) in originals {
            assert_eq!(admission_asset_hash(&path, before.len())?, before_hash);
            let after = fs::symlink_metadata(path)?;
            assert_eq!(after.len(), before.len());
            assert_eq!(after.modified()?, before.modified()?);
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
                assert_eq!(
                    (after.ctime(), after.ctime_nsec()),
                    (before.ctime(), before.ctime_nsec())
                );
            }
        }
        println!(
            "PUBLIC_SPEAKER_PREFLIGHT_SETUP_HASH_BYTES={} OWNED_MODEL_BYTES={}",
            model.files.iter().map(|asset| asset.bytes).sum::<u64>() * 3,
            model.files.iter().map(|asset| asset.bytes).sum::<u64>()
        );
        assert_eq!(
            error.to_string(),
            "Finish recording, transcription and writing before changing models"
        );
        assert_eq!(
            bytes, 0,
            "public recording refusal must not hash speaker assets"
        );
        Ok(())
    }

    #[test]
    fn cancelling_suspends_automatic_retries_until_reenabled() {
        let root = tempfile::tempdir().unwrap();
        let e = Engine::open(root.path().into()).unwrap();
        let m = e.create_meeting("Cancel test").unwrap();
        e.set_meeting_diarization(&m.id, true).unwrap();
        {
            let mut s = e.status.lock().unwrap();
            s.diarization_busy = true;
            s.diarization_meeting_id = Some(m.id.clone());
        }
        e.cancel_diarization();
        assert!(e.diarization_cancelled.lock().unwrap().contains(&m.id));
        e.status.lock().unwrap().diarization_busy = false;
        e.set_meeting_diarization(&m.id, true).unwrap();
        assert!(!e.diarization_cancelled.lock().unwrap().contains(&m.id));
    }
    #[test]
    fn overlapping_or_balanced_voices_are_not_guessed() {
        let s = Segment {
            id: "s".into(),
            meeting_id: "m".into(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 1000,
            text: "hello".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        assert_eq!(
            dominant_speaker(
                &s,
                &[
                    Turn {
                        start_ms: 0,
                        end_ms: 800,
                        speaker: 0
                    },
                    Turn {
                        start_ms: 800,
                        end_ms: 1000,
                        speaker: 1
                    }
                ]
            ),
            Some(0)
        );
        assert_eq!(
            dominant_speaker(
                &s,
                &[
                    Turn {
                        start_ms: 0,
                        end_ms: 1000,
                        speaker: 0
                    },
                    Turn {
                        start_ms: 0,
                        end_ms: 1000,
                        speaker: 1
                    }
                ]
            ),
            None
        );
        assert_eq!(dominant_speaker(&s, &[]), None);
    }
    #[test]
    fn renaming_and_assignment_keep_revision_history_and_named_exports() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        let meeting = e.create_meeting("Speaker test").unwrap();
        let s = Segment {
            id: uuid::Uuid::new_v4().to_string(),
            meeting_id: meeting.id.clone(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 1000,
            text: "预算待确认".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        e.store.lock().unwrap().save_segment(&s).unwrap();
        let id = e.rename_speaker(&meeting.id, None, "张三").unwrap();
        e.assign_speaker(&meeting.id, &s.id, Some(id.clone()), 1)
            .unwrap();
        assert!(e.assign_speaker(&meeting.id, &s.id, None, 1).is_err());
        e.rename_speaker(&meeting.id, Some(id), "李四").unwrap();
        let snap = e.snapshot(&meeting.id).unwrap();
        assert_eq!(snap.segments[0].revision, 3);
        assert!(snap.segments[0].speaker_manual);
        assert_eq!(
            labeled_text(&snap.segments[0], &snap.meeting),
            "李四: 预算待确认"
        );
        let history: i64 = e
            .store
            .lock()
            .unwrap()
            .conn
            .query_row("SELECT count(*) FROM segment_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(history, 2);
        let files = e.meeting_files(&meeting.id).unwrap();
        let transcript = files
            .files
            .iter()
            .find(|f| f.name == "transcript.md")
            .unwrap();
        assert!(std::fs::read_to_string(&transcript.path)
            .unwrap()
            .contains("李四: 预算待确认"));
        assert!(e
            .rename_speaker(&meeting.id, Some("unknown".into()), "Someone")
            .is_err());
    }
    #[test]
    fn manual_voice_anchors_follow_reordered_clusters_but_not_ambiguous_voices() {
        let segment = |id: &str, start, end, name: &str| Segment {
            id: id.into(),
            meeting_id: "test".into(),
            run: 1,
            seq: 1,
            start_ms: start,
            end_ms: end,
            text: "原文".into(),
            revision: 1,
            speaker: Some(name.into()),
            speaker_manual: true,
        };
        let segments = vec![
            segment("a", 0, 2000, "Alice"),
            segment("b", 2000, 4000, "Bob"),
        ];
        let turns = vec![
            Turn {
                start_ms: 0,
                end_ms: 2000,
                speaker: 8,
            },
            Turn {
                start_ms: 2000,
                end_ms: 4000,
                speaker: 3,
            },
        ];
        assert_eq!(
            anchored_names(&segments, &turns),
            BTreeMap::from([(3, "Bob".into()), (8, "Alice".into())])
        );
        let ambiguous = vec![Turn {
            start_ms: 0,
            end_ms: 4000,
            speaker: 0,
        }];
        assert!(anchored_names(&segments, &ambiguous).is_empty());
        let mut long = segments[0].clone();
        long.end_ms = 12000;
        assert_eq!(
            dominant_speaker(
                &long,
                &[Turn {
                    start_ms: 0,
                    end_ms: 350,
                    speaker: 0
                }]
            ),
            None
        );
    }
    #[test]
    fn assigning_a_voice_group_preserves_other_human_assignments() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        let m = e.create_meeting("Group").unwrap();
        let group = e.rename_speaker(&m.id, None, "Speaker 1").unwrap();
        let alice = e.rename_speaker(&m.id, None, "Alice").unwrap();
        for seq in 1..=3 {
            let s = Segment {
                id: format!("{}:1:{seq}", m.id),
                meeting_id: m.id.clone(),
                run: 1,
                seq,
                start_ms: seq as u64 * 1000,
                end_ms: seq as u64 * 1000 + 1000,
                text: "预算未定".into(),
                revision: 1,
                speaker: Some(group.clone()),
                speaker_manual: seq == 3,
            };
            e.store.lock().unwrap().save_segment(&s).unwrap();
        }
        e.assign_speaker_group(
            &m.id,
            &format!("{}:1:1", m.id),
            Some(alice.clone()),
            1,
            true,
        )
        .unwrap();
        let snap = e.snapshot(&m.id).unwrap();
        assert_eq!(snap.segments[0].speaker.as_ref(), Some(&alice));
        assert_eq!(snap.segments[1].speaker.as_ref(), Some(&alice));
        assert_eq!(snap.segments[2].speaker.as_ref(), Some(&group));
    }
}
