//! Bounded contrasts for explicit, named report and review tasks. These checks
//! do not resolve ambiguous ownership or establish general semantic entailment.
use anyhow::Result;
use regex::Regex;
use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mismatch {
    Delivery,
    Responsibility,
}
impl Mismatch {
    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::Delivery => " 报告是送交事项：按原文保留send、deliver或submit的动作、报告名称和期限。Keep the stated delivery action separate from the report title.",
            Self::Responsibility => " 该句是任务分工：owns a task 表示负责该任务，不是拥有物品、版权或审批权。保留同一负责人、原任务对象、动作和期限。Use responsibility for the stated task, without inventing ownership or approval rights.",
        }
    }
}
impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.hint().trim())
    }
}
impl std::error::Error for Mismatch {}

#[derive(Default)]
struct Evidence {
    delivery: bool,
    publication: bool,
    task_owner: bool,
    literal_owner: bool,
}

fn english_frames() -> &'static Regex {
    static FRAMES: OnceLock<Regex> = OnceLock::new();
    FRAMES.get_or_init(|| Regex::new(concat!(
        r"(?i)(?:^|[.;!?。！？；\n])\s*(?P<actor>[a-z][a-z'’-]{1,29})\s+",
        r"(?:(?P<modal>will|would|shall|can|could|must|should)\s+)?",
        r"(?P<verb>sends?|sent|delivers?|delivered|submits?|submitted|owns?|publishes|publish|published|releases?|released|possesses|possess|possessed)\s+",
        r"(?:(?:the|a|an)\s+)?(?P<task>(?:[a-z0-9-]+\s+){0,6}reports?)\b",
    )).expect("report task frames"))
}

fn named_actor(actor: &str) -> bool {
    ![
        "we", "he", "she", "they", "you", "it", "who", "that", "which", "someone", "everyone",
        "nobody",
    ]
    .contains(&actor)
}

fn report_task(task: &str) -> bool {
    !task.split_whitespace().any(|w| {
        [
            "and", "or", "but", "to", "will", "would", "can", "could", "not",
        ]
        .contains(&w)
    })
}

fn task_delivery(tail: &str) -> bool {
    static DELIVERY: OnceLock<Regex> = OnceLock::new();
    let tail: String = tail
        .split(['.', ';', '!', '?', '\n'])
        .next()
        .unwrap_or("")
        .chars()
        .take(100)
        .collect();
    // Require the same subject's coordinated delivery, not a different person
    // mentioned somewhere else in the paragraph.
    DELIVERY.get_or_init(|| Regex::new(
        r"(?i)^\s+and\s+(?:(?:will|shall|must|should)\s+)?(?:deliver|submit|send)\s+(?:it|the\s+report)\b"
    ).unwrap()).is_match(&tail)
}

pub(crate) struct Anchor {
    pub actor: String,
    pub predicate: String,
    pub objects: &'static [&'static str],
}

