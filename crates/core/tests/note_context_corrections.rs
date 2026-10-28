//! Integration coverage for source-backed note spellings and revisioned evidence.
//!
//! These tests seed synthetic ASR text and a synthetic minutes section. They do
//! not run speech recognition or a language model and cannot establish that an
//! arbitrary phonetic error is corrected automatically.

use anyhow::Result;
use omascribe_core::{library::Entry, Engine, Segment, Source, Suggestion};

fn segment(meeting: &str, seq: u32, text: &str) -> Segment {
    Segment {
        id: format!("{meeting}:1:{seq}"),
        meeting_id: meeting.into(),
        run: 1,
        seq,
        start_ms: u64::from(seq - 1) * 3000,
        end_ms: u64::from(seq) * 3000,
        text: text.into(),
        revision: 1,
        speaker: None,
        speaker_manual: false,
    }
}

fn source(segment: &Segment) -> Source {
    Source {
        segment_id: segment.id.clone(),
        revision: segment.revision,
        start_ms: segment.start_ms,
        end_ms: segment.end_ms,
        quote: segment.text.clone(),
        speaker_name: None,
    }
}

fn section(meeting: &str, evidence: &Segment, text: &str) -> Suggestion {
    Suggestion {
        focus: String::new(),
        id: String::new(),
        meeting_id: meeting.into(),
        kind: "summary".into(),
        text: text.into(),
        sources: vec![source(evidence)],
        status: "pending".into(),
        stale: false,
    }
}

fn explicit_term(title: &str, aliases: &str) -> Entry {
    Entry {
        kind: "term".into(),
        title: title.into(),
        aliases: aliases.into(),
        ..Default::default()
    }
}

#[test]
fn note_and_confirmed_aliases_update_source_backed_minutes_evidence() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = Engine::open(root.path().into())?;
    let meeting = engine.create_meeting("Synthetic note context")?;
    let raw = segment(
        &meeting.id,
        1,
        "OMA SCRIBE 尚未完成。艾莉丝检查 NVD GTX 一六五零。",
    );
    engine.store.lock().unwrap().save_segment(&raw)?;

    // Existing synthetic minutes must not silently keep a changed source quote.
    let old = engine.snapshot(&meeting.id)?;
    let old_section = section(&meeting.id, &raw, "旧的合成纪要");
    engine.store.lock().unwrap().commit_document(
        &old,
        &old.segments,
        1,
        vec![old_section],
        false,
    )?;

    engine.save_note(
        &meeting.id,
        None,
        "OmaScribe\nAlice\nNVIDIA\nGTX 1650",
        "document",
        0,
    )?;
    let with_note = engine.snapshot(&meeting.id)?;
    assert_eq!(
        with_note.segments[0].text,
        "OmaScribe 尚未完成。艾莉丝检查 NVD GTX 1650。"
    );
    assert!(with_note.suggestions[0].stale);
    assert_eq!(with_note.suggestions[0].sources[0].quote, raw.text);
    assert_eq!(with_note.suggestions[0].sources[0].revision, 1);
    assert!(engine
        .store
        .lock()
        .unwrap()
        .commit_document(&old, &old.segments, 1, vec![], false)
        .is_err());

    // These cross-language/abbreviated spellings are explicit user-supplied
    // aliases: the note alone does not prove either mapping.
    engine.save_library_entry(explicit_term("Alice", "艾莉丝"))?;
    engine.save_library_entry(explicit_term("NVIDIA", "NVD"))?;
    let corrected = engine.snapshot(&meeting.id)?;
    let current = &corrected.segments[0];
    assert_eq!(
        current.text,
        "OmaScribe 尚未完成。Alice检查 NVIDIA GTX 1650。"
    );
    assert!(current.revision > 1);
    let history: String = engine.store.lock().unwrap().conn.query_row(
        "SELECT json FROM segment_history WHERE id=?1 AND revision=1",
        [&raw.id],
        |r| r.get(0),
    )?;
    assert_eq!(serde_json::from_str::<Segment>(&history)?.text, raw.text);

    let terms = engine.terminology(&meeting.id)?;
    let body = "OmaScribe 尚未完成。Alice检查 NVIDIA GTX 1650。";
    terms.validate_writer_names(&current.text, body)?;
    assert!(terms
        .validate_writer_names(&current.text, "OmaScribe 尚未完成。Alice检查 NVIDIA 显卡。")
        .is_err());
    let context = engine.writer_context(&corrected.notes, &current.text)?;
    let spellings = context["spellings"].as_array().unwrap();
    for expected in ["OmaScribe", "Alice", "NVIDIA", "GTX 1650"] {
        assert!(
            spellings.iter().any(|v| v == expected),
            "missing {expected}"
        );
    }

    // This manually supplied section verifies commit/revision wiring only; it
    // is not evidence of a generated AI minutes result.
    engine.store.lock().unwrap().commit_document(
        &corrected,
        &corrected.segments,
        1,
        vec![section(&meeting.id, current, body)],
        false,
    )?;
    let after = engine.snapshot(&meeting.id)?;
    let committed = &after.suggestions[0];
    assert!(!committed.stale);
    assert_eq!(committed.sources[0].segment_id, current.id);
    assert_eq!(committed.sources[0].revision, current.revision);
    assert_eq!(committed.sources[0].quote, current.text);

    engine.edit_segment(
        &current.id,
        "人工确认：这个段落仅讨论 GTX 1660。",
        current.revision,
    )?;
    engine.correct_terminology(&meeting.id)?;
    let human = engine.snapshot(&meeting.id)?;
    assert_eq!(
        human.segments[0].text,
        "人工确认：这个段落仅讨论 GTX 1660。"
    );
    assert!(human.suggestions[0].stale);
    Ok(())
}

#[test]
fn note_spelling_does_not_create_unspoken_terms_or_merge_different_models() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = Engine::open(root.path().into())?;
    let meeting = engine.create_meeting("Synthetic negatives")?;
    let raw = segment(&meeting.id, 1, "预算尚未确定。GTX 一六六零仍待测试。");
    engine.store.lock().unwrap().save_segment(&raw)?;
    engine.save_note(
        &meeting.id,
        None,
        "OmaScribe\nAlice\nNVIDIA\nGTX 1650",
        "document",
        0,
    )?;
    let snapshot = engine.snapshot(&meeting.id)?;
    assert_eq!(snapshot.segments[0].text, raw.text);
    assert_eq!(snapshot.segments[0].revision, 1);
    let context = engine.writer_context(&snapshot.notes, &snapshot.segments[0].text)?;
    let spellings = context.get("spellings").and_then(|v| v.as_array());
    for absent in ["OmaScribe", "Alice", "NVIDIA", "GTX 1650"] {
        assert!(
            !spellings.is_some_and(|names| names.iter().any(|v| v == absent)),
            "invented spelling hint {absent}"
        );
    }
    let terms = engine.terminology(&meeting.id)?;
    for text in ["GTX 1660", "GTX 16500", "RTX 1650", ""] {
        assert_eq!(terms.normalize(text), text);
    }
    Ok(())
}
