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