const REVIEW_OBJECTS: &[&str] = &["审核", "审查", "评审", "复核"];
struct Review {
    actor: String,
    object: &'static str,
    rights: bool,
}
fn review_frames(source: &str) -> Vec<Review> {
    // These are activity objects, not titles of arbitrary works called Review.
    // Extra modifiers, physical copies, rights and hypothetical/revised owners
    // are deliberately outside the responsibility anchor.
    static FRAME: OnceLock<Regex> = OnceLock::new();
    static UNCERTAIN: OnceLock<Regex> = OnceLock::new();
    if source.contains(['?', '？']) || UNCERTAIN.get_or_init(|| Regex::new(r"(?i)\b(?:if|unless|provided|either|or|maybe|perhaps|possibly|probably|cancel\w*|withdraw\w*|reassign\w*|transfer\w*|instead)\b|no longer|not responsible").unwrap()).is_match(source) {
        return Vec::new();
    }
    let frame = FRAME.get_or_init(|| Regex::new(concat!(
        r"(?i)(?:^|[.;!?。！？；\n])\s*(?P<actor>[a-z][a-z'’-]{1,29})\s+owns\s+",
        r"(?:(?:the|a|an)\s+)?(?P<object>budget|invoice|code|design|security|accessibility|quality|release|test|performance)\s+review\b",
    )).unwrap());
    let mut reviews = Vec::new();
    for c in frame.captures_iter(source).take(12) {
        let actor = &c["actor"];
        if !named_actor(&actor.to_ascii_lowercase())
            || c["object"] != c["object"].to_ascii_lowercase()
            || [
                "team",
                "staff",
                "management",
                "server",
                "system",
                "department",
            ]
            .contains(&actor.to_ascii_lowercase().as_str())
        {
            continue;
        }
        let tail = source[c.get(0).unwrap().end()..]
            .split(['.', ';', '!', '?', '。', '；', '！', '？', '\n'])
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let rights = ["rights", "copyright"].contains(&tail.as_str());
        // A trailing clause could assign another owner, change the object or
        // qualify this role. Do not constrain it from a partial first phrase.
        if !tail.is_empty() && !rights {
            continue;
        }
        let object = match c["object"].to_ascii_lowercase().as_str() {
            "budget" => "budget",
            "invoice" => "invoice",
            "code" => "code",
            "design" => "design",
            "security" => "security",
            "accessibility" => "accessibility",
            "quality" => "quality",
            "release" => "release",
            "test" => "test",
            _ => "performance",
        };
        reviews.push(Review {
            actor: actor.into(),
            object,
            rights,
        });
    }
    reviews
}
fn review_object(clause: &str, object: &str) -> bool {
    let aliases: &[&str] = match object {
        "budget" => &["budget", "预算"],
        "invoice" => &["invoice", "发票", "账单"],
        "code" => &["code", "代码", "程式"],
        "design" => &["design", "设计"],
        "security" => &["security", "安全"],
        "accessibility" => &["accessibility", "无障碍", "可访问性"],
        "quality" => &["quality", "质量", "品质"],
        "release" => &["release", "发布", "发行"],
        "test" => &["test", "测试"],
        _ => &["performance", "性能", "效能"],
    };
    aliases.iter().any(|word| clause.contains(word))
        && REVIEW_OBJECTS
            .iter()
            .chain(["review"].iter())
            .any(|word| clause.contains(word))
}
fn validate_review_ownership(body: &str, reviews: &[Review]) -> Result<()> {
    static POSSESSION: OnceLock<Regex> = OnceLock::new();
    let possession = POSSESSION.get_or_init(|| Regex::new(r"(?i)^\s*(?:(?:(?:currently|now|still)\s+)?(?:possesses?\b|(?:has|holds|owns)\s+(?:the\s+)?(?:approval\s+)?(?:rights?|authority|copyright)\b|(?:has|holds|owns)\b[^.;。；,，\n]{0,80}\breview\s+(?:rights?|authority|copyright)\b)|(?:现在|目前|已经|已)?(?:拥有|擁有|持有))").unwrap());
    let lower = body.to_ascii_lowercase();
    for review in reviews.iter().filter(|r| !r.rights) {
        if reviews.iter().any(|r| {
            r.rights && r.actor.eq_ignore_ascii_case(&review.actor) && r.object == review.object
        }) {
            continue;
        }
        let actor = review.actor.to_ascii_lowercase();
        for (at, _) in lower.match_indices(&actor) {
            let end = at + actor.len();
            if lower[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric())
                || lower[end..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric())
            {
                continue;
            }
            let start = lower[..at]
                .rfind(['.', ';', '!', '?', '。', '；', '！', '？', ',', '，', '\n'])
                .map_or(0, |i| i + lower[i..].chars().next().unwrap().len_utf8());
            let tail = lower[end..]
                .split(['.', ';', '!', '?', '。', '；', '！', '？', ',', '，', '\n'])
                .next()
                .unwrap_or("");
            let clause = &lower[start..end + tail.len()];
            if review_object(clause, review.object)
                && (possession.is_match(tail)
                    || tail.trim_start().starts_with("具有")
                        && ["审核权", "审查权", "评审权", "审批权"]
                            .iter()
                            .any(|word| tail.contains(word))
                    || tail.trim_start().starts_with('是') && tail.contains("所有者"))
            {
                return Err(Mismatch::Responsibility.into());
            }
        }
    }
    Ok(())
}

