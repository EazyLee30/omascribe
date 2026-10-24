//! Automatic meeting-document pipeline: retrieve → compose → validate → transactional merge.
mod retrieval;
mod stream;
mod validation;
mod writer;
use crate::Engine;
use anyhow::{bail, Context, Result};
pub use retrieval::{document_notes, fingerprint, focus_terms, needs_update, ranked_text};
use serde_json::{json, Value};
use std::{
    fs::File,
    io::BufReader,
    net::TcpListener,
    process::{Child, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;
pub use validation::validate;
pub const MODEL_FILE: &str = "Qwen3-1.7B-Q4_K_M.gguf";

fn check_memory(available: Option<u64>, required: u64) -> Result<()> {
    match available {
        Some(mib) if mib < required => {
            bail!("writer:low-memory:{mib}:{required}")
        }
        None => bail!("writer:memory-unavailable"),
        _ => Ok(()),
    }
}

struct Server(Child, Arc<AtomicBool>);
impl Drop for Server {
    fn drop(&mut self) {
        self.1.store(true, Ordering::SeqCst);
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn generate(engine: &Arc<Engine>, meeting: &str) -> Result<usize> {
    let started = Instant::now();
    let job = Uuid::new_v4().to_string();
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
        bail!("笔记模型尚未安装，请运行 bin/setup");
    }
    let context = retrieval::retrieve(&snapshot);
    let prefs = engine.preferences.lock().unwrap().clone();
    engine.store.lock().unwrap().start_job(&job,meeting,&json!({"pipeline":"document-v1","model":prefs.llm_model,"cursor_before":snapshot.meeting.notes_cursor,"selected":context.segments.iter().map(|s|json!({"id":s.id,"revision":s.revision})).collect::<Vec<_>>()}))?;
    let result = generate_inner(engine, meeting, &snapshot, context, &prefs);
    let outcome = match &result {
        Ok(n) => json!({"status":"committed","sections":n}),
        Err(e) => json!({"status":"deferred","reason":format!("{e:#}")}),
    };
    engine.store.lock().unwrap().finish_job(
        &job,
        &outcome,
        started.elapsed().as_millis() as u64,
    )?;
    result
}
fn generate_inner(
    engine: &Arc<Engine>,
    meeting: &str,
    snapshot: &crate::Snapshot,
    mut context: retrieval::Context,
    prefs: &crate::preferences::Preferences,
) -> Result<usize> {
    let budget = crate::inference::budget_mib(prefs);
    let estimate = crate::models::model(&prefs.llm_model)?.memory_mib
        + u64::from(prefs.context_size.saturating_sub(2048)) / 4;
    if estimate > budget {
        bail!("writer:resource-budget:{estimate}:{budget}");
    }
    if prefs.resource_mode == "quality" && (snapshot.runtime.recording || snapshot.runtime.paused) {
        bail!("质量模式将在录音结束后整理纪要");
    }
    // Loading under heavy swap pressure repeatedly makes the whole desktop unresponsive.
    check_memory(
        crate::platform::available_memory_mib(),
        crate::models::model(&prefs.llm_model)?.memory_mib
            + u64::from(prefs.context_size.saturating_sub(2048)) / 4,
    )?;
    engine.status.lock().unwrap().notes_status = "writer:loading".into();
    engine.status.lock().unwrap().notes_elapsed_ms = 0;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let token = Uuid::new_v4().to_string();
    let (mut command, backend) = crate::inference::server_command(engine, prefs, port, &token)?;
    let log = File::create(engine.root.join("llm-runtime.log"))?;
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()
        .context("无法启动本地笔记模型")?;
    let done = Arc::new(AtomicBool::new(false));
    let mut server = Server(child, done.clone());
    let pid = server.0.id();
    engine.llm_pid.store(pid, Ordering::SeqCst);
    let weak = Arc::downgrade(engine);
    let deadline = Instant::now();
    let timed_out = Arc::new(AtomicBool::new(false));
    let timeout_flag = timed_out.clone();
    let resource_stop = Arc::new(AtomicBool::new(false));
    let resource_flag = resource_stop.clone();
    thread::spawn(move || {
        let mut lag_count = 0;
        let mut memory = sysinfo::System::new();
        let process = sysinfo::Pid::from_u32(pid);
        while !done.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(500));
            if done.load(Ordering::SeqCst) {
                break;
            }
            memory.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[process]), true);
            let over_budget = memory
                .process(process)
                .is_some_and(|p| p.memory() / 1024 / 1024 > budget);
            let pressure = crate::platform::available_memory_mib().is_some_and(|m| m < 512);
            if over_budget || pressure {
                resource_flag.store(true, Ordering::SeqCst);
                crate::platform::stop_process(pid, false);
                break;
            }
            let Some(engine) = weak.upgrade() else { break };
            let s = {
                let mut s = engine.status.lock().unwrap();
                s.notes_elapsed_ms = deadline.elapsed().as_millis() as u64;
                s.clone()
            };
            if deadline.elapsed() > Duration::from_secs(600) {
                timeout_flag.store(true, Ordering::SeqCst);
                crate::platform::stop_process(pid, false);
                break;
            }
            if s.recording && s.elapsed_ms.saturating_sub(s.processed_ms) > 4000 {
                lag_count += 1;
            } else {
                lag_count = 0;
            }
            if engine.note_cancel.load(Ordering::SeqCst) || lag_count >= 6 {
                engine.note_cancel.store(true, Ordering::SeqCst);
                crate::platform::stop_process(pid, false);
                break;
            }
        }
    });
    // This HTTP client only talks to an authenticated loopback process; proxies are disabled.
    let agent = ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .redirects(0)
        .timeout_connect(Duration::from_secs(3))
        .timeout_write(Duration::from_secs(15))
        // This is an idle-read limit, not a limit on the total streamed response.
        .timeout_read(Duration::from_secs(300))
        .build();
    let base = format!("http://127.0.0.1:{port}");
    let auth = format!("Bearer {token}");
    let start = Instant::now();
    loop {
        if resource_stop.load(Ordering::SeqCst) {
            bail!("writer:resource-pressure");
        }
        if server.0.try_wait()?.is_some() {
            bail!("笔记模型已停止；原文保留，可以稍后重试");
        }
        if agent
            .get(&format!("{base}/health"))
            .set("Authorization", &auth)
            .timeout(Duration::from_secs(1))
            .call()
            .is_ok()
        {
            break;
        }
        if start.elapsed() > Duration::from_secs(120) {
            bail!("笔记模型启动超时");
        }
        thread::sleep(Duration::from_millis(200));
    }

    let context_revision = engine.context_revision.load(Ordering::SeqCst);
    let mut background = engine.writer_context(
        &snapshot.notes,
        &context
            .segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    )?;
    let (system, content) = loop {
        let (system, content) = writer::prompt(snapshot, &context, &background);
        let tokens: Value = agent
            .post(&format!("{base}/tokenize"))
            .set("Authorization", &auth)
            .send_json(json!({"content":format!("{system}\n{content}")}))?
            .into_json()?;
        if tokens["tokens"]
            .as_array()
            .context("Invalid tokenizer response")?
            .len()
            <= prefs.context_size as usize - 1148
        {
            break (system, content);
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
    let mut messages = vec![
        json!({"role":"system","content":system}),
        json!({"role":"user","content":content}),
    ];
    let mut verified = None;
    // One evidence-guided repair, inside the same overall deadline. Never relax validation.
    for attempt in 0..2 {
        engine.status.lock().unwrap().notes_status = if attempt == 0 {
            "writer:reading"
        } else {
            "writer:checking"
        }
        .into();
        let response=agent.post(&format!("{base}/v1/chat/completions")).set("Authorization",&auth).send_json(json!({
        "messages":messages,
        "temperature":0.1,"seed":42,"chat_template_kwargs":{"enable_thinking":false},"max_tokens":440,"stream":true,"stream_options":{"include_usage":true},"cache_prompt":attempt > 0,
        "response_format":{"type":"json_schema","json_schema":{"name":"document","strict":true,"schema":writer::schema(context.segments.len())}}
    }));
        let generated = response.map_err(anyhow::Error::from).and_then(|response| {
            stream::read(BufReader::new(response.into_reader()), |characters| {
                engine.status.lock().unwrap().notes_status = format!("writer:writing:{characters}");
            })
        });
        if engine.note_cancel.load(Ordering::SeqCst) {
            bail!("已优先处理录音与字幕，纪要稍后自动继续");
        }
        if resource_stop.load(Ordering::SeqCst) {
            bail!("writer:resource-pressure");
        }
        if timed_out.load(Ordering::SeqCst) {
            bail!("本轮整理超过计算预算，原文已保留，稍后自动重试");
        }
        let (text, usage) = generated.map_err(|error| {
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
        std::fs::write(
            engine.root.join("last-generation.json"),
            serde_json::to_vec_pretty(
                &json!({"meeting_id":meeting,"model":prefs.llm_model,"backend":backend.name(),"resource_mode":prefs.resource_mode,"memory_budget_mib":budget,"draft":text,"usage":usage,"attempt":attempt + 1}),
            )?,
        )?;
        match validation::validate_with_speakers(
            &text,
            &context.segments,
            meeting,
            &snapshot.meeting.speaker_names,
        ) {
            Ok(sections) => {
                verified = Some(sections);
                break;
            }
            Err(error) if attempt == 0 => {
                messages.push(json!({"role":"assistant","content":text}));
                messages.push(json!({"role":"user","content":format!("Validation failed: {error}. Recheck each section against the exact transcript passage IDs. Sources are transcript IDs, NOT section numbers. Correct wrong citations; remove unsupported claims instead of inventing evidence. Preserve uncertainty and original dates. Return corrected JSON only.")}));
                let repair_input = messages
                    .iter()
                    .filter_map(|m| m["content"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let tokens: Value = agent
                    .post(&format!("{base}/tokenize"))
                    .set("Authorization", &auth)
                    .send_json(json!({"content":repair_input}))?
                    .into_json()?;
                if tokens["tokens"]
                    .as_array()
                    .context("Invalid tokenizer response")?
                    .len()
                    > prefs.context_size as usize - 598
                {
                    return Err(error.context("复核超出上下文预算，待处理内容已保留"));
                }
            }
            Err(error) => return Err(error),
        }
    }
    let sections = verified.context("纪要复核未完成，待处理内容已保留")?;
    let mut db = engine.store.lock().unwrap();
    if context_revision != engine.context_revision.load(Ordering::SeqCst) {
        bail!("辅助背景已更新，将使用新配置重新整理");
    }
    db.commit_document(
        snapshot,
        &context.segments,
        context.cursor,
        sections,
        engine.note_cancel.load(Ordering::SeqCst),
    )
}

#[cfg(test)]
mod memory_tests {
    use super::*;

    #[test]
    fn writer_requires_measured_headroom() {
        assert!(check_memory(Some(4236), 1400).is_ok());
        assert!(check_memory(Some(1400), 1400).is_ok());
        assert_eq!(
            check_memory(Some(1399), 1400).unwrap_err().to_string(),
            "writer:low-memory:1399:1400"
        );
        assert!(check_memory(Some(0), 1400).is_err());
        assert_eq!(
            check_memory(None, 1400).unwrap_err().to_string(),
            "writer:memory-unavailable"
        );
    }
}
