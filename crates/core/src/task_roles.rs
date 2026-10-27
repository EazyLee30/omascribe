//! Explicit task-by-actor responsibility frames. Preserve the observed actor
//! label; do not resolve identities, transliterate names or infer assignments.
use anyhow::Result;
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug)]
pub(crate) struct Mismatch;
impl Mismatch {
    pub(crate) fn hint(&self) -> &'static str {
        " Preserve the stated task owner and direction: in 任务由姓名负责, the named actor is responsible for the task, not the reverse. Copy the supplied actor spelling; preserve all other clauses and consent conditions."
    }
}
impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.hint().trim())
    }
}
impl std::error::Error for Mismatch {}

pub(crate) struct Assignment {
    pub actor: String,
    task: String,
    end: usize,
}
fn frames(source: &str) -> Vec<Assignment> {
    static FRAME: OnceLock<Regex> = OnceLock::new();
    let frame = FRAME.get_or_init(|| Regex::new(concat!(
        r"(?:^|[。.!?！？；;，,\n])\s*(?P<task>[^。.!?！？；;，,\n]{2,64}?)由\s*",
        r"(?P<actor>[\p{Han}]{2,6}|[A-Z][A-Za-z'’-]{1,39}(?:[ \t]+[A-Z][a-z][A-Za-z'’-]{0,38}){0,2})\s*负责",
    )).unwrap());
    frame
        .captures_iter(source)
        .filter_map(|c| {
            if source[c.get(0)?.end()..]
                .trim_start()
                .chars()
                .next()
                .is_some_and(|ch| !"。.!?！？；;，,\n".contains(ch))
            {
                return None;
            }
            Some(Assignment {
                actor: c["actor"].into(),
                task: c["task"].trim().into(),
                end: c.get(0)?.end(),
            })
        })
        .take(12)
        .collect()
}
fn references(source: &str) -> Vec<Assignment> {
    frames(source)
        .into_iter()
        .filter(|a| {
            let actor = a.actor.as_str();
            // Pronouns, generic staff/device roles and alternatives are not name
            // spellings. An unknown label stays literal; no person role is inferred.
            if [
                "我们",
                "你们",
                "他们",
                "她们",
                "用户",
                "客服",
                "客服团队",
                "团队",
                "小组",
                "同事",
                "部门",
                "系统",
                "服务器",
                "程序",
                "工程师",
                "负责人",
                "We",
                "They",
                "Team",
                "Someone",
            ]
            .iter()
            .any(|v| actor.eq_ignore_ascii_case(v))
                || actor.contains(['和', '或', '与', '及', '、'])
            {
                return false;
            }
            true
        })
        .collect()
}
pub(crate) fn has_frame(source: &str) -> bool {
    !frames(source).is_empty()
}
/// Keep the task/actor clause separate from its independent consent or release
/// condition. Return original byte offsets, retaining every source character.
pub(crate) fn boundary(source: &str) -> Option<usize> {
    frames(source).into_iter().find_map(|a| {
        let separator = source[a.end..].chars().next()?;
        let end = a.end + separator.len_utf8();
        (matches!(separator, '，' | ',' | '；' | ';') && !source[end..].trim().is_empty())
            .then_some(end)
    })
}
pub(crate) fn actors(source: &str) -> Vec<String> {
    let mut names: Vec<_> = references(source).into_iter().map(|a| a.actor).collect();
    names.sort();
    names.dedup();
    names
}
pub(crate) fn assignments(source: &str) -> Vec<Assignment> {
    // Do not force an old or hypothetical assignment through a later change.
    if [
        "不再负责",
        "不再由",
        "不会由",
        "不能由",
        "不需要由",
        "未由",
        "暂定由",
        "或许由",
        "拟由",
        "应由",
        "需要由",
        "不由",
        "不是由",
        "并非由",
        "不负责",
        "不应由",
        "改由",
        "转由",
        "接替",
        "取消",
        "撤回",
        "是否",
        "如果",
        "假如",
        "可能",
        "计划由",
        "将由",
        "曾由",
        "之前由",
    ]
    .iter()
    .any(|p| source.contains(p))
        || source.contains(['?', '？'])
    {
        return Vec::new();
    }
    references(source)
        .into_iter()
        .filter(|a| !a.task.contains(['？', '?']) && source.matches(&a.actor).count() == 1)
        .collect()
}
pub(crate) fn validate(source: &str, body: &str) -> Result<()> {
    static BUILD: OnceLock<Regex> = OnceLock::new();
    if frames(source).iter().any(|a| a.task.contains("构建"))
        && !body.contains("构建")
        && !BUILD
            .get_or_init(|| {
                Regex::new(r"(?i)\b(?:build(?:s|ing)?|construct(?:ion|ing|s)?)\b").unwrap()
            })
            .is_match(body)
    {
        return Err(Mismatch.into());
    }
    static AFTER: OnceLock<Regex> = OnceLock::new();
    static PASSIVE: OnceLock<Regex> = OnceLock::new();
    let after = AFTER.get_or_init(|| Regex::new(r"(?i)^\s*(?:(?:is\s+(?:(?:currently|now|still)\s+)?responsible\s+for)\b|(?:handles|manages|oversees)\b|(?:仍|将)?(?:负责|承担|管理)|(?:是|为)[^，,。.;；\n]{0,40}负责人)").unwrap());
    let passive = PASSIVE.get_or_init(|| Regex::new(r"(?i)(?:(?:handled|managed|overseen)\s+by\s*|responsibility\s+[^.;。；\n]{0,60}\s+(?:lies|rests)\s+with\s*|负责人(?:是|为))$").unwrap());
    for a in assignments(source) {
        let occurrences: Vec<_> = body
            .match_indices(&a.actor)
            .filter(|(i, _)| {
                !a.actor.is_ascii()
                    || (body[..*i]
                        .chars()
                        .next_back()
                        .is_none_or(|c| !c.is_ascii_alphanumeric())
                        && body[*i + a.actor.len()..]
                            .chars()
                            .next()
                            .is_none_or(|c| !c.is_ascii_alphanumeric()))
            })
            .collect();
        if !occurrences.iter().any(|(i, _)| {
            after.is_match(&body[i + a.actor.len()..])
                || passive.is_match(
                    body[..*i]
                        .rsplit(['。', '.', ';', '；', '\n'])
                        .next()
                        .unwrap_or(""),
                )
        }) {
            return Err(Mismatch.into());
        }
        // A second correct clause must not mask the original reversed one.
        if occurrences.iter().any(|(i, _)| {
            body[..*i]
                .trim_end()
                .to_ascii_lowercase()
                .ends_with("responsible for")
        }) {
            return Err(Mismatch.into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn task_qualifiers_and_following_conditions_keep_independent_source_parts() {
        for source in [
            "构建检查由李华负责，完成前不能发布。",
            "构建任务由服务器负责，人工不需要介入。",
        ] {
            let end = boundary(source).unwrap();
            assert!(source[..end].ends_with('，'));
            assert!(!source[end..].is_empty());
            assert!(validate(source, "The task is handled by the server.").is_err());
        }
        assert!(validate(
            "构建任务由服务器负责，人工不需要介入。",
            "The build task is handled by the server. No human intervention is required."
        )
        .is_ok());
    }
    #[test]
    fn explicit_roles_keep_direction_and_literal_actor_labels() {
        let source = "离线模型下载由陈明负责，下载必须获得用户同意。";
        assert_eq!(actors(source), ["陈明"]);
        for body in [
            "陈明 is responsible for offline model download; downloading requires user consent.",
            "Offline model download is handled by 陈明 and requires user consent.",
            source,
        ] {
            assert!(validate(source, body).is_ok(), "{body}");
        }
        for body in [
            "Offline model download is responsible for 陈明.",
            "Offline model download needs consent from 陈明.",
            "陈明 is not responsible for offline model download.",
            "陈明 is responsible for download. Offline model download is responsible for 陈明.",
        ] {
            assert!(validate(source, body).is_err(), "{body}");
        }
        assert_eq!(actors("构建任务由李华负责。"), ["李华"]);
        assert_eq!(actors("构建任务由Nora负责。"), ["Nora"]);
        assert_eq!(actors("下载由陈明负责，构建检查由李华负责。").len(), 2);
        assert!(validate(
            "下载由陈明负责，构建检查由李华负责。",
            "陈明负责下载，李华负责构建检查。"
        )
        .is_ok());
        for role in ["用户", "客服团队", "服务器", "我们", "陈明或李华"] {
            assert!(actors(&format!("构建任务由{role}负责。")).is_empty());
        }
    }
    #[test]
    fn changed_hypothetical_and_negative_assignments_do_not_force_a_current_role() {
        for source in [
            "下载不由陈明负责。",
            "下载不再由陈明负责。",
            "下载是否由陈明负责？",
            "如果下载由陈明负责，先确认。",
            "下载可能由陈明负责。",
            "下载将由陈明负责。",
            "下载由陈明负责，现在改由李华接手。",
            "下载之前由陈明负责。",
        ] {
            assert!(assignments(source).is_empty(), "{source}");
        }
    }
}
