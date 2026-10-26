//! One local model process at a time, leased by a job and reclaimed while idle.
use crate::Engine;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::json;
use std::{
    fs::File,
    net::TcpListener,
    ops::Deref,
    process::{Child, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Default)]
pub(crate) struct WorkerPool(Mutex<PoolState>);
#[derive(Default)]
struct PoolState {
    idle: Option<IdleWorker>,
    leased: bool,
    epoch: u64,
    batches: usize,
    starts: u64,
    reuses: u64,
}
struct IdleWorker {
    session: LocalSession,
    key: String,
    expires: Instant,
    ttl: Duration,
}
#[derive(Serialize)]
pub struct WorkerStatus {
    pub leased: bool,
    pub idle_pid: Option<u32>,
    pub starts: u64,
    pub reuses: u64,
}
impl WorkerPool {
    pub fn status(&self) -> WorkerStatus {
        let state = self.0.lock().unwrap();
        WorkerStatus {
            leased: state.leased,
            idle_pid: state.idle.as_ref().map(|i| i.session.server.0.id()),
            starts: state.starts,
            reuses: state.reuses,
        }
    }
    pub fn invalidate(&self) {
        let idle = {
            let mut state = self.0.lock().unwrap();
            state.epoch = state.epoch.wrapping_add(1);
            state.idle.take()
        };
        // Do not wait for a child while holding the pool lock.
        drop(idle);
    }
    pub fn reap_idle(&self, now: Instant) {
        let idle = {
            let mut state = self.0.lock().unwrap();
            if state.idle.as_ref().is_some_and(|i| i.expires <= now) {
                state.idle.take()
            } else {
                None
            }
        };
        drop(idle);
    }
    pub fn batch(&self) -> Batch<'_> {
        self.0.lock().unwrap().batches += 1;
        Batch(self)
    }
}
/// Allow adjacent ready tasks to reuse weights even when idle keep-alive is off.
pub(crate) struct Batch<'a>(&'a WorkerPool);
impl Drop for Batch<'_> {
    fn drop(&mut self) {
        let idle = {
            let mut state = self.0 .0.lock().unwrap();
            state.batches = state.batches.saturating_sub(1);
            if state.batches == 0 && state.idle.as_ref().is_some_and(|i| i.ttl.is_zero()) {
                state.idle.take()
            } else {
                None
            }
        };
        drop(idle);
    }
}
pub(crate) struct SessionLease {
    session: Option<LocalSession>,
    engine: Weak<Engine>,
    key: String,
    epoch: u64,
    ttl: Duration,
    pub reused: bool,
    pub startup_ms: u64,
    usable: std::cell::Cell<bool>,
}
impl SessionLease {
    pub fn begin_request(&self) {
        self.usable.set(false);
    }
    pub fn complete_request(&self) {
        self.usable.set(true);
    }
}
impl Deref for SessionLease {
    type Target = LocalSession;
    fn deref(&self) -> &Self::Target {
        self.session.as_ref().unwrap()
    }
}
impl Drop for SessionLease {
    fn drop(&mut self) {
        let Some(mut session) = self.session.take() else {
            return;
        };
        let Some(engine) = self.engine.upgrade() else {
            return;
        };
        if session.backend.optional_gpu()
            && !self.usable.get()
            && !engine.note_cancel.load(Ordering::SeqCst)
            && !session.resource_stop.load(Ordering::SeqCst)
            && !engine.maintenance.load(Ordering::SeqCst)
        {
            crate::acceleration::reject(&engine, "runtime_request_failed", true);
        }
        let healthy = self.usable.get()
            && session.healthy()
            && !engine.note_cancel.load(Ordering::SeqCst)
            && !engine.maintenance.load(Ordering::SeqCst)
            && crate::platform::available_memory_mib().is_some_and(|m| m >= 768);
        *session.active_since.lock().unwrap() = None;
        let mut state = engine.inference_pool.0.lock().unwrap();
        state.leased = false;
        if healthy && state.epoch == self.epoch && (!self.ttl.is_zero() || state.batches > 0) {
            let lifetime = if self.ttl.is_zero() {
                Duration::from_secs(2)
            } else {
                self.ttl
            };
            state.idle = Some(IdleWorker {
                session,
                key: self.key.clone(),
                expires: Instant::now() + lifetime,
                ttl: self.ttl,
            });
        }
        // The local session (when not reused) is dropped after this lock guard.
    }
}
impl LocalSession {
    fn healthy(&mut self) -> bool {
        !self.resource_stop.load(Ordering::SeqCst)
            && !self.timed_out.load(Ordering::SeqCst)
            && !self.server.1.load(Ordering::SeqCst)
            && matches!(self.server.0.try_wait(), Ok(None))
    }
}
fn worker_key(
    engine: &Engine,
    prefs: &crate::preferences::Preferences,
    meeting: &str,
    run: u32,
    spec: &crate::inference::LaunchSpec,
) -> Result<String> {
    use sha2::{Digest, Sha256};
    let model = engine.model_file(&prefs.llm_model)?;
    let metadata = model.metadata()?;
    let key = json!({
        "model":prefs.llm_model,"path":model,"bytes":metadata.len(),
        "modified":metadata.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_nanos().to_string(),
        "backend":spec.backend.name(),"component":spec.identity,"device":spec.device,"runtime":std::fs::read(spec.directory.join("build-info.json")).ok(),
        "threads":prefs.threads,"context":prefs.context_size,"mode":prefs.resource_mode,
        "budget":crate::inference::budget_mib(prefs),"meeting":meeting,"run":run,
    });
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&key)?)))
}
pub(crate) fn open_session(
    engine: &Arc<Engine>,
    prefs: &crate::preferences::Preferences,
    meeting: &str,
    run: u32,
) -> Result<SessionLease> {
    let ttl = if prefs.resource_mode == "eco" {
        0
    } else {
        prefs.llm_keep_alive_seconds
    };
    let spec = crate::inference::launch_spec(engine, prefs)?;
    let key = worker_key(engine, prefs, meeting, run, &spec)?;
    let result = checkout(engine, key, Duration::from_secs(ttl.into()), || {
        spawn_session_with_spec(engine, prefs, &spec)
    });
    if result.is_err()
        && spec.backend.optional_gpu()
        && !engine.note_cancel.load(Ordering::SeqCst)
        && !engine.inference_pool.status().leased
    {
        crate::acceleration::reject(engine, "runtime_start_failed", false);
        engine.inference_pool.invalidate();
        let mut cpu = prefs.clone();
        cpu.inference_backend = "cpu".into();
        return open_session(engine, &cpu, meeting, run);
    }
    result
}
fn checkout(
    engine: &Arc<Engine>,
    key: String,
    ttl: Duration,
    spawn: impl FnOnce() -> Result<LocalSession>,
) -> Result<SessionLease> {
    let started = Instant::now();
    let (idle, epoch) = {
        let mut state = engine.inference_pool.0.lock().unwrap();
        if state.leased {
            bail!("Local inference worker is already leased");
        }
        state.leased = true;
        (state.idle.take(), state.epoch)
    };
    let mut reused = false;
    let session = idle.and_then(|mut idle| {
        if idle.key == key
            && idle.expires > Instant::now()
            && idle.session.healthy()
            && crate::platform::available_memory_mib().is_some_and(|m| m >= 768)
        {
            reused = true;
            Some(idle.session)
        } else {
            None
        }
    });
    let session = match session.map(Ok).unwrap_or_else(spawn) {
        Ok(session) => session,
        Err(error) => {
            engine.inference_pool.0.lock().unwrap().leased = false;
            return Err(error);
        }
    };
    *session.active_since.lock().unwrap() = Some(Instant::now());
    {
        let mut state = engine.inference_pool.0.lock().unwrap();
        if reused {
            state.reuses += 1;
        } else {
            state.starts += 1;
        }
    }
    Ok(SessionLease {
        session: Some(session),
        engine: Arc::downgrade(engine),
        key,
        epoch,
        ttl,
        reused,
        startup_ms: started.elapsed().as_millis() as u64,
        usable: std::cell::Cell::new(true),
    })
}

