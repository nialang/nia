// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    fs,
    io::Read,
    path::PathBuf,
    process::{Command, ExitStatus, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

static TEMP_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);
const COMMAND_LIMIT: usize = 4;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(45);
const COMMAND_SLOT_STALE_AFTER: Duration = Duration::from_secs(15 * 60);

struct CommandPermit {
    slot: PathBuf,
}

fn acquire_command_permit() -> CommandPermit {
    let root = command_slot_root();
    fs::create_dir_all(&root).expect("create command slot root");
    loop {
        for index in 0..COMMAND_LIMIT {
            let slot = root.join(index.to_string());
            match fs::create_dir(&slot) {
                Ok(()) => {
                    write_slot_owner(&slot);
                    return CommandPermit { slot };
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    reclaim_stale_slot(&slot);
                }
                Err(error) => panic!("create command slot {}: {error}", slot.display()),
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn command_slot_root() -> PathBuf {
    let mut root = std::env::temp_dir();
    root.push("nia_cli_command_slots");
    root
}

fn write_slot_owner(slot: &std::path::Path) {
    let _ = fs::write(slot.join("owner"), std::process::id().to_string());
}

fn reclaim_stale_slot(slot: &std::path::Path) {
    if slot_owner_is_alive(slot) {
        return;
    }
    let stale_by_age = fs::metadata(slot)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed >= COMMAND_SLOT_STALE_AFTER);
    if !stale_by_age && slot_owner_is_unknown(slot) {
        return;
    }
    let _ = fs::remove_dir_all(slot);
}

fn slot_owner_is_unknown(slot: &std::path::Path) -> bool {
    fs::read_to_string(slot.join("owner"))
        .ok()
        .and_then(|owner| owner.trim().parse::<u32>().ok())
        .is_none()
}

fn slot_owner_is_alive(slot: &std::path::Path) -> bool {
    let Some(pid) = fs::read_to_string(slot.join("owner"))
        .ok()
        .and_then(|owner| owner.trim().parse::<u32>().ok())
    else {
        return false;
    };
    process_is_alive(pid)
}

#[cfg(target_os = "linux")]
fn process_is_alive(pid: u32) -> bool {
    std::path::Path::new("/proc").join(pid.to_string()).exists()
}

#[cfg(not(target_os = "linux"))]
fn process_is_alive(_pid: u32) -> bool {
    true
}

impl Drop for CommandPermit {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.slot);
    }
}

pub(crate) trait CommandExt {
    fn output_timeout(&mut self, context: &str) -> Output;
    fn status_timeout(&mut self, context: &str) -> ExitStatus;
}

impl CommandExt for Command {
    fn output_timeout(&mut self, context: &str) -> Output {
        let _permit = acquire_command_permit();
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
        let _permit = acquire_command_permit();
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
