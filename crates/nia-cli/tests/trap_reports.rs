// SPDX-License-Identifier: GPL-3.0-or-later
//! Failed runtime checks report their cause and location before trapping.

mod support;

use nia_test_support::{CommandExt, test_dir};
use std::process::Command;

const PRELUDE: &str = "@[inlineNever]\nfun id[T](v: T) T { v }\n\
pub fun main(init: std::process::Init) std::process::ExitCode!() {\n    _ = init;\n";

/// Builds `body` (the rest of `main`) as `name`, runs it, and returns its
/// standard error after checking that it failed the way a trap fails.
fn run_failing(root: &std::path::Path, name: &str, body: &str, options: &[&str]) -> String {
    let source = root.join(format!("{name}.nia"));
    std::fs::write(&source, format!("{PRELUDE}{body}")).expect("write program");
    let exe = root.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    let output = support::nia_command()
        .args(["emit", "--exe"])
        .args(options)
        .arg(&source)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("build failing check");
    assert!(
        output.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new(&exe).output_timeout_for_runtime("run failing check");
    assert!(!output.status.success(), "{name} did not trap");
    // The report adds output; the program ends as an unreported trap does.
    #[cfg(windows)]
    assert_eq!(output.status.code(), Some(0xC000001Du32 as i32), "{name}");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(output.status.signal(), Some(4), "{name}: expected SIGILL");
    }
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Whether `stderr` holds the report line `nia: <path>/<report>`. Locations
/// use the source's display path, here the absolute path the test passed.
fn reports(stderr: &str, report: &str) -> bool {
    stderr.lines().any(|line| {
        line.strip_prefix("nia: ").is_some_and(|rest| {
            rest.ends_with(report) && rest[..rest.len() - report.len()].ends_with(['/', '\\'])
        })
    })
}

#[test]
fn every_failed_check_reports_cause_operands_and_location() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = test_dir("trap_reports");
    for (name, body, report) in [
        (
            "add",
            "    let x: u8 = id(250u8) + id(10u8);\n    _ = x;\n    !()\n}\n",
            "add.nia:5:17: integer overflow: 250 + 10 does not fit in u8",
        ),
        (
            "neg",
            "    let x = -id(-128i8);\n    _ = x;\n    !()\n}\n",
            "neg.nia:5:13: integer overflow: -(-128) does not fit in i8",
        ),
        (
            "div",
            "    let x = id(7i32) / id(0i32);\n    _ = x;\n    !()\n}\n",
            "div.nia:5:13: division by zero: 7 / 0 in i32",
        ),
        (
            "rem",
            "    let x = id(7u64) % id(0u64);\n    _ = x;\n    !()\n}\n",
            "rem.nia:5:13: remainder by zero: 7 % 0 in u64",
        ),
        (
            "min",
            "    let x = id(-2147483648i32) / id(-1i32);\n    _ = x;\n    !()\n}\n",
            "min.nia:5:13: integer overflow: -2147483648 / -1 does not fit in i32",
        ),
        (
            "shl",
            "    let x = id(1u32) << id(40u32);\n    _ = x;\n    !()\n}\n",
            "shl.nia:5:13: shift amount out of range: 1 << 40, where u32 allows 0..32",
        ),
        (
            "shr",
            "    let x = id(1i32) >> id(-3i32);\n    _ = x;\n    !()\n}\n",
            "shr.nia:5:13: shift amount out of range: 1 >> -3, where i32 allows 0..32",
        ),
        (
            "shift_overflow",
            "    let x = id(200u8) << id(2u8);\n    _ = x;\n    !()\n}\n",
            "shift_overflow.nia:5:13: integer overflow: 200 << 2 does not fit in u8",
        ),
        (
            "wide",
            "    let x = id(170141183460469231731687303715884105727i128) + id(1i128);\n    _ = x;\n    !()\n}\n",
            "wide.nia:5:13: integer overflow: 170141183460469231731687303715884105727 + 1 does not fit in i128",
        ),
        (
            "pointer_width",
            "    let x = id(0usize) - id(1usize);\n    _ = x;\n    !()\n}\n",
            "pointer_width.nia:5:13: integer overflow: 0 - 1 does not fit in usize",
        ),
        (
            "vector",
            "    let a = std::builtin::splat[u8x16](200u8);\n    let b = std::builtin::insert(std::builtin::splat[u8x16](1u8), 5, 100u8);\n    let c = id(a) + id(b);\n    _ = c;\n    !()\n}\n",
            "vector.nia:7:13: integer overflow in lane 5 of u8x16: 200 + 100 does not fit in u8",
        ),
        (
            "lane",
            "    let a = std::builtin::splat[u8x16](1u8);\n    _ = std::builtin::extract(a, id(20usize));\n    !()\n}\n",
            "lane.nia:6:9: vector lane index 20 is out of range for u8x16",
        ),
        (
            "explicit",
            "    if id(true) { std::builtin::trap(); }\n    !()\n}\n",
            "explicit.nia:5:19: explicit trap",
        ),
    ] {
        let stderr = run_failing(&root, name, body, &[]);
        assert!(reports(&stderr, report), "{name}: {stderr}");
    }
}

