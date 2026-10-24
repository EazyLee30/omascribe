//! Capture ownership and bounded streaming conversion to the recorder's 16 kHz mono PCM.
use anyhow::{Context, Result};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::{
    io::{self, Read},
    process::Child,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::JoinHandle,
};

pub struct Capture {
    pub stdout: Option<Box<dyn Read + Send>>,
    pub stop: Arc<AtomicBool>,
    child: Option<Child>,
    worker: Option<JoinHandle<Result<()>>>,
}
impl Capture {
    pub fn process(mut child: Child) -> Result<Self> {
        let stdout = child.stdout.take().context("Cannot read capture stream")?;
        Ok(Self {
            stdout: Some(Box::new(stdout)),
            stop: Arc::new(AtomicBool::new(false)),
            child: Some(child),
            worker: None,
        })
    }
    pub fn reader(
        reader: impl Read + Send + 'static,
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<Result<()>>>,
    ) -> Self {
        Self {
            stdout: Some(Box::new(reader)),
            stop,
            child: None,
            worker,
        }
    }
    pub fn id(&self) -> u32 {
        self.child.as_ref().map_or(0, Child::id)
    }
    pub fn kill(&mut self) -> io::Result<()> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(child) = &mut self.child {
            child.kill()?;
        }
        Ok(())
    }
    pub fn wait(&mut self) -> Result<bool> {
        if let Some(child) = &mut self.child {
            return Ok(child.wait()?.success());
        }
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| anyhow::anyhow!("Audio worker stopped unexpectedly"))??;
        }
        Ok(true)
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(child) = &mut self.child {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

pub trait MonoSource: Send {
    fn next_frames(&mut self) -> io::Result<Option<Vec<f32>>>;
}

/// Keep the resampler off the realtime callback. Trim filter delay and flush exactly the
/// input duration, so take boundaries stay sample-accurate at 44.1/48/96 kHz too.
pub struct PcmReader<S> {
    source: S,
    resampler: SincFixedIn<f32>,
    pending: Vec<f32>,
    bytes: io::Cursor<Vec<u8>>,
    rate: u32,
    input: u64,
    output: u64,
    delay: usize,
    eof: bool,
    stop: Arc<AtomicBool>,
}
impl<S: MonoSource> PcmReader<S> {
    pub fn new(source: S, rate: u32, stop: Arc<AtomicBool>) -> Result<Self> {
        anyhow::ensure!(
            (8000..=384000).contains(&rate),
            "Unsupported sample rate: {rate}"
        );
        let resampler = SincFixedIn::new(
            16000.0 / rate as f64,
            1.0,
            SincInterpolationParameters {
                sinc_len: 128,
                f_cutoff: 0.94,
                interpolation: SincInterpolationType::Linear,
                oversampling_factor: 128,
                window: WindowFunction::BlackmanHarris2,
            },
            1024,
            1,
        )?;
        let delay = resampler.output_delay();
        Ok(Self {
            source,
            resampler,
            pending: Vec::new(),
            bytes: io::Cursor::new(Vec::new()),
            rate,
            input: 0,
            output: 0,
            delay,
            eof: false,
            stop,
        })
    }
    fn fill(&mut self) -> io::Result<bool> {
        loop {
            while self.pending.len() < 1024 && !self.eof {
                // Native sources drain their callback queue after stop; file sources can finish immediately.
                match self.source.next_frames()? {
                    Some(frames) => {
                        self.input += frames.len() as u64;
                        self.pending.extend(frames);
                    }
                    None => self.eof = true,
                }
            }
            let expected = self.input * 16000 / self.rate as u64;
            if self.eof && self.output >= expected {
                return Ok(false);
            }
            if self.pending.len() < 1024 {
                self.pending.resize(1024, 0.);
            }
            let input: Vec<f32> = self.pending.drain(..1024).collect();
            let converted = self
                .resampler
                .process(&[input], None)
                .map_err(io::Error::other)?;
            let skip = self.delay.min(converted[0].len());
            self.delay -= skip;
            let mut frames = &converted[0][skip..];
            if self.eof {
                frames = &frames[..frames
                    .len()
                    .min(expected.saturating_sub(self.output) as usize)];
            }
            self.output += frames.len() as u64;
            let mut bytes = Vec::with_capacity(frames.len() * 2);
            for sample in frames {
                bytes.extend_from_slice(
                    &((sample.clamp(-1., 1.) * 32767.).round() as i16).to_le_bytes(),
                );
            }
            if !bytes.is_empty() {
                self.bytes = io::Cursor::new(bytes);
                return Ok(true);
            }
        }
    }
}
impl<S: MonoSource> Read for PcmReader<S> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.bytes.position() as usize == self.bytes.get_ref().len() && !self.fill()? {
            return Ok(0);
        }
        self.bytes.read(out)
    }
}
impl<S> Drop for PcmReader<S> {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

struct WavSource {
    reader: hound::WavReader<io::BufReader<std::fs::File>>,
    stop: Arc<AtomicBool>,
}
impl MonoSource for WavSource {
    fn next_frames(&mut self) -> io::Result<Option<Vec<f32>>> {
        if self.stop.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let spec = self.reader.spec();
        let samples: Vec<f32> = if spec.sample_format == hound::SampleFormat::Float {
            self.reader
                .samples::<f32>()
                .take(1024 * spec.channels as usize)
                .collect::<Result<Vec<_>, _>>()
                .map_err(io::Error::other)?
        } else {
            let divisor = 2f32.powi(spec.bits_per_sample as i32 - 1);
            self.reader
                .samples::<i32>()
                .take(1024 * spec.channels as usize)
                .map(|s| s.map(|v| v as f32 / divisor))
                .collect::<Result<Vec<_>, _>>()
                .map_err(io::Error::other)?
        };
        if samples.is_empty() {
            return Ok(None);
        }
        if !samples.len().is_multiple_of(spec.channels as usize) {
            return Err(io::Error::other("Incomplete WAV frame"));
        }
        Ok(Some(
            samples
                .chunks_exact(spec.channels as usize)
                .map(|c| c.iter().sum::<f32>() / spec.channels as f32)
                .collect(),
        ))
    }
}
pub fn wav(path: &std::path::Path) -> Result<Capture> {
    let reader = hound::WavReader::open(path).context("Cannot read PCM WAV file")?;
    let spec = reader.spec();
    anyhow::ensure!(
        spec.channels > 0 && spec.channels <= 32,
        "Unsupported WAV channel count"
    );
    let stop = Arc::new(AtomicBool::new(false));
    let stream = PcmReader::new(
        WavSource {
            reader,
            stop: stop.clone(),
        },
        spec.sample_rate,
        stop.clone(),
    )?;
    Ok(Capture::reader(stream, stop, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resampling_preserves_duration_and_rejects_aliasing() -> Result<()> {
        struct Source(Option<Vec<f32>>);
        impl MonoSource for Source {
            fn next_frames(&mut self) -> io::Result<Option<Vec<f32>>> {
                Ok(self.0.take())
            }
        }
        for rate in [16000u32, 44100, 48000, 96000] {
            for frequency in [1000., 12000.] {
                if frequency >= rate as f32 / 2. {
                    continue;
                }
                let input = (0..rate + 137)
                    .map(|n| {
                        (n as f32 * frequency * std::f32::consts::TAU / rate as f32).sin() * 0.5
                    })
                    .collect();
                let mut reader =
                    PcmReader::new(Source(Some(input)), rate, Arc::new(AtomicBool::new(false)))?;
                let mut bytes = Vec::new();
                reader.read_to_end(&mut bytes)?;
                assert_eq!(
                    bytes.len() as u64 / 2,
                    (rate as u64 + 137) * 16000 / rate as u64
                );
                let samples: Vec<f64> = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| i16::from_le_bytes([c[0], c[1]]) as f64 / 32768.)
                    .collect();
                let body = &samples[500..samples.len() - 500];
                let rms = (body.iter().map(|s| s * s).sum::<f64>() / body.len() as f64).sqrt();
                if frequency == 1000. {
                    assert!((0.33..0.37).contains(&rms), "{rate}: {rms}");
                } else {
                    assert!(rms < 0.01, "Aliased input at {rate}: {rms}");
                }
            }
        }
        Ok(())
    }
}
