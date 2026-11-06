use anyhow::Result;
use omascribe_core::{models, Engine, Segment, Suggestion};
use std::{fs, sync::Arc};

fn isolated_engine(root: &std::path::Path) -> Result<Arc<Engine>> {
    let mut engine = Engine::open(root.into())?;
    let unique = Arc::get_mut(&mut engine).expect("new engine has no other owners");
    unique.models = root.join("no-bundled-models");
    unique.runtime_dir = root.join("no-native-runtime");
    Ok(engine)
}

fn corrupt_model(engine: &Engine, id: &str) -> Result<()> {
    let spec = models::model(id)?;
    let directory = engine.root.join("model-store").join(id);
    fs::create_dir_all(&directory)?;
    for asset in &spec.files {
        // Sparse files satisfy installation metadata without real model assets.
        fs::File::create(directory.join(&asset.name))?.set_len(asset.bytes)?;
    }
    assert_eq!(engine.model_directory(id)?, directory);
    Ok(())
}

#[test]
fn corrupted_asr_refuses_capture_and_retranscription_before_meeting_changes() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = isolated_engine(root.path())?;
    corrupt_model(&engine, "zipformer-small")?;
    let mut meeting = engine.create_meeting("Integrity admission")?;
    meeting.status = "ready".into();
    meeting.notes_cursor = 7;
    meeting.diarization_cursor = 7;
    meeting
        .speaker_names
        .insert("named".into(), "Speaker".into());
    engine.store.lock().unwrap().save_meeting(&meeting)?;
    let suggestion = Suggestion {
        focus: String::new(),
        id: "previous-suggestion".into(),
        meeting_id: meeting.id.clone(),
        kind: "fact".into(),
        text: "Retain this suggestion".into(),
        sources: vec![],
        status: "pending".into(),
        stale: false,
    };
    engine.store.lock().unwrap().save_suggestion(&suggestion)?;
    let audio = engine.audio_dir(&meeting.id)?;
    fs::write(audio.join("000000.pcm"), [0_u8; 32])?;
    for marker in ["capture.done", "capture.paused", "capture.error"] {
        fs::write(audio.join(marker), marker)?;
    }
    let before = serde_json::to_value(engine.snapshot(&meeting.id)?)?;
    let capture_error = engine
        .start_recording(&meeting.id, "auto", None)
        .unwrap_err();
    assert!(capture_error.to_string().starts_with("model:integrity:"));
    let retranscribe_error = engine.retranscribe(&meeting.id).unwrap_err();
    assert!(retranscribe_error
        .to_string()
        .starts_with("model:integrity:"));
    assert_eq!(serde_json::to_value(engine.snapshot(&meeting.id)?)?, before);
    assert!(engine.active.lock().unwrap().is_none());
    assert!(engine.store.lock().unwrap().takes(&meeting.id)?.is_empty());
    for marker in ["capture.done", "capture.paused", "capture.error"] {
        assert_eq!(fs::read(audio.join(marker))?, marker.as_bytes());
    }
    assert_eq!(fs::read(audio.join("000000.pcm"))?, [0_u8; 32]);
    assert!(!audio.join("capture.log").exists());
    assert!(!audio.join("hotwords.txt").exists());
    Ok(())
}

#[test]
fn corrupted_speaker_model_preserves_transcript_and_diarization_state() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = isolated_engine(root.path())?;
    corrupt_model(&engine, "speaker-diarization")?;
    let meeting = engine.create_meeting("Speaker integrity admission")?;
    engine.store.lock().unwrap().save_segment(&Segment {
        id: "previous-segment".into(),
        meeting_id: meeting.id.clone(),
        run: meeting.run,
        seq: 1,
        start_ms: 0,
        end_ms: 1000,
        text: "Retain this transcript".into(),
        revision: 1,
        speaker: Some("previous-speaker".into()),
        speaker_manual: true,
    })?;
    let before = serde_json::to_value(engine.snapshot(&meeting.id)?)?;
    let error = engine.diarize_with_count(&meeting.id, Some(2)).unwrap_err();
    assert!(error.to_string().starts_with("model:integrity:"));
    assert_eq!(serde_json::to_value(engine.snapshot(&meeting.id)?)?, before);
    assert_eq!(
        engine
            .diarization_pid
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert!(!root.path().join("diarization.log").exists());
    Ok(())
}
