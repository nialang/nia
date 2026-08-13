// SPDX-License-Identifier: GPL-3.0-or-later
//! Resource-accounted process helpers for ordinary libtest tests.
//!
//! Libtest remains responsible for choosing and scheduling test threads. This
//! crate coordinates the expensive child processes those threads launch across
//! every test binary in one workspace: compiler and runtime pools bound their
//! distinct CPU/process pressure, while a shared memory-token pool bounds their
//! combined resident-memory estimate. Directory-backed permits extend the same
//! contract across concurrently running Cargo test processes.
mod cases;

pub use cases::{CaseManifest, case_directories, copy_case_tree, fixture_relative_path};

use std::{
    cell::Cell,
    ffi::OsStr,
    fs,
    io::{self, Read},
    marker::PhantomData,
    ops::Deref,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Output, Stdio},
    rc::Rc,
    sync::{
        Condvar, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const MAX_PARALLEL_COMPILERS: usize = 8;
const MAX_PARALLEL_RUNTIMES: usize = 32;
const UNKNOWN_MEMORY_PARALLEL_COMPILERS: usize = 1;
const UNKNOWN_MEMORY_PARALLEL_RUNTIMES: usize = 1;
const COMPILER_MEMORY_BYTES: usize = 1536 * 1024 * 1024;
const RUNTIME_MEMORY_BYTES: usize = 256 * 1024 * 1024;
const AVAILABLE_MEMORY_HEADROOM_BYTES: usize = 512 * 1024 * 1024;
const PERMIT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const UNKNOWN_OWNER_STALE_AFTER: Duration = Duration::from_secs(2 * 60 * 60);
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(420);

static COMPILER_POOL: OnceLock<ResourcePool> = OnceLock::new();
static RUNTIME_POOL: OnceLock<ResourcePool> = OnceLock::new();
static MEMORY_POOL: OnceLock<ResourcePool> = OnceLock::new();
static TEST_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    // Explicit test-wide sessions already reserve the process budget for all
    // commands issued by that test. Command helpers must not acquire a second
    // permit while such a session is active on the same libtest worker.
    static ACTIVE_TEST_RESOURCE_SESSIONS: Cell<usize> = const { Cell::new(0) };
}

pub struct TestDir {
    path: PathBuf,
}

impl Deref for TestDir {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl AsRef<Path> for TestDir {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<OsStr> for TestDir {
    fn as_ref(&self) -> &OsStr {
        self.path.as_os_str()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub fn test_dir(name: &str) -> TestDir {
    let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "nia-test-{name}-{}-{:?}-{id}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path)
        .unwrap_or_else(|error| panic!("create test directory {}: {error}", path.display()));
    TestDir { path }
}

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
            Self::Compiler => ResourceRequest::new(1, COMPILER_MEMORY_BYTES),
            Self::Build => ResourceRequest::new(2, COMPILER_MEMORY_BYTES),
        }
    }
}

pub fn acquire_test_resources(workload: TestWorkload) -> TestResourceSession<'static> {
    acquire_resources(compiler_pool(), workload.resource_request()).register_for_current_thread()
}

pub trait CommandExt {
    fn output_timeout_for_compiler(&mut self, context: &str) -> Output;
    fn output_timeout_for_build(&mut self, context: &str) -> Output;
    fn output_timeout_for_runtime(&mut self, context: &str) -> Output;
    fn output_timeout_in_session(&mut self, context: &str) -> Output;
}

pub trait CommandStatusExt {
    fn status_timeout(&mut self, context: &str) -> ExitStatus;
}

impl CommandExt for Command {
    fn output_timeout_for_compiler(&mut self, context: &str) -> Output {
        let _resources = acquire_command_resources(TestWorkload::Compiler);
        output_timeout_inner(self, context)
    }

    fn output_timeout_for_build(&mut self, context: &str) -> Output {
        let _resources = acquire_command_resources(TestWorkload::Build);
        output_timeout_inner(self, context)
    }

    fn output_timeout_for_runtime(&mut self, context: &str) -> Output {
        let _resources = acquire_runtime_resources();
        output_timeout_inner(self, context)
    }

