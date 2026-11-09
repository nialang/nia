// SPDX-License-Identifier: GPL-3.0-or-later
//! Values that live in memory (structs, tuples, arrays, optionals, error
//! unions, and enums with payloads) keep value semantics however they are
//! built, copied, passed, and returned, at every optimization level.

use std::process::Command;

mod support;

use nia_test_support::{CommandExt, CommandStatusExt, test_dir as temp_dir};

fn run_at_each_level(name: &str, source: &str) {
    let root = temp_dir(name);
    let main = root.join("main.nia");
    std::fs::write(&main, source).expect("write executable source");
    for level in ["-O0", "-O2"] {
        let exe = root.join(format!(
            "main{}{}",
            level.trim_start_matches('-'),
            std::env::consts::EXE_SUFFIX
        ));
        let output = support::nia_command()
            .arg(level)
            .args(["emit", "--exe"])
            .arg(&main)
            .arg("-o")
            .arg(&exe)
            .output_timeout_for_build("emit aggregate executable");
        assert!(
            output.status.success(),
            "{level} stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let status = Command::new(&exe).status_timeout("run aggregate executable");
        assert_eq!(status.code(), Some(0), "{level}");
    }
}

#[test]
fn emit_exe_assignments_read_their_source_before_writing() {
    run_at_each_level(
        "emit_exe_assignments_read_their_source_before_writing",
        r#"
using std::process;

struct Pair {
    a: u64,
    b: u64,
    pad: [u64; 4],
}

fun swapped(p: &Pair) Pair {
    Pair { a: p.b, b: p.a, pad: p.pad }
}

fun bumped(p: &mut Pair) usize {
    p.a += 10;
    1
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    // The callee reads `s` while building its result.
    let mut s = Pair { a: 1, b: 2, pad: [0; 4] };
    s = swapped(&s);
    if s.a != 2 or s.b != 1 {
        return process::ExitCode(1)!;
    }
    // The literal reads `t.a` after it would have written it.
    let mut t = Pair { a: 1, b: 2, pad: [0; 4] };
    t = Pair { a: t.b, b: t.a, pad: t.pad };
    if t.a != 2 or t.b != 1 {
        return process::ExitCode(2)!;
    }
    // The right-hand side is read before the index changes it.
    let mut source = Pair { a: 5, b: 6, pad: [0; 4] };
    let mut slots = [Pair { a: 0, b: 0, pad: [0; 4] }; 2];
    slots[bumped(&mut source)] = source;
    if slots[1].a != 5 or source.a != 15 {
        return process::ExitCode(3)!;
    }
    // A copy is independent of its source.
    let mut copy = slots[1];
    copy.b = 60;
    if slots[1].b != 6 or copy.b != 60 {
        return process::ExitCode(4)!;
    }
    !()
}
"#,
    );
}

#[test]
fn emit_exe_arguments_and_results_are_copies() {
    run_at_each_level(
        "emit_exe_arguments_and_results_are_copies",
        r#"
using std::process;

struct Big {
    words: [u64; 1000],
    tag: u64,
}

fun first_word(big: Big) u64 {
    big.words[0] + big.tag
}

fun both(before: Big, after: u64) u64 {
    before.tag * 100 + after
}

fun retag(big: &mut Big, tag: u64) u64 {
    big.tag = tag;
    tag
}

fun made(seed: u64) Big {
    Big { words: [seed; 1000], tag: seed + 1 }
}

fun maybe(present: bool) ?Big {
    if present { ?made(3) } else { null }
}

fun relayed(present: bool) ?Big {
    let big = maybe(present).?;
    ?big
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    // An 8 KB argument copied on every iteration must not grow the stack.
    let big = made(1);
    let mut total: u64 = 0;
    let mut round: u64 = 0;
    while round < 100000 {
        total += first_word(big);
        round += 1;
    }
    if total != 300000 {
        return process::ExitCode(1)!;
    }
    // The first argument is copied before the second changes its source.
    let mut changing = made(4);
    if both(changing, retag(&mut changing, 9)) != 509 or changing.tag != 9 {
        return process::ExitCode(2)!;
    }
    // Optional results propagate through `.?` with their payload intact.
    if relayed(true) is ?found {
        if found.words[999] != 3 or found.tag != 4 {
            return process::ExitCode(3)!;
        }
    } else {
        return process::ExitCode(4)!;
    }
    if relayed(false) is ?_ {
        return process::ExitCode(5)!;
    }
    !()
}
"#,
    );
}

#[test]
fn emit_exe_deferred_code_runs_after_the_result_is_built() {
    run_at_each_level(
        "emit_exe_deferred_code_runs_after_the_result_is_built",
        r#"
using std::process;

struct Report {
    count: u64,
    marks: [u8; 32],
}

fun counted() Report {
    let mut count: u64 = 1;
    defer count = 100;
    Report { count: count, marks: [7; 32] }
}

fun overridden() Report {
    defer {
        return Report { count: 2, marks: [0; 32] };
    };
    Report { count: 1, marks: [1; 32] }
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let report = counted();
    if report.count != 1 or report.marks[31] != 7 {
        return process::ExitCode(1)!;
    }
    let replaced = overridden();
    if replaced.count != 2 or replaced.marks[0] != 0 {
        return process::ExitCode(2)!;
    }
    !()
}
"#,
    );
}

#[test]
fn emit_exe_literal_arrays_are_fresh_copies() {
    run_at_each_level(
        "emit_exe_literal_arrays_are_fresh_copies",
        r#"
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut word = "abc";
    let other = "abc";
    word[0] = 'x';
    if word[0] != 'x' or other[0] != 'a' or word[2] != 'c' {
        return process::ExitCode(1)!;
    }
    let mut bytes = b"hi";
    bytes[1] = b'o';
    if bytes[1] != b'o' {
        return process::ExitCode(2)!;
    }
    let zeros: [u32; 64] = [0; 64];
    let sevens: [u8; 9] = [7; 9];
    let mut seed: u16 = 300;
    let repeated: [u16; 5] = [seed; 5];
    seed = 1;
    if zeros[63] != 0 or sevens[8] != 7 or repeated[4] != 300 or seed != 1 {
        return process::ExitCode(3)!;
    }
    !()
}
"#,
    );
}
