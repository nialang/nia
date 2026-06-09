// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    io::Read,
    process::{Command, ExitStatus, Output, Stdio},
    sync::{
        Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

static TEMP_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);
static COMMAND_LIMIT: CommandLimit = CommandLimit::new(4);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);

struct CommandLimit {
    state: Mutex<usize>,
    available: Condvar,
    max: usize,
}

struct CommandPermit<'a> {
    limit: &'a CommandLimit,
}

impl CommandLimit {
    const fn new(max: usize) -> Self {
        Self {
            state: Mutex::new(0),
            available: Condvar::new(),
            max,
        }
    }

    fn acquire(&self) -> CommandPermit<'_> {
        let mut running = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while *running >= self.max {
            running = self
                .available
                .wait(running)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        *running += 1;
        CommandPermit { limit: self }
    }
}

impl Drop for CommandPermit<'_> {
    fn drop(&mut self) {
        let mut running = self
            .limit
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *running -= 1;
        self.limit.available.notify_one();
    }
}

trait CommandExt {
    fn output_timeout(&mut self, context: &str) -> Output;
    fn status_timeout(&mut self, context: &str) -> ExitStatus;
}

impl CommandExt for Command {
    fn output_timeout(&mut self, context: &str) -> Output {
        let _permit = COMMAND_LIMIT.acquire();
        self.stdout(Stdio::piped()).stderr(Stdio::piped());
        prepare_command(self);
        let mut child = self
            .spawn()
            .unwrap_or_else(|error| panic!("{context}: failed to spawn command: {error}"));
        let stdout = child
            .stdout
            .take()
            .expect("stdout pipe was configured before spawn");
        let stderr = child
            .stderr
            .take()
            .expect("stderr pipe was configured before spawn");
        let stdout_reader = thread::spawn(move || {
            let mut stdout = stdout;
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).map(|_| bytes)
        });
        let stderr_reader = thread::spawn(move || {
            let mut stderr = stderr;
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).map(|_| bytes)
        });
        let status = wait_child_timeout(&mut child, context);
        let stdout = stdout_reader
            .join()
            .unwrap_or_else(|_| panic!("{context}: stdout reader panicked"))
            .unwrap_or_else(|error| panic!("{context}: failed to read stdout: {error}"));
        let stderr = stderr_reader
            .join()
            .unwrap_or_else(|_| panic!("{context}: stderr reader panicked"))
            .unwrap_or_else(|error| panic!("{context}: failed to read stderr: {error}"));
        Output {
            status,
            stdout,
            stderr,
        }
    }

    fn status_timeout(&mut self, context: &str) -> ExitStatus {
        let _permit = COMMAND_LIMIT.acquire();
        prepare_command(self);
        let mut child = self
            .spawn()
            .unwrap_or_else(|error| panic!("{context}: failed to spawn command: {error}"));
        wait_child_timeout(&mut child, context)
    }
}

fn prepare_command(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
}

fn wait_child_timeout(child: &mut std::process::Child, context: &str) -> ExitStatus {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status,
            Ok(None) if start.elapsed() >= COMMAND_TIMEOUT => {
                terminate_child(child);
                let _ = child.wait();
                panic!("{context}: command timed out after {COMMAND_TIMEOUT:?}");
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => panic!("{context}: failed to wait for command: {error}"),
        }
    }
}

#[cfg(unix)]
fn terminate_child(child: &mut std::process::Child) {
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(format!("-{}", child.id()))
        .status();
    thread::sleep(Duration::from_millis(100));
    if matches!(child.try_wait(), Ok(None)) {
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg(format!("-{}", child.id()))
            .status();
    }
}

#[cfg(not(unix))]
fn terminate_child(child: &mut std::process::Child) {
    let _ = child.kill();
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
    assert!(!help_stdout.contains("lex <file.nia>"), "{help_stdout}");
    assert!(!help_stdout.contains("parse <file.nia>"), "{help_stdout}");
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
        .output_timeout("run nia help check");
    assert!(
        check_help.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&check_help.stderr)
    );
    let check_stdout = String::from_utf8_lossy(&check_help.stdout);
    assert!(check_stdout.contains("--exe"), "{check_stdout}");
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
    assert!(emit_stdout.contains("--opt-report"), "{emit_stdout}");
    assert!(
        emit_stdout
            .contains("optimization policy, enabled passes, change count, and changes to stderr"),
        "{emit_stdout}"
    );
    for level in ["-O0", "-O1", "-O2", "-O3", "-Os", "-Oz"] {
        assert!(emit_stdout.contains(level), "{emit_stdout}");
    }

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
        .output_timeout("run nia check with trailing -M");

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
        .output_timeout("run nia check with reserved root module map");

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
import std.process;

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
import std.process;

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
fn atomic_std_facade_checks_emits_and_runs() {
    let root = temp_dir("atomic_std_facade_checks_emits_and_runs");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std;
import std.atomic;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var value = std::Atomic[usize]::init(1usize);
    let old = value.fetch_add_monotonic(2usize);
    let now = value.load_acquire();
    if old != 1usize or now != 3usize {
        return process::exit(1)!;
    }
    if value.fetch_or_seq_cst(4usize) != 3usize {
        return process::exit(2)!;
    }
    if value.fetch_and_seq_cst(6usize) != 7usize {
        return process::exit(3)!;
    }
    if value.fetch_xor_seq_cst(2usize) != 6usize {
        return process::exit(4)!;
    }
    switch value.cmpxchg_strong_seq_cst(4usize, 5usize) {
        ?actual => {
            _ = actual;
            return process::exit(5)!;
        },
        null => {},
    }
    switch value.cmpxchg_strong_seq_cst(4usize, 5usize) {
        ?actual => {
            if actual != 5usize {
                return process::exit(6)!;
            }
        },
        null => return process::exit(7)!,
    }
    atomic::fence_seq_cst();
    !{}
}
"#,
    )
    .expect("write test source");

    let check = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("check")
        .arg("--exe")
        .arg(&main)
        .output_timeout("run nia check --exe atomic");
    assert!(
        check.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&check.stderr)
    );

    let llvm = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--llvm")
        .arg(&main)
        .output_timeout("run nia emit --llvm atomic");
    assert!(
        llvm.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let stdout = String::from_utf8_lossy(&llvm.stdout);
    assert!(stdout.contains("atomicrmw add"), "{stdout}");
    assert!(stdout.contains("atomicrmw or"), "{stdout}");
    assert!(stdout.contains("atomicrmw and"), "{stdout}");
    assert!(stdout.contains("atomicrmw xor"), "{stdout}");
    assert!(stdout.contains("load atomic"), "{stdout}");
    assert!(stdout.contains("cmpxchg"), "{stdout}");
    assert!(stdout.contains("fence seq_cst"), "{stdout}");

    let emit = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe atomic");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );
    let status = Command::new(&exe).status_timeout("run emitted atomic executable");
    assert_eq!(status.code(), Some(0));
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
    (7 as process::ExitCode)!
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(7));
}

#[test]
fn emit_exe_exit_code_is_open_enum() {
    let root = temp_dir("emit_exe_exit_code_is_open_enum");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.process;
import std.fs;

using process::{ExitCode, exit};

fn pick(flag: bool) ExitCode {
    if flag {
        11 as ExitCode
    } else {
        ExitCode::Success
    }
}

fn pick_result() fs::Error!ExitCode {
    !pick(true)
}

fn fail_with_no_space() fs::Error!void {
    fs::Error::NoSpace!
}

pub fn main(init: process::Init) ExitCode!void {
    _ = init;

    if (ExitCode::Success as i32) != 0 {
        return exit(1)!;
    }
    if (exit(11) as i32) != 11 {
        return exit(2)!;
    }
    if (fs::Error::NotFound.as_exit_code() as i32) != 2 {
        return exit(3)!;
    }
    let picked = pick_result().exit().?;
    if (picked as i32) != 11 {
        return exit(4)!;
    }
    fail_with_no_space().exit()
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(28));
}

