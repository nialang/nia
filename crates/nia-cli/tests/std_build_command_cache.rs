// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg(any(unix, windows))]

mod support;

use std::{fs, io::Write, path::Path, process::Command};

use nia_test_support::{CommandExt, test_dir};

fn command_fixture(root: &Path, policy: &str, explicit_environment: bool) {
    let program = if cfg!(windows) {
        "nia-cache-tool.exe"
    } else {
        "nia-cache-tool"
    };
    let source = if cfg!(windows) {
        let system_root = std::env::var_os("SystemRoot").expect("Windows system root");
        std::path::PathBuf::from(system_root).join("System32/cmd.exe")
    } else {
        std::path::PathBuf::from("/bin/sh")
    };
    fs::create_dir(root.join("tools")).expect("create tool directory");
    fs::copy(source, root.join("tools").join(program)).expect("copy fixture interpreter");
    let script = if cfg!(windows) {
        "@echo off\r\n> \"%~2\" echo(%NIA_COMMAND_VALUE%\r\ntype \"%~1\" >> \"%~2\"\r\n"
    } else {
        "printf '%s\\n' \"$NIA_COMMAND_VALUE\" > \"$2\"\nwhile IFS= read -r line; do printf '%s\\n' \"$line\"; done < \"$1\" >> \"$2\"\n"
    };
    fs::write(root.join("command-script"), script).expect("write command script");
    // cmd.exe requires a .cmd extension even when the script is a declared input.
    let script_name = if cfg!(windows) {
        "command-script.cmd"
    } else {
        "command-script"
    };
    if cfg!(windows) {
        fs::rename(root.join("command-script"), root.join(script_name)).expect("name cmd script");
    }
    fs::write(root.join("input.txt"), "first\n").expect("write declared input");
    let prefix = if cfg!(windows) {
        "build::CommandArgument::literal(&\"/d\"),\n        build::CommandArgument::literal(&\"/c\"),"
    } else {
        ""
    };
    let environment = if explicit_environment {
        ".withEnvironment(&environment[..])"
    } else {
        ""
    };
    let clear = if policy == "withDeclaredInputCache" {
        ".withClearedEnvironment()"
    } else {
        ""
    };
    fs::write(
        root.join("build.nia"),
        format!(
            r#"
using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {{
    let arguments = [
        {prefix}
        build::CommandArgument::packageInput(b.rootPackage(), fs::PathView::init(&"{script_name}")),
        build::CommandArgument::packageInput(b.rootPackage(), fs::PathView::init(&"input.txt")),
        build::CommandArgument::buildOutput(build::BuildPathView::init(&"result.txt")),
    ];
    let environment = [build::CommandEnvironmentInput::init(&"NIA_COMMAND_VALUE", &"override")];
    let step = b.addExternalCommandStep(
        &"cached-command",
        build::ExternalCommandOptions::search(&"{program}")
            .withArguments(&arguments[..]){environment}{clear}.{policy}(),
    ).?;
    b.setDefaultStep(step)
}}
"#
        ),
    )
    .expect("write command build script");
}

fn build_command(root: &Path, value: Option<&str>) -> Command {
    let mut command = support::nia_command();
    let path = std::env::join_paths(std::iter::once(root.join("tools")).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("fixture PATH");
    command.env("PATH", path).env_remove("NIA_COMMAND_VALUE");
    if let Some(value) = value {
        command.env("NIA_COMMAND_VALUE", value);
    }
    command
        .args(["--diagnostics-format=json", "build", "--root"])
        .arg(root)
        .arg("--print-cache");
    command
}

fn cache_outcome(root: &Path, value: Option<&str>) -> String {
    command_cache_outcome(build_command(root, value))
}

fn command_cache_outcome(mut command: Command) -> String {
    let output = command.output_timeout_for_build("run command cache fixture");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let report = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|value| value["kind"] == "action-cache")
        .unwrap_or_else(|| panic!("missing action cache report: {stdout}"));
    assert_eq!(report["entries"].as_array().unwrap().len(), 1, "{report}");
    assert!(
        report["entries"][0]["decision"]["status"].is_string(),
        "{report}"
    );
    report["entries"][0]["outcome"].as_str().unwrap().to_owned()
}

