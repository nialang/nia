// SPDX-License-Identifier: GPL-3.0-or-later
//! Command-line ownership for compiler inspection, checking, emission, and builds.
//!
//! Global target, module-map, optimization, timing, and toolchain options are
//! parsed once and then translated into typed `nia-driver` or `nia-build`
//! requests. Native outputs remain file-only, inspection output uses stdout,
//! and diagnostics/timings use stderr. Compiler failures reach this boundary as
//! structured diagnostics rather than unwinding through the CLI.
//!
//! The commands are `build` and `test`, which run a package's `build.nia`
//! through `nia-build`, `check`, which validates without code generation, and
//! `emit`, which writes one product selected by `--tokens`, `--ast`,
//! `--checked`, `--backend`, `--llvm`, `--obj`, or `--exe`. Global options
//! include `-O0` through `-Oz`, `-M name=path` module mappings, and
//! `--timings`; `--opt-report` prints the backend optimization report.
#[cfg(feature = "perf-alloc")]
use std::alloc::System;
use std::{
    collections::BTreeMap,
    env, fmt, fs, io,
    io::{IsTerminal, Write},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossterm::{
    cursor::{MoveToColumn, MoveUp},
    execute,
    style::Print,
    terminal::{self, Clear, ClearType},
};
use unicode_width::UnicodeWidthChar;

use nia_driver::{ModuleMap, NiaOptimizationLevel, RuntimeSpec, SourcePath};
use nia_target_config::BuildProfile;
use nia_timing::{TimingFormat, TimingOptions, TimingTrace};

mod help;

use help::{HelpStyle, error_help_command, error_help_text, help_text};

#[cfg(feature = "perf-alloc")]
#[global_allocator]
static GLOBAL_ALLOCATOR: nia_timing::CountingAllocator<System> =
    nia_timing::CountingAllocator::new(System);

fn main() -> ExitCode {
    #[cfg(feature = "perf-alloc")]
    nia_timing::register_allocation_instrumentation();
    let args = env::args().skip(1).collect::<Vec<_>>();
    let diagnostics_format = requested_diagnostics_format(&args);
    match parse_cli(args) {
        Ok(CliAction::Help(topic)) => write_stdout(format_args!(
            "{}",
            help_text(topic, HelpStyle::for_stdout())
        )),
        Ok(CliAction::Version) => {
            write_stdout(format_args!("nia {}\n", nia_compat::COMPILER_VERSION))
        }
        Ok(CliAction::Run(mut cli)) => {
            if let Err(error) = resolve_output_policy(&mut cli) {
                report_cli_error(&error.message, error.help, cli.diagnostics_format);
                return ExitCode::FAILURE;
            }
            let timing_options = cli.timing_options();
            nia_timing::collect_to_stderr(timing_options, || run_cli(cli))
        }
        Err(error) => {
            if error.is_help {
                return write_stdout(format_args!(
                    "{}",
                    help_text(error.help, HelpStyle::for_stdout())
                ));
            }
            report_cli_error(&error.message, error.help, diagnostics_format);
            ExitCode::FAILURE
        }
    }
}

fn write_stdout(args: fmt::Arguments<'_>) -> ExitCode {
    match io::stdout().lock().write_fmt(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: failed printing to stdout: {error}");
            ExitCode::FAILURE
        }
    }
}

fn report_cli_error(message: &str, help: HelpTopic, diagnostics_format: DiagnosticsFormat) {
    let rendered = render_cli_invocation_error(message, help, diagnostics_format);
    write_diagnostic_report(&rendered, diagnostics_format);
    if diagnostics_format == DiagnosticsFormat::Text {
        eprintln!();
        eprint!("{}", error_help_text(help, HelpStyle::for_stderr()));
    }
}

fn write_diagnostic_report(rendered: &str, format: DiagnosticsFormat) {
    if format == DiagnosticsFormat::Text
        && io::stderr().is_terminal()
        && std::env::var_os("NO_COLOR").is_none()
    {
        eprint!("{}", colorize_diagnostic_report(rendered));
    } else {
        eprint!("{rendered}");
    }
}

fn colorize_diagnostic_report(rendered: &str) -> String {
    const RESET: &str = "\x1b[0m";
    const RED: &str = "\x1b[1;31m";
    const YELLOW: &str = "\x1b[1;33m";
    const CYAN: &str = "\x1b[36m";
    const BLUE: &str = "\x1b[34m";
    const GREEN: &str = "\x1b[32m";
    const MAGENTA: &str = "\x1b[35m";
    let mut output = String::with_capacity(rendered.len() + rendered.len() / 4);
    for line in rendered.split_inclusive('\n') {
        let color = if line.starts_with("error") {
            RED
        } else if line.starts_with("warning") {
            YELLOW
        } else if line.starts_with("  -->") {
            CYAN
        } else if line.starts_with("note:") {
            BLUE
        } else if line.starts_with("help:") || line.starts_with("suggestion") {
            GREEN
        } else if line.starts_with("related:") {
            MAGENTA
        } else {
            ""
        };
        if color.is_empty() {
            output.push_str(line);
        } else {
            output.push_str(color);
            output.push_str(line);
            output.push_str(RESET);
        }
    }
    output
}

fn requested_diagnostics_format(args: &[String]) -> DiagnosticsFormat {
    args.iter()
        .find_map(|arg| arg.strip_prefix("--diagnostics-format="))
        .map_or(DiagnosticsFormat::Text, |value| {
            if value == "json" {
                DiagnosticsFormat::Json
            } else {
                DiagnosticsFormat::Text
            }
        })
}

fn render_cli_invocation_error(
    message: &str,
    help_topic: HelpTopic,
    format: DiagnosticsFormat,
) -> String {
    let help = format!(
        "for more information, run `{}`",
        error_help_command(help_topic)
    );
    let diagnostic =
        nia_diagnostic::Diagnostic::user_error(nia_diagnostic::codes::CLI_USAGE, message)
            .help(help)
            .finish();
    match format {
        DiagnosticsFormat::Text => {
            nia_diagnostic::render_diagnostic("<command line>", "", &diagnostic)
        }
        DiagnosticsFormat::Json => nia_diagnostic::render_diagnostics_json_at(
            "<command line>",
            &[diagnostic],
            nia_diagnostic::DiagnosticReportConfig::default(),
        ),
    }
}

fn render_cli_boundary_error(
    code: nia_diagnostic::codes::DiagnosticCodeDef,
    message: &str,
    format: DiagnosticsFormat,
) -> String {
    let help = match code {
        nia_diagnostic::codes::TOOLCHAIN => {
            "check the Nia installation, resource root, and toolchain manifest"
        }
        nia_diagnostic::codes::LOAD => {
            "check the source path, package layout, and filesystem permissions"
        }
        _ => "check the command options and target configuration",
    };
    let diagnostic = nia_diagnostic::Diagnostic::user_error(code, message)
        .help(help)
        .finish();
    match format {
        DiagnosticsFormat::Text => {
            nia_diagnostic::render_diagnostic("<command line>", "", &diagnostic)
        }
        DiagnosticsFormat::Json => nia_diagnostic::render_diagnostics_json_at(
            "<command line>",
            &[diagnostic],
            nia_diagnostic::DiagnosticReportConfig::default(),
        ),
    }
}

