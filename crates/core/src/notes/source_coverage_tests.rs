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

#[test]
fn later_unknown_negative_field_does_not_require_the_superseded_may() -> Result<()> {
    use super::{language, retrieval, source_coverage::View, validation, writer};
    let source = snapshot(&[
        "Music on the main stages may have finished.",
        "Main-stage music has not ended.",
    ]);
    let body = "主舞台音乐尚未结束。";
    let terms = crate::terminology::Terms::new(&[], &[]);
    // Check the pre-coverage production gates first: this is not a request to
    // widen an older modal, spelling or language exception.
    let draft = writer::document(body, &source.segments, &source.meeting.speaker_names)?;
    let sections = validation::validate_with_terms(
        &draft,
        &source.segments,
        &source.meeting.id,
        &source.meeting.speaker_names,
        &terms,
    )?;
    let original = source
        .segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    terms.validate_writer_names(&original, body)?;
    language::validate_with_source(
        body,
        &crate::translation::source_name_mask(body, &original, &terms),
        &original,
        true,
    )?;
    let view = View::new(&source.segments, &source.segments, &source.meeting)?;
    let context = retrieval::Context {
        cursor: 2,
        segments: source.segments.clone(),
        keywords: Vec::new(),
    };
    let (mut system, mut input) =
        writer::prompt_mode(&source, &context, &serde_json::json!({}), "zh", false);
    view.augment(&mut system, &mut input)?;
    let input: Value = serde_json::from_str(&input)?;
    let coverage = view.validate(&sections);
    assert!(input.get("required_source_fields").is_none() && coverage.is_ok(), "the old gates accept the faithful negative; a later unknown/negative statement of this same field must make its old obligation abstain: data={}, coverage={coverage:?}",input["required_source_fields"]);
    Ok(())
}

#[test]
fn cached_music_coverage_ranges_bind_the_model_visible_coordinate_space() -> Result<()> {
    use super::{language, retrieval, source_coverage::View, writer};
    let mut source = snapshot(&[ORIGINAL_SOURCES[4]]);
    let segment = &source.segments[0];
    let cached = crate::translation::Translation {
        segment_id: segment.id.clone(),
        revision: segment.revision,
        target: "zh".into(),
        model: "local".into(),
        cache_key: "validated-snapshot-key".into(),
        text: BOTH_MUSIC_FIELDS.into(),
    };
    let terms = crate::terminology::Terms::new(&[], &[]);
    assert_eq!(
        language::cached_text(segment, &segment.text, &cached, true, &terms),
        Some(BOTH_MUSIC_FIELDS)
    );
    source.translations.insert(segment.id.clone(), cached);
    let context = retrieval::Context {
        cursor: 1,
        segments: source.segments.clone(),
        keywords: Vec::new(),
    };
    let (mut system, mut input) = writer::prompt(&source, &context, &serde_json::json!({}), "zh");
    assert_eq!(
        language::reused_count(&input, &context.segments, &source),
        1
    );
    let view = View::new(&context.segments, &source.segments, &source.meeting)?;
    view.augment(&mut system, &mut input)?;
    let input: Value = serde_json::from_str(&input)?;
    let entry = &input["transcript"][0];
    assert_eq!(
        entry["text"], BOTH_MUSIC_FIELDS,
        "a fully proved cache remains reusable"
    );
    for field in input["required_source_fields"].as_array().unwrap() {
        // Rebased actual-text ranges or an explicitly exposed original source
        // are both sound; English coordinates on an unseen English original
        // must not silently point into the displayed Chinese cache.
        let visible = if field["range_basis"] == "original_source" {
            entry["original_source"].as_str().unwrap()
        } else {
            entry["text"].as_str().unwrap()
        };
        let range = &field["source_range"];
        let observed =
            visible.get(range[0].as_u64().unwrap() as usize..range[1].as_u64().unwrap() as usize);
        let valid=match field["field"].as_str().unwrap() {
            "MainStageMusicEnding"=>matches!(observed,Some("MUSIC ON THE MAIN STAGES MAY HAVE FINISHED"|"音乐在主舞台可能已经结束")),
            "FestivalPartsPlayingUntilLateNight"=>matches!(observed,Some("THERE MAY BE SECTIONS OF THE FESTIVAL THAT WILL KEEP PLAYING MUSIC UNTIL LATE INTO THE NIGHT"|"嘉年华仍可能有部分区域持续演奏至深夜")),
            other=>panic!("unexpected field {other}"),
        };
        assert!(valid,"coverage range must identify the promised fact in its declared visible source: field={field}, slice={observed:?}");
    }
    Ok(())
}

