//! Readable artifacts beside the lossless PCM timeline. SQLite remains authoritative.
use crate::{audio, store::Store, Engine, MeetingFile, MeetingFiles, RecordingTake, SAMPLE_RATE};
use anyhow::Result;
use std::{fs, path::Path};
use uuid::Uuid;

/// Additive migration: old recordings become take 1; unfinished takes retain all saved samples.
pub fn recover(store: &Store, root: &Path) -> Result<()> {
    for meeting in store.meetings()? {
        let audio = root.join("meetings").join(&meeting.id).join("audio");
        let samples = audio::samples_on_disk(&audio)?;
        let takes = store.takes(&meeting.id)?;
        if takes.is_empty() && samples > 0 {
            store.save_take(&RecordingTake {
                id: Uuid::new_v4().to_string(),
                meeting_id: meeting.id.clone(),
                number: 1,
                started_at: meeting.created_at.clone(),
                start_sample: 0,
                end_sample: Some(samples),
                status: if meeting.status == "interrupted" {
                    "interrupted"
                } else {
                    "ready"
                }
                .into(),
            })?;
        }
        for mut take in takes.into_iter().filter(|t| t.end_sample.is_none()) {
            take.end_sample = Some(samples.max(take.start_sample));
            take.status = "interrupted".into();
            store.save_take(&take)?;
        }
    }
    Ok(())
}
fn timestamp(ms: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60
    )
}
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    if fs::read_to_string(path).is_ok_and(|old| old == text) {
        return Ok(());
    }
    let tmp = path.with_extension("part");
    fs::write(&tmp, text)?;
    fs::rename(tmp, path)?;
    Ok(())
}
impl Engine {
    /// Serialized mirrors cannot overwrite a newer revision with an older snapshot.
    pub fn meeting_files(&self, id: &str) -> Result<MeetingFiles> {
        let _files = self.files_lock.lock().unwrap();
        let audio = self.audio_dir(id)?;
        let directory = audio.parent().unwrap().join("files");
        fs::create_dir_all(&directory)?;
        let (meeting, segments, takes) = {
            let db = self.store.lock().unwrap();
            let meeting = db.meeting(id)?;
            let segments = db.segments(&meeting)?;
            (meeting, segments, db.takes(id)?)
        };
        let total = audio::samples_on_disk(&audio)?;
        let mut files = Vec::new();
        let transcript = format!(
            "# {}\n\n{}\n\n{}",
            meeting.title,
            meeting.created_at,
            segments
                .iter()
                .map(|s| format!(
                    "[{}–{}] {}\n\n",
                    timestamp(s.start_ms),
                    timestamp(s.end_ms),
                    s.text
                ))
                .collect::<String>()
        );
        let transcript_path = directory.join("transcript.md");
        write_atomic(&transcript_path, &transcript)?;
        files.push(MeetingFile {
            name: "transcript.md".into(),
            path: transcript_path.to_string_lossy().into(),
            kind: "transcript".into(),
            bytes: fs::metadata(&transcript_path)?.len(),
            take: None,
            start_ms: 0,
            duration_ms: total * 1000 / SAMPLE_RATE,
            status: "ready".into(),
        });
        for take in takes {
            let end = take.end_sample.unwrap_or(total).max(take.start_sample);
            let wav = directory.join(format!("recording-{:03}.wav", take.number));
            if take.end_sample.is_some()
                && !fs::metadata(&wav).is_ok_and(|m| m.len() == 44 + (end - take.start_sample) * 2)
            {
                audio::export_wav_range(&audio, &wav, take.start_sample, end)?;
            }
            files.push(MeetingFile {
                name: wav.file_name().unwrap().to_string_lossy().into(),
                path: wav.to_string_lossy().into(),
                kind: "audio".into(),
                bytes: fs::metadata(&wav).map(|m| m.len()).unwrap_or(0),
                take: Some(take.number),
                start_ms: take.start_sample * 1000 / SAMPLE_RATE,
                duration_ms: (end - take.start_sample) * 1000 / SAMPLE_RATE,
                status: take.status.clone(),
            });
            let text = segments
                .iter()
                .filter(|s| {
                    s.start_ms >= take.start_sample * 1000 / SAMPLE_RATE
                        && s.start_ms < end * 1000 / SAMPLE_RATE
                })
                .map(|s| {
                    format!(
                        "[{}] {}\n",
                        timestamp(
                            s.start_ms
                                .saturating_sub(take.start_sample * 1000 / SAMPLE_RATE)
                        ),
                        s.text
                    )
                })
                .collect::<String>();
            let path = directory.join(format!("transcript-{:03}.txt", take.number));
            write_atomic(&path, &text)?;
            files.push(MeetingFile {
                name: path.file_name().unwrap().to_string_lossy().into(),
                path: path.to_string_lossy().into(),
                kind: "transcript".into(),
                bytes: fs::metadata(&path)?.len(),
                take: Some(take.number),
                start_ms: take.start_sample * 1000 / SAMPLE_RATE,
                duration_ms: (end - take.start_sample) * 1000 / SAMPLE_RATE,
                status: take.status,
            });
        }
        Ok(MeetingFiles {
            directory: directory.to_string_lossy().into(),
            files,
        })
    }
    pub(crate) fn sync_meeting_files(&self, id: &str) {
        if let Err(error) = self.meeting_files(id) {
            self.status.lock().unwrap().error =
                Some(format!("可读文件同步失败，原始录音和转写仍已保存：{error}"));
        }
    }
    pub(crate) fn finish_take(&self, active: &crate::engine::Active, failed: bool) -> Result<()> {
        let db = self.store.lock().unwrap();
        if let Some(mut take) = db
            .takes(&active.id)?
            .into_iter()
            .find(|t| Some(t.number) == active.take_number)
        {
            take.end_sample = Some(active.samples.load(std::sync::atomic::Ordering::SeqCst));
            take.status = if failed { "interrupted" } else { "ready" }.into();
            db.save_take(&take)?;
        }
        Ok(())
    }
}
