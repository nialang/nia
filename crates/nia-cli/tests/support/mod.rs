// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    fs,
    io::Read,
    path::PathBuf,
    process::{Command, ExitStatus, Output, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

static TEMP_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);
#[allow(dead_code)]
static BUILD_COMMAND_LOCK: Mutex<()> = Mutex::new(());

const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(300);
const DEFAULT_BUILD_COMMAND_TIMEOUT: Duration = Duration::from_secs(300);
const COMMAND_TIMEOUT_ENV: &str = "NIA_TEST_COMMAND_TIMEOUT_SECS";
const BUILD_COMMAND_TIMEOUT_ENV: &str = "NIA_TEST_BUILD_COMMAND_TIMEOUT_SECS";

pub(crate) trait CommandExt {
    fn output_timeout(&mut self, context: &str) -> Output;
}

pub(crate) trait CommandStatusExt {
    fn status_timeout(&mut self, context: &str) -> ExitStatus;
}

#[allow(dead_code)]
pub(crate) fn build_command_output_timeout(command: &mut Command, context: &str) -> Output {
    let _guard = BUILD_COMMAND_LOCK
        .lock()
        .expect("build command test lock poisoned");
    output_timeout_with(
        command,
        command_timeout_with(BUILD_COMMAND_TIMEOUT_ENV, DEFAULT_BUILD_COMMAND_TIMEOUT),
        context,
    )
}

impl CommandExt for Command {
    fn output_timeout(&mut self, context: &str) -> Output {
        output_timeout_with(self, command_timeout(), context)
    }
}

fn output_timeout_with(command: &mut Command, timeout: Duration, context: &str) -> Output {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    prepare_command(command);

    let mut child = command
        .spawn()
        .unwrap_or_else(|error| panic!("{context}: failed to spawn command: {error}"));
    let stdout = child
        .stdout
        .take()
        .expect("stdout pipe was configured before spawn");
    let stderr = child
        .stderr
        .take()
        .expect("stderr pipe was configured before spawn");
    let stdout_reader = thread::spawn(move || read_pipe(stdout));
    let stderr_reader = thread::spawn(move || read_pipe(stderr));

    let started = Instant::now();
    let wait = wait_child_timeout(&mut child, timeout, context);
    let run_time = started.elapsed();
    let stdout = join_reader(stdout_reader, context, "stdout");
    let stderr = join_reader(stderr_reader, context, "stderr");
    let status = match wait {
        Ok(status) => status,
        Err(()) => panic!(
            "{context}: command timed out after {timeout:?}; run_time={run_time:?}\nstdout tail:\n{}\nstderr tail:\n{}",
            output_tail(&stdout),
            output_tail(&stderr),
        ),
    };

    Output {
        status,
        stdout,
        stderr,
    }
}

impl CommandStatusExt for Command {
    fn status_timeout(&mut self, context: &str) -> ExitStatus {
        self.stdout(Stdio::null()).stderr(Stdio::null());
        prepare_command(self);

        let timeout = command_timeout();
        let mut child = self
            .spawn()
            .unwrap_or_else(|error| panic!("{context}: failed to spawn command: {error}"));
        wait_child_timeout(&mut child, timeout, context).unwrap_or_else(|()| {
            panic!("{context}: command timed out after {timeout:?}");
        })
    }
}

fn read_pipe<R: Read>(mut pipe: R) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes).map(|_| bytes)
}

fn join_reader(
    reader: thread::JoinHandle<std::io::Result<Vec<u8>>>,
    context: &str,
    stream: &str,
) -> Vec<u8> {
    reader
        .join()
        .unwrap_or_else(|_| panic!("{context}: {stream} reader panicked"))
        .unwrap_or_else(|error| panic!("{context}: failed to read {stream}: {error}"))
}

fn command_timeout() -> Duration {
    command_timeout_with(COMMAND_TIMEOUT_ENV, DEFAULT_COMMAND_TIMEOUT)
}

fn command_timeout_with(env: &str, default: Duration) -> Duration {
    std::env::var(env)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .unwrap_or(default)
}

fn prepare_command(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
}

fn wait_child_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
    context: &str,
) -> Result<ExitStatus, ()> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if start.elapsed() >= timeout => {
                terminate_child(child);
                let _ = child.wait();
                return Err(());
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => panic!("{context}: failed to wait for command: {error}"),
        }
    }
}

fn output_tail(bytes: &[u8]) -> String {
    const MAX_TAIL: usize = 4096;
    let start = bytes.len().saturating_sub(MAX_TAIL);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

#[cfg(unix)]
fn terminate_child(child: &mut std::process::Child) {
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(format!("-{}", child.id()))
        .status();
    thread::sleep(Duration::from_millis(100));
    if matches!(child.try_wait(), Ok(None)) {
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg(format!("-{}", child.id()))
            .status();
    }
}

#[cfg(not(unix))]
fn terminate_child(child: &mut std::process::Child) {
    let _ = child.kill();
}

pub(crate) fn temp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let id = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.push(format!(
        "nia_cli_test_{name}_{}_{:?}_{id}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}
