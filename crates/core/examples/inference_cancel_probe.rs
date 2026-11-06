//! One real manual-minutes cancellation observation; never a quality or native-exit verdict.
//! Usage: inference_cancel_probe OWNED_ROOT MEETING_ID|--init
//! The outer runner creates the fresh root and marker, seeds public content, pins
//! the existing model/runtime, and independently observes native process exits.
use anyhow::{ensure, Context, Result};
use omascribe_core::Engine;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{atomic::Ordering, Arc},
    thread,
    time::{Duration, Instant},
};

const MARKER: &[u8] = b"omascribe-public-cancel-v1\n";

struct Shutdown(Option<Arc<Engine>>);
impl Shutdown {
    fn close(&mut self) -> Option<bool> {
        self.0.take().map(|engine| {
            engine.shutdown();
            engine.wait_for_shutdown_tasks(Duration::from_secs(5))
        })
    }
}
impl Drop for Shutdown {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn owned_root(argument: &str) -> Result<PathBuf> {
    let root = Path::new(argument);
    ensure!(root.is_absolute(), "OWNED_ROOT must be absolute");
    ensure!(
        fs::symlink_metadata(root)?.file_type().is_dir(),
        "OWNED_ROOT must be an existing real directory"
    );
    let marker = root.join(".inference-cancel-probe");
    let metadata = fs::symlink_metadata(&marker)?;
    ensure!(
        metadata.file_type().is_file() && metadata.len() == MARKER.len() as u64,
        "Missing task ownership marker"
    );
    ensure!(fs::read(marker)? == MARKER, "Invalid task ownership marker");
    Ok(root.canonicalize()?)
}

fn state(engine: &Engine) -> Value {
    let status = engine.status.lock().unwrap().clone();
    json!({
        "note_busy":engine.note_busy.load(Ordering::SeqCst),
        "notes_status":status.notes_status,
        "llm_pid":engine.llm_pid.load(Ordering::SeqCst),
        "worker":engine.inference_worker_status(),
    })
}

fn observe(engine: &Arc<Engine>, meeting: &str) -> Result<Value> {
    engine
        .snapshot(meeting)
        .context("Public meeting is missing")?;
    let started = Instant::now();
    engine.generate_notes(meeting)?;
    let mut trigger = None;
    let mut reason = "generation_finished_before_observed_token";
    let mut cancel_call_ms = None;
    let mut cancel_wait_ms = None;
    loop {
        if started.elapsed() >= Duration::from_secs(60) {
            reason = "no_observed_token_within_60_seconds";
            break;
        }
        if !engine.note_busy.load(Ordering::SeqCst) {
            break;
        }
        // The production SSE callback publishes this count only after content
        // arrives. request_started alone precedes the HTTP send and is weaker.
        let characters = {
            let status = engine.status.lock().unwrap();
            status
                .notes_status
                .strip_prefix("writer:writing:")
                .and_then(|count| count.parse::<usize>().ok())
                .unwrap_or(0)
        };
        let pid = engine.llm_pid.load(Ordering::SeqCst);
        if characters > 0 && pid > 0 && engine.note_busy.load(Ordering::SeqCst) {
            let cancelled_at = Instant::now();
            let boundary = json!({
                "at":chrono::Utc::now().to_rfc3339(),
                "elapsed_ms":started.elapsed().as_millis(),
                "pid":pid,"characters":characters,
                "signal":"writer:writing:positive+note_busy+llm_pid",
            });
            // No status/pool guard or file I/O remains held at the actual call.
            engine.cancel_notes();
            cancel_call_ms = Some(cancelled_at.elapsed().as_millis());
            trigger = Some(boundary);
            let waiting = Instant::now();
            while engine.note_busy.load(Ordering::SeqCst)
                && waiting.elapsed() < Duration::from_secs(5)
            {
                thread::sleep(Duration::from_millis(5));
            }
            cancel_wait_ms = Some(cancelled_at.elapsed().as_millis());
            reason = "cancel_called_after_observed_stream_content";
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    // Capture this BEFORE global shutdown: its cleanup must not hide a failed
    // cancel_notes operation. The outer gate also checks the completed=false
    // request, matching job, cancelled diagnostic and actual PID identity exit.
    let before_shutdown = state(engine);
    Ok(json!({
        "status":if trigger.is_some() { "triggered" } else { "indeterminate" },
        "reason":reason,"trigger":trigger,
        "elapsed_before_shutdown_ms":started.elapsed().as_millis(),
        "cancel_call_ms":cancel_call_ms,
        "cancel_to_busy_observation_ms":cancel_wait_ms,
        "before_shutdown":before_shutdown,
    }))
}

fn run() -> Result<Value> {
    let arguments: Vec<String> = std::env::args().collect();
    ensure!(
        arguments.len() == 3,
        "Usage: inference_cancel_probe OWNED_ROOT MEETING_ID|--init"
    );
    let root = owned_root(&arguments[1])?;
    let initialize = arguments[2] == "--init";
    let engine = Engine::open(root.clone())?;
    let mut shutdown = Shutdown(Some(engine.clone()));
    // No scheduler, recording, translation call, preference mutation or retries.
    let result = if initialize {
        Ok(json!({"status":"initialized","initialized":true,"before_shutdown":state(&engine)}))
    } else {
        observe(&engine, &arguments[2])
    };
    let mut report = match result {
        Ok(report) => report,
        Err(error) => {
            json!({"status":"error","error":format!("{error:#}"),"before_shutdown":state(&engine)})
        }
    };
    let drain = shutdown.close();
    report["schema"] = json!(1);
    report["kind"] = json!("production-manual-minutes-cancellation");
    report["build"] = omascribe_core::build_identity();
    report["task_drain"] = json!(drain);
    report["after_shutdown"] = state(&engine);
    report["native_process_exit"] = json!("not_verified_by_probe");
    report["quality"] = json!("not_evaluated");
    report["limits"] = json!("A trigger or drained task is not cancellation success; independently check the incomplete request, cancelled outcome, unchanged content and native PID exit. An early completed response is indeterminate.");
    let name = if initialize {
        "cancel-probe-init.json"
    } else {
        "cancel-probe.json"
    };
    fs::write(root.join(name), serde_json::to_vec_pretty(&report)?)?;
    Ok(report)
}

fn main() -> ExitCode {
    match run() {
        Ok(report) => {
            println!("{report}");
            if report["status"] == "error" {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            println!(
                "{}",
                json!({"status":"error","error":format!("{error:#}"),"native_process_exit":"not_verified_by_probe"})
            );
            ExitCode::FAILURE
        }
    }
}
