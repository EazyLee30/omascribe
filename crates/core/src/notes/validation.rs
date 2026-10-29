//! Deterministic guardrails, not a proof of semantic entailment. All generated prose retains citations.
use crate::{Segment, Source, Suggestion};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::HashSet;

/// Targeted guards for meeting-wide assertions that short excerpts do not prove.
/// These are not a general semantic-entailment classifier.
fn unsupported_summary_claim(body: &str, evidence: &str) -> Option<&'static str> {
    static RULES: std::sync::OnceLock<Vec<(&str, regex::Regex)>> = std::sync::OnceLock::new();
    let rules=RULES.get_or_init(||[
        ("会议结束结论",r"(?i)\bmeeting\s+(?:(?:has|had)\s+)?(?:ended|concluded|finished)\b|会议(?:已经|已)结束|会议结束了|已经散会"),
        ("未采取行动",r"(?i)\bno\s+(?:further\s+)?actions?\s+(?:(?:has|have|had|was|were)\s+)?(?:been\s+)?taken\b|(?:没有|未)采取(?:任何|后续|进一步)*行动"),
        ("不需要后续行动",r"(?i)\bno\s+(?:further\s+)?actions?\s+(?:is|are)\s+(?:needed|required)\b|(?:无需|不需要)(?:采取)?(?:任何|后续|进一步)*行动"),
        ("未讨论其他议题",r"(?i)(?:did\s+not|didn['’]t)\s+(?:discuss|cover|address)\s+(?:any\s+)?other\s+topics|\bno\s+other\s+topics\s+(?:were|have\s+been|had\s+been)\s+(?:discussed|covered|addressed)|(?:没有|未)讨论(?:任何)?其他(?:议题|话题)"),
        ("期限缺乏信息的推断",r"(?i)no\s+(?:indication|mention|evidence)\s+of\s+(?:a\s+|the\s+)?deadline|没有(?:提及|提供)(?:审批)?期限"),
        ("不同场次的会议",r"(?i)\b(?:first|second|third)\s+(?:meeting|session)\b|第[一二三](?:次|场)会议"),
        ("额外的公开测试阶段",r"(?i)公测|公測|公开测试|公開測試|\b(?:public|open)\s+(?:beta|testing)\b"),
        ("额外的测试版本阶段",r"(?i)测试版|測試版|\bbeta\s+(?:release|version|build|testing)\b|\bbeta\s*版"),
        ("未发生修改或更正",r"(?i)\bno\s+(?:changes?|corrections?)\s+(?:(?:or|and)\s+(?:changes?|corrections?)\s+)?(?:were|was|have\s+been|had\s+been)\s+(?:made|applied)\b|(?:没有|未)(?:进行|做出|作出)?(?:任何)?(?:修改|更改|更正)"),
    ].into_iter().map(|(name,pattern)|(name,regex::Regex::new(pattern).unwrap())).collect());
    let assertions = |text: &str| {
        text.split_inclusive(['.', '。', '?', '？', '!', '！', ';', '；', '\n'])
            .filter(|sentence| {
                let sentence = sentence.trim().to_lowercase();
                !sentence.ends_with(['?', '？'])
                    && ![
                        "if ",
                        "when ",
                        "once ",
                        "unless ",
                        "after ",
                        "如果",
                        "假如",
                        "待会议",
                        "等会议",
                    ]
                    .iter()
                    .any(|prefix| sentence.starts_with(prefix))
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let body = assertions(body);
    let evidence = assertions(evidence);
    rules
        .iter()
        .find(|(_, rule)| rule.is_match(&body) && !rule.is_match(&evidence))
        .map(|(name, _)| *name)
}

fn inverted_fragmentation_request(body: &str, evidence: &str) -> bool {
    let evidence = evidence.to_lowercase();
    let complaint = evidence.split(['。', '\n']).any(|s| {
        ["纪要", "摘要", "minutes", "summary"]
            .iter()
            .any(|w| s.contains(w))
            && [
                "一小句一小句",
                "一句一句",
                "逐句",
                "分句",
                "sentence by sentence",
                "one sentence at a time",
                "fragmented",
            ]
            .iter()
            .any(|w| s.contains(w))
            && [
                "不连贯",
                "太碎",
                "混乱",
                "难读",
                "disjointed",
                "hard to read",
                "incoherent",
            ]
            .iter()
            .any(|w| s.contains(w))
    });
    static REQUEST: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let request=REQUEST.get_or_init(||regex::Regex::new(r"(?i)(?:需要|仍需|需|应该|应|必须|建议).{0,8}(?:分句|逐句|一小句一小句|一句一句).{0,8}(?:显示|呈现)|\b(?:needs?|should|must)\b.{0,15}(?:split(?:ting)?|one sentence at a time|sentence by sentence)").unwrap());
    let asks = |text: &str| {
        request.find_iter(text).any(|m| {
            !text[..m.start()]
                .trim_end()
                .ends_with(['不', '无', '勿', '别'])
                && !text[..m.start()].trim_end().to_lowercase().ends_with("not")
                && !m.as_str().to_lowercase().contains("not ")
                && !m.as_str().contains("n't")
        })
    };
    complaint && asks(body) && !asks(&evidence)
}

fn causal_language(text: &str) -> bool {
    let text = text.to_lowercase();
    [
        "因为", "由于", "导致", "造成", "引起", "引发", "因此", "所以", "使得", "源于", "缘于",
    ]
    .iter()
    .any(|word| text.contains(word))
        || regex::Regex::new(r"\b(because|due to|caused?|resulted in|therefore|as a result)\b")
            .unwrap()
            .is_match(&text)
}

/// Preserve explicit recording qualifiers across translation. Negated/changed
/// modes abstain rather than forcing an obsolete qualifier into the article.
fn missing_technical_qualifier(body: &str, evidence: &str) -> Option<&'static str> {
    let source = evidence.to_lowercase();
    let body = body.to_lowercase();
    let recording = [
        "转写",
        "转录",
        "转錄",
        "录音",
        "錄音",
        "音频",
        "音訊",
        "transcription",
        "recorder",
        "recording",
        "audio",
    ]
    .iter()
    .any(|w| body.contains(w));
    if !recording {
        return None;
    }
    static NEGATED: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let negated=NEGATED.get_or_init(||regex::Regex::new(r"(?i)\b(?:not|never|without|no|instead of)\b[^.!?;。\n]{0,30}\boffline\b|\boffline\b[^.!?;。\n]{0,25}\b(?:now|moved|changed|switched)\b[^.!?;。\n]{0,15}\bonline\b").unwrap());
    if source.contains("offline")
        && ["transcription", "recorder", "recording"]
            .iter()
            .any(|w| source.contains(w))
        && !negated.is_match(&source)
        && !["改为在线", "转为在线"].iter().any(|w| source.contains(w))
        && !["离线", "離線", "offline"].iter().any(|w| body.contains(w))
    {
        return Some("offline / 离线");
    }
    let system = ["系统音频", "系統音訊", "system audio"];
    if system.iter().any(|w| source.contains(w))
        && ![
            "not system audio",
            "不是系统音频",
            "改为麦克风",
            "switched to microphone",
        ]
        .iter()
        .any(|w| source.contains(w))
        && ![
            "系统音频",
            "系統音訊",
            "system audio",
            "desktop audio",
            "loopback audio",
            "系统声音",
        ]
        .iter()
        .any(|w| body.contains(w))
    {
        return Some("system audio / 系统音频");
    }
    None
}

/// Replacing an unlocked paragraph must not silently discard its still-selected
/// evidence. This checks citation coverage, not semantic entailment of every word.
pub fn preserve_replaced_sources(
    previous: &[Suggestion],
    selected: &[Segment],
    next: &[Suggestion],
) -> Result<()> {
    let cited: HashSet<_> = next
        .iter()
        .flat_map(|s| &s.sources)
        .map(|s| s.segment_id.as_str())
        .collect();
    let mut missing = Vec::new();
    for section in previous.iter().filter(|s| {
        s.status == "pending"
            && !s.sources.is_empty()
            && s.sources
                .iter()
                .all(|q| selected.iter().any(|s| s.id == q.segment_id))
    }) {
        for source in &section.sources {
            if let Some((index, segment)) = selected
                .iter()
                .enumerate()
                .find(|(_, s)| s.id == source.segment_id)
            {
                if !segment.text.trim().is_empty()
                    && !super::writer::non_substantive(&segment.text)
                    && !cited.contains(segment.id.as_str())
                    && !missing.contains(&(index + 1))
                {
                    missing.push(index + 1);
                }
            }
        }
    }
    if !missing.is_empty() {
        bail!("纪要遗漏历史来源 {missing:?}，本轮未提交");
    }
    Ok(())
}
#[cfg(test)]
mod conditional_role_replay_tests {
    use super::*;

    fn original_sources() -> Vec<Segment> {
        let cases: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/document-cases.json"
        )))
        .unwrap();
        cases
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == "zh_recorded_mixed_prose_fidelity")
            .unwrap()["transcript"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, text)| Segment {
                id: format!("m:1:{}", i + 1),
                meeting_id: "m".into(),
                run: 1,
                seq: i as u32 + 1,
                start_ms: i as u64 * 10_000,
                end_ms: (i as u64 + 1) * 10_000,
                text: text.as_str().unwrap().into(),
                revision: 1,
                speaker: None,
                speaker_manual: false,
            })
            .collect()
    }

    fn draft(body: &str, sources: &[usize]) -> String {
        serde_json::json!({"sections":[{
            "title":"会议纪要", "body":body, "sources":sources
        }]})
        .to_string()
    }

    // These are the actual two committed raw prose responses from the rejected
    // 8da0767 trial, not model output manufactured to match the new guard.
    #[test]
    fn captured_article_cannot_promote_a_conditional_participant_into_a_speaker() {
        let sources = original_sources();
        for (body, cited, selected) in [
            (
                "当迪大学的 PAMELA FURGO 森教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性时，记者表示对此有疑问。随后，有人指出即使主舞台的音乐可能已经结束，嘉年华的某些部分仍可能继续演奏音乐直到深夜。邓迪大学的潘 MALE FERGUSON 教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性。",
                &[4, 2, 1, 3, 5][..],
                &sources[..5],
            ),
            (
                "当迪大学的 PAMELA FURGO 森教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性时，记者表示将根据情况决定是否公布。同时，有人指出即使主舞台的音乐可能已经结束，嘉年华的某些部分仍可能继续演奏音乐直到深夜。大学的潘 MALE FERGUSON 教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性。",
                &[2, 4, 6, 1, 3, 5][..],
                &sources[..],
            ),
        ] {
            let error = validate(&draft(body, cited), selected, "m").unwrap_err();
            assert!(
                error
                    .downcast_ref::<super::super::attribution_roles::Mismatch>()
                    .is_some(),
                "unexpected rejection: {error:#}"
            );
        }
    }

    #[test]
    fn captured_sources_can_still_be_summarized_without_inventing_a_response() {
        let sources = original_sources();
        let body = "当迪大学的 PAMELA FURGO森教授与邓迪大学的潘 MALE FERGUSON教授均指出，记者如果公布嫌疑人的照片等信息，似乎确实存在危害性。即使主舞台的音乐可能已经结束，嘉年华的某些部分仍可能继续演奏音乐直到深夜。";
        let sections = validate(&draft(body, &[1, 2, 3, 4, 5, 6]), &sources, "m")
            .expect("the original conditional risk claim is still valid");
        assert_eq!(sections[0].sources.len(), 6);
        for quote in &sections[0].sources {
            let source = sources.iter().find(|s| s.id == quote.segment_id).unwrap();
            assert_eq!(
                (quote.revision, &quote.quote),
                (source.revision, &source.text)
            );
        }
    }

    #[test]
    fn a_bound_speaker_can_report_their_complete_source_but_cannot_invent_a_response() {
        let mut source = original_sources().remove(1);
        source.speaker = Some("bound-voice".into());
        source.speaker_manual = true;
        let names = [("bound-voice".to_owned(), "记者".to_owned())]
            .into_iter()
            .collect();
        let faithful = format!("记者表示，{}。", source.text);
        validate_with_speakers(
            &draft(&faithful, &[1]),
            std::slice::from_ref(&source),
            "m",
            &names,
        )
        .expect("manual voice identity supports attribution of its complete original speech");
        for body in ["记者表示对此有疑问。", "记者表示将根据情况决定是否公布。"]
        {
            assert!(validate_with_speakers(
                &draft(body, &[1]),
                std::slice::from_ref(&source),
                "m",
                &names,
            )
            .is_err());
        }
    }

    #[test]
    fn presentation_items_keep_other_actors_outside_a_conditional_participants_clause() {
        let mut sources = original_sources();
        sources.truncate(2);
        sources[0].text = "审计员指出承包商如果上传日志可能泄露信息".into();
        sources[1].text = "工程师表示缓存需要复核".into();
        let raw = "1. 审计员指出承包商如果上传日志可能泄露信息\n2. 工程师表示缓存需要复核";
        let body = super::super::writer::body_text(raw, "{}").unwrap();
        assert_eq!(
            body,
            "审计员指出承包商如果上传日志可能泄露信息。 工程师表示缓存需要复核。"
        );
        validate(&draft(&body, &[1, 2]), &sources, "m")
            .expect("presentation normalization must preserve independent statements");
    }

    fn prepared_completion() -> (
        Vec<Segment>,
        crate::terminology::Terms,
        String,
        super::super::source_language::CompletionBridge,
    ) {
        let mut sources = original_sources();
        sources.truncate(5);
        let terms = crate::terminology::Terms::new(&[], &[]);
        let input = serde_json::json!({"transcript":sources.iter().enumerate()
            .map(|(i, source)|serde_json::json!({"id":i+1,"text":source.text}))
            .collect::<Vec<_>>()})
        .to_string();
        let ended = "记住即使主舞台的音乐可能已经结束，嘉年华的某些部分仍可能继续演奏音乐直到深夜";
        let completed =
            "记住即使主舞台的音乐可能已经完成，嘉年华的某些部分仍可能继续演奏音乐直到深夜";
        let translated = serde_json::json!({"A":ended,"B":completed,"C":ended}).to_string();
        let prepared = super::super::source_language::Plan::new(&input, &sources, true, &terms)
            .unwrap()
            .unwrap()
            .prepare(&translated, &sources, &terms)
            .unwrap();
        let applied = prepared.apply(&input, &sources).unwrap();
        let raw = format!(
            "1. {ended}\n2. {}\n3. {completed}\n4. {}\n5. {ended}",
            sources[1].text, sources[3].text
        );
        let composed = super::super::writer::body_with_evidence(&raw, &applied).unwrap();
        assert_eq!(composed.copies().len(), 5);
        let bridge = prepared
            .completion_bridge(&composed, &input, &sources, &terms, true)
            .unwrap()
            .expect("the actual translated completion has source-bound copy evidence");
        (sources, terms, composed.text().to_owned(), bridge)
    }

    #[test]
    fn a_checked_copy_can_transport_its_uncertain_completion_without_rewriting_originals() {
        let (sources, terms, body, bridge) = prepared_completion();
        let document =
            super::super::writer::document(&body, &sources, &Default::default()).unwrap();
        assert!(validate(&document, &sources, "m")
            .unwrap_err()
            .to_string()
            .contains("计划写成了已完成"));
        let checked = validate_with_bridge(
            &document,
            &sources,
            "m",
            &Default::default(),
            &terms,
            true,
            &bridge,
        )
        .unwrap();
        assert_eq!(checked[0].text, body);
        for quote in &checked[0].sources {
            let source = sources.iter().find(|s| s.id == quote.segment_id).unwrap();
            assert_eq!(
                (quote.revision, &quote.quote),
                (source.revision, &source.text)
            );
        }
    }

    #[test]
    fn completion_transport_requires_each_actual_citation_and_unchanged_scope() {
        let (sources, terms, body, bridge) = prepared_completion();
        // Other music sources describe a similar event but cannot lend source3's permit.
        assert!(validate_with_bridge(
            &draft(&body, &[1, 2, 4, 5]),
            &sources,
            "m",
            &Default::default(),
            &terms,
            true,
            &bridge,
        )
        .is_err());
        for changed in [
            format!("{body} 审核已经完成。"),
            format!("不是{body}"),
            format!("如果{body}"),
            format!("“{body}”"),
        ] {
            assert!(
                validate_with_bridge(
                    &draft(&changed, &[1, 2, 3, 4, 5]),
                    &sources,
                    "m",
                    &Default::default(),
                    &terms,
                    true,
                    &bridge,
                )
                .is_err(),
                "{changed}"
            );
        }
        let document = draft(&body, &[1, 2, 3, 4, 5]);
        let mut changed = sources.clone();
        changed[2].run += 1;
        assert!(validate_with_bridge(
            &document,
            &changed,
            "m",
            &Default::default(),
            &terms,
            true,
            &bridge,
        )
        .is_err());
        assert!(validate_with_bridge(
            &document,
            &sources,
            "m",
            &Default::default(),
            &terms,
            false,
            &bridge,
        )
        .is_err());
    }
}

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
pub(super) fn chinese_number(text: &str) -> Option<u32> {
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
    normalize_digit_words(&normalize_clock_numbers(&normalize_date_numbers(
        &super::amounts::normalize(s),
    )))
    .split(|c: char| !c.is_ascii_digit() && c != '.')
    .filter(|s| s.chars().any(|c| c.is_ascii_digit()))
    .map(|s| s.trim_matches('.').to_owned())
    .collect()
}
/// A known model series and its complete digit sequence form one identity.
/// Reuse the terminology boundaries so dates, quantities, suffixes and partial
/// identifiers cannot acquire a new model reading in the number guard.
fn numeric_model_has_continuation(text: &str) -> bool {
    static MEMORY: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    // A separated, explicit capacity is another numeric attribute, not more
    // model digits. An unlabelled trailing number keeps the strict boundary.
    let capacity = MEMORY
        .get_or_init(|| {
            regex::Regex::new(
                r"(?i)^[ \t]+(?:-[ \t]+)?[0-9]+(?:\.[0-9]+)?[ \t]*(?:gib|mib|gb|mb)(?:$|[^a-z0-9_])",
            )
            .unwrap()
        })
        .is_match(text);
    // A second separator begins an opaque suffix/range. A clearly separated
    // explicit capacity above remains a different attribute. Keep this local
    // to numeric evidence, not term replacement.
    (text.trim_start_matches([' ', '\t']).starts_with('-')
        || crate::terminology::model_has_continuation(text))
        && !capacity
}
fn model_number_spellings(text: &str) -> Vec<(std::ops::Range<usize>, String)> {
    static MODEL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    MODEL
        .get_or_init(|| {
            regex::Regex::new(
                r"(?i)([a-z]{2,8})[ \t]*(?:-[ \t]*)?([0-9]{2,8}|[零〇一二三四五六七八九]{2,8})",
            )
            .unwrap()
        })
        .captures_iter(text)
        .filter_map(|parts| {
            let whole = parts.get(0)?;
            if text[..whole.start()]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
                || numeric_model_has_continuation(&text[whole.end()..])
            {
                return None;
            }
            let prefix = parts.get(1)?.as_str();
            let digits: String = parts
                .get(2)?
                .as_str()
                .chars()
                .map(|c| match c {
                    '零' | '〇' => '0',
                    '一' => '1',
                    '二' => '2',
                    '三' => '3',
                    '四' => '4',
                    '五' => '5',
                    '六' => '6',
                    '七' => '7',
                    '八' => '8',
                    '九' => '9',
                    c => c,
                })
                .collect();
            let canonical = format!("{}{digits}", prefix.to_ascii_lowercase());
            crate::terminology::model_identifier(&canonical)?;
            Some((whole.range(), canonical))
        })
        .collect()
}
#[derive(Default)]
struct NumericEvidence {
    ordinary: HashSet<String>,
    models: HashSet<String>,
}
fn numeric_evidence<'a>(texts: impl IntoIterator<Item = &'a str>) -> NumericEvidence {
    let mut evidence = NumericEvidence::default();
    for text in texts {
        let mut ordinary = String::new();
        let mut cursor = 0;
        for (range, canonical) in model_number_spellings(text) {
            ordinary.push_str(&text[cursor..range.start]);
            // A model number cannot support an unrelated count, date or invoice.
            // A separator also prevents joining the text on either side.
            ordinary.push(' ');
            cursor = range.end;
            evidence.models.insert(canonical);
        }
        ordinary.push_str(&text[cursor..]);
        evidence.ordinary.extend(numbers(&ordinary));
    }
    evidence
}

