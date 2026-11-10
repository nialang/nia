// SPDX-License-Identifier: GPL-3.0-or-later
//! `@[inlineAlways]`: every direct call is inlined at every optimization
//! level, including calls into another module's codegen unit without
//! link-time optimization. Code generation verifies the promise after
//! optimization and fails the build if a call remains, so a successful build
//! here is itself evidence that no call to the function is left.
use std::fs;
use std::process::Command;

mod support;

use nia_test_support::{CommandExt, CommandStatusExt, TestWorkload, test_dir as temp_dir};

const HELPERS: &str = r#"
@[inlineAlways]
pub fun triple(value: u64) u64 {
    value * 3
}

// Generic instances are inlined like monomorphic functions.
@[inlineAlways]
pub fun pick[T](first: T, second: T, takeFirst: bool) T {
    if takeFirst { first } else { second }
}
"#;

const MAIN: &str = r#"
using std::process;
module helpers;

@[inlineAlways]
fun twice(value: u64) u64 {
    value * 2
}

// Kept out of line so that each call site stays a separate function.
@[inlineNever]
fun local(value: u64) u64 {
    twice(value) + 1
}

@[inlineNever]
fun remote(value: u64) u64 {
    helpers::triple(value) + helpers::pick(1u64, 2u64, value > 3)
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    // Taking the address of an inline-always function still gives a function.
    let pointer: &fun(u64) u64 = helpers::triple;
    if local(3) != 7 or remote(4) != 13 or pointer(5) != 15 {
        return process::ExitCode(1)!;
    }
    !()
}
"#;

#[test]
fn inline_always_inlines_calls_into_other_modules_at_every_level() {
    let root = temp_dir("inline_always_inlines_calls_into_other_modules");
    fs::write(root.join("helpers.nia"), HELPERS).expect("write helpers");
    let main = root.join("main.nia");
    fs::write(&main, MAIN).expect("write main");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    for options in [
        &["-O0"][..],
        &["-O2"],
        &["-O2", "--lto", "off"],
        &["-O2", "--lto", "full"],
        &["-Oz"],
    ] {
        let output = support::nia_command()
            .arg("emit")
            .arg("--exe")
            .args(options)
            .arg(&main)
            .arg("-o")
            .arg(&exe)
            .output_timeout_for_build("emit inline-always executable");
        assert!(
            output.status.success(),
            "{options:?} stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let run = Command::new(&exe).status_timeout("run inline-always executable");
        assert_eq!(run.code(), Some(0), "{options:?}");
    }
}

#[test]
fn inline_always_rejects_impossible_uses() {
    let root = temp_dir("inline_always_rejects_impossible_uses");
    let main = root.join("main.nia");
    fs::write(
        &main,
        r#"
@[inlineAlways(fast)]
fun withArguments() i32 { 1 }

@[inlineAlways]
@[inlineAlways]
fun twice() i32 { 2 }

@[inlineAlways]
@[inlineNever]
fun conflicting() i32 { 3 }

@[inlineAlways]
extern fun external() i32;

fun main() i32 { withArguments() + twice() + conflicting() }
"#,
    )
    .expect("write source");
    let output = support::nia_command()
        .arg("check")
        .arg(&main)
        .output_timeout_for_build("check impossible inline-always uses");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stderr:\n{stderr}");
    for expected in [
        "`@[inlineAlways]` does not take arguments",
        "duplicate `@[inlineAlways]` function attribute",
        "`@[inlineAlways]` cannot be combined with `@[inlineNever]`",
        "`@[inlineAlways]` requires a function body",
    ] {
        assert!(stderr.contains(expected), "missing `{expected}`:\n{stderr}");
    }
    assert!(!stderr.contains("internal"), "stderr:\n{stderr}");
}

#[test]
fn inline_always_rejects_call_cycles_across_modules() {
    let root = temp_dir("inline_always_rejects_call_cycles");
    fs::write(
        root.join("other.nia"),
        r#"@[inlineAlways]
pub fun pong(value: u64) u64 {
    if value == 0 { 1 } else { root::ping(value) }
}
"#,
    )
    .expect("write other module");
    let main = root.join("main.nia");
    fs::write(
        &main,
        r#"using std::process;
module other;

@[inlineAlways]
fun countdown(value: u64) u64 {
    if value == 0 { 0 } else { countdown(value - 1) }
}

@[inlineAlways]
pub fun ping(value: u64) u64 {
    if value == 0 { 0 } else { other::pong(value - 1) }
}

// A chain without a cycle is allowed.
@[inlineAlways]
fun first(value: u64) u64 { second(value) + 1 }

@[inlineAlways]
fun second(value: u64) u64 { value * 2 }

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    if countdown(3) + ping(2) + first(1) == 99 {
        return process::ExitCode(1)!;
    }
    !()
}
"#,
    )
    .expect("write main module");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit inline-always cycles");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stderr:\n{stderr}");
    assert_eq!(
        stderr
            .matches("`@[inlineAlways]` functions cannot call each other in a cycle")
            .count(),
        2,
        "stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("this `@[inlineAlways]` function calls itself"),
        "stderr:\n{stderr}"
    );
    // Each member of a cycle is shown in its own file.
    assert!(stderr.contains("main.nia:9:1"), "stderr:\n{stderr}");
    assert!(stderr.contains("other.nia:1:1"), "stderr:\n{stderr}");
    assert!(!stderr.contains("internal"), "stderr:\n{stderr}");
}

// A caller's codegen unit carries a copy of an inline-always callee from
// another module, so changing only the callee must rebuild the caller.
#[test]
fn inline_always_callers_rebuild_when_the_callee_changes() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let workspace = temp_dir("inline_always_callers_rebuild");
    fs::create_dir_all(workspace.join("src")).expect("create source directory");
    fs::write(
        workspace.join("nia.toml"),
        "[package]\nname = \"app\"\nversion = \"1.0.0\"\n\
         [[executable]]\nname = \"app\"\npath = \"src/main.nia\"\n",
    )
    .expect("write manifest");
    fs::write(
        workspace.join("src/main.nia"),
        r#"using std::process;
module value;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    process::ExitCode(value::code())!
}
"#,
    )
    .expect("write main");
    let write_value = |code: i32| {
        fs::write(
            workspace.join("src/value.nia"),
            format!("@[inlineAlways]\npub fun code() i32 {{ {code} }}\n"),
        )
        .expect("write value module")
    };
    let build_and_run = |expected: i32| {
        for options in [&["-O0"][..], &["-O2", "--lto", "off"]] {
            let build = support::nia_command()
                .args(["build", "app"])
                .args(options)
                .arg("--root")
                .arg(&workspace)
                .output_timeout_in_session("build the inline-always package");
            assert!(
                build.status.success(),
                "{options:?}: {}",
                String::from_utf8_lossy(&build.stderr)
            );
            let run = Command::new(workspace.join("build/out/app"))
                .output_timeout_in_session("run the inline-always package");
            assert_eq!(run.status.code(), Some(expected), "{options:?}");
        }
    };
    write_value(5);
    build_and_run(5);
    write_value(9);
    build_and_run(9);
}
