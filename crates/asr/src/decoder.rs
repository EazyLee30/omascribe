//! Model ownership and utterance-boundary hotwords. Stream updates remain opt-in.
use crate::hotwords;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use sherpa_onnx::{OnlineRecognizer, OnlineRecognizerConfig, OnlineStream};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Instant,
};

const MAX_WORD_BYTES: u64 = 16_000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Legacy,
    Stream,
}
impl Mode {
    pub fn configured() -> Self {
        Self::parse(
            std::env::var("OMASCRIBE_ASR_STREAM_HOTWORDS")
                .ok()
                .as_deref(),
        )
    }
    fn parse(value: Option<&str>) -> Self {
        if value == Some("1") {
            Self::Stream
        } else {
            Self::Legacy
        }
    }
    fn name(self) -> &'static str {
        if self == Self::Stream {
            "stream_experiment"
        } else {
            "recognizer"
        }
    }
}

pub fn read_words(path: &Path) -> Result<String> {
    let mut words = String::new();
    fs::File::open(path)?
        .take(MAX_WORD_BYTES + 1)
        .read_to_string(&mut words)?;
    anyhow::ensure!(
        words.len() as u64 <= MAX_WORD_BYTES,
        "Hotword input too large"
    );
    Ok(words)
}
fn validate_words(raw: &str) -> Result<String> {
    anyhow::ensure!(
        raw.len() as u64 <= MAX_WORD_BYTES,
        "Hotword input too large"
    );
    let words: Vec<_> = raw
        .lines()
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .collect();
    anyhow::ensure!(
        words.len() <= 80
            && words
                .iter()
                .all(|w| w.chars().count() <= 80
                    && w.chars().all(|c| c.is_alphanumeric() || c == ' ')),
        "Invalid hotword input"
    );
    Ok(words.join("\n"))
}

pub struct Decoder {
    // Destroy the stream before the recognizer that created it.
    pub stream: OnlineStream,
    pub recognizer: OnlineRecognizer,
    config: OnlineRecognizerConfig,
    words: String,
    mode: Mode,
    model: PathBuf,
    audio: PathBuf,
    shared: Option<PathBuf>,
    fallback: Option<String>,
    model_load_attempts: u32,
    stream_updates: u32,
    initial_load_ms: u64,
    update_ms: u64,
}
fn load(
    config: &mut OnlineRecognizerConfig,
    words: &str,
    mode: Mode,
    model: &Path,
    audio: &Path,
    shared: Option<&Path>,
) -> Result<(OnlineRecognizer, Option<String>, u32)> {
    config.decoding_method = Some("greedy_search".into());
    config.hotwords_buf = None;
    config.model_config.modeling_unit = None;
    config.model_config.bpe_vocab = None;
    let mut fallback = None;
    if !words.is_empty() {
        match hotwords::vocabulary(model, audio, shared) {
            Ok(path) => {
                config.decoding_method = Some("modified_beam_search".into());
                config.max_active_paths = 4;
                config.hotwords_score = 1.5;
                // Per-stream words are added to defaults by sherpa: defaults must be empty.
                if mode == Mode::Legacy {
                    config.hotwords_buf = Some(words.as_bytes().to_vec());
                }
                config.model_config.modeling_unit = Some("cjkchar+bpe".into());
                config.model_config.bpe_vocab = Some(path.to_string_lossy().into_owned());
            }
            Err(error) => fallback = Some(format!("{error:#}")),
        }
    }
    let hotword_decoder = config.decoding_method.as_deref() == Some("modified_beam_search");
    let mut attempts = 1;
    let mut recognizer = OnlineRecognizer::create(config);
    if recognizer.is_none() && hotword_decoder {
        fallback = Some("Hotword decoder unavailable".into());
        config.decoding_method = Some("greedy_search".into());
        config.hotwords_buf = None;
        config.model_config.modeling_unit = None;
        config.model_config.bpe_vocab = None;
        attempts += 1;
        recognizer = OnlineRecognizer::create(config);
    }
    Ok((
        recognizer.context("无法加载双语识别模型")?,
        fallback,
        attempts,
    ))
}
fn enabled(config: &OnlineRecognizerConfig) -> bool {
    config.decoding_method.as_deref() == Some("modified_beam_search")
}
fn new_stream(
    recognizer: &OnlineRecognizer,
    config: &OnlineRecognizerConfig,
    words: &str,
    mode: Mode,
) -> OnlineStream {
    if mode == Mode::Stream && enabled(config) && !words.is_empty() {
        recognizer.create_stream_with_hotwords(words)
    } else {
        recognizer.create_stream()
    }
}
impl Decoder {
    pub fn new(
        mut config: OnlineRecognizerConfig,
        raw: &str,
        mode: Mode,
        model: &Path,
        audio: &Path,
        shared: Option<&Path>,
    ) -> Result<Self> {
        let at = Instant::now();
        let words = validate_words(raw);
        let invalid = words.is_err();
        let words = words.unwrap_or_default();
        let (recognizer, mut fallback, model_load_attempts) =
            load(&mut config, &words, mode, model, audio, shared)?;
        if invalid {
            fallback = Some("Invalid hotword input; using ordinary decoder".into());
        }
        let stream = new_stream(&recognizer, &config, &words, mode);
        let decoder = Self {
            stream,
            recognizer,
            config,
            words,
            mode,
            model: model.into(),
            audio: audio.into(),
            shared: shared.map(Path::to_owned),
            fallback,
            model_load_attempts,
            stream_updates: 0,
            initial_load_ms: at.elapsed().as_millis() as u64,
            update_ms: 0,
        };
        decoder.save_diagnostics();
        Ok(decoder)
    }
    /// Called only after the old utterance has been committed and reset.
    pub fn update(&mut self, raw: &str) -> Result<bool> {
        let words = validate_words(raw)?;
        if words == self.words {
            return Ok(false);
        }
        let at = Instant::now();
        if self.mode == Mode::Stream && enabled(&self.config) && !words.is_empty() {
            self.stream = self.recognizer.create_stream_with_hotwords(&words);
            self.stream_updates += 1;
        } else {
            // Enabling/disabling contextual decoding changes the search algorithm.
            let mut config = self.config.clone();
            let (recognizer, fallback, attempts) = load(
                &mut config,
                &words,
                self.mode,
                &self.model,
                &self.audio,
                self.shared.as_deref(),
            )?;
            self.stream = new_stream(&recognizer, &config, &words, self.mode);
            self.recognizer = recognizer;
            self.config = config;
            self.fallback = fallback;
            self.model_load_attempts += attempts;
        }
        self.words = words;
        self.update_ms = at.elapsed().as_millis() as u64;
        self.save_diagnostics();
        Ok(true)
    }
    pub fn refresh(&mut self, path: Option<&Path>) {
        if let Some(words) = path.and_then(|p| read_words(p).ok()) {
            let _ = self.update(&words);
        }
    }
    pub fn diagnostics(&self) -> Value {
        json!({"decoder":self.config.decoding_method,"threads":self.config.model_config.num_threads,
            "model_type":self.config.model_config.model_type,
            "hotword_count":if enabled(&self.config) { self.words.lines().count() } else {0},"fallback":self.fallback,
            "hotword_update_mode":self.mode.name(),"model_load_attempts":self.model_load_attempts,
            "stream_updates":self.stream_updates,"initial_load_ms":self.initial_load_ms,"last_update_ms":self.update_ms,
            "default_hotword_buffer_present":self.config.hotwords_buf.is_some()})
    }
    fn save_diagnostics(&self) {
        let _ = fs::write(
            self.audio.join("asr-config.json"),
            serde_json::to_vec_pretty(&self.diagnostics()).unwrap(),
        );
    }
}

