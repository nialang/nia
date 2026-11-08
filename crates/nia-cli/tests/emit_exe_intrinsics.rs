// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use nia_test_support::{CommandExt, CommandStatusExt, test_dir as temp_dir};

#[test]
fn float_bit_access_and_classification_follow_ieee754_fields() {
    let root = temp_dir("float_bit_access_and_classification_follow_ieee754_fields");
    let source = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &source,
        r#"
using std::{math::float, process};
using std::parse::float as floatParse;
using std::{fmt, io};

fun parsesNegativeZero(input: &[u8]) bool {
    match floatParse::parse64(input) {
        !value => float::toBits64(value) == 0x8000000000000000u64,
        error! => { _ = error; false },
    }
}

fun parsesOneTenth(input: &[u8]) bool {
    match floatParse::parse64(input) {
        !value => float::toBits64(value) == 0x3fb999999999999au64,
        error! => { _ = error; false },
    }
}

fun parses32As(input: &[u8], bits: u32) bool {
    match floatParse::parse32(input) {
        !value => float::toBits32(value) == bits,
        error! => { _ = error; false },
    }
}

fun parses64As(input: &[u8], bits: u64) bool {
    match floatParse::parse64(input) {
        !value => float::toBits64(value) == bits,
        error! => { _ = error; false },
    }
}

fun rejectsFloat(input: &[u8]) bool {
    match floatParse::parse64(input) {
        !value => { _ = value; false },
        floatParse::ParseError::Invalid! => true,
        floatParse::ParseError::Empty! => true,
        floatParse::ParseError::Overflow! => true,
        error! => { _ = error; false },
    }
}

fun overflows64(input: &[u8]) bool {
    match floatParse::parse64(input) {
        !value => { _ = value; false },
        floatParse::ParseError::Overflow! => true,
        error! => { _ = error; false },
    }
}

fun writeFloat(value: f64, buffer: &mut [u8]) fmt::Error!usize {
    let mut output = io::FixedBufferWriter::init(buffer);
    let writer: &mut fmt::FormatWriter = &mut output;
    let mut formatter = fmt::Formatter::init(writer);
    formatter.writeFloat64(value).?;
    !output.len()
}

fun writeFloat32(value: f32, buffer: &mut [u8]) fmt::Error!usize {
    let mut output = io::FixedBufferWriter::init(buffer);
    let writer: &mut fmt::FormatWriter = &mut output;
    let mut formatter = fmt::Formatter::init(writer);
    formatter.writeFloat32(value).?;
    !output.len()
}

fun writeFloatSpec(
    value: f64,
    precision: ?usize,
    alternate: bool,
    width: usize,
    fill: char,
    signPlus: bool,
    buffer: &mut [u8],
) fmt::Error!usize {
    let mut output = io::FixedBufferWriter::init(buffer);
    let writer: &mut fmt::FormatWriter = &mut output;
    let mut formatter = fmt::Formatter::init(writer);
    let spec = fmt::FormatSpec::init(
        fmt::FormatPresentation::Display,
        width,
        precision,
        fill,
        fmt::FormatAlignment::Right,
        signPlus,
        alternate,
    );
    value.formatSpec(&mut formatter, spec).?;
    !output.len()
}

fun writeFloat32Spec(
    value: f32,
    precision: ?usize,
    alternate: bool,
    buffer: &mut [u8],
) fmt::Error!usize {
    let mut output = io::FixedBufferWriter::init(buffer);
    let writer: &mut fmt::FormatWriter = &mut output;
    let mut formatter = fmt::Formatter::init(writer);
    let spec = fmt::FormatSpec::init(
        fmt::FormatPresentation::Display,
        0usize,
        precision,
        ' ',
        fmt::FormatAlignment::None,
        false,
        alternate,
    );
    value.formatSpec(&mut formatter, spec).?;
    !output.len()
}

fun floatSpecEquals(
    value: f64,
    precision: ?usize,
    alternate: bool,
    expected: &[u8],
    buffer: &mut [u8],
) bool {
    let length = match writeFloatSpec(value, precision, alternate, 0, ' ', false, buffer) {
        !length => length,
        error! => { _ = error; return false; },
    };
    if length != expected.len() { return false; }
    let mut index: usize = 0;
    while index < length {
        if buffer[index] != expected[index] { return false; }
        index += 1;
    }
    true
}

fun rejectsUnrepresentableFloatPrecision(buffer: &mut [u8]) bool {
    let mut output = io::FixedBufferWriter::init(buffer);
    let writer: &mut fmt::FormatWriter = &mut output;
    let mut formatter = fmt::Formatter::init(writer);
    let spec = fmt::FormatSpec::init(
        fmt::FormatPresentation::Display,
        0usize,
        ?usize::MAX,
        ' ',
        fmt::FormatAlignment::None,
        false,
        false,
    );
    match 1.0f64.formatSpec(&mut formatter, spec) {
        !() => false,
        fmt::Error::InvalidTemplate! => output.len() == 0,
        error! => { _ = error; false },
    }
}

fun float64RoundTrips(bits: u64, buffer: &mut [u8]) bool {
    let value = float::fromBits64(bits);
    let length = match writeFloat(value, buffer) {
        !length => length,
        error! => { _ = error; return false; },
    };
    match floatParse::parse64(&buffer[0..length]) {
        !parsed => float::toBits64(parsed) == bits,
        error! => { _ = error; false },
    }
}

fun float32RoundTrips(bits: u32, buffer: &mut [u8]) bool {
    let value = float::fromBits32(bits);
    let length = match writeFloat32(value, buffer) {
        !length => length,
        error! => { _ = error; return false; },
    };
    match floatParse::parse32(&buffer[0..length]) {
        !parsed => float::toBits32(parsed) == bits,
        error! => { _ = error; false },
    }
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    if not parsesNegativeZero(b"-0e999999") {
        return process::ExitCode(9)!;
    }
    if not parsesOneTenth(b"0.1") {
        return process::ExitCode(10)!;
    }
    if not parses64As(b"1.00000000000000011102230246251565404236316680908203125", 0x3ff0000000000000u64) {
        return process::ExitCode(12)!;
    }
    if not parses64As(b"1.00000000000000033306690738754696212708950042724609375", 0x3ff0000000000002u64) {
        return process::ExitCode(13)!;
    }
    if not parses64As(b"2.5e-324", 1u64) {
        return process::ExitCode(14)!;
    }
    if not parses64As(b"2.4703282292062327e-324", 0u64)
        or not parses64As(b"2.4703282292062328e-324", 1u64)
    {
        return process::ExitCode(25)!;
    }
    if not parses64As(b"2.2250738585072011e-308", 0x000fffffffffffffu64)
        or not parses64As(b"2.2250738585072012e-308", 0x0010000000000000u64)
    {
        return process::ExitCode(26)!;
    }
    if not parses64As(b"1.7976931348623157e308", 0x7fefffffffffffffu64) {
        return process::ExitCode(15)!;
    }
    if not parses32As(b"0.1", 0x3dcccccdu32) {
        return process::ExitCode(16)!;
    }
    if not parses32As(b"1.000000059604644775390625", 0x3f800000u32)
        or not parses32As(b"1.000000178813934326171875", 0x3f800002u32)
    {
        return process::ExitCode(33)!;
    }
    if not parses32As(b"1.40129846e-45", 1u32)
        or not parses32As(b"-1.40129846e-45", 0x80000001u32)
    {
        return process::ExitCode(27)!;
    }
    if not parses32As(b"7.0e-46", 0u32)
        or not parses32As(b"7.1e-46", 1u32)
        or not parses32As(b"9e-46", 1u32)
        or not parses32As(b"-9e-46", 0x80000001u32)
    {
        return process::ExitCode(29)!;
    }
    if not parses64As(
        b"1.00000000000000011102230246251565404236316680908203125",
        0x3ff0000000000000u64,
    ) or not parses64As(
        b"1.00000000000000033306690738754696212708950042724609375",
        0x3ff0000000000002u64,
    ) or not parses64As(b"2.4703282292062327e-324", 0u64)
        or not parses64As(b"2.4703282292062328e-324", 1u64)
    {
        return process::ExitCode(30)!;
    }
    if not parses32As(b"3.4028235e38", 0x7f7fffffu32) {
        return process::ExitCode(31)!;
    }
    if not rejectsFloat(b"+1") or not rejectsFloat(b"NaN") or not rejectsFloat(b" 1") {
        return process::ExitCode(17)!;
    }
    match floatParse::parse64(b"1.7976931348623159e308") {
        !value => { _ = value; return process::ExitCode(18)!; },
        floatParse::ParseError::Overflow! => {},
        error! => { _ = error; return process::ExitCode(19)!; },
    }
    if not parses64As(b"1e-999999999999999999999", 0u64)
        or not overflows64(b"1e999999999999999999999")
    {
        return process::ExitCode(28)!;
    }
    let mut rendered: [u8; 64] = [0; 64];
    let renderedLen = writeFloat(0.1, &mut rendered[..]).?;
    if renderedLen != 3usize or rendered[0] != b'0' or rendered[1] != b'.' or rendered[2] != b'1' {
        return process::ExitCode(20)!;
    }
    rendered[0] = 0;
    rendered[1] = 0;
    rendered[2] = 0;
    let rendered32Len = writeFloat32(0.1f32, &mut rendered[..]).?;
    if rendered32Len != 3usize or rendered[0] != b'0' or rendered[1] != b'.' or rendered[2] != b'1' {
        return process::ExitCode(21)!;
    }
    let float64Cases: [u64; 14] = [
        0u64,
        0x8000000000000000u64,
        1u64,
        2u64,
        0x000fffffffffffffu64,
        0x0010000000000000u64,
        0x3fefffffffffffffu64,
        0x3ff0000000000000u64,
        0x3ff0000000000001u64,
        0x4340000000000000u64,
        0x7fdfffffffffffffu64,
        0x7fe0000000000000u64,
        0x7feffffffffffffeu64,
        0x7fefffffffffffffu64,
    ];
    let mut floatIndex: usize = 0;
    while floatIndex < float64Cases.len() {
        if not float64RoundTrips(float64Cases[floatIndex], &mut rendered[..]) {
            return process::ExitCode(40 + floatIndex as i32)!;
        }
        floatIndex += 1;
    }
    let float32Cases: [u32; 14] = [
        0u32,
        0x80000000u32,
        1u32,
        2u32,
        0x007fffffu32,
        0x00800000u32,
        0x3f7fffffu32,
        0x3f800000u32,
        0x3f800001u32,
        0x4b800000u32,
        0x7effffffu32,
        0x7f000000u32,
        0x7f7ffffeu32,
        0x7f7fffffu32,
    ];
    floatIndex = 0;
    while floatIndex < float32Cases.len() {
        if not float32RoundTrips(float32Cases[floatIndex], &mut rendered[..]) {
            return process::ExitCode(60 + floatIndex as i32)!;
        }
        floatIndex += 1;
    }
    let sampled64: [u64; 8] = [
        0x722806b1effed2dbu64,
        0x7c6a94a55ab2f9beu64,
        0x28aa0490841433cdu64,
        0x608c08e8b7bd48eau64,
        0x19047d19b16b48fbu64,
        0x5ccf462b3090c0aau64,
        0x1c80a1d04d828babu64,
        0x32ca64acd436047cu64,
    ];
    floatIndex = 0;
    while floatIndex < sampled64.len() {
        if not float64RoundTrips(sampled64[floatIndex], &mut rendered[..]) {
            return process::ExitCode(80 + floatIndex as i32)!;
        }
        floatIndex += 1;
    }
    let sampled32: [u32; 8] = [
        0x6f7ed2dbu32,
        0x5a32f9beu32,
        0x041433cdu32,
        0x373d48eau32,
        0x316b48fbu32,
        0x3010c0aau32,
        0x4d028babu32,
        0x5436047cu32,
    ];
    floatIndex = 0;
    while floatIndex < sampled32.len() {
        if not float32RoundTrips(sampled32[floatIndex], &mut rendered[..]) {
            return process::ExitCode(90 + floatIndex as i32)!;
        }
        floatIndex += 1;
    }
    let minSubnormalLen = writeFloat(float::fromBits64(1u64), &mut rendered[..]).?;
    if minSubnormalLen != 6usize
        or rendered[0] != b'5'
        or rendered[1] != b'e'
        or rendered[2] != b'-'
        or rendered[3] != b'3'
        or rendered[4] != b'2'
        or rendered[5] != b'4'
    {
        return process::ExitCode(24)!;
    }
    let minSubnormal32Len = writeFloat32(float::fromBits32(1u32), &mut rendered[..]).?;
    if minSubnormal32Len != 5usize
        or rendered[0] != b'1'
        or rendered[1] != b'e'
        or rendered[2] != b'-'
        or rendered[3] != b'4'
        or rendered[4] != b'5'
    {
        return process::ExitCode(32)!;
    }
    let largePowerLen = writeFloat(1e20, &mut rendered[..]).?;
    if largePowerLen != 4usize
        or rendered[0] != b'1'
        or rendered[1] != b'e'
        or rendered[2] != b'2'
        or rendered[3] != b'0'
    {
        return process::ExitCode(34)!;
    }
    if not floatSpecEquals(1.25, ?1usize, false, b"1.2", &mut rendered[..])
        or not floatSpecEquals(1.375, ?2usize, false, b"1.38", &mut rendered[..])
        or not floatSpecEquals(2.5, ?0usize, false, b"2", &mut rendered[..])
        or not floatSpecEquals(3.5, ?0usize, false, b"4", &mut rendered[..])
        or not floatSpecEquals(0.5, ?0usize, false, b"0", &mut rendered[..])
        or not floatSpecEquals(1.5, ?0usize, false, b"2", &mut rendered[..])
        or not floatSpecEquals(-0.5, ?0usize, false, b"-0", &mut rendered[..])
        or not floatSpecEquals(float::fromBits64(1u64), ?0usize, false, b"0", &mut rendered[..])
        or not floatSpecEquals(0.05, ?1usize, false, b"0.1", &mut rendered[..])
        or not floatSpecEquals(0.15, ?1usize, false, b"0.1", &mut rendered[..])
        or not floatSpecEquals(9.999, ?2usize, false, b"10.00", &mut rendered[..])
        or not floatSpecEquals(-0.125, ?2usize, false, b"-0.12", &mut rendered[..])
        or not floatSpecEquals(12.0, null, true, b"12.", &mut rendered[..])
        or not floatSpecEquals(1e20, null, true, b"1.e20", &mut rendered[..])
        or not floatSpecEquals(12.0, ?0usize, true, b"12.", &mut rendered[..])
        or not floatSpecEquals(12.0, ?3usize, true, b"12.000", &mut rendered[..])
        or not floatSpecEquals(0.0, ?0usize, true, b"0.", &mut rendered[..])
    {
        return process::ExitCode(35)!;
    }
    if not rejectsUnrepresentableFloatPrecision(&mut rendered[..]) {
        return process::ExitCode(38)!;
    }
    let signedPaddedLen = writeFloatSpec(1.2, ?2usize, false, 7, '0', true, &mut rendered[..]).?;
    if signedPaddedLen != 7usize
        or rendered[0] != b'+'
        or rendered[1] != b'0'
        or rendered[2] != b'0'
        or rendered[3] != b'1'
        or rendered[4] != b'.'
        or rendered[5] != b'2'
        or rendered[6] != b'0'
    {
        return process::ExitCode(36)!;
    }
    let rounded32Len = writeFloat32Spec(1.375f32, ?2usize, false, &mut rendered[..]).?;
    if rounded32Len != 4usize
        or rendered[0] != b'1'
        or rendered[1] != b'.'
        or rendered[2] != b'3'
        or rendered[3] != b'8'
    {
        return process::ExitCode(37)!;
    }
    let rounded32IntegerLen = writeFloat32Spec(2.5f32, ?0usize, false, &mut rendered[..]).?;
    if rounded32IntegerLen != 1usize or rendered[0] != b'2' {
        return process::ExitCode(39)!;
    }
    let negativeZero32 = float::fromBits32(0x80000000u32);
    if float::toBits32(negativeZero32) != 0x80000000u32 or not float::signBit32(negativeZero32) {
        return process::ExitCode(1)!;
    }
    let subnormal32 = float::fromBits32(1u32);
    if not float::isFinite32(subnormal32) or float::isNan32(subnormal32) or float::isInfinite32(subnormal32) {
        return process::ExitCode(2)!;
    }
    let infinity32 = float::fromBits32(0x7f800000u32);
    if not float::isInfinite32(infinity32) or float::isFinite32(infinity32) or float::isNan32(infinity32) {
        return process::ExitCode(3)!;
    }
    let nan32 = float::fromBits32(0x7fc00000u32);
    if not float::isNan32(nan32) or float::isFinite32(nan32) or float::isInfinite32(nan32) {
        return process::ExitCode(4)!;
    }

    let negativeZero64 = float::fromBits64(0x8000000000000000u64);
    if float::toBits64(negativeZero64) != 0x8000000000000000u64 or not float::signBit64(negativeZero64) {
        return process::ExitCode(5)!;
    }
    let subnormal64 = float::fromBits64(1u64);
    if not float::isFinite64(subnormal64) or float::isNan64(subnormal64) or float::isInfinite64(subnormal64) {
        return process::ExitCode(6)!;
    }
    let infinity64 = float::fromBits64(0x7ff0000000000000u64);
    if not float::isInfinite64(infinity64) or float::isFinite64(infinity64) or float::isNan64(infinity64) {
        return process::ExitCode(7)!;
    }
    let nan64 = float::fromBits64(0x7ff8000000000000u64);
    if not float::isNan64(nan64) or float::isFinite64(nan64) or float::isInfinite64(nan64) {
        return process::ExitCode(8)!;
    }
    !()
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&source)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit IEEE-754 bit representation test");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = Command::new(&exe).status_timeout("run IEEE-754 bit representation test");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn float_formatter_validates_before_output_and_propagates_short_writer_failure() {
    let root = temp_dir("float_formatter_writer_contract");
    let source = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &source,
        r#"
using std::{fmt, io, math::float, process};

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut bytes: [u8; 2] = [0; 2];
    let mut output = io::FixedBufferWriter::init(&mut bytes[..]);
    let writer: &mut fmt::FormatWriter = &mut output;
    let mut formatter = fmt::Formatter::init(writer);
    match formatter.writeFloat64(float::fromBits64(0x7ff0000000000000u64)) {
        fmt::Error::InvalidTemplate! => {},
        _ => return process::ExitCode(1)!,
    }
    match formatter.writeFloat32(float::fromBits32(0xffc00000u32)) {
        fmt::Error::InvalidTemplate! => {},
        _ => return process::ExitCode(2)!,
    }
    match formatter.writeFloat64Spec(1.0, fmt::FormatSpec::lowerHex()) {
        fmt::Error::InvalidTemplate! => {},
        _ => return process::ExitCode(3)!,
    }
    if output.len() != 0 { return process::ExitCode(4)!; }
    match formatter.writeFloat64(1.25) {
        fmt::Error::Write! => {},
        _ => return process::ExitCode(5)!,
    }
    if output.len() != 2 or bytes[0] != b'1' or bytes[1] != b'.' {
        return process::ExitCode(6)!;
    }
    !()
}
"#,
    )
    .expect("write float formatter writer contract");
    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&source)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit float formatter writer contract");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = Command::new(&exe).status_timeout("run float formatter writer contract");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn float_decimal_parser_matches_host_round_trip_samples() {
    use std::fmt::Write as _;

    let root = temp_dir("float_decimal_parser_matches_host_round_trip_samples");
    let source = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    let mut program = String::from(
        r#"
using std::{math::float, process};
using std::parse::float as floatParse;
using std::{fmt, io};

fun matches64(input: &[u8], expected: u64) bool {
    match floatParse::parse64(input) {
        !value => float::toBits64(value) == expected,
        error! => { _ = error; false },
    }
}

fun matches32(input: &[u8], expected: u32) bool {
    match floatParse::parse32(input) {
        !value => float::toBits32(value) == expected,
        error! => { _ = error; false },
    }
}

fun significantDigitCount(text: &[u8]) usize {
    let mut end = text.len();
    let mut index: usize = 0;
    while index < text.len() {
        if text[index] == b'e' { end = index; break; }
        index += 1;
    }
    let mut first = text.len();
    let mut last: usize = 0;
    index = 0;
    while index < end {
        if text[index] >= b'1' and text[index] <= b'9' {
            if first == text.len() { first = index; }
            last = index;
        }
        index += 1;
    }
    if first == text.len() { return 1; }
    let mut count: usize = 0;
    index = first;
    while index <= last {
        if text[index] >= b'0' and text[index] <= b'9' { count += 1; }
        index += 1;
    }
    count
}

fun shortestDigits64(value: f64) usize {
    let mut bytes: [u8; 800] = [0; 800];
    let mut output = io::FixedBufferWriter::init(&mut bytes[..]);
    let writer: &mut fmt::FormatWriter = &mut output;
    let mut formatter = fmt::Formatter::init(writer);
    match value.format(&mut formatter) {
        !() => {},
        error! => { _ = error; return usize::MAX; },
    }
    significantDigitCount(&bytes[0..output.len()])
}

fun shortestDigits32(value: f32) usize {
    let mut bytes: [u8; 800] = [0; 800];
    let mut output = io::FixedBufferWriter::init(&mut bytes[..]);
    let writer: &mut fmt::FormatWriter = &mut output;
    let mut formatter = fmt::Formatter::init(writer);
    match value.format(&mut formatter) {
        !() => {},
        error! => { _ = error; return usize::MAX; },
    }
    significantDigitCount(&bytes[0..output.len()])
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
"#,
    );

    let mut state = 0x9e3779b97f4a7c15u64;
    let mut index = 0;
    while index < 64 {
        state ^= state << 7;
        state ^= state >> 9;
        state ^= state << 8;
        let value = f64::from_bits(state);
        if value.is_finite() {
            let text = value.to_string();
            let digits = text
                .split(['e', 'E'])
                .next()
                .expect("float mantissa")
                .chars()
                .filter(char::is_ascii_digit)
                .collect::<String>();
            let shortest_digits = digits
                .trim_start_matches('0')
                .trim_end_matches('0')
                .len()
                .max(1);
            writeln!(
                program,
                "    if not matches64(b\"{text}\", 0x{:016x}u64) {{ return process::ExitCode(1)!; }}",
                state
            )
            .expect("append f64 parser sample");
            if index < 8 {
                let exponent_index = text.find(['e', 'E']).unwrap_or(text.len());
                let mantissa = &text[..exponent_index];
                let exponent = if exponent_index == text.len() {
                    0
                } else {
                    text[exponent_index + 1..]
                        .parse::<i32>()
                        .expect("host float exponent")
                };
                let fractional_digits = mantissa
                    .split_once('.')
                    .map_or(0, |(_, fraction)| fraction.len())
                    as i32;
                let coefficient = mantissa.replace('.', "");
                let extended = format!(
                    "{coefficient}{}e{}",
                    "0".repeat(80),
                    exponent - fractional_digits - 80
                );
                writeln!(
                    program,
                    "    if not matches64(b\"{extended}\", 0x{:016x}u64) {{ return process::ExitCode(5)!; }}",
                    state
                )
                .expect("append long-tail f64 parser sample");
            }
            if index < 8 {
                writeln!(
                    program,
                    "    if shortestDigits64(float::fromBits64(0x{:016x}u64)) != {shortest_digits}usize {{ return process::ExitCode(3)!; }}",
                    state
                )
                .expect("append f64 shortestness sample");
            }
        }

        let bits = state as u32;
        let value = f32::from_bits(bits);
        if value.is_finite() {
            let text = value.to_string();
            let digits = text
                .split(['e', 'E'])
                .next()
                .expect("float mantissa")
                .chars()
                .filter(char::is_ascii_digit)
                .collect::<String>();
            let shortest_digits = digits
                .trim_start_matches('0')
                .trim_end_matches('0')
                .len()
                .max(1);
            writeln!(
                program,
                "    if not matches32(b\"{text}\", 0x{:08x}u32) {{ return process::ExitCode(2)!; }}",
                bits
            )
            .expect("append f32 parser sample");
            if index < 8 {
                let exponent_index = text.find(['e', 'E']).unwrap_or(text.len());
                let mantissa = &text[..exponent_index];
                let exponent = if exponent_index == text.len() {
                    0
                } else {
                    text[exponent_index + 1..]
                        .parse::<i32>()
                        .expect("host float exponent")
                };
                let fractional_digits = mantissa
                    .split_once('.')
                    .map_or(0, |(_, fraction)| fraction.len())
                    as i32;
                let coefficient = mantissa.replace('.', "");
                let extended = format!(
                    "{coefficient}{}e{}",
                    "0".repeat(80),
                    exponent - fractional_digits - 80
                );
                writeln!(
                    program,
                    "    if not matches32(b\"{extended}\", 0x{:08x}u32) {{ return process::ExitCode(6)!; }}",
                    bits
                )
                .expect("append long-tail f32 parser sample");
            }
            if index < 8 {
                writeln!(
                    program,
                    "    if shortestDigits32(float::fromBits32(0x{:08x}u32)) != {shortest_digits}usize {{ return process::ExitCode(4)!; }}",
                    bits
                )
                .expect("append f32 shortestness sample");
            }
        }
        index += 1;
    }

    program.push_str("    !()\n}\n");
    std::fs::write(&source, program).expect("write generated float parser test");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&source)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit float parser differential test");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = Command::new(&exe).status_timeout("run float parser differential test");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn float_decimal_parser_fuzzes_random_host_decimals() {
    use std::fmt::Write as _;

    let root = temp_dir("float_decimal_parser_matches_random_host_decimals");
    let source = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    let mut program = String::from(
        r#"
using std::{math::float, process};
using std::parse::float as floatParse;

fun matches64(input: &[u8], expected: u64) bool {
    match floatParse::parse64(input) {
        !value => float::toBits64(value) == expected,
        error! => { _ = error; false },
    }
}

fun matches32(input: &[u8], expected: u32) bool {
    match floatParse::parse32(input) {
        !value => float::toBits32(value) == expected,
        error! => { _ = error; false },
    }
}

fun overflows64(input: &[u8]) bool {
    match floatParse::parse64(input) {
        !value => { _ = value; false },
        floatParse::ParseError::Overflow! => true,
        error! => { _ = error; false },
    }
}

fun overflows32(input: &[u8]) bool {
    match floatParse::parse32(input) {
        !value => { _ = value; false },
        floatParse::ParseError::Overflow! => true,
        error! => { _ = error; false },
    }
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
"#,
    );

    // Generate decimal inputs independently of the compiler under test. The
    // host parser supplies the correctly rounded oracle for both widths.
    let mut state = 0x243f6a8885a308d3u64;
    for index in 0..384usize {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let negative = state & 1 != 0;
        let digit_count = 1 + ((state >> 8) % 19) as usize;
        let mut digits = String::with_capacity(digit_count + 24);
        for digit_index in 0..digit_count {
            state = state
                .wrapping_mul(2862933555777941757)
                .wrapping_add(3037000493);
            let digit = if digit_index == 0 {
                b'1' + ((state >> 17) % 9) as u8
            } else {
                b'0' + ((state >> 17) % 10) as u8
            };
            digits.push(char::from(digit));
        }
        let mut exponent = ((state >> 32) % 801) as i32 - 400;
        if index % 4 == 0 {
            // Equivalent spellings with long discarded tails exercise exact
            // midpoint certification after the fast candidate path.
            digits.push_str(&"0".repeat(24));
            exponent -= 24;
        }
        let mantissa = if digits.len() == 1 {
            digits
        } else {
            format!("{}.{}", &digits[..1], &digits[1..])
        };
        let leading_zeroes = if index % 3 == 0 { "000" } else { "" };
        let text = format!(
            "{}{}{mantissa}e{exponent:+}",
            if negative { "-" } else { "" },
            leading_zeroes,
        );
        let value64 = text.parse::<f64>().expect("host f64 decimal");
        if value64.is_finite() {
            writeln!(
                program,
                "    if not matches64(b\"{text}\", 0x{:016x}u64) {{ return process::ExitCode(1)!; }}",
                value64.to_bits()
            )
            .expect("append random f64 decimal");
        } else {
            writeln!(
                program,
                "    if not overflows64(b\"{text}\") {{ return process::ExitCode(2)!; }}"
            )
            .expect("append random f64 overflow decimal");
        }
        let value32 = text.parse::<f32>().expect("host f32 decimal");
        if value32.is_finite() {
            writeln!(
                program,
                "    if not matches32(b\"{text}\", 0x{:08x}u32) {{ return process::ExitCode(3)!; }}",
                value32.to_bits()
            )
            .expect("append random f32 decimal");
        } else {
            writeln!(
                program,
                "    if not overflows32(b\"{text}\") {{ return process::ExitCode(4)!; }}"
            )
            .expect("append random f32 overflow decimal");
        }
    }

    program.push_str("    !()\n}\n");
    std::fs::write(&source, program).expect("write random float parser test");
    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&source)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit random float parser differential test");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = Command::new(&exe).status_timeout("run random float parser differential test");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn float_fixed_precision_matches_host_formatting_samples() {
    use std::fmt::Write as _;

    let root = temp_dir("float_fixed_precision_matches_host_formatting_samples");
    let source = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    let mut program = String::from(
        r#"
using std::{fmt, io, math::float, process};

fun matches64(value: f64, precision: usize, alternate: bool, expected: &[u8]) bool {
    let mut bytes: [u8; 1600] = [0; 1600];
    let mut output = io::FixedBufferWriter::init(&mut bytes[..]);
    let writer: &mut fmt::FormatWriter = &mut output;
    let mut formatter = fmt::Formatter::init(writer);
    let spec = fmt::FormatSpec::init(
        fmt::FormatPresentation::Display,
        0usize,
        ?precision,
        ' ',
        fmt::FormatAlignment::None,
        false,
        alternate,
    );
    match value.formatSpec(&mut formatter, spec) {
        !() => {},
        error! => { _ = error; return false; },
    }
    if output.len() != expected.len() { return false; }
    let mut index: usize = 0;
    while index < output.len() {
        if bytes[index] != expected[index] { return false; }
        index += 1;
    }
    true
}

fun matches32(value: f32, precision: usize, alternate: bool, expected: &[u8]) bool {
    let mut bytes: [u8; 1600] = [0; 1600];
    let mut output = io::FixedBufferWriter::init(&mut bytes[..]);
    let writer: &mut fmt::FormatWriter = &mut output;
    let mut formatter = fmt::Formatter::init(writer);
    let spec = fmt::FormatSpec::init(
        fmt::FormatPresentation::Display,
        0usize,
        ?precision,
        ' ',
        fmt::FormatAlignment::None,
        false,
        alternate,
    );
    match value.formatSpec(&mut formatter, spec) {
        !() => {},
        error! => { _ = error; return false; },
    }
    if output.len() != expected.len() { return false; }
    let mut index: usize = 0;
    while index < output.len() {
        if bytes[index] != expected[index] { return false; }
        index += 1;
    }
    true
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
"#,
    );

    let mut state = 0x243f6a8885a308d3u64;
    let mut index = 0;
    while index < 40 {
        state ^= state << 7;
        state ^= state >> 9;
        state ^= state << 8;

        let bits64 = (state & 0x800fffffffffffffu64) | ((((state >> 19) % 2046 + 1) & 0x7ff) << 52);
        let value64 = f64::from_bits(bits64);
        let precision = (state as usize) % 9;
        let alternate = (state & 0x100u64) != 0;
        let mut expected64 = if alternate {
            format!("{:#.*}", precision, value64)
        } else {
            format!("{:.*}", precision, value64)
        };
        if alternate && precision == 0 {
            expected64.push('.');
        }
        writeln!(
            program,
            "    if not matches64(float::fromBits64(0x{:016x}u64), {precision}usize, {alternate}, b\"{expected64}\") {{ return process::ExitCode({})!; }}",
            bits64,
            index + 51,
        )
        .expect("append f64 precision case");

        let bits32 =
            (state as u32 & 0x807fffffu32) | ((((state >> 28) as u32 % 254 + 1) & 0xff) << 23);
        let value32 = f32::from_bits(bits32);
        let precision32 = ((state >> 8) as usize) % 9;
        let alternate32 = (state & 0x200u64) != 0;
        let mut expected32 = if alternate32 {
            format!("{:#.*}", precision32, value32)
        } else {
            format!("{:.*}", precision32, value32)
        };
        if alternate32 && precision32 == 0 {
            expected32.push('.');
        }
        writeln!(
            program,
            "    if not matches32(float::fromBits32(0x{:08x}u32), {precision32}usize, {alternate32}, b\"{expected32}\") {{ return process::ExitCode({})!; }}",
            bits32,
            index + 1,
        )
        .expect("append f32 precision case");
        index += 1;
    }

    for (case_index, (bits, precision, alternate)) in [
        (0x3ff4000000000000u64, 1usize, false),
        (0x3ffc000000000000u64, 1usize, false),
        (0xbff4000000000000u64, 1usize, false),
        (0x4004000000000000u64, 0usize, false),
        (0x400e000000000000u64, 0usize, false),
        (0xc00c000000000000u64, 0usize, false),
        (0x4023000000000000u64, 0usize, true),
        (0x4058e00000000000u64, 0usize, false),
        (0x8000000000000000u64, 4usize, false),
        (0x0000000000000000u64, 0usize, true),
        (0x0000000000000001u64, 17usize, false),
        (0x7fefffffffffffffu64, 20usize, false),
        (0x0010000000000000u64, 20usize, true),
        (0x3ff0000000000000u64, 20usize, false),
        (0x0000000000000001u64, 1073usize, false),
        (0x0000000000000001u64, 1074usize, false),
        (0x8000000000000001u64, 1075usize, true),
        (0x000fffffffffffffu64, 1074usize, false),
        (0x0010000000000000u64, 1022usize, true),
        (0x7fefffffffffffffu64, 1074usize, false),
    ]
    .into_iter()
    .enumerate()
    {
        let value = f64::from_bits(bits);
        let mut expected = if alternate {
            format!("{:#.*}", precision, value)
        } else {
            format!("{:.*}", precision, value)
        };
        if alternate && precision == 0 {
            expected.push('.');
        }
        writeln!(
            program,
            "    if not matches64(float::fromBits64(0x{bits:016x}u64), {precision}usize, {alternate}, b\"{expected}\") {{ return process::ExitCode({})!; }}",
            101 + case_index
        )
        .expect("append f64 boundary formatting case");
    }

    for (case_index, (bits, precision, alternate)) in [
        (0x3fa00000u32, 1usize, false),
        (0x3fe00000u32, 1usize, false),
        (0xbfa00000u32, 1usize, false),
        (0x40200000u32, 0usize, false),
        (0x40600000u32, 0usize, false),
        (0xc0600000u32, 0usize, false),
        (0x41180000u32, 0usize, true),
        (0x42c70000u32, 0usize, false),
        (0x80000000u32, 4usize, false),
        (0x00000000u32, 0usize, true),
        (0x00000001u32, 17usize, false),
        (0x7f7fffffu32, 20usize, false),
        (0x00800000u32, 20usize, true),
        (0x3f800000u32, 20usize, false),
        (0x00000001u32, 148usize, false),
        (0x00000001u32, 149usize, false),
        (0x80000001u32, 150usize, true),
        (0x007fffffu32, 149usize, false),
        (0x00800000u32, 126usize, true),
    ]
    .into_iter()
    .enumerate()
    {
        let value = f32::from_bits(bits);
        let mut expected = if alternate {
            format!("{:#.*}", precision, value)
        } else {
            format!("{:.*}", precision, value)
        };
        if alternate && precision == 0 {
            expected.push('.');
        }
        writeln!(
            program,
            "    if not matches32(float::fromBits32(0x{bits:08x}u32), {precision}usize, {alternate}, b\"{expected}\") {{ return process::ExitCode({})!; }}",
            151 + case_index
        )
        .expect("append f32 boundary formatting case");
    }

    program.push_str("    !()\n}\n");
    std::fs::write(&source, program).expect("write float precision differential test");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&source)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit float precision differential test");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = Command::new(&exe).status_timeout("run float precision differential test");
    assert_eq!(
        status.code(),
        Some(0),
        "formatting differential sample failed with case code {:?}",
        status.code()
    );
}

