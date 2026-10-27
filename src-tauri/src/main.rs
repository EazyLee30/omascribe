#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use omascribe_core::{Device, Diagnostics, Engine, Meeting, Snapshot, UserNote};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tauri::Manager;
mod companion;
mod file_manager;
#[cfg(target_os = "linux")]
mod overlay;
#[cfg(not(target_os = "linux"))]
#[path = "overlay_native.rs"]
mod overlay;
mod profiles;
#[cfg(feature = "ui-test")]
mod ui_test_probe;
mod updates;
mod views;
mod workspace;
use tauri::{Emitter, State};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_dialog::DialogExt;
use updates::*;
use views::{
    display_monitors, drag_transcript, get_view_settings, set_view_meeting, set_view_settings,
    show_main_window, transcript_snapshot,
};
use workspace::*;
type AppState = Arc<Engine>;
struct ExitPermit(AtomicBool);
type Reply<T> = Result<T, String>;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
fn list_meetings(engine: State<AppState>) -> Reply<Vec<Meeting>> {
    engine.meetings().map_err(err)
}
#[tauri::command]
fn create_meeting(engine: State<AppState>, title: String) -> Reply<Meeting> {
    engine.create_meeting(&title).map_err(err)
}
#[tauri::command]
fn snapshot(engine: State<AppState>, id: String) -> Reply<Snapshot> {
    engine.snapshot(&id).map_err(err)
}
#[tauri::command]
async fn extract_focus_terms(content: String) -> Vec<String> {
    tauri::async_runtime::spawn_blocking(move || omascribe_core::notes::extract_focus(&content))
        .await
        .unwrap_or_default()
}
#[tauri::command]
fn diagnostics(engine: State<AppState>) -> Diagnostics {
    engine.diagnostics()
}
#[tauri::command]
async fn list_devices() -> Reply<Vec<Device>> {
    tauri::async_runtime::spawn_blocking(|| omascribe_core::audio::devices().map_err(err))
        .await
        .map_err(err)?
}
#[tauri::command]
fn microphone_test_status(engine: State<AppState>) -> omascribe_core::microphone::InputLevel {
    engine.microphone_level()
}
#[tauri::command]
async fn start_microphone_test(
    engine: State<'_, AppState>,
    device: String,
) -> Reply<omascribe_core::microphone::InputLevel> {
    let e = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || e.start_microphone_preview(device).map_err(err))
        .await
        .map_err(err)?
}
#[tauri::command]
async fn stop_microphone_test(
    engine: State<'_, AppState>,
) -> Reply<omascribe_core::microphone::InputLevel> {
    let e = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || e.stop_microphone_preview())
        .await
        .map_err(err)
}
#[tauri::command]
fn update_meeting(
    engine: State<AppState>,
    id: String,
    title: String,
    auto_notes: bool,
) -> Reply<()> {
    engine.update_meeting(&id, &title, auto_notes).map_err(err)
}
#[tauri::command]
fn save_note(
    engine: State<AppState>,
    id: String,
    note_id: Option<String>,
    content: String,
    kind: String,
    anchor_ms: u64,
) -> Reply<UserNote> {
    engine
        .save_note(&id, note_id, &content, &kind, anchor_ms)
        .map_err(err)
}
#[tauri::command]
fn edit_segment(engine: State<AppState>, id: String, text: String, revision: u32) -> Reply<()> {
    engine.edit_segment(&id, &text, revision).map_err(err)
}
#[tauri::command]
fn update_suggestion(
    engine: State<AppState>,
    meeting: String,
    id: String,
    status: String,
    text: Option<String>,
) -> Reply<()> {
    engine
        .store
        .lock()
        .unwrap()
        .update_suggestion(&meeting, &id, &status, text)
        .map_err(err)
}
#[tauri::command]
async fn start_recording(engine: State<'_, AppState>, id: String, device: String) -> Reply<()> {
    let e = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || e.start_recording(&id, &device, None).map_err(err))
        .await
        .map_err(err)?
}
#[tauri::command]
fn pause_recording(engine: State<AppState>) -> Reply<()> {
    engine.pause_recording().map_err(err)
}
#[tauri::command]
async fn resume_recording(engine: State<'_, AppState>) -> Reply<()> {
    let e = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || e.resume_recording().map_err(err))
        .await
        .map_err(err)?
}
#[tauri::command]
fn stop_recording(engine: State<AppState>) -> Reply<()> {
    engine.stop_recording().map_err(err)
}
#[tauri::command]
fn retranscribe(engine: State<AppState>, id: String) -> Reply<()> {
    engine.inner().retranscribe(&id).map_err(err)
}
#[tauri::command]
fn generate_notes(engine: State<AppState>, id: String) -> Reply<()> {
    engine.inner().generate_notes(&id).map_err(err)
}
#[tauri::command]
fn cancel_notes(engine: State<AppState>) {
    engine.cancel_notes()
}
#[tauri::command]
async fn audio_clip(
    engine: State<'_, AppState>,
    id: String,
    start_ms: u64,
    end_ms: u64,
) -> Reply<String> {
    let e = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || e.clip(&id, start_ms, end_ms).map_err(err))
        .await
        .map_err(err)?
}
#[tauri::command]
async fn export_meeting(engine: State<'_, AppState>, id: String, audio: bool) -> Reply<String> {
    let e = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || e.export(&id, audio).map_err(err))
        .await
        .map_err(err)?
}
#[tauri::command]
async fn import_audio(
    app: tauri::AppHandle,
    diarization: Option<bool>,
    engine: State<'_, AppState>,
) -> Reply<Option<Meeting>> {
    let e = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(path) = app
            .dialog()
            .file()
            .add_filter(
                "Audio",
                &["wav", "mp3", "m4a", "flac", "ogg", "opus", "mp4", "webm"],
            )
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path: PathBuf = path.into_path().map_err(err)?;
        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Imported audio");
        let mut m = e.create_meeting(title).map_err(err)?;
        if let Some(enabled) = diarization {
            e.set_meeting_diarization(&m.id, enabled).map_err(err)?;
            m.diarization = enabled;
        }
        e.start_recording(&m.id, "auto", Some(path)).map_err(err)?;
        Ok(Some(m))
    })
    .await
    .map_err(err)?
}
#[tauri::command]
async fn meeting_files(
    engine: State<'_, AppState>,
    id: String,
) -> Reply<omascribe_core::MeetingFiles> {
    let e = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || e.meeting_files(&id).map_err(err))
        .await
        .map_err(err)?
}
#[tauri::command]
async fn open_meeting_files(
    engine: State<'_, AppState>,
    id: String,
    name: Option<String>,
) -> Reply<()> {
    let e = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let files = e.meeting_files(&id).map_err(err)?;
        if let Some(name) = name {
            let file = files
                .files
                .iter()
                .find(|f| f.name == name)
                .ok_or("Unknown meeting file")?;
            if !std::path::Path::new(&file.path).is_file() {
                return Err("Recording is still being saved".into());
            }
            return file_manager::open(std::path::Path::new(&file.path), true);
        }
        file_manager::open(std::path::Path::new(&files.directory), false)
    })
    .await
    .map_err(err)?
}
#[tauri::command]
fn open_exports(engine: State<AppState>) -> Reply<()> {
    let dir = engine.root.join("exports");
    std::fs::create_dir_all(&dir).map_err(err)?;
    file_manager::open(&dir, false)
}
#[tauri::command]
async fn quit(app: tauri::AppHandle, engine: State<'_, AppState>) -> Reply<()> {
    let e = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || e.shutdown())
        .await
        .map_err(err)?;
    app.state::<ExitPermit>().0.store(true, Ordering::SeqCst);
    app.exit(0);
    Ok(())
}

