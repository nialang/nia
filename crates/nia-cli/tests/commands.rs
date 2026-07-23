// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

trait BuildCommandExt {
    fn build_output_timeout(&mut self, context: &str) -> std::process::Output;
}

impl BuildCommandExt for Command {
    fn build_output_timeout(&mut self, context: &str) -> std::process::Output {
        self.output_timeout_with_resource(context, nia_test_support::build_permit())
    }
}

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
            .output_timeout("run nia check --runtime freestanding on README nia example");

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
            .output_timeout(&format!("run nia emit --ast on {example}"));

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
            .output_timeout(&format!(
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
        .output_timeout("run nia check with unused import warning");

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
        .output_timeout("emit object with unused import warning");

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
        .output_timeout("run nia help check");
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
        .output_timeout("run nia help build");
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
fn build_command_accepts_timings_after_build_command() {
    let root = temp_dir("build_command_accepts_timings_after_build_command");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;

pub fn build(b: &mut build::Build) build::Error!void {
    _ = b;
    !{}
}
"#,
    )
    .expect("write build script");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("build")
        .arg("--timings")
        .arg("--root")
        .arg(&root)
        .build_output_timeout("run nia build with timings");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("timing"), "{stderr}");
}

#[test]
fn build_command_compiles_and_runs_build_script() {
    let root = temp_dir("build_command_compiles_and_runs_build_script");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;
using std::fs;
using std::fmt;
using std::io;

pub fn build(b: &mut build::Build) build::Error!void {
    let mut buffer: [1024]u8 = [_]u8[0; 1024];
    let mut stdout = io::FileWriter::stdout(b.io(), &mut buffer[..]);
    stdout.print(&"root={}\n", &[b.package_root().text()]).as_build_error().?;
    stdout.print(&"build={}\n", &[b.build_dir().text()]).as_build_error().?;
    stdout.print(&"cache={}\n", &[b.cache_dir().text()]).as_build_error().?;
    stdout.print(&"toolchain={}\n", &[b.toolchain_executable().text()]).as_build_error().?;
    stdout.flush().as_build_error().?;
    static src_main = "src/main.nia";
    static app_name = "app";
    static build_step_name = "build";
    static check_step_name = "check";
    let root_module = b.add_module(build::ModuleOptions::init(fs::PathView::init(&src_main))).?;
    let app = b.add_executable(build::ExecutableOptions::init(&app_name, root_module)).?;
    let build_step = b.add_emit_executable_step(&build_step_name, app).?;
    _ = b.add_check_executable_step(&check_step_name, app).?;
    b.set_default_step(build_step).?;
    !{}
}
"#,
    )
    .expect("write build script");
    std::fs::create_dir_all(root.join("src").join("nested")).expect("create nested dir");
    std::fs::write(
        root.join("src").join("main.nia"),
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
    )
    .expect("write main source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("build")
        .arg("--timings")
        .arg("--root")
        .arg(root.join("src").join("nested"))
        .build_output_timeout("run nia build");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains(&format!("root={}\n", root.display())),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("build={}\n", root.join(".nia-build").display())),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("cache={}\n", root.join(".nia-cache").display())),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("toolchain={}\n", env!("CARGO_BIN_EXE_nia"))),
        "{stdout}"
    );
    assert!(!stderr.contains("error"), "{stderr}");
    assert!(root.join(".nia-build/runner").is_dir());
    assert!(root.join(".nia-cache").is_dir());
    assert!(root.join(".nia-build/app").is_file());

    let status =
        Command::new(root.join(".nia-build/app")).status_timeout("run emitted build target");
    assert_eq!(status.code(), Some(0));

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("build")
        .arg("check")
        .arg("--root")
        .arg(&root)
        .build_output_timeout("run nia build check");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn build_command_runs_step_dependencies_once_before_dependant() {
    let root = temp_dir("build_command_runs_step_dependencies_once_before_dependant");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;
using std::fmt;
using std::io;

fn prepare(b: &mut build::Build) build::Error!void {
    let mut buffer: [128]u8 = [_]u8[0; 128];
    let mut stdout = io::FileWriter::stdout(b.io(), &mut buffer[..]);
    stdout.print(&"prepare\n", &[]).as_build_error().?;
    stdout.flush().as_build_error().?;
    !{}
}

fn build_app(b: &mut build::Build) build::Error!void {
    let mut buffer: [128]u8 = [_]u8[0; 128];
    let mut stdout = io::FileWriter::stdout(b.io(), &mut buffer[..]);
    stdout.print(&"build\n", &[]).as_build_error().?;
    stdout.flush().as_build_error().?;
    !{}
}

fn check(b: &mut build::Build) build::Error!void {
    let mut buffer: [128]u8 = [_]u8[0; 128];
    let mut stdout = io::FileWriter::stdout(b.io(), &mut buffer[..]);
    stdout.print(&"check\n", &[]).as_build_error().?;
    stdout.flush().as_build_error().?;
    !{}
}

pub fn build(b: &mut build::Build) build::Error!void {
    static prepare_name = "prepare";
    static build_name = "build";
    static check_name = "check";
    let prepare_step = b.add_step(&prepare_name, &prepare).?;
    let build_step = b.add_step(&build_name, &build_app).?;
    let check_step = b.add_step(&check_name, &check).?;
    b.depend_on(build_step, prepare_step).?;
    b.depend_on(check_step, prepare_step).?;
    b.depend_on(check_step, build_step).?;
    !{}
}
"#,
    )
    .expect("write build script");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("build")
        .arg("check")
        .arg("--root")
        .arg(&root)
        .build_output_timeout("run nia build check with dependencies");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, "prepare\nbuild\ncheck\n");
}

