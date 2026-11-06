//! Saving one library row must preserve the full view's prior-context decisions.
//! Legacy rows are inserted through Store to exercise imported SQL/JSON identities
//! and dates. No scheduler, speech recognizer, or language model is started.

use anyhow::{Context, Result};
use omascribe_core::{library::Entry, Engine, Meeting};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;

fn insert_json(engine: &Engine, sql_id: &str, value: &Value, sql_date: &str) -> Result<()> {
    engine.store.lock().unwrap().conn.execute(
        "INSERT INTO library_entries VALUES(?1,?2,?3)",
        rusqlite::params![sql_id, value.to_string(), sql_date],
    )?;
    Ok(())
}

fn prior(engine: &Engine, id: &str) -> Result<Option<Entry>> {
    Ok(engine.library_entries()?.into_iter().find(|e| e.id == id))
}

fn marked_meeting(engine: &Engine) -> Result<Meeting> {
    let mut meeting = engine.create_meeting("Public context compatibility fixture")?;
    meeting.notes_fingerprint = "existing-minutes-fingerprint".into();
    engine.store.lock().unwrap().save_meeting(&meeting)?;
    Ok(meeting)
}

fn assert_invalidation(
    engine: &Engine,
    meeting: &Meeting,
    before: u64,
    expected: bool,
) -> Result<()> {
    assert_eq!(
        engine.context_revision.load(Ordering::SeqCst),
        before + u64::from(expected)
    );
    let after = engine.store.lock().unwrap().meeting(&meeting.id)?;
    assert_eq!(
        after.notes_fingerprint,
        if expected {
            ""
        } else {
            meeting.notes_fingerprint.as_str()
        }
    );
    Ok(())
}

#[test]
fn imported_prior_uses_logical_id_then_stable_json_and_sql_dates() -> Result<()> {
    // Each pair has the same logical ID and distinct SQL IDs. The full view
    // first orders SQL dates, then stably orders JSON dates before find().
    for (label, dates, knowledge, winner) in [
        (
            "JSON date wins over SQL date",
            [("2099", "2000"), ("1900", "2100")],
            [false, true],
            1,
        ),
        (
            "equal JSON dates retain SQL date order",
            [("1900", "2100"), ("2099", "2100")],
            [false, true],
            1,
        ),
        (
            "equal dates retain original row order",
            [("2099", "2100"), ("2099", "2100")],
            [false, true],
            0,
        ),
    ] {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        engine.preferences.lock().unwrap().use_knowledge = true;
        let meeting = marked_meeting(&engine)?;
        let id = uuid::Uuid::new_v4().to_string();
        for index in 0..2 {
            let entry = Entry {
                id: id.clone(),
                title: format!("Imported row {index}"),
                knowledge: knowledge[index],
                updated_at: dates[index].1.into(),
                ..Default::default()
            };
            insert_json(
                &engine,
                &format!("legacy-sql-row-{index}"),
                &serde_json::to_value(entry)?,
                dates[index].0,
            )?;
        }
        let selected = prior(&engine, &id)?.context("Missing imported prior")?;
        assert_eq!(selected.title, format!("Imported row {winner}"), "{label}");
        assert_eq!(selected.knowledge, knowledge[winner], "{label}");
        let before = engine.context_revision.load(Ordering::SeqCst);
        engine.save_library_entry(Entry {
            id,
            title: "Now an ordinary personal note".into(),
            ..Default::default()
        })?;
        assert_invalidation(&engine, &meeting, before, knowledge[winner])?;
    }
    Ok(())
}

#[test]
fn removing_linked_note_knowledge_keeps_prior_overlay_eligibility() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = Engine::open(root.path().into())?;
    engine.preferences.lock().unwrap().use_knowledge = true;
    let mut meeting = marked_meeting(&engine)?;
    let note = engine.save_note(&meeting.id, None, "Current linked body", "note", 0)?;
    let old = Entry {
        id: note.id.clone(),
        content: "Imported stale body".into(),
        knowledge: true,
        meeting_id: Some(meeting.id.clone()),
        ..Default::default()
    };
    insert_json(&engine, &old.id, &serde_json::to_value(&old)?, "2026")?;
    let selected = prior(&engine, &old.id)?.context("Missing linked prior")?;
    assert_eq!(selected.content, note.content);
    assert!(selected.knowledge);
    // Save-note terminology maintenance is allowed during setup; this sentinel
    // checks the actual knowledge removal's background invalidation.
    meeting.notes_fingerprint = "existing-minutes-fingerprint".into();
    engine.store.lock().unwrap().save_meeting(&meeting)?;
    let before = engine.context_revision.load(Ordering::SeqCst);
    engine.save_library_entry(Entry {
        knowledge: false,
        content: note.content,
        ..old
    })?;
    assert_invalidation(&engine, &meeting, before, true)
}

