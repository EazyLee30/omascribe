//! Process-local observations for pausing identical rejected automatic minutes.
//! No worker, model handles or generated text is retained. Ordinary local file
//! observations do not establish immutable mmap/remote-filesystem identities.
use crate::{inference::Backend, inference_cache::RuntimeReceipt, Engine};
use anyhow::{ensure, Context, Result};
use std::{
    ffi::OsString,
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
};

const MAX_MEMBERS: usize = 16;
const MAX_COMMAND_BYTES: usize = 128 * 1024;
const MAX_RUNTIME_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Witness {
    model: crate::models::Model,
    directory: PathBuf,
    model_fingerprint: String,
    runtime_directory: PathBuf,
    members: Vec<(OsString, String)>,
    configured_backend: String,
    producer: Option<(PathBuf, String)>,
}

fn no_loader_overrides() -> bool {
    environment_supported(std::env::vars_os())
}

fn environment_supported(vars: impl IntoIterator<Item = (OsString, OsString)>) -> bool {
    !vars.into_iter().any(|(key, value)| {
        key.to_string_lossy().starts_with("DYLD_")
            || key.to_string_lossy().starts_with("GGML_")
            || key.to_string_lossy().starts_with("LLAMA_")
            || key.to_string_lossy().starts_with("MTL_")
            || key.to_string_lossy().starts_with("METAL_")
            || key.to_string_lossy().starts_with("LD_")
            || ([
                "OMASCRIBE_PREPARE_SOURCES",
                "OMASCRIBE_INCREMENTAL_MINUTES",
                "OMASCRIBE_GROUPED_MINUTES",
            ]
            .iter()
            .any(|name| key == *name)
                && value == "1")
    })
}

fn official_native_directory(engine: &Engine) -> Result<PathBuf> {
    ensure!(
        std::env::var_os("OMASCRIBE_RUNTIME_DIR").is_none(),
        "custom runtime override"
    );
    let exe = std::env::current_exe()?.canonicalize()?;
    let contents = exe
        .ancestors()
        .find(|path| {
            path.file_name().is_some_and(|name| name == "Contents")
                && path
                    .parent()
                    .and_then(Path::extension)
                    .is_some_and(|name| name == "app")
        })
        .context("not an installed app executable")?;
    let native = contents.join("Resources/native").canonicalize()?;
    for relative in ["Resources", "Resources/native", "Resources/native/runtime"] {
        ensure!(
            fs::symlink_metadata(contents.join(relative))?
                .file_type()
                .is_dir(),
            "linked bundle resources"
        );
    }
    ensure!(native.starts_with(contents), "external bundle resources");
    ensure!(
        (exe.starts_with(contents.join("MacOS")) || exe.starts_with(native.join("runtime")))
            && engine.runtime_dir.canonicalize()? == native.join("runtime").canonicalize()?,
        "custom runtime location"
    );
    Ok(native)
}

fn descriptor(path: &Path) -> Result<serde_json::Value> {
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(16_385).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 16_384, "runtime descriptor exceeds bound");
    Ok(serde_json::from_slice(&bytes)?)
}

fn members(directory: &Path) -> Result<Vec<OsString>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        ensure!(entry.file_type()?.is_file(), "unobserved runtime member");
        names.push(entry.file_name());
        ensure!(
            names.len() <= MAX_MEMBERS,
            "runtime inventory exceeds bound"
        );
    }
    names.sort();
    Ok(names)
}

fn runtime_directory(engine: &Engine) -> Result<PathBuf> {
    let base = engine.runtime_dir.canonicalize()?;
    let path = engine.runtime_dir.join("llama");
    ensure!(
        fs::symlink_metadata(&path)?.file_type().is_dir(),
        "linked llama directory"
    );
    let directory = path.canonicalize()?;
    ensure!(directory == base.join("llama"), "external llama directory");
    Ok(directory)
}

impl Witness {
    pub(super) fn observe(
        engine: &Engine,
        prefs: &crate::preferences::Preferences,
        spec: &crate::inference::LaunchSpec,
        model: &crate::model_integrity::Receipt,
        runtime: &RuntimeReceipt,
    ) -> Result<Self> {
        ensure!(
            crate::inference::apple_silicon(),
            "unsupported execution platform"
        );
        let native = official_native_directory(engine)?;
        let path = native.join("build-info.json");
        let receipt = engine
            .inference_fingerprints
            .refresh_bounded(&path, 16_384)?;
        let producer = descriptor(&path)?;
        let lock: serde_json::Value =
            serde_json::from_str(include_str!("../../../../models.lock.json"))?;
        ensure!(
            producer["target"] == "aarch64-apple-darwin"
                && producer["llama_cpp"] == lock["llama_cpp"]["revision"],
            "unknown producer recipe"
        );
        receipt.check_current()?;
        let mut witness = Self::observe_model(
            engine,
            prefs,
            spec,
            crate::models::model(&prefs.llm_model)?,
            model,
            runtime,
        )?;
        witness.producer = Some((path, receipt.digest().to_owned()));
        ensure!(
            witness.current(engine, prefs),
            "producer observation changed"
        );
        Ok(witness)
    }

