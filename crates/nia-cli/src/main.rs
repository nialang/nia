// SPDX-License-Identifier: GPL-3.0-or-later
//! Command-line ownership for compiler inspection, checking, emission, and builds.
//!
//! The commands are `build` and `test`, which run a package's `build.nia`
//! through `nia-build`, `check`, which validates without code generation, and
//! `emit`, which writes one product selected by `--tokens`, `--ast`,
//! `--checked`, `--backend`, `--llvm`, `--obj`, or `--exe`.
//!
//! - `args` declares every option once and scans arguments against it;
//! - `cli` turns the scan into a typed invocation, validating every option
//!   before any file is read;
//! - `help` renders help from the same option table;
//! - `progress` renders build progress;
//! - this module resolves the toolchain and runs the command through typed
//!   `nia-driver` or `nia-build` requests.
//!
//! Requested products use stdout; diagnostics, timings, progress, and the
//! optimization report use stderr. A malformed invocation exits with status 2,
//! a failed command with status 1. Compiler failures reach this boundary as
//! structured diagnostics rather than unwinding through the CLI.
#[cfg(feature = "perf-alloc")]
use std::alloc::System;
use std::{
    env, fmt, fs, io,
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use nia_driver::{NiaOptimizationLevel, RuntimeSpec};
use nia_timing::TimingFormat;

mod args;
mod cli;
mod help;
mod progress;

use cli::{
    Cli, CliAction, CliCommand, CliError, DiagnosticsFormat, EmitExeOptions, EmitObjOptions,
    EmitTarget, HelpTopic, ObjectOutput, PackageOptions, ProgressMode, RuntimeMode,
    USAGE_EXIT_CODE,
};
use help::{HelpStyle, error_help_text, help_command, help_text};

#[cfg(feature = "perf-alloc")]
#[global_allocator]
static GLOBAL_ALLOCATOR: nia_timing::CountingAllocator<System> =
    nia_timing::CountingAllocator::new(System);

fn main() -> ExitCode {
    #[cfg(feature = "perf-alloc")]
    nia_timing::register_allocation_instrumentation();
    let args = match collect_args() {
        Ok(args) => args,
        Err(error) => {
            report_cli_error(&error, DiagnosticsFormat::Text);
            return ExitCode::from(USAGE_EXIT_CODE);
        }
    };
    let diagnostics_format = cli::requested_diagnostics_format(&args);
    match cli::parse_cli(args) {
        Ok(CliAction::Help(topic)) => write_stdout(format_args!(
            "{}",
            help_text(topic, HelpStyle::for_stdout())
        )),
        Ok(CliAction::Version) => write_stdout(format_args!(
            "nia {}
",
            nia_compat::COMPILER_VERSION
        )),
        Ok(CliAction::Run(cli)) => {
            let mut cli = *cli;
            if let Err(error) = resolve_output_policy(&mut cli) {
                report_cli_error(&error, cli.diagnostics_format);
                return ExitCode::from(USAGE_EXIT_CODE);
            }
            let timing_options = cli.timing_options();
            nia_timing::collect_to_stderr(timing_options, || run_cli(cli))
        }
        Err(error) => {
            report_cli_error(&error, diagnostics_format);
            ExitCode::from(USAGE_EXIT_CODE)
        }
    }
}

/// Collects the arguments as UTF-8, reporting the first argument that is not
/// valid Unicode instead of panicking.
fn collect_args() -> Result<Vec<String>, CliError> {
    env::args_os()
        .skip(1)
        .map(|arg| {
            arg.into_string().map_err(|arg| {
                CliError::new(
                    format!("argument `{}` is not valid Unicode", arg.to_string_lossy()),
                    HelpTopic::Main,
                )
            })
        })
        .collect()
}

fn write_stdout(args: fmt::Arguments<'_>) -> ExitCode {
    match io::stdout().lock().write_fmt(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => {
            write_stderr(format_args!(
                "error: failed printing to stdout: {error}
"
            ));
            ExitCode::FAILURE
        }
    }
}

/// Writes to stderr, ignoring failures: there is no other stream left on
/// which to report them, and a closed stderr must not abort the process.
fn write_stderr(args: fmt::Arguments<'_>) {
    let _ = io::stderr().lock().write_fmt(args);
}

fn report_cli_error(error: &CliError, diagnostics_format: DiagnosticsFormat) {
    let rendered = render_cli_invocation_error(&error.message, error.help, diagnostics_format);
    write_diagnostic_report(&rendered, diagnostics_format);
    if diagnostics_format == DiagnosticsFormat::Text {
        write_stderr(format_args!(
            "
{}",
            error_help_text(error.help, HelpStyle::for_stderr())
        ));
    }
}

fn write_diagnostic_report(rendered: &str, format: DiagnosticsFormat) {
    if rendered.is_empty() {
        return;
    }
    if format == DiagnosticsFormat::Text
        && io::stderr().is_terminal()
        && std::env::var_os("NO_COLOR").is_none()
    {
        write_stderr(format_args!("{}", colorize_diagnostic_report(rendered)));
    } else {
        write_stderr(format_args!("{rendered}"));
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

fn render_cli_invocation_error(
    message: &str,
    help_topic: HelpTopic,
    format: DiagnosticsFormat,
) -> String {
    let help = format!("for more information, run `{}`", help_command(help_topic));
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

fn run_cli(cli: Cli) -> ExitCode {
    let diagnostics_format = cli.diagnostics_format;
    let toolchain = match resolve_toolchain_layout(cli.resource_root.clone(), cli.target) {
        Ok(toolchain) => toolchain,
        Err(message) => {
            report_boundary_error(
                nia_diagnostic::codes::TOOLCHAIN,
                &message,
                diagnostics_format,
            );
            return ExitCode::FAILURE;
        }
    };
    let settings = CompileSettings {
        module_map: cli.module_map,
        optimization: cli.optimization,
        profile: cli.profile,
        timings: cli.timings,
        timing_format: cli.timing_format,
        diagnostics_format,
    };
    match cli.command {
        CliCommand::Build {
            package,
            step,
            link_time_optimization,
            dry_run,
            explain,
            print_plan,
            print_cache,
            print_observations,
        } => {
            let mut request = package_request(toolchain, &package, &settings);
            if let Some(step) = step {
                request = request.with_step(step);
            }
            let request = request
                .with_link_time_optimization(link_time_optimization)
                .with_dry_run(dry_run)
                .with_explain(explain)
                .with_print_plan(print_plan)
                .with_print_cache(print_cache)
                .with_print_observations(print_observations)
                .with_output_json(machine_output(&package, diagnostics_format));
            run_package_request(request, diagnostics_format)
        }
        CliCommand::Test {
            package,
            filter,
            list,
            fail_fast,
        } => {
            let mut request = package_request(toolchain, &package, &settings).with_test_mode(true);
            if let Some(filter) = filter {
                request = request.with_test_filter(filter);
            }
            let request = request.with_test_list(list).with_test_fail_fast(fail_fast);
            run_package_request(request, diagnostics_format)
        }
        CliCommand::Update { root, names } => run_update(root, &names, diagnostics_format),
        CliCommand::Check {
            path,
            opt_report,
            runtime,
            cache_dir,
        } => {
            let Some(entry) = load_entry(&path, diagnostics_format) else {
                return ExitCode::FAILURE;
            };
            run_check(
                &entry,
                CheckRunOptions {
                    settings,
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
            let Some(entry) = load_entry(&path, diagnostics_format) else {
                return ExitCode::FAILURE;
            };
            run_emit(
                &entry,
                target,
                EmitContext {
                    settings,
                    opt_report,
                    toolchain,
                },
            )
        }
    }
}

/// Compiler settings shared by every command.
struct CompileSettings {
    module_map: nia_driver::ModuleMap,
    optimization: NiaOptimizationLevel,
    profile: nia_target_config::BuildProfile,
    timings: nia_driver::TimingMode,
    timing_format: TimingFormat,
    diagnostics_format: DiagnosticsFormat,
}

/// A loaded source entry for `check` and `emit`.
struct SourceEntry {
    path: String,
    source: String,
}

impl SourceEntry {
    fn request(
        &self,
        settings: &CompileSettings,
        runtime: RuntimeSpec,
    ) -> nia_driver::CheckRequest {
        nia_driver::CheckRequest::new(&self.path)
            .with_module_map(settings.module_map.clone())
            .with_optimization(settings.optimization)
            .with_profile(settings.profile)
            .with_timings(settings.timings)
            .with_runtime(runtime)
    }

    fn report_driver_error(&self, error: &nia_driver::DriverError, format: DiagnosticsFormat) {
        write_diagnostic_report(
            &render_driver_failure(error, Some(&self.path), Some(&self.source), format),
            format,
        );
    }

    /// Resolves the runtime resources, reporting a missing runtime against
    /// this entry.
    fn resolve_runtime(
        &self,
        runtime: RuntimeMode,
        toolchain: &nia_toolchain::ToolchainLayout,
        format: DiagnosticsFormat,
    ) -> Option<RuntimeSpec> {
        match runtime.resolve(toolchain) {
            Ok(runtime) => Some(runtime),
            Err(error) => {
                self.report_driver_error(&nia_driver::DriverError::Runtime(error), format);
                None
            }
        }
    }
}

/// Resolves, reads, and locates the package of a source argument, reporting
/// any failure as a load diagnostic.
fn load_entry(path: &str, format: DiagnosticsFormat) -> Option<SourceEntry> {
    let loaded = resolve_source_entry(path).and_then(|path| {
        let source = read_source(&path)?;
        Ok(SourceEntry { path, source })
    });
    match loaded {
        Ok(entry) => Some(entry),
        Err(message) => {
            report_boundary_error(nia_diagnostic::codes::LOAD, &message, format);
            None
        }
    }
}

fn report_boundary_error(
    code: nia_diagnostic::codes::DiagnosticCodeDef,
    message: &str,
    format: DiagnosticsFormat,
) {
    write_diagnostic_report(&render_cli_boundary_error(code, message, format), format);
}

fn machine_output(package: &PackageOptions, format: DiagnosticsFormat) -> bool {
    format == DiagnosticsFormat::Json || package.progress == ProgressMode::Json
}

/// The request options `nia build` and `nia test` share.
fn package_request(
    toolchain: Arc<nia_toolchain::ToolchainLayout>,
    package: &PackageOptions,
    settings: &CompileSettings,
) -> nia_build::BuildRequest {
    let mut request = nia_build::BuildRequest::new(toolchain)
        .with_forward_output(!machine_output(package, settings.diagnostics_format));
    if let Some(output) = progress::progress_observer(
        package.progress,
        package.quiet,
        package.verbose,
        settings.diagnostics_format,
    ) {
        request = request.with_progress_observer(output.observer);
        if let Some(writer) = output.process_output_writer {
            request = request.with_process_output_writer(writer);
        }
    }
    if let Some(root) = &package.root {
        request = request.with_root(root.clone());
    }
    for name in &package.packages {
        request = request.with_package(name.clone());
    }
    request = request
        .with_workspace_all(package.workspace_all)
        .with_lock_policy(nia_build::LockPolicy {
            locked: package.locked,
            offline: package.offline,
        });
    if let Some(jobs) = package.jobs {
        request = request.with_max_parallel_actions(jobs);
    }
    request
        .with_optimization(build_optimization(settings.optimization))
        .with_profile(settings.profile)
        .with_timings(settings.timings)
        .with_timing_format(settings.timing_format)
}

fn run_package_request(request: nia_build::BuildRequest, format: DiagnosticsFormat) -> ExitCode {
    match nia_build::run_build(request) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let rendered = match format {
                DiagnosticsFormat::Text => nia_build::render_build_error(&error, None, None),
                DiagnosticsFormat::Json => nia_build::render_build_error_json(&error),
            };
            write_diagnostic_report(&rendered, format);
            ExitCode::FAILURE
        }
    }
}

fn run_update(root: Option<PathBuf>, names: &[String], format: DiagnosticsFormat) -> ExitCode {
    match nia_build::run_update(root, names) {
        Ok(updates) => {
            let short = |value: &str| value.get(..12).unwrap_or(value).to_owned();
            for update in &updates {
                let label = format!("{} ({})", update.name, update.selector);
                let verb = match update.kind {
                    nia_build::LockUpdateKind::Git => "revision",
                    nia_build::LockUpdateKind::Registry => "version",
                };
                match (&update.old_value, &update.new_value) {
                    (Some(old), Some(new)) => write_stderr(format_args!(
                        "Updated {label} {verb} {} -> {}
",
                        short(old),
                        short(new)
                    )),
                    (None, Some(new)) => {
                        write_stderr(format_args!(
                            "Added {label} {verb} {}
",
                            short(new)
                        ));
                    }
                    (Some(old), None) => {
                        write_stderr(format_args!(
                            "Removed {label} {verb} {}
",
                            short(old)
                        ));
                    }
                    (None, None) => {}
                }
            }
            if updates.is_empty() {
                write_stderr(format_args!(
                    "nia.lock is up to date
"
                ));
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            let rendered = match format {
                DiagnosticsFormat::Text => nia_build::render_build_error(&error, None, None),
                DiagnosticsFormat::Json => nia_build::render_build_error_json(&error),
            };
            write_diagnostic_report(&rendered, format);
            ExitCode::FAILURE
        }
    }
}

/// Applies the stream rules of `--progress`: JSON progress makes all of
/// stderr machine-readable, so it selects JSON diagnostics and timings unless
/// the caller explicitly chose text.
fn resolve_output_policy(cli: &mut Cli) -> Result<(), CliError> {
    let (progress, help) = match &cli.command {
        CliCommand::Build { package, .. } => (package.progress, HelpTopic::Build),
        CliCommand::Test { package, .. } => (package.progress, HelpTopic::Test),
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
            cli.diagnostics_format = DiagnosticsFormat::Json;
            if cli.timings != nia_driver::TimingMode::Off {
                if cli.timing_format_explicit && cli.timing_format == TimingFormat::Text {
                    return Err(CliError::new(
                        "`--progress=json` requires `--timings-format=json` when timings are enabled",
                        help,
                    ));
                }
                cli.timing_format = TimingFormat::Json;
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

/// Resolves a file-or-directory argument used by checking and emission.
///
/// A directory names its executable entry, `main.nia`. A library has no entry
/// file of its own: check it through the file its manifest names.
fn resolve_source_entry(path: &str) -> Result<String, String> {
    let candidate = Path::new(path);
    match fs::metadata(candidate) {
        Ok(metadata) if metadata.is_file() => Ok(path.to_string()),
        Ok(metadata) if metadata.is_dir() => {
            let main = candidate.join("main.nia");
            if main.is_file() {
                return Ok(main.to_string_lossy().into_owned());
            }
            if let Some(library) = manifest_library_entry(candidate)? {
                return Ok(library);
            }
            Err(format!(
                "source directory `{path}` has no `main.nia` and no `nia.toml` with a `[library]` path"
            ))
        }
        Ok(_) => Err(format!(
            "source path `{path}` is neither a file nor a directory"
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path.to_string()),
        Err(error) => Err(format!("cannot inspect source path `{path}`: {error}")),
    }
}

/// Returns the library entry a package directory's `nia.toml` declares.
fn manifest_library_entry(directory: &Path) -> Result<Option<String>, String> {
    if !directory.join("nia.toml").is_file() {
        return Ok(None);
    }
    let manifest = nia_build::PackageManifest::read(directory).map_err(|error| {
        format!(
            "cannot read `{}`: {error}",
            directory.join("nia.toml").display()
        )
    })?;
    Ok(manifest
        .library_path
        .map(|path| directory.join(path).to_string_lossy().into_owned()))
}

/// Resolves an `-M name=path` target to the root file of the mapped tree:
/// the file itself, or the `[library]` path of a package directory.
fn resolve_module_map_source(path: &str) -> Result<String, String> {
    let candidate = Path::new(path);
    match fs::metadata(candidate) {
        Ok(metadata) if metadata.is_dir() => manifest_library_entry(candidate)?.ok_or_else(|| {
            format!("module map directory `{path}` has no `nia.toml` with a `[library]` path")
        }),
        Ok(metadata) if metadata.is_file() => Ok(path.to_string()),
        Ok(_) => Err(format!(
            "module map path `{path}` is neither a file nor a directory"
        )),
        // A path that does not exist yet keeps its spelling, so the loader
        // reports it with the importing source.
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path.to_string()),
        Err(error) => Err(format!("cannot inspect module map path `{path}`: {error}")),
    }
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

/// Reports the warnings an artifact retained, in the requested format. An
/// artifact without warnings prints nothing.
fn report_artifact_warnings(
    diagnostics: &[nia_driver::ProgramDiagnostic],
    text: impl FnOnce() -> String,
    format: DiagnosticsFormat,
) {
    if !diagnostics
        .iter()
        .any(nia_driver::ProgramDiagnostic::is_warning)
    {
        return;
    }
    let rendered = match format {
        DiagnosticsFormat::Text => text(),
        DiagnosticsFormat::Json => nia_driver::render_artifact_warnings_json(diagnostics),
    };
    write_diagnostic_report(&rendered, format);
}

fn checked_program_from_output(
    output: nia_driver::DriverOutput<nia_driver::CheckedProgram>,
    entry: &SourceEntry,
    format: DiagnosticsFormat,
) -> Option<nia_driver::CheckedProgram> {
    match output.result {
        Ok(program) => {
            report_artifact_warnings(
                &program.diagnostics,
                || {
                    nia_driver::render_program_warnings(
                        &program,
                        Some(&entry.path),
                        Some(&entry.source),
                    )
                },
                format,
            );
            Some(program)
        }
        Err(error) => {
            entry.report_driver_error(&error, format);
            None
        }
    }
}

fn codegen_program_from_output(
    output: nia_driver::DriverOutput<nia_driver::CodegenProgram>,
    entry: &SourceEntry,
    format: DiagnosticsFormat,
) -> Option<nia_driver::CodegenProgram> {
    match output.result {
        Ok(program) => {
            report_artifact_warnings(
                &program.diagnostics,
                || {
                    nia_driver::render_codegen_program_warnings(
                        &program,
                        Some(&entry.path),
                        Some(&entry.source),
                    )
                },
                format,
            );
            Some(program)
        }
        Err(error) => {
            entry.report_driver_error(&error, format);
            None
        }
    }
}

struct CheckRunOptions {
    settings: CompileSettings,
    opt_report: bool,
    runtime: RuntimeMode,
    cache_dir: Option<PathBuf>,
}

fn run_check(
    entry: &SourceEntry,
    options: CheckRunOptions,
    toolchain: Arc<nia_toolchain::ToolchainLayout>,
) -> ExitCode {
    let format = options.settings.diagnostics_format;
    let Some(runtime) = entry.resolve_runtime(options.runtime, &toolchain, format) else {
        return ExitCode::FAILURE;
    };
    let driver = nia_driver::Driver::with_config(nia_driver::DriverConfig {
        artifact_cache_dir: options.cache_dir,
        ..nia_driver::DriverConfig::new(toolchain)
    });
    let output = time_summary_stage(options.settings.timings, "check", || {
        driver.check_entry(entry.request(&options.settings, runtime.clone()))
    });
    if checked_program_from_output(output, entry, format).is_none() {
        return ExitCode::FAILURE;
    }
    if options.opt_report {
        let output = time_summary_stage(options.settings.timings, "codegen", || {
            driver.codegen(entry.request(&options.settings, runtime))
        });
        let Some(codegen) = codegen_program_from_output(output, entry, format) else {
            return ExitCode::FAILURE;
        };
        print_optimization_report(&nia_driver::optimization_report(&codegen));
    }
    ExitCode::SUCCESS
}

struct EmitContext {
    settings: CompileSettings,
    opt_report: bool,
    toolchain: Arc<nia_toolchain::ToolchainLayout>,
}

fn run_emit(entry: &SourceEntry, target: EmitTarget, context: EmitContext) -> ExitCode {
    let format = context.settings.diagnostics_format;
    match target {
        EmitTarget::Tokens => time_summary_stage(context.settings.timings, "lex", || {
            write_stdout(format_args!(
                "{}",
                nia_driver::tokens_inspection(&entry.source).text
            ))
        }),
        EmitTarget::Ast => time_summary_stage(context.settings.timings, "parse", || {
            run_parse(entry, format)
        }),
        EmitTarget::Checked { runtime } => run_emit_checked(entry, runtime, context),
        EmitTarget::Backend { runtime } => run_emit_backend(entry, runtime, context),
        EmitTarget::Llvm { runtime } => run_emit_llvm(entry, runtime, context),
        EmitTarget::Obj(options) => run_emit_obj(entry, options, context),
        EmitTarget::Exe(options) => run_emit_exe(entry, *options, context),
    }
}

fn run_parse(entry: &SourceEntry, format: DiagnosticsFormat) -> ExitCode {
    let inspection = nia_driver::ast_inspection(&entry.source);
    let status = write_stdout(format_args!("{}", inspection.text));
    if status != ExitCode::SUCCESS {
        return status;
    }
    if !inspection.parse_errors.is_empty() {
        write_diagnostic_report(
            &render_parse_failure(&entry.path, &entry.source, &inspection.parse_errors, format),
            format,
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run_emit_checked(entry: &SourceEntry, runtime: RuntimeMode, context: EmitContext) -> ExitCode {
    let format = context.settings.diagnostics_format;
    let Some(runtime) = entry.resolve_runtime(runtime, &context.toolchain, format) else {
        return ExitCode::FAILURE;
    };
    let driver = nia_driver::Driver::new(context.toolchain);
    let output = time_summary_stage(context.settings.timings, "check", || {
        driver.check_entry(entry.request(&context.settings, runtime))
    });
    match checked_program_from_output(output, entry, format) {
        Some(program) => write_stdout(format_args!("{program:#?}\n")),
        None => ExitCode::FAILURE,
    }
}

fn run_emit_backend(entry: &SourceEntry, runtime: RuntimeMode, context: EmitContext) -> ExitCode {
    let format = context.settings.diagnostics_format;
    let Some(runtime) = entry.resolve_runtime(runtime, &context.toolchain, format) else {
        return ExitCode::FAILURE;
    };
    let driver = nia_driver::Driver::new(context.toolchain);
    let output = time_summary_stage(context.settings.timings, "codegen", || {
        driver.codegen(entry.request(&context.settings, runtime))
    });
    let Some(program) = codegen_program_from_output(output, entry, format) else {
        return ExitCode::FAILURE;
    };
    if context.opt_report {
        print_optimization_report(&nia_driver::optimization_report(&program));
    }
    write_stdout(format_args!("{:#?}\n", program.backend_lowering.program))
}

fn run_emit_llvm(entry: &SourceEntry, runtime: RuntimeMode, context: EmitContext) -> ExitCode {
    let format = context.settings.diagnostics_format;
    let Some(runtime) = entry.resolve_runtime(runtime, &context.toolchain, format) else {
        return ExitCode::FAILURE;
    };
    let driver = nia_driver::Driver::new(context.toolchain);
    let output = time_summary_stage(context.settings.timings, "emit_llvm_ir", || {
        driver.emit_llvm_ir(nia_driver::EmitLlvmRequest::new(
            entry.request(&context.settings, runtime),
        ))
    });
    let artifact = match output.result {
        Ok(artifact) => artifact,
        Err(error) => {
            entry.report_driver_error(&error, format);
            return ExitCode::FAILURE;
        }
    };
    report_artifact_warnings(
        &artifact.diagnostics,
        || nia_driver::render_llvm_ir_warnings(&artifact, Some(&entry.path), Some(&entry.source)),
        format,
    );
    if context.opt_report {
        print_optimization_report(&nia_driver::llvm_ir_optimization_report(&artifact));
    }
    for module in artifact.modules {
        let status = write_stdout(format_args!("{}", module.ir));
        if status != ExitCode::SUCCESS {
            return status;
        }
    }
    ExitCode::SUCCESS
}

fn run_emit_obj(entry: &SourceEntry, options: EmitObjOptions, context: EmitContext) -> ExitCode {
    let format = context.settings.diagnostics_format;
    let Some(runtime) = entry.resolve_runtime(options.runtime, &context.toolchain, format) else {
        return ExitCode::FAILURE;
    };
    let driver = nia_driver::Driver::with_config(nia_driver::DriverConfig {
        artifact_cache_dir: options.cache_dir,
        ..nia_driver::DriverConfig::new(context.toolchain)
    });
    let output = time_summary_stage(context.settings.timings, "emit_native_objects", || {
        driver.emit_native_objects(nia_driver::EmitObjectRequest::new(
            entry.request(&context.settings, runtime),
        ))
    });
    let objects = match output.result {
        Ok(objects) => objects,
        Err(error) => {
            entry.report_driver_error(&error, format);
            return ExitCode::FAILURE;
        }
    };
    report_artifact_warnings(
        &objects.diagnostics,
        || nia_driver::render_object_warnings(&objects, Some(&entry.path), Some(&entry.source)),
        format,
    );
    if context.opt_report {
        print_optimization_report(&nia_driver::object_optimization_report(&objects));
    }
    let output_path = match options.output {
        ObjectOutput::Single(path) => nia_driver::ObjectOutput::Single(
            path.unwrap_or_else(|| default_output_path(&entry.path, "o")),
        ),
        ObjectOutput::Directory(path) => nia_driver::ObjectOutput::Directory(path),
    };
    match driver
        .write_native_objects_from_artifact(&objects, output_path)
        .result
    {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            entry.report_driver_error(&error, format);
            ExitCode::FAILURE
        }
    }
}

fn run_emit_exe(entry: &SourceEntry, options: EmitExeOptions, context: EmitContext) -> ExitCode {
    let format = context.settings.diagnostics_format;
    let Some(runtime) =
        entry.resolve_runtime(RuntimeMode::Freestanding, &context.toolchain, format)
    else {
        return ExitCode::FAILURE;
    };
    let output_path = options.output.unwrap_or_else(|| {
        default_output_path(
            &entry.path,
            context.toolchain.artifact_target().executable_extension(),
        )
    });
    let driver = nia_driver::Driver::with_config(nia_driver::DriverConfig {
        artifact_cache_dir: options.cache_dir,
        ..nia_driver::DriverConfig::new(context.toolchain)
    });
    let output = time_summary_stage(context.settings.timings, "link_executable", || {
        driver.link_executable(nia_driver::LinkExecutableRequest {
            check: entry.request(&context.settings, runtime),
            output: output_path,
            link_options: options.link_options,
            link_time_optimization: options.link_time_optimization,
        })
    });
    let executable = match output.result {
        Ok(executable) => executable,
        Err(error) => {
            entry.report_driver_error(&error, format);
            return ExitCode::FAILURE;
        }
    };
    report_artifact_warnings(
        &executable.diagnostics,
        || {
            nia_driver::render_executable_warnings(
                &executable,
                Some(&entry.path),
                Some(&entry.source),
            )
        },
        format,
    );
    if context.opt_report {
        print_optimization_report(&nia_driver::optimization_report_from_parts(
            executable.optimization,
            &executable.optimization_report,
        ));
    }
    ExitCode::SUCCESS
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

fn default_output_path(source: &str, extension: &str) -> PathBuf {
    let mut path = PathBuf::from(source);
    path.set_extension(extension);
    path
}

/// The optimization report is diagnostic output, so every command writes it
/// to stderr and leaves stdout to the requested product.
fn print_optimization_report(report: &str) {
    write_stderr(format_args!("{report}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_library_package(root: &Path) -> PathBuf {
        fs::create_dir_all(root.join("src")).expect("create sources");
        fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"math\"\nversion = \"1.0.0\"\n[library]\npath = \"src/math.nia\"\n",
        )
        .expect("write manifest");
        let library = root.join("src/math.nia");
        fs::write(&library, "").expect("write library");
        library
    }

    #[test]
    fn source_directory_prefers_main_then_manifest_library() {
        let root = nia_test_support::test_dir("source-directory-entry");
        let library = write_library_package(&root);
        assert_eq!(
            resolve_source_entry(root.to_string_lossy().as_ref()).expect("library entry"),
            library.to_string_lossy()
        );
        let main = root.join("main.nia");
        fs::write(&main, "").expect("write main");
        assert_eq!(
            resolve_source_entry(root.to_string_lossy().as_ref()).expect("main entry"),
            main.to_string_lossy()
        );
    }

    #[test]
    fn source_directory_without_an_entry_is_an_error() {
        let root = nia_test_support::test_dir("source-directory-without-entry");
        // A file named like the old package root carries no meaning.
        fs::write(root.join("pkg.nia"), "").expect("write unrelated file");
        let error = resolve_source_entry(root.to_string_lossy().as_ref())
            .expect_err("directory without an entry");
        assert!(error.contains("no `main.nia`"), "{error}");
    }

    #[test]
    fn module_map_directory_names_its_manifest_library() {
        let root = nia_test_support::test_dir("module-map-directory");
        let library = write_library_package(&root);
        assert_eq!(
            resolve_module_map_source(root.to_string_lossy().as_ref()).expect("library"),
            library.to_string_lossy()
        );
        let empty = nia_test_support::test_dir("module-map-directory-without-library");
        let error = resolve_module_map_source(empty.to_string_lossy().as_ref())
            .expect_err("directory without a library");
        assert!(error.contains("`[library]` path"), "{error}");
        assert_eq!(
            resolve_module_map_source("vendor/math.nia").expect("future file"),
            "vendor/math.nia"
        );
    }

    #[test]
    fn json_progress_resolves_machine_formats_before_running() {
        let mut cli = match cli::parse_cli(
            ["--timings", "build", "--progress=json"]
                .into_iter()
                .map(ToString::to_string)
                .collect(),
        )
        .expect("parse JSON progress")
        {
            CliAction::Run(cli) => *cli,
            _ => panic!("expected runnable command"),
        };
        resolve_output_policy(&mut cli).expect("resolve JSON stream formats");
        assert_eq!(cli.diagnostics_format, DiagnosticsFormat::Json);
        assert_eq!(cli.timing_format, TimingFormat::Json);

        let mut cli = match cli::parse_cli(
            ["--diagnostics-format=text", "build", "--progress=json"]
                .into_iter()
                .map(ToString::to_string)
                .collect(),
        )
        .expect("parse explicit text diagnostics")
        {
            CliAction::Run(cli) => *cli,
            _ => panic!("expected runnable command"),
        };
        let error = resolve_output_policy(&mut cli).expect_err("reject mixed stderr formats");
        assert!(error.message.contains("machine-readable"));
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
}
