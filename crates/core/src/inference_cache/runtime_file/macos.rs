//! Zero-timeout vnode observations for the held runtime and parent descriptors.
//! https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/KernelQueues/KernelQueues.html
use anyhow::{ensure, Context, Result};
use std::{
    fs::File,
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    ptr,
};

pub(super) struct Observer {
    queue: OwnedFd,
}
impl Observer {
    pub(super) fn open(parent: &File, file: &File) -> Result<Self> {
        // SAFETY: kqueue returns a new descriptor, transferred exactly once.
        let raw = unsafe { libc::kqueue() };
        ensure!(
            raw >= 0,
            "cannot create runtime vnode observer: {}",
            io::Error::last_os_error()
        );
        let queue = unsafe { OwnedFd::from_raw_fd(raw) };
        // SAFETY: F_SETFD applies only to this owned descriptor.
        ensure!(
            unsafe { libc::fcntl(queue.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } >= 0,
            "cannot mark runtime vnode observer close-on-exec"
        );
        let notes = libc::NOTE_WRITE
            | libc::NOTE_ATTRIB
            | libc::NOTE_RENAME
            | libc::NOTE_DELETE
            | libc::NOTE_EXTEND
            | libc::NOTE_LINK
            | libc::NOTE_REVOKE;
        let changes: Vec<_> = [parent, file]
            .into_iter()
            .map(|file| libc::kevent {
                ident: file.as_raw_fd() as libc::uintptr_t,
                filter: libc::EVFILT_VNODE,
                flags: libc::EV_ADD | libc::EV_CLEAR | libc::EV_ENABLE,
                fflags: notes,
                data: 0,
                udata: ptr::null_mut(),
            })
            .collect();
        let timeout = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: input points to two initialized events; no output buffer is requested.
        let result = unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                changes.as_ptr(),
                changes.len() as i32,
                ptr::null_mut(),
                0,
                &timeout,
            )
        };
        ensure!(
            result == 0,
            "cannot register runtime vnode observer: {}",
            io::Error::last_os_error()
        );
        Ok(Self { queue })
    }
    #[cfg(test)]
    pub(super) fn swap_descriptor(&mut self, replacement: OwnedFd) -> OwnedFd {
        std::mem::replace(&mut self.queue, replacement)
    }

    // Receipt's mutex serializes consumption and makes every error/event sticky.
    pub(super) fn check_current(&self) -> Result<()> {
        // SAFETY: zero is valid for the kevent output record written by the kernel.
        let mut event: libc::kevent = unsafe { std::mem::zeroed() };
        let timeout = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: queue is retained; output is one valid record; zero timeout never waits.
        let count = unsafe {
            libc::kevent(
                self.queue.as_raw_fd(),
                ptr::null(),
                0,
                &mut event,
                1,
                &timeout,
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error())
                .context("cannot inspect runtime vnode observer");
        }
        ensure!(
            count == 0,
            "runtime vnode observer received a mutation or error event"
        );
        Ok(())
    }
}
