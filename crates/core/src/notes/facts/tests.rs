use super::*;
use std::sync::Arc;
fn fixture(lines: &[&str]) -> Result<(tempfile::TempDir, Arc<crate::Engine>, Snapshot)> {
    let root = tempfile::tempdir()?;
    let e = crate::Engine::open(root.path().into())?;
    let m = e.create_meeting("Fictional fact ledger")?;
    for (i, line) in lines.iter().enumerate() {
        e.store.lock().unwrap().save_segment(&Segment {
            id: format!("{}:1:{}", m.id, i + 1),
            meeting_id: m.id.clone(),
            run: 1,
            seq: i as u32 + 1,
            start_ms: i as u64 * 1000,
            end_ms: (i as u64 + 1) * 1000,
            text: (*line).into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        })?;
    }
    let snapshot = e.snapshot(&m.id)?;
    Ok((root, e, snapshot))
}
fn update(
    id: u32,
    subject: &str,
    kind: &str,
    value: &str,
    state: &str,
    sources: Vec<usize>,
) -> Value {
    json!({"id":if id==0 {"new".to_owned()} else {format!("f{id}")},"subject":subject,"kind":kind,"value":value,"state":state,"sources":sources})
}
#[test]
fn changing_an_amount_keeps_unrelated_owner_deadline_and_pending_status() -> Result<()> {
    assert!(!KEY.is_empty());
    let (_root, _e, s) = fixture(&[
        "Budget is 3000, not approved. Morgan owns the review. Deadline is Thursday.",
        "Budget is corrected to 5000, still not approved.",
    ])?;
    let old = clauses(&s, &s.segments[..1]);
    assert_eq!(old.len(), 4);
    let initial = Ledger::default().apply(
        &s,
        &old,
        &json!({"updates":[
        update(0,"Budget","amount","3000","uncertain",vec![1,2]),
        update(0,"review","owner","Morgan","reported",vec![3]),
        update(0,"Deadline","deadline","Thursday","reported",vec![4])]})
        .to_string(),
        1,
    )?;
    let fresh = clauses(&s, &s.segments[1..]);
    let next = initial.apply(
        &s,
        &fresh,
        &json!({"updates":[update(1,"Budget","amount","5000","reported",vec![1])]}).to_string(),
        2,
    )?;
    assert_eq!(next.facts.len(), 4); // The uncited 'still not approved' clause is retained too.
    assert_eq!(next.facts[0].value, "5000");
    assert_eq!(next.facts[0].state, State::Uncertain);
    assert_eq!(next.facts[1], initial.facts[1]);
    assert_eq!(next.facts[2], initial.facts[2]);
    assert_eq!(initial.facts[0].value, "3000");
    let article = json!({"sections":[{"body":"Morgan owns the review. The budget is 5000, still unapproved, and the deadline is Thursday.","facts":[1,2,3,4]}]});
    let document = next.document(&s, &article.to_string(), "en")?;
    assert!(document[0].sources.iter().all(|q| s
        .segments
        .iter()
        .any(|x| x.id == q.segment_id && x.revision == q.revision && x.text == q.quote)));
    let wrong = article.to_string().replace("5000", "3000");
    assert!(next.document(&s, &wrong, "en").is_err());
    Ok(())
}
#[test]
fn literal_pending_and_completed_states_cannot_be_inverted() -> Result<()> {
    let (_root, _e, s) = fixture(&[
        "Upload is not completed.",
        "Upload has now been completed.",
        "No task has been completed.",
    ])?;
    let input = clauses(&s, &s.segments[..1]);
    let ledger=Ledger::default().apply(&s,&input,&json!({"updates":[update(0,"Upload","status","Upload is not completed","incomplete",vec![1])]}).to_string(),1)?;
    for patch in [
        update(
            0,
            "Upload",
            "status",
            "Upload is not completed",
            "completed",
            vec![1],
        ),
        update(0, "Upload", "status", "completed", "completed", vec![1]),
    ] {
        assert!(Ledger::default()
            .apply(&s, &input, &json!({"updates":[patch]}).to_string(), 1)
            .is_err());
    }
    let fresh = clauses(&s, &s.segments[1..2]);
    let next=ledger.apply(&s,&fresh,&json!({"updates":[update(1,"Upload","status","Upload has now been completed","completed",vec![1])]}).to_string(),2)?;
    let body =
        json!({"sections":[{"body":"The upload has now been completed.","facts":[1]}]}).to_string();
    assert!(next.document(&s, &body, "en").is_ok());
    let negative = clauses(&s, &s.segments[2..]);
    assert!(Ledger { cursor: 2, ..Ledger::default() }.apply(&s,&negative,&json!({"updates":[update(0,"task","status","No task has been completed","completed",vec![1])]}).to_string(),3).is_err());
    Ok(())
}
#[test]
fn hallucinated_fields_wrong_targets_and_stale_spans_fail_without_changes() -> Result<()> {
    let (_root, _e, s) = fixture(&[
        "Budget is 3000.",
        "Budget is now 5000. Review owner is Riley.",
    ])?;
    let input = clauses(&s, &s.segments[..1]);
    let old = Ledger::default().apply(
        &s,
        &input,
        &json!({"updates":[update(0,"Budget","amount","3000","reported",vec![1])]}).to_string(),
        1,
    )?;
    let fresh = clauses(&s, &s.segments[1..]);
    for patch in [
        update(1, "Budget", "owner", "Riley", "reported", vec![2]),
        update(3, "Budget", "amount", "5000", "reported", vec![1]),
        update(1, "Other", "amount", "5000", "reported", vec![1]),
        update(1, "Budget", "amount", "7000", "reported", vec![1]),
        update(0, "Budget", "amount", "5000", "reported", vec![1]),
        update(1, "Budget", "amount", "5000", "reported", vec![9]),
    ] {
        assert!(old
            .apply(&s, &fresh, &json!({"updates":[patch]}).to_string(), 2)
            .is_err());
        assert_eq!(old.facts[0].value, "3000");
    }
    let mut changed = s.clone();
    changed.segments[1].revision += 1;
    assert!(old
        .apply(
            &changed,
            &fresh,
            &json!({"updates":[update(1,"Budget","amount","5000","reported",vec![1])]}).to_string(),
            2
        )
        .is_err());
    Ok(())
}
#[test]
fn incomplete_extraction_retains_literal_clauses_and_document_cannot_drop_fact_ids() -> Result<()> {
    let (_root, _e, s) =
        fixture(&["Keep original audio. Manual edits stay intact. No network upload."])?;
    let input = clauses(&s, &s.segments);
    let ledger = Ledger::default().apply(&s, &input, r#"{"updates":[]}"#, 1)?;
    assert_eq!(ledger.facts.len(), 3);
    assert!(ledger
        .document(
            &s,
            r#"{"sections":[{"body":"Keep original audio.","facts":[1]}]}"#,
            "en"
        )
        .is_err());
    assert!(ledger.document(&s,r#"{"sections":[{"body":"Keep original audio; manual edits stay intact and no network upload occurs.","facts":[1,2,3]}]}"#,"en").is_ok());
    Ok(())
}
#[test]
fn clause_offsets_preserve_unicode_decimal_currency_and_question_scope() -> Result<()> {
    let (_root, _e, s) = fixture(&["预算3,000.50元，尚未确定。是否周五完成？"])?;
    let input = clauses(&s, &s.segments);
    assert_eq!(input.len(), 3);
    assert!(input[0].text.contains("3,000.50"));
    assert!(input[2].text.ends_with('？'));
    for c in input {
        assert_eq!(c.evidence.quote(&s)?, c.text);
    }
    Ok(())
}

#[test]
fn prefixes_and_older_revisions_cannot_be_skipped() -> Result<()> {
    let (_root, _e, s) = fixture(&[
        "First original claim. Second original claim.",
        "Later original claim.",
    ])?;
    let first = clauses(&s, &s.segments[..1]);
    let later = clauses(&s, &s.segments[1..]);
    for (input, cursor) in [(&first[..1], 1), (&first[..], 2), (&later[..], 2)] {
        assert!(Ledger::default()
            .apply(&s, input, r#"{"updates":[]}"#, cursor)
            .is_err());
    }
    let ledger = Ledger::default().apply(&s, &first, r#"{"updates":[]}"#, 1)?;
    let mut changed = s.clone();
    changed.segments[0].revision += 1;
    assert!(ledger
        .apply(&changed, &later, r#"{"updates":[]}"#, 2)
        .is_err());
    Ok(())
}
#[test]
fn an_identical_proposal_does_not_claim_new_evidence_or_erase_uncaptured_words() -> Result<()> {
    let (_root, _e, s) = fixture(&["Budget is 3000.", "Riley will test accessibility."])?;
    let first = clauses(&s, &s.segments[..1]);
    let later = clauses(&s, &s.segments[1..]);
    let old = Ledger::default().apply(
        &s,
        &first,
        &json!({"updates":[update(0,"Budget","amount","3000","reported",vec![1])]}).to_string(),
        1,
    )?;
    let next = old.apply(
        &s,
        &later,
        &json!({"updates":[update(1,"Budget","amount","3000","reported",vec![1])]}).to_string(),
        2,
    )?;
    assert_eq!(next.facts[0], old.facts[0]);
    assert_eq!(next.facts.len(), 2);
    assert!(next.facts[1].value.contains("accessibility"));
    assert_eq!(next.cursor, 2);
    Ok(())
}
#[test]
fn currency_prefixes_are_never_inferred_from_bare_amounts() -> Result<()> {
    for source in ["Budget is 3000.", "Budget is $3000."] {
        let (_root, _e, s) = fixture(&[source])?;
        let input = clauses(&s, &s.segments);
        let ledger = Ledger::default().apply(
            &s,
            &input,
            &json!({"updates":[update(0,"Budget","amount","3000","reported",vec![1])]}).to_string(),
            1,
        )?;
        let dollar = r#"{"sections":[{"body":"Budget is $3000.","facts":[1]}]}"#;
        assert_eq!(
            ledger.document(&s, dollar, "en").is_ok(),
            source.contains('$')
        );
    }
    Ok(())
}
#[test]
fn a_positive_clause_cannot_relabel_another_negative_status_value() -> Result<()> {
    let (_root, _e, s) = fixture(&["Upload is not completed. Upload has now been completed."])?;
    let input = clauses(&s, &s.segments);
    let mixed=json!({"updates":[update(0,"Upload","status","Upload is not completed","completed",vec![1,2])]}).to_string();
    assert!(Ledger::default().apply(&s, &input, &mixed, 1).is_err());
    Ok(())
}

#[test]
fn clause_choices_cannot_borrow_another_subjects_value_state_or_old_identity() -> Result<()> {
    let (_root,_e,s)=fixture(&[
        "Morgan will run upload verification on Thursday.",
        "Morgan confirms upload verification is completed. There are no remaining tasks. Budget is 5000. Upload remains unapproved."
    ])?;
    let old=Ledger::default().apply(&s,&clauses(&s,&s.segments[..1]),&json!({"updates":[update(0,"Morgan","statement","Morgan will run upload verification on Thursday","planned",vec![1])]}).to_string(),1)?;
    let fresh = clauses(&s, &s.segments[1..]);
    for patch in [
        // Morgan occurs in the utterance but not in this independent clause.
        update(
            1,
            "Morgan",
            "statement",
            "There are no remaining tasks",
            "reported",
            vec![2],
        ),
        // A true negative from a separate subject cannot qualify this amount.
        update(0, "Budget", "amount", "5000", "uncertain", vec![3, 4]),
        // A whole-clause value cannot cite a different independent clause.
        update(
            0,
            "Morgan",
            "statement",
            "There are no remaining tasks",
            "reported",
            vec![1, 2],
        ),
    ] {
        assert!(old
            .apply(&s, &fresh, &json!({"updates":[patch]}).to_string(), 2)
            .is_err());
        assert!(old.facts[0].value.contains("Thursday"));
    }
    let next=old.apply(&s,&fresh,&json!({"updates":[update(1,"Morgan","statement","Morgan confirms upload verification is completed","completed",vec![1])]}).to_string(),2)?;
    assert_eq!(next.facts[0].state, State::Completed);
    assert_eq!(old.facts[0].state, State::Planned);
    Ok(())
}
#[test]
fn subordinate_support_never_crosses_periods_or_source_revisions() -> Result<()> {
    for (source, allowed) in [
        ("Budget is 3000, not approved.", true),
        ("Budget is 3000. Not approved.", false),
    ] {
        let (_root, _e, s) = fixture(&[source])?;
        let proposal =
            json!({"updates":[update(0,"Budget","amount","3000","uncertain",vec![1,2])]})
                .to_string();
        assert_eq!(
            Ledger::default()
                .apply(&s, &clauses(&s, &s.segments), &proposal, 1)
                .is_ok(),
            allowed
        );
    }
    let (_root, _e, s) = fixture(&["Budget is 3000,", "not approved."])?;
    let proposal =
        json!({"updates":[update(0,"Budget","amount","3000","uncertain",vec![1,2])]}).to_string();
    assert!(Ledger::default()
        .apply(&s, &clauses(&s, &s.segments), &proposal, 2)
        .is_err());
    Ok(())
}

#[test]
fn replacing_an_owner_then_withdrawing_the_old_name_keeps_the_new_owner() -> Result<()> {
    let (_root, _e, s) = fixture(&[
        "负责人是李明，期限是周五。",
        "更正负责人，王华负责，李明不再负责。",
    ])?;
    let first = clauses(&s, &s.segments[..1]);
    let old = Ledger::default().apply(
        &s,
        &first,
        &json!({"updates":[update(0,"负责人","owner","李明","reported",vec![1])]}).to_string(),
        1,
    )?;
    let fresh = clauses(&s, &s.segments[1..]);
    let next = old.apply(
        &s,
        &fresh,
        &json!({"updates":[
        update(1,"负责人","owner","王华","reported",vec![1,2]),
        update(1,"负责人","owner","李明","withdrawn",vec![1,3])]})
        .to_string(),
        2,
    )?;
    assert_eq!(next.facts[0].value, "王华");
    assert_eq!(next.facts[0].state, State::Reported);
    assert_eq!(next.facts[0].version, 2);
    assert_eq!(next.facts[1], old.facts[1]);
    assert!(next.facts.iter().any(|f| f.kind == Kind::Statement
        && f.state == State::Withdrawn
        && f.value.contains("李明不再负责")));
    assert_eq!(old.facts[0].value, "李明");
    Ok(())
}
#[test]
fn duplicate_same_value_is_idempotent_but_two_conflicting_assignments_are_rejected() -> Result<()> {
    let (_root, _e, s) = fixture(&["负责人是李明。", "更正负责人，王华负责，陈晨负责。"])?;
    let old = Ledger::default().apply(
        &s,
        &clauses(&s, &s.segments[..1]),
        &json!({"updates":[update(0,"负责人","owner","李明","reported",vec![1])]}).to_string(),
        1,
    )?;
    let fresh = clauses(&s, &s.segments[1..]);
    let same = update(1, "负责人", "owner", "王华", "reported", vec![1, 2]);
    let accepted = old.apply(
        &s,
        &fresh,
        &json!({"updates":[same.clone(),same.clone()]}).to_string(),
        2,
    )?;
    assert_eq!(accepted.facts[0].version, 2);
    // Extra claims in omitted clauses are retained, even for idempotent proposals.
    assert!(accepted.facts.iter().any(|f| f.value.contains("陈晨负责")));
    let other = update(1, "负责人", "owner", "陈晨", "reported", vec![1, 3]);
    assert!(old
        .apply(&s, &fresh, &json!({"updates":[same,other]}).to_string(), 2)
        .is_err());
    assert_eq!(old.facts[0].value, "李明");
    Ok(())
}
#[test]
fn withdrawal_of_the_current_owner_without_a_replacement_remains_withdrawn() -> Result<()> {
    let (_root, _e, s) = fixture(&["负责人是李明。", "更正负责人，李明不再负责。"])?;
    let old = Ledger::default().apply(
        &s,
        &clauses(&s, &s.segments[..1]),
        &json!({"updates":[update(0,"负责人","owner","李明","reported",vec![1])]}).to_string(),
        1,
    )?;
    let next = old.apply(
        &s,
        &clauses(&s, &s.segments[1..]),
        &json!({"updates":[update(1,"负责人","owner","李明","withdrawn",vec![1,2])]}).to_string(),
        2,
    )?;
    assert_eq!(next.facts[0].state, State::Withdrawn);
    assert_eq!(next.facts[0].value, "李明");
    Ok(())
}
