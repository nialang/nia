// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn help_and_version_use_nia_command_name() {
    let help = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("--help")
        .output_timeout("run nia --help");
    assert!(
        help.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&help.stderr)
    );
    let help_stdout = String::from_utf8_lossy(&help.stdout);
    assert!(help_stdout.contains("Usage:\n  nia"), "{help_stdout}");
    assert!(
        help_stdout.contains("emit --<target> <file.nia>"),
        "{help_stdout}"
    );
    assert!(!help_stdout.contains("lex <file.nia>"), "{help_stdout}");
    assert!(!help_stdout.contains("parse <file.nia>"), "{help_stdout}");
    assert!(
        help_stdout.contains("-O, -O0, -O1, -O2, -O3, -Os, -Oz"),
        "{help_stdout}"
    );
    assert!(help_stdout.contains("--timings"), "{help_stdout}");
    for level in ["-O0", "-O1", "-O2", "-O3", "-Os", "-Oz"] {
        assert!(help_stdout.contains(level), "{help_stdout}");
    }

    let check_help = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("help")
        .arg("check")
        .output_timeout("run nia help check");
    assert!(
        check_help.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&check_help.stderr)
    );
    let check_stdout = String::from_utf8_lossy(&check_help.stdout);
    assert!(check_stdout.contains("--exe"), "{check_stdout}");
    assert!(
        check_stdout.contains("--runtime <bare|freestanding>"),
        "{check_stdout}"
    );
    assert!(check_stdout.contains("--opt-report"), "{check_stdout}");
    assert!(
        check_stdout.contains("-O, -O0, -O1, -O2, -O3, -Os, -Oz"),
        "{check_stdout}"
    );
    assert!(
        check_stdout.contains("optimization policy, enabled passes, change count, and changes"),
        "{check_stdout}"
    );
    assert!(check_stdout.contains("--timings"), "{check_stdout}");
    assert!(
        check_stdout.contains("Timing reports are written to stderr"),
        "{check_stdout}"
    );

    let emit_help = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("help")
        .arg("emit")
        .output_timeout("run nia help emit");
    assert!(
        emit_help.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit_help.stderr)
    );
    let emit_stdout = String::from_utf8_lossy(&emit_help.stdout);
    for target in [
        "--tokens",
        "--ast",
        "--checked",
        "--backend",
        "--llvm",
        "--obj",
        "--exe",
    ] {
        assert!(emit_stdout.contains(target), "{emit_stdout}");
    }
    assert!(
        emit_stdout.contains("nia emit --obj <file.nia>"),
        "{emit_stdout}"
    );
    assert!(emit_stdout.contains("--out-dir <dir>"), "{emit_stdout}");
    assert!(
        emit_stdout.contains("--runtime <bare|freestanding>"),
        "{emit_stdout}"
    );
    assert!(emit_stdout.contains("--link-arg <arg>"), "{emit_stdout}");
    assert!(emit_stdout.contains("--opt-report"), "{emit_stdout}");
    assert!(emit_stdout.contains("--timings"), "{emit_stdout}");
    assert!(
        emit_stdout
            .contains("optimization policy, enabled passes, change count, and changes to stderr"),
        "{emit_stdout}"
    );
    for level in ["-O0", "-O1", "-O2", "-O3", "-Os", "-Oz"] {
        assert!(emit_stdout.contains(level), "{emit_stdout}");
    }
    assert!(
        emit_stdout.contains("Timing reports are written to stderr"),
        "{emit_stdout}"
    );

    let version = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("--version")
        .output_timeout("run nia --version");
    assert!(
        version.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&version.stderr)
    );
    let version_stdout = String::from_utf8_lossy(&version.stdout);
    assert!(version_stdout.starts_with("nia "), "{version_stdout}");

    let version_status = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("--version")
        .status_timeout("run nia --version status");
    assert!(version_status.success());
}