#[test]
fn build_command_runs_executable_artifact_step_as_dependency() {
    let root = temp_dir("build_command_runs_executable_artifact_step_as_dependency");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;
using std::fs;
using std::fmt;
using std::io;

fn verify(b: &mut build::Build) build::Error!void {
    let mut buffer: [128]u8 = [_]u8[0; 128];
    let mut stdout = io::FileWriter::stdout(b.io(), &mut buffer[..]);
    stdout.print(&"verified\n", &[]).as_build_error().?;
    stdout.flush().as_build_error().?;
    !{}
}

pub fn build(b: &mut build::Build) build::Error!void {
    static src_main = "src/main.nia";
    static app_name = "app";
    static emit_name = "emit-app";
    static verify_name = "verify";
    let root_module = b.add_module(build::ModuleOptions::init(fs::PathView::init(&src_main))).?;
    let app = b.add_executable(build::ExecutableOptions::init(&app_name, root_module)).?;
    let emit = b.add_emit_executable_step(&emit_name, app).?;
    let verify_step = b.add_step(&verify_name, &verify).?;
    b.depend_on(verify_step, emit).?;
    !{}
}
"#,
    )
    .expect("write build script");
    std::fs::create_dir_all(root.join("src")).expect("create src dir");
    std::fs::write(
        root.join("src").join("main.nia"),
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
    )
    .expect("write main source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("build")
        .arg("verify")
        .arg("--root")
        .arg(&root)
        .build_output_timeout("run nia build verify with executable dependency");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "verified\n");
    assert!(root.join(".nia-build/app").is_file());
}

#[test]
fn build_command_emits_executable_with_configured_output_name() {
    let root = temp_dir("build_command_emits_executable_with_configured_output_name");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;
using std::fs;

pub fn build(b: &mut build::Build) build::Error!void {
    static src_main = "src/main.nia";
    static app_name = "app";
    static output_name = "custom-app";
    static emit_name = "emit-app";
    let root_module = b.add_module(build::ModuleOptions::init(fs::PathView::init(&src_main))).?;
    let app = b.add_executable(
        build::ExecutableOptions::init(&app_name, root_module).with_output_name(&output_name),
    ).?;
    let emit = b.add_emit_executable_step(&emit_name, app).?;
    b.set_default_step(emit).?;
    !{}
}
"#,
    )
    .expect("write build script");
    std::fs::create_dir_all(root.join("src")).expect("create src dir");
    std::fs::write(
        root.join("src").join("main.nia"),
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
    )
    .expect("write main source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("build")
        .arg("--root")
        .arg(&root)
        .build_output_timeout("run nia build with configured output name");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.join(".nia-build/custom-app").is_file());
    assert!(!root.join(".nia-build/app").exists());
}

