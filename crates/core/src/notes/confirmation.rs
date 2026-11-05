//! Veto a confirmed assertion when the same literal source object is pending.
//! A later explicit update must be in this paragraph's own original citations.
//! This does not infer aliases, translate objects or create a general fact store.
use crate::{terminology::Terms, Segment, Source};
use anyhow::{ensure, Result};
use regex::Regex;
use std::{collections::BTreeMap, ops::Range, sync::OnceLock};

const MAX_BYTES: usize = 262_144;
const MAX_FRAMES: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Pending,
    Confirmed,
}
struct Frame {
    object: String,
    state: State,
}
struct Evidence {
    seq: u32,
    state: Option<State>,
    id: String,
    revision: u32,
    pending: bool,
}
#[derive(Default)]
pub(super) struct View {
    objects: BTreeMap<String, Evidence>,
    same_scope: bool,
}

fn key(object: &str) -> String {
    object
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

// Match the writer's distinction between apostrophes inside words and quotes.
fn word_apostrophe(text: &str, at: usize, ch: char) -> bool {
    matches!(ch, '\'' | '’')
        && text[..at].ends_with(|c: char| c.is_ascii_alphanumeric())
        && text[at + ch.len_utf8()..].starts_with(|c: char| c.is_ascii_alphanumeric())
}

// A condition/quotation may cover commas, semicolons and line wrapping. Only
// sentence stops outside quotations reset its scope; do not promote an inner
// phrase into an independent factual update.
fn sentences(text: &str) -> Vec<Range<usize>> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut close = None;
    let mut escaped = false;
    for (at, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if close.is_some() && ch == '\\' {
            escaped = true;
            continue;
        }
        if word_apostrophe(text, at, ch) {
            continue;
        }
        if let Some(expected) = close {
            if ch == expected {
                close = None;
            }
            continue;
        }
        close = match ch {
            '“' => Some('”'),
            '‘' => Some('’'),
            '「' => Some('」'),
            '『' => Some('』'),
            '"' | '\'' | '`' => Some(ch),
            _ => None,
        };
        if close.is_none()
            && matches!(ch, '。' | '.' | '!' | '！' | '?' | '？')
            && !(ch == '.'
                && text[..at].ends_with(|c: char| c.is_ascii_alphanumeric())
                && text[at + 1..].starts_with(|c: char| c.is_ascii_alphanumeric()))
        {
            let end = at + ch.len_utf8();
            result.push(start..end);
            start = end;
        }
    }
    if start < text.len() {
        result.push(start..text.len());
    }
    result
}

fn frames(text: &str, terms: &Terms) -> Result<Vec<Frame>> {
    ensure!(
        text.len() <= MAX_BYTES,
        "确认状态校验超过文本预算，本轮未提交"
    );
    static STATE_WORD: OnceLock<Regex> = OnceLock::new();
    if !STATE_WORD
        .get_or_init(|| Regex::new(r"确认|(?i:\b(?:unconfirmed|confirmed)\b)").unwrap())
        .is_match(text)
    {
        return Ok(Vec::new());
    }
    static NONASSERTIVE: OnceLock<Regex> = OnceLock::new();
    static ZH: OnceLock<Regex> = OnceLock::new();
    static EN: OnceLock<Regex> = OnceLock::new();
    let nonassertive = NONASSERTIVE.get_or_init(|| Regex::new(concat!(
        r"(?i)\b(?:if|unless|when|once|whether|assuming|suppose|maybe|perhaps|possibly|might|may|could|would|should|will)\b",
        r"|如果|假如|倘若|除非|一旦|只要|只有|是否|能否|可能|也许|或许|假设|并非|不是"
    )).unwrap());
    let zh = ZH.get_or_init(|| {
        Regex::new(concat!(
            r"^(?P<object>[\p{Han}A-Za-z0-9# _-]{2,120}?)",
            r"(?:(?P<pending>仍未确认|尚未确认|还没有确认|还没确认|没有确认|未确认)",
            r"|(?P<confirmed>已经确认|现已确认|已确认))(?:了)?$"
        ))
        .unwrap()
    });
    let en = EN.get_or_init(|| Regex::new(concat!(
        r"(?i)^(?:the\s+)?(?P<object>[A-Za-z0-9#][A-Za-z0-9# _-]{1,119}?)\s+",
        r"(?:(?P<pending>is\s+(?:not(?:\s+yet)?\s+confirmed|still\s+(?:not\s+confirmed|unconfirmed)|unconfirmed)",
        r"|has\s+not(?:\s+yet)?\s+been\s+confirmed)",
        r"|(?P<confirmed>is\s+(?:(?:now|already)\s+)?confirmed|has\s+(?:(?:now|already)\s+)?been\s+confirmed))$"
    )).unwrap());
    let mut result = Vec::new();
    let syntax = super::validation::completion_name_mask(text, text, terms);
    for range in sentences(text) {
        let sentence = &text[range.clone()];
        let syntax = &syntax[range];
        if syntax.contains([
            '?', '？', '"', '“', '”', '‘', '’', '「', '」', '『', '』', '`',
        ]) || syntax
            .char_indices()
            .any(|(at, ch)| ch == '\'' && !word_apostrophe(syntax, at, ch))
            || nonassertive.is_match(syntax)
        {
            continue;
        }
        // Masking preserves byte offsets, so literal subject keys always come
        // from original text; protected names cannot supply a state predicate.
        for (at, clause) in sentence
            .match_indices([',', '，', ';', '；'])
            .map(|(at, delimiter)| (at, delimiter.len()))
            .chain(std::iter::once((sentence.len(), 0)))
            .scan(0, |start, (end, delimiter_len)| {
                let range = *start..end;
                *start = end + delimiter_len;
                Some(range)
            })
            .map(|range| (range.start, &sentence[range]))
        {
            let leading = clause.len() - clause.trim_start().len();
            let original = clause
                .trim()
                .trim_end_matches(['。', '.', '!', '！'])
                .trim_end();
            if original.is_empty() {
                continue;
            }
            let syntax = &syntax[at + leading..at + leading + original.len()];
            let Some(capture) = zh.captures(syntax).or_else(|| en.captures(syntax)) else {
                continue;
            };
            let object = capture.name("object").expect("literal object").range();
            let mut object_start = object.start;
            // Reuse an explicitly masked reported professor/doctor name and
            // the existing report verbs. A verb inside an arbitrary object
            // such as 说明报告 must not merge it with a different report.
            if capture["object"]
                .chars()
                .any(|c| ('\u{3400}'..='\u{9fff}').contains(&c))
            {
                if let Some(end) = super::attribution_roles::REPORT_VERBS
                    .iter()
                    .filter_map(|verb| {
                        let prefix = &syntax[object.clone()];
                        let at = prefix.rfind(*verb)?;
                        let reporter = prefix[..at].trim_end();
                        ["教授", "博士"]
                            .iter()
                            .any(|title| {
                                reporter
                                    .strip_suffix(*title)
                                    .is_some_and(|name| name.trim_end().ends_with('#'))
                            })
                            .then_some(at + verb.len())
                    })
                    .max()
                {
                    object_start += end;
                }
            }
            let object = original[object_start..object.end].trim();
            if object.chars().count() < 2 {
                continue;
            }
            ensure!(
                result.len() < MAX_FRAMES,
                "确认状态校验超过断言预算，本轮未提交"
            );
            result.push(Frame {
                object: key(object),
                state: if capture.name("pending").is_some() {
                    State::Pending
                } else {
                    State::Confirmed
                },
            });
        }
    }
    Ok(result)
}

