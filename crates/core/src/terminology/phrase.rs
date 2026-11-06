//! Spelling-only repair of complete marked names with two exact source anchors.
//! This does not grant a note phrase permission to appear in minutes or captions.
use super::{edit_distance, note_asr_marked_spans, UserNote};
use regex::{Regex, RegexBuilder};
use std::collections::{BTreeSet, HashSet};

#[derive(serde::Serialize)]
struct Candidate {
    spelling: String,
    words: Vec<String>,
}

#[derive(Default)]
pub(super) struct Corrections {
    candidates: Vec<Candidate>,
    // Includes every explicit glossary owner, not just the selected 80 entries.
    guards: Vec<String>,
    guard_matcher: Option<Regex>,
}

/// Do not authorize a substring of a Unicode word, identifier or compound.
pub(super) fn full_boundary(text: &str, start: usize, end: usize) -> bool {
    let word = |c: char| {
        static MARK: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        c.is_alphanumeric()
            || matches!(c, '_' | '-' | '\'' | '’')
            || !c.is_ascii()
                && MARK
                    .get_or_init(|| Regex::new(r"\p{M}").unwrap())
                    .is_match(c.encode_utf8(&mut [0; 4]))
    };
    let mut before = text[..start].chars().rev();
    let mut after = text[end..].chars();
    let left = before.next();
    let right = after.next();
    !(left.is_some_and(word)
        || right.is_some_and(word)
        || left == Some('.') && before.next().is_some_and(char::is_alphanumeric)
        || right == Some('.') && after.next().is_some_and(char::is_alphanumeric))
}

