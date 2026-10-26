//! Local inference policy. A resource budget is an admission/monitoring policy, not an OS quota.
use crate::{models, preferences::Preferences, Engine};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Cpu,
    Metal,
    Mlx,
    Cuda,
    Vulkan,
}
impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Metal => "metal",
            Self::Mlx => "mlx",
            Self::Cuda => "cuda",
            Self::Vulkan => "vulkan",
        }
    }
    pub fn optional_gpu(self) -> bool {
        matches!(self, Self::Cuda | Self::Vulkan)
    }
}
#[derive(Debug, Clone)]
pub(crate) struct LaunchSpec {
    pub backend: Backend,
    pub directory: PathBuf,
    pub device: Option<String>,
    pub identity: String,
}
pub(crate) fn launch_spec(engine: &Engine, prefs: &Preferences) -> Result<LaunchSpec> {
    if prefs.inference_backend == "auto" {
        if let Some(spec) = crate::acceleration::selected(engine, prefs) {
            return Ok(spec);
        }
    }
    let backend = resolve(prefs, &engine.runtime_dir)?;
    Ok(LaunchSpec {
        backend,
        directory: engine.runtime_dir.join(if backend == Backend::Mlx {
            "mlx"
        } else {
            "llama"
        }),
        device: None,
        identity: String::new(),
    })
}
pub fn apple_silicon() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}
fn metal_device_available() -> bool {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        // Virtual Macs may have an arm64 CPU without an exposed Metal device.
        // Cache the native capability check; do not launch an inference server to probe it.
        static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *AVAILABLE.get_or_init(|| unsafe {
            #[link(name = "Metal", kind = "framework")]
            extern "C" {
                fn MTLCreateSystemDefaultDevice() -> *mut std::ffi::c_void;
            }
            #[link(name = "objc")]
            extern "C" {
                fn objc_release(object: *mut std::ffi::c_void);
            }
            let device = MTLCreateSystemDefaultDevice();
            if device.is_null() {
                false
            } else {
                objc_release(device);
                true
            }
        })
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    false
}
#[derive(Deserialize, Default)]
struct BuildInfo {
    #[serde(default)]
    metal: bool,
}
pub fn runtime_backends(runtime: &Path) -> Vec<String> {
    let mut backends = vec!["cpu".into()];
    let info = std::fs::read(runtime.join("llama/build-info.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<BuildInfo>(&bytes).ok())
        .unwrap_or_default();
    if metal_device_available() && info.metal {
        backends.push("metal".into());
    }
    if apple_silicon()
        && runtime.join("mlx/venv/bin/python3").is_file()
        && runtime.join("mlx/runner.py").is_file()
        && runtime.join("mlx/build-info.json").is_file()
    {
        backends.push("mlx".into());
    }
    backends
}
pub fn resolve(prefs: &Preferences, runtime: &Path) -> Result<Backend> {
    let available = runtime_backends(runtime);
    let mlx_model = models::model(&prefs.llm_model)?.format == "mlx";
    let requested = match prefs.inference_backend.as_str() {
        "auto" if mlx_model => "mlx",
        "auto" if available.iter().any(|v| v == "metal") => "metal",
        "auto" => "cpu",
        other => other,
    };
    if !available.iter().any(|v| v == requested) {
        bail!("Local {requested} runtime is unavailable; choose an installed backend");
    }
    Ok(match requested {
        "metal" => Backend::Metal,
        "mlx" => Backend::Mlx,
        _ => Backend::Cpu,
    })
}
pub fn budget_mib(prefs: &Preferences) -> u64 {
    if prefs.memory_budget_mib > 0 {
        return prefs.memory_budget_mib;
    }
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let total = system.total_memory() / 1024 / 1024;
    default_budget_mib(total, &prefs.resource_mode)
}
fn default_budget_mib(total: u64, mode: &str) -> u64 {
    match mode {
        "eco" => (total / 4).clamp(1024, 2048),
        "quality" => (total / 2).clamp(2048, 8192),
        // CPU repacking plus the bundled writer peaks around 2.5 GiB RSS.
        // Leave most RAM for the desktop while permitting that working set on 7–8 GiB hosts.
        _ => (total * 2 / 5).clamp(1536, 4096),
    }
}
pub fn server_command(
    engine: &Engine,
    prefs: &Preferences,
    port: u16,
    token: &str,
) -> Result<(Command, Backend)> {
    let spec = launch_spec(engine, prefs)?;
    server_command_with_spec(engine, prefs, port, token, &spec)
}
pub(crate) fn server_command_with_spec(
    engine: &Engine,
    prefs: &Preferences,
    port: u16,
    token: &str,
    spec: &LaunchSpec,
) -> Result<(Command, Backend)> {
    let backend = spec.backend;
    let budget = budget_mib(prefs);
    let mut command = if backend == Backend::Mlx {
        let mut c = crate::platform::child_command(engine.runtime_dir.join("mlx/venv/bin/python3"));
        c.arg("-I")
            .arg(engine.runtime_dir.join("mlx/runner.py"))
            .arg("--model")
            .arg(engine.model_directory(&prefs.llm_model)?)
            .args([
                "--port",
                &port.to_string(),
                "--context",
                &prefs.context_size.to_string(),
                "--memory-mib",
                &budget.to_string(),
            ])
            .env("OMASCRIBE_RUNTIME_TOKEN", token);
        c
    } else {
        let directory = spec
            .directory
            .canonicalize()
            .context("Local runtime directory unavailable")?;
        let mut c = crate::platform::child_command(
            directory.join(crate::platform::executable("llama-server")),
        );
        let threads = if prefs.resource_mode == "eco" {
            prefs.threads.min(2)
        } else {
            prefs.threads
        };
        let (batch, ubatch) = match (backend, prefs.resource_mode.as_str()) {
            (_, "quality") => ("512", "128"),
            (Backend::Cpu, "eco") => ("32", "32"),
            (Backend::Cpu, _) => ("64", "64"),
            (_, "eco") => ("64", "64"),
            _ => ("128", "128"),
        };
        c.arg("-m")
            .arg(engine.model_file(&prefs.llm_model)?.canonicalize()?)
            .args([
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--api-key",
                token,
                "--ctx-size",
                &prefs.context_size.to_string(),
                "--threads",
                &threads.to_string(),
                "--threads-batch",
                &threads.to_string(),
                "--parallel",
                "1",
                "--n-gpu-layers",
                if backend == Backend::Metal || backend.optional_gpu() {
                    "99"
                } else {
                    "0"
                },
                "--batch-size",
                batch,
                "--ubatch-size",
                ubatch,
                "--prio",
                "-1",
                "--poll",
                "0",
                "--reasoning",
                "off",
                "--cache-ram",
                "0",
                "--no-webui",
                "--no-warmup",
                "--offline",
            ])
            .env("LD_LIBRARY_PATH", &directory)
            .env_remove("GGML_BACKEND_PATH")
            .current_dir(&directory);
        if backend == Backend::Metal {
            // The pinned runtime exposes actual model-layer offload at level 4.
            // Level 5 includes request contents and is not needed for admission.
            c.args(["--log-verbosity", "4"]);
        }
        if backend == Backend::Cpu {
            c.args(["--device", "none"]);
        } else if let Some(device) = &spec.device {
            c.args(["--device", device]);
        }
        c
    };
    command
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .env("HF_HUB_DISABLE_TELEMETRY", "1")
        .env("DO_NOT_TRACK", "1")
        .env("OMP_NUM_THREADS", "1")
        .env("OPENBLAS_NUM_THREADS", "1");
    Ok((command, backend))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auto_keeps_legacy_cpu_runtime_and_explicit_unavailable_backend_fails() {
        let root = tempfile::tempdir().unwrap();
        let mut p = Preferences::default();
        assert_eq!(resolve(&p, root.path()).unwrap(), Backend::Cpu);
        p.inference_backend = "metal".into();
        assert!(resolve(&p, root.path()).is_err());
        p.inference_backend = "mlx".into();
        assert!(p.validate().is_err());
    }
    #[test]
    fn metal_requires_an_explicit_runtime_build_manifest() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("llama")).unwrap();
        std::fs::write(
            root.path().join("llama/build-info.json"),
            br#"{"metal":true}"#,
        )
        .unwrap();
        let expected = if metal_device_available() {
            Backend::Metal
        } else {
            Backend::Cpu
        };
        assert_eq!(
            resolve(&Preferences::default(), root.path()).unwrap(),
            expected
        );
    }
    #[test]
    fn explicit_memory_budget_is_bounded_and_preserved() {
        let mut p = Preferences {
            memory_budget_mib: 2048,
            ..Default::default()
        };
        assert_eq!(budget_mib(&p), 2048);
        p.memory_budget_mib = 100;
        assert!(p.validate().is_err());
    }
    #[test]
    fn balanced_default_fits_bundled_cpu_writer_without_consuming_most_ram() {
        for total in [7168, 8192] {
            let budget = default_budget_mib(total, "balanced");
            assert!(budget >= 2600 && budget < total / 2);
        }
        assert_eq!(default_budget_mib(24576, "balanced"), 4096);
        assert_eq!(default_budget_mib(8192, "eco"), 2048);
    }
}
