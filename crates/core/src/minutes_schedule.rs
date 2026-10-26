//! Per-meeting coalescing while live, prompt catch-up after capture, bounded failure backoff.
//! This is scheduling metadata; no speech, generated claims or model state is retained.
use crate::Meeting;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

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
    pub fn started(&mut self, meeting: &Meeting, now: Instant) {
        let Some(entry) = self.0.get_mut(&meeting.id) else {
            return;
        };
        entry.before = Some(Frontier::from(meeting));
        entry.in_flight = true;
        entry.final_pending = false;
        entry.progressed = false;
        entry.finished_at = Some(now);
    }
    pub fn finished(&mut self, meeting: &Meeting, now: Instant) {
        let Some(entry) = self.0.get_mut(&meeting.id) else {
            return;
        };
        if entry.run != meeting.run {
            *entry = Entry::new(meeting.run);
            return;
        }
        let current = Frontier::from(meeting);
        entry.progressed = entry.before.as_ref().is_some_and(|before| {
            current.cursor > before.cursor || current.fingerprint != before.fingerprint
        });
        entry.failures = if entry.progressed {
            0
        } else {
            entry.failures.saturating_add(1).min(3)
        };
        entry.in_flight = false;
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