#[test]
fn build_command_accepts_configured_module_optimization() {
    let root = temp_dir("build_command_accepts_configured_module_optimization");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;
using std::fs;

pub fn build(b: &mut build::Build) build::Error!void {
    static src_main = "src/main.nia";
    static app_name = "app";
    static check_name = "check";
    let root_module = b.add_module(
        build::ModuleOptions::init(fs::PathView::init(&src_main))
            .with_optimization(build::OptimizationMode::O0),
    ).?;
    let app = b.add_executable(build::ExecutableOptions::init(&app_name, root_module)).?;
    let check = b.add_check_executable_step(&check_name, app).?;
    b.set_default_step(check).?;
    !{}
}
"#,
    )
    .expect("write build script");
    std::fs::create_dir_all(root.join("src")).expect("create src dir");
    std::fs::write(
        root.join("src").join("main.nia"),
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
    )
    .expect("write main source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("build")
        .arg("--root")
        .arg(&root)
        .build_output_timeout("run nia build with configured module optimization");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn build_command_accepts_configured_module_imports() {
    let root = temp_dir("build_command_accepts_configured_module_imports");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;
using std::fs;

pub fn build(b: &mut build::Build) build::Error!void {
    static helper_name = "helper";
    static helper_path = "deps/helper.nia";
    static src_main = "src/main.nia";
    static app_name = "app";
    static emit_name = "emit-app";
    let imports = [
        build::ModuleImport::init(&helper_name, fs::PathView::init(&helper_path)),
    ];
    let root_module = b.add_module(
        build::ModuleOptions::init(fs::PathView::init(&src_main))
            .with_imports(&imports[..]),
    ).?;
    let app = b.add_executable(build::ExecutableOptions::init(&app_name, root_module)).?;
    let emit = b.add_emit_executable_step(&emit_name, app).?;
    b.set_default_step(emit).?;
    !{}
}
"#,
    )
    .expect("write build script");
    std::fs::create_dir_all(root.join("src")).expect("create src dir");
    std::fs::create_dir_all(root.join("deps")).expect("create deps dir");
    std::fs::write(
        root.join("deps").join("helper.nia"),
        r#"
pub fn value() i32 {
    7
}
"#,
    )
    .expect("write helper source");
    std::fs::write(
        root.join("src").join("main.nia"),
        r#"
using helper;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    if helper::value() != 7 {
        return process::exit(1)!;
    }
    !{}
}
"#,
    )
    .expect("write main source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("build")
        .arg("--root")
        .arg(&root)
        .build_output_timeout("run nia build with configured module imports");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = Command::new(root.join(".nia-build/app"))
        .status_timeout("run emitted module import executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn build_command_reports_step_dependency_cycle() {
    let root = temp_dir("build_command_reports_step_dependency_cycle");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;

fn first(b: &mut build::Build) build::Error!void {
    _ = b;
    !{}
}

fn second(b: &mut build::Build) build::Error!void {
    _ = b;
    !{}
}

pub fn build(b: &mut build::Build) build::Error!void {
    static first_name = "first";
    static second_name = "second";
    let first_step = b.add_step(&first_name, &first).?;
    let second_step = b.add_step(&second_name, &second).?;
    b.depend_on(first_step, second_step).?;
    b.depend_on(second_step, first_step).?;
    !{}
}
"#,
    )
    .expect("write build script");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("build")
        .arg("first")
        .arg("--root")
        .arg(&root)
        .build_output_timeout("run nia build first with dependency cycle");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("build runner"), "{stderr}");
    assert!(stderr.contains("exit status: 22"), "{stderr}");
}

#[test]
fn build_command_reports_unknown_named_step() {
    let root = temp_dir("build_command_reports_unknown_named_step");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;

fn check(b: &mut build::Build) build::Error!void {
    _ = b;
    !{}
}

pub fn build(b: &mut build::Build) build::Error!void {
    static check_name = "check";
    _ = b.add_step(&check_name, &check).?;
    !{}
}
"#,
    )
    .expect("write build script");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("build")
        .arg("missing")
        .arg("--root")
        .arg(&root)
        .build_output_timeout("run nia build missing");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown build step `missing`"), "{stderr}");
    assert!(stderr.contains("build runner"), "{stderr}");
    assert!(stderr.contains("exit status: 2"), "{stderr}");
}

#[test]
fn build_command_requires_explicit_default_step_when_no_step_is_requested() {
    let root = temp_dir("build_command_requires_explicit_default_step_when_no_step_is_requested");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;

fn check(b: &mut build::Build) build::Error!void {
    _ = b;
    !{}
}

pub fn build(b: &mut build::Build) build::Error!void {
    static check_name = "check";
    _ = b.add_step(&check_name, &check).?;
    !{}
}
"#,
    )
    .expect("write build script");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("build")
        .arg("--root")
        .arg(&root)
        .build_output_timeout("run nia build without explicit default step");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("build runner"), "{stderr}");
    assert!(stderr.contains("exit status: 22"), "{stderr}");
}

