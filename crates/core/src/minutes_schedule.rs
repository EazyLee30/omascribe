//! Per-meeting coalescing while live, prompt catch-up after capture, bounded failure backoff.
//! This is scheduling metadata; no speech, generated claims or model state is retained.
use crate::{preferences::Preferences, Meeting, Snapshot};
use anyhow::Result;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

/// Scheduling-change marker only; never a reusable inference result identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InputDigest([u8; 32]);
impl InputDigest {
    pub fn from_snapshot(
        snapshot: &Snapshot,
        prefs: &Preferences,
        background_revision: u64,
    ) -> Result<Self> {
        struct Sink(Sha256);
        impl std::io::Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.update(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        #[derive(Serialize)]
        struct Note<'a> {
            id: &'a str,
            content: &'a str,
            kind: &'a str,
            anchor_ms: u64,
        }
        let mut notes: Vec<_> = crate::notes::document_notes(&snapshot.notes)
            .into_iter()
            .map(|note| Note {
                id: &note.id,
                content: &note.content,
                kind: &note.kind,
                anchor_ms: note.anchor_ms,
            })
            .collect();
        // Select the actual active document before removing timestamps. Legacy
        // cards are then stable under a same-content timestamp-only resave.
        notes.sort_by(|left, right| left.id.cmp(right.id));
        let mut translations: Vec<_> = snapshot.translations.values().collect();
        translations.sort_by(|left, right| left.segment_id.cmp(&right.segment_id));
        let mut preferences = serde_json::to_value(prefs)?;
        preferences
            .as_object_mut()
            .expect("preferences object")
            .remove("appearance");
        let mut sink = Sink(Sha256::new());
        serde_json::to_writer(
            &mut sink,
            &(
                "minutes-penalty-input-v1",
                &snapshot.meeting.id,
                snapshot.meeting.run,
                snapshot.meeting.notes_cursor,
                &snapshot.meeting.notes_fingerprint,
                &snapshot.meeting.speaker_names,
                &snapshot.segments,
                notes,
                &snapshot.suggestions,
                translations,
                preferences,
                background_revision,
            ),
        )?;
        Ok(Self(sink.0.finalize().into()))
    }
}

