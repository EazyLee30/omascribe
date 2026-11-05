//! Joint preparation controls, not evidence of generated article quality.
use super::{
    coverage_coordinates_unchanged, coverage_slice,
    modal_provenance_tests::{snapshot, ORIGINAL_SOURCES},
    prepare_generated_writer_document, prepare_writer_input, retrieval, source_coverage,
    validate_writer_document, writer,
};
use crate::{terminology::Terms, Snapshot};
use anyhow::Result;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn terms(source: &Snapshot) -> Terms {
    Terms::new(&source.notes, &[]).with_actor_spellings(
        source.segments.iter().map(|s| s.text.as_str()),
        source.meeting.speaker_names.values().map(String::as_str),
    )
}

#[test]
fn initial_and_original_retry_rebuild_joint_bindings_after_reorder_and_trim() -> Result<()> {
    let source = snapshot(&ORIGINAL_SOURCES);
    let before = serde_json::to_value(&source)?;
    let terms = terms(&source);
    for (selected, repair) in [
        (vec![0, 1, 2, 3, 4], false),
        (vec![4, 1, 3, 0, 2], true),
        (vec![1, 3, 4], true),
    ] {
        let context = retrieval::Context {
            cursor: 5,
            segments: selected
                .iter()
                .map(|&index| source.segments[index].clone())
                .collect(),
            keywords: Vec::new(),
        };
        let (system, input) = if repair {
            writer::prompt_mode(&source, &context, &json!({}), "zh", false)
        } else {
            writer::prompt(&source, &context, &json!({}), "zh")
        };
        let (prepared, original) = prepare_writer_input(
            system,
            input,
            &source,
            &context.segments,
            &terms,
            (true, true, true),
            repair.then_some("Preserve the missing ORIGINAL source fields."),
        )?;
        let plan = prepared.plan.as_ref().expect("current names are encoded");
        assert_eq!(
            plan.metrics()["input_sha256"],
            format!(
                "{:x}",
                Sha256::digest(format!("{}\n{}", prepared.system, prepared.content))
            )
        );
        let original: Value = serde_json::from_str(&original)?;
        let encoded: Value = serde_json::from_str(&prepared.content)?;
        let fields = original["required_source_fields"].as_array().unwrap();
        assert_eq!(fields.len(), 2);
        let music_entry = selected.iter().position(|&i| i == 4).unwrap() + 1;
        for field in fields {
            assert_eq!(field["source_entry"], music_entry);
            assert_eq!(field["qualifier"], "may");
            assert_eq!(
                coverage_slice(&original, field),
                coverage_slice(&encoded, field)
            );
            assert!(coverage_slice(&encoded, field).is_some_and(|text| text.contains("MAY")));
        }
        assert!(original.to_string().contains("PAMELA FURGO森"));
        assert!(!original.to_string().contains("[N1]"));
        assert!(prepared.content.contains("[N1]"));
        assert_eq!(
            encoded["transcript"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["id"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            (1..=selected.len() as u64).collect::<Vec<_>>()
        );
    }
    assert_eq!(serde_json::to_value(&source)?, before);
    Ok(())
}

#[test]
fn joint_name_rendering_keeps_coverage_veto_and_abstains_on_moved_coordinates() -> Result<()> {
    let source = snapshot(&ORIGINAL_SOURCES);
    let terms = terms(&source);
    let context = retrieval::Context {
        cursor: 5,
        segments: source.segments.clone(),
        keywords: Vec::new(),
    };
    let (system, input) = writer::prompt(&source, &context, &json!({}), "zh");
    let (prepared, original) = prepare_writer_input(
        system,
        input,
        &source,
        &context.segments,
        &terms,
        (true, true, true),
        None,
    )?;
    let encoded: Value = serde_json::from_str(&prepared.content)?;
    let raw = format!(
        "{}。{}。",
        encoded["transcript"][1]["text"].as_str().unwrap(),
        encoded["transcript"][3]["text"].as_str().unwrap()
    );
    let restored = prepared
        .plan
        .as_ref()
        .unwrap()
        .composition(writer::body_with_evidence(&raw, &prepared.content)?)?;
    assert!(restored.copies().is_empty());
    assert!(restored.input().is_none());
    let draft =
        prepare_generated_writer_document(restored.text(), &source, &context.segments, &terms)?;
    let error = validate_writer_document(
        &draft.draft,
        &source,
        &context.segments,
        &terms,
        &source.meeting.id,
        true,
        None,
    )
    .expect_err("correct name rendering cannot acknowledge omitted music facts");
    assert!(error.is::<source_coverage::Missing>(), "{error:#}");

    let mut moved = encoded.clone();
    let text = moved["transcript"][4]["text"].as_str().unwrap();
    moved["transcript"][4]["text"] = format!("[N1] {text}").into();
    assert!(coverage_coordinates_unchanged(&original, &moved.to_string()).is_err());
    coverage_coordinates_unchanged(&original, &prepared.content)?;
    assert_eq!(source.meeting.notes_cursor, 0);
    Ok(())
}

#[test]
fn actual_second_retrieval_rebuilds_names_and_current_coverage_for_source_six() -> Result<()> {
    let cases: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/document-cases.json"
    ))?;
    let case = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "zh_recorded_mixed_prose_fidelity")
        .unwrap();
    let text = case["transcript"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(text[..5], ORIGINAL_SOURCES);
    let mut source = snapshot(&text);
    let first = retrieval::retrieve(&source, None);
    assert_eq!(first.cursor, 5);
    assert!(first.segments.iter().all(|s| s.seq <= 5));
    let terms = terms(&source);
    let (system, input) = writer::prompt(&source, &first, &json!({}), "zh");
    let (prepared_first, _) = prepare_writer_input(
        system,
        input,
        &source,
        &first.segments,
        &terms,
        (true, true, true),
        None,
    )?;
    source.meeting.notes_cursor = 5;
    let second = retrieval::retrieve(&source, None);
    assert_eq!(second.cursor, 6);
    assert!(second.segments.iter().any(|s| s.seq == 6));
    let (system, input) = writer::prompt_mode(&source, &second, &json!({}), "zh", false);
    let (prepared_second, original) = prepare_writer_input(
        system,
        input,
        &source,
        &second.segments,
        &terms,
        (true, true, true),
        Some("Use the current ORIGINAL sources."),
    )?;
    let first_metrics = prepared_first.plan.as_ref().unwrap().metrics();
    let second_plan = prepared_second.plan.as_ref().unwrap();
    assert_ne!(
        first_metrics["bindings_sha256"],
        second_plan.metrics()["bindings_sha256"]
    );
    let input: Value = serde_json::from_str(&prepared_second.content)?;
    let sixth = second.segments.iter().position(|s| s.seq == 6).unwrap();
    let rendered = second_plan.restore(input["transcript"][sixth]["text"].as_str().unwrap())?;
    assert_eq!(rendered, text[5]);
    coverage_coordinates_unchanged(&original, &prepared_second.content)?;
    assert_eq!(
        source.meeting.notes_cursor, 5,
        "preparation does not commit the new source"
    );
    Ok(())
}
