// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_can_write_stdout_through_std_io() {
    let root = temp_dir("emit_exe_can_write_stdout_through_std_io");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let mut buffer: [0]u8 = [];
    let mut stdout = io::FileWriter::stdout(init.io(), &mut buffer[..]);
    if !ok = stdout.write_all(&b"nia\n") { _ = ok; } or error! { return (1 as process::ExitCode)!; }
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
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).output_timeout_without_resources("run emitted executable");
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
using std::io;
using std::fmt;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let mut buffer: [128]u8 = [0; 128];
    let mut stdout = io::FileWriter::stdout(init.io(), &mut buffer[..]);
    if !ok = stdout.print(&"A¢€😀, {}\n", &[&'λ']) { _ = ok; } or error! { return (1 as process::ExitCode)!; }
    if !ok = stdout.flush() { _ = ok; } or error! { return (2 as process::ExitCode)!; }
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
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).output_timeout_without_resources("run emitted executable");
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
using std::io;
using std::fmt;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut storage: [8]u8 = [0, 0, 0, 0, 0, 0, 0, 0];
    let mut writer = io::FixedBufferWriter::init(&mut storage[..]);
    writer.write_all(&b"ni").exit().?;
    if !ok = writer.print(&"nia {}", &[&7]) { _ = ok; } or error! { return (1 as process::ExitCode)!; }
    if writer.len() != 7 {
        return (2 as process::ExitCode)!;
    }
    if !ok = writer.write_all(&b"++") {
        _ = ok;
        return (5 as process::ExitCode)!;
    } or error! {
        if error != io::BufferError::NoSpace {
            return (6 as process::ExitCode)!;
        }
    }

    let mut copied: [7]u8 = [0, 0, 0, 0, 0, 0, 0];
    let mut reader = io::FixedBufferReader::init(writer.written());
    if !ok = reader.read_exact(&mut copied[..]) { _ = ok; } or error! { return (3 as process::ExitCode)!; }
    let mut expected: &[u8] = &b"ninia 7";
    if copied[0] != expected[0] or copied[1] != expected[1] or copied[2] != expected[2] or copied[3] != expected[3] or copied[4] != expected[4] or copied[5] != expected[5] or copied[6] != expected[6] {
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
        .output_timeout_for_build("run nia emit --exe");

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
using std;
using std::collections;
using std::fmt;
using std::io;
using std::mem;
using std::process;
using std::slice;

pub fn main(init: process::Init) process::ExitCode!void {
    let mut raw: [256]u8 = [_]u8[0; 256];
    let mut stdout = io::FileWriter::stdout(init.io(), &mut raw);

    let mut allocator = mem::PageAllocator::init();
    let mut values = std::ArrayList[i32]::init();
    defer values.deinit(&mut allocator).exit().?;

    values.push(&mut allocator, 10).exit().?;
    values.push(&mut allocator, 20).exit().?;
    values.push(&mut allocator, 30).exit().?;

    let mut total = 0;
    for &value in values.iter() {
        total += value;
    }

    let signed: i8 = -5i8;
    let wide: u64 = 123456789u64;
    let max_u128 = u128::MAX;
    let ok = true;
    let ch = 'λ';
    let byte = 171u8;
    let neg = -171i32;
    let numbers: [3]i32 = [4, 5, 6];
    stdout.print(&"list={} slice={} total={} signed={} wide={} max_u128={} ok={} ch={} hex={:x} HEX={:X} bin={:b} oct={:o} neg_hex={:x}\n", &[
        &values,
        &numbers[..],
        &total,
        &signed,
        &wide,
        &max_u128,
        &ok,
        &ch,
        &byte,
        &byte,
        &byte,
        &byte,
        &neg,
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
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).output_timeout_without_resources("run emitted executable");
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "list=[10, 20, 30] slice=[4, 5, 6] total=60 signed=-5 wide=123456789 max_u128=340282366920938463463374607431768211455 ok=true ch=λ hex=ab HEX=AB bin=10101011 oct=253 neg_hex=-ab\n"
    );
}

#[test]
fn emit_exe_std_fmt_formats_alignment_and_width() {
    let root = temp_dir("emit_exe_std_fmt_formats_alignment_and_width");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::fmt;
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let mut raw: [256]u8 = [_]u8[0; 256];
    let mut stdout = io::FileWriter::stdout(init.io(), &mut raw);
    let value: i32 = 7;
    let neg: i32 = -7;
    let byte = 171u8;
    let text = "nia";
    let text_slice: &[char] = &text;
    let ch = 'λ';
    let flag = true;
    let width = 5usize;
    let precision = 2usize;
    stdout.print(&"r='{:>5}' l='{:<5}' c='{:^5}' fr='{:_>5}' fl='{:_<5}' fc='{:*^5}' z='{:05}' ez='{:0>5}' plus='{:+}' plusw='{:+5}' plusz='{:+05}' nz='{:05}' hx='{:08x}' alt='{:#x}' altw='{:#08x}' bin='{:#b}' oct='{:#o}' text='{:>5}' trunc='{:.2}' trw='{:>5.2}' tf='{:_>5.2}' ch='{:<3}' ch0='{:.0}' bool='{:>6}' btr='{:>4.2}' hex='{:x}' dw='{:<{}}' dp='{:.{}}' dwp='{:>{}.{}}'\n", &[
        &value,
        &value,
        &value,
        &value,
        &value,
        &value,
        &value,
        &value,
        &value,
        &value,
        &value,
        &neg,
        &byte,
        &byte,
        &byte,
        &byte,
        &byte,
        text_slice,
        text_slice,
        text_slice,
        text_slice,
        &ch,
        &ch,
        &flag,
        &flag,
        &byte,
        text_slice,
        &width,
        text_slice,
        &precision,
        text_slice,
        &width,
        &precision,
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
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).output_timeout_without_resources("run emitted executable");
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "r='    7' l='7    ' c='  7  ' fr='____7' fl='7____' fc='**7**' z='00007' ez='00007' plus='+7' plusw='   +7' plusz='+0007' nz='-0007' hx='000000ab' alt='0xab' altw='0x0000ab' bin='0b10101011' oct='0253' text='  nia' trunc='ni' trw='   ni' tf='___ni' ch='λ  ' ch0='' bool='  true' btr='  tr' hex='ab' dw='nia  ' dp='ni' dwp='   ni'\n"
    );
}

#[test]
fn emit_exe_std_fmt_formats_byte_slices_as_bytes() {
    let root = temp_dir("emit_exe_std_fmt_formats_byte_slices_as_bytes");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fmt;
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let mut raw: [128]u8 = [_]u8[0; 128];
    let mut stdout = io::FileWriter::stdout(init.io(), &mut raw);
    let bytes: &[u8] = &b"nia";
    stdout.print(&"bytes={} right='{:>5}' left='{:<5}' trunc='{:.2}'\n", &[
        bytes,
        bytes,
        bytes,
        bytes,
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
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).output_timeout_without_resources("run emitted executable");
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "bytes=nia right='  nia' left='nia  ' trunc='ni'\n"
    );
}

#[test]
fn emit_exe_std_fmt_formats_pointers() {
    let root = temp_dir("emit_exe_std_fmt_formats_pointers");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fmt;
using std::io;
using std::process;

fn eq_bytes(left: &[u8], right: &[u8]) bool {
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0usize;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1usize;
    }
    true
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut value = 1234i32;
    let ptr = &value;
    let addr = ptr as usize;

    let mut pointer_storage: [64]u8 = [0; 64];
    let mut pointer_writer = io::FixedBufferWriter::init(&mut pointer_storage[..]);
    pointer_writer.print(&"{:p}", &[&ptr]).exit().?;

    let mut addr_storage: [64]u8 = [0; 64];
    let mut addr_writer = io::FixedBufferWriter::init(&mut addr_storage[..]);
    addr_writer.print(&"{:#x}", &[&addr]).exit().?;

    if not eq_bytes(pointer_writer.written(), addr_writer.written()) {
        return (1 as process::ExitCode)!;
    }

    let mut display_storage: [64]u8 = [0; 64];
    let mut display_writer = io::FixedBufferWriter::init(&mut display_storage[..]);
    display_writer.print(&"{}", &[&ptr]).exit().?;
    if not eq_bytes(display_writer.written(), addr_writer.written()) {
        return (2 as process::ExitCode)!;
    }

    let mut mut_ptr = &mut value;
    let mut mut_storage: [64]u8 = [0; 64];
    let mut mut_writer = io::FixedBufferWriter::init(&mut mut_storage[..]);
    mut_writer.print(&"{:p}", &[&mut_ptr]).exit().?;
    if mut_writer.len() < 3usize or mut_writer.written()[0] != b'0' or mut_writer.written()[1] != b'x' {
        return (3 as process::ExitCode)!;
    }

    let mut padded_storage: [80]u8 = [0; 80];
    let mut padded_writer = io::FixedBufferWriter::init(&mut padded_storage[..]);
    padded_writer.print(&"{:_>20p}", &[&ptr]).exit().?;
    if padded_writer.len() != 20usize {
        return (4 as process::ExitCode)!;
    }
    let written = padded_writer.written();
    let mut index = 0usize;
    while index + pointer_writer.len() < 20usize {
        if written[index] != b'_' {
            return (5 as process::ExitCode)!;
        }
        index += 1usize;
    }
    let mut pointer_index = 0usize;
    while pointer_index < pointer_writer.len() {
        if written[index + pointer_index] != pointer_writer.written()[pointer_index] {
            return (6 as process::ExitCode)!;
        }
        pointer_index += 1usize;
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
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).output_timeout_without_resources("run emitted executable");
    assert_eq!(run.status.code(), Some(0));
}

#[test]
fn emit_exe_std_fmt_reports_template_errors() {
    let root = temp_dir("emit_exe_std_fmt_reports_template_errors");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fmt;
using std::io;
using std::process;

fn expect_error(result: fmt::Error!void, expected: fmt::Error) bool {
    if !ok = result { _ = ok;
            false } or error! { error == expected }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut storage: [32]u8 = [0; 32];
    let mut writer = io::FixedBufferWriter::init(&mut storage[..]);
    let value = 7;

    if not expect_error(writer.print(&"{}", &[]), fmt::Error::MissingArgument) {
        return (1 as process::ExitCode)!;
    }
    if not expect_error(writer.print(&"", &[&value]), fmt::Error::ExtraArgument) {
        return (2 as process::ExitCode)!;
    }
    if not expect_error(writer.print(&"{", &[]), fmt::Error::InvalidTemplate) {
        return (3 as process::ExitCode)!;
    }
    if not expect_error(writer.print(&"}", &[]), fmt::Error::InvalidTemplate) {
        return (4 as process::ExitCode)!;
    }
    if !ok = writer.print(&"{{{}}}", &[&value]) { _ = ok; } or error! { return (5 as process::ExitCode)!; }
    if writer.len() != 3 {
        return (6 as process::ExitCode)!;
    }
    let written = writer.written();
    if written[0] != b'{' or written[1] != b'7' or written[2] != b'}' {
        return (7 as process::ExitCode)!;
    }
    if not expect_error(writer.print(&"{q}", &[&value]), fmt::Error::InvalidTemplate) {
        return (8 as process::ExitCode)!;
    }
    let flag = true;
    if not expect_error(writer.print(&"{x}", &[&flag]), fmt::Error::InvalidTemplate) {
        return (9 as process::ExitCode)!;
    }
    if not expect_error(writer.print(&"{x}", &[&value]), fmt::Error::InvalidTemplate) {
        return (10 as process::ExitCode)!;
    }
    if !ok = fmt::print_unchecked(&mut writer, &"{:X}", &[&value]) { _ = ok; } or error! { return (11 as process::ExitCode)!; }
    if not expect_error(writer.print(&"{:q}", &[&value]), fmt::Error::InvalidTemplate) {
        return (12 as process::ExitCode)!;
    }
    if not expect_error(writer.print(&"{:08", &[&value]), fmt::Error::InvalidTemplate) {
        return (13 as process::ExitCode)!;
    }
    let byte = 7u8;
    if not expect_error(writer.print(&"{:+}", &[&byte]), fmt::Error::InvalidTemplate) {
        return (14 as process::ExitCode)!;
    }
    if not expect_error(writer.print(&"{:+}", &[&flag]), fmt::Error::InvalidTemplate) {
        return (15 as process::ExitCode)!;
    }
    if not expect_error(writer.print(&"{:#}", &[&value]), fmt::Error::InvalidTemplate) {
        return (16 as process::ExitCode)!;
    }
    if not expect_error(writer.print(&"{:#}", &[&flag]), fmt::Error::InvalidTemplate) {
        return (17 as process::ExitCode)!;
    }
    if not expect_error(writer.print(&"{:.}", &[&flag]), fmt::Error::InvalidTemplate) {
        return (18 as process::ExitCode)!;
    }
    if not expect_error(writer.print(&"{:.2}", &[&value]), fmt::Error::InvalidTemplate) {
        return (19 as process::ExitCode)!;
    }
    if not expect_error(writer.print(&"{:_5}", &[&flag]), fmt::Error::InvalidTemplate) {
        return (20 as process::ExitCode)!;
    }
    let ptr = &value;
    if not expect_error(writer.print(&"{:+p}", &[&ptr]), fmt::Error::InvalidTemplate) {
        return (21 as process::ExitCode)!;
    }
    if not expect_error(writer.print(&"{:#p}", &[&ptr]), fmt::Error::InvalidTemplate) {
        return (22 as process::ExitCode)!;
    }
    if not expect_error(writer.print(&"{:.2p}", &[&ptr]), fmt::Error::InvalidTemplate) {
        return (23 as process::ExitCode)!;
    }
    if not expect_error(writer.print(&"{:<{}}", &[&value]), fmt::Error::MissingArgument) {
        return (24 as process::ExitCode)!;
    }
    let bad_width = 5u32;
    if not expect_error(writer.print(&"{:<{}}", &[&value, &bad_width]), fmt::Error::InvalidTemplate) {
        return (25 as process::ExitCode)!;
    }
    if not expect_error(writer.print(&"{:.{}}", &[&flag]), fmt::Error::MissingArgument) {
        return (26 as process::ExitCode)!;
    }
    if not expect_error(writer.print(&"{:.{}}", &[&flag, &bad_width]), fmt::Error::InvalidTemplate) {
        return (27 as process::ExitCode)!;
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
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_fmt_parses_primitives() {
    let root = temp_dir("emit_exe_std_fmt_parses_primitives");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fmt;
using std::process;

fn expect_i32(result: fmt::ParseError!i32, expected: i32) bool {
    if !value = result { value == expected } or error! { _ = error;
            false }
}

fn expect_error_i32(result: fmt::ParseError!i32, expected: fmt::ParseError) bool {
    if !value = result { _ = value;
            false } or error! { error == expected }
}

fn expect_error_u8(result: fmt::ParseError!u8, expected: fmt::ParseError) bool {
    if !value = result { _ = value;
            false } or error! { error == expected }
}

fn expect_u8(result: fmt::ParseError!u8, expected: u8) bool {
    if !value = result { value == expected } or error! { _ = error;
            false }
}

fn expect_error_bool(result: fmt::ParseError!bool, expected: fmt::ParseError) bool {
    if !value = result { _ = value;
            false } or error! { error == expected }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;

    if not expect_i32(fmt::parse[i32](&"-2147483648"[..]), i32::MIN) {
        return (1 as process::ExitCode)!;
    }
    if not expect_i32(fmt::parse[i32](&"+2147483647"[..]), i32::MAX) {
        return (2 as process::ExitCode)!;
    }
    if !value = fmt::parse[u128](&"340282366920938463463374607431768211455"[..]) { if value != u128::MAX {
            return (3 as process::ExitCode)!;
        } } or error! { return (4 as process::ExitCode)!; }
    if !value = fmt::parse[usize](&"12345"[..]) { if value != 12345usize {
            return (5 as process::ExitCode)!;
        } } or error! { return (6 as process::ExitCode)!; }
    if !value = fmt::parse[bool](&"false"[..]) { if value {
            return (7 as process::ExitCode)!;
        } } or error! { return (8 as process::ExitCode)!; }

    if not expect_error_i32(fmt::parse[i32](&""[..]), fmt::ParseError::Empty) {
        return (9 as process::ExitCode)!;
    }
    if not expect_error_i32(fmt::parse[i32](&"-"[..]), fmt::ParseError::InvalidDigit) {
        return (10 as process::ExitCode)!;
    }
    if not expect_error_i32(fmt::parse[i32](&"12x"[..]), fmt::ParseError::InvalidDigit) {
        return (11 as process::ExitCode)!;
    }
    if not expect_error_i32(fmt::parse[i32](&"2147483648"[..]), fmt::ParseError::Overflow) {
        return (12 as process::ExitCode)!;
    }
    if not expect_error_u8(fmt::parse[u8](&"-1"[..]), fmt::ParseError::InvalidSign) {
        return (13 as process::ExitCode)!;
    }
    if not expect_error_u8(fmt::parse[u8](&"256"[..]), fmt::ParseError::Overflow) {
        return (14 as process::ExitCode)!;
    }
    if not expect_u8(fmt::parse_radix[u8](&"ff"[..], 16u32), 255u8) {
        return (15 as process::ExitCode)!;
    }
    if not expect_u8(fmt::parse[u8](&"0xff"[..]), 255u8) {
        return (16 as process::ExitCode)!;
    }
    if not expect_u8(fmt::parse_radix[u8](&"10101010"[..], 2u32), 170u8) {
        return (17 as process::ExitCode)!;
    }
    if not expect_u8(fmt::parse[u8](&"0b10101010"[..]), 170u8) {
        return (18 as process::ExitCode)!;
    }
    if not expect_u8(fmt::parse[u8](&"0o377"[..]), 255u8) {
        return (19 as process::ExitCode)!;
    }
    if not expect_i32(fmt::parse_radix[i32](&"-7B"[..], 16u32), -123) {
        return (20 as process::ExitCode)!;
    }
    if not expect_i32(fmt::parse[i32](&"-0x7B"[..]), -123) {
        return (21 as process::ExitCode)!;
    }
    if not expect_i32(fmt::parse[i32](&"+0b1111011"[..]), 123) {
        return (22 as process::ExitCode)!;
    }
    if not expect_error_u8(fmt::parse_radix[u8](&"2"[..], 2u32), fmt::ParseError::InvalidDigit) {
        return (23 as process::ExitCode)!;
    }
    if not expect_error_u8(fmt::parse_radix[u8](&"10"[..], 1u32), fmt::ParseError::InvalidRadix) {
        return (24 as process::ExitCode)!;
    }
    if not expect_error_bool(fmt::parse_radix[bool](&"true"[..], 10u32), fmt::ParseError::InvalidRadix) {
        return (25 as process::ExitCode)!;
    }
    if !value = fmt::parse_radix[u128](&"ffffffffffffffffffffffffffffffff"[..], 16u32) { if value != u128::MAX {
            return (26 as process::ExitCode)!;
        } } or error! { return (27 as process::ExitCode)!; }
    if !value = fmt::parse_radix[u128](&"100000000000000000000000000000000"[..], 16u32) { _ = value;
            return (28 as process::ExitCode)!; } or error! { if error != fmt::ParseError::Overflow {
            return (29 as process::ExitCode)!;
        } }
    if not expect_error_u8(fmt::parse[u8](&"+1"[..]), fmt::ParseError::InvalidSign) {
        return (30 as process::ExitCode)!;
    }
    if not expect_error_u8(fmt::parse_radix[u8](&"0xff"[..], 16u32), fmt::ParseError::InvalidDigit) {
        return (31 as process::ExitCode)!;
    }
    if not expect_error_u8(fmt::parse[u8](&"0x"[..]), fmt::ParseError::InvalidDigit) {
        return (32 as process::ExitCode)!;
    }
    if not expect_error_u8(fmt::parse[u8](&"0b2"[..]), fmt::ParseError::InvalidDigit) {
        return (33 as process::ExitCode)!;
    }
    if not expect_u8(fmt::parse[u8](&b"255"[..]), 255u8) {
        return (34 as process::ExitCode)!;
    }
    if not expect_u8(fmt::parse[u8](&b"0xff"[..]), 255u8) {
        return (35 as process::ExitCode)!;
    }
    if not expect_u8(fmt::parse_radix[u8](&b"ff"[..], 16u32), 255u8) {
        return (36 as process::ExitCode)!;
    }
    if !value = fmt::parse[bool](&b"true"[..]) { if not value {
            return (37 as process::ExitCode)!;
        } } or error! { return (38 as process::ExitCode)!; }
    if not expect_error_u8(fmt::parse_radix[u8](&b"0xff"[..], 16u32), fmt::ParseError::InvalidDigit) {
        return (39 as process::ExitCode)!;
    }
    let invalid_bytes: [1]u8 = [255u8];
    if not expect_error_u8(fmt::parse[u8](&invalid_bytes[..]), fmt::ParseError::InvalidDigit) {
        return (40 as process::ExitCode)!;
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
        .output_timeout_for_build("run nia emit --exe");

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
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut discard = io::DiscardingWriter::init();
    if !ok = discard.write_all(&b"abcdef") { _ = ok; } or error! { return (1 as process::ExitCode)!; }
    if discard.len() != 6 {
        return (2 as process::ExitCode)!;
    }

    let mut source = io::FixedBufferReader::init(&b"abcdef");
    let mut limited = io::LimitedReader[io::FixedBufferReader]::init(
        &mut source,
        io::Limit::limited(3),
    );
    let mut copied: [4]u8 = [0, 0, 0, 0];
    let mut n: usize;
    if !value = limited.read(&mut copied[..]) { n = value; } or error! { return (3 as process::ExitCode)!; }
    if n != 3 {
        return (4 as process::ExitCode)!;
    }
    if copied[0] != b'a' or copied[1] != b'b' or copied[2] != b'c' {
        return (5 as process::ExitCode)!;
    }
    if !value = limited.read(&mut copied[..]) { n = value; } or error! { return (6 as process::ExitCode)!; }
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
        .output_timeout_for_build("run nia emit --exe");

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
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut storage: [16]u8 = [0; 16];
    let mut backing = io::FixedBufferWriter::init(&mut storage[..]);
    let mut buffer_storage: [4]u8 = [0; 4];
    let mut writer = io::BufferedWriter[io::FixedBufferWriter]::init(
        &mut backing,
        &mut buffer_storage[..],
    );

    if !ok = writer.write_all(&b"abc") { _ = ok; } or error! { return (1 as process::ExitCode)!; }
    if writer.len() != 3 or backing.len() != 0 {
        return (2 as process::ExitCode)!;
    }

    if !ok = writer.write_byte(b'd') { _ = ok; } or error! { return (3 as process::ExitCode)!; }
    if writer.len() != 4 or backing.len() != 0 {
        return (4 as process::ExitCode)!;
    }

    if !ok = writer.write_all(&b"efghij") { _ = ok; } or error! { return (5 as process::ExitCode)!; }
    if writer.len() != 0 or backing.len() != 10 {
        return (6 as process::ExitCode)!;
    }

    if !ok = writer.flush() { _ = ok; } or error! { return (7 as process::ExitCode)!; }
    if backing.len() != 10 {
        return (8 as process::ExitCode)!;
    }

    let mut expected: &[u8] = &b"abcdefghij";
    let mut written = backing.written();
    let mut index = 0usize;
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
        .output_timeout_for_build("run nia emit --exe");

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
using std::io;
using std::process;

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
        let mut count = bytes.len();
        if count > 2usize {
            count = 2usize;
        }
        self.inner.write(&bytes[0..count])
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut storage: [16]u8 = [0; 16];
    let mut backing = PartialWriter::init(&mut storage[..]);
    let mut buffer_storage: [8]u8 = [0; 8];
    let mut writer = io::BufferedWriter[PartialWriter]::init(
        &mut backing,
        &mut buffer_storage[..],
    );

    if !ok = writer.write_all(&b"abcdef") { _ = ok; } or error! { return (1 as process::ExitCode)!; }
    if writer.len() != 6 or backing.len() != 0 {
        return (2 as process::ExitCode)!;
    }

    if !ok = writer.flush() { _ = ok; } or error! { return (3 as process::ExitCode)!; }
    if writer.len() != 0 or backing.len() != 6 {
        return (4 as process::ExitCode)!;
    }

    let expected: &[u8] = &b"abcdef";
    let written = backing.written();
    let mut index = 0usize;
    while index < expected.len() {
        if written[index] != expected[index] {
            return (5 as process::ExitCode)!;
        }
        index += 1usize;
    }

    let mut direct_storage: [16]u8 = [0; 16];
    let mut direct_backing = PartialWriter::init(&mut direct_storage[..]);
    let mut direct_buffer_storage: [4]u8 = [0; 4];
    let mut direct_writer = io::BufferedWriter[PartialWriter]::init(
        &mut direct_backing,
        &mut direct_buffer_storage[..],
    );
    if !ok = direct_writer.write_all(&b"ghijkl") { _ = ok; } or error! { return (6 as process::ExitCode)!; }
    if direct_writer.len() != 0 or direct_backing.len() != 6 {
        return (7 as process::ExitCode)!;
    }
    let direct_expected: &[u8] = &b"ghijkl";
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
        .output_timeout_for_build("run nia emit --exe");

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
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut source = io::FixedBufferReader::init(&b"abcdefghij");
    let mut buffer_storage: [4]u8 = [0; 4];
    let mut reader = io::BufferedReader[io::FixedBufferReader]::init(
        &mut source,
        &mut buffer_storage[..],
    );

    let mut first: [2]u8 = [0; 2];
    let mut n: usize;
    if !value = reader.read(&mut first[..]) { n = value; } or error! { return (1 as process::ExitCode)!; }
    if n != 2 or first[0] != b'a' or first[1] != b'b' {
        return (2 as process::ExitCode)!;
    }
    if reader.len() != 2 {
        return (3 as process::ExitCode)!;
    }

    let mut second: [3]u8 = [0; 3];
    if !value = reader.read(&mut second[..]) { n = value; } or error! { return (4 as process::ExitCode)!; }
    if n != 2 or second[0] != b'c' or second[1] != b'd' {
        return (5 as process::ExitCode)!;
    }
    if reader.len() != 0 {
        return (6 as process::ExitCode)!;
    }

    let mut third: [5]u8 = [0; 5];
    if !value = reader.read(&mut third[..]) { n = value; } or error! { return (7 as process::ExitCode)!; }
    if n != 5 {
        return (8 as process::ExitCode)!;
    }
    if third[0] != b'e' or third[1] != b'f' or third[2] != b'g' or third[3] != b'h' or third[4] != b'i' {
        return (9 as process::ExitCode)!;
    }

    let mut fourth: [2]u8 = [0; 2];
    if !value = reader.read(&mut fourth[..]) { n = value; } or error! { return (10 as process::ExitCode)!; }
    if n != 1 or fourth[0] != b'j' {
        return (11 as process::ExitCode)!;
    }

    if !value = reader.read(&mut fourth[..]) { n = value; } or error! { return (12 as process::ExitCode)!; }
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
        .output_timeout_for_build("run nia emit --exe");

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
using std::io;
using std::process;

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
        let mut count = bytes.len();
        if count > 2usize {
            count = 2usize;
        }
        self.inner.read(&mut bytes[0..count])
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut source = PartialReader::init(&b"abcdef");
    let mut bytes: [6]u8 = [0; 6];
    if !ok = source.read_exact(&mut bytes[..]) { _ = ok; } or error! { return (1 as process::ExitCode)!; }
    let expected: &[u8] = &b"abcdef";
    let mut index = 0usize;
    while index < expected.len() {
        if bytes[index] != expected[index] {
            return (2 as process::ExitCode)!;
        }
        index += 1usize;
    }

    let mut short = PartialReader::init(&b"xy");
    let mut too_many: [3]u8 = [0; 3];
    if !ok = short.read_exact(&mut too_many[..]) { _ = ok;
            return (3 as process::ExitCode)!; } or error! { }
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
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}
