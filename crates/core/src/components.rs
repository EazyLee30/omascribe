//! Optional, signed acceleration components. The base installer never contains these libraries.
use crate::{
    models::{Asset, Download},
    Engine,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

const INDEX_URL: &str =
    "https://github.com/EazyLee30/omascribe/releases/download/acceleration-v1/index.json";
const PREVIEW_VERSION: &str = "acceleration-ci-40367160891";
// The GitHub release allows replacing assets. Pin the reviewed signed payload.
const PREVIEW_PAYLOAD_SHA256: &str =
    "f0f6261191b3f5cc3f720adbc83060ad11fa399eba060db472c52faaaed648d7";
const MAX_INDEX: u64 = 512 * 1024;
const MAX_UNPACKED: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
enum CatalogChannel {
    Stable,
    Candidate(String),
    Preview,
}
impl CatalogChannel {
    fn version(&self) -> Option<&str> {
        match self {
            Self::Stable => None,
            Self::Candidate(version) => Some(version),
            Self::Preview => Some(PREVIEW_VERSION),
        }
    }

    fn parse(candidate: Option<&str>) -> Result<Self> {
        match candidate {
            None => Ok(Self::Stable),
            Some(version)
                if ((version.starts_with("acceleration-candidate-")
                    && version.len() > "acceleration-candidate-".len())
                    || version.strip_prefix("acceleration-ci-").is_some_and(|run| {
                        !run.is_empty() && run.bytes().all(|b| b.is_ascii_digit())
                    }))
                    && version.len() <= 64
                    && basename(version) =>
            {
                Ok(Self::Candidate(version.into()))
            }
            _ => bail!("Invalid acceleration candidate version"),
        }
    }
    fn selected() -> Result<Self> {
        match std::env::var("OMASCRIBE_ACCELERATION_CANDIDATE_VERSION") {
            Ok(version) => Self::parse(Some(&version)),
            Err(std::env::VarError::NotPresent) => Self::parse(None),
            Err(_) => bail!("Invalid acceleration candidate version"),
        }
    }
    fn url(&self) -> String {
        match self.version() {
            None => INDEX_URL.into(),
            Some(version) => format!(
                "https://github.com/EazyLee30/omascribe/releases/download/{version}/index.json"
            ),
        }
    }
    fn cache(&self, root: &Path) -> PathBuf {
        match self.version() {
            None => root.join("acceleration-index.json"),
            Some(version) => root
                .join("acceleration-candidates")
                .join(version)
                .join("index.json"),
        }
    }
    fn verify(&self, envelope: &SignedIndex, key: &str) -> Result<Index> {
        if matches!(self, Self::Preview)
            && format!("{:x}", Sha256::digest(envelope.payload.as_bytes()))
                != PREVIEW_PAYLOAD_SHA256
        {
            bail!("Unexpected built-in preview catalog payload");
        }
        let index = verify_index_with_key(envelope, key)?;
        if self
            .version()
            .is_some_and(|version| index.version != version)
        {
            bail!("Acceleration candidate version mismatch");
        }
        Ok(index)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedIndex {
    pub payload: String,
    pub signature: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Index {
    pub schema: u32,
    pub version: String,
    pub runtime_revision: String,
    pub components: Vec<Component>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    pub id: String,
    pub name: String,
    pub backend: String,
    pub target: String,
    pub entrypoint: String,
    pub license: String,
    pub archive: Asset,
    pub files: Vec<Asset>,
}
#[derive(Serialize)]
pub struct Status {
    pub component: Component,
    pub version: String,
    pub preview: bool,
    pub present: bool,
    pub installed: bool,
    pub downloadable: bool,
    pub compatible: bool,
    pub download: Option<Download>,
}
#[derive(Serialize)]
pub struct CatalogStatus {
    pub state: String,
    pub preview_available: bool,
    pub components: Vec<Status>,
    pub unavailable: Vec<UnavailableComponent>,
    pub verification: crate::acceleration::Status,
}
/// Directory identity for local cleanup only; never an executable component.
#[derive(Serialize)]
pub struct UnavailableComponent {
    pub id: String,
    pub version: String,
    pub reason: String,
    pub busy: bool,
}

pub fn target() -> String {
    let os = std::env::consts::OS;
    format!("{os}-{}", std::env::consts::ARCH)
}
fn basename(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
fn managed_identity(id: &str, version: &str) -> bool {
    [
        "cuda-windows-x86_64",
        "vulkan-windows-x86_64",
        "cuda-linux-x86_64",
        "vulkan-linux-x86_64",
    ]
    .contains(&id)
        && basename(version)
        && version
            .strip_prefix("acceleration-")
            .is_some_and(|suffix| !suffix.is_empty())
}
fn public_key() -> String {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../../../src-tauri/tauri.conf.json"))
            .expect("embedded updater config");
    config["plugins"]["updater"]["pubkey"]
        .as_str()
        .expect("embedded updater public key")
        .into()
}
fn pinned_revision() -> String {
    let lock: serde_json::Value = serde_json::from_str(include_str!("../../../models.lock.json"))
        .expect("embedded runtime lock");
    lock["llama_cpp"]["revision"]
        .as_str()
        .expect("pinned runtime revision")
        .into()
}
fn valid_sha(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn trusted_asset(asset: &Asset, version: &str) -> bool {
    asset.bytes > 0
        && asset.bytes <= MAX_UNPACKED
        && valid_sha(&asset.sha256)
        && basename(&asset.name)
        && asset.url
            == format!(
                "https://github.com/EazyLee30/omascribe/releases/download/{version}/{}",
                asset.name
            )
}
fn validate_index(index: &Index) -> Result<()> {
    validate_inventory(index)?;
    if index.runtime_revision != pinned_revision() {
        bail!("Unsupported acceleration runtime");
    }
    Ok(())
}
fn validate_inventory(index: &Index) -> Result<()> {
    if index.schema != 1
        || !index.version.starts_with("acceleration-")
        || !basename(&index.version)
        || index.components.len() > 24
    {
        bail!("Unsupported acceleration catalog");
    }
    let mut ids = std::collections::HashSet::new();
    for component in &index.components {
        if !basename(&component.id)
            || !ids.insert(&component.id)
            || !["cuda", "vulkan", "hip"].contains(&component.backend.as_str())
            || !["windows-x86_64", "linux-x86_64"].contains(&component.target.as_str())
            || component.entrypoint
                != if component.target.starts_with("windows") {
                    "llama-server.exe"
                } else {
                    "llama-server"
                }
            || component.files.is_empty()
            || component.files.len() > 512
            || component.name.len() > 160
            || component.license.len() > 100
        {
            bail!("Invalid acceleration component");
        }
        if !trusted_asset(&component.archive, &index.version) {
            bail!("Untrusted component archive");
        }
        let mut names = std::collections::HashSet::new();
        let mut bytes = 0u64;
        for file in &component.files {
            if !basename(&file.name)
                || !names.insert(&file.name)
                || !valid_sha(&file.sha256)
                || file.bytes > MAX_UNPACKED
                || !file.url.is_empty()
                || [
                    "installed.json",
                    "verified.json",
                    "active.json",
                    "component-index.json",
                    "payload.tar.bz2",
                ]
                .contains(&file.name.as_str())
            {
                bail!("Invalid component file");
            }
            bytes = bytes
                .checked_add(file.bytes)
                .context("Component size overflow")?;
        }
        if bytes > MAX_UNPACKED
            || !names.contains(&component.entrypoint)
            || !names.contains(&"build-info.json".to_string())
        {
            bail!("Incomplete acceleration component");
        }
    }
    Ok(())
}
fn verify_index_with_key(envelope: &SignedIndex, key: &str) -> Result<Index> {
    let index = verify_inventory_with_key(envelope, key)?;
    validate_index(&index)?;
    Ok(index)
}
// Inventory and explicit removal may retain signed older runtimes. Execution
// and remote catalog admission still require verify_index_with_key above.
fn verify_inventory_with_key(envelope: &SignedIndex, key: &str) -> Result<Index> {
    if envelope.payload.len() as u64 > MAX_INDEX || envelope.signature.len() > 8192 {
        bail!("Component catalog too large");
    }
    let index: Index = serde_json::from_str(&envelope.payload)?;
    crate::update_signature::verify_bytes(
        envelope.payload.as_bytes(),
        &envelope.signature,
        key,
        &index.version,
    )?;
    validate_inventory(&index)?;
    Ok(index)
}
pub fn cached_index(root: &Path) -> Result<(SignedIndex, Index)> {
    cached_index_for(root, &CatalogChannel::selected()?, &public_key())
}
fn cached_index_for(
    root: &Path,
    channel: &CatalogChannel,
    key: &str,
) -> Result<(SignedIndex, Index)> {
    let bytes = read_limited(&channel.cache(root), MAX_INDEX * 2)?;
    let envelope: SignedIndex = serde_json::from_slice(&bytes)?;
    let index = channel.verify(&envelope, key)?;
    Ok((envelope, index))
}

fn offers_preview(channel: &CatalogChannel, host: &str, current: Option<&Index>) -> bool {
    matches!(channel, CatalogChannel::Stable)
        && matches!(host, "windows-x86_64" | "linux-x86_64")
        && !current.is_some_and(|index| index.components.iter().any(|c| c.target == host))
}

fn catalog_for_display(
    root: &Path,
    primary: &CatalogChannel,
    host: &str,
    key: &str,
) -> (Option<Index>, bool) {
    let read = |channel| {
        cached_index_for(root, channel, key)
            .ok()
            .map(|(_, index)| index)
    };
    let current = read(primary);
    if offers_preview(primary, host, current.as_ref()) {
        (read(&CatalogChannel::Preview), true)
    } else {
        (current, false)
    }
}

fn requested_channel(
    root: &Path,
    primary: &CatalogChannel,
    host: &str,
    key: &str,
    preview: bool,
) -> Result<CatalogChannel> {
    if !preview {
        return Ok(primary.clone());
    }
    let current = cached_index_for(root, primary, key)
        .ok()
        .map(|(_, index)| index);
    if !offers_preview(primary, host, current.as_ref()) {
        bail!("Preview components are unavailable; refresh the component list");
    }
    Ok(CatalogChannel::Preview)
}

fn requested_download(
    root: &Path,
    primary: &CatalogChannel,
    host: &str,
    key: &str,
    id: &str,
    version: Option<&str>,
    preview: bool,
) -> Result<(SignedIndex, Index, Component)> {
    if preview && version != Some(PREVIEW_VERSION) {
        bail!("Preview component version changed; refresh the component list");
    }
    let channel = requested_channel(root, primary, host, key, preview)?;
    let (envelope, index) = cached_index_for(root, &channel, key)?;
    if version.is_some_and(|expected| expected != index.version) {
        bail!("Component version changed; refresh the component list");
    }
    let component = index
        .components
        .iter()
        .find(|c| c.id == id && c.target == host)
        .context("Unknown component for this platform")?
        .clone();
    Ok((envelope, index, component))
}
fn read_limited(path: &Path, max: u64) -> Result<Vec<u8>> {
    let mut bytes = vec![];
    File::open(path)?.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        bail!("Component metadata too large");
    }
    Ok(bytes)
}
fn location(root: &Path, index: &Index, component: &Component) -> PathBuf {
    root.join("accelerators")
        .join(&component.id)
        .join(&index.version)
}
pub(crate) fn installed_component(
    root: &Path,
    id: &str,
    version: Option<&str>,
) -> Result<(Index, Component, PathBuf)> {
    if !basename(id) || version.is_some_and(|v| !basename(v)) {
        bail!("Invalid component identity");
    }
    let index = if let Some(version) = version {
        installed_index(root, id, version)?
    } else {
        cached_index(root)?.1
    };
    let component = index
        .components
        .iter()
        .find(|c| c.id == id && c.target == target())
        .context("Component unavailable for this platform")?
        .clone();
    if version.is_some_and(|v| v != index.version) || !installed(root, &index, &component) {
        bail!("Component is not completely installed");
    }
    let path = location(root, &index, &component);
    Ok((index, component, path))
}
// Each installed version retains its signed catalog so a later catalog refresh
// cannot erase its identity or prevent local testing and removal.
fn installed_index(root: &Path, id: &str, version: &str) -> Result<Index> {
    let index = saved_index(root, id, version)?;
    validate_index(&index)?;
    Ok(index)
}
fn saved_index(root: &Path, id: &str, version: &str) -> Result<Index> {
    let path = checked_component_path(root, id, version)?.join("component-index.json");
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.file_type().is_file() || !unlinked(&metadata) {
        bail!("Installed component metadata is not a regular file");
    }
    let envelope: SignedIndex = serde_json::from_slice(&read_limited(&path, MAX_INDEX * 2)?)?;
    let index = verify_inventory_with_key(&envelope, &public_key())?;
    if index.version != version {
        bail!("Installed component version mismatch");
    }
    Ok(index)
}
pub(crate) fn exact_directory(component: &Component, directory: &Path) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type()?.is_file()
            || !(component.files.iter().any(|f| f.name == name)
                || ["installed.json", "component-index.json"].contains(&name.as_str()))
        {
            bail!("Unexpected file in acceleration component directory");
        }
    }
    Ok(())
}
pub(crate) fn verify_component_files(
    component: &Component,
    directory: &Path,
    cancel: &AtomicBool,
) -> Result<()> {
    exact_directory(component, directory)?;
    for file in &component.files {
        let mut input = File::open(directory.join(&file.name))?;
        let mut hash = Sha256::new();
        let mut bytes = 0u64;
        let mut buffer = [0u8; 128 * 1024];
        loop {
            if cancel.load(Ordering::SeqCst) {
                bail!("Component verification cancelled");
            }
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            bytes += n as u64;
            if bytes > file.bytes {
                bail!("Component file size changed");
            }
            hash.update(&buffer[..n]);
        }
        if bytes != file.bytes || format!("{:x}", hash.finalize()) != file.sha256 {
            bail!("Component checksum mismatch");
        }
    }
    Ok(())
}
fn installed(root: &Path, index: &Index, component: &Component) -> bool {
    let path = location(root, index, component);
    let marker = read_limited(&path.join("installed.json"), 4096)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    marker.is_some_and(|m| m["archive_sha256"].as_str() == Some(component.archive.sha256.as_str()))
        && component.files.iter().all(|f| {
            fs::symlink_metadata(path.join(&f.name))
                .is_ok_and(|m| m.file_type().is_file() && m.len() == f.bytes)
        })
}

impl Engine {
    pub fn acceleration_components(&self) -> CatalogStatus {
        let (current, preview_available) = CatalogChannel::selected()
            .map(|primary| catalog_for_display(&self.root, &primary, &target(), &public_key()))
            .unwrap_or_default();
        catalog_status(self, current, preview_available, |id, version| {
            saved_index(&self.root, id, version)
        })
    }
    /// Explicit user action. Fetches only the signed catalog, never uploads inventory or content.
    pub fn check_acceleration_components(&self, preview: bool) -> Result<CatalogStatus> {
        self.require_model_network()?;
        let primary = CatalogChannel::selected()?;
        let host = target();
        let key = public_key();
        let channel = requested_channel(&self.root, &primary, &host, &key, preview)?;
        let response = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(20))
            .build()
            .get(&channel.url())
            .call();
        let response = match response {
            Err(ureq::Error::Status(404, _)) if !preview => {
                let mut status = self.acceleration_components();
                if status.state != "ready" {
                    status.state = "not_published".into();
                }
                return Ok(status);
            }
            other => other?,
        };
        let mut bytes = vec![];
        response
            .into_reader()
            .take(MAX_INDEX * 2 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_INDEX * 2 {
            bail!("Component catalog too large");
        }
        let envelope: SignedIndex = serde_json::from_slice(&bytes)?;
        channel.verify(&envelope, &key)?;
        let _gate = self.config_lock.lock().unwrap();
        self.require_model_network()?;
        if preview {
            // A concurrent stable check may have filled its cache during the request.
            requested_channel(&self.root, &primary, &host, &key, true)?;
        }
        let cache = channel.cache(&self.root);
        fs::create_dir_all(cache.parent().context("Invalid catalog cache")?)?;
        crate::preferences::atomic_json(&cache, &envelope)?;
        drop(_gate);
        Ok(self.acceleration_components())
    }
    pub fn download_acceleration_component(
        self: &Arc<Self>,
        id: &str,
        version: Option<&str>,
        preview: bool,
    ) -> Result<()> {
        let _gate = self.config_lock.lock().unwrap();
        self.require_available()?;
        self.require_model_network()?;
        let (envelope, index, component) = requested_download(
            &self.root,
            &CatalogChannel::selected()?,
            &target(),
            &public_key(),
            id,
            version,
            preview,
        )?;
        if installed(&self.root, &index, &component) {
            return Ok(());
        }
        let required = component.archive.bytes
            + component.files.iter().map(|f| f.bytes).sum::<u64>()
            + 256 * 1024 * 1024;
        if fs2::available_space(&self.root)? < required {
            bail!("Not enough disk space for component verification");
        }
        let mut jobs = self.component_downloads.cancel.lock().unwrap();
        if jobs.contains_key(id) {
            bail!("Component download already running");
        }
        let cancel = Arc::new(AtomicBool::new(false));
        jobs.insert(id.into(), cancel.clone());
        self.component_downloads.status.lock().unwrap().insert(
            id.into(),
            Download {
                state: "downloading".into(),
                total: component.archive.bytes,
                ..Default::default()
            },
        );
        let engine = self.clone();
        std::thread::spawn(move || {
            let stage = engine
                .root
                .join("accelerators")
                .join(format!(".download-{}", uuid::Uuid::new_v4()));
            let result = install(&engine, &envelope, &index, &component, &stage, &cancel);
            let _ = fs::remove_dir_all(stage);
            if let Some(progress) = engine
                .component_downloads
                .status
                .lock()
                .unwrap()
                .get_mut(&component.id)
            {
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
            engine
                .component_downloads
                .cancel
                .lock()
                .unwrap()
                .remove(&component.id);
        });
        Ok(())
    }
    pub fn cancel_acceleration_download(&self, id: &str) {
        if let Some(cancel) = self.component_downloads.cancel.lock().unwrap().get(id) {
            cancel.store(true, Ordering::SeqCst);
        }
    }
    /// Local, explicit removal; a damaged installation need not pass its own
    /// completeness or hash checks before it can be removed and downloaded again.
    pub fn remove_acceleration_component(&self, id: &str, version: &str) -> Result<()> {
        remove_component(self, id, version, || match cached_index(&self.root) {
            Ok((_, index))
                if index.version == version
                    && index
                        .components
                        .iter()
                        .any(|c| c.id == id && c.target == target()) =>
            {
                Ok(index)
            }
            _ => saved_index(&self.root, id, version),
        })
    }
}

fn catalog_status(
    engine: &Engine,
    current: Option<Index>,
    preview_available: bool,
    load_installed: impl Fn(&str, &str) -> Result<Index>,
) -> CatalogStatus {
    // Restore before the download-status lock, matching configuration actions.
    let verification = engine.acceleration_status();
    let downloads = engine.component_downloads.status.lock().unwrap().clone();
    let mut statuses = Vec::new();
    let mut unavailable = Vec::new();
    if let Some(index) = &current {
        for component in index.components.iter().filter(|c| c.target == target()) {
            statuses.push(Status {
                component: component.clone(),
                version: index.version.clone(),
                preview: preview_available,
                present: fs::symlink_metadata(location(&engine.root, index, component)).is_ok(),
                installed: installed(&engine.root, index, component),
                downloadable: true,
                compatible: true,
                download: downloads.get(&component.id).cloned(),
            });
        }
    }
    append_installed_statuses(
        &engine.root,
        &mut statuses,
        &mut unavailable,
        load_installed,
    );
    if !unavailable.is_empty() {
        let busy = engine.require_available().is_err()
            || engine.configuration_idle().is_err()
            || engine.note_busy.load(Ordering::SeqCst)
            || verification.state == "testing"
            || engine.inference_pool.status().leased;
        let installing = engine.component_downloads.cancel.lock().unwrap();
        for entry in &mut unavailable {
            entry.busy = busy || installing.contains_key(&entry.id);
        }
    }
    CatalogStatus {
        state: if current.is_some() || !statuses.is_empty() {
            "ready"
        } else {
            "catalog_required"
        }
        .into(),
        preview_available,
        components: statuses,
        unavailable,
        verification,
    }
}

fn append_installed_statuses(
    root: &Path,
    statuses: &mut Vec<Status>,
    unavailable: &mut Vec<UnavailableComponent>,
    load_installed: impl Fn(&str, &str) -> Result<Index>,
) {
    let directory = root.join("accelerators");
    if !fs::symlink_metadata(&directory).is_ok_and(|m| plain_directory(&m)) {
        return;
    }
    let Ok(components) = fs::read_dir(directory) else {
        return;
    };
    let mut previous = Vec::new();
    for component in components.flatten() {
        let id = component.file_name().to_string_lossy().into_owned();
        if !basename(&id)
            || !fs::symlink_metadata(component.path()).is_ok_and(|m| plain_directory(&m))
        {
            continue;
        }
        let Ok(versions) = fs::read_dir(component.path()) else {
            continue;
        };
        for version in versions.flatten() {
            let name = version.file_name().to_string_lossy().into_owned();
            if !basename(&name)
                || !fs::symlink_metadata(version.path()).is_ok_and(|m| plain_directory(&m))
            {
                continue;
            }
            let current = statuses
                .iter()
                .any(|s| s.component.id == id && s.version == name);
            // Recheck managed current versions before deduplication: a cached
            // catalog cannot make damaged saved metadata healthy or testable.
            let managed = managed_identity(&id, &name);
            if current && !managed {
                continue;
            }
            let loaded = load_installed(&id, &name);
            let identified = loaded.as_ref().is_ok_and(|index| {
                index.version == name && index.components.iter().any(|c| c.id == id)
            });
            if managed && !identified {
                let reason = match fs::symlink_metadata(version.path().join("component-index.json"))
                {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        "metadata_missing"
                    }
                    _ => "metadata_invalid",
                };
                statuses.retain(|s| s.component.id != id || s.version != name);
                unavailable.push(UnavailableComponent {
                    id: id.clone(),
                    version: name,
                    reason: reason.into(),
                    busy: false,
                });
                continue;
            }
            if current {
                continue;
            }
            let Ok(index) = loaded else { continue };
            let Some(component) = index
                .components
                .iter()
                .find(|c| c.id == id && c.target == target())
            else {
                continue;
            };
            if index.version != name {
                continue;
            }
            previous.push(Status {
                component: component.clone(),
                preview: name == PREVIEW_VERSION,
                version: name,
                present: true,
                installed: installed(root, &index, component),
                downloadable: false,
                compatible: index.runtime_revision == pinned_revision(),
                download: None,
            });
        }
    }
    previous.sort_by(|a, b| (&a.component.id, &a.version).cmp(&(&b.component.id, &b.version)));
    unavailable.sort_by(|a, b| (&a.id, &a.version).cmp(&(&b.id, &b.version)));
    statuses.extend(previous);
}

#[cfg(test)]
type BeforeComponentRemove = Option<Box<dyn FnOnce() -> Result<()>>>;
#[cfg(test)]
thread_local! {
    // Deterministic error after the real removal gate has revoked receipts;
    // avoids permission-based deletion failures that root/Windows can bypass.
    static BEFORE_COMPONENT_REMOVE: std::cell::RefCell<BeforeComponentRemove> =
        const { std::cell::RefCell::new(None) };
}

fn remove_component(
    engine: &Engine,
    id: &str,
    version: &str,
    catalog: impl FnOnce() -> Result<Index>,
) -> Result<()> {
    if !basename(id) || !basename(version) {
        bail!("Invalid component identity");
    }
    let _gate = engine.config_lock.lock().unwrap();
    engine.require_available()?;
    engine.configuration_idle()?;
    if engine.note_busy.load(Ordering::SeqCst)
        || engine.acceleration_test_running()
        || engine.inference_pool.status().leased
    {
        bail!("Wait for local inference and component testing to finish");
    }
    // Cancellation only signals the installer. Its job remains registered
    // until extraction/publication and stage cleanup have actually finished.
    if engine
        .component_downloads
        .cancel
        .lock()
        .unwrap()
        .contains_key(id)
    {
        bail!("Cancel the component download and wait for it to finish");
    }
    let path = checked_component_path(&engine.root, id, version)?;
    let authenticated = catalog().and_then(|index| {
        if index.version != version {
            bail!("Component catalog changed; refresh before removing this version");
        }
        index
            .components
            .iter()
            .find(|c| c.id == id && c.target == target())
            .context("Unknown component for this platform")?;
        Ok(())
    });
    if let Err(error) = authenticated {
        // Only the manager's fixed package layout can be cleaned without its
        // signed inventory. Metadata never supplies a deletion path or files.
        if !managed_identity(id, version)
            || !fs::symlink_metadata(&path).is_ok_and(|m| plain_directory(&m))
        {
            return Err(error);
        }
    }
    engine.inference_pool.invalidate();
    let _runtime_removal = engine.inference_fingerprints.block_directory(&path)?;
    #[cfg(test)]
    if let Some(interrupt) = BEFORE_COMPONENT_REMOVE.with(|slot| slot.borrow_mut().take()) {
        interrupt()?;
    }
    crate::acceleration::forget_component(engine, id, version)?;
    if path.exists() {
        fs::remove_dir_all(&path)?;
    }
    engine.component_downloads.status.lock().unwrap().remove(id);
    Ok(())
}

fn checked_component_path(root: &Path, id: &str, version: &str) -> Result<PathBuf> {
    if !basename(id) || !basename(version) {
        bail!("Invalid component identity");
    }
    let path = root.join("accelerators").join(id).join(version);
    for directory in [
        root.join("accelerators"),
        root.join("accelerators").join(id),
        path.clone(),
    ] {
        match fs::symlink_metadata(directory) {
            Ok(metadata) if !plain_directory(&metadata) => {
                bail!("Refusing to remove a linked or non-directory component path")
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(path)
}

fn plain_directory(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_dir() && unlinked(metadata)
}
fn unlinked(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Junctions and other reparse directories must not become delete roots.
        if metadata.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    true
}

fn install(
    engine: &Engine,
    envelope: &SignedIndex,
    index: &Index,
    component: &Component,
    stage: &Path,
    cancel: &AtomicBool,
) -> Result<()> {
    fs::create_dir_all(stage)?;
    let archive = stage.join("payload.tar.bz2");
    engine.require_model_network()?;
    let response = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(30))
        .build()
        .get(&component.archive.url)
        .call()?;
    let mut reader = response.into_reader();
    let mut out = File::create(&archive)?;
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 128 * 1024];
    loop {
        if cancel.load(Ordering::SeqCst) {
            bail!("Component download cancelled");
        }
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        if bytes > component.archive.bytes {
            bail!("Component archive exceeds signed size");
        }
        hash.update(&buffer[..n]);
        out.write_all(&buffer[..n])?;
        if let Some(progress) = engine
            .component_downloads
            .status
            .lock()
            .unwrap()
            .get_mut(&component.id)
        {
            progress.downloaded = bytes;
        }
    }
    out.sync_all()?;
    drop(out);
    if bytes != component.archive.bytes
        || format!("{:x}", hash.finalize()) != component.archive.sha256
    {
        bail!("Component archive checksum mismatch");
    }
    if let Some(progress) = engine
        .component_downloads
        .status
        .lock()
        .unwrap()
        .get_mut(&component.id)
    {
        progress.state = "verifying".into();
    }
    extract(&archive, stage, &component.files, cancel)?;
    let info: serde_json::Value =
        serde_json::from_slice(&read_limited(&stage.join("build-info.json"), 4096)?)?;
    if info["revision"].as_str() != Some(&index.runtime_revision)
        || info["backend"].as_str() != Some(&component.backend)
        || info["target"].as_str() != Some(&component.target)
    {
        bail!("Component runtime metadata mismatch");
    }
    fs::remove_file(archive)?;
    fs::write(
        stage.join("component-index.json"),
        serde_json::to_vec(envelope)?,
    )?;
    fs::write(
        stage.join("installed.json"),
        serde_json::to_vec(&serde_json::json!({"archive_sha256":component.archive.sha256}))?,
    )?;
    if cancel.load(Ordering::SeqCst) {
        bail!("Component download cancelled");
    }
    let destination = location(&engine.root, index, component);
    fs::create_dir_all(
        destination
            .parent()
            .context("Invalid component destination")?,
    )?;
    if destination.exists() {
        bail!("Component directory already exists; remove incomplete component before retrying");
    }
    fs::rename(stage, destination)?;
    Ok(())
}
fn extract(archive: &Path, stage: &Path, files: &[Asset], cancel: &AtomicBool) -> Result<()> {
    let mut tar = tar::Archive::new(bzip2::read::BzDecoder::new(File::open(archive)?));
    let mut seen = std::collections::HashSet::new();
    for entry in tar.entries()? {
        if cancel.load(Ordering::SeqCst) {
            bail!("Component download cancelled");
        }
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let spec = files
            .iter()
            .find(|f| path == Path::new(&f.name))
            .context("Unexpected component archive member")?;
        if !entry.header().entry_type().is_file()
            || !seen.insert(&spec.name)
            || entry.size() != spec.bytes
        {
            bail!("Invalid component archive member");
        }
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(stage.join(&spec.name))?;
        let mut hash = Sha256::new();
        let mut copied = 0;
        let mut buffer = [0u8; 128 * 1024];
        loop {
            if cancel.load(Ordering::SeqCst) {
                bail!("Component download cancelled");
            }
            let n = entry.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            copied += n as u64;
            if copied > spec.bytes {
                bail!("Component file exceeds signed size");
            }
            hash.update(&buffer[..n]);
            out.write_all(&buffer[..n])?;
        }
        if copied != spec.bytes || format!("{:x}", hash.finalize()) != spec.sha256 {
            bail!("Component file checksum mismatch");
        }
        out.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(stage.join(&spec.name), fs::Permissions::from_mode(0o755))?;
        }
    }
    if seen.len() != files.len() {
        bail!("Component archive is incomplete");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn removal_fixture() -> Result<(tempfile::TempDir, Arc<Engine>, Index, PathBuf)> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let component = Component {
            id: "fixture-gpu".into(),
            name: "Fictional GPU component".into(),
            backend: "vulkan".into(),
            target: target(),
            entrypoint: "llama-server".into(),
            license: "fixture".into(),
            archive: file("archive.tar.bz2", b"archive"),
            files: vec![file("llama-server", b"good")],
        };
        let index = Index {
            schema: 1,
            version: "acceleration-fixture-1".into(),
            runtime_revision: pinned_revision(),
            components: vec![component],
        };
        let path = location(root.path(), &index, &index.components[0]);
        Ok((root, engine, index, path))
    }
    fn fixture_marker(path: &Path, index: &Index) -> Result<()> {
        fs::write(
            path.join("installed.json"),
            serde_json::to_vec(&serde_json::json!({
                "archive_sha256":index.components[0].archive.sha256
            }))?,
        )?;
        Ok(())
    }
    fn fixture_certificate(index: &Index) -> Result<Vec<u8>> {
        let trial = serde_json::json!({"startup_ms":1,"requests_ms":[1],"total_ms":2,
            "peak_rss_mib":null,"runtime_gpu_buffer_mib":null});
        Ok(serde_json::to_vec(&serde_json::json!({
            "schema":1,"component_id":index.components[0].id,"version":index.version,
            "device":"fixture-device","binding":"fixture-binding",
            "report":{"cpu":trial,"gpu":trial,"faster":true}
        }))?)
    }
    fn damaged_catalog_fixture() -> Result<(SignedIndex, Index)> {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/component-signature");
        let key = fs::read_to_string(fixture.join("public.key"))?;
        let mut envelope = SignedIndex {
            payload: fs::read_to_string(fixture.join("payload"))?,
            signature: fs::read_to_string(fixture.join("payload.sig"))?,
        };
        let index = verify_index_with_key(&envelope, &key)?;
        // Preserve valid JSON while invalidating the genuine fixture signature.
        // The fixture key is never admitted by the application's public readers.
        envelope.payload.push('\n');
        assert!(verify_inventory_with_key(&envelope, &key).is_err());
        Ok((envelope, index))
    }
    fn unavailable_identity_count(view: &serde_json::Value, id: &str, version: &str) -> usize {
        view["unavailable"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter(|entry| entry["id"] == id && entry["version"] == version)
                    .count()
            })
            .unwrap_or(0)
    }

    #[test]
    fn public_inventory_and_removal_recover_managed_versions_without_valid_metadata() -> Result<()>
    {
        let mut failures = Vec::new();
        for damaged in [false, true] {
            let (root, engine, mut index, _) = removal_fixture()?;
            let (envelope, signed_index) = damaged_catalog_fixture()?;
            // This is a real managed package identity even on a host where no
            // optional GPU runtime can execute. Cleanup must not execute it.
            index.components[0].id = "vulkan-linux-x86_64".into();
            index.version = signed_index.version;
            let component = &index.components[0];
            let path = location(root.path(), &index, component);
            fs::create_dir_all(&path)?;
            fs::write(path.join("llama-server"), b"good")?;
            fixture_marker(&path, &index)?;
            if damaged {
                fs::write(
                    path.join("component-index.json"),
                    serde_json::to_vec(&envelope)?,
                )?;
            }
            let other = path.parent().unwrap().join("acceleration-keep");
            fs::create_dir(&other)?;
            fs::write(other.join("keep"), b"other version")?;
            assert!(
                installed_component(&engine.root, &component.id, Some(&index.version)).is_err()
            );

            // Exercise both public management entry points before reporting a
            // RED, so a listing failure does not hide the independent delete gap.
            let listed = serde_json::to_value(engine.acceleration_components())?;
            let removal = engine.remove_acceleration_component(&component.id, &index.version);
            assert_eq!(fs::read(other.join("keep"))?, b"other version");
            let metadata = if damaged { "damaged" } else { "missing" };
            if unavailable_identity_count(&listed, &component.id, &index.version) != 1 {
                failures.push(format!(
                    "{metadata} metadata: public inventory must expose one unavailable management entry: {listed}"
                ));
            }
            if removal.is_err() || path.exists() {
                failures.push(format!(
                    "{metadata} metadata: public removal must clean this exact managed version: {removal:?}"
                ));
            }
            let remaining = serde_json::to_value(engine.acceleration_components())?;
            if unavailable_identity_count(&remaining, &component.id, &index.version) != 0 {
                failures.push(format!(
                    "{metadata} metadata: removed version must leave the management inventory"
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        Ok(())
    }

    #[test]
    fn current_catalog_does_not_promote_an_install_with_damaged_metadata() -> Result<()> {
        let (root, engine, mut current, _) = removal_fixture()?;
        let (envelope, signed_index) = damaged_catalog_fixture()?;
        current.components[0].id = "vulkan-linux-x86_64".into();
        current.version = signed_index.version;
        let component = &current.components[0];
        let path = location(root.path(), &current, component);
        fs::create_dir_all(&path)?;
        fs::write(path.join("llama-server"), b"good")?;
        fixture_marker(&path, &current)?;
        fs::write(
            path.join("component-index.json"),
            serde_json::to_vec(&envelope)?,
        )?;
        assert!(installed_component(&engine.root, &component.id, Some(&current.version)).is_err());
        // As in the retained-component controls, the host-target catalog is
        // synthetic. This exercises the production view builder and real saved
        // reader, not production signature admission or GPU execution.
        let listed = serde_json::to_value(catalog_status(
            &engine,
            Some(current.clone()),
            false,
            |id, version| saved_index(&engine.root, id, version),
        ))?;
        let promoted = listed["components"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| {
                entry["component"]["id"] == component.id
                    && entry["version"] == current.version
                    && (entry["installed"] == true || entry["downloadable"] == true)
            });
        assert!(
            !promoted && unavailable_identity_count(&listed, &component.id, &current.version) == 1,
            "A matching current catalog must not hide damaged local metadata behind installed/testable/recommended flags: {listed}"
        );
        Ok(())
    }

    #[test]
    fn unavailable_cleanup_preserves_job_gates_and_managed_directory_scope() -> Result<()> {
        let (root, engine, mut index, _) = removal_fixture()?;
        index.components[0].id = "vulkan-linux-x86_64".into();
        let id = &index.components[0].id;
        let version = &index.version;
        let path = location(root.path(), &index, &index.components[0]);
        fs::create_dir_all(&path)?;
        let runtime = path.join("keep");
        fs::write(&runtime, b"target")?;
        let unknown = root.path().join("accelerators/unknown").join(version);
        let unmanaged = path.parent().unwrap().join("manual");
        let other = path.parent().unwrap().join("acceleration-other");
        for directory in [&unknown, &unmanaged, &other] {
            fs::create_dir_all(directory)?;
            fs::write(directory.join("keep"), b"retain")?;
        }
        let receipt = engine.inference_fingerprints.refresh(&runtime)?;
        let held_clone = receipt.clone();
        engine.note_busy.store(true, Ordering::SeqCst);
        let listed = engine.acceleration_components();
        assert!(listed
            .unavailable
            .iter()
            .any(|entry| entry.id == *id && entry.version == *version && entry.busy));
        let certificate = root.path().join("accelerators/active.json");
        let certificate_bytes = fixture_certificate(&index)?;
        fs::write(&certificate, &certificate_bytes)?;
        assert!(engine.remove_acceleration_component(id, version).is_err());
        engine.note_busy.store(false, Ordering::SeqCst);
        engine
            .component_downloads
            .cancel
            .lock()
            .unwrap()
            .insert(id.clone(), Arc::new(AtomicBool::new(true)));
        assert!(engine.remove_acceleration_component(id, version).is_err());
        engine.component_downloads.cancel.lock().unwrap().remove(id);
        for (candidate_id, candidate_version) in [
            ("unknown", version.as_str()),
            (id.as_str(), "manual"),
            ("../escape", version.as_str()),
            (id.as_str(), "../escape"),
        ] {
            assert!(engine
                .remove_acceleration_component(candidate_id, candidate_version)
                .is_err());
        }
        assert_eq!(fs::read(&certificate)?, certificate_bytes);
        receipt.check_current()?;
        held_clone.check_current()?;
        engine.remove_acceleration_component(id, version)?;
        assert!(!path.exists());
        assert!(!certificate.exists());
        assert!(receipt.check_current().is_err());
        assert!(held_clone.check_current().is_err());
        for directory in [&unknown, &unmanaged, &other] {
            assert_eq!(fs::read(directory.join("keep"))?, b"retain");
        }
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn unavailable_cleanup_refuses_linked_roots_and_does_not_read_linked_metadata() -> Result<()> {
        use std::os::unix::fs::symlink;
        for parent in 0..3 {
            let (root, engine, mut index, _) = removal_fixture()?;
            index.components[0].id = "vulkan-linux-x86_64".into();
            let path = location(root.path(), &index, &index.components[0]);
            let outside = tempfile::tempdir()?;
            fs::write(outside.path().join("keep"), b"external")?;
            let linked = match parent {
                0 => root.path().join("accelerators"),
                1 => path.parent().unwrap().to_path_buf(),
                _ => path,
            };
            fs::create_dir_all(linked.parent().unwrap())?;
            symlink(outside.path(), &linked)?;
            assert!(engine
                .remove_acceleration_component(&index.components[0].id, &index.version)
                .is_err());
            assert!(engine.acceleration_components().unavailable.is_empty());
            assert_eq!(fs::read(outside.path().join("keep"))?, b"external");
        }
        let (root, engine, mut index, _) = removal_fixture()?;
        index.components[0].id = "vulkan-linux-x86_64".into();
        let id = &index.components[0].id;
        let path = location(root.path(), &index, &index.components[0]);
        fs::create_dir_all(&path)?;
        let outside = tempfile::tempdir()?;
        let metadata = outside.path().join("keep");
        fs::write(&metadata, b"external")?;
        symlink(&metadata, path.join("component-index.json"))?;
        let error = saved_index(root.path(), id, &index.version).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Installed component metadata is not a regular file"
        );
        let listed = engine.acceleration_components();
        assert!(listed.unavailable.iter().any(|entry| {
            entry.id == *id && entry.version == index.version && entry.reason == "metadata_invalid"
        }));
        engine.remove_acceleration_component(id, &index.version)?;
        assert!(!path.exists());
        assert_eq!(fs::read(metadata)?, b"external");
        Ok(())
    }
    #[test]
    fn catalog_refresh_keeps_active_download_on_admitted_version_without_files() -> Result<()> {
        let (_root, engine, old, old_path) = removal_fixture()?;
        let id = &old.components[0].id;
        engine
            .component_downloads
            .cancel
            .lock()
            .unwrap()
            .insert(id.clone(), Arc::new(AtomicBool::new(false)));
        engine.component_downloads.status.lock().unwrap().insert(
            id.clone(),
            Download {
                state: "downloading".into(),
                downloaded: 4,
                total: old.components[0].archive.bytes,
                error: None,
            },
        );
        let read_installed = |_: &str, _: &str| bail!("No installed component");
        let before = catalog_status(&engine, Some(old.clone()), true, read_installed);
        assert_eq!(before.components[0].version, old.version);
        assert_eq!(
            before.components[0].download.as_ref().unwrap().downloaded,
            4
        );
        assert!(!old_path.exists());

        let mut current = old.clone();
        current.version = "acceleration-stable-fixture".into();
        let refreshed = catalog_status(&engine, Some(current.clone()), false, read_installed);
        let stable = refreshed
            .components
            .iter()
            .find(|c| c.version == current.version)
            .unwrap();
        assert!(
            stable.download.is_none(),
            "Progress cannot move to a different version"
        );
        let admitted = refreshed
            .components
            .iter()
            .find(|c| c.version == old.version)
            .unwrap();
        assert!(admitted.preview && !admitted.present && !admitted.downloadable);
        assert_eq!(admitted.download.as_ref().unwrap().downloaded, 4);

        let offline = catalog_status(&engine, None, false, read_installed);
        assert_eq!(offline.state, "ready");
        assert_eq!(offline.components.len(), 1);
        assert_eq!(offline.components[0].version, old.version);
        assert!(offline.components[0].download.is_some());
        Ok(())
    }

    #[test]
    fn catalog_refresh_keeps_prior_installs_visible_and_removable() -> Result<()> {
        for damaged in [false, true] {
            let (_root, engine, old, old_path) = removal_fixture()?;
            fs::create_dir_all(&old_path)?;
            fixture_marker(&old_path, &old)?;
            fs::write(
                old_path.join("llama-server"),
                if damaged {
                    b"bad".as_slice()
                } else {
                    b"good".as_slice()
                },
            )?;
            // The production reader verifies the saved signed envelope. This
            // fixture substitutes only that reader to support the host target.
            fs::write(
                old_path.join("component-index.json"),
                serde_json::to_vec(&old)?,
            )?;
            let mut current = old.clone();
            current.version = "acceleration-fixture-2".into();
            let current_path = location(&engine.root, &current, &current.components[0]);
            fs::create_dir_all(&current_path)?;
            fixture_marker(&current_path, &current)?;
            fs::write(current_path.join("llama-server"), b"good")?;
            engine.component_downloads.status.lock().unwrap().insert(
                old.components[0].id.clone(),
                Download {
                    state: "installed".into(),
                    ..Default::default()
                },
            );
            let read_old = |id: &str, version: &str| -> Result<Index> {
                Ok(serde_json::from_slice(&fs::read(
                    engine
                        .root
                        .join("accelerators")
                        .join(id)
                        .join(version)
                        .join("component-index.json"),
                )?)?)
            };
            let listed = catalog_status(&engine, Some(current.clone()), false, read_old);
            assert_eq!(listed.state, "ready");
            assert_eq!(listed.components.len(), 2);
            let new_status = &listed.components[0];
            assert_eq!(new_status.version, current.version);
            assert!(new_status.downloadable);
            assert!(new_status.compatible);
            assert!(new_status.installed);
            assert!(new_status.download.is_some());
            let old_status = &listed.components[1];
            assert_eq!(old_status.version, old.version);
            assert!(old_status.present);
            assert_eq!(old_status.installed, !damaged);
            assert!(!old_status.downloadable);
            assert!(old_status.compatible);
            assert!(old_status.download.is_none());
            let receipt = engine
                .inference_fingerprints
                .refresh(&old_path.join("llama-server"))?;
            remove_component(
                &engine,
                &old_status.component.id,
                &old_status.version,
                || read_old(&old_status.component.id, &old_status.version),
            )?;
            assert!(!old_path.exists());
            assert!(receipt.check_current().is_err());
            assert_eq!(fs::read(current_path.join("llama-server"))?, b"good");
            let remaining = catalog_status(&engine, Some(current), false, read_old);
            assert_eq!(remaining.components.len(), 1);
        }
        Ok(())
    }

    #[test]
    fn missing_catalog_retains_local_installs_without_allowing_downloads() -> Result<()> {
        let (_root, engine, old, path) = removal_fixture()?;
        fs::create_dir_all(&path)?;
        fs::write(path.join("llama-server"), b"good")?;
        fixture_marker(&path, &old)?;
        let status = catalog_status(&engine, None, false, |_, _| Ok(old.clone()));
        assert_eq!(status.state, "ready");
        assert_eq!(status.components.len(), 1);
        assert!(status.components[0].installed);
        assert!(!status.components[0].downloadable);
        assert!(status.components[0].download.is_none());
        // A copied catalog for a different version cannot make a directory
        // visible or authorize its removal.
        let mut wrong = old.clone();
        wrong.version = "acceleration-wrong".into();
        let rejected = catalog_status(&engine, None, false, |_, _| Ok(wrong.clone()));
        assert_eq!(rejected.state, "catalog_required");
        assert!(rejected.components.is_empty());
        // The public reader still rejects unsigned metadata and cannot turn
        // an arbitrary local directory into a downloadable/usable component.
        fs::write(path.join("component-index.json"), serde_json::to_vec(&old)?)?;
        assert!(engine.acceleration_components().components.is_empty());
        assert!(engine
            .remove_acceleration_component(&old.components[0].id, &old.version)
            .is_err());
        assert!(path.exists());
        Ok(())
    }

    #[test]
    fn retired_runtime_remains_removable_but_cannot_be_enabled() -> Result<()> {
        let (_root, engine, mut old, path) = removal_fixture()?;
        old.runtime_revision = "retired-runtime".into();
        fs::create_dir_all(&path)?;
        fs::write(path.join("llama-server"), b"good")?;
        fixture_marker(&path, &old)?;
        // Start from a genuinely verified structurally valid catalog, then
        // change only its runtime pin to exercise inventory/execute policy.
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/component-signature");
        let key = fs::read_to_string(fixture.join("public.key"))?;
        let envelope = SignedIndex {
            payload: fs::read_to_string(fixture.join("payload"))?,
            signature: fs::read_to_string(fixture.join("payload.sig"))?,
        };
        let mut retired = verify_inventory_with_key(&envelope, &key)?;
        retired.runtime_revision = "retired-runtime".into();
        validate_inventory(&retired)?;
        assert!(validate_index(&retired).is_err());
        // Host-target listing/removal uses the synthetic component reader, as
        // in the other removal fixtures. Its bytes are never executable.
        let catalog = catalog_status(&engine, None, false, |_, _| Ok(old.clone()));
        assert_eq!(catalog.components.len(), 1);
        let status = &catalog.components[0];
        assert!(status.installed);
        assert!(!status.compatible);
        assert!(!status.downloadable);
        remove_component(&engine, &status.component.id, &status.version, || {
            Ok(old.clone())
        })?;
        assert!(!path.exists());
        Ok(())
    }

    #[test]
    fn removal_releases_cloned_runtime_receipts_and_preserves_other_directories() -> Result<()> {
        let (_root, engine, index, path) = removal_fixture()?;
        fs::create_dir_all(&path)?;
        fixture_marker(&path, &index)?;
        let runtime = path.join("llama-server");
        fs::write(&runtime, b"good")?;
        let receipt = engine.inference_fingerprints.refresh(&runtime)?;
        let held_clone = receipt.clone();
        assert_eq!(receipt.digest(), format!("{:x}", Sha256::digest(b"good")));
        receipt.check_current()?;
        held_clone.check_current()?;
        assert!(engine.inference_fingerprints.current(&runtime).is_some());

        let other = path.parent().unwrap().join("acceleration-other");
        fs::create_dir(&other)?;
        let other_runtime = other.join("llama-server");
        fs::write(&other_runtime, b"keep")?;
        let other_receipt = engine.inference_fingerprints.refresh(&other_runtime)?;
        let other_clone = other_receipt.clone();
        let component = &index.components[0];
        // No runtime is executed. On Windows the still-live receipt clones
        // would otherwise retain deny-delete handles and block this removal.
        remove_component(&engine, &component.id, &index.version, || Ok(index.clone()))?;
        assert!(!path.exists());
        assert!(receipt.check_current().is_err());
        assert!(held_clone.check_current().is_err());
        assert!(engine.inference_fingerprints.current(&runtime).is_none());
        other_receipt.check_current()?;
        other_clone.check_current()?;
        assert!(engine
            .inference_fingerprints
            .current(&other_runtime)
            .is_some());
        assert_eq!(fs::read(&other_runtime)?, b"keep");

        // Successful removal must not leave the version permanently blocked.
        // A new installation needs a new receipt, never the revoked old clone.
        fs::create_dir(&path)?;
        fs::write(&runtime, b"good")?;
        fixture_marker(&path, &index)?;
        let replacement = engine.inference_fingerprints.refresh(&runtime)?;
        replacement.check_current()?;
        assert!(receipt.check_current().is_err());
        remove_component(&engine, &component.id, &index.version, || Ok(index.clone()))?;
        assert!(replacement.check_current().is_err());
        other_receipt.check_current()?;
        Ok(())
    }

    #[test]
    fn failed_removal_unblocks_fresh_runtime_observation_but_not_old_receipts() -> Result<()> {
        let (_root, engine, index, path) = removal_fixture()?;
        fs::create_dir_all(&path)?;
        let runtime = path.join("llama-server");
        fs::write(&runtime, b"good")?;
        fixture_marker(&path, &index)?;
        let receipt = engine.inference_fingerprints.refresh(&runtime)?;
        let held_clone = receipt.clone();
        let component = &index.components[0];
        let certificate = engine.root.join("accelerators/active.json");
        let certificate_bytes = fixture_certificate(&index)?;
        fs::write(&certificate, &certificate_bytes)?;
        engine.component_downloads.status.lock().unwrap().insert(
            component.id.clone(),
            Download {
                state: "failed".into(),
                ..Default::default()
            },
        );
        BEFORE_COMPONENT_REMOVE.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(|| {
                Err(anyhow::anyhow!("fixture component removal interrupted"))
            }));
        });
        let error = remove_component(&engine, &component.id, &index.version, || Ok(index.clone()))
            .expect_err("the one-shot fixture interrupts after revocation");
        assert!(format!("{error:#}").contains("fixture component removal interrupted"));
        assert_eq!(fs::read(&runtime)?, b"good");
        assert_eq!(fs::read(&certificate)?, certificate_bytes);
        assert!(engine
            .component_downloads
            .status
            .lock()
            .unwrap()
            .contains_key(&component.id));
        assert!(receipt.check_current().is_err());
        assert!(held_clone.check_current().is_err());
        assert!(engine.inference_fingerprints.current(&runtime).is_none());
        // The error return dropped its directory guard. A complete fresh
        // observation may publish again; old cloned authority stays revoked.
        let fresh = engine.inference_fingerprints.refresh(&runtime)?;
        fresh.check_current()?;
        assert!(receipt.check_current().is_err());
        remove_component(&engine, &component.id, &index.version, || Ok(index.clone()))?;
        assert!(!path.exists());
        assert!(!certificate.exists());
        assert!(fresh.check_current().is_err());
        assert!(!engine
            .component_downloads
            .status
            .lock()
            .unwrap()
            .contains_key(&component.id));
        Ok(())
    }

    #[test]
    fn removal_revokes_evicted_runtime_owners_before_filesystem_deletion() -> Result<()> {
        let (_root, engine, index, path) = removal_fixture()?;
        fs::create_dir_all(&path)?;
        let runtime = path.join("llama-server");
        fs::write(&runtime, b"good")?;
        fixture_marker(&path, &index)?;
        let pressure = engine.root.join("runtime-cache-pressure");
        fs::create_dir(&pressure)?;
        let others: Vec<_> = (0..16)
            .map(|i| pressure.join(format!("runtime-{i}")))
            .collect();
        // Finish fixture mutations before observing any parent directory.
        for other in &others {
            fs::write(other, b"warm")?;
        }
        let receipt = engine.inference_fingerprints.refresh(&runtime)?;
        let held_clone = receipt.clone();
        // Seventeen distinct files exceed the existing sixteen-entry warm
        // cache. Eviction must not destroy a live owner's authority by itself.
        for other in &others {
            engine.inference_fingerprints.refresh(other)?;
        }
        assert!(engine.inference_fingerprints.current(&runtime).is_none());
        receipt.check_current()?;
        held_clone.check_current()?;
        BEFORE_COMPONENT_REMOVE.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(|| {
                Err(anyhow::anyhow!(
                    "fixture removal interrupted after cache eviction"
                ))
            }));
        });
        let component = &index.components[0];
        let error = remove_component(&engine, &component.id, &index.version, || Ok(index.clone()))
            .expect_err("the interruption keeps the original runtime file present");
        assert!(format!("{error:#}").contains("fixture removal interrupted after cache eviction"));
        assert_eq!(fs::read(&runtime)?, b"good");
        // File disappearance cannot explain this rejection: only explicit
        // directory revocation can invalidate the already-evicted live clone.
        assert!(receipt.check_current().is_err());
        assert!(held_clone.check_current().is_err());
        assert!(engine.inference_fingerprints.current(&runtime).is_none());
        let fresh = engine.inference_fingerprints.refresh(&runtime)?;
        fresh.check_current()?;
        remove_component(&engine, &component.id, &index.version, || Ok(index.clone()))?;
        assert!(!path.exists());
        assert!(fresh.check_current().is_err());
        Ok(())
    }

    #[test]
    fn damaged_and_partial_versions_can_be_removed_before_verified_reinstallation() -> Result<()> {
        for damage in [
            "same_size",
            "wrong_size",
            "missing_file",
            "bad_marker",
            "missing_marker",
        ] {
            let (root, engine, index, path) = removal_fixture()?;
            fs::create_dir_all(&path)?;
            fixture_marker(&path, &index)?;
            if damage != "missing_file" {
                fs::write(path.join("llama-server"), b"evil")?;
            }
            if damage == "bad_marker" {
                fs::write(path.join("installed.json"), b"invalid")?;
            } else if damage == "missing_marker" {
                fs::remove_file(path.join("installed.json"))?;
            } else if damage == "wrong_size" {
                fs::write(path.join("llama-server"), b"bad")?;
            }
            let certificate = root.path().join("accelerators/active.json");
            fs::write(&certificate, fixture_certificate(&index)?)?;
            let component = &index.components[0];
            assert_eq!(
                installed(root.path(), &index, component),
                damage == "same_size"
            );
            assert!(verify_component_files(component, &path, &AtomicBool::new(false)).is_err());
            let other = path.parent().unwrap().join("acceleration-older");
            fs::create_dir(&other)?;
            fs::write(other.join("keep"), b"other version")?;
            engine.component_downloads.status.lock().unwrap().insert(
                component.id.clone(),
                Download {
                    state: "failed".into(),
                    ..Default::default()
                },
            );
            // Production supplies this index only through the signed loader;
            // a pure fixture exercises removal without downloading/executing it.
            remove_component(&engine, &component.id, &index.version, || Ok(index.clone()))?;
            assert!(!path.exists());
            assert!(!certificate.exists());
            assert!(other.join("keep").exists());
            assert!(!engine
                .component_downloads
                .status
                .lock()
                .unwrap()
                .contains_key(&component.id));
            // Missing-directory removal also revokes a matching stale opt-in;
            // a repeated removal remains harmless.
            fs::write(&certificate, fixture_certificate(&index)?)?;
            remove_component(&engine, &component.id, &index.version, || Ok(index.clone()))?;
            assert!(!certificate.exists());
            remove_component(&engine, &component.id, &index.version, || Ok(index.clone()))?;
            fs::create_dir(&path)?;
            fs::write(path.join("llama-server"), b"good")?;
            fixture_marker(&path, &index)?;
            verify_component_files(component, &path, &AtomicBool::new(false))?;
            assert!(installed(root.path(), &index, component));
            assert!(
                !root.path().join("accelerators/active.json").exists(),
                "Reinstallation alone must not authorize acceleration"
            );
        }
        Ok(())
    }
    #[test]
    fn removal_rejects_busy_jobs_cancelled_installers_and_stale_identity() -> Result<()> {
        for busy in [
            "recording",
            "paused",
            "transcribing",
            "notes",
            "translation",
            "diarization",
            "note_busy",
            "maintenance",
            "download",
            "cancelled_download",
        ] {
            let (_root, engine, index, path) = removal_fixture()?;
            fs::create_dir_all(&path)?;
            fs::write(path.join("keep"), b"not removed")?;
            let runtime = path.join("keep");
            let receipt = engine.inference_fingerprints.refresh(&runtime)?;
            let held_clone = receipt.clone();
            let certificate = engine.root.join("accelerators/active.json");
            let original_certificate = fixture_certificate(&index)?;
            fs::write(&certificate, &original_certificate)?;
            {
                let mut status = engine.status.lock().unwrap();
                match busy {
                    "recording" => status.recording = true,
                    "paused" => status.paused = true,
                    "transcribing" => status.transcribing = true,
                    "notes" => status.notes_busy = true,
                    "translation" => status.translation_busy = true,
                    "diarization" => status.diarization_busy = true,
                    "note_busy" => engine.note_busy.store(true, Ordering::SeqCst),
                    "maintenance" => engine.maintenance.store(true, Ordering::SeqCst),
                    _ => {
                        engine.component_downloads.cancel.lock().unwrap().insert(
                            index.components[0].id.clone(),
                            Arc::new(AtomicBool::new(busy == "cancelled_download")),
                        );
                    }
                }
            }
            assert!(
                remove_component(&engine, &index.components[0].id, &index.version, || Ok(
                    index.clone()
                ))
                .is_err(),
                "{busy}"
            );
            assert!(path.join("keep").exists(), "{busy}");
            assert_eq!(fs::read(certificate)?, original_certificate, "{busy}");
            receipt.check_current()?;
            held_clone.check_current()?;
            assert!(engine.inference_fingerprints.current(&runtime).is_some());
        }
        let (_root, engine, index, path) = removal_fixture()?;
        fs::create_dir_all(&path)?;
        let runtime = path.join("keep");
        fs::write(&runtime, b"not removed")?;
        let receipt = engine.inference_fingerprints.refresh(&runtime)?;
        for (id, version) in [
            ("fixture-gpu", "acceleration-stale"),
            ("unknown", index.version.as_str()),
            ("../escape", index.version.as_str()),
            ("fixture-gpu", "../escape"),
        ] {
            assert!(remove_component(&engine, id, version, || Ok(index.clone())).is_err());
            assert!(path.exists());
            receipt.check_current()?;
            assert!(engine.inference_fingerprints.current(&runtime).is_some());
        }
        // The public entry point never trusts an unsigned cached catalog.
        fs::write(engine.root.join("acceleration-index.json"), b"{}")?;
        assert!(engine
            .remove_acceleration_component("fixture-gpu", &index.version)
            .is_err());
        assert!(path.exists());
        receipt.check_current()?;
        assert!(engine.inference_fingerprints.current(&runtime).is_some());
        Ok(())
    }
    #[test]
    #[cfg(unix)]
    fn removal_refuses_linked_parents_and_unlinks_only_owned_child_links() -> Result<()> {
        use std::os::unix::fs::symlink;
        for parent in 0..3 {
            let (_root, engine, index, path) = removal_fixture()?;
            let outside = tempfile::tempdir()?;
            fs::write(outside.path().join("keep"), b"external")?;
            let linked = match parent {
                0 => engine.root.join("accelerators"),
                1 => path.parent().unwrap().to_path_buf(),
                _ => path.clone(),
            };
            fs::create_dir_all(linked.parent().unwrap())?;
            symlink(outside.path(), &linked)?;
            assert!(
                remove_component(&engine, &index.components[0].id, &index.version, || Ok(
                    index.clone()
                ))
                .is_err()
            );
            assert_eq!(fs::read(outside.path().join("keep"))?, b"external");
        }
        let (_root, engine, index, path) = removal_fixture()?;
        let outside = tempfile::tempdir()?;
        fs::write(outside.path().join("keep"), b"external")?;
        fs::create_dir_all(&path)?;
        symlink(outside.path(), path.join("child-link"))?;
        remove_component(&engine, &index.components[0].id, &index.version, || {
            Ok(index.clone())
        })?;
        assert!(!path.exists());
        assert!(outside.path().join("keep").exists());
        Ok(())
    }
    #[test]
    #[cfg(windows)]
    fn removal_refuses_windows_junction_roots() -> Result<()> {
        for parent in 0..3 {
            let (_root, engine, index, path) = removal_fixture()?;
            let outside = tempfile::tempdir()?;
            fs::write(outside.path().join("keep"), b"external")?;
            let linked = match parent {
                0 => engine.root.join("accelerators"),
                1 => path.parent().unwrap().to_path_buf(),
                _ => path.clone(),
            };
            fs::create_dir_all(linked.parent().unwrap())?;
            let made = crate::platform::child_command("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&linked)
                .arg(outside.path())
                .output()?;
            assert!(
                made.status.success(),
                "Could not create fixture junction: {made:?}"
            );
            assert!(!plain_directory(&fs::symlink_metadata(&linked)?));
            assert!(
                remove_component(&engine, &index.components[0].id, &index.version, || {
                    Ok(index.clone())
                })
                .is_err()
            );
            assert_eq!(fs::read(outside.path().join("keep"))?, b"external");
            fs::remove_dir(linked)?;
        }
        Ok(())
    }
    fn file(name: &str, bytes: &[u8]) -> Asset {
        Asset {
            name: name.into(),
            url: String::new(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
            bytes: bytes.len() as u64,
        }
    }
    #[test]
    fn archive_rejects_links_unknown_paths_missing_files_and_wrong_bytes() -> Result<()> {
        let root = tempfile::tempdir()?;
        let make = |name: &str, data: &[u8], link: bool| -> Result<PathBuf> {
            let path = root
                .path()
                .join(format!("{}.tar.bz2", uuid::Uuid::new_v4()));
            let encoder =
                bzip2::write::BzEncoder::new(File::create(&path)?, bzip2::Compression::fast());
            let mut tar = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o755);
            header.set_size(data.len() as u64);
            if link {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_link_name("target")?;
            }
            header.set_cksum();
            tar.append_data(&mut header, name, data)?;
            tar.into_inner()?.finish()?;
            Ok(path)
        };
        for (name, data, link) in [
            ("llama-server", b"wrong".as_slice(), false),
            ("unknown", b"ok".as_slice(), false),
            ("llama-server", b"ok".as_slice(), true),
        ] {
            let stage = root.path().join(uuid::Uuid::new_v4().to_string());
            fs::create_dir(&stage)?;
            assert!(extract(
                &make(name, data, link)?,
                &stage,
                &[file("llama-server", b"ok")],
                &AtomicBool::new(false)
            )
            .is_err());
        }
        let stage = root.path().join("valid");
        fs::create_dir(&stage)?;
        extract(
            &make("llama-server", b"ok", false)?,
            &stage,
            &[file("llama-server", b"ok")],
            &AtomicBool::new(false),
        )?;
        assert_eq!(fs::read(stage.join("llama-server"))?, b"ok");
        assert!(!basename("../escape"));
        assert!(!basename("C:\\bad.dll"));
        Ok(())
    }
    #[test]
    fn unsigned_or_tampered_catalog_cannot_be_installed() {
        let envelope = SignedIndex {
            payload: "{}".into(),
            signature: "invalid".into(),
        };
        assert!(verify_index_with_key(&envelope, &public_key()).is_err());
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("acceleration-index.json"),
            serde_json::to_vec(&envelope).unwrap(),
        )
        .unwrap();
        assert!(cached_index(root.path()).is_err());
    }
    #[test]
    fn signed_catalog_binds_payload_version_and_runtime_revision() -> Result<()> {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/component-signature");
        let key = fs::read_to_string(root.join("public.key"))?;
        let mut envelope = SignedIndex {
            payload: fs::read_to_string(root.join("payload"))?,
            signature: fs::read_to_string(root.join("payload.sig"))?,
        };
        assert_eq!(
            verify_index_with_key(&envelope, &key)?.version,
            "acceleration-fixture-1"
        );
        envelope.payload = envelope
            .payload
            .replace("acceleration-fixture-1", "acceleration-fixture-2");
        assert!(verify_index_with_key(&envelope, &key).is_err());
        let mut index = Index {
            schema: 1,
            version: "acceleration-fixture-1".into(),
            runtime_revision: "wrong".into(),
            components: vec![],
        };
        assert!(validate_index(&index).is_err());
        index.runtime_revision = pinned_revision();
        validate_index(&index)?;
        Ok(())
    }
    #[test]
    fn candidate_catalog_is_fixed_origin_version_bound_and_separate_from_stable() -> Result<()> {
        let root = tempfile::tempdir()?;
        let stable = CatalogChannel::parse(None)?;
        let candidate = CatalogChannel::parse(Some("acceleration-candidate-123"))?;
        assert!(CatalogChannel::parse(Some("acceleration-ci-40367160891")).is_ok());
        assert!(CatalogChannel::parse(Some("acceleration-ci-not-a-run")).is_err());
        assert_eq!(stable.url(), INDEX_URL);
        assert_eq!(candidate.url(), "https://github.com/EazyLee30/omascribe/releases/download/acceleration-candidate-123/index.json");
        assert_ne!(stable.cache(root.path()), candidate.cache(root.path()));
        for value in [
            "",
            "acceleration-v1",
            "acceleration-fixture-1",
            "acceleration-candidate-",
            "../escape",
            "https://example.com/index.json",
            "acceleration-candidate-../escape",
            "acceleration-candidate-test?redirect=1",
        ] {
            assert!(CatalogChannel::parse(Some(value)).is_err(), "{value}");
        }
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/component-signature");
        let key = fs::read_to_string(fixture.join("public.key"))?;
        let envelope = SignedIndex {
            payload: fs::read_to_string(fixture.join("payload"))?,
            signature: fs::read_to_string(fixture.join("payload.sig"))?,
        };
        crate::preferences::atomic_json(&stable.cache(root.path()), &envelope)?;
        assert!(cached_index_for(root.path(), &stable, &key).is_ok());
        assert!(
            cached_index_for(root.path(), &candidate, &key).is_err(),
            "Never reuse the stable cache for candidate acceptance"
        );
        fs::create_dir_all(candidate.cache(root.path()).parent().unwrap())?;
        crate::preferences::atomic_json(&candidate.cache(root.path()), &envelope)?;
        assert!(
            candidate
                .verify(&envelope, &key)
                .unwrap_err()
                .to_string()
                .contains("version mismatch"),
            "A genuine signature for another release is insufficient"
        );
        assert!(cached_index_for(root.path(), &candidate, &key).is_err());
        assert!(cached_index_for(root.path(), &stable, &key).is_ok());
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/candidate-signature");
        let candidate_key = fs::read_to_string(fixture.join("public.key"))?;
        let mut candidate_envelope = SignedIndex {
            payload: fs::read_to_string(fixture.join("payload"))?,
            signature: fs::read_to_string(fixture.join("payload.sig"))?,
        };
        let fixture_channel = CatalogChannel::parse(Some("acceleration-candidate-fixture-1"))?;
        fs::create_dir_all(fixture_channel.cache(root.path()).parent().unwrap())?;
        crate::preferences::atomic_json(&fixture_channel.cache(root.path()), &candidate_envelope)?;
        assert!(cached_index_for(root.path(), &fixture_channel, &candidate_key).is_ok());
        assert!(
            cached_index_for(root.path(), &fixture_channel, &public_key()).is_err(),
            "Test key must never be trusted by the application"
        );
        candidate_envelope.payload = candidate_envelope.payload.replace("fixture-1", "fixture-2");
        assert!(fixture_channel
            .verify(&candidate_envelope, &candidate_key)
            .is_err());
        let mut index = Index {
            schema: 1,
            version: "acceleration-candidate-123".into(),
            runtime_revision: pinned_revision(),
            components: vec![],
        };
        validate_index(&index)?;
        index.runtime_revision = "untrusted runtime".into();
        assert!(validate_index(&index).is_err());
        Ok(())
    }

    #[test]
    fn signed_preview_is_discovered_locally_without_changing_the_default_request() -> Result<()> {
        let root = tempfile::tempdir()?;
        let host = "linux-x86_64";
        let stable = CatalogChannel::Stable;
        let key = public_key();
        let (current, offer) = catalog_for_display(root.path(), &stable, host, &key);
        assert!(current.is_none() && offer);
        assert_eq!(
            requested_channel(root.path(), &stable, host, &key, false)?.url(),
            INDEX_URL,
        );
        let preview = requested_channel(root.path(), &stable, host, &key, true)?;
        assert_eq!(
            preview.url(),
            format!("https://github.com/EazyLee30/omascribe/releases/download/{PREVIEW_VERSION}/index.json")
        );

        // The real published envelope exercises the release key and runtime pin
        // without a network request or any runtime archives during tests.
        let envelope = include_bytes!("../../../tests/fixtures/acceleration-preview-index.json");
        let path = preview.cache(root.path());
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(&path, envelope)?;
        let (current, offer) = catalog_for_display(root.path(), &stable, host, &key);
        assert!(offer);
        assert_eq!(current.unwrap().version, PREVIEW_VERSION);

        let override_channel = CatalogChannel::parse(Some("acceleration-candidate-fixture-1"))?;
        let (current, offer) = catalog_for_display(root.path(), &override_channel, host, &key);
        assert!(current.is_none() && !offer);
        assert!(requested_channel(root.path(), &override_channel, host, &key, true).is_err());
        let (current, offer) = catalog_for_display(root.path(), &stable, "windows-aarch64", &key);
        assert!(current.is_none() && !offer);
        assert!(requested_channel(root.path(), &stable, "windows-aarch64", &key, true).is_err());

        let mut bad: SignedIndex = serde_json::from_slice(envelope)?;
        bad.payload.push(' ');
        assert_eq!(
            preview.verify(&bad, &key).unwrap_err().to_string(),
            "Unexpected built-in preview catalog payload",
        );
        crate::preferences::atomic_json(&path, &bad)?;
        let (current, offer) = catalog_for_display(root.path(), &stable, host, &key);
        assert!(current.is_none() && offer);
        Ok(())
    }

    #[test]
    fn signed_download_selection_rejects_stale_version_and_preview_after_stable_arrives(
    ) -> Result<()> {
        let root = tempfile::tempdir()?;
        let host = "windows-x86_64";
        let stable = CatalogChannel::Stable;
        let key = public_key();
        let id = "cuda-windows-x86_64";
        let select = |version, preview| {
            requested_download(root.path(), &stable, host, &key, id, version, preview)
        };
        let envelope = include_bytes!("../../../tests/fixtures/acceleration-preview-index.json");
        let preview_path = CatalogChannel::Preview.cache(root.path());
        fs::create_dir_all(preview_path.parent().unwrap())?;
        fs::write(preview_path, envelope)?;
        assert!(
            select(None, false).is_err(),
            "Unversioned stable calls cannot pick a preview"
        );
        assert!(select(None, true).is_err());
        assert!(select(Some("acceleration-old"), true).is_err());
        let (_, index, component) = select(Some(PREVIEW_VERSION), true)?;
        assert_eq!(index.version, PREVIEW_VERSION);
        assert_eq!(component.id, id);

        // Stable promotion can point to the same immutable signed payload.
        // A stale preview card must still fail rather than switch channels.
        fs::write(stable.cache(root.path()), envelope)?;
        let (current, offer) = catalog_for_display(root.path(), &stable, host, &key);
        assert_eq!(current.unwrap().version, PREVIEW_VERSION);
        assert!(!offer);
        assert!(select(Some(PREVIEW_VERSION), true).is_err());
        assert!(select(Some("acceleration-old"), false).is_err());
        assert!(select(Some(PREVIEW_VERSION), false).is_ok());
        assert!(select(None, false).is_ok());
        Ok(())
    }
}