fn check_memory(available: Option<u64>, required: u64) -> Result<()> {
    match available {
        Some(mib) if mib < required => {
            bail!("writer:low-memory:{mib}:{required}")
        }
        None => bail!("writer:memory-unavailable"),
        _ => Ok(()),
    }
}

struct Server(Child, Arc<AtomicBool>, std::sync::Weak<Engine>);
impl Drop for Server {
    fn drop(&mut self) {
        self.1.store(true, Ordering::SeqCst);
        let _ = self.0.kill();
        let _ = self.0.wait();
        if let Some(engine) = self.2.upgrade() {
            let _ =
                engine
                    .llm_pid
                    .compare_exchange(self.0.id(), 0, Ordering::SeqCst, Ordering::SeqCst);
        }
    }
}

pub(crate) struct LocalSession {
    pub agent: ureq::Agent,
    pub base: String,
    pub auth: String,
    pub budget: u64,
    pub backend: crate::inference::Backend,
    pub resource_stop: Arc<AtomicBool>,
    pub budget_stop: Arc<AtomicBool>,
    pub timed_out: Arc<AtomicBool>,
    pub peak_rss_mib: Arc<AtomicU64>,
    active_since: Arc<std::sync::Mutex<Option<Instant>>>,
    server: Server,
}
pub(crate) fn spawn_session_with_spec(
    engine: &Arc<Engine>,
    prefs: &crate::preferences::Preferences,
    spec: &crate::inference::LaunchSpec,
) -> Result<LocalSession> {
    let budget = crate::inference::budget_mib(prefs);
    let estimate = crate::models::model(&prefs.llm_model)?.memory_mib
        + u64::from(prefs.context_size.saturating_sub(2048)) / 4;
    if estimate > budget {
        bail!("writer:resource-budget:{estimate}:{budget}");
    }
    // Loading under heavy swap pressure repeatedly makes the whole desktop unresponsive.
    check_memory(
        crate::platform::available_memory_mib(),
        crate::models::model(&prefs.llm_model)?.memory_mib
            + u64::from(prefs.context_size.saturating_sub(2048)) / 4,
    )?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let token = Uuid::new_v4().to_string();
    let (mut command, backend) =
        crate::inference::server_command_with_spec(engine, prefs, port, &token, spec)?;
    let resource_path = engine.root.join("last-writer-resource.json");
    std::fs::write(
        &resource_path,
        serde_json::to_vec(
            &json!({"backend": backend.name(), "budget_mib": budget, "available_before_mib": crate::platform::available_memory_mib()}),
        )?,
    )?;
    let log = File::create(engine.root.join("llm-runtime.log"))?;
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()
        .context("无法启动本地笔记模型")?;
    let done = Arc::new(AtomicBool::new(false));
    let mut server = Server(child, done.clone(), Arc::downgrade(engine));
    let pid = server.0.id();
    engine.llm_pid.store(pid, Ordering::SeqCst);
    let weak = Arc::downgrade(engine);
    let active_since = Arc::new(std::sync::Mutex::new(Some(Instant::now())));
    let observed_since = active_since.clone();
    let timed_out = Arc::new(AtomicBool::new(false));
    let timeout_flag = timed_out.clone();
    let resource_stop = Arc::new(AtomicBool::new(false));
    let resource_flag = resource_stop.clone();
    let budget_stop = Arc::new(AtomicBool::new(false));
    let budget_flag = budget_stop.clone();
    let peak_rss_mib = Arc::new(AtomicU64::new(0));
    let observed_peak = peak_rss_mib.clone();
    thread::spawn(move || {
        let mut lag_count = 0;
        let mut memory = sysinfo::System::new();
        let process = sysinfo::Pid::from_u32(pid);
        while !done.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(500));
            if done.load(Ordering::SeqCst) {
                break;
            }
            memory.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[process]), true);
            let rss_mib = memory.process(process).map(|p| p.memory() / 1024 / 1024);
            if let Some(mib) = rss_mib {
                observed_peak.fetch_max(mib, Ordering::Relaxed);
            }
            let available_mib = crate::platform::available_memory_mib();
            let over_budget = rss_mib.is_some_and(|m| m > budget);
            let pressure = available_mib.is_some_and(|m| m < 512);
            if over_budget || pressure {
                let _ = std::fs::write(&resource_path, serde_json::to_vec(&json!({"backend":backend.name(), "reason":if over_budget {"process_budget"} else {"system_pressure"}, "budget_mib":budget,"rss_mib":rss_mib,"available_mib":available_mib})).unwrap_or_default());
                budget_flag.store(over_budget, Ordering::SeqCst);
                resource_flag.store(true, Ordering::SeqCst);
                crate::platform::stop_process(pid, false);
                if let Some(engine) = weak.upgrade() {
                    engine.inference_pool.invalidate();
                }
                break;
            }
            let Some(engine) = weak.upgrade() else { break };
            engine.inference_pool.reap_idle(Instant::now());
            if done.load(Ordering::SeqCst) {
                break;
            }
            let active_at = *observed_since.lock().unwrap();
            let s = {
                let mut s = engine.status.lock().unwrap();
                if let Some(since) = active_at {
                    s.notes_elapsed_ms = since.elapsed().as_millis() as u64;
                }
                s.clone()
            };
            if active_at.is_some_and(|since| since.elapsed() > Duration::from_secs(600)) {
                timeout_flag.store(true, Ordering::SeqCst);
                crate::platform::stop_process(pid, false);
                break;
            }
            if s.recording && s.elapsed_ms.saturating_sub(s.processed_ms) > 4000 {
                lag_count += 1;
            } else {
                lag_count = 0;
            }
            if engine.note_cancel.load(Ordering::SeqCst) || lag_count >= 6 {
                engine.note_cancel.store(true, Ordering::SeqCst);
                crate::platform::stop_process(pid, false);
                break;
            }
        }
    });
    // This HTTP client only talks to an authenticated loopback process; proxies are disabled.
    let agent = ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .redirects(0)
        .timeout_connect(Duration::from_secs(3))
        .timeout_write(Duration::from_secs(15))
        // This is an idle-read limit, not a limit on the total streamed response.
        .timeout_read(Duration::from_secs(300))
        .build();
    let base = format!("http://127.0.0.1:{port}");
    let auth = format!("Bearer {token}");
    let start = Instant::now();
    loop {
        if resource_stop.load(Ordering::SeqCst) {
            if budget_stop.load(Ordering::SeqCst) {
                bail!("writer:resource-budget-hit:{budget}");
            }
            bail!("writer:resource-pressure");
        }
        if server.0.try_wait()?.is_some() {
            bail!("笔记模型已停止；原文保留，可以稍后重试");
        }
        if agent
            .get(&format!("{base}/health"))
            .set("Authorization", &auth)
            .timeout(Duration::from_secs(1))
            .call()
            .is_ok()
        {
            if backend.optional_gpu() {
                let log = std::fs::read_to_string(engine.root.join("llm-runtime.log"))
                    .unwrap_or_default();
                if !crate::acceleration::offloaded(&log) {
                    bail!("GPU runtime loaded without model offload");
                }
            }
            break;
        }
        if engine.note_cancel.load(Ordering::SeqCst) {
            bail!("Inference cancelled");
        }
        if start.elapsed() > Duration::from_secs(120) {
            bail!("笔记模型启动超时");
        }
        thread::sleep(Duration::from_millis(200));
    }

    Ok(LocalSession {
        agent,
        base,
        auth,
        budget,
        backend,
        resource_stop,
        budget_stop,
        timed_out,
        peak_rss_mib,
        active_since,
        server,
    })
}

