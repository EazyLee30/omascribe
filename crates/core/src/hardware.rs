//! Local adapter inventory. This detects candidates, not successful model acceleration.
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GpuInventory {
    pub schema: u32,
    pub status: String,
    pub devices: Vec<GpuDevice>,
    pub error: Option<String>,
    /// Maximum CUDA Driver API supported by the installed driver, not the SDK.
    #[serde(default)]
    pub cuda_driver_version: Option<i32>,
    /// Provenance of the adapter driver version, not an acceleration result.
    /// Missing on older helpers; kernel-only versions are not userspace identity.
    #[serde(default)]
    pub driver_identity: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuDevice {
    pub local_id: String,
    pub name: String,
    pub vendor: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub driver: Option<String>,
    pub driver_version: Option<String>,
    pub dedicated_memory_bytes: Option<u64>,
    pub shared_memory_bytes: Option<u64>,
    pub budget_bytes: Option<u64>,
    pub budget_source: Option<String>,
    pub integrated: Option<bool>,
}
#[cfg(any(target_os = "linux", target_os = "windows", test))]
fn vendor_name(id: u32) -> &'static str {
    match id {
        0x10de => "nvidia",
        0x1002 | 0x1022 => "amd",
        0x8086 => "intel",
        _ => "other",
    }
}
fn unavailable(reason: &str) -> GpuInventory {
    GpuInventory {
        schema: 1,
        status: "unavailable".into(),
        devices: vec![],
        error: Some(reason.into()),
        cuda_driver_version: None,
        driver_identity: None,
    }
}

/// Execute driver APIs in the app's helper, never on the GUI process or audio thread.
pub fn detect(root: &Path, runtime: &Path) -> GpuInventory {
    let name = crate::platform::executable("omascribe-cli");
    let current = std::env::current_exe().ok();
    let sibling = current
        .as_ref()
        .and_then(|p| p.parent())
        .map(|p| p.join(&name));
    let helper = sibling
        .filter(|p| p.is_file())
        .unwrap_or_else(|| runtime.join(name));
    if !helper.is_file() {
        return unavailable("probe_helper_missing");
    }
    let temporary = root.join(format!(".hardware-probe-{}", uuid::Uuid::new_v4()));
    let mut command = crate::platform::child_command(helper);
    // Older helpers must not open the user's workspace if they don't understand this flag.
    command
        .arg("--hardware-probe")
        .env("OMASCRIBE_DATA_DIR", &temporary);
    let result = crate::probe::capture(&mut command, Duration::from_secs(5), 64 * 1024);
    let _ = std::fs::remove_dir_all(temporary);
    match result {
        Ok(bytes) => match serde_json::from_slice::<GpuInventory>(&bytes) {
            Ok(value) if value.schema == 1 && value.devices.len() <= 32 => value,
            _ => unavailable("invalid_probe_output"),
        },
        Err(_) => unavailable("probe_failed_or_timed_out"),
    }
}

