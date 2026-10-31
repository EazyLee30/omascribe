//! Bind GPU selection to the exact selector values passed to a child.
//! Explicit Vulkan driver files are observed separately from selector strings.
//! Default driver discovery, loaded libraries and unknown layers remain unfinished.
use super::Backend;
use anyhow::{ensure, Result};
use sha2::{Digest, Sha256};
use std::{ffi::OsString, fmt, process::Command};

mod driver_files;
use driver_files::DriverFiles;

const CUDA_KEYS: [&str; 2] = ["CUDA_VISIBLE_DEVICES", "CUDA_DEVICE_ORDER"];

const VULKAN_KEYS: &[&str] = &[
    "GGML_VK_VISIBLE_DEVICES",
    "VK_DRIVER_FILES",
    "VK_ICD_FILENAMES",
    "VK_ADD_DRIVER_FILES",
    "VK_LOADER_DRIVERS_SELECT",
    "VK_LOADER_DRIVERS_DISABLE",
    "VK_LAYER_PATH",
    "VK_ADD_LAYER_PATH",
    "VK_IMPLICIT_LAYER_PATH",
    "VK_ADD_IMPLICIT_LAYER_PATH",
    "VK_INSTANCE_LAYERS",
    "VK_LOADER_LAYERS_ENABLE",
    "VK_LOADER_LAYERS_DISABLE",
    "VK_LOADER_LAYERS_ALLOW",
    "DRI_PRIME",
    "MESA_VK_DEVICE_SELECT",
    "MESA_VK_DEVICE_SELECT_FORCE_DEFAULT_DEVICE",
    #[cfg(target_os = "linux")]
    "HOME",
    #[cfg(target_os = "linux")]
    "XDG_CONFIG_HOME",
    #[cfg(target_os = "linux")]
    "XDG_CONFIG_DIRS",
    #[cfg(target_os = "linux")]
    "XDG_DATA_HOME",
    #[cfg(target_os = "linux")]
    "XDG_DATA_DIRS",
    #[cfg(target_os = "linux")]
    "__NV_PRIME_RENDER_OFFLOAD",
    #[cfg(target_os = "linux")]
    "__VK_LAYER_NV_optimus",
];

#[derive(Clone, PartialEq, Eq)]
enum Scope {
    Unscoped,
    Cuda([Option<OsString>; 2]),
    Vulkan(Vec<Option<OsString>>),
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SelectorEnv {
    scope: Scope,
    driver_files: DriverFiles,
}

impl fmt::Debug for SelectorEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Environment values are private launch inputs, not diagnostic text.
        f.debug_struct("SelectorEnv")
            .field(
                "backend",
                &match self.scope {
                    Scope::Cuda(_) => "cuda",
                    Scope::Vulkan(_) => "vulkan",
                    Scope::Unscoped => "unscoped",
                },
            )
            .field("digest", &self.digest())
            .finish()
    }
}

impl SelectorEnv {
    pub(crate) fn capture(backend: Backend) -> Self {
        Self::capture_from(backend, |key| std::env::var_os(key))
    }

    fn capture_from(backend: Backend, mut lookup: impl FnMut(&str) -> Option<OsString>) -> Self {
        let scope = match backend {
            Backend::Cuda => Scope::Cuda(CUDA_KEYS.map(lookup)),
            Backend::Vulkan => Scope::Vulkan(VULKAN_KEYS.iter().map(|key| lookup(key)).collect()),
            _ => Scope::Unscoped,
        };
        let driver_files = match &scope {
            Scope::Vulkan(values) => DriverFiles::capture(VULKAN_KEYS, values),
            _ => DriverFiles::default(),
        };
        Self {
            scope,
            driver_files,
        }
    }

    /// The default discovery path is unchanged and is not claimed as observed.
    /// Explicit overrides must be resolvable before a certificate can be used.
    pub(crate) fn ensure_driver_files_verified(&self) -> Result<()> {
        self.driver_files.ensure_verified()
    }

