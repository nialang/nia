// SPDX-License-Identifier: GPL-3.0-or-later
//! The one helper that must be compiled into each test binary: only
//! integration tests of this crate can name the compiler under test. Shared
//! helpers live in `nia-test-support`.
use std::process::Command;

/// The compiler under test, using this checkout's toolchain resources.
pub(crate) fn nia_command() -> Command {
    nia_test_support::nia_command(env!("CARGO_BIN_EXE_nia"))
}
