//! Bounded lexical RAG: short Chinese terms, bilingual BM25, chronology, and source closure.
use crate::{Segment, Snapshot, UserNote};
use std::collections::{HashMap, HashSet};
/// Once a Markdown document exists, legacy cards are retained as backup, not duplicated as context.
pub fn document_notes(notes: &[UserNote]) -> Vec<&UserNote> {
    if let Some(document) = notes.iter().find(|n| n.kind == "document") {
        vec![document]
    } else {
        notes.iter().collect()
    }
}
pub fn focus_terms(notes: &[UserNote]) -> Vec<String> {
    let mut result = Vec::new();
    for note in document_notes(notes) {
        if note.kind == "opinion" {
            continue;
        }
        // Explicit bold keywords take precedence. Remaining short phrases are useful Chinese queries.
        let bold: Vec<&str> = note
            .content
            .split("**")
            .enumerate()
            .filter(|(i, _)| i % 2 == 1)
            .map(|(_, s)| s)
            .collect();
        let explicit = !bold.is_empty();
        let candidates: Vec<&str> = if explicit {
            bold
        } else {
            note.content
                .split(['，', ',', '；', ';', '\n', ' ', '？', '?', '：', ':'])
                .collect()
        };
        for term in candidates {
            let term = term.trim_matches(|c: char| c.is_whitespace() || "#*-[]()>_`".contains(c));
            if !term.chars().all(|c| c.is_ascii_digit() || c == '.')
                && (2..=24).contains(&term.chars().count())
                && !result
                    .iter()
                    .any(|v: &String| v.to_lowercase() == term.to_lowercase())
            {
                result.push(term.to_string());
            }
            if result.len() >= 16 {
                return result;
            }
        }
    }
    result
}
pub fn fingerprint(notes: &[UserNote], segments: &[Segment], cursor: u32) -> String {
    // Stable FNV-1a digest is a change marker, not a security primitive.
    let mut hash = 0xcbf29ce484222325_u64;
    let mut add = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    for note in document_notes(notes) {
        add(note.content.as_bytes());
        add(note.kind.as_bytes());
        add(&[0]);
    }
    for segment in segments.iter().filter(|s| s.seq <= cursor) {
        add(segment.id.as_bytes());
        add(&segment.revision.to_le_bytes());
    }
    format!("{hash:x}")
}
pub fn needs_update(snapshot: &Snapshot) -> bool {
    !snapshot.segments.is_empty()
        && (snapshot
            .suggestions
            .iter()
            .any(|s| s.stale && s.status == "pending")
            || snapshot
                .segments
                .last()
                .is_some_and(|s| s.seq > snapshot.meeting.notes_cursor)
            || fingerprint(
                &snapshot.notes,
                &snapshot.segments,
                snapshot.meeting.notes_cursor,
            ) != snapshot.meeting.notes_fingerprint)
}

