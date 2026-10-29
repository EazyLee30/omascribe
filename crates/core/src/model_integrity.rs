//! Process-local receipts for pinned contents and observable filesystem changes.
//!
//! Cold verification hashes every catalog asset. Cached receipts recheck only
//! filesystem-observable identity and timestamps; they are not secure immutable
//! file-descriptor snapshots. An uncoordinated writable mmap can change bytes
//! without observable stamp changes, notably on macOS, so these checks cannot
//! detect every concurrent writer. Native loaders subsequently reopen paths:
//! callers must check immediately before/after loading and discard a worker on
//! any detected change, without claiming that those native descriptors are pinned.
use crate::models::Model;
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    fs::{self, File, Metadata, OpenOptions},
    hash::{Hash, Hasher},
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};

const READ_BUFFER_BYTES: usize = 64 * 1024;
const MAX_CACHED_RECEIPTS: usize = 32;

/// A distinct error type prevents an integrity failure being retried on CPU.
#[derive(Debug)]
pub(crate) struct IntegrityError(String);

impl fmt::Display for IntegrityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "model:integrity: {}", self.0)
    }
}

impl std::error::Error for IntegrityError {}

fn failure(message: impl Into<String>) -> anyhow::Error {
    IntegrityError(message.into()).into()
}

fn io_failure(action: &str, path: &Path, error: impl fmt::Display) -> anyhow::Error {
    failure(format!("{action} {}: {error}", path.display()))
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Descriptor {
    name: String,
    url: String,
    sha256: String,
    bytes: u64,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CatalogKey {
    id: String,
    kind: String,
    format: String,
    files: Vec<Descriptor>,
}

impl CatalogKey {
    fn new(spec: &Model) -> Result<Self> {
        if spec.files.is_empty() {
            return Err(failure("model catalog contains no assets"));
        }
        let mut names = HashSet::new();
        for asset in &spec.files {
            // Catalog assets are flat files. Reject both platform separators,
            // Windows aliases/streams, and case collisions on every platform.
            let name = &asset.name;
            let mut components = Path::new(name).components();
            let single_name = matches!(components.next(), Some(Component::Normal(_)))
                && components.next().is_none();
            let stem = name
                .split('.')
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase();
            let device_name = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || (stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && matches!(stem.as_bytes()[3], b'1'..=b'9'));
            if !single_name
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
                || name.ends_with(['.', ' '])
                || device_name
                || !names.insert(name.to_ascii_lowercase())
            {
                return Err(failure(format!(
                    "invalid or duplicate catalog asset name: {name:?}"
                )));
            }
            if asset.sha256.len() != 64 || !asset.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(failure(format!("invalid SHA-256 for catalog asset {name}")));
            }
            if asset.bytes == u64::MAX {
                return Err(failure(format!("invalid length for catalog asset {name}")));
            }
        }
        Ok(Self {
            id: spec.id.clone(),
            kind: spec.kind.clone(),
            format: spec.format.clone(),
            files: spec
                .files
                .iter()
                .map(|asset| Descriptor {
                    name: asset.name.clone(),
                    url: asset.url.clone(),
                    sha256: asset.sha256.clone(),
                    bytes: asset.bytes,
                })
                .collect(),
        })
    }
}

#[derive(Clone, Debug)]
struct CacheKey {
    // Keep the exact supplied path; do not merge lexical aliases into evidence.
    directory: PathBuf,
    catalog: CatalogKey,
}

impl PartialEq for CacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.directory.as_os_str() == other.directory.as_os_str() && self.catalog == other.catalog
    }
}

impl Eq for CacheKey {}

impl Hash for CacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.directory.as_os_str().hash(state);
        self.catalog.hash(state);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    directory: Identity,
    files: Vec<Identity>,
}

#[derive(Debug)]
struct Verified {
    key: CacheKey,
    snapshot: Snapshot,
    fingerprint: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Receipt(Arc<Verified>);

impl Receipt {
    pub(crate) fn fingerprint(&self) -> &str {
        &self.0.fingerprint
    }

    pub(crate) fn directory(&self) -> &Path {
        &self.0.key.directory
    }

