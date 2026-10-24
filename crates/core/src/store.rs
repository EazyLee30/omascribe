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
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS meetings(id TEXT PRIMARY KEY, json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS segments(id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, run INTEGER NOT NULL, seq INTEGER NOT NULL, json TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS segments_meeting ON segments(meeting_id,run,seq);
            CREATE TABLE IF NOT EXISTS segment_history(id TEXT NOT NULL, revision INTEGER NOT NULL, json TEXT NOT NULL, PRIMARY KEY(id,revision));
            CREATE TABLE IF NOT EXISTS notes(id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS suggestions(id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS document_history(id INTEGER PRIMARY KEY, meeting_id TEXT NOT NULL, created_at TEXT NOT NULL, json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS inference_jobs(id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, started_at TEXT NOT NULL, input TEXT NOT NULL, outcome TEXT, elapsed_ms INTEGER);
            CREATE TABLE IF NOT EXISTS recording_takes(id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, number INTEGER NOT NULL, json TEXT NOT NULL, UNIQUE(meeting_id,number));
            PRAGMA user_version=3;")?;
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
        let mut list: Vec<Meeting> = self.json_query("SELECT json FROM meetings", &[])?;
        list.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(list)
    }
    pub fn segments(&self, m: &Meeting) -> Result<Vec<Segment>> {
        self.json_query(
            "SELECT json FROM segments WHERE meeting_id=?1 AND run=?2 ORDER BY seq",
            &[&m.id, &m.run],
        )
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
            self.json_query("SELECT json FROM notes WHERE meeting_id=?1", &[&meeting])?;
        rows.sort_by_key(|n| n.anchor_ms);
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
        mut sections: Vec<Suggestion>,
        cancelled: bool,
    ) -> Result<usize> {
        if cancelled {
            bail!("任务已取消，待处理内容已保留");
        }
        let mut current = self.meeting(&before.meeting.id)?;
        let now = self.segments(&current)?;
        let notes = self.notes(&current.id)?;
        if current.run != before.meeting.run
            || selected
                .iter()
                .any(|s| !now.iter().any(|n| n.id == s.id && n.revision == s.revision))
            || crate::notes::fingerprint(&notes, &now, 0)
                != crate::notes::fingerprint(&before.notes, &before.segments, 0)
        {
            bail!("原文或关键词已更新，稍后自动重新整理");
        }
        let existing = self.suggestions(&current.id)?;
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
