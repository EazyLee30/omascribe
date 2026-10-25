//! A transparent, desktop-level recording companion. No network or recording on launch.
use crate::{err, views::ViewState, AppState, Reply};
use tauri::{Emitter, Manager};

pub fn apply(app: &tauri::AppHandle, enabled: bool) -> Reply<()> {
    let existing = app.get_webview_window("companion");
    if !enabled {
        if let Some(w) = existing {
            if w.is_visible().unwrap_or(false) {
                w.hide().map_err(err)?;
            }
        }
        return Ok(());
    }
    if let Some(w) = existing {
        if !w.is_visible().unwrap_or(false) {
            w.show().map_err(err)?;
        }
        return Ok(());
    }
    let w = tauri::WebviewWindowBuilder::new(
        app,
        "companion",
        tauri::WebviewUrl::App("companion.html".into()),
    )
    .title("OmaScribe Companion")
    .inner_size(100., 112.)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .resizable(false)
    .always_on_top(true)
    .visible_on_all_workspaces(true)
    .skip_taskbar(true)
    .focused(false)
    .accept_first_mouse(true)
    .visible(false)
    .build()
    .map_err(err)?;
    #[cfg(not(target_os = "linux"))]
    {
        let prefs = app.state::<ViewState>().prefs.lock().unwrap().clone();
        let monitor = w
            .current_monitor()
            .map_err(err)?
            .or(w.primary_monitor().map_err(err)?);
        if let Some(m) = monitor {
            let area = m.work_area();
            let scale = m.scale_factor();
            let size = (100. * scale, 112. * scale);
            let anchor = prefs
                .companion_anchor
                .filter(|(x, y)| {
                    app.available_monitors()
                        .unwrap_or_default()
                        .iter()
                        .any(|m| {
                            let a = m.work_area();
                            *x >= a.position.x
                                && *y >= a.position.y
                                && *x <= a.position.x + a.size.width as i32
                                && *y <= a.position.y + a.size.height as i32
                        })
                })
                .unwrap_or((
                    area.position.x + area.size.width as i32 - (24. * scale) as i32,
                    area.position.y + area.size.height as i32 - (24. * scale) as i32,
                ));
            w.set_position(tauri::PhysicalPosition::new(
                anchor.0 - size.0 as i32,
                anchor.1 - size.1 as i32,
            ))
            .map_err(err)?;
        }
    }
    w.show().map_err(err)?;
    #[cfg(target_os = "linux")]
    place_hyprland(100, 112, true);
    Ok(())
}

#[tauri::command]
pub fn set_companion_expanded(app: tauri::AppHandle, expanded: bool) -> Reply<()> {
    let w = app
        .get_webview_window("companion")
        .ok_or("Companion unavailable")?;
    let (width, height) = if expanded { (380., 340.) } else { (100., 112.) };
    #[cfg(not(target_os = "linux"))]
    let (p, old, scale) = (
        w.outer_position().map_err(err)?,
        w.inner_size().map_err(err)?,
        w.scale_factor().map_err(err)?,
    );
    #[cfg(not(target_os = "linux"))]
    let anchor = (p.x + old.width as i32, p.y + old.height as i32);
    w.set_size(tauri::LogicalSize::new(width, height))
        .map_err(err)?;
    #[cfg(not(target_os = "linux"))]
    {
        let mut x = anchor.0 - (width * scale) as i32;
        let mut y = anchor.1 - (height * scale) as i32;
        if let Some(m) = w.current_monitor().map_err(err)? {
            let a = m.work_area();
            x = x.clamp(
                a.position.x,
                (a.position.x + a.size.width as i32 - (width * scale) as i32).max(a.position.x),
            );
            y = y.clamp(
                a.position.y,
                (a.position.y + a.size.height as i32 - (height * scale) as i32).max(a.position.y),
            );
        }
        w.set_position(tauri::PhysicalPosition::new(x, y))
            .map_err(err)?;
    }
    #[cfg(target_os = "linux")]
    place_hyprland(width as u32, height as u32, false);
    if expanded {
        w.set_focus().map_err(err)?;
    }
    Ok(())
}

