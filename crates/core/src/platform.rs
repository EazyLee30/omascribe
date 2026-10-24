//! Small OS boundary. Models and inference remain identical across desktop platforms.
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub fn executable(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

pub fn data_root() -> PathBuf {
    if let Some(path) = std::env::var_os("OMASCRIBE_DATA_DIR") {
        return path.into();
    }
    #[cfg(target_os = "windows")]
    return PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("OmaScribe");
    #[cfg(target_os = "macos")]
    return PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support/OmaScribe");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        })
        .join("omascribe")
}

pub fn asset_dir(root: &Path, name: &str, variable: &str) -> PathBuf {
    if let Some(path) = std::env::var_os(variable) {
        return path.into();
    }
    let user = root.join(name);
    if user.is_dir() {
        return user;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            // Tauri resources: beside the executable on Windows, Contents/Resources on macOS.
            for bundle in std::iter::once(parent.join("../Resources/native"))
                .chain(parent.ancestors().take(3).map(|p| p.join("native")))
            {
                let candidate = bundle.join(name);
                if candidate.is_dir() {
                    return candidate;
                }
            }
        }
    }
    user
}

pub fn child_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW: inference is an app-owned background task.
    }
    cmd
}

pub fn stop_process(pid: u32, interrupt: bool) {
    if pid == 0 {
        return;
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(
            pid as i32,
            if interrupt {
                libc::SIGINT
            } else {
                libc::SIGTERM
            },
        );
    }
    #[cfg(windows)]
    {
        let _ = interrupt;
        let _ = child_command("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
}

pub fn available_memory_mib() -> u64 {
    #[cfg(target_os = "linux")]
    return std::fs::read_to_string("/proc/meminfo")
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("MemAvailable:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        / 1024;
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let mut system = sysinfo::System::new();
        system.refresh_memory();
        system.available_memory() / 1024 / 1024
    }
}
