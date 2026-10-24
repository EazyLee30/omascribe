use crate::{overlay, AppState, Reply};
#[cfg(target_os = "linux")]
use gtk::prelude::*;
use omascribe_core::Snapshot;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::{Emitter, Manager};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptionPreferences {
    pub enabled: bool,
    pub position: String,
    pub font_size: u32,
    pub lines: u32,
    pub width_percent: u32,
    pub margin: u32,
    pub opacity: f64,
    pub text_color: String,
    pub background_color: String,
    pub click_through: bool,
    pub idle_seconds: u64,
    pub radius: u32,
    pub align: String,
    pub monitor: i32,
}
impl Default for CaptionPreferences {
    fn default() -> Self {
        Self {
            enabled: false,
            position: "bottom".into(),
            font_size: 28,
            lines: 2,
            width_percent: 80,
            margin: 32,
            opacity: 0.8,
            text_color: "#f1f4eb".into(),
            background_color: "#182014".into(),
            click_through: true,
            idle_seconds: 12,
            radius: 10,
            align: "center".into(),
            monitor: -1,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewPreferences {
    pub sidebar_collapsed: bool,
    pub notebook_collapsed: bool,
    pub minutes_collapsed: bool,
    pub transcript_mode: String,
    pub dock_width: u32,
    pub floating_width: u32,
    pub floating_height: u32,
    pub compact: bool,
    pub snap: String,
    pub follow_recording: bool,
    pub transcript_font: u32,
    pub captions: CaptionPreferences,
}
impl Default for ViewPreferences {
    fn default() -> Self {
        Self {
            sidebar_collapsed: false,
            notebook_collapsed: false,
            minutes_collapsed: false,
            transcript_mode: "hidden".into(),
            dock_width: 320,
            floating_width: 380,
            floating_height: 560,
            compact: false,
            snap: "right".into(),
            follow_recording: true,
            transcript_font: 15,
            captions: CaptionPreferences::default(),
        }
    }
}

impl ViewPreferences {
    fn normalize(&mut self) -> Reply<()> {
        if !["hidden", "docked", "floating"].contains(&self.transcript_mode.as_str())
            || !["free", "left", "right"].contains(&self.snap.as_str())
            || !["top", "bottom"].contains(&self.captions.position.as_str())
        {
            return Err("Invalid display mode".into());
        }
        for value in [&self.captions.text_color, &self.captions.background_color] {
            if value.len() != 7
                || !value.starts_with('#')
                || !value[1..].chars().all(|c| c.is_ascii_hexdigit())
            {
                return Err("Use a six-digit hex color".into());
            }
        }
        self.dock_width = self.dock_width.clamp(240, 560);
        self.floating_width = self.floating_width.clamp(280, 1000);
        self.floating_height = self.floating_height.clamp(220, 1200);
        self.transcript_font = self.transcript_font.clamp(11, 28);
        let c = &mut self.captions;
        c.font_size = c.font_size.clamp(14, 64);
        c.lines = c.lines.clamp(1, 5);
        c.width_percent = c.width_percent.clamp(30, 100);
        c.margin = c.margin.min(300);
        c.opacity = if c.opacity.is_finite() {
            c.opacity.clamp(0., 1.)
        } else {
            0.8
        };
        c.idle_seconds = c.idle_seconds.min(120);
        c.radius = c.radius.min(36);
        if c.align != "left" {
            c.align = "center".into();
        }
        Ok(())
    }
}
pub struct Views {
    pub prefs: Mutex<ViewPreferences>,
    pub meeting: Mutex<Option<String>>,
    root: PathBuf,
    dirty: AtomicBool,
}
pub type ViewState = Arc<Views>;
impl Views {
    pub fn new(root: PathBuf) -> ViewState {
        let mut prefs = fs::read(root.join("view-preferences.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ViewPreferences>(&bytes).ok())
            .unwrap_or_default();
        if prefs.normalize().is_err() {
            prefs = ViewPreferences::default();
        }
        // Visibility is session-scoped; keep visual preferences without showing private text at login.
        prefs.captions.enabled = false;
        prefs.transcript_mode = "hidden".into();
        Arc::new(Self {
            prefs: Mutex::new(prefs),
            meeting: Mutex::new(None),
            root,
            dirty: AtomicBool::new(false),
        })
    }
    fn persist(&self) -> Reply<()> {
        let bytes =
            serde_json::to_vec_pretty(&*self.prefs.lock().unwrap()).map_err(|e| e.to_string())?;
        let part = self.root.join("view-preferences.json.part");
        fs::write(&part, bytes).map_err(|e| e.to_string())?;
        fs::rename(part, self.root.join("view-preferences.json")).map_err(|e| e.to_string())
    }
}
#[tauri::command]
pub fn get_view_settings(views: tauri::State<ViewState>) -> ViewPreferences {
    views.prefs.lock().unwrap().clone()
}
#[tauri::command]
pub fn set_view_meeting(app: tauri::AppHandle, views: tauri::State<ViewState>, id: String) {
    *views.meeting.lock().unwrap() = Some(id.clone());
    let _ = app.emit("view-meeting", id);
}
fn active_id(engine: &AppState, views: &Views) -> Option<String> {
    if views.prefs.lock().unwrap().follow_recording {
        let status = engine.status.lock().unwrap();
        if status.recording || status.paused {
            return status.meeting_id.clone();
        }
    }
    views.meeting.lock().unwrap().clone()
}
#[tauri::command]
pub fn transcript_snapshot(
    engine: tauri::State<AppState>,
    views: tauri::State<ViewState>,
) -> Reply<Option<Snapshot>> {
    active_id(engine.inner(), &views)
        .map(|id| engine.snapshot(&id).map_err(|e| e.to_string()))
        .transpose()
}
#[tauri::command]
pub async fn set_view_settings(
    app: tauri::AppHandle,
    views: tauri::State<'_, ViewState>,
    engine: tauri::State<'_, AppState>,
    mut preferences: ViewPreferences,
) -> Reply<ViewPreferences> {
    preferences.normalize()?;
    let handle = app.clone();
    let state = views.inner().clone();
    let runtime = engine.runtime_dir.clone();
    let result = preferences.clone();
    let (send, receive) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let applied = apply_windows(&handle, &state, &runtime, &preferences).and_then(|_| {
            *state.prefs.lock().unwrap() = preferences.clone();
            state.persist()?;
            let _ = handle.emit("view-settings", &preferences);
            Ok(())
        });
        let _ = send.send(applied);
    })
    .map_err(|e| e.to_string())?;
    receive.await.map_err(|e| e.to_string())??;
    Ok(result)
}
#[tauri::command]
pub fn drag_transcript(app: tauri::AppHandle) -> Reply<()> {
    app.get_webview_window("transcript")
        .ok_or("Transcript window is not open")?
        .start_dragging()
        .map_err(|e| e.to_string())
}
fn apply_windows(
    app: &tauri::AppHandle,
    views: &Views,
    runtime: &std::path::Path,
    prefs: &ViewPreferences,
) -> Reply<()> {
    overlay::apply(app, runtime, &prefs.captions)?;
    if prefs.transcript_mode == "floating" {
        let old = views.prefs.lock().unwrap().clone();
        let window = if let Some(window) = app.get_webview_window("transcript") {
            window
        } else {
            tauri::WebviewWindowBuilder::new(
                app,
                "transcript",
                tauri::WebviewUrl::App("transcript.html".into()),
            )
            .title("OmaScribe — Live transcript")
            .inner_size(prefs.floating_width as f64, prefs.floating_height as f64)
            .min_inner_size(280., 56.)
            .decorations(false)
            .resizable(true)
            .always_on_top(true)
            .build()
            .map_err(|e| e.to_string())?
        };
        window
            .set_size(tauri::LogicalSize::new(
                prefs.floating_width as f64,
                if prefs.compact {
                    64.
                } else {
                    prefs.floating_height as f64
                },
            ))
            .map_err(|e| e.to_string())?;
        window.show().map_err(|e| e.to_string())?;
        if old.transcript_mode != "floating"
            || old.snap != prefs.snap
            || old.floating_width != prefs.floating_width
            || old.compact != prefs.compact
        {
            #[cfg(target_os = "linux")]
            place_hyprland_window(prefs.clone());
            #[cfg(not(target_os = "linux"))]
            place_native_window(&window, prefs)?;
        }
    } else if let Some(window) = app.get_webview_window("transcript") {
        window.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}
// Wayland clients cannot place ordinary windows themselves. Target only our own auxiliary surface;
// transient compositor commands do not edit the user's Hyprland or Omarchy configuration.
#[cfg(target_os = "linux")]
fn place_hyprland_window(prefs: ViewPreferences) {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return;
    }
    std::thread::spawn(move || {
        for _ in 0..15 {
            std::thread::sleep(Duration::from_millis(100));
            let Ok(output) = std::process::Command::new("hyprctl")
                .args(["clients", "-j"])
                .output()
            else {
                return;
            };
            let Ok(windows) = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout)
            else {
                return;
            };
            let Some(window) = windows.iter().find(|w| {
                w["pid"].as_u64() == Some(std::process::id() as u64)
                    && w["title"].as_str() == Some("OmaScribe — Live transcript")
            }) else {
                continue;
            };
            let Some(address) = window["address"]
                .as_str()
                .filter(|a| a.starts_with("0x") && a[2..].chars().all(|c| c.is_ascii_hexdigit()))
            else {
                return;
            };
            let target = format!("address:{address}");
            let run = |code: String| {
                let _ = std::process::Command::new("hyprctl")
                    .args(["eval", &code])
                    .output();
            };
            run(format!(
                "hl.dispatch(hl.dsp.window.float({{action='on',window='{target}'}}))"
            ));
            std::thread::sleep(Duration::from_millis(120));
            let height = if prefs.compact {
                64
            } else {
                prefs.floating_height
            };
            run(format!("hl.dispatch(hl.dsp.window.resize({{x={},y={height},relative=false,window='{target}'}}))",prefs.floating_width));
            if prefs.snap != "free" {
                if let Ok(output) = std::process::Command::new("hyprctl")
                    .args(["monitors", "-j"])
                    .output()
                {
                    if let Ok(monitors) =
                        serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout)
                    {
                        if let Some(monitor) = monitors
                            .iter()
                            .find(|m| m["focused"].as_bool() == Some(true))
                            .or(monitors.first())
                        {
                            let scale = monitor["scale"].as_f64().unwrap_or(1.);
                            let left = monitor["x"].as_i64().unwrap_or(0);
                            let top = monitor["y"].as_i64().unwrap_or(0);
                            let width = (monitor["width"].as_f64().unwrap_or(1440.) / scale) as i64;
                            let x = if prefs.snap == "left" {
                                left + 16
                            } else {
                                left + width - prefs.floating_width as i64 - 16
                            };
                            run(format!("hl.dispatch(hl.dsp.window.move({{x={x},y={},relative=false,window='{target}'}}))",top+60));
                        }
                    }
                }
            }
            return;
        }
    });
}
#[derive(Serialize)]
pub struct MonitorInfo {
    id: i32,
    name: String,
    width: i32,
    height: i32,
}
#[cfg(target_os = "linux")]
#[tauri::command]
pub async fn display_monitors(app: tauri::AppHandle) -> Reply<Vec<MonitorInfo>> {
    let (send, receive) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let monitors = gtk::gdk::Display::default()
            .map(|d| {
                (0..d.n_monitors())
                    .filter_map(|id| {
                        d.monitor(id).map(|m| {
                            let g = m.geometry();
                            MonitorInfo {
                                id,
                                name: m
                                    .model()
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| format!("Display {}", id + 1)),
                                width: g.width(),
                                height: g.height(),
                            }
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let _ = send.send(monitors);
    })
    .map_err(|e| e.to_string())?;
    receive.await.map_err(|e| e.to_string())
}
pub fn start(app: &tauri::AppHandle) {
    let handle = app.clone();
    let tick = move || {
        let engine = handle.state::<AppState>();
        let views = handle.state::<ViewState>();
        let prefs = views.prefs.lock().unwrap().clone();
        if prefs.captions.enabled {
            let text = active_id(engine.inner(), &views)
                .and_then(|id| {
                    let status = engine.status.lock().unwrap().clone();
                    let db = engine.store.lock().unwrap();
                    let meeting = db.meeting(&id).ok()?;
                    let mut lines: Vec<String> = db
                        .recent_segments(&meeting, 2)
                        .ok()?
                        .into_iter()
                        .map(|s| s.text)
                        .collect();
                    if status.meeting_id.as_deref() == Some(&id) && !status.partial.is_empty() {
                        lines.push(status.partial);
                    }
                    let text = lines.join(" ");
                    let limit = (prefs.captions.width_percent as usize * 3).min(300)
                        * prefs.captions.lines as usize;
                    let chars: Vec<char> = text.chars().collect();
                    Some(if chars.len() > limit {
                        format!(
                            "…{}",
                            chars[chars.len() - limit..].iter().collect::<String>()
                        )
                    } else {
                        text
                    })
                })
                .unwrap_or_default();
            overlay::render(&handle, &text, &prefs.captions);
        }
        if views.dirty.swap(false, Ordering::SeqCst) && views.persist().is_err() {
            views.dirty.store(true, Ordering::SeqCst);
        }
    };
    #[cfg(target_os = "linux")]
    gtk::glib::timeout_add_local(Duration::from_millis(350), move || {
        tick();
        gtk::glib::ControlFlow::Continue
    });
    #[cfg(not(target_os = "linux"))]
    {
        let app = app.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(350));
            let tick = tick.clone();
            if app.run_on_main_thread(tick).is_err() {
                break;
            }
        });
    }
}
pub fn window_event(window: &tauri::Window, event: &tauri::WindowEvent) -> bool {
    if window.label() == "captions" {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = window.hide();
            let views = window.state::<ViewState>();
            let mut prefs = views.prefs.lock().unwrap();
            prefs.captions.enabled = false;
            let _ = window.app_handle().emit("view-settings", prefs.clone());
            views.dirty.store(true, Ordering::SeqCst);
        }
        return true;
    }
    if window.label() != "transcript" {
        return false;
    }
    if let tauri::WindowEvent::Resized(size) = event {
        let views = window.state::<ViewState>();
        let mut prefs = views.prefs.lock().unwrap();
        let scale = window.scale_factor().unwrap_or(1.);
        let logical = size.to_logical::<u32>(scale);
        if logical.width >= 280 {
            prefs.floating_width = logical.width.min(1000);
        }
        if !prefs.compact && logical.height >= 220 {
            prefs.floating_height = logical.height.min(1200);
        }
        views.dirty.store(true, Ordering::SeqCst);
    }
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
        let views = window.state::<ViewState>();
        let mut prefs = views.prefs.lock().unwrap();
        prefs.transcript_mode = "hidden".into();
        let _ = window.emit("view-settings", prefs.clone());
        drop(prefs);
        views.dirty.store(true, Ordering::SeqCst);
    }
    true
}