#[tauri::command]
pub fn companion_snapshot(
    engine: tauri::State<AppState>,
    views: tauri::State<ViewState>,
) -> Reply<serde_json::Value> {
    let status = engine.status.lock().unwrap().clone();
    let id = if status.recording || status.paused {
        status.meeting_id.clone()
    } else {
        views.meeting.lock().unwrap().clone()
    };
    let Some(id) = id else {
        return Ok(serde_json::Value::Null);
    };
    let meeting = engine.store.lock().unwrap().meeting(&id).map_err(err)?;
    let mode = engine.preferences.lock().unwrap().translation_mode.clone();
    Ok(
        serde_json::json!({"meeting":{"id":id,"title":meeting.title},"runtime":{"recording":status.recording,"paused":status.paused,"elapsed_ms":status.elapsed_ms},"translation_mode":mode}),
    )
}

#[tauri::command]
pub fn companion_start(
    app: tauri::AppHandle,
    engine: tauri::State<AppState>,
    views: tauri::State<ViewState>,
    device: String,
    new_meeting: bool,
) -> Reply<String> {
    let status = engine.status.lock().unwrap().clone();
    if status.recording || status.paused {
        return Err("Finish or resume the active recording first".into());
    }
    let selected = views.meeting.lock().unwrap().clone();
    let id = match selected.filter(|_| !new_meeting) {
        Some(id) => id,
        None => engine.create_meeting("OmaScribe").map_err(err)?.id,
    };
    engine.start_recording(&id, &device, None).map_err(err)?;
    *views.meeting.lock().unwrap() = Some(id.clone());
    let _ = app.emit("view-meeting", &id);
    let _ = app.emit("companion-meeting", &id);
    Ok(id)
}

pub fn window_event(window: &tauri::Window, event: &tauri::WindowEvent) -> bool {
    if window.label() != "companion" {
        return false;
    }
    let state = window.state::<ViewState>();
    if let tauri::WindowEvent::Moved(p) = event {
        if let Ok(size) = window.inner_size() {
            state.prefs.lock().unwrap().companion_anchor =
                Some((p.x + size.width as i32, p.y + size.height as i32));
            state.mark_dirty();
        }
    }
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
        let mut prefs = state.prefs.lock().unwrap();
        prefs.companion_enabled = false;
        let _ = window.app_handle().emit("view-settings", prefs.clone());
        state.mark_dirty();
    }
    true
}

#[cfg(target_os = "linux")]
fn place_hyprland(width: u32, height: u32, initial: bool) {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return;
    }
    std::thread::spawn(move || {
        for _ in 0..15 {
            std::thread::sleep(std::time::Duration::from_millis(150));
            let Ok(out) = std::process::Command::new("hyprctl")
                .args(["clients", "-j"])
                .output()
            else {
                return;
            };
            let Ok(clients) = serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout) else {
                return;
            };
            if let Some(c) = clients.iter().find(|w| {
                w["pid"].as_u64() == Some(std::process::id() as u64)
                    && w["title"].as_str() == Some("OmaScribe Companion")
            }) {
                let Some(address) = c["address"].as_str().filter(|a| {
                    a.starts_with("0x") && a[2..].chars().all(|c| c.is_ascii_hexdigit())
                }) else {
                    return;
                };
                let target = format!("address:{address}");
                let code=format!("hl.dispatch(hl.dsp.window.float({{action='set',window='{target}'}})); {} hl.dispatch(hl.dsp.window.resize({{x={width},y={height},relative=false,window='{target}'}}))",if initial {format!("hl.dispatch(hl.dsp.window.pin({{action='set',window='{target}'}}));")} else {String::new()});
                let modern = std::process::Command::new("hyprctl")
                    .args(["eval", &code])
                    .output()
                    .is_ok_and(|o| {
                        o.status.success()
                            && !String::from_utf8_lossy(&o.stdout)
                                .to_lowercase()
                                .contains("error")
                            && !String::from_utf8_lossy(&o.stdout).contains("unknown request")
                    });
                if !modern {
                    let _ = std::process::Command::new("hyprctl")
                        .args(["dispatch", "setfloating", &target])
                        .output();
                    if initial && c["pinned"].as_bool() != Some(true) {
                        let _ = std::process::Command::new("hyprctl")
                            .args(["dispatch", "pin", &target])
                            .output();
                    }
                    let _ = std::process::Command::new("hyprctl")
                        .args([
                            "dispatch",
                            "resizewindowpixel",
                            &format!("exact {width} {height},{target}"),
                        ])
                        .output();
                }
                return;
            }
        }
    });
}
