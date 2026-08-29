// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    cell::RefCell,
    marker::PhantomData,
    rc::Rc,
    sync::{
        Condvar, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

#[cfg(target_os = "linux")]
use std::{fs, io::Read as _};

#[cfg(target_os = "linux")]
use std::path::{Component, Path};

const MAX_PARALLEL_LLVM_TASKS: usize = 4;
const LLVM_TASK_MEMORY_BYTES: usize = 1536 * 1024 * 1024;
const LLVM_MEMORY_HEADROOM_BYTES: usize = 512 * 1024 * 1024;
// Kernel pseudo-files are tiny protocols. These budgets leave generous room
// for large machines and deeply nested containers without trusting a growing
// procfs/cgroupfs stream to choose the allocation size.
#[cfg(target_os = "linux")]
const MAX_MEMINFO_BYTES: usize = 1024 * 1024;
#[cfg(target_os = "linux")]
const MAX_CGROUP_MEMBERSHIP_BYTES: usize = 64 * 1024;
#[cfg(target_os = "linux")]
const MAX_CGROUP_SCALAR_BYTES: usize = 128;

static LLVM_MEMORY_BUDGET: OnceLock<MemoryBudget> = OnceLock::new();

thread_local! {
    static MEMORY_BUDGET_DEPTHS: RefCell<Vec<(usize, usize)>> = const { RefCell::new(Vec::new()) };
}

struct MemoryBudget {
    capacity: usize,
    minimum_available_bytes: Option<usize>,
    state: Mutex<MemoryBudgetState>,
    ready: Condvar,
    peak_active: AtomicUsize,
}

#[derive(Default)]
struct MemoryBudgetState {
    active: usize,
}

/// Non-`Send` lease for one process-wide LLVM memory slot.
///
/// Nested acquisition on the same thread is reentrant; only the outer lease
/// consumes capacity and participates in the memory-pressure wait.
pub struct ProcessMemoryPermit<'a> {
    budget: &'a MemoryBudget,
    waited: bool,
    _not_send: PhantomData<Rc<()>>,
}

/// Waits for and acquires one process-wide LLVM memory permit.
pub fn acquire_llvm_memory_permit() -> ProcessMemoryPermit<'static> {
    llvm_memory_budget().acquire()
}

/// Returns the configured process-wide LLVM task capacity.
pub fn llvm_memory_task_capacity() -> usize {
    llvm_memory_budget().capacity
}

/// Returns the lower of system and cgroup memory limits when observable.
pub fn effective_memory_limit_bytes() -> Option<usize> {
    [system_memory_bytes(), cgroup_memory_limit_bytes()]
        .into_iter()
        .flatten()
        .min()
}

/// Returns the lower of system and cgroup available-memory estimates.
pub fn effective_available_memory_bytes() -> Option<usize> {
    [
        system_available_memory_bytes(),
        cgroup_available_memory_bytes(),
    ]
    .into_iter()
    .flatten()
    .min()
}

impl ProcessMemoryPermit<'_> {
    /// Reports whether acquiring this lease waited for capacity or memory pressure.
    pub fn waited(&self) -> bool {
        self.waited
    }
}

impl Drop for ProcessMemoryPermit<'_> {
    fn drop(&mut self) {
        if !leave_memory_budget(self.budget.identity()) {
            return;
        }
        let mut state = lock_unpoisoned(&self.budget.state);
        state.active = state
            .active
            .checked_sub(1)
            .expect("process memory budget active count underflow");
        drop(state);
        self.budget.ready.notify_all();
    }
}

impl MemoryBudget {
    fn new(capacity: usize, minimum_available_bytes: Option<usize>) -> Self {
        assert!(capacity > 0, "memory budget capacity must be non-zero");
        Self {
            capacity,
            minimum_available_bytes,
            state: Mutex::new(MemoryBudgetState::default()),
            ready: Condvar::new(),
            peak_active: AtomicUsize::new(0),
        }
    }