#[derive(Debug)]
struct Cli {
    resource_root: Option<PathBuf>,
    target: Option<nia_target::TargetConfig>,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    profile: BuildProfile,
    timings: nia_driver::TimingMode,
    timing_trace: TimingTrace,
    timing_format: TimingFormat,
    timing_format_explicit: bool,
    diagnostics_format: DiagnosticsFormat,
    diagnostics_format_explicit: bool,
    command: CliCommand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiagnosticsFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProgressMode {
    Auto,
    Plain,
    Json,
    None,
}

fn parse_progress_mode(value: &str, help: HelpTopic) -> Result<ProgressMode, CliError> {
    match value {
        "auto" => Ok(ProgressMode::Auto),
        "plain" => Ok(ProgressMode::Plain),
        "none" => Ok(ProgressMode::None),
        "json" => Ok(ProgressMode::Json),
        _ => Err(CliError::new(
            format!("invalid progress mode `{value}`; expected auto, plain, json, or none"),
            help,
        )),
    }
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
        link_time_optimization: nia_driver::LinkTimeOptimization,
        dry_run: bool,
        explain: bool,
        print_plan: bool,
        print_cache: bool,
        print_observations: bool,
        quiet: u8,
        verbose: u8,
        progress: ProgressMode,
    },
    Test {
        root: Option<PathBuf>,
        filter: Option<String>,
        list: bool,
        fail_fast: bool,
        jobs: Option<NonZeroUsize>,
        quiet: u8,
        verbose: u8,
        progress: ProgressMode,
    },
    Check {
        path: String,
        opt_report: bool,
        runtime: RuntimeMode,
        cache_dir: Option<PathBuf>,
    },
    Emit {
        path: String,
        target: EmitTarget,
        opt_report: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeMode {
    Bare,
    Freestanding,
}

impl RuntimeMode {
    fn resolve(
        self,
        toolchain: &nia_toolchain::ToolchainLayout,
    ) -> Result<RuntimeSpec, nia_toolchain::RuntimeSpecError> {
        match self {
            Self::Bare => Ok(RuntimeSpec::Bare),
            Self::Freestanding => RuntimeSpec::freestanding(toolchain, toolchain.artifact_target()),
        }
    }
}

#[derive(Debug)]
enum EmitTarget {
    Tokens,
    Ast,
    Checked { runtime: RuntimeMode },
    Backend { runtime: RuntimeMode },
    Llvm { runtime: RuntimeMode },
    Obj { args: Vec<String> },
    Exe { args: Vec<String> },
}

#[derive(Debug)]
enum CliAction {
    Help(HelpTopic),
    Version,
    Run(Cli),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelpTopic {
    Main,
    Build,
    Test,
    Check,
    Emit,
    EmitTokens,
    EmitAst,
    EmitChecked,
    EmitBackend,
    EmitLlvm,
    EmitObj,
    EmitExe,
}

#[derive(Debug)]
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
    let diagnostics_format = cli.diagnostics_format;
    let timing_format = cli.timing_format;
    let toolchain = match resolve_toolchain_layout(cli.resource_root, cli.target) {
        Ok(toolchain) => toolchain,
        Err(message) => {
            write_diagnostic_report(
                &render_cli_boundary_error(
                    nia_diagnostic::codes::TOOLCHAIN,
                    &message,
                    diagnostics_format,
                ),
                diagnostics_format,
            );
            return ExitCode::FAILURE;
        }
    };
    match cli.command {
        CliCommand::Build {
            root,
            step,
            jobs,
            link_time_optimization,
            dry_run,
            explain,
            print_plan,
            print_cache,
            print_observations,
            quiet,
            verbose,
            progress,
        } => run_build(BuildContext {
            root,
            step,
            jobs,
            link_time_optimization,
            dry_run,
            explain,
            print_plan,
            print_cache,
            print_observations,
            quiet,
            verbose,
            progress,
            optimization: cli.optimization,
            profile: cli.profile,
            timings: cli.timings,
            timing_format,
            diagnostics_format: cli.diagnostics_format,
            toolchain,
        }),
        CliCommand::Test {
            root,
            filter,
            list,
            fail_fast,
            jobs,
            quiet,
            verbose,
            progress,
        } => run_test(TestContext {
            root,
            filter,
            list,
            fail_fast,
            jobs,
            quiet,
            verbose,
            progress,
            optimization: cli.optimization,
            profile: cli.profile,
            timings: cli.timings,
            timing_format,
            diagnostics_format: cli.diagnostics_format,
            toolchain,
        }),
        CliCommand::Check {
            path,
            opt_report,
            runtime,
            cache_dir,
        } => {
            let path = match resolve_source_entry(&path) {
                Ok(path) => path,
                Err(message) => {
                    write_diagnostic_report(
                        &render_cli_boundary_error(
                            nia_diagnostic::codes::LOAD,
                            &message,
                            diagnostics_format,
                        ),
                        diagnostics_format,
                    );
                    return ExitCode::FAILURE;
                }
            };
            let source = match read_source(&path) {
                Ok(source) => source,
                Err(message) => {
                    write_diagnostic_report(
                        &render_cli_boundary_error(
                            nia_diagnostic::codes::LOAD,
                            &message,
                            diagnostics_format,
                        ),
                        diagnostics_format,
                    );
                    return ExitCode::FAILURE;
                }
            };
            let package_root = match discover_package_root(&path) {
                Ok(package_root) => package_root,
                Err(message) => {
                    write_diagnostic_report(
                        &render_cli_boundary_error(
                            nia_diagnostic::codes::LOAD,
                            &message,
                            diagnostics_format,
                        ),
                        diagnostics_format,
                    );
                    return ExitCode::FAILURE;
                }
            };
            run_check(
                &path,
                &source,
                cli.module_map,
                CheckRunOptions {
                    optimization: cli.optimization,
                    profile: cli.profile,
                    timings: cli.timings,
                    opt_report,
                    runtime,
                    cache_dir,
                    package_root,
                    diagnostics_format: cli.diagnostics_format,
                },
                toolchain,
            )
        }
        CliCommand::Emit {
            path,
            target,
            opt_report,
        } => {
            let path = match resolve_source_entry(&path) {
                Ok(path) => path,
                Err(message) => {
                    write_diagnostic_report(
                        &render_cli_boundary_error(
                            nia_diagnostic::codes::LOAD,
                            &message,
                            diagnostics_format,
                        ),
                        diagnostics_format,
                    );
                    return ExitCode::FAILURE;
                }
            };
            let source = match read_source(&path) {
                Ok(source) => source,
                Err(message) => {
                    write_diagnostic_report(
                        &render_cli_boundary_error(
                            nia_diagnostic::codes::LOAD,
                            &message,
                            diagnostics_format,
                        ),
                        diagnostics_format,
                    );
                    return ExitCode::FAILURE;
                }
            };
            let package_root = match discover_package_root(&path) {
                Ok(package_root) => package_root,
                Err(message) => {
                    write_diagnostic_report(
                        &render_cli_boundary_error(
                            nia_diagnostic::codes::LOAD,
                            &message,
                            diagnostics_format,
                        ),
                        diagnostics_format,
                    );
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
                    profile: cli.profile,
                    timings: cli.timings,
                    opt_report,
                    package_root,
                    toolchain,
                    diagnostics_format: cli.diagnostics_format,
                },
            )
        }
    }
}

fn resolve_toolchain_layout(
    resource_root: Option<PathBuf>,
    target: Option<nia_target::TargetConfig>,
) -> Result<Arc<nia_toolchain::ToolchainLayout>, String> {
    let executable = env::current_exe()
        .map_err(|error| format!("failed to resolve compiler executable: {error}"))?;
    let request = match resource_root {
        Some(resource_root) => {
            nia_toolchain::ToolchainLayoutRequest::explicit(executable, resource_root)
        }
        None => nia_toolchain::ToolchainLayoutRequest::installed(executable),
    };
    // An explicit target is the artifact target for every command; build
    // scripts and host tools still compile for the host.
    let request = match target {
        Some(target) => request.with_artifact_target(target),
        None => request,
    };
    nia_toolchain::ToolchainLayout::resolve(request)
        .map(Arc::new)
        .map_err(|error| format!("invalid toolchain layout: {error}"))
}

fn read_source(path: &str) -> Result<String, String> {
    let source = match nia_source::read_source_text(path) {
        Ok(source) => source,
        Err(err) => {
            return Err(format!("failed to read `{path}`: {err}"));
        }
    };
    Ok(source)
}

/// Resolves a file-or-package argument used by checking and emission.
///
/// A package directory prefers its executable entry (`main.nia`) and falls
/// back to the package root (`pkg.nia`) for library-only packages.
fn resolve_source_entry(path: &str) -> Result<String, String> {
    let candidate = Path::new(path);
    match fs::metadata(candidate) {
        Ok(metadata) if metadata.is_file() => Ok(path.to_string()),
        Ok(metadata) if metadata.is_dir() => {
            let main = candidate.join("main.nia");
            if main.is_file() {
                return Ok(main.to_string_lossy().into_owned());
            }
            let package = candidate.join("pkg.nia");
            if package.is_file() {
                return Ok(package.to_string_lossy().into_owned());
            }
            Err(format!(
                "source directory `{path}` contains neither `main.nia` nor `pkg.nia`"
            ))
        }
        Ok(_) => Err(format!(
            "source path `{path}` is neither a file nor a directory"
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path.to_string()),
        Err(error) => Err(format!("cannot inspect source path `{path}`: {error}")),
    }
}

/// Finds the nearest conventional package root for a source entry.
fn discover_package_root(path: &str) -> Result<Option<SourcePath>, String> {
    let source = Path::new(path);
    if source.file_name().is_some_and(|name| name == "pkg.nia") {
        return Ok(None);
    }
    let mut directory = source
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    loop {
        let candidate = directory.join("pkg.nia");
        match fs::metadata(&candidate) {
            Ok(metadata) if metadata.is_file() => {
                return Ok(Some(SourcePath::new(
                    candidate.to_string_lossy().into_owned(),
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "cannot inspect package root `{}`: {error}",
                    candidate.display()
                ));
            }
        }
        if !directory.pop() {
            return Ok(None);
        }
    }
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
    let optimization = global_options
        .optimization
        .unwrap_or_else(|| default_profile_optimization(global_options.profile));
    match parse_command(remaining)? {
        ParsedCommand::Help(topic) => Ok(CliAction::Help(topic)),
        ParsedCommand::Run(command) => Ok(CliAction::Run(Cli {
            resource_root: global_options.resource_root,
            target: global_options.target,
            module_map: global_options.module_map,
            optimization,
            profile: global_options.profile,
            timings: global_options.timings,
            timing_trace: global_options.timing_trace,
            timing_format: global_options.timing_format,
            timing_format_explicit: global_options.timing_format_explicit,
            diagnostics_format: global_options.diagnostics_format,
            diagnostics_format_explicit: global_options.diagnostics_format_explicit,
            command,
        })),
    }
}

struct GlobalOptions {
    resource_root: Option<PathBuf>,
    target: Option<nia_target::TargetConfig>,
    module_map: ModuleMap,
    optimization: Option<NiaOptimizationLevel>,
    profile: BuildProfile,
    timings: nia_driver::TimingMode,
    timing_trace: TimingTrace,
    timing_format: TimingFormat,
    timing_format_explicit: bool,
    diagnostics_format: DiagnosticsFormat,
    diagnostics_format_explicit: bool,
}

fn extract_global_options(
    args: Vec<String>,
    help: HelpTopic,
) -> Result<(Vec<String>, GlobalOptions), CliError> {
    let mut map = ModuleMap::new();
    let mut resource_root = None;
    let mut target = None;
    let mut optimization = None;
    let mut profile = BuildProfile::default();
    let mut timings = nia_driver::TimingMode::Off;
    let mut timing_trace = TimingTrace::Off;
    let mut timing_format = TimingFormat::Text;
    let mut timing_format_explicit = false;
    let mut diagnostics_format = DiagnosticsFormat::Text;
    let mut diagnostics_format_explicit = false;
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
        if arg == "--target" {
            let name = iter
                .next()
                .ok_or_else(|| CliError::new("missing target name after `--target`", help))?;
            set_target(&mut target, &name, help)?;
            continue;
        }
        if let Some(name) = arg.strip_prefix("--target=") {
            set_target(&mut target, name, help)?;
            continue;
        }
        if emit_target_option_takes_value(&arg) {
            preserve_next = true;
            remaining.push(arg);
            continue;
        }
        if let Some(level) = parse_optimization_flag(&arg) {
            optimization = Some(level.map_err(|message| CliError::new(message, help))?);
            continue;
        }
        if matches!(arg.as_str(), "--debug" | "--release") {
            profile = if arg == "--debug" {
                BuildProfile::Debug
            } else {
                BuildProfile::Release
            };
            continue;
        }
        if arg == "--profile" {
            let value = iter
                .next()
                .ok_or_else(|| CliError::new("missing profile after `--profile`", help))?;
            profile = parse_profile(&value).map_err(|message| CliError::new(message, help))?;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--profile=") {
            profile = parse_profile(value).map_err(|message| CliError::new(message, help))?;
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
            timing_format_explicit = true;
            timing_format = format.map_err(|message| CliError::new(message, help))?;
            continue;
        }
        if let Some(format) = parse_diagnostics_format_flag(&arg) {
            diagnostics_format_explicit = true;
            diagnostics_format = format.map_err(|message| CliError::new(message, help))?;
            continue;
        }
        remaining.push(arg);
    }
    Ok((
        remaining,
        GlobalOptions {
            resource_root,
            target,
            module_map: map,
            optimization,
            profile,
            timings,
            timing_trace,
            timing_format,
            timing_format_explicit,
            diagnostics_format,
            diagnostics_format_explicit,
        },
    ))
}

fn set_target(
    slot: &mut Option<nia_target::TargetConfig>,
    name: &str,
    help: HelpTopic,
) -> Result<(), CliError> {
    let target = nia_target::TargetConfig::parse(name)
        .map_err(|error| CliError::new(error.to_string(), help))?;
    if slot.replace(target).is_some() {
        return Err(CliError::new("`--target` may be specified only once", help));
    }
    Ok(())
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
            | "--cache-dir"
            | "--link-arg"
            | "--executable"
            | "--interpreter"
            | "--library-path"
            | "-L"
            | "--library"
            | "-l"
            | "--rpath"
            | "--linker"
            | "--linker-flavor"
            | "--lto"
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

fn parse_profile(value: &str) -> Result<BuildProfile, String> {
    match value {
        "debug" => Ok(BuildProfile::Debug),
        "release" => Ok(BuildProfile::Release),
        _ => Err(format!(
            "unknown profile `{value}`; expected debug or release"
        )),
    }
}

fn default_profile_optimization(profile: BuildProfile) -> NiaOptimizationLevel {
    match profile {
        BuildProfile::Debug => NiaOptimizationLevel::O0,
        BuildProfile::Release => NiaOptimizationLevel::O2,
    }
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

fn parse_diagnostics_format_flag(arg: &str) -> Option<Result<DiagnosticsFormat, String>> {
    let value = arg.strip_prefix("--diagnostics-format=")?;
    Some(match value {
        "text" => Ok(DiagnosticsFormat::Text),
        "json" => Ok(DiagnosticsFormat::Json),
        _ => Err(format!(
            "unknown diagnostics format `{value}`; expected text or json"
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
        "help" => help_topic_from_args(&rest).map(ParsedCommand::Help),
        "build" => parse_build_command(rest).map(ParsedCommand::Run),
        "test" => parse_test_command(rest).map(ParsedCommand::Run),
        "check" => parse_check_command(rest).map(ParsedCommand::Run),
        "emit" => parse_emit_command(rest).map(ParsedCommand::Run),
        _ => Err(CliError::new(
            format!("unknown command `{command}`"),
            HelpTopic::Main,
        )),
    }
}

fn help_topic_from_args(args: &[String]) -> Result<HelpTopic, CliError> {
    let args = args
        .iter()
        .filter(|arg| *arg != "-h" && *arg != "--help")
        .collect::<Vec<_>>();
    match args.as_slice() {
        [] => Ok(HelpTopic::Main),
        [command] if *command == "build" => Ok(HelpTopic::Build),
        [command] if *command == "test" => Ok(HelpTopic::Test),
        [command] if *command == "check" => Ok(HelpTopic::Check),
        [command] if *command == "emit" => Ok(HelpTopic::Emit),
        [command, target] if *command == "emit" => emit_target_flag(target)
            .map(ParsedEmitTarget::help_topic)
            .ok_or_else(|| {
                CliError::new(format!("unknown emit target `{target}`"), HelpTopic::Emit)
            }),
        [topic, ..] => Err(CliError::new(
            format!("unknown help topic `{topic}`"),
            HelpTopic::Main,
        )),
    }
}

fn parse_build_command(args: Vec<String>) -> Result<CliCommand, CliError> {
    if has_help_flag(&args) {
        return Err(CliError::help(HelpTopic::Build));
    }
    let mut root = None::<PathBuf>;
    let mut step = None::<String>;
    let mut jobs = None::<NonZeroUsize>;
    let mut link_time_optimization = nia_driver::LinkTimeOptimization::Off;
    let mut dry_run = false;
    let mut explain = false;
    let mut print_plan = false;
    let mut print_cache = false;
    let mut print_observations = false;
    let mut quiet = 0u8;
    let mut verbose = 0u8;
    let mut progress = ProgressMode::Auto;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--root=") {
            if value.is_empty() {
                return Err(CliError::new("`--root` cannot be empty", HelpTopic::Build));
            }
            root = Some(PathBuf::from(value));
            continue;
        }
        if let Some(value) = arg.strip_prefix("--progress=") {
            progress = parse_progress_mode(value, HelpTopic::Build)?;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--jobs=") {
            jobs = Some(parse_build_jobs(value)?);
            continue;
        }
        if let Some(value) = arg.strip_prefix("--lto=") {
            link_time_optimization = parse_link_time_optimization(value)
                .map_err(|message| CliError::new(message, HelpTopic::Build))?;
            continue;
        }
        if let Some(value) = arg.strip_prefix("-j").filter(|value| !value.is_empty()) {
            jobs = Some(parse_build_jobs(value)?);
            continue;
        }
        match arg.as_str() {
            "-q" | "--quiet" | "-qq" | "-qqq" => quiet = quiet.saturating_add(1),
            "-v" | "--verbose" => {
                if verbose == 3 {
                    return Err(CliError::new(
                        "verbosity may not exceed `-vvv`",
                        HelpTopic::Build,
                    ));
                }
                verbose += 1;
            }
            "-vv" => verbose = 2,
            "-vvv" => verbose = 3,
            _ if arg.starts_with('-')
                && arg.len() > 2
                && arg
                    .chars()
                    .all(|character| character == '-' || character == 'v') =>
            {
                return Err(CliError::new(
                    "verbosity may not exceed `-vvv`",
                    HelpTopic::Build,
                ));
            }
            "--progress" => {
                let Some(value) = iter.next() else {
                    return Err(CliError::new(
                        "missing mode after `--progress`",
                        HelpTopic::Build,
                    ));
                };
                progress = parse_progress_mode(&value, HelpTopic::Build)?;
            }
            "--dry-run" => dry_run = true,
            "--explain" => explain = true,
            "--print-plan" => print_plan = true,
            "--print-cache" => print_cache = true,
            "--print-observations" => print_observations = true,
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
            "--lto" => {
                let Some(value) = iter.next() else {
                    return Err(CliError::new(
                        "missing mode after `--lto`",
                        HelpTopic::Build,
                    ));
                };
                link_time_optimization = parse_link_time_optimization(&value)
                    .map_err(|message| CliError::new(message, HelpTopic::Build))?;
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::new(
                    format!("unknown `nia build` option `{arg}`"),
                    HelpTopic::Build,
                ));
            }
            _ if looks_like_path_argument(&arg) => {
                return Err(CliError::new(
                    format!("package path `{arg}` must be passed with `--root`"),
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
    if quiet > 0 && verbose > 0 {
        return Err(CliError::new(
            "`--quiet` and `--verbose` cannot be used together",
            HelpTopic::Build,
        ));
    }
    if quiet > 0 && progress == ProgressMode::Json {
        return Err(CliError::new(
            "`--quiet` cannot be combined with `--progress=json`",
            HelpTopic::Build,
        ));
    }
    Ok(CliCommand::Build {
        root,
        step,
        jobs,
        link_time_optimization,
        dry_run,
        explain,
        print_plan,
        print_cache,
        print_observations,
        quiet,
        verbose,
        progress,
    })
}

fn looks_like_path_argument(arg: &str) -> bool {
    matches!(arg, "." | "..") || arg.contains('/') || arg.contains('\\')
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

fn parse_test_command(args: Vec<String>) -> Result<CliCommand, CliError> {
    if has_help_flag(&args) {
        return Err(CliError::help(HelpTopic::Test));
    }
    let mut root = None;
    let mut filter = None;
    let mut list = false;
    let mut fail_fast = false;
    let mut jobs = None;
    let mut quiet = 0u8;
    let mut verbose = 0u8;
    let mut progress = ProgressMode::Auto;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--root=") {
            if value.is_empty() {
                return Err(CliError::new("`--root` cannot be empty", HelpTopic::Test));
            }
            root = Some(PathBuf::from(value));
            continue;
        }
        if let Some(value) = arg.strip_prefix("--progress=") {
            progress = parse_progress_mode(value, HelpTopic::Test)?;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--filter=") {
            filter = Some(value.to_string());
            continue;
        }
        if let Some(value) = arg.strip_prefix("--jobs=") {
            jobs = Some(parse_test_jobs(value)?);
            continue;
        }
        if let Some(value) = arg.strip_prefix("-j").filter(|value| !value.is_empty()) {
            jobs = Some(parse_test_jobs(value)?);
            continue;
        }
        match arg.as_str() {
            "-q" | "--quiet" | "-qq" | "-qqq" => quiet = quiet.saturating_add(1),
            "-v" | "--verbose" => {
                if verbose == 3 {
                    return Err(CliError::new(
                        "verbosity may not exceed `-vvv`",
                        HelpTopic::Test,
                    ));
                }
                verbose += 1;
            }
            "-vv" => verbose = 2,
            "-vvv" => verbose = 3,
            _ if arg.starts_with('-')
                && arg.len() > 2
                && arg
                    .chars()
                    .all(|character| character == '-' || character == 'v') =>
            {
                return Err(CliError::new(
                    "verbosity may not exceed `-vvv`",
                    HelpTopic::Test,
                ));
            }
            "--progress" => {
                let Some(value) = iter.next() else {
                    return Err(CliError::new(
                        "missing mode after `--progress`",
                        HelpTopic::Test,
                    ));
                };
                progress = parse_progress_mode(&value, HelpTopic::Test)?;
            }
            "--root" => {
                let Some(value) = iter.next() else {
                    return Err(CliError::new(
                        "missing path after `--root`",
                        HelpTopic::Test,
                    ));
                };
                root = Some(PathBuf::from(value));
            }
            "--filter" => {
                let Some(value) = iter.next() else {
                    return Err(CliError::new(
                        "missing value after `--filter`",
                        HelpTopic::Test,
                    ));
                };
                filter = Some(value);
            }
            "--list" => list = true,
            "--fail-fast" => fail_fast = true,
            "--jobs" | "-j" => {
                let Some(value) = iter.next() else {
                    return Err(CliError::new(
                        format!("missing count after `{arg}`"),
                        HelpTopic::Test,
                    ));
                };
                jobs = Some(parse_test_jobs(&value)?);
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::new(
                    format!("unknown `nia test` option `{arg}`"),
                    HelpTopic::Test,
                ));
            }
            _ => {
                return Err(CliError::new(
                    format!("unexpected argument `{arg}` for `nia test`"),
                    HelpTopic::Test,
                ));
            }
        }
    }
    if quiet > 0 && verbose > 0 {
        return Err(CliError::new(
            "`--quiet` and `--verbose` cannot be used together",
            HelpTopic::Test,
        ));
    }
    if quiet > 0 && progress == ProgressMode::Json {
        return Err(CliError::new(
            "`--quiet` cannot be combined with `--progress=json`",
            HelpTopic::Test,
        ));
    }
    Ok(CliCommand::Test {
        root,
        filter,
        list,
        fail_fast,
        jobs,
        quiet,
        verbose,
        progress,
    })
}

fn parse_test_jobs(value: &str) -> Result<NonZeroUsize, CliError> {
    value
        .parse::<usize>()
        .ok()
        .and_then(NonZeroUsize::new)
        .ok_or_else(|| {
            CliError::new(
                format!("invalid test job count `{value}`; expected a positive integer"),
                HelpTopic::Test,
            )
        })
}

fn parse_check_command(args: Vec<String>) -> Result<CliCommand, CliError> {
    if has_help_flag(&args) {
        return Err(CliError::help(HelpTopic::Check));
    }
    let mut path = None;
    let mut opt_report = false;
    let mut runtime = RuntimeMode::Bare;
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
        return Err(CliError::help(emit_help_topic(&args)));
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
            None if looks_like_emit_target_name(&arg) && path.is_none() => {
                return Err(CliError::new(
                    format!("emit targets are flags; use `--{arg}`"),
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
                || arg.starts_with("--cache-dir=")
                || arg.starts_with("--lto=")
                || arg.starts_with("--link-arg=")
                || arg.starts_with("--executable=")
                || arg.starts_with("--interpreter=")
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
                let help = target.map_or(HelpTopic::Emit, ParsedEmitTarget::help_topic);
                return Err(CliError::new(
                    format!("unknown `nia emit` option `{arg}`"),
                    help,
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
    let target_help = target.help_topic();
    let (runtime, target_args) = if target.accepts_runtime() {
        parse_emit_runtime_args(target_args, target_help)?
    } else {
        (RuntimeMode::Bare, target_args)
    };
    let Some(path) = path else {
        return Err(CliError::new(
            "missing source file for `nia emit`",
            target_help,
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
            target_help,
        ));
    }
    if opt_report && !target.accepts_opt_report() {
        return Err(CliError::new(
            format!(
                "`--opt-report` is not valid for `nia emit {}`",
                target.flag_name()
            ),
            target_help,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    fn help_topic(self) -> HelpTopic {
        match self {
            Self::Tokens => HelpTopic::EmitTokens,
            Self::Ast => HelpTopic::EmitAst,
            Self::Checked => HelpTopic::EmitChecked,
            Self::Backend => HelpTopic::EmitBackend,
            Self::Llvm => HelpTopic::EmitLlvm,
            Self::Obj => HelpTopic::EmitObj,
            Self::Exe => HelpTopic::EmitExe,
        }
    }
}

fn parse_emit_runtime_args(
    args: Vec<String>,
    help: HelpTopic,
) -> Result<(RuntimeMode, Vec<String>), CliError> {
    let mut runtime = RuntimeMode::Bare;
    let mut remaining = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--runtime=") {
            runtime = parse_runtime(value)
                .map_err(|message| CliError::new(format!("{message} for `nia emit`"), help))?;
            continue;
        }
        match arg.as_str() {
            "--runtime" => {
                let Some(value) = iter.next() else {
                    return Err(CliError::new("missing runtime after `--runtime`", help));
                };
                runtime = parse_runtime(&value)
                    .map_err(|message| CliError::new(format!("{message} for `nia emit`"), help))?;
            }
            _ => remaining.push(arg),
        }
    }
    Ok((runtime, remaining))
}

fn emit_help_topic(args: &[String]) -> HelpTopic {
    let mut targets = args.iter().filter_map(|arg| emit_target_flag(arg));
    let Some(target) = targets.next() else {
        return HelpTopic::Emit;
    };
    if targets.next().is_some() {
        HelpTopic::Emit
    } else {
        target.help_topic()
    }
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

fn looks_like_emit_target_name(arg: &str) -> bool {
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
    let path = resolve_package_root_path(path)?;
    map.try_insert(name, SourcePath::new(path.to_string_lossy().into_owned()))
}

/// Resolves the user-facing package mapping shorthand.
///
/// A directory is a package only when it owns the conventional `pkg.nia`
/// source root. Explicit source files remain accepted during the package-root
/// migration so installed resources and generated packages can move together.
fn resolve_package_root_path(path: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(path);
    match fs::metadata(candidate) {
        Ok(metadata) if metadata.is_dir() => {
            let package_root = candidate.join("pkg.nia");
            if package_root.is_file() {
                Ok(package_root)
            } else {
                Err(format!(
                    "module map package directory `{path}` does not contain `pkg.nia`"
                ))
            }
        }
        Ok(metadata) if metadata.is_file() => Ok(candidate.to_path_buf()),
        Ok(_) => Err(format!(
            "module map package path `{path}` is neither a file nor a directory"
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Preserve useful loader diagnostics for paths that will be
            // generated before compilation, while applying directory syntax.
            if candidate.extension().is_none() {
                Ok(candidate.join("pkg.nia"))
            } else {
                Ok(candidate.to_path_buf())
            }
        }
        Err(error) => Err(format!(
            "cannot inspect module map package `{path}`: {error}"
        )),
    }
}

fn run_lex(source: &str) -> ExitCode {
    write_stdout(format_args!(
        "{}",
        nia_driver::tokens_inspection(source).text
    ))
}

fn run_parse(path: &str, source: &str, diagnostics_format: DiagnosticsFormat) -> ExitCode {
    let inspection = nia_driver::ast_inspection(source);
    let status = write_stdout(format_args!("{}", inspection.text));
    if status != ExitCode::SUCCESS {
        return status;
    }
    if !inspection.parse_errors.is_empty() {
        write_diagnostic_report(
            &render_parse_failure(path, source, &inspection.parse_errors, diagnostics_format),
            diagnostics_format,
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn render_driver_failure(
    error: &nia_driver::DriverError,
    path: Option<&str>,
    source: Option<&str>,
    format: DiagnosticsFormat,
) -> String {
    match format {
        DiagnosticsFormat::Text => nia_driver::render_driver_error(error, path, source),
        DiagnosticsFormat::Json => nia_driver::render_driver_error_json_at(error, path),
    }
}

fn render_parse_failure(
    path: &str,
    source: &str,
    errors: &[nia_driver::ParseError],
    format: DiagnosticsFormat,
) -> String {
    match format {
        DiagnosticsFormat::Text => nia_driver::render_parse_errors(path, source, errors),
        DiagnosticsFormat::Json => nia_driver::render_parse_errors_json(path, errors),
    }
}

struct CheckRunOptions {
    optimization: NiaOptimizationLevel,
    profile: BuildProfile,
    timings: nia_driver::TimingMode,
    opt_report: bool,
    runtime: RuntimeMode,
    cache_dir: Option<PathBuf>,
    package_root: Option<SourcePath>,
    diagnostics_format: DiagnosticsFormat,
}

struct DriverCheckOptions {
    module_map: ModuleMap,
    package_root: Option<SourcePath>,
    optimization: NiaOptimizationLevel,
    profile: BuildProfile,
    timings: nia_driver::TimingMode,
    runtime: RuntimeSpec,
}

fn run_check(
    path: &str,
    source: &str,
    module_map: ModuleMap,
    options: CheckRunOptions,
    toolchain: Arc<nia_toolchain::ToolchainLayout>,
) -> ExitCode {
    let runtime = match options.runtime.resolve(&toolchain) {
        Ok(runtime) => runtime,
        Err(error) => {
            write_diagnostic_report(
                &render_driver_failure(
                    &nia_driver::DriverError::Runtime(error),
                    Some(path),
                    Some(source),
                    options.diagnostics_format,
                ),
                options.diagnostics_format,
            );
            return ExitCode::FAILURE;
        }
    };
    let driver = nia_driver::Driver::with_config(nia_driver::DriverConfig {
        artifact_cache_dir: options.cache_dir,
        ..nia_driver::DriverConfig::new(toolchain)
    });
    let output = time_summary_stage(options.timings, "check", || {
        driver.check_entry(
            check_request(path, options.package_root.as_ref())
                .with_module_map(module_map.clone())
                .with_optimization(options.optimization)
                .with_profile(options.profile)
                .with_timings(options.timings)
                .with_runtime(runtime.clone()),
        )
    });
    match checked_program_from_output(output, path, source, options.diagnostics_format) {
        Ok(_) => {}
        Err(code) => return code,
    }
    if options.opt_report {
        let output = time_summary_stage(options.timings, "codegen", || {
            codegen_with_driver(
                &driver,
                path,
                DriverCheckOptions {
                    module_map,
                    package_root: options.package_root.clone(),
                    optimization: options.optimization,
                    profile: options.profile,
                    timings: options.timings,
                    runtime,
                },
            )
        });
        let codegen =
            match codegen_program_from_output(output, path, source, options.diagnostics_format) {
                Ok(program) => program,
                Err(code) => return code,
            };
        return print_optimization_report(&codegen);
    }
    ExitCode::SUCCESS
}

fn check_with_driver(
    path: &str,
    options: DriverCheckOptions,
    toolchain: Arc<nia_toolchain::ToolchainLayout>,
) -> nia_driver::DriverOutput<nia_driver::CheckedProgram> {
    nia_driver::Driver::new(toolchain).check_entry(
        check_request(path, options.package_root.as_ref())
            .with_module_map(options.module_map)
            .with_optimization(options.optimization)
            .with_profile(options.profile)
            .with_timings(options.timings)
            .with_runtime(options.runtime),
    )
}

fn check_request(path: &str, package_root: Option<&SourcePath>) -> nia_driver::CheckRequest {
    let mut request = nia_driver::CheckRequest::new(path);
    if let Some(package_root) = package_root {
        request = request.with_package_root(package_root.clone());
    }
    request
}

fn checked_program_from_output(
    output: nia_driver::DriverOutput<nia_driver::CheckedProgram>,
    path: &str,
    source: &str,
    diagnostics_format: DiagnosticsFormat,
) -> Result<nia_driver::CheckedProgram, ExitCode> {
    match output.result {
        Ok(program) => {
            print_check_warnings(&program, path, source, diagnostics_format);
            Ok(program)
        }
        Err(error) => {
            write_diagnostic_report(
                &render_driver_failure(&error, Some(path), Some(source), diagnostics_format),
                diagnostics_format,
            );
            Err(ExitCode::FAILURE)
        }
    }
}

fn codegen_with_driver(
    driver: &nia_driver::Driver,
    path: &str,
    options: DriverCheckOptions,
) -> nia_driver::DriverOutput<nia_driver::CodegenProgram> {
    driver.codegen(
        check_request(path, options.package_root.as_ref())
            .with_module_map(options.module_map)
            .with_optimization(options.optimization)
            .with_profile(options.profile)
            .with_timings(options.timings)
            .with_runtime(options.runtime),
    )
}

fn codegen_program_from_output(
    output: nia_driver::DriverOutput<nia_driver::CodegenProgram>,
    path: &str,
    source: &str,
    diagnostics_format: DiagnosticsFormat,
) -> Result<nia_driver::CodegenProgram, ExitCode> {
    match output.result {
        Ok(program) => {
            print_codegen_warnings(&program, path, source, diagnostics_format);
            Ok(program)
        }
        Err(error) => {
            write_diagnostic_report(
                &render_driver_failure(&error, Some(path), Some(source), diagnostics_format),
                diagnostics_format,
            );
            Err(ExitCode::FAILURE)
        }
    }
}

fn print_check_warnings(
    program: &nia_driver::CheckedProgram,
    path: &str,
    source: &str,
    diagnostics_format: DiagnosticsFormat,
) {
    if program
        .diagnostics
        .iter()
        .any(nia_driver::ProgramDiagnostic::is_warning)
    {
        let rendered = match diagnostics_format {
            DiagnosticsFormat::Text => {
                nia_driver::render_program_warnings(program, Some(path), Some(source))
            }
            DiagnosticsFormat::Json => nia_driver::render_program_warnings_json(program),
        };
        write_diagnostic_report(&rendered, diagnostics_format);
    }
}

fn print_codegen_warnings(
    program: &nia_driver::CodegenProgram,
    path: &str,
    source: &str,
    diagnostics_format: DiagnosticsFormat,
) {
    if program
        .diagnostics
        .iter()
        .any(nia_driver::ProgramDiagnostic::is_warning)
    {
        let rendered = match diagnostics_format {
            DiagnosticsFormat::Text => {
                nia_driver::render_codegen_program_warnings(program, Some(path), Some(source))
            }
            DiagnosticsFormat::Json => nia_driver::render_codegen_program_warnings_json(program),
        };
        write_diagnostic_report(&rendered, diagnostics_format);
    }
}

struct EmitContext {
    module_map: ModuleMap,
    package_root: Option<SourcePath>,
    optimization: NiaOptimizationLevel,
    profile: BuildProfile,
    timings: nia_driver::TimingMode,
    opt_report: bool,
    toolchain: Arc<nia_toolchain::ToolchainLayout>,
    diagnostics_format: DiagnosticsFormat,
}

fn run_emit(path: &str, source: &str, target: EmitTarget, context: EmitContext) -> ExitCode {
    match target {
        EmitTarget::Tokens => time_summary_stage(context.timings, "lex", || run_lex(source)),
        EmitTarget::Ast => time_summary_stage(context.timings, "parse", || {
            run_parse(path, source, context.diagnostics_format)
        }),
        EmitTarget::Checked { runtime } => run_emit_checked(path, source, runtime, context),
        EmitTarget::Backend { runtime } => run_emit_backend(path, source, runtime, context),
        EmitTarget::Llvm { runtime } => run_emit_llvm(path, source, runtime, context),
        EmitTarget::Obj { args } => run_emit_obj(path, source, args, context),
        EmitTarget::Exe { args } => run_emit_exe(path, source, args, context),
    }
}

struct BuildContext {
    root: Option<PathBuf>,
    step: Option<String>,
    jobs: Option<NonZeroUsize>,
    link_time_optimization: nia_driver::LinkTimeOptimization,
    dry_run: bool,
    explain: bool,
    print_plan: bool,
    print_cache: bool,
    print_observations: bool,
    quiet: u8,
    verbose: u8,
    progress: ProgressMode,
    optimization: NiaOptimizationLevel,
    profile: BuildProfile,
    timings: nia_driver::TimingMode,
    timing_format: TimingFormat,
    diagnostics_format: DiagnosticsFormat,
    toolchain: Arc<nia_toolchain::ToolchainLayout>,
}

fn run_build(context: BuildContext) -> ExitCode {
    let BuildContext {
        root,
        step,
        jobs,
        link_time_optimization,
        dry_run,
        explain,
        print_plan,
        print_cache,
        print_observations,
        quiet,
        verbose,
        progress,
        optimization,
        profile,
        timings,
        timing_format,
        diagnostics_format,
        toolchain,
    } = context;
    let machine_output =
        diagnostics_format == DiagnosticsFormat::Json || progress == ProgressMode::Json;
    let mut request = nia_build::BuildRequest::new(toolchain).with_forward_output(!machine_output);
    if let Some(output) = progress_observer(progress, quiet, verbose, diagnostics_format) {
        request = request.with_progress_observer(output.observer);
        if let Some(writer) = output.process_output_writer {
            request = request.with_process_output_writer(writer);
        }
    }
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
        .with_profile(profile)
        .with_link_time_optimization(link_time_optimization)
        .with_dry_run(dry_run)
        .with_explain(explain)
        .with_print_plan(print_plan)
        .with_print_cache(print_cache)
        .with_print_observations(print_observations)
        .with_output_json(machine_output)
        .with_timings(timings)
        .with_timing_format(timing_format);
    match nia_build::run_build(request) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let rendered = match diagnostics_format {
                DiagnosticsFormat::Text => nia_build::render_build_error(&error, None, None),
                DiagnosticsFormat::Json => nia_build::render_build_error_json(&error),
            };
            write_diagnostic_report(&rendered, diagnostics_format);
            ExitCode::FAILURE
        }
    }
}

fn resolve_output_policy(cli: &mut Cli) -> Result<(), CliError> {
    let (progress, help) = match &cli.command {
        CliCommand::Build { progress, .. } => (*progress, HelpTopic::Build),
        CliCommand::Test { progress, .. } => (*progress, HelpTopic::Test),
        _ => return Ok(()),
    };
    match progress {
        ProgressMode::Json => {
            if cli.diagnostics_format_explicit && cli.diagnostics_format == DiagnosticsFormat::Text
            {
                return Err(CliError::new(
                    "`--progress=json` requires `--diagnostics-format=json` to keep stderr machine-readable",
                    help,
                ));
            }
            if !cli.diagnostics_format_explicit {
                cli.diagnostics_format = DiagnosticsFormat::Json;
            }
            if cli.timings != nia_driver::TimingMode::Off {
                if cli.timing_format_explicit && cli.timing_format == TimingFormat::Text {
                    return Err(CliError::new(
                        "`--progress=json` requires `--timings-format=json` when timings are enabled",
                        help,
                    ));
                }
                if !cli.timing_format_explicit {
                    cli.timing_format = TimingFormat::Json;
                }
            }
        }
        ProgressMode::Plain
            if cli.diagnostics_format_explicit
                && cli.diagnostics_format == DiagnosticsFormat::Json =>
        {
            return Err(CliError::new(
                "`--progress=plain` cannot be combined with `--diagnostics-format=json`",
                help,
            ));
        }
        _ => {}
    }
    Ok(())
}

struct TestContext {
    root: Option<PathBuf>,
    filter: Option<String>,
    list: bool,
    fail_fast: bool,
    jobs: Option<NonZeroUsize>,
    quiet: u8,
    verbose: u8,
    progress: ProgressMode,
    optimization: NiaOptimizationLevel,
    profile: BuildProfile,
    timings: nia_driver::TimingMode,
    timing_format: TimingFormat,
    diagnostics_format: DiagnosticsFormat,
    toolchain: Arc<nia_toolchain::ToolchainLayout>,
}

#[derive(Default, Clone)]
struct PlainProgressState {
    total: usize,
    selection_ready: bool,
    execution_started: bool,
    finished: usize,
    flushed: usize,
    cached: usize,
    executed: usize,
    no_work: usize,
    cancelled: usize,
    failed: usize,
    started: Option<Instant>,
    order: Vec<nia_build::ActionKey>,
    ordinals: std::collections::HashMap<nia_build::ActionKey, usize>,
    descriptions: BTreeMap<nia_build::ActionKey, nia_build::ProgressActionDescription>,
    pending: std::collections::BTreeMap<
        nia_build::ActionKey,
        (
            nia_build::ProgressOutcome,
            Option<nia_build::ActionCacheOutcome>,
            Duration,
        ),
    >,
    active: BTreeMap<nia_build::ActionKey, ActiveProgressAction>,
    queued: BTreeMap<nia_build::ActionKey, nia_build::ActionResourceClass>,
    waits: BTreeMap<nia_build::ActionKey, (nia_build::ProgressWaitKind, Instant)>,
    activities: BTreeMap<nia_build::ActionKey, nia_build::ProgressActivity>,
    current_phase: Option<(nia_build::ProgressPhase, Instant)>,
    phase_wait: Option<(nia_build::ProgressPhase, Instant)>,
    wave: usize,
    resource_generation: u64,
    resource_capacity: usize,
    resource_used: usize,
    recent_completion: Option<String>,
    build_complete: bool,
}

#[derive(Clone)]
struct ActiveProgressAction {
    resource_class: nia_build::ActionResourceClass,
    started: Instant,
    wave: usize,
    scheduler_wait: Duration,
    resource_wait: Duration,
}

struct PlainProgress {
    state: Arc<Mutex<PlainProgressState>>,
    verbose: u8,
    screen: Option<Arc<InteractiveScreen>>,
    refresh_worker: Mutex<Option<JoinHandle<()>>>,
    output_failed: std::sync::atomic::AtomicBool,
    plain_gate: Arc<Mutex<()>>,
}

impl PlainProgress {
    fn new(verbose: u8, interactive: bool) -> Arc<Self> {
        let state = Arc::new(Mutex::new(PlainProgressState::default()));
        let screen = interactive.then(|| {
            Arc::new(InteractiveScreen {
                state: Arc::clone(&state),
                terminal: Mutex::new(TerminalState {
                    enabled: true,
                    ..TerminalState::default()
                }),
                wake: Condvar::new(),
                verbose,
            })
        });
        let progress = Arc::new(Self {
            state,
            verbose,
            screen,
            refresh_worker: Mutex::new(None),
            output_failed: std::sync::atomic::AtomicBool::new(false),
            plain_gate: Arc::new(Mutex::new(())),
        });
        if let Some(screen) = &progress.screen
            && let Ok(worker) = thread::Builder::new()
                .name("nia-progress-renderer".into())
                .spawn({
                    let screen = Arc::clone(screen);
                    move || interactive_refresh_loop(screen)
                })
            && let Ok(mut slot) = progress.refresh_worker.lock()
        {
            *slot = Some(worker);
        }
        progress
    }

    fn write_plain(&self, lines: impl IntoIterator<Item = String>) {
        if self
            .output_failed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        let _gate = self.plain_gate.lock().ok();
        let mut stderr = io::stderr().lock();
        for line in lines {
            if writeln!(stderr, "{line}").is_err() {
                self.output_failed
                    .store(true, std::sync::atomic::Ordering::Release);
                return;
            }
        }
        if stderr.flush().is_err() {
            self.output_failed
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }

    fn output_records(&self, lines: Vec<String>) {
        if lines.is_empty() {
            return;
        }
        if let Some(screen) = &self.screen {
            screen.write_records(&lines);
        } else {
            self.write_plain(lines);
        }
    }

    fn finish_output(&self, summary: String) {
        if let Some(screen) = &self.screen {
            screen.finish(&summary);
        } else {
            self.write_plain([summary]);
        }
    }
}

impl Drop for PlainProgress {
    fn drop(&mut self) {
        if let Some(screen) = &self.screen
            && let Ok(mut terminal) = screen.terminal.lock()
        {
            terminal.stopped = true;
            screen.wake.notify_all();
        }
        if let Ok(worker) = self.refresh_worker.get_mut()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}

#[derive(Default)]
struct TerminalState {
    enabled: bool,
    stopped: bool,
    drawn_rows: usize,
    partial_streams: BTreeMap<u64, bool>,
    next_stream_id: u64,
    broken_line: bool,
    last_draw: Option<Instant>,
}

struct InteractiveScreen {
    state: Arc<Mutex<PlainProgressState>>,
    terminal: Mutex<TerminalState>,
    wake: Condvar,
    verbose: u8,
}

impl InteractiveScreen {
    fn notify(&self) {
        self.wake.notify_all();
    }

    fn write_records(&self, lines: &[String]) {
        let Ok(mut terminal) = self.terminal.lock() else {
            return;
        };
        if terminal.enabled && clear_progress_frame(&mut terminal).is_err() {
            terminal.enabled = false;
        }
        let mut stderr = io::stderr().lock();
        for line in lines {
            if writeln!(stderr, "{line}").is_err() {
                terminal.enabled = false;
                return;
            }
        }
        let _ = stderr.flush();
        self.wake.notify_all();
    }

    fn finish(&self, summary: &str) {
        let Ok(mut terminal) = self.terminal.lock() else {
            return;
        };
        terminal.stopped = true;
        if terminal.enabled && clear_progress_frame(&mut terminal).is_err() {
            terminal.enabled = false;
        }
        let mut stderr = io::stderr().lock();
        if terminal.broken_line {
            let _ = stderr.write_all(b"\r\n");
        }
        if writeln!(stderr, "{summary}").is_err() {
            terminal.enabled = false;
        }
        let _ = stderr.flush();
        self.wake.notify_all();
    }

    fn open_stream(
        self: &Arc<Self>,
        stream: nia_build::ProcessOutputStream,
    ) -> Box<dyn Write + Send> {
        let stream_id = if let Ok(mut terminal) = self.terminal.lock() {
            let stream_id = terminal.next_stream_id;
            terminal.next_stream_id = terminal.next_stream_id.wrapping_add(1);
            terminal.partial_streams.insert(stream_id, false);
            stream_id
        } else {
            0
        };
        Box::new(CoordinatedChildOutput {
            screen: Arc::clone(self),
            stream,
            stream_id,
            finished: false,
        })
    }
}

struct CoordinatedChildOutput {
    screen: Arc<InteractiveScreen>,
    stream: nia_build::ProcessOutputStream,
    stream_id: u64,
    finished: bool,
}

impl Write for CoordinatedChildOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut terminal = self
            .screen
            .terminal
            .lock()
            .map_err(|_| io::Error::other("progress terminal lock is poisoned"))?;
        if terminal.enabled && clear_progress_frame(&mut terminal).is_err() {
            // A terminal renderer failure disables only the frame; child
            // bytes still follow the existing forwarding/error policy.
            terminal.enabled = false;
        }
        let result = write_child_bytes(self.stream, bytes);
        if let Ok(written) = result {
            if written > 0 {
                let partial = !matches!(bytes[written - 1], b'\n' | b'\r');
                if let Some(current) = terminal.partial_streams.get_mut(&self.stream_id) {
                    *current = partial;
                }
            }
            self.screen.wake.notify_all();
            Ok(written)
        } else {
            terminal.enabled = false;
            result
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.stream {
            nia_build::ProcessOutputStream::Stdout => io::stdout().flush(),
            nia_build::ProcessOutputStream::Stderr => io::stderr().flush(),
        }
    }
}

impl Drop for CoordinatedChildOutput {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if let Ok(mut terminal) = self.screen.terminal.lock()
            && let Some(partial) = terminal.partial_streams.remove(&self.stream_id)
            && partial
        {
            terminal.broken_line = true;
        }
        self.screen.wake.notify_all();
        self.finished = true;
    }
}

impl nia_build::ProcessOutputWriter for PlainProgress {
    fn open_stream(&self, stream: nia_build::ProcessOutputStream) -> Box<dyn Write + Send> {
        match &self.screen {
            Some(screen) => screen.open_stream(stream),
            None => Box::new(AppendChildOutput {
                gate: Arc::clone(&self.plain_gate),
                stream,
            }),
        }
    }
}

struct AppendChildOutput {
    gate: Arc<Mutex<()>>,
    stream: nia_build::ProcessOutputStream,
}

impl Write for AppendChildOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let _gate = self
            .gate
            .lock()
            .map_err(|_| io::Error::other("progress output lock is poisoned"))?;
        write_child_bytes(self.stream, bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        let _gate = self
            .gate
            .lock()
            .map_err(|_| io::Error::other("progress output lock is poisoned"))?;
        match self.stream {
            nia_build::ProcessOutputStream::Stdout => io::stdout().flush(),
            nia_build::ProcessOutputStream::Stderr => io::stderr().flush(),
        }
    }
}

impl nia_build::ProgressObserver for PlainProgress {
    fn observe(&self, event: nia_build::ProgressEvent) {
        use nia_build::{ProgressEventKind, ProgressOutcome};
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let build_failed = matches!(&event.kind, ProgressEventKind::BuildFailed);
        let mut lines = Vec::new();
        let mut summary = None;
        let occurred_at = state
            .started
            .and_then(|start| start.checked_add(event.elapsed))
            .unwrap_or_else(Instant::now);
        match event.kind {
            ProgressEventKind::BuildStarted => {
                state.started = Instant::now().checked_sub(event.elapsed)
            }
            ProgressEventKind::SelectionReady { total, actions } => {
                state.selection_ready = true;
                state.total = total;
                state.ordinals = actions
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, action)| (action, index + 1))
                    .collect();
                state.order = actions;
            }
            ProgressEventKind::ActionDescribed {
                action,
                description,
            } => {
                if self.verbose >= 3 && self.screen.is_none() {
                    lines.push(format!(
                        "Details {}: {} inputs [{}], {} outputs [{}] ({})",
                        action_progress_label(&action),
                        description.total_inputs,
                        description
                            .inputs
                            .iter()
                            .map(progress_logical_path_label)
                            .collect::<Vec<_>>()
                            .join(", "),
                        description.total_outputs,
                        description
                            .outputs
                            .iter()
                            .map(progress_logical_path_label)
                            .collect::<Vec<_>>()
                            .join(", "),
                        description.kind
                    ));
                }
                state.descriptions.insert(action, description);
            }
            ProgressEventKind::ActionCacheDecision { .. } => {}
            ProgressEventKind::ActionFinished {
                action,
                outcome,
                cache,
                duration,
            } => {
                state.active.remove(&action);
                state.queued.remove(&action);
                state.waits.remove(&action);
                state.activities.remove(&action);
                state.finished += 1;
                match outcome {
                    ProgressOutcome::Cached => state.cached += 1,
                    ProgressOutcome::Executed => state.executed += 1,
                    ProgressOutcome::NoWork => state.no_work += 1,
                    ProgressOutcome::Failed => state.failed += 1,
                    ProgressOutcome::Cancelled => state.cancelled += 1,
                }
                if self.verbose > 0 {
                    state.pending.insert(action, (outcome, cache, duration));
                    while state.flushed < state.order.len() {
                        let Some(action) = state.order.get(state.flushed).cloned() else {
                            break;
                        };
                        let Some((outcome, cache, duration)) = state.pending.remove(&action) else {
                            break;
                        };
                        let ordinal = state.ordinals.get(&action).copied().unwrap_or(0);
                        let label = action_progress_label(&action);
                        let detail = if self.verbose >= 3 {
                            cache
                                .as_ref()
                                .map(|cache| format!("; {}", cache_progress_detail(cache)))
                                .unwrap_or_default()
                        } else {
                            String::new()
                        };
                        let line = format!(
                            "[{ordinal}/{}] {} {} ({}, {}{detail})",
                            state.total,
                            progress_outcome_name(&outcome),
                            label,
                            state
                                .descriptions
                                .get(&action)
                                .map_or("action", |description| description.kind),
                            format_duration(duration)
                        );
                        state.recent_completion = Some(line.clone());
                        lines.push(line);
                        state.flushed += 1;
                    }
                } else {
                    state.recent_completion = Some(format!(
                        "{} {} ({})",
                        progress_outcome_name(&outcome),
                        action_progress_label(&action),
                        format_duration(duration)
                    ));
                }
            }
            ProgressEventKind::BuildFinished | ProgressEventKind::BuildFailed => {
                state.build_complete = true;
                let elapsed = event.elapsed;
                summary = Some(format!(
                    "{} {} actions in {} ({} cached, {} executed, {} no-work{}{})",
                    if build_failed {
                        "Failed after"
                    } else {
                        "Finished"
                    },
                    state.total,
                    format_duration(elapsed),
                    state.cached,
                    state.executed,
                    state.no_work,
                    if state.failed == 0 {
                        String::new()
                    } else {
                        format!(", {} failed", state.failed)
                    },
                    if state.cancelled == 0 {
                        String::new()
                    } else {
                        format!(", {} cancelled", state.cancelled)
                    }
                ));
                if !state.selection_ready {
                    summary = Some(format!(
                        "{} build in {}",
                        if build_failed { "Failed" } else { "Finished" },
                        format_duration(elapsed)
                    ));
                } else if !state.execution_started && !build_failed {
                    summary = Some(format!(
                        "Configured build in {} ({} selected actions)",
                        format_duration(elapsed),
                        state.total
                    ));
                }
            }
            ProgressEventKind::PhaseStarted { phase } => {
                if phase == nia_build::ProgressPhase::Execute {
                    state.execution_started = true;
                }
                state.current_phase = Some((phase, occurred_at));
                if self.verbose == 0
                    && self.screen.is_none()
                    && matches!(
                        phase,
                        nia_build::ProgressPhase::Resolve
                            | nia_build::ProgressPhase::Runner
                            | nia_build::ProgressPhase::Configure
                            | nia_build::ProgressPhase::Execute
                    )
                {
                    lines.push(phase_name(phase).to_string());
                }
            }
            ProgressEventKind::PhaseFinished { phase, .. } => {
                if state
                    .current_phase
                    .is_some_and(|(active, _)| active == phase)
                {
                    state.current_phase = None;
                }
            }
            ProgressEventKind::PhaseWaitStarted { phase } => {
                state.phase_wait = Some((phase, occurred_at));
                if self.verbose >= 2 && self.screen.is_none() {
                    lines.push(format!("Waiting for cache lock ({})", phase_name(phase)));
                }
            }
            ProgressEventKind::PhaseCacheDecision { phase, reused } => {
                if self.verbose >= 1 && self.screen.is_none() {
                    lines.push(format!(
                        "{}: {}",
                        phase_name(phase),
                        if reused { "reused" } else { "cache miss" }
                    ));
                }
            }
            ProgressEventKind::PhaseWaitFinished {
                phase,
                outcome,
                duration,
            } => {
                state.phase_wait = None;
                if self.verbose >= 2 && self.screen.is_none() {
                    lines.push(format!(
                        "Cache lock {:?} ({}, {})",
                        outcome,
                        phase_name(phase),
                        format_duration(duration)
                    ));
                }
            }
            ProgressEventKind::WaveStarted { wave, actions } => {
                state.wave = wave;
                if self.verbose >= 2 && self.screen.is_none() {
                    lines.push(format!("Wave {}: {} actions ready", wave + 1, actions));
                }
            }
            ProgressEventKind::ActionStarted {
                action,
                resource_class,
                wave,
                scheduler_wait,
                resource_wait,
            } => {
                state.queued.remove(&action);
                state.active.insert(
                    action.clone(),
                    ActiveProgressAction {
                        resource_class,
                        started: occurred_at,
                        wave,
                        scheduler_wait,
                        resource_wait,
                    },
                );
                if self.verbose >= 2 && self.screen.is_none() {
                    lines.push(format!(
                        "Running {} ({}, wave {}, queue {}, permit {})",
                        action_progress_label(&action),
                        resource_class_name(resource_class),
                        wave + 1,
                        format_duration(scheduler_wait),
                        format_duration(resource_wait)
                    ));
                }
            }
            ProgressEventKind::ActionWaitStarted { action, kind } => {
                state.waits.insert(action.clone(), (kind, occurred_at));
                if self.verbose >= 2 && self.screen.is_none() {
                    lines.push(format!(
                        "Waiting for {} ({})",
                        action_progress_label(&action),
                        progress_wait_kind_name(kind)
                    ));
                }
            }
            ProgressEventKind::ActionQueued {
                action,
                resource_class,
                ..
            } => {
                state.queued.insert(action, resource_class);
            }
            ProgressEventKind::ActionWaitFinished {
                action,
                kind,
                outcome,
                duration,
            } => {
                state.waits.remove(&action);
                if self.verbose >= 2 && self.screen.is_none() {
                    lines.push(format!(
                        "Wait {:?} {} ({}, {})",
                        outcome,
                        action_progress_label(&action),
                        progress_wait_kind_name(kind),
                        format_duration(duration)
                    ));
                }
            }
            ProgressEventKind::ActionActivityStarted { action, activity } => {
                state.activities.insert(action.clone(), activity);
                if self.verbose >= 3 && self.screen.is_none() {
                    lines.push(format!(
                        "Activity {} ({})",
                        action_progress_label(&action),
                        progress_activity_name(activity)
                    ));
                }
            }
            ProgressEventKind::ActionActivityFinished { action, .. } => {
                state.activities.remove(&action);
            }
            ProgressEventKind::ResourceUsageChanged {
                generation,
                capacity,
                used,
            } => {
                if generation >= state.resource_generation {
                    state.resource_generation = generation;
                    state.resource_capacity = capacity;
                    state.resource_used = used;
                }
                if self.verbose >= 2 && self.screen.is_none() {
                    lines.push(format!("Resources: {used}/{capacity} action permits"));
                }
            }
        }
        drop(state);
        if let Some(screen) = &self.screen {
            screen.notify();
        }
        self.output_records(lines);
        if let Some(summary) = summary {
            self.finish_output(summary);
        }
    }
}

fn interactive_refresh_loop(screen: Arc<InteractiveScreen>) {
    loop {
        let Ok(mut terminal_state) = screen.terminal.lock() else {
            return;
        };
        if terminal_state.stopped {
            return;
        }
        let (next_state, _) = match screen
            .wake
            .wait_timeout(terminal_state, Duration::from_millis(100))
        {
            Ok(result) => result,
            Err(_) => return,
        };
        terminal_state = next_state;
        if terminal_state.stopped {
            return;
        }
        let state = match screen.state.lock() {
            Ok(state) => state.clone(),
            Err(_) => return,
        };
        if state.build_complete
            || terminal_state.broken_line
            || !terminal_state.partial_streams.is_empty()
            || !terminal_state.enabled
        {
            continue;
        }
        let Some(started) = state.started else {
            continue;
        };
        let now = Instant::now();
        if now.duration_since(started) < Duration::from_millis(250)
            || terminal_state
                .last_draw
                .is_some_and(|last| now.duration_since(last) < Duration::from_millis(100))
        {
            continue;
        }
        let Ok((width, height)) = terminal::size() else {
            terminal_state.enabled = false;
            continue;
        };
        let lines = progress_frame_lines(&state, screen.verbose, now, width, height);
        if lines.is_empty() {
            continue;
        }
        if clear_progress_frame(&mut terminal_state).is_err() {
            terminal_state.enabled = false;
            continue;
        }
        let mut stderr = io::stderr().lock();
        let rendered = lines.iter().enumerate().try_for_each(|(index, line)| {
            if index > 0 {
                execute!(stderr, Print("\r\n"))?;
            }
            execute!(stderr, Print(line))
        });
        if rendered.is_err() || stderr.flush().is_err() {
            terminal_state.enabled = false;
            terminal_state.drawn_rows = 0;
        } else {
            terminal_state.drawn_rows = lines.len();
            terminal_state.last_draw = Some(now);
        }
    }
}

fn clear_progress_frame(terminal_state: &mut TerminalState) -> io::Result<()> {
    if terminal_state.drawn_rows == 0 {
        return Ok(());
    }
    let mut stderr = io::stderr().lock();
    for row in (0..terminal_state.drawn_rows).rev() {
        execute!(stderr, MoveToColumn(0), Clear(ClearType::CurrentLine))?;
        if row > 0 {
            execute!(stderr, MoveUp(1))?;
        }
    }
    stderr.flush()?;
    terminal_state.drawn_rows = 0;
    Ok(())
}

fn write_child_bytes(stream: nia_build::ProcessOutputStream, bytes: &[u8]) -> io::Result<usize> {
    match stream {
        nia_build::ProcessOutputStream::Stdout => io::stdout().lock().write_all(bytes)?,
        nia_build::ProcessOutputStream::Stderr => io::stderr().lock().write_all(bytes)?,
    }
    Ok(bytes.len())
}

fn progress_frame_lines(
    state: &PlainProgressState,
    verbose: u8,
    now: Instant,
    width: u16,
    height: u16,
) -> Vec<String> {
    let elapsed = state
        .started
        .map_or(Duration::ZERO, |started| now.duration_since(started));
    if let Some((phase, started)) = state.current_phase
        && phase != nia_build::ProgressPhase::Execute
    {
        let waiting = if state.phase_wait.is_some() {
            " (cache lock)"
        } else {
            ""
        };
        return vec![truncate_progress_cells(
            &format!(
                "{}{} {}",
                phase_name(phase),
                waiting,
                format_duration(now.duration_since(started))
            ),
            width.saturating_sub(1) as usize,
        )];
    }
    if verbose < 2 {
        let (label, phase_elapsed) = if let Some(action) = state
            .order
            .iter()
            .find(|action| state.active.contains_key(*action))
        {
            (action_progress_label(action), elapsed)
        } else {
            ("Preparing build".to_string(), elapsed)
        };
        return vec![truncate_progress_cells(
            &format!(
                "Building [{}/{} actions, {} active] {} {}",
                state.finished,
                state.total,
                state.active.len(),
                label,
                format_duration(phase_elapsed)
            ),
            width.saturating_sub(1) as usize,
        )];
    }

    let mut lines = vec![truncate_progress_cells(
        &format!(
            "Building [{}/{} actions] wave {}   elapsed {}",
            state.finished,
            state.total,
            state.wave + 1,
            format_duration(elapsed)
        ),
        width.saturating_sub(1) as usize,
    )];
    lines.push(truncate_progress_cells(
        &format!(
            "{} cached  {} executed  {} failed  {} cancelled",
            state.cached, state.executed, state.failed, state.cancelled
        ),
        width.saturating_sub(1) as usize,
    ));
    lines.push(truncate_progress_cells(
        &format!(
            "action permits {}/{} reserved; {} active; {} queued; {} dependency-blocked",
            state.resource_used,
            state.resource_capacity,
            state.active.len(),
            state.queued.len(),
            state
                .waits
                .values()
                .filter(|(kind, _)| *kind == nia_build::ProgressWaitKind::Dependency)
                .count()
        ),
        width.saturating_sub(1) as usize,
    ));
    let max_rows = usize::from(height).saturating_sub(1).min(8);
    let pending = state
        .order
        .iter()
        .filter(|action| {
            state.active.contains_key(*action)
                || state.queued.contains_key(*action)
                || state.waits.contains_key(*action)
        })
        .count();
    let row_limit = if pending > max_rows.saturating_sub(lines.len()) {
        max_rows.saturating_sub(1)
    } else {
        max_rows
    };
    let mut visible = 0usize;
    for action in &state.order {
        let Some(active) = state.active.get(action) else {
            if (state.queued.contains_key(action) || state.waits.contains_key(action))
                && lines.len() < row_limit
            {
                let label = state.waits.get(action).map_or_else(
                    || "queued".to_string(),
                    |(kind, started)| {
                        format!(
                            "{} {}",
                            progress_wait_kind_name(*kind),
                            format_duration(now.saturating_duration_since(*started))
                        )
                    },
                );
                lines.push(truncate_progress_cells(
                    &format!("  {} {}", action_progress_label(action), label),
                    width.saturating_sub(1) as usize,
                ));
                visible += 1;
            }
            continue;
        };
        if lines.len() >= row_limit {
            break;
        }
        let wave = active.wave + 1;
        let active_duration = format_duration(now.duration_since(active.started));
        let queue_wait = format_duration(active.scheduler_wait);
        let permit_wait = format_duration(active.resource_wait);
        let wait = state
            .waits
            .get(action)
            .map(|(kind, started)| {
                format!(
                    " wait:{} {}",
                    progress_wait_kind_name(*kind),
                    format_duration(now.duration_since(*started))
                )
            })
            .unwrap_or_default();
        let activity = state
            .activities
            .get(action)
            .map(|activity| format!(" activity:{}", progress_activity_name(*activity)))
            .unwrap_or_default();
        lines.push(truncate_progress_cells(
            &format!(
                "  {:<12} {}  wave {}  {}  q:{} p:{}{wait}{activity}",
                resource_class_name(active.resource_class),
                action_progress_label(action),
                wave,
                active_duration,
                queue_wait,
                permit_wait
            ),
            width.saturating_sub(1) as usize,
        ));
        visible += 1;
    }
    let pending = state
        .order
        .iter()
        .filter(|action| {
            state.active.contains_key(*action)
                || state.queued.contains_key(*action)
                || state.waits.contains_key(*action)
        })
        .count();
    let hidden = pending.saturating_sub(visible);
    if hidden > 0 && lines.len() < max_rows {
        lines.push(format!("  +{hidden} actions"));
    }
    if verbose >= 3
        && let Some(recent) = &state.recent_completion
        && lines.len() < max_rows
    {
        lines.push(truncate_progress_cells(
            recent,
            width.saturating_sub(1) as usize,
        ));
    }
    lines.truncate(max_rows.max(1));
    lines
}

fn truncate_progress_cells(text: &str, max_width: usize) -> String {
    if unicode_width::UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    let suffix = "...";
    let target = max_width.saturating_sub(suffix.len());
    let mut output = String::new();
    let mut width = 0usize;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width + character_width > target {
            break;
        }
        output.push(character);
        width += character_width;
    }
    output.push_str(&suffix[..suffix.len().min(max_width)]);
    output
}

fn action_progress_label(action: &nia_build::ActionKey) -> String {
    format!(
        "{}:{}",
        escape_progress_label(action.package().as_str()),
        escape_progress_label(action.name())
    )
}

fn progress_outcome_name(outcome: &nia_build::ProgressOutcome) -> &'static str {
    match outcome {
        nia_build::ProgressOutcome::Cached => "Cached",
        nia_build::ProgressOutcome::Executed => "Executed",
        nia_build::ProgressOutcome::NoWork => "Complete",
        nia_build::ProgressOutcome::Failed => "Failed",
        nia_build::ProgressOutcome::Cancelled => "Cancelled",
    }
}

fn cache_progress_detail(cache: &nia_build::ActionCacheOutcome) -> &'static str {
    match cache {
        nia_build::ActionCacheOutcome::Hit => "cache hit",
        nia_build::ActionCacheOutcome::Miss(nia_build::ActionCacheMissReason::NotFound) => {
            "cache miss: not found"
        }
        nia_build::ActionCacheOutcome::Miss(nia_build::ActionCacheMissReason::Invalidated(_)) => {
            "cache miss: invalidated"
        }
        nia_build::ActionCacheOutcome::Miss(nia_build::ActionCacheMissReason::Uncacheable) => {
            "cache miss: uncacheable"
        }
        nia_build::ActionCacheOutcome::Miss(nia_build::ActionCacheMissReason::Corrupt) => {
            "cache miss: corrupt"
        }
        nia_build::ActionCacheOutcome::Miss(nia_build::ActionCacheMissReason::ReadError) => {
            "cache miss: read error"
        }
        nia_build::ActionCacheOutcome::Miss(nia_build::ActionCacheMissReason::WriteError) => {
            "cache miss: write error"
        }
    }
}

fn resource_class_name(class: nia_build::ActionResourceClass) -> &'static str {
    match class {
        nia_build::ActionResourceClass::Cpu => "cpu",
        nia_build::ActionResourceClass::Io => "io",
        nia_build::ActionResourceClass::Conservative => "conservative",
    }
}

fn progress_wait_kind_name(kind: nia_build::ProgressWaitKind) -> &'static str {
    match kind {
        nia_build::ProgressWaitKind::Dependency => "dependency",
        nia_build::ProgressWaitKind::ResourcePermit => "resource permit",
        nia_build::ProgressWaitKind::OutputLock => "output lock",
        nia_build::ProgressWaitKind::CacheLock => "cache lock",
    }
}

fn progress_activity_name(activity: nia_build::ProgressActivity) -> &'static str {
    match activity {
        nia_build::ProgressActivity::Checking => "checking",
        nia_build::ProgressActivity::Codegen => "codegen",
        nia_build::ProgressActivity::Linking => "linking",
        nia_build::ProgressActivity::CompilingAndLinking => "compiling and linking",
    }
}

fn interactive_terminal_available() -> bool {
    let term = env::var("TERM").unwrap_or_default();
    io::stderr().is_terminal()
        && !term.eq_ignore_ascii_case("dumb")
        && env::var_os("CI").is_none()
        && terminal::size().is_ok()
}

struct ProgressOutput {
    observer: Arc<dyn nia_build::ProgressObserver>,
    process_output_writer: Option<Arc<dyn nia_build::ProcessOutputWriter>>,
}

#[derive(Default)]
struct JsonProgressState {
    started: Option<Instant>,
    total: usize,
    selection_ready: bool,
    completed: usize,
    ordinals: std::collections::HashMap<nia_build::ActionKey, usize>,
    descriptions: BTreeMap<nia_build::ActionKey, nia_build::ProgressActionDescription>,
}

struct JsonProgress {
    state: Mutex<JsonProgressState>,
    verbosity: u8,
    output_failed: std::sync::atomic::AtomicBool,
}

impl nia_build::ProgressObserver for JsonProgress {
    fn observe(&self, event: nia_build::ProgressEvent) {
        use nia_build::ProgressEventKind;
        if self
            .output_failed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let elapsed = event.elapsed.as_millis() as u64;
        let mut record = serde_json::json!({
            "kind": "build-progress",
            "schema_version": 1,
            "sequence": event.sequence,
            "elapsed_ms": elapsed,
        });
        let object = record.as_object_mut().expect("progress record object");
        let action = match &event.kind {
            ProgressEventKind::ActionDescribed { action, .. }
            | ProgressEventKind::ActionCacheDecision { action, .. }
            | ProgressEventKind::ActionQueued { action, .. }
            | ProgressEventKind::ActionStarted { action, .. }
            | ProgressEventKind::ActionWaitStarted { action, .. }
            | ProgressEventKind::ActionWaitFinished { action, .. }
            | ProgressEventKind::ActionActivityStarted { action, .. }
            | ProgressEventKind::ActionActivityFinished { action, .. }
            | ProgressEventKind::ActionFinished { action, .. } => Some(action),
            _ => None,
        };
        if let Some(action) = action
            && let Some(ordinal) = state.ordinals.get(action)
        {
            object.insert("ordinal".into(), (*ordinal).into());
        }
        match event.kind {
            ProgressEventKind::BuildStarted => {
                state.started = Some(Instant::now());
                object.insert("elapsed_ms".into(), 0.into());
                object.insert("event".into(), "build-started".into());
            }
            ProgressEventKind::BuildFinished => {
                object.insert("event".into(), "build-finished".into());
                if state.selection_ready {
                    object.insert("completed".into(), state.completed.into());
                    object.insert("total".into(), state.total.into());
                }
            }
            ProgressEventKind::BuildFailed => {
                object.insert("event".into(), "build-failed".into());
                if state.selection_ready {
                    object.insert("completed".into(), state.completed.into());
                    object.insert("total".into(), state.total.into());
                }
            }
            ProgressEventKind::PhaseStarted { phase } => {
                object.insert("event".into(), "phase-started".into());
                object.insert("phase".into(), format!("{phase:?}").to_lowercase().into());
            }
            ProgressEventKind::PhaseFinished {
                phase,
                duration,
                success,
            } => {
                object.insert("event".into(), "phase-finished".into());
                object.insert("phase".into(), format!("{phase:?}").to_lowercase().into());
                object.insert("duration_ms".into(), (duration.as_millis() as u64).into());
                object.insert("success".into(), success.into());
            }
            ProgressEventKind::PhaseWaitStarted { phase } if self.verbosity >= 2 => {
                object.insert("event".into(), "phase-wait-started".into());
                object.insert("phase".into(), format!("{phase:?}").to_lowercase().into());
                object.insert("wait".into(), "cache lock".into());
            }
            ProgressEventKind::PhaseCacheDecision { phase, reused } if self.verbosity >= 1 => {
                object.insert("event".into(), "phase-cache-decision".into());
                object.insert("phase".into(), format!("{phase:?}").to_lowercase().into());
                object.insert("reused".into(), reused.into());
            }
            ProgressEventKind::PhaseWaitFinished {
                phase,
                outcome,
                duration,
            } if self.verbosity >= 2 => {
                object.insert("event".into(), "phase-wait-finished".into());
                object.insert("phase".into(), format!("{phase:?}").to_lowercase().into());
                object.insert("wait".into(), "cache lock".into());
                object.insert(
                    "outcome".into(),
                    format!("{outcome:?}").to_lowercase().into(),
                );
                object.insert("duration_ms".into(), (duration.as_millis() as u64).into());
            }
            ProgressEventKind::SelectionReady { total, actions } => {
                state.selection_ready = true;
                state.total = total;
                state.ordinals = actions
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, action)| (action, index + 1))
                    .collect();
                object.insert("event".into(), "selection-ready".into());
                object.insert("total".into(), total.into());
                object.insert(
                    "actions".into(),
                    actions
                        .into_iter()
                        .map(|action| {
                            serde_json::json!({
                                "package": action.package().as_str(),
                                "name": action.name(),
                            })
                        })
                        .collect::<Vec<_>>()
                        .into(),
                );
            }
            ProgressEventKind::ActionDescribed {
                action,
                description,
            } => {
                state
                    .descriptions
                    .insert(action.clone(), description.clone());
                if self.verbosity < 3 {
                    return;
                }
                object.insert("event".into(), "action-described".into());
                object.insert("action".into(), serde_json::json!({"package": action.package().as_str(), "name": action.name()}));
                object.insert("action_kind".into(), description.kind.into());
                object.insert("total_inputs".into(), description.total_inputs.into());
                object.insert("total_outputs".into(), description.total_outputs.into());
                object.insert(
                    "inputs".into(),
                    description
                        .inputs
                        .iter()
                        .map(progress_logical_path_json)
                        .collect::<Vec<_>>()
                        .into(),
                );
                object.insert(
                    "outputs".into(),
                    description
                        .outputs
                        .iter()
                        .map(progress_logical_path_json)
                        .collect::<Vec<_>>()
                        .into(),
                );
            }
            ProgressEventKind::WaveStarted { wave, actions } if self.verbosity >= 2 => {
                object.insert("event".into(), "wave-started".into());
                object.insert("wave".into(), wave.into());
                object.insert("actions".into(), actions.into());
            }
            ProgressEventKind::ActionStarted {
                action,
                resource_class,
                wave,
                scheduler_wait,
                resource_wait,
            } if self.verbosity >= 1 => {
                object.insert("event".into(), "action-started".into());
                object.insert("action".into(), serde_json::json!({"package": action.package().as_str(), "name": action.name()}));
                object.insert(
                    "resource_class".into(),
                    format!("{resource_class:?}").to_lowercase().into(),
                );
                object.insert("wave".into(), wave.into());
                object.insert(
                    "scheduler_wait_ms".into(),
                    (scheduler_wait.as_millis() as u64).into(),
                );
                object.insert(
                    "resource_wait_ms".into(),
                    (resource_wait.as_millis() as u64).into(),
                );
            }
            ProgressEventKind::ActionWaitStarted { action, kind } if self.verbosity >= 2 => {
                object.insert("event".into(), "action-wait-started".into());
                object.insert(
                    "action".into(),
                    serde_json::json!({"package": action.package().as_str(), "name": action.name()}),
                );
                object.insert("wait".into(), progress_wait_kind_name(kind).into());
            }
            ProgressEventKind::ActionQueued {
                action,
                resource_class,
                wave,
            } if self.verbosity >= 2 => {
                object.insert("event".into(), "action-queued".into());
                object.insert("action".into(), serde_json::json!({"package": action.package().as_str(), "name": action.name()}));
                object.insert(
                    "resource_class".into(),
                    resource_class_name(resource_class).into(),
                );
                object.insert("wave".into(), wave.into());
            }
            ProgressEventKind::ActionWaitFinished {
                action,
                kind,
                outcome,
                duration,
            } if self.verbosity >= 2 => {
                object.insert("event".into(), "action-wait-finished".into());
                object.insert(
                    "action".into(),
                    serde_json::json!({"package": action.package().as_str(), "name": action.name()}),
                );
                object.insert("wait".into(), progress_wait_kind_name(kind).into());
                object.insert(
                    "outcome".into(),
                    format!("{outcome:?}").to_lowercase().into(),
                );
                object.insert("duration_ms".into(), (duration.as_millis() as u64).into());
            }
            ProgressEventKind::ActionActivityStarted { action, activity }
                if self.verbosity >= 3 =>
            {
                object.insert("event".into(), "action-activity-started".into());
                object.insert(
                    "action".into(),
                    serde_json::json!({"package": action.package().as_str(), "name": action.name()}),
                );
                object.insert("activity".into(), progress_activity_name(activity).into());
            }
            ProgressEventKind::ActionActivityFinished {
                action,
                activity,
                duration,
                success,
            } if self.verbosity >= 3 => {
                object.insert("event".into(), "action-activity-finished".into());
                object.insert(
                    "action".into(),
                    serde_json::json!({"package": action.package().as_str(), "name": action.name()}),
                );
                object.insert("activity".into(), progress_activity_name(activity).into());
                object.insert("duration_ms".into(), (duration.as_millis() as u64).into());
                object.insert("success".into(), success.into());
            }
            ProgressEventKind::ResourceUsageChanged {
                generation,
                capacity,
                used,
            } if self.verbosity >= 2 => {
                object.insert("event".into(), "resource-usage-changed".into());
                object.insert("generation".into(), generation.into());
                object.insert("capacity".into(), capacity.into());
                object.insert("used".into(), used.into());
            }
            ProgressEventKind::ActionCacheDecision { action, cache } if self.verbosity >= 1 => {
                object.insert("event".into(), "action-cache-decision".into());
                object.insert("action".into(), serde_json::json!({"package": action.package().as_str(), "name": action.name()}));
                object.insert("cache".into(), cache_json_value(cache));
            }
            ProgressEventKind::ActionFinished {
                action,
                outcome,
                cache,
                duration,
            } => {
                object.insert("event".into(), "action-finished".into());
                if let Some(description) = state.descriptions.get(&action) {
                    object.insert("action_kind".into(), description.kind.into());
                }
                state.completed += 1;
                object.insert("action".into(), serde_json::json!({"package": action.package().as_str(), "name": action.name()}));
                object.insert(
                    "ordinal".into(),
                    state
                        .ordinals
                        .get(&action)
                        .copied()
                        .map(serde_json::Value::from)
                        .unwrap_or(serde_json::Value::Null),
                );
                object.insert(
                    "outcome".into(),
                    format!("{outcome:?}").to_lowercase().into(),
                );
                if let Some(cache) = cache {
                    object.insert("cache".into(), cache_json_value(cache));
                }
                object.insert("duration_ms".into(), (duration.as_millis() as u64).into());
                object.insert("completed".into(), state.completed.into());
                object.insert("total".into(), state.total.into());
            }
            ProgressEventKind::WaveStarted { .. }
            | ProgressEventKind::ActionCacheDecision { .. }
            | ProgressEventKind::PhaseCacheDecision { .. }
            | ProgressEventKind::ActionQueued { .. }
            | ProgressEventKind::PhaseWaitStarted { .. }
            | ProgressEventKind::PhaseWaitFinished { .. }
            | ProgressEventKind::ActionStarted { .. }
            | ProgressEventKind::ActionWaitStarted { .. }
            | ProgressEventKind::ActionWaitFinished { .. }
            | ProgressEventKind::ActionActivityStarted { .. }
            | ProgressEventKind::ActionActivityFinished { .. }
            | ProgressEventKind::ResourceUsageChanged { .. } => return,
        }
        drop(state);
        if writeln!(io::stderr().lock(), "{}", record).is_err() {
            self.output_failed
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

fn progress_logical_path_json(path: &nia_build::LogicalPath) -> serde_json::Value {
    use nia_build::LogicalPathRoot;
    let root = match path.root() {
        LogicalPathRoot::Package(package) => {
            serde_json::json!({"kind": "package", "package": package.as_str()})
        }
        LogicalPathRoot::Build => serde_json::json!({"kind": "build"}),
        LogicalPathRoot::Cache => serde_json::json!({"kind": "cache"}),
        LogicalPathRoot::Toolchain => serde_json::json!({"kind": "toolchain"}),
        LogicalPathRoot::Artifact(artifact) => {
            serde_json::json!({"kind": "artifact", "package": artifact.package().as_str(), "artifact": artifact.name()})
        }
    };
    serde_json::json!({"root": root, "path": path.protocol_path()})
}

fn progress_logical_path_label(path: &nia_build::LogicalPath) -> String {
    use nia_build::LogicalPathRoot;
    let root = match path.root() {
        LogicalPathRoot::Package(package) => {
            format!("package:{}", escape_progress_label(package.as_str()))
        }
        LogicalPathRoot::Build => "build".into(),
        LogicalPathRoot::Cache => "cache".into(),
        LogicalPathRoot::Toolchain => "toolchain".into(),
        LogicalPathRoot::Artifact(artifact) => format!(
            "artifact:{}:{}",
            escape_progress_label(artifact.package().as_str()),
            escape_progress_label(artifact.name())
        ),
    };
    format!("{root}/{}", escape_progress_label(&path.protocol_path()))
}

fn cache_json_value(cache: nia_build::ActionCacheOutcome) -> serde_json::Value {
    use nia_build::ActionCacheOutcome;
    match cache {
        ActionCacheOutcome::Hit => serde_json::json!({"status": "hit"}),
        ActionCacheOutcome::Miss(reason) => {
            let reason = match reason {
                nia_build::ActionCacheMissReason::NotFound => "not_found",
                nia_build::ActionCacheMissReason::Invalidated(_) => "invalidated",
                nia_build::ActionCacheMissReason::Uncacheable => "uncacheable",
                nia_build::ActionCacheMissReason::Corrupt => "corrupt",
                nia_build::ActionCacheMissReason::ReadError => "read_error",
                nia_build::ActionCacheMissReason::WriteError => "write_error",
            };
            serde_json::json!({"status": "miss", "reason": reason})
        }
    }
}

fn escape_progress_label(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| {
            if character.is_control() {
                character.escape_default().collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect::<String>()
}

fn format_duration(duration: Duration) -> String {
    if duration.as_secs() > 0 {
        format!("{:.1}s", duration.as_secs_f64())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

fn phase_name(phase: nia_build::ProgressPhase) -> &'static str {
    match phase {
        nia_build::ProgressPhase::Resolve => "Resolving package graph",
        nia_build::ProgressPhase::Prepare => "Preparing build directories",
        nia_build::ProgressPhase::Runner => "Preparing build runner",
        nia_build::ProgressPhase::Configure => "Configuring build plan",
        nia_build::ProgressPhase::Preview => "Selecting build actions",
        nia_build::ProgressPhase::Execute => "Executing build actions",
    }
}

fn progress_observer(
    mode: ProgressMode,
    quiet: u8,
    verbose: u8,
    diagnostics_format: DiagnosticsFormat,
) -> Option<ProgressOutput> {
    if quiet > 0
        || (mode == ProgressMode::None)
        || (mode == ProgressMode::Auto && diagnostics_format == DiagnosticsFormat::Json)
    {
        return None;
    }
    if mode == ProgressMode::Json {
        let observer: Arc<dyn nia_build::ProgressObserver> = Arc::new(JsonProgress {
            state: Mutex::new(JsonProgressState::default()),
            verbosity: verbose,
            output_failed: std::sync::atomic::AtomicBool::new(false),
        });
        Some(ProgressOutput {
            observer,
            process_output_writer: None,
        })
    } else {
        let interactive = mode == ProgressMode::Auto && interactive_terminal_available();
        let progress = PlainProgress::new(verbose, interactive);
        let observer: Arc<dyn nia_build::ProgressObserver> = progress.clone();
        let process_output_writer =
            Some(progress.clone() as Arc<dyn nia_build::ProcessOutputWriter>);
        Some(ProgressOutput {
            observer,
            process_output_writer,
        })
    }
}

fn run_test(context: TestContext) -> ExitCode {
    let TestContext {
        root,
        filter,
        list,
        fail_fast,
        jobs,
        quiet,
        verbose,
        progress,
        optimization,
        profile,
        timings,
        timing_format,
        diagnostics_format,
        toolchain,
    } = context;
    let machine_output =
        diagnostics_format == DiagnosticsFormat::Json || progress == ProgressMode::Json;
    let mut request = nia_build::BuildRequest::new(toolchain)
        .with_test_mode(true)
        .with_forward_output(!machine_output);
    if let Some(output) = progress_observer(progress, quiet, verbose, diagnostics_format) {
        request = request.with_progress_observer(output.observer);
        if let Some(writer) = output.process_output_writer {
            request = request.with_process_output_writer(writer);
        }
    }
    if let Some(root) = root {
        request = request.with_root(root);
    }
    if let Some(filter) = filter {
        request = request.with_test_filter(filter);
    }
    if let Some(jobs) = jobs {
        request = request.with_max_parallel_actions(jobs);
    }
    request = request
        .with_test_list(list)
        .with_test_fail_fast(fail_fast)
        .with_optimization(build_optimization(optimization))
        .with_profile(profile)
        .with_timings(timings)
        .with_timing_format(timing_format);
    match nia_build::run_build(request) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let rendered = match diagnostics_format {
                DiagnosticsFormat::Text => nia_build::render_build_error(&error, None, None),
                DiagnosticsFormat::Json => nia_build::render_build_error_json(&error),
            };
            write_diagnostic_report(&rendered, diagnostics_format);
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

fn run_emit_checked(
    path: &str,
    source: &str,
    runtime: RuntimeMode,
    context: EmitContext,
) -> ExitCode {
    let runtime = match runtime.resolve(&context.toolchain) {
        Ok(runtime) => runtime,
        Err(error) => {
            write_diagnostic_report(
                &render_driver_failure(
                    &nia_driver::DriverError::Runtime(error),
                    Some(path),
                    Some(source),
                    context.diagnostics_format,
                ),
                context.diagnostics_format,
            );
            return ExitCode::FAILURE;
        }
    };
    let output = time_summary_stage(context.timings, "check", || {
        check_with_driver(
            path,
            DriverCheckOptions {
                module_map: context.module_map,
                package_root: context.package_root,
                optimization: context.optimization,
                profile: context.profile,
                timings: context.timings,
                runtime,
            },
            context.toolchain,
        )
    });
    let program =
        match checked_program_from_output(output, path, source, context.diagnostics_format) {
            Ok(program) => program,
            Err(code) => return code,
        };
    write_stdout(format_args!("{program:#?}\n"))
}

fn run_emit_backend(
    path: &str,
    source: &str,
    runtime: RuntimeMode,
    context: EmitContext,
) -> ExitCode {
    let runtime = match runtime.resolve(&context.toolchain) {
        Ok(runtime) => runtime,
        Err(error) => {
            write_diagnostic_report(
                &render_driver_failure(
                    &nia_driver::DriverError::Runtime(error),
                    Some(path),
                    Some(source),
                    context.diagnostics_format,
                ),
                context.diagnostics_format,
            );
            return ExitCode::FAILURE;
        }
    };
    let driver = nia_driver::Driver::new(context.toolchain);
    let output = time_summary_stage(context.timings, "codegen", || {
        codegen_with_driver(
            &driver,
            path,
            DriverCheckOptions {
                module_map: context.module_map,
                package_root: context.package_root,
                optimization: context.optimization,
                profile: context.profile,
                timings: context.timings,
                runtime,
            },
        )
    });
    let program =
        match codegen_program_from_output(output, path, source, context.diagnostics_format) {
            Ok(program) => program,
            Err(code) => return code,
        };
    if context.opt_report {
        print_optimization_report_to_stderr(&program);
    }
    write_stdout(format_args!("{:#?}\n", program.backend_lowering.program))
}

fn run_emit_llvm(path: &str, source: &str, runtime: RuntimeMode, context: EmitContext) -> ExitCode {
    let runtime = match runtime.resolve(&context.toolchain) {
        Ok(runtime) => runtime,
        Err(error) => {
            write_diagnostic_report(
                &render_driver_failure(
                    &nia_driver::DriverError::Runtime(error),
                    Some(path),
                    Some(source),
                    context.diagnostics_format,
                ),
                context.diagnostics_format,
            );
            return ExitCode::FAILURE;
        }
    };
    let driver = nia_driver::Driver::new(context.toolchain);
    let output = time_summary_stage(context.timings, "emit_llvm_ir", || {
        driver.emit_llvm_ir(nia_driver::EmitLlvmRequest::new(
            check_request(path, context.package_root.as_ref())
                .with_module_map(context.module_map)
                .with_optimization(context.optimization)
                .with_profile(context.profile)
                .with_timings(context.timings)
                .with_runtime(runtime),
        ))
    });
    match output.result {
        Ok(artifact) => {
            write_diagnostic_report(
                &nia_driver::render_llvm_ir_warnings(&artifact, Some(path), Some(source)),
                context.diagnostics_format,
            );
            if context.opt_report {
                eprint!("{}", nia_driver::llvm_ir_optimization_report(&artifact));
            }
            for module in artifact.modules {
                let status = write_stdout(format_args!("{}", module.ir));
                if status != ExitCode::SUCCESS {
                    return status;
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            write_diagnostic_report(
                &render_driver_failure(
                    &error,
                    Some(path),
                    Some(source),
                    context.diagnostics_format,
                ),
                context.diagnostics_format,
            );
            ExitCode::FAILURE
        }
    }
}

fn run_emit_obj(path: &str, source: &str, args: Vec<String>, context: EmitContext) -> ExitCode {
    let options = match parse_emit_obj_options(path, args) {
        Ok(options) => options,
        Err(message) => {
            write_diagnostic_report(
                &render_cli_boundary_error(
                    nia_diagnostic::codes::TARGET_CONFIG,
                    &message,
                    context.diagnostics_format,
                ),
                context.diagnostics_format,
            );
            return ExitCode::FAILURE;
        }
    };
    let runtime = match options.runtime.resolve(&context.toolchain) {
        Ok(runtime) => runtime,
        Err(error) => {
            write_diagnostic_report(
                &render_driver_failure(
                    &nia_driver::DriverError::Runtime(error),
                    Some(path),
                    Some(source),
                    context.diagnostics_format,
                ),
                context.diagnostics_format,
            );
            return ExitCode::FAILURE;
        }
    };
    let driver = nia_driver::Driver::with_config(nia_driver::DriverConfig {
        artifact_cache_dir: options.cache_dir.clone(),
        ..nia_driver::DriverConfig::new(context.toolchain)
    });
    let output = time_summary_stage(context.timings, "emit_native_objects", || {
        driver.emit_native_objects(nia_driver::EmitObjectRequest::new(
            check_request(path, context.package_root.as_ref())
                .with_module_map(context.module_map)
                .with_optimization(context.optimization)
                .with_profile(context.profile)
                .with_timings(context.timings)
                .with_runtime(runtime),
        ))
    });
    let objects = match output.result {
        Ok(objects) => objects,
        Err(error) => {
            write_diagnostic_report(
                &render_driver_failure(
                    &error,
                    Some(path),
                    Some(source),
                    context.diagnostics_format,
                ),
                context.diagnostics_format,
            );
            return ExitCode::FAILURE;
        }
    };
    if !objects.diagnostics.is_empty() {
        write_diagnostic_report(
            &nia_driver::render_object_warnings(&objects, Some(path), Some(source)),
            context.diagnostics_format,
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
            write_diagnostic_report(
                &render_driver_failure(
                    &error,
                    Some(path),
                    Some(source),
                    context.diagnostics_format,
                ),
                context.diagnostics_format,
            );
            ExitCode::FAILURE
        }
    }
}

fn run_emit_exe(path: &str, source: &str, args: Vec<String>, context: EmitContext) -> ExitCode {
    let options = match parse_emit_exe_options(path, *context.toolchain.artifact_target(), args) {
        Ok(options) => options,
        Err(message) => {
            write_diagnostic_report(
                &render_cli_boundary_error(
                    nia_diagnostic::codes::TARGET_CONFIG,
                    &message,
                    context.diagnostics_format,
                ),
                context.diagnostics_format,
            );
            return ExitCode::FAILURE;
        }
    };
    let runtime = match options.runtime.resolve(&context.toolchain) {
        Ok(runtime) => runtime,
        Err(error) => {
            write_diagnostic_report(
                &render_driver_failure(
                    &nia_driver::DriverError::Runtime(error),
                    Some(path),
                    Some(source),
                    context.diagnostics_format,
                ),
                context.diagnostics_format,
            );
            return ExitCode::FAILURE;
        }
    };
    let driver = nia_driver::Driver::with_config(nia_driver::DriverConfig {
        artifact_cache_dir: options.cache_dir.clone(),
        ..nia_driver::DriverConfig::new(context.toolchain)
    });
    let output = time_summary_stage(context.timings, "link_executable", || {
        driver.link_executable(nia_driver::LinkExecutableRequest {
            check: check_request(path, context.package_root.as_ref())
                .with_module_map(context.module_map)
                .with_optimization(context.optimization)
                .with_profile(context.profile)
                .with_timings(context.timings)
                .with_runtime(runtime),
            output: options.output.clone(),
            link_options: options.link_options.clone(),
            link_time_optimization: options.link_time_optimization,
        })
    });
    let executable = match output.result {
        Ok(executable) => executable,
        Err(error) => {
            write_diagnostic_report(
                &render_driver_failure(
                    &error,
                    Some(path),
                    Some(source),
                    context.diagnostics_format,
                ),
                context.diagnostics_format,
            );
            return ExitCode::FAILURE;
        }
    };
    if !executable.diagnostics.is_empty() {
        write_diagnostic_report(
            &nia_driver::render_executable_warnings(&executable, Some(path), Some(source)),
            context.diagnostics_format,
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
    runtime: RuntimeMode,
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
    let mut runtime = RuntimeMode::Bare;
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
    runtime: RuntimeMode,
    link_options: nia_linker::LinkOptions,
    link_time_optimization: nia_driver::LinkTimeOptimization,
}

fn parse_emit_exe_options(
    source: &str,
    target: nia_target::TargetConfig,
    args: Vec<String>,
) -> Result<EmitExeOptions, String> {
    let mut output = None::<PathBuf>;
    let mut cache_dir = None::<PathBuf>;
    let mut runtime = RuntimeMode::Freestanding;
    let mut link_options = nia_linker::LinkOptions::default();
    let mut link_time_optimization = nia_driver::LinkTimeOptimization::Off;
    let mut explicit_linker_program = false;
    let mut form = None::<nia_linker::ExecutableForm>;
    let mut interpreter = None::<String>;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--cache-dir=") {
            cache_dir = Some(PathBuf::from(value));
            continue;
        }
        if let Some(value) = arg.strip_prefix("--lto=") {
            link_time_optimization = parse_link_time_optimization(value)?;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--runtime=") {
            runtime = parse_runtime(value)
                .map_err(|message| format!("{message} for `nia emit --exe`"))?;
            if runtime != RuntimeMode::Freestanding {
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
        if let Some(value) = arg.strip_prefix("--executable=") {
            form = Some(parse_executable_form(value)?);
            continue;
        }
        if let Some(value) = arg.strip_prefix("--interpreter=") {
            interpreter = Some(value.to_string());
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
            select_linker_flavor(&mut link_options.linker, flavor, explicit_linker_program);
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
            "--lto" => {
                let Some(value) = iter.next() else {
                    return Err("missing mode after `--lto`".to_string());
                };
                link_time_optimization = parse_link_time_optimization(&value)?;
            }
            "--runtime" => {
                let Some(value) = iter.next() else {
                    return Err("missing runtime after `--runtime`".to_string());
                };
                runtime = parse_runtime(&value)
                    .map_err(|message| format!("{message} for `nia emit --exe`"))?;
                if runtime != RuntimeMode::Freestanding {
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
            "--executable" => {
                let Some(value) = iter.next() else {
                    return Err("missing form after `--executable`".to_string());
                };
                form = Some(parse_executable_form(&value)?);
            }
            "--interpreter" => {
                let Some(value) = iter.next() else {
                    return Err("missing path after `--interpreter`".to_string());
                };
                interpreter = Some(value);
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
                select_linker_flavor(&mut link_options.linker, flavor, explicit_linker_program);
            }
            _ => return Err(format!("unknown `nia emit --exe` option `{arg}`")),
        }
    }
    // An interpreter exists only for a dynamic executable, whatever the
    // order in which the options appear.
    link_options.form = match (form, interpreter) {
        (Some(nia_linker::ExecutableForm::Dynamic(_)), Some(path)) => Some(
            nia_linker::ExecutableForm::Dynamic(nia_linker::Interpreter::Path(path)),
        ),
        (_, Some(_)) => {
            return Err("`--interpreter` requires `--executable dynamic`".to_string());
        }
        (form, None) => form,
    };
    Ok(EmitExeOptions {
        output: output
            .unwrap_or_else(|| default_output_path(source, target.executable_extension())),
        cache_dir,
        runtime,
        link_options,
        link_time_optimization,
    })
}

fn parse_link_time_optimization(value: &str) -> Result<nia_driver::LinkTimeOptimization, String> {
    match value {
        "off" => Ok(nia_driver::LinkTimeOptimization::Off),
        "thin" => Ok(nia_driver::LinkTimeOptimization::Thin),
        "full" => Ok(nia_driver::LinkTimeOptimization::Full),
        _ => Err(format!(
            "unknown LTO mode `{value}`; expected `off`, `thin`, or `full`"
        )),
    }
}

// An LLD flavor discovers its program unless `--linker` names one, rather
// than reinterpreting `NIA_LINKER` under LLD's contract. Other flavors keep
// the selected program: `NIA_LINKER`, or `ld` when it is unset.
fn select_linker_flavor(
    linker: &mut nia_linker::ExecutableLinker,
    flavor: nia_linker::LinkerFlavor,
    explicit_program: bool,
) {
    if explicit_program || !flavor.is_lld() {
        linker.flavor = Some(flavor);
    } else {
        *linker = nia_linker::ExecutableLinker::with_flavor(flavor);
    }
}

fn parse_linker_flavor(value: &str) -> Result<nia_linker::LinkerFlavor, String> {
    match value {
        "gnu" => Ok(nia_linker::LinkerFlavor::Gnu),
        "lld" => Ok(nia_linker::LinkerFlavor::Lld),
        "lld-link" => Ok(nia_linker::LinkerFlavor::LldLink),
        "ld64.lld" => Ok(nia_linker::LinkerFlavor::Ld64Lld),
        "self-hosted-elf" => Ok(nia_linker::LinkerFlavor::SelfHostedElf),
        _ => Err(format!(
            "unknown linker flavor `{value}`; expected `gnu`, `lld`, `lld-link`, `ld64.lld`, \n             or `self-hosted-elf`"
        )),
    }
}

// The form alone; a dynamic interpreter is chosen separately.
fn parse_executable_form(value: &str) -> Result<nia_linker::ExecutableForm, String> {
    match value {
        "static-pie" => Ok(nia_linker::ExecutableForm::StaticPie),
        "static" => Ok(nia_linker::ExecutableForm::Static),
        "dynamic" => Ok(nia_linker::ExecutableForm::Dynamic(
            nia_linker::Interpreter::Standard,
        )),
        _ => Err(format!(
            "unknown executable form `{value}`; expected `static-pie`, `static`, or `dynamic`"
        )),
    }
}

fn parse_runtime(value: &str) -> Result<RuntimeMode, String> {
    match value {
        "bare" => Ok(RuntimeMode::Bare),
        "freestanding" => Ok(RuntimeMode::Freestanding),
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

fn print_optimization_report(program: &nia_driver::CodegenProgram) -> ExitCode {
    write_stdout(format_args!("{}", nia_driver::optimization_report(program)))
}

fn print_optimization_report_to_stderr(program: &nia_driver::CodegenProgram) {
    eprint!("{}", nia_driver::optimization_report(program));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_target() -> nia_target::TargetConfig {
        nia_target::TargetConfig::parse("x86_64-unknown-linux").unwrap()
    }

    #[test]
    fn emit_exe_selects_executable_forms_and_interpreters() {
        let parse = |args: &[&str]| {
            parse_emit_exe_options(
                "main.nia",
                test_target(),
                args.iter().map(|arg| (*arg).to_owned()).collect(),
            )
        };
        assert_eq!(parse(&[]).unwrap().link_options.form, None);
        assert_eq!(
            parse(&["--executable", "static"])
                .unwrap()
                .link_options
                .form,
            Some(nia_linker::ExecutableForm::Static)
        );
        // The interpreter may precede the form it belongs to.
        assert_eq!(
            parse(&["--interpreter=/loader", "--executable=dynamic"])
                .unwrap()
                .link_options
                .form,
            Some(nia_linker::ExecutableForm::Dynamic(
                nia_linker::Interpreter::Path("/loader".to_owned())
            ))
        );
        for args in [
            &["--interpreter", "/loader"][..],
            &["--executable", "static-pie", "--interpreter", "/l"],
        ] {
            assert_eq!(
                parse(args).err().as_deref(),
                Some("`--interpreter` requires `--executable dynamic`")
            );
        }
        assert!(parse(&["--executable", "pie"]).is_err());
    }

    #[test]
    fn emit_exe_output_extension_follows_the_target() {
        let windows = nia_target::TargetConfig::parse("x86_64-pc-windows-msvc").unwrap();
        let options = parse_emit_exe_options("main.nia", windows, Vec::new()).unwrap();
        assert_eq!(options.output, PathBuf::from("main.exe"));
        let options = parse_emit_exe_options("main.nia", test_target(), Vec::new()).unwrap();
        assert_eq!(options.output, PathBuf::from("main"));
    }

    #[test]
    fn emit_exe_parses_explicit_lto_policies() {
        let options = parse_emit_exe_options(
            "main.nia",
            test_target(),
            vec!["--lto=thin".to_owned(), "-o".to_owned(), "main".to_owned()],
        )
        .expect("parse ThinLTO executable options");
        assert_eq!(
            options.link_time_optimization,
            nia_driver::LinkTimeOptimization::Thin
        );
        assert_eq!(options.output, PathBuf::from("main"));

        let options =
            parse_emit_exe_options("main.nia", test_target(), vec!["--lto=full".to_owned()])
                .expect("parse full-LTO executable options");
        assert_eq!(
            options.link_time_optimization,
            nia_driver::LinkTimeOptimization::Full
        );

        let error = parse_emit_exe_options("main.nia", test_target(), vec!["--lto=fat".to_owned()])
            .err()
            .expect("unknown LTO modes must be rejected");
        assert!(
            error.contains("expected `off`, `thin`, or `full`"),
            "{error}"
        );
    }

    #[test]
    fn profile_defaults_and_release_optimization_are_workflow_based() {
        let CliAction::Run(debug) = parse_cli(vec!["build".into()]).expect("parse debug build")
        else {
            panic!("expected build command");
        };
        assert_eq!(debug.optimization, NiaOptimizationLevel::O0);
        assert_eq!(debug.profile, BuildProfile::Debug);

        let CliAction::Run(release) =
            parse_cli(vec!["--release".into(), "build".into()]).expect("parse release build")
        else {
            panic!("expected build command");
        };
        assert_eq!(release.optimization, NiaOptimizationLevel::O2);
        assert_eq!(release.profile, BuildProfile::Release);

        let CliAction::Run(test) = parse_cli(vec!["test".into()]).expect("parse test workflow")
        else {
            panic!("expected test command");
        };
        assert_eq!(test.optimization, NiaOptimizationLevel::O0);
        assert_eq!(test.profile, BuildProfile::Debug);

        let CliAction::Run(test_release) = parse_cli(vec!["test".into(), "--release".into()])
            .expect("parse release test workflow")
        else {
            panic!("expected test command");
        };
        assert_eq!(test_release.optimization, NiaOptimizationLevel::O2);
        assert_eq!(test_release.profile, BuildProfile::Release);
    }

    #[test]
    fn explicit_optimization_overrides_profile_default() {
        let CliAction::Run(cli) = parse_cli(vec!["--release".into(), "-O0".into(), "build".into()])
            .expect("parse release build with explicit optimization")
        else {
            panic!("expected build command");
        };
        assert_eq!(cli.profile, BuildProfile::Release);
        assert_eq!(cli.optimization, NiaOptimizationLevel::O0);
    }

    #[test]
    fn test_is_a_command_workflow_not_a_profile_name() {
        let error = parse_cli(vec!["--profile".into(), "test".into(), "build".into()])
            .expect_err("test must not be a build profile");
        assert!(error.message.contains("expected debug or release"));
    }

    #[test]
    fn module_map_directory_resolves_package_root() {
        let directory = nia_test_support::test_dir("module-map-package-root");
        let package_root = directory.join("pkg.nia");
        fs::write(&package_root, "").expect("write package root");

        assert_eq!(
            resolve_package_root_path(directory.to_string_lossy().as_ref())
                .expect("resolve package directory"),
            package_root
        );
    }

    #[test]
    fn module_map_directory_requires_package_root() {
        let directory = nia_test_support::test_dir("module-map-missing-package-root");

        let error = resolve_package_root_path(directory.to_string_lossy().as_ref())
            .expect_err("directory without pkg.nia must fail");

        assert!(error.contains("does not contain `pkg.nia`"), "{error}");
    }

    #[test]
    fn unresolved_module_map_directory_uses_package_root_convention() {
        assert_eq!(
            resolve_package_root_path("vendor/math").expect("resolve directory spelling"),
            PathBuf::from("vendor/math/pkg.nia")
        );
        assert_eq!(
            resolve_package_root_path("vendor/math.nia").expect("preserve source spelling"),
            PathBuf::from("vendor/math.nia")
        );
    }

    #[test]
    fn discover_package_root_uses_nearest_ancestor() {
        let root = nia_test_support::test_dir("discover-package-root-nearest");
        let package_root = root.join("pkg.nia");
        let nested = root.join("src/nested");
        fs::create_dir_all(&nested).expect("create nested source directory");
        fs::write(&package_root, "").expect("write package root");
        let entry = nested.join("main.nia");

        assert_eq!(
            discover_package_root(entry.to_string_lossy().as_ref()).expect("discover package root"),
            Some(SourcePath::new(package_root.to_string_lossy()))
        );
    }

    #[test]
    fn discover_package_root_prefers_nested_package() {
        let root = nia_test_support::test_dir("discover-package-root-nested");
        let nested = root.join("vendor/math");
        fs::create_dir_all(&nested).expect("create nested package directory");
        fs::write(root.join("pkg.nia"), "").expect("write outer package root");
        let nested_package = nested.join("pkg.nia");
        fs::write(&nested_package, "").expect("write nested package root");
        let entry = nested.join("main.nia");

        assert_eq!(
            discover_package_root(entry.to_string_lossy().as_ref())
                .expect("discover nested package root"),
            Some(SourcePath::new(nested_package.to_string_lossy()))
        );
    }

    #[test]
    fn discover_package_root_does_not_self_attach_pkg_module() {
        let root = nia_test_support::test_dir("discover-package-root-self");
        let package_root = root.join("pkg.nia");
        fs::write(&package_root, "").expect("write package root");

        assert_eq!(
            discover_package_root(package_root.to_string_lossy().as_ref())
                .expect("discover package root"),
            None
        );
    }

    #[test]
    fn discover_package_root_keeps_standalone_sources_without_root() {
        let root = nia_test_support::test_dir("discover-package-root-standalone");
        let source = root.join("main.nia");
        fs::create_dir_all(&root).expect("create standalone source directory");

        assert_eq!(
            discover_package_root(source.to_string_lossy().as_ref())
                .expect("discover standalone package root"),
            None
        );
    }

    #[test]
    fn source_directory_prefers_main_entry() {
        let root = nia_test_support::test_dir("source-directory-prefers-main");
        fs::write(root.join("pkg.nia"), "").expect("write package root");
        fs::write(root.join("main.nia"), "").expect("write main entry");

        assert_eq!(
            resolve_source_entry(root.to_string_lossy().as_ref()).expect("resolve source entry"),
            root.join("main.nia").to_string_lossy()
        );
    }

    #[test]
    fn source_directory_falls_back_to_package_root() {
        let root = nia_test_support::test_dir("source-directory-package-root-fallback");
        fs::write(root.join("pkg.nia"), "").expect("write package root");

        assert_eq!(
            resolve_source_entry(root.to_string_lossy().as_ref()).expect("resolve source entry"),
            root.join("pkg.nia").to_string_lossy()
        );
    }

    #[test]
    fn source_directory_requires_an_entry_file() {
        let root = nia_test_support::test_dir("source-directory-missing-entry");
        let error = resolve_source_entry(root.to_string_lossy().as_ref())
            .expect_err("empty source directory must fail");

        assert!(
            error.contains("neither `main.nia` nor `pkg.nia`"),
            "{error}"
        );
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
    fn progress_controls_parse_and_reject_conflicts() {
        let command = parse_build_command(
            ["-vv", "--progress=json"]
                .into_iter()
                .map(ToString::to_string)
                .collect(),
        )
        .expect("progress options");
        assert!(matches!(
            command,
            CliCommand::Build {
                verbose: 2,
                progress: ProgressMode::Json,
                quiet: 0,
                ..
            }
        ));

        let error = parse_test_command(vec!["-q".into(), "-v".into()])
            .expect_err("quiet and verbose conflict");
        assert!(error.message.contains("cannot be used together"));
    }

    #[test]
    fn json_progress_resolves_machine_formats_before_running() {
        let mut cli = match parse_cli(
            ["--timings", "build", "--progress=json"]
                .into_iter()
                .map(ToString::to_string)
                .collect(),
        )
        .expect("parse JSON progress")
        {
            CliAction::Run(cli) => cli,
            _ => panic!("expected runnable command"),
        };
        resolve_output_policy(&mut cli).expect("resolve JSON stream formats");
        assert_eq!(cli.diagnostics_format, DiagnosticsFormat::Json);
        assert_eq!(cli.timing_format, TimingFormat::Json);

        let mut cli = match parse_cli(
            ["--diagnostics-format=text", "build", "--progress=json"]
                .into_iter()
                .map(ToString::to_string)
                .collect(),
        )
        .expect("parse explicit text diagnostics")
        {
            CliAction::Run(cli) => cli,
            _ => panic!("expected runnable command"),
        };
        let error = resolve_output_policy(&mut cli).expect_err("reject mixed stderr formats");
        assert!(error.message.contains("machine-readable"));
    }

    #[test]
    fn progress_labels_escape_terminal_controls() {
        assert_eq!(escape_progress_label("pkg\n\x1b[31m"), "pkg\\n\\u{1b}[31m");
        assert_eq!(escape_progress_label("任务/编译"), "任务/编译");
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

    #[test]
    fn build_diagnostic_flags_are_typed() {
        let command = parse_build_command(
            [
                "--dry-run",
                "--explain",
                "--print-plan",
                "--print-cache",
                "--print-observations",
            ]
            .into_iter()
            .map(ToString::to_string)
            .collect(),
        )
        .unwrap();
        assert!(matches!(
            command,
            CliCommand::Build {
                dry_run: true,
                explain: true,
                print_plan: true,
                print_cache: true,
                print_observations: true,
                ..
            }
        ));
    }

    #[test]
    fn build_lto_accepts_typed_modes_and_rejects_invalid_values() {
        for (args, expected) in [
            (vec!["--lto=off"], nia_driver::LinkTimeOptimization::Off),
            (
                vec!["--lto", "thin"],
                nia_driver::LinkTimeOptimization::Thin,
            ),
            (vec!["--lto=full"], nia_driver::LinkTimeOptimization::Full),
        ] {
            let command = parse_build_command(
                args.into_iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_else(|error| panic!("parse build LTO: {}", error.message));
            assert!(matches!(
                command,
                CliCommand::Build { link_time_optimization, .. }
                    if link_time_optimization == expected
            ));
        }

        for args in [vec!["--lto"], vec!["--lto=invalid"]] {
            let error = parse_build_command(
                args.into_iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
            )
            .expect_err("invalid build LTO must fail");
            assert!(error.message.contains("lto") || error.message.contains("LTO"));
        }
    }

    #[test]
    fn build_requires_explicit_root_without_confusing_named_steps() {
        let error = parse_build_command(vec![".".to_string()])
            .expect_err("positional package paths must fail");
        assert!(error.message.contains("must be passed with `--root`"));

        let command = parse_build_command(vec!["--root".to_string(), ".".to_string()])
            .unwrap_or_else(|error| panic!("parse build root: {}", error.message));
        assert!(matches!(
            command,
            CliCommand::Build {
                root: Some(root),
                step: None,
                ..
            } if root == Path::new(".")
        ));

        let command = parse_build_command(vec!["check".to_string()])
            .unwrap_or_else(|error| panic!("parse build step: {}", error.message));
        assert!(matches!(
            command,
            CliCommand::Build {
                root: None,
                step: Some(step),
                ..
            } if step == "check"
        ));
    }

    #[test]
    fn test_command_parses_selection_controls() {
        let command = parse_test_command(
            [
                "--root",
                "tests",
                "--filter",
                "parser",
                "--list",
                "--fail-fast",
                "-j4",
            ]
            .into_iter()
            .map(ToString::to_string)
            .collect(),
        )
        .unwrap_or_else(|error| panic!("parse test command: {}", error.message));
        assert!(matches!(
            command,
            CliCommand::Test {
                root: Some(root),
                filter: Some(filter),
                list: true,
                fail_fast: true,
                jobs: Some(jobs),
                ..
            } if root.as_path() == std::path::Path::new("tests")
                && filter == "parser"
                && jobs.get() == 4
        ));
    }

    #[test]
    fn diagnostics_format_is_global_and_validated() {
        let action = parse_cli(vec![
            "--diagnostics-format=json".to_string(),
            "check".to_string(),
            "main.nia".to_string(),
        ])
        .expect("parse diagnostics format");
        assert!(matches!(
            action,
            CliAction::Run(Cli {
                diagnostics_format: DiagnosticsFormat::Json,
                ..
            })
        ));

        let error = parse_cli(vec![
            "--diagnostics-format=xml".to_string(),
            "check".to_string(),
            "main.nia".to_string(),
        ])
        .expect_err("invalid diagnostics format must fail");
        assert!(error.message.contains("expected text or json"));
    }

    #[test]
    fn cli_boundary_diagnostics_keep_code_and_help_in_both_formats() {
        let text = render_cli_boundary_error(
            nia_diagnostic::codes::TOOLCHAIN,
            "invalid toolchain layout",
            DiagnosticsFormat::Text,
        );
        assert!(text.contains("error[E0104]"), "{text}");
        assert!(text.contains("help: check the Nia installation"), "{text}");

        let json = render_cli_boundary_error(
            nia_diagnostic::codes::TOOLCHAIN,
            "invalid toolchain layout",
            DiagnosticsFormat::Json,
        );
        assert!(json.contains("\"code\":\"E0104\""), "{json}");
        assert!(json.contains("check the Nia installation"), "{json}");
    }

    #[test]
    fn cli_invocation_diagnostics_keep_code_help_and_format() {
        let text = render_cli_invocation_error(
            "unknown command `frobnicate`",
            HelpTopic::Main,
            DiagnosticsFormat::Text,
        );
        assert!(
            text.contains("error[E0105]: unknown command `frobnicate`"),
            "{text}"
        );
        assert!(text.contains("run `nia help`"), "{text}");

        let json = render_cli_invocation_error(
            "unknown command `frobnicate`",
            HelpTopic::Main,
            DiagnosticsFormat::Json,
        );
        assert!(json.contains("\"code\":\"E0105\""), "{json}");
        assert!(json.contains("run `nia help`"), "{json}");
    }

    #[test]
    fn interactive_text_reports_color_only_diagnostic_sections() {
        let rendered = "error[E0101]: bad input\n  --> main.nia:1:1\nnote: context\nhelp: fix it\n";
        let colored = colorize_diagnostic_report(rendered);
        assert!(colored.contains("\x1b[1;31merror[E0101]"), "{colored:?}");
        assert!(colored.contains("\x1b[36m  --> main.nia"), "{colored:?}");
        assert!(colored.contains("\x1b[34mnote: context"), "{colored:?}");
        assert!(colored.contains("\x1b[32mhelp: fix it"), "{colored:?}");
        assert!(colored.contains("\x1b[0m"), "{colored:?}");
    }

    #[test]
    fn progress_frame_truncates_by_display_width() {
        let rendered = truncate_progress_cells("任务/very-long-action", 10);
        assert!(unicode_width::UnicodeWidthStr::width(rendered.as_str()) <= 10);
        assert!(rendered.ends_with("..."));
    }

    #[test]
    fn progress_panel_caps_rows_and_keeps_canonical_order() {
        let first = nia_build::ActionKey::new(nia_build::PackageKey::root(), "first")
            .expect("first action");
        let second = nia_build::ActionKey::new(nia_build::PackageKey::root(), "second")
            .expect("second action");
        let now = Instant::now();
        let mut state = PlainProgressState {
            total: 2,
            finished: 0,
            started: Some(now),
            order: vec![first.clone(), second.clone()],
            wave: 0,
            resource_capacity: 2,
            resource_used: 2,
            ..PlainProgressState::default()
        };
        state.active.insert(
            first,
            ActiveProgressAction {
                resource_class: nia_build::ActionResourceClass::Cpu,
                started: now,
                wave: 0,
                scheduler_wait: Duration::ZERO,
                resource_wait: Duration::ZERO,
            },
        );
        state.active.insert(
            second,
            ActiveProgressAction {
                resource_class: nia_build::ActionResourceClass::Io,
                started: now,
                wave: 0,
                scheduler_wait: Duration::ZERO,
                resource_wait: Duration::ZERO,
            },
        );
        let lines = progress_frame_lines(&state, 2, now, 80, 6);
        assert!(lines.len() <= 5);
        assert!(lines.iter().any(|line| line.contains("first")));
        assert!(lines.iter().any(|line| line.contains("second")));
    }

    #[test]
    fn progress_panel_exposes_blocked_actions_without_inventing_active_workers() {
        let now = Instant::now();
        let keys = (0..10)
            .map(|index| {
                nia_build::ActionKey::new(nia_build::PackageKey::root(), format!("task-{index}"))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let mut state = PlainProgressState {
            total: 10,
            order: keys.clone(),
            started: Some(now),
            resource_capacity: 2,
            ..Default::default()
        };
        state
            .queued
            .insert(keys[0].clone(), nia_build::ActionResourceClass::Cpu);
        for action in keys {
            state
                .waits
                .insert(action, (nia_build::ProgressWaitKind::Dependency, now));
        }
        let lines = progress_frame_lines(&state, 2, now, 100, 10);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("0 active; 1 queued; 10 dependency-blocked"))
        );
        assert!(lines.iter().any(|line| line.contains("dependency")));
        assert!(lines.iter().any(|line| line.contains("+6 actions")));
        assert!(lines.len() <= 8);
    }

    #[test]
    fn bootstrap_panel_names_real_phase_and_cache_wait() {
        let now = Instant::now();
        let state = PlainProgressState {
            current_phase: Some((nia_build::ProgressPhase::Configure, now)),
            phase_wait: Some((nia_build::ProgressPhase::Configure, now)),
            ..Default::default()
        };
        let lines = progress_frame_lines(&state, 3, now, 80, 10);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Configuring build plan (cache lock)"));
    }
}
