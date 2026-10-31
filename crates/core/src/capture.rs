//! Capture ownership and bounded streaming conversion to the recorder's 16 kHz mono PCM.
use anyhow::{Context, Result};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    process::Child,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::JoinHandle,
};

const MAX_INFO_PCM_BYTES: u64 = 16_000 * 120 * 2;
const MAX_INFO_INPUT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(PartialEq, Eq)]
struct WavIdentity {
    bytes: u64,
    modified: std::time::SystemTime,
    created: Option<std::time::SystemTime>,
    #[cfg(unix)]
    file: (u64, u64, i64, i64),
    #[cfg(windows)]
    file: (u64, [u8; 16], i64),
}

fn wav_identity(file: &File) -> Result<WavIdentity> {
    let metadata = file.metadata()?;
    anyhow::ensure!(metadata.is_file(), "WAV input must be a regular file");
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    };
    #[cfg(windows)]
    let identity = {
        use std::{mem::size_of, os::windows::io::AsRawHandle};
        use windows::Win32::{
            Foundation::HANDLE,
            Storage::FileSystem::{
                FileBasicInfo, FileIdInfo, GetFileInformationByHandleEx, FILE_BASIC_INFO,
                FILE_ID_INFO,
            },
        };
        let mut id = FILE_ID_INFO::default();
        let mut basic = FILE_BASIC_INFO::default();
        unsafe {
            let handle = HANDLE(file.as_raw_handle());
            GetFileInformationByHandleEx(
                handle,
                FileIdInfo,
                (&mut id as *mut FILE_ID_INFO).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )?;
            GetFileInformationByHandleEx(
                handle,
                FileBasicInfo,
                (&mut basic as *mut FILE_BASIC_INFO).cast(),
                size_of::<FILE_BASIC_INFO>() as u32,
            )?;
        }
        (
            id.VolumeSerialNumber,
            id.FileId.Identifier,
            basic.ChangeTime,
        )
    };
    Ok(WavIdentity {
        bytes: metadata.len(),
        modified: metadata.modified()?,
        created: metadata.created().ok(),
        #[cfg(any(unix, windows))]
        file: identity,
    })
}

fn wav_input_digest(file: &mut File, identity: &WavIdentity) -> Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let limit = identity
        .bytes
        .checked_add(1)
        .context("WAV input is too large")?;
    let mut reader = (&mut *file).take(limit);
    let mut digest = Sha256::new();
    let mut count = 0;
    let mut buffer = [0; 16_384];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        anyhow::ensure!(count <= identity.bytes, "WAV input changed while hashing");
        digest.update(&buffer[..n]);
    }
    anyhow::ensure!(
        count == identity.bytes && wav_identity(file)? == *identity,
        "WAV input changed while hashing"
    );
    Ok(format!("{:x}", digest.finalize()))
}

fn info_pcm_digest(reader: impl Read, maximum: u64) -> Result<(u64, String)> {
    let mut reader = reader.take(maximum + 1);
    let mut digest = Sha256::new();
    let mut count = 0;
    let mut buffer = [0; 16_384];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        anyhow::ensure!(count <= maximum, "Decoded WAV exceeds 120 seconds");
        digest.update(&buffer[..n]);
    }
    anyhow::ensure!(
        count > 0 && count.is_multiple_of(2),
        "Decoded WAV must contain complete PCM samples"
    );
    Ok((count, format!("{:x}", digest.finalize())))
}