    fn acquire(&self) -> ProcessMemoryPermit<'_> {
        let identity = self.identity();
        let nested = memory_budget_is_active(identity);
        let mut waited = false;
        if !nested {
            let mut state = lock_unpoisoned(&self.state);
            while state.active >= self.capacity || !self.memory_pressure_allows(state.active) {
                waited = true;
                state = self
                    .ready
                    .wait(state)
                    .unwrap_or_else(|error| error.into_inner());
            }
            state.active += 1;
            self.peak_active.fetch_max(state.active, Ordering::Relaxed);
        }
        enter_memory_budget(identity);
        ProcessMemoryPermit {
            budget: self,
            waited,
            _not_send: PhantomData,
        }
    }

    fn memory_pressure_allows(&self, active: usize) -> bool {
        active == 0
            || self.minimum_available_bytes.is_none_or(|minimum| {
                effective_available_memory_bytes().is_none_or(|available| available >= minimum)
            })
    }

    fn identity(&self) -> usize {
        self as *const Self as usize
    }

    #[cfg(test)]
    fn peak_active(&self) -> usize {
        self.peak_active.load(Ordering::Relaxed)
    }
}

fn memory_budget_is_active(identity: usize) -> bool {
    MEMORY_BUDGET_DEPTHS.with(|depths| {
        depths
            .borrow()
            .iter()
            .any(|(budget, _depth)| *budget == identity)
    })
}

fn enter_memory_budget(identity: usize) {
    MEMORY_BUDGET_DEPTHS.with(|depths| {
        let mut depths = depths.borrow_mut();
        if let Some((_, depth)) = depths
            .iter_mut()
            .find(|(budget, _depth)| *budget == identity)
        {
            *depth += 1;
        } else {
            depths.push((identity, 1));
        }
    });
}

fn leave_memory_budget(identity: usize) -> bool {
    MEMORY_BUDGET_DEPTHS.with(|depths| {
        let mut depths = depths.borrow_mut();
        let position = depths
            .iter()
            .position(|(budget, _depth)| *budget == identity)
            .expect("process memory budget permit dropped without an active depth");
        let depth = &mut depths[position].1;
        *depth = depth
            .checked_sub(1)
            .expect("process memory budget depth underflow");
        if *depth == 0 {
            depths.swap_remove(position);
            true
        } else {
            false
        }
    })
}

fn llvm_memory_budget() -> &'static MemoryBudget {
    LLVM_MEMORY_BUDGET.get_or_init(|| {
        MemoryBudget::new(
            memory_task_capacity(
                std::thread::available_parallelism()
                    .map(usize::from)
                    .unwrap_or(1),
                effective_memory_limit_bytes(),
            ),
            Some(LLVM_TASK_MEMORY_BYTES.saturating_add(LLVM_MEMORY_HEADROOM_BYTES)),
        )
    })
}

fn memory_task_capacity(available_cpus: usize, memory_limit: Option<usize>) -> usize {
    let cpu_capacity = available_cpus.clamp(1, MAX_PARALLEL_LLVM_TASKS);
    let memory_capacity = memory_limit
        .map(|limit| {
            (limit / 2)
                .checked_div(LLVM_TASK_MEMORY_BYTES)
                .unwrap_or(0)
                .max(1)
        })
        .unwrap_or(1);
    cpu_capacity.min(memory_capacity).max(1)
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
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
    let meminfo = read_bounded_utf8(Path::new("/proc/meminfo"), MAX_MEMINFO_BYTES)?;
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
    let cgroup = read_bounded_utf8(Path::new("/proc/self/cgroup"), MAX_CGROUP_MEMBERSHIP_BYTES)?;
    if let Some(directory) = cgroup_v2_directory(&cgroup) {
        let mount = Path::new("/sys/fs/cgroup");
        return cgroup_v2_memory_limit(mount, &mount.join(directory));
    }
    let path = cgroup_v1_memory_directory(&cgroup)?;
    let value = read_bounded_utf8(
        &Path::new("/sys/fs/cgroup/memory")
            .join(path)
            .join("memory.limit_in_bytes"),
        MAX_CGROUP_SCALAR_BYTES,
    )?;
    parse_memory_limit(&value)
}

