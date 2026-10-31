//! Process-local observations of one runtime file, never a model or SDK bundle.
//!
//! A cold observation hashes the complete bounded file after installing mutation
//! watches (Unix) or retaining deny-write/delete handles (Windows). Warm checks
//! read no file contents. This is not an immutable native-loader snapshot:
//! unobservable writable mmap, remote filesystems and later path reopening are
//! outside the guarantee. It does not establish complete dependency/driver identity.
use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

const MAX_RUNTIME_BYTES: u64 = 256 * 1024 * 1024;
const READ_BUFFER_BYTES: usize = 64 * 1024;

pub(super) struct Receipt {
    path: PathBuf,
    parent_path: PathBuf,
    file: File,
    parent: File,
    file_identity: Identity,
    parent_identity: Identity,
    digest: String,
    // Serialize queue consumption with sticky invalidation for every Arc owner.
    invalid: Mutex<bool>,
    #[cfg(target_os = "linux")]
    observer: linux::Observer,
    #[cfg(target_os = "macos")]
    observer: macos::Observer,
}

impl Receipt {
    pub(super) fn observe(path: &Path, mut on_read: impl FnMut(usize)) -> Result<Self> {
        ensure!(
            cfg!(any(target_os = "linux", target_os = "macos", windows)),
            "runtime file observation is unsupported on this platform"
        );
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()?.join(path)
        };
        ensure!(path.file_name().is_some(), "runtime file has no name");
        let parent_path = path
            .parent()
            .context("runtime file has no parent")?
            .to_owned();
        let (parent, parent_identity) = open_checked(&parent_path, true)?;
        let (mut file, file_identity) = open_checked(&path, false)?;
        ensure!(
            file_identity.bytes() <= MAX_RUNTIME_BYTES,
            "runtime file exceeds byte limit"
        );
        #[cfg(target_os = "linux")]
        let observer = linux::Observer::open(&parent_path, &path)?;
        #[cfg(target_os = "macos")]
        let observer = macos::Observer::open(&parent, &file)?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        observer.check_current()?;
        check_pair(&parent, &parent_path, true, &parent_identity)?;
        check_pair(&file, &path, false, &file_identity)?;
        let digest = hash_stream(&mut file, file_identity.bytes(), &mut on_read)?;
        let receipt = Self {
            path,
            parent_path,
            file,
            parent,
            file_identity,
            parent_identity,
            digest,
            invalid: Mutex::new(false),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            observer,
        };
        receipt.check_current()?;
        Ok(receipt)
    }

    pub(super) fn digest(&self) -> &str {
        &self.digest
    }

    pub(super) fn check_current(&self) -> Result<()> {
        let mut invalid = self
            .invalid
            .lock()
            .map_err(|_| anyhow::anyhow!("runtime observation lock poisoned"))?;
        ensure!(!*invalid, "runtime observation was invalidated");
        let result = self.check_inner();
        if result.is_err() {
            *invalid = true;
        }
        result
    }

    fn check_inner(&self) -> Result<()> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        self.observer.check_current()?;
        check_pair(&self.parent, &self.parent_path, true, &self.parent_identity)?;
        check_pair(&self.file, &self.path, false, &self.file_identity)?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        self.observer.check_current()?;
        Ok(())
    }
}

fn hash_stream(
    input: &mut impl Read,
    expected: u64,
    on_read: &mut impl FnMut(usize),
) -> Result<String> {
    ensure!(
        expected <= MAX_RUNTIME_BYTES,
        "runtime file exceeds byte limit"
    );
    let mut input = input.take(expected + 1);
    let mut hash = Sha256::new();
    let mut observed = 0_u64;
    let mut buffer = [0_u8; READ_BUFFER_BYTES];
    loop {
        let count = input
            .read(&mut buffer)
            .context("cannot read runtime file")?;
        if count == 0 {
            break;
        }
        on_read(count);
        observed += count as u64;
        ensure!(observed <= expected, "runtime file grew while hashing");
        hash.update(&buffer[..count]);
    }
    ensure!(observed == expected, "runtime file shortened while hashing");
    Ok(format!("{:x}", hash.finalize()))
}

fn validate_kind(metadata: &Metadata, directory: bool) -> Result<()> {
    let kind = metadata.file_type();
    ensure!(!kind.is_symlink(), "runtime observation rejects symlinks");
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        ensure!(
            metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0,
            "runtime observation rejects reparse points"
        );
    }
    ensure!(
        if directory {
            kind.is_dir()
        } else {
            kind.is_file()
        },
        "runtime observation requires a regular file and parent directory"
    );
    Ok(())
}

fn open_checked(path: &Path, directory: bool) -> Result<(File, Identity)> {
    let before = fs::symlink_metadata(path).context("cannot inspect runtime path")?;
    validate_kind(&before, directory)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            libc::O_NOFOLLOW | libc::O_NONBLOCK | if directory { libc::O_DIRECTORY } else { 0 },
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        };
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
    let file = options.open(path).context("cannot open runtime path")?;
    let observed = identity(&file, directory)?;
    #[cfg(unix)]
    ensure!(
        Identity::from_metadata(&before) == observed,
        "runtime path changed while opening"
    );
    Ok((file, observed))
}

