// SPDX-License-Identifier: GPL-3.0-or-later
use nia_test_support::CommandExt;

mod support;

use nia_test_support::test_dir as temp_dir;

#[test]
fn new_generates_a_checkable_executable_package() {
    let parent = temp_dir("new_generates_executable_package");
    let root = parent.join("hello");
    let output = support::nia_command()
        .args(["new", "hello"])
        .current_dir(&*parent)
        .output_timeout_for_compiler("create an executable package");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.join("nia.toml").is_file());
    assert!(root.join("lib/root.nia").is_file());
    assert!(root.join("src/main.nia").is_file());
    assert!(root.join("tests/unit.nia").is_file());
    assert_eq!(
        std::fs::read_to_string(root.join(".gitignore")).unwrap(),
        "/build/\n"
    );

    let check = support::nia_command()
        .args(["build", "test", "--root"])
        .arg(&root)
        .output_timeout_for_build("build and run the generated test template");
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn init_dot_and_workspace_new_commands_create_native_layouts() {
    let parent = temp_dir("init_dot_and_workspace_commands");
    let initialized = parent.join("initialized");
    std::fs::create_dir(&initialized).unwrap();
    let init = support::nia_command()
        .args(["init"])
        .current_dir(&initialized)
        .output_timeout_for_compiler("initialize the current directory");
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    assert!(initialized.join("lib/root.nia").is_file());
    assert!(initialized.join("src/main.nia").is_file());
    assert!(initialized.join("tests/unit.nia").is_file());
    assert_eq!(
        std::fs::read_to_string(initialized.join(".gitignore")).unwrap(),
        "/build/\n"
    );

    let workspace = parent.join("mono");
    let create = support::nia_command()
        .args(["new", "mono", "--workspace", "--member", "app"])
        .current_dir(&*parent)
        .output_timeout_for_compiler("create a workspace");
    assert!(
        create.status.success(),
        "{}",
        String::from_utf8_lossy(&create.stderr)
    );
    let add = support::nia_command()
        .args(["new", "../core", "--lib"])
        .current_dir(workspace.join("packages/app"))
        .output_timeout_for_compiler("create a workspace member through nia new");
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    assert!(workspace.join("packages/core/nia.toml").is_file());
    assert!(workspace.join("packages/core/lib/root.nia").is_file());
    assert!(workspace.join("packages/core/tests/unit.nia").is_file());
    assert!(!workspace.join("packages/core/src/main.nia").exists());
    assert_eq!(
        std::fs::read_to_string(workspace.join(".gitignore")).unwrap(),
        "/build/\n"
    );
    assert!(!workspace.join("packages/app/.gitignore").exists());
    let manifest = std::fs::read_to_string(workspace.join("nia.toml")).unwrap();
    assert!(manifest.contains("members = [\"packages/*\"]"));

    let initialized_member = workspace.join("packages/tools");
    std::fs::create_dir(&initialized_member).unwrap();
    let init = support::nia_command()
        .args(["init", "../tools", "--exe"])
        .current_dir(workspace.join("packages/app"))
        .output_timeout_for_compiler("initialize a workspace member through nia init");
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    assert!(initialized_member.join("src/main.nia").is_file());
    assert!(initialized_member.join("tests/unit.nia").is_file());
    assert!(!initialized_member.join("lib/root.nia").exists());
}

#[test]
fn add_path_dependency_edits_the_selected_package_manifest() {
    let parent = temp_dir("add_path_dependency");
    for (name, args) in [
        ("app", vec!["new", "app", "--exe"]),
        ("util", vec!["new", "util", "--lib"]),
    ] {
        let output = support::nia_command()
            .args(args)
            .current_dir(&*parent)
            .output_timeout_for_compiler("create a package for dependency editing");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(parent.join(name).join("nia.toml").is_file());
    }

    let app = parent.join("app");
    let output = support::nia_command()
        .args(["add", "util", "--path", "../util", "--scope", "runtime"])
        .current_dir(&app)
        .output_timeout_for_compiler("add a local path dependency");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest = std::fs::read_to_string(app.join("nia.toml")).unwrap();
    assert!(manifest.contains("util = { path = \"../util\" }"));
}
