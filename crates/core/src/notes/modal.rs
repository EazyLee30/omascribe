//! Local modal veto for main-stage music ending and festival sections playing
//! until late into the night. Not a general entailment or object alias rule.
//! Later evidence can discharge only its own predicate in its own citations.
use crate::{terminology::Terms, Segment, Source};
use anyhow::{ensure, Result};
use regex::Regex;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

const MAX_BYTES: usize = 262_144;
const MAX_FRAMES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Field {
    MainStageMusicEnding,
    FestivalPartsPlaying,
}
#[derive(Debug)]
pub(super) struct Mismatch(Field);
impl Mismatch {
    pub(super) fn hint(&self) -> &'static str {
        match self.0 {
            Field::MainStageMusicEnding => "保留原SOURCE中主舞台音乐结束的局部MAY；只有该对象真正后续、且本段引用的明确事实才能更新它。继续演奏的可能性须单独保留，不能借别句似乎。Keep MAY with the music-ending claim and separately with possible continuing festival sections.",
            Field::FestivalPartsPlaying => "保留原SOURCE中节庆区域继续演奏至深夜的局部MAY；只有该完整谓词真正后续、且本段引用的明确事实才能更新它，另一字段或更短时段不能代替。Keep MAY for continuing sections until late into the night, separate from the music-ending claim.",
        }
    }
}
impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.hint())
    }
}
impl std::error::Error for Mismatch {}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Modal,
    Definite,
}
struct Frame {
    field: Field,
    state: Option<State>,
    modal: bool,
    nonassertive: bool,
    closed_witness: bool,
}
struct Evidence {
    modal_seq: Option<u32>,
    latest_seq: u32,
    state: Option<State>,
    id: String,
    revision: u32,
}
pub(super) struct View {
    fields: BTreeMap<Field, Evidence>,
    same_scope: bool,
}

struct Pattern {
    field: Field,
    regex: Regex,
}
fn patterns() -> &'static [Pattern] {
    static PATTERNS: OnceLock<Vec<Pattern>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let state = r"(?:(?:仍|将|会|可能|或许|也许|似乎|继续|持续)[ \t]*){0,6}";
        vec![
            Pattern { field:Field::MainStageMusicEnding, regex:Regex::new(concat!(
                r"(?i)\b(?:music\s+on\s+(?:the\s+)?main\s+stages?|main[- ]stage\s+music)\s+",
                r"(?P<modal>(?:may|might)\s+have\s+(?:been\s+)?(?:finished|finish|ended|completed))\b"
            )).unwrap() },
            Pattern { field:Field::MainStageMusicEnding, regex:Regex::new(concat!(
                r"(?i)^[ \t]*(?:music\s+on\s+(?:the\s+)?main\s+stages?|main[- ]stage\s+music)\s+",
                r"(?:has\s+(?:now\s+)?(?:been\s+)?|is\s+(?:now\s+)?)(?:finished|ended|completed)[ \t]*(?:$|[.,;!?])"
            )).unwrap() },
            Pattern { field:Field::MainStageMusicEnding, regex:Regex::new(concat!(
                r"(?:^|[，,；;])[ \t]*(?:但|不过|然而)?[ \t]*",
                r"(?:音乐[ \t]*在[ \t]*主舞台|主舞台(?:的)?音乐)[ \t]*",
                r"(?:已经|已)?[ \t]*(?:结束|完毕|完成)(?:了)?[ \t]*(?:$|[，,；;。.!！])"
            )).unwrap() },
            // Negative provenance is local to the exact object/predicate,
            // independent of a discourse prefix. It grants no positive fact.
            Pattern { field:Field::MainStageMusicEnding, regex:Regex::new(concat!(
                r"(?P<hyp>即使)?[ \t]*(?:音乐[ \t]*在[ \t]*主舞台|主舞台(?:的)?音乐)[ \t]*",
                r"(?P<modal>可能|或许|也许|似乎)[ \t]*(?:已经|已)?[ \t]*(?:结束|完毕|完成)(?:了)?[ \t]*(?:$|[，,；;。.!！])"
            )).unwrap() },
            Pattern { field:Field::MainStageMusicEnding, regex:Regex::new(concat!(
                r"(?P<hyp>即使)[ \t]*(?:音乐[ \t]*在[ \t]*主舞台|主舞台(?:的)?音乐)[ \t]*",
                r"(?:已经|已)?[ \t]*(?:结束|完毕|完成)(?:了)?[ \t]*(?:$|[，,；;。.!！])"
            )).unwrap() },
            Pattern { field:Field::FestivalPartsPlaying, regex:Regex::new(concat!(
                r"(?i)\bthere\s+(?P<modal>may|might)\s+be\s+(?:some\s+)?sections?\s+of\s+(?:the\s+)?festival\s+",
                r"that\s+(?:will\s+)?(?:keep|continue)\s+playing\s+music\b"
            )).unwrap() },
            Pattern { field:Field::FestivalPartsPlaying, regex:Regex::new(concat!(
                r"(?i)^[ \t]*(?:some\s+)?sections?\s+of\s+(?:the\s+)?festival\s+",
                r"(?P<modal>may\s+|might\s+)?(?:will\s+)?(?:keep|continue)\s+playing\s+music\s+until\s+late\s+into\s+the\s+night[ \t]*(?:$|[.,;!?])"
            )).unwrap() },
            Pattern { field:Field::FestivalPartsPlaying, regex:Regex::new(&format!(
                concat!(r"(?:^|[，,；;])[ \t]*(?:但|不过|然而)?[ \t]*嘉年华",
                    r"(?:(?:的)?(?:某些部分|某些区域|部分区域){state}|{state}有(?:某些部分|某些区域|部分区域){state})",
                    r"演奏(?:音乐)?(?:直到|至)深夜[ \t]*(?:$|[，,；;。.!！])"),state=state
            )).unwrap() },
        ]
    })
}

