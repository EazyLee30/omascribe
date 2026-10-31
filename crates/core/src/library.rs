use crate::{preferences::atomic_json, Engine, UserNote};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, path::Path};

const TERMINOLOGY_ENTRIES_SQL: &str =
    "SELECT json FROM library_entries WHERE json_extract(json,'$.kind')='term' ORDER BY updated_at DESC";
// Keep the full persisted view's ordering and ID shadowing, but transfer only
// context candidates' bodies into Rust. Ordinary historical notes have no
// knowledge flag until their explicit library row opts in.
const WRITER_ENTRIES_SQL: &str = "SELECT json_extract(json,'$.id'), CASE WHEN
    json_extract(json,'$.kind')='term'
    OR (?1 AND json_extract(json,'$.kind')='memory')
    OR (?2 AND json_type(json,'$.knowledge') IS NOT NULL AND json_type(json,'$.knowledge')!='false')
    THEN json END FROM library_entries
    WHERE NOT EXISTS(SELECT 1 FROM deleted_meetings d WHERE d.id=json_extract(library_entries.json,'$.meeting_id'))
    ORDER BY updated_at DESC";
const WRITER_OVERLAYS_SQL: &str = "SELECT n.json,
    json_extract(m.json,'$.id'), json_extract(m.json,'$.created_at'), m.rowid, n.rowid
    FROM notes n JOIN meetings m ON n.meeting_id=json_extract(m.json,'$.id')
    LEFT JOIN library_entries l ON l.id=n.id
    WHERE m.id NOT IN (SELECT id FROM deleted_meetings)
    AND COALESCE(json_extract(l.json,'$.deleted'),0)=0
    AND json_extract(n.json,'$.id') IN (SELECT value FROM json_each(?1))
    AND json_extract(n.json,'$.kind') IN ('note','document')";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub content: String,
    pub aliases: String,
    pub enabled: bool,
    pub knowledge: bool,
    pub deleted: bool,
    pub meeting_id: Option<String>,
    pub updated_at: String,
}
impl Default for Entry {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: "note".into(),
            title: String::new(),
            content: String::new(),
            aliases: String::new(),
            enabled: true,
            knowledge: false,
            deleted: false,
            meeting_id: None,
            updated_at: String::new(),
        }
    }
}
impl crate::store::Store {
    /// Read terms under the caller's existing Store guard so note context and
    /// spelling changes can share one consistent database snapshot.
    pub(crate) fn terminology_entries(&self) -> Result<Vec<Entry>> {
        let mut query = self.conn.prepare(TERMINOLOGY_ENTRIES_SQL)?;
        let mut entries: Vec<Entry> = query
            .query_map([], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect::<Result<_>>()?;
        entries.retain(|e| {
            e.meeting_id.as_ref().is_none_or(|id| {
                !self
                    .conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM deleted_meetings WHERE id=?1)",
                        [id],
                        |r| r.get::<_, bool>(0),
                    )
                    .unwrap_or(false)
            })
        });
        // JSON dates can differ from their SQL sort column in imported data.
        // A stable sort also preserves the full view's order for equal dates.
        entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(entries)
    }

    fn writer_entries(&self, use_memory: bool, use_knowledge: bool) -> Result<Vec<Entry>> {
        let mut query = self.conn.prepare(WRITER_ENTRIES_SQL)?;
        let rows = query.query_map(rusqlite::params![use_memory, use_knowledge], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })?;
        let mut entries = Vec::<Entry>::new();
        // The public full view overlays only the last persisted occurrence of
        // a JSON ID, even if another same-ID row is not a writer candidate.
        let mut owners = HashMap::<String, Option<usize>>::new();
        for row in rows {
            let (id, json) = row?;
            let owner = if let Some(json) = json {
                let entry: Entry = serde_json::from_str(&json)?;
                let index = entries.len();
                entries.push(entry);
                Some(index)
            } else {
                None
            };
            owners.insert(id.unwrap_or_default(), owner);
        }
        let ids: Vec<_> = owners
            .iter()
            .filter_map(|(id, owner)| owner.map(|_| id.as_str()))
            .collect();
        if !ids.is_empty() {
            struct Overlay {
                note: UserNote,
                meeting_id: String,
                created_at: String,
                meeting_row: i64,
                note_row: i64,
            }
            let mut query = self.conn.prepare(WRITER_OVERLAYS_SQL)?;
            let mut overlays = query
                .query_map([serde_json::to_string(&ids)?], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })?
                .map(|row| {
                    let (json, meeting_id, created_at, meeting_row, note_row) = row?;
                    Ok(Overlay {
                        note: serde_json::from_str(&json)?,
                        meeting_id,
                        created_at,
                        meeting_row,
                        note_row,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            // Reproduce meetings() then notes() ordering without constructing
            // unrelated meetings or note bodies. Row IDs preserve stable ties
            // from those original table scans; anchors remain exact u64s.
            overlays.sort_by(|a, b| {
                b.created_at
                    .cmp(&a.created_at)
                    .then(a.meeting_row.cmp(&b.meeting_row))
                    .then(a.note.anchor_ms.cmp(&b.note.anchor_ms))
                    .then(b.note.updated_at.cmp(&a.note.updated_at))
                    .then(a.note_row.cmp(&b.note_row))
            });
            for overlay in overlays {
                if let Some(Some(index)) = owners.get(&overlay.note.id) {
                    entries[*index].content = overlay.note.content;
                    entries[*index].meeting_id = Some(overlay.meeting_id);
                }
            }
        }
        // Do not LIMIT before this stable sort or before Terms/knowledge
        // ranking: JSON dates and SQL dates can differ in imported entries.
        entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(entries)
    }
}
impl Engine {
    pub fn invalidate_background(&self) -> Result<()> {
        let db = self.store.lock().unwrap();
        self.context_revision
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        for mut meeting in db.meetings()? {
            meeting.notes_fingerprint.clear();
            db.save_meeting(&meeting)?;
        }
        Ok(())
    }
    pub fn library_entries(&self) -> Result<Vec<Entry>> {
        let db = self.store.lock().unwrap();
        let mut query = db
            .conn
            .prepare("SELECT json FROM library_entries ORDER BY updated_at DESC")?;
        let mut entries: Vec<Entry> = query
            .query_map([], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect::<Result<_>>()?;
        entries.retain(|e| {
            e.meeting_id.as_ref().is_none_or(|id| {
                !db.conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM deleted_meetings WHERE id=?1)",
                        [id],
                        |r| r.get::<_, bool>(0),
                    )
                    .unwrap_or(false)
            })
        });
        let map: HashMap<String, usize> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id.clone(), i))
            .collect();
        for meeting in db.meetings()? {
            for note in db.notes(&meeting.id)? {
                if note.kind != "document" && note.kind != "note" {
                    continue;
                }
                if let Some(&i) = map.get(&note.id) {
                    entries[i].content = note.content;
                    entries[i].meeting_id = Some(meeting.id.clone());
                } else {
                    entries.push(Entry {
                        id: note.id,
                        kind: "note".into(),
                        title: meeting.title.clone(),
                        content: note.content,
                        meeting_id: Some(meeting.id.clone()),
                        updated_at: note.updated_at,
                        ..Default::default()
                    });
                }
            }
        }
        entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(entries)
    }
    /// Load terminology without materializing historical meetings and notes.
    /// Keep disabled/deleted rows: Terms owns their eligibility rules.
    pub fn terminology_entries(&self) -> Result<Vec<Entry>> {
        self.store.lock().unwrap().terminology_entries()
    }
    pub fn save_library_entry(&self, mut entry: Entry) -> Result<Entry> {
        let prefs = self.preferences.lock().unwrap().clone();
        let relevant = |e: &Entry| {
            !e.deleted
                && e.enabled
                && (e.kind == "term"
                    || e.kind == "memory" && prefs.use_memory
                    || e.kind == "note" && e.knowledge && prefs.use_knowledge)
        };
        let previous = self
            .library_entries()?
            .into_iter()
            .find(|e| e.id == entry.id);
        let was_context = previous.as_ref().is_some_and(relevant);
        let was_term = previous.as_ref().is_some_and(|e| e.kind == "term");
        if !["note", "term", "memory"].contains(&entry.kind.as_str())
            || entry.title.chars().count() > 160
            || entry.content.chars().count() > 200_000
            || entry.aliases.chars().count() > 2000
        {
            bail!("Invalid library entry");
        }
        if entry.id.is_empty() {
            entry.id = uuid::Uuid::new_v4().to_string();
        }
        uuid::Uuid::parse_str(&entry.id)?;
        if entry.kind != "note" {
            entry.knowledge = false;
            if entry.meeting_id.is_some() {
                bail!("Only notes can link to a meeting");
            }
        }
        if let Some(ref mid) = entry.meeting_id {
            let notes: Vec<UserNote> = {
                let db = self.store.lock().unwrap();
                let mut q = db
                    .conn
                    .prepare("SELECT json FROM notes WHERE meeting_id=?1")?;
                let rows = q
                    .query_map([mid], |r| r.get::<_, String>(0))?
                    .map(|r| Ok(serde_json::from_str(&r?)?))
                    .collect::<Result<Vec<_>>>()?;
                rows
            };
            let note = notes
                .iter()
                .find(|n| n.id == entry.id)
                .context("Linked note does not belong to this meeting")?;
            self.save_note(
                mid,
                Some(entry.id.clone()),
                &entry.content,
                &note.kind,
                note.anchor_ms,
            )?;
        }
        entry.updated_at = chrono::Utc::now().to_rfc3339();
        self.store.lock().unwrap().conn.execute("INSERT INTO library_entries(id,json,updated_at) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET json=excluded.json,updated_at=excluded.updated_at",rusqlite::params![entry.id,serde_json::to_string(&entry)?,entry.updated_at])?;
        // Linked note visibility changes only after the library row is persisted.
        if let Some(ref meeting) = entry.meeting_id {
            self.correct_terminology(meeting)?;
            self.refresh_live_hotwords()?;
        }
        if was_context || relevant(&entry) {
            self.invalidate_background()?;
            if entry.kind == "term" || was_term {
                for meeting in self.meetings()? {
                    self.correct_terminology(&meeting.id)?;
                }
            }
            if entry.kind == "term" || was_term {
                self.refresh_live_hotwords()?;
            }
        }
        Ok(entry)
    }
    pub fn writer_context(
        &self,
        notes: &[UserNote],
        transcript: &str,
    ) -> Result<serde_json::Value> {
        let prefs = self.preferences.lock().unwrap().clone();
        let entries = self
            .store
            .lock()
            .unwrap()
            .writer_entries(prefs.use_memory, prefs.use_knowledge)?;
        Ok(writer_context_from_entries(
            &entries,
            notes,
            transcript,
            prefs.use_memory,
            prefs.use_knowledge,
        ))
    }
}
fn writer_context_from_entries(
    entries: &[Entry],
    notes: &[UserNote],
    transcript: &str,
    use_memory: bool,
    use_knowledge: bool,
) -> serde_json::Value {
    let spellings = crate::terminology::Terms::new(notes, entries).spellings(transcript);
    let terms:Vec<_>=entries.iter().filter(|e|!e.deleted&&e.enabled&&e.kind=="term").take(40).map(|e|serde_json::json!({"term":e.title,"aliases":e.aliases,"definition":e.content.chars().take(160).collect::<String>()})).collect();
    let memory: Vec<_> = entries
        .iter()
        .filter(|e| use_memory && !e.deleted && e.enabled && e.kind == "memory")
        .take(8)
        .map(|e| e.content.chars().take(240).collect::<String>())
        .collect();
    let mut query = crate::notes::focus_terms(notes);
    query.push(transcript.to_owned());
    let candidates: Vec<_> = entries
        .iter()
        .filter(|e| use_knowledge && !e.deleted && e.enabled && e.knowledge)
        .collect();
    let texts: Vec<_> = candidates
        .iter()
        .map(|e| format!("{} {}", e.title, e.content))
        .collect();
    let refs: Vec<_> = texts.iter().map(|s| s.as_str()).collect();
    let knowledge:Vec<_>=crate::notes::ranked_text(&refs,&query).into_iter().take(3).map(|i| {
            let e=candidates[i];serde_json::json!({"title":e.title,"context":e.content.chars().take(320).collect::<String>()})
        }).collect();
    let mut context = serde_json::json!({"glossary":terms,"writing_preferences":memory,"background_notes":knowledge});
    if !spellings.is_empty() {
        context["spellings"] = serde_json::json!(spellings);
    }
    context
}
#[derive(Serialize)]
pub struct Storage {
    pub recordings: u64,
    pub database: u64,
    pub models: u64,
    pub bundled_models: u64,
    pub downloads: u64,
    pub exports: u64,
    pub free: u64,
    pub root: String,
}
fn size(path: &Path) -> u64 {
    let Ok(meta) = path.symlink_metadata() else {
        return 0;
    };
    if meta.file_type().is_symlink() {
        return 0;
    }
    if meta.is_file() {
        return meta.len();
    }
    fs::read_dir(path)
        .map(|it| it.filter_map(Result::ok).map(|e| size(&e.path())).sum())
        .unwrap_or(0)
}
pub fn storage(engine: &Engine) -> Storage {
    let managed = engine.root.join("model-store");
    let mut models = 0;
    let mut downloads = 0;
    if let Ok(entries) = fs::read_dir(managed) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with(".download-") {
                downloads += size(&e.path());
            } else {
                models += size(&e.path());
            }
        }
    }
    Storage {
        recordings: size(&engine.root.join("meetings")),
        database: size(&engine.root.join("meetings.sqlite3"))
            + size(&engine.root.join("meetings.sqlite3-wal")),
        models,
        // Count the actual bundled files as well as the user-managed model store.
        // A model directory inside the managed store is already counted above.
        bundled_models: if engine.models.canonicalize().ok().is_some_and(|p| {
            engine
                .root
                .join("model-store")
                .canonicalize()
                .ok()
                .is_some_and(|m| p.starts_with(m))
        }) {
            0
        } else {
            size(&engine.models)
        },
        downloads: downloads + size(&engine.root.join("updates")),
        exports: size(&engine.root.join("exports")),
        free: fs2::available_space(&engine.root).unwrap_or(0),
        root: engine.root.to_string_lossy().into_owned(),
    }
}
// Exported library data is explicit and separate from shareable machine settings.
pub fn export_library(engine: &Engine, path: &Path) -> Result<()> {
    atomic_json(path, &engine.library_entries()?)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminology::Terms;

    fn insert_entry(e: &Engine, entry: &Entry, sort_date: &str) {
        e.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "INSERT INTO library_entries VALUES(?1,?2,?3)",
                rusqlite::params![entry.id, serde_json::to_string(entry).unwrap(), sort_date],
            )
            .unwrap();
    }

    fn assert_same_terms(e: &Engine, notes: &[UserNote]) {
        let all = e.library_entries().unwrap();
        let only = e.terminology_entries().unwrap();
        let expected: Vec<_> = all.iter().filter(|entry| entry.kind == "term").collect();
        assert_eq!(
            serde_json::to_value(expected).unwrap(),
            serde_json::to_value(&only).unwrap()
        );
        let before = Terms::new(notes, &all);
        let after = Terms::new(notes, &only);
        assert_eq!(before.fingerprint(), after.fingerprint());
        assert_eq!(before.hotwords, after.hotwords);
        assert_eq!(before.recognition_hotwords(), after.recognition_hotwords());
        let text = "LegacyAlias SharedAlias Model 1650 mixedalias";
        assert_eq!(before.normalize(text), after.normalize(text));
        assert_eq!(before.spellings(text), after.spellings(text));
    }

    #[test]
    fn terminology_entries_preserve_visibility_order_and_alias_budgets() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        let meeting = e.create_meeting("Current").unwrap();
        e.store
            .lock()
            .unwrap()
            .conn
            .execute("INSERT INTO deleted_meetings VALUES('removed')", [])
            .unwrap();
        for i in 0..90 {
            insert_entry(
                &e,
                &Entry {
                    id: format!("term-{i}"),
                    kind: "term".into(),
                    title: format!("CanonicalTerm{i:02}"),
                    aliases: format!("Alias{i:02}|SharedAlias"),
                    // JSON dates, not the SQL sort column, determine the final
                    // order; stable ties keep the SQL ordering as before.
                    updated_at: format!("2026-10-{:02}", i % 4 + 1),
                    enabled: i != 3,
                    deleted: i == 4,
                    meeting_id: match i {
                        5 => Some("removed".into()),
                        6 => Some(meeting.id.clone()),
                        _ => None,
                    },
                    ..Default::default()
                },
                &format!("2026-09-{:03}", 90 - i),
            );
        }
        for (id, kind) in [("note", "note"), ("memory", "memory"), ("case", "Term")] {
            insert_entry(
                &e,
                &Entry {
                    id: id.into(),
                    kind: kind.into(),
                    title: "Unrelated entry".into(),
                    updated_at: "2099".into(),
                    ..Default::default()
                },
                "2099",
            );
        }
        insert_entry(
            &e,
            &Entry {
                id: "model".into(),
                kind: "term".into(),
                title: "Model1650".into(),
                aliases: "Model 1650, LegacyAlias; mixedalias".into(),
                updated_at: "2026-11-01".into(),
                ..Default::default()
            },
            "1900",
        );
        let notes = vec![UserNote {
            id: "current-note".into(),
            meeting_id: meeting.id,
            content: "**CanonicalTerm00**\n**Model1650**".into(),
            kind: "note".into(),
            anchor_ms: 0,
            updated_at: "2026-10-30".into(),
        }];
        assert_same_terms(&e, &[]);
        assert_same_terms(&e, &notes);
        let entries = e.terminology_entries().unwrap();
        assert_eq!(entries[0].id, "model");
        assert!(entries.iter().any(|entry| !entry.enabled));
        assert!(entries.iter().any(|entry| entry.deleted));
        assert!(!entries.iter().any(|entry| entry.id == "term-5"));
    }

    #[test]
    fn terminology_entries_do_not_read_historical_meetings_or_notes() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        insert_entry(
            &e,
            &Entry {
                id: "kept-term".into(),
                kind: "term".into(),
                title: "CanonicalTerm".into(),
                ..Default::default()
            },
            "2026",
        );
        // This isolated database no longer has either history table. A
        // terminology read must not need to prepare or deserialize their rows.
        e.store
            .lock()
            .unwrap()
            .conn
            .execute_batch("DROP TABLE notes; DROP TABLE meetings;")
            .unwrap();
        assert!(e.library_entries().is_err());
        let entries = e.terminology_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "kept-term");
    }

    #[test]
    fn terminology_entries_read_current_rows_and_reject_invalid_terms() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        let mut term = Entry {
            id: "current-term".into(),
            kind: "term".into(),
            title: "FirstSpelling".into(),
            aliases: "LegacyAlias".into(),
            ..Default::default()
        };
        insert_entry(&e, &term, "2026");
        let before = Terms::new(&[], &e.terminology_entries().unwrap());
        assert_eq!(before.normalize("LegacyAlias"), "FirstSpelling");
        term.title = "SecondSpelling".into();
        e.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "UPDATE library_entries SET json=?1 WHERE id=?2",
                rusqlite::params![serde_json::to_string(&term).unwrap(), term.id],
            )
            .unwrap();
        let after = Terms::new(&[], &e.terminology_entries().unwrap());
        assert_ne!(before.fingerprint(), after.fingerprint());
        assert_eq!(after.normalize("LegacyAlias"), "SecondSpelling");
        e.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "UPDATE library_entries SET json=?1 WHERE id=?2",
                rusqlite::params![r#"{"kind":"term","enabled":"invalid"}"#, term.id],
            )
            .unwrap();
        assert!(e.terminology_entries().is_err());
    }

    fn note(id: &str, meeting: &str, content: &str, kind: &str) -> UserNote {
        UserNote {
            id: id.into(),
            meeting_id: meeting.into(),
            content: content.into(),
            kind: kind.into(),
            anchor_ms: 0,
            updated_at: "2026-10-30".into(),
        }
    }

    fn assert_same_writer_context(e: &Engine, notes: &[UserNote], transcript: &str) {
        let full = e.library_entries().unwrap();
        for use_memory in [false, true] {
            for use_knowledge in [false, true] {
                {
                    let mut prefs = e.preferences.lock().unwrap();
                    prefs.use_memory = use_memory;
                    prefs.use_knowledge = use_knowledge;
                }
                assert_eq!(
                    e.writer_context(notes, transcript).unwrap(),
                    writer_context_from_entries(
                        &full,
                        notes,
                        transcript,
                        use_memory,
                        use_knowledge,
                    ),
                    "memory={use_memory}, knowledge={use_knowledge}"
                );
            }
        }
    }

    #[test]
    fn writer_context_preserves_budgets_order_defaults_and_opt_in_settings() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        for i in 0..86 {
            insert_entry(
                &e,
                &Entry {
                    id: format!("writer-term-{i}"),
                    kind: "term".into(),
                    title: format!("CanonicalTerm{i:02}"),
                    aliases: format!("LegacyAlias{i:02}"),
                    content: format!("Definition {i}: {}", "定".repeat(180)),
                    enabled: i != 0,
                    deleted: i == 1,
                    updated_at: format!("2026-10-{:02}", 1 + i % 4),
                    ..Default::default()
                },
                &format!("2026-09-{:03}", 86 - i),
            );
        }
        for i in 0..13 {
            insert_entry(
                &e,
                &Entry {
                    id: format!("writer-memory-{i}"),
                    kind: "memory".into(),
                    content: format!("Preference {i}: {}", "记".repeat(250)),
                    enabled: i != 0,
                    deleted: i == 1,
                    updated_at: format!("2026-10-{:02}", 1 + i % 3),
                    ..Default::default()
                },
                &format!("2026-08-{:03}", 13 - i),
            );
        }
        for i in 0..7 {
            insert_entry(
                &e,
                &Entry {
                    id: format!("writer-knowledge-{i}"),
                    title: format!("Project Owl {i}"),
                    content: format!("Project Owl reference {i}: {}", "知".repeat(340)),
                    knowledge: true,
                    enabled: i != 0,
                    deleted: i == 1,
                    updated_at: format!("2026-10-{:02}", 1 + i % 2),
                    ..Default::default()
                },
                &format!("2026-07-{:03}", 7 - i),
            );
        }
        // Missing fields use Entry's serde defaults, including kind=note,
        // enabled=true, deleted=false and empty JSON date.
        e.store.lock().unwrap().conn.execute(
            "INSERT INTO library_entries VALUES('default-note',?1,'2099')",
            [r#"{"id":"default-note","title":"Project Owl default","content":"Project Owl default background","knowledge":true}"#],
        ).unwrap();
        let notes = vec![note(
            "focus",
            "current",
            "**CanonicalTerm85** **Project Owl**",
            "note",
        )];
        assert_same_writer_context(&e, &notes, "LegacyAlias85 Project Owl");
        let context = e
            .writer_context(&notes, "LegacyAlias85 Project Owl")
            .unwrap();
        assert_eq!(context["glossary"].as_array().unwrap().len(), 40);
        assert_eq!(context["writing_preferences"].as_array().unwrap().len(), 8);
        assert_eq!(context["background_notes"].as_array().unwrap().len(), 3);
        assert_eq!(
            context["glossary"][0]["definition"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            160
        );
        assert_eq!(
            context["writing_preferences"][0]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            240
        );
        assert!(context["spellings"].to_string().contains("CanonicalTerm85"));
    }

    #[test]
    fn writer_context_uses_latest_linked_note_and_preserves_deleted_visibility() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        let meeting = e.create_meeting("Linked meeting").unwrap();
        let removed = e.create_meeting("Removed meeting").unwrap();
        {
            let db = e.store.lock().unwrap();
            db.save_note(&note(
                "linked",
                &meeting.id,
                "Project Owl latest body",
                "document",
            ))
            .unwrap();
            db.save_note(&note(
                "not-linked",
                &meeting.id,
                "Project Owl ordinary history",
                "note",
            ))
            .unwrap();
            db.save_note(&note(
                "removed-link",
                &removed.id,
                "Project Owl removed body",
                "note",
            ))
            .unwrap();
            db.conn
                .execute("INSERT INTO deleted_meetings VALUES(?1)", [&removed.id])
                .unwrap();
        }
        for (id, mid, deleted) in [
            ("linked", Some(meeting.id.clone()), false),
            ("removed-link", Some(removed.id), false),
            ("deleted-link", Some(meeting.id.clone()), true),
            ("missing-link", None, false),
        ] {
            insert_entry(
                &e,
                &Entry {
                    id: id.into(),
                    title: "Project Owl".into(),
                    content: format!("Project Owl persisted stale {id}"),
                    knowledge: true,
                    deleted,
                    meeting_id: mid,
                    updated_at: "2026-10-30".into(),
                    ..Default::default()
                },
                "1900",
            );
        }
        assert_same_writer_context(&e, &[], "Project Owl");
        let context = e.writer_context(&[], "Project Owl").unwrap();
        let backgrounds = context["background_notes"].to_string();
        assert!(backgrounds.contains("latest body"));
        assert!(!backgrounds.contains("stale linked"));
        assert!(!backgrounds.contains("ordinary history"));
        assert!(!backgrounds.contains("removed body"));
        e.store
            .lock()
            .unwrap()
            .save_note(&note(
                "linked",
                &meeting.id,
                "Project Owl edited again",
                "note",
            ))
            .unwrap();
        assert_same_writer_context(&e, &[], "Project Owl");
        assert!(
            e.writer_context(&[], "Project Owl").unwrap()["background_notes"]
                .to_string()
                .contains("edited again")
        );
    }

    #[test]
    fn writer_context_does_not_deserialize_unrelated_historical_meetings_and_notes() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        insert_entry(
            &e,
            &Entry {
                id: "writer-kept-term".into(),
                kind: "term".into(),
                title: "CanonicalTerm".into(),
                content: "Definition stays available".into(),
                ..Default::default()
            },
            "2026",
        );
        {
            let db = e.store.lock().unwrap();
            // Valid JSON metadata, but not deserializable Meeting/UserNote
            // payloads. The writer has no link to either historical record.
            db.conn
                .execute(
                    "INSERT INTO meetings VALUES('unrelated',?1)",
                    [r#"{"id":"unrelated","created_at":"2026"}"#],
                )
                .unwrap();
            db.conn
                .execute(
                    "INSERT INTO notes VALUES('unrelated-note','unrelated',?1)",
                    [r#"{"id":"unrelated-note","kind":"document"}"#],
                )
                .unwrap();
        }
        assert!(e.library_entries().is_err());
        let context = e.writer_context(&[], "CanonicalTerm");
        assert!(
            context.is_ok(),
            "Unrelated historical JSON was materialized: {context:?}"
        );
        assert_eq!(
            context.unwrap()["glossary"][0]["definition"],
            "Definition stays available"
        );
    }

    #[test]
    fn writer_context_preserves_imported_id_shadows_and_overlay_order() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        let mut newer = e.create_meeting("Newer").unwrap();
        let mut older = e.create_meeting("Older").unwrap();
        newer.created_at = "2026-10-30".into();
        older.created_at = "2026-10-29".into();
        for (id, title, body) in [
            ("same-json-id", "ShadowedTerm", "Persisted definition"),
            ("overlay-json-id", "OverlaidTerm", "Stale definition"),
        ] {
            insert_entry(
                &e,
                &Entry {
                    id: id.into(),
                    kind: "term".into(),
                    title: title.into(),
                    content: body.into(),
                    updated_at: "2026".into(),
                    ..Default::default()
                },
                "2099",
            );
        }
        {
            let db = e.store.lock().unwrap();
            db.save_meeting(&newer).unwrap();
            db.save_meeting(&older).unwrap();
            let shadow = Entry {
                id: "same-json-id".into(),
                content: "Unrelated shadow".into(),
                ..Default::default()
            };
            db.conn
                .execute(
                    "INSERT INTO library_entries VALUES('different-sql-id',?1,'1900')",
                    [serde_json::to_string(&shadow).unwrap()],
                )
                .unwrap();
            db.save_note(&note(
                "same-json-id",
                &newer.id,
                "Must not replace the term definition",
                "note",
            ))
            .unwrap();
            for (sql_id, meeting, anchor, body) in [
                ("newer-note", &newer.id, 0, "Newer meeting overlay"),
                (
                    "older-first",
                    &older.id,
                    u64::MAX - 1,
                    "First older overlay",
                ),
                ("older-last", &older.id, u64::MAX, "Last older overlay"),
            ] {
                let mut current = note("overlay-json-id", meeting, body, "document");
                current.anchor_ms = anchor;
                db.conn
                    .execute(
                        "INSERT INTO notes VALUES(?1,?2,?3)",
                        rusqlite::params![
                            sql_id,
                            meeting,
                            serde_json::to_string(&current).unwrap()
                        ],
                    )
                    .unwrap();
            }
        }
        assert_same_writer_context(&e, &[], "ShadowedTerm OverlaidTerm");
        let context = e.writer_context(&[], "ShadowedTerm OverlaidTerm").unwrap();
        let terms = context["glossary"].as_array().unwrap();
        assert_eq!(
            terms.iter().find(|e| e["term"] == "ShadowedTerm").unwrap()["definition"],
            "Persisted definition"
        );
        assert_eq!(
            terms.iter().find(|e| e["term"] == "OverlaidTerm").unwrap()["definition"],
            "Last older overlay"
        );
    }

    #[test]
    #[ignore = "controlled synthetic writer lookup measurement"]
    fn writer_context_large_history_measurement() {
        use std::time::Instant;

        fn retained_entry_bytes(entries: &[Entry], capacity: usize) -> usize {
            capacity * std::mem::size_of::<Entry>()
                + entries
                    .iter()
                    .map(|e| {
                        e.id.capacity()
                            + e.kind.capacity()
                            + e.title.capacity()
                            + e.content.capacity()
                            + e.aliases.capacity()
                            + e.updated_at.capacity()
                            + e.meeting_id.as_ref().map_or(0, String::capacity)
                    })
                    .sum::<usize>()
        }
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        let template = e.create_meeting("Synthetic writer lookup control").unwrap();
        {
            let db = e.store.lock().unwrap();
            let large = format!("Project Owl reference {}", "x".repeat(65_536));
            for i in 0..320 {
                let mut meeting = template.clone();
                meeting.id = format!("writer-history-{i}");
                db.save_meeting(&meeting).unwrap();
                db.save_note(&note(
                    &format!("writer-history-note-{i}"),
                    &meeting.id,
                    &large,
                    "document",
                ))
                .unwrap();
            }
        }
        for i in 0..120 {
            insert_entry(
                &e,
                &Entry {
                    id: format!("writer-unrelated-{i}"),
                    content: "y".repeat(24_576),
                    updated_at: "2026-10-29".into(),
                    ..Default::default()
                },
                "2026-10-29",
            );
        }
        for i in 0..40 {
            insert_entry(
                &e,
                &Entry {
                    id: format!("writer-term-{i}"),
                    kind: "term".into(),
                    title: format!("CanonicalTerm{i}"),
                    aliases: format!("LegacyAlias{i}"),
                    content: "Definition stays available".into(),
                    updated_at: "2026-10-30".into(),
                    ..Default::default()
                },
                "2026-10-30",
            );
        }
        for i in 0..12 {
            insert_entry(
                &e,
                &Entry {
                    id: format!("writer-preference-{i}"),
                    kind: "memory".into(),
                    content: "Concise paragraphs ".repeat(50),
                    updated_at: "2026-10-30".into(),
                    ..Default::default()
                },
                "2026-10-30",
            );
        }
        for i in 0..6 {
            insert_entry(
                &e,
                &Entry {
                    id: format!("writer-history-note-{i}"),
                    title: format!("Project Owl reference {i}"),
                    content: "Stale library body".into(),
                    knowledge: true,
                    meeting_id: Some(format!("writer-history-{i}")),
                    updated_at: "2026-10-30".into(),
                    ..Default::default()
                },
                "2026-10-30",
            );
        }
        let full = e.library_entries().unwrap();
        let selected = e.store.lock().unwrap().writer_entries(true, true).unwrap();
        let expected =
            writer_context_from_entries(&full, &[], "Project Owl LegacyAlias39", true, true);
        assert_eq!(
            expected,
            writer_context_from_entries(&selected, &[], "Project Owl LegacyAlias39", true, true)
        );
        {
            let mut prefs = e.preferences.lock().unwrap();
            prefs.use_memory = true;
            prefs.use_knowledge = true;
        }
        assert_eq!(
            e.writer_context(&[], "Project Owl LegacyAlias39").unwrap(),
            expected
        );
        assert_eq!(expected["glossary"].as_array().unwrap().len(), 40);
        assert_eq!(expected["writing_preferences"].as_array().unwrap().len(), 8);
        assert_eq!(expected["background_notes"].as_array().unwrap().len(), 3);
        let mut full_us = Vec::new();
        let mut selected_us = Vec::new();
        for turn in 0..8 {
            for selected_first in [turn % 2 == 0, turn % 2 != 0] {
                let started = Instant::now();
                let entries = if selected_first {
                    e.store.lock().unwrap().writer_entries(true, true).unwrap()
                } else {
                    e.library_entries().unwrap()
                };
                if selected_first {
                    selected_us.push(started.elapsed().as_micros());
                } else {
                    full_us.push(started.elapsed().as_micros());
                }
                std::hint::black_box(entries);
            }
        }
        println!(
            "{}",
            serde_json::json!({
                "kind": "controlled-synthetic-writer-lookup",
                "historical_meetings": 320, "historical_notes": 320,
                "unrelated_persisted_note_rows": 120,
                "linked_knowledge_rows": 6, "term_rows": 40, "memory_rows": 12,
                "full_entries": full.len(), "selected_entries": selected.len(),
                "full_serialized_bytes": serde_json::to_vec(&full).unwrap().len(),
                "selected_serialized_bytes": serde_json::to_vec(&selected).unwrap().len(),
                "full_retained_entry_capacity_bytes": retained_entry_bytes(&full, full.capacity()),
                "selected_retained_entry_capacity_bytes": retained_entry_bytes(&selected, selected.capacity()),
                "lookup_full_us": full_us, "lookup_selected_us": selected_us,
                "context_exact_equal": true, "build": crate::build_identity(),
                "limitation": "Alternating warm synthetic lookup only. Capacity is actual returned Vec/String retained capacity, excludes allocator metadata and transient SQL/serde/overlay allocations; not allocation totals, process RSS, model inference, or whole-app performance. SQL may scan JSON metadata, but unrelated history bodies are not transferred into Rust."
            })
        );
    }

    #[test]
    #[ignore = "controlled synthetic history lookup measurement"]
    fn terminology_entries_large_history_measurement() {
        use std::time::Instant;

        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        let template = e.create_meeting("Synthetic lookup control").unwrap();
        {
            let db = e.store.lock().unwrap();
            let large_content = "x".repeat(65_536);
            for i in 0..320 {
                let mut meeting = template.clone();
                meeting.id = format!("history-meeting-{i}");
                db.save_meeting(&meeting).unwrap();
                db.save_note(&UserNote {
                    id: format!("history-note-{i}"),
                    meeting_id: meeting.id,
                    content: large_content.clone(),
                    kind: "document".into(),
                    anchor_ms: 0,
                    updated_at: "2026-10-29".into(),
                })
                .unwrap();
            }
        }
        for i in 0..120 {
            insert_entry(
                &e,
                &Entry {
                    id: format!("library-history-{i}"),
                    kind: if i % 2 == 0 { "note" } else { "memory" }.into(),
                    content: "y".repeat(24_576),
                    updated_at: "2026-10-29".into(),
                    ..Default::default()
                },
                "2026-10-29",
            );
        }
        for i in 0..40 {
            insert_entry(
                &e,
                &Entry {
                    id: format!("term-{i}"),
                    kind: "term".into(),
                    title: format!("CanonicalTerm{i}"),
                    aliases: format!("LegacyAlias{i}"),
                    updated_at: "2026-10-30".into(),
                    ..Default::default()
                },
                "2026-10-30",
            );
        }
        assert_same_terms(&e, &[]);
        let mut full_us = Vec::new();
        let mut terms_us = Vec::new();
        // Alternate calls against one warm database; this is a lookup-only
        // synthetic observation, not an inference or process RSS benchmark.
        for _ in 0..7 {
            let started = Instant::now();
            let full = e.library_entries().unwrap();
            full_us.push(started.elapsed().as_micros());
            std::hint::black_box(full);
            let started = Instant::now();
            let terms = e.terminology_entries().unwrap();
            terms_us.push(started.elapsed().as_micros());
            std::hint::black_box(terms);
        }
        let full = e.library_entries().unwrap();
        let terms = e.terminology_entries().unwrap();
        let query_plan: Vec<String> = e
            .store
            .lock()
            .unwrap()
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {TERMINOLOGY_ENTRIES_SQL}"))
            .unwrap()
            .query_map([], |row| row.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(query_plan
            .iter()
            .all(|row| !row.contains("notes") && !row.contains("meetings")));
        println!(
            "{}",
            serde_json::json!({
                "historical_meetings": 320,
                "historical_notes": 320,
                "history_note_content_bytes_each": 65_536,
                "persisted_nonterm_entries": 120,
                "nonterm_content_bytes_each": 24_576,
                "term_rows": terms.len(),
                "all_rows": full.len(),
                "all_serialized_bytes": serde_json::to_vec(&full).unwrap().len(),
                "term_serialized_bytes": serde_json::to_vec(&terms).unwrap().len(),
                "lookup_full_us": full_us,
                "lookup_terms_us": terms_us,
                "query_plan": query_plan,
                "fingerprint_equal": Terms::new(&[], &full).fingerprint() == Terms::new(&[], &terms).fingerprint(),
                "recognition_hotwords_equal": Terms::new(&[], &full).recognition_hotwords() == Terms::new(&[], &terms).recognition_hotwords()
            })
        );
    }
    #[test]
    fn knowledge_is_opt_in_and_deletion_is_reversible() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        let mut note = e
            .save_library_entry(Entry {
                title: "Test".into(),
                content: "Private note".into(),
                ..Default::default()
            })
            .unwrap();
        assert!(!e.library_entries().unwrap()[0].knowledge);
        note.knowledge = true;
        note.deleted = true;
        e.save_library_entry(note.clone()).unwrap();
        assert!(e.library_entries().unwrap()[0].deleted);
        note.deleted = false;
        e.save_library_entry(note).unwrap();
        assert!(e.library_entries().unwrap()[0].knowledge);
    }
    #[test]
    fn private_context_is_opt_in_and_edits_invalidate_existing_minutes() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        let m = e.create_meeting("Context").unwrap();
        e.save_library_entry(Entry {
            kind: "memory".into(),
            content: "Concise paragraphs".into(),
            ..Default::default()
        })
        .unwrap();
        e.save_library_entry(Entry {
            title: "Project Owl".into(),
            content: "Private background".into(),
            knowledge: true,
            ..Default::default()
        })
        .unwrap();
        e.save_library_entry(Entry {
            kind: "term".into(),
            title: "OmaScribe".into(),
            content: "Product name".into(),
            ..Default::default()
        })
        .unwrap();
        let context = e.writer_context(&[], "Project Owl").unwrap();
        assert_eq!(context["writing_preferences"].as_array().unwrap().len(), 0);
        assert_eq!(context["background_notes"].as_array().unwrap().len(), 0);
        assert_eq!(context["glossary"].as_array().unwrap().len(), 1);
        let mut prefs = e.preferences.lock().unwrap().clone();
        prefs.use_memory = true;
        prefs.use_knowledge = true;
        e.update_preferences(prefs).unwrap();
        let context = e.writer_context(&[], "Project Owl").unwrap();
        assert_eq!(context["background_notes"].as_array().unwrap().len(), 1);
        assert_eq!(context["writing_preferences"].as_array().unwrap().len(), 1);
        assert!(e.context_revision.load(std::sync::atomic::Ordering::SeqCst) > 0);
        assert!(e
            .snapshot(&m.id)
            .unwrap()
            .meeting
            .notes_fingerprint
            .is_empty());
    }
}
