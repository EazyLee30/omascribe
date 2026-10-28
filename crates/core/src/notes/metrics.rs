//! Source-bound error-rate references, not general numeric or semantic entailment.
use anyhow::Result;
use regex::Regex;
use std::sync::OnceLock;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Metric {
    Character,
    Word,
    Sentence,
}
impl Metric {
    fn label(self, chinese: bool) -> &'static str {
        match (self, chinese) {
            (Self::Character, true) => "字错率",
            (Self::Word, true) => "词错率",
            (Self::Sentence, true) => "句错率",
            (Self::Character, false) => "character error rate",
            (Self::Word, false) => "word error rate",
            (Self::Sentence, false) => "sentence error rate",
        }
    }
}
fn patterns() -> &'static [(Metric, Regex)] {
    static VALUES: OnceLock<Vec<(Metric, Regex)>> = OnceLock::new();
    VALUES.get_or_init(|| [
        (Metric::Character,r"(?i)(?-u:\b)(?:character|char)[ -]+error[ -]+rates?(?-u:\b)|(?-u:\b)CER(?-u:\b)|(?:字符|字元|字)(?:错误|错|錯誤|錯)率"),
        (Metric::Word,r"(?i)(?-u:\b)word[ -]+error[ -]+rates?(?-u:\b)|(?-u:\b)WER(?-u:\b)|(?:单词|單詞|词|詞)(?:错误|错|錯誤|錯)率"),
        (Metric::Sentence,r"(?i)(?-u:\b)sentence[ -]+error[ -]+rates?(?-u:\b)|(?-u:\b)SER(?-u:\b)|(?:句子|句)(?:错误|错|錯誤|錯)率"),
    ].into_iter().map(|(m,p)|(m,Regex::new(p).unwrap())).collect())
}
fn mentions(text: &str) -> Vec<Metric> {
    let mut found: Vec<_> = patterns()
        .iter()
        .filter_map(|(m, p)| p.find(text).map(|v| (v.start(), *m)))
        .collect();
    found.sort_by_key(|(i, _)| *i);
    found.into_iter().map(|(_, m)| m).collect()
}
fn request(source: &str) -> Option<regex::Captures<'_>> {
    if ["不再", "撤回", "取消", "改为", "改成"]
        .iter()
        .any(|s| source.contains(s))
    {
        return None;
    }
    static REQUEST: OnceLock<Regex> = OnceLock::new();
    let frame=REQUEST.get_or_init(||Regex::new(r"(?:^|[。；;.!?\n])\s*(?P<subject>测试报告|评估报告|报告)(?P<modal>必须|应该|需要|应|要)(?:包括|包含|列出|记录)(?P<list>[^。！？.!?；;\n]+)").unwrap()).captures(source)?;
    if ["不要", "不需要", "或者", "或", "任选", "二选一", "不是"]
        .iter()
        .any(|s| frame["list"].contains(s))
        || source[frame.get(0)?.end()..].starts_with(['?', '？'])
    {
        return None;
    }
    Some(frame)
}
#[derive(Debug)]
pub(crate) struct Mismatch;
impl Mismatch {
    pub(crate) fn hint(&self) -> &'static str {
        " Preserve each stated error-rate type: 字错率 is character error rate (CER), 词错率 is word error rate (WER), and 句错率 is sentence error rate (SER). Do not substitute one type for another or add an unstated metric. Preserve the report requirement, latency and all other source clauses."
    }
}
impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.hint().trim())
    }
}
impl std::error::Error for Mismatch {}
pub(super) fn validate(body: &str, source: &str, literal: bool) -> Result<()> {
    let source_metrics = mentions(source);
    let output = mentions(body);
    let required = if literal {
        source_metrics.clone()
    } else {
        request(source)
            .map(|c| mentions(&c["list"]))
            .unwrap_or_default()
    };
    if output.iter().any(|m| !source_metrics.contains(m))
        || required.iter().any(|m| !output.contains(m))
    {
        return Err(Mismatch.into());
    }
    Ok(())
}
pub(crate) fn prefix(source: &str, chinese: bool) -> Option<String> {
    let frame = request(source)?;
    // A closed list does not represent later independent facts. Leave a
    // multi-sentence source to the original-evidence repair instead of allowing
    // its constrained first sentence to finish the whole translation.
    if source[frame.get(0)?.end()..]
        .chars()
        .any(char::is_alphanumeric)
    {
        return None;
    }
    // Constrain report-leading Chinese lists without moving earlier facts or
    // duplicating protected acronyms. Other forms still undergo validation.
    if frame.get(0)?.start() != 0 || !source.trim_start().starts_with(&frame["subject"]) {
        return None;
    }
    static SYMBOL: OnceLock<Regex> = OnceLock::new();
    if SYMBOL
        .get_or_init(|| Regex::new(r"(?-u:\b)(?:CER|WER|SER)(?-u:\b)").unwrap())
        .is_match(&frame["list"])
    {
        return None;
    }
    let metrics = mentions(&frame["list"]);
    if metrics.is_empty() {
        return None;
    }
    let mut remainder = frame["list"].to_owned();
    for (_, p) in patterns() {
        remainder = p.replace_all(&remainder, "").into_owned();
    }
    let complete = remainder
        .chars()
        .all(|c| c.is_whitespace() || "、，,和与及以等".contains(c));
    let labels: Vec<_> = metrics.into_iter().map(|m| m.label(chinese)).collect();
    let names = labels.join(if chinese { "、" } else { " and " });
    let modal = &frame["modal"];
    let mut prefix = if chinese {
        format!(
            "{}{}包括{names}",
            &frame["subject"],
            if modal == "必须" {
                "必须"
            } else if modal == "需要" {
                "需要"
            } else {
                "应"
            }
        )
    } else {
        let subject = match &frame["subject"] {
            "测试报告" => "The test report",
            "评估报告" => "The evaluation report",
            _ => "The report",
        };
        format!(
            "{subject} {} include {names}",
            if modal == "必须" {
                "must"
            } else if modal == "需要" {
                "needs to"
            } else {
                "should"
            }
        )
    };
    if complete {
        prefix.push(if chinese { '。' } else { '.' });
    } else {
        prefix.push(' ');
    }
    Some(prefix)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metrics_preserve_concepts_and_do_not_invent_an_unmentioned_rate() {
        let s = "测试报告要包括字错率、词错率和转写延迟，不能只写处理成功。";
        assert!(validate(
            "The report includes CER, WER and transcription latency.",
            s,
            true
        )
        .is_ok());
        assert!(validate("报告包括CER和WER。", "测试报告要包括字错率、词错率。", true).is_ok());
        assert!(validate(
            "The report includes word error rate and sentence error rate.",
            s,
            true
        )
        .is_err());
        assert!(validate("The report includes character error rate.", s, true).is_err());
        assert!(validate("The report includes SER.", "测试报告要包括句错率。", true).is_ok());
        assert!(validate(
            "No further sentence error rate is required.",
            "不再要求句错率。",
            false
        )
        .is_ok());
        assert!(prefix("测试报告要包括词错率或句错率。", false).is_none());
        assert!(prefix("测试报告要包括字错率吗？", false).is_none());
        assert!(prefix("会议尚未结束。测试报告要包括字错率。", false).is_none());
        assert!(prefix("测试报告要包括字错率、词错率。缓存保存在本机。", false).is_none());
    }
    #[test]
    fn a_closed_request_can_finish_without_inventing_more_prose() {
        assert_eq!(
            prefix("测试报告要包括字错率、词错率。", false).as_deref(),
            Some("The test report should include character error rate and word error rate.")
        );
        assert_eq!(
            prefix("报告必须包括句错率。", true).as_deref(),
            Some("报告必须包括句错率。")
        );
        assert!(prefix(
            "测试报告要包括字错率、词错率和转写延迟，不能只写处理成功。",
            false
        )
        .unwrap()
        .ends_with(' '));
    }
}
