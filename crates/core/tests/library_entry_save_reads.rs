//! A single ordinary note save must not deserialize unrelated library history.
//! Synthetic, valid JSON with an invalid body type makes broad reads observable;
//! these tests do not run a scheduler, speech recognition, or a language model.

use anyhow::Result;
use omascribe_core::{library::Entry, Engine};
use std::sync::atomic::Ordering;

enum UnrelatedBody {
    HistoricalNote,
    PersistedEntry,
}

enum Change {
    Edit,
    Disable,
    Delete,
}

type Rows = Vec<(String, String, String, String)>;

fn unrelated_rows(engine: &Engine, target: &str) -> Result<Rows> {
    let db = engine.store.lock().unwrap();
    let mut query = db.conn.prepare(
        "SELECT 'meeting', id, json, '' FROM meetings
         UNION ALL SELECT 'note', id, json, '' FROM notes
         UNION ALL SELECT 'entry', id, json, updated_at FROM library_entries WHERE id != ?1
         ORDER BY 1, 2",
    )?;
    let rows = query
        .query_map([target], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

fn save_ordinary_note(body: UnrelatedBody, change: Change) -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = Engine::open(root.path().into())?;
    let mut target = engine.save_library_entry(Entry {
        title: "Public synthetic personal note".into(),
        content: "Only this unlinked note is being changed.".into(),
        ..Default::default()
    })?;
    assert_eq!(target.kind, "note");
    assert!(target.meeting_id.is_none() && !target.knowledge);
    let mut meeting = engine.create_meeting("Unrelated public history")?;
    meeting.notes_fingerprint = "unrelated-minutes-must-stay-current".into();
    {
        let db = engine.store.lock().unwrap();
        db.save_meeting(&meeting)?;
        let unrelated_id = "4d2c3938-9b26-40ae-9b51-f7eb222a9981";
        match body {
            UnrelatedBody::HistoricalNote => {
                let json = serde_json::json!({
                    "id": unrelated_id, "meeting_id": meeting.id,
                    "kind": "document", "anchor_ms": 0,
                    "content": {"synthetic_invalid_body_type": true},
                    "updated_at": "2026-11-07T00:00:00Z"
                });
                db.conn.execute(
                    "INSERT INTO notes VALUES(?1,?2,?3)",
                    rusqlite::params![unrelated_id, meeting.id, json.to_string()],
                )?;
            }
            UnrelatedBody::PersistedEntry => {
                let mut json = serde_json::to_value(Entry {
                    id: unrelated_id.into(),
                    title: "Unrelated persisted note".into(),
                    ..Default::default()
                })?;
                json["content"] = serde_json::json!({"synthetic_invalid_body_type": true});
                db.conn.execute(
                    "INSERT INTO library_entries VALUES(?1,?2,?3)",
                    rusqlite::params![unrelated_id, json.to_string(), "2026-11-07"],
                )?;
            }
        }
    }
    // Negative control: a full-body materialization really encounters the poison.
    assert!(engine.library_entries().is_err());
    let before = unrelated_rows(&engine, &target.id)?;
    let revision = engine.context_revision.load(Ordering::SeqCst);
    match change {
        Change::Edit => {
            target.title = "Edited personal note".into();
            target.content = "A new private reminder, still outside writer context.".into();
        }
        Change::Disable => target.enabled = false,
        Change::Delete => target.deleted = true,
    }
    let saved = engine.save_library_entry(target.clone())?;
    assert_eq!(saved.id, target.id);
    assert_eq!(saved.title, target.title);
    assert_eq!(saved.content, target.content);
    assert_eq!(saved.enabled, target.enabled);
    assert_eq!(saved.deleted, target.deleted);
    assert!(saved.meeting_id.is_none() && !saved.knowledge);
    let persisted: Entry = {
        let db = engine.store.lock().unwrap();
        let json: String = db.conn.query_row(
            "SELECT json FROM library_entries WHERE id=?1",
            [&target.id],
            |row| row.get(0),
        )?;
        serde_json::from_str(&json)?
    };
    assert_eq!(
        serde_json::to_value(persisted)?,
        serde_json::to_value(saved)?
    );
    assert_eq!(engine.context_revision.load(Ordering::SeqCst), revision);
    assert_eq!(unrelated_rows(&engine, &target.id)?, before);
    Ok(())
}

#[test]
fn edit_unlinked_note_does_not_read_historical_note_bodies() -> Result<()> {
    save_ordinary_note(UnrelatedBody::HistoricalNote, Change::Edit)
}

#[test]
fn disable_unlinked_note_does_not_read_historical_note_bodies() -> Result<()> {
    save_ordinary_note(UnrelatedBody::HistoricalNote, Change::Disable)
}

#[test]
fn delete_unlinked_note_does_not_read_historical_note_bodies() -> Result<()> {
    save_ordinary_note(UnrelatedBody::HistoricalNote, Change::Delete)
}

#[test]
fn edit_unlinked_note_does_not_read_unrelated_persisted_entry_bodies() -> Result<()> {
    save_ordinary_note(UnrelatedBody::PersistedEntry, Change::Edit)
}

#[test]
fn disable_unlinked_note_does_not_read_unrelated_persisted_entry_bodies() -> Result<()> {
    save_ordinary_note(UnrelatedBody::PersistedEntry, Change::Disable)
}

#[test]
fn delete_unlinked_note_does_not_read_unrelated_persisted_entry_bodies() -> Result<()> {
    save_ordinary_note(UnrelatedBody::PersistedEntry, Change::Delete)
}
