//! Bounded observations of explicitly selected Vulkan ICD manifests.
//!
//! This does not discover default ICDs or prove which library a loader opened.
//! Manifest bytes are hashed on each snapshot. Libraries use observable file
//! identity, not a full content hash: mmap or unobservable timestamp-quantum
//! writes remain outside this certificate input.
use anyhow::{ensure, Result};
use serde::{de::IgnoredAny, Deserialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

const MAX_PATHS: usize = 16;
const MAX_MANIFESTS: usize = 64;
const MAX_DIRECTORY_ENTRIES: usize = 128;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_SELECTOR_BYTES: usize = 32 * 1024;

#[derive(Deserialize)]
struct Manifest {
    #[serde(rename = "ICD")]
    icd: Driver,
    #[serde(flatten)]
    other: BTreeMap<String, IgnoredAny>,
}

#[derive(Deserialize)]
struct Driver {
    library_path: String,
    #[serde(flatten)]
    other: BTreeMap<String, IgnoredAny>,
}

fn no_native_shadow(fields: &BTreeMap<String, IgnoredAny>, name: &str) -> bool {
    // cJSON's object lookup is ASCII case-insensitive and uses C strings.
    // A differently spelled/NUL-suffixed duplicate must not select another
    // library than the exact fields accepted by serde's typed deserializer.
    !fields.keys().any(|key| {
        key.split('\0')
            .next()
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(name))
    })
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) enum DriverFiles {
    /// Existing system discovery remains unchanged and is not verified here.
    #[default]
    DefaultDiscovery,
    Verified(String),
    Unresolved,
}

impl DriverFiles {
    pub(super) fn capture(keys: &[&str], values: &[Option<OsString>]) -> Self {
        // Khronos precedence is presence-based, including empty values.
        let selected = ["VK_DRIVER_FILES", "VK_ICD_FILENAMES", "VK_ADD_DRIVER_FILES"]
            .iter()
            .find_map(|selected| {
                keys.iter()
                    .position(|key| key == selected)
                    .and_then(|i| values[i].as_ref())
            });
        match selected {
            None => Self::DefaultDiscovery,
            Some(value) => match observe(value) {
                Ok(digest) => Self::Verified(digest),
                Err(_) => Self::Unresolved,
            },
        }
    }

    pub(super) fn ensure_verified(&self) -> Result<()> {
        ensure!(
            !matches!(self, Self::Unresolved),
            "Vulkan explicit driver files could not be verified; use absolute manifests and resolvable library paths before testing acceleration"
        );
        Ok(())
    }

    pub(super) fn update_digest(&self, hash: &mut Sha256) {
        match self {
            Self::DefaultDiscovery => hash.update([0]),
            Self::Verified(digest) => {
                hash.update([1]);
                field(hash, digest.as_bytes());
            }
            Self::Unresolved => hash.update([2]),
        }
    }
}

fn field(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_le_bytes());
    hash.update(value);
}

fn observe(value: &std::ffi::OsStr) -> Result<String> {
    ensure!(value.as_encoded_bytes().len() <= MAX_SELECTOR_BYTES);
    let paths: Vec<_> = std::env::split_paths(value).take(MAX_PATHS + 1).collect();
    ensure!(!paths.is_empty() && paths.len() <= MAX_PATHS);
    let mut hash = Sha256::new();
    hash.update(b"explicit-vulkan-driver-files-v1\0");
    let mut count = 0;
    for path in paths {
        // Listing and workers use the component directory, not the GUI cwd.
        ensure!(path.is_absolute(), "relative manifest scope is unknown");
        let metadata = fs::metadata(&path)?;
        if metadata.is_dir() {
            let (directory, before) = open_snapshot(&path, true)?;
            let mut entries = fs::read_dir(&path)?
                .take(MAX_DIRECTORY_ENTRIES + 1)
                .collect::<std::io::Result<Vec<_>>>()?;
            ensure!(entries.len() <= MAX_DIRECTORY_ENTRIES);
            entries.sort_by_key(|entry| entry.file_name());
            stamp(&mut hash, &before);
            for entry in entries {
                // All entry names bind membership; only flat JSONs are read.
                field(&mut hash, entry.file_name().as_encoded_bytes());
                if entry.path().extension() == Some(std::ffi::OsStr::new("json")) {
                    manifest(&entry.path(), &mut hash, &mut count)?;
                }
            }
            check_snapshot(&directory, &path, true, &before)?;
        } else {
            manifest(&path, &mut hash, &mut count)?;
        }
    }
    ensure!(count > 0, "no explicit manifests were observed");
    Ok(format!("{:x}", hash.finalize()))
}

