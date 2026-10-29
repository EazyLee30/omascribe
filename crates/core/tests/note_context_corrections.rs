//! Integration coverage for source-backed note spellings and revisioned evidence.
//!
//! These tests seed synthetic ASR text and a synthetic minutes section. They do
//! not run speech recognition or a language model and cannot establish that an
//! arbitrary phonetic error is corrected automatically.

use anyhow::Result;
use omascribe_core::{
    library::Entry, notes, Engine, Segment, Snapshot, Source, Suggestion, UserNote,
};

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
        assert!(notes::needs_update(&after), "{guard}");
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
        assert!(notes::needs_update(&after), "{origin}");
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
    assert!(notes::needs_update(&after));
    Ok(())
}

const CEDAR_DOCUMENT_BEFORE: &str = "CEDARLEDGER\n\nlocal test\n\nbudget";
const CEDAR_DOCUMENT_AFTER: &str = "CedarLedger\n\nlocal test\n\nbudget";
const CEDAR_SOURCE: &str =
    "CEDARLEDGER has not completed the local test. The budget has not been approved.";
const CEDAR_PROSE: &str =
    "CEDARLEDGER has not completed the local test, and the budget has not been approved.";

/// Establish an actually synchronized document before editing its existing ID.
/// Creating a replacement document changes active context and is a different case.
fn clean_cedar_document(engine: &Engine, title: &str) -> Result<(UserNote, Segment, Snapshot)> {
    let meeting = engine.create_meeting(title)?;
    engine.update_meeting(&meeting.id, &meeting.title, true)?;
    let note = engine.save_note(&meeting.id, None, CEDAR_DOCUMENT_BEFORE, "document", 0)?;
    let raw = segment(&meeting.id, 1, CEDAR_SOURCE);
    engine.store.lock().unwrap().save_segment(&raw)?;
    let before = engine.snapshot(&meeting.id)?;
    engine.store.lock().unwrap().commit_document(
        &before,
        &before.segments,
        1,
        vec![section(&meeting.id, &raw, CEDAR_PROSE)],
        false,
    )?;
    let clean = engine.snapshot(&meeting.id)?;
    assert!(!notes::needs_update(&clean));
    assert!(!clean.meeting.notes_fingerprint.is_empty());
    assert_eq!(
        clean.meeting.notes_fingerprint,
        notes::fingerprint(&clean.notes, &clean.segments, 1)
    );
    Ok((note, raw, clean))
}

fn inference_jobs(engine: &Engine) -> Result<i64> {
    Ok(engine.store.lock().unwrap().conn.query_row(
        "SELECT COUNT(*) FROM inference_jobs",
        [],
        |row| row.get(0),
    )?)
}

#[test]
fn same_document_spelling_only_edit_keeps_complete_minutes_synchronized() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = Engine::open(root.path().into())?;
    let (note, raw, before) = clean_cedar_document(&engine, "Same document spelling")?;
    assert_eq!(inference_jobs(&engine)?, 0);

    let saved = engine.save_note(
        &note.meeting_id,
        Some(note.id.clone()),
        CEDAR_DOCUMENT_AFTER,
        "document",
        0,
    )?;
    let after = engine.snapshot(&note.meeting_id)?;
    assert_eq!(saved.id, note.id);
    assert_eq!(after.notes.len(), 1);
    assert_eq!(after.notes[0].content, CEDAR_DOCUMENT_AFTER);
    assert_eq!(after.meeting.notes_cursor, 1);
    assert_eq!(after.segments[0].id, raw.id);
    assert_eq!(after.segments[0].revision, 2);
    assert_eq!(
        after.segments[0].text,
        CEDAR_SOURCE.replace("CEDARLEDGER", "CedarLedger")
    );
    let article = &after.suggestions[0];
    assert_eq!(article.id, before.suggestions[0].id);
    assert_eq!(
        article.text,
        CEDAR_PROSE.replace("CEDARLEDGER", "CedarLedger")
    );
    assert_eq!(article.status, "pending");
    assert!(!article.stale);
    assert_eq!(article.sources.len(), 1);
    assert_eq!(article.sources[0].segment_id, raw.id);
    assert_eq!(article.sources[0].revision, 2);
    assert_eq!(article.sources[0].quote, after.segments[0].text);
    assert_eq!(article.sources[0].start_ms, raw.start_ms);
    assert_eq!(article.sources[0].end_ms, raw.end_ms);
    assert_eq!(article.sources[0].speaker_name, None);
    assert!(!notes::needs_update(&after));
    assert!(!after.meeting.notes_fingerprint.is_empty());
    assert_eq!(
        after.meeting.notes_fingerprint,
        notes::fingerprint(&after.notes, &after.segments, after.meeting.notes_cursor)
    );
    assert_ne!(
        after.meeting.notes_fingerprint,
        before.meeting.notes_fingerprint
    );
    assert_eq!(inference_jobs(&engine)?, 0);

    let db = engine.store.lock().unwrap();
    let original: String = db.conn.query_row(
        "SELECT json FROM segment_history WHERE id=?1 AND revision=1",
        [&raw.id],
        |row| row.get(0),
    )?;
    assert_eq!(
        serde_json::from_str::<Segment>(&original)?.text,
        CEDAR_SOURCE
    );
    let archived: String = db.conn.query_row(
        "SELECT json FROM document_history WHERE meeting_id=?1 ORDER BY id DESC LIMIT 1",
        [&note.meeting_id],
        |row| row.get(0),
    )?;
    let archived: Vec<Suggestion> = serde_json::from_str(&archived)?;
    assert_eq!(archived[0].id, before.suggestions[0].id);
    assert_eq!(archived[0].text, CEDAR_PROSE);
    assert_eq!(archived[0].sources[0].quote, CEDAR_SOURCE);
    assert_eq!(archived[0].sources[0].revision, 1);
    Ok(())
}

