#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use crate::native_audio::devices;
use crate::types::*;
use anyhow::{bail, Context, Result};
#[cfg(target_os = "linux")]
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub use crate::platform::child_command;

fn ffmpeg_input(input: &Path, replay_seconds: Option<u64>) -> Command {
    let mut command = child_command("ffmpeg");
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        // A local container or disguised playlist can reference a remote stream.
        // Decode only local files and pipes, including nested protocol requests.
        "-protocol_whitelist",
        "file,pipe",
    ]);
    if replay_seconds.is_some() {
        // FFmpeg's default initial read burst leads the live sample clock by
        // about half a second. Keep file replay close to wall time so ASR
        // admission is measured against paced input, like a capture device.
        command.args([
            "-re",
            "-readrate_initial_burst",
            "0.001",
            "-stream_loop",
            "-1",
        ]);
    }
    command.arg("-i").arg(input);
    if let Some(seconds) = replay_seconds {
        command.arg("-t").arg(seconds.to_string());
    }
    command.args(["-vn", "-ac", "1", "-ar", "16000", "-f", "s16le", "pipe:1"]);
    command
}

#[cfg(target_os = "linux")]
pub fn devices() -> Result<Vec<Device>> {
    let output = Command::new("pw-dump")
        .output()
        .context("无法读取 PipeWire 设备")?;
    if !output.status.success() {
        bail!("PipeWire 未运行");
    }
    let data: Value = serde_json::from_slice(&output.stdout)?;
    let mut result = vec![Device {
        id: "auto".into(),
        name: "系统默认麦克风".into(),
    }];
    for node in data.as_array().context("设备列表格式错误")? {
        let p = &node["info"]["props"];
        if p["media.class"].as_str() == Some("Audio/Source") {
            if let Some(name) = p["node.name"].as_str() {
                result.push(Device {
                    id: name.into(),
                    name: p["node.description"].as_str().unwrap_or(name).into(),
                });
            }
        }
    }
    Ok(result)
}