#[test]
fn build_command_rejects_duplicate_executable_target_name() {
    let root = temp_dir("build_command_rejects_duplicate_executable_target_name");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;
using std::fs;

pub fn build(b: &mut build::Build) build::Error!void {
    static src_main = "src/main.nia";
    static src_other = "src/other.nia";
    static app_name = "app";
    let root_module = b.add_module(build::ModuleOptions::init(fs::PathView::init(&src_main))).?;
    let other_module = b.add_module(build::ModuleOptions::init(fs::PathView::init(&src_other))).?;
    let app_options = build::ExecutableOptions::init(&app_name, root_module);
    let other_options = build::ExecutableOptions::init(&app_name, other_module);
    _ = b.add_executable(app_options).?;
    _ = b.add_executable(other_options).?;
    !{}
}
"#,
    )
    .expect("write build script");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("build")
        .arg("--root")
        .arg(&root)
        .build_output_timeout("run nia build duplicate target");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("build runner"), "{stderr}");
    assert!(stderr.contains("exit status: 22"), "{stderr}");
}

#[test]
fn build_command_rejects_invalid_executable_target_name() {
    let root = temp_dir("build_command_rejects_invalid_executable_target_name");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;
using std::fs;

pub fn build(b: &mut build::Build) build::Error!void {
    static src_main = "src/main.nia";
    static app_name = "../bad";
    let root_module = b.add_module(build::ModuleOptions::init(fs::PathView::init(&src_main))).?;
    let app_options = build::ExecutableOptions::init(&app_name, root_module);
    _ = b.add_executable(app_options).?;
    !{}
}
"#,
    )
    .expect("write build script");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("build")
        .arg("--root")
        .arg(&root)
        .build_output_timeout("run nia build invalid target");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("build runner"), "{stderr}");
    assert!(stderr.contains("exit status: 22"), "{stderr}");
    assert!(!root.join("bad").exists());
}

#[test]
fn build_command_rejects_invalid_executable_output_name() {
    let root = temp_dir("build_command_rejects_invalid_executable_output_name");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;
using std::fs;

pub fn build(b: &mut build::Build) build::Error!void {
    static src_main = "src/main.nia";
    static app_name = "app";
    static output_name = "nested/bad";
    let root_module = b.add_module(build::ModuleOptions::init(fs::PathView::init(&src_main))).?;
    let app_options = build::ExecutableOptions::init(&app_name, root_module)
        .with_output_name(&output_name);
    _ = b.add_executable(app_options).?;
    !{}
}
"#,
    )
    .expect("write build script");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("build")
        .arg("--root")
        .arg(&root)
        .build_output_timeout("run nia build invalid executable output name");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("build runner"), "{stderr}");
    assert!(stderr.contains("exit status: 22"), "{stderr}");
    assert!(!root.join(".nia-build/nested").exists());
}

#[test]
fn build_command_rejects_bare_runtime_executable_artifact() {
    let root = temp_dir("build_command_rejects_bare_runtime_executable_artifact");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;
using std::fs;

pub fn build(b: &mut build::Build) build::Error!void {
    static src_main = "src/main.nia";
    static app_name = "app";
    let root_module = b.add_module(build::ModuleOptions::init(fs::PathView::init(&src_main))).?;
    _ = b.add_executable(
        build::ExecutableOptions::init(&app_name, root_module).with_runtime(build::Runtime::Bare),
    ).?;
    !{}
}
"#,
    )
    .expect("write build script");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("build")
        .arg("--root")
        .arg(&root)
        .build_output_timeout("run nia build bare executable artifact");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("build runner"), "{stderr}");
    assert!(stderr.contains("exit status: 22"), "{stderr}");
}

