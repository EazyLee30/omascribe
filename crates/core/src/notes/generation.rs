//! Job-wide request accounting and optimistic identity checks between local stages.
//! Checks retain no database/configuration guard and do not inspect model files.
use crate::{preferences::Preferences, Engine, Segment, Snapshot, UserNote};
use anyhow::{bail, Context, Result};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{cell::Cell, collections::BTreeMap, sync::atomic::Ordering};

/// Shared by all stages and CPU fallback invocations of one generation job.
#[derive(Default)]
pub(super) struct Budget {
    used: Cell<usize>,
}
impl Budget {
    pub(super) fn take(&self) -> Result<usize> {
        self.ensure_remaining()?;
        let sequence = self.used.get() + 1;
        self.used.set(sequence);
        Ok(sequence)
    }

    pub(super) fn ensure_remaining(&self) -> Result<()> {
        if self.used.get() >= 2 {
            bail!("本轮整理已达到请求预算，原文已保留，稍后自动重试");
        }
        Ok(())
    }
}

pub(super) struct Scope {
    meeting: String,
    run: u32,
    cursor: u32,
    document: String,
    preferences: Value,
    notes: String,
    speakers: BTreeMap<String, String>,
    actors: String,
    terms: String,
    context_revision: u64,
}
impl Scope {
    pub(super) fn new(
        engine: &Engine,
        snapshot: &Snapshot,
        prefs: &Preferences,
        context_revision: u64,
    ) -> Result<Self> {
        let actors = crate::terminology::Terms::new(&[], &[])
            .with_actor_spellings(
                snapshot
                    .segments
                    .iter()
                    .filter(|s| {
                        s.meeting_id == snapshot.meeting.id && s.run == snapshot.meeting.run
                    })
                    .map(|s| s.text.as_str()),
                snapshot.meeting.speaker_names.values().map(String::as_str),
            )
            .actor_spellings_digest();
        let terms = engine
            .store
            .lock()
            .unwrap()
            .cache_dependency(&snapshot.meeting.id, "terms")?;
        let scope = Self {
            meeting: snapshot.meeting.id.clone(),
            run: snapshot.meeting.run,
            cursor: snapshot.meeting.notes_cursor,
            document: super::incremental::document_hash(&snapshot.suggestions),
            preferences: preferences(prefs)?,
            notes: notes(&snapshot.notes)?,
            speakers: snapshot.meeting.speaker_names.clone(),
            actors,
            terms,
            context_revision,
        };
        // The selected request context is checked separately before its first
        // chat. Avoid one PK query per historical passage on long meetings.
        scope.check(engine, &[])?;
        Ok(scope)
    }

    pub(super) fn check(&self, engine: &Engine, selected: &[Segment]) -> Result<()> {
        self.check_revision(engine)?;
        self.check_preferences(engine)?;
        {
            let db = engine.store.lock().unwrap();
            self.check_store(&db, selected)?;
        }
        self.check_preferences(engine)?;
        self.check_revision(engine)
    }

