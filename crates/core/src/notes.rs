//! Automatic meeting-document pipeline: retrieve → compose → validate → transactional merge.
mod amounts;
pub(crate) use amounts::normalize as normalize_money;
pub(crate) use amounts::repair_unitless_yuan;
mod completion;
mod corrections;
#[cfg(test)]
pub(crate) mod facts;
mod generation;
pub(crate) mod incremental;
mod language;
pub(crate) use language::validate_with_source as validate_language_with_source;
mod lexical;
mod metrics;
mod qualifiers;
pub(crate) use metrics::{prefix as metric_prefix, Mismatch as MetricMismatch};
pub(crate) use qualifiers::{
    boundary as qualifier_boundary, prefix as qualifier_prefix, Mismatch as QualifierMismatch,
};
mod recording_retention;
mod relations;
pub(crate) use recording_retention::{
    anchor as retention_anchor, negative_deadline, Mismatch as RecordingRetentionMismatch,
};
mod submission;
pub(crate) use relations::anchors as report_anchors;
pub(crate) use relations::Mismatch as RelationMismatch;
pub(crate) use submission::{anchor as submission_anchor, Mismatch as SubmissionMismatch};
mod retrieval;
mod source_language;
pub(crate) mod stream;
mod uncertainty;
mod validation;
mod week_qualifiers;
mod writer;
pub(crate) use crate::inference_worker::open_session;
use crate::Engine;
use anyhow::{bail, Context, Result};
pub(crate) use lexical::Cache as RetrievalCache;
pub use retrieval::{
    document_notes, extract_focus, fingerprint, focus_terms, needs_update, ranked_text,
};
use serde_json::{json, Value};
use std::{
    io::BufReader,
    sync::{atomic::Ordering, Arc},
    time::Instant,
};
use uuid::Uuid;
pub use validation::validate;
pub(crate) use validation::{compact_source_model_separators, validate_translation_with_terms};
pub(crate) use week_qualifiers::{
    repair as repair_week_qualifiers, validate as validate_week_qualifiers,
    Mismatch as WeekQualifierMismatch,
};
pub const MODEL_FILE: &str = "Qwen3-1.7B-Q4_K_M.gguf";

struct Generation<'a> {
    trace: &'a crate::inference_trace::TaskTrace,
    scope: &'a generation::Scope,
    requests: &'a generation::Budget,
    context_revision: u64,
}
impl Generation<'_> {
    fn ready(
        &self,
        engine: &Engine,
        selected: &[crate::Segment],
        resource_stop: bool,
        budget_stop: bool,
        timed_out: bool,
        budget: u64,
    ) -> Result<()> {
        if engine.note_cancel.load(Ordering::SeqCst) || engine.maintenance.load(Ordering::SeqCst) {
            bail!("已优先处理录音与字幕，纪要稍后自动继续");
        }
        if resource_stop {
            if budget_stop {
                bail!("writer:resource-budget-hit:{budget}");
            }
            bail!("writer:resource-pressure");
        }
        if timed_out {
            bail!("本轮整理超过计算预算，原文已保留，稍后自动重试");
        }
        self.requests.ensure_remaining()?;
        self.scope.check(engine, selected)
    }
}

