// SPDX-License-Identifier: GPL-3.0-or-later
use std::{fs, process::Command};

mod support;

use nia_test_support::CommandExt;
use nia_test_support::TestWorkload;

#[test]
fn test_steps_are_listed_selected_and_reported_together() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let workspace = nia_test_support::test_dir("test-command-suites");
    fs::create_dir_all(workspace.join("tests")).expect("create test source directory");
    fs::write(
        workspace.join("build.nia"),
        r#"using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {
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

fun passingCase() test::Error!() {
    test::expectEqual(2, 2).?;
    !()
}

fun failingCase() test::Error!() {
    test::fail()
}

fun recordCaseResult(result: test::CaseResult) () {
    _ = result;
    ()
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let cases: [test::Case; 2] = [
        .init(&"passing", &passingCase),
        .init(&"failing", &failingCase),
    ];
    let first = cases[0].run();
    if not first.isPassed() {
        return process::ExitCode(9)!;
    }
    _ = first.name();
    let failed = cases[1].run();
    match failed.error() {
        ?error => match error {
            .AssertionFailed(location) => {
                if location.line() != 10 or location.column() != 5 {
                    return process::ExitCode(15)!;
                }
            },
            .ValuesEqual(_) => return process::ExitCode(16)!,
            .ValuesNotEqual(_) => return process::ExitCode(17)!,
        },
        null => return process::ExitCode(18)!,
    }
    let summary = test::Runner::init(&cases[..]).run();
    if summary.total() != 2 or summary.passed() != 1 or summary.failed() != 1
        or summary.skipped() != 0 or summary.isSuccessful()
    {
        return process::ExitCode(10)!;
    }
    let mut caseRuns: usize = 0;
    let stackCaseState = \[&mut caseRuns] -> {
        caseRuns.* += 1;
        test::expect(true)
    };
    let stackCase: &Fun() test::Error!() = &stackCaseState;
    let stackCases: [test::Case; 1] = [.init(&"stack-backed", stackCase)];
    let stackSummary = test::Runner::init(&stackCases[..]).run();
    if stackSummary.total() != 1 or stackSummary.passed() != 1 or caseRuns != 1 {
        return process::ExitCode(14)!;
    }
    let failFastCases: [test::Case; 2] = [
        .init(&"failing", &failingCase),
        .init(&"passing", &passingCase),
    ];
    let failFast = test::Runner::init(&failFastCases[..]).withFailFast().run();
    if failFast.total() != 2 or failFast.passed() != 0 or failFast.failed() != 1
        or failFast.skipped() != 1
    {
        return process::ExitCode(11)!;
    }
    let reported = test::Runner::init(&cases[..]).runWith(&recordCaseResult);
    if reported.total() != 2 or reported.passed() != 1 or reported.failed() != 1 {
        return process::ExitCode(12)!;
    }
    let mut reportedCount: usize = 0;
    let mut reportedFailures: usize = 0;
    let captured = test::Runner::init(&cases[..]).runWith(
        &mut \[&mut reportedCount, &mut reportedFailures] result: test::CaseResult -> {
            reportedCount.* += 1;
            if result.isFailed() {
                reportedFailures.* += 1;
            }
            ()
        },
    );
    if captured.total() != 2 or reportedCount != 2 or reportedFailures != 1 {
        return process::ExitCode(13)!;
    }
    !()
}
"#,
    )
    .expect("write passing suite");
    fs::write(
        workspace.join("tests/fail.nia"),
        r#"using std::process;
using std::io;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut buffer: [u8; 0] = [];
    let mut stderr = io::FileWriter::stderr(&mut buffer[..]);
    match stderr.writeAll(&b"failed-suite-output\n") {
        !ok => { _ = ok; },
        error! => { _ = error; return process::ExitCode(9)!; },
    }
    process::ExitCode(7)!
}
"#,
    )
    .expect("write failing suite");
    fs::write(
        workspace.join("tests/fail_second.nia"),
        r#"using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    process::ExitCode(8)!
}
"#,
    )
    .expect("write second failing suite");

    let listed = build_command(&workspace)
        .arg("--list")
        .output_timeout_in_session("list the package's steps");
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    assert_eq!(listed.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout),
        "fail                 test-executable\n\
         fail-second          test-executable\n\
         fail-second.compile  compiler-emit\n\
         fail.compile         compiler-emit\n\
         pass                 test-executable\n\
         pass.compile         compiler-emit\n\
         test                 aggregate\n"
    );

    let passing = build_command(&workspace)
        .arg("pass")
        .output_timeout_in_session("run one Nia test suite");
    assert!(
        passing.status.success(),
        "{}",
        String::from_utf8_lossy(&passing.stderr)
    );
    assert_eq!(passing.status.code(), Some(0));

    let all = build_command(&workspace)
        .args(["test", "--keep-going"])
        .output_timeout_in_session("run all Nia test suites");
    assert!(!all.status.success());
    assert_eq!(all.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&all.stderr);
    assert!(stderr.contains("2 test suite(s) failed"), "{stderr}");
    assert!(stderr.contains("fail:"), "{stderr}");
    assert!(stderr.contains("fail-second:"), "{stderr}");
    assert!(all.stdout.is_empty());
    assert!(stderr.contains("failed-suite-output"), "{stderr}");

    let json = build_command(&workspace)
        .args(["test", "--keep-going", "--diagnostics-format=json"])
        .output_timeout_in_session("report failed suites as JSON");
    assert_eq!(json.status.code(), Some(1));
    assert!(json.stdout.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&json.stderr).unwrap_or_else(|error| {
        panic!(
            "suite JSON report: {error}: {}",
            String::from_utf8_lossy(&json.stderr)
        )
    });
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1, "{report}");
    assert_eq!(diagnostics[0]["code"], "E0705", "{report}");
    assert_eq!(diagnostics[0]["kind"], "independent", "{report}");
    assert_eq!(
        diagnostics[0]["summary"], "2 test suite(s) failed",
        "{report}"
    );
    assert!(
        diagnostics[0]["labels"].as_array().unwrap().is_empty(),
        "{report}"
    );
    assert_eq!(report["summary"]["errors"], 1, "{report}");
    let notes = diagnostics[0]["notes"].as_array().unwrap();
    assert!(
        notes.iter().any(|note| note
            .as_str()
            .unwrap()
            .contains("test suite `fail`: stderr tail (bounded):\nfailed-suite-output")),
        "{report}"
    );
    assert!(
        notes.iter().any(|note| note
            .as_str()
            .unwrap()
            .contains("test suite `fail`: command exited with status")),
        "{report}"
    );

    // Without `--keep-going`, actions after the first failure are cancelled.
    let fail_fast = build_command(&workspace)
        .args(["test", "--jobs", "1"])
        .output_timeout_in_session("stop after first failing Nia test suite");
    assert!(!fail_fast.status.success());
    assert_eq!(fail_fast.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&fail_fast.stderr);
    assert!(stderr.contains("1 test suite(s) failed"), "{stderr}");
}