    pub(crate) fn check_current(&self) -> Result<()> {
        let observed = snapshot(self.directory(), &self.0.key.catalog)?;
        if observed != self.0.snapshot {
            return Err(failure(format!(
                "model files changed after verification: {}",
                self.directory().display()
            )));
        }
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct Cache {
    successes: Mutex<HashMap<CacheKey, Receipt>>,
    in_flight: Mutex<HashMap<CacheKey, Weak<Mutex<()>>>>,
    #[cfg(test)]
    hashed_bytes: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    verification_count: std::sync::atomic::AtomicUsize,
}

impl Cache {
    /// A cold or stale lookup never hashes, and never restores disk evidence.
    pub(crate) fn current(&self, directory: &Path, spec: &Model) -> Result<Option<Receipt>> {
        self.current_key(&CacheKey {
            directory: directory.to_path_buf(),
            catalog: CatalogKey::new(spec)?,
        })
    }

    fn current_key(&self, key: &CacheKey) -> Result<Option<Receipt>> {
        let cached = self
            .successes
            .lock()
            .map_err(|_| failure("verification cache lock poisoned"))?
            .get(key)
            .cloned();
        if let Some(receipt) = cached {
            // No filesystem work (and especially no hashing) holds the mutex.
            if receipt.check_current().is_ok() {
                return Ok(Some(receipt));
            }
            self.invalidate(&key.directory);
        }
        Ok(None)
    }

    pub(crate) fn verify(&self, directory: &Path, spec: &Model) -> Result<Receipt> {
        let key = CacheKey {
            directory: directory.to_path_buf(),
            catalog: CatalogKey::new(spec)?,
        };
        if let Some(receipt) = self.current_key(&key)? {
            return Ok(receipt);
        }
        let gate = {
            let mut in_flight = self
                .in_flight
                .lock()
                .map_err(|_| failure("verification flight map lock poisoned"))?;
            // Retain only gates held by a verifier or its waiting callers.
            // Dead keys do not accumulate across model/catalog changes.
            in_flight.retain(|_, gate| gate.strong_count() > 0);
            if let Some(gate) = in_flight.get(&key).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(Mutex::new(()));
                in_flight.insert(key.clone(), Arc::downgrade(&gate));
                gate
            }
        };
        // Waiting and hashing hold only this exact key's gate. Other models,
        // including ASR during a large language-model hash, remain independent.
        let _flight = gate
            .lock()
            .map_err(|_| failure("model verification flight lock poisoned"))?;
        if let Some(receipt) = self.current_key(&key)? {
            return Ok(receipt);
        }
        #[cfg(test)]
        self.verification_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let before = snapshot(directory, &key.catalog)?;
        for (asset, expected) in key.catalog.files.iter().zip(&before.files) {
            self.verify_asset(directory, asset, expected)?;
        }
        // Recheck the entire set, including secondary files already hashed.
        if snapshot(directory, &key.catalog)? != before {
            return Err(failure("model directory changed during verification"));
        }
        let fingerprint = fingerprint(&key, &before);
        let receipt = Receipt(Arc::new(Verified {
            key: key.clone(),
            snapshot: before,
            fingerprint,
        }));
        let mut successes = self
            .successes
            .lock()
            .map_err(|_| failure("verification cache lock poisoned"))?;
        // Catalog subsets can share a directory (for example an encoder model
        // and its tokenizer). Keep their independent successful receipts so a
        // smaller verification does not evict the full model's evidence.
        if successes.len() >= MAX_CACHED_RECEIPTS && !successes.contains_key(&key) {
            if let Some(evicted) = successes.keys().next().cloned() {
                successes.remove(&evicted);
            }
        }
        successes.insert(key, receipt.clone());
        Ok(receipt)
    }

    pub(crate) fn invalidate(&self, directory: &Path) {
        // Recovering a poisoned lock only removes trust; it cannot add success.
        self.successes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|key, _| key.directory.as_os_str() != directory.as_os_str());
    }

