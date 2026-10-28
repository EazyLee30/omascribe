//! Delete a provably added week modifier from generated weekday prose only.
//! This does not resolve dates, choose a week, or change source/user text.
use anyhow::Result;
use regex::Regex;
use std::{ops::Range, sync::OnceLock};

const MAX_BYTES: usize = 16_384;

#[derive(Debug)]
pub(crate) struct Mismatch;
impl Mismatch {
    pub(crate) fn hint(&self) -> &'static str {
        " Preserve the stated weekday without choosing a week. A source saying only on Tuesday does not support this/next/last Tuesday or 本周/下周/上周二. Remove only the unsupported week modifier; preserve the weekday, action, owner, negation and all other clauses."
    }
}
impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.hint().trim())
    }
}
impl std::error::Error for Mismatch {}

pub(crate) struct Repair {
    pub(crate) text: String,
    pub(crate) removed_qualifiers: usize,
}

fn mask_matches(text: &str, mask: &str) -> bool {
    text.len() == mask.len()
        && text
            .bytes()
            .zip(mask.bytes())
            .all(|(a, b)| a == b || b == b' ')
}

struct Weekday {
    span: Range<usize>,
    day: u8,
}
fn single_weekday(text: &str) -> Option<Weekday> {
    static DAY: OnceLock<Regex> = OnceLock::new();
    let pattern = DAY.get_or_init(|| Regex::new(r"(?i)(?-u:\b)(?:Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday)(?-u:\b)|(?:周|星期|礼拜)[一二三四五六日天]").unwrap());
    let mut days = pattern.find_iter(text);
    let found = days.next()?;
    if days.next().is_some() {
        return None;
    }
    // Reuse the existing weekday mapping only on an exact, bounded token.
    // Its relative-week equivalences never establish week evidence here.
    let normalized = super::validation::normalized_time(found.as_str());
    let suffix = normalized
        .strip_prefix("周")
        .or_else(|| normalized.strip_prefix("星期"))
        .or_else(|| normalized.strip_prefix("礼拜"))?;
    let day = match suffix {
        "一" => 1,
        "二" => 2,
        "三" => 3,
        "四" => 4,
        "五" => 5,
        "六" => 6,
        "日" | "天" => 7,
        _ => return None,
    };
    Some(Weekday {
        span: found.range(),
        day,
    })
}

fn clause(text: &str, span: &Range<usize>) -> Range<usize> {
    let boundary = |c: char| ".。!?！？;；\n".contains(c);
    let start = text[..span.start]
        .rfind(boundary)
        .map_or(0, |i| i + text[i..].chars().next().unwrap().len_utf8());
    let end = text[span.end..]
        .find(boundary)
        .map_or(text.len(), |i| span.end + i);
    start..end
}

fn week_or_date(text: &str) -> bool {
    static EVIDENCE: OnceLock<Regex> = OnceLock::new();
    EVIDENCE.get_or_init(|| Regex::new(concat!(
        r"(?i)\b(?:this|next|last|coming|following|previous|current)\s+(?:week|Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday)\b",
        r"|\b(?:weeks?|weekly|every|each|alternat(?:e|ing)|today|tomorrow|yesterday)\b",
        r"|(?:本|这|上|下)(?:个)?(?:周|星期|礼拜)|(?:今|明|昨|前|后)天|今日|昨日|翌日|(?:本|这|上|下)月",
        r"|[0-9零〇一二两三四五六七八九十]{1,4}\s*(?:年|月|日|号|周)",
        r"|[0-9]{1,4}[-/.][0-9]{1,2}|\b(?:19|20)[0-9]{6}\b|\b[0-9]{1,2}(?:st|nd|rd|th)\b",
        r"|\b(?:January|February|March|April|May|June|July|August|September|October|November|December)\b",
    )).unwrap()).is_match(text)
}
fn uncertain(text: &str) -> bool {
    static CUE: OnceLock<Regex> = OnceLock::new();
    CUE.get_or_init(|| Regex::new(r"(?i)\b(?:if|unless|when|once|after|before|until|provided|assuming|whether|maybe|perhaps|possibly|depending)\b|\b(?:subject\s+to|as\s+long\s+as|on\s+condition)\b|如果|假如|倘若|若|除非|只有|一旦|可能|也许|或许|取决于|条件|前提|(?:批准|确认|通过|验收)(?:后|之后)|(?:待|等)(?:批准|确认|验收|通过)").unwrap()).is_match(text)
}
fn negated(text: &str) -> bool {
    static NEGATION: OnceLock<Regex> = OnceLock::new();
    NEGATION.get_or_init(|| Regex::new(r"(?i)\b(?:not|no|never|neither|nor|without|instead|cancelled|canceled|withdrawn)\b|n['’]t\b|不|没|未|无|别|勿|取消|撤回|改为|改成|而非").unwrap()).is_match(text)
}
fn literal_or_name(text: &str) -> bool {
    static NAMING: OnceLock<Regex> = OnceLock::new();
    text.contains(['"', '\'', '‘', '’', '“', '”', '`', '「', '」', '『', '』'])
        || NAMING.get_or_init(|| Regex::new(r"(?i)\b(?:names?|named|called|labels?|titled)\b|姓名|名字|名叫|名为|命名|称为|标签|标题").unwrap()).is_match(text)
}
fn action() -> &'static Regex {
    static ACTION: OnceLock<Regex> = OnceLock::new();
    ACTION.get_or_init(|| Regex::new(r"(?:进行|测试|试验|检查|核对|复核|审核|讨论|提交|发送|交付|发布|完成|开始|结束|演示|上线|开会|参加|举办|见面)").unwrap())
}