#[test]
fn timings_option_reports_stage_timings_to_stderr() {
    let root = temp_dir("timings_option_reports_stage_timings_to_stderr");
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

    let check = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("check")
        .arg(&main)
        .arg("--timings")
        .output_timeout("run nia check --timings");
    assert!(
        check.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let stdout = String::from_utf8_lossy(&check.stdout);
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("timing check:"), "{stderr}");
    assert!(!stderr.contains("query timing"), "{stderr}");

    let tokens = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--tokens")
        .arg(&main)
        .arg("--timings")
        .output_timeout("run nia emit --tokens --timings");
    assert!(
        tokens.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&tokens.stderr)
    );
    let stdout = String::from_utf8_lossy(&tokens.stdout);
    let stderr = String::from_utf8_lossy(&tokens.stderr);
    assert!(stdout.contains("Fn"), "{stdout}");
    assert!(!stdout.contains("timing "), "{stdout}");
    assert!(stderr.contains("timing lex:"), "{stderr}");

    let llvm = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("--timings=detail")
        .arg("emit")
        .arg("--llvm")
        .arg(&main)
        .output_timeout("run nia --timings=detail emit --llvm");
    assert!(
        llvm.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let stdout = String::from_utf8_lossy(&llvm.stdout);
    let stderr = String::from_utf8_lossy(&llvm.stderr);
    assert!(stdout.contains("define i32 @"), "{stdout}");
    assert!(!stdout.contains("timing "), "{stdout}");
    assert!(stderr.contains("timing check:"), "{stderr}");
    assert!(stderr.contains("timing emit_llvm_ir:"), "{stderr}");
    assert!(
        stderr.contains("query timing backend_lowering:"),
        "{stderr}"
    );
}

#[test]
fn invalid_timings_option_reports_expected_modes() {
    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("--timings=trace")
        .arg("check")
        .arg("main.nia")
        .output_timeout("run nia with invalid timings option");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown timings mode `--timings=trace`"),
        "{stderr}"
    );
    assert!(stderr.contains("--timings=detail"), "{stderr}");
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

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("-O2")
        .arg("emit")
        .arg("--llvm")
        .arg(&main)
        .output_timeout("run nia -O2 emit --llvm");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("define i32 @"), "{stdout}");
}

#[test]
fn emit_can_print_frontend_inspection_stages() {
    let root = temp_dir("emit_can_print_frontend_inspection_stages");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    42
}
"#,
    )
    .expect("write test source");

    let tokens = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--tokens")
        .arg(&main)
        .output_timeout("run nia emit --tokens");
    assert!(
        tokens.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&tokens.stderr)
    );
    let stdout = String::from_utf8_lossy(&tokens.stdout);
    assert!(stdout.contains("Fn"), "{stdout}");
    assert!(stdout.contains("Ident"), "{stdout}");

    let ast = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--ast")
        .arg(&main)
        .output_timeout("run nia emit --ast");
    assert!(
        ast.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&ast.stderr)
    );
    let stdout = String::from_utf8_lossy(&ast.stdout);
    assert!(stdout.contains("FunctionItem"), "{stdout}");
    assert!(stdout.contains("main"), "{stdout}");

    let checked = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--checked")
        .arg(&main)
        .output_timeout("run nia emit --checked");
    assert!(
        checked.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let stdout = String::from_utf8_lossy(&checked.stdout);
    assert!(stdout.contains("CheckedProgram"), "{stdout}");
    assert!(stdout.contains("backend_lowering"), "{stdout}");
}

#[test]
fn removed_top_level_inspection_commands_are_rejected() {
    let root = temp_dir("removed_top_level_inspection_commands_are_rejected");
    let main = root.join("main.nia");
    std::fs::write(&main, "fn main() i32 { 0 }").expect("write test source");

    let lex = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("lex")
        .arg(&main)
        .output_timeout("run removed nia lex");
    assert!(!lex.status.success());
    let stderr = String::from_utf8_lossy(&lex.stderr);
    assert!(stderr.contains("unknown command `lex`"), "{stderr}");

    let parse = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("parse")
        .arg(&main)
        .output_timeout("run removed nia parse");
    assert!(!parse.status.success());
    let stderr = String::from_utf8_lossy(&parse.stderr);
    assert!(stderr.contains("unknown command `parse`"), "{stderr}");
}

