use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use nia_maintain::audit::{compatibility, std_build_host};
use nia_maintain::report::crate_boundaries;
use nia_maintain::{MaintainResult, parse_usize, repository_root};

const USAGE: &str = "\
usage: nia-maintain <command> [options]

commands:
  audit compatibility      check compatibility identities
  audit std-build-host     check the std build-host closure
  report crate-boundaries  report workspace crate evidence
  check                    run every fast repository audit";

fn take_value(arguments: &[String], index: &mut usize, option: &str) -> MaintainResult<String> {
    *index += 1;
    arguments
        .get(*index)
        .cloned()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn compatibility_command(arguments: &[String]) -> MaintainResult<()> {
    let mut root = repository_root();
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--root" => root = PathBuf::from(take_value(arguments, &mut index, "--root")?),
            option => return Err(format!("unknown compatibility audit option: {option}")),
        }
        index += 1;
    }
    compatibility::run(&root)
}

fn std_build_host_command(arguments: &[String]) -> MaintainResult<()> {
    let root = repository_root();
    let mut options = std_build_host::Options::for_repository(&root);
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--print" => options.print = true,
            "--snapshot" => {
                options.snapshot = PathBuf::from(take_value(arguments, &mut index, "--snapshot")?)
            }
            option => return Err(format!("unknown std build-host audit option: {option}")),
        }
        index += 1;
    }
    std_build_host::run(&root, &options)
}

fn crate_boundaries_command(arguments: &[String]) -> MaintainResult<()> {
    let mut options = crate_boundaries::Options::default();
    let mut index = 0;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        match option {
            "--max-rust-loc" => {
                let value = take_value(arguments, &mut index, option)?;
                options.max_rust_loc = Some(parse_usize(&value, option)?);
            }
            "--max-production-dependents" => {
                let value = take_value(arguments, &mut index, option)?;
                options.max_production_dependents = Some(parse_usize(&value, option)?);
            }
            _ => return Err(format!("unknown crate-boundaries option: {option}")),
        }
        index += 1;
    }
    crate_boundaries::run(&repository_root(), &options)
}

fn check(arguments: &[String]) -> MaintainResult<()> {
    if !arguments.is_empty() {
        return Err("usage: nia-maintain check".to_owned());
    }
    let root = repository_root();
    compatibility::run(&root)?;
    std_build_host::run(&root, &std_build_host::Options::for_repository(&root))
}

fn dispatch(arguments: &[String]) -> MaintainResult<()> {
    match arguments {
        [] => {
            println!("{USAGE}");
            Ok(())
        }
        [help] if help == "--help" || help == "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        [first, second, rest @ ..] if first == "audit" && second == "compatibility" => {
            compatibility_command(rest)
        }
        [first, second, rest @ ..] if first == "audit" && second == "std-build-host" => {
            std_build_host_command(rest)
        }
        [first, second, rest @ ..] if first == "report" && second == "crate-boundaries" => {
            crate_boundaries_command(rest)
        }
        [command, rest @ ..] if command == "check" => check(rest),
        _ => Err(format!(
            "unknown maintenance command: {}\n{USAGE}",
            arguments.join(" ")
        )),
    }
}

fn main() -> ExitCode {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    match dispatch(&arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_lists_owned_command_groups() {
        assert!(USAGE.contains("audit compatibility"));
        assert!(USAGE.contains("report crate-boundaries"));
        assert!(USAGE.contains("check"));
    }

    #[test]
    fn numeric_options_reject_negative_values() {
        let error = crate_boundaries_command(&["--max-rust-loc".to_owned(), "-1".to_owned()])
            .expect_err("negative source size should fail");
        assert!(error.contains("non-negative integer"));
    }
}