#[cfg(feature = "ui-test")]
#[tauri::command]
fn ui_test_resize(window: tauri::WebviewWindow, width: u32, height: u32) -> Reply<()> {
    window
        .set_size(tauri::LogicalSize::new(
            width.clamp(740, 1600),
            height.clamp(560, 1000),
        ))
        .map_err(err)
}
#[cfg(feature = "ui-test")]
#[tauri::command]
fn ui_test_windows(app: tauri::AppHandle) -> serde_json::Value {
    use tauri::Manager;
    serde_json::json!(app.webview_windows().iter().map(|(label,w)|serde_json::json!({"label":label,"visible":w.is_visible().unwrap_or(false),"maximized":w.is_maximized().unwrap_or(false),"position":w.outer_position().ok().map(|p|(p.x,p.y)),"size":w.inner_size().ok().map(|s|(s.width,s.height))})).collect::<Vec<_>>())
}
#[cfg(feature = "ui-test")]
#[tauri::command]
fn ui_test_replay(engine: State<AppState>, id: String) -> Reply<()> {
    engine
        .inner()
        .replay_audio(&id, engine.root.join("test-recording.wav"), 30)
        .map_err(err)
}

#[cfg(feature = "ui-test")]
#[tauri::command]
fn ui_test_report(report: serde_json::Value) -> Reply<()> {
    let path = std::env::var("OMASCRIBE_UI_TEST_REPORT").map_err(err)?;
    std::fs::write(path, serde_json::to_vec_pretty(&report).map_err(err)?).map_err(err)
}

#[cfg(all(feature = "ui-test", target_os = "linux"))]
#[tauri::command]
async fn ui_test_screenshot(window: tauri::WebviewWindow, name: String) -> Reply<()> {
    use webkit2gtk::WebViewExt;
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("Invalid snapshot name".into());
    }
    let directory = std::env::var("OMASCRIBE_UI_SCREENSHOTS").map_err(err)?;
    let path = std::path::Path::new(&directory).join(format!("{name}.png"));
    let (send, receive) = tokio::sync::oneshot::channel();
    window
        .with_webview(move |view| {
            view.inner().snapshot(
                webkit2gtk::SnapshotRegion::FullDocument,
                webkit2gtk::SnapshotOptions::NONE,
                None::<&gtk::gio::Cancellable>,
                move |result| {
                    let saved = result.map_err(err).and_then(|surface| {
                        let mut file = std::fs::File::create(path).map_err(err)?;
                        surface.write_to_png(&mut file).map_err(err)
                    });
                    let _ = send.send(saved);
                },
            );
        })
        .map_err(err)?;
    receive.await.map_err(err)?
}

