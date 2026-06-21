// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io::Read,
    path::PathBuf,
    process::{Command, ExitStatus, Output, Stdio},
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

static TEMP_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);
const MAX_COMMAND_LIMIT: usize = 4;
const MAX_HEAVY_COMPILER_LIMIT: usize = 3;
const MAX_OBJECT_EMIT_LIMIT: usize = 3;
const MAX_ARTIFACT_EMIT_LIMIT: usize = 2;
const COMMAND_SLOT_MEMORY_BYTES: usize = 384 * 1024 * 1024;
const HEAVY_COMPILER_SLOT_MEMORY_BYTES: usize = 1024 * 1024 * 1024;
const OBJECT_EMIT_SLOT_MEMORY_BYTES: usize = 1024 * 1024 * 1024;
const ARTIFACT_EMIT_SLOT_MEMORY_BYTES: usize = 1536 * 1024 * 1024;
const GENERAL_COMMAND_TIMEOUT: Duration = Duration::from_secs(45);
const HEAVY_COMPILER_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const ARTIFACT_EMIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(180);
const PERMIT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const COMMAND_SLOT_STALE_AFTER: Duration = Duration::from_secs(15 * 60);

struct CommandPermit {
    slots: Vec<PathBuf>,
}

#[derive(Clone, Copy)]
enum CommandClass {
    General,
    HeavyCompiler,
    ObjectEmit,
    ArtifactEmit,
}

#[derive(Clone, Copy)]
enum SlotClass {
    Command,
    HeavyCompiler,
    ObjectEmit,
    ArtifactEmit,
}

fn acquire_command_permit(class: CommandClass) -> CommandPermit {
    let mut slots = Vec::new();
    match class {
        CommandClass::General => {}
        CommandClass::HeavyCompiler => {
            slots.push(acquire_slot(SlotClass::HeavyCompiler));
        }
        CommandClass::ObjectEmit => {
            slots.push(acquire_slot(SlotClass::HeavyCompiler));
            slots.push(acquire_slot(SlotClass::ObjectEmit));
        }
        CommandClass::ArtifactEmit => {
            slots.push(acquire_slot(SlotClass::HeavyCompiler));
            slots.push(acquire_slot(SlotClass::ArtifactEmit));
        }
    }
    slots.push(acquire_slot(SlotClass::Command));
    CommandPermit { slots }
}

fn acquire_slot(class: SlotClass) -> PathBuf {
    let root = command_slot_root(class);
    fs::create_dir_all(&root).expect("create command slot root");
    let start = Instant::now();
    let mut sleep = Duration::from_millis(10);
    loop {
        for index in 0..slot_limit(class) {
            let slot = root.join(index.to_string());
            match fs::create_dir(&slot) {
                Ok(()) => {
                    write_slot_owner(&slot);
                    return slot;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    reclaim_stale_slot(&slot);
                }
                Err(error) => panic!("create command slot {}: {error}", slot.display()),
            }
        }
        if start.elapsed() >= PERMIT_TIMEOUT {
            panic!(
                "timed out after {PERMIT_TIMEOUT:?} waiting for {} command slot in {}",
                class.slot_name(),
                root.display()
            );
        }
        thread::sleep(sleep);
        sleep = (sleep * 2).min(Duration::from_millis(250));
    }
}

fn command_slot_root(class: SlotClass) -> PathBuf {
    let mut root = std::env::temp_dir();
    root.push("nia_cli_command_slots");
    root.push(workspace_slot_namespace());
    root.push(class.slot_name());
    root
}

fn workspace_slot_namespace() -> String {
    let mut hasher = DefaultHasher::new();
    env!("CARGO_MANIFEST_DIR").hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn slot_limit(class: SlotClass) -> usize {
    match class {
        SlotClass::Command => command_slot_limit(),
        SlotClass::HeavyCompiler => heavy_compiler_slot_limit()
            .min(child_compiler_check_limit())
            .min(command_slot_limit()),
        SlotClass::ObjectEmit => object_emit_slot_limit()
            .min(child_compiler_check_limit())
            .min(command_slot_limit()),
        SlotClass::ArtifactEmit => artifact_emit_slot_limit()
            .min(child_compiler_check_limit())
            .min(command_slot_limit()),
    }
}

fn command_slot_limit() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        if let Some(limit) = env_slot_limit("NIA_CLI_TEST_COMMAND_LIMIT") {
            return limit;
        }
        let cpu_limit = available_parallelism().clamp(1, MAX_COMMAND_LIMIT);
        let memory_limit =
            memory_limited_parallelism(COMMAND_SLOT_MEMORY_BYTES).unwrap_or(MAX_COMMAND_LIMIT);
        cpu_limit.min(memory_limit).clamp(1, MAX_COMMAND_LIMIT)
    })
}