#[test]
fn build_command_reports_missing_build_script() {
    let root = temp_dir("build_command_reports_missing_build_script");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("build")
        .arg("--root")
        .arg(&root)
        .build_output_timeout("run nia build without build script");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to find `build.nia`"), "{stderr}");
    assert!(
        stderr.contains(&root.to_string_lossy().to_string()),
        "{stderr}"
    );
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
    assert!(stderr.contains("timing summary stage check:"), "{stderr}");
    assert!(!stderr.contains("query timing"), "{stderr}");
    assert!(!stderr.contains("allocator."), "{stderr}");

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
    assert!(stderr.contains("timing summary stage lex:"), "{stderr}");

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
    assert!(stderr.contains("timing summary stage codegen:"), "{stderr}");
    assert!(
        stderr.contains("timing summary stage emit_llvm_ir:"),
        "{stderr}"
    );
    assert!(
        stderr.contains("timing summary query backend_lowering:"),
        "{stderr}"
    );
    assert!(
        stderr.contains("timing summary counter llvm.memory_permits: 1"),
        "{stderr}"
    );

    let traced = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("--timings")
        .arg("--timing-trace=events")
        .arg("check")
        .arg(&main)
        .output_timeout("run nia check --timing-trace=events");
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
        .output_timeout("run nia check with JSON timings");
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
        report.contains("\"driver.provider_demand_rounds\":"),
        "{report}"
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
fn invalid_timings_format_reports_expected_formats() {
    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("--timings-format=csv")
        .arg("check")
        .arg("main.nia")
        .output_timeout("run nia with invalid timings format");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown timings format `csv`"), "{stderr}");
    assert!(stderr.contains("expected text or json"), "{stderr}");
}

#[test]
fn invalid_timing_trace_option_reports_expected_modes() {
    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("--timing-trace=spans")
        .arg("check")
        .arg("main.nia")
        .output_timeout("run nia with invalid timing trace option");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown timing trace mode `--timing-trace=spans`"),
        "{stderr}"
    );
    assert!(stderr.contains("--timing-trace=events"), "{stderr}");
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
    assert!(stdout.contains("name: SymbolId"), "{stdout}");

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
    assert!(stdout.contains("modules"), "{stdout}");
    assert!(!stdout.contains("backend_lowering"), "{stdout}");
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
pub const answer: i32 = 42;
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

    for reserved in ["entry", "pkg", "builtin"] {
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
fn check_runtime_freestanding_uses_startup_contract() {
    let root = temp_dir("check_runtime_freestanding_uses_startup_contract");
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
        .arg(&private_main)
        .arg("--runtime")
        .arg("freestanding")
        .output_timeout("run nia check --runtime freestanding");

    assert!(!exe_check.status.success());
    let stderr = String::from_utf8_lossy(&exe_check.stderr);
    assert!(stderr.contains("private"), "{stderr}");
    assert!(stderr.contains("entry::main"), "{stderr}");

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
        .arg(&public_main)
        .arg("--runtime")
        .arg("freestanding")
        .output_timeout("run nia check --runtime freestanding with public entry");

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

    let removed_alias = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("check")
        .arg("--exe")
        .arg(&public_main)
        .output_timeout("run removed nia check --exe alias");

    assert!(!removed_alias.status.success());
    let stderr = String::from_utf8_lossy(&removed_alias.stderr);
    assert!(
        stderr.contains("unknown `nia check` option `--exe`"),
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
static zeroes: [4]i32 = [0; 4];

fn main() i32 {
    let mut unused = 1;
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
    let mut x = 41;
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

#[cfg(unix)]
#[test]
fn emit_exe_passes_structured_link_options_to_linker() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_dir("emit_exe_passes_structured_link_options_to_linker");
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
        .arg("--dynamic-linker")
        .arg("/loader")
        .arg("--linker")
        .arg(&linker)
        .arg("--linker-flavor")
        .arg("lld")
        .arg("-L")
        .arg("/native/lib")
        .arg("-l")
        .arg("native_api")
        .arg("--rpath")
        .arg("$ORIGIN")
        .arg("-o")
        .arg(&executable)
        .output_timeout("run nia emit --exe with structured link options");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let args = std::fs::read_to_string(&args_log).expect("read linker args");
    assert!(args.lines().any(|arg| arg == "--dynamic-linker"), "{args}");
    assert!(args.lines().any(|arg| arg == "/loader"), "{args}");
    assert!(args.lines().any(|arg| arg == "-L"), "{args}");
    assert!(args.lines().any(|arg| arg == "/native/lib"), "{args}");
    assert!(args.lines().any(|arg| arg == "-l"), "{args}");
    assert!(args.lines().any(|arg| arg == "native_api"), "{args}");
    assert!(args.lines().any(|arg| arg == "-rpath"), "{args}");
    assert!(args.lines().any(|arg| arg == "$ORIGIN"), "{args}");
}

#[test]
fn emit_exe_reports_reserved_self_hosted_linker_flavor() {
    let root = temp_dir("emit_exe_reports_reserved_self_hosted_linker_flavor");
    let main = root.join("main.nia");
    let executable = root.join("main");
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

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("--linker-flavor")
        .arg("self-hosted-elf")
        .arg("-o")
        .arg(&executable)
        .output_timeout("run nia emit --exe with reserved linker flavor");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("linker flavor `SelfHostedElf` is not implemented"),
        "{stderr}"
    );
}

#[test]
fn emit_exe_reports_missing_lld_when_not_found() {
    let root = temp_dir("emit_exe_reports_missing_lld_when_not_found");
    let main = root.join("main.nia");
    let executable = root.join("main");
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

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .env("PATH", "")
        .env_remove("NIA_LLD")
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("--linker-flavor")
        .arg("lld")
        .arg("-o")
        .arg(&executable)
        .output_timeout("run nia emit --exe with missing lld");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("linker `ld.lld` for flavor `Lld` was not found"),
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
        if path.extension().is_none_or(|extension| extension != "o") {
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