#[test]
fn disabled_term_to_enabled_memory_still_rechecks_terminology() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = Engine::open(root.path().into())?;
    engine.preferences.lock().unwrap().use_memory = true;
    let meeting = marked_meeting(&engine)?;
    let id = uuid::Uuid::new_v4().to_string();
    let old = Entry {
        id: id.clone(),
        kind: "term".into(),
        title: "CedarLedger".into(),
        enabled: false,
        ..Default::default()
    };
    insert_json(&engine, &id, &serde_json::to_value(old)?, "2026")?;
    let selected = prior(&engine, &id)?.context("Missing disabled prior")?;
    assert_eq!(selected.kind, "term");
    assert!(!selected.enabled);
    engine.store.lock().unwrap().conn.execute(
        "INSERT INTO cache_dependencies VALUES(?1,'terms','not-yet-rechecked')",
        [&meeting.id],
    )?;
    let before = engine.context_revision.load(Ordering::SeqCst);
    engine.save_library_entry(Entry {
        id,
        kind: "memory".into(),
        content: "Prefer short paragraphs.".into(),
        ..Default::default()
    })?;
    assert_invalidation(&engine, &meeting, before, true)?;
    let expected = engine.terminology(&meeting.id)?.fingerprint();
    let actual: String = engine.store.lock().unwrap().conn.query_row(
        "SELECT value FROM cache_dependencies WHERE meeting_id=?1 AND kind='terms'",
        [&meeting.id],
        |row| row.get(0),
    )?;
    assert_ne!(actual, "not-yet-rechecked");
    assert_eq!(actual, expected);
    Ok(())
}

#[test]
fn matching_invalid_row_is_rejected_before_deleted_meeting_filter() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = Engine::open(root.path().into())?;
    let meeting = marked_meeting(&engine)?;
    engine
        .store
        .lock()
        .unwrap()
        .conn
        .execute("INSERT INTO deleted_meetings VALUES(?1)", [&meeting.id])?;
    let id = uuid::Uuid::new_v4().to_string();
    let invalid = json!({
        "id": id, "meeting_id": meeting.id,
        "kind": "note", "enabled": "not-a-bool"
    });
    insert_json(&engine, &id, &invalid, "2026")?;
    assert!(prior(&engine, &id).is_err());
    let before = engine.context_revision.load(Ordering::SeqCst);
    assert!(engine
        .save_library_entry(Entry {
            id: id.clone(),
            ..Default::default()
        })
        .is_err());
    assert_invalidation(&engine, &meeting, before, false)?;
    let unchanged: String = engine.store.lock().unwrap().conn.query_row(
        "SELECT json FROM library_entries WHERE id=?1",
        [&id],
        |row| row.get(0),
    )?;
    assert_eq!(serde_json::from_str::<Value>(&unchanged)?, invalid);
    Ok(())
}

#[test]
fn missing_logical_id_defaults_before_uuid_creation_but_null_is_rejected() -> Result<()> {
    for explicit_null in [false, true] {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        engine.preferences.lock().unwrap().use_knowledge = true;
        let meeting = marked_meeting(&engine)?;
        let mut old = json!({"kind": "note", "knowledge": true, "updated_at": "2026"});
        if explicit_null {
            old["id"] = Value::Null;
        }
        insert_json(&engine, "legacy-default-id", &old, "2026")?;
        let before = engine.context_revision.load(Ordering::SeqCst);
        if explicit_null {
            assert!(prior(&engine, "").is_err());
            assert!(engine.save_library_entry(Entry::default()).is_err());
            assert_invalidation(&engine, &meeting, before, false)?;
            let rows: i64 = engine.store.lock().unwrap().conn.query_row(
                "SELECT COUNT(*) FROM library_entries",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(rows, 1);
        } else {
            let selected = prior(&engine, "")?.context("Missing default-ID prior")?;
            assert!(selected.id.is_empty() && selected.knowledge);
            let saved = engine.save_library_entry(Entry::default())?;
            uuid::Uuid::parse_str(&saved.id)?;
            assert_invalidation(&engine, &meeting, before, true)?;
        }
    }
    Ok(())
}

#[test]
fn prior_linked_to_deleted_meeting_does_not_invalidate_current_context() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = Engine::open(root.path().into())?;
    engine.preferences.lock().unwrap().use_knowledge = true;
    let current = marked_meeting(&engine)?;
    let removed = engine.create_meeting("Removed public fixture")?;
    engine
        .store
        .lock()
        .unwrap()
        .conn
        .execute("INSERT INTO deleted_meetings VALUES(?1)", [&removed.id])?;
    let id = uuid::Uuid::new_v4().to_string();
    insert_json(
        &engine,
        &id,
        &serde_json::to_value(Entry {
            id: id.clone(),
            knowledge: true,
            meeting_id: Some(removed.id),
            ..Default::default()
        })?,
        "2026",
    )?;
    assert!(prior(&engine, &id)?.is_none());
    let before = engine.context_revision.load(Ordering::SeqCst);
    engine.save_library_entry(Entry {
        id,
        title: "New ordinary unlinked note".into(),
        ..Default::default()
    })?;
    assert_invalidation(&engine, &current, before, false)
}
