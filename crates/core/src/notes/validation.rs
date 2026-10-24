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
// Date spelling may legitimately change between ASR and prose. Normalize only
// calendar-number fields; ordinary Chinese numerals remain untouched.
fn chinese_number(text: &str) -> Option<u32> {
    fn digit(c: char) -> Option<u32> {
        match c {
            '零' | '〇' => Some(0),
            '一' => Some(1),
            '二' | '两' => Some(2),
            '三' => Some(3),
            '四' => Some(4),
            '五' => Some(5),
            '六' => Some(6),
            '七' => Some(7),
            '八' => Some(8),
            '九' => Some(9),
            _ => None,
        }
    }
    if text.is_empty() || text.chars().count() > 12 {
        return None;
    }
    if text.chars().all(|c| digit(c).is_some()) {
        return text
            .chars()
            .try_fold(0u32, |n, c| n.checked_mul(10)?.checked_add(digit(c)?));
    }
    let (mut sum, mut current, mut previous_unit) = (0u32, None, 10_000u32);
    for c in text.chars() {
        if let Some(d) = digit(c) {
            if current.is_some_and(|n| n != 0) {
                return None;
            }
            current = Some(d);
        } else {
            let unit = match c {
                '十' => 10,
                '百' => 100,
                '千' => 1000,
                _ => return None,
            };
            if unit >= previous_unit {
                return None;
            }
            sum += current.unwrap_or(1) * unit;
            current = None;
            previous_unit = unit;
        }
    }
    Some(sum + current.unwrap_or(0))
}
fn normalize_date_numbers(text: &str) -> String {
    let mut result = String::new();
    let mut pending = String::new();
    for c in text.chars() {
        if "零〇一二两三四五六七八九十百千".contains(c) {
            pending.push(c);
            continue;
        }
        if !pending.is_empty() {
            let max = match c {
                '年' => 9999,
                '月' => 12,
                '日' | '号' => 31,
                _ => 0,
            };
            // Digit-by-digit years are common; "一二月" can mean January or
            // February, so do not reinterpret bare multi-digit month/day words.
            let unambiguous =
                c == '年' || pending.chars().count() == 1 || pending.contains(['十', '百', '千']);
            if let Some(n) = chinese_number(&pending).filter(|n| unambiguous && *n > 0 && *n <= max)
            {
                result.push_str(&n.to_string());
            } else {
                result.push_str(&pending);
            }
            pending.clear();
        }
        result.push(c);
    }
    result.push_str(&pending);
    result
}
fn numbers(s: &str) -> HashSet<String> {
    normalize_clock_numbers(&normalize_date_numbers(s))
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .filter(|s| s.chars().any(|c| c.is_ascii_digit()))
        .map(|s| s.trim_matches('.').to_owned())
        .collect()
}
fn normalize_clock_numbers(text: &str) -> String {
    // Only reinterpret a complete hour/minute expression, never unrelated
    // Chinese numbers such as “十分重要” or a decimal amount like 5.44.
    static CLOCK: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    CLOCK
        .get_or_init(|| regex::Regex::new(r"(凌晨|早上|上午|中午|下午|傍晚|晚上|晚间)?([零〇一二两三四五六七八九十0-9]{1,4})[点點时時:：]([零〇一二两三四五六七八九十0-9]{1,4})(?:分)?").unwrap())
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let parse = |s: &str| s.parse::<u32>().ok().or_else(|| chinese_number(s));
            match (parse(&captures[2]), parse(&captures[3])) {
                (Some(mut h), Some(m)) if h < 24 && m < 60 => {
                    let period = captures.get(1).map_or("", |p| p.as_str());
                    if ["中午", "下午", "傍晚", "晚上", "晚间"].contains(&period) && h < 12 { h += 12; }
                    if ["凌晨", "早上", "上午"].contains(&period) && h == 12 { h = 0; }
                    format!("{h}:{m:02}")
                },
                _ => captures[0].to_owned(),
            }
        })
        .into_owned()
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
fn calendar_weekdays(text: &str) -> HashSet<String> {
    use chrono::Datelike;
    let text = normalize_date_numbers(text);
    let mut days = HashSet::new();
    for (index, _) in text.match_indices('年') {
        let year: String = text[..index]
            .trim_end()
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        if year.len() != 4 {
            continue;
        }
        let Some((month, rest)) = text[index + '年'.len_utf8()..].split_once('月') else {
            continue;
        };
        let day = rest.split(['日', '号']).next().unwrap_or("");
        let (Ok(year), Ok(month), Ok(day)) =
            (year.parse(), month.trim().parse(), day.trim().parse())
        else {
            continue;
        };
        if let Some(date) = chrono::NaiveDate::from_ymd_opt(year, month, day) {
            days.insert(format!(
                "周{}",
                ["一", "二", "三", "四", "五", "六", "日"]
                    [date.weekday().num_days_from_monday() as usize]
            ));
        }
    }
    days
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
    validate_with_speakers(content, segments, meeting, &Default::default())
}
pub fn validate_with_speakers(
    content: &str,
    segments: &[Segment],
    meeting: &str,
    names: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<Suggestion>> {
    let draft: Draft =
        serde_json::from_str(content).context("文档结构校验失败，待处理内容已保留")?;
    if draft.sections.len() > 3 {
        bail!("文档超过本轮段落预算");
    }
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for (index, section) in draft.sections.into_iter().enumerate() {
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
                    speaker_name: s.speaker.as_ref().and_then(|id| names.get(id)).cloned(),
                });
            }
        }
        let evidence = sources
            .iter()
            .map(|s| format!("{} {}", s.speaker_name.as_deref().unwrap_or(""), s.quote))
            .collect::<Vec<_>>()
            .join(" ");
        if [
            "参会者：",
            "参与者：",
            "出席者：",
            "attendees:",
            "participants:",
        ]
        .iter()
        .any(|label| body.to_lowercase().contains(label))
            && ![
                "参会",
                "参与者",
                "出席",
                "参加",
                "attendee",
                "participant",
                "attended",
            ]
            .iter()
            .any(|word| evidence.to_lowercase().contains(word))
        {
            bail!("The cited transcript does not provide an attendee list. Interests and user notes are not participant names. Remove the attendee list and summarize the actual transcript.");
        }
        for name in names.values().filter(|n| !n.is_empty()) {
            if body.contains(name) && !evidence.contains(name) {
                bail!("Unsupported speaker attribution: {name}. Passage IDs are not speaker numbers. The cited evidence is: {evidence}. Remove this name unless it appears in that evidence; describe an unassigned passage without naming a speaker.");
            }
        }
        let attributes_speaker = sources.iter().any(|s| {
            s.speaker_name
                .as_ref()
                .is_some_and(|name| body.contains(name))
        });
        if attributes_speaker
            && sources.iter().any(|s| {
                s.speaker_name
                    .as_ref()
                    .is_none_or(|name| !body.contains(name))
            })
        {
            bail!("Ambiguous speaker attribution: this paragraph names a speaker but also cites another or unassigned voice without identifying it. Name each cited speaker with their own statement, or remove speaker names and summarize neutrally. Do not attribute all cited statements to one speaker. Evidence: {evidence}");
        }
        if !numbers(&format!("{title} {body}")).is_subset(&numbers(&evidence)) {
            bail!(
                "第 {} 段的数字或日期不在其引用的原文中，请核对出处编号；本轮未提交",
                index + 1
            );
        }
        let calendar_days = calendar_weekdays(&evidence);
        // An exact calendar date can support its actual weekday, never an arbitrary weekday.
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
                && !calendar_days.contains(&normalized_time(time))
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
        if shares_uncertain_topic
            && body.contains("暂定")
            && !evidence.contains("暂定")
            && ["不定", "未定", "未确定", "没有确定", "未决定"]
                .iter()
                .any(|s| evidence.contains(s))
        {
            bail!("文档将尚未决定写成暂定，本轮未提交");
        }
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
    #[test]
    fn spoken_clock_notation_and_twenty_four_hour_time_are_equivalent() {
        let mut source = data();
        source[0].text = "会议时间是二零二五年十月二十三日下午五点四十四分，讨论离线转写。".into();
        for time in ["下午5:44", "17:44"] {
            let draft = serde_json::json!({"sections":[{"title":"转写测试", "body":format!("2025年10月23日{time}讨论离线转写。"),"sources":[1]}]});
            assert!(validate(&draft.to_string(), &source, "m").is_ok());
        }
        for time in ["下午5:45", "上午5:44", "18:44"] {
            let draft = serde_json::json!({"sections":[{"title":"转写测试", "body":format!("会议时间是{time}。"),"sources":[1]}]});
            assert!(validate(&draft.to_string(), &source, "m").is_err());
        }
        assert!(!numbers("十分重要").contains("10"));
        assert_ne!(numbers("5.44"), numbers("五点四十四分"));
    }
    #[test]
    fn topic_preferences_cannot_become_an_attendee_list() {
        let mut source = data();
        source[0].text = "正在测试会议纪要和关键词提取。".into();
        let draft = serde_json::json!({"sections":[{"title":"参与者", "body":"会议参与者：测试、关键词、会议纪要。","sources":[1]}]});
        assert!(validate(&draft.to_string(), &source, "m").is_err());
    }
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
            speaker: None,
            speaker_manual: false,
        }]
    }
    #[test]
    fn rejects_guessed_amount_invalid_source_and_lost_uncertainty() {
        for input in [
            r#"{"sections":[{"title":"预算","body":"预算确定为3000。","sources":[1]}]}"#,
            r#"{"sections":[{"title":"预算","body":"预算已确定。","sources":[1]}]}"#,
            r#"{"sections":[{"title":"预算","body":"预算未定。","sources":[2]}]}"#,
            r#"{"sections":[{"title":"预算","body":"预算暂定，待下周再定。","sources":[1]}]}"#,
        ] {
            assert!(validate(input, &data(), "m").is_err());
        }
        let sections=validate(r#"{"sections":[{"title":"测试与预算","body":"张老师将于周五提供测试结果。预算尚未确定，将于下周继续讨论。","sources":[1]}]}"#,&data(),"m").unwrap();
        assert_eq!(sections[0].sources[0].quote, data()[0].text);
    }
    #[test]
    fn chinese_calendar_spelling_is_equivalent_but_new_numbers_are_rejected() {
        let mut source = data();
        source[0].text = "会议日期是二〇二四年五月八日。".into();
        assert!(validate(
            r#"{"sections":[{"title":"日期","body":"会议日期为2024年5月8日。","sources":[1]}]}"#,
            &source,
            "m"
        )
        .is_ok());
        assert!(validate(r#"{"sections":[{"title":"日期","body":"会议日期为2024年5月8日（周三）。","sources":[1]}]}"#, &source, "m").is_ok());
        for body in [
            "会议日期为2024年5月9日。",
            "会议日期为2024年5月8日（周四）。",
            "预算为3000元。",
        ] {
            let draft =
                serde_json::json!({"sections":[{"title":"会议", "body":body, "sources":[1]}]});
            assert!(validate(&draft.to_string(), &source, "m").is_err());
        }
        assert_eq!(
            normalize_date_numbers("二千零二十四年十二月二十三日"),
            "2024年12月23日"
        );
        assert_eq!(
            normalize_date_numbers("预算两千元，一百个问题，十四月，四十日，一二月"),
            "预算两千元，一百个问题，十四月，四十日，一二月"
        );
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
    #[test]
    fn speaker_metadata_is_citable_without_changing_the_original_quote() {
        let mut segments = data();
        segments[0].speaker = Some("a".into());
        let names = std::collections::BTreeMap::from([
            ("a".into(), "Speaker 1".into()),
            ("b".into(), "Speaker 2".into()),
        ]);
        let draft = r#"{"sections":[{"title":"预算","body":"Speaker 1 提到预算尚未确定。","sources":[1]}]}"#;
        let result = validate_with_speakers(draft, &segments, "m", &names).unwrap();
        assert_eq!(result[0].sources[0].quote, segments[0].text);
        assert_eq!(
            result[0].sources[0].speaker_name.as_deref(),
            Some("Speaker 1")
        );
        assert!(validate_with_speakers(
            &draft.replace("Speaker 1", "Speaker 2"),
            &segments,
            "m",
            &names
        )
        .is_err());
    }
    #[test]
    fn rejects_merging_other_voices_under_one_speaker_name() {
        let mut segments = data();
        segments[0].text = "我们播放测试音频。".into();
        segments[0].speaker = Some("a".into());
        let mut other = segments[0].clone();
        other.id = "s2".into();
        other.text = "我在阳台上吹风。".into();
        other.speaker = Some("b".into());
        segments.push(other);
        let names = std::collections::BTreeMap::from([
            ("a".into(), "Speaker 1".into()),
            ("b".into(), "Speaker 2".into()),
        ]);
        let check = |body: &str, segments: &[Segment]| {
            let draft =
                serde_json::json!({"sections":[{"title":"测试音频","body":body,"sources":[1,2]}]});
            validate_with_speakers(&draft.to_string(), segments, "m", &names)
        };
        assert!(check("Speaker 1 播放测试音频，随后提到在阳台上吹风。", &segments).is_err());
        assert!(check(
            "Speaker 1 播放测试音频，Speaker 2 提到在阳台上吹风。",
            &segments
        )
        .is_ok());
        assert!(check("播放了测试音频，其中提到在阳台上吹风。", &segments).is_ok());
        segments[1].speaker = None;
        assert!(check("Speaker 1 播放测试音频，随后提到在阳台上吹风。", &segments).is_err());
        assert!(check("播放了测试音频，其中提到在阳台上吹风。", &segments).is_ok());
    }
}