#[test]
fn array_pointer_methods_preserve_storage_and_evaluate_receivers_once() {
    let root = temp_dir("array_pointer_methods_preserve_storage_and_evaluate_receivers_once");
    let source = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &source,
        r#"
using std::process;

type Bytes = [u8; 3];
struct Packet { bytes: Bytes }
struct ConstPacket { bytes: [u8; 3] }
fun make(calls: &mut usize) [i32; 2] {
    calls.* += 1;
    [7, 8]
}
fun index(calls: &mut usize) usize {
    calls.* += 1;
    0
}
fun first[T, N: usize](values: [T; N]) T { values.ptr().* }
const fun localCopy(values: [i32; 2]) i32 {
    let pointer = values.ptrMut();
    values[0] = 8;
    pointer.*
}
const copied: i32 = localCopy([1, 2]);
const fun projected() i32 {
    let mut values: [i32; 2] = [1, 2];
    let pointer = values.ptrMut();
    values[0] = 5;
    let mut packet = ConstPacket { bytes: [1, 2, 3] };
    let bytes = packet.bytes.ptrMut();
    packet.bytes[0] = 4;
    pointer.* + b"a\0".ptr().* as i32 + "a".ptr().* as u32 as i32
        + (&"a").ptr().* as u32 as i32 + bytes.* as i32
}
const result: i32 = projected();

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let values = [1i32, 2];
    let read: &i32 = values.ptr();
    if read as usize != (&values).ptr() as usize or read.* != 1 {
        return process::ExitCode(1)!;
    }
    let mut writable = [3i32, 4];
    let write: &mut i32 = writable.ptrMut();
    write.* = 9;
    if writable[0] != 9 { return process::ExitCode(2)!; }
    let mut packet = Packet { bytes: [1, 2, 3] };
    packet.bytes.ptrMut()[1] = 6;
    if packet.bytes[1] != 6 or packet.bytes.ptr()[0] != 1 {
        return process::ExitCode(3)!;
    }
    let text = b"nia\0".ptr();
    if text[0] != b'n' or text[3] != 0u8 or "nia".ptr()[0] != 'n' {
        return process::ExitCode(4)!;
    }
    let mut calls = 0usize;
    if make(calls).ptr()[1] != 8 or calls != 1 {
        return process::ExitCode(5)!;
    }
    let mut nested = [[1i32, 2], [3, 4]];
    nested[index(calls)].ptrMut().* = 11;
    if nested[0][0] != 11 or calls != 2 {
        return process::ExitCode(6)!;
    }
    let mut mutablePointer = &mut writable;
    mutablePointer.ptrMut().* = 12;
    if writable[0] != 12 or first([13i32, 14]) != 13 or first([true]) != true
        or result != 300 or projected() != result or copied != 8
        or localCopy(values) != copied or values[0] != 1
    { return process::ExitCode(7)!; }
    !()
}
"#,
    )
    .expect("write array pointer-method source");
    let build = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&source)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit array pointer-method executable");
    assert!(
        build.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let status = Command::new(&exe).status_timeout("run array pointer-method executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn runtime_shifts_trap_on_overflow_but_widened_rotations_truncate() {
    let root = temp_dir("runtime_shifts_trap_on_overflow_but_widened_rotations_truncate");
    let rotate_source = root.join("rotate.nia");
    let rotate_exe = root.join(format!("rotate{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &rotate_source,
        r#"
using std::process;

fun rotateLeft(value: u64, amount: u64) u64 {
    (((value as u128) << amount) as u64) | (value >> (64u64 - amount))
}
fun shiftLeft(value: u8) u8 { value << 1u8 }

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    if rotateLeft(0x0123456789abcdefu64, 8u64) != 0x23456789abcdef01u64 {
        return process::ExitCode(1)!;
    }
    if shiftLeft(64u8) != 128u8 {
        return process::ExitCode(1)!;
    }
    !()
}
"#,
    )
    .expect("write widened rotation source");
    let build = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&rotate_source)
        .arg("-o")
        .arg(&rotate_exe)
        .output_timeout_for_build("emit widened rotation semantics executable");
    assert!(
        build.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let status = Command::new(&rotate_exe).status_timeout("run widened rotation semantics");
    assert_eq!(status.code(), Some(0));

    let overflow_source = root.join("overflow.nia");
    let overflow_exe = root.join(format!("overflow{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &overflow_source,
        r#"
using std::process;

fun overflowingShift(value: u8) u8 { value << 1u8 }

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    _ = overflowingShift(128u8);
    !()
}
"#,
    )
    .expect("write overflowing shift source");
    let build = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&overflow_source)
        .arg("-o")
        .arg(&overflow_exe)
        .output_timeout_for_build("emit overflowing shift semantics executable");
    assert!(
        build.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let status = Command::new(&overflow_exe).status_timeout("run overflowing shift semantics");
    assert!(!status.success(), "overflowing left shift must trap");

    let signed_overflow_source = root.join("signed-overflow.nia");
    let signed_overflow_exe = root.join(format!("signed-overflow{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &signed_overflow_source,
        r#"
using std::process;

fun overflowingSignedShift(value: i8) i8 { value << 1i8 }

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    _ = overflowingSignedShift(64i8);
    !()
}
"#,
    )
    .expect("write overflowing signed shift source");
    let build = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&signed_overflow_source)
        .arg("-o")
        .arg(&signed_overflow_exe)
        .output_timeout_for_build("emit overflowing signed shift semantics executable");
    assert!(
        build.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let status =
        Command::new(&signed_overflow_exe).status_timeout("run overflowing signed shift semantics");
    assert!(!status.success(), "signed overflowing left shift must trap");
}

#[test]
fn emit_exe_array_repeat_evaluates_each_element_once() {
    let root = temp_dir("emit_exe_array_repeat_evaluates_each_element_once");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

static mut calls: i32 = 0;

fun next() i32 {
    calls += 1;
    calls
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let values: [i32; 4] = [next(); 4];
    if calls != 4 {
        return process::ExitCode(1)!;
    }
    if values[0] != 1 or values[1] != 2 or values[2] != 3 or values[3] != 4 {
        return process::ExitCode(2)!;
    }

    let empty: [i32; 0] = [next(); 0];
    _ = empty;
    if calls != 4 {
        return process::ExitCode(3)!;
    }
    !()
}
"#,
    )
    .expect("write array repeat evaluation source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit array repeat evaluation executable");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run array repeat evaluation executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_preserves_128_bit_numeric_endpoints() {
    let root = temp_dir("emit_exe_preserves_128_bit_numeric_endpoints");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

const DECIMAL_MAX: u128 = 340282366920938463463374607431768211455u128;
const HEX_MAX: u128 = 0xffff_ffff_ffff_ffff_ffff_ffff_ffff_ffffu128;
const SIGNED_MIN: i128 = -170141183460469231731687303715884105728i128;
const FLOAT_TO_U128: u128 = 170141183460469231731687303715884105728.0f64 as u128;

pub fun main(init: process::Init) process::ExitCode!() {
    let argc = init.argc();
    let runtime = 340282366920938463463374607431768211455u128;
    if DECIMAL_MAX != u128::MAX or HEX_MAX != u128::MAX or runtime != u128::MAX {
        return process::ExitCode(1)!;
    }
    if runtime - 1u128 != 340282366920938463463374607431768211454u128 {
        return process::ExitCode(2)!;
    }
    if SIGNED_MIN != i128::MIN or -170141183460469231731687303715884105728i128 != i128::MIN {
        return process::ExitCode(3)!;
    }
    if FLOAT_TO_U128 != (1u128 << 127u128) {
        return process::ExitCode(4)!;
    }
    let small = 123456789.0f64 + argc as f64;
    if small as u128 != 123456789u128 + argc as u128 {
        return process::ExitCode(5)!;
    }
    let mixed = 1267650600228230245921633337344.0f64 + argc as f64;
    if mixed as u128 != (1u128 << 100u128) + (3u128 << 48u128) {
        return process::ExitCode(6)!;
    }
    let wide32 = 1267650600228229401496703205376.0f32 + argc as f32;
    if wide32 as u128 != (1u128 << 100u128) {
        return process::ExitCode(7)!;
    }
    let signedSmall = -123456789.0f64 as i128;
    if signedSmall != -123456789i128 {
        return process::ExitCode(8)!;
    }
    let signedWide = -170141183460469231731687303715884105728.0f64 as i128;
    if signedWide != i128::MIN {
        return process::ExitCode(9)!;
    }
    let signed32 = -12345.0f32 as i128;
    if signed32 != -12345i128 {
        return process::ExitCode(10)!;
    }
    let unsignedTieEven64 = ((1u128 << 53u128) + 1u128) as f64;
    if unsignedTieEven64 != 9007199254740992.0f64 {
        return process::ExitCode(11)!;
    }
    let unsignedRoundUp64 = ((1u128 << 53u128) + 3u128) as f64;
    if unsignedRoundUp64 != 9007199254740996.0f64 {
        return process::ExitCode(12)!;
    }
    let unsignedTieEven32 = ((1u128 << 24u128) + 1u128) as f32;
    if unsignedTieEven32 != 16777216.0f32 {
        return process::ExitCode(13)!;
    }
    let unsignedRoundUp32 = ((1u128 << 24u128) + 3u128) as f32;
    if unsignedRoundUp32 != 16777220.0f32 {
        return process::ExitCode(14)!;
    }
    let signedMin64 = i128::MIN as f64;
    if signedMin64 != -170141183460469231731687303715884105728.0f64 {
        return process::ExitCode(15)!;
    }
    let zeroUnsigned = 0u128 as f64;
    let zeroSigned = 0i128 as f32;
    if zeroUnsigned != 0.0f64 or zeroSigned != 0.0f32 {
        return process::ExitCode(16)!;
    }
    let maxUnsigned64 = u128::MAX as f64;
    if maxUnsigned64 != 340282366920938463463374607431768211455.0f64 {
        return process::ExitCode(17)!;
    }
    let signedRound32 = -16777219i128 as f32;
    if signedRound32 != -16777220.0f32 {
        return process::ExitCode(18)!;
    }
    !()
}
"#,
    )
    .expect("write 128-bit literal endpoint source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit 128-bit numeric endpoint executable");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = Command::new(&exe).status_timeout("run 128-bit numeric endpoint executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_simd_bitmask_matches_lane_bits() {
    let root = temp_dir("emit_exe_simd_bitmask_matches_lane_bits");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;

    let values: u8x16 = std::builtin::insert(std::builtin::insert(std::builtin::insert(std::builtin::splat[u8x16](0u8), 1usize, 7u8), 4usize, 7u8), 15usize, 7u8);
    let mask = std::builtin::bitmask(values == std::builtin::splat[u8x16](7u8));
    if mask != 0x8012usize {
        return process::ExitCode(1)!;
    }

    let other = std::builtin::bitmask(values == std::builtin::splat[u8x16](0u8));
    if other != 0x7fedusize {
        return process::ExitCode(2)!;
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
fn emit_exe_const_vector_union_preserves_storage_across_runtime() {
    let root = temp_dir("emit_exe_const_vector_union_preserves_storage_across_runtime");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

union VectorSlot {
    vector: u16x2,
    bytes: [u8; 4],
}

union MaskSlot {
    mask: boolx4,
    raw: u8,
}

const fun makeVector() u16x2 {
    std::builtin::insert(std::builtin::splat[u16x2](4386), 1, 13124)
}

const fun makeSlot(vector: u16x2) VectorSlot {
    VectorSlot { vector }
}

const fun readBytes(slot: VectorSlot) [u8; 4] {
    slot.bytes
}

const fun decode(bytes: [u8; 4]) u16x2 {
    let slot = VectorSlot { bytes: bytes };
    slot.vector
}

const compileVector: u16x2 = makeVector();
const compileSlot: VectorSlot = makeSlot(compileVector);
const compileBytes: [u8; 4] = readBytes(compileSlot);
const decoded: u16x2 = decode(compileBytes);
const compileMask: boolx4 = std::builtin::insert(
    std::builtin::splat[boolx4](false),
    1,
    true,
);
const compileMaskSlot: MaskSlot = MaskSlot { mask: compileMask };
const compileMaskRaw: u8 = compileMaskSlot.raw;
const compileMaskBits: usize = std::builtin::bitmask(compileMask);

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let runtimeBytes = readBytes(makeSlot(makeVector()));
    let materializedBytes = readBytes(compileSlot);
    let runtimeMaskSlot = MaskSlot { mask: compileMask };
    if compileBytes[0] != 34
        or compileBytes[1] != 17
        or compileBytes[2] != 68
        or compileBytes[3] != 51
        or runtimeBytes[2] != compileBytes[2]
        or materializedBytes[3] != compileBytes[3]
        or std::builtin::extract(decoded, 1) != 13124
        or compileMaskRaw != 2
        or compileMaskBits != 2
        or runtimeMaskSlot.raw != compileMaskRaw {
        return process::ExitCode(1)!;
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
        .output_timeout_for_build("run nia emit --exe const vector union");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted const vector union executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_simd_lane_out_of_range_traps() {
    let root = temp_dir("emit_exe_simd_lane_out_of_range_traps");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

fun lane(index: usize) u8 {
    std::builtin::extract(std::builtin::splat[u8x4](7), index)
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    _ = lane(4);
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
        .output_timeout_for_build("run nia emit --exe SIMD bounds trap");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted SIMD bounds trap executable");
    assert!(!status.success(), "out-of-range SIMD lane access must trap");
}

#[test]
fn emit_exe_bit_intrinsics_are_zero_defined() {
    let root = temp_dir("emit_exe_bit_intrinsics_are_zero_defined");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;

    let bits = std::builtin::size[usize]() * 8usize;
    if std::builtin::ctz[usize](0usize) != bits {
        return process::ExitCode(1)!;
    }
    if std::builtin::clz[usize](0usize) != bits {
        return process::ExitCode(2)!;
    }
    if std::builtin::ctz[usize](0x8010usize) != 4usize {
        return process::ExitCode(3)!;
    }
    if std::builtin::clz[usize](0x8010usize) != bits - 16usize {
        return process::ExitCode(4)!;
    }
    if std::builtin::popcount[usize](0x8010usize) != 2usize {
        return process::ExitCode(5)!;
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
fn emit_exe_char_checks_unicode_scalar_values() {
    let root = temp_dir("emit_exe_char_checks_unicode_scalar_values");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;
using std::unicode;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;

    let ascii = match unicode::fromScalarValue(65) {
        ?ch => {
            ch
        },
        null => {
            return process::ExitCode(1)!;
        },
    };
    if ascii.codepoint() != 65 {
        return process::ExitCode(2)!;
    }

    let direct = match std::builtin::charFromU32(66) {
        ?ch => {
            ch
        },
        null => {
            return process::ExitCode(10)!;
        },
    };
    if direct.codepoint() != 66 {
        return process::ExitCode(11)!;
    }

    let maxScalar: u32 = 0x10ffff;
    let surrogate: u32 = 0xd800;
    if not unicode::isValidScalarValue(maxScalar) or unicode::isValidScalarValue(surrogate) {
        return process::ExitCode(23)!;
    }

    let max = match unicode::fromScalarValue(0x10ffff) {
        ?ch => {
            ch
        },
        null => {
            return process::ExitCode(3)!;
        },
    };
    if max.codepoint() != 0x10ffff {
        return process::ExitCode(4)!;
    }

    match unicode::fromScalarValue(0xd800) {
        ?ch => {
            _ = ch;
            return process::ExitCode(5)!;
        },
        null => {},
    }
    match unicode::fromScalarValue(0x110000) {
        ?ch => {
            _ = ch;
            return process::ExitCode(6)!;
        },
        null => {},
    }

    let euroBytes: [u8; 3] = [0xe2, 0x82, 0xac];
    let euro = match unicode::decodeUtf8First(&euroBytes) {
        !decoded => {
            decoded
        },
        error! => {
            _ = error;
            return process::ExitCode(7)!;
        },
    };
    if euro.byteLen() != 3 or euro.scalar().codepoint() != 0x20ac {
        return process::ExitCode(8)!;
    }

    let overlong: [u8; 2] = [0xc0, 0x80];
    match unicode::decodeUtf8First(&overlong) {
        !decoded => { _ = decoded; return process::ExitCode(9)!; },
        error! => if error != unicode::Utf8DecodeError::Overlong {
            return process::ExitCode(12)!;
        },
    }

    match unicode::decodeUtf8First(&overlong[0..0]) {
        !decoded => { _ = decoded; return process::ExitCode(13)!; },
        error! => if error != unicode::Utf8DecodeError::Empty {
            return process::ExitCode(14)!;
        },
    }

    let truncated: [u8; 2] = [0xe2, 0x82];
    match unicode::decodeUtf8First(&truncated) {
        !decoded => { _ = decoded; return process::ExitCode(15)!; },
        error! => if error != unicode::Utf8DecodeError::Truncated {
            return process::ExitCode(16)!;
        },
    }

    let invalidLeading: [u8; 1] = [0x80];
    match unicode::decodeUtf8First(&invalidLeading) {
        !decoded => { _ = decoded; return process::ExitCode(17)!; },
        error! => if error != unicode::Utf8DecodeError::InvalidLeadingByte {
            return process::ExitCode(18)!;
        },
    }

    let invalidContinuation: [u8; 3] = [0xe2, 0x28, 0xa1];
    match unicode::decodeUtf8First(&invalidContinuation) {
        !decoded => { _ = decoded; return process::ExitCode(19)!; },
        error! => if error != unicode::Utf8DecodeError::InvalidContinuation {
            return process::ExitCode(20)!;
        },
    }

    let invalidScalar: [u8; 3] = [0xed, 0xa0, 0x80];
    match unicode::decodeUtf8First(&invalidScalar) {
        !decoded => { _ = decoded; return process::ExitCode(21)!; },
        error! => if error != unicode::Utf8DecodeError::InvalidScalar {
            return process::ExitCode(22)!;
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

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_unaligned_vector_load_reads_lanes() {
    let root = temp_dir("emit_exe_unaligned_vector_load_reads_lanes");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;

    let bytes: [u8; 10] = [99u8, 1u8, 2u8, 3u8, 4u8, 5u8, 6u8, 7u8, 8u8, 100u8];
    let vec = std::builtin::loadUnaligned[u8x8](&bytes[1]);
    if std::builtin::extract(vec, 0usize) != 1u8 {
        return process::ExitCode(1)!;
    }
    if std::builtin::extract(vec, 7usize) != 8u8 {
        return process::ExitCode(2)!;
    }
    let mask = std::builtin::bitmask(vec == std::builtin::splat[u8x8](4u8));
    if mask != 0x08usize {
        return process::ExitCode(3)!;
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
fn emit_exe_const_functions_run_at_comptime_and_runtime() {
    let root = temp_dir("emit_exe_const_functions_run_at_comptime_and_runtime");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
module pairs;
using root::pairs;
using std::process;

struct Width {
    value: usize,
}

struct Counter {
    current: usize,
    end: usize,
}

union Bits {
    integer: u32,
    float: f32,
}

union ByteBits {
    bytes: [u8; 4],
    float: f32,
}

extend Counter : Iterator {
    type Item = usize;

    const fun next(&mut self) ?usize {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

const fun double(value: usize) usize {
    value * 2
}

const fun compileOnlyWidth() usize {
    3
}

extend Width {
    const fun fromValue(value: usize) Width {
        Self { value }
    }

    const fun doubled(self) usize {
        self.value * 2
    }

    const fun increment(&mut self) usize {
        self.value += 1;
        self.value
    }
}

const fun incrementedWidth() usize {
    let mut width = Width::fromValue(3);
    width.increment()
}

const fun iterationTotal(end: usize) usize {
    let mut total = 0usize;
    let mut iter = Counter { current: 0, end: end };
    for value in iter {
        total += value;
    }
    total
}

const fun oneBits() u32 {
    let bits = Bits { float: 1.0 };
    bits.integer
}

const fun oneFromBits() f32 {
    let bits = Bits { integer: 1065353216 };
    bits.float
}

const fun readBits(bits: Bits) u32 {
    bits.integer
}

const fun oneBytes() [u8; 4] {
    let bits = ByteBits { float: 1.0 };
    bits.bytes
}

const fun floatFromBytes(bytes: [u8; 4]) f32 {
    let bits = ByteBits { bytes: bytes };
    bits.float
}

const fun readByteFloat(bits: ByteBits) f32 {
    bits.float
}

const fun pairTotal(values: pairs::Pair[usize]) usize {
    let mut total: usize = 0;
    for value in values {
        total += value;
    }
    total
}

const arrayLen: usize = double(2)
    + Width::fromValue(3).doubled()
    + compileOnlyWidth()
    + incrementedWidth()
    + iterationTotal(3)
    + pairTotal(pairs::pair(1, 2));
const compileOneBits: u32 = readBits(Bits { float: 1.0 });
const compileUnion: Bits = Bits { float: 1.0 };
const compileOneBytes: [u8; 4] = oneBytes();
const compileOneFromBytes: f32 = floatFromBytes(compileOneBytes);
const compileByteUnion: ByteBits = ByteBits { bytes: compileOneBytes };
const compileImportedSlot: pairs::ScalarSlot[f32] = pairs::scalarSlot[f32](1.0);
const compileImportedBits: u32 = pairs::readSlotBits[f32](compileImportedSlot);
const compilePairBytes: [u8; 8] = pairs::encodePair[u32]([7, 8]);
const compilePairValues: [u32; 2] = pairs::decodePair[u32](compilePairBytes);
const compileStructBytes: [u8; 5] = pairs::encodeStruct[u32](
    pairs::Padded[u32] { marker: 170, value: 287454020 },
);
const compileStruct: pairs::Padded[u32] = pairs::decodeStruct[u32](compileStructBytes);
const packetWidth: usize = 2;
const compilePacketValue: pairs::Packet[u8, packetWidth, u16] = pairs::Packet[u8, packetWidth, u16] {
    marker: 170,
    values: [4386, 13124],
};
const compilePacketBytes: [u8; 5] = pairs::encodePacket(compilePacketValue);
const compilePacket: pairs::Packet[u8, 2, u16] = pairs::decodePacket(compilePacketBytes);
const compileNestedSlot: pairs::OuterSlot[u32] = pairs::layeredSlot[u32](1144201745, 26197);
const compileNestedBytes: [u8; 4] = pairs::readLayeredBytes[u32](compileNestedSlot);

fun materializedNestedSlot() pairs::OuterSlot[u32] {
    compileNestedSlot
}

fun runtimeChecks(value: usize) bool {
    let values: [u8; arrayLen] = [0; arrayLen];
    let mut width = Width::fromValue(value);
    let runtimeImportedSlot: pairs::ScalarSlot[f32] = pairs::scalarSlot[f32](1.0);
    let runtimePairValues: [u32; 2] = pairs::decodePair[u32](
        pairs::encodePair[u32]([value as u32, value as u32 + 1])
    );
    let runtimeStructBytes = pairs::encodeStruct[u32](
        pairs::Padded[u32] { marker: 170, value: 287454020 },
    );
    let runtimeStruct = pairs::decodeStruct[u32](runtimeStructBytes);
    let runtimePacketBytes = pairs::encodePacket(pairs::Packet[u8, 2, u16] {
        marker: 170,
        values: [4386, 13124],
    });
    let runtimePacket = pairs::decodePacket(runtimePacketBytes);
    let runtimeNestedSlot = pairs::layeredSlot[u32](1144201745, 26197);
    let runtimeNestedBytes = pairs::readLayeredBytes[u32](runtimeNestedSlot);
    let copiedNestedSlot: pairs::OuterSlot[u32] = compileNestedSlot;
    let copiedNestedBytes = pairs::readLayeredBytes[u32](copiedNestedSlot);
    let materializedNestedBytes = pairs::readLayeredBytes[u32](compileNestedSlot);
    let returnedNestedBytes = pairs::readLayeredBytes[u32](materializedNestedSlot());
    double(value) == 14
        and width.doubled() == 14
        and width.increment() == 8
        and width.value == 8
        and iterationTotal(value) == 21
        and pairTotal(pairs::pair(value, value + 1)) == 15
        and compileOneBits == 1065353216
        and compileUnion.integer == compileOneBits
        and oneBits() == compileOneBits
        and oneFromBits() == 1.0
        and readBits(compileUnion) == compileOneBits
        and floatFromBytes(oneBytes()) == compileOneFromBytes
        and floatFromBytes(compileOneBytes) == 1.0
        and readByteFloat(compileByteUnion) == 1.0
        and compileImportedBits == compileOneBits
        and pairs::readSlotBits[f32](compileImportedSlot) == compileImportedBits
        and pairs::readSlotBits[f32](runtimeImportedSlot) == compileImportedBits
        and compilePairValues[0] == 7
        and compilePairValues[1] == 8
        and pairs::decodePair[u32](compilePairBytes)[1] == compilePairValues[1]
        and runtimePairValues[0] == 7
        and runtimePairValues[1] == 8
        and compileStructBytes[0] == 68
        and compileStructBytes[4] == 170
        and compileStruct.value == 287454020
        and runtimeStructBytes[4] == compileStructBytes[4]
        and runtimeStruct.value == compileStruct.value
        and compilePacketBytes[0] == 34
        and compilePacketBytes[4] == 170
        and compilePacket.values[1] == 13124
        and runtimePacketBytes[4] == compilePacketBytes[4]
        and runtimePacket.values[0] == compilePacket.values[0]
        and compileNestedBytes[0] == 85
        and compileNestedBytes[1] == 102
        and compileNestedBytes[2] == 51
        and compileNestedBytes[3] == 68
        and runtimeNestedBytes[3] == compileNestedBytes[3]
        and copiedNestedBytes[0] == compileNestedBytes[0]
        and materializedNestedBytes[2] == compileNestedBytes[2]
        and returnedNestedBytes[1] == compileNestedBytes[1]
        and values.len() == 23
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    if not runtimeChecks(7) {
        return process::ExitCode(1)!;
    }
    !()
}
"#,
    )
    .expect("write test source");
    std::fs::write(
        root.join("pairs.nia"),
        r#"
pub struct Pair[T] {
    first: T,
    second: T,
}

pub struct PairIter[T] {
    first: T,
    second: T,
    index: usize,
}

pub struct Padded[T] {
    marker: u8,
    value: T,
}

pub struct Packet[T, N: usize, U] {
    marker: T,
    values: [U; N],
}

pub union ScalarSlot[T] {
    value: T,
    bits: u32,
}

pub union PairBytes[T] {
    values: [T; 2],
    bytes: [u8; 8],
}

pub union PaddedBytes[T] {
    value: Padded[T],
    prefix: [u8; 5],
}

pub union PacketBytes[T, N: usize, U] {
    value: Packet[T, N, U],
    prefix: [u8; 5],
}

pub union InnerSlot[T] {
    wide: T,
    narrow: u16,
}

pub union OuterSlot[T] {
    inner: InnerSlot[T],
    bytes: [u8; 4],
}

extend[T] PairIter[T] : Iterator {
    type Item = T;

    pub const fun next(&mut self) ?T {
        match self.index {
            0usize => {
                self.index += 1;
                ?self.first
            },
            1usize => {
                self.index += 1;
                ?self.second
            },
            _ => null,
        }
    }
}

extend[T] Pair[T] : Iterable {
    type Item = T;
    type Iter = PairIter[T];

    pub const fun iter(&self) PairIter[T] {
        PairIter[T] { first: self.first, second: self.second, index: 0 }
    }
}

pub const fun pair[T](first: T, second: T) Pair[T] {
    Pair[T] { first: first, second: second }
}

pub const fun scalarSlot[T](value: T) ScalarSlot[T] {
    ScalarSlot[T] { value }
}

pub const fun readSlotBits[T](slot: ScalarSlot[T]) u32 {
    slot.bits
}

pub const fun encodePair[T](values: [T; 2]) [u8; 8] {
    let slot = PairBytes[T] { values: values };
    slot.bytes
}

pub const fun decodePair[T](bytes: [u8; 8]) [T; 2] {
    let slot = PairBytes[T] { bytes: bytes };
    slot.values
}

pub const fun encodeStruct[T](value: Padded[T]) [u8; 5] {
    let slot = PaddedBytes[T] { value: value };
    slot.prefix
}

pub const fun decodeStruct[T](bytes: [u8; 5]) Padded[T] {
    let slot = PaddedBytes[T] { prefix: bytes };
    slot.value
}

pub const fun encodePacket(value: Packet[u8, 2, u16]) [u8; 5] {
    let slot = PacketBytes[u8, 2, u16] { value: value };
    slot.prefix
}

pub const fun decodePacket(bytes: [u8; 5]) Packet[u8, 2, u16] {
    let slot = PacketBytes[u8, 2, u16] { prefix: bytes };
    slot.value
}

pub const fun layeredSlot[T](wide: T, narrow: u16) OuterSlot[T] {
    let mut inner = InnerSlot[T] { wide: wide };
    inner.narrow = narrow;
    OuterSlot[T] { inner }
}

pub const fun readLayeredBytes[T](slot: OuterSlot[T]) [u8; 4] {
    slot.bytes
}
"#,
    )
    .expect("write imported generic iterator source");

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
