//! Bounded, local task metadata. Never accepts transcript, prompt, or generated text.
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Instant,
};

const MAX_BYTES: u64 = 4 * 1024 * 1024;
static WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Default, Serialize)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub prefill_ms: Option<f64>,
    pub decode_ms: Option<f64>,
}
impl Usage {
    pub fn parse(value: &Value) -> Self {
        Self {
            input_tokens: value["prompt_tokens"].as_u64(),
            output_tokens: value["completion_tokens"].as_u64(),
            cached_tokens: value["prompt_tokens_details"]["cached_tokens"].as_u64(),
            prefill_ms: value["timings"]["prompt_ms"].as_f64(),
            decode_ms: value["timings"]["predicted_ms"].as_f64(),
        }
    }
}

pub struct TaskTrace {
    root: PathBuf,
    id: String,
    kind: &'static str,
    started: Instant,
    finished: bool,
}
impl TaskTrace {
    pub fn start(root: &Path, kind: &'static str, job: &str) -> Self {
        let trace = Self {
            root: root.to_owned(),
            id: job.to_owned(),
            kind,
            started: Instant::now(),
            finished: false,
        };
        trace.event("started", json!({}));
        trace
    }
    pub fn event(&self, phase: &'static str, metrics: Value) {
        let row = json!({
            "schema":1,"at":chrono::Utc::now().to_rfc3339(),"job":self.id,
            "task":self.kind,"phase":phase,"elapsed_ms":self.started.elapsed().as_millis() as u64,
            "metrics":metrics,
        });
        // Diagnostics must never cause a recording or generation to fail.
        let _ = append(&self.root, &row);
    }
    pub fn generation(
        &self,
        attempt: usize,
        elapsed_ms: u64,
        first_token_ms: Option<u64>,
        usage: &Value,
        completed: bool,
    ) {
        self.event(
            "generation",
            json!({
                "attempt":attempt,"request_ms":elapsed_ms,"first_token_ms":first_token_ms,
                "usage":Usage::parse(usage),"completed":completed,
            }),
        );
    }
    pub fn finish(&mut self, committed: bool, count: Option<usize>) {
        self.event("finished", json!({"committed":committed,"items":count}));
        self.finished = true;
    }
}
impl Drop for TaskTrace {
    fn drop(&mut self) {
        if !self.finished {
            self.event("abandoned", json!({}));
        }
    }
}
fn append(root: &Path, row: &Value) -> std::io::Result<()> {
    let _lock = WRITE_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let path = root.join("inference-trace.jsonl");
    if path.metadata().is_ok_and(|m| m.len() >= MAX_BYTES) {
        let previous = root.join("inference-trace.previous.jsonl");
        // Windows cannot atomically replace an existing file with rename.
        match fs::remove_file(&previous) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        fs::rename(&path, previous)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    serde_json::to_writer(&mut file, row)?;
    file.write_all(b"\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_usage_is_unknown_and_zero_is_preserved() {
        assert_eq!(Usage::parse(&Value::Null).cached_tokens, None);
        let usage = Usage::parse(&json!({"prompt_tokens":30,"completion_tokens":2,
            "prompt_tokens_details":{"cached_tokens":0},"timings":{"prompt_ms":1.25}}));
        assert_eq!(usage.cached_tokens, Some(0));
        assert_eq!(usage.prefill_ms, Some(1.25));
        assert_eq!(usage.decode_ms, None);
    }
    #[test]
    fn trace_rotates_and_records_terminal_outcome() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        fs::write(
            root.path().join("inference-trace.jsonl"),
            vec![b' '; MAX_BYTES as usize],
        )?;
        let mut trace = TaskTrace::start(root.path(), "translation", "job-id");
        trace.generation(1, 100, Some(10), &json!({"prompt_tokens":3}), true);
        trace.finish(true, Some(1));
        let data = fs::read_to_string(root.path().join("inference-trace.jsonl"))?;
        let rows: Vec<Value> = data
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?;
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2]["phase"], "finished");
        assert_eq!(rows[1]["metrics"]["usage"]["cached_tokens"], Value::Null);
        assert!(root.path().join("inference-trace.previous.jsonl").exists());
        Ok(())
    }
}