#[test]
fn executable_and_test_modules_can_share_one_package_import_set() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let workspace = nia_test_support::test_dir("test-and-executable-shared-package-imports");
    fs::create_dir_all(workspace.join("src")).expect("create executable source directory");
    fs::create_dir_all(workspace.join("tests")).expect("create test source directory");
    fs::create_dir_all(workspace.join("packages/foo")).expect("create foo package directory");
    fs::write(
        workspace.join("build.nia"),
        r#"using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {
    let foo = b.addPackage(build::PackageOptions::init(
        &"foo", fs::PathView::init(&"packages/foo"),
    )).?;
    let imports = [build::ModuleImport::fromPackage(
        &"foo", foo, fs::PathView::init(&"main.nia"),
    )];
    let appModule = b.addModule(build::ModuleOptions::init(
        &"app", fs::PathView::init(&"src/main.nia"),
    ).withImports(&imports[..])).?;
    let testModule = b.addModule(build::ModuleOptions::init(
        &"tests", fs::PathView::init(&"tests/main.nia"),
    ).withImports(&imports[..])).?;
    let app = b.addExecutable(build::ExecutableOptions::init(&"app", appModule)).?;
    let emit = b.addEmitExecutableStep(&"build", app).?;
    _ = b.addTestSuite(&"foo-tests", testModule).?;
    b.setDefaultStep(emit)
}
"#,
    )
    .expect("write build script");
    fs::write(
        workspace.join("packages/foo/main.nia"),
        "pub fun value() i32 { 7 }\n",
    )
    .expect("write foo package source");
    let source = r#"using foo;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    if foo::value() != 7 {
        return process::ExitCode(1)!;
    }
    !()
}
"#;
    fs::write(workspace.join("src/main.nia"), source).expect("write executable source");
    fs::write(workspace.join("tests/main.nia"), source).expect("write test source");

    let build = support::nia_command()
        .args(["build", "--root"])
        .arg(&workspace)
        .output_timeout_in_session("build executable with shared package imports");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let test = support::nia_command()
        .args(["build", "foo-tests", "--root"])
        .arg(&workspace)
        .output_timeout_in_session("test executable with shared package imports");
    assert!(
        test.status.success(),
        "{}",
        String::from_utf8_lossy(&test.stderr)
    );
}

