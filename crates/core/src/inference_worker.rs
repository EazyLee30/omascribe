//! One leased model worker; detached children are reaped outside the pool lock.
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

mod lifecycle;
use lifecycle::{millis, ReleaseReason, WorkerLifecycle};

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
    lifecycle: WorkerLifecycle,
    // One failed scope only: no per-meeting map or persistent hardware ban.
    metal_failure: Option<String>,
    metal_failed_requests: u64,
}
struct IdleWorker {
    session: LocalSession,
    key: String,
    expires: Instant,
    ttl: Duration,
    since: Instant,
}
#[derive(Clone, Serialize)]
pub struct WorkerStatus {
    pub leased: bool,
    pub idle_pid: Option<u32>,
    pub starts: u64,
    pub reuses: u64,
    pub idle_for_ms: Option<u64>,
    pub expires_in_ms: Option<u64>,
    pub lifecycle: WorkerLifecycle,
}
impl WorkerStatus {
    /// A vacant slot is not proof that the previous child has exited: cleanup
    /// may still be waiting outside the pool lock, alongside a newer lease.
    pub fn confirms_idle_exit_since(&self, initial: &Self) -> bool {
        initial.idle_pid.is_some_and(|pid| {
            self.idle_pid.is_none()
                && !self.leased
                && self.lifecycle.last_release.as_ref().is_some_and(|release| {
                    release.sequence > initial.lifecycle.releases
                        && release.pid == pid
                        && release.exit_confirmed
                })
        })
    }
}
impl WorkerPool {
    fn metal_rejected(&self, key: &str) -> bool {
        self.0.lock().unwrap().metal_failure.as_deref() == Some(key)
    }
    pub(crate) fn metal_failed_requests(&self) -> u64 {
        self.0.lock().unwrap().metal_failed_requests
    }
    fn reject_metal(&self, key: &str, request: bool) {
        let mut state = self.0.lock().unwrap();
        state.metal_failure = Some(key.to_owned());
        if request {
            state.metal_failed_requests = state.metal_failed_requests.wrapping_add(1);
        }
    }
    pub fn status(&self) -> WorkerStatus {
        let state = self.0.lock().unwrap();
        let now = Instant::now();
        WorkerStatus {
            leased: state.leased,
            idle_pid: state.idle.as_ref().map(|i| i.session.server.child.id()),
            starts: state.starts,
            reuses: state.reuses,
            idle_for_ms: state
                .idle
                .as_ref()
                .map(|i| millis(now.saturating_duration_since(i.since))),
            expires_in_ms: state
                .idle
                .as_ref()
                .map(|i| millis(i.expires.saturating_duration_since(now))),
            lifecycle: state.lifecycle.clone(),
        }
    }
    pub fn invalidate(&self) {
        let idle = {
            let mut state = self.0.lock().unwrap();
            state.epoch = state.epoch.wrapping_add(1);
            state.idle.take()
        };
        // Do not wait for a child while holding the pool lock.
        if let Some(idle) = idle {
            release_idle(idle, ReleaseReason::Invalidated);
        }
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
        if let Some(idle) = idle {
            let reason = if idle.ttl.is_zero() {
                ReleaseReason::BatchGraceExpired
            } else {
                ReleaseReason::TtlExpired
            };
            release_idle(idle, reason);
        }
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
        if let Some(idle) = idle {
            release_idle(idle, ReleaseReason::BatchEnd);
        }
    }
}
fn release_idle(mut idle: IdleWorker, reason: ReleaseReason) {
    idle.session
        .server
        .release_as(reason, idle.ttl, Some(idle.since));
    drop(idle);
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
    #[cfg(test)]
    retention_memory: Option<Option<u64>>,
}
impl SessionLease {
    pub fn begin_request(&self) -> Result<()> {
        self.usable.set(false);
        self.check_model()
    }
    pub fn complete_request(&self) -> Result<()> {
        self.check_model()?;
        self.usable.set(true);
        Ok(())
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
        let model_current = session.check_model().is_ok();
        if !model_current {
            engine.inference_pool.invalidate();
        }
        if !self.usable.get()
            && model_current
            && !engine.note_cancel.load(Ordering::SeqCst)
            && !session.resource_stop.load(Ordering::SeqCst)
            && !session.timed_out.load(Ordering::SeqCst)
            && !engine.maintenance.load(Ordering::SeqCst)
        {
            if session.backend.optional_gpu() {
                crate::acceleration::reject(&engine, "runtime_request_failed", true);
            } else if session.backend == crate::inference::Backend::Metal {
                reject_metal(&engine, &self.key, true);
            }
        }
        let otherwise_healthy = model_current
            && self.usable.get()
            && session.healthy()
            && !engine.note_cancel.load(Ordering::SeqCst)
            && !engine.maintenance.load(Ordering::SeqCst);
        let available = if otherwise_healthy {
            self.retention_headroom()
        } else {
            None
        };
        let memory_ok = available.is_some_and(|m| m >= 768);
        *session.active_since.lock().unwrap() = None;
        let mut state = engine.inference_pool.0.lock().unwrap();
        state.leased = false;
        let eligible = state.epoch == self.epoch && (!self.ttl.is_zero() || state.batches > 0);
        if otherwise_healthy && memory_ok && eligible {
            let lifetime = if self.ttl.is_zero() {
                Duration::from_secs(2)
            } else {
                self.ttl
            };
            let since = Instant::now();
            let replaced = state.idle.replace(IdleWorker {
                session,
                key: self.key.clone(),
                expires: since + lifetime,
                ttl: self.ttl,
                since,
            });
            drop(state);
            if let Some(idle) = replaced {
                release_idle(idle, ReleaseReason::Invalidated);
            }
            return;
        }
        let reason = if !model_current {
            ReleaseReason::ModelChanged
        } else if session.resource_stop.load(Ordering::SeqCst) {
            if session.budget_stop.load(Ordering::SeqCst) {
                ReleaseReason::ProcessBudget
            } else {
                ReleaseReason::SystemPressure
            }
        } else if session.timed_out.load(Ordering::SeqCst) {
            ReleaseReason::Timeout
        } else if engine.note_cancel.load(Ordering::SeqCst) {
            ReleaseReason::Cancelled
        } else if engine.maintenance.load(Ordering::SeqCst) {
            ReleaseReason::Maintenance
        } else if !self.usable.get() {
            ReleaseReason::RequestFailed
        } else if otherwise_healthy && eligible && !memory_ok {
            state.lifecycle.retention_refusals += 1;
            if available.is_none() {
                ReleaseReason::MemoryUnknown
            } else {
                ReleaseReason::LowMemory
            }
        } else if state.epoch != self.epoch {
            ReleaseReason::Invalidated
        } else if !otherwise_healthy {
            ReleaseReason::WorkerUnhealthy
        } else {
            ReleaseReason::LeaseEnd
        };
        drop(state);
        session.server.release_as(reason, self.ttl, None);
        drop(session);
    }
}
impl SessionLease {
    fn retention_headroom(&self) -> Option<u64> {
        #[cfg(test)]
        if let Some(memory) = self.retention_memory {
            return memory;
        }
        crate::platform::available_memory_mib()
    }
}
impl LocalSession {
    pub(crate) fn check_model(&self) -> Result<()> {
        if let Some(receipt) = &self.model_receipt {
            receipt.check_current()?;
        }
        Ok(())
    }
    fn healthy(&mut self) -> bool {
        !self.resource_stop.load(Ordering::SeqCst)
            && !self.timed_out.load(Ordering::SeqCst)
            && !self.server.done.load(Ordering::SeqCst)
            && matches!(self.server.child.try_wait(), Ok(None))
    }
}
fn worker_key(
    engine: &Engine,
    prefs: &crate::preferences::Preferences,
    meeting: &str,
    run: u32,
    spec: &crate::inference::LaunchSpec,
    model_identity: &str,
) -> Result<String> {
    use sha2::{Digest, Sha256};
    // Appending a recording creates a take without changing the transcription run.
    // Read only its indexed number; do not deserialize a meeting's audio history.
    let take = engine
        .store
        .lock()
        .unwrap()
        .conn
        .query_row(
            "SELECT MAX(number) FROM recording_takes WHERE meeting_id=?1",
            [meeting],
            |row| row.get::<_, Option<u32>>(0),
        )?
        .unwrap_or_default();
    let key = json!({
        "schema":"worker-cuda-selectors-v1", "selectors":spec.selectors.digest(),
        "model":prefs.llm_model,"verified_assets":model_identity,
        "backend":spec.backend.name(),"component":spec.identity,"device":spec.device,"runtime":std::fs::read(spec.directory.join("build-info.json")).ok(),
        "executable":engine.inference_fingerprints.file(&spec.directory.join(if spec.backend == crate::inference::Backend::Mlx {"runner.py".into()} else {crate::platform::executable("llama-server")})),
        "threads":prefs.threads,"context":prefs.context_size,"mode":prefs.resource_mode,
        "budget":crate::inference::budget_mib(prefs),"meeting":meeting,"run":run,"take":take,
    });
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&key)?)))
}
pub(crate) fn open_session(
    engine: &Arc<Engine>,
    prefs: &crate::preferences::Preferences,
    meeting: &str,
    run: u32,
) -> Result<SessionLease> {
    admission_attempt(engine, || open_session_inner(engine, prefs, meeting, run))
}
fn admission_attempt(
    engine: &Engine,
    operation: impl FnOnce() -> Result<SessionLease>,
) -> Result<SessionLease> {
    let result = operation();
    if result.is_err() {
        engine
            .inference_pool
            .0
            .lock()
            .unwrap()
            .lifecycle
            .admission_refusals += 1;
    }
    result
}
fn open_session_inner(
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
    let receipt = match engine.verified_model(&prefs.llm_model) {
        Ok(receipt) => receipt,
        Err(error) => {
            engine.inference_pool.invalidate();
            return Err(error);
        }
    };
    let spec = crate::inference::launch_spec(engine, prefs)?;
    let key = worker_key(engine, prefs, meeting, run, &spec, receipt.fingerprint())?;
    if spec.backend == crate::inference::Backend::Metal
        && engine.inference_pool.metal_rejected(&key)
    {
        let mut cpu = prefs.clone();
        cpu.inference_backend = "cpu".into();
        return open_session_inner(engine, &cpu, meeting, run);
    }
    let result = checkout(engine, key.clone(), Duration::from_secs(ttl.into()), || {
        spawn_session(engine, prefs, &spec, Some(Duration::from_secs(ttl.into())))
    });
    if result
        .as_ref()
        .is_err_and(|error| retryable_start_failure(engine, error))
        && (spec.backend.optional_gpu() || spec.backend == crate::inference::Backend::Metal)
        && !engine.inference_pool.status().leased
    {
        if spec.backend.optional_gpu() {
            crate::acceleration::reject(engine, "runtime_start_failed", false);
        } else {
            reject_metal(engine, &key, false);
        }
        engine.inference_pool.invalidate();
        let mut cpu = prefs.clone();
        cpu.inference_backend = "cpu".into();
        return open_session_inner(engine, &cpu, meeting, run);
    }
    result
}
fn reject_metal(engine: &Engine, key: &str, request: bool) {
    engine.inference_pool.reject_metal(key, request);
    // Metadata only; never persist nested transport errors, prompts or text.
    let _ = std::fs::write(
        engine.root.join("last-metal-fallback.json"),
        serde_json::to_vec(&json!({"schema":1,"scope_sha256":key,"stage":if request {"request"} else {"startup"},"backend":"cpu","at":chrono::Utc::now().to_rfc3339()})).unwrap_or_default(),
    );
}
fn retryable_start_failure(engine: &Engine, error: &anyhow::Error) -> bool {
    !engine.note_cancel.load(Ordering::SeqCst)
        && !engine.maintenance.load(Ordering::SeqCst)
        && !error.chain().any(|error| {
            let message = error.to_string();
            message.starts_with("writer:resource-")
                || message.starts_with("writer:low-memory:")
                || message == "writer:memory-unavailable"
                || message.starts_with("model:integrity:")
        })
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
        let reason = if idle.key != key {
            Some(ReleaseReason::ScopeChanged)
        } else if idle.expires <= Instant::now() {
            Some(if idle.ttl.is_zero() {
                ReleaseReason::BatchGraceExpired
            } else {
                ReleaseReason::TtlExpired
            })
        } else if idle.session.check_model().is_err() {
            Some(ReleaseReason::ModelChanged)
        } else if !idle.session.healthy() {
            Some(ReleaseReason::WorkerUnhealthy)
        } else {
            match crate::platform::available_memory_mib() {
                Some(memory) if memory >= 768 => None,
                memory => {
                    engine
                        .inference_pool
                        .0
                        .lock()
                        .unwrap()
                        .lifecycle
                        .retention_refusals += 1;
                    Some(if memory.is_none() {
                        ReleaseReason::MemoryUnknown
                    } else {
                        ReleaseReason::LowMemory
                    })
                }
            }
        };
        if let Some(reason) = reason {
            release_idle(idle, reason);
            None
        } else {
            reused = true;
            Some(idle.session)
        }
    });
    let mut session = match session.map(Ok).unwrap_or_else(spawn) {
        Ok(session) => session,
        Err(error) => {
            engine.inference_pool.0.lock().unwrap().leased = false;
            return Err(error);
        }
    };
    session
        .server
        .release_as(ReleaseReason::WorkerUnhealthy, ttl, None);
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
        #[cfg(test)]
        retention_memory: None,
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

