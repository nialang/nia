// SPDX-License-Identifier: GPL-3.0-or-later
use std::{borrow::Cow, path::Path, process::Command};

mod support;

use nia_test_support::{
    CaseManifest, CommandExt, CommandStatusExt, TestWorkload, case_directories, copy_case_tree,
    fixture_relative_path,
};

macro_rules! build_cases {
    ($($name:ident),+ $(,)?) => {
        const BUILD_CASE_NAMES: &[&str] = &[$(stringify!($name)),+];

        $(
            #[test]
            fn $name() {
                run_build_case(stringify!($name));
            }
        )+
    };
}

build_cases!(
    configured_optimization,
    configured_success,
    dependency_cycle,
    duplicate_module,
    duplicate_target,
    executable_dependency,
    invalid_output,
    invalid_target,
    missing_default,
    missing_script,
    step_order,
    unknown_step,
    unselected_dependency_cycle,
);

#[test]
fn run_steps_get_the_terminal_after_the_build_summary() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let root = nia_test_support::test_dir("run-step-progress");
    std::fs::create_dir(root.join("src")).expect("source directory");
    std::fs::write(
        root.join("src/main.nia"),
        r#"using std::io;
using std::process;
using process::{Init, ExitCode};

pub fun main(init: Init) ExitCode!() {
    _ = init;
    io::debugPrint(&"program-output\n", &[]).?;
    !()
}
"#,
    )
    .expect("program source");
    std::fs::write(
        root.join("build.nia"),
        r#"using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {
    let app = b.addModule(
        build::ModuleOptions::init(&"app", fs::PathView::init(&"src/main.nia")),
    ).?;
    let exe = b.addExecutable(build::ExecutableOptions::init(&"app", app)).?;
    let emit = b.addEmitExecutableStep(&"build", exe).?;
    _ = b.addRunExecutableStep(&"run", build::RunOptions::init(exe)).?;
    b.setDefaultStep(emit).?;
    !()
}
"#,
    )
    .expect("build script");

    let run = support::nia_command()
        .args(["build", "run", "--progress=plain", "--root"])
        .arg(&*root)
        .output_timeout_in_session("run a step at the default progress level");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    let lines = stderr.lines().collect::<Vec<_>>();
    assert!(
        lines.len() == 3 && lines[0].starts_with("Finished 1 action in "),
        "{stderr}"
    );
    assert_eq!(
        lines[1..],
        ["Running root:run", "program-output"],
        "{stderr}"
    );

    // Without a run step the summary closes the build, and phases stay at -v.
    let build = support::nia_command()
        .args(["build", "--progress=plain", "--root"])
        .arg(&*root)
        .output_timeout_in_session("build at the default progress level");
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(build.status.success(), "{stderr}");
    assert!(
        stderr.lines().count() == 1 && stderr.starts_with("Finished 1 action in "),
        "{stderr}"
    );
}

#[test]
fn json_progress_keeps_success_and_failure_stderr_machine_readable() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/build");

    let success_root = nia_test_support::test_dir("json-progress-success");
    copy_case_tree(&fixtures.join("configured_success"), &success_root);
    let success = support::nia_command()
        .args(["--timings", "build", "-vvv", "--progress=json", "--root"])
        .arg(&success_root)
        .output_timeout_in_session("build with JSON progress");
    assert!(
        success.status.success(),
        "{}",
        String::from_utf8_lossy(&success.stderr)
    );
    let success_records = parse_json_lines(&success.stderr);
    let progress = success_records
        .iter()
        .filter(|record| record["kind"] == "build-progress")
        .collect::<Vec<_>>();
    assert!(progress.iter().all(|record| record["event"].is_string()));
    assert!(
        progress
            .windows(2)
            .all(|pair| pair[0]["sequence"].as_u64() < pair[1]["sequence"].as_u64())
    );
    assert!(
        progress
            .iter()
            .any(|record| record["event"] == "build-started")
    );
    assert!(
        success_records
            .iter()
            .any(|record| record["protocol_epoch"].is_number() && record["timings"].is_array())
    );
    assert!(progress.iter().any(|record| {
        record["event"] == "action-started"
            && record["scheduler_wait_ms"].is_number()
            && record["resource_wait_ms"].is_number()
    }));
    assert!(progress.iter().any(|record| {
        record["event"] == "resource-usage-changed"
            && record["capacity"].is_number()
            && record["used"].is_number()
            && record["generation"].is_number()
    }));
    let terminal = progress
        .iter()
        .filter(|record| record["event"] == "action-finished")
        .collect::<Vec<_>>();
    let finished = progress
        .iter()
        .find(|record| record["event"] == "build-finished")
        .expect("successful progress stream has a build terminal record");
    assert_eq!(terminal.len(), finished["total"].as_u64().unwrap() as usize);
    assert_eq!(finished["completed"], finished["total"]);
    let terminal_index = success_records
        .iter()
        .position(|record| record["event"] == "build-finished")
        .unwrap();
    let timings_index = success_records
        .iter()
        .position(|record| record["timings"].is_array())
        .unwrap();
    assert!(
        terminal_index < timings_index,
        "progress must be flushed before timings"
    );

    // Reuse the configured package so the projection checks do not recompile
    // the runner. Lower levels must omit detail records completely.
    for verbosity in ["", "-v"] {
        let mut command = support::nia_command();
        command
            .args(["build", "--progress=json", "--root"])
            .arg(&success_root);
        if !verbosity.is_empty() {
            command.arg(verbosity);
        }
        let output = command.output_timeout_in_session("project JSON progress verbosity");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let records = parse_json_lines(&output.stderr);
        assert!(records.iter().all(|record| record["event"].is_string()));
        assert_eq!(
            records
                .iter()
                .any(|record| record["event"] == "action-cache-decision"),
            verbosity == "-v",
        );
        assert!(
            !records
                .iter()
                .any(|record| record["event"] == "action-described"
                    || record["event"] == "action-wait-started"
                    || record["event"] == "resource-usage-changed")
        );
        assert!(
            records
                .iter()
                .any(|record| record["event"] == "build-finished")
        );
        assert!(
            records
                .iter()
                .filter(|record| record["event"] == "action-finished")
                .all(|record| record["action_kind"].is_string())
        );
    }

    let failure_root = nia_test_support::test_dir("json-progress-failure");
    copy_case_tree(&fixtures.join("missing_script"), &failure_root);
    let failure = support::nia_command()
        .args(["build", "--progress=json", "--root"])
        .arg(&failure_root)
        .output_timeout_in_session("fail build with JSON progress");
    assert!(!failure.status.success());
    let failure_records = parse_json_lines(&failure.stderr);
    assert!(
        failure_records.iter().any(|record| {
            record["kind"] == "build-progress" && record["event"] == "build-failed"
        })
    );
    assert!(
        failure_records
            .iter()
            .any(|record| record["kind"] != "build-progress"),
        "failure stderr had no structured diagnostic: {}",
        String::from_utf8_lossy(&failure.stderr)
    );
}