#[test]
fn removed_emit_target_argument_syntax_is_rejected() {
    let root = temp_dir("removed_emit_target_argument_syntax_is_rejected");
    let main = root.join("main.nia");
    std::fs::write(&main, "fn main() i32 { 0 }").expect("write test source");

    let old_obj = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("obj")
        .arg(&main)
        .output_timeout("run removed nia emit obj");
    assert!(!old_obj.status.success());
    let stderr = String::from_utf8_lossy(&old_obj.stderr);
    assert!(
        stderr.contains("old `nia emit obj` syntax was removed; use `nia emit --obj`"),
        "{stderr}"
    );

    let missing_target = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg(&main)
        .output_timeout("run nia emit without target flag");
    assert!(!missing_target.status.success());
    let stderr = String::from_utf8_lossy(&missing_target.stderr);
    assert!(stderr.contains("missing emit target flag"), "{stderr}");

    let duplicate_target = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--llvm")
        .arg("--backend")
        .arg(&main)
        .output_timeout("run nia emit with duplicate target flags");
    assert!(!duplicate_target.status.success());
    let stderr = String::from_utf8_lossy(&duplicate_target.stderr);
    assert!(
        stderr.contains("use exactly one emit target flag"),
        "{stderr}"
    );
}

#[test]
fn optimization_option_can_follow_command_arguments() {
    let root = temp_dir("optimization_option_can_follow_command_arguments");
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

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("check")
        .arg(&main)
        .arg("-Oz")
        .arg("--opt-report")
        .output_timeout("run nia check main.nia -Oz --opt-report");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("policy level=Oz"), "{stdout}");
    assert!(stdout.contains("prefer_size=true"), "{stdout}");
    assert!(stdout.contains("llvm_size=tiny"), "{stdout}");
}

#[test]
fn bare_optimization_option_aliases_o2() {
    let root = temp_dir("bare_optimization_option_aliases_o2");
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

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("-O")
        .arg("check")
        .arg(&main)
        .arg("--opt-report")
        .output_timeout("run nia -O check --opt-report");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("policy level=O2"), "{stdout}");
    assert!(stdout.contains("inline=normal"), "{stdout}");
    assert!(stdout.contains("specialize=normal"), "{stdout}");
    assert!(stdout.contains("llvm_codegen=default"), "{stdout}");
    assert!(stdout.contains("llvm_size=default"), "{stdout}");
}

#[test]
fn invalid_optimization_option_reports_expected_levels() {
    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("-O9")
        .arg("check")
        .arg("main.nia")
        .output_timeout("run nia with invalid optimization option");

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
using share;

fn main() i32 {
    share::answer
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        &mapped,
        r#"
pub comptime let answer: i32 = 42;
"#,
    )
    .expect("write mapped source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("check")
        .arg(&main)
        .arg("-M")
        .arg(format!("share={}", mapped.display()))
        .output_timeout("run nia check with trailing -M");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn module_map_rejects_compiler_reserved_roots() {
    let root = temp_dir("module_map_rejects_compiler_reserved_roots");
    let main = root.join("main.nia");
    std::fs::write(&main, "fn main() i32 { 0 }").expect("write main source");

    for reserved in ["root", "package"] {
        let output = Command::new(env!("CARGO_BIN_EXE_nia"))
            .arg("check")
            .arg(&main)
            .arg("-M")
            .arg(format!("{reserved}={}", main.display()))
            .output_timeout("run nia check with reserved module map");

        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!("`{reserved}` is a compiler-reserved module root")),
            "{stderr}"
        );
    }
}

#[test]
fn check_uses_default_std_module_map() {
    let root = temp_dir("check_uses_default_std_module_map");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
using std;

fn main() i32 {
    0
}
"#,
    )
    .expect("write main source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("check")
        .arg(&main)
        .output_timeout("run nia check with default std");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn check_exe_uses_freestanding_startup_contract() {
    let root = temp_dir("check_exe_uses_freestanding_startup_contract");
    let private_main = root.join("private_main.nia");
    std::fs::write(
        &private_main,
        r#"
using std::process;

fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
    )
    .expect("write private entry source");

    let ordinary_check = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("check")
        .arg(&private_main)
        .output_timeout("run ordinary nia check");

    assert!(
        ordinary_check.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&ordinary_check.stderr)
    );

    let exe_check = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("check")
        .arg("--exe")
        .arg(&private_main)
        .output_timeout("run nia check --exe");

    assert!(!exe_check.status.success());
    let stderr = String::from_utf8_lossy(&exe_check.stderr);
    assert!(stderr.contains("private"), "{stderr}");
    assert!(stderr.contains("root::main"), "{stderr}");

    let public_main = root.join("public_main.nia");
    std::fs::write(
        &public_main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
    )
    .expect("write public entry source");

    let exe_check = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("check")
        .arg("--exe")
        .arg(&public_main)
        .output_timeout("run nia check --exe with public entry");

    assert!(
        exe_check.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&exe_check.stderr)
    );

    let runtime_check = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("check")
        .arg(&public_main)
        .arg("--runtime")
        .arg("freestanding")
        .output_timeout("run nia check --runtime freestanding");

    assert!(
        runtime_check.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&runtime_check.stderr)
    );

    let repeated_runtime_check = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("check")
        .arg(&public_main)
        .arg("--runtime")
        .arg("freestanding")
        .arg("--runtime=bare")
        .output_timeout("run nia check with repeated runtime");

    assert!(
        repeated_runtime_check.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&repeated_runtime_check.stderr)
    );

    let conflict = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("check")
        .arg("--exe")
        .arg("--runtime=bare")
        .arg(&public_main)
        .output_timeout("run nia check --exe --runtime=bare");

    assert!(!conflict.status.success());
    let stderr = String::from_utf8_lossy(&conflict.stderr);
    assert!(
        stderr.contains("`--runtime bare` cannot be combined with `--exe`"),
        "{stderr}"
    );
}