fn mask_numeric_speaker_names(text: &str, sources: &[Source]) -> String {
    let mut names: Vec<_> = sources
        .iter()
        .filter_map(|source| source.speaker_name.as_deref())
        .filter(|name| {
            name.chars().any(|c| c.is_ascii_digit())
                && name.chars().any(char::is_alphabetic)
                // A hardware model used as a label cannot establish that device.
                && model_number_spellings(name).is_empty()
        })
        .collect();
    names.sort_by_key(|name| std::cmp::Reverse(name.len()));
    names.dedup();
    let mut result = text.to_owned();
    for name in names {
        let mut masked = String::new();
        let mut cursor = 0;
        for (start, value) in result.match_indices(name) {
            let end = start + value.len();
            if result[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
                || result[end..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                continue;
            }
            masked.push_str(&result[cursor..start]);
            masked.push(' ');
            cursor = end;
        }
        masked.push_str(&result[cursor..]);
        result = masked;
    }
    result
}

fn opaque_model_suffix(text: &str) -> &str {
    let text = text.trim_start_matches([' ', '\t']);
    let ascii = text.chars().next().is_some_and(|c| {
        c.is_ascii_alphanumeric() || ['_', '.', '．', '%', '％', '-'].contains(&c)
    });
    let mut number_or_identifier_bytes = 0;
    let mut after_hyphen = false;
    for (offset, c) in text.char_indices() {
        let identifier_char = c.is_numeric()
            || if ascii {
                c.is_ascii_alphanumeric() || ['_', '.', '．', '%', '％', '-'].contains(&c)
            } else {
                "零〇一二三四五六七八九十百千万亿两幺点".contains(c)
            };
        let hyphen_space = ascii
            && [' ', '\t'].contains(&c)
            && (after_hyphen
                || text[offset..]
                    .trim_start_matches([' ', '\t'])
                    .starts_with('-'));
        if !identifier_char && !hyphen_space {
            break;
        }
        number_or_identifier_bytes = offset + c.len_utf8();
        if !hyphen_space {
            after_hyphen = c == '-';
        }
    }
    let unit_bytes = crate::terminology::MODEL_CONTINUATION_UNITS
        .iter()
        .find(|unit| text[number_or_identifier_bytes..].starts_with(**unit))
        .map_or(0, |unit| unit.len());
    &text[..number_or_identifier_bytes + unit_bytes]
}

/// Only inspect apparent ASCII identifiers carrying digits from an explicit
/// source model. Unknown or suffixed forms must be quoted literally; they must
/// not disappear from the model guard and borrow an unrelated invoice value.
fn unsupported_model_literal(body: &str, quotes: &[&str], models: &HashSet<String>) -> bool {
    static IDENTIFIER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let digits: HashSet<_> = models
        .iter()
        .filter_map(|model| crate::terminology::model_identifier(model).map(|(_, n)| n))
        .collect();
    if digits.is_empty() {
        return false;
    }
    let identifiers = |text: &str| {
        IDENTIFIER
            // Detection deliberately includes malformed separators. Only the
            // narrower model_number_spellings grammar can establish evidence.
            .get_or_init(|| regex::Regex::new(r"(?i)([a-z]{2,8})([ \t-]*)([0-9]{2,8})").unwrap())
            .captures_iter(text)
            .filter_map(|parts| {
                let whole = parts.get(0)?;
                let prefix = parts.get(1)?.as_str();
                let separator = parts.get(2)?.as_str();
                let number = parts.get(3)?.as_str();
                let canonical = format!("{}{number}", prefix.to_ascii_lowercase());
                let known_series = crate::terminology::model_identifier(&canonical).is_some();
                if (!digits.contains(number) && !known_series)
                    || text[..whole.start()]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
                    // Currency notation remains under the existing amount guard.
                    || ["rmb", "cny", "usd", "eur", "gbp", "jpy", "hkd", "cad", "aud", "chf", "inr", "krw"]
                        .iter()
                        .any(|code| prefix.eq_ignore_ascii_case(code))
                {
                    return None;
                }
                let continuation = &text[whole.end()..];
                let extended = numeric_model_has_continuation(continuation);
                let suffix = if extended {
                    opaque_model_suffix(continuation)
                } else {
                    ""
                };
                let separators: String = separator.chars().filter(|c| *c == '-').collect();
                let valid_separator = separators.len() <= 1;
                let known = !extended && valid_separator && known_series;
                let literal = if known || separators.is_empty() {
                    canonical
                } else {
                    format!("{}{separators}{number}", prefix.to_ascii_lowercase())
                };
                Some((format!("{literal}{}", suffix.to_ascii_lowercase()), known))
            })
            .collect::<Vec<_>>()
    };
    let quoted: HashSet<_> = quotes
        .iter()
        .flat_map(|quote| identifiers(quote))
        .map(|(literal, _)| literal)
        .collect();
    identifiers(body)
        .iter()
        .any(|(literal, known)| !known && !quoted.contains(literal))
}

/// Use compact display spelling only for complete, validated known models also
/// present in this source. This changes separator typography, never model digits
/// or source text. Protected source/note names and opaque identifiers stay literal.
pub(crate) fn compact_source_model_separators<'a>(
    text: &'a str,
    source: &str,
    protected: &[String],
) -> std::borrow::Cow<'a, str> {
    if !text.contains('-') {
        return std::borrow::Cow::Borrowed(text);
    }
    let source_models = numeric_evidence([source]).models;
    let mut result = String::new();
    let mut cursor = 0;
    for (range, canonical) in model_number_spellings(text) {
        let spelling = &text[range.clone()];
        if !spelling.contains('-')
            || !source_models.contains(&canonical)
            || protected.iter().any(|name| {
                name.as_bytes()
                    .windows(spelling.len())
                    .any(|part| part.eq_ignore_ascii_case(spelling.as_bytes()))
            })
        {
            continue;
        }
        result.push_str(&text[cursor..range.start]);
        result.extend(spelling.chars().filter(|c| !['-', ' ', '\t'].contains(c)));
        cursor = range.end;
    }
    if cursor == 0 {
        return std::borrow::Cow::Borrowed(text);
    }
    result.push_str(&text[cursor..]);
    std::borrow::Cow::Owned(result)
}
fn normalize_digit_words(text: &str) -> String {
    // Literal digit-by-digit readings (e.g. a mic check or a spoken ID) are
    // equivalent to the same Arabic digits. Do not reinterpret approximate
    // counts, ordinary Han words, dates or magnitude-bearing amounts.
    static DIGITS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    DIGITS
        .get_or_init(|| regex::Regex::new(r"[零〇一二三四五六七八九]{2,8}").unwrap())
        .replace_all(text, |c: &regex::Captures<'_>| {
            let m = c.get(0).unwrap();
            if text[..m.start()]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric)
                || text[m.end()..]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphanumeric)
            {
                return m.as_str().to_owned();
            }
            m.as_str()
                .chars()
                .map(|c| match c {
                    '零' | '〇' => '0',
                    '一' => '1',
                    '二' => '2',
                    '三' => '3',
                    '四' => '4',
                    '五' => '5',
                    '六' => '6',
                    '七' => '7',
                    '八' => '8',
                    '九' => '9',
                    _ => unreachable!(),
                })
                .collect()
        })
        .into_owned()
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
/// Normalize only for polarity classification; original text/citations stay intact.
pub(super) fn polarity_text(s: &str) -> String {
    s.to_lowercase()
        .replace("n't", " not")
        .replace("n’t", " not")
}
pub(super) fn uncertain(s: &str) -> bool {
    static NEGATIVE_STATE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let s = polarity_text(s);
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
        "undetermined",
        "pending",
        "unconfirmed",
        "not approved",
        "not been approved",
        "unapproved",
        "not yet",
        "deferred",
        "not final",
    ]
    .iter()
    .any(|q| s.contains(q))
        || NEGATIVE_STATE.get_or_init(|| regex::Regex::new(
            r"\bnot\s+(?:(?:been|yet|officially|formally|fully)\s+){0,4}(?:approved|decided|determined|confirmed|finalized)\b"
        ).unwrap()).is_match(&s)
}