fn heavy_compiler_slot_limit() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        if let Some(limit) = env_slot_limit("NIA_CLI_TEST_HEAVY_LIMIT") {
            return limit;
        }
        let cpu_limit = (available_parallelism() / 4).clamp(1, MAX_HEAVY_COMPILER_LIMIT);
        let memory_limit =
            memory_limited_parallelism(HEAVY_COMPILER_SLOT_MEMORY_BYTES).unwrap_or(cpu_limit);
        cpu_limit
            .min(memory_limit)
            .clamp(1, MAX_HEAVY_COMPILER_LIMIT)
    })
}

fn artifact_emit_slot_limit() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        if let Some(limit) = env_slot_limit("NIA_CLI_TEST_ARTIFACT_LIMIT") {
            return limit;
        }
        let cpu_limit = (available_parallelism() / 4).clamp(1, MAX_ARTIFACT_EMIT_LIMIT);
        let memory_limit =
            memory_limited_parallelism(ARTIFACT_EMIT_SLOT_MEMORY_BYTES).unwrap_or(cpu_limit);
        cpu_limit
            .min(memory_limit)
            .clamp(1, MAX_ARTIFACT_EMIT_LIMIT)
    })
}

fn object_emit_slot_limit() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        if let Some(limit) = env_slot_limit("NIA_CLI_TEST_OBJECT_LIMIT") {
            return limit;
        }
        let cpu_limit = (available_parallelism() / 4).clamp(1, MAX_OBJECT_EMIT_LIMIT);
        let memory_limit =
            memory_limited_parallelism(OBJECT_EMIT_SLOT_MEMORY_BYTES).unwrap_or(cpu_limit);
        cpu_limit.min(memory_limit).clamp(1, MAX_OBJECT_EMIT_LIMIT)
    })
}

fn child_compiler_check_limit() -> usize {
    env_slot_limit("NIA_COMPILER_CHECK_LIMIT").unwrap_or_else(heavy_compiler_slot_limit)
}

fn env_slot_limit(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map(|limit| limit.max(1))
}

fn available_parallelism() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
}

fn memory_limited_parallelism(bytes_per_slot: usize) -> Option<usize> {
    let mem_available_kb = linux_mem_available_kb()?;
    let available_bytes = mem_available_kb.saturating_mul(1024);
    Some((available_bytes / bytes_per_slot).max(1))
}

#[cfg(target_os = "linux")]
fn linux_mem_available_kb() -> Option<usize> {
    let meminfo = fs::read_to_string("/proc/meminfo").ok()?;
    for line in meminfo.lines() {
        let Some(rest) = line.strip_prefix("MemAvailable:") else {
            continue;
        };
        return rest.split_whitespace().next()?.parse().ok();
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn linux_mem_available_kb() -> Option<usize> {
    None
}

impl SlotClass {
    fn slot_name(self) -> &'static str {
        match self {
            SlotClass::Command => "command",
            SlotClass::HeavyCompiler => "heavy_compiler",
            SlotClass::ObjectEmit => "object_emit",
            SlotClass::ArtifactEmit => "artifact_emit",
        }
    }
}

impl CommandClass {
    fn command_timeout(self) -> Duration {
        match self {
            CommandClass::General => GENERAL_COMMAND_TIMEOUT,
            CommandClass::HeavyCompiler => HEAVY_COMPILER_COMMAND_TIMEOUT,
            CommandClass::ObjectEmit => ARTIFACT_EMIT_COMMAND_TIMEOUT,
            CommandClass::ArtifactEmit => ARTIFACT_EMIT_COMMAND_TIMEOUT,
        }
    }

    fn name(self) -> &'static str {
        match self {
            CommandClass::General => "general",
            CommandClass::HeavyCompiler => "heavy-compiler",
            CommandClass::ObjectEmit => "object-emit",
            CommandClass::ArtifactEmit => "artifact-emit",
        }
    }

    fn debug_summary(self) -> String {
        format!(
            "class={}, command_limit={}, heavy_limit={}, object_limit={}, artifact_limit={}, child_check_limit={}, NIA_COMPILER_CHECK_LIMIT={:?}",
            self.name(),
            command_slot_limit(),
            heavy_compiler_slot_limit(),
            object_emit_slot_limit(),
            artifact_emit_slot_limit(),
            child_compiler_check_limit(),
            std::env::var("NIA_COMPILER_CHECK_LIMIT").ok(),
        )
    }
}

fn write_slot_owner(slot: &std::path::Path) {
    let pid = std::process::id();
    let start_time = process_start_time(pid).unwrap_or(0);
    let _ = fs::write(slot.join("owner"), format!("{pid} {start_time}"));
}