#[test]
fn check_can_emit_backend_optimization_report() {
    let root = temp_dir("check_can_emit_backend_optimization_report");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
let zeroes: [4]i32 = [0; 4];

fn main() i32 {
    var unused = 1;
    zeroes[0]
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("-O2")
        .arg("check")
        .arg(&main)
        .arg("--opt-report")
        .output_timeout("run nia check --opt-report");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("backend optimization report:"), "{stdout}");
    assert!(stdout.contains("policy level=O2"), "{stdout}");
    assert!(stdout.contains("inline=normal"), "{stdout}");
    assert!(stdout.contains("specialize=normal"), "{stdout}");
    assert!(
        stdout.contains("dedup_monomorphized_instances=true"),
        "{stdout}"
    );
    assert!(stdout.contains("prefer_size=false"), "{stdout}");
    assert!(stdout.contains("llvm_codegen=default"), "{stdout}");
    assert!(stdout.contains("llvm_size=default"), "{stdout}");
    assert!(stdout.contains("enabled_module_passes="), "{stdout}");
    assert!(stdout.contains("inline-leaf-functions"), "{stdout}");
    assert!(stdout.contains("remove-unused-functions"), "{stdout}");
    assert!(stdout.contains("enabled_function_passes="), "{stdout}");
    assert!(
        stdout.contains("enabled_global_passes=simplify-static-init"),
        "{stdout}"
    );
    assert!(stdout.contains("changes="), "{stdout}");
    assert!(!stdout.contains("changes=0"), "{stdout}");
    assert!(stdout.contains("remove-unused-local-bindings"), "{stdout}");
    assert!(stdout.contains("global simplify-static-init"), "{stdout}");

    let o0 = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("-O0")
        .arg("check")
        .arg(&main)
        .arg("--opt-report")
        .output_timeout("run nia -O0 check --opt-report");

    assert!(
        o0.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&o0.stderr)
    );
    let stdout = String::from_utf8_lossy(&o0.stdout);
    assert!(stdout.contains("backend optimization report:"), "{stdout}");
    assert!(stdout.contains("policy level=O0"), "{stdout}");
    assert!(stdout.contains("inline=never"), "{stdout}");
    assert!(stdout.contains("llvm_codegen=none"), "{stdout}");
    assert!(stdout.contains("llvm_size=default"), "{stdout}");
    assert!(stdout.contains("enabled_module_passes=none"), "{stdout}");
    assert!(stdout.contains("enabled_function_passes=none"), "{stdout}");
    assert!(stdout.contains("enabled_global_passes=none"), "{stdout}");
    assert!(stdout.contains("changes=0"), "{stdout}");
    assert!(stdout.contains("no changes"), "{stdout}");

    let o3 = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("-O3")
        .arg("check")
        .arg(&main)
        .arg("--opt-report")
        .output_timeout("run nia -O3 check --opt-report");

    assert!(
        o3.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&o3.stderr)
    );
    let stdout = String::from_utf8_lossy(&o3.stdout);
    assert!(stdout.contains("backend optimization report:"), "{stdout}");
    assert!(stdout.contains("policy level=O3"), "{stdout}");
    assert!(stdout.contains("llvm_codegen=aggressive"), "{stdout}");
    assert!(stdout.contains("llvm_size=default"), "{stdout}");
    assert!(
        stdout.contains("devirtualize-direct-trait-calls"),
        "{stdout}"
    );
    assert!(
        stdout.contains("propagate-cross-function-constants"),
        "{stdout}"
    );

    let oz = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("-Oz")
        .arg("check")
        .arg(&main)
        .arg("--opt-report")
        .output_timeout("run nia -Oz check --opt-report");

    assert!(
        oz.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&oz.stderr)
    );
    let stdout = String::from_utf8_lossy(&oz.stdout);
    assert!(stdout.contains("policy level=Oz"), "{stdout}");
    assert!(
        stdout.contains("dedup_monomorphized_instances=true"),
        "{stdout}"
    );
    assert!(stdout.contains("prefer_size=true"), "{stdout}");
    assert!(stdout.contains("llvm_codegen=less"), "{stdout}");
    assert!(stdout.contains("llvm_size=tiny"), "{stdout}");

    let os = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("-Os")
        .arg("check")
        .arg(&main)
        .arg("--opt-report")
        .output_timeout("run nia -Os check --opt-report");

    assert!(
        os.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&os.stderr)
    );
    let stdout = String::from_utf8_lossy(&os.stdout);
    assert!(stdout.contains("policy level=Os"), "{stdout}");
    assert!(
        stdout.contains("dedup_monomorphized_instances=true"),
        "{stdout}"
    );
    assert!(stdout.contains("prefer_size=true"), "{stdout}");
    assert!(stdout.contains("llvm_codegen=default"), "{stdout}");
    assert!(stdout.contains("llvm_size=small"), "{stdout}");
}

