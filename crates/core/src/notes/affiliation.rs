//! Literal Chinese institution + observed reported-person affiliation veto.
//! This is not entity resolution: generic大学 stays literal, labels/aliases do
//! not merge, and each paragraph needs its own original source occurrence.
use crate::{Segment, Source};
use anyhow::{ensure, Result};
use regex::Regex;
use std::{collections::BTreeSet, ops::Range, sync::OnceLock};

const MAX_BYTES: usize = 120_000;
const MAX_FRAMES: usize = 64;

#[derive(Debug)]
struct Mismatch;
impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("纪要中的专有名称机构与报道人名配对没有当前段落原话支持；保留来源的完整字面配对，不猜机构或近似姓名")
    }
}
impl std::error::Error for Mismatch {}

struct Pair<'a> {
    institution: &'a str,
    person: &'a str,
    institution_range: Range<usize>,
    person_range: Range<usize>,
    title_end: usize,
    // A copied qualified frame may retain its exact wrapper; it cannot grant
    // an unqualified affiliation. Payload conditions after the title do not
    // qualify the institution/person relationship itself.
    qualified: Option<&'a str>,
}
struct Evidence<'a> {
    id: &'a str,
    revision: u32,
    pairs: Vec<Pair<'a>>,
}
pub(super) struct View<'a> {
    evidence: Vec<Evidence<'a>>,
    same_scope: bool,
}

fn relevant(text: &str) -> bool {
    (text.contains("大学") || text.contains("学院"))
        && (text.contains("教授") || text.contains("博士"))
}
fn quoted_prefix(text: &str) -> bool {
    text.char_indices().any(|(at, ch)| {
        matches!(
            ch,
            '"' | '“' | '”' | '‘' | '’' | '「' | '」' | '『' | '』' | '`'
        ) || ch == '\'' && !super::confirmation::word_apostrophe(text, at, ch)
    })
}

