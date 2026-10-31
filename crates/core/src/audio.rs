#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use crate::native_audio::devices;
use crate::types::*;
use anyhow::{bail, Context, Result};
#[cfg(target_os = "linux")]
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

pub use crate::platform::child_command;

#[derive(Clone, PartialEq, Eq)]
struct FfmpegIdentity {
    path: PathBuf,
    bytes: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    file: (u64, u64, i64, i64),
    #[cfg(windows)]
    file: (u64, [u8; 16], i64),
}
fn ffmpeg_identity(path: &Path) -> Result<FfmpegIdentity> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(metadata.is_file(), "FFmpeg executable is not a file");
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
    Ok(FfmpegIdentity {
        path: path.to_owned(),
        bytes: metadata.len(),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
        #[cfg(any(unix, windows))]
        file: identity,
    })
}

#[derive(Default)]
struct FfmpegFeatures(Mutex<Vec<(FfmpegIdentity, bool)>>);
impl FfmpegFeatures {
    fn initial_burst(&self, executable: &Path, probe: impl FnOnce() -> Result<bool>) -> bool {
        let before = ffmpeg_identity(executable).ok();
        if let Some(ref identity) = before {
            if let Some((_, supported)) = self
                .0
                .lock()
                .unwrap()
                .iter()
                .find(|(old, _)| old == identity)
            {
                return *supported;
            }
        }
        // No cache lock is held while starting or waiting for the probe.
        let Ok(supported) = probe() else {
            return false;
        };
        let after = ffmpeg_identity(executable).ok();
        if before.is_some() && before != after {
            return false;
        }
        if let Some(identity) = before {
            let mut cache = self.0.lock().unwrap();
            cache.retain(|(old, _)| old.path != identity.path);
            if cache.len() == 8 {
                cache.remove(0);
            }
            cache.push((identity, supported));
        }
        supported
    }
}

struct FfmpegProbe(Child);
impl Drop for FfmpegProbe {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
fn probe_initial_burst(executable: &Path, timeout: Duration) -> Result<bool> {
    // Parsing an unknown input option fails before -version. No input, output,
    // model, network protocol or collected process output is involved.
    let child = child_command(executable)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-readrate_initial_burst",
            "0.001",
            "-version",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    wait_ffmpeg_probe(child, timeout)
}
fn wait_ffmpeg_probe(child: Child, timeout: Duration) -> Result<bool> {
    let mut child = FfmpegProbe(child);
    let started = Instant::now();
    loop {
        if let Some(status) = child.0.try_wait()? {
            return Ok(status.success());
        }
        anyhow::ensure!(
            started.elapsed() < timeout,
            "FFmpeg feature probe timed out"
        );
        std::thread::sleep(Duration::from_millis(5).min(timeout.saturating_sub(started.elapsed())));
    }
}
fn replay_ffmpeg() -> Result<(PathBuf, bool)> {
    static FEATURES: OnceLock<FfmpegFeatures> = OnceLock::new();
    let mut directories = Vec::new();
    #[cfg(windows)]
    {
        if let Some(parent) = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_owned))
        {
            directories.push(parent);
        }
        if let Ok(current) = std::env::current_dir() {
            directories.push(current);
        }
        if let Some(system) = std::env::var_os("SystemRoot") {
            directories.push(PathBuf::from(&system).join("System32"));
            directories.push(PathBuf::from(system));
        }
    }
    directories.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    for directory in directories {
        let candidate = directory.join(crate::platform::executable("ffmpeg"));
        let Ok(metadata) = candidate.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                continue;
            }
        }
        let executable = candidate.canonicalize()?;
        let supported = FEATURES
            .get_or_init(FfmpegFeatures::default)
            .initial_burst(&executable, || {
                probe_initial_burst(&executable, Duration::from_secs(1))
            });
        return Ok((executable, supported));
    }
    bail!("Cannot find FFmpeg for paced replay")
}

