// SPDX-License-Identifier: GPL-3.0-or-later
mod cases;

pub use cases::{CaseManifest, case_directories, fixture_relative_path};

use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Output, Stdio},
    sync::{Condvar, Mutex, MutexGuard, OnceLock},
    thread,
    time::{Duration, Instant},
};

const MAX_PARALLEL_COMPILERS: usize = 8;
const UNKNOWN_MEMORY_PARALLEL_COMPILERS: usize = 1;
const COMPILER_MEMORY_BYTES: usize = 1536 * 1024 * 1024;
const AVAILABLE_MEMORY_HEADROOM_BYTES: usize = 512 * 1024 * 1024;
const PERMIT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const UNKNOWN_OWNER_STALE_AFTER: Duration = Duration::from_secs(2 * 60 * 60);
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(420);

static COMPILER_POOL: OnceLock<ResourcePool> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestWorkload {
    Compiler,
    Build,
}

impl TestWorkload {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Compiler => "compiler",
            Self::Build => "build",
        }
    }

    const fn resource_request(self) -> ResourceRequest {
        match self {
            Self::Compiler => ResourceRequest::new(1, 1),
            Self::Build => ResourceRequest::new(2, 1),
        }
    }
}

pub fn acquire_test_resources(workload: TestWorkload) -> TestResourceSession<'static> {
    compiler_pool().acquire_session(workload.resource_request())
}

pub trait CommandExt {
    fn output_timeout(&mut self, context: &str) -> Output;
    fn output_timeout_with_workload(&mut self, context: &str, workload: TestWorkload) -> Output;
    fn output_timeout_in_session(&mut self, context: &str) -> Output;
}

pub trait CommandStatusExt {
    fn status_timeout(&mut self, context: &str) -> ExitStatus;
}

impl CommandExt for Command {
    fn output_timeout(&mut self, context: &str) -> Output {
        self.output_timeout_with_workload(context, TestWorkload::Compiler)
    }

    fn output_timeout_with_workload(&mut self, context: &str, workload: TestWorkload) -> Output {
        let _resources = acquire_test_resources(workload);
        self.output_timeout_in_session(context)
    }

    fn output_timeout_in_session(&mut self, context: &str) -> Output {
        self.stdout(Stdio::piped()).stderr(Stdio::piped());
        prepare_command(self);

        let timeout = DEFAULT_COMMAND_TIMEOUT;
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
}

impl CommandStatusExt for Command {
    fn status_timeout(&mut self, context: &str) -> ExitStatus {
        self.stdout(Stdio::null()).stderr(Stdio::null());
        prepare_command(self);

        let mut child = self
            .spawn()
            .unwrap_or_else(|error| panic!("{context}: failed to spawn command: {error}"));
        wait_child_timeout(&mut child, DEFAULT_COMMAND_TIMEOUT, context).unwrap_or_else(|()| {
            panic!("{context}: command timed out after {DEFAULT_COMMAND_TIMEOUT:?}");
        })
    }
}

fn read_pipe<R: Read>(mut pipe: R) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes).map(|_| bytes)
}

fn join_reader(
    reader: thread::JoinHandle<io::Result<Vec<u8>>>,
    context: &str,
    stream: &str,
) -> Vec<u8> {
    reader
        .join()
        .unwrap_or_else(|_| panic!("{context}: {stream} reader panicked"))
        .unwrap_or_else(|error| panic!("{context}: failed to read {stream}: {error}"))
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

#[derive(Clone, Copy)]
struct ResourceRequest {
    slots: usize,
    memory_units: usize,
}

impl ResourceRequest {
    const fn new(slots: usize, memory_units: usize) -> Self {
        Self {
            slots,
            memory_units,
        }
    }
}

fn compiler_pool() -> &'static ResourcePool {
    COMPILER_POOL.get_or_init(|| {
        ResourcePool::with_memory_gate(
            parallel_compiler_limit(),
            compiler_slot_root(),
            nia_query::effective_memory_limit_bytes(),
        )
    })
}

fn parallel_compiler_limit() -> usize {
    let available_cpus = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    compiler_limit(available_cpus, nia_query::effective_memory_limit_bytes())
}

fn compiler_limit(available_cpus: usize, system_memory: Option<usize>) -> usize {
    let cpu_limit = available_cpus.clamp(1, MAX_PARALLEL_COMPILERS);
    let memory_limit = system_memory
        .map(|total| {
            test_memory_budget(total)
                .checked_div(COMPILER_MEMORY_BYTES)
                .unwrap_or(0)
                .max(1)
        })
        .unwrap_or(UNKNOWN_MEMORY_PARALLEL_COMPILERS);
    cpu_limit.min(memory_limit).max(1)
}

fn test_memory_budget(total: usize) -> usize {
    total / 2
}

struct ResourcePool {
    capacity: usize,
    available: Mutex<usize>,
    ready: Condvar,
    slot_root: PathBuf,
    memory_limit: Option<usize>,
}

