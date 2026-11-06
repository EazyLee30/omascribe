//! Fixed DOM probes for isolated native UI tests. Absent from production builds.
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};
use tauri::{Manager, State};
use tokio::sync::oneshot;

#[derive(Default)]
pub struct Probes(Mutex<HashMap<u64, (String, oneshot::Sender<Value>)>>);
static NEXT: AtomicU64 = AtomicU64::new(1);

#[tauri::command]
pub async fn ui_test_window_probe(
    app: tauri::AppHandle,
    probes: State<'_, Probes>,
    label: String,
    expand: bool,
) -> Result<Value, String> {
    if !["main", "companion", "transcript", "captions"].contains(&label.as_str()) {
        return Err("Unknown native test window".into());
    }
    let window = app
        .get_webview_window(&label)
        .ok_or("Test window is not open")?;
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let (send, receive) = oneshot::channel();
    probes.0.lock().unwrap().insert(id, (label, send));
    let request = json!({"id":id,"expand":expand}).to_string();
    let script = include_str!("ui_test_probe.js").replace("__PROBE_REQUEST__", &request);
    if let Err(error) = window.eval(script) {
        probes.0.lock().unwrap().remove(&id);
        return Err(error.to_string());
    }
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), receive)
        .await
        .map_err(|_| "Native window probe timed out".to_owned())
        .and_then(|value| value.map_err(|e| e.to_string()));
    probes.0.lock().unwrap().remove(&id);
    result
}

#[tauri::command]
pub fn ui_test_window_result(
    window: tauri::WebviewWindow,
    probes: State<Probes>,
    request_id: u64,
    snapshot: Value,
) -> Result<(), String> {
    let mut pending = probes.0.lock().unwrap();
    if snapshot.to_string().len() > 8192
        || pending
            .get(&request_id)
            .is_none_or(|(label, _)| label != window.label())
    {
        return Err("Invalid native window probe response".into());
    }
    let (_, send) = pending.remove(&request_id).ok_or("Unknown native probe")?;
    let _ = send.send(snapshot);
    Ok(())
}
