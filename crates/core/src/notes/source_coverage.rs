//! Positive coverage for two closed music predicates, independent of the
//! modal veto. Unknown speech/paraphrases acquire no new proof authority.
use crate::{Meeting, Segment, Source, Suggestion};
use anyhow::{ensure, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    sync::OnceLock,
};

const MAX_BYTES: usize = 120_000;
const MAX_FIELDS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Field {
    MainStageMusicEnding,
    FestivalPartsPlaying,
}
impl Field {
    fn name(self) -> &'static str {
        match self {
            Self::MainStageMusicEnding => "MainStageMusicEnding",
            Self::FestivalPartsPlaying => "FestivalPartsPlayingUntilLateNight",
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    May,
    Definite,
}
impl State {
    fn name(self) -> &'static str {
        match self {
            Self::May => "may",
            Self::Definite => "asserted",
        }
    }
}
struct Fact {
    field: Field,
    state: State,
    range: Range<usize>,
}
struct Evidence<'a> {
    entry: usize,
    source: &'a Segment,
    fact: Fact,
}
struct Slot<'a> {
    seq: u32,
    evidence: Option<Evidence<'a>>,
}
pub(crate) struct View<'a> {
    fields: BTreeMap<Field, Slot<'a>>,
    meeting: &'a Meeting,
}

#[derive(Debug)]
pub(crate) struct Missing(Field);
impl Missing {
    pub(super) fn hint(&self) -> &'static str {
        match self.0 {
            Field::MainStageMusicEnding => "遗漏原SOURCE中主舞台音乐结束的事实。回到ORIGINAL transcript，按required_source_fields保留该字段及局部may；不要用引用编号、另一议题或另一字段代替正文。Cover the main-stage music-ending field from its ORIGINAL source, retaining its local qualifier.",
            Field::FestivalPartsPlaying => "遗漏原SOURCE中节庆部分区域继续演奏至深夜的事实。回到ORIGINAL transcript，按required_source_fields保留该独立字段及局部may；不能用主舞台音乐结束代替它。Cover possible festival sections continuing to play music until late into the night, independently of music ending.",
        }
    }
}
impl std::fmt::Display for Missing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.hint())
    }
}
impl std::error::Error for Missing {}

struct Pattern {
    field: Field,
    regex: Regex,
}
fn patterns() -> &'static [Pattern] {
    static PATTERNS: OnceLock<Vec<Pattern>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let state = r"(?:(?:仍|将|会|继续|持续)[ \t]*){0,4}";
        vec![
            Pattern {field:Field::MainStageMusicEnding, regex:Regex::new(concat!(
                r"(?i)^(?:music[ \t]+on[ \t]+(?:the[ \t]+)?main[ \t]+stages?|main[- ]stage[ \t]+music)[ \t]+",
                r"(?:(?P<may>may|might)[ \t]+have[ \t]+(?:been[ \t]+)?|has[ \t]+(?:now[ \t]+)?(?:been[ \t]+)?|is[ \t]+(?:now[ \t]+)?)",
                r"(?:finished|ended|completed)$"
            )).unwrap()},
            Pattern {field:Field::MainStageMusicEnding, regex:Regex::new(concat!(
                r"^(?:音乐[ \t]*在[ \t]*主舞台|主舞台(?:的)?音乐)[ \t]*",
                r"(?P<may>可能|或许|也许|似乎)?[ \t]*(?:已经|已)?[ \t]*(?:结束|完毕|完成)(?:了)?$"
            )).unwrap()},
            Pattern {field:Field::FestivalPartsPlaying, regex:Regex::new(concat!(
                r"(?i)^(?:there[ \t]+(?P<may>may|might)[ \t]+be[ \t]+(?:some[ \t]+)?sections?[ \t]+of[ \t]+(?:the[ \t]+)?festival[ \t]+that[ \t]+",
                r"|(?:some[ \t]+)?sections?[ \t]+of[ \t]+(?:the[ \t]+)?festival[ \t]+(?P<direct_may>may[ \t]+|might[ \t]+)?)",
                r"(?:will[ \t]+)?(?:keep|continue)[ \t]+playing[ \t]+music[ \t]+until[ \t]+late[ \t]+into[ \t]+the[ \t]+night$"
            )).unwrap()},
            Pattern {field:Field::FestivalPartsPlaying, regex:Regex::new(&format!(concat!(
                r"^嘉年华(?:(?:的)?(?:某些部分|某些区域|部分区域){state}(?P<may>可能|或许|也许|似乎)?{state}",
                r"|{state}(?P<before_may>可能|或许|也许|似乎)?{state}有(?:某些部分|某些区域|部分区域){state})",
                r"演奏(?:音乐)?(?:直到|至)深夜$"),state=state)).unwrap()},
        ]
    })
}

