//! Replay only concrete, source-backed spelling edits into fresh AI prose.
use crate::{store::Store, Meeting, Segment, Source, Suggestion, UserNote};
use anyhow::Result;
use regex::Regex;
use rusqlite::{params, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub(super) struct Replacement {
    pub old: String,
    pub new: String,
    pub model: bool,
}

#[cfg(test)]
mod unchanged_source_tests {
    use super::*;
    use crate::Engine;
    use std::sync::Arc;

    const CANONICAL: &str = "OmaScribe尚未修复，预算仍未批准。";
    const OLD_MINUTES: &str = "ALMA SCRIBE尚未修复，预算仍未批准。";
    const NOTE: &str = "我正在测试omascribe的功能";

    fn seed() -> Result<(tempfile::TempDir, Arc<Engine>, UserNote, crate::Snapshot)> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let meeting = engine.create_meeting("Existing canonical source note spelling")?;
        engine.update_meeting(&meeting.id, &meeting.title, false)?;
        let note = engine.save_note(&meeting.id, None, "功能待测试", "document", 0)?;
        let source = Segment {
            id: format!("{}:1:1", meeting.id),
            meeting_id: meeting.id.clone(),
            run: 1,
            seq: 1,
            start_ms: 100,
            end_ms: 1200,
            text: CANONICAL.into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        engine.store.lock().unwrap().save_segment(&source)?;
        let snapshot = engine.snapshot(&meeting.id)?;
        engine.store.lock().unwrap().commit_document(
            &snapshot,
            std::slice::from_ref(&source),
            1,
            vec![Suggestion {
                id: String::new(),
                meeting_id: meeting.id.clone(),
                kind: "section".into(),
                focus: String::new(),
                text: OLD_MINUTES.into(),
                status: "pending".into(),
                stale: false,
                sources: vec![Source {
                    segment_id: source.id.clone(),
                    revision: 1,
                    start_ms: source.start_ms,
                    end_ms: source.end_ms,
                    quote: CANONICAL.into(),
                    speaker_name: None,
                }],
            }],
            false,
        )?;
        let snapshot = engine.snapshot(&meeting.id)?;
        Ok((root, engine, note, snapshot))
    }

    #[test]
    fn plain_note_repairs_old_alias_minutes_when_current_cited_source_is_already_canonical(
    ) -> Result<()> {
        let (_root, engine, note, before) = seed()?;
        assert!(!before.meeting.auto_notes);
        engine.save_note(&before.meeting.id, Some(note.id), NOTE, "document", 0)?;
        let after = engine.snapshot(&before.meeting.id)?;
        assert_eq!(after.suggestions[0].text, CANONICAL);
        assert_eq!(after.suggestions[0].id, before.suggestions[0].id);
        assert_eq!(after.suggestions[0].status, "pending");
        assert!(!after.suggestions[0].stale);
        assert_eq!(
            serde_json::to_value(&after.suggestions[0].sources)?,
            serde_json::to_value(&before.suggestions[0].sources)?
        );
        assert_eq!(
            serde_json::to_value(&after.segments)?,
            serde_json::to_value(&before.segments)?
        );
        assert_eq!(after.segments[0].revision, 1);
        let jobs: i64 = engine.store.lock().unwrap().conn.query_row(
            "SELECT COUNT(*) FROM inference_jobs",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(jobs, 0);
        Ok(())
    }

    fn history(engine: &Engine, meeting: &str) -> Result<Vec<String>> {
        let db = engine.store.lock().unwrap();
        let mut query = db
            .conn
            .prepare("SELECT json FROM document_history WHERE meeting_id=?1 ORDER BY id")?;
        let rows = query.query_map([meeting], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    #[test]
    fn canonical_source_replay_respects_human_state_current_citations_and_whole_claims(
    ) -> Result<()> {
        for case in [
            "accepted",
            "stale",
            "revision",
            "quote",
            "start",
            "end",
            "speaker",
            "missing",
            "human",
            "legacy",
            "budget",
            "negation",
            "uncited",
            "duplicate",
            "ambiguous",
        ] {
            let (_root, engine, note, seeded) = seed()?;
            let mut section = seeded.suggestions[0].clone();
            match case {
                "accepted" => section.status = "accepted".into(),
                "stale" => section.stale = true,
                "revision" => section.sources[0].revision += 1,
                "quote" => section.sources[0].quote = OLD_MINUTES.into(),
                "start" => section.sources[0].start_ms += 1,
                "end" => section.sources[0].end_ms += 1,
                "speaker" => section.sources[0].speaker_name = Some("Human label".into()),
                "missing" => section.sources[0].segment_id = "missing-source".into(),
                "human" => {
                    engine.store.lock().unwrap().conn.execute(
                        "INSERT INTO segment_text_edits VALUES(?1,1,'human')",
                        [&seeded.segments[0].id],
                    )?;
                }
                "legacy" => {
                    let mut original = seeded.segments[0].clone();
                    original.text = "人工检查前的转写。".into();
                    let mut current = seeded.segments[0].clone();
                    current.revision = 2;
                    let db = engine.store.lock().unwrap();
                    db.conn.execute(
                        "INSERT INTO segment_history VALUES(?1,1,?2)",
                        params![original.id, serde_json::to_string(&original)?],
                    )?;
                    db.save_segment(&current)?;
                    section.sources[0].revision = 2;
                }
                "budget" => section.text = OLD_MINUTES.replace("仍未批准", "已批准"),
                "negation" => section.text = OLD_MINUTES.replace("尚未修复", "已经修复"),
                "uncited" | "duplicate" => {
                    let mut other = seeded.segments[0].clone();
                    other.id.push_str(":other");
                    other.seq = 2;
                    if case == "uncited" {
                        other.text = "测试尚未开始，预算仍未批准。".into();
                        section.sources.clear();
                    }
                    engine.store.lock().unwrap().save_segment(&other)?;
                    section.sources.push(Source {
                        segment_id: other.id,
                        revision: other.revision,
                        start_ms: other.start_ms,
                        end_ms: other.end_ms,
                        quote: other.text,
                        speaker_name: None,
                    });
                }
                "ambiguous" => {}
                _ => unreachable!(),
            }
            engine.store.lock().unwrap().save_suggestion(&section)?;
            let before = engine.snapshot(&seeded.meeting.id)?;
            let old_history = history(&engine, &seeded.meeting.id)?;
            let note_text = if case == "ambiguous" {
                "我正在测试OmaScribe和AlmaScribe的功能"
            } else {
                NOTE
            };
            engine.save_note(&before.meeting.id, Some(note.id), note_text, "document", 0)?;
            let after = engine.snapshot(&before.meeting.id)?;
            if ["budget", "negation", "duplicate"].contains(&case) {
                let mut expected = before.suggestions.clone();
                expected[0].text = expected[0].text.replace("ALMA SCRIBE", "OmaScribe");
                expected[0].stale = true;
                assert_eq!(
                    serde_json::to_value(&after.suggestions)?,
                    serde_json::to_value(&expected)?,
                    "{case}: only the name changes; other claims and all citations are retained"
                );
                assert!(after.meeting.notes_fingerprint.is_empty(), "{case}");
                assert!(crate::notes::needs_update(&after), "{case}");
                let updated_history = history(&engine, &before.meeting.id)?;
                assert_eq!(updated_history.len(), old_history.len() + 1, "{case}");
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(updated_history.last().unwrap())?,
                    serde_json::to_value(&before.suggestions)?,
                    "{case}"
                );
            } else {
                assert_eq!(
                    serde_json::to_value(&after.suggestions)?,
                    serde_json::to_value(&before.suggestions)?,
                    "{case}"
                );
                assert_eq!(history(&engine, &before.meeting.id)?, old_history, "{case}");
            }
            assert_eq!(
                serde_json::to_value(&after.segments)?,
                serde_json::to_value(&before.segments)?,
                "{case}"
            );
        }
        Ok(())
    }

    #[test]
    fn canonical_source_replay_is_idempotent_and_does_not_archive_or_infer_again() -> Result<()> {
        for alias in ["ALMA SCRIBE", "OMA SCRIBE", "奥马斯"] {
            let (_root, engine, note, before) = seed()?;
            let mut old = before.suggestions[0].clone();
            old.text = CANONICAL.replace("OmaScribe", alias);
            engine.store.lock().unwrap().save_suggestion(&old)?;
            let old_history = history(&engine, &before.meeting.id)?;
            engine.save_note(
                &before.meeting.id,
                Some(note.id.clone()),
                NOTE,
                "document",
                0,
            )?;
            let updated_history = history(&engine, &before.meeting.id)?;
            assert_eq!(updated_history.len(), old_history.len() + 1, "{alias}");
            assert_eq!(
                serde_json::from_str::<Vec<Suggestion>>(updated_history.last().unwrap())?[0].text,
                old.text
            );
            let after = engine.snapshot(&before.meeting.id)?;
            assert_eq!(after.suggestions[0].text, CANONICAL, "{alias}");
            assert_eq!(engine.correct_terminology(&before.meeting.id)?, 0);
            engine.save_note(&before.meeting.id, Some(note.id), NOTE, "document", 0)?;
            assert_eq!(history(&engine, &before.meeting.id)?, updated_history);
            let again = engine.snapshot(&before.meeting.id)?;
            assert_eq!(
                serde_json::to_value(&again.suggestions)?,
                serde_json::to_value(&after.suggestions)?
            );
            assert_eq!(again.segments[0].revision, 1);
            let jobs: i64 = engine.store.lock().unwrap().conn.query_row(
                "SELECT COUNT(*) FROM inference_jobs",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(jobs, 0);
        }
        Ok(())
    }

    #[test]
    fn canonical_source_replay_failure_rolls_back_terms_document_and_history() -> Result<()> {
        let (_root, engine, note, before) = seed()?;
        let old_history = history(&engine, &before.meeting.id)?;
        let old_terms = engine
            .store
            .lock()
            .unwrap()
            .cache_dependency(&before.meeting.id, "terms")?;
        engine.store.lock().unwrap().conn.execute_batch(
            "CREATE TRIGGER reject_canonical_repair BEFORE UPDATE ON suggestions
             BEGIN SELECT RAISE(ABORT,'fixture canonical repair failed'); END;",
        )?;
        let error = engine
            .save_note(&before.meeting.id, Some(note.id), NOTE, "document", 0)
            .expect_err("the real document write must fail inside the terms transaction");
        assert!(format!("{error:#}").contains("fixture canonical repair failed"));
        let after = engine.snapshot(&before.meeting.id)?;
        assert_eq!(after.notes[0].content, NOTE);
        assert_eq!(
            serde_json::to_value(&after.suggestions)?,
            serde_json::to_value(&before.suggestions)?
        );
        assert_eq!(
            serde_json::to_value(&after.segments)?,
            serde_json::to_value(&before.segments)?
        );
        assert_eq!(history(&engine, &before.meeting.id)?, old_history);
        let db = engine.store.lock().unwrap();
        assert_eq!(db.cache_dependency(&before.meeting.id, "terms")?, old_terms);
        assert!(db.conn.is_autocommit());
        Ok(())
    }

    #[test]
    fn one_plain_note_repairs_changed_and_already_canonical_sources_with_their_own_minutes(
    ) -> Result<()> {
        let (_root, engine, note, first) = seed()?;
        let second = Segment {
            id: format!("{}:1:2", first.meeting.id),
            meeting_id: first.meeting.id.clone(),
            run: 1,
            seq: 2,
            start_ms: 1300,
            end_ms: 2400,
            text: "ALMA SCRIBE的字幕仍待测试。".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        engine.store.lock().unwrap().save_segment(&second)?;
        let added = engine.snapshot(&first.meeting.id)?;
        let mut sections = added.suggestions.clone();
        sections.push(Suggestion {
            id: String::new(),
            meeting_id: first.meeting.id.clone(),
            kind: "section".into(),
            focus: String::new(),
            text: second.text.clone(),
            status: "pending".into(),
            stale: false,
            sources: vec![Source {
                segment_id: second.id.clone(),
                revision: 1,
                start_ms: second.start_ms,
                end_ms: second.end_ms,
                quote: second.text.clone(),
                speaker_name: None,
            }],
        });
        engine.store.lock().unwrap().commit_document(
            &added,
            &added.segments,
            2,
            sections,
            false,
        )?;
        let before = engine.snapshot(&first.meeting.id)?;
        assert!(!before.meeting.auto_notes);
        engine.save_note(&first.meeting.id, Some(note.id), NOTE, "document", 0)?;
        let after = engine.snapshot(&first.meeting.id)?;
        let first_section = after
            .suggestions
            .iter()
            .find(|section| section.id == before.suggestions[0].id)
            .unwrap();
        let second_section = after
            .suggestions
            .iter()
            .find(|section| section.id == before.suggestions[1].id)
            .unwrap();
        assert_eq!(second_section.text, "OmaScribe的字幕仍待测试。");
        assert_eq!(second_section.sources[0].quote, after.segments[1].text);
        assert_eq!(second_section.sources[0].revision, 2);
        assert!(!second_section.stale);
        assert_eq!(first_section.text, CANONICAL);
        assert!(!first_section.stale);
        assert_eq!(first_section.status, "pending");
        assert_eq!(
            serde_json::to_value(&first_section.sources)?,
            serde_json::to_value(&before.suggestions[0].sources)?
        );
        assert_eq!(
            serde_json::to_value(&after.segments[0])?,
            serde_json::to_value(&before.segments[0])?
        );
        assert_eq!(after.segments[0].revision, 1);
        assert_eq!(after.segments[1].revision, 2);
        assert_eq!(after.meeting.notes_cursor, 2);
        assert!(after.meeting.notes_fingerprint.is_empty());
        assert!(crate::notes::needs_update(&after));
        let db = engine.store.lock().unwrap();
        let original: String = db.conn.query_row(
            "SELECT json FROM segment_history WHERE id=?1 AND revision=1",
            [&second.id],
            |row| row.get(0),
        )?;
        assert_eq!(
            serde_json::from_str::<Segment>(&original)?.text,
            second.text
        );
        let jobs: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM inference_jobs", [], |row| row.get(0))?;
        assert_eq!(jobs, 0);
        Ok(())
    }

    fn mixed_seed() -> Result<(tempfile::TempDir, Arc<Engine>, UserNote, crate::Snapshot)> {
        let (root, engine, note, first) = seed()?;
        let second = Segment {
            id: format!("{}:1:2", first.meeting.id),
            meeting_id: first.meeting.id.clone(),
            run: 1,
            seq: 2,
            start_ms: 1300,
            end_ms: 2400,
            text: "ALMA SCRIBE的字幕仍待测试。".into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        engine.store.lock().unwrap().save_segment(&second)?;
        let snapshot = engine.snapshot(&first.meeting.id)?;
        let mut sections = snapshot.suggestions.clone();
        sections.push(Suggestion {
            id: String::new(),
            meeting_id: first.meeting.id.clone(),
            kind: "section".into(),
            focus: String::new(),
            text: second.text.clone(),
            status: "pending".into(),
            stale: false,
            sources: vec![Source {
                segment_id: second.id.clone(),
                revision: 1,
                start_ms: second.start_ms,
                end_ms: second.end_ms,
                quote: second.text,
                speaker_name: None,
            }],
        });
        engine.store.lock().unwrap().commit_document(
            &snapshot,
            &snapshot.segments,
            2,
            sections,
            false,
        )?;
        let before = engine.snapshot(&first.meeting.id)?;
        Ok((root, engine, note, before))
    }

    #[test]
    fn mixed_replay_does_not_grant_old_stale_human_or_invalid_citation_authority() -> Result<()> {
        for case in ["accepted", "stale", "human", "revision", "quote", "time"] {
            let (_root, engine, note, seeded) = mixed_seed()?;
            let mut first = seeded.suggestions[0].clone();
            match case {
                "accepted" => first.status = "accepted".into(),
                "stale" => first.stale = true,
                "human" => {
                    engine.store.lock().unwrap().conn.execute(
                        "INSERT INTO segment_text_edits VALUES(?1,1,'human')",
                        [&seeded.segments[0].id],
                    )?;
                }
                "revision" => first.sources[0].revision += 1,
                "quote" => first.sources[0].quote = OLD_MINUTES.into(),
                "time" => first.sources[0].start_ms += 1,
                _ => unreachable!(),
            }
            engine.store.lock().unwrap().save_suggestion(&first)?;
            engine.save_note(&seeded.meeting.id, Some(note.id), NOTE, "document", 0)?;
            let after = engine.snapshot(&seeded.meeting.id)?;
            assert_eq!(after.suggestions[0].text, OLD_MINUTES, "{case}");
            assert_eq!(after.suggestions[0].status, first.status, "{case}");
            assert_eq!(
                serde_json::to_value(&after.suggestions[0].sources)?,
                serde_json::to_value(&first.sources)?,
                "{case}"
            );
            assert_eq!(
                after.suggestions[1].text, "OmaScribe的字幕仍待测试。",
                "{case}"
            );
            assert!(!after.suggestions[1].stale, "{case}");
            assert_eq!(after.suggestions[1].sources[0].revision, 2, "{case}");
            assert_eq!(after.segments[0].revision, 1, "{case}");
            assert_eq!(after.segments[1].revision, 2, "{case}");
        }
        Ok(())
    }

    #[test]
    fn mixed_replay_archives_once_stays_dirty_and_is_idempotent_without_inference() -> Result<()> {
        let (_root, engine, note, before) = mixed_seed()?;
        let old_history = history(&engine, &before.meeting.id)?;
        engine.save_note(
            &before.meeting.id,
            Some(note.id.clone()),
            NOTE,
            "document",
            0,
        )?;
        let updated_history = history(&engine, &before.meeting.id)?;
        assert_eq!(updated_history.len(), old_history.len() + 1);
        let archived: Vec<Suggestion> = serde_json::from_str(updated_history.last().unwrap())?;
        assert_eq!(archived[0].text, OLD_MINUTES);
        assert_eq!(archived[1].text, "ALMA SCRIBE的字幕仍待测试。");
        assert_eq!(archived[0].sources[0].revision, 1);
        assert_eq!(archived[1].sources[0].revision, 1);
        let after = engine.snapshot(&before.meeting.id)?;
        assert_eq!(after.suggestions[0].text, CANONICAL);
        assert_eq!(after.suggestions[1].text, "OmaScribe的字幕仍待测试。");
        assert!(after.meeting.notes_fingerprint.is_empty());
        assert!(crate::notes::needs_update(&after));
        assert_eq!(engine.correct_terminology(&before.meeting.id)?, 0);
        engine.save_note(&before.meeting.id, Some(note.id), NOTE, "document", 0)?;
        assert_eq!(history(&engine, &before.meeting.id)?, updated_history);
        let again = engine.snapshot(&before.meeting.id)?;
        assert_eq!(
            serde_json::to_value(&again.suggestions)?,
            serde_json::to_value(&after.suggestions)?
        );
        let jobs: i64 = engine.store.lock().unwrap().conn.query_row(
            "SELECT COUNT(*) FROM inference_jobs",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(jobs, 0);
        Ok(())
    }

    #[test]
    fn mixed_replay_second_document_write_failure_rolls_back_the_entire_source_transaction(
    ) -> Result<()> {
        let (_root, engine, note, before) = mixed_seed()?;
        let old_history = history(&engine, &before.meeting.id)?;
        let old_terms = engine
            .store
            .lock()
            .unwrap()
            .cache_dependency(&before.meeting.id, "terms")?;
        engine.store.lock().unwrap().conn.execute_batch(
            "CREATE TRIGGER reject_second_mixed_repair BEFORE UPDATE ON suggestions
             WHEN json_extract(NEW.json,'$.text')='OmaScribe的字幕仍待测试。'
             BEGIN SELECT RAISE(ABORT,'fixture second mixed repair failed'); END;",
        )?;
        let error = engine
            .save_note(&before.meeting.id, Some(note.id), NOTE, "document", 0)
            .expect_err("second real document update must fail after the first repair write");
        assert!(format!("{error:#}").contains("fixture second mixed repair failed"));
        let after = engine.snapshot(&before.meeting.id)?;
        assert_eq!(after.notes[0].content, NOTE);
        assert_eq!(
            serde_json::to_value(&after.segments)?,
            serde_json::to_value(&before.segments)?
        );
        assert_eq!(
            serde_json::to_value(&after.suggestions)?,
            serde_json::to_value(&before.suggestions)?
        );
        assert_eq!(
            after.meeting.notes_fingerprint,
            before.meeting.notes_fingerprint
        );
        assert_eq!(after.meeting.notes_cursor, before.meeting.notes_cursor);
        assert_eq!(history(&engine, &before.meeting.id)?, old_history);
        let db = engine.store.lock().unwrap();
        assert_eq!(db.cache_dependency(&before.meeting.id, "terms")?, old_terms);
        let raw_rows: i64 = db.conn.query_row(
            "SELECT COUNT(*) FROM segment_history WHERE id=?1",
            [&before.segments[1].id],
            |row| row.get(0),
        )?;
        assert_eq!(raw_rows, 0);
        assert!(db.conn.is_autocommit());
        Ok(())
    }

    #[test]
    fn mixed_checkpoint_cannot_ack_a_name_alias_paraphrase_without_literal_repair_proof(
    ) -> Result<()> {
        let (_root, engine, note, seeded) = mixed_seed()?;
        engine.save_note(
            &seeded.meeting.id,
            Some(note.id.clone()),
            "ALMA SCRIBE",
            "document",
            0,
        )?;
        let snapshot = engine.snapshot(&seeded.meeting.id)?;
        let mut sections = snapshot.suggestions.clone();
        sections[0].text = "ALMA SCRIBE的修复尚未完成，预算还没有批准。".into();
        engine.store.lock().unwrap().commit_document(
            &snapshot,
            &snapshot.segments,
            2,
            sections,
            false,
        )?;
        let before = engine.snapshot(&seeded.meeting.id)?;
        assert!(!crate::notes::needs_update(&before));
        assert!(!before.meeting.notes_fingerprint.is_empty());
        engine.save_note(
            &before.meeting.id,
            Some(note.id),
            "OmaScribe",
            "document",
            0,
        )?;
        let after = engine.snapshot(&before.meeting.id)?;
        assert_eq!(
            after.suggestions[0].text,
            before.suggestions[0]
                .text
                .replace("ALMA SCRIBE", "OmaScribe")
        );
        assert_eq!(
            serde_json::to_value(&after.suggestions[0].sources)?,
            serde_json::to_value(&before.suggestions[0].sources)?
        );
        assert!(
            after.suggestions[0].stale,
            "a name correction that lacks whole-source proof must remain pending, not pass the old unchanged checkpoint"
        );
        assert_eq!(after.suggestions[1].text, "OmaScribe的字幕仍待测试。");
        assert!(!after.suggestions[1].stale);
        assert_eq!(after.segments[0].revision, 1);
        assert_eq!(after.segments[1].revision, 2);
        assert!(after.meeting.notes_fingerprint.is_empty());
        assert!(crate::notes::needs_update(&after));
        Ok(())
    }

    #[test]
    fn canonical_only_name_check_does_not_decode_an_irrelevant_cited_source() -> Result<()> {
        let (_root, engine, note, before) = seed()?;
        let mut plain = before.suggestions[0].clone();
        plain.text = "测试和预算仍待确认。".into();
        engine.store.lock().unwrap().save_suggestion(&plain)?;
        let old_history = history(&engine, &before.meeting.id)?;
        let mut name_note = note;
        name_note.content = NOTE.into();
        let terms = super::super::Terms::new(&[name_note], &[]);
        let mut db = engine.store.lock().unwrap();
        // The queue trigger extracts $.revision, so storage must contain valid
        // JSON. A scalar is valid JSON but cannot deserialize as a Segment.
        let invalid_segment = serde_json::to_string("fixture is not a Segment")?;
        db.conn.execute(
            "UPDATE segments SET json=?2 WHERE id=?1",
            params![before.segments[0].id, invalid_segment],
        )?;
        // Direct production savepoint/helper control: Engine's earlier source
        // scan is deliberately out of this ordering test's scope. This proves
        // no decode for a no-name-edit paragraph, not zero database IO.
        assert_eq!(
            db.with_terminology_savepoint(|db| {
                db.repair_canonical_source_documents(&before.meeting, &terms)
            })?,
            0
        );
        let still_invalid: String = db.conn.query_row(
            "SELECT json FROM segments WHERE id=?1",
            [&before.segments[0].id],
            |row| row.get(0),
        )?;
        assert_eq!(still_invalid, invalid_segment);
        let mut relevant = plain.clone();
        relevant.text = OLD_MINUTES.into();
        db.save_suggestion(&relevant)?;
        assert!(db
            .with_terminology_savepoint(|db| {
                db.repair_canonical_source_documents(&before.meeting, &terms)
            })
            .is_err());
        assert!(db.conn.is_autocommit());
        drop(db);
        assert_eq!(history(&engine, &before.meeting.id)?, old_history);
        Ok(())
    }
}
pub(super) struct SourceChange {
    pub before: Segment,
    pub after: Segment,
    pub replacements: Vec<Replacement>,
}

/// Captured before saving one existing active document. This is only a possible
/// spelling edit; no synchronization is acknowledged until source-backed replay
/// and the complete article transaction have both succeeded.
pub(crate) struct NoteSpellingEdit {
    meeting: Meeting,
    notes: Vec<UserNote>,
    sections: Vec<Suggestion>,
    saved: UserNote,
}

impl NoteSpellingEdit {
    pub(super) fn reusable_fingerprint(
        &self,
        meeting: &Meeting,
        notes: &[UserNote],
        sections: &[Suggestion],
        sources: &[Segment],
        changes: &[SourceChange],
        changed: usize,
    ) -> Result<Option<String>> {
        if changed == 0
            || changed != changes.len()
            || serde_json::to_string(meeting)? != serde_json::to_string(&self.meeting)?
            || serde_json::to_string(sections)? != serde_json::to_string(&self.sections)?
            || sources.last().is_none_or(|s| s.seq > meeting.notes_cursor)
            || crate::notes::document_notes(notes)
                .first()
                .is_none_or(|note| note.id != self.saved.id)
        {
            return Ok(None);
        }
        let mut expected = self.notes.clone();
        let Some(old_note) = expected.iter_mut().find(|note| note.id == self.saved.id) else {
            return Ok(None);
        };
        let mut edits = BTreeMap::new();
        for edit in changes.iter().flat_map(|change| &change.replacements) {
            if merge(&mut edits, edit).is_none() {
                return Ok(None);
            }
        }
        let Some((replayed, used)) = replay(&old_note.content, &edits) else {
            return Ok(None);
        };
        if used.is_empty() || replayed != self.saved.content {
            return Ok(None);
        }
        *old_note = self.saved.clone();
        expected.sort_by(|a, b| a.id.cmp(&b.id));
        let mut observed = notes.to_vec();
        observed.sort_by(|a, b| a.id.cmp(&b.id));
        if serde_json::to_string(&expected)? != serde_json::to_string(&observed)? {
            return Ok(None);
        }
        let mut old_sources = sources.to_vec();
        for change in changes {
            let Some(current) = old_sources.iter_mut().find(|s| s.id == change.after.id) else {
                return Ok(None);
            };
            if serde_json::to_string(current)? != serde_json::to_string(&change.after)? {
                return Ok(None);
            }
            *current = change.before.clone();
        }
        if crate::notes::fingerprint(&self.notes, &old_sources, meeting.notes_cursor)
            != self.meeting.notes_fingerprint
        {
            return Ok(None);
        }
        Ok(Some(crate::notes::fingerprint(
            notes,
            sources,
            meeting.notes_cursor,
        )))
    }
}

/// Case-sensitive literals observed in source, never near-name matching of prose.
fn replay(text: &str, edits: &BTreeMap<String, Replacement>) -> Option<(String, BTreeSet<String>)> {
    if edits.is_empty() {
        return None;
    }
    let mut keys: Vec<_> = edits.keys().collect();
    keys.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let matcher = Regex::new(
        &keys
            .iter()
            .map(|s| regex::escape(s))
            .collect::<Vec<_>>()
            .join("|"),
    )
    .ok()?;
    let mut output = String::new();
    let mut cursor = 0;
    let mut used = BTreeSet::new();
    for hit in matcher.find_iter(text) {
        let edit = edits.get(hit.as_str())?;
        let first = edit.old.chars().next()?;
        let last = edit.old.chars().last()?;
        let before = text[..hit.start()].chars().next_back();
        let after = text[hit.end()..].chars().next();
        let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
        if first.is_ascii_alphanumeric() && before.is_some_and(word)
            || last.is_ascii_alphanumeric() && after.is_some_and(word)
            || edit.model && super::model_has_continuation(&text[hit.end()..])
        {
            continue;
        }
        output.push_str(&text[cursor..hit.start()]);
        output.push_str(&edit.new);
        cursor = hit.end();
        used.insert(edit.old.clone());
    }
    output.push_str(&text[cursor..]);
    Some((output, used))
}

fn merge(edits: &mut BTreeMap<String, Replacement>, edit: &Replacement) -> Option<()> {
    if edit.old.is_empty() || edit.old == edit.new {
        return None;
    }
    if let Some(old) = edits.get_mut(&edit.old) {
        if old.new != edit.new {
            return None;
        }
        old.model |= edit.model;
    } else {
        edits.insert(edit.old.clone(), edit.clone());
    }
    Some(())
}

fn source_matches(source: &Source, segment: &Segment, meeting: &Meeting) -> bool {
    source.segment_id == segment.id
        && source.revision == segment.revision
        && source.quote == segment.text
        && source.start_ms == segment.start_ms
        && source.end_ms == segment.end_ms
        && source.speaker_name
            == segment
                .speaker
                .as_ref()
                .and_then(|id| meeting.speaker_names.get(id))
                .cloned()
        && segment.meeting_id == meeting.id
        && segment.run == meeting.run
}

fn repair(
    before: &Suggestion,
    meeting: &Meeting,
    current_sources: &BTreeMap<String, Segment>,
    changes: &[SourceChange],
    allow_unchanged: bool,
) -> Option<Suggestion> {
    if before.meeting_id != meeting.id
        || before.status != "pending"
        || before.stale
        || before.sources.is_empty()
    {
        return None;
    }
    let mut edits = BTreeMap::new();
    let mut updated = before.clone();
    let mut changed = false;
    for source in &mut updated.sources {
        let current = current_sources.get(&source.segment_id)?;
        if let Some(change) = changes.iter().find(|c| c.before.id == source.segment_id) {
            if !source_matches(source, &change.before, meeting)
                || !source_matches(
                    &Source {
                        segment_id: change.after.id.clone(),
                        revision: change.after.revision,
                        start_ms: change.after.start_ms,
                        end_ms: change.after.end_ms,
                        quote: change.after.text.clone(),
                        speaker_name: source.speaker_name.clone(),
                    },
                    current,
                    meeting,
                )
            {
                return None;
            }
            let mut local = BTreeMap::new();
            for edit in &change.replacements {
                merge(&mut local, edit)?;
            }
            let (revised, used) = replay(&change.before.text, &local)?;
            if revised != change.after.text || used.is_empty() {
                return None;
            }
            for key in used {
                merge(&mut edits, &local[&key])?;
            }
            source.quote = current.text.clone();
            source.revision = current.revision;
            source.start_ms = current.start_ms;
            source.end_ms = current.end_ms;
            changed = true;
        } else if !source_matches(source, current, meeting) {
            return None;
        }
    }
    if !changed {
        return allow_unchanged.then(|| before.clone());
    }
    let (body, used) = replay(&before.text, &edits)?;
    if used.is_empty() {
        return None;
    }
    let used_edits = edits
        .into_iter()
        .filter(|(key, _)| used.contains(key))
        .collect();
    // Every cited use needs its own authorization. A source changed for Omega
    // must not inherit another source's ALPHA replacement inside ALPHA Beta.
    for source in &before.sources {
        let mut local = BTreeMap::new();
        if let Some(change) = changes.iter().find(|c| c.before.id == source.segment_id) {
            for edit in &change.replacements {
                if used.contains(&edit.old) {
                    merge(&mut local, edit)?;
                }
            }
        }
        let authorized = if local.is_empty() {
            source.quote.clone()
        } else {
            replay(&source.quote, &local)?.0
        };
        if replay(&source.quote, &used_edits)?.0 != authorized {
            return None;
        }
    }
    updated.text = body;
    updated.stale = false;
    Some(updated)
}

/// Plan only: whole-source recovery and supported name spelling have distinct
/// freshness contracts. This does not write, archive, or grant stale authority.
enum CanonicalReplay {
    Unchanged,
    Recovered(Suggestion),
    // Current own citations support these literal names, not the other claims
    // in a paraphrase. Keep the corrected paragraph pending factual validation.
    PendingSpelling(Suggestion),
    // A known name still needs correction (or cannot be checked in budget).
    // The old unchanged checkpoint must not certify this paragraph as current.
    Pending,
}

fn source_has_canonical_name(terms: &super::Terms, text: &str, name: &str) -> bool {
    if terms
        .names
        .get(&name.to_lowercase())
        .is_none_or(|owner| owner != name)
    {
        return false;
    }
    let model = super::model_identifier(name).is_some();
    terms
        .matcher
        .iter()
        .flat_map(|matcher| matcher.find_iter(text))
        .any(|hit| {
            // The existing longest exact matcher keeps another declared complete
            // name opaque; its shared prefix cannot authorize the shorter name.
            hit.as_str() == name
                && !text[..hit.start()]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || model && c == '_')
                && if model {
                    !super::model_has_continuation(&text[hit.end()..])
                } else {
                    !text[hit.end()..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphanumeric())
                }
        })
}

fn canonical_source_repair(
    conn: &rusqlite::Connection,
    meeting: &Meeting,
    terms: &super::Terms,
    old: &Suggestion,
    budget: &mut usize,
) -> Result<CanonicalReplay> {
    const MAX_CITATIONS: usize = 64;
    if old.meeting_id != meeting.id
        || old.status != "pending"
        || old.stale
        || old.sources.is_empty()
        || old.sources.len() > MAX_CITATIONS
    {
        return Ok(CanonicalReplay::Pending);
    }
    let Some(remaining) = budget.checked_sub(old.text.len()) else {
        *budget = 0;
        return Ok(CanonicalReplay::Pending);
    };
    *budget = remaining;
    let (body, replacements) = terms.normalize_with_changes(&old.text);
    if body == old.text || replacements.is_empty() {
        return Ok(CanonicalReplay::Unchanged);
    }
    let mut edits = BTreeMap::new();
    if !replacements
        .iter()
        .all(|replacement| merge(&mut edits, replacement).is_some())
        || replay(&old.text, &edits)
            .is_none_or(|(text, used)| text != body || used.len() != edits.len())
    {
        return Ok(CanonicalReplay::Pending);
    }
    let mut seen = BTreeSet::new();
    let mut witnesses = 0usize;
    let mut supported_names = BTreeSet::new();
    for source in &old.sources {
        if !seen.insert(&source.segment_id) {
            return Ok(CanonicalReplay::Pending);
        }
        let value: Option<String> = conn
            .query_row(
                "SELECT json FROM segments WHERE id=?1 AND meeting_id=?2 AND run=?3",
                params![source.segment_id, meeting.id, meeting.run],
                |row| row.get(0),
            )
            .optional()?;
        let origin: Option<String> = conn
            .query_row(
                "SELECT origin FROM segment_text_edits WHERE id=?1 ORDER BY revision DESC LIMIT 1",
                [&source.segment_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(value) = value else {
            return Ok(CanonicalReplay::Pending);
        };
        let current: Segment = serde_json::from_str(&value)?;
        let Some(remaining) = budget.checked_sub(current.text.len()) else {
            *budget = 0;
            return Ok(CanonicalReplay::Pending);
        };
        *budget = remaining;
        if origin
            .as_deref()
            .is_some_and(|origin| origin != "terminology")
            || !source_matches(source, &current, meeting)
            || terms.normalize(&current.text) != current.text
        {
            return Ok(CanonicalReplay::Pending);
        }
        if origin.is_none() {
            let original: Option<String> = conn
                .query_row(
                    "SELECT json FROM segment_history WHERE id=?1 ORDER BY revision LIMIT 1",
                    [&source.segment_id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(original) = original {
                let original: Segment = serde_json::from_str(&original)?;
                if original.text != current.text {
                    // Legacy text changes have unknown provenance.
                    return Ok(CanonicalReplay::Pending);
                }
            }
        }
        witnesses += usize::from(current.text == body);
        for edit in edits.values() {
            if source_has_canonical_name(terms, &current.text, &edit.new) {
                supported_names.insert(edit.new.as_str());
            }
        }
    }
    Ok(if witnesses == 1 {
        let mut updated = old.clone();
        updated.text = body;
        CanonicalReplay::Recovered(updated)
    } else if edits.values().all(|edit| {
        terms.writer_names.contains(&edit.new) && supported_names.contains(edit.new.as_str())
    }) {
        let mut updated = old.clone();
        updated.text = body;
        updated.stale = true;
        CanonicalReplay::PendingSpelling(updated)
    } else {
        CanonicalReplay::Pending
    })
}

impl Store {
    /// No source changed; use the same proof, archive and transaction as mixed
    /// source replay. No independent second document write is permitted.
    pub(super) fn repair_canonical_source_documents(
        &mut self,
        meeting: &Meeting,
        terms: &super::Terms,
    ) -> Result<usize> {
        anyhow::ensure!(!self.conn.is_autocommit(), "名称同步需要当前术语事务");
        let before = self.suggestions(&meeting.id)?;
        self.repair_terminology_documents(meeting, &before, &[], None, Some(terms))
    }

    pub(crate) fn note_spelling_edit(
        &self,
        meeting: &Meeting,
        notes: &[UserNote],
        saved: &UserNote,
    ) -> Result<Option<NoteSpellingEdit>> {
        let active = crate::notes::document_notes(notes);
        let Some(before) = active.first() else {
            return Ok(None);
        };
        if before.id != saved.id
            || before.kind != "document"
            || saved.kind != "document"
            || before.anchor_ms != saved.anchor_ms
            || before.content == saved.content
            || meeting.notes_fingerprint.is_empty()
            || meeting.notes_cursor == 0
        {
            return Ok(None);
        }
        let sections = self.suggestions(&meeting.id)?;
        if sections.is_empty()
            || sections
                .iter()
                .any(|s| s.status != "pending" || s.stale || s.sources.is_empty())
        {
            return Ok(None);
        }
        Ok(Some(NoteSpellingEdit {
            meeting: meeting.clone(),
            notes: notes.to_vec(),
            sections,
            saved: saved.clone(),
        }))
    }

    pub(super) fn repair_terminology_documents(
        &mut self,
        meeting: &Meeting,
        before: &[Suggestion],
        changes: &[SourceChange],
        reusable_fingerprint: Option<String>,
        canonical_terms: Option<&super::Terms>,
    ) -> Result<usize> {
        if changes.is_empty() && canonical_terms.is_none() {
            return Ok(0);
        }
        let tx = self.conn.savepoint()?;
        let mut current_meeting: Meeting = serde_json::from_str(&tx.query_row(
            "SELECT json FROM meetings WHERE id=?1",
            [&meeting.id],
            |r| r.get::<_, String>(0),
        )?)?;
        if serde_json::to_string(&current_meeting)? != serde_json::to_string(meeting)? {
            return Ok(0);
        }
        let mut query =
            tx.prepare("SELECT json FROM suggestions WHERE meeting_id=?1 ORDER BY rowid")?;
        let existing: Vec<Suggestion> = query
            .query_map([&meeting.id], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect::<Result<_>>()?;
        drop(query);
        let mut repaired = Vec::new();
        let mut canonical_budget = 120_000usize;
        let mut canonical_recovered = false;
        let mut pending_spelling = false;
        for old in before.iter().filter(|s| s.status == "pending" && !s.stale) {
            let Some(current) = existing.iter().find(|s| s.id == old.id) else {
                continue;
            };
            let mut expected = old.clone();
            expected.stale = current.stale;
            if serde_json::to_string(&expected)? != serde_json::to_string(current)? {
                continue;
            }
            let mut protected = false;
            let replayed = if changes.is_empty() {
                // The source-change replay cannot contribute in this case.
                // Let the bounded name plan check whether any source is needed
                // before reading or decoding its citation records.
                None
            } else {
                let mut sources = BTreeMap::new();
                for source in &old.sources {
                    let value: Option<String> = tx
                        .query_row(
                            "SELECT json FROM segments WHERE id=?1 AND meeting_id=?2 AND run=?3",
                            params![source.segment_id, meeting.id, meeting.run],
                            |r| r.get(0),
                        )
                        .optional()?;
                    let origin: Option<String> = tx.query_row(
                        "SELECT origin FROM segment_text_edits WHERE id=?1 ORDER BY revision DESC LIMIT 1",
                        [&source.segment_id], |r| r.get(0),
                    ).optional()?;
                    if origin.as_deref() == Some("human") {
                        protected = true;
                        break;
                    }
                    if let Some(value) = value {
                        let segment: Segment = serde_json::from_str(&value)?;
                        sources.insert(segment.id.clone(), segment);
                    }
                }
                if protected {
                    None
                } else {
                    repair(
                        old,
                        &current_meeting,
                        &sources,
                        changes,
                        reusable_fingerprint.is_some(),
                    )
                }
            };
            if !protected {
                // Only a paragraph fresh in the captured pre-change document
                // reaches this point. The current equality above permits this
                // transaction's stale marker, never preexisting stale/human text.
                let canonical = canonical_terms
                    .map(|terms| {
                        canonical_source_repair(
                            &tx,
                            &current_meeting,
                            terms,
                            replayed.as_ref().unwrap_or(old),
                            &mut canonical_budget,
                        )
                    })
                    .transpose()?
                    .unwrap_or(CanonicalReplay::Unchanged);
                match canonical {
                    CanonicalReplay::Recovered(section) => {
                        canonical_recovered = true;
                        repaired.push(section);
                    }
                    CanonicalReplay::PendingSpelling(section) => {
                        pending_spelling = true;
                        repaired.push(section);
                    }
                    CanonicalReplay::Unchanged => {
                        if let Some(section) = replayed {
                            repaired.push(section);
                        }
                    }
                    CanonicalReplay::Pending => {}
                }
            }
        }
        if !repaired.is_empty() {
            tx.execute(
                "INSERT INTO document_history(meeting_id,created_at,json) VALUES(?1,?2,?3)",
                params![
                    meeting.id,
                    chrono::Utc::now().to_rfc3339(),
                    serde_json::to_string(&existing)?
                ],
            )?;
            for section in &repaired {
                tx.execute(
                    "UPDATE suggestions SET json=?2 WHERE id=?1 AND meeting_id=?3",
                    params![section.id, serde_json::to_string(section)?, meeting.id],
                )?;
            }
            if pending_spelling {
                current_meeting.notes_fingerprint.clear();
                tx.execute(
                    "UPDATE meetings SET json=?2 WHERE id=?1",
                    params![meeting.id, serde_json::to_string(&current_meeting)?],
                )?;
            }
            // Neither whole-source recovery nor pending name spelling proves
            // reuse of the old checkpoint. Leave regeneration pending.
            if !canonical_recovered
                && !pending_spelling
                && repaired.len() == before.len()
                && existing.len() == before.len()
            {
                if let Some(fingerprint) = reusable_fingerprint {
                    current_meeting.notes_fingerprint = fingerprint;
                    tx.execute(
                        "UPDATE meetings SET json=?2 WHERE id=?1",
                        params![meeting.id, serde_json::to_string(&current_meeting)?],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(repaired.len())
    }
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;
    use crate::{notes, Engine, Snapshot};
    use std::sync::Arc;

    const NOTE: &str = "**CEDARLEDGER**\n\nThe test and budget remain pending.";
    const SOURCE: &str =
        "CEDARLEDGER has not completed the local test. The budget has not been approved.";

    fn seed() -> Result<(tempfile::TempDir, Arc<Engine>, UserNote, Snapshot)> {
        let root = tempfile::tempdir()?;
        let engine = Engine::open(root.path().into())?;
        let meeting = engine.create_meeting("Spelling checkpoint transaction")?;
        engine.update_meeting(&meeting.id, &meeting.title, false)?;
        let note = engine.save_note(&meeting.id, None, NOTE, "document", 0)?;
        let mut sources = Vec::new();
        for (seq, text) in [(1, SOURCE), (2, "The microphone trial is still pending.")] {
            let source = Segment {
                id: format!("{}:1:{seq}", meeting.id),
                meeting_id: meeting.id.clone(),
                run: 1,
                seq,
                start_ms: u64::from(seq - 1) * 1000,
                end_ms: u64::from(seq) * 1000,
                text: text.into(),
                revision: 1,
                speaker: None,
                speaker_manual: false,
            };
            engine.store.lock().unwrap().save_segment(&source)?;
            sources.push(source);
        }
        let snapshot = engine.snapshot(&meeting.id)?;
        let sections = sources
            .iter()
            .map(|source| Suggestion {
                id: String::new(),
                meeting_id: meeting.id.clone(),
                kind: "section".into(),
                focus: String::new(),
                text: source.text.clone(),
                status: "pending".into(),
                stale: false,
                sources: vec![Source {
                    segment_id: source.id.clone(),
                    revision: source.revision,
                    start_ms: source.start_ms,
                    end_ms: source.end_ms,
                    quote: source.text.clone(),
                    speaker_name: None,
                }],
            })
            .collect();
        engine
            .store
            .lock()
            .unwrap()
            .commit_document(&snapshot, &sources, 2, sections, false)?;
        let clean = engine.snapshot(&meeting.id)?;
        assert!(!notes::needs_update(&clean));
        Ok((root, engine, note, clean))
    }

    fn history(engine: &Engine, meeting: &str) -> Result<Vec<String>> {
        let db = engine.store.lock().unwrap();
        let mut query = db
            .conn
            .prepare("SELECT json FROM document_history WHERE meeting_id=?1 ORDER BY id")?;
        let rows = query.query_map([meeting], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    #[test]
    fn captured_note_checkpoint_cannot_ack_intervening_context_or_article_changes() -> Result<()> {
        for race in ["note", "source", "article", "dirty"] {
            let (_root, engine, note, clean) = seed()?;
            let mut saved = note.clone();
            saved.content = NOTE.replace("CEDARLEDGER", "CedarLedger");
            saved.updated_at = chrono::Utc::now().to_rfc3339();
            let checkpoint = {
                let db = engine.store.lock().unwrap();
                let checkpoint = db.note_spelling_edit(&clean.meeting, &clean.notes, &saved)?;
                assert!(checkpoint.is_some());
                db.save_note(&saved)?;
                checkpoint
            };
            // Interleave a real store mutation in save_note's unlocked window.
            match race {
                "note" => {
                    saved
                        .content
                        .push_str("\nPriority: review the microphone first.");
                    engine.store.lock().unwrap().save_note(&saved)?;
                }
                "source" => engine.edit_segment(
                    &clean.segments[1].id,
                    "The microphone trial is now complete.",
                    1,
                )?,
                "article" => engine.store.lock().unwrap().update_suggestion(
                    &note.meeting_id,
                    &clean.suggestions[1].id,
                    "pending",
                    Some("Human review: keep this separate microphone conclusion.".into()),
                )?,
                "dirty" => {
                    let mut meeting = clean.meeting.clone();
                    meeting.notes_fingerprint.clear();
                    engine.store.lock().unwrap().save_meeting(&meeting)?;
                }
                _ => unreachable!(),
            }
            assert_eq!(
                engine.correct_terminology_after_note(&note.meeting_id, checkpoint)?,
                1
            );
            let after = engine.snapshot(&note.meeting_id)?;
            assert_eq!(
                after.segments[0].text,
                SOURCE.replace("CEDARLEDGER", "CedarLedger")
            );
            assert_eq!(after.segments[0].revision, 2);
            assert_eq!(after.meeting.notes_cursor, 2);
            assert!(after.meeting.notes_fingerprint.is_empty(), "{race}");
            assert!(notes::needs_update(&after), "{race}");
            if race == "note" {
                assert_eq!(after.notes[0].content, saved.content);
            } else if race == "source" {
                assert_eq!(
                    after.segments[1].text,
                    "The microphone trial is now complete."
                );
                assert_eq!(after.segments[1].revision, 2);
            } else if race == "article" {
                assert_eq!(after.suggestions[1].status, "accepted");
                assert_eq!(
                    after.suggestions[1].text,
                    "Human review: keep this separate microphone conclusion."
                );
            }
        }
        Ok(())
    }

    #[test]
    fn rejected_fingerprint_ack_rolls_back_article_sources_and_history() -> Result<()> {
        let (_root, engine, note, before) = seed()?;
        let old_history = history(&engine, &note.meeting_id)?;
        let table_state = || -> Result<Vec<Vec<String>>> {
            let db = engine.store.lock().unwrap();
            let mut state = Vec::new();
            for table in [
                "segment_history",
                "segment_text_edits",
                "term_queue",
                "term_checks",
                "cache_dependencies",
            ] {
                let mut query = db
                    .conn
                    .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))?;
                let columns = query.column_count();
                for row in query.query_map([], |row| {
                    (0..columns)
                        .map(|column| {
                            row.get::<_, rusqlite::types::Value>(column)
                                .map(|v| format!("{v:?}"))
                        })
                        .collect::<rusqlite::Result<Vec<_>>>()
                })? {
                    let mut row = row?;
                    row.insert(0, table.to_owned());
                    state.push(row);
                }
            }
            Ok(state)
        };
        let old_tables = table_state()?;
        engine.store.lock().unwrap().conn.execute_batch(
            "CREATE TRIGGER reject_spelling_ack BEFORE UPDATE ON meetings
             WHEN COALESCE(json_extract(NEW.json,'$.notes_fingerprint'),'') != ''
             BEGIN SELECT RAISE(ABORT,'fixture fingerprint acknowledgement failed'); END;",
        )?;
        let error = engine
            .save_note(
                &note.meeting_id,
                Some(note.id),
                &NOTE.replace("CEDARLEDGER", "CedarLedger"),
                "document",
                0,
            )
            .expect_err("the new nonempty acknowledgement must fail after clear succeeds");
        assert!(format!("{error:#}").contains("fixture fingerprint acknowledgement failed"));
        let after = engine.snapshot(&note.meeting_id)?;
        assert_eq!(
            after.meeting.notes_fingerprint,
            before.meeting.notes_fingerprint
        );
        assert_eq!(after.meeting.notes_cursor, before.meeting.notes_cursor);
        assert!(notes::needs_update(&after));
        assert_eq!(
            after.notes[0].content,
            NOTE.replace("CEDARLEDGER", "CedarLedger")
        );
        assert_eq!(
            serde_json::to_value(&after.segments)?,
            serde_json::to_value(&before.segments)?
        );
        assert_eq!(
            serde_json::to_value(&after.suggestions)?,
            serde_json::to_value(&before.suggestions)?
        );
        assert_eq!(table_state()?, old_tables);
        assert_eq!(history(&engine, &note.meeting_id)?, old_history);
        assert!(engine.store.lock().unwrap().conn.is_autocommit());
        let jobs: i64 = engine.store.lock().unwrap().conn.query_row(
            "SELECT COUNT(*) FROM inference_jobs",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(jobs, 0);
        Ok(())
    }
}