impl Corrections {
    pub(super) fn new(
        notes: &[UserNote],
        explicit: impl FnOnce() -> Option<HashSet<String>>,
        ambiguous: &BTreeSet<String>,
    ) -> Self {
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        for note in crate::notes::document_notes(notes) {
            if note.content.chars().take(200_001).count() > 200_000 {
                // An unread tail could contain a competing spelling.
                return Self::default();
            }
            for span in note_asr_marked_spans(&note.content) {
                let spelling = span.as_str();
                let words: Vec<_> = spelling.split(' ').filter(|w| !w.is_empty()).collect();
                if !(3..=6).contains(&words.len())
                    || words
                        .iter()
                        .any(|w| !w.bytes().all(|c| c.is_ascii_alphabetic()))
                    || !words.iter().any(|w| (8..=40).contains(&w.len()))
                {
                    continue;
                }
                // Scan the entire bounded document, including phrases beyond
                // the ASR hint cap. Truncating could hide a competing spelling.
                // Overflow abstains for this new corrector alone.
                if !seen.insert(spelling.to_owned()) {
                    continue;
                }
                if candidates.len() == crate::asr_hotwords::MAX_WORDS {
                    return Self::default();
                }
                candidates.push(Candidate {
                    spelling: spelling.to_owned(),
                    words: words.into_iter().map(str::to_ascii_lowercase).collect(),
                });
            }
        }
        if candidates.is_empty() {
            return Self::default();
        }
        // Most contexts have no eligible marked phrase. Do not repeat strong
        // note-word extraction or materialize its guard vocabulary for them.
        let Some(explicit) = explicit() else {
            return Self::default();
        };
        // Guards are precomputed once, never rebuilt for each live partial.
        // An oversized glossary or regex fails closed for phrase repair only.
        let mut guards = BTreeSet::new();
        let mut guard_bytes = 0usize;
        for guard in explicit.iter().chain(ambiguous) {
            if guard.is_empty() || guards.contains(guard) {
                continue;
            }
            guard_bytes = guard_bytes.saturating_add(guard.len());
            if guard_bytes > 16_000 {
                return Self::default();
            }
            guards.insert(guard.clone());
        }
        let guards: Vec<_> = guards.into_iter().collect();
        let guard_matcher = if guards.is_empty() {
            None
        } else {
            let mut alternatives: Vec<_> = guards.iter().collect();
            alternatives.sort_by_key(|s| std::cmp::Reverse(s.len()));
            // Put boundaries in the regex so a rejected long alternative can
            // fall back to a shorter valid alias at the same position. Dot
            // adjacency is conservatively guarded, even though a correction
            // itself cannot be part of a dotted identifier.
            let boundary = r"[^\p{L}\p{N}\p{M}_\-'’]";
            let body = alternatives
                .iter()
                .map(|s| regex::escape(s))
                .collect::<Vec<_>>()
                .join("|");
            let pattern = format!(r"(?:^|{boundary})(?P<guard>{body})(?:$|{boundary})");
            let Ok(matcher) = RegexBuilder::new(&pattern)
                .case_insensitive(true)
                .size_limit(1_000_000)
                .build()
            else {
                return Self::default();
            };
            Some(matcher)
        };
        let mut result = Self {
            candidates,
            guards,
            guard_matcher,
        };
        // Even an exact canonical glossary constituent retains its ownership.
        // Do not let a marked phrase repurpose it as a fuzzy source anchor.
        let matcher = &result.guard_matcher;
        result.candidates.retain(|candidate| {
            matcher
                .as_ref()
                .is_none_or(|m| !m.is_match(&candidate.spelling))
        });
        result
            .candidates
            .sort_by(|a, b| a.spelling.cmp(&b.spelling));
        if result.candidates.is_empty() {
            Self::default()
        } else {
            result
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    pub(super) fn fingerprint(&self) -> impl serde::Serialize + '_ {
        (&self.candidates, &self.guards)
    }

    pub(super) fn owns(&self, name: &str) -> bool {
        self.candidates
            .iter()
            .any(|candidate| candidate.spelling == name)
    }

    fn protected(&self, text: &str) -> Vec<std::ops::Range<usize>> {
        let Some(matcher) = &self.guard_matcher else {
            return Vec::new();
        };
        let mut ranges = Vec::new();
        let mut offset = 0;
        while let Some(captures) = matcher.captures_at(text, offset) {
            let hit = captures.name("guard").expect("guard capture");
            ranges.push(hit.range());
            // Keep the shared boundary available to the next alternative and
            // expose any valid shorter guard inside the preceding one.
            offset = hit.start() + hit.as_str().chars().next().unwrap().len_utf8();
        }
        merged_ranges(ranges)
    }

    pub(super) fn contains_canonical(&self, text: &str, name: &str) -> bool {
        let Some(candidate) = self.candidates.iter().find(|c| c.spelling == name) else {
            return false;
        };
        let protected = self.protected(text);
        let words = source_words(text);
        // Precompute competing exact names once. Repeating a shorter label in
        // a long source must not rescan the whole passage per witness.
        let mut opaque = Vec::new();
        for other in self.candidates.iter().filter(|c| c.spelling != name) {
            for span in words.windows(other.words.len()) {
                if exact_window(text, span, &other.words) {
                    opaque.push(span[0].start()..span[span.len() - 1].end());
                }
            }
        }
        let opaque = merged_ranges(opaque);
        words.windows(candidate.words.len()).any(|span| {
            let (start, end) = (span[0].start(), span[span.len() - 1].end());
            exact_window(text, span, &candidate.words)
                && !overlaps(&protected, start, end)
                && !overlaps(&opaque, start, end)
        })
    }

    pub(super) fn normalize_with(
        &self,
        text: &str,
        mut replaced: impl FnMut(&str, &str),
    ) -> String {
        if self.is_empty() {
            return text.to_owned();
        }
        let words = source_words(text);
        let protected = self.protected(text);
        // Exact names also enter this list as opaque windows. A different note
        // spelling must not replace an already exact, complete source name.
        let mut matches: Vec<(usize, usize, &Candidate, bool)> = Vec::new();
        for i in 0..words.len() {
            for candidate in &self.candidates {
                let count = candidate.words.len();
                let Some(span) = words.get(i..i + count) else {
                    continue;
                };
                let (start, end) = (span[0].start(), span[count - 1].end());
                if !whole_window(text, span) || overlaps(&protected, start, end) {
                    continue;
                }
                let mut mismatch = None;
                let mut valid = true;
                for (source, target) in span.iter().zip(&candidate.words) {
                    let source = source.as_str();
                    if source.eq_ignore_ascii_case(target) {
                        continue;
                    }
                    if mismatch.is_some()
                        || !(8..=40).contains(&target.len())
                        || source.len() < 7
                        || source.len().abs_diff(target.len()) > 2
                        || !source.bytes().all(|c| c.is_ascii_alphabetic())
                    {
                        valid = false;
                        break;
                    }
                    let source = source.to_ascii_lowercase();
                    let shared = source[..4] == target[..4]
                        || source[source.len() - 4..] == target[target.len() - 4..];
                    let distance = edit_distance(source.as_bytes(), target.as_bytes());
                    if !shared || distance > 2 || distance * 4 > target.len() {
                        valid = false;
                        break;
                    }
                    mismatch = Some(());
                }
                if valid {
                    matches.push((start, end, candidate, mismatch.is_none()));
                }
            }
        }
        // Any competing/overlapping window abstains; do not choose the first
        // candidate or the closest distance when both have source support.
        let mut out = String::new();
        let mut cursor = 0;
        let mut group = 0;
        while group < matches.len() {
            let mut next = group + 1;
            let mut end = matches[group].1;
            while next < matches.len() && matches[next].0 < end {
                end = end.max(matches[next].1);
                next += 1;
            }
            let (start, end, candidate, exact) = matches[group];
            if next == group + 1 && !exact {
                out.push_str(&text[cursor..start]);
                out.push_str(&candidate.spelling);
                replaced(&text[start..end], &candidate.spelling);
                cursor = end;
            }
            group = next;
        }
        out.push_str(&text[cursor..]);
        out
    }
}

fn source_words(text: &str) -> Vec<regex::Match<'_>> {
    static WORD: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    WORD.get_or_init(|| Regex::new(r"[A-Za-z0-9_]+").unwrap())
        .find_iter(text)
        .collect()
}

