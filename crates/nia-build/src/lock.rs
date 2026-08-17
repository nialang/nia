//! Cross-process ownership locks for stable logical build outputs.
//!
//! Equal destinations serialize through the same cache-rooted coordination key.
//! Owner PID, process start time, and acquisition sequence prevent stale or
//! older same-process guards from removing a live successor's lock.

#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::{
    fs,
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use nia_query::{FingerprintDomain, QueryFingerprintBuilder};

use crate::LogicalPath;

const STALE_AFTER: Duration = Duration::from_secs(15 * 60);
// pid, process generation, acquisition sequence, separators, and newline fit
// comfortably inside this protocol budget.
const MAX_LOCK_OWNER_BYTES: usize = 128;
#[cfg(target_os = "linux")]
const MAX_PROC_STAT_BYTES: usize = 4096;
const OUTPUT_LOCK_DOMAIN: FingerprintDomain = FingerprintDomain::new("nia.build.output-lock.v1");
static LOCK_SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[cfg(not(target_os = "linux"))]
static PROCESS_GENERATION: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcessIdentity {
    pub(crate) pid: u32,
    pub(crate) start_time: u64,
}

impl ProcessIdentity {
    pub(crate) fn current() -> Self {
        let pid = std::process::id();
        Self {
            pid,
            start_time: process_start_time(pid).unwrap_or(0),
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn is_alive(self) -> bool {
        if self.start_time == 0 {
            let Ok(pid) = i32::try_from(self.pid) else {
                return false;
            };
            let result = unsafe { libc::kill(pid, 0) };
            return result == 0
                || io::Error::last_os_error().kind() == io::ErrorKind::PermissionDenied;
        }
        process_start_time(self.pid) == Some(self.start_time)
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn is_alive(self) -> bool {
        true
    }
}

pub(crate) fn output_lock_path(cache_dir: &Path, output: &LogicalPath) -> PathBuf {
    let mut builder = QueryFingerprintBuilder::new(OUTPUT_LOCK_DOMAIN);
    builder.write_str(&output.protocol_path());
    let [first, second] = builder.finish().parts();
    cache_dir
        .join("coordination/output-locks")
        .join(format!("{first:016x}{second:016x}.lock"))
}

pub(crate) struct ScopedFileLock {
    path: PathBuf,
    token: String,
    _file: fs::File,
}

impl ScopedFileLock {
    #[cfg(test)]
    pub(crate) fn acquire(path: PathBuf) -> io::Result<Self> {
        Self::acquire_interruptible(path, || false)
            .map(|lock| lock.expect("an unset cancellation flag must allow lock acquisition"))
    }

    pub(crate) fn acquire_interruptible(
        path: PathBuf,
        mut is_cancelled: impl FnMut() -> bool,
    ) -> io::Result<Option<Self>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let start = Instant::now();
        let mut sleep = Duration::from_millis(10);
        loop {
            if is_cancelled() {
                return Ok(None);
            }
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => {
                    let (token, file) = match write_lock_owner(file) {
                        Ok(owner) => owner,
                        Err(error) => {
                            let _ = fs::remove_file(&path);
                            return Err(error);
                        }
                    };
                    return Ok(Some(Self {
                        path,
                        token,
                        _file: file,
                    }));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    reclaim_stale_lock(&path, STALE_AFTER);
                }
                Err(error) => return Err(error),
            }
            if start.elapsed() >= STALE_AFTER {
                reclaim_stale_lock(&path, Duration::ZERO);
            }
            if is_cancelled() {
                return Ok(None);
            }
            thread::sleep(sleep);
            sleep = (sleep * 2).min(Duration::from_millis(250));
        }
    }
}

impl Drop for ScopedFileLock {
    fn drop(&mut self) {
        if read_bounded_utf8(&self.path, MAX_LOCK_OWNER_BYTES)
            .is_some_and(|current| current.trim_end() == self.token)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn write_lock_owner(mut file: fs::File) -> io::Result<(String, fs::File)> {
    let identity = ProcessIdentity::current();
    let sequence = LOCK_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let token = format!("{}:{}:{sequence}", identity.pid, identity.start_time);
    writeln!(file, "{token}")?;
    file.sync_all()?;
    #[cfg(unix)]
    // A reclaimer may briefly hold the inode lock between `create_new` and
    // this point. Wait for it instead of treating that harmless race as a
    // publication failure; the owner token is already visible and proves the
    // creator is alive, so the reclaimer will leave the path in place.
    lock_file(&file)?;
    Ok((token, file))
}

fn reclaim_stale_lock(path: &Path, stale_after: Duration) {
    #[cfg(unix)]
    {
        // The owner keeps an advisory lock on the inode for its whole scope.
        // A stale reclaimer must acquire that same lock before unlinking, so a
        // live owner or a competing reclaimer can never be mistaken for stale.
        let Ok(file) = fs::OpenOptions::new().read(true).write(true).open(path) else {
            return;
        };
        let Ok(true) = try_lock_file(&file) else {
            return;
        };
        let stale = match read_lock_owner(path) {
            Some(_) => !lock_owner_is_alive(path),
            None => lock_is_stale_by_age(path, stale_after),
        };
        if stale {
            let _ = fs::remove_file(path);
        }
    }

    #[cfg(not(unix))]
    {
        if lock_owner_is_alive(path) {
            return;
        }
        if read_lock_owner(path).is_none() && !lock_is_stale_by_age(path, stale_after) {
            return;
        }
        let _ = fs::remove_file(path);
    }
}

#[cfg(unix)]
fn lock_file(file: &fs::File) -> io::Result<()> {
    // SAFETY: `file` owns a live descriptor for the duration of the call, and
    // `flock` neither takes ownership nor retains the descriptor pointer.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn try_lock_file(file: &fs::File) -> io::Result<bool> {
    // SAFETY: `file` owns a live descriptor for the duration of the call, and
    // `flock` neither takes ownership nor retains the descriptor pointer.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if matches!(
        error.raw_os_error(),
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN
    ) {
        return Ok(false);
    }
    Err(error)
}

fn lock_is_stale_by_age(path: &Path, stale_after: Duration) -> bool {
    if stale_after == Duration::ZERO {
        return true;
    }
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age >= stale_after)
}

#[cfg(unix)]
fn lock_owner_is_alive(path: &Path) -> bool {
    let Some(identity) = read_lock_owner(path) else {
        return false;
    };
    identity.is_alive()
}

#[cfg(not(unix))]
fn lock_owner_is_alive(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < STALE_AFTER)
}