/// Read-only canonical PCM identity using the exact native WAV import decoder.
/// No Engine, models, subprocesses or PCM output files are created.
pub fn wav_pcm_info(path: &Path) -> Result<serde_json::Value> {
    anyhow::ensure!(
        path.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("wav")),
        "Expected a WAV input path"
    );
    let mut input = File::open(path).context("Cannot open diagnostic WAV input")?;
    let identity = wav_identity(&input)?;
    anyhow::ensure!(
        identity.bytes <= MAX_INFO_INPUT_BYTES,
        "Diagnostic WAV input exceeds 64 MiB"
    );
    let input_sha256 = wav_input_digest(&mut input, &identity)?;
    anyhow::ensure!(
        wav_identity(&File::open(path)?)? == identity,
        "WAV input path changed"
    );
    let mut capture = wav(path)?;
    let reader = capture
        .stdout
        .take()
        .context("Cannot read native WAV decoder")?;
    let (pcm_bytes, pcm_sha256) = info_pcm_digest(reader, MAX_INFO_PCM_BYTES)?;
    anyhow::ensure!(capture.wait()?, "Native WAV decoder failed");
    anyhow::ensure!(
        wav_input_digest(&mut input, &identity)? == input_sha256
            && wav_identity(&File::open(path)?)? == identity,
        "WAV input changed while decoding"
    );
    Ok(
        serde_json::json!({"schema":1,"decoder":"native_wav_import", "input_sha256":input_sha256,
        "pcm_sha256":pcm_sha256,"pcm_bytes":pcm_bytes,"samples":pcm_bytes/2,"sample_rate":16000}),
    )
}

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