fn manifest(path: &Path, hash: &mut Sha256, count: &mut usize) -> Result<()> {
    *count += 1;
    ensure!(*count <= MAX_MANIFESTS);
    let (mut file, before) = open_snapshot(path, false)?;
    ensure!(before.bytes <= MAX_MANIFEST_BYTES);
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_MANIFEST_BYTES);
    // Typed fields reject exact duplicates instead of serde_json::Value's
    // last-wins behavior, which differs from the native loader's first match.
    let json: Manifest = serde_json::from_slice(&bytes)?;
    ensure!(no_native_shadow(&json.other, "ICD"));
    ensure!(no_native_shadow(&json.icd.other, "library_path"));
    ensure!(!json.icd.library_path.is_empty());
    let library = Path::new(&json.icd.library_path);
    let library = if library.is_absolute() {
        library.to_owned()
    } else {
        // Khronos' Windows combiner checks DIRECTORY_SYMBOL ('\\'), while
        // Rust also accepts '/'. A slash-only relative string can be passed
        // through to LoadLibrary instead of being relative to the manifest.
        #[cfg(windows)]
        ensure!(library.as_os_str().as_encoded_bytes().contains(&b'\\'));
        // A filename alone invokes the OS shared-library search algorithm.
        // Do not guess that a sibling file is what the loader will open.
        ensure!(library.parent().is_some_and(|p| !p.as_os_str().is_empty()));
        ensure!(!library
            .components()
            .any(|c| matches!(c, Component::Prefix(_) | Component::RootDir)));
        path.parent()
            .ok_or_else(|| anyhow::anyhow!("manifest has no parent"))?
            .join(library)
    };
    let (library_file, library_stamp) = open_snapshot(&library, false)?;
    check_snapshot(&library_file, &library, false, &library_stamp)?;
    check_snapshot(&file, path, false, &before)?;
    field(hash, path.as_os_str().as_encoded_bytes());
    stamp(hash, &before);
    field(hash, &Sha256::digest(&bytes));
    stamp(hash, &library_stamp);
    Ok(())
}

#[derive(PartialEq, Eq)]
struct FileStamp {
    canonical: PathBuf,
    bytes: u64,
    modified: SystemTime,
    native: Vec<u8>,
}

fn stamp(hash: &mut Sha256, stamp: &FileStamp) {
    field(hash, stamp.canonical.as_os_str().as_encoded_bytes());
    hash.update(stamp.bytes.to_le_bytes());
    // Native identity already includes precise mtime; retain the generic one
    // as a comparison boundary for unsupported/extended filesystems.
    field(hash, format!("{:?}", stamp.modified).as_bytes());
    field(hash, &stamp.native);
}

fn open_snapshot(path: &Path, directory: bool) -> Result<(File, FileStamp)> {
    let canonical = fs::canonicalize(path)?;
    let metadata = fs::metadata(&canonical)?;
    ensure!(if directory {
        metadata.is_dir()
    } else {
        metadata.is_file()
    });
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A file may be replaced after the preliminary kind check. Avoid
        // waiting on a FIFO, and do not follow a replacement final symlink.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_READ};
        // Windows may defer file times until a writer closes. An open writer
        // or delete handle must make this snapshot unavailable instead.
        options.share_mode(FILE_SHARE_READ.0);
        if directory {
            options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0);
        }
    }
    let file = options.open(&canonical)?;
    let stamp = file_stamp(&file, canonical, directory)?;
    Ok((file, stamp))
}

fn check_snapshot(file: &File, path: &Path, directory: bool, before: &FileStamp) -> Result<()> {
    let (_, current) = open_snapshot(path, directory)?;
    ensure!(
        *before == current && *before == file_stamp(file, before.canonical.clone(), directory)?,
        "driver input changed during observation"
    );
    Ok(())
}

fn file_stamp(file: &File, canonical: PathBuf, directory: bool) -> Result<FileStamp> {
    let metadata = file.metadata()?;
    ensure!(if directory {
        metadata.is_dir()
    } else {
        metadata.is_file()
    });
    let mut native = Vec::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        native.extend(metadata.dev().to_le_bytes());
        native.extend(metadata.ino().to_le_bytes());
        native.extend(metadata.mtime().to_le_bytes());
        native.extend(metadata.mtime_nsec().to_le_bytes());
        native.extend(metadata.ctime().to_le_bytes());
        native.extend(metadata.ctime_nsec().to_le_bytes());
    }
    #[cfg(windows)]
    {
        use std::{mem::size_of, os::windows::io::AsRawHandle};
        use windows::Win32::{
            Foundation::HANDLE,
            Storage::FileSystem::{
                FileBasicInfo, FileIdInfo, GetFileInformationByHandleEx, FILE_BASIC_INFO,
                FILE_ID_INFO,
            },
        };
        let mut id = FILE_ID_INFO::default();
        let mut basic = FILE_BASIC_INFO::default();
        unsafe {
            let handle = HANDLE(file.as_raw_handle());
            GetFileInformationByHandleEx(
                handle,
                FileIdInfo,
                (&mut id as *mut FILE_ID_INFO).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )?;
            GetFileInformationByHandleEx(
                handle,
                FileBasicInfo,
                (&mut basic as *mut FILE_BASIC_INFO).cast(),
                size_of::<FILE_BASIC_INFO>() as u32,
            )?;
        }
        ensure!(id.FileId.Identifier != [0; 16] && basic.ChangeTime != 0);
        native.extend(id.VolumeSerialNumber.to_le_bytes());
        native.extend(id.FileId.Identifier);
        native.extend(basic.LastWriteTime.to_le_bytes());
        native.extend(basic.ChangeTime.to_le_bytes());
    }
    #[cfg(not(any(unix, windows)))]
    anyhow::bail!("stable driver file identity unavailable");
    Ok(FileStamp {
        canonical,
        bytes: metadata.len(),
        modified: metadata.modified()?,
        native,
    })
}
