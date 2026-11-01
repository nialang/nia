// SPDX-License-Identifier: GPL-3.0-or-later
//! Every example builds, runs, and prints the output its README promises. On
//! Linux x86_64 the same programs also run on every Linux target: i686 natively
//! and aarch64 under user-mode QEMU.

use std::path::{Path, PathBuf};
use std::process::Command;
mod support;
use nia_test_support::{CommandExt, test_dir as temp_dir};

/// The examples in reading order, each with its expected output. Printed
/// pointers are written as `<address>`, since they differ between runs and
/// between 32- and 64-bit targets.
const EXAMPLES: [(&str, &str); 10] = [
    ("hello.nia", "hello from nia\n"),
    (
        "basics.nia",
        "distance=41 total=211 evens=5 level=1 high=1\ndefer order=3 2 1\n",
    ),
    (
        "data.nia",
        "window=2 3 filled=7 name=5 bytes=3 sum=312\nterminated=4 copied=3 missing=-1\n",
    ),
    (
        "errors.nia",
        "good=8080 empty=1 bad=2 large=3\ndigit=7 none=0\n",
    ),
    (
        "traits.nia",
        "first=2 ints=6 cents=449 large=2 largest=120\n",
    ),
    (
        "closures.nia",
        "a=2 b=21 c=5 calls=2 d=42 answer=42\nsquares=30 firstLarge=4\n",
    ),
    (
        "collections.nia",
        "list=3 total=60 answer=430 keys=102030 text=4\n",
    ),
    (
        "io.nia",
        "name=___nia answer=+0042 hex=0xab ptr=<address>\n",
    ),
    ("process.nia", "child wrote 18 bytes and exited cleanly\n"),
    ("modules/main.nia", "sum=5 len2=25 color=3 diff=5\n"),
];

fn source(example: &str) -> PathBuf {
    nia_test_support::workspace_root()
        .join("examples")
        .join(example)
}

fn executable_name(example: &str) -> String {
    example.trim_end_matches(".nia").replace('/', "-")
}

// The process example runs `/bin/sh`, which only POSIX hosts provide.
fn runs_on_host(example: &str) -> bool {
    !(cfg!(windows) && example == "process.nia")
}

fn emit(source: &Path, target: Option<&str>, output: &Path) {
    let mut command = support::nia_command();
    if let Some(target) = target {
        command.args(["--target", target]);
    }
    let emitted = command
        .args(["emit", "--exe"])
        .arg(source)
        .arg("-o")
        .arg(output)
        .output_timeout_for_build("emit example");
    assert!(
        emitted.status.success(),
        "{} {target:?}: {}",
        source.display(),
        String::from_utf8_lossy(&emitted.stderr)
    );
}

fn mask_addresses(text: &str) -> String {
    let mut words = text
        .split(|ch: char| ch.is_whitespace() || ch == '=')
        .collect::<Vec<_>>();
    for index in 1..words.len() {
        if words[index - 1] == "ptr" {
            words[index] = "<address>";
        }
    }
    words.join(" ")
}

fn run(mut command: Command, case: &str) -> String {
    let output = command.output_timeout_for_runtime("run example");
    assert_eq!(output.status.code(), Some(0), "{case}");
    let text = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    mask_addresses(&text)
}

#[test]
fn examples_print_their_documented_output_on_the_host() {
    let root = temp_dir("examples_print_their_documented_output_on_the_host");
    for (example, expected) in EXAMPLES {
        let source = source(example);
        if !runs_on_host(example) {
            let checked = support::nia_command()
                .args(["check", "--runtime", "freestanding"])
                .arg(&source)
                .output_timeout_for_compiler("check example");
            assert!(
                checked.status.success(),
                "{example}: {}",
                String::from_utf8_lossy(&checked.stderr)
            );
            continue;
        }
        let exe = root
            .join(executable_name(example))
            .with_extension(std::env::consts::EXE_EXTENSION);
        emit(&source, None, &exe);
        assert_eq!(
            run(Command::new(&exe), example),
            mask_addresses(expected),
            "{example}"
        );
    }
}

#[test]
fn every_example_file_is_listed() {
    let root = nia_test_support::workspace_root().join("examples");
    let mut found = std::fs::read_dir(&root)
        .expect("read examples")
        .map(|entry| entry.expect("example entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "nia"))
        .map(|path| {
            path.file_name()
                .expect("example file name")
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    found.push("modules/main.nia".to_owned());
    found.sort();
    let mut listed = EXAMPLES
        .iter()
        .map(|(example, _)| (*example).to_owned())
        .collect::<Vec<_>>();
    listed.sort();
    assert_eq!(found, listed);
}

#[test]
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn examples_run_identically_on_every_linux_target() {
    let root = temp_dir("examples_run_identically_on_every_linux_target");
    for (example, expected) in EXAMPLES {
        let source = source(example);
        let name = executable_name(example);
        for target in nia_test_support::LINUX_TARGETS {
            let exe = root.join(format!("{name}-{target}"));
            emit(&source, Some(target), &exe);
            let command = nia_test_support::linux_target_command(target, &exe);
            assert_eq!(
                run(command, &format!("{example} {target}")),
                mask_addresses(expected),
                "{example} on {target}"
            );
        }
    }
}
