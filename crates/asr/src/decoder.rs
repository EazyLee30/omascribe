//! Model ownership and utterance-boundary hotwords. Stream updates remain opt-in.
use crate::hotwords;
use anyhow::{Context, Result};
use omascribe_core::asr_hotwords::{validate_words, MAX_WORD_BYTES};
use serde_json::{json, Value};
use sherpa_onnx::{OnlineRecognizer, OnlineRecognizerConfig, OnlineStream};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Instant,
};

/// Only stable categories are persisted: file paths and vocabulary text stay private.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputError {
    Read { kind: String, os_code: Option<i32> },
    TooLarge,
    InvalidUtf8,
    InvalidWords,
    DecoderUpdate,
}
impl InputError {
    fn read(error: std::io::Error) -> Self {
        Self::Read {
            kind: format!("{:?}", error.kind()),
            os_code: error.raw_os_error(),
        }
    }
    fn code(&self) -> &'static str {
        match self {
            Self::Read { .. } => "read_failed",
            Self::TooLarge => "input_too_large",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::InvalidWords => "invalid_words",
            Self::DecoderUpdate => "decoder_update_failed",
        }
    }
    fn diagnostics(&self) -> Value {
        match self {
            Self::Read { kind, os_code } => {
                json!({"code":self.code(),"io_kind":kind,"os_code":os_code})
            }
            Self::TooLarge => json!({"code":self.code(),"max_bytes":MAX_WORD_BYTES}),
            _ => json!({"code":self.code()}),
        }
    }
}
impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for InputError {}

#[derive(Clone, PartialEq, Eq)]
struct InputStatus {
    stage: &'static str,
    error: Option<InputError>,
    previous_context_retained: bool,
}
impl InputStatus {
    fn accepted(stage: &'static str) -> Self {
        Self {
            stage,
            error: None,
            previous_context_retained: false,
        }
    }
    fn rejected(stage: &'static str, error: InputError, previous_context_retained: bool) -> Self {
        Self {
            stage,
            error: Some(error),
            previous_context_retained,
        }
    }
    /// Validate before touching native state. None is an unchanged-context no-op.
    fn prepare_update(
        &mut self,
        input: std::result::Result<String, InputError>,
        stage: &'static str,
        current: &str,
    ) -> std::result::Result<Option<String>, InputError> {
        let words = match input.and_then(validated) {
            Ok(words) => words,
            Err(error) => {
                *self = Self::rejected(stage, error.clone(), true);
                return Err(error);
            }
        };
        if words == current {
            *self = Self::accepted(stage);
            return Ok(None);
        }
        Ok(Some(words))
    }
    fn diagnostics(&self) -> Value {
        json!({"stage":self.stage,"status":if self.error.is_some() {"rejected"} else {"accepted"},
            "error":self.error.as_ref().map(InputError::diagnostics),
            "previous_context_retained":self.previous_context_retained})
    }
    fn fallback(&self) -> Option<String> {
        self.error.as_ref().map(|error| {
            if self.previous_context_retained {
                format!(
                    "Hotword update rejected ({}); previous context retained",
                    error.code()
                )
            } else {
                format!(
                    "Hotword input rejected ({}); using ordinary decoder",
                    error.code()
                )
            }
        })
    }
}

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

