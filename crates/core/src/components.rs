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
const MAX_INDEX: u64 = 512 * 1024;
const MAX_UNPACKED: u64 = 4 * 1024 * 1024 * 1024;

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
    pub installed: bool,
    pub download: Option<Download>,
}
#[derive(Serialize)]
pub struct CatalogStatus {
    pub state: String,
    pub components: Vec<Status>,
    pub verification: crate::acceleration::Status,
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
    if index.schema != 1
        || !index.version.starts_with("acceleration-")
        || !basename(&index.version)
        || index.runtime_revision != pinned_revision()
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
    validate_index(&index)?;
    Ok(index)
}
pub fn cached_index(root: &Path) -> Result<(SignedIndex, Index)> {
    let bytes = read_limited(&root.join("acceleration-index.json"), MAX_INDEX * 2)?;
    let envelope: SignedIndex = serde_json::from_slice(&bytes)?;
    let index = verify_index_with_key(&envelope, &public_key())?;
    Ok((envelope, index))
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
    let (_, index) = if let Some(version) = version {
        let path = root
            .join("accelerators")
            .join(id)
            .join(version)
            .join("component-index.json");
        let envelope: SignedIndex = serde_json::from_slice(&read_limited(&path, MAX_INDEX * 2)?)?;
        let index = verify_index_with_key(&envelope, &public_key())?;
        (envelope, index)
    } else {
        cached_index(root)?
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
        let Ok((_, index)) = cached_index(&self.root) else {
            return CatalogStatus {
                state: "catalog_required".into(),
                components: vec![],
                verification: self.acceleration_status(),
            };
        };
        let downloads = self.component_downloads.status.lock().unwrap();
        CatalogStatus {
            state: "ready".into(),
            verification: self.acceleration_status(),
            components: index
                .components
                .iter()
                .filter(|c| c.target == target())
                .map(|c| Status {
                    component: c.clone(),
                    installed: installed(&self.root, &index, c),
                    download: downloads.get(&c.id).cloned(),
                })
                .collect(),
        }
    }
    /// Explicit user action. Fetches only the signed catalog, never uploads inventory or content.
    pub fn check_acceleration_components(&self) -> Result<CatalogStatus> {
        self.require_model_network()?;
        let response = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(20))
            .build()
            .get(INDEX_URL)
            .call()?;
        let mut bytes = vec![];
        response
            .into_reader()
            .take(MAX_INDEX * 2 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_INDEX * 2 {
            bail!("Component catalog too large");
        }
        let envelope: SignedIndex = serde_json::from_slice(&bytes)?;
        verify_index_with_key(&envelope, &public_key())?;
        let _gate = self.config_lock.lock().unwrap();
        self.require_model_network()?;
        crate::preferences::atomic_json(&self.root.join("acceleration-index.json"), &envelope)?;
        Ok(self.acceleration_components())
    }
    pub fn download_acceleration_component(self: &Arc<Self>, id: &str) -> Result<()> {
        let _gate = self.config_lock.lock().unwrap();
        self.require_available()?;
        self.require_model_network()?;
        let (envelope, index) = cached_index(&self.root)?;
        let component = index
            .components
            .iter()
            .find(|c| c.id == id && c.target == target())
            .context("Unknown component for this platform")?
            .clone();
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
}