fn ffmpeg_input(input: &Path, replay_seconds: Option<u64>) -> Command {
    ffmpeg_input_with("ffmpeg", input, replay_seconds, true)
}
fn ffmpeg_input_with(
    executable: impl AsRef<std::ffi::OsStr>,
    input: &Path,
    replay_seconds: Option<u64>,
    initial_burst: bool,
) -> Command {
    let mut command = child_command(executable);
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
        command.arg("-re");
        if initial_burst {
            command.args(["-readrate_initial_burst", "0.001"]);
        }
        command.args(["-stream_loop", "-1"]);
    }
    command.arg("-i").arg(input);
    if let Some(seconds) = replay_seconds {
        command.arg("-t").arg(seconds.to_string());
    }
    command.args(["-vn", "-ac", "1", "-ar", "16000", "-f", "s16le", "pipe:1"]);
    command
}

fn replay_capture(
    input: &Path,
    seconds: u64,
    log: &Path,
    executable: &Path,
    initial_burst: bool,
) -> Result<crate::capture::Capture> {
    let child = ffmpeg_input_with(executable, input, Some(seconds), initial_burst)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(File::create(log)?)
        .spawn()
        .context("Cannot start FFmpeg paced replay")?;
    let mut capture = crate::capture::Capture::process(child)?;
    if !initial_burst {
        let reader = capture
            .stdout
            .take()
            .context("Cannot read FFmpeg replay stream")?;
        capture.stdout = Some(Box::new(crate::capture::PacedReader::new(
            reader,
            capture.stop.clone(),
        )));
    }
    Ok(capture)
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
    let mut result = vec![Device::microphone("auto", "系统默认麦克风")];
    let nodes = data.as_array().context("设备列表格式错误")?;
    for node in nodes {
        let p = &node["info"]["props"];
        if p["media.class"].as_str() == Some("Audio/Source") {
            if let Some(name) = p["node.name"].as_str() {
                result.push(Device::microphone(
                    name,
                    p["node.description"].as_str().unwrap_or(name),
                ));
            }
        }
    }
    // Meeting software often plays the other side through the speakers, so a
    // mic-only recording loses half the conversation. Offer the combined
    // capture first, then the playback-only monitor of each hardware sink.
    // Virtual filter sinks (speaker EQ, spatial audio) are duplicates of the
    // same hardware and are skipped.
    if let Some(sink) = default_sink() {
        let description = nodes
            .iter()
            .filter_map(|node| node["info"]["props"].as_object())
            .find(|p| {
                p.get("media.class").and_then(Value::as_str) == Some("Audio/Sink")
                    && p.get("node.name").and_then(Value::as_str) == Some(sink.as_str())
            })
            .and_then(|p| p.get("node.description").and_then(Value::as_str))
            .unwrap_or("系统声音");
        result.push(Device::mixed("mix", format!("麦克风 + {description}")));
    }
    for node in nodes {
        let p = &node["info"]["props"];
        if p["media.class"].as_str() != Some("Audio/Sink")
            || p["node.virtual"].as_bool() == Some(true)
            || p["device.class"].as_str() != Some("sound")
        {
            continue;
        }
        let Some(name) = p["node.name"].as_str() else {
            continue;
        };
        result.push(Device::system(
            format!("{name}.monitor"),
            p["node.description"].as_str().unwrap_or(name),
        ));
    }
    Ok(result)
}

