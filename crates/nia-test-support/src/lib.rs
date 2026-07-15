// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    fs, io,
    path::{Path, PathBuf},
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

static COMPILER_POOL: OnceLock<ResourcePool> = OnceLock::new();

pub fn compiler_permit() -> ResourcePermit<'static> {
    compiler_pool().acquire(ResourceRequest::new(1, 1))
}

pub fn build_permit() -> ResourcePermit<'static> {
    compiler_pool().acquire(ResourceRequest::new(2, 1))
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
            effective_memory_limit_bytes(),
        )
    })
}

fn parallel_compiler_limit() -> usize {
    let available_cpus = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    compiler_limit(available_cpus, effective_memory_limit_bytes())
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

    fn acquire(&self, request: ResourceRequest) -> ResourcePermit<'_> {
        let permits = request.slots.clamp(1, self.capacity);
        let mut available = lock_unpoisoned(&self.available);
        while *available < permits {
            available = self
                .ready
                .wait(available)
                .unwrap_or_else(|error| error.into_inner());
        }
        *available -= permits;
        drop(available);

        let minimum_available_memory = self
            .memory_limit
            .map(|limit| minimum_available_memory(limit, request.memory_units.max(1)));
        let slots = match acquire_process_slots(
            &self.slot_root,
            self.capacity,
            permits,
            minimum_available_memory,
        ) {
            Ok(slots) => slots,
            Err(error) => {
                self.release(permits);
                panic!(
                    "failed to acquire Nia test resource permits in {}: {error}",
                    self.slot_root.display()
                );
            }
        };
        ResourcePermit {
            pool: self,
            permits,
            slots,
        }
    }

    fn release(&self, permits: usize) {
        *lock_unpoisoned(&self.available) += permits;
        self.ready.notify_all();
    }
}

pub struct ResourcePermit<'a> {
    pool: &'a ResourcePool,
    permits: usize,
    slots: Vec<PathBuf>,
}