fn pairs(text: &str) -> Result<Vec<Pair<'_>>> {
    if !relevant(text) {
        return Ok(Vec::new());
    }
    ensure!(text.len() <= MAX_BYTES, "机构来源校验超过文本预算");
    let occurrences = crate::translation::reported_name_occurrences(text)?;
    static PREFIX: OnceLock<Regex> = OnceLock::new();
    static CONJOINED: OnceLock<Regex> = OnceLock::new();
    static NONASSERTIVE: OnceLock<Regex> = OnceLock::new();
    let prefix = PREFIX.get_or_init(|| {
        Regex::new(concat!(
        r#"(?:^|[ \t\n。！？!?；;，,、.：:“”‘’「」『』"`]|(?:教授|博士)[ \t]*(?:与|和|及))[ \t]*"#,
        r"(?P<qualification>如果|假如|若|可能|或许|也许|似乎|据称|据传|不是|并非)?[ \t]*",
        r"(?:来自)?(?P<institution>[\p{Han}]{0,12}(?:大学|学院))的[ \t]*$"
    )).unwrap()
    });
    let nonassertive=NONASSERTIVE.get_or_init(||Regex::new(r"(?i)\b(?:if|unless|when|whether|assuming|suppose|may|might)\b|如果|假如|除非|一旦|是否|能否|可能|或许|也许|似乎|据称|据传|未确认|不确定|不是|并非").unwrap());
    let conjoined = CONJOINED.get_or_init(|| Regex::new(
        r"(?:教授|博士)[ \t]*(?:与|和|及)[ \t]*(?P<institution>[\p{Han}]{0,12}(?:大学|学院))的[ \t]*$"
    ).unwrap());
    // Reported labels are opaque data, including labels such as MAY or IF.
    let mut syntax = text.as_bytes().to_vec();
    for range in &occurrences {
        syntax[range.clone()].fill(b'#');
    }
    let syntax = String::from_utf8(syntax).expect("whole labels masked as ASCII");
    static REPORT: OnceLock<Regex> = OnceLock::new();
    let report = REPORT.get_or_init(|| {
        Regex::new(r"^(?:教授|博士)[ \t]*(?:指出|表示|称|说|认为|提到|强调)").unwrap()
    });
    let sentences = super::confirmation::sentences(text);
    let mut result = Vec::new();
    for person in &occurrences {
        let Some(title) = ["教授", "博士"]
            .into_iter()
            .find(|title| text[person.end..].starts_with(*title))
        else {
            continue;
        };
        let head = &text[..person.start];
        let Some(capture) = conjoined.captures(head).or_else(|| prefix.captures(head)) else {
            continue;
        };
        let institution = capture.name("institution").expect("institution capture");
        // An unrecognized discourse wrapper is not part of an observed literal
        // institution. Abstain instead of inventing a longer institution label.
        if [
            "来自", "教授", "博士", "指出", "表示", "如果", "假如", "似乎", "可能", "据称", "据传",
        ]
        .iter()
        .any(|word| institution.as_str().contains(word))
        {
            continue;
        }
        let mut sentence_start = sentences
            .iter()
            .find(|r| r.contains(&institution.start()))
            .map_or(0, |r| r.start);
        // A previous person's reported payload can contain如果 without
        // qualifying the next independent affiliation after its comma. Outer
        // wrappers before that previous person still retain their scope.
        if let Some(previous) = occurrences
            .iter()
            .rev()
            .find(|r| r.end <= institution.start() && r.start >= sentence_start)
        {
            let outer = &text[sentence_start..previous.start];
            if !nonassertive.is_match(&syntax[sentence_start..previous.start])
                && !quoted_prefix(outer)
                && report.is_match(&text[previous.end..institution.start()])
            {
                if let Some((at, ch)) = text[previous.end..institution.start()]
                    .char_indices()
                    .rev()
                    .find(|(_, ch)| matches!(ch, '，' | ',' | '；' | ';'))
                {
                    sentence_start = previous.end + at + ch.len_utf8();
                }
            }
        }
        let before = &text[sentence_start..institution.start()];
        let quoted = quoted_prefix(before);
        let mut end = person.end + title.len();
        let tail = text[end..].trim_start_matches([' ', '\t']);
        let question = tail.starts_with(['?', '？']);
        if question {
            end += text[end..].len() - tail.len() + tail.chars().next().unwrap().len_utf8();
        }
        let qualified = (quoted
            || question
            || capture.name("qualification").is_some()
            || nonassertive.is_match(&syntax[sentence_start..institution.start()]))
        .then_some(&text[sentence_start..end]);
        ensure!(result.len() < MAX_FRAMES, "机构来源校验超过断言预算");
        result.push(Pair {
            institution: institution.as_str(),
            person: &text[person.clone()],
            institution_range: institution.range(),
            person_range: person.clone(),
            title_end: person.end + title.len(),
            qualified,
        });
    }
    Ok(result)
}

