//! Positive recovery of a received failure through the production preparation,
//! final validator and transactional store. The original raw-veto tests remain.
use super::affiliation_provenance_tests::ACTUAL_RAW_BODY;
use super::modal_provenance_tests::{snapshot, ORIGINAL_SOURCES};
use super::*;
use crate::{Segment, Snapshot, Suggestion};
use sha2::{Digest, Sha256};

fn terms(snapshot: &Snapshot) -> crate::terminology::Terms {
    crate::terminology::Terms::new(&snapshot.notes, &[]).with_actor_spellings(
        snapshot.segments.iter().map(|s| s.text.as_str()),
        snapshot.meeting.speaker_names.values().map(String::as_str),
    )
}

fn draft(snapshot: &Snapshot, raw: &str) -> Result<String> {
    let context = retrieval::Context {
        cursor: snapshot.segments.last().map_or(0, |s| s.seq),
        segments: snapshot.segments.clone(),
        keywords: Vec::new(),
    };
    let (_, input) = writer::prompt(snapshot, &context, &json!({}), "zh");
    let composition = writer::body_with_evidence(raw, &input)?;
    let checked = checked_writer_prose(composition.text(), &snapshot.segments, &terms(snapshot))?;
    terms(snapshot).normalize_draft(&writer::document(
        &checked,
        &snapshot.segments,
        &snapshot.meeting.speaker_names,
    )?)
}

fn validate(snapshot: &Snapshot, draft: &str) -> Result<Vec<Suggestion>> {
    validate_writer_document(
        draft,
        snapshot,
        &snapshot.segments,
        &terms(snapshot),
        &snapshot.meeting.id,
        true,
        None,
    )
}

#[test]
fn received_draft_restores_only_the_uniquely_cited_missing_institution_character() -> Result<()> {
    assert_eq!(
        format!("{:x}", Sha256::digest(ACTUAL_RAW_BODY.as_bytes())),
        "6b2d199a121fb636919e87f939a63cd933520d6c642bc72a1b39050dda595216"
    );
    let snapshot = snapshot(&ORIGINAL_SOURCES);
    let before = serde_json::to_value(&snapshot)?;
    let generated = draft(&snapshot, ACTUAL_RAW_BODY)?;
    // Keep the original failure and validator: only the generated preparation
    // stage may repair it, never a validator accepting an unsupported spelling.
    assert!(validate(&snapshot, &generated).is_err());
    let restored =
        validation::restore_generated_affiliations(&generated, &snapshot, &snapshot.segments)?;
    assert_eq!(restored.edits.len(), 1);
    let expected =
        writer::paragraphs(&ACTUAL_RAW_BODY.replace("迪大学的潘", "邓迪大学的潘")).join("\n\n");
    assert_eq!(restored.body, expected);
    // This operation does not hide the independent repetition defect.
    assert_eq!(restored.body.matches("主舞台音乐可能已结束").count(), 2);
    let edit = &restored.edits[0];
    assert_eq!(edit.source_id, snapshot.segments[3].id);
    assert_eq!(edit.source_revision, 1);
    assert_eq!(edit.suffix.len(), 110);
    assert_eq!(
        format!("{:x}", Sha256::digest(edit.suffix.as_bytes())),
        "dbe45fb0b6e5bbe29556b7ade10d10c420936dc7d5af7c50516d82cc732d346d"
    );
    let parsed: Value = serde_json::from_str(&generated)?;
    let body = parsed["sections"][edit.section_index]["body"]
        .as_str()
        .unwrap();
    assert_eq!(&body[edit.body_range.clone()], "迪大学");
    assert_eq!(edit.replacement, "邓迪大学");
    assert_eq!(
        &snapshot.segments[3].text[edit.source_suffix_range.clone()],
        edit.suffix
    );
    let sections = validate(&snapshot, &restored.draft)?;
    assert_eq!(sections[0].text, expected);
    assert!(sections[0].sources.iter().all(|q| snapshot
        .segments
        .iter()
        .any(|s| q.segment_id == s.id && q.revision == s.revision && q.quote == s.text)));
    assert_eq!(serde_json::to_value(&snapshot)?, before);
    let again =
        validation::restore_generated_affiliations(&restored.draft, &snapshot, &snapshot.segments)?;
    assert!(again.edits.is_empty());
    assert_eq!(again.draft, restored.draft);
    Ok(())
}