fn check_pair(file: &File, path: &Path, directory: bool, expected: &Identity) -> Result<()> {
    let (_, current) = open_checked(path, directory)?;
    ensure!(
        current == *expected && identity(file, directory)? == *expected,
        "runtime file or parent changed after observation"
    );
    Ok(())
}

#[cfg(unix)]
#[derive(Clone, Debug, PartialEq, Eq)]
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
fn identity(file: &File, directory: bool) -> Result<Identity> {
    let metadata = file
        .metadata()
        .context("cannot inspect held runtime file")?;
    validate_kind(&metadata, directory)?;
    Ok(Identity::from_metadata(&metadata))
}

#[cfg(windows)]
#[derive(Clone, Debug, PartialEq, Eq)]
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
fn identity(file: &File, directory: bool) -> Result<Identity> {
    use std::{mem::size_of, os::windows::io::AsRawHandle};
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            FileBasicInfo, FileIdInfo, GetFileInformationByHandleEx, FILE_BASIC_INFO, FILE_ID_INFO,
        },
    };
    let metadata = file
        .metadata()
        .context("cannot inspect held runtime file")?;
    validate_kind(&metadata, directory)?;
    let mut id = FILE_ID_INFO::default();
    let mut basic = FILE_BASIC_INFO::default();
    // SAFETY: both output buffers have the specified size and remain writable.
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
    ensure!(
        id.FileId.Identifier != [0; 16] && basic.ChangeTime != 0,
        "runtime filesystem identity is unavailable"
    );
    Ok(Identity {
        volume: id.VolumeSerialNumber,
        file_id: id.FileId.Identifier,
        bytes: metadata.len(),
        modified: basic.LastWriteTime,
        changed: basic.ChangeTime,
    })
}

