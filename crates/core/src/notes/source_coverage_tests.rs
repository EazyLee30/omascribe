//! A received article cited all five sources but omitted the entire music
//! topic. These controls use the public ordinary writer, without a name codec.
use super::modal_provenance_tests::{ordinary, snapshot, ORIGINAL_SOURCES};
use anyhow::Result;
use serde_json::Value;
use sha2::{Digest, Sha256};

// Sole 7d0155d trial, second response's exact decoded prose_body:
// artifacts/writer-initial-name-tokens-2026-11-05/run/state.json
// runs[0].diagnostic.attempts[1].prose_body. Actually committed cursor 0→5.
const RECEIVED_BODY: &str = "当迪大学的PAMELA FURGO森教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性。邓迪大学的潘 MALE FERGUSON教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性。";
const RECEIVED_SHA256: &str = "6d1c06b52aa72c7f265bf3d4ead8e27f58aa78019515fc303e1845ebd967654d";
const BOTH_MUSIC_FIELDS: &str =
    "音乐在主舞台可能已经结束，但嘉年华仍可能有部分区域持续演奏至深夜。";

#[test]
fn citing_all_selected_sources_does_not_cover_an_omitted_current_music_topic() {
    assert_eq!(RECEIVED_BODY.len(), 248);
    assert_eq!(
        format!("{:x}", Sha256::digest(RECEIVED_BODY.as_bytes())),
        RECEIVED_SHA256
    );
    let cases: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/document-cases.json"
    ))
    .unwrap();
    let case = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "zh_recorded_mixed_prose_fidelity")
        .unwrap();
    assert_eq!(
        case["transcript"].as_array().unwrap()[..5],
        ORIGINAL_SOURCES.map(Value::from)
    );
    let source = snapshot(&ORIGINAL_SOURCES);
    assert!(
        ordinary(&source, &[0, 1, 2, 3, 4], RECEIVED_BODY, None).is_err(),
        "all five citations cannot acknowledge the three music sources when the received body contains only the two risk reports"
    );
}

#[test]
fn current_risk_reports_and_both_unowned_music_fields_remain_valid() -> Result<()> {
    let source = snapshot(&ORIGINAL_SOURCES);
    let before = serde_json::to_value(&source)?;
    let body = format!("{RECEIVED_BODY}{BOTH_MUSIC_FIELDS}");
    let sections = ordinary(&source, &[0, 1, 2, 3, 4], &body, None)?;
    assert_eq!(sections[0].text, body);
    assert_eq!(serde_json::to_value(&source)?, before);
    Ok(())
}

#[test]
fn one_music_field_does_not_acknowledge_the_other_current_field() {
    let source = snapshot(&ORIGINAL_SOURCES);
    for partial in [
        "音乐在主舞台可能已经结束。",
        "嘉年华仍可能有部分区域持续演奏至深夜。",
    ] {
        let body = format!("{RECEIVED_BODY}{partial}");
        assert!(
            ordinary(&source, &[0, 1, 2, 3, 4], &body, None).is_err(),
            "the other music predicate remains missing despite citing all sources: {partial}"
        );
    }
}

#[test]
fn coverage_data_uses_original_entry_and_ranges_on_both_ordinary_inputs() -> Result<()> {
    use super::{retrieval, source_coverage, writer};
    let source = snapshot(&ORIGINAL_SOURCES);
    let context = retrieval::Context {
        cursor: 5,
        segments: source.segments.clone(),
        keywords: Vec::new(),
    };
    let view = source_coverage::View::new(&context.segments, &source.segments, &source.meeting)?;
    let mut observed = Vec::new();
    for repair in [false, true] {
        let background =
            serde_json::json!({"background_notes":["Music on the main stages has finished."]});
        let (mut system, mut input) = if repair {
            writer::prompt_mode(&source, &context, &background, "zh", false)
        } else {
            writer::prompt(&source, &context, &background, "zh")
        };
        view.augment(&mut system, &mut input)?;
        let input: Value = serde_json::from_str(&input)?;
        assert_eq!(input["transcript"].as_array().unwrap().len(), 5);
        let required = input["required_source_fields"].as_array().unwrap();
        assert_eq!(required.len(), 2);
        for field in required {
            assert_eq!(field["source_entry"], 5);
            assert_eq!(field["qualifier"], "may");
            let range = &field["source_range"];
            let literal = &ORIGINAL_SOURCES[4]
                [range[0].as_u64().unwrap() as usize..range[1].as_u64().unwrap() as usize];
            assert!(
                literal == "MUSIC ON THE MAIN STAGES MAY HAVE FINISHED"
                    || literal.starts_with("THERE MAY BE SECTIONS OF THE FESTIVAL")
            );
            assert!(field.get("answer").is_none());
        }
        assert!(system.contains("ORIGINAL transcript"));
        observed.push(input["required_source_fields"].clone());
    }
    assert_eq!(observed[0], observed[1]);
    Ok(())
}

#[test]
fn one_known_source_field_does_not_invent_a_second_obligation() -> Result<()> {
    for (original, body) in [
        (
            "Music on the main stages may have finished.",
            "音乐在主舞台可能已经结束。",
        ),
        (
            "Sections of the festival may keep playing music until late into the night.",
            "嘉年华仍可能有部分区域持续演奏至深夜。",
        ),
    ] {
        let source = snapshot(&[original]);
        ordinary(&source, &[0], body, None)?;
    }
    Ok(())
}