fn frames(text: &str, terms: &Terms) -> Result<Vec<Frame>> {
    ensure!(
        text.len() <= MAX_BYTES,
        "模态来源校验超过文本预算，本轮未提交"
    );
    static TRIGGER: OnceLock<Regex> = OnceLock::new();
    if !TRIGGER
        .get_or_init(|| Regex::new(r"音乐|嘉年华|(?i:\bmusic\b|\bfestival\b)").unwrap())
        .is_match(text)
    {
        return Ok(Vec::new());
    }
    static LOCAL_MODAL: OnceLock<Regex> = OnceLock::new();
    static CONDITION: OnceLock<Regex> = OnceLock::new();
    static END_MENTION: OnceLock<Regex> = OnceLock::new();
    static PLAY_MENTION: OnceLock<Regex> = OnceLock::new();
    let local_modal = LOCAL_MODAL.get_or_init(|| Regex::new(r"可能|或许|也许|似乎").unwrap());
    let condition=CONDITION.get_or_init(||Regex::new(r"(?i)\b(?:if|unless|when|once|whether|assuming|suppose)\b|如果|假如|除非|一旦|即使|是否|能否|不是|并非").unwrap());
    let end_mention=END_MENTION.get_or_init(||Regex::new(concat!(
        r"(?i)(?:(?:音乐[ \t]*在[ \t]*主舞台|主舞台(?:的)?音乐)[ \t]*(?:可能|或许|也许|似乎|已经|已|未|不|没有|结束|完成|完毕)",
        r"|(?:\bmusic\s+on\s+(?:the\s+)?main\s+stages?\b|\bmain[- ]stage\s+music\b)\s+(?:may|might|has|is|not)\b)",
        r"[^。.!?！？]{0,48}(?:结束|完毕|完成|\b(?:finish(?:ed)?|ended|completed)\b)"
    )).unwrap());
    let play_mention=PLAY_MENTION.get_or_init(||Regex::new(concat!(
        r"(?i)(?:嘉年华(?:的)?(?:某些部分|某些区域|部分区域)[ \t]*(?:仍|将|会|可能|或许|也许|似乎|继续|持续|演奏|不|未)",
        r"|嘉年华(?:(?:仍|可能|或许|也许|似乎)[ \t]*){0,3}有(?:某些部分|某些区域|部分区域)",
        r"|\bsections?\s+of\s+(?:the\s+)?festival\b\s+(?:that|will|may|might|keep|continue)\b)",
        r"[^。.!?！？]{0,64}(?:演奏|playing\s+music)"
    )).unwrap());
    let mask = super::validation::completion_name_mask(text, text, terms);
    let mut result = Vec::new();
    for range in super::confirmation::sentences(text) {
        let sentence = &mask[range];
        let first = result.len();
        let mut covered = Vec::new();
        let nonassertive = sentence.contains([
            '?', '？', '"', '“', '”', '‘', '’', '「', '」', '『', '』', '`',
        ]) || sentence
            .char_indices()
            .any(|(at, ch)| ch == '\'' && !super::confirmation::word_apostrophe(sentence, at, ch))
            || condition.is_match(sentence);
        for field in [Field::MainStageMusicEnding, Field::FestivalPartsPlaying] {
            let mut states = Vec::new();
            for pattern in patterns().iter().filter(|p| p.field == field) {
                for capture in pattern.regex.captures_iter(sentence) {
                    ensure!(
                        covered.len() < MAX_FRAMES,
                        "模态来源校验超过局部断言预算，本轮未提交"
                    );
                    covered.push(capture.get(0).unwrap().range());
                    let explicit_modal = capture.name("modal").is_some()
                        || capture.name("pre").is_some()
                        || capture.name("hyp").is_some();
                    // The Chinese continuation expression's modifiers are
                    // inside this exact object's frame, not the whole article.
                    let scoped_modal = field == Field::FestivalPartsPlaying
                        && local_modal.is_match(capture.get(0).unwrap().as_str());
                    states.push(if explicit_modal || scoped_modal {
                        State::Modal
                    } else {
                        State::Definite
                    });
                }
            }
            let mention = match field {
                Field::MainStageMusicEnding => end_mention.is_match(sentence),
                Field::FestivalPartsPlaying => play_mention.is_match(sentence),
            };
            if states.is_empty() && !mention {
                continue;
            }
            ensure!(
                result.len() < MAX_FRAMES,
                "模态来源校验超过断言预算，本轮未提交"
            );
            let modal = states.contains(&State::Modal);
            let state = if states.len() == 1 {
                Some(states[0])
            } else {
                None
            };
            result.push(Frame {
                field,
                state,
                modal,
                nonassertive,
                closed_witness: false,
            });
        }
        // Positive evidence must be a whole simple assertion of these known
        // predicates. Unknown wrappers/qualifiers cannot become permission by
        // matching a factual-looking substring; negative modal scans need not
        // prove such a closed assertion. This is not a prose equivalence bridge.
        covered.sort_by_key(|r| (r.start, r.end));
        let mut end = 0;
        let mut closed = !covered.is_empty();
        for range in covered {
            if range.start > end && !sentence[end..range.start].trim().is_empty() {
                closed = false;
            }
            end = end.max(range.end);
        }
        closed &= sentence[end..].trim().is_empty();
        for frame in &mut result[first..] {
            frame.closed_witness = closed;
        }
    }
    Ok(result)
}