#[cfg(not(any(unix, windows)))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity;
#[cfg(not(any(unix, windows)))]
impl Identity {
    fn bytes(&self) -> u64 {
        0
    }
}
#[cfg(not(any(unix, windows)))]
fn identity(_: &File, _: bool) -> Result<Identity> {
    anyhow::bail!("runtime filesystem identity is unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, sync::Arc};

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("runtime");
        fs::create_dir(&directory).unwrap();
        let path = directory.join("server");
        fs::write(&path, b"original").unwrap();
        (root, path)
    }

    #[test]
    fn complete_hash_and_warm_read_only_checks_do_not_read_content_again() -> Result<()> {
        let (_root, path) = fixture();
        let mut bytes = 0;
        let receipt = Receipt::observe(&path, |n| bytes += n)?;
        assert_eq!(bytes, 8);
        assert_eq!(
            receipt.digest(),
            format!("{:x}", Sha256::digest(b"original"))
        );
        for _ in 0..20 {
            assert_eq!(fs::read(&path)?, b"original");
            receipt.check_current()?;
        }
        assert_eq!(bytes, 8);
        Ok(())
    }

    #[test]
    fn missing_nonregular_and_oversized_inputs_never_hash() -> Result<()> {
        let (root, path) = fixture();
        let mut reads = 0;
        assert!(Receipt::observe(&root.path().join("absent"), |n| reads += n).is_err());
        assert!(Receipt::observe(path.parent().unwrap(), |n| reads += n).is_err());
        let file = OpenOptions::new().write(true).open(&path)?;
        file.set_len(MAX_RUNTIME_BYTES + 1)?;
        drop(file);
        assert!(Receipt::observe(&path, |n| reads += n).is_err());
        assert_eq!(reads, 0);
        Ok(())
    }

    #[test]
    fn bounded_hash_rejects_growth_and_shortening_with_real_read_counts() {
        let mut count = 0;
        assert!(hash_stream(&mut Cursor::new(b"four"), 3, &mut |n| count += n).is_err());
        assert_eq!(count, 4); // The excess-byte sentinel was genuinely read.
        let mut count = 0;
        assert!(hash_stream(&mut Cursor::new(b"four"), 5, &mut |n| count += n).is_err());
        assert_eq!(count, 4);
        assert!(hash_stream(
            &mut Cursor::new([]),
            MAX_RUNTIME_BYTES + 1,
            &mut |_| panic!("must not read")
        )
        .is_err());
    }

    #[cfg(unix)]
    fn rewrite_with_old_mtime(
        path: &Path,
        content: &[u8],
        modified: std::time::SystemTime,
    ) -> Result<()> {
        fs::write(path, content)?;
        File::open(path)?.set_times(fs::FileTimes::new().set_modified(modified))?;
        Ok(())
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn same_size_restored_mtime_write_is_observed_and_stays_invalid() -> Result<()> {
        let (_root, path) = fixture();
        let modified = fs::metadata(&path)?.modified()?;
        let receipt = Receipt::observe(&path, |_| {})?;
        rewrite_with_old_mtime(&path, b"modified", modified)?;
        assert_eq!(fs::metadata(&path)?.modified()?, modified);
        assert!(receipt
            .check_current()
            .unwrap_err()
            .to_string()
            .contains("observer"));
        rewrite_with_old_mtime(&path, b"original", modified)?;
        assert!(receipt
            .check_current()
            .unwrap_err()
            .to_string()
            .contains("invalidated"));
        Ok(())
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn same_size_time_atomic_replacement_is_observed() -> Result<()> {
        let (_root, path) = fixture();
        let modified = fs::metadata(&path)?.modified()?;
        let replacement = path.with_file_name("replacement");
        rewrite_with_old_mtime(&replacement, b"modified", modified)?;
        let receipt = Receipt::observe(&path, |_| {})?;
        fs::rename(replacement, &path)?;
        assert_eq!(fs::metadata(&path)?.modified()?, modified);
        assert!(receipt
            .check_current()
            .unwrap_err()
            .to_string()
            .contains("observer"));
        Ok(())
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn outside_parent_hardlink_write_is_observed() -> Result<()> {
        let (root, path) = fixture();
        let alias = root.path().join("outside-hardlink");
        fs::hard_link(&path, &alias)?;
        let modified = fs::metadata(&path)?.modified()?;
        let receipt = Receipt::observe(&path, |_| {})?;
        rewrite_with_old_mtime(&alias, b"modified", modified)?;
        assert!(receipt
            .check_current()
            .unwrap_err()
            .to_string()
            .contains("observer"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlink_and_fifo_are_rejected_without_waiting() -> Result<()> {
        use std::{
            ffi::CString,
            os::unix::{ffi::OsStrExt, fs::symlink},
        };
        let (root, path) = fixture();
        let alias = root.path().join("link");
        symlink(&path, &alias)?;
        assert!(Receipt::observe(&alias, |_| panic!("must not read")).is_err());
        let fifo = root.path().join("fifo");
        let name = CString::new(fifo.as_os_str().as_bytes())?;
        // SAFETY: name is a retained, valid NUL-terminated owned fixture path.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(Receipt::observe(&fifo, |_| panic!("must not read")).is_err());
        Ok(())
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn mutation_during_cold_hash_cannot_publish_a_receipt() -> Result<()> {
        let (_root, path) = fixture();
        fs::write(&path, vec![b'a'; READ_BUFFER_BYTES * 2])?;
        let modified = fs::metadata(&path)?.modified()?;
        let mut changed = false;
        let observed = Receipt::observe(&path, |_| {
            if !changed {
                rewrite_with_old_mtime(&path, &vec![b'b'; READ_BUFFER_BYTES * 2], modified)
                    .unwrap();
                changed = true;
            }
        });
        assert!(changed && observed.is_err());
        Ok(())
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn observer_error_stays_invalid_after_a_clean_queue_is_restored() -> Result<()> {
        let (_root, path) = fixture();
        let mut receipt = Receipt::observe(&path, |_| {})?;
        #[cfg(target_os = "linux")]
        let wrong = File::open(path.parent().unwrap())?;
        #[cfg(target_os = "macos")]
        let wrong = File::open(&path)?;
        let queue = receipt.observer.swap_descriptor(wrong.into());
        assert!(receipt.check_current().is_err());
        drop(receipt.observer.swap_descriptor(queue));
        assert!(receipt
            .check_current()
            .unwrap_err()
            .to_string()
            .contains("invalidated"));
        Ok(())
    }

    #[test]
    fn final_arc_owner_controls_receipt_lifetime() -> Result<()> {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<Receipt>();
        let (_root, path) = fixture();
        let receipt = Arc::new(Receipt::observe(&path, |_| {})?);
        let other = Arc::clone(&receipt);
        let weak = Arc::downgrade(&receipt);
        drop(receipt);
        other.check_current()?;
        assert!(weak.upgrade().is_some());
        drop(other);
        assert!(weak.upgrade().is_none());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_writer_delete_are_denied_until_last_owner_releases() -> Result<()> {
        let (_root, path) = fixture();
        let receipt = Arc::new(Receipt::observe(&path, |_| {})?);
        let other = Arc::clone(&receipt);
        drop(receipt);
        assert!(OpenOptions::new().write(true).open(&path).is_err());
        assert!(fs::remove_file(&path).is_err());
        other.check_current()?;
        drop(other);
        fs::write(&path, b"modified")?;
        assert_eq!(fs::read(&path)?, b"modified");
        fs::remove_file(path)?;
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_existing_writer_prevents_observation() -> Result<()> {
        let (_root, path) = fixture();
        let writer = OpenOptions::new().write(true).open(&path)?;
        assert!(Receipt::observe(&path, |_| panic!("must not read")).is_err());
        drop(writer);
        Receipt::observe(&path, |_| {})?.check_current()?;
        Ok(())
    }
}
