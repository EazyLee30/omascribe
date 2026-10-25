use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    pub mode: String,
    pub theme: String,
    pub accent: String,
    pub background: String,
    pub foreground: String,
    pub font: String,
    pub font_size: u32,
}
impl Default for Appearance {
    fn default() -> Self {
        Self {
            mode: "system".into(),
            theme: "neutral".into(),
            accent: "#d83b36".into(),
            background: String::new(),
            foreground: String::new(),
            font: "system".into(),
            font_size: 13,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub appearance: Appearance,
    pub asr_model: String,
    pub llm_model: String,
    pub threads: u32,
    pub context_size: u32,
    pub inference_backend: String,
    pub resource_mode: String,
    pub memory_budget_mib: u64,
    pub auto_notes: bool,
    pub translation_mode: String,
    pub minutes_language: String,
    pub diarization: bool,
    pub speaker_count: u32,
    pub use_knowledge: bool,
    pub use_memory: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            appearance: Appearance::default(),
            asr_model: "zipformer-small".into(),
            llm_model: "qwen3-1.7b".into(),
            threads: 1,
            context_size: 2048,
            inference_backend: "auto".into(),
            resource_mode: "balanced".into(),
            memory_budget_mib: 0,
            auto_notes: true,
            translation_mode: "off".into(),
            minutes_language: "auto".into(),
            diarization: false,
            speaker_count: 0,
            use_knowledge: false,
            use_memory: false,
        }
    }
}
impl Preferences {
    pub fn validate(&self) -> Result<()> {
        if !["auto", "cpu", "metal", "mlx"].contains(&self.inference_backend.as_str())
            || !["eco", "balanced", "quality"].contains(&self.resource_mode.as_str())
            || (self.memory_budget_mib != 0 && !(1024..=32768).contains(&self.memory_budget_mib))
        {
            bail!("Invalid local inference resource settings");
        }
        if !["off", "bilingual", "zh-en", "en-zh"].contains(&self.translation_mode.as_str())
            || !["auto", "zh", "en"].contains(&self.minutes_language.as_str())
        {
            bail!("Invalid language setting");
        }
        let a = &self.appearance;
        if !["system", "light", "dark"].contains(&a.mode.as_str())
            || !["forest", "neutral", "slate"].contains(&a.theme.as_str())
            || !["system", "serif", "mono"].contains(&a.font.as_str())
        {
            bail!("Invalid appearance option");
        }
        for (color, optional) in [
            (&a.accent, false),
            (&a.background, true),
            (&a.foreground, true),
        ] {
            if !(optional && color.is_empty()
                || color.len() == 7
                    && color.starts_with('#')
                    && color[1..].bytes().all(|b| b.is_ascii_hexdigit()))
            {
                bail!("Use a six-digit hex color");
            }
        }
        if !(11..=20).contains(&a.font_size)
            || !(1..=16).contains(&self.threads)
            || ![2048, 4096, 8192].contains(&self.context_size)
            || self.speaker_count > 16
        {
            bail!("Setting outside supported range");
        }
        if crate::models::model(&self.asr_model)?.kind != "asr"
            || crate::models::model(&self.llm_model)?.kind != "llm"
        {
            bail!("Invalid model combination");
        }
        let mlx = crate::models::model(&self.llm_model)?.format == "mlx";
        if (mlx && !["auto", "mlx"].contains(&self.inference_backend.as_str()))
            || (!mlx && self.inference_backend == "mlx")
        {
            bail!("MLX requires an MLX model; CPU and Metal use GGUF models");
        }
        Ok(())
    }
    pub fn load(root: &Path) -> Result<Self> {
        let p = root.join("preferences.json");
        let prefs: Self = if p.exists() {
            serde_json::from_slice(&std::fs::read(p)?)?
        } else {
            Self::default()
        };
        prefs.validate()?;
        Ok(prefs)
    }
    pub fn save(&self, root: &Path) -> Result<()> {
        self.validate()?;
        atomic_json(&root.join("preferences.json"), self)
    }
}
pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let tmp = path.with_extension("json.part");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub schema: u32,
    pub name: String,
    pub hardware_hint: String,
    pub preferences: Preferences,
}
impl Profile {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > 64 * 1024 {
            bail!("Configuration exceeds 64 KiB");
        }
        let profile: Self = serde_json::from_slice(bytes)?;
        if profile.schema != 1 || profile.name.len() > 160 || profile.hardware_hint.len() > 300 {
            bail!("Unsupported configuration");
        }
        profile.preferences.validate()?;
        Ok(profile)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_cannot_supply_urls_paths_or_commands() {
        let p = Preferences {
            llm_model: "../../other".into(),
            ..Default::default()
        };
        assert!(p.validate().is_err());
        assert!(Profile::parse(
            br#"{"schema":1,"name":"x","hardware_hint":"","preferences":{},"command":"bad"}"#
        )
        .is_err());
    }
    #[test]
    fn defaults_and_custom_theme_round_trip() {
        let mut p = Preferences::default();
        p.appearance.mode = "light".into();
        p.appearance.background = "#ffffff".into();
        let root = tempfile::tempdir().unwrap();
        p.save(root.path()).unwrap();
        assert_eq!(Preferences::load(root.path()).unwrap(), p);
    }
}
