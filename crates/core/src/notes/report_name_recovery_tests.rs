//! Fresh production preparation, not a replacement name validator or alias map.
use super::modal_provenance_tests::{ordinary, snapshot, ORIGINAL_SOURCES};
use super::*;
use crate::{Snapshot, Suggestion};
use sha2::{Digest, Sha256};

// First actual d466 candidate response. Its name error used the first retry;
// recovery of that field must not authorize the still-unfaithful music claim.
const RECEIVED: &str = "当迪大学的 PAMELA FURGO 森教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性。邓迪大学的潘 MALE FERGUSON 教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性。音乐在主舞台已经结束，但 festival 中可能有部分区域将持续播放音乐直到深夜。";
const MUSIC: &str = "音乐在主舞台可能已经结束，但嘉年华仍可能有部分区域持续演奏至深夜。";

fn terms(snapshot: &Snapshot) -> crate::terminology::Terms {
    crate::terminology::Terms::new(&snapshot.notes, &[]).with_actor_spellings(
        snapshot.segments.iter().map(|s| s.text.as_str()),
        snapshot.meeting.speaker_names.values().map(String::as_str),
    )
}

fn validate(snapshot: &Snapshot, document: &str) -> Result<Vec<Suggestion>> {
    validate_writer_document(
        document,
        snapshot,
        &snapshot.segments,
        &terms(snapshot),
        &snapshot.meeting.id,
        true,
        None,
    )
}

#[test]
fn received_name_field_is_restored_before_checking_its_actual_music_failure() -> Result<()> {
    assert_eq!(
        format!("{:x}", Sha256::digest(RECEIVED.as_bytes())),
        "3dee5760f2c29ed3258d65a298657f1e9dc358b9b98f9d0670f4bf5c8d3deff0"
    );
    let snapshot = snapshot(&ORIGINAL_SOURCES);
    assert!(ordinary(&snapshot, &[0, 1, 2, 3, 4], RECEIVED, None).is_err());
    let recovered = prepare_generated_writer_document(
        RECEIVED,
        &snapshot,
        &snapshot.segments,
        &terms(&snapshot),
    )?;
    assert_eq!(recovered.edits.len(), 1);
    assert_eq!(recovered.edits[0].field, "person");
    assert_eq!(recovered.edits[0].source_id, snapshot.segments[1].id);
    assert_eq!(recovered.body, RECEIVED.replace("FURGO 森", "FURGO森"));
    assert!(validate(&snapshot, &recovered.draft).is_err());
    Ok(())
}

#[test]
fn complete_current_reports_recover_two_name_fields_without_changing_claims_or_quotes() -> Result<()>
{
    let snapshot = snapshot(&ORIGINAL_SOURCES);
    let before = serde_json::to_value(&snapshot)?;
    let raw = format!(
        "{}。{}。{}",
        ORIGINAL_SOURCES[1].replace("FURGO森", "FURGO 森"),
        ORIGINAL_SOURCES[3].replace("潘 MALE FERGUSON教授", "PAN MALE FERGUSON 教授"),
        MUSIC,
    );
    assert!(ordinary(&snapshot, &[0, 1, 2, 3, 4], &raw, None).is_err());
    let recovered =
        prepare_generated_writer_document(&raw, &snapshot, &snapshot.segments, &terms(&snapshot))?;
    assert_eq!(recovered.edits.len(), 2);
    assert!(recovered.edits.iter().all(|edit| edit.field == "person"));
    let expected = format!(
        "{}。{}。{}",
        ORIGINAL_SOURCES[1], ORIGINAL_SOURCES[3], MUSIC
    );
    assert_eq!(recovered.body, expected);
    let sections = validate(&snapshot, &recovered.draft)?;
    assert_eq!(sections[0].text, expected);
    assert!(sections[0].sources.iter().all(|q| snapshot
        .segments
        .iter()
        .any(|s| q.segment_id == s.id && q.revision == s.revision && q.quote == s.text)));
    assert_eq!(serde_json::to_value(&snapshot)?, before);
    Ok(())
}

