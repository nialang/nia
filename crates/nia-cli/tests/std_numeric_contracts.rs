// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use nia_test_support::{CommandExt, CommandStatusExt, test_dir};

fn emit_and_run(name: &str, source: &str, expected: i32) {
    let root = test_dir(name);
    let input = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&input, source).expect("write std contract");
    let output = support::nia_command()
        .args(["emit", "--exe"])
        .arg(&input)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit std contract");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        Command::new(exe).status_timeout("run std contract").code(),
        Some(expected)
    );
}

#[test]
fn numeric_parsing_shares_text_bytes_and_error_contracts() {
    emit_and_run(
        "numeric_parse_contracts",
        r#"
using std::{parse, process, math::float};

fun same(text: &[char], bytes: &[u8], bits64: u64, bits32: u32) bool {
    let a = match text.parse[f64]() { !value => value, _ => return false };
    let b = match bytes.parse[f64]() { !value => value, _ => return false };
    let c = match text.parse[f32]() { !value => value, _ => return false };
    let d = match bytes.parse[f32]() { !value => value, _ => return false };
    float::toBits64(a) == bits64 and float::toBits64(b) == bits64
        and float::toBits32(c) == bits32 and float::toBits32(d) == bits32
}

fun rejects(text: &[char], bytes: &[u8], expected: parse::Error) bool {
    let a = match text.parse[f64]() { error! => error, _ => return false };
    let b = match bytes.parse[f64]() { error! => error, _ => return false };
    let c = match text.parse[f32]() { error! => error, _ => return false };
    let d = match bytes.parse[f32]() { error! => error, _ => return false };
    a == expected and b == expected and c == expected and d == expected
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    if not same("1.5", b"1.5", 0x3ff8000000000000u64, 0x3fc00000u32) { return process::ExitCode(1)!; }
    if not same("-0e999999999999999999999", b"-0e999999999999999999999", 0x8000000000000000u64, 0x80000000u32) { return process::ExitCode(2)!; }
    if not same("0.1", b"0.1", 0x3fb999999999999au64, 0x3dcccccdu32) { return process::ExitCode(3)!; }
    if not same("-1e-99999", b"-1e-99999", 0x8000000000000000u64, 0x80000000u32) { return process::ExitCode(4)!; }
    if not rejects("", b"", .Empty) { return process::ExitCode(5)!; }
    if not rejects("+1.5", b"+1.5", .InvalidSign) { return process::ExitCode(6)!; }
    if not rejects("1e99999", b"1e99999", .Overflow) { return process::ExitCode(7)!; }
    let invalid: [&[char]; 12] = ["-", "1.", ".", "1e", "1e+", "NaN", "Infinity", " 1", "1_0", "1.2.3", "１.５", "İ.5"];
    for &text in &invalid[..] {
        match text.parse[f64]() {
            parse::Error::InvalidDigit! => {},
            _ => return process::ExitCode(8)!,
        }
    }
    match "42".parse[i32]() { !42 => {}, _ => return process::ExitCode(9)! };
    !()
}
"#,
        0,
    );
}

#[test]
fn nonfinite_formatting_preserves_its_value_error_through_process_conversion() {
    emit_and_run(
        "nonfinite_format_exit",
        r#"
using std::{io, math::float, process};
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let value = float::fromBits64(0x7ff8000000000001u64);
    io::debugPrint("{}", [value]).?;
    !()
}
"#,
        33,
    );
}

#[test]
fn test_cases_preserve_propagated_memory_errors_and_continue_running() {
    emit_and_run(
        "test_memory_error",
        r#"
using std::{mem, test, io, fmt, process};
fun allocation() mem::Error!() { .OutOfMemory! }
fun failing() test::Error!() { allocation().?; !() }
fun passing() test::Error!() { test::expect(true).?; !() }
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let cases = [test::Case::init("allocation", failing), test::Case::init("assertion", passing)];
    let result = cases[0].run();
    let error = match result.error() { ?error => error, null => return process::ExitCode(1)! };
    match error { .Memory(cause) => { if cause != mem::Error::OutOfMemory { return process::ExitCode(2)!; } }, _ => return process::ExitCode(3)! };
    let summary = test::Runner::init(cases).run();
    if summary.failed() != 1 or summary.passed() != 1 { return process::ExitCode(4)!; }
    let mut bytes: [u8; 128] = [0; 128];
    let mut output = io::FixedBufferWriter::init(&mut bytes[..]);
    let mut formatter = fmt::Formatter::init(&mut output);
    error.format(&mut formatter).?;
    if output.len() == 0 { return process::ExitCode(5)!; }
    failing().?;
    !()
}
"#,
        1,
    );
}
