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
    assert!(help_stdout.contains("emit obj <file.nia>"), "{help_stdout}");

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

fn temp_dir(name: &str) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("nia_cli_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}
