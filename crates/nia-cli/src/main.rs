// SPDX-License-Identifier: GPL-3.0-or-later
//! Command-line ownership for compiler inspection, checking, emission, and builds.
//!
//! Global target, module-map, optimization, timing, and toolchain options are
//! parsed once and then translated into typed `nia-driver` or `nia-build`
//! requests. Native outputs remain file-only, inspection output uses stdout,
//! and diagnostics/timings use stderr. Panics cross one installed ICE boundary
//! and become a failed process exit rather than unwinding through the CLI.
#[cfg(feature = "perf-alloc")]
use std::alloc::System;
use std::{env, fs, num::NonZeroUsize, path::PathBuf, process::ExitCode, sync::Arc};

use nia_driver::{ModuleMap, NiaOptimizationLevel, Runtime, SourcePath};
use nia_timing::{TimingFormat, TimingOptions, TimingTrace};

mod help;

use help::{HelpStyle, help_text};

#[cfg(feature = "perf-alloc")]
#[global_allocator]
static GLOBAL_ALLOCATOR: nia_timing::CountingAllocator<System> =
    nia_timing::CountingAllocator::new(System);

fn main() -> ExitCode {
    #[cfg(feature = "perf-alloc")]
    nia_timing::register_allocation_instrumentation();
    nia_ice::install_panic_hook();
    match parse_cli(env::args().skip(1).collect()) {
        Ok(CliAction::Help(topic)) => {
            print!("{}", help_text(topic, HelpStyle::for_stdout()));
            ExitCode::SUCCESS
        }
        Ok(CliAction::Version) => {
            println!("nia {}", nia_compat::COMPILER_VERSION);
            ExitCode::SUCCESS
        }
        Ok(CliAction::Run(cli)) => {
            let timing_options = cli.timing_options();
            run_with_ice_boundary(
                timing_options,
                || run_cli(cli),
                |ice| eprintln!("{}", ice.render_message()),
            )
        }
        Err(error) => {
            if error.is_help {
                print!("{}", help_text(error.help, HelpStyle::for_stdout()));
                return ExitCode::SUCCESS;
            }
            eprintln!("error: {}", error.message);
            eprintln!();
            eprint!("{}", help_text(error.help, HelpStyle::for_stderr()));
            ExitCode::FAILURE
        }
    }
}

fn run_with_ice_boundary(
    timing_options: TimingOptions,
    f: impl FnOnce() -> ExitCode,
    report: impl FnOnce(&nia_ice::Ice),
) -> ExitCode {
    match nia_ice::catch_ice(|| nia_timing::collect_to_stderr(timing_options, f)) {
        Ok(code) => code,
        Err(ice) => {
            report(&ice);
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug)]
struct Cli {
    resource_root: Option<PathBuf>,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    timings: nia_driver::TimingMode,
    timing_trace: TimingTrace,
    timing_format: TimingFormat,
    command: CliCommand,
}

impl Cli {
    fn timing_options(&self) -> TimingOptions {
        TimingOptions::new(self.timings)
            .with_trace(self.timing_trace)
            .with_format(self.timing_format)
    }
}

#[derive(Debug)]
enum CliCommand {
    Build {
        root: Option<PathBuf>,
        step: Option<String>,
        jobs: Option<NonZeroUsize>,
    },
    Check {
        path: String,
        opt_report: bool,
        runtime: Runtime,
        cache_dir: Option<PathBuf>,
    },
    Emit {
        path: String,
        target: EmitTarget,
        opt_report: bool,
    },
}

#[derive(Debug)]
enum EmitTarget {
    Tokens,
    Ast,
    Checked { runtime: Runtime },
    Backend { runtime: Runtime },
    Llvm { runtime: Runtime },
    Obj { args: Vec<String> },
    Exe { args: Vec<String> },
}

enum CliAction {
    Help(HelpTopic),
    Version,
    Run(Cli),
}

#[derive(Clone, Copy)]
enum HelpTopic {
    Main,
    Build,
    Check,
    Emit,
}

struct CliError {
    message: String,
    help: HelpTopic,
    is_help: bool,
}

impl CliError {
    fn new(message: impl Into<String>, help: HelpTopic) -> Self {
        Self {
            message: message.into(),
            help,
            is_help: false,
        }
    }

    fn help(help: HelpTopic) -> Self {
        Self {
            message: String::new(),
            help,
            is_help: true,
        }
    }
}

fn run_cli(cli: Cli) -> ExitCode {
    let toolchain = match resolve_toolchain_layout(cli.resource_root) {
        Ok(toolchain) => toolchain,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };
    let timing_format = cli.timing_format;
    match cli.command {
        CliCommand::Build { root, step, jobs } => run_build(
            root,
            step,
            jobs,
            cli.optimization,
            cli.timings,
            timing_format,
            toolchain,
        ),
        CliCommand::Check {
            path,
            opt_report,
            runtime,
            cache_dir,
        } => {
            let source = match read_source(&path) {
                Ok(source) => source,
                Err(message) => {
                    eprintln!("error: {message}");
                    return ExitCode::FAILURE;
                }
            };
            run_check(
                &path,
                &source,
                cli.module_map,
                CheckRunOptions {
                    optimization: cli.optimization,
                    timings: cli.timings,
                    opt_report,
                    runtime,
                    cache_dir,
                },
                toolchain,
            )
        }
        CliCommand::Emit {
            path,
            target,
            opt_report,
        } => {
            let source = match read_source(&path) {
                Ok(source) => source,
                Err(message) => {
                    eprintln!("error: {message}");
                    return ExitCode::FAILURE;
                }
            };
            run_emit(
                &path,
                &source,
                target,
                EmitContext {
                    module_map: cli.module_map,
                    optimization: cli.optimization,
                    timings: cli.timings,
                    opt_report,
                    toolchain,
                },
            )
        }
    }
}

fn resolve_toolchain_layout(
    resource_root: Option<PathBuf>,
) -> Result<Arc<nia_toolchain::ToolchainLayout>, String> {
    let executable = env::current_exe()
        .map_err(|error| format!("failed to resolve compiler executable: {error}"))?;
    let request = match resource_root {
        Some(resource_root) => {
            nia_toolchain::ToolchainLayoutRequest::explicit(executable, resource_root)
        }
        None => nia_toolchain::ToolchainLayoutRequest::installed(executable),
    };
    nia_toolchain::ToolchainLayout::resolve(request)
        .map(Arc::new)
        .map_err(|error| format!("invalid toolchain layout: {error}"))
}

fn read_source(path: &str) -> Result<String, String> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(err) => {
            return Err(format!("failed to read `{path}`: {err}"));
        }
    };
    Ok(source)
}

