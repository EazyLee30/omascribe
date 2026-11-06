//! Ordinary local mutation observations, not mmap/remote-filesystem immutability.
use anyhow::{ensure, Context, Result};
use std::{
    ffi::CString,
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::Path,
};

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

pub(super) struct Observer {
    fd: OwnedFd,
}
impl Observer {
    pub(super) fn open(parent: &Path, file: &Path) -> Result<Self> {
        // SAFETY: a successful descriptor is newly owned and transferred once.
        let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        ensure!(
            raw >= 0,
            "cannot create runtime mutation observer: {}",
            io::Error::last_os_error()
        );
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        for (path, directory) in [(parent, true), (file, false)] {
            let name = CString::new(path.as_os_str().as_bytes())
                .context("invalid runtime observer path")?;
            let mask =
                MUTATIONS | libc::IN_DONT_FOLLOW | if directory { libc::IN_ONLYDIR } else { 0 };
            // SAFETY: fd and the NUL-terminated path remain alive during the call.
            ensure!(
                unsafe { libc::inotify_add_watch(fd.as_raw_fd(), name.as_ptr(), mask) } >= 0,
                "cannot register runtime mutation observer: {}",
                io::Error::last_os_error()
            );
        }
        Ok(Self { fd })
    }
    #[cfg(test)]
    pub(super) fn swap_descriptor(&mut self, replacement: OwnedFd) -> OwnedFd {
        std::mem::replace(&mut self.fd, replacement)
    }

    // The containing Receipt serializes reads and makes all events/errors sticky.
    pub(super) fn check_current(&self) -> Result<()> {
        let mut bytes = [0_u8; 4096];
        // SAFETY: the owned fd is live and bytes is a writable output buffer.
        let count =
            unsafe { libc::read(self.fd.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) };
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::WouldBlock {
                return Ok(());
            }
            return Err(error).context("cannot inspect runtime mutation observer");
        }
        // IN_Q_OVERFLOW/IN_IGNORED also invalidate; never treat queue loss as clean.
        anyhow::bail!("runtime mutation observer received a mutation or invalid event stream")
    }
}
