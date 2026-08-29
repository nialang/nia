// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_reports_private_entry_main_called_by_freestanding_start() {
    let root = temp_dir("emit_exe_reports_private_entry_main_called_by_freestanding_start");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
using std::process;

fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    process::exit(7)!
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .output_timeout_for_build("run nia emit --exe");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("private"), "{stderr}");
    assert!(stderr.contains("entry::main"), "{stderr}");
}

#[test]
fn emit_exe_entry_name_is_chosen_by_std_runtime_not_compiler() {
    let root = temp_dir("emit_exe_entry_name_is_chosen_by_std_runtime_not_compiler");
    let main = root.join("main.nia");
    let std_root = root.join("custom_std/std.nia");
    let std_builtin = root.join("custom_std/std/builtin.nia");
    let std_process = root.join("custom_std/std/process.nia");
    let std_start = root.join("custom_std/std/start.nia");
    let std_start_freestanding = root.join("custom_std/std/start/freestanding.nia");
    let std_start_freestanding_linux = root.join("custom_std/std/start/freestanding/linux.nia");
    let std_start_linux_x86_64 = root.join("custom_std/std/start/freestanding/linux/x86_64.nia");
    let std_start_linux_x86 = root.join("custom_std/std/start/freestanding/linux/x86.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::create_dir_all(std_start_linux_x86_64.parent().expect("std start parent"))
        .expect("create custom std dir");
    std::fs::write(
        &std_root,
        r#"
pub module builtin;
"#,
    )
    .expect("write custom std root");
    std::fs::write(
        &std_builtin,
        r#"
@[builtin("AsmConfig")]
pub type AsmConfig;

@[builtin("AsmInputs")]
pub type AsmInputs;

@[builtin("AsmOutputs")]
pub type AsmOutputs;

@[builtin("asm")]
pub fn asm(config: AsmConfig) ();
"#,
    )
    .expect("write custom std builtin");
    std::fs::write(&std_process, "").expect("write custom std process");
    std::fs::write(
        &std_start,
        r#"
@[if os == "linux" and (arch == "x86_64" or arch == "x86")]
pub(pkg) module freestanding;
@[if os == "linux" and arch == "x86_64"]
using pkg::start::freestanding::linux::x86_64;
@[if os == "linux" and arch == "x86"]
using pkg::start::freestanding::linux::x86;
"#,
    )
    .expect("write custom std start facade");
    std::fs::write(
        &std_start_freestanding,
        r#"
@[if os == "linux"]
pub(pkg) module linux;
"#,
    )
    .expect("write custom std freestanding facade");
    std::fs::write(
        &std_start_freestanding_linux,
        r#"
@[if arch == "x86_64"]
pub(pkg) module x86_64;
@[if arch == "x86"]
pub(pkg) module x86;
"#,
    )
    .expect("write custom std linux facade");
    std::fs::write(
        &std_start_linux_x86_64,
        r#"
using entry;

fn syscall_exit(code: i32) () {
    std::builtin::asm(std::builtin::AsmConfig {
        code:
            b\\syscall
        ,
        inputs: std::builtin::AsmInputs {
            rax: 60,
            rdi: code,
        },
        clobbers: [b"rcx", b"r11", b"memory"],
        options: [b"volatile"],
    });
}

@[naked]
pub extern fn _start() () {
    std::builtin::asm(std::builtin::AsmConfig {
        code:
            b\\call custom_start
            \\ud2
        ,
        clobbers: [b"rax", b"rcx", b"r11", b"memory"],
        options: [b"volatile"],
    });
    loop {}
}

extern fn custom_start() () {
    syscall_exit(entry::mymain());
    loop {}
}
"#,
    )
    .expect("write custom std start");
    std::fs::write(
        &std_start_linux_x86,
        r#"
using entry;

fn syscall_exit(code: i32) () {
    std::builtin::asm(std::builtin::AsmConfig {
        code:
            b\\int 0x80
        ,
        inputs: std::builtin::AsmInputs {
            eax: 1,
            ebx: code,
        },
        clobbers: [b"memory"],
        options: [b"volatile"],
    });
}

@[naked]
pub extern fn _start() () {
    std::builtin::asm(std::builtin::AsmConfig {
        code:
            b\\call custom_start
            \\ud2
        ,
        clobbers: [b"eax", b"ecx", b"edx", b"memory"],
        options: [b"volatile"],
    });
    loop {}
}

extern fn custom_start() () {
    syscall_exit(entry::mymain());
    loop {}
}
"#,
    )
    .expect("write custom i686 std start");
    std::fs::write(
        &main,
        r#"
pub fn mymain() i32 {
    11
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-M")
        .arg(format!("std={}", std_root.display()))
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe with custom std start");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(11));
}

#[test]
fn emit_exe_preserves_output_paths_that_look_like_optimization_flags() {
    let root = temp_dir("emit_exe_preserves_output_paths_that_look_like_optimization_flags");
    let main = root.join("main.nia");
    let exe_name = format!("-Orunnable{}", std::env::consts::EXE_SUFFIX);
    let exe = root.join(&exe_name);
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    process::exit(9)!
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
        .current_dir(&root)
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe_name)
        .output_timeout_for_build("run nia emit --exe -o -Orunnable");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(9));

    let report_name = format!("--opt-report{}", std::env::consts::EXE_SUFFIX);
    let report_path = root.join(&report_name);
    let output = support::nia_command()
        .current_dir(&root)
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&report_name)
        .output_timeout_for_build("run nia emit --exe -o --opt-report");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("backend optimization report:"), "{stdout}");
    assert!(!stderr.contains("backend optimization report:"), "{stderr}");

    let status =
        Command::new(&report_path).status_timeout("run emitted executable named --opt-report");
    assert_eq!(status.code(), Some(9));
}

#[test]
fn emit_exe_can_emit_optimization_report_to_stderr() {
    let root = temp_dir("emit_exe_can_emit_optimization_report_to_stderr");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    process::exit(5)!
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
        .arg("-Oz")
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .arg("--opt-report")
        .output_timeout_for_build("run nia emit --exe --opt-report");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!stdout.contains("backend optimization report:"), "{stdout}");
    assert!(stderr.contains("backend optimization report:"), "{stderr}");
    assert!(stderr.contains("policy level=Oz"), "{stderr}");
    assert!(stderr.contains("llvm_codegen=less"), "{stderr}");
    assert!(stderr.contains("llvm_size=tiny"), "{stderr}");

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(5));
}
