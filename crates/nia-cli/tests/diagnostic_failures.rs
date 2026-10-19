// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::{Command, Output};

mod support;

use nia_test_support::{CommandExt, test_dir as temp_dir};

fn assert_operational_failure(
    output: &Output,
    format: &str,
    code: &str,
    summary: &str,
    path: &str,
    expected_stdout: &[u8],
) {
    assert_failure_report(output, 1, format, code, summary, path, expected_stdout);
}

fn assert_failure_report(
    output: &Output,
    status: i32,
    format: &str,
    code: &str,
    summary: &str,
    path: &str,
    expected_stdout: &[u8],
) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(status), "{stderr}");
    assert_eq!(output.stdout, expected_stdout, "{stderr}");
    assert!(!stderr.contains('\x1b'), "{stderr}");
    if format == "text" {
        assert!(stderr.contains(&format!("error[{code}]")), "{stderr}");
        assert!(stderr.contains(summary), "{stderr}");
        return;
    }
    let report: serde_json::Value = serde_json::from_str(&stderr).expect("complete JSON report");
    let diagnostics = report["diagnostics"]
        .as_array()
        .expect("diagnostic entries");
    assert_eq!(diagnostics.len(), 1, "{stderr}");
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic["code"], code, "{stderr}");
    assert_eq!(diagnostic["kind"], "independent", "{stderr}");
    assert_eq!(diagnostic["severity"], "error", "{stderr}");
    assert_eq!(diagnostic["category"], "user", "{stderr}");
    assert!(
        diagnostic["summary"].as_str().unwrap().contains(summary),
        "{stderr}"
    );
    assert!(
        diagnostic["labels"].as_array().unwrap().is_empty(),
        "{stderr}"
    );
    assert_eq!(
        std::path::Path::new(diagnostic["path"].as_str().unwrap()),
        std::path::Path::new(path),
        "{stderr}"
    );
    assert_eq!(report["summary"]["errors"], 1, "{stderr}");
    assert_eq!(report["summary"]["warnings"], 0, "{stderr}");
}

#[test]
fn unavailable_inputs_preserve_operational_identity_in_both_formats() {
    let root = temp_dir("unavailable_input_reports");
    for args in [
        &["check"][..],
        &["emit", "--checked"],
        &["emit", "--llvm"],
        &["emit", "--obj"],
        &["build"],
        &["test"],
    ] {
        for format in ["text", "json"] {
            let workflow = matches!(args[0], "build" | "test");
            let mut command = support::nia_command();
            command
                .args(args)
                .arg(format!("--diagnostics-format={format}"));
            if workflow {
                command.arg("--root").arg(&*root);
            } else {
                command.arg(root.join("missing.nia"));
            }
            let output = command.output_timeout_for_build("report unavailable command input");
            let (code, summary) = if workflow {
                ("E0704", "could not find `build.nia`")
            } else {
                ("E0102", "failed to read")
            };
            assert_operational_failure(
                &output,
                format,
                code,
                summary,
                if workflow {
                    "<unknown>"
                } else {
                    "<command line>"
                },
                b"",
            );
        }
    }
}

#[test]
fn missing_toolchain_preserves_operational_identity_across_commands() {
    let root = temp_dir("missing_toolchain_reports");
    for args in [&["check"][..], &["emit", "--llvm"], &["build"], &["test"]] {
        for format in ["text", "json"] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_nia"));
            command
                .arg("--resource-root")
                .arg(root.join("absent-resources"))
                .args(args)
                .arg(format!("--diagnostics-format={format}"));
            if matches!(args[0], "build" | "test") {
                command.arg("--root").arg(&*root);
            } else {
                command.arg(root.join("absent.nia"));
            }
            let output = command.output_timeout_for_build("report missing toolchain");
            assert_operational_failure(
                &output,
                format,
                "E0104",
                "invalid toolchain layout",
                "<command line>",
                b"",
            );
        }
    }
}

#[test]
fn artifact_write_errors_preserve_operational_identity_in_both_formats() {
    let root = temp_dir("artifact_write_reports");
    let main = root.join("main.nia");
    std::fs::write(&main, "pub fn main() i32 { 0 }\n").expect("valid entry");
    for (flag, output_path, summary) in [("-o", &*root, "write"), ("--out-dir", &main, "create")] {
        for format in ["text", "json"] {
            let output = support::nia_command()
                .args(["emit", "--obj"])
                .arg(format!("--diagnostics-format={format}"))
                .arg(&main)
                .arg(flag)
                .arg(output_path)
                .output_timeout_for_compiler("reject an output path that is a directory");
            assert_operational_failure(
                &output,
                format,
                "E0702",
                summary,
                &main.to_string_lossy(),
                b"",
            );
        }
    }
}