// Test-ness belongs to the artifact: `nia build <suite>` compiles the suite
// with `@[test]` source and development dependencies, while an ordinary
// executable in the same plan sees neither.
#[test]
fn test_suites_are_test_compilations_under_nia_build() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let workspace = nia_test_support::test_dir("test-suite-artifact-mode");
    fs::create_dir_all(workspace.join("src")).expect("create source directory");
    fs::create_dir_all(workspace.join("checks")).expect("create dev dependency directory");
    fs::write(
        workspace.join("nia.toml"),
        "[package]\nversion = \"1.0.0\"\nname = \"root\"\n[dev-dependencies]\nchecks = { path = \"checks\" }\n",
    )
    .expect("write manifest");
    fs::write(
        workspace.join("checks/nia.toml"),
        "[package]\nversion = \"1.0.0\"\nname = \"checks\"\n[library]\npath = \"root.nia\"\n",
    )
    .expect("write dev dependency manifest");
    fs::write(
        workspace.join("checks/root.nia"),
        "pub fun expected() i32 { 7 }\n",
    )
    .expect("write dev dependency source");
    fs::write(
        workspace.join("build.nia"),
        r#"using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {
    let appModule = b.addModule(build::ModuleOptions::init(
        &"app", fs::PathView::init(&"src/main.nia"),
    )).?;
    let app = b.addExecutable(build::ExecutableOptions::init(&"app", appModule)).?;
    let emit = b.addEmitExecutableStep(&"app", app).?;
    _ = b.addTestSuite(&"unit", appModule).?;
    b.setDefaultStep(emit)
}
"#,
    )
    .expect("write build script");
    fs::write(
        workspace.join("src/main.nia"),
        r#"using std::process;

@[test]
using checks;

@[test]
fun check() process::ExitCode!() {
    if checks::expected() != 7 {
        return process::ExitCode(1)!;
    }
    !()
}

// Without the test code, the ordinary executable reports 3.
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    @[test]
    return check();
    process::ExitCode(3)!
}
"#,
    )
    .expect("write source");

    let build = |step: &str| {
        support::nia_command()
            .args(["build", step, "--root"])
            .arg(&workspace)
            .output_timeout_in_session("build one step of a package with a test suite")
    };
    let app = build("app");
    assert!(
        app.status.success(),
        "{}",
        String::from_utf8_lossy(&app.stderr)
    );
    let app = Command::new(workspace.join("build/out/app"))
        .output_timeout_in_session("run the ordinary executable");
    assert_eq!(app.status.code(), Some(3));
    let unit = build("unit");
    assert!(
        unit.status.success(),
        "{}",
        String::from_utf8_lossy(&unit.stderr)
    );
}