fn message_tokens(
    agent: &ureq::Agent,
    base: &str,
    auth: &str,
    messages: &[Value],
) -> Result<usize> {
    let input = messages
        .iter()
        .filter_map(|message| message["content"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let tokens: Value = agent
        .post(&format!("{base}/tokenize"))
        .set("Authorization", auth)
        .send_json(json!({"content":input}))?
        .into_json()?;
    Ok(tokens["tokens"]
        .as_array()
        .context("Invalid tokenizer response")?
        .len())
}

fn source_repair(
    error: &anyhow::Error,
    source: &str,
    terms: &crate::terminology::Terms,
    chinese: bool,
) -> Option<crate::translation::SourceRepair> {
    let factual = error.is::<RelationMismatch>()
        || error.is::<RecordingRetentionMismatch>()
        || error.is::<QualifierMismatch>()
        || error.is::<crate::task_roles::Mismatch>()
        || error.is::<MetricMismatch>()
        || error.is::<SubmissionMismatch>()
        || error.to_string().contains("名称");
    // A copied English draft can reach the language gate before its retention
    // relationship is translated. Use the original identified frame in the
    // same second attempt instead of freely translating that draft.
    let identified_retention =
        error.to_string().contains("输出语言") && retention_anchor(source, chinese).is_some();
    if !factual && !identified_retention {
        return None;
    }
    crate::translation::SourceRepair::from_source(source, terms, chinese)
}

pub fn generate(engine: &Arc<Engine>, meeting: &str) -> Result<usize> {
    let started = Instant::now();
    let job = Uuid::new_v4().to_string();
    let mut trace = crate::inference_trace::TaskTrace::start(&engine.root, "minutes", &job);
    let context_revision = engine.context_revision.load(Ordering::SeqCst);
    let snapshot = engine.snapshot(meeting)?;
    if !needs_update(&snapshot) {
        bail!("纪要已与当前关键词和转写同步");
    }
    if engine.note_cancel.load(Ordering::SeqCst) {
        bail!("已取消，待处理内容已保留");
    }
    if snapshot.runtime.transcribing
        && snapshot
            .runtime
            .elapsed_ms
            .saturating_sub(snapshot.runtime.processed_ms)
            > 2500
    {
        bail!("字幕正在追赶，稍后自动整理");
    }
    if !engine.diagnostics().llm_ready {
        bail!("笔记模型尚未安装，请运行 scripts/bin/setup");
    }
    let prefs = engine.preferences.lock().unwrap().clone();
    let scope = generation::Scope::new(engine, &snapshot, &prefs, context_revision)?;
    let requests = generation::Budget::default();
    let work = Generation {
        trace: &trace,
        scope: &scope,
        requests: &requests,
        context_revision,
    };
    let state_environment = if std::env::var("OMASCRIBE_INCREMENTAL_MINUTES").as_deref() == Ok("1")
    {
        Some(incremental::scope(engine, &snapshot, &prefs)?)
    } else {
        None
    };
    let delta = if let Some(environment) = &state_environment {
        let saved = engine
            .store
            .lock()
            .unwrap()
            .cache_dependency(meeting, incremental::KEY)?;
        incremental::prepare(&snapshot, environment, &saved)
    } else {
        None
    };
    let context = delta
        .as_ref()
        .map(|d| d.context.clone())
        .unwrap_or_else(|| retrieval::retrieve(&snapshot, Some(&engine.retrieval_cache)));
    trace.event(
        "retrieval",
        json!({"selected":context.segments.len(),"cursor":context.cursor}),
    );
    engine.store.lock().unwrap().start_job(&job,meeting,&json!({"pipeline":"document-v3","model":prefs.llm_model,"retrieval_keywords":context.keywords,"cursor_before":snapshot.meeting.notes_cursor,"selected":context.segments.iter().map(|s|json!({"id":s.id,"revision":s.revision})).collect::<Vec<_>>()}))?;
    let result = crate::acceleration::with_cpu_fallback(engine, || {
        requests.ensure_remaining()?;
        generate_inner(
            engine,
            meeting,
            &snapshot,
            context.clone(),
            &prefs,
            &work,
            (delta.clone(), state_environment.as_deref()),
        )
    });
    let outcome = match &result {
        Ok(n) => json!({"status":"committed","sections":n}),
        Err(e) => json!({"status":"deferred","reason":format!("{e:#}")}),
    };
    engine.store.lock().unwrap().finish_job(
        &job,
        &outcome,
        started.elapsed().as_millis() as u64,
    )?;
    trace.finish(result.is_ok(), result.as_ref().ok().copied());
    result
}
fn generate_inner(
    engine: &Arc<Engine>,
    meeting: &str,
    snapshot: &crate::Snapshot,
    mut context: retrieval::Context,
    prefs: &crate::preferences::Preferences,
    work: &Generation<'_>,
    article: (Option<incremental::Delta>, Option<&str>),
) -> Result<usize> {
    let trace = work.trace;
    let (mut delta, state_environment) = article;
    trace.event(
        "article_state",
        json!({"mode":if delta.is_some(){"delta"}else{"full"},"enabled":state_environment.is_some(),
        "selected":context.segments.len(),"fresh":delta.as_ref().map(|d|d.fresh().count())}),
    );
    if delta
        .as_ref()
        .is_some_and(|d| d.fresh().all(|s| writer::non_substantive(&s.text)))
    {
        std::fs::write(
            engine.root.join("last-generation.json"),
            serde_json::to_vec_pretty(&json!({
                "meeting_id":meeting,"model":prefs.llm_model,"attempt":0,"reason":"retained_verified_article",
                "cursor_before":snapshot.meeting.notes_cursor,"cursor_after":context.cursor,"usage":null
            }))?,
        )?;
        trace.event(
            "article_state",
            json!({"mode":"retain_verified_article","reason":"new_mic_checks_only"}),
        );
        return engine.store.lock().unwrap().commit_document_with_state(
            snapshot,
            &context.segments,
            context.cursor,
            vec![],
            engine.note_cancel.load(Ordering::SeqCst),
            state_environment,
        );
    }
    if context
        .segments
        .iter()
        .all(|s| writer::non_substantive(&s.text))
    {
        // Known mic checks do not need a resident LLM, and are not evidence of completed work.
        std::fs::write(
            engine.root.join("last-generation.json"),
            serde_json::to_vec_pretty(&json!({
                "meeting_id":meeting, "model":prefs.llm_model, "attempt":0,
                "reason":"non_substantive_only", "draft":"{\"sections\":[]}", "usage":null,
            }))?,
        )?;
        return engine.store.lock().unwrap().commit_document_with_state(
            snapshot,
            &context.segments,
            context.cursor,
            vec![],
            engine.note_cancel.load(Ordering::SeqCst),
            state_environment,
        );
    }
    if prefs.resource_mode == "quality" && (snapshot.runtime.recording || snapshot.runtime.paused) {
        bail!("质量模式将在录音结束后整理纪要");
    }
    engine.status.lock().unwrap().notes_status = "writer:loading".into();
    engine.status.lock().unwrap().notes_elapsed_ms = 0;
    let session = open_session(engine, prefs, meeting, snapshot.meeting.run)?;
    trace.event(
        "worker",
        json!({"reused":session.reused,"startup_ms":session.startup_ms,
        "backend":session.backend.name(),"budget_mib":session.budget}),
    );
    let crate::inference_worker::LocalSession {
        agent,
        base,
        auth,
        budget,
        backend,
        resource_stop,
        budget_stop,
        timed_out,
        ..
    } = &*session;
    let mut background = engine.writer_context(
        &snapshot.notes,
        &context
            .segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    )?;
    let mut trimmed_background = 0;
    let mut trimmed_passages = 0;
    let (system, content, mut prompt_tokens) = loop {
        let (system, content) = delta.as_ref().map_or_else(
            || writer::prompt(snapshot, &context, &background, &prefs.minutes_language),
            |d| d.prompt(snapshot, &background, &prefs.minutes_language, false),
        );
        let tokens: Value = agent
            .post(&format!("{base}/tokenize"))
            .set("Authorization", auth)
            .send_json(json!({"content":format!("{system}\n{content}")}))?
            .into_json()?;
        let count = tokens["tokens"]
            .as_array()
            .context("Invalid tokenizer response")?
            .len();
        if count <= prefs.context_size as usize - 1148 {
            break (system, content, count);
        }
        if delta.take().is_some() {
            trace.event(
                "article_state",
                json!({"mode":"full_fallback","reason":"input_budget"}),
            );
            context = retrieval::retrieve(snapshot, Some(&engine.retrieval_cache));
            continue;
        }
        // Optional background yields to transcript evidence when context is tight.
        let mut trimmed = false;
        for key in ["background_notes", "writing_preferences", "glossary"] {
            if let Some(items) = background
                .get_mut(key)
                .and_then(|v| v.as_array_mut())
                .filter(|v| !v.is_empty())
            {
                items.pop();
                trimmed = true;
                break;
            }
        }
        if trimmed {
            trimmed_background += 1;
            continue;
        }
        if context.segments.len() <= 1 {
            bail!("转写片段过长，无法装入本地模型上下文");
        }
        // Drop a historical hit first. If a pending segment is removed, it stays pending.
        let index = context
            .segments
            .iter()
            .rposition(|s| s.seq <= snapshot.meeting.notes_cursor)
            .unwrap_or(context.segments.len() - 1);
        let removed = context.segments.remove(index);
        trimmed_passages += 1;
        if removed.seq > snapshot.meeting.notes_cursor {
            context.cursor = context
                .segments
                .iter()
                .filter(|s| s.seq > snapshot.meeting.notes_cursor)
                .map(|s| s.seq)
                .max()
                .unwrap_or(snapshot.meeting.notes_cursor);
        }
    };
    let compose_messages = vec![
        json!({"role":"system","content":system}),
        json!({"role":"user","content":content}),
    ];
    let mut messages = compose_messages.clone();
    // Keep the source envelope separate from its model-facing presentation.
    // Echo validation must always compare IDs against the bound envelope.
    let mut composition_input = content.clone();
    let mut verified = None;
    let mut attempts = Vec::<Value>::new();
    let mut translation_repair: Option<language::Repair> = None;
    let mut relation_repair: Option<crate::translation::SourceRepair> = None;
    let mut prepared_sources: Option<Value> = None;
    // Keep original selected evidence separate from translated prompt passages
    // and generated prose. Names, quotations and cleanup must be source-backed.
    let selected_source = context
        .segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let terms = engine.terminology(meeting)?.with_actor_spellings(
        snapshot
            .segments
            .iter()
            .filter(|s| s.meeting_id == snapshot.meeting.id && s.run == snapshot.meeting.run)
            .map(|s| s.text.as_str()),
        snapshot.meeting.speaker_names.values().map(String::as_str),
    );
    let source_name_mask =
        crate::translation::source_name_mask(&selected_source, &selected_source, &terms);
    // A full manual recomposition has no caption cache to supply the selected
    // output language. Prepare only its missing representations, keeping each
    // translation bound to its original source. The existing two-call budget
    // includes this stage; it does not add another repair attempt.
    let mut source_plan = if delta.is_none() {
        source_language::Plan::new(
            &content,
            &context.segments,
            writer::chinese(snapshot, &prefs.minutes_language),
            &terms,
        )?
    } else {
        None
    };
    if let Some(plan) = &source_plan {
        messages = plan.messages.clone();
        prompt_tokens = message_tokens(agent, base, auth, &messages)?;
        if prompt_tokens > prefs.context_size as usize - 1148 {
            bail!("来源语言准备超出上下文预算，待处理内容已保留");
        }
    }
    trace.event(
        "article_prompt",
        json!({"mode":if source_plan.is_some(){"prepare_sources"}else if delta.is_some(){"delta"}else{"full"},
        "input_tokens":prompt_tokens,"source_preparation":source_plan.as_ref().map(|plan|plan.metrics()),
        "reused_translation_passages":language::reused_count(&content, &context.segments, snapshot)}),
    );
    // Preparation, composition, repair and CPU fallback share two chat sends.
    for attempt in 0..2 {
        engine.status.lock().unwrap().notes_status = if attempt == 0 {
            "writer:reading"
        } else {
            "writer:checking"
        }
        .into();
        let generation_started = Instant::now();
        let mut first_token_ms = None;
        let mut partial_draft = String::new();
        work.ready(
            engine,
            &context.segments,
            resource_stop.load(Ordering::SeqCst),
            budget_stop.load(Ordering::SeqCst),
            timed_out.load(Ordering::SeqCst),
            *budget,
        )?;
        session.check_model()?;
        let request_attempt = work.requests.take()?;
        session.begin_request()?;
        let request_stage = if source_plan.is_some() {
            "prepare_sources"
        } else if prepared_sources.is_some() {
            "compose_prepared_sources"
        } else if attempt == 0 {
            "compose"
        } else {
            "repair"
        };
        let mut request = json!({
            "messages":messages,"temperature":0.1,"seed":42,"chat_template_kwargs":{"enable_thinking":false},
            "max_tokens":440,"stream":true,"stream_options":{"include_usage":true},"cache_prompt":true
        });
        if let Some(plan) = &source_plan {
            request["temperature"] = json!(0.0);
            request["response_format"] = json!({"type":"json_schema","json_schema":{
                "name":"source_languages","strict":true,"schema":plan.schema()}});
        } else if let Some(plan) = &relation_repair {
            request["temperature"] = json!(0.0);
            request["grammar"] = plan.grammar().into();
        } else if let Some(plan) = &translation_repair {
            request["temperature"] = json!(0.0);
            request["response_format"] = json!({"type":"json_schema","json_schema":{
                "name":"translation","strict":true,"schema":plan.schema()}});
        }
        trace.event(
            "request_started",
            json!({"attempt":request_attempt,"endpoint":"chat","stage":request_stage}),
        );
        let response = agent
            .post(&format!("{base}/v1/chat/completions"))
            .set("Authorization", auth)
            .send_json(request);
        let generated = response.map_err(anyhow::Error::from).and_then(|response| {
            stream::read_observed(BufReader::new(response.into_reader()), |draft| {
                partial_draft = draft.to_owned();
                let characters = draft.chars().count();
                if characters > 0 {
                    first_token_ms.get_or_insert(generation_started.elapsed().as_millis() as u64);
                }
                engine.status.lock().unwrap().notes_status = format!("writer:writing:{characters}");
            })
        });
        trace.generation(
            request_attempt,
            generation_started.elapsed().as_millis() as u64,
            first_token_ms,
            &generated
                .as_ref()
                .map(|(_, usage)| usage.clone())
                .unwrap_or(Value::Null),
            generated.is_ok(),
        );
        if generated.is_ok()
            || generated
                .as_ref()
                .is_err_and(|e| e.is::<stream::OutputLimit>())
        {
            session.complete_request()?;
        }
        if generated.is_err() {
            attempts.push(json!({"attempt":request_attempt,"stage":request_stage,"draft":partial_draft,"failure":"incomplete_local_response",
                "elapsed_ms":generation_started.elapsed().as_millis() as u64,"first_token_ms":first_token_ms}));
            std::fs::write(
                engine.root.join("last-generation.json"),
                serde_json::to_vec_pretty(&json!({
                    "meeting_id":meeting,"model":prefs.llm_model,"backend":backend.name(),
                    "attempt":request_attempt,"attempts":attempts,"draft":partial_draft,"committed":false,
                }))?,
            )?;
        }
        if engine.note_cancel.load(Ordering::SeqCst) {
            bail!("已优先处理录音与字幕，纪要稍后自动继续");
        }
        if resource_stop.load(Ordering::SeqCst) {
            if budget_stop.load(Ordering::SeqCst) {
                bail!("writer:resource-budget-hit:{budget}");
            }
            bail!("writer:resource-pressure");
        }
        if timed_out.load(Ordering::SeqCst) {
            bail!("本轮整理超过计算预算，原文已保留，稍后自动重试");
        }
        let (raw_body, usage) = generated.map_err(|error| {
            // Transport details (ephemeral URLs and nested socket errors) are not useful in the UI.
            if error
                .chain()
                .any(|e| e.is::<ureq::Error>() || e.is::<std::io::Error>())
            {
                anyhow::anyhow!("本地模型响应中断或等待过久；原文已保留，稍后自动重试")
            } else {
                error
            }
        })?;
        if let Some(plan) = source_plan.take() {
            let preparation = plan.prepare(&raw_body, &context.segments, &terms);
            let metrics = plan.metrics();
            attempts.push(json!({
                "attempt":request_attempt,"stage":request_stage,"draft":raw_body,"body_draft":raw_body,
                "mode":"prepare_sources","source_preparation":metrics,"usage":usage,
                "input_tokens":prompt_tokens,"context_size":prefs.context_size,
                "validation_error":preparation.as_ref().err().map(|error|format!("{error:#}")),
                "elapsed_ms":generation_started.elapsed().as_millis() as u64,"first_token_ms":first_token_ms,
                "selected":context.segments.iter().map(|s|json!({"id":s.id,"seq":s.seq,"revision":s.revision})).collect::<Vec<_>>(),
            }));
            std::fs::write(
                engine.root.join("last-generation.json"),
                serde_json::to_vec_pretty(&json!({
                    "meeting_id":meeting,"model":prefs.llm_model,"backend":backend.name(),
                    "attempt":request_attempt,"attempts":attempts,"draft":raw_body,
                    "cursor_before":snapshot.meeting.notes_cursor,"cursor_after":snapshot.meeting.notes_cursor,
                    "stage":"source_preparation","committed":false,
                }))?,
            )?;
            let prepared = preparation?;
            // Do not spend the second request on stale representations or
            // silently trim away a translated source that no longer fits.
            work.ready(
                engine,
                &context.segments,
                resource_stop.load(Ordering::SeqCst),
                budget_stop.load(Ordering::SeqCst),
                timed_out.load(Ordering::SeqCst),
                *budget,
            )?;
            session.check_model()?;
            let prepared_content = prepared.apply(&content, &context.segments)?;
            messages = compose_messages.clone();
            let presentation = writer::composition_content(&prepared_content)?;
            let instruction = if writer::chinese(snapshot, &prefs.minutes_language) {
                " 同一事实只写一次；名称、金额、否定或状态不同的内容仍须保留，不能擅自合并。归纳成连贯段落，不要逐条翻译或复述输入。"
            } else {
                " State each repeated fact once. Preserve differences in names, amounts, negation and status instead of merging them. Synthesize connected paragraphs rather than translating or repeating each input passage."
            };
            messages[0]["content"] = format!(
                "{}{instruction}",
                messages[0]["content"]
                    .as_str()
                    .context("Missing composition instruction")?
            )
            .into();
            messages[1]["content"] = presentation.into();
            composition_input = prepared_content;
            prompt_tokens = message_tokens(agent, base, auth, &messages)?;
            if prompt_tokens > prefs.context_size as usize - 1148 {
                bail!("译后来源超出纪要上下文预算，待处理内容已保留");
            }
            trace.event(
                "article_prompt",
                json!({"mode":"compose_prepared_sources","input_tokens":prompt_tokens,
                    "source_preparation":metrics,
                    "reused_translation_passages":language::reused_count(&content,&context.segments,snapshot)}),
            );
            prepared_sources = Some(metrics);
            continue;
        }
        let prose = if let Some(plan) = &relation_repair {
            plan.body(&raw_body)
        } else if let Some(plan) = &translation_repair {
            plan.body(&raw_body)
        } else {
            writer::body_text(&raw_body, &composition_input)
        };
        let mut checked_prose = None;
        let mut unitless_yuan_corrections = 0;
        let mut week_qualifier_corrections = 0;
        let normalized = prose
            .as_ref()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .and_then(|body| {
                let repaired = amounts::repair_unitless_yuan(body, &selected_source);
                unitless_yuan_corrections = body
                    .chars()
                    .count()
                    .saturating_sub(repaired.chars().count());
                let repaired = week_qualifiers::repair(
                    &repaired,
                    &selected_source,
                    &crate::translation::source_name_mask(&repaired, &selected_source, &terms),
                    &source_name_mask,
                )
                .map_or(repaired, |repair| {
                    week_qualifier_corrections = repair.removed_qualifiers;
                    repair.text
                });
                let body = uncertainty::restore(&terms.normalize(&repaired), &context.segments);
                terms.validate_writer_names(
                    &context
                        .segments
                        .iter()
                        .filter(|s| !writer::non_substantive(&s.text))
                        .map(|s| s.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                    &body,
                )?;
                let document =
                    writer::document(&body, &context.segments, &snapshot.meeting.speaker_names)?;
                checked_prose = Some(body);
                Ok(document)
            })
            .and_then(|text| terms.normalize_draft(&text));
        let text = normalized.as_ref().map(String::as_str).unwrap_or(&raw_body);
        let validation = normalized
            .as_ref()
            .map_err(|e| {
                // Keep the semantic category from the spelling/normalization
                // stage so the same bounded retry uses ORIGINAL source roles.
                if e.is::<crate::task_roles::Mismatch>() {
                    crate::task_roles::Mismatch.into()
                } else {
                    anyhow::anyhow!("{e}")
                }
            })
            .and_then(|draft| {
                let sections = validation::validate_with_terms(
                    draft,
                    &context.segments,
                    meeting,
                    &snapshot.meeting.speaker_names,
                    &terms,
                )?;
                validation::preserve_replaced_sources(
                    &snapshot.suggestions,
                    &context.segments,
                    &sections,
                )?;
                // A language-only repair may translate this checked prose; it
                // must never be used to launder an unsupported first draft.
                let body = sections
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                language::validate_with_source(
                    &body,
                    &crate::translation::source_name_mask(&body, &selected_source, &terms),
                    &selected_source,
                    writer::chinese(snapshot, &prefs.minutes_language),
                )?;
                Ok(sections)
            });
        attempts.push(json!({
            "attempt":request_attempt, "stage":request_stage, "draft":text, "body_draft":raw_body, "prose_body":prose.as_ref().ok(), "normalized_draft":normalized.as_ref().ok(), "usage":usage,
            "week_qualifier_corrections":week_qualifier_corrections,
            "mode":if relation_repair.is_some() {"translate_source_relations"} else if translation_repair.is_some() {"translate_checked_prose"} else if prepared_sources.is_some() {"compose_prepared_sources"} else {"compose"},
            "source_preparation":prepared_sources,
            "translation_plan":translation_repair.as_ref().map(|p|p.metrics()),
            "unitless_yuan_repaired":unitless_yuan_corrections > 0,
            "unitless_yuan_corrections":unitless_yuan_corrections,
            "validation_error":validation.as_ref().err().map(|e|format!("{e:#}")),
            "elapsed_ms":generation_started.elapsed().as_millis() as u64, "first_token_ms":first_token_ms,
            "input_tokens":prompt_tokens, "citation_scope":"selected_transcript_context", "context_size":prefs.context_size,
            "selected":context.segments.iter().map(|s|json!({"id":s.id,"seq":s.seq,"revision":s.revision})).collect::<Vec<_>>(),
        }));
        std::fs::write(
            engine.root.join("last-generation.json"),
            serde_json::to_vec_pretty(&json!({
                "meeting_id":meeting,"model":prefs.llm_model,"backend":backend.name(),
                "resource_mode":prefs.resource_mode,"memory_budget_mib":budget,"draft":text,
                "usage":usage,"attempt":request_attempt,"attempts":attempts,
                "cursor_before":snapshot.meeting.notes_cursor,"cursor_after":context.cursor,
                "trimmed_background":trimmed_background,"trimmed_passages":trimmed_passages,
            }))?,
        )?;
        match validation {
            Ok(sections) => {
                verified = Some(sections);
                break;
            }
            Err(error) if attempt == 0 => {
                // Semantic failures protect ORIGINAL source relationships.
                // A language-only failure below edits the checked spans instead.
                let source_plan = {
                    let (_, input) = writer::prompt_mode(
                        snapshot,
                        &context,
                        &json!({}),
                        &prefs.minutes_language,
                        false,
                    );
                    let original: Value = serde_json::from_str(&input)?;
                    let source = original["transcript"]
                        .as_array()
                        .context("Missing source transcript")?
                        .iter()
                        .filter_map(|p| p["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    source_repair(
                        &error,
                        &source,
                        &terms,
                        writer::chinese(snapshot, &prefs.minutes_language),
                    )
                };
                if let Some(plan) = source_plan {
                    trace.event("article_repair", json!({"mode":"source_relations","source_predicates":plan.source_predicates(),"source_slots":plan.source_slots()}));
                    messages = plan.messages.clone();
                    relation_repair = Some(plan);
                } else if error.to_string().contains("输出语言") {
                    let prose = checked_prose.as_deref().context("Missing checked prose")?;
                    let plan = language::repair_messages_with_source(
                        normalized.as_ref().map_err(|e| anyhow::anyhow!("{e}"))?,
                        prose,
                        &crate::translation::source_name_mask(prose, &selected_source, &terms),
                        &selected_source,
                        writer::chinese(snapshot, &prefs.minutes_language),
                    )?;
                    messages = plan.messages.clone();
                    translation_repair = Some(plan);
                } else {
                    // Rebuild from original evidence for factual failures: small
                    // models otherwise repeat invalid attributions.
                    // Keep historical evidence too: a repair must not forget earlier decisions.
                    let spelling_context = background
                        .get("spellings")
                        .map(|v| json!({"spellings":v}))
                        .unwrap_or_else(|| json!({}));
                    let narrow = error.to_string().contains("数字或日期");
                    let (repair_system, repair_content) = delta.as_ref().map_or_else(
                        || {
                            writer::prompt_mode(
                                snapshot,
                                &context,
                                &spelling_context,
                                &prefs.minutes_language,
                                narrow,
                            )
                        },
                        |d| d.prompt(snapshot, &spelling_context, &prefs.minutes_language, narrow),
                    );
                    let mut repair: Value = serde_json::from_str(&repair_content)?;
                    let attribution = error.to_string().contains("speaker attribution");
                    if attribution {
                        for passage in repair["transcript"].as_array_mut().unwrap() {
                            passage["speaker"] = Value::Null;
                        }
                    }
                    let history_hint = format!("{error}。保留这些历史来源中尚未被更正的事实和未完成事项，不要忽略旧议题。Preserve facts and unresolved items from these missing historical passages.");
                    let name_hint = error.to_string();
                    let unsupported_hint = format!("{error}。只保留原话支持的事实与限定。State only supported facts and qualifiers; omit the unsupported claim named above, in the selected output language.");
                    let hint = if let Some(retention) =
                        error.downcast_ref::<RecordingRetentionMismatch>()
                    {
                        retention.hint()
                    } else if let Some(week) = error.downcast_ref::<WeekQualifierMismatch>() {
                        week.hint()
                    } else if let Some(metric) = error.downcast_ref::<MetricMismatch>() {
                        metric.hint()
                    } else if let Some(submission) = error.downcast_ref::<SubmissionMismatch>() {
                        submission.hint()
                    } else if let Some(role) = error.downcast_ref::<crate::task_roles::Mismatch>() {
                        role.hint()
                    } else if let Some(qualifier) = error.downcast_ref::<QualifierMismatch>() {
                        qualifier.hint()
                    } else if let Some(relation) = error.downcast_ref::<RelationMismatch>() {
                        relation.hint()
                    } else if name_hint.contains("专有名称") {
                        &name_hint
                    } else if error.to_string().contains("历史来源") {
                        &history_hint
                    } else if error.to_string().contains("转写仍待整理") {
                        "提问和问题反馈也是实质内容。仅整理可理解的原话，保留疑问而不将它写成确认结论；不能因部分片段含混而忽略全部内容。Questions and reported problems are substantive. Keep them as questions, not confirmed conclusions; unclear passages do not invalidate the rest."
                    } else if error.to_string().contains("预算最新更正") {
                        "采用current_budget中已由原文明确更正的最新金额，不输出被替换的旧金额；保留其他议题、负责人、期限和否定状态。Use the current_budget value and omit superseded amounts, retaining unrelated topics, owners, deadlines and negative status."
                    } else if error.to_string().contains("最新完成状态") {
                        "同一任务后文明示完成时写已完成；其他任务仍未完成的状态保留。不要把已被更新的旧计划写成当前计划。Use the later explicit completion for that same task, retain unrelated pending work, and omit superseded plans."
                    } else if error.to_string().contains("无依据补写") {
                        &unsupported_hint
                    } else if error.to_string().contains("问题反馈反向") {
                        "原话抱怨的现状只记录为问题，不要将其改写成需要继续这样做，不推测解决方案。Preserve the complaint as a problem report; do not turn the criticized behavior into a new requirement."
                    } else if error.to_string().contains("技术限定") {
                        &name_hint
                    } else if error.to_string().contains("因果关系") {
                        "原话没有明确说明因果关系。不要用因为、导致等词补充解释，分别记录已说出的事实和疑问。The evidence does not explicitly state causation. Record the stated facts and questions separately without adding explanations such as because or caused by."
                    } else if error.to_string().contains("未完成") {
                        "保留来源中仍未完成的具体事项。另一项任务已完成，不能作为这项也已完成的依据。Keep the specific unresolved work from its own source; another task's completion does not resolve it."
                    } else if error.to_string().contains("相对日期") {
                        "今天、明天、本周、下周这类相对日期，只能原样来自引用的片段，做不到就删去时间词，只保留原话支持的内容。"
                    } else if attribution {
                        "所有片段采用中性叙述，不写人名，不推测是谁说的。"
                    } else if error.to_string().contains("attendee") {
                        "不要生成参会名单，只写转写明确支持的讨论内容。"
                    } else {
                        "数字和时间保留原文写法，保留否定、问题和待定状态；省略没有原文支持的内容。"
                    };
                    messages = vec![
                        json!({"role":"system","content":format!("{repair_system} {hint}")}),
                        json!({"role":"user","content":serde_json::to_string(&repair)?}),
                    ];
                    composition_input = serde_json::to_string(&repair)?;
                }
                let repair_input = messages
                    .iter()
                    .filter_map(|m| m["content"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let tokens: Value = agent
                    .post(&format!("{base}/tokenize"))
                    .set("Authorization", auth)
                    .send_json(json!({"content":repair_input}))?
                    .into_json()?;
                prompt_tokens = tokens["tokens"]
                    .as_array()
                    .context("Invalid tokenizer response")?
                    .len();
                if prompt_tokens > prefs.context_size as usize - 598 {
                    return Err(error.context("复核超出上下文预算，待处理内容已保留"));
                }
            }
            Err(error) => return Err(error),
        }
    }
    let sections = verified.context("纪要复核未完成，待处理内容已保留")?;
    work.scope.check(engine, &context.segments)?;
    session.check_model()?;
    let mut db = engine.store.lock().unwrap();
    work.scope.check_store(&db, &context.segments)?;
    if work.context_revision != engine.context_revision.load(Ordering::SeqCst) {
        bail!("辅助背景已更新，将使用新配置重新整理");
    }
    db.commit_document_with_state(
        snapshot,
        &context.segments,
        context.cursor,
        sections,
        engine.note_cancel.load(Ordering::SeqCst),
        state_environment,
    )
}

#[cfg(test)]
mod source_repair_tests {
    use super::*;

    #[test]
    fn copied_retention_draft_uses_original_fields_in_the_existing_language_repair() {
        let terms = crate::terminology::Terms::new(&[], &[]);
        let error = anyhow::anyhow!("纪要输出语言与选择不符，原话仍待整理");
        let source =
            "Friday is not the deadline. Recording in meeting notes should stay on this computer.";
        let plan = source_repair(&error, source, &terms, true).unwrap();
        assert_eq!(plan.source_slots(), 2);
        assert!(plan
            .grammar()
            .contains("会议笔记中的录音应保留在这台电脑上。"));
        assert_eq!(
            plan.body(r#"[{"translation":"星期五不是截止日期。"},{"translation":"会议笔记中的录音应保留在这台电脑上。"}]"#).unwrap(),
            "星期五不是截止日期。 会议笔记中的录音应保留在这台电脑上。"
        );
        assert!(plan.body(r#"[{"translation":"星期五不是截止日期。"},{"translation":"会议笔记的录音应保持在本计算机上。"}]"#).is_err());
        for other_source in [
            "Meeting notes should stay on this computer.",
            "If approved, recording in meeting notes should stay on this computer.",
            "Recording in meeting notes should move to an external drive.",
        ] {
            assert!(source_repair(&error, other_source, &terms, true).is_none());
        }
    }

    #[test]
    fn second_stage_is_not_dispatched_after_input_or_resource_changes() -> Result<()> {
        for mutation in [
            "cancel",
            "maintenance",
            "source",
            "note",
            "terms",
            "speaker",
            "preferences",
            "background",
            "pressure",
            "budget",
            "timeout",
        ] {
            let root = tempfile::tempdir()?;
            let engine = Engine::open(root.path().into())?;
            let meeting = engine.create_meeting("Fictional two-stage preparation")?;
            let source = crate::Segment {
                id: format!("{}:1:1", meeting.id),
                meeting_id: meeting.id.clone(),
                run: meeting.run,
                seq: 1,
                start_ms: 0,
                end_ms: 1000,
                text: "缓存仍应保存在本机。".into(),
                revision: 1,
                speaker: None,
                speaker_manual: false,
            };
            engine.store.lock().unwrap().save_segment(&source)?;
            let snapshot = engine.snapshot(&meeting.id)?;
            let prefs = engine.preferences.lock().unwrap().clone();
            let revision = engine.context_revision.load(Ordering::SeqCst);
            let scope = generation::Scope::new(&engine, &snapshot, &prefs, revision)?;
            let requests = generation::Budget::default();
            let trace = crate::inference_trace::TaskTrace::start(&engine.root, "minutes", "stages");
            let work = Generation {
                trace: &trace,
                scope: &scope,
                requests: &requests,
                context_revision: revision,
            };
            let mut dispatched = 0;
            work.ready(&engine, &snapshot.segments, false, false, false, 4096)?;
            assert_eq!(requests.take()?, 1);
            assert!(engine.store.try_lock().is_ok());
            assert!(engine.preferences.try_lock().is_ok());
            assert!(engine.config_lock.try_lock().is_ok());
            dispatched += 1;
            match mutation {
                "cancel" => engine.note_cancel.store(true, Ordering::SeqCst),
                "maintenance" => engine.maintenance.store(true, Ordering::SeqCst),
                "source" => {
                    let mut changed = source.clone();
                    changed.text = "缓存将上传到服务器。".into();
                    engine.store.lock().unwrap().save_segment(&changed)?;
                }
                "note" => {
                    engine.save_note(&meeting.id, None, "新的整理重点", "note", 0)?;
                }
                "terms" => engine.store.lock().unwrap().set_cache_dependency(
                    &meeting.id,
                    "terms",
                    "changed",
                )?,
                "speaker" => {
                    let mut changed = meeting.clone();
                    changed
                        .speaker_names
                        .insert("speaker-1".into(), "林悦".into());
                    engine.store.lock().unwrap().save_meeting(&changed)?;
                }
                "preferences" => engine.preferences.lock().unwrap().minutes_language = "en".into(),
                "background" => engine.invalidate_background()?,
                "pressure" | "budget" | "timeout" => {}
                _ => unreachable!(),
            }
            if work
                .ready(
                    &engine,
                    &snapshot.segments,
                    matches!(mutation, "pressure" | "budget"),
                    mutation == "budget",
                    mutation == "timeout",
                    4096,
                )
                .is_ok()
            {
                dispatched += 1;
            }
            assert_eq!(dispatched, 1, "{mutation}");
            assert_eq!(engine.snapshot(&meeting.id)?.meeting.notes_cursor, 0);
            assert_eq!(
                requests.take()?,
                2,
                "a rejected dispatch must not consume quota"
            );
            assert!(requests.take().is_err());
        }
        Ok(())
    }
}
