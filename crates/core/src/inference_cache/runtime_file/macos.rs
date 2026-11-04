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
    file_ident: libc::uintptr_t,
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
        Ok(Self {
            queue,
            file_ident: file.as_raw_fd() as libc::uintptr_t,
        })
    }
    #[cfg(test)]
    pub(super) fn swap_descriptor(&mut self, replacement: OwnedFd) -> OwnedFd {
        std::mem::replace(&mut self.queue, replacement)
    }

    // Receipt's mutex serializes consumption and makes every error/event sticky.
    pub(super) fn check_current(&self) -> Result<()> {
        // Two vnode registrations can each have one queued record. Inspect
        // both so an access-time notification cannot hide a simultaneous write.
        // SAFETY: zero is valid for kevent output records written by the kernel.
        let mut events: [libc::kevent; 2] = unsafe { std::mem::zeroed() };
        let timeout = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: queue is retained; output has space for two records; timeout never waits.
        let count = unsafe {
            libc::kevent(
                self.queue.as_raw_fd(),
                ptr::null(),
                0,
                events.as_mut_ptr(),
                events.len() as i32,
                &timeout,
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error())
                .context("cannot inspect runtime vnode observer");
        }
        for event in &events[..count as usize] {
            // exec can update access time and emit NOTE_ATTRIB while dev/inode,
            // size, mtime and ctime remain unchanged. Receipt still compares
            // the held and named identities. Keep every other file/parent event
            // and EV_ERROR/EV_EOF or unknown flags as a sticky failure.
            let access_attribute = event.ident == self.file_ident
                && event.filter == libc::EVFILT_VNODE
                && event.fflags == libc::NOTE_ATTRIB
                && event.flags & !(libc::EV_ADD | libc::EV_ENABLE | libc::EV_CLEAR) == 0
                && event.data == 0;
            ensure!(
                access_attribute,
                "runtime vnode observer received a mutation or error event"
            );
        }
        Ok(())
    }
}
