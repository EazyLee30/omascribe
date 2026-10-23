use crate::types::*;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub fn child_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new(program);
    // No inference or recording subprocess should outlive an unexpectedly closed app.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    cmd
}

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
) -> Result<Child> {
    let mut cmd = if let Some(input) = input {
        let mut c = child_command("ffmpeg");
        c.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
        if replay_seconds.is_some() {
            c.args(["-re", "-stream_loop", "-1"]);
        }
        c.arg("-i").arg(input);
        if let Some(seconds) = replay_seconds {
            c.arg("-t").arg(seconds.to_string());
        }
        c.args(["-vn", "-ac", "1", "-ar", "16000", "-f", "s16le", "pipe:1"]);
        c
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
    cmd.spawn()
        .context("无法启动音频采集，请检查 PipeWire / ffmpeg")
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
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let tmp = target.with_extension("wav.part");
    let mut writer = hound::WavWriter::create(&tmp, spec)?;
    let total = samples_on_disk(dir)?;
    for chunk in 0..total.div_ceil(CHUNK_SAMPLES) {
        let data = fs::read(dir.join(format!("{chunk:06}.pcm")))?;
        for p in data.as_chunks::<2>().0.iter() {
            writer.write_sample(i16::from_le_bytes([p[0], p[1]]))?;
        }
    }
    writer.finalize()?;
    fs::rename(tmp, target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        Ok(())
    }
}
