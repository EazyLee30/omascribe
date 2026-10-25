//! Conservative, local spelling normalization. Only explicit aliases and casing are replaced.
use crate::{library::Entry, Engine, UserNote};
use anyhow::Result;
use regex::{Regex, RegexBuilder};
use std::collections::{BTreeMap, HashSet};

pub struct Terms {
    matcher: Option<Regex>,
    names: BTreeMap<String, String>,
    pub hotwords: Vec<String>,
}
impl Terms {
    pub fn new(notes: &[UserNote], entries: &[Entry]) -> Self {
        let mut names = BTreeMap::new();
        let mut conflicts = HashSet::new();
        let mut hotwords = Vec::new();
        let explicit: HashSet<String> = entries
            .iter()
            .filter(|e| e.kind == "term" && e.enabled && !e.deleted)
            .flat_map(|e| {
                std::iter::once(e.title.as_str())
                    .chain(e.aliases.split([',', '，', ';', '；', '\n', '|']))
            })
            .map(|s| s.trim().to_lowercase())
            .collect();
        let mut add = |alias: &str, name: &str| {
            let alias = alias.trim();
            let name = name.trim();
            if !(2..=80).contains(&alias.chars().count()) || name.is_empty() {
                return;
            }
            let key = alias.to_lowercase();
            if names.get(&key).is_some_and(|old| old != name) {
                conflicts.insert(key.clone());
            }
            names.insert(key, name.to_owned());
        };
        for e in entries
            .iter()
            .filter(|e| e.kind == "term" && e.enabled && !e.deleted)
            .take(80)
        {
            hotwords.push(e.title.trim().to_owned());
            add(&e.title, &e.title);
            for alias in e.aliases.split([',', '，', ';', '；', '\n', '|']) {
                add(alias, &e.title);
            }
        }
        // User-written Latin names supply spelling/casing, never facts or inferred aliases.
        let latin = Regex::new(r"[A-Za-z][A-Za-z0-9_+.-]{2,39}").unwrap();
        for n in crate::notes::document_notes(notes) {
            for word in latin.find_iter(&n.content).take(80) {
                let word = word.as_str();
                if ["nbsp", "amp", "http", "https"].contains(&word.to_lowercase().as_str()) {
                    continue;
                }
                let name = if word.eq_ignore_ascii_case("omascribe") {
                    "OmaScribe"
                } else {
                    word
                };
                if !explicit.contains(&word.to_lowercase()) {
                    add(word, name);
                    hotwords.push(name.to_owned());
                }
            }
        }
        // Known compound spelling variants, only activated by the user's explicit product name.
        if hotwords.iter().any(|s| s.eq_ignore_ascii_case("omascribe")) {
            for alias in [
                "OMA SCRIBE",
                "OMA SCRIB",
                "奥马SCRIB",
                "奥马的SCRIB",
                "奥马（OMA）的SCRIB",
                "奥马 (OMA) 的 SCRIB",
            ] {
                add(alias, "OmaScribe");
            }
        }
        for key in conflicts {
            names.remove(&key);
        }
        let mut aliases: Vec<_> = names.keys().collect();
        aliases.sort_by_key(|s| std::cmp::Reverse(s.len()));
        let matcher = (!aliases.is_empty()).then(|| {
            RegexBuilder::new(
                &aliases
                    .iter()
                    .map(|s| regex::escape(s))
                    .collect::<Vec<_>>()
                    .join("|"),
            )
            .case_insensitive(true)
            .build()
            .unwrap()
        });
        hotwords.retain(|s| {
            !s.is_empty()
                && s.chars().count() <= 80
                && s.chars().all(|c| c.is_alphanumeric() || c == ' ')
        });
        hotwords.sort();
        hotwords.dedup();
        hotwords.truncate(80);
        Self {
            matcher,
            names,
            hotwords,
        }
    }
    pub fn normalize(&self, text: &str) -> String {
        let Some(matcher) = &self.matcher else {
            return text.to_owned();
        };
        let mut result = String::new();
        let mut cursor = 0;
        for hit in matcher.find_iter(text) {
            let value = hit.as_str();
            let first = value.chars().next().unwrap();
            let last = value.chars().last().unwrap();
            if first.is_ascii_alphanumeric()
                && text[..hit.start()]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric())
                || last.is_ascii_alphanumeric()
                    && text[hit.end()..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphanumeric())
            {
                continue;
            }
            result.push_str(&text[cursor..hit.start()]);
            result.push_str(&self.names[&value.to_lowercase()]);
            cursor = hit.end();
        }
        result.push_str(&text[cursor..]);
        result
    }
}
impl Engine {
    pub fn terminology(&self, meeting: &str) -> Result<Terms> {
        let notes = self.store.lock().unwrap().notes(meeting)?;
        Ok(Terms::new(&notes, &self.library_entries()?))
    }
    /// Archive the original ASR revision. Human edits and protected minutes remain authoritative.
    pub fn correct_terminology(&self, meeting: &str) -> Result<usize> {
        let terms = self.terminology(meeting)?;
        let mut db = self.store.lock().unwrap();
        let mut m = db.meeting(meeting)?;
        let mut changed = 0;
        for s in db.segments(&m)? {
            // A human edit has revision > 1; do not replace it implicitly.
            let original: String = db
                .conn
                .query_row(
                    "SELECT json FROM segment_history WHERE id=?1 ORDER BY revision LIMIT 1",
                    [&s.id],
                    |r| r.get(0),
                )
                .unwrap_or_else(|_| serde_json::to_string(&s).unwrap());
            let original: crate::Segment = serde_json::from_str(&original)?;
            // Speaker-only revisions are safe, but human text edits are not.
            if s.text != original.text && terms.normalize(&original.text) != s.text {
                continue;
            }
            let text = terms.normalize(&s.text);
            if text != s.text {
                db.edit_segment(&s.id, &text, s.revision)?;
                changed += 1;
            }
        }
        if changed > 0 {
            m.notes_fingerprint.clear();
            db.save_meeting(&m)?;
            for mut s in db.suggestions(meeting)? {
                if s.status == "pending" {
                    s.text = terms.normalize(&s.text);
                    s.stale = true;
                    db.save_suggestion(&s)?;
                }
            }
        }
        drop(db);
        if changed > 0 {
            self.sync_meeting_files(meeting);
        }
        Ok(changed)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_are_bounded_and_require_explicit_context() {
        let entries = vec![Entry {
            kind: "term".into(),
            title: "OmaScribe".into(),
            aliases: "奥马（OMA）的SCRIB,奥马的SCRIB".into(),
            ..Default::default()
        }];
        let t = Terms::new(&[], &entries);
        assert_eq!(
            t.normalize("测试奥马（OMA）的SCRIB，OMASCRIBE未完成。"),
            "测试OmaScribe，OmaScribe未完成。"
        );
        assert_eq!(
            t.normalize("MYOMASCRIBER OS 预算3000"),
            "MYOMASCRIBER OS 预算3000"
        );
        assert_eq!(Terms::new(&[], &[]).normalize("奥马的SCRIB"), "奥马的SCRIB");
    }
    #[test]
    fn conflicting_aliases_are_not_guessed() {
        let entries = vec![
            Entry {
                kind: "term".into(),
                title: "Alpha".into(),
                aliases: "项目".into(),
                ..Default::default()
            },
            Entry {
                kind: "term".into(),
                title: "Beta".into(),
                aliases: "项目".into(),
                ..Default::default()
            },
        ];
        assert_eq!(
            Terms::new(&[], &entries).normalize("项目没有完成"),
            "项目没有完成"
        );
    }
    #[test]
    fn note_spelling_corrects_transcript_but_keeps_raw_revision_and_human_edits() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        let m = e.create_meeting("Term test").unwrap();
        let segment = crate::Segment {
            id: format!("{}:1:1", m.id),
            meeting_id: m.id.clone(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 3000,
            text: "正在测试奥马（OMA）的SCRIB，尚未完成。".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        e.store.lock().unwrap().save_segment(&segment).unwrap();
        e.save_note(&m.id, None, "测试omascribe的功能", "document", 0)
            .unwrap();
        let corrected = e.snapshot(&m.id).unwrap().segments[0].clone();
        assert_eq!(corrected.text, "正在测试OmaScribe，尚未完成。");
        assert_eq!(corrected.revision, 2);
        let raw: String = e
            .store
            .lock()
            .unwrap()
            .conn
            .query_row(
                "SELECT json FROM segment_history WHERE id=?1 AND revision=1",
                [&segment.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<crate::Segment>(&raw).unwrap().text,
            segment.text
        );
        e.edit_segment(&segment.id, "人工确认：奥马的SCRIB是错误示例", 2)
            .unwrap();
        assert_eq!(e.correct_terminology(&m.id).unwrap(), 0);
        assert!(e.snapshot(&m.id).unwrap().segments[0]
            .text
            .contains("错误示例"));
    }
}
