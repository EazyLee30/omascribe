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
    observe_rejection: Cell<bool>,
    ordinary_job: Cell<bool>,
    witness: RefCell<Option<crate::inference_worker::rejection::Witness>>,
    completed_responses: Cell<usize>,
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
            observe_rejection: Cell::new(false),
            ordinary_job: Cell::new(true),
            witness: RefCell::new(None),
            completed_responses: Cell::new(0),
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
        self.witness.borrow_mut().take();
        self.completed_responses.set(0);
    }

    pub(super) fn observe_automatic(&self, ordinary: bool) {
        self.observe_rejection.set(true);
        self.ordinary_job.set(ordinary);
    }
    pub(super) fn completed_response(&self) {
        self.completed_responses
            .set(self.completed_responses.get().saturating_add(1));
    }

    pub(super) fn observe_execution(
        &self,
        engine: &Engine,
        prefs: &crate::preferences::Preferences,
        session: &crate::inference_worker::SessionLease,
    ) {
        if self.observe_rejection.get() && self.ordinary_job.get() {
            *self.witness.borrow_mut() = session.rejection_witness(engine, prefs);
        }
    }

    pub(super) fn rejected_execution(
        &self,
        engine: &Engine,
        result: &Result<usize>,
    ) -> Option<crate::inference_worker::rejection::Witness> {
        if result.is_ok()
            || self.committed()
            || self.commit_started.get()
            || self.completed_responses.get() != 2
            || !self.ordinary_job.get()
            || !matches!(self.terminal.get(), Some(Terminal::ValidationRejected))
            || engine.note_cancel.load(Ordering::SeqCst)
            || engine.maintenance.load(Ordering::SeqCst)
        {
            return None;
        }
        let prefs = engine.preferences.lock().unwrap().clone();
        self.witness
            .borrow()
            .as_ref()
            .filter(|w| w.current(engine, &prefs))
            .cloned()
    }

    #[cfg(test)]
    pub(crate) fn observe_execution_control(
        &self,
        witness: crate::inference_worker::rejection::Witness,
    ) {
        *self.witness.borrow_mut() = Some(witness);
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

    fn clean_observation_child(name: &str) -> Result<bool> {
        const CHILD: &str = "OMASCRIBE_REJECTION_OBSERVATION_CHILD";
        if std::env::var(CHILD).as_deref() == Ok(name) {
            return Ok(false);
        }
        let mut command = std::process::Command::new(std::env::current_exe()?);
        command
            .args(["--exact", name, "--nocapture"])
            .env(CHILD, name);
        // Cargo injects a DYLD lookup; only the isolated control child is
        // cleaned. Product override policy has no Cargo exception.
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy();
            if name.starts_with("DYLD_")
                || name.starts_with("GGML_")
                || name.starts_with("LLAMA_")
                || name.starts_with("MTL_")
                || name.starts_with("METAL_")
                || name.starts_with("LD_")
                || [
                    "OMASCRIBE_PREPARE_SOURCES",
                    "OMASCRIBE_INCREMENTAL_MINUTES",
                    "OMASCRIBE_GROUPED_MINUTES",
                ]
                .contains(&name.as_ref())
            {
                command.env_remove(key);
            }
        }
        anyhow::ensure!(
            command.status()?.success(),
            "isolated observation control failed"
        );
        Ok(true)
    }

    struct RejectionControl {
        _root: tempfile::TempDir,
        engine: std::sync::Arc<Engine>,
        snapshot: crate::Snapshot,
        input: crate::minutes_schedule::InputDigest,
        observation: crate::notes::Observation,
        dispatch: crate::minutes_schedule::Dispatch,
        schedule: crate::minutes_schedule::Schedule,
        at: std::time::Instant,
    }

    fn mutate_observed_control(
        control: &RejectionControl,
        path: &Path,
        bytes: &[u8],
        model: bool,
    ) -> Result<()> {
        fn read_optional(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
            match std::fs::read(path) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error),
            }
        }
        let original = read_optional(path)?;
        match std::fs::write(path, bytes) {
            Ok(()) => {}
            Err(error) if cfg!(windows) && error.raw_os_error() == Some(32) => {
                // A retained Windows receipt protects the original file. The
                // witness keeps digests only and must not retain that protection
                // after the actual cache-revocation lifecycle releases it.
                assert_eq!(
                    read_optional(path)?,
                    original,
                    "denied write changed observed bytes"
                );
                assert!(
                    !control
                        .engine
                        .automatic_minutes_admission(&control.snapshot, &control.input),
                    "denied write lost the unchanged execution observation"
                );
                let directory = path.parent().context("observed mutation has no parent")?;
                if model {
                    control.engine.model_integrity.invalidate(directory);
                    if original.is_some() {
                        std::fs::remove_file(path)?;
                    }
                    std::fs::write(path, bytes)?;
                } else {
                    let blocked = control
                        .engine
                        .inference_fingerprints
                        .block_directory(directory)?;
                    if original.is_some() {
                        std::fs::remove_file(path)?;
                    }
                    std::fs::write(path, bytes)?;
                    drop(blocked);
                }
            }
            Err(error) => return Err(error.into()),
        }
        assert_eq!(
            std::fs::read(path)?,
            bytes,
            "observed mutation was not applied"
        );
        Ok(())
    }

    fn rejection_control() -> Result<RejectionControl> {
        use crate::minutes_schedule::{Dispatch, InputDigest, Schedule};
        use std::{sync::Arc, time::Instant};
        let root = tempfile::tempdir()?;
        let mut engine = Engine::open(root.path().into())?;
        Arc::get_mut(&mut engine).unwrap().runtime_dir = root.path().join("runtime");
        Arc::get_mut(&mut engine).unwrap().models = root.path().join("no-bundled-models");
        engine.preferences.lock().unwrap().inference_backend = "cpu".into();
        let prefs = engine.preferences.lock().unwrap().clone();
        let meeting = engine.create_meeting("Observed automatic rejection control")?;
        engine.store.lock().unwrap().save_segment(&crate::Segment {
            id: format!("{}:1:1", meeting.id), meeting_id: meeting.id.clone(), run: meeting.run, seq: 1,
            start_ms: 0, end_ms: 1000,
            text: "当迪大学的 PAMELA FURGO森教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性".into(),
            revision: 1, speaker: None, speaker_manual: false,
        })?;
        let snapshot = engine.snapshot(&meeting.id)?;
        let scope = super::super::generation::Scope::new(&engine, &snapshot, &prefs, 0)?;
        let mut actual = None;
        super::super::record_admitted_input(&scope, &snapshot, &prefs, 0, Some(&mut actual));
        assert_eq!(
            actual,
            Some(InputDigest::from_snapshot(&snapshot, &prefs, 0)?)
        );
        let actual = actual.unwrap();
        let witness = crate::inference_worker::rejection::observation_control(&engine, &prefs)?;
        assert!(witness.current(&engine, &prefs));
        let dispatch = Dispatch::new(meeting.run, Some(actual.clone()));
        engine.note_busy.store(true, Ordering::SeqCst);
        engine.register_minutes_dispatch(&meeting.id, &dispatch)?;
        let at = Instant::now();
        let mut schedule = Schedule::default();
        schedule.observe_with_input(&meeting, false, Some(&actual));
        schedule.started_for_dispatch(&meeting, &dispatch, at);
        let trace = TaskTrace::start(root.path(), "minutes", "rejection-admission-control");
        let report = Report::new(root.path(), &meeting.id, &prefs.llm_model, 0, &trace);
        report.observe_execution_control(witness);
        // Account two completed response metadata events. No request/model is
        // launched; the rejected bytes below go through the actual validator.
        let requests = super::super::generation::Budget::default();
        for attempt in 1..=2 {
            assert_eq!(requests.take()?, attempt);
            report.completed_response();
        }
        let error = spelling_rejection();
        report.record(&engine, json!({"attempts":[{"attempt":2,"stage":"repair","validation_error":format!("{error:#}")}]}))?;
        let result = Err(report.validation_rejected(error));
        report.finish(&engine, &result);
        let observation = crate::notes::Observation {
            input: Some(actual.clone()),
            rejected: report.rejected_execution(&engine, &result),
            suppressed: false,
        };
        assert!(observation.rejected.is_some());
        engine.finish_minutes_observation(
            &meeting.id,
            meeting.run,
            &dispatch,
            &observation,
            result,
        );
        dispatch.complete_observed(false, Some(actual.clone()));
        let done = dispatch.take_completion().unwrap();
        schedule.finished_for_input(&meeting, Some(&actual), Some(&done), at);
        engine.note_busy.store(false, Ordering::SeqCst);
        Ok(RejectionControl {
            _root: root,
            engine,
            snapshot,
            input: actual,
            observation,
            dispatch,
            schedule,
            at,
        })
    }

    #[test]
    fn unchanged_semantic_rejection_cannot_reenter_automatic_after_backoff() -> Result<()> {
        if clean_observation_child("notes::diagnostic::tests::unchanged_semantic_rejection_cannot_reenter_automatic_after_backoff")? { return Ok(()); }
        let f = rejection_control()?;
        assert!(f.schedule.due(
            &f.snapshot.meeting.id,
            false,
            false,
            f.at + std::time::Duration::from_secs(361)
        ));
        let bytes = f.engine.inference_fingerprints.hashed_bytes();
        for _ in 0..3 {
            assert!(!f.engine.automatic_minutes_admission(&f.snapshot, &f.input), "unchanged complete semantic rejection re-entered automatic generation after backoff");
        }
        assert_eq!(bytes, f.engine.inference_fingerprints.hashed_bytes());
        f.engine.focus_meeting(&f.snapshot.meeting.id)?;
        {
            let mut status = f.engine.status.lock().unwrap();
            status.meeting_id = Some("unrelated-active-meeting".into());
            status.notes_status = "other meeting status".into();
        }
        assert!(!f.engine.automatic_minutes_admission(&f.snapshot, &f.input));
        assert_eq!(
            f.engine.status.lock().unwrap().notes_status,
            "other meeting status"
        );
        assert_eq!(
            f.engine
                .snapshot(&f.snapshot.meeting.id)?
                .meeting
                .notes_cursor,
            0
        );
        assert!(f
            .engine
            .snapshot(&f.snapshot.meeting.id)?
            .suggestions
            .is_empty());
        assert_eq!(f.engine.inference_worker_status().starts, 0);
        Ok(())
    }

    #[test]
    fn automatic_pause_preserves_dispatch_ownership_and_manual_retry() -> Result<()> {
        if clean_observation_child("notes::diagnostic::tests::automatic_pause_preserves_dispatch_ownership_and_manual_retry")? { return Ok(()); }
        use crate::minutes_schedule::Dispatch;
        let mut f = rejection_control()?;
        let meeting = &f.snapshot.meeting;
        let replacement = Dispatch::new(meeting.run, Some(f.input.clone()));
        f.engine
            .register_minutes_dispatch(&meeting.id, &replacement)?;
        f.engine.finish_minutes_observation(
            &meeting.id,
            meeting.run,
            &f.dispatch,
            &f.observation,
            Err(spelling_rejection()),
        );
        assert!(!f.engine.automatic_minutes_admission(&f.snapshot, &f.input));
        let before = f
            .engine
            .legacy_minutes_failures
            .lock()
            .unwrap()
            .count(meeting);
        let suppressed = crate::notes::Observation {
            input: Some(f.input.clone()),
            rejected: None,
            suppressed: true,
        };
        f.schedule.started_for_dispatch(meeting, &replacement, f.at);
        f.engine.finish_minutes_observation(
            &meeting.id,
            meeting.run,
            &replacement,
            &suppressed,
            Err(anyhow::anyhow!(crate::minutes_schedule::AUTOMATIC_PAUSED)),
        );
        replacement.complete_suppressed(Some(f.input.clone()));
        let done = replacement.take_completion().unwrap();
        f.schedule
            .finished_for_input(meeting, Some(&f.input), Some(&done), f.at);
        assert_eq!(
            before,
            f.engine
                .legacy_minutes_failures
                .lock()
                .unwrap()
                .count(meeting)
        );
        assert!(!f.engine.automatic_minutes_admission(&f.snapshot, &f.input));
        f.engine.note_busy.store(true, Ordering::SeqCst);
        assert!(f.engine.generate_notes(&meeting.id).is_err());
        f.engine.note_busy.store(false, Ordering::SeqCst);
        assert!(!f.engine.automatic_minutes_admission(&f.snapshot, &f.input));
        // No real model is installed in this isolated root. Manual admission
        // still succeeds, clears the hold and ends at normal model admission.
        f.engine.generate_notes(&meeting.id)?;
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while f.engine.note_busy.load(Ordering::SeqCst) && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!f.engine.note_busy.load(Ordering::SeqCst));
        assert!(f.engine.automatic_minutes_admission(&f.snapshot, &f.input));
        assert_eq!(f.engine.inference_worker_status().starts, 0);
        Ok(())
    }

    #[test]
    fn automatic_pause_invalidates_real_input_and_observed_files() -> Result<()> {
        if clean_observation_child(
            "notes::diagnostic::tests::automatic_pause_invalidates_real_input_and_observed_files",
        )? {
            return Ok(());
        }
        for mutation in [
            "note",
            "source",
            "append",
            "model",
            "language",
            "background",
            "runtime",
            "model_file",
            "descriptor",
            "inventory",
            "linked_runtime",
            "unknown",
            "delete_restore",
            "run",
        ] {
            let f = rejection_control()?;
            let meeting = &f.snapshot.meeting;
            let changed = (|| -> Result<bool> {
                match mutation {
                    "note" => {
                        f.engine.save_note(
                            &meeting.id,
                            None,
                            "OmaScribe\n核对名称。",
                            "document",
                            0,
                        )?;
                    }
                    "source" => {
                        let s = &f.snapshot.segments[0];
                        f.engine
                            .edit_segment(&s.id, "照片可能存在风险，仍需核对。", s.revision)?;
                    }
                    "append" => {
                        let mut s = f.snapshot.segments[0].clone();
                        s.id.push_str(":new");
                        s.seq += 1;
                        s.text = "另需核对发布范围。".into();
                        f.engine.store.lock().unwrap().save_segment(&s)?;
                    }
                    "model" | "language" => {
                        let mut p = f.engine.preferences.lock().unwrap().clone();
                        if mutation == "model" {
                            p.llm_model = "qwen3-0.6b".into();
                        } else {
                            p.minutes_language = "en".into();
                        }
                        f.engine.update_preferences(p)?;
                    }
                    "background" => f.engine.invalidate_background()?,
                    "runtime" => {
                        let path = f
                            .engine
                            .runtime_dir
                            .join("llama")
                            .join(crate::platform::executable("llama-server"));
                        let mut bytes = std::fs::read(&path)?;
                        let last = bytes.len() - 1;
                        bytes[last] ^= 1;
                        mutate_observed_control(&f, &path, &bytes, false)?;
                    }
                    "model_file" => {
                        let m =
                            crate::models::model(&f.engine.preferences.lock().unwrap().llm_model)?
                                .clone();
                        mutate_observed_control(
                            &f,
                            &f.engine
                                .root
                                .join("model-store")
                                .join(m.id)
                                .join(&m.files[0].name),
                            b"evil",
                            true,
                        )?;
                    }
                    "descriptor" => mutate_observed_control(
                        &f,
                        &f.engine.runtime_dir.join("llama/build-info.json"),
                        b"{}",
                        false,
                    )?,
                    "inventory" => mutate_observed_control(
                        &f,
                        &f.engine.runtime_dir.join("llama/added.bin"),
                        b"new",
                        false,
                    )?,
                    "linked_runtime" => {
                        #[cfg(unix)]
                        {
                            let external = f.engine.root.join("custom-llama");
                            std::fs::rename(f.engine.runtime_dir.join("llama"), &external)?;
                            std::os::unix::fs::symlink(
                                &external,
                                f.engine.runtime_dir.join("llama"),
                            )?;
                        }
                        #[cfg(not(unix))]
                        {
                            return Ok(false);
                        }
                    }
                    "unknown" => {
                        let block = f
                            .engine
                            .inference_fingerprints
                            .block_directory(&f.engine.runtime_dir.join("llama"))?;
                        drop(block);
                    }
                    "delete_restore" => {
                        f.engine.set_meeting_deleted(&meeting.id, true)?;
                        f.engine.set_meeting_deleted(&meeting.id, false)?;
                        f.engine.finish_minutes_observation(
                            &meeting.id,
                            meeting.run,
                            &f.dispatch,
                            &f.observation,
                            Err(spelling_rejection()),
                        );
                    }
                    "run" => {
                        let mut next = meeting.clone();
                        next.run += 1;
                        f.engine.store.lock().unwrap().save_meeting(&next)?;
                    }
                    _ => unreachable!(),
                }
                Ok(true)
            })();
            if !changed.with_context(|| format!("automatic pause mutation {mutation}"))? {
                continue;
            }
            let current = f.engine.snapshot(&meeting.id)?;
            let prefs = f.engine.preferences.lock().unwrap().clone();
            let input = crate::minutes_schedule::InputDigest::from_snapshot(
                &current,
                &prefs,
                f.engine.context_revision.load(Ordering::SeqCst),
            )?;
            assert!(
                f.engine.automatic_minutes_admission(&current, &input),
                "{mutation}: old rejection blocked changed/unknown scope"
            );
            assert_eq!(f.engine.inference_worker_status().starts, 0);
        }
        let f = rejection_control()?;
        let mut prefs = f.engine.preferences.lock().unwrap().clone();
        prefs.appearance.accent = "#112233".into();
        f.engine.update_preferences(prefs)?;
        let current = f.engine.snapshot(&f.snapshot.meeting.id)?;
        let prefs = f.engine.preferences.lock().unwrap().clone();
        let input = crate::minutes_schedule::InputDigest::from_snapshot(&current, &prefs, 0)?;
        assert_eq!(input, f.input);
        assert!(!f.engine.automatic_minutes_admission(&current, &input));
        Ok(())
    }

    #[test]
    fn automatic_pause_requires_final_complete_validation_terminal() -> Result<()> {
        if clean_observation_child(
            "notes::diagnostic::tests::automatic_pause_requires_final_complete_validation_terminal",
        )? {
            return Ok(());
        }
        for stop in [
            "one_response",
            "nonordinary",
            "unknown_execution",
            "untyped",
            "incomplete",
            "scope",
            "resource",
            "cancel",
            "maintenance",
            "commit",
            "job_write",
            "fallback",
        ] {
            let f = rejection_control()?;
            let prefs = f.engine.preferences.lock().unwrap().clone();
            let trace = TaskTrace::start(&f.engine.root, "minutes", stop);
            let report = Report::new(
                &f.engine.root,
                &f.snapshot.meeting.id,
                &prefs.llm_model,
                0,
                &trace,
            );
            if stop != "unknown_execution" {
                report.observe_execution_control(f.observation.rejected.clone().unwrap());
            }
            if stop == "nonordinary" {
                report.observe_automatic(false);
            }
            for _ in 0..if stop == "one_response" { 1 } else { 2 } {
                report.completed_response();
            }
            let first = spelling_rejection();
            report.record(&f.engine,json!({"attempts":[{"attempt":1,"stage":"compose","validation_error":format!("{first:#}")}]}))?;
            let result: Result<usize> = match stop {
                "one_response" | "nonordinary" | "unknown_execution" => {
                    Err(report.validation_rejected(first))
                }
                "untyped" => Err(first),
                "incomplete" => Err(report.incomplete_response(
                    std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "controlled incomplete stream",
                    )
                    .into(),
                )),
                "scope" => {
                    let scope =
                        super::super::generation::Scope::new(&f.engine, &f.snapshot, &prefs, 0)?;
                    f.engine.save_note(
                        &f.snapshot.meeting.id,
                        None,
                        "需要核对发布范围。",
                        "document",
                        0,
                    )?;
                    scope.check(&f.engine, &f.snapshot.segments).map(|()| 0)
                }
                "resource" => {
                    let scope =
                        super::super::generation::Scope::new(&f.engine, &f.snapshot, &prefs, 0)?;
                    let requests = super::super::generation::Budget::default();
                    let work = super::super::Generation {
                        trace: &trace,
                        diagnostic: &report,
                        scope: &scope,
                        requests: &requests,
                        context_revision: 0,
                    };
                    work.ready(&f.engine, &f.snapshot.segments, true, false, false, 4096)
                        .map(|()| 0)
                }
                "cancel" => {
                    f.engine.cancel_notes();
                    Err(report.validation_rejected(first))
                }
                "maintenance" => {
                    f.engine.maintenance.store(true, Ordering::SeqCst);
                    Err(report.validation_rejected(first))
                }
                "commit" => {
                    report.commit_started();
                    f.engine.store.lock().unwrap().commit_document(
                        &f.snapshot,
                        &f.snapshot.segments,
                        1,
                        vec![],
                        true,
                    )
                }
                "job_write" => {
                    let _rejected = report.validation_rejected(first);
                    f.engine
                        .store
                        .lock()
                        .unwrap()
                        .conn
                        .execute("DROP TABLE inference_jobs", [])?;
                    let written =
                        f.engine
                            .store
                            .lock()
                            .unwrap()
                            .finish_job("control", &json!({}), 0);
                    assert!(written.is_err());
                    report.observe_job_write(&written);
                    written.map(|()| 0)
                }
                "fallback" => {
                    let _rejected = report.validation_rejected(first);
                    report.begin_operation();
                    let requests = super::super::generation::Budget::default();
                    requests.take()?;
                    requests.take()?;
                    requests.ensure_remaining().map(|()| 0)
                }
                _ => unreachable!(),
            };
            assert!(result.is_err(), "{stop}: actual control did not stop");
            report.finish(&f.engine, &result);
            assert!(
                report.rejected_execution(&f.engine, &result).is_none(),
                "{stop}: non-final rejection created a hold"
            );
        }
        Ok(())
    }

    #[test]
    fn automatic_pause_registry_is_bounded_and_stale_owners_cannot_evict() -> Result<()> {
        if clean_observation_child("notes::diagnostic::tests::automatic_pause_registry_is_bounded_and_stale_owners_cannot_evict")? { return Ok(()); }
        use crate::minutes_schedule::{Dispatch, LegacyFailures};
        let f = rejection_control()?;
        let witness = f.observation.rejected.clone().unwrap();
        let mut registry = LegacyFailures::default();
        // Registry-capacity metadata only, not fabricated model outcomes.
        for i in 0..17 {
            let mut meeting = f.snapshot.meeting.clone();
            meeting.id = format!("registry-{i}");
            let dispatch = Dispatch::new(meeting.run, Some(f.input.clone()));
            registry.register_dispatch(&meeting, &dispatch);
            if i == 16 {
                let stale = Dispatch::new(meeting.run, Some(f.input.clone()));
                registry.finished_rejected_dispatch(
                    &meeting,
                    &stale,
                    Some(&f.input),
                    false,
                    Some(witness.clone()),
                );
                let mut first = meeting.clone();
                first.id = "registry-0".into();
                assert!(registry.rejected(&first, &f.input).is_some());
            }
            registry.finished_rejected_dispatch(
                &meeting,
                &dispatch,
                Some(&f.input),
                false,
                Some(witness.clone()),
            );
        }
        let mut first = f.snapshot.meeting.clone();
        first.id = "registry-0".into();
        let mut last = first.clone();
        last.id = "registry-16".into();
        assert!(registry.rejected(&first, &f.input).is_none());
        assert!(registry.rejected(&last, &f.input).is_some());
        Ok(())
    }

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