fn parse_json_lines(bytes: &[u8]) -> Vec<serde_json::Value> {
    let stderr = String::from_utf8_lossy(bytes);
    assert!(
        !stderr.lines().any(str::is_empty),
        "unexpected blank stderr record: {stderr}"
    );
    stderr
        .lines()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("stderr was not JSON Lines ({error}): {line:?}"))
        })
        .collect()
}

#[test]
fn progress_preserves_generated_products_and_warm_cache_decisions() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let off_root = nia_test_support::test_dir("progress-products-off");
    let on_root = nia_test_support::test_dir("progress-products-on");
    let script = r#"using std::build;
pub fun build(b: &mut build::Build) build::Error!() {
    let first = b.addGeneratedFileStep(&"first", build::BuildPathView::init(&"first.txt"), &b"first\n"[..]).?;
    let second = b.addGeneratedFileStep(&"second", build::BuildPathView::init(&"second.txt"), &b"second\n"[..]).?;
    let all = b.addAggregateStep(&"all").?;
    b.dependOn(all, first).?;
    b.dependOn(all, second).?;
    b.setDefaultStep(all).?;
    !()
}
"#;
    for root in [&off_root, &on_root] {
        std::fs::write(root.join("build.nia"), script).unwrap();
    }
    for (root, mode) in [(&off_root, "none"), (&on_root, "plain")] {
        let output = support::nia_command()
            .args(["build", "--progress", mode, "-vvv", "-j", "2", "--root"])
            .arg(root)
            .output_timeout_in_session("cold progress product comparison");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.stderr.contains(&0x1b));
        if mode == "none" {
            assert!(output.stderr.is_empty());
        }
    }
    for name in ["first.txt", "second.txt", "build-plan.bin"] {
        assert_eq!(
            std::fs::read(off_root.join("build/out").join(name)).unwrap(),
            std::fs::read(on_root.join("build/out").join(name)).unwrap(),
            "different {name}"
        );
    }
    let mut baseline_cache = None;
    for (sample, (root, mode, jobs)) in [
        (&off_root, "none", "1"),
        (&on_root, "plain", "2"),
        (&on_root, "auto", "1"),
    ]
    .into_iter()
    .cycle()
    .take(15)
    .enumerate()
    {
        let started = std::time::Instant::now();
        let output = support::nia_command()
            .args([
                "build",
                "--progress",
                mode,
                "--print-cache",
                "-j",
                jobs,
                "--root",
            ])
            .arg(root)
            .output_timeout_in_session("warm progress cache comparison");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cache_text = String::from_utf8_lossy(&output.stdout);
        assert!(cache_text.contains("action cache:"), "{cache_text}");
        assert!(
            cache_text.contains(": hit; publication=restored; outputs="),
            "{cache_text}"
        );
        assert!(
            !cache_text.contains("Miss(") && !cache_text.contains("Hit"),
            "cache text must use stable user-facing outcomes: {cache_text}"
        );
        assert!(!output.stderr.contains(&0x1b));
        if let Some(baseline) = &baseline_cache {
            assert_eq!(&output.stdout, baseline);
        } else {
            baseline_cache = Some(output.stdout.clone());
        }
        eprintln!(
            "warm sample={sample}, progress={mode}, jobs={jobs}: {}ms, stderr={} bytes",
            started.elapsed().as_millis(),
            output.stderr.len()
        );
    }
    let quiet = support::nia_command()
        .args(["build", "-q", "--print-plan", "--root"])
        .arg(&on_root)
        .output_timeout_in_session("quiet preserves requested plan output");
    assert!(quiet.status.success());
    assert!(quiet.stderr.is_empty());
    assert!(!quiet.stdout.is_empty());
    let preview = support::nia_command()
        .args(["build", "--dry-run", "--progress=plain", "--root"])
        .arg(&on_root)
        .output_timeout_in_session("dry run reports configuration instead of execution");
    assert!(preview.status.success());
    let stderr = String::from_utf8_lossy(&preview.stderr);
    assert!(
        stderr.contains("Configured build"),
        "dry-run progress: {stderr}"
    );
    assert!(!stderr.contains("executed"));

    // Named selection must preserve both the plan and the selected cache records.
    let mut selected_cache = None;
    for (root, mode, jobs) in [(&off_root, "none", "1"), (&on_root, "json", "2")] {
        let selected = support::nia_command()
            .args([
                "--diagnostics-format=json",
                "build",
                "first",
                "--progress",
                mode,
                "--print-cache",
                "-j",
                jobs,
                "--root",
            ])
            .arg(root)
            .output_timeout_in_session("named selection with progress off/on");
        assert!(
            selected.status.success(),
            "{}",
            String::from_utf8_lossy(&selected.stderr)
        );
        if let Some(cache) = &selected_cache {
            assert_eq!(&selected.stdout, cache);
            let records = parse_json_lines(&selected.stderr);
            assert!(
                records
                    .iter()
                    .any(|record| record["event"] == "selection-ready" && record["total"] == 1)
            );
        } else {
            selected_cache = Some(selected.stdout);
        }
    }

    // A successful lookup is not a successful action if restoring the output fails.
    for root in [&off_root, &on_root] {
        let output = root.join("build/out/first.txt");
        std::fs::remove_file(&output).unwrap();
        std::fs::create_dir(&output).unwrap();
    }
    let mut failure_diagnostic = None;
    for (root, mode, verbose) in [
        (&off_root, "none", "-v"),
        (&on_root, "json", "-v"),
        (&on_root, "auto", "-q"),
    ] {
        let failure = support::nia_command()
            .args([
                "--diagnostics-format=json",
                "build",
                "first",
                "--progress",
                mode,
                verbose,
                "--root",
            ])
            .arg(root)
            .output_timeout_in_session("failed cached output restore with progress off/on");
        assert!(!failure.status.success());
        assert!(failure.stdout.is_empty());
        let records = parse_json_lines(&failure.stderr);
        let diagnostic = records
            .iter()
            .find(|record| record["kind"] != "build-progress")
            .expect("failure diagnostic");
        let code = &diagnostic["diagnostics"][0]["code"];
        // Physical roots differ; compare the stable error code.
        if let Some(expected) = &failure_diagnostic {
            assert_eq!(code, expected);
        } else {
            assert!(code.is_string(), "{diagnostic}");
            failure_diagnostic = Some(code.clone());
        }
        if mode == "json" && verbose == "-v" {
            assert!(
                records
                    .iter()
                    .any(|record| record["event"] == "action-cache-decision"
                        && record["cache"]["status"] == "hit")
            );
            assert!(
                records
                    .iter()
                    .any(|record| record["event"] == "action-finished"
                        && record["outcome"] == "failed"
                        && record["cache"]["status"] == "hit")
            );
            assert!(
                records
                    .iter()
                    .any(|record| record["event"] == "build-failed")
            );
        } else {
            assert!(
                records
                    .iter()
                    .all(|record| record["kind"] != "build-progress")
            );
        }
        assert!(root.join("build/out/first.txt").is_dir());
    }
}

