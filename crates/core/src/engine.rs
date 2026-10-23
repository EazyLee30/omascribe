use crate::{audio, notes, store::Store, types::*};
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
    pub capture_pid: AtomicU32,
    pub worker_pid: AtomicU32,
    pub capture_done: AtomicBool,
    pub worker_done: AtomicBool,
    pub stopping: AtomicBool,
    pub paused: AtomicBool,
    pub device: String,
    pub resumable: bool,
    transition: Mutex<()>,
    pub samples: AtomicU64,
}
pub struct Engine {
    pub root: PathBuf,
    pub models: PathBuf,
    pub runtime_dir: PathBuf,
    pub store: Mutex<Store>,
    pub status: Mutex<RuntimeStatus>,
    pub active: Mutex<Option<Arc<Active>>>,
    pub note_busy: AtomicBool,
    pub note_cancel: AtomicBool,
    pub llm_pid: AtomicU32,
    notes_target: Mutex<Option<String>>,
    _lock: File,
}
pub fn default_root() -> PathBuf {
    if let Some(path) = std::env::var_os("OMASCRIBE_DATA_DIR") {
        return PathBuf::from(path);
    }
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        })
        .join("omascribe")
}
impl Engine {
    pub fn open(root: PathBuf) -> Result<Arc<Self>> {
        use std::os::{fd::AsRawFd, unix::fs::PermissionsExt};
        fs::create_dir_all(root.join("meetings"))?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join("app.lock"))?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!("OmaScribe 已在运行，请切换到已打开的窗口");
        }
        let models = std::env::var_os("OMASCRIBE_MODELS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("models"));
        let runtime_dir = std::env::var_os("OMASCRIBE_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("runtime"));
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
        Ok(Arc::new(Self {
            root,
            models,
            runtime_dir,
            store: Mutex::new(store),
            status: Mutex::new(RuntimeStatus::default()),
            active: Mutex::new(None),
            note_busy: AtomicBool::new(false),
            note_cancel: AtomicBool::new(false),
            llm_pid: AtomicU32::new(0),
            notes_target: Mutex::new(None),
            _lock: lock,
        }))
    }
    pub fn start_scheduler(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        thread::spawn(move || {
            let mut attempted = std::collections::HashMap::<String, Instant>::new();
            let mut in_flight: Option<String> = None;
            loop {
                thread::sleep(Duration::from_secs(2));
                let Some(engine) = weak.upgrade() else { break };
                if engine.note_busy.load(Ordering::SeqCst) {
                    continue;
                }
                if let Some(id) = in_flight.take() {
                    attempted.insert(id, Instant::now());
                }
                let status = engine.status.lock().unwrap().clone();
                if status.transcribing
                    && status.elapsed_ms.saturating_sub(status.processed_ms) > 2500
                {
                    continue;
                }
                let mut ids = Vec::new();
                if let Some(id) = status.meeting_id {
                    ids.push(id);
                }
                if let Some(id) = engine.notes_target.lock().unwrap().clone() {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                if let Ok(meetings) = engine.meetings() {
                    for meeting in meetings {
                        if meeting.auto_notes && !ids.contains(&meeting.id) {
                            ids.push(meeting.id);
                        }
                    }
                }
                for id in ids {
                    let Ok(snapshot) = engine.snapshot(&id) else {
                        continue;
                    };
                    if !snapshot.meeting.auto_notes || !notes::needs_update(&snapshot) {
                        continue;
                    }
                    // Coalesce bursts, retry failed jobs without advancing their coverage, and never
                    // require an explicit UI action. A finished recording gets the next free slot.
                    if attempted
                        .get(&id)
                        .is_some_and(|t| t.elapsed() < Duration::from_secs(45))
                    {
                        continue;
                    }
                    let editing = snapshot
                        .notes
                        .iter()
                        .filter_map(|n| chrono::DateTime::parse_from_rfc3339(&n.updated_at).ok())
                        .any(|t| chrono::Utc::now().signed_duration_since(t).num_seconds() < 5);
                    if editing {
                        continue;
                    }
                    attempted.insert(id.clone(), Instant::now());
                    if engine.generate_notes(&id).is_ok() {
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
            auto_notes: true,
            notes_fingerprint: String::new(),
        };
        fs::create_dir_all(self.audio_dir(&m.id)?)?;
        self.store.lock().unwrap().save_meeting(&m)?;
        Ok(m)
    }
    pub fn meetings(&self) -> Result<Vec<Meeting>> {
        self.store.lock().unwrap().meetings()
    }
    pub fn snapshot(&self, id: &str) -> Result<Snapshot> {
        let db = self.store.lock().unwrap();
        let mut m = db.meeting(id)?;
        let runtime = self.status.lock().unwrap().clone();
        if runtime.meeting_id.as_deref() == Some(id) {
            m.duration_ms = m.duration_ms.max(runtime.elapsed_ms);
        }
        Ok(Snapshot {
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
        *self.notes_target.lock().unwrap() = Some(id.to_owned());
        Ok(n)
    }
    pub fn edit_segment(&self, id: &str, text: &str, revision: u32) -> Result<()> {
        if text.chars().count() > 4000 {
            bail!("片段文字过长");
        }
        self.store.lock().unwrap().edit_segment(id, text, revision)
    }
    pub fn worker_path(&self) -> PathBuf {
        let installed = self.runtime_dir.join("omascribe-asr");
        if installed.exists() {
            return installed;
        }
        std::env::current_exe()
            .unwrap_or_default()
            .with_file_name("omascribe-asr")
    }
    pub fn start_recording(
        self: &Arc<Self>,
        id: &str,
        device: &str,
        input: Option<PathBuf>,
    ) -> Result<()> {
        self.start_capture(id, device, input, None)
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
        let mut active_guard = self.active.lock().unwrap();
        if active_guard.as_ref().is_some_and(|a| {
            !a.worker_done.load(Ordering::SeqCst) || !a.capture_done.load(Ordering::SeqCst)
        }) {
            bail!("请先结束当前录音或转写");
        }
        let mut m = self.store.lock().unwrap().meeting(id)?;
        if m.duration_ms > 0 {
            bail!("这场会议已有音频，请新建会议后开始录音");
        }
        if !self.diagnostics().asr_ready || !self.worker_path().exists() {
            bail!("离线转写模型或运行时尚未准备好，请运行 bin/setup");
        }
        if let Some(ref p) = input {
            if !p.is_file() {
                bail!("找不到导入的音频文件");
            }
        }
        let dir = self.audio_dir(id)?;
        let _ = fs::remove_file(dir.join("capture.done"));
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
            capture_pid: AtomicU32::new(child.id()),
            worker_pid: AtomicU32::new(0),
            capture_done: AtomicBool::new(false),
            worker_done: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            device: device.to_owned(),
            resumable: input.is_none(),
            transition: Mutex::new(()),
            samples: AtomicU64::new(0),
        });
        m.status = if live { "recording" } else { "importing" }.into();
        self.store.lock().unwrap().save_meeting(&m)?;
        *self.status.lock().unwrap() = RuntimeStatus {
            meeting_id: Some(id.into()),
            recording: live,
            transcribing: true,
            ..Default::default()
        };
        *active_guard = Some(active.clone());
        drop(active_guard);
        if let Err(e) = self.spawn_asr(active.clone(), m.run) {
            let _ = child.kill();
            let _ = child.wait();
            active.capture_done.store(true, Ordering::SeqCst);
            active.worker_done.store(true, Ordering::SeqCst);
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
        mut child: std::process::Child,
        output: std::process::ChildStdout,
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
            active.capture_done.store(true, Ordering::SeqCst);
            active.paused.store(false, Ordering::SeqCst);
            let _ = fs::remove_file(dir.join("capture.paused"));
            let marker = fs::write(dir.join("capture.done"), count.to_string());
            let stopped = active.stopping.load(Ordering::SeqCst);
            let mut failure = result.err().map(|e| format!("录音保存失败：{e:#}"));
            if marker.is_err() {
                failure = Some("无法写入录音结束标记".into());
            }
            if count == 0 {
                failure = Some("没有采集到音频，请检查麦克风或导入文件".into());
            }
            if !stopped && exit.as_ref().is_ok_and(|s| !s.success()) {
                failure = Some("音频输入异常结束，已保留采集到的音频".into());
            }
            if live
                && wall.elapsed().as_secs_f64() - ((count - initial) as f64 / SAMPLE_RATE as f64)
                    > 3.0
                && count > 0
            {
                failure = Some("录音期间检测到输入时间缺口，请回听确认；已有音频已保存".into());
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
        });
    }
    fn spawn_asr(self: &Arc<Self>, active: Arc<Active>, run: u32) -> Result<()> {
        let dir = self.audio_dir(&active.id)?;
        let mut child = audio::child_command(self.worker_path())
            .arg(self.models.join("zipformer"))
            .arg(&dir)
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
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                let Ok(event) = serde_json::from_str::<AsrEvent>(&line) else {
                    continue;
                };
                match event {
                    AsrEvent::Ready => {}
                    AsrEvent::Partial { text } => engine.status.lock().unwrap().partial = text,
                    AsrEvent::Progress { processed_ms, rtf } => {
                        let mut s = engine.status.lock().unwrap();
                        s.processed_ms = processed_ms;
                        s.asr_rtf = rtf;
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
                        };
                        if let Err(e) = engine.store.lock().unwrap().save_segment(&segment) {
                            engine.status.lock().unwrap().error =
                                Some(format!("转写保存失败：{e}"));
                        }
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
            active.worker_done.store(true, Ordering::SeqCst);
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
            let mut s = engine.status.lock().unwrap();
            s.transcribing = false;
            s.partial.clear();
            if !complete || exit.as_ref().is_ok_and(|x| !x.success()) {
                s.error = Some("转写进程中断，录音仍独立保存；结束后可重新转写".into());
            }
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
        if pid > 0 {
            unsafe {
                libc::kill(pid as i32, libc::SIGINT);
            }
        }
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
        if active.capture_pid.load(Ordering::SeqCst) > 0 || !self.status.lock().unwrap().paused {
            bail!("正在暂停，请稍后继续");
        }
        let dir = self.audio_dir(&active.id)?;
        let mut child = audio::spawn_capture(&active.device, None, None, &dir.join("capture.log"))?;
        let output = child.stdout.take().context("无法读取录音流")?;
        active.capture_pid.store(child.id(), Ordering::SeqCst);
        active.paused.store(false, Ordering::SeqCst);
        let _ = fs::remove_file(dir.join("capture.paused"));
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
        if pid > 0 {
            unsafe {
                libc::kill(pid as i32, libc::SIGINT);
            }
        } else if active.paused.swap(false, Ordering::SeqCst) {
            let dir = self.audio_dir(&active.id)?;
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
            capture_pid: AtomicU32::new(0),
            worker_pid: AtomicU32::new(0),
            capture_done: AtomicBool::new(true),
            worker_done: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            device: "auto".into(),
            resumable: false,
            transition: Mutex::new(()),
            samples: AtomicU64::new(samples),
        });
        *guard = Some(active.clone());
        drop(guard);
        *self.status.lock().unwrap() = RuntimeStatus {
            meeting_id: Some(id.into()),
            elapsed_ms: samples * 1000 / SAMPLE_RATE,
            transcribing: true,
            ..Default::default()
        };
        self.spawn_asr(active, m.run)
    }
    pub fn generate_notes(self: &Arc<Self>, id: &str) -> Result<()> {
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
            engine.llm_pid.store(0, Ordering::SeqCst);
            engine.note_busy.store(false, Ordering::SeqCst);
            let mut s = engine.status.lock().unwrap();
            s.notes_busy = false;
            s.notes_status = match result {
                Ok(n) => format!("文档已同步 · {n} 个章节"),
                Err(e) => format!("{e:#}"),
            };
        });
        Ok(())
    }
    pub fn cancel_notes(&self) {
        self.note_cancel.store(true, Ordering::SeqCst);
        let pid = self.llm_pid.load(Ordering::SeqCst);
        if pid > 0 {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
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
        for n in data.suggestions.iter().filter(|n| n.status != "dismissed") {
            text.push_str(&format!(
                "### {}\n\n{}\n\n",
                if n.focus.is_empty() {
                    "Notes"
                } else {
                    &n.focus
                },
                n.text
            ));
            if n.stale {
                text.push_str(
                    "> Source revised; review this paragraph. / 原文已修订，请复核本段。\n\n",
                );
            }
            for s in &n.sources {
                text.push_str(&format!(
                    "> [{}] {} (revision {})\n",
                    timestamp(s.start_ms),
                    s.quote,
                    s.revision
                ));
            }
            text.push('\n');
        }
        text.push_str("\n## Transcript / 转写\n\n");
        for s in &data.segments {
            text.push_str(&format!("**[{}]** {}\n\n", timestamp(s.start_ms), s.text));
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
        let names = [
            "encoder-epoch-99-avg-1.int8.onnx",
            "decoder-epoch-99-avg-1.onnx",
            "joiner-epoch-99-avg-1.int8.onnx",
            "tokens.txt",
        ];
        let mem = fs::read_to_string("/proc/meminfo")
            .unwrap_or_default()
            .lines()
            .find(|l| l.starts_with("MemAvailable:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            / 1024;
        let accent_path = PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
            .join(".local/state/omarchy/current/theme/colors.toml");
        let accent = fs::read_to_string(accent_path)
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.trim_start().starts_with("accent ="))
                    .map(|l| {
                        l.split('=')
                            .nth(1)
                            .unwrap_or("")
                            .trim()
                            .trim_matches('"')
                            .to_string()
                    })
            })
            .filter(|s| s.starts_with('#') && s.len() == 7)
            .unwrap_or_else(|| "#e5b866".into());
        Diagnostics {
            data_dir: self.root.to_string_lossy().into_owned(),
            asr_ready: names
                .iter()
                .all(|n| self.models.join("zipformer").join(n).is_file()),
            llm_ready: self.models.join(notes::MODEL_FILE).is_file(),
            recorder_ready: Path::new("/usr/bin/pw-record").exists(),
            worker_ready: self.worker_path().exists(),
            server_ready: self.runtime_dir.join("llama/llama-server").exists(),
            available_memory_mib: mem,
            accent,
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }
    pub fn shutdown(&self) {
        let _ = self.stop_recording();
        self.cancel_notes();
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
                    unsafe {
                        libc::kill(pid as i32, libc::SIGTERM);
                    }
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
