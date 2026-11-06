//! Skip materializing unchanged, verified idle meetings on the two-second tick.
//! Only the no-work decision is retained: no transcript or model state is cached.

use super::*;
use std::collections::HashMap;

#[derive(PartialEq)]
struct ContextKey {
    preferences: crate::preferences::Preferences,
    revision: u64,
    selected: Option<String>,
    runtime_target: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
struct DatabaseRevision {
    local: u64,
    external: i64,
}

impl DatabaseRevision {
    fn read(db: &Store) -> Option<Self> {
        // An uncommitted read is not a stable no-work decision. A failed query
        // also falls back to the full path rather than reusing an old stamp.
        if !db.conn.is_autocommit() {
            return None;
        }
        let external = db
            .conn
            .pragma_query_value(None, "data_version", |row| row.get(0))
            .ok()?;
        Some(Self {
            local: db.conn.total_changes(),
            external,
        })
    }
}

#[derive(Default)]
pub(super) struct IdleMeetings {
    context: Option<ContextKey>,
    meetings: HashMap<String, DatabaseRevision>,
}

impl IdleMeetings {
    pub(super) fn retain(&mut self, ids: &[String]) {
        self.meetings.retain(|id, _| ids.contains(id));
    }

    fn observe_context(&mut self, context: ContextKey) {
        if self.context.as_ref() != Some(&context) {
            self.meetings.clear();
            self.context = Some(context);
        }
    }
}

fn runtime_idle(runtime: &RuntimeStatus) -> bool {
    !runtime.recording
        && !runtime.paused
        && !runtime.transcribing
        && !runtime.diarization_busy
        && !runtime.notes_busy
        && !runtime.translation_busy
}

impl Engine {
    /// Called only after scheduler completion handling. None means a previously
    /// verified no-work decision still holds; it never means work is throttled.
    pub(super) fn scheduler_snapshot(
        &self,
        id: &str,
        idle: &mut IdleMeetings,
    ) -> Result<Option<Snapshot>> {
        let prefs = self.preferences.lock().unwrap().clone();
        let db = self.store.lock().unwrap();
        let runtime = self.status.lock().unwrap().clone();
        let selected = self.notes_target.lock().unwrap().clone();
        let translation_target =
            runtime.meeting_id.as_deref() == Some(id) || selected.as_deref() == Some(id);
        idle.observe_context(ContextKey {
            preferences: prefs.clone(),
            revision: self.context_revision.load(Ordering::SeqCst),
            selected,
            runtime_target: runtime.meeting_id.clone(),
        });
        let eligible = runtime_idle(&runtime)
            && !self.note_busy.load(Ordering::SeqCst)
            && !self.maintenance.load(Ordering::SeqCst)
            && self.diarization_pid.load(Ordering::SeqCst) == 0
            // Translation receipts also live outside SQLite. Always validate
            // them for targets which can be translated on this tick.
            && (prefs.translation_mode == "off" || !translation_target);
        // Read BEFORE any snapshot queries, on the same connection and while
        // holding Store. External commits during the read invalidate next tick;
        // an after-only stamp could incorrectly bless an older snapshot.
        let revision = eligible.then(|| DatabaseRevision::read(&db)).flatten();
        if revision.is_some() && idle.meetings.get(id).copied() == revision {
            return Ok(None);
        }
        idle.meetings.remove(id);
        let (meeting, runtime) = self.snapshot_state(&db, id)?;
        let snapshot = self.snapshot_body(&db, id, prefs, meeting, runtime)?;
        #[cfg(test)]
        AFTER_SNAPSHOT.with(|hook| -> Result<()> {
            if let Some(write) = hook.borrow_mut().take() {
                write()?;
            }
            Ok(())
        })?;
        let pending_speakers = snapshot.meeting.diarization
            && snapshot
                .segments
                .last()
                .is_some_and(|segment| segment.seq > snapshot.meeting.diarization_cursor);
        if let Some(revision) = revision {
            if runtime_idle(&snapshot.runtime)
                && idle
                    .context
                    .as_ref()
                    .is_some_and(|context| context.runtime_target == snapshot.runtime.meeting_id)
                && !notes::needs_update(&snapshot)
                && !pending_speakers
            {
                idle.meetings.insert(id.into(), revision);
            }
        }
        Ok(Some(snapshot))
    }
}

#[cfg(test)]
type SnapshotHook = Box<dyn FnOnce() -> Result<()>>;

#[cfg(test)]
std::thread_local! {
    static SEGMENT_MATERIALIZATION: std::cell::Cell<(usize, usize)> = const {
        std::cell::Cell::new((0, 0))
    };
    // Deterministically commit between reads and decision insertion. No sleeps,
    // background threads, model loads, or production callback are involved.
    static AFTER_SNAPSHOT: std::cell::RefCell<Option<SnapshotHook>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
pub(super) fn record_segment_materialization(rows: usize) {
    SEGMENT_MATERIALIZATION.with(|counter| {
        let (reads, old_rows) = counter.get();
        counter.set((reads + 1, old_rows + rows));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts() -> (usize, usize) {
        SEGMENT_MATERIALIZATION.with(|counter| counter.replace((0, 0)))
    }

    fn synced_meeting(engine: &Engine, rows: u32) -> Result<Meeting> {
        let meeting = engine.create_meeting("Scheduler source fixture")?;
        {
            let db = engine.store.lock().unwrap();
            db.conn.execute_batch("BEGIN")?;
            for seq in 1..=rows {
                db.save_segment(&Segment {
                    id: format!("{}:{}:{seq}", meeting.id, meeting.run),
                    meeting_id: meeting.id.clone(),
                    run: meeting.run,
                    seq,
                    start_ms: u64::from(seq - 1) * 1000,
                    end_ms: u64::from(seq) * 1000,
                    text: "The review remains pending.".into(),
                    revision: 1,
                    speaker: None,
                    speaker_manual: false,
                })?;
            }
            db.conn.execute_batch("COMMIT")?;
        }
        synchronize(engine, &meeting.id)
    }

    fn synchronize(engine: &Engine, id: &str) -> Result<Meeting> {
        let snapshot = engine.snapshot(id)?;
        let mut meeting = snapshot.meeting;
        meeting.notes_cursor = snapshot.segments.last().map_or(0, |s| s.seq);
        meeting.notes_fingerprint =
            notes::fingerprint(&snapshot.notes, &snapshot.segments, meeting.notes_cursor);
        engine.store.lock().unwrap().save_meeting(&meeting)?;
        Ok(meeting)
    }

    fn verify_refresh_then_idle(engine: &Engine, id: &str, idle: &mut IdleMeetings) -> Result<()> {
        assert!(engine.scheduler_snapshot(id, idle)?.is_some());
        assert!(engine.scheduler_snapshot(id, idle)?.is_none());
        Ok(())
    }

    #[test]
    fn unchanged_selected_long_meeting_stops_materializing_history() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().to_path_buf())?;
        let meeting = synced_meeting(&engine, 5_000)?;
        *engine.notes_target.lock().unwrap() = Some(meeting.id.clone());
        engine.status.lock().unwrap().meeting_id = Some(meeting.id.clone());
        let mut idle = IdleMeetings::default();
        counts();
        for _ in 0..10 {
            assert_eq!(engine.snapshot(&meeting.id)?.segments.len(), 5_000);
        }
        let baseline = counts();
        assert_eq!(baseline, (10, 50_000));
        assert!(engine.scheduler_snapshot(&meeting.id, &mut idle)?.is_some());
        for _ in 0..9 {
            assert!(engine.scheduler_snapshot(&meeting.id, &mut idle)?.is_none());
        }
        let optimized = counts();
        assert_eq!(optimized, (1, 5_000));
        assert_eq!(idle.meetings.len(), 1);
        idle.retain(&[]);
        assert!(idle.meetings.is_empty());
        verify_refresh_then_idle(&engine, &meeting.id, &mut idle)?;
        assert_eq!(counts(), (1, 5_000));
        println!("scheduler: 5000 sources, 10 checks; uncached reads/rows={baseline:?}, idle cache reads/rows={optimized:?}");
        Ok(())
    }

    #[test]
    fn idle_decision_tracks_database_preferences_context_and_targets() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().to_path_buf())?;
        let mut meeting = synced_meeting(&engine, 3)?;
        let mut idle = IdleMeetings::default();
        verify_refresh_then_idle(&engine, &meeting.id, &mut idle)?;
        meeting.title = "Changed through scheduler connection".into();
        engine.store.lock().unwrap().save_meeting(&meeting)?;
        verify_refresh_then_idle(&engine, &meeting.id, &mut idle)?;
        let external = Store {
            conn: rusqlite::Connection::open(root.path().join("meetings.sqlite3"))?,
        };
        meeting.title = "Changed through another connection".into();
        external.save_meeting(&meeting)?;
        verify_refresh_then_idle(&engine, &meeting.id, &mut idle)?;
        engine.context_revision.fetch_add(1, Ordering::SeqCst);
        verify_refresh_then_idle(&engine, &meeting.id, &mut idle)?;
        engine.preferences.lock().unwrap().minutes_language = "zh".into();
        verify_refresh_then_idle(&engine, &meeting.id, &mut idle)?;
        *engine.notes_target.lock().unwrap() = Some(meeting.id.clone());
        verify_refresh_then_idle(&engine, &meeting.id, &mut idle)?;
        engine.status.lock().unwrap().meeting_id = Some(meeting.id.clone());
        verify_refresh_then_idle(&engine, &meeting.id, &mut idle)?;

        engine.preferences.lock().unwrap().translation_mode = "bilingual".into();
        counts();
        for _ in 0..2 {
            assert!(engine.scheduler_snapshot(&meeting.id, &mut idle)?.is_some());
        }
        assert_eq!(counts(), (2, 6));
        assert!(idle.meetings.is_empty());
        // Translation-on background meetings can be idle until selected. A
        // target switch must restore external-receipt validation immediately.
        *engine.notes_target.lock().unwrap() = None;
        engine.status.lock().unwrap().meeting_id = None;
        verify_refresh_then_idle(&engine, &meeting.id, &mut idle)?;
        *engine.notes_target.lock().unwrap() = Some(meeting.id.clone());
        assert!(engine.scheduler_snapshot(&meeting.id, &mut idle)?.is_some());
        assert!(engine.scheduler_snapshot(&meeting.id, &mut idle)?.is_some());
        Ok(())
    }