#[cfg(not(target_os = "linux"))]
fn place_native_window(window: &tauri::WebviewWindow, prefs: &ViewPreferences) -> Reply<()> {
    if prefs.snap == "free" {
        return Ok(());
    }
    if let Some(monitor) = window.current_monitor().map_err(|e| e.to_string())? {
        let size = monitor.size().to_logical::<f64>(monitor.scale_factor());
        let offset = if prefs.snap == "left" {
            16.
        } else {
            (size.width - prefs.floating_width as f64 - 16.).max(0.)
        };
        window
            .set_position(tauri::PhysicalPosition::new(
                monitor.position().x + (offset * monitor.scale_factor()) as i32,
                monitor.position().y + (60. * monitor.scale_factor()) as i32,
            ))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
#[cfg(not(target_os = "linux"))]
#[tauri::command]
pub fn display_monitors(app: tauri::AppHandle) -> Reply<Vec<MonitorInfo>> {
    Ok(app
        .available_monitors()
        .map_err(|e| e.to_string())?
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let size = m.size().to_logical::<i32>(m.scale_factor());
            MonitorInfo {
                id: i as i32,
                name: m
                    .name()
                    .cloned()
                    .unwrap_or_else(|| format!("Display {}", i + 1)),
                width: size.width,
                height: size.height,
            }
        })
        .collect())
}

#[cfg(test)]
mod preference_tests {
    use super::ViewPreferences;

    #[test]
    fn old_preferences_keep_the_main_columns_open() {
        let prefs: ViewPreferences =
            serde_json::from_str(r#"{"transcript_mode":"docked","dock_width":380}"#).unwrap();
        assert!(!prefs.sidebar_collapsed);
        assert!(!prefs.notebook_collapsed);
        assert!(!prefs.minutes_collapsed);
        assert_eq!(prefs.dock_width, 380);
        assert_eq!(prefs.transcript_mode, "docked");
    }

    #[test]
    fn collapsed_columns_survive_serialization() {
        let prefs = ViewPreferences {
            sidebar_collapsed: true,
            notebook_collapsed: true,
            minutes_collapsed: true,
            ..ViewPreferences::default()
        };
        let restored: ViewPreferences =
            serde_json::from_slice(&serde_json::to_vec(&prefs).unwrap()).unwrap();
        assert!(
            restored.sidebar_collapsed && restored.notebook_collapsed && restored.minutes_collapsed
        );
    }
}