#[derive(Default)]
struct StopState {
    closing: bool,
    reason: Option<ReleaseReason>,
}
struct ReleaseContext {
    reason: ReleaseReason,
    ttl: Duration,
    idle_since: Option<Instant>,
}
struct Server {
    child: Child,
    done: Arc<AtomicBool>,
    engine: Weak<Engine>,
    stop: Arc<Mutex<StopState>>,
    pooled: Option<ReleaseContext>,
    #[cfg(test)]
    force_unconfirmed_exit: bool,
    #[cfg(test)]
    before_reap: Option<Box<dyn FnOnce() + Send>>,
}
impl Server {
    fn new(
        child: Child,
        done: Arc<AtomicBool>,
        engine: &Arc<Engine>,
        pooled: Option<Duration>,
    ) -> Self {
        Self {
            child,
            done,
            engine: Arc::downgrade(engine),
            stop: Arc::new(Mutex::new(StopState::default())),
            pooled: pooled.map(|ttl| ReleaseContext {
                reason: ReleaseReason::StartupFailed,
                ttl,
                idle_since: None,
            }),
            #[cfg(test)]
            force_unconfirmed_exit: false,
            #[cfg(test)]
            before_reap: None,
        }
    }
    fn release_as(&mut self, reason: ReleaseReason, ttl: Duration, idle_since: Option<Instant>) {
        self.pooled = Some(ReleaseContext {
            reason,
            ttl,
            idle_since,
        });
    }
}
fn stop_from_watchdog(
    engine: Option<&Engine>,
    stop: &Mutex<StopState>,
    pid: u32,
    reason: ReleaseReason,
    pooled: bool,
) -> bool {
    let accepted = {
        let mut state = stop.lock().unwrap();
        if state.closing || state.reason.is_some() {
            false
        } else {
            state.reason = Some(reason);
            if matches!(reason, ReleaseReason::Cancelled | ReleaseReason::AsrBacklog) {
                if let Some(engine) = engine {
                    // Preserve cancellation-before-signal ordering, but only
                    // for this accepted stop. A stale, closing worker must not
                    // cancel a newer lease through the shared global flag.
                    engine.note_cancel.store(true, Ordering::SeqCst);
                }
            }
            // Serialize the actual signal with closing, not just its reason.
            // Once Drop can wait/reap, this old PID must never be signalled.
            crate::platform::stop_process(pid, false);
            true
        }
    };
    if accepted
        && pooled
        && matches!(
            reason,
            ReleaseReason::ProcessBudget | ReleaseReason::SystemPressure
        )
    {
        if let Some(engine) = engine {
            engine
                .inference_pool
                .0
                .lock()
                .unwrap()
                .lifecycle
                .resource_stops += 1;
        }
    }
    accepted
}
impl Drop for Server {
    fn drop(&mut self) {
        let stop_reason = {
            let mut state = self.stop.lock().unwrap();
            state.closing = true;
            state.reason
        };
        self.done.store(true, Ordering::SeqCst);
        let idle = self
            .pooled
            .as_ref()
            .and_then(|context| context.idle_since.map(|since| since.elapsed()));
        #[cfg(test)]
        if let Some(hook) = self.before_reap.take() {
            hook();
        }
        let prior = self.child.try_wait();
        let running = matches!(prior, Ok(None));
        let unknown = prior.is_err();
        if !matches!(prior, Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let confirmed = self.child.wait().is_ok();
        #[cfg(test)]
        let confirmed = confirmed && !self.force_unconfirmed_exit;
        if let Some(engine) = self.engine.upgrade() {
            let _ = engine.llm_pid.compare_exchange(
                self.child.id(),
                0,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
            if let Some(context) = &self.pooled {
                let reason = if unknown || !confirmed {
                    ReleaseReason::ExitUnconfirmed
                } else if let Some(reason) = stop_reason {
                    reason
                } else if context.reason.policy() && !running {
                    ReleaseReason::WorkerExited
                } else {
                    context.reason
                };
                // Kill/wait and all session cleanup above are outside the pool
                // mutex. Allocate sequence only now, after completion.
                engine.inference_pool.0.lock().unwrap().lifecycle.released(
                    self.child.id(),
                    reason,
                    context.ttl,
                    idle,
                    confirmed && !unknown,
                );
            }
        }
    }
}

pub(crate) struct LocalSession {
    model_receipt: Option<crate::model_integrity::Receipt>,
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
    // Calibration sessions are not pooled launches or lifecycle evidence.
    spawn_session(engine, prefs, spec, None)
}
fn spawn_session(
    engine: &Arc<Engine>,
    prefs: &crate::preferences::Preferences,
    spec: &crate::inference::LaunchSpec,
    pooled: Option<Duration>,
) -> Result<LocalSession> {
    let receipt = engine.verified_model(&prefs.llm_model)?;
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
        crate::inference::server_command_with_model(engine, prefs, port, &token, spec, &receipt)?;
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
    let mut server = Server::new(child, done.clone(), engine, pooled);
    let stop_state = Arc::clone(&server.stop);
    let pid = server.child.id();
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
                let engine = weak.upgrade();
                let reason = if over_budget {
                    ReleaseReason::ProcessBudget
                } else {
                    ReleaseReason::SystemPressure
                };
                if stop_from_watchdog(
                    engine.as_deref(),
                    &stop_state,
                    pid,
                    reason,
                    pooled.is_some(),
                ) {
                    if let Some(engine) = engine {
                        engine.inference_pool.invalidate();
                    }
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
                stop_from_watchdog(
                    Some(&engine),
                    &stop_state,
                    pid,
                    ReleaseReason::Timeout,
                    pooled.is_some(),
                );
                break;
            }
            if s.recording && s.elapsed_ms.saturating_sub(s.processed_ms) > 4000 {
                lag_count += 1;
            } else {
                lag_count = 0;
            }
            let cancelled = engine.note_cancel.load(Ordering::SeqCst);
            if cancelled || lag_count >= 6 {
                let reason = if cancelled {
                    ReleaseReason::Cancelled
                } else {
                    ReleaseReason::AsrBacklog
                };
                stop_from_watchdog(Some(&engine), &stop_state, pid, reason, pooled.is_some());
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
        if server.child.try_wait()?.is_some() {
            bail!("笔记模型已停止；原文保留，可以稍后重试");
        }
        if agent
            .get(&format!("{base}/health"))
            .set("Authorization", &auth)
            .timeout(Duration::from_secs(1))
            .call()
            .is_ok()
        {
            if backend.optional_gpu() || backend == crate::inference::Backend::Metal {
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

    receipt.check_current()?;
    Ok(LocalSession {
        model_receipt: Some(receipt),
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

    #[test]
    #[ignore = "isolated selector environment fixture; never starts a model worker"]
    fn cuda_selector_key_child() -> Result<()> {
        let root =
            std::path::PathBuf::from(std::env::var_os("OMASCRIBE_TEST_SELECTOR_ROOT").unwrap());
        let output =
            std::path::PathBuf::from(std::env::var_os("OMASCRIBE_TEST_SELECTOR_OUTPUT").unwrap());
        let engine = Engine::open(root.clone())?;
        let prefs = crate::preferences::Preferences {
            memory_budget_mib: 1024,
            ..Default::default()
        };
        let (backend, directory, device) =
            match std::env::var("OMASCRIBE_TEST_SELECTOR_BACKEND").as_deref() {
                Ok("vulkan") => (crate::inference::Backend::Vulkan, "vulkan", "Vulkan0"),
                Ok("cuda") | Err(_) => (crate::inference::Backend::Cuda, "cuda", "CUDA0"),
                _ => bail!("Unsupported synthetic selector backend"),
            };
        let spec = crate::inference::LaunchSpec {
            backend,
            directory: root.join(directory),
            device: Some(device.into()),
            identity: "fixed-synthetic-certificate".into(),
            selectors: crate::inference::SelectorEnv::capture(backend),
        };
        let key = worker_key(
            &engine,
            &prefs,
            "synthetic-meeting",
            1,
            &spec,
            "synthetic-model",
        )?;
        std::fs::write(output, key)?;
        Ok(())
    }

    fn vulkan_selector_pair(variable: &str, before: &str, after: &str) -> Result<()> {
        let root = tempfile::tempdir()?;
        let runtime = root.path().join("vulkan");
        std::fs::create_dir(&runtime)?;
        std::fs::write(
            runtime.join(crate::platform::executable("llama-server")),
            b"never executed",
        )?;
        let key = |value: &str, label: &str| -> Result<String> {
            let output = root.path().join(format!("{label}.key"));
            let child = std::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "inference_worker::tests::cuda_selector_key_child",
                    "--ignored",
                ])
                .env("OMASCRIBE_TEST_SELECTOR_ROOT", root.path())
                .env("OMASCRIBE_TEST_SELECTOR_OUTPUT", &output)
                .env("OMASCRIBE_TEST_SELECTOR_BACKEND", "vulkan")
                .env(
                    "OMASCRIBE_TEST_UNRELATED_UI",
                    if label == "stable" { "dark" } else { "light" },
                )
                .env(variable, value)
                .output()?;
            anyhow::ensure!(
                child.status.success(),
                "selector fixture failed: {}",
                String::from_utf8_lossy(&child.stderr)
            );
            Ok(std::fs::read_to_string(output)?)
        };
        let first = key(before, "first")?;
        assert_ne!(
            first,
            key(after, "changed")?,
            "Vulkan0 binding must follow {variable}"
        );
        assert_eq!(first, key(before, "stable")?);
        Ok(())
    }

    #[test]
    fn vulkan_selector_ggml_order_changes_invalidate_a_real_worker_key() -> Result<()> {
        vulkan_selector_pair("GGML_VK_VISIBLE_DEVICES", "0,1", "1,0")
    }

    #[test]
    fn vulkan_selector_dri_prime_changes_invalidate_a_real_worker_key() -> Result<()> {
        vulkan_selector_pair("DRI_PRIME", "0", "1!")
    }

    #[test]
    fn vulkan_selector_driver_manifest_changes_invalidate_a_real_worker_key() -> Result<()> {
        vulkan_selector_pair("VK_DRIVER_FILES", "synthetic-a.json", "synthetic-b.json")
    }

    #[test]
    fn cuda_selector_changes_invalidate_a_real_worker_key() -> Result<()> {
        let root = tempfile::tempdir()?;
        let runtime = root.path().join("cuda");
        std::fs::create_dir(&runtime)?;
        std::fs::write(
            runtime.join(crate::platform::executable("llama-server")),
            b"never executed",
        )?;
        let key =
            |visible: Option<&str>, order: Option<&str>, ui: &str, label: &str| -> Result<String> {
                let output = root.path().join(format!("{label}.key"));
                let mut command = std::process::Command::new(std::env::current_exe()?);
                command
                    .args([
                        "--exact",
                        "inference_worker::tests::cuda_selector_key_child",
                        "--ignored",
                    ])
                    .env("OMASCRIBE_TEST_SELECTOR_ROOT", root.path())
                    .env("OMASCRIBE_TEST_SELECTOR_OUTPUT", &output)
                    .env("OMASCRIBE_TEST_SELECTOR_BACKEND", "cuda")
                    .env("OMASCRIBE_TEST_UNRELATED_UI", ui);
                for (name, value) in [
                    ("CUDA_VISIBLE_DEVICES", visible),
                    ("CUDA_DEVICE_ORDER", order),
                ] {
                    if let Some(value) = value {
                        command.env(name, value);
                    } else {
                        command.env_remove(name);
                    }
                }
                let child = command.output()?;
                anyhow::ensure!(
                    child.status.success(),
                    "selector fixture failed: {}",
                    String::from_utf8_lossy(&child.stderr)
                );
                Ok(std::fs::read_to_string(output)?)
            };
        let first = key(Some("0,1"), None, "light", "first")?;
        let other = key(Some("1,0"), None, "light", "reordered")?;
        assert_ne!(
            first, other,
            "CUDA0 can name another physical GPU after selector reordering"
        );
        assert_eq!(first, key(Some("0,1"), None, "dark", "unchanged")?);
        assert_ne!(
            first,
            key(Some("0,1"), Some("PCI_BUS_ID"), "light", "order")?
        );
        assert_ne!(
            key(None, None, "light", "absent")?,
            key(Some(""), None, "light", "empty")?
        );
        Ok(())
    }

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
            model_receipt: None,
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
            server: Server::new(child, Default::default(), engine, None),
        })
    }
    fn lease(engine: &Arc<Engine>, key: &str, ttl: u64) -> Result<SessionLease> {
        checkout(engine, key.into(), Duration::from_secs(ttl), || {
            fake(engine)
        })
    }
    fn retained_lease(engine: &Arc<Engine>, ttl: u64) -> Result<SessionLease> {
        let mut session = lease(engine, "lifecycle-fixture", ttl)?;
        session.retention_memory = Some(Some(2048));
        Ok(session)
    }
    fn assert_reaped(pid: u32) {
        #[cfg(unix)]
        unsafe {
            let mut status = 0;
            assert_eq!(libc::waitpid(pid as i32, &mut status, libc::WNOHANG), -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
        }
        #[cfg(windows)]
        {
            let mut processes = sysinfo::System::new();
            processes.refresh_processes(
                sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
                true,
            );
            assert!(processes.process(sysinfo::Pid::from_u32(pid)).is_none());
        }
    }
    #[test]
    fn lifecycle_zero_ttl_lease_and_final_batch_confirm_real_child_exit() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let first = retained_lease(&engine, 0)?;
        let first_pid = first.server.child.id();
        drop(first);
        assert_reaped(first_pid);
        let status = engine.inference_pool.status();
        assert_eq!(status.lifecycle.lease_releases, 1);
        assert_eq!(status.lifecycle.last_release.unwrap().reason, "lease_end");
        let batch = engine.inference_pool.batch();
        let nested = engine.inference_pool.batch();
        let second = retained_lease(&engine, 0)?;
        let pid = second.server.child.id();
        drop(second);
        drop(nested);
        assert_eq!(engine.inference_pool.status().idle_pid, Some(pid));
        let reused = retained_lease(&engine, 0)?;
        assert!(reused.reused);
        drop(reused);
        drop(batch);
        assert_reaped(pid);
        let status = engine.inference_pool.status();
        assert_eq!(status.lifecycle.releases, 2);
        assert_eq!(status.lifecycle.batch_releases, 1);
        assert_eq!(status.lifecycle.ttl_releases, 0);
        assert_eq!(status.lifecycle.other_releases, 0);
        let last = status.lifecycle.last_release.unwrap();
        assert_eq!(last.reason, "batch_end");
        assert!(last.exit_confirmed);
        Ok(())
    }
    #[test]
    fn lifecycle_ttl_and_zero_ttl_batch_grace_are_distinct() -> Result<()> {
        for ttl in [0, 10] {
            let root = tempfile::tempdir()?;
            let engine = Engine::open(root.path().into())?;
            let batch = engine.inference_pool.batch();
            drop(retained_lease(&engine, ttl)?);
            let initial = engine.inference_pool.status();
            let pid = initial.idle_pid.unwrap();
            let lifetime = Duration::from_secs(if ttl == 0 { 2 } else { ttl });
            // Backdate only the fixture clock; actual child termination/reaping
            // still executes the same production reaper with a real process.
            {
                let mut state = engine.inference_pool.0.lock().unwrap();
                let idle = state.idle.as_mut().unwrap();
                idle.since = Instant::now() - lifetime;
                idle.expires = Instant::now();
            }
            engine.inference_pool.reap_idle(Instant::now());
            assert_reaped(pid);
            let final_status = engine.inference_pool.status();
            assert!(final_status.confirms_idle_exit_since(&initial));
            assert_eq!(final_status.lifecycle.ttl_releases, u64::from(ttl != 0));
            assert_eq!(final_status.lifecycle.batch_releases, u64::from(ttl == 0));
            let last = final_status.lifecycle.last_release.unwrap();
            assert_eq!(
                last.reason,
                if ttl == 0 {
                    "batch_grace_expired"
                } else {
                    "ttl_expired"
                }
            );
            assert!(last.idle_ms.unwrap() >= millis(lifetime));
            assert_eq!(last.ttl_ms, ttl * 1000);
            drop(batch);
        }
        Ok(())
    }
    #[test]
    fn lifecycle_short_real_clock_expiry_confirms_the_same_child() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        // This is a short real-clock regression, not a 10/30-second benchmark.
        let ttl = Duration::from_millis(250);
        let mut session = checkout(&engine, "short-real-clock".into(), ttl, || fake(&engine))?;
        session.retention_memory = Some(Some(2048));
        let pid = session.server.child.id();
        drop(session);
        let initial = engine.inference_pool.status();
        assert_eq!(initial.idle_pid, Some(pid));
        let remaining = initial.expires_in_ms.unwrap();
        assert!(remaining > 0);
        assert!((initial.idle_for_ms.unwrap() + remaining).abs_diff(250) <= 2);
        engine.inference_pool.reap_idle(Instant::now());
        assert_eq!(engine.inference_pool.status().idle_pid, Some(pid));
        let deadline = engine
            .inference_pool
            .0
            .lock()
            .unwrap()
            .idle
            .as_ref()
            .unwrap()
            .expires;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            if remaining.is_zero() {
                break;
            }
            thread::sleep(remaining.min(Duration::from_millis(5)));
        }
        engine.inference_pool.reap_idle(Instant::now());
        assert_reaped(pid);
        let final_status = engine.inference_pool.status();
        assert!(final_status.confirms_idle_exit_since(&initial));
        let release = final_status.lifecycle.last_release.unwrap();
        assert_eq!(release.pid, pid);
        assert_eq!(release.reason, "ttl_expired");
        assert_eq!(release.ttl_ms, 250);
        assert!(release.idle_ms.unwrap() >= 250);
        assert!(release.exit_confirmed);
        Ok(())
    }
    #[test]
    fn lifecycle_leased_watchdog_stops_keep_exact_cause_and_count_resources_once() -> Result<()> {
        for (reason, expected) in [
            (ReleaseReason::ProcessBudget, "process_budget"),
            (ReleaseReason::SystemPressure, "system_pressure"),
            (ReleaseReason::AsrBacklog, "asr_backlog"),
            (ReleaseReason::Cancelled, "cancelled"),
            (ReleaseReason::Timeout, "timeout"),
        ] {
            let root = tempfile::tempdir()?;
            let engine = Engine::open(root.path().into())?;
            let session = retained_lease(&engine, 10)?;
            let pid = session.server.child.id();
            session.begin_request()?;
            assert!(stop_from_watchdog(
                Some(&engine),
                &session.server.stop,
                pid,
                reason,
                true
            ));
            assert!(!stop_from_watchdog(
                Some(&engine),
                &session.server.stop,
                pid,
                reason,
                true
            ));
            if matches!(
                reason,
                ReleaseReason::ProcessBudget | ReleaseReason::SystemPressure
            ) {
                session.resource_stop.store(true, Ordering::SeqCst);
                session
                    .budget_stop
                    .store(reason == ReleaseReason::ProcessBudget, Ordering::SeqCst);
                engine.inference_pool.invalidate();
            } else if reason == ReleaseReason::Timeout {
                session.timed_out.store(true, Ordering::SeqCst);
            }
            // Backlog and user cancellation both set this flag in production;
            // the first observed cause must survive that shared flag.
            if matches!(reason, ReleaseReason::AsrBacklog | ReleaseReason::Cancelled) {
                assert!(engine.note_cancel.load(Ordering::SeqCst));
            }
            drop(session);
            assert_reaped(pid);
            let status = engine.inference_pool.status();
            assert_eq!(
                status.lifecycle.resource_stops,
                u64::from(matches!(
                    reason,
                    ReleaseReason::ProcessBudget | ReleaseReason::SystemPressure
                ))
            );
            assert_eq!(status.lifecycle.other_releases, 1);
            assert_eq!(status.lifecycle.ttl_releases, 0);
            assert_eq!(status.lifecycle.last_release.unwrap().reason, expected);
        }
        Ok(())
    }
    #[test]
    fn lifecycle_unknown_exit_stays_adverse_after_a_later_clean_release() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let mut session = retained_lease(&engine, 0)?;
        let pid = session.server.child.id();
        // Inject an unknown wait result only after real kill/wait has run, so
        // the negative telemetry test cannot leak a real child process.
        session
            .session
            .as_mut()
            .unwrap()
            .server
            .force_unconfirmed_exit = true;
        drop(session);
        assert_reaped(pid);
        let first = engine.inference_pool.status();
        assert!(
            !first
                .lifecycle
                .last_release
                .as_ref()
                .unwrap()
                .exit_confirmed
        );
        assert_eq!(first.lifecycle.other_releases, 1);
        drop(retained_lease(&engine, 0)?);
        let later = engine.inference_pool.status();
        assert!(
            later
                .lifecycle
                .last_release
                .as_ref()
                .unwrap()
                .exit_confirmed
        );
        assert_eq!(later.lifecycle.last_release.as_ref().unwrap().sequence, 2);
        assert_eq!(later.lifecycle.releases, 2);
        assert_eq!(later.lifecycle.lease_releases, 1);
        assert_eq!(later.lifecycle.other_releases, 1);
        Ok(())
    }
    #[test]
    fn lifecycle_pooled_startup_drop_is_recorded_but_calibration_is_not() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let direct = fake(&engine)?;
        let direct_pid = direct.server.child.id();
        drop(direct);
        assert_reaped(direct_pid);
        assert_eq!(engine.inference_pool.status().lifecycle.releases, 0);
        let result = checkout(&engine, "startup".into(), Duration::from_secs(10), || {
            let mut startup = fake(&engine)?;
            startup
                .server
                .release_as(ReleaseReason::StartupFailed, Duration::from_secs(10), None);
            drop(startup);
            bail!("fixture refusal after child spawn")
        });
        assert!(result.is_err());
        let status = engine.inference_pool.status();
        assert!(!status.leased);
        assert_eq!(status.starts, 0);
        assert_eq!(status.lifecycle.releases, 1);
        assert_eq!(status.lifecycle.other_releases, 1);
        assert_eq!(
            status.lifecycle.last_release.unwrap().reason,
            "startup_failed"
        );
        Ok(())
    }
    #[test]
    fn lifecycle_memory_refusal_requires_eligible_retention() -> Result<()> {
        for ttl in [0, 10] {
            for available in [None, Some(0)] {
                let root = tempfile::tempdir()?;
                let engine = Engine::open(root.path().into())?;
                let mut session = retained_lease(&engine, ttl)?;
                session.retention_memory = Some(available);
                drop(session);
                let status = engine.inference_pool.status();
                assert_eq!(status.lifecycle.retention_refusals, u64::from(ttl > 0));
                assert_eq!(status.lifecycle.other_releases, u64::from(ttl > 0));
                assert_eq!(status.lifecycle.lease_releases, u64::from(ttl == 0));
            }
        }
        Ok(())
    }
    #[test]
    fn lifecycle_admission_counts_the_whole_attempt_once_after_fallback() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        assert!(admission_attempt(&engine, || {
            let _ = checkout(&engine, "first".into(), Duration::ZERO, || {
                bail!("first startup failed")
            });
            checkout(&engine, "fallback".into(), Duration::ZERO, || {
                bail!("fallback failed")
            })
        })
        .is_err());
        assert_eq!(
            engine.inference_pool.status().lifecycle.admission_refusals,
            1
        );
        let session = admission_attempt(&engine, || {
            let _ = checkout(&engine, "first".into(), Duration::ZERO, || {
                bail!("first startup failed")
            });
            lease(&engine, "fallback", 0)
        })?;
        assert_eq!(
            engine.inference_pool.status().lifecycle.admission_refusals,
            1
        );
        drop(session);
        Ok(())
    }
    #[test]
    fn lifecycle_pending_wait_does_not_confirm_exit_or_hold_the_pool_lock() -> Result<()> {
        use std::sync::mpsc;
        for mode in ["no_new_lease", "new_finishes_first", "new_finishes_last"] {
            let root = tempfile::tempdir()?;
            let engine = Engine::open(root.path().into())?;
            let (waiting_tx, waiting_rx) = mpsc::channel();
            let (continue_tx, continue_rx) = mpsc::channel();
            let mut old = retained_lease(&engine, 10)?;
            let old_pid = old.server.child.id();
            let old_stop = Arc::clone(&old.server.stop);
            old.session.as_mut().unwrap().server.before_reap = Some(Box::new(move || {
                // Even an assertion failure/disconnected test controller must
                // let real kill/wait cleanup continue rather than leak a child.
                let _ = waiting_tx.send(());
                let _ = continue_rx.recv_timeout(Duration::from_secs(5));
            }));
            drop(old);
            let initial = engine.inference_pool.status();
            assert_eq!(initial.idle_pid, Some(old_pid));
            let reaping_engine = Arc::clone(&engine);
            let reaping = thread::spawn(move || reaping_engine.inference_pool.invalidate());
            waiting_rx.recv_timeout(Duration::from_secs(5))?;
            let pending = engine.inference_pool.status();
            assert!(pending.idle_pid.is_none());
            assert!(!pending.confirms_idle_exit_since(&initial));
            assert_eq!(pending.lifecycle.releases, 0);
            assert!(!stop_from_watchdog(
                Some(&engine),
                &old_stop,
                old_pid,
                ReleaseReason::ProcessBudget,
                true
            ));
            assert_eq!(engine.inference_pool.status().lifecycle.resource_stops, 0);
            assert!(!stop_from_watchdog(
                Some(&engine),
                &old_stop,
                old_pid,
                ReleaseReason::AsrBacklog,
                true
            ));
            assert!(!engine.note_cancel.load(Ordering::SeqCst));
            let mut newer = if mode == "no_new_lease" {
                None
            } else {
                Some(retained_lease(&engine, 0)?)
            };
            let new_pid = newer.as_ref().map(|s| s.server.child.id());
            if mode == "new_finishes_first" {
                drop(newer.take());
                let completed_new = engine.inference_pool.status();
                assert_eq!(completed_new.lifecycle.releases, 1);
                assert_eq!(
                    completed_new.lifecycle.last_release.unwrap().pid,
                    new_pid.unwrap()
                );
                assert!(!engine
                    .inference_pool
                    .status()
                    .confirms_idle_exit_since(&initial));
            }
            continue_tx.send(())?;
            reaping.join().unwrap();
            assert_reaped(old_pid);
            let final_status = engine.inference_pool.status();
            let release = final_status.lifecycle.last_release.as_ref().unwrap();
            assert_eq!(release.pid, old_pid);
            assert_eq!(
                release.sequence,
                if mode == "new_finishes_first" { 2 } else { 1 }
            );
            assert_eq!(release.sequence, final_status.lifecycle.releases);
            if mode == "new_finishes_last" {
                assert!(final_status.leased);
                assert!(!final_status.confirms_idle_exit_since(&initial));
                assert_eq!(
                    engine.llm_pid.load(Ordering::SeqCst),
                    new_pid.unwrap(),
                    "old cleanup must not clear the new PID"
                );
            } else {
                assert!(final_status.confirms_idle_exit_since(&initial));
            }
            drop(newer);
        }
        Ok(())
    }
    #[test]
    fn batch_reuses_one_child_and_zero_ttl_reaps_it_at_batch_end() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let batch = engine.inference_pool.batch();
        let first = lease(&engine, "meeting/run/model", 0)?;
        let pid = first.server.child.id();
        assert!(
            lease(&engine, "other", 0).is_err(),
            "one inference at a time"
        );
        drop(first);
        let second = lease(&engine, "meeting/run/model", 0)?;
        assert!(second.reused);
        assert_eq!(second.server.child.id(), pid);
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
        let pid = first.server.child.id();
        drop(first);
        let second = lease(&engine, "run2", 30)?;
        assert!(!second.reused);
        assert_ne!(second.server.child.id(), pid);
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
        session.begin_request()?;
        drop(session);
        assert_eq!(engine.acceleration_status().state, "cpu_fallback");
        assert_eq!(engine.llm_pid.load(Ordering::SeqCst), 0);
        assert!(!engine.inference_pool.status().leased);
        Ok(())
    }
    #[test]
    fn metal_failure_is_scoped_reaped_and_does_not_revoke_optional_components() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        std::fs::create_dir_all(root.path().join("accelerators"))?;
        let certificate = root.path().join("accelerators/active.json");
        std::fs::write(&certificate, b"unrelated optional-component certificate")?;
        let mut calls = 0;
        let result = crate::acceleration::with_cpu_fallback(&engine, || {
            calls += 1;
            if calls == 1 {
                let session = checkout(
                    &engine,
                    "metal-scope".into(),
                    Duration::from_secs(30),
                    || {
                        let mut session = fake(&engine)?;
                        session.backend = crate::inference::Backend::Metal;
                        Ok(session)
                    },
                )?;
                session.begin_request()?;
                drop(session);
                bail!("fixture interrupted Metal response");
            }
            assert!(engine.inference_pool.metal_rejected("metal-scope"));
            assert!(!engine
                .inference_pool
                .metal_rejected("different-model-config-run"));
            assert_eq!(engine.llm_pid.load(Ordering::SeqCst), 0);
            drop(lease(&engine, "cpu-scope", 0)?);
            Ok(())
        });
        result?;
        assert_eq!(calls, 2);
        assert_eq!(engine.inference_pool.metal_failed_requests(), 1);
        assert_eq!(
            std::fs::read(certificate)?,
            b"unrelated optional-component certificate"
        );
        assert!(!engine.inference_pool.status().leased);
        assert_eq!(engine.llm_pid.load(Ordering::SeqCst), 0);
        Ok(())
    }
    #[test]
    fn cancellation_pressure_timeout_and_valid_response_never_blacklist_metal() -> Result<()> {
        for condition in [
            "cancel",
            "pressure",
            "timeout",
            "maintenance",
            "valid-response",
        ] {
            let root = tempfile::tempdir()?;
            let engine = Engine::open(root.path().into())?;
            let session = checkout(&engine, "metal-scope".into(), Duration::ZERO, || {
                let mut session = fake(&engine)?;
                session.backend = crate::inference::Backend::Metal;
                Ok(session)
            })?;
            session.begin_request()?;
            match condition {
                "cancel" => engine.note_cancel.store(true, Ordering::SeqCst),
                "pressure" => session.resource_stop.store(true, Ordering::SeqCst),
                "timeout" => session.timed_out.store(true, Ordering::SeqCst),
                "maintenance" => engine.maintenance.store(true, Ordering::SeqCst),
                _ => session.complete_request()?,
            }
            drop(session);
            assert!(
                !engine.inference_pool.metal_rejected("metal-scope"),
                "{condition}"
            );
            assert_eq!(
                engine.inference_pool.metal_failed_requests(),
                0,
                "{condition}"
            );
            assert_eq!(engine.llm_pid.load(Ordering::SeqCst), 0);
        }
        Ok(())
    }
    #[test]
    fn startup_resource_refusal_never_retries_on_cpu() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        for reason in [
            "writer:resource-budget:5000:4096",
            "writer:resource-budget-hit:4096",
            "writer:resource-pressure",
            "writer:low-memory:100:1400",
            "writer:memory-unavailable",
            "model:integrity: SHA-256 mismatch",
        ] {
            let error = anyhow::anyhow!(reason).context("fixture context");
            assert!(!retryable_start_failure(&engine, &error), "{reason}");
        }
        let transport = anyhow::anyhow!("fixture runtime startup failed");
        assert!(retryable_start_failure(&engine, &transport));
        engine.note_cancel.store(true, Ordering::SeqCst);
        assert!(!retryable_start_failure(&engine, &transport));
        engine.note_cancel.store(false, Ordering::SeqCst);
        engine.maintenance.store(true, Ordering::SeqCst);
        assert!(!retryable_start_failure(&engine, &transport));
        Ok(())
    }
    #[test]
    fn metal_failure_scope_changes_with_runtime_model_configuration_and_take() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let prefs = crate::preferences::Preferences::default();
        let mut model = crate::models::model(&prefs.llm_model)?.clone();
        model.files.truncate(1);
        model.files[0].bytes = 4;
        model.files[0].sha256 = format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(b"good"));
        let directory = root.path().join("model-store").join(&model.id);
        std::fs::create_dir_all(&directory)?;
        std::fs::write(directory.join(&model.files[0].name), b"good")?;
        let receipt = engine.model_integrity.verify(&directory, &model)?;
        let model_identity = receipt.fingerprint().to_owned();
        let runtime = root.path().join("llama");
        std::fs::create_dir(&runtime)?;
        let executable = runtime.join(crate::platform::executable("llama-server"));
        std::fs::write(&executable, b"first runtime fixture")?;
        let spec = crate::inference::LaunchSpec {
            backend: crate::inference::Backend::Metal,
            directory: runtime,
            device: None,
            identity: String::new(),
            selectors: crate::inference::SelectorEnv::capture(crate::inference::Backend::Metal),
        };
        let meeting = engine.create_meeting("Fictional Metal lifecycle")?;
        save_fixture_take(&engine, &meeting.id, 1)?;
        let first = worker_key(
            &engine,
            &prefs,
            &meeting.id,
            meeting.run,
            &spec,
            &model_identity,
        )?;
        engine.inference_pool.reject_metal(&first, false);
        assert!(engine.inference_pool.metal_rejected(&first));
        save_fixture_take(&engine, &meeting.id, 1)?;
        assert_eq!(
            first,
            worker_key(
                &engine,
                &prefs,
                &meeting.id,
                meeting.run,
                &spec,
                &model_identity
            )?
        );
        save_fixture_take(&engine, &meeting.id, 2)?;
        assert_eq!(
            engine.store.lock().unwrap().meeting(&meeting.id)?.run,
            meeting.run
        );
        assert_ne!(
            first,
            worker_key(
                &engine,
                &prefs,
                &meeting.id,
                meeting.run,
                &spec,
                &model_identity
            )?
        );
        let first = worker_key(
            &engine,
            &prefs,
            &meeting.id,
            meeting.run,
            &spec,
            &model_identity,
        )?;
        assert_ne!(
            first,
            worker_key(
                &engine,
                &prefs,
                &meeting.id,
                meeting.run + 1,
                &spec,
                &model_identity
            )?
        );
        let mut other = prefs.clone();
        other.context_size += 512;
        assert_ne!(
            first,
            worker_key(
                &engine,
                &other,
                &meeting.id,
                meeting.run,
                &spec,
                &model_identity
            )?
        );
        std::fs::write(&executable, b"different runtime fixture")?;
        assert_ne!(
            first,
            worker_key(
                &engine,
                &prefs,
                &meeting.id,
                meeting.run,
                &spec,
                &model_identity
            )?
        );
        let model_path = directory.join(&model.files[0].name);
        #[cfg(not(windows))]
        {
            std::fs::write(&model_path, b"evil")?;
            assert!(receipt.check_current().is_err());
        }
        #[cfg(windows)]
        {
            assert!(std::fs::write(&model_path, b"evil").is_err());
            engine.model_integrity.invalidate(&directory);
            drop(receipt);
            std::fs::write(&model_path, b"evil")?;
        }
        assert!(engine.model_integrity.verify(&directory, &model).is_err());
        assert_ne!(
            first,
            worker_key(
                &engine,
                &prefs,
                &meeting.id,
                meeting.run,
                &spec,
                "changed verified asset identity"
            )?
        );
        Ok(())
    }
    fn save_fixture_take(engine: &Engine, meeting: &str, number: u32) -> Result<()> {
        engine
            .store
            .lock()
            .unwrap()
            .save_take(&crate::RecordingTake {
                id: format!("{meeting}:take:{number}"),
                meeting_id: meeting.into(),
                number,
                started_at: "2026-10-27T00:00:00Z".into(),
                start_sample: u64::from(number - 1) * 16000,
                end_sample: Some(u64::from(number) * 16000),
                status: "complete".into(),
            })
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    #[ignore = "explicit native Metal startup-fault injection with real CPU fallback; requires local public weights/runtime"]
    fn live_metal_startup_fallback() -> Result<()> {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let native = std::env::var("OMASCRIBE_TEST_NATIVE_LLAMA")?;
        let model_store = std::env::var("OMASCRIBE_TEST_MODEL_STORE")?;
        let root = tempfile::tempdir()?;
        symlink(model_store, root.path().join("model-store"))?;
        let runtime = root.path().join("runtime/llama");
        std::fs::create_dir_all(&runtime)?;
        std::fs::write(runtime.join("build-info.json"), br#"{"metal":true}"#)?;
        let launches = root.path().join("launches");
        let quoted = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        let script = format!("#!/bin/sh\nprevious=''\nfor argument; do\n if [ \"$previous\" = '--n-gpu-layers' ] && [ \"$argument\" != '0' ]; then\n  printf 'metal\\n' >> {log}\n  exit 70\n fi\n previous=\"$argument\"\ndone\nprintf 'cpu\\n' >> {log}\nexec {native} \"$@\"\n", log=quoted(launches.to_str().unwrap()), native=quoted(&native));
        let executable = runtime.join("llama-server");
        std::fs::write(&executable, script)?;
        std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o700))?;
        let mut engine = Engine::open(root.path().into())?;
        Arc::get_mut(&mut engine).unwrap().runtime_dir = root.path().join("runtime");
        let prefs = crate::preferences::Preferences {
            inference_backend: "metal".into(),
            threads: 1,
            memory_budget_mib: 4096,
            ..Default::default()
        };
        let meeting = engine.create_meeting("Fictional fallback")?;
        for take in [1, 1, 2] {
            save_fixture_take(&engine, &meeting.id, take)?;
            let session = open_session(&engine, &prefs, &meeting.id, meeting.run)?;
            assert_eq!(session.backend, crate::inference::Backend::Cpu);
            assert!(session
                .agent
                .get(&format!("{}/health", session.base))
                .set("Authorization", &session.auth)
                .call()
                .is_ok());
            drop(session);
            assert_eq!(engine.llm_pid.load(Ordering::SeqCst), 0);
            assert!(!engine.inference_pool.status().leased);
        }
        assert_eq!(
            std::fs::read_to_string(launches)?,
            "metal\ncpu\ncpu\nmetal\ncpu\n"
        );
        assert_eq!(engine.inference_pool.metal_failed_requests(), 0);
        let report: serde_json::Value = serde_json::from_slice(&std::fs::read(
            root.path().join("last-metal-fallback.json"),
        )?)?;
        assert_eq!(report["stage"], "startup");
        assert_ne!(engine.acceleration_status().state, "cpu_fallback");
        let low_budget = crate::preferences::Preferences {
            memory_budget_mib: 1024,
            ..prefs.clone()
        };
        let error = open_session(&engine, &low_budget, &meeting.id, meeting.run)
            .err()
            .context("budget refusal should fail before launching a worker")?;
        assert!(error.to_string().starts_with("writer:resource-budget:"));
        assert_eq!(
            std::fs::read_to_string(root.path().join("launches"))?,
            "metal\ncpu\ncpu\nmetal\ncpu\n"
        );
        Ok(())
    }
    #[test]
    fn invalidation_while_leased_and_failed_spawn_release_ownership() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let first = lease(&engine, "model", 30)?;
        let epoch = engine.inference_pool.0.lock().unwrap().epoch;
        let error = engine
            .remove_acceleration_component("fixture-gpu", "acceleration-fixture")
            .unwrap_err();
        assert!(error.to_string().contains("local inference"));
        assert!(engine.inference_pool.status().leased);
        assert_eq!(engine.inference_pool.0.lock().unwrap().epoch, epoch);
        assert_eq!(
            engine.llm_pid.load(Ordering::SeqCst),
            first.server.child.id()
        );
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

    #[cfg(not(windows))]
    #[test]
    fn model_change_reaps_gpu_lease_without_rejection_or_cpu_retry() -> Result<()> {
        use sha2::Digest;
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let directory = root.path().join("fixture");
        std::fs::create_dir(&directory)?;
        let mut model = crate::models::model("qwen3-1.7b")?.clone();
        model.files.truncate(1);
        model.files[0].bytes = 4;
        model.files[0].sha256 = format!("{:x}", sha2::Sha256::digest(b"good"));
        let path = directory.join(&model.files[0].name);
        std::fs::write(&path, b"good")?;
        let receipt = engine.model_integrity.verify(&directory, &model)?;
        let mut calls = 0;
        let result = crate::acceleration::with_cpu_fallback(&engine, || {
            calls += 1;
            let session = checkout(
                &engine,
                "gpu-fixture".into(),
                Duration::from_secs(30),
                || {
                    let mut session = fake(&engine)?;
                    session.backend = crate::inference::Backend::Cuda;
                    session.model_receipt = Some(receipt.clone());
                    Ok(session)
                },
            )?;
            session.begin_request()?;
            std::fs::write(&path, b"evil")?;
            session.complete_request()
        });
        assert!(result
            .unwrap_err()
            .to_string()
            .starts_with("model:integrity:"));
        assert_eq!(calls, 1);
        assert_ne!(engine.acceleration_status().state, "cpu_fallback");
        assert!(!engine.inference_pool.status().leased);
        assert!(engine.inference_pool.status().idle_pid.is_none());
        assert_eq!(engine.llm_pid.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn corrupt_llm_is_refused_before_backend_selection_or_worker_launch() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut engine = Engine::open(root.path().into())?;
        Arc::get_mut(&mut engine).unwrap().models = root.path().join("no-bundled-model");
        let prefs = crate::preferences::Preferences::default();
        let model = crate::models::model(&prefs.llm_model)?;
        let directory = root.path().join("model-store").join(&model.id);
        std::fs::create_dir_all(&directory)?;
        for asset in &model.files {
            // Entry ordering fixture: fail metadata admission without hashing
            // gigabytes of sparse data. Same-size SHA failures use tiny fixtures.
            File::create(directory.join(&asset.name))?.set_len(asset.bytes + 1)?;
        }
        let error = open_session(&engine, &prefs, "unused", 1)
            .err()
            .context("corrupt model admitted")?;
        assert!(error.to_string().starts_with("model:integrity:"));
        assert_eq!(engine.inference_pool.status().starts, 0);
        assert!(!engine.inference_pool.status().leased);
        assert_eq!(engine.llm_pid.load(Ordering::SeqCst), 0);
        assert!(!root.path().join("last-metal-fallback.json").exists());
        assert!(!root.path().join("llm-runtime.log").exists());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn verified_windows_lease_blocks_writes_then_releases_for_model_removal() -> Result<()> {
        use sha2::Digest;
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let directory = root.path().join("fixture");
        std::fs::create_dir(&directory)?;
        let mut model = crate::models::model("qwen3-1.7b")?.clone();
        model.files.truncate(1);
        model.files[0].bytes = 4;
        model.files[0].sha256 = format!("{:x}", sha2::Sha256::digest(b"good"));
        let path = directory.join(&model.files[0].name);
        std::fs::write(&path, b"good")?;
        let receipt = engine.model_integrity.verify(&directory, &model)?;
        let session = checkout(
            &engine,
            "windows-model".into(),
            Duration::from_secs(30),
            || {
                let mut session = fake(&engine)?;
                session.backend = crate::inference::Backend::Cuda;
                session.model_receipt = Some(receipt.clone());
                Ok(session)
            },
        )?;
        session.begin_request()?;
        assert!(std::fs::write(&path, b"evil").is_err());
        session.complete_request()?;
        drop(session);
        assert_ne!(engine.acceleration_status().state, "cpu_fallback");
        engine.inference_pool.invalidate();
        engine.model_integrity.clear();
        drop(receipt);
        std::fs::write(&path, b"evil")?;
        assert!(engine.model_integrity.verify(&directory, &model).is_err());
        std::fs::remove_dir_all(directory)?;
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
                session.begin_request()?;
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
                session.complete_request()?;
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
