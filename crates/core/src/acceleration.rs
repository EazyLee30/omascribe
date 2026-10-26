//! Test signed optional runtimes locally before selecting them for writing jobs.
use crate::{
    components,
    inference::{Backend, LaunchSpec},
    preferences::Preferences,
    Engine,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::Path,
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

#[derive(Default)]
pub(crate) struct State {
    status: Status,
    failed_requests: u64,
    hardware: Option<(Instant, String)>,
    nonce: String,
}
#[derive(Debug, Default, Clone, Serialize)]
pub struct Status {
    pub state: String,
    pub component_id: Option<String>,
    pub reason: Option<String>,
    pub report: Option<Report>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trial {
    pub startup_ms: u64,
    pub requests_ms: Vec<u64>,
    pub total_ms: u64,
    pub peak_rss_mib: Option<u64>,
    pub runtime_gpu_buffer_mib: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub cpu: Trial,
    pub gpu: Trial,
    pub faster: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Certificate {
    schema: u32,
    component_id: String,
    version: String,
    device: String,
    binding: String,
    report: Report,
}
fn certificate_path(engine: &Engine) -> std::path::PathBuf {
    engine.root.join("accelerators/active.json")
}
fn hash(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn stamp(path: &Path) -> Result<Value> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        bail!("Runtime files must be regular files");
    }
    Ok(json!([
        path,
        metadata.len(),
        metadata
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
            .to_string()
    ]))
}
fn hardware_key(engine: &Engine) -> Result<String> {
    {
        let state = engine.acceleration.lock().unwrap();
        if let Some((at, key)) = &state.hardware {
            if at.elapsed() < Duration::from_secs(30) {
                return Ok(key.clone());
            }
        }
    }
    let inventory = crate::hardware::detect(&engine.root, &engine.runtime_dir);
    if inventory.status != "detected" || inventory.devices.is_empty() {
        bail!("Hardware probe unavailable");
    }
    let mut state = engine.acceleration.lock().unwrap();
    if state.nonce.is_empty() {
        state.nonce = uuid::Uuid::new_v4().to_string();
    }
    // Unknown driver versions are session-bound; they cannot authorize a future launch.
    let uncertain = inventory.devices.iter().any(|d| d.driver_version.is_none());
    let devices: Vec<_> = inventory
        .devices
        .iter()
        .map(|d| {
            json!([
                d.local_id,
                d.vendor_id,
                d.device_id,
                d.driver,
                d.driver_version
            ])
        })
        .collect();
    let key = hash(&(devices, uncertain.then_some(&state.nonce)))?;
    state.hardware = Some((Instant::now(), key.clone()));
    Ok(key)
}
fn binding(
    engine: &Engine,
    prefs: &Preferences,
    component: &components::Component,
    directory: &Path,
    device: &str,
) -> Result<String> {
    let model = crate::models::model(&prefs.llm_model)?;
    if model.format != "gguf" {
        bail!("Optional GPU runtimes require a GGUF model");
    }
    components::exact_directory(component, directory)?;
    let files: Vec<_> = component
        .files
        .iter()
        .map(|f| stamp(&directory.join(&f.name)))
        .collect::<Result<_>>()?;
    hash(
        &json!({"schema":1,"hardware":hardware_key(engine)?,"model":stamp(&engine.model_file(&prefs.llm_model)?)?,"model_sha256":model.files[0].sha256,
        "archive":component.archive.sha256,"files":files,"device":device,"threads":prefs.threads,"context":prefs.context_size,
        "mode":prefs.resource_mode,"budget":crate::inference::budget_mib(prefs)}),
    )
}
fn backend(component: &components::Component) -> Result<Backend> {
    match component.backend.as_str() {
        "cuda" => Ok(Backend::Cuda),
        "vulkan" => Ok(Backend::Vulkan),
        _ => bail!("Component backend is not yet supported"),
    }
}
pub(crate) fn selected(engine: &Engine, prefs: &Preferences) -> Option<LaunchSpec> {
    // Explicit CPU/Metal/MLX choices never consult the optional component registry.
    if prefs.inference_backend != "auto" || !cfg!(any(target_os = "linux", target_os = "windows")) {
        return None;
    }
    let result = (|| -> Result<LaunchSpec> {
        let mut data = Vec::new();
        fs::File::open(certificate_path(engine))?
            .take(16_385)
            .read_to_end(&mut data)?;
        if data.len() > 16_384 {
            bail!("Invalid acceleration certificate");
        }
        let cert: Certificate = serde_json::from_slice(&data)?;
        if cert.schema != 1 || !cert.report.faster {
            bail!("Acceleration has not passed local verification");
        }
        let (_, component, directory) =
            components::installed_component(&engine.root, &cert.component_id, Some(&cert.version))?;
        if binding(engine, prefs, &component, &directory, &cert.device)? != cert.binding {
            bail!("Acceleration configuration changed; test again");
        }
        let mut state = engine.acceleration.lock().unwrap();
        state.status.state = "active".into();
        state.status.component_id = Some(cert.component_id);
        state.status.report = Some(cert.report);
        state.status.reason = None;
        Ok(LaunchSpec {
            backend: backend(&component)?,
            directory,
            device: Some(cert.device),
            identity: cert.binding,
        })
    })();
    if result.is_err() && certificate_path(engine).exists() {
        let mut state = engine.acceleration.lock().unwrap();
        state.status.state = "test_required".into();
        state.status.reason = Some("configuration_or_driver_changed".into());
        let _ = fs::remove_file(certificate_path(engine));
    }
    result.ok()
}
pub(crate) fn reject(engine: &Engine, reason: &str, request_failure: bool) {
    let mut state = engine.acceleration.lock().unwrap();
    let _ = fs::remove_file(certificate_path(engine));
    state.status.state = "cpu_fallback".into();
    state.status.reason = Some(reason.into());
    if request_failure {
        state.failed_requests = state.failed_requests.wrapping_add(1);
    }
}
/// Retry once after the lease rejects a GPU transport, preserving resource/cancel stops.
pub(crate) fn with_cpu_fallback<T>(
    engine: &Engine,
    mut operation: impl FnMut() -> Result<T>,
) -> Result<T> {
    let before = engine.acceleration.lock().unwrap().failed_requests;
    let metal_before = engine.inference_pool.metal_failed_requests();
    let result = operation();
    if result.is_err()
        && !engine.note_cancel.load(Ordering::SeqCst)
        && (engine.acceleration.lock().unwrap().failed_requests != before
            || engine.inference_pool.metal_failed_requests() != metal_before)
    {
        engine.inference_pool.invalidate();
        return operation();
    }
    result
}
pub(crate) fn offloaded(log: &str) -> bool {
    regex::Regex::new(r"offloaded [1-9][0-9]*/[0-9]+ layers to GPU")
        .unwrap()
        .is_match(log)
}
fn choose_device(listing: &str, backend: Backend, required_mib: u64) -> Result<String> {
    let prefix = match backend {
        Backend::Cuda => "CUDA",
        Backend::Vulkan => "Vulkan",
        _ => bail!("Unsupported GPU device"),
    };
    let pattern =
        regex::Regex::new(r"^\s*([A-Za-z]+[0-9]+):.*\([0-9]+ MiB, ([0-9]+) MiB free\)\s*$")?;
    listing
        .lines()
        .filter_map(|line| pattern.captures(line))
        .filter_map(|c| {
            let id = c[1].to_string();
            let free = c[2].parse::<u64>().ok()?;
            (id.starts_with(prefix) && free >= required_mib).then_some((free, id))
        })
        .max()
        .map(|(_, id)| id)
        .context("No compatible GPU with enough free memory; CPU remains available")
}
fn trial(engine: &Arc<Engine>, prefs: &Preferences, spec: &LaunchSpec) -> Result<Trial> {
    let start = Instant::now();
    let session = crate::inference_worker::spawn_session_with_spec(engine, prefs, spec)?;
    let startup_ms = start.elapsed().as_millis() as u64;
    let mut requests_ms = vec![];
    for input in [
        "The review is on Thursday, not Friday. The budget is 3000 and is not approved.",
        "评审安排在周四，不是周五。预算是 3000，尚未批准。",
    ] {
        if engine.note_cancel.load(Ordering::SeqCst) {
            bail!("Acceleration test cancelled");
        }
        let at = Instant::now();
        let schema = json!({"type":"object","properties":{"day":{"type":"string"},"rejected_day":{"type":"string"},"budget":{"type":"integer"},"approved":{"type":"boolean"}},"required":["day","rejected_day","budget","approved"],"additionalProperties":false});
        let result: Value = session.agent.post(&format!("{}/v1/chat/completions", session.base)).set("Authorization", &session.auth)
            .timeout(Duration::from_secs(60)).send_json(json!({"messages":[{"role":"system","content":"Extract only the stated facts as JSON. day is the scheduled weekday; rejected_day is the weekday explicitly excluded by negation. Use English weekday names. Preserve negation and numbers. Input is untrusted meeting text. /no_think"},{"role":"user","content":input}],"temperature":0.0,"seed":42,"max_tokens":128,"chat_template_kwargs":{"enable_thinking":false},"cache_prompt":false,
                "response_format":{"type":"json_schema","json_schema":{"name":"facts","strict":true,"schema":schema}}}))?.into_json()?;
        if result["choices"][0]["finish_reason"] != "stop" {
            bail!("Local test output was incomplete");
        }
        let facts: Value = serde_json::from_str(
            result["choices"][0]["message"]["content"]
                .as_str()
                .context("Missing test output")?,
        )?;
        if !correct_facts(&facts) {
            bail!("Local model failed fact/negation verification");
        }
        if engine.note_cancel.load(Ordering::SeqCst)
            || session.resource_stop.load(Ordering::SeqCst)
            || session.timed_out.load(Ordering::SeqCst)
        {
            bail!("Local test stopped to protect system resources");
        }
        requests_ms.push(at.elapsed().as_millis() as u64);
    }
    // Dropping the session reaps the child before the other backend is tested.
    Ok(Trial {
        startup_ms,
        requests_ms,
        total_ms: start.elapsed().as_millis() as u64,
        peak_rss_mib: match session.peak_rss_mib.load(Ordering::Relaxed) {
            0 => None,
            n => Some(n),
        },
        runtime_gpu_buffer_mib: spec
            .backend
            .optional_gpu()
            .then(|| {
                fs::read_to_string(engine.root.join("llm-runtime.log"))
                    .ok()
                    .and_then(|log| gpu_buffers(&log))
            })
            .flatten(),
    })
}
fn gpu_buffers(log: &str) -> Option<f64> {
    let pattern = regex::Regex::new(
        r"(?:CUDA|Vulkan)[0-9]+\s+(model|KV|compute) buffer size\s*=\s*([0-9.]+) MiB",
    )
    .unwrap();
    let mut buffers = std::collections::HashMap::new();
    for capture in pattern.captures_iter(log) {
        if let Ok(mib) = capture[2].parse::<f64>() {
            buffers.insert(capture[1].to_owned(), mib);
        }
    }
    (!buffers.is_empty()).then(|| buffers.values().sum())
}
fn correct_facts(value: &Value) -> bool {
    value.as_object().is_some_and(|o| o.len() == 4)
        && value["day"]
            .as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("Thursday"))
        && value["rejected_day"]
            .as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("Friday"))
        && value["budget"].as_u64() == Some(3000)
        && value["approved"].as_bool() == Some(false)
}
fn verify(engine: &Arc<Engine>, id: &str) -> Result<Report> {
    engine.inference_pool.invalidate();
    let prefs = engine.preferences.lock().unwrap().clone();
    let (index, component, directory) = components::installed_component(&engine.root, id, None)?;
    components::verify_component_files(&component, &directory, &engine.note_cancel)?;
    let kind = backend(&component)?;
    if kind == Backend::Cuda
        && crate::hardware::detect(&engine.root, &engine.runtime_dir)
            .cuda_driver_version
            .is_some_and(|v| v < 12000)
    {
        bail!("The installed NVIDIA driver does not support CUDA 12. Update the NVIDIA driver or test the Vulkan component; CPU remains available.");
    }
    let mut command = crate::platform::child_command(directory.join(&component.entrypoint));
    command
        .arg("--list-devices")
        .current_dir(&directory)
        .env("LD_LIBRARY_PATH", &directory)
        .env_remove("GGML_BACKEND_PATH");
    let listing = crate::probe::capture(&mut command, Duration::from_secs(15), 64 * 1024)?;
    let required = crate::models::model(&prefs.llm_model)?.memory_mib
        + u64::from(prefs.context_size.saturating_sub(2048)) / 4
        + 256;
    let device = choose_device(&String::from_utf8(listing)?, kind, required)?;
    let key = binding(engine, &prefs, &component, &directory, &device)?;
    engine.inference_pool.invalidate();
    let cpu_spec = LaunchSpec {
        backend: Backend::Cpu,
        directory: engine.runtime_dir.join("llama"),
        device: None,
        identity: String::new(),
    };
    let cpu = trial(engine, &prefs, &cpu_spec)?;
    let gpu_spec = LaunchSpec {
        backend: kind,
        directory: directory.clone(),
        device: Some(device.clone()),
        identity: key.clone(),
    };
    let gpu = trial(engine, &prefs, &gpu_spec)?;
    let faster = gpu.total_ms < cpu.total_ms && gpu.requests_ms.last() < cpu.requests_ms.last();
    let report = Report { cpu, gpu, faster };
    // A driver/config change or cancellation during a test cannot activate stale results.
    engine.acceleration.lock().unwrap().hardware = None;
    if engine.note_cancel.load(Ordering::SeqCst)
        || key != binding(engine, &prefs, &component, &directory, &device)?
    {
        bail!("Acceleration test configuration changed");
    }
    fs::write(
        engine.root.join("last-acceleration-test.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    if faster {
        fs::create_dir_all(engine.root.join("accelerators"))?;
        crate::preferences::atomic_json(
            &certificate_path(engine),
            &Certificate {
                schema: 1,
                component_id: id.into(),
                version: index.version,
                device,
                binding: key,
                report: report.clone(),
            },
        )?;
    } else {
        let _ = fs::remove_file(certificate_path(engine));
    }
    Ok(report)
}
impl Engine {
    pub(crate) fn acceleration_test_running(&self) -> bool {
        self.acceleration.lock().unwrap().status.state == "testing"
    }
    pub fn acceleration_status(&self) -> Status {
        // Serialize restoration with Test & enable, Use CPU and configuration
        // changes, then recheck state so an older query cannot overwrite them.
        let _config = self.config_lock.lock().unwrap();
        // A new Engine has no in-memory status. Revalidate the persisted opt-in
        // before displaying it, without waiting for a generation to select it.
        let restore = self.acceleration.lock().unwrap().status.state.is_empty();
        if restore {
            let prefs = self.preferences.lock().unwrap().clone();
            let _ = selected(self, &prefs);
        }
        self.acceleration.lock().unwrap().status.clone()
    }
    pub fn verify_acceleration(self: &Arc<Self>, id: &str) -> Result<()> {
        let gate = self.config_lock.lock().unwrap();
        self.require_available()?;
        self.configuration_idle()?;
        if self.preferences.lock().unwrap().inference_backend != "auto" {
            bail!("Choose Automatic inference before testing optional acceleration");
        }
        components::installed_component(&self.root, id, None)?;
        if self.note_busy.swap(true, Ordering::SeqCst) {
            bail!("Wait for local inference to finish");
        }
        self.note_cancel.store(false, Ordering::SeqCst);
        self.acceleration.lock().unwrap().status = Status {
            state: "testing".into(),
            component_id: Some(id.into()),
            ..Default::default()
        };
        self.status.lock().unwrap().notes_busy = true;
        drop(gate);
        let engine = self.clone();
        let id = id.to_owned();
        std::thread::spawn(move || {
            let result = verify(&engine, &id);
            let mut state = engine.acceleration.lock().unwrap();
            state.status = match result {
                Ok(report) => Status {
                    state: if report.faster {
                        "active"
                    } else {
                        "cpu_preferred"
                    }
                    .into(),
                    component_id: Some(id),
                    report: Some(report),
                    reason: None,
                },
                Err(error) => {
                    let _ = fs::remove_file(certificate_path(&engine));
                    Status {
                        state: "test_failed".into(),
                        component_id: Some(id),
                        reason: Some(format!("{error:#}")),
                        report: None,
                    }
                }
            };
            drop(state);
            engine.status.lock().unwrap().notes_busy = false;
            engine.note_busy.store(false, Ordering::SeqCst);
        });
        Ok(())
    }
    pub fn disable_acceleration(&self) -> Result<()> {
        let _gate = self.config_lock.lock().unwrap();
        self.configuration_idle()?;
        self.inference_pool.invalidate();
        let mut state = self.acceleration.lock().unwrap();
        let _ = fs::remove_file(certificate_path(self));
        state.status = Status::default();
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn status_revalidates_a_persisted_opt_in_before_the_first_job() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        fs::create_dir_all(root.path().join("accelerators"))?;
        // A persisted file is insufficient: the usual selector must reject an
        // unverified report even when no generation has run in this Engine.
        let trial = Trial {
            startup_ms: 0,
            requests_ms: vec![1, 1],
            total_ms: 2,
            peak_rss_mib: None,
            runtime_gpu_buffer_mib: None,
        };
        crate::preferences::atomic_json(
            &certificate_path(&engine),
            &Certificate {
                schema: 1,
                component_id: "fixture".into(),
                version: "acceleration-fixture".into(),
                device: "CUDA0".into(),
                binding: "unverified".into(),
                report: Report {
                    cpu: trial.clone(),
                    gpu: trial,
                    faster: false,
                },
            },
        )?;
        let status = engine.acceleration_status();
        assert_eq!(status.state, "test_required");
        assert_eq!(
            status.reason.as_deref(),
            Some("configuration_or_driver_changed")
        );
        assert!(!certificate_path(&engine).exists());
        Ok(())
    }
    #[test]
    fn status_does_not_replace_an_in_progress_test_with_a_persisted_file() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        fs::create_dir_all(root.path().join("accelerators"))?;
        fs::write(certificate_path(&engine), b"not a certificate")?;
        engine.acceleration.lock().unwrap().status.state = "testing".into();
        assert_eq!(engine.acceleration_status().state, "testing");
        assert!(certificate_path(&engine).exists());
        Ok(())
    }
    #[test]
    fn status_query_waits_for_newer_test_or_disable_decisions() -> Result<()> {
        use std::sync::mpsc;
        for testing in [true, false] {
            let root = tempfile::tempdir()?;
            let engine = Engine::open(root.path().into())?;
            fs::create_dir_all(root.path().join("accelerators"))?;
            fs::write(certificate_path(&engine), b"old certificate")?;
            let config = engine.config_lock.lock().unwrap();
            let (started_tx, started_rx) = mpsc::channel();
            let (result_tx, result_rx) = mpsc::channel();
            let reader = engine.clone();
            let query = std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                result_tx.send(reader.acceleration_status().state).unwrap();
            });
            started_rx.recv_timeout(Duration::from_secs(5))?;
            // The query began with empty status while a configuration action
            // owns its gate. It must wait, not read/delete the old certificate.
            assert!(matches!(
                result_rx.recv_timeout(Duration::from_millis(200)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            assert!(certificate_path(&engine).exists());
            if testing {
                engine.acceleration.lock().unwrap().status.state = "testing".into();
                fs::write(certificate_path(&engine), b"new test certificate")?;
            } else {
                fs::remove_file(certificate_path(&engine))?;
                engine.acceleration.lock().unwrap().status = Status::default();
            }
            drop(config);
            assert_eq!(
                result_rx.recv_timeout(Duration::from_secs(5))?,
                if testing { "testing" } else { "" }
            );
            query.join().expect("status query panicked");
            if testing {
                assert_eq!(
                    fs::read(certificate_path(&engine))?,
                    b"new test certificate"
                );
            } else {
                assert!(!certificate_path(&engine).exists());
            }
        }
        Ok(())
    }
    #[test]
    fn device_selection_requires_real_backend_free_memory_and_offload() -> Result<()> {
        let listing = "Available devices:\n CUDA0: GTX (4096 MiB, 3500 MiB free)\n CUDA1: Another (8192 MiB, 7000 MiB free)\n Vulkan0: AMD (8192 MiB, 6000 MiB free)\n";
        assert_eq!(choose_device(listing, Backend::Cuda, 4000)?, "CUDA1");
        assert_eq!(choose_device(listing, Backend::Vulkan, 2000)?, "Vulkan0");
        assert!(choose_device(listing, Backend::Cuda, 9000).is_err());
        assert!(choose_device("CUDA0: unknown memory", Backend::Cuda, 2000).is_err());
        assert!(!offloaded("offloaded 0/29 layers to GPU"));
        assert!(offloaded("llama_model_load: offloaded 29/29 layers to GPU"));
        assert!(!correct_facts(
            &json!({"day":"Friday","rejected_day":"Thursday","budget":3000,"approved":true})
        ));
        assert!(correct_facts(
            &json!({"day":"thursday","rejected_day":"friday","budget":3000,"approved":false})
        ));
        assert!(!correct_facts(
            &json!({"day":"Thursday","rejected_day":"Thursday","budget":3000,"approved":false})
        ));
        assert!(!correct_facts(
            &json!({"day":"Thursday","rejected_day":"Friday","budget":3000,"approved":true})
        ));
        assert_eq!(gpu_buffers("llama: CUDA0 model buffer size = 1000.25 MiB\nllama: CUDA0 KV buffer size = 40.00 MiB\n"), Some(1040.25));
        assert_eq!(
            gpu_buffers("CUDA device found, no memory measurement"),
            None
        );
        Ok(())
    }
    #[test]
    fn gpu_failure_retries_once_and_cancellation_never_retries() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let mut calls = 0;
        let result = with_cpu_fallback(&engine, || {
            calls += 1;
            reject(&engine, "fixture", true);
            bail!("fixture transport failure")
        }) as Result<()>;
        assert!(result.is_err());
        assert_eq!(calls, 2);
        engine.note_cancel.store(true, Ordering::SeqCst);
        calls = 0;
        let result = with_cpu_fallback(&engine, || {
            calls += 1;
            reject(&engine, "fixture", true);
            bail!("cancelled")
        }) as Result<()>;
        assert!(result.is_err());
        assert_eq!(calls, 1);
        Ok(())
    }
    #[test]
    #[ignore = "explicit developer GPU trial; never installs or activates a component"]
    fn live_developer_gpu_model_gate() -> Result<()> {
        let directory = std::path::PathBuf::from(std::env::var("OMASCRIBE_TEST_GPU_RUNTIME")?)
            .canonicalize()?;
        let kind = match std::env::var("OMASCRIBE_TEST_GPU_BACKEND").as_deref() {
            Ok("vulkan") => Backend::Vulkan,
            Ok("cuda") | Err(_) => Backend::Cuda,
            _ => bail!("Use cuda or vulkan for the explicit developer trial"),
        };
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let prefs = engine.preferences.lock().unwrap().clone();
        let spec = LaunchSpec {
            backend: kind,
            directory: directory.clone(),
            device: Some(
                if kind == Backend::Cuda {
                    "CUDA0"
                } else {
                    "Vulkan0"
                }
                .into(),
            ),
            identity: "explicit-developer-trial".into(),
        };
        let tested = trial(&engine, &prefs, &spec);
        let log = fs::read_to_string(engine.root.join("llm-runtime.log")).unwrap_or_default();
        let output = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/benchmarks");
        fs::create_dir_all(&output)?;
        fn sha(path: &Path) -> Result<String> {
            let mut stream = fs::File::open(path)?;
            let mut digest = Sha256::new();
            let mut bytes = [0u8; 128 * 1024];
            loop {
                let size = stream.read(&mut bytes)?;
                if size == 0 {
                    break;
                }
                digest.update(&bytes[..size]);
            }
            Ok(format!("{:x}", digest.finalize()))
        }
        let report = json!({"schema":1,"kind":"explicit_developer_gpu_admission","passed":tested.is_ok() && offloaded(&log),
            "build":crate::build_identity(),"backend":kind.name(),"runtime_sha256":sha(&directory.join(crate::platform::executable("llama-server")))?,
            "model_sha256":sha(&engine.model_file(&prefs.llm_model)?)?,"threads":prefs.threads,"context":prefs.context_size,
            "trial":tested.as_ref().ok(),"error":tested.as_ref().err().map(|e|format!("{e:#}")),"offloaded":offloaded(&log),
            "limitation":"Two fixed fictional bilingual fact/negation requests. Not a full-meeting quality or warm throughput benchmark. No production component activation."});
        fs::write(
            output.join("developer-gpu-gate.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        fs::write(output.join("developer-gpu-runtime.log"), log)?;
        tested?;
        anyhow::ensure!(report["offloaded"] == true, "GPU offload was not proven");
        Ok(())
    }
    #[test]
    #[ignore = "explicit real local CPU model test; no GPU performance claim"]
    fn live_cpu_model_gate() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let prefs = engine.preferences.lock().unwrap().clone();
        let spec = LaunchSpec {
            backend: Backend::Cpu,
            directory: engine.runtime_dir.join("llama"),
            device: None,
            identity: String::new(),
        };
        let report = trial(&engine, &prefs, &spec)?;
        assert_eq!(report.requests_ms.len(), 2);
        assert_eq!(engine.llm_pid.load(Ordering::SeqCst), 0);
        let output = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../artifacts/benchmarks/acceleration-cpu-gate.json");
        fs::create_dir_all(output.parent().unwrap())?;
        fn file_sha(path: &Path) -> Result<String> {
            let mut input = fs::File::open(path)?;
            let mut hasher = Sha256::new();
            let mut buffer = [0u8; 128 * 1024];
            loop {
                let n = input.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buffer[..n]);
            }
            Ok(format!("{:x}", hasher.finalize()))
        }
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let git_sha = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["rev-parse", "HEAD"])
            .output()?;
        let dirty = !std::process::Command::new("git")
            .current_dir(&repo)
            .args(["diff", "--quiet", "HEAD", "--", "crates/core"])
            .status()?
            .success();
        let evidence = json!({"schema":1,"kind":"cpu_local_admission","git_sha":String::from_utf8_lossy(&git_sha.stdout).trim(),"core_tracked_changes":dirty,
            "inference_sources_sha256":hash(&(include_str!("acceleration.rs"),include_str!("inference.rs"),include_str!("inference_worker.rs"),include_str!("components.rs"),include_str!("hardware.rs")))?,
            "runtime":{"sha256":file_sha(&spec.directory.join(crate::platform::executable("llama-server")))?,"build_info":serde_json::from_slice::<Value>(&fs::read(spec.directory.join("build-info.json"))?)?},
            "model":{"id":prefs.llm_model,"sha256":file_sha(&engine.model_file(&prefs.llm_model)?)?,"threads":prefs.threads,"context":prefs.context_size,"resource_mode":prefs.resource_mode,"budget_mib":crate::inference::budget_mib(&prefs)},
            "corpus":"two fixed synthetic English/Chinese fact and negation requests in acceleration::trial", "result":report});
        fs::write(output, serde_json::to_vec_pretty(&evidence)?)?;
        Ok(())
    }
}