// Keep the pre-coverage ordinary production gates explicit here. The new
// accepted paragraph control must fail only at the new public coverage gate,
// not because its raw model draft or an existing validator is already invalid.
fn ordinary_before_source_coverage(
    source: &crate::Snapshot,
    raw: &str,
) -> Result<(String, Vec<crate::Suggestion>)> {
    use super::{language, retrieval, validation, writer};
    let terms = crate::terminology::Terms::new(&source.notes, &[]).with_actor_spellings(
        source.segments.iter().map(|s| s.text.as_str()),
        source.meeting.speaker_names.values().map(String::as_str),
    );
    let context = retrieval::Context {
        cursor: source.segments.last().unwrap().seq,
        segments: source.segments.clone(),
        keywords: Vec::new(),
    };
    let (_, input) = writer::prompt(source, &context, &serde_json::json!({}), "zh");
    let composed = writer::body_with_evidence(raw, &input)?;
    let checked = super::checked_writer_prose(composed.text(), &source.segments, &terms)?;
    let draft = terms.normalize_draft(&writer::document(
        &checked,
        &source.segments,
        &source.meeting.speaker_names,
    )?)?;
    let sections = validation::validate_with_terms(
        &draft,
        &source.segments,
        &source.meeting.id,
        &source.meeting.speaker_names,
        &terms,
    )?;
    validation::preserve_replaced_sources(&source.suggestions, &source.segments, &sections)?;
    let body = sections
        .iter()
        .map(|section| section.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let original = source
        .segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    terms.validate_writer_names(&original, &body)?;
    language::validate_with_source(
        &body,
        &crate::translation::source_name_mask(&body, &original, &terms),
        &original,
        true,
    )?;
    Ok((draft, sections))
}

#[test]
fn accepted_current_music_does_not_require_repeating_it_in_new_risk_prose() -> Result<()> {
    use super::source_coverage::View;
    let risk = ORIGINAL_SOURCES[1];
    let mut source = snapshot(&[ORIGINAL_SOURCES[4], risk]);
    source.meeting.notes_cursor = 1;
    let (_, mut music) = ordinary_before_source_coverage(&source, BOTH_MUSIC_FIELDS)?;
    assert_eq!(music.len(), 1);
    let mut accepted = music.remove(0);
    accepted.id = "accepted-current-music".into();
    accepted.status = "accepted".into();
    accepted
        .sources
        .retain(|q| q.segment_id == source.segments[0].id);
    assert_eq!(accepted.sources.len(), 1);
    source.suggestions.push(accepted);
    let before = serde_json::to_value(&source)?;

    // The SAME model draft passes every old ordinary gate, and the exact
    // projected article covers music in the immutable accepted paragraph.
    let (_, next) = ordinary_before_source_coverage(&source, risk)?;
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].text, risk);
    let view = View::new(&source.segments, &source.segments, &source.meeting)?;
    assert!(view.validate(&next).is_err());
    view.validate(source.suggestions.iter().chain(next.iter()))?;
    assert_eq!(source.suggestions[0].status, "accepted");
    assert!(!source.suggestions[0].stale);

    let result = ordinary(&source, &[0, 1], risk, None);
    assert!(result.is_ok(), "a new risk paragraph must not repeat music that remains covered by current accepted text; old gates and the final projection passed: {result:?}");
    assert_eq!(serde_json::to_value(&source)?, before);
    Ok(())
}