#[test]
fn repairs_of_different_fields_are_planned_from_original_bytes_without_cascading() -> Result<()> {
    let snapshot = snapshot(&ORIGINAL_SOURCES);
    let raw = format!(
        "{}。{}。{}",
        ORIGINAL_SOURCES[1].replace("FURGO森", "FURGO 森"),
        ORIGINAL_SOURCES[3].replace("邓迪大学", "迪大学"),
        MUSIC,
    );
    let recovered =
        prepare_generated_writer_document(&raw, &snapshot, &snapshot.segments, &terms(&snapshot))?;
    assert_eq!(recovered.edits.len(), 2);
    assert!(recovered.edits.iter().any(|e| e.field == "institution"));
    assert!(recovered.edits.iter().any(|e| e.field == "person"));
    validate(&snapshot, &recovered.draft)?;
    let both_wrong = ORIGINAL_SOURCES[3]
        .replace("邓迪大学", "迪大学")
        .replace("潘 MALE FERGUSON", "PAN MALE FERGUSON");
    assert!(prepare_generated_writer_document(
        &both_wrong,
        &snapshot,
        &snapshot.segments,
        &terms(&snapshot),
    )
    .is_err());
    Ok(())
}

#[test]
fn folding_cannot_manufacture_name_recovery_or_borrow_an_uncited_witness() -> Result<()> {
    let snapshot = snapshot(&[ORIGINAL_SOURCES[3]]);
    let raw = ORIGINAL_SOURCES[3].replace("潘 MALE", "PAN\nMALE");
    let document = writer::document(&raw, &snapshot.segments, &Default::default())?;
    let recovered =
        validation::restore_generated_reports(&document, &snapshot, &snapshot.segments, &raw)?;
    assert!(recovered.edits.is_empty());
    assert!(prepare_generated_writer_document(
        &raw,
        &snapshot,
        &snapshot.segments,
        &terms(&snapshot),
    )
    .is_err());

    let snapshot = super::modal_provenance_tests::snapshot(&ORIGINAL_SOURCES);
    let raw = format!("{}。", ORIGINAL_SOURCES[3].replace("潘 MALE", "PAN MALE"));
    let mut document: Value = serde_json::from_str(&writer::document(
        &raw,
        &snapshot.segments,
        &Default::default(),
    )?)?;
    document["sections"][0]["sources"] = json!([1, 2, 3, 5]);
    let recovered = validation::restore_generated_reports(
        &document.to_string(),
        &snapshot,
        &snapshot.segments,
        &raw,
    )?;
    assert!(recovered.edits.is_empty());
    assert!(validate(&snapshot, &recovered.draft).is_err());
    // A different original string is not authority to repair this document.
    document["sections"][0]["sources"] = json!([1, 2, 3, 4, 5]);
    let recovered = validation::restore_generated_reports(
        &document.to_string(),
        &snapshot,
        &snapshot.segments,
        "a different original",
    )?;
    assert!(recovered.edits.is_empty());
    Ok(())
}

#[test]
fn folding_cannot_authorize_institution_recovery_through_a_split_person() -> Result<()> {
    let snapshot = snapshot(&[ORIGINAL_SOURCES[3]]);
    let raw = ORIGINAL_SOURCES[3]
        .replace("邓迪大学", "迪大学")
        .replace("潘 MALE", "潘\nMALE");
    let document = writer::document(&raw, &snapshot.segments, &Default::default())?;
    let recovered =
        validation::restore_generated_reports(&document, &snapshot, &snapshot.segments, &raw)?;
    assert!(
        recovered.edits.is_empty(),
        "a display fold cannot authorize a report field repair"
    );
    let prepared =
        prepare_generated_writer_document(&raw, &snapshot, &snapshot.segments, &terms(&snapshot))?;
    assert!(validate(&snapshot, &prepared.draft).is_err());

    let raw = ORIGINAL_SOURCES[3].replace("邓迪大学", "迪大学");
    let document = writer::document(&raw, &snapshot.segments, &Default::default())?;
    assert_eq!(
        validation::restore_generated_reports(&document, &snapshot, &snapshot.segments, &raw)?
            .edits
            .len(),
        1,
    );
    let mismatched = validation::restore_generated_reports(
        &document,
        &snapshot,
        &snapshot.segments,
        "a different original",
    )?;
    assert!(mismatched.edits.is_empty());
    Ok(())
}
