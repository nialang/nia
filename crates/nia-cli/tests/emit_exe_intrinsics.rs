// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_simd_bitmask_matches_lane_bits() {
    let root = temp_dir("emit_exe_simd_bitmask_matches_lane_bits");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;

    let values: u8x16 = std::builtin::insert(std::builtin::insert(std::builtin::insert(std::builtin::splat[u8x16](0u8), 1usize, 7u8), 4usize, 7u8), 15usize, 7u8);
    let mask = std::builtin::bitmask(values == std::builtin::splat[u8x16](7u8));
    if mask != 0x8012usize {
        return process::exit(1)!;
    }

    let other = std::builtin::bitmask(values == std::builtin::splat[u8x16](0u8));
    if other != 0x7fedusize {
        return process::exit(2)!;
    }

    !{}
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
fn emit_exe_bit_intrinsics_are_zero_defined() {
    let root = temp_dir("emit_exe_bit_intrinsics_are_zero_defined");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;

    if std::builtin::ctz[usize](0usize) != 64usize {
        return process::exit(1)!;
    }
    if std::builtin::clz[usize](0usize) != 64usize {
        return process::exit(2)!;
    }
    if std::builtin::ctz[usize](0x8010usize) != 4usize {
        return process::exit(3)!;
    }
    if std::builtin::clz[usize](0x8010usize) != 48usize {
        return process::exit(4)!;
    }
    if std::builtin::popcount[usize](0x8010usize) != 2usize {
        return process::exit(5)!;
    }

    !{}
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

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;

    let ascii = switch unicode::fromScalarValue(65) {
        ?ch => {
            ch
        },
        null => {
            return process::exit(1)!;
        },
    };
    if ascii.codepoint() != 65 {
        return process::exit(2)!;
    }

    let direct = switch std::builtin::charFromU32(66) {
        ?ch => {
            ch
        },
        null => {
            return process::exit(10)!;
        },
    };
    if direct.codepoint() != 66 {
        return process::exit(11)!;
    }

    let maxScalar: u32 = 0x10ffff;
    let surrogate: u32 = 0xd800;
    if not unicode::isValidScalarValue(maxScalar) or unicode::isValidScalarValue(surrogate) {
        return process::exit(23)!;
    }

    let max = switch unicode::fromScalarValue(0x10ffff) {
        ?ch => {
            ch
        },
        null => {
            return process::exit(3)!;
        },
    };
    if max.codepoint() != 0x10ffff {
        return process::exit(4)!;
    }

    switch unicode::fromScalarValue(0xd800) {
        ?ch => {
            _ = ch;
            return process::exit(5)!;
        },
        null => {},
    }
    switch unicode::fromScalarValue(0x110000) {
        ?ch => {
            _ = ch;
            return process::exit(6)!;
        },
        null => {},
    }

    let euroBytes: [3]u8 = [0xe2, 0x82, 0xac];
    let euro = switch unicode::decodeUtf8First(&euroBytes) {
        !decoded => {
            decoded
        },
        error! => {
            _ = error;
            return process::exit(7)!;
        },
    };
    if euro.byteLen() != 3 or euro.scalar().codepoint() != 0x20ac {
        return process::exit(8)!;
    }

    let overlong: [2]u8 = [0xc0, 0x80];
    switch unicode::decodeUtf8First(&overlong) {
        !decoded => { _ = decoded; return process::exit(9)!; },
        error! => if error != unicode::Utf8DecodeError::Overlong {
            return process::exit(12)!;
        },
    }

    switch unicode::decodeUtf8First(&overlong[0..0]) {
        !decoded => { _ = decoded; return process::exit(13)!; },
        error! => if error != unicode::Utf8DecodeError::Empty {
            return process::exit(14)!;
        },
    }

    let truncated: [2]u8 = [0xe2, 0x82];
    switch unicode::decodeUtf8First(&truncated) {
        !decoded => { _ = decoded; return process::exit(15)!; },
        error! => if error != unicode::Utf8DecodeError::Truncated {
            return process::exit(16)!;
        },
    }

    let invalidLeading: [1]u8 = [0x80];
    switch unicode::decodeUtf8First(&invalidLeading) {
        !decoded => { _ = decoded; return process::exit(17)!; },
        error! => if error != unicode::Utf8DecodeError::InvalidLeadingByte {
            return process::exit(18)!;
        },
    }

    let invalidContinuation: [3]u8 = [0xe2, 0x28, 0xa1];
    switch unicode::decodeUtf8First(&invalidContinuation) {
        !decoded => { _ = decoded; return process::exit(19)!; },
        error! => if error != unicode::Utf8DecodeError::InvalidContinuation {
            return process::exit(20)!;
        },
    }

    let invalidScalar: [3]u8 = [0xed, 0xa0, 0x80];
    switch unicode::decodeUtf8First(&invalidScalar) {
        !decoded => { _ = decoded; return process::exit(21)!; },
        error! => if error != unicode::Utf8DecodeError::InvalidScalar {
            return process::exit(22)!;
        },
    }

    !{}
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

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;

    let bytes: [10]u8 = [99u8, 1u8, 2u8, 3u8, 4u8, 5u8, 6u8, 7u8, 8u8, 100u8];
    let vec = std::builtin::load_unaligned[u8x8](&bytes[1]);
    if std::builtin::extract(vec, 0usize) != 1u8 {
        return process::exit(1)!;
    }
    if std::builtin::extract(vec, 7usize) != 8u8 {
        return process::exit(2)!;
    }
    let mask = std::builtin::bitmask(vec == std::builtin::splat[u8x8](4u8));
    if mask != 0x08usize {
        return process::exit(3)!;
    }

    !{}
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
using entry::pairs;
using std::process;

struct Width {
    value: usize,
}

struct Counter {
    current: usize,
    end: usize,
}

extend Counter : Iterator {
    type Item = usize;

    const fn next(&mut self) ?usize {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

const fn double(value: usize) usize {
    value * 2
}

const fn compileOnlyWidth() usize {
    3
}

extend Width {
    const fn fromValue(value: usize) Width {
        { value: value }
    }

    const fn doubled(self) usize {
        self.value * 2
    }

    const fn increment(&mut self) usize {
        self.value += 1;
        self.value
    }
}

const fn incrementedWidth() usize {
    let mut width = Width::fromValue(3);
    width.increment()
}

const fn iterationTotal(end: usize) usize {
    let mut total = 0usize;
    let mut iter = Counter { current: 0, end: end };
    for value in iter {
        total += value;
    }
    total
}

const fn pairTotal(values: pairs::Pair[usize]) usize {
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

fn runtimeChecks(value: usize) bool {
    let values: [arrayLen]u8 = [0; arrayLen];
    let mut width = Width::fromValue(value);
    double(value) == 14
        and width.doubled() == 14
        and width.increment() == 8
        and width.value == 8
        and iterationTotal(value) == 21
        and pairTotal(pairs::pair(value, value + 1)) == 15
        and values.len() == 23
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    if not runtimeChecks(7) {
        return process::exit(1)!;
    }
    !{}
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

extend[T] PairIter[T] : Iterator {
    type Item = T;

    pub const fn next(&mut self) ?T {
        switch self.index {
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

    pub const fn iter(&self) PairIter[T] {
        PairIter[T] { first: self.first, second: self.second, index: 0 }
    }
}

pub const fn pair[T](first: T, second: T) Pair[T] {
    Pair[T] { first: first, second: second }
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
