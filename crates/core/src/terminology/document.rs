//! Replay only concrete, source-backed spelling edits into fresh AI prose.
use crate::{store::Store, Meeting, Segment, Source, Suggestion};
use anyhow::Result;
use regex::Regex;
use rusqlite::{params, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub(super) struct Replacement {
    pub old: String,
    pub new: String,
    pub model: bool,
}
pub(super) struct SourceChange {
    pub before: Segment,
    pub after: Segment,
    pub replacements: Vec<Replacement>,
}

/// Case-sensitive literals observed in source, never near-name matching of prose.
fn replay(text: &str, edits: &BTreeMap<String, Replacement>) -> Option<(String, BTreeSet<String>)> {
    if edits.is_empty() {
        return None;
    }
    let mut keys: Vec<_> = edits.keys().collect();
    keys.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let matcher = Regex::new(
        &keys
            .iter()
            .map(|s| regex::escape(s))
            .collect::<Vec<_>>()
            .join("|"),
    )
    .ok()?;
    let mut output = String::new();
    let mut cursor = 0;
    let mut used = BTreeSet::new();
    for hit in matcher.find_iter(text) {
        let edit = edits.get(hit.as_str())?;
        let first = edit.old.chars().next()?;
        let last = edit.old.chars().last()?;
        let before = text[..hit.start()].chars().next_back();
        let after = text[hit.end()..].chars().next();
        let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
        if first.is_ascii_alphanumeric() && before.is_some_and(word)
            || last.is_ascii_alphanumeric() && after.is_some_and(word)
            || edit.model && super::model_has_continuation(&text[hit.end()..])
        {
            continue;
        }
        output.push_str(&text[cursor..hit.start()]);
        output.push_str(&edit.new);
        cursor = hit.end();
        used.insert(edit.old.clone());
    }
    output.push_str(&text[cursor..]);
    Some((output, used))
}

fn merge(edits: &mut BTreeMap<String, Replacement>, edit: &Replacement) -> Option<()> {
    if edit.old.is_empty() || edit.old == edit.new {
        return None;
    }
    if let Some(old) = edits.get_mut(&edit.old) {
        if old.new != edit.new {
            return None;
        }
        old.model |= edit.model;
    } else {
        edits.insert(edit.old.clone(), edit.clone());
    }
    Some(())
}

fn source_matches(source: &Source, segment: &Segment, meeting: &Meeting) -> bool {
    source.segment_id == segment.id
        && source.revision == segment.revision
        && source.quote == segment.text
        && source.start_ms == segment.start_ms
        && source.end_ms == segment.end_ms
        && source.speaker_name
            == segment
                .speaker
                .as_ref()
                .and_then(|id| meeting.speaker_names.get(id))
                .cloned()
        && segment.meeting_id == meeting.id
        && segment.run == meeting.run
}

fn repair(
    before: &Suggestion,
    meeting: &Meeting,
    current_sources: &BTreeMap<String, Segment>,
    changes: &[SourceChange],
) -> Option<Suggestion> {
    if before.meeting_id != meeting.id
        || before.status != "pending"
        || before.stale
        || before.sources.is_empty()
    {
        return None;
    }
    let mut edits = BTreeMap::new();
    let mut updated = before.clone();
    let mut changed = false;
    for source in &mut updated.sources {
        let current = current_sources.get(&source.segment_id)?;
        if let Some(change) = changes.iter().find(|c| c.before.id == source.segment_id) {
            if !source_matches(source, &change.before, meeting)
                || !source_matches(
                    &Source {
                        segment_id: change.after.id.clone(),
                        revision: change.after.revision,
                        start_ms: change.after.start_ms,
                        end_ms: change.after.end_ms,
                        quote: change.after.text.clone(),
                        speaker_name: source.speaker_name.clone(),
                    },
                    current,
                    meeting,
                )
            {
                return None;
            }
            let mut local = BTreeMap::new();
            for edit in &change.replacements {
                merge(&mut local, edit)?;
            }
            let (revised, used) = replay(&change.before.text, &local)?;
            if revised != change.after.text || used.is_empty() {
                return None;
            }
            for key in used {
                merge(&mut edits, &local[&key])?;
            }
            source.quote = current.text.clone();
            source.revision = current.revision;
            source.start_ms = current.start_ms;
            source.end_ms = current.end_ms;
            changed = true;
        } else if !source_matches(source, current, meeting) {
            return None;
        }
    }
    if !changed {
        return None;
    }
    let (body, used) = replay(&before.text, &edits)?;
    if used.is_empty() {
        return None;
    }
    let used_edits = edits
        .into_iter()
        .filter(|(key, _)| used.contains(key))
        .collect();
    // Every cited use needs its own authorization. A source changed for Omega
    // must not inherit another source's ALPHA replacement inside ALPHA Beta.
    for source in &before.sources {
        let mut local = BTreeMap::new();
        if let Some(change) = changes.iter().find(|c| c.before.id == source.segment_id) {
            for edit in &change.replacements {
                if used.contains(&edit.old) {
                    merge(&mut local, edit)?;
                }
            }
        }
        let authorized = if local.is_empty() {
            source.quote.clone()
        } else {
            replay(&source.quote, &local)?.0
        };
        if replay(&source.quote, &used_edits)?.0 != authorized {
            return None;
        }
    }
    updated.text = body;
    updated.stale = false;
    Some(updated)
}

impl Store {
    pub(super) fn repair_terminology_documents(
        &mut self,
        meeting: &Meeting,
        before: &[Suggestion],
        changes: &[SourceChange],
    ) -> Result<usize> {
        if changes.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        let current_meeting: Meeting = serde_json::from_str(&tx.query_row(
            "SELECT json FROM meetings WHERE id=?1",
            [&meeting.id],
            |r| r.get::<_, String>(0),
        )?)?;
        if current_meeting.run != meeting.run {
            return Ok(0);
        }
        let mut query =
            tx.prepare("SELECT json FROM suggestions WHERE meeting_id=?1 ORDER BY rowid")?;
        let existing: Vec<Suggestion> = query
            .query_map([&meeting.id], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect::<Result<_>>()?;
        drop(query);
        let mut repaired = Vec::new();
        for old in before.iter().filter(|s| s.status == "pending" && !s.stale) {
            let Some(current) = existing.iter().find(|s| s.id == old.id) else {
                continue;
            };
            let mut expected = old.clone();
            expected.stale = current.stale;
            if serde_json::to_string(&expected)? != serde_json::to_string(current)? {
                continue;
            }
            let mut sources = BTreeMap::new();
            let mut protected = false;
            for source in &old.sources {
                let value: Option<String> = tx
                    .query_row(
                        "SELECT json FROM segments WHERE id=?1 AND meeting_id=?2 AND run=?3",
                        params![source.segment_id, meeting.id, meeting.run],
                        |r| r.get(0),
                    )
                    .optional()?;
                let origin: Option<String> = tx.query_row(
                    "SELECT origin FROM segment_text_edits WHERE id=?1 ORDER BY revision DESC LIMIT 1",
                    [&source.segment_id], |r| r.get(0),
                ).optional()?;
                if origin.as_deref() == Some("human") {
                    protected = true;
                    break;
                }
                if let Some(value) = value {
                    let segment: Segment = serde_json::from_str(&value)?;
                    sources.insert(segment.id.clone(), segment);
                }
            }
            if !protected {
                if let Some(section) = repair(old, &current_meeting, &sources, changes) {
                    repaired.push(section);
                }
            }
        }
        if !repaired.is_empty() {
            tx.execute(
                "INSERT INTO document_history(meeting_id,created_at,json) VALUES(?1,?2,?3)",
                params![
                    meeting.id,
                    chrono::Utc::now().to_rfc3339(),
                    serde_json::to_string(&existing)?
                ],
            )?;
            for section in &repaired {
                tx.execute(
                    "UPDATE suggestions SET json=?2 WHERE id=?1 AND meeting_id=?3",
                    params![section.id, serde_json::to_string(section)?, meeting.id],
                )?;
            }
        }
        tx.commit()?;
        Ok(repaired.len())
    }
}
