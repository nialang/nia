// SPDX-License-Identifier: GPL-3.0-or-later
mod support;

use nia_test_support::{CommandExt, test_dir as temp_dir};

fn probe_tool_fixture(
    root: &std::path::Path,
) -> (
    &'static str,
    &'static str,
    Option<&'static str>,
    usize,
    &'static str,
) {
    let (tool_name, command_path, contents, argument_count, command_arguments) = if cfg!(windows) {
        (
            "nia-probe-tool.exe",
            "tools/nia-probe-tool.exe",
            None,
            4,
            "[&\"/d\"[..], &\"/c\"[..], &\"echo\"[..], &\"probe-one\"[..]]",
        )
    } else {
        (
            "nia-probe-tool",
            "tools/nia-probe-tool",
            Some(
                "#!/bin/sh\nif [ \"$1\" = \"--probe\" ]; then printf 'probe-one\\n'; else printf 'version-one\\n'; fi\n",
            ),
            1,
            "[&\"--probe\"[..]]",
        )
    };
    std::fs::create_dir_all(root.join("tools")).expect("create probe tool directory");
    if cfg!(windows) {
        let system_root = std::env::var_os("SystemRoot").expect("Windows system root");
        std::fs::copy(
            std::path::PathBuf::from(system_root).join("System32/cmd.exe"),
            root.join(command_path),
        )
        .expect("copy probe tool");
    } else {
        std::fs::write(root.join(command_path), contents.expect("Unix probe tool"))
            .expect("write probe tool");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join(command_path);
        let mut permissions = std::fs::metadata(&path)
            .expect("probe tool metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("make probe tool executable");
    }
    (
        tool_name,
        command_path,
        contents,
        argument_count,
        command_arguments,
    )
}

fn probe_build_command(root: &std::path::Path) -> std::process::Command {
    let tool_dir = root.join("tools");
    let path = std::env::join_paths(std::iter::once(tool_dir).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .expect("probe PATH");
    let mut command = support::nia_command();
    command
        .env("PATH", path)
        .args(["build", "--root"])
        .arg(root)
        .arg("--print-cache");
    command
}

#[test]
fn structured_build_inspection_reports_nodes_tools_observations_and_dry_run() {
    let root = temp_dir("structured_build_inspection");
    std::fs::create_dir_all(root.join("src")).expect("create source directory");
    std::fs::write(root.join("src/main.nia"), "pub const value: u8 = 1;\n").expect("write source");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;
using std::fs;

pub fn build(b: &mut build::Build) build::Error!() {
    let mut buffer: [u8; 64] = [0; 64];
    _ = b.probeFile(fs::PathView::init(&"src/main.nia"), &mut buffer[..]).?;
    let rootModule = b.addModule(build::ModuleOptions::init(
        &"main", fs::PathView::init(&"src/main.nia"),
    )).?;
    let executable = b.addExecutable(build::ExecutableOptions::init(&"app", rootModule)).?;
    let emit = b.addEmitExecutableStep(&"emit", executable).?;
    b.setDefaultStep(emit)
}
"#,
    )
    .expect("write build script");

    let output = support::nia_command()
        .args([
            "build",
            "--root",
            root.to_str().unwrap(),
            "--dry-run",
            "--explain",
            "--print-plan",
            "--print-cache",
            "--print-observations",
            "--diagnostics-format=json",
        ])
        .output_timeout_for_build("render structured build inspection");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).expect("JSON inspection record")
        })
        .collect::<Vec<_>>();
    let plan = records
        .iter()
        .find(|record| record["kind"] == "build-plan")
        .unwrap();
    assert!(
        plan["modules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|module| module["name"] == "main")
    );
    assert!(
        plan["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|artifact| artifact["name"] == "app")
    );
    assert!(
        plan["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|action| action["kind"] == "compiler-emit")
    );
    assert!(
        plan["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step["name"] == "emit")
    );

    let explanation = records
        .iter()
        .find(|record| record["kind"] == "build-explanation")
        .unwrap();
    assert!(explanation["observations"]["valid"].as_bool().unwrap());
    assert!(
        !explanation["observations"]["observations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(explanation["tools"].as_array().is_some());

    let cache = records
        .iter()
        .find(|record| record["kind"] == "action-cache" && record["mode"] == "dry_run")
        .unwrap();
    assert_eq!(cache["entries"][0]["outcome"]["status"], "not_consulted");
    assert_eq!(
        cache["entries"][0]["outcome"]["publication"],
        "not_attempted"
    );
    assert!(!root.join(".nia-build/app").exists());
}

#[test]
fn command_and_tool_probes_reuse_and_invalidate_on_tool_change() {
    let root = temp_dir("build_command_and_tool_probes");
    let (_tool_name, command_path, _contents, argument_count, command_arguments) =
        probe_tool_fixture(&root);
    std::fs::create_dir_all(root.join("src")).expect("create source directory");
    std::fs::write(
        root.join("nia.package"),
        "name=probe\nversion=0.1.0\nsource-root=src\nexclude=tools\n",
    )
    .expect("write package manifest");
    std::fs::write(root.join("src/main.nia"), "pub const value: u8 = 1;\n")
        .expect("write package source");
    std::fs::write(
        root.join("build.nia"),
        format!(
            r#"
using std::build;
using std::fs;

pub fn build(b: &mut build::Build) build::Error!() {{
    let mut buffer: [u8; 4096] = [0; 4096];
    let arguments: [&[char]; {argument_count}] = {command_arguments};
    _ = b.probeCommand(fs::PathView::init(&"{command_path}"), &arguments[..], &mut buffer[..]).?;
    _ = b.probeTool(fs::PathView::init(&"{command_path}"), &mut buffer[..]).?;
    let step = b.addAggregateStep(&"default").?;
    b.setDefaultStep(step)
}}
"#
        ),
    )
    .expect("write command and tool probe build script");

    let first = probe_build_command(&root)
        .arg("--print-observations")
        .output_timeout_for_build("run command and tool probes");
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let observations = String::from_utf8_lossy(&first.stdout);
    assert!(observations.contains("command-result"), "{observations}");
    assert!(observations.contains("tool-identity"), "{observations}");

    let warm = probe_build_command(&root).output_timeout_for_build("reuse command probes");
    assert!(
        warm.status.success(),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    assert!(
        String::from_utf8_lossy(&warm.stdout).contains("runner reused"),
        "{}",
        String::from_utf8_lossy(&warm.stdout)
    );

    let changed_contents = if cfg!(windows) {
        None
    } else {
        Some(
            "#!/bin/sh\nif [ \"$1\" = \"--probe\" ]; then printf 'probe-two\\n'; else printf 'version-two\\n'; fi\n",
        )
    };
    if let Some(contents) = changed_contents {
        std::fs::write(root.join(command_path), contents).expect("change probe tool");
    } else {
        let system_root = std::env::var_os("SystemRoot").expect("Windows system root");
        std::fs::copy(
            std::path::PathBuf::from(system_root).join("System32/where.exe"),
            root.join(command_path),
        )
        .expect("replace Windows probe tool");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join(command_path);
        let mut permissions = std::fs::metadata(&path)
            .expect("changed probe metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("retain probe tool executable bit");
    }
    let changed = probe_build_command(&root)
        .output_timeout_for_build("invalidate changed command and tool probes");
    assert!(
        changed.status.success(),
        "{}",
        String::from_utf8_lossy(&changed.stderr)
    );
    assert!(
        String::from_utf8_lossy(&changed.stdout).contains("observation changed"),
        "{}",
        String::from_utf8_lossy(&changed.stdout)
    );
}

#[test]
fn git_state_probe_reuses_and_invalidates_when_repository_state_changes() {
    let root = temp_dir("build_git_state_probe");
    std::fs::create_dir_all(root.join("src")).expect("create source directory");
    std::fs::write(
        root.join("nia.package"),
        "name=probe\nversion=0.1.0\nsource-root=src\n",
    )
    .expect("write package manifest");
    std::fs::write(root.join(".gitignore"), ".nia-build\n.nia-cache\n").expect("write gitignore");
    std::fs::write(root.join("src/main.nia"), "pub const value: u8 = 1;\n")
        .expect("write package source");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;

pub fn build(b: &mut build::Build) build::Error!() {
    let mut buffer: [u8; 4096] = [0; 4096];
    _ = b.probeGitState(&mut buffer[..]).?;
    let step = b.addAggregateStep(&"default").?;
    b.setDefaultStep(step)
}
"#,
    )
    .expect("write Git probe build script");

    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .current_dir(&root)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .expect("run git");
        assert!(status.success(), "git command failed: {args:?}");
    };
    git(&["init"]);
    git(&["config", "user.email", "nia-tests@example.invalid"]);
    git(&["config", "user.name", "Nia Tests"]);
    git(&["add", "."]);
    git(&["commit", "-m", "baseline"]);

    let first = probe_build_command(&root)
        .arg("--print-observations")
        .output_timeout_for_build("run Git state probe");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        String::from_utf8_lossy(&first.stdout).contains("git-state"),
        "{}",
        String::from_utf8_lossy(&first.stdout)
    );
    let warm = probe_build_command(&root).output_timeout_for_build("reuse Git state probe");
    assert!(
        warm.status.success(),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    assert!(
        String::from_utf8_lossy(&warm.stdout).contains("runner reused"),
        "{}",
        String::from_utf8_lossy(&warm.stdout)
    );

    std::fs::write(root.join("git-state-marker"), "changed\n").expect("change Git state");
    let changed = probe_build_command(&root).output_timeout_for_build("invalidate Git probe");
    assert!(
        changed.status.success(),
        "{}",
        String::from_utf8_lossy(&changed.stderr)
    );
    assert!(
        String::from_utf8_lossy(&changed.stdout).contains("observation changed"),
        "{}",
        String::from_utf8_lossy(&changed.stdout)
    );
}

#[test]
fn build_environment_probes_distinguish_absent_empty_and_changed_values() {
    let root = temp_dir("build_environment_probes_distinguish_states");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;

pub fn build(b: &mut build::Build) build::Error!() {
    _ = b.probeEnv(&"NIA_BUILD_PROBE_ABSENT").?;
    _ = b.probeEnv(&"NIA_BUILD_PROBE_EMPTY").?;
    _ = b.probeEnv(&"NIA_BUILD_PROBE_VALUE").?;
    let step = b.addAggregateStep(&"default").?;
    b.setDefaultStep(step)
}

"#,
    )
    .expect("write build script");

    let first = support::nia_command()
        .env_remove("NIA_BUILD_PROBE_ABSENT")
        .env("NIA_BUILD_PROBE_EMPTY", "")
        .env("NIA_BUILD_PROBE_VALUE", "first")
        .arg("build")
        .arg("--root")
        .arg(&root)
        .arg("--print-cache")
        .arg("--print-observations")
        .output_timeout_for_build("run environment probe build");
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let observations = String::from_utf8_lossy(&first.stdout);
    assert!(
        observations.contains("NIA_BUILD_PROBE_ABSENT"),
        "{observations}"
    );
    assert!(
        observations.contains("NIA_BUILD_PROBE_EMPTY"),
        "{observations}"
    );
    assert!(
        observations.contains("NIA_BUILD_PROBE_VALUE"),
        "{observations}"
    );

    let warm = support::nia_command()
        .env_remove("NIA_BUILD_PROBE_ABSENT")
        .env("NIA_BUILD_PROBE_EMPTY", "")
        .env("NIA_BUILD_PROBE_VALUE", "first")
        .args(["build", "--root"])
        .arg(&root)
        .arg("--print-cache")
        .output_timeout_for_build("reuse unchanged environment observations");
    assert!(
        warm.status.success(),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    assert!(
        String::from_utf8_lossy(&warm.stdout).contains("runner reused"),
        "{}",
        String::from_utf8_lossy(&warm.stdout)
    );
    let second = support::nia_command()
        .env_remove("NIA_BUILD_PROBE_ABSENT")
        .env("NIA_BUILD_PROBE_EMPTY", "")
        .env("NIA_BUILD_PROBE_VALUE", "second")
        .arg("build")
        .arg("--root")
        .arg(&root)
        .arg("--print-cache")
        .output_timeout_for_build("rerun environment probe build");
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let cache = String::from_utf8_lossy(&second.stdout);
    assert!(cache.contains("observation changed"), "{cache}");
    let present = support::nia_command()
        .env("NIA_BUILD_PROBE_ABSENT", "")
        .env("NIA_BUILD_PROBE_EMPTY", "")
        .env("NIA_BUILD_PROBE_VALUE", "second")
        .args(["build", "--root"])
        .arg(&root)
        .arg("--print-cache")
        .output_timeout_for_build("observe absent variable becoming explicitly empty");
    assert!(
        present.status.success(),
        "{}",
        String::from_utf8_lossy(&present.stderr)
    );
    assert!(
        String::from_utf8_lossy(&present.stdout).contains("observation changed"),
        "{}",
        String::from_utf8_lossy(&present.stdout)
    );
}

#[test]
fn build_directory_probe_records_entry_size_and_invalidates_on_resize() {
    let root = temp_dir("build_directory_probe_records_entry_size");
    std::fs::create_dir(root.join("inputs")).expect("create observed directory");
    std::fs::write(
        root.join("nia.package"),
        "name=probe\nversion=0.1.0\nexclude=inputs\n",
    )
    .expect("write package manifest");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;
using std::fs;

pub fn build(b: &mut build::Build) build::Error!() {
    let mut buffer: [u8; 4096] = [0; 4096];
    b.probeDirectory(fs::PathView::init(&"inputs"), &mut buffer[..]).?;
    let step = b.addAggregateStep(&"default").?;
    b.setDefaultStep(step)
}
"#,
    )
    .expect("write build script");

    let first = support::nia_command()
        .arg("build")
        .arg("--root")
        .arg(&root)
        .arg("--print-observations")
        .output_timeout_for_build("run directory probe build");
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        String::from_utf8_lossy(&first.stdout).contains("directory-contents"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let warm = support::nia_command()
        .args(["build", "--root"])
        .arg(&root)
        .arg("--print-cache")
        .output_timeout_for_build("reuse unchanged directory observation");
    assert!(
        warm.status.success(),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    assert!(
        String::from_utf8_lossy(&warm.stdout).contains("runner reused"),
        "{}",
        String::from_utf8_lossy(&warm.stdout)
    );
    std::fs::write(root.join("inputs/value.txt"), b"one").expect("write observed entry");
    let second = support::nia_command()
        .arg("build")
        .arg("--root")
        .arg(&root)
        .arg("--print-cache")
        .output_timeout_for_build("rerun directory probe after entry creation");
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(
        String::from_utf8_lossy(&second.stdout).contains("observation changed"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );

    std::fs::write(root.join("inputs/value.txt"), b"one-two").expect("resize observed entry");
    let third = support::nia_command()
        .arg("build")
        .arg("--root")
        .arg(&root)
        .arg("--print-cache")
        .output_timeout_for_build("rerun directory probe after entry resize");
    assert!(
        third.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&third.stderr)
    );
    assert!(
        String::from_utf8_lossy(&third.stdout).contains("observation changed"),
        "{}",
        String::from_utf8_lossy(&third.stdout)
    );
}

#[test]
fn build_file_presence_probe_invalidates_on_create_and_remove() {
    let root = temp_dir("build_file_presence_probe_invalidates");
    std::fs::write(
        root.join("nia.package"),
        "name=probe\nversion=0.1.0\nexclude=optional\n",
    )
    .expect("write package manifest");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;
using std::fs;

pub fn build(b: &mut build::Build) build::Error!() {
    let present = b.probeFilePresence(fs::PathView::init(&"optional/input")).?;
    let name: &[char] = if present { &"present" } else { &"absent" };
    let step = b.addAggregateStep(name).?;
    b.setDefaultStep(step)
}
"#,
    )
    .expect("write build script");

    let run = |label: &str| {
        let output = support::nia_command()
            .args(["build", "--root"])
            .arg(&root)
            .arg("--print-observations")
            .arg("--print-cache")
            .output_timeout_for_build(label);
        assert!(
            output.status.success(),
            "{label}: stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    };

    let presence_line = |stdout: &str| {
        stdout
            .lines()
            .find(|line| {
                line.trim_start()
                    .starts_with("file-presence optional/input ")
            })
            .map(str::to_owned)
            .unwrap_or_else(|| panic!("missing file-presence observation:\n{stdout}"))
    };

    let first = run("run file presence probe build");
    let absent = presence_line(&first);
    let warm = run("reuse unchanged file presence observation");
    assert!(warm.contains("runner reused"), "{warm}");

    std::fs::create_dir(root.join("optional")).expect("create observed parent");
    std::fs::write(root.join("optional/input"), b"present").expect("create observed file");
    let created = run("rerun file presence probe after creation");
    assert!(created.contains("observation changed"), "{created}");
    assert_ne!(presence_line(&created), absent);

    // Removal restores the absent fact, so the first configuration is valid again.
    std::fs::remove_file(root.join("optional/input")).expect("remove observed file");
    let removed = run("rerun file presence probe after removal");
    assert!(removed.contains("runner reused"), "{removed}");
    assert_eq!(presence_line(&removed), absent);
}

#[test]
fn directory_iterator_compiles_for_all_linux_targets() {
    let root = temp_dir("directory_iterator_linux_targets");
    let source = root.join("directory.nia");
    std::fs::write(
        &source,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut directory = fs::Dir::cwd().?;
    defer directory.close().?;
    let mut buffer: [u8; 4096] = [0; 4096];
    let mut entries = directory.entries(&mut buffer[..]).?;
    defer entries.close().?;
    let mut size: u64 = 0;
    loop {
        let entry = match entries.next() {
            ?result => result.?,
            null => break,
        };
        size = size | entry.size();
    }
    if size == u64::MAX { return process::ExitCode(1)!; }
    !()
}
"#,
    )
    .expect("write Linux directory iterator source");
    for target in nia_test_support::LINUX_TARGETS {
        let output = support::nia_command()
            .args(["--target", target, "check"])
            .arg(&source)
            .output_timeout_for_build("check Linux directory iterator");
        assert!(
            output.status.success(),
            "target {target}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
