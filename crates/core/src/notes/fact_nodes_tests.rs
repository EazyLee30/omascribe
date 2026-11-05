use super::{
    fact_nodes,
    modal_provenance_tests::{snapshot, ORIGINAL_SOURCES},
    retrieval, source_language, writer,
};
use crate::{terminology::Terms, Snapshot};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const MUSIC: &str = "音乐在主舞台可能已经结束，但嘉年华仍可能有部分区域持续演奏至深夜。";
const RISK: &str = "教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性。";

fn prepared(
    texts: &[&str],
) -> Result<(
    Snapshot,
    Terms,
    source_language::Prepared,
    String,
    fact_nodes::Plan,
)> {
    let source = snapshot(texts);
    let terms = Terms::new(&[], &[]).with_actor_spellings(
        source.segments.iter().map(|s| s.text.as_str()),
        std::iter::empty(),
    );
    let context = retrieval::Context {
        segments: source.segments.clone(),
        cursor: source.segments.last().unwrap().seq,
        keywords: Vec::new(),
    };
    let (mut system, mut input) = writer::prompt_mode(&source, &context, &json!({}), "zh", false);
    super::source_coverage::View::new(&source.segments, &source.segments, &source.meeting)?
        .augment_with_accepted(&mut system, &mut input, &source.suggestions)?;
    let plan = source_language::Plan::new(&input, &source.segments, true, &terms)?
        .context("Expected actual translation preparation")?;
    let validated = plan.prepare(&json!({"A":MUSIC}).to_string(), &source.segments, &terms)?;
    let nodes = fact_nodes::Plan::from_prepared(&validated, &input, &source, &terms, true)?;
    Ok((source, terms, validated, input, nodes))
}
fn article() -> String {
    json!({"paragraphs":[{"parts":[{"f":1},{"f":2},{"f":3}]}]}).to_string()
}
fn validate(
    source: &Snapshot,
    terms: &Terms,
    nodes: &fact_nodes::Plan,
    raw: &str,
) -> Result<Vec<crate::Suggestion>> {
    let rendered = nodes.render(raw, nodes.input(), source, terms, true)?;
    let composed = rendered.composition();
    assert!(composed.input().is_none());
    assert!(composed.copies().is_empty());
    let checked = super::checked_writer_prose(composed.text(), &source.segments, terms)?;
    nodes.check_rendered(&rendered, &checked)?;
    let bridge =
        source_language::CompletionBridge::from_fact_nodes(nodes, &rendered, source, terms, true)?;
    let draft = terms.normalize_draft(&writer::document(
        &checked,
        &source.segments,
        &source.meeting.speaker_names,
    )?)?;
    let sections = super::validate_writer_document(
        &draft,
        source,
        &source.segments,
        terms,
        &source.meeting.id,
        true,
        bridge.as_ref(),
    )?;
    nodes.check_sections(&rendered, &sections)?;
    Ok(sections)
}