    fn output_timeout_in_session(&mut self, context: &str) -> Output {
        assert!(
            ACTIVE_TEST_RESOURCE_SESSIONS.with(Cell::get) != 0,
            "{context}: command requires an active test resource session"
        );
        output_timeout_inner(self, context)
    }
}

fn output_timeout_inner(command: &mut Command, context: &str) -> Output {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    prepare_command(command);

    let timeout = DEFAULT_COMMAND_TIMEOUT;
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
        let _resources = acquire_runtime_resources();
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

fn acquire_runtime_resources() -> Option<TestResourceSession<'static>> {
    if ACTIVE_TEST_RESOURCE_SESSIONS.with(Cell::get) != 0 {
        return None;
    }
    Some(acquire_resources(
        runtime_pool(),
        ResourceRequest::runtime(),
    ))
}

fn acquire_command_resources(workload: TestWorkload) -> Option<TestResourceSession<'static>> {
    if ACTIVE_TEST_RESOURCE_SESSIONS.with(Cell::get) != 0 {
        return None;
    }
    Some(acquire_test_resources(workload))
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
    minimum_memory_bytes: usize,
}

impl ResourceRequest {
    const fn new(slots: usize, minimum_memory_bytes: usize) -> Self {
        Self {
            slots,
            minimum_memory_bytes,
        }
    }

    const fn runtime() -> Self {
        Self {
            slots: 1,
            minimum_memory_bytes: RUNTIME_MEMORY_BYTES,
        }
    }
}

fn compiler_pool() -> &'static ResourcePool {
    COMPILER_POOL.get_or_init(|| {
        let available_cpus = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1);
        let memory_limit = nia_query::effective_memory_limit_bytes();
        ResourcePool::with_memory_gate(
            compiler_limit(available_cpus, memory_limit),
            resource_slot_root().join("compiler"),
            None,
        )
    })
}

fn runtime_pool() -> &'static ResourcePool {
    RUNTIME_POOL.get_or_init(|| {
        let available_cpus = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1);
        let memory_limit = nia_query::effective_memory_limit_bytes();
        ResourcePool::with_memory_gate(
            runtime_limit(available_cpus, memory_limit),
            resource_slot_root().join("runtime"),
            None,
        )
    })
}

fn memory_pool() -> &'static ResourcePool {
    MEMORY_POOL.get_or_init(|| {
        let memory_limit = nia_query::effective_memory_limit_bytes();
        ResourcePool::with_memory_gate(
            memory_token_capacity(memory_limit),
            resource_slot_root().join("memory"),
            memory_limit,
        )
    })
}

fn acquire_resources(
    scheduling_pool: &'static ResourcePool,
    request: ResourceRequest,
) -> TestResourceSession<'static> {
    // Memory is reserved before a scheduling slot so queued work cannot pass
    // the shared budget merely because compiler and runtime pools are distinct.
    let memory = memory_pool().acquire(
        memory_tokens(request.minimum_memory_bytes),
        request.minimum_memory_bytes,
    );
    let scheduling = scheduling_pool.acquire(request.slots, 0);
    TestResourceSession::new([memory, scheduling])
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

fn runtime_limit(available_cpus: usize, system_memory: Option<usize>) -> usize {
    let cpu_limit = available_cpus.clamp(1, MAX_PARALLEL_RUNTIMES);
    let memory_limit = system_memory
        .map(|total| {
            test_memory_budget(total)
                .checked_div(RUNTIME_MEMORY_BYTES)
                .unwrap_or(0)
                .max(1)
        })
        .unwrap_or(UNKNOWN_MEMORY_PARALLEL_RUNTIMES);
    cpu_limit.min(memory_limit).max(1)
}

fn memory_token_capacity(system_memory: Option<usize>) -> usize {
    system_memory
        .map(|total| test_memory_budget(total) / RUNTIME_MEMORY_BYTES)
        .unwrap_or(1)
        .max(1)
}