#[test]
fn successive_note_spellings_replay_from_the_current_source_revision() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = Engine::open(root.path().into())?;
    let meeting = engine.create_meeting("Successive source-backed spellings")?;
    engine.update_meeting(&meeting.id, &meeting.title, false)?;
    let original_note = CEDAR_DOCUMENT_BEFORE.replace("CEDARLEDGER", "CEDAR LEDGER");
    let note = engine.save_note(&meeting.id, None, &original_note, "document", 0)?;
    let raw = segment(
        &meeting.id,
        1,
        &CEDAR_SOURCE.replace("CEDARLEDGER", "CEDAR LEDGER"),
    );
    let raw_prose = CEDAR_PROSE.replace("CEDARLEDGER", "CEDAR LEDGER");
    engine.store.lock().unwrap().save_segment(&raw)?;
    let before = engine.snapshot(&meeting.id)?;
    engine.store.lock().unwrap().commit_document(
        &before,
        &before.segments,
        1,
        vec![section(&meeting.id, &raw, &raw_prose)],
        false,
    )?;
    let article_id = engine.snapshot(&meeting.id)?.suggestions[0].id.clone();

    for (index, content) in [CEDAR_DOCUMENT_BEFORE, CEDAR_DOCUMENT_AFTER]
        .into_iter()
        .enumerate()
    {
        engine.save_note(&meeting.id, Some(note.id.clone()), content, "document", 0)?;
        let after = engine.snapshot(&meeting.id)?;
        let expected_name = content.lines().next().unwrap();
        let expected_source = CEDAR_SOURCE.replace("CEDARLEDGER", expected_name);
        let expected_prose = CEDAR_PROSE.replace("CEDARLEDGER", expected_name);
        let revision = index as u32 + 2;
        assert_eq!(after.segments[0].id, raw.id);
        assert_eq!(after.segments[0].revision, revision);
        assert_eq!(after.segments[0].text, expected_source);
        assert_eq!(after.suggestions[0].id, article_id);
        assert_eq!(after.suggestions[0].text, expected_prose);
        assert_eq!(after.suggestions[0].status, "pending");
        assert!(!after.suggestions[0].stale);
        assert_eq!(after.suggestions[0].sources[0].segment_id, raw.id);
        assert_eq!(after.suggestions[0].sources[0].revision, revision);
        assert_eq!(after.suggestions[0].sources[0].quote, expected_source);
        assert_eq!(after.suggestions[0].sources[0].start_ms, raw.start_ms);
        assert_eq!(after.suggestions[0].sources[0].end_ms, raw.end_ms);
        assert_eq!(after.meeting.notes_cursor, 1);
        assert!(!notes::needs_update(&after));
        assert_eq!(inference_jobs(&engine)?, 0);
    }
    let db = engine.store.lock().unwrap();
    let mut query = db
        .conn
        .prepare("SELECT json FROM segment_history WHERE id=?1 ORDER BY revision")?;
    let archived = query
        .query_map([&raw.id], |row| row.get::<_, String>(0))?
        .map(|row| Ok(serde_json::from_str::<Segment>(&row?)?))
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(archived.len(), 2);
    assert_eq!(archived[0].text, raw.text);
    assert_eq!(archived[0].revision, 1);
    assert_eq!(archived[1].text, CEDAR_SOURCE);
    assert_eq!(archived[1].revision, 2);
    let mut query = db
        .conn
        .prepare("SELECT json FROM document_history WHERE meeting_id=?1 ORDER BY id")?;
    let documents = query
        .query_map([&meeting.id], |row| row.get::<_, String>(0))?
        .map(|row| Ok(serde_json::from_str::<Vec<Suggestion>>(&row?)?))
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(documents.len(), 3);
    assert_eq!(documents[1][0].text, raw_prose);
    assert_eq!(documents[1][0].sources[0].quote, raw.text);
    assert_eq!(documents[1][0].sources[0].revision, 1);
    assert_eq!(documents[2][0].text, CEDAR_PROSE);
    assert_eq!(documents[2][0].sources[0].quote, CEDAR_SOURCE);
    assert_eq!(documents[2][0].sources[0].revision, 2);
    Ok(())
}