#[test]
fn emit_backend_prints_backend_ir() {
    let root = temp_dir("emit_backend_prints_backend_ir");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    42
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--backend")
        .arg(&main)
        .output_timeout("run nia emit --backend");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("BackendProgram"), "{stdout}");
    assert!(stdout.contains("functions"), "{stdout}");
    assert!(stdout.contains("main"), "{stdout}");
}

#[test]
fn emit_backend_can_emit_optimization_report_to_stderr() {
    let root = temp_dir("emit_backend_can_emit_optimization_report_to_stderr");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn answer() i32 {
    42
}

fn main() i32 {
    answer()
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("-O1")
        .arg("emit")
        .arg("--backend")
        .arg(&main)
        .arg("--opt-report")
        .output_timeout("run nia emit --backend --opt-report");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(stdout.contains("BackendProgram"), "{stdout}");
    assert!(!stdout.contains("backend optimization report:"), "{stdout}");
    assert!(stderr.contains("backend optimization report:"), "{stderr}");
    assert!(stderr.contains("llvm_codegen=less"), "{stderr}");
    assert!(stderr.contains("llvm_size=default"), "{stderr}");
    assert!(stderr.contains("enabled_module_passes="), "{stderr}");
    assert!(stderr.contains("enabled_function_passes="), "{stderr}");
    assert!(stderr.contains("changes="), "{stderr}");
    assert!(stderr.contains("inline-leaf-functions"), "{stderr}");
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

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--llvm")
        .arg(&main)
        .output_timeout("run nia emit --llvm");

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
fn atomic_generic_builtin_rejects_non_atomic_instantiations_at_emit() {
    let root = temp_dir("atomic_generic_builtin_rejects_non_atomic_instantiations_at_emit");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Point {
    x: i32,
}

fn load[T](ptr: &T) T
where T: Sized
{
    @atomic_load[T](ptr, 1usize)
}

fn main() i32 {
    var point: Point = { x: 1 };
    _ = load[Point](&point);
    0
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--llvm")
        .arg(&main)
        .output_timeout("run nia emit --llvm invalid atomic");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unsupported atomic value type"), "{stderr}");
}

#[test]
fn emit_llvm_can_emit_backend_optimization_report_to_stderr() {
    let root = temp_dir("emit_llvm_can_emit_backend_optimization_report_to_stderr");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn answer() i32 {
    42
}

fn main() i32 {
    answer()
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("-O1")
        .arg("emit")
        .arg("--llvm")
        .arg(&main)
        .arg("--opt-report")
        .output_timeout("run nia emit --llvm --opt-report");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(stdout.contains("define i32 @"), "{stdout}");
    assert!(!stdout.contains("backend optimization report:"), "{stdout}");
    assert!(stderr.contains("backend optimization report:"), "{stderr}");
    assert!(stderr.contains("policy level=O1"), "{stderr}");
    assert!(stderr.contains("inline=small"), "{stderr}");
    assert!(stderr.contains("llvm_codegen=less"), "{stderr}");
    assert!(stderr.contains("llvm_size=default"), "{stderr}");
    assert!(stderr.contains("enabled_module_passes="), "{stderr}");
    assert!(stderr.contains("enabled_function_passes="), "{stderr}");
    assert!(stderr.contains("changes="), "{stderr}");
    assert!(stderr.contains("inline-leaf-functions"), "{stderr}");
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

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--obj")
        .arg(&main)
        .arg("-o")
        .arg(&object)
        .output_timeout("run nia emit --obj");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata = std::fs::metadata(&object).expect("object metadata");
    assert!(metadata.len() > 0);
}

#[cfg(all(unix, target_os = "linux", target_arch = "x86_64"))]
#[test]
fn emit_obj_defaults_to_bare_runtime_and_can_emit_freestanding_startup() {
    let root = temp_dir("emit_obj_defaults_to_bare_runtime_and_can_emit_freestanding_startup");
    let main = root.join("main.nia");
    let bare_dir = root.join("bare");
    let freestanding_dir = root.join("freestanding");
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
    )
    .expect("write test source");

    let bare = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--obj")
        .arg(&main)
        .arg("--out-dir")
        .arg(&bare_dir)
        .output_timeout("run nia emit --obj bare runtime");

    assert!(
        bare.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&bare.stderr)
    );
    assert!(
        !object_dir_defines_symbol(&bare_dir, "_start"),
        "bare object output unexpectedly defines _start"
    );

    let freestanding = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--obj")
        .arg(&main)
        .arg("--runtime=freestanding")
        .arg("--out-dir")
        .arg(&freestanding_dir)
        .output_timeout("run nia emit --obj --runtime=freestanding");

    assert!(
        freestanding.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&freestanding.stderr)
    );
    assert!(
        object_dir_defines_symbol(&freestanding_dir, "_start"),
        "freestanding object output did not define _start"
    );
}

#[cfg(unix)]
#[test]
fn emit_exe_passes_link_args_to_linker() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_dir("emit_exe_passes_link_args_to_linker");
    let main = root.join("main.nia");
    let executable = root.join("main");
    let linker = root.join("linker.sh");
    let args_log = root.join("linker.args");
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
    )
    .expect("write test source");
    std::fs::write(
        &linker,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            args_log.display()
        ),
    )
    .expect("write linker script");
    let mut permissions = std::fs::metadata(&linker)
        .expect("linker metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&linker, permissions).expect("make linker executable");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .env("NIA_LINKER", &linker)
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("--link-arg")
        .arg("-lc")
        .arg("--link-arg=-lm")
        .arg("--link-arg")
        .arg("-Olinker")
        .arg("-o")
        .arg(&executable)
        .output_timeout("run nia emit --exe --link-arg");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let args = std::fs::read_to_string(&args_log).expect("read linker args");
    assert!(args.lines().any(|arg| arg == "-lc"), "{args}");
    assert!(args.lines().any(|arg| arg == "-lm"), "{args}");
    assert!(args.lines().any(|arg| arg == "-Olinker"), "{args}");
    assert!(args.lines().any(|arg| arg == "-o"), "{args}");
    assert!(
        args.lines().any(|arg| arg == executable.to_string_lossy()),
        "{args}"
    );

    let bare_runtime = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("--runtime")
        .arg("bare")
        .arg("-o")
        .arg(root.join("bare-main"))
        .output_timeout("run nia emit --exe --runtime bare");

    assert!(!bare_runtime.status.success());
    let stderr = String::from_utf8_lossy(&bare_runtime.stderr);
    assert!(
        stderr.contains("`nia emit --exe` currently supports only `--runtime freestanding`"),
        "{stderr}"
    );
}

