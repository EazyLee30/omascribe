//! Task-local spelling rendering for an ordinary writer request. These tokens
//! carry no source, speaker, copy, affiliation, or factual proof permission.
use crate::{terminology::Terms, Segment};
use anyhow::{ensure, Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    sync::OnceLock,
};

const MAX_BYTES: usize = 120_000;
const MAX_NAMES: usize = 12;
const MAX_OCCURRENCES: usize = 64;

#[derive(serde::Serialize)]
struct Origin {
    source_id: String,
    revision: u32,
    source_sha256: String,
    field: &'static str,
    range: Range<usize>,
}
struct Span {
    entry: usize,
    field: &'static str,
    range: Range<usize>,
    literal: String,
    origin: Origin,
}
pub(super) struct Plan {
    names: Vec<String>,
    people: BTreeSet<String>,
    origins: Vec<Origin>,
    input_sha256: String,
}
pub(super) struct Prepared {
    pub system: String,
    pub content: String,
    pub plan: Option<Plan>,
}

fn reserved(text: &str) -> bool {
    static RESERVED: OnceLock<Regex> = OnceLock::new();
    RESERVED
        .get_or_init(|| {
            Regex::new(r"(?i)\[\s*n|(?:^|[^A-Za-z0-9_])n[0-9]+(?:$|[^A-Za-z0-9_])").unwrap()
        })
        .is_match(text)
}
fn needs_person_boundary(ch: Option<char>, separated: bool) -> bool {
    let Some(ch) = ch.filter(|ch| ch.is_alphanumeric()) else {
        return false;
    };
    if !separated {
        return true;
    }
    if ch.is_ascii() {
        return false;
    }
    static HAN: OnceLock<Regex> = OnceLock::new();
    let mut encoded = [0u8; 4];
    HAN.get_or_init(|| Regex::new(r"^\p{Han}$").unwrap())
        .is_match(ch.encode_utf8(&mut encoded))
}
fn boundaries(text: &str, name: &str) -> Vec<Range<usize>> {
    text.match_indices(name)
        .filter_map(|(at, value)| {
            let end = at + value.len();
            let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
            let model = crate::terminology::model_identifier(name).is_some();
            (!(name.starts_with(word) && text[..at].ends_with(word))
                && !(name.ends_with(word) && text[end..].starts_with(word))
                && !(model && crate::terminology::model_has_continuation(&text[end..])))
            .then_some(at..end)
        })
        .collect()
}
fn functional(name: &str) -> bool {
    [
        "if", "may", "might", "no", "not", "will", "would", "should", "could", "must", "and", "or",
        "but", "the", "is", "are", "was", "were", "has", "have", "had", "to", "in", "of", "on",
        "budget", "date", "test", "如果", "可能", "不是", "并非", "预算", "日期", "测试", "录音",
        "转写",
    ]
    .iter()
    .any(|word| name.eq_ignore_ascii_case(word))
}

