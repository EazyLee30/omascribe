use anyhow::{bail, Context, Result};
use omascribe_core::{engine::default_root, Engine};
use std::{
    io::{BufWriter, Write},
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(|s| s.as_str()).unwrap_or("help");
    if command == "build-info" {
        println!("{}", omascribe_core::build_identity());
        return Ok(());
    }
    if command == "--hardware-probe" {
        println!(
            "{}",
            serde_json::to_string(&omascribe_core::hardware::native_inventory())?
        );
        return Ok(());
    }
    if command == "help" {
        println!("OmaScribe CLI\n  verify-update FILE SIGNATURE PUBLIC_KEY VERSION\n  diagnose\n  preserve-models\n  correct-terms MEETING_ID\n  translate MEETING_ID\n  models\n  hardware\n  network [downloads|recommendations] [on|off]\n  download-model MODEL_ID\n  diarize MEETING_ID\n  list\n  import AUDIO_PATH [TITLE]\n  record SECONDS [TITLE]\n  replay AUDIO_PATH SECONDS [--telemetry NEW_JSONL_PATH]\n  replay-settle AUDIO_PATH SECONDS SETTLE_SECONDS [--telemetry NEW_JSONL_PATH]\n  append MEETING_ID AUDIO_PATH\n  files MEETING_ID\n  retranscribe MEETING_ID\n  snapshot MEETING_ID\n  note MEETING_ID TEXT [note|focus|term|opinion]\n  generate MEETING_ID\n  settle MEETING_ID SECONDS\n  export MEETING_ID [--audio]\nAll command results are JSON. OMASCRIBE_DATA_DIR selects an isolated workspace.");
        return Ok(());
    }
    let arg = |i| args.get(i).context("缺少命令参数");
    if command == "verify-update" {
        omascribe_core::update_signature::verify(
            std::path::Path::new(arg(2)?),
            &std::fs::read_to_string(arg(3)?)?,
            &std::fs::read_to_string(arg(4)?)?,
            arg(5)?,
        )?;
        println!("{}", serde_json::json!({"verified":true,"version":arg(5)?}));
        return Ok(());
    }
    let engine = Engine::open(default_root())?;
    let replay = matches!(command, "replay" | "replay-settle");
    let settle_seconds = if command == "replay-settle" {
        let seconds = arg(4)?.parse::<u64>()?;
        anyhow::ensure!(
            (1..=600).contains(&seconds),
            "Use a 1–600 second settle budget"
        );
        seconds
    } else {
        0
    };
    let telemetry_arg = if command == "replay-settle" { 5 } else { 4 };
    // Opt-in replay diagnostics use counts/timing only, never speech or model text.
    // create_new preserves previous evidence instead of overwriting it.
    let mut replay_telemetry = if replay && args.len() > telemetry_arg {
        anyhow::ensure!(
            args.len() == telemetry_arg + 2 && args[telemetry_arg] == "--telemetry",
            "Use replay[-settle] AUDIO_PATH SECONDS [SETTLE_SECONDS] [--telemetry NEW_JSONL_PATH]"
        );
        Some(BufWriter::new(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(arg(telemetry_arg + 1)?)?,
        ))
    } else {
        None
    };
    let result = match command {
        "translate" => {
            omascribe_core::translation::run(&engine, arg(2)?)?;
            serde_json::to_value(engine.snapshot(arg(2)?)?)?
        }
        "inference-status" => serde_json::to_value(engine.inference_worker_status())?,
        "acceleration-components" => serde_json::to_value(engine.acceleration_components())?,
        "verify-acceleration" => {
            engine.verify_acceleration(arg(2)?)?;
            while engine.note_busy.load(std::sync::atomic::Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(200));
            }
            serde_json::to_value(engine.acceleration_status())?
        }
        "disable-acceleration" => {
            engine.disable_acceleration()?;
            serde_json::json!({"disabled":true})
        }
        "preserve-models" => {
            engine.preserve_bundled_models()?;
            serde_json::json!({"preserved":true})
        }
        "correct-terms" => serde_json::json!({"corrected":engine.correct_terminology(arg(2)?)?}),
        "models" => serde_json::to_value(engine.model_status())?,
        "hardware" => serde_json::to_value(omascribe_core::models::hardware(&engine.root))?,
        "network" => {
            let mut policy = engine.get_network_policy()?;
            if let Some(feature) = args.get(2) {
                let enabled = match arg(3)?.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => bail!("Use network [downloads|recommendations] [on|off]"),
                };
                match feature.as_str() {
                    "downloads" => policy.allow_model_downloads = enabled,
                    "recommendations" => policy.allow_online_recommendations = enabled,
                    _ => bail!("Use network [downloads|recommendations] [on|off]"),
                }
                policy = engine.set_network_policy(policy)?;
            }
            serde_json::to_value(policy)?
        }
        "download-model" => {
            let id = arg(2)?;
            engine.download_model(id)?;
            loop {
                let status = engine
                    .model_status()
                    .into_iter()
                    .find(|m| m.model.id == *id)
                    .context("Unknown model")?;
                if status.installed {
                    break serde_json::to_value(status)?;
                }
                if let Some(progress) = status.download {
                    if progress.state == "failed" || progress.state == "cancelled" {
                        bail!("{}", progress.error.unwrap_or(progress.state));
                    }
                }
                thread::sleep(Duration::from_millis(500));
            }
        }
        "diarize" => {
            engine.diarize(arg(2)?)?;
            while engine.status.lock().unwrap().diarization_busy {
                thread::sleep(Duration::from_millis(200));
            }
            let status = engine.status.lock().unwrap().clone();
            if status.diarization_status != "done" {
                bail!("{}", status.diarization_status);
            }
            serde_json::to_value(engine.snapshot(arg(2)?)?)?
        }
        "diagnose" => serde_json::to_value(engine.diagnostics())?,
        "list" => serde_json::to_value(engine.meetings()?)?,
        "files" => serde_json::to_value(engine.meeting_files(arg(2)?)?)?,
        "snapshot" => serde_json::to_value(engine.snapshot(arg(2)?)?)?,
        "import" | "record" | "replay" | "replay-settle" | "append" => {
            let m = if command == "append" {
                engine.snapshot(arg(2)?)?.meeting
            } else {
                engine.create_meeting(if replay {
                    "Public sample · paced replay"
                } else {
                    args.get(3)
                        .map(|s| s.as_str())
                        .unwrap_or("Local audio test")
                })?
            };
            let input = if command == "append" {
                Some(PathBuf::from(arg(3)?))
            } else if command == "import" {
                Some(PathBuf::from(arg(2)?))
            } else {
                None
            };
            let duration = if command == "record" {
                Some(arg(2)?.parse::<u64>()?)
            } else {
                None
            };
            if replay {
                engine.replay_audio(&m.id, PathBuf::from(arg(2)?), arg(3)?.parse()?)?;
                engine.start_scheduler();
            } else {
                engine.start_recording(&m.id, "auto", input)?;
            }
            let start = Instant::now();
            let mut stopped = false;
            let mut last_report = Instant::now();
            let mut capture_finished = None;
            loop {
                if duration.is_some_and(|d| start.elapsed().as_secs() >= d) && !stopped {
                    engine.stop_recording()?;
                    stopped = true;
                }
                let s = engine.snapshot(&m.id)?;
                if let Some(writer) = replay_telemetry.as_mut() {
                    let status = &s.runtime;
                    serde_json::to_writer(
                        &mut *writer,
                        &serde_json::json!({
                            "schema":1,"wall_ms":start.elapsed().as_millis() as u64,
                            "captured_ms":status.elapsed_ms,"processed_ms":status.processed_ms,
                            "recording":status.recording,"transcribing":status.transcribing,
                            "asr_rtf":status.asr_rtf,"segments":s.segments.len(),
                            "translations":s.translations.len(),"notes_cursor":s.meeting.notes_cursor,
                            "sections":s.suggestions.len(),"notes_busy":status.notes_busy,
                            "translation_busy":status.translation_busy,"has_error":status.error.is_some(),
                            "worker":engine.inference_worker_status(),
                        }),
                    )?;
                    writeln!(writer)?;
                    writer.flush()?;
                }
                if replay && last_report.elapsed() > Duration::from_secs(10) {
                    let status = &s.runtime;
                    eprintln!(
                        "{}",
                        serde_json::json!({"elapsed_s":start.elapsed().as_secs_f64(),"captured_ms":s.meeting.duration_ms,"processed_ms":status.processed_ms,"rtf":status.asr_rtf,"lag_ms":status.elapsed_ms.saturating_sub(status.processed_ms),"segments":s.segments.len(),"suggestions":s.suggestions.len(),"notes_busy":status.notes_busy,"notes_status":status.notes_status,"error":status.error})
                    );
                    last_report = Instant::now();
                }
                if !s.runtime.recording && !s.runtime.transcribing {
                    let finished = *capture_finished.get_or_insert_with(Instant::now);
                    let pending = s.meeting.auto_notes && omascribe_core::notes::needs_update(&s)
                        || omascribe_core::translation::pending(&s);
                    if settle_seconds == 0
                        || !pending && !engine.note_busy.load(std::sync::atomic::Ordering::SeqCst)
                        || finished.elapsed() >= Duration::from_secs(settle_seconds)
                    {
                        break;
                    }
                }
                if start.elapsed() > Duration::from_secs(14_500) {
                    engine.shutdown();
                    bail!("处理超时，音频已保留");
                }
                thread::sleep(Duration::from_millis(200));
            }
            let snapshot = engine.snapshot(&m.id)?;
            if settle_seconds > 0 {
                let pending = snapshot.meeting.auto_notes
                    && omascribe_core::notes::needs_update(&snapshot)
                    || omascribe_core::translation::pending(&snapshot);
                serde_json::json!({"settle":{
                    "budget_seconds":settle_seconds,
                    "completed":!pending && !engine.note_busy.load(std::sync::atomic::Ordering::SeqCst),
                    "wait_ms":capture_finished.map(|at|at.elapsed().as_millis() as u64),
                    "wall_ms":start.elapsed().as_millis() as u64,
                },"snapshot":snapshot})
            } else {
                serde_json::to_value(snapshot)?
            }
        }
        "retranscribe" => {
            let id = arg(2)?;
            engine.retranscribe(id)?;
            let start = Instant::now();
            while engine.status.lock().unwrap().transcribing {
                if start.elapsed() > Duration::from_secs(14_500) {
                    engine.shutdown();
                    bail!("转写超时，原始录音仍已保存");
                }
                thread::sleep(Duration::from_millis(200));
            }
            serde_json::to_value(engine.snapshot(id)?)?
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
        "settle" => {
            let id = arg(2)?;
            let seconds = arg(3)?.parse::<u64>()?;
            anyhow::ensure!(
                (1..=600).contains(&seconds),
                "Use a 1–600 second settle budget"
            );
            anyhow::ensure!(
                engine.snapshot(id)?.meeting.auto_notes,
                "Enable automatic minutes before settling"
            );
            engine.focus_meeting(id)?;
            engine.start_scheduler();
            let started = Instant::now();
            loop {
                let snapshot = engine.snapshot(id)?;
                let pending = omascribe_core::notes::needs_update(&snapshot)
                    || omascribe_core::translation::pending(&snapshot);
                if !pending && !engine.note_busy.load(std::sync::atomic::Ordering::SeqCst) {
                    break serde_json::to_value(snapshot)?;
                }
                if started.elapsed() >= Duration::from_secs(seconds) {
                    engine.shutdown();
                    bail!("Scheduled work did not settle within the explicit budget; prior content is retained");
                }
                thread::sleep(Duration::from_millis(200));
            }
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