#[cfg(target_os = "linux")]
fn cgroup_available_memory_bytes() -> Option<usize> {
    let cgroup = read_bounded_utf8(Path::new("/proc/self/cgroup"), MAX_CGROUP_MEMBERSHIP_BYTES)?;
    if let Some(directory) = cgroup_v2_directory(&cgroup) {
        let mount = Path::new("/sys/fs/cgroup");
        return cgroup_v2_available_memory(mount, &mount.join(directory));
    }
    let path = cgroup_v1_memory_directory(&cgroup)?;
    let root = Path::new("/sys/fs/cgroup/memory").join(path);
    let limit = parse_memory_limit(&read_bounded_utf8(
        &root.join("memory.limit_in_bytes"),
        MAX_CGROUP_SCALAR_BYTES,
    )?)?;
    let current = read_bounded_utf8(&root.join("memory.usage_in_bytes"), MAX_CGROUP_SCALAR_BYTES)?
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
        .and_then(relative_cgroup_path)
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
            .then(|| relative_cgroup_path(path))?
    })
}

#[cfg(target_os = "linux")]
fn relative_cgroup_path(path: &str) -> Option<&Path> {
    let path = Path::new(path.trim_start_matches('/'));
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
        .then_some(path)
}

#[cfg(target_os = "linux")]
fn cgroup_v2_memory_limit(mount: &Path, leaf: &Path) -> Option<usize> {
    cgroup_v2_ancestors(mount, leaf)
        .filter_map(|directory| {
            parse_memory_limit(&read_bounded_utf8(
                &directory.join("memory.max"),
                MAX_CGROUP_SCALAR_BYTES,
            )?)
        })
        .min()
}