impl ResourcePool {
    #[cfg(test)]
    fn new(capacity: usize, slot_root: PathBuf) -> Self {
        Self::with_memory_gate(capacity, slot_root, None)
    }

    fn with_memory_gate(capacity: usize, slot_root: PathBuf, memory_limit: Option<usize>) -> Self {
        Self {
            capacity,
            available: Mutex::new(capacity),
            ready: Condvar::new(),
            slot_root,
            memory_limit,
        }
    }

    fn acquire_session(&self, request: ResourceRequest) -> TestResourceSession<'_> {
        let reserved_slots = request.slots.clamp(1, self.capacity);
        let mut available = lock_unpoisoned(&self.available);
        while *available < reserved_slots {
            available = self
                .ready
                .wait(available)
                .unwrap_or_else(|error| error.into_inner());
        }
        *available -= reserved_slots;
        drop(available);

        let minimum_available_memory = self
            .memory_limit
            .map(|limit| minimum_available_memory(limit, request.memory_units.max(1)));
        let slots = match acquire_process_slots(
            &self.slot_root,
            self.capacity,
            reserved_slots,
            minimum_available_memory,
        ) {
            Ok(slots) => slots,
            Err(error) => {
                self.release(reserved_slots);
                panic!(
                    "failed to acquire Nia test resource session in {}: {error}",
                    self.slot_root.display()
                );
            }
        };
        TestResourceSession {
            pool: self,
            reserved_slots,
            slots,
        }
    }

    fn release(&self, reserved_slots: usize) {
        *lock_unpoisoned(&self.available) += reserved_slots;
        self.ready.notify_all();
    }
}

pub struct TestResourceSession<'a> {
    pool: &'a ResourcePool,
    reserved_slots: usize,
    slots: Vec<PathBuf>,
}

impl Drop for TestResourceSession<'_> {
    fn drop(&mut self) {
        for slot in self.slots.drain(..) {
            let _ = fs::remove_dir_all(slot);
        }
        self.pool.release(self.reserved_slots);
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn compiler_slot_root() -> PathBuf {
    let workspace = env!("CARGO_MANIFEST_DIR")
        .replace(['/', '\\'], "_")
        .replace(':', "_");
    std::env::temp_dir()
        .join("nia_test_resource_slots")
        .join(workspace)
}

fn acquire_process_slots(
    root: &Path,
    capacity: usize,
    requested: usize,
    minimum_available_memory: Option<usize>,
) -> io::Result<Vec<PathBuf>> {
    fs::create_dir_all(root)?;
    let start = Instant::now();
    let mut delay = Duration::from_millis(10);
    loop {
        let mut acquired = Vec::with_capacity(requested);
        for index in 0..capacity {
            let slot = root.join(index.to_string());
            match fs::create_dir(&slot) {
                Ok(()) => {
                    if let Err(error) = write_process_owner(&slot) {
                        let _ = fs::remove_dir_all(&slot);
                        release_process_slots(acquired);
                        return Err(error);
                    }
                    acquired.push(slot);
                    if acquired.len() == requested {
                        if memory_pressure_allows(minimum_available_memory) {
                            return Ok(acquired);
                        }
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    reclaim_stale_process_slot(&slot);
                }
                Err(error) => {
                    release_process_slots(acquired);
                    return Err(error);
                }
            }
        }
        release_process_slots(acquired);
        if start.elapsed() >= PERMIT_TIMEOUT {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("timed out after {PERMIT_TIMEOUT:?}"),
            ));
        }
        thread::sleep(delay + process_backoff_jitter());
        delay = (delay * 2).min(Duration::from_millis(250));
    }
}

fn minimum_available_memory(memory_limit: usize, memory_units: usize) -> usize {
    let workload_memory = memory_units
        .saturating_mul(COMPILER_MEMORY_BYTES)
        .saturating_add(AVAILABLE_MEMORY_HEADROOM_BYTES);
    workload_memory.min(test_memory_budget(memory_limit))
}

fn memory_pressure_allows(minimum_available_memory: Option<usize>) -> bool {
    minimum_available_memory.is_none_or(|minimum| {
        nia_query::effective_available_memory_bytes().is_none_or(|available| available >= minimum)
    })
}

fn release_process_slots(slots: Vec<PathBuf>) {
    for slot in slots {
        let _ = fs::remove_dir_all(slot);
    }
}

fn process_backoff_jitter() -> Duration {
    Duration::from_millis(u64::from(std::process::id() % 17))
}

fn write_process_owner(slot: &Path) -> io::Result<()> {
    let pid = std::process::id();
    let start_time = process_start_time(pid).unwrap_or(0);
    fs::write(slot.join("owner"), format!("{pid} {start_time}"))
}

