//! Reuse revision/environment-validated caption translations for unchanged prompt
//! passages. Citations and all fact checks continue to use the original speech.
use crate::{translation::Translation, Segment};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

pub(super) fn cached_text<'a>(
    source: &Segment,
    active_text: &str,
    cached: &'a Translation,
    chinese: bool,
) -> Option<&'a str> {
    // Snapshot.translations already enforces the model/config/environment key.
    // A budget/current-task view may have removed stale clauses: never restore
    // them by substituting a translation of the original, longer passage.
    let target = if chinese { "zh" } else { "en" };
    // All-caps ASR cannot distinguish names/acronyms from prose (e.g. UN VIDEO
    // CARD). The caption fact guards do not prove such translations faithful.
    // Keep this source on the original-prose/checked-language-repair path.
    let ambiguous_caps = source.text.chars().filter(char::is_ascii_uppercase).count() > 16
        && !source
            .text
            .chars()
            .any(|c| c.is_ascii_lowercase() || ('\u{3400}'..='\u{9fff}').contains(&c));
    (active_text == source.text
        && !ambiguous_caps
        && cached.segment_id == source.id
        && cached.revision == source.revision
        && cached.target == target
        && !cached.cache_key.is_empty()
        && !cached.text.trim().is_empty()
        && crate::terminology::preserves_owner_spelling(&source.text, &cached.text)
        && super::writer::validate_language(&cached.text, chinese).is_ok()
        && super::validation::validate_translation(&cached.text, source, &source.meeting_id)
            .is_ok())
    .then_some(cached.text.as_str())
}

/// Count substitutions in the actual token-budgeted prompt, without logging text.
pub(super) fn reused_count(input: &str, sources: &[Segment], snapshot: &crate::Snapshot) -> usize {
    let Ok(input) = serde_json::from_str::<Value>(input) else {
        return 0;
    };
    input["transcript"].as_array().map_or(0, |passages| {
        passages
            .iter()
            .filter(|p| {
                p["id"]
                    .as_u64()
                    .and_then(|id| id.checked_sub(1))
                    .and_then(|id| sources.get(id as usize))
                    .and_then(|s| snapshot.translations.get(&s.id).map(|t| (s, t)))
                    .is_some_and(|(s, t)| {
                        t.text != s.text && p["text"].as_str() == Some(t.text.as_str())
                    })
            })
            .count()
    })
}

/// Used only after the normal fact/name/citation checks accept a draft and its
/// sole remaining failure is output language. This is the existing second
/// attempt, with no extra model, worker, retry or transcript echo in the prompt.
pub(super) fn repair_messages(checked_document: &str, chinese: bool) -> Result<Vec<Value>> {
    let document: Value = serde_json::from_str(checked_document)?;
    let body = document["sections"]
        .as_array()
        .context("Missing checked document")?
        .iter()
        .map(|s| s["body"].as_str().context("Missing checked body"))
        .collect::<Result<Vec<_>>>()?
        .join("\n\n");
    let system = crate::translation::SYSTEM_TEMPLATE.replace(
        "{language}",
        if chinese {
            "Simplified Chinese"
        } else {
            "English"
        },
    );
    Ok(vec![
        json!({"role":"system","content":system}),
        json!({"role":"user","content":super::amounts::normalize(&body)}),
    ])
}