pub fn read_words(path: &Path) -> std::result::Result<String, InputError> {
    let file = fs::File::open(path).map_err(InputError::read)?;
    let mut bytes = Vec::new();
    file.take((MAX_WORD_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(InputError::read)?;
    // Check the byte limit before UTF-8: the bounded read can split a multibyte word.
    if bytes.len() > MAX_WORD_BYTES {
        return Err(InputError::TooLarge);
    }
    String::from_utf8(bytes).map_err(|_| InputError::InvalidUtf8)
}
fn validated(raw: String) -> std::result::Result<String, InputError> {
    if raw.len() > MAX_WORD_BYTES {
        return Err(InputError::TooLarge);
    }
    validate_words(&raw).map_err(|_| InputError::InvalidWords)
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
    input_status: InputStatus,
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
    pub fn from_path(
        config: OnlineRecognizerConfig,
        path: Option<&Path>,
        mode: Mode,
        model: &Path,
        audio: &Path,
        shared: Option<&Path>,
    ) -> Result<Self> {
        Self::with_input(
            config,
            path.map(read_words).unwrap_or_else(|| Ok(String::new())),
            mode,
            model,
            audio,
            shared,
        )
    }
    pub fn new(
        config: OnlineRecognizerConfig,
        raw: &str,
        mode: Mode,
        model: &Path,
        audio: &Path,
        shared: Option<&Path>,
    ) -> Result<Self> {
        Self::with_input(config, Ok(raw.to_owned()), mode, model, audio, shared)
    }
    fn with_input(
        mut config: OnlineRecognizerConfig,
        input: std::result::Result<String, InputError>,
        mode: Mode,
        model: &Path,
        audio: &Path,
        shared: Option<&Path>,
    ) -> Result<Self> {
        let at = Instant::now();
        let (words, input_status) = match input.and_then(validated) {
            Ok(words) => (words, InputStatus::accepted("startup")),
            Err(error) => (
                String::new(),
                InputStatus::rejected("startup", error, false),
            ),
        };
        let (recognizer, fallback, model_load_attempts) =
            load(&mut config, &words, mode, model, audio, shared)?;
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
            input_status,
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
        self.apply_input(Ok(raw.to_owned()), "update")
    }
    fn apply_input(
        &mut self,
        input: std::result::Result<String, InputError>,
        stage: &'static str,
    ) -> Result<bool> {
        let previous_input_status = self.input_status.clone();
        let words = match self.input_status.prepare_update(input, stage, &self.words) {
            Ok(Some(words)) => words,
            Ok(None) => {
                // A repaired file can contain the same valid context: clear the
                // error without changing the recognizer or creating a stream.
                if self.input_status != previous_input_status {
                    self.save_diagnostics();
                }
                return Ok(false);
            }
            Err(error) => {
                if self.input_status != previous_input_status {
                    self.save_diagnostics();
                }
                return Err(error.into());
            }
        };
        let at = Instant::now();
        if self.mode == Mode::Stream && enabled(&self.config) && !words.is_empty() {
            self.stream = self.recognizer.create_stream_with_hotwords(&words);
            self.stream_updates += 1;
        } else {
            // Enabling/disabling contextual decoding changes the search algorithm.
            let mut config = self.config.clone();
            let loaded = load(
                &mut config,
                &words,
                self.mode,
                &self.model,
                &self.audio,
                self.shared.as_deref(),
            );
            let (recognizer, fallback, attempts) = match loaded {
                Ok(loaded) => loaded,
                Err(error) => {
                    self.input_status =
                        InputStatus::rejected(stage, InputError::DecoderUpdate, true);
                    self.save_diagnostics();
                    return Err(error);
                }
            };
            self.stream = new_stream(&recognizer, &config, &words, self.mode);
            self.recognizer = recognizer;
            self.config = config;
            self.fallback = fallback;
            self.model_load_attempts += attempts;
        }
        self.words = words;
        self.input_status = InputStatus::accepted(stage);
        self.update_ms = at.elapsed().as_millis() as u64;
        self.save_diagnostics();
        Ok(true)
    }
    pub fn refresh(&mut self, path: Option<&Path>) {
        if let Some(path) = path {
            let _ = self.apply_input(read_words(path), "refresh");
        }
    }
    pub fn diagnostics(&self) -> Value {
        json!({"decoder":self.config.decoding_method,"threads":self.config.model_config.num_threads,
            "model_type":self.config.model_config.model_type,
            "hotword_count":if enabled(&self.config) { self.words.lines().count() } else {0},
            "fallback":self.input_status.fallback().or_else(|| self.fallback.clone()),
            "hotword_input":self.input_status.diagnostics(),
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

    struct TempInput(PathBuf);
    impl TempInput {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "omascribe-hotword-input-{}-{nonce}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn write(&self, bytes: impl AsRef<[u8]>) -> PathBuf {
            let path = self.0.join("private-hotword-context.txt");
            fs::write(&path, bytes).unwrap();
            path
        }
    }
    impl Drop for TempInput {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

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

    #[test]
    fn file_reads_classify_missing_utf8_and_multibyte_overflow_without_leaking_context() {
        let dir = TempInput::new();
        let missing = dir.0.join("private-missing-context");
        let error = read_words(&missing).unwrap_err();
        assert!(matches!(&error, InputError::Read { kind, .. } if kind == "NotFound"));
        let startup = InputStatus::rejected("startup", error, false);
        let diagnostics = startup.diagnostics().to_string();
        assert!(diagnostics.contains("read_failed"));
        assert!(diagnostics.contains("NotFound"));
        assert!(!diagnostics.contains("private-missing-context"));
        assert!(startup.fallback().unwrap().contains("ordinary decoder"));

        let malformed = dir.write([b'A', 0xff]);
        assert_eq!(read_words(&malformed).unwrap_err(), InputError::InvalidUtf8);
        // The MAX+1 bounded read ends inside a three-byte scalar. The cause is
        // oversized context, not malformed UTF-8 or a silently empty vocabulary.
        let oversized = dir.write("界".repeat(MAX_WORD_BYTES / 3 + 1));
        assert_eq!(read_words(&oversized).unwrap_err(), InputError::TooLarge);
    }

    #[test]
    fn exact_wire_limit_round_trips_and_native_syntax_diagnostics_are_private() {
        let dir = TempInput::new();
        let mut words = vec!["界".repeat(80); 66];
        words.push(format!("{}A", "界".repeat(31)));
        let exact = words.join("\n");
        assert_eq!(exact.len(), MAX_WORD_BYTES);
        let path = dir.write(&exact);
        assert_eq!(validated(read_words(&path).unwrap()).unwrap(), exact);
        let oversized = dir.write(exact + "A");
        assert_eq!(read_words(&oversized).unwrap_err(), InputError::TooLarge);

        let private_words = "PRIVATEPROJECT :99999";
        let error = validated(private_words.into()).unwrap_err();
        assert_eq!(error, InputError::InvalidWords);
        let rejected = InputStatus::rejected("refresh", error, true);
        let diagnostics = rejected.diagnostics();
        assert_eq!(diagnostics["error"]["code"], "invalid_words");
        assert_eq!(diagnostics["previous_context_retained"], true);
        assert!(!diagnostics.to_string().contains("PRIVATEPROJECT"));
        assert!(rejected
            .fallback()
            .unwrap()
            .contains("previous context retained"));
    }

    #[test]
    fn a_valid_unchanged_context_clears_rejection_metadata_without_changing_words() {
        let retained = "OMASCRIBE\nPROJECT ALPHA";
        let mut state = InputStatus::accepted("startup");
        assert_eq!(
            state
                .prepare_update(Ok("NAME :99999".into()), "refresh", retained)
                .unwrap_err(),
            InputError::InvalidWords
        );
        assert_eq!(state.diagnostics()["status"], "rejected");
        assert_eq!(state.diagnostics()["previous_context_retained"], true);
        let action = state
            .prepare_update(Ok(format!(" {retained} \n")), "refresh", retained)
            .unwrap();
        // The production apply_input returns before any native load for None.
        assert_eq!(action, None);
        assert_eq!(state.diagnostics()["status"], "accepted");
        assert_eq!(state.diagnostics()["error"], Value::Null);
        assert_eq!(state.diagnostics()["previous_context_retained"], false);
        assert_eq!(state.fallback(), None);
    }
}