fn read_process_owner(slot: &Path) -> Option<(u32, u64)> {
    let owner = fs::read_to_string(slot.join("owner")).ok()?;
    let mut parts = owner.split_whitespace();
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn reclaim_stale_process_slot(slot: &Path) {
    if let Some((pid, start_time)) = read_process_owner(slot) {
        match process_is_alive(pid, start_time) {
            Some(true) => return,
            Some(false) => {
                let _ = fs::remove_dir_all(slot);
                return;
            }
            None => {}
        }
    }
    if slot_age(slot).is_some_and(|age| age >= UNKNOWN_OWNER_STALE_AFTER) {
        let _ = fs::remove_dir_all(slot);
    }
}

fn slot_age(slot: &Path) -> Option<Duration> {
    fs::metadata(slot).ok()?.modified().ok()?.elapsed().ok()
}

#[cfg(target_os = "linux")]
fn process_is_alive(pid: u32, expected_start_time: u64) -> Option<bool> {
    match fs::read_to_string(process_stat_path(pid)) {
        Ok(stat) => parse_process_start_time(&stat)
            .map(|start_time| expected_start_time == 0 || start_time == expected_start_time),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
}

#[cfg(not(target_os = "linux"))]
fn process_is_alive(_pid: u32, _expected_start_time: u64) -> Option<bool> {
    None
}

#[cfg(target_os = "linux")]
fn process_start_time(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(process_stat_path(pid)).ok()?;
    parse_process_start_time(&stat)
}

#[cfg(target_os = "linux")]
fn process_stat_path(pid: u32) -> PathBuf {
    Path::new("/proc").join(pid.to_string()).join("stat")
}

#[cfg(target_os = "linux")]
fn parse_process_start_time(stat: &str) -> Option<u64> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_limit_reserves_memory_and_caps_cpu_parallelism() {
        assert_eq!(compiler_limit(32, Some(8 * 1024 * 1024 * 1024)), 2);
        assert_eq!(compiler_limit(32, Some(3 * 1024 * 1024 * 1024)), 1);
        assert_eq!(compiler_limit(2, Some(8 * 1024 * 1024 * 1024)), 2);
        assert_eq!(compiler_limit(32, Some(32 * 1024 * 1024 * 1024)), 8);
    }

    #[test]
    fn compiler_limit_always_allows_progress() {
        assert_eq!(compiler_limit(0, Some(0)), 1);
        assert_eq!(compiler_limit(16, None), UNKNOWN_MEMORY_PARALLEL_COMPILERS);
    }

    #[test]
    fn available_memory_gate_scales_down_on_small_hosts() {
        assert_eq!(
            minimum_available_memory(8 * 1024 * 1024 * 1024, 2),
            7 * 512 * 1024 * 1024
        );
        assert_eq!(
            minimum_available_memory(3 * 1024 * 1024 * 1024, 1),
            3 * 512 * 1024 * 1024
        );
    }

    #[test]
    fn weighted_sessions_return_their_full_capacity() {
        let pool = ResourcePool::new(4, test_slot_root("weighted_sessions"));
        let session = pool.acquire_session(TestWorkload::Build.resource_request());
        assert_eq!(*lock_unpoisoned(&pool.available), 2);
        drop(session);
        assert_eq!(*lock_unpoisoned(&pool.available), 4);
    }

    #[test]
    fn scheduling_weight_does_not_inflate_memory_requirement() {
        let build = ResourceRequest::new(2, 1);
        assert_eq!(
            minimum_available_memory(8 * 1024 * 1024 * 1024, build.memory_units),
            2 * 1024 * 1024 * 1024
        );
    }

    #[test]
    fn process_slots_coordinate_independent_pools() {
        let root = test_slot_root("cross_process_slots");
        let first = ResourcePool::new(2, root.clone());
        let second = ResourcePool::new(2, root);
        let session = first.acquire_session(ResourceRequest::new(2, 1));
        assert!(second.slot_root.join("0").is_dir());
        assert!(second.slot_root.join("1").is_dir());
        drop(session);
        let second_session = second.acquire_session(ResourceRequest::new(2, 1));
        drop(second_session);
    }

    #[test]
    fn process_slots_block_independent_pools_until_release() {
        let root = test_slot_root("cross_process_blocking");
        let first = ResourcePool::new(1, root.clone());
        let first_session = first.acquire_session(ResourceRequest::new(1, 1));
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let second = ResourcePool::new(1, root);
            let second_session = second.acquire_session(ResourceRequest::new(1, 1));
            acquired_tx.send(()).expect("report acquired process slot");
            drop(second_session);
        });

        assert!(
            acquired_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "independent pool acquired an occupied process slot"
        );
        drop(first_session);
        acquired_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("independent pool should acquire the released process slot");
        waiter.join().expect("process slot waiter");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn process_liveness_uses_pid_and_start_time() {
        let pid = std::process::id();
        let start_time = process_start_time(pid).expect("current process has a start time");
        assert_eq!(process_is_alive(pid, start_time), Some(true));
        assert_eq!(
            process_is_alive(pid, start_time.saturating_add(1)),
            Some(false)
        );
    }

    fn test_slot_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("nia-test-support-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }
}
