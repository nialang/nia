// SPDX-License-Identifier: GPL-3.0-or-later
use std::path::Path;

mod support;

use nia_test_support::{CommandExt, test_dir};

fn package_root(label: &str) -> nia_test_support::TestDir {
    let root = test_dir(label);
    std::fs::write(
        root.join("nia.toml"),
        "[package]\nname = \"clean-test\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    root
}

fn run_clean(root: &Path, args: &[&str]) -> std::process::Output {
    let mut command = support::nia_command();
    command.args(["clean", "--root"]).arg(root).args(args);
    command.output_timeout_for_runtime("run nia clean")
}

#[test]
fn clean_removes_outputs_and_action_cache_but_keeps_packages() {
    let root = package_root("clean-current-state");
    for path in [
        "build/out/file",
        "build/cache/file",
        "build/packages/pkg/file",
    ] {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"state").unwrap();
    }
    std::fs::write(root.join("build/packages.lock"), b"lock").unwrap();

    let output = run_clean(&root, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!root.join("build/out").exists());
    assert!(!root.join("build/cache").exists());
    assert!(root.join("build/packages/pkg/file").is_file());
    assert!(root.join("build/packages.lock").is_file());
}

#[test]
fn clean_migrates_legacy_state_and_refuses_destination_collisions() {
    let root = package_root("clean-migrate-state");
    for (path, contents) in [
        (".nia-build/out.txt", b"out".as_slice()),
        (".nia-cache/cache.txt", b"cache".as_slice()),
        (".nia/packages/pkg/file", b"package".as_slice()),
        (".nia/packages.lock", b"lock".as_slice()),
    ] {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    let output = run_clean(&root, &["--migrate"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read(root.join("build/out/out.txt")).unwrap(),
        b"out"
    );
    assert_eq!(
        std::fs::read(root.join("build/cache/cache.txt")).unwrap(),
        b"cache"
    );
    assert_eq!(
        std::fs::read(root.join("build/packages/pkg/file")).unwrap(),
        b"package"
    );
    assert_eq!(
        std::fs::read(root.join("build/packages.lock")).unwrap(),
        b"lock"
    );
    assert!(!root.join(".nia").exists());

    std::fs::create_dir_all(root.join(".nia-build")).unwrap();
    std::fs::write(root.join(".nia-build/left"), b"legacy").unwrap();
    std::fs::create_dir_all(root.join("build/out")).unwrap();
    std::fs::write(root.join("build/out/current"), b"current").unwrap();
    let output = run_clean(&root, &["--migrate"]);
    assert!(!output.status.success());
    assert!(root.join(".nia-build/left").is_file());
    assert_eq!(
        std::fs::read(root.join("build/out/current")).unwrap(),
        b"current"
    );
}

#[test]
fn clean_legacy_removes_only_known_legacy_entries() {
    let root = package_root("clean-legacy-state");
    std::fs::create_dir_all(root.join(".nia/packages")).unwrap();
    std::fs::write(root.join(".nia/packages/item"), b"old").unwrap();
    std::fs::write(root.join(".nia/keep"), b"user").unwrap();
    std::fs::create_dir_all(root.join("build/out")).unwrap();
    std::fs::write(root.join("build/out/current"), b"current").unwrap();

    let output = run_clean(&root, &["--legacy"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!root.join(".nia/packages").exists());
    assert_eq!(std::fs::read(root.join(".nia/keep")).unwrap(), b"user");
    assert!(root.join("build/out/current").is_file());
}
