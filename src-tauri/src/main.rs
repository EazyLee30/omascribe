use omascribe_core::{Device, Diagnostics, Engine, Meeting, Snapshot, UserNote};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tauri::Manager;
mod file_manager;
#[cfg(target_os = "linux")]
mod overlay;
#[cfg(not(target_os = "linux"))]
#[path = "overlay_native.rs"]
mod overlay;
mod views;
use tauri::{Emitter, State};
use tauri_plugin_dialog::DialogExt;
use views::{
    display_monitors, drag_transcript, get_view_settings, set_view_meeting, set_view_settings,
    transcript_snapshot,
};
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
        let m = e.create_meeting(title).map_err(err)?;
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
fn ui_test_report(report: serde_json::Value) -> Reply<()> {
    let path = std::env::var("OMASCRIBE_UI_TEST_REPORT").map_err(err)?;
    std::fs::write(path, serde_json::to_vec_pretty(&report).map_err(err)?).map_err(err)
}

#[cfg(feature = "ui-test")]
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

fn main() {
    let engine = match Engine::open(omascribe_core::engine::default_root()) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("OmaScribe: {e:#}");
            return;
        }
    };
    engine.start_scheduler();
    let regular: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool = tauri::generate_handler![
        list_meetings,
        create_meeting,
        snapshot,
        diagnostics,
        list_devices,
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
        drag_transcript,
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
    let tester: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool =
        tauri::generate_handler![ui_test_report, ui_test_screenshot];
    let view_state = views::Views::new(engine.root.clone());
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(engine)
        .manage(view_state)
        .manage(ExitPermit(AtomicBool::new(false)))
        .invoke_handler(move |request| {
            #[cfg(feature = "ui-test")]
            if ["ui_test_report", "ui_test_screenshot"].contains(&request.message.command()) {
                return tester(request);
            }
            regular(request)
        })
        .setup(|app| {
            views::start(app.handle());
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
