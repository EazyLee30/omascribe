//! Literal named action frames, not entity resolution or general entailment.
//! Both labels must already be protected spellings observed once in the source.
use anyhow::Result;
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug)]
pub(super) struct Mismatch;
impl Mismatch {
    pub(super) fn hint(&self) -> &'static str {
        " Preserve the explicit action direction: in A will check/test/verify/review B, A is the actor and B is the object, never the reverse. Copy both supplied spellings exactly. Preserve the future action. 'It has not been approved' is passive (尚未获批), not an agent refusing to approve."
    }
}
impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.hint().trim())
    }
}
impl std::error::Error for Mismatch {}

#[derive(Debug, Clone)]
pub(super) struct Action {
    pub actor: String,
    pub object: String,
    pub predicate: &'static str,
    pub leading_prefix: Option<&'static str>,
}

fn generic(label: &str) -> bool {
    [
        "we",
        "you",
        "they",
        "he",
        "she",
        "it",
        "someone",
        "anyone",
        "team",
        "staff",
        "user",
        "server",
        "system",
        "the team",
        "the user",
        "the server",
        "card",
        "video card",
        "gpu",
        "model",
        "device",
        "report",
        "file",
        "computer",
        "document",
        "something",
        "用户",
        "系统",
        "服务器",
        "团队",
        "显卡",
        "模型",
        "设备",
        "报告",
        "文件",
    ]
    .iter()
    .any(|word| label.eq_ignore_ascii_case(word))
}

fn terminal(source: &str) -> &str {
    let source = source.trim();
    source
        .strip_suffix('.')
        .or_else(|| source.strip_suffix('。'))
        .unwrap_or(source)
        .trim_end()
}

pub(super) fn action(source: &str, labels: &[String], chinese: bool) -> Option<Action> {
    if !chinese || labels.len() > 12 || source.contains(['?', '？']) {
        return None;
    }
    let source = source.trim();
    let mut speech = source;
    let mut leading_prefix = None;
    const APPROVAL: &str = "IT HAS NOT BEEN APPROVED";
    if speech
        .get(..APPROVAL.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(APPROVAL))
    {
        let rest = &speech[APPROVAL.len()..];
        if !rest.starts_with(|c: char| c.is_whitespace() || matches!(c, '.' | '。')) {
            return None;
        }
        speech = rest.trim_start();
        if let Some(rest) = speech
            .strip_prefix('.')
            .or_else(|| speech.strip_prefix('。'))
        {
            speech = rest.trim_start();
        }
        leading_prefix = Some("尚未获批。 ");
    }
    let speech = terminal(speech);
    static VERB: OnceLock<Regex> = OnceLock::new();
    let verb = VERB.get_or_init(|| {
        Regex::new(r"(?i)^\s+will\s+(check|test|verify|review)\s+(?:the\s+)?").unwrap()
    });
    let mut found = None;
    for actor in labels {
        if actor.is_empty() || generic(actor) || source.matches(actor.as_str()).count() != 1 {
            continue;
        }
        let Some(rest) = speech.strip_prefix(actor) else {
            continue;
        };
        let Some(frame) = verb.captures(rest) else {
            continue;
        };
        let object_text = &rest[frame.get(0)?.end()..];
        for object in labels {
            if object_text != object
                || actor.eq_ignore_ascii_case(object)
                || generic(object)
                || source.matches(object.as_str()).count() != 1
            {
                continue;
            }
            // The entire remaining source is this one action. Additional
            // clauses/conditions cannot be discarded by a constrained repair.
            if found.is_some() {
                return None;
            }
            found = Some(Action {
                actor: actor.clone(),
                object: object.clone(),
                predicate: match frame[1].to_ascii_lowercase().as_str() {
                    "check" => "将检查",
                    "test" => "将测试",
                    "verify" => "将核验",
                    _ => "将审阅",
                },
                leading_prefix,
            });
        }
    }
    found
}

fn literal(label: &str) -> String {
    format!(
        "{}{}{}",
        if label.starts_with(|c: char| c.is_ascii_alphanumeric()) {
            r"(?-u:\b)"
        } else {
            ""
        },
        regex::escape(label),
        if label.ends_with(|c: char| c.is_ascii_alphanumeric()) {
            r"(?-u:\b)"
        } else {
            ""
        }
    )
}

fn boundary(c: char) -> bool {
    "。.!?！？；;，,\n".contains(c)
}

fn masked(text: &str, action: &Action) -> String {
    text.replace(&action.actor, "\u{e000}")
        .replace(&action.object, "\u{e001}")
}

