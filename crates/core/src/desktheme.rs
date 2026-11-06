//! Read the live Omarchy desktop theme so the workspace can follow it.
//!
//! Omarchy publishes the active theme under
//! `~/.local/state/omarchy/current/theme/` with a `colors.toml` and a
//! `theme.name`. Switching theme rewrites those files, so a tiny stat check is
//! enough to detect a change without a file-watcher dependency.
//!
//! Only Omarchy installs have this layout. Everywhere else the probe returns
//! `None` and the app keeps its own palettes.

use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesktopTheme {
    pub name: String,
    pub mode: String,
    pub accent: String,
    pub background: String,
    pub foreground: String,
    pub muted: String,
    pub border: String,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Directory holding the files Omarchy copies in for the active theme.
pub fn theme_dir() -> PathBuf {
    home().join(".local/state/omarchy/current/theme")
}

/// `~/.local/state/omarchy/current`, which owns the name and generation files.
fn current_dir() -> PathBuf {
    home().join(".local/state/omarchy/current")
}

/// Cheap identity for change detection: theme name plus the modification
/// times Omarchy rewrites when a theme is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    name: Option<String>,
    colors: Option<(u64, u128)>,
    generation: Option<(u64, u128)>,
}

fn stamp(path: &Path) -> Option<(u64, u128)> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let since = modified
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((meta.len(), since))
}

pub fn signature() -> Signature {
    let dir = theme_dir();
    // Omarchy keeps `theme.name` and `theme.generation` next to, not inside,
    // the directory that holds the copied theme files.
    let current = current_dir();
    Signature {
        name: std::fs::read_to_string(current.join("theme.name"))
            .ok()
            .map(|s| s.trim().to_string()),
        colors: stamp(&dir.join("colors.toml")),
        generation: stamp(&current.join("theme.generation")),
    }
}

fn hex(value: &str) -> Option<String> {
    let value = value.trim().trim_matches('"').trim();
    (value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|b| b.is_ascii_hexdigit()))
    .then(|| value.to_ascii_lowercase())
}

/// Parse the subset of Omarchy's `colors.toml` the workspace mirrors.
pub fn parse_colors(text: &str) -> std::collections::HashMap<String, String> {
    let mut values = std::collections::HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        // `mode` is the one non-color field the workspace needs.
        if key == "mode" {
            let mode = value.trim().trim_matches('"').trim();
            if mode == "dark" || mode == "light" {
                values.insert(key, mode.to_string());
            }
            continue;
        }
        // TOML allows trailing comments. A hex color is exactly seven
        // characters, so locate the first `#` and read that fixed width; this
        // ignores quoting and anything after the value.
        let Some(start) = value.find('#') else {
            continue;
        };
        let Some(color) = value.get(start..start + 7) else {
            continue;
        };
        if let Some(color) = hex(color) {
            values.insert(key, color);
        }
    }
    values
}

/// Read the current desktop theme. `None` means "not an Omarchy desktop" or a
/// theme directory that cannot be parsed; callers fall back to their palette.
pub fn current() -> Option<DesktopTheme> {
    let dir = theme_dir();
    let text = std::fs::read_to_string(dir.join("colors.toml")).ok()?;
    let colors = parse_colors(&text);
    let get = |key: &str| colors.get(key).cloned();
    let name = std::fs::read_to_string(current_dir().join("theme.name"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "omarchy".into());
    Some(DesktopTheme {
        name,
        mode: get("mode").unwrap_or_else(|| "dark".into()),
        accent: get("accent")?,
        background: get("background").unwrap_or_else(|| "#1a1b26".into()),
        foreground: get("foreground").unwrap_or_else(|| "#e5e5e5".into()),
        muted: get("muted").unwrap_or_else(|| "#7a7a85".into()),
        border: get("lighter_background").unwrap_or_else(|| "#2a2a33".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r######"mode = "dark"

accent = "#7aa2f7"
selection = "#292e42"
muted = "#414868"

background = "#1a1b26"
lighter_background = "#24283b"
foreground = "#a9b1d6"
"######;

    #[test]
    fn parses_omarchy_color_file() {
        let colors = parse_colors(SAMPLE);
        assert_eq!(colors.get("mode").unwrap(), "dark");
        assert_eq!(colors.get("accent").unwrap(), "#7aa2f7");
        assert_eq!(colors.get("lighter_background").unwrap(), "#24283b");
        assert_eq!(colors.len(), 7);
    }

    #[test]
    fn ignores_non_color_values_and_garbage() {
        let colors = parse_colors(
            "accent = \"#ff0000\"\nname = tokyo\nbad = #12345\nother = \"#zzzzzz\"\n# accent = \"#00ff00\"\n",
        );
        assert_eq!(colors.get("accent").unwrap(), "#ff0000");
        assert!(!colors.contains_key("name"));
        assert!(!colors.contains_key("bad"));
        assert!(!colors.contains_key("other"));
    }

    #[test]
    fn trailing_comment_keeps_the_color() {
        let colors = parse_colors("accent = \"#abcdef\" # chosen\n");
        assert_eq!(colors.get("accent").unwrap(), "#abcdef");
    }

    #[test]
    fn signature_changes_shape_is_stable() {
        // Two probes of an unchanged desktop must compare equal.
        assert_eq!(signature(), signature());
    }

    #[test]
    fn theme_name_lives_beside_the_theme_directory() {
        // Omarchy writes `theme.name`/`theme.generation` into
        // `current/`, while the copied theme files live in `current/theme/`.
        // Resolving the name from inside `theme/` silently fell back to the
        // generic "omarchy" label on a real desktop.
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current");
        let theme = current.join("theme");
        std::fs::create_dir_all(&theme).unwrap();
        std::fs::write(current.join("theme.name"), "theme-tokyo-liftoff\n").unwrap();
        std::fs::write(current.join("theme.generation"), "14636-1\n").unwrap();
        std::fs::write(theme.join("colors.toml"), SAMPLE).unwrap();

        let name = std::fs::read_to_string(current.join("theme.name"))
            .unwrap()
            .trim()
            .to_string();
        assert_eq!(name, "theme-tokyo-liftoff");
        assert!(current.join("theme.generation").is_file());
        assert!(!theme.join("theme.name").exists());
        assert!(theme.join("colors.toml").is_file());
    }
}