#[test]
fn validated_runtime_clause_is_immutable_and_plain_names_gain_no_copy_proof() -> Result<()> {
    let (source, terms, validated, original_input, nodes) = prepared(&[
        ORIGINAL_SOURCES[4],
        ORIGINAL_SOURCES[1],
        ORIGINAL_SOURCES[3],
    ])?;
    let input: Value = serde_json::from_str(nodes.input())?;
    assert_eq!(input["transcript"].as_array().unwrap().len(), 3);
    assert_eq!(input["fact_nodes"].as_array().unwrap().len(), 3);
    assert_eq!(input["fact_nodes"][0]["source_entry"], 1);
    assert_eq!(input["fact_nodes"][0]["text"], MUSIC);
    assert_eq!(input["fact_nodes"][1]["text"], ORIGINAL_SOURCES[1]);
    assert_eq!(input["fact_nodes"][2]["text"], ORIGINAL_SOURCES[3]);
    assert!(input.get("required_source_fields").is_none());
    assert!(input["transcript"][0].get("original_source").is_none());
    let actual: Value = serde_json::from_str(&validated.checked_input(
        &original_input,
        &source.segments,
        &terms,
        true,
    )?)?;
    assert_eq!(nodes.expanded_transcript()?, actual["transcript"]);
    let sections = validate(&source, &terms, &nodes, &article())?;
    assert_eq!(sections.len(), 1);
    assert!(sections[0].text.contains(MUSIC));
    assert!(sections[0].text.contains("PAMELA FURGO森教授"));
    assert!(sections[0].text.contains("潘 MALE FERGUSON教授"));
    assert!(sections[0]
        .sources
        .iter()
        .any(|q| q.quote == ORIGINAL_SOURCES[4] && q.revision == 1));
    // Actual earlier successful preparation, not a new model response or a
    // prefilled production answer. Origin: after-guard/.../batch-1/last-generation.
    let received =
        include_str!("../../../../tests/fixtures/received-source-preparation-2026-10-30.json");
    assert_eq!(received.len(), 364);
    assert_eq!(
        format!("{:x}", Sha256::digest(received.as_bytes())),
        "c0245e7b88cbe230e3f851160b7ffb4f370d06d96948d8c26dbbd111103ac7e5"
    );
    let all = snapshot(&ORIGINAL_SOURCES);
    let terms = Terms::new(&[], &[]).with_actor_spellings(
        all.segments.iter().map(|s| s.text.as_str()),
        std::iter::empty(),
    );
    let context = retrieval::Context {
        segments: all.segments.clone(),
        cursor: 5,
        keywords: Vec::new(),
    };
    let (mut system, mut input) = writer::prompt_mode(&all, &context, &json!({}), "zh", false);
    super::source_coverage::View::new(&all.segments, &all.segments, &all.meeting)?
        .augment_with_accepted(&mut system, &mut input, &[])?;
    let plan = source_language::Plan::new(&input, &all.segments, true, &terms)?.unwrap();
    let prepared = plan.prepare(received, &all.segments, &terms)?;
    let nodes = fact_nodes::Plan::from_prepared(&prepared, &input, &all, &terms, true)?;
    let visible: Value = serde_json::from_str(nodes.input())?;
    let target: Value = serde_json::from_str(received)?;
    assert_eq!(visible["fact_nodes"][2]["text"], target["C"]);
    let article = validate(&all, &terms, &nodes, &article())?;
    assert!(article[0].text.contains(target["C"].as_str().unwrap()));
    // The presentation cannot borrow a global or wrong-revision source.
    let body = format!(
        "{}。{}。{}",
        ORIGINAL_SOURCES[1],
        ORIGINAL_SOURCES[3],
        target["C"].as_str().unwrap()
    );
    assert!(super::modal_provenance_tests::ordinary(
        &all,
        &[0, 1, 2, 3, 4],
        &body,
        Some(&[1, 2, 3, 4])
    )
    .unwrap_err()
    .is::<super::source_coverage::Missing>());
    let mut current = all.clone();
    current.segments[4].text = ORIGINAL_SOURCES[4].replacen("REMEMBER THAT ", "", 1);
    let context = retrieval::Context {
        segments: current.segments.clone(),
        cursor: 5,
        keywords: Vec::new(),
    };
    let (_, input) = writer::prompt_mode(&current, &context, &json!({}), "zh", false);
    let plan = source_language::Plan::new(&input, &current.segments, true, &terms)?.unwrap();
    let rejected = plan
        .prepare(received, &current.segments, &terms)
        .and_then(|prepared| {
            fact_nodes::Plan::from_prepared(&prepared, &input, &current, &terms, true)
        });
    assert!(
        rejected.is_err(),
        "unspoken REMEMBER cannot be inferred from another source"
    );
    Ok(())
}

