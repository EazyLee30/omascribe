//! Local spelling context: explicit aliases and unambiguous near-spellings of long names.
use crate::{library::Entry, Engine, UserNote};
use anyhow::Result;
use regex::{Regex, RegexBuilder};
use rusqlite::OptionalExtension;
use std::collections::{BTreeMap, HashSet};

pub struct Terms {
    matcher: Option<Regex>,
    names: BTreeMap<String, String>,
    pub hotwords: Vec<String>,
    writer_names: Vec<String>,
    approximate: Vec<String>,
}
impl Terms {
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&("terms-v2", &self.names, &self.approximate))
                    .expect("term signature")
            )
        )
    }
    pub fn new(notes: &[UserNote], entries: &[Entry]) -> Self {
        let mut names = BTreeMap::new();
        let mut conflicts = HashSet::new();
        let mut hotwords = Vec::new();
        let mut writer_names = Vec::new();
        let mut approximate = Vec::new();
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
            writer_names.push(e.title.trim().to_owned());
            approximate.push(e.title.trim().to_owned());
            add(&e.title, &e.title);
            for alias in e.aliases.split([',', '，', ';', '；', '\n', '|']) {
                add(alias, &e.title);
            }
        }
        // User-written Latin names supply spelling/casing, never facts or inferred aliases.
        let latin = Regex::new(r"[A-Za-z][A-Za-z0-9_+.-]{2,39}").unwrap();
        for n in crate::notes::document_notes(notes) {
            let chinese_context = n
                .content
                .chars()
                .any(|c| ('\u{3400}'..='\u{9fff}').contains(&c));
            for word in latin.find_iter(&n.content).take(80) {
                let word = word.as_str().trim_end_matches(['.', '-']);
                if ["nbsp", "amp", "http", "https"].contains(&word.to_lowercase().as_str()) {
                    continue;
                }
                // A standalone keyword line is an explicit spelling hint even
                // when typed in lowercase (for example gtx1650 or nvidia).
                let keyword_line = n
                    .content
                    .lines()
                    .any(|line| line.trim().trim_matches(['*', '-', '#', ' ']) == word);
                let named = keyword_line
                    || word.chars().skip(1).any(|c| c.is_ascii_uppercase())
                    || word.eq_ignore_ascii_case("omascribe")
                    || word.starts_with(char::is_uppercase)
                        && n.content.contains(&format!("**{word}**"));
                if n.kind != "term" && !chinese_context && !named {
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
                    // Mixed-language notes also supply ordinary ASR words.
                    // Only names explicitly marked as such constrain minutes;
                    // ordinary words must remain free to translate.
                    if named || n.kind == "term" {
                        writer_names.push(name.to_owned());
                    }
                    approximate.push(name.to_owned());
                }
            }
        }
        // Known compound spelling variants, only activated by the user's explicit product name.
        if hotwords.iter().any(|s| s.eq_ignore_ascii_case("omascribe")) {
            for alias in [
                "OMA SCRIBE",
                "OMA SCRIB",
                "奥马SCRIB",
                "奥马 SCRIB",
                "奥马斯",
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
        writer_names.retain(|s| hotwords.contains(s) && names.get(&s.to_lowercase()) == Some(s));
        writer_names.sort();
        writer_names.dedup();
        approximate.retain(|s| {
            (8..=40).contains(&s.len())
                && s.chars().all(|c| c.is_ascii_alphabetic())
                && names.get(&s.to_lowercase()) == Some(s)
        });
        approximate.sort();
        approximate.dedup();
        approximate.truncate(80);
        Self {
            matcher,
            names,
            hotwords,
            writer_names,
            approximate,
        }
    }
    pub fn normalize(&self, text: &str) -> String {
        self.normalize_near_names(&self.normalize_exact(text))
    }
    fn normalize_exact(&self, text: &str) -> String {
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
    fn normalize_near_names(&self, text: &str) -> String {
        if self.approximate.is_empty() {
            return text.to_owned();
        }
        static WORD: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let words: Vec<_> = WORD
            .get_or_init(|| Regex::new(r"[A-Za-z0-9_]+").unwrap())
            .find_iter(text)
            .collect();
        let mut out = String::new();
        let (mut cursor, mut i) = (0, 0);
        while i < words.len() {
            let mut matched = None;
            for count in (1..=3.min(words.len() - i)).rev() {
                let span = &words[i..i + count];
                if count > 1 && (span[0].len() < 3 || span[count - 1].len() < 3) {
                    continue;
                }
                if span.windows(2).any(|w| {
                    let gap = &text[w[0].end()..w[1].start()];
                    gap.is_empty() || gap.len() > 3 || !gap.chars().all(|c| c == ' ' || c == '-')
                }) {
                    continue;
                }
                let compact = span
                    .iter()
                    .map(|w| w.as_str())
                    .collect::<String>()
                    .to_ascii_lowercase();
                if compact.len() < 7 || !compact.bytes().all(|c| c.is_ascii_alphabetic()) {
                    continue;
                }
                // Never turn an already recognized *different* name into a nearby name.
                if let Some(name) = self.names.get(&compact) {
                    if count > 1 {
                        matched = Some((count, name));
                        break;
                    }
                    continue;
                }
                if count > 1
                    && span.iter().any(|word| {
                        let key = word.as_str().to_lowercase();
                        self.names
                            .get(&key)
                            .is_some_and(|name| name.to_lowercase() == key)
                    })
                {
                    continue;
                }
                let mut candidates: Vec<_> = self
                    .approximate
                    .iter()
                    .filter_map(|name| {
                        let key = name.to_ascii_lowercase();
                        if key.len().abs_diff(compact.len()) > 2 {
                            return None;
                        }
                        let shared = key.starts_with(&compact[..4])
                            || key.ends_with(&compact[compact.len() - 4..]);
                        let distance = edit_distance(compact.as_bytes(), key.as_bytes());
                        (shared && distance <= 2 && distance * 4 <= key.len())
                            .then_some((distance, name))
                    })
                    .collect();
                candidates.sort_by_key(|(distance, _)| *distance);
                if let Some((distance, name)) = candidates.first() {
                    if candidates.get(1).is_some_and(|(next, _)| next <= distance) {
                        continue;
                    }
                    matched = Some((count, *name));
                    break;
                }
            }
            if let Some((count, name)) = matched {
                out.push_str(&text[cursor..words[i].start()]);
                out.push_str(name);
                cursor = words[i + count - 1].end();
                i += count;
            } else {
                i += 1;
            }
        }
        out.push_str(&text[cursor..]);
        out
    }
    /// A small spelling-only vocabulary survives context trimming and repair attempts.
    pub fn spellings(&self, transcript: &str) -> Vec<String> {
        let normalized = self.normalize(transcript).to_lowercase();
        let mut names = self.writer_names.clone();
        names.extend(source_owner_names(transcript));
        // Unspoken note words must not become invented meeting subjects.
        names.retain(|n| contains_name(&normalized, &n.to_lowercase()));
        names.sort();
        names.dedup();
        names.sort_by_key(|n| n.len());
        let mut budget = 240;
        names
            .into_iter()
            .filter(|n| {
                let size = n.chars().count();
                if size > budget {
                    return false;
                }
                budget -= size;
                true
            })
            .take(12)
            .collect()
    }
    pub fn validate_writer_names(&self, transcript: &str, body: &str) -> Result<()> {
        let body = body.to_lowercase();
        let owners = source_owner_names(transcript);
        // A sole explicit owner is a source spelling, not a guessed translation.
        // Multiple/changed ownership needs field identity; do not demand obsolete
        // owners merely because their names occurred in an earlier passage.
        static CHANGED: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let changed = CHANGED.get_or_init(|| Regex::new(r"(?i)no longer|not responsible|changed?|corrected|reassigned|不再负责|负责人.{0,12}(?:改|换|变)").unwrap());
        let sole_owner = owners.len() == 1 && !changed.is_match(transcript);
        let missing: Vec<_> = self
            .spellings(transcript)
            .into_iter()
            .filter(|name| self.writer_names.contains(name) || sole_owner && owners.contains(name))
            .filter(|name| !contains_name(&body, &name.to_lowercase()))
            .collect();
        if !missing.is_empty() {
            anyhow::bail!("纪要遗漏原话中的专有名称：{}。保留这些名称和对应原话的状态，不新增事实。Preserve these source names and their stated status.", missing.join(", "));
        }
        Ok(())
    }
    pub fn normalize_draft(&self, draft: &str) -> Result<String> {
        let mut value: serde_json::Value = serde_json::from_str(draft)?;
        if let Some(sections) = value["sections"].as_array_mut() {
            for section in sections {
                for field in ["title", "body"] {
                    if let Some(text) = section[field].as_str() {
                        section[field] = self.normalize(text).into();
                    }
                }
            }
        }
        Ok(serde_json::to_string(&value)?)
    }
}
/// Explicit ownership subjects only. Ordinary capitalized technical prose is
/// translatable; this does not create aliases or alter the original transcript.
fn source_owner_names(text: &str) -> Vec<String> {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re=RE.get_or_init(||Regex::new(r"\b(?P<name>[A-Z][A-Za-z'’-]{1,39}(?:\s+[A-Z][A-Za-z'’-]{1,39}){0,2})\s+(?:owns\b|is responsible for\b)").unwrap());
    let mut names = Vec::new();
    for c in re.captures_iter(text) {
        let name = &c["name"];
        if [
            "We",
            "You",
            "They",
            "He",
            "She",
            "It",
            "Someone",
            "Anyone",
            "This",
            "That",
            "Team",
            "The Team",
            "Nobody",
            "Everybody",
            "Management",
        ]
        .contains(&name)
        {
            continue;
        }
        if !names.iter().any(|n| n == name) {
            names.push(name.to_owned());
        }
        if names.len() == 12 {
            break;
        }
    }
    names
}
fn contains_name(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(start, value)| {
        !text[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric())
            && !text[start + value.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric())
    })
}
fn edit_distance(a: &[u8], b: &[u8]) -> usize {
    let mut row: Vec<_> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let old = row[j + 1];
            row[j + 1] = (previous + usize::from(x != y))
                .min(row[j] + 1)
                .min(old + 1);
            previous = old;
        }
    }
    row[b.len()]
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
        let terms_key = terms.fingerprint();
        db.schedule_terminology(&m, &terms_key)?;
        loop {
            let candidates = db.terminology_candidates(&m)?;
            if candidates.is_empty() {
                break;
            }
            let mut checked = Vec::with_capacity(candidates.len());
            for mut s in candidates {
                let origin: Option<String> = db.conn.query_row(
                "SELECT origin FROM segment_text_edits WHERE id=?1 ORDER BY revision DESC LIMIT 1",
                [&s.id], |r| r.get(0),
            ).optional()?;
                if origin.as_deref() == Some("human") {
                    checked.push(s);
                    continue;
                }
                let original: Option<String> = db
                    .conn
                    .query_row(
                        "SELECT json FROM segment_history WHERE id=?1 ORDER BY revision LIMIT 1",
                        [&s.id],
                        |r| r.get(0),
                    )
                    .optional()?;
                let original: crate::Segment = original
                    .map(|s| serde_json::from_str(&s))
                    .transpose()?
                    .unwrap_or_else(|| s.clone());
                // Legacy revisions lack provenance. Keep unknown edits; speaker-only changes and
                // old exact-alias corrections can safely migrate. New edits carry explicit origin.
                if origin.is_none()
                    && s.text != original.text
                    && terms.normalize_exact(&original.text) != s.text
                {
                    checked.push(s);
                    continue;
                }
                let text = terms.normalize(&original.text);
                if text != s.text {
                    db.edit_segment_text(&s.id, &text, s.revision, "terminology")?;
                    s.revision += 1;
                    changed += 1;
                }
                checked.push(s);
            }
            db.mark_terminology_batch(&checked, &terms_key)?;
        }
        db.set_cache_dependency(meeting, "terms", &terms_key)?;
        if changed > 0 {
            m.notes_fingerprint.clear();
            db.save_meeting(&m)?;
            for mut s in db.suggestions(meeting)? {
                if s.status == "pending" {
                    // Regenerate against corrected, revisioned evidence; never patch prose
                    // while leaving its citations attached to the old words.
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
    fn explicit_source_owner_keeps_spelling_without_guessing_transliterations_or_old_owners() {
        let terms = Terms::new(&[], &[]);
        let source="The offline recorder is ready for testing. Morgan owns the latency report and will deliver it on Thursday.";
        assert_eq!(terms.spellings(source), ["Morgan"]);
        assert!(terms
            .validate_writer_names(source, "Morgan 周四交付延迟报告。")
            .is_ok());
        assert!(terms
            .validate_writer_names(source, "莫根周四交付延迟报告。")
            .is_err());
        assert_eq!(terms.normalize(source), source);
        assert!(terms
            .spellings("The offline prototype is ready for review. We will test it on Friday.")
            .is_empty());
        let changed =
            "Morgan owns the latency report. Ownership changed: Riley owns the latency report.";
        assert!(terms
            .validate_writer_names(changed, "Riley 周四交付延迟报告。")
            .is_ok());
    }
    #[test]
    fn unchanged_terminology_only_rechecks_new_or_revised_passages() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let meeting = engine.create_meeting("Incremental terminology")?;
        for seq in 1..=300 {
            engine.store.lock().unwrap().save_segment(&crate::Segment {
                id: format!("source-{seq}"),
                meeting_id: meeting.id.clone(),
                run: 1,
                seq,
                start_ms: u64::from(seq) * 1000,
                end_ms: u64::from(seq + 1) * 1000,
                text: "预算尚未确定。".into(),
                revision: 1,
                speaker: None,
                speaker_manual: false,
            })?;
        }
        assert_eq!(
            engine
                .store
                .lock()
                .unwrap()
                .terminology_candidates(&meeting)?
                .len(),
            128
        );
        engine.correct_terminology(&meeting.id)?;
        assert!(engine
            .store
            .lock()
            .unwrap()
            .terminology_candidates(&meeting)?
            .is_empty());
        // Late insertion before the previous maximum seq must not be skipped by a cursor.
        let source = crate::Segment {
            id: "late-source".into(),
            meeting_id: meeting.id.clone(),
            run: 1,
            seq: 0,
            start_ms: 0,
            end_ms: 1000,
            text: "尚未批准。".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        engine.store.lock().unwrap().save_segment(&source)?;
        assert_eq!(
            engine
                .store
                .lock()
                .unwrap()
                .terminology_candidates(&meeting)?
                .len(),
            1
        );
        engine.correct_terminology(&meeting.id)?;
        engine
            .store
            .lock()
            .unwrap()
            .edit_segment("late-source", "人工文字", 1)?;
        assert_eq!(
            engine
                .store
                .lock()
                .unwrap()
                .terminology_candidates(&meeting)?
                .len(),
            1
        );
        engine.correct_terminology(&meeting.id)?;
        assert!(engine
            .store
            .lock()
            .unwrap()
            .terminology_candidates(&meeting)?
            .is_empty());
        assert_eq!(engine.snapshot(&meeting.id)?.segments[0].text, "人工文字");
        drop(engine);
        let reopened = Engine::open(root.path().into())?;
        assert!(reopened
            .store
            .lock()
            .unwrap()
            .terminology_candidates(&meeting)?
            .is_empty());
        Ok(())
    }
    #[test]
    fn disabling_a_term_reverts_only_automatic_corrections() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let meeting = engine.create_meeting("Term disable")?;
        for seq in 1..=2 {
            engine.store.lock().unwrap().save_segment(&crate::Segment {
                id: format!("source-{seq}"),
                meeting_id: meeting.id.clone(),
                run: 1,
                seq,
                start_ms: 0,
                end_ms: 1000,
                text: "Project zargle is pending".into(),
                revision: 1,
                speaker: None,
                speaker_manual: false,
            })?;
        }
        let mut entry = engine.save_library_entry(crate::library::Entry {
            kind: "term".into(),
            title: "Zargle".into(),
            aliases: "zargle".into(),
            ..Default::default()
        })?;
        let source = engine.snapshot(&meeting.id)?.segments[0].clone();
        assert_eq!(source.text, "Project Zargle is pending");
        engine.edit_segment(&source.id, "Human wording: Zargle", source.revision)?;
        let before = engine
            .store
            .lock()
            .unwrap()
            .cache_dependency(&meeting.id, "terms")?;
        entry.enabled = false;
        engine.save_library_entry(entry)?;
        let snapshot = engine.snapshot(&meeting.id)?;
        assert_eq!(snapshot.segments[0].text, "Human wording: Zargle");
        assert_eq!(snapshot.segments[1].text, "Project zargle is pending");
        assert_ne!(
            before,
            engine
                .store
                .lock()
                .unwrap()
                .cache_dependency(&meeting.id, "terms")?
        );
        Ok(())
    }
    #[test]
    fn hiding_and_restoring_linked_notes_updates_spelling_context() -> Result<()> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let meeting = engine.create_meeting("Linked spelling")?;
        engine.store.lock().unwrap().save_segment(&crate::Segment {
            id: "linked-source".into(),
            meeting_id: meeting.id.clone(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 1000,
            text: "测试 OMA SCRIBE".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        })?;
        let note = engine.save_note(&meeting.id, None, "测试omascribe", "document", 0)?;
        assert_eq!(
            engine.snapshot(&meeting.id)?.segments[0].text,
            "测试 OmaScribe"
        );
        let mut entry = engine
            .library_entries()?
            .into_iter()
            .find(|e| e.id == note.id)
            .unwrap();
        entry.deleted = true;
        engine.save_library_entry(entry.clone())?;
        assert_eq!(
            engine.snapshot(&meeting.id)?.segments[0].text,
            "测试 OMA SCRIBE"
        );
        entry.deleted = false;
        engine.save_library_entry(entry)?;
        assert_eq!(
            engine.snapshot(&meeting.id)?.segments[0].text,
            "测试 OmaScribe"
        );
        Ok(())
    }
    fn note(content: &str) -> UserNote {
        UserNote {
            id: "n".into(),
            meeting_id: "m".into(),
            kind: "document".into(),
            content: content.into(),
            anchor_ms: 0,
            updated_at: String::new(),
        }
    }
    #[test]
    fn note_names_handle_split_asr_spelling_without_consuming_other_words() {
        let terms = Terms::new(&[note("测试omascribe和PixelLedger的功能")], &[]);
        for (input, expected) in [
            ("这个 ALMA SCRIBE的功能", "这个 OmaScribe的功能"),
            ("I use OMA SCRIBE today", "I use OmaScribe today"),
            ("测试奥马 SCRIB的一些功能", "测试OmaScribe的一些功能"),
            ("左侧已经写了奥马斯", "左侧已经写了OmaScribe"),
            (
                "测试 PIXEL LEDGER 和 PIXEL LEDGRE",
                "测试 PixelLedger 和 PixelLedger",
            ),
            ("OMASCRIBE", "OmaScribe"),
        ] {
            assert_eq!(terms.normalize(input), expected, "{input}");
        }
        for input in [
            "OS, OAS, SCRIBE",
            "MYOMASCRIBER",
            "ALMOST RIB RIGHT NOW",
            "预算3000，奥斯的方案",
            "I transcribe interviews",
        ] {
            assert_eq!(terms.normalize(input), input);
        }
        assert_eq!(Terms::new(&[], &[]).normalize("ALMA SCRIBE"), "ALMA SCRIBE");
        assert_eq!(
            Terms::new(&[note("We need transcription improvements")], &[])
                .normalize("TRANSCRIPTON"),
            "TRANSCRIPTON"
        );
    }
    #[test]
    fn competing_names_are_not_merged() {
        let terms = Terms::new(&[note("测试OmaScribe和OpaScribe")], &[]);
        assert_eq!(terms.normalize("ONA SCRIBE"), "ONA SCRIBE");
        assert_eq!(terms.normalize("OpaScribe"), "OpaScribe");
        let terms = Terms::new(&[note("测试OmaScribe和AlmaScribe")], &[]);
        assert_eq!(terms.normalize("ALMA SCRIBE"), "AlmaScribe");
        let terms = Terms::new(
            &[note("测试OmaScribe")],
            &[Entry {
                kind: "term".into(),
                title: "Alma".into(),
                ..Default::default()
            }],
        );
        assert_eq!(terms.normalize("Alma SCRIBE"), "Alma SCRIBE");
        let terms = Terms::new(
            &[note("测试OmaScribe")],
            &[Entry {
                kind: "term".into(),
                title: "奥马斯".into(),
                ..Default::default()
            }],
        );
        assert_eq!(terms.normalize("奥马斯的方案"), "奥马斯的方案");
    }
    #[test]
    fn draft_normalization_keeps_source_ids_and_only_changes_prose() {
        let terms = Terms::new(&[note("测试omascribe")], &[]);
        let value: serde_json::Value = serde_json::from_str(&terms.normalize_draft(
            r#"{"sections":[{"title":"ALMA SCRIBE","body":"ALMA SCRIBE尚未完成。","sources":[1,3]}]}"#
        ).unwrap()).unwrap();
        assert_eq!(value["sections"][0]["body"], "OmaScribe尚未完成。");
        assert_eq!(value["sections"][0]["sources"], serde_json::json!([1, 3]));
    }
    #[test]
    fn automatic_edits_can_follow_new_aliases_but_human_edits_remain_locked() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        let m = e.create_meeting("Term revisions").unwrap();
        let s = crate::Segment {
            id: format!("{}:1:1", m.id),
            meeting_id: m.id.clone(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 3000,
            text: "ALMA SCRIBE尚未修复".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        e.store.lock().unwrap().save_segment(&s).unwrap();
        e.save_note(&m.id, None, "测试omascribe", "document", 0)
            .unwrap();
        assert_eq!(
            e.snapshot(&m.id).unwrap().segments[0].text,
            "OmaScribe尚未修复"
        );
        let mut term = e
            .save_library_entry(Entry {
                kind: "term".into(),
                title: "OmaScribe Studio".into(),
                aliases: "ALMA SCRIBE".into(),
                ..Default::default()
            })
            .unwrap();
        let revised = e.snapshot(&m.id).unwrap().segments[0].clone();
        assert_eq!(revised.text, "OmaScribe Studio尚未修复");
        assert_eq!(revised.revision, 3);
        e.edit_segment(&s.id, "手动确认原话为 ALMA SCRIBE", 3)
            .unwrap();
        term.title = "OmaScribe Desktop".into();
        e.save_library_entry(term).unwrap();
        assert_eq!(
            e.snapshot(&m.id).unwrap().segments[0].text,
            "手动确认原话为 ALMA SCRIBE"
        );
        let original: String = e
            .store
            .lock()
            .unwrap()
            .conn
            .query_row(
                "SELECT json FROM segment_history WHERE id=?1 AND revision=1",
                [&s.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<crate::Segment>(&original)
                .unwrap()
                .text,
            s.text
        );
    }
    #[test]
    fn lowercase_keyword_lines_guide_names_without_inventing_missing_subjects() {
        let terms = Terms::new(&[note("omascribe\n\ngtx1650\n\nnvidia")], &[]);
        assert_eq!(terms.hotwords, ["OmaScribe", "gtx1650", "nvidia"]);
        assert!(Terms::new(&[note("we can review the budget")], &[])
            .hotwords
            .is_empty());
        assert!(terms
            .spellings("今天只讨论麦克风权限，预算未确定。")
            .is_empty());
        assert!(terms.spellings("MYOMASCRIBER").is_empty());
        assert!(terms
            .validate_writer_names("OmaScribe 尚未修复。", "功能尚未修复。")
            .is_err());
        assert!(terms
            .validate_writer_names("OmaScribe 尚未修复。", "OmaScribe 功能尚未修复。")
            .is_ok());
        assert!(terms
            .validate_writer_names("M二的功能尚未运行。", "功能尚未运行。")
            .is_ok());
    }
    #[test]
    fn mixed_language_asr_words_may_translate_without_becoming_required_names() {
        let terms = Terms::new(
            &[note(
                "**录音** **offline transcription** **系统音频** OmaScribe",
            )],
            &[],
        );
        let source = "OmaScribe offline transcription is ready for review.";
        assert!(terms.hotwords.contains(&"offline".to_owned()));
        assert_eq!(terms.spellings(source), ["OmaScribe"]);
        assert!(terms
            .validate_writer_names(source, "OmaScribe 离线转写原型已可评审。")
            .is_ok());
        assert!(terms
            .validate_writer_names(source, "离线转写原型已可评审。")
            .is_err());
    }
    #[test]
    fn writer_receives_note_spelling_without_opt_in_knowledge_or_memory() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path().into()).unwrap();
        let context = e
            .writer_context(&[note("测试omascribe的功能 &nbsp;")], "ALMA SCRIBE 的功能")
            .unwrap();
        assert_eq!(context["spellings"], serde_json::json!(["OmaScribe"]));
        assert_eq!(context["background_notes"], serde_json::json!([]));
        assert_eq!(context["writing_preferences"], serde_json::json!([]));
        let english = e
            .writer_context(&[note("Review PixelLedger.")], "PIXEL LEDGRE fails")
            .unwrap();
        assert_eq!(english["spellings"], serde_json::json!(["PixelLedger"]));
        assert_eq!(
            Terms::new(&[note("Review PixelLedger")], &[]).normalize("under review"),
            "under review"
        );
    }
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
