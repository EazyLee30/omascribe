use super::*;
use crate::{Meeting, Segment, Snapshot, Suggestion, UserNote};

fn snapshot(texts: &[&str]) -> Snapshot {
    Snapshot {
        meeting: Meeting {
            id: "m".into(),
            title: "Confirmation-state control".into(),
            created_at: String::new(),
            status: "ready".into(),
            duration_ms: texts.len() as u64 * 1000,
            run: 1,
            notes_cursor: 0,
            auto_notes: false,
            notes_fingerprint: String::new(),
            diarization: false,
            diarization_cursor: 0,
            speaker_names: Default::default(),
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

// The ordinary generation path's parser, checked prose, document construction,
// normalization and complete final validation, without Engine/store/model work.
// Optional citation indices exercise that same final gate with a paragraph's
// declared original sources, rather than borrowing any selected source.
fn ordinary(
    snapshot: &Snapshot,
    selected: &[usize],
    raw: &str,
    chinese: bool,
    citations: Option<&[usize]>,
) -> Result<Vec<Suggestion>> {
    let sources = selected
        .iter()
        .map(|i| snapshot.segments[*i].clone())
        .collect::<Vec<_>>();
    let context = retrieval::Context {
        cursor: sources.last().map_or(0, |s| s.seq),
        segments: sources.clone(),
        keywords: Vec::new(),
    };
    let terms = crate::terminology::Terms::new(&snapshot.notes, &[]);
    let (_, input) = writer::prompt(
        snapshot,
        &context,
        &json!({}),
        if chinese { "zh" } else { "en" },
    );
    let composition = writer::body_with_evidence(raw, &input)?;
    let body = checked_writer_prose(composition.text(), &sources, &terms)?;
    let mut document: Value = serde_json::from_str(&writer::document(
        &body,
        &sources,
        &snapshot.meeting.speaker_names,
    )?)?;
    if let Some(citations) = citations {
        document["sections"][0]["sources"] = json!(citations);
    }
    let document = terms.normalize_draft(&document.to_string())?;
    validate_writer_document(
        &document,
        snapshot,
        &sources,
        &terms,
        &snapshot.meeting.id,
        chinese,
        None,
    )
}

#[test]
fn ordinary_writer_rejects_pending_confirmation_with_source_spelling_unchanged() {
    let cases = [
        (
            "北岭大学的 LENA FARO森教授指出报告仍未确认。",
            "北岭大学的 LENA FARO森教授指出报告已经确认。",
        ),
        ("验收报告仍未确认。", "验收报告已经确认。"),
        ("供应商回复尚未确认。", "供应商回复现已确认。"),
        ("数据清单还没有确认。", "数据清单已确认。"),
    ];
    let mut accepted = Vec::new();
    for (source, changed) in cases {
        let snapshot = snapshot(&[source]);
        assert!(
            ordinary(&snapshot, &[0], source, true, None).is_ok(),
            "{source}"
        );
        if ordinary(&snapshot, &[0], changed, true, None).is_ok() {
            accepted.push((source, changed));
        }
    }
    assert!(
        accepted.is_empty(),
        "Unsupported confirmation accepted: {accepted:?}"
    );
}

#[test]
fn ordinary_writer_rejects_english_pending_confirmation() {
    let cases = [
        (
            "The supplier response is not yet confirmed.",
            "The supplier response is already confirmed.",
        ),
        (
            "The acceptance report is still unconfirmed.",
            "The acceptance report has been confirmed.",
        ),
    ];
    let mut accepted = Vec::new();
    for (source, changed) in cases {
        let snapshot = snapshot(&[source]);
        assert!(
            ordinary(&snapshot, &[0], source, false, None).is_ok(),
            "{source}"
        );
        if ordinary(&snapshot, &[0], changed, false, None).is_ok() {
            accepted.push((source, changed));
        }
    }
    assert!(
        accepted.is_empty(),
        "Unsupported confirmation accepted: {accepted:?}"
    );
}

#[test]
fn ordinary_writer_allows_explicit_later_confirmation_of_the_same_object() {
    for (sources, body, chinese) in [
        (
            ["验收报告仍未确认。", "验收报告现已确认。"],
            "验收报告已经确认。",
            true,
        ),
        (
            [
                "The supplier response is not yet confirmed.",
                "The supplier response is now confirmed.",
            ],
            "The supplier response is confirmed.",
            false,
        ),
    ] {
        assert!(
            ordinary(&snapshot(&sources), &[0, 1], body, chinese, None).is_ok(),
            "{body}"
        );
    }
}

#[test]
fn ordinary_writer_keeps_confirmation_polarity_bound_to_its_own_object() {
    for (sources, correct, changed, chinese) in [
        (
            ["验收报告仍未确认。", "会议地点已经确认。"],
            "验收报告仍未确认，会议地点已经确认。",
            "验收报告已经确认，会议地点仍未确认。",
            true,
        ),
        (
            [
                "The acceptance report is still unconfirmed.",
                "The meeting location is confirmed.",
            ],
            "The acceptance report is still unconfirmed. The meeting location is confirmed.",
            "The acceptance report is confirmed. The meeting location is still unconfirmed.",
            false,
        ),
    ] {
        let snapshot = snapshot(&sources);
        assert!(
            ordinary(&snapshot, &[0, 1], correct, chinese, None).is_ok(),
            "{correct}"
        );
        assert!(
            ordinary(&snapshot, &[0, 1], changed, chinese, None).is_err(),
            "{changed}"
        );
    }
}

#[test]
fn ordinary_writer_cannot_use_quoted_or_conditional_confirmation_as_an_update() {
    let mut accepted = Vec::new();
    for (pending, nonassertive, changed, chinese) in [
        (
            "验收报告仍未确认。",
            "如果验收报告已经确认，就发送链接。",
            "验收报告已经确认。",
            true,
        ),
        (
            "验收报告仍未确认。",
            "请不要把“验收报告已经确认”当作事实。",
            "验收报告已经确认。",
            true,
        ),
        (
            "The supplier response is not yet confirmed.",
            "If the supplier response is confirmed, send the link.",
            "The supplier response is confirmed.",
            false,
        ),
        (
            "The supplier response is not yet confirmed.",
            "Do not treat \"the supplier response is confirmed\" as a fact.",
            "The supplier response is confirmed.",
            false,
        ),
    ] {
        let snapshot = snapshot(&[pending, nonassertive]);
        let exact = format!("{pending} {nonassertive}");
        assert!(
            ordinary(&snapshot, &[0, 1], &exact, chinese, None).is_ok(),
            "{exact}"
        );
        if ordinary(&snapshot, &[0, 1], changed, chinese, None).is_ok() {
            accepted.push(nonassertive);
        }
    }
    assert!(
        accepted.is_empty(),
        "Nonassertive witnesses accepted: {accepted:?}"
    );
}

#[test]
fn ordinary_writer_confirmation_witness_must_be_in_the_paragraphs_own_citations() {
    let snapshot = snapshot(&["验收报告仍未确认。", "验收报告现已确认。"]);
    let body = "验收报告已经确认。";
    assert!(ordinary(&snapshot, &[0, 1], body, true, Some(&[2])).is_ok());
    assert!(ordinary(&snapshot, &[0, 1], body, true, Some(&[1])).is_err());
}

#[test]
fn ordinary_writer_unrelated_confirmation_does_not_cover_a_pending_report() {
    let snapshot = snapshot(&["验收报告仍未确认。", "会议地点已经确认。"]);
    assert!(ordinary(&snapshot, &[0, 1], "会议地点已经确认。", true, Some(&[2])).is_ok());
    assert!(ordinary(&snapshot, &[0, 1], "验收报告已经确认。", true, Some(&[2])).is_err());
}

#[test]
fn ordinary_writer_already_confirmed_sources_are_not_rejected() -> Result<()> {
    for (source, chinese) in [
        ("北岭大学的 LENA FARO森教授指出报告已经确认。", true),
        ("验收报告现已确认。", true),
        ("The supplier response has been confirmed.", false),
    ] {
        let snapshot = snapshot(&[source]);
        let before = serde_json::to_value(&snapshot)?;
        let result = ordinary(&snapshot, &[0], source, chinese, None)?;
        assert_eq!(result[0].text, source);
        assert_eq!(result[0].sources[0].quote, source);
        assert_eq!(result[0].sources[0].revision, 1);
        assert_eq!(serde_json::to_value(snapshot)?, before);
    }
    Ok(())
}

#[test]
fn ordinary_writer_note_only_or_unselected_confirmation_is_not_a_witness() {
    let mut snapshot = snapshot(&["验收报告仍未确认。", "验收报告已经确认。"]);
    snapshot.notes.push(UserNote {
        id: "note".into(),
        meeting_id: "m".into(),
        content: "验收报告已经确认。".into(),
        kind: "document".into(),
        anchor_ms: 0,
        updated_at: String::new(),
    });
    assert!(ordinary(&snapshot, &[0], "验收报告已经确认。", true, None).is_err());
}

#[test]
fn ordinary_writer_uses_latest_object_state_without_resurrecting_old_confirmation() {
    let snapshot = snapshot(&[
        "验收报告仍未确认。",
        "验收报告现已确认。",
        "验收报告仍未确认。",
    ]);
    assert!(ordinary(&snapshot, &[0, 1, 2], "验收报告仍未确认。", true, None).is_ok());
    assert!(ordinary(&snapshot, &[0, 1, 2], "验收报告已经确认。", true, None).is_err());
    assert!(ordinary(
        &snapshot,
        &[0, 1, 2],
        "验收报告已经确认。",
        true,
        Some(&[2])
    )
    .is_err());
}

#[test]
fn ordinary_writer_confirmation_updates_require_newer_sequence_and_same_scope() {
    let mut snapshot = snapshot(&["验收报告仍未确认。", "验收报告现已确认。"]);
    snapshot.segments[0].seq = 5;
    assert!(ordinary(&snapshot, &[0, 1], "验收报告已经确认。", true, None).is_err());
    assert!(ordinary(&snapshot, &[1, 0], "验收报告已经确认。", true, None).is_err());
    snapshot.segments[0].seq = 1;
    snapshot.segments[1].run = 2;
    assert!(ordinary(&snapshot, &[0, 1], "验收报告已经确认。", true, None).is_err());
    snapshot.segments[1].run = 1;
    snapshot.segments[1].meeting_id = "another-meeting".into();
    assert!(ordinary(&snapshot, &[0, 1], "验收报告已经确认。", true, None).is_err());
}

#[test]
fn ordinary_writer_confirmation_does_not_merge_longer_embedded_object_names() {
    let chinese = snapshot(&["报告仍未确认。", "验收报告已经确认。"]);
    assert!(ordinary(
        &chinese,
        &[0, 1],
        "报告仍未确认，验收报告已经确认。",
        true,
        None
    )
    .is_ok());
    assert!(ordinary(&chinese, &[0, 1], "报告已经确认。", true, None).is_err());
    let english = snapshot(&[
        "The report is unconfirmed.",
        "The acceptance report is confirmed.",
    ]);
    assert!(ordinary(
        &english,
        &[0, 1],
        "The report is unconfirmed. The acceptance report is confirmed.",
        false,
        None
    )
    .is_ok());
    assert!(ordinary(&english, &[0, 1], "The report is confirmed.", false, None).is_err());
    let reported_object = snapshot(&["验收说明报告未确认。", "事故说明报告已确认。"]);
    assert!(ordinary(
        &reported_object,
        &[0, 1],
        "验收说明报告未确认，事故说明报告已确认。",
        true,
        None
    )
    .is_ok());
    assert!(ordinary(
        &reported_object,
        &[0, 1],
        "验收说明报告已确认。",
        true,
        None
    )
    .is_err());
}

#[test]
fn ordinary_writer_confirmation_does_not_promote_cross_line_condition_or_quotation() {
    for update in [
        "如果链接可用；\n验收报告已经确认。",
        "不要相信“链接已经发送。\n验收报告已经确认。”",
    ] {
        let snapshot = snapshot(&["验收报告仍未确认。", update]);
        let exact = format!("验收报告仍未确认。 {update}");
        assert!(
            ordinary(&snapshot, &[0, 1], &exact, true, None).is_ok(),
            "{exact}"
        );
        assert!(
            ordinary(&snapshot, &[0, 1], "验收报告已经确认。", true, None).is_err(),
            "{update}"
        );
    }
}

#[test]
fn confirmation_view_bounds_frames_across_all_selected_sources() {
    let texts = (0..65)
        .map(|i| format!("项目{i}报告未确认。"))
        .collect::<Vec<_>>();
    let refs = texts.iter().map(String::as_str).collect::<Vec<_>>();
    let snapshot = snapshot(&refs);
    let terms = crate::terminology::Terms::new(&[], &[]);
    assert!(confirmation::View::new(&snapshot.segments[..64], "m", &terms).is_ok());
    assert!(confirmation::View::new(&snapshot.segments, "m", &terms).is_err());
}

#[test]
fn ordinary_writer_single_quoted_sentences_cannot_confirm_pending_state() {
    for quoted in [
        "Do not treat 'Earlier example. The supplier response is confirmed. End example.' as fact.",
        "Do not treat 'Earlier example, The supplier response is confirmed, End example' as fact.",
    ] {
        let snapshot = snapshot(&["The supplier response is not yet confirmed.", quoted]);
        assert!(
            ordinary(
                &snapshot,
                &[0, 1],
                "The supplier response is confirmed.",
                false,
                Some(&[2])
            )
            .is_err(),
            "quoted update became a confirmation witness: {quoted}"
        );
    }
}

#[test]
fn ordinary_writer_confirmation_keeps_contractions_outside_quotation_scope() {
    let snapshot = snapshot(&[
        "The supplier response is not yet confirmed.",
        "The supplier response is confirmed, it's ready.",
    ]);
    assert!(ordinary(
        &snapshot,
        &[0, 1],
        "The supplier response is confirmed.",
        false,
        Some(&[2])
    )
    .is_ok());
}
