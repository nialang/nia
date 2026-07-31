use std::{
    fs, io,
    io::Write as _,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use nia_query::QueryFingerprintBuilder;

use crate::LogicalPath;

const STALE_AFTER: Duration = Duration::from_secs(15 * 60);
static LOCK_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
    let mut builder = QueryFingerprintBuilder::new("nia.build.output-lock.v1");
    builder.write_str(&output.protocol_path());
    let [first, second] = builder.finish().parts();
    cache_dir
        .join("coordination/output-locks")
        .join(format!("{first:016x}{second:016x}.lock"))
}

pub(crate) struct ScopedFileLock {
    path: PathBuf,
    token: String,
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
                    let token = match write_lock_owner(file) {
                        Ok(token) => token,
                        Err(error) => {
                            let _ = fs::remove_file(&path);
                            return Err(error);
                        }
                    };
                    return Ok(Some(Self { path, token }));
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
        if fs::read_to_string(&self.path).is_ok_and(|current| current.trim_end() == self.token) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn write_lock_owner(mut file: fs::File) -> io::Result<String> {
    let identity = ProcessIdentity::current();
    let sequence = LOCK_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let token = format!("{}:{}:{sequence}", identity.pid, identity.start_time);
    writeln!(file, "{token}")?;
    file.sync_all()?;
    Ok(token)
}

fn reclaim_stale_lock(path: &Path, stale_after: Duration) {
    if lock_owner_is_alive(path) {
        return;
    }
    if read_lock_owner(path).is_none() && !lock_is_stale_by_age(path, stale_after) {
        return;
    }
    let _ = fs::remove_file(path);
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
    let owner = fs::read_to_string(path).ok()?;
    let token = owner.split_whitespace().next()?;
    let mut parts = token.split(':');
    Some(ProcessIdentity {
        pid: parts.next()?.parse().ok()?,
        start_time: parts.next()?.parse().ok()?,
    })
}

#[cfg(target_os = "linux")]
fn process_start_time(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(Path::new("/proc").join(pid.to_string()).join("stat")).ok()?;
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
}