fn parse_cli(args: Vec<String>) -> Result<CliAction, CliError> {
    if args.is_empty() {
        return Ok(CliAction::Help(HelpTopic::Main));
    }

    let (remaining, global_options) = extract_global_options(args, HelpTopic::Main)?;
    if remaining.is_empty() {
        return Ok(CliAction::Help(HelpTopic::Main));
    }
    if remaining.len() == 1 {
        match remaining[0].as_str() {
            "-h" | "--help" => return Ok(CliAction::Help(HelpTopic::Main)),
            "-V" | "--version" => return Ok(CliAction::Version),
            _ => {}
        }
    }
    match parse_command(remaining)? {
        ParsedCommand::Help(topic) => Ok(CliAction::Help(topic)),
        ParsedCommand::Run(command) => Ok(CliAction::Run(Cli {
            resource_root: global_options.resource_root,
            module_map: global_options.module_map,
            optimization: global_options.optimization,
            timings: global_options.timings,
            timing_trace: global_options.timing_trace,
            timing_format: global_options.timing_format,
            command,
        })),
    }
}

struct GlobalOptions {
    resource_root: Option<PathBuf>,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    timings: nia_driver::TimingMode,
    timing_trace: TimingTrace,
    timing_format: TimingFormat,
}

fn extract_global_options(
    args: Vec<String>,
    help: HelpTopic,
) -> Result<(Vec<String>, GlobalOptions), CliError> {
    let mut map = ModuleMap::new();
    let mut resource_root = None;
    let mut optimization = NiaOptimizationLevel::default();
    let mut timings = nia_driver::TimingMode::Off;
    let mut timing_trace = TimingTrace::Off;
    let mut timing_format = TimingFormat::Text;
    let mut remaining = Vec::new();
    let mut iter = args.into_iter();
    let mut preserve_next = false;
    while let Some(arg) = iter.next() {
        if preserve_next {
            preserve_next = false;
            remaining.push(arg);
            continue;
        }
        if let Some(payload) = module_payload_from_short(&arg) {
            let payload = match payload {
                Some(payload) => payload,
                None => iter
                    .next()
                    .ok_or_else(|| CliError::new("missing argument after `-M`", help))?,
            };
            insert_module_map_entry(&mut map, &payload)
                .map_err(|message| CliError::new(message, help))?;
            continue;
        }
        if arg == "--module" {
            let payload = iter
                .next()
                .ok_or_else(|| CliError::new("missing argument after `--module`", help))?;
            insert_module_map_entry(&mut map, &payload)
                .map_err(|message| CliError::new(message, help))?;
            continue;
        }
        if let Some(payload) = arg.strip_prefix("--module=") {
            insert_module_map_entry(&mut map, payload)
                .map_err(|message| CliError::new(message, help))?;
            continue;
        }
        if arg == "--resource-root" {
            let path = iter
                .next()
                .ok_or_else(|| CliError::new("missing path after `--resource-root`", help))?;
            set_resource_root(&mut resource_root, path, help)?;
            continue;
        }
        if let Some(path) = arg.strip_prefix("--resource-root=") {
            set_resource_root(&mut resource_root, path.to_string(), help)?;
            continue;
        }
        if emit_target_option_takes_value(&arg) {
            preserve_next = true;
            remaining.push(arg);
            continue;
        }
        if let Some(level) = parse_optimization_flag(&arg) {
            optimization = level.map_err(|message| CliError::new(message, help))?;
            continue;
        }
        if let Some(mode) = parse_timings_flag(&arg) {
            timings = mode.map_err(|message| CliError::new(message, help))?;
            continue;
        }
        if let Some(trace) = parse_timing_trace_flag(&arg, &mut iter) {
            timing_trace = trace.map_err(|message| CliError::new(message, help))?;
            continue;
        }
        if let Some(format) = parse_timing_format_flag(&arg) {
            timing_format = format.map_err(|message| CliError::new(message, help))?;
            continue;
        }
        remaining.push(arg);
    }
    Ok((
        remaining,
        GlobalOptions {
            resource_root,
            module_map: map,
            optimization,
            timings,
            timing_trace,
            timing_format,
        },
    ))
}

fn set_resource_root(
    slot: &mut Option<PathBuf>,
    path: String,
    help: HelpTopic,
) -> Result<(), CliError> {
    if path.is_empty() {
        return Err(CliError::new("`--resource-root` cannot be empty", help));
    }
    if slot.replace(PathBuf::from(path)).is_some() {
        return Err(CliError::new(
            "`--resource-root` may be specified only once",
            help,
        ));
    }
    Ok(())
}

fn emit_target_option_takes_value(arg: &str) -> bool {
    matches!(
        arg,
        "-o" | "--out-dir"
            | "--runtime"
            | "--link-arg"
            | "--dynamic-linker"
            | "--library-path"
            | "-L"
            | "--library"
            | "-l"
            | "--rpath"
            | "--linker"
            | "--linker-flavor"
    )
}

fn module_payload_from_short(arg: &str) -> Option<Option<String>> {
    if arg == "-M" {
        return Some(None);
    }
    let payload = arg.strip_prefix("-M")?;
    if payload.is_empty() {
        return Some(None);
    }
    if let Some(rest) = payload.strip_prefix('=') {
        return Some(Some(rest.to_string()));
    }
    Some(Some(payload.to_string()))
}

fn parse_optimization_flag(arg: &str) -> Option<Result<NiaOptimizationLevel, String>> {
    let level = match arg {
        "-O" => NiaOptimizationLevel::O2,
        "-O0" => NiaOptimizationLevel::O0,
        "-O1" => NiaOptimizationLevel::O1,
        "-O2" => NiaOptimizationLevel::O2,
        "-O3" => NiaOptimizationLevel::O3,
        "-Os" => NiaOptimizationLevel::Os,
        "-Oz" => NiaOptimizationLevel::Oz,
        _ if arg.starts_with("-O") => {
            return Some(Err(format!(
                "unknown optimization level `{arg}`; expected -O0, -O1, -O2, -O3, -Os, or -Oz"
            )));
        }
        _ => return None,
    };
    Some(Ok(level))
}