pub fn spawn_capture(
    device: &str,
    input: Option<&Path>,
    replay_seconds: Option<u64>,
    log: &Path,
) -> Result<crate::capture::Capture> {
    if replay_seconds.is_none() {
        if let Some(path) =
            input.filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav")))
        {
            return crate::capture::wav(path);
        }
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if input.is_none() {
        return crate::native_audio::capture(device);
    }
    let mut cmd = if let Some(input) = input {
        ffmpeg_input(input, replay_seconds)
    } else {
        let mut c = child_command("pw-record");
        c.args([
            "--raw",
            "--rate",
            "16000",
            "--channels",
            "1",
            "--format",
            "s16",
            "--latency",
            "100ms",
        ]);
        if device != "auto" {
            c.arg("--target").arg(device);
        }
        c.arg("-");
        c
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(File::create(log)?);
    crate::capture::Capture::process(
        cmd.spawn()
            .context("Cannot start audio input; compressed-file imports require ffmpeg")?,
    )
}

pub fn capture_stream(
    mut reader: impl Read,
    dir: &Path,
    mut progress: impl FnMut(u64, f32),
) -> Result<u64> {
    capture_stream_from(&mut reader, dir, 0, &mut progress)
}

pub fn capture_stream_from(
    mut reader: impl Read,
    dir: &Path,
    start_samples: u64,
    mut progress: impl FnMut(u64, f32),
) -> Result<u64> {
    fs::create_dir_all(dir)?;
    if samples_on_disk(dir)? != start_samples {
        bail!("录音续写位置与磁盘数据不一致");
    }
    let mut total = start_samples;
    let chunk_path = dir.join(format!("{:06}.pcm", total / CHUNK_SAMPLES));
    let mut chunk = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(chunk_path)?;
    let mut buffer = [0u8; 6400];
    let mut last_sync = Instant::now();
    let mut carry = None;
    loop {
        let prefix = if let Some(b) = carry.take() {
            buffer[0] = b;
            1
        } else {
            0
        };
        let n = reader.read(&mut buffer[prefix..])?;
        if n == 0 {
            break;
        }
        let len = n + prefix;
        let even = len / 2 * 2;
        if len % 2 == 1 {
            carry = Some(buffer[len - 1]);
        }
        let mut offset = 0;
        let mut power = 0f64;
        while offset < even {
            if total > 0 && total.is_multiple_of(CHUNK_SAMPLES) {
                chunk.sync_data()?;
                chunk = File::create(dir.join(format!("{:06}.pcm", total / CHUNK_SAMPLES)))?;
            }
            let remaining = ((CHUNK_SAMPLES - total % CHUNK_SAMPLES) * 2) as usize;
            let count = remaining.min(even - offset);
            chunk.write_all(&buffer[offset..offset + count])?;
            for sample in buffer[offset..offset + count].as_chunks::<2>().0.iter() {
                let v = i16::from_le_bytes([sample[0], sample[1]]) as f64 / 32768.;
                power += v * v;
            }
            total += count as u64 / 2;
            offset += count;
        }
        if last_sync.elapsed() > Duration::from_secs(1) {
            chunk.sync_data()?;
            last_sync = Instant::now();
        }
        let rms = if even > 0 {
            (power / (even / 2) as f64).sqrt() as f32
        } else {
            0.
        };
        progress(total, rms);
    }
    chunk.sync_data()?;
    if carry.is_some() {
        bail!("音频流尾部含不完整采样，已保留此前音频");
    }
    Ok(total)
}

pub fn samples_on_disk(dir: &Path) -> Result<u64> {
    if !dir.exists() {
        return Ok(0);
    }
    let mut total = 0;
    for entry in fs::read_dir(dir)? {
        let p = entry?.path();
        if p.extension().is_some_and(|s| s == "pcm") {
            total += fs::metadata(p)?.len() / 2;
        }
    }
    Ok(total)
}

pub fn pcm_range(dir: &Path, start_ms: u64, end_ms: u64) -> Result<Vec<i16>> {
    if end_ms <= start_ms || end_ms - start_ms > 90_000 {
        bail!("回听片段长度须在 0 至 90 秒之间");
    }
    let mut start = start_ms * SAMPLE_RATE / 1000;
    let end = (end_ms * SAMPLE_RATE / 1000).min(samples_on_disk(dir)?);
    let mut result = Vec::with_capacity(end.saturating_sub(start) as usize);
    while start < end {
        let chunk = start / CHUNK_SAMPLES;
        let offset = start % CHUNK_SAMPLES;
        let count = (CHUNK_SAMPLES - offset).min(end - start);
        let mut file = File::open(dir.join(format!("{chunk:06}.pcm")))?;
        file.seek(SeekFrom::Start(offset * 2))?;
        let mut bytes = vec![0; count as usize * 2];
        file.read_exact(&mut bytes)?;
        result.extend(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| i16::from_le_bytes([p[0], p[1]])),
        );
        start += count;
    }
    Ok(result)
}

pub fn wav_bytes(samples: &[i16]) -> Result<Vec<u8>> {
    let mut output = std::io::Cursor::new(Vec::new());
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    {
        let mut writer = hound::WavWriter::new(&mut output, spec)?;
        for s in samples {
            writer.write_sample(*s)?;
        }
        writer.finalize()?;
    }
    Ok(output.into_inner())
}

pub fn export_wav(dir: &Path, target: &Path) -> Result<()> {
    export_wav_range(dir, target, 0, samples_on_disk(dir)?)
}

/// Sample-exact, bounded-memory export of one take from the continuous meeting timeline.
pub fn export_wav_range(dir: &Path, target: &Path, start: u64, end: u64) -> Result<()> {
    if end < start || end > samples_on_disk(dir)? {
        bail!("录音片段范围无效");
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let tmp = target.with_extension("wav.part");
    let mut writer = hound::WavWriter::create(&tmp, spec)?;
    let mut position = start;
    while position < end {
        let chunk = position / CHUNK_SAMPLES;
        let offset = position % CHUNK_SAMPLES;
        let count = (CHUNK_SAMPLES - offset).min(end - position);
        let mut file = File::open(dir.join(format!("{chunk:06}.pcm")))?;
        file.seek(SeekFrom::Start(offset * 2))?;
        let mut data = vec![0; count as usize * 2];
        file.read_exact(&mut data)?;
        for p in data.as_chunks::<2>().0.iter() {
            writer.write_sample(i16::from_le_bytes([p[0], p[1]]))?;
        }
        position += count;
    }
    writer.finalize()?;
    fs::rename(tmp, target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_ffmpeg() -> bool {
        Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    #[test]
    fn paced_replay_preserves_pcm_without_an_initial_read_burst() -> Result<()> {
        if !has_ffmpeg() {
            eprintln!("FFmpeg is not installed; paced replay integration check skipped");
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        let input = root.path().join("one-second.wav");
        let samples: Vec<i16> = (0..16000)
            .map(|n| ((n as f32 * 440. * std::f32::consts::TAU / 16000.).sin() * 8000.) as i16)
            .collect();
        fs::write(&input, wav_bytes(&samples)?)?;
        let expected: Vec<u8> = samples
            .iter()
            .cycle()
            .take(48000)
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let mut capture = spawn_capture(
            "auto",
            Some(&input),
            Some(3),
            &root.path().join("replay.log"),
        )?;
        let mut reader = capture.stdout.take().unwrap();
        let mut pcm = Vec::new();
        let mut buffer = [0u8; 640];
        let mut first_delivery = None;
        let mut max_lead = Duration::ZERO;
        loop {
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            let now = Instant::now();
            let first = *first_delivery.get_or_insert(now);
            pcm.extend_from_slice(&buffer[..n]);
            let supplied = Duration::from_secs_f64(pcm.len() as f64 / 32000.);
            max_lead = max_lead.max(supplied.saturating_sub(now.duration_since(first)));
        }
        assert!(capture.wait()?);
        assert_eq!(pcm, expected);
        // Covers one 200ms persistence block plus scheduling/IO jitter, while
        // rejecting the roughly 500ms initial burst of the default -re path.
        assert!(max_lead <= Duration::from_millis(250), "{max_lead:?}");
        Ok(())
    }

    #[test]
    fn ffmpeg_decodes_local_compressed_audio_with_network_protocols_disabled() -> Result<()> {
        if !has_ffmpeg() {
            eprintln!("FFmpeg is not installed; compressed audio integration check skipped");
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        let wav = root.path().join("input.wav");
        let samples: Vec<i16> = (0..8000)
            .map(|n| ((n as f32 * 440. * std::f32::consts::TAU / 16000.).sin() * 8000.) as i16)
            .collect();
        fs::write(&wav, wav_bytes(&samples)?)?;
        for extension in ["mp3", "m4a"] {
            let compressed = root.path().join(format!("input.{extension}"));
            let encoded = Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"])
                .arg(&wav)
                .arg(&compressed)
                .output()?;
            anyhow::ensure!(
                encoded.status.success(),
                "fixture encoding failed: {}",
                String::from_utf8_lossy(&encoded.stderr)
            );
            let mut capture = spawn_capture(
                "auto",
                Some(&compressed),
                None,
                &root.path().join("decode.log"),
            )?;
            let mut pcm = Vec::new();
            capture.stdout.take().unwrap().read_to_end(&mut pcm)?;
            assert!(capture.wait()?);
            // Lossy formats add encoder padding; this verifies useful decoded PCM,
            // its duration and sample amplitude instead of mirroring command flags.
            assert!(
                (15_000..=20_000).contains(&pcm.len()),
                "unexpected {extension} duration: {} bytes",
                pcm.len()
            );
            assert!(pcm
                .as_chunks::<2>()
                .0
                .iter()
                .any(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs() > 1000));
        }
        Ok(())
    }

    #[test]
    fn local_playlist_cannot_contact_a_network_endpoint() -> Result<()> {
        if !has_ffmpeg() {
            eprintln!("FFmpeg is not installed; network protocol integration check skipped");
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let endpoint = listener.local_addr()?;
        let path = root.path().join("local-playlist.m3u8");
        fs::write(&path, format!("#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:1.0,\nhttp://{endpoint}/audio.ts\n#EXT-X-ENDLIST\n"))?;
        let stderr = root.path().join("ffmpeg.log");
        let mut child = ffmpeg_input(&path, None)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create(&stderr)?)
            .spawn()?;
        let start = Instant::now();
        let mut connections = 0;
        let status = loop {
            if let Ok((mut stream, _)) = listener.accept() {
                connections += 1;
                stream.set_write_timeout(Some(Duration::from_millis(100)))?;
                let _ = stream.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if start.elapsed() > Duration::from_secs(5) {
                let _ = child.kill();
                let _ = child.wait();
                bail!("FFmpeg did not reject the network playlist in time");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        // Check queued accepts too: process exit is not evidence of no connection.
        while listener.accept().is_ok() {
            connections += 1;
        }
        assert_eq!(
            connections, 0,
            "local audio decoding attempted a network connection"
        );
        assert!(!status.success());
        let message = fs::read_to_string(stderr)?;
        assert!(message.contains("not on whitelist"), "{message}");
        Ok(())
    }

    #[test]
    fn pause_resume_appends_without_overwriting_audio() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let first = vec![4u8; 30_010];
        let second = vec![9u8; 32_000];
        let count = capture_stream(std::io::Cursor::new(&first), tmp.path(), |_, _| {})?;
        assert_eq!(
            capture_stream_from(std::io::Cursor::new(&second), tmp.path(), count, |_, _| {})?,
            31_005
        );
        let bytes = fs::read(tmp.path().join("000000.pcm"))?;
        assert_eq!(&bytes[..first.len()], first.as_slice());
        assert_eq!(&bytes[first.len()..], second.as_slice());
        assert!(
            capture_stream_from(std::io::Cursor::new(&second), tmp.path(), 0, |_, _| {}).is_err()
        );
        Ok(())
    }
    #[test]
    fn chunk_rotation_and_clip_crossing_boundary_preserve_every_sample() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let samples: Vec<i16> = (0..CHUNK_SAMPLES + 32)
            .map(|i| (i % 30000) as i16)
            .collect();
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        assert_eq!(
            capture_stream(std::io::Cursor::new(bytes), tmp.path(), |_, _| {})?,
            samples.len() as u64
        );
        assert_eq!(
            fs::metadata(tmp.path().join("000000.pcm"))?.len(),
            CHUNK_SAMPLES * 2
        );
        assert_eq!(fs::metadata(tmp.path().join("000001.pcm"))?.len(), 64);
        let clip = pcm_range(tmp.path(), 29_999, 30_002)?;
        assert_eq!(clip, samples[(CHUNK_SAMPLES - 16) as usize..].to_vec());
        let wave = wav_bytes(&clip)?;
        let reader = hound::WavReader::new(std::io::Cursor::new(wave))?;
        assert_eq!(reader.duration(), 48);
        let output = tmp.path().join("take.wav");
        export_wav_range(tmp.path(), &output, CHUNK_SAMPLES - 17, CHUNK_SAMPLES + 13)?;
        let mut reader = hound::WavReader::open(output)?;
        let actual = reader
            .samples::<i16>()
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert_eq!(
            actual,
            samples[(CHUNK_SAMPLES - 17) as usize..(CHUNK_SAMPLES + 13) as usize]
        );
        Ok(())
    }
}
