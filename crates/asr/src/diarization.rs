use anyhow::{bail, Context, Result};
use sherpa_onnx::{OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig};
use std::{fs, io::Read, path::Path};
pub fn run(args: &[String]) -> Result<()> {
    if args.len() < 8 {
        bail!("Usage: --diarize SEGMENTATION EMBEDDING AUDIO OUTPUT SPEAKERS THREADS");
    }
    let mut config = OfflineSpeakerDiarizationConfig::default();
    config.segmentation.pyannote.model = Some(args[2].clone());
    config.embedding.model = Some(args[3].clone());
    let count: i32 = args[6].parse()?;
    config.clustering.num_clusters = if count == 0 { -1 } else { count };
    let threads = args[7].parse::<i32>()?.clamp(1, 16);
    config.segmentation.num_threads = threads;
    config.embedding.num_threads = threads;
    let diarizer =
        OfflineSpeakerDiarization::create(&config).context("Unable to load speaker models")?;
    if diarizer.sample_rate() != 16000 {
        bail!("Expected 16 kHz speaker model");
    }
    let dir = Path::new(&args[4]);
    let count = omascribe_core::audio::samples_on_disk(dir)?;
    if count > 16000 * 7200 {
        bail!("Speaker analysis limited to two hours");
    }
    let mut samples = Vec::with_capacity(count as usize);
    let mut chunk = 0;
    while samples.len() < (count as usize) {
        let mut bytes = Vec::new();
        fs::File::open(dir.join(format!("{chunk:06}.pcm")))?.read_to_end(&mut bytes)?;
        samples.extend(
            bytes
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.),
        );
        chunk += 1;
    }
    let result = diarizer
        .process(&samples)
        .context("Speaker analysis failed")?;
    let turns: Vec<_> = result
        .sort_by_start_time()
        .into_iter()
        .filter(|t| t.speaker >= 0 && t.start.is_finite() && t.end.is_finite() && t.end > t.start)
        .map(|t| omascribe_core::speakers::Turn {
            start_ms: (t.start.max(0.) * 1000.) as u64,
            end_ms: (t.end * 1000.) as u64,
            speaker: t.speaker as u32,
        })
        .collect();
    fs::write(&args[5], serde_json::to_vec(&turns)?)?;
    Ok(())
}