fn reclaim_stale_slot(slot: &std::path::Path) {
    if slot_owner_is_alive(slot) {
        return;
    }
    if slot_owner_is_unknown(slot) && !slot_is_stale_by_age(slot) {
        return;
    }
    let _ = fs::remove_dir_all(slot);
}

fn slot_is_stale_by_age(slot: &std::path::Path) -> bool {
    fs::metadata(slot)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed >= COMMAND_SLOT_STALE_AFTER)
}

fn slot_owner_is_unknown(slot: &std::path::Path) -> bool {
    read_slot_owner(slot).is_none()
}

fn slot_owner_is_alive(slot: &std::path::Path) -> bool {
    let Some((pid, start_time)) = read_slot_owner(slot) else {
        return false;
    };
    process_is_alive(pid, start_time)
}

fn read_slot_owner(slot: &std::path::Path) -> Option<(u32, u64)> {
    let owner = fs::read_to_string(slot.join("owner")).ok()?;
    let mut parts = owner.split_whitespace();
    let pid = parts.next()?.parse().ok()?;
    let start_time = parts.next()?.parse().ok()?;
    Some((pid, start_time))
}

#[cfg(target_os = "linux")]
fn process_is_alive(pid: u32, expected_start_time: u64) -> bool {
    let Some(start_time) = process_start_time(pid) else {
        return false;
    };
    expected_start_time == 0 || start_time == expected_start_time
}

#[cfg(not(target_os = "linux"))]
fn process_is_alive(_pid: u32, _expected_start_time: u64) -> bool {
    true
}

#[cfg(target_os = "linux")]
fn process_start_time(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(
        std::path::Path::new("/proc")
            .join(pid.to_string())
            .join("stat"),
    )
    .ok()?;
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

#[cfg(not(target_os = "linux"))]
fn process_start_time(_pid: u32) -> Option<u64> {
    None
}

impl Drop for CommandPermit {
    fn drop(&mut self) {
        while let Some(slot) = self.slots.pop() {
            let _ = fs::remove_dir_all(slot);
        }
    }
}

pub(crate) trait CommandExt {
    fn output_timeout(&mut self, context: &str) -> Output;
}

pub(crate) trait CommandStatusExt {
    fn status_timeout(&mut self, context: &str) -> ExitStatus;
}

impl CommandExt for Command {
    fn output_timeout(&mut self, context: &str) -> Output {
        let class = classify_command(self);
        let _permit = acquire_command_permit(class);
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
        let status = wait_child_timeout(&mut child, class, context);
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
}

impl CommandStatusExt for Command {
    fn status_timeout(&mut self, context: &str) -> ExitStatus {
        let class = classify_command(self);
        let _permit = acquire_command_permit(class);
        self.stdout(Stdio::null()).stderr(Stdio::null());
        prepare_command(self);
        let mut child = self
            .spawn()
            .unwrap_or_else(|error| panic!("{context}: failed to spawn command: {error}"));
        wait_child_timeout(&mut child, class, context)
    }
}

fn classify_command(command: &Command) -> CommandClass {
    let mut saw_emit = false;
    let mut saw_check = false;
    let mut saw_heavy_emit_target = false;
    let mut saw_exe_target = false;
    let mut saw_obj_target = false;
    for arg in command.get_args().filter_map(|arg| arg.to_str()) {
        if arg == "check" {
            saw_check = true;
        }
        if arg == "emit" {
            saw_emit = true;
        }
        if matches!(arg, "--checked" | "--backend" | "--llvm") {
            saw_heavy_emit_target = true;
        }
        if arg == "--exe" {
            saw_exe_target = true;
        }
        if arg == "--obj" {
            saw_obj_target = true;
        }
    }
    if saw_emit && saw_exe_target {
        CommandClass::ArtifactEmit
    } else if saw_emit && saw_obj_target {
        CommandClass::ObjectEmit
    } else if saw_check || (saw_emit && saw_heavy_emit_target) {
        CommandClass::HeavyCompiler
    } else {
        CommandClass::General
    }
}

fn prepare_command(command: &mut Command) {
    if std::env::var_os("NIA_COMPILER_CHECK_LIMIT").is_none() {
        command.env(
            "NIA_COMPILER_CHECK_LIMIT",
            heavy_compiler_slot_limit().to_string(),
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
}

fn wait_child_timeout(
    child: &mut std::process::Child,
    class: CommandClass,
    context: &str,
) -> ExitStatus {
    let timeout = class.command_timeout();
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status,
            Ok(None) if start.elapsed() >= timeout => {
                terminate_child(child);
                let _ = child.wait();
                panic!(
                    "{context}: command timed out after {timeout:?}; {}",
                    class.debug_summary()
                );
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
