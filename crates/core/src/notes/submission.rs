//! Explicit Chinese submission slots. Unmarked schedules do not establish
//! completed delivery; this is not a general tense or event extractor.
use anyhow::Result;
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug)]
pub(crate) struct Mismatch;
impl Mismatch {
    pub(crate) fn hint(&self) -> &'static str {
        " Preserve the stated submission/sending object and weekday. An unmarked schedule is not evidence that delivery already happened. Keep test links separate from releasing a page, and keep all other source clauses."
    }
}
impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.hint().trim())
    }
}
impl std::error::Error for Mismatch {}

struct Frame {
    actor: Option<String>,
    object: String,
    action: String,
    day: String,
    more: bool,
}
fn patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| [
        r"(?:^|[，,])\s*(?P<object>测试结果|测试报告|结果|报告)(?:将|会|要)?(?:在|于)?(?P<day>(?:下周|本周|星期|周)[一二三四五六日天])(?P<action>提交|发送|送交|交付|发)",
        r"(?:^|[，,])\s*(?P<actor>[\p{Han}]{2,9})(?:在|于)(?P<day>(?:下周|本周|星期|周)[一二三四五六日天])(?P<action>提交|发送|送交|交付|发)(?P<object>测试结果|测试报告|测试链接|结果|报告|链接)",
        r"(?:^|[，,])\s*(?P<day>(?:下周|本周|星期|周)[一二三四五六日天])(?:将|会|要)?(?P<action>提交|发送|送交|交付|发)(?P<object>测试结果|测试报告|测试链接|结果|报告|链接)",
    ].into_iter().map(|p| Regex::new(p).unwrap()).collect())
}
fn frames(source: &str) -> Vec<Frame> {
    let mut result = Vec::new();
    for sentence in source.split_inclusive(['。', '.', ';', '；', '!', '！', '?', '？', '\n']) {
        if sentence.trim_end().ends_with(['?', '？'])
            || [
                "如果", "假如", "是否", "或者", "或许", "可能", "取消", "撤回", "不再", "改为",
                "改成",
            ]
            .iter()
            .any(|s| sentence.contains(s))
        {
            continue;
        }
        for pattern in patterns() {
            for c in pattern.captures_iter(sentence) {
                let whole = c.get(0).unwrap();
                if c.name("actor").is_some_and(|m| {
                    let actor = m.as_str();
                    [
                        "已经", "早已", "曾经", "之前", "此前", "先前", "刚刚", "去年", "昨天",
                        "前天", "昨日",
                    ]
                    .iter()
                    .any(|cue| actor.starts_with(cue) || actor.ends_with(cue))
                        || actor.ends_with(['已', '曾', '刚'])
                }) {
                    continue;
                }
                if sentence[whole.end()..]
                    .trim_start()
                    .chars()
                    .next()
                    .is_some_and(|c| !"，,。.!?！？；;\n".contains(c))
                {
                    continue;
                }
                result.push(Frame {
                    actor: c.name("actor").map(|m| m.as_str().to_owned()),
                    object: c["object"].into(),
                    action: c["action"].into(),
                    day: c["day"].into(),
                    more: source
                        .replacen(whole.as_str(), "", 1)
                        .chars()
                        .any(char::is_alphanumeric),
                });
                if result.len() == 3 {
                    return result;
                }
            }
        }
    }
    result
}
fn object_matches(object: &str, body: &str) -> bool {
    if body.contains(object) {
        return true;
    }
    static OBJECTS: OnceLock<Vec<(&str, Regex)>> = OnceLock::new();
    OBJECTS
        .get_or_init(|| {
            [
                ("结果", r"(?i)\b(?:test\s+)?results?\b"),
                ("测试结果", r"(?i)\btest(?:ing)?\s+results?\b"),
                ("报告", r"(?i)\breports?\b"),
                ("测试报告", r"(?i)\btest(?:ing)?\s+reports?\b"),
                ("链接", r"(?i)\blinks?\b"),
                ("测试链接", r"(?i)\btest(?:ing)?\s+links?\b"),
            ]
            .into_iter()
            .map(|(s, p)| (s, Regex::new(p).unwrap()))
            .collect()
        })
        .iter()
        .any(|(s, p)| *s == object && p.is_match(body))
}
fn day(day: &str) -> String {
    let name = match day.chars().next_back().unwrap() {
        '一' => "Monday",
        '二' => "Tuesday",
        '三' => "Wednesday",
        '四' => "Thursday",
        '五' => "Friday",
        '六' => "Saturday",
        _ => "Sunday",
    };
    format!(
        "{name}{}",
        if day.starts_with("下周") {
            " next week"
        } else if day.starts_with("本周") {
            " this week"
        } else {
            ""
        }
    )
}
fn has_completed_object(source: &str, object: &str) -> bool {
    static COMPLETED: OnceLock<Vec<Regex>> = OnceLock::new();
    let patterns = COMPLETED.get_or_init(|| {
        let object = r"(?P<object>测试结果|测试报告|测试链接|结果|报告|链接)";
        let action = r"(?:提交|发送|送交|交付|发)";
        let day = r"(?:(?:在|于)?(?:上周|下周|本周|星期|周)[一二三四五六日天])?";
        [
            format!(r"(?:已经|早已|曾经|之前|此前|刚刚)[^，,。.;；\n]{{0,16}}{action}{object}"),
            format!(r"(?:已|曾|刚){day}{action}{object}"),
            format!(r"{object}(?:已经|早已|已|曾经|曾){day}{action}"),
            format!(r"{action}{object}(?:了|过)"),
        ]
        .into_iter()
        .map(|p| Regex::new(&p).unwrap())
        .collect()
    });
    source
        .split(['。', '.', ';', '；', '，', ',', '\n'])
        .any(|clause| {
            patterns.iter().any(|p| {
                p.captures_iter(clause).any(|c| {
                    let evidence = &clause[..c.get(0).unwrap().end()];
                    ![
                        "如果", "假如", "是否", "可能", "没有", "尚未", "未曾", "不曾", "计划",
                        "打算", "考虑", "原定", "安排", "将在", "将于", "将会",
                    ]
                    .iter()
                    .any(|cue| evidence.contains(cue))
                        && object_matches(object, &c["object"])
                })
            })
        })
}
fn invented_page_publication(body: &str, source: &str) -> bool {
    static PUBLICATION: OnceLock<Regex> = OnceLock::new();
    static NEGATIVE: OnceLock<Regex> = OnceLock::new();
    static PAGE: OnceLock<Regex> = OnceLock::new();
    let publication = PUBLICATION.get_or_init(|| {
        Regex::new(r"(?i)发布|公布|上线|\b(?:publish\w*|releas\w*|launch\w*)\b").unwrap()
    });
    let negative = NEGATIVE.get_or_init(|| Regex::new(r"(?:不|未|没|尚未|还没|没有|不得|不能|不会|不应)(?:再|正式|公开|对外)?(?:发布|公布|上线)|(?i:\b(?:not|never|cannot|can't|won't)(?:\s+(?:yet|be|been|being|going|to))*\s+(?:publish\w*|releas\w*|launch\w*)\b)").unwrap());
    // Other affirmative publication context is outside this bounded contrast:
    // do not guess its actor/object, condition or date from a word match.
    if source
        .split(['。', '.', ';', '；', '，', ',', '\n'])
        .any(|c| publication.is_match(c) && !negative.is_match(c))
    {
        return false;
    }
    let page = PAGE.get_or_init(|| Regex::new(r"(?i)(?:\b(?:pages?|webpages?|websites?)\b\s*,?\s*(?:(?:which|that)\s+)?(?:(?:will|shall|is|are|was|were|has|have|had|already|been|be|being|going|to)\s+)*(?:published|released|launched)\b|\b(?:publish|release|launch)\s+(?:(?:the|a|an|new|booking|reservation|tour|test)\s+)*(?:page|webpage|website)\b|(?:页面|网页)(?:(?:将|会|已经|已|在|于|下周|本周|星期|周|[一二三四五六日天]){0,10})(?:发布|公布|上线)|(?:发布|公布|上线)(?:参观|预约|测试|新|的){0,5}(?:页面|网页))").unwrap());
    body.split(['。', '.', ';', '；', '\n'])
        .any(|c| page.is_match(c) && !negative.is_match(c))
}
pub(super) fn validate(body: &str, source: &str, literal: bool) -> Result<()> {
    static DONE: OnceLock<Regex> = OnceLock::new();
    let done = DONE.get_or_init(|| Regex::new(r"(?i)\b(?:was|were|has\s+been|have\s+been|had\s+been|already)\s+(?:successfully\s+)?(?:submitted|sent|delivered)\b|(?:已|已经)(?:提交|发送|送交|交付)").unwrap());
    static ACTION: OnceLock<Regex> = OnceLock::new();
    let action = ACTION.get_or_init(|| Regex::new(r"(?i)\b(?:submit\w*|sen[dt]\w*|deliver\w*|releas\w*|publish\w*)\b|提交|发送|送交|交付|发布").unwrap());
    static PAST: OnceLock<Regex> = OnceLock::new();
    static UNFINISHED: OnceLock<Regex> = OnceLock::new();
    let past = PAST.get_or_init(|| Regex::new(r"(?i)\b(?:submitted|sent|delivered)\b").unwrap());
    let unfinished = UNFINISHED.get_or_init(|| Regex::new(r"(?i)(?:(?:will|shall|would|should|must|may|can|could)\s+(?:have\s+)?(?:been|be)|to\s+be|(?:is|are)\s+(?:to\s+be)?|not\s+(?:yet\s+)?(?:been\s+)?)\s*$").unwrap());
    for f in frames(source) {
        let named_day = day(&f.day).split_whitespace().next().unwrap().to_owned();
        let has_object = object_matches(&f.object, body);
        if !has_object && (literal || body.contains(&named_day) && action.is_match(body)) {
            return Err(Mismatch.into());
        }
        if f.object.ends_with("链接") && invented_page_publication(body, source) {
            return Err(Mismatch.into());
        }
        // A different actor may already have submitted an object of this kind.
        // Without exact multilingual actor binding, preserve that legitimate
        // completion rather than applying the schedule to every report/link.
        if has_completed_object(source, &f.object) {
            continue;
        }
        for clause in body.split(['。', '.', ';', '；', '\n']) {
            if object_matches(&f.object, clause)
                && (done.is_match(clause)
                    || past
                        .find_iter(clause)
                        .any(|m| !unfinished.is_match(&clause[..m.start()])))
            {
                return Err(Mismatch.into());
            }
        }
    }
    Ok(())
}
pub(crate) struct Anchor {
    pub actor: Option<String>,
    pub text: String,
    pub require_context: bool,
}
/// Actor spellings must be corroborated elsewhere in the current source scope.
/// Never split a possible three-character name merely because it ends in 先.
pub(crate) fn anchor(source: &str, known: &[String]) -> Option<Anchor> {
    let mut frames = frames(source);
    if frames.len() != 1 {
        return None;
    }
    let f = frames.pop()?;
    let actor = if let Some(raw) = f.actor {
        Some(
            known
                .iter()
                .filter(|n| {
                    raw.strip_prefix(n.as_str())
                        .is_some_and(|tail| ["", "先", "将", "会", "要", "将会"].contains(&tail))
                })
                .max_by_key(|n| n.len())?
                .clone(),
        )
    } else {
        None
    };
    let object = match f.object.as_str() {
        "测试结果" => "the test results",
        "结果" => "the results",
        "测试报告" => "the test report",
        "报告" => "the report",
        "测试链接" => "the test link",
        _ => "the link",
    };
    let verb = match f.action.as_str() {
        "提交" => "submit",
        "交付" => "deliver",
        _ => "send",
    };
    let text = if actor.is_some() {
        format!(" is to {verb} {object} on {}", day(&f.day))
    } else {
        let passive = match verb {
            "submit" => "submitted",
            "deliver" => "delivered",
            _ => "sent",
        };
        let object = format!("{}{}", object[..1].to_ascii_uppercase(), &object[1..]);
        format!(
            "{object} {} to be {passive} on {}.",
            if f.object.ends_with("结果") {
                "are"
            } else {
                "is"
            },
            day(&f.day)
        )
    };
    Some(Anchor {
        actor,
        text,
        require_context: f.more,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unmarked_submission_does_not_prove_completion_or_a_different_object() {
        let s = "实时转写的延迟测试由我负责，结果周四提交。";
        assert!(validate(
            "I am responsible for the latency test. The results are to be submitted on Thursday.",
            s,
            true
        )
        .is_ok());
        assert!(validate(
            "The test I am responsible for was submitted on Thursday.",
            s,
            true
        )
        .is_err());
        assert!(validate("The results were submitted on Thursday.", s, true).is_err());
        assert!(validate("The results will be submitted on Thursday.", s, true).is_ok());
        let s = "参观预约页面由陈老师负责，下周三发测试链接，页面尚未完成。";
        assert!(validate("陈老师 is responsible for the unfinished page. The test link will be sent on Wednesday next week.",s,true).is_ok());
        assert!(validate(
            "The page will be released Wednesday next week, and is unfinished.",
            s,
            true
        )
        .is_err());
        assert!(validate(s, s, false).is_ok());
    }
    #[test]
    fn completion_conditions_negation_and_actual_page_release_are_not_schedules() {
        for s in [
            "结果已经在周四提交了。",
            "结果上周四提交了。",
            "结果周四不提交。",
            "如果通过验收，结果周四提交。",
            "结果周四提交吗？",
            "下周三发布预约页面。",
            "结果周四提交的安排已经取消。",
        ] {
            assert!(anchor(s, &[]).is_none(), "{s}");
        }
        assert!(validate(
            "The report was submitted on Thursday.",
            "报告已在周四提交。",
            true
        )
        .is_ok());
    }
    #[test]
    fn named_schedule_requires_corroboration_and_keeps_the_longest_exact_label() {
        let s = "林悦先在周四提交测试报告。";
        assert!(anchor(s, &[]).is_none());
        let a = anchor(s, &["林悦".into()]).unwrap();
        assert_eq!(a.actor.as_deref(), Some("林悦"));
        assert_eq!(a.text, " is to submit the test report on Thursday");
        assert_eq!(
            anchor("张敬先在周四提交报告。", &["张敬先".into(), "张敬".into()])
                .unwrap()
                .actor
                .as_deref(),
            Some("张敬先")
        );
    }
    #[test]
    fn actual_named_completion_is_not_an_unmarked_schedule() {
        for source in [
            "林悦已经在周四提交测试报告。",
            "林悦曾在周四提交测试报告。",
            "之前林悦在周四提交测试报告。",
            "林悦之前在周四提交测试报告。",
            "林悦已在周四提交测试报告。",
            "测试报告已经在周四提交。",
        ] {
            assert!(anchor(source, &["林悦".into()]).is_none(), "{source}");
            assert!(
                validate("林悦 submitted the test report on Thursday.", source, true).is_ok(),
                "{source}"
            );
        }
        let source = "林悦在周四提交测试报告。陈明已经提交测试报告。";
        assert!(validate(
            "林悦 is to submit the test report on Thursday. 陈明 has submitted the test report.",
            source,
            true
        )
        .is_ok());
        // Mixed completion abstains only from completion rejection; it cannot
        // omit the scheduled object or turn link sending into page release.
        assert!(validate("There is a meeting on Thursday.", source, true).is_err());
        let source = "结果周四提交。";
        assert!(validate("The results will be submitted on Thursday.", source, true).is_ok());
        assert!(validate(
            "The results are scheduled to be submitted on Thursday.",
            source,
            true
        )
        .is_ok());
        assert!(validate("The results were submitted on Thursday.", source, true).is_err());
        assert!(anchor("曾琳在周四提交报告。", &["曾琳".into()]).is_some());
    }
    #[test]
    fn sending_a_test_link_does_not_establish_page_publication() {
        let source = "参观预约页面由陈老师负责，下周三发测试链接，页面尚未完成。";
        assert!(validate("陈老师 is responsible for the unfinished reservation page. The test link is to be sent on Wednesday next week. The page will be released Wednesday next week.",source,true).is_err());
        assert!(validate(
            "The test link will be sent on Wednesday next week. The page has not been released.",
            source,
            true
        )
        .is_ok());
        let source = "下周三发测试链接。预约页面将于周五发布。";
        assert!(validate("The test link will be sent on Wednesday next week. The reservation page will be released on Friday.",source,true).is_ok());
        let source = "预约页面将于周五发布。测试链接已经发送。";
        assert!(validate(
            "The reservation page will be released on Friday. The test link has already been sent.",
            source,
            true
        )
        .is_ok());
        let source = "下周三发测试链接。页面尚未发布。";
        assert!(validate("The test link will be sent on Wednesday next week. The page will be released on Friday.",source,true).is_err());
        for source in [
            "如果验收通过，下周三发测试链接。",
            "下周三发测试链接的安排已经取消。",
        ] {
            assert!(anchor(source, &[]).is_none());
        }
    }
}