impl View {
    pub(super) fn new(segments: &[Segment], meeting: &str, terms: &Terms) -> Result<Self> {
        let mut view = Self {
            same_scope: segments.first().is_some_and(|first| {
                segments.iter().all(|s| {
                    s.meeting_id == meeting
                        && s.meeting_id == first.meeting_id
                        && s.run == first.run
                })
            }),
            ..Default::default()
        };
        ensure!(
            segments
                .iter()
                .try_fold(0usize, |total, s| total.checked_add(s.text.len()))
                .is_some_and(|total| total <= MAX_BYTES),
            "确认状态校验超过来源预算，本轮未提交"
        );
        let mut frame_count = 0usize;
        for source in segments {
            let source_frames = frames(&source.text, terms)?;
            frame_count = frame_count
                .checked_add(source_frames.len())
                .filter(|count| *count <= MAX_FRAMES)
                .ok_or_else(|| anyhow::anyhow!("确认状态校验超过来源断言预算，本轮未提交"))?;
            // Each map object requires an admitted frame, so this same total
            // bound also caps the complete view's distinct object count.
            for frame in source_frames {
                if let Some(current) = view.objects.get_mut(&frame.object) {
                    current.pending |= frame.state == State::Pending;
                    if source.seq > current.seq {
                        current.seq = source.seq;
                        current.state = Some(frame.state);
                        current.id.clone_from(&source.id);
                        current.revision = source.revision;
                    } else if source.seq == current.seq
                        && (current.state != Some(frame.state)
                            || current.id != source.id
                            || current.revision != source.revision)
                    {
                        // Conflicting states in one passage or duplicate sequence
                        // identities cannot establish a later explicit update.
                        current.state = None;
                    }
                } else {
                    view.objects.insert(
                        frame.object,
                        Evidence {
                            seq: source.seq,
                            state: Some(frame.state),
                            id: source.id.clone(),
                            revision: source.revision,
                            pending: frame.state == State::Pending,
                        },
                    );
                }
            }
        }
        Ok(view)
    }

    pub(super) fn validate(&self, body: &str, sources: &[Source], terms: &Terms) -> Result<()> {
        for claim in frames(body, terms)? {
            let Some(evidence) = self.objects.get(&claim.object).filter(|e| e.pending) else {
                continue;
            };
            if claim.state == State::Confirmed {
                ensure!(self.same_scope
                    && evidence.state == Some(State::Confirmed)
                    && sources.iter().any(|s| s.segment_id == evidence.id && s.revision == evidence.revision),
                    "纪要把同一事项的未确认状态写成已确认，请保留当前状态及对应原话；本轮未提交");
            }
        }
        Ok(())
    }
}
