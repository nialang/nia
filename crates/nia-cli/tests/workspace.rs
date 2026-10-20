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

pub fn build(b: &mut build::Build) build::Error!() {
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
            "[package]\nname = \"".to_owned()
                + member_name
                + "\"\n[dependencies]\nshared = { path = \"../../dependencies/"
                + dependency_name
                + "\" }\n",
        )
        .unwrap();
        fs::write(
            dependency.join("nia.toml"),
            "[package]\nname = \"".to_owned()
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
            "using shared;\nusing std::process;\n\npub const value: u8 = shared::value;\n\npub fn main(init: process::Init) process::ExitCode!() {\n    _ = init;\n    !()\n}\n",
        )
        .unwrap();
        fs::write(
            member.join("build.nia"),
            r#"using std::build;
using std::fs;

pub fn build(b: &mut build::Build) build::Error!() {
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