fn future_action(body: &str, span: regex::Match<'_>, action: &Action) -> bool {
    static FUTURE: OnceLock<Regex> = OnceLock::new();
    static FRONTED: OnceLock<Regex> = OnceLock::new();
    static NEGATIVE: OnceLock<Regex> = OnceLock::new();
    static DENIED_SUBJECT: OnceLock<Regex> = OnceLock::new();
    static CONDITIONAL: OnceLock<Regex> = OnceLock::new();
    static FOLLOWING_CONDITION: OnceLock<Regex> = OnceLock::new();
    let frame = masked(span.as_str(), action);
    let after_label = frame
        .strip_prefix('\u{e000}')
        .or_else(|| frame.strip_prefix('\u{e001}'))
        .unwrap_or(&frame);
    let before = masked(
        body[..span.start()].rsplit(boundary).next().unwrap_or(""),
        action,
    );
    let after = masked(
        body[span.end()..].split(boundary).next().unwrap_or(""),
        action,
    );
    let following = masked(
        body[span.end()..].trim_start_matches(|c: char| boundary(c) || c.is_whitespace()),
        action,
    );
    // Modality belongs to this action, with opaque labels removed. A legal
    // preceding 尚未获批 clause must not become a negation of the checking task.
    let future =
        FUTURE.get_or_init(|| Regex::new(r"^\s*(?:将会|将|会|即将|计划|打算|准备|拟|要)").unwrap());
    let fronted = FRONTED.get_or_init(|| {
        Regex::new(r"(?:将会|将|会|即将|计划|打算|准备|拟)\s*(?:由|让)\s*$").unwrap()
    });
    let negative =
        NEGATIVE.get_or_init(|| Regex::new(r"不|尚未|未|没有|没|无需|无须|取消|撤回").unwrap());
    let denied_subject = DENIED_SUBJECT.get_or_init(|| {
        Regex::new(r"(?:不是|并非|不会由|不再由|不应由|不能由|不由|不让|不允许|无需|不需要)\s*$")
            .unwrap()
    });
    let conditional = CONDITIONAL.get_or_init(|| {
        Regex::new(r"(?i)如果|假如|倘若|若|除非|只有|一旦|可能|也许|或许|前提|取决于|条件是|\b(?:if|unless|maybe|perhaps|possibly|provided|assuming|whether)\b").unwrap()
    });
    let following_condition = FOLLOWING_CONDITION.get_or_init(|| {
        Regex::new(r"^(?:但|不过|只是|而|且)?\s*(?:如果|假如|倘若|若|除非|只有|一旦|可能|也许|或许|前提|取决于|条件是)").unwrap()
    });
    (future.is_match(after_label) || fronted.is_match(&before))
        && !negative.is_match(&frame)
        && !denied_subject.is_match(&before)
        && !conditional.is_match(&before)
        && !conditional.is_match(&frame)
        && !conditional.is_match(&after)
        && !following_condition.is_match(&following)
}

