//! Bounded contrasts for explicit, named report tasks. These checks do not
//! resolve ambiguous ownership or establish general semantic entailment.
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
            Self::Delivery => " 该句的动作是发送：send 译作“发送”。报告名称与发送动作分开翻译，保留原期限。Keep the sending action and the report title separately.",
            Self::Responsibility => " 该句是任务分工：owns the report 译作“负责报告”。保留同一负责人、报告名称、交付动作和期限。Use responsibility for this task-and-delivery context.",
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
        r"(?:(?:will|would|shall|can|could|must|should)\s+)?",
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

pub(super) fn validate<'a>(body: &str, sources: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut evidence: BTreeMap<String, Evidence> = BTreeMap::new();
    for source in sources {
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
