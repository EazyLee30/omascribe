use crate::views::CaptionPreferences;
use std::{sync::Mutex, time::Instant};
use tauri::{Emitter, Manager};

static LAST: Mutex<Option<(String, Instant)>> = Mutex::new(None);

pub fn apply(
    app: &tauri::AppHandle,
    _: &std::path::Path,
    p: &CaptionPreferences,
) -> Result<(), String> {
    if !p.enabled {
        if let Some(window) = app.get_webview_window("captions") {
            window.hide().map_err(|e| e.to_string())?;
        }
        *LAST.lock().unwrap() = None;
        return Ok(());
    }
    let window = match app.get_webview_window("captions") {
        Some(window) => window,
        None => tauri::WebviewWindowBuilder::new(
            app,
            "captions",
            tauri::WebviewUrl::App("captions.html".into()),
        )
        .title("OmaScribe — Captions")
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .visible(false)
        .build()
        .map_err(|e| e.to_string())?,
    };
    let monitors = app.available_monitors().map_err(|e| e.to_string())?;
    let monitor = if p.monitor >= 0 {
        monitors.get(p.monitor as usize).cloned()
    } else {
        None
    }
    .or_else(|| {
        app.get_webview_window("main")
            .and_then(|w| w.current_monitor().ok().flatten())
    })
    .or_else(|| app.primary_monitor().ok().flatten());
    if let Some(monitor) = monitor {
        let size = monitor.size().to_logical::<f64>(monitor.scale_factor());
        let origin = monitor.position();
        let width = (size.width * p.width_percent as f64 / 100.)
            .min(size.width - 16.)
            .max(1.);
        let height = (p.font_size as f64 * 1.5 * p.lines as f64 + 28.).min(size.height);
        let margin = (p.margin as f64).min((size.height - height).max(0.));
        let x = origin.x + ((size.width - width) * 0.5 * monitor.scale_factor()) as i32;
        let y = origin.y
            + ((if p.position == "top" {
                margin
            } else {
                size.height - height - margin
            }) * monitor.scale_factor()) as i32;
        window
            .set_size(tauri::LogicalSize::new(width, height))
            .map_err(|e| e.to_string())?;
        window
            .set_position(tauri::PhysicalPosition::new(x, y))
            .map_err(|e| e.to_string())?;
    }
    window
        .set_ignore_cursor_events(p.click_through)
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn render(app: &tauri::AppHandle, text: &str, p: &CaptionPreferences) {
    let Some(window) = app.get_webview_window("captions") else {
        return;
    };
    let mut last = LAST.lock().unwrap();
    if last.as_ref().is_none_or(|(old, _)| old != text) {
        *last = Some((text.into(), Instant::now()));
    }
    let idle = last
        .as_ref()
        .is_some_and(|(_, time)| p.idle_seconds > 0 && time.elapsed().as_secs() >= p.idle_seconds);
    if text.is_empty() || !p.enabled || idle {
        let _ = window.hide();
    } else {
        let _ = window.emit(
            "caption-text",
            serde_json::json!({"text": text, "preferences": p}),
        );
        if !window.is_visible().unwrap_or(false) {
            let _ = window.show();
        }
    }
}
