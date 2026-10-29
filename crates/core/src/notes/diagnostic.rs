//! The generation report describes observed state, never a proposed commit.
//! Trace events carry only existing request/stage metadata, not draft text.
use crate::{inference_trace::TaskTrace, Engine};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

pub(super) struct Report<'a> {
    path: PathBuf,
    meeting: String,
    before: u32,
    trace: &'a TaskTrace,
    value: RefCell<Value>,
    failed_attempts: RefCell<HashSet<usize>>,
    commit_started: Cell<bool>,
    committed: Cell<bool>,
    sections: Cell<Option<usize>>,
}
impl<'a> Report<'a> {
    pub(super) fn new(
        root: &Path,
        meeting: &str,
        model: &str,
        before: u32,
        trace: &'a TaskTrace,
    ) -> Self {
        Self {
            path: root.join("last-generation.json"),
            meeting: meeting.into(),
            before,
            trace,
            value: RefCell::new(json!({"meeting_id":meeting,"model":model,"attempts":[]})),
            failed_attempts: RefCell::new(HashSet::new()),
            commit_started: Cell::new(false),
            committed: Cell::new(false),
            sections: Cell::new(None),
        }
    }

    pub(super) fn record(&self, engine: &Engine, mut value: Value) -> Result<()> {
        if let Some(attempts) = value["attempts"].as_array() {
            for attempt in attempts {
                if attempt["validation_error"].is_string() {
                    if let Some(number) = attempt["attempt"].as_u64() {
                        if let Ok(number) = usize::try_from(number) {
                            if self.failed_attempts.borrow_mut().insert(number) {
                                self.trace.event(
                                    "validation_failed",
                                    json!({"attempt":number,"stage":attempt["stage"],"reason":"validation"}),
                                );
                            }
                        }
                    }
                }
            }
            let latest = attempts.last().cloned();
            value["validation_error"] = latest
                .as_ref()
                .and_then(|attempt| attempt.get("validation_error"))
                .cloned()
                .unwrap_or(Value::Null);
            if value.get("stage").is_none() {
                value["stage"] = latest
                    .as_ref()
                    .and_then(|attempt| attempt.get("stage"))
                    .cloned()
                    .unwrap_or(Value::Null);
            }
        } else {
            value["validation_error"] = Value::Null;
        }
        *self.value.borrow_mut() = value;
        self.write_observed(engine)
    }

    // This marker records the transaction's result, not validation success.
    // It performs no I/O and can be called while the database guard is held.
    pub(super) fn commit_started(&self) {
        self.commit_started.set(true);
    }
    pub(super) fn commit_result(&self, result: &Result<usize>) {
        if let Ok(count) = result {
            self.committed.set(true);
            self.sections.set(Some(*count));
        }
    }
    pub(super) fn committed(&self) -> bool {
        self.committed.get()
    }
    pub(super) fn sections(&self) -> Option<usize> {
        self.sections.get()
    }

    pub(super) fn finish(&self, engine: &Engine) {
        let mut value = self.value.borrow_mut();
        let outcome = if self.committed() {
            "committed"
        } else if engine.note_cancel.load(Ordering::SeqCst)
            || engine.maintenance.load(Ordering::SeqCst)
        {
            "cancelled"
        } else if self.commit_started.get() {
            "commit_failed"
        } else if value["validation_error"].is_string() {
            "validation_rejected"
        } else if value["attempts"]
            .as_array()
            .and_then(|attempts| attempts.last())
            .is_some_and(|attempt| attempt["failure"] == "incomplete_local_response")
        {
            "incomplete_local_response"
        } else {
            "deferred"
        };
        value["outcome"] = outcome.into();
        if ["cancelled", "commit_failed", "committed"].contains(&outcome) {
            value["stage"] = outcome.into();
        }
        drop(value);
        // A diagnostic write after the transaction cannot undo that transaction
        // or turn an otherwise successful generation into an application error.
        let _ = self.write_observed(engine);
    }

