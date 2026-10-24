// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use nia_test_support::{CommandExt, CommandStatusExt, test_dir as temp_dir};

#[test]
fn emit_llvm_retains_reachable_imports_from_a_declarations_only_module() {
    let root = temp_dir("emit_llvm_declarations_only_module");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module native;
using root::native;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    process::ExitCode(native::readValue())!
}
"#,
    )
    .expect("write caller");
    std::fs::write(
        root.join("native.nia"),
        r#"
@[linkName("nia_test_external_value")]
pub extern fun readValue() i32;
pub extern fun nia_test_unused_import() i32;
"#,
    )
    .expect("write external declarations");

    for runtime in ["bare", "freestanding"] {
        let output = support::nia_command()
            .args(["emit", "--llvm", "--runtime", runtime])
            .arg(&main)
            .output_timeout_for_compiler("emit external declaration module");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let llvm = String::from_utf8_lossy(&output.stdout);
        assert!(llvm.contains("@nia_test_external_value("), "{llvm}");
        assert!(!llvm.contains("@nia_test_unused_import("), "{llvm}");
    }
}

#[test]
fn emit_exe_reports_private_entry_main_called_by_freestanding_start() {
    let root = temp_dir("emit_exe_reports_private_entry_main_called_by_freestanding_start");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
using std::process;

fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    process::ExitCode(7)!
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
    assert!(stderr.contains("program::main"), "{stderr}");
}