#[test]
fn tracked_functions_report_their_caller() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = test_dir("trap_reports_tracked");
    let source = root.join("tracked.nia");
    std::fs::write(
        &source,
        "@[trackCaller]\nfun add(a: u8, b: u8) u8 { a + b }\n\
@[inlineNever]\nfun id[T](v: T) T { v }\n\
pub fun main(init: std::process::Init) std::process::ExitCode!() {\n\
    _ = init;\n    let x = add(id(200u8), id(100u8));\n    _ = x;\n    !()\n}\n",
    )
    .expect("write program");
    let exe = root.join(format!("tracked{}", std::env::consts::EXE_SUFFIX));
    let output = support::nia_command()
        .args(["emit", "--exe"])
        .arg(&source)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("build tracked check");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new(&exe).output_timeout_for_runtime("run tracked check");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        reports(
            &stderr,
            "tracked.nia:7:13: integer overflow: 200 + 100 does not fit in u8"
        ),
        "{stderr}"
    );
}

#[test]
fn debug_builds_report_at_every_optimization_and_lto_mode() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = test_dir("trap_reports_profiles");
    let body = "    let x: u8 = id(250u8) + id(10u8);\n    _ = x;\n    !()\n}\n";
    for options in [
        &["--debug", "-O0"][..],
        &["--debug", "-O2"],
        &["--debug", "-O2", "--lto", "thin"],
        &["--debug", "-O2", "--lto", "full"],
    ] {
        let stderr = run_failing(&root, "add", body, options);
        assert!(
            reports(
                &stderr,
                "add.nia:5:17: integer overflow: 250 + 10 does not fit in u8"
            ),
            "{options:?}: {stderr}"
        );
    }
    // A release build traps the same way without a report, and links no
    // reporter: reports at every check grew release code by about 28%.
    for options in [&["--release"][..], &["--release", "--lto", "full"]] {
        let stderr = run_failing(&root, "add", body, options);
        assert!(!stderr.contains("nia: "), "{options:?}: {stderr}");
    }
}

#[test]
fn code_without_the_runtime_traps_without_a_report_symbol() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Compiler);
    let root = test_dir("trap_reports_bare");
    let source = root.join("lib.nia");
    std::fs::write(&source, "pub extern fun add(a: u8, b: u8) u8 { a + b }\n")
        .expect("write library");
    for target in nia_target::SUPPORTED_TARGETS {
        // Objects, libraries and bare programs link no runtime, so a failed
        // check is the trap instruction and references no support symbol.
        let output = support::nia_command()
            .args([
                "--target",
                &target.name(),
                "emit",
                "--llvm",
                "--runtime",
                "bare",
            ])
            .arg(&source)
            .output_timeout_for_compiler("emit bare checked code");
        assert!(
            output.status.success(),
            "{target}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let ir = String::from_utf8_lossy(&output.stdout);
        assert!(!ir.contains("__nia_trap"), "{target}: {ir}");
        assert!(ir.contains("@llvm.trap"), "{target}: {ir}");
    }
}

#[test]
fn the_reporter_never_reports_its_own_traps() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Compiler);
    let root = test_dir("trap_reports_runtime");
    let source = root.join("main.nia");
    std::fs::write(
        &source,
        format!("{PRELUDE}    let x = id(1u8) + id(2u8);\n    _ = x;\n    !()\n}}\n"),
    )
    .expect("write program");
    let output = support::nia_command()
        .args([
            "--target",
            "x86_64-unknown-linux",
            "emit",
            "--llvm",
            "--runtime",
            "freestanding",
        ])
        .arg(&source)
        .output_timeout_for_compiler("emit freestanding checked code");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ir = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let definition = ir
        .split("define void @__nia_trap(")
        .nth(1)
        .expect("runtime defines the reporter");
    let reporter = definition
        .split_once('{')
        .map(|(_, body)| body)
        .unwrap_or_else(|| panic!("reporter body: {definition}"))
        .split("\n}\n")
        .next()
        .expect("reporter body");
    // Runtime code traps without reporting, so the reporter cannot re-enter
    // itself through a check of its own.
    assert!(!reporter.contains("@__nia_trap("), "{reporter}");
    assert!(reporter.contains("@llvm.trap"), "{reporter}");
    // User code reports.
    assert!(ir.contains("call void @__nia_trap("), "{ir}");
}

