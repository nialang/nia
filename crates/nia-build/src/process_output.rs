// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded subprocess output capture and owned process-tree cleanup.
//!
//! Output may be forwarded while retaining only a fixed tail for diagnostics.
//! Timeout, cancellation, and parent failure retire descendants instead of
//! leaving background tools attached to a completed build.

use std::{
    io::{self, Read, Write},
    process::{Child, Command},
    thread::JoinHandle,
};

#[derive(Clone, Copy)]
pub(crate) enum CapturedStream {
    Stdout,
    Stderr,
}

pub(crate) struct StreamCapture {
    pub(crate) tail: Vec<u8>,
    pub(crate) error: Option<io::Error>,
}

pub(crate) enum CaptureFailure {
    Thread {
        stream: &'static str,
    },
    Io {
        stream: &'static str,
        error: io::Error,
    },
}

pub(crate) struct CapturedOutput {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) failure: Option<CaptureFailure>,
}

/// Retire an already started worker when starting its sibling failed. The
/// startup failure remains primary, but any completed capture is still useful.
pub(crate) fn join_capture_tail(reader: JoinHandle<StreamCapture>) -> Vec<u8> {
    reader
        .join()
        .map(|capture| capture.tail)
        .unwrap_or_default()
}

/// Retire both workers before interpreting either result. A failed worker
/// must not detach its sibling or discard output the sibling already captured.
pub(crate) fn join_captures(
    stdout: JoinHandle<StreamCapture>,
    stderr: JoinHandle<StreamCapture>,
) -> CapturedOutput {
    let stdout = stdout.join();
    let stderr = stderr.join();
    let failed_worker = if stdout.is_err() {
        Some("stdout")
    } else if stderr.is_err() {
        Some("stderr")
    } else {
        None
    };
    let empty = || StreamCapture {
        tail: Vec::new(),
        error: None,
    };
    let stdout = stdout.unwrap_or_else(|_| empty());
    let stderr = stderr.unwrap_or_else(|_| empty());
    let failure = failed_worker
        .map(|stream| CaptureFailure::Thread { stream })
        .or_else(|| {
            stdout.error.map(|error| CaptureFailure::Io {
                stream: "stdout",
                error,
            })
        })
        .or_else(|| {
            stderr.error.map(|error| CaptureFailure::Io {
                stream: "stderr",
                error,
            })
        });
    CapturedOutput {
        stdout: stdout.tail,
        stderr: stderr.tail,
        failure,
    }
}

pub(crate) fn capture_stream(
    mut reader: impl Read,
    stream: CapturedStream,
    forward_output: bool,
    tail_limit: usize,
) -> StreamCapture {
    let mut tail = Vec::new();
    let mut first_error = None;
    let mut buffer = [0u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                append_output_tail(&mut tail, &buffer[..count], tail_limit);
                let forwarded = match (forward_output, stream) {
                    (false, _) => Ok(()),
                    (true, CapturedStream::Stdout) => io::stdout().write_all(&buffer[..count]),
                    (true, CapturedStream::Stderr) => io::stderr().write_all(&buffer[..count]),
                };
                if first_error.is_none() {
                    first_error = forwarded.err();
                }
            }
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
                break;
            }
        }
    }
    if forward_output {
        let flushed = match stream {
            CapturedStream::Stdout => io::stdout().flush(),
            CapturedStream::Stderr => io::stderr().flush(),
        };
        if first_error.is_none() {
            first_error = flushed.err();
        }
    }
    StreamCapture {
        tail,
        error: first_error,
    }
}

pub(crate) fn append_output_tail(tail: &mut Vec<u8>, bytes: &[u8], limit: usize) {
    if bytes.len() >= limit {
        tail.clear();
        tail.extend_from_slice(&bytes[bytes.len() - limit..]);
        return;
    }
    let excess = tail.len().saturating_add(bytes.len()).saturating_sub(limit);
    if excess != 0 {
        tail.drain(..excess);
    }
    tail.extend_from_slice(bytes);
}

#[cfg(unix)]
pub(crate) fn prepare_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0);
}

#[cfg(windows)]
pub(crate) fn prepare_process_group(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;
    // CREATE_NEW_PROCESS_GROUP gives taskkill /T a stable root even when the
    // child launches through cmd.exe or another Windows command host.
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP);
}