fn tokens(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut latin = String::new();
    let mut previous = None;
    for c in text.to_lowercase().chars().chain(std::iter::once(' ')) {
        if ('\u{3400}'..='\u{9fff}').contains(&c) {
            if !latin.is_empty() {
                result.push(std::mem::take(&mut latin));
            }
            result.push(c.to_string());
            if let Some(p) = previous {
                result.push(format!("{p}{c}"));
            }
            previous = Some(c);
        } else {
            previous = None;
            if c.is_ascii_alphanumeric() {
                latin.push(c);
            } else if !latin.is_empty() {
                result.push(std::mem::take(&mut latin));
            }
        }
    }
    result
}
/// A transient per-meeting index avoids a resident embedding model on an 8 GB device.
pub fn ranked(all: &[Segment], query: &[String]) -> Vec<usize> {
    ranked_text(
        &all.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
        query,
    )
}
pub fn ranked_text(all: &[&str], query: &[String]) -> Vec<usize> {
    let documents: Vec<Vec<String>> = all.iter().map(|s| tokens(s)).collect();
    let terms: HashSet<String> = query.iter().flat_map(|q| tokens(q)).collect();
    let average =
        documents.iter().map(Vec::len).sum::<usize>() as f64 / documents.len().max(1) as f64;
    let mut df = HashMap::<&str, usize>::new();
    for document in &documents {
        for word in document.iter().collect::<HashSet<_>>() {
            *df.entry(word.as_str()).or_default() += 1;
        }
    }
    let mut scores: Vec<(usize, f64)> = documents
        .iter()
        .enumerate()
        .map(|(i, document)| {
            let mut score = 0.;
            for term in &terms {
                let frequency = document.iter().filter(|t| *t == term).count() as f64;
                let hits = *df.get(term.as_str()).unwrap_or(&0) as f64;
                let idf = (1. + (documents.len() as f64 - hits + 0.5) / (hits + 0.5)).ln();
                score += idf * frequency * 2.2
                    / (frequency + 1.2 * (0.25 + 0.75 * document.len() as f64 / average.max(1.)));
            }
            let lower = all[i].to_lowercase();
            score += query
                .iter()
                .filter(|q| lower.contains(&q.to_lowercase()))
                .count() as f64
                * 3.;
            (i, score)
        })
        .filter(|(_, score)| *score > 0.)
        .collect();
    scores.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.0.cmp(&a.0)));
    scores.into_iter().map(|(i, _)| i).collect()
}
#[derive(Debug)]
pub struct Context {
    pub segments: Vec<Segment>,
    pub cursor: u32,
    pub keywords: Vec<String>,
}
pub fn retrieve(snapshot: &Snapshot) -> Context {
    let all = &snapshot.segments;
    let mut selected = Vec::new();
    let mut chars = 0;
    let mut cursor = snapshot.meeting.notes_cursor;
    for s in all.iter().filter(|s| s.seq > snapshot.meeting.notes_cursor) {
        if selected.len() >= 5 || (!selected.is_empty() && chars + s.text.chars().count() > 750) {
            break;
        }
        selected.push(s.clone());
        chars += s.text.chars().count();
        cursor = s.seq;
    }
    let keywords = focus_terms(&snapshot.notes);
    let mut query = keywords.clone();
    query.extend(selected.iter().map(|s| s.text.clone()));
    for index in ranked(all, &query).into_iter().take(12) {
        let s = &all[index];
        if selected.iter().any(|old| old.id == s.id) || s.seq > cursor {
            continue;
        }
        if chars + s.text.chars().count() > 1100 || selected.len() >= 9 {
            continue;
        }
        selected.push(s.clone());
        chars += s.text.chars().count();
    }
    if selected.is_empty() {
        selected.extend(all.iter().rev().take(3).cloned());
    }
    // A revision must revisit its original section even when no keyword hits it.
    for section in snapshot
        .suggestions
        .iter()
        .filter(|s| s.stale && s.status == "pending")
    {
        for source in &section.sources {
            if let Some(s) = all.iter().find(|s| s.id == source.segment_id) {
                if !selected.iter().any(|old| old.id == s.id)
                    && chars + s.text.chars().count() <= 1400
                {
                    selected.push(s.clone());
                    chars += s.text.chars().count();
                }
            }
        }
    }
    // Pull the rest of a related paragraph's evidence before allowing it to be rewritten.
    for section in snapshot
        .suggestions
        .iter()
        .filter(|s| s.status == "pending")
    {
        if section
            .sources
            .iter()
            .any(|q| selected.iter().any(|s| s.id == q.segment_id))
        {
            for source in &section.sources {
                if let Some(s) = all.iter().find(|s| s.id == source.segment_id) {
                    if !selected.iter().any(|old| old.id == s.id)
                        && chars + s.text.chars().count() <= 1400
                    {
                        selected.push(s.clone());
                        chars += s.text.chars().count();
                    }
                }
            }
        }
    }
    selected.sort_by_key(|s| s.seq);
    Context {
        segments: selected,
        cursor,
        keywords,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn segment(seq: u32, text: &str) -> Segment {
        Segment {
            id: format!("s{seq}"),
            meeting_id: "m".into(),
            run: 1,
            seq,
            start_ms: 0,
            end_ms: 1000,
            text: text.into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        }
    }
    #[test]
    fn short_chinese_names_and_english_keywords_retrieve_history() {
        let all = vec![
            segment(1, "张三负责预算复核，金额未定。"),
            segment(2, "Today we discussed the dashboard."),
            segment(3, "Offline transcription will be tested on Friday."),
        ];
        assert_eq!(ranked(&all, &["张三".into()])[0], 0);
        assert_eq!(ranked(&all, &["offline transcription".into()])[0], 2);
    }
    #[test]
    fn retrieval_does_not_skip_pending_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = crate::Engine::open(tmp.path().to_path_buf()).unwrap();
        let m = engine.create_meeting("test").unwrap();
        for i in 1..=20 {
            let mut s = segment(i, &"内容".repeat(80));
            s.meeting_id = m.id.clone();
            engine.store.lock().unwrap().save_segment(&s).unwrap();
        }
        let context = retrieve(&engine.snapshot(&m.id).unwrap());
        assert!(context.cursor < 20);
        assert_eq!(context.segments.first().unwrap().seq, 1);
        assert_eq!(context.segments.last().unwrap().seq, context.cursor);
    }
}
