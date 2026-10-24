//! A deep link is untrusted configuration data, never an instruction to apply it.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use omascribe_core::preferences::Profile;
use std::sync::Mutex;
use tauri::{Emitter, Manager, State};

#[derive(Default)]
pub struct PendingProfile(Mutex<Option<Result<Profile, String>>>);

pub fn parse_link(link: &str) -> Result<Profile, String> {
    if link.len() > 16 * 1024 {
        return Err("Configuration link is too large; use a JSON file instead".into());
    }
    let encoded = link
        .strip_prefix("omascribe://profile?data=")
        .ok_or("Unsupported configuration link")?;
    if encoded.is_empty()
        || !encoded
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("Invalid configuration payload".into());
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| "Invalid configuration encoding")?;
    Profile::parse(&bytes).map_err(|e| e.to_string())
}

pub fn receive(app: &tauri::AppHandle, link: &str) {
    // Only a bounded local preview is queued. No request, model load, or preference write.
    *app.state::<PendingProfile>().0.lock().unwrap() = Some(parse_link(link));
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.emit("profile-import-available", ());
    }
}

#[tauri::command]
pub fn take_pending_profile(state: State<PendingProfile>) -> Result<Option<Profile>, String> {
    state.0.lock().unwrap().take().transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn link(value: serde_json::Value) -> String {
        format!(
            "omascribe://profile?data={}",
            URL_SAFE_NO_PAD.encode(value.to_string())
        )
    }
    #[test]
    fn accepts_only_data_and_never_urls_paths_or_network_permissions() {
        let value = serde_json::json!({"schema":1,"name":"M 系列配置","hardware_hint":"Apple Silicon","preferences":{}});
        assert_eq!(parse_link(&link(value.clone())).unwrap().name, "M 系列配置");
        for malicious in [
            serde_json::json!({"llm_model":"../../model"}),
            serde_json::json!({"url":"https://example.com/model"}),
            serde_json::json!({"allow_model_downloads":true}),
            serde_json::json!({"command":"do something"}),
        ] {
            let mut p = value.clone();
            p["preferences"] = malicious;
            assert!(parse_link(&link(p)).is_err());
        }
        for bad in [
            "https://example.com/profile",
            "omascribe://record?data=e30",
            "omascribe://profile?data=e30&apply=true",
            "omascribe://profile?url=https://example.com",
            "omascribe://profile?data=e30#x",
        ] {
            assert!(parse_link(bad).is_err());
        }
        assert!(parse_link(&format!("omascribe://profile?data={}", "a".repeat(17000))).is_err());
    }
}