pub(super) fn prepare(
    system: String,
    content: String,
    sources: &[Segment],
    terms: &Terms,
    speakers: &BTreeMap<String, String>,
    enabled: bool,
    chinese: bool,
) -> Result<Prepared> {
    let mut prepared = Prepared {
        system,
        content,
        plan: None,
    };
    if !enabled
        || reserved(&prepared.system)
        || reserved(&prepared.content)
        || sources
            .iter()
            .try_fold(prepared.content.len(), |n, s| n.checked_add(s.text.len()))
            .is_none_or(|n| n > MAX_BYTES)
    {
        return Ok(prepared);
    }
    let Some(first) = sources.first() else {
        return Ok(prepared);
    };
    ensure!(
        sources
            .iter()
            .all(|s| s.meeting_id == first.meeting_id && s.run == first.run),
        "专有名称代码来源不属于同一会议轮次"
    );
    let mut input: Value = serde_json::from_str(&prepared.content)?;
    let entries = input["transcript"]
        .as_array()
        .context("Missing writer name input")?;
    let mut seen = BTreeSet::new();
    let mut spans = Vec::new();
    let mut observed_people = BTreeSet::new();
    for (entry, passage) in entries.iter().enumerate() {
        let id = passage["id"]
            .as_u64()
            .and_then(|id| usize::try_from(id).ok())
            .context("Invalid writer name source")?;
        ensure!(seen.insert(id), "重复的名称来源编号");
        let source = id
            .checked_sub(1)
            .and_then(|id| sources.get(id))
            .context("Invalid writer name source")?;
        let actual = passage["text"]
            .as_str()
            .context("Invalid writer name text")?;
        let original_people = match crate::translation::reported_name_occurrences(&source.text) {
            Ok(people) => people,
            Err(_) => return Ok(prepared),
        };
        let people = match crate::translation::reported_name_occurrences(actual) {
            Ok(people) => people,
            Err(_) => return Ok(prepared),
        };
        let source_sha256 = format!("{:x}", Sha256::digest(serde_json::to_vec(source)?));
        let mut accepted: Vec<Range<usize>> = Vec::new();
        let mut add =
            |field: &'static str, range: Range<usize>, literal: &str, original: Range<usize>| {
                if field == "text"
                    && accepted
                        .iter()
                        .any(|old| old.start < range.end && range.start < old.end)
                {
                    return;
                }
                if field == "text" {
                    accepted.push(range.clone());
                }
                spans.push(Span {
                    entry,
                    field,
                    range,
                    literal: literal.into(),
                    origin: Origin {
                        source_id: source.id.clone(),
                        revision: source.revision,
                        source_sha256: source_sha256.clone(),
                        field,
                        range: original,
                    },
                });
            };
        for range in people {
            let literal = &actual[range.clone()];
            if let Some(original) = original_people
                .iter()
                .find(|original| &source.text[(*original).clone()] == literal)
            {
                observed_people.insert(literal.to_owned());
                add("text", range, literal, original.clone());
            }
        }
        for name in terms
            .caption_spellings(&source.text)
            .into_iter()
            .filter(|name| !functional(name))
        {
            let Some(original) = boundaries(&source.text, &name).into_iter().next() else {
                continue;
            };
            for range in boundaries(actual, &name) {
                add("text", range, &name, original.clone());
            }
        }
        if let Some(speaker) = passage.get("speaker").and_then(Value::as_str) {
            ensure!(
                source
                    .speaker
                    .as_ref()
                    .and_then(|id| speakers.get(id))
                    .is_some_and(|name| name == speaker),
                "名称代码不能更改来源说话人"
            );
            if !speaker.is_empty() {
                add("speaker", 0..speaker.len(), speaker, 0..speaker.len());
            }
        }
        if spans.len() > MAX_OCCURRENCES {
            return Ok(prepared);
        }
    }
    let names = spans
        .iter()
        .map(|span| span.literal.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if names.is_empty() || names.len() > MAX_NAMES {
        return Ok(prepared);
    }
    spans.sort_by(|a, b| {
        a.entry
            .cmp(&b.entry)
            .then(a.field.cmp(b.field))
            .then(b.range.start.cmp(&a.range.start))
    });
    for span in &spans {
        let id = names.binary_search(&span.literal).expect("known literal") + 1;
        let text = input["transcript"][span.entry][span.field]
            .as_str()
            .context("Invalid writer name field")?;
        let mut encoded = text.to_owned();
        encoded.replace_range(span.range.clone(), &format!("[N{id}]"));
        input["transcript"][span.entry][span.field] = encoded.into();
    }
    prepared.content = serde_json::to_string(&input)?;
    prepared.system.push_str(if chinese {
        " 文中[N1]这类代码是原名称，使用时逐字保留代码，不拼写或翻译；按原话归属写连贯正文，未提及的名称无需补写。"
    } else {
        " Name references such as [N1] are opaque spellings. Keep used references unchanged; do not expand or translate them. Preserve original attribution; do not add unused names."
    });
    let input_sha256 = format!(
        "{:x}",
        Sha256::digest(format!("{}\n{}", prepared.system, prepared.content))
    );
    prepared.plan = Some(Plan {
        names,
        people: observed_people,
        origins: spans.into_iter().map(|span| span.origin).collect(),
        input_sha256,
    });
    Ok(prepared)
}