pub(super) fn validate(source: &str, body: &str, labels: &[String], target: &str) -> Result<()> {
    let Some(action) = action(source, labels, target == "zh") else {
        return Ok(());
    };
    let actor = literal(&action.actor);
    let object = literal(&action.object);
    let verbs = match action.predicate {
        "将检查" => "(?:检查|核查|核对|检验)",
        "将测试" => "(?:测试|试验|检测)",
        "将核验" => "(?:核验|验证|核查|核对)",
        _ => "(?:审阅|审查|评审|审核|复核)",
    };
    const GAP: &str = r"[^。.!?！？；;，,\n]{0,32}?";
    let direction = |actor: &str, object: &str| {
        Regex::new(&format!(
            "(?:{actor}{GAP}{verbs}{GAP}{object}|{object}{GAP}(?:由|被){GAP}{actor}{GAP}{verbs}|{actor}{GAP}(?:把|对|针对){GAP}{object}{GAP}{verbs})"
        ))
        .expect("escaped local action direction")
    };
    let correct = direction(&actor, &object);
    if direction(&object, &actor).is_match(body) || !correct.is_match(body) {
        return Err(Mismatch.into());
    }
    if correct
        .find_iter(body)
        .any(|span| !future_action(body, span, &action))
    {
        return Err(Mismatch.into());
    }
    // The parsed source explicitly says WILL. Do not accept a completed action
    // merely because its two labels retain the correct direction. Scope this
    // check to the actual matched action, not unrelated article-wide words.
    static COMPLETED: OnceLock<Regex> = OnceLock::new();
    let completed = COMPLETED.get_or_init(|| {
        Regex::new(concat!(
            r"(?:已经|现已|已|刚刚|曾经|曾)[^。.!?！？；;，,\n]{0,32}(?:检查|核查|核对|检验|测试|试验|检测|核验|验证|审阅|审查|评审|审核|复核)",
            r"|(?:检查|核查|核对|检验|测试|试验|检测|核验|验证|审阅|审查|评审|审核|复核)(?:了|过|完了)"
        )).unwrap()
    });
    if correct.find_iter(body).any(|span| {
        completed.find_iter(span.as_str()).any(|found| {
            !["未", "没", "不", "没有"].iter().any(|negative| {
                span.as_str()[..found.start()]
                    .trim_end()
                    .ends_with(negative)
            })
        })
    }) {
        return Err(Mismatch.into());
    }
    if action.leading_prefix.is_some() {
        static PASSIVE: OnceLock<Regex> = OnceLock::new();
        static ACTIVE: OnceLock<Regex> = OnceLock::new();
        let passive = PASSIVE.get_or_init(|| {
            Regex::new(r"(?:尚未|仍未|还未|未|没有)(?:(?:获得|得到|获|被)批准|获批|通过审批)")
                .unwrap()
        });
        let active = ACTIVE.get_or_init(|| {
            Regex::new(r"(?:它|他|她|它们|他们|她们)(?:尚未|仍未|还未|未|没有)批准").unwrap()
        });
        if !passive.is_match(body) || active.is_match(body) {
            return Err(Mismatch.into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn labels() -> Vec<String> {
        vec!["UN VIDEO CARD".into(), "ALICE".into()]
    }
    const SOURCE: &str = "IT HAS NOT BEEN APPROVED ALICE WILL CHECK THE UN VIDEO CARD";

    #[test]
    fn exact_labels_and_passive_approval_are_source_data_only() {
        let frame = action(SOURCE, &labels(), true).unwrap();
        assert_eq!(frame.actor, "ALICE");
        assert_eq!(frame.object, "UN VIDEO CARD");
        assert_eq!(frame.predicate, "将检查");
        assert_eq!(frame.leading_prefix, Some("尚未获批。 "));
        assert!(action(SOURCE, &labels(), false).is_none());
        assert!(action(SOURCE, &[], true).is_none());
    }

    #[test]
    fn active_and_passive_preserve_the_actor_and_object() {
        for body in [
            "尚未获批。ALICE将检查UN VIDEO CARD。",
            "它尚未被批准。UN VIDEO CARD将由ALICE检查。",
            "尚未获得批准。ALICE会对UN VIDEO CARD进行检查。",
            "尚未获批，ALICE将检查UN VIDEO CARD。",
            "尚未获批。将由ALICE检查UN VIDEO CARD。",
        ] {
            assert!(validate(SOURCE, body, &labels(), "zh").is_ok(), "{body}");
        }
        for body in [
            "它尚未批准UN VIDEO CARD将检查ALICE",
            "尚未获批。ALICE将由UN VIDEO CARD检查。",
            "尚未获批。ALICE将检查UN VIDEO CARD。UN VIDEO CARD将检查ALICE。",
            "它尚未批准。ALICE将检查UN VIDEO CARD。",
            "ALICE将检查UN VIDEO CARD。",
            "尚未获批。ALICE将检查NVIDIA。",
            "尚未获批。ALICE已经检查了UN VIDEO CARD。",
            "尚未获批。UN VIDEO CARD已由ALICE检查。",
            "尚未获批。ALICE不会检查UN VIDEO CARD。",
            "尚未获批。UN VIDEO CARD不会被ALICE检查。",
            "尚未获批。如果ALICE将检查UN VIDEO CARD。",
            "尚未获批。ALICE可能会检查UN VIDEO CARD。",
            "尚未获批。ALICE检查UN VIDEO CARD。",
            "尚未获批。不是ALICE将检查UN VIDEO CARD。",
            "尚未获批。ALICE将检查UN VIDEO CARD。ALICE不会检查UN VIDEO CARD。",
            "尚未获批。ALICE将检查UN VIDEO CARD，前提是先批准。",
        ] {
            assert!(validate(SOURCE, body, &labels(), "zh").is_err(), "{body}");
        }
    }

    #[test]
    fn qualifiers_extra_facts_and_generic_labels_abstain() {
        for source in [
            "If ALICE will check the UN VIDEO CARD, we can proceed.",
            "ALICE will check the UN VIDEO CARD if it arrives.",
            "ALICE will not check the UN VIDEO CARD.",
            "ALICE will check the UN VIDEO CARD?",
            "Cancelled: ALICE will check the UN VIDEO CARD.",
            "ALICE will publish the UN VIDEO CARD.",
            "ALICE will check the UN VIDEO CARD. The cache stays local.",
            "ALICE will check the UN VIDEO CARD and ALICE will review it.",
            "IT HAS NOT BEEN APPROVED. A different task comes first. ALICE will check the UN VIDEO CARD.",
        ] {
            assert!(action(source, &labels(), true).is_none(), "{source}");
        }
        assert!(action(
            "THE TEAM will check the UN VIDEO CARD.",
            &["THE TEAM".into(), "UN VIDEO CARD".into()],
            true
        )
        .is_none());
        assert!(action(
            "ALICE will check the model.",
            &["ALICE".into(), "model".into()],
            true
        )
        .is_none());
    }

    #[test]
    fn action_words_are_bounded_and_labels_are_not_substrings() {
        for (verb, predicate) in [
            ("test", "将测试"),
            ("verify", "将核验"),
            ("review", "将审阅"),
        ] {
            let source = format!("ALICE will {verb} UN VIDEO CARD.");
            assert_eq!(
                action(&source, &labels(), true).unwrap().predicate,
                predicate
            );
        }
        assert!(validate(
            "ALICE will check the UN VIDEO CARD.",
            "MALICE将检查UN VIDEO CARD。",
            &labels(),
            "zh"
        )
        .is_err());
        assert!(action(
            "ALICE will check the UN VIDEO CARD and ALICE.",
            &labels(),
            true
        )
        .is_none());
    }
}