/// Explicit model-lifecycle microbenchmark; no audio, accuracy or GPU assertion.
pub fn benchmark(
    config: OnlineRecognizerConfig,
    model: &Path,
    audio: &Path,
    shared: Option<&Path>,
) -> Result<()> {
    let mut decoder = Decoder::new(
        config,
        "OMASCRIBE\nPROJECT ALPHA",
        Mode::configured(),
        model,
        audio,
        shared,
    )?;
    anyhow::ensure!(
        enabled(&decoder.config),
        "Benchmark needs a compatible local hotword tokenizer"
    );
    let mut updates = Vec::new();
    for index in 0..12 {
        let words = if index % 2 == 0 {
            "OMASCRIBE\nPROJECT BETA"
        } else {
            "OMASCRIBE\nPROJECT ALPHA"
        };
        let at = Instant::now();
        anyhow::ensure!(decoder.update(words)?, "Expected a changed vocabulary");
        updates.push(at.elapsed().as_secs_f64() * 1000.);
    }
    let after_updates = decoder.diagnostics();
    anyhow::ensure!(
        !decoder.update("OMASCRIBE\nPROJECT ALPHA")?,
        "Unchanged context should be a no-op"
    );
    decoder.update("")?;
    anyhow::ensure!(
        !enabled(&decoder.config),
        "Empty context should return to ordinary decoding"
    );
    decoder.update("OMASCRIBE\nPROJECT ALPHA")?;
    anyhow::ensure!(enabled(&decoder.config), "Context should be enabled again");
    println!(
        "{}",
        json!({"schema":1,"kind":"hotword-lifecycle-only","updates_ms":updates,
        "after_updates":after_updates,"after_disable_enable":decoder.diagnostics(),"unchanged_noop":true,
        "peak_process_tree_rss_mib":null,"process_tree_cpu_seconds":null,"includes_audio":false,
        "core_build":omascribe_core::build_identity()})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_words_reject_native_syntax_controls_and_nul() {
        assert!(Mode::parse(None) == Mode::Legacy);
        assert!(Mode::parse(Some("0")) == Mode::Legacy);
        assert!(Mode::parse(Some("true")) == Mode::Legacy);
        assert!(Mode::parse(Some("1")) == Mode::Stream);
        assert_eq!(
            validate_words(" OMASCRIBE \n\n转写\n").unwrap(),
            "OMASCRIBE\n转写"
        );
        for raw in [
            "A\0B".into(),
            "NAME :99999".into(),
            "ONE/TWO".into(),
            "X\n".repeat(81),
            "X".repeat(81),
        ] {
            assert!(validate_words(&raw).is_err());
        }
        assert_eq!(validate_words(" \n").unwrap(), "");
    }
}