#[test]
fn replaceable_pending_music_cannot_lend_coverage_to_its_risk_replacement() -> Result<()> {
    use super::source_coverage::View;
    let risk = ORIGINAL_SOURCES[1];
    let mut source = snapshot(&[ORIGINAL_SOURCES[4], risk]);
    source.meeting.notes_cursor = 1;
    let (_, mut music) = ordinary_before_source_coverage(&source, BOTH_MUSIC_FIELDS)?;
    let mut pending = music.remove(0);
    pending.id = "replaceable-current-music".into();
    pending
        .sources
        .retain(|q| q.segment_id == source.segments[0].id);
    assert_eq!(pending.status, "pending");
    assert!(pending.sources.iter().all(|q| source
        .segments
        .iter()
        .any(|selected| selected.id == q.segment_id)));
    source.suggestions.push(pending);
    let before = serde_json::to_value(&source)?;
    let (_, next) = ordinary_before_source_coverage(&source, risk)?;
    let view = View::new(&source.segments, &source.segments, &source.meeting)?;
    // Retaining the pending old body would make a naive union appear covered,
    // but Store will replace it: only the new risk body survives that projection.
    view.validate(source.suggestions.iter().chain(next.iter()))?;
    assert!(view.validate(&next).is_err());
    let error = ordinary(&source, &[0, 1], risk, None)
        .expect_err("a pending paragraph that will be replaced cannot provide music coverage");
    assert!(error.is::<super::source_coverage::Missing>(), "{error:#}");
    assert_eq!(serde_json::to_value(&source)?, before);
    Ok(())
}

#[test]
fn an_unresolved_update_retires_only_its_field_without_granting_positive_proof() -> Result<()> {
    use super::{retrieval, source_coverage::View, writer};
    for (update, retained, field) in [
        (
            "Main-stage music has not ended.",
            "嘉年华仍可能有部分区域持续演奏至深夜。",
            "FestivalPartsPlayingUntilLateNight",
        ),
        (
            "Main-stage music has perhaps ended.",
            "嘉年华仍可能有部分区域持续演奏至深夜。",
            "FestivalPartsPlayingUntilLateNight",
        ),
        (
            "Sections of the festival will not keep playing music until late into the night.",
            "音乐在主舞台可能已经结束。",
            "MainStageMusicEnding",
        ),
    ] {
        let source = snapshot(&[ORIGINAL_SOURCES[4], update]);
        ordinary(&source, &[0, 1], retained, None)?;
        // Abstention never bypasses the old modal/source certainty veto.
        assert!(ordinary(
            &source,
            &[0, 1],
            "音乐在主舞台已经结束，但嘉年华的某些部分仍将持续演奏至深夜。",
            None
        )
        .is_err());
        let view = View::new(&source.segments, &source.segments, &source.meeting)?;
        let context = retrieval::Context {
            cursor: 2,
            segments: source.segments.clone(),
            keywords: Vec::new(),
        };
        let (mut system, mut input) =
            writer::prompt_mode(&source, &context, &serde_json::json!({}), "zh", false);
        view.augment(&mut system, &mut input)?;
        let input: Value = serde_json::from_str(&input)?;
        let required = input["required_source_fields"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0]["field"], field);
        assert_eq!(required[0]["source_entry"], 1);
        assert_eq!(required[0]["qualifier"], "may");
    }
    Ok(())
}

