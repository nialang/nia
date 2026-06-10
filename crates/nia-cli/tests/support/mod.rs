// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    io::Read,
    process::{Command, ExitStatus, Output, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

static TEMP_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);
static COMMAND_LIMIT: CommandLimit = CommandLimit::new(4);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);

struct CommandLimit {
    state: Mutex<usize>,
    available: Condvar,
    max: usize,
}

struct CommandPermit<'a> {
    limit: &'a CommandLimit,
}

impl CommandLimit {
    const fn new(max: usize) -> Self {
        Self {
            state: Mutex::new(0),
            available: Condvar::new(),
            max,
        }
    }

    fn acquire(&self) -> CommandPermit<'_> {
        let mut running = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while *running >= self.max {
            running = self
                .available
                .wait(running)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        *running += 1;
        CommandPermit { limit: self }
    }
}

impl Drop for CommandPermit<'_> {
    fn drop(&mut self) {
        let mut running = self
            .limit
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *running -= 1;
        self.limit.available.notify_one();
    }
}

pub(crate) trait CommandExt {
    fn output_timeout(&mut self, context: &str) -> Output;
    fn status_timeout(&mut self, context: &str) -> ExitStatus;
}

impl CommandExt for Command {
    fn output_timeout(&mut self, context: &str) -> Output {
        let _permit = COMMAND_LIMIT.acquire();
        self.stdout(Stdio::piped()).stderr(Stdio::piped());
        prepare_command(self);
        let mut child = self
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
        let stdout_reader = thread::spawn(move || {
            let mut stdout = stdout;
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).map(|_| bytes)
        });
        let stderr_reader = thread::spawn(move || {
            let mut stderr = stderr;
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).map(|_| bytes)
        });
        let status = wait_child_timeout(&mut child, context);
        let stdout = stdout_reader
            .join()
            .unwrap_or_else(|_| panic!("{context}: stdout reader panicked"))
            .unwrap_or_else(|error| panic!("{context}: failed to read stdout: {error}"));
        let stderr = stderr_reader
            .join()
            .unwrap_or_else(|_| panic!("{context}: stderr reader panicked"))
            .unwrap_or_else(|error| panic!("{context}: failed to read stderr: {error}"));
        Output {
            status,
            stdout,
            stderr,
        }
    }

    fn status_timeout(&mut self, context: &str) -> ExitStatus {
        let _permit = COMMAND_LIMIT.acquire();
        prepare_command(self);
        let mut child = self
            .spawn()
            .unwrap_or_else(|error| panic!("{context}: failed to spawn command: {error}"));
        wait_child_timeout(&mut child, context)
    }
}

fn prepare_command(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
}

fn wait_child_timeout(child: &mut std::process::Child, context: &str) -> ExitStatus {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status,
            Ok(None) if start.elapsed() >= COMMAND_TIMEOUT => {
                terminate_child(child);
                let _ = child.wait();
                panic!("{context}: command timed out after {COMMAND_TIMEOUT:?}");
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => panic!("{context}: failed to wait for command: {error}"),
        }
    }
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

pub(crate) fn temp_dir(name: &str) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    let id = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.push(format!(
        "nia_cli_{name}_{}_{:?}_{id}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}
