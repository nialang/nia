// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static TEMP_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[test]
fn help_and_version_use_nia_command_name() {
    let help = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("--help")
        .output()
        .expect("run nia --help");
    assert!(
        help.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&help.stderr)
    );
    let help_stdout = String::from_utf8_lossy(&help.stdout);
    assert!(help_stdout.contains("Usage:\n  nia"), "{help_stdout}");
    assert!(
        help_stdout.contains("emit <target> <file.nia>"),
        "{help_stdout}"
    );
    assert!(
        help_stdout.contains("-O, -O0, -O1, -O2, -O3, -Os, -Oz"),
        "{help_stdout}"
    );
    for level in ["-O0", "-O1", "-O2", "-O3", "-Os", "-Oz"] {
        assert!(help_stdout.contains(level), "{help_stdout}");
    }

    let check_help = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("help")
        .arg("check")
        .output()
        .expect("run nia help check");
    assert!(
        check_help.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&check_help.stderr)
    );
    let check_stdout = String::from_utf8_lossy(&check_help.stdout);
    assert!(check_stdout.contains("--opt-report"), "{check_stdout}");
    assert!(
        check_stdout.contains("-O, -O0, -O1, -O2, -O3, -Os, -Oz"),
        "{check_stdout}"
    );
    assert!(
        check_stdout.contains("optimization policy, enabled passes, change count, and changes"),
        "{check_stdout}"
    );

    let emit_help = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("help")
        .arg("emit")
        .output()
        .expect("run nia help emit");
    assert!(
        emit_help.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit_help.stderr)
    );
    let emit_stdout = String::from_utf8_lossy(&emit_help.stdout);
    assert!(emit_stdout.contains("backend <file.nia>"), "{emit_stdout}");

    let emit_backend_help = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("help")
        .arg("emit")
        .arg("backend")
        .output()
        .expect("run nia help emit backend");
    assert!(
        emit_backend_help.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit_backend_help.stderr)
    );
    let emit_backend_stdout = String::from_utf8_lossy(&emit_backend_help.stdout);
    assert!(
        emit_backend_stdout.contains("nia emit backend <file.nia>"),
        "{emit_backend_stdout}"
    );
    assert!(
        emit_backend_stdout.contains("--opt-report"),
        "{emit_backend_stdout}"
    );
    assert!(
        emit_backend_stdout
            .contains("optimization policy, enabled passes, change count, and changes to stderr"),
        "{emit_backend_stdout}"
    );

    let emit_llvm_help = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("help")
        .arg("emit")
        .arg("llvm")
        .output()
        .expect("run nia help emit llvm");
    assert!(
        emit_llvm_help.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit_llvm_help.stderr)
    );
    let emit_llvm_stdout = String::from_utf8_lossy(&emit_llvm_help.stdout);
    assert!(
        emit_llvm_stdout.contains("nia emit llvm <file.nia>"),
        "{emit_llvm_stdout}"
    );
    assert!(
        emit_llvm_stdout
            .contains("optimization policy, enabled passes, change count, and changes to stderr"),
        "{emit_llvm_stdout}"
    );

    let emit_obj_help = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("help")
        .arg("emit")
        .arg("obj")
        .output()
        .expect("run nia help emit obj");
    assert!(
        emit_obj_help.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit_obj_help.stderr)
    );
    let emit_obj_stdout = String::from_utf8_lossy(&emit_obj_help.stdout);
    assert!(
        emit_obj_stdout.contains("nia emit obj <file.nia>"),
        "{emit_obj_stdout}"
    );
    assert!(
        emit_obj_stdout.contains("--out-dir <dir>"),
        "{emit_obj_stdout}"
    );
    assert!(
        emit_obj_stdout.contains("--opt-report"),
        "{emit_obj_stdout}"
    );
    for level in ["-O0", "-O1", "-O2", "-O3", "-Os", "-Oz"] {
        assert!(emit_obj_stdout.contains(level), "{emit_obj_stdout}");
    }

    let emit_exe_help = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("help")
        .arg("emit")
        .arg("exe")
        .output()
        .expect("run nia help emit exe");
    assert!(
        emit_exe_help.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit_exe_help.stderr)
    );
    let emit_exe_stdout = String::from_utf8_lossy(&emit_exe_help.stdout);
    assert!(
        emit_exe_stdout.contains("nia emit exe <file.nia>"),
        "{emit_exe_stdout}"
    );
    assert!(
        emit_exe_stdout.contains("-O, -O0, -O1, -O2, -O3, -Os, -Oz"),
        "{emit_exe_stdout}"
    );
    assert!(
        emit_exe_stdout.contains("--opt-report"),
        "{emit_exe_stdout}"
    );

    let version = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("--version")
        .output()
        .expect("run nia --version");
    assert!(
        version.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&version.stderr)
    );
    let version_stdout = String::from_utf8_lossy(&version.stdout);
    assert!(version_stdout.starts_with("nia "), "{version_stdout}");
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
        .arg("llvm")
        .arg(&main)
        .output()
        .expect("run nia -O2 emit llvm");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("define i32 @"), "{stdout}");
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
        .output()
        .expect("run nia check main.nia -Oz --opt-report");

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
        .output()
        .expect("run nia -O check --opt-report");

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
        .output()
        .expect("run nia with invalid optimization option");

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
pub comptime let answer: i32 = 42;
"#,
    )
    .expect("write mapped source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("check")
        .arg(&main)
        .arg("-M")
        .arg(format!("share={}", mapped.display()))
        .output()
        .expect("run nia check with trailing -M");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn module_map_rejects_compiler_reserved_root() {
    let root = temp_dir("module_map_rejects_compiler_reserved_root");
    let main = root.join("main.nia");
    std::fs::write(&main, "fn main() i32 { 0 }").expect("write main source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("check")
        .arg(&main)
        .arg("-M")
        .arg(format!("root={}", main.display()))
        .output()
        .expect("run nia check with reserved root module map");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`root` is a compiler-reserved module root"),
        "{stderr}"
    );
}

#[test]
fn check_uses_default_std_module_map() {
    let root = temp_dir("check_uses_default_std_module_map");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
import std;

fn main() i32 {
    0
}
"#,
    )
    .expect("write main source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("check")
        .arg(&main)
        .output()
        .expect("run nia check with default std");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
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
        .output()
        .expect("run nia check --opt-report");

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
        .output()
        .expect("run nia -O0 check --opt-report");

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
        .output()
        .expect("run nia -O3 check --opt-report");

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
        .output()
        .expect("run nia -Oz check --opt-report");

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
        .output()
        .expect("run nia -Os check --opt-report");

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
        .arg("backend")
        .arg(&main)
        .output()
        .expect("run nia emit backend");

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
        .arg("backend")
        .arg(&main)
        .arg("--opt-report")
        .output()
        .expect("run nia emit backend --opt-report");

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
        .arg("llvm")
        .arg(&main)
        .output()
        .expect("run nia emit llvm");

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
        .arg("llvm")
        .arg(&main)
        .arg("--opt-report")
        .output()
        .expect("run nia emit llvm --opt-report");

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
        .arg("obj")
        .arg(&main)
        .arg("-o")
        .arg(&object)
        .output()
        .expect("run nia emit obj");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata = std::fs::metadata(&object).expect("object metadata");
    assert!(metadata.len() > 0);
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
        let output = Command::new(env!("CARGO_BIN_EXE_nia"))
            .arg(level)
            .arg("emit")
            .arg("obj")
            .arg(&main)
            .arg("-o")
            .arg(&object)
            .output()
            .unwrap_or_else(|error| panic!("run nia {level} emit obj: {error}"));

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
        .arg("obj")
        .arg(&main)
        .arg("-o")
        .arg("-Oartifact.o")
        .output()
        .expect("run nia emit obj -o -Oartifact.o");

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
        .arg("obj")
        .arg(&main)
        .arg("-o")
        .arg("--opt-report")
        .output()
        .expect("run nia emit obj -o --opt-report");

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
        .arg("obj")
        .arg(&main)
        .arg("--out-dir")
        .arg("-Oobjects")
        .output()
        .expect("run nia emit obj --out-dir -Oobjects");

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
        .arg("obj")
        .arg(&main)
        .arg("-o")
        .arg(&object)
        .arg("--opt-report")
        .output()
        .expect("run nia emit obj --opt-report");

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
        .arg("obj")
        .arg("--opt-report")
        .arg(&main)
        .arg("-o")
        .arg(&object_before_source)
        .output()
        .expect("run nia emit obj --opt-report before source");

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
        .arg("obj")
        .arg(&main)
        .arg("--opt-report")
        .arg("-o")
        .arg(&object_before_output_flag)
        .output()
        .expect("run nia emit obj --opt-report before -o");

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

#[test]
fn emit_exe_links_freestanding_executable() {
    let root = temp_dir("emit_exe_links_freestanding_executable");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    process::ExitCode::init(7)!
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(7));
}

#[test]
fn emit_exe_can_use_std_root_facade_modules() {
    let root = temp_dir("emit_exe_can_use_std_root_facade_modules");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std;

pub fn main(init: std::process::Init) std::process::ExitCode!void {
    _ = init;
    var writer = std::io::DiscardingWriter::init();
    switch writer.write_all(b"nia") {
        !ok => _ = ok,
        error! => return std::process::ExitCode::init(1)!,
    }
    if writer.len() != 3 {
        return std::process::ExitCode::init(2)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_exposes_process_args_without_raw_argv() {
    let root = temp_dir("emit_exe_exposes_process_args_without_raw_argv");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std;

pub fn main(init: std::process::Init) std::process::ExitCode!void {
    var args = init.args();
    if args.len() != 3 {
        return std::process::ExitCode::init(1)!;
    }
    var first_arg = switch args.get(1) {
        ?value => value,
        null => return std::process::ExitCode::init(2)!,
    };
    var second_arg = switch args.get(2) {
        ?value => value,
        null => return std::process::ExitCode::init(3)!,
    };
    var first = first_arg.raw_bytes();
    var second = second_arg.raw_bytes();
    if first.len() != 3 {
        return std::process::ExitCode::init(4)!;
    }
    if first[0] != 110u8 or first[1] != 105u8 or first[2] != 97u8 {
        return std::process::ExitCode::init(5)!;
    }
    if second.len() != 4 {
        return std::process::ExitCode::init(6)!;
    }
    switch args.get(3) {
        ?value => {
            _ = value;
            return std::process::ExitCode::init(7)!;
        },
        null => {},
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .arg("nia")
        .arg("lang")
        .status()
        .expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_exposes_process_env_as_values() {
    let root = temp_dir("emit_exe_exposes_process_env_as_values");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std;

fn starts_with_needle(bytes: &[u8]) bool {
    var needle = b"NIA_TEST_ENV=ok";
    if bytes.len() < needle.len() {
        return false;
    }
    var index = 0usize;
    while index < needle.len() {
        if bytes[index] != needle[index] {
            return false;
        }
        index += 1usize;
    }
    true
}

pub fn main(init: std::process::Init) std::process::ExitCode!void {
    var env = init.env();
    var index = 0usize;
    while index < env.len() {
        var item = switch env.get(index) {
            ?value => value,
            null => return std::process::ExitCode::init(1)!,
        };
        if starts_with_needle(item.raw_bytes()) {
            return !{};
        }
        index += 1usize;
    }
    return std::process::ExitCode::init(2)!;
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .env("NIA_TEST_ENV", "ok")
        .status()
        .expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_can_map_error_unions_with_std_result() {
    let root = temp_dir("emit_exe_can_map_error_unions_with_std_result");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.process;
import std.result;

enum ParseError: i32 {
    Bad = 1,
    _
}

enum AppError: i32 {
    InvalidInput = 7,
    _
}

fn map_parse_error(error: ParseError) AppError {
    _ = error;
    AppError::InvalidInput
}

fn parse() ParseError!i32 {
    ParseError::Bad!
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    switch parse().map_err[AppError](&map_parse_error) {
        !value => return process::ExitCode::init(value)!,
        err! => return process::ExitCode::init(err as i32)!,
    }
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(7));
}

#[test]
fn emit_exe_can_write_stdout_through_std_fs() {
    let root = temp_dir("emit_exe_can_write_stdout_through_std_fs");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.fs;
import std.io;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var stdout = fs::File::stdout();
    switch stdout.write_all(b"nia\n") {
        !ok => _ = ok,
        error! => return process::ExitCode::init(1)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).output().expect("run emitted executable");
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&run.stdout), "nia\n");
}

#[test]
fn emit_exe_can_format_to_stdout() {
    let root = temp_dir("emit_exe_can_format_to_stdout");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std;

pub fn main(init: std::process::Init) std::process::ExitCode!void {
    _ = init;
    var stdout = std::fs::File::stdout();
    switch stdout.print("A¢€😀, {}\n", [&'λ']) {
        !ok => _ = ok,
        error! => return std::process::ExitCode::init(1)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).output().expect("run emitted executable");
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&run.stdout), "A¢€😀, λ\n");
}

#[test]
fn emit_exe_can_use_std_io_fixed_buffers() {
    let root = temp_dir("emit_exe_can_use_std_io_fixed_buffers");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std;

pub fn main(init: std::process::Init) std::process::ExitCode!void {
    _ = init;
    var storage: [8]u8 = [0, 0, 0, 0, 0, 0, 0, 0];
    var writer = std::io::FixedBufferWriter::init(&mut storage[..]);
    switch writer.print("nia {}", [&7]) {
        !ok => _ = ok,
        error! => return std::process::ExitCode::init(1)!,
    }
    if writer.len() != 5 {
        return std::process::ExitCode::init(2)!;
    }

    var copied: [5]u8 = [0, 0, 0, 0, 0];
    var reader = std::io::FixedBufferReader::init(writer.written());
    switch reader.read_exact(&mut copied[..]) {
        !ok => _ = ok,
        error! => return std::process::ExitCode::init(3)!,
    }
    var expected = b"nia 7";
    if copied[0] != expected[0] or copied[1] != expected[1] or copied[2] != expected[2] or copied[3] != expected[3] or copied[4] != expected[4] {
        return std::process::ExitCode::init(4)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_can_use_std_io_discarding_writer_and_limited_reader() {
    let root = temp_dir("emit_exe_can_use_std_io_discarding_writer_and_limited_reader");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.io;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var discard = io::DiscardingWriter::init();
    switch discard.write_all(b"abcdef") {
        !ok => _ = ok,
        error! => return process::ExitCode::init(1)!,
    }
    if discard.len() != 6 {
        return process::ExitCode::init(2)!;
    }

    var source = io::FixedBufferReader::init(b"abcdef");
    var limited = io::LimitedReader[io::FixedBufferReader]::init(
        &mut source,
        io::Limit::limited(3),
    );
    var copied: [4]u8 = [0, 0, 0, 0];
    var n: usize;
    switch limited.read(&mut copied[..]) {
        !value => n = value,
        error! => return process::ExitCode::init(3)!,
    }
    if n != 3 {
        return process::ExitCode::init(4)!;
    }
    if copied[0] != b'a' or copied[1] != b'b' or copied[2] != b'c' {
        return process::ExitCode::init(5)!;
    }
    switch limited.read(&mut copied[..]) {
        !value => n = value,
        error! => return process::ExitCode::init(6)!,
    }
    if n != 0 {
        return process::ExitCode::init(7)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_can_use_std_io_buffered_writer() {
    let root = temp_dir("emit_exe_can_use_std_io_buffered_writer");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.io;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var storage: [16]u8 = [0; 16];
    var backing = io::FixedBufferWriter::init(&mut storage[..]);
    var buffer_storage: [4]u8 = [0; 4];
    var writer = io::BufferedWriter[io::FixedBufferWriter]::init(
        &mut backing,
        &mut buffer_storage[..],
    );

    switch writer.write_all(b"abc") {
        !ok => _ = ok,
        error! => return process::ExitCode::init(1)!,
    }
    if writer.len() != 3 or backing.len() != 0 {
        return process::ExitCode::init(2)!;
    }

    switch writer.write_byte(b'd') {
        !ok => _ = ok,
        error! => return process::ExitCode::init(3)!,
    }
    if writer.len() != 4 or backing.len() != 0 {
        return process::ExitCode::init(4)!;
    }

    switch writer.write_all(b"efghij") {
        !ok => _ = ok,
        error! => return process::ExitCode::init(5)!,
    }
    if writer.len() != 0 or backing.len() != 10 {
        return process::ExitCode::init(6)!;
    }

    switch writer.flush() {
        !ok => _ = ok,
        error! => return process::ExitCode::init(7)!,
    }
    if backing.len() != 10 {
        return process::ExitCode::init(8)!;
    }

    var expected = b"abcdefghij";
    var written = backing.written();
    var index = 0usize;
    while index < written.len() {
        if written[index] != expected[index] {
            return process::ExitCode::init(9)!;
        }
        index += 1usize;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_can_use_std_io_buffered_reader() {
    let root = temp_dir("emit_exe_can_use_std_io_buffered_reader");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.io;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var source = io::FixedBufferReader::init(b"abcdefghij");
    var buffer_storage: [4]u8 = [0; 4];
    var reader = io::BufferedReader[io::FixedBufferReader]::init(
        &mut source,
        &mut buffer_storage[..],
    );

    var first: [2]u8 = [0; 2];
    var n: usize;
    switch reader.read(&mut first[..]) {
        !value => n = value,
        error! => return process::ExitCode::init(1)!,
    }
    if n != 2 or first[0] != b'a' or first[1] != b'b' {
        return process::ExitCode::init(2)!;
    }
    if reader.len() != 2 {
        return process::ExitCode::init(3)!;
    }

    var second: [3]u8 = [0; 3];
    switch reader.read(&mut second[..]) {
        !value => n = value,
        error! => return process::ExitCode::init(4)!,
    }
    if n != 2 or second[0] != b'c' or second[1] != b'd' {
        return process::ExitCode::init(5)!;
    }
    if reader.len() != 0 {
        return process::ExitCode::init(6)!;
    }

    var third: [5]u8 = [0; 5];
    switch reader.read(&mut third[..]) {
        !value => n = value,
        error! => return process::ExitCode::init(7)!,
    }
    if n != 5 {
        return process::ExitCode::init(8)!;
    }
    if third[0] != b'e' or third[1] != b'f' or third[2] != b'g' or third[3] != b'h' or third[4] != b'i' {
        return process::ExitCode::init(9)!;
    }

    var fourth: [2]u8 = [0; 2];
    switch reader.read(&mut fourth[..]) {
        !value => n = value,
        error! => return process::ExitCode::init(10)!,
    }
    if n != 1 or fourth[0] != b'j' {
        return process::ExitCode::init(11)!;
    }

    switch reader.read(&mut fourth[..]) {
        !value => n = value,
        error! => return process::ExitCode::init(12)!,
    }
    if n != 0 {
        return process::ExitCode::init(13)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_can_create_open_read_and_write_std_fs_files() {
    let root = temp_dir("emit_exe_can_create_open_read_and_write_std_fs_files");
    let data_path = root.join("data.txt");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.fs;
import std.io;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var path = fs::Path::init("data.txt");
    var cwd = fs::Dir::cwd();
    var file: fs::File;
    switch cwd.create_file(path, fs::CreateOptions::read_write()) {
        !value => file = value,
        error! => return process::ExitCode::init(1)!,
    }
    switch file.write_all(b"nia fs") {
        !ok => _ = ok,
        error! => return process::ExitCode::init(2)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return process::ExitCode::init(3)!,
    }

    var opened: fs::File;
    switch cwd.open_file(path, fs::OpenOptions::read_only()) {
        !value => opened = value,
        error! => return process::ExitCode::init(4)!,
    }
    var bytes: [6]u8 = [0, 0, 0, 0, 0, 0];
    switch opened.read_exact(&mut bytes[..]) {
        !ok => _ = ok,
        error! => return process::ExitCode::init(5)!,
    }
    switch opened.close() {
        !ok => _ = ok,
        error! => return process::ExitCode::init(6)!,
    }
    var expected = b"nia fs";
    var index = 0usize;
    while index < bytes.len() {
        if bytes[index] != expected[index] {
            return process::ExitCode::init(7)!;
        }
        index += 1usize;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status()
        .expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        std::fs::read(&data_path).expect("read data file"),
        b"nia fs"
    );
}

#[test]
fn emit_exe_can_open_std_fs_paths_from_text() {
    let root = temp_dir("emit_exe_can_open_std_fs_paths_from_text");
    let data_path = root.join("nia-λ.txt");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.fs;
import std.io;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var path = fs::Path::init("nia-λ.txt");
    var cwd = fs::Dir::cwd();
    var file: fs::File;
    switch cwd.create_file(path, fs::CreateOptions::init()) {
        !value => file = value,
        error! => return process::ExitCode::init(1)!,
    }
    switch file.write_all(b"ok") {
        !ok => _ = ok,
        error! => return process::ExitCode::init(2)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return process::ExitCode::init(3)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status()
        .expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
    assert_eq!(std::fs::read(&data_path).expect("read data file"), b"ok");
}

#[test]
fn emit_exe_std_fs_rejects_nul_in_text_path() {
    let root = temp_dir("emit_exe_std_fs_rejects_nul_in_text_path");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.fs;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var path = fs::Path::init("bad\0path");
    var cwd = fs::Dir::cwd();
    switch cwd.open_file(path, fs::OpenOptions::read_only()) {
        !file => {
            _ = file;
            return process::ExitCode::init(1)!;
        },
        err! => {
            if err == fs::Error::Invalid {
                !{}
            } else {
                return process::ExitCode::init(2)!;
            }
        },
    }
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status()
        .expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_mut_ref_receiver_updates_original_aggregate() {
    let root = temp_dir("emit_exe_mut_ref_receiver_updates_original_aggregate");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.process;

struct Counter {
    value: i32,
}

extend Counter {
    fn init() Counter {
        { value: 0 }
    }

    fn add(&mut self, amount: i32) void {
        self.value += amount;
    }

    fn get(&self) i32 {
        self.value
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var counter = Counter::init();
    counter.add(7);
    if counter.get() != 7 {
        return process::ExitCode::init(1)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_can_allocate_with_std_mem_page_allocator() {
    let root = temp_dir("emit_exe_can_allocate_with_std_mem_page_allocator");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    var layout: mem::Layout;
    switch mem::Layout::of[u8]() {
        !value => layout = value,
        error! => return process::ExitCode::init(5)!,
    }
    switch allocator.alloc_bytes(4096, layout.align()) {
        !block => {
            var ptr = block.ptr();
            ptr.* = 42u8;
            if ptr.* != 42u8 {
                return process::ExitCode::init(2)!;
            }
            switch allocator.free(block) {
                !ok => _ = ok,
                error! => return process::ExitCode::init(3)!,
            }
        },
        error! => return process::ExitCode::init(1)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_page_allocator_supports_overaligned_layouts() {
    let root = temp_dir("emit_exe_std_mem_page_allocator_supports_overaligned_layouts");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    var layout: mem::Layout;
    switch mem::Layout::init(64, 8192) {
        !value => layout = value,
        error! => return process::ExitCode::init(1)!,
    }
    var block: mem::Block;
    switch allocator.alloc(layout) {
        !value => block = value,
        error! => return process::ExitCode::init(2)!,
    }
    if block.ptr() as usize % 8192usize != 0usize {
        return process::ExitCode::init(3)!;
    }
    var bytes = block.bytes();
    bytes[0] = 17u8;
    bytes[63] = 23u8;
    if bytes[0] != 17u8 or bytes[63] != 23u8 {
        return process::ExitCode::init(4)!;
    }
    switch allocator.free(block) {
        !ok => _ = ok,
        error! => return process::ExitCode::init(5)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_layout_rejects_invalid_alignment() {
    let root = temp_dir("emit_exe_std_mem_layout_rejects_invalid_alignment");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    switch mem::Layout::init(16, 3) {
        !ok => {
            _ = ok;
            return process::ExitCode::init(1)!;
        },
        err! => {
            if err as i32 != mem::Error::InvalidAlignment as i32 {
                return process::ExitCode::init(2)!;
            }
        },
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_layout_rejects_array_size_overflow() {
    let root = temp_dir("emit_exe_std_mem_layout_rejects_array_size_overflow");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    switch mem::Layout::array[i32](4611686018427387904usize) {
        !ok => {
            _ = ok;
            return process::ExitCode::init(1)!;
        },
        err! => {
            if err as i32 != mem::Error::OutOfMemory as i32 {
                return process::ExitCode::init(2)!;
            }
        },
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_allocator_can_allocate_typed_slices() {
    let root = temp_dir("emit_exe_std_mem_allocator_can_allocate_typed_slices");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    switch allocator.alloc_slice[i32](4) {
        !items => {
            items[0] = 10;
            items[1] = 20;
            items[2] = 30;
            items[3] = 40;
            if items.len() != 4 {
                return process::ExitCode::init(2)!;
            }
            if items[0] + items[1] + items[2] + items[3] != 100 {
                return process::ExitCode::init(3)!;
            }
            switch allocator.free_slice[i32](items) {
                !ok => _ = ok,
                error! => return process::ExitCode::init(4)!,
            }
        },
        error! => return process::ExitCode::init(1)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_allocator_realloc_preserves_byte_prefix() {
    let root = temp_dir("emit_exe_std_mem_allocator_realloc_preserves_byte_prefix");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    var block: mem::Block;
    switch allocator.alloc_bytes(4, 1) {
        !value => block = value,
        error! => return process::ExitCode::init(1)!,
    }
    var bytes = block.bytes();
    bytes[0] = 10u8;
    bytes[1] = 20u8;
    bytes[2] = 30u8;
    bytes[3] = 40u8;

    var grow_layout: mem::Layout;
    switch mem::Layout::init(8, 1) {
        !value => grow_layout = value,
        error! => return process::ExitCode::init(2)!,
    }
    switch allocator.realloc(block, grow_layout) {
        !value => block = value,
        error! => return process::ExitCode::init(3)!,
    }
    if block.size() != 8 {
        return process::ExitCode::init(4)!;
    }
    bytes = block.bytes();
    if bytes[0] != 10u8 or bytes[1] != 20u8 or bytes[2] != 30u8 or bytes[3] != 40u8 {
        return process::ExitCode::init(5)!;
    }
    bytes[4] = 50u8;
    bytes[5] = 60u8;

    var shrink_layout: mem::Layout;
    switch mem::Layout::init(2, 1) {
        !value => shrink_layout = value,
        error! => return process::ExitCode::init(6)!,
    }
    switch allocator.realloc(block, shrink_layout) {
        !value => block = value,
        error! => return process::ExitCode::init(7)!,
    }
    if block.size() != 2 {
        return process::ExitCode::init(8)!;
    }
    bytes = block.bytes();
    if bytes[0] != 10u8 or bytes[1] != 20u8 {
        return process::ExitCode::init(9)!;
    }

    switch allocator.free(block) {
        !ok => _ = ok,
        error! => return process::ExitCode::init(10)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_allocator_resize_and_remap_have_precise_semantics() {
    let root = temp_dir("emit_exe_std_mem_allocator_resize_and_remap_have_precise_semantics");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    var layout: mem::Layout;
    switch mem::Layout::init(16, 8) {
        !value => layout = value,
        error! => return process::ExitCode::init(1)!,
    }
    var block: mem::Block;
    switch allocator.alloc(layout) {
        !value => block = value,
        error! => return process::ExitCode::init(2)!,
    }
    if not allocator.resize(block, layout) {
        return process::ExitCode::init(3)!;
    }

    var larger: mem::Layout;
    switch mem::Layout::init(32, 8) {
        !value => larger = value,
        error! => return process::ExitCode::init(4)!,
    }
    if not allocator.resize(block, larger) {
        return process::ExitCode::init(5)!;
    }
    switch allocator.remap(block, larger) {
        ?same => {
            if same.ptr() as usize != block.ptr() as usize or same.size() != 32 {
                return process::ExitCode::init(6)!;
            }
            block = same;
        },
        null => return process::ExitCode::init(7)!,
    }
    switch allocator.remap(block, layout) {
        ?same => {
            if same.ptr() as usize != block.ptr() as usize or same.size() != 16 {
                return process::ExitCode::init(8)!;
            }
            block = same;
        },
        null => return process::ExitCode::init(9)!,
    }

    var next_page: mem::Layout;
    switch mem::Layout::init(8192, 8) {
        !value => next_page = value,
        error! => return process::ExitCode::init(10)!,
    }
    if allocator.resize(block, next_page) {
        return process::ExitCode::init(11)!;
    }
    switch allocator.remap(block, next_page) {
        ?moved => {
            _ = moved;
            return process::ExitCode::init(12)!;
        },
        null => {},
    }
    switch allocator.free(block) {
        !ok => _ = ok,
        error! => return process::ExitCode::init(13)!,
    }

    var empty_a: mem::Layout;
    switch mem::Layout::init(0, 8) {
        !value => empty_a = value,
        error! => return process::ExitCode::init(14)!,
    }
    switch allocator.alloc(empty_a) {
        !value => block = value,
        error! => return process::ExitCode::init(15)!,
    }
    var empty_b: mem::Layout;
    switch mem::Layout::init(0, 16) {
        !value => empty_b = value,
        error! => return process::ExitCode::init(16)!,
    }
    if allocator.resize(block, empty_b) {
        return process::ExitCode::init(17)!;
    }
    switch allocator.remap(block, empty_b) {
        ?moved => {
            if moved.size() != 0 or moved.align() != 16 {
                return process::ExitCode::init(18)!;
            }
        },
        null => return process::ExitCode::init(19)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_allocator_realloc_from_empty_block() {
    let root = temp_dir("emit_exe_std_mem_allocator_realloc_from_empty_block");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    var empty_layout: mem::Layout;
    switch mem::Layout::init(0, 8) {
        !value => empty_layout = value,
        error! => return process::ExitCode::init(1)!,
    }
    var block: mem::Block;
    switch allocator.alloc(empty_layout) {
        !value => block = value,
        error! => return process::ExitCode::init(2)!,
    }
    if block.size() != 0 {
        return process::ExitCode::init(3)!;
    }

    var full_layout: mem::Layout;
    switch mem::Layout::init(16, 8) {
        !value => full_layout = value,
        error! => return process::ExitCode::init(4)!,
    }
    switch allocator.realloc(block, full_layout) {
        !value => block = value,
        error! => return process::ExitCode::init(5)!,
    }
    if block.size() != 16 or block.align() != 8 {
        return process::ExitCode::init(6)!;
    }
    var bytes = block.bytes();
    bytes[0] = 77u8;
    bytes[15] = 99u8;
    if bytes[0] != 77u8 or bytes[15] != 99u8 {
        return process::ExitCode::init(7)!;
    }

    switch allocator.free(block) {
        !ok => _ = ok,
        error! => return process::ExitCode::init(8)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_allocator_preserves_empty_slice_len() {
    let root = temp_dir("emit_exe_std_mem_allocator_preserves_empty_slice_len");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    switch allocator.alloc_slice[i32](0) {
        !items => {
            if items.len() != 0 {
                return process::ExitCode::init(2)!;
            }
            switch allocator.free_slice[i32](items) {
                !ok => _ = ok,
                error! => return process::ExitCode::init(3)!,
            }
        },
        error! => return process::ExitCode::init(1)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_allocator_preserves_zero_sized_slice_len() {
    let root = temp_dir("emit_exe_std_mem_allocator_preserves_zero_sized_slice_len");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    switch allocator.alloc_slice[void](4) {
        !items => {
            if items.len() != 4 {
                return process::ExitCode::init(2)!;
            }
            switch allocator.free_slice[void](items) {
                !ok => _ = ok,
                error! => return process::ExitCode::init(3)!,
            }
        },
        error! => return process::ExitCode::init(1)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_copy_forwards_and_backwards() {
    let root = temp_dir("emit_exe_std_mem_copy_forwards_and_backwards");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var left: [5]i32 = [1, 2, 3, 4, 5];
    mem::copy_forwards[i32](&mut left[0..3], &left[1..4]);
    let expected_left: [5]i32 = [2, 3, 4, 4, 5];
    if not mem::equal[i32](&left[..], &expected_left[..]) {
        return process::ExitCode::init(1)!;
    }

    var right: [5]i32 = [1, 2, 3, 4, 5];
    mem::copy_backwards[i32](&mut right[1..4], &right[0..3]);
    let expected_right: [5]i32 = [1, 1, 2, 3, 5];
    if not mem::equal[i32](&right[..], &expected_right[..]) {
        return process::ExitCode::init(2)!;
    }

    var exact_to: [3]u8 = [0, 0, 0];
    let exact_from: [3]u8 = [7, 8, 9];
    mem::copy_forwards[u8](&mut exact_to[..], &exact_from[..]);
    if not mem::equal[u8](&exact_to[..], &exact_from[..]) {
        return process::ExitCode::init(3)!;
    }

    let low: [2]u8 = [1, 2];
    let high: [2]u8 = [1, 3];
    if mem::order[u8](&low[..], &high[..]) != mem::Order::Less {
        return process::ExitCode::init(4)!;
    }
    if mem::order[u8](&high[..], &low[..]) != mem::Order::Greater {
        return process::ExitCode::init(5)!;
    }
    if mem::order[u8](&low[..], &low[..]) != mem::Order::Equal {
        return process::ExitCode::init(6)!;
    }
    let prefix: [1]u8 = [1];
    if mem::order[u8](&prefix[..], &low[..]) != mem::Order::Less {
        return process::ExitCode::init(7)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_memory_intrinsic_builtins() {
    let root = temp_dir("emit_exe_memory_intrinsic_builtins");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;

    var ints: [3]i32 = [0, 0, 0];
    let source_ints: [3]i32 = [7, 8, 9];
    @memcpy(&mut ints[..], &source_ints[..]);
    if ints[0] != 7 or ints[1] != 8 or ints[2] != 9 {
        return process::ExitCode::init(1)!;
    }

    var wide: [5]i32 = [0, 0, 0, 44, 55];
    let short: [3]i32 = [11, 22, 33];
    @memcpy(&mut wide[..], &short[..]);
    if wide[0] != 11 or wide[1] != 22 or wide[2] != 33 or wide[3] != 44 or wide[4] != 55 {
        return process::ExitCode::init(4)!;
    }

    var overlap: [5]u8 = [1, 2, 3, 4, 5];
    @memmove(&mut overlap[1..], &overlap[0..4]);
    if overlap[0] != 1 or overlap[1] != 1 or overlap[2] != 2 or overlap[3] != 3 or overlap[4] != 4 {
        return process::ExitCode::init(2)!;
    }

    var bytes: [4]u8 = [1, 2, 3, 4];
    @memset(&mut bytes[1..3], 9);
    if bytes[0] != 1 or bytes[1] != 9 or bytes[2] != 9 or bytes[3] != 4 {
        return process::ExitCode::init(3)!;
    }

    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_cross_module_generic_memory_intrinsic_keeps_param_locals() {
    let root = temp_dir("emit_exe_cross_module_generic_memory_intrinsic_keeps_param_locals");
    let main = root.join("main.nia");
    let helper = root.join("helper.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &helper,
        r#"
pub fn copy_prefix[T](to: &mut [T], from: &[T]) void
where T: Sized
{
    @memcpy(to, from);
}
"#,
    )
    .expect("write helper source");
    std::fs::write(
        &main,
        r#"
import helper;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var dest: [2]u8 = [0; 2];
    let source: [2]u8 = [b'a', b'b'];
    helper::copy_prefix[u8](&mut dest[..], &source[..]);
    if dest[0] != b'a' or dest[1] != b'b' {
        return process::ExitCode::init(1)!;
    }
    !{}
}
"#,
    )
    .expect("write main source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-M")
        .arg(format!("helper={}", helper.display()))
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_array_list_push_pop_and_deinit() {
    let root = temp_dir("emit_exe_std_array_list_push_pop_and_deinit");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.array_list;
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    let page = &mut allocator;
    var exact: array_list::ArrayList[i32];
    switch array_list::ArrayList[i32]::init_capacity(page, 3) {
        !value => exact = value,
        error! => return process::ExitCode::init(1)!,
    }
    if exact.len() != 0 or exact.capacity() != 3 {
        return process::ExitCode::init(2)!;
    }
    switch exact.deinit(page) {
        !ok => _ = ok,
        error! => return process::ExitCode::init(3)!,
    }

    var ops = array_list::ArrayList[i32]::init();
    switch ops.push(page, 1) {
        !ok => _ = ok,
        error! => return process::ExitCode::init(26)!,
    }
    switch ops.push(page, 3) {
        !ok => _ = ok,
        error! => return process::ExitCode::init(27)!,
    }
    switch ops.insert(page, 1, 2) {
        !ok => _ = ok,
        error! => return process::ExitCode::init(28)!,
    }
    let inserted_tail: [2]i32 = [4, 5];
    switch ops.insert_slice(page, 3, &inserted_tail[..]) {
        !ok => _ = ok,
        error! => return process::ExitCode::init(29)!,
    }
    let expected_ops: [5]i32 = [1, 2, 3, 4, 5];
    if not mem::equal[i32](ops.as_slice(), &expected_ops[..]) {
        return process::ExitCode::init(30)!;
    }
    switch ops.ordered_remove(1) {
        ?value => {
            if value != 2 {
                return process::ExitCode::init(31)!;
            }
        },
        null => return process::ExitCode::init(32)!,
    }
    let expected_ordered: [4]i32 = [1, 3, 4, 5];
    if not mem::equal[i32](ops.as_slice(), &expected_ordered[..]) {
        return process::ExitCode::init(33)!;
    }
    switch ops.swap_remove(0) {
        ?value => {
            if value != 1 {
                return process::ExitCode::init(34)!;
            }
        },
        null => return process::ExitCode::init(35)!,
    }
    let expected_swap: [3]i32 = [5, 3, 4];
    if not mem::equal[i32](ops.as_slice(), &expected_swap[..]) {
        return process::ExitCode::init(36)!;
    }
    switch ops.deinit(page) {
        !ok => _ = ok,
        error! => return process::ExitCode::init(37)!,
    }

    var list = array_list::ArrayList[i32]::init();
    if list.len() != 0 or not list.is_empty() {
        return process::ExitCode::init(4)!;
    }
    switch list.reserve_exact(page, 2) {
        !ok => _ = ok,
        error! => return process::ExitCode::init(5)!,
    }
    if list.capacity() != 2 {
        return process::ExitCode::init(6)!;
    }
    switch list.reserve(page, 3) {
        !ok => _ = ok,
        error! => return process::ExitCode::init(7)!,
    }
    if list.capacity() < 5 {
        return process::ExitCode::init(8)!;
    }
    var index = 0;
    while index < 6 {
        switch list.push(page, index * 10) {
            !ok => _ = ok,
            error! => return process::ExitCode::init(9)!,
        }
        index += 1;
    }
    if list.len() != 6 or list.capacity() < 6 {
        return process::ExitCode::init(10)!;
    }
    let items = list.as_slice();
    if items[0] != 0 or items[1] != 10 or items[5] != 50 {
        return process::ExitCode::init(11)!;
    }

    let more: [3]i32 = [60, 70, 80];
    switch list.append_slice(page, &more[..]) {
        !ok => _ = ok,
        error! => return process::ExitCode::init(12)!,
    }
    if list.len() != 9 or list.as_slice()[8] != 80 {
        return process::ExitCode::init(13)!;
    }

    switch list.add_one(page) {
        !slot => slot.* = 90,
        error! => return process::ExitCode::init(14)!,
    }
    if list.len() != 10 or list.as_slice()[9] != 90 {
        return process::ExitCode::init(15)!;
    }

    switch list.add_many_as_slice(page, 2) {
        !slots => {
            slots[0] = 100;
            slots[1] = 110;
        },
        error! => return process::ExitCode::init(16)!,
    }
    if list.len() != 12 or list.as_slice()[11] != 110 {
        return process::ExitCode::init(17)!;
    }

    let retained_capacity = list.capacity();
    list.shrink_retaining_capacity(10);
    if list.len() != 10 or list.capacity() != retained_capacity {
        return process::ExitCode::init(18)!;
    }

    let tail: [2]i32 = [100, 110];
    list.append_slice_assume_capacity(&tail[..]);
    if list.len() != 12 or list.as_slice()[10] != 100 or list.as_slice()[11] != 110 {
        return process::ExitCode::init(19)!;
    }

    switch list.pop() {
        ?value => {
            if value != 110 {
                return process::ExitCode::init(20)!;
            }
        },
        null => return process::ExitCode::init(21)!,
    }
    if list.len() != 11 {
        return process::ExitCode::init(22)!;
    }
    var mutable_items = list.as_mut_slice();
    mutable_items[2] = 77;
    if list.as_slice()[2] != 77 {
        return process::ExitCode::init(23)!;
    }
    list.clear_retaining_capacity();
    if not list.is_empty() {
        return process::ExitCode::init(24)!;
    }
    switch list.clear_and_free(page) {
        !ok => _ = ok,
        error! => return process::ExitCode::init(25)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_reports_private_root_entry_called_by_freestanding_start() {
    let root = temp_dir("emit_exe_reports_private_root_entry_called_by_freestanding_start");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
import std.process;

fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    process::ExitCode::init(7)!
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .output()
        .expect("run nia emit exe");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("private"), "{stderr}");
    assert!(stderr.contains("root::main"), "{stderr}");
}

#[test]
fn emit_exe_entry_name_is_chosen_by_std_runtime_not_compiler() {
    let root = temp_dir("emit_exe_entry_name_is_chosen_by_std_runtime_not_compiler");
    let main = root.join("main.nia");
    let std_root = root.join("custom_std/std.nia");
    let std_process = root.join("custom_std/std/process.nia");
    let std_start = root.join("custom_std/std/start.nia");
    let std_start_linux_x86_64 = root.join("custom_std/std/start/freestanding/linux/x86_64.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::create_dir_all(std_start_linux_x86_64.parent().expect("std start parent"))
        .expect("create custom std dir");
    std::fs::write(&std_root, "").expect("write custom std root");
    std::fs::write(&std_process, "").expect("write custom std process");
    std::fs::write(
        &std_start,
        r#"
comptime if @builtin().target.os == "linux"
    and @builtin().target.arch == "x86_64"
{
    import std.start.freestanding.linux.x86_64;
}
"#,
    )
    .expect("write custom std start facade");
    std::fs::write(
        &std_start_linux_x86_64,
        r#"
import root;

fn syscall_exit(code: i32) void {
    @asm({
        code:
            b\\syscall
        ,
        inputs: {
            rax: 60,
            rdi: code,
        },
        clobbers: [b"rcx", b"r11", b"memory"],
        options: [b"volatile"],
    });
}

@[naked]
pub extern fn _start() void {
    @asm({
        code:
            b\\call custom_start
            \\ud2
        ,
        clobbers: [b"rax", b"rcx", b"r11", b"memory"],
        options: [b"volatile"],
    });
    loop {}
}

extern fn custom_start() void {
    syscall_exit(root::mymain());
    loop {}
}
"#,
    )
    .expect("write custom std start");
    std::fs::write(
        &main,
        r#"
pub fn mymain() i32 {
    11
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-M")
        .arg(format!("std={}", std_root.display()))
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run nia emit exe with custom std start");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
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
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    process::ExitCode::init(9)!
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .current_dir(&root)
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe_name)
        .output()
        .expect("run nia emit exe -o -Orunnable");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(9));

    let report_name = format!("--opt-report{}", std::env::consts::EXE_SUFFIX);
    let report_path = root.join(&report_name);
    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .current_dir(&root)
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&report_name)
        .output()
        .expect("run nia emit exe -o --opt-report");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("backend optimization report:"), "{stdout}");
    assert!(!stderr.contains("backend optimization report:"), "{stderr}");

    let status = Command::new(&report_path)
        .status()
        .expect("run emitted executable named --opt-report");
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
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    process::ExitCode::init(5)!
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("-Oz")
        .arg("emit")
        .arg("exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .arg("--opt-report")
        .output()
        .expect("run nia emit exe --opt-report");

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

    let status = Command::new(&exe).status().expect("run emitted executable");
    assert_eq!(status.code(), Some(5));
}

#[test]
fn emitted_executables_preserve_semantics_across_optimization_levels() {
    let root = temp_dir("emitted_executables_preserve_semantics_across_optimization_levels");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
import std.process;

fn pick(flag: bool, a: i32, b: i32) i32 {
    if flag {
        a
    } else {
        b
    }
}

fn answer() i32 {
    40
}

fn identity[T](value: T) T {
    value
}

fn plus_two(value: i32) i32 {
    value + 2
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var x = answer();
    var y = x;
    y = identity[i32](y);
    var unused = plus_two(99);
    process::ExitCode::init(pick(true, plus_two(y), unused))!
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
        let output = Command::new(env!("CARGO_BIN_EXE_nia"))
            .arg(level)
            .arg("emit")
            .arg("exe")
            .arg(&main)
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap_or_else(|error| panic!("run nia {level} emit exe: {error}"));

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
    let id = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.push(format!(
        "nia_cli_{name}_{}_{:?}_{id}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}
