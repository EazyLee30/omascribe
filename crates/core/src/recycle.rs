//! Reversible entity removal. Original audio and text remain local.
use crate::{Engine, Meeting};
use anyhow::{Context, Result};
impl Engine {
    pub fn deleted_meetings(&self) -> Result<Vec<Meeting>> {
        let db = self.store.lock().unwrap();
        let mut q = db
            .conn
            .prepare("SELECT json FROM meetings WHERE id IN (SELECT id FROM deleted_meetings)")?;
        let rows = q
            .query_map([], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect();
        rows
    }
    pub fn set_meeting_deleted(&self, id: &str, deleted: bool) -> Result<()> {
        let _gate = self.config_lock.lock().unwrap();
        self.configuration_idle()?;
        let mut db = self.store.lock().unwrap();
        db.meeting(id)?;
        let tx = db.conn.transaction()?;
        if deleted {
            tx.execute("INSERT OR IGNORE INTO deleted_meetings VALUES(?1)", [id])?;
            tx.execute("DELETE FROM translations WHERE segment_id IN (SELECT id FROM segments WHERE meeting_id=?1)",[id])?;
            tx.execute("DELETE FROM translation_parts WHERE segment_id IN (SELECT id FROM segments WHERE meeting_id=?1)",[id])?;
            tx.execute("DELETE FROM term_checks WHERE segment_id IN (SELECT id FROM segments WHERE meeting_id=?1)",[id])?;
            tx.execute("DELETE FROM term_queue WHERE meeting_id=?1", [id])?;
            tx.execute("DELETE FROM cache_dependencies WHERE meeting_id=?1", [id])?;
        } else {
            tx.execute("DELETE FROM deleted_meetings WHERE id=?1", [id])?;
        }
        tx.commit()?;
        drop(db);
        if deleted {
            self.retrieval_cache.clear_meeting(id);
        }
        self.invalidate_background()
    }
    pub fn delete_speaker(&self, meeting: &str, id: &str) -> Result<()> {
        let _gate = self.config_lock.lock().unwrap();
        if self.status.lock().unwrap().diarization_busy {
            anyhow::bail!("Wait for speaker analysis to finish");
        }
        let mut db = self.store.lock().unwrap();
        let mut m = db.meeting(meeting)?;
        m.speaker_names.remove(id).context("Unknown speaker")?;
        let segments = db.segments(&m)?;
        let mut suggestions = db.suggestions(meeting)?;
        let tx = db.conn.transaction()?;
        for mut s in segments
            .into_iter()
            .filter(|s| s.speaker.as_deref() == Some(id))
        {
            tx.execute(
                "INSERT OR IGNORE INTO segment_history VALUES(?1,?2,?3)",
                rusqlite::params![s.id, s.revision, serde_json::to_string(&s)?],
            )?;
            s.speaker = None;
            s.speaker_manual = true;
            s.revision += 1;
            tx.execute(
                "UPDATE segments SET json=?2 WHERE id=?1",
                rusqlite::params![s.id, serde_json::to_string(&s)?],
            )?;
            for suggestion in &mut suggestions {
                if suggestion
                    .sources
                    .iter()
                    .any(|source| source.segment_id == s.id)
                {
                    suggestion.stale = true;
                }
            }
        }
        m.notes_fingerprint.clear();
        tx.execute(
            "UPDATE meetings SET json=?2 WHERE id=?1",
            rusqlite::params![m.id, serde_json::to_string(&m)?],
        )?;
        for s in suggestions {
            tx.execute(
                "UPDATE suggestions SET json=?2 WHERE id=?1",
                rusqlite::params![s.id, serde_json::to_string(&s)?],
            )?;
        }
        tx.commit()?;
        drop(db);
        self.sync_meeting_files(meeting);
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deleting_and_restoring_preserves_meeting_and_note_content() -> Result<()> {
        let root = tempfile::tempdir()?;
        let e = Engine::open(root.path().into())?;
        let m = e.create_meeting("Keep audio")?;
        let n = e.save_note(&m.id, None, "Original document", "document", 0)?;
        let mut entry = e
            .library_entries()?
            .into_iter()
            .find(|v| v.id == n.id)
            .unwrap();
        entry.deleted = true;
        e.save_library_entry(entry.clone())?;
        assert!(e.snapshot(&m.id)?.notes.is_empty());
        assert_eq!(e.library_entries()?[0].content, "Original document");
        entry.deleted = false;
        e.save_library_entry(entry)?;
        assert_eq!(e.snapshot(&m.id)?.notes[0].content, "Original document");
        let source = crate::Segment {
            id: "retained-source".into(),
            meeting_id: m.id.clone(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 1000,
            text: "Original speech".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        {
            let db = e.store.lock().unwrap();
            db.save_segment(&source)?;
            db.conn.execute(
                "INSERT INTO translations VALUES(?1,1,'zh','fixture','{}')",
                [&source.id],
            )?;
            db.conn.execute(
                "INSERT INTO translation_parts VALUES(?1,'fixture','{}')",
                [&source.id],
            )?;
            db.set_cache_dependency(&m.id, "terms", "fixture")?;
            // Any failure must roll back both the deletion marker and the cache purge.
            db.conn.execute_batch("CREATE TRIGGER reject_purge BEFORE DELETE ON translations BEGIN SELECT RAISE(ABORT,'simulated failure'); END;")?;
        }
        assert!(e.set_meeting_deleted(&m.id, true).is_err());
        assert_eq!(e.meetings()?.len(), 1);
        e.store
            .lock()
            .unwrap()
            .conn
            .execute_batch("DROP TRIGGER reject_purge;")?;
        e.set_meeting_deleted(&m.id, true)?;
        assert!(e.meetings()?.is_empty());
        assert!(e.library_entries()?.is_empty());
        assert_eq!(e.deleted_meetings()?.len(), 1);
        {
            let db = e.store.lock().unwrap();
            for table in [
                "translations",
                "translation_parts",
                "term_checks",
                "term_queue",
                "cache_dependencies",
            ] {
                let count: u32 =
                    db.conn
                        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
                assert_eq!(count, 0, "{table}");
            }
        }
        e.set_meeting_deleted(&m.id, false)?;
        assert_eq!(e.meetings()?.len(), 1);
        assert_eq!(e.snapshot(&m.id)?.notes[0].content, "Original document");
        assert_eq!(e.snapshot(&m.id)?.segments[0].text, source.text);
        Ok(())
    }
    #[test]
    fn speaker_deletion_preserves_words_and_revision_history() -> Result<()> {
        let root = tempfile::tempdir()?;
        let e = Engine::open(root.path().into())?;
        let m = e.create_meeting("Speakers")?;
        let id = e.rename_speaker(&m.id, None, "Alice")?;
        let s = crate::Segment {
            id: "passage".into(),
            meeting_id: m.id.clone(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 1000,
            text: "Keep these words".into(),
            revision: 1,
            speaker: Some(id.clone()),
            speaker_manual: false,
        };
        e.store.lock().unwrap().save_segment(&s)?;
        e.delete_speaker(&m.id, &id)?;
        let snap = e.snapshot(&m.id)?;
        assert!(snap.meeting.speaker_names.is_empty());
        assert_eq!(snap.segments[0].text, s.text);
        assert!(snap.segments[0].speaker.is_none() && snap.segments[0].speaker_manual);
        assert_eq!(snap.segments[0].revision, 2);
        assert_eq!(
            e.store.lock().unwrap().conn.query_row(
                "SELECT COUNT(*) FROM segment_history",
                [],
                |r| r.get::<_, u32>(0)
            )?,
            1
        );
        Ok(())
    }
}