fn parse_timings_flag(arg: &str) -> Option<Result<nia_driver::TimingMode, String>> {
    match arg {
        "--timings" | "--timings=summary" => Some(Ok(nia_driver::TimingMode::Summary)),
        "--timings=detail" => Some(Ok(nia_driver::TimingMode::Detail)),
        _ if arg.starts_with("--timings=") => Some(Err(format!(
            "unknown timings mode `{arg}`; expected --timings, --timings=summary, or --timings=detail"
        ))),
        _ => None,
    }
}

fn parse_timing_trace_flag(
    arg: &str,
    iter: &mut impl Iterator<Item = String>,
) -> Option<Result<TimingTrace, String>> {
    if let Some(value) = arg.strip_prefix("--timing-trace=") {
        return Some(parse_timing_trace_value(value));
    }
    if arg == "--timing-trace" {
        let Some(value) = iter.next() else {
            return Some(Err("missing mode after `--timing-trace`".to_string()));
        };
        return Some(parse_timing_trace_value(&value));
    }
    None
}

fn parse_timing_trace_value(value: &str) -> Result<TimingTrace, String> {
    match value {
        "off" => Ok(TimingTrace::Off),
        "events" => Ok(TimingTrace::Events),
        _ => Err(format!(
            "unknown timing trace mode `--timing-trace={value}`; expected --timing-trace=off or --timing-trace=events"
        )),
    }
}

fn parse_timing_format_flag(arg: &str) -> Option<Result<TimingFormat, String>> {
    let value = arg.strip_prefix("--timings-format=")?;
    Some(match value {
        "text" => Ok(TimingFormat::Text),
        "json" => Ok(TimingFormat::Json),
        _ => Err(format!(
            "unknown timings format `{value}`; expected text or json"
        )),
    })
}

enum ParsedCommand {
    Help(HelpTopic),
    Run(CliCommand),
}

fn parse_command(args: Vec<String>) -> Result<ParsedCommand, CliError> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Err(CliError::new("missing command", HelpTopic::Main));
    };
    let rest = args.collect::<Vec<_>>();
    match command.as_str() {
        "help" => Ok(ParsedCommand::Help(help_topic_from_args(&rest))),
        "build" => parse_build_command(rest).map(ParsedCommand::Run),
        "check" => parse_check_command(rest).map(ParsedCommand::Run),
        "emit" => parse_emit_command(rest).map(ParsedCommand::Run),
        _ => Err(CliError::new(
            format!("unknown command `{command}`"),
            HelpTopic::Main,
        )),
    }
}

fn help_topic_from_args(args: &[String]) -> HelpTopic {
    match args {
        [] => HelpTopic::Main,
        [command] if command == "build" => HelpTopic::Build,
        [command] if command == "check" => HelpTopic::Check,
        [command] if command == "emit" => HelpTopic::Emit,
        [command, target] if command == "emit" && emit_target_flag(target).is_some() => {
            HelpTopic::Emit
        }
        _ => HelpTopic::Main,
    }
}

fn parse_build_command(args: Vec<String>) -> Result<CliCommand, CliError> {
    if has_help_flag(&args) {
        return Err(CliError::help(HelpTopic::Build));
    }
    let mut root = None::<PathBuf>;
    let mut step = None::<String>;
    let mut jobs = None::<NonZeroUsize>;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--root=") {
            if value.is_empty() {
                return Err(CliError::new("`--root` cannot be empty", HelpTopic::Build));
            }
            root = Some(PathBuf::from(value));
            continue;
        }
        if let Some(value) = arg.strip_prefix("--jobs=") {
            jobs = Some(parse_build_jobs(value)?);
            continue;
        }
        if let Some(value) = arg.strip_prefix("-j").filter(|value| !value.is_empty()) {
            jobs = Some(parse_build_jobs(value)?);
            continue;
        }
        match arg.as_str() {
            "--root" => {
                let Some(value) = iter.next() else {
                    return Err(CliError::new(
                        "missing path after `--root`",
                        HelpTopic::Build,
                    ));
                };
                root = Some(PathBuf::from(value));
            }
            "--jobs" | "-j" => {
                let Some(value) = iter.next() else {
                    return Err(CliError::new(
                        format!("missing count after `{arg}`"),
                        HelpTopic::Build,
                    ));
                };
                jobs = Some(parse_build_jobs(&value)?);
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::new(
                    format!("unknown `nia build` option `{arg}`"),
                    HelpTopic::Build,
                ));
            }
            _ if step.is_none() => step = Some(arg),
            _ => {
                return Err(CliError::new(
                    format!("unexpected argument `{arg}` for `nia build`"),
                    HelpTopic::Build,
                ));
            }
        }
    }
    Ok(CliCommand::Build { root, step, jobs })
}

fn parse_build_jobs(value: &str) -> Result<NonZeroUsize, CliError> {
    value
        .parse::<usize>()
        .ok()
        .and_then(NonZeroUsize::new)
        .ok_or_else(|| {
            CliError::new(
                format!("invalid build job count `{value}`; expected a positive integer"),
                HelpTopic::Build,
            )
        })
}

