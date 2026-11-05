use super::*;
use crate::{Meeting, Segment, Snapshot, Suggestion};
use sha2::{Digest, Sha256};

// Actual sole private c4bd53e attempt and committed article, copied unchanged.
// Receipt: artifacts/writer-zh-grammar-2026-11-05/q16/
// zh_recorded_mixed_prose_fidelity/candidate/result.json
// receipt SHA256: b142b13d3c1e66110208e7723f5997056ccb584f391ee4343632806d79417396
// Original q16.json SHA256: 5557341930f3807cc1f3c0dbe9e614e0cdcd09cd5a03880febd6e4fa3c1119e2
// These controls call the PUBLIC ordinary validation path, not that experiment.
const ACTUAL_BODY: &str = "当迪大学的 PAMELA FURGO森教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性。邓迪大学的潘 MALE FERGUSON教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性。音乐在主舞台已经结束，但嘉年华的某些部分仍将持续演奏至深夜。";
const ACTUAL_BODY_SHA256: &str = "055507d4f1538fc37021c90c4263cf1ad8eadf7ca3aee594c9349513c18afedf";
const ORIGINAL_SOURCES: [&str; 5] = [
    "REMEMBER THAT EVEN THOUGH MUSIC ON THE MAIN STAGES MAY HAVE FINISHED THERE MAY BE SECTIONS OF THE FESTIFUL THAT WILL KEEP PLAYING MUSIC UNTIL LAY INTO THE NIGHT",
    "当迪大学的 PAMELA FURGO森教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性",
    "REMEMBER THAT EVEN THOUGH MUSIC ON THE MAIN STAGES MAY HAVE FINISH THERE MAY BE SECTIONS OF THE FESTIFUL THAT WILL KEEP PLAYING MUSIC UNTIL LATE INTO THE NIGHT",
    "邓迪大学的潘 MALE FERGUSON教授指出记者如果公布嫌疑人的照片等信息的话似乎确实存在危害性",
    "REMEMBER THAT EVEN THOUGH MUSIC ON THE MAIN STAGES MAY HAVE FINISHED THERE MAY BE SECTIONS OF THE FESTIVAL THAT WILL KEEP PLAYING MUSIC UNTIL LATE INTO THE NIGHT",
];
const DEFINITE: &str = "音乐在主舞台已经结束，但嘉年华的某些部分仍将持续演奏至深夜。";
const BOTH_MODAL: &str = "音乐在主舞台可能已经结束，但嘉年华仍可能有部分区域持续演奏至深夜。";
const ONLY_END_MODAL: &str = "音乐在主舞台可能已经结束，但嘉年华的某些部分仍将持续演奏至深夜。";
const ONLY_CONTINUATION_MODAL: &str =
    "音乐在主舞台已经结束，但嘉年华仍可能有部分区域持续演奏至深夜。";