#[cfg(all(not(unix), not(windows)))]
pub(crate) fn prepare_process_group(_command: &mut Command) {}

#[cfg(unix)]
pub(crate) fn terminate_process_tree(child: &mut Child) {
    let Ok(group) = i32::try_from(child.id()) else {
        let _ = child.kill();
        return;
    };
    terminate_process_group(group);
}

#[cfg(windows)]
pub(crate) fn terminate_process_tree(child: &mut Child) {
    terminate_process_descendants(child.id());
    let _ = child.kill();
}

#[cfg(all(not(unix), not(windows)))]
pub(crate) fn terminate_process_tree(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(unix)]
pub(crate) fn terminate_process_descendants(group: u32) {
    if let Ok(group) = i32::try_from(group) {
        terminate_process_group(group);
    }
}

#[cfg(windows)]
pub(crate) fn terminate_process_descendants(group: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &group.to_string(), "/T", "/F"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(all(not(unix), not(windows)))]
pub(crate) fn terminate_process_descendants(_group: u32) {}

#[cfg(unix)]
fn terminate_process_group(group: i32) {
    let signaled = unsafe { libc::kill(-group, libc::SIGTERM) } == 0;
    if signaled {
        std::thread::sleep(std::time::Duration::from_millis(100));
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn capture_worker(stream: CapturedStream, fail: bool) -> JoinHandle<StreamCapture> {
        struct FailedRead;
        impl Read for FailedRead {
            fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("injected read failure"))
            }
        }
        std::thread::spawn(move || {
            let marker = match stream {
                CapturedStream::Stdout => "stdout-tail",
                CapturedStream::Stderr => "stderr-tail",
            };
            let bytes = io::Cursor::new(format!("discarded-prefix/{marker}"));
            if fail {
                capture_stream(bytes.chain(FailedRead), stream, false, marker.len())
            } else {
                capture_stream(bytes, stream, false, marker.len())
            }
        })
    }

    pub(crate) fn success_status() -> std::process::ExitStatus {
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt as _;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt as _;
        std::process::ExitStatus::from_raw(0)
    }

    #[test]
    fn startup_cleanup_keeps_partial_output_from_an_existing_worker() {
        assert_eq!(
            join_capture_tail(capture_worker(CapturedStream::Stdout, true)),
            b"stdout-tail"
        );
        assert!(
            join_capture_tail(std::thread::spawn(|| panic!("injected capture panic"))).is_empty()
        );
    }

    #[test]
    fn capture_failure_joins_sibling_and_retains_its_bounded_tail() {
        for failed in ["stdout", "stderr"] {
            let panic_worker = || std::thread::spawn(|| panic!("injected capture panic"));
            let stdout = if failed == "stdout" {
                panic_worker()
            } else {
                capture_worker(CapturedStream::Stdout, false)
            };
            let stderr = if failed == "stderr" {
                panic_worker()
            } else {
                capture_worker(CapturedStream::Stderr, false)
            };
            let result = join_captures(stdout, stderr);
            assert!(
                matches!(result.failure, Some(CaptureFailure::Thread { stream }) if stream == failed)
            );
            assert_eq!(
                result.stdout,
                if failed == "stdout" {
                    b"".as_slice()
                } else {
                    b"stdout-tail"
                }
            );
            assert_eq!(
                result.stderr,
                if failed == "stderr" {
                    b"".as_slice()
                } else {
                    b"stderr-tail"
                }
            );
        }
    }

    #[test]
    fn stream_io_failure_keeps_both_bounded_tails_and_stream_identity() {
        for failed in [None, Some("stdout"), Some("stderr")] {
            let result = join_captures(
                capture_worker(CapturedStream::Stdout, failed == Some("stdout")),
                capture_worker(CapturedStream::Stderr, failed == Some("stderr")),
            );
            assert_eq!(result.stdout, b"stdout-tail");
            assert_eq!(result.stderr, b"stderr-tail");
            match (failed, result.failure) {
                (None, None) => {}
                (Some(expected), Some(CaptureFailure::Io { stream, error })) => {
                    assert_eq!(stream, expected);
                    assert_eq!(error.to_string(), "injected read failure");
                }
                _ => panic!("capture failure changed identity"),
            }
        }
    }
}