fn parse_check_command(args: Vec<String>) -> Result<CliCommand, CliError> {
    if has_help_flag(&args) {
        return Err(CliError::help(HelpTopic::Check));
    }
    let mut path = None;
    let mut opt_report = false;
    let mut runtime = Runtime::Bare;
    let mut cache_dir = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--runtime=") {
            runtime = parse_runtime(value).map_err(|message| {
                CliError::new(format!("{message} for `nia check`"), HelpTopic::Check)
            })?;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--cache-dir=") {
            cache_dir = Some(PathBuf::from(value));
            continue;
        }
        match arg.as_str() {
            "--opt-report" => opt_report = true,
            "--runtime" => {
                let Some(value) = iter.next() else {
                    return Err(CliError::new(
                        "missing runtime after `--runtime`",
                        HelpTopic::Check,
                    ));
                };
                runtime = parse_runtime(&value).map_err(|message| {
                    CliError::new(format!("{message} for `nia check`"), HelpTopic::Check)
                })?;
            }
            "--cache-dir" => {
                let Some(path) = iter.next() else {
                    return Err(CliError::new(
                        "missing path after `--cache-dir`",
                        HelpTopic::Check,
                    ));
                };
                cache_dir = Some(PathBuf::from(path));
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::new(
                    format!("unknown `nia check` option `{arg}`"),
                    HelpTopic::Check,
                ));
            }
            _ if path.is_none() => path = Some(arg),
            _ => {
                return Err(CliError::new(
                    format!("unexpected argument `{arg}` for `nia check`"),
                    HelpTopic::Check,
                ));
            }
        }
    }
    let Some(path) = path else {
        return Err(CliError::new(
            "missing source file for `nia check`",
            HelpTopic::Check,
        ));
    };
    Ok(CliCommand::Check {
        path,
        opt_report,
        runtime,
        cache_dir,
    })
}

fn parse_emit_command(args: Vec<String>) -> Result<CliCommand, CliError> {
    if args.is_empty() {
        return Err(CliError::help(HelpTopic::Emit));
    }
    if has_help_flag(&args) {
        return Err(CliError::help(HelpTopic::Emit));
    }

    let mut target = None;
    let mut opt_report = false;
    let mut path = None;
    let mut target_args = Vec::new();
    let mut preserve_next = false;
    for arg in args {
        if preserve_next {
            preserve_next = false;
            target_args.push(arg);
            continue;
        }
        match emit_target_flag(&arg) {
            Some(flag) => {
                if target.is_some() {
                    return Err(CliError::new(
                        "use exactly one emit target flag",
                        HelpTopic::Emit,
                    ));
                }
                target = Some(flag);
                continue;
            }
            None if looks_like_removed_emit_target(&arg) && path.is_none() => {
                return Err(CliError::new(
                    format!("old `nia emit {arg}` syntax was removed; use `nia emit --{arg}`"),
                    HelpTopic::Emit,
                ));
            }
            None => {}
        }
        match arg.as_str() {
            "--opt-report" => opt_report = true,
            _ if emit_target_option_takes_value(&arg) => {
                preserve_next = true;
                target_args.push(arg);
            }
            _ if arg.starts_with("--runtime=")
                || arg.starts_with("--link-arg=")
                || arg.starts_with("--dynamic-linker=")
                || arg.starts_with("--library-path=")
                || arg.starts_with("-L")
                || arg.starts_with("--library=")
                || arg.starts_with("-l")
                || arg.starts_with("--rpath=")
                || arg.starts_with("--linker=")
                || arg.starts_with("--linker-flavor=") =>
            {
                target_args.push(arg);
            }
            _ if arg.starts_with('-') && path.is_none() => {
                return Err(CliError::new(
                    format!("unknown `nia emit` option `{arg}`"),
                    HelpTopic::Emit,
                ));
            }
            _ if path.is_none() => path = Some(arg),
            _ => {
                target_args.push(arg);
            }
        }
    }
    let Some(target) = target else {
        return Err(CliError::new(
            "missing emit target flag; expected one of --tokens, --ast, --checked, --backend, --llvm, --obj, or --exe",
            HelpTopic::Emit,
        ));
    };
    let (runtime, target_args) = if target.accepts_runtime() {
        parse_emit_runtime_args(target_args)?
    } else {
        (Runtime::Bare, target_args)
    };
    let Some(path) = path else {
        return Err(CliError::new(
            "missing source file for `nia emit`",
            HelpTopic::Emit,
        ));
    };
    if !target.accepts_target_args()
        && let Some(arg) = target_args.first()
    {
        return Err(CliError::new(
            format!(
                "unexpected argument `{arg}` for `nia emit {}`",
                target.flag_name()
            ),
            HelpTopic::Emit,
        ));
    }
    if opt_report && !target.accepts_opt_report() {
        return Err(CliError::new(
            format!(
                "`--opt-report` is not valid for `nia emit {}`",
                target.flag_name()
            ),
            HelpTopic::Emit,
        ));
    }
    let target = match target {
        ParsedEmitTarget::Tokens => EmitTarget::Tokens,
        ParsedEmitTarget::Ast => EmitTarget::Ast,
        ParsedEmitTarget::Checked => EmitTarget::Checked { runtime },
        ParsedEmitTarget::Backend => EmitTarget::Backend { runtime },
        ParsedEmitTarget::Llvm => EmitTarget::Llvm { runtime },
        ParsedEmitTarget::Obj => EmitTarget::Obj { args: target_args },
        ParsedEmitTarget::Exe => EmitTarget::Exe { args: target_args },
    };
    Ok(CliCommand::Emit {
        path,
        target,
        opt_report,
    })
}

#[derive(Clone, Copy)]
enum ParsedEmitTarget {
    Tokens,
    Ast,
    Checked,
    Backend,
    Llvm,
    Obj,
    Exe,
}

impl ParsedEmitTarget {
    fn flag_name(self) -> &'static str {
        match self {
            Self::Tokens => "--tokens",
            Self::Ast => "--ast",
            Self::Checked => "--checked",
            Self::Backend => "--backend",
            Self::Llvm => "--llvm",
            Self::Obj => "--obj",
            Self::Exe => "--exe",
        }
    }

    fn accepts_target_args(self) -> bool {
        matches!(self, Self::Obj | Self::Exe)
    }

    fn accepts_opt_report(self) -> bool {
        matches!(self, Self::Backend | Self::Llvm | Self::Obj | Self::Exe)
    }

    fn accepts_runtime(self) -> bool {
        matches!(self, Self::Checked | Self::Backend | Self::Llvm)
    }
}

fn parse_emit_runtime_args(args: Vec<String>) -> Result<(Runtime, Vec<String>), CliError> {
    let mut runtime = Runtime::Bare;
    let mut remaining = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--runtime=") {
            runtime = parse_runtime(value).map_err(|message| {
                CliError::new(format!("{message} for `nia emit`"), HelpTopic::Emit)
            })?;
            continue;
        }
        match arg.as_str() {
            "--runtime" => {
                let Some(value) = iter.next() else {
                    return Err(CliError::new(
                        "missing runtime after `--runtime`",
                        HelpTopic::Emit,
                    ));
                };
                runtime = parse_runtime(&value).map_err(|message| {
                    CliError::new(format!("{message} for `nia emit`"), HelpTopic::Emit)
                })?;
            }
            _ => remaining.push(arg),
        }
    }
    Ok((runtime, remaining))
}