/// Follow an explicit budget topic only across a local pronoun/time clause.
/// An unrelated report or release must not inherit its pending review state.
fn budget_clauses(text: &str) -> Vec<String> {
    static BUDGET: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let budget = BUDGET.get_or_init(|| regex::Regex::new(r"\bbudget\b|预算").unwrap());
    let mut active = false;
    text.to_lowercase()
        .split([',', '，', ';', '；', '.', '。', '?', '？', '!', '！'])
        .filter_map(|clause| {
            let clause = clause.trim();
            let inherited = active
                && [
                    "it ", "this ", "we ", "next ", "tomorrow", "下周", "本周", "明天", "将", "再",
                ]
                .iter()
                .any(|prefix| clause.starts_with(prefix));
            active = budget.is_match(clause) || inherited;
            active.then(|| clause.to_owned())
        })
        .collect()
}

fn unsupported_budget_commitment(body: &str, evidence: &str) -> bool {
    static REVIEW: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static DECISION: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let review = REVIEW.get_or_init(|| {
        regex::Regex::new(r"再看|继续讨论|再议|\b(?:review|revisit|discuss)\b").unwrap()
    });
    let decision = DECISION.get_or_init(|| regex::Regex::new(
        r"\bwill\s+(?:be\s+)?(?:decide[ds]?|determine[ds]?|finali[sz]e[ds]?|approve[ds]?)\b|(?:将|会|下周|本周|明天).{0,12}(?:确定|定下|敲定|批准|决定|定案)"
    ).unwrap());
    let source = budget_clauses(evidence);
    let promises = |clauses: &[String]| {
        clauses.iter().any(|clause| {
            decision.is_match(clause)
                && ![
                    "not ",
                    "是否",
                    "可能",
                    "也许",
                    "不确定",
                    "不会",
                    "未必",
                    "if ",
                    "whether ",
                ]
                .iter()
                .any(|cue| clause.contains(cue))
        })
    };
    source.iter().any(|clause| uncertain(clause))
        && source.iter().any(|clause| review.is_match(clause))
        && !promises(&source)
        && promises(&budget_clauses(body))
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
pub(super) fn normalized_time(text: &str) -> String {
    let mut text = text
        .to_lowercase()
        .replace("这周", "本周")
        .replace("这个星期", "本周")
        .replace("本星期", "本周");
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
        text = text.replace(&format!("this {english}"), &format!("本周{chinese}"));
        text = text.replace(english, &format!("周{chinese}"));
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
    validate_with_terms(
        content,
        segments,
        meeting,
        names,
        &crate::terminology::Terms::new(&[], &[]),
    )
}
pub(super) fn validate_with_terms(
    content: &str,
    segments: &[Segment],
    meeting: &str,
    names: &std::collections::BTreeMap<String, String>,
    terms: &crate::terminology::Terms,
) -> Result<Vec<Suggestion>> {
    validate_document(content, segments, meeting, names, 1200, true, terms)
}

pub(super) fn validate_with_bridge(
    content: &str,
    segments: &[Segment],
    meeting: &str,
    names: &std::collections::BTreeMap<String, String>,
    terms: &crate::terminology::Terms,
    chinese: bool,
    bridge: &super::source_language::CompletionBridge,
) -> Result<Vec<Suggestion>> {
    bridge.check_scope(segments, terms, chinese)?;
    validate_document_inner(
        content,
        segments,
        meeting,
        names,
        terms,
        ValidationOptions {
            max_body_chars: 1200,
            require_prose: true,
            bridge: Some(bridge),
        },
    )
}
#[cfg(test)]
pub(crate) fn validate_translation(text: &str, source: &Segment, meeting: &str) -> Result<()> {
    validate_translation_with_terms(
        text,
        source,
        meeting,
        &crate::terminology::Terms::new(&[], &[]),
    )
}
pub(crate) fn validate_translation_with_terms(
    text: &str,
    source: &Segment,
    meeting: &str,
    terms: &crate::terminology::Terms,
) -> Result<()> {
    validate_unapproved(&source.text, text)?;
    let content =
        serde_json::json!({"sections":[{"title":"Translation","body":text,"sources":[1]}]})
            .to_string();
    validate_document(
        &content,
        std::slice::from_ref(source),
        meeting,
        &Default::default(),
        16_000,
        false,
        terms,
    )?;
    Ok(())
}
fn validate_unapproved(source: &str, text: &str) -> Result<()> {
    static UNAPPROVED: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let unapproved=UNAPPROVED.get_or_init(||regex::Regex::new(concat!(
        r"\bunapproved\b|\bnot\s+(?:(?:been|yet|officially|formally|fully)\s+){0,4}approved\b",
        r"|(?:未|没|待)[^。；;,.，!?！？]{0,6}(?:批准|审批|获批|过审|核准)",
        r"|(?:预算|申请|方案|提案|议案)[^。；;,.，!?！？]{0,12}(?:未|没|待)[^。；;,.，!?！？]{0,6}通过",
        // A subjectless continuation retains the explicit approval topic.
        // A new noun (e.g. “但测试未通过”) must not inherit that state.
        r"|(?:预算|申请|方案|提案|议案)[^。；;,.，!?！？]{0,40}[,，]\s*(?:但|不过|而)?\s*(?:仍|还|依然|目前|暂时|尚)?(?:未|没)(?:被|获|能)?通过"
    )).unwrap());
    if unapproved.is_match(&polarity_text(source)) && !unapproved.is_match(&polarity_text(text)) {
        bail!("译文遗漏原话中的未批准状态，请保留否定而非只翻译后续事项");
    }
    Ok(())
}
pub(super) fn completion_name_mask(
    text: &str,
    source: &str,
    terms: &crate::terminology::Terms,
) -> String {
    let mut mask = crate::translation::source_name_mask(text, source, terms).into_bytes();
    // Keep erased names opaque. Removing a literal such as `not` or `if` must
    // not turn a negated or conditional statement into a closed assertion.
    for (original, masked) in text.bytes().zip(&mut mask) {
        if original != *masked {
            *masked = b'#';
        }
    }
    String::from_utf8(mask).expect("masked name spans contain ASCII only")
}

#[cfg(test)]
mod hedged_translation_completion_tests {
    use super::*;

    const ACTUAL_SOURCE: &str = "REMEMBER THAT EVEN THOUGH MUSIC ON THE MAIN STAGES MAY HAVE FINISH THERE MAY BE SECTIONS OF THE FESTIFUL THAT WILL KEEP PLAYING MUSIC UNTIL LATE INTO THE NIGHT";
    const ACTUAL_TARGET: &str =
        "记住即使主舞台的音乐可能已经完成，嘉年华的某些部分仍可能继续演奏音乐直到深夜";

    fn source(text: &str) -> Segment {
        Segment {
            id: "hedged-source".into(),
            meeting_id: "m".into(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 1000,
            text: text.into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        }
    }

    #[test]
    fn actual_hedged_asr_finish_translation_keeps_its_uncertain_past() -> Result<()> {
        crate::translation::validate_text(
            ACTUAL_TARGET,
            &source(ACTUAL_SOURCE),
            "zh",
            &crate::terminology::Terms::new(&[], &[]),
        )
    }

    #[test]
    fn generic_modal_past_completion_does_not_ban_unrelated_future_work() -> Result<()> {
        for predicate in [
            "MAY HAVE FINISHED",
            "MIGHT HAVE COMPLETED",
            "MIGHT HAVE BEEN COMPLETED",
        ] {
            crate::translation::validate_text(
                "任务或许已经完成，播放仍将继续。",
                &source(&format!("THE TASK {predicate}. PLAYBACK WILL CONTINUE.")),
                "zh",
                &crate::terminology::Terms::new(&[], &[]),
            )?;
        }
        Ok(())
    }

    #[test]
    fn translation_hedge_cannot_waive_other_states_or_nonassertive_evidence() {
        let valid = "THE TASK MAY HAVE FINISHED.";
        let target = "任务可能已经完成。";
        for (original, body) in [
            (
                ACTUAL_SOURCE,
                "记住即使主舞台的音乐已经完成，嘉年华仍可能继续演奏音乐直到深夜。",
            ),
            ("THE TASK WILL BE COMPLETED.", target),
            ("THE TASK MAY FINISH.", target),
            ("THE TASK MAY NOT HAVE FINISHED.", target),
            ("NOT THE TASK MAY HAVE FINISHED.", target),
            (
                "IF THE TASK MAY HAVE FINISHED, PLAYBACK WILL CONTINUE.",
                target,
            ),
            ("\"THE TASK MAY HAVE FINISHED.\"", target),
            ("THE TASK MAY HAVE FINISHED?", target),
            (valid, "如果任务可能已经完成，就继续播放。"),
            (valid, "“任务可能已经完成。”"),
            (valid, "任务不可能已经完成。"),
            (valid, "任务未必可能已经完成。"),
            (valid, "任务可能已经完成吗"),
            (valid, "任务可能已经完成，审核已经完成。"),
            (
                "THE TASK MAY HAVE FINISHED. THE AUDIT WAS COMPLETED.",
                target,
            ),
            ("THE TASK MAY HAVE FINISHED. THE AUDIT WILL FINISH.", target),
            (valid, "任务可能已经完成，已告知团队。"),
            (valid, "任务可能已经完成，已提供材料。"),
        ] {
            assert!(
                crate::translation::validate_text(
                    body,
                    &source(original),
                    "zh",
                    &crate::terminology::Terms::new(&[], &[]),
                )
                .is_err(),
                "{original} -> {body}"
            );
        }
    }

    #[test]
    fn conditional_or_second_completion_cannot_borrow_a_modal_state() {
        let valid = "THE TASK MAY HAVE FINISHED.";
        let target = "任务可能已经完成。";
        let accepted: Vec<_> = [
            (
                "WHEN THE TASK MAY HAVE FINISHED, PLAYBACK WILL CONTINUE.",
                target,
            ),
            (
                "ONCE THE TASK MAY HAVE FINISHED, PLAYBACK WILL CONTINUE.",
                target,
            ),
            (valid, "一旦任务可能已经完成，就继续播放。"),
            (valid, "只要任务可能已经完成，就继续播放。"),
            (valid, "只有任务可能已经完成，才继续播放。"),
            (valid, "任务可能已经完成，审核已经结束。"),
            ("THE TASK MAY HAVE FINISHED. THE AUDIT HAS ENDED.", target),
        ]
        .into_iter()
        .filter(|(original, body)| {
            crate::translation::validate_text(
                body,
                &source(original),
                "zh",
                &crate::terminology::Terms::new(&[], &[]),
            )
            .is_ok()
        })
        .collect();
        assert!(accepted.is_empty(), "{accepted:?}");
    }

    #[test]
    fn protected_modal_or_predicate_names_cannot_authorize_a_completion() {
        for literal in ["may", "completed", "if"] {
            let terms = crate::terminology::Terms::new(
                &[],
                &[crate::library::Entry {
                    kind: "term".into(),
                    title: literal.into(),
                    enabled: true,
                    ..Default::default()
                }],
            );
            let original = if literal == "if" {
                "IF THE TASK MAY HAVE COMPLETED."
            } else {
                "THE TASK MAY HAVE COMPLETED."
            };
            assert!(completion_name_mask(original, original, &terms).contains('#'));
            assert!(validate_translation_with_terms(
                "任务可能已经完成。",
                &source(original),
                "m",
                &terms
            )
            .is_err());
        }
    }

    #[test]
    fn modal_translation_exception_does_not_extend_to_prose_or_multiple_sources() {
        let first = source("THE TASK MAY HAVE FINISHED.");
        let draft = |body: &str, indices: &[usize]| {
            serde_json::json!({
                "sections":[{"title":"状态","body":body,"sources":indices}]
            })
            .to_string()
        };
        assert!(validate(
            &draft("任务可能已经完成。", &[1]),
            std::slice::from_ref(&first),
            "m"
        )
        .is_err());
        let mut second = source("THE AUDIT WILL FINISH.");
        second.id = "other-source".into();
        second.seq = 2;
        assert!(validate_document(
            &draft("审核可能已经完成。", &[1, 2]),
            &[first, second],
            "m",
            &Default::default(),
            16_000,
            false,
            &crate::terminology::Terms::new(&[], &[]),
        )
        .is_err());
    }
}

fn validate_document(
    content: &str,
    segments: &[Segment],
    meeting: &str,
    names: &std::collections::BTreeMap<String, String>,
    max_body_chars: usize,
    require_prose: bool,
    terms: &crate::terminology::Terms,
) -> Result<Vec<Suggestion>> {
    validate_document_inner(
        content,
        segments,
        meeting,
        names,
        terms,
        ValidationOptions {
            max_body_chars,
            require_prose,
            bridge: None,
        },
    )
}

struct ValidationOptions<'a> {
    max_body_chars: usize,
    require_prose: bool,
    bridge: Option<&'a super::source_language::CompletionBridge>,
}

fn validate_document_inner(
    content: &str,
    segments: &[Segment],
    meeting: &str,
    names: &std::collections::BTreeMap<String, String>,
    terms: &crate::terminology::Terms,
    options: ValidationOptions<'_>,
) -> Result<Vec<Suggestion>> {
    let ValidationOptions {
        max_body_chars,
        require_prose,
        bridge,
    } = options;
    let draft: Draft =
        serde_json::from_str(content).context("文档结构校验失败，待处理内容已保留")?;
    if draft.sections.is_empty()
        && segments
            .iter()
            .any(|s| !super::writer::non_substantive(&s.text))
    {
        bail!("模型未生成可核对的纪要，转写仍待整理");
    }
    let unfinished = super::completion::unfinished;
    let completed = if require_prose {
        super::completion::view(segments)
    } else {
        super::completion::View::default()
    };
    if require_prose {
        completed.validate(
            &draft
                .sections
                .iter()
                .map(|s| s.body.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        )?;
    }
    if segments
        .iter()
        .enumerate()
        .any(|(i, s)| !completed.retired(i) && unfinished(&s.text))
        && !draft.sections.iter().any(|s| {
            unfinished(&s.body)
                || s.body.contains("待完成")
                || s.body.contains("尚待")
                || s.body.to_lowercase().contains("pending")
        })
        // Accept this bounded equivalent only from each unfinished passage's
        // own cited paragraph. It must not discharge a different pending task,
        // borrow another citation, or read a protected literal name as a fact.
        && !segments.iter().enumerate().filter(|(i, s)| {
            !completed.retired(*i) && unfinished(&s.text)
        }).all(|(i, s)| {
            let source_mask = completion_name_mask(&s.text, &s.text, terms);
            draft.sections.iter().any(|section| {
                if !section.sources.contains(&(i + 1)) {
                    return false;
                }
                let cited_text = section.sources.iter().filter_map(|index| {
                    index.checked_sub(1).and_then(|index| segments.get(index))
                }).map(|source| source.text.as_str()).collect::<Vec<_>>().join("\n");
                validate_unapproved(&s.text, &section.body).is_ok()
                    && super::completion::incomplete_transcription_covers(
                    &source_mask,
                    &completion_name_mask(&section.body, &cited_text, terms),
                )
            })
        })
    {
        bail!("纪要遗漏了尚未完成的状态，请明确保留未完成事项");
    }
    if draft.sections.len() > 3 {
        bail!("文档超过本轮段落预算");
    }
    super::metrics::validate(
        &draft
            .sections
            .iter()
            .map(|s| s.body.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        &segments
            .iter()
            .enumerate()
            .filter(|(i, _)| !completed.retired(*i))
            .map(|(_, s)| s.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        !require_prose,
    )?;
    // A source qualifier may live in a different paragraph than its citation.
    // Check the current article once, while per-paragraph source/fact checks
    // below still reject unsupported content and stale source revisions.
    super::qualifiers::validate(
        &draft
            .sections
            .iter()
            .map(|s| s.body.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        &segments
            .iter()
            .enumerate()
            .filter(|(i, _)| !completed.retired(*i))
            .map(|(_, s)| s.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    )?;
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for (index, section) in draft.sections.into_iter().enumerate() {
        let body = section.body.trim();
        let title = section.title.trim();
        if body.is_empty()
            || title.is_empty()
            || body.chars().count() > max_body_chars
            || title.chars().count() > 60
            || section.sources.is_empty()
            || section.sources.len() > segments.len()
        {
            bail!("文档段落或出处无效");
        }
        if require_prose
            && (body
                .lines()
                .any(|l| l.trim_start().starts_with(['#', '*', '-', '•']))
                || title.contains(['\n', '#', '*']))
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
        let current_evidence = segments
            .iter()
            .enumerate()
            .filter(|(i, s)| !completed.retired(*i) && sources.iter().any(|q| q.segment_id == s.id))
            .map(|(_, s)| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        super::relations::validate(body, sources.iter().map(|s| s.quote.as_str()))?;
        let quoted_sources = sources.iter().map(|s| s.quote.as_str()).collect::<Vec<_>>();
        if sources.iter().all(|s| s.speaker_name.is_none()) {
            super::attribution_roles::validate(body, &quoted_sources, terms)?;
        } else {
            super::attribution_roles::validate_with_authors(
                body,
                &quoted_sources,
                &sources
                    .iter()
                    .map(|s| s.speaker_name.as_deref())
                    .collect::<Vec<_>>(),
                terms,
            )?;
        }
        super::recording_retention::validate(body, sources.iter().map(|s| s.quote.as_str()))?;
        super::submission::validate(body, &current_evidence, !require_prose)?;
        if require_prose {
            let raw_evidence = sources
                .iter()
                .map(|s| s.quote.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if let Some(claim) = unsupported_summary_claim(body, &raw_evidence) {
                bail!("纪要含有无依据补写（{claim}），请仅保留原话支持的内容；本轮未提交");
            }
            if inverted_fragmentation_request(body, &raw_evidence) {
                bail!("纪要把问题反馈反向改成了要求，请保留原话的问题状态；本轮未提交");
            }
        }
        if causal_language(body) && !causal_language(&evidence) {
            bail!("纪要增加了原话未明确说明的因果关系，请分别记录事实和疑问");
        }
        let lower = evidence.to_lowercase();
        if let Some(qualifier) = missing_technical_qualifier(body, &evidence) {
            bail!("纪要遗漏了原文技术限定（{qualifier}），请翻译并保留限定词；本轮未提交");
        }
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
        let numeric_title = mask_numeric_speaker_names(title, &sources);
        let numeric_body = mask_numeric_speaker_names(body, &sources);
        let generated_numbers = numeric_evidence([numeric_title.as_str(), numeric_body.as_str()]);
        let quotes: Vec<_> = sources.iter().map(|source| source.quote.as_str()).collect();
        // Do not construct an identifier across citations or from a speaker label.
        let source_numbers = numeric_evidence(quotes.iter().copied());
        if !generated_numbers
            .ordinary
            .is_subset(&source_numbers.ordinary)
        {
            bail!(
                "第 {} 段的数字或日期不在其引用的原文中，请核对出处编号；本轮未提交",
                index + 1
            );
        }
        if !generated_numbers.models.is_subset(&source_numbers.models)
            || unsupported_model_literal(&numeric_title, &quotes, &source_numbers.models)
            || unsupported_model_literal(&numeric_body, &quotes, &source_numbers.models)
        {
            bail!(
                "第 {} 段的完整型号不在其引用的原文中，请保留型号前缀和全部数字；本轮未提交",
                index + 1
            );
        }
        if let Some(currency) = super::amounts::added_currency(body, &evidence) {
            bail!("文档增加了原话未给出的币种（{currency}），请保留未指定的单位；本轮未提交");
        }
        if unsupported_budget_commitment(body, &current_evidence) {
            bail!("文档将预算复议写成了确定或批准的承诺，请保留待讨论状态；本轮未提交");
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
            .any(|word| {
                if !body.contains(word) || evidence.contains(word) {
                    return false;
                }
                let completion = matches!(*word, "已完成" | "已经完成");
                let translated = completion
                    && !require_prose
                    && sources.len() == 1
                    && super::completion::hedged_completion_covers(
                        &completion_name_mask(&sources[0].quote, &sources[0].quote, terms),
                        &completion_name_mask(body, &sources[0].quote, terms),
                    );
                body.match_indices(word).any(|(at, _)| {
                    !(translated
                        || (require_prose
                            && completion
                            && bridge.is_some_and(|bridge| {
                                bridge.permits(body, at..at + word.len(), &sources)
                            })))
                })
            })
        {
            bail!("文档可能将计划写成了已完成事项，本轮未提交");
        }
        let shares_uncertain_topic = [
            &["预算", "budget", "金额"][..],
            &["launch", "release", "发布", "公测", "公測", "beta"][..],
            &["日期"][..],
        ]
        .iter()
        .any(|group| {
            group.iter().any(|t| lower.contains(t))
                && group.iter().any(|t| body.to_lowercase().contains(t))
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
        if shares_uncertain_topic && uncertain(&current_evidence) && !uncertain(body) {
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
mod adjacent_model_number_repro {
    use super::*;

    fn source(text: &str) -> Segment {
        Segment {
            id: "s1".into(),
            meeting_id: "m".into(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 1000,
            text: text.into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        }
    }
    fn check(source_text: &str, body: &str) -> anyhow::Result<Vec<Suggestion>> {
        let draft = serde_json::json!({"sections":[{"title":"设备","body":body,"sources":[1]}]})
            .to_string();
        validate(&draft, &[source(source_text)], "m")
    }

    #[test]
    fn adjacent_model_repro_rejects_a_changed_digit() {
        let error = check("设备代号是 MX二七五零。", "设备代号是 MX2751。")
            .expect_err("a different model number must remain unsupported");
        assert!(error.to_string().contains("完整型号"));
    }
    #[test]
    fn adjacent_model_repro_spaced_and_wrong_digit_controls() {
        assert!(check("设备代号是 MX 二七五零。", "设备代号是 MX2750。").is_ok());
        assert!(check("设备代号是 MX 二七五零。", "设备代号是 MX2751。").is_err());
        assert_eq!(normalize_digit_words("一二月"), "一二月");
        assert_eq!(normalize_digit_words("三千"), "三千");
    }
    #[test]
    fn adjacent_model_repro_should_accept_equal_identifier() {
        assert!(check("设备代号是 MX二七五零。", "设备代号是 MX2750。").is_ok());
    }
    #[test]
    fn adjacent_model_digits_keep_complete_series_and_leading_zeroes() {
        for (spoken, canonical) in [
            ("GTX一六五零", "GTX1650"),
            ("RTX三零六零", "RTX3060"),
            ("GTS四五零", "GTS450"),
            ("GT一零三零", "GT1030"),
            ("MX〇一二三", "MX0123"),
            ("RX六六零零", "RX6600"),
            ("ARC七七零", "ARC770"),
        ] {
            let input = format!("设备代号是 {spoken}。");
            let body = format!("设备代号是 {canonical}。");
            assert!(check(&input, &body).is_ok(), "{input} -> {body}");
            assert!(check(&body, &input).is_ok(), "{body} -> {input}");
            assert!(validate_translation(&body, &source(&input), "m").is_ok());
        }
        for (input, body) in [
            ("设备代号是 MX二七五零。", "设备代号是 RX2750。"),
            ("设备代号是 MX二七五零一。", "设备代号是 MX2750。"),
            ("设备代号是 MX〇一二三。", "设备代号是 MX123。"),
            (
                "设备是 MX二七五零 和 RX三零六零。",
                "设备是 RX2750 和 MX3060。",
            ),
        ] {
            assert!(check(input, body).is_err(), "{input} -> {body}");
            assert!(validate_translation(body, &source(input), "m").is_err());
        }
    }
    #[test]
    fn adjacent_model_reading_does_not_consume_suffixes_dates_or_quantities() {
        for value in [
            "MX二七五零一二三四五",
            "MX二七五零5",
            "MX二七五零.5",
            "MX二七五零点五",
            "MX二七五零台",
            "MX二七五零年",
            "MX二七五零月",
            "MX二七五零万",
            "MX二七五零 Ti",
            "MY_MX二七五零",
            "QX二七五零",
        ] {
            let evidence = numeric_evidence([value]);
            assert!(evidence.models.is_empty(), "{value}");
            assert_eq!(evidence.ordinary, numbers(value), "{value}");
            assert!(
                check(&format!("设备代号是 {value}。"), "设备代号是 MX2750。").is_err(),
                "{value}"
            );
        }
        for value in ["一二月", "三千", "预算一二", "RMB二七五零"] {
            assert_eq!(normalize_digit_words(value), value);
            let evidence = numeric_evidence([value]);
            assert!(evidence.models.is_empty(), "{value}");
            assert_eq!(evidence.ordinary, numbers(value), "{value}");
        }
    }
    #[test]
    fn adjacent_model_digits_cannot_become_an_unknown_or_extended_model() {
        for body in [
            "设备代号是 QX2750。",
            "设备代号是 MX2750 Ti。",
            "设备代号是 MX2750台。",
            "设备代号是 MX2750年。",
        ] {
            assert!(check("设备代号是 MX二七五零。", body).is_err(), "{body}");
        }
        for body in ["设备代号是 QX2750。", "设备代号是 MX2750 Ti。"] {
            assert!(
                check("设备代号是 MX二七五零，发票编号是2750。", body).is_err(),
                "same-valued invoice must not support {body}"
            );
        }
        assert!(check(
            "设备代号是 MX二七五零，另一个代号是 QX2750。",
            "另一个代号是 QX2750。"
        )
        .is_ok());
    }
    #[test]
    fn adjacent_model_digits_cannot_become_an_ordinary_count_or_date() {
        for body in ["设备数量是2750台。", "时间是2750年。"] {
            assert!(check("设备代号是 MX二七五零。", body).is_err(), "{body}");
        }
        assert!(check(
            "设备代号是 MX二七五零。",
            "设备代号是 MX2750，数量是2750台。"
        )
        .is_err());
        assert!(check("发票编号是2750。", "设备代号是 MX2750。").is_err());
        assert!(check("设备是 RX三零六零，发票编号是2750。", "设备代号是 RX2750。").is_err());
    }
    #[test]
    fn adjacent_model_identity_cannot_be_joined_across_cited_segments() {
        let first = source("设备代号是 MX");
        let mut second = source("二七五零。");
        second.id = "s2".into();
        second.seq = 2;
        second.start_ms = 1000;
        second.end_ms = 2000;
        let draft = serde_json::json!({"sections":[{
            "title":"设备","body":"设备代号是 MX2750。","sources":[1,2]
        }]})
        .to_string();
        assert!(validate(&draft, &[first, second], "m").is_err());
    }
    #[test]
    fn adjacent_model_identity_cannot_borrow_a_speaker_label() {
        let mut segment = source("二七五零。");
        segment.speaker = Some("voice".into());
        let draft = serde_json::json!({"sections":[{
            "title":"设备","body":"设备代号是 MX2750。","sources":[1]
        }]})
        .to_string();
        let names = [("voice".into(), "MX".into())].into_iter().collect();
        assert!(validate_with_speakers(&draft, &[segment], "m", &names).is_err());
    }
    #[test]
    fn adjacent_model_guard_preserves_literal_legacy_identifiers_and_english_readings() {
        for literal in ["QX2750", "MX2750 Ti", "GTX ONE SIX FIVE ZERO"] {
            let text = format!("设备代号是 {literal}。");
            assert!(check(&text, &text).is_ok(), "{literal}");
        }
        assert!(check("发票编号是2750。", "发票编号是2750。").is_ok());
    }
    #[test]
    fn adjacent_model_guard_preserves_an_independent_memory_capacity() {
        assert!(check("显卡是 GTX1650 4GB。", "显卡是 GTX1650，显存4GB。").is_ok());
        assert!(check("显卡是 GTX一六五零 4GB。", "显卡是 GTX1650，显存4GB。").is_ok());
        assert!(check("显卡是 GTX一六五零，显存4GB。", "显卡是 GTX1650 4GB。").is_ok());
        assert!(check("显卡是 GTX1650 4GB。", "显卡是 GTX1650，显存8GB。").is_err());
        assert!(check("显卡是 GTX1650 4。", "显卡是 GTX1650。").is_err());
        assert!(check("显卡是 GTX1650 - 4GB。", "显卡是 GTX1650，显存4GB。").is_ok());
    }
    #[test]
    fn adjacent_model_guard_preserves_supported_alphanumeric_speaker_names() {
        let mut segment = source("本周继续测试。");
        segment.speaker = Some("voice".into());
        let draft = serde_json::json!({"sections":[{
            "title":"测试","body":"Eazy2026表示本周继续测试。","sources":[1]
        }]})
        .to_string();
        let names = [("voice".into(), "Eazy2026".into())].into_iter().collect();
        assert!(
            validate_with_speakers(&draft, std::slice::from_ref(&segment), "m", &names).is_ok()
        );
        for body in [
            "Eazy2026表示2026年继续测试。",
            "preEazy2026表示本周继续测试。",
            "Eazy2026X表示本周继续测试。",
        ] {
            let draft = serde_json::json!({"sections":[{
                "title":"测试","body":body,"sources":[1]
            }]})
            .to_string();
            assert!(
                validate_with_speakers(&draft, std::slice::from_ref(&segment), "m", &names)
                    .is_err(),
                "{body}"
            );
        }
        segment.text = "本周继续测试，设备代号是 MX二七五零。".into();
        let draft = serde_json::json!({"sections":[{
            "title":"测试","body":"Eazy2026表示本周继续测试，设备代号是 MX2750。","sources":[1]
        }]})
        .to_string();
        assert!(validate_with_speakers(&draft, &[segment], "m", &names).is_ok());
    }
    #[test]
    fn adjacent_model_guard_does_not_turn_a_model_like_speaker_name_into_hardware_evidence() {
        let mut segment = source("本周继续测试。");
        segment.speaker = Some("voice".into());
        let names = [("voice".into(), "GTX1650".into())].into_iter().collect();
        let draft = serde_json::json!({"sections":[{
            "title":"测试","body":"显卡型号是GTX1650，本周继续测试。","sources":[1]
        }]})
        .to_string();
        assert!(validate_with_speakers(&draft, &[segment], "m", &names).is_err());
    }
    #[test]
    fn adjacent_model_opaque_suffix_stops_before_adjacent_han_prose() {
        for (input, body) in [
            (
                "设备是 RTX三零六零，另一个设备 RTX3060Ti已完成测试。",
                "另一个设备 RTX3060Ti 已完成测试。",
            ),
            (
                "设备是 RTX三零六零，另一个设备 RTX3060Ti 已完成测试。",
                "另一个设备 RTX3060Ti已完成测试。",
            ),
        ] {
            assert!(check(input, body).is_ok(), "{input} -> {body}");
        }
        for body in ["设备代号是 MX2750台。", "设备代号是 MX2750年。"] {
            assert!(
                check(
                    "设备代号是 MX二七五零，也写作 MX2750，发票编号是2750。",
                    body
                )
                .is_err(),
                "a Han unit must not become the plain model literal: {body}"
            );
        }
    }
    #[test]
    fn adjacent_model_single_hyphen_preserves_complete_identity() {
        for spelling in ["MX-2750", "mx - 2750", "MX\t-\t2750", "MX-二七五零"] {
            let spoken = "设备代号是 MX二七五零。";
            let body = format!("设备代号是 {spelling}。");
            assert!(check(spoken, &body).is_ok(), "{spelling}");
            assert!(check(&body, spoken).is_ok(), "{spelling}");
        }
        // A following ASCII hyphen is part of an opaque identifier, not a
        // licence to keep only its prefix or borrow independent invoice digits.
        for model in [
            "MX-2750-1",
            "MX-2750 - 1",
            "MX-2750-Ti",
            "MX-2750 Ti",
            "MX-2750台",
            "MX-2750年",
            "MX-2750--1",
            "MX--2750",
            "MX-27-50",
            "MX-2751",
            "QX-2750",
            "RX-2750",
        ] {
            let input = "设备是 MX二七五零，发票编号是2750、27、50、1。";
            let body = format!("设备代号是 {model}。");
            assert!(check(input, &body).is_err(), "unsupported {model}");
        }
        for input_model in ["MX-2750-1", "MX-27-50", "MX--2750"] {
            let input = format!("设备代号是 {input_model}。");
            assert!(check(&input, "设备代号是 MX2750。").is_err(), "{input}");
        }
        for model in ["QX-2750", "MX-2750-1", "MX-2750-Ti", "MX--2750"] {
            let text = format!("设备是 MX二七五零，另一个代号是 {model}。");
            assert!(check(&text, &text).is_ok(), "literal {model}");
        }
        assert!(check(
            "设备是 MX二七五零，另一个代号是 MX-2750-，发票编号是1。",
            "另一个代号是 MX-2750- 1。"
        )
        .is_err());
        assert!(check("设备是 MX二七五零。", "设备是 MX-2750，数量2750台。").is_err());
        assert!(check("发票编号是2750。", "设备代号是 MX-2750。").is_err());
        for (opaque, changed) in [
            ("QX-2750", "QX2750"),
            ("QX2750", "QX-2750"),
            ("MX-2750Ti", "MX2750Ti"),
        ] {
            let input = format!("设备是 MX二七五零，另一个代号是 {opaque}。");
            let body = format!("另一个代号是 {changed}。");
            assert!(check(&input, &body).is_err(), "{input} -> {body}");
        }
    }
    #[test]
    fn compact_caption_models_are_source_bound_and_do_not_touch_opaque_forms() {
        let input = "设备是 MX二七五零。";
        assert_eq!(
            compact_source_model_separators("Use MX-2750.", input, &[]),
            "Use MX2750."
        );
        assert_eq!(
            compact_source_model_separators("Use MX-2750.", input, &["MX-2750".into()]),
            "Use MX-2750."
        );
        for value in [
            "MX-2751",
            "RX-2750",
            "QX-2750",
            "MX--2750",
            "MX-2750-Ti",
            "MX-2750台",
        ] {
            assert_eq!(compact_source_model_separators(value, input, &[]), value);
        }
        assert_eq!(
            compact_source_model_separators("Use MX-2750.", "发票编号2750。", &[]),
            "Use MX-2750."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn submission_schedule_cannot_be_completed_or_change_its_object() {
        for (input, wrong) in [
            ("实时转写的延迟测试由我负责，结果周四提交。", "The real-time transcription delay test I am responsible for was submitted on Thursday."),
            ("参观预约页面由陈老师负责，下周三发测试链接，页面尚未完成。", "陈老师 is responsible for the tour reservation page, which will be released on Wednesday next week, and the page is still not completed."),
        ] {
            let mut source = data()[0].clone();
            source.text = input.into();
            assert!(validate_translation(wrong, &source, "m").is_err(), "{wrong}");
        }
    }
    #[test]
    fn source_metric_types_cannot_change_or_appear_without_evidence() {
        let mut source = data()[0].clone();
        source.text = "测试报告要包括字错率、词错率和转写延迟，不能只写处理成功。".into();
        let wrong="The test report should include word error rate, sentence error rate, and transcription delay, and should not only mention successful processing.";
        assert!(validate_translation(wrong, &source, "m").is_err());
        assert!(validate_translation("The test report should include character error rate, word error rate and transcription latency, not just successful processing.",&source,"m").is_ok());
        source.text = "测试报告要包括句错率和转写延迟，不能只写处理成功。".into();
        assert!(validate_translation("The test report should include sentence error rate and transcription latency, not just successful processing.",&source,"m").is_ok());
    }
    #[test]
    fn source_qualifier_exclusive_scope_cannot_disappear() {
        let mut source = data()[0].clone();
        source.text = "本阶段只实现麦克风录音与实时转写，系统音频采集推迟到下个版本。".into();
        let body = "This phase implements microphone recording and real-time transcription. System audio capture is postponed to the next version.";
        assert!(validate_translation(body, &source, "m").is_err());
    }
    #[test]
    fn source_qualifier_toolbar_request_cannot_become_an_existing_capability() {
        let mut source = data()[0].clone();
        source.text = "还有实时转写和实时字幕，入口放在工具栏，别藏到二级菜单。".into();
        let body = "Real-time transcription and real-time subtitles, with the entry placed in the toolbar, not hidden in a secondary menu.";
        assert!(validate_translation(body, &source, "m").is_err());
    }
    #[test]
    fn report_relations_reject_the_retained_candidate6_semantic_failures() {
        for (input, body) in [
            (
                "Morgan will send the release report on Thursday. We have not approved the launch date yet.",
                "Morgan 将于周四发布释放报告。我们尚未批准启动日期。",
            ),
            (
                "Morgan owns the latency report and will deliver it on Thursday.",
                "Morgan 拥有延迟报告，并将于周四交付。",
            ),
        ] {
            let mut source = data()[0].clone();
            source.text = input.into();
            assert!(validate_translation(body, &source, "m").is_err(), "caption: {body}");
            let draft = serde_json::json!({"sections":[{"title":"报告","body":body,"sources":[1]}]});
            assert!(validate(&draft.to_string(), &[source], "m").is_err(), "minutes: {body}");
        }
    }
    #[test]
    fn literal_spoken_digits_support_the_same_number_without_supporting_a_changed_count() {
        let mut source = data()[0].clone();
        source.text = "一二三，测试麦克风。谢谢，再见。".into();
        assert!(
            validate_translation("123, test microphone. Thank you, goodbye.", &source, "m").is_ok()
        );
        for body in [
            "321, test microphone.",
            "124, test microphone.",
            "3000, test microphone.",
            "123 yuan, test microphone.",
        ] {
            assert!(validate_translation(body, &source, "m").is_err(), "{body}");
        }
        for text in [
            "一二月",
            "一二人",
            "两三",
            "三千",
            "预算一二",
            "一二三四五六七八九",
        ] {
            assert_eq!(normalize_digit_words(text), text);
        }
        assert_eq!(normalize_digit_words("编号：〇一二三。"), "编号：0123。");
    }
    #[test]
    fn caption_approval_negation_cannot_disappear_when_the_subject_is_a_pronoun() {
        let mut source = data()[0].clone();
        source.text = "IT HAS NOT BEEN APPROVED ALICE WILL CHECK THE UN VIDEO CARD".into();
        assert!(validate_translation("ALICE将检查UN VIDEO CARD。", &source, "m").is_err());
        assert!(validate_translation("尚未批准，ALICE将检查UN VIDEO CARD。", &source, "m").is_ok());
        source.text = "It has now been approved. Alice will check the model.".into();
        assert!(validate_translation("已获批准，Alice将检查模型。", &source, "m").is_ok());
        source.text = "测试尚未通过。".into();
        assert!(validate_translation("The test has not passed yet.", &source, "m").is_ok());
    }
    #[test]
    fn approval_continuations_do_not_borrow_an_unrelated_failed_test() {
        let mut source = data()[0].clone();
        source.text="The budget amount is corrected to 5000, still not approved. Morgan keeps the Thursday deadline.".into();
        for body in [
            "预算金额已修正为5000，但仍未通过。Morgan仍保持周四的截止日期。",
            "预算金额改为5000，尚未通过。Morgan保留周四期限。",
        ] {
            assert!(validate_translation(body, &source, "m").is_ok(), "{body}");
        }
        for body in [
            "预算金额改为5000，但测试未通过。Morgan保留周四期限。",
            "预算金额改为5000，但仍已通过。Morgan保留周四期限。",
            "预算金额改为5000，报告未通过。Morgan保留周四期限。",
        ] {
            assert!(validate_translation(body, &source, "m").is_err(), "{body}");
        }
    }
    #[test]
    fn budget_review_is_not_a_promised_decision_even_with_a_pending_adjective() {
        let mut source = data()[0].clone();
        source.text = "张老师周五给测试结果，预算先不定，下周再看。".into();
        for body in [
            "Mr. Zhang will provide the test results on Friday. The budget remains undetermined and will be reviewed next week.",
            "The budget is undetermined; we will revisit it next week.",
            "The budget was not determined yet; it will be reviewed next week.",
            "预算仍未确定，下周继续讨论。",
        ] {
            assert!(validate_translation(body, &source, "m").is_ok(), "{body}");
        }
        for body in [
            "Mr. Zhang will provide the test results on Friday, but the budget is still undetermined; it will be determined next week.",
            "The budget remains undecided; we will approve it next week.",
            "预算尚未确定，下周会敲定。",
        ] {
            let error = validate_translation(body, &source, "m").unwrap_err();
            assert!(error.to_string().contains("复议"), "{body}: {error}");
            let draft = serde_json::json!({"sections":[{"title":"Budget","body":body,"sources":[1]}]});
            assert!(validate(&draft.to_string(), std::slice::from_ref(&source), "m").is_err());
        }
        // An independent report and an explicit future decision remain valid.
        source.text = "The budget is undecided; we will review it next week. The report will be finalized next week.".into();
        assert!(validate_translation(&source.text, &source, "m").is_ok());
        source.text =
            "The budget is undecided; we will review it and will decide next week.".into();
        assert!(validate_translation(&source.text, &source, "m").is_ok());
        assert!(!uncertain("The budget is determined and approved."));
    }
    #[test]
    fn bare_amounts_do_not_support_an_invented_currency_or_different_number() {
        let mut source = data()[0].clone();
        source.text = "The Budget of 3000 is not approved.".into();
        assert!(validate_translation("3000的预算尚未批准。", &source, "m").is_ok());
        for body in [
            "2000年预算未通过。",
            "3000美元的预算未通过批准。",
            "预算3000元尚未批准。",
        ] {
            assert!(validate_translation(body, &source, "m").is_err(), "{body}");
        }
        for (input, body) in [
            (
                "The budget of 3000 dollars is not approved.",
                "3000美元的预算未批准。",
            ),
            (
                "预算3000元未批准。",
                "The budget of 3000 yuan is not approved.",
            ),
            (
                "预算三千美元未批准。",
                "The budget of 3000 dollars is unapproved.",
            ),
        ] {
            source.text = input.into();
            assert!(
                validate_translation(body, &source, "m").is_ok(),
                "{input} -> {body}"
            );
        }
    }
    #[test]
    fn spoken_money_does_not_allow_a_tenfold_translation_error() {
        let mut source = data();
        source[0].text = "THE BUDGET IS THREE THOUSAND DOLLARS AND IT HAS NOT BEEN APPROVED".into();
        assert!(validate_translation("预算三千美元尚未批准。", &source[0], "m").is_ok());
        for body in ["预算三万美元尚未批准。", "预算30000美元尚未批准。"] {
            assert!(validate_translation(body, &source[0], "m").is_err());
            let draft =
                serde_json::json!({"sections":[{"title":"Budget","body":body,"sources":[1]}]})
                    .to_string();
            assert!(validate(&draft, &source, "m").is_err());
        }
    }
    #[test]
    fn approval_negation_accepts_bounded_adverbs_without_accepting_affirmation() {
        let mut source = data();
        source[0].text = "The budget is provisionally 3000 yuan and has not been approved.".into();
        for body in [
            "The budget is provisionally set at 3000 yuan and has not been officially approved.",
            "The budget has not yet been formally approved; it is provisionally 3000 yuan.",
            "The budget hasn't been fully approved; it is provisionally 3000 yuan.",
        ] {
            let draft =
                serde_json::json!({"sections":[{"title":"Budget","body":body,"sources":[1]}]})
                    .to_string();
            assert!(validate(&draft, &source, "m").is_ok(), "{body}");
            assert!(validate_translation(body, &source[0], "m").is_ok());
        }
        for body in [
            "The budget has been officially approved at 3000 yuan.",
            "The budget has not only been officially approved but increased to 3000 yuan.",
            "The budget is 3000 yuan. This is not a question; it has been approved.",
        ] {
            let draft =
                serde_json::json!({"sections":[{"title":"Budget","body":body,"sources":[1]}]})
                    .to_string();
            assert!(validate(&draft, &source, "m").is_err(), "{body}");
        }
    }
    #[test]
    fn a_public_release_does_not_establish_a_public_beta_stage() {
        let source="The offline recorder is ready for internal testing. The public release is not approved yet.";
        assert!(unsupported_summary_claim("公测发布尚未批准。", source).is_some());
        assert!(unsupported_summary_claim("公开发布尚未批准。", source).is_none());
        assert!(unsupported_summary_claim(
            "公测发布尚未批准。",
            "The public beta release is not approved yet."
        )
        .is_none());
        assert!(unsupported_summary_claim(
            "公测发布尚未批准。",
            "The private beta release is not approved yet."
        )
        .is_some());
    }
    #[test]
    fn recording_qualifiers_survive_translation_but_negated_or_changed_modes_are_not_forced() {
        let source =
            "The offline transcription prototype is ready for review. 本阶段只实现麦克风录音。";
        assert!(missing_technical_qualifier("转写原型已可评审。", source).is_some());
        assert!(missing_technical_qualifier("离线转写原型已可评审。", source).is_none());
        assert!(missing_technical_qualifier(
            "Online transcription is used.",
            "We are not using offline transcription."
        )
        .is_none());
        assert!(missing_technical_qualifier(
            "Online transcription is used.",
            "Offline transcription was switched to online transcription."
        )
        .is_none());
        assert!(missing_technical_qualifier(
            "The system has not implemented audio acquisition.",
            "系统音频采集还没有实现。"
        )
        .is_some());
        assert!(missing_technical_qualifier(
            "System audio capture is not implemented.",
            "系统音频采集还没有实现。"
        )
        .is_none());
        assert!(missing_technical_qualifier(
            "Desktop audio capture is not implemented.",
            "系统音频采集还没有实现。"
        )
        .is_none());
    }
    #[test]
    fn a_complaint_about_fragmented_minutes_is_not_a_request_to_continue_it() {
        let source = "AI纪要还是一小句一小句地显示，阅读起来不连贯。";
        assert!(inverted_fragmentation_request(
            "AI纪要显示不连贯，需分句显示。",
            source
        ));
        assert!(inverted_fragmentation_request(
            "AI纪要仍需一小句一小句显示。",
            source
        ));
        assert!(inverted_fragmentation_request(
            "The minutes should display one sentence at a time.",
            source
        ));
        assert!(!inverted_fragmentation_request(
            "AI纪要仍逐句显示，阅读不连贯。",
            source
        ));
        assert!(!inverted_fragmentation_request(
            "AI纪要不应逐句显示。",
            source
        ));
        assert!(!inverted_fragmentation_request(
            "The minutes should not display one sentence at a time.",
            source
        ));
        assert!(!inverted_fragmentation_request(
            "这部分需要分句显示。",
            &format!("{source}这部分需要分句显示。")
        ));
    }
    #[test]
    fn incomplete_transcriptions_accept_the_actual_draft_in_minutes_and_captions() {
        let mut source = data()[0].clone();
        source.text = "The transcriptions are incomplete. The report is not approved.".into();
        let body = "语音转录不完整，报告未批准。";
        let draft =
            serde_json::json!({"sections":[{"title":"会议纪要","body":body,"sources":[1]}]})
                .to_string();
        let sections = validate(&draft, std::slice::from_ref(&source), "m").unwrap();
        assert_eq!(sections[0].text, body);
        assert_eq!(sections[0].sources[0].quote, source.text);
        assert_eq!(sections[0].sources[0].revision, source.revision);
        assert!(validate_translation(body, &source, "m").is_ok());
        for wrong in [
            "报告未批准。",
            "语音转录完整，报告未批准。",
            "报告不完整且未批准。",
            "如果语音转录不完整，报告未批准。",
            "语音转录不完整吗？报告未批准。",
            "“语音转录不完整”，报告未批准。",
            "语音转录不完整是一个示例，报告未批准。",
            "语音转录不完整，报告已获批准。",
        ] {
            let draft =
                serde_json::json!({"sections":[{"title":"会议纪要","body":wrong,"sources":[1]}]})
                    .to_string();
            assert!(
                validate(&draft, std::slice::from_ref(&source), "m").is_err(),
                "minutes: {wrong}"
            );
            assert!(
                validate_translation(wrong, &source, "m").is_err(),
                "caption: {wrong}"
            );
        }
    }
    #[test]
    fn incomplete_fallback_cannot_discharge_other_work_or_borrow_another_citation() {
        let mut segments = vec![data()[0].clone(); 2];
        for (i, (s, text)) in segments
            .iter_mut()
            .zip([
                "The transcriptions are incomplete.",
                "The invoice review is not completed.",
            ])
            .enumerate()
        {
            s.id = format!("s{i}");
            s.seq = i as u32 + 1;
            s.text = text.into();
        }
        for citations in [vec![1], vec![2], vec![1, 2]] {
            let draft = serde_json::json!({"sections":[{"title":"状态","body":"语音转录不完整。","sources":citations}]}).to_string();
            assert!(validate(&draft, &segments, "m").is_err(), "{draft}");
        }
        segments[1].text = "The report is not approved.".into();
        let draft = serde_json::json!({"sections":[
            {"title":"报告","body":"语音转录不完整，报告未批准。","sources":[2]},
            {"title":"转录","body":"这是转录内容。","sources":[1]}
        ]})
        .to_string();
        assert!(validate(&draft, &segments, "m").is_err());
        segments[0].text = "The invoice review is incomplete.".into();
        assert!(validate_translation("语音转录不完整。", &segments[0], "m").is_err());
        segments[0].text =
            "The transcriptions are incomplete. The invoice review is not completed.".into();
        assert!(validate_translation("语音转录不完整。", &segments[0], "m").is_err());
    }
    #[test]
    fn incomplete_fallback_does_not_read_protected_literal_names_as_status() {
        let terms = crate::terminology::Terms::new(
            &[],
            &[crate::library::Entry {
                kind: "term".into(),
                title: "语音转录不完整".into(),
                enabled: true,
                ..Default::default()
            }],
        );
        let mut source = data()[0].clone();
        source.text = "The transcriptions are incomplete. The product is 语音转录不完整.".into();
        let body = "语音转录不完整。";
        let draft = serde_json::json!({"sections":[{"title":"状态","body":body,"sources":[1]}]})
            .to_string();
        assert!(validate_with_terms(
            &draft,
            std::slice::from_ref(&source),
            "m",
            &Default::default(),
            &terms
        )
        .is_err());
        assert!(validate_translation_with_terms(body, &source, "m", &terms).is_err());
        let mut other = source.clone();
        source.text = "The transcriptions are incomplete.".into();
        other.id = "other".into();
        other.seq += 1;
        other.text = "The product is 语音转录不完整.".into();
        let draft = serde_json::json!({"sections":[{"title":"状态","body":body,"sources":[1,2]}]})
            .to_string();
        assert!(
            validate_with_terms(&draft, &[source, other], "m", &Default::default(), &terms)
                .is_err()
        );
        let terms = crate::terminology::Terms::new(
            &[],
            &[crate::library::Entry {
                kind: "term".into(),
                title: "not".into(),
                enabled: true,
                ..Default::default()
            }],
        );
        let mut source = data()[0].clone();
        source.text = "The transcriptions are not incomplete. The report is not approved.".into();
        let body = "语音转录不完整，报告未批准。";
        let draft = serde_json::json!({"sections":[{"title":"状态","body":body,"sources":[1]}]})
            .to_string();
        assert!(validate_with_terms(
            &draft,
            std::slice::from_ref(&source),
            "m",
            &Default::default(),
            &terms
        )
        .is_err());
        assert!(validate_translation_with_terms(body, &source, "m", &terms).is_err());
    }
    #[test]
    fn bare_unfinished_assertions_keep_their_status() {
        let mut source = data()[0].clone();
        source.text =
            "今天只讨论这些验收要求，没有完成系统音频采集，也没有确定新版本发布日期。".into();
        let actual = "Today, only these acceptance requirements will be discussed, without completing the system audio capture, and without determining the release date of the new version.";
        assert!(validate_translation(actual, &source, "m").is_err());
        assert!(validate_translation(
            "Today we are only discussing these acceptance requirements. System audio capture has not yet been completed, and the new version release date is still undecided.",
            &source,
            "m"
        )
        .is_ok());
    }
    #[test]
    fn conditional_bare_completion_keeps_valid_without_translation() {
        let mut source = data()[0].clone();
        for (original, translated) in [
            (
                "没有完成系统音频采集就不能发布。",
                "Do not release without completing system audio capture.",
            ),
            (
                "如果预算不批，没有完成系统音频采集就不要发布。",
                "If the budget is not approved, do not release without completing system audio capture.",
            ),
            (
                "没有完成系统音频采集前，先不要公开发布。",
                "Do not publicly release before completing system audio capture.",
            ),
        ] {
            source.text = original.into();
            assert!(
                validate_translation(translated, &source, "m").is_ok(),
                "{original}"
            );
        }
    }
    #[test]
    fn actual_completion_updates_only_that_task_and_does_not_weaken_translations() {
        let mut segments = vec![data()[0].clone(); 3];
        for (i, (s, text)) in segments.iter_mut().zip([
            "The upload verification is not completed. Morgan will run it on Thursday.",
            "The invoice review has not been completed. Riley will run it on Friday.",
            "Morgan confirms the upload verification has now been completed successfully on Thursday. There are no remaining upload tasks."
        ]).enumerate() {
            s.id = format!("s{i}"); s.seq = i as u32 + 1; s.text = text.into();
        }
        let draft = |body: &str| {
            serde_json::json!({"sections":[{"title":"Tasks","body":body,"sources":[1,2,3]}]})
                .to_string()
        };
        let body="Morgan confirms the upload verification has now been completed on Thursday. The invoice review has not been completed; Riley will run it on Friday.";
        let valid = validate(&draft(body), &segments, "m").unwrap();
        assert_eq!(valid[0].sources[0].quote, segments[0].text);
        assert!(validate(
            &draft("The upload verification has been completed on Thursday."),
            &segments,
            "m"
        )
        .is_err());
        assert!(validate(
            &draft(
                "The upload verification is not completed. The invoice review is not completed."
            ),
            &segments,
            "m"
        )
        .is_err());
        assert!(validate_translation(
            "The upload verification has been completed.",
            &segments[0],
            "m"
        )
        .is_err());
        segments.remove(1);
        let draft=serde_json::json!({"sections":[{"title":"Tasks","body":"Morgan confirms the upload verification has now been completed on Thursday.","sources":[1,2]}]}).to_string();
        assert!(validate(&draft, &segments, "m").is_ok());
    }
    #[test]
    fn excerpt_does_not_prove_meeting_wide_absence_or_closure() {
        let mut source = data()[0].clone();
        let fact = "The budget of 3000 is not approved.";
        for claim in [
            "The meeting concluded.",
            "In the first meeting, the budget was discussed.",
            "No further action has been taken.",
            "No further action is required.",
            "There is no indication of a deadline for approval.",
            "The meeting did not discuss any other topics.",
            "No changes or corrections were made to the original data.",
        ] {
            let body = format!("{fact} {claim}");
            let draft =
                serde_json::json!({"sections":[{"title":"Budget","body":body,"sources":[1]}]})
                    .to_string();
            source.text = fact.into();
            assert!(
                validate(&draft, std::slice::from_ref(&source), "m").is_err(),
                "{claim}"
            );
            source.text = body;
            assert!(
                validate(&draft, std::slice::from_ref(&source), "m").is_ok(),
                "{claim}"
            );
        }
        assert!(
            unsupported_summary_claim("The meeting has ended.", "Has the meeting ended?").is_some()
        );
        assert!(unsupported_summary_claim(
            "The meeting has ended.",
            "When the meeting has ended, save the audio."
        )
        .is_some());
        assert!(
            unsupported_summary_claim("No changes were made.", "No changes should be made.")
                .is_some()
        );
        assert!(unsupported_summary_claim(
            "If the meeting has ended, save the audio.",
            "Save the audio."
        )
        .is_none());
        assert!(unsupported_summary_claim("会议已结束。", "预算尚未批准。").is_some());
        assert!(unsupported_summary_claim("会议已结束。", "会议已经结束。").is_none());
    }
    #[test]
    fn english_approval_negation_is_preserved_and_synonyms_are_accepted() {
        let mut source = data()[0].clone();
        for source_text in [
            "The budget of 3000 is not approved.",
            "The budget of 3000 remains unapproved.",
            "The budget of 3000 has not been approved.",
            "The budget of 3000 hasn't been approved.",
            "The budget of 3000 hasn’t been approved.",
            "The budget of 3000 isn't approved.",
        ] {
            source.text = source_text.into();
            for body in [
                "The budget of 3000 is not approved.",
                "The budget of 3000 remains unapproved.",
                "The budget of 3000 has not been approved.",
                "The budget of 3000 hasn't been approved.",
                "The budget of 3000 isn’t approved.",
            ] {
                let draft =
                    serde_json::json!({"sections":[{"title":"Budget","body":body,"sources":[1]}]});
                assert!(
                    validate(&draft.to_string(), std::slice::from_ref(&source), "m").is_ok(),
                    "{source_text} -> {body}"
                );
            }
            for affirmative in [
                "The budget of 3000 is approved.",
                "The budget of 3000 has been approved.",
                "The budget of 3000 is not only approved but funded.",
            ] {
                let wrong = serde_json::json!({"sections":[{"title":"Budget","body":affirmative,"sources":[1]}]});
                assert!(validate(&wrong.to_string(), std::slice::from_ref(&source), "m").is_err());
            }
        }
    }
    #[test]
    fn translation_lengths_are_separate_from_minutes_but_fact_guards_remain() {
        let mut source = data()[0].clone();
        source.text =
            "The budget 3000 is not decided. Morgan will send the report on Thursday. ".repeat(80);
        let translation = "预算3000尚未确定。Morgan将在周四发送报告。".repeat(80);
        assert!(translation.chars().count() > 1200);
        assert!(validate_translation(&translation, &source, "m").is_ok());
        assert!(validate_translation(&translation.replace("3000", "4000"), &source, "m").is_err());
        assert!(validate_translation(&translation.replace("周四", "周一"), &source, "m").is_err());
        let draft =
            serde_json::json!({"sections":[{"title":"Budget","body":translation,"sources":[1]}]});
        assert!(validate(&draft.to_string(), std::slice::from_ref(&source), "m").is_err());
    }
    #[test]
    fn a_long_paragraph_can_cite_all_ten_passages_including_the_last_deadline() {
        let sources: Vec<_> = (1..=10)
            .map(|seq| {
                let mut s = data()[0].clone();
                s.seq = seq;
                s.id = format!("s{seq}");
                s
            })
            .collect();
        let draft = serde_json::json!({"sections":[{"title":"测试安排","body":"张老师周五提供测试结果，预算尚未确定，下周讨论。","sources":(1..=10).collect::<Vec<_>>()}]});
        assert_eq!(
            validate(&draft.to_string(), &sources, "m").unwrap()[0]
                .sources
                .len(),
            10
        );
        let invalid = draft.to_string().replace("9,10", "9,11");
        assert!(validate(&invalid, &sources, "m").is_err());
    }
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
    fn same_relative_week_is_equivalent_but_a_different_week_is_rejected() {
        let mut passages = data();
        passages[0].text = "字幕这周还不能交付，时间未定。".into();
        for body in [
            "字幕本周不能交付，时间未定。",
            "字幕这个星期不能交付，时间未定。",
        ] {
            let draft =
                serde_json::json!({"sections":[{"title":"交付", "body":body,"sources":[1]}]})
                    .to_string();
            assert!(validate(&draft, &passages, "m").is_ok());
        }
        let other = r#"{"sections":[{"title":"交付","body":"字幕下周不能交付，时间未定。","sources":[1]}]}"#;
        assert!(validate(other, &passages, "m").is_err());
    }
    #[test]
    fn unqualified_weekday_does_not_establish_this_week() {
        let mut source = data().remove(0);
        source.text = "Riley will test keyboard accessibility on Tuesday. No accessibility test has been completed yet.".into();
        assert!(validate_translation(
            "Riley将在周二测试键盘可访问性，尚未完成任何测试。",
            &source,
            "m"
        )
        .is_ok());
        for body in [
            "Riley本周二将测试键盘可访问性，目前尚未完成任何测试。",
            "Riley这个星期二将测试键盘可访问性，目前尚未完成任何测试。",
            "Riley will test keyboard accessibility this Tuesday. No test has been completed yet.",
        ] {
            assert!(validate_translation(body, &source, "m").is_err(), "{body}");
        }
        source.text = "Riley will test keyboard accessibility this Tuesday. No accessibility test has been completed yet.".into();
        assert!(validate_translation(
            "Riley本周二将测试键盘可访问性，目前尚未完成任何测试。",
            &source,
            "m"
        )
        .is_ok());
    }
    #[test]
    fn recording_retention_cannot_be_replaced_by_meeting_notes_alone() {
        let mut source = data().remove(0);
        source.text =
            "FRIDAY IS NOT THE DEADLINE RECORDING IN MEETING NOTES SHOULD STAY ON THIS COMPUTER"
                .into();
        assert!(
            validate_translation("星期五不是截止日期，会议记录应留在此电脑上。", &source, "m")
                .is_err()
        );
        assert!(validate_translation(
            "星期五不是截止日期，会议笔记中的录音应留在此电脑上。",
            &source,
            "m"
        )
        .is_ok());
    }
    #[test]
    fn unstated_causal_explanations_are_rephrased_instead_of_committed() {
        let mut passages = data();
        for body in [
            "预算未定导致测试失败。",
            "The test failed because the budget is undecided.",
        ] {
            let draft =
                serde_json::json!({"sections":[{"title":"测试","body":body,"sources":[1]}]})
                    .to_string();
            assert!(validate(&draft, &passages, "m")
                .unwrap_err()
                .to_string()
                .contains("因果关系"));
        }
        passages[0].text = "网络中断导致下载失败。".into();
        let supported = r#"{"sections":[{"title":"下载","body":"The download failed because the network disconnected.","sources":[1]}]}"#;
        assert!(validate(supported, &passages, "m").is_ok());
    }
    #[test]
    fn empty_model_response_does_not_mark_substantive_transcript_as_reviewed() {
        let mut passages = data();
        assert!(validate(r#"{"sections":[]}"#, &passages, "m")
            .unwrap_err()
            .to_string()
            .contains("转写仍待整理"));
        passages[0].text = "Hello! Mic check. Can you hear me?".into();
        assert!(validate(r#"{"sections":[]}"#, &passages, "m")
            .unwrap()
            .is_empty());
    }
    #[test]
    fn recomposition_cannot_silently_drop_a_previous_topic() {
        let mut passages = data();
        let mut second = passages[0].clone();
        second.id = "s2".into();
        second.seq = 2;
        second.text = "系统音频尚未实现。".into();
        passages.push(second);
        let full = r#"{"sections":[{"title":"测试安排","body":"张老师周五提供测试结果，预算尚未确定，下周讨论。系统音频尚未实现。","sources":[1,2]}]}"#;
        let dropped = r#"{"sections":[{"title":"测试安排","body":"张老师周五提供测试结果，预算尚未确定，下周讨论。","sources":[1]}]}"#;
        let mut previous = validate(full, &passages, "m").unwrap();
        let next = validate(dropped, &passages, "m").unwrap();
        assert!(preserve_replaced_sources(&previous, &passages, &next)
            .unwrap_err()
            .to_string()
            .contains("[2]"));
        assert!(preserve_replaced_sources(&previous, &passages, &previous).is_ok());
        // A protected paragraph remains in the document; it is not being replaced.
        previous[0].status = "accepted".into();
        assert!(preserve_replaced_sources(&previous, &passages, &next).is_ok());
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