    fn observe_model(
        engine: &Engine,
        prefs: &crate::preferences::Preferences,
        spec: &crate::inference::LaunchSpec,
        catalog: &crate::models::Model,
        model: &crate::model_integrity::Receipt,
        runtime: &RuntimeReceipt,
    ) -> Result<Self> {
        ensure!(no_loader_overrides(), "loader environment is unobserved");
        ensure!(catalog.format == "gguf", "unsupported model closure");
        ensure!(
            matches!(spec.backend, Backend::Cpu | Backend::Metal)
                && spec.identity.is_empty()
                && spec.device.is_none(),
            "fallback or optional runtime"
        );
        let directory = runtime_directory(engine)?;
        ensure!(
            spec.directory.canonicalize()? == directory,
            "external runtime directory"
        );
        runtime.check_current()?;
        ensure!(
            runtime.declared_system_loads(),
            "unobserved native dependencies"
        );
        model.check_current()?;
        let names = members(&directory)?;
        ensure!(
            names.contains(&OsString::from("build-info.json")),
            "missing runtime descriptor"
        );
        let mut observed = Vec::new();
        let mut remaining = MAX_RUNTIME_BYTES;
        for name in names {
            let path = directory.join(&name);
            let limit = fs::symlink_metadata(&path)?.len();
            remaining = remaining
                .checked_sub(limit)
                .context("runtime observation byte budget exceeded")?;
            let receipt = engine
                .inference_fingerprints
                .refresh_bounded(&path, limit)?;
            observed.push((name, receipt.digest().to_owned()));
        }
        let info = descriptor(&directory.join("build-info.json"))?;
        let lock: serde_json::Value =
            serde_json::from_str(include_str!("../../../../models.lock.json"))?;
        ensure!(
            info["format"] == 2
                && info["metal"].is_boolean()
                && info["revision"] == lock["llama_cpp"]["revision"]
                && info["optional_gpu_backends"]
                    .as_array()
                    .is_some_and(Vec::is_empty),
            "unknown bundled runtime descriptor"
        );
        let nominal = match prefs.inference_backend.as_str() {
            "cpu" => Backend::Cpu,
            "metal" if crate::inference::metal_device_available() && info["metal"] == true => {
                Backend::Metal
            }
            "auto" if crate::inference::metal_device_available() && info["metal"] == true => {
                Backend::Metal
            }
            "auto" => Backend::Cpu,
            _ => anyhow::bail!("unsupported backend configuration"),
        };
        ensure!(nominal == spec.backend, "actual backend is a fallback");
        let witness = Self {
            model: catalog.clone(),
            directory: model.directory().to_owned(),
            model_fingerprint: model.fingerprint().into(),
            runtime_directory: directory,
            members: observed,
            configured_backend: prefs.inference_backend.clone(),
            producer: None,
        };
        ensure!(
            witness.current(engine, prefs),
            "execution scope changed during observation"
        );
        Ok(witness)
    }

    pub(crate) fn current(&self, engine: &Engine, prefs: &crate::preferences::Preferences) -> bool {
        (|| -> Result<()> {
            ensure!(
                no_loader_overrides() && self.model.id == prefs.llm_model,
                "configuration changed"
            );
            ensure!(
                self.configured_backend == prefs.inference_backend,
                "backend changed"
            );
            if let Some((path, digest)) = &self.producer {
                let native = official_native_directory(engine)?;
                ensure!(path == &native.join("build-info.json"), "producer moved");
                let receipt = engine
                    .inference_fingerprints
                    .current(path)
                    .context("producer observation unavailable")?;
                ensure!(receipt.digest() == digest, "producer changed");
            }
            ensure!(
                runtime_directory(engine)? == self.runtime_directory,
                "runtime moved"
            );
            ensure!(
                engine.model_directory_for_spec(&self.model)? == self.directory,
                "model selection changed"
            );
            let model = engine
                .model_integrity
                .current(&self.directory, &self.model)?
                .context("model observation unavailable")?;
            ensure!(
                model.fingerprint() == self.model_fingerprint,
                "model changed"
            );
            ensure!(
                members(&self.runtime_directory)?
                    == self
                        .members
                        .iter()
                        .map(|(name, _)| name.clone())
                        .collect::<Vec<_>>(),
                "runtime inventory changed"
            );
            for (name, digest) in &self.members {
                let receipt = engine
                    .inference_fingerprints
                    .current(&self.runtime_directory.join(name))
                    .context("runtime observation unavailable")?;
                ensure!(receipt.digest() == digest, "runtime changed");
            }
            Ok(())
        })()
        .is_ok()
    }
}

