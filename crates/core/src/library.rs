use crate::{preferences::atomic_json, Engine, UserNote};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, path::Path};
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
    pub fn save_library_entry(&self, mut entry: Entry) -> Result<Entry> {
        let prefs = self.preferences.lock().unwrap().clone();
        let relevant = |e: &Entry| {
            !e.deleted
                && e.enabled
                && (e.kind == "term"
                    || e.kind == "memory" && prefs.use_memory
                    || e.kind == "note" && e.knowledge && prefs.use_knowledge)
        };
        let was_context = self
            .library_entries()?
            .iter()
            .any(|e| e.id == entry.id && relevant(e));
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
            let notes = self.store.lock().unwrap().notes(mid)?;
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
        if was_context || relevant(&entry) {
            self.invalidate_background()?;
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
        Ok(
            serde_json::json!({"glossary":terms,"writing_preferences":memory,"background_notes":knowledge}),
        )
    }
}
#[derive(Serialize)]
pub struct Storage {
    pub recordings: u64,
    pub database: u64,
    pub models: u64,
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
        downloads,
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