    fn values(&self) -> (&'static [&'static str], &[Option<OsString>]) {
        match &self.scope {
            Scope::Cuda(values) => (&CUDA_KEYS, values),
            Scope::Vulkan(values) => (VULKAN_KEYS, values),
            Scope::Unscoped => (&[], &[]),
        }
    }

    pub(crate) fn digest(&self) -> String {
        let mut hash = Sha256::new();
        // Keep the existing CUDA and CPU/Metal/MLX byte protocol unchanged.
        hash.update(if matches!(self.scope, Scope::Vulkan(_)) {
            b"vulkan-selector-env-v2\0".as_slice()
        } else {
            b"cuda-selector-env-v1\0".as_slice()
        });
        if !matches!(self.scope, Scope::Unscoped) {
            hash.update([1]);
            let (keys, values) = self.values();
            for (key, value) in keys.iter().zip(values) {
                hash.update(key.as_bytes());
                match value {
                    None => hash.update([0]),
                    Some(value) => {
                        // This preserves non-Unicode OsString values. Never
                        // conflate absent, empty, or lossy display spellings.
                        let bytes = value.as_encoded_bytes();
                        hash.update([1]);
                        hash.update((bytes.len() as u64).to_le_bytes());
                        hash.update(bytes);
                    }
                }
            }
        } else {
            hash.update([0]);
        }
        if matches!(self.scope, Scope::Vulkan(_)) {
            self.driver_files.update_digest(&mut hash);
        }
        format!("{:x}", hash.finalize())
    }

    pub(crate) fn apply(&self, backend: Backend, command: &mut Command) -> Result<()> {
        ensure!(
            matches!(
                (&self.scope, backend),
                (Scope::Cuda(_), Backend::Cuda)
                    | (Scope::Vulkan(_), Backend::Vulkan)
                    | (
                        Scope::Unscoped,
                        Backend::Cpu | Backend::Metal | Backend::Mlx
                    )
            ),
            "Acceleration selector scope does not match backend"
        );
        {
            let (keys, values) = self.values();
            for (key, value) in keys.iter().zip(values) {
                match value {
                    Some(value) => command.env(key, value),
                    None => command.env_remove(key),
                };
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn cuda(visible: Option<OsString>, order: Option<OsString>) -> Self {
        Self {
            scope: Scope::Cuda([visible, order]),
            driver_files: DriverFiles::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn vulkan(values: &[(&str, Option<OsString>)]) -> Self {
        assert!(values.iter().all(|(key, _)| VULKAN_KEYS.contains(key)));
        Self::capture_from(Backend::Vulkan, |key| {
            values
                .iter()
                .find(|(name, _)| *name == key)
                .and_then(|(_, value)| value.clone())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relative_driver_library() -> &'static str {
        if cfg!(windows) {
            r".\driver.bin"
        } else {
            "./driver.bin"
        }
    }

    fn driver_json(library: &str, api_version: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "file_format_version":"1.0.0",
            "ICD":{"library_path":library,"api_version":api_version}
        }))
        .unwrap()
    }

    fn driver_fixture(root: &std::path::Path) -> Result<std::path::PathBuf> {
        let manifest = root.join("driver.json");
        std::fs::write(root.join("driver.bin"), b"driver-a")?;
        std::fs::write(&manifest, driver_json(relative_driver_library(), "1.2.0"))?;
        Ok(manifest)
    }

    fn driver_snapshot(key: &str, path: &std::path::Path) -> SelectorEnv {
        SelectorEnv::vulkan(&[(key, Some(path.as_os_str().to_owned()))])
    }

    #[test]
    fn same_path_vulkan_manifest_edits_change_the_snapshot() -> Result<()> {
        for key in ["VK_DRIVER_FILES", "VK_ICD_FILENAMES", "VK_ADD_DRIVER_FILES"] {
            let root = tempfile::tempdir()?;
            let manifest = driver_fixture(root.path())?;
            let before = driver_snapshot(key, &manifest);
            assert_eq!(before, driver_snapshot(key, &manifest));
            std::fs::write(&manifest, driver_json(relative_driver_library(), "1.3.0"))?;
            assert_ne!(before.digest(), driver_snapshot(key, &manifest).digest());
        }
        Ok(())
    }

    #[test]
    fn same_path_vulkan_driver_replacement_changes_the_snapshot() -> Result<()> {
        let root = tempfile::tempdir()?;
        let manifest = driver_fixture(root.path())?;
        let before = driver_snapshot("VK_DRIVER_FILES", &manifest);
        let library = root.path().join("driver.bin");
        let modified = std::fs::metadata(&library)?.modified()?;
        std::fs::remove_file(&library)?;
        let replacement = std::fs::File::create(&library)?;
        std::fs::write(&library, b"driver-b")?;
        replacement.set_times(std::fs::FileTimes::new().set_modified(modified))?;
        drop(replacement);
        let after = driver_snapshot("VK_DRIVER_FILES", &manifest);
        assert!(after.ensure_driver_files_verified().is_ok());
        assert_ne!(before.digest(), after.digest());
        Ok(())
    }

    #[test]
    fn missing_or_unresolvable_vulkan_drivers_cannot_reuse_a_snapshot() -> Result<()> {
        let root = tempfile::tempdir()?;
        let manifest = driver_fixture(root.path())?;
        let before = driver_snapshot("VK_DRIVER_FILES", &manifest);
        assert!(before.ensure_driver_files_verified().is_ok());
        std::fs::remove_file(&manifest)?;
        let missing = driver_snapshot("VK_DRIVER_FILES", &manifest);
        assert_ne!(before.digest(), missing.digest());
        assert!(missing.ensure_driver_files_verified().is_err());
        assert_eq!(missing, driver_snapshot("VK_DRIVER_FILES", &manifest));
        std::fs::write(&manifest, br#"{"ICD":{"library_path":"driver.bin"}}"#)?;
        let unresolved = driver_snapshot("VK_DRIVER_FILES", &manifest);
        assert!(unresolved.ensure_driver_files_verified().is_err());
        assert_eq!(unresolved, driver_snapshot("VK_DRIVER_FILES", &manifest));
        let error = unresolved
            .ensure_driver_files_verified()
            .unwrap_err()
            .to_string();
        assert!(!error.contains(root.path().to_str().unwrap()));
        assert!(!error.contains("driver.bin"));
        Ok(())
    }

    #[test]
    fn explicit_vulkan_precedence_and_unobserved_default_are_distinct() -> Result<()> {
        let root = tempfile::tempdir()?;
        let manifest = driver_fixture(root.path())?;
        for selected in ["VK_DRIVER_FILES", "VK_ICD_FILENAMES"] {
            let mut values = vec![("VK_ADD_DRIVER_FILES", Some("missing.json".into()))];
            if selected == "VK_DRIVER_FILES" {
                values.push(("VK_ICD_FILENAMES", Some("also-missing.json".into())));
            }
            values.push((selected, Some(manifest.as_os_str().to_owned())));
            assert!(SelectorEnv::vulkan(&values)
                .ensure_driver_files_verified()
                .is_ok());
        }
        let empty_override = SelectorEnv::vulkan(&[
            ("VK_DRIVER_FILES", Some(OsString::new())),
            ("VK_ICD_FILENAMES", Some(manifest.as_os_str().to_owned())),
        ]);
        assert!(empty_override.ensure_driver_files_verified().is_err());
        assert!(
            driver_snapshot("VK_DRIVER_FILES", std::path::Path::new("relative.json"))
                .ensure_driver_files_verified()
                .is_err()
        );
        assert!(SelectorEnv::vulkan(&[])
            .ensure_driver_files_verified()
            .is_ok());
        for backend in [Backend::Cpu, Backend::Metal, Backend::Mlx, Backend::Cuda] {
            assert!(SelectorEnv::capture(backend)
                .ensure_driver_files_verified()
                .is_ok());
        }
        Ok(())
    }

    #[test]
    fn explicit_vulkan_directory_membership_and_removed_library_are_observed() -> Result<()> {
        let root = tempfile::tempdir()?;
        let manifest = driver_fixture(root.path())?;
        let before = driver_snapshot("VK_DRIVER_FILES", root.path());
        assert!(before.ensure_driver_files_verified().is_ok());
        assert_eq!(before, driver_snapshot("VK_DRIVER_FILES", root.path()));
        let added = root.path().join("second.json");
        std::fs::copy(&manifest, &added)?;
        let changed = driver_snapshot("VK_DRIVER_FILES", root.path());
        assert!(changed.ensure_driver_files_verified().is_ok());
        assert_ne!(before, changed);
        std::fs::remove_file(root.path().join("driver.bin"))?;
        assert!(driver_snapshot("VK_DRIVER_FILES", root.path())
            .ensure_driver_files_verified()
            .is_err());
        Ok(())
    }

    #[test]
    fn explicit_vulkan_manifest_and_path_limits_fail_without_guessing() -> Result<()> {
        let root = tempfile::tempdir()?;
        let manifest = driver_fixture(root.path())?;
        std::fs::write(&manifest, vec![b' '; 64 * 1024 + 1])?;
        assert!(driver_snapshot("VK_DRIVER_FILES", &manifest)
            .ensure_driver_files_verified()
            .is_err());
        let many = std::env::join_paths(std::iter::repeat_n(manifest.as_path(), 17))?;
        assert!(SelectorEnv::vulkan(&[("VK_DRIVER_FILES", Some(many))])
            .ensure_driver_files_verified()
            .is_err());
        Ok(())
    }

    #[test]
    fn explicit_vulkan_duplicate_or_native_shadow_fields_are_unresolved() -> Result<()> {
        let root = tempfile::tempdir()?;
        let manifest = driver_fixture(root.path())?;
        let library = serde_json::to_string(relative_driver_library())?;
        let driver = format!(r#"{{"library_path":{library}}}"#);
        for input in [
            format!(r#"{{"ICD":{driver},"ICD":{driver}}}"#),
            format!(r#"{{"ICD":{{"library_path":{library},"library_path":{library}}}}}"#),
            format!(r#"{{"icd":{driver},"ICD":{driver}}}"#),
            format!(r#"{{"ICD":{{"LIBRARY_PATH":{library},"library_path":{library}}}}}"#),
            format!(r#"{{"ICD\u0000shadow":{driver},"ICD":{driver}}}"#),
            format!(
                r#"{{"ICD":{{"library_path\u0000shadow":{library},"library_path":{library}}}}}"#
            ),
        ] {
            std::fs::write(&manifest, input)?;
            assert!(driver_snapshot("VK_DRIVER_FILES", &manifest)
                .ensure_driver_files_verified()
                .is_err());
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn explicit_vulkan_library_symlink_retarget_is_observed() -> Result<()> {
        let root = tempfile::tempdir()?;
        let manifest = driver_fixture(root.path())?;
        let library = root.path().join("driver.bin");
        std::fs::rename(&library, root.path().join("first.bin"))?;
        std::os::unix::fs::symlink("first.bin", &library)?;
        let before = driver_snapshot("VK_DRIVER_FILES", &manifest);
        assert!(before.ensure_driver_files_verified().is_ok());
        std::fs::write(root.path().join("second.bin"), b"driver-b")?;
        std::fs::remove_file(&library)?;
        std::os::unix::fs::symlink("second.bin", &library)?;
        assert_ne!(before, driver_snapshot("VK_DRIVER_FILES", &manifest));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "owned FIFO fixture child; never loads a driver"]
    fn vulkan_fifo_probe_child() -> Result<()> {
        let root =
            std::path::PathBuf::from(std::env::var_os("OMASCRIBE_VULKAN_FIFO_PROBE").unwrap());
        for path in [root.join("blocked.json"), root.join("library.json"), root] {
            assert!(driver_snapshot("VK_DRIVER_FILES", &path)
                .ensure_driver_files_verified()
                .is_err());
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn explicit_vulkan_fifos_fail_in_a_bounded_owned_probe() -> Result<()> {
        use std::os::unix::ffi::OsStrExt;
        let root = tempfile::tempdir()?;
        for name in ["blocked.json", "blocked.bin"] {
            let path = root.path().join(name);
            let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
            // Both FIFO paths belong only to this temporary test directory.
            ensure!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) } == 0);
        }
        std::fs::write(
            root.path().join("library.json"),
            br#"{"ICD":{"library_path":"./blocked.bin"}}"#,
        )?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args([
                "--exact",
                "inference::selector_env::tests::vulkan_fifo_probe_child",
                "--ignored",
                "--nocapture",
            ])
            .env("OMASCRIBE_VULKAN_FIFO_PROBE", root.path());
        let output =
            crate::probe::capture(&mut command, std::time::Duration::from_secs(5), 16 * 1024)?;
        ensure!(String::from_utf8(output)?.contains("1 passed"));
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn explicit_vulkan_windows_relative_slash_only_library_is_unresolved() -> Result<()> {
        let root = tempfile::tempdir()?;
        let manifest = driver_fixture(root.path())?;
        assert!(driver_snapshot("VK_DRIVER_FILES", &manifest)
            .ensure_driver_files_verified()
            .is_ok());
        std::fs::write(&manifest, driver_json("./driver.bin", "1.2.0"))?;
        assert!(driver_snapshot("VK_DRIVER_FILES", &manifest)
            .ensure_driver_files_verified()
            .is_err());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn explicit_vulkan_windows_open_writer_cannot_reuse_a_snapshot() -> Result<()> {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        let root = tempfile::tempdir()?;
        let manifest = driver_fixture(root.path())?;
        let before = driver_snapshot("VK_DRIVER_FILES", &manifest);
        assert!(before.ensure_driver_files_verified().is_ok());
        for path in [root.path().join("driver.bin"), manifest.clone()] {
            let writer = std::fs::OpenOptions::new()
                .write(true)
                .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
                .open(path)?;
            assert!(driver_snapshot("VK_DRIVER_FILES", &manifest)
                .ensure_driver_files_verified()
                .is_err());
            drop(writer);
            assert!(driver_snapshot("VK_DRIVER_FILES", &manifest)
                .ensure_driver_files_verified()
                .is_ok());
        }
        Ok(())
    }

    #[test]
    fn selectors_distinguish_unset_empty_order_and_non_unicode_without_logging_values() {
        let absent = SelectorEnv::cuda(None, None);
        let empty = SelectorEnv::cuda(Some(OsString::new()), None);
        let ordered = SelectorEnv::cuda(Some("0,1".into()), None);
        assert_ne!(absent.digest(), empty.digest());
        assert_ne!(
            ordered.digest(),
            SelectorEnv::cuda(Some("1,0".into()), None).digest()
        );
        assert_ne!(
            ordered.digest(),
            SelectorEnv::cuda(Some("0,1".into()), Some("PCI_BUS_ID".into())).digest()
        );
        assert_eq!(ordered.digest(), ordered.clone().digest());
        assert!(!format!("{ordered:?}").contains("0,1"));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            assert_ne!(
                SelectorEnv::cuda(Some(OsString::from_vec(vec![0xff])), None).digest(),
                SelectorEnv::cuda(Some(OsString::from_vec(vec![0xfe])), None).digest()
            );
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            assert_ne!(
                SelectorEnv::cuda(Some(OsString::from_wide(&[0xd800])), None).digest(),
                SelectorEnv::cuda(Some(OsString::from_wide(&[0xd801])), None).digest()
            );
        }
    }

    #[test]
    fn command_explicitly_sets_or_removes_every_captured_cuda_selector() -> Result<()> {
        let values = SelectorEnv::cuda(Some(OsString::new()), None);
        let mut command = Command::new("never executed");
        // An inherited or intervening command override cannot replace the
        // captured empty selector or resurrect a captured absence.
        command
            .env("CUDA_VISIBLE_DEVICES", "later")
            .env("CUDA_DEVICE_ORDER", "later");
        values.apply(Backend::Cuda, &mut command)?;
        let env: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        assert_eq!(
            env[std::ffi::OsStr::new("CUDA_VISIBLE_DEVICES")],
            Some(std::ffi::OsStr::new(""))
        );
        assert_eq!(env[std::ffi::OsStr::new("CUDA_DEVICE_ORDER")], None);
        assert!(values.apply(Backend::Vulkan, &mut command).is_err());
        for backend in [Backend::Cpu, Backend::Metal, Backend::Mlx] {
            assert!(values.apply(backend, &mut command).is_err());
            let none = SelectorEnv::capture(backend);
            let mut other = Command::new("never executed");
            none.apply(backend, &mut other)?;
            assert!(other.get_envs().next().is_none());
            assert!(none.apply(Backend::Cuda, &mut other).is_err());
        }
        Ok(())
    }

    #[test]
    fn extending_vulkan_keeps_existing_cuda_and_unscoped_digests() {
        for backend in [Backend::Cpu, Backend::Metal, Backend::Mlx] {
            assert_eq!(
                SelectorEnv::capture(backend).digest(),
                "49abdedf72ef7a04d145c4cebd7074f936afbbc2cb26fc0e2d98872a7d6ee708"
            );
        }
        assert_eq!(
            SelectorEnv::cuda(None, None).digest(),
            "0d528361268a035ac11ae615c47b98345f8de1cbfccc7d511c94c19d8eac62d7"
        );
        assert_eq!(
            SelectorEnv::cuda(Some("0,1".into()), None).digest(),
            "0fcf0f539d4d97db805bd398ec198c02bf08a49985d12a956a37c08ef25c5dc9"
        );
        let vulkan = SelectorEnv::vulkan(&[]);
        assert_ne!(vulkan.digest(), SelectorEnv::capture(Backend::Cpu).digest());
        assert_ne!(vulkan.digest(), SelectorEnv::cuda(None, None).digest());
    }

    #[test]
    fn every_fixed_vulkan_key_binds_unset_empty_and_value_without_reading_paths() {
        let none = SelectorEnv::vulkan(&[]);
        let keys: std::collections::HashSet<_> = VULKAN_KEYS.iter().copied().collect();
        assert_eq!(keys.len(), VULKAN_KEYS.len());
        #[cfg(target_os = "linux")]
        assert_eq!(keys.len(), 24);
        #[cfg(not(target_os = "linux"))]
        assert_eq!(keys.len(), 17);
        for key in VULKAN_KEYS {
            let empty = SelectorEnv::vulkan(&[(key, Some(OsString::new()))]);
            let value =
                SelectorEnv::vulkan(&[(key, Some("nonexistent/opaque:selection;input".into()))]);
            assert_ne!(none.digest(), empty.digest(), "{key}");
            assert_ne!(empty.digest(), value.digest(), "{key}");
            assert_eq!(value.digest(), value.clone().digest(), "{key}");
            assert!(!format!("{value:?}").contains("nonexistent"));
        }
        let original = SelectorEnv::capture_from(Backend::Vulkan, |_| None);
        let unrelated = SelectorEnv::capture_from(Backend::Vulkan, |key| {
            (key == "UNRELATED_UI_THEME").then(|| "dark".into())
        });
        assert_eq!(original, unrelated);
    }

    #[test]
    fn vulkan_command_freezes_all_selectors_without_leaking_into_cpu_or_cuda() -> Result<()> {
        let values = SelectorEnv::vulkan(&[
            ("GGML_VK_VISIBLE_DEVICES", Some(OsString::new())),
            ("VK_DRIVER_FILES", Some("opaque:one.json;two.json".into())),
            ("VK_ICD_FILENAMES", Some("also-preserved.json".into())),
            ("DRI_PRIME", Some("1!".into())),
        ]);
        let mut command = Command::new("never executed");
        for key in VULKAN_KEYS {
            command.env(key, "later override");
        }
        values.apply(Backend::Vulkan, &mut command)?;
        let env: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        assert_eq!(env.len(), VULKAN_KEYS.len());
        let (keys, captured) = values.values();
        for (key, value) in keys.iter().zip(captured) {
            assert_eq!(env[std::ffi::OsStr::new(key)], value.as_deref(), "{key}");
        }
        for backend in [Backend::Cpu, Backend::Metal, Backend::Mlx, Backend::Cuda] {
            assert!(values.apply(backend, &mut command).is_err());
        }
        assert!(SelectorEnv::capture(Backend::Cpu)
            .apply(Backend::Vulkan, &mut command)
            .is_err());
        assert!(SelectorEnv::cuda(None, None)
            .apply(Backend::Vulkan, &mut command)
            .is_err());
        Ok(())
    }
}