/// A repair may constrain an explicit source predicate, never infer a role
/// from an ambiguous name, physical possession, conditional or revised task.
pub(crate) fn anchors(source: &str, mismatch: Mismatch) -> Vec<Anchor> {
    let lower = source.to_ascii_lowercase();
    if [
        "no longer",
        "cancel",
        "withdraw",
        "instead",
        "not send",
        "not deliver",
        "not submit",
        "撤回",
        "取消",
        "改为",
    ]
    .iter()
    .any(|word| lower.contains(word))
    {
        return Vec::new();
    }
    let frames: Vec<_> = english_frames().captures_iter(source).collect();
    let mut anchors: Vec<_> = frames
        .iter()
        .filter_map(|frame| {
            let actor = &frame["actor"];
            let task = frame["task"].to_ascii_lowercase();
            if !named_actor(&actor.to_ascii_lowercase())
                || !report_task(&task)
                || frames
                    .iter()
                    .filter(|f| f["actor"].eq_ignore_ascii_case(actor))
                    .count()
                    != 1
            {
                return None;
            }
            let tail = source[frame.get(0)?.end()..]
                .split(['.', ';', '!', '?', '\n'])
                .next()
                .unwrap_or("");
            static QUALIFIED: OnceLock<Regex> = OnceLock::new();
            if QUALIFIED.get_or_init(|| Regex::new(r"(?i)\b(if|unless|provided|or|either|maybe|perhaps|possibly|probably)\b").unwrap()).is_match(tail) {
                return None;
            }
            static WEEKDAYS: OnceLock<Regex> = OnceLock::new();
            if WEEKDAYS.get_or_init(|| Regex::new(r"(?i)\b(?:Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday)\b|(?:周|星期|礼拜)[一二三四五六日天]").unwrap()).find_iter(tail).count() > 1 {
                return None;
            }
            let modal = frame.name("modal").map(|s| s.as_str().to_ascii_lowercase());
            let verb = frame["verb"].to_ascii_lowercase();
            let predicate = match (mismatch, modal.as_deref(), verb.as_str()) {
                (Mismatch::Delivery, Some("will"), "send") => "将发送",
                (Mismatch::Delivery, Some("will"), "deliver") => "将交付",
                (Mismatch::Delivery, Some("will"), "submit") => "将提交",
                (Mismatch::Responsibility, None, "owns")
                    if !task.split_whitespace().any(|w| {
                        [
                            "copy",
                            "copies",
                            "copyright",
                            "rights",
                            "original",
                            "signed",
                            "printed",
                            "paper",
                        ]
                        .contains(&w)
                    }) && task_delivery(&source[frame.get(0)?.end()..]) =>
                {
                    "负责"
                }
                _ => return None,
            };
            // Bind a directly stated weekday to the sending action. Leaving
            // its position unconstrained can turn "on Thursday" into a report
            // about last Thursday even when the verb itself is correct.
            let mut predicate = predicate.to_owned();
            if mismatch == Mismatch::Delivery {
                static DAY: OnceLock<Regex> = OnceLock::new();
                if let Some(day) = DAY.get_or_init(|| Regex::new(r"(?i)\bon\s+(Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday)\b").unwrap()).captures(tail) {
                    let day = match day[1].to_ascii_lowercase().as_str() {
                        "monday" => "周一", "tuesday" => "周二", "wednesday" => "周三",
                        "thursday" => "周四", "friday" => "周五", "saturday" => "周六", _ => "周日",
                    };
                    predicate = format!("将于{day}{}", predicate.strip_prefix('将').unwrap());
                }
            }
            Some(Anchor {
                actor: actor.into(),
                predicate,
                objects: &["报告"],
            })
        })
        .collect();
    if mismatch == Mismatch::Responsibility {
        let reviews = review_frames(source);
        for review in reviews.iter().filter(|r| !r.rights) {
            // An additional role or explicit rights for the same actor/object
            // needs richer source binding than a single actor terminal.
            if reviews
                .iter()
                .filter(|r| r.actor.eq_ignore_ascii_case(&review.actor))
                .count()
                != 1
                || frames
                    .iter()
                    .any(|f| f["actor"].eq_ignore_ascii_case(&review.actor))
            {
                continue;
            }
            anchors.push(Anchor {
                actor: review.actor.clone(),
                predicate: "负责".into(),
                objects: REVIEW_OBJECTS,
            });
        }
    }
    anchors
}