impl Plan {
    pub(super) fn metrics(&self) -> Value {
        json!({"codec":"writer-literal-names-v1","names":self.names.len(),"occurrences":self.origins.len(),"input_sha256":self.input_sha256,
            "bindings_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&(&self.names,&self.people,&self.origins)).expect("name bindings")))})
    }
    pub(super) fn restore(&self, body: &str) -> Result<String> {
        ensure!(body.len() <= MAX_BYTES, "专有名称代码正文超过字节预算");
        static TOKEN: OnceLock<Regex> = OnceLock::new();
        let token = TOKEN.get_or_init(|| Regex::new(r"\[N([1-9][0-9]*)\]").unwrap());
        let mut restored = String::new();
        let mut cursor = 0;
        let mut count = 0;
        let mut depth = 0usize;
        let mut rendered_people = Vec::new();
        for capture in token.captures_iter(body) {
            count += 1;
            ensure!(count <= MAX_OCCURRENCES, "专有名称代码超过出现预算");
            let hit = capture.get(0).expect("name token");
            let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
            ensure!(
                !body[..hit.start()].ends_with(word) && !body[hit.end()..].starts_with(word),
                "专有名称代码不能嵌入另一个名字"
            );
            for ch in body[cursor..hit.start()].chars() {
                if ch == '[' {
                    depth += 1;
                }
                if ch == ']' {
                    depth = depth.saturating_sub(1);
                }
            }
            ensure!(
                depth == 0 && !body[hit.end()..].trim_start().starts_with(']'),
                "专有名称代码括号无效"
            );
            let id: usize = capture[1].parse()?;
            let name = id
                .checked_sub(1)
                .and_then(|id| self.names.get(id))
                .context("未知的专有名称代码")?;
            restored.push_str(&body[cursor..hit.start()]);
            let start = restored.len();
            restored.push_str(name);
            cursor = hit.end();
            if self.people.contains(name) {
                rendered_people.push(start..restored.len());
                let tail = &body[cursor..];
                let horizontal = tail.trim_start_matches([' ', '\t']);
                if ["教授", "博士"]
                    .iter()
                    .any(|title| horizontal.starts_with(title))
                {
                    // Only this already-bound person's adjacent horizontal
                    // presentation gap is normalized. A line break cannot
                    // manufacture a reported-person field for later folding.
                    cursor += tail.len() - horizontal.len();
                } else {
                    ensure!(
                        !["教授", "博士"]
                            .iter()
                            .any(|title| tail.trim_start().starts_with(title)),
                        "名称代码不能跨行建立报道人名"
                    );
                }
            }
            ensure!(restored.len() <= MAX_BYTES, "还原名称超过字节预算");
        }
        restored.push_str(&body[cursor..]);
        ensure!(
            restored.len() <= MAX_BYTES && !reserved(&restored),
            "未知或损坏的专有名称代码"
        );
        if !rendered_people.is_empty() {
            let full_people = crate::translation::reported_name_occurrences(&restored)?;
            for range in rendered_people {
                let before = &restored[..range.start];
                let after = &restored[range.end..];
                let lead = before.trim_end_matches([' ', '\t']);
                let tail = after.trim_start_matches([' ', '\t']);
                if needs_person_boundary(lead.chars().next_back(), lead.len() != before.len()) {
                    ensure!(
                        ["的", "由", "与", "和", "及", "向", "请"]
                            .iter()
                            .any(|word| lead.ends_with(word)),
                        "名称代码不能附加未观察的姓名前缀"
                    );
                }
                if needs_person_boundary(tail.chars().next(), tail.len() != after.len()) {
                    ensure!(
                        ["教授", "博士", "的", "与", "和", "及"]
                            .iter()
                            .chain(super::attribution_roles::REPORT_VERBS.iter())
                            .any(|word| tail.starts_with(word)),
                        "名称代码不能附加未观察的姓名后缀"
                    );
                }
                if ["教授", "博士"].iter().any(|title| tail.starts_with(title)) {
                    ensure!(
                        full_people.contains(&range),
                        "还原代码必须是来源中的完整报道人名，不能拼成另一个姓名"
                    );
                }
            }
        }
        Ok(restored)
    }
    pub(super) fn composition(
        &self,
        composed: super::writer::Composition,
    ) -> Result<super::writer::Composition> {
        Ok(super::writer::Composition::without_copy_authority(
            self.restore(composed.text())?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        modal_provenance_tests::{ordinary, snapshot, ORIGINAL_SOURCES},
        retrieval, writer,
    };
    use super::*;

    fn encoded(source: &crate::Snapshot, selected: &[usize], terms: &Terms) -> Result<Prepared> {
        let sources = selected
            .iter()
            .map(|i| source.segments[*i].clone())
            .collect::<Vec<_>>();
        let context = retrieval::Context {
            cursor: sources.last().map_or(0, |s| s.seq),
            segments: sources.clone(),
            keywords: Vec::new(),
        };
        let (system, content) = writer::prompt(source, &context, &json!({}), "zh");
        prepare(
            system,
            content,
            &sources,
            terms,
            &source.meeting.speaker_names,
            true,
            true,
        )
    }
    fn empty_terms() -> Terms {
        Terms::new(&[], &[])
    }

    #[test]
    fn current_named_sources_are_encoded_without_reordering_or_merging_literals() -> Result<()> {
        let sixth =
            "大学的潘 MALE FERGUSTON教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性";
        let mut texts = ORIGINAL_SOURCES.to_vec();
        texts.push(sixth);
        let source = snapshot(&texts);
        let before = serde_json::to_value(&source)?;
        let first = encoded(&source, &[0, 1, 2, 3, 4], &empty_terms())?;
        let plan = first.plan.as_ref().unwrap();
        assert_eq!(plan.names, ["PAMELA FURGO森", "潘 MALE FERGUSON"]);
        assert!(plan.restore("[N3]教授指出风险。").is_err());
        let input: Value = serde_json::from_str(&first.content)?;
        assert_eq!(input["transcript"].as_array().unwrap().len(), 5);
        for i in [0, 2, 4] {
            assert_eq!(input["transcript"][i]["text"], ORIGINAL_SOURCES[i]);
        }
        assert_eq!(
            input["transcript"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["id"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            [1, 2, 3, 4, 5]
        );
        let all = encoded(&source, &[0, 1, 2, 3, 4, 5], &empty_terms())?;
        assert_eq!(
            all.plan.as_ref().unwrap().names,
            ["PAMELA FURGO森", "潘 MALE FERGUSON", "潘 MALE FERGUSTON"]
        );
        assert_eq!(
            all.plan.as_ref().unwrap().restore("[N2]与[N3]")?,
            "潘 MALE FERGUSON与潘 MALE FERGUSTON"
        );
        assert_eq!(serde_json::to_value(&source)?, before);
        Ok(())
    }

    #[test]
    fn only_proven_name_spans_are_hidden_and_unspoken_terms_never_gain_tokens() -> Result<()> {
        let source = snapshot(&["MAY教授指出材料仍需核对。", "THE TASK MAY HAVE COMPLETED."]);
        let data = encoded(&source, &[0, 1], &empty_terms())?;
        let input: Value = serde_json::from_str(&data.content)?;
        assert_eq!(input["transcript"][0]["text"], "[N1]教授指出材料仍需核对。");
        assert_eq!(
            input["transcript"][1]["text"],
            "THE TASK MAY HAVE COMPLETED."
        );
        let conditional = snapshot(&["如果当迪大学的 PAMELA FURGO森教授指出材料仍需核对。"]);
        let data = encoded(&conditional, &[0], &empty_terms())?;
        let input: Value = serde_json::from_str(&data.content)?;
        assert_eq!(
            input["transcript"][0]["text"],
            "如果当迪大学的 [N1]教授指出材料仍需核对。"
        );
        let note = crate::UserNote {
            id: "n".into(),
            meeting_id: "m".into(),
            kind: "document".into(),
            content: "OmaScribe\nGTX1650".into(),
            anchor_ms: 0,
            updated_at: String::new(),
        };
        let terms = Terms::new(&[note], &[]);
        let source = snapshot(&["OmaScribe仍待测试。"]);
        let data = encoded(&source, &[0], &terms)?;
        assert_eq!(data.plan.as_ref().unwrap().names, ["OmaScribe"]);
        assert!(!data.content.contains("GTX1650"));
        Ok(())
    }

    #[test]
    fn restored_names_drop_copy_authority_without_bypassing_language_or_report_gates() -> Result<()>
    {
        let source = snapshot(&ORIGINAL_SOURCES);
        let data = encoded(&source, &[0, 1, 2, 3, 4], &empty_terms())?;
        let plan = data.plan.as_ref().unwrap();
        let copy = writer::body_with_evidence(&data.content, &data.content)?;
        assert!(!copy.copies().is_empty());
        let restored = plan.composition(copy)?;
        assert!(restored.copies().is_empty());
        assert!(restored.input().is_none());
        let error = ordinary(&source, &[0, 1, 2, 3, 4], restored.text(), None)
            .expect_err("name rendering cannot translate the copied English stack");
        assert!(error.to_string().contains("输出语言"), "{error:#}");
        let music = "音乐在主舞台可能已经结束，但嘉年华仍可能有部分区域持续演奏至深夜。";
        let faithful = format!(
            "{}。{}。{}",
            ORIGINAL_SOURCES[1], ORIGINAL_SOURCES[3], music
        );
        let opaque = faithful
            .replace("PAMELA FURGO森", "[N1]")
            .replace("潘 MALE FERGUSON", "[N2]");
        let body = plan.composition(writer::body_with_evidence(&opaque, &data.content)?)?;
        assert_eq!(body.text(), faithful);
        ordinary(&source, &[0, 1, 2, 3, 4], body.text(), None)?;
        let wrong = format!("[N1]教授指出{}{}。", music, ORIGINAL_SOURCES[3]);
        let wrong = plan.restore(&wrong)?;
        assert!(ordinary(&source, &[0, 1, 2, 3, 4], &wrong, None).is_err());
        Ok(())
    }

    #[test]
    fn reserved_input_abstains_and_invalid_generated_references_are_rejected() -> Result<()> {
        let source = snapshot(&ORIGINAL_SOURCES);
        let data = encoded(&source, &[0, 1, 2, 3, 4], &empty_terms())?;
        let plan = data.plan.as_ref().unwrap();
        for raw in [
            "[N0]", "[N99]", "[n1]", "[ N1]", "[N1", "N1]", "[[N1]]", "[N1]]", "[[N1]", "X[N1]",
        ] {
            assert!(plan.restore(raw).is_err(), "{raw}");
        }
        let mut input: Value = serde_json::from_str(&data.content)?;
        // Start from original source data, not already encoded fields.
        for (i, text) in ORIGINAL_SOURCES.iter().enumerate() {
            input["transcript"][i]["text"] = (*text).into();
        }
        input["background"] = json!({"writing_preferences":["literal [N1] is user data"]});
        let original = input.to_string();
        let fallback = prepare(
            "Write prose.".into(),
            original.clone(),
            &source.segments,
            &empty_terms(),
            &BTreeMap::new(),
            true,
            true,
        )?;
        assert!(fallback.plan.is_none());
        assert_eq!(fallback.content, original);
        Ok(())
    }

    #[test]
    fn omitted_and_repeated_labels_do_not_fabricate_summary_coverage() -> Result<()> {
        let source = snapshot(&ORIGINAL_SOURCES);
        let data = encoded(&source, &[0, 1, 2, 3, 4], &empty_terms())?;
        let plan = data.plan.as_ref().unwrap();
        assert_eq!(plan.restore("材料仍需核对。")?, "材料仍需核对。");
        assert_eq!(
            plan.restore("[N1]的材料待核对。[N1]的报告待检查。")?,
            "PAMELA FURGO森的材料待核对。PAMELA FURGO森的报告待检查。"
        );
        assert!(plan.restore(&"[N1]。".repeat(MAX_OCCURRENCES + 1)).is_err());
        Ok(())
    }

    #[test]
    fn bounded_or_disabled_rendering_leaves_the_original_request_intact() -> Result<()> {
        let lines = (b'A'..=b'M')
            .map(|c| format!("NAME{}教授指出材料仍需核对。", c as char))
            .collect::<Vec<_>>();
        let refs = lines.iter().map(String::as_str).collect::<Vec<_>>();
        let source = snapshot(&refs);
        let data = encoded(&source, &(0..13).collect::<Vec<_>>(), &empty_terms())?;
        assert!(data.plan.is_none());
        let original: Value = serde_json::from_str(&data.content)?;
        assert_eq!(original["transcript"][0]["text"], lines[0]);
        let source = snapshot(&ORIGINAL_SOURCES);
        let context = retrieval::Context {
            cursor: 5,
            segments: source.segments.clone(),
            keywords: Vec::new(),
        };
        let (system, content) = writer::prompt(&source, &context, &json!({}), "zh");
        let bypass = prepare(
            system.clone(),
            content.clone(),
            &source.segments,
            &empty_terms(),
            &BTreeMap::new(),
            false,
            true,
        )?;
        assert!(bypass.plan.is_none());
        assert_eq!(bypass.system, system);
        assert_eq!(bypass.content, content);
        Ok(())
    }

    #[test]
    fn restored_report_person_cannot_gain_an_unobserved_han_name_prefix() -> Result<()> {
        let source = snapshot(&["北岭大学的 ALICE教授指出记者如果公布照片似乎存在风险。"]);
        let data = encoded(&source, &[0], &empty_terms())?;
        let plan = data.plan.as_ref().unwrap();
        assert_eq!(plan.names, ["ALICE"]);
        let raw = "北岭大学的王[N1]教授指出记者如果公布照片似乎存在风险。";
        // The actual ordinary chain may reject a malformed rendered reference
        // at decoding or any existing final gate; it must not accept 王ALICE
        // as a new person merely because the known name token was intact.
        let result = writer::body_with_evidence(raw, &data.content)
            .and_then(|composed| plan.composition(composed))
            .and_then(|composed| ordinary(&source, &[0], composed.text(), None));
        assert!(
            result.is_err(),
            "an unobserved Han prefix changed the complete person label while the ordinary writer accepted it"
        );
        Ok(())
    }

    #[test]
    fn complete_person_tokens_keep_natural_grammar_and_horizontal_title_gaps() -> Result<()> {
        let source = snapshot(&["北岭大学的 ALICE教授指出记者如果公布照片似乎存在风险。"]);
        let data = encoded(&source, &[0], &empty_terms())?;
        let plan = data.plan.as_ref().unwrap();
        for raw in [
            "[N1]教授指出记者如果公布照片似乎存在风险。",
            "北岭大学的[N1]教授指出记者如果公布照片似乎存在风险。",
            "北岭大学的[N1] \t教授指出记者如果公布照片似乎存在风险。",
        ] {
            let body = plan.composition(writer::body_with_evidence(raw, &data.content)?)?;
            assert!(body.text().contains("ALICE教授"), "{raw}");
            assert!(body.copies().is_empty());
            assert!(body.input().is_none());
            ordinary(&source, &[0], body.text(), None)?;
        }
        assert_eq!(plan.restore("[N1]")?, "ALICE");
        assert!(plan
            .restore("[N1]\n教授指出记者如果公布照片似乎存在风险。")
            .is_err());
        Ok(())
    }

    #[test]
    fn unobserved_person_suffixes_are_rejected_without_restricting_identifier_prose() -> Result<()>
    {
        let source = snapshot(&["北岭大学的 ALICE教授指出记者如果公布照片似乎存在风险。"]);
        let data = encoded(&source, &[0], &empty_terms())?;
        let plan = data.plan.as_ref().unwrap();
        for raw in [
            "北岭大学的[N1]王教授指出记者如果公布照片似乎存在风险。",
            "北岭大学的[N1] 王教授指出记者如果公布照片似乎存在风险。",
            "北岭大学的王 [N1]教授指出记者如果公布照片似乎存在风险。",
        ] {
            let checked = writer::body_with_evidence(raw, &data.content)
                .and_then(|composed| plan.composition(composed))
                .and_then(|composed| ordinary(&source, &[0], composed.text(), None));
            assert!(checked.is_err(), "{raw}");
        }
        let note = crate::UserNote {
            id: "n".into(),
            meeting_id: "m".into(),
            kind: "document".into(),
            content: "OmaScribe\nGTX1650".into(),
            anchor_ms: 0,
            updated_at: String::new(),
        };
        let terms = Terms::new(&[note], &[]);
        for (raw, opaque) in [
            ("我正在测试OmaScribe的功能。", "我正在测试[N1]的功能。"),
            ("显卡使用GTX1650进行测试。", "显卡使用[N1]进行测试。"),
        ] {
            let source = snapshot(&[raw]);
            let data = encoded(&source, &[0], &terms)?;
            assert_eq!(data.plan.as_ref().unwrap().restore(opaque)?, raw);
        }
        Ok(())
    }

    #[test]
    fn whitespace_delimited_english_person_reference_passes_the_ordinary_final_chain() -> Result<()>
    {
        const ORIGINAL: &str =
            "Professor ALICE said that publishing a suspect's photograph might cause harm.";
        let source = snapshot(&[ORIGINAL]);
        let terms = empty_terms();
        let context = retrieval::Context {
            cursor: 1,
            segments: source.segments.clone(),
            keywords: Vec::new(),
        };
        let (system, input) = writer::prompt(&source, &context, &json!({}), "en");
        let ordinary_english = |body: &str| -> Result<Vec<crate::Suggestion>> {
            let checked = super::super::checked_writer_prose(body, &source.segments, &terms)?;
            let draft = terms.normalize_draft(&writer::document(
                &checked,
                &source.segments,
                &source.meeting.speaker_names,
            )?)?;
            super::super::validate_writer_document(
                &draft,
                &source,
                &source.segments,
                &terms,
                &source.meeting.id,
                false,
                None,
            )
        };
        ordinary_english(ORIGINAL)?;
        let data = prepare(
            system,
            input,
            &source.segments,
            &terms,
            &source.meeting.speaker_names,
            true,
            false,
        )?;
        let plan = data.plan.as_ref().unwrap();
        assert_eq!(plan.names, ["ALICE"]);
        let raw = "Professor [N1] said that publishing a suspect's photograph might cause harm.";
        let result = writer::body_with_evidence(raw, &data.content)
            .and_then(|composed| plan.composition(composed))
            .and_then(|composed| ordinary_english(composed.text()));
        assert!(result.is_ok(), "a source-identical English person reference separated by real spaces must remain valid: {:?}", result.err());
        Ok(())
    }

    #[test]
    fn bare_name_namespace_is_rejected_but_real_source_products_leave_the_plan_inactive(
    ) -> Result<()> {
        let source = snapshot(&ORIGINAL_SOURCES);
        let data = encoded(&source, &[0, 1, 2, 3, 4], &empty_terms())?;
        let plan = data.plan.as_ref().unwrap();
        for raw in ["N1", "N77", "n1", "文中N1教授", "Reference N77."] {
            assert!(plan.restore(raw).is_err(), "{raw}");
        }
        let product = snapshot(&["北岭大学的 ALICE教授指出材料仍需核对。", "N1产品仍待测试。"]);
        let context = retrieval::Context {
            cursor: 2,
            segments: product.segments.clone(),
            keywords: Vec::new(),
        };
        let (system, input) = writer::prompt(&product, &context, &json!({}), "zh");
        let fallback = prepare(
            system.clone(),
            input.clone(),
            &product.segments,
            &empty_terms(),
            &product.meeting.speaker_names,
            true,
            true,
        )?;
        assert!(fallback.plan.is_none());
        assert_eq!(fallback.content, input);
        assert_eq!(fallback.system, system);
        let unchanged: Value = serde_json::from_str(&fallback.content)?;
        assert_eq!(unchanged["transcript"][1]["text"], "N1产品仍待测试。");
        Ok(())
    }

    #[test]
    fn non_han_words_need_real_whitespace_instead_of_name_concatenation() -> Result<()> {
        let source = snapshot(&["Professor ALICE said the report remains pending."]);
        let data = encoded(&source, &[0], &empty_terms())?;
        let plan = data.plan.as_ref().unwrap();
        for raw in ["Professor[N1]", "[N1]said", "Ж[N1]", "[N1]Ж"] {
            assert!(plan.restore(raw).is_err(), "{raw}");
        }
        assert_eq!(plan.restore("Ж [N1] сказал")?, "Ж ALICE сказал");
        Ok(())
    }
}
