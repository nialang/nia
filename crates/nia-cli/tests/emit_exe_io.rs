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

pub fn main(init: process::Init) process::ExitCode!() {
    let mut buffer: [u8; 0] = [];
    let mut stdout = io::FileWriter::stdout(&mut buffer[..]);
    match stdout.writeAll(&b"nia\n") {
        !ok => { _ = ok; },
        error! => { return process::exit(1)!; },
    }
    match stdout.writeUtf8(&"lambda: λ\n") {
        !ok => { _ = ok; },
        error! => { return process::exit(2)!; },
    }
    let mut storage: [u8; 1] = [0];
    let mut bounded = io::FixedBufferWriter::init(&mut storage[..]);
    match bounded.writeUtf8(&"λ") {
        !ok => {
            _ = ok;
            return process::exit(3)!;
        },
        io::BufferError::NoSpace! => {},
        error! => {
            _ = error;
            return process::exit(4)!;
        },
    }
    !()
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
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

    let run = Command::new(&exe).output_timeout_for_runtime("run emitted executable");
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&run.stdout), "nia\nlambda: λ\n");
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

pub fn main(init: process::Init) process::ExitCode!() {
    let mut buffer: [u8; 128] = [0; 128];
    let mut stdout = io::FileWriter::stdout(&mut buffer[..]);
    match stdout.print(&"A¢€😀, {}\n", &[&'λ']) {
        !ok => { _ = ok; },
        error! => { return process::exit(1)!; },
    }
    match stdout.flush() {
        !ok => { _ = ok; },
        error! => { return process::exit(2)!; },
    }
    !()
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
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

    let run = Command::new(&exe).output_timeout_for_runtime("run emitted executable");
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut storage: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 0];
    let mut writer = io::FixedBufferWriter::init(&mut storage[..]);
    writer.writeAll(&b"ni").exit().?;
    match writer.print(&"nia {}", &[&7]) {
        !ok => { _ = ok; },
        error! => { return process::exit(1)!; },
    }
    if writer.len() != 7 {
        return process::exit(2)!;
    }
    match writer.writeAll(&b"++") {
        !ok => {
            _ = ok;
            return process::exit(5)!;
        },
        error! => {
            if error != io::BufferError::NoSpace {
                return process::exit(6)!;
            }
        },
    }

    let mut copied: [u8; 7] = [0, 0, 0, 0, 0, 0, 0];
    let mut reader = io::FixedBufferReader::init(writer.written());
    match reader.readExact(&mut copied[..]) {
        !ok => { _ = ok; },
        error! => { return process::exit(3)!; },
    }
    let mut expected: &[u8] = &b"ninia 7";
    if copied[0] != expected[0] or copied[1] != expected[1] or copied[2] != expected[2] or copied[3] != expected[3] or copied[4] != expected[4] or copied[5] != expected[5] or copied[6] != expected[6] {
        return process::exit(4)!;
    }
    !()
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
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