    fn write_observed(&self, engine: &Engine) -> Result<()> {
        let actual = engine
            .store
            .lock()
            .unwrap()
            .meeting(&self.meeting)
            .ok()
            .map(|meeting| meeting.notes_cursor);
        let mut value = self.value.borrow_mut();
        value["cursor_before"] = self.before.into();
        value["cursor_after"] = actual.into();
        value["committed"] = self.committed().into();
        if value.get("validation_error").is_none() {
            value["validation_error"] = Value::Null;
        }
        std::fs::write(&self.path, serde_json::to_vec_pretty(&*value)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(root: &Path) -> Result<Value> {
        Ok(serde_json::from_slice(&std::fs::read(
            root.join("last-generation.json"),
        )?)?)
    }

    #[test]
    fn final_quota_exhausted_validation_failure_is_reported_once_without_advancing_cursor(
    ) -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let mut meeting = engine.create_meeting("Fictional exhausted validation")?;
        meeting.notes_cursor = 5;
        engine.store.lock().unwrap().save_meeting(&meeting)?;
        let trace = TaskTrace::start(root.path(), "minutes", "diagnostic-final-validation");
        let report = Report::new(root.path(), &meeting.id, "local-model", 5, &trace);
        let requests = super::super::generation::Budget::default();
        assert_eq!(requests.take()?, 1);
        report.record(
            &engine,
            json!({
                "meeting_id":meeting.id, "stage":"source_preparation", "cursor_after":6,
                "attempts":[{"attempt":1,"stage":"prepare_sources","validation_error":null}]
            }),
        )?;
        assert_eq!(requests.take()?, 2);
        let rejected = json!({
            "meeting_id":meeting.id, "draft":"fictional private draft", "cursor_after":6,
            "attempts":[
                {"attempt":1,"stage":"prepare_sources","validation_error":null},
                {"attempt":2,"stage":"compose_prepared_sources","validation_error":"actor attribution rejected"}
            ]
        });
        report.record(&engine, rejected.clone())?;
        assert!(requests.ensure_remaining().is_err());
        report.record(&engine, rejected)?;
        report.finish(&engine);
        let value = read(root.path())?;
        assert_eq!(value["committed"], false);
        assert_eq!(value["cursor_before"], 5);
        assert_eq!(value["cursor_after"], 5);
        assert_eq!(value["validation_error"], "actor attribution rejected");
        assert_eq!(
            value["validation_error"],
            value["attempts"][1]["validation_error"]
        );
        assert_eq!(value["stage"], "compose_prepared_sources");
        assert_eq!(value["outcome"], "validation_rejected");
        let events = std::fs::read_to_string(root.path().join("inference-trace.jsonl"))?;
        let rows: Vec<Value> = events
            .lines()
            .map(serde_json::from_str)
            .collect::<std::result::Result<_, _>>()?;
        let failures: Vec<_> = rows
            .iter()
            .filter(|row| row["phase"] == "validation_failed")
            .collect();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0]["metrics"]["attempt"], 2);
        assert_eq!(failures[0]["metrics"]["stage"], "compose_prepared_sources");
        assert!(!events.contains("fictional private draft"));
        assert!(!events.contains("actor attribution rejected"));
        Ok(())
    }

    #[test]
    fn preparation_rejection_incomplete_response_and_cancellation_have_observed_state() -> Result<()>
    {
        for outcome in [
            "prepare_rejected",
            "incomplete",
            "cancelled",
            "validated_only",
        ] {
            let root = tempfile::tempdir()?;
            let engine = Engine::open(root.path().into())?;
            let mut meeting = engine.create_meeting("Fictional diagnostic stage")?;
            meeting.notes_cursor = 5;
            engine.store.lock().unwrap().save_meeting(&meeting)?;
            let trace = TaskTrace::start(root.path(), "minutes", outcome);
            let report = Report::new(root.path(), &meeting.id, "local-model", 5, &trace);
            let mut attempt =
                json!({"attempt":1,"stage":"prepare_sources","validation_error":null});
            if outcome == "prepare_rejected" {
                attempt["validation_error"] = "source preparation rejected".into();
            } else if outcome == "incomplete" {
                attempt["stage"] = "compose".into();
                attempt["failure"] = "incomplete_local_response".into();
                attempt.as_object_mut().unwrap().remove("validation_error");
            } else if outcome == "cancelled" {
                engine.note_cancel.store(true, Ordering::SeqCst);
            }
            report.record(
                &engine,
                json!({"meeting_id":meeting.id,"cursor_after":6,"attempts":[attempt]}),
            )?;
            report.finish(&engine);
            let value = read(root.path())?;
            assert_eq!(value["committed"], false, "{outcome}");
            assert_eq!(value["cursor_after"], 5, "{outcome}");
            assert_eq!(
                value["validation_error"].is_string(),
                outcome == "prepare_rejected"
            );
            let expected = match outcome {
                "prepare_rejected" => "validation_rejected",
                "incomplete" => "incomplete_local_response",
                "cancelled" => "cancelled",
                _ => "deferred",
            };
            assert_eq!(value["outcome"], expected);
            let events = std::fs::read_to_string(root.path().join("inference-trace.jsonl"))?;
            let failures = events
                .lines()
                .map(serde_json::from_str::<Value>)
                .collect::<std::result::Result<Vec<_>, _>>()?
                .into_iter()
                .filter(|row| row["phase"] == "validation_failed")
                .count();
            assert_eq!(failures, usize::from(outcome == "prepare_rejected"));
        }
        Ok(())
    }

    #[test]
    fn actual_database_cursor_and_unknown_meeting_are_never_replaced_by_a_target() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let mut meeting = engine.create_meeting("Fictional observed cursor")?;
        let trace = TaskTrace::start(root.path(), "minutes", "observed-cursor");
        let report = Report::new(root.path(), &meeting.id, "local-model", 5, &trace);
        meeting.notes_cursor = 7;
        engine.store.lock().unwrap().save_meeting(&meeting)?;
        report.record(&engine, json!({"cursor_after":6}))?;
        assert_eq!(read(root.path())?["cursor_after"], 7);
        assert!(!report.committed());
        engine
            .store
            .lock()
            .unwrap()
            .conn
            .execute("DELETE FROM meetings WHERE id=?1", [&meeting.id])?;
        report.finish(&engine);
        let value = read(root.path())?;
        assert_eq!(value["cursor_before"], 5);
        assert_eq!(value["cursor_after"], Value::Null);
        assert_eq!(value["committed"], false);
        Ok(())
    }
}
