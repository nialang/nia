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
using std::io;
using std::fmt;
using std::process;

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
using std::io;
using std::fmt;
using std::process;

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
using std;
using std::fmt;
using std::io;
using std::mem;
using std::process;

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
    let max_u128 = u128::MAX;
    let ok = true;
    let ch = 'λ';
    let byte = 171u8;
    let neg = -171i32;
    let numbers: [3]i32 = [4, 5, 6];
    stdout.print("list={} slice={} total={} signed={} wide={} max_u128={} ok={} ch={} hex={:x} HEX={:X} bin={:b} oct={:o} neg_hex={:x}\n", [
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
    var raw: [256]u8 = [_]u8[0; 256];
    var stdout = io::FileWriter::stdout(init.io(), raw);
    let value: i32 = 7;
    let neg: i32 = -7;
    let byte = 171u8;
    let text = "nia";
    let ch = 'λ';
    let flag = true;
    stdout.print("r='{:>5}' l='{:<5}' c='{:^5}' z='{:05}' plus='{:+}' plusw='{:+5}' plusz='{:+05}' nz='{:05}' hx='{:08x}' alt='{:#x}' altw='{:#08x}' bin='{:#b}' oct='{:#o}' text='{:>5}' ch='{:<3}' bool='{:>6}' hex='{:x}'\n", [
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
        &text[..],
        &ch,
        &flag,
        &byte,
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
        "r='    7' l='7    ' c='  7  ' z='00007' plus='+7' plusw='   +7' plusz='+0007' nz='-0007' hx='000000ab' alt='0xab' altw='0x0000ab' bin='0b10101011' oct='0253' text='  nia' ch='λ  ' bool='  true' hex='ab'\n"
    );
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
    switch result {
        !ok => {
            _ = ok;
            false
        },
        error! => error == expected,
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var storage: [32]u8 = [0; 32];
    var writer = io::FixedBufferWriter::init(&mut storage[..]);
    let value = 7;

    if not expect_error(writer.print("{}", []), fmt::Error::MissingArgument) {
        return (1 as process::ExitCode)!;
    }
    if not expect_error(writer.print("", [&value]), fmt::Error::ExtraArgument) {
        return (2 as process::ExitCode)!;
    }
    if not expect_error(writer.print("{", []), fmt::Error::InvalidTemplate) {
        return (3 as process::ExitCode)!;
    }
    if not expect_error(writer.print("}", []), fmt::Error::InvalidTemplate) {
        return (4 as process::ExitCode)!;
    }
    switch writer.print("{{{}}}", [&value]) {
        !ok => _ = ok,
        error! => return (5 as process::ExitCode)!,
    }
    if writer.len() != 3 {
        return (6 as process::ExitCode)!;
    }
    let written = writer.written();
    if written[0] != b'{' or written[1] != b'7' or written[2] != b'}' {
        return (7 as process::ExitCode)!;
    }
    if not expect_error(writer.print("{q}", [&value]), fmt::Error::InvalidTemplate) {
        return (8 as process::ExitCode)!;
    }
    let flag = true;
    if not expect_error(writer.print("{x}", [&flag]), fmt::Error::InvalidTemplate) {
        return (9 as process::ExitCode)!;
    }
    if not expect_error(writer.print("{x}", [&value]), fmt::Error::InvalidTemplate) {
        return (10 as process::ExitCode)!;
    }
    switch fmt::print_unchecked(&mut writer, "{:X}", [&value]) {
        !ok => _ = ok,
        error! => return (11 as process::ExitCode)!,
    }
    if not expect_error(writer.print("{:q}", [&value]), fmt::Error::InvalidTemplate) {
        return (12 as process::ExitCode)!;
    }
    if not expect_error(writer.print("{:08", [&value]), fmt::Error::InvalidTemplate) {
        return (13 as process::ExitCode)!;
    }
    let byte = 7u8;
    if not expect_error(writer.print("{:+}", [&byte]), fmt::Error::InvalidTemplate) {
        return (14 as process::ExitCode)!;
    }
    if not expect_error(writer.print("{:+}", [&flag]), fmt::Error::InvalidTemplate) {
        return (15 as process::ExitCode)!;
    }
    if not expect_error(writer.print("{:#}", [&value]), fmt::Error::InvalidTemplate) {
        return (16 as process::ExitCode)!;
    }
    if not expect_error(writer.print("{:#}", [&flag]), fmt::Error::InvalidTemplate) {
        return (17 as process::ExitCode)!;
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
    switch result {
        !value => value == expected,
        error! => {
            _ = error;
            false
        },
    }
}

fn expect_error_i32(result: fmt::ParseError!i32, expected: fmt::ParseError) bool {
    switch result {
        !value => {
            _ = value;
            false
        },
        error! => error == expected,
    }
}

fn expect_error_u8(result: fmt::ParseError!u8, expected: fmt::ParseError) bool {
    switch result {
        !value => {
            _ = value;
            false
        },
        error! => error == expected,
    }
}

fn expect_u8(result: fmt::ParseError!u8, expected: u8) bool {
    switch result {
        !value => value == expected,
        error! => {
            _ = error;
            false
        },
    }
}

fn expect_error_bool(result: fmt::ParseError!bool, expected: fmt::ParseError) bool {
    switch result {
        !value => {
            _ = value;
            false
        },
        error! => error == expected,
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;

    if not expect_i32(fmt::parse[i32]("-2147483648"), i32::MIN) {
        return (1 as process::ExitCode)!;
    }
    if not expect_i32([i32]::parse("+2147483647"), i32::MAX) {
        return (2 as process::ExitCode)!;
    }
    switch fmt::parse[u128]("340282366920938463463374607431768211455") {
        !value => if value != u128::MAX {
            return (3 as process::ExitCode)!;
        },
        error! => return (4 as process::ExitCode)!,
    }
    switch fmt::parse[usize]("12345") {
        !value => if value != 12345usize {
            return (5 as process::ExitCode)!;
        },
        error! => return (6 as process::ExitCode)!,
    }
    switch fmt::parse[bool]("false") {
        !value => if value {
            return (7 as process::ExitCode)!;
        },
        error! => return (8 as process::ExitCode)!,
    }

    if not expect_error_i32(fmt::parse[i32](""), fmt::ParseError::Empty) {
        return (9 as process::ExitCode)!;
    }
    if not expect_error_i32(fmt::parse[i32]("-"), fmt::ParseError::InvalidDigit) {
        return (10 as process::ExitCode)!;
    }
    if not expect_error_i32(fmt::parse[i32]("12x"), fmt::ParseError::InvalidDigit) {
        return (11 as process::ExitCode)!;
    }
    if not expect_error_i32(fmt::parse[i32]("2147483648"), fmt::ParseError::Overflow) {
        return (12 as process::ExitCode)!;
    }
    if not expect_error_u8(fmt::parse[u8]("-1"), fmt::ParseError::InvalidSign) {
        return (13 as process::ExitCode)!;
    }
    if not expect_error_u8(fmt::parse[u8]("256"), fmt::ParseError::Overflow) {
        return (14 as process::ExitCode)!;
    }
    if not expect_u8(fmt::parse_radix[u8]("ff", 16u32), 255u8) {
        return (15 as process::ExitCode)!;
    }
    if not expect_u8([u8]::parse_radix("10101010", 2u32), 170u8) {
        return (16 as process::ExitCode)!;
    }
    if not expect_i32(fmt::parse_radix[i32]("-7B", 16u32), -123) {
        return (17 as process::ExitCode)!;
    }
    if not expect_error_u8(fmt::parse_radix[u8]("2", 2u32), fmt::ParseError::InvalidDigit) {
        return (18 as process::ExitCode)!;
    }
    if not expect_error_u8(fmt::parse_radix[u8]("10", 1u32), fmt::ParseError::InvalidRadix) {
        return (19 as process::ExitCode)!;
    }
    if not expect_error_bool(fmt::parse_radix[bool]("true", 10u32), fmt::ParseError::InvalidRadix) {
        return (20 as process::ExitCode)!;
    }
    switch fmt::parse_radix[u128]("ffffffffffffffffffffffffffffffff", 16u32) {
        !value => if value != u128::MAX {
            return (21 as process::ExitCode)!;
        },
        error! => return (22 as process::ExitCode)!,
    }
    switch fmt::parse_radix[u128]("100000000000000000000000000000000", 16u32) {
        !value => {
            _ = value;
            return (23 as process::ExitCode)!;
        },
        error! => if error != fmt::ParseError::Overflow {
            return (24 as process::ExitCode)!;
        },
    }
    if not expect_error_u8(fmt::parse[u8]("+1"), fmt::ParseError::InvalidSign) {
        return (25 as process::ExitCode)!;
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
using std::io;
using std::process;

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
using std::io;
using std::process;

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
using std::io;
using std::process;

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