/// Node name of the server-side default playback device.
#[cfg(target_os = "linux")]
fn default_sink() -> Option<String> {
    let from_pactl = Command::new("pactl")
        .args(["get-default-sink"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    if from_pactl.is_some() {
        return from_pactl;
    }
    // Without libpulse tools, fall back to the first hardware sink in pw-dump.
    let output = Command::new("pw-dump").output().ok()?;
    let data: Value = serde_json::from_slice(&output.stdout).ok()?;
    data.as_array()?
        .iter()
        .filter_map(|node| node["info"]["props"].as_object())
        .find(|p| {
            p.get("media.class").and_then(Value::as_str) == Some("Audio/Sink")
                && p.get("node.virtual").and_then(Value::as_bool) != Some(true)
        })
        .and_then(|p| p.get("node.name").and_then(Value::as_str))
        .map(str::to_owned)
}

/// Whether a PipeWire node name is currently published by the server.
#[cfg(target_os = "linux")]
fn node_exists(name: &str) -> bool {
    let Ok(output) = Command::new("pw-dump").output() else {
        return false;
    };
    let Ok(data) = serde_json::from_slice::<Value>(&output.stdout) else {
        return false;
    };
    data.as_array().is_some_and(|nodes| {
        nodes
            .iter()
            .any(|node| node["info"]["props"]["node.name"].as_str() == Some(name))
    })
}

/// Input devices this ffmpeg build can open. `pipewire` is the modern alias and
/// counts as PulseAudio support because it speaks the same protocol.
#[cfg(target_os = "linux")]
fn ffmpeg_input_devices() -> String {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-devices"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase())
        .unwrap_or_default()
}

pub fn spawn_capture(
    device: &str,
    input: Option<&Path>,
    replay_seconds: Option<u64>,
    log: &Path,
) -> Result<crate::capture::Capture> {
    if let (Some(path), Some(seconds)) = (input, replay_seconds) {
        let (executable, initial_burst) = replay_ffmpeg()?;
        return replay_capture(path, seconds, log, &executable, initial_burst);
    }
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
    } else if device == "mix" {
        // A remote meeting plays the other participants through the speakers
        // while the local microphone picks up the room. Recording either one
        // alone loses half the conversation, so mix both with a fixed gain and
        // no normalization: a silent side must not amplify its own noise floor.
        #[cfg(target_os = "linux")]
        {
            // The monitor source is created on demand and does not appear in
            // `pw-dump`, so the precondition to check is its owning sink.
            let sink = default_sink().context("找不到系统播放设备；请确认 PipeWire 正在运行")?;
            // Fail here with an actionable message instead of a take that
            // silently records nothing when the sink vanished or ffmpeg was
            // built without the PulseAudio input device.
            anyhow::ensure!(
                node_exists(&sink),
                "默认播放设备 {sink} 不存在；请确认 PipeWire 与音频服务正常"
            );
            anyhow::ensure!(
                ffmpeg_input_devices().contains("pulse"),
                "ffmpeg 未编译 PulseAudio 输入支持，无法混录系统声音"
            );
            let monitor = format!("{sink}.monitor");
            let mut c = child_command("ffmpeg");
            c.args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                // Local capture only: no playlist or container may open a URL.
                "-protocol_whitelist",
                "file,pipe",
                "-f",
                "pulse",
                "-i",
                "default",
                "-f",
                "pulse",
                "-i",
                &monitor,
                "-filter_complex",
                "[0:a]volume=1.0[mic];[1:a]volume=1.0[sys];[mic][sys]amix=inputs=2:duration=longest:dropout_transition=0:normalize=0",
                "-ac",
                "1",
                "-ar",
                "16000",
                "-f",
                "s16le",
                "pipe:1",
            ]);
            c
        }
        #[cfg(not(target_os = "linux"))]
        {
            bail!("麦克风与系统声音混录目前仅在 Linux（PipeWire）上可用")
        }
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

    /// A meeting capture must not silently drop the other side of the call.
    /// The device list is built from a real `pw-dump` fixture kept in the
    /// repository so this stays checkable on a machine without audio devices.
    #[test]
    fn linux_devices_offer_a_mixed_capture_before_monitors() {
        use serde_json::Value;

        let fixture = include_str!("../../../tests/fixtures/pw-dump-minimal.json");
        let data: Value = serde_json::from_str(fixture).unwrap();
        let nodes = data.as_array().unwrap();
        let sinks: Vec<&Value> = nodes
            .iter()
            .filter(|node| node["info"]["props"]["media.class"].as_str() == Some("Audio/Sink"))
            .collect();
        assert_eq!(
            sinks.len(),
            2,
            "fixture keeps a hardware sink and a virtual one"
        );
        let hardware: Vec<&Value> = sinks
            .iter()
            .copied()
            .filter(|node| node["info"]["props"]["node.virtual"].as_bool() != Some(true))
            .collect();
        assert_eq!(
            hardware.len(),
            1,
            "virtual filter sinks must not be offered"
        );
        let name = hardware[0]["info"]["props"]["node.name"].as_str().unwrap();
        assert_eq!(name, "alsa_output.test.analog-stereo");
        // The monitor id the capture path targets is the sink name plus `.monitor`.
        assert_eq!(
            format!("{name}.monitor"),
            "alsa_output.test.analog-stereo.monitor"
        );
    }

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
        assert_paced_replay(false)
    }

    #[test]
    fn forced_legacy_replay_preserves_pcm_and_the_same_initial_burst_limit() -> Result<()> {
        assert_paced_replay(true)
    }

    fn assert_paced_replay(force_legacy: bool) -> Result<()> {
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
        let log = root.path().join("replay.log");
        let mut capture = if force_legacy {
            let (executable, _) = replay_ffmpeg()?;
            replay_capture(&input, 3, &log, &executable, false)?
        } else {
            spawn_capture("auto", Some(&input), Some(3), &log)?
        };
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
        assert!(
            capture.wait()?,
            "FFmpeg replay failed: {}",
            fs::read_to_string(&log).unwrap_or_else(|_| "capture log unavailable".into())
        );
        assert_eq!(pcm, expected);
        // Covers one 200ms persistence block plus scheduling/IO jitter, while
        // rejecting the roughly 500ms initial burst of the default -re path.
        assert!(max_lead <= Duration::from_millis(250), "{max_lead:?}");
        Ok(())
    }

    #[test]
    fn replay_feature_cache_tracks_executable_replacement_and_retries_probe_failure() -> Result<()>
    {
        let root = tempfile::tempdir()?;
        let executable = root.path().join("ffmpeg-fixture");
        fs::write(&executable, b"original")?;
        let cache = FfmpegFeatures::default();
        let mut calls = 0;
        assert!(!cache.initial_burst(&executable, || {
            calls += 1;
            bail!("fixture probe failure")
        }));
        assert!(cache.0.lock().unwrap().is_empty());
        assert!(cache.initial_burst(&executable, || {
            calls += 1;
            Ok(true)
        }));
        assert!(cache.initial_burst(&executable, || panic!("unchanged executable was reprobed")));
        assert_eq!(calls, 2);

        let modified = executable.metadata()?.modified()?;
        let replacement = root.path().join("replacement");
        fs::write(&replacement, b"replaced")?;
        File::options()
            .write(true)
            .open(&replacement)?
            .set_times(fs::FileTimes::new().set_modified(modified))?;
        fs::remove_file(&executable)?;
        fs::rename(replacement, &executable)?;
        assert!(!cache.initial_burst(&executable, || {
            calls += 1;
            Ok(false)
        }));
        assert!(!cache.initial_burst(&executable, || panic!("legacy result was not cached")));
        assert_eq!(calls, 3);
        assert_eq!(cache.0.lock().unwrap().len(), 1);

        assert!(!cache.initial_burst(&root.path().join("different-executable"), || Ok(false)));
        assert!(!cache.initial_burst(&executable, || panic!(
            "another path changed cache identity"
        )));
        Ok(())
    }

    #[test]
    fn replay_feature_probe_does_not_cache_an_executable_changed_during_probe() -> Result<()> {
        let root = tempfile::tempdir()?;
        let executable = root.path().join("ffmpeg-fixture");
        fs::write(&executable, b"first")?;
        let cache = FfmpegFeatures::default();
        assert!(!cache.initial_burst(&executable, || {
            fs::write(&executable, b"changed executable")?;
            Ok(true)
        }));
        assert!(cache.0.lock().unwrap().is_empty());
        assert!(cache.initial_burst(&executable, || Ok(true)));
        Ok(())
    }

    #[test]
    #[ignore = "child fixture invoked only by the bounded feature-probe test"]
    fn replay_probe_child() {
        if std::env::var_os("OMASCRIBE_TEST_FFMPEG_PROBE").is_some() {
            std::thread::sleep(Duration::from_secs(30));
        }
    }

    #[test]
    fn replay_feature_probe_timeout_reaps_its_child_without_collecting_output() -> Result<()> {
        let child = child_command(std::env::current_exe()?)
            .args(["--exact", "audio::tests::replay_probe_child", "--ignored"])
            .env("OMASCRIBE_TEST_FFMPEG_PROBE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let pid = child.id();
        assert!(wait_ffmpeg_probe(child, Duration::from_millis(40)).is_err());
        #[cfg(unix)]
        unsafe {
            let mut status = 0;
            assert_eq!(libc::waitpid(pid as i32, &mut status, libc::WNOHANG), -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
        }
        #[cfg(windows)]
        {
            let mut processes = sysinfo::System::new();
            processes.refresh_processes(
                sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
                true,
            );
            assert!(processes.process(sysinfo::Pid::from_u32(pid)).is_none());
        }
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
