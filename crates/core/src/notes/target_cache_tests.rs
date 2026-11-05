//! Real Engine/Store cache inputs; tiny runtime bytes are observed, never run.
//! These controls do not claim translation-model quality or GPU qualification.
use super::*;
use crate::{minutes_schedule::InputDigest, preferences::Preferences, Meeting, Segment, Snapshot};
use std::{path::PathBuf, sync::Arc};

const ENGLISH: &str = "Alice will check the report on Thursday.";
const CHINESE: &str = "Alice将在周四检查报告。";

struct Fixture {
    _root: tempfile::TempDir,
    engine: Arc<Engine>,
    meeting: Meeting,
    source: Segment,
    prefs: Preferences,
    executable: PathBuf,
}

impl Fixture {
    fn new(mode: &str, target: &str, source_text: &str) -> Result<Self> {
        let root = tempfile::tempdir()?;
        let mut engine = Engine::open(root.path().into())?;
        let isolated = Arc::get_mut(&mut engine).context("Fresh fixture engine must be unique")?;
        isolated.models = root.path().join("no-models");
        isolated.runtime_dir = root.path().join("fixture-runtime");
        let prefs = {
            let mut prefs = engine.preferences.lock().unwrap();
            prefs.inference_backend = "cpu".into();
            prefs.translation_mode = mode.into();
            prefs.minutes_language = target.into();
            prefs.clone()
        };
        let executable = engine
            .runtime_dir
            .join("llama")
            .join(crate::platform::executable("llama-server"));
        std::fs::create_dir_all(executable.parent().unwrap())?;
        std::fs::write(&executable, b"minutes cache runtime fixture")?;
        crate::inference_cache::refresh_environment(&engine, &prefs)?;
        let meeting = engine.create_meeting("Minutes target cache control")?;
        let source = Segment {
            id: format!("{}:1:1", meeting.id),
            meeting_id: meeting.id.clone(),
            run: meeting.run,
            seq: 1,
            start_ms: 0,
            end_ms: 1000,
            text: source_text.into(),
            revision: 1,
            speaker: None,
            speaker_manual: false,
        };
        engine.store.lock().unwrap().save_segment(&source)?;
        Ok(Self {
            _root: root,
            engine,
            meeting,
            source,
            prefs,
            executable,
        })
    }

    fn save_translation(&self, target: &str, text: &str, check_text: bool) -> Result<()> {
        let db = self.engine.store.lock().unwrap();
        if check_text {
            crate::translation::validate_text(
                text,
                &self.source,
                target,
                &crate::terminology::Terms::new(&[], &[]),
            )?;
        }
        let environment =
            crate::inference_cache::environment(&self.engine, &db, &self.meeting, &self.prefs)?;
        let translation = crate::translation::Translation {
            segment_id: self.source.id.clone(),
            revision: self.source.revision,
            target: target.into(),
            model: self.prefs.llm_model.clone(),
            text: text.into(),
            cache_key: crate::inference_cache::translation_key(&environment, &self.source, target),
        };
        db.conn.execute(
            "INSERT OR REPLACE INTO translations VALUES(?1,?2,?3,?4,?5)",
            rusqlite::params![
                translation.segment_id,
                translation.revision,
                translation.target,
                translation.model,
                serde_json::to_string(&translation)?
            ],
        )?;
        Ok(())
    }

    fn public(&self) -> Result<Snapshot> {
        self.engine.snapshot(&self.meeting.id)
    }

    fn input(&self) -> Result<Snapshot> {
        input_snapshot(&self.engine, &self.public()?, &self.prefs)
    }

    fn prompt_text(&self, input: &Snapshot) -> Result<String> {
        let context = retrieval::Context {
            cursor: self.source.seq,
            segments: input.segments.clone(),
            keywords: Vec::new(),
        };
        let (_, prompt) = writer::prompt(input, &context, &json!({}), &self.prefs.minutes_language);
        let value: Value = serde_json::from_str(&prompt)?;
        Ok(value["transcript"][0]["text"]
            .as_str()
            .context("Missing prompt source")?
            .into())
    }

