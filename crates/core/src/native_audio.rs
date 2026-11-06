use crate::{
    capture::{Capture, MonoSource, PcmReader},
    types::Device,
};
use anyhow::{bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    time::Duration,
};

pub fn devices() -> Result<Vec<Device>> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|device| device.name().ok())
        .unwrap_or_else(|| "System default microphone".into());
    let mut devices = vec![Device::microphone("auto", &default_name)];
    for device in host.input_devices()? {
        let name = device.name()?;
        devices.push(Device::microphone(name.clone(), name));
    }
    Ok(devices)
}

pub(crate) fn input_name(name: &str) -> Result<String> {
    if name == "auto" {
        cpal::default_host()
            .default_input_device()
            .context("No default microphone")?
            .name()
            .map_err(Into::into)
    } else {
        Ok(name.to_owned())
    }
}

struct Input {
    receiver: Receiver<Vec<f32>>,
    error: Arc<Mutex<Option<String>>>,
}
impl MonoSource for Input {
    fn next_frames(&mut self) -> io::Result<Option<Vec<f32>>> {
        match self.receiver.recv() {
            Ok(frames) => Ok(Some(frames)),
            Err(_) => match self.error.lock().unwrap().take() {
                Some(error) => Err(io::Error::other(error)),
                None => Ok(None),
            },
        }
    }
}

fn stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    send: SyncSender<Vec<f32>>,
    error: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = config.channels as usize;
    let failed = error.clone();
    let stopped = stop.clone();
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _| {
            use cpal::Sample;
            let mono: Vec<f32> = data
                .chunks_exact(channels)
                .map(|frame| {
                    frame.iter().map(|&s| f32::from_sample(s)).sum::<f32>() / channels as f32
                })
                .collect();
            // Never wait for disk or inference on the audio callback. An overflow is a visible gap,
            // never a silently shortened recording. Already-delivered audio is drained and preserved.
            if send.try_send(mono).is_err() {
                *failed.lock().unwrap() =
                    Some("Microphone queue overflow; captured audio has been preserved".into());
                stopped.store(true, Ordering::SeqCst);
            }
        },
        move |e| {
            *error.lock().unwrap() = Some(format!("Microphone disconnected or unavailable: {e}"));
            stop.store(true, Ordering::SeqCst);
        },
        None,
    )?)
}

pub fn capture(name: &str) -> Result<Capture> {
    let name = name.to_owned();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let error = Arc::new(Mutex::new(None));
    let failed = error.clone();
    let (send, receiver) = mpsc::sync_channel(128);
    let (ready, startup) = mpsc::sync_channel(1);
    // Create, run and drop the platform stream on one thread (CoreAudio / WASAPI ownership).
    let worker = std::thread::spawn(move || -> Result<()> {
        let start = || -> Result<(cpal::Stream, u32)> {
            let host = cpal::default_host();
            let device = if name == "auto" {
                host.default_input_device()
            } else {
                host.input_devices()?
                    .find(|d| d.name().is_ok_and(|n| n == name))
            }
            .context(
                "Microphone not found. Check the selected device and system microphone permission.",
            )?;
            let supported = device
                .default_input_config()
                .context("Cannot open microphone. Allow OmaScribe in system Privacy settings.")?;
            let config: cpal::StreamConfig = supported.clone().into();
            let stream = match supported.sample_format() {
                cpal::SampleFormat::F32 => {
                    stream::<f32>(&device, &config, send, failed, stopped.clone())
                }
                cpal::SampleFormat::I16 => {
                    stream::<i16>(&device, &config, send, failed, stopped.clone())
                }
                cpal::SampleFormat::U16 => {
                    stream::<u16>(&device, &config, send, failed, stopped.clone())
                }
                cpal::SampleFormat::I32 => {
                    stream::<i32>(&device, &config, send, failed, stopped.clone())
                }
                cpal::SampleFormat::F64 => {
                    stream::<f64>(&device, &config, send, failed, stopped.clone())
                }
                other => bail!("Unsupported microphone format: {other}"),
            }?;
            stream.play()?;
            Ok((stream, config.sample_rate.0))
        };
        match start() {
            Ok((stream, rate)) => {
                if ready.send(Ok(rate)).is_err() {
                    return Ok(());
                }
                while !stopped.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                drop(stream);
                Ok(())
            }
            Err(e) => {
                let _ = ready.send(Err(format!("{e:#}")));
                Err(e)
            }
        }
    });
    let rate = match startup.recv_timeout(Duration::from_secs(90)) {
        Ok(Ok(rate)) => rate,
        result => {
            stop.store(true, Ordering::SeqCst);
            bail!(
                "{}",
                match result {
                    Ok(Err(e)) => e,
                    _ => "Microphone startup timed out. Check system permission, then try again."
                        .into(),
                }
            );
        }
    };
    let reader = match PcmReader::new(Input { receiver, error }, rate, stop.clone()) {
        Ok(reader) => reader,
        Err(error) => {
            stop.store(true, Ordering::SeqCst);
            return Err(error);
        }
    };
    Ok(Capture::reader(reader, stop, Some(worker)))
}