// A package with only nia.toml: each artifact is a step, `all` is the default,
// and `test` runs the suites. build.nia may still extend the manifest graph.
#[test]
fn manifest_artifacts_build_without_a_build_script() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let workspace = nia_test_support::test_dir("manifest-artifacts");
    fs::create_dir_all(workspace.join("src")).expect("create source directory");
    fs::create_dir_all(workspace.join("tests")).expect("create test directory");
    fs::write(
        workspace.join("nia.toml"),
        "[package]\nversion = \"1.0.0\"\nname = \"root\"\n\
         [[executable]]\nname = \"app\"\npath = \"src/main.nia\"\n\
         [[test]]\nname = \"unit\"\npath = \"tests/unit.nia\"\n",
    )
    .expect("write manifest");
    let program = |status: u8| {
        format!(
            "using std::process;\n\npub fun main(init: process::Init) process::ExitCode!() {{\n    _ = init;\n    process::ExitCode({status})!\n}}\n"
        )
    };
    fs::write(workspace.join("src/main.nia"), program(4)).expect("write app");
    fs::write(workspace.join("tests/unit.nia"), program(0)).expect("write suite");

    let listed = build_command(&workspace)
        .arg("--list")
        .output_timeout_in_session("list manifest-derived steps");
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout),
        "all           aggregate (default)\n\
         app           compiler-emit\n\
         run-app       external-command\n\
         test          aggregate\n\
         unit          test-executable\n\
         unit.compile  compiler-emit\n"
    );

    let all = build_command(&workspace).output_timeout_in_session("build every manifest artifact");
    assert!(
        all.status.success(),
        "{}",
        String::from_utf8_lossy(&all.stderr)
    );
    let app = Command::new(workspace.join("build/out/app"))
        .output_timeout_in_session("run the manifest executable");
    assert_eq!(app.status.code(), Some(4));

    let run = build_command(&workspace)
        .arg("run-app")
        .output_timeout_in_session("run the manifest executable through its step");
    assert_eq!(
        run.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );

    fs::write(
        workspace.join("build.nia"),
        r#"using std::build;

pub fun build(b: &mut build::Build) build::Error!() {
    let app = b.executable(&"app").?;
    let check = b.addCheckExecutableStep(&"check", app).?;
    b.dependOn(b.step(&"test").?, check).?;
    !()
}
"#,
    )
    .expect("write extending build script");
    let tested = build_command(&workspace)
        .args(["test", "--print-plan"])
        .output_timeout_in_session("run suites after the script's check");
    assert!(
        tested.status.success(),
        "{}",
        String::from_utf8_lossy(&tested.stderr)
    );
}

fn build_command(workspace: &std::path::Path) -> Command {
    let command_directory = workspace
        .parent()
        .expect("temporary workspace has a parent");
    let relative_root = workspace
        .strip_prefix(command_directory)
        .expect("test package is below command directory");
    let mut command = support::nia_command();
    command
        .current_dir(command_directory)
        .arg("build")
        .arg("--root")
        .arg(relative_root);
    command
}

#[test]
fn successful_suites_are_reused_independently_and_failures_are_never_cached() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let workspace = nia_test_support::test_dir("incremental-test-suites");
    fs::create_dir_all(workspace.join("tests")).unwrap();
    fs::write(
        workspace.join("build.nia"),
        r#"using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {
    let alpha = b.addModule(build::ModuleOptions::init(
        &"alpha-module", fs::PathView::init(&"tests/alpha.nia"),
    )).?;
    let beta = b.addModule(build::ModuleOptions::init(
        &"beta-module", fs::PathView::init(&"tests/beta.nia"),
    )).?;
    _ = b.addTestSuite(&"alpha", alpha).?;
    _ = b.addTestSuite(&"beta", beta).?;
    !()
}
"#,
    )
    .unwrap();
    let suite = |marker: &str, status: i32| {
        format!(
            r#"using std::process;
using std::io;

pub fun main(init: process::Init) process::ExitCode!() {{
    _ = init;
    let mut buffer: [u8; 0] = [];
    let mut stdout = io::FileWriter::stdout(&mut buffer[..]);
    match stdout.writeAll(&b"{marker}\n") {{
        !ok => {{ _ = ok; }},
        error! => {{ _ = error; return process::ExitCode(9)!; }},
    }}
    process::ExitCode({status})!
}}
"#,
        )
    };
    fs::write(workspace.join("tests/alpha.nia"), suite("alpha-ran", 0)).unwrap();
    fs::write(workspace.join("tests/beta.nia"), suite("beta-ran", 0)).unwrap();
    let run = || {
        build_command(&workspace)
            .arg("test")
            .output_timeout_in_session("run incremental suites")
    };
    let cold = run();
    assert!(
        cold.status.success(),
        "{}",
        String::from_utf8_lossy(&cold.stderr)
    );
    let stdout = String::from_utf8_lossy(&cold.stdout);
    assert!(stdout.contains("alpha-ran"), "{stdout}");
    assert!(stdout.contains("beta-ran"), "{stdout}");
    let warm = run();
    assert!(
        warm.status.success(),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    assert!(
        warm.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&warm.stdout)
    );

    fs::write(workspace.join("tests/alpha.nia"), suite("alpha-edited", 0)).unwrap();
    let edited = run();
    assert!(
        edited.status.success(),
        "{}",
        String::from_utf8_lossy(&edited.stderr)
    );
    let stdout = String::from_utf8_lossy(&edited.stdout);
    assert!(stdout.contains("alpha-edited"), "{stdout}");
    assert!(!stdout.contains("beta-ran"), "{stdout}");

    fs::write(workspace.join("tests/alpha.nia"), suite("alpha-failed", 7)).unwrap();
    for _ in 0..2 {
        let failed = run();
        assert!(!failed.status.success());
        let stdout = String::from_utf8_lossy(&failed.stdout);
        assert!(stdout.contains("alpha-failed"), "{stdout}");
        assert!(!stdout.contains("beta-ran"), "{stdout}");
    }
}