impl Drop for ResourcePermit<'_> {
    fn drop(&mut self) {
        for slot in self.slots.drain(..) {
            let _ = fs::remove_dir_all(slot);
        }
        self.pool.release(self.permits);
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

fn minimum_available_memory(memory_limit: usize, permits: usize) -> usize {
    let workload_memory = permits
        .saturating_mul(COMPILER_MEMORY_BYTES)
        .saturating_add(AVAILABLE_MEMORY_HEADROOM_BYTES);
    workload_memory.min(test_memory_budget(memory_limit))
}

fn memory_pressure_allows(minimum_available_memory: Option<usize>) -> bool {
    minimum_available_memory.is_none_or(|minimum| {
        effective_available_memory_bytes().is_none_or(|available| available >= minimum)
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

fn effective_memory_limit_bytes() -> Option<usize> {
    [system_memory_bytes(), cgroup_memory_limit_bytes()]
        .into_iter()
        .flatten()
        .min()
}

fn effective_available_memory_bytes() -> Option<usize> {
    [
        system_available_memory_bytes(),
        cgroup_available_memory_bytes(),
    ]
    .into_iter()
    .flatten()
    .min()
}

#[cfg(target_os = "linux")]
fn system_memory_bytes() -> Option<usize> {
    meminfo_bytes("MemTotal:")
}

#[cfg(target_os = "linux")]
fn system_available_memory_bytes() -> Option<usize> {
    meminfo_bytes("MemAvailable:")
}

#[cfg(target_os = "linux")]
fn meminfo_bytes(field: &str) -> Option<usize> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let value_kib = meminfo.lines().find_map(|line| {
        line.strip_prefix(field)?
            .split_whitespace()
            .next()?
            .parse::<usize>()
            .ok()
    })?;
    value_kib.checked_mul(1024)
}

#[cfg(target_os = "linux")]
fn cgroup_memory_limit_bytes() -> Option<usize> {
    let cgroup = fs::read_to_string("/proc/self/cgroup").ok()?;
    if let Some(directory) = cgroup_v2_directory(&cgroup) {
        let mount = Path::new("/sys/fs/cgroup");
        return cgroup_v2_memory_limit(mount, &mount.join(directory));
    }
    let path = cgroup_v1_memory_directory(&cgroup)?;
    let value = fs::read_to_string(
        Path::new("/sys/fs/cgroup/memory")
            .join(path)
            .join("memory.limit_in_bytes"),
    )
    .ok()?;
    parse_memory_limit(&value)
}

#[cfg(target_os = "linux")]
fn cgroup_available_memory_bytes() -> Option<usize> {
    let cgroup = fs::read_to_string("/proc/self/cgroup").ok()?;
    if let Some(directory) = cgroup_v2_directory(&cgroup) {
        let mount = Path::new("/sys/fs/cgroup");
        return cgroup_v2_available_memory(mount, &mount.join(directory));
    }
    let path = cgroup_v1_memory_directory(&cgroup)?;
    let root = Path::new("/sys/fs/cgroup/memory").join(path);
    let limit = parse_memory_limit(&fs::read_to_string(root.join("memory.limit_in_bytes")).ok()?)?;
    let current = fs::read_to_string(root.join("memory.usage_in_bytes"))
        .ok()?
        .trim()
        .parse::<usize>()
        .ok()?;
    Some(limit.saturating_sub(current))
}

#[cfg(target_os = "linux")]
fn cgroup_v2_directory(cgroup: &str) -> Option<&Path> {
    cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(|path| Path::new(path.trim_start_matches('/')))
}

#[cfg(target_os = "linux")]
fn cgroup_v1_memory_directory(cgroup: &str) -> Option<&Path> {
    cgroup.lines().find_map(|line| {
        let mut fields = line.splitn(3, ':');
        let _hierarchy = fields.next()?;
        let controllers = fields.next()?;
        let path = fields.next()?;
        controllers
            .split(',')
            .any(|item| item == "memory")
            .then(|| Path::new(path.trim_start_matches('/')))
    })
}

#[cfg(target_os = "linux")]
fn cgroup_v2_memory_limit(mount: &Path, leaf: &Path) -> Option<usize> {
    cgroup_v2_ancestors(mount, leaf)
        .filter_map(|directory| {
            parse_memory_limit(&fs::read_to_string(directory.join("memory.max")).ok()?)
        })
        .min()
}

#[cfg(target_os = "linux")]
fn cgroup_v2_available_memory(mount: &Path, leaf: &Path) -> Option<usize> {
    cgroup_v2_ancestors(mount, leaf)
        .filter_map(|directory| {
            let limit =
                parse_memory_limit(&fs::read_to_string(directory.join("memory.max")).ok()?)?;
            let current = fs::read_to_string(directory.join("memory.current"))
                .ok()?
                .trim()
                .parse::<usize>()
                .ok()?;
            Some(limit.saturating_sub(current))
        })
        .min()
}

#[cfg(target_os = "linux")]
fn cgroup_v2_ancestors<'a>(mount: &'a Path, leaf: &'a Path) -> impl Iterator<Item = &'a Path> {
    leaf.ancestors()
        .take_while(move |path| path.starts_with(mount))
}

#[cfg(not(target_os = "linux"))]
fn cgroup_memory_limit_bytes() -> Option<usize> {
    None
}

#[cfg(not(target_os = "linux"))]
fn cgroup_available_memory_bytes() -> Option<usize> {
    None
}

fn parse_memory_limit(value: &str) -> Option<usize> {
    let value = value.trim();
    (value != "max").then(|| value.parse().ok()).flatten()
}

#[cfg(not(target_os = "linux"))]
fn system_memory_bytes() -> Option<usize> {
    None
}

#[cfg(not(target_os = "linux"))]
fn system_available_memory_bytes() -> Option<usize> {
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
    fn weighted_permits_return_their_full_capacity() {
        let pool = ResourcePool::new(4, test_slot_root("weighted_permits"));
        let permit = pool.acquire(ResourceRequest::new(2, 1));
        assert_eq!(*lock_unpoisoned(&pool.available), 2);
        drop(permit);
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
        let permit = first.acquire(ResourceRequest::new(2, 1));
        assert!(second.slot_root.join("0").is_dir());
        assert!(second.slot_root.join("1").is_dir());
        drop(permit);
        let second_permit = second.acquire(ResourceRequest::new(2, 1));
        drop(second_permit);
    }

    #[test]
    fn process_slots_block_independent_pools_until_release() {
        let root = test_slot_root("cross_process_blocking");
        let first = ResourcePool::new(1, root.clone());
        let first_permit = first.acquire(ResourceRequest::new(1, 1));
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let second = ResourcePool::new(1, root);
            let second_permit = second.acquire(ResourceRequest::new(1, 1));
            acquired_tx.send(()).expect("report acquired process slot");
            drop(second_permit);
        });

        assert!(
            acquired_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "independent pool acquired an occupied process slot"
        );
        drop(first_permit);
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

    #[test]
    fn parses_cgroup_memory_limits() {
        assert_eq!(parse_memory_limit("1073741824\n"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_memory_limit("max\n"), None);
        assert_eq!(parse_memory_limit("invalid\n"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cgroup_v2_inherits_parent_memory_budget() {
        let mount = test_slot_root("cgroup_v2_parent");
        let leaf = mount.join("parent/leaf");
        fs::create_dir_all(&leaf).expect("create cgroup fixture");
        fs::write(mount.join("memory.max"), "max").expect("write root memory.max");
        fs::write(mount.join("memory.current"), "0").expect("write root memory.current");
        fs::write(mount.join("parent/memory.max"), "4096").expect("write parent memory.max");
        fs::write(mount.join("parent/memory.current"), "1024")
            .expect("write parent memory.current");
        fs::write(leaf.join("memory.max"), "max").expect("write leaf memory.max");
        fs::write(leaf.join("memory.current"), "512").expect("write leaf memory.current");

        assert_eq!(cgroup_v2_memory_limit(&mount, &leaf), Some(4096));
        assert_eq!(cgroup_v2_available_memory(&mount, &leaf), Some(3072));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cgroup_v2_uses_tightest_nested_memory_budget() {
        let mount = test_slot_root("cgroup_v2_nested");
        let leaf = mount.join("parent/leaf");
        fs::create_dir_all(&leaf).expect("create cgroup fixture");
        fs::write(mount.join("memory.max"), "8192").expect("write root memory.max");
        fs::write(mount.join("memory.current"), "2048").expect("write root memory.current");
        fs::write(mount.join("parent/memory.max"), "4096").expect("write parent memory.max");
        fs::write(mount.join("parent/memory.current"), "1024")
            .expect("write parent memory.current");
        fs::write(leaf.join("memory.max"), "2048").expect("write leaf memory.max");
        fs::write(leaf.join("memory.current"), "512").expect("write leaf memory.current");

        assert_eq!(cgroup_v2_memory_limit(&mount, &leaf), Some(2048));
        assert_eq!(cgroup_v2_available_memory(&mount, &leaf), Some(1536));
    }

    fn test_slot_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("nia-test-support-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }
}
