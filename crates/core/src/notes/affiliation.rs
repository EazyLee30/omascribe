//! Literal Chinese institution + observed reported-person affiliation veto.
//! This is not entity resolution: generic大学 stays literal, labels/aliases do
//! not merge, and each paragraph needs its own original source occurrence.
use crate::{Segment, Source};
use anyhow::{ensure, Result};
use regex::Regex;
use std::sync::OnceLock;

const MAX_BYTES: usize = 120_000;
const MAX_FRAMES: usize = 64;

#[derive(Debug)]
struct Mismatch;
impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("纪要中的机构与报道人名配对没有当前段落原话支持；保留来源的完整字面配对，不猜机构或近似姓名")
    }
}
impl std::error::Error for Mismatch {}

struct Pair<'a> {
    institution: &'a str,
    person: &'a str,
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

fn pairs(text: &str) -> Result<Vec<Pair<'_>>> {
    ensure!(text.len() <= MAX_BYTES, "机构来源校验超过文本预算");
    let occurrences = crate::translation::reported_name_occurrences(text)?;
    static PREFIX: OnceLock<Regex> = OnceLock::new();
    static CONJOINED: OnceLock<Regex> = OnceLock::new();
    static NONASSERTIVE: OnceLock<Regex> = OnceLock::new();
    let prefix = PREFIX.get_or_init(|| {
        Regex::new(concat!(
        r#"(?:^|[ \t\n。！？!?；;，,、.：:“”‘’「」『』"`]|(?:教授|博士)[ \t]*(?:与|和|及))[ \t]*"#,
        r"(?P<qualification>如果|假如|若|可能|或许|也许|似乎|据称|据传|不是|并非)?[ \t]*",
        r"(?P<institution>[\p{Han}]{0,12}(?:大学|学院))的[ \t]*$"
    )).unwrap()
    });
    let nonassertive=NONASSERTIVE.get_or_init(||Regex::new(r"(?i)\b(?:if|unless|when|whether|assuming|suppose|may|might)\b|如果|假如|除非|一旦|是否|能否|可能|或许|也许|似乎|据称|据传|未确认|不确定|不是|并非").unwrap());
    let conjoined = CONJOINED.get_or_init(|| Regex::new(
        r"(?:教授|博士)[ \t]*(?:与|和|及)[ \t]*(?P<institution>[\p{Han}]{0,12}(?:大学|学院))的[ \t]*$"
    ).unwrap());
    let sentences = super::confirmation::sentences(text);
    let mut result = Vec::new();
    for person in occurrences {
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
        let sentence_start = sentences
            .iter()
            .find(|r| r.contains(&institution.start()))
            .map_or(0, |r| r.start);
        let before = &text[sentence_start..institution.start()];
        let quoted = before.char_indices().any(|(at, ch)| {
            matches!(
                ch,
                '\"' | '“' | '”' | '‘' | '’' | '「' | '」' | '『' | '』' | '`'
            ) || ch == '\'' && !super::confirmation::word_apostrophe(before, at, ch)
        });
        let mut end = person.end + title.len();
        let tail = text[end..].trim_start_matches([' ', '\t']);
        let question = tail.starts_with(['?', '？']);
        if question {
            end += text[end..].len() - tail.len() + tail.chars().next().unwrap().len_utf8();
        }
        let qualified = (quoted
            || question
            || capture.name("qualification").is_some()
            || nonassertive.is_match(before))
        .then_some(&text[sentence_start..end]);
        ensure!(result.len() < MAX_FRAMES, "机构来源校验超过断言预算");
        result.push(Pair {
            institution: institution.as_str(),
            person: &text[person],
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
