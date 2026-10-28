use crate::types::*;
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;

pub struct Store {
    pub conn: Connection,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        let previous_version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS meetings(id TEXT PRIMARY KEY, json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS segments(id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, run INTEGER NOT NULL, seq INTEGER NOT NULL, json TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS segments_meeting ON segments(meeting_id,run,seq);
            CREATE TABLE IF NOT EXISTS segment_history(id TEXT NOT NULL, revision INTEGER NOT NULL, json TEXT NOT NULL, PRIMARY KEY(id,revision));
            CREATE TABLE IF NOT EXISTS segment_text_edits(id TEXT NOT NULL, revision INTEGER NOT NULL, origin TEXT NOT NULL, PRIMARY KEY(id,revision));
            CREATE TABLE IF NOT EXISTS notes(id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS suggestions(id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS document_history(id INTEGER PRIMARY KEY, meeting_id TEXT NOT NULL, created_at TEXT NOT NULL, json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS inference_jobs(id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, started_at TEXT NOT NULL, input TEXT NOT NULL, outcome TEXT, elapsed_ms INTEGER);
            CREATE TABLE IF NOT EXISTS recording_takes(id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, number INTEGER NOT NULL, json TEXT NOT NULL, UNIQUE(meeting_id,number));
            CREATE TABLE IF NOT EXISTS translations(segment_id TEXT NOT NULL,revision INTEGER NOT NULL,target TEXT NOT NULL,model TEXT NOT NULL,json TEXT NOT NULL, PRIMARY KEY(segment_id,revision,target,model));
            CREATE TABLE IF NOT EXISTS translation_parts(segment_id TEXT PRIMARY KEY,cache_key TEXT NOT NULL,json TEXT NOT NULL);
            CREATE TRIGGER IF NOT EXISTS translation_parts_update AFTER UPDATE ON segments WHEN OLD.json!=NEW.json OR OLD.run!=NEW.run OR OLD.meeting_id!=NEW.meeting_id BEGIN
              DELETE FROM translation_parts WHERE segment_id=OLD.id;
            END;
            CREATE TRIGGER IF NOT EXISTS translation_parts_delete AFTER DELETE ON segments BEGIN
              DELETE FROM translation_parts WHERE segment_id=OLD.id;
            END;
            CREATE TABLE IF NOT EXISTS deleted_meetings(id TEXT PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS library_entries(id TEXT PRIMARY KEY,json TEXT NOT NULL,updated_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS term_checks(segment_id TEXT PRIMARY KEY,run INTEGER NOT NULL,revision INTEGER NOT NULL,terms_key TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS cache_dependencies(meeting_id TEXT NOT NULL,kind TEXT NOT NULL,value TEXT NOT NULL,PRIMARY KEY(meeting_id,kind));
            CREATE TRIGGER IF NOT EXISTS actor_source_insert AFTER INSERT ON segments BEGIN
              INSERT INTO cache_dependencies SELECT NEW.meeting_id,'source-generation','1' WHERE NOT EXISTS(SELECT 1 FROM deleted_meetings WHERE id=NEW.meeting_id) ON CONFLICT(meeting_id,kind) DO UPDATE SET value=CAST(value AS INTEGER)+1;
            END;
            CREATE TRIGGER IF NOT EXISTS actor_source_update AFTER UPDATE ON segments WHEN OLD.json!=NEW.json OR OLD.run!=NEW.run OR OLD.meeting_id!=NEW.meeting_id OR OLD.seq!=NEW.seq BEGIN
              INSERT INTO cache_dependencies SELECT OLD.meeting_id,'source-generation','1' WHERE NOT EXISTS(SELECT 1 FROM deleted_meetings WHERE id=OLD.meeting_id) ON CONFLICT(meeting_id,kind) DO UPDATE SET value=CAST(value AS INTEGER)+1;
              INSERT INTO cache_dependencies SELECT NEW.meeting_id,'source-generation','1' WHERE NEW.meeting_id!=OLD.meeting_id AND NOT EXISTS(SELECT 1 FROM deleted_meetings WHERE id=NEW.meeting_id) ON CONFLICT(meeting_id,kind) DO UPDATE SET value=CAST(value AS INTEGER)+1;
            END;
            CREATE TRIGGER IF NOT EXISTS actor_source_delete AFTER DELETE ON segments BEGIN
              INSERT INTO cache_dependencies SELECT OLD.meeting_id,'source-generation','1' WHERE NOT EXISTS(SELECT 1 FROM deleted_meetings WHERE id=OLD.meeting_id) ON CONFLICT(meeting_id,kind) DO UPDATE SET value=CAST(value AS INTEGER)+1;
            END;
            CREATE TABLE IF NOT EXISTS term_queue(segment_id TEXT PRIMARY KEY,meeting_id TEXT NOT NULL,run INTEGER NOT NULL,seq INTEGER NOT NULL,revision INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS term_queue_scope ON term_queue(meeting_id,run,seq,segment_id);
            CREATE TRIGGER IF NOT EXISTS term_source_insert AFTER INSERT ON segments BEGIN
              INSERT INTO term_queue VALUES(NEW.id,NEW.meeting_id,NEW.run,NEW.seq,COALESCE(json_extract(NEW.json,'$.revision'),1)) ON CONFLICT(segment_id) DO UPDATE SET meeting_id=excluded.meeting_id,run=excluded.run,seq=excluded.seq,revision=excluded.revision;
            END;
            CREATE TRIGGER IF NOT EXISTS term_source_update AFTER UPDATE ON segments WHEN OLD.json!=NEW.json OR OLD.run!=NEW.run OR OLD.meeting_id!=NEW.meeting_id OR OLD.seq!=NEW.seq BEGIN
              INSERT INTO term_queue VALUES(NEW.id,NEW.meeting_id,NEW.run,NEW.seq,COALESCE(json_extract(NEW.json,'$.revision'),1)) ON CONFLICT(segment_id) DO UPDATE SET meeting_id=excluded.meeting_id,run=excluded.run,seq=excluded.seq,revision=excluded.revision;
            END;
            CREATE TRIGGER IF NOT EXISTS term_source_delete AFTER DELETE ON segments BEGIN
              DELETE FROM term_queue WHERE segment_id=OLD.id;
              DELETE FROM term_checks WHERE segment_id=OLD.id;
            END;
            PRAGMA user_version=8;")?;
        if previous_version < 6 {
            conn.execute("INSERT OR IGNORE INTO term_queue SELECT s.id,s.meeting_id,s.run,s.seq,COALESCE(json_extract(s.json,'$.revision'),1) FROM segments s JOIN meetings m ON m.id=s.meeting_id WHERE s.run=json_extract(m.json,'$.run')", [])?;
        }
        conn.execute("UPDATE inference_jobs SET outcome='{\"status\":\"interrupted\"}' WHERE outcome IS NULL", [])?;
        Ok(Self { conn })
    }
    pub fn save_meeting(&self, m: &Meeting) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meetings VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET json=excluded.json",
            params![m.id, serde_json::to_string(m)?],
        )?;
        Ok(())
    }
    pub fn takes(&self, meeting: &str) -> Result<Vec<RecordingTake>> {
        self.json_query(
            "SELECT json FROM recording_takes WHERE meeting_id=?1 ORDER BY number",
            &[&meeting],
        )
    }
    pub fn save_take(&self, take: &RecordingTake) -> Result<()> {
        self.conn.execute("INSERT INTO recording_takes VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET json=excluded.json", params![take.id,take.meeting_id,take.number,serde_json::to_string(take)?])?;
        Ok(())
    }
    pub fn meeting(&self, id: &str) -> Result<Meeting> {
        let s: String = self
            .conn
            .query_row("SELECT json FROM meetings WHERE id=?1", [id], |r| r.get(0))
            .context("找不到这场会议")?;
        Ok(serde_json::from_str(&s)?)
    }
    pub fn meetings(&self) -> Result<Vec<Meeting>> {
        let mut list: Vec<Meeting> = self.json_query(
            "SELECT json FROM meetings WHERE id NOT IN (SELECT id FROM deleted_meetings)",
            &[],
        )?;
        list.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(list)
    }
    pub fn segments(&self, m: &Meeting) -> Result<Vec<Segment>> {
        self.json_query(
            "SELECT json FROM segments WHERE meeting_id=?1 AND run=?2 ORDER BY seq",
            &[&m.id, &m.run],
        )
    }
    /// Revisit changed evidence only. LIMIT bounds temporary Rust allocations for long meetings.
    pub(crate) fn terminology_candidates(&self, m: &Meeting) -> Result<Vec<Segment>> {
        self.json_query("SELECT s.json FROM term_queue q JOIN segments s ON s.id=q.segment_id WHERE q.meeting_id=?1 AND q.run=?2 ORDER BY q.seq,q.segment_id LIMIT 128", &[&m.id, &m.run])
    }
    pub(crate) fn schedule_terminology(&self, m: &Meeting, key: &str) -> Result<()> {
        if self.cache_dependency(&m.id, "terms")? != key {
            self.conn.execute("INSERT INTO term_queue SELECT s.id,s.meeting_id,s.run,s.seq,COALESCE(json_extract(s.json,'$.revision'),1) FROM segments s LEFT JOIN term_checks c ON c.segment_id=s.id WHERE s.meeting_id=?1 AND s.run=?2 AND (c.segment_id IS NULL OR c.run!=s.run OR c.revision!=json_extract(s.json,'$.revision') OR c.terms_key!=?3) ON CONFLICT(segment_id) DO UPDATE SET meeting_id=excluded.meeting_id,run=excluded.run,seq=excluded.seq,revision=excluded.revision",params![m.id,m.run,key])?;
        }
        Ok(())
    }
    pub(crate) fn mark_terminology_batch(&mut self, segments: &[Segment], key: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut mark = tx.prepare("INSERT INTO term_checks VALUES(?1,?2,?3,?4) ON CONFLICT(segment_id) DO UPDATE SET run=excluded.run,revision=excluded.revision,terms_key=excluded.terms_key")?;
            let mut dequeue = tx
                .prepare("DELETE FROM term_queue WHERE segment_id=?1 AND run=?2 AND revision=?3")?;
            for segment in segments {
                mark.execute(params![segment.id, segment.run, segment.revision, key])?;
                dequeue.execute(params![segment.id, segment.run, segment.revision])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn cache_dependency(&self, meeting: &str, kind: &str) -> Result<String> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM cache_dependencies WHERE meeting_id=?1 AND kind=?2",
                params![meeting, kind],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default())
    }
    pub(crate) fn set_cache_dependency(
        &self,
        meeting: &str,
        kind: &str,
        value: &str,
    ) -> Result<()> {
        self.conn.execute("INSERT INTO cache_dependencies VALUES(?1,?2,?3) ON CONFLICT(meeting_id,kind) DO UPDATE SET value=excluded.value WHERE value!=excluded.value",params![meeting,kind,value])?;
        Ok(())
    }
    pub fn recent_segments(&self, meeting: &Meeting, limit: u32) -> Result<Vec<Segment>> {
        let limit = limit.clamp(1, 20);
        let mut segments: Vec<Segment> = self.json_query(
            "SELECT json FROM segments WHERE meeting_id=?1 AND run=?2 ORDER BY seq DESC LIMIT ?3",
            &[&meeting.id, &meeting.run, &limit],
        )?;
        segments.reverse();
        Ok(segments)
    }
    pub fn save_segment(&self, s: &Segment) -> Result<()> {
        self.conn.execute("INSERT INTO segments VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET json=excluded.json",params![s.id,s.meeting_id,s.run,s.seq,serde_json::to_string(s)?])?;
        Ok(())
    }
    pub fn edit_segment(&mut self, id: &str, text: &str, expected_revision: u32) -> Result<()> {
        self.edit_segment_text(id, text, expected_revision, "human")
    }
    pub(crate) fn edit_segment_text(
        &mut self,
        id: &str,
        text: &str,
        expected_revision: u32,
        origin: &str,
    ) -> Result<()> {
        let original: String =
            self.conn
                .query_row("SELECT json FROM segments WHERE id=?1", [id], |r| r.get(0))?;
        let mut s: Segment = serde_json::from_str(&original)?;
        if s.revision != expected_revision {
            bail!("转写已更新，请刷新后再修改");
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO segment_history VALUES(?1,?2,?3)",
            params![s.id, s.revision, original],
        )?;
        s.text = text.to_owned();
        s.revision += 1;
        tx.execute(
            "UPDATE segments SET json=?2 WHERE id=?1",
            params![id, serde_json::to_string(&s)?],
        )?;
        tx.execute(
            "INSERT INTO segment_text_edits VALUES(?1,?2,?3)",
            params![id, s.revision, origin],
        )?;
        tx.commit()?;
        for mut suggestion in self.suggestions(&s.meeting_id)? {
            if suggestion
                .sources
                .iter()
                .any(|source| source.segment_id == id)
            {
                suggestion.stale = true;
                self.save_suggestion(&suggestion)?;
            }
        }
        Ok(())
    }
    pub fn notes(&self, meeting: &str) -> Result<Vec<UserNote>> {
        let mut rows: Vec<UserNote> =
            self.json_query("SELECT notes.json FROM notes LEFT JOIN library_entries l ON l.id=notes.id WHERE meeting_id=?1 AND COALESCE(json_extract(l.json,'$.deleted'),0)=0", &[&meeting])?;
        rows.sort_by(|a, b| {
            a.anchor_ms
                .cmp(&b.anchor_ms)
                .then(b.updated_at.cmp(&a.updated_at))
        });
        Ok(rows)
    }
    pub fn save_note(&self, n: &UserNote) -> Result<()> {
        self.conn.execute(
            "INSERT INTO notes VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET json=excluded.json",
            params![n.id, n.meeting_id, serde_json::to_string(n)?],
        )?;
        Ok(())
    }
    pub fn suggestions(&self, meeting: &str) -> Result<Vec<Suggestion>> {
        self.json_query(
            "SELECT json FROM suggestions WHERE meeting_id=?1 ORDER BY rowid",
            &[&meeting],
        )
    }
    pub fn save_suggestion(&self, s: &Suggestion) -> Result<()> {
        self.conn.execute("INSERT INTO suggestions VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET json=excluded.json",params![s.id,s.meeting_id,serde_json::to_string(s)?])?;
        Ok(())
    }
    pub fn update_suggestion(
        &self,
        meeting: &str,
        id: &str,
        status: &str,
        text: Option<String>,
    ) -> Result<()> {
        if !["accepted", "dismissed", "pending"].contains(&status) {
            bail!("无效的笔记状态");
        }
        let mut s = self
            .suggestions(meeting)?
            .into_iter()
            .find(|s| s.id == id)
            .context("找不到笔记建议")?;
        if let Some(text) = text {
            s.text = text;
        }
        s.status = status.to_owned();
        self.save_suggestion(&s)
    }
    pub fn start_job(&self, id: &str, meeting: &str, input: &serde_json::Value) -> Result<()> {
        self.conn.execute(
            "INSERT INTO inference_jobs(id,meeting_id,started_at,input) VALUES(?1,?2,?3,?4)",
            params![
                id,
                meeting,
                chrono::Utc::now().to_rfc3339(),
                input.to_string()
            ],
        )?;
        Ok(())
    }
    pub fn finish_job(&self, id: &str, outcome: &serde_json::Value, elapsed: u64) -> Result<()> {
        self.conn.execute(
            "UPDATE inference_jobs SET outcome=?2,elapsed_ms=?3 WHERE id=?1",
            params![id, outcome.to_string(), elapsed as i64],
        )?;
        Ok(())
    }
    /// Commit prose, document history and coverage as one transaction after optimistic revision checks.
    pub fn commit_document(
        &mut self,
        before: &Snapshot,
        selected: &[Segment],
        cursor: u32,
        sections: Vec<Suggestion>,
        cancelled: bool,
    ) -> Result<usize> {
        self.commit_document_with_state(before, selected, cursor, sections, cancelled, None)
    }
    pub(crate) fn commit_document_with_state(
        &mut self,
        before: &Snapshot,
        selected: &[Segment],
        cursor: u32,
        mut sections: Vec<Suggestion>,
        cancelled: bool,
        state_environment: Option<&str>,
    ) -> Result<usize> {
        if cancelled {
            bail!("任务已取消，待处理内容已保留");
        }
        let mut current = self.meeting(&before.meeting.id)?;
        let now = self.segments(&current)?;
        let notes = self.notes(&current.id)?;
        // Derive spelling context once at commit, not on every UI fingerprint
        // query. A changed supporting source must not seal an old name repair.
        let actors = |segments: &[Segment], meeting: &Meeting| {
            crate::terminology::Terms::new(&[], &[])
                .with_actor_spellings(
                    segments.iter().map(|s| s.text.as_str()),
                    meeting.speaker_names.values().map(String::as_str),
                )
                .actor_spellings_digest()
        };
        if current.run != before.meeting.run
            || selected
                .iter()
                .any(|s| !now.iter().any(|n| n.id == s.id && n.revision == s.revision))
            || crate::notes::fingerprint(&notes, &now, 0)
                != crate::notes::fingerprint(&before.notes, &before.segments, 0)
            || actors(&now, &current) != actors(&before.segments, &before.meeting)
        {
            bail!("原文或关键词已更新，稍后自动重新整理");
        }
        let existing = self.suggestions(&current.id)?;
        if state_environment.is_some()
            && (current.notes_fingerprint != before.meeting.notes_fingerprint
                || crate::notes::incremental::document_hash(&existing)
                    != crate::notes::incremental::document_hash(&before.suggestions)
                || crate::notes::incremental::source_hash(&now, before.meeting.notes_cursor)
                    != crate::notes::incremental::source_hash(
                        &before.segments,
                        before.meeting.notes_cursor,
                    ))
        {
            bail!("旧纪要或历史原话已更新，本轮增量结果未提交");
        }
        // Recompose only unlocked paragraphs whose entire evidence is in this request.
        // IDs are assigned here, never by the language model. Archive the old document below.
        let replaceable: Vec<&Suggestion> = existing
            .iter()
            .filter(|s| {
                s.status == "pending"
                    && !s.sources.is_empty()
                    && s.sources
                        .iter()
                        .all(|q| selected.iter().any(|e| e.id == q.segment_id))
            })
            .collect();
        let mut remove = std::collections::HashSet::new();
        let mut assigned = std::collections::HashSet::new();
        for section in &mut sections {
            if !section.id.is_empty() {
                let old = existing
                    .iter()
                    .find(|s| s.id == section.id)
                    .context("无效的文档章节")?;
                if old.status != "pending" {
                    bail!("人工保留的章节不能被模型覆盖");
                }
            } else if let Some(old) = replaceable
                .iter()
                .filter(|s| !assigned.contains(&s.id))
                .max_by_key(|s| {
                    let overlap = s
                        .sources
                        .iter()
                        .filter(|q| section.sources.iter().any(|n| n.segment_id == q.segment_id))
                        .count();
                    overlap + usize::from(s.focus == section.focus) * 100
                })
                .filter(|s| {
                    s.sources
                        .iter()
                        .any(|q| section.sources.iter().any(|n| n.segment_id == q.segment_id))
                })
            {
                section.id = old.id.clone();
            } else {
                section.id = uuid::Uuid::new_v4().to_string();
            }
            assigned.insert(section.id.clone());
        }
        if !sections.is_empty() {
            remove.extend(replaceable.into_iter().map(|s| s.id.clone()));
        }
        let mut ids = std::collections::HashSet::new();
        if sections.iter().any(|s| !ids.insert(&s.id)) {
            bail!("模型重复更新了同一个章节");
        }
        sections.retain(|s| {
            !existing.iter().any(|e| {
                e.status != "pending"
                    && (e.text == s.text
                        || s.sources.iter().all(|q| {
                            e.sources.iter().any(|old| {
                                old.segment_id == q.segment_id && old.revision == q.revision
                            })
                        }))
            })
        });
        let count = sections.len();
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO document_history(meeting_id,created_at,json) VALUES(?1,?2,?3)",
            params![
                current.id,
                chrono::Utc::now().to_rfc3339(),
                serde_json::to_string(&existing)?
            ],
        )?;
        for id in remove {
            tx.execute("DELETE FROM suggestions WHERE id=?1", [id])?;
        }
        for section in sections {
            tx.execute(
                "INSERT INTO suggestions VALUES(?1,?2,?3)",
                params![
                    section.id,
                    section.meeting_id,
                    serde_json::to_string(&section)?
                ],
            )?;
        }
        current.notes_cursor = cursor;
        current.notes_fingerprint = crate::notes::fingerprint(&before.notes, &now, cursor);
        tx.execute(
            "UPDATE meetings SET json=?2 WHERE id=?1",
            params![current.id, serde_json::to_string(&current)?],
        )?;
        let state = if let Some(environment) = state_environment {
            let document = {
                let mut q = tx.prepare("SELECT json FROM suggestions WHERE meeting_id=?1")?;
                let rows = q.query_map([&current.id], |r| r.get::<_, String>(0))?;
                rows.map(|row| Ok(serde_json::from_str::<Suggestion>(&row?)?))
                    .collect::<Result<Vec<_>>>()?
            };
            crate::notes::incremental::seal(environment, &current, &now, &notes, &document)
        } else {
            None
        };
        if let Some(state) = state {
            tx.execute("INSERT INTO cache_dependencies VALUES(?1,?2,?3) ON CONFLICT(meeting_id,kind) DO UPDATE SET value=excluded.value",
                params![current.id,crate::notes::incremental::KEY,serde_json::to_string(&state)?])?;
        } else {
            tx.execute(
                "DELETE FROM cache_dependencies WHERE meeting_id=?1 AND kind=?2",
                params![current.id, crate::notes::incremental::KEY],
            )?;
        }
        tx.commit()?;
        Ok(count)
    }
    fn json_query<T: serde::de::DeserializeOwned>(
        &self,
        sql: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<T>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(args, |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminology_queue_migrates_legacy_data_and_preserves_newer_work() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = crate::Engine::open(root.path().into())?;
        let meeting = engine.create_meeting("Legacy migration")?;
        let mut source = Segment {
            id: "legacy-source".into(),
            meeting_id: meeting.id.clone(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 1000,
            text: "Original evidence".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        engine.store.lock().unwrap().save_segment(&source)?;
        drop(engine);
        let path = root.path().join("meetings.sqlite3");
        {
            let conn = Connection::open(&path)?;
            conn.execute_batch("DROP TRIGGER term_source_insert; DROP TRIGGER term_source_update; DROP TRIGGER term_source_delete; DROP TABLE term_queue; DROP TABLE term_checks; DROP TABLE cache_dependencies; PRAGMA user_version=5;")?;
        }
        let mut db = Store::open(&path)?;
        assert_eq!(db.terminology_candidates(&meeting)?.len(), 1);
        let old = source.clone();
        source.text = "Newer evidence".into();
        source.revision = 2;
        db.save_segment(&source)?;
        db.mark_terminology_batch(&[old], "terms-fixture")?;
        // An acknowledgement for rev 1 cannot remove queued rev 2.
        assert_eq!(db.terminology_candidates(&meeting)?[0].revision, 2);
        assert_eq!(db.segments(&meeting)?[0].text, "Newer evidence");
        db.mark_terminology_batch(&[source], "terms-fixture")?;
        assert!(db.terminology_candidates(&meeting)?.is_empty());
        drop(db);
        let reopened = Store::open(&path)?;
        assert!(reopened.terminology_candidates(&meeting)?.is_empty());
        Ok(())
    }
    #[test]
    fn corrections_preserve_history_and_mark_accepted_notes_stale() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let mut db = Store::open(&tmp.path().join("test.db"))?;
        let segment = Segment {
            id: "s1".into(),
            meeting_id: "m1".into(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 1000,
            text: "预算确定".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        db.save_segment(&segment)?;
        db.save_suggestion(&Suggestion {
            focus: String::new(),
            id: "n1".into(),
            meeting_id: "m1".into(),
            kind: "fact".into(),
            text: "人工确认的笔记".into(),
            sources: vec![Source {
                segment_id: "s1".into(),
                revision: 1,
                start_ms: 0,
                end_ms: 1000,
                quote: "预算确定".into(),
                speaker_name: None,
            }],
            status: "accepted".into(),
            stale: false,
        })?;
        db.edit_segment("s1", "预算未确定", 1)?;
        assert!(db.edit_segment("s1", "旧界面覆盖", 1).is_err());
        let s = &db.suggestions("m1")?[0];
        assert!(s.stale);
        assert_eq!(s.text, "人工确认的笔记");
        assert_eq!(s.status, "accepted");
        assert_eq!(
            db.conn
                .query_row("SELECT COUNT(*) FROM segment_history", [], |r| r
                    .get::<_, i64>(0))?,
            1
        );
        Ok(())
    }
}
#[cfg(test)]
mod document_tests {
    use super::*;
    #[test]
    fn source_actor_context_is_checked_once_at_commit_without_rejecting_unrelated_text(
    ) -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = crate::Engine::open(root.path().into())?;
        let m = engine.create_meeting("Actor context fixture")?;
        let source = |id: &str, seq, text: &str| Segment {
            id: id.into(),
            meeting_id: m.id.clone(),
            run: 1,
            seq,
            start_ms: 0,
            end_ms: 1000,
            text: text.into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        let provider = source("actor", 1, "工具由林悦负责测试。");
        let date = source("date", 2, "林悦先在周四提交报告。");
        {
            let db = engine.store.lock().unwrap();
            db.save_segment(&provider)?;
            db.save_segment(&date)?;
        }
        let before = engine.snapshot(&m.id)?;
        let section = Suggestion {
            id: String::new(),
            meeting_id: m.id.clone(),
            kind: "section".into(),
            focus: "提交安排".into(),
            text: "林悦将在周四提交报告。".into(),
            sources: vec![Source {
                segment_id: date.id.clone(),
                revision: 1,
                start_ms: 0,
                end_ms: 1000,
                quote: date.text.clone(),
                speaker_name: None,
            }],
            status: "pending".into(),
            stale: false,
        };
        let mut db = engine.store.lock().unwrap();
        db.edit_segment(&provider.id, "工具由陈明负责测试。", 1)?;
        assert!(db
            .commit_document(
                &before,
                std::slice::from_ref(&date),
                2,
                vec![section.clone()],
                false
            )
            .is_err());
        assert!(db.suggestions(&m.id)?.is_empty());
        db.edit_segment(&provider.id, "工具由林悦负责测试。", 2)?;
        drop(db);
        let before = engine.snapshot(&m.id)?;
        let mut db = engine.store.lock().unwrap();
        db.save_segment(&source("cache", 3, "缓存保存在本机。"))?;
        assert_eq!(
            db.commit_document(&before, &[date], 2, vec![section], false)?,
            1
        );
        Ok(())
    }
    #[test]
    fn document_commit_is_atomic_and_rejects_midflight_edits() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = crate::Engine::open(root.path().to_path_buf())?;
        let m = engine.create_meeting("fixture")?;
        let segment = Segment {
            id: "s1".into(),
            meeting_id: m.id.clone(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 1000,
            text: "预算未定".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        engine.store.lock().unwrap().save_segment(&segment)?;
        let before = engine.snapshot(&m.id)?;
        let section = Suggestion {
            id: String::new(),
            meeting_id: m.id.clone(),
            kind: "section".into(),
            focus: "预算".into(),
            text: "预算尚未确定。".into(),
            sources: vec![Source {
                segment_id: segment.id.clone(),
                revision: 1,
                start_ms: 0,
                end_ms: 1000,
                quote: segment.text.clone(),
                speaker_name: None,
            }],
            status: "pending".into(),
            stale: false,
        };
        let mut db = engine.store.lock().unwrap();
        assert!(db
            .commit_document(
                &before,
                std::slice::from_ref(&segment),
                1,
                vec![section.clone()],
                true
            )
            .is_err());
        assert_eq!(db.meeting(&m.id)?.notes_cursor, 0);
        db.commit_document(
            &before,
            std::slice::from_ref(&segment),
            1,
            vec![section.clone()],
            false,
        )?;
        assert_eq!(db.meeting(&m.id)?.notes_cursor, 1);
        assert_eq!(db.suggestions(&m.id)?.len(), 1);
        let saved = db.suggestions(&m.id)?[0].clone();
        db.update_suggestion(
            &m.id,
            &saved.id,
            "accepted",
            Some("用户手动保留的内容".into()),
        )?;
        let mut replacement = section;
        replacement.id = saved.id;
        assert!(db
            .commit_document(
                &before,
                std::slice::from_ref(&segment),
                1,
                vec![replacement],
                false
            )
            .is_err());
        db.edit_segment("s1", "预算已确定为2000", 1)?;
        assert!(db
            .commit_document(&before, &[segment], 1, vec![], false)
            .is_err());
        assert_eq!(db.suggestions(&m.id)?[0].text, "用户手动保留的内容");
        Ok(())
    }
}