#[test]
fn repairing_a_name_does_not_authorize_lost_modality() -> Result<()> {
    let snapshot = snapshot(&ORIGINAL_SOURCES);
    let unfaithful = ACTUAL_RAW_BODY.replace(
        "主舞台音乐可能已结束，但节庆部分可能继续演奏至深夜。",
        "音乐在主舞台已经结束，但嘉年华的某些部分仍将持续演奏至深夜。",
    );
    let restored = validation::restore_generated_affiliations(
        &draft(&snapshot, &unfaithful)?,
        &snapshot,
        &snapshot.segments,
    )?;
    assert_eq!(restored.edits.len(), 1);
    let error = validate(&snapshot, &restored.draft).unwrap_err();
    assert!(error.is::<modal::Mismatch>(), "{error:#}");
    Ok(())
}

#[test]
fn a_generated_paragraph_must_have_its_own_current_citation_for_recovery() -> Result<()> {
    let snapshot = snapshot(&ORIGINAL_SOURCES);
    let mut generated: Value = serde_json::from_str(&draft(&snapshot, ACTUAL_RAW_BODY)?)?;
    for indices in [json!([1, 2, 3, 5]), json!([0]), json!([6])] {
        generated["sections"][0]["sources"] = indices;
        let encoded = generated.to_string();
        if let Ok(restored) =
            validation::restore_generated_affiliations(&encoded, &snapshot, &snapshot.segments)
        {
            assert!(restored.edits.is_empty());
            assert_eq!(restored.draft, encoded);
            assert!(validate(&snapshot, &restored.draft).is_err());
        }
    }
    // A selected clone cannot claim to be the current snapshot by ID alone.
    let generated = draft(&snapshot, ACTUAL_RAW_BODY)?;
    let mutations: &[fn(&mut Segment)] = &[
        |s| s.revision += 1,
        |s| s.text.push('。'),
        |s| s.start_ms += 1,
        |s| s.meeting_id = "other-meeting".into(),
        |s| s.run += 1,
        |s| s.speaker_manual = true,
    ];
    for mutate in mutations {
        let mut selected = snapshot.segments.clone();
        mutate(&mut selected[3]);
        if let Ok(restored) =
            validation::restore_generated_affiliations(&generated, &snapshot, &selected)
        {
            assert!(restored.edits.is_empty());
            assert_eq!(restored.draft, generated);
        }
    }
    Ok(())
}

#[test]
fn recovery_keeps_strict_document_fields_and_qualified_affiliation_boundaries() -> Result<()> {
    for source in [
        format!("如果{}", ORIGINAL_SOURCES[3]),
        format!("不要把“{}”当作事实", ORIGINAL_SOURCES[3]),
        ORIGINAL_SOURCES[3].replace("邓迪大学", "大学"),
    ] {
        let snapshot = snapshot(&[&source]);
        let raw = ORIGINAL_SOURCES[3].replace("邓迪大学", "迪大学");
        let generated = draft(&snapshot, &raw)?;
        let restored =
            validation::restore_generated_affiliations(&generated, &snapshot, &snapshot.segments)?;
        assert!(restored.edits.is_empty(), "{source}");
        assert_eq!(restored.draft, generated);
        assert!(validate(&snapshot, &restored.draft).is_err());
    }
    let snapshot = snapshot(&ORIGINAL_SOURCES);
    let mut generated: Value = serde_json::from_str(&draft(&snapshot, ACTUAL_RAW_BODY)?)?;
    generated["sections"][0]["human"] = true.into();
    assert!(validation::restore_generated_affiliations(
        &generated.to_string(),
        &snapshot,
        &snapshot.segments,
    )
    .is_err());
    Ok(())
}

