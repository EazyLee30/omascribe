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
    return PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default())
        .join("io.omarchy.omascribe");
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
    if let Some(path) = std::env::current_exe()
        .ok()
        .and_then(|exe| bundled_asset_dir(&exe, name))
    {
        return path;
    }
    user
}

fn bundled_asset_dir(exe: &Path, name: &str) -> Option<PathBuf> {
    if let Some(parent) = exe.parent() {
        // Tauri: beside the executable on Windows, Resources on macOS,
        // usr/lib/oma-scribe on Linux (also relative to an extracted AppImage/deb).
        for bundle in std::iter::once(parent.join("../Resources/native"))
            .chain(std::iter::once(parent.join("../lib/oma-scribe/native")))
            .chain(std::iter::once(parent.join("../lib/omascribe/native")))
            .chain(parent.ancestors().take(3).map(|p| p.join("native")))
        {
            let candidate = bundle.join(name);
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    None
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

pub fn available_memory_mib() -> Option<u64> {
    #[cfg(target_os = "linux")]
    return std::fs::read_to_string("/proc/meminfo")
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("MemAvailable:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
        .map(|kib| kib / 1024);
    #[cfg(target_os = "macos")]
    return macos_available_memory_mib();
    #[cfg(target_os = "windows")]
    {
        let mut system = sysinfo::System::new();
        system.refresh_memory();
        (system.total_memory() > 0).then(|| system.available_memory() / 1024 / 1024)
    }
}

#[cfg(target_os = "macos")]
fn macos_available_memory_mib() -> Option<u64> {
    extern "C" {
        fn mach_port_deallocate(
            task: libc::mach_port_t,
            name: libc::mach_port_t,
        ) -> libc::kern_return_t;
    }
    // sysinfo 0.37 subtracts the compressor from an already separate pool, which can
    // report zero on a healthy Mac. Ask Mach for free and reclaimable pages instead.
    unsafe {
        let page_size = libc::sysconf(libc::_SC_PAGESIZE);
        if page_size <= 0 {
            return None;
        }
        let mut stats: libc::vm_statistics64 = std::mem::zeroed();
        let mut count = libc::HOST_VM_INFO64_COUNT;
        #[allow(deprecated)]
        let host = libc::mach_host_self();
        let result = libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            &mut stats as *mut _ as *mut _,
            &mut count,
        );
        #[allow(deprecated)]
        mach_port_deallocate(libc::mach_task_self(), host);
        (result == libc::KERN_SUCCESS).then(|| {
            reclaimable_memory_mib(
                stats.free_count.into(),
                stats.external_page_count.into(),
                stats.purgeable_count.into(),
                page_size as u64,
            )
        })
    }
}

#[cfg(any(target_os = "macos", test))]
fn reclaimable_memory_mib(free: u64, file_backed: u64, purgeable: u64, page_size: u64) -> u64 {
    // XNU's fully reclaimable pool: free + file-backed + purgeable. Do not count
    // anonymous inactive pages, compressed pages, or swap as immediately available.
    // Speculative pages are already in free_count; adding them would count twice.
    // https://github.com/apple-oss-distributions/xnu/blob/main/doc/vm/memorystatus_notify.md
    free.saturating_add(file_backed)
        .saturating_add(purgeable)
        .saturating_mul(page_size)
        / 1024
        / 1024
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_gui_and_cli_find_tauris_kebab_cased_resource_directory() {
        let root = tempfile::tempdir().unwrap();
        let native = root.path().join("usr/lib/oma-scribe/native");
        std::fs::create_dir_all(native.join("models")).unwrap();
        std::fs::create_dir_all(native.join("runtime")).unwrap();
        std::fs::create_dir_all(root.path().join("usr/bin")).unwrap();
        for exe in [
            root.path().join("usr/bin/omascribe"),
            native.join("runtime/omascribe-cli"),
        ] {
            assert_eq!(
                bundled_asset_dir(&exe, "models")
                    .unwrap()
                    .canonicalize()
                    .unwrap(),
                native.join("models").canonicalize().unwrap()
            );
        }
    }

    #[test]
    fn macos_cache_remains_available_on_a_compressed_system() {
        // A 24 GiB Mac with ~9 GiB in the compressor used to report zero.
        assert_eq!(reclaimable_memory_mib(11569, 251077, 8463, 16384), 4236);
    }

    #[test]
    fn macos_respects_page_size_and_real_low_memory() {
        assert_eq!(reclaimable_memory_mib(1024, 2048, 1024, 4096), 16);
        assert_eq!(reclaimable_memory_mib(1024, 2048, 1024, 16384), 64);
        assert_eq!(reclaimable_memory_mib(0, 0, 0, 16384), 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_can_read_mach_memory_statistics() {
        assert!(macos_available_memory_mib().is_some());
    }
}