fn read_lock_owner(path: &Path) -> Option<ProcessIdentity> {
    let owner = read_bounded_utf8(path, MAX_LOCK_OWNER_BYTES)?;
    let token = owner.split_whitespace().next()?;
    let mut parts = token.split(':');
    Some(ProcessIdentity {
        pid: parts.next()?.parse().ok()?,
        start_time: parts.next()?.parse().ok()?,
    })
}

/// Reads small coordination records with a stream-enforced `max + 1` budget.
/// Metadata is deliberately not trusted: a file that grows after opening is
/// still rejected without allocating in proportion to its contents.
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

#[cfg(target_os = "linux")]
fn process_start_time(pid: u32) -> Option<u64> {
    let stat = read_bounded_utf8(
        &Path::new("/proc").join(pid.to_string()).join("stat"),
        MAX_PROC_STAT_BYTES,
    )?;
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

#[cfg(not(target_os = "linux"))]
fn process_start_time(_pid: u32) -> Option<u64> {
    // Platforms without a process start-time API still need one stable
    // per-process generation so PID reuse cannot collide with persisted names.
    Some(*PROCESS_GENERATION.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos() as u64
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn test_root(name: &str) -> PathBuf {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "nia-build-lock-{name}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn matching_lock_paths_serialize() {
        let path = test_root("serialize").join("output.lock");
        let first = ScopedFileLock::acquire(path.clone()).unwrap();
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let second = ScopedFileLock::acquire(path).unwrap();
            acquired_tx.send(()).unwrap();
            drop(second);
        });

        assert!(
            acquired_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
        drop(first);
        acquired_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn distinct_lock_paths_do_not_serialize() {
        let root = test_root("distinct");
        let first = ScopedFileLock::acquire(root.join("first.lock")).unwrap();
        let second = ScopedFileLock::acquire(root.join("second.lock")).unwrap();

        assert_ne!(first.token, second.token);
        drop(second);
        drop(first);
    }

    #[test]
    fn cancelled_lock_wait_returns_without_acquiring() {
        let path = test_root("cancelled").join("output.lock");
        let held = ScopedFileLock::acquire(path.clone()).unwrap();
        let cancellation = std::sync::Arc::new(AtomicBool::new(false));
        let worker_cancellation = std::sync::Arc::clone(&cancellation);
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let result = ScopedFileLock::acquire_interruptible(path, || {
                worker_cancellation.load(Ordering::Acquire)
            });
            finished_tx.send(result.map(|lock| lock.is_none())).unwrap();
        });

        cancellation.store(true, Ordering::Release);

        assert!(
            finished_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap()
        );
        handle.join().unwrap();
        drop(held);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn dead_owner_is_reclaimed_without_age_delay() {
        let path = test_root("stale").join("output.lock");
        let pid = std::process::id();
        let start_time = process_start_time(pid).unwrap();
        fs::write(&path, format!("{pid}:{}\n", start_time + 1)).unwrap();

        let lock = ScopedFileLock::acquire(path.clone()).unwrap();

        assert!(path.is_file());
        drop(lock);
        assert!(!path.exists());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn reclaiming_a_live_lock_preserves_the_canonical_path() {
        let path = test_root("live-reclaim").join("output.lock");
        let lock = ScopedFileLock::acquire(path.clone()).unwrap();
        let token = lock.token.clone();

        reclaim_stale_lock(&path, Duration::ZERO);

        assert_eq!(fs::read_to_string(&path).unwrap().trim(), token);
        drop(lock);
        assert!(!path.exists());
    }

    #[test]
    #[cfg(unix)]
    fn oversized_owner_record_is_never_parsed_from_its_valid_prefix() {
        let path = test_root("oversized-owner").join("output.lock");
        let identity = ProcessIdentity::current();
        let mut owner = format!("{}:{}:0\n", identity.pid, identity.start_time).into_bytes();
        owner.resize(MAX_LOCK_OWNER_BYTES + 1, b'x');
        fs::write(&path, owner).unwrap();

        assert_eq!(read_lock_owner(&path), None);
        reclaim_stale_lock(&path, Duration::ZERO);
        assert!(!path.exists());
    }

    #[test]
    fn drop_does_not_remove_an_oversized_replacement_record() {
        let path = test_root("oversized-replacement").join("output.lock");
        let lock = ScopedFileLock::acquire(path.clone()).unwrap();
        fs::write(&path, vec![b'x'; MAX_LOCK_OWNER_BYTES + 1]).unwrap();

        drop(lock);

        assert!(path.exists());
        fs::remove_file(path).unwrap();
    }
}