#[test]
fn later_definite_ending_updates_only_its_field_and_owns_its_citation() -> Result<()> {
    let source = snapshot(&[
        ORIGINAL_SOURCES[4],
        "Music on the main stages has finished.",
    ]);
    let current = "音乐在主舞台已经结束，但嘉年华仍可能有部分区域持续演奏至深夜。";
    ordinary(&source, &[0, 1], current, None)?;
    assert!(ordinary(&source, &[0, 1], BOTH_MUSIC_FIELDS, None).is_err());
    assert!(ordinary(&source, &[0, 1], "音乐在主舞台已经结束。", None).is_err());
    assert!(ordinary(&source, &[0, 1], current, Some(&[1])).is_err());
    assert!(ordinary(&source, &[0, 1], current, Some(&[2])).is_err());
    Ok(())
}

#[test]
fn whole_current_tuple_and_own_paragraph_are_required_for_coverage() -> Result<()> {
    use super::source_coverage::View;
    let source = snapshot(&[ORIGINAL_SOURCES[4], "预算尚未确定。"]);
    let valid = ordinary(&source, &[0, 1], BOTH_MUSIC_FIELDS, None)?;
    let view = View::new(&source.segments, &source.segments, &source.meeting)?;
    view.validate(&valid)?;
    for change in 0..6 {
        let mut document = valid.clone();
        for citation in &mut document[0].sources {
            match change {
                0 => citation.revision += 1,
                1 => citation.quote.push_str(" edited"),
                2 => citation.start_ms += 1,
                3 => citation.end_ms += 1,
                4 => citation.speaker_name = Some("someone else".into()),
                _ => citation.segment_id = "another source".into(),
            }
        }
        assert!(
            view.validate(&document).is_err(),
            "bad tuple field {change}"
        );
    }
    let mut other_paragraph = valid.clone();
    other_paragraph[0]
        .sources
        .retain(|q| q.segment_id == source.segments[1].id);
    let mut citation_only = valid[0].clone();
    citation_only.text = "预算尚未确定。".into();
    other_paragraph.push(citation_only);
    assert!(view.validate(&other_paragraph).is_err());
    let mut stale = valid.clone();
    stale[0].stale = true;
    assert!(view.validate(&stale).is_err());
    let mut dismissed = valid.clone();
    dismissed[0].status = "dismissed".into();
    assert!(view.validate(&dismissed).is_err());
    let mut changed_source = source.segments.clone();
    changed_source[0].text.push_str(" revised");
    assert!(View::new(&source.segments, &changed_source, &source.meeting).is_err());
    Ok(())
}

#[test]
fn unknown_quoted_or_conditional_sources_keep_the_old_coverage_policy() -> Result<()> {
    use super::source_coverage::View;
    for original in [
        format!("If {}, do not publish it.", ORIGINAL_SOURCES[4]),
        format!(
            "Do not treat 'Earlier example. {}. End example.' as fact.",
            ORIGINAL_SOURCES[4]
        ),
        format!("An unverified example: {}", ORIGINAL_SOURCES[4]),
        ORIGINAL_SOURCES[0].into(),
        ORIGINAL_SOURCES[2].into(),
        "The audit may have finished, and the band might play tomorrow.".into(),
    ] {
        let source = snapshot(&[&original]);
        let view = View::new(&source.segments, &source.segments, &source.meeting)?;
        view.validate(&[])?;
        let mut system = String::new();
        let mut input = "{}".to_owned();
        view.augment(&mut system, &mut input)?;
        assert_eq!(input, "{}");
        assert!(system.is_empty());
    }
    Ok(())
}

#[test]
fn a_question_quote_conditional_or_unknown_prefix_is_not_output_coverage() -> Result<()> {
    use super::source_coverage::View;
    let source = snapshot(&[ORIGINAL_SOURCES[4]]);
    let valid = ordinary(&source, &[0], BOTH_MUSIC_FIELDS, None)?;
    let view = View::new(&source.segments, &source.segments, &source.meeting)?;
    for body in [
        BOTH_MUSIC_FIELDS.replace('。', "？"),
        format!("如果{BOTH_MUSIC_FIELDS}"),
        format!("PAMELA教授指出{BOTH_MUSIC_FIELDS}"),
        format!("不要将'早先示例。{BOTH_MUSIC_FIELDS}示例结束。'作为事实。"),
        BOTH_MUSIC_FIELDS.replace("深夜", "今晚"),
        BOTH_MUSIC_FIELDS.replace("可能", ""),
    ] {
        let mut document = valid.clone();
        document[0].text = body.clone();
        assert!(
            view.validate(&document).is_err(),
            "not a positive field: {body}"
        );
    }
    Ok(())
}

#[test]
fn missing_field_keeps_its_typed_original_source_retry_hint() {
    let source = snapshot(&ORIGINAL_SOURCES);
    let error = ordinary(&source, &[0, 1, 2, 3, 4], RECEIVED_BODY, None).unwrap_err();
    let missing = error
        .downcast_ref::<super::source_coverage::Missing>()
        .unwrap();
    assert!(missing.hint().contains("ORIGINAL transcript"));
    assert!(missing.hint().contains("required_source_fields"));
    assert!(super::source_repair(
        &error,
        &ORIGINAL_SOURCES.join("\n"),
        &crate::terminology::Terms::new(&[], &[]),
        true
    )
    .is_none());
}