#[cfg(target_os = "linux")]
fn cgroup_v2_available_memory(mount: &Path, leaf: &Path) -> Option<usize> {
    cgroup_v2_ancestors(mount, leaf)
        .filter_map(|directory| {
            let limit = parse_memory_limit(&read_bounded_utf8(
                &directory.join("memory.max"),
                MAX_CGROUP_SCALAR_BYTES,
            )?)?;
            let current =
                read_bounded_utf8(&directory.join("memory.current"), MAX_CGROUP_SCALAR_BYTES)?
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
fn system_memory_bytes() -> Option<usize> {
    None
}

#[cfg(not(target_os = "linux"))]
fn system_available_memory_bytes() -> Option<usize> {
    None
}

#[cfg(not(target_os = "linux"))]
fn cgroup_memory_limit_bytes() -> Option<usize> {
    None
}

#[cfg(not(target_os = "linux"))]
fn cgroup_available_memory_bytes() -> Option<usize> {
    None
}

#[cfg(target_os = "linux")]
fn parse_memory_limit(value: &str) -> Option<usize> {
    let value = value.trim();
    (value != "max").then(|| value.parse().ok()).flatten()
}

#[cfg(target_os = "linux")]
fn read_bounded_utf8(path: &Path, max_bytes: usize) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(u64::try_from(max_bytes).ok()?.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > max_bytes {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn memory_task_capacity_reserves_half_of_visible_memory() {
        let large_limit =
            usize::try_from(8u64 * 1024 * 1024 * 1024).unwrap_or(3usize * 1024 * 1024 * 1024);
        let expected_large_capacity = if cfg!(target_pointer_width = "64") {
            2
        } else {
            1
        };
        assert_eq!(
            memory_task_capacity(32, Some(large_limit)),
            expected_large_capacity
        );
        assert_eq!(memory_task_capacity(32, Some(3 * 1024 * 1024 * 1024)), 1);
        assert_eq!(
            memory_task_capacity(2, Some(large_limit)),
            expected_large_capacity.min(2)
        );
        assert_eq!(memory_task_capacity(32, None), 1);
    }

    #[test]
    fn memory_budget_caps_parallel_heavy_tasks() {
        let budget = Arc::new(MemoryBudget::new(2, None));
        let barrier = Arc::new(Barrier::new(2));
        let mut workers = Vec::new();
        for _ in 0..4 {
            let budget = Arc::clone(&budget);
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                let _permit = budget.acquire();
                barrier.wait();
            }));
        }
        for worker in workers {
            worker.join().expect("memory budget worker");
        }

        assert_eq!(budget.peak_active(), 2);
    }

    #[test]
    fn nested_memory_tasks_reuse_the_current_permit() {
        let budget = MemoryBudget::new(1, None);
        let outer = budget.acquire();
        let inner = budget.acquire();

        assert!(!outer.waited());
        assert!(!inner.waited());
        assert_eq!(budget.peak_active(), 1);
        drop(outer);
        assert_eq!(lock_unpoisoned(&budget.state).active, 1);
        drop(inner);
        assert_eq!(lock_unpoisoned(&budget.state).active, 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_cgroup_memory_limits() {
        assert_eq!(parse_memory_limit("1073741824\n"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_memory_limit("max\n"), None);
        assert_eq!(parse_memory_limit("invalid\n"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cgroup_paths_reject_parent_components() {
        assert_eq!(
            cgroup_v2_directory("0::/parent/leaf\n"),
            Some(Path::new("parent/leaf"))
        );
        assert_eq!(cgroup_v2_directory("0::/../../outside\n"), None);
        assert_eq!(
            cgroup_v1_memory_directory("7:cpu,memory:/parent/leaf\n"),
            Some(Path::new("parent/leaf"))
        );
        assert_eq!(cgroup_v1_memory_directory("7:memory:/../outside\n"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bounded_resource_file_rejects_oversized_valid_prefix() {
        let root = test_root("oversized_resource_file");
        fs::create_dir_all(&root).expect("create resource fixture");
        let path = root.join("memory.max");
        let mut value = b"4096\n".to_vec();
        value.resize(MAX_CGROUP_SCALAR_BYTES + 1, b'0');
        fs::write(&path, value).expect("write oversized resource file");

        assert_eq!(read_bounded_utf8(&path, MAX_CGROUP_SCALAR_BYTES), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cgroup_v2_inherits_parent_memory_budget() {
        let root = test_root("cgroup_v2_parent");
        let leaf = root.join("parent/leaf");
        fs::create_dir_all(&leaf).expect("create cgroup fixture");
        fs::write(root.join("memory.max"), "max").expect("write root memory.max");
        fs::write(root.join("memory.current"), "0").expect("write root memory.current");
        fs::write(root.join("parent/memory.max"), "4096").expect("write parent memory.max");
        fs::write(root.join("parent/memory.current"), "1024").expect("write parent memory.current");
        fs::write(leaf.join("memory.max"), "max").expect("write leaf memory.max");
        fs::write(leaf.join("memory.current"), "512").expect("write leaf memory.current");

        assert_eq!(cgroup_v2_memory_limit(&root, &leaf), Some(4096));
        assert_eq!(cgroup_v2_available_memory(&root, &leaf), Some(3072));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cgroup_v2_uses_tightest_nested_memory_budget() {
        let root = test_root("cgroup_v2_nested");
        let leaf = root.join("parent/leaf");
        fs::create_dir_all(&leaf).expect("create cgroup fixture");
        fs::write(root.join("memory.max"), "8192").expect("write root memory.max");
        fs::write(root.join("memory.current"), "2048").expect("write root memory.current");
        fs::write(root.join("parent/memory.max"), "4096").expect("write parent memory.max");
        fs::write(root.join("parent/memory.current"), "1024").expect("write parent memory.current");
        fs::write(leaf.join("memory.max"), "2048").expect("write leaf memory.max");
        fs::write(leaf.join("memory.current"), "512").expect("write leaf memory.current");

        assert_eq!(cgroup_v2_memory_limit(&root, &leaf), Some(2048));
        assert_eq!(cgroup_v2_available_memory(&root, &leaf), Some(1536));
    }

    #[cfg(target_os = "linux")]
    fn test_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("nia-query-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }
}