pub(super) fn repaired_body(raw: &str) -> Result<String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Output {
        translation: String,
    }
    let parsed: Output =
        serde_json::from_str(raw).context("Invalid translated minutes response")?;
    let text = parsed.translation.trim();
    if text.is_empty() || text.chars().count() > 4000 {
        bail!("Invalid translated minutes length");
    }
    Ok(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captions_cannot_restore_retired_clauses_or_bypass_source_fact_checks() {
        let mut source = Segment {
            id: "s".into(),
            meeting_id: "m".into(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 1,
            revision: 1,
            text: "The budget is 3000 yuan and has not been approved.".into(),
            speaker: None,
            speaker_manual: false,
        };
        let mut cached = Translation {
            segment_id: "s".into(),
            revision: 1,
            target: "zh".into(),
            model: "local".into(),
            cache_key: "validated-snapshot-key".into(),
            text: "预算3000元尚未批准。".into(),
        };
        assert_eq!(
            cached_text(&source, &source.text, &cached, true),
            Some(cached.text.as_str())
        );
        assert!(cached_text(&source, "", &cached, true).is_none());
        assert!(cached_text(&source, "It has not been approved.", &cached, true).is_none());
        assert!(cached_text(&source, &source.text, &cached, false).is_none());
        source.revision += 1;
        assert!(cached_text(&source, &source.text, &cached, true).is_none());
        cached.revision = source.revision;
        cached.text = "预算5000元尚未批准。".into();
        assert!(cached_text(&source, &source.text, &cached, true).is_none());
        cached.text = "预算3000元已获批准。".into();
        assert!(cached_text(&source, &source.text, &cached, true).is_none());
        source.text = "ALICE WILL CHECK THE UN VIDEO CARD".into();
        cached.text = "艾丽丝将检查未授权的视频卡。".into();
        assert!(cached_text(&source, &source.text, &cached, true).is_none());
    }
    #[test]
    fn language_repair_has_only_checked_prose_and_strict_translation_shape() {
        let checked = r#"{"sections":[{"body":"The budget has not been officially approved.","sources":[1]}]}"#;
        let messages = repair_messages(checked, true).unwrap();
        assert_eq!(
            messages[1]["content"],
            "The budget has not been officially approved."
        );
        assert!(messages[0]["content"]
            .as_str()
            .unwrap()
            .contains("Simplified Chinese"));
        assert_eq!(
            repaired_body(r#"{"translation":"预算尚未正式批准。"}"#).unwrap(),
            "预算尚未正式批准。"
        );
        for raw in [
            r#"{"translation":""}"#,
            r#"{"translation":"内容","transcript":[]}"#,
            "raw English",
        ] {
            assert!(repaired_body(raw).is_err());
        }
    }

    #[test]
    fn cached_prompt_keeps_original_citations_and_factual_repairs_use_original_text() -> Result<()>
    {
        let root = tempfile::tempdir()?;
        let engine = crate::Engine::open(root.path().into())?;
        let meeting = engine.create_meeting("Prompt surfaces")?;
        let mut snapshot = engine.snapshot(&meeting.id)?;
        let source = Segment {
            id: "s".into(),
            meeting_id: meeting.id.clone(),
            run: 1,
            seq: 1,
            start_ms: 0,
            end_ms: 1,
            revision: 1,
            text: "The budget is 3000 yuan and has not been approved.".into(),
            speaker: None,
            speaker_manual: false,
        };
        let cached = Translation {
            segment_id: source.id.clone(),
            revision: 1,
            target: "zh".into(),
            model: "local".into(),
            cache_key: "validated-snapshot-key".into(),
            text: "预算3000元尚未批准。".into(),
        };
        snapshot.segments.push(source.clone());
        snapshot
            .translations
            .insert(source.id.clone(), cached.clone());
        let context = super::super::retrieval::Context {
            segments: vec![source.clone()],
            cursor: 1,
            keywords: vec![],
        };
        let (_, prompt) = super::super::writer::prompt(&snapshot, &context, &json!({}), "zh");
        assert_eq!(reused_count(&prompt, &context.segments, &snapshot), 1);
        let (_, original) =
            super::super::writer::prompt_mode(&snapshot, &context, &json!({}), "zh", false);
        assert_eq!(reused_count(&original, &context.segments, &snapshot), 0);
        assert_eq!(
            serde_json::from_str::<Value>(&original)?["transcript"][0]["text"],
            source.text
        );
        let document =
            super::super::writer::document(&cached.text, &context.segments, &Default::default())?;
        let validated =
            super::super::validation::validate(&document, &context.segments, &meeting.id)?;
        assert_eq!(validated[0].sources[0].quote, source.text);
        assert_eq!(validated[0].sources[0].revision, source.revision);
        Ok(())
    }
}