impl<'a> View<'a> {
    pub(super) fn new(segments: &'a [Segment], meeting: &str) -> Result<Self> {
        ensure!(
            segments
                .iter()
                .filter(|s| relevant(&s.text))
                .try_fold(0usize, |n, s| n.checked_add(s.text.len()))
                .is_some_and(|n| n <= MAX_BYTES),
            "机构来源校验超过来源预算"
        );
        let same_scope = segments.first().is_some_and(|first| {
            segments
                .iter()
                .all(|s| s.meeting_id == meeting && s.run == first.run)
        });
        let mut evidence = Vec::new();
        let mut frames = 0usize;
        for source in segments {
            let pairs = pairs(&source.text)?;
            frames = frames
                .checked_add(pairs.len())
                .filter(|n| *n <= MAX_FRAMES)
                .ok_or_else(|| anyhow::anyhow!("机构来源校验超过来源断言预算"))?;
            evidence.push(Evidence {
                id: &source.id,
                revision: source.revision,
                pairs,
            });
        }
        Ok(Self {
            evidence,
            same_scope,
        })
    }
    pub(super) fn validate(&self, body: &str, sources: &[Source]) -> Result<()> {
        for candidate in pairs(body)? {
            // Extra veto only for a person with an actually parsed original
            // pair (including the explicit generic大学). Unknown source forms
            // retain the old guards, never become guessed affiliation evidence.
            if !self
                .evidence
                .iter()
                .any(|e| e.pairs.iter().any(|p| p.person == candidate.person))
            {
                continue;
            }
            ensure!(
                self.same_scope
                    && self.evidence.iter().any(|e| {
                        sources
                            .iter()
                            .any(|s| s.segment_id == e.id && s.revision == e.revision)
                            && e.pairs.iter().any(|p| {
                                p.person == candidate.person
                                    && p.institution == candidate.institution
                                    && (p.qualified.is_none() || p.qualified == candidate.qualified)
                            })
                    }),
                Mismatch
            );
        }
        Ok(())
    }
}

/// Evidence for a single replacement in the ORIGINAL generated section body.
/// This is not a DirectCopy/translation bridge or a permanent name alias.
#[derive(Debug)]
pub(super) struct Edit {
    pub section_index: usize,
    pub body_range: Range<usize>,
    pub replacement: String,
    pub source_id: String,
    pub source_revision: u32,
    pub source_suffix_range: Range<usize>,
    pub suffix: String,
}
#[derive(Debug)]
pub(super) struct Recovery {
    pub draft: String,
    pub body: String,
    pub edits: Vec<Edit>,
}

pub(super) fn recovery_inputs_current(snapshot: &crate::Snapshot, selected: &[Segment]) -> bool {
    if selected.len() > MAX_FRAMES {
        return false;
    }
    let mut seen = BTreeSet::new();
    selected.iter().all(|s| {
        if !seen.insert(s.id.as_str())
            || s.meeting_id != snapshot.meeting.id
            || s.run != snapshot.meeting.run
        {
            return false;
        }
        let mut rows = snapshot
            .segments
            .iter()
            .filter(|current| current.id == s.id);
        let Some(current) = rows.next() else {
            return false;
        };
        rows.next().is_none()
            && current.meeting_id == s.meeting_id
            && current.run == s.run
            && current.seq == s.seq
            && current.revision == s.revision
            && current.text == s.text
            && current.start_ms == s.start_ms
            && current.end_ms == s.end_ms
            && current.speaker == s.speaker
            && current.speaker_manual == s.speaker_manual
    })
}

fn proper_institution(value: &str) -> bool {
    !["大学", "学院"].contains(&value)
        && ![
            "不", "未", "没", "非", "无", "否", "若", "可能", "似乎", "如果", "假如", "据称",
            "据传", "或许", "也许", "来自",
        ]
        .iter()
        .any(|cue| value.contains(cue))
}