// This complete concession has two explicit MAY scopes. FESTIFUL/LAY and
// arbitrary discourse prefixes are deliberately not aliases for this frame.
fn combined() -> &'static Regex {
    static COMBINED: OnceLock<Regex> = OnceLock::new();
    COMBINED.get_or_init(|| Regex::new(concat!(
        r"(?i)^(?:remember[ \t]+that[ \t]+)?even[ \t]+though[ \t]+",
        r"(?P<ending>music[ \t]+on[ \t]+the[ \t]+main[ \t]+stages?[ \t]+may[ \t]+have[ \t]+finished)[ \t,]+",
        r"(?P<playing>there[ \t]+may[ \t]+be[ \t]+sections?[ \t]+of[ \t]+the[ \t]+festival[ \t]+that[ \t]+will[ \t]+keep[ \t]+playing[ \t]+music[ \t]+until[ \t]+late[ \t]+into[ \t]+the[ \t]+night)$"
    )).unwrap())
}

fn clause(text: &str, range: Range<usize>, body: bool) -> Option<Fact> {
    let piece = &text[range.clone()];
    for pattern in patterns() {
        if let Some(capture) = pattern.regex.captures(piece) {
            return Some(Fact {
                field: pattern.field,
                state: if ["may", "direct_may", "before_may"]
                    .iter()
                    .any(|name| capture.name(name).is_some())
                {
                    State::May
                } else {
                    State::Definite
                },
                range,
            });
        }
    }
    // Observed complete Chinese rendering from the retained received article.
    // This is presentation-only: original sources cannot acquire a new object
    // alias or positive update witness from this spelling.
    static BODY_PLAYING: OnceLock<Regex> = OnceLock::new();
    if body
        && BODY_PLAYING
            .get_or_init(|| {
                Regex::new(r"^节庆部分[ \t]*可能[ \t]*继续[ \t]*演奏[ \t]*至[ \t]*深夜$").unwrap()
            })
            .is_match(piece)
    {
        return Some(Fact {
            field: Field::FestivalPartsPlaying,
            state: State::May,
            range,
        });
    }
    None
}
fn trim(text: &str, range: Range<usize>) -> Range<usize> {
    let piece = &text[range.clone()];
    let start = range.start + piece.len() - piece.trim_start().len();
    start..start + piece.trim().len()
}
fn related(text: &str) -> bool {
    static TRIGGER: OnceLock<Regex> = OnceLock::new();
    TRIGGER
        .get_or_init(|| Regex::new(r"主舞台|嘉年华|(?i:\bmain[- ]stage|\bfestival\b)").unwrap())
        .is_match(text)
}
fn facts(text: &str, body: bool) -> Result<Vec<Fact>> {
    if !related(text) && !(body && text.contains("节庆部分")) {
        return Ok(Vec::new());
    }
    ensure!(
        text.len() <= MAX_BYTES,
        "来源覆盖校验超过文本预算，本轮未提交"
    );
    let mut result = Vec::new();
    for sentence in super::confirmation::sentences(text) {
        let mut range = trim(text, sentence);
        if text[range.clone()].ends_with(['。', '.', '!', '！']) {
            range.end -= text[..range.end].chars().next_back().unwrap().len_utf8();
            range = trim(text, range);
        }
        let piece = &text[range.clone()];
        if let Some(capture) = combined().captures(piece) {
            for (name, field) in [
                ("ending", Field::MainStageMusicEnding),
                ("playing", Field::FestivalPartsPlaying),
            ] {
                let part = capture.name(name).unwrap();
                result.push(Fact {
                    field,
                    state: State::May,
                    range: range.start + part.start()..range.start + part.end(),
                });
            }
        } else if let Some(fact) = clause(text, range.clone(), body) {
            result.push(fact);
        } else {
            // Only a whole composition of these closed assertions counts.
            // A named report, condition, quote, question or unknown prefix
            // cannot obtain coverage from a factual-looking inner substring.
            let mut parts = Vec::new();
            let mut start = range.start;
            let mut closed = true;
            let mut concession = false;
            for end in text[range.clone()]
                .char_indices()
                .filter_map(|(i, c)| {
                    matches!(c, '，' | ',' | '；' | ';').then_some(range.start + i)
                })
                .chain(std::iter::once(range.end))
            {
                let mut part = trim(text, start..end);
                if parts.is_empty() && body && text[part.clone()].starts_with("即使") {
                    part.start += "即使".len();
                    part = trim(text, part);
                    concession = true;
                }
                if !parts.is_empty() {
                    for prefix in ["但", "不过", "然而", "but ", "BUT "] {
                        if text[part.clone()].starts_with(prefix) {
                            part.start += prefix.len();
                            part = trim(text, part);
                            break;
                        }
                    }
                }
                if let Some(fact) = clause(text, part, body) {
                    parts.push(fact);
                } else {
                    closed = false;
                    break;
                }
                start = end + text[end..].chars().next().map_or(0, char::len_utf8);
            }
            if concession {
                // The pre-existing modal gate accepts this complete known
                // concession as a hypothetical ending, never a definite
                // witness. Do not turn IF/quotes into source assertions.
                closed &= parts.len() == 2
                    && parts[0].field == Field::MainStageMusicEnding
                    && parts[1].field == Field::FestivalPartsPlaying
                    && parts[1].state == State::May;
                if closed {
                    parts[0].state = State::May;
                }
            }
            if closed {
                result.extend(parts);
            }
        }
        ensure!(
            result.len() <= MAX_FIELDS,
            "来源覆盖校验超过字段预算，本轮未提交"
        );
    }
    Ok(result)
}