fn whole_window(text: &str, span: &[regex::Match<'_>]) -> bool {
    full_boundary(text, span[0].start(), span[span.len() - 1].end())
        && !span.windows(2).any(|w| {
            let gap = &text[w[0].end()..w[1].start()];
            gap.is_empty() || gap.len() > 3 || !gap.bytes().all(|b| b == b' ')
        })
}

fn exact_window(text: &str, span: &[regex::Match<'_>], target: &[String]) -> bool {
    whole_window(text, span)
        && span
            .iter()
            .zip(target)
            .all(|(word, target)| word.as_str().eq_ignore_ascii_case(target))
}

fn merged_ranges(mut ranges: Vec<std::ops::Range<usize>>) -> Vec<std::ops::Range<usize>> {
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut().filter(|last| range.start <= last.end) {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

fn overlaps(ranges: &[std::ops::Range<usize>], start: usize, end: usize) -> bool {
    ranges
        .get(ranges.partition_point(|range| range.end <= start))
        .is_some_and(|range| range.start < end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminology::Terms;

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

    fn context(content: &str) -> Terms {
        Terms::new(&[note(content)], &[])
    }

    const NOTES: &str = "**Subtropical Storm Jerry**\n**Sundarbans National Park**";

    #[test]
    fn ordinary_context_does_not_materialize_phrase_guards() {
        for content in [
            "Normal meeting notes.",
            "**Budget review**",
            "**Chandra Shekhar Solanki**",
        ] {
            let result = Corrections::new(
                &[note(content)],
                || panic!("unneeded name scan"),
                &BTreeSet::new(),
            );
            assert!(result.is_empty());
        }
        let calls = std::cell::Cell::new(0);
        let result = Corrections::new(
            &[note(NOTES)],
            || {
                calls.set(calls.get() + 1);
                Some(HashSet::new())
            },
            &BTreeSet::new(),
        );
        assert!(!result.is_empty());
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn repairs_two_frozen_names_with_complete_source_replacements_only() {
        let terms = context(NOTES);
        let input = "SUBTROPIC STORM JERRY is pending; SUNDARBONS NATIONAL PARK remains closed.";
        let expected =
            "Subtropical Storm Jerry is pending; Sundarbans National Park remains closed.";
        let (text, edits) = terms.normalize_with_changes(input);
        assert_eq!(text, expected);
        assert_eq!(terms.normalize(input), expected);
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].old, "SUBTROPIC STORM JERRY");
        assert_eq!(edits[0].new, "Subtropical Storm Jerry");
        assert_eq!(edits[1].old, "SUNDARBONS NATIONAL PARK");
        assert!(edits.iter().all(|e| e.phrase && !e.model));
        assert_ne!(terms.fingerprint(), Terms::new(&[], &[]).fingerprint());
        assert!(terms.writer_names.is_empty());
        assert!(terms.caption_names.is_empty());
        assert_eq!(terms.normalize(&text), text);
        for input in [
            "Subtropical Storm Jerry is pending.",
            "SUBTROPICAL STORM JERRY is pending.",
            "SUNDARBANS NATIONAL PARK remains closed.",
            "The report is pending.",
            "SUNDARBONS is closed.",
        ] {
            assert_eq!(terms.normalize(input), input);
        }
        let generic = context("**Budget review**");
        assert!(generic
            .validate_writer_names("The Budget review is not approved.", "预算审查未获批准。")
            .is_ok());
    }

    #[test]
    fn complete_boundaries_and_short_or_unanchored_names_abstain() {
        let terms = context(NOTES);
        for input in [
            "éSUBTROPIC STORM JERRY",
            "e\u{301}SUBTROPIC STORM JERRY",
            "SUBTROPIC STORM JERRY\u{301}",
            "SUBTROPIC STORM JERRY-Plus",
            "SUBTROPIC STORM JERRY's",
            "SUBTROPIC STORM JERRY.Name",
            "Name.SUBTROPIC STORM JERRY",
            "SUBTROPIC\nSTORM JERRY",
            "SUBTROPIC, STORM JERRY",
            "SUBTROPIC-STORM JERRY",
            "SUBTROPIC\tSTORM JERRY",
            "SUBTROPIC STORMS JERRY",
            "SUNDARBONS NATIONAL PARKS",
        ] {
            assert_eq!(terms.normalize(input), input, "{input}");
        }
        for (note, input) in [
            ("**Atlantic Ocean**", "ALANIC OCEAN"),
            ("**Erik the Red**", "ERIC THE RED"),
            ("**Lord Byron**", "LORD BRIAN"),
            ("**Easter Vigil**", "EASTER VIGILANCE"),
            ("**USA Gymnastics**", "USA JIMASTICS"),
        ] {
            assert_eq!(context(note).normalize(input), input, "{input}");
        }
    }

    #[test]
    fn ambiguous_or_overlapping_phrases_do_not_pick_a_winner() {
        for note in [
            "**Sundarbans National Park**\n**Sunderbons National Park**",
            "**Sundarbans National Park**\n**SUNDARBANS NATIONAL PARK**",
            "**Sundarbans National Park**\n**Sundarbons National Park**",
        ] {
            let terms = context(note);
            assert!(
                terms.names.is_empty(),
                "phrase parts cannot own aliases: {note}"
            );
            assert!(
                terms.approximate.is_empty(),
                "phrase parts cannot fuzzy-match alone: {note}"
            );
            assert_eq!(
                terms.normalize("SUNDARBONS NATIONAL PARK"),
                "SUNDARBONS NATIONAL PARK",
                "{note}"
            );
        }
        let terms = context("**Subtropical Storm Jerry**\n**Subtropical Storm Jerry Observatory**");
        assert!(!terms.phrases.contains_canonical(
            "Subtropical Storm Jerry Observatory",
            "Subtropical Storm Jerry"
        ));
        assert!(terms.phrases.contains_canonical(
            "Subtropical Storm Jerry is pending.",
            "Subtropical Storm Jerry"
        ));
        assert!(terms.phrases.contains_canonical(
            "SUBTROPICAL STORM JERRY is pending.",
            "Subtropical Storm Jerry"
        ));
        assert_eq!(
            terms.normalize("SUBTROPIC STORM JERRY OBSERVATORY"),
            "SUBTROPIC STORM JERRY OBSERVATORY"
        );
    }

    #[test]
    fn actual_declared_names_keep_ownership_without_guarding_weak_note_words() {
        let known = context("**Subtropical Storm Jerry**\n\n**Subtropic**");
        assert_eq!(
            known.normalize("SUBTROPIC STORM JERRY"),
            "Subtropic STORM JERRY"
        );
        let mixed = context("今天讨论 **Subtropical Storm Jerry**");
        let (text, edits) = mixed.normalize_with_changes("SUBTROPIC STORM JERRY remains pending.");
        assert_eq!(text, "Subtropical Storm Jerry remains pending.");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].old, "SUBTROPIC STORM JERRY");
        assert!(edits[0].phrase);
    }

    #[test]
    fn guards_include_unselected_glossary_and_overlapping_aliases() {
        let notes = [note("**Subtropical XFoo Bar**")];
        let guarded = Corrections::new(
            &notes,
            || Some(HashSet::from(["foo bar".into(), "bar".into()])),
            &BTreeSet::new(),
        );
        assert!(guarded.is_empty());
        let notes = [note("**Subtropical Storm Jerry**")];
        let guarded = Corrections::new(
            &notes,
            || Some(HashSet::from(["subtropic storm".into(), "storm".into()])),
            &BTreeSet::new(),
        );
        assert!(guarded.is_empty());
        let guarded = Corrections::new(
            &notes,
            || Some(HashSet::from(["subtropic".into()])),
            &BTreeSet::new(),
        );
        assert_eq!(
            guarded.normalize_with("SUBTROPIC STORM JERRY", |_, _| panic!("guard")),
            "SUBTROPIC STORM JERRY"
        );
        let mut entries: Vec<_> = (0..80)
            .map(|i| crate::library::Entry {
                kind: "term".into(),
                title: format!("Existing{i}"),
                ..Default::default()
            })
            .collect();
        entries.push(crate::library::Entry {
            kind: "term".into(),
            title: "Subtropic".into(),
            ..Default::default()
        });
        let terms = Terms::new(&notes, &entries);
        assert_eq!(
            terms
                .phrases
                .normalize_with("SUBTROPIC STORM JERRY", |_, _| panic!("unselected guard")),
            "SUBTROPIC STORM JERRY"
        );
        let named = (0..80)
            .map(|i| {
                format!(
                    "**Marker{}{}**",
                    (b'a' + i / 26) as char,
                    (b'a' + i % 26) as char
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let truncated = context(&format!(
            "**Subtropical Storm Jerry**\n{named}\n**Subtropic**"
        ));
        assert!(truncated.phrases.is_empty());
        assert_eq!(
            truncated.normalize("SUBTROPIC STORM JERRY"),
            "SUBTROPIC STORM JERRY"
        );
    }

    #[test]
    fn capacity_never_hides_a_competing_phrase() {
        let filler = (0..79)
            .map(|i| {
                format!(
                    "**Longname{}{} Route Alpha**",
                    (b'a' + i / 26) as char,
                    (b'a' + i % 26) as char
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let content = format!("**Sundarbans National Park**\n{filler}");
        assert_eq!(
            context(&content).normalize("SUNDARBONS NATIONAL PARK"),
            "Sundarbans National Park"
        );
        let overflow = context(&format!("{content}\n**Sunderbons National Park**"));
        assert!(overflow.phrases.is_empty());
        assert_eq!(
            overflow.normalize("SUNDARBONS NATIONAL PARK"),
            "SUNDARBONS NATIONAL PARK"
        );
        let duplicates = context(&"**Sundarbans National Park**\n".repeat(100));
        assert_eq!(
            duplicates.normalize("SUNDARBONS NATIONAL PARK"),
            "Sundarbans National Park"
        );
        assert_eq!(
            duplicates.fingerprint(),
            context("**Sundarbans National Park**").fingerprint()
        );
    }
}