/// Only the short-lived CLI probe calls this function.
pub fn native_inventory() -> GpuInventory {
    #[cfg(target_os = "windows")]
    {
        windows_inventory()
    }
    #[cfg(target_os = "linux")]
    {
        linux_inventory(Path::new("/sys/bus/pci/devices"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        GpuInventory {
            schema: 1,
            status: "not_required".into(),
            devices: vec![],
            error: None,
            cuda_driver_version: None,
            driver_identity: None,
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_inventory(pci: &Path) -> GpuInventory {
    fn number(path: &Path) -> Option<u64> {
        let value = std::fs::read_to_string(path).ok()?;
        let value = value.trim();
        if let Some(hex) = value.strip_prefix("0x") {
            u64::from_str_radix(hex, 16).ok()
        } else {
            value.parse().ok()
        }
    }
    let Ok(entries) = std::fs::read_dir(pci) else {
        return unavailable("pci_inventory_unavailable");
    };
    let mut devices = vec![];
    for entry in entries.flatten() {
        let path = entry.path();
        // Display / 3D controllers, including headless NVIDIA compute adapters.
        if number(&path.join("class")).is_none_or(|c| c >> 16 != 3) {
            continue;
        }
        let (Some(vendor), Some(device)) =
            (number(&path.join("vendor")), number(&path.join("device")))
        else {
            continue;
        };
        let vendor = vendor as u32;
        let driver = std::fs::read_link(path.join("driver"))
            .ok()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()));
        let total = number(&path.join("mem_info_vram_total"));
        let driver_version = std::fs::read_to_string(path.join("driver/module/version"))
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .or_else(|| {
                driver.as_ref().and_then(|_| {
                    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").ok()?;
                    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
                    Some(format!("kernel:{}:boot:{}", kernel.trim(), boot.trim()))
                })
            });
        let used = number(&path.join("mem_info_vram_used"));
        let budget = total
            .zip(used)
            .map(|(total, used)| total.saturating_sub(used));
        devices.push(GpuDevice {
            local_id: entry.file_name().to_string_lossy().into_owned(),
            name: std::fs::read_to_string(path.join("product_name"))
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("{} GPU {:04x}", vendor_name(vendor), device)),
            vendor: vendor_name(vendor).into(),
            vendor_id: vendor,
            device_id: device as u32,
            driver,
            driver_version,
            dedicated_memory_bytes: total,
            shared_memory_bytes: None,
            budget_bytes: budget,
            budget_source: budget.map(|_| "sysfs_device_free_estimate".into()),
            integrated: None,
        });
        if devices.len() == 32 {
            break;
        }
    }
    devices.sort_by(|a, b| a.local_id.cmp(&b.local_id));
    GpuInventory {
        schema: 1,
        status: "detected".into(),
        devices,
        error: None,
        cuda_driver_version: None,
        driver_identity: Some("linux_kernel_only".into()),
    }
}

#[cfg(target_os = "windows")]
fn cuda_driver_version() -> Option<i32> {
    use windows::{
        core::{s, w},
        Win32::{Foundation::FreeLibrary, System::LibraryLoader::*},
    };
    // Only load the system's driver library in the isolated hardware helper.
    // No toolkit, driver installation, cuInit, or GPU allocation is required.
    unsafe {
        let library = LoadLibraryExW(w!("nvcuda.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
        let result = GetProcAddress(library, s!("cuDriverGetVersion")).and_then(|entry| {
            let version: unsafe extern "system" fn(*mut i32) -> i32 = std::mem::transmute(entry);
            let mut value = 0;
            (version(&mut value) == 0 && value > 0).then_some(value)
        });
        let _ = FreeLibrary(library);
        result
    }
}

#[cfg(target_os = "windows")]
fn windows_inventory() -> GpuInventory {
    use windows::{core::Interface, Win32::Graphics::Dxgi::*};
    let mut devices = vec![];
    // All COM interfaces are scoped to this helper invocation and released by RAII.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return unavailable("dxgi_unavailable");
        };
        for index in 0..32 {
            let adapter = match factory.EnumAdapters1(index) {
                Ok(adapter) => adapter,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(_) => return unavailable("dxgi_enumeration_failed"),
            };
            let Ok(desc) = adapter.GetDesc1() else {
                continue;
            };
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            let end = desc
                .Description
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(desc.Description.len());
            let mut memory = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
            let budget = adapter.cast::<IDXGIAdapter3>().ok().and_then(|a| {
                a.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut memory)
                    .ok()
                    .map(|_| memory.Budget)
            });
            let driver_version = adapter
                .CheckInterfaceSupport(&IDXGIDevice::IID)
                .ok()
                .map(|version| version.to_string());
            devices.push(GpuDevice {
                local_id: format!(
                    "{:08x}:{:08x}",
                    desc.AdapterLuid.HighPart as u32, desc.AdapterLuid.LowPart
                ),
                name: String::from_utf16_lossy(&desc.Description[..end]),
                vendor: vendor_name(desc.VendorId).into(),
                vendor_id: desc.VendorId,
                device_id: desc.DeviceId,
                driver: None,
                driver_version,
                dedicated_memory_bytes: Some(desc.DedicatedVideoMemory as u64),
                shared_memory_bytes: Some(desc.SharedSystemMemory as u64),
                budget_bytes: budget,
                budget_source: budget.map(|_| "dxgi_probe_process_budget".into()),
                integrated: None,
            });
        }
    }
    GpuInventory {
        schema: 1,
        status: "detected".into(),
        devices,
        error: None,
        cuda_driver_version: cuda_driver_version(),
        driver_identity: Some("dxgi_adapter_version".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linux_detects_both_vendors_without_vendor_command_line_tools() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        for (id, vendor, class) in [
            ("gpu-01", "0x10de", "0x030200"),
            ("gpu-02", "0x1002", "0x030000"),
            ("other", "0x8086", "0x020000"),
        ] {
            let p = root.path().join(id);
            std::fs::create_dir(&p)?;
            for (file, value) in [("vendor", vendor), ("device", "0x1234"), ("class", class)] {
                std::fs::write(p.join(file), value)?;
            }
        }
        let result = linux_inventory(root.path());
        assert_eq!(result.devices.len(), 2);
        assert_eq!(result.devices[0].vendor, "nvidia");
        assert_eq!(result.devices[1].vendor, "amd");
        assert_eq!(
            result.devices[0].budget_bytes, None,
            "unknown memory is not zero"
        );
        assert_eq!(
            linux_inventory(&root.path().join("missing")).status,
            "unavailable"
        );
        Ok(())
    }
}
