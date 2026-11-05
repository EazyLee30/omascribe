//! Actual received wrong-owner article through the public ordinary writer path.
//! No private sampling profile, grammar, model call, or replacement oracle.
use super::modal_provenance_tests::{ordinary, snapshot, ORIGINAL_SOURCES};
use anyhow::Result;
use serde_json::Value;
use sha2::{Digest, Sha256};

// Second raw body from the sole d466 official-profile trial, received unchanged:
// artifacts/writer-official-sampling-2026-11-05/q16/
// zh_recorded_mixed_prose_fidelity/candidate/job-1/last-generation.json
// attempts[1].body_draft. The actual producer accepted it and committed 0→5.
const RECEIVED_BODY: &str = "PAMELA FURGO森教授指出，即使主舞台的音乐已经结束，嘉年华仍有部分区域将持续播放音乐直到深夜。邓迪大学的潘 MALE FERGUSON教授指出，如果记者公布嫌疑人的照片等信息，似乎存在危害性。";
const RECEIVED_SHA256: &str = "dd70bcbcc56f6f18c39547854e4594df899590f0bae357771fcbdf9333114dee";
const SOURCE_JSON_SHA256: &str = "738f4885a4c9aa48a2400e532dfbe0813c84f1ef7ed6903b44b9d79aad43721c";
const MUSIC_WITH_BOTH_MAY: &str =
    "音乐在主舞台可能已经结束，但嘉年华仍可能有部分区域持续演奏至深夜。";

#[test]
fn received_article_cannot_attribute_unowned_music_to_a_named_risk_reporter() {
    assert_eq!(RECEIVED_BODY.len(), 251);
    assert_eq!(
        format!("{:x}", Sha256::digest(RECEIVED_BODY.as_bytes())),
        RECEIVED_SHA256
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&ORIGINAL_SOURCES).unwrap())
        ),
        SOURCE_JSON_SHA256
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
        "the received body gives PAMELA the anonymous music claim and loses continuation MAY; globally cited passages do not authorize that report"
    );
}

#[test]
fn faithful_risk_reports_and_unowned_music_keep_both_modal_claims() -> Result<()> {
    let source = snapshot(&ORIGINAL_SOURCES);
    let before = serde_json::to_value(&source)?;
    let body = format!(
        "{}。{}。{}",
        ORIGINAL_SOURCES[1], ORIGINAL_SOURCES[3], MUSIC_WITH_BOTH_MAY
    );
    let sections = ordinary(&source, &[0, 1, 2, 3, 4], &body, None)?;
    assert_eq!(sections[0].text, body);
    assert_eq!(serde_json::to_value(&source)?, before);
    Ok(())
}

#[test]
fn preserving_music_modals_does_not_authorize_a_new_reporter_relationship() {
    let source = snapshot(&ORIGINAL_SOURCES);
    assert!(!source.segments[1].text.contains("音乐"));
    let body = format!(
        "PAMELA FURGO森教授指出，{}{}。",
        MUSIC_WITH_BOTH_MAY, ORIGINAL_SOURCES[3]
    );
    assert!(
        ordinary(&source, &[0, 1, 2, 3, 4], &body, None).is_err(),
        "anonymous music evidence and a separate named photo-risk report cannot be joined into PAMELA reporting music, even when every passage is globally cited"
    );
}

#[test]
fn explicit_named_payload_needs_its_own_current_citation_and_unknown_paraphrases_stay_free(
) -> Result<()> {
    let named = format!("当迪大学的 PAMELA FURGO森教授指出{}", MUSIC_WITH_BOTH_MAY);
    let mut texts = ORIGINAL_SOURCES.to_vec();
    texts.push(&named);
    let source = snapshot(&texts);
    let body = format!(
        "PAMELA FURGO森教授指出，{}{}。{}",
        MUSIC_WITH_BOTH_MAY, ORIGINAL_SOURCES[3], MUSIC_WITH_BOTH_MAY
    );
    // The named report and the independent anonymous music fact have separate
    // own-source obligations. Source5 cannot authorize PAMELA's new report.
    ordinary(&source, &[0, 1, 2, 3, 4, 5], &body, Some(&[4, 5, 6]))?;
    assert!(ordinary(&source, &[0, 1, 2, 3, 4, 5], &body, Some(&[1, 2, 3, 4, 5])).is_err());
    let terms = crate::terminology::Terms::new(&[], &[]);
    let view = super::attribution_roles::ReportedView::new(&source.segments, "m", &terms)?;
    let citation = |segment: &crate::Segment| crate::Source {
        segment_id: segment.id.clone(),
        revision: segment.revision,
        start_ms: segment.start_ms,
        end_ms: segment.end_ms,
        quote: segment.text.clone(),
        speaker_name: None,
    };
    let own = vec![citation(&source.segments[3]), citation(&source.segments[5])];
    view.validate(&body, &own, &terms)?;
    for field in ["revision", "quote", "time"] {
        let mut stale = own.clone();
        match field {
            "revision" => stale[1].revision += 1,
            "quote" => stale[1].quote = ORIGINAL_SOURCES[0].into(),
            "time" => stale[1].start_ms += 1,
            _ => unreachable!(),
        }
        assert!(view.validate(&body, &stale, &terms).is_err(), "{field}");
    }
    let unknown = snapshot(&["当迪大学的 PAMELA FURGO森教授指出材料仍需核对。"]);
    ordinary(
        &unknown,
        &[0],
        "当迪大学的 PAMELA FURGO森教授表示材料仍需复核。",
        None,
    )?;
    Ok(())
}

#[test]
fn qualified_named_reports_do_not_authorize_bare_reports_across_sentence_stops() -> Result<()> {
    let named = format!("当迪大学的 PAMELA FURGO森教授指出{}", MUSIC_WITH_BOTH_MAY);
    let bare = format!(
        "PAMELA FURGO森教授指出，{}{}。{}",
        MUSIC_WITH_BOTH_MAY, ORIGINAL_SOURCES[3], MUSIC_WITH_BOTH_MAY
    );
    for wrapped in [
        format!("如果{named}"),
        format!("“{named}”"),
        format!("Do not treat 'Earlier example. {named} End example.' as fact."),
    ] {
        let mut texts = ORIGINAL_SOURCES.to_vec();
        texts.push(&wrapped);
        let source = snapshot(&texts);
        assert!(
            ordinary(&source, &[0, 1, 2, 3, 4, 5], &bare, None).is_err(),
            "{wrapped}"
        );
    }
    for wrapped in [format!("如果{named}"), format!("“{named}”")] {
        let mut texts = ORIGINAL_SOURCES.to_vec();
        texts.push(&wrapped);
        let source = snapshot(&texts);
        // Keep the copied qualifier/quotation intact; only the independent
        // anonymous sentence and its source5 citation cover the bare fact.
        let copied = format!(
            "{wrapped}。{}。{}",
            ORIGINAL_SOURCES[3], MUSIC_WITH_BOTH_MAY
        );
        ordinary(&source, &[0, 1, 2, 3, 4, 5], &copied, Some(&[4, 5, 6]))?;
    }
    Ok(())
}
