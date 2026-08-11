use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use nia_maintain::audit::{compatibility, std_build_host};
use nia_maintain::baseline::{build, compare, compiler};
use nia_maintain::report::crate_boundaries;
use nia_maintain::{MaintainResult, parse_usize, repository_root};

const USAGE: &str = "\
usage: nia-maintain <command> [options]

commands:
  audit compatibility      check compatibility identities
  audit std-build-host     check the std build-host closure
  report crate-boundaries  report workspace crate evidence
  baseline compiler        collect compiler performance samples
  baseline compare         compare compiler performance samples
  baseline build           collect the representative build baseline
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

fn parse_f64(value: &str, option: &str) -> MaintainResult<f64> {
    value
        .parse()
        .map_err(|_| format!("{option} requires a number, found {value:?}"))
}

fn compiler_baseline_command(arguments: &[String]) -> MaintainResult<()> {
    let root = repository_root();
    let mut options = compiler::Options::for_repository(&root);
    let mut index = 0;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        match option {
            "--compiler" => {
                options.compiler = PathBuf::from(take_value(arguments, &mut index, option)?)
            }
            "--resource-root" => {
                options.resource_root = PathBuf::from(take_value(arguments, &mut index, option)?)
            }
            "--output" => {
                options.output = PathBuf::from(take_value(arguments, &mut index, option)?)
            }
            "--repeat" => {
                let value = take_value(arguments, &mut index, option)?;
                options.repeat = parse_usize(&value, option)?;
            }
            "--no-build" => options.build_compiler = false,
            "--runner-class" => {
                options.runner_class = Some(take_value(arguments, &mut index, option)?)
            }
            "--workload" => options
                .workloads
                .push(take_value(arguments, &mut index, option)?),
            _ => return Err(format!("unknown compiler baseline option: {option}")),
        }
        index += 1;
    }
    compiler::run(&root, &options)
}

fn compare_baseline_command(arguments: &[String]) -> MaintainResult<bool> {
    let mut positional = Vec::new();
    let mut max_wall_regression = 50.0;
    let mut max_rss_regression = 30.0;
    let mut max_query_regression = 5.0;
    let mut max_allocation_regression = 20.0;
    let mut allow_machine_mismatch = false;
    let mut index = 0;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        match option {
            "--max-wall-regression" => {
                let value = take_value(arguments, &mut index, option)?;
                max_wall_regression = parse_f64(&value, option)?;
            }
            "--max-rss-regression" => {
                let value = take_value(arguments, &mut index, option)?;
                max_rss_regression = parse_f64(&value, option)?;
            }
            "--max-query-regression" => {
                let value = take_value(arguments, &mut index, option)?;
                max_query_regression = parse_f64(&value, option)?;
            }
            "--max-allocation-regression" => {
                let value = take_value(arguments, &mut index, option)?;
                max_allocation_regression = parse_f64(&value, option)?;
            }
            "--allow-machine-mismatch" => allow_machine_mismatch = true,
            value if value.starts_with('-') => {
                return Err(format!("unknown baseline comparison option: {value}"));
            }
            value => positional.push(PathBuf::from(value)),
        }
        index += 1;
    }
    let [baseline, candidate] = positional.as_slice() else {
        return Err(
            "usage: nia-maintain baseline compare <baseline.json> <candidate.json> [options]"
                .to_owned(),
        );
    };
    compare::run(&compare::Options {
        baseline: baseline.clone(),
        candidate: candidate.clone(),
        max_wall_regression,
        max_rss_regression,
        max_query_regression,
        max_allocation_regression,
        allow_machine_mismatch,
    })
}

fn build_baseline_command(arguments: &[String]) -> MaintainResult<()> {
    let root = repository_root();
    let mut options = build::Options::for_repository(&root);
    let mut index = 0;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        match option {
            "--nia" => options.nia = PathBuf::from(take_value(arguments, &mut index, option)?),
            "--resource-root" => {
                options.resource_root = PathBuf::from(take_value(arguments, &mut index, option)?)
            }
            "--fixture" => {
                options.fixture = PathBuf::from(take_value(arguments, &mut index, option)?)
            }
            "--output" => {
                options.output = PathBuf::from(take_value(arguments, &mut index, option)?)
            }
            "--timeout-seconds" => {
                let value = take_value(arguments, &mut index, option)?;
                options.timeout_seconds = value.parse().map_err(|_| {
                    format!("{option} requires a non-negative integer, found {value:?}")
                })?;
            }
            "--repetitions" => {
                let value = take_value(arguments, &mut index, option)?;
                options.repetitions = parse_usize(&value, option)?;
            }
            "--keep-workspace" => options.keep_workspace = true,
            _ => return Err(format!("unknown build baseline option: {option}")),
        }
        index += 1;
    }
    build::run(&options)
}

fn check(arguments: &[String]) -> MaintainResult<()> {
    if !arguments.is_empty() {
        return Err("usage: nia-maintain check".to_owned());
    }
    let root = repository_root();
    compatibility::run(&root)?;
    std_build_host::run(&root, &std_build_host::Options::for_repository(&root))
}

fn dispatch(arguments: &[String]) -> MaintainResult<bool> {
    match arguments {
        [] => {
            println!("{USAGE}");
            Ok(true)
        }
        [help] if help == "--help" || help == "-h" => {
            println!("{USAGE}");
            Ok(true)
        }
        [first, second, rest @ ..] if first == "audit" && second == "compatibility" => {
            compatibility_command(rest).map(|()| true)
        }
        [first, second, rest @ ..] if first == "audit" && second == "std-build-host" => {
            std_build_host_command(rest).map(|()| true)
        }
        [first, second, rest @ ..] if first == "report" && second == "crate-boundaries" => {
            crate_boundaries_command(rest).map(|()| true)
        }
        [first, second, rest @ ..] if first == "baseline" && second == "compiler" => {
            compiler_baseline_command(rest).map(|()| true)
        }
        [first, second, rest @ ..] if first == "baseline" && second == "compare" => {
            compare_baseline_command(rest)
        }
        [first, second, rest @ ..] if first == "baseline" && second == "build" => {
            build_baseline_command(rest).map(|()| true)
        }
        [command, rest @ ..] if command == "check" => check(rest).map(|()| true),
        _ => Err(format!(
            "unknown maintenance command: {}\n{USAGE}",
            arguments.join(" ")
        )),
    }
}

fn main() -> ExitCode {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    match dispatch(&arguments) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
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