#[test]
fn progress_long_parallel_actions_preserve_products_and_report_resources() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let root = nia_test_support::test_dir("progress-long-parallel");
    let command = if cfg!(windows) {
        r#"build::ExternalCommandOptions::search(&"powershell.exe").withArguments(&[
            build::CommandArgument::literal(&"-NoProfile"),
            build::CommandArgument::literal(&"-Command"),
            build::CommandArgument::literal(&"Start-Sleep -Milliseconds 600"),
        ])"#
    } else {
        r#"build::ExternalCommandOptions::search(&"sh").withArguments(&[
            build::CommandArgument::literal(&"-c"),
            build::CommandArgument::literal(&"sleep 0.6"),
        ])"#
    };
    std::fs::write(root.join("build.nia"), format!(r#"using std::build;
pub fun build(b: &mut build::Build) build::Error!() {{
    let first = b.addExternalCommandStep(&"first", {command}.withResourceClass(build::ActionResourceClass::Io)).?;
    let second = b.addExternalCommandStep(&"second", {command}.withResourceClass(build::ActionResourceClass::Io)).?;
    let all = b.addGeneratedFileStep(&"all", build::BuildPathView::init(&"result.txt"), &b"result\n"[..]).?;
    b.dependOn(all, first).?;
    b.dependOn(all, second).?;
    b.setDefaultStep(all).?;
    !()
}}
"#)).unwrap();
    let mut plans = std::collections::BTreeMap::new();
    for (step, json) in [
        ("first", false),
        ("first", true),
        ("all", false),
        ("all", true),
    ] {
        let configured = support::nia_command()
            .args([
                if json {
                    "--diagnostics-format=json"
                } else {
                    "--diagnostics-format=text"
                },
                "build",
                step,
                "--dry-run",
                "--print-plan",
                "--progress=none",
                "--root",
            ])
            .arg(&root)
            .output_timeout_in_session("configure long progress workload");
        assert!(
            configured.status.success(),
            "{}",
            String::from_utf8_lossy(&configured.stderr)
        );
        plans.insert((step, json), configured.stdout);
    }
    // This file is the configure-runner product. Cache reuse need not publish
    // it again when another selection becomes active; compare the active plan
    // through the public inspection above, and preserve this warm product too.
    let plan_file = std::fs::read(root.join("build/out/build-plan.bin")).unwrap();
    // One pass over every selection, renderer, and parallelism; the samples
    // are compared with the dry-run plans, not with each other.
    for (step, mode, jobs) in [
        ("first", "none", "1"),
        ("first", "plain", "1"),
        ("first", "json", "1"),
        ("all", "none", "1"),
        ("all", "plain", "1"),
        ("all", "json", "1"),
        ("all", "none", "2"),
        ("all", "plain", "2"),
        ("all", "json", "2"),
    ] {
        let output = support::nia_command()
            .args([
                "build",
                step,
                "--progress",
                mode,
                "--print-plan",
                "-vvv",
                "-j",
                jobs,
                "--root",
            ])
            .arg(&root)
            .output_timeout_in_session("long parallel progress acceptance sample");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if mode == "json" {
            let mut actual: serde_json::Value =
                serde_json::from_slice(&output.stdout).expect("parse printed build plan");
            let mut expected: serde_json::Value =
                serde_json::from_slice(&plans[&(step, true)]).expect("parse dry-run build plan");
            for plan in [&mut actual, &mut expected] {
                plan["invocation"]
                    .as_object_mut()
                    .unwrap()
                    .remove("configure_cache");
                plan["invocation"]
                    .as_object_mut()
                    .unwrap()
                    .remove("configure_result");
                plan["invocation"]
                    .as_object_mut()
                    .unwrap()
                    .remove("runner_executable_cache");
            }
            assert_eq!(actual, expected);
        } else {
            let stable_lines = |bytes: &[u8]| {
                String::from_utf8_lossy(bytes)
                    .lines()
                    .filter(|line| {
                        !line.starts_with("  configuration:")
                            && !line.starts_with("  configure cache candidate:")
                            && !line.starts_with("  configure result:")
                            && !line.starts_with("  runner executable cache:")
                    })
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                stable_lines(&output.stdout),
                stable_lines(&plans[&(step, false)]),
            );
        }
        assert!(!output.stderr.contains(&0x1b));
        if step == "all" {
            assert_eq!(
                std::fs::read(root.join("build/out/result.txt")).unwrap(),
                b"result\n"
            );
        }
        assert_eq!(
            std::fs::read(root.join("build/out/build-plan.bin")).unwrap(),
            plan_file
        );
        if mode == "none" {
            assert!(output.stderr.is_empty());
        } else if mode == "json" {
            let records = parse_json_lines(&output.stderr);
            assert_eq!(
                records
                    .iter()
                    .filter(|record| record["event"] == "action-finished")
                    .count(),
                if step == "all" { 3 } else { 1 }
            );
            assert_eq!(
                records
                    .iter()
                    .filter(|record| record["event"] == "action-finished"
                        && record["outcome"] == "executed")
                    .count(),
                if step == "all" { 2 } else { 1 }
            );
            assert!(
                records.iter().any(
                    |record| record["event"] == "resource-usage-changed" && record["used"] == 0
                )
            );
        }
    }
}

#[test]
fn configured_build_fixtures_have_independent_tests() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/build");
    let configured = case_directories(&fixtures, "build")
        .into_iter()
        .map(|path| {
            path.file_name()
                .expect("build case directory has a name")
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        configured,
        BUILD_CASE_NAMES
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>()
    );
}

#[test]
fn configure_timing_distinguishes_runner_execution_from_plan_reuse() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/build/runner-only");
    let workspace = nia_test_support::test_dir("configure-timing");
    copy_case_tree(&fixture, &workspace);
    for cold in [true, false] {
        let output = support::nia_command()
            .args([
                "build",
                "--timings=detail",
                "--timings-format=json",
                "--root",
            ])
            .arg(&workspace)
            .output_timeout_in_session("measure configure timing");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        let reports = stderr
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|report| {
                report["counters"]
                    .get("build.runner_compilations")
                    .is_some()
            })
            .collect::<Vec<_>>();
        assert_eq!(reports.len(), 1, "{stderr}");
        let report = &reports[0];
        let stages = report["timings"].as_array().unwrap();
        for (name, count) in [
            ("build_configure_plan", 1),
            ("build_run_runner", usize::from(cold)),
        ] {
            assert_eq!(
                stages
                    .iter()
                    .filter(|stage| stage["kind"] == "stage" && stage["name"] == name)
                    .count(),
                count,
                "{stderr}"
            );
        }
        for (counter, expected) in [
            ("build.runner_executions", u64::from(cold)),
            ("build.configure_cache_hits", u64::from(!cold)),
            ("build.configure_cache_misses", u64::from(cold)),
        ] {
            assert_eq!(
                report["counters"][counter].as_u64().unwrap_or(0),
                expected,
                "{stderr}"
            );
        }
    }
    assert_no_transient_runner_files(&workspace);
}