fn snapshot(texts: &[&str]) -> Snapshot {
    Snapshot {
        meeting: Meeting {
            id: "m".into(),
            title: "Writer modal provenance".into(),
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

// Same production parser, checked prose, document folding/normalization and
// final name/fact/source/language gates. No Engine, model or replacement gate.
// Explicit citation indices only narrow the final paragraph's declared sources.
fn ordinary(
    snapshot: &Snapshot,
    selected: &[usize],
    raw: &str,
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
    let terms = crate::terminology::Terms::new(&snapshot.notes, &[]).with_actor_spellings(
        snapshot.segments.iter().map(|s| s.text.as_str()),
        snapshot.meeting.speaker_names.values().map(String::as_str),
    );
    let (_, input) = writer::prompt(snapshot, &context, &json!({}), "zh");
    let composed = writer::body_with_evidence(raw, &input)?;
    let checked = checked_writer_prose(composed.text(), &sources, &terms)?;
    let mut document: Value = serde_json::from_str(&writer::document(
        &checked,
        &sources,
        &snapshot.meeting.speaker_names,
    )?)?;
    if let Some(indices) = citations {
        document["sections"][0]["sources"] = json!(indices);
    }
    let document = terms.normalize_draft(&document.to_string())?;
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
fn article(music: &str) -> String {
    format!(
        "{}。{}。{}",
        ORIGINAL_SOURCES[1], ORIGINAL_SOURCES[3], music
    )
}

#[test]
fn ordinary_writer_rejects_the_actual_committed_q16_modal_loss() {
    assert_eq!(
        format!("{:x}", Sha256::digest(ACTUAL_BODY.as_bytes())),
        ACTUAL_BODY_SHA256
    );
    assert_eq!(article(DEFINITE), ACTUAL_BODY);
    let source = snapshot(&ORIGINAL_SOURCES);
    assert!(
        ordinary(&source, &[0, 1, 2, 3, 4], ACTUAL_BODY, None).is_err(),
        "actual received article with both MAY scopes removed passed public final validation"
    );
}

#[test]
fn ordinary_writer_ending_hedge_cannot_discharge_continuation_uncertainty() {
    let source = snapshot(&ORIGINAL_SOURCES);
    assert!(ordinary(&source, &[0, 1, 2, 3, 4], &article(ONLY_END_MODAL), None).is_err());
}

#[test]
fn ordinary_writer_continuation_hedge_cannot_discharge_ending_uncertainty() {
    let source = snapshot(&ORIGINAL_SOURCES);
    assert!(ordinary(
        &source,
        &[0, 1, 2, 3, 4],
        &article(ONLY_CONTINUATION_MODAL),
        None
    )
    .is_err());
}

#[test]
fn ordinary_writer_allows_both_locally_preserved_modal_claims() -> Result<()> {
    let source = snapshot(&ORIGINAL_SOURCES);
    let before = serde_json::to_value(&source)?;
    for music in [
        BOTH_MODAL,
        "音乐在主舞台或许已经结束，但嘉年华也许仍有部分区域持续演奏至深夜。",
    ] {
        let body = article(music);
        let sections = ordinary(&source, &[0, 1, 2, 3, 4], &body, None)?;
        assert_eq!(sections[0].text, body);
        assert!(sections[0].sources.iter().all(|q| source
            .segments
            .iter()
            .any(|s| s.id == q.segment_id && s.text == q.quote && s.revision == q.revision)));
    }
    assert_eq!(serde_json::to_value(source)?, before);
    Ok(())
}

#[test]
fn ordinary_writer_cannot_borrow_seems_from_the_other_risk_claim() {
    let source = snapshot(&[ORIGINAL_SOURCES[4], ORIGINAL_SOURCES[1]]);
    let body = format!("{}。{}", ORIGINAL_SOURCES[1], DEFINITE);
    assert!(ordinary(&source, &[0, 1], &body, None).is_err());
}

#[test]
fn ordinary_writer_allows_explicit_later_current_claims_only_in_their_own_citations() {
    let source = snapshot(&[ORIGINAL_SOURCES[4], DEFINITE]);
    assert!(ordinary(&source, &[0, 1], DEFINITE, Some(&[2])).is_ok());
    assert!(
        ordinary(&source, &[0, 1], DEFINITE, Some(&[1])).is_err(),
        "uncited later assertion cannot authorize definite claims in the old modal source"
    );
}

#[test]
fn ordinary_writer_one_explicit_update_cannot_confirm_the_other_modal_claim() {
    for (update, valid) in [
        ("音乐在主舞台已经结束。", ONLY_CONTINUATION_MODAL),
        ("嘉年华的某些部分仍将持续演奏至深夜。", ONLY_END_MODAL),
    ] {
        let source = snapshot(&[ORIGINAL_SOURCES[4], update]);
        assert!(
            ordinary(&source, &[0, 1], valid, Some(&[1, 2])).is_ok(),
            "{valid}"
        );
        assert!(
            ordinary(&source, &[0, 1], DEFINITE, Some(&[1, 2])).is_err(),
            "{update}"
        );
    }
}

#[test]
fn ordinary_writer_unknown_or_different_objects_cannot_authorize_music_certainty() {
    for unrelated in [
        "主舞台的音响检查已经结束，嘉年华的安全检查仍将持续至深夜。",
        "The music audit has finished. Some festival inspection teams will keep working until late into the night.",
    ] {
        let source=snapshot(&[ORIGINAL_SOURCES[4],unrelated]);
        assert!(ordinary(&source,&[0,1],DEFINITE,Some(&[1,2])).is_err(),"{unrelated}");
    }
}

#[test]
fn ordinary_writer_conditional_or_quoted_updates_are_not_factual_witnesses() {
    for nonassertive in [
        "如果音乐在主舞台已经结束，嘉年华的某些部分仍将持续演奏至深夜。",
        "不要把“音乐在主舞台已经结束，但嘉年华的某些部分仍将持续演奏至深夜。”当作事实。",
    ] {
        let source = snapshot(&[ORIGINAL_SOURCES[4], nonassertive]);
        assert!(
            ordinary(&source, &[0, 1], DEFINITE, Some(&[1, 2])).is_err(),
            "{nonassertive}"
        );
    }
}
