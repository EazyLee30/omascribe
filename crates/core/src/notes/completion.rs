//! Retire only a whole, simple pending-task passage covered by a later explicit
//! completion of the same literal task, including its stated owner and date.
//! Raw transcript and citation revisions remain unchanged. Ambiguous passages
//! stay on the normal source path; this is not a general task/fact extractor.
use crate::Segment;
use anyhow::{bail, Result};
use regex::Regex;
use std::{collections::BTreeSet, sync::OnceLock};

#[derive(Default)]
pub(super) struct View {
    retired: BTreeSet<usize>,
    tasks: BTreeSet<String>,
}
struct Claim {
    task: String,
    owner: Option<String>,
    date: Option<String>,
}
fn clauses(text: &str) -> impl Iterator<Item = &str> {
    text.split(['.', '。', ';', '；', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
}
fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
pub(super) fn unfinished(text: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)未完成|还没有完成|还没完成|待完成|\bnot(?:\s+(?:yet|been)){0,2}\s+(?:completed?|finished)\b|\bhasn['’]t\s+been\s+(?:completed|finished)\b|\b(?:still\s+pending|incomplete|unfinished)\b").unwrap()).is_match(text)
        || bare_unfinished_assertion(text)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IncompleteLanguage {
    English,
    Chinese,
}

/// Closed incompleteness assertions only. Callers mask protected names first;
/// an erased subject or predicate cannot become a witness. Every marked state
/// clause must fit this same predicate, so another unfinished task abstains.
fn incomplete_transcription_language(text: &str) -> Option<IncompleteLanguage> {
    static ENGLISH: OnceLock<Regex> = OnceLock::new();
    static CHINESE: OnceLock<Regex> = OnceLock::new();
    static NON_ASSERTIVE: OnceLock<Regex> = OnceLock::new();
    static PENDING: OnceLock<Regex> = OnceLock::new();
    let english = ENGLISH.get_or_init(|| {
        Regex::new(r"(?i)^(?:the\s+)?(?:transcription\s+is|transcriptions\s+are)\s+incomplete$")
            .unwrap()
    });
    let chinese = CHINESE.get_or_init(|| Regex::new(r"^(?:语音转录|转录|转写)\s*不完整$").unwrap());
    let non_assertive = NON_ASSERTIVE.get_or_init(|| {
        Regex::new(concat!(
            r"(?i)\b(?:if|unless|when|once|before|until|provided|suppose|assuming|maybe|perhaps|possibly|might|may|could|would|should|will|shall)\b",
            r"|如果|假如|倘若|除非|只要|只有|一旦|要是|即使|即便|是否|能否|可能|也许|假设"
        )).unwrap()
    });
    let pending = PENDING.get_or_init(|| Regex::new(r"(?i)\bpending\b").unwrap());
    // A quotation or conditional may span sentence and line boundaries. This
    // narrow fallback abstains for the whole supplied evidence rather than
    // turning an inner sentence into an independent assertion.
    if text.contains([
        '?', '？', '"', '\'', '“', '”', '‘', '’', '「', '」', '『', '』', '`', '#',
    ]) || non_assertive.is_match(text)
    {
        return None;
    }
    let mut language = None;
    for sentence in text.split_inclusive(['。', '.', '!', '！', '?', '？', '\n']) {
        let sentence = sentence.trim();
        for clause in sentence
            .trim_end_matches(['。', '.', '!', '！', '?', '？', '\n'])
            .split([',', '，', ';', '；'])
            .map(str::trim)
        {
            if !unfinished(clause)
                && !clause.contains("不完整")
                && !clause.contains("尚待")
                && !pending.is_match(clause)
            {
                continue;
            }
            let current = if english.is_match(clause) {
                IncompleteLanguage::English
            } else if chinese.is_match(clause) {
                IncompleteLanguage::Chinese
            } else {
                return None;
            };
            if language.is_some_and(|previous| previous != current) {
                return None;
            }
            language = Some(current);
        }
    }
    language
}

/// A bounded cross-language fallback for the shared source-evidence validator.
/// It does not widen `unfinished`, retire tasks, or infer arbitrary subjects.
pub(super) fn incomplete_transcription_covers(source_mask: &str, target_mask: &str) -> bool {
    matches!(
        (
            incomplete_transcription_language(source_mask),
            incomplete_transcription_language(target_mask),
        ),
        (
            Some(IncompleteLanguage::English),
            Some(IncompleteLanguage::Chinese)
        ) | (
            Some(IncompleteLanguage::Chinese),
            Some(IncompleteLanguage::English)
        )
    )
}

fn bare_unfinished_assertion(text: &str) -> bool {
    if !text.contains("没有完成") {
        return false;
    }
    static CONDITIONAL: OnceLock<Regex> = OnceLock::new();
    let conditional = CONDITIONAL.get_or_init(|| {
        Regex::new(r"(?i)\b(?:if|unless|when|once|before|until|provided)\b|如果|假如|倘若|除非|只要|只有|一旦|要是|即使|即便|是否|能否").unwrap()
    });
    // This admits only an explicit local assertion, not a new general task
    // parser. Keep preceding conditional context in the same sentence: a comma
    // or semicolon after 如果 does not turn its consequence into a current fact.
    text.split_inclusive(['。', '.', '!', '！', '?', '？', '\n'])
        .any(|sentence| {
            if sentence.contains(['?', '？']) || conditional.is_match(sentence) {
                return false;
            }
            sentence.split([',', '，', ';', '；']).any(|clause| {
                let Some(object) = clause.trim().strip_prefix("没有完成") else {
                    return false;
                };
                let object = object.trim_end_matches(['。', '.', '!', '！', '\n']).trim();
                object.chars().any(char::is_alphanumeric)
                    && !object.contains(['就', '前', '才'])
                    && !object.contains("的话")
                    && !object.ends_with(['吗', '么', '呢'])
            })
        })
}
fn pending(text: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(concat!(
        r"(?i)^(?:(?:the\s+)?(?P<en>[A-Za-z0-9][A-Za-z0-9 _-]{1,79}?)\s+",
        r"(?:is\s+(?:not(?:\s+yet)?|still\s+not)\s+(?:completed|complete|finished)|has\s+not\s+(?:yet\s+)?been\s+(?:completed|finished)|hasn['’]t\s+been\s+(?:completed|finished))",
        r"|(?P<zh>[\u{3400}-\u{9fff}A-Za-z0-9 _-]{2,60}?)(?:尚未完成|还没有完成|还没完成|未完成))$"
    )).unwrap());
    let c = re.captures(text)?;
    Some(normalize(c.name("en").or_else(|| c.name("zh"))?.as_str()))
}
fn old_claim(text: &str) -> Option<Claim> {
    let mut parts = clauses(text);
    let task = pending(parts.next()?)?;
    let mut claim = Claim {
        task,
        owner: None,
        date: None,
    };
    if let Some(assignment) = parts.next() {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(concat!(
            r"(?i)^(?P<owner>[A-Za-z][A-Za-z'’ -]{0,60}?)\s+will\s+(?:run|perform|complete|finish|verify)\s+",
            r"(?:it|the\s+(?P<task>[A-Za-z0-9][A-Za-z0-9 _-]{1,79}?))(?:\s+on\s+(?P<date>[A-Za-z0-9 -]{1,40}))?$"
        )).unwrap());
        let c = re.captures(assignment)?;
        if c.name("task")
            .is_some_and(|m| normalize(m.as_str()) != claim.task)
        {
            return None;
        }
        claim.owner = Some(normalize(&c["owner"]));
        claim.date = c.name("date").map(|m| normalize(m.as_str()));
    }
    parts.next().is_none().then_some(claim)
}
fn completed(text: &str) -> Option<Claim> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(concat!(
        r"(?i)^(?:(?:(?P<owner>[A-Za-z][A-Za-z'’ -]{0,60}?)\s+(?:confirms?|confirmed|reports?|reported)\s+(?:that\s+)?)?",
        r"(?:the\s+)?(?P<en>[A-Za-z0-9][A-Za-z0-9 _-]{1,79}?)\s+(?:has\s+(?:now\s+)?been|is\s+now)\s+(?:completed|finished|complete)(?:\s+successfully)?(?:\s+on\s+(?P<date>[A-Za-z0-9 -]{1,40}))?",
        r"|(?P<zh>[\u{3400}-\u{9fff}A-Za-z0-9 _-]{2,60}?)(?:现已完成|已经完成|已完成))$"
    )).unwrap());
    let lower = normalize(text);
    if [
        "if ", "when ", "once ", "unless ", "如果", "假如", "等", "待",
    ]
    .iter()
    .any(|p| lower.starts_with(p))
    {
        return None;
    }
    let c = re.captures(text)?;
    Some(Claim {
        task: normalize(c.name("en").or_else(|| c.name("zh"))?.as_str()),
        owner: c.name("owner").map(|m| normalize(m.as_str())),
        date: c.name("date").map(|m| normalize(m.as_str())),
    })
}
pub(super) fn view(segments: &[Segment]) -> View {
    let mut result = View::default();
    let Some(first) = segments.first() else {
        return result;
    };
    if segments
        .iter()
        .any(|s| s.meeting_id != first.meeting_id || s.run != first.run)
    {
        return result;
    }
    for (index, s) in segments.iter().enumerate() {
        let Some(old) = old_claim(&s.text) else {
            continue;
        };
        // Any newer, unparsed mention is ambiguous. A pending/reopened task must
        // not inherit an older completion merely because its words also match.
        let Some(latest) = segments
            .iter()
            .filter(|n| n.seq > s.seq && normalize(&n.text).contains(&old.task))
            .max_by_key(|n| n.seq)
        else {
            continue;
        };
        if normalize(&latest.text).matches(&old.task).count() != 1 {
            continue;
        }
        let Some(new) = clauses(&latest.text)
            .filter_map(completed)
            .find(|c| c.task == old.task)
        else {
            continue;
        };
        if old
            .owner
            .as_ref()
            .is_some_and(|o| Some(o) != new.owner.as_ref())
            || old
                .date
                .as_ref()
                .is_some_and(|d| Some(d) != new.date.as_ref())
        {
            continue;
        }
        result.retired.insert(index);
        result.tasks.insert(old.task);
    }
    result
}
impl View {
    pub(super) fn retired(&self, index: usize) -> bool {
        self.retired.contains(&index)
    }
    pub(super) fn validate(&self, body: &str) -> Result<()> {
        for task in &self.tasks {
            let escaped = regex::escape(task);
            let now = Regex::new(&format!(
                r"(?i){escaped}\s*(?:has\s+(?:now\s+)?been|is(?:\s+now)?|现已|已经|已)\s*(?:successfully\s+)?(?:completed|complete|finished|完成)"
            ))?;
            let stale = Regex::new(&format!(
                r"(?i){escaped}\s*(?:is\s+(?:not(?:\s+yet)?|still\s+not)\s+(?:completed|complete|finished)|has\s+not\s+(?:yet\s+)?been\s+(?:completed|finished)|hasn['’]t\s+been\s+(?:completed|finished)|尚未完成|还没有完成|还没完成|未完成)"
            ))?;
            if !now.is_match(body) || stale.is_match(body) {
                bail!("纪要未采用同一任务的最新完成状态，原始转写已保留；本轮未提交");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(seq: u32, text: &str) -> Segment {
        Segment {
            id: format!("m:1:{seq}"),
            meeting_id: "m".into(),
            run: 1,
            seq,
            start_ms: 0,
            end_ms: 1,
            text: text.into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        }
    }

    #[test]
    fn incomplete_transcription_covers_the_actual_draft_and_closed_subjects() {
        let source = "The transcriptions are incomplete. The report is not approved.";
        let target = "语音转录不完整，报告未批准。";
        assert!(incomplete_transcription_covers(source, target));
        assert!(incomplete_transcription_covers(target, source));
        for english in [
            "The transcriptions are incomplete.",
            "Transcriptions are incomplete.",
            "The transcription is incomplete.",
            "\tThe transcriptions are incomplete.\r\n",
        ] {
            for chinese in ["语音转录不完整。", "转录不完整。", "转写不完整。"]
            {
                assert!(incomplete_transcription_covers(english, chinese));
                assert!(incomplete_transcription_covers(chinese, english));
            }
        }
        assert!(incomplete_transcription_covers(
            "The transcriptions are incomplete. Transcriptions are incomplete.",
            "语音转录不完整。"
        ));
    }

    #[test]
    fn incomplete_transcription_cannot_waive_another_unfinished_source_clause() {
        for other in [
            "The invoice review is not completed.",
            "The upload verification has not yet been completed.",
            "The upload is still pending.",
            "Upload is pending.",
            "The transcriptions are unfinished.",
            "The transcriptions are not completed.",
            "没有完成系统音频采集。",
            "报告不完整。",
            "上传尚待完成。",
        ] {
            for separator in [" ", ", ", "; "] {
                let source = format!("The transcriptions are incomplete{separator}{other}");
                assert!(
                    !incomplete_transcription_covers(&source, "语音转录不完整。"),
                    "{source}"
                );
            }
        }
        assert!(!incomplete_transcription_covers(
            "The transcriptions are incomplete. The invoice review is not completed.",
            "语音转录不完整。"
        ));
        assert!(!incomplete_transcription_covers(
            "语音转录不完整，发票审核尚未完成。",
            "The transcriptions are incomplete."
        ));
    }

    #[test]
    fn incomplete_transcription_rejects_unknown_nonassertive_or_changed_predicates() {
        let valid_source = "The transcriptions are incomplete. The report is not approved.";
        let valid_target = "语音转录不完整，报告未批准。";
        for source in [
            "",
            "The report is not approved.",
            "The transcriptions are complete.",
            "The transcriptions are not incomplete.",
            "The transcriptions are incomplete?",
            "Are the transcriptions incomplete?",
            "The transcriptions may be incomplete.",
            "If the report is approved, the transcriptions are incomplete.",
            "If the report is approved; the transcriptions are incomplete.",
            "If the report is approved,\nthe transcriptions are incomplete.",
            "Incomplete are the transcriptions.",
            "The report is incomplete.",
            "The incomplete transcriptions are complete.",
            "No one says the transcriptions are incomplete.",
            "“The transcriptions are incomplete.”",
            "'The transcriptions are incomplete.'",
            "“Quoted text. The transcriptions are incomplete. End quote.”",
        ] {
            assert!(
                !incomplete_transcription_covers(source, valid_target),
                "{source}"
            );
        }
        for target in [
            "",
            "报告未批准。",
            "语音转录完整，报告未批准。",
            "报告不完整且未批准。",
            "如果报告未批准，语音转录不完整。",
            "如果报告未批准；语音转录不完整。",
            "如果报告未批准，\n语音转录不完整。",
            "语音转录不完整？",
            "语音转录不完整吗。",
            "语音转录可能不完整。",
            "语音转录并非不完整。",
            "不完整的是语音转录。",
            "“语音转录不完整”，报告未批准。",
            "“引用开始。语音转录不完整。引用结束。”",
            "语音转录不完整是一个项目名。",
            "语音转录不完整，报告也不完整。",
            "语音转录不完整，发票审核未完成。",
        ] {
            assert!(
                !incomplete_transcription_covers(valid_source, target),
                "{target}"
            );
        }
    }

    #[test]
    fn incomplete_transcription_needs_an_unmasked_subject_and_status_without_widening_unfinished() {
        let source = "The transcriptions are incomplete.";
        let target = "语音转录不完整。";
        for masked_source in [
            "The                are incomplete.",
            "The transcriptions are           .",
            "                                 .",
        ] {
            assert!(!incomplete_transcription_covers(masked_source, target));
        }
        for masked_target in [
            "             不完整。报告未批准。",
            "语音转录          ，报告未批准。",
            "                          ，报告未批准。",
        ] {
            assert!(!incomplete_transcription_covers(source, masked_target));
        }
        assert!(unfinished(source));
        assert!(!unfinished(target));
        assert!(unfinished("If the transcriptions are incomplete, wait."));
    }

    #[test]
    fn bare_unfinished_requires_an_assertive_local_clause() {
        for text in [
            "没有完成系统音频采集。",
            "今天只讨论这些验收要求，没有完成系统音频采集，也没有确定新版本发布日期。",
            "周五不是截止日期，没有完成库存复核。",
            "是否继续测试？没有完成库存复核。",
        ] {
            assert!(unfinished(text), "{text}");
        }
        for text in [
            "没有完成。",
            "没有完成——。",
            "没有完成系统音频采集就不能发布。",
            "没有完成系统音频采集前先不要发布。",
            "没有完成系统音频采集才继续测试。",
            "没有完成系统音频采集的话先不要发布。",
            "如果预算不批，没有完成系统音频采集，也不要发布。",
            "假如预算不批；没有完成系统音频采集也不要发布。",
            "If the budget is rejected，没有完成系统音频采集就不要发布。",
            "没有完成系统音频采集吗？",
            "没有完成系统音频采集吗。",
            "系统音频采集没有完成。",
            "不是没有完成系统音频采集。",
        ] {
            assert!(!unfinished(text), "{text}");
        }
        // New bare-form admission does not narrow any existing pattern.
        assert!(unfinished("如果上传还没有完成，就不要发布。"));
        assert!(unfinished("The invoice review is not yet completed."));
    }
    #[test]
    fn only_a_later_explicit_same_task_completion_retires_the_old_state_and_plan() {
        let sources = vec![source(1,"The upload verification is not completed. Morgan will run it on Thursday."),source(2,"The invoice review is not completed. Riley will run it on Friday."),source(3,"Morgan confirms the upload verification has now been completed successfully on Thursday. There are no remaining upload tasks.")];
        let original = sources[0].text.clone();
        let current = view(&sources);
        assert!(current.retired(0));
        assert!(!current.retired(1));
        assert_eq!(sources[0].text, original);
        assert!(current.validate("Morgan confirms the upload verification has now been completed on Thursday. The invoice review is not completed.").is_ok());
        assert!(current
            .validate("The upload verification is not completed.")
            .is_err());
        assert!(current
            .validate("The invoice review has been completed.")
            .is_err());
    }
    #[test]
    fn missing_identity_owner_date_and_conditional_or_later_reopening_abstain() {
        for new in [
            "Morgan confirms the invoice review has now been completed successfully on Thursday.",
            "Riley confirms the upload verification has now been completed successfully on Thursday.",
            "Morgan confirms the upload verification has now been completed successfully on Friday.",
            "The upload verification has now been completed successfully on Thursday.",
            "If Morgan confirms the upload verification has now been completed successfully on Thursday.",
            "Morgan confirms the upload verification has now been completed successfully on Thursday?",
        ] {
            assert!(!view(&[source(1,"The upload verification is not completed. Morgan will run it on Thursday."),source(2,new)]).retired(0), "{new}");
        }
        let mut sources = vec![
            source(1, "上传验证尚未完成。"),
            source(2, "上传验证现已完成。"),
        ];
        assert!(view(&sources).retired(0));
        assert!(view(&sources).validate("上传验证现已完成。").is_ok());
        sources.push(source(3, "上传验证还没有完成。"));
        assert!(!view(&sources).retired(0));
        sources.pop();
        sources[1].run = 2;
        assert!(!view(&sources).retired(0));
    }
}