fn clean_previously_normalized_document(
    engine: &Engine,
    title: &str,
) -> Result<(UserNote, Snapshot)> {
    let meeting = engine.create_meeting(title)?;
    engine.update_meeting(&meeting.id, &meeting.title, false)?;
    let note = engine.save_note(
        &meeting.id,
        None,
        &CEDAR_DOCUMENT_BEFORE.replace("CEDARLEDGER", "CEDAR LEDGER"),
        "document",
        0,
    )?;
    let raw = segment(
        &meeting.id,
        1,
        &CEDAR_SOURCE.replace("CEDARLEDGER", "CEDAR LEDGER"),
    );
    engine.store.lock().unwrap().save_segment(&raw)?;
    let before = engine.snapshot(&meeting.id)?;
    engine.store.lock().unwrap().commit_document(
        &before,
        &before.segments,
        1,
        vec![section(
            &meeting.id,
            &raw,
            &CEDAR_PROSE.replace("CEDARLEDGER", "CEDAR LEDGER"),
        )],
        false,
    )?;
    engine.save_note(
        &meeting.id,
        Some(note.id.clone()),
        CEDAR_DOCUMENT_BEFORE,
        "document",
        0,
    )?;
    let clean = engine.snapshot(&meeting.id)?;
    assert_eq!(clean.segments[0].revision, 2);
    assert_eq!(clean.segments[0].text, CEDAR_SOURCE);
    assert_eq!(clean.suggestions[0].text, CEDAR_PROSE);
    assert!(!notes::needs_update(&clean));
    Ok((note, clean))
}

#[test]
fn successive_spellings_keep_human_accepted_and_mismatched_evidence_protected() -> Result<()> {
    for guard in [
        "human_source",
        "accepted",
        "human_article",
        "wrong_quote",
        "stale",
    ] {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let (note, clean) = clean_previously_normalized_document(&engine, guard)?;
        let mut article = clean.suggestions[0].clone();
        match guard {
            "human_source" => engine.edit_segment(
                &clean.segments[0].id,
                "Human review keeps CEDARLEDGER as a counterexample.",
                2,
            )?,
            "human_article" => engine.store.lock().unwrap().update_suggestion(
                &note.meeting_id,
                &article.id,
                "pending",
                Some("Human review keeps CEDARLEDGER as a counterexample.".into()),
            )?,
            "accepted" => {
                article.status = "accepted".into();
                engine.store.lock().unwrap().save_suggestion(&article)?;
            }
            "wrong_quote" => {
                article.sources[0].quote = "An unrelated source.".into();
                engine.store.lock().unwrap().save_suggestion(&article)?;
            }
            _ => {
                article.stale = true;
                engine.store.lock().unwrap().save_suggestion(&article)?;
            }
        }
        let protected = engine.snapshot(&note.meeting_id)?;
        engine.save_note(
            &note.meeting_id,
            Some(note.id),
            CEDAR_DOCUMENT_AFTER,
            "document",
            0,
        )?;
        let after = engine.snapshot(&note.meeting_id)?;
        assert_eq!(
            after.suggestions[0].text, protected.suggestions[0].text,
            "{guard}"
        );
        assert_eq!(
            after.suggestions[0].status, protected.suggestions[0].status,
            "{guard}"
        );
        assert_eq!(
            serde_json::to_value(&after.suggestions[0].sources)?,
            serde_json::to_value(&protected.suggestions[0].sources)?,
            "{guard}"
        );
        assert!(notes::needs_update(&after), "{guard}");
        if guard == "human_source" {
            assert_eq!(
                serde_json::to_value(&after.segments)?,
                serde_json::to_value(&protected.segments)?
            );
        } else {
            assert_eq!(
                after.segments[0].text,
                CEDAR_SOURCE.replace("CEDARLEDGER", "CedarLedger")
            );
            assert_eq!(after.segments[0].revision, 3);
        }
        assert_eq!(inference_jobs(&engine)?, 0);
    }
    Ok(())
}