/// Default live retries preserve task Ok/Err semantics. Keep this separate
/// from catch-up/post-capture scheduling's persisted-frontier progress tracking.
/// Each meeting retains at most one run, and successful tasks retain nothing.
#[derive(Default)]
pub(crate) struct LegacyFailures(HashMap<String, Failure>);
struct Failure {
    run: u32,
    count: u32,
    input: Option<InputDigest>,
    registration: Option<Registration>,
}
struct Registration {
    owner: Weak<Mutex<Option<AttemptCompletion>>>,
    input: Option<InputDigest>,
}
impl LegacyFailures {
    pub fn count(&self, meeting: &Meeting) -> u32 {
        self.0
            .get(&meeting.id)
            .filter(|entry| entry.run == meeting.run)
            .map_or(0, |entry| entry.count)
    }
    pub fn observe_input(&mut self, meeting: &Meeting, input: Option<&InputDigest>) {
        let Some(entry) = self
            .0
            .get_mut(&meeting.id)
            .filter(|entry| entry.run == meeting.run)
        else {
            return;
        };
        if let Some(input) = input {
            if entry.input.as_ref().is_some_and(|old| old != input) {
                entry.count = 0;
            }
            entry.input = Some(input.clone());
        }
    }
    /// The caller holds Store and has checked the live run/deletion state.
    pub fn finished(&mut self, meeting: &Meeting, succeeded: bool) {
        self.finished_for_input(meeting, None, succeeded);
    }
    pub fn finished_for_input(
        &mut self,
        meeting: &Meeting,
        input: Option<&InputDigest>,
        succeeded: bool,
    ) {
        let previous = self
            .0
            .get(&meeting.id)
            .filter(|entry| entry.run == meeting.run);
        // A stale automatic completion cannot modify a newer observed input.
        if input
            .zip(previous.and_then(|entry| entry.input.as_ref()))
            .is_some_and(|(launched, current)| launched != current)
        {
            return;
        }
        if succeeded {
            self.forget(&meeting.id);
        } else {
            let count = self.count(meeting).saturating_add(1).min(3);
            let input = input
                .cloned()
                .or_else(|| previous.and_then(|entry| entry.input.clone()));
            self.0.insert(
                meeting.id.clone(),
                Failure {
                    run: meeting.run,
                    count,
                    input,
                    registration: None,
                },
            );
        }
    }
    /// Register only after the dispatcher acquired note_busy and will spawn.
    pub fn register_dispatch(&mut self, meeting: &Meeting, dispatch: &Dispatch) {
        if dispatch.run != meeting.run {
            return;
        }
        let entry = self.0.entry(meeting.id.clone()).or_insert_with(|| Failure {
            run: meeting.run,
            count: 0,
            input: None,
            registration: None,
        });
        if entry.run != meeting.run {
            *entry = Failure {
                run: meeting.run,
                count: 0,
                input: None,
                registration: None,
            };
        }
        if let Some(input) = &dispatch.input {
            if entry.input.as_ref().is_some_and(|old| old != input) {
                entry.count = 0;
            }
            entry.input = Some(input.clone());
        }
        entry.registration = Some(Registration {
            owner: dispatch.identity(),
            input: entry.input.clone(),
        });
    }
    pub fn finished_dispatch(
        &mut self,
        meeting: &Meeting,
        dispatch: &Dispatch,
        actual: Option<&InputDigest>,
        succeeded: bool,
    ) {
        let Some(entry) = self
            .0
            .get_mut(&meeting.id)
            .filter(|entry| entry.run == meeting.run && dispatch.run == meeting.run)
        else {
            return;
        };
        let Some(registration) = entry
            .registration
            .as_ref()
            .filter(|registration| registration.owner.ptr_eq(&dispatch.identity()))
        else {
            return;
        };
        // A later observed input owns its own penalty. Finishing the old job
        // must neither reset it nor attach that job's rejection to it.
        if entry.input != registration.input && entry.input.as_ref() != actual {
            return;
        }
        entry.registration = None;
        if let Some(actual) = actual {
            if entry.input.as_ref().is_some_and(|old| old != actual) {
                entry.count = 0;
            }
            entry.input = Some(actual.clone());
        }
        if succeeded {
            self.forget(&meeting.id);
        } else {
            entry.count = entry.count.saturating_add(1).min(3);
        }
    }
    pub fn forget(&mut self, id: &str) {
        self.0.remove(id);
    }
    #[cfg(test)]
    pub fn tracked(&self) -> usize {
        self.0.len()
    }
}

