//! Time- and output-bounded helper processes. Callers choose trusted app-owned binaries.
use anyhow::{bail, Context, Result};
use std::{
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(crate) fn capture(command: &mut Command, timeout: Duration, limit: usize) -> Result<Vec<u8>> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().context("Probe stdout unavailable")?;
    let reader = std::thread::spawn(move || {
        let mut output = vec![];
        stdout
            .take((limit + 1) as u64)
            .read_to_end(&mut output)
            .map(|_| output)
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(anyhow::anyhow!("Probe timed out or stopped"));
            }
        }
    };
    let output = reader
        .join()
        .map_err(|_| anyhow::anyhow!("Probe reader stopped"))??;
    if output.len() > limit {
        bail!("Probe output exceeded limit");
    }
    if !status?.success() {
        bail!("Probe failed");
    }
    Ok(output)
}
