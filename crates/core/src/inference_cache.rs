//! Small runtime fingerprint cache. Contains hashes/metadata, never prompt or meeting text.
use crate::{preferences::Preferences, store::Store, Engine, Meeting};
use anyhow::Result;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};

#[derive(Default)]
pub(crate) struct Fingerprints(Mutex<HashMap<PathBuf, (u64, SystemTime, String)>>);
impl Fingerprints {
    pub(crate) fn file(&self, path: &Path) -> Option<String> {
        let metadata = fs::metadata(path).ok()?;
        if !metadata.is_file() {
            return None;
        }
        let modified = metadata.modified().ok()?;
        let mut cache = self.0.lock().unwrap();
        if let Some((bytes, time, hash)) = cache.get(path) {
            if *bytes == metadata.len() && *time == modified {
                return Some(hash.clone());
            }
        }
        let mut input = fs::File::open(path).ok()?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let n = input.read(&mut buffer).ok()?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        let after = input.metadata().ok()?;
        if after.len() != metadata.len() || after.modified().ok()? != modified {
            return None;
        }
        let digest = format!("{:x}", hasher.finalize());
        if cache.len() >= 16 {
            cache.clear();
        }
        cache.insert(path.to_owned(), (metadata.len(), modified, digest.clone()));
        Some(digest)
    }
}
fn stamp(path: &Path) -> Option<Value> {
    let metadata = fs::metadata(path).ok()?;
    Some(json!([
        metadata.len(),
        metadata
            .modified()
            .ok()?
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_nanos()
            .to_string()
    ]))
}
pub(crate) fn environment(
    engine: &Engine,
    db: &Store,
    meeting: &Meeting,
    prefs: &Preferences,
) -> Result<String> {
    let model = crate::models::model(&prefs.llm_model)?;
    let directory = engine.model_directory(&model.id).ok();
    let assets: Vec<_> = model
        .files
        .iter()
        .map(|asset| {
            json!([
                asset.name,
                asset.sha256,
                directory.as_ref().and_then(|p| stamp(&p.join(&asset.name)))
            ])
        })
        .collect();
    let runtime = if model.format == "mlx" {
        engine.runtime_dir.join("mlx/runner.py")
    } else {
        engine
            .runtime_dir
            .join("llama")
            .join(crate::platform::executable("llama-server"))
    };
    let component = if prefs.inference_backend == "auto" {
        let mut bytes = Vec::new();
        fs::File::open(engine.root.join("accelerators/active.json"))
            .ok()
            .and_then(|file| {
                file.take(16_385).read_to_end(&mut bytes).ok()?;
                if bytes.len() > 16_384 {
                    return None;
                }
                let value: Value = serde_json::from_slice(&bytes).ok()?;
                value["binding"]
                    .as_str()
                    .filter(|s| s.len() == 64)
                    .map(str::to_owned)
            })
    } else {
        None
    };
    let input = json!({"schema":"translation-cache-v8-checked-name-reference-grammar","model":model.id,"assets":assets,
        "runtime":engine.inference_fingerprints.file(&runtime),"component":component,
        "template":crate::translation::SYSTEM_TEMPLATE,"output_schema":crate::translation::OUTPUT_SCHEMA,
        "generation":{"seed":42,"temperature":0,"max_tokens":crate::translation::chunks::OUTPUT_TOKENS,"thinking":false,"input_budget":crate::translation::chunks::INPUT_TOKENS},
        "configuration":{"context":prefs.context_size,"threads":prefs.threads,"backend":prefs.inference_backend,"mode":prefs.resource_mode,"memory_budget":prefs.memory_budget_mib},
        "permissions":{"knowledge":prefs.use_knowledge,"memory":prefs.use_memory},
        "scope":{"meeting":meeting.id,"run":meeting.run,"terms":db.cache_dependency(&meeting.id,"terms")?,"speakers":meeting.speaker_names}});
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&input)?)))
}
pub(crate) fn translation_key(environment: &str, source: &crate::Segment, target: &str) -> String {
    let input = json!([
        environment,
        source.id,
        source.meeting_id,
        source.run,
        source.revision,
        source.text,
        source.speaker,
        source.speaker_manual,
        target
    ]);
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&input).expect("translation cache identity"))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_fingerprint_changes_when_executable_changes() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("runtime-fixture");
        let cache = Fingerprints::default();
        fs::write(&path, b"first executable")?;
        let first = cache.file(&path).unwrap();
        assert_eq!(cache.file(&path).as_ref(), Some(&first));
        fs::write(&path, b"different executable bytes")?;
        assert_ne!(cache.file(&path).as_ref(), Some(&first));
        fs::remove_file(&path)?;
        assert!(cache.file(&path).is_none());
        Ok(())
    }
}