/// Masks must preserve UTF-8 byte offsets and replace only source-backed name
/// spans by spaces. Ambiguous sources/clauses are left for ordinary validation;
/// abstention is never evidence that an arbitrary relative date is valid.
pub(crate) fn repair(
    body: &str,
    source: &str,
    body_name_mask: &str,
    source_name_mask: &str,
) -> Option<Repair> {
    if body.len() > MAX_BYTES
        || source.len() > MAX_BYTES
        || !mask_matches(body, body_name_mask)
        || !mask_matches(source, source_name_mask)
        || week_or_date(source_name_mask)
        || literal_or_name(source_name_mask)
        || literal_or_name(body_name_mask)
        || uncertain(source_name_mask)
        || uncertain(body_name_mask)
        || source_name_mask.contains(['?', '？'])
        || body_name_mask.contains(['?', '？'])
    {
        return None;
    }
    let source_day = single_weekday(source_name_mask)?;
    let body_day = single_weekday(body_name_mask)?;
    if source_day.day != body_day.day {
        return None;
    }
    let source_clause = clause(source_name_mask, &source_day.span);
    let body_clause = clause(body_name_mask, &body_day.span);
    if source_clause.len() > 512
        || body_clause.len() > 512
        || negated(&source_name_mask[source_clause.clone()])
        || negated(&body_name_mask[body_clause.clone()])
    {
        return None;
    }
    static ON: OnceLock<Regex> = OnceLock::new();
    if !ON
        .get_or_init(|| Regex::new(r"(?i)(?:\bon\s+|(?:在|于)\s*)$").unwrap())
        .is_match(&source_name_mask[source_clause.start..source_day.span.start])
    {
        return None;
    }
    static QUALIFIER: OnceLock<Regex> = OnceLock::new();
    let candidate = QUALIFIER
        .get_or_init(|| {
            Regex::new(
                r"(?P<week>本|这(?:个)?|上(?:个)?|下(?:个)?)(?:周|星期|礼拜)[一二三四五六日天]",
            )
            .unwrap()
        })
        .captures(body_name_mask)?;
    let whole = candidate.get(0)?;
    let modifier = candidate.name("week")?;
    if whole.end() != body_day.span.end || modifier.end() != body_day.span.start {
        return None;
    }
    let tail = body_name_mask[body_day.span.end..body_clause.end].trim_start();
    static LEADING_ACTION: OnceLock<Regex> = OnceLock::new();
    let leading = LEADING_ACTION.get_or_init(|| Regex::new(r"^(?:(?:将会|将|会|要|计划|打算|准备|预计|安排)\s*)?(?:进行|测试|试验|检查|核对|复核|审核|讨论|提交|发送|交付|发布|完成|开始|结束|演示|上线|开会|参加|举办|见面)").unwrap());
    let before = &body_name_mask[body_clause.start..modifier.start()];
    let trailing_date =
        tail.is_empty() && before.trim_end().ends_with(['在', '于']) && action().is_match(before);
    if !leading.is_match(tail) && !trailing_date {
        return None;
    }
    let mut checked = body_name_mask.to_owned();
    checked.replace_range(modifier.range(), "");
    if week_or_date(&checked) {
        return None;
    }
    let mut text = body.to_owned();
    text.replace_range(modifier.range(), "");
    Some(Repair {
        text,
        removed_qualifiers: 1,
    })
}

