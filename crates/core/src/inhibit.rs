//! Hold the desktop awake while a take is recording.
//!
//! A meeting take runs for tens of minutes and the capture pipeline is a child
//! process; if the session locks, blanks or suspends the writer stops receiving
//! frames and the audio has a hole. Linux sessions solve this with a logind
//! inhibitor, Windows with `SetThreadExecutionState`, macOS with an IOPM
//! assertion. Only Linux is implemented here; the other platforms keep their
//! current behaviour and this module compiles to an inert stub there.
//!
//! The inhibitor is a short-lived child process, not a library call, so a
//! crashed app cannot leak it: the kernel drops the handle with the last
//! reference and logind re-checks. `PR_SET_PDEATHSIG` additionally ends the child
//! when OmaScribe exits without unwinding.

use std::{
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

/// Why the hold exists. Kept short because it is shown by desktop tools.
pub const REASON: &str = "Recording a meeting";

pub struct Inhibitor {
    child: Option<Child>,
    released: Arc<AtomicBool>,
}

#[cfg(target_os = "linux")]
fn spawn() -> Option<Child> {
    // Only an active graphical session has anything to inhibit. Without a
    // session bus (headless CI, SSH, container) systemd-inhibit still runs but
    // holds nothing, so skip the process entirely.
    let session = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
        || std::env::var_os("XDG_SESSION_ID").is_some();
    if !session {
        return None;
    }
    let mut command = Command::new("systemd-inhibit");
    command
        .args([
            // `idle` covers Omarchy's screensaver and lock timers; `sleep`
            // covers suspend. `handle-lid-switch` is deliberately absent: the
            // user closing the lid should still suspend.
            "--what=idle:sleep",
            "--who=OmaScribe",
            "--why=Recording a meeting",
            "--mode=block",
            "sleep",
            "infinity",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        let parent = std::process::id() as i32;
        unsafe {
            command.pre_exec(move || {
                // Guard against the app dying before the inhibitor settles.
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                if libc::getppid() as i32 != parent {
                    return Err(std::io::Error::from(std::io::ErrorKind::Other));
                }
                Ok(())
            });
        }
    }
    command.spawn().ok()
}

#[cfg(not(target_os = "linux"))]
fn spawn() -> Option<Child> {
    None
}

impl Inhibitor {
    /// Start holding the session awake. Failure is not fatal: recording must
    /// still work on a desktop that cannot inhibit, it just may idle.
    pub fn acquire() -> Self {
        Self {
            child: spawn(),
            released: Arc::new(AtomicBool::new(false)),
        }
    }
    /// True when a live inhibitor is actually held.
    pub fn held(&self) -> bool {
        self.child.is_some() && !self.released.load(Ordering::SeqCst)
    }
    /// Release the hold. Called on pause, finish and app shutdown.
    pub fn release(&mut self) {
        self.released.store(true, Ordering::SeqCst);
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Inhibitor {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releasing_twice_is_harmless() {
        let mut inhibitor = Inhibitor {
            child: None,
            released: Arc::new(AtomicBool::new(false)),
        };
        assert!(!inhibitor.held());
        inhibitor.release();
        inhibitor.release();
        assert!(!inhibitor.held());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn headless_sessions_do_not_spawn_a_child() {
        // The test runner normally has no session bus. Never assert on the
        // inverse: a developer box running tests inside their session can.
        let previous = std::env::var_os("DBUS_SESSION_BUS_ADDRESS");
        let previous_id = std::env::var_os("XDG_SESSION_ID");
        std::env::remove_var("DBUS_SESSION_BUS_ADDRESS");
        std::env::remove_var("XDG_SESSION_ID");
        let inhibitor = Inhibitor::acquire();
        let held = inhibitor.held();
        if let Some(value) = previous {
            std::env::set_var("DBUS_SESSION_BUS_ADDRESS", value);
        }
        if let Some(value) = previous_id {
            std::env::set_var("XDG_SESSION_ID", value);
        }
        assert!(!held, "a session-less process must not hold an inhibitor");
    }

    /// The hold must actually reach logind, and releasing it must not leave a
    /// stray inhibitor behind: a leak would keep laptops awake after a meeting.
    #[cfg(target_os = "linux")]
    #[test]
    fn session_hold_is_visible_to_logind_and_released_cleanly() {
        // Only meaningful inside a real login session. CI and containers have no
        // session bus, so the behavioural path is verified on a desktop instead
        // of faking one with an unreachable address.
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
            return;
        }
        if !std::process::Command::new("systemd-inhibit")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return;
        }
        let mut inhibitor = Inhibitor::acquire();
        if !inhibitor.held() {
            return; // No usable logind on this session.
        }
        // logind registers a new block inhibitor asynchronously after the tool
        // acquires it; poll briefly instead of racing the listing.
        let mut listing = String::new();
        for _ in 0..40 {
            listing = std::process::Command::new("systemd-inhibit")
                .arg("--list")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default();
            if listing.contains("Recording a meeting") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            listing.contains("Recording a meeting"),
            "the take must register an idle/sleep inhibitor: {listing}"
        );
        inhibitor.release();
        let after = std::process::Command::new("systemd-inhibit")
            .arg("--list")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        let still_there = after
            .lines()
            .any(|line| line.contains("OmaScribe") && line.contains("Recording a meeting"));
        assert!(!still_there, "releasing must drop the inhibitor: {after}");
    }
}