    fn digest(&self, input: &Snapshot) -> Result<InputDigest> {
        InputDigest::from_snapshot(
            input,
            &self.prefs,
            self.engine.context_revision.load(Ordering::SeqCst),
        )
    }

    fn assert_unused_worker(&self) -> Result<()> {
        assert_eq!(self.engine.inference_worker_status().starts, 0);
        assert!(!self.engine.inference_worker_status().leased);
        let jobs: i64 = self.engine.store.lock().unwrap().conn.query_row(
            "SELECT COUNT(*) FROM inference_jobs",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(jobs, 0);
        assert!(!self.engine.models.exists());
        Ok(())
    }

    fn assert_valid_saved_row(&self, target: &str, text: &str) -> Result<()> {
        // Prove the positive fixture is already accepted by the real cache
        // loader under its caption policy; this is not a made-up cache key.
        let mut caption_prefs = self.prefs.clone();
        caption_prefs.translation_mode = "bilingual".into();
        let db = self.engine.store.lock().unwrap();
        let cached = crate::translation::cached(&self.engine, &db, &self.meeting, &caption_prefs)?;
        let row = cached
            .get(&self.source.id)
            .context("Fixture row was not current")?;
        assert_eq!(row.target, target);
        assert_eq!(row.text, text);
        Ok(())
    }
}

#[test]
fn minutes_cache_reuses_chinese_while_caption_translation_is_off() -> Result<()> {
    let fixture = Fixture::new("off", "zh", ENGLISH)?;
    fixture.save_translation("zh", CHINESE, true)?;
    fixture.save_translation("en", ENGLISH, true)?;
    fixture.assert_valid_saved_row("zh", CHINESE)?;
    let public = fixture.public()?;
    assert!(public.translations.is_empty());
    assert!(!crate::translation::pending(&public));
    let input = fixture.input()?;
    assert_eq!(fixture.prompt_text(&input)?, CHINESE);
    assert_eq!(input.translations.len(), 1);
    assert_eq!(input.translations[&fixture.source.id].target, "zh");
    assert_eq!(public.translation_mode, input.translation_mode);
    assert!(fixture.public()?.translations.is_empty());
    fixture.assert_unused_worker()
}

#[test]
fn minutes_cache_reuses_chinese_when_caption_policy_only_targets_english() -> Result<()> {
    let fixture = Fixture::new("zh-en", "zh", ENGLISH)?;
    fixture.save_translation("zh", CHINESE, true)?;
    fixture.assert_valid_saved_row("zh", CHINESE)?;
    let public = fixture.public()?;
    assert!(public.translations.is_empty());
    assert!(!crate::translation::pending(&public));
    assert_eq!(fixture.prompt_text(&fixture.input()?)?, CHINESE);
    assert!(fixture.public()?.translations.is_empty());
    fixture.assert_unused_worker()
}

#[test]
fn minutes_cache_reuses_english_without_enabling_caption_translation() -> Result<()> {
    let fixture = Fixture::new("off", "en", CHINESE)?;
    fixture.save_translation("en", ENGLISH, true)?;
    fixture.assert_valid_saved_row("en", ENGLISH)?;
    assert!(fixture.public()?.translations.is_empty());
    let input = fixture.input()?;
    assert_eq!(fixture.prompt_text(&input)?, ENGLISH);
    assert_eq!(input.translations[&fixture.source.id].target, "en");
    assert!(fixture.public()?.translations.is_empty());
    fixture.assert_unused_worker()
}

#[test]
fn minutes_cache_target_changes_do_not_borrow_the_caption_map() -> Result<()> {
    let fixture = Fixture::new("bilingual", "en", ENGLISH)?;
    fixture.save_translation("zh", CHINESE, true)?;
    fixture.assert_valid_saved_row("zh", CHINESE)?;
    let public = fixture.public()?;
    assert_eq!(public.translations[&fixture.source.id].target, "zh");
    let input = fixture.input()?;
    assert!(input.translations.is_empty());
    assert_eq!(fixture.prompt_text(&input)?, ENGLISH);
    assert_eq!(
        fixture.public()?.translations[&fixture.source.id].target,
        "zh"
    );
    fixture.assert_unused_worker()
}

#[test]
fn minutes_cache_identity_tracks_internal_inputs_while_caption_view_stays_off() -> Result<()> {
    let fixture = Fixture::new("off", "zh", ENGLISH)?;
    fixture.save_translation("zh", CHINESE, true)?;
    let public_before = fixture.public()?;
    let first = fixture.digest(&fixture.input()?)?;
    assert_eq!(first, fixture.digest(&fixture.input()?)?);
    let mut failures = crate::minutes_schedule::LegacyFailures::default();
    for _ in 0..3 {
        failures.finished_for_input(&fixture.meeting, Some(&first), false);
    }
    assert_eq!(failures.count(&fixture.meeting), 3);
    fixture.save_translation("zh", "Alice将在周四核对报告。", true)?;
    let actual = fixture.digest(&fixture.input()?)?;
    assert_ne!(
        first, actual,
        "a changed minutes input must release an old penalty"
    );
    assert_eq!(actual, fixture.digest(&fixture.input()?)?);
    assert_eq!(
        fixture.digest(&public_before)?,
        fixture.digest(&fixture.public()?)?,
        "hidden caption state is not the minutes admission identity"
    );
    failures.observe_input(&fixture.meeting, Some(&actual));
    assert_eq!(failures.count(&fixture.meeting), 0);
    fixture.assert_unused_worker()
}

#[test]
fn minutes_cache_invalid_text_never_becomes_an_admitted_input() -> Result<()> {
    let fixture = Fixture::new("bilingual", "zh", ENGLISH)?;
    fixture.save_translation("zh", "Bob将在周四检查报告。", false)?;
    // Injected on-disk corruption has matching metadata but fails the original
    // name/source checks. No cache row can certify its own factual content.
    assert!(!fixture.public()?.translations.is_empty());
    let input = fixture.input()?;
    assert!(input.translations.is_empty());
    assert_eq!(fixture.prompt_text(&input)?, ENGLISH);
    fixture.assert_unused_worker()
}

#[test]
fn minutes_cache_keeps_existing_source_and_runtime_invalidation() -> Result<()> {
    for changed in ["source", "terms", "configuration", "runtime"] {
        let mut fixture = Fixture::new("bilingual", "zh", ENGLISH)?;
        fixture.save_translation("zh", CHINESE, true)?;
        fixture.assert_valid_saved_row("zh", CHINESE)?;
        assert_eq!(fixture.prompt_text(&fixture.input()?)?, CHINESE);
        match changed {
            "source" => {
                fixture.source.revision += 1;
                fixture
                    .engine
                    .store
                    .lock()
                    .unwrap()
                    .save_segment(&fixture.source)?;
            }
            "terms" => fixture.engine.store.lock().unwrap().set_cache_dependency(
                &fixture.meeting.id,
                "terms",
                "updated-terms",
            )?,
            "configuration" => {
                fixture.prefs.context_size = 4096;
                *fixture.engine.preferences.lock().unwrap() = fixture.prefs.clone();
            }
            "runtime" => {
                {
                    let _blocked = fixture
                        .engine
                        .inference_fingerprints
                        .block_directory(fixture.executable.parent().unwrap())?;
                }
                std::fs::remove_file(&fixture.executable)?;
            }
            _ => unreachable!(),
        }
        assert!(fixture.input()?.translations.is_empty(), "{changed}");
        fixture.assert_unused_worker()?;
    }
    Ok(())
}
