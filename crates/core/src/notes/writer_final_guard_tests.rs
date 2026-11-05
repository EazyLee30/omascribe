use super::*;
use crate::{Meeting, Segment, Snapshot, Source, Suggestion};

const SOURCE: &str = "北岭大学的 LENA FARO森教授指出报告仍未确认。";

fn fixture_snapshot(texts: &[&str]) -> Snapshot {
    Snapshot {
        meeting: Meeting {
            id: "m".into(),
            title: "Final writer name guard".into(),
            created_at: String::new(),
            status: "ready".into(),
            duration_ms: texts.len() as u64 * 1000,
            run: 1,
            notes_cursor: 0,
            auto_notes: false,
            diarization: false,
            diarization_cursor: 0,
            speaker_names: Default::default(),
            notes_fingerprint: String::new(),
        },
        segments: texts
            .iter()
            .enumerate()
            .map(|(i, text)| Segment {
                id: format!("m:1:{}", i + 1),
                meeting_id: "m".into(),
                run: 1,
                seq: i as u32 + 1,
                start_ms: i as u64 * 1000,
                end_ms: (i as u64 + 1) * 1000,
                text: (*text).into(),
                revision: 1,
                speaker: None,
                speaker_manual: false,
            })
            .collect(),
        notes: Vec::new(),
        suggestions: Vec::new(),
        runtime: Default::default(),
        translation_mode: "off".into(),
        translations: Default::default(),
    }
}

fn terms(snapshot: &Snapshot) -> crate::terminology::Terms {
    crate::terminology::Terms::new(&snapshot.notes, &[]).with_actor_spellings(
        snapshot.segments.iter().map(|s| s.text.as_str()),
        snapshot.meeting.speaker_names.values().map(String::as_str),
    )
}

// Actual ordinary production parsing, early spelling check, document folding
// and final validation helpers; no replacement writer or test-only validator.
fn ordinary(snapshot: &Snapshot, selected: &[usize], raw: &str) -> Result<Vec<Suggestion>> {
    let sources = selected
        .iter()
        .map(|i| snapshot.segments[*i].clone())
        .collect::<Vec<_>>();
    let context = retrieval::Context {
        cursor: sources.last().map_or(0, |s| s.seq),
        segments: sources.clone(),
        keywords: Vec::new(),
    };
    let terms = terms(snapshot);
    let (_, input) = writer::prompt(snapshot, &context, &json!({}), "zh");
    let composed = writer::body_with_evidence(raw, &input)?;
    let checked = checked_writer_prose(composed.text(), &sources, &terms)?;
    let document = terms.normalize_draft(&writer::document(
        &checked,
        &sources,
        &snapshot.meeting.speaker_names,
    )?)?;
    validate_writer_document(
        &document,
        snapshot,
        &sources,
        &terms,
        &snapshot.meeting.id,
        true,
        None,
    )
}

fn split_wrong_name(source: &str, raw: &str, folded: &str) -> Result<()> {
    let snapshot = fixture_snapshot(&[source]);
    let terms = terms(&snapshot);
    // The bug is after the early check: presentation removes the line break
    // and exposes a wrong label that the existing validator already rejects.
    assert!(checked_writer_prose(raw, &snapshot.segments, &terms).is_ok());
    let document = writer::document(raw, &snapshot.segments, &snapshot.meeting.speaker_names)?;
    let parsed: Value = serde_json::from_str(&document)?;
    assert_eq!(parsed["sections"][0]["body"], folded);
    assert!(terms.validate_writer_names(source, folded).is_err());
    assert!(
        ordinary(&snapshot, &[0], raw).is_err(),
        "folded wrong name bypassed ordinary final validation: {raw}"
    );
    Ok(())
}

#[test]
fn final_name_guard_rejects_split_alena_after_document_folding() -> Result<()> {
    split_wrong_name(
        SOURCE,
        "北岭大学的 ALENA FARO\n森教授指出报告仍未确认。",
        "北岭大学的 ALENA FARO 森教授指出报告仍未确认。",
    )
}

