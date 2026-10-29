mod blocks;
mod decoder;
mod diarization;
mod hotwords;
mod level;
use anyhow::{Context, Result};
use omascribe_core::{AsrEvent, CHUNK_SAMPLES, SAMPLE_RATE};
use sherpa_onnx::OnlineRecognizerConfig;
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
fn recognizer_config(model: &Path) -> OnlineRecognizerConfig {
    let path = |s: &str| Some(model.join(s).to_string_lossy().into_owned());
    let mut config = OnlineRecognizerConfig::default();
    config.model_config.transducer.encoder = path("encoder-epoch-99-avg-1.int8.onnx");
    config.model_config.transducer.decoder = path("decoder-epoch-99-avg-1.onnx");
    config.model_config.transducer.joiner = path("joiner-epoch-99-avg-1.int8.onnx");
    config.model_config.tokens = path("tokens.txt");
    // Both pinned bilingual models are Zipformer. Avoid Sherpa loading the encoder
    // once just to detect its family before constructing the actual recognizer.
    config.model_config.model_type = Some("zipformer".into());
    config.model_config.num_threads = std::env::var("OMASCRIBE_ASR_THREADS")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(1)
        .clamp(1, 16);
    config.model_config.provider = Some("cpu".into());
    config.enable_endpoint = true;
    config.rule1_min_trailing_silence = 2.4;
    config.rule2_min_trailing_silence = 0.8;
    config.rule3_min_utterance_length = 12.;
    config
}
fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "--diarize") {
        return diarization::run(&args);
    }
    if args.get(1).is_some_and(|a| a == "--benchmark-hotwords") {
        let model = Path::new(args.get(2).context("Missing benchmark model directory")?);
        let audio = Path::new(
            args.get(3)
                .context("Missing empty benchmark output directory")?,
        );
        let shared = args.get(4).filter(|s| !s.is_empty()).map(Path::new);
        anyhow::ensure!(
            !audio.exists() || fs::read_dir(audio)?.next().is_none(),
            "Use an empty benchmark directory"
        );
        fs::create_dir_all(audio)?;
        return decoder::benchmark(recognizer_config(model), model, audio, shared);
    }
    let model = Path::new(
        args.get(1)
            .context("用法：omascribe-asr MODEL_DIR AUDIO_DIR")?,
    );
    let audio = Path::new(args.get(2).context("缺少音频目录")?);
    let hotword_path = args.get(6).map(Path::new);
    let shared = args.get(7).filter(|s| !s.is_empty()).map(Path::new);
    let mut decoder = decoder::Decoder::from_path(
        recognizer_config(model),
        hotword_path,
        decoder::Mode::configured(),
        model,
        audio,
        shared,
    )?;
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
    // Words can change while an empty stream waits, especially during pause.
    // Recheck only before its first actual PCM block, never during speech.
    let mut fresh_stream = true;
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
            decoder
                .stream
                .accept_waveform(SAMPLE_RATE as i32, &vec![0f32; 4800]);
            while decoder.recognizer.is_ready(&decoder.stream) {
                decoder.recognizer.decode(&decoder.stream);
            }
            busy += timer.elapsed();
            let text = decoder
                .recognizer
                .get_result(&decoder.stream)
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
            decoder.recognizer.reset(&decoder.stream);
            decoder.refresh(hotword_path);
            fresh_stream = true;
            segment_start = processed;
            last_text.clear();
            emit(AsrEvent::Partial {
                text: String::new(),
            });
            pause_flushed = true;
            if take_boundary {
                boundaries.pop_front();
            }
            emit(AsrEvent::Boundary {
                processed_ms: processed * 1000 / SAMPLE_RATE,
                cause: if take_boundary { "take" } else { "pause" }.into(),
            });
        }
        if available == 0 {
            if audio.join("capture.done").exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
            continue;
        }
        pause_flushed = false;
        // File reads can arrive in different packet sizes under Metal/CPU load.
        // Wait for a stable 200ms block so leveling and decode calls see the
        // same samples. Explicit pause/take/end boundaries still drain tails.
        let Some(count) = blocks::read_count(
            processed,
            available,
            boundaries.front().copied(),
            audio.join("capture.done").exists() || audio.join("capture.paused").exists(),
        ) else {
            std::thread::sleep(Duration::from_millis(40));
            continue;
        };
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
        // read_count(None) and failed reads above must not consume this state.
        // Every accepted block does, including a short pause/take/end tail.
        if fresh_stream {
            decoder.refresh(hotword_path);
            fresh_stream = false;
        }
        let timer = Instant::now();
        decoder.stream.accept_waveform(SAMPLE_RATE as i32, &samples);
        while decoder.recognizer.is_ready(&decoder.stream) {
            decoder.recognizer.decode(&decoder.stream);
        }
        busy += timer.elapsed();
        processed += count;
        let text = decoder
            .recognizer
            .get_result(&decoder.stream)
            .map(|r| r.text)
            .unwrap_or_default()
            .trim()
            .to_string();
        if text != last_text {
            emit(AsrEvent::Partial { text: text.clone() });
            last_text = text.clone();
        }
        if decoder.recognizer.is_endpoint(&decoder.stream) {
            if !text.is_empty() {
                emit(AsrEvent::Segment {
                    seq,
                    start_ms: segment_start * 1000 / SAMPLE_RATE,
                    end_ms: processed * 1000 / SAMPLE_RATE,
                    text,
                });
                seq += 1;
            }
            decoder.recognizer.reset(&decoder.stream);
            decoder.refresh(hotword_path);
            fresh_stream = true;
            segment_start = processed;
            last_text.clear();
            emit(AsrEvent::Partial {
                text: String::new(),
            });
            emit(AsrEvent::Boundary {
                processed_ms: processed * 1000 / SAMPLE_RATE,
                cause: "endpoint".into(),
            });
        }
        // Acknowledge the first decoded block and catch-up work immediately.
        // The scheduler must not mistake already decoded PCM for ASR backlog.
        if processed.saturating_sub(initial) == count
            || available > count
            || last_progress.elapsed() > Duration::from_millis(300)
        {
            emit(AsrEvent::Progress {
                processed_ms: processed * 1000 / SAMPLE_RATE,
                rtf: busy.as_secs_f64()
                    / (processed.saturating_sub(initial).max(1) as f64 / SAMPLE_RATE as f64),
            });
            last_progress = Instant::now();
        }
    }
    let timer = Instant::now();
    decoder
        .stream
        .accept_waveform(SAMPLE_RATE as i32, &vec![0f32; 4800]);
    decoder.stream.input_finished();
    while decoder.recognizer.is_ready(&decoder.stream) {
        decoder.recognizer.decode(&decoder.stream);
    }
    busy += timer.elapsed();
    let text = decoder
        .recognizer
        .get_result(&decoder.stream)
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
