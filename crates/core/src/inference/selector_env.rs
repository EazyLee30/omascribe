//! Bind CUDA ordinals to the exact selector values passed to a child.
//! Vulkan loader and userspace driver identity are separate, unfinished work.
use super::Backend;
use anyhow::{ensure, Result};
use sha2::{Digest, Sha256};
use std::{ffi::OsString, fmt, process::Command};

const CUDA_KEYS: [&str; 2] = ["CUDA_VISIBLE_DEVICES", "CUDA_DEVICE_ORDER"];

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SelectorEnv {
    cuda: Option<[Option<OsString>; 2]>,
}

impl fmt::Debug for SelectorEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Environment values are private launch inputs, not diagnostic text.
        f.debug_struct("SelectorEnv")
            .field("cuda", &self.cuda.is_some())
            .field("digest", &self.digest())
            .finish()
    }
}

impl SelectorEnv {
    pub(crate) fn capture(backend: Backend) -> Self {
        Self {
            cuda: (backend == Backend::Cuda).then(|| CUDA_KEYS.map(std::env::var_os)),
        }
    }

    pub(crate) fn digest(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(b"cuda-selector-env-v1\0");
        if let Some(values) = &self.cuda {
            hash.update([1]);
            for (key, value) in CUDA_KEYS.iter().zip(values) {
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
            self.cuda.is_some() == (backend == Backend::Cuda),
            "Acceleration selector scope does not match backend"
        );
        if let Some(values) = &self.cuda {
            for (key, value) in CUDA_KEYS.iter().zip(values) {
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
            cuda: Some([visible, order]),
        }
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
        for backend in [Backend::Cpu, Backend::Metal, Backend::Mlx, Backend::Vulkan] {
            assert!(values.apply(backend, &mut command).is_err());
            let none = SelectorEnv::capture(backend);
            let mut other = Command::new("never executed");
            none.apply(backend, &mut other)?;
            assert!(other.get_envs().next().is_none());
            assert!(none.apply(Backend::Cuda, &mut other).is_err());
        }
        Ok(())
    }
}