#[test]
fn successive_spelling_never_uses_current_text_as_the_authoritative_target() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = Engine::open(root.path().into())?;
    let (note, before) = clean_previously_normalized_document(&engine, "Withdrawn spelling")?;
    engine.save_note(
        &note.meeting_id,
        Some(note.id),
        "local test\n\nbudget",
        "document",
        0,
    )?;
    let after = engine.snapshot(&note.meeting_id)?;
    // Removing the term restores archived ASR. The current text cannot reach
    // that target, so its old replacement proof must not authorize a patch.
    assert_eq!(
        after.segments[0].text,
        CEDAR_SOURCE.replace("CEDARLEDGER", "CEDAR LEDGER")
    );
    assert_eq!(after.segments[0].revision, 3);
    assert_eq!(after.suggestions[0].text, before.suggestions[0].text);
    assert_eq!(
        serde_json::to_value(&after.suggestions[0].sources)?,
        serde_json::to_value(&before.suggestions[0].sources)?
    );
    assert!(after.suggestions[0].stale);
    assert!(notes::needs_update(&after));
    assert_eq!(inference_jobs(&engine)?, 0);
    Ok(())
}

#[test]
fn successive_spelling_does_not_invent_new_note_facts_or_acknowledge_them() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = Engine::open(root.path().into())?;
    let (note, _) = clean_previously_normalized_document(&engine, "New fact is not a spelling")?;
    engine.save_note(
        &note.meeting_id,
        Some(note.id),
        &format!("{CEDAR_DOCUMENT_AFTER}\n\nBudget 9000 is approved."),
        "document",
        0,
    )?;
    let after = engine.snapshot(&note.meeting_id)?;
    assert_eq!(after.segments[0].revision, 3);
    assert_eq!(
        after.segments[0].text,
        CEDAR_SOURCE.replace("CEDARLEDGER", "CedarLedger")
    );
    assert_eq!(
        after.suggestions[0].text,
        CEDAR_PROSE.replace("CEDARLEDGER", "CedarLedger")
    );
    assert!(!after.suggestions[0].text.contains("9000"));
    assert!(after.suggestions[0].text.contains("not been approved"));
    assert!(notes::needs_update(&after));
    assert!(after.meeting.notes_fingerprint.is_empty());
    assert_eq!(inference_jobs(&engine)?, 0);
    Ok(())
}

#[test]
fn successive_spelling_cannot_borrow_authority_for_another_sources_shared_prefix() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = Engine::open(root.path().into())?;
    let (note, clean) = clean_previously_normalized_document(&engine, "Successive shared prefix")?;
    engine.save_library_entry(explicit_term("CEDARLEDGER Beta", ""))?;
    let other = segment(&note.meeting_id, 2, "CEDARLEDGER Beta still needs review.");
    engine.store.lock().unwrap().save_segment(&other)?;
    let before = engine.snapshot(&note.meeting_id)?;
    let body = format!("{} {}", CEDAR_PROSE, other.text);
    let mut combined = section(&note.meeting_id, &before.segments[0], &body);
    combined.sources.push(source(&other));
    engine.store.lock().unwrap().commit_document(
        &before,
        &before.segments,
        2,
        vec![combined],
        false,
    )?;
    engine.save_note(
        &note.meeting_id,
        Some(note.id),
        CEDAR_DOCUMENT_AFTER,
        "document",
        0,
    )?;
    let after = engine.snapshot(&note.meeting_id)?;
    assert_eq!(after.segments[0].revision, clean.segments[0].revision + 1);
    assert_eq!(
        after.segments[0].text,
        CEDAR_SOURCE.replace("CEDARLEDGER", "CedarLedger")
    );
    assert_eq!(
        serde_json::to_value(&after.segments[1])?,
        serde_json::to_value(&other)?
    );
    assert_eq!(after.suggestions[0].text, body);
    assert!(after.suggestions[0].stale);
    assert!(notes::needs_update(&after));
    assert_eq!(inference_jobs(&engine)?, 0);
    Ok(())
}