#[test]
fn a_condition_question_quote_or_other_object_cannot_retire_a_music_field() -> Result<()> {
    use super::source_coverage::View;
    for update in [
        "If main-stage music has not ended, do not start the audit.",
        "Main-stage music has not ended?",
        "Do not treat 'Earlier example. Main-stage music has not ended. End example.' as fact.",
        "Main-stage music audit has not ended.",
        "Main-stage music has not ended buffering.",
        "If sections of the festival will not keep playing music until late into the night, cancel the audit.",
        "Sections of the festival will not keep playing music until late into the night?",
        "Sections of the festival inspection team will not keep playing music until late into the night.",
    ] {
        let source=snapshot(&[ORIGINAL_SOURCES[4],update]);
        let complete=ordinary(&source,&[0,1],BOTH_MUSIC_FIELDS,None)?;
        let view=View::new(&source.segments,&source.segments,&source.meeting)?;
        for partial in ["音乐在主舞台可能已经结束。","嘉年华仍可能有部分区域持续演奏至深夜。"] {
            let mut document=complete.clone();
            document[0].text=partial.into();
            assert!(view.validate(&document).is_err(),"{update} must not retire either field");
        }
    }
    // Input order cannot turn an earlier unresolved observation into a later
    // update of the known current source.
    let source = snapshot(&["Main-stage music has not ended.", ORIGINAL_SOURCES[4]]);
    let complete = ordinary(&source, &[1, 0], BOTH_MUSIC_FIELDS, None)?;
    let view = View::new(&source.segments, &source.segments, &source.meeting)?;
    let mut partial = complete;
    partial[0].text = "嘉年华仍可能有部分区域持续演奏至深夜。".into();
    assert!(view.validate(&partial).is_err());
    Ok(())
}

#[test]
fn ordinary_prompt_demands_only_fields_missing_from_current_retained_accepted_text() -> Result<()> {
    use super::{retrieval, source_coverage::View, writer};
    let source = snapshot(&[ORIGINAL_SOURCES[4], ORIGINAL_SOURCES[1]]);
    let (_, mut article) = ordinary_before_source_coverage(&source, BOTH_MUSIC_FIELDS)?;
    let mut accepted = article.remove(0);
    accepted.status = "accepted".into();
    accepted
        .sources
        .retain(|q| q.segment_id == source.segments[0].id);
    let view = View::new(&source.segments, &source.segments, &source.meeting)?;
    let context = retrieval::Context {
        cursor: 2,
        segments: source.segments.clone(),
        keywords: Vec::new(),
    };
    for change in 0..9 {
        let mut retained = accepted.clone();
        match change {
            0 => {}
            1 => retained.status = "pending".into(),
            2 => retained.stale = true,
            3 => retained.meeting_id = "other meeting".into(),
            4 => retained.sources[0].revision += 1,
            5 => retained.sources[0].quote.push_str(" altered"),
            6 => retained.status = "dismissed".into(),
            7 => retained.text = BOTH_MUSIC_FIELDS.replace('。', "？"),
            _ => retained.text = "音乐在主舞台可能已经结束。".into(),
        }
        for retry in [false, true] {
            let (mut system, mut input) = if retry {
                writer::prompt_mode(&source, &context, &serde_json::json!({}), "zh", false)
            } else {
                writer::prompt(&source, &context, &serde_json::json!({}), "zh")
            };
            let before = input.clone();
            view.augment_with_accepted(&mut system, &mut input, std::slice::from_ref(&retained))?;
            let input: Value = serde_json::from_str(&input)?;
            if change == 0 {
                assert!(input.get("required_source_fields").is_none());
                assert_eq!(serde_json::from_str::<Value>(&before)?, input);
            } else if change == 8 {
                let fields = input["required_source_fields"].as_array().unwrap();
                assert_eq!(fields.len(), 1);
                assert_eq!(fields[0]["field"], "FestivalPartsPlayingUntilLateNight");
            } else {
                assert_eq!(
                    input["required_source_fields"].as_array().unwrap().len(),
                    2,
                    "invalid retained authority {change}"
                );
            }
        }
    }
    Ok(())
}
