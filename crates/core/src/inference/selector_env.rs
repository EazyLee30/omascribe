//! Bind GPU selection to the exact selector values passed to a child.
//! Same-path manifest/library changes and unknown layer inputs remain unfinished.
use super::Backend;
use anyhow::{ensure, Result};
use sha2::{Digest, Sha256};
use std::{ffi::OsString, fmt, process::Command};

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
        Self {
            scope: match backend {
                Backend::Cuda => Scope::Cuda(CUDA_KEYS.map(lookup)),
                Backend::Vulkan => {
                    Scope::Vulkan(VULKAN_KEYS.iter().map(|key| lookup(key)).collect())
                }
                _ => Scope::Unscoped,
            },
        }
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
            b"vulkan-selector-env-v1\0".as_slice()
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
