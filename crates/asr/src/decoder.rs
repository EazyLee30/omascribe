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
    InventoryUnavailable,
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
            Self::InventoryUnavailable => "inventory_unavailable",
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

fn prepare_input(
    input: std::result::Result<String, InputError>,
    model: &Path,
    shared: Option<&Path>,
    known_scope: bool,
) -> std::result::Result<hotwords::Prepared, InputError> {
    let raw = input?;
    // The existing wire validator owns syntax/byte/term limits. Coverage
    // ordinals count normalized nonempty wire lines, matching publication.
    validated(raw.clone())?;
    hotwords::prepare(model, shared, &raw, known_scope)
        .map_err(|_| InputError::InventoryUnavailable)
}
/// Inventory work is measured separately from native context updates, including
/// rejected and unchanged inputs. Fractional milliseconds retain short reads.
fn timed_prepare_input(
    input: std::result::Result<String, InputError>,
    model: &Path,
    shared: Option<&Path>,
    known_scope: bool,
) -> (std::result::Result<hotwords::Prepared, InputError>, f64) {
    let at = Instant::now();
    let result = prepare_input(input, model, shared, known_scope);
    (result, at.elapsed().as_secs_f64() * 1000.)
}
fn known_coverage_scope(config: &OnlineRecognizerConfig) -> bool {
    // This worker installs cjkchar+bpe for the pinned Zipformer families.
    // A future model family is forwarded as unknown, never guessed unsupported.
    config.model_config.model_type.as_deref() == Some("zipformer")
}

