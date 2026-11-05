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
    let draft = terms.normalize_draft(&writer::document(
        &checked,
        &source.segments,
        &source.meeting.speaker_names,
    )?)?;
    super::validate_writer_document(
        &draft,
        source,
        &source.segments,
        terms,
        &source.meeting.id,
        true,
        None,
    )
}

#[test]
fn validated_runtime_clause_is_immutable_and_plain_names_gain_no_copy_proof() -> Result<()> {
    let (source, terms, _, _, nodes) = prepared(&[
        ORIGINAL_SOURCES[4],
        ORIGINAL_SOURCES[1],
        ORIGINAL_SOURCES[3],
    ])?;
    let input: Value = serde_json::from_str(nodes.input())?;
    assert_eq!(input["transcript"].as_array().unwrap().len(), 3);
    assert_eq!(input["fact_nodes"].as_array().unwrap().len(), 3);
    assert_eq!(input["fact_nodes"][0]["source_entry"], 1);
    assert_eq!(input["fact_nodes"][0]["text"], MUSIC);
    assert_eq!(
        input["fact_nodes"][1]["text"],
        format!("{}。", ORIGINAL_SOURCES[1])
    );
    assert_eq!(
        input["fact_nodes"][2]["text"],
        format!("{}。", ORIGINAL_SOURCES[3])
    );
    assert_eq!(
        input["required_source_fields"][0]["range_basis"],
        "original_source"
    );
    assert_eq!(
        input["transcript"][0]["original_source"],
        ORIGINAL_SOURCES[4]
    );
    let sections = validate(&source, &terms, &nodes, &article())?;
    assert_eq!(sections.len(), 1);
    assert!(sections[0].text.contains(MUSIC));
    assert!(sections[0].text.contains("PAMELA FURGO森教授"));
    assert!(sections[0].text.contains("潘 MALE FERGUSON教授"));
    assert!(sections[0]
        .sources
        .iter()
        .any(|q| q.quote == ORIGINAL_SOURCES[4] && q.revision == 1));
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
