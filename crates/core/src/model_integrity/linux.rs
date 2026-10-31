//! Receipt-local invalidation for ordinary local filesystem mutations.
//!
//! Inode timestamps can remain unchanged across several writes in one Linux
//! clock tick. These watches complement snapshots; they do not observe mmap or
//! remote network-filesystem writes, or pin paths reopened by native loaders.
use super::{failure, io_failure, Descriptor};
use anyhow::Result;
use std::{
    ffi::CString,
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::Path,
    sync::Mutex,
};

// Reads/open/close-read events are deliberately absent: verification and
// native read-only model loads must not invalidate their own evidence.
const MUTATIONS: u32 = libc::IN_MODIFY
    | libc::IN_ATTRIB
    | libc::IN_CLOSE_WRITE
    | libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_DELETE_SELF
    | libc::IN_MOVE_SELF
    | libc::IN_UNMOUNT;

#[derive(Debug)]
pub(super) struct MutationObserver {
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    fd: OwnedFd,
    dirty: bool,
}

impl MutationObserver {
    /// Register the complete group before the first snapshot or content read.
    pub(super) fn open(directory: &Path, assets: &[Descriptor]) -> Result<Self> {
        // SAFETY: this call returns a new fd, transferred exactly once to OwnedFd.
        let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if raw < 0 {
            return Err(io_failure(
                "cannot create mutation observer for",
                directory,
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: raw is a newly created, owned descriptor from inotify_init1.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        Self::watch(&fd, directory, true)?;
        for asset in assets {
            // Inode watches also catch writes through hardlinks outside this
            // directory. A directory-only watch cannot observe those accesses.
            Self::watch(&fd, &directory.join(&asset.name), false)?;
        }
        Ok(Self {
            state: Mutex::new(State { fd, dirty: false }),
        })
    }

    fn watch(fd: &OwnedFd, path: &Path, directory: bool) -> Result<()> {
        let name = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| failure("NUL byte in mutation observer path"))?;
        let mask = MUTATIONS | libc::IN_DONT_FOLLOW | if directory { libc::IN_ONLYDIR } else { 0 };
        // SAFETY: name is NUL-terminated and fd remains owned for this call.
        if unsafe { libc::inotify_add_watch(fd.as_raw_fd(), name.as_ptr(), mask) } < 0 {
            return Err(io_failure(
                "cannot watch model path",
                path,
                io::Error::last_os_error(),
            ));
        }
        Ok(())
    }

    pub(super) fn check_current(&self, directory: &Path) -> Result<()> {
        // Serialize consuming the event queue with publishing sticky dirtiness.
        // Otherwise another owner could see EAGAIN after an event was consumed
        // but before the first checker had published its invalidation.
        let mut state = self
            .state
            .lock()
            .map_err(|_| failure("model mutation observer lock poisoned"))?;
        if state.dirty {
            return Err(io_failure(
                "model mutation observed in",
                directory,
                "receipt invalidated",
            ));
        }
        let mut events = [0_u8; 4096];
        // SAFETY: events is a valid writable buffer; the mutex keeps fd alive.
        let count = unsafe {
            libc::read(
                state.fd.as_raw_fd(),
                events.as_mut_ptr().cast(),
                events.len(),
            )
        };
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::WouldBlock {
                return Ok(());
            }
            state.dirty = true;
            return Err(io_failure(
                "cannot read mutation observer for",
                directory,
                error,
            ));
        }
        // Every subscribed event is a mutation. IN_Q_OVERFLOW and IN_IGNORED
        // arrive independently of the subscription mask and also invalidate.
        // No event parsing or unbounded queue drain is needed to reject trust.
        state.dirty = true;
        if count == 0 {
            return Err(io_failure(
                "cannot read mutation observer for",
                directory,
                "unexpected end of event stream",
            ));
        }
        Err(io_failure(
            "model mutation observed in",
            directory,
            "receipt invalidated",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write, os::unix::net::UnixStream, path::PathBuf, sync::Arc};

    fn fixture() -> (tempfile::TempDir, PathBuf, Vec<Descriptor>) {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("model");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("weights.gguf"), b"good").unwrap();
        let assets = vec![Descriptor {
            name: "weights.gguf".into(),
            url: "https://example.invalid/pinned".into(),
            sha256: "0".repeat(64),
            bytes: 4,
        }];
        (temp, directory, assets)
    }

    #[test]
    fn missing_directory_or_asset_cannot_create_an_observer() {
        let (_temp, directory, assets) = fixture();
        assert!(MutationObserver::open(&directory.join("missing"), &assets).is_err());
        fs::remove_file(directory.join("weights.gguf")).unwrap();
        assert!(MutationObserver::open(&directory, &assets).is_err());
    }

    #[test]
    fn observer_fd_flags_and_final_owner_lifetime() {
        let (_temp, directory, assets) = fixture();
        let observer = Arc::new(MutationObserver::open(&directory, &assets).unwrap());
        let other_owner = Arc::clone(&observer);
        let last_owner = Arc::downgrade(&observer);
        let raw = observer.state.lock().unwrap().fd.as_raw_fd();
        // SAFETY: the observer owners retain raw until both Arcs are dropped.
        let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
        assert!(flags >= 0);
        assert_ne!(flags & libc::O_NONBLOCK, 0);
        // SAFETY: same valid retained descriptor.
        let flags = unsafe { libc::fcntl(raw, libc::F_GETFD) };
        assert!(flags >= 0);
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
        drop(observer);
        other_owner.check_current(&directory).unwrap();
        assert!(last_owner.upgrade().is_some());
        drop(other_owner);
        // OwnedFd closes with the final State owner. Do not probe the old
        // integer after drop: parallel tests could already have reused it.
        assert!(last_owner.upgrade().is_none());
    }

    #[test]
    fn read_error_stays_dirty_even_after_an_empty_observer_fd_is_restored() {
        let (_temp, directory, assets) = fixture();
        let observer = MutationObserver::open(&directory, &assets).unwrap();
        // Reading a directory fd fails with EISDIR, without closing an fd that
        // OwnedFd still owns or risking descriptor reuse/double-close in tests.
        observer.state.lock().unwrap().fd = fs::File::open(&directory).unwrap().into();
        assert!(observer.check_current(&directory).is_err());
        let clean = MutationObserver::open(&directory, &assets).unwrap();
        clean.check_current(&directory).unwrap();
        observer.state.lock().unwrap().fd = clean.state.into_inner().unwrap().fd;
        assert!(observer.check_current(&directory).is_err());
    }

    #[test]
    fn queued_overflow_event_is_sticky_without_requiring_a_watch_descriptor() {
        let (_temp, directory, assets) = fixture();
        let observer = MutationObserver::open(&directory, &assets).unwrap();
        let (reader, mut writer) = UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        // Inject the kernel overflow record layout through a private test fd.
        // This avoids overflowing the host's real shared event limits.
        let mut event = Vec::new();
        event.extend((-1_i32).to_ne_bytes());
        event.extend(libc::IN_Q_OVERFLOW.to_ne_bytes());
        event.extend(0_u32.to_ne_bytes());
        event.extend(0_u32.to_ne_bytes());
        writer.write_all(&event).unwrap();
        observer.state.lock().unwrap().fd = reader.into();
        assert!(observer.check_current(&directory).is_err());
        assert!(observer.check_current(&directory).is_err());
    }
}
