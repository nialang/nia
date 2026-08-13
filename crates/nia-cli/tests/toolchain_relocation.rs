// SPDX-License-Identifier: GPL-3.0-or-later
use std::{fs, path::Path, process::Command};

#[allow(dead_code, unused_imports)]
mod support;

use nia_test_support::{CommandExt, TestWorkload, copy_case_tree};

#[test]
fn copied_installed_toolchain_reuses_caches_and_drives_build() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let root = support::temp_dir("copied_installed_toolchain");
    let first = root.join("first");
    let second = root.join("second");
    let cache = root.join("cache");
    let output = root.join("output");
    fs::create_dir_all(first.join("bin")).expect("create installed bin directory");
    fs::create_dir_all(first.join("lib/nia")).expect("create installed resource directory");
    fs::create_dir_all(&output).expect("create output directory");
    fs::copy(env!("CARGO_BIN_EXE_nia"), first.join("bin/nia")).expect("copy installed compiler");
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("nia-cli lives under crates/");
    copy_tree(&workspace_root.join("lib"), &first.join("lib/nia"));
    let source = workspace_root.join("examples/00_minimal.nia");

    assert_success(
        Command::new(first.join("bin/nia"))
            .arg("check")
            .arg(&source)
            .arg("--cache-dir")
            .arg(&cache)
            .output_timeout_in_session("check from first installed toolchain"),
    );
    assert_success(
        Command::new(first.join("bin/nia"))
            .arg("emit")
            .arg("--obj")
            .arg(&source)
            .arg("--runtime")
            .arg("freestanding")
            .arg("--cache-dir")
            .arg(&cache)
            .arg("--out-dir")
            .arg(output.join("objects"))
            .output_timeout_in_session("emit objects from first installed toolchain"),
    );
    let first_link = Command::new(first.join("bin/nia"))
        .arg("--timings")
        .arg("emit")
        .arg("--exe")
        .arg(&source)
        .arg("--cache-dir")
        .arg(&cache)
        .arg("-o")
        .arg(output.join("first-exe"))
        .output_timeout_in_session("link from first installed toolchain");
    assert_success(first_link);

    copy_tree(&first, &second);
    assert_success(
        Command::new(second.join("bin/nia"))
            .arg("check")
            .arg(&source)
            .arg("--cache-dir")
            .arg(&cache)
            .output_timeout_in_session("check from relocated installed toolchain"),
    );
    assert_success(
        Command::new(second.join("bin/nia"))
            .arg("emit")
            .arg("--obj")
            .arg(&source)
            .arg("--runtime")
            .arg("freestanding")
            .arg("--cache-dir")
            .arg(&cache)
            .arg("--out-dir")
            .arg(output.join("relocated-objects"))
            .output_timeout_in_session("emit objects from relocated installed toolchain"),
    );
    let second_exe = output.join("second-exe");
    let relocated = Command::new(second.join("bin/nia"))
        .arg("--timings")
        .arg("emit")
        .arg("--exe")
        .arg(&source)
        .arg("--cache-dir")
        .arg(&cache)
        .arg("-o")
        .arg(&second_exe)
        .output_timeout_in_session("link from relocated installed toolchain");
    assert_success_ref(&relocated);
    let timings = String::from_utf8_lossy(&relocated.stderr);
    assert!(
        timing_counter(&timings, "llvm.object_reuse_hits") > 0,
        "relocated compilation should reuse at least one object in {timings}"
    );
    for counter in [
        "llvm.object_reuse_misses: 0",
        "link.result_reuse_hits: 1",
        "link.result_reuse_misses: 0",
        "link.result_invalidation_toolchain: 0",
    ] {
        assert!(
            timings.contains(&format!("timing summary counter {counter}")),
            "missing {counter:?} in {timings}"
        );
    }
    assert_success(Command::new(&second_exe).output_timeout_in_session("run relocated executable"));

    let build_fixture = manifest_dir.join("tests/cases/build/configured_success");
    let build_workspace = root.join("build-workspace");
    copy_case_tree(&build_fixture, &build_workspace);
    assert_success(
        Command::new(second.join("bin/nia"))
            .arg("build")
            .arg("--root")
            .arg(&build_workspace)
            .output_timeout_in_session("build from relocated installed toolchain"),
    );
    assert!(build_workspace.join(".nia-build/custom-app").is_file());
}

fn timing_counter(timings: &str, name: &str) -> u64 {
    let prefix = format!("timing summary counter {name}: ");
    timings
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("missing integer counter {name:?} in {timings}"))
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create copied directory");
    for entry in fs::read_dir(source).expect("read copied directory") {
        let entry = entry.expect("read copied entry");
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy file");
        }
    }
}

fn assert_success(output: std::process::Output) {
    assert_success_ref(&output);
}

fn assert_success_ref(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