#[test]
fn optional_and_lazy_dependencies_are_loaded_only_from_imported_sources() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let root = nia_test_support::test_dir("optional-lazy-dependency-loading");
    for package in ["optional", "lazy"] {
        std::fs::create_dir_all(root.join(package).join("src")).unwrap();
        std::fs::write(
            root.join(package).join("nia.toml"),
            format!(
                "[package]\nname = \"{package}\"\nversion = \"1.0.0\"\n[library]\npath = \"src/root.nia\"\n"
            ),
        )
        .unwrap();
    }
    std::fs::write(
        root.join("optional/src/root.nia"),
        "pub fun value() i32 { 7 }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("lazy/src/root.nia"),
        "this is intentionally invalid Nia source\n",
    )
    .unwrap();
    std::fs::write(
        root.join("nia.toml"),
        "[package]\nname = \"scope-root\"\nversion = \"1.0.0\"\n[optional-dependencies]\noptional = { path = \"optional\" }\n[lazy-dependencies]\nlazy = { path = \"lazy\" }\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/main.nia"),
        "using std::process;\npub fun main(init: process::Init) process::ExitCode!() { _ = init; !() }\n",
    )
    .unwrap();

    let build_script = |with_optional_import: bool| {
        let imports = if with_optional_import {
            r#"let imports = [build::ModuleImport::fromPackage(
        &"optionalAlias",
        optional,
        fs::PathView::init(&"src/root.nia"),
    )];
    let rootModule = b.addModule(
        build::ModuleOptions::init(&"app", fs::PathView::init(&"src/main.nia"))
            .withImports(&imports[..]),
    ).?;"#
        } else {
            r#"let rootModule = b.addModule(
        build::ModuleOptions::init(&"app", fs::PathView::init(&"src/main.nia")),
    ).?;"#
        };
        format!(
            r#"using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {{
    let optional = b.addDependency(&"optional").?;
    _ = b.addDependency(&"lazy").?;
    {imports}
    let executable = b.addExecutable(build::ExecutableOptions::init(&"app", rootModule)).?;
    let check = b.addCheckExecutableStep(&"check", executable).?;
    b.setDefaultStep(check).?;
    !()
}}
"#
        )
    };

    std::fs::write(root.join("build.nia"), build_script(false)).unwrap();
    let unused = support::nia_command()
        .args(["build", "--root"])
        .arg(&root)
        .output_timeout_in_session("build with unimported optional and lazy dependencies");
    assert!(
        unused.status.success(),
        "{}",
        String::from_utf8_lossy(&unused.stderr)
    );

    std::fs::write(
        root.join("src/main.nia"),
        "using optionalAlias;\nusing std::process;\npub fun main(init: process::Init) process::ExitCode!() { _ = init; _ = optionalAlias::value(); !() }\n",
    )
    .unwrap();
    std::fs::write(root.join("build.nia"), build_script(true)).unwrap();
    let selected = support::nia_command()
        .args(["build", "--root"])
        .arg(&root)
        .output_timeout_in_session("build with an explicitly imported optional dependency");
    assert!(
        selected.status.success(),
        "{}",
        String::from_utf8_lossy(&selected.stderr)
    );

    std::fs::write(
        root.join("lazy/src/root.nia"),
        "pub fun value() i32 { 9 }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.nia"),
        "using lazy;\nusing std::process;\npub fun main(init: process::Init) process::ExitCode!() { _ = init; _ = lazy::value(); !() }\n",
    )
    .unwrap();
    std::fs::write(root.join("build.nia"), build_script(false)).unwrap();
    let lazy_selected = support::nia_command()
        .args(["build", "--root"])
        .arg(&root)
        .output_timeout_in_session("build with an imported lazy dependency");
    assert!(
        lazy_selected.status.success(),
        "{}",
        String::from_utf8_lossy(&lazy_selected.stderr)
    );
}

fn run_build_case(name: &str) {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/build");
    let case_root = fixtures.join(name);
    assert!(case_root.is_dir(), "build case {name:?} is not a fixture");
    let mut manifest = CaseManifest::load(&case_root);
    let manifest_path = manifest.path().to_owned();
    let mode = manifest.required("mode");
    let workload = match manifest.required("resource").as_str() {
        "compiler" => TestWorkload::Compiler,
        "build" => TestWorkload::Build,
        resource => panic!("unknown build case resource {resource:?}"),
    };
    let _resources = nia_test_support::acquire_test_resources(workload);
    let contract = manifest.required("contract");
    let step = manifest.required("step");
    let workspace = nia_test_support::test_dir(&format!("build-case-{name}"));
    copy_case_tree(&case_root, &workspace);

    let mut command = support::nia_command();
    command.arg("build");
    if contract == "step-order" {
        command.arg("--jobs=1");
    }
    if contract == "module-optimization" {
        command.arg("-Os");
    }
    let command_root = if mode == "configured-build-success" {
        command.arg("--timings=detail").arg("--timings-format=json");
        let nested = workspace.join("src/nested");
        std::fs::create_dir_all(&nested).expect("create nested build case directory");
        let command_directory = workspace
            .parent()
            .expect("temporary workspace has a parent");
        command.current_dir(command_directory);
        nested
            .strip_prefix(command_directory)
            .expect("nested package path is below command directory")
            .to_path_buf()
    } else {
        workspace.to_path_buf()
    };
    if step != "default" {
        command.arg(&step);
    }
    let output = command
        .arg("--root")
        .arg(&command_root)
        .output_timeout_in_session("run build metadata case");

    match mode.as_str() {
        "dependency-success" => {
            manifest.finish();
            assert_dependency_success(&contract, &workspace, &output);
        }
        "runner-error" => {
            let runner_status = manifest.required_i32("runner-status");
            let forbidden = manifest.required("forbidden");
            manifest.finish();
            let forbidden = fixture_path_or_none(&manifest_path, &workspace, forbidden);
            assert_runner_error(
                &contract,
                runner_status,
                &workspace,
                &forbidden,
                &output,
                "text",
            );
            for (list, format) in [(false, "json"), (true, "text"), (true, "json")] {
                // Listing selects no step, so it needs neither a known step
                // nor a default. Definition/graph validation applies to both.
                if list && matches!(contract.as_str(), "unknown-step" | "missing-default") {
                    continue;
                }
                let mut command = support::nia_command();
                command
                    .arg("build")
                    .arg(format!("--diagnostics-format={format}"));
                if list {
                    command.arg("--list");
                } else if step != "default" {
                    command.arg(&step);
                }
                let output = command
                    .arg("--root")
                    .arg(&command_root)
                    .output_timeout_in_session("report invalid build plan");
                assert_runner_error(
                    &contract,
                    runner_status,
                    &workspace,
                    &forbidden,
                    &output,
                    format,
                );
            }
        }
        "missing-script-error" => {
            manifest.finish();
            assert!(!output.status.success());
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("could not find `build.nia`"), "{stderr}");
            assert!(
                stderr.contains(workspace.to_string_lossy().as_ref()),
                "{stderr}"
            );
        }
        "configured-build-success" => {
            let contracts = manifest.required_list("contracts");
            manifest.finish();
            assert_configured_build_success(&contracts, &workspace, &output);
        }
        "module-check-success" => {
            manifest.finish();
            assert!(
                output.status.success(),
                "stderr:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_module_optimization_inheritance(&workspace);
        }
        _ => panic!(
            "unknown build case mode {mode:?} in {}",
            manifest_path.display()
        ),
    }
}

