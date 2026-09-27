use std::env;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

const REAL_LLVM_CONFIG: &str = env!("NIA_LLVM_CONFIG_REAL");

fn main() {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let output = Command::new(REAL_LLVM_CONFIG)
        .args(&args)
        .output()
        .unwrap_or_else(|error| {
            eprintln!("failed to run real llvm-config: {error}");
            process::exit(1);
        });

    if !output.stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if args.iter().any(|arg| arg == "--system-libs") {
        // Some official Windows archives contain absolute paths from the LLVM
        // build machine. llvm-sys treats those as files and fails if they are
        // not present on the current machine. A few archives also list
        // optional .lib files that were not included in the package.
        let filtered = stdout
            .split_whitespace()
            .filter_map(normalize_library)
            .collect::<Vec<_>>()
            .join(" ");
        println!("{filtered}");
    } else {
        print!("{stdout}");
    }

    process::exit(output.status.code().unwrap_or(1));
}

fn normalize_library(token: &str) -> Option<String> {
    let bytes = token.as_bytes();
    let has_drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\');
    if has_drive {
        if Path::new(token).exists() {
            return Path::new(token)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned);
        }
        return find_library_variant(Path::new(token).file_name()?.to_str()?);
    }

    if !token.ends_with(".lib") || token.contains(['/', '\\']) {
        return Some(token.to_owned());
    }

    find_library_variant(token)
}

fn find_library_variant(name: &str) -> Option<String> {
    let mut names = vec![name.to_owned()];
    if let Some(stem) = name.strip_suffix("_static.lib") {
        names.push(format!("{stem}.lib"));
    }

    let llvm_lib = Path::new(REAL_LLVM_CONFIG)
        .parent()
        .and_then(Path::parent)
        .map(|prefix| prefix.join("lib"));
    let directories: Vec<PathBuf> = llvm_lib
        .into_iter()
        .chain(
            env::var_os("LIB")
                .into_iter()
                .flat_map(|value| {
                    value
                        .to_string_lossy()
                        .split(';')
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .map(|directory| PathBuf::from(directory)),
        )
        .collect();
    for candidate in names {
        if directories
            .iter()
            .any(|directory| directory.join(&candidate).exists())
        {
            return Some(candidate);
        }
    }
    None
}
