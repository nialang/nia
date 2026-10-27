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

fn run_clean(root: &Path) -> std::process::Output {
    let mut command = support::nia_command();
    command.args(["clean", "--root"]).arg(root);
    command.output_timeout_for_runtime("run nia clean")
}

fn write(root: &Path, path: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"state").unwrap();
}

#[test]
fn clean_removes_outputs_caches_and_transients_but_keeps_dependencies() {
    let root = package_root("clean-current-state");
    for path in [
        "build/out/file",
        "build/cache/file",
        "build/tmp/file",
        "build/packages/pkg/file",
        "build/registry/indexes/nia/index.toml",
        "build/packages.lock",
    ] {
        write(&root, path);
    }

    let output = run_clean(&root);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!root.join("build/out").exists());
    assert!(!root.join("build/cache").exists());
    assert!(!root.join("build/tmp").exists());
    assert!(root.join("build/packages/pkg/file").is_file());
    assert!(root.join("build/registry/indexes/nia/index.toml").is_file());
    assert!(root.join("build/packages.lock").is_file());
}

#[test]
fn clean_from_a_workspace_member_cleans_the_shared_state_root() {
    let root = test_dir("clean-workspace-member");
    std::fs::write(root.join("nia.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::create_dir_all(root.join("app/src")).unwrap();
    std::fs::write(
        root.join("app/nia.toml"),
        "[package]\nname = \"app\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    write(&root, "build/out/app/file");
    write(&root, "build/cache/file");

    let output = run_clean(&root.join("app/src"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!root.join("build/out/app").exists());
    assert!(!root.join("build/cache").exists());
}

#[test]
fn clean_refuses_a_directory_that_is_not_a_nia_package() {
    let root = test_dir("clean-foreign-directory");
    write(&root, "build/out/foreign");
    write(&root, "build/cache/foreign");

    let output = run_clean(&root);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("build.nia"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.join("build/out/foreign").is_file());
    assert!(root.join("build/cache/foreign").is_file());
}

#[cfg(unix)]
#[test]
fn clean_refuses_a_linked_build_directory() {
    let root = package_root("clean-linked-build");
    let outside = test_dir("clean-linked-build-outside");
    write(&outside, "out/kept");
    std::os::unix::fs::symlink(&*outside, root.join("build")).unwrap();

    let output = run_clean(&root);
    assert!(!output.status.success());
    assert!(outside.join("out/kept").is_file());
}
