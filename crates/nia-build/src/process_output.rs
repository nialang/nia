// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded subprocess output capture and owned process-tree cleanup.
//!
//! Output may be forwarded while retaining only a fixed tail for diagnostics.
//! Timeout, cancellation, and parent failure retire descendants instead of
//! leaving background tools attached to a completed build.

use std::{
    io::{self, Read, Write},
    process::{Child, Command},
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

#[cfg(not(unix))]
pub(crate) fn prepare_process_group(_command: &mut Command) {}

#[cfg(unix)]
pub(crate) fn terminate_process_tree(child: &mut Child) {
    let Ok(group) = i32::try_from(child.id()) else {
        let _ = child.kill();
        return;
    };
    terminate_process_group(group);
}

#[cfg(not(unix))]
pub(crate) fn terminate_process_tree(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(unix)]
pub(crate) fn terminate_process_descendants(group: u32) {
    if let Ok(group) = i32::try_from(group) {
        terminate_process_group(group);
    }
}

#[cfg(not(unix))]
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
