//! Short, memory-only input preview. Its lifetime does not depend on WebView timers.
use crate::capture::Capture;
use anyhow::{Context, Result};
use serde::Serialize;
use std::{
    collections::VecDeque,
    io::Read,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const LIMIT: Duration = Duration::from_secs(30);
const HISTORY: usize = 80;

#[derive(Clone, Default, Serialize)]
pub struct InputLevel {
    pub active: bool,
    pub starting: bool,
    pub device: String,
    pub rms: f32,
    pub peak: f32,
    pub waveform: VecDeque<f32>,
    pub elapsed_ms: u64,
    pub error: Option<String>,
}

struct Session {
    stop: Arc<AtomicBool>,
    worker: JoinHandle<()>,
}

#[derive(Default)]
pub struct MicrophoneMonitor {
    session: Mutex<Option<Session>>,
    level: Arc<Mutex<InputLevel>>,
}

impl MicrophoneMonitor {
    pub fn status(&self) -> InputLevel {
        self.level.lock().unwrap().clone()
    }

    /// Reap the old capture before a new device/real recording opens the input.
    pub fn stop(&self) {
        let mut session = self.session.lock().unwrap();
        stop_session(&mut session);
    }

    pub fn start(&self, device: String, log: &Path) -> Result<InputLevel> {
        let log = log.to_owned();
        self.start_with(device, LIMIT, move |name| {
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            let name = crate::native_audio::input_name(name)?;
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            let name = name.to_owned();
            let capture = crate::audio::spawn_capture(&name, None, None, &log)?;
            Ok((name, capture))
        })
    }

    fn start_with(
        &self,
        device: String,
        limit: Duration,
        open: impl FnOnce(&str) -> Result<(String, Capture)> + Send + 'static,
    ) -> Result<InputLevel> {
        let mut session = self.session.lock().unwrap();
        stop_session(&mut session);
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let level = self.level.clone();
        *level.lock().unwrap() = InputLevel {
            active: true,
            starting: true,
            device: device.clone(),
            ..Default::default()
        };
        let worker = thread::Builder::new()
            .name("microphone-preview".into())
            .spawn(move || {
                let result = (|| -> Result<()> {
                    let (name, mut capture) = open(&device)?;
                    if cancelled.load(Ordering::SeqCst) {
                        capture.kill()?;
                        capture.wait()?;
                        return Ok(());
                    }
                    let mut input = capture
                        .stdout
                        .take()
                        .context("Cannot read microphone preview")?;
                    let capture_stop = capture.stop.clone();
                    let watchdog_stop = cancelled.clone();
                    let began = Instant::now();
                    let watchdog = thread::spawn(move || {
                        while !watchdog_stop.load(Ordering::SeqCst) && began.elapsed() < limit {
                            thread::sleep(Duration::from_millis(20));
                        }
                        capture_stop.store(true, Ordering::SeqCst);
                    });
                    {
                        let mut state = level.lock().unwrap();
                        state.device = name;
                        state.starting = false;
                    }
                    let mut buffer = [0u8; 1024];
                    let mut pending_byte = None;
                    let sampled = (|| -> Result<()> {
                        loop {
                            let read = input.read(&mut buffer)?;
                            if read == 0 {
                                break;
                            }
                            let mut samples = Vec::with_capacity(read / 2 + 1);
                            for &byte in &buffer[..read] {
                                if let Some(first) = pending_byte.take() {
                                    samples.push(i16::from_le_bytes([first, byte]));
                                } else {
                                    pending_byte = Some(byte);
                                }
                            }
                            if samples.is_empty() {
                                continue;
                            }
                            let (rms, peak) = amplitude(&samples);
                            let mut state = level.lock().unwrap();
                            state.rms = rms;
                            state.peak = peak;
                            state.elapsed_ms = began.elapsed().as_millis() as u64;
                            state.waveform.push_back(peak);
                            if state.waveform.len() > HISTORY {
                                state.waveform.pop_front();
                            }
                        }
                        Ok(())
                    })();
                    cancelled.store(true, Ordering::SeqCst);
                    let _ = watchdog.join();
                    let finished = capture.wait();
                    sampled?;
                    finished?;
                    Ok(())
                })();
                let mut state = level.lock().unwrap();
                state.active = false;
                state.starting = false;
                state.error = result.err().map(|e| format!("{e:#}"));
            })?;
        *session = Some(Session { stop, worker });
        Ok(self.status())
    }
}

fn stop_session(slot: &mut Option<Session>) {
    if let Some(session) = slot.take() {
        session.stop.store(true, Ordering::SeqCst);
        let _ = session.worker.join();
    }
}

impl Drop for MicrophoneMonitor {
    fn drop(&mut self) {
        self.stop();
    }
}

fn amplitude(samples: &[i16]) -> (f32, f32) {
    let (sum, peak) = samples
        .iter()
        .fold((0.0f64, 0.0f32), |(sum, peak), &sample| {
            let value = f32::from(sample) / 32768.0;
            (sum + f64::from(value).powi(2), peak.max(value.abs()))
        });
    ((sum / samples.len() as f64).sqrt() as f32, peak)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Tone(Arc<AtomicBool>);
    impl Read for Tone {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if self.0.load(Ordering::SeqCst) {
                return Ok(0);
            }
            thread::sleep(Duration::from_millis(2));
            for pair in out.as_chunks_mut::<2>().0 {
                pair.copy_from_slice(&16384i16.to_le_bytes());
            }
            Ok(out.len() / 2 * 2)
        }
    }
    fn open(name: &str) -> Result<(String, Capture)> {
        let stop = Arc::new(AtomicBool::new(false));
        Ok((name.into(), Capture::reader(Tone(stop.clone()), stop, None)))
    }
    #[test]
    fn preview_stops_without_a_ui_poll_and_bounds_signal_history() {
        let monitor = MicrophoneMonitor::default();
        monitor
            .start_with("Realtek".into(), Duration::from_millis(250), open)
            .unwrap();
        thread::sleep(Duration::from_millis(400));
        let level = monitor.status();
        assert!(!level.active && level.error.is_none());
        assert_eq!(level.device, "Realtek");
        assert!((level.rms - 0.5).abs() < 0.001 && level.peak == 0.5);
        assert!(!level.waveform.is_empty() && level.waveform.len() <= HISTORY);
    }
    #[test]
    fn changing_device_reaps_previous_capture_and_errors_remain_visible() {
        let monitor = MicrophoneMonitor::default();
        monitor.start_with("first".into(), LIMIT, open).unwrap();
        monitor.start_with("second".into(), LIMIT, open).unwrap();
        monitor.stop();
        assert!(!monitor.status().active);
        monitor
            .start_with("missing".into(), LIMIT, |_| {
                anyhow::bail!("Device disconnected")
            })
            .unwrap();
        monitor.stop();
        assert!(monitor
            .status()
            .error
            .unwrap()
            .contains("Device disconnected"));
    }
}
