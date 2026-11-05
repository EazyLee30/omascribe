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