    /// Recheck database identities while the caller retains the commit lock.
    /// This helper never reads engine preferences or reacquires the store.
    pub(super) fn check_store(&self, db: &crate::store::Store, selected: &[Segment]) -> Result<()> {
        let deleted: bool = db.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM deleted_meetings WHERE id=?1)",
            [&self.meeting],
            |row| row.get(0),
        )?;
        if deleted {
            bail!("会议已移入已删除，本轮结果未提交");
        }
        let meeting = db.meeting(&self.meeting)?;
        if meeting.id != self.meeting
            || meeting.run != self.run
            || meeting.notes_cursor != self.cursor
            || meeting.speaker_names != self.speakers
            || notes(&db.notes(&self.meeting)?)? != self.notes
            || super::incremental::document_hash(&db.suggestions(&self.meeting)?) != self.document
            || db.cache_dependency(&self.meeting, "terms")? != self.terms
        {
            bail!("原文、笔记或名称配置已更新，稍后自动重新整理");
        }
        for source in selected {
            if source.meeting_id != self.meeting || source.run != self.run {
                bail!("来源不属于本轮会议，待处理内容已保留");
            }
            let encoded: Option<String> = db
                .conn
                .query_row(
                    "SELECT json FROM segments WHERE id=?1 AND meeting_id=?2 AND run=?3 AND seq=?4",
                    params![source.id, self.meeting, self.run, source.seq],
                    |row| row.get(0),
                )
                .optional()?;
            let current: Segment =
                serde_json::from_str(encoded.as_deref().context("来源已移除，待处理内容已保留")?)?;
            // Compare every source field, including exact text, timestamps
            // and speaker binding even if a writer failed to bump revision.
            if serde_json::to_value(&current)? != serde_json::to_value(source)? {
                bail!("来源已更新，待处理内容已保留");
            }
        }
        // Ordinary ASR appends may proceed. Only a changed actor spelling
        // environment invalidates preparation based on unselected sources.
        if crate::source_context::digest(db, &meeting)? != self.actors {
            bail!("来源名称环境已更新，稍后自动重新整理");
        }
        Ok(())
    }

    fn check_preferences(&self, engine: &Engine) -> Result<()> {
        let current = engine.preferences.lock().unwrap().clone();
        if preferences(&current)? != self.preferences {
            bail!("生成配置已更新，稍后自动重新整理");
        }
        Ok(())
    }

    fn check_revision(&self, engine: &Engine) -> Result<()> {
        if engine.context_revision.load(Ordering::SeqCst) != self.context_revision {
            bail!("辅助背景已更新，将使用新配置重新整理");
        }
        Ok(())
    }
}

fn preferences(prefs: &Preferences) -> Result<Value> {
    let mut value = serde_json::to_value(prefs)?;
    value
        .as_object_mut()
        .context("Invalid generation configuration")?
        .remove("appearance");
    Ok(value)
}

