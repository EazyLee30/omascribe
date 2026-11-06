//! Runtime receipts and translation identities; no prompt or meeting content in the cache.
mod runtime_file;
use crate::{preferences::Preferences, store::Store, Engine, Meeting};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::SystemTime,
};

#[derive(Debug)]
struct RuntimeIdentityError;
impl std::fmt::Display for RuntimeIdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("runtime:integrity: local runtime changed or could not be observed")
    }
}
impl std::error::Error for RuntimeIdentityError {}
pub(crate) fn integrity_error(error: &anyhow::Error) -> bool {
    error.is::<RuntimeIdentityError>()
}
fn runtime_error(error: impl Into<anyhow::Error>) -> anyhow::Error {
    error.into().context(RuntimeIdentityError)
}

struct ObservedRuntime {
    path: PathBuf,
    digest: String,
    receipt: Mutex<Option<runtime_file::Receipt>>,
    declared_system_loads: Mutex<Option<bool>>,
}
#[derive(Clone)]
pub(crate) struct RuntimeReceipt(Arc<ObservedRuntime>);
impl RuntimeReceipt {
    pub(crate) fn declared_system_loads(&self) -> bool {
        let mut cached = self.0.declared_system_loads.lock().unwrap();
        if self.check_current().is_err() {
            return false;
        }
        if let Some(value) = *cached {
            return value;
        }
        let value = fs::File::open(&self.0.path).ok().is_some_and(|mut file| {
            crate::inference_worker::rejection::sealed_macho(&mut file).is_ok()
        }) && self.check_current().is_ok();
        *cached = Some(value);
        value
    }
    pub(crate) fn digest(&self) -> &str {
        &self.0.digest
    }
    pub(crate) fn check_current(&self) -> Result<()> {
        let mut held = self.0.receipt.lock().unwrap();
        let result = held
            .as_ref()
            .context("runtime receipt revoked")
            .and_then(|r| r.check_current());
        if result.is_err() {
            held.take();
        }
        result.map_err(runtime_error)
    }
    fn revoke(&self) {
        self.0.receipt.lock().unwrap().take();
    }
}
#[derive(Default)]
struct State {
    entries: HashMap<PathBuf, RuntimeReceipt>,
    gates: HashMap<PathBuf, Weak<Mutex<()>>>,
    live: Vec<Weak<ObservedRuntime>>,
    contexts: HashMap<String, RuntimeContext>,
    blocked: Vec<(u64, PathBuf)>,
    epoch: u64,
    next_block: u64,
}
#[derive(Default)]
pub(crate) struct Fingerprints {
    state: Mutex<State>,
    #[cfg(test)]
    hashed: std::sync::atomic::AtomicU64,
}
fn normalized(path: &Path) -> Result<PathBuf> {
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        anyhow::bail!("linked runtime input");
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut clean = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                clean.pop();
            }
            other => clean.push(other.as_os_str()),
        }
    }
    if let (Some(parent), Some(name)) = (clean.parent(), clean.file_name()) {
        if let Ok(parent) = parent.canonicalize() {
            return Ok(parent.join(name));
        }
    }
    Ok(clean)
}
fn blocked(state: &State, path: &Path) -> bool {
    state.blocked.iter().any(|(_, dir)| path.starts_with(dir))
}
impl Fingerprints {
    /// Fast validation only. It never opens a content stream or repairs a stale receipt.
    pub(crate) fn current(&self, path: &Path) -> Option<RuntimeReceipt> {
        self.current_checked(path).ok().flatten()
    }
    /// Distinguish an unobserved file from an observed runtime that changed.
    pub(crate) fn current_checked(&self, path: &Path) -> Result<Option<RuntimeReceipt>> {
        let path = normalized(path).map_err(runtime_error)?;
        let entry = {
            let state = self.state.lock().unwrap();
            anyhow::ensure!(!blocked(&state, &path), RuntimeIdentityError);
            let Some(entry) = state.entries.get(&path) else {
                return Ok(None);
            };
            entry.clone()
        };
        let observed = entry.check_current();
        if observed.is_ok() {
            let state = self.state.lock().unwrap();
            return Ok((!blocked(&state, &path)
                && state
                    .entries
                    .get(&path)
                    .is_some_and(|now| Arc::ptr_eq(&now.0, &entry.0)))
            .then_some(entry));
        }
        let mut state = self.state.lock().unwrap();
        if state
            .entries
            .get(&path)
            .is_some_and(|now| Arc::ptr_eq(&now.0, &entry.0))
        {
            state.entries.remove(&path);
        }
        observed.map(|()| None)
    }
    /// Admission-only refresh. Call outside Store/preferences/pool/global-map locks.
    pub(crate) fn refresh(&self, path: &Path) -> Result<RuntimeReceipt> {
        self.refresh_with(path, || {})
    }
    pub(crate) fn refresh_bounded(&self, path: &Path, maximum: u64) -> Result<RuntimeReceipt> {
        self.refresh_bounded_with(path, Some(maximum), || {})
    }
    fn refresh_with(&self, path: &Path, before_observe: impl FnOnce()) -> Result<RuntimeReceipt> {
        self.refresh_bounded_with(path, None, before_observe)
    }
    fn refresh_bounded_with(
        &self,
        path: &Path,
        maximum: Option<u64>,
        before_observe: impl FnOnce(),
    ) -> Result<RuntimeReceipt> {
        let path = normalized(path).map_err(runtime_error)?;
        if let Some(entry) = self.current(&path) {
            return Ok(entry);
        }
        let gate = {
            let mut state = self.state.lock().unwrap();
            anyhow::ensure!(!blocked(&state, &path), RuntimeIdentityError);
            state.gates.retain(|_, gate| gate.strong_count() > 0);
            let gate = state
                .gates
                .get(&path)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| Arc::new(Mutex::new(())));
            state.gates.insert(path.clone(), Arc::downgrade(&gate));
            gate
        };
        let _exclusive = gate.lock().unwrap();
        if let Some(entry) = self.current(&path) {
            return Ok(entry);
        }
        let epoch = {
            let state = self.state.lock().unwrap();
            anyhow::ensure!(!blocked(&state, &path), RuntimeIdentityError);
            state.epoch
        };
        before_observe();
        let observe = |bytes| {
            #[cfg(test)]
            self.hashed
                .fetch_add(bytes as u64, std::sync::atomic::Ordering::Relaxed);
            #[cfg(not(test))]
            let _ = bytes;
        };
        let receipt = if let Some(maximum) = maximum {
            runtime_file::Receipt::observe_bounded(&path, maximum, observe)
        } else {
            runtime_file::Receipt::observe(&path, observe)
        }
        .map_err(runtime_error)?;
        receipt.check_current().map_err(runtime_error)?;
        let entry = RuntimeReceipt(Arc::new(ObservedRuntime {
            path: path.clone(),
            digest: receipt.digest().to_owned(),
            receipt: Mutex::new(Some(receipt)),
            declared_system_loads: Mutex::new(None),
        }));
        let retired = {
            let mut state = self.state.lock().unwrap();
            if state.epoch != epoch || blocked(&state, &path) {
                drop(state);
                entry.revoke();
                return Err(RuntimeIdentityError.into());
            }
            let retired = if state.entries.len() >= 16 {
                std::mem::take(&mut state.entries)
            } else {
                HashMap::new()
            };
            state.live.retain(|entry| entry.strong_count() > 0);
            state.live.push(Arc::downgrade(&entry.0));
            state.entries.insert(path, entry.clone());
            retired
        };
        drop(retired); // File handles never close under the global map lock.
        Ok(entry)
    }
    /// Revoke and release existing owners, then join same-directory in-flight observations.
    pub(crate) fn block_directory(&self, directory: &Path) -> Result<BlockedDirectory<'_>> {
        let directory = directory
            .canonicalize()
            .or_else(|_| normalized(directory))
            .map_err(runtime_error)?;
        let (id, entries, gates) = {
            let mut state = self.state.lock().unwrap();
            state.epoch = state.epoch.wrapping_add(1);
            state.next_block = state.next_block.wrapping_add(1);
            let id = state.next_block;
            state.blocked.push((id, directory.clone()));
            let entries: Vec<_> = state
                .live
                .iter()
                .filter_map(Weak::upgrade)
                .filter(|entry| entry.path.starts_with(&directory))
                .map(RuntimeReceipt)
                .collect();
            state.entries.retain(|p, _| !p.starts_with(&directory));
            state
                .contexts
                .retain(|_, context| !context.receipt.0.path.starts_with(&directory));
            state.live.retain(|entry| entry.strong_count() > 0);
            let gates: Vec<_> = state
                .gates
                .iter()
                .filter(|(p, _)| p.starts_with(&directory))
                .filter_map(|(_, g)| g.upgrade())
                .collect();
            (id, entries, gates)
        };
        for entry in entries {
            entry.revoke();
        }
        for gate in gates {
            drop(gate.lock().unwrap());
        }
        Ok(BlockedDirectory { cache: self, id })
    }
    #[cfg(test)]
    pub(crate) fn file(&self, path: &Path) -> Option<String> {
        self.refresh(path).ok().map(|r| r.digest().to_owned())
    }
    #[cfg(test)]
    pub(crate) fn hashed_bytes(&self) -> u64 {
        self.hashed.load(std::sync::atomic::Ordering::Relaxed)
    }
}
pub(crate) struct BlockedDirectory<'a> {
    cache: &'a Fingerprints,
    id: u64,
}
impl Drop for BlockedDirectory<'_> {
    fn drop(&mut self) {
        self.cache
            .state
            .lock()
            .unwrap()
            .blocked
            .retain(|(id, _)| *id != self.id);
    }
}
fn stamp(path: &Path) -> Option<Value> {
    let metadata = fs::metadata(path).ok()?;
    Some(json!([
        metadata.len(),
        metadata
            .modified()
            .ok()?
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_nanos()
            .to_string()
    ]))
}
#[derive(Clone)]
struct RuntimeContext {
    receipt: RuntimeReceipt,
    identity: Value,
    component: String,
}
fn configuration(prefs: &Preferences) -> Value {
    json!({"model":prefs.llm_model,"context":prefs.context_size,"threads":prefs.threads,
        "backend":prefs.inference_backend,"mode":prefs.resource_mode,"memory_budget":prefs.memory_budget_mib})
}
fn configuration_key(prefs: &Preferences) -> String {
    format!("{:x}", Sha256::digest(configuration(prefs).to_string()))
}
fn active_binding(engine: &Engine, prefs: &Preferences) -> Option<String> {
    if prefs.inference_backend != "auto" || !cfg!(any(target_os = "windows", target_os = "linux")) {
        return None;
    }
    let path = engine.root.join("accelerators/active.json");
    if !fs::symlink_metadata(&path).ok()?.file_type().is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(16_385)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 16_384 {
        return None;
    }
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value["binding"]
        .as_str()
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_owned)
}
/// Record the actual admitted backend, including a CPU fallback, outside Store/pool locks.
/// This is process-local cache provenance, never authorization to launch a component.
pub(crate) fn record_runtime(
    engine: &Engine,
    prefs: &Preferences,
    spec: &crate::inference::LaunchSpec,
    receipt: &RuntimeReceipt,
) -> Result<()> {
    let entry = if spec.backend == crate::inference::Backend::Mlx {
        engine.runtime_dir.join("mlx/runner.py")
    } else {
        spec.directory
            .join(crate::platform::executable("llama-server"))
    };
    anyhow::ensure!(
        normalized(&entry).map_err(runtime_error)? == receipt.0.path,
        RuntimeIdentityError
    );
    receipt.check_current()?;
    let context = RuntimeContext {
        receipt: receipt.clone(),
        component: spec.identity.clone(),
        identity: json!({"entry":receipt.0.path,"sha256":receipt.digest(),"backend":spec.backend.name(),
            "component":spec.identity,"device":spec.device,"selectors":spec.selectors.digest()}),
    };
    let retired = {
        let mut state = engine.inference_fingerprints.state.lock().unwrap();
        anyhow::ensure!(!blocked(&state, &receipt.0.path), RuntimeIdentityError);
        let retired = if state.contexts.len() >= 16 {
            std::mem::take(&mut state.contexts)
        } else {
            HashMap::new()
        };
        let previous = state.contexts.insert(configuration_key(prefs), context);
        (retired, previous)
    };
    drop(retired);
    Ok(())
}
/// Cold admission only. No caller may hold Store, preferences or pool locks here.
pub(crate) fn refresh_environment(engine: &Engine, prefs: &Preferences) -> Result<()> {
    let cancelled = || {
        engine.note_cancel.load(std::sync::atomic::Ordering::SeqCst)
            || engine.maintenance.load(std::sync::atomic::Ordering::SeqCst)
    };
    let check = || -> Result<()> {
        anyhow::ensure!(!cancelled(), crate::model_integrity::VerificationCancelled);
        Ok(())
    };
    check()?;
    // Optional authorization needs a verified model receipt. A UI poll must never do this work.
    if active_binding(engine, prefs).is_some() {
        engine.verified_model_cancellable(&prefs.llm_model, &cancelled)?;
    }
    check()?;
    let spec = crate::inference::launch_spec(engine, prefs)?;
    let path = if spec.backend == crate::inference::Backend::Mlx {
        engine.runtime_dir.join("mlx/runner.py")
    } else {
        spec.directory
            .join(crate::platform::executable("llama-server"))
    };
    // Runtime executable verification still performs one complete observation;
    // cancellation is checked around it, not between executable hash reads.
    check()?;
    let receipt = engine.inference_fingerprints.refresh(&path)?;
    check()?;
    record_runtime(engine, prefs, &spec, &receipt)?;
    check()
}
fn current_runtime(engine: &Engine, prefs: &Preferences) -> Result<Value> {
    let context = {
        let state = engine.inference_fingerprints.state.lock().unwrap();
        let context = state
            .contexts
            .get(&configuration_key(prefs))
            .cloned()
            .ok_or(RuntimeIdentityError)?;
        anyhow::ensure!(
            !blocked(&state, &context.receipt.0.path),
            RuntimeIdentityError
        );
        context
    };
    context.receipt.check_current()?;
    if cfg!(any(target_os = "windows", target_os = "linux")) && prefs.inference_backend == "auto" {
        anyhow::ensure!(
            active_binding(engine, prefs).as_deref().unwrap_or("") == context.component,
            RuntimeIdentityError
        );
    }
    let state = engine.inference_fingerprints.state.lock().unwrap();
    anyhow::ensure!(
        !blocked(&state, &context.receipt.0.path)
            && state
                .contexts
                .get(&configuration_key(prefs))
                .is_some_and(|now| Arc::ptr_eq(&now.receipt.0, &context.receipt.0)
                    && now.identity == context.identity),
        RuntimeIdentityError
    );
    Ok(context.identity)
}
/// Deterministic closed translations have no runtime/model execution dependency.
pub(crate) fn literal_environment(
    db: &Store,
    meeting: &Meeting,
    prefs: &Preferences,
) -> Result<String> {
    let input = json!({"schema":"literal-caption-v1","model":prefs.llm_model,"mode":prefs.translation_mode,
        "reported_source_names":crate::translation::SOURCE_REPORTED_NAMES_VERSION,
        "scope":{"meeting":meeting.id,"run":meeting.run,"terms":db.cache_dependency(&meeting.id,"terms")?,
            "speakers":meeting.speaker_names,"actors":crate::source_context::digest(db, meeting)?}});
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&input)?)))
}
/// Fast current observations only; Unknown must never be serialized as a shared null key.
pub(crate) fn environment(
    engine: &Engine,
    db: &Store,
    meeting: &Meeting,
    prefs: &Preferences,
) -> Result<String> {
    let model = crate::models::model(&prefs.llm_model)?;
    let directory = engine.model_directory(&model.id).ok();
    let assets: Vec<_> = model
        .files
        .iter()
        .map(|asset| {
            json!([
                asset.name,
                asset.sha256,
                directory.as_ref().and_then(|p| stamp(&p.join(&asset.name)))
            ])
        })
        .collect();
    let runtime = current_runtime(engine, prefs)?;
    let actor_context = crate::source_context::digest(db, meeting)?;
    let input = json!({"schema":"translation-cache-v32-runtime-receipt","model":model.id,"assets":assets,
        "runtime":runtime,
        "template":crate::translation::SYSTEM_TEMPLATE,"output_schema":crate::translation::OUTPUT_SCHEMA,
        "reported_source_names":crate::translation::SOURCE_REPORTED_NAMES_VERSION,
        "generation":{"seed":42,"temperature":0,"max_tokens":crate::translation::chunks::OUTPUT_TOKENS,"thinking":false,"input_budget":crate::translation::chunks::INPUT_TOKENS},
        "configuration":configuration(prefs),
        "permissions":{"knowledge":prefs.use_knowledge,"memory":prefs.use_memory},
        "scope":{"meeting":meeting.id,"run":meeting.run,"terms":db.cache_dependency(&meeting.id,"terms")?,"speakers":meeting.speaker_names,"actors":actor_context}});
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&input)?)))
}
pub(crate) fn translation_key(environment: &str, source: &crate::Segment, target: &str) -> String {
    let input = json!([
        environment,
        source.id,
        source.meeting_id,
        source.run,
        source.revision,
        source.text,
        source.speaker,
        source.speaker_manual,
        target
    ]);
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&input).expect("translation cache identity"))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_cold_refresh_does_not_read_or_publish_an_environment() -> Result<()> {
        use std::sync::atomic::Ordering;
        for maintenance in [false, true] {
            let root = tempfile::tempdir()?;
            let mut engine = Engine::open(root.path().into())?;
            let isolated = Arc::get_mut(&mut engine).context("Fresh engine must be unique")?;
            isolated.models = root.path().join("no-models");
            isolated.runtime_dir = root.path().join("fixture-runtime");
            let prefs = Preferences {
                inference_backend: "cpu".into(),
                ..Default::default()
            };
            let executable = engine
                .runtime_dir
                .join("llama")
                .join(crate::platform::executable("llama-server"));
            fs::create_dir_all(executable.parent().unwrap())?;
            let bytes = b"cold admission runtime fixture";
            fs::write(&executable, bytes)?;
            if maintenance {
                engine.maintenance.store(true, Ordering::SeqCst);
            } else {
                engine.note_cancel.store(true, Ordering::SeqCst);
            }
            let error = refresh_environment(&engine, &prefs).unwrap_err();
            assert!(crate::model_integrity::is_cancelled(&error));
            assert!(!integrity_error(&error));
            assert_eq!(engine.model_integrity.hashed_bytes(), 0);
            assert_eq!(engine.inference_fingerprints.hashed_bytes(), 0);
            {
                let state = engine.inference_fingerprints.state.lock().unwrap();
                assert!(state.entries.is_empty());
                assert!(state.contexts.is_empty());
            }
            // The same real entry would observe and bind this fixture without
            // cancellation; no worker launch or synthetic lifecycle is involved.
            engine.maintenance.store(false, Ordering::SeqCst);
            engine.note_cancel.store(false, Ordering::SeqCst);
            refresh_environment(&engine, &prefs)?;
            assert_eq!(
                engine.inference_fingerprints.hashed_bytes(),
                bytes.len() as u64
            );
            current_runtime(&engine, &prefs)?;
        }
        Ok(())
    }
    #[test]
    fn runtime_fingerprint_changes_when_executable_changes() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("runtime-fixture");
        let cache = Fingerprints::default();
        fs::write(&path, b"first executable")?;
        let first = cache.file(&path).unwrap();
        assert_eq!(cache.file(&path).as_ref(), Some(&first));
        drop(cache.block_directory(root.path())?);
        fs::write(&path, b"different executable bytes")?;
        assert_ne!(cache.file(&path).as_ref(), Some(&first));
        drop(cache.block_directory(root.path())?);
        fs::remove_file(&path)?;
        assert!(cache.file(&path).is_none());
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn runtime_fingerprint_changes_after_same_size_write_with_preserved_mtime() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("runtime-fixture");
        fs::write(&path, b"runtime A")?;
        let modified = fs::metadata(&path)?.modified()?;
        let cache = Fingerprints::default();
        let first = cache.refresh(&path)?;
        let read = cache.hashed_bytes();
        for _ in 0..100 {
            cache
                .current(&path)
                .context("warm receipt")?
                .check_current()?;
        }
        assert_eq!(read, cache.hashed_bytes());
        fs::write(&path, b"runtime B")?;
        fs::File::options()
            .write(true)
            .open(&path)?
            .set_times(fs::FileTimes::new().set_modified(modified))?;
        assert!(first.check_current().is_err());
        assert!(cache.current(&path).is_none());
        assert_eq!(read, cache.hashed_bytes());
        assert_ne!(first.digest(), cache.refresh(&path)?.digest());
        Ok(())
    }
    #[test]
    fn directory_block_joins_inflight_and_refuses_late_publication() -> Result<()> {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        let root = tempfile::tempdir()?;
        let path = root.path().join("runtime-fixture");
        fs::write(&path, b"runtime A")?;
        let cache = Arc::new(Fingerprints::default());
        let (started_tx, started_rx) = mpsc::channel();
        let (continue_tx, continue_rx) = mpsc::channel();
        let refreshing = {
            let cache = cache.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                cache.refresh_with(&path, || {
                    started_tx.send(()).unwrap();
                    continue_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                })
            })
        };
        started_rx.recv_timeout(Duration::from_secs(5))?;
        let (blocked_tx, blocked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let removing = {
            let cache = cache.clone();
            let directory = root.path().to_owned();
            std::thread::spawn(move || -> Result<()> {
                let guard = cache.block_directory(&directory)?;
                blocked_tx.send(())?;
                release_rx.recv_timeout(Duration::from_secs(5))?;
                drop(guard);
                Ok(())
            })
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while cache.state.lock().unwrap().blocked.is_empty() {
            anyhow::ensure!(Instant::now() < deadline, "block was not installed");
            std::thread::yield_now();
        }
        assert!(
            blocked_rx.try_recv().is_err(),
            "removal must join the held admission gate"
        );
        assert!(cache.current(&path).is_none());
        assert!(integrity_error(
            &cache
                .refresh(&path)
                .err()
                .context("blocked refresh must fail")?
        ));
        continue_tx.send(())?;
        assert!(integrity_error(
            &refreshing
                .join()
                .unwrap()
                .err()
                .context("late publication must fail")?
        ));
        blocked_rx.recv_timeout(Duration::from_secs(5))?;
        assert!(cache.current(&path).is_none());
        release_tx.send(())?;
        removing.join().unwrap()?;
        cache.refresh(&path)?.check_current()?;
        Ok(())
    }
}
