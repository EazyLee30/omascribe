//! Preserve an explicit approval predicate in a simple negative budget field.
//! This is wording repair after validation, not a general entailment parser.
use super::names::Names;
use regex::Regex;
use std::sync::OnceLock;

pub(super) fn precise_budget_approval(
    source: &str,
    caption: &str,
    names: &Names,
) -> Option<String> {
    if !caption.contains("预算")
        || !caption.contains("未通过")
        || source.contains([
            '?', '？', '"', '\'', '“', '”', '‘', '’', '「', '」', '『', '』',
        ])
        || caption.contains([
            '?', '？', '"', '\'', '“', '”', '‘', '’', '「', '」', '『', '』',
        ])
    {
        return None;
    }
    static META: OnceLock<Regex> = OnceLock::new();
    if META.get_or_init(|| Regex::new(concat!(
        r"(?i)\b(?:if|unless|whether|deny|denies|denied|false|untrue|incorrect|inaccurate|wrong|opposite|say|says|said|claim|claims|claimed|quote|quoted|withdraw|withdrawn|retract|retracted|reverse|reversed|disregard|ignore|correction|actually|instead|now|rejected|it|that|this|these|those)\b",
        r"|\b(?:never\s+mind|not\s+true|not\s+accurate|not\s+the\s+case|no\s+longer)\b"
    )).unwrap()).is_match(source) {
        return None;
    }
    static BUDGET: OnceLock<Regex> = OnceLock::new();
    static APPROVAL: OnceLock<Regex> = OnceLock::new();
    if BUDGET
        .get_or_init(|| Regex::new(r"(?i)\bbudgets?\b").unwrap())
        .find_iter(source)
        .count()
        != 1
        || APPROVAL
            .get_or_init(|| {
                Regex::new(r"(?i)\b(?:approv(?:al|als|e|es|ed|ing)|unapproved|rejection)\b")
                    .unwrap()
            })
            .find_iter(source)
            .count()
            != 1
        || caption.matches("预算").count() != 1
    {
        return None;
    }
    // Never pull an assertion out of a comma/colon-led condition or report.
    static SOURCE: OnceLock<Regex> = OnceLock::new();
    let source_field = SOURCE
        .get_or_init(|| {
            Regex::new(concat!(
                r"(?i)(?:^|[.。;；\n])[ \t]*(?P<field>(?:the[ \t]+)?budget[ \t]+(?:",
                r"has[ \t]+not[ \t]+(?:yet[ \t]+)?been[ \t]+approved|",
                r"is[ \t]+not[ \t]+(?:yet[ \t]+)?approved|remains[ \t]+unapproved))",
                r"[ \t]*(?:[.。;；\n]|$)"
            ))
            .unwrap()
        })
        .captures(source)?
        .name("field")?;
    if names.mask(source)[source_field.range()] != source[source_field.range()] {
        return None;
    }
    static TARGET: OnceLock<Regex> = OnceLock::new();
    let target = TARGET.get_or_init(|| Regex::new(
        r"(?:^|[。；;！？!\n])[ \t]*(?P<field>预算(?:仍|还)?(?P<predicate>未通过))[ \t]*(?:[。；;！？!\n]|$)"
    ).unwrap()).captures(caption)?;
    let field = target.name("field")?;
    if names.mask(caption)[field.range()] != caption[field.range()] {
        return None;
    }
    let predicate = target.name("predicate")?;
    Some(format!(
        "{}未获批准{}",
        &caption[..predicate.start()],
        &caption[predicate.end()..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn repair(source: &str, caption: &str) -> Option<String> {
        precise_budget_approval(
            source,
            caption,
            &Names::new(&crate::terminology::Terms::new(&[], &[]), source),
        )
    }
    #[test]
    fn complete_single_approval_fields_preserve_other_sentences_and_qualifiers() {
        for source in [
            "The budget has not been approved.",
            "Budget is not approved.",
            "The budget remains unapproved.",
            "The budget has not yet been approved.",
        ] {
            assert_eq!(
                repair(source, "预算仍未通过。测试未通过。"),
                Some("预算仍未获批准。测试未通过。".into())
            );
        }
        let source =
            "CedarLedger has not completed the local test. The budget has not been approved.";
        assert_eq!(
            repair(source, "CedarLedger 未完成本地测试。预算未通过。"),
            Some("CedarLedger 未完成本地测试。预算未获批准。".into())
        );
    }
    #[test]
    fn reported_conditional_reversed_or_other_predicates_are_not_authorization() {
        for source in [
            "The budget has been approved.",
            "The budget has not passed.",
            "The test has not been approved.",
            "The budget has not been approved by Alice.",
            "If this is true, the budget has not been approved.",
            "The budget has not been approved. It has been approved.",
            "The budget has not been approved. That is false.",
            "The budget has not been approved. That is not the case.",
            "The budget has not been approved. The statement is not accurate.",
            "The budget has not been approved. Ignore that.",
            "The budget has not been approved?",
            "\"The budget has not been approved.\"",
            "They said: the budget has not been approved.",
            "The budget has not been approved, unless the review succeeds.",
            "The budget has not been approved. Another budget is pending.",
            "The budget of3000 has not been approved.",
        ] {
            assert!(repair(source, "预算未通过。").is_none(), "{source}");
        }
    }
    #[test]
    fn target_complements_negations_questions_and_protected_names_stay_literal() {
        let source = "The budget has not been approved.";
        for caption in [
            "预算未通过审核。",
            "预算未通过测试。",
            "并非预算未通过。",
            "如果预算未通过。",
            "预算未通过才继续。",
            "预算未通过？",
            "“预算未通过。”",
            "预算未获批准。",
            "预算未通过。另一个预算未通过。",
        ] {
            assert!(repair(source, caption).is_none(), "{caption}");
        }
        let terms = crate::terminology::Terms::new(
            &[],
            &[crate::library::Entry {
                kind: "term".into(),
                title: "未通过".into(),
                aliases: "PendingGate".into(),
                ..Default::default()
            }],
        );
        let source = "The project is PendingGate. The budget has not been approved.";
        let names = Names::new(&terms, source);
        assert!(names.spellings().contains(&"未通过".into()));
        let caption = "未通过 是项目名。预算未通过。";
        assert!(precise_budget_approval(source, caption, &names).is_none());
    }
}