#[test]
fn invalid_link_configuration_preserves_both_report_formats() {
    let root = temp_dir("link_configuration_reports");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        include_str!("cases/linker/selection_errors/main.nia"),
    )
    .expect("valid entry");
    // A link configuration the linker rejects is an operational failure; an
    // option the command does not support is a usage error, reported before
    // anything is compiled.
    for (args, status, code, summary, path) in [
        (
            ["--linker-flavor", "self-hosted-elf"],
            1,
            "E0701",
            "invalid linker configuration",
            main.to_string_lossy().into_owned(),
        ),
        (
            ["--runtime", "bare"],
            2,
            "E0105",
            "supports only `--runtime freestanding`",
            "<command line>".to_string(),
        ),
    ] {
        for format in ["text", "json"] {
            let output = support::nia_command()
                .args(["emit", "--exe"])
                .arg(&main)
                .args(args)
                .arg("-o")
                .arg(root.join("program"))
                .arg(format!("--diagnostics-format={format}"))
                .output_timeout_for_compiler("report invalid executable configuration");
            assert_failure_report(&output, status, format, code, summary, &path, b"");
        }
    }
}

#[test]
fn runner_failure_preserves_captured_output_across_workflows() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = temp_dir("runner_failure_reports");
    std::fs::write(
        root.join("build.nia"),
        r#"using std::build;
 pub fn build(b: &mut build::Build) build::Error!() {
    _ = b;
    build::Error::Invalid { operation: .Validate, subject: .Build }!
}
"#,
    )
    .expect("failing runner source");
    for workflow in ["build", "test"] {
        for format in ["text", "json"] {
            let output = support::nia_command()
                .arg(workflow)
                .arg("--root")
                .arg(&*root)
                .arg(format!("--diagnostics-format={format}"))
                .output_timeout_in_session("report runner failure");
            assert_operational_failure(
                &output,
                format,
                "E0703",
                "build runner exited unsuccessfully",
                "<unknown>",
                b"",
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Text mode already showed the runner's output; JSON captured it.
            assert_eq!(stderr.matches("build error:").count(), 1, "{stderr}");
            assert_eq!(
                stderr.contains("stderr tail (bounded):"),
                format == "json",
                "{stderr}"
            );
            assert!(stderr.contains("exited with status 22"), "{stderr}");
        }
    }
}

#[test]
fn external_command_failure_preserves_streams_without_corrupting_json() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = temp_dir("external_failure_reports");
    std::fs::write(root.join("build.nia"), r#"using std::build;
pub fn build(b: &mut build::Build) build::Error!() {
    let step = (if b.hostTarget().os().equals(&"windows") {
        let args = [
            build::CommandArgument::literal(&"/d"),
            build::CommandArgument::literal(&"/c"),
            build::CommandArgument::literal(&"echo external-stdout& echo external-stderr 1>&2& exit /b 7"),
        ];
        b.addExternalCommandStep(&"fail", build::ExternalCommandOptions::search(&"cmd.exe").withArguments(&args))
    } else {
        let args = [
            build::CommandArgument::literal(&"-c"),
            build::CommandArgument::literal(&"echo external-stdout; echo external-stderr >&2; exit 7"),
        ];
        b.addExternalCommandStep(&"fail", build::ExternalCommandOptions::search(&"sh").withArguments(&args))
    }).?;
    b.setDefaultStep(step).?;
    !()
}
"#).expect("external command source");
    for format in ["text", "json"] {
        let output = support::nia_command()
            .arg("build")
            .arg("--root")
            .arg(&*root)
            .arg(format!("--diagnostics-format={format}"))
            .output_timeout_in_session("report external command failure");
        let stdout = if format == "json" {
            &b""[..]
        } else if cfg!(windows) {
            &b"external-stdout\r\n"[..]
        } else {
            &b"external-stdout\n"[..]
        };
        assert_operational_failure(
            &output,
            format,
            "E0705",
            "external command action `fail` failed",
            "<unknown>",
            stdout,
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("command exited with status 7"), "{stderr}");
        assert_eq!(stderr.matches("external-stderr").count(), 1, "{stderr}");
        if format == "json" {
            // Captured output only reaches the user through the report.
            for expected in [
                "stdout tail (bounded):",
                "external-stdout",
                "stderr tail (bounded):",
            ] {
                assert!(stderr.contains(expected), "{stderr}");
            }
        } else {
            // Forwarded output was already shown and is not repeated.
            assert!(!stderr.contains("tail (bounded)"), "{stderr}");
            assert!(!stderr.contains("external-stdout"), "{stderr}");
        }
    }
}

