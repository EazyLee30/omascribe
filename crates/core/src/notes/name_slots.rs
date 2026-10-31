//! A spelling codec for the existing second article request. IDs carry only
//! literal source bytes; they never authorize a claim, actor or sentence scope.
use crate::{terminology::Terms, Segment};
use anyhow::{ensure, Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;
const MAX_INPUT: usize = 16_384;
const MAX_BODY: usize = 65_536;

pub(super) struct Plan {
    pub messages: Vec<Value>,
    grammar: String,
    names: Vec<String>,
    sources: Vec<Source>,
}
struct Source {
    ordinal: usize,
    id: String,
    meeting_id: String,
    run: u32,
    revision: u32,
    text: String,
    spans: Vec<std::ops::Range<usize>>,
}
impl Plan {
    pub fn new(system: &str, input: &str, segments: &[Segment], terms: &Terms) -> Option<Self> {
        if input.len() > MAX_INPUT {
            return None;
        }
        let mut encoded: Value = serde_json::from_str(input).ok()?;
        let passages = encoded["transcript"].as_array_mut()?;
        let mut sources = Vec::new();
        let mut names = Vec::new();
        for passage in passages.iter() {
            let ordinal = usize::try_from(passage["id"].as_u64()?).ok()?;
            let source = segments.get(ordinal.checked_sub(1)?)?;
            let text = passage["text"].as_str()?;
            // This full-prompt codec deliberately declines rewritten budget
            // views/cached translations and literal bracket syntax. Its source
            // binding is established here, not inferred from generated IDs.
            if text != source.text || text.contains(['[', ']']) {
                return None;
            }
            let spans = crate::translation::source_spelling_spans(text, terms)?;
            for span in &spans {
                let name = text[span.clone()].to_owned();
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            sources.push(Source {
                ordinal,
                id: source.id.clone(),
                meeting_id: source.meeting_id.clone(),
                run: source.run,
                revision: source.revision,
                text: text.to_owned(),
                spans,
            });
        }
        if names.is_empty()
            || names.len() > 12
            || names.iter().map(|s| s.chars().count()).sum::<usize>() > 240
        {
            return None;
        }
        names.sort_by_key(|name| std::cmp::Reverse(name.len()));
        for (passage, source) in passages.iter_mut().zip(&sources) {
            let text = passage["text"].as_str()?;
            let mut replaced = String::new();
            let mut end = 0;
            for span in &source.spans {
                replaced.push_str(&text[end..span.start]);
                let index = names.iter().position(|name| name == &text[span.clone()])?;
                replaced.push_str(&format!("[{}]", char::from(b'A' + index as u8)));
                end = span.end;
            }
            replaced.push_str(&text[end..]);
            passage["text"] = replaced.into();
        }
        let choices = (0..names.len())
            .map(|i| format!("\"[{}]\"", char::from(b'A' + i as u8)))
            .collect::<Vec<_>>()
            .join(" | ");
        // Linear grammar, no factorial name permutations or mandatory mentions:
        // a summary can mention a name more than once or omit an irrelevant one.
        // The ordinary coverage, language and factual checks still decide if
        // that article is acceptable after literal decoding.
        let grammar = format!(
            "root ::= (prose | {choices})+\nprose ::= [^\\x00-\\x08\\x0b-\\x1f\\x5b\\x5d]\n"
        );
        let system = format!("{system} [A]等标记是原话中的名称字面量，出现时保持标记完整，不猜写名称，不解释标记。Placeholders such as [A] are literal names in their original source positions; keep each marker intact when used, without explaining or guessing its name.");
        Some(Self {
            messages: vec![
                json!({"role":"system","content":system}),
                json!({"role":"user","content":serde_json::to_string(&encoded).ok()?}),
            ],
            grammar,
            names,
            sources,
        })
    }
    pub fn grammar(&self) -> &str {
        &self.grammar
    }
    pub fn count(&self) -> usize {
        self.names.len()
    }
    pub fn body(&self, raw: &str, segments: &[Segment]) -> Result<String> {
        ensure!(raw.len() <= MAX_BODY, "模型正文超过原始字节预算");
        static ALIAS: OnceLock<Regex> = OnceLock::new();
        ensure!(
            !ALIAS
                .get_or_init(|| Regex::new(r"[（(]\s*[A-L]\s*[)）]").unwrap())
                .find_iter(raw)
                .any(|m| !self.sources.iter().any(|s| s.text.contains(m.as_str()))),
            "模型输出了未解析的名称标记别名"
        );
        for bound in &self.sources {
            let source = segments
                .get(bound.ordinal - 1)
                .context("名称标记来源已失效")?;
            ensure!(
                source.id == bound.id
                    && source.revision == bound.revision
                    && source.text == bound.text
                    && source.run == bound.run
                    && source.meeting_id == bound.meeting_id,
                "名称标记来源已失效"
            );
        }
        let mut body = String::new();
        let mut remainder = raw;
        while let Some(start) = remainder.find(['[', ']']) {
            ensure!(body.len() + start <= MAX_BODY, "名称展开超过正文字节预算");
            body.push_str(&remainder[..start]);
            let token = remainder
                .get(start..start + 3)
                .context("模型输出了不完整的名称标记")?;
            let bytes = token.as_bytes();
            ensure!(
                bytes[0] == b'[' && bytes[2] == b']' && bytes[1].is_ascii_uppercase(),
                "模型输出了无效名称标记"
            );
            let name = self
                .names
                .get(usize::from(bytes[1] - b'A'))
                .context("模型引用了不存在的名称标记")?;
            ensure!(
                body.len() + name.len() <= MAX_BODY,
                "名称展开超过正文字节预算"
            );
            body.push_str(name);
            remainder = &remainder[start + 3..];
        }
        ensure!(
            body.len() + remainder.len() <= MAX_BODY,
            "名称展开超过正文字节预算"
        );
        body.push_str(remainder);
        ensure!(!body.trim().is_empty(), "模型未生成纪要正文");
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(seq: i64, text: &str) -> Segment {
        serde_json::from_value(json!({"id":format!("source-{seq}"),"meeting_id":"m","seq":seq,"run":1,"revision":1,"start_ms":0,"end_ms":1,"text":text})).unwrap()
    }
    #[test]
    fn names_decode_only_from_bound_sources_without_new_claim_authority() {
        let segments = vec![
            source(1, "当迪大学的 PAMELA FURGO森教授指出照片可能有风险"),
            source(2, "邓迪大学的潘 MALE FERGUSON教授指出照片可能有风险"),
        ];
        let input = json!({"transcript":[{"id":1,"text":segments[0].text},{"id":2,"text":segments[1].text}]}).to_string();
        let plan = Plan::new("写中文正文", &input, &segments, &Terms::new(&[], &[])).unwrap();
        let encoded: Value =
            serde_json::from_str(plan.messages[1]["content"].as_str().unwrap()).unwrap();
        assert!(!encoded.to_string().contains("PAMELA"));
        assert!(!encoded.to_string().contains("当迪大学"));
        let raw = encoded["transcript"][0]["text"].as_str().unwrap();
        assert_eq!(plan.body(raw, &segments).unwrap(), segments[0].text);
        // Omission/repetition is a prose decision, not a spelling decoder fact.
        assert_eq!(
            plan.body("[A][A]", &segments).unwrap(),
            plan.names[0].repeat(2)
        );
        for raw in ["[Z]", "[A", "A]", "[AB]", "[中]", "(A)", "（ A ）"] {
            assert!(plan.body(raw, &segments).is_err(), "{raw}");
        }
        let mut stale = segments.clone();
        stale[0].revision += 1;
        assert!(plan.body(raw, &stale).is_err());
    }
    #[test]
    fn input_ids_and_original_literals_are_checked_before_encoding() {
        let segments = vec![source(1, "OmaScribe 的录音仍未完成")];
        let terms = Terms::new(&[], &[]);
        for input in [
            json!({"transcript":[{"id":2,"text":segments[0].text}]}),
            json!({"transcript":[{"id":1,"text":"OmaScribe 的录音已完成"}]}),
            json!({"transcript":[{"id":0,"text":segments[0].text}]}),
        ] {
            assert!(Plan::new("写中文正文", &input.to_string(), &segments, &terms).is_none());
        }
        let brackets = vec![source(1, "OmaScribe 使用[A]语法")];
        assert!(Plan::new(
            "写中文正文",
            &json!({"transcript":[{"id":1,"text":brackets[0].text}]}).to_string(),
            &brackets,
            &terms
        )
        .is_none());
    }
    #[test]
    fn reported_labels_do_not_encode_unrelated_conditionals_or_negation() {
        let segments = vec![
            source(
                1,
                "Professor If said the cache is untested. If approved, release tomorrow.",
            ),
            source(2, "If approved, release tomorrow."),
            source(
                3,
                "Professor Not said the test remains pending. Not approved for release.",
            ),
            source(4, "Not approved for public release."),
        ];
        let input=json!({"transcript":segments.iter().enumerate().map(|(i,s)|json!({"id":i+1,"text":s.text})).collect::<Vec<_>>()}).to_string();
        let plan = Plan::new("Write prose", &input, &segments, &Terms::new(&[], &[])).unwrap();
        let encoded: Value =
            serde_json::from_str(plan.messages[1]["content"].as_str().unwrap()).unwrap();
        let first = encoded["transcript"][0]["text"].as_str().unwrap();
        assert!(first.starts_with("Professor ["));
        assert!(first.ends_with("If approved, release tomorrow."));
        let third = encoded["transcript"][2]["text"].as_str().unwrap();
        assert!(third.starts_with("Professor ["));
        assert!(third.ends_with("Not approved for release."));
        assert_eq!(encoded["transcript"][1]["text"], segments[1].text);
        assert_eq!(encoded["transcript"][3]["text"], segments[3].text);
    }
    #[test]
    fn repeated_name_markers_cannot_expand_beyond_the_body_byte_budget() {
        let segments = vec![source(
            1,
            "Professor Abcdefghijklmnopqrstuvwxyz said the cache is untested.",
        )];
        let input = json!({"transcript":[{"id":1,"text":segments[0].text}]}).to_string();
        let plan = Plan::new("Write prose", &input, &segments, &Terms::new(&[], &[])).unwrap();
        assert!(plan.body(&"[A]".repeat(MAX_BODY / 3), &segments).is_err());
        assert!(plan.body(&"x".repeat(MAX_BODY + 1), &segments).is_err());
    }
}