#[test]
fn emit_exe_can_use_direct_std_modules() {
    let root = temp_dir("emit_exe_can_use_direct_std_modules");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.io;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var writer = io::DiscardingWriter::init();
    switch writer.write_all(b"nia") {
        !ok => _ = ok,
        error! => return (1 as process::ExitCode)!,
    }
    if writer.len() != 3 {
        return (2 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_can_use_std_math_usize_helpers() {
    let root = temp_dir("emit_exe_can_use_std_math_usize_helpers");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.math;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    if 0usize.is_power_of_two() {
        return (1 as process::ExitCode)!;
    }
    if not 4096usize.is_power_of_two() {
        return (2 as process::ExitCode)!;
    }
    switch 10usize.checked_add(5usize) {
        ?value => {
            if value != 15usize {
                return (3 as process::ExitCode)!;
            }
        },
        null => return (4 as process::ExitCode)!,
    }
    switch 18446744073709551615usize.checked_add(1usize) {
        ?value => {
            _ = value;
            return (5 as process::ExitCode)!;
        },
        null => {},
    }
    switch 12usize.checked_mul(3usize) {
        ?value => {
            if value != 36usize {
                return (6 as process::ExitCode)!;
            }
        },
        null => return (7 as process::ExitCode)!,
    }
    switch 4611686018427387904usize.checked_mul(4usize) {
        ?value => {
            _ = value;
            return (8 as process::ExitCode)!;
        },
        null => {},
    }
    switch 17usize.align_forward(8usize) {
        ?value => {
            if value != 24usize {
                return (9 as process::ExitCode)!;
            }
        },
        null => return (10 as process::ExitCode)!,
    }
    switch 17usize.align_forward(3usize) {
        ?value => {
            _ = value;
            return (11 as process::ExitCode)!;
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
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    var args = init.args();
    if args.len() != 3 {
        return (1 as process::ExitCode)!;
    }
    var first_arg = switch args.get(1) {
        ?value => value,
        null => return (2 as process::ExitCode)!,
    };
    var second_arg = switch args.get(2) {
        ?value => value,
        null => return (3 as process::ExitCode)!,
    };
    var first = first_arg.raw_bytes();
    var second = second_arg.raw_bytes();
    if first.len() != 3 {
        return (4 as process::ExitCode)!;
    }
    if first[0] != 110u8 or first[1] != 105u8 or first[2] != 97u8 {
        return (5 as process::ExitCode)!;
    }
    if second.len() != 4 {
        return (6 as process::ExitCode)!;
    }
    switch args.get(3) {
        ?value => {
            _ = value;
            return (7 as process::ExitCode)!;
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
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .arg("nia")
        .arg("lang")
        .status_timeout("run emitted executable");
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
import std.process;

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

pub fn main(init: process::Init) process::ExitCode!void {
    var env = init.env();
    var index = 0usize;
    while index < env.len() {
        var item = switch env.get(index) {
            ?value => value,
            null => return (1 as process::ExitCode)!,
        };
        if starts_with_needle(item.raw_bytes()) {
            return !{};
        }
        index += 1usize;
    }
    return (2 as process::ExitCode)!;
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .env("NIA_TEST_ENV", "ok")
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_can_use_error_union_conversion_extension() {
    let root = temp_dir("emit_exe_can_use_error_union_conversion_extension");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.process;

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

extend[T] ParseError!T {
    fn as_app_error(self) AppError!T {
        switch self {
            !value => !value,
            err! => map_parse_error(err)!,
        }
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    switch parse().as_app_error() {
        !value => return (value as process::ExitCode)!,
        err! => return (err as i32 as process::ExitCode)!,
    }
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(7));
}

#[test]
fn emit_exe_can_write_stdout_through_std_io() {
    let root = temp_dir("emit_exe_can_write_stdout_through_std_io");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.io;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    var buffer: [0]u8 = [];
    var stdout = io::FileWriter::stdout(init.io(), &mut buffer[..]);
    switch stdout.write_all(b"nia\n") {
        !ok => _ = ok,
        error! => return (1 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).output_timeout("run emitted executable");
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
import std.io;
import std.fmt;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    var buffer: [128]u8 = [0; 128];
    var stdout = io::FileWriter::stdout(init.io(), &mut buffer[..]);
    switch stdout.print("A¢€😀, {}\n", [&'λ']) {
        !ok => _ = ok,
        error! => return (1 as process::ExitCode)!,
    }
    switch stdout.flush() {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).output_timeout("run emitted executable");
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
import std.io;
import std.fmt;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var storage: [8]u8 = [0, 0, 0, 0, 0, 0, 0, 0];
    var writer = io::FixedBufferWriter::init(&mut storage[..]);
    switch writer.print("nia {}", [&7]) {
        !ok => _ = ok,
        error! => return (1 as process::ExitCode)!,
    }
    if writer.len() != 5 {
        return (2 as process::ExitCode)!;
    }

    var copied: [5]u8 = [0, 0, 0, 0, 0];
    var reader = io::FixedBufferReader::init(writer.written());
    switch reader.read_exact(&mut copied[..]) {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }
    var expected = b"nia 7";
    if copied[0] != expected[0] or copied[1] != expected[1] or copied[2] != expected[2] or copied[3] != expected[3] or copied[4] != expected[4] {
        return (4 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_fmt_formats_primitives_and_array_list() {
    let root = temp_dir("emit_exe_std_fmt_formats_primitives_and_array_list");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std;
import std.fmt;
import std.io;
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    var raw: [256]u8 = [_]u8[0; 256];
    var stdout = io::FileWriter::stdout(init.io(), raw);

    var allocator = mem::PageAllocator::init();
    var values = std::ArrayList[i32]::init();
    defer values.deinit(&mut allocator).exit().?;

    values.push(&mut allocator, 10).exit().?;
    values.push(&mut allocator, 20).exit().?;
    values.push(&mut allocator, 30).exit().?;

    var total = 0;
    for &value in values.iter() {
        total += value;
    }

    let signed: i8 = -5i8;
    let wide: u64 = 123456789u64;
    let ok = true;
    let ch = 'λ';
    stdout.print("list={} total={} signed={} wide={} ok={} ch={}\n", [
        &values,
        &total,
        &signed,
        &wide,
        &ok,
        &ch,
    ]).exit().?;
    stdout.flush().exit().?;
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).output_timeout("run emitted executable");
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "list=[10, 20, 30] total=60 signed=-5 wide=123456789 ok=true ch=λ\n"
    );
}

#[test]
fn emit_exe_local_pointer_binding_patterns_destructure_values() {
    let root = temp_dir("emit_exe_local_pointer_binding_patterns_destructure_values");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var left = 20;
    var right = 22;

    let &x = &left;
    var &mut y: i32 = &mut right;
    y += 1;

    if x + y != 43 {
        return (1 as process::ExitCode)!;
    }
    if right != 22 {
        return (2 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
        error! => return (1 as process::ExitCode)!,
    }
    if discard.len() != 6 {
        return (2 as process::ExitCode)!;
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
        error! => return (3 as process::ExitCode)!,
    }
    if n != 3 {
        return (4 as process::ExitCode)!;
    }
    if copied[0] != b'a' or copied[1] != b'b' or copied[2] != b'c' {
        return (5 as process::ExitCode)!;
    }
    switch limited.read(&mut copied[..]) {
        !value => n = value,
        error! => return (6 as process::ExitCode)!,
    }
    if n != 0 {
        return (7 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
        error! => return (1 as process::ExitCode)!,
    }
    if writer.len() != 3 or backing.len() != 0 {
        return (2 as process::ExitCode)!;
    }

    switch writer.write_byte(b'd') {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }
    if writer.len() != 4 or backing.len() != 0 {
        return (4 as process::ExitCode)!;
    }

    switch writer.write_all(b"efghij") {
        !ok => _ = ok,
        error! => return (5 as process::ExitCode)!,
    }
    if writer.len() != 0 or backing.len() != 10 {
        return (6 as process::ExitCode)!;
    }

    switch writer.flush() {
        !ok => _ = ok,
        error! => return (7 as process::ExitCode)!,
    }
    if backing.len() != 10 {
        return (8 as process::ExitCode)!;
    }

    var expected = b"abcdefghij";
    var written = backing.written();
    var index = 0usize;
    while index < written.len() {
        if written[index] != expected[index] {
            return (9 as process::ExitCode)!;
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
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_io_buffered_writer_flushes_partial_writes() {
    let root = temp_dir("emit_exe_std_io_buffered_writer_flushes_partial_writes");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.io;
import std.process;

struct PartialWriter {
    inner: io::FixedBufferWriter,
}

extend PartialWriter {
    fn init(buffer: &mut [u8]) PartialWriter {
        { inner: io::FixedBufferWriter::init(buffer) }
    }

    fn len(&self) usize {
        self.inner.len()
    }

    fn written(&self) &[u8] {
        self.inner.written()
    }
}

extend PartialWriter : io::Writer {
    type Error = io::BufferError;

    fn short_write(&self) Error {
        io::BufferError::ShortWrite
    }

    fn write(&mut self, bytes: &[u8]) Error!usize {
        var count = bytes.len();
        if count > 2usize {
            count = 2usize;
        }
        self.inner.write(&bytes[0..count])
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var storage: [16]u8 = [0; 16];
    var backing = PartialWriter::init(&mut storage[..]);
    var buffer_storage: [8]u8 = [0; 8];
    var writer = io::BufferedWriter[PartialWriter]::init(
        &mut backing,
        &mut buffer_storage[..],
    );

    switch writer.write_all(b"abcdef") {
        !ok => _ = ok,
        error! => return (1 as process::ExitCode)!,
    }
    if writer.len() != 6 or backing.len() != 0 {
        return (2 as process::ExitCode)!;
    }

    switch writer.flush() {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }
    if writer.len() != 0 or backing.len() != 6 {
        return (4 as process::ExitCode)!;
    }

    let expected = b"abcdef";
    let written = backing.written();
    var index = 0usize;
    while index < expected.len() {
        if written[index] != expected[index] {
            return (5 as process::ExitCode)!;
        }
        index += 1usize;
    }

    var direct_storage: [16]u8 = [0; 16];
    var direct_backing = PartialWriter::init(&mut direct_storage[..]);
    var direct_buffer_storage: [4]u8 = [0; 4];
    var direct_writer = io::BufferedWriter[PartialWriter]::init(
        &mut direct_backing,
        &mut direct_buffer_storage[..],
    );
    switch direct_writer.write_all(b"ghijkl") {
        !ok => _ = ok,
        error! => return (6 as process::ExitCode)!,
    }
    if direct_writer.len() != 0 or direct_backing.len() != 6 {
        return (7 as process::ExitCode)!;
    }
    let direct_expected = b"ghijkl";
    let direct_written = direct_backing.written();
    index = 0usize;
    while index < direct_expected.len() {
        if direct_written[index] != direct_expected[index] {
            return (8 as process::ExitCode)!;
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
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
        error! => return (1 as process::ExitCode)!,
    }
    if n != 2 or first[0] != b'a' or first[1] != b'b' {
        return (2 as process::ExitCode)!;
    }
    if reader.len() != 2 {
        return (3 as process::ExitCode)!;
    }

    var second: [3]u8 = [0; 3];
    switch reader.read(&mut second[..]) {
        !value => n = value,
        error! => return (4 as process::ExitCode)!,
    }
    if n != 2 or second[0] != b'c' or second[1] != b'd' {
        return (5 as process::ExitCode)!;
    }
    if reader.len() != 0 {
        return (6 as process::ExitCode)!;
    }

    var third: [5]u8 = [0; 5];
    switch reader.read(&mut third[..]) {
        !value => n = value,
        error! => return (7 as process::ExitCode)!,
    }
    if n != 5 {
        return (8 as process::ExitCode)!;
    }
    if third[0] != b'e' or third[1] != b'f' or third[2] != b'g' or third[3] != b'h' or third[4] != b'i' {
        return (9 as process::ExitCode)!;
    }

    var fourth: [2]u8 = [0; 2];
    switch reader.read(&mut fourth[..]) {
        !value => n = value,
        error! => return (10 as process::ExitCode)!,
    }
    if n != 1 or fourth[0] != b'j' {
        return (11 as process::ExitCode)!;
    }

    switch reader.read(&mut fourth[..]) {
        !value => n = value,
        error! => return (12 as process::ExitCode)!,
    }
    if n != 0 {
        return (13 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_io_read_exact_handles_partial_reads() {
    let root = temp_dir("emit_exe_std_io_read_exact_handles_partial_reads");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.io;
import std.process;

struct PartialReader {
    inner: io::FixedBufferReader,
}

extend PartialReader {
    fn init(bytes: &[u8]) PartialReader {
        { inner: io::FixedBufferReader::init(bytes) }
    }
}

extend PartialReader : io::Reader {
    type Error = io::BufferError;

    fn end_of_stream(&self) Error {
        io::BufferError::EndOfStream
    }

    fn read(&mut self, bytes: &mut [u8]) Error!usize {
        var count = bytes.len();
        if count > 2usize {
            count = 2usize;
        }
        self.inner.read(&mut bytes[0..count])
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var source = PartialReader::init(b"abcdef");
    var bytes: [6]u8 = [0; 6];
    switch source.read_exact(&mut bytes[..]) {
        !ok => _ = ok,
        error! => return (1 as process::ExitCode)!,
    }
    let expected = b"abcdef";
    var index = 0usize;
    while index < expected.len() {
        if bytes[index] != expected[index] {
            return (2 as process::ExitCode)!;
        }
        index += 1usize;
    }

    var short = PartialReader::init(b"xy");
    var too_many: [3]u8 = [0; 3];
    switch short.read_exact(&mut too_many[..]) {
        !ok => {
            _ = ok;
            return (3 as process::ExitCode)!;
        },
        error! => {},
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
    var path = fs::Path::init("data.txt");
    var cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => cwd = value,
        error! => return (90 as process::ExitCode)!,
    }
    defer {
        switch cwd.close() {
            !ok => _ = ok,
            error! => {},
        };
    };
    var file: fs::File;
    switch cwd.create_file(path, fs::CreateOptions::read_write()) {
        !value => file = value,
        error! => return (1 as process::ExitCode)!,
    }
    var write_buffer: [64]u8 = [0; 64];
    var writer = file.writer(init.io(), &mut write_buffer[..]);
    switch writer.write_all(b"nia fs") {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }
    switch writer.flush() {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (4 as process::ExitCode)!,
    }

    var opened: fs::File;
    switch cwd.open_file(path, fs::OpenOptions::read_only()) {
        !value => opened = value,
        error! => return (5 as process::ExitCode)!,
    }
    var read_buffer: [64]u8 = [0; 64];
    var reader = opened.reader(init.io(), &mut read_buffer[..]);
    var bytes: [6]u8 = [0, 0, 0, 0, 0, 0];
    switch reader.read_exact(&mut bytes[..]) {
        !ok => _ = ok,
        error! => return (6 as process::ExitCode)!,
    }
    switch opened.close() {
        !ok => _ = ok,
        error! => return (7 as process::ExitCode)!,
    }
    var expected = b"nia fs";
    var index = 0usize;
    while index < bytes.len() {
        if bytes[index] != expected[index] {
            return (8 as process::ExitCode)!;
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
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        std::fs::read(&data_path).expect("read data file"),
        b"nia fs"
    );
}

#[test]
fn emit_exe_std_fs_file_open_create_and_close() {
    let root = temp_dir("emit_exe_std_fs_file_open_create_and_close");
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
    var path = fs::Path::init("data.txt");
    var file: fs::File;
    switch fs::File::create(path, fs::CreateOptions::read_write()) {
        !value => file = value,
        error! => return (1 as process::ExitCode)!,
    }
    var write_buffer: [16]u8 = [0; 16];
    var writer = file.writer(init.io(), &mut write_buffer[..]);
    switch writer.write_all(b"open close") {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }
    switch writer.flush() {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (4 as process::ExitCode)!,
    }

    var opened: fs::File;
    switch fs::File::open(path, fs::OpenOptions::read_only()) {
        !value => opened = value,
        error! => return (5 as process::ExitCode)!,
    }
    var read_buffer: [16]u8 = [0; 16];
    var reader = opened.reader(init.io(), &mut read_buffer[..]);
    var bytes: [10]u8 = [0; 10];
    switch reader.read_exact(&mut bytes[..]) {
        !ok => _ = ok,
        error! => return (6 as process::ExitCode)!,
    }
    switch opened.close() {
        !ok => _ = ok,
        error! => return (7 as process::ExitCode)!,
    }
    var expected = b"open close";
    var index = 0usize;
    while index < bytes.len() {
        if bytes[index] != expected[index] {
            return (8 as process::ExitCode)!;
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
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        std::fs::read(&data_path).expect("read data file"),
        b"open close"
    );
}

#[test]
fn emit_exe_std_fs_file_seek_len_truncate_and_sync() {
    let root = temp_dir("emit_exe_std_fs_file_seek_len_truncate_and_sync");
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
    var path = fs::Path::init("data.txt");
    var file: fs::File;
    switch fs::File::create(path, fs::CreateOptions::read_write()) {
        !value => file = value,
        error! => return (1 as process::ExitCode)!,
    }

    var write_buffer: [16]u8 = [0; 16];
    var writer = file.writer(init.io(), &mut write_buffer[..]);
    switch writer.write_all(b"abcdef") {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }
    switch writer.flush() {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }

    switch file.len() {
        !value => {
            if value != 6u64 {
                return (4 as process::ExitCode)!;
            }
        },
        error! => return (5 as process::ExitCode)!,
    }
    switch file.seek_by(0) {
        !value => {
            if value != 6u64 {
                return (6 as process::ExitCode)!;
            }
        },
        error! => return (7 as process::ExitCode)!,
    }
    switch file.seek_to(2u64) {
        !value => {
            if value != 2u64 {
                return (8 as process::ExitCode)!;
            }
        },
        error! => return (9 as process::ExitCode)!,
    }
    switch file.seek_by(1i64) {
        !value => {
            if value != 3u64 {
                return (10 as process::ExitCode)!;
            }
        },
        error! => return (11 as process::ExitCode)!,
    }
    switch file.seek_from_end(-2i64) {
        !value => {
            if value != 4u64 {
                return (12 as process::ExitCode)!;
            }
        },
        error! => return (13 as process::ExitCode)!,
    }

    switch file.truncate(4u64) {
        !ok => _ = ok,
        error! => return (14 as process::ExitCode)!,
    }
    switch file.seek_to(9223372036854775808u64) {
        !value => {
            _ = value;
            return (20 as process::ExitCode)!;
        },
        err! => {
            if err != fs::Error::OutOfRange {
                return (21 as process::ExitCode)!;
            }
        },
    }
    switch file.truncate(9223372036854775808u64) {
        !ok => {
            _ = ok;
            return (22 as process::ExitCode)!;
        },
        err! => {
            if err != fs::Error::OutOfRange {
                return (23 as process::ExitCode)!;
            }
        },
    }
    switch file.len() {
        !value => {
            if value != 4u64 {
                return (15 as process::ExitCode)!;
            }
        },
        error! => return (16 as process::ExitCode)!,
    }
    switch file.sync_data() {
        !ok => _ = ok,
        error! => return (17 as process::ExitCode)!,
    }
    switch file.sync() {
        !ok => _ = ok,
        error! => return (18 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (19 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
    assert_eq!(std::fs::read(&data_path).expect("read data file"), b"abcd");
}

#[test]
fn emit_exe_std_fs_file_metadata() {
    let root = temp_dir("emit_exe_std_fs_file_metadata");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.fs;
import std.io;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    var path = fs::Path::init("data.txt");
    var file: fs::File;
    switch fs::File::create(path, fs::CreateOptions::read_write()) {
        !value => file = value,
        error! => return (1 as process::ExitCode)!,
    }

    var write_buffer: [16]u8 = [0; 16];
    var writer = file.writer(init.io(), &mut write_buffer[..]);
    switch writer.write_all(b"metadata") {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }
    switch writer.flush() {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }

    switch file.metadata() {
        !metadata => {
            if metadata.kind() != fs::FileKind::File {
                return (4 as process::ExitCode)!;
            }
            if metadata.size() != 8u64 {
                return (5 as process::ExitCode)!;
            }
            switch metadata.link_count() {
                ?value => {
                    if value == 0u32 {
                        return (6 as process::ExitCode)!;
                    }
                },
                null => {},
            }
            if metadata.preferred_block_size() == 0u32 {
                return (7 as process::ExitCode)!;
            }
        },
        error! => return (8 as process::ExitCode)!,
    }

    var cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => cwd = value,
        error! => return (9 as process::ExitCode)!,
    }
    switch cwd.metadata(path, fs::MetadataOptions::init()) {
        !metadata => {
            if metadata.kind() != fs::FileKind::File {
                return (10 as process::ExitCode)!;
            }
            if metadata.size() != 8u64 {
                return (11 as process::ExitCode)!;
            }
            switch metadata.accessed() {
                ?time => {
                    _ = time.seconds();
                    _ = time.nanos();
                },
                null => {},
            }
            _ = metadata.modified().seconds();
            switch metadata.status_changed() {
                ?time => _ = time.nanos(),
                null => {},
            }
        },
        error! => return (12 as process::ExitCode)!,
    }

    switch cwd.close() {
        !ok => _ = ok,
        error! => return (13 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (14 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
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
    var path = fs::Path::init("nia-λ.txt");
    var cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => cwd = value,
        error! => return (90 as process::ExitCode)!,
    }
    defer {
        switch cwd.close() {
            !ok => _ = ok,
            error! => {},
        };
    };
    var file: fs::File;
    switch cwd.create_file(path, fs::CreateOptions::init()) {
        !value => file = value,
        error! => return (1 as process::ExitCode)!,
    }
    var buffer: [64]u8 = [0; 64];
    var writer = file.writer(init.io(), &mut buffer[..]);
    switch writer.write_all(b"ok") {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }
    switch writer.flush() {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (4 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
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
    var cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => cwd = value,
        error! => return (90 as process::ExitCode)!,
    }
    defer {
        switch cwd.close() {
            !ok => _ = ok,
            error! => {},
        };
    };
    switch cwd.open_file(path, fs::OpenOptions::read_only()) {
        !file => {
            _ = file;
            return (1 as process::ExitCode)!;
        },
        err! => {
            if err == fs::Error::Invalid {
                !{}
            } else {
                return (2 as process::ExitCode)!;
            }
        },
    }
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
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
        return (1 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_fs_can_delete_files() {
    let root = temp_dir("emit_exe_std_fs_can_delete_files");
    let data_path = root.join("delete-me.txt");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.fs;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => cwd = value,
        error! => return (90 as process::ExitCode)!,
    }
    defer {
        switch cwd.close() {
            !ok => _ = ok,
            error! => {},
        };
    };
    var file: fs::File;
    switch cwd.create_file(fs::Path::init("delete-me.txt"), fs::CreateOptions::init()) {
        !value => file = value,
        error! => return (1 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }
    switch cwd.delete_file(fs::Path::init("delete-me.txt")) {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }
    switch cwd.open_file(fs::Path::init("delete-me.txt"), fs::OpenOptions::read_only()) {
        !file => {
            _ = file;
            return (4 as process::ExitCode)!;
        },
        error! => {},
    }

    switch cwd.delete_file(fs::Path::init("bad\0path")) {
        !ok => {
            _ = ok;
            return (5 as process::ExitCode)!;
        },
        error! => {},
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
    assert!(!data_path.exists());
}

#[test]
fn emit_exe_std_fs_can_create_rename_and_delete_dirs() {
    let root = temp_dir("emit_exe_std_fs_can_create_rename_and_delete_dirs");
    let old_path = root.join("old-name.txt");
    let new_path = root.join("subdir").join("new-name.txt");
    let dir_path = root.join("subdir");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.fs;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => cwd = value,
        error! => return (90 as process::ExitCode)!,
    }
    defer {
        switch cwd.close() {
            !ok => _ = ok,
            error! => {},
        };
    };

    switch cwd.create_dir(fs::Path::init("subdir"), fs::CreateDirOptions::init()) {
        !ok => _ = ok,
        error! => return (1 as process::ExitCode)!,
    }

    var file: fs::File;
    switch cwd.create_file(fs::Path::init("old-name.txt"), fs::CreateOptions::init()) {
        !value => file = value,
        error! => return (2 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }

    switch cwd.rename(fs::Path::init("old-name.txt"), fs::Path::init("subdir/new-name.txt")) {
        !ok => _ = ok,
        error! => return (4 as process::ExitCode)!,
    }

    switch cwd.open_file(fs::Path::init("old-name.txt"), fs::OpenOptions::read_only()) {
        !value => {
            _ = value;
            return (5 as process::ExitCode)!;
        },
        error! => {},
    }

    switch cwd.open_file(fs::Path::init("subdir/new-name.txt"), fs::OpenOptions::read_only()) {
        !value => file = value,
        error! => return (6 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (7 as process::ExitCode)!,
    }

    switch cwd.delete_dir(fs::Path::init("subdir")) {
        !ok => {
            _ = ok;
            return (8 as process::ExitCode)!;
        },
        error! => {},
    }

    switch cwd.delete_file(fs::Path::init("subdir/new-name.txt")) {
        !ok => _ = ok,
        error! => return (9 as process::ExitCode)!,
    }
    switch cwd.delete_dir(fs::Path::init("subdir")) {
        !ok => _ = ok,
        error! => return (10 as process::ExitCode)!,
    }

    switch cwd.create_dir(fs::Path::init("bad\0path"), fs::CreateDirOptions::init()) {
        !ok => {
            _ = ok;
            return (11 as process::ExitCode)!;
        },
        error! => {},
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
    assert!(!old_path.exists());
    assert!(!new_path.exists());
    assert!(!dir_path.exists());
}

#[test]
fn emit_exe_std_fs_can_open_dirs_as_capabilities() {
    let root = temp_dir("emit_exe_std_fs_can_open_dirs_as_capabilities");
    let data_path = root.join("subdir").join("inside.txt");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.fs;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => cwd = value,
        error! => return (90 as process::ExitCode)!,
    }
    defer {
        switch cwd.close() {
            !ok => _ = ok,
            error! => {},
        };
    };
    switch cwd.create_dir(fs::Path::init("subdir"), fs::CreateDirOptions::init()) {
        !ok => _ = ok,
        error! => return (1 as process::ExitCode)!,
    }

    var subdir: fs::Dir;
    switch cwd.open_dir(fs::Path::init("subdir"), fs::OpenDirOptions::init()) {
        !value => subdir = value,
        error! => return (2 as process::ExitCode)!,
    }

    var file: fs::File;
    switch subdir.create_file(fs::Path::init("inside.txt"), fs::CreateOptions::init()) {
        !value => file = value,
        error! => return (3 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (4 as process::ExitCode)!,
    }

    switch subdir.open_file(fs::Path::init("inside.txt"), fs::OpenOptions::read_only()) {
        !value => file = value,
        error! => return (5 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (6 as process::ExitCode)!,
    }

    switch subdir.close() {
        !ok => _ = ok,
        error! => return (7 as process::ExitCode)!,
    }

    switch cwd.open_dir(fs::Path::init("subdir/inside.txt"), fs::OpenDirOptions::init()) {
        !value => {
            _ = value;
            return (8 as process::ExitCode)!;
        },
        error! => {},
    }

    switch cwd.delete_file(fs::Path::init("subdir/inside.txt")) {
        !ok => _ = ok,
        error! => return (9 as process::ExitCode)!,
    }
    switch cwd.delete_dir(fs::Path::init("subdir")) {
        !ok => _ = ok,
        error! => return (10 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
    assert!(!data_path.exists());
}

#[test]
fn emit_exe_std_fs_can_iterate_dir_entries() {
    let root = temp_dir("emit_exe_std_fs_can_iterate_dir_entries");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.fs;
import std.mem;
import std.process;

fn bytes_equal(left: &[u8], right: &[u8]) bool {
    mem::equal[u8](left, right)
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => cwd = value,
        error! => return (1 as process::ExitCode)!,
    }

    switch cwd.create_dir(fs::Path::init("entries"), fs::CreateDirOptions::init()) {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }

    var first: fs::File;
    switch cwd.create_file(fs::Path::init("entries/alpha.txt"), fs::CreateOptions::init()) {
        !value => first = value,
        error! => return (3 as process::ExitCode)!,
    }
    switch first.close() {
        !ok => _ = ok,
        error! => return (4 as process::ExitCode)!,
    }

    var second: fs::File;
    switch cwd.create_file(fs::Path::init("entries/beta.txt"), fs::CreateOptions::init()) {
        !value => second = value,
        error! => return (5 as process::ExitCode)!,
    }
    switch second.close() {
        !ok => _ = ok,
        error! => return (6 as process::ExitCode)!,
    }

    var dir: fs::Dir;
    switch cwd.open_dir(fs::Path::init("entries"), fs::OpenDirOptions::init()) {
        !value => dir = value,
        error! => return (7 as process::ExitCode)!,
    }

    var buffer: [1024]u8 = [0; 1024];
    var iter: fs::DirIterator;
    switch dir.entries(&mut buffer[..]) {
        !value => iter = value,
        error! => return (8 as process::ExitCode)!,
    }

    var saw_alpha = false;
    var saw_beta = false;
    var count = 0usize;
    for result in iter {
        let value = switch result {
            !entry => entry,
            error! => return (10 as process::ExitCode)!,
        };
        if not value.is_dot() and not value.is_dot_dot() {
            count += 1usize;
            if value.kind() != fs::FileKind::File and value.kind() != fs::FileKind::Unknown {
                return (9 as process::ExitCode)!;
            }
            if bytes_equal(value.name(), b"alpha.txt") {
                saw_alpha = true;
            } else if bytes_equal(value.name(), b"beta.txt") {
                saw_beta = true;
            }
        }
    }

    if count != 2usize {
        return (11 as process::ExitCode)!;
    }
    if not saw_alpha or not saw_beta {
        return (12 as process::ExitCode)!;
    }

    switch dir.close() {
        !ok => _ = ok,
        error! => return (13 as process::ExitCode)!,
    }
    switch cwd.delete_file(fs::Path::init("entries/alpha.txt")) {
        !ok => _ = ok,
        error! => return (14 as process::ExitCode)!,
    }
    switch cwd.delete_file(fs::Path::init("entries/beta.txt")) {
        !ok => _ = ok,
        error! => return (15 as process::ExitCode)!,
    }
    switch cwd.delete_dir(fs::Path::init("entries")) {
        !ok => _ = ok,
        error! => return (16 as process::ExitCode)!,
    }
    switch cwd.close() {
        !ok => _ = ok,
        error! => return (17 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
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
        error! => return (5 as process::ExitCode)!,
    }
    switch allocator.alloc_bytes(4096, layout.align()) {
        !block => {
            var ptr = block.ptr();
            ptr.* = 42u8;
            if ptr.* != 42u8 {
                return (2 as process::ExitCode)!;
            }
            switch allocator.free(block) {
                !ok => _ = ok,
                error! => return (3 as process::ExitCode)!,
            }
        },
        error! => return (1 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
        error! => return (1 as process::ExitCode)!,
    }
    var block: mem::Block;
    switch allocator.alloc(layout) {
        !value => block = value,
        error! => return (2 as process::ExitCode)!,
    }
    if block.ptr() as usize % 8192usize != 0usize {
        return (3 as process::ExitCode)!;
    }
    var bytes = block.bytes();
    bytes[0] = 17u8;
    bytes[63] = 23u8;
    if bytes[0] != 17u8 or bytes[63] != 23u8 {
        return (4 as process::ExitCode)!;
    }
    switch allocator.free(block) {
        !ok => _ = ok,
        error! => return (5 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
            return (1 as process::ExitCode)!;
        },
        err! => {
            if err as i32 != mem::Error::InvalidAlignment as i32 {
                return (2 as process::ExitCode)!;
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
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
            return (1 as process::ExitCode)!;
        },
        err! => {
            if err as i32 != mem::Error::OutOfMemory as i32 {
                return (2 as process::ExitCode)!;
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
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
                return (2 as process::ExitCode)!;
            }
            if items[0] + items[1] + items[2] + items[3] != 100 {
                return (3 as process::ExitCode)!;
            }
            switch allocator.free_slice[i32](items) {
                !ok => _ = ok,
                error! => return (4 as process::ExitCode)!,
            }
        },
        error! => return (1 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_fixed_buffer_allocator_supports_array_list() {
    let root = temp_dir("emit_exe_std_mem_fixed_buffer_allocator_supports_array_list");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std;
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var storage: [256]u8 = [_]u8[0; 256];
    var allocator = mem::FixedBufferAllocator::init(&mut storage[..]);

    var list = std::ArrayList[i32]::init();
    list.push(&mut allocator, 10).exit().?;
    list.push(&mut allocator, 20).exit().?;
    list.push(&mut allocator, 30).exit().?;

    var total = 0;
    for &value in list.iter() {
        total += value;
    }
    if total != 60 {
        return (1 as process::ExitCode)!;
    }
    if allocator.used() == 0usize {
        return (2 as process::ExitCode)!;
    }

    list.deinit(&mut allocator).exit().?;
    allocator.reset();
    if allocator.used() != 0usize or allocator.remaining() != allocator.capacity() {
        return (3 as process::ExitCode)!;
    }

    var tiny: [8]u8 = [_]u8[0; 8];
    var failing = mem::FixedBufferAllocator::init(&mut tiny[..]);
    switch failing.alloc_bytes(16, 1) {
        !block => {
            _ = block;
            return (4 as process::ExitCode)!;
        },
        err! => {
            if err as i32 != mem::Error::OutOfMemory as i32 {
                return (5 as process::ExitCode)!;
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
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_fixed_buffer_allocator_resize_and_reset() {
    let root = temp_dir("emit_exe_std_mem_fixed_buffer_allocator_resize_and_reset");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var storage: [64]u8 = [_]u8[0; 64];
    var allocator = mem::FixedBufferAllocator::init(&mut storage[..]);

    let first_layout = mem::Layout::init(8, 8).exit().?;
    var first = allocator.alloc(first_layout).exit().?;
    if first.ptr() as usize % 8usize != 0usize {
        return (1 as process::ExitCode)!;
    }

    let second_layout = mem::Layout::init(8, 1).exit().?;
    var second = allocator.alloc(second_layout).exit().?;
    if allocator.resize(first, mem::Layout::init(16, 8).exit().?) {
        return (2 as process::ExitCode)!;
    }
    if not allocator.resize(first, mem::Layout::init(4, 8).exit().?) {
        return (3 as process::ExitCode)!;
    }

    switch allocator.remap(second, mem::Layout::init(24, 1).exit().?) {
        ?grown => second = grown,
        null => return (4 as process::ExitCode)!,
    }
    if second.size() != 24 {
        return (5 as process::ExitCode)!;
    }

    allocator.free(second).exit().?;
    switch allocator.alloc_bytes(48, 1) {
        !block => {
            allocator.free(block).exit().?;
        },
        error! => return (6 as process::ExitCode)!,
    }

    allocator.reset();
    if allocator.used() != 0usize or allocator.remaining() != 64usize {
        return (7 as process::ExitCode)!;
    }

    switch allocator.alloc_bytes(64, 1) {
        !block => {
            allocator.free(block).exit().?;
        },
        error! => return (8 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_arena_allocator_supports_array_list_and_retain_reset() {
    let root = temp_dir("emit_exe_std_mem_arena_allocator_supports_array_list_and_retain_reset");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std;
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var page = mem::PageAllocator::init();
    var arena = mem::ArenaAllocator::init(&mut page);
    defer arena.deinit().exit().?;

    var list = std::ArrayList[i32]::init();
    list.push(&mut arena, 10).exit().?;
    list.push(&mut arena, 20).exit().?;
    list.push(&mut arena, 30).exit().?;

    var total = 0;
    for &value in list.iter() {
        total += value;
    }
    if total != 60 {
        return (1 as process::ExitCode)!;
    }

    let capacity = arena.query_capacity();
    if capacity == 0usize or arena.query_used() == 0usize {
        return (2 as process::ExitCode)!;
    }

    arena.reset_retain_capacity().exit().?;
    if arena.query_capacity() != capacity or arena.query_used() != 0usize {
        return (3 as process::ExitCode)!;
    }

    var bytes = arena.alloc_slice[u8](64).exit().?;
    bytes[0] = 7u8;
    bytes[63] = 9u8;
    if bytes[0] != 7u8 or bytes[63] != 9u8 {
        return (4 as process::ExitCode)!;
    }

    arena.reset_retain_with_limit(0).exit().?;
    if arena.query_capacity() != 0usize or arena.query_used() != 0usize {
        return (5 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_arena_allocator_resize_remap_and_free_edges() {
    let root = temp_dir("emit_exe_std_mem_arena_allocator_resize_remap_and_free_edges");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var page = mem::PageAllocator::init();
    var arena = mem::ArenaAllocator::init(&mut page);
    defer arena.deinit().exit().?;

    var first = arena.alloc_bytes(16, 8).exit().?;
    var second = arena.alloc_bytes(16, 8).exit().?;
    if arena.resize(first, mem::Layout::init(32, 8).exit().?) {
        return (1 as process::ExitCode)!;
    }
    if not arena.resize(first, mem::Layout::init(8, 8).exit().?) {
        return (2 as process::ExitCode)!;
    }

    switch arena.remap(second, mem::Layout::init(40, 8).exit().?) {
        ?grown => second = grown,
        null => return (3 as process::ExitCode)!,
    }
    if second.size() != 40 {
        return (4 as process::ExitCode)!;
    }

    arena.free(second).exit().?;
    switch arena.alloc_bytes(40, 8) {
        !again => {
            if again.ptr() as usize != second.ptr() as usize {
                return (5 as process::ExitCode)!;
            }
        },
        error! => return (6 as process::ExitCode)!,
    }

    arena.reset().exit().?;
    if arena.query_capacity() != 0usize or arena.query_used() != 0usize {
        return (7 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_general_purpose_allocator_supports_small_allocations_and_array_list() {
    let root = temp_dir(
        "emit_exe_std_mem_general_purpose_allocator_supports_small_allocations_and_array_list",
    );
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std;
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var page = mem::PageAllocator::init();
    var allocator = mem::GeneralPurposeAllocator::init(&mut page);
    defer allocator.deinit().exit().?;

    let layout = mem::Layout::init(24, 8).exit().?;
    var first = allocator.alloc(layout).exit().?;
    var second = allocator.alloc(layout).exit().?;
    if first.ptr() as usize == second.ptr() as usize {
        return (1 as process::ExitCode)!;
    }

    let first_addr = first.ptr() as usize;
    allocator.free(first).exit().?;
    var reused = allocator.alloc(layout).exit().?;
    if reused.ptr() as usize != first_addr {
        return (2 as process::ExitCode)!;
    }

    allocator.free(reused).exit().?;
    allocator.free(second).exit().?;
    if not allocator.is_empty() {
        return (3 as process::ExitCode)!;
    }

    var list = std::ArrayList[i32]::init();
    defer list.deinit(&mut allocator).exit().?;
    list.push(&mut allocator, 10).exit().?;
    list.push(&mut allocator, 20).exit().?;
    list.push(&mut allocator, 30).exit().?;

    var total = 0;
    for &value in list.iter() {
        total += value;
    }
    if total != 60 {
        return (4 as process::ExitCode)!;
    }
    if allocator.query_used() == 0usize or allocator.query_capacity() == 0usize {
        return (5 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_general_purpose_allocator_supports_large_overaligned_realloc() {
    let root =
        temp_dir("emit_exe_std_mem_general_purpose_allocator_supports_large_overaligned_realloc");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var page = mem::PageAllocator::init();
    var allocator = mem::GeneralPurposeAllocator::init(&mut page);
    defer allocator.deinit().exit().?;

    let layout = mem::Layout::init(3000, 4096).exit().?;
    var block = allocator.alloc(layout).exit().?;
    if block.ptr() as usize % 4096usize != 0usize {
        return (1 as process::ExitCode)!;
    }
    var bytes = block.bytes();
    bytes[0] = 11u8;
    bytes[2999] = 22u8;

    let grown_layout = mem::Layout::init(3040, 4096).exit().?;
    let old_addr = block.ptr() as usize;
    block = allocator.realloc(block, grown_layout).exit().?;
    if block.ptr() as usize != old_addr or block.size() != 3040usize {
        return (2 as process::ExitCode)!;
    }
    bytes = block.bytes();
    if bytes[0] != 11u8 or bytes[2999] != 22u8 {
        return (3 as process::ExitCode)!;
    }

    let moved_layout = mem::Layout::init(12000, 4096).exit().?;
    block = allocator.realloc(block, moved_layout).exit().?;
    if block.ptr() as usize % 4096usize != 0usize or block.size() != 12000usize {
        return (4 as process::ExitCode)!;
    }
    bytes = block.bytes();
    if bytes[0] != 11u8 or bytes[2999] != 22u8 {
        return (5 as process::ExitCode)!;
    }

    allocator.free(block).exit().?;
    if not allocator.is_empty() {
        return (6 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_general_purpose_allocator_rejects_invalid_free_and_resize() {
    let root =
        temp_dir("emit_exe_std_mem_general_purpose_allocator_rejects_invalid_free_and_resize");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var page = mem::PageAllocator::init();
    var allocator = mem::GeneralPurposeAllocator::init(&mut page);
    defer allocator.deinit().exit().?;

    let small_layout = mem::Layout::init(32, 8).exit().?;
    let small = allocator.alloc(small_layout).exit().?;
    allocator.free(small).exit().?;
    switch allocator.free(small) {
        !ok => {
            _ = ok;
            return (1 as process::ExitCode)!;
        },
        err! => {
            if err as i32 != mem::Error::Invalid as i32 {
                return (2 as process::ExitCode)!;
            }
        },
    }
    if allocator.resize(small, small_layout) {
        return (3 as process::ExitCode)!;
    }

    var resized = allocator.alloc(small_layout).exit().?;
    let resized_layout = mem::Layout::init(40, 8).exit().?;
    if not allocator.resize(resized, resized_layout) {
        return (7 as process::ExitCode)!;
    }
    if allocator.query_used() != 40usize {
        return (8 as process::ExitCode)!;
    }
    switch allocator.free(resized) {
        !ok => {
            _ = ok;
            return (9 as process::ExitCode)!;
        },
        err! => {
            if err as i32 != mem::Error::Invalid as i32 {
                return (10 as process::ExitCode)!;
            }
        },
    }
    resized = mem::Block::init(resized.ptr(), resized_layout);
    allocator.free(resized).exit().?;

    let align_layout = mem::Layout::init(1, 1).exit().?;
    let align_block = allocator.alloc(align_layout).exit().?;
    if allocator.resize(align_block, mem::Layout::init(1, 2).exit().?) {
        return (11 as process::ExitCode)!;
    }
    allocator.free(align_block).exit().?;

    let large_layout = mem::Layout::init(4096, 4096).exit().?;
    let large = allocator.alloc(large_layout).exit().?;
    let wrong_layout = mem::Layout::init(2048, 4096).exit().?;
    let wrong = mem::Block::init(large.ptr(), wrong_layout);
    switch allocator.free(wrong) {
        !ok => {
            _ = ok;
            return (4 as process::ExitCode)!;
        },
        err! => {
            if err as i32 != mem::Error::Invalid as i32 {
                return (5 as process::ExitCode)!;
            }
        },
    }
    allocator.free(large).exit().?;
    if not allocator.is_empty() {
        return (6 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
        error! => return (1 as process::ExitCode)!,
    }
    var bytes = block.bytes();
    bytes[0] = 10u8;
    bytes[1] = 20u8;
    bytes[2] = 30u8;
    bytes[3] = 40u8;

    var grow_layout: mem::Layout;
    switch mem::Layout::init(8, 1) {
        !value => grow_layout = value,
        error! => return (2 as process::ExitCode)!,
    }
    switch allocator.realloc(block, grow_layout) {
        !value => block = value,
        error! => return (3 as process::ExitCode)!,
    }
    if block.size() != 8 {
        return (4 as process::ExitCode)!;
    }
    bytes = block.bytes();
    if bytes[0] != 10u8 or bytes[1] != 20u8 or bytes[2] != 30u8 or bytes[3] != 40u8 {
        return (5 as process::ExitCode)!;
    }
    bytes[4] = 50u8;
    bytes[5] = 60u8;

    var shrink_layout: mem::Layout;
    switch mem::Layout::init(2, 1) {
        !value => shrink_layout = value,
        error! => return (6 as process::ExitCode)!,
    }
    switch allocator.realloc(block, shrink_layout) {
        !value => block = value,
        error! => return (7 as process::ExitCode)!,
    }
    if block.size() != 2 {
        return (8 as process::ExitCode)!;
    }
    bytes = block.bytes();
    if bytes[0] != 10u8 or bytes[1] != 20u8 {
        return (9 as process::ExitCode)!;
    }

    switch allocator.free(block) {
        !ok => _ = ok,
        error! => return (10 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_allocator_realloc_frees_new_block_when_old_free_fails() {
    let root = temp_dir("emit_exe_std_mem_allocator_realloc_frees_new_block_when_old_free_fails");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std.mem;
import std.process;

struct CountingAllocator {
    buffer: &mut [u8],
    end_index: usize,
    free_count: usize,
}

extend CountingAllocator {
    fn init(buffer: &mut [u8]) CountingAllocator {
        { buffer: buffer, end_index: 0, free_count: 0 }
    }
}

extend CountingAllocator : mem::Allocator {
    fn alloc(&mut self, layout: mem::Layout) mem::Error!mem::Block {
        if layout.is_empty() {
            return !mem::Block::init(layout.align() as &mut u8, layout);
        }
        let base = self.buffer.get_ptr() as usize;
        let current = base + self.end_index;
        let aligned = switch current.align_forward(layout.align()) {
            ?value => value,
            null => return mem::Error::OutOfMemory!,
        };
        let offset = aligned - base;
        let next = switch offset.checked_add(layout.size()) {
            ?value => value,
            null => return mem::Error::OutOfMemory!,
        };
        if next > self.buffer.len() {
            return mem::Error::OutOfMemory!;
        }
        self.end_index = next;
        !mem::Block::init(aligned as &mut u8, layout)
    }

    fn free(&mut self, block: mem::Block) mem::Error!void {
        if block.is_empty() {
            return !{};
        }
        self.free_count += 1usize;
        if self.free_count == 1usize {
            return mem::Error::Invalid!;
        }
        !{}
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var storage: [64]u8 = [_]u8[0; 64];
    var allocator = CountingAllocator::init(storage);
    let old_layout = mem::Layout::init(4, 1).exit().?;
    let new_layout = mem::Layout::init(8, 1).exit().?;
    var block = allocator.alloc(old_layout).exit().?;
    var bytes = block.bytes();
    bytes[0] = 10u8;
    bytes[1] = 20u8;

    switch allocator.realloc(block, new_layout) {
        !new_block => {
            _ = new_block;
            return (1 as process::ExitCode)!;
        },
        err! => {
            if err as i32 != mem::Error::Invalid as i32 {
                return (2 as process::ExitCode)!;
            }
        },
    }

    if allocator.free_count != 2usize {
        return (3 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
        error! => return (1 as process::ExitCode)!,
    }
    var block: mem::Block;
    switch allocator.alloc(layout) {
        !value => block = value,
        error! => return (2 as process::ExitCode)!,
    }
    if not allocator.resize(block, layout) {
        return (3 as process::ExitCode)!;
    }

    var larger: mem::Layout;
    switch mem::Layout::init(32, 8) {
        !value => larger = value,
        error! => return (4 as process::ExitCode)!,
    }
    if not allocator.resize(block, larger) {
        return (5 as process::ExitCode)!;
    }
    switch allocator.remap(block, larger) {
        ?same => {
            if same.ptr() as usize != block.ptr() as usize or same.size() != 32 {
                return (6 as process::ExitCode)!;
            }
            block = same;
        },
        null => return (7 as process::ExitCode)!,
    }
    switch allocator.remap(block, layout) {
        ?same => {
            if same.ptr() as usize != block.ptr() as usize or same.size() != 16 {
                return (8 as process::ExitCode)!;
            }
            block = same;
        },
        null => return (9 as process::ExitCode)!,
    }

    var next_page: mem::Layout;
    switch mem::Layout::init(8192, 8) {
        !value => next_page = value,
        error! => return (10 as process::ExitCode)!,
    }
    if allocator.resize(block, next_page) {
        return (11 as process::ExitCode)!;
    }
    switch allocator.remap(block, next_page) {
        ?moved => {
            _ = moved;
            return (12 as process::ExitCode)!;
        },
        null => {},
    }
    switch allocator.free(block) {
        !ok => _ = ok,
        error! => return (13 as process::ExitCode)!,
    }

    var empty_a: mem::Layout;
    switch mem::Layout::init(0, 8) {
        !value => empty_a = value,
        error! => return (14 as process::ExitCode)!,
    }
    switch allocator.alloc(empty_a) {
        !value => block = value,
        error! => return (15 as process::ExitCode)!,
    }
    var empty_b: mem::Layout;
    switch mem::Layout::init(0, 16) {
        !value => empty_b = value,
        error! => return (16 as process::ExitCode)!,
    }
    if allocator.resize(block, empty_b) {
        return (17 as process::ExitCode)!;
    }
    switch allocator.remap(block, empty_b) {
        ?moved => {
            if moved.size() != 0 or moved.align() != 16 {
                return (18 as process::ExitCode)!;
            }
        },
        null => return (19 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
        error! => return (1 as process::ExitCode)!,
    }
    var block: mem::Block;
    switch allocator.alloc(empty_layout) {
        !value => block = value,
        error! => return (2 as process::ExitCode)!,
    }
    if block.size() != 0 {
        return (3 as process::ExitCode)!;
    }

    var full_layout: mem::Layout;
    switch mem::Layout::init(16, 8) {
        !value => full_layout = value,
        error! => return (4 as process::ExitCode)!,
    }
    switch allocator.realloc(block, full_layout) {
        !value => block = value,
        error! => return (5 as process::ExitCode)!,
    }
    if block.size() != 16 or block.align() != 8 {
        return (6 as process::ExitCode)!;
    }
    var bytes = block.bytes();
    bytes[0] = 77u8;
    bytes[15] = 99u8;
    if bytes[0] != 77u8 or bytes[15] != 99u8 {
        return (7 as process::ExitCode)!;
    }

    switch allocator.free(block) {
        !ok => _ = ok,
        error! => return (8 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
                return (2 as process::ExitCode)!;
            }
            switch allocator.free_slice[i32](items) {
                !ok => _ = ok,
                error! => return (3 as process::ExitCode)!,
            }
        },
        error! => return (1 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
                return (2 as process::ExitCode)!;
            }
            switch allocator.free_slice[void](items) {
                !ok => _ = ok,
                error! => return (3 as process::ExitCode)!,
            }
        },
        error! => return (1 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_mem_block_as_slice_handles_zero_sized_element_type() {
    let root = temp_dir("emit_exe_std_mem_block_as_slice_handles_zero_sized_element_type");
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
    switch mem::Layout::array[void](8) {
        !value => layout = value,
        error! => return (1 as process::ExitCode)!,
    }
    var block: mem::Block;
    switch allocator.alloc(layout) {
        !value => block = value,
        error! => return (2 as process::ExitCode)!,
    }
    var items = block.as_slice[void]();
    if items.len() != 0 {
        return (3 as process::ExitCode)!;
    }
    switch allocator.free(block) {
        !ok => _ = ok,
        error! => return (4 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
        return (1 as process::ExitCode)!;
    }

    var right: [5]i32 = [1, 2, 3, 4, 5];
    mem::copy_backwards[i32](&mut right[1..4], &right[0..3]);
    let expected_right: [5]i32 = [1, 1, 2, 3, 5];
    if not mem::equal[i32](&right[..], &expected_right[..]) {
        return (2 as process::ExitCode)!;
    }

    var exact_to: [3]u8 = [0, 0, 0];
    let exact_from: [3]u8 = [7, 8, 9];
    mem::copy_forwards[u8](&mut exact_to[..], &exact_from[..]);
    if not mem::equal[u8](&exact_to[..], &exact_from[..]) {
        return (3 as process::ExitCode)!;
    }

    var short_to: [2]u8 = [0, 0];
    let long_from: [4]u8 = [5, 6, 7, 8];
    mem::copy_forwards[u8](&mut short_to[..], &long_from[..]);
    let expected_short_to: [2]u8 = [5, 6];
    if not mem::equal[u8](&short_to[..], &expected_short_to[..]) {
        return (8 as process::ExitCode)!;
    }

    var short_backward: [2]u8 = [0, 0];
    mem::copy_backwards[u8](&mut short_backward[..], &long_from[..]);
    if not mem::equal[u8](&short_backward[..], &expected_short_to[..]) {
        return (9 as process::ExitCode)!;
    }

    let low: [2]u8 = [1, 2];
    let high: [2]u8 = [1, 3];
    if mem::order[u8](&low[..], &high[..]) != mem::Order::Less {
        return (4 as process::ExitCode)!;
    }
    if mem::order[u8](&high[..], &low[..]) != mem::Order::Greater {
        return (5 as process::ExitCode)!;
    }
    if mem::order[u8](&low[..], &low[..]) != mem::Order::Equal {
        return (6 as process::ExitCode)!;
    }
    let prefix: [1]u8 = [1];
    if mem::order[u8](&prefix[..], &low[..]) != mem::Order::Less {
        return (7 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
        return (1 as process::ExitCode)!;
    }

    var wide: [5]i32 = [0, 0, 0, 44, 55];
    let short: [3]i32 = [11, 22, 33];
    @memcpy(&mut wide[..], &short[..]);
    if wide[0] != 11 or wide[1] != 22 or wide[2] != 33 or wide[3] != 44 or wide[4] != 55 {
        return (4 as process::ExitCode)!;
    }

    var narrow: [4]u8 = [0, 0, 77, 88];
    let long: [4]u8 = [10, 20, 30, 40];
    @memcpy(&mut narrow[0..2], &long[..]);
    if narrow[0] != 10 or narrow[1] != 20 or narrow[2] != 77 or narrow[3] != 88 {
        return (5 as process::ExitCode)!;
    }

    var overlap: [5]u8 = [1, 2, 3, 4, 5];
    @memmove(&mut overlap[1..], &overlap[0..4]);
    if overlap[0] != 1 or overlap[1] != 1 or overlap[2] != 2 or overlap[3] != 3 or overlap[4] != 4 {
        return (2 as process::ExitCode)!;
    }

    var short_move: [4]u8 = [9, 8, 7, 6];
    @memmove(&mut short_move[0..2], &short_move[1..4]);
    if short_move[0] != 8 or short_move[1] != 7 or short_move[2] != 7 or short_move[3] != 6 {
        return (6 as process::ExitCode)!;
    }

    var bytes: [4]u8 = [1, 2, 3, 4];
    @memset(&mut bytes[1..3], 9);
    if bytes[0] != 1 or bytes[1] != 9 or bytes[2] != 9 or bytes[3] != 4 {
        return (3 as process::ExitCode)!;
    }

    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
        return (1 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write main source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-M")
        .arg(format!("helper={}", helper.display()))
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_cross_module_slice_pointee_extension_receiver_passes_slice_value() {
    let root =
        temp_dir("emit_exe_cross_module_slice_pointee_extension_receiver_passes_slice_value");
    let main = root.join("main.nia");
    let helper = root.join("helper.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &helper,
        r#"
pub struct SliceIter[T]
where T: Sized
{
    ptr: &T,
    len: usize,
    index: usize,
}

extend[T] SliceIter[T]
where T: Sized
{
    pub fn from_raw_parts(ptr: &T, len: usize) SliceIter[T] {
        { ptr: ptr, len: len, index: 0 }
    }
}

extend[T] SliceIter[T] : Iterator
where T: Sized
{
    type Item = &T;

    fn next(&mut self) ?&T {
        if self.index >= self.len {
            null
        } else {
            let item = (self.ptr as usize + self.index * @size[T]()) as &T;
            self.index += 1usize;
            ?item
        }
    }
}

extend[T] [T]
where T: Sized
{
    pub fn iter(&self) SliceIter[T] {
        SliceIter[T]::from_raw_parts(self.get_ptr_read(), self.len())
    }
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
    var data: [3]i32 = [10, 20, 30];
    var total = 0;
    for &value in (&data[..]).iter() {
        total += value;
    }
    if total != 60 {
        return (1 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write main source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-M")
        .arg(format!("helper={}", helper.display()))
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
import std;
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    let page = &mut allocator;
    var exact: std::ArrayList[i32];
    switch std::ArrayList[i32]::init_capacity(page, 3) {
        !value => exact = value,
        error! => return (1 as process::ExitCode)!,
    }
    if exact.len() != 0 or exact.capacity() != 3 {
        return (2 as process::ExitCode)!;
    }
    switch exact.deinit(page) {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }

    var ops = std::ArrayList[i32]::init();
    switch ops.push(page, 1) {
        !ok => _ = ok,
        error! => return (26 as process::ExitCode)!,
    }
    switch ops.push(page, 3) {
        !ok => _ = ok,
        error! => return (27 as process::ExitCode)!,
    }
    switch ops.insert(page, 1, 2) {
        !ok => _ = ok,
        error! => return (28 as process::ExitCode)!,
    }
    let inserted_tail: [2]i32 = [4, 5];
    switch ops.insert_slice(page, 3, &inserted_tail[..]) {
        !ok => _ = ok,
        error! => return (29 as process::ExitCode)!,
    }
    let expected_ops: [5]i32 = [1, 2, 3, 4, 5];
    if not mem::equal[i32](ops.as_slice(), &expected_ops[..]) {
        return (30 as process::ExitCode)!;
    }
    switch ops.ordered_remove(1) {
        ?value => {
            if value != 2 {
                return (31 as process::ExitCode)!;
            }
        },
        null => return (32 as process::ExitCode)!,
    }
    let expected_ordered: [4]i32 = [1, 3, 4, 5];
    if not mem::equal[i32](ops.as_slice(), &expected_ordered[..]) {
        return (33 as process::ExitCode)!;
    }
    switch ops.swap_remove(0) {
        ?value => {
            if value != 1 {
                return (34 as process::ExitCode)!;
            }
        },
        null => return (35 as process::ExitCode)!,
    }
    let expected_swap: [3]i32 = [5, 3, 4];
    if not mem::equal[i32](ops.as_slice(), &expected_swap[..]) {
        return (36 as process::ExitCode)!;
    }
    switch ops.deinit(page) {
        !ok => _ = ok,
        error! => return (37 as process::ExitCode)!,
    }

    var alias = std::ArrayList[i32]::init();
    switch alias.reserve_exact(page, 2) {
        !ok => _ = ok,
        error! => return (38 as process::ExitCode)!,
    }
    switch alias.push(page, 1) {
        !ok => _ = ok,
        error! => return (39 as process::ExitCode)!,
    }
    switch alias.push(page, 2) {
        !ok => _ = ok,
        error! => return (40 as process::ExitCode)!,
    }
    switch alias.append_slice(page, alias.as_slice()) {
        !ok => _ = ok,
        error! => return (41 as process::ExitCode)!,
    }
    let expected_alias_append: [4]i32 = [1, 2, 1, 2];
    if not mem::equal[i32](alias.as_slice(), &expected_alias_append[..]) {
        return (42 as process::ExitCode)!;
    }
    switch alias.insert_slice(page, 1, alias.as_slice()) {
        !ok => _ = ok,
        error! => return (43 as process::ExitCode)!,
    }
    let expected_alias_insert: [8]i32 = [1, 1, 2, 1, 2, 2, 1, 2];
    if not mem::equal[i32](alias.as_slice(), &expected_alias_insert[..]) {
        return (44 as process::ExitCode)!;
    }
    switch alias.deinit(page) {
        !ok => _ = ok,
        error! => return (45 as process::ExitCode)!,
    }

    var list = std::ArrayList[i32]::init();
    if list.len() != 0 or not list.is_empty() {
        return (4 as process::ExitCode)!;
    }
    switch list.reserve_exact(page, 2) {
        !ok => _ = ok,
        error! => return (5 as process::ExitCode)!,
    }
    if list.capacity() != 2 {
        return (6 as process::ExitCode)!;
    }
    switch list.reserve(page, 3) {
        !ok => _ = ok,
        error! => return (7 as process::ExitCode)!,
    }
    if list.capacity() < 5 {
        return (8 as process::ExitCode)!;
    }
    var index = 0;
    while index < 6 {
        switch list.push(page, index * 10) {
            !ok => _ = ok,
            error! => return (9 as process::ExitCode)!,
        }
        index += 1;
    }
    if list.len() != 6 or list.capacity() < 6 {
        return (10 as process::ExitCode)!;
    }
    let items = list.as_slice();
    if items[0] != 0 or items[1] != 10 or items[5] != 50 {
        return (11 as process::ExitCode)!;
    }

    let more: [3]i32 = [60, 70, 80];
    switch list.append_slice(page, &more[..]) {
        !ok => _ = ok,
        error! => return (12 as process::ExitCode)!,
    }
    if list.len() != 9 or list.as_slice()[8] != 80 {
        return (13 as process::ExitCode)!;
    }

    switch list.add_one(page) {
        !slot => slot.* = 90,
        error! => return (14 as process::ExitCode)!,
    }
    if list.len() != 10 or list.as_slice()[9] != 90 {
        return (15 as process::ExitCode)!;
    }

    switch list.add_many_as_slice(page, 2) {
        !slots => {
            slots[0] = 100;
            slots[1] = 110;
        },
        error! => return (16 as process::ExitCode)!,
    }
    if list.len() != 12 or list.as_slice()[11] != 110 {
        return (17 as process::ExitCode)!;
    }

    switch list.add_many_at(page, 2, 2) {
        !slots => {
            slots[0] = 21;
            slots[1] = 22;
        },
        error! => return (46 as process::ExitCode)!,
    }
    if list.len() != 14 or list.as_slice()[2] != 21 or list.as_slice()[3] != 22 or list.as_slice()[4] != 20 {
        return (47 as process::ExitCode)!;
    }

    list.append_assume_capacity(120);
    if list.len() != 15 or list.as_slice()[14] != 120 {
        return (48 as process::ExitCode)!;
    }

    switch list.resize(page, 18) {
        !ok => _ = ok,
        error! => return (49 as process::ExitCode)!,
    }
    if list.len() != 18 {
        return (50 as process::ExitCode)!;
    }
    var unused = list.unused_capacity_slice();
    if unused.len() < 2 {
        return (51 as process::ExitCode)!;
    }
    unused[0] = 180;
    unused[1] = 190;
    switch list.add_many_as_slice(page, 2) {
        !slots => {
            if slots[0] != 180 or slots[1] != 190 {
                return (52 as process::ExitCode)!;
            }
        },
        error! => return (53 as process::ExitCode)!,
    }
    if list.len() != 20 {
        return (54 as process::ExitCode)!;
    }

    switch list.resize(page, 12) {
        !ok => _ = ok,
        error! => return (55 as process::ExitCode)!,
    }
    if list.len() != 12 {
        return (56 as process::ExitCode)!;
    }

    let before_shrink_capacity = list.capacity();
    switch list.shrink_to_len(page) {
        !ok => _ = ok,
        error! => return (57 as process::ExitCode)!,
    }
    if list.len() != 12 or list.capacity() > before_shrink_capacity or list.capacity() < list.len() {
        return (58 as process::ExitCode)!;
    }

    switch list.reserve_exact(page, 4) {
        !ok => _ = ok,
        error! => return (59 as process::ExitCode)!,
    }
    list.expand_to_capacity();
    if list.len() != list.capacity() {
        return (60 as process::ExitCode)!;
    }

    switch list.shrink_and_free(page, 10) {
        !ok => _ = ok,
        error! => return (61 as process::ExitCode)!,
    }
    if list.len() != 10 or list.capacity() < 10 {
        return (62 as process::ExitCode)!;
    }

    let allocated = list.allocated_slice();
    if allocated.len() != list.capacity() {
        return (63 as process::ExitCode)!;
    }

    let retained_capacity = list.capacity();
    list.shrink_retaining_capacity(10);
    if list.len() != 10 or list.capacity() != retained_capacity {
        return (18 as process::ExitCode)!;
    }

    let tail: [2]i32 = [100, 110];
    list.append_slice_assume_capacity(&tail[..]);
    if list.len() != 12 or list.as_slice()[10] != 100 or list.as_slice()[11] != 110 {
        return (19 as process::ExitCode)!;
    }

    switch list.pop() {
        ?value => {
            if value != 110 {
                return (20 as process::ExitCode)!;
            }
        },
        null => return (21 as process::ExitCode)!,
    }
    if list.len() != 11 {
        return (22 as process::ExitCode)!;
    }
    var mutable_items = list.as_mut_slice();
    mutable_items[2] = 77;
    if list.as_slice()[2] != 77 {
        return (23 as process::ExitCode)!;
    }
    list.clear_retaining_capacity();
    if not list.is_empty() {
        return (24 as process::ExitCode)!;
    }
    switch list.clear_and_free(page) {
        !ok => _ = ok,
        error! => return (25 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_array_list_can_shrink_to_zero_capacity_and_reuse() {
    let root = temp_dir("emit_exe_std_array_list_can_shrink_to_zero_capacity_and_reuse");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std;
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    let page = &mut allocator;
    var list = std::ArrayList[i32]::init();
    switch list.push(page, 10) {
        !ok => _ = ok,
        error! => return (1 as process::ExitCode)!,
    }
    switch list.push(page, 20) {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }
    switch list.shrink_to_capacity(page, 0) {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }
    if list.len() != 0 or list.capacity() != 0 {
        return (4 as process::ExitCode)!;
    }

    switch list.push(page, 30) {
        !ok => _ = ok,
        error! => return (5 as process::ExitCode)!,
    }
    switch list.push(page, 40) {
        !ok => _ = ok,
        error! => return (6 as process::ExitCode)!,
    }
    let expected: [2]i32 = [30, 40];
    if not mem::equal[i32](list.as_slice(), &expected[..]) {
        return (7 as process::ExitCode)!;
    }
    switch list.deinit(page) {
        !ok => _ = ok,
        error! => return (8 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_array_list_owned_slice_and_clone() {
    let root = temp_dir("emit_exe_std_array_list_owned_slice_and_clone");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std;
import std.mem;
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    let page = &mut allocator;

    var source = std::ArrayList[i32]::init();
    switch source.push(page, 1) {
        !ok => _ = ok,
        error! => return (1 as process::ExitCode)!,
    }
    switch source.push(page, 2) {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }

    var cloned: std::ArrayList[i32];
    switch source.clone(page) {
        !value => cloned = value,
        error! => return (3 as process::ExitCode)!,
    }
    var source_items = source.as_mut_slice();
    source_items[0] = 9;
    let expected_source: [2]i32 = [9, 2];
    let expected_clone: [2]i32 = [1, 2];
    if not mem::equal[i32](source.as_slice(), &expected_source[..]) {
        return (4 as process::ExitCode)!;
    }
    if not mem::equal[i32](cloned.as_slice(), &expected_clone[..]) {
        return (5 as process::ExitCode)!;
    }

    var owned: &mut [i32];
    switch source.into_owned_slice(page) {
        !value => owned = value,
        error! => return (6 as process::ExitCode)!,
    }
    if source.len() != 0 or source.capacity() != 0 {
        return (7 as process::ExitCode)!;
    }
    if not mem::equal[i32](owned, &expected_source[..]) {
        return (8 as process::ExitCode)!;
    }
    switch page.free_slice[i32](owned) {
        !ok => _ = ok,
        error! => return (9 as process::ExitCode)!,
    }

    var external: &mut [i32];
    switch page.alloc_slice[i32](3) {
        !items => external = items,
        error! => return (10 as process::ExitCode)!,
    }
    external[0] = 4;
    external[1] = 5;
    external[2] = 6;
    var adopted = std::ArrayList[i32]::from_owned_slice(external);
    let expected_adopted: [3]i32 = [4, 5, 6];
    if adopted.capacity() != 3 or not mem::equal[i32](adopted.as_slice(), &expected_adopted[..]) {
        return (11 as process::ExitCode)!;
    }
    switch adopted.deinit(page) {
        !ok => _ = ok,
        error! => return (12 as process::ExitCode)!,
    }
    switch cloned.deinit(page) {
        !ok => _ = ok,
        error! => return (13 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_array_list_handles_zero_sized_elements_without_allocation() {
    let root = temp_dir("emit_exe_std_array_list_handles_zero_sized_elements_without_allocation");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
import std;
import std.mem;
import std.process;

struct Marker {}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var allocator = mem::PageAllocator::init();
    let page = &mut allocator;
    var list = std::ArrayList[Marker]::init();
    switch list.reserve(page, 4) {
        !ok => _ = ok,
        error! => return (1 as process::ExitCode)!,
    }
    if list.capacity() != usize::MAX {
        return (2 as process::ExitCode)!;
    }
    switch list.push(page, {}) {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }
    switch list.resize(page, 16) {
        !ok => _ = ok,
        error! => return (4 as process::ExitCode)!,
    }
    if list.len() != 16 or list.capacity() != usize::MAX {
        return (5 as process::ExitCode)!;
    }
    switch list.shrink_and_free(page, 3) {
        !ok => _ = ok,
        error! => return (6 as process::ExitCode)!,
    }
    if list.len() != 3 or list.capacity() != usize::MAX {
        return (7 as process::ExitCode)!;
    }
    switch list.deinit(page) {
        !ok => _ = ok,
        error! => return (8 as process::ExitCode)!,
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
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
    (7 as process::ExitCode)!
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .output_timeout("run nia emit --exe");

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
        .arg("--exe")
        .arg(&main)
        .arg("-M")
        .arg(format!("std={}", std_root.display()))
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe with custom std start");

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
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    (9 as process::ExitCode)!
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .current_dir(&root)
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe_name)
        .output_timeout("run nia emit --exe -o -Orunnable");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(9));

    let report_name = format!("--opt-report{}", std::env::consts::EXE_SUFFIX);
    let report_path = root.join(&report_name);
    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .current_dir(&root)
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&report_name)
        .output_timeout("run nia emit --exe -o --opt-report");

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
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    (5 as process::ExitCode)!
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("-Oz")
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .arg("--opt-report")
        .output_timeout("run nia emit --exe --opt-report");

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
    (pick(true, plus_two(y), unused) as process::ExitCode)!
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
        let output_context = format!("run nia {level} emit --exe");
        let output = Command::new(env!("CARGO_BIN_EXE_nia"))
            .arg(level)
            .arg("emit")
            .arg("--exe")
            .arg(&main)
            .arg("-o")
            .arg(&exe)
            .output_timeout(&output_context);

        assert!(
            output.status.success(),
            "{level} stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );

        let run_context = format!("run emitted executable for {level}");
        let status = Command::new(&exe).status_timeout(&run_context);
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
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}
