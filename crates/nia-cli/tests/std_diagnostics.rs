// SPDX-License-Identifier: GPL-3.0-or-later
//! The standard library checks without errors or warnings on every operating
//! system it supports, so a user never sees diagnostics from std's own code.

mod support;

use nia_test_support::CommandExt;

#[test]
fn std_checks_cleanly_for_every_operating_system() {
    let std_root = nia_test_support::workspace_root().join("lib/std");
    for target in [
        "x86_64-unknown-linux",
        "x86_64-pc-windows-msvc",
        "aarch64-apple-macos",
    ] {
        let output = support::nia_command()
            .args(["--target", target, "check"])
            .arg(&std_root)
            .output_timeout_for_compiler(&format!("check std for {target}"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{target}:\n{stderr}");
        assert!(!stderr.contains("warning["), "{target}:\n{stderr}");
    }
}