#[test]
fn changed_note_content_or_replacement_document_still_needs_generation() -> Result<()> {
    for change in ["new_budget", "new_topic", "replacement_document"] {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let (note, _, _) = clean_cedar_document(&engine, change)?;
        let content = match change {
            "new_budget" => format!("{CEDAR_DOCUMENT_AFTER}\n\nBudget 3000?"),
            "new_topic" => format!("{CEDAR_DOCUMENT_AFTER}\n\nlegal review"),
            _ => "CedarLedger".into(),
        };
        let note_id = (change != "replacement_document").then(|| note.id.clone());
        let saved = engine.save_note(&note.meeting_id, note_id, &content, "document", 0)?;
        let after = engine.snapshot(&note.meeting_id)?;
        assert_eq!(after.segments[0].revision, 2, "{change}");
        assert!(notes::needs_update(&after), "{change}");
        assert_ne!(
            after.meeting.notes_fingerprint,
            notes::fingerprint(&after.notes, &after.segments, after.meeting.notes_cursor),
            "{change} must not be sealed as spelling-only"
        );
        assert_eq!(inference_jobs(&engine)?, 0);
        if change == "replacement_document" {
            assert_ne!(saved.id, note.id);
            assert_eq!(notes::document_notes(&after.notes)[0].id, saved.id);
        } else {
            assert_eq!(saved.id, note.id);
        }
    }
    Ok(())
}

#[test]
fn spelling_only_edit_preserves_an_unaffected_source_and_pending_section() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = Engine::open(root.path().into())?;
    let (note, raw, _) = clean_cedar_document(&engine, "Two complete sections")?;
    let unchanged_source = segment(
        &note.meeting_id,
        2,
        "The microphone trial is still pending.",
    );
    engine
        .store
        .lock()
        .unwrap()
        .save_segment(&unchanged_source)?;
    let before = engine.snapshot(&note.meeting_id)?;
    engine.store.lock().unwrap().commit_document(
        &before,
        &before.segments,
        2,
        vec![
            before.suggestions[0].clone(),
            section(
                &note.meeting_id,
                &unchanged_source,
                "Microphone trial: still pending.",
            ),
        ],
        false,
    )?;
    let clean = engine.snapshot(&note.meeting_id)?;
    assert!(!notes::needs_update(&clean));
    assert_eq!(clean.meeting.notes_cursor, 2);
    let old_affected = clean
        .suggestions
        .iter()
        .find(|article| article.sources[0].segment_id == raw.id)
        .unwrap();
    let old_unaffected = clean
        .suggestions
        .iter()
        .find(|article| article.sources[0].segment_id == unchanged_source.id)
        .unwrap();

    engine.save_note(
        &note.meeting_id,
        Some(note.id),
        CEDAR_DOCUMENT_AFTER,
        "document",
        0,
    )?;
    let after = engine.snapshot(&note.meeting_id)?;
    assert_eq!(after.meeting.notes_cursor, 2);
    assert_eq!(after.segments[0].revision, 2);
    assert_eq!(
        after.segments[0].text,
        CEDAR_SOURCE.replace("CEDARLEDGER", "CedarLedger")
    );
    assert_eq!(
        serde_json::to_value(&after.segments[1])?,
        serde_json::to_value(&unchanged_source)?
    );
    let affected = after
        .suggestions
        .iter()
        .find(|article| article.id == old_affected.id)
        .unwrap();
    assert_eq!(
        affected.text,
        CEDAR_PROSE.replace("CEDARLEDGER", "CedarLedger")
    );
    assert_eq!(affected.sources[0].revision, 2);
    assert_eq!(affected.sources[0].quote, after.segments[0].text);
    let unaffected = after
        .suggestions
        .iter()
        .find(|article| article.id == old_unaffected.id)
        .unwrap();
    // Source-wide dirty marking must not strand an otherwise untouched section,
    // and restoring freshness must preserve every other field byte-for-byte.
    assert_eq!(
        serde_json::to_value(unaffected)?,
        serde_json::to_value(old_unaffected)?
    );
    assert!(after
        .suggestions
        .iter()
        .all(|s| s.status == "pending" && !s.stale));
    assert!(!notes::needs_update(&after));
    assert_eq!(
        after.meeting.notes_fingerprint,
        notes::fingerprint(&after.notes, &after.segments, 2)
    );
    assert_eq!(inference_jobs(&engine)?, 0);
    let untouched_history: i64 = engine.store.lock().unwrap().conn.query_row(
        "SELECT COUNT(*) FROM segment_history WHERE id=?1",
        [&unchanged_source.id],
        |row| row.get(0),
    )?;
    assert_eq!(untouched_history, 0);
    Ok(())
}

