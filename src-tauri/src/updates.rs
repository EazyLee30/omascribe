//! Signed, user-initiated software updates. No meeting content enters these requests.
use crate::{err, AppState, ExitPermit, Reply};
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use std::time::Duration;
use tauri::{Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Clone, Default, Serialize)]
pub struct Status {
    pub phase: String,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
    pub supported: bool,
}
#[derive(Default)]
pub struct Updates {
    status: Mutex<Status>,
    pending: Mutex<Option<Update>>,
    operation: tokio::sync::Mutex<()>,
    cancel: AtomicBool,
}
fn supported() -> bool {
    if cfg!(target_os = "linux") {
        std::env::var_os("APPIMAGE").is_some()
    } else {
        !cfg!(debug_assertions)
    }
}
fn allowed(engine: &AppState) -> Reply<()> {
    if !engine
        .get_network_policy()
        .map_err(err)?
        .allow_update_checks
    {
        return Err("update:network-disabled".into());
    }
    if !supported() {
        return Err("update:package-manager".into());
    }
    Ok(())
}
fn trusted_url(url: &tauri::Url, version: &str) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.path().starts_with(&format!(
            "/EazyLee30/omascribe/releases/download/v{version}/OmaScribe_{version}_"
        ))
        && !version.is_empty()
        && version.bytes().all(|c| c.is_ascii_digit() || c == b'.')
}
fn fail(state: &Updates, error: String) -> String {
    let mut status = state.status.lock().unwrap();
    status.phase = "error".into();
    status.error = Some(error.clone());
    error
}
async fn cancelled(engine: &AppState, state: &Updates) {
    loop {
        if state.cancel.load(Ordering::SeqCst)
            || !engine
                .get_network_policy()
                .map(|p| p.allow_update_checks)
                .unwrap_or(false)
            || state.status.lock().unwrap().downloaded > 3_000_000_000
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
#[tauri::command]
pub fn update_status(state: State<Updates>) -> Status {
    let mut s = state.status.lock().unwrap().clone();
    s.supported = supported();
    s
}
#[tauri::command]
pub fn cancel_update(state: State<Updates>) {
    state.cancel.store(true, Ordering::SeqCst);
}
#[tauri::command]
pub async fn check_update(
    app: tauri::AppHandle,
    engine: State<'_, AppState>,
    state: State<'_, Updates>,
) -> Reply<Status> {
    let _operation = state.operation.try_lock().map_err(|_| "update:busy")?;
    allowed(&engine)?;
    state.cancel.store(false, Ordering::SeqCst);
    *state.pending.lock().unwrap() = None;
    *state.status.lock().unwrap() = Status {
        phase: "checking".into(),
        supported: supported(),
        ..Default::default()
    };
    let updater = app
        .updater_builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| fail(&state, err(e)))?;
    let result = tokio::select! {
        result = updater.check() => result.map_err(err),
        _ = cancelled(&engine, &state) => Err("update:cancelled".into()),
    };
    let update = result.map_err(|e| fail(&state, e))?;
    if let Some(update) = update {
        if !trusted_url(&update.download_url, &update.version) || update.signature.is_empty() {
            return Err(fail(&state, "update:untrusted-release".into()));
        }
        *state.status.lock().unwrap() = Status {
            phase: "available".into(),
            version: Some(update.version.clone()),
            notes: update.body.clone(),
            supported: true,
            ..Default::default()
        };
        *state.pending.lock().unwrap() = Some(update);
    } else {
        state.status.lock().unwrap().phase = "current".into();
    }
    let result = state.status.lock().unwrap().clone();
    Ok(result)
}
#[tauri::command]
pub async fn download_update(engine: State<'_, AppState>, state: State<'_, Updates>) -> Reply<()> {
    let _operation = state.operation.try_lock().map_err(|_| "update:busy")?;
    allowed(&engine)?;
    let mut update = state
        .pending
        .lock()
        .unwrap()
        .clone()
        .ok_or("update:check-first")?;
    update.timeout = Some(Duration::from_secs(3600));
    state.cancel.store(false, Ordering::SeqCst);
    {
        let mut s = state.status.lock().unwrap();
        s.phase = "downloading".into();
        s.downloaded = 0;
        s.total = None;
        s.error = None;
    }
    let result = tokio::select! {
        result = update.download(|count, total| {
            let mut s = state.status.lock().unwrap();
            s.downloaded += count as u64; s.total = total;
        }, || { state.status.lock().unwrap().phase = "verifying".into(); }) => result.map_err(err),
        _ = cancelled(&engine, &state) => Err("update:cancelled".into()),
    };
    let bytes = result.map_err(|e| fail(&state, e))?;
    allowed(&engine).map_err(|e| fail(&state, e))?;
    let root = engine.root.clone();
    tauri::async_runtime::spawn_blocking(move || -> Reply<()> {
        let dir = root.join("updates");
        std::fs::create_dir_all(&dir).map_err(err)?;
        std::fs::write(dir.join("verified-update.bin"), bytes).map_err(err)
    })
    .await
    .map_err(|e| fail(&state, err(e)))?
    .map_err(|e| fail(&state, e))?;
    state.status.lock().unwrap().phase = "ready".into();
    Ok(())
}
#[tauri::command]
pub async fn install_update(
    app: tauri::AppHandle,
    engine: State<'_, AppState>,
    state: State<'_, Updates>,
) -> Reply<()> {
    let _operation = state.operation.try_lock().map_err(|_| "update:busy")?;
    allowed(&engine)?;
    if state.status.lock().unwrap().phase != "ready" {
        return Err("update:download-first".into());
    }
    let update = state
        .pending
        .lock()
        .unwrap()
        .clone()
        .ok_or("update:check-first")?;
    {
        let _gate = engine.config_lock.lock().unwrap();
        let busy = engine.active.lock().unwrap().as_ref().is_some_and(|a| {
            !a.capture_done.load(Ordering::SeqCst) || !a.worker_done.load(Ordering::SeqCst)
        });
        if busy
            || engine.note_busy.load(Ordering::SeqCst)
            || engine.status.lock().unwrap().diarization_busy
        {
            return Err("update:finish-work".into());
        }
        engine.maintenance.store(true, Ordering::SeqCst);
    }
    state.status.lock().unwrap().phase = "installing".into();
    let e = engine.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Reply<()> {
        let bytes = std::fs::read(e.root.join("updates/verified-update.bin")).map_err(err)?;
        update.verify(&bytes).map_err(err)?;
        e.shutdown();
        update.install(bytes).map_err(err)
    })
    .await
    .map_err(err)
    .and_then(|r| r);
    if let Err(error) = result {
        engine.maintenance.store(false, Ordering::SeqCst);
        return Err(fail(&state, error));
    }
    let _ = std::fs::remove_file(engine.root.join("updates/verified-update.bin"));
    app.state::<ExitPermit>().0.store(true, Ordering::SeqCst);
    app.request_restart();
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_urls_are_fixed_to_this_repository_and_version() {
        let good = "https://github.com/EazyLee30/omascribe/releases/download/v0.3.1/OmaScribe_0.3.1_macOS-AppleSilicon.app.tar.gz";
        assert!(trusted_url(&good.parse().unwrap(), "0.3.1"));
        for bad in [
            good.replace("https:", "http:"),
            good.replace("github.com/", "github.com.evil.test/"),
            good.replace("EazyLee30/", "someone/"),
            format!("{good}?token=oops"),
            good.replace("v0.3.1/", "v0.3.0/"),
        ] {
            assert!(!trusted_url(&bad.parse().unwrap(), "0.3.1"));
        }
    }
}
