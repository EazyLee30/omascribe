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

#[test]
fn late_note_updates_existing_ai_spelling_with_automatic_minutes_disabled() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = Engine::open(root.path().into())?;
    let meeting = engine.create_meeting("Late source-backed spelling")?;
    engine.update_meeting(&meeting.id, &meeting.title, false)?;
    let raw = segment(&meeting.id, 1, "CEDARLEDGER测试尚未完成，预算仍未确定。");
    engine.store.lock().unwrap().save_segment(&raw)?;
    let before = engine.snapshot(&meeting.id)?;
    engine.store.lock().unwrap().commit_document(
        &before,
        &before.segments,
        1,
        vec![section(&meeting.id, &raw, &raw.text)],
        false,
    )?;
    let note = engine.save_note(&meeting.id, None, "CedarLedger", "document", 0)?;
    let after = engine.snapshot(&meeting.id)?;
    assert!(!after.meeting.auto_notes);
    assert_eq!(after.meeting.notes_cursor, 1);
    assert_eq!(
        after.segments[0].text,
        "CedarLedger测试尚未完成，预算仍未确定。"
    );
    assert_eq!(after.segments[0].revision, 2);
    assert_eq!(after.suggestions[0].text, after.segments[0].text);
    assert_eq!(
        after.suggestions[0].sources[0].quote,
        after.segments[0].text
    );
    assert_eq!(after.suggestions[0].sources[0].revision, 2);
    assert!(!after.suggestions[0].stale);
    let original: String = engine.store.lock().unwrap().conn.query_row(
        "SELECT json FROM segment_history WHERE id=?1 AND revision=1",
        [&raw.id],
        |row| row.get(0),
    )?;
    assert_eq!(serde_json::from_str::<Segment>(&original)?.text, raw.text);
    let history: String = engine.store.lock().unwrap().conn.query_row(
        "SELECT json FROM document_history WHERE meeting_id=?1 ORDER BY id DESC LIMIT 1",
        [&meeting.id],
        |row| row.get(0),
    )?;
    assert_eq!(
        serde_json::from_str::<Vec<Suggestion>>(&history)?[0].text,
        raw.text
    );
    let counts = || -> Result<(i64, i64)> {
        let db = engine.store.lock().unwrap();
        Ok((
            db.conn
                .query_row("SELECT COUNT(*) FROM document_history", [], |r| r.get(0))?,
            db.conn
                .query_row("SELECT COUNT(*) FROM inference_jobs", [], |r| r.get(0))?,
        ))
    };
    let once = counts()?;
    assert_eq!(once.1, 0);
    engine.save_note(&meeting.id, Some(note.id), "CedarLedger", "document", 0)?;
    assert_eq!(counts()?, once);
    Ok(())
}

#[test]
fn late_explicit_alias_replays_only_observed_spelling_in_prose() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = Engine::open(root.path().into())?;
    let meeting = engine.create_meeting("Explicit bilingual spelling")?;
    engine.update_meeting(&meeting.id, &meeting.title, false)?;
    let raw = segment(&meeting.id, 1, "雪松账本周四测试，预算3000元仍未批准。");
    engine.store.lock().unwrap().save_segment(&raw)?;
    let before = engine.snapshot(&meeting.id)?;
    engine.store.lock().unwrap().commit_document(
        &before,
        &before.segments,
        1,
        vec![section(&meeting.id, &raw, &raw.text)],
        false,
    )?;
    engine.save_library_entry(explicit_term("CedarLedger", "雪松账本"))?;
    let after = engine.snapshot(&meeting.id)?;
    assert_eq!(
        after.suggestions[0].text,
        "CedarLedger周四测试，预算3000元仍未批准。"
    );
    assert_eq!(
        after.suggestions[0].sources[0].quote,
        after.segments[0].text
    );
    assert_eq!(after.suggestions[0].sources[0].revision, 2);
    assert!(!after.suggestions[0].stale);
    Ok(())
}

#[test]
fn late_names_do_not_overwrite_protected_or_unverifiable_articles() -> Result<()> {
    for guard in [
        "accepted",
        "dismissed",
        "stale",
        "old_revision",
        "wrong_quote",
        "missing_source",
        "human_pending",
    ] {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let meeting = engine.create_meeting(guard)?;
        engine.update_meeting(&meeting.id, &meeting.title, false)?;
        let raw = segment(&meeting.id, 1, "CEDARLEDGER尚未完成，预算未定。");
        engine.store.lock().unwrap().save_segment(&raw)?;
        let before = engine.snapshot(&meeting.id)?;
        engine.store.lock().unwrap().commit_document(
            &before,
            &before.segments,
            1,
            vec![section(&meeting.id, &raw, &raw.text)],
            false,
        )?;
        let mut protected = engine.snapshot(&meeting.id)?.suggestions.remove(0);
        match guard {
            "accepted" | "dismissed" => protected.status = guard.into(),
            "stale" => protected.stale = true,
            "old_revision" => protected.sources[0].revision = 0,
            "wrong_quote" => protected.sources[0].quote = "另一个对象".into(),
            "missing_source" => {
                let mut missing = protected.sources[0].clone();
                missing.segment_id = "missing".into();
                protected.sources.push(missing);
            }
            _ => {}
        }
        engine.store.lock().unwrap().save_suggestion(&protected)?;
        if guard == "human_pending" {
            protected.text = "人工保留：CEDARLEDGER是反例，预算未定。".into();
            engine.store.lock().unwrap().update_suggestion(
                &meeting.id,
                &protected.id,
                "pending",
                Some(protected.text.clone()),
            )?;
            protected.status = "accepted".into();
        }
        engine.save_note(&meeting.id, None, "CedarLedger", "document", 0)?;
        let after = engine.snapshot(&meeting.id)?;
        assert_eq!(after.suggestions[0].text, protected.text, "{guard}");
        assert_eq!(after.suggestions[0].status, protected.status, "{guard}");
        assert!(after.suggestions[0].stale, "{guard}");
        assert_eq!(
            after.suggestions[0].sources[0].quote, protected.sources[0].quote,
            "{guard}"
        );
    }
    Ok(())
}