#[test]
fn spelling_only_edit_cannot_seal_incomplete_dirty_or_protected_minutes() -> Result<()> {
    for guard in [
        "uncovered_source",
        "already_dirty",
        "already_stale",
        "human_article",
        "human_source",
        "wrong_quote",
        "missing_source",
    ] {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let (note, raw, clean) = clean_cedar_document(&engine, guard)?;
        let mut article = clean.suggestions[0].clone();
        match guard {
            "uncovered_source" => {
                let later = segment(&note.meeting_id, 2, "The battery test is still pending.");
                engine.store.lock().unwrap().save_segment(&later)?;
            }
            "already_dirty" => {
                let mut meeting = clean.meeting.clone();
                meeting.notes_fingerprint.clear();
                engine.store.lock().unwrap().save_meeting(&meeting)?;
            }
            "already_stale" => {
                article.stale = true;
                engine.store.lock().unwrap().save_suggestion(&article)?;
            }
            "human_article" => {
                engine.store.lock().unwrap().update_suggestion(
                    &note.meeting_id,
                    &article.id,
                    "pending",
                    Some("Human review: CEDARLEDGER is only a counterexample.".into()),
                )?;
            }
            "human_source" => engine.edit_segment(
                &raw.id,
                "Human review: CEDARLEDGER is only a label; the test is still pending.",
                1,
            )?,
            "wrong_quote" => {
                article.sources[0].quote = "Different source evidence.".into();
                engine.store.lock().unwrap().save_suggestion(&article)?;
            }
            "missing_source" => {
                let mut absent = article.sources[0].clone();
                absent.segment_id = "absent-source".into();
                article.sources.push(absent);
                engine.store.lock().unwrap().save_suggestion(&article)?;
            }
            _ => unreachable!(),
        }
        let before = engine.snapshot(&note.meeting_id)?;
        engine.save_note(
            &note.meeting_id,
            Some(note.id),
            CEDAR_DOCUMENT_AFTER,
            "document",
            0,
        )?;
        let after = engine.snapshot(&note.meeting_id)?;
        assert!(notes::needs_update(&after), "{guard}");
        assert_ne!(
            after.meeting.notes_fingerprint,
            notes::fingerprint(&after.notes, &after.segments, after.meeting.notes_cursor),
            "{guard} must not be marked synchronized"
        );
        assert_eq!(after.meeting.notes_cursor, before.meeting.notes_cursor);
        assert_eq!(inference_jobs(&engine)?, 0);
        if guard == "human_source" {
            assert_eq!(after.segments[0].text, before.segments[0].text);
            assert_eq!(after.segments[0].revision, before.segments[0].revision);
        } else {
            assert_eq!(after.segments[0].revision, 2, "{guard}");
        }
        if !matches!(guard, "uncovered_source" | "already_dirty") {
            assert_eq!(
                after.suggestions[0].text, before.suggestions[0].text,
                "{guard}"
            );
            assert_eq!(
                after.suggestions[0].status, before.suggestions[0].status,
                "{guard}"
            );
            assert_eq!(
                serde_json::to_value(&after.suggestions[0].sources)?,
                serde_json::to_value(&before.suggestions[0].sources)?,
                "{guard}"
            );
        }
        if guard == "human_article" {
            assert_eq!(after.suggestions[0].status, "accepted");
        }
    }
    Ok(())
}