fn sealed_path(path: &str) -> bool {
    (path.starts_with("/usr/lib/") || path.starts_with("/System/Library/"))
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}

/// Bounded thin arm64 executable header inspection. No native tools or library
/// loading; the caller validates the held mutation receipt before and afterwards.
pub(crate) fn sealed_macho(input: &mut (impl Read + Seek)) -> Result<()> {
    let mut header = [0u8; 32];
    input.seek(SeekFrom::Start(0))?;
    input.read_exact(&mut header)?;
    let word = |bytes: &[u8]| u32::from_le_bytes(bytes.try_into().unwrap());
    ensure!(
        word(&header[..4]) == 0xfeed_facf
            && word(&header[4..8]) == 0x0100_000c
            && word(&header[12..16]) == 2,
        "not a thin arm64 executable"
    );
    let count = word(&header[16..20]) as usize;
    let size = word(&header[20..24]) as usize;
    ensure!(
        count > 0 && count <= 1024 && size <= MAX_COMMAND_BYTES,
        "load command bound exceeded"
    );
    let mut commands = vec![0; size];
    input.read_exact(&mut commands)?;
    let mut at = 0usize;
    for _ in 0..count {
        let command = commands
            .get(at..at.checked_add(8).context("command overflow")?)
            .context("truncated load command")?;
        let kind = word(&command[..4]);
        let length = word(&command[4..8]) as usize;
        ensure!(
            length >= 8 && length.is_multiple_of(8),
            "malformed load command"
        );
        let command = commands
            .get(at..at.checked_add(length).context("command overflow")?)
            .context("truncated load command")?;
        ensure!(
            [
                0x2,
                0xb,
                0xc,
                0xe,
                0x19,
                0x1b,
                0x1d,
                0x24,
                0x26,
                0x29,
                0x2a,
                0x32,
                0x8000_0018,
                0x8000_001f,
                0x8000_0022,
                0x8000_0023,
                0x20,
                0x8000_0028,
                0x8000_0033,
                0x8000_0034
            ]
            .contains(&kind),
            "unobserved loader command"
        );
        if [0xc, 0x8000_0018, 0x8000_001f, 0x8000_0023, 0x20, 0xe].contains(&kind) {
            let minimum = if kind == 0xe { 12 } else { 24 };
            ensure!(length >= minimum, "short dependency command");
            let offset = word(&command[8..12]) as usize;
            ensure!(offset >= minimum, "overlapping dependency string");
            let tail = command.get(offset..).context("invalid dependency offset")?;
            let end = tail
                .iter()
                .position(|b| *b == 0)
                .context("unterminated dependency string")?;
            let path = std::str::from_utf8(&tail[..end])?;
            ensure!(
                sealed_path(path) && (kind != 0xe || path == "/usr/lib/dyld"),
                "external native dependency"
            );
        }
        at += length;
    }
    ensure!(at == commands.len(), "unclaimed load command bytes");
    Ok(())
}

