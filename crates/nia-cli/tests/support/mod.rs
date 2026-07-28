// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

pub(crate) use nia_test_support::{CommandExt, CommandStatusExt};

static TEMP_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn temp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let id = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.push(format!(
        "nia_cli_test_{name}_{}_{:?}_{id}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}
