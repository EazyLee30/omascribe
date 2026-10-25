mod diarization;
mod hotwords;
mod level;
use anyhow::{Context, Result};
use omascribe_core::{AsrEvent, CHUNK_SAMPLES, SAMPLE_RATE};
use sherpa_onnx::{OnlineRecognizer, OnlineRecognizerConfig};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    time::{Duration, Instant},
};

fn emit(event: AsrEvent) {
    println!("{}", serde_json::to_string(&event).unwrap());
    let _ = std::io::stdout().flush();
}
/// Optional spelling context must never prevent audio transcription from starting.
fn create_recognizer(
    config: &mut OnlineRecognizerConfig,
    words: &str,
    model: &Path,
    audio: &Path,
    shared: Option<&Path>,
) -> Result<OnlineRecognizer> {
    config.decoding_method = Some("greedy_search".into());
    config.hotwords_buf = None;
    config.model_config.modeling_unit = None;
    config.model_config.bpe_vocab = None;
    let mut fallback = None;
    if !words.trim().is_empty() && words.len() <= 16000 {
        match hotwords::vocabulary(model, audio, shared) {
            Ok(path) => {
                config.decoding_method = Some("modified_beam_search".into());
                config.max_active_paths = 4;
                config.hotwords_score = 1.5;
                config.hotwords_buf = Some(words.as_bytes().to_vec());
                config.model_config.modeling_unit = Some("cjkchar+bpe".into());
                config.model_config.bpe_vocab = Some(path.to_string_lossy().into_owned());
            }
            Err(error) => fallback = Some(format!("{error:#}")),
        }
    }
    let mut recognizer = OnlineRecognizer::create(config);
    if recognizer.is_none() && config.hotwords_buf.is_some() {
        fallback = Some("Hotword decoder unavailable".into());
        config.decoding_method = Some("greedy_search".into());
        config.hotwords_buf = None;
        config.model_config.modeling_unit = None;
        config.model_config.bpe_vocab = None;
        recognizer = OnlineRecognizer::create(config);
    }
    // Metadata only: no user vocabulary or transcript content in this diagnostic.
    let _ = fs::write(
        audio.join("asr-config.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "decoder": config.decoding_method, "threads": config.model_config.num_threads,
            "hotword_count": if config.hotwords_buf.is_some() { words.lines().filter(|s| !s.trim().is_empty()).count() } else { 0 },
            "fallback": fallback,
        }))?,
    );
    recognizer.context("无法加载双语识别模型")
}
#[allow(clippy::too_many_arguments)]
fn reload_hotwords(
    args: &[String],
    words: &mut String,
    config: &mut OnlineRecognizerConfig,
    recognizer: &mut OnlineRecognizer,
    stream: &mut sherpa_onnx::OnlineStream,
    model: &Path,
    audio: &Path,
    shared: Option<&Path>,
) {
    // Switch at utterance boundaries only; preserve sample positions and passage IDs.
    if let Some(updated) = args.get(6).and_then(|p| fs::read_to_string(p).ok()) {
        if updated != *words && updated.len() <= 16000 {
            if let Ok(next) = create_recognizer(config, &updated, model, audio, shared) {
                *stream = next.create_stream();
                *recognizer = next;
                *words = updated;
            }
        }
    }
}
fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "--diarize") {
        return diarization::run(&args);
    }
    let model = Path::new(
        args.get(1)
            .context("用法：omascribe-asr MODEL_DIR AUDIO_DIR")?,
    );
    let audio = Path::new(args.get(2).context("缺少音频目录")?);
    let path = |s: &str| Some(model.join(s).to_string_lossy().into_owned());
    let mut config = OnlineRecognizerConfig::default();
    config.model_config.transducer.encoder = path("encoder-epoch-99-avg-1.int8.onnx");
    config.model_config.transducer.decoder = path("decoder-epoch-99-avg-1.onnx");
    config.model_config.transducer.joiner = path("joiner-epoch-99-avg-1.int8.onnx");
    config.model_config.tokens = path("tokens.txt");
    config.model_config.num_threads = std::env::var("OMASCRIBE_ASR_THREADS")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(1)
        .clamp(1, 16);
    config.model_config.provider = Some("cpu".into());
    let mut hotwords = args
        .get(6)
        .and_then(|p| fs::read_to_string(p).ok())
        .unwrap_or_default();
    let shared = args.get(7).filter(|s| !s.is_empty()).map(Path::new);
    config.enable_endpoint = true;
    config.rule1_min_trailing_silence = 2.4;
    config.rule2_min_trailing_silence = 0.8;
    config.rule3_min_utterance_length = 12.;
    let mut recognizer = create_recognizer(&mut config, &hotwords, model, audio, shared)?;
    let mut stream = recognizer.create_stream();
    emit(AsrEvent::Ready);
    let initial: u64 = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(0);
    let mut boundaries: std::collections::VecDeque<u64> = args
        .get(5)
        .map(String::as_str)
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    boundaries.retain(|end| *end > initial);
    let mut processed = initial;
    let mut segment_start = initial;
    let mut seq: u32 = args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(1);
    let mut last_text = String::new();
    let mut level = level::SpeechLevel::default();
    let mut busy = Duration::ZERO;
    let mut last_progress = Instant::now();
    let mut pause_flushed = false;
    loop {
        let index = processed / CHUNK_SAMPLES;
        let offset = processed % CHUNK_SAMPLES;
        let p = audio.join(format!("{index:06}.pcm"));
        let available = fs::metadata(&p)
            .map(|m| m.len() / 2)
            .unwrap_or(0)
            .saturating_sub(offset);
        let take_boundary = boundaries.front().is_some_and(|end| *end == processed);
        if take_boundary
            || (available == 0 && audio.join("capture.paused").exists() && !pause_flushed)
        {
            let timer = Instant::now();
            stream.accept_waveform(SAMPLE_RATE as i32, &vec![0f32; 4800]);
            while recognizer.is_ready(&stream) {
                recognizer.decode(&stream);
            }
            busy += timer.elapsed();
            let text = recognizer
                .get_result(&stream)
                .map(|r| r.text)
                .unwrap_or_default()
                .trim()
                .to_string();
            if !text.is_empty() {
                emit(AsrEvent::Segment {
                    seq,
                    start_ms: segment_start * 1000 / SAMPLE_RATE,
                    end_ms: processed * 1000 / SAMPLE_RATE,
                    text,
                });
                seq += 1;
            }
            recognizer.reset(&stream);
            reload_hotwords(
                &args,
                &mut hotwords,
                &mut config,
                &mut recognizer,
                &mut stream,
                model,
                audio,
                shared,
            );
            segment_start = processed;
            last_text.clear();
            emit(AsrEvent::Partial {
                text: String::new(),
            });
            pause_flushed = true;
            if take_boundary {
                boundaries.pop_front();
            }
        }
        if available == 0 {
            if audio.join("capture.done").exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
            continue;
        }
        pause_flushed = false;
        let count = available.min(3200).min(
            boundaries
                .front()
                .map_or(u64::MAX, |end| end.saturating_sub(processed)),
        );
        let mut f = File::open(p)?;
        f.seek(SeekFrom::Start(offset * 2))?;
        let mut bytes = vec![0u8; count as usize * 2];
        f.read_exact(&mut bytes)?;
        let mut samples: Vec<f32> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| i16::from_le_bytes([p[0], p[1]]) as f32 / 32768.)
            .collect();
        // Improve quiet speech only in the recognizer input; archived PCM/WAV stays exact.
        level.process(&mut samples);
        let timer = Instant::now();
        stream.accept_waveform(SAMPLE_RATE as i32, &samples);
        while recognizer.is_ready(&stream) {
            recognizer.decode(&stream);
        }
        busy += timer.elapsed();
        processed += count;
        let text = recognizer
            .get_result(&stream)
            .map(|r| r.text)
            .unwrap_or_default()
            .trim()
            .to_string();
        if text != last_text {
            emit(AsrEvent::Partial { text: text.clone() });
            last_text = text.clone();
        }
        if recognizer.is_endpoint(&stream) {
            if !text.is_empty() {
                emit(AsrEvent::Segment {
                    seq,
                    start_ms: segment_start * 1000 / SAMPLE_RATE,
                    end_ms: processed * 1000 / SAMPLE_RATE,
                    text,
                });
                seq += 1;
            }
            recognizer.reset(&stream);
            reload_hotwords(
                &args,
                &mut hotwords,
                &mut config,
                &mut recognizer,
                &mut stream,
                model,
                audio,
                shared,
            );
            segment_start = processed;
            last_text.clear();
            emit(AsrEvent::Partial {
                text: String::new(),
            });
        }
        if last_progress.elapsed() > Duration::from_millis(300) {
            emit(AsrEvent::Progress {
                processed_ms: processed * 1000 / SAMPLE_RATE,
                rtf: busy.as_secs_f64()
                    / (processed.saturating_sub(initial).max(1) as f64 / SAMPLE_RATE as f64),
            });
            last_progress = Instant::now();
        }
    }
    let timer = Instant::now();
    stream.accept_waveform(SAMPLE_RATE as i32, &vec![0f32; 4800]);
    stream.input_finished();
    while recognizer.is_ready(&stream) {
        recognizer.decode(&stream);
    }
    busy += timer.elapsed();
    let text = recognizer
        .get_result(&stream)
        .map(|r| r.text)
        .unwrap_or_default()
        .trim()
        .to_string();
    if !text.is_empty() {
        emit(AsrEvent::Segment {
            seq,
            start_ms: segment_start * 1000 / SAMPLE_RATE,
            end_ms: processed * 1000 / SAMPLE_RATE,
            text,
        });
    }
    emit(AsrEvent::Done {
        processed_ms: processed * 1000 / SAMPLE_RATE,
        rtf: busy.as_secs_f64()
            / (processed.saturating_sub(initial).max(1) as f64 / SAMPLE_RATE as f64),
    });
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        emit(AsrEvent::Error {
            message: format!("{e:#}"),
        });
        std::process::exit(1);
    }
}