    fn verify_asset(
        &self,
        directory: &Path,
        asset: &Descriptor,
        expected: &Identity,
    ) -> Result<()> {
        let path = directory.join(&asset.name);
        let (mut file, before) = open_checked(&path, false)?;
        if &before != expected || observe(&path, false)? != before {
            return Err(failure(format!(
                "model asset changed before hashing: {}",
                path.display()
            )));
        }
        let digest = hash_stream(&mut file, asset.bytes, |bytes| {
            #[cfg(test)]
            self.hashed_bytes
                .fetch_add(bytes as u64, std::sync::atomic::Ordering::Relaxed);
            #[cfg(not(test))]
            let _ = bytes;
        })
        .map_err(|error| io_failure("cannot hash", &path, error))?;
        if digest != asset.sha256.to_ascii_lowercase() {
            return Err(failure(format!("SHA-256 mismatch: {}", path.display())));
        }
        if identity(&file, &path, false)? != before || observe(&path, false)? != before {
            return Err(failure(format!(
                "model asset changed during hashing: {}",
                path.display()
            )));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn hashed_bytes(&self) -> u64 {
        self.hashed_bytes.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn verification_count(&self) -> usize {
        self.verification_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

fn hash_stream(
    input: &mut impl Read,
    expected_bytes: u64,
    mut on_read: impl FnMut(usize),
) -> std::io::Result<String> {
    let mut buffer = [0_u8; READ_BUFFER_BYTES];
    let mut hash = Sha256::new();
    let mut bytes = 0_u64;
    loop {
        // One byte beyond the catalog length proves excess data. Even a file
        // being continually appended cannot make this loop run unboundedly.
        let remaining = expected_bytes.saturating_sub(bytes).saturating_add(1);
        let limit = remaining.min(READ_BUFFER_BYTES as u64) as usize;
        let read = input.read(&mut buffer[..limit])?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        on_read(read);
        #[cfg(test)]
        run_read_hook();
        if bytes > expected_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "asset exceeds catalog length",
            ));
        }
        hash.update(&buffer[..read]);
    }
    if bytes != expected_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "asset is shorter than catalog length",
        ));
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn snapshot(directory: &Path, catalog: &CatalogKey) -> Result<Snapshot> {
    let (handle, directory_before) = open_checked(directory, true)?;
    if catalog.format == "mlx" {
        check_directory_entries(directory, catalog)?;
    }
    let mut files = Vec::with_capacity(catalog.files.len());
    for asset in &catalog.files {
        let path = directory.join(&asset.name);
        let observed = observe(&path, false)?;
        if observed.bytes() != asset.bytes {
            return Err(failure(format!("size mismatch: {}", path.display())));
        }
        files.push(observed);
    }
    if catalog.format == "mlx" {
        check_directory_entries(directory, catalog)?;
    }
    if identity(&handle, directory, true)? != directory_before
        || observe(directory, true)? != directory_before
    {
        return Err(failure(format!(
            "model directory changed: {}",
            directory.display()
        )));
    }
    Ok(Snapshot {
        directory: directory_before,
        files,
    })
}

fn check_directory_entries(directory: &Path, catalog: &CatalogKey) -> Result<()> {
    let allowed: HashSet<&str> = catalog
        .files
        .iter()
        .map(|asset| asset.name.as_str())
        .collect();
    for entry in
        fs::read_dir(directory).map_err(|error| io_failure("cannot list", directory, error))?
    {
        let entry = entry.map_err(|error| io_failure("cannot list", directory, error))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| io_failure("cannot inspect", &path, error))?;
        validate_kind(&metadata, &path, false)?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| failure("non-UTF-8 entry in MLX model directory"))?;
        // MLX receives a directory and may discover weights, tokenizers, indexes
        // or code by itself. An allowlist covers future loader extensions too.
        if !allowed.contains(name)
            && !matches!(
                name,
                ".DS_Store"
                    | "README"
                    | "README.md"
                    | "LICENSE"
                    | "LICENSE.txt"
                    | ".gitattributes"
                    | ".gitignore"
            )
        {
            return Err(failure(format!(
                "unexpected file in MLX model directory: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn observe(path: &Path, directory: bool) -> Result<Identity> {
    open_checked(path, directory).map(|(_, identity)| identity)
}

fn open_checked(path: &Path, directory: bool) -> Result<(File, Identity)> {
    let before =
        fs::symlink_metadata(path).map_err(|error| io_failure("cannot inspect", path, error))?;
    validate_kind(&before, path, directory)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NONBLOCK prevents a swapped-in FIFO from hanging before fstat.
        let flags =
            libc::O_NOFOLLOW | libc::O_NONBLOCK | if directory { libc::O_DIRECTORY } else { 0 };
        options.custom_flags(flags);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        };
        // A writer can defer ChangeTime/LastWriteTime updates until close.
        // Deny concurrent writers/deletion while observing or hashing a handle;
        // an existing writable handle makes this check fail closed instead.
        options.share_mode(FILE_SHARE_READ.0);
        options.custom_flags(
            FILE_FLAG_OPEN_REPARSE_POINT.0
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS.0
                } else {
                    0
                },
        );
    }
    #[cfg(not(any(unix, windows)))]
    return Err(failure(
        "strong filesystem identity is unsupported on this platform",
    ));
    let file = options
        .open(path)
        .map_err(|error| io_failure("cannot open", path, error))?;
    let observed = identity(&file, path, directory)?;
    #[cfg(unix)]
    if Identity::from_metadata(&before) != observed {
        return Err(failure(format!(
            "model path changed while opening: {}",
            path.display()
        )));
    }
    Ok((file, observed))
}

fn validate_kind(metadata: &Metadata, path: &Path, directory: bool) -> Result<()> {
    let kind = metadata.file_type();
    let valid = !kind.is_symlink()
        && if directory {
            kind.is_dir()
        } else {
            kind.is_file()
        };
    #[cfg(windows)]
    let valid = {
        use std::os::windows::fs::MetadataExt;
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        valid && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0
    };
    if !valid {
        return Err(failure(format!(
            "expected a regular model {} without symlinks: {}",
            if directory { "directory" } else { "file" },
            path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct Identity {
    device: u64,
    inode: u64,
    bytes: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

#[cfg(unix)]
impl Identity {
    fn from_metadata(metadata: &Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            bytes: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }

    fn bytes(&self) -> u64 {
        self.bytes
    }
}

#[cfg(unix)]
fn identity(file: &File, path: &Path, directory: bool) -> Result<Identity> {
    let metadata = file
        .metadata()
        .map_err(|error| io_failure("cannot stat", path, error))?;
    validate_kind(&metadata, path, directory)?;
    Ok(Identity::from_metadata(&metadata))
}

#[cfg(windows)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct Identity {
    volume: u64,
    file_id: [u8; 16],
    bytes: u64,
    modified: i64,
    changed: i64,
}

#[cfg(windows)]
impl Identity {
    fn bytes(&self) -> u64 {
        self.bytes
    }
}

#[cfg(windows)]
fn identity(file: &File, path: &Path, directory: bool) -> Result<Identity> {
    use std::{mem::size_of, os::windows::io::AsRawHandle};
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            FileBasicInfo, FileIdInfo, GetFileInformationByHandleEx, FILE_ATTRIBUTE_REPARSE_POINT,
            FILE_BASIC_INFO, FILE_ID_INFO,
        },
    };
    let metadata = file
        .metadata()
        .map_err(|error| io_failure("cannot stat", path, error))?;
    validate_kind(&metadata, path, directory)?;
    let mut id = FILE_ID_INFO::default();
    let mut basic = FILE_BASIC_INFO::default();
    // FileIdInfo is mandatory: filesystems that cannot supply it fail closed.
    unsafe {
        let handle = HANDLE(file.as_raw_handle());
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&mut id as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
        .map_err(|error| io_failure("cannot obtain stable file ID for", path, error))?;
        GetFileInformationByHandleEx(
            handle,
            FileBasicInfo,
            (&mut basic as *mut FILE_BASIC_INFO).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
        .map_err(|error| io_failure("cannot obtain change time for", path, error))?;
    }
    if basic.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(failure(format!(
            "reparse point in model path: {}",
            path.display()
        )));
    }
    if id.FileId.Identifier == [0; 16] || basic.ChangeTime == 0 {
        return Err(failure(format!(
            "filesystem did not supply a stable file ID and change time: {}",
            path.display()
        )));
    }
    Ok(Identity {
        volume: id.VolumeSerialNumber,
        file_id: id.FileId.Identifier,
        bytes: metadata.len(),
        modified: basic.LastWriteTime,
        changed: basic.ChangeTime,
    })
}

#[cfg(not(any(unix, windows)))]
#[derive(Clone, Debug, Eq, PartialEq)]
struct Identity;

#[cfg(not(any(unix, windows)))]
impl Identity {
    fn bytes(&self) -> u64 {
        0
    }
}

#[cfg(not(any(unix, windows)))]
fn identity(_: &File, _: &Path, _: bool) -> Result<Identity> {
    Err(failure(
        "strong filesystem identity is unsupported on this platform",
    ))
}

fn fingerprint(key: &CacheKey, snapshot: &Snapshot) -> String {
    let mut hash = Sha256::new();
    hash.update(b"omascribe-model-integrity-v1\0");
    // Debug strings are length-framed and contain the exact OS path, every
    // catalog descriptor, and platform identity, rather than file names alone.
    for value in [format!("{key:?}"), format!("{snapshot:?}")] {
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

#[cfg(test)]
thread_local! {
    static READ_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = std::cell::RefCell::new(None);
}

#[cfg(test)]
fn run_read_hook() {
    let hook = READ_HOOK.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Asset;
    use tempfile::TempDir;

    fn fixture(files: &[(&str, &[u8])]) -> (TempDir, PathBuf, Model) {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("model");
        fs::create_dir(&directory).unwrap();
        let files = files
            .iter()
            .map(|(name, bytes)| {
                fs::write(directory.join(name), bytes).unwrap();
                Asset {
                    name: (*name).into(),
                    url: "https://example.invalid/pinned".into(),
                    sha256: format!("{:x}", Sha256::digest(bytes)),
                    bytes: bytes.len() as u64,
                }
            })
            .collect();
        let model = Model {
            id: "fixture".into(),
            name: "fixture".into(),
            kind: "llm".into(),
            format: "gguf".into(),
            license: "test".into(),
            source: "test".into(),
            memory_mib: 1,
            bundled: false,
            files,
            archive: None,
        };
        (temp, directory, model)
    }

    fn assert_integrity(result: Result<Receipt>) {
        let error = result.unwrap_err();
        assert!(
            error.downcast_ref::<IntegrityError>().is_some(),
            "{error:?}"
        );
        assert!(error.to_string().starts_with("model:integrity:"));
    }

    #[test]
    fn cached_receipt_rechecks_without_reading_contents() {
        let (_temp, directory, spec) = fixture(&[("weights.gguf", b"pinned weights")]);
        let cache = Cache::default();
        assert!(cache.current(&directory, &spec).unwrap().is_none());
        let first = cache.verify(&directory, &spec).unwrap();
        let bytes = cache.hashed_bytes();
        let second = cache.verify(&directory, &spec).unwrap();
        assert!(Arc::ptr_eq(&first.0, &second.0));
        assert_eq!(bytes, b"pinned weights".len() as u64);
        assert_eq!(cache.hashed_bytes(), bytes);
        assert_eq!(cache.verification_count(), 1);
        assert_eq!(first.directory(), directory);
        assert_eq!(first.fingerprint(), second.fingerprint());
        first.check_current().unwrap();
        cache.invalidate(&directory);
        assert!(cache.current(&directory, &spec).unwrap().is_none());
    }

    #[test]
    fn concurrent_cold_verification_shares_a_hash_without_blocking_other_models() {
        use std::{
            sync::mpsc,
            thread,
            time::{Duration, Instant},
        };
        let (_temp, directory, spec) = fixture(&[("weights.gguf", b"shared weights")]);
        let (_other_temp, other_directory, other_spec) = fixture(&[("encoder.onnx", b"encoder")]);
        let cache = Arc::new(Cache::default());
        let key = CacheKey {
            directory: directory.clone(),
            catalog: CatalogKey::new(&spec).unwrap(),
        };
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first_cache = Arc::clone(&cache);
        let first_directory = directory.clone();
        let first_spec = spec.clone();
        let first = thread::spawn(move || {
            READ_HOOK.with(|slot| {
                *slot.borrow_mut() = Some(Box::new(move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                }))
            });
            first_cache.verify(&first_directory, &first_spec).unwrap()
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let waiting_cache = Arc::clone(&cache);
        let (waiting_tx, waiting_rx) = mpsc::channel();
        let waiting = thread::spawn(move || {
            waiting_tx
                .send(waiting_cache.verify(&directory, &spec))
                .unwrap();
        });
        // Observe the second caller holding the same gate before unpausing the
        // first, rather than depending on thread scheduling or a timing sleep.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let callers = cache
                .in_flight
                .lock()
                .unwrap()
                .get(&key)
                .unwrap()
                .strong_count();
            if callers >= 2 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "second verifier did not reach the shared flight"
            );
            thread::yield_now();
        }
        let other_cache = Arc::clone(&cache);
        let (other_tx, other_rx) = mpsc::channel();
        let other = thread::spawn(move || {
            other_tx
                .send(other_cache.verify(&other_directory, &other_spec))
                .unwrap();
        });
        // The unrelated model must finish while the first hash stays paused.
        other_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert!(matches!(
            waiting_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        release_tx.send(()).unwrap();
        let second = waiting_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        let first = first.join().unwrap();
        waiting.join().unwrap();
        other.join().unwrap();
        assert!(Arc::ptr_eq(&first.0, &second.0));
        assert_eq!(cache.verification_count(), 2);
        assert_eq!(
            cache.hashed_bytes(),
            (b"shared weights".len() + b"encoder".len()) as u64
        );
    }

    #[test]
    fn same_size_corruption_invalidates_cached_receipt() {
        let (_temp, directory, spec) = fixture(&[("weights.gguf", b"good")]);
        let cache = Cache::default();
        let receipt = cache.verify(&directory, &spec).unwrap();
        fs::write(directory.join("weights.gguf"), b"evil").unwrap();
        assert!(receipt.check_current().is_err());
        assert!(cache.current(&directory, &spec).unwrap().is_none());
        assert_integrity(cache.verify(&directory, &spec));
    }

    #[test]
    fn restored_modified_time_still_invalidates_receipt() {
        let (_temp, directory, spec) = fixture(&[("weights.gguf", b"good")]);
        let cache = Cache::default();
        let receipt = cache.verify(&directory, &spec).unwrap();
        let path = directory.join("weights.gguf");
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        fs::write(&path, b"evil").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        assert!(receipt.check_current().is_err());
        assert_integrity(cache.verify(&directory, &spec));
    }

    #[test]
    fn secondary_asset_change_invalidates_whole_model() {
        let (_temp, directory, spec) =
            fixture(&[("weights.gguf", b"primary"), ("config.json", b"secondary")]);
        let cache = Cache::default();
        let receipt = cache.verify(&directory, &spec).unwrap();
        fs::write(directory.join("config.json"), b"corrupted").unwrap();
        assert!(receipt.check_current().is_err());
        assert_integrity(cache.verify(&directory, &spec));
    }

    #[test]
    fn exact_catalog_descriptors_are_part_of_cache_key() {
        let (_temp, directory, mut spec) = fixture(&[("weights.gguf", b"good")]);
        let cache = Cache::default();
        cache.verify(&directory, &spec).unwrap();
        spec.files[0].url.push_str("?another-revision");
        assert!(cache.current(&directory, &spec).unwrap().is_none());
        cache.verify(&directory, &spec).unwrap();
        assert_eq!(cache.verification_count(), 2);
        spec.files[0].sha256 = "0".repeat(64);
        assert_integrity(cache.verify(&directory, &spec));
    }

    #[test]
    fn subset_verification_preserves_full_model_receipt() {
        let (_temp, directory, full) = fixture(&[
            ("encoder.onnx", b"encoder"),
            ("tokens.txt", b"tokens"),
            ("bpe.model", b"tokenizer"),
        ]);
        let mut subset = full.clone();
        subset.files.remove(0);
        let cache = Cache::default();
        let first = cache.verify(&directory, &full).unwrap();
        cache.verify(&directory, &subset).unwrap();
        let bytes = cache.hashed_bytes();
        let again = cache.verify(&directory, &full).unwrap();
        assert!(Arc::ptr_eq(&first.0, &again.0));
        assert_eq!(cache.hashed_bytes(), bytes);
        assert_eq!(cache.verification_count(), 2);
        assert!(cache.current(&directory, &subset).unwrap().is_some());
        cache.invalidate(&directory);
        assert!(cache.current(&directory, &full).unwrap().is_none());
        assert!(cache.current(&directory, &subset).unwrap().is_none());
    }

    #[test]
    fn cache_successes_are_bounded() {
        let (_temp, directory, mut spec) = fixture(&[("weights.gguf", b"good")]);
        let cache = Cache::default();
        for index in 0..MAX_CACHED_RECEIPTS + 3 {
            spec.id = format!("fixture-{index}");
            cache.verify(&directory, &spec).unwrap();
        }
        assert_eq!(cache.successes.lock().unwrap().len(), MAX_CACHED_RECEIPTS);
        assert!(cache.current(&directory, &spec).unwrap().is_some());
        let flights = cache.in_flight.lock().unwrap();
        assert_eq!(flights.len(), 1);
        assert_eq!(flights.values().next().unwrap().strong_count(), 0);
    }

    #[test]
    fn exact_directory_spelling_is_part_of_cache_key() {
        let (_temp, directory, spec) = fixture(&[("weights.gguf", b"good")]);
        let cache = Cache::default();
        cache.verify(&directory, &spec).unwrap();
        let alias = directory.join(".");
        assert_ne!(directory.as_os_str(), alias.as_os_str());
        assert!(cache.current(&alias, &spec).unwrap().is_none());
        cache.verify(&alias, &spec).unwrap();
        assert_eq!(cache.verification_count(), 2);
    }

    #[cfg(windows)]
    #[test]
    fn existing_writer_prevents_reuse_of_deferred_windows_timestamps() {
        let (_temp, directory, spec) = fixture(&[("weights.gguf", b"good")]);
        let cache = Cache::default();
        let receipt = cache.verify(&directory, &spec).unwrap();
        let writer = File::options()
            .write(true)
            .open(directory.join("weights.gguf"))
            .unwrap();
        assert!(receipt.check_current().is_err());
        assert!(cache.current(&directory, &spec).unwrap().is_none());
        assert_integrity(cache.verify(&directory, &spec));
        drop(writer);
        cache.verify(&directory, &spec).unwrap();
    }

    #[test]
    fn rejects_size_mismatch_and_directory_asset() {
        let (_temp, directory, spec) = fixture(&[("weights.gguf", b"good")]);
        let cache = Cache::default();
        fs::write(directory.join("weights.gguf"), b"too long").unwrap();
        assert_integrity(cache.verify(&directory, &spec));
        fs::remove_file(directory.join("weights.gguf")).unwrap();
        fs::create_dir(directory.join("weights.gguf")).unwrap();
        assert_integrity(cache.verify(&directory, &spec));
        assert_eq!(cache.hashed_bytes(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_file_and_symlink_directory() {
        use std::os::unix::fs::symlink;
        let (temp, directory, spec) = fixture(&[("weights.gguf", b"good")]);
        let path = directory.join("weights.gguf");
        let target = temp.path().join("target");
        fs::rename(&path, &target).unwrap();
        symlink(&target, &path).unwrap();
        let cache = Cache::default();
        assert_integrity(cache.verify(&directory, &spec));
        fs::remove_file(&path).unwrap();
        fs::rename(&target, &path).unwrap();
        let alias = temp.path().join("alias");
        symlink(&directory, &alias).unwrap();
        assert_integrity(cache.verify(&alias, &spec));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_fifo_without_opening_it() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let (_temp, directory, spec) = fixture(&[("weights.gguf", b"good")]);
        let path = directory.join("weights.gguf");
        fs::remove_file(&path).unwrap();
        let name = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert_integrity(Cache::default().verify(&directory, &spec));
    }

    #[test]
    fn rejects_path_replacement_with_matching_contents() {
        let (_temp, directory, spec) = fixture(&[("weights.gguf", b"good")]);
        let cache = Cache::default();
        let receipt = cache.verify(&directory, &spec).unwrap();
        let replacement = directory.join("replacement");
        fs::write(&replacement, b"good").unwrap();
        fs::rename(&replacement, directory.join("weights.gguf")).unwrap();
        assert!(receipt.check_current().is_err());
        let replacement_receipt = cache.verify(&directory, &spec).unwrap();
        assert_ne!(receipt.fingerprint(), replacement_receipt.fingerprint());
    }

    #[cfg(unix)]
    #[test]
    fn detects_path_swap_during_read() {
        let content = vec![42_u8; READ_BUFFER_BYTES * 2];
        let (_temp, directory, spec) = fixture(&[("weights.gguf", &content)]);
        let path = directory.join("weights.gguf");
        let replacement = directory.join("replacement");
        fs::write(&replacement, &content).unwrap();
        READ_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                fs::rename(&replacement, &path).unwrap();
            }))
        });
        assert_integrity(Cache::default().verify(&directory, &spec));
    }

    #[test]
    fn detects_secondary_change_after_it_was_hashed() {
        let (_temp, directory, spec) =
            fixture(&[("config.json", b"good"), ("weights.gguf", b"weights")]);
        let path = directory.join("config.json");
        // The first hook installs a hook that runs while the second file hashes.
        READ_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                READ_HOOK.with(|slot| {
                    *slot.borrow_mut() = Some(Box::new(move || {
                        fs::write(&path, b"evil").unwrap();
                    }))
                });
            }))
        });
        assert_integrity(Cache::default().verify(&directory, &spec));
    }

