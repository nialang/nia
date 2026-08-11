// SPDX-License-Identifier: GPL-3.0-or-later

use std::{env, fs, path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    let Some(command) = args.next() else {
        return usage();
    };
    let Some(path) = args.next().map(PathBuf::from) else {
        return usage();
    };
    if args.next().is_some() {
        return usage();
    }

    let expected = nia_compat::toolchain_manifest();
    match command.to_str() {
        Some("check") => match fs::read_to_string(&path) {
            Ok(actual) if actual == expected => ExitCode::SUCCESS,
            Ok(_) => {
                eprintln!(
                    "{} is stale; run `cargo run -p nia-compat -- write {}`",
                    path.display(),
                    path.display()
                );
                ExitCode::FAILURE
            }
            Err(error) => {
                eprintln!("failed to read {}: {error}", path.display());
                ExitCode::FAILURE
            }
        },
        Some("write") => match fs::write(&path, expected) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("failed to write {}: {error}", path.display());
                ExitCode::FAILURE
            }
        },
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: nia-compat <check|write> <toolchain.meta>");
    ExitCode::FAILURE
}