pub(super) fn validate<'a>(body: &str, sources: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut evidence: BTreeMap<String, Evidence> = BTreeMap::new();
    let mut reviews = Vec::new();
    for source in sources {
        reviews.extend(review_frames(source));
        for frame in english_frames().captures_iter(source) {
            let actor = frame["actor"].to_ascii_lowercase();
            let task = frame["task"].to_ascii_lowercase();
            if !named_actor(&actor) || !report_task(&task) {
                continue;
            }
            let facts = evidence.entry(actor).or_default();
            match frame["verb"].to_ascii_lowercase().as_str() {
                "send" | "sends" | "sent" | "deliver" | "delivers" | "delivered" | "submit"
                | "submits" | "submitted" => facts.delivery = true,
                "publish" | "publishes" | "published" | "release" | "releases" | "released" => {
                    facts.publication = true
                }
                "own" | "owns" => {
                    let physical = task.split_whitespace().any(|w| {
                        [
                            "copy",
                            "copies",
                            "copyright",
                            "rights",
                            "original",
                            "signed",
                            "printed",
                            "paper",
                        ]
                        .contains(&w)
                    });
                    if !physical && task_delivery(&source[frame.get(0).unwrap().end()..]) {
                        facts.task_owner = true;
                    } else {
                        facts.literal_owner = true;
                    }
                }
                _ => {}
            }
        }
    }
    validate_review_ownership(body, &reviews)?;
    if evidence.is_empty() {
        return Ok(());
    }
    for frame in english_frames().captures_iter(body) {
        if !report_task(&frame["task"].to_ascii_lowercase()) {
            continue;
        }
        if let Some(facts) = evidence.get(&frame["actor"].to_ascii_lowercase()) {
            match frame["verb"].to_ascii_lowercase().as_str() {
                "publish" | "publishes" | "published" | "release" | "releases" | "released"
                    if facts.delivery && !facts.publication =>
                {
                    return Err(Mismatch::Delivery.into())
                }
                "possess" | "possesses" | "possessed"
                    if facts.task_owner && !facts.literal_owner =>
                {
                    return Err(Mismatch::Responsibility.into())
                }
                _ => {}
            }
        }
    }
    static CHINESE: OnceLock<Regex> = OnceLock::new();
    let chinese = CHINESE.get_or_init(|| {
        Regex::new(
        r"(?P<actor>[A-Za-z][A-Za-z'’-]{1,29})\s*(?P<predicate>[^A-Za-z。！？；;\n,，]{0,60}?)报告"
    ).unwrap()
    });
    for frame in chinese.captures_iter(body) {
        let Some(facts) = evidence.get(&frame["actor"].to_ascii_lowercase()) else {
            continue;
        };
        let predicate = &frame["predicate"];
        let delivery = ["发送", "送出", "提交", "交付", "寄送"]
            .iter()
            .filter_map(|v| predicate.find(v))
            .min();
        let publication = ["发布", "发表", "出版"]
            .iter()
            .filter_map(|v| {
                predicate
                    .match_indices(v)
                    .find(|(i, _)| !predicate[*i..].starts_with("发布会"))
                    .map(|(i, _)| i)
            })
            .min();
        if facts.delivery
            && !facts.publication
            && publication.is_some_and(|p| delivery.is_none_or(|d| p < d))
        {
            return Err(Mismatch::Delivery.into());
        }
        if facts.task_owner && !facts.literal_owner && predicate.contains("拥有") {
            return Err(Mismatch::Responsibility.into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_task_responsibility_is_not_an_approval_right() {
        let source = "Morgan owns the budget review. The amount of 3000 is not approved yet. The review deadline is Thursday.";
        let original = "Morgan 拥有预算审核权。3000的金额尚未批准。审核截止日期为周四。";
        let error = validate(original, [source].into_iter()).unwrap_err();
        assert_eq!(
            error.downcast_ref::<Mismatch>(),
            Some(&Mismatch::Responsibility)
        );
        for body in [
            "Morgan负责预算审核。3000尚未批准，审核截止日期为周四。",
            "预算审核由Morgan负责，3000尚未批准，截止日期为周四。",
            source,
        ] {
            assert!(validate(body, [source].into_iter()).is_ok(), "{body}");
        }
    }

    #[test]
    fn review_anchors_preserve_activity_without_report_or_rights() {
        for source in [
            "Morgan owns the budget review.",
            "Riley owns the security review. The review deadline is Monday.",
            "Nora owns the code review.",
        ] {
            let found = anchors(source, Mismatch::Responsibility);
            assert_eq!(found.len(), 1, "{source}");
            assert_eq!(found[0].predicate, "负责");
            assert_eq!(found[0].objects, REVIEW_OBJECTS);
            assert!(anchors(source, Mismatch::Delivery).is_empty());
        }
        for source in [
            "Morgan owns the original paper report and will deliver it on Thursday.",
            "Morgan owns the original budget review.",
            "Morgan owns the budget review document.",
            "Morgan owns the budget review rights.",
            "Morgan owns the Budget Review.",
            "Morgan owns the budget review if approved.",
            "Morgan owns the budget review?",
            "Morgan will own the budget review.",
            "Morgan no longer owns the budget review.",
            "Morgan owns the budget review. It was reassigned to Riley.",
            "Morgan owns the budget review or the code review.",
            "Team owns the budget review.",
            "Ann Morgan owns the budget review.",
        ] {
            assert!(
                anchors(source, Mismatch::Responsibility).is_empty(),
                "{source}"
            );
        }
    }

    #[test]
    fn review_possession_checks_bind_actor_and_activity_and_allow_actual_rights() {
        let source = "Morgan owns the budget review. Riley owns the code review.";
        // Unrelated actor/object claims are outside this contrast; do not
        // borrow Riley's code-review role to constrain Morgan's other claims.
        for body in [
            "Morgan负责预算审核。Riley负责代码评审。",
            "Morgan possesses the code review.",
            "Alice拥有预算审核权。",
        ] {
            assert!(validate(body, [source].into_iter()).is_ok(), "{body}");
        }
        for body in [
            "Morgan拥有预算审核权。Riley负责代码评审。",
            "预算审核由Morgan拥有。",
            "Morgan owns the budget review rights.",
            "Morgan has approval authority over the budget review.",
            "Riley possesses the code review.",
        ] {
            let error = validate(body, [source].into_iter()).unwrap_err();
            assert_eq!(
                error.downcast_ref::<Mismatch>(),
                Some(&Mismatch::Responsibility)
            );
        }
        for source in [
            "Morgan owns the budget review rights.",
            "Morgan owns the original budget review.",
            "Morgan owns the budget review document.",
            "Morgan owns the Budget Review.",
            "Morgan owns the budget review if approved.",
        ] {
            assert!(
                validate("Morgan拥有预算审核权。", [source].into_iter()).is_ok(),
                "{source}"
            );
        }
        let sources = [
            "Morgan owns the budget review.",
            "Morgan owns the budget review rights.",
        ];
        assert!(validate("Morgan拥有预算审核权。", sources.into_iter()).is_ok());
        let sources = [
            "Morgan owns the budget review.",
            "Alice owns the budget review rights.",
        ];
        assert!(validate("Morgan拥有预算审核权。", sources.into_iter()).is_err());
    }

    #[test]
    fn source_predicates_preserve_action_and_modality_without_coercing_ambiguous_roles() {
        for (source, reason, actor, predicate) in [
            (
                "Morgan will send the release report on Thursday.",
                Mismatch::Delivery,
                "Morgan",
                "将于周四发送",
            ),
            (
                "Taylor will submit the budget report on Monday.",
                Mismatch::Delivery,
                "Taylor",
                "将于周一提交",
            ),
            (
                "Nora will deliver the technical report on Friday.",
                Mismatch::Delivery,
                "Nora",
                "将于周五交付",
            ),
            (
                "Riley owns the latency report and will deliver it on Thursday.",
                Mismatch::Responsibility,
                "Riley",
                "负责",
            ),
        ] {
            let found = anchors(source, reason);
            assert_eq!(found.len(), 1, "{source}");
            assert_eq!(found[0].actor, actor);
            assert_eq!(found[0].predicate, predicate);
        }
        for source in [
            "Morgan may send the release report.",
            "Morgan should send the release report.",
            "Morgan will send the release report if approved.",
            "Morgan will send the release report on Thursday,if approved.",
            "Morgan will send the release report on Thursday or Friday.",
            "Morgan will send the release report on Thursday/Friday.",
            "Morgan will send the release report on Thursday-Friday.",
            "Morgan will send the release report on Thursday and Friday.",
            "Morgan will send the release report on Thursday, probably.",
            "Morgan will send the report. Morgan will publish another report.",
            "Morgan will send the report. It was cancelled.",
            "Morgan owns the original paper report and will deliver it on Thursday.",
            "Morgan owns a car and will deliver the report.",
            "Morgan owns the report and Alice will deliver it.",
            "Ann Morgan will send the report. Ben Morgan will publish it.",
        ] {
            for reason in [Mismatch::Delivery, Mismatch::Responsibility] {
                assert!(anchors(source, reason).is_empty(), "{source}");
            }
        }
    }

    #[test]
    fn delivery_and_publication_are_bound_to_the_named_subject() {
        let source = "Morgan will send the release report on Thursday. Alice will publish the deployment report.";
        for body in [
            "Morgan将在周四发送发布报告。Alice将发布部署报告。",
            "Morgan将在周四提交报告。Alice将发表部署报告。",
            "Morgan将在发布会后发送报告。Alice将发表部署报告。",
            source,
        ] {
            assert!(validate(body, [source].into_iter()).is_ok(), "{body}");
        }
        for body in [
            "Morgan将在周四发布报告。Alice将发送部署报告。",
            "Morgan will publish the release report on Thursday.",
        ] {
            let error = validate(body, [source].into_iter()).unwrap_err();
            assert_eq!(error.downcast_ref::<Mismatch>(), Some(&Mismatch::Delivery));
        }
        // A real publication by this subject is not prohibited. Mixed tasks
        // for the same person need a richer object relation than this guard.
        for source in [
            "Morgan will publish the release report on Thursday.",
            "Morgan will send the latency report. Morgan will publish the release report.",
        ] {
            assert!(validate("Morgan将发布报告。", [source].into_iter()).is_ok());
        }
        // Do not collapse different full names to a shared last word.
        let source = "Ann Morgan will send the report. Ben Morgan will publish the report.";
        assert!(validate(
            "Ann Morgan将发送报告。Ben Morgan将发布报告。",
            [source].into_iter()
        )
        .is_ok());
    }

    #[test]
    fn only_explicit_task_and_delivery_ownership_is_checked() {
        let source = "Morgan owns the latency report and will deliver it on Thursday.";
        for body in [
            "Morgan负责延迟报告，并将在周四交付。",
            "延迟报告由Morgan负责，周四交付。",
            "Morgan is responsible for the latency report and will deliver it on Thursday.",
            source,
        ] {
            assert!(validate(body, [source].into_iter()).is_ok(), "{body}");
        }
        for body in [
            "Morgan拥有延迟报告，并将在周四交付。",
            "Morgan possesses the latency report and will deliver it on Thursday.",
        ] {
            let error = validate(body, [source].into_iter()).unwrap_err();
            assert_eq!(
                error.downcast_ref::<Mismatch>(),
                Some(&Mismatch::Responsibility)
            );
        }
        for (source, body) in [
            (
                "Morgan owns the original report and will deliver it on Thursday.",
                "Morgan拥有原版报告，并将在周四交付。",
            ),
            ("Morgan owns the report.", "Morgan拥有报告。"),
            (
                "Morgan owns the report and Alice will deliver it.",
                "Morgan拥有报告，Alice将交付。",
            ),
            (
                "Morgan owns a car and will deliver the report.",
                "Morgan拥有汽车并将交付报告。",
            ),
        ] {
            assert!(
                validate(body, [source].into_iter()).is_ok(),
                "{source}: {body}"
            );
        }
    }

    #[test]
    fn an_unrelated_source_cannot_supply_the_same_subjects_publication() {
        let sources = [
            "Morgan will send the release report.",
            "Alice will publish the deployment report.",
        ];
        assert!(validate("Morgan将发布报告。", sources.into_iter()).is_err());
        assert!(validate("Morgan将发送报告。Alice将发布报告。", sources.into_iter()).is_ok());
        assert!(validate("Alice将发布报告。", [sources[1]].into_iter()).is_ok());
    }
}
