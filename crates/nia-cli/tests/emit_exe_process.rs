// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn atomic_std_facade_checks_emits_and_runs() {
    let root = temp_dir("atomic_std_facade_checks_emits_and_runs");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::atomic;
using std::process;

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
fn emit_exe_links_freestanding_executable() {
    let root = temp_dir("emit_exe_links_freestanding_executable");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

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
fn emit_exe_runs_slice_trait_object_dynamic_dispatch() {
    let root = temp_dir("emit_exe_runs_slice_trait_object_dynamic_dispatch");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

trait Source {
    fn get(& self) i32;
}

extend[T] [T] : Source {
    fn get(& self) i32 {
        self.len() as i32
    }
}

fn read(source: & Source) i32 {
    source.get()
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var values: [3]i32 = [1, 2, 3];
    (read(&values[..]) as process::ExitCode)!
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
    assert_eq!(status.code(), Some(3));
}

#[test]
fn emit_exe_runs_slice_trait_object_dispatch_with_zst_argument() {
    let root = temp_dir("emit_exe_runs_slice_trait_object_dispatch_with_zst_argument");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

struct Empty {}

trait Source {
    fn add(& self, empty: Empty, rhs: i32) i32;
}

extend[T] [T] : Source {
    fn add(& self, empty: Empty, rhs: i32) i32 {
        _ = empty;
        self.len() as i32 + rhs
    }
}

fn read(source: & Source) i32 {
    source.add({}, 4)
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var values: [3]i32 = [1, 2, 3];
    (read(&values[..]) as process::ExitCode)!
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
fn emit_exe_links_freestanding_u128_division_builtins() {
    let root = temp_dir("emit_exe_links_freestanding_u128_division_builtins");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let argc = init.argc();
    let value = (1u128 << 100u32) + 12345u128;
    let by = argc as u128 + 53u128;
    let q = value / by;
    let r = value % by;
    if q * by + r != value {
        return (1 as process::ExitCode)!;
    }
    if r >= by {
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
fn emit_exe_links_freestanding_i128_division_builtins() {
    let root = temp_dir("emit_exe_links_freestanding_i128_division_builtins");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let argc = init.argc();
    let base = (1i128 << 100u32) + 12345i128;
    let divisor = argc as i128 + 53i128;

    let q0 = base / divisor;
    let r0 = base % divisor;
    if q0 * divisor + r0 != base {
        return (1 as process::ExitCode)!;
    }
    if r0 < 0i128 or r0 >= divisor {
        return (2 as process::ExitCode)!;
    }

    let neg_base = -base;
    let q1 = neg_base / divisor;
    let r1 = neg_base % divisor;
    if q1 * divisor + r1 != neg_base {
        return (3 as process::ExitCode)!;
    }
    if r1 > 0i128 or r1 <= -divisor {
        return (4 as process::ExitCode)!;
    }

    let neg_divisor = -divisor;
    let q2 = base / neg_divisor;
    let r2 = base % neg_divisor;
    if q2 * neg_divisor + r2 != base {
        return (5 as process::ExitCode)!;
    }
    if r2 < 0i128 or r2 >= divisor {
        return (6 as process::ExitCode)!;
    }

    let q3 = neg_base / neg_divisor;
    let r3 = neg_base % neg_divisor;
    if q3 * neg_divisor + r3 != neg_base {
        return (7 as process::ExitCode)!;
    }
    if r3 > 0i128 or r3 <= -divisor {
        return (8 as process::ExitCode)!;
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
fn emit_exe_exit_code_is_open_enum() {
    let root = temp_dir("emit_exe_exit_code_is_open_enum");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;
using std::fs;

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
using std::io;
using std::process;

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
using std::math;
using std::process;

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
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    var args = init.args();
    if args.len() != 3 {
        return (1 as process::ExitCode)!;
    }
    switch args.program() {
        ?program => if program.is_empty() {
            return (9 as process::ExitCode)!;
        },
        null => return (10 as process::ExitCode)!,
    }
    var iter = args.skip_program();
    if iter.remaining() != 2usize {
        return (11 as process::ExitCode)!;
    }
    var first_arg = switch iter.next() {
        ?value => value,
        null => return (2 as process::ExitCode)!,
    };
    if iter.remaining() != 1usize {
        return (12 as process::ExitCode)!;
    }
    var second_arg = switch iter.next() {
        ?value => value,
        null => return (3 as process::ExitCode)!,
    };
    if iter.remaining() != 0usize {
        return (13 as process::ExitCode)!;
    }
    var first = first_arg.bytes();
    var second = second_arg.bytes();
    if first.len() != 3 {
        return (4 as process::ExitCode)!;
    }
    if first[0] != 110u8 or first[1] != 105u8 or first[2] != 97u8 {
        return (5 as process::ExitCode)!;
    }
    if second.len() != 4 {
        return (6 as process::ExitCode)!;
    }
    var storage: [16]u8 = [0; 16];
    var writer = io::FixedBufferWriter::init(&mut storage[..]);
    writer.print("{:_>5.2}", [&first_arg]).exit().?;
    let written = writer.written();
    if written.len() != 5usize or written[0] != b'_' or written[1] != b'_' or written[2] != b'_' or written[3] != b'n' or written[4] != b'i' {
        return (8 as process::ExitCode)!;
    }
    switch iter.next() {
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
using std::process;

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
    var iter = init.env().iter();
    while iter.remaining() != 0usize {
        var item = switch iter.next() {
            ?value => value,
            null => return (1 as process::ExitCode)!,
        };
        if starts_with_needle(item.bytes()) {
            return !{};
        }
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
using std::process;

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
fn emit_exe_local_pointer_binding_patterns_destructure_values() {
    let root = temp_dir("emit_exe_local_pointer_binding_patterns_destructure_values");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

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
fn emit_exe_mut_ref_receiver_updates_original_aggregate() {
    let root = temp_dir("emit_exe_mut_ref_receiver_updates_original_aggregate");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

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