#[test]
fn same_language_original_can_supply_a_node_but_unknown_sources_stay_visible() -> Result<()> {
    let source = snapshot(&[MUSIC]);
    let terms = Terms::new(&[], &[]);
    let context = retrieval::Context {
        segments: source.segments.clone(),
        cursor: 1,
        keywords: Vec::new(),
    };
    let (_, input) = writer::prompt_mode(&source, &context, &json!({}), "zh", false);
    let nodes = fact_nodes::Plan::from_original(&input, &source, &terms, true)?;
    validate(
        &source,
        &terms,
        &nodes,
        &json!({"paragraphs":[{"parts":[{"f":1}]}]}).to_string(),
    )?;
    let unknown = snapshot(&[ORIGINAL_SOURCES[0]]);
    let context = retrieval::Context {
        segments: unknown.segments.clone(),
        cursor: 1,
        keywords: Vec::new(),
    };
    let (_, input) = writer::prompt_mode(&unknown, &context, &json!({}), "zh", false);
    let nodes = fact_nodes::Plan::from_original(&input, &unknown, &terms, true)?;
    let input: Value = serde_json::from_str(nodes.input())?;
    assert_eq!(input["transcript"][0]["text"], ORIGINAL_SOURCES[0]);
    assert!(input["fact_nodes"].as_array().unwrap().is_empty());
    for original in [format!("如果{MUSIC}"), format!("不要把“{MUSIC}”作为事实。")] {
        let unknown = snapshot(&[&original]);
        let context = retrieval::Context {
            segments: unknown.segments.clone(),
            cursor: 1,
            keywords: Vec::new(),
        };
        let (_, input) = writer::prompt_mode(&unknown, &context, &json!({}), "zh", false);
        let nodes = fact_nodes::Plan::from_original(&input, &unknown, &terms, true)?;
        let visible: Value = serde_json::from_str(nodes.input())?;
        assert!(visible["fact_nodes"].as_array().unwrap().is_empty());
        assert_eq!(visible["transcript"][0]["text"], original);
    }
    assert!(nodes
        .render(
            &json!({"paragraphs":[{"parts":[{"f":1}]}]}).to_string(),
            nodes.input(),
            &unknown,
            &terms,
            true
        )
        .is_err());
    let risk = snapshot(&[ORIGINAL_SOURCES[1], ORIGINAL_SOURCES[3]]);
    let unspoken = Terms::new(
        &[],
        &[crate::library::Entry {
            kind: "term".into(),
            title: "UNSPOKEN".into(),
            enabled: true,
            ..Default::default()
        }],
    );
    let context = retrieval::Context {
        segments: risk.segments.clone(),
        cursor: 2,
        keywords: Vec::new(),
    };
    let (_, input) = writer::prompt_mode(&risk, &context, &json!({}), "zh", false);
    let nodes = fact_nodes::Plan::from_original(&input, &risk, &unspoken, true)?;
    let visible: Value = serde_json::from_str(nodes.input())?;
    assert_eq!(visible["fact_nodes"].as_array().unwrap().len(), 2);
    assert!(visible["fact_nodes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|node| !node["text"].as_str().unwrap().contains("UNSPOKEN")));
    for original in [
        format!("如果{}", ORIGINAL_SOURCES[1]),
        format!("不要把“{}”当作事实。", ORIGINAL_SOURCES[1]),
    ] {
        let unknown = snapshot(&[&original]);
        let context = retrieval::Context {
            segments: unknown.segments.clone(),
            cursor: 1,
            keywords: Vec::new(),
        };
        let (_, input) = writer::prompt_mode(&unknown, &context, &json!({}), "zh", false);
        let nodes = fact_nodes::Plan::from_original(&input, &unknown, &terms, true)?;
        let visible: Value = serde_json::from_str(nodes.input())?;
        assert!(visible["fact_nodes"].as_array().unwrap().is_empty());
        assert_eq!(visible["transcript"][0]["text"], original);
    }
    let mut product = snapshot(&["我正在测试OmaScribe的功能。"]);
    product.notes = vec![crate::UserNote {
        id: "n".into(),
        meeting_id: product.meeting.id.clone(),
        kind: "document".into(),
        content: "OmaScribe\nUNSPOKEN".into(),
        anchor_ms: 0,
        updated_at: String::new(),
    }];
    let product_terms = Terms::new(&product.notes, &[]);
    let context = retrieval::Context {
        segments: product.segments.clone(),
        cursor: 1,
        keywords: Vec::new(),
    };
    let (_, input) = writer::prompt_mode(&product, &context, &json!({}), "zh", false);
    let nodes = fact_nodes::Plan::from_original(&input, &product, &product_terms, true)?;
    let visible: Value = serde_json::from_str(nodes.input())?;
    assert_eq!(visible["names"], json!([{"id":1,"text":"OmaScribe"}]));
    validate(
        &product,
        &product_terms,
        &nodes,
        &json!({"paragraphs":[{"parts":[{"t":"我正在测试"},{"n":1},{"t":"的功能。"}]}]})
            .to_string(),
    )?;
    Ok(())
}

#[test]
fn closed_b466_bad_drafts_are_not_automatically_repaired() -> Result<()> {
    let (source, terms, _, _, nodes) = prepared(&[
        ORIGINAL_SOURCES[4],
        ORIGINAL_SOURCES[1],
        ORIGINAL_SOURCES[3],
    ])?;
    let first="[N1]教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性  \n[ N2 ]教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性  \n主舞台的音乐已结束，但节日部分可能继续演奏至深夜";
    let second="音乐在主舞台结束，但节日的某些部分仍将持续演奏至深夜。[N1]教授指出，如果公布嫌疑人的照片等信息，可能存在危害性。[N2]教授也指出，如果公布嫌疑人的照片等信息，可能存在危害性。";
    assert_eq!(
        format!("{:x}", Sha256::digest(first.as_bytes())),
        "cf673999bb98d875aff80e6745f6f04c34bc6d468158a818f94459412d14fe7e"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(second.as_bytes())),
        "f8bba698373c35e145c32c64cf07e2a2d24bfbfc64042f7cb11ab5e5a431e515"
    );
    for raw in [first, second] {
        assert!(nodes
            .render(raw, nodes.input(), &source, &terms, true)
            .is_err());
    }
    Ok(())
}

#[test]
fn node_references_cannot_be_missing_repeated_unknown_or_logically_wrapped() -> Result<()> {
    let (source, terms, _, _, nodes) = prepared(&[
        ORIGINAL_SOURCES[4],
        ORIGINAL_SOURCES[1],
        ORIGINAL_SOURCES[3],
    ])?;
    for mut parts in [
        json!([{"t":"普通内容。"}]),
        json!([{"f":1},{"f":1}]),
        json!([{"f":77}]),
        json!([{"f":1,"t":"改写"}]),
        json!([{"t":"如果这是真的，"},{"f":1}]),
        json!([{"n":1},{"t":"教授指出。"},{"f":1}]),
        json!([{"t":"不要把“"},{"f":1},{"t":"”当作事实。"}]),
        json!([{"f":1},{"t":"的话，就无需复核。"}]),
        json!([{"f":1},{"t":"王"},{"n":1},{"t":RISK}]),
        json!([{"f":1},{"n":2},{"t":"王教授指出记者公布照片似乎有风险。"}]),
        json!([{"f":1},{"n":77},{"t":RISK}]),
    ] {
        parts
            .as_array_mut()
            .unwrap()
            .extend([json!({"f":2}), json!({"f":3})]);
        let raw = json!({"paragraphs":[{"parts":parts}]}).to_string();
        assert!(
            nodes
                .render(&raw, nodes.input(), &source, &terms, true)
                .is_err(),
            "{raw}"
        );
    }
    Ok(())
}

#[test]
fn node_scope_is_exact_and_source_six_name_remains_distinct() -> Result<()> {
    let sixth =
        "大学的潘 MALE FERGUSTON教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性";
    let (source, terms, prepared, input, nodes) = prepared(&[
        ORIGINAL_SOURCES[4],
        ORIGINAL_SOURCES[1],
        ORIGINAL_SOURCES[3],
        sixth,
    ])?;
    let names: Value = serde_json::from_str(nodes.input())?;
    assert_eq!(names["names"][1]["text"], "潘 MALE FERGUSON");
    assert_eq!(names["names"][2]["text"], "潘 MALE FERGUSTON");
    let raw = json!({"paragraphs":[{"parts":[{"f":1},{"f":2},{"f":3},{"f":4}]}]}).to_string();
    validate(&source, &terms, &nodes, &raw)?;
    let mut wrong: Value = serde_json::from_str(&raw)?;
    wrong["paragraphs"][0]["parts"]
        .as_array_mut()
        .unwrap()
        .extend([json!({"t":"邓迪大学的"}), json!({"n":3}), json!({"t":RISK})]);
    assert!(
        validate(&source, &terms, &nodes, &wrong.to_string()).is_err(),
        "ordinary name nodes cannot borrow source4's institution for source6's near name"
    );
    for field in 0..5 {
        let mut changed = source.clone();
        match field {
            0 => changed.segments[0].revision += 1,
            1 => changed.segments[0].end_ms += 1,
            2 => changed.segments[0].speaker = Some("other".into()),
            3 => changed.segments[3].text = sixth.replace("FERGUSTON", "FERGUSON"),
            _ => changed.meeting.run += 1,
        }
        assert!(nodes
            .render(&raw, nodes.input(), &changed, &terms, true)
            .is_err());
    }
    assert!(nodes
        .render(&raw, &format!("{} ", nodes.input()), &source, &terms, true)
        .is_err());
    assert!(nodes
        .render(&raw, nodes.input(), &source, &terms, false)
        .is_err());
    let changed = Terms::new(
        &[],
        &[crate::library::Entry {
            kind: "term".into(),
            title: "Nova".into(),
            enabled: true,
            ..Default::default()
        }],
    );
    assert!(nodes
        .render(&raw, nodes.input(), &source, &changed, true)
        .is_err());
    assert!(fact_nodes::Plan::from_prepared(&prepared, &input, &source, &changed, true).is_err());
    assert!(fact_nodes::Plan::from_prepared(&prepared, &input, &source, &terms, false).is_err());
    Ok(())
}

fn current_budget_input(texts: &[&str]) -> Result<(Snapshot, Terms, String)> {
    let source = snapshot(texts);
    let terms = Terms::new(&[], &[]);
    let context = retrieval::Context {
        segments: source.segments.clone(),
        cursor: source.segments.last().unwrap().seq,
        keywords: Vec::new(),
    };
    let (_, input) = writer::prompt_mode(&source, &context, &json!({}), "zh", false);
    assert!(
        source_language::Plan::new(&input, &source.segments, true, &terms)?.is_none(),
        "the existing checked active projection needs no translation request"
    );
    Ok((source, terms, input))
}

#[test]
fn original_factory_accepts_writer_omission_of_only_superseded_budget_evidence() -> Result<()> {
    let (source, terms, input) = current_budget_input(&["预算为3000元。", "预算改为5000元。"])?;
    let actual: Value = serde_json::from_str(&input)?;
    assert_eq!(actual["transcript"].as_array().unwrap().len(), 1);
    assert_eq!(actual["transcript"][0]["id"], 2);
    assert_eq!(actual["transcript"][0]["text"], "预算改为5000元。");
    let body = "预算为5000元。";
    super::modal_provenance_tests::ordinary(&source, &[0, 1], body, None)?;
    let nodes = fact_nodes::Plan::from_original(&input, &source, &terms, true)
        .expect("the factory must accept the real writer's validated current-budget projection");
    let article = json!({"paragraphs":[{"parts":[{"t":body}]}]}).to_string();
    validate(&source, &terms, &nodes, &article)?;
    Ok(())
}

#[test]
fn original_factory_accepts_the_retained_pending_clause_of_a_mixed_budget_source() -> Result<()> {
    let old = "预算为3000元，录音仍未保存。";
    let (source, terms, input) = current_budget_input(&[old, "预算改为5000元。"])?;
    let actual: Value = serde_json::from_str(&input)?;
    let retained = actual["transcript"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == 1)
        .unwrap();
    assert!(retained["text"].as_str().unwrap().contains("录音仍未保存"));
    assert!(!retained["text"].as_str().unwrap().contains("3000"));
    assert_ne!(retained["text"], old);
    let body = "预算为5000元，录音仍未保存。";
    super::modal_provenance_tests::ordinary(&source, &[0, 1], body, None)?;
    let nodes=fact_nodes::Plan::from_original(&input,&source,&terms,true)
        .expect("a checked projection retains the unrelated pending clause without becoming a whole-source fact proof");
    assert!(serde_json::from_str::<Value>(nodes.input())?["fact_nodes"]
        .as_array()
        .unwrap()
        .is_empty());
    validate(
        &source,
        &terms,
        &nodes,
        &json!({"paragraphs":[{"parts":[{"t":body}]}]}).to_string(),
    )?;
    Ok(())
}

#[test]
fn active_projection_does_not_authorize_arbitrary_edits_restoration_or_missing_unknown_content(
) -> Result<()> {
    let (source, terms, input) =
        current_budget_input(&["预算为3000元。", "预算改为5000元。", "录音仍未保存。"])?;
    let actual: Value = serde_json::from_str(&input)?;
    let mut arbitrary = actual.clone();
    arbitrary["transcript"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["id"] == 2)
        .unwrap()["text"] = "预算改为6000元。".into();
    let mut restored = actual.clone();
    restored["transcript"]
        .as_array_mut()
        .unwrap()
        .insert(0, json!({"id":1,"text":"预算为3000元。"}));
    let mut missing = actual;
    missing["transcript"]
        .as_array_mut()
        .unwrap()
        .retain(|entry| entry["id"] != 3);
    let mut wrongly_admitted = Vec::new();
    for (name, value) in [
        ("arbitrary 6000", arbitrary),
        ("restored retired 3000", restored),
        ("missing active unknown", missing),
    ] {
        let input = value.to_string();
        assert!(
            source_language::Plan::new(&input, &source.segments, true, &terms).is_err(),
            "the old source-language projection rejects {name}"
        );
        if fact_nodes::Plan::from_original(&input, &source, &terms, true).is_ok() {
            wrongly_admitted.push(name);
        }
    }
    let (mixed, terms, input) =
        current_budget_input(&["预算为3000元，录音仍未保存。", "预算改为5000元。"])?;
    let mut missing: Value = serde_json::from_str(&input)?;
    missing["transcript"]
        .as_array_mut()
        .unwrap()
        .retain(|entry| entry["id"] != 1);
    assert!(
        source_language::Plan::new(&missing.to_string(), &mixed.segments, true, &terms).is_err()
    );
    if fact_nodes::Plan::from_original(&missing.to_string(), &mixed, &terms, true).is_ok() {
        wrongly_admitted.push("missing retained mixed clause");
    }
    assert!(
        wrongly_admitted.is_empty(),
        "the factory must share admission, not merely reuse raw source IDs: {wrongly_admitted:?}"
    );
    Ok(())
}

#[test]
fn an_active_photo_fragment_does_not_become_a_whole_original_fact_proof() -> Result<()> {
    let old = format!("预算为3000元，{}", ORIGINAL_SOURCES[1]);
    let (source, terms, input) = current_budget_input(&[&old, "预算改为5000元。"])?;
    assert!(
        super::attribution_roles::original_photo_report(&old, &terms)?.is_some(),
        "the complete raw statement includes its original budget prefix"
    );
    let actual: Value = serde_json::from_str(&input)?;
    let fragment = actual["transcript"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == 1)
        .unwrap()["text"]
        .as_str()
        .unwrap();
    assert_ne!(fragment, old);
    assert!(!fragment.contains("3000"));
    let nodes = fact_nodes::Plan::from_original(&input, &source, &terms, true)?;
    let visible: Value = serde_json::from_str(nodes.input())?;
    assert!(
        visible["fact_nodes"].as_array().unwrap().is_empty(),
        "active admission does not replace the raw whole-source predicate proof"
    );
    assert_eq!(visible["transcript"][0]["text"], fragment);
    Ok(())
}

#[test]
fn prepared_current_music_possible_completion_survives_the_ordinary_final_gate() -> Result<()> {
    // Exact already received wording from B; only its runtime source binding
    // is now the complete current source5, rather than typo-bearing source3.
    let received =
        include_str!("../../../../tests/fixtures/received-source-preparation-2026-10-30.json");
    assert_eq!(
        format!("{:x}", Sha256::digest(received.as_bytes())),
        "c0245e7b88cbe230e3f851160b7ffb4f370d06d96948d8c26dbbd111103ac7e5"
    );
    let mut targets: Value = serde_json::from_str(received)?;
    let complete = targets["B"].as_str().unwrap().to_owned();
    assert!(complete.contains("可能已经完成"));
    assert_eq!(complete.matches("可能").count(), 2);
    targets["C"] = complete.clone().into();
    let source = snapshot(&ORIGINAL_SOURCES);
    assert_eq!(source.segments[4].text, ORIGINAL_SOURCES[4]);
    assert_eq!(source.segments[4].revision, 1);
    let terms = Terms::new(&[], &[]).with_actor_spellings(
        source.segments.iter().map(|s| s.text.as_str()),
        std::iter::empty(),
    );
    let context = retrieval::Context {
        segments: source.segments.clone(),
        cursor: 5,
        keywords: Vec::new(),
    };
    let (_, input) = writer::prompt_mode(&source, &context, &json!({}), "zh", false);
    let translation = source_language::Plan::new(&input, &source.segments, true, &terms)?.unwrap();
    let prepared=translation.prepare(&targets.to_string(),&source.segments,&terms)
        .expect("the genuine source-language preparation accepts this locally qualified past completion");
    let nodes = fact_nodes::Plan::from_prepared(&prepared, &input, &source, &terms, true)?;
    let visible: Value = serde_json::from_str(nodes.input())?;
    assert_eq!(visible["fact_nodes"][2]["source_entry"], 5);
    assert_eq!(visible["fact_nodes"][2]["text"], complete);
    // Ordinary n/t remains ordinary, even beside an immutable fact node.
    let raw =
        json!({"paragraphs":[{"parts":[{"f":1},{"f":2},{"f":3},{"n":1},{"t":RISK}]}]}).to_string();
    let rendered = nodes.render(&raw, nodes.input(), &source, &terms, true)?;
    let composed = rendered.composition();
    assert!(composed.input().is_none());
    assert!(composed.copies().is_empty());
    assert!(composed.text().contains(&complete));
    assert_eq!(composed.text().matches("可能").count(), 2);
    let mut wrong_revision = source.clone();
    wrong_revision.segments[4].revision += 1;
    assert!(nodes
        .render(&raw, nodes.input(), &wrong_revision, &terms, true)
        .is_err());
    let sections=validate(&source,&terms,&nodes,&raw)
        .expect("a complete current-source prepared MAY clause must survive the ordinary final checks without granting n/t copy authority");
    assert!(sections
        .iter()
        .any(|section| section.text.contains(&complete)
            && section
                .sources
                .iter()
                .any(|quote| quote.segment_id == source.segments[4].id
                    && quote.revision == 1
                    && quote.quote == ORIGINAL_SOURCES[4])));
    Ok(())
}

#[test]
fn sealed_completion_permits_cover_only_current_f_words_not_ordinary_statuses() -> Result<()> {
    let mut targets: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/received-source-preparation-2026-10-30.json"
    ))?;
    targets["C"] = targets["B"].clone();
    let source = snapshot(&ORIGINAL_SOURCES);
    let terms = Terms::new(&[], &[]).with_actor_spellings(
        source.segments.iter().map(|s| s.text.as_str()),
        std::iter::empty(),
    );
    let context = retrieval::Context {
        segments: source.segments.clone(),
        cursor: 5,
        keywords: Vec::new(),
    };
    let (_, input) = writer::prompt_mode(&source, &context, &json!({}), "zh", false);
    let translation = source_language::Plan::new(&input, &source.segments, true, &terms)?.unwrap();
    let prepared = translation.prepare(&targets.to_string(), &source.segments, &terms)?;
    let nodes = fact_nodes::Plan::from_prepared(&prepared, &input, &source, &terms, true)?;
    let rendered = nodes.render(&article(), nodes.input(), &source, &terms, true)?;
    let composed = rendered.composition();
    assert!(composed.input().is_none() && composed.copies().is_empty());
    let body = composed.text();
    let at = body.find("已经完成").unwrap();
    let range = at..at + "已经完成".len();
    let bridge = source_language::CompletionBridge::from_fact_nodes(
        &nodes, &rendered, &source, &terms, true,
    )?
    .unwrap();
    let segment = &source.segments[4];
    let quote = crate::Source {
        segment_id: segment.id.clone(),
        revision: segment.revision,
        start_ms: segment.start_ms,
        end_ms: segment.end_ms,
        quote: segment.text.clone(),
        speaker_name: None,
    };
    assert!(bridge.permits(body, range.clone(), std::slice::from_ref(&quote)));
    assert!(!bridge.permits(body, at + 1..range.end, std::slice::from_ref(&quote)));
    assert!(!bridge.permits(
        &format!("{body} 已修改。"),
        range.clone(),
        std::slice::from_ref(&quote)
    ));
    for field in 0..6 {
        let mut wrong = quote.clone();
        match field {
            0 => wrong.segment_id = source.segments[0].id.clone(),
            1 => wrong.revision += 1,
            2 => wrong.start_ms += 1,
            3 => wrong.end_ms += 1,
            4 => wrong.quote.push('改'),
            _ => wrong.speaker_name = Some("other".into()),
        }
        assert!(!bridge.permits(body, range.clone(), &[wrong]));
    }
    let mut changed = source.clone();
    changed.segments[4].revision += 1;
    assert!(bridge.check_scope(&changed.segments, &terms, true).is_err());
    assert!(source_language::CompletionBridge::from_fact_nodes(
        &nodes, &rendered, &changed, &terms, true
    )
    .is_err());
    assert!(bridge.check_scope(&source.segments, &terms, false).is_err());
    let changed_terms = Terms::new(
        &[],
        &[crate::library::Entry {
            kind: "term".into(),
            title: "Nova".into(),
            enabled: true,
            ..Default::default()
        }],
    );
    assert!(bridge
        .check_scope(&source.segments, &changed_terms, true)
        .is_err());
    for status in ["任务已完成。", "已告知团队。"] {
        let raw =
            json!({"paragraphs":[{"parts":[{"f":1},{"f":2},{"f":3},{"t":status}]}]}).to_string();
        let rendered = nodes.render(&raw, nodes.input(), &source, &terms, true)?;
        let composed = rendered.composition();
        assert!(composed.input().is_none() && composed.copies().is_empty());
        let bridge = source_language::CompletionBridge::from_fact_nodes(
            &nodes, &rendered, &source, &terms, true,
        )?
        .unwrap();
        let at = composed
            .text()
            .rfind(if status.contains("已完成") {
                "已完成"
            } else {
                "已告知"
            })
            .unwrap();
        assert!(!bridge.permits(
            composed.text(),
            at..at + "已完成".len(),
            std::slice::from_ref(&quote)
        ));
        assert!(
            validate(&source, &terms, &nodes, &raw).is_err(),
            "ordinary text cannot inherit a qualified fact's completion permit: {status}"
        );
    }
    Ok(())
}

