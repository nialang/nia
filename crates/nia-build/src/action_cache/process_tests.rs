// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use nia_query::QueryFingerprint;

use super::*;
use crate::PackageKey;

const CACHE_STRESS_ROOT: &str = "NIA_TEST_ACTION_CACHE_STRESS_ROOT";
const CACHE_STRESS_ROLE: &str = "NIA_TEST_ACTION_CACHE_STRESS_ROLE";
const CACHE_STRESS_WORKER: &str = "NIA_TEST_ACTION_CACHE_STRESS_WORKER";
const CACHE_STRESS_TEST: &str =
    "action_cache::process_tests::cross_process_generated_file_cache_worker";

struct ChildGroup(Vec<Child>);

impl Drop for ChildGroup {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn test_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "nia-build-action-cache-cross-process-stress-{}-{}",
        std::process::id(),
        CACHE_STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

fn action() -> ActionKey {
    ActionKey::new(PackageKey::root(), "generate").expect("action key")
}

fn output() -> LogicalPath {
    LogicalPath::new(LogicalPathRoot::Build, "generated/large.bin").expect("logical output")
}

fn toolchain() -> GeneratedFileToolchainComponents {
    GeneratedFileToolchainComponents {
        compiler: QueryFingerprint::from_parts([1, 1]),
        resource_layout: QueryFingerprint::from_parts([1, 2]),
        standard_library: QueryFingerprint::from_parts([1, 3]),
        build_protocol: QueryFingerprint::from_parts([1, 4]),
    }
}

fn stress_payload() -> Vec<u8> {
    vec![0x5a; 4 * 1024 * 1024]
}

fn identity(payload: &[u8]) -> GeneratedFileCacheIdentity {
    GeneratedFileCacheIdentity::with_toolchain_components(
        &action(),
        &output(),
        payload,
        toolchain(),
    )
}

fn wait_for_path(path: &Path, deadline: Instant) {
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for `{}`",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for_children(children: &mut ChildGroup, deadline: Instant) {
    let mut complete = vec![false; children.0.len()];
    while complete.iter().any(|complete| !complete) {
        if Instant::now() >= deadline {
            for (child, complete) in children.0.iter_mut().zip(&complete) {
                if !complete {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            panic!("cross-process action-cache workers timed out");
        }
        for (child, complete) in children.0.iter_mut().zip(&mut complete) {
            if *complete {
                continue;
            }
            if let Some(status) = child.try_wait().expect("poll cache stress worker") {
                assert!(status.success(), "cache stress worker exited with {status}");
                *complete = true;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "spawned by the cross-process cache stress test"]
fn cross_process_generated_file_cache_worker() {
    let Some(root) = std::env::var_os(CACHE_STRESS_ROOT).map(PathBuf::from) else {
        return;
    };
    let role = std::env::var(CACHE_STRESS_ROLE).expect("cache stress worker role");
    let worker = std::env::var(CACHE_STRESS_WORKER).expect("cache stress worker identity");
    let ready = root.join("ready").join(worker);
    fs::create_dir_all(ready.parent().expect("ready directory")).expect("create ready root");
    fs::write(&ready, b"ready").expect("publish worker readiness");
    wait_for_path(
        &root.join("start"),
        Instant::now() + Duration::from_secs(10),
    );

    let payload = stress_payload();
    let identity = identity(&payload);
    let cache = GeneratedFileCache::new(root.clone());
    match role.as_str() {
        "publisher" => cache
            .publish(&identity, &payload)
            .expect("cross-process publication"),
        "reader" => {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match cache.lookup(&identity).expect("cross-process lookup") {
                    GeneratedFileCacheLookup::Hit(found) => {
                        assert_eq!(found, payload);
                        break;
                    }
                    GeneratedFileCacheLookup::Miss(ActionCacheMissReason::NotFound) => {
                        assert!(
                            Instant::now() < deadline,
                            "reader never observed an accepted entry"
                        );
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    unexpected => panic!("reader observed non-atomic cache state: {unexpected:?}"),
                }
            }
            for _ in 0..8 {
                assert_eq!(
                    cache.lookup(&identity).expect("repeated accepted lookup"),
                    GeneratedFileCacheLookup::Hit(payload.clone())
                );
            }
        }
        unexpected => panic!("unknown cache stress worker role `{unexpected}`"),
    }
}

#[test]
fn cross_process_publishers_and_readers_share_one_complete_entry() {
    let root = test_root();
    fs::create_dir_all(root.join("ready")).expect("create stress root");
    let executable = std::env::current_exe().expect("current test executable");
    let roles = [
        "publisher",
        "reader",
        "publisher",
        "reader",
        "publisher",
        "reader",
        "publisher",
        "reader",
    ];
    let mut children = ChildGroup(Vec::with_capacity(roles.len()));
    for (worker, role) in roles.iter().enumerate() {
        let child = Command::new(&executable)
            .args(["--exact", CACHE_STRESS_TEST, "--ignored", "--nocapture"])
            .env(CACHE_STRESS_ROOT, &root)
            .env(CACHE_STRESS_ROLE, role)
            .env(CACHE_STRESS_WORKER, worker.to_string())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn cache stress worker");
        children.0.push(child);
    }

    let ready_deadline = Instant::now() + Duration::from_secs(10);
    for worker in 0..roles.len() {
        wait_for_path(&root.join("ready").join(worker.to_string()), ready_deadline);
    }
    fs::write(root.join("start"), b"start").expect("release cache stress workers");
    wait_for_children(&mut children, Instant::now() + Duration::from_secs(20));

    let payload = stress_payload();
    let identity = identity(&payload);
    let cache = GeneratedFileCache::new(root.clone());
    assert_eq!(
        cache.lookup(&identity).expect("final cache lookup"),
        GeneratedFileCacheLookup::Hit(payload)
    );
    let entry_directory = cache.key_dir(identity.fingerprints.cache_key);
    let entries = fs::read_dir(&entry_directory)
        .expect("read cache key directory")
        .map(|entry| entry.expect("cache directory entry").path())
        .collect::<Vec<_>>();
    assert_eq!(
        entries
            .iter()
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("entry"))
            .count(),
        1
    );
    assert!(
        entries
            .iter()
            .all(|path| path.extension().and_then(|value| value.to_str()) == Some("entry")),
        "staged cache files remained after publication: {entries:?}"
    );
    let mutation_locks = root
        .join("coordination")
        .join("action-cache-mutations")
        .join(GENERATED_FILE_SCHEMA);
    assert_eq!(
        fs::read_dir(mutation_locks)
            .expect("read mutation lock directory")
            .count(),
        0,
        "mutation locks remained after workers exited"
    );

    let _ = fs::remove_dir_all(root);
}