    #[cfg(unix)]
    #[test]
    fn detects_directory_swap_during_read() {
        let content = vec![42_u8; READ_BUFFER_BYTES * 2];
        let (temp, directory, spec) = fixture(&[("weights.gguf", &content)]);
        let moved = temp.path().join("moved");
        let changed_directory = directory.clone();
        READ_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                fs::rename(&changed_directory, &moved).unwrap();
                fs::create_dir(&changed_directory).unwrap();
                fs::write(changed_directory.join("weights.gguf"), &content).unwrap();
            }))
        });
        assert_integrity(Cache::default().verify(&directory, &spec));
    }

    #[test]
    fn mlx_rejects_unlisted_model_files_and_allows_ds_store() {
        let (_temp, directory, mut spec) =
            fixture(&[("weights.safetensors", b"good"), ("config.json", b"{}")]);
        spec.format = "mlx".into();
        fs::write(directory.join(".DS_Store"), b"finder metadata").unwrap();
        let cache = Cache::default();
        cache.verify(&directory, &spec).unwrap();
        for extra in [
            "other.safetensors",
            "other.bin",
            "other.gguf",
            "other.onnx",
            "other.json",
            "other.model",
            "model.py",
        ] {
            fs::write(directory.join(extra), b"untrusted").unwrap();
            assert!(cache.current(&directory, &spec).unwrap().is_none());
            assert_integrity(cache.verify(&directory, &spec));
            fs::remove_file(directory.join(extra)).unwrap();
        }
    }

    #[test]
    fn rejects_escaping_catalog_names() {
        let (_temp, directory, mut spec) = fixture(&[("weights.gguf", b"good")]);
        for name in [
            "../escape",
            "/absolute",
            "nested/file",
            "nested\\file",
            ".",
            "..",
            "NUL",
            "file:stream",
            "alias.",
            "alias ",
            "",
        ] {
            spec.files[0].name = name.into();
            assert_integrity(Cache::default().verify(&directory, &spec));
        }
    }

    #[test]
    fn hashing_is_bounded_by_catalog_length_and_buffer_size() {
        struct Endless {
            count: usize,
            largest_read: usize,
        }
        impl Read for Endless {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                self.count += output.len();
                self.largest_read = self.largest_read.max(output.len());
                output.fill(42);
                Ok(output.len())
            }
        }
        let mut input = Endless {
            count: 0,
            largest_read: 0,
        };
        let expected = READ_BUFFER_BYTES as u64 * 2 + 5;
        assert!(hash_stream(&mut input, expected, |_| {}).is_err());
        assert_eq!(input.count as u64, expected + 1);
        assert_eq!(input.largest_read, READ_BUFFER_BYTES);
    }
}