#[test]
fn test_only_source_edits_do_not_invalidate_ordinary_build_artifacts() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let workspace = nia_test_support::test_dir("test-only-edit-ordinary-build-cache");
    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::create_dir_all(workspace.join("tests/src")).unwrap();
    fs::write(
        workspace.join("nia.toml"),
        "[package]\nversion = \"1.0.0\"\nname = \"root\"\n[dev-dependencies]\ntests = { path = \"tests\" }\n",
    )
    .unwrap();
    fs::write(
        workspace.join("tests/nia.toml"),
        "[package]\nversion = \"1.0.0\"\nname = \"tests\"\n",
    )
    .unwrap();
    fs::write(
        workspace.join("build.nia"),
        r#"using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {
    let appModule = b.addModule(build::ModuleOptions::init(
        &"app", fs::PathView::init(&"src/main.nia"),
    )).?;
    let appExe = b.addExecutable(build::ExecutableOptions::init(&"app", appModule)).?;
    let buildStep = b.addEmitExecutableStep(&"build", appExe).?;
    b.setDefaultStep(buildStep).?;
    !()
}
"#,
    )
    .unwrap();
    fs::write(
        workspace.join("src/main.nia"),
        r#"using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    process::ExitCode(0)!
}
"#,
    )
    .unwrap();
    fs::write(
        workspace.join("tests/src/suite.nia"),
        "not valid Nia source\n",
    )
    .unwrap();

    let run = || {
        let mut command = support::nia_command();
        command
            .args([
                "--diagnostics-format=json",
                "build",
                "--print-cache",
                "--root",
            ])
            .arg(&workspace);
        command.output_timeout_in_session("build without test-only sources")
    };
    let cold = run();
    assert!(
        cold.status.success(),
        "{}",
        String::from_utf8_lossy(&cold.stderr)
    );
    let warm = run();
    assert!(
        warm.status.success(),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    let cache = String::from_utf8_lossy(&warm.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|value| value["kind"] == "action-cache")
        .expect("ordinary build cache report");
    let entries = cache["entries"].as_array().expect("cache entries");
    assert!(!entries.is_empty(), "{cache}");
    assert!(
        entries
            .iter()
            .all(|entry| entry["decision"]["status"] == "hit"),
        "warm ordinary build did not reuse every cached artifact: {cache}"
    );

    fs::write(
        workspace.join("tests/src/suite.nia"),
        "still invalid Nia, but changed test-only input\n",
    )
    .unwrap();
    let after_test_edit = run();
    assert!(
        after_test_edit.status.success(),
        "{}",
        String::from_utf8_lossy(&after_test_edit.stderr)
    );
    let cache = String::from_utf8_lossy(&after_test_edit.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|value| value["kind"] == "action-cache")
        .expect("ordinary build cache report after test-only edit");
    let entries = cache["entries"].as_array().expect("cache entries");
    assert!(
        entries
            .iter()
            .all(|entry| entry["decision"]["status"] == "hit"),
        "test-only source edit invalidated ordinary artifact actions: {cache}"
    );
}