pub fn main(init: process::Init) process::ExitCode!() {
    let mut raw: [u8; 256] = [0; 256];
    let mut stdout = io::FileWriter::stdout(&mut raw);

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
    let numbers: [i32; 3] = [4, 5, 6];
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
    !()
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
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

    let run = Command::new(&exe).output_timeout_for_runtime("run emitted executable");
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

pub fn main(init: process::Init) process::ExitCode!() {
    let mut raw: [u8; 256] = [0; 256];
    let mut stdout = io::FileWriter::stdout(&mut raw);
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
    !()
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
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

    let run = Command::new(&exe).output_timeout_for_runtime("run emitted executable");
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

pub fn main(init: process::Init) process::ExitCode!() {
    let mut raw: [u8; 128] = [0; 128];
    let mut stdout = io::FileWriter::stdout(&mut raw);
    let bytes: &[u8] = &b"nia";
    stdout.print(&"bytes={} right='{:>5}' left='{:<5}' trunc='{:.2}'\n", &[
        bytes,
        bytes,
        bytes,
        bytes,
    ]).exit().?;
    stdout.flush().exit().?;
    !()
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
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

    let run = Command::new(&exe).output_timeout_for_runtime("run emitted executable");
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut value = 1234i32;
    let ptr = &value;
    let addr = ptr as usize;

    let mut pointer_storage: [u8; 64] = [0; 64];
    let mut pointer_writer = io::FixedBufferWriter::init(&mut pointer_storage[..]);
    pointer_writer.print(&"{:p}", &[&ptr]).exit().?;

    let mut addr_storage: [u8; 64] = [0; 64];
    let mut addr_writer = io::FixedBufferWriter::init(&mut addr_storage[..]);
    addr_writer.print(&"{:#x}", &[&addr]).exit().?;

    if not eq_bytes(pointer_writer.written(), addr_writer.written()) {
        return process::exit(1)!;
    }

    let mut display_storage: [u8; 64] = [0; 64];
    let mut display_writer = io::FixedBufferWriter::init(&mut display_storage[..]);
    display_writer.print(&"{}", &[&ptr]).exit().?;
    if not eq_bytes(display_writer.written(), addr_writer.written()) {
        return process::exit(2)!;
    }

    let mut mut_ptr = &mut value;
    let mut mut_storage: [u8; 64] = [0; 64];
    let mut mut_writer = io::FixedBufferWriter::init(&mut mut_storage[..]);
    mut_writer.print(&"{:p}", &[&mut_ptr]).exit().?;
    if mut_writer.len() < 3usize or mut_writer.written()[0] != b'0' or mut_writer.written()[1] != b'x' {
        return process::exit(3)!;
    }

    let mut padded_storage: [u8; 80] = [0; 80];
    let mut padded_writer = io::FixedBufferWriter::init(&mut padded_storage[..]);
    padded_writer.print(&"{:_>20p}", &[&ptr]).exit().?;
    if padded_writer.len() != 20usize {
        return process::exit(4)!;
    }
    let written = padded_writer.written();
    let mut index = 0usize;
    while index + pointer_writer.len() < 20usize {
        if written[index] != b'_' {
            return process::exit(5)!;
        }
        index += 1usize;
    }
    let mut pointer_index = 0usize;
    while pointer_index < pointer_writer.len() {
        if written[index + pointer_index] != pointer_writer.written()[pointer_index] {
            return process::exit(6)!;
        }
        pointer_index += 1usize;
    }

    !()
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
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

    let run = Command::new(&exe).output_timeout_for_runtime("run emitted executable");
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

fn expect_error(result: fmt::Error!(), expected: fmt::Error) bool {
    match result {
        !ok => { _ = ok;
                false },
        error! => { error == expected },
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut storage: [u8; 32] = [0; 32];
    let mut writer = io::FixedBufferWriter::init(&mut storage[..]);
    let value = 7;

    if not expect_error(writer.print(&"{}", &[]), fmt::Error::MissingArgument) {
        return process::exit(1)!;
    }
    if not expect_error(writer.print(&"", &[&value]), fmt::Error::ExtraArgument) {
        return process::exit(2)!;
    }
    if not expect_error(writer.print(&"{", &[]), fmt::Error::InvalidTemplate) {
        return process::exit(3)!;
    }
    if not expect_error(writer.print(&"}", &[]), fmt::Error::InvalidTemplate) {
        return process::exit(4)!;
    }
    match writer.print(&"{{{}}}", &[&value]) {
        !ok => { _ = ok; },
        error! => { return process::exit(5)!; },
    }
    if writer.len() != 3 {
        return process::exit(6)!;
    }
    let written = writer.written();
    if written[0] != b'{' or written[1] != b'7' or written[2] != b'}' {
        return process::exit(7)!;
    }
    if not expect_error(writer.print(&"{q}", &[&value]), fmt::Error::InvalidTemplate) {
        return process::exit(8)!;
    }
    let flag = true;
    if not expect_error(writer.print(&"{x}", &[&flag]), fmt::Error::InvalidTemplate) {
        return process::exit(9)!;
    }
    if not expect_error(writer.print(&"{x}", &[&value]), fmt::Error::InvalidTemplate) {
        return process::exit(10)!;
    }
    match writer.print(&"{:X}", &[&value]) {
        !ok => { _ = ok; },
        error! => { return process::exit(11)!; },
    }
    if not expect_error(writer.print(&"{:q}", &[&value]), fmt::Error::InvalidTemplate) {
        return process::exit(12)!;
    }
    if not expect_error(writer.print(&"{:08", &[&value]), fmt::Error::InvalidTemplate) {
        return process::exit(13)!;
    }
    let byte = 7u8;
    if not expect_error(writer.print(&"{:+}", &[&byte]), fmt::Error::InvalidTemplate) {
        return process::exit(14)!;
    }
    if not expect_error(writer.print(&"{:+}", &[&flag]), fmt::Error::InvalidTemplate) {
        return process::exit(15)!;
    }
    if not expect_error(writer.print(&"{:#}", &[&value]), fmt::Error::InvalidTemplate) {
        return process::exit(16)!;
    }
    if not expect_error(writer.print(&"{:#}", &[&flag]), fmt::Error::InvalidTemplate) {
        return process::exit(17)!;
    }
    if not expect_error(writer.print(&"{:.}", &[&flag]), fmt::Error::InvalidTemplate) {
        return process::exit(18)!;
    }
    if not expect_error(writer.print(&"{:.2}", &[&value]), fmt::Error::InvalidTemplate) {
        return process::exit(19)!;
    }
    if not expect_error(writer.print(&"{:_5}", &[&flag]), fmt::Error::InvalidTemplate) {
        return process::exit(20)!;
    }
    let ptr = &value;
    if not expect_error(writer.print(&"{:+p}", &[&ptr]), fmt::Error::InvalidTemplate) {
        return process::exit(21)!;
    }
    if not expect_error(writer.print(&"{:#p}", &[&ptr]), fmt::Error::InvalidTemplate) {
        return process::exit(22)!;
    }
    if not expect_error(writer.print(&"{:.2p}", &[&ptr]), fmt::Error::InvalidTemplate) {
        return process::exit(23)!;
    }
    if not expect_error(writer.print(&"{:<{}}", &[&value]), fmt::Error::MissingArgument) {
        return process::exit(24)!;
    }
    let bad_width = 5u32;
    if not expect_error(writer.print(&"{:<{}}", &[&value, &bad_width]), fmt::Error::InvalidTemplate) {
        return process::exit(25)!;
    }
    if not expect_error(writer.print(&"{:.{}}", &[&flag]), fmt::Error::MissingArgument) {
        return process::exit(26)!;
    }
    if not expect_error(writer.print(&"{:.{}}", &[&flag, &bad_width]), fmt::Error::InvalidTemplate) {
        return process::exit(27)!;
    }
    if not expect_error(
        writer.print(&"{:999999999999999999999999999999}", &[&value]),
        fmt::Error::InvalidTemplate,
    ) {
        return process::exit(28)!;
    }
    if not expect_error(
        writer.print(&"{:.999999999999999999999999999999}", &[&value]),
        fmt::Error::InvalidTemplate,
    ) {
        return process::exit(29)!;
    }
    !()
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
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
fn emit_exe_std_debug_print_returns_format_and_flush_errors() {
    let root = temp_dir("emit_exe_std_debug_print_returns_format_and_flush_errors");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::debug;
using std::fmt;
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    match debug::print(&"{", &[]) {
        !ok => {
            _ = ok;
            return process::exit(1)!;
        },
        debug::Error::Format(fmt::Error::InvalidTemplate)! => {},
        error! => {
            _ = error;
            return process::exit(2)!;
        },
    }

    match debug::print(&"closed stderr\n", &[]) {
        !ok => {
            _ = ok;
            return process::exit(3)!;
        },
        debug::Error::Flush(fs::Error::BadFd)! => {},
        error! => {
            _ = error;
            return process::exit(4)!;
        },
    }

    if (debug::Error::Format(fmt::Error::MissingArgument).asExitCode() as i32) != 22 {
        return process::exit(5)!;
    }
    if (debug::Error::Flush(fs::Error::BadFd).asExitCode() as i32) != 9 {
        return process::exit(6)!;
    }

    !()
}
"#,
    )
    .expect("write debug error source");

    let emit = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe std debug errors");
    assert!(
        emit.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&emit.stderr)
    );

    let status = Command::new("/bin/sh")
        .arg("-c")
        .arg("exec 2>&-; exec \"$1\"")
        .arg("sh")
        .arg(&exe)
        .status_timeout("run emitted std debug error executable with closed stderr");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_parse_parses_primitives() {
    let root = temp_dir("emit_exe_std_parse_parses_primitives");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::parse;
using std::process;
using std::slice;

enum LevelError: i32 {
    Invalid = 1,
    _,
}

struct Level {
    code: u8,
}

extend Level : parse::From[&[char]] {
    type Error = LevelError;

    fn from(input: &[char]) LevelError!Level {
        if input.equals(&"high") {
            !Level { code: 1 }
        } else {
            LevelError::Invalid!
        }
    }
}

fn expect_i32(result: parse::Error!i32, expected: i32) bool {
    match result {
        !value => { value == expected },
        error! => { _ = error;
                false },
    }
}

fn expect_error_i32(result: parse::Error!i32, expected: parse::Error) bool {
    match result {
        !value => { _ = value;
                false },
        error! => { error == expected },
    }
}

fn expect_error_u8(result: parse::Error!u8, expected: parse::Error) bool {
    match result {
        !value => { _ = value;
                false },
        error! => { error == expected },
    }
}

fn expect_u8(result: parse::Error!u8, expected: u8) bool {
    match result {
        !value => { value == expected },
        error! => { _ = error;
                false },
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;

    if not expect_i32((&"-2147483648").parse[i32](), i32::MIN) {
        return process::exit(1)!;
    }
    if not expect_i32((&"+2147483647").parse[i32](), i32::MAX) {
        return process::exit(2)!;
    }
    match (&"340282366920938463463374607431768211455").parse[u128]() {
        !value => { if value != u128::MAX {
                return process::exit(3)!;
            } },
        error! => { return process::exit(4)!; },
    }
    match (&"12345").parse[usize]() {
        !value => { if value != 12345 {
                return process::exit(5)!;
            } },
        error! => { return process::exit(6)!; },
    }
    match (&"false").parse[bool]() {
        !value => { if value {
                return process::exit(7)!;
            } },
        error! => { return process::exit(8)!; },
    }

    if not expect_error_i32((&"").parse[i32](), parse::Error::Empty) {
        return process::exit(9)!;
    }
    if not expect_error_i32((&"-").parse[i32](), parse::Error::InvalidDigit) {
        return process::exit(10)!;
    }
    if not expect_error_i32((&"12x").parse[i32](), parse::Error::InvalidDigit) {
        return process::exit(11)!;
    }
    if not expect_error_i32((&"2147483648").parse[i32](), parse::Error::Overflow) {
        return process::exit(12)!;
    }
    if not expect_error_u8((&"-1").parse[u8](), parse::Error::InvalidSign) {
        return process::exit(13)!;
    }
    if not expect_error_u8((&"256").parse[u8](), parse::Error::Overflow) {
        return process::exit(14)!;
    }
    if not expect_u8((&"ff").parseRadix[u8](16), 255) {
        return process::exit(15)!;
    }
    if not expect_u8((&"0xff").parse[u8](), 255) {
        return process::exit(16)!;
    }
    if not expect_u8((&"10101010").parseRadix[u8](2), 170) {
        return process::exit(17)!;
    }
    if not expect_u8((&"0b10101010").parse[u8](), 170) {
        return process::exit(18)!;
    }
    if not expect_u8((&"0o377").parse[u8](), 255) {
        return process::exit(19)!;
    }
    if not expect_i32((&"-7B").parseRadix[i32](16), -123) {
        return process::exit(20)!;
    }
    if not expect_i32((&"-0x7B").parse[i32](), -123) {
        return process::exit(21)!;
    }
    if not expect_i32((&"+0b1111011").parse[i32](), 123) {
        return process::exit(22)!;
    }
    if not expect_error_u8((&"2").parseRadix[u8](2), parse::Error::InvalidDigit) {
        return process::exit(23)!;
    }
    if not expect_error_u8((&"10").parseRadix[u8](1), parse::Error::InvalidRadix) {
        return process::exit(24)!;
    }
    match (&"yes").parse[bool]() {
        !value => { _ = value;
                return process::exit(25)!; },
        error! => { if error != parse::Error::InvalidValue {
                return process::exit(25)!;
            } },
    }
    match (&"high").parse[Level]() {
        !level => { if level.code != 1 {
                return process::exit(41)!;
            } },
        error! => { return process::exit(42)!; },
    }
    match (&"low").parse[Level]() {
        !level => { _ = level;
                return process::exit(43)!; },
        error! => { if error != LevelError::Invalid {
                return process::exit(44)!;
            } },
    }
    match (&"ffffffffffffffffffffffffffffffff").parseRadix[u128](16) {
        !value => { if value != u128::MAX {
                return process::exit(26)!;
            } },
        error! => { return process::exit(27)!; },
    }
    match (&"100000000000000000000000000000000").parseRadix[u128](16) {
        !value => { _ = value;
                return process::exit(28)!; },
        error! => { if error != parse::Error::Overflow {
                return process::exit(29)!;
            } },
    }
    if not expect_error_u8((&"+1").parse[u8](), parse::Error::InvalidSign) {
        return process::exit(30)!;
    }
    if not expect_error_u8((&"0xff").parseRadix[u8](16), parse::Error::InvalidDigit) {
        return process::exit(31)!;
    }
    if not expect_error_u8((&"0x").parse[u8](), parse::Error::InvalidDigit) {
        return process::exit(32)!;
    }
    if not expect_error_u8((&"0b2").parse[u8](), parse::Error::InvalidDigit) {
        return process::exit(33)!;
    }
    if not expect_u8((&b"255").parse[u8](), 255) {
        return process::exit(34)!;
    }
    if not expect_u8((&b"0xff").parse[u8](), 255) {
        return process::exit(35)!;
    }
    if not expect_u8((&b"ff").parseRadix[u8](16), 255) {
        return process::exit(36)!;
    }
    match (&b"true").parse[bool]() {
        !value => { if not value {
                return process::exit(37)!;
            } },
        error! => { return process::exit(38)!; },
    }
    if not expect_error_u8((&b"0xff").parseRadix[u8](16), parse::Error::InvalidDigit) {
        return process::exit(39)!;
    }
    let invalidBytes: [u8; 1] = [255];
    if not expect_error_u8((&invalidBytes).parse[u8](), parse::Error::InvalidDigit) {
        return process::exit(40)!;
    }
    !()
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut discard = io::DiscardingWriter::init();
    match discard.writeAll(&b"abcdef") {
        !ok => { _ = ok; },
        error! => { return process::exit(1)!; },
    }
    if discard.len() != 6 {
        return process::exit(2)!;
    }

    let mut source = io::FixedBufferReader::init(&b"abcdef");
    let mut limited = io::LimitedReader[io::FixedBufferReader]::init(
        &mut source,
        io::Limit::limited(3),
    );
    let mut copied: [u8; 4] = [0, 0, 0, 0];
    let mut n: usize;
    match limited.read(&mut copied[..]) {
        !value => { n = value; },
        error! => { return process::exit(3)!; },
    }
    if n != 3 {
        return process::exit(4)!;
    }
    if copied[0] != b'a' or copied[1] != b'b' or copied[2] != b'c' {
        return process::exit(5)!;
    }
    match limited.read(&mut copied[..]) {
        !value => { n = value; },
        error! => { return process::exit(6)!; },
    }
    if n != 0 {
        return process::exit(7)!;
    }
    !()
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut storage: [u8; 16] = [0; 16];
    let mut backing = io::FixedBufferWriter::init(&mut storage[..]);
    let mut buffer_storage: [u8; 4] = [0; 4];
    let mut writer = io::BufferedWriter[io::FixedBufferWriter]::init(
        &mut backing,
        &mut buffer_storage[..],
    );

    match writer.writeAll(&b"abc") {
        !ok => { _ = ok; },
        error! => { return process::exit(1)!; },
    }
    if writer.len() != 3 or backing.len() != 0 {
        return process::exit(2)!;
    }

    match writer.writeByte(b'd') {
        !ok => { _ = ok; },
        error! => { return process::exit(3)!; },
    }
    if writer.len() != 4 or backing.len() != 0 {
        return process::exit(4)!;
    }

    match writer.writeAll(&b"efghij") {
        !ok => { _ = ok; },
        error! => { return process::exit(5)!; },
    }
    if writer.len() != 0 or backing.len() != 10 {
        return process::exit(6)!;
    }

    match writer.flush() {
        !ok => { _ = ok; },
        error! => { return process::exit(7)!; },
    }
    if backing.len() != 10 {
        return process::exit(8)!;
    }

    let mut expected: &[u8] = &b"abcdefghij";
    let mut written = backing.written();
    let mut index = 0usize;
    while index < written.len() {
        if written[index] != expected[index] {
            return process::exit(9)!;
        }
        index += 1usize;
    }
    !()
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
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
        Self { inner: io::FixedBufferWriter::init(buffer) }
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

    fn shortWrite(&self) Error {
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut storage: [u8; 16] = [0; 16];
    let mut backing = PartialWriter::init(&mut storage[..]);
    let mut buffer_storage: [u8; 8] = [0; 8];
    let mut writer = io::BufferedWriter[PartialWriter]::init(
        &mut backing,
        &mut buffer_storage[..],
    );

    match writer.writeAll(&b"abcdef") {
        !ok => { _ = ok; },
        error! => { return process::exit(1)!; },
    }
    if writer.len() != 6 or backing.len() != 0 {
        return process::exit(2)!;
    }

    match writer.flush() {
        !ok => { _ = ok; },
        error! => { return process::exit(3)!; },
    }
    if writer.len() != 0 or backing.len() != 6 {
        return process::exit(4)!;
    }

    let expected: &[u8] = &b"abcdef";
    let written = backing.written();
    let mut index = 0usize;
    while index < expected.len() {
        if written[index] != expected[index] {
            return process::exit(5)!;
        }
        index += 1usize;
    }

    let mut direct_storage: [u8; 16] = [0; 16];
    let mut direct_backing = PartialWriter::init(&mut direct_storage[..]);
    let mut direct_buffer_storage: [u8; 4] = [0; 4];
    let mut direct_writer = io::BufferedWriter[PartialWriter]::init(
        &mut direct_backing,
        &mut direct_buffer_storage[..],
    );
    match direct_writer.writeAll(&b"ghijkl") {
        !ok => { _ = ok; },
        error! => { return process::exit(6)!; },
    }
    if direct_writer.len() != 0 or direct_backing.len() != 6 {
        return process::exit(7)!;
    }
    let direct_expected: &[u8] = &b"ghijkl";
    let direct_written = direct_backing.written();
    index = 0usize;
    while index < direct_expected.len() {
        if direct_written[index] != direct_expected[index] {
            return process::exit(8)!;
        }
        index += 1usize;
    }
    !()
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
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
fn emit_exe_std_io_buffered_writer_retains_only_unwritten_bytes_after_failure() {
    let root =
        temp_dir("emit_exe_std_io_buffered_writer_retains_only_unwritten_bytes_after_failure");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::io;
using std::process;
using std::slice;

enum RetryError {
    Injected,
    ShortWrite,
}

struct RetryWriter {
    inner: io::FixedBufferWriter,
    attempt: usize,
}

extend RetryWriter {
    fn init(buffer: &mut [u8]) RetryWriter {
        Self { inner: io::FixedBufferWriter::init(buffer), attempt: 0 }
    }

    fn written(&self) &[u8] {
        self.inner.written()
    }
}

extend RetryWriter : io::Writer {
    type Error = RetryError;

    fn shortWrite(&self) Error {
        RetryError::ShortWrite
    }

    fn write(&mut self, bytes: &[u8]) Error!usize {
        if self.attempt == 1usize {
            self.attempt += 1usize;
            return RetryError::Injected!;
        }
        self.attempt += 1usize;
        let mut count = bytes.len();
        if count > 2usize {
            count = 2usize;
        }
        match self.inner.write(&bytes[0..count]) {
            !written => { !written },
            error! => { _ = error; RetryError::ShortWrite! },
        }
    }
}

struct ZeroWriter {
    attempts: usize,
}

extend ZeroWriter : io::Writer {
    type Error = RetryError;

    fn shortWrite(&self) Error {
        RetryError::ShortWrite
    }

    fn write(&mut self, bytes: &[u8]) Error!usize {
        _ = bytes;
        self.attempts += 1usize;
        !0usize
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut storage: [u8; 16] = [0; 16];
    let mut backing = RetryWriter::init(&mut storage[..]);
    let mut bufferedStorage: [u8; 8] = [0; 8];
    let mut writer = io::BufferedWriter[RetryWriter]::init(
        &mut backing,
        &mut bufferedStorage[..],
    );
    match writer.writeAll(&b"abcdef") {
        !ok => { _ = ok; },
        error! => { _ = error; return process::exit(1)!; },
    }
    match writer.flush() {
        !ok => { _ = ok; return process::exit(2)!; },
        RetryError::Injected! => {},
        RetryError::ShortWrite! => { return process::exit(3)!; },
    }
    if writer.len() != 4usize
        or not writer.buffered().equals(&b"cdef")
        or not backing.written().equals(&b"ab")
    {
        return process::exit(4)!;
    }
    match writer.flush() {
        !ok => { _ = ok; },
        error! => { _ = error; return process::exit(5)!; },
    }
    if writer.len() != 0usize or not backing.written().equals(&b"abcdef") {
        return process::exit(6)!;
    }

    let mut zero = ZeroWriter { attempts: 0 };
    let mut zeroStorage: [u8; 4] = [0; 4];
    let mut stalled = io::BufferedWriter[ZeroWriter]::init(&mut zero, &mut zeroStorage[..]);
    match stalled.writeAll(&b"xy") {
        !ok => { _ = ok; },
        error! => { _ = error; return process::exit(7)!; },
    }
    match stalled.flush() {
        !ok => { _ = ok; return process::exit(8)!; },
        RetryError::ShortWrite! => {},
        RetryError::Injected! => { return process::exit(9)!; },
    }
    if stalled.len() != 2usize or not stalled.buffered().equals(&b"xy") or zero.attempts != 1usize {
        return process::exit(10)!;
    }
    !()
}
"#,
    )
    .expect("write buffered writer recovery source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe buffered writer recovery");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run buffered writer recovery executable");
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut source = io::FixedBufferReader::init(&b"abcdefghij");
    let mut buffer_storage: [u8; 4] = [0; 4];
    let mut reader = io::BufferedReader[io::FixedBufferReader]::init(
        &mut source,
        &mut buffer_storage[..],
    );

    let mut first: [u8; 2] = [0; 2];
    let mut n: usize;
    match reader.read(&mut first[..]) {
        !value => { n = value; },
        error! => { return process::exit(1)!; },
    }
    if n != 2 or first[0] != b'a' or first[1] != b'b' {
        return process::exit(2)!;
    }
    if reader.len() != 2 {
        return process::exit(3)!;
    }

    let mut second: [u8; 3] = [0; 3];
    match reader.read(&mut second[..]) {
        !value => { n = value; },
        error! => { return process::exit(4)!; },
    }
    if n != 2 or second[0] != b'c' or second[1] != b'd' {
        return process::exit(5)!;
    }
    if reader.len() != 0 {
        return process::exit(6)!;
    }

    let mut third: [u8; 5] = [0; 5];
    match reader.read(&mut third[..]) {
        !value => { n = value; },
        error! => { return process::exit(7)!; },
    }
    if n != 5 {
        return process::exit(8)!;
    }
    if third[0] != b'e' or third[1] != b'f' or third[2] != b'g' or third[3] != b'h' or third[4] != b'i' {
        return process::exit(9)!;
    }

    let mut fourth: [u8; 2] = [0; 2];
    match reader.read(&mut fourth[..]) {
        !value => { n = value; },
        error! => { return process::exit(10)!; },
    }
    if n != 1 or fourth[0] != b'j' {
        return process::exit(11)!;
    }

    match reader.read(&mut fourth[..]) {
        !value => { n = value; },
        error! => { return process::exit(12)!; },
    }
    if n != 0 {
        return process::exit(13)!;
    }
    !()
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
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
        Self { inner: io::FixedBufferReader::init(bytes) }
    }
}

extend PartialReader : io::Reader {
    type Error = io::BufferError;

    fn endOfStream(&self) Error {
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut source = PartialReader::init(&b"abcdef");
    let mut bytes: [u8; 6] = [0; 6];
    match source.readExact(&mut bytes[..]) {
        !ok => { _ = ok; },
        error! => { return process::exit(1)!; },
    }
    let expected: &[u8] = &b"abcdef";
    let mut index = 0usize;
    while index < expected.len() {
        if bytes[index] != expected[index] {
            return process::exit(2)!;
        }
        index += 1usize;
    }

    let mut short = PartialReader::init(&b"xy");
    let mut too_many: [u8; 3] = [0; 3];
    match short.readExact(&mut too_many[..]) {
        !ok => { _ = ok;
                return process::exit(3)!; },
        error! => { },
    }
    !()
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
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
fn emit_exe_std_io_rejects_invalid_transfer_counts() {
    let root = temp_dir("emit_exe_std_io_rejects_invalid_transfer_counts");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::io;
using std::process;

enum TransferError {
    End,
    Short,
}

struct BadReader {
    marker: bool,
}

extend BadReader : io::Reader {
    type Error = TransferError;

    fn endOfStream(&self) Error {
        TransferError::End
    }

    fn read(&mut self, bytes: &mut [u8]) Error!usize {
        !(bytes.len() + 1usize)
    }
}

struct BadWriter {
    marker: bool,
}

extend BadWriter : io::Writer {
    type Error = TransferError;

    fn shortWrite(&self) Error {
        TransferError::Short
    }

    fn write(&mut self, bytes: &[u8]) Error!usize {
        !(bytes.len() + 1usize)
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;

    let mut reader = BadReader { marker: false };
    let mut destination: [u8; 2] = [0; 2];
    match reader.readExact(&mut destination[..]) {
        !ok => { _ = ok; return process::exit(1)!; },
        TransferError::End! => {},
        TransferError::Short! => { return process::exit(10)!; },
    }

    let mut bufferedStorage: [u8; 4] = [0; 4];
    let mut bufferedReader = io::BufferedReader[BadReader]::init(
        &mut reader,
        &mut bufferedStorage[..],
    );
    match bufferedReader.read(&mut destination[..]) {
        !ok => { _ = ok; return process::exit(2)!; },
        TransferError::End! => {},
        TransferError::Short! => { return process::exit(11)!; },
    }
    if bufferedReader.len() != 0 {
        return process::exit(3)!;
    }

    let mut limitedReader = io::LimitedReader[BadReader]::init(
        &mut reader,
        io::Limit::limited(2usize),
    );
    match limitedReader.read(&mut destination[..]) {
        !ok => { _ = ok; return process::exit(4)!; },
        TransferError::End! => {},
        TransferError::Short! => { return process::exit(12)!; },
    }
    match limitedReader.remaining() {
        ?remaining => {
            if remaining != 2usize {
                return process::exit(5)!;
            }
        },
        null => {
            return process::exit(6)!;
        },
    }

    let mut writer = BadWriter { marker: false };
    match writer.writeAll(&b"ab") {
        !ok => { _ = ok; return process::exit(7)!; },
        TransferError::Short! => {},
        TransferError::End! => { return process::exit(13)!; },
    }

    let mut directBacking = BadWriter { marker: false };
    let mut directBuffer: [u8; 1] = [0];
    let mut direct = io::BufferedWriter[BadWriter]::init(
        &mut directBacking,
        &mut directBuffer[..],
    );
    match direct.write(&b"ab") {
        !ok => { _ = ok; return process::exit(18)!; },
        TransferError::End! => { return process::exit(19)!; },
        TransferError::Short! => {},
    }

    let mut bufferedWriterStorage: [u8; 4] = [0; 4];
    let mut bufferedWriter = io::BufferedWriter[BadWriter]::init(
        &mut writer,
        &mut bufferedWriterStorage[..],
    );
    match bufferedWriter.writeAll(&b"ab") {
        !ok => { _ = ok; },
        TransferError::End! => { return process::exit(15)!; },
        TransferError::Short! => { return process::exit(16)!; },
    }
    match bufferedWriter.flush() {
        !ok => { _ = ok; return process::exit(8)!; },
        TransferError::Short! => {},
        TransferError::End! => { return process::exit(17)!; },
    }
    if bufferedWriter.len() != 2usize {
        return process::exit(9)!;
    }
    !()
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe invalid transfer counts");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run invalid transfer count executable");
    assert_eq!(status.code(), Some(0));
}