#[cfg(windows)]
#[test]
fn external_command_search_tries_pathext_with_and_without_a_cache_policy() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = temp_dir("external_command_pathext");
    std::fs::write(
        root.join("nia.toml"),
        "[package]\nname = \"pathext\"\nversion = \"0.1.0\"\n",
    )
    .expect("package manifest");
    std::fs::write(root.join("input.txt"), "pathext\n").expect("command input");
    // `cmd` has no extension, so both steps only resolve through `PATHEXT`.
    // The cached step also pins the program into its cache identity, and the
    // uncached one must run the same file the cached lookup chose.
    std::fs::write(
        root.join("build.nia"),
        r#"using std::build;
pub fn build(b: &mut build::Build) build::Error!() {
    let plainArgs = [
        build::CommandArgument::literal(&"/d"),
        build::CommandArgument::literal(&"/c"),
        build::CommandArgument::literal(&"exit /b 0"),
    ];
    let plain = b.addExternalCommandStep(&"plain", build::ExternalCommandOptions::search(&"cmd").withArguments(&plainArgs)).?;
    let cachedArgs = [
        build::CommandArgument::literal(&"/d"),
        build::CommandArgument::literal(&"/c"),
        build::CommandArgument::literal(&"copy"),
        build::CommandArgument::literal(&"/y"),
        build::CommandArgument::packageInput(b.rootPackage(), std::fs::PathView::init(&"input.txt")),
        build::CommandArgument::buildOutput(build::BuildPathView::init(&"pathext.txt")),
    ];
    let cached = b.addExternalCommandStep(&"cached", build::ExternalCommandOptions::search(&"cmd").withArguments(&cachedArgs).withToolAndInputCache()).?;
    let all = b.addAggregateStep(&"all").?;
    b.dependOn(all, plain).?;
    b.dependOn(all, cached).?;
    b.setDefaultStep(all).?;
    !()
}
"#,
    )
    .expect("build script");
    let output = support::nia_command()
        .arg("build")
        .arg("--root")
        .arg(&*root)
        .output_timeout_in_session("run PATHEXT external commands");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let written = std::fs::read_to_string(root.join(".nia-build/pathext.txt"))
        .expect("cached command output");
    assert_eq!(written.trim(), "pathext");
}

#[cfg(unix)]
#[test]
fn linker_process_failure_preserves_both_captured_streams() {
    use std::os::unix::fs::PermissionsExt;
    let root = temp_dir("linker_failure_reports");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        include_str!("cases/linker/selection_errors/main.nia"),
    )
    .expect("valid entry");
    let linker = root.join("linker.sh");
    std::fs::write(
        &linker,
        "#!/bin/sh\necho linker-stdout\necho linker-stderr >&2\nexit 7\n",
    )
    .expect("mock linker");
    std::fs::set_permissions(&linker, std::fs::Permissions::from_mode(0o755))
        .expect("executable linker");
    for format in ["text", "json"] {
        let output = support::nia_command()
            .args(["emit", "--exe"])
            .arg(&main)
            .arg("--linker")
            .arg(&linker)
            .arg("-o")
            .arg(root.join("program"))
            .arg(format!("--diagnostics-format={format}"))
            .output_timeout_for_compiler("report linker process failure");
        assert_operational_failure(
            &output,
            format,
            "E0701",
            "linker",
            &main.to_string_lossy(),
            b"",
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        for expected in ["linker-stdout", "linker-stderr", "7"] {
            assert!(stderr.contains(expected), "{stderr}");
        }
    }
}