/// Read-only typed object/predicate observations for a bounded report payload.
/// A mention-only/veto fallback is not a complete field and grants no witness.
/// This exports no state, modality discharge, or whole-source proof authority.
pub(super) fn reported_payload_fields(text: &str, terms: &Terms) -> Result<BTreeSet<Field>> {
    Ok(frames(text, terms)?
        .into_iter()
        .filter(|frame| frame.state.is_some())
        .map(|frame| frame.field)
        .collect())
}

impl View {
    pub(super) fn new(segments: &[Segment], meeting: &str, terms: &Terms) -> Result<Self> {
        ensure!(
            segments
                .iter()
                .try_fold(0usize, |sum, s| sum.checked_add(s.text.len()))
                .is_some_and(|n| n <= MAX_BYTES),
            "模态来源校验超过来源预算，本轮未提交"
        );
        let mut result = Self {
            fields: BTreeMap::new(),
            same_scope: segments.first().is_some_and(|first| {
                segments
                    .iter()
                    .all(|s| s.meeting_id == meeting && s.run == first.run)
            }),
        };
        let mut count = 0usize;
        for source in segments {
            let observations = frames(&source.text, terms)?;
            count = count
                .checked_add(observations.len())
                .filter(|n| *n <= MAX_FRAMES)
                .ok_or_else(|| anyhow::anyhow!("模态来源校验超过来源断言预算，本轮未提交"))?;
            for frame in observations {
                let state = if (frame.nonassertive || !frame.closed_witness)
                    && frame.state == Some(State::Definite)
                {
                    None
                } else {
                    frame.state
                };
                if let Some(old) = result.fields.get_mut(&frame.field) {
                    if frame.modal {
                        old.modal_seq =
                            Some(old.modal_seq.map_or(source.seq, |s| s.max(source.seq)));
                    }
                    if source.seq > old.latest_seq {
                        old.latest_seq = source.seq;
                        old.state = state;
                        old.id.clone_from(&source.id);
                        old.revision = source.revision;
                    } else if source.seq == old.latest_seq
                        && (old.state != state
                            || old.id != source.id
                            || old.revision != source.revision)
                    {
                        old.state = None;
                    }
                } else {
                    result.fields.insert(
                        frame.field,
                        Evidence {
                            modal_seq: frame.modal.then_some(source.seq),
                            latest_seq: source.seq,
                            state,
                            id: source.id.clone(),
                            revision: source.revision,
                        },
                    );
                }
            }
        }
        Ok(result)
    }
    pub(super) fn validate(&self, body: &str, sources: &[Source], terms: &Terms) -> Result<()> {
        for claim in frames(body, terms)? {
            let Some(evidence) = self.fields.get(&claim.field) else {
                continue;
            };
            let Some(modal_seq) = evidence.modal_seq else {
                continue;
            };
            if claim.state == Some(State::Modal) {
                continue;
            }
            ensure!(
                self.same_scope
                    && claim.state == Some(State::Definite)
                    && evidence.state == Some(State::Definite)
                    && evidence.latest_seq > modal_seq
                    && sources
                        .iter()
                        .any(|s| s.segment_id == evidence.id && s.revision == evidence.revision),
                Mismatch(claim.field)
            );
        }
        Ok(())
    }
}