pub struct Decoder {
    // Destroy the stream before the recognizer that created it.
    pub stream: OnlineStream,
    pub recognizer: OnlineRecognizer,
    config: OnlineRecognizerConfig,
    words: String,
    inventory: Option<[u8; 32]>,
    coverage: hotwords::Coverage,
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
    inventory_prepare_ms: f64,
}
fn configure_context(
    config: &mut OnlineRecognizerConfig,
    words: &str,
    mode: Mode,
    vocabulary: Option<&Path>,
    tokens: Option<&[u8]>,
) -> Result<()> {
    config.decoding_method = Some("greedy_search".into());
    config.hotwords_buf = None;
    config.model_config.modeling_unit = None;
    config.model_config.bpe_vocab = None;
    if !words.is_empty() {
        let path = vocabulary.context("Prepared hotword vocabulary unavailable")?;
        let tokens = tokens.context("Prepared token inventory unavailable")?;
        anyhow::ensure!(!tokens.is_empty(), "Prepared token inventory unavailable");
        // Fixed sherpa1.13.8 gives tokens_buf priority. Bind the symbol table
        // to the same complete bytes used by coverage, never a later path read.
        config.model_config.tokens = None;
        config.model_config.tokens_buf = Some(tokens.to_vec());
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
    Ok(())
}
fn load(
    config: &mut OnlineRecognizerConfig,
    words: &str,
    mode: Mode,
    vocabulary: Option<&Path>,
    tokens: Option<&[u8]>,
) -> Result<(OnlineRecognizer, Option<String>, u32)> {
    configure_context(config, words, mode, vocabulary, tokens)?;
    let mut fallback = None;
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
        let (preparation, inventory_prepare_ms) =
            timed_prepare_input(input, model, shared, known_coverage_scope(&config));
        let (mut prepared, mut input_status) = match preparation {
            Ok(prepared) => (prepared, InputStatus::accepted("startup")),
            Err(error) => (
                hotwords::Prepared::unavailable(),
                InputStatus::rejected("startup", error, false),
            ),
        };
        let vocabulary = match prepared.export(audio) {
            Ok(path) => path,
            Err(_) => {
                input_status =
                    InputStatus::rejected("startup", InputError::InventoryUnavailable, false);
                prepared = hotwords::Prepared::unavailable();
                None
            }
        };
        let (recognizer, fallback, model_load_attempts) = load(
            &mut config,
            &prepared.words,
            mode,
            vocabulary.as_deref(),
            prepared.tokens(),
        )?;
        let stream = new_stream(&recognizer, &config, &prepared.words, mode);
        let inventory = prepared.inventory_id();
        let decoder = Self {
            stream,
            recognizer,
            config,
            words: prepared.words,
            inventory,
            coverage: prepared.coverage,
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
            inventory_prepare_ms,
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
        let (preparation, elapsed) = timed_prepare_input(
            input,
            &self.model,
            self.shared.as_deref(),
            known_coverage_scope(&self.config),
        );
        self.inventory_prepare_ms = elapsed;
        let prepared = match preparation {
            Ok(prepared) => prepared,
            Err(error) => {
                self.input_status = InputStatus::rejected(stage, error.clone(), true);
                // Repeated failures still have a newly measured preparation.
                self.save_diagnostics();
                return Err(error.into());
            }
        };
        let changed = prepared.changes_native_context(&self.words, self.inventory);
        // Preserve existing accepted/rejected metadata behavior, but compare
        // only after the whole inventory check has succeeded.
        if let Err(error) =
            self.input_status
                .prepare_update(Ok(prepared.words.clone()), stage, &self.words)
        {
            self.save_diagnostics();
            return Err(error.into());
        }
        if !changed {
            self.inventory = prepared.inventory_id();
            self.coverage = prepared.coverage;
            self.input_status = InputStatus::accepted(stage);
            // A native no-op still performed inventory I/O and parsing.
            self.save_diagnostics();
            return Ok(false);
        }
        let at = Instant::now();
        if self.mode == Mode::Stream
            && enabled(&self.config)
            && !prepared.words.is_empty()
            && prepared.inventory_id() == self.inventory
        {
            self.stream = self.recognizer.create_stream_with_hotwords(&prepared.words);
            self.stream_updates += 1;
        } else {
            // Inventory changes require a new native tokenizer even if the
            // filtered words are identical; never reuse it based on timestamps.
            let vocabulary = match prepared.export(&self.audio) {
                Ok(path) => path,
                Err(_) => {
                    self.input_status =
                        InputStatus::rejected(stage, InputError::InventoryUnavailable, true);
                    self.save_diagnostics();
                    return Err(InputError::InventoryUnavailable.into());
                }
            };
            let mut config = self.config.clone();
            let loaded = load(
                &mut config,
                &prepared.words,
                self.mode,
                vocabulary.as_deref(),
                prepared.tokens(),
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
            self.stream = new_stream(&recognizer, &config, &prepared.words, self.mode);
            self.recognizer = recognizer;
            self.config = config;
            self.fallback = fallback;
            self.model_load_attempts += attempts;
        }
        self.inventory = prepared.inventory_id();
        self.words = prepared.words;
        self.coverage = prepared.coverage;
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
            "hotword_coverage":self.coverage.diagnostics(),
            "fallback":self.input_status.fallback().or_else(|| self.fallback.clone()),
            "hotword_input":self.input_status.diagnostics(),
            "hotword_update_mode":self.mode.name(),"model_load_attempts":self.model_load_attempts,
            "stream_updates":self.stream_updates,"initial_load_ms":self.initial_load_ms,"last_update_ms":self.update_ms,
            "last_inventory_prepare_ms":self.inventory_prepare_ms,
            "default_hotword_buffer_present":self.config.hotwords_buf.is_some(),
            "token_inventory_buffer_present":self.config.model_config.tokens_buf.is_some()})
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

    #[test]
    fn contextual_config_uses_the_checked_token_snapshot_for_both_modes() -> Result<()> {
        let root = hotwords::tests::coverage_fixture()?;
        let mut config = OnlineRecognizerConfig::default();
        config.model_config.model_type = Some("zipformer".into());
        config.model_config.tokens = Some(root.0.join("tokens.txt").to_string_lossy().into_owned());
        let prepared = prepare_input(Ok("CHINA\nF1".into()), &root.0, None, true)?;
        let checked = fs::read(root.0.join("tokens.txt"))?;
        let audio = root.0.join("audio");
        fs::create_dir(&audio)?;
        let exported = prepared.export(&audio)?;
        // A path edit after prepare cannot replace the native symbol table.
        fs::write(root.0.join("tokens.txt"), b"invalid later path contents")?;
        for mode in [Mode::Legacy, Mode::Stream] {
            let mut next = config.clone();
            configure_context(
                &mut next,
                &prepared.words,
                mode,
                exported.as_deref(),
                prepared.tokens(),
            )?;
            assert!(next.model_config.tokens.is_none());
            assert_eq!(next.model_config.tokens_buf.as_ref(), Some(&checked));
            assert_eq!(next.max_active_paths, 4);
            assert_eq!(next.hotwords_score, 1.5);
            assert_eq!(next.hotwords_buf.is_some(), mode == Mode::Legacy);
        }
        Ok(())
    }

    #[test]
    fn inventory_failure_does_not_export_or_silently_produce_empty_context() -> Result<()> {
        let root = hotwords::tests::coverage_fixture()?;
        let retained = prepare_input(Ok("CHINA".into()), &root.0, None, true)?;
        let audio = root.0.join("audio");
        fs::create_dir(&audio)?;
        let exported = retained.export(&audio)?.unwrap();
        let before = fs::read(&exported)?;
        fs::write(root.0.join("bpe.model"), b"private malformed inventory")?;
        let error = match prepare_input(Ok("CHINA\nF1".into()), &root.0, None, true) {
            Ok(_) => panic!("invalid inventory must reject the whole candidate"),
            Err(error) => error,
        };
        assert_eq!(error, InputError::InventoryUnavailable);
        let state = InputStatus::rejected("refresh", error, true);
        assert_eq!(state.diagnostics()["previous_context_retained"], true);
        assert_eq!(
            state.diagnostics()["error"]["code"],
            "inventory_unavailable"
        );
        assert!(!state.diagnostics().to_string().contains("private"));
        assert!(!state.diagnostics().to_string().contains("CHINA"));
        assert_eq!(retained.words, "CHINA");
        assert_eq!(fs::read(exported)?, before);
        Ok(())
    }

    #[test]
    fn inventory_prepare_timing_covers_success_failure_and_effective_noop() -> Result<()> {
        let root = hotwords::tests::coverage_fixture()?;
        let (first, success_ms) = timed_prepare_input(Ok("CHINA".into()), &root.0, None, true);
        let first = first?;
        let (unchanged, noop_ms) = timed_prepare_input(Ok("CHINA\nF1".into()), &root.0, None, true);
        let unchanged = unchanged?;
        assert!(!unchanged.changes_native_context(&first.words, first.inventory_id()));
        fs::write(root.0.join("bpe.model"), b"malformed inventory")?;
        let (failed, failure_ms) = timed_prepare_input(Ok("CHINA".into()), &root.0, None, true);
        assert!(matches!(failed, Err(InputError::InventoryUnavailable)));
        for elapsed in [success_ms, noop_ms, failure_ms] {
            assert!(elapsed.is_finite() && elapsed >= 0.);
            assert!(
                json!({"last_inventory_prepare_ms":elapsed})["last_inventory_prepare_ms"]
                    .is_number()
            );
        }
        Ok(())
    }
}
