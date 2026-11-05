//! Fixed received affiliation counterexample through ordinary final validation.
//! No grammar/model execution or entity alias inference is part of these controls.
use super::modal_provenance_tests::{ordinary, snapshot, ORIGINAL_SOURCES};
use anyhow::Result;
use serde_json::Value;
use sha2::{Digest, Sha256};

// The second raw response from the sole 7842c898 trial. Original receipt:
// artifacts/writer-zh-modal-2026-11-05/q16/
// zh_recorded_mixed_prose_fidelity/candidate/job-1/last-generation.json
const ACTUAL_RAW_BODY: &str = "当迪大学的 PAMELA FURGO森教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性。  \n迪大学的潘 MALE FERGUSON教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性。  \n主舞台音乐可能已结束，但节庆部分可能继续演奏至深夜。  \n主舞台音乐可能已结束，但节庆部分可能继续演奏至深夜。";
const ACTUAL_RAW_SHA256: &str = "6b2d199a121fb636919e87f939a63cd933520d6c642bc72a1b39050dda595216";
const SOURCE6: &str =
    "大学的潘 MALE FERGUSTON教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性";

fn corrected_affiliation() -> String {
    // Change only the observed institution typo; keep the received repeated
    // music paragraph. Passing this control is not full Q16 quality proof.
    ACTUAL_RAW_BODY.replace("迪大学的潘", "邓迪大学的潘")
}

#[test]
fn received_institution_truncation_is_rejected_by_public_ordinary_final_validation() {
    assert_eq!(
        format!("{:x}", Sha256::digest(ACTUAL_RAW_BODY.as_bytes())),
        ACTUAL_RAW_SHA256
    );
    let cases: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/document-cases.json"
    ))
    .unwrap();
    let case = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "zh_recorded_mixed_prose_fidelity")
        .unwrap();
    assert_eq!(
        case["transcript"].as_array().unwrap()[..5],
        ORIGINAL_SOURCES.map(Value::from)
    );
    assert_eq!(case["transcript"][5], SOURCE6);
    let source = snapshot(&ORIGINAL_SOURCES);
    assert!(
        ordinary(&source, &[0, 1, 2, 3, 4], ACTUAL_RAW_BODY, None).is_err(),
        "actual raw response changes 邓迪大学 to 迪大学 yet public final validation accepted it"
    );
}

#[test]
fn observed_ambiguous_affiliations_and_omitting_affiliation_remain_valid() -> Result<()> {
    let source = snapshot(&ORIGINAL_SOURCES);
    let correct = corrected_affiliation();
    assert!(correct.contains("当迪大学的 PAMELA FURGO森"));
    assert!(correct.contains("邓迪大学的潘 MALE FERGUSON"));
    ordinary(&source, &[0, 1, 2, 3, 4], &correct, None)?;
    let omitted = correct.replace("当迪大学的 ", "").replace("邓迪大学的", "");
    ordinary(&source, &[0, 1, 2, 3, 4], &omitted, None)?;
    Ok(())
}

#[test]
fn globally_present_institutions_do_not_authorize_swapped_person_pairs() {
    let source = snapshot(&ORIGINAL_SOURCES);
    let swapped = corrected_affiliation()
        .replace("当迪大学的 PAMELA", "邓迪大学的 PAMELA")
        .replace("邓迪大学的潘", "当迪大学的潘");
    assert!(swapped.contains("当迪大学") && swapped.contains("邓迪大学"));
    assert!(
        ordinary(&source, &[0, 1, 2, 3, 4], &swapped, None).is_err(),
        "independent global institution/person membership cannot authorize new affiliation pairs"
    );
}

#[test]
fn uncited_observed_pair_cannot_authorize_this_paragraph_affiliation() {
    let source = snapshot(&ORIGINAL_SOURCES);
    assert!(
        ordinary(
            &source,
            &[0, 1, 2, 3, 4],
            &corrected_affiliation(),
            Some(&[1, 2, 3, 5])
        )
        .is_err(),
        "the 潘 MALE FERGUSON affiliation comes only from source 4, which this paragraph did not cite"
    );
}

#[test]
fn a_generic_source6_cannot_borrow_an_institution_from_a_near_person_label() -> Result<()> {
    let source = snapshot(&[ORIGINAL_SOURCES[3], SOURCE6]);
    ordinary(&source, &[0, 1], SOURCE6, Some(&[2]))?;
    let guessed = SOURCE6.replace("大学的潘", "邓迪大学的潘");
    assert!(
        ordinary(&source, &[0, 1], &guessed, Some(&[2])).is_err(),
        "FERGUSON source cannot lend 邓迪大学 to the distinct raw FERGUSTON label in source 6"
    );
    Ok(())
}
