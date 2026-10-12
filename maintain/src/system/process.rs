use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::MaintainResult;

const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;

/// Captured result from one independently executed child process.
#[derive(Debug)]
pub struct BoundedOutput {
    /// Child process identifier.
    pub process_id: u32,
    /// Child exit status.
    pub status: ExitStatus,
    /// Captured standard output, bounded by the maintenance capture limit.
    pub stdout: Vec<u8>,
    /// Captured standard error, bounded by the maintenance capture limit.
    pub stderr: Vec<u8>,
    /// Parent-observed wall time in seconds.
    pub elapsed: f64,
}

#[cfg(unix)]
fn terminate_process_group(child: &mut std::process::Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
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

#[cfg(not(unix))]
fn terminate_process_group(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn configure_process_group(process: &mut Command) {
    process.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_process: &mut Command) {}

fn read_pipe<R: Read + Send + 'static>(
    mut pipe: R,
) -> thread::JoinHandle<std::io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 8192];
        let mut overflowed = false;
        loop {
            let count = pipe.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            if bytes.len() < MAX_CAPTURE_BYTES {
                let retained = count.min(MAX_CAPTURE_BYTES - bytes.len());
                bytes.extend_from_slice(&buffer[..retained]);
                overflowed |= retained != count;
            } else {
                overflowed = true;
            }
        }
        if overflowed {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("child output exceeded {MAX_CAPTURE_BYTES} bytes"),
            ));
        }
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

/// Executes a command in an owned process group with bounded output capture.
pub fn run_bounded(
    command: &[String],
    cwd: &Path,
    timeout_seconds: u64,
) -> MaintainResult<BoundedOutput> {
    if command.is_empty() {
        return Err("cannot run an empty command".to_owned());
    }
    let started = Instant::now();
    let mut process = Command::new(&command[0]);
    process
        .args(&command[1..])
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_process_group(&mut process);
    let mut child = process
        .spawn()
        .map_err(|error| format!("failed to run {}: {error}", command.join(" ")))?;
    let process_id = child.id();
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
        process_id,
        status,
        stdout: join_pipe(stdout, "stdout")?,
        stderr: join_pipe(stderr, "stderr")?,
        elapsed: started.elapsed().as_secs_f64(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn capture_rejects_output_beyond_the_fixed_limit() {
        let bytes = vec![b'x'; MAX_CAPTURE_BYTES + 1];
        let result = read_pipe(Cursor::new(bytes)).join().unwrap();
        let error = result.expect_err("oversized output must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("child output exceeded"));
    }

    #[test]
    fn capture_accepts_output_at_the_fixed_limit() {
        let bytes = vec![b'x'; MAX_CAPTURE_BYTES];
        let captured = read_pipe(Cursor::new(bytes)).join().unwrap().unwrap();
        assert_eq!(captured.len(), MAX_CAPTURE_BYTES);
    }

    #[test]
    fn empty_commands_are_rejected_before_process_setup() {
        let error = run_bounded(&[], Path::new("."), 1).expect_err("empty command");
        assert_eq!(error, "cannot run an empty command");
    }
}