fn memory_tokens(bytes: usize) -> usize {
    bytes
        .saturating_add(RUNTIME_MEMORY_BYTES - 1)
        .checked_div(RUNTIME_MEMORY_BYTES)
        .unwrap_or(usize::MAX)
        .max(1)
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

    fn acquire(
        &self,
        requested_slots: usize,
        minimum_memory_bytes: usize,
    ) -> ResourceReservation<'_> {
        let reserved_slots = requested_slots.clamp(1, self.capacity);
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
            .map(|limit| minimum_available_memory(limit, minimum_memory_bytes));
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
        ResourceReservation {
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

struct ResourceReservation<'a> {
    pool: &'a ResourcePool,
    reserved_slots: usize,
    slots: Vec<PathBuf>,
}

impl Drop for ResourceReservation<'_> {
    fn drop(&mut self) {
        for slot in self.slots.drain(..) {
            let _ = fs::remove_dir_all(slot);
        }
        self.pool.release(self.reserved_slots);
    }
}

pub struct TestResourceSession<'a> {
    reservations: Vec<ResourceReservation<'a>>,
    registered_with_thread: bool,
    // The active-session marker is thread-local, so its guard must stay on the
    // libtest worker that acquired it.
    not_send: PhantomData<Rc<()>>,
}

impl TestResourceSession<'_> {
    fn new<const N: usize>(reservations: [ResourceReservation<'_>; N]) -> TestResourceSession<'_> {
        TestResourceSession {
            reservations: Vec::from(reservations),
            registered_with_thread: false,
            not_send: PhantomData,
        }
    }

    fn register_for_current_thread(mut self) -> Self {
        ACTIVE_TEST_RESOURCE_SESSIONS.with(|active| active.set(active.get() + 1));
        self.registered_with_thread = true;
        self
    }
}

impl Drop for TestResourceSession<'_> {
    fn drop(&mut self) {
        if self.registered_with_thread {
            ACTIVE_TEST_RESOURCE_SESSIONS.with(|active| {
                let count = active.get();
                debug_assert!(count != 0, "registered test resource session underflow");
                active.set(count.saturating_sub(1));
            });
        }
        self.reservations.clear();
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn resource_slot_root() -> PathBuf {
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

fn minimum_available_memory(memory_limit: usize, workload_memory_bytes: usize) -> usize {
    let workload_memory = workload_memory_bytes.saturating_add(AVAILABLE_MEMORY_HEADROOM_BYTES);
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
    fn runtime_limit_uses_cpu_and_lightweight_memory_budget() {
        assert_eq!(runtime_limit(32, Some(8 * 1024 * 1024 * 1024)), 16);
        assert_eq!(runtime_limit(8, Some(8 * 1024 * 1024 * 1024)), 8);
        assert_eq!(runtime_limit(32, Some(1024 * 1024 * 1024)), 2);
        assert_eq!(runtime_limit(32, None), UNKNOWN_MEMORY_PARALLEL_RUNTIMES);
    }

    #[test]
    fn shared_memory_tokens_bound_mixed_workloads() {
        assert_eq!(memory_token_capacity(Some(8 * 1024 * 1024 * 1024)), 16);
        assert_eq!(memory_tokens(COMPILER_MEMORY_BYTES), 6);
        assert_eq!(memory_tokens(RUNTIME_MEMORY_BYTES), 1);
        // Two compilers may overlap with four runtimes inside the 4 GiB test
        // budget, but a third compiler cannot enter concurrently.
        assert_eq!(2 * memory_tokens(COMPILER_MEMORY_BYTES) + 4, 16);
        assert!(3 * memory_tokens(COMPILER_MEMORY_BYTES) > 16);
    }

    #[test]
    fn test_directories_are_removed_when_the_owner_drops() {
        let directory = test_dir("scoped-directory");
        let path = directory.to_path_buf();
        fs::write(directory.join("owned"), b"test").expect("write test directory fixture");
        drop(directory);
        assert!(!path.exists());
    }

    #[test]
    fn compiler_limit_always_allows_progress() {
        assert_eq!(compiler_limit(0, Some(0)), 1);
        assert_eq!(compiler_limit(16, None), UNKNOWN_MEMORY_PARALLEL_COMPILERS);
    }

    #[test]
    fn available_memory_gate_scales_down_on_small_hosts() {
        assert_eq!(
            minimum_available_memory(8 * 1024 * 1024 * 1024, 2 * COMPILER_MEMORY_BYTES),
            7 * 512 * 1024 * 1024
        );
        assert_eq!(
            minimum_available_memory(3 * 1024 * 1024 * 1024, COMPILER_MEMORY_BYTES),
            3 * 512 * 1024 * 1024
        );
        assert_eq!(
            minimum_available_memory(8 * 1024 * 1024 * 1024, RUNTIME_MEMORY_BYTES),
            RUNTIME_MEMORY_BYTES + AVAILABLE_MEMORY_HEADROOM_BYTES
        );
    }

    #[test]
    fn explicit_sessions_cover_nested_command_helpers() {
        let root = test_slot_root("explicit-session-nesting");
        let pool = ResourcePool::new(2, root.to_path_buf());
        let session = TestResourceSession::new([pool.acquire(1, 0)]).register_for_current_thread();

        assert!(acquire_runtime_resources().is_none());
        assert!(acquire_command_resources(TestWorkload::Build).is_none());
        drop(session);
        ACTIVE_TEST_RESOURCE_SESSIONS.with(|active| assert_eq!(active.get(), 0));
    }

    #[test]
    fn command_scoped_sessions_do_not_clear_explicit_session_markers() {
        let root = test_slot_root("command-session-markers");
        let pool = ResourcePool::new(2, root.to_path_buf());
        let explicit = TestResourceSession::new([pool.acquire(1, 0)]).register_for_current_thread();
        let command = TestResourceSession::new([pool.acquire(1, 0)]);

        drop(command);
        ACTIVE_TEST_RESOURCE_SESSIONS.with(|active| assert_eq!(active.get(), 1));
        drop(explicit);
        ACTIVE_TEST_RESOURCE_SESSIONS.with(|active| assert_eq!(active.get(), 0));
    }

    #[test]
    fn weighted_sessions_return_their_full_capacity() {
        let root = test_slot_root("weighted-sessions");
        let pool = ResourcePool::new(4, root.to_path_buf());
        let session = pool.acquire(2, 0);
        assert_eq!(*lock_unpoisoned(&pool.available), 2);
        drop(session);
        assert_eq!(*lock_unpoisoned(&pool.available), 4);
    }

    #[test]
    fn scheduling_weight_does_not_inflate_memory_requirement() {
        let build = ResourceRequest::new(2, COMPILER_MEMORY_BYTES);
        assert_eq!(
            minimum_available_memory(8 * 1024 * 1024 * 1024, build.minimum_memory_bytes),
            2 * 1024 * 1024 * 1024
        );
    }

    #[test]
    fn process_slots_coordinate_independent_pools() {
        let root = test_slot_root("cross-process-slots");
        let first = ResourcePool::new(2, root.to_path_buf());
        let second = ResourcePool::new(2, root.to_path_buf());
        let session = first.acquire(2, 0);
        assert!(second.slot_root.join("0").is_dir());
        assert!(second.slot_root.join("1").is_dir());
        drop(session);
        let second_session = second.acquire(2, 0);
        drop(second_session);
    }

    #[test]
    fn process_slots_block_independent_pools_until_release() {
        let root = test_slot_root("cross-process-blocking");
        let first = ResourcePool::new(1, root.to_path_buf());
        let waiter_root = root.to_path_buf();
        let first_session = first.acquire(1, 0);
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let second = ResourcePool::new(1, waiter_root);
            let second_session = second.acquire(1, 0);
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

    #[test]
    fn shared_memory_slots_block_distinct_schedulers() {
        let root = test_slot_root("shared-memory-slots");
        let compiler = ResourcePool::new(2, root.join("compiler"));
        let runtime = ResourcePool::new(2, root.join("runtime"));
        let memory_owner = ResourcePool::new(2, root.join("memory"));
        let memory_waiter = ResourcePool::new(2, root.join("memory"));
        let compiler_session =
            TestResourceSession::new([memory_owner.acquire(2, 0), compiler.acquire(1, 0)]);

        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let runtime_session =
                TestResourceSession::new([memory_waiter.acquire(1, 0), runtime.acquire(1, 0)]);
            acquired_tx.send(()).expect("report runtime acquisition");
            drop(runtime_session);
        });

        assert!(
            acquired_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "runtime scheduler bypassed shared memory capacity"
        );
        drop(compiler_session);
        acquired_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("runtime should acquire released memory capacity");
        waiter.join().expect("shared memory waiter");
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

    fn test_slot_root(name: &str) -> TestDir {
        test_dir(&format!("test-support-{name}"))
    }
}
