//! Curated, pinned model downloads. Imported profiles can select IDs, never URLs or paths.
use crate::{preferences::Preferences, Engine};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Archive {
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
    pub root: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[serde(default = "default_format")]
    pub format: String,
    pub license: String,
    pub source: String,
    pub memory_mib: u64,
    pub bundled: bool,
    pub files: Vec<Asset>,
    pub archive: Option<Archive>,
}
fn default_format() -> String {
    "gguf".into()
}
#[derive(Deserialize)]
struct Catalog {
    models: Vec<Model>,
}
pub fn catalog() -> &'static [Model] {
    static CATALOG: OnceLock<Vec<Model>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str::<Catalog>(include_str!("../../../model-catalog.json"))
            .expect("embedded catalog")
            .models
    })
}
pub fn model(id: &str) -> Result<&'static Model> {
    catalog()
        .iter()
        .find(|m| m.id == id)
        .context("Unknown model ID")
}
#[derive(Debug, Clone, Serialize, Default)]
pub struct Download {
    pub state: String,
    pub downloaded: u64,
    pub total: u64,
    pub error: Option<String>,
}
#[derive(Default)]
pub struct Downloads {
    pub status: Mutex<HashMap<String, Download>>,
    pub(crate) cancel: Mutex<HashMap<String, Arc<AtomicBool>>>,
}
#[derive(Serialize)]
pub struct ModelStatus {
    pub model: Model,
    pub installed: bool,
    pub installed_bytes: u64,
    pub managed: bool,
    pub selected: bool,
    pub download: Option<Download>,
}
fn complete(path: &Path, spec: &Model) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir())
        && spec.files.iter().all(|a| {
            fs::symlink_metadata(path.join(&a.name))
                .is_ok_and(|m| m.file_type().is_file() && m.len() == a.bytes)
        })
}
impl Engine {
    pub fn recover_downloads(&self) -> Result<()> {
        let store = self.root.join("model-store");
        if let Ok(entries) = fs::read_dir(store) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name
                    .strip_prefix(".download-")
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
                    && entry.file_type()?.is_dir()
                {
                    fs::remove_dir_all(entry.path())?;
                }
            }
        }
        Ok(())
    }
    pub fn cancel_all_downloads(&self) {
        for cancel in self.downloads.cancel.lock().unwrap().values() {
            cancel.store(true, Ordering::SeqCst);
        }
        for cancel in self.component_downloads.cancel.lock().unwrap().values() {
            cancel.store(true, Ordering::SeqCst);
        }
    }
    pub fn model_directory(&self, id: &str) -> Result<PathBuf> {
        let spec = model(id)?;
        let managed = self.root.join("model-store").join(id);
        if complete(&managed, spec) {
            return Ok(managed);
        }
        if spec.bundled {
            let bundled = if spec.kind == "asr" {
                self.models.join("zipformer")
            } else {
                self.models.clone()
            };
            if complete(&bundled, spec) {
                return Ok(bundled);
            }
        }
        bail!("Model not installed: {}", spec.name)
    }
    /// Execution admission only. UI/status callers must keep using metadata-only
    /// resolution; hashing is streamed once per unchanged model in this process.
    pub(crate) fn verified_model(&self, id: &str) -> Result<crate::model_integrity::Receipt> {
        self.verify_model_spec(model(id)?)
    }
    pub fn verify_model_integrity(&self, id: &str) -> Result<serde_json::Value> {
        self.require_available()?;
        let before = self.cached_model(id)?;
        let started = std::time::Instant::now();
        let receipt = self.verified_model(id)?;
        Ok(serde_json::json!({"model":id,"state":"verified",
            "cached":before.as_ref().is_some_and(|old|old.fingerprint()==receipt.fingerprint()),
            "elapsed_ms":started.elapsed().as_millis(),
            "bytes":model(id)?.files.iter().map(|asset|asset.bytes).sum::<u64>(),
            "receipt":receipt.fingerprint()}))
    }
    pub(crate) fn cached_model(&self, id: &str) -> Result<Option<crate::model_integrity::Receipt>> {
        let spec = model(id)?;
        for directory in self.model_candidates(spec) {
            if let Some(receipt) = self.model_integrity.current(&directory, spec)? {
                return Ok(Some(receipt));
            }
        }
        Ok(None)
    }
    fn model_candidates(&self, spec: &Model) -> Vec<PathBuf> {
        let mut directories = vec![self.root.join("model-store").join(&spec.id)];
        if spec.bundled {
            directories.push(if spec.kind == "asr" {
                self.models.join("zipformer")
            } else {
                self.models.clone()
            });
        }
        directories
    }
    fn verify_model_spec(&self, spec: &Model) -> Result<crate::model_integrity::Receipt> {
        let directories = self.model_candidates(spec);
        // A valid bundled receipt can serve the identical pinned model even if
        // a corrupt managed copy exists, without rereading that bad copy per job.
        for directory in &directories {
            if let Some(receipt) = self.model_integrity.current(directory, spec)? {
                return Ok(receipt);
            }
        }
        let mut failure = None;
        for directory in directories {
            if directory.symlink_metadata().is_err() {
                continue;
            }
            match self.model_integrity.verify(&directory, spec) {
                Ok(receipt) => return Ok(receipt),
                Err(error) => failure = Some(error),
            }
        }
        Err(failure.unwrap_or_else(|| anyhow::anyhow!("Model not installed: {}", spec.name)))
    }
    pub(crate) fn verified_asr_model(
        &self,
        id: &str,
    ) -> Result<Vec<crate::model_integrity::Receipt>> {
        let primary = self.verified_model(id)?;
        let mut receipts = vec![primary];
        if id == "zipformer-bilingual" {
            // The native decoder prefers a local BPE file. This model does not
            // ship one: do not let an unlisted file override the pinned tokenizer.
            if receipts[0]
                .directory()
                .join("bpe.model")
                .symlink_metadata()
                .is_ok()
            {
                bail!("model:integrity:unexpected-bilingual-tokenizer");
            }
            let mut tokenizer = model("zipformer-small")?.clone();
            tokenizer
                .files
                .retain(|a| ["tokens.txt", "bpe.model"].contains(&a.name.as_str()));
            if tokenizer.files.len() != 2 {
                bail!("model:integrity:missing-tokenizer-catalog");
            }
            receipts.push(self.verify_model_spec(&tokenizer)?);
        }
        Ok(receipts)
    }
    /// Keep verified model files outside the replaceable application before a runtime-only update.
    pub fn preserve_bundled_models(&self) -> Result<()> {
        let store = self.root.join("model-store");
        fs::create_dir_all(&store)?;
        for spec in catalog().iter().filter(|s| s.bundled) {
            let target = store.join(&spec.id);
            if complete(&target, spec) {
                self.model_integrity.verify(&target, spec)?;
                continue;
            }
            let source = if spec.kind == "asr" {
                self.models.join("zipformer")
            } else {
                self.models.clone()
            };
            if !complete(&source, spec) {
                continue;
            }
            if target.exists() {
                bail!(
                    "Incomplete model copy exists: {}. Remove it in Settings before updating.",
                    spec.name
                );
            }
            let stage = store.join(format!(".download-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&stage)?;
            let result = (|| -> Result<()> {
                for a in &spec.files {
                    let file = source.join(&a.name);
                    if !fs::symlink_metadata(&file)?.file_type().is_file() {
                        bail!("Unexpected model file");
                    }
                    let mut input = File::open(file)?;
                    let mut output = File::create(stage.join(&a.name))?;
                    let mut hash = Sha256::new();
                    let mut buf = [0_u8; 65536];
                    let mut bytes = 0;
                    loop {
                        let n = input.read(&mut buf)?;
                        if n == 0 {
                            break;
                        }
                        hash.update(&buf[..n]);
                        output.write_all(&buf[..n])?;
                        bytes += n as u64;
                    }
                    if bytes != a.bytes || format!("{:x}", hash.finalize()) != a.sha256 {
                        bail!("Model checksum mismatch; application was not replaced");
                    }
                    output.sync_all()?;
                }
                fs::rename(&stage, &target)?;
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_dir_all(&stage);
            }
            result?;
        }
        Ok(())
    }
    pub fn model_file(&self, id: &str) -> Result<PathBuf> {
        Ok(self.model_directory(id)?.join(&model(id)?.files[0].name))
    }
    pub fn model_status(&self) -> Vec<ModelStatus> {
        let prefs = self.preferences.lock().unwrap().clone();
        let progress = self.downloads.status.lock().unwrap();
        catalog()
            .iter()
            .map(|m| ModelStatus {
                model: m.clone(),
                installed: self.model_directory(&m.id).is_ok(),
                installed_bytes: self
                    .model_directory(&m.id)
                    .map(|dir| {
                        m.files
                            .iter()
                            .filter_map(|a| fs::metadata(dir.join(&a.name)).ok())
                            .map(|meta| meta.len())
                            .sum()
                    })
                    .unwrap_or(0),
                managed: complete(&self.root.join("model-store").join(&m.id), m),
                selected: m.id == prefs.asr_model || m.id == prefs.llm_model,
                download: progress.get(&m.id).cloned(),
            })
            .collect()
    }
    pub fn configuration_idle(&self) -> Result<()> {
        let s = self.status.lock().unwrap();
        if s.recording
            || s.paused
            || s.transcribing
            || s.notes_busy
            || s.translation_busy
            || s.diarization_busy
        {
            bail!("Finish recording, transcription and writing before changing models");
        }
        Ok(())
    }
    pub fn update_preferences(&self, prefs: Preferences) -> Result<Preferences> {
        prefs.validate()?;
        let _gate = self.config_lock.lock().unwrap();
        let mut current = self.preferences.lock().unwrap();
        if current.asr_model != prefs.asr_model
            || current.llm_model != prefs.llm_model
            || current.threads != prefs.threads
            || current.context_size != prefs.context_size
            || current.inference_backend != prefs.inference_backend
            || current.resource_mode != prefs.resource_mode
            || current.memory_budget_mib != prefs.memory_budget_mib
            || current.llm_keep_alive_seconds != prefs.llm_keep_alive_seconds
        {
            self.configuration_idle()?;
            self.inference_pool.invalidate();
            if current.llm_model != prefs.llm_model
                || current.threads != prefs.threads
                || current.context_size != prefs.context_size
                || current.inference_backend != prefs.inference_backend
                || current.resource_mode != prefs.resource_mode
                || current.memory_budget_mib != prefs.memory_budget_mib
            {
                crate::acceleration::reject(self, "configuration_changed", false);
            }
        }
        let context_changed = current.use_memory != prefs.use_memory
            || current.use_knowledge != prefs.use_knowledge
            || current.llm_model != prefs.llm_model
            || current.minutes_language != prefs.minutes_language;
        prefs.save(&self.root)?;
        if current.translation_mode != prefs.translation_mode
            && self.status.lock().unwrap().translation_busy
        {
            self.note_cancel.store(true, Ordering::SeqCst);
        }
        *current = prefs.clone();
        drop(current);
        if context_changed {
            self.invalidate_background()?;
        }
        Ok(prefs)
    }
    pub fn download_model(self: &Arc<Self>, id: &str) -> Result<()> {
        let _gate = self.config_lock.lock().unwrap();
        self.require_model_network()?;
        let spec = model(id)?.clone();
        let mut jobs = self.downloads.cancel.lock().unwrap();
        if jobs.contains_key(id) {
            bail!("Model download already running");
        }
        if self.model_directory(id).is_ok() {
            return Ok(());
        }
        let total = spec
            .archive
            .as_ref()
            .map_or_else(|| spec.files.iter().map(|a| a.bytes).sum(), |a| a.bytes);
        let required = total + spec.files.iter().map(|a| a.bytes).sum::<u64>() + 256 * 1024 * 1024;
        if fs2::available_space(&self.root)? < required {
            bail!("Not enough disk space for download and verification");
        }
        let cancel = Arc::new(AtomicBool::new(false));
        jobs.insert(id.into(), cancel.clone());
        self.downloads.status.lock().unwrap().insert(
            id.into(),
            Download {
                state: "downloading".into(),
                total,
                ..Default::default()
            },
        );
        let engine = self.clone();
        std::thread::spawn(move || {
            let stage = engine
                .root
                .join("model-store")
                .join(format!(".download-{}", uuid::Uuid::new_v4()));
            let result = install(&engine, &spec, &stage, &cancel);
            let _ = fs::remove_dir_all(&stage);
            let mut status = engine.downloads.status.lock().unwrap();
            if let Some(progress) = status.get_mut(&spec.id) {
                progress.state = if result.is_ok() {
                    "installed"
                } else if cancel.load(Ordering::SeqCst) {
                    "cancelled"
                } else {
                    "failed"
                }
                .into();
                progress.error = result.err().map(|e| format!("{e:#}"));
            }
            drop(status);
            engine.downloads.cancel.lock().unwrap().remove(&spec.id);
        });
        Ok(())
    }
    pub fn cancel_download(&self, id: &str) -> Result<()> {
        if let Some(cancel) = self.downloads.cancel.lock().unwrap().get(id) {
            cancel.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
    pub fn delete_model(&self, id: &str) -> Result<()> {
        model(id)?;
        let _gate = self.config_lock.lock().unwrap();
        self.configuration_idle()?;
        let p = self.preferences.lock().unwrap();
        if p.asr_model == id || p.llm_model == id {
            bail!("Select another model before deleting this one");
        }
        if self.downloads.cancel.lock().unwrap().contains_key(id) {
            bail!("Cancel the download first");
        }
        let path = self.root.join("model-store").join(id);
        if path.exists() {
            if path.symlink_metadata()?.file_type().is_symlink() {
                bail!("Refusing to delete a linked model directory");
            }
            fs::remove_dir_all(path)?;
        } else {
            bail!(
                "Bundled models belong to the application; only downloaded copies can be deleted"
            );
        }
        self.model_integrity
            .invalidate(&self.root.join("model-store").join(id));
        self.downloads.status.lock().unwrap().remove(id);
        Ok(())
    }
}
fn download(
    engine: &Engine,
    id: &str,
    asset: &Asset,
    target: &Path,
    cancel: &AtomicBool,
) -> Result<()> {
    let baseline = engine
        .downloads
        .status
        .lock()
        .unwrap()
        .get(id)
        .map(|p| p.downloaded)
        .unwrap_or(0);
    for attempt in 0..3 {
        if cancel.load(Ordering::SeqCst) {
            bail!("Download cancelled");
        }
        engine.require_model_network()?;
        if let Some(p) = engine.downloads.status.lock().unwrap().get_mut(id) {
            p.downloaded = baseline;
        }
        match download_once(engine, id, asset, target, cancel) {
            Ok(()) => return Ok(()),
            Err(e)
                if attempt < 2
                    && e.chain().any(|c| {
                        c.downcast_ref::<ureq::Error>()
                            .is_some_and(|e| matches!(e, ureq::Error::Transport(_)))
                            || c.is::<std::io::Error>()
                    }) =>
            {
                // Reopen a fresh verified TLS connection; never weaken certificate or hash checks.
                for _ in 0..(5 * (attempt + 1)) {
                    if cancel.load(Ordering::SeqCst) {
                        bail!("Download cancelled");
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}
fn download_once(
    engine: &Engine,
    id: &str,
    asset: &Asset,
    target: &Path,
    cancel: &AtomicBool,
) -> Result<()> {
    if cancel.load(Ordering::SeqCst) {
        bail!("Download cancelled");
    }
    // A queued worker must recheck the device-local policy before opening HTTP.
    engine.require_model_network()?;
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(30))
        .build();
    if cancel.load(Ordering::SeqCst) {
        bail!("Download cancelled");
    }
    let response = agent
        .get(&asset.url)
        .call()
        .context("Model download failed")?;
    let mut reader = response.into_reader();
    let mut out = File::create(target)?;
    let mut hasher = Sha256::new();
    let mut bytes = 0u64;
    let mut buf = [0u8; 128 * 1024];
    loop {
        if cancel.load(Ordering::SeqCst) {
            bail!("Download cancelled");
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        if bytes > asset.bytes {
            bail!("Downloaded file exceeds catalog size");
        }
        out.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        if let Some(p) = engine.downloads.status.lock().unwrap().get_mut(id) {
            p.downloaded += n as u64;
        }
    }
    out.sync_all()?;
    if bytes != asset.bytes || format!("{:x}", hasher.finalize()) != asset.sha256 {
        bail!("Checksum mismatch; model was not installed");
    }
    Ok(())
}
fn install(engine: &Engine, spec: &Model, stage: &Path, cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) {
        bail!("Download cancelled");
    }
    engine.require_model_network()?;
    fs::create_dir_all(stage)?;
    if let Some(archive) = &spec.archive {
        let path = stage.join("archive.tar.bz2");
        download(
            engine,
            &spec.id,
            &Asset {
                name: String::new(),
                url: archive.url.clone(),
                sha256: archive.sha256.clone(),
                bytes: archive.bytes,
            },
            &path,
            cancel,
        )?;
        if let Some(p) = engine.downloads.status.lock().unwrap().get_mut(&spec.id) {
            p.state = "verifying".into();
        }
        let mut tar = tar::Archive::new(bzip2::read::BzDecoder::new(File::open(&path)?));
        for member in tar.entries()? {
            if cancel.load(Ordering::SeqCst) {
                bail!("Download cancelled");
            }
            let mut member = member?;
            let path = member.path()?.into_owned();
            let Some(a) = spec
                .files
                .iter()
                .find(|a| path == Path::new(&archive.root).join(&a.name))
            else {
                continue;
            };
            if !member.header().entry_type().is_file() || member.size() != a.bytes {
                bail!("Unexpected archive member");
            }
            let mut target = File::create(stage.join(&a.name))?;
            let mut hash = Sha256::new();
            let mut buf = [0u8; 65536];
            loop {
                let n = member.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                target.write_all(&buf[..n])?;
                hash.update(&buf[..n]);
            }
            if format!("{:x}", hash.finalize()) != a.sha256 {
                bail!("Archive model checksum mismatch");
            }
        }
        fs::remove_file(stage.join("archive.tar.bz2"))?;
    } else {
        for asset in &spec.files {
            download(engine, &spec.id, asset, &stage.join(&asset.name), cancel)?;
        }
    }
    if !complete(stage, spec) || cancel.load(Ordering::SeqCst) {
        bail!("Incomplete or cancelled model download");
    }
    let target = engine.root.join("model-store").join(&spec.id);
    if target.exists() {
        bail!("Model directory already exists; remove the incomplete copy before retrying");
    }
    fs::rename(stage, target)?;
    Ok(())
}
#[derive(Serialize)]
pub struct Hardware {
    pub platform: String,
    pub gpus: crate::hardware::GpuInventory,
    pub os: String,
    pub arch: String,
    pub cpu: String,
    pub cores: usize,
    pub total_memory_mib: u64,
    pub available_memory_mib: Option<u64>,
    pub free_disk_bytes: u64,
    pub recommended: Preferences,
    pub tier: String,
    pub apple_silicon: bool,
    pub backends: Vec<String>,
    pub resource_presets: Vec<ResourcePreset>,
}
#[derive(Serialize)]
pub struct ResourcePreset {
    pub id: String,
    pub name: String,
    pub preferences: Preferences,
}
pub fn hardware(root: &Path) -> Hardware {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.refresh_cpu_all();
    let total = sys.total_memory() / 1024 / 1024;
    let cores = sys.cpus().len().max(1);
    let tier = if total >= 8 * 1024 {
        "balanced"
    } else {
        "compact"
    };
    let prefs = preset(tier);
    Hardware {
        platform: std::env::consts::OS.into(),
        gpus: crate::hardware::detect(
            root,
            &crate::platform::asset_dir(root, "runtime", "OMASCRIBE_RUNTIME_DIR"),
        ),
        os: sysinfo::System::long_os_version().unwrap_or_else(|| std::env::consts::OS.into()),
        arch: std::env::consts::ARCH.into(),
        cpu: sys
            .cpus()
            .first()
            .map(|c| c.brand().to_string())
            .unwrap_or_default(),
        cores,
        total_memory_mib: total,
        available_memory_mib: crate::platform::available_memory_mib(),
        free_disk_bytes: fs2::available_space(root).unwrap_or(0),
        recommended: prefs,
        tier: tier.into(),
        apple_silicon: crate::inference::apple_silicon(),
        backends: crate::inference::runtime_backends(&crate::platform::asset_dir(
            root,
            "runtime",
            "OMASCRIBE_RUNTIME_DIR",
        )),
        resource_presets: ["eco", "balanced", "quality"]
            .into_iter()
            .map(|id| ResourcePreset {
                id: id.into(),
                name: id.into(),
                preferences: preset(id),
            })
            .collect(),
    }
}
pub fn preset(tier: &str) -> Preferences {
    let mut p = Preferences::default();
    match tier {
        "compact" | "eco" => {
            p.llm_model = "qwen3-0.6b".into();
            p.resource_mode = "eco".into();
        }
        "quality" => {
            p.llm_model = "qwen3-4b".into();
            p.asr_model = "zipformer-bilingual".into();
            p.threads = 4;
            p.resource_mode = "quality".into();
            p.context_size = 4096;
        }
        _ => p.threads = 2,
    }
    p
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_is_pinned_and_paths_are_basenames() {
        for m in catalog() {
            assert!(m
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.'));
            for f in &m.files {
                assert_eq!(Path::new(&f.name).components().count(), 1);
                assert_eq!(f.sha256.len(), 64);
                assert!(f.bytes > 0);
                assert!(f.url.is_empty() || f.url.starts_with("https://"));
            }
        }
    }
    #[test]
    fn presets_use_supported_model_pairs() {
        for t in ["compact", "balanced", "quality"] {
            preset(t).validate().unwrap();
        }
    }
    #[test]
    fn installation_verifies_bytes_before_exposing_model_and_cancel_keeps_it_hidden() {
        use std::net::TcpListener;
        let root = tempfile::tempdir().unwrap();
        let engine = Engine::open(root.path().into()).unwrap();
        engine
            .set_network_policy(crate::network::NetworkPolicy {
                allow_model_downloads: true,
                allow_online_recommendations: false,
                allow_update_checks: false,
            })
            .unwrap();
        fn fixture(body: &'static [u8]) -> String {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 2048];
                let _ = stream.read(&mut request);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                let _ = stream.write_all(body);
            });
            format!("http://{address}/model")
        }
        let mut spec = Model {
            id: "fixture".into(),
            name: "fixture".into(),
            kind: "llm".into(),
            format: "gguf".into(),
            license: "MIT".into(),
            source: String::new(),
            memory_mib: 1,
            bundled: false,
            archive: None,
            files: vec![Asset {
                name: "test.gguf".into(),
                url: fixture(b"good"),
                sha256: format!("{:x}", Sha256::digest(b"good")),
                bytes: 4,
            }],
        };
        let stage = root.path().join("model-store/.test");
        install(&engine, &spec, &stage, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            fs::read(root.path().join("model-store/fixture/test.gguf")).unwrap(),
            b"good"
        );
        fs::remove_dir_all(root.path().join("model-store/fixture")).unwrap();
        spec.files[0].url = fixture(b"evil");
        assert!(install(&engine, &spec, &stage, &AtomicBool::new(false)).is_err());
        assert!(!root.path().join("model-store/fixture").exists());
        fs::remove_dir_all(&stage).unwrap();
        assert!(install(&engine, &spec, &stage, &AtomicBool::new(true)).is_err());
        assert!(!stage.exists());
        assert!(!root.path().join("model-store/fixture").exists());
        engine
            .set_network_policy(crate::network::NetworkPolicy::default())
            .unwrap();
        assert_eq!(
            install(&engine, &spec, &stage, &AtomicBool::new(false))
                .unwrap_err()
                .to_string(),
            "network:model-downloads-disabled"
        );
        assert!(!stage.exists());
    }
    #[test]
    fn running_jobs_prevent_model_changes_and_selected_model_deletion() {
        let root = tempfile::tempdir().unwrap();
        let e = Engine::open(root.path().into()).unwrap();
        e.status.lock().unwrap().recording = true;
        let mut next = e.preferences.lock().unwrap().clone();
        next.llm_model = "qwen3-0.6b".into();
        assert!(e.update_preferences(next).is_err());
        let mut next = e.preferences.lock().unwrap().clone();
        next.appearance.mode = "light".into();
        assert!(e.update_preferences(next).is_ok());
        e.status.lock().unwrap().recording = false;
        assert!(e.delete_model("qwen3-1.7b").is_err());
        assert!(e.delete_model("../../secret").is_err());
    }
}