pub(crate) fn validate(
    body: &str,
    source: &str,
    body_name_mask: &str,
    source_name_mask: &str,
) -> Result<()> {
    if repair(body, source, body_name_mask, source_name_mask).is_some() {
        return Err(Mismatch.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const SOURCE: &str = "Riley will test keyboard accessibility on Tuesday. No accessibility test has been completed yet.";
    const BODY: &str = "Riley 本周二将测试键盘可访问性。尚未完成任何可访问性测试。";

    #[test]
    fn actual_added_week_is_rejected_then_only_its_modifier_is_removed() {
        let error = validate(BODY, SOURCE, BODY, SOURCE).unwrap_err();
        assert!(error.is::<Mismatch>());
        for modifier in ["本", "这", "上", "下"] {
            let body = BODY.replace("本", modifier);
            let repaired = repair(&body, SOURCE, &body, SOURCE).unwrap();
            assert_eq!(
                repaired.text,
                "Riley 周二将测试键盘可访问性。尚未完成任何可访问性测试。"
            );
            assert_eq!(repaired.removed_qualifiers, 1);
            assert!(validate(&repaired.text, SOURCE, &repaired.text, SOURCE).is_ok());
        }
        let source = "Riley将在周二测试键盘可访问性。尚未完成任何可访问性测试。";
        assert!(repair(BODY, source, BODY, source).is_some());
        let body = "Riley将测试键盘可访问性，安排在本周二。";
        assert_eq!(
            repair(body, SOURCE, body, SOURCE).unwrap().text,
            "Riley将测试键盘可访问性，安排在周二。"
        );
    }

    #[test]
    fn explicit_week_dates_multiple_or_different_days_are_not_rewritten() {
        for source in [
            "Riley will test on this Tuesday.",
            "Riley will test on next Tuesday.",
            "Riley will test on last Tuesday.",
            "Riley will test on Tuesday next week.",
            "Riley will test on Tuesday this week.",
            "Riley将在下周二测试。",
            "Riley将在这周二测试。",
            "Riley will test every Tuesday.",
            "Riley will test on Tuesday, October 28.",
            "Riley will test on Tuesday 2026-10-28.",
            "Riley将在十月二十八日周二测试。",
            "Riley will test on Tuesday. Morgan will check on Thursday.",
            "Riley will test on Tuesday and report on Tuesday.",
            "Riley will test on Wednesday.",
        ] {
            assert!(repair(BODY, source, BODY, source).is_none(), "{source}");
        }
        for body in [
            "Riley本周三将测试。",
            "Riley本周二将测试，下周完成审核。",
            "Riley本周二将测试，周四发送报告。",
            "Riley将在2026年本周二测试。",
            "Riley下下周二将测试。",
            "Riley周二将测试。",
        ] {
            assert!(repair(body, SOURCE, body, SOURCE).is_none(), "{body}");
        }
    }

    #[test]
    fn names_quotes_negation_and_unclear_conditions_are_not_modified() {
        for source in [
            "Riley will not test on Tuesday.",
            "If approved, Riley will test on Tuesday.",
            "Riley will test on Tuesday unless cancelled.",
            "Riley will test on Tuesday. If approved.",
            "When approved, Riley will test on Tuesday.",
            "Will Riley test on Tuesday?",
            "The label is called On Tuesday.",
            "Riley said \"test on Tuesday\".",
            "Riley said ‘test on Tuesday’.",
            "Riley used `on Tuesday`.",
            "TuesdayLabs will test.",
        ] {
            assert!(repair(BODY, source, BODY, source).is_none(), "{source}");
        }
        for body in [
            "Riley不是本周二测试。",
            "如果批准，Riley本周二将测试。",
            "Riley本周二不会测试。",
            "Riley说“本周二将测试”。",
            "本周二版将测试键盘。",
            "名字为本周二。",
        ] {
            assert!(repair(body, SOURCE, body, SOURCE).is_none(), "{body}");
        }
        let body = "本周二将测试键盘。";
        let mask = body.replace("本周二", &" ".repeat("本周二".len()));
        assert!(repair(body, SOURCE, &mask, SOURCE).is_none());
        let source = "The project 本周二 will test on Tuesday.";
        let source_mask = source.replace("本周二", &" ".repeat("本周二".len()));
        assert!(repair(body, source, &mask, &source_mask).is_none());
    }

    #[test]
    fn invalid_masks_and_oversized_inputs_abstain_without_truncation() {
        assert!(repair(BODY, SOURCE, "wrong", SOURCE).is_none());
        assert!(repair(BODY, SOURCE, BODY, &SOURCE.replace("Riley", "Alice")).is_none());
        let source = format!("{}{}", SOURCE, " ".repeat(MAX_BYTES));
        assert!(repair(BODY, &source, BODY, &source).is_none());
        let body = format!("{}{}", BODY, " ".repeat(MAX_BYTES));
        assert!(repair(&body, SOURCE, &body, SOURCE).is_none());
    }
}
