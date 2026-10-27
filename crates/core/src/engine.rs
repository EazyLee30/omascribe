use crate::{audio, notes, platform, store::Store, types::*};
use anyhow::{bail, Context, Result};
use base64::Engine as _;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

pub struct Active {
    pub id: String,
    pub take_number: Option<u32>,
    pub capture_pid: AtomicU32,
    pub capture_running: AtomicBool,
    pub capture_stop: Mutex<Option<Arc<AtomicBool>>>,
    pub worker_pid: AtomicU32,
    pub capture_done: AtomicBool,
    pub worker_done: AtomicBool,
    pub stopping: AtomicBool,
    pub paused: AtomicBool,
    pub device: String,
    pub resumable: bool,
    transition: Mutex<()>,
    pub samples: AtomicU64,
    /// Session sleep/idle hold. Present only while frames are arriving.
    hold: Mutex<Option<crate::inhibit::Inhibitor>>,
}

impl Active {
    /// Hold the desktop awake for as long as this take is capturing.
    fn hold_awake(&self) {
        let mut hold = self.hold.lock().unwrap();
        if hold.as_ref().is_none_or(|inhibitor| !inhibitor.held()) {
            *hold = Some(crate::inhibit::Inhibitor::acquire());
        }
    }
    /// Release the hold on pause, finish and failure.
    fn release_awake(&self) {
        if let Some(mut inhibitor) = self.hold.lock().unwrap().take() {
            inhibitor.release();
        }
    }
}