/// Each scheduled dispatch owns its completion slot; manual work never writes it.
#[derive(Clone)]
pub(crate) struct Dispatch {
    pub run: u32,
    pub input: Option<InputDigest>,
    completion: Arc<Mutex<Option<AttemptCompletion>>>,
}
pub(crate) struct AttemptCompletion {
    pub run: u32,
    pub input: Option<InputDigest>,
    pub succeeded: bool,
    owner: Weak<Mutex<Option<AttemptCompletion>>>,
}
impl Dispatch {
    pub fn new(run: u32, input: Option<InputDigest>) -> Self {
        Self {
            run,
            input,
            completion: Arc::new(Mutex::new(None)),
        }
    }
    #[cfg(test)]
    pub fn complete(&self, succeeded: bool) {
        self.complete_observed(succeeded, self.input.clone());
    }
    pub fn complete_observed(&self, succeeded: bool, input: Option<InputDigest>) {
        *self.completion.lock().unwrap() = Some(AttemptCompletion {
            run: self.run,
            input,
            succeeded,
            owner: self.identity(),
        });
    }
    fn identity(&self) -> Weak<Mutex<Option<AttemptCompletion>>> {
        Arc::downgrade(&self.completion)
    }
    pub fn take_completion(&self) -> Option<AttemptCompletion> {
        self.completion.lock().unwrap().take()
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Frontier {
    run: u32,
    cursor: u32,
    fingerprint: String,
}
impl From<&Meeting> for Frontier {
    fn from(meeting: &Meeting) -> Self {
        Self {
            run: meeting.run,
            cursor: meeting.notes_cursor,
            fingerprint: meeting.notes_fingerprint.clone(),
        }
    }
}
struct Entry {
    run: u32,
    was_active: bool,
    final_pending: bool,
    in_flight: bool,
    before: Option<Frontier>,
    finished_at: Option<Instant>,
    progressed: bool,
    failures: u32,
    input: Option<InputDigest>,
    launch_input: Option<InputDigest>,
    owner: Option<Weak<Mutex<Option<AttemptCompletion>>>>,
}
impl Entry {
    fn new(run: u32) -> Self {
        Self {
            run,
            was_active: false,
            final_pending: false,
            in_flight: false,
            before: None,
            finished_at: None,
            progressed: false,
            failures: 0,
            input: None,
            launch_input: None,
            owner: None,
        }
    }
}
#[derive(Default)]
pub(crate) struct Schedule(HashMap<String, Entry>);
impl Schedule {
    pub fn observe(&mut self, meeting: &Meeting, own_capture_active: bool) {
        let entry = self
            .0
            .entry(meeting.id.clone())
            .or_insert_with(|| Entry::new(meeting.run));
        if entry.run != meeting.run {
            *entry = Entry::new(meeting.run);
        }
        if entry.was_active && !own_capture_active {
            entry.final_pending = true;
        }
        if own_capture_active {
            // A new take gets its own one-shot finalization opportunity.
            entry.final_pending = false;
        }
        entry.was_active = own_capture_active;
    }
    pub fn observe_with_input(
        &mut self,
        meeting: &Meeting,
        own_capture_active: bool,
        input: Option<&InputDigest>,
    ) {
        self.observe(meeting, own_capture_active);
        let entry = self.0.get_mut(&meeting.id).expect("observed entry");
        if let Some(input) = input {
            if entry.input.as_ref().is_some_and(|old| old != input) {
                entry.failures = 0;
            }
            entry.input = Some(input.clone());
        }
    }
    pub fn penalized(&self, meeting: &Meeting) -> bool {
        self.0
            .get(&meeting.id)
            .is_some_and(|entry| entry.run == meeting.run && entry.failures > 0)
    }
    pub fn due(&self, id: &str, any_capture_active: bool, quality: bool, now: Instant) -> bool {
        let Some(entry) = self.0.get(id) else {
            return false;
        };
        if entry.in_flight || any_capture_active && quality {
            return false;
        }
        if !any_capture_active && (entry.final_pending || entry.progressed) {
            return true;
        }
        let delay = Duration::from_secs(45 * 2u64.pow(entry.failures.min(3)));
        entry
            .finished_at
            .is_none_or(|finished| now.saturating_duration_since(finished) >= delay)
    }
    #[cfg(test)]
    pub fn started(&mut self, meeting: &Meeting, now: Instant) {
        self.started_for_input(meeting, None, now);
    }
    pub fn started_for_input(
        &mut self,
        meeting: &Meeting,
        input: Option<&InputDigest>,
        now: Instant,
    ) {
        let Some(entry) = self.0.get_mut(&meeting.id) else {
            return;
        };
        entry.before = Some(Frontier::from(meeting));
        entry.launch_input = input.cloned();
        entry.owner = None;
        if let Some(input) = input {
            entry.input = Some(input.clone());
        }
        entry.in_flight = true;
        entry.final_pending = false;
        entry.progressed = false;
        entry.finished_at = Some(now);
    }
    #[cfg(test)]
    pub fn finished(&mut self, meeting: &Meeting, now: Instant) {
        self.finished_for_input(meeting, None, None, now);
    }
    pub fn started_for_dispatch(&mut self, meeting: &Meeting, dispatch: &Dispatch, now: Instant) {
        if dispatch.run != meeting.run {
            return;
        }
        self.started_for_input(meeting, dispatch.input.as_ref(), now);
        if let Some(entry) = self
            .0
            .get_mut(&meeting.id)
            .filter(|entry| entry.run == dispatch.run)
        {
            entry.owner = Some(dispatch.identity());
        }
    }
    pub fn finished_for_input(
        &mut self,
        meeting: &Meeting,
        input: Option<&InputDigest>,
        completion: Option<&AttemptCompletion>,
        now: Instant,
    ) {
        let Some(entry) = self.0.get_mut(&meeting.id) else {
            return;
        };
        if let Some(completion) = completion {
            let same_dispatch = entry.owner.as_ref().map_or_else(
                || completion.input == entry.launch_input,
                |owner| owner.ptr_eq(&completion.owner),
            );
            if !entry.in_flight
                || completion.run != meeting.run
                || entry.run != meeting.run
                || !same_dispatch
            {
                return;
            }
        }
        if entry.run != meeting.run {
            *entry = Entry::new(meeting.run);
            return;
        }
        let current = Frontier::from(meeting);
        let frontier_progress = entry.before.as_ref().is_some_and(|before| {
            current.cursor > before.cursor || current.fingerprint != before.fingerprint
        });
        if let Some(completed) = completion {
            entry.progressed = completed.succeeded && frontier_progress;
            let changed_since_admission = completed
                .input
                .as_ref()
                .zip(input)
                .is_some_and(|(actual, current)| actual != current);
            let changed_at_admission = completed
                .input
                .as_ref()
                .zip(entry.input.as_ref())
                .is_some_and(|(actual, previous)| actual != previous);
            if changed_at_admission {
                entry.failures = 0;
            }
            entry.failures = if entry.progressed || changed_since_admission {
                0
            } else {
                entry.failures.saturating_add(1).min(3)
            };
            if let Some(actual) = &completed.input {
                entry.input = Some(input.unwrap_or(actual).clone());
            }
        } else {
            // Missing completion provenance keeps the original frontier policy.
            entry.progressed = frontier_progress;
            entry.failures = if entry.progressed {
                0
            } else {
                entry.failures.saturating_add(1).min(3)
            };
        }
        entry.owner = None;
        entry.in_flight = false;
        entry.finished_at = Some(now);
    }
    /// A missing completion snapshot must release the existing in-flight slot.
    /// It cannot create an entry or assert persisted frontier progress.
    pub fn finish_unavailable(&mut self, id: &str, dispatch: &Dispatch, now: Instant) {
        let Some(entry) = self.0.get_mut(id).filter(|entry| entry.run == dispatch.run) else {
            return;
        };
        let same_dispatch = entry.owner.as_ref().map_or_else(
            || entry.launch_input == dispatch.input,
            |owner| owner.ptr_eq(&dispatch.identity()),
        );
        if !entry.in_flight || !same_dispatch {
            return;
        }
        entry.owner = None;
        entry.in_flight = false;
        entry.progressed = false;
        entry.failures = entry.failures.saturating_add(1).min(3);
        entry.finished_at = Some(now);
    }
    pub fn forget(&mut self, id: &str) {
        self.0.remove(id);
    }
    pub fn retain(&mut self, ids: &[String]) {
        self.0.retain(|id, _| ids.contains(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(byte: u8) -> InputDigest {
        InputDigest([byte; 32])
    }
    fn completion(dispatch: &Dispatch, succeeded: bool) -> AttemptCompletion {
        dispatch.complete(succeeded);
        dispatch.take_completion().unwrap()
    }
    #[test]
    fn post_translation_actual_admission_is_bound_instead_of_scheduled_input() {
        let m = meeting("translated");
        let at = Instant::now();
        let scheduled = input(1);
        let actual = input(2);
        let edited = input(3);
        for edit_after_admission in [false, true] {
            let mut schedule = Schedule::default();
            schedule.observe_with_input(&m, false, Some(&scheduled));
            for _ in 0..3 {
                schedule.started(&m, at);
                schedule.finished(&m, at);
            }
            let dispatch = Dispatch::new(m.run, Some(scheduled.clone()));
            schedule.started_for_dispatch(&m, &dispatch, at);
            dispatch.complete_observed(false, Some(actual.clone()));
            let done = dispatch.take_completion().unwrap();
            let current = if edit_after_admission {
                &edited
            } else {
                &actual
            };
            schedule.finished_for_input(&m, Some(current), Some(&done), at);
            assert!(!schedule.0[&m.id].in_flight);
            assert!(!schedule.0[&m.id].progressed);
            assert!(!schedule.due(&m.id, false, false, at + Duration::from_secs(2)));
            assert_eq!(
                schedule.due(&m.id, false, false, at + Duration::from_secs(46)),
                edit_after_admission
            );
            assert!(schedule.due(&m.id, false, false, at + Duration::from_secs(90)));
            let mut legacy = LegacyFailures::default();
            for _ in 0..3 {
                legacy.finished_for_input(&m, Some(&scheduled), false);
            }
            legacy.register_dispatch(&m, &dispatch);
            legacy.finished_dispatch(&m, &dispatch, Some(&actual), false);
            assert_eq!(legacy.count(&m), 1);
            legacy.observe_input(&m, Some(current));
            assert_eq!(legacy.count(&m), if edit_after_admission { 0 } else { 1 });
        }
    }
    #[test]
    fn dispatch_identity_accepts_actual_key_change_and_rejects_other_slot() {
        let m = meeting("owner");
        let at = Instant::now();
        let old = input(1);
        let actual = input(2);
        let a = Dispatch::new(m.run, Some(old.clone()));
        let b = Dispatch::new(m.run, Some(old.clone()));
        let mut schedule = Schedule::default();
        schedule.observe_with_input(&m, false, Some(&old));
        schedule.started_for_dispatch(&m, &b, at);
        a.complete_observed(false, Some(actual.clone()));
        let wrong = a.take_completion().unwrap();
        schedule.finished_for_input(&m, Some(&actual), Some(&wrong), at);
        assert!(schedule.0[&m.id].in_flight);
        b.complete_observed(true, Some(actual.clone()));
        let done = b.take_completion().unwrap();
        let mut committed = m.clone();
        committed.notes_cursor = 5;
        committed.notes_fingerprint = "committed".into();
        schedule.finished_for_input(&committed, Some(&input(3)), Some(&done), at);
        assert!(schedule.due(&m.id, false, false, at));
    }
    #[test]
    fn unknown_actual_keeps_failure_delay_and_healthy_observation_creates_no_failure_entry() {
        let m = meeting("unknown_actual");
        let key = input(1);
        let at = Instant::now();
        let mut legacy = LegacyFailures::default();
        legacy.observe_input(&m, Some(&key));
        assert_eq!(legacy.tracked(), 0);
        for _ in 0..3 {
            legacy.finished_for_input(&m, Some(&key), false);
        }
        let dispatch = Dispatch::new(m.run, Some(key.clone()));
        legacy.register_dispatch(&m, &dispatch);
        legacy.finished_dispatch(&m, &dispatch, None, false);
        assert_eq!(legacy.count(&m), 3);
        let mut schedule = Schedule::default();
        schedule.observe_with_input(&m, false, Some(&key));
        for _ in 0..3 {
            schedule.started(&m, at);
            schedule.finished(&m, at);
        }
        schedule.started_for_dispatch(&m, &dispatch, at);
        dispatch.complete_observed(false, None);
        let done = dispatch.take_completion().unwrap();
        schedule.finished_for_input(&m, Some(&key), Some(&done), at);
        assert!(!schedule.0[&m.id].in_flight);
        assert!(!schedule.due(&m.id, false, false, at + Duration::from_secs(359)));
        assert!(schedule.due(&m.id, false, false, at + Duration::from_secs(360)));
    }

    #[test]
    fn known_changed_input_clears_only_penalty_not_clock_or_inflight() {
        let m = meeting("clock");
        let at = Instant::now();
        let mut schedule = Schedule::default();
        let old = input(1);
        let new = input(2);
        schedule.observe_with_input(&m, false, Some(&old));
        for _ in 0..3 {
            schedule.started(&m, at);
            schedule.finished(&m, at);
        }
        schedule.observe_with_input(&m, false, Some(&new));
        assert_eq!(schedule.0[&m.id].finished_at, Some(at));
        assert!(!schedule.due(&m.id, false, false, at + Duration::from_secs(2)));
        assert!(schedule.due(&m.id, false, false, at + Duration::from_secs(46)));
        schedule.started_for_input(&m, Some(&new), at);
        schedule.observe_with_input(&m, false, Some(&input(3)));
        assert!(schedule.0[&m.id].in_flight);
        assert!(!schedule.due(&m.id, false, false, at + Duration::from_secs(500)));
    }
    #[test]
    fn unchanged_or_unknown_input_keeps_failure_delay() {
        let m = meeting("old");
        let at = Instant::now();
        let key = input(1);
        let mut schedule = Schedule::default();
        schedule.observe_with_input(&m, false, Some(&key));
        for _ in 0..3 {
            schedule.started(&m, at);
            schedule.finished(&m, at);
        }
        for value in [Some(&key), None] {
            schedule.observe_with_input(&m, false, value);
            assert!(!schedule.due(&m.id, false, false, at + Duration::from_secs(359)));
            assert!(schedule.due(&m.id, false, false, at + Duration::from_secs(360)));
        }
    }
    #[test]
    fn rejected_completion_cannot_call_cleared_fingerprint_success() {
        for current_known in [true, false] {
            let mut m = meeting("changed");
            m.notes_fingerprint = "old".into();
            let at = Instant::now();
            let old = input(1);
            let new = input(2);
            let dispatch = Dispatch::new(m.run, Some(old.clone()));
            let mut schedule = Schedule::default();
            schedule.observe_with_input(&m, false, Some(&old));
            schedule.started_for_input(&m, Some(&old), at);
            m.notes_fingerprint.clear();
            let done = completion(&dispatch, false);
            schedule.finished_for_input(&m, current_known.then_some(&new), Some(&done), at);
            assert!(!schedule.0[&m.id].progressed);
            assert!(!schedule.due(&m.id, false, false, at + Duration::from_secs(2)));
            assert_eq!(
                schedule.due(&m.id, false, false, at + Duration::from_secs(46)),
                current_known
            );
        }
    }
    #[test]
    fn successful_cursor_progress_survives_new_document_key() {
        let mut m = meeting("success");
        let at = Instant::now();
        let old = input(1);
        let new = input(2);
        let dispatch = Dispatch::new(m.run, Some(old.clone()));
        let mut schedule = Schedule::default();
        schedule.observe_with_input(&m, false, Some(&old));
        schedule.started_for_input(&m, Some(&old), at);
        m.notes_cursor = 5;
        m.notes_fingerprint = "committed".into();
        let done = completion(&dispatch, true);
        schedule.finished_for_input(&m, Some(&new), Some(&done), at);
        assert!(schedule.due(&m.id, false, false, at));
        assert!(!schedule.due(&m.id, true, false, at + Duration::from_secs(2)));
    }
    #[test]
    fn dispatch_completions_are_isolated_and_stale_results_do_not_restore_state() {
        let m = meeting("isolation");
        let old = input(1);
        let new = input(2);
        let at = Instant::now();
        let a = Dispatch::new(m.run, Some(old.clone()));
        let b = Dispatch::new(m.run, Some(new.clone()));
        a.complete(false);
        b.complete(true);
        assert!(!a.take_completion().unwrap().succeeded);
        assert!(b.take_completion().unwrap().succeeded);
        assert!(a.take_completion().is_none());
        let mut failures = LegacyFailures::default();
        failures.finished_for_input(&m, Some(&old), false);
        failures.observe_input(&m, Some(&new));
        failures.finished_for_input(&m, Some(&old), false);
        assert_eq!(failures.count(&m), 0);
        failures.finished_for_input(&m, Some(&new), false);
        assert_eq!(failures.count(&m), 1);
        assert_eq!(failures.tracked(), 1);
        let mut schedule = Schedule::default();
        schedule.observe_with_input(&m, false, Some(&new));
        schedule.started_for_input(&m, Some(&new), at);
        let mut old_run = completion(&a, false);
        old_run.run = m.run.saturating_sub(1);
        schedule.finished_for_input(&m, Some(&new), Some(&old_run), at);
        assert!(schedule.0[&m.id].in_flight);
        schedule.forget(&m.id);
        schedule.finish_unavailable(&m.id, &b, at);
        assert!(schedule.0.is_empty());
    }
    #[test]
    fn unavailable_snapshot_releases_only_existing_matching_dispatch() {
        let m = meeting("unavailable");
        let key = input(1);
        let at = Instant::now();
        let dispatch = Dispatch::new(m.run, Some(key.clone()));
        let mut schedule = Schedule::default();
        schedule.observe_with_input(&m, false, Some(&key));
        schedule.started_for_input(&m, Some(&key), at);
        schedule.finish_unavailable(&m.id, &dispatch, at);
        assert!(!schedule.0[&m.id].in_flight);
        assert!(!schedule.due(&m.id, false, false, at + Duration::from_secs(2)));
        assert!(schedule.due(&m.id, false, false, at + Duration::from_secs(90)));
    }
    fn meeting(id: &str) -> Meeting {
        Meeting {
            id: id.into(),
            title: String::new(),
            created_at: String::new(),
            status: "recording".into(),
            duration_ms: 0,
            run: 1,
            notes_cursor: 0,
            notes_fingerprint: String::new(),
            auto_notes: true,
            diarization: false,
            diarization_cursor: 0,
            speaker_names: Default::default(),
        }
    }
    #[test]
    fn successful_live_work_coalesces_but_stop_drains_progress_without_45s_gaps() {
        let now = Instant::now();
        let mut schedule = Schedule::default();
        let mut m = meeting("one");
        schedule.observe(&m, true);
        assert!(schedule.due(&m.id, true, false, now));
        schedule.started(&m, now);
        assert!(!schedule.due(&m.id, false, false, now));
        m.notes_cursor = 5;
        schedule.finished(&m, now);
        assert!(!schedule.due(&m.id, true, false, now + Duration::from_secs(2)));
        // Paused/transcribing are still active; only their final transition drains.
        schedule.observe(&m, true);
        assert!(!schedule.due(&m.id, true, false, now + Duration::from_secs(2)));
        schedule.observe(&m, false);
        assert!(schedule.due(&m.id, false, false, now + Duration::from_secs(2)));
        schedule.started(&m, now);
        m.notes_cursor = 10;
        schedule.finished(&m, now);
        assert!(schedule.due(&m.id, false, false, now));
    }
    #[test]
    fn failed_finalization_backs_off_and_another_meeting_is_independent() {
        let now = Instant::now();
        let mut schedule = Schedule::default();
        let a = meeting("one");
        schedule.observe(&a, true);
        schedule.started(&a, now);
        schedule.finished(&a, now);
        schedule.observe(&a, false);
        assert!(schedule.due(&a.id, false, false, now));
        schedule.started(&a, now);
        schedule.finished(&a, now);
        assert!(!schedule.due(&a.id, false, false, now + Duration::from_secs(2)));
        assert!(schedule.due(&a.id, false, false, now + Duration::from_secs(180)));
        let b = meeting("two");
        schedule.observe(&b, false);
        assert!(schedule.due(&b.id, false, false, now));
    }
    #[test]
    fn quality_mode_defers_without_failed_model_attempts_and_new_runs_reset_backoff() {
        let now = Instant::now();
        let mut schedule = Schedule::default();
        let mut m = meeting("one");
        schedule.observe(&m, true);
        assert!(!schedule.due(&m.id, true, true, now));
        schedule.observe(&m, false);
        assert!(schedule.due(&m.id, false, true, now));
        schedule.started(&m, now);
        schedule.finished(&m, now);
        assert!(!schedule.due(&m.id, false, true, now));
        m.run += 1;
        schedule.observe(&m, false);
        assert!(schedule.due(&m.id, false, true, now));
        schedule.forget(&m.id);
        assert!(!schedule.due(&m.id, false, false, now));
        schedule.observe(&m, false);
        schedule.retain(&[]);
        assert!(!schedule.due(&m.id, false, false, now));
    }
}