#[cfg(all(feature = "ui-test", not(target_os = "linux")))]
#[tauri::command]
async fn ui_test_screenshot(_window: tauri::WebviewWindow, _name: String) -> Reply<()> {
    Err("Use a native screen capture on this platform".into())
}

fn main() {
    let regular: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool = tauri::generate_handler![
        profiles::take_pending_profile,
        get_network_policy,
        set_network_policy,
        open_configuration_gallery,
        get_preferences,
        set_preferences,
        model_status,
        download_model,
        cancel_download,
        delete_model,
        hardware_profile,
        acceleration_components,
        check_acceleration_components,
        download_acceleration_component,
        cancel_acceleration_download,
        verify_acceleration,
        disable_acceleration,
        storage_usage,
        list_library,
        save_library_entry,
        export_profile,
        import_profile,
        open_data_folder,
        export_library,
        set_meeting_diarization,
        rename_speaker,
        delete_speaker,
        deleted_meetings,
        set_meeting_deleted,
        assign_speaker,
        diarize,
        cancel_diarization,
        update_status,
        check_update,
        download_update,
        cancel_update,
        install_update,
        list_meetings,
        create_meeting,
        snapshot,
        diagnostics,
        extract_focus_terms,
        list_devices,
        microphone_test_status,
        start_microphone_test,
        stop_microphone_test,
        update_meeting,
        save_note,
        edit_segment,
        update_suggestion,
        start_recording,
        stop_recording,
        pause_recording,
        resume_recording,
        get_view_settings,
        set_view_settings,
        set_view_meeting,
        transcript_snapshot,
        show_main_window,
        drag_transcript,
        companion::set_companion_expanded,
        companion::companion_start,
        companion::companion_snapshot,
        display_monitors,
        retranscribe,
        generate_notes,
        cancel_notes,
        audio_clip,
        export_meeting,
        import_audio,
        open_exports,
        meeting_files,
        open_meeting_files,
        quit
    ];
    #[cfg(feature = "ui-test")]
    let tester: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool = tauri::generate_handler![
        ui_test_report,
        ui_test_screenshot,
        ui_test_resize,
        ui_test_replay,
        ui_test_windows,
        ui_test_probe::ui_test_window_probe,
        ui_test_probe::ui_test_window_result
    ];
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Updates::default())
        .manage(profiles::PendingProfile::default())
        .manage(ExitPermit(AtomicBool::new(false)))
        .invoke_handler(move |request| {
            #[cfg(feature = "ui-test")]
            if ["ui_test_report", "ui_test_screenshot", "ui_test_resize", "ui_test_replay", "ui_test_windows", "ui_test_window_probe", "ui_test_window_result"].contains(&request.message.command()) {
                return tester(request);
            }
            regular(request)
        })
        .setup(|app| {
            #[cfg(feature = "ui-test")]
            app.manage(ui_test_probe::Probes::default());
            // Single-instance forwarding runs before taking the workspace lock.
            let engine = Engine::open(omascribe_core::engine::default_root())?;
            let view_state = views::Views::new(engine.root.clone());
            engine.start_scheduler();
            app.manage(engine);
            app.manage(view_state);
            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                if let Some(url) = event.urls().last() {
                    profiles::receive(&handle, url.as_str());
                }
            });
            // Subscribe before draining startup URLs so an open event cannot fall in a gap.
            if let Some(urls) = app.deep_link().get_current()? {
                if let Some(url) = urls.last() {
                    profiles::receive(app.handle(), url.as_str());
                }
            }
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            if let Err(error) = app.deep_link().register_all() {
                // Desktop integration is optional; missing Linux tools or registry
                // permissions must not prevent access to the offline workspace.
                eprintln!("Configuration link registration unavailable: {error}. Configuration files can still be imported in Settings.");
            }
            views::start(app.handle());
            #[cfg(target_os = "linux")]
            companion::start_anchor_sync(app.handle());
            #[cfg(feature = "ui-test")]
            if let Ok(path) = std::env::var("OMASCRIBE_UI_TEST_SCRIPT") {
                let script = std::fs::read_to_string(path)?;
                let window = app.get_webview_window("main").unwrap();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    let _ = window.eval(script);
                });
            }
            #[cfg(not(feature = "ui-test"))]
            let _ = app;
            Ok(())
        })
        .on_window_event(|window, event| {
            if companion::window_event(window, event) {
                return;
            }
            if views::window_event(window, event) {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Let the UI flush autosaves and ask about an active recording before exiting.
                api.prevent_close();
                let _ = window.emit("close-requested", ());
            }
        })
        .build(tauri::generate_context!())
        .expect("OmaScribe could not start")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                if !app.state::<ExitPermit>().0.load(Ordering::SeqCst) {
                    // Native menu / Cmd-Q follows the same autosave and active-take confirmation
                    // as the close button. Only the completed frontend quit flow permits exit.
                    api.prevent_exit();
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.emit("close-requested", ());
                    }
                }
            }
        });
}
