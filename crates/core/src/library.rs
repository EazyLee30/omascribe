use crate::{preferences::atomic_json, Engine, UserNote};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, path::Path};

const TERMINOLOGY_ENTRIES_SQL: &str =
    "SELECT json FROM library_entries WHERE json_extract(json,'$.kind')='term' ORDER BY updated_at DESC";

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
        let entries = self.library_entries()?;
        let spellings = crate::terminology::Terms::new(notes, &entries).spellings(transcript);
        let terms:Vec<_>=entries.iter().filter(|e|!e.deleted&&e.enabled&&e.kind=="term").take(40).map(|e|serde_json::json!({"term":e.title,"aliases":e.aliases,"definition":e.content.chars().take(160).collect::<String>()})).collect();
        let memory: Vec<_> = entries
            .iter()
            .filter(|e| prefs.use_memory && !e.deleted && e.enabled && e.kind == "memory")
            .take(8)
            .map(|e| e.content.chars().take(240).collect::<String>())
            .collect();
        let mut query = crate::notes::focus_terms(notes);
        query.push(transcript.to_owned());
        let candidates: Vec<_> = entries
            .iter()
            .filter(|e| prefs.use_knowledge && !e.deleted && e.enabled && e.knowledge)
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
        Ok(context)
    }
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