#[cfg(test)]
mod memory_tests {
    use super::*;

    #[test]
    fn writer_requires_measured_headroom() {
        assert!(check_memory(Some(4236), 1400).is_ok());
        assert!(check_memory(Some(1400), 1400).is_ok());
        assert_eq!(
            check_memory(Some(1399), 1400).unwrap_err().to_string(),
            "writer:low-memory:1399:1400"
        );
        assert!(check_memory(Some(0), 1400).is_err());
        assert_eq!(
            check_memory(None, 1400).unwrap_err().to_string(),
            "writer:memory-unavailable"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A real child process verifies ownership and reaping without requiring model weights.
    #[test]
    #[ignore = "child fixture invoked only by lease lifecycle tests"]
    fn worker_child() {
        if std::env::var_os("OMASCRIBE_TEST_WORKER").is_some() {
            thread::sleep(Duration::from_secs(120));
        }
    }
    fn fake(engine: &Arc<Engine>) -> Result<LocalSession> {
        let child = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "inference_worker::tests::worker_child",
                "--ignored",
            ])
            .env("OMASCRIBE_TEST_WORKER", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()?;
        engine.llm_pid.store(child.id(), Ordering::SeqCst);
        Ok(LocalSession {
            agent: ureq::AgentBuilder::new().try_proxy_from_env(false).build(),
            base: String::new(),
            auth: String::new(),
            budget: 1024,
            backend: crate::inference::Backend::Cpu,
            resource_stop: Default::default(),
            budget_stop: Default::default(),
            timed_out: Default::default(),
            peak_rss_mib: Default::default(),
            active_since: Arc::new(Mutex::new(Some(Instant::now()))),
            server: Server(child, Default::default(), Arc::downgrade(engine)),
        })
    }
    fn lease(engine: &Arc<Engine>, key: &str, ttl: u64) -> Result<SessionLease> {
        checkout(engine, key.into(), Duration::from_secs(ttl), || {
            fake(engine)
        })
    }
    #[test]
    fn batch_reuses_one_child_and_zero_ttl_reaps_it_at_batch_end() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let batch = engine.inference_pool.batch();
        let first = lease(&engine, "meeting/run/model", 0)?;
        let pid = first.server.0.id();
        assert!(
            lease(&engine, "other", 0).is_err(),
            "one inference at a time"
        );
        drop(first);
        let second = lease(&engine, "meeting/run/model", 0)?;
        assert!(second.reused);
        assert_eq!(second.server.0.id(), pid);
        drop(second);
        assert_eq!(engine.inference_pool.status().starts, 1);
        drop(batch);
        assert_eq!(engine.llm_pid.load(Ordering::SeqCst), 0);
        assert!(engine.inference_pool.status().idle_pid.is_none());
        Ok(())
    }
    #[test]
    fn new_scope_expiry_and_cancellation_cannot_reuse_a_child() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let first = lease(&engine, "run1", 30)?;
        let pid = first.server.0.id();
        drop(first);
        let second = lease(&engine, "run2", 30)?;
        assert!(!second.reused);
        assert_ne!(second.server.0.id(), pid);
        drop(second);
        engine
            .inference_pool
            .reap_idle(Instant::now() + Duration::from_secs(31));
        assert_eq!(engine.llm_pid.load(Ordering::SeqCst), 0);
        let third = lease(&engine, "run2", 30)?;
        engine.cancel_notes();
        drop(third);
        assert!(engine.inference_pool.status().idle_pid.is_none());
        assert_eq!(engine.llm_pid.load(Ordering::SeqCst), 0);
        Ok(())
    }
    #[test]
    fn broken_gpu_request_is_reaped_and_releases_ownership() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let make = || -> Result<LocalSession> {
            let mut session = fake(&engine)?;
            session.backend = crate::inference::Backend::Cuda;
            Ok(session)
        };
        let before = engine.acceleration_status();
        assert_ne!(before.state, "cpu_fallback");
        let session = checkout(&engine, "gpu-fixture".into(), Duration::from_secs(30), make)?;
        session.begin_request();
        drop(session);
        assert_eq!(engine.acceleration_status().state, "cpu_fallback");
        assert_eq!(engine.llm_pid.load(Ordering::SeqCst), 0);
        assert!(!engine.inference_pool.status().leased);
        Ok(())
    }
    #[test]
    fn invalidation_while_leased_and_failed_spawn_release_ownership() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let first = lease(&engine, "model", 30)?;
        engine.inference_pool.invalidate();
        drop(first);
        assert!(engine.inference_pool.status().idle_pid.is_none());
        assert!(
            checkout(&engine, "model".into(), Duration::ZERO, || anyhow::bail!(
                "spawn failed"
            ))
            .is_err()
        );
        assert!(!engine.inference_pool.status().leased);
        drop(lease(&engine, "model", 0)?);
        assert_eq!(engine.llm_pid.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[test]
    #[ignore = "explicit local model benchmark; requires verified model and runtime directories"]
    fn live_reuse_benchmark() -> Result<()> {
        use std::io::BufReader;
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let prefs = engine.preferences.lock().unwrap().clone();
        let mut rows = vec![];
        for batched in [false, true] {
            let batch = batched.then(|| engine.inference_pool.batch());
            for repeat in 0..2 {
                let job_start = Instant::now();
                let session = open_session(&engine, &prefs, "public-benchmark", 1)?;
                session.begin_request();
                let request_start = Instant::now();
                let mut first = None;
                let response=session.agent.post(&format!("{}/v1/chat/completions",session.base))
                    .set("Authorization",&session.auth).send_json(json!({
                        "messages":[{"role":"system","content":"Translate the supplied English sentence into Chinese. Preserve negation. Return only the translation. /no_think"},
                                    {"role":"user","content":"The launch date is not approved. Morgan will send the report on Thursday."}],
                        "temperature":0,"seed":42,"chat_template_kwargs":{"enable_thinking":false},
                        "max_tokens":96,"stream":true,"stream_options":{"include_usage":true},"cache_prompt":true,
                    }))?;
                let (text, usage) = crate::notes::stream::read_observed(
                    BufReader::new(response.into_reader()),
                    |text| {
                        if !text.is_empty() {
                            first.get_or_insert(request_start.elapsed().as_millis() as u64);
                        }
                    },
                )?;
                session.complete_request();
                anyhow::ensure!(
                    text.contains('不') || text.contains('未') || text.contains("没有"),
                    "negation missing"
                );
                anyhow::ensure!(
                    text.contains("周四") || text.contains("星期四") || text.contains("礼拜四"),
                    "weekday missing"
                );
                rows.push(json!({"batched":batched,"repeat":repeat,"reused":session.reused,
                    "backend":session.backend.name(),"startup_ms":session.startup_ms,
                    "first_token_ms":first,"request_ms":request_start.elapsed().as_millis() as u64,
                    "job_ms":job_start.elapsed().as_millis() as u64,"usage":crate::inference_trace::Usage::parse(&usage)}));
                drop(session);
            }
            drop(batch);
            anyhow::ensure!(
                engine.llm_pid.load(Ordering::SeqCst) == 0,
                "worker did not unload after batch"
            );
        }
        let status = engine.inference_pool.status();
        anyhow::ensure!(
            status.starts == 3 && status.reuses == 1,
            "expected three cold starts and one reuse"
        );
        let report = json!({"schema":1,"measured_at":chrono::Utc::now().to_rfc3339(),
            "model":prefs.llm_model,"runtime_revision":std::fs::read_to_string(engine.runtime_dir.join("llama/build-info.json")).ok(),
            "synthetic_input":true,"runs":rows,"worker":status,
            "limitations":["Four short requests on one machine; not an ASR or minutes quality benchmark.",
                "No energy measurement; no claim of Windows/Linux GPU performance."]});
        if let Some(path) = std::env::var_os("OMASCRIBE_BENCHMARK_OUTPUT") {
            let path = std::path::PathBuf::from(path);
            let path = if path.is_absolute() {
                path
            } else {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .join(path)
            };
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, serde_json::to_vec_pretty(&report)?)?;
        }
        println!("{}", serde_json::to_string(&report)?);
        Ok(())
    }
}