fn notes(notes: &[UserNote]) -> Result<String> {
    let mut ordered: Vec<_> = notes.iter().collect();
    ordered.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&ordered)?)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn source(snapshot: &Snapshot, seq: u32, text: &str) -> Segment {
        Segment {
            id: format!("{}:1:{seq}", snapshot.meeting.id),
            meeting_id: snapshot.meeting.id.clone(),
            run: snapshot.meeting.run,
            seq,
            start_ms: u64::from(seq - 1) * 1000,
            end_ms: u64::from(seq) * 1000,
            text: text.into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        }
    }

    fn seed() -> Result<(tempfile::TempDir, Arc<Engine>, Snapshot, Preferences)> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let meeting = engine.create_meeting("Fictional generation identity")?;
        let initial = engine.snapshot(&meeting.id)?;
        engine.store.lock().unwrap().save_segment(&source(
            &initial,
            1,
            "缓存保存在本机，仍需确认。",
        ))?;
        let snapshot = engine.snapshot(&meeting.id)?;
        let prefs = engine.preferences.lock().unwrap().clone();
        Ok((root, engine, snapshot, prefs))
    }

    fn scope(engine: &Engine, snapshot: &Snapshot, prefs: &Preferences) -> Result<Scope> {
        Scope::new(
            engine,
            snapshot,
            prefs,
            engine.context_revision.load(Ordering::SeqCst),
        )
    }

    fn assert_unlocked(engine: &Engine) {
        assert!(engine.store.try_lock().is_ok());
        assert!(engine.preferences.try_lock().is_ok());
        assert!(engine.config_lock.try_lock().is_ok());
    }

    #[test]
    fn fallback_closures_share_monotonic_two_request_budget() -> Result<()> {
        let budget = Budget::default();
        let operation = || budget.take();
        assert_eq!(operation()?, 1);
        assert_eq!(operation()?, 2);
        assert!(operation().is_err());
        assert!(budget.ensure_remaining().is_err());
        assert!(operation().is_err());
        Ok(())
    }

    #[test]
    fn unchanged_scope_and_appearance_change_leave_all_guards_released() -> Result<()> {
        let (_root, engine, snapshot, prefs) = seed()?;
        let scope = scope(&engine, &snapshot, &prefs)?;
        scope.check(&engine, &snapshot.segments)?;
        assert_unlocked(&engine);
        engine.preferences.lock().unwrap().appearance.accent = "#112233".into();
        scope.check(&engine, &snapshot.segments)?;
        assert_unlocked(&engine);
        Ok(())
    }

    #[test]
    fn selected_full_tuple_changes_are_rejected_even_without_revision_change() -> Result<()> {
        let mutations: &[fn(&mut Segment)] = &[
            |s| s.text = "缓存将上传。".into(),
            |s| s.start_ms += 1,
            |s| s.end_ms += 1,
            |s| s.speaker = Some("speaker-1".into()),
            |s| s.speaker_manual = true,
            |s| s.revision += 1,
            |s| s.seq += 1,
            |s| s.run += 1,
            |s| s.meeting_id = "another-meeting".into(),
        ];
        for mutate in mutations {
            let (_root, engine, snapshot, prefs) = seed()?;
            let scope = scope(&engine, &snapshot, &prefs)?;
            let mut changed = snapshot.segments[0].clone();
            mutate(&mut changed);
            engine.store.lock().unwrap().save_segment(&changed)?;
            assert!(scope.check(&engine, &snapshot.segments).is_err());
            assert_unlocked(&engine);
        }
        Ok(())
    }

    #[test]
    fn removed_selected_source_is_rejected() -> Result<()> {
        let (_root, engine, snapshot, prefs) = seed()?;
        let scope = scope(&engine, &snapshot, &prefs)?;
        engine.store.lock().unwrap().conn.execute(
            "DELETE FROM segments WHERE id=?1",
            [&snapshot.segments[0].id],
        )?;
        assert!(scope.check(&engine, &snapshot.segments).is_err());
        assert_unlocked(&engine);
        Ok(())
    }

    #[test]
    fn ordinary_append_is_allowed_but_new_unselected_actor_is_rejected() -> Result<()> {
        let (_root, engine, snapshot, prefs) = seed()?;
        let scope = scope(&engine, &snapshot, &prefs)?;
        engine
            .store
            .lock()
            .unwrap()
            .save_segment(&source(&snapshot, 2, "还需要确认导出格式。"))?;
        scope.check(&engine, &snapshot.segments)?;
        engine.store.lock().unwrap().save_segment(&source(
            &snapshot,
            3,
            "离线转写工具由林悦负责测试。",
        ))?;
        assert!(scope.check(&engine, &snapshot.segments).is_err());
        assert_unlocked(&engine);
        Ok(())
    }

    #[test]
    fn run_and_speaker_changes_are_rejected() -> Result<()> {
        for change_run in [true, false] {
            let (_root, engine, snapshot, prefs) = seed()?;
            let scope = scope(&engine, &snapshot, &prefs)?;
            let mut meeting = snapshot.meeting.clone();
            if change_run {
                meeting.run += 1;
            } else {
                meeting
                    .speaker_names
                    .insert("speaker-1".into(), "林悦".into());
            }
            engine.store.lock().unwrap().save_meeting(&meeting)?;
            assert!(scope.check(&engine, &snapshot.segments).is_err());
            assert_unlocked(&engine);
        }
        Ok(())
    }

    #[test]
    fn notes_terms_preferences_and_background_changes_are_rejected() -> Result<()> {
        for mutation in ["note", "terms", "preferences", "background"] {
            let (_root, engine, snapshot, prefs) = seed()?;
            let scope = scope(&engine, &snapshot, &prefs)?;
            match mutation {
                "note" => {
                    engine.save_note(&snapshot.meeting.id, None, "新的整理重点", "note", 0)?;
                }
                "terms" => engine.store.lock().unwrap().set_cache_dependency(
                    &snapshot.meeting.id,
                    "terms",
                    "updated",
                )?,
                "preferences" => {
                    engine.preferences.lock().unwrap().minutes_language = "en".into();
                }
                "background" => engine.invalidate_background()?,
                _ => unreachable!(),
            }
            assert!(
                scope.check(&engine, &snapshot.segments).is_err(),
                "{mutation}"
            );
            assert_unlocked(&engine);
        }
        Ok(())
    }

    #[test]
    fn a_change_between_snapshot_and_scope_capture_is_not_a_new_baseline() -> Result<()> {
        let (_root, engine, snapshot, prefs) = seed()?;
        let revision = engine.context_revision.load(Ordering::SeqCst);
        engine.save_note(&snapshot.meeting.id, None, "来源已修改", "note", 0)?;
        assert!(Scope::new(&engine, &snapshot, &prefs, revision).is_err());
        assert_unlocked(&engine);
        Ok(())
    }

    #[test]
    fn held_store_recheck_rejects_changes_after_an_earlier_scope_check() -> Result<()> {
        for mutation in ["source_text", "source_seq_column", "document", "cursor"] {
            let (_root, engine, snapshot, prefs) = seed()?;
            let scope = scope(&engine, &snapshot, &prefs)?;
            scope.check(&engine, &snapshot.segments)?;
            let db = engine.store.lock().unwrap();
            scope.check_store(&db, &snapshot.segments)?;
            match mutation {
                "source_text" => {
                    db.conn.execute(
                        "UPDATE segments SET json=json_set(json,'$.text','缓存将上传。') WHERE id=?1",
                        [&snapshot.segments[0].id],
                    )?;
                }
                "source_seq_column" => {
                    db.conn.execute(
                        "UPDATE segments SET seq=seq+1 WHERE id=?1",
                        [&snapshot.segments[0].id],
                    )?;
                }
                "document" => db.save_suggestion(&crate::Suggestion {
                    focus: String::new(),
                    id: "changed-document".into(),
                    meeting_id: snapshot.meeting.id.clone(),
                    kind: "paragraph".into(),
                    text: "刚刚编辑的纪要内容。".into(),
                    sources: vec![],
                    status: "pending".into(),
                    stale: false,
                })?,
                "cursor" => {
                    db.conn.execute(
                        "UPDATE meetings SET json=json_set(json,'$.notes_cursor',?2) WHERE id=?1",
                        params![snapshot.meeting.id, snapshot.meeting.notes_cursor + 1],
                    )?;
                }
                _ => unreachable!(),
            }
            let revision: u32 = db.conn.query_row(
                "SELECT json_extract(json,'$.revision') FROM segments WHERE id=?1",
                [&snapshot.segments[0].id],
                |row| row.get(0),
            )?;
            assert_eq!(revision, snapshot.segments[0].revision);
            // The same guard can now proceed directly to commit only if the
            // complete identity is still current, not merely id/revision.
            assert!(
                scope.check_store(&db, &snapshot.segments).is_err(),
                "{mutation}"
            );
            assert!(engine.preferences.try_lock().is_ok());
            assert!(engine.config_lock.try_lock().is_ok());
            drop(db);
            assert_unlocked(&engine);
        }
        Ok(())
    }

    #[test]
    fn deletion_mismatched_meeting_identity_and_cursor_changes_are_rejected() -> Result<()> {
        for mutation in ["deleted", "meeting_id", "cursor"] {
            let (_root, engine, snapshot, prefs) = seed()?;
            let scope = scope(&engine, &snapshot, &prefs)?;
            {
                let db = engine.store.lock().unwrap();
                match mutation {
                    "deleted" => {
                        db.conn.execute(
                            "INSERT INTO deleted_meetings VALUES(?1)",
                            [&snapshot.meeting.id],
                        )?;
                    }
                    "meeting_id" => {
                        db.conn.execute(
                            "UPDATE meetings SET json=json_set(json,'$.id','another-meeting') WHERE id=?1",
                            [&snapshot.meeting.id],
                        )?;
                    }
                    "cursor" => {
                        let mut meeting = snapshot.meeting.clone();
                        meeting.notes_cursor += 1;
                        db.save_meeting(&meeting)?;
                    }
                    _ => unreachable!(),
                }
            }
            assert!(
                scope.check(&engine, &snapshot.segments).is_err(),
                "{mutation}"
            );
            assert_unlocked(&engine);
        }
        Ok(())
    }
}