#[test]
fn a_document_write_failure_rolls_back_body_sources_and_history_together() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = Engine::open(root.path().into())?;
    let meeting = engine.create_meeting("Atomic spelling revision")?;
    let raw = segment(&meeting.id, 1, "CEDARLEDGER仍未完成。");
    engine.store.lock().unwrap().save_segment(&raw)?;
    let before = engine.snapshot(&meeting.id)?;
    engine.store.lock().unwrap().commit_document(
        &before,
        &before.segments,
        1,
        vec![section(&meeting.id, &raw, &raw.text)],
        false,
    )?;
    engine.store.lock().unwrap().conn.execute_batch(
        "CREATE TRIGGER reject_spelling_document BEFORE UPDATE ON suggestions
         WHEN json_extract(NEW.json,'$.stale')=0
         BEGIN SELECT RAISE(ABORT,'fixture document write failed'); END;",
    )?;
    assert!(engine
        .save_note(&meeting.id, None, "CedarLedger", "document", 0)
        .is_err());
    let after = engine.snapshot(&meeting.id)?;
    assert_eq!(after.segments[0].text, "CedarLedger仍未完成。");
    assert_eq!(after.segments[0].revision, 2);
    assert_eq!(after.suggestions[0].text, raw.text);
    assert_eq!(after.suggestions[0].sources[0].quote, raw.text);
    assert_eq!(after.suggestions[0].sources[0].revision, 1);
    assert!(after.suggestions[0].stale);
    let history: i64 = engine.store.lock().unwrap().conn.query_row(
        "SELECT COUNT(*) FROM document_history",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(history, 1);
    Ok(())
}

#[test]
fn another_cited_human_or_legacy_use_cannot_inherit_a_name_replacement() -> Result<()> {
    for origin in ["human", "legacy"] {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let meeting = engine.create_meeting(origin)?;
        let raw = segment(&meeting.id, 1, "CEDARLEDGER尚未完成。");
        let mut other = segment(&meeting.id, 2, "另一个原始对象。");
        engine.store.lock().unwrap().save_segment(&raw)?;
        engine.store.lock().unwrap().save_segment(&other)?;
        let retained = "CEDARLEDGER在此仅为错误示例。";
        if origin == "human" {
            engine.edit_segment(&other.id, retained, 1)?;
        } else {
            // Imported legacy edits have source history but no origin record.
            let db = engine.store.lock().unwrap();
            db.conn.execute(
                "INSERT INTO segment_history VALUES(?1,1,?2)",
                rusqlite::params![other.id, serde_json::to_string(&other)?],
            )?;
            other.text = retained.into();
            other.revision = 2;
            db.save_segment(&other)?;
        }
        let before = engine.snapshot(&meeting.id)?;
        let body = "CEDARLEDGER尚未完成；CEDARLEDGER在另一段中仅为错误示例。";
        let mut article = section(&meeting.id, &raw, body);
        article.sources.push(source(&before.segments[1]));
        engine.store.lock().unwrap().commit_document(
            &before,
            &before.segments,
            2,
            vec![article],
            false,
        )?;
        engine.save_note(&meeting.id, None, "CedarLedger", "document", 0)?;
        let after = engine.snapshot(&meeting.id)?;
        assert_eq!(after.segments[0].text, "CedarLedger尚未完成。");
        assert_eq!(after.segments[1].text, retained, "{origin}");
        assert_eq!(after.suggestions[0].text, body, "{origin}");
        assert!(after.suggestions[0].stale, "{origin}");
    }
    Ok(())
}

#[test]
fn another_changed_source_cannot_authorize_a_shared_name_prefix() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = Engine::open(root.path().into())?;
    let meeting = engine.create_meeting("Changed-source name collision")?;
    engine.save_library_entry(explicit_term("ALPHA Beta", ""))?;
    engine.save_library_entry(explicit_term("Omega", ""))?;
    let first = segment(&meeting.id, 1, "ALPHA尚未完成。");
    let second = segment(&meeting.id, 2, "ALPHA Beta仍未完成，OMEGA待测。");
    engine.store.lock().unwrap().save_segment(&first)?;
    engine.store.lock().unwrap().save_segment(&second)?;
    let before = engine.snapshot(&meeting.id)?;
    let body = format!("{}{}", first.text, second.text);
    let mut article = section(&meeting.id, &first, &body);
    article.sources.push(source(&second));
    engine.store.lock().unwrap().commit_document(
        &before,
        &before.segments,
        2,
        vec![article],
        false,
    )?;
    engine.save_library_entry(explicit_term("AlphaProduct", "ALPHA"))?;
    let after = engine.snapshot(&meeting.id)?;
    assert_eq!(after.segments[0].text, "AlphaProduct尚未完成。");
    assert_eq!(after.segments[1].text, "ALPHA Beta仍未完成，Omega待测。");
    assert_eq!(after.suggestions[0].text, body);
    assert!(after.suggestions[0].stale);
    Ok(())
}
