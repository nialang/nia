// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

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
pub comptime answer: i32 = 42;
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
fn check_can_emit_backend_optimization_report() {
    let root = temp_dir("check_can_emit_backend_optimization_report");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
const zeroes: [4]i32 = [0; 4];

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
fn emit_exe_preserves_output_paths_that_look_like_optimization_flags() {
    let root = temp_dir("emit_exe_preserves_output_paths_that_look_like_optimization_flags");
    let main = root.join("main.nia");
    let exe_name = format!("-Orunnable{}", std::env::consts::EXE_SUFFIX);
    let exe = root.join(&exe_name);
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    9
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
fn main() i32 {
    5
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

fn main() i32 {
    var x = answer();
    var y = x;
    y = identity[i32](y);
    var unused = plus_two(99);
    pick(true, plus_two(y), unused)
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
    dir.push(format!("nia_cli_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}