#[test]
fn emit_exe_entry_name_is_chosen_by_selected_runtime_not_compiler() {
    let root = temp_dir("emit_exe_entry_name_is_chosen_by_selected_runtime_not_compiler");
    let main = root.join("main.nia");
    let resource_root = root.join("custom_toolchain/lib");
    let std_root = resource_root.join("std/root.nia");
    let std_builtin = resource_root.join("std/builtin.nia");
    let runtime_root = resource_root.join("runtime/root.nia");
    let std_start = resource_root.join("runtime/start.nia");
    let std_start_freestanding = resource_root.join("runtime/start/freestanding.nia");
    let std_start_freestanding_linux = resource_root.join("runtime/start/freestanding/linux.nia");
    let std_start_linux_x86_64 = resource_root.join("runtime/start/freestanding/linux/x86_64.nia");
    let std_start_linux_x86 = resource_root.join("runtime/start/freestanding/linux/x86.nia");
    let std_start_freestanding_windows =
        resource_root.join("runtime/start/freestanding/windows.nia");
    let std_start_freestanding_macos = resource_root.join("runtime/start/freestanding/macos.nia");
    let std_start_windows_x86_64 =
        resource_root.join("runtime/start/freestanding/windows/x86_64.nia");
    let std_start_macos_arch = resource_root.join(format!(
        "runtime/start/freestanding/macos/{}.nia",
        if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "x86_64"
        }
    ));
    let windows_builtins = resource_root.join("runtime/builtins/windows/x86_64.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::create_dir_all(std_start_linux_x86_64.parent().expect("std start parent"))
        .expect("create custom runtime dir");
    std::fs::create_dir_all(
        std_start_windows_x86_64
            .parent()
            .expect("std Windows start parent"),
    )
    .expect("create custom Windows runtime dir");
    std::fs::create_dir_all(
        std_start_macos_arch
            .parent()
            .expect("std macOS start parent"),
    )
    .expect("create custom macOS runtime dir");
    std::fs::create_dir_all(windows_builtins.parent().expect("Windows builtins parent"))
        .expect("create custom Windows builtins dir");
    std::fs::create_dir_all(std_builtin.parent().expect("custom std builtin parent"))
        .expect("create custom std library dir");
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("nia-cli lives under crates/");
    std::fs::copy(
        workspace_root.join("lib/toolchain.meta"),
        resource_root.join("toolchain.meta"),
    )
    .expect("copy toolchain manifest");
    for package in ["std", "runtime"] {
        std::fs::copy(
            workspace_root.join("lib").join(package).join("nia.toml"),
            resource_root.join(package).join("nia.toml"),
        )
        .expect("copy toolchain package manifest");
    }
    // Windows links need the toolchain's system import descriptions.
    let imports = resource_root.join("imports/windows");
    std::fs::create_dir_all(&imports).expect("create custom import descriptions");
    for library in ["kernel32.def", "bcryptprimitives.def"] {
        std::fs::copy(
            workspace_root.join("lib/imports/windows").join(library),
            imports.join(library),
        )
        .expect("copy import description");
    }
    let imports = resource_root.join("imports/macos");
    std::fs::create_dir_all(&imports).expect("create custom Darwin imports");
    std::fs::copy(
        workspace_root.join("lib/imports/macos/libSystem.tbd"),
        imports.join("libSystem.tbd"),
    )
    .expect("copy Darwin import description");
    std::fs::write(
        &std_root,
        r#"
pub module builtin;
"#,
    )
    .expect("write custom std root");
    std::fs::write(
        &runtime_root,
        "pub(root) module start; pub(root) module builtins;",
    )
    .expect("write custom runtime root");
    std::fs::write(
        resource_root.join("runtime/builtins.nia"),
        "pub(root) module windows;",
    )
    .expect("write builtins facade");
    std::fs::write(
        resource_root.join("runtime/builtins/windows.nia"),
        "pub(root) module x86_64;",
    )
    .expect("write Windows builtins facade");
    std::fs::copy(
        workspace_root.join("lib/runtime/builtins/windows/x86_64.nia"),
        &windows_builtins,
    )
    .expect("copy Windows stack probe");
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
pub fun asm(config: AsmConfig) ();
"#,
    )
    .expect("write custom std builtin");
    std::fs::write(
        &std_start,
        r#"
@[if os == "linux" and (arch == "x86_64" or arch == "x86")]
pub(root) module freestanding;
@[if os == "windows" and arch == "x86_64"]
pub(root) module freestanding;
@[if os == "macos" and (arch == "x86_64" or arch == "aarch64")]
pub(root) module freestanding;
@[if os == "linux" and arch == "x86_64"]
using root::start::freestanding::linux::x86_64;
@[if os == "linux" and arch == "x86"]
using root::start::freestanding::linux::x86;
@[if os == "windows" and arch == "x86_64"]
using root::start::freestanding::windows::x86_64;
@[if os == "macos"]
using root::start::freestanding::macos;
"#,
    )
    .expect("write custom std start facade");
    std::fs::write(
        &std_start_freestanding,
        r#"
@[if os == "linux"]
pub(root) module linux;
@[if os == "windows"]
pub(root) module windows;
@[if os == "macos"]
pub(root) module macos;
"#,
    )
    .expect("write custom std freestanding facade");
    std::fs::write(
        &std_start_freestanding_windows,
        "pub(root) module startup;\n@[if arch == \"x86_64\"]\npub(root) module x86_64;\n",
    )
    .expect("write custom std Windows freestanding facade");
    std::fs::write(
        &std_start_freestanding_macos,
        "pub(root) module startup;\n@[if arch == \"x86_64\"]\npub(root) module x86_64;\n@[if arch == \"aarch64\"]\npub(root) module aarch64;\n",
    )
    .expect("write custom Darwin freestanding facade");
    std::fs::write(
        &std_start_freestanding_linux,
        r#"
pub(root) module startup;
@[if arch == "x86_64"]
pub(root) module x86_64;
@[if arch == "x86"]
pub(root) module x86;
"#,
    )
    .expect("write custom std linux facade");
    // The runtime roots Linux startup in `startup.nia` by definition name.
    std::fs::write(
        std_start_freestanding_linux
            .with_extension("")
            .join("startup.nia"),
        r#"
using program;

@[if arch == "x86_64"]
fun syscallExit(code: i32) () {
    std::builtin::asm(.{
        code: b"syscall",
        inputs: .{
            rax: 60,
            rdi: code,
        },
        clobbers: [b"rcx", b"r11", b"memory"],
        options: [b"volatile"],
    });
}

@[if arch == "x86"]
fun syscallExit(code: i32) () {
    std::builtin::asm(.{
        code: b"int 0x80",
        inputs: .{
            eax: 1,
            ebx: code,
        },
        clobbers: [b"memory"],
        options: [b"volatile"],
    });
}

extern fun niaStartStack() () {
    syscallExit(program::mymain());
    loop {}
}
"#,
    )
    .expect("write custom Linux startup");
    std::fs::write(
        &std_start_linux_x86_64,
        r#"
@[naked]
pub extern fun _start() () {
    std::builtin::asm(.{
        code:
            b"call niaStartStack\n"
            b"ud2",
        clobbers: [b"rax", b"rcx", b"r11", b"memory"],
        options: [b"volatile"],
    });
    loop {}
}
"#,
    )
    .expect("write custom std start");
    std::fs::write(
        &std_start_linux_x86,
        r#"
@[naked]
pub extern fun _start() () {
    std::builtin::asm(.{
        code:
            b"call niaStartStack\n"
            b"ud2",
        clobbers: [b"eax", b"ecx", b"edx", b"memory"],
        options: [b"volatile"],
    });
    loop {}
}
"#,
    )
    .expect("write custom i686 std start");
    std::fs::write(
        &std_start_windows_x86_64,
        "using root::builtins::windows::x86_64::__chkstk;\n\n@[naked]\npub extern fun _start() () {\n    std::builtin::asm(.{ code: b\"sub rsp, 40\\ncall niaWindowsStart\\nud2\", clobbers: [b\"memory\"], options: [b\"volatile\"] });\n    loop {}\n}\n",
    )
    .expect("write custom Windows std start");
    // The runtime roots Windows startup in `startup.nia` by definition name.
    std::fs::write(
        std_start_windows_x86_64.with_file_name("startup.nia"),
        "using program;\n\nextern fun ExitProcess(code: u32) ();\n\nextern fun niaWindowsStart() () { ExitProcess(11u32); loop {} }\n",
    )
    .expect("write custom Windows startup");
    std::fs::write(
        std_start_macos_arch.with_file_name("startup.nia"),
        "using program;\n\nextern fun exit(code: i32) ();\n\npub(root) extern fun niaDarwinStart(argc: i32, argv: &&u8, envp: &&u8) () { _ = argc; _ = argv; _ = envp; exit(program::mymain()); loop {} }\n",
    )
    .expect("write custom Darwin startup");
    for arch in ["x86_64", "aarch64"] {
        std::fs::write(
            std_start_macos_arch.with_file_name(format!("{arch}.nia")),
            "using root::start::freestanding::macos::startup::niaDarwinStart;\npub extern fun main(argc: i32, argv: &&u8, envp: &&u8) i32 { niaDarwinStart(argc, argv, envp); loop {} }\n",
        )
        .expect("write custom Darwin start");
    }
    std::fs::write(
        &main,
        r#"
pub fun mymain() i32 {
    11
}
"#,
    )
    .expect("write test source");

    let output =
        nia_test_support::nia_command_with_resource_root(env!("CARGO_BIN_EXE_nia"), &resource_root)
            .arg("emit")
            .arg("--exe")
            .arg(&main)
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

    for target in ["x86_64-apple-macos", "aarch64-apple-macos"] {
        let output = nia_test_support::nia_command_with_resource_root(
            env!("CARGO_BIN_EXE_nia"),
            &resource_root,
        )
        .args(["--target", target, "emit", "--exe"])
        .arg(&main)
        .arg("-o")
        .arg(root.join(target))
        .output_timeout_for_build("cross-link custom Darwin startup");
        assert!(
            output.status.success(),
            "{target}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
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

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    process::ExitCode(9)!
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
fn emit_exe_creates_missing_relative_output_directories() {
    let root = temp_dir("emit_exe_creates_missing_relative_output_directories");
    let main = root.join("main.nia");
    let exe_name = format!("out/nested/runnable{}", std::env::consts::EXE_SUFFIX);
    std::fs::write(
        &main,
        r#"
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    process::ExitCode(7)!
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
        .output_timeout_for_build("run nia emit --exe -o out/nested/runnable");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = Command::new(root.join(&exe_name)).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(7));
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

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    process::ExitCode(5)!
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

// Startup runs before any stack frame exists, so it must work at every
// optimization level, including the unoptimized default where the register
// allocator spills freely. Arguments reach `main` intact on both x86 Linux
// targets.
#[test]
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn linux_startup_passes_arguments_at_every_optimization_level() {
    let root = temp_dir("linux_startup_passes_arguments_at_every_optimization_level");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    if init.args().len() != 3 {
        return process::ExitCode(1)!;
    }
    !()
}
"#,
    )
    .expect("write startup argument source");
    for target in ["x86_64-unknown-linux", "x86-unknown-linux"] {
        for level in ["-O0", "-O2"] {
            let exe = root.join(format!("main-{target}{level}"));
            let output = support::nia_command()
                .args(["--target", target, level, "emit", "--exe"])
                .arg(&main)
                .arg("-o")
                .arg(&exe)
                .output_timeout_for_build("emit startup argument executable");
            assert!(
                output.status.success(),
                "{target} {level}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let status = Command::new(&exe)
                .args(["first", "second"])
                .status_timeout("run startup argument executable");
            assert_eq!(status.code(), Some(0), "{target} {level}");
        }
    }
}