#[test]
fn declared_command_policies_restore_outputs_and_invalidate_inputs_and_tools() {
    for policy in ["withDeclaredInputCache", "withToolAndInputCache"] {
        let root = test_dir(policy);
        command_fixture(&root, policy, true);
        assert_eq!(cache_outcome(&root, Some("inherited")), "Miss(NotFound)");
        let result = root.join(".nia-build/result.txt");
        let expected = fs::read(&result).expect("cold output");
        assert!(String::from_utf8_lossy(&expected).contains("override"));
        fs::remove_file(&result).expect("remove output before restore");
        assert_eq!(cache_outcome(&root, Some("changed inherited value")), "Hit");
        assert_eq!(fs::read(&result).unwrap(), expected);
        fs::write(root.join("input.txt"), "second\n").unwrap();
        assert_eq!(cache_outcome(&root, None), "Miss(Invalidated([Inputs]))");
        assert!(fs::read_to_string(&result).unwrap().contains("second"));

        let program = if cfg!(windows) {
            "nia-cache-tool.exe"
        } else {
            "nia-cache-tool"
        };
        let mut tool = fs::OpenOptions::new()
            .append(true)
            .open(root.join("tools").join(program))
            .unwrap();
        tool.write_all(b"\nchanged tool identity\n").unwrap();
        drop(tool);
        assert_eq!(
            cache_outcome(&root, None),
            "Miss(Invalidated([ExternalTool]))"
        );
        assert_eq!(cache_outcome(&root, None), "Hit");

        let replacement = root.join("replacement");
        fs::create_dir(&replacement).unwrap();
        fs::copy(root.join("tools").join(program), replacement.join(program)).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(replacement.join(program))
            .unwrap()
            .write_all(b"\nreplacement PATH tool\n")
            .unwrap();
        let mut command = build_command(&root, None);
        command.env(
            "PATH",
            std::env::join_paths(std::iter::once(replacement).chain(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            )))
            .unwrap(),
        );
        assert_eq!(
            command_cache_outcome(command),
            "Miss(Invalidated([ExternalTool]))"
        );
    }
}

#[test]
fn full_environment_command_cache_distinguishes_absent_empty_and_changed_values() {
    let root = test_dir("full_environment_command_cache");
    command_fixture(&root, "withFullEnvironmentCache", false);
    assert_eq!(cache_outcome(&root, None), "Miss(NotFound)");
    assert_eq!(cache_outcome(&root, None), "Hit");
    for value in ["", "first", "second"] {
        assert_eq!(
            cache_outcome(&root, Some(value)),
            "Miss(Invalidated([Environment]))"
        );
        assert_eq!(cache_outcome(&root, Some(value)), "Hit");
        let result = root.join(".nia-build/result.txt");
        let expected = format!("{value}\nfirst\n");
        assert_eq!(
            fs::read_to_string(&result).unwrap().replace("\r\n", "\n"),
            expected
        );
        fs::remove_file(&result).unwrap();
        assert_eq!(cache_outcome(&root, Some(value)), "Hit");
        assert_eq!(
            fs::read_to_string(&result).unwrap().replace("\r\n", "\n"),
            expected
        );
    }
}

#[test]
fn full_environment_command_cache_rejects_explicit_overrides() {
    let root = test_dir("full_environment_command_overrides");
    command_fixture(&root, "withFullEnvironmentCache", true);
    let source = fs::read_to_string(root.join("build.nia")).unwrap();
    for source in [
        source.clone(),
        source.replace(
            ".withEnvironment(&environment[..]).withFullEnvironmentCache()",
            ".withFullEnvironmentCache().withEnvironment(&environment[..])",
        ),
    ] {
        fs::write(root.join("build.nia"), source).unwrap();
        let output =
            build_command(&root, None).output_timeout_for_build("reject full environment override");
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("invalid"), "{stderr}");
        assert!(!root.join(".nia-build/result.txt").exists());
    }
}

#[test]
#[cfg(target_os = "linux")]
fn oversized_environment_executes_without_publishing_command_cache() {
    let root = test_dir("oversized_environment_command_cache");
    command_fixture(&root, "withFullEnvironmentCache", false);
    for _ in 0..2 {
        let mut command = build_command(&root, Some("large environment"));
        for index in 0..36 {
            command.env(format!("NIA_COMMAND_PADDING_{index}"), "x".repeat(30_000));
        }
        assert_eq!(command_cache_outcome(command), "Miss(Uncacheable)");
        assert!(
            fs::read_to_string(root.join(".nia-build/result.txt"))
                .unwrap()
                .contains("large environment")
        );
        assert!(!root.join(".nia-cache/actions/external-commands").exists());
    }
}
