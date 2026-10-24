// SPDX-License-Identifier: GPL-3.0-or-later
use std::fs;

use nia_test_support::{CommandExt, TestWorkload};

mod support;

#[test]
fn workspace_build_selects_defaults_named_members_and_all_members() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let root = nia_test_support::test_dir("workspace-build-selection");
    fs::write(
        root.join("nia.toml"),
        "[workspace]\nmembers = [\"packages/*\"]\ndefault-members = [\"packages/app\"]\n[workspace.package]\nversion = \"1.2.3\"\n",
    )
    .unwrap();
    for name in ["app", "core"] {
        let member = root.join("packages").join(name);
        fs::create_dir_all(member.join("src")).unwrap();
        fs::write(
            member.join("nia.toml"),
            format!("[package]\nname = \"{name}\"\nversion.workspace = true\n"),
        )
        .unwrap();
        fs::write(
            member.join("build.nia"),
            r#"using std::build;

pub fun build(b: &mut build::Build) build::Error!() {
    let generated = b.addGeneratedFileStep(
        &"generated",
        build::BuildPathView::init(&"selected.txt"),
        &b"selected\n"[..],
    ).?;
    b.setDefaultStep(generated).?;
    !()
}
"#,
        )
        .unwrap();
    }
    let app_output = root.join("packages/app/.nia-build/selected.txt");
    let core_output = root.join("packages/core/.nia-build/selected.txt");
    let run = |directory: &std::path::Path, options: &[&str]| {
        let output = support::nia_command()
            .current_dir(directory)
            .arg("build")
            .args(options)
            .output_timeout_in_session("build selected workspace members");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };

    run(&root, &[]);
    assert!(app_output.is_file());
    assert!(!core_output.exists());
    fs::remove_file(&app_output).unwrap();

    run(&root, &["-p", "core"]);
    assert!(!app_output.exists());
    assert!(core_output.is_file());
    fs::remove_file(&core_output).unwrap();

    run(&root.join("packages/core/src"), &[]);
    assert!(!app_output.exists());
    assert!(core_output.is_file());
    fs::remove_file(&core_output).unwrap();

    run(&root, &["--workspace"]);
    assert!(app_output.is_file());
    assert!(core_output.is_file());
}