fn emit_target_flag(arg: &str) -> Option<ParsedEmitTarget> {
    match arg {
        "--tokens" => Some(ParsedEmitTarget::Tokens),
        "--ast" => Some(ParsedEmitTarget::Ast),
        "--checked" => Some(ParsedEmitTarget::Checked),
        "--backend" => Some(ParsedEmitTarget::Backend),
        "--llvm" => Some(ParsedEmitTarget::Llvm),
        "--obj" => Some(ParsedEmitTarget::Obj),
        "--exe" => Some(ParsedEmitTarget::Exe),
        _ => None,
    }
}

fn looks_like_removed_emit_target(arg: &str) -> bool {
    matches!(
        arg,
        "tokens" | "ast" | "checked" | "backend" | "llvm" | "obj" | "exe"
    )
}

fn has_help_flag(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "-h" || arg == "--help")
}

fn insert_module_map_entry(map: &mut ModuleMap, payload: &str) -> Result<(), String> {
    let Some((name, path)) = payload.split_once('=') else {
        return Err(format!(
            "expected module map entry in `name=path` form, got `{payload}`"
        ));
    };
    if name.is_empty() {
        return Err("module map name cannot be empty".to_string());
    }
    if path.is_empty() {
        return Err(format!("module map `{name}` has empty path"));
    }
    map.try_insert(name, SourcePath::new(path))
}

fn run_lex(source: &str) -> ExitCode {
    print!("{}", nia_driver::tokens_inspection(source).text);
    ExitCode::SUCCESS
}

