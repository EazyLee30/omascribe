//! A meeting/run-local spelling digest. Persist only dependency metadata and
//! hashes; unchanged UI/cache queries must not rescan historical source text.
use crate::{store::Store, terminology::Terms, Meeting};
use anyhow::{bail, Context, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const KEY: &str = "source-actor-context-v1";
const GENERATION: &str = "source-generation";
const SOURCE_TEXTS_SQL: &str = "SELECT json_extract(json,'$.text') FROM segments WHERE meeting_id=?1 AND run=?2 AND json_extract(json,'$.meeting_id')=?1 AND json_extract(json,'$.run')=?2 ORDER BY seq";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Cached {
    schema: u32,
    run: u32,
    generation: String,
    manual_digest: String,
    digest: String,
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn generation(db: &Store, meeting: &str) -> Result<String> {
    db.conn.query_row(
        "SELECT CASE WHEN EXISTS(SELECT 1 FROM deleted_meetings WHERE id=?1) THEN NULL ELSE COALESCE((SELECT value FROM cache_dependencies WHERE meeting_id=?1 AND kind=?2),'0') END",
        params![meeting, GENERATION],
        |r| r.get::<_, Option<String>>(0),
    )?.context("Source context belongs to a removed meeting")
}
fn scan(db: &Store, meeting: &Meeting) -> Result<String> {
    // The meeting/run index excludes other meetings and previous recognition
    // runs. This full derivation occurs only after a relevant generation change.
    let mut stmt = db.conn.prepare(SOURCE_TEXTS_SQL)?;
    let mut rows = stmt.query_map(params![meeting.id, meeting.run], |r| r.get::<_, String>(0))?;
    let mut first_error = None;
    let terms = Terms::new(&[], &[]).with_actor_spellings(
        rows.by_ref().map_while(|row| match row {
            Ok(text) => Some(text),
            Err(error) => {
                first_error = Some(error);
                None
            }
        }),
        meeting.speaker_names.values().map(String::as_str),
    );
    if let Some(error) = first_error {
        return Err(error.into());
    }
    // Extraction stops at the existing actor budget, but every remaining row
    // must still pass the original String conversion before sealing a digest.
    // Keep only the current row, not every historical source's owned text.
    for row in rows {
        drop(row?);
    }
    Ok(terms.actor_spellings_digest())
}
pub(crate) fn digest(db: &Store, meeting: &Meeting) -> Result<String> {
    digest_with(db, meeting, || scan(db, meeting))
}
fn digest_with(
    db: &Store,
    meeting: &Meeting,
    derive: impl Fn() -> Result<String>,
) -> Result<String> {
    let manual_digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&meeting.speaker_names)?)
    );
    for _ in 0..2 {
        let before = generation(db, &meeting.id)?;
        // Bound corrupted dependency records before materializing/parsing them.
        let cached: Option<String> = db.conn.query_row(
            "SELECT value FROM cache_dependencies WHERE meeting_id=?1 AND kind=?2 AND length(value)<=2048",
            params![meeting.id, KEY], |r| r.get(0),
        ).optional()?;
        if let Some(cached) = cached
            .as_deref()
            .and_then(|s| serde_json::from_str::<Cached>(s).ok())
        {
            if cached.schema == 1
                && cached.run == meeting.run
                && cached.generation == before
                && cached.manual_digest == manual_digest
                && valid_digest(&cached.digest)
                && generation(db, &meeting.id)? == before
            {
                return Ok(cached.digest);
            }
        }
        let digest = derive()?;
        if generation(db, &meeting.id)? != before {
            continue;
        }
        let cached = serde_json::to_string(&Cached {
            schema: 1,
            run: meeting.run,
            generation: before.clone(),
            manual_digest: manual_digest.clone(),
            digest: digest.clone(),
        })?;
        // A separate connection may update source text between the scan and
        // this write. Never seal the old digest under the new generation.
        db.conn.execute(
            "INSERT INTO cache_dependencies SELECT ?1,?2,?3 WHERE COALESCE((SELECT value FROM cache_dependencies WHERE meeting_id=?1 AND kind=?4),'0')=?5 AND NOT EXISTS(SELECT 1 FROM deleted_meetings WHERE id=?1) ON CONFLICT(meeting_id,kind) DO UPDATE SET value=excluded.value WHERE value!=excluded.value",
            params![meeting.id, KEY, cached, GENERATION, before],
        )?;
        if generation(db, &meeting.id)? == before {
            return Ok(digest);
        }
    }
    bail!("Source context changed during spelling derivation; pending work retained")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Segment;
    use std::{cell::Cell, collections::BTreeMap};
    fn meeting(id: &str, run: u32) -> Meeting {
        Meeting {
            id: id.into(),
            title: "Fictional source context".into(),
            created_at: "now".into(),
            status: "ready".into(),
            duration_ms: 0,
            run,
            notes_cursor: 0,
            auto_notes: false,
            notes_fingerprint: String::new(),
            diarization: false,
            diarization_cursor: 0,
            speaker_names: BTreeMap::new(),
        }
    }
    fn source(m: &Meeting, id: &str, seq: u32, text: &str) -> Segment {
        Segment {
            id: id.into(),
            meeting_id: m.id.clone(),
            run: m.run,
            seq,
            start_ms: 0,
            end_ms: 1000,
            text: text.into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        }
    }

    // The former production path is retained only as an equivalence oracle.
    fn eager_reference(db: &Store, meeting: &Meeting) -> Result<String> {
        let mut stmt = db.conn.prepare(SOURCE_TEXTS_SQL)?;
        let texts = stmt
            .query_map(params![meeting.id, meeting.run], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Terms::new(&[], &[])
            .with_actor_spellings(
                texts.iter().map(String::as_str),
                meeting.speaker_names.values().map(String::as_str),
            )
            .actor_spellings_digest())
    }

    fn full_actor_prefix() -> String {
        (b'A'..=b'L')
            .map(|letter| format!("测试任务由Actor{}负责检查。", char::from(letter)))
            .collect::<String>()
    }

    #[test]
    fn streaming_scan_preserves_eager_source_order_and_manual_anchor_semantics() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let db = Store::open(&temp.path().join("streaming.sqlite3"))?;
        let mut m = meeting("manual-order", 1);
        m.speaker_names.insert("speaker-2".into(), "林悦".into());
        m.speaker_names.insert("speaker-1".into(), "Morgan".into());
        m.speaker_names
            .insert("speaker-3".into(), "NotSpoken".into());
        for (index, text) in [
            "这是没有名称的普通转写。",
            "林悦先在周四提交测试报告。",
            "Riley will run the review. Morgan owns the original report.",
            "Riley will run the review. MORGAN CONFIRMS THE UPLOAD IS COMPLETE.",
            "离线转写工具由张敬先负责测试。",
        ]
        .iter()
        .enumerate()
        {
            db.save_segment(&source(
                &m,
                &format!("manual-{index}"),
                index as u32 + 1,
                text,
            ))?;
        }
        assert_eq!(scan(&db, &m)?, eager_reference(&db, &m)?);
        assert_eq!(digest(&db, &m)?, eager_reference(&db, &m)?);

        let capped = meeting("capped-order", 1);
        let prefix = full_actor_prefix();
        assert_eq!(
            Terms::new(&[], &[])
                .with_actor_spellings([prefix.as_str()], std::iter::empty::<&str>())
                .actor_spellings()
                .len(),
            12
        );
        db.save_segment(&source(&capped, "capped-first", 1, &prefix))?;
        let prefix_digest = scan(&db, &capped)?;
        for index in 0..64 {
            db.save_segment(&source(
                &capped,
                &format!("capped-tail-{index}"),
                index + 2,
                "Nora will run the review. 这部分晚于已满的名称上下文。",
            ))?;
        }
        assert_eq!(scan(&db, &capped)?, prefix_digest);
        assert_eq!(scan(&db, &capped)?, eager_reference(&db, &capped)?);
        Ok(())
    }

    #[test]
    fn invalid_tail_rows_after_actor_cap_cannot_seal_a_digest() -> Result<()> {
        for kind in ["null", "number", "malformed"] {
            let temp = tempfile::tempdir()?;
            let db = Store::open(&temp.path().join("invalid-tail.sqlite3"))?;
            let m = meeting(kind, 1);
            db.save_segment(&source(&m, "full-prefix", 1, &full_actor_prefix()))?;
            let tail = source(&m, "invalid-tail", 2, "Nora will run the review.");
            db.save_segment(&tail)?;
            digest(&db, &m)?;
            let before = db.cache_dependency(&m.id, KEY)?;
            let before_generation = generation(&db, &m.id)?;
            let damaged = if kind == "malformed" {
                // Simulate an already-corrupted/imported database. The normal
                // term-queue trigger rejects malformed JSON before persistence.
                db.conn.execute_batch("DROP TRIGGER term_source_update;")?;
                "{malformed historical source".to_owned()
            } else {
                let mut value = serde_json::to_value(&tail)?;
                value["text"] = if kind == "null" {
                    serde_json::Value::Null
                } else {
                    42.into()
                };
                serde_json::to_string(&value)?
            };
            db.conn.execute(
                "UPDATE segments SET json=?2 WHERE id=?1",
                params![tail.id, damaged],
            )?;
            assert_ne!(generation(&db, &m.id)?, before_generation, "{kind}");
            let expected = eager_reference(&db, &m).unwrap_err().to_string();
            assert_eq!(scan(&db, &m).unwrap_err().to_string(), expected, "{kind}");
            assert!(digest(&db, &m).is_err(), "{kind}");
            assert_eq!(db.cache_dependency(&m.id, KEY)?, before, "{kind}");
            assert!(digest(&db, &m).is_err(), "{kind}: bad state was sealed");
        }
        Ok(())
    }

    #[test]
    fn streaming_scan_preserves_the_first_row_error_before_the_actor_cap() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let db = Store::open(&temp.path().join("first-error.sqlite3"))?;
        let m = meeting("first-error", 1);
        for (index, bad_text) in [serde_json::Value::Null, 42.into()].into_iter().enumerate() {
            let source = source(&m, &format!("invalid-{index}"), index as u32 + 1, "text");
            db.save_segment(&source)?;
            let mut value = serde_json::to_value(&source)?;
            value["text"] = bad_text;
            db.conn.execute(
                "UPDATE segments SET json=?2 WHERE id=?1",
                params![source.id, serde_json::to_string(&value)?],
            )?;
        }
        let expected = eager_reference(&db, &m).unwrap_err().to_string();
        assert_eq!(scan(&db, &m).unwrap_err().to_string(), expected);
        assert!(digest(&db, &m).is_err());
        assert!(db.cache_dependency(&m.id, KEY)?.is_empty());
        Ok(())
    }

    #[test]
    fn cache_hits_load_no_source_and_unrelated_changes_keep_actor_identity() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let db = Store::open(&temp.path().join("source.sqlite3"))?;
        let m = meeting("current", 1);
        let mut s = source(&m, "one", 1, "离线转写工具由林悦负责测试。");
        db.save_segment(&s)?;
        let expected = digest(&db, &m)?;
        let changes = db.conn.total_changes();
        assert_eq!(
            digest_with(&db, &m, || bail!("Cache hit must not load sources"))?,
            expected
        );
        assert_eq!(db.conn.total_changes(), changes);
        let old_generation = generation(&db, &m.id)?;
        db.save_segment(&s)?;
        assert_eq!(generation(&db, &m.id)?, old_generation);
        assert_eq!(
            digest_with(&db, &m, || bail!("Unchanged JSON must not rescan"))?,
            expected
        );

        db.save_segment(&source(&m, "two", 2, "缓存保存在本机。"))?;
        assert_ne!(generation(&db, &m.id)?, old_generation);
        assert_eq!(digest(&db, &m)?, expected);
        assert_eq!(
            digest_with(&db, &m, || bail!("Refreshed version must hit"))?,
            expected
        );
        // A revision-only change is observed even if the actor set is unchanged.
        let old_generation = generation(&db, &m.id)?;
        s.revision += 1;
        db.save_segment(&s)?;
        assert_ne!(generation(&db, &m.id)?, old_generation);
        assert_eq!(digest(&db, &m)?, expected);
        let row = db.cache_dependency(&m.id, KEY)?;
        assert!(!row.contains("林悦") && !row.contains(&s.text));
        assert!(row.len() < 512);
        Ok(())
    }
    #[test]
    fn actor_edits_deletes_runs_and_meeting_moves_do_not_reuse_context() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let db = Store::open(&temp.path().join("source.sqlite3"))?;
        let m = meeting("first", 1);
        let other = meeting("other", 1);
        let mut s = source(&m, "one", 1, "Riley will run the review.");
        db.save_segment(&s)?;
        let riley = digest(&db, &m)?;
        let empty = digest(&db, &other)?;
        assert_ne!(riley, empty);
        s.text = "Nora will perform the review.".into();
        s.revision += 1;
        db.save_segment(&s)?;
        let nora = digest(&db, &m)?;
        assert_ne!(nora, riley);
        s.run = 2;
        db.conn.execute(
            "UPDATE segments SET run=?2,json=?3 WHERE id=?1",
            params![s.id, s.run, serde_json::to_string(&s)?],
        )?;
        assert_eq!(digest(&db, &m)?, empty);
        let mut second_run = m.clone();
        second_run.run = 2;
        assert_eq!(digest(&db, &second_run)?, nora);
        assert_eq!(digest(&db, &m)?, empty);
        s.meeting_id = other.id.clone();
        s.run = other.run;
        db.conn.execute(
            "UPDATE segments SET meeting_id=?2,run=?3,json=?4 WHERE id=?1",
            params![s.id, s.meeting_id, s.run, serde_json::to_string(&s)?],
        )?;
        assert_eq!(digest(&db, &second_run)?, empty);
        assert_eq!(digest(&db, &other)?, nora);
        db.conn
            .execute("DELETE FROM segments WHERE id=?1", [&s.id])?;
        assert_eq!(digest(&db, &other)?, empty);
        Ok(())
    }
    #[test]
    fn manual_bindings_are_scoped_and_generation_changes_cannot_seal_stale_values() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let db = Store::open(&temp.path().join("source.sqlite3"))?;
        let mut m = meeting("manual", 1);
        let s = source(&m, "one", 1, "林悦先在周四提交报告。张敬先在周五提交报告。");
        db.save_segment(&s)?;
        let empty = digest(&db, &m)?;
        m.speaker_names.insert("speaker-1".into(), "林悦".into());
        let first = digest(&db, &m)?;
        assert_ne!(first, empty);
        m.speaker_names.insert("speaker-1".into(), "张敬先".into());
        assert_ne!(digest(&db, &m)?, first);
        db.set_cache_dependency(&m.id, KEY, "corrupt dependency")?;
        let calls = Cell::new(0);
        let actual = digest_with(&db, &m, || {
            let derived = scan(&db, &m)?;
            if calls.get() == 0 {
                let mut changed = s.clone();
                changed.text = "Nora will perform the review.".into();
                changed.revision += 1;
                db.save_segment(&changed)?;
            }
            calls.set(calls.get() + 1);
            Ok(derived)
        })?;
        assert_eq!(calls.get(), 2);
        assert_eq!(actual, digest(&db, &m)?);
        assert_eq!(actual, scan(&db, &m)?);
        // Returning to a current run with no bound actor never sees an old label.
        m.run = 2;
        assert_eq!(digest(&db, &m)?, empty);
        Ok(())
    }
}