/// Pace legacy FFmpeg's already-decoded 16 kHz mono s16le output. The clock
/// starts with the first nonempty read, so decoder startup cannot buy a burst.
/// At most 20 ms is delivered per read; no sample conversion or buffering occurs.
pub(crate) struct PacedReader<R> {
    reader: R,
    stop: Arc<AtomicBool>,
    started: Option<std::time::Instant>,
    delivered: u64,
}
impl<R> PacedReader<R> {
    pub(crate) fn new(reader: R, stop: Arc<AtomicBool>) -> Self {
        Self {
            reader,
            stop,
            started: None,
            delivered: 0,
        }
    }
}
impl<R: Read> Read for PacedReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let limit = out.len().min(640);
        let n = self.reader.read(&mut out[..limit])?;
        if n == 0 {
            return Ok(0);
        }
        let started = *self.started.get_or_insert_with(std::time::Instant::now);
        let due = std::time::Duration::from_secs(self.delivered / 32_000)
            + std::time::Duration::from_nanos((self.delivered % 32_000) * 1_000_000_000 / 32_000);
        while !self.stop.load(Ordering::SeqCst) {
            let Some(remaining) = due.checked_sub(started.elapsed()) else {
                break;
            };
            if remaining.is_zero() {
                break;
            }
            std::thread::sleep(remaining.min(std::time::Duration::from_millis(5)));
        }
        // Stop closes the producer separately. Drain everything it already
        // emitted without further pacing delays, rather than discard its tail.
        self.delivered = self.delivered.saturating_add(n as u64);
        Ok(n)
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

    fn write_wav(path: &Path, rate: u32, channels: u16, frames: u32) -> Result<()> {
        let mut writer = hound::WavWriter::create(
            path,
            hound::WavSpec {
                channels,
                sample_rate: rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )?;
        for i in 0..u64::from(frames) * u64::from(channels) {
            writer.write_sample(((i * 97) % 16_000) as i16 - 8_000)?;
        }
        writer.finalize()?;
        Ok(())
    }

    #[test]
    fn wav_pcm_info_matches_the_existing_decoder_without_changing_input() -> Result<()> {
        let dir = tempfile::tempdir()?;
        for (rate, channels) in [(16_000, 1), (48_000, 2)] {
            let path = dir.path().join(format!("{rate}-{channels}.wav"));
            write_wav(&path, rate, channels, 2049)?;
            let before = std::fs::read(&path)?;
            let mut capture = wav(&path)?;
            let mut pcm = Vec::new();
            capture.stdout.take().unwrap().read_to_end(&mut pcm)?;
            assert!(capture.wait()?);
            let info = wav_pcm_info(&path)?;
            assert_eq!(info["schema"], 1);
            assert_eq!(info["decoder"], "native_wav_import");
            assert_eq!(
                info["input_sha256"],
                format!("{:x}", Sha256::digest(&before))
            );
            assert_eq!(info["pcm_sha256"], format!("{:x}", Sha256::digest(&pcm)));
            assert_eq!(info["pcm_bytes"], pcm.len());
            assert_eq!(info["samples"], 2049_u64 * 16_000 / u64::from(rate));
            assert_eq!(info["sample_rate"], 16_000);
            assert_eq!(std::fs::read(&path)?, before);
        }
        Ok(())
    }

    #[test]
    fn pcm_info_read_is_bounded_and_requires_whole_nonempty_samples() -> Result<()> {
        let bytes = [1, 2, 3, 4, 5, 6];
        assert_eq!(
            info_pcm_digest(io::Cursor::new(bytes), 6)?,
            (6, format!("{:x}", Sha256::digest(bytes)))
        );
        assert!(info_pcm_digest(io::empty(), 6).is_err());
        assert!(info_pcm_digest(io::Cursor::new([1, 2, 3]), 6).is_err());
        let mut source = io::Cursor::new([0; 100]);
        assert!(info_pcm_digest(&mut source, 6).is_err());
        assert_eq!(
            source.position(),
            7,
            "Only one byte beyond the limit is consumed"
        );
        Ok(())
    }

    #[test]
    fn wav_pcm_info_rejects_non_wav_empty_truncated_and_over_limit_audio() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let malformed = dir.path().join("not-a-wave.wav");
        std::fs::write(&malformed, b"not a WAV")?;
        assert!(wav_pcm_info(&malformed).is_err());
        let oversized = dir.path().join("oversized.wav");
        File::create(&oversized)?.set_len(MAX_INFO_INPUT_BYTES + 1)?;
        assert!(wav_pcm_info(&oversized)
            .unwrap_err()
            .to_string()
            .contains("exceeds 64 MiB"));
        let empty = dir.path().join("empty.wav");
        write_wav(&empty, 16_000, 1, 0)?;
        assert!(wav_pcm_info(&empty).is_err());
        let truncated = dir.path().join("truncated.wav");
        write_wav(&truncated, 16_000, 1, 1024)?;
        let file = std::fs::OpenOptions::new().write(true).open(&truncated)?;
        file.set_len(file.metadata()?.len() - 1)?;
        assert!(wav_pcm_info(&truncated).is_err());
        let over = dir.path().join("over.wav");
        write_wav(&over, 16_000, 1, (MAX_INFO_PCM_BYTES / 2 + 1) as u32)?;
        assert!(wav_pcm_info(&over)
            .unwrap_err()
            .to_string()
            .contains("exceeds 120 seconds"));
        Ok(())
    }

    #[test]
    fn input_hashing_rejects_changes_to_the_bound_file() -> Result<()> {
        use std::io::Write;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("source.wav");
        write_wav(&path, 16_000, 1, 1024)?;
        let mut file = File::open(&path)?;
        let identity = wav_identity(&file)?;
        wav_input_digest(&mut file, &identity)?;
        let mut edit = std::fs::OpenOptions::new().append(true).open(&path)?;
        edit.write_all(b"changed")?;
        assert!(wav_input_digest(&mut file, &identity).is_err());
        Ok(())
    }

    #[test]
    fn legacy_pacer_starts_on_delivery_and_drains_exact_bytes_after_stop() -> Result<()> {
        let bytes: Vec<_> = (0..1601).map(|i| (i % 251) as u8).collect();
        let stop = Arc::new(AtomicBool::new(false));
        let mut reader = PacedReader::new(io::Cursor::new(bytes.clone()), stop.clone());
        assert_eq!(reader.read(&mut [])?, 0);
        // Time spent constructing/opening the decoder must not buy a burst.
        std::thread::sleep(std::time::Duration::from_millis(40));
        let mut buffer = [0; 1280];
        let first = reader.read(&mut buffer)?;
        assert_eq!(first, 640);
        let mut observed = buffer[..first].to_vec();
        let delivered = std::time::Instant::now();
        let second = reader.read(&mut buffer)?;
        assert!(delivered.elapsed() >= std::time::Duration::from_millis(15));
        observed.extend_from_slice(&buffer[..second]);
        stop.store(true, Ordering::SeqCst);
        reader.read_to_end(&mut observed)?;
        assert_eq!(observed, bytes);
        Ok(())
    }

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