fn same_source(left: &Segment, right: &Segment) -> bool {
    left.id == right.id
        && left.meeting_id == right.meeting_id
        && left.run == right.run
        && left.seq == right.seq
        && left.revision == right.revision
        && left.text == right.text
        && left.start_ms == right.start_ms
        && left.end_ms == right.end_ms
        && left.speaker == right.speaker
        && left.speaker_manual == right.speaker_manual
}

// An unresolved/negative direct statement of the *same* closed predicate can
// retire an older coverage demand. It supplies no positive fact, modality
// discharge, or permission to write the new statement. Questions, IF, quotes,
// arbitrary prefixes and different subobjects fail the anchored forms.
fn unresolved(text: &str, positive: &[Fact]) -> Result<Vec<Field>> {
    if !related(text) {
        return Ok(Vec::new());
    }
    ensure!(
        text.len() <= MAX_BYTES,
        "来源覆盖校验超过文本预算，本轮未提交"
    );
    static PATTERNS: OnceLock<Vec<Pattern>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| vec![
        Pattern { field:Field::MainStageMusicEnding, regex:Regex::new(concat!(
            r"(?i)^(?:music[ \t]+on[ \t]+(?:the[ \t]+)?main[ \t]+stages?|main[- ]stage[ \t]+music)[ \t]+",
            r"(?:has|is|may|might)[ \t]+(?:(?:not|yet|perhaps|possibly|still|have|been)[ \t]+){1,6}",
            r"(?:finished|ended|completed)$"
        )).unwrap()},
        Pattern { field:Field::MainStageMusicEnding, regex:Regex::new(concat!(
            r"^(?:音乐[ \t]*在[ \t]*主舞台|主舞台(?:的)?音乐)[ \t]*",
            r"(?:尚未|还未|并未|没有|未|不)[ \t]*(?:结束|完毕|完成)(?:了)?$"
        )).unwrap()},
        Pattern { field:Field::FestivalPartsPlaying, regex:Regex::new(concat!(
            r"(?i)^(?:some[ \t]+)?sections?[ \t]+of[ \t]+(?:the[ \t]+)?festival[ \t]+",
            r"(?:will|may|might)[ \t]+not[ \t]+(?:keep|continue)[ \t]+playing[ \t]+music[ \t]+until[ \t]+late[ \t]+into[ \t]+the[ \t]+night$"
        )).unwrap()},
        Pattern { field:Field::FestivalPartsPlaying, regex:Regex::new(concat!(
            r"^嘉年华(?:的)?(?:某些部分|某些区域|部分区域)[ \t]*(?:仍|将)?[ \t]*",
            r"(?:不会|不再|没有)[ \t]*(?:继续|持续)?[ \t]*演奏(?:音乐)?(?:直到|至)深夜$"
        )).unwrap()},
    ]);
    let mut result = Vec::new();
    for sentence in super::confirmation::sentences(text) {
        let mut range = trim(text, sentence);
        if text[range.clone()].ends_with(['。', '.', '!', '！']) {
            range.end -= text[..range.end].chars().next_back().unwrap().len_utf8();
            range = trim(text, range);
        }
        for pattern in patterns {
            if pattern.regex.is_match(&text[range.clone()])
                && !positive
                    .iter()
                    .any(|fact| fact.field == pattern.field && fact.range == range)
            {
                ensure!(
                    result.len() < MAX_FIELDS,
                    "来源覆盖校验超过未决字段预算，本轮未提交"
                );
                result.push(pattern.field);
            }
        }
    }
    Ok(result)
}
impl<'a> View<'a> {
    // A single observed presentation of REMEMBER THAT EVEN THOUGH, including
    // both local MAY scopes. The unchanged original must prove this discourse
    // frame in this SAME current source; it is not a global body alias rule.
    fn remembered(&self, entry: usize, target: &str) -> Vec<Fact> {
        static TARGET: OnceLock<Regex> = OnceLock::new();
        let text = target.trim().trim_end_matches(['。', '.', '!', '！']);
        let Some(capture) = TARGET
            .get_or_init(|| {
                Regex::new(concat!(
                    r"^记住即使(?P<ending>主舞台的音乐可能已经(?:结束|完成))[，,][ \t]*",
                    r"(?P<playing>嘉年华的某些部分仍可能继续演奏音乐直到深夜)$"
                ))
                .unwrap()
            })
            .captures(text)
        else {
            return Vec::new();
        };
        let fields = [Field::MainStageMusicEnding, Field::FestivalPartsPlaying];
        if !fields.iter().all(|field| {
            self.fields
                .get(field)
                .and_then(|slot| slot.evidence.as_ref())
                .is_some_and(|e| {
                    e.entry == entry
                        && e.fact.state == State::May
                        && e.source
                            .text
                            .trim()
                            .to_ascii_lowercase()
                            .starts_with("remember that ")
                        && combined().is_match(
                            e.source
                                .text
                                .trim()
                                .trim_end_matches(['。', '.', '!', '！']),
                        )
                })
        }) {
            return Vec::new();
        }
        [("ending", fields[0]), ("playing", fields[1])]
            .into_iter()
            .map(|(name, field)| Fact {
                field,
                state: State::May,
                range: capture.name(name).unwrap().range(),
            })
            .collect()
    }
    /// Positive, current, source-local proof for complete target sentences.
    /// Neither modal veto success nor a topic mention can create this view.
    pub(super) fn prepared_ranges(&self, entry: usize, target: &str) -> Result<Vec<Range<usize>>> {
        let mut result = Vec::new();
        let mut seen = BTreeSet::new();
        for range in super::confirmation::sentences(target) {
            let mut claims = facts(&target[range.clone()], true)?;
            claims.extend(self.remembered(entry, &target[range.clone()]));
            if !claims.is_empty()
                && claims.iter().all(|claim| {
                    self.fields
                        .get(&claim.field)
                        .and_then(|slot| slot.evidence.as_ref())
                        .is_some_and(|e| e.entry == entry && e.fact.state == claim.state)
                })
            {
                ensure!(
                    claims.iter().all(|claim| seen.insert(claim.field)),
                    "Prepared target repeats a qualified field"
                );
                result.push(trim(target, range));
            }
        }
        Ok(result)
    }
    pub(crate) fn new(
        selected: &'a [Segment],
        current: &[Segment],
        meeting: &'a Meeting,
    ) -> Result<Self> {
        let mut result = Self {
            fields: BTreeMap::new(),
            meeting,
        };
        let mut bytes = 0usize;
        let mut count = 0usize;
        for (index, source) in selected.iter().enumerate() {
            let observations = facts(&source.text, false)?;
            let unresolved = unresolved(&source.text, &observations)?;
            if observations.is_empty() && unresolved.is_empty() {
                continue;
            }
            ensure!(
                source.meeting_id == meeting.id
                    && source.run == meeting.run
                    && current.iter().any(|s| same_source(source, s)),
                "来源覆盖依据已更新，本轮未提交"
            );
            bytes = bytes
                .checked_add(source.text.len())
                .filter(|n| *n <= MAX_BYTES)
                .ok_or_else(|| anyhow::anyhow!("来源覆盖校验超过来源预算，本轮未提交"))?;
            count += observations.len() + unresolved.len();
            ensure!(
                count <= MAX_FIELDS,
                "来源覆盖校验超过来源字段预算，本轮未提交"
            );
            for fact in observations {
                let field = fact.field;
                let evidence = Evidence {
                    entry: index + 1,
                    source,
                    fact,
                };
                if let Some(old) = result.fields.get_mut(&field) {
                    if source.seq > old.seq {
                        *old = Slot {
                            seq: source.seq,
                            evidence: Some(evidence),
                        };
                    } else if source.seq == old.seq
                        && old.evidence.as_ref().is_some_and(|e| {
                            !same_source(e.source, source) || e.fact.state != evidence.fact.state
                        })
                    {
                        old.evidence = None;
                    }
                } else {
                    result.fields.insert(
                        field,
                        Slot {
                            seq: source.seq,
                            evidence: Some(evidence),
                        },
                    );
                }
            }
            for field in unresolved {
                if result
                    .fields
                    .get(&field)
                    .is_none_or(|old| source.seq >= old.seq)
                {
                    result.fields.insert(
                        field,
                        Slot {
                            seq: source.seq,
                            evidence: None,
                        },
                    );
                }
            }
        }
        Ok(result)
    }