#[test]
fn final_name_guard_rejects_split_pan_after_document_folding() -> Result<()> {
    split_wrong_name(
        "研究所的潘 MALE FERGUSON森教授指出报告仍未确认。",
        "研究所的 PAN MALE FERGUSON\n森教授指出报告仍未确认。",
        "研究所的 PAN MALE FERGUSON 森教授指出报告仍未确认。",
    )
}

#[test]
fn final_name_guard_keeps_ordinary_wrapping_current_names_and_human_state() -> Result<()> {
    let mut snapshot = fixture_snapshot(&[SOURCE]);
    snapshot.suggestions.push(Suggestion {
        id: "human".into(),
        meeting_id: "m".into(),
        kind: "section".into(),
        focus: "人工确认".into(),
        text: "人工确认的段落应继续保留。".into(),
        status: "accepted".into(),
        stale: false,
        sources: vec![Source {
            segment_id: "m:1:1".into(),
            revision: 1,
            start_ms: 0,
            end_ms: 1000,
            quote: SOURCE.into(),
            speaker_name: None,
        }],
    });
    let before = serde_json::to_value(&snapshot)?;
    for raw in [SOURCE, "北岭大学的 LENA FARO森教授指出\n报告仍未确认。"] {
        let sections = ordinary(&snapshot, &[0], raw)?;
        assert_eq!(sections[0].text, raw.replace('\n', " "));
        assert_eq!(sections[0].sources[0].quote, SOURCE);
    }
    assert_eq!(serde_json::to_value(&snapshot)?, before);
    let selected = fixture_snapshot(&["本次报告仍未确认。", SOURCE]);
    let sections = ordinary(&selected, &[1], SOURCE)?;
    assert_eq!(sections[0].sources[0].segment_id, "m:1:2");
    assert_eq!(sections[0].sources[0].quote, SOURCE);
    Ok(())
}

#[test]
fn final_name_guard_preserves_exact_source_completion_bridge() -> Result<()> {
    let snapshot = fixture_snapshot(&[SOURCE, "The task may have finished."]);
    let sources = &snapshot.segments;
    let terms = terms(&snapshot);
    let context = retrieval::Context {
        cursor: 2,
        segments: sources.clone(),
        keywords: Vec::new(),
    };
    let (_, input) = writer::prompt(&snapshot, &context, &json!({}), "zh");
    let plan = source_language::Plan::new(&input, sources, true, &terms)?
        .context("expected source-language slot")?;
    let prepared = plan.prepare(
        &json!({"A":"任务可能已经完成。"}).to_string(),
        sources,
        &terms,
    )?;
    let applied = prepared.apply(&input, sources)?;
    let value: Value = serde_json::from_str(&applied)?;
    let composed = writer::body_with_evidence(
        &json!({"transcript":value["transcript"]}).to_string(),
        &applied,
    )?;
    let checked = checked_writer_prose(composed.text(), sources, &terms)?;
    assert_eq!(checked, composed.text());
    let bridge = prepared
        .completion_bridge(&composed, &input, sources, &terms, true)?
        .context("expected exact-copy completion bridge")?;
    let document = terms.normalize_draft(&writer::document(
        &checked,
        sources,
        &snapshot.meeting.speaker_names,
    )?)?;
    let sections = validate_writer_document(
        &document,
        &snapshot,
        sources,
        &terms,
        "m",
        true,
        Some(&bridge),
    )?;
    assert!(sections
        .iter()
        .any(|section| section.text.contains("可能已经完成")));
    assert!(sections
        .iter()
        .flat_map(|section| &section.sources)
        .any(|source| source.segment_id == "m:1:2"));
    let mut changed = sources.clone();
    changed[1].revision += 1;
    assert!(validate_writer_document(
        &document,
        &snapshot,
        &changed,
        &terms,
        "m",
        true,
        Some(&bridge)
    )
    .is_err());
    Ok(())
}
