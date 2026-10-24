//! Local settings, curated model management, and private workspace data.
use crate::{err, AppState, Reply};
use omascribe_core::{
    library::{Entry, Storage},
    models::{Hardware, ModelStatus},
    preferences::{Preferences, Profile},
};
use tauri::{Emitter, State};
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
pub fn get_network_policy(
    engine: State<AppState>,
) -> Reply<omascribe_core::network::NetworkPolicy> {
    engine.get_network_policy().map_err(err)
}
#[tauri::command]
pub fn set_network_policy(
    engine: State<AppState>,
    policy: omascribe_core::network::NetworkPolicy,
) -> Reply<omascribe_core::network::NetworkPolicy> {
    engine.set_network_policy(policy).map_err(err)
}
#[tauri::command]
pub fn open_configuration_gallery(engine: State<AppState>) -> Reply<()> {
    engine.require_online_recommendations().map_err(err)?;
    // No device IDs, hardware data, profiles or meeting content are placed in this URL.
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(target_os = "linux")]
    let program = "xdg-open";
    omascribe_core::platform::child_command(program)
        .arg("https://omascribe.eazylee.si/community/")
        .spawn()
        .map_err(err)?;
    Ok(())
}

#[tauri::command]
pub fn get_preferences(engine: State<AppState>) -> Preferences {
    engine.preferences.lock().unwrap().clone()
}
#[tauri::command]
pub fn set_preferences(
    app: tauri::AppHandle,
    engine: State<AppState>,
    preferences: Preferences,
) -> Reply<Preferences> {
    let prefs = engine.update_preferences(preferences).map_err(err)?;
    let _ = app.emit("app-preferences", &prefs);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || crate::views::refresh_captions(&handle));
    Ok(prefs)
}
#[tauri::command]
pub fn model_status(engine: State<AppState>) -> Vec<ModelStatus> {
    engine.model_status()
}
#[tauri::command]
pub fn download_model(engine: State<AppState>, id: String) -> Reply<()> {
    engine.inner().download_model(&id).map_err(err)
}
#[tauri::command]
pub fn cancel_download(engine: State<AppState>, id: String) -> Reply<()> {
    engine.cancel_download(&id).map_err(err)
}
#[tauri::command]
pub fn delete_model(engine: State<AppState>, id: String) -> Reply<()> {
    engine.delete_model(&id).map_err(err)
}
#[tauri::command]
pub async fn hardware_profile(engine: State<'_, AppState>) -> Reply<Hardware> {
    let e = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || omascribe_core::models::hardware(&e.root))
        .await
        .map_err(err)
}
#[tauri::command]
pub async fn storage_usage(engine: State<'_, AppState>) -> Reply<Storage> {
    let e = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || omascribe_core::library::storage(&e))
        .await
        .map_err(err)
}
#[tauri::command]
pub fn list_library(engine: State<AppState>) -> Reply<Vec<Entry>> {
    engine.library_entries().map_err(err)
}
#[tauri::command]
pub fn save_library_entry(engine: State<AppState>, entry: Entry) -> Reply<Entry> {
    engine.save_library_entry(entry).map_err(err)
}
#[tauri::command]
pub fn set_meeting_diarization(engine: State<AppState>, id: String, enabled: bool) -> Reply<()> {
    engine.set_meeting_diarization(&id, enabled).map_err(err)
}
#[tauri::command]
pub fn rename_speaker(
    engine: State<AppState>,
    meeting: String,
    id: Option<String>,
    name: String,
) -> Reply<String> {
    engine.rename_speaker(&meeting, id, &name).map_err(err)
}
#[tauri::command]
pub fn assign_speaker(
    engine: State<AppState>,
    meeting: String,
    segment: String,
    speaker: Option<String>,
    revision: u32,
) -> Reply<()> {
    engine
        .assign_speaker(&meeting, &segment, speaker, revision)
        .map_err(err)
}
#[tauri::command]
pub fn diarize(engine: State<AppState>, id: String) -> Reply<()> {
    engine.inner().diarize(&id).map_err(err)
}
#[tauri::command]
pub fn cancel_diarization(engine: State<AppState>) {
    engine.cancel_diarization()
}
#[tauri::command]
pub async fn export_profile(
    app: tauri::AppHandle,
    engine: State<'_, AppState>,
    name: String,
) -> Reply<Option<String>> {
    let e = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let hardware = omascribe_core::models::hardware(&e.root);
        let profile = Profile {
            schema: 1,
            name,
            hardware_hint: format!(
                "{} / {} / {} / {} GiB",
                hardware.os,
                hardware.arch,
                hardware.cpu,
                hardware.total_memory_mib / 1024
            ),
            preferences: e.preferences.lock().unwrap().clone(),
        };
        Profile::parse(&serde_json::to_vec(&profile).map_err(err)?).map_err(err)?;
        let Some(path) = app
            .dialog()
            .file()
            .add_filter("OmaScribe configuration", &["json"])
            .set_file_name("omascribe-config.json")
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = path.into_path().map_err(err)?;
        omascribe_core::preferences::atomic_json(&path, &profile).map_err(err)?;
        Ok(Some(path.to_string_lossy().into_owned()))
    })
    .await
    .map_err(err)?
}
#[tauri::command]
pub async fn import_profile(app: tauri::AppHandle) -> Reply<Option<Profile>> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(path) = app
            .dialog()
            .file()
            .add_filter("OmaScribe configuration", &["json"])
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = path.into_path().map_err(err)?;
        if std::fs::metadata(&path).map_err(err)?.len() > 64 * 1024 {
            return Err("Configuration exceeds 64 KiB".into());
        }
        Profile::parse(&std::fs::read(path).map_err(err)?)
            .map(Some)
            .map_err(err)
    })
    .await
    .map_err(err)?
}
#[tauri::command]
pub fn open_data_folder(engine: State<AppState>) -> Reply<()> {
    crate::file_manager::open(&engine.root, false)
}
#[tauri::command]
pub async fn export_library(
    app: tauri::AppHandle,
    engine: State<'_, AppState>,
) -> Reply<Option<String>> {
    let e = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(path) = app
            .dialog()
            .file()
            .add_filter("Personal library", &["json"])
            .set_file_name("omascribe-private-library.json")
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = path.into_path().map_err(err)?;
        omascribe_core::library::export_library(&e, &path).map_err(err)?;
        Ok(Some(path.to_string_lossy().into_owned()))
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub fn deleted_meetings(engine: State<AppState>) -> Reply<Vec<omascribe_core::Meeting>> {
    engine.deleted_meetings().map_err(err)
}
#[tauri::command]
pub fn set_meeting_deleted(engine: State<AppState>, id: String, deleted: bool) -> Reply<()> {
    engine.set_meeting_deleted(&id, deleted).map_err(err)
}
#[tauri::command]
pub fn delete_speaker(engine: State<AppState>, meeting: String, id: String) -> Reply<()> {
    engine.delete_speaker(&meeting, &id).map_err(err)
}