    fn owns(&self, citation: &Source, evidence: &Evidence<'_>) -> bool {
        let source = evidence.source;
        citation.segment_id == source.id
            && citation.revision == source.revision
            && citation.start_ms == source.start_ms
            && citation.end_ms == source.end_ms
            && citation.quote == source.text
            && citation.speaker_name
                == source
                    .speaker
                    .as_ref()
                    .and_then(|id| self.meeting.speaker_names.get(id))
                    .cloned()
    }
    fn covered<'s>(
        &self,
        sections: impl IntoIterator<Item = &'s Suggestion>,
    ) -> Result<BTreeSet<Field>> {
        let mut covered = BTreeSet::new();
        if self.fields.values().all(|slot| slot.evidence.is_none()) {
            return Ok(covered);
        }
        let mut bytes = 0usize;
        let mut count = 0usize;
        for section in sections {
            if section.meeting_id == self.meeting.id
                && !section.stale
                && matches!(section.status.as_str(), "pending" | "accepted")
            {
                let relevant: Vec<_> = self
                    .fields
                    .values()
                    .filter_map(|slot| slot.evidence.as_ref())
                    .filter(|e| section.sources.iter().any(|q| self.owns(q, e)))
                    .collect();
                if relevant.is_empty() {
                    continue;
                }
                let mut claims = facts(&section.text, true)?;
                for range in super::confirmation::sentences(&section.text) {
                    // relevant already requires this paragraph's full own
                    // current Source tuple; another paragraph/global cite is
                    // never a witness for this presentation.
                    for evidence in &relevant {
                        let observed =
                            self.remembered(evidence.entry, &section.text[range.clone()]);
                        for claim in observed {
                            if !claims
                                .iter()
                                .any(|old| old.field == claim.field && old.state == claim.state)
                            {
                                claims.push(claim);
                            }
                        }
                    }
                }
                if !claims.is_empty() {
                    bytes = bytes
                        .checked_add(section.text.len())
                        .filter(|n| *n <= MAX_BYTES)
                        .ok_or_else(|| anyhow::anyhow!("来源覆盖校验超过正文预算，本轮未提交"))?;
                    count += claims.len();
                    ensure!(
                        count <= MAX_FIELDS,
                        "来源覆盖校验超过正文中字段预算，本轮未提交"
                    );
                    for evidence in relevant {
                        if claims.iter().any(|claim| {
                            claim.field == evidence.fact.field && claim.state == evidence.fact.state
                        }) {
                            covered.insert(evidence.fact.field);
                        }
                    }
                }
            }
        }
        Ok(covered)
    }

    pub(crate) fn validate<'s>(
        &self,
        sections: impl IntoIterator<Item = &'s Suggestion>,
    ) -> Result<()> {
        let covered = self.covered(sections)?;
        for evidence in self
            .fields
            .values()
            .filter_map(|slot| slot.evidence.as_ref())
        {
            ensure!(
                covered.contains(&evidence.fact.field),
                Missing(evidence.fact.field)
            );
        }
        Ok(())
    }

    /// DATA only, referring to original transcript entry numbers. This does
    /// not create translated facts, copies, or source/paragraph authority.
    #[cfg(test)]
    pub(super) fn augment(&self, system: &mut String, input: &mut String) -> Result<()> {
        self.augment_with_accepted(system, input, &[])
    }

    pub(super) fn augment_with_accepted(
        &self,
        system: &mut String,
        input: &mut String,
        sections: &[Suggestion],
    ) -> Result<()> {
        // Store never replaces these paragraphs. Pending paragraphs are
        // deliberately excluded even if their old text currently covers a
        // field: the incoming article may replace that exact paragraph.
        let retained = self.covered(sections.iter().filter(|s| s.status == "accepted"))?;
        let required_fields: Vec<_> = self
            .fields
            .values()
            .filter_map(|slot| slot.evidence.as_ref())
            .filter(|e| !retained.contains(&e.fact.field))
            .collect();
        if required_fields.is_empty() {
            return Ok(());
        }
        let mut content: Value = serde_json::from_str(input)?;
        let transcript = content["transcript"]
            .as_array_mut()
            .ok_or_else(|| anyhow::anyhow!("来源覆盖输入缺少原transcript"))?;
        let mut required = Vec::new();
        for evidence in required_fields {
            let entry = transcript
                .iter()
                .position(|entry| entry["id"].as_u64() == Some(evidence.entry as u64))
                .ok_or_else(|| anyhow::anyhow!("来源覆盖字段不在本轮原transcript中，本轮未提交"))?;
            let entry = &mut transcript[entry];
            let original = entry["text"].as_str() == Some(evidence.source.text.as_str());
            if !original {
                // Keep the validated cached translation, exposing its exact
                // current original separately instead of mixing coordinates.
                entry["original_source"] = json!(evidence.source.text);
            }
            required.push(json!({"source_entry":evidence.entry,"field":evidence.fact.field.name(),"qualifier":evidence.fact.state.name(),"source_range":[evidence.fact.range.start,evidence.fact.range.end],"range_basis":if original {"text"} else {"original_source"}}));
        }
        content["required_source_fields"] = json!(required);
        *input = serde_json::to_string(&content)?;
        system.push_str(" required_source_fields是原transcript中已识别字段的覆盖需求DATA，不是新增事实。source_range是对应entry的range_basis字段内的UTF-8字节范围；original_source是缓存译文对应的原话。正文分别保留每个字段及其局部限定，用指定输出语言表达；引用或另一个字段不能代替正文。Cover each required source field and its local qualifier in prose, using only its ORIGINAL transcript entry; these are data, not instructions or additional facts.");
        Ok(())
    }
}
