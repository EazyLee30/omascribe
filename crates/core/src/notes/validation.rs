//! Deterministic guardrails, not a proof of semantic entailment. All generated prose retains citations.
use crate::{Segment, Source, Suggestion};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::HashSet;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Draft {
    sections: Vec<Section>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Section {
    title: String,
    body: String,
    sources: Vec<usize>,
}
fn numbers(s: &str) -> HashSet<String> {
    s.split(|c: char| !c.is_ascii_digit() && c != '.')
        .filter(|s| s.chars().any(|c| c.is_ascii_digit()))
        .map(|s| s.trim_matches('.').to_owned())
        .collect()
}
fn uncertain(s: &str) -> bool {
    let s = s.to_lowercase();
    [
        "未",
        "不定",
        "待",
        "暂不",
        "暂缓",
        "再看",
        "还没",
        "不确定",
        "not decided",
        "undecided",
        "pending",
        "unconfirmed",
        "not yet",
        "deferred",
        "not final",
    ]
    .iter()
    .any(|q| s.contains(q))
}
fn normalized_time(text: &str) -> String {
    let mut text = text.to_lowercase();
    for (english, chinese) in [
        ("monday", "一"),
        ("tuesday", "二"),
        ("wednesday", "三"),
        ("thursday", "四"),
        ("friday", "五"),
        ("saturday", "六"),
        ("sunday", "日"),
    ] {
        text = text.replace(&format!("next {english}"), &format!("下周{chinese}"));
        text = text.replace(&format!("last {english}"), &format!("上周{chinese}"));
        text = text.replace(&format!("this {english}"), &format!("周{chinese}"));
        text = text.replace(english, &format!("周{chinese}"));
        text = text.replace(&format!("本周{chinese}"), &format!("周{chinese}"));
        text = text.replace(&format!("星期{chinese}"), &format!("周{chinese}"));
    }
    for (english, chinese) in [
        ("today", "今天"),
        ("tomorrow", "明天"),
        ("yesterday", "昨天"),
        ("next week", "下周"),
        ("this week", "本周"),
        ("last week", "上周"),
    ] {
        text = text.replace(english, chinese);
    }
    text
}
pub fn validate(content: &str, segments: &[Segment], meeting: &str) -> Result<Vec<Suggestion>> {
    let draft: Draft =
        serde_json::from_str(content).context("文档结构校验失败，待处理内容已保留")?;
    if draft.sections.len() > 3 {
        bail!("文档超过本轮段落预算");
    }
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for section in draft.sections {
        let body = section.body.trim();
        let title = section.title.trim();
        if body.is_empty()
            || title.is_empty()
            || body.chars().count() > 1200
            || title.chars().count() > 60
            || section.sources.is_empty()
            || section.sources.len() > 9
        {
            bail!("文档段落或出处无效");
        }
        if body
            .lines()
            .any(|l| l.trim_start().starts_with(['#', '*', '-', '•']))
            || title.contains(['\n', '#', '*'])
        {
            bail!("模型没有返回段落形式的文档");
        }
        let mut sources = Vec::new();
        for index in section.sources {
            let s = segments
                .get(index.checked_sub(1).context("无效出处编号")?)
                .context("模型引用了不存在的片段")?;
            if !sources.iter().any(|q: &Source| q.segment_id == s.id) {
                sources.push(Source {
                    segment_id: s.id.clone(),
                    revision: s.revision,
                    start_ms: s.start_ms,
                    end_ms: s.end_ms,
                    quote: s.text.clone(),
                });
            }
        }
        let evidence = sources
            .iter()
            .map(|s| s.quote.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        if !numbers(&format!("{title} {body}")).is_subset(&numbers(&evidence)) {
            bail!("文档出现无出处的数字，本轮未提交");
        }
        // A conservative uncertainty check catches the common 'budget undecided → approved' reversal.
        for time in [
            "今天",
            "明天",
            "昨天",
            "本周",
            "下周",
            "上周",
            "周一",
            "周二",
            "周三",
            "周四",
            "周五",
            "周六",
            "周日",
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
            "Sunday",
        ] {
            if normalized_time(body).contains(&normalized_time(time))
                && !normalized_time(&evidence).contains(&normalized_time(time))
            {
                bail!("文档包含与出处不符的相对日期（{time}），本轮未提交");
            }
        }
        if ["已告知", "已提供", "已完成", "已经完成"]
            .iter()
            .any(|word| body.contains(word) && !evidence.contains(word))
        {
            bail!("文档可能将计划写成了已完成事项，本轮未提交");
        }
        let shares_uncertain_topic = ["预算", "budget", "金额", "launch", "发布日期", "日期"]
            .iter()
            .any(|topic| {
                evidence.to_lowercase().contains(topic) && body.to_lowercase().contains(topic)
            });
        if shares_uncertain_topic && uncertain(&evidence) && !uncertain(body) {
            bail!("文档可能遗漏待确认状态，本轮未提交");
        }
        if !seen.insert(body.to_string()) {
            continue;
        }
        result.push(Suggestion {
            id: String::new(),
            meeting_id: meeting.into(),
            kind: "section".into(),
            focus: title.into(),
            text: body.into(),
            sources,
            status: "pending".into(),
            stale: false,
        });
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn data() -> Vec<Segment> {
        vec![Segment {
            id: "s1".into(),
            meeting_id: "m".into(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 1000,
            text: "张老师周五提供测试结果，预算尚未确定，下周讨论。".into(),
            revision: 1,
        }]
    }
    #[test]
    fn rejects_guessed_amount_invalid_source_and_lost_uncertainty() {
        for input in [
            r#"{"sections":[{"title":"预算","body":"预算确定为3000。","sources":[1]}]}"#,
            r#"{"sections":[{"title":"预算","body":"预算已确定。","sources":[1]}]}"#,
            r#"{"sections":[{"title":"预算","body":"预算未定。","sources":[2]}]}"#,
        ] {
            assert!(validate(input, &data(), "m").is_err());
        }
        let sections=validate(r#"{"sections":[{"title":"测试与预算","body":"张老师将于周五提供测试结果。预算尚未确定，将于下周继续讨论。","sources":[1]}]}"#,&data(),"m").unwrap();
        assert_eq!(sections[0].sources[0].quote, data()[0].text);
    }
    #[test]
    fn bilingual_date_translation_is_not_an_unsupported_date() {
        let mut source = data();
        source[0].text = "We will test the prototype on Friday.".into();
        assert!(validate(
            r#"{"sections":[{"title":"测试安排","body":"原型将于周五进行测试。","sources":[1]}]}"#,
            &source,
            "m"
        )
        .is_ok());
        assert!(validate(r#"{"sections":[{"title":"测试安排","body":"原型将于下周一进行测试。","sources":[1]}]}"#,&source,"m").is_err());
    }
}
