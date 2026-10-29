use anyhow::{bail, Context, Result};
use omascribe_core::{engine::default_root, Engine};
use std::{
    io::{BufWriter, Write},
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
#[derive(Default, Debug, PartialEq, Eq)]
struct ReplayOptions {
    telemetry: Option<PathBuf>,
    idle_seconds: u64,
}
fn replay_options(arguments: &[String], settle: bool) -> Result<ReplayOptions> {
    let mut result = ReplayOptions::default();
    let mut idle_seen = false;
    let mut index = 0;
    while index < arguments.len() {
        let value = arguments
            .get(index + 1)
            .context("Replay option requires a value")?;
        match arguments[index].as_str() {
            "--telemetry" if result.telemetry.is_none() => result.telemetry = Some(value.into()),
            "--idle-observe-seconds" if settle && !idle_seen => {
                result.idle_seconds = value.parse()?;
                anyhow::ensure!(
                    result.idle_seconds <= 60,
                    "Use a 0–60 second idle observation"
                );
                idle_seen = true;
            }
            _ => bail!("Unknown, repeated or incompatible replay option"),
        }
        index += 2;
    }
    Ok(result)
}
fn replay_row(
    writer: &mut Option<BufWriter<std::fs::File>>,
    engine: &Engine,
    snapshot: &omascribe_core::Snapshot,
    start: Instant,
    phase: &str,
    idle_elapsed_ms: Option<u64>,
) -> Result<()> {
    if let Some(writer) = writer.as_mut() {
        let status = &snapshot.runtime;
        serde_json::to_writer(
            &mut *writer,
            &serde_json::json!({
                "schema":1,"unix_ms":chrono::Utc::now().timestamp_millis(),
                "wall_ms":start.elapsed().as_millis() as u64,"phase":phase,"idle_elapsed_ms":idle_elapsed_ms,
                "captured_ms":status.elapsed_ms,"processed_ms":status.processed_ms,
                "recording":status.recording,"transcribing":status.transcribing,
                "asr_rtf":status.asr_rtf,"segments":snapshot.segments.len(),
                "translations":snapshot.translations.len(),"notes_cursor":snapshot.meeting.notes_cursor,
                "sections":snapshot.suggestions.len(),"notes_busy":status.notes_busy,
                "translation_busy":status.translation_busy,"has_error":status.error.is_some(),
                "worker":engine.inference_worker_status(),
            }),
        )?;
        writeln!(writer)?;
        writer.flush()?;
    }
    Ok(())
}
fn settled(engine: &Engine, snapshot: &omascribe_core::Snapshot) -> bool {
    !(snapshot.meeting.auto_notes && omascribe_core::notes::needs_update(snapshot)
        || omascribe_core::translation::pending(snapshot)
        || engine.note_busy.load(std::sync::atomic::Ordering::SeqCst))
}
fn replay_data(snapshot: &omascribe_core::Snapshot) -> Result<Vec<u8>> {
    let translations: std::collections::BTreeMap<_, _> = snapshot.translations.iter().collect();
    Ok(serde_json::to_vec(&(
        snapshot.meeting.run,
        snapshot.meeting.duration_ms,
        snapshot.meeting.notes_cursor,
        &snapshot.notes,
        &snapshot.segments,
        &snapshot.suggestions,
        translations,
    ))?)
}
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
        println!("OmaScribe CLI\n  verify-update FILE SIGNATURE PUBLIC_KEY VERSION\n  diagnose\n  preserve-models\n  correct-terms MEETING_ID\n  translate MEETING_ID\n  models\n  hardware\n  network [downloads|recommendations] [on|off]\n  download-model MODEL_ID\n  verify-model MODEL_ID [REPEATS]\n  diarize MEETING_ID\n  list\n  import AUDIO_PATH [TITLE]\n  record SECONDS [TITLE]\n  replay AUDIO_PATH SECONDS [--telemetry NEW_JSONL_PATH]\n  replay-settle AUDIO_PATH SECONDS SETTLE_SECONDS [--telemetry NEW_JSONL_PATH] [--idle-observe-seconds 0..60]\n  append MEETING_ID AUDIO_PATH\n  files MEETING_ID\n  retranscribe MEETING_ID\n  snapshot MEETING_ID\n  note MEETING_ID TEXT [note|focus|term|opinion]\n  edit-note MEETING_ID NOTE_ID TEXT [kind=document]\n  generate MEETING_ID\n  settle MEETING_ID SECONDS\n  export MEETING_ID [--audio]\nAll command results are JSON. OMASCRIBE_DATA_DIR selects an isolated workspace.");
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
    let replay_options = if replay {
        replay_options(
            args.get(telemetry_arg..).unwrap_or_default(),
            settle_seconds > 0,
        )?
    } else {
        ReplayOptions::default()
    };
    let mut replay_telemetry = replay_options
        .telemetry
        .as_ref()
        .map(|path| {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map(BufWriter::new)
        })
        .transpose()?;
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
        "verify-model" => {
            let id = args.get(2).context("verify-model MODEL_ID [REPEATS]")?;
            let repeats: usize = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(1);
            anyhow::ensure!(
                (1..=8).contains(&repeats),
                "Use 1 to 8 verification repeats"
            );
            let rows = (0..repeats)
                .map(|_| engine.verify_model_integrity(id))
                .collect::<Result<Vec<_>>>()?;
            serde_json::to_value(rows)?
        }
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
                replay_row(
                    &mut replay_telemetry,
                    &engine,
                    &s,
                    start,
                    if s.runtime.recording || s.runtime.transcribing {
                        "capture"
                    } else {
                        "settle"
                    },
                    None,
                )?;
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
                // Preserve settlement timing before idle sampling; idle residency
                // must not inflate or hide the original completion budget.
                let completed = settled(&engine, &snapshot);
                let settlement = serde_json::json!({
                    "budget_seconds":settle_seconds,"completed":completed,
                    "wait_ms":capture_finished.map(|at|at.elapsed().as_millis() as u64),
                    "wall_ms":start.elapsed().as_millis() as u64,
                });
                let idle_observation = if completed && replay_options.idle_seconds > 0 {
                    let settled_data = replay_data(&snapshot)?;
                    let idle_start = Instant::now();
                    let started_wall_ms = start.elapsed().as_millis() as u64;
                    let started_unix_ms = chrono::Utc::now().timestamp_millis();
                    let worker_at_start = engine.inference_worker_status();
                    let mut queue_unchanged = true;
                    let mut worker_stopped_after_ms = None;
                    loop {
                        let current = engine.snapshot(&m.id)?;
                        let worker = engine.inference_worker_status();
                        let elapsed_ms = idle_start.elapsed().as_millis() as u64;
                        queue_unchanged &= settled(&engine, &current)
                            && replay_data(&current)? == settled_data
                            && current.segments.len() == snapshot.segments.len()
                            && current.translations.len() == snapshot.translations.len()
                            && current.meeting.notes_cursor == snapshot.meeting.notes_cursor
                            && current.suggestions.len() == snapshot.suggestions.len()
                            && worker.starts == worker_at_start.starts
                            && worker.reuses == worker_at_start.reuses;
                        if worker.idle_pid.is_none() && !worker.leased {
                            worker_stopped_after_ms.get_or_insert(elapsed_ms);
                        }
                        replay_row(
                            &mut replay_telemetry,
                            &engine,
                            &current,
                            start,
                            "idle",
                            Some(elapsed_ms),
                        )?;
                        if idle_start.elapsed() >= Duration::from_secs(replay_options.idle_seconds)
                        {
                            break;
                        }
                        thread::sleep(Duration::from_millis(200));
                    }
                    serde_json::json!({"requested_seconds":replay_options.idle_seconds,
                        "elapsed_ms":idle_start.elapsed().as_millis() as u64,
                        "started_wall_ms":started_wall_ms,"started_unix_ms":started_unix_ms,
                        "completed":true,"queue_unchanged":queue_unchanged,
                        "worker_at_start":worker_at_start,"worker_at_end":engine.inference_worker_status(),
                        "worker_stopped_after_ms":worker_stopped_after_ms})
                } else {
                    serde_json::Value::Null
                };
                serde_json::json!({"settle":settlement,"snapshot":snapshot,"idle_observation":idle_observation})
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
        "edit-note" => serde_json::to_value(engine.save_note(
            arg(2)?,
            Some(arg(3)?.clone()),
            arg(4)?,
            args.get(5).map(|s| s.as_str()).unwrap_or("document"),
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

#[cfg(test)]
mod replay_option_tests {
    use super::*;
    #[test]
    fn idle_data_signature_ignores_map_order_and_runtime_clocks_but_detects_edits() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let meeting = engine.create_meeting("Idle data check")?;
        let mut first = engine.snapshot(&meeting.id)?;
        for id in ["first", "second"] {
            first.translations.insert(
                id.into(),
                omascribe_core::translation::Translation {
                    segment_id: id.into(),
                    revision: 1,
                    target: "zh".into(),
                    model: "fixture".into(),
                    text: id.into(),
                    cache_key: "fixed".into(),
                },
            );
        }
        let mut second = first.clone();
        second.translations.clear();
        for id in ["second", "first"] {
            second
                .translations
                .insert(id.into(), first.translations[id].clone());
        }
        second.runtime.notes_elapsed_ms = 1000;
        assert_eq!(replay_data(&first)?, replay_data(&second)?);
        second.translations.get_mut("first").unwrap().text = "changed".into();
        assert_ne!(replay_data(&first)?, replay_data(&second)?);
        Ok(())
    }
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).into()).collect()
    }
    #[test]
    fn existing_and_opt_in_replay_options_are_bounded_and_unambiguous() -> Result<()> {
        assert_eq!(replay_options(&[], false)?, ReplayOptions::default());
        let options = replay_options(
            &args(&["--telemetry", "test.jsonl", "--idle-observe-seconds", "32"]),
            true,
        )?;
        assert_eq!(options.telemetry, Some("test.jsonl".into()));
        assert_eq!(options.idle_seconds, 32);
        assert_eq!(
            replay_options(
                &args(&["--idle-observe-seconds", "32", "--telemetry", "test.jsonl"]),
                true
            )?,
            options
        );
        for values in [
            vec!["--telemetry"],
            vec!["--telemetry", "one", "--telemetry", "two"],
            vec!["--idle-observe-seconds", "61"],
            vec!["--idle-observe-seconds", "-1"],
            vec!["--idle-observe-seconds", "0", "--idle-observe-seconds", "1"],
            vec!["--unknown", "value"],
        ] {
            assert!(replay_options(&args(&values), true).is_err(), "{values:?}");
        }
        assert!(replay_options(&args(&["--idle-observe-seconds", "1"]), false).is_err());
        Ok(())
    }
}
