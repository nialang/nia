// SPDX-License-Identifier: GPL-3.0-or-later
use std::{fs, process::Command};

#[allow(dead_code, unused_imports)]
mod support;

use nia_test_support::TestWorkload;
use support::CommandExt;

#[test]
fn test_command_lists_filters_and_reports_registered_suites() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let workspace = support::temp_dir("test-command-suites");
    fs::create_dir_all(workspace.join("tests")).expect("create test source directory");
    fs::write(
        workspace.join("build.nia"),
        r#"using std::build;
using std::fs;

pub fn build(b: &mut build::Build) build::Error!() {
    let passModule = b.addModule(build::ModuleOptions::init(
        &"pass-module",
        fs::PathView::init(&"tests/pass.nia"),
    )).?;
    let failModule = b.addModule(build::ModuleOptions::init(
        &"fail-module",
        fs::PathView::init(&"tests/fail.nia"),
    )).?;
    let secondFailModule = b.addModule(build::ModuleOptions::init(
        &"second-fail-module",
        fs::PathView::init(&"tests/fail_second.nia"),
    )).?;
    _ = b.addTestSuite(&"pass", passModule).?;
    _ = b.addTestSuite(&"fail", failModule).?;
    _ = b.addTestSuite(&"fail-second", secondFailModule).?;
    !()
}
"#,
    )
    .expect("write test build script");
    fs::write(
        workspace.join("tests/pass.nia"),
        r#"using std::process;
using std::test;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    match test::expectEqual(2, 2) {
        !ok => _ = ok,
        error! => {
            _ = error;
            return process::exit(1)!;
        },
    }
    !()
}
"#,
    )
    .expect("write passing suite");
    fs::write(
        workspace.join("tests/fail.nia"),
        r#"using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    process::exit(7)!
}
"#,
    )
    .expect("write failing suite");
    fs::write(
        workspace.join("tests/fail_second.nia"),
        r#"using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    process::exit(8)!
}
"#,
    )
    .expect("write second failing suite");

    let listed = test_command(&workspace)
        .arg("--list")
        .output_timeout_in_session("list registered Nia test suites");
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout),
        "fail\nfail-second\npass\n"
    );

    let passing = test_command(&workspace)
        .args(["--filter", "pass"])
        .output_timeout_in_session("run filtered Nia test suite");
    assert!(
        passing.status.success(),
        "{}",
        String::from_utf8_lossy(&passing.stderr)
    );

    let all = test_command(&workspace).output_timeout_in_session("run all Nia test suites");
    assert!(!all.status.success());
    let stderr = String::from_utf8_lossy(&all.stderr);
    assert!(stderr.contains("2 test suite(s) failed"), "{stderr}");
    assert!(stderr.contains("fail:"), "{stderr}");
    assert!(stderr.contains("fail-second:"), "{stderr}");

    let fail_fast = test_command(&workspace)
        .args(["--fail-fast", "--jobs", "1"])
        .output_timeout_in_session("stop after first failing Nia test suite");
    assert!(!fail_fast.status.success());
    let stderr = String::from_utf8_lossy(&fail_fast.stderr);
    assert!(stderr.contains("1 test suite(s) failed"), "{stderr}");
}

fn test_command(workspace: &std::path::Path) -> Command {
    let mut command = support::nia_command();
    command.arg("test").arg("--root").arg(workspace);
    command
}