#[test]
fn emit_obj_accepts_each_optimization_level() {
    let root = temp_dir("emit_obj_accepts_each_optimization_level");
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

    for level in ["-O0", "-O1", "-O2", "-O3", "-Os", "-Oz", "-O"] {
        let object = root.join(format!("main_{}.o", level.trim_start_matches('-')));
        let output_context = format!("run nia {level} emit --obj");
        let output = Command::new(env!("CARGO_BIN_EXE_nia"))
            .arg(level)
            .arg("emit")
            .arg("--obj")
            .arg(&main)
            .arg("-o")
            .arg(&object)
            .output_timeout(&output_context);

        assert!(
            output.status.success(),
            "{level} stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let metadata = std::fs::metadata(&object)
            .unwrap_or_else(|error| panic!("object metadata for {level}: {error}"));
        assert!(metadata.len() > 0, "{level} produced an empty object");
    }
}

#[test]
fn emit_obj_preserves_output_paths_that_look_like_optimization_flags() {
    let root = temp_dir("emit_obj_preserves_output_paths_that_look_like_optimization_flags");
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

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .current_dir(&root)
        .arg("emit")
        .arg("--obj")
        .arg(&main)
        .arg("-o")
        .arg("-Oartifact.o")
        .output_timeout("run nia emit --obj -o -Oartifact.o");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata = std::fs::metadata(root.join("-Oartifact.o")).expect("object metadata");
    assert!(metadata.len() > 0);

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .current_dir(&root)
        .arg("emit")
        .arg("--obj")
        .arg(&main)
        .arg("-o")
        .arg("--opt-report")
        .output_timeout("run nia emit --obj -o --opt-report");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("backend optimization report:"), "{stdout}");
    assert!(!stderr.contains("backend optimization report:"), "{stderr}");
    let metadata = std::fs::metadata(root.join("--opt-report")).expect("object metadata");
    assert!(metadata.len() > 0);

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .current_dir(&root)
        .arg("emit")
        .arg("--obj")
        .arg(&main)
        .arg("--out-dir")
        .arg("-Oobjects")
        .output_timeout("run nia emit --obj --out-dir -Oobjects");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let object_count = std::fs::read_dir(root.join("-Oobjects"))
        .expect("read object dir")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "o"))
        .count();
    assert_eq!(object_count, 1);
}