#[test]
fn workspace_members_can_use_the_same_dependency_alias_for_different_packages() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let root = nia_test_support::test_dir("workspace-local-dependency-aliases");
    fs::write(
        root.join("nia.toml"),
        "[workspace]\nmembers = [\"members/*\"]\ndefault-members = [\"members/first\", \"members/second\"]\nexclude = [\"dependencies\"]\n",
    )
    .unwrap();

    for (member_name, dependency_name, value) in [
        ("first", "first-dependency", 11_u8),
        ("second", "second-dependency", 22_u8),
    ] {
        let member = root.join("members").join(member_name);
        let dependency = root.join("dependencies").join(dependency_name);
        fs::create_dir_all(member.join("src")).unwrap();
        fs::create_dir_all(dependency.join("src")).unwrap();
        fs::write(
            member.join("nia.toml"),
            "[package]\nversion = \"1.0.0\"\nname = \"".to_owned()
                + member_name
                + "\"\n[dependencies]\nshared = { path = \"../../dependencies/"
                + dependency_name
                + "\" }\n",
        )
        .unwrap();
        fs::write(
            dependency.join("nia.toml"),
            "[package]\nversion = \"1.0.0\"\nname = \"".to_owned()
                + dependency_name
                + "\"\n[library]\npath = \"src/lib.nia\"\n",
        )
        .unwrap();
        fs::write(
            dependency.join("src/lib.nia"),
            format!("pub const value: u8 = {value};\n"),
        )
        .unwrap();
        fs::write(
            member.join("src/main.nia"),
            "using shared;\nusing std::process;\n\npub const value: u8 = shared::value;\n\npub fun main(init: process::Init) process::ExitCode!() {\n    _ = init;\n    !()\n}\n",
        )
        .unwrap();
        fs::write(
            member.join("build.nia"),
            r#"using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {
    let moduleHandle = b.addModule(build::ModuleOptions::init(&"app", fs::PathView::init(&"src/main.nia"))).?;
    let executableHandle = b.addExecutable(build::ExecutableOptions::init(&"app", moduleHandle)).?;
    let check = b.addCheckExecutableStep(&"check", executableHandle).?;
    b.setDefaultStep(check).?;
    !()
}
"#,
        )
        .unwrap();
    }

    let output = support::nia_command()
        .current_dir(&root)
        .arg("build")
        .arg("--workspace")
        .output_timeout_in_session("compile workspace members with local aliases");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn workspace_test_selection_continues_by_default_and_honors_fail_fast() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let root = nia_test_support::test_dir("workspace-test-selection");
    fs::write(
        root.join("nia.toml"),
        "[workspace]\nmembers = [\"members/*\"]\ndefault-members = [\"members/alpha\", \"members/beta\"]\n",
    )
    .unwrap();
    for (name, status) in [("alpha", 7), ("beta", 0)] {
        let member = root.join("members").join(name);
        fs::create_dir_all(member.join("tests")).unwrap();
        fs::write(
            member.join("nia.toml"),
            format!("[package]\nversion = \"1.0.0\"\nname = \"{name}\"\n"),
        )
        .unwrap();
        fs::write(
            member.join("build.nia"),
            r#"using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {
    let testModule = b.addModule(build::ModuleOptions::init(&"tests", fs::PathView::init(&"tests/main.nia"))).?;
    _ = b.addTestSuite(&"suite", testModule).?;
    !()
}
"#,
        )
        .unwrap();
        fs::write(
            member.join("tests/main.nia"),
            format!(
                "using std::io;\nusing std::process;\n\npub fun main(init: process::Init) process::ExitCode!() {{\n    _ = init;\n    let mut buffer: [u8; 0] = [];\n    let mut stdout = io::FileWriter::stdout(&mut buffer[..]);\n    stdout.writeAll(&b\"{name}-ran\\n\").?;\n    process::ExitCode({status})!\n}}\n"
            ),
        )
        .unwrap();
    }

    let run = |options: &[&str]| {
        support::nia_command()
            .current_dir(&root)
            .arg("test")
            .args(options)
            .output_timeout_in_session("run selected workspace test suites")
    };
    let selected = run(&["-p", "beta"]);
    assert!(
        selected.status.success(),
        "{}",
        String::from_utf8_lossy(&selected.stderr)
    );
    let selected_stdout = String::from_utf8_lossy(&selected.stdout);
    assert!(selected_stdout.contains("beta-ran"), "{selected_stdout}");
    assert!(!selected_stdout.contains("alpha-ran"), "{selected_stdout}");

    let beta_suite = root.join("members/beta/tests/main.nia");
    let beta_source = fs::read_to_string(&beta_suite).unwrap();
    fs::write(
        &beta_suite,
        beta_source.replace("beta-ran", "beta-workspace-ran"),
    )
    .unwrap();
    let all = run(&["--workspace"]);
    assert_eq!(all.status.code(), Some(1));
    let all_stdout = String::from_utf8_lossy(&all.stdout);
    assert!(all_stdout.contains("alpha-ran"), "{all_stdout}");
    assert!(all_stdout.contains("beta-workspace-ran"), "{all_stdout}");

    let beta_source = fs::read_to_string(&beta_suite).unwrap();
    fs::write(
        &beta_suite,
        beta_source.replace("beta-workspace-ran", "beta-failfast-ran"),
    )
    .unwrap();
    let fail_fast = run(&["--workspace", "--fail-fast"]);
    assert_eq!(fail_fast.status.code(), Some(1));
    let fail_fast_stdout = String::from_utf8_lossy(&fail_fast.stdout);
    assert!(fail_fast_stdout.contains("alpha-ran"), "{fail_fast_stdout}");
    assert!(!fail_fast_stdout.contains("beta-ran"), "{fail_fast_stdout}");
}

#[test]
fn executables_reach_their_own_library_as_a_separate_tree() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let root = nia_test_support::test_dir("own-library-tree");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("nia.toml"),
        "[package]\nname = \"hello\"\nversion = \"0.1.0\"\n[library]\npath = \"src/lib.nia\"\n",
    )
    .unwrap();
    fs::write(
        root.join("src/lib.nia"),
        "pub fun answer() i32 { 42 }\npub(root) fun hidden() i32 { 7 }\n",
    )
    .unwrap();
    fs::write(
        root.join("build.nia"),
        r#"using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {
    let app = b.addModule(build::ModuleOptions::init(&"app", fs::PathView::init(&"src/main.nia"))).?;
    let exe = b.addExecutable(build::ExecutableOptions::init(&"app", app)).?;
    let emit = b.addEmitExecutableStep(&"app", exe).?;
    _ = b.addRunExecutableStep(&"run", build::RunOptions::init(exe)).?;
    b.setDefaultStep(emit).?;
    !()
}
"#,
    )
    .unwrap();
    let main = |call: &str| {
        format!(
            "using std::process;\nusing hello;\n\npub fun main(init: process::Init) process::ExitCode!() {{\n    _ = init;\n    if {call} != 42 {{\n        return process::ExitCode(1)!;\n    }}\n    !()\n}}\n"
        )
    };
    let build = |step: &[&str]| {
        support::nia_command()
            .current_dir(&root)
            .arg("build")
            .args(step)
            .arg("-q")
            .output_timeout_in_session("build own-library package")
    };

    // The executable opens its package's library by the package name.
    fs::write(root.join("src/main.nia"), main("hello::answer()")).unwrap();
    let run = build(&["run"]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );

    // The library is its own module tree, so `pub(root)` stays inside it.
    fs::write(root.join("src/main.nia"), main("hello::hidden()")).unwrap();
    let hidden = build(&[]);
    assert!(!hidden.status.success());
    let stderr = String::from_utf8_lossy(&hidden.stderr);
    assert!(
        stderr.contains("`hello::hidden` is restricted to its module tree"),
        "{stderr}"
    );
}
