//! A conservative current-value view for an explicitly corrected, unqualified budget.
//! Only a simple older budget-only clause already covered by the later state may
//! leave the prompt. Owners, dates and other claims remain in the original evidence.
//! Original transcript/revisions are never changed. Ambiguous scopes stay on the
//! normal evidence path; this is not a general field extractor or fact ledger.
use crate::Segment;
use anyhow::{bail, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::{ops::Range, sync::OnceLock};

struct Amount {
    segment: usize,
    range: Range<usize>,
    clause: Range<usize>,
    pending: bool,
    value: String,
    unit: String,
    correction: bool,
}
pub(super) struct BudgetView {
    old: Vec<Amount>,
    latest: Amount,
    forbidden: Vec<String>,
}
fn numbers() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\d+(?:[.,]\d+)*").unwrap())
}
fn number(text: &str) -> String {
    text.replace(',', "")
}
fn boundary(c: char) -> bool {
    ['.', '。', '?', '？', '!', '！', ';', '；', '\n', '，'].contains(&c)
}
fn plain_scope(text: &str, at: usize, end: usize) -> bool {
    let prefix = text[..at].trim_end();
    let previous = prefix
        .rsplit(|c: char| !c.is_alphabetic())
        .next()
        .unwrap_or("")
        .to_lowercase();
    let after = text[end..].trim_start().to_lowercase();
    [
        "",
        "the",
        "a",
        "our",
        "this",
        "current",
        "previous",
        "old",
        "new",
        "initial",
        "tentative",
        "final",
        "and",
        "but",
        "最终",
        "当前",
        "之前",
        "原先",
        "新的",
        "旧的",
        "这个",
        "我们的",
        "更正",
        "把",
        "将",
    ]
    .contains(&previous.as_str())
        && !after.starts_with(['(', '（'])
        && !after.starts_with("for ")
}
fn amount_pattern() -> &'static Regex {
    static AMOUNT: OnceLock<Regex> = OnceLock::new();
    AMOUNT.get_or_init(|| Regex::new(concat!(
        r"(?ix)(?P<key>\bbudget(?:\s+amount)?\b|预算(?:金额)?|\bamount\s+of\b)\s*[:：]?\s*",
        r"(?P<link>(?:(?:is|was|of|now|finally|set|to|at|corrected|changed|revised|amended|updated|final|确定|最终|暂按|先按|改为|调整为|为|是|按|先|暂定|不|未)\s*){0,8})",
        r"(?P<prefix>[$€£¥]?)\s*(?P<value>\d+(?:[.,]\d+)*)(?P<suffix>\s*(?:元|人民币|USD\b|EUR\b|GBP\b|CNY\b|RMB\b|dollars?\b|euros?\b|pounds?\b))?"
    )).unwrap())
}
fn simple_clause(text: &str, matched: Range<usize>) -> bool {
    let rest = format!("{} {}", &text[..matched.start], &text[matched.end..]);
    if rest.chars().any(|c| ('\u{3400}'..='\u{9fff}').contains(&c)) {
        let rest = rest.trim_matches(|c: char| {
            c.is_whitespace() || c.is_ascii_punctuation() || "，。；".contains(c)
        });
        return [
            "",
            "还没有确定",
            "尚未确定",
            "仍未确定",
            "还未确定",
            "未确定",
            "未批准",
            "尚未批准",
            "仍未批准",
            "没有批准",
            "还没有批准",
            "还未批准",
            "待确认",
            "未获批",
            "尚未获批",
            "还没确定",
            "暂未确定",
            "但仍未确定",
            "但未确定",
            "算",
            "安排",
        ]
        .contains(&rest);
    }
    rest.to_lowercase()
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|s| !s.is_empty())
        .all(|s| {
            [
                "the",
                "a",
                "our",
                "this",
                "current",
                "previous",
                "old",
                "new",
                "initial",
                "tentative",
                "final",
                "is",
                "was",
                "has",
                "have",
                "been",
                "not",
                "approved",
                "unapproved",
                "yet",
                "still",
                "pending",
                "undecided",
                "unconfirmed",
                "remains",
                "remain",
                "currently",
                "to",
                "be",
                "but",
            ]
            .contains(&s)
        })
}
pub(super) fn budget_view(segments: &[Segment]) -> Option<BudgetView> {
    static TOPIC: OnceLock<Regex> = OnceLock::new();
    static UNCERTAIN: OnceLock<Regex> = OnceLock::new();
    let topic = TOPIC.get_or_init(|| Regex::new(r"(?i)\bbudget\b|预算").unwrap());
    let amount = amount_pattern();
    let uncertain = UNCERTAIN.get_or_init(|| Regex::new(r"(?i)\b(if|when|once|unless|would|could|might|proposed?)\b|如果|假如|要是|是否|不改|未改|没改").unwrap());
    let first = segments.first()?;
    if segments
        .iter()
        .any(|s| s.meeting_id != first.meeting_id || s.run != first.run)
    {
        return None;
    }
    let mut found = vec![];
    for (index, s) in segments.iter().enumerate() {
        // Qualified/multiple budget scopes need real field identity, not global replacement.
        if topic
            .find_iter(&s.text)
            .any(|m| !plain_scope(&s.text, m.start(), m.end()))
        {
            return None;
        }
        for captures in amount.captures_iter(&s.text) {
            let key = captures.name("key")?;
            let value = captures.name("value")?;
            if key.as_str().to_lowercase().starts_with("amount")
                && !topic.is_match(&s.text[..key.start()])
            {
                continue;
            }
            let link = &captures["link"];
            let prefix = &captures["prefix"];
            let suffix = captures.name("suffix").map_or("", |m| m.as_str()).trim();
            if link.trim().is_empty()
                && prefix.is_empty()
                && suffix.is_empty()
                && key.as_str().eq_ignore_ascii_case("budget")
            {
                continue;
            }
            if s.text[value.end()..].trim_start().starts_with("年") {
                continue;
            }
            let start = s.text[..key.start()]
                .rfind(boundary)
                .map_or(0, |p| p + s.text[p..].chars().next().unwrap().len_utf8());
            let end = s.text[value.end()..]
                .find(boundary)
                .map_or(s.text.len(), |p| {
                    value.end() + p + s.text[value.end() + p..].chars().next().unwrap().len_utf8()
                });
            let clause = &s.text[start..end];
            // Alternatives, dates or additional numeric attributes are intentionally not guessed.
            if numbers().find_iter(clause).count() != 1 {
                return None;
            }
            let matched = captures.get(0)?;
            if !simple_clause(clause, matched.start() - start..matched.end() - start) {
                return None;
            }

            let correction = [
                "corrected",
                "changed",
                "revised",
                "amended",
                "updated",
                "final",
                "改为",
                "调整为",
                "最终",
            ]
            .iter()
            .any(|cue| link.to_lowercase().contains(cue));
            if correction
                && (uncertain.is_match(clause)
                    || clause.ends_with(['?', '？'])
                    || link.contains(['不', '未']))
            {
                return None;
            }
            let range = captures
                .name("prefix")
                .filter(|p| !p.as_str().is_empty())
                .map_or(value.start(), |p| p.start())
                ..captures.name("suffix").map_or(value.end(), |m| m.end());
            found.push(Amount {
                segment: index,
                clause: start..end,
                pending: super::validation::uncertain(clause)
                    || clause.contains("先按")
                    || clause.contains("暂按"),
                range,
                value: value.as_str().into(),
                unit: format!("{prefix}{suffix}").to_lowercase(),
                correction,
            });
        }
    }
    found.sort_by_key(|a| (segments[a.segment].seq, a.range.start));
    let at = found.iter().rposition(|a| a.correction)?;
    // A later value without an explicit transition is ambiguous, not a new correction.
    if at + 1 != found.len() {
        return None;
    }
    let latest = found.pop()?;
    if found.is_empty()
        || found
            .iter()
            .any(|a| a.unit != latest.unit || a.pending && !latest.pending)
    {
        return None;
    }
    let old: Vec<_> = found
        .into_iter()
        .filter(|a| number(&a.value) != number(&latest.value))
        .collect();
    if old.is_empty() {
        return None;
    }
    let mut forbidden = vec![];
    for previous in &old {
        let normalized = number(&previous.value);
        let has_other_source = segments.iter().enumerate().any(|(index, s)| {
            numbers().find_iter(&s.text).any(|n| {
                number(n.as_str()) == normalized
                    && !old.iter().any(|a| {
                        a.segment == index && a.range.start <= n.start() && a.range.end >= n.end()
                    })
            })
        });
        if !has_other_source && !forbidden.contains(&normalized) {
            forbidden.push(normalized);
        }
    }
    Some(BudgetView {
        old,
        latest,
        forbidden,
    })
}
fn historical(text: &str, at: usize) -> bool {
    static HISTORY: OnceLock<Regex> = OnceLock::new();
    let history=HISTORY.get_or_init(||Regex::new(r"(?i)\b(previous|previously|original|originally|initially|earlier)\b|原预算|此前|之前|原先|最初|当时").unwrap());
    let start = text[..at]
        .rfind(|c| boundary(c) || c == ',')
        .map_or(0, |p| p + text[p..].chars().next().unwrap().len_utf8());
    let prefix = &text[start..at];
    let Some(past) = history.find_iter(prefix).last() else {
        return false;
    };
    static CURRENT: OnceLock<Regex> = OnceLock::new();
    let current = CURRENT.get_or_init(|| {
        Regex::new(r"(?i)\b(current|currently|now|latest|final)\b|当前|现在|最新|最终").unwrap()
    });
    !current
        .find_iter(prefix)
        .any(|now| now.start() >= past.end())
}
impl BudgetView {
    pub(super) fn text(&self, index: usize, original: &str) -> String {
        let mut text = original.to_owned();
        for old in self.old.iter().rev().filter(|a| a.segment == index) {
            text.replace_range(old.clause.clone(), "");
        }
        text
    }
    pub(super) fn context(&self) -> Value {
        json!({"amount":self.latest.value,"source_passage":self.latest.segment+1})
    }
    pub(super) fn validate(&self, body: &str) -> Result<()> {
        let values: Vec<_> = numbers()
            .find_iter(body)
            .map(|m| number(m.as_str()))
            .collect();
        let wrong_budget = amount_pattern().captures_iter(body).any(|c| {
            !c["key"].to_lowercase().starts_with("amount")
                && number(&c["value"]) != number(&self.latest.value)
                && !historical(body, c.name("value").unwrap().start())
        });
        let old_current = numbers()
            .find_iter(body)
            .any(|m| self.forbidden.contains(&number(m.as_str())) && !historical(body, m.start()));
        if wrong_budget || old_current || !values.contains(&number(&self.latest.value)) {
            bail!("纪要未采用预算最新更正，原始转写已保留；本轮未提交");
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
    fn explicit_budget_correction_hides_only_old_numeric_field() {
        let s=vec![source(1,"Morgan owns the budget review. The amount of 3000 is not approved yet. The review deadline is Thursday."),
            source(2,"Riley will test keyboard accessibility on Tuesday."),source(3,"The budget amount is corrected to 5000, still not approved. Morgan keeps the Thursday deadline.")];
        let original = s[0].text.clone();
        let view = budget_view(&s).unwrap();
        let old = view.text(0, &s[0].text);
        assert!(!old.contains("3000"));
        assert!(old.contains("Morgan") && old.contains("Thursday"));
        assert_eq!(view.text(1, &s[1].text), s[1].text);
        assert_eq!(s[0].text, original);
        assert!(view
            .validate("Morgan's budget is 5000, still not approved; Riley tests on Tuesday.")
            .is_ok());
        assert!(view
            .validate("The amount of 3000 is not approved. Budget is corrected to 5000.")
            .is_err());
        for stale in [
            "The previous budget was 5000, but the current budget is 3000.",
            "Previously the budget was 5000 but now the budget is 3000.",
        ] {
            assert!(view.validate(stale).is_err(), "{stale}");
        }
        assert!(view
            .validate(
                "The previous budget was 3000. The current budget is 5000, still not approved."
            )
            .is_ok());
    }
    #[test]
    fn ambiguous_scopes_dates_questions_conditions_and_currencies_are_not_rewritten() {
        for (old, new) in [
            (
                "The hardware budget is 3000.",
                "The marketing budget is corrected to 5000.",
            ),
            (
                "The budget is 3000 or 4000.",
                "The budget is corrected to 5000.",
            ),
            (
                "The budget is 3000.",
                "If the budget is corrected to 5000, check again.",
            ),
            ("The budget is 3000.", "The budget is corrected to 5000?"),
            (
                "The budget is 3000 EUR.",
                "The budget is corrected to 5000 USD.",
            ),
            (
                "The budget is 3000.",
                "The budget is not corrected to 5000.",
            ),
            ("预算3000元尚未确定。", "预算是否改为5000元？"),
        ] {
            assert!(
                budget_view(&[source(1, old), source(2, new)]).is_none(),
                "{old} / {new}"
            );
        }
    }
    #[test]
    fn another_topic_can_keep_the_same_number_and_chinese_status_survives() {
        let s = vec![
            source(1, "预算3000元尚未确定。"),
            source(2, "发票金额3000元还要核对。"),
            source(3, "预算改为5000元但仍未确定。"),
        ];
        let view = budget_view(&s).unwrap();
        assert!(!view.text(0, &s[0].text).contains("3000"));
        assert_eq!(view.text(1, &s[1].text), s[1].text);
        assert!(view
            .validate("预算5000元仍未确定，发票3000元待核对。")
            .is_ok());
        assert!(view
            .validate("预算3000元仍未确定，发票5000元待核对。")
            .is_err());
    }
}
