// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub(crate) fn nia_command() -> Command {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("nia-cli lives under crates/");
    nia_command_with_resource_root(workspace_root.join("lib"))
}

pub(crate) fn nia_command_with_resource_root(resource_root: impl Into<PathBuf>) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nia"));
    command.arg("--resource-root").arg(resource_root.into());
    command
}

pub(crate) use nia_test_support::{CommandExt, CommandStatusExt};

pub(crate) fn temp_dir(name: &str) -> nia_test_support::TestDir {
    nia_test_support::test_dir(name)
}