    #[test]
    fn external_commit_after_reads_does_not_bless_an_old_idle_snapshot() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().to_path_buf())?;
        let mut meeting = synced_meeting(&engine, 3)?;
        let external = Store {
            conn: rusqlite::Connection::open(root.path().join("meetings.sqlite3"))?,
        };
        let id = meeting.id.clone();
        AFTER_SNAPSHOT.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                meeting.notes_cursor = 0;
                external.save_meeting(&meeting)
            }));
        });
        let mut idle = IdleMeetings::default();
        counts();
        let before = engine.scheduler_snapshot(&id, &mut idle)?.unwrap();
        assert!(!notes::needs_update(&before));
        let after = engine.scheduler_snapshot(&id, &mut idle)?.unwrap();
        assert!(notes::needs_update(&after));
        assert!(idle.meetings.is_empty());
        assert_eq!(counts(), (2, 6));
        Ok(())
    }

    #[test]
    fn saving_a_note_and_editing_source_wake_a_previously_idle_meeting() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().to_path_buf())?;
        let meeting = synced_meeting(&engine, 3)?;
        let mut idle = IdleMeetings::default();
        // Keep selection constant so this regression relies on content
        // invalidation, not the selected-target change in save_note.
        *engine.notes_target.lock().unwrap() = Some(meeting.id.clone());
        verify_refresh_then_idle(&engine, &meeting.id, &mut idle)?;
        let note = engine.save_note(
            &meeting.id,
            None,
            "Review the deployment schedule.",
            "document",
            0,
        )?;
        for _ in 0..2 {
            let snapshot = engine.scheduler_snapshot(&meeting.id, &mut idle)?.unwrap();
            assert!(notes::needs_update(&snapshot));
            assert_eq!(snapshot.notes[0].content, note.content);
            assert!(idle.meetings.is_empty());
        }
        synchronize(&engine, &meeting.id)?;
        verify_refresh_then_idle(&engine, &meeting.id, &mut idle)?;
        engine.edit_segment(
            &format!("{}:{}:1", meeting.id, meeting.run),
            "The deployment review is now complete.",
            1,
        )?;
        for _ in 0..2 {
            let snapshot = engine.scheduler_snapshot(&meeting.id, &mut idle)?.unwrap();
            assert!(notes::needs_update(&snapshot));
            assert_eq!(snapshot.segments[0].revision, 2);
            assert_eq!(
                snapshot.segments[0].text,
                "The deployment review is now complete."
            );
            assert!(idle.meetings.is_empty());
        }
        Ok(())
    }

    #[test]
    fn active_work_and_pending_minutes_or_speakers_never_use_idle_decisions() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().to_path_buf())?;
        let mut meeting = synced_meeting(&engine, 3)?;
        let mut idle = IdleMeetings::default();
        for active in [
            RuntimeStatus {
                recording: true,
                ..Default::default()
            },
            RuntimeStatus {
                paused: true,
                ..Default::default()
            },
            RuntimeStatus {
                transcribing: true,
                ..Default::default()
            },
            RuntimeStatus {
                diarization_busy: true,
                ..Default::default()
            },
            RuntimeStatus {
                notes_busy: true,
                ..Default::default()
            },
            RuntimeStatus {
                translation_busy: true,
                ..Default::default()
            },
        ] {
            verify_refresh_then_idle(&engine, &meeting.id, &mut idle)?;
            *engine.status.lock().unwrap() = active;
            assert!(engine.scheduler_snapshot(&meeting.id, &mut idle)?.is_some());
            assert!(idle.meetings.is_empty());
            *engine.status.lock().unwrap() = RuntimeStatus::default();
        }
        verify_refresh_then_idle(&engine, &meeting.id, &mut idle)?;
        engine.note_busy.store(true, Ordering::SeqCst);
        assert!(engine.scheduler_snapshot(&meeting.id, &mut idle)?.is_some());
        engine.note_busy.store(false, Ordering::SeqCst);
        verify_refresh_then_idle(&engine, &meeting.id, &mut idle)?;
        engine.diarization_pid.store(42, Ordering::SeqCst);
        assert!(engine.scheduler_snapshot(&meeting.id, &mut idle)?.is_some());
        engine.diarization_pid.store(0, Ordering::SeqCst);
        meeting.notes_cursor = 0;
        meeting.auto_notes = false;
        engine.store.lock().unwrap().save_meeting(&meeting)?;
        for _ in 0..2 {
            let snapshot = engine.scheduler_snapshot(&meeting.id, &mut idle)?.unwrap();
            assert!(notes::needs_update(&snapshot));
            assert!(idle.meetings.is_empty());
        }
        meeting = synchronize(&engine, &meeting.id)?;
        meeting.diarization = true;
        meeting.diarization_cursor = 0;
        engine.store.lock().unwrap().save_meeting(&meeting)?;
        engine
            .diarization_cancelled
            .lock()
            .unwrap()
            .insert(meeting.id.clone());
        for _ in 0..2 {
            assert!(engine.scheduler_snapshot(&meeting.id, &mut idle)?.is_some());
            assert!(idle.meetings.is_empty());
        }
        Ok(())
    }

    #[test]
    fn transaction_or_failed_snapshot_cannot_retain_an_idle_decision() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().to_path_buf())?;
        let meeting = synced_meeting(&engine, 3)?;
        let mut idle = IdleMeetings::default();
        verify_refresh_then_idle(&engine, &meeting.id, &mut idle)?;
        engine.store.lock().unwrap().conn.execute_batch("BEGIN")?;
        for _ in 0..2 {
            assert!(engine.scheduler_snapshot(&meeting.id, &mut idle)?.is_some());
            assert!(idle.meetings.is_empty());
        }
        engine
            .store
            .lock()
            .unwrap()
            .conn
            .execute_batch("ROLLBACK")?;
        verify_refresh_then_idle(&engine, &meeting.id, &mut idle)?;
        // A malformed row after a real local write must fail closed; successful
        // decisions never outlive an unsuccessful snapshot read.
        engine.store.lock().unwrap().conn.execute(
            "UPDATE segments SET json='{}' WHERE meeting_id=?1",
            [&meeting.id],
        )?;
        assert!(engine.scheduler_snapshot(&meeting.id, &mut idle).is_err());
        assert!(idle.meetings.is_empty());
        Ok(())
    }
}