pub struct Engine {
    pub root: PathBuf,
    pub preferences: Mutex<crate::preferences::Preferences>,
    pub context_revision: AtomicU64,
    pub downloads: crate::models::Downloads,
    pub component_downloads: crate::models::Downloads,
    pub config_lock: Mutex<()>,
    pub diarization_pid: AtomicU32,
    pub diarization_cancelled: Mutex<std::collections::HashSet<String>>,
    pub models: PathBuf,
    pub runtime_dir: PathBuf,
    pub store: Mutex<Store>,
    pub status: Mutex<RuntimeStatus>,
    pub active: Mutex<Option<Arc<Active>>>,
    pub note_busy: AtomicBool,
    pub maintenance: AtomicBool,
    pub note_failures: AtomicU32,
    pub(crate) files_lock: Mutex<()>,
    pub note_cancel: AtomicBool,
    pub llm_pid: AtomicU32,
    pub(crate) inference_pool: crate::inference_worker::WorkerPool,
    pub(crate) inference_fingerprints: crate::inference_cache::Fingerprints,
    pub(crate) retrieval_cache: crate::notes::RetrievalCache,
    pub(crate) acceleration: Mutex<crate::acceleration::State>,
    microphone_monitor: crate::microphone::MicrophoneMonitor,
    notes_target: Mutex<Option<String>>,
    _lock: File,
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.microphone_monitor.stop();
        self.inference_pool.invalidate();
        let _ = fs2::FileExt::unlock(&self._lock);
    }
}
pub fn default_root() -> PathBuf {
    platform::data_root()
}
impl Engine {
    pub fn require_available(&self) -> Result<()> {
        if self.maintenance.load(Ordering::SeqCst) {
            bail!("update:installing");
        }
        Ok(())
    }
    pub fn open(root: PathBuf) -> Result<Arc<Self>> {
        use fs2::FileExt;
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(root.join("meetings"))?;
        #[cfg(unix)]
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join("app.lock"))?;
        if FileExt::try_lock_exclusive(&lock).is_err() {
            bail!("OmaScribe 已在运行，请切换到已打开的窗口");
        }
        let models = platform::asset_dir(&root, "models", "OMASCRIBE_MODELS_DIR");
        let runtime_dir = platform::asset_dir(&root, "runtime", "OMASCRIBE_RUNTIME_DIR");
        let store = Store::open(&root.join("meetings.sqlite3"))?;
        // A previous process can leave raw chunks behind. Recovery never deletes those chunks.
        for mut m in store.meetings()? {
            if ["recording", "paused", "importing", "transcribing"].contains(&m.status.as_str()) {
                let audio = root.join("meetings").join(&m.id).join("audio");
                m.duration_ms = audio::samples_on_disk(&audio)? * 1000 / SAMPLE_RATE;
                m.status = "interrupted".into();
                store.save_meeting(&m)?;
                if audio.exists() {
                    fs::write(audio.join("capture.done"), b"interrupted")?;
                }
            }
        }
        crate::files::recover(&store, &root)?;
        let preferences = crate::preferences::Preferences::load(&root)?;
        let engine = Arc::new(Self {
            preferences: Mutex::new(preferences),
            context_revision: AtomicU64::new(0),
            downloads: crate::models::Downloads::default(),
            component_downloads: crate::models::Downloads::default(),
            config_lock: Mutex::new(()),
            diarization_pid: AtomicU32::new(0),
            diarization_cancelled: Mutex::new(Default::default()),
            root,
            models,
            runtime_dir,
            store: Mutex::new(store),
            status: Mutex::new(RuntimeStatus::default()),
            active: Mutex::new(None),
            note_busy: AtomicBool::new(false),
            maintenance: AtomicBool::new(false),
            note_failures: AtomicU32::new(0),
            files_lock: Mutex::new(()),
            note_cancel: AtomicBool::new(false),
            llm_pid: AtomicU32::new(0),
            inference_pool: Default::default(),
            inference_fingerprints: Default::default(),
            retrieval_cache: Default::default(),
            acceleration: Default::default(),
            microphone_monitor: Default::default(),
            notes_target: Mutex::new(None),
            _lock: lock,
        });
        engine.recover_downloads()?;
        Ok(engine)
    }
    pub fn start_scheduler(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        // Experimental until paired real-model/capture checks establish the tradeoff.
        let catchup = std::env::var("OMASCRIBE_MINUTES_CATCHUP").as_deref() == Ok("1");
        thread::spawn(move || {
            let mut attempted = std::collections::HashMap::<String, Instant>::new();
            let mut minute_schedule = crate::minutes_schedule::Schedule::default();
            let mut in_flight: Option<String> = None;
            let mut translation_attempts = std::collections::HashMap::<String, Instant>::new();
            let mut speaker_attempts = std::collections::HashMap::<String, Instant>::new();
            loop {
                thread::sleep(Duration::from_secs(2));
                let Some(engine) = weak.upgrade() else { break };
                if engine.note_busy.load(Ordering::SeqCst) {
                    continue;
                }
                if let Some(id) = in_flight.take() {
                    attempted.insert(id.clone(), Instant::now());
                    if catchup {
                        if let Ok(snapshot) = engine.snapshot(&id) {
                            minute_schedule.finished(&snapshot.meeting, Instant::now());
                        }
                    }
                }
                let status = engine.status.lock().unwrap().clone();
                let capture_active = status.recording || status.paused || status.transcribing;
                let quality =
                    catchup && engine.preferences.lock().unwrap().resource_mode == "quality";
                if status.diarization_busy {
                    continue;
                }
                if status.transcribing
                    && status.elapsed_ms.saturating_sub(status.processed_ms) > 2500
                {
                    continue;
                }
                let mut ids = Vec::new();
                if let Some(id) = status.meeting_id.clone() {
                    ids.push(id);
                }
                if let Some(id) = engine.notes_target.lock().unwrap().clone() {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                if let Ok(meetings) = engine.meetings() {
                    for meeting in meetings {
                        if (meeting.auto_notes || meeting.diarization) && !ids.contains(&meeting.id)
                        {
                            ids.push(meeting.id);
                        }
                    }
                }
                if catchup {
                    minute_schedule.retain(&ids);
                }
                for id in ids {
                    let Ok(snapshot) = engine.snapshot(&id) else {
                        continue;
                    };
                    let pending_minutes = notes::needs_update(&snapshot);
                    if catchup {
                        let own_active =
                            capture_active && status.meeting_id.as_deref() == Some(&id);
                        minute_schedule.observe(&snapshot.meeting, own_active);
                        if !own_active && (!snapshot.meeting.auto_notes || !pending_minutes) {
                            minute_schedule.forget(&id);
                        }
                    }
                    let translation_due = (status.meeting_id.as_deref() == Some(&id)
                        || engine.notes_target.lock().unwrap().as_deref() == Some(&id))
                        && crate::translation::pending(&snapshot)
                        && translation_attempts.get(&id).is_none_or(|t| {
                            t.elapsed()
                                > Duration::from_secs(
                                    if status
                                        .translation_status
                                        .starts_with("translation:deferred")
                                    {
                                        30
                                    } else {
                                        2
                                    },
                                )
                        });
                    let minutes_due = snapshot.meeting.auto_notes
                        && pending_minutes
                        && if catchup {
                            minute_schedule.due(&id, capture_active, quality, Instant::now())
                        } else {
                            attempted.get(&id).is_none_or(|t| {
                                t.elapsed()
                                    > Duration::from_secs(
                                        45 * 2u64.pow(
                                            engine.note_failures.load(Ordering::SeqCst).min(3),
                                        ),
                                    )
                            })
                        }
                        && !snapshot
                            .notes
                            .iter()
                            .filter_map(|n| {
                                chrono::DateTime::parse_from_rfc3339(&n.updated_at).ok()
                            })
                            .any(|t| chrono::Utc::now().signed_duration_since(t).num_seconds() < 5);
                    if translation_due {
                        translation_attempts.insert(id.clone(), Instant::now());
                        if engine
                            .translate_with_minutes(&id, minutes_due)
                            .unwrap_or(false)
                        {
                            if minutes_due {
                                attempted.insert(id.clone(), Instant::now());
                                if catchup {
                                    minute_schedule.started(&snapshot.meeting, Instant::now());
                                    in_flight = Some(id.clone());
                                }
                            }
                            break;
                        }
                    }
                    if snapshot.meeting.diarization
                        && !engine.diarization_cancelled.lock().unwrap().contains(&id)
                        && !status.recording
                        && !status.paused
                        && !status.transcribing
                        && snapshot
                            .segments
                            .last()
                            .is_some_and(|s| s.seq > snapshot.meeting.diarization_cursor)
                        && speaker_attempts
                            .get(&id)
                            .is_none_or(|t| t.elapsed() > Duration::from_secs(90))
                    {
                        speaker_attempts.insert(id.clone(), Instant::now());
                        match engine.diarize(&id) {
                            Ok(()) => break,
                            Err(error) => {
                                let mut runtime = engine.status.lock().unwrap();
                                runtime.diarization_meeting_id = Some(id.clone());
                                runtime.diarization_status = error.to_string();
                            }
                        }
                    }
                    if !minutes_due {
                        continue;
                    }
                    attempted.insert(id.clone(), Instant::now());
                    let launched = engine.generate_notes(&id).is_ok();
                    if catchup {
                        minute_schedule.started(&snapshot.meeting, Instant::now());
                        if !launched {
                            minute_schedule.finished(&snapshot.meeting, Instant::now());
                        }
                    }
                    if launched {
                        in_flight = Some(id);
                    }
                    break;
                }
            }
        });
    }
    pub fn audio_dir(&self, id: &str) -> Result<PathBuf> {
        Uuid::parse_str(id).context("无效的会议编号")?;
        Ok(self.root.join("meetings").join(id).join("audio"))
    }
    pub fn create_meeting(&self, title: &str) -> Result<Meeting> {
        let defaults = self.preferences.lock().unwrap().clone();
        let m = Meeting {
            id: Uuid::new_v4().to_string(),
            title: if title.trim().is_empty() {
                "Untitled meeting".into()
            } else {
                title.trim().chars().take(160).collect()
            },
            created_at: chrono::Utc::now().to_rfc3339(),
            status: "draft".into(),
            duration_ms: 0,
            run: 1,
            notes_cursor: 0,
            auto_notes: defaults.auto_notes,
            diarization: defaults.diarization,
            speaker_names: Default::default(),
            diarization_cursor: 0,
            notes_fingerprint: String::new(),
        };
        fs::create_dir_all(self.audio_dir(&m.id)?)?;
        self.store.lock().unwrap().save_meeting(&m)?;
        Ok(m)
    }
    pub fn meetings(&self) -> Result<Vec<Meeting>> {
        self.store.lock().unwrap().meetings()
    }
    pub fn focus_meeting(&self, id: &str) -> Result<()> {
        self.correct_terminology(id)?;
        self.snapshot(id)?;
        *self.notes_target.lock().unwrap() = Some(id.into());
        Ok(())
    }
    pub fn snapshot(&self, id: &str) -> Result<Snapshot> {
        let prefs = self.preferences.lock().unwrap().clone();
        let db = self.store.lock().unwrap();
        let mut m = db.meeting(id)?;
        if db.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM deleted_meetings WHERE id=?1)",
            [id],
            |r| r.get::<_, bool>(0),
        )? {
            bail!("This meeting is in Deleted; restore it from Settings → Storage");
        }
        let runtime = self.status.lock().unwrap().clone();
        if runtime.meeting_id.as_deref() == Some(id) {
            m.duration_ms = m.duration_ms.max(runtime.elapsed_ms);
        }
        Ok(Snapshot {
            translations: crate::translation::cached(self, &db, &m, &prefs)?,
            translation_mode: prefs.translation_mode,
            segments: db.segments(&m)?,
            notes: db.notes(id)?,
            suggestions: db.suggestions(id)?,
            meeting: m,
            runtime,
        })
    }
    pub fn update_meeting(&self, id: &str, title: &str, auto_notes: bool) -> Result<()> {
        let db = self.store.lock().unwrap();
        let mut m = db.meeting(id)?;
        m.title = title.trim().chars().take(160).collect();
        m.auto_notes = auto_notes;
        db.save_meeting(&m)
    }
    pub fn save_note(
        &self,
        id: &str,
        note_id: Option<String>,
        content: &str,
        kind: &str,
        anchor_ms: u64,
    ) -> Result<UserNote> {
        if !["note", "focus", "term", "opinion", "document"].contains(&kind) {
            bail!("无效的记录类型");
        }
        if content.chars().count() > 200_000 {
            bail!("文档超过 200,000 字符，请拆分后保存；原有内容未改动");
        }
        let db = self.store.lock().unwrap();
        db.meeting(id)?;
        let nid = note_id.unwrap_or_else(|| Uuid::new_v4().to_string());
        Uuid::parse_str(&nid)?;
        let existing = db.notes(id)?;
        if let Ok(other) =
            db.conn
                .query_row("SELECT meeting_id FROM notes WHERE id=?1", [&nid], |r| {
                    r.get::<_, String>(0)
                })
        {
            if other != id {
                bail!("笔记不属于这场会议");
            }
        }
        let anchor = existing
            .iter()
            .find(|n| n.id == nid)
            .map(|n| n.anchor_ms)
            .unwrap_or(anchor_ms);
        let n = UserNote {
            id: nid,
            meeting_id: id.into(),
            content: content.to_owned(),
            kind: kind.into(),
            anchor_ms: anchor,
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        db.save_note(&n)?;
        drop(db);
        self.correct_terminology(id)?;
        self.refresh_live_hotwords()?;
        *self.notes_target.lock().unwrap() = Some(id.to_owned());
        Ok(n)
    }
    pub(crate) fn refresh_live_hotwords(&self) -> Result<()> {
        let id = self.active.lock().unwrap().as_ref().map(|a| a.id.clone());
        if let Some(id) = id {
            let dir = self.audio_dir(&id)?;
            let words = self.terminology(&id)?.hotwords.join("\n").to_uppercase();
            let temp = dir.join(format!("hotwords-{}.next", Uuid::new_v4()));
            fs::write(&temp, words)?;
            if let Err(error) = fs::rename(&temp, dir.join("hotwords.txt")) {
                let _ = fs::remove_file(temp);
                return Err(error.into());
            }
        }
        Ok(())
    }
    pub fn edit_segment(&self, id: &str, text: &str, revision: u32) -> Result<()> {
        if text.chars().count() > 4000 {
            bail!("片段文字过长");
        }
        let meeting = {
            let mut db = self.store.lock().unwrap();
            db.edit_segment(id, text, revision)?;
            db.conn
                .query_row("SELECT meeting_id FROM segments WHERE id=?1", [id], |r| {
                    r.get::<_, String>(0)
                })?
        };
        self.sync_meeting_files(&meeting);
        Ok(())
    }
    pub fn worker_path(&self) -> PathBuf {
        let installed = self.runtime_dir.join(platform::executable("omascribe-asr"));
        if installed.exists() {
            return installed;
        }
        std::env::current_exe()
            .unwrap_or_default()
            .with_file_name(platform::executable("omascribe-asr"))
    }
    pub fn start_recording(
        self: &Arc<Self>,
        id: &str,
        device: &str,
        input: Option<PathBuf>,
    ) -> Result<()> {
        self.start_capture(id, device, input, None)
    }
    pub fn microphone_level(&self) -> crate::microphone::InputLevel {
        self.microphone_monitor.status()
    }
    pub fn start_microphone_preview(
        &self,
        device: String,
    ) -> Result<crate::microphone::InputLevel> {
        let _config = self.config_lock.lock().unwrap();
        self.require_available()?;
        let status = self.status.lock().unwrap().clone();
        if status.recording || status.paused || status.transcribing {
            bail!("Finish recording and transcription before testing the microphone");
        }
        if !audio::devices()?
            .iter()
            .any(|input| input.id == device && input.kind == "microphone")
        {
            bail!("Select an available microphone input before testing");
        }
        self.microphone_monitor
            .start(device, &self.root.join("microphone-preview.log"))
    }
    pub fn stop_microphone_preview(&self) -> crate::microphone::InputLevel {
        self.microphone_monitor.stop();
        self.microphone_monitor.status()
    }
    /// Paced file replay exercises the same writer, ASR, scheduler and persistence as a meeting.
    pub fn replay_audio(self: &Arc<Self>, id: &str, input: PathBuf, seconds: u64) -> Result<()> {
        if seconds == 0 || seconds > 14_400 {
            bail!("回放测试时长须为 1 至 14400 秒");
        }
        self.start_capture(id, "auto", Some(input), Some(seconds))
    }
    fn start_capture(
        self: &Arc<Self>,
        id: &str,
        device: &str,
        input: Option<PathBuf>,
        replay_seconds: Option<u64>,
    ) -> Result<()> {
        let _config = self.config_lock.lock().unwrap();
        self.microphone_monitor.stop();
        self.require_available()?;
        if self.status.lock().unwrap().diarization_busy {
            bail!("Cancel speaker analysis before starting another recording");
        }
        // start_capture already owns config_lock; this only checks whether a
        // calibration needs cancellation, without restoring persisted state.
        if self.acceleration_test_running() {
            self.cancel_notes();
        }
        let mut active_guard = self.active.lock().unwrap();
        if active_guard.as_ref().is_some_and(|a| {
            !a.worker_done.load(Ordering::SeqCst) || !a.capture_done.load(Ordering::SeqCst)
        }) {
            bail!("请先结束当前录音或转写");
        }
        let mut m = self.store.lock().unwrap().meeting(id)?;
        if !self.diagnostics().asr_ready || !self.worker_path().exists() {
            bail!("离线转写模型或运行时尚未准备好，请运行 scripts/bin/setup");
        }
        if let Some(ref p) = input {
            if !p.is_file() {
                bail!("找不到导入的音频文件");
            }
        }
        let dir = self.audio_dir(id)?;
        let initial = audio::samples_on_disk(&dir)?;
        let (take_number, next_seq) = {
            let db = self.store.lock().unwrap();
            (
                db.takes(id)?.last().map_or(1, |t| t.number + 1),
                db.segments(&m)?.last().map_or(1, |s| s.seq + 1),
            )
        };
        for marker in ["capture.done", "capture.paused", "capture.error"] {
            let _ = fs::remove_file(dir.join(marker));
        }
        let live = input.is_none() || replay_seconds.is_some();
        let mut child = audio::spawn_capture(
            device,
            input.as_deref(),
            replay_seconds,
            &dir.join("capture.log"),
        )?;
        let output = child.stdout.take().context("无法读取录音流")?;
        let active = Arc::new(Active {
            id: id.into(),
            take_number: Some(take_number),
            capture_pid: AtomicU32::new(child.id()),
            capture_running: AtomicBool::new(true),
            capture_stop: Mutex::new(Some(child.stop.clone())),
            worker_pid: AtomicU32::new(0),
            capture_done: AtomicBool::new(false),
            worker_done: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            device: device.to_owned(),
            resumable: input.is_none(),
            transition: Mutex::new(()),
            samples: AtomicU64::new(initial),
            hold: Mutex::new(None),
        });
        if let Err(error) = self.store.lock().unwrap().save_take(&RecordingTake {
            id: Uuid::new_v4().to_string(),
            meeting_id: id.into(),
            number: take_number,
            started_at: chrono::Utc::now().to_rfc3339(),
            start_sample: initial,
            end_sample: None,
            status: "recording".into(),
        }) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        m.status = if live { "recording" } else { "importing" }.into();
        self.store.lock().unwrap().save_meeting(&m)?;
        let previous_status = self.status.lock().unwrap().clone();
        *self.status.lock().unwrap() = RuntimeStatus {
            notes_busy: previous_status.notes_busy,
            translation_busy: previous_status.translation_busy,
            translation_status: previous_status.translation_status,
            notes_status: previous_status.notes_status,
            notes_elapsed_ms: previous_status.notes_elapsed_ms,
            meeting_id: Some(id.into()),
            recording: live,
            elapsed_ms: initial * 1000 / SAMPLE_RATE,
            processed_ms: initial * 1000 / SAMPLE_RATE,
            transcribing: true,
            ..Default::default()
        };
        *active_guard = Some(active.clone());
        drop(active_guard);
        // Hold only for a real capture. File imports and replay finish quickly and
        // should not keep a laptop awake.
        if live {
            active.hold_awake();
        }
        if let Err(e) = self.spawn_asr(active.clone(), m.run, initial, next_seq) {
            let _ = child.kill();
            let _ = child.wait();
            active.release_awake();
            active.capture_done.store(true, Ordering::SeqCst);
            active.worker_done.store(true, Ordering::SeqCst);
            self.finish_take(&active, true)?;
            self.status.lock().unwrap().recording = false;
            self.status.lock().unwrap().transcribing = false;
            m.status = "interrupted".into();
            self.store.lock().unwrap().save_meeting(&m)?;
            return Err(e);
        }
        self.capture_thread(active, child, output, dir, live);
        Ok(())
    }
    fn capture_thread(
        self: &Arc<Self>,
        active: Arc<Active>,
        mut child: crate::capture::Capture,
        output: Box<dyn std::io::Read + Send>,
        dir: PathBuf,
        live: bool,
    ) {
        let engine = self.clone();
        thread::spawn(move || {
            let wall = Instant::now();
            let initial = active.samples.load(Ordering::SeqCst);
            let result = audio::capture_stream_from(output, &dir, initial, |samples, level| {
                active.samples.store(samples, Ordering::SeqCst);
                let mut status = engine.status.lock().unwrap();
                status.elapsed_ms = samples * 1000 / SAMPLE_RATE;
                status.level = level;
            });
            let exit = child.wait();
            let _transition = active.transition.lock().unwrap();
            active.capture_pid.store(0, Ordering::SeqCst);
            active.capture_running.store(false, Ordering::SeqCst);
            let count = active.samples.load(Ordering::SeqCst);
            if active.paused.load(Ordering::SeqCst)
                && !active.stopping.load(Ordering::SeqCst)
                && result.is_ok()
                && fs::write(dir.join("capture.paused"), count.to_string()).is_ok()
            {
                let db = engine.store.lock().unwrap();
                if let Ok(mut meeting) = db.meeting(&active.id) {
                    meeting.duration_ms = count * 1000 / SAMPLE_RATE;
                    meeting.status = "paused".into();
                    let _ = db.save_meeting(&meeting);
                }
                drop(db);
                let mut status = engine.status.lock().unwrap();
                status.recording = false;
                status.paused = true;
                status.level = 0.;
                return;
            }
            // Capture is over, whatever the outcome: never keep the machine
            // awake because a take ended abnormally.
            active.release_awake();
            active.capture_done.store(true, Ordering::SeqCst);
            active.paused.store(false, Ordering::SeqCst);
            let _ = fs::remove_file(dir.join("capture.paused"));
            let marker = fs::write(dir.join("capture.done"), count.to_string());
            let stopped = active.stopping.load(Ordering::SeqCst);
            let mut failure = result.err().map(|e| format!("录音保存失败：{e:#}"));
            if marker.is_err() {
                failure = Some("无法写入录音结束标记".into());
            }
            if count == initial {
                failure = Some("没有采集到音频，请检查麦克风或导入文件".into());
            }
            if !stopped && exit.as_ref().is_ok_and(|s| !*s) {
                failure = Some("音频输入异常结束，已保留采集到的音频".into());
            }
            if live
                && wall.elapsed().as_secs_f64() - ((count - initial) as f64 / SAMPLE_RATE as f64)
                    > 3.0
                && count > 0
            {
                failure = Some("录音期间检测到输入时间缺口，请回听确认；已有音频已保存".into());
            }
            if let Err(e) = engine.finish_take(&active, failure.is_some()) {
                failure = Some(format!("录音片段保存失败：{e}"));
            }
            let db = engine.store.lock().unwrap();
            if let Ok(mut meeting) = db.meeting(&active.id) {
                meeting.duration_ms = count * 1000 / SAMPLE_RATE;
                meeting.status = if failure.is_some() {
                    "interrupted"
                } else if active.worker_done.load(Ordering::SeqCst) {
                    "ready"
                } else {
                    "transcribing"
                }
                .into();
                let _ = db.save_meeting(&meeting);
            }
            drop(db);
            let mut status = engine.status.lock().unwrap();
            status.recording = false;
            status.paused = false;
            status.level = 0.;
            if let Some(error) = failure {
                let _ = fs::write(dir.join("capture.error"), &error);
                status.error = Some(error);
            }
            drop(status);
            engine.sync_meeting_files(&active.id);
        });
    }
    fn spawn_asr(
        self: &Arc<Self>,
        active: Arc<Active>,
        run: u32,
        start_sample: u64,
        next_seq: u32,
    ) -> Result<()> {
        let dir = self.audio_dir(&active.id)?;
        let boundaries = self
            .store
            .lock()
            .unwrap()
            .takes(&active.id)?
            .iter()
            .filter_map(|take| take.end_sample)
            .filter(|end| *end > start_sample && *end < active.samples.load(Ordering::SeqCst))
            .map(|end| end.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let prefs = self.preferences.lock().unwrap().clone();
        let hotwords = dir.join("hotwords.txt");
        fs::write(
            &hotwords,
            self.terminology(&active.id)?
                .hotwords
                .join("\n")
                .to_uppercase(),
        )?;
        let mut child = audio::child_command(self.worker_path())
            .arg(self.model_directory(&prefs.asr_model)?)
            .arg(&dir)
            .arg(start_sample.to_string())
            .arg(next_seq.to_string())
            .arg(boundaries)
            .arg(hotwords)
            // The quality model has the same token inventory but no bundled tokenizer.
            // The worker verifies the inventories before reusing this local asset.
            .arg(self.model_directory("zipformer-small").unwrap_or_default())
            .env("OMASCRIBE_ASR_THREADS", prefs.threads.to_string())
            .env("OMP_NUM_THREADS", "1")
            .env("OPENBLAS_NUM_THREADS", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(File::create(dir.join(format!("asr-{run}.log")))?)
            .spawn()?;
        active.worker_pid.store(child.id(), Ordering::SeqCst);
        let output = child.stdout.take().context("无法连接转写进程")?;
        let engine = self.clone();
        thread::spawn(move || {
            let mut complete = false;
            let mut partial_terms = engine.terminology(&active.id).ok();
            let mut terms_refreshed = Instant::now();
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                let Ok(event) = serde_json::from_str::<AsrEvent>(&line) else {
                    continue;
                };
                match event {
                    AsrEvent::Ready => {}
                    AsrEvent::Partial { text } => {
                        // A live caption uses the same spellings as a finalized passage.
                        // Refresh at most once a second, not on every decoder event.
                        if terms_refreshed.elapsed() >= Duration::from_secs(1) {
                            partial_terms = engine.terminology(&active.id).ok();
                            terms_refreshed = Instant::now();
                        }
                        engine.status.lock().unwrap().partial = partial_terms
                            .as_ref()
                            .map_or_else(|| text.clone(), |terms| terms.normalize(&text));
                    }
                    AsrEvent::Progress { processed_ms, rtf } => {
                        let mut s = engine.status.lock().unwrap();
                        s.processed_ms = processed_ms;
                        s.asr_rtf = rtf;
                    }
                    AsrEvent::Boundary { processed_ms, .. } => {
                        let mut status = engine.status.lock().unwrap();
                        status.processed_ms = processed_ms;
                        status.partial.clear();
                    }
                    AsrEvent::Segment {
                        seq,
                        start_ms,
                        end_ms,
                        text,
                    } => {
                        let segment = Segment {
                            id: format!("{}:{run}:{seq}", active.id),
                            meeting_id: active.id.clone(),
                            run,
                            seq,
                            start_ms,
                            end_ms,
                            text,
                            revision: 1,
                            speaker: None,
                            speaker_manual: false,
                        };
                        if let Err(e) = engine.store.lock().unwrap().save_segment(&segment) {
                            engine.status.lock().unwrap().error =
                                Some(format!("转写保存失败：{e}"));
                        }
                        if let Err(e) = engine.correct_terminology(&active.id) {
                            engine.status.lock().unwrap().error =
                                Some(format!("术语纠正失败：{e}"));
                        }
                        engine.sync_meeting_files(&active.id);
                    }
                    AsrEvent::Done { processed_ms, rtf } => {
                        complete = true;
                        let mut s = engine.status.lock().unwrap();
                        s.processed_ms = processed_ms;
                        s.asr_rtf = rtf;
                        s.partial.clear();
                    }
                    AsrEvent::Error { message } => {
                        engine.status.lock().unwrap().error = Some(message)
                    }
                }
            }
            let exit = child.wait();
            active.worker_pid.store(0, Ordering::SeqCst);
            let _transition = active.transition.lock().unwrap();
            let db = engine.store.lock().unwrap();
            if let Ok(mut m) = db.meeting(&active.id) {
                m.duration_ms = active.samples.load(Ordering::SeqCst) * 1000 / SAMPLE_RATE;
                if active.capture_done.load(Ordering::SeqCst) {
                    m.status = if complete && !dir.join("capture.error").exists() {
                        "ready"
                    } else {
                        "interrupted"
                    }
                    .into();
                }
                let _ = db.save_meeting(&m);
            }
            drop(db);
            engine.sync_meeting_files(&active.id);
            let mut s = engine.status.lock().unwrap();
            s.transcribing = false;
            s.partial.clear();
            if !complete || exit.as_ref().is_ok_and(|x| !x.success()) {
                s.error = Some("转写进程中断，录音仍独立保存；结束后可重新转写".into());
            }
            active.worker_done.store(true, Ordering::SeqCst);
        });
        Ok(())
    }
    pub fn pause_recording(&self) -> Result<()> {
        let guard = self.active.lock().unwrap();
        let active = guard.as_ref().context("当前没有录音")?;
        let _transition = active.transition.lock().unwrap();
        if !active.resumable || active.capture_done.load(Ordering::SeqCst) {
            bail!("当前录音不能暂停");
        }
        if active.stopping.load(Ordering::SeqCst) {
            bail!("录音正在结束");
        }
        active.paused.store(true, Ordering::SeqCst);
        let pid = active.capture_pid.load(Ordering::SeqCst);
        if let Some(stop) = active.capture_stop.lock().unwrap().as_ref() {
            stop.store(true, Ordering::SeqCst);
        }
        if active.capture_running.load(Ordering::SeqCst) {
            platform::stop_process(pid, true);
        }
        // A pause may last a long time; let the desktop idle again.
        active.release_awake();
        Ok(())
    }
    pub fn resume_recording(self: &Arc<Self>) -> Result<()> {
        let active = self
            .active
            .lock()
            .unwrap()
            .as_ref()
            .cloned()
            .context("当前没有暂停的录音")?;
        let transition = active.transition.lock().unwrap();
        if !active.paused.load(Ordering::SeqCst)
            || active.stopping.load(Ordering::SeqCst)
            || !active.resumable
        {
            bail!("当前录音不能继续");
        }
        if active.capture_running.load(Ordering::SeqCst) || !self.status.lock().unwrap().paused {
            bail!("正在暂停，请稍后继续");
        }
        let dir = self.audio_dir(&active.id)?;
        let mut child = audio::spawn_capture(&active.device, None, None, &dir.join("capture.log"))?;
        let output = child.stdout.take().context("无法读取录音流")?;
        active.capture_pid.store(child.id(), Ordering::SeqCst);
        active.capture_running.store(true, Ordering::SeqCst);
        *active.capture_stop.lock().unwrap() = Some(child.stop.clone());
        active.paused.store(false, Ordering::SeqCst);
        let _ = fs::remove_file(dir.join("capture.paused"));
        active.hold_awake();
        let db = self.store.lock().unwrap();
        let mut meeting = db.meeting(&active.id)?;
        meeting.status = "recording".into();
        db.save_meeting(&meeting)?;
        drop(db);
        {
            let mut status = self.status.lock().unwrap();
            status.recording = true;
            status.paused = false;
        }
        drop(transition);
        self.capture_thread(active, child, output, dir, true);
        Ok(())
    }
    pub fn stop_recording(&self) -> Result<()> {
        let guard = self.active.lock().unwrap();
        let active = guard.as_ref().context("当前没有录音")?;
        let _transition = active.transition.lock().unwrap();
        active.stopping.store(true, Ordering::SeqCst);
        let pid = active.capture_pid.load(Ordering::SeqCst);
        if let Some(stop) = active.capture_stop.lock().unwrap().as_ref() {
            stop.store(true, Ordering::SeqCst);
        }
        if active.capture_running.load(Ordering::SeqCst) {
            platform::stop_process(pid, true);
        } else if active.paused.swap(false, Ordering::SeqCst) {
            let dir = self.audio_dir(&active.id)?;
            active.release_awake();
            self.finish_take(active, false)?;
            let db = self.store.lock().unwrap();
            let mut meeting = db.meeting(&active.id)?;
            meeting.status = "transcribing".into();
            db.save_meeting(&meeting)?;
            fs::write(
                dir.join("capture.done"),
                active.samples.load(Ordering::SeqCst).to_string(),
            )?;
            let _ = fs::remove_file(dir.join("capture.paused"));
            active.capture_done.store(true, Ordering::SeqCst);
            drop(db);
            self.status.lock().unwrap().paused = false;
        }
        Ok(())
    }
    pub fn retranscribe(self: &Arc<Self>, id: &str) -> Result<()> {
        let _gate = self.config_lock.lock().unwrap();
        self.configuration_idle()?;
        let mut guard = self.active.lock().unwrap();
        if guard.as_ref().is_some_and(|a| {
            !a.capture_done.load(Ordering::SeqCst) || !a.worker_done.load(Ordering::SeqCst)
        }) {
            bail!("请等待当前录音和转写结束");
        }
        if self.note_busy.load(Ordering::SeqCst) {
            bail!("请先等待或取消笔记生成");
        }
        let dir = self.audio_dir(id)?;
        let samples = audio::samples_on_disk(&dir)?;
        if samples == 0 {
            bail!("这场会议没有可转写的音频");
        }
        fs::write(dir.join("capture.done"), samples.to_string())?;
        let db = self.store.lock().unwrap();
        let mut m = db.meeting(id)?;
        m.run += 1;
        m.diarization_cursor = 0;
        m.speaker_names.clear();
        m.notes_cursor = 0;
        m.status = "transcribing".into();
        db.save_meeting(&m)?;
        for mut s in db.suggestions(id)? {
            s.stale = true;
            db.save_suggestion(&s)?;
        }
        drop(db);
        let active = Arc::new(Active {
            id: id.into(),
            take_number: None,
            capture_pid: AtomicU32::new(0),
            capture_running: AtomicBool::new(false),
            capture_stop: Mutex::new(None),
            worker_pid: AtomicU32::new(0),
            capture_done: AtomicBool::new(true),
            worker_done: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            device: "auto".into(),
            resumable: false,
            transition: Mutex::new(()),
            samples: AtomicU64::new(samples),
            hold: Mutex::new(None),
        });
        *guard = Some(active.clone());
        drop(guard);
        *self.status.lock().unwrap() = RuntimeStatus {
            meeting_id: Some(id.into()),
            elapsed_ms: samples * 1000 / SAMPLE_RATE,
            transcribing: true,
            ..Default::default()
        };
        self.spawn_asr(active, m.run, 0, 1)
    }
    pub fn generate_notes(self: &Arc<Self>, id: &str) -> Result<()> {
        self.correct_terminology(id)?;
        let _config = self.config_lock.lock().unwrap();
        self.require_available()?;
        if self.status.lock().unwrap().diarization_busy {
            bail!("Wait for speaker analysis to finish");
        }
        self.store.lock().unwrap().meeting(id)?;
        if self.note_busy.swap(true, Ordering::SeqCst) {
            bail!("已有笔记正在生成");
        }
        self.note_cancel.store(false, Ordering::SeqCst);
        {
            let mut status = self.status.lock().unwrap();
            status.notes_busy = true;
            status.notes_status = "正在整理会议文档…".into();
        }
        let engine = self.clone();
        let id = id.to_string();
        thread::spawn(move || {
            let result = notes::generate(&engine, &id);
            engine.finish_minutes(result);
            engine.note_busy.store(false, Ordering::SeqCst);
        });
        Ok(())
    }
    pub(crate) fn finish_minutes(&self, result: Result<usize>) {
        if result.is_ok() {
            self.note_failures.store(0, Ordering::SeqCst);
        } else {
            self.note_failures.fetch_add(1, Ordering::SeqCst);
        }
        let mut status = self.status.lock().unwrap();
        status.notes_busy = false;
        status.notes_status = match result {
            Ok(n) => format!("文档已同步 · {n} 个章节"),
            Err(e) => format!("{e:#}"),
        };
    }
    pub fn inference_worker_status(&self) -> crate::WorkerStatus {
        self.inference_pool.status()
    }
    pub fn cancel_notes(&self) {
        self.note_cancel.store(true, Ordering::SeqCst);
        self.inference_pool.invalidate();
        let pid = self.llm_pid.load(Ordering::SeqCst);
        if pid > 0 {
            platform::stop_process(pid, false);
        }
    }
    pub fn clip(&self, id: &str, start_ms: u64, end_ms: u64) -> Result<String> {
        let pcm = audio::pcm_range(&self.audio_dir(id)?, start_ms, end_ms)?;
        Ok(base64::engine::general_purpose::STANDARD.encode(audio::wav_bytes(&pcm)?))
    }
    pub fn export(&self, id: &str, audio: bool) -> Result<String> {
        let data = self.snapshot(id)?;
        let folder = self.root.join("exports");
        fs::create_dir_all(&folder)?;
        let stem = format!(
            "meeting-{}-{}",
            data.meeting.created_at.get(..10).unwrap_or("export"),
            &id[..8]
        );
        let target = folder.join(format!("{stem}.md"));
        let mut text = format!(
            "# {}\n\n{} · OmaScribe\n\n## Your notes / 个人笔记\n\n",
            data.meeting.title, data.meeting.created_at
        );
        for n in notes::document_notes(&data.notes) {
            if !n.content.trim().is_empty() {
                text.push_str(&n.content);
                text.push_str("\n\n");
            }
        }
        text.push_str("\n## Meeting minutes / 会议纪要\n\n");
        let mut footnotes = String::new();
        for (index, n) in data
            .suggestions
            .iter()
            .filter(|n| n.status != "dismissed")
            .enumerate()
        {
            let reference = format!("minutes-{}", index + 1);
            text.push_str(&format!("{}[^{reference}]\n\n", n.text.trim()));
            if n.stale {
                text.push_str(
                    "> Source revised; review this paragraph. / 原文已修订，请复核本段。\n\n",
                );
            }
            footnotes.push_str(&format!("[^{reference}]: "));
            for (source_index, s) in n.sources.iter().enumerate() {
                if source_index > 0 {
                    footnotes.push_str("\n    ");
                }
                footnotes.push_str(&format!(
                    "[{}] {} (revision {})\n",
                    timestamp(s.start_ms),
                    format_args!(
                        "{}{}",
                        s.speaker_name
                            .as_ref()
                            .map(|n| format!("{n}: "))
                            .unwrap_or_default(),
                        s.quote.replace('\n', " ")
                    ),
                    s.revision
                ));
            }
            footnotes.push('\n');
        }
        text.push_str(&footnotes);
        text.push_str("\n## Transcript / 转写\n\n");
        for s in &data.segments {
            text.push_str(&format!(
                "**[{}]** {}\n\n",
                timestamp(s.start_ms),
                crate::speakers::labeled_text(s, &data.meeting)
            ));
        }
        let tmp = target.with_extension("md.part");
        fs::write(&tmp, text)?;
        fs::rename(tmp, &target)?;
        if audio {
            audio::export_wav(&self.audio_dir(id)?, &folder.join(format!("{stem}.wav")))?;
        }
        Ok(target.to_string_lossy().into_owned())
    }
    pub fn diagnostics(&self) -> Diagnostics {
        let prefs = self.preferences.lock().unwrap().clone();
        let names = [
            "encoder-epoch-99-avg-1.int8.onnx",
            "decoder-epoch-99-avg-1.onnx",
            "joiner-epoch-99-avg-1.int8.onnx",
            "tokens.txt",
        ];
        let mem = platform::available_memory_mib();
        // Follow the live Omarchy theme when there is one; otherwise keep the
        // built-in accent so the app is not styled by a stale copied color.
        let theme = crate::desktheme::current();
        let accent = theme
            .as_ref()
            .map(|t| t.accent.clone())
            .unwrap_or_else(|| "#e5b866".into());
        Diagnostics {
            data_dir: self.root.to_string_lossy().into_owned(),
            asr_ready: names.iter().all(|n| {
                self.model_directory(&prefs.asr_model)
                    .is_ok_and(|path| path.join(n).is_file())
            }),
            llm_ready: self.model_file(&prefs.llm_model).is_ok(),
            recorder_ready: cfg!(any(target_os = "macos", target_os = "windows"))
                || Path::new("/usr/bin/pw-record").exists(),
            worker_ready: self.worker_path().exists(),
            server_ready: crate::inference::resolve(&prefs, &self.runtime_dir).is_ok_and(
                |backend| {
                    if backend == crate::inference::Backend::Mlx {
                        self.runtime_dir.join("mlx/venv/bin/python3").is_file()
                    } else {
                        self.runtime_dir
                            .join("llama")
                            .join(platform::executable("llama-server"))
                            .is_file()
                    }
                },
            ),
            available_memory_mib: mem,
            accent,
            desktop_theme: theme,
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }
    pub fn shutdown(&self) {
        self.microphone_monitor.stop();
        self.cancel_all_downloads();
        let _ = self.stop_recording();
        self.cancel_notes();
        // Drop the session hold immediately; shutdown must not wait for a drain.
        if let Some(active) = self.active.lock().unwrap().as_ref() {
            active.release_awake();
        }
        platform::stop_process(self.diarization_pid.load(Ordering::SeqCst), false);
        for _ in 0..30 {
            if self
                .active
                .lock()
                .unwrap()
                .as_ref()
                .is_none_or(|a| a.capture_done.load(Ordering::SeqCst))
            {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        if let Some(a) = self.active.lock().unwrap().as_ref() {
            for p in [&a.capture_pid, &a.worker_pid] {
                let pid = p.load(Ordering::SeqCst);
                if pid > 0 {
                    platform::stop_process(pid, false);
                }
            }
        }
    }
}
pub fn timestamp(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn unfinished_capture_recovers_audio_and_does_not_allow_two_writers() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let engine = Engine::open(tmp.path().into())?;
        assert!(Engine::open(tmp.path().into()).is_err());
        let mut meeting = engine.create_meeting("Recovery test")?;
        meeting.status = "recording".into();
        engine.store.lock().unwrap().save_meeting(&meeting)?;
        let dir = engine.audio_dir(&meeting.id)?;
        fs::write(dir.join("000000.pcm"), vec![0u8; 32_000])?;
        drop(engine);
        let reopened = Engine::open(tmp.path().into())?;
        let result = reopened.snapshot(&meeting.id)?;
        assert_eq!(result.meeting.status, "interrupted");
        assert_eq!(result.meeting.duration_ms, 1000);
        assert_eq!(fs::metadata(dir.join("000000.pcm"))?.len(), 32_000);
        assert!(dir.join("capture.done").exists());
        let takes = reopened.store.lock().unwrap().takes(&meeting.id)?;
        assert_eq!(takes.len(), 1);
        assert_eq!(takes[0].start_sample, 0);
        assert_eq!(takes[0].end_sample, Some(16_000));
        let files = reopened.meeting_files(&meeting.id)?;
        let wav = files.files.iter().find(|f| f.kind == "audio").unwrap();
        assert_eq!(hound::WavReader::open(&wav.path)?.duration(), 16_000);
        drop(reopened);
        let reopened = Engine::open(tmp.path().into())?;
        assert_eq!(reopened.store.lock().unwrap().takes(&meeting.id)?.len(), 1);
        Ok(())
    }
}

#[cfg(test)]
mod note_size_tests {
    use super::*;
    #[test]
    fn long_markdown_is_preserved_and_oversize_edits_do_not_replace_saved_text() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().to_path_buf())?;
        let meeting = engine.create_meeting("Document integrity")?;
        let content = "会议记录".repeat(8_000);
        let saved = engine.save_note(&meeting.id, None, &content, "document", 0)?;
        assert_eq!(saved.content, content);
        assert!(engine
            .save_note(
                &meeting.id,
                Some(saved.id),
                &"字".repeat(200_001),
                "document",
                0
            )
            .is_err());
        assert_eq!(engine.snapshot(&meeting.id)?.notes[0].content, content);
        Ok(())
    }
}