// The report must be one bare, complete observed clause. Only its optional
// terminal full stop is presentation; questions, quotations and payload bytes
// are not normalized. In particular, the conditional risk payload remains data.
fn report_suffix(text: &str, pair: &Pair<'_>) -> Result<Option<Range<usize>>> {
    if pair.qualified.is_some() || !proper_institution(pair.institution) {
        return Ok(None);
    }
    let Some(clause) = super::confirmation::sentences(text)
        .into_iter()
        .find(|range| range.contains(&pair.institution_range.start))
    else {
        return Ok(None);
    };
    let mut end = clause.end;
    while end > clause.start && text[..end].ends_with(char::is_whitespace) {
        end -= text[..end].chars().next_back().unwrap().len_utf8();
    }
    if text[..end].ends_with('。') {
        end -= '。'.len_utf8();
    } else if text[..end].ends_with('.') {
        end -= 1;
    }
    if end < pair.title_end
        || !text[clause.start..pair.institution_range.start]
            .trim()
            .is_empty()
    {
        return Ok(None);
    }
    // The unchanged joiner and complete label must be the same literal field.
    if pair.person_range.start < pair.institution_range.end
        || text[pair.person_range.clone()] != *pair.person
        || text[pair.institution_range.end..pair.person_range.start].trim_end_matches([' ', '\t'])
            != "的"
    {
        return Ok(None);
    }
    let complete = &text[clause.start..end];
    if quoted_prefix(complete)
        || complete.matches("大学").count() + complete.matches("学院").count() != 1
        || crate::translation::reported_name_occurrences(complete)?.len() != 1
    {
        return Ok(None);
    }
    static REPORT: OnceLock<Regex> = OnceLock::new();
    let report = REPORT.get_or_init(|| Regex::new(r"^(?:指出|表示|称|说|认为|提到|强调)").unwrap());
    let Some(verb) = report.find(&text[pair.title_end..end]) else {
        return Ok(None);
    };
    if text[pair.title_end + verb.end()..end].trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(pair.institution_range.end..end))
}

/// Plan from original bytes only; callers apply all nonoverlapping edits once
/// and still run every ordinary final validator and Store/Scope protection.
pub(super) fn generated_recovery_edits(
    section_index: usize,
    body: &str,
    own: &[usize],
    snapshot: &crate::Snapshot,
    selected: &[Segment],
) -> Result<Vec<Edit>> {
    if !recovery_inputs_current(snapshot, selected)
        || snapshot.suggestions.iter().any(|old| old.text == body)
        || own.is_empty()
        || own.len() > selected.len()
        || own.iter().any(|i| *i == 0 || *i > selected.len())
        || own.iter().collect::<BTreeSet<_>>().len() != own.len()
    {
        return Ok(Vec::new());
    }
    let view = View::new(selected, &snapshot.meeting.id)?;
    let mut edits = Vec::new();
    for candidate in pairs(body)? {
        let Some(target_suffix) = report_suffix(body, &candidate)? else {
            continue;
        };
        let institutions = view
            .evidence
            .iter()
            .flat_map(|entry| &entry.pairs)
            .filter(|p| p.person == candidate.person)
            .map(|p| p.institution)
            .collect::<BTreeSet<_>>();
        if institutions.len() != 1 {
            continue;
        }
        let mut witnesses = Vec::new();
        for index in own {
            let source = &selected[index - 1];
            for original in &view.evidence[index - 1].pairs {
                if original.person != candidate.person
                    || original.institution.len() <= candidate.institution.len()
                    || !original.institution.ends_with(candidate.institution)
                {
                    continue;
                }
                let Some(suffix) = report_suffix(&source.text, original)? else {
                    continue;
                };
                if source.text[suffix.clone()] == body[target_suffix.clone()] {
                    witnesses.push((source, original, suffix));
                }
            }
        }
        if witnesses.len() != 1 {
            continue;
        }
        let (source, original, suffix) = witnesses.pop().unwrap();
        edits.push(Edit {
            section_index,
            body_range: candidate.institution_range,
            replacement: original.institution.to_owned(),
            source_id: source.id.clone(),
            source_revision: source.revision,
            source_suffix_range: suffix.clone(),
            suffix: source.text[suffix].to_owned(),
        });
    }
    Ok(edits)
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn recovery_report_suffix_keeps_the_complete_observed_conditional_payload() -> Result<()> {
        let source = super::super::modal_provenance_tests::ORIGINAL_SOURCES[3];
        let observed = pairs(source)?.pop().unwrap();
        let suffix = report_suffix(source, &observed)?.unwrap();
        assert_eq!(source[suffix.clone()].len(), 110);
        assert_eq!(
            &source[suffix],
            "的潘 MALE FERGUSON教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性"
        );
        Ok(())
    }
}