fn assert_module_optimization_inheritance(workspace: &Path) {
    let plan = nia_build::read_build_plan(&workspace.join("build/out/build-plan.bin"))
        .expect("decode optimization build plan");
    assert_eq!(plan.modules().len(), 2);
    assert_eq!(plan.modules()[0].key.name(), "app");
    assert_eq!(
        plan.modules()[0].optimization,
        nia_build::OptimizationMode::Os
    );
    assert_eq!(plan.modules()[1].key.name(), "override");
    assert_eq!(
        plan.modules()[1].optimization,
        nia_build::OptimizationMode::O0
    );
}

fn assert_configured_build_success(
    contracts: &[String],
    workspace: &Path,
    output: &std::process::Output,
) {
    // The build runner reports physical paths after canonicalization. macOS
    // exposes `/private` for temporary directories even when the test creates
    // them through `/var`, so compare against the same canonical identity.
    let reported_workspace: Cow<'_, Path> = if cfg!(target_os = "macos") {
        Cow::Owned(
            workspace
                .canonicalize()
                .expect("canonicalize configured build workspace"),
        )
    } else {
        Cow::Borrowed(workspace)
    };
    assert_eq!(
        contracts,
        [
            "timings-json",
            "runner-context",
            "configured-output",
            "module-imports",
            "build-plan",
            "external-run",
            "staged-output",
        ]
    );
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for expected in [
        format!("root={}\n", reported_workspace.display()),
        format!(
            "cache={}\n",
            reported_workspace.join("build").join("cache").display()
        ),
        format!("toolchain={}\n", env!("CARGO_BIN_EXE_nia")),
        "run=roadmap\n".to_string(),
        "host=roadmap\n".to_string(),
    ] {
        assert!(
            stdout.contains(&expected),
            "missing {expected:?} in {stdout}"
        );
    }
    let expected_build = format!(
        "build={}",
        reported_workspace.join("build").join("out").display()
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.trim_end_matches(['\\', '/']) == expected_build),
        "missing {expected_build:?} in {stdout}"
    );
    let compatibility = format!("\"protocol_epoch\":{}", nia_compat::PROTOCOL_EPOCH);
    let json_lines = stderr
        .lines()
        .filter(|line| line.starts_with('{') && line.contains(&compatibility))
        .collect::<Vec<_>>();
    assert!(!json_lines.is_empty(), "{stderr}");
    assert!(
        json_lines
            .iter()
            .any(|line| line.contains("\"build.runner_executions\":1")),
        "{stderr}"
    );
    assert!(
        json_lines
            .iter()
            .any(|line| line.contains("\"llvm.units\"")),
        "{stderr}"
    );
    for counter in [
        "compiler.backend_modules",
        "compiler.checked_modules",
        "compiler.link_inputs",
        "compiler.loaded_modules",
        "compiler.monomorphized_instances",
        "compiler.semantic_selected_modules",
        "query.dependency_edges",
        "query.max_dependency_fanout",
        "query.waits",
    ] {
        assert!(
            json_lines
                .iter()
                .any(|line| line.contains(&format!("\"{counter}\""))),
            "missing {counter} in {stderr}"
        );
    }
    assert!(
        json_lines
            .iter()
            .any(|line| line.contains("\"build.steps_executed\":12")),
        "{stderr}"
    );
    assert!(
        json_lines
            .iter()
            .any(|line| line.contains("\"build.actions_executed\":12")),
        "{stderr}"
    );
    assert!(
        !stderr.lines().any(|line| line.starts_with("error:")),
        "{stderr}"
    );
    assert!(workspace.join("build/tmp").is_dir());
    assert!(workspace.join("build/cache").is_dir());
    assert_no_transient_runner_files(workspace);
    let plan_path = workspace.join("build/out/build-plan.bin");
    let plan = nia_build::read_build_plan(&plan_path).expect("decode published build plan");
    assert_eq!(
        plan.protocol_epoch(),
        nia_compat::formats::BUILD_PLAN.protocol_epoch
    );
    assert_eq!(plan.root_package().as_str(), "root");
    assert_eq!(plan.packages().len(), 2);
    let assets = plan
        .packages()
        .iter()
        .find(|package| package.key.as_str() == "assets")
        .expect("manifest dependency");
    assert_eq!(
        assets.source,
        nia_build::PlanPackageSource::Dependency("assets".into())
    );
    let root = plan
        .packages()
        .iter()
        .find(|package| package.key.as_str() == "root")
        .expect("root package");
    assert_eq!(
        root.source,
        nia_build::PlanPackageSource::Local(String::new())
    );
    assert_eq!(plan.modules().len(), 4);
    assert_eq!(plan.modules()[0].key.name(), "app");
    assert_eq!(
        plan.modules()[0].optimization,
        nia_build::OptimizationMode::O0
    );
    assert_eq!(
        plan.modules()[0].root_source.protocol_path(),
        "src/main.nia"
    );
    assert!(matches!(
        plan.modules()[0].root_source.root(),
        nia_build::LogicalPathRoot::Package(package) if package.as_str() == "assets"
    ));
    assert_eq!(plan.modules()[1].key.name(), "archive");
    assert_eq!(
        plan.modules()[1].root_source.protocol_path(),
        "generated/helper.nia"
    );
    assert!(matches!(
        plan.modules()[1].root_source.root(),
        nia_build::LogicalPathRoot::Build
    ));
    assert_eq!(plan.modules()[2].key.name(), "host-tool");
    assert_eq!(
        plan.modules()[2].root_source.protocol_path(),
        "src/host_tool.nia"
    );
    assert_eq!(plan.modules()[3].key.name(), "worker");
    assert_eq!(
        plan.modules()[3].optimization,
        nia_build::OptimizationMode::O0
    );
    assert_eq!(
        plan.modules()[3].root_source.protocol_path(),
        "generated/worker.nia"
    );
    assert!(matches!(
        plan.modules()[3].root_source.root(),
        nia_build::LogicalPathRoot::Build
    ));
    assert_eq!(plan.modules()[0].imports.len(), 2);
    let helper_import = plan.modules()[0]
        .imports
        .iter()
        .find(|import| import.name == "helper")
        .expect("generated helper import");
    assert_eq!(helper_import.path.protocol_path(), "generated/helper.nia");
    assert!(matches!(
        helper_import.path.root(),
        nia_build::LogicalPathRoot::Build
    ));
    let asset_helper_import = plan.modules()[0]
        .imports
        .iter()
        .find(|import| import.name == "assetHelper")
        .expect("package helper import");
    assert_eq!(asset_helper_import.path.protocol_path(), "helper.nia");
    assert!(matches!(
        asset_helper_import.path.root(),
        nia_build::LogicalPathRoot::Package(package) if package.as_str() == "assets"
    ));
    assert_eq!(plan.artifacts().len(), 6);
    let app_artifact = plan
        .artifacts()
        .iter()
        .find(|artifact| artifact.key.name() == "app")
        .expect("app artifact");
    assert_eq!(app_artifact.root_module.name(), "app");
    assert_eq!(app_artifact.output.protocol_path(), "custom-app");
    assert_eq!(app_artifact.runtime, nia_build::Runtime::Freestanding);
    let bare_artifact = plan
        .artifacts()
        .iter()
        .find(|artifact| artifact.key.name() == "bare")
        .expect("bare artifact");
    assert_eq!(bare_artifact.root_module.name(), "app");
    assert_eq!(bare_artifact.runtime, nia_build::Runtime::Bare);
    let host_artifact = plan
        .artifacts()
        .iter()
        .find(|artifact| artifact.key.name() == "host-tool")
        .expect("host artifact");
    assert_eq!(host_artifact.root_module.name(), "host-tool");
    assert_eq!(host_artifact.output.protocol_path(), "custom-host-tool");
    let object_artifact = plan
        .artifacts()
        .iter()
        .find(|artifact| artifact.key.name() == "objects")
        .expect("object artifact");
    assert_eq!(object_artifact.root_module.name(), "app");
    assert_eq!(object_artifact.output.protocol_path(), "custom-objects");
    assert_eq!(object_artifact.kind, nia_build::PlanArtifactKind::ObjectSet);
    let archive_artifact = plan
        .artifacts()
        .iter()
        .find(|artifact| artifact.key.name() == "archive")
        .expect("static archive artifact");
    assert_eq!(archive_artifact.root_module.name(), "archive");
    assert_eq!(archive_artifact.output.protocol_path(), "libconfigured.a");
    assert_eq!(
        archive_artifact.kind,
        nia_build::PlanArtifactKind::StaticArchive
    );
    let worker_artifact = plan
        .artifacts()
        .iter()
        .find(|artifact| artifact.key.name() == "worker")
        .expect("worker artifact");
    assert_eq!(worker_artifact.root_module.name(), "worker");
    assert_eq!(worker_artifact.output.protocol_path(), "custom-worker");
    assert_eq!(plan.actions().len(), 13);
    assert!(matches!(
        &plan
            .actions()
            .iter()
            .find(|action| action.key.name() == "build")
            .expect("app emit action")
            .kind,
        nia_build::ActionKind::CompilerEmit { static_archives, .. }
            if static_archives.iter().map(nia_build::ArtifactKey::name).collect::<Vec<_>>()
                == ["archive"]
    ));
    assert!(matches!(
        plan.actions()
            .iter()
            .find(|action| action.key.name() == "check")
            .expect("app check action")
            .kind,
        nia_build::ActionKind::CompilerCheck { .. }
    ));
    let run_action = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "run")
        .expect("run action");
    match &run_action.kind {
        nia_build::ActionKind::ExternalCommand(nia_build::CommandAction {
            resource_class,
            environment_policy,
            cache_policy,
            program,
            arguments,
            working_directory,
            environment,
            inputs,
            outputs,
            ..
        }) => {
            assert_eq!(
                *resource_class,
                nia_build::ActionResourceClass::Conservative
            );
            assert_eq!(
                *environment_policy,
                nia_build::CommandEnvironmentPolicy::Inherit
            );
            assert_eq!(*cache_policy, nia_build::CommandCachePolicy::Uncacheable);
            assert!(matches!(
                program,
                nia_build::CommandProgram::Path(path)
                    if matches!(
                        path.root(),
                        nia_build::LogicalPathRoot::Artifact(artifact)
                            if artifact.name() == "app"
                    ) && path.components().is_empty()
            ));
            assert_eq!(
                arguments,
                &[nia_build::CommandArgument::Literal("roadmap".to_string())]
            );
            assert!(matches!(
                working_directory.root(),
                nia_build::LogicalPathRoot::Package(package) if package.as_str() == "root"
            ));
            assert!(working_directory.components().is_empty());
            assert!(environment.is_empty());
            assert!(inputs.is_empty());
            assert!(outputs.is_empty());
        }
        other => panic!("expected external run action, found {other:?}"),
    }
    let tool_action = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "tool")
        .expect("tool action");
    match &tool_action.kind {
        nia_build::ActionKind::ExternalCommand(nia_build::CommandAction {
            resource_class,
            environment_policy,
            cache_policy,
            program,
            arguments,
            working_directory,
            environment,
            inputs,
            outputs,
            ..
        }) => {
            assert_eq!(*resource_class, nia_build::ActionResourceClass::Io);
            assert_eq!(
                *environment_policy,
                nia_build::CommandEnvironmentPolicy::Clear
            );
            assert_eq!(*cache_policy, nia_build::CommandCachePolicy::ToolAndInputs);
            if cfg!(windows) {
                assert_eq!(
                    program,
                    &nia_build::CommandProgram::Search("cmd.exe".to_string())
                );
                assert_eq!(arguments.len(), 12);
                assert_eq!(
                    arguments[0],
                    nia_build::CommandArgument::Literal("/d".to_string())
                );
                assert_eq!(
                    arguments[1],
                    nia_build::CommandArgument::Literal("/s".to_string())
                );
                assert_eq!(
                    arguments[2],
                    nia_build::CommandArgument::Literal("/c".to_string())
                );
                assert!(matches!(
                    arguments[3],
                    nia_build::CommandArgument::InputPath(ref path)
                        if matches!(
                            path.root(),
                            nia_build::LogicalPathRoot::Package(package)
                                if package.as_str() == "assets"
                        ) && path.protocol_path() == "tool.cmd"
                ));
            } else {
                assert_eq!(
                    program,
                    &nia_build::CommandProgram::Search("sh".to_string())
                );
                assert_eq!(
                    arguments,
                    &[
                        nia_build::CommandArgument::Literal("-c".to_string()),
                        nia_build::CommandArgument::Literal(
                            "test \"$MODE\" = fixture && test -s \"$4\" && test -s \"$5\" && test -d \"$6\" && test -s \"$7\" && tr a-z A-Z < \"$1\" > \"$2\" && printf 'source=tool-input\\n' > \"$3\""
                                .to_string()
                        ),
                        nia_build::CommandArgument::Literal("nia-build-tool".to_string()),
                        nia_build::CommandArgument::InputPath(inputs[0].clone()),
                        nia_build::CommandArgument::OutputPath(outputs[1].clone()),
                        nia_build::CommandArgument::OutputPath(outputs[0].clone()),
                        nia_build::CommandArgument::InputPath(inputs[1].clone()),
                        nia_build::CommandArgument::InputPath(inputs[4].clone()),
                        nia_build::CommandArgument::InputPath(inputs[3].clone()),
                        nia_build::CommandArgument::InputPath(inputs[2].clone()),
                    ]
                );
            }
            assert!(matches!(
                working_directory.root(),
                nia_build::LogicalPathRoot::Package(package) if package.as_str() == "root"
            ));
            assert!(working_directory.components().is_empty());
            if cfg!(windows) {
                assert_eq!(
                    environment,
                    &[
                        nia_build::EnvironmentInput {
                            name: "MODE".to_string(),
                            value: Some("fixture".to_string()),
                        },
                        nia_build::EnvironmentInput {
                            name: "SYSTEMROOT".to_string(),
                            value: Some("C:\\Windows".to_string()),
                        },
                    ]
                );
            } else {
                assert_eq!(
                    environment,
                    &[nia_build::EnvironmentInput {
                        name: "MODE".to_string(),
                        value: Some("fixture".to_string()),
                    }]
                );
            }
            assert!(matches!(
                inputs[0].root(),
                nia_build::LogicalPathRoot::Package(package) if package.as_str() == "assets"
            ));
            assert_eq!(inputs[0].protocol_path(), "tool-input.txt");
            if cfg!(windows) {
                assert_eq!(inputs.len(), 6);
                assert!(matches!(
                    inputs[1].root(),
                    nia_build::LogicalPathRoot::Package(package) if package.as_str() == "assets"
                ));
                assert_eq!(inputs[1].protocol_path(), "tool.cmd");
                assert!(matches!(
                    inputs[2].root(),
                    nia_build::LogicalPathRoot::Artifact(artifact) if artifact.name() == "app"
                ));
                assert!(inputs[2].components().is_empty());
                assert!(matches!(
                    inputs[3].root(),
                    nia_build::LogicalPathRoot::Artifact(artifact) if artifact.name() == "archive"
                ));
                assert!(inputs[3].components().is_empty());
                assert!(matches!(
                    inputs[4].root(),
                    nia_build::LogicalPathRoot::Artifact(artifact) if artifact.name() == "objects"
                ));
                assert!(inputs[4].components().is_empty());
                assert!(matches!(
                    inputs[5].root(),
                    nia_build::LogicalPathRoot::Artifact(artifact) if artifact.name() == "worker"
                ));
                assert!(inputs[5].components().is_empty());
            } else {
                assert_eq!(inputs.len(), 5);
                assert!(matches!(
                    inputs[1].root(),
                    nia_build::LogicalPathRoot::Artifact(artifact) if artifact.name() == "app"
                ));
                assert!(inputs[1].components().is_empty());
                assert!(matches!(
                    inputs[2].root(),
                    nia_build::LogicalPathRoot::Artifact(artifact) if artifact.name() == "archive"
                ));
                assert!(inputs[2].components().is_empty());
                assert!(matches!(
                    inputs[3].root(),
                    nia_build::LogicalPathRoot::Artifact(artifact) if artifact.name() == "objects"
                ));
                assert!(inputs[3].components().is_empty());
                assert!(matches!(
                    inputs[4].root(),
                    nia_build::LogicalPathRoot::Artifact(artifact) if artifact.name() == "worker"
                ));
                assert!(inputs[4].components().is_empty());
            }
            assert_eq!(outputs.len(), 2);
            assert!(matches!(
                outputs[0].root(),
                nia_build::LogicalPathRoot::Build
            ));
            assert_eq!(outputs[0].protocol_path(), "transformed.meta");
            assert_eq!(outputs[1].protocol_path(), "transformed.txt");
        }
        other => panic!("expected staged external tool action, found {other:?}"),
    }
    let host_emit_action = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "host-tool")
        .expect("host tool emit action");
    assert!(matches!(
        &host_emit_action.kind,
        nia_build::ActionKind::CompilerEmit { artifact, target, .. }
            if artifact.name() == "host-tool" && target == plan.host_target()
    ));
    let archive_emit_action = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "archive")
        .expect("static archive emit action");
    assert!(matches!(
        &archive_emit_action.kind,
        nia_build::ActionKind::CompilerEmit { artifact, target, .. }
            if artifact.name() == "archive" && target == plan.artifact_target()
    ));
    let host_run_action = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "run-host-tool")
        .expect("host tool run action");
    assert!(matches!(
        &host_run_action.kind,
        nia_build::ActionKind::ExternalCommand(nia_build::CommandAction {
            program: nia_build::CommandProgram::Path(path),
            ..
        }) if matches!(
            path.root(),
            nia_build::LogicalPathRoot::Artifact(artifact) if artifact.name() == "host-tool"
        ) && path.components().is_empty()
    ));
    let install_action = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "install")
        .expect("install action");
    assert!(matches!(
        &install_action.kind,
        nia_build::ActionKind::InstallArtifact { artifact, destination }
            if artifact.name() == "app"
                && matches!(destination.root(), nia_build::LogicalPathRoot::Build)
                && destination.protocol_path() == "install/custom-app"
    ));
    let install_archive_action = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "install-archive")
        .expect("static archive install action");
    assert!(matches!(
        &install_archive_action.kind,
        nia_build::ActionKind::InstallArtifact { artifact, destination }
            if artifact.name() == "archive"
                && matches!(destination.root(), nia_build::LogicalPathRoot::Build)
                && destination.protocol_path() == "install/libconfigured.a"
    ));
    assert_eq!(plan.steps().len(), 13);
    let build_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "build")
        .expect("app emit step");
    assert_eq!(
        build_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["archive", "generate-helper"]
    );
    let check_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "check")
        .expect("app check step");
    assert_eq!(
        check_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["generate-helper"]
    );
    let run_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "run")
        .expect("run step");
    assert_eq!(
        run_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["build"]
    );
    let tool_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "tool")
        .expect("tool step");
    assert_eq!(
        tool_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        [
            "archive",
            "build",
            "install",
            "install-archive",
            "objects",
            "run",
            "run-host-tool",
            "worker"
        ]
    );
    let host_run_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "run-host-tool")
        .expect("host tool run step");
    assert_eq!(
        host_run_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["host-tool"]
    );
    let install_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "install")
        .expect("install step");
    assert_eq!(
        install_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["build"]
    );
    let install_archive_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "install-archive")
        .expect("static archive install step");
    assert_eq!(
        install_archive_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["archive"]
    );
    let worker_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "worker")
        .expect("worker emit step");
    assert_eq!(
        worker_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["generate-worker"]
    );
    let object_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "objects")
        .expect("object emit step");
    assert_eq!(
        object_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["generate-helper"]
    );
    let archive_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "archive")
        .expect("static archive emit step");
    assert_eq!(
        archive_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["generate-helper"]
    );
    assert_eq!(
        plan.default_step().map(nia_build::StepKey::name),
        Some("tool")
    );
    assert_eq!(
        plan.selected_step().map(nia_build::StepKey::name),
        Some("tool")
    );
    assert!(workspace.join("build/out/custom-app").is_file());
    assert!(workspace.join("build/out/install/custom-app").is_file());
    assert!(!workspace.join("build/out/app").exists());
    assert!(workspace.join("build/out/custom-worker").is_file());
    assert!(workspace.join("build/out/custom-host-tool").is_file());
    assert!(workspace.join("build/out/custom-objects").is_dir());
    assert!(
        std::fs::read_dir(workspace.join("build/out/custom-objects"))
            .unwrap()
            .any(|entry| entry.unwrap().path().is_file())
    );
    assert!(
        std::fs::read(workspace.join("build/out/libconfigured.a"))
            .unwrap()
            .starts_with(b"!<arch>\n")
    );
    assert!(
        std::fs::read(workspace.join("build/out/install/libconfigured.a"))
            .unwrap()
            .starts_with(b"!<arch>\n")
    );
    assert!(workspace.join("build/out/generated/helper.nia").is_file());
    assert!(workspace.join("build/out/generated/worker.nia").is_file());
    assert!(!workspace.join("build/out/worker").exists());
    let transformed = std::fs::read(workspace.join("build/out/transformed.txt")).unwrap();
    assert_eq!(
        transformed
            .strip_suffix(b"\r\n")
            .or_else(|| transformed.strip_suffix(b"\n")),
        Some(&b"EXTERNAL ROADMAP"[..])
    );
    let metadata = std::fs::read(workspace.join("build/out/transformed.meta")).unwrap();
    assert_eq!(
        metadata
            .strip_suffix(b"\r\n")
            .or_else(|| metadata.strip_suffix(b"\n")),
        Some(&b"source=tool-input"[..])
    );
    assert!(
        std::fs::read_dir(workspace.join("build/out"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".nia-command-"))
    );
    assert_eq!(
        Command::new(workspace.join("build/out/custom-app"))
            .arg("roadmap")
            .status_timeout("run configured build target")
            .code(),
        Some(0)
    );
    assert_eq!(
        Command::new(workspace.join("build/out/install/custom-app"))
            .arg("roadmap")
            .status_timeout("run installed configured build target")
            .code(),
        Some(0)
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(
            std::fs::metadata(workspace.join("build/out/install/custom-app"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
    }
    assert_eq!(
        Command::new(workspace.join("build/out/custom-worker"))
            .status_timeout("run configured worker artifact")
            .code(),
        Some(0)
    );
    assert_eq!(
        Command::new(workspace.join("build/out/custom-host-tool"))
            .arg("roadmap")
            .status_timeout("run configured host tool artifact")
            .code(),
        Some(0)
    );

    let check = support::nia_command()
        .arg("build")
        .arg("check")
        .arg("--root")
        .arg(workspace)
        .output_timeout_in_session("run configured build check step");
    assert!(
        check.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&check.stderr)
    );
    assert_no_transient_runner_files(workspace);
    let checked_plan = nia_build::read_build_plan(&plan_path).expect("decode replaced build plan");
    assert_eq!(
        checked_plan.selected_step().map(nia_build::StepKey::name),
        Some("check")
    );
    let checked_bytes = std::fs::read(&plan_path).expect("read checked plan bytes");
    let unknown = support::nia_command()
        .arg("build")
        .arg("does-not-exist")
        .arg("--root")
        .arg(workspace)
        .output_timeout_in_session("run unknown step after published plan");
    assert!(!unknown.status.success());
    assert_no_transient_runner_files(workspace);
    assert_eq!(
        std::fs::read(&plan_path).expect("read plan after rejected build"),
        checked_bytes,
        "a failed runner must not replace the last canonical plan"
    );
}

fn assert_dependency_success(contract: &str, workspace: &Path, output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(workspace.join("build/out/build-plan.bin").is_file());
    assert_no_transient_runner_files(workspace);
    match contract {
        "step-order" => assert!(output.stdout.is_empty()),
        "executable-dependency" => {
            assert!(output.stdout.is_empty());
            assert!(workspace.join("build/out/app").is_file());
        }
        _ => panic!("unknown dependency-success contract {contract:?}"),
    }
}

fn assert_runner_error(
    contract: &str,
    runner_status: i32,
    workspace: &Path,
    forbidden: &Option<std::path::PathBuf>,
    output: &std::process::Output,
    format: &str,
) {
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
    assert_no_transient_runner_files(workspace);
    assert!(!workspace.join("build/out/build-plan.bin").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.stdout.is_empty(), "{stderr}");
    assert!(!stderr.contains('\x1b'), "{stderr}");
    if format == "json" {
        let report: serde_json::Value =
            serde_json::from_str(&stderr).expect("complete build failure JSON");
        let diagnostics = report["diagnostics"].as_array().unwrap();
        assert_eq!(diagnostics.len(), 1, "{stderr}");
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic["code"], "E0703", "{stderr}");
        assert_eq!(diagnostic["kind"], "independent", "{stderr}");
        assert_eq!(diagnostic["severity"], "error", "{stderr}");
        assert_eq!(diagnostic["category"], "user", "{stderr}");
        assert_eq!(
            diagnostic["summary"], "build runner exited unsuccessfully",
            "{stderr}"
        );
        assert_eq!(diagnostic["path"], "<unknown>", "{stderr}");
        assert!(
            diagnostic["labels"].as_array().unwrap().is_empty(),
            "{stderr}"
        );
        assert!(
            diagnostic["notes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|note| note
                    .as_str()
                    .unwrap()
                    .starts_with("stderr tail (bounded):\n")
                    && note.as_str().unwrap().contains("build error:")),
            "{stderr}"
        );
        assert_eq!(report["summary"]["errors"], 1, "{stderr}");
        assert_eq!(report["summary"]["warnings"], 0, "{stderr}");
    } else {
        assert!(stderr.contains("error[E0703]"), "{stderr}");
    }
    assert!(stderr.contains("build runner"), "{stderr}");
    assert!(
        stderr.contains(&format!("exited with status {runner_status}")),
        "{stderr}"
    );
    if contract == "unknown-step" {
        assert!(stderr.contains("unknown build step `missing`"), "{stderr}");
    }
    if let Some(forbidden) = forbidden {
        assert!(
            !forbidden.exists(),
            "{} must not exist",
            forbidden.display()
        );
    }
}

fn assert_no_transient_runner_files(workspace: &Path) {
    let tmp_dir = workspace.join("build/tmp");
    if tmp_dir.is_dir() {
        let left = std::fs::read_dir(&tmp_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert!(left.is_empty(), "transient files left behind: {left:?}");
    }
}

fn fixture_path_or_none(
    manifest_path: &Path,
    workspace: &Path,
    value: String,
) -> Option<std::path::PathBuf> {
    (value != "none").then(|| workspace.join(fixture_relative_path(manifest_path, value)))
}