#[test]
fn a_person_first_seen_as_a_caption_name_keeps_human_name_boundaries() -> Result<()> {
    let mut source = snapshot(&[
        "ALICE工具仍待测试。",
        "北岭大学的 ALICE教授指出材料仍需核对。",
    ]);
    source.notes = vec![crate::UserNote {
        id: "names".into(),
        meeting_id: source.meeting.id.clone(),
        kind: "document".into(),
        content: "ALICE\nUNSPOKEN".into(),
        anchor_ms: 0,
        updated_at: String::new(),
    }];
    let terms = Terms::new(&source.notes, &[]);
    let context = retrieval::Context {
        segments: source.segments.clone(),
        cursor: 2,
        keywords: Vec::new(),
    };
    let (_, input) = writer::prompt_mode(&source, &context, &json!({}), "zh", false);
    let nodes = fact_nodes::Plan::from_original(&input, &source, &terms, true)?;
    assert_eq!(
        serde_json::from_str::<Value>(nodes.input())?["names"],
        json!([{"id":1,"text":"ALICE"}])
    );
    let valid=json!({"paragraphs":[{"parts":[{"t":"北岭大学的 "},{"n":1},{"t":" \t教授指出材料仍需核对。"}]}]}).to_string();
    let rendered = nodes.render(&valid, nodes.input(), &source, &terms, true)?;
    let composed = rendered.composition();
    assert!(composed.text().contains("ALICE教授"));
    assert!(composed.input().is_none() && composed.copies().is_empty());
    for parts in [
        json!([{"t":"王"},{"n":1},{"t":"教授指出材料仍需核对。"}]),
        json!([{"n":1},{"t":"王教授指出材料仍需核对。"}]),
    ] {
        let raw = json!({"paragraphs":[{"parts":parts}]}).to_string();
        assert!(
            nodes
                .render(&raw, nodes.input(), &source, &terms, true)
                .is_err(),
            "a prior caption occurrence cannot relax the observed person's boundary: {raw}"
        );
    }
    Ok(())
}
