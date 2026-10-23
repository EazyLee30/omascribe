use anyhow::{bail, Context, Result};
use omascribe_core::{engine::default_root, Engine};
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(|s| s.as_str()).unwrap_or("help");
    if command == "help" {
        println!("OmaScribe CLI\n  diagnose\n  list\n  import AUDIO_PATH [TITLE]\n  record SECONDS [TITLE]\n  replay AUDIO_PATH SECONDS\n  snapshot MEETING_ID\n  note MEETING_ID TEXT [note|focus|term|opinion]\n  generate MEETING_ID\n  export MEETING_ID [--audio]\nAll command results are JSON. OMASCRIBE_DATA_DIR selects an isolated workspace.");
        return Ok(());
    }
    let engine = Engine::open(default_root())?;
    let arg = |i| args.get(i).context("缺少命令参数");
    let result = match command {
        "diagnose" => serde_json::to_value(engine.diagnostics())?,
        "list" => serde_json::to_value(engine.meetings()?)?,
        "snapshot" => serde_json::to_value(engine.snapshot(arg(2)?)?)?,
        "import" | "record" | "replay" => {
            let m = engine.create_meeting(if command == "replay" {
                "Public sample · paced replay"
            } else {
                args.get(3)
                    .map(|s| s.as_str())
                    .unwrap_or("Local audio test")
            })?;
            let input = if command == "import" {
                Some(PathBuf::from(arg(2)?))
            } else {
                None
            };
            let duration = if command == "record" {
                Some(arg(2)?.parse::<u64>()?)
            } else {
                None
            };
            if command == "replay" {
                engine.replay_audio(&m.id, PathBuf::from(arg(2)?), arg(3)?.parse()?)?;
                engine.start_scheduler();
            } else {
                engine.start_recording(&m.id, "auto", input)?;
            }
            let start = Instant::now();
            let mut stopped = false;
            let mut last_report = Instant::now();
            loop {
                if duration.is_some_and(|d| start.elapsed().as_secs() >= d) && !stopped {
                    engine.stop_recording()?;
                    stopped = true;
                }
                let s = engine.snapshot(&m.id)?;
                if command == "replay" && last_report.elapsed() > Duration::from_secs(10) {
                    let status = &s.runtime;
                    eprintln!(
                        "{}",
                        serde_json::json!({"elapsed_s":start.elapsed().as_secs_f64(),"captured_ms":s.meeting.duration_ms,"processed_ms":status.processed_ms,"rtf":status.asr_rtf,"lag_ms":status.elapsed_ms.saturating_sub(status.processed_ms),"segments":s.segments.len(),"suggestions":s.suggestions.len(),"notes_busy":status.notes_busy,"notes_status":status.notes_status,"error":status.error})
                    );
                    last_report = Instant::now();
                }
                if !s.runtime.recording && !s.runtime.transcribing {
                    break;
                }
                if start.elapsed() > Duration::from_secs(14_500) {
                    engine.shutdown();
                    bail!("处理超时，音频已保留");
                }
                thread::sleep(Duration::from_millis(200));
            }
            serde_json::to_value(engine.snapshot(&m.id)?)?
        }
        "note" => serde_json::to_value(engine.save_note(
            arg(2)?,
            None,
            arg(3)?,
            args.get(4).map(|s| s.as_str()).unwrap_or("focus"),
            0,
        )?)?,
        "generate" => {
            engine.generate_notes(arg(2)?)?;
            while engine.note_busy.load(std::sync::atomic::Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(200));
            }
            serde_json::to_value(engine.snapshot(arg(2)?)?)?
        }
        "export" => {
            serde_json::json!({"path":engine.export(arg(2)?,args.iter().any(|a|a=="--audio"))?})
        }
        _ => bail!("未知命令；运行 omascribe-cli help"),
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    engine.shutdown();
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("{}", serde_json::json!({"error":format!("{e:#}")}));
        std::process::exit(1);
    }
}