#[cfg(test)]
pub(crate) fn observation_control(
    engine: &Engine,
    prefs: &crate::preferences::Preferences,
) -> Result<Witness> {
    // Tiny observed bytes exercise receipt/revocation metadata only. They are
    // never executed and do not claim native/model performance or quality.
    use sha2::{Digest, Sha256};
    let mut model = crate::models::model(&prefs.llm_model)?.clone();
    model.files.truncate(1);
    model.files[0].bytes = 4;
    model.files[0].sha256 = format!("{:x}", Sha256::digest(b"good"));
    let directory = engine.root.join("model-store").join(&model.id);
    fs::create_dir_all(&directory)?;
    fs::write(directory.join(&model.files[0].name), b"good")?;
    let model_receipt = engine.model_integrity.verify(&directory, &model)?;
    let runtime = engine.runtime_dir.join("llama");
    fs::create_dir_all(&runtime)?;
    let path = b"/usr/lib/libSystem.B.dylib\0";
    let size = (24 + path.len() + 7) / 8 * 8;
    let mut bytes = vec![0u8; 32 + size];
    for (at, value) in [
        (0, 0xfeed_facfu32),
        (4, 0x0100_000c),
        (12, 2),
        (16, 1),
        (20, size as u32),
        (32, 0xc),
        (36, size as u32),
        (40, 24),
    ] {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[56..56 + path.len()].copy_from_slice(path);
    let executable = runtime.join(crate::platform::executable("llama-server"));
    fs::write(&executable, bytes)?;
    let lock: serde_json::Value =
        serde_json::from_str(include_str!("../../../../models.lock.json"))?;
    fs::write(
        runtime.join("build-info.json"),
        serde_json::to_vec(&serde_json::json!({"format":2,
        "revision":lock["llama_cpp"]["revision"], "optional_gpu_backends":[],"metal":false}))?,
    )?;
    let receipt = engine.inference_fingerprints.refresh(&executable)?;
    Witness::observe_model(
        engine,
        prefs,
        &crate::inference::LaunchSpec {
            backend: Backend::Cpu,
            directory: runtime,
            device: None,
            identity: String::new(),
            selectors: crate::inference::SelectorEnv::capture(Backend::Cpu),
        },
        &model,
        &model_receipt,
        &receipt,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn image(kind: u32, path: &str) -> Vec<u8> {
        let size = (24 + path.len() + 1).div_ceil(8) * 8;
        let mut bytes = vec![0; 32 + size];
        for (at, value) in [
            (0, 0xfeed_facf_u32),
            (4, 0x0100_000c),
            (12, 2),
            (16, 1),
            (20, size as u32),
            (32, kind),
            (36, size as u32),
            (40, 24),
        ] {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[56..56 + path.len()].copy_from_slice(path.as_bytes());
        bytes
    }

    #[test]
    fn declared_system_load_commands_are_bounded_and_external_paths_abstain() {
        for path in [
            "/usr/lib/libSystem.B.dylib",
            "/System/Library/Frameworks/Metal.framework/Versions/A/Metal",
        ] {
            assert!(sealed_macho(&mut Cursor::new(image(0xc, path))).is_ok());
        }
        for path in [
            "@rpath/libggml.dylib",
            "@loader_path/extra.dylib",
            "/usr/local/lib/extra.dylib",
            "/System/Library/../../tmp/extra.dylib",
        ] {
            assert!(sealed_macho(&mut Cursor::new(image(0xc, path))).is_err());
        }
        for kind in [0x8000_001c, 0x27, 0x6, 0x8000_0035, 0x1234] {
            assert!(
                sealed_macho(&mut Cursor::new(image(kind, "/usr/lib/libSystem.B.dylib"))).is_err()
            );
        }
        for (at, value) in [
            (0, 0xcafe_babe),
            (4, 0x0100_0007),
            (16, 2048),
            (20, (MAX_COMMAND_BYTES + 1) as u32),
            (36, 7),
            (40, 4),
        ] {
            let mut bytes = image(0xc, "/usr/lib/libSystem.B.dylib");
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
            assert!(sealed_macho(&mut Cursor::new(bytes)).is_err());
        }
        let mut bytes = image(0xc, "/usr/lib/libSystem.B.dylib");
        bytes.pop();
        assert!(sealed_macho(&mut Cursor::new(bytes)).is_err());
    }

    #[test]
    fn loader_resource_and_generation_overrides_are_unknown() {
        assert!(environment_supported([]));
        for key in [
            "DYLD_INSERT_LIBRARIES",
            "DYLD_FALLBACK_LIBRARY_PATH",
            "GGML_METAL_PATH_RESOURCES",
            "GGML_BACKEND_PATH",
            "LLAMA_ARG_DEVICE",
            "MTL_DEVICE_WRAPPER_TYPE",
            "METAL_DEVICE_WRAPPER_TYPE",
            "LD_PRELOAD",
            "OMASCRIBE_PREPARE_SOURCES",
            "OMASCRIBE_INCREMENTAL_MINUTES",
            "OMASCRIBE_GROUPED_MINUTES",
        ] {
            assert!(!environment_supported([(key.into(), "1".into())]), "{key}");
        }
        assert!(environment_supported([(
            "OMASCRIBE_PREPARE_SOURCES".into(),
            "0".into()
        )]));
    }

    #[test]
    fn non_bundle_runtime_never_becomes_a_producer_scope() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        assert!(official_native_directory(&engine).is_err());
        assert!(descriptor(&root.path().join("missing.json")).is_err());
        let oversized = root.path().join("oversized.json");
        fs::write(&oversized, vec![b' '; 16_385])?;
        assert!(descriptor(&oversized).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn linked_llama_directory_is_rejected_before_cold_observation() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut engine = Engine::open(root.path().into())?;
        std::sync::Arc::get_mut(&mut engine).unwrap().runtime_dir = root.path().join("runtime");
        fs::create_dir_all(engine.runtime_dir.join("llama"))?;
        assert_eq!(
            runtime_directory(&engine)?,
            engine.runtime_dir.join("llama").canonicalize()?
        );
        let external = root.path().join("custom-llama");
        fs::rename(engine.runtime_dir.join("llama"), &external)?;
        std::os::unix::fs::symlink(&external, engine.runtime_dir.join("llama"))?;
        assert!(runtime_directory(&engine).is_err());
        Ok(())
    }
}
