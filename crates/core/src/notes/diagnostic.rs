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

#[derive(Clone, Copy)]
enum Terminal {
    ValidationRejected,
    IncompleteLocalResponse,
}

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
    terminal: Cell<Option<Terminal>>,
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
            terminal: Cell::new(None),
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

    // A CPU fallback is another operation, not the previous operation's
    // terminal result. Clear before its budget/admission checks can return.
    pub(super) fn begin_operation(&self) {
        self.terminal.set(None);
    }

    pub(super) fn validation_rejected(&self, error: anyhow::Error) -> anyhow::Error {
        self.terminal.set(Some(Terminal::ValidationRejected));
        error
    }

    pub(super) fn incomplete_response(&self, error: anyhow::Error) -> anyhow::Error {
        self.terminal.set(Some(Terminal::IncompleteLocalResponse));
        error
    }

    pub(super) fn observe_job_write(&self, result: &Result<()>) {
        if result.is_err() {
            self.terminal.set(None);
        }
    }

    pub(super) fn finish(&self, engine: &Engine, result: &Result<usize>) {
        let mut value = self.value.borrow_mut();
        let outcome = if self.committed() {
            "committed"
        } else if engine.note_cancel.load(Ordering::SeqCst)
            || engine.maintenance.load(Ordering::SeqCst)
        {
            "cancelled"
        } else if self.commit_started.get() {
            "commit_failed"
        } else if result.is_err() {
            match self.terminal.get() {
                Some(Terminal::ValidationRejected) => "validation_rejected",
                Some(Terminal::IncompleteLocalResponse) => "incomplete_local_response",
                None => "deferred",
            }
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

    fn later_stop_is_not_the_first_draft_validation_rejection(stop: &str) -> Result<()> {
        {
            let root = tempfile::tempdir()?;
            let engine = Engine::open(root.path().into())?;
            let meeting = engine.create_meeting("Public diagnostic terminal control")?;
            let source = crate::Segment {
                id: format!("{}:1:1", meeting.id),
                meeting_id: meeting.id.clone(),
                run: meeting.run,
                seq: 1,
                start_ms: 0,
                end_ms: 1000,
                text: "当迪大学的 PAMELA FURGO森教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性".into(),
                revision: 1,
                speaker: None,
                speaker_manual: false,
            };
            engine.store.lock().unwrap().save_segment(&source)?;
            let snapshot = engine.snapshot(&meeting.id)?;
            let prefs = engine.preferences.lock().unwrap().clone();
            let scope = super::super::generation::Scope::new(&engine, &snapshot, &prefs, 0)?;
            let terms = engine
                .terminology(&meeting.id)?
                .with_actor_spellings([source.text.as_str()], std::iter::empty::<&str>());
            // This is the real production spelling validator, not a made-up
            // quality Result; no local model or transport is started.
            let first_error = terms
                .validate_writer_names(
                    &source.text,
                    "当迪大学的 PAMELA FURGO 森教授指出，公布照片似乎存在危害性。",
                )
                .unwrap_err();
            let trace = TaskTrace::start(root.path(), "minutes", stop);
            let report = Report::new(root.path(), &meeting.id, "local-model", 0, &trace);
            report.record(
                &engine,
                json!({"attempts":[{"attempt":1,"stage":"compose",
                    "validation_error":format!("{first_error:#}")}]}),
            )?;
            let requests = super::super::generation::Budget::default();
            assert_eq!(requests.take()?, 1);
            let terminal = if stop == "scope" {
                engine.save_note(&meeting.id, None, "待核对发布范围。", "document", 0)?;
                scope.check(&engine, std::slice::from_ref(&source))
            } else {
                let work = super::super::Generation {
                    trace: &trace,
                    diagnostic: &report,
                    scope: &scope,
                    requests: &requests,
                    context_revision: 0,
                };
                work.ready(
                    &engine,
                    std::slice::from_ref(&source),
                    true,
                    false,
                    false,
                    4096,
                )
            };
            assert!(terminal.is_err(), "{stop}: production stop was not reached");
            report.finish(&engine, &terminal.map(|()| 0));
            let value = read(root.path())?;
            assert_eq!(
                value["attempts"][0]["validation_error"],
                format!("{first_error:#}")
            );
            assert_eq!(value["committed"], false);
            assert_eq!(value["cursor_after"], 0);
            assert_eq!(
                value["outcome"], "deferred",
                "{stop}: a first-draft error was mistaken for the later terminal stop"
            );
        }
        Ok(())
    }

    #[test]
    fn later_scope_stop_is_not_the_first_draft_validation_rejection() -> Result<()> {
        later_stop_is_not_the_first_draft_validation_rejection("scope")
    }

    #[test]
    fn later_resource_stop_is_not_the_first_draft_validation_rejection() -> Result<()> {
        later_stop_is_not_the_first_draft_validation_rejection("resource")
    }

    fn spelling_rejection() -> anyhow::Error {
        let source = "当迪大学的 PAMELA FURGO森教授指出公布照片似乎存在危害性。";
        crate::terminology::Terms::new(&[], &[])
            .with_actor_spellings([source], std::iter::empty::<&str>())
            .validate_writer_names(
                source,
                "当迪大学的 PAMELA FURGO 森教授指出公布照片存在危害性。",
            )
            .unwrap_err()
    }

    #[test]
    fn fallback_entry_clears_the_previous_terminal_before_budget_check() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let meeting = engine.create_meeting("Diagnostic fallback metadata control")?;
        let trace = TaskTrace::start(root.path(), "minutes", "diagnostic-fallback");
        let report = Report::new(root.path(), &meeting.id, "local-model", 0, &trace);
        let requests = super::super::generation::Budget::default();
        let mut calls = 0;
        let result: Result<usize> = crate::acceleration::with_cpu_fallback(&engine, || {
            report.begin_operation();
            requests.ensure_remaining()?;
            calls += 1;
            assert_eq!(requests.take()?, 1);
            assert_eq!(requests.take()?, 2);
            let error = spelling_rejection();
            report.record(
                &engine,
                json!({"attempts":[{"attempt":2,"stage":"repair",
                    "validation_error":format!("{error:#}")}]}),
            )?;
            // Force the existing fallback branch using metadata only. This is
            // not a claim that a real GPU/model produced this rejection.
            crate::acceleration::reject(&engine, "diagnostic_control", true);
            Err(report.validation_rejected(error))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1, "fallback must stop at its existing budget check");
        report.finish(&engine, &result);
        assert_eq!(read(root.path())?["outcome"], "deferred");
        Ok(())
    }

    #[test]
    fn failed_job_write_replaces_only_terminal_provenance_not_attempt_history() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let meeting = engine.create_meeting("Diagnostic job-write control")?;
        let trace = TaskTrace::start(root.path(), "minutes", "diagnostic-job-write");
        let report = Report::new(root.path(), &meeting.id, "local-model", 0, &trace);
        let error = spelling_rejection();
        let text = format!("{error:#}");
        report.record(
            &engine,
            json!({"attempts":[{"attempt":2,"stage":"repair","validation_error":text}]}),
        )?;
        let rejected: Result<usize> = Err(report.validation_rejected(error));
        assert_eq!(format!("{:#}", rejected.as_ref().unwrap_err()), text);
        engine
            .store
            .lock()
            .unwrap()
            .conn
            .execute("DROP TABLE inference_jobs", [])?;
        let written = engine
            .store
            .lock()
            .unwrap()
            .finish_job("control", &json!({}), 0);
        assert!(written.is_err(), "the real Store write must fail");
        report.observe_job_write(&written);
        let result = written.map(|()| 0);
        report.finish(&engine, &result);
        let value = read(root.path())?;
        assert_eq!(value["outcome"], "deferred");
        assert_eq!(value["attempts"][0]["validation_error"], text);
        assert_eq!(value["cursor_after"], 0);
        Ok(())
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
        let result = Err(report.validation_rejected(anyhow::anyhow!("actor attribution rejected")));
        report.finish(&engine, &result);
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
            let result = match outcome {
                "prepare_rejected" => {
                    Err(report.validation_rejected(anyhow::anyhow!("source preparation rejected")))
                }
                "incomplete" => {
                    let error =
                        super::super::stream::read_observed("".as_bytes(), |_| {}).unwrap_err();
                    Err(report.incomplete_response(error))
                }
                _ => Ok(0),
            };
            report.finish(&engine, &result);
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
        report.finish(&engine, &Ok(0));
        let value = read(root.path())?;
        assert_eq!(value["cursor_before"], 5);
        assert_eq!(value["cursor_after"], Value::Null);
        assert_eq!(value["committed"], false);
        Ok(())
    }
}
