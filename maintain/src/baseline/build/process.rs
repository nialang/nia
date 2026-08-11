use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::MaintainResult;
use crate::system::resources::probe_host_resources;

const MIN_AVAILABLE_MEMORY_BYTES: u64 = 2 * 1024 * 1024 * 1024;

pub(super) struct BoundedOutput {
    pub(super) status: ExitStatus,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
    pub(super) elapsed: f64,
    pub(super) available_memory: Option<u64>,
}

fn require_memory_headroom() -> MaintainResult<Option<u64>> {
    let available = probe_host_resources().available_memory_bytes();
    if available.is_some_and(|value| value < MIN_AVAILABLE_MEMORY_BYTES) {
        return Err(format!(
            "build baseline refused to start under memory pressure: available={} required={MIN_AVAILABLE_MEMORY_BYTES}",
            available.unwrap_or_default()
        ));
    }
    Ok(available)
}

fn terminate_process_group(child: &mut std::process::Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }

    // The build runner may leave descendants holding output pipes. Signal the
    // owned process group so timeout cleanup cannot strand those descendants.
    let group = format!("-{}", child.id());
    let _ = Command::new("kill")
        .args(["-TERM", "--", &group])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    let _ = Command::new("kill")
        .args(["-KILL", "--", &group])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.wait();
}

fn read_pipe<R: Read + Send + 'static>(
    mut pipe: R,
) -> thread::JoinHandle<std::io::Result<Vec<u8>>> {
    // Drain both pipes concurrently; waiting before draining can deadlock once
    // either kernel pipe buffer fills.
    thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

fn join_pipe(
    handle: thread::JoinHandle<std::io::Result<Vec<u8>>>,
    name: &str,
) -> MaintainResult<Vec<u8>> {
    handle
        .join()
        .map_err(|_| format!("{name} reader thread panicked"))?
        .map_err(|error| format!("failed to read child {name}: {error}"))
}

pub(super) fn run_bounded(
    command: &[String],
    cwd: &Path,
    timeout_seconds: u64,
) -> MaintainResult<BoundedOutput> {
    let available_memory = require_memory_headroom()?;
    let started = Instant::now();
    let mut process = Command::new(&command[0]);
    process
        .args(&command[1..])
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = process
        .spawn()
        .map_err(|error| format!("failed to run {}: {error}", command.join(" ")))?;
    let stdout = read_pipe(child.stdout.take().expect("piped child stdout"));
    let stderr = read_pipe(child.stderr.take().expect("piped child stderr"));
    let deadline = started + Duration::from_secs(timeout_seconds);
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("failed to wait for child process: {error}"))?
        {
            break status;
        }
        if Instant::now() >= deadline {
            terminate_process_group(&mut child);
            let _ = join_pipe(stdout, "stdout");
            let _ = join_pipe(stderr, "stderr");
            return Err(format!(
                "command timed out after {timeout_seconds}s: {}",
                command.join(" ")
            ));
        }
        thread::sleep(Duration::from_millis(25));
    };
    Ok(BoundedOutput {
        status,
        stdout: join_pipe(stdout, "stdout")?,
        stderr: join_pipe(stderr, "stderr")?,
        elapsed: started.elapsed().as_secs_f64(),
        available_memory,
    })
}