#[test]
fn emit_obj_can_emit_optimization_report_to_stderr() {
    let root = temp_dir("emit_obj_can_emit_optimization_report_to_stderr");
    let main = root.join("main.nia");
    let object = root.join("main.o");
    let object_before_source = root.join("main_before_source.o");
    let object_before_output_flag = root.join("main_before_output_flag.o");
    std::fs::write(
        &main,
        r#"
fn answer() i32 {
    42
}

fn main() i32 {
    answer()
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("-Os")
        .arg("emit")
        .arg("--obj")
        .arg(&main)
        .arg("-o")
        .arg(&object)
        .arg("--opt-report")
        .output_timeout("run nia emit --obj --opt-report");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!stdout.contains("backend optimization report:"), "{stdout}");
    assert!(stderr.contains("backend optimization report:"), "{stderr}");
    assert!(stderr.contains("policy level=Os"), "{stderr}");
    assert!(stderr.contains("llvm_codegen=default"), "{stderr}");
    assert!(stderr.contains("llvm_size=small"), "{stderr}");
    let metadata = std::fs::metadata(&object).expect("object metadata");
    assert!(metadata.len() > 0);

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("-Os")
        .arg("emit")
        .arg("--obj")
        .arg("--opt-report")
        .arg(&main)
        .arg("-o")
        .arg(&object_before_source)
        .output_timeout("run nia emit --obj --opt-report before source");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("backend optimization report:"), "{stderr}");
    let metadata = std::fs::metadata(&object_before_source).expect("object metadata before source");
    assert!(metadata.len() > 0);

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("-Os")
        .arg("emit")
        .arg("--obj")
        .arg(&main)
        .arg("--opt-report")
        .arg("-o")
        .arg(&object_before_output_flag)
        .output_timeout("run nia emit --obj --opt-report before -o");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("backend optimization report:"), "{stderr}");
    let metadata =
        std::fs::metadata(&object_before_output_flag).expect("object metadata before output flag");
    assert!(metadata.len() > 0);
}

#[cfg(all(unix, target_os = "linux", target_arch = "x86_64"))]
fn object_dir_defines_symbol(dir: &std::path::Path, symbol: &str) -> bool {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("read object dir {}: {error}", dir.display()));
    for entry in entries {
        let entry = entry.expect("read object entry");
        let path = entry.path();
        if !path.extension().is_some_and(|extension| extension == "o") {
            continue;
        }
        let output = Command::new("nm")
            .arg("--defined-only")
            .arg(&path)
            .output_timeout("run nm on emitted object");
        assert!(
            output.status.success(),
            "nm stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout
            .lines()
            .any(|line| line.split_whitespace().last() == Some(symbol))
        {
            return true;
        }
    }
    false
}
