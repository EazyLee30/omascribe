//! Offline production-path probe. Usage: asr_context_probe NEW_ROOT PUBLIC_WAV stream|legacy
//! Provide task-owned OMASCRIBE_MODELS_DIR (with zipformer/) and OMASCRIBE_RUNTIME_DIR.
//! Stream tests the unset default; legacy requires the explicit override 0. No scheduler/LLM.
//! Keep the root and raw evidence until an outer process-tree observer confirms exit.
use anyhow::{ensure, Context, Result};
use omascribe_core::{Engine, Snapshot};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::Path,
    sync::{atomic::Ordering, Arc},
    thread,
    time::{Duration, Instant},
};

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
fn write(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
fn remaining(deadline: Instant) -> Result<()> {
    ensure!(
        Instant::now() < deadline,
        "40-second probe budget exhausted"
    );
    Ok(())
}
fn wait(engine: &Engine, id: &str, deadline: Instant) -> Result<()> {
    loop {
        remaining(deadline)?;
        let status = engine.status.lock().unwrap().clone();
        ensure!(status.error.is_none(), "ASR failed: {:?}", status.error);
        let done = engine
            .active
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|active| {
                active.id == id
                    && active.capture_done.load(Ordering::SeqCst)
                    && active.worker_done.load(Ordering::SeqCst)
            });
        if done && !status.recording && !status.transcribing {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
}
fn inspect(
    engine: &Engine,
    id: &str,
    stage: &str,
    expected: &[u8],
    mode: &str,
    report: &mut Value,
) -> Result<Snapshot> {
    let snapshot = engine.snapshot(id)?;
    write(
        &engine.root.join(format!("{stage}-snapshot.json")),
        &snapshot,
    )?;
    let audio = engine.audio_dir(id)?;
    let config: Value = serde_json::from_slice(&fs::read(audio.join("asr-config.json"))?)?;
    write(&engine.root.join(format!("{stage}-config.json")), &config)?;
    let words = fs::read_to_string(audio.join("hotwords.txt"))?;
    fs::write(engine.root.join(format!("{stage}-hotwords.txt")), &words)?;
    let mut files = fs::read_dir(&audio)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    files.retain(|path| path.extension().is_some_and(|ext| ext == "pcm"));
    files.sort();
    let mut pcm = Vec::new();
    for file in files {
        pcm.extend(fs::read(file)?);
    }
    let duration = expected.len() as u64 / 2 * 1000 / 16_000;
    report["stages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"stage":stage,
        "mode":config["hotword_update_mode"],"run":snapshot.meeting.run,
        "segments":snapshot.segments.len(),"duration_ms":duration,
        "pcm_sha256":format!("{:x}",Sha256::digest(&pcm))}));
    ensure!(pcm == expected, "{stage}: archived PCM changed");
    ensure!(
        config["hotword_update_mode"] == mode && config["threads"] == 1,
        "{stage}: actual worker mode/thread count differs"
    );
    ensure!(
        config["hotword_count"].as_u64().is_some_and(|n| n > 0)
            && config["decoder"] == "modified_beam_search"
            && config["fallback"].is_null(),
        "{stage}: expected usable note hotwords"
    );
    ensure!(
        snapshot.meeting.duration_ms == duration && snapshot.runtime.processed_ms == duration,
        "{stage}: incomplete tail/duration"
    );
    ensure!(
        !snapshot.segments.is_empty(),
        "{stage}: no public speech recognized"
    );
    let mut end = 0;
    for (index, segment) in snapshot.segments.iter().enumerate() {
        ensure!(
            segment.seq as usize == index + 1
                && segment.run == snapshot.meeting.run
                && segment.start_ms >= end
                && segment.end_ms >= segment.start_ms
                && segment.end_ms <= duration,
            "{stage}: invalid sequence/timeline"
        );
        end = segment.end_ms;
    }
    ensure!(
        !snapshot.meeting.auto_notes
            && snapshot.translation_mode == "off"
            && snapshot.suggestions.is_empty()
            && snapshot.translations.is_empty()
            && engine.inference_worker_status().starts == 0,
        "Unexpected notes/translation inference"
    );
    Ok(snapshot)
}
fn probe(root: &Path, wav: &Path, mode: &str, deadline: Instant, report: &mut Value) -> Result<()> {
    let reader = hound::WavReader::open(wav)?;
    let spec = reader.spec();
    ensure!(
        spec.channels == 1
            && spec.sample_rate == 16_000
            && spec.bits_per_sample == 16
            && spec.sample_format == hound::SampleFormat::Int
            && (1..=240_000).contains(&reader.duration()),
        "Use a nonempty public mono 16 kHz s16 WAV of at most 15 seconds"
    );
    let mut pcm = Vec::new();
    // Native import deliberately filters even 16 kHz WAVs. Compare its canonical
    // output, not container sample bytes; this starts no ASR/inference process.
    let mut reference = omascribe_core::audio::spawn_capture(
        "auto",
        Some(wav),
        None,
        &root.join("reference-decode.log"),
    )?;
    reference
        .stdout
        .take()
        .context("Missing reference decoder")?
        .read_to_end(&mut pcm)?;
    ensure!(
        reference.wait()? && !pcm.is_empty(),
        "Canonical WAV decoding failed"
    );
    report["input_sha256"] = json!(format!("{:x}", Sha256::digest(fs::read(wav)?)));
    let engine = Engine::open(root.to_path_buf())?;
    let mut shutdown = Shutdown(Some(engine.clone()));
    let result = (|| -> Result<()> {
        {
            let mut prefs = engine.preferences.lock().unwrap();
            prefs.auto_notes = false;
            prefs.translation_mode = "off".into();
            prefs.diarization = false;
            prefs.threads = 1;
            prefs.asr_model = "zipformer-small".into();
        }
        let meeting = engine.create_meeting("Public ASR context probe")?;
        report["meeting_id"] = json!(meeting.id);
        let note = engine.save_note(
            &meeting.id,
            None,
            "**OmaScribe** **Project Alpha**",
            "document",
            0,
        )?;
        remaining(deadline)?;
        engine.start_recording(&meeting.id, "auto", Some(wav.to_path_buf()))?;
        wait(&engine, &meeting.id, deadline)?;
        let imported = inspect(&engine, &meeting.id, "import", &pcm, mode, report)?;
        let first = &imported.segments[0];
        engine.edit_segment(&first.id, "Human correction: OMA SCRIBE.", first.revision)?;
        let protected = engine.snapshot(&meeting.id)?.segments[0].clone();
        write(&root.join("protected-segment.json"), &protected)?;
        engine.save_note(
            &meeting.id,
            Some(note.id),
            "**OmaScribe** **Project Beta**",
            "document",
            0,
        )?;
        let words = fs::read_to_string(engine.audio_dir(&meeting.id)?.join("hotwords.txt"))?;
        ensure!(
            words.contains("PROJECT BETA") && !words.contains("PROJECT ALPHA"),
            "Same-Engine note edit did not replace the published hotwords"
        );
        remaining(deadline)?;
        engine.start_recording(&meeting.id, "auto", Some(wav.to_path_buf()))?;
        wait(&engine, &meeting.id, deadline)?;
        let doubled = [pcm.as_slice(), pcm.as_slice()].concat();
        let appended = inspect(&engine, &meeting.id, "append", &doubled, mode, report)?;
        ensure!(
            appended.meeting.run == imported.meeting.run
                && appended.segments.len() > imported.segments.len(),
            "Append did not retain its run/add speech"
        );
        ensure!(
            serde_json::to_value(&appended.segments[0])? == serde_json::to_value(&protected)?,
            "Note update/append changed a human-protected segment"
        );
        remaining(deadline)?;
        engine.retranscribe(&meeting.id)?;
        wait(&engine, &meeting.id, deadline)?;
        let repeated = inspect(&engine, &meeting.id, "retranscribe", &doubled, mode, report)?;
        ensure!(
            repeated.meeting.run == appended.meeting.run + 1,
            "Retranscribe did not create a new run"
        );
        let historical = engine.store.lock().unwrap().segments(&appended.meeting)?;
        ensure!(
            serde_json::to_value(historical.first().context("Previous run disappeared")?)?
                == serde_json::to_value(&protected)?,
            "Retranscribe overwrote the previous run's human correction"
        );
        remaining(deadline)
    })();
    let drained = shutdown.close();
    report["shutdown_task_drain"] = json!(drained);
    report["native_process_exit"] = json!("not_verified; outer process-tree observer required");
    result?;
    ensure!(drained == Some(true), "Shutdown task drain is unknown");
    Ok(())
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    ensure!(
        args.len() == 4,
        "Usage: asr_context_probe NEW_ROOT PUBLIC_WAV stream|legacy"
    );
    let root = Path::new(&args[1]);
    let wav = Path::new(&args[2]).canonicalize()?;
    let (mode, expected_override) = match args[3].to_str() {
        Some("stream") => ("stream_experiment", None),
        Some("legacy") => ("recognizer", Some(std::ffi::OsString::from("0"))),
        _ => anyhow::bail!("Expected stream or legacy"),
    };
    ensure!(
        std::env::var_os("OMASCRIBE_ASR_STREAM_HOTWORDS") == expected_override,
        "Stream must test unset environment; legacy must test explicit 0"
    );
    ensure!(
        root.is_absolute() && root.parent().is_some_and(Path::is_dir),
        "Use a fresh absolute root under an existing parent"
    );
    for key in ["OMASCRIBE_MODELS_DIR", "OMASCRIBE_RUNTIME_DIR"] {
        ensure!(
            std::env::var_os(key).is_some_and(|p| Path::new(&p).is_dir()),
            "Provide task-owned {key}"
        );
    }
    fs::create_dir(root)
        .context("Root must not exist; never use the installed/user data directory")?;
    write(
        &root.join("probe-owner.json"),
        &json!({"owner":uuid::Uuid::new_v4().to_string(),"kind":"asr-context-probe"}),
    )?;
    let started = Instant::now();
    let mut report = json!({"schema":1,"passed":false,"stages":[],"build":omascribe_core::build_identity(),
        "expected_mode":mode,"input":wav,"root":root,"budget_seconds":40,
        "cleanup":"Evidence/root retained; outer owner must confirm process exit before removal",
        "limits":["No live microphone/pause-resume claim", "Does not prove recognition quality or release admission"]});
    let result = probe(
        root,
        &wav,
        mode,
        started + Duration::from_secs(40),
        &mut report,
    );
    report["wall_seconds"] = json!(started.elapsed().as_secs_f64());
    report["budget_met"] = json!(started.elapsed() <= Duration::from_secs(40));
    report["error"] = json!(result.as_ref().err().map(|error| format!("{error:#}")));
    report["passed"] = json!(result.is_ok() && report["budget_met"] == true);
    write(&root.join("report.json"), &report)?;
    println!("{}", serde_json::to_string(&report)?);
    ensure!(
        report["passed"] == true,
        "Production ASR context probe failed; see report.json"
    );
    Ok(())
}
