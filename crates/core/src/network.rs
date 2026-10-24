//! Device-local network permissions. Never serialize these into a shared Profile.
use crate::Engine;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
    sync::Mutex,
};

const MAX_POLICY_BYTES: u64 = 4096;
static POLICY_IO: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkPolicy {
    pub allow_model_downloads: bool,
    pub allow_online_recommendations: bool,
}

impl NetworkPolicy {
    fn load(root: &Path) -> Result<Self> {
        let path = root.join("network-policy.json");
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(error.into()),
        };
        let mut bytes = Vec::new();
        file.take(MAX_POLICY_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_POLICY_BYTES {
            bail!("Invalid network policy: file exceeds 4 KiB");
        }
        serde_json::from_slice(&bytes)
            .context("Invalid network policy; online actions remain blocked")
    }

    fn save(&self, root: &Path) -> Result<()> {
        let tmp = root.join(format!(".network-policy-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&tmp)?;
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&tmp, root.join("network-policy.json"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
}

impl Engine {
    pub fn get_network_policy(&self) -> Result<NetworkPolicy> {
        let _io = POLICY_IO.lock().unwrap();
        NetworkPolicy::load(&self.root)
    }

    pub fn set_network_policy(&self, policy: NetworkPolicy) -> Result<NetworkPolicy> {
        // Match download_model's lock order: configuration → policy I/O → jobs.
        // A new job cannot register between saving "off" and cancelling jobs.
        let _gate = self.config_lock.lock().unwrap();
        let _io = POLICY_IO.lock().unwrap();
        policy.save(&self.root)?;
        if !policy.allow_model_downloads {
            self.cancel_all_downloads();
        }
        Ok(policy)
    }

    pub fn require_model_network(&self) -> Result<()> {
        if !self.get_network_policy()?.allow_model_downloads {
            bail!("network:model-downloads-disabled");
        }
        Ok(())
    }

    pub fn require_online_recommendations(&self) -> Result<()> {
        if !self.get_network_policy()?.allow_online_recommendations {
            bail!("network:recommendations-disabled");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_features_are_separately_opted_in_and_persist_on_this_device() {
        let root = tempfile::tempdir().unwrap();
        let engine = Engine::open(root.path().into()).unwrap();
        assert_eq!(
            engine.get_network_policy().unwrap(),
            NetworkPolicy::default()
        );
        assert!(engine.require_model_network().is_err());
        assert!(engine.require_online_recommendations().is_err());
        let model_only = NetworkPolicy {
            allow_model_downloads: true,
            ..Default::default()
        };
        engine.set_network_policy(model_only.clone()).unwrap();
        assert!(engine.require_model_network().is_ok());
        assert!(engine.require_online_recommendations().is_err());
        drop(engine);
        let reopened = Engine::open(root.path().into()).unwrap();
        assert_eq!(reopened.get_network_policy().unwrap(), model_only);
        reopened
            .set_network_policy(NetworkPolicy::default())
            .unwrap();
        assert!(reopened.require_model_network().is_err());
        assert!(!root.path().join("preferences.json").exists());
    }

    #[test]
    fn corrupt_or_expanded_policy_cannot_grant_network_access() {
        let root = tempfile::tempdir().unwrap();
        let engine = Engine::open(root.path().into()).unwrap();
        for bytes in [
            b"not json".to_vec(),
            br#"{"allow_model_downloads":true,"allow_uploads":true}"#.to_vec(),
            br#"{"allow_model_downloads":"yes"}"#.to_vec(),
            vec![b' '; MAX_POLICY_BYTES as usize + 1],
        ] {
            fs::write(root.path().join("network-policy.json"), bytes).unwrap();
            assert!(engine.require_model_network().is_err());
            assert!(engine.require_online_recommendations().is_err());
        }
    }

    #[test]
    fn shared_configuration_cannot_enable_network_permissions() {
        let root = tempfile::tempdir().unwrap();
        let engine = Engine::open(root.path().into()).unwrap();
        let mut profile = serde_json::json!({
            "schema":1,"name":"Shared setup","hardware_hint":"","preferences":{}
        });
        profile["preferences"]["allow_model_downloads"] = true.into();
        assert!(
            crate::preferences::Profile::parse(&serde_json::to_vec(&profile).unwrap()).is_err()
        );
        profile["preferences"] = serde_json::json!({});
        profile["network_policy"] = serde_json::json!({"allow_model_downloads":true});
        assert!(
            crate::preferences::Profile::parse(&serde_json::to_vec(&profile).unwrap()).is_err()
        );
        let preferences = crate::preferences::Preferences::default();
        engine.update_preferences(preferences).unwrap();
        assert!(engine.require_model_network().is_err());
    }
}