fn run_parse(path: &str, source: &str) -> ExitCode {
    let inspection = nia_driver::ast_inspection(source);
    print!("{}", inspection.text);
    if !inspection.parse_errors.is_empty() {
        eprint!(
            "{}",
            nia_driver::render_parse_errors(path, source, &inspection.parse_errors)
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

struct CheckRunOptions {
    optimization: NiaOptimizationLevel,
    timings: nia_driver::TimingMode,
    opt_report: bool,
    runtime: Runtime,
    cache_dir: Option<PathBuf>,
}

fn run_check(
    path: &str,
    source: &str,
    module_map: ModuleMap,
    options: CheckRunOptions,
    toolchain: Arc<nia_toolchain::ToolchainLayout>,
) -> ExitCode {
    let driver = nia_driver::Driver::with_config(nia_driver::DriverConfig {
        artifact_cache_dir: options.cache_dir,
        ..nia_driver::DriverConfig::new(toolchain)
    });
    let output = time_summary_stage(options.timings, "check", || {
        driver.check_entry(
            nia_driver::CheckRequest::new(path)
                .with_module_map(module_map.clone())
                .with_optimization(options.optimization)
                .with_timings(options.timings)
                .with_runtime(options.runtime),
        )
    });
    match checked_program_from_output(output, path, source) {
        Ok(_) => {}
        Err(code) => return code,
    }
    if options.opt_report {
        let output = time_summary_stage(options.timings, "codegen", || {
            codegen_with_driver(
                &driver,
                path,
                module_map,
                options.optimization,
                options.timings,
                options.runtime,
            )
        });
        let codegen = match codegen_program_from_output(output, path, source) {
            Ok(program) => program,
            Err(code) => return code,
        };
        print_optimization_report(&codegen);
    }
    ExitCode::SUCCESS
}

fn check_with_driver(
    path: &str,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    timings: nia_driver::TimingMode,
    runtime: Runtime,
    toolchain: Arc<nia_toolchain::ToolchainLayout>,
) -> nia_driver::DriverOutput<nia_driver::CheckedProgram> {
    nia_driver::Driver::new(toolchain).check_entry(
        nia_driver::CheckRequest::new(path)
            .with_module_map(module_map)
            .with_optimization(optimization)
            .with_timings(timings)
            .with_runtime(runtime),
    )
}

fn checked_program_from_output(
    output: nia_driver::DriverOutput<nia_driver::CheckedProgram>,
    path: &str,
    source: &str,
) -> Result<nia_driver::CheckedProgram, ExitCode> {
    match output.result {
        Ok(program) => {
            print_check_warnings(&program, path, source);
            Ok(program)
        }
        Err(error) => {
            eprint!(
                "{}",
                nia_driver::render_driver_error(&error, Some(path), Some(source))
            );
            Err(ExitCode::FAILURE)
        }
    }
}

fn codegen_with_driver(
    driver: &nia_driver::Driver,
    path: &str,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    timings: nia_driver::TimingMode,
    runtime: Runtime,
) -> nia_driver::DriverOutput<nia_driver::CodegenProgram> {
    driver.codegen(
        nia_driver::CheckRequest::new(path)
            .with_module_map(module_map)
            .with_optimization(optimization)
            .with_timings(timings)
            .with_runtime(runtime),
    )
}

fn codegen_program_from_output(
    output: nia_driver::DriverOutput<nia_driver::CodegenProgram>,
    path: &str,
    source: &str,
) -> Result<nia_driver::CodegenProgram, ExitCode> {
    match output.result {
        Ok(program) => {
            print_codegen_warnings(&program, path, source);
            Ok(program)
        }
        Err(error) => {
            eprint!(
                "{}",
                nia_driver::render_driver_error(&error, Some(path), Some(source))
            );
            Err(ExitCode::FAILURE)
        }
    }
}

fn print_check_warnings(program: &nia_driver::CheckedProgram, path: &str, source: &str) {
    if program
        .diagnostics
        .iter()
        .any(nia_driver::ProgramDiagnostic::is_warning)
    {
        eprint!(
            "{}",
            nia_driver::render_program_warnings(program, Some(path), Some(source))
        );
    }
}

fn print_codegen_warnings(program: &nia_driver::CodegenProgram, path: &str, source: &str) {
    if program
        .diagnostics
        .iter()
        .any(nia_driver::ProgramDiagnostic::is_warning)
    {
        eprint!(
            "{}",
            nia_driver::render_codegen_program_warnings(program, Some(path), Some(source))
        );
    }
}

struct EmitContext {
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    timings: nia_driver::TimingMode,
    opt_report: bool,
    toolchain: Arc<nia_toolchain::ToolchainLayout>,
}

fn run_emit(path: &str, source: &str, target: EmitTarget, context: EmitContext) -> ExitCode {
    match target {
        EmitTarget::Tokens => time_summary_stage(context.timings, "lex", || run_lex(source)),
        EmitTarget::Ast => time_summary_stage(context.timings, "parse", || run_parse(path, source)),
        EmitTarget::Checked { runtime } => run_emit_checked(path, source, runtime, context),
        EmitTarget::Backend { runtime } => run_emit_backend(path, source, runtime, context),
        EmitTarget::Llvm { runtime } => run_emit_llvm(path, source, runtime, context),
        EmitTarget::Obj { args } => run_emit_obj(path, source, args, context),
        EmitTarget::Exe { args } => run_emit_exe(path, source, args, context),
    }
}

fn run_build(
    root: Option<PathBuf>,
    step: Option<String>,
    jobs: Option<NonZeroUsize>,
    optimization: NiaOptimizationLevel,
    timings: nia_driver::TimingMode,
    timing_format: TimingFormat,
    toolchain: Arc<nia_toolchain::ToolchainLayout>,
) -> ExitCode {
    let mut request = nia_build::BuildRequest::new(toolchain);
    if let Some(root) = root {
        request = request.with_root(root);
    }
    if let Some(step) = step {
        request = request.with_step(step);
    }
    if let Some(jobs) = jobs {
        request = request.with_max_parallel_actions(jobs);
    }
    request = request
        .with_optimization(build_optimization(optimization))
        .with_timings(timings)
        .with_timing_format(timing_format);
    match nia_build::run_build(request) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn build_optimization(optimization: NiaOptimizationLevel) -> nia_build::OptimizationMode {
    match optimization {
        NiaOptimizationLevel::O0 => nia_build::OptimizationMode::O0,
        NiaOptimizationLevel::O1 => nia_build::OptimizationMode::O1,
        NiaOptimizationLevel::O2 => nia_build::OptimizationMode::O2,
        NiaOptimizationLevel::O3 => nia_build::OptimizationMode::O3,
        NiaOptimizationLevel::Os => nia_build::OptimizationMode::Os,
        NiaOptimizationLevel::Oz => nia_build::OptimizationMode::Oz,
    }
}

fn time_summary_stage<T>(timings: nia_driver::TimingMode, name: &str, f: impl FnOnce() -> T) -> T {
    nia_timing::time_stage(timings, nia_timing::TimingLevel::Summary, name, f)
}

fn run_emit_checked(path: &str, source: &str, runtime: Runtime, context: EmitContext) -> ExitCode {
    let output = time_summary_stage(context.timings, "check", || {
        check_with_driver(
            path,
            context.module_map,
            context.optimization,
            context.timings,
            runtime,
            context.toolchain,
        )
    });
    let program = match checked_program_from_output(output, path, source) {
        Ok(program) => program,
        Err(code) => return code,
    };
    println!("{program:#?}");
    ExitCode::SUCCESS
}

fn run_emit_backend(path: &str, source: &str, runtime: Runtime, context: EmitContext) -> ExitCode {
    let driver = nia_driver::Driver::new(context.toolchain);
    let output = time_summary_stage(context.timings, "codegen", || {
        codegen_with_driver(
            &driver,
            path,
            context.module_map,
            context.optimization,
            context.timings,
            runtime,
        )
    });
    let program = match codegen_program_from_output(output, path, source) {
        Ok(program) => program,
        Err(code) => return code,
    };
    if context.opt_report {
        print_optimization_report_to_stderr(&program);
    }
    println!("{:#?}", program.backend_lowering.program);
    ExitCode::SUCCESS
}

fn run_emit_llvm(path: &str, source: &str, runtime: Runtime, context: EmitContext) -> ExitCode {
    let driver = nia_driver::Driver::new(context.toolchain);
    let output = time_summary_stage(context.timings, "emit_llvm_ir", || {
        driver.emit_llvm_ir(nia_driver::EmitLlvmRequest::new(
            nia_driver::CheckRequest::new(path)
                .with_module_map(context.module_map)
                .with_optimization(context.optimization)
                .with_timings(context.timings)
                .with_runtime(runtime),
        ))
    });
    match output.result {
        Ok(artifact) => {
            eprint!(
                "{}",
                nia_driver::render_llvm_ir_warnings(&artifact, Some(path), Some(source))
            );
            if context.opt_report {
                eprint!("{}", nia_driver::llvm_ir_optimization_report(&artifact));
            }
            for module in artifact.modules {
                print!("{}", module.ir);
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprint!(
                "{}",
                nia_driver::render_driver_error(&error, Some(path), Some(source))
            );
            ExitCode::FAILURE
        }
    }
}

fn run_emit_obj(path: &str, source: &str, args: Vec<String>, context: EmitContext) -> ExitCode {
    let options = match parse_emit_obj_options(path, args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let driver = nia_driver::Driver::with_config(nia_driver::DriverConfig {
        artifact_cache_dir: options.cache_dir.clone(),
        ..nia_driver::DriverConfig::new(context.toolchain)
    });
    let output = time_summary_stage(context.timings, "emit_native_objects", || {
        driver.emit_native_objects(nia_driver::EmitObjectRequest::new(
            nia_driver::CheckRequest::new(path)
                .with_module_map(context.module_map)
                .with_optimization(context.optimization)
                .with_timings(context.timings)
                .with_runtime(options.runtime),
        ))
    });
    let objects = match output.result {
        Ok(objects) => objects,
        Err(error) => {
            eprint!(
                "{}",
                nia_driver::render_driver_error(&error, Some(path), Some(source))
            );
            return ExitCode::FAILURE;
        }
    };
    if !objects.diagnostics.is_empty() {
        eprint!(
            "{}",
            nia_driver::render_object_warnings(&objects, Some(path), Some(source))
        );
    }
    if context.opt_report {
        eprint!("{}", nia_driver::object_optimization_report(&objects));
    }
    let output =
        driver.write_native_objects_from_artifact(&objects, options.output.into_driver_output());
    match output.result {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprint!(
                "{}",
                nia_driver::render_driver_error(&error, Some(path), Some(source))
            );
            ExitCode::FAILURE
        }
    }
}

fn run_emit_exe(path: &str, source: &str, args: Vec<String>, context: EmitContext) -> ExitCode {
    let options = match parse_emit_exe_options(path, args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let driver = nia_driver::Driver::with_config(nia_driver::DriverConfig {
        artifact_cache_dir: options.cache_dir.clone(),
        ..nia_driver::DriverConfig::new(context.toolchain)
    });
    let output = time_summary_stage(context.timings, "link_executable", || {
        driver.link_executable(nia_driver::LinkExecutableRequest {
            check: nia_driver::CheckRequest::new(path)
                .with_module_map(context.module_map)
                .with_optimization(context.optimization)
                .with_timings(context.timings)
                .with_runtime(Runtime::Freestanding),
            output: options.output.clone(),
            link_options: options.link_options.clone(),
        })
    });
    let executable = match output.result {
        Ok(executable) => executable,
        Err(error) => {
            eprint!(
                "{}",
                nia_driver::render_driver_error(&error, Some(path), Some(source))
            );
            return ExitCode::FAILURE;
        }
    };
    if !executable.diagnostics.is_empty() {
        eprint!(
            "{}",
            nia_driver::render_executable_warnings(&executable, Some(path), Some(source))
        );
    }
    if context.opt_report {
        eprint!(
            "{}",
            nia_driver::optimization_report_from_parts(
                executable.optimization,
                &executable.optimization_report,
            )
        );
    }
    ExitCode::SUCCESS
}

struct EmitObjOptions {
    output: EmitObjOutput,
    runtime: Runtime,
    cache_dir: Option<PathBuf>,
}

enum EmitObjOutput {
    Single(PathBuf),
    Directory(PathBuf),
}

impl EmitObjOutput {
    fn into_driver_output(self) -> nia_driver::ObjectOutput {
        match self {
            Self::Single(path) => nia_driver::ObjectOutput::Single(path),
            Self::Directory(path) => nia_driver::ObjectOutput::Directory(path),
        }
    }
}

fn parse_emit_obj_options(source: &str, args: Vec<String>) -> Result<EmitObjOptions, String> {
    let mut output = None::<PathBuf>;
    let mut out_dir = None::<PathBuf>;
    let mut runtime = Runtime::Bare;
    let mut cache_dir = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--runtime=") {
            runtime = parse_runtime(value)
                .map_err(|message| format!("{message} for `nia emit --obj`"))?;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--cache-dir=") {
            cache_dir = Some(PathBuf::from(value));
            continue;
        }
        match arg.as_str() {
            "-o" => {
                let Some(path) = iter.next() else {
                    return Err("missing path after `-o`".to_string());
                };
                output = Some(PathBuf::from(path));
            }
            "--out-dir" => {
                let Some(path) = iter.next() else {
                    return Err("missing path after `--out-dir`".to_string());
                };
                out_dir = Some(PathBuf::from(path));
            }
            "--runtime" => {
                let Some(value) = iter.next() else {
                    return Err("missing runtime after `--runtime`".to_string());
                };
                runtime = parse_runtime(&value)
                    .map_err(|message| format!("{message} for `nia emit --obj`"))?;
            }
            "--cache-dir" => {
                let Some(path) = iter.next() else {
                    return Err("missing path after `--cache-dir`".to_string());
                };
                cache_dir = Some(PathBuf::from(path));
            }
            _ => return Err(format!("unknown `nia emit --obj` option `{arg}`")),
        }
    }
    let output = match (output, out_dir) {
        (Some(_), Some(_)) => Err("use either `-o` or `--out-dir`, not both".to_string()),
        (Some(path), None) => Ok(EmitObjOutput::Single(path)),
        (None, Some(path)) => Ok(EmitObjOutput::Directory(path)),
        (None, None) => Ok(EmitObjOutput::Single(default_output_path(source, "o"))),
    }?;
    Ok(EmitObjOptions {
        output,
        runtime,
        cache_dir,
    })
}

struct EmitExeOptions {
    output: PathBuf,
    cache_dir: Option<PathBuf>,
    link_options: nia_linker::LinkOptions,
}

fn parse_emit_exe_options(source: &str, args: Vec<String>) -> Result<EmitExeOptions, String> {
    let mut output = None::<PathBuf>;
    let mut cache_dir = None::<PathBuf>;
    let mut link_options = nia_linker::LinkOptions::default();
    let mut explicit_linker_program = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--cache-dir=") {
            cache_dir = Some(PathBuf::from(value));
            continue;
        }
        if let Some(value) = arg.strip_prefix("--runtime=") {
            let runtime = parse_runtime(value)
                .map_err(|message| format!("{message} for `nia emit --exe`"))?;
            if runtime != Runtime::Freestanding {
                return Err(
                    "`nia emit --exe` currently supports only `--runtime freestanding`".to_string(),
                );
            }
            continue;
        }
        if let Some(value) = arg.strip_prefix("--link-arg=") {
            link_options.raw_args.push(value.to_string());
            continue;
        }
        if let Some(value) = arg.strip_prefix("--dynamic-linker=") {
            link_options = link_options
                .with_dynamic_mode()
                .with_dynamic_linker(parse_dynamic_linker(value));
            continue;
        }
        if let Some(value) = arg.strip_prefix("--library-path=") {
            link_options = link_options.add_library_path(value);
            continue;
        }
        if let Some(value) = arg.strip_prefix("-L")
            && !value.is_empty()
        {
            link_options = link_options.add_library_path(value);
            continue;
        }
        if let Some(value) = arg.strip_prefix("--library=") {
            link_options = link_options.add_library(value);
            continue;
        }
        if let Some(value) = arg.strip_prefix("-l")
            && !value.is_empty()
        {
            link_options = link_options.add_library(value);
            continue;
        }
        if let Some(value) = arg.strip_prefix("--rpath=") {
            link_options = link_options.add_rpath(value);
            continue;
        }
        if let Some(value) = arg.strip_prefix("--linker=") {
            link_options.linker.program = value.to_string();
            explicit_linker_program = true;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--linker-flavor=") {
            let flavor = parse_linker_flavor(value)?;
            link_options.linker.flavor = flavor;
            if flavor == nia_linker::LinkerFlavor::Lld && !explicit_linker_program {
                link_options.linker = nia_linker::ExecutableLinker::lld();
            }
            continue;
        }
        match arg.as_str() {
            "-o" => {
                let Some(path) = iter.next() else {
                    return Err("missing path after `-o`".to_string());
                };
                output = Some(PathBuf::from(path));
            }
            "--cache-dir" => {
                let Some(path) = iter.next() else {
                    return Err("missing path after `--cache-dir`".to_string());
                };
                cache_dir = Some(PathBuf::from(path));
            }
            "--runtime" => {
                let Some(value) = iter.next() else {
                    return Err("missing runtime after `--runtime`".to_string());
                };
                let runtime = parse_runtime(&value)
                    .map_err(|message| format!("{message} for `nia emit --exe`"))?;
                if runtime != Runtime::Freestanding {
                    return Err(
                        "`nia emit --exe` currently supports only `--runtime freestanding`"
                            .to_string(),
                    );
                }
            }
            "--link-arg" => {
                let Some(value) = iter.next() else {
                    return Err("missing argument after `--link-arg`".to_string());
                };
                link_options.raw_args.push(value);
            }
            "--dynamic-linker" => {
                let Some(value) = iter.next() else {
                    return Err("missing path after `--dynamic-linker`".to_string());
                };
                link_options = link_options
                    .with_dynamic_mode()
                    .with_dynamic_linker(parse_dynamic_linker(&value));
            }
            "--no-dynamic-linker" => {
                link_options = link_options
                    .with_dynamic_mode()
                    .with_dynamic_linker(nia_linker::DynamicLinker::None);
            }
            "--library-path" | "-L" => {
                let Some(value) = iter.next() else {
                    return Err(format!("missing path after `{arg}`"));
                };
                link_options = link_options.add_library_path(value);
            }
            "--library" | "-l" => {
                let Some(value) = iter.next() else {
                    return Err(format!("missing library name after `{arg}`"));
                };
                link_options = link_options.add_library(value);
            }
            "--rpath" => {
                let Some(value) = iter.next() else {
                    return Err("missing path after `--rpath`".to_string());
                };
                link_options = link_options.add_rpath(value);
            }
            "--linker" => {
                let Some(value) = iter.next() else {
                    return Err("missing program after `--linker`".to_string());
                };
                link_options.linker.program = value;
                explicit_linker_program = true;
            }
            "--linker-flavor" => {
                let Some(value) = iter.next() else {
                    return Err("missing flavor after `--linker-flavor`".to_string());
                };
                let flavor = parse_linker_flavor(&value)?;
                link_options.linker.flavor = flavor;
                if flavor == nia_linker::LinkerFlavor::Lld && !explicit_linker_program {
                    link_options.linker = nia_linker::ExecutableLinker::lld();
                }
            }
            _ => return Err(format!("unknown `nia emit --exe` option `{arg}`")),
        }
    }
    Ok(EmitExeOptions {
        output: output.unwrap_or_else(|| default_output_path(source, env::consts::EXE_EXTENSION)),
        cache_dir,
        link_options,
    })
}

fn parse_linker_flavor(value: &str) -> Result<nia_linker::LinkerFlavor, String> {
    match value {
        "gnu" => Ok(nia_linker::LinkerFlavor::Gnu),
        "lld" => Ok(nia_linker::LinkerFlavor::Lld),
        "self-hosted-elf" => Ok(nia_linker::LinkerFlavor::SelfHostedElf),
        _ => Err(format!(
            "unknown linker flavor `{value}`; expected `gnu`, `lld`, or `self-hosted-elf`"
        )),
    }
}

fn parse_dynamic_linker(value: &str) -> nia_linker::DynamicLinker {
    match value {
        "auto" => nia_linker::DynamicLinker::Auto,
        "none" => nia_linker::DynamicLinker::None,
        path => nia_linker::DynamicLinker::Path(path.to_string()),
    }
}

fn parse_runtime(value: &str) -> Result<Runtime, String> {
    match value {
        "bare" => Ok(Runtime::Bare),
        "freestanding" => Ok(Runtime::Freestanding),
        _ => Err(format!(
            "unknown runtime `{value}`; expected `bare` or `freestanding`"
        )),
    }
}

fn default_output_path(source: &str, extension: &str) -> PathBuf {
    let mut path = PathBuf::from(source);
    path.set_extension(extension);
    path
}

fn print_optimization_report(program: &nia_driver::CodegenProgram) {
    print!("{}", nia_driver::optimization_report(program));
}

fn print_optimization_report_to_stderr(program: &nia_driver::CodegenProgram) {
    eprint!("{}", nia_driver::optimization_report(program));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ice_boundary_converts_panic_to_failure() {
        let mut message = String::new();
        let code = run_with_ice_boundary(
            TimingOptions::default(),
            || panic!("Nia ICE: forced failure"),
            |ice| message = ice.render_message(),
        );

        assert_eq!(code, ExitCode::FAILURE);
        assert!(message.contains("internal compiler error: forced failure"));
        assert!(message.contains("Please report it"));
    }

    #[test]
    fn build_jobs_accept_long_and_short_forms() {
        for args in [
            vec!["--jobs=3"],
            vec!["--jobs", "3"],
            vec!["-j3"],
            vec!["-j", "3"],
        ] {
            let command = match parse_build_command(
                args.into_iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
            ) {
                Ok(command) => command,
                Err(error) => panic!("parse build jobs: {}", error.message),
            };
            assert!(matches!(
                command,
                CliCommand::Build { jobs: Some(jobs), .. } if jobs.get() == 3
            ));
        }
    }

    #[test]
    fn build_jobs_reject_missing_zero_and_invalid_counts() {
        for (args, expected) in [
            (vec!["--jobs"], "missing count after `--jobs`"),
            (vec!["-j"], "missing count after `-j`"),
            (vec!["--jobs=0"], "expected a positive integer"),
            (vec!["-jinvalid"], "expected a positive integer"),
        ] {
            let error = parse_build_command(
                args.into_iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
            )
            .expect_err("invalid build jobs must fail");
            assert!(error.message.contains(expected), "{}", error.message);
        }
    }
}
