// SPDX-License-Identifier: GPL-3.0-or-later
//! Every example builds and runs on the host, and on Linux x86_64 the same
//! programs run identically on every Linux target: i686 natively and aarch64
//! under user-mode QEMU.

use std::path::{Path, PathBuf};
use std::process::Command;
mod support;
use nia_test_support::{CommandExt, test_dir as temp_dir};

fn examples() -> Vec<PathBuf> {
    let root = nia_test_support::workspace_root().join("examples");
    let mut paths = std::fs::read_dir(&root)
        .expect("read examples")
        .map(|entry| entry.expect("example entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "nia"))
        .collect::<Vec<_>>();
    paths.push(root.join("modules/main.nia"));
    paths.sort();
    paths
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

// Output with printed pointers masked, since addresses differ between runs
// and between 32- and 64-bit targets.
fn run(mut command: Command, case: &str) -> String {
    let output = command.output_timeout_for_runtime("run example");
    assert_eq!(output.status.code(), Some(0), "{case}");
    let text = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
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

#[test]
fn examples_run_on_the_host() {
    let root = temp_dir("examples_run_on_the_host");
    for source in examples() {
        // The process example runs `/bin/sh`, which only POSIX hosts provide.
        if cfg!(windows) && source.ends_with("process.nia") {
            continue;
        }
        let exe = root
            .join(source.file_stem().expect("example name"))
            .with_extension(std::env::consts::EXE_EXTENSION);
        emit(&source, None, &exe);
        run(Command::new(&exe), &source.display().to_string());
    }
}

#[test]
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn examples_run_identically_on_every_linux_target() {
    let qemu = ["qemu-aarch64-static", "qemu-aarch64"]
        .into_iter()
        .find(|program| Command::new(program).arg("--version").output().is_ok())
        .expect("aarch64 Linux executables run under user-mode QEMU; install qemu-user-static");
    let root = temp_dir("examples_run_identically_on_every_linux_target");
    for source in examples() {
        let name = source
            .file_stem()
            .expect("example name")
            .to_string_lossy()
            .into_owned();
        let mut outputs = Vec::new();
        for target in [
            "x86_64-unknown-linux",
            "x86-unknown-linux",
            "aarch64-unknown-linux",
        ] {
            let exe = root.join(format!("{name}-{target}"));
            emit(&source, Some(target), &exe);
            let command = if target.starts_with("aarch64") {
                let mut command = Command::new(qemu);
                command.arg(&exe);
                command
            } else {
                Command::new(&exe)
            };
            outputs.push((target, run(command, &format!("{name} {target}"))));
        }
        for (target, output) in &outputs[1..] {
            assert_eq!(
                output, &outputs[0].1,
                "{name}: {target} differs from x86_64"
            );
        }
    }
}
