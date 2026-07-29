// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn readme_nia_examples_check_as_freestanding_programs() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme_path = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .expect("nia-cli lives under crates/")
        .join("README.md");
    let readme = std::fs::read_to_string(&readme_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", readme_path.display()));
    let examples = nia_code_blocks(&readme);
    assert!(
        !examples.is_empty(),
        "README.md should contain at least one nia code block"
    );

    for (index, source) in examples {
        let root = temp_dir(&format!("readme_nia_examples_check_{index}"));
        let main = root.join(format!("example_{index}.nia"));
        std::fs::write(&main, source).expect("write README example source");

        let output = Command::new(env!("CARGO_BIN_EXE_nia"))
            .arg("check")
            .arg("--runtime")
            .arg("freestanding")
            .arg(&main)
            .output_timeout_for_compiler(
                "run nia check --runtime freestanding on README nia example",
            );

        assert!(
            output.status.success(),
            "README nia example {index} failed\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn repository_examples_parse_and_representative_examples_check() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let examples_dir = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .expect("nia-cli lives under crates/")
        .join("examples");
    let examples = [
        "00_minimal.nia",
        "01_values_control_flow.nia",
        "02_slices_and_strings.nia",
        "03_stdout.nia",
        "04_array_list.nia",
        "05_traits_generics.nia",
        "06_optional_error.nia",
        "07_arena_allocator.nia",
        "08_general_purpose_allocator.nia",
        "09_hash_map.nia",
        "modules/main.nia",
    ];

    for example in examples {
        let path = examples_dir.join(example);
        let output = Command::new(env!("CARGO_BIN_EXE_nia"))
            .arg("emit")
            .arg("--ast")
            .arg(&path)
            .output_timeout_for_compiler(&format!("run nia emit --ast on {example}"));

        assert!(
            output.status.success(),
            "example {example} failed to parse\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    for example in ["00_minimal.nia", "09_hash_map.nia", "modules/main.nia"] {
        let path = examples_dir.join(example);
        let output = Command::new(env!("CARGO_BIN_EXE_nia"))
            .arg("check")
            .arg("--runtime")
            .arg("freestanding")
            .arg(&path)
            .output_timeout_for_compiler(&format!(
                "run nia check --runtime freestanding on {example}"
            ));

        assert!(
            output.status.success(),
            "representative example {example} failed\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn check_reports_unused_import_warning_without_failing() {
    let root = temp_dir("check_reports_unused_import_warning_without_failing");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
using std::collections;

fn main() void {}
"#,
    )
    .expect("write source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("check")
        .arg(&main)
        .output_timeout_for_compiler("run nia check with unused import warning");

    assert!(
        output.status.success(),
        "check should succeed with warnings\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning[W0201]"), "{stderr}");
    assert!(stderr.contains("unused import `collections`"), "{stderr}");
}

#[test]
fn emit_obj_reports_unused_import_warning_without_skipping_codegen() {
    let root = temp_dir("emit_obj_reports_unused_import_warning_without_skipping_codegen");
    let main = root.join("main.nia");
    let object = root.join("main.o");
    std::fs::write(
        &main,
        r#"
using std::collections;

pub fn main() void {}
"#,
    )
    .expect("write source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--obj")
        .arg(&main)
        .arg("-o")
        .arg(&object)
        .output_timeout_for_build("emit object with unused import warning");

    assert!(
        output.status.success(),
        "emit should succeed with warnings\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning[W0201]"), "{stderr}");
    assert!(stderr.contains("unused import `collections`"), "{stderr}");
    assert!(
        std::fs::metadata(&object)
            .expect("emitted object metadata")
            .len()
            > 0,
        "emitted object should not be empty"
    );
}

fn nia_code_blocks(markdown: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut current = None::<String>;
    for line in markdown.lines() {
        match (current.as_mut(), line.trim()) {
            (None, "```nia") => current = Some(String::new()),
            (Some(source), "```") => {
                let source = std::mem::take(source);
                current = None;
                blocks.push((blocks.len(), source));
            }
            (Some(source), _) => {
                source.push_str(line);
                source.push('\n');
            }
            (None, _) => {}
        }
    }
    blocks
}

#[test]
fn help_and_version_use_nia_command_name() {
    let help = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("--help")
        .output_timeout_without_resources("run nia --help");
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
    assert!(help_stdout.contains("build [step]"), "{help_stdout}");
    assert!(!help_stdout.contains("lex <file.nia>"), "{help_stdout}");
    assert!(!help_stdout.contains("parse <file.nia>"), "{help_stdout}");
    assert!(
        help_stdout.contains("-O, -O0, -O1, -O2, -O3, -Os, -Oz"),
        "{help_stdout}"
    );
    assert!(help_stdout.contains("--timings"), "{help_stdout}");
    assert!(help_stdout.contains("--timings-format"), "{help_stdout}");
    assert!(help_stdout.contains("--timing-trace"), "{help_stdout}");
    for level in ["-O0", "-O1", "-O2", "-O3", "-Os", "-Oz"] {
        assert!(help_stdout.contains(level), "{help_stdout}");
    }

    let check_help = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("help")
        .arg("check")
        .output_timeout_without_resources("run nia help check");
    assert!(
        check_help.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&check_help.stderr)
    );
    let check_stdout = String::from_utf8_lossy(&check_help.stdout);
    assert!(!check_stdout.contains("--exe"), "{check_stdout}");
    assert!(
        check_stdout.contains("--runtime <bare|freestanding>"),
        "{check_stdout}"
    );
    assert!(check_stdout.contains("--opt-report"), "{check_stdout}");
    assert!(
        check_stdout.contains("--cache-dir <path>"),
        "{check_stdout}"
    );
    assert!(
        check_stdout.contains("-O, -O0, -O1, -O2, -O3, -Os, -Oz"),
        "{check_stdout}"
    );
    assert!(
        check_stdout.contains("optimization policy, enabled passes, change count, and changes"),
        "{check_stdout}"
    );
    assert!(check_stdout.contains("--timings"), "{check_stdout}");
    assert!(check_stdout.contains("--timings-format"), "{check_stdout}");
    assert!(check_stdout.contains("--timing-trace"), "{check_stdout}");
    assert!(
        check_stdout.contains("Timing reports are written to stderr"),
        "{check_stdout}"
    );

    let build_help = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("help")
        .arg("build")
        .output_timeout_without_resources("run nia help build");
    assert!(
        build_help.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&build_help.stderr)
    );
    let build_stdout = String::from_utf8_lossy(&build_help.stdout);
    assert!(build_stdout.contains("nia build [step]"), "{build_stdout}");
    assert!(build_stdout.contains("--root <dir>"), "{build_stdout}");
    assert!(build_stdout.contains("build.nia"), "{build_stdout}");
    assert!(
        build_stdout
            .contains("Global options such as --timings may appear before or after `build`"),
        "{build_stdout}"
    );
    assert!(build_stdout.contains(".nia-build/"), "{build_stdout}");
    assert!(build_stdout.contains(".nia-cache/"), "{build_stdout}");

    let emit_help = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("help")
        .arg("emit")
        .output_timeout_without_resources("run nia help emit");
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
    assert!(emit_stdout.contains("--timings-format"), "{emit_stdout}");
    assert!(emit_stdout.contains("--timing-trace"), "{emit_stdout}");
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
        .output_timeout_without_resources("run nia --version");
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
        .output_timeout_for_compiler("run nia check --timings");
    assert!(
        check.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let stdout = String::from_utf8_lossy(&check.stdout);
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("timing summary stage check:"), "{stderr}");
    assert!(!stderr.contains("query timing"), "{stderr}");
    assert!(!stderr.contains("allocator."), "{stderr}");

    let tokens = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--tokens")
        .arg(&main)
        .arg("--timings")
        .output_timeout_for_compiler("run nia emit --tokens --timings");
    assert!(
        tokens.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&tokens.stderr)
    );
    let stdout = String::from_utf8_lossy(&tokens.stdout);
    let stderr = String::from_utf8_lossy(&tokens.stderr);
    assert!(stdout.contains("Fn"), "{stdout}");
    assert!(!stdout.contains("timing "), "{stdout}");
    assert!(stderr.contains("timing summary stage lex:"), "{stderr}");

    let llvm = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("--timings=detail")
        .arg("emit")
        .arg("--llvm")
        .arg(&main)
        .output_timeout_for_build("run nia --timings=detail emit --llvm");
    assert!(
        llvm.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let stdout = String::from_utf8_lossy(&llvm.stdout);
    let stderr = String::from_utf8_lossy(&llvm.stderr);
    assert!(stdout.contains("define i32 @"), "{stdout}");
    assert!(!stdout.contains("timing "), "{stdout}");
    assert!(
        !stderr.contains("timing summary stage codegen:"),
        "{stderr}"
    );
    assert!(
        stderr.contains("timing summary stage emit_llvm_ir:"),
        "{stderr}"
    );
    assert!(
        stderr.contains("timing summary query codegen_preparation:"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("timing summary query backend_lowering:"),
        "{stderr}"
    );
    assert!(
        stderr.contains("timing summary counter llvm.memory_permits: 1"),
        "{stderr}"
    );
    assert!(
        stderr.contains("timing summary counter llvm.ready_task_submissions: 1"),
        "{stderr}"
    );

    let traced = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("--timings")
        .arg("--timing-trace=events")
        .arg("check")
        .arg(&main)
        .output_timeout_for_compiler("run nia check --timing-trace=events");
    assert!(
        traced.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&traced.stderr)
    );
    let stderr = String::from_utf8_lossy(&traced.stderr);
    assert!(stderr.contains("timing check:"), "{stderr}");
    assert!(stderr.contains("timing summary stage check:"), "{stderr}");

    let json = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("--timings=detail")
        .arg("--timings-format=json")
        .arg("check")
        .arg(&main)
        .output_timeout_for_compiler("run nia check with JSON timings");
    assert!(
        json.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&json.stderr)
    );
    let stderr = String::from_utf8_lossy(&json.stderr);
    let report = stderr
        .lines()
        .find(|line| line.starts_with("{\"schema_version\":1,"))
        .expect("missing JSON timing report");
    assert!(report.contains("\"max_rss_bytes\":"), "{report}");
    if cfg!(feature = "perf-alloc") {
        assert!(report.contains("\"allocator.alloc_calls\":"), "{report}");
        assert!(
            report.contains("\"allocator.allocated_bytes\":"),
            "{report}"
        );
        assert!(report.contains("\"allocator.live_bytes\":"), "{report}");
        assert!(
            report.contains("\"allocator.peak_live_bytes\":"),
            "{report}"
        );
        assert!(report.contains("\"query.value_clone_bytes\":"), "{report}");
    } else {
        assert!(!report.contains("\"allocator."), "{report}");
        assert!(!report.contains("\"query.value_clone_bytes\":"), "{report}");
    }
    assert!(report.contains("\"query.executions\":"), "{report}");
    assert!(
        report.contains("\"query.executions.parsed_module\":"),
        "{report}"
    );
    assert!(
        report.contains("\"query.executions.loader_public_surface_module_facts\":"),
        "{report}"
    );
    assert!(
        report.contains("\"driver.provider_demand_rounds\":"),
        "{report}"
    );
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
    std::builtin::atomic_load[T](ptr, 1usize)
}

fn main() i32 {
    let mut point: Point = { x: 1 };
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
        .output_timeout_for_build("run nia emit --llvm invalid atomic");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unsupported atomic value type"), "{stderr}");
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
        .output_timeout_for_build("run nia emit --obj");

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
        .output_timeout_for_build("run nia emit --obj bare runtime");

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
        .output_timeout_for_build("run nia emit --obj --runtime=freestanding");

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

#[cfg(all(unix, target_os = "linux", target_arch = "x86_64"))]
fn object_dir_defines_symbol(dir: &std::path::Path, symbol: &str) -> bool {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("read object dir {}: {error}", dir.display()));
    for entry in entries {
        let entry = entry.expect("read object entry");
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "o") {
            continue;
        }
        let output = Command::new("nm")
            .arg("--defined-only")
            .arg(&path)
            .output_timeout_without_resources("run nm on emitted object");
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
