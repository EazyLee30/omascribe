//! Bounded lexical RAG: short Chinese terms, bilingual BM25, chronology, and source closure.
pub use super::lexical::ranked_text;
use crate::{Segment, Snapshot, UserNote};
/// Once a Markdown document exists, legacy cards are retained as backup, not duplicated as context.
pub fn document_notes(notes: &[UserNote]) -> Vec<&UserNote> {
    if let Some(document) = notes.iter().find(|n| n.kind == "document") {
        vec![document]
    } else {
        notes.iter().collect()
    }
}
/// Shared by retrieval and the UI: local dictionary + TF-IDF, never a cloud request.
pub fn extract_focus(content: &str) -> Vec<String> {
    use jieba_rs::{Jieba, KeywordExtract, KeywordExtractConfig, TfIdf};
    use std::sync::OnceLock;
    static EXTRACTOR: OnceLock<(Jieba, TfIdf)> = OnceLock::new();
    let content: String = content.chars().take(20_000).collect();
    let content = content
        .replace("&nbsp;", " ")
        .replace("&#160;", " ")
        .replace("&amp;", "&")
        .replace("\u{a0}", " ");
    let explicit: Vec<String> = content
        .split("**")
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, text)| text.trim().to_owned())
        .filter(|text| {
            (2..=24).contains(&text.chars().count()) && !text.contains(['。', '！', '？', '\n'])
        })
        .collect();
    let candidates = if explicit.is_empty() {
        let (jieba, extractor) = EXTRACTOR.get_or_init(|| {
            let mut config = KeywordExtractConfig::builder().min_keyword_length(2);
            for word in [
                "现在",
                "正在",
                "目前",
                "这个",
                "这些",
                "那个",
                "那些",
                "什么",
                "为什么",
                "怎么",
                "没有",
                "不是",
                "是否",
                "可以",
                "能够",
                "需要",
                "希望",
                "感觉",
                "觉得",
                "然后",
                "但是",
                "所以",
                "因为",
                "这里",
                "那边",
                "这边",
                "内容",
                "东西",
                "事情",
                "一下",
                "一些",
                "已经",
                "还有",
                "我们",
                "你们",
                "自己",
                "请问",
                "时候",
                "出来",
                "进行",
                "一个",
                "want",
                "would",
                "could",
                "should",
                "please",
                "about",
                "some",
                "just",
                "what",
                "when",
                "where",
                "why",
                "how",
                "now",
                "need",
                "our",
                "your",
                "was",
                "were",
                "been",
                "also",
                "will",
                "them",
                "they",
                "but",
                "does",
                "did",
                "very",
                "into",
                "than",
                "there",
                "these",
                "those",
            ] {
                config = config.add_stop_word(word);
            }
            (
                Jieba::new(),
                TfIdf::new(None::<&mut std::io::Cursor<Vec<u8>>>, config.build()),
            )
        });
        extractor
            .extract_keywords(
                jieba,
                &content,
                24,
                ["n", "nr", "ns", "nt", "nz", "vn", "eng"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            )
            .into_iter()
            .map(|word| word.keyword)
            .collect()
    } else {
        explicit
    };
    let mut result = Vec::<String>::new();
    for word in candidates {
        if (2..=24).contains(&word.chars().count())
            && word.chars().any(char::is_alphabetic)
            && !result.iter().any(|old| old.eq_ignore_ascii_case(&word))
        {
            result.push(word);
        }
        if result.len() == 6 {
            break;
        }
    }
    result
}
pub fn focus_terms(notes: &[UserNote]) -> Vec<String> {
    extract_focus(
        &document_notes(notes)
            .into_iter()
            .filter(|note| note.kind != "opinion")
            .map(|note| note.content.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    )
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
    // Revisit automatic drafts once for source-state composition and spelling fixes.
    // Human paragraphs remain protected by the transactional document merge.
    add(b"document-v7-preserve-target-language-prose");
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

/// A transient per-meeting index avoids a resident embedding model on an 8 GB device.
pub fn ranked(all: &[Segment], query: &[String]) -> Vec<usize> {
    ranked_text(
        &all.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
        query,
    )
}
#[derive(Debug, Clone)]
pub struct Context {
    pub segments: Vec<Segment>,
    pub cursor: u32,
    pub keywords: Vec<String>,
}
pub fn retrieve(snapshot: &Snapshot, cache: Option<&super::lexical::Cache>) -> Context {
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
    let ranked = cache.map_or_else(
        || ranked(all, &query),
        |cache| cache.ranked(&snapshot.meeting.id, snapshot.meeting.run, all, &query),
    );
    for index in ranked.into_iter().take(12) {
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
    #[test]
    fn prose_becomes_topics_instead_of_sentence_copies() {
        let text = "我正在测试会议纪要功能，但是没有自动提取关键词，然后关注那边全部是我写的内容。";
        let words = extract_focus(text);
        assert!(!words.is_empty());
        assert!(words.iter().all(|w| w.chars().count() <= 8), "{words:?}");
        assert!(
            words
                .iter()
                .any(|w| w.contains("纪要") || w.contains("关键词")),
            "{words:?}"
        );
        assert!(!words
            .iter()
            .any(|w| ["正在", "没有", "内容", "然后"].contains(&w.as_str())));
        assert_eq!(
            extract_focus("请关注 **离线转写** 和 **项目预算**"),
            ["离线转写", "项目预算"]
        );
        assert!(extract_focus("现在我正在这里，然后为什么呢？").is_empty());
        let english = extract_focus(
            "We need offline transcription and speaker identification for the research interview.",
        );
        assert!(english.iter().any(|s| s == "transcription"));
        assert!(!english.iter().any(|s| s == "need" || s == "the"));
    }
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
        let context = retrieve(&engine.snapshot(&m.id).unwrap(), None);
        assert!(context.cursor < 20);
        assert_eq!(context.segments.first().unwrap().seq, 1);
        assert_eq!(context.segments.last().unwrap().seq, context.cursor);
    }
}