fn seeded() -> Result<(tempfile::TempDir, Arc<Engine>, Snapshot)> {
    let root = tempfile::tempdir()?;
    let engine = Engine::open(root.path().into())?;
    let meeting = engine.create_meeting("Received spelling repair transaction")?;
    for mut source in snapshot(&ORIGINAL_SOURCES).segments {
        source.id = format!("{}:1:{}", meeting.id, source.seq);
        source.meeting_id = meeting.id.clone();
        engine.store.lock().unwrap().save_segment(&source)?;
    }
    let snapshot = engine.snapshot(&meeting.id)?;
    Ok((root, engine, snapshot))
}

#[test]
fn a_recovered_fresh_article_commits_with_original_current_source_quotes() -> Result<()> {
    let (_root, engine, snapshot) = seeded()?;
    let prefs = engine.preferences.lock().unwrap().clone();
    let scope = generation::Scope::new(
        &engine,
        &snapshot,
        &prefs,
        engine.context_revision.load(Ordering::SeqCst),
    )?;
    let restored = validation::restore_generated_affiliations(
        &draft(&snapshot, ACTUAL_RAW_BODY)?,
        &snapshot,
        &snapshot.segments,
    )?;
    let sections = validate(&snapshot, &restored.draft)?;
    scope.check(&engine, &snapshot.segments)?;
    let mut db = engine.store.lock().unwrap();
    scope.check_store(&db, &snapshot.segments)?;
    assert_eq!(
        db.commit_document(&snapshot, &snapshot.segments, 5, sections, false)?,
        1
    );
    let stored = db.suggestions(&snapshot.meeting.id)?;
    assert_eq!(stored[0].text, restored.body);
    assert!(stored[0].text.contains("邓迪大学的潘 MALE FERGUSON"));
    assert_eq!(db.meeting(&snapshot.meeting.id)?.notes_cursor, 5);
    assert_eq!(
        serde_json::to_value(db.segments(&snapshot.meeting)?)?,
        serde_json::to_value(&snapshot.segments)?
    );
    let jobs: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM inference_jobs", [], |r| r.get(0))?;
    assert_eq!(jobs, 0);
    Ok(())
}

#[test]
fn human_changes_after_recovery_invalidate_scope_and_cannot_be_overwritten() -> Result<()> {
    let (_root, engine, snapshot) = seeded()?;
    let prefs = engine.preferences.lock().unwrap().clone();
    let scope = generation::Scope::new(
        &engine,
        &snapshot,
        &prefs,
        engine.context_revision.load(Ordering::SeqCst),
    )?;
    let restored = validation::restore_generated_affiliations(
        &draft(&snapshot, ACTUAL_RAW_BODY)?,
        &snapshot,
        &snapshot.segments,
    )?;
    let mut sections = validate(&snapshot, &restored.draft)?;
    let mut human = sections[0].clone();
    human.id = "human-accepted".into();
    human.status = "accepted".into();
    human.text = "人工确认的内容，保留原样。".into();
    let mut db = engine.store.lock().unwrap();
    db.save_suggestion(&human)?;
    let before = serde_json::to_value(db.suggestions(&snapshot.meeting.id)?)?;
    assert!(scope.check_store(&db, &snapshot.segments).is_err());
    sections[0].id = human.id;
    assert!(db
        .commit_document(&snapshot, &snapshot.segments, 5, sections, false)
        .is_err());
    assert_eq!(
        serde_json::to_value(db.suggestions(&snapshot.meeting.id)?)?,
        before
    );
    assert_eq!(db.meeting(&snapshot.meeting.id)?.notes_cursor, 0);
    Ok(())
}
