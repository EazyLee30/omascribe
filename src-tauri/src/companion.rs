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
    .always_on_top(true)
    .visible_on_all_workspaces(true)
    .skip_taskbar(true)
    .focused(false)
    .accept_first_mouse(true)
    .visible(false);
    // On Wayland a non-resizable window rejects compositor-driven sizes, so the
    // pet kept the WebView's natural 710x427: an invisible transparent block
    // that swallowed clicks and hid whatever was behind it, and expanding
    // could not grow it either. The surface is undecorated, so the user has no
    // resize affordance anyway; allow programmatic sizes and let the CSS size
    // the pet itself.
    #[cfg(target_os = "linux")]
    let w = w.resizable(true);
    #[cfg(not(target_os = "linux"))]
    let w = w.resizable(false);
    let w = w.build().map_err(err)?;
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
    {
        // Wayland clients cannot place themselves, so the compositor moves the
        // window. Restore the saved anchor there too, in logical coordinates
        // with the pet kept fully on-screen.
        let anchor = app
            .state::<ViewState>()
            .prefs
            .lock()
            .unwrap()
            .companion_anchor;
        place_hyprland(100, 112, true, anchor);
    }
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
    {
        // Growing or shrinking must keep the pet anchored at the same corner,
        // otherwise expanding lands somewhere else on screen and the saved
        // position becomes meaningless.
        sync_anchor(&app.state::<ViewState>());
        let anchor = app
            .state::<ViewState>()
            .prefs
            .lock()
            .unwrap()
            .companion_anchor;
        place_hyprland(width as u32, height as u32, false, anchor);
    }
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
    // Wayland compositors move the surface, and Tauri's `Moved` event then
    // reports a stale or unscaled position: the pet saved `[200,224]` on a
    // 1.6x panel while actually sitting at 700,1500, so the position never
    // survived a restart. Linux reconciles the anchor from the compositor
    // instead; see `sync_anchor`.
    #[cfg(not(target_os = "linux"))]
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
    // Hiding is the reliable "user finished moving it" signal on Wayland,
    // where no `Moved` event arrives for a compositor-driven move.
    #[cfg(target_os = "linux")]
    if matches!(
        event,
        tauri::WindowEvent::Destroyed | tauri::WindowEvent::CloseRequested { .. }
    ) {
        sync_anchor(&state);
    }
    true
}

/// Keep the stored anchor in step with the pet's real position.
///
/// Wayland reports no `Moved` event for a compositor-driven move, so the saved
/// position would only refresh when the pet happened to be hidden. Polling the
/// compositor on a background thread avoids blocking the GTK main thread, and
/// only runs while the pet is actually visible.
#[cfg(target_os = "linux")]
pub fn start_anchor_sync(app: &tauri::AppHandle) {
    use tauri::Manager;
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(3));
        let Some(window) = app.get_webview_window("companion") else {
            continue;
        };
        if window.is_visible().unwrap_or(false) {
            sync_anchor(&app.state::<ViewState>());
        }
    });
}

/// Record the pet's real position from the compositor.
///
/// Tauri's own position getters disagree with Hyprland's logical coordinates on
/// a fractionally scaled output, and the `Moved` event only fires for
/// client-side moves. Hyprland reports `at` in the same logical space the
/// restore path uses, so store the pet's bottom-right corner from there.
#[cfg(target_os = "linux")]
pub fn sync_anchor(views: &ViewState) {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return;
    }
    let Ok(out) = std::process::Command::new("hyprctl")
        .args(["clients", "-j"])
        .output()
    else {
        return;
    };
    let Ok(clients) = serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout) else {
        return;
    };
    let Some(window) = clients.iter().find(|w| {
        w["pid"].as_u64() == Some(std::process::id() as u64)
            && w["title"].as_str() == Some("OmaScribe Companion")
    }) else {
        return;
    };
    let (Some(x), Some(y), Some(width), Some(height)) = (
        window["at"][0].as_i64(),
        window["at"][1].as_i64(),
        window["size"][0].as_i64(),
        window["size"][1].as_i64(),
    ) else {
        return;
    };
    if width <= 0 || height <= 0 {
        return;
    }
    let anchor = ((x + width) as i32, (y + height) as i32);
    let mut prefs = views.prefs.lock().unwrap();
    if prefs.companion_anchor != Some(anchor) {
        prefs.companion_anchor = Some(anchor);
        drop(prefs);
        views.mark_dirty();
    }
}

#[cfg(target_os = "linux")]
fn place_hyprland(width: u32, height: u32, initial: bool, anchor: Option<(i32, i32)>) {
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
                // The pet is an undecorated, transparent surface. Two
                // compositor defaults draw furniture around it: an active
                // border (a visible blue frame on Omarchy) and a drop shadow.
                // Neither has a Tauri builder option, and `hyprctl setprop`
                // has no batch form, so both are applied to this exact window
                // address through the Lua eval path already used here.
                //
                // The saved anchor is the pet's bottom-right corner, matching
                // what `Moved` records, so place the window by its own size
                // from there and keep it on the requested monitor.
                let move_code = anchor.map_or_else(String::new, |(x, y)| {
                    format!(
                        "; hl.dispatch(hl.dsp.window.move({{x={},y={},relative=false,window='{target}'}}))",
                        x - width as i32,
                        y - height as i32
                    )
                });
                let code=format!("hl.dispatch(hl.dsp.window.set_prop({{window='{target}',prop='border_size',value=0}})); hl.dispatch(hl.dsp.window.set_prop({{window='{target}',prop='no_shadow',value=1}})); hl.dispatch(hl.dsp.window.float({{action='set',window='{target}'}})); {} hl.dispatch(hl.dsp.window.resize({{x={width},y={height},relative=false,window='{target}'}})){move_code}",if initial {format!("hl.dispatch(hl.dsp.window.pin({{action='set',window='{target}'}}));")} else {String::new()});
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
