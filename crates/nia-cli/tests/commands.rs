// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

#[test]
fn help_and_version_use_niac_command_name() {
    let help = Command::new(env!("CARGO_BIN_EXE_niac"))
        .arg("--help")
        .output()
        .expect("run niac --help");
    assert!(
        help.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&help.stderr)
    );
    let help_stdout = String::from_utf8_lossy(&help.stdout);
    assert!(help_stdout.contains("Usage:\n  niac"), "{help_stdout}");
    assert!(
        help_stdout.contains("emit <target> <file.nia>"),
        "{help_stdout}"
    );
    assert!(help_stdout.contains("-O0..-O3"), "{help_stdout}");
    assert!(help_stdout.contains("-Os"), "{help_stdout}");
    assert!(help_stdout.contains("-Oz"), "{help_stdout}");

    let emit_obj_help = Command::new(env!("CARGO_BIN_EXE_niac"))
        .arg("help")
        .arg("emit")
        .arg("obj")
        .output()
        .expect("run niac help emit obj");
    assert!(
        emit_obj_help.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit_obj_help.stderr)
    );
    let emit_obj_stdout = String::from_utf8_lossy(&emit_obj_help.stdout);
    assert!(
        emit_obj_stdout.contains("niac emit obj <file.nia>"),
        "{emit_obj_stdout}"
    );
    assert!(
        emit_obj_stdout.contains("--out-dir <dir>"),
        "{emit_obj_stdout}"
    );

    let version = Command::new(env!("CARGO_BIN_EXE_niac"))
        .arg("--version")
        .output()
        .expect("run niac --version");
    assert!(
        version.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&version.stderr)
    );
    let version_stdout = String::from_utf8_lossy(&version.stdout);
    assert!(version_stdout.starts_with("niac "), "{version_stdout}");
}

#[test]
fn optimization_option_can_precede_emit_command() {
    let root = temp_dir("optimization_option_can_precede_emit_command");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    0
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_niac"))
        .arg("-O2")
        .arg("emit")
        .arg("llvm")
        .arg(&main)
        .output()
        .expect("run niac -O2 emit llvm");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("define i32 @"), "{stdout}");
}

#[test]
fn invalid_optimization_option_reports_expected_levels() {
    let output = Command::new(env!("CARGO_BIN_EXE_niac"))
        .arg("-O9")
        .arg("check")
        .arg("main.nia")
        .output()
        .expect("run niac with invalid optimization option");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown optimization level `-O9`"),
        "{stderr}"
    );
    assert!(stderr.contains("-Oz"), "{stderr}");
}

#[test]
fn module_map_option_can_follow_command_arguments() {
    let root = temp_dir("module_map_option_can_follow_command_arguments");
    let main = root.join("main.nia");
    let mapped = root.join("share.nia");
    std::fs::write(
        &main,
        r#"
import share;

fn main() i32 {
    share::answer
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        &mapped,
        r#"
pub comptime answer: i32 = 42;
"#,
    )
    .expect("write mapped source");

    let output = Command::new(env!("CARGO_BIN_EXE_niac"))
        .arg("check")
        .arg(&main)
        .arg("-M")
        .arg(format!("share={}", mapped.display()))
        .output()
        .expect("run niac check with trailing -M");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn emit_llvm_prints_checked_program_ir() {
    let root = temp_dir("emit_llvm_prints_checked_program_ir");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    var x = 41;
    x + 1
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_niac"))
        .arg("emit")
        .arg("llvm")
        .arg(&main)
        .output()
        .expect("run niac emit llvm");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("define i32 @"), "{stdout}");
    assert!(stdout.contains("ret i32"), "{stdout}");
}

#[test]
fn emit_obj_writes_native_object() {
    let root = temp_dir("emit_obj_writes_native_object");
    let main = root.join("main.nia");
    let object = root.join("main.o");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    0
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_niac"))
        .arg("emit")
        .arg("obj")
        .arg(&main)
        .arg("-o")
        .arg(&object)
        .output()
        .expect("run niac emit obj");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata = std::fs::metadata(&object).expect("object metadata");
    assert!(metadata.len() > 0);
}

#[test]
fn emit_exe_links_hosted_executable() {
    let root = temp_dir("emit_exe_links_hosted_executable");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    7
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_niac"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run niac emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(7));
}

#[test]
fn emitted_executables_preserve_semantics_across_optimization_levels() {
    let root = temp_dir("emitted_executables_preserve_semantics_across_optimization_levels");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn pick(flag: bool, a: i32, b: i32) i32 {
    if flag {
        a
    } else {
        b
    }
}

fn main() i32 {
    var x = 39;
    var y = x;
    y = y + 1;
    var unused = 99;
    pick(true, y + 2, unused)
}
"#,
    )
    .expect("write test source");

    for level in ["-O0", "-O1", "-O2", "-O3", "-Os", "-Oz", "-O"] {
        let exe_name = format!(
            "main_{}{}",
            level.trim_start_matches('-'),
            std::env::consts::EXE_SUFFIX
        );
        let exe = root.join(exe_name);
        let output = Command::new(env!("CARGO_BIN_EXE_niac"))
            .arg(level)
            .arg("emit")
            .arg("exe")
            .arg(&main)
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap_or_else(|error| panic!("run niac {level} emit exe: {error}"));

        assert!(
            output.status.success(),
            "{level} stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );

        let status = Command::new(&exe)
            .status()
            .unwrap_or_else(|error| panic!("run emitted executable for {level}: {error}"));
        assert_eq!(status.code(), Some(42), "{level}");
    }
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("nia_cli_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}