/// A builtin operator or method used only for its effect is still
/// evaluated, so its checks run. Discarding one stopped code generation
/// with an internal error ("builtin operator cannot be emitted as a raw
/// call").
#[test]
fn discarded_operations_are_still_checked() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = test_dir("trap_reports_discarded");
    let body = "    _ = id(1u8) + id(2u8);\n    let xs: [u8; 3] = [1, 2, 3];\n    _ = (&xs[..]).len();\n    _ = id(250u8) + id(10u8);\n    !()\n}\n";
    let stderr = run_failing(&root, "discarded", body, &[]);
    assert!(
        reports(
            &stderr,
            "discarded.nia:8:9: integer overflow: 250 + 10 does not fit in u8"
        ),
        "{stderr}"
    );
    run_failing(&root, "discarded", body, &["--release"]);
}

/// Array and slice indexing and slicing are checked in every profile;
/// pointer bases are not.
#[test]
fn array_and_slice_bounds_are_checked() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = test_dir("trap_reports_bounds");
    let setup = "    let xs: [u8; 3] = [1, 2, 3];\n    let s = &xs[..];\n";
    for (name, statement, report) in [
        (
            "array",
            "_ = xs[id(3usize)];",
            "7:9: index 3 is out of bounds for length 3",
        ),
        (
            "slice",
            "_ = s[id(5usize)];",
            "7:9: index 5 is out of bounds for length 3",
        ),
        (
            "place",
            "let mut ys: [u8; 3] = [0; 3]; ys[id(4usize)] = 1;",
            "7:35: index 4 is out of bounds for length 3",
        ),
        (
            "end",
            "_ = &s[0..id(4usize)];",
            "7:9: slice end 4 is out of bounds for length 3",
        ),
        (
            "inclusive_end",
            "_ = &s[0..=id(3usize)];",
            "7:9: inclusive slice end 3 is out of bounds for length 3",
        ),
        (
            "maximum_end",
            "_ = &s[0..=id(18446744073709551615usize)];",
            "7:9: inclusive slice end 18446744073709551615 is out of bounds for length 3",
        ),
        (
            "order",
            "_ = &s[id(2usize)..id(1usize)];",
            "7:9: slice start 2 is after its end 1",
        ),
        (
            "inclusive_order",
            "_ = &s[id(3usize)..=id(1usize)];",
            "7:9: slice start 3 is after its inclusive end 1",
        ),
        (
            "array_end",
            "_ = &xs[1..id(9usize)];",
            "7:9: slice end 9 is out of bounds for length 3",
        ),
    ] {
        let body = format!("{setup}    {statement}\n    !()\n}}\n");
        let stderr = run_failing(&root, name, &body, &[]);
        assert!(
            reports(&stderr, &format!("{name}.nia:{report}")),
            "{name}: {stderr}"
        );
        // Release builds check too, and trap without a report.
        let stderr = run_failing(&root, name, &body, &["--release"]);
        assert!(!stderr.contains("nia: "), "{name}: {stderr}");
    }
    // Bounds that hold, including an empty inclusive range and indexing
    // through a pointer, which stays unchecked.
    let source = root.join("valid.nia");
    std::fs::write(
        &source,
        format!(
            "{PRELUDE}{setup}    _ = s[id(2usize)];\n    _ = &s[id(3usize)..];\n    \
             _ = &s[id(1usize)..=id(2usize)];\n    _ = &s[id(2usize)..=id(1usize)];\n    \
             let p = &xs;\n    _ = p[0];\n    !()\n}}\n"
        ),
    )
    .expect("write valid program");
    let exe = root.join(format!("valid{}", std::env::consts::EXE_SUFFIX));
    let output = support::nia_command()
        .args(["emit", "--exe"])
        .arg(&source)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("build valid bounds");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new(&exe).output_timeout_for_runtime("run valid bounds");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