#[cfg(unix)]
#[test]
fn archive_process_failure_preserves_streams_across_workflows() {
    use std::os::unix::fs::PermissionsExt;
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = temp_dir("archive_failure_reports");
    std::fs::write(
        root.join("main.nia"),
        include_str!("cases/linker/selection_errors/main.nia"),
    )
    .expect("valid entry");
    std::fs::write(
        root.join("build.nia"),
        r#"using std::build;
using std::fs;
pub fn build(b: &mut build::Build) build::Error!() {
    let root = b.addModule(build::ModuleOptions::init(&"app", fs::PathView::init(&"main.nia"))).?;
    let archive = b.addStaticArchive(build::StaticArchiveOptions::init(&"library", root)).?;
    let archiveStep = b.addEmitStaticArchiveStep(&"archive", archive).?;
    let testStep = b.addTestSuite(&"suite", root).?;
    b.dependOn(testStep, archiveStep).?;
    b.setDefaultStep(archiveStep).?;
     !()
 }
"#,
    )
    .expect("archive build source");

    let tool = root.join("archive.sh");
    std::fs::write(
        &tool,
        "#!/bin/sh\necho archive-stdout\necho archive-stderr >&2\nexit 7\n",
    )
    .expect("mock archive tool");
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755))
        .expect("executable tool");
    for workflow in ["build", "test"] {
        for format in ["text", "json"] {
            let output = support::nia_command()
                .env("NIA_AR", &tool)
                .arg(workflow)
                .arg("--root")
                .arg(&*root)
                .args(["--jobs", "1"])
                .arg(format!("--diagnostics-format={format}"))
                .output_timeout_in_session("report archive process failure");
            assert_operational_failure(&output, format, "E0701", "archive tool", "<unknown>", b"");
            let stderr = String::from_utf8_lossy(&output.stderr);
            for expected in ["archive-stdout", "archive-stderr", "exited with status"] {
                assert!(stderr.contains(expected), "{stderr}");
            }
        }
    }
}

#[cfg(windows)]
#[test]
fn windows_linker_process_failure_preserves_both_captured_streams() {
    let root = temp_dir("windows_linker_failure_reports");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        include_str!("cases/linker/selection_errors/main.nia"),
    )
    .expect("valid entry");
    let linker = root.join("linker.cmd");
    std::fs::write(
        &linker,
        "@echo off\r\necho linker-stdout\r\necho linker-stderr 1>&2\r\nexit /b 7\r\n",
    )
    .expect("mock linker");
    for format in ["text", "json"] {
        let output = support::nia_command()
            .args(["emit", "--exe"])
            .arg(&main)
            .arg("--linker")
            .arg(&linker)
            .arg("-o")
            .arg(root.join("program.exe"))
            .arg(format!("--diagnostics-format={format}"))
            .output_timeout_for_compiler("report Windows linker process failure");
        assert_operational_failure(
            &output,
            format,
            "E0701",
            "linker",
            &main.to_string_lossy(),
            b"",
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        for expected in ["linker-stdout", "linker-stderr", "7"] {
            assert!(stderr.contains(expected), "{stderr}");
        }
    }
}

#[cfg(windows)]
#[test]
fn windows_archive_process_failure_preserves_streams_across_workflows() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = temp_dir("windows_archive_failure_reports");
    std::fs::write(
        root.join("main.nia"),
        include_str!("cases/linker/selection_errors/main.nia"),
    )
    .expect("valid entry");
    std::fs::write(
        root.join("build.nia"),
        r#"using std::build;
using std::fs;
pub fn build(b: &mut build::Build) build::Error!() {
    let root = b.addModule(build::ModuleOptions::init(&"app", fs::PathView::init(&"main.nia"))).?;
    let archive = b.addStaticArchive(build::StaticArchiveOptions::init(&"library", root)).?;
    let archiveStep = b.addEmitStaticArchiveStep(&"archive", archive).?;
    let testStep = b.addTestSuite(&"suite", root).?;
    b.dependOn(testStep, archiveStep).?;
    b.setDefaultStep(archiveStep).?;
    !()
}
"#,
    )
    .expect("archive build source");
    let tool = root.join("archive.cmd");
    std::fs::write(
        &tool,
        "@echo off\r\necho archive-stdout\r\necho archive-stderr 1>&2\r\nexit /b 7\r\n",
    )
    .expect("mock archive tool");
    for workflow in ["build", "test"] {
        for format in ["text", "json"] {
            let output = support::nia_command()
                .env("NIA_AR", &tool)
                .arg(workflow)
                .arg("--root")
                .arg(&*root)
                .args(["--jobs", "1"])
                .arg(format!("--diagnostics-format={format}"))
                .output_timeout_in_session("report Windows archive process failure");
            assert_operational_failure(&output, format, "E0701", "archive tool", "<unknown>", b"");
            let stderr = String::from_utf8_lossy(&output.stderr);
            for expected in ["archive-stdout", "archive-stderr", "exited with status"] {
                assert!(stderr.contains(expected), "{stderr}");
            }
        }
    }
}
