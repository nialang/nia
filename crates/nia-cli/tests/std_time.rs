// SPDX-License-Identifier: GPL-3.0-or-later
//! `std::time` reads a monotonic clock and measures spans with it.

use std::process::Command;

mod support;

use nia_test_support::{CommandExt, test_dir as temp_dir};

#[test]
fn instants_measure_elapsed_time_on_the_monotonic_clock() {
    let root = temp_dir("instants_measure_elapsed_time_on_the_monotonic_clock");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;
using std::time;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    // The clock advances while the program waits on it.
    let start = time::Instant::now();
    while start.elapsed().millis() < 20 {}
    let later = time::Instant::now();
    let span = later.durationSince(start);
    if span.millis() < 20 or span.seconds() > 60 {
        return process::ExitCode(1)!;
    }
    // A span measured backwards is empty.
    if start.durationSince(later).nanos() != 0 {
        return process::ExitCode(2)!;
    }
    let mut previous = later;
    for _ in 0..1000 {
        let next = time::Instant::now();
        if previous.durationSince(next).nanos() != 0 {
            return process::ExitCode(3)!;
        }
        previous = next;
    }
    // Units convert exactly and round down.
    let spans = time::Duration::fromMillis(1500);
    if spans.secondsF64() != 1.5 or spans.seconds() != 1 or spans.micros() != 1500000 {
        return process::ExitCode(4)!;
    }
    if time::Duration::fromSeconds(2).nanos() != 2000000000 or time::Duration::fromMicros(7).nanos() != 7000 {
        return process::ExitCode(5)!;
    }
    if time::Duration::fromNanos(999).micros() != 0 or time::Duration::zero.nanos() != 0 {
        return process::ExitCode(6)!;
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
        .output_timeout_for_build("compile std time test");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let run = Command::new(&exe).output_timeout_for_runtime("run std time test");
    assert_eq!(
        run.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}
