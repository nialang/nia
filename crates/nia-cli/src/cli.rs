// SPDX-License-Identifier: GPL-3.0-or-later
//! Turns scanned arguments into a typed invocation.
//!
//! [`crate::args`] owns how options are spelled; this module owns what they
//! mean for each command. Global options may appear before or after the
//! command and are scanned in the same pass as the command's own options, so
//! a value can never be taken by another option.

use std::{num::NonZeroUsize, path::PathBuf};

use nia_driver::{ModuleMap, NiaOptimizationLevel, SourcePath};
use nia_target_config::BuildProfile;
use nia_timing::{TimingFormat, TimingOptions, TimingTrace};

use crate::args::{self, EmitKind, Occurrence, OptionId, ScanError, Scanned, Scope};

/// Process exit status for a malformed invocation, as distinct from a failed
/// check or build.
pub(crate) const USAGE_EXIT_CODE: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HelpTopic {
    Main,
    Build,
    Update,
    Clean,
    Check,
    Emit,
    EmitKind(EmitKind),
}

impl HelpTopic {
    pub(crate) fn scope(self) -> Scope {
        match self {
            Self::Main => Scope::GLOBAL,
            Self::Build => Scope::BUILD,
            Self::Update => Scope::UPDATE,
            Self::Clean => Scope::CLEAN,
            Self::Check => Scope::CHECK,
            Self::Emit => Scope::EMIT_ANY,
            Self::EmitKind(kind) => kind.scope(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiagnosticsFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProgressMode {
    Auto,
    Plain,
    Json,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeMode {
    Bare,
    Freestanding,
}

#[derive(Debug)]
pub(crate) struct Cli {
    pub(crate) resource_root: Option<PathBuf>,
    pub(crate) target: Option<nia_target::TargetConfig>,
    pub(crate) module_map: ModuleMap,
    pub(crate) optimization: NiaOptimizationLevel,
    pub(crate) profile: BuildProfile,
    pub(crate) timings: nia_driver::TimingMode,
    pub(crate) timing_trace: TimingTrace,
    pub(crate) timing_format: TimingFormat,
    pub(crate) timing_format_explicit: bool,
    pub(crate) diagnostics_format: DiagnosticsFormat,
    pub(crate) diagnostics_format_explicit: bool,
    pub(crate) command: CliCommand,
}

impl Cli {
    pub(crate) fn timing_options(&self) -> TimingOptions {
        TimingOptions::new(self.timings)
            .with_trace(self.timing_trace)
            .with_format(self.timing_format)
    }
}

/// Package selection and progress options of `nia build`.
#[derive(Debug)]
pub(crate) struct PackageOptions {
    pub(crate) root: Option<PathBuf>,
    pub(crate) packages: Vec<String>,
    pub(crate) workspace_all: bool,
    pub(crate) locked: bool,
    pub(crate) offline: bool,
    pub(crate) jobs: Option<NonZeroUsize>,
    pub(crate) quiet: u8,
    pub(crate) verbose: u8,
    pub(crate) progress: ProgressMode,
}

#[derive(Debug)]
pub(crate) enum CliCommand {
    Build {
        package: PackageOptions,
        step: Option<String>,
        link_time_optimization: nia_driver::LinkTimeOptimization,
        dry_run: bool,
        explain: bool,
        print_plan: bool,
        print_cache: bool,
        print_observations: bool,
        list: bool,
        keep_going: bool,
    },
    Update {
        root: Option<PathBuf>,
        names: Vec<String>,
    },
    Clean {
        root: Option<PathBuf>,
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

#[derive(Debug)]
pub(crate) enum EmitTarget {
    Tokens,
    Ast,
    Checked { runtime: RuntimeMode },
    Backend { runtime: RuntimeMode },
    Llvm { runtime: RuntimeMode },
    Obj(EmitObjOptions),
    Exe(Box<EmitExeOptions>),
}

#[derive(Debug)]
pub(crate) struct EmitObjOptions {
    pub(crate) output: ObjectOutput,
    pub(crate) runtime: RuntimeMode,
    pub(crate) cache_dir: Option<PathBuf>,
}

#[derive(Debug)]
pub(crate) enum ObjectOutput {
    /// Write one object; `None` derives the path from the source.
    Single(Option<PathBuf>),
    Directory(PathBuf),
}

#[derive(Debug)]
pub(crate) struct EmitExeOptions {
    /// `None` derives the path from the source and artifact target.
    pub(crate) output: Option<PathBuf>,
    pub(crate) cache_dir: Option<PathBuf>,
    pub(crate) link_options: nia_linker::LinkOptions,
    pub(crate) link_time_optimization: nia_driver::LinkTimeOptimization,
}

#[derive(Debug)]
pub(crate) enum CliAction {
    Help(HelpTopic),
    Version,
    Run(Box<Cli>),
}

#[derive(Debug)]
pub(crate) struct CliError {
    pub(crate) message: String,
    pub(crate) help: HelpTopic,
}

impl CliError {
    pub(crate) fn new(message: impl Into<String>, help: HelpTopic) -> Self {
        Self {
            message: message.into(),
            help,
        }
    }
}

type CliResult<T> = Result<T, CliError>;

/// The diagnostics format an invocation asked for, decided by the same rule
/// as the full parse (last occurrence wins). Used to report a parse failure
/// in the format the caller expects even when the invocation is malformed.
pub(crate) fn requested_diagnostics_format(args: &[String]) -> DiagnosticsFormat {
    let mut format = DiagnosticsFormat::Text;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            break;
        }
        let value = match arg.strip_prefix("--diagnostics-format") {
            Some("") => iter.next().map(String::as_str),
            Some(rest) => rest.strip_prefix('='),
            None => None,
        };
        match value {
            Some("json") => format = DiagnosticsFormat::Json,
            Some("text") => format = DiagnosticsFormat::Text,
            _ => {}
        }
    }
    format
}

pub(crate) fn parse_cli(args: Vec<String>) -> CliResult<CliAction> {
    // The command is the first argument that is not an option or an option
    // value, so global options may precede it.
    let (command_index, prefix) = locate_command(&args)?;
    let Some(command_index) = command_index else {
        if prefix.has(OptionId::Version) {
            return Ok(CliAction::Version);
        }
        if args.is_empty() || prefix.has(OptionId::Help) {
            return Ok(CliAction::Help(HelpTopic::Main));
        }
        return Err(CliError::new("missing command", HelpTopic::Main));
    };
    if prefix.has(OptionId::Version) {
        return Ok(CliAction::Version);
    }
    let command = args[command_index].as_str();
    let rest = &args[command_index + 1..];
    let topic = match command {
        "help" => return help_topic(rest).map(CliAction::Help),
        "build" => HelpTopic::Build,
        "update" => HelpTopic::Update,
        "clean" => HelpTopic::Clean,
        "check" => HelpTopic::Check,
        "emit" => HelpTopic::Emit,
        _ => {
            return Err(CliError::new(
                format!("unknown command `{command}`"),
                HelpTopic::Main,
            ));
        }
    };
    let topic = match topic {
        HelpTopic::Emit => emit_topic(rest)?,
        topic => topic,
    };
    let scanned =
        args::scan_from(rest, topic.scope(), prefix).map_err(|error| scan_error(error, topic))?;
    if scanned.has(OptionId::Help) {
        return Ok(CliAction::Help(topic));
    }
    if scanned.has(OptionId::Version) {
        return Ok(CliAction::Version);
    }
    let global = GlobalOptions::from_scanned(&scanned, topic)?;
    let command = match topic {
        HelpTopic::Build => parse_build(&scanned)?,
        HelpTopic::Update => parse_update(&scanned),
        HelpTopic::Clean => parse_clean(&scanned)?,
        HelpTopic::Check => parse_check(&scanned)?,
        HelpTopic::Emit => {
            if rest.is_empty() {
                return Ok(CliAction::Help(HelpTopic::Emit));
            }
            return Err(CliError::new(
                "missing emit target flag; expected one of --tokens, --ast, --checked, --backend, --llvm, --obj, or --exe",
                HelpTopic::Emit,
            ));
        }
        HelpTopic::EmitKind(kind) => parse_emit(&scanned, kind)?,
        HelpTopic::Main => unreachable!("the main topic is not a command"),
    };
    Ok(CliAction::Run(Box::new(global.into_cli(command))))
}

/// Scans the global options before the command and returns the command's
/// position, if any.
fn locate_command(args: &[String]) -> CliResult<(Option<usize>, Scanned)> {
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if !arg.starts_with('-') || arg == "-" {
            break;
        }
        if arg == "--" {
            return Err(CliError::new("`--` must follow a command", HelpTopic::Main));
        }
        // A value-taking global option consumes the next argument too.
        let width = match args::scan(&args[index..=index], Scope::GLOBAL) {
            Ok(_) => 1,
            Err(ScanError::MissingValue { .. }) if index + 1 < args.len() => 2,
            Err(ScanError::OutOfScope { option }) => {
                return Err(CliError::new(
                    format!("`{option}` must follow the command it belongs to"),
                    HelpTopic::Main,
                ));
            }
            Err(error) => return Err(scan_error(error, HelpTopic::Main)),
        };
        index += width;
    }
    let prefix = args::scan(&args[..index], Scope::GLOBAL)
        .map_err(|error| scan_error(error, HelpTopic::Main))?;
    Ok(((index < args.len()).then_some(index), prefix))
}

fn scan_error(error: ScanError, topic: HelpTopic) -> CliError {
    let message = match &error {
        ScanError::OutOfScope { option } => match owner_of(option) {
            Some(owner) => format!(
                "`{option}` is not an option of {}; it belongs to {owner}",
                topic_name(topic)
            ),
            None => format!("`{option}` is not an option of {}", topic_name(topic)),
        },
        ScanError::Unknown { option } if topic == HelpTopic::Main => {
            format!("unknown option `{option}`")
        }
        ScanError::Unknown { option } => {
            format!("unknown {} option `{option}`", topic_name(topic))
        }
        _ => error.to_string(),
    };
    CliError::new(message, topic)
}

fn topic_name(topic: HelpTopic) -> String {
    match topic {
        HelpTopic::Main => "`nia`".to_string(),
        HelpTopic::Build => "`nia build`".to_string(),
        HelpTopic::Update => "`nia update`".to_string(),
        HelpTopic::Clean => "`nia clean`".to_string(),
        HelpTopic::Check => "`nia check`".to_string(),
        HelpTopic::Emit => "`nia emit`".to_string(),
        HelpTopic::EmitKind(kind) => format!("`nia emit {}`", kind.flag()),
    }
}

/// Names the commands an out-of-scope option does belong to.
fn owner_of(option: &str) -> Option<String> {
    let spec = args::OPTIONS
        .iter()
        .find(|spec| spec.names.contains(&option))?;
    let mut owners = Vec::new();
    for (scope, name) in [
        (Scope::BUILD, "`nia build`".to_string()),
        (Scope::CLEAN, "`nia clean`".to_string()),
        (Scope::CHECK, "`nia check`".to_string()),
    ] {
        if spec.scope.contains(scope) {
            owners.push(name);
        }
    }
    for kind in EmitKind::ALL {
        if spec.scope.contains(kind.scope()) {
            owners.push(format!("`nia emit {}`", kind.flag()));
        }
    }
    (!owners.is_empty()).then(|| owners.join(", "))
}

/// Finds the emit kind before the full scan, so the scan can apply the
/// kind's own options.
fn emit_topic(args: &[String]) -> CliResult<HelpTopic> {
    let mut kind = None;
    for arg in args {
        if arg == "--" {
            break;
        }
        if let Some(found) = EmitKind::from_flag(arg) {
            if kind.is_some_and(|kind| kind != found) {
                return Err(CliError::new(
                    "use exactly one emit target flag",
                    HelpTopic::Emit,
                ));
            }
            kind = Some(found);
        }
    }
    if let Some(kind) = kind {
        return Ok(HelpTopic::EmitKind(kind));
    }
    if let Some(name) = args.iter().find_map(|arg| EmitKind::from_bare_name(arg)) {
        return Err(CliError::new(
            format!("emit targets are flags; use `{}`", name.flag()),
            HelpTopic::Emit,
        ));
    }
    Ok(HelpTopic::Emit)
}

fn help_topic(args: &[String]) -> CliResult<HelpTopic> {
    let words = args
        .iter()
        .filter(|arg| !matches!(arg.as_str(), "-h" | "--help"))
        .map(String::as_str)
        .collect::<Vec<_>>();
    let unknown =
        |word: &str, topic: HelpTopic| CliError::new(format!("unknown help topic `{word}`"), topic);
    match words.as_slice() {
        [] => Ok(HelpTopic::Main),
        ["build"] => Ok(HelpTopic::Build),
        ["update"] => Ok(HelpTopic::Update),
        ["clean"] => Ok(HelpTopic::Clean),
        ["check"] => Ok(HelpTopic::Check),
        ["emit"] => Ok(HelpTopic::Emit),
        ["emit", target] => match EmitKind::from_flag(target) {
            Some(kind) => Ok(HelpTopic::EmitKind(kind)),
            None => match EmitKind::from_bare_name(target) {
                Some(kind) => Err(CliError::new(
                    format!(
                        "emit targets are flags; use `nia help emit {}`",
                        kind.flag()
                    ),
                    HelpTopic::Emit,
                )),
                None => Err(CliError::new(
                    format!("unknown emit target `{target}`"),
                    HelpTopic::Emit,
                )),
            },
        },
        [command @ ("build" | "check" | "clean"), extra, ..] => {
            let topic = help_topic(&[(*command).to_string()])?;
            Err(unknown(extra, topic))
        }
        ["emit", _, extra, ..] => Err(unknown(extra, HelpTopic::Emit)),
        [topic, ..] => Err(unknown(topic, HelpTopic::Main)),
    }
}

struct GlobalOptions {
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
}

impl GlobalOptions {
    fn from_scanned(scanned: &Scanned, topic: HelpTopic) -> CliResult<Self> {
        let err = |message: String| CliError::new(message, topic);
        let mut module_map = ModuleMap::new();
        for occurrence in scanned.all(OptionId::Module) {
            insert_module_map_entry(&mut module_map, value(occurrence)).map_err(err)?;
        }
        // The profile is the last of `--debug`, `--release`, and `--profile`.
        let mut profile = BuildProfile::default();
        for occurrence in &scanned.options {
            profile = match occurrence.id {
                OptionId::Debug => BuildProfile::Debug,
                OptionId::Release => BuildProfile::Release,
                OptionId::Profile => parse_profile(value(occurrence)).map_err(err)?,
                _ => continue,
            };
        }
        let optimization = match scanned.last(OptionId::Optimization) {
            Some(occurrence) => parse_optimization(occurrence.value.as_deref()).map_err(err)?,
            None => default_profile_optimization(profile),
        };
        let timings = match scanned.last(OptionId::Timings) {
            Some(occurrence) => parse_timings(occurrence.value.as_deref()).map_err(err)?,
            None => nia_driver::TimingMode::Off,
        };
        let timing_trace = match scanned.last(OptionId::TimingTrace) {
            Some(occurrence) => parse_timing_trace(value(occurrence)).map_err(err)?,
            None => TimingTrace::Off,
        };
        let timing_format = match scanned.last(OptionId::TimingsFormat) {
            Some(occurrence) => parse_text_or_json(value(occurrence), "timings format")
                .map(|json| {
                    if json {
                        TimingFormat::Json
                    } else {
                        TimingFormat::Text
                    }
                })
                .map_err(err)?,
            None => TimingFormat::Text,
        };
        let diagnostics_format = match scanned.last(OptionId::DiagnosticsFormat) {
            Some(occurrence) => parse_text_or_json(value(occurrence), "diagnostics format")
                .map(|json| {
                    if json {
                        DiagnosticsFormat::Json
                    } else {
                        DiagnosticsFormat::Text
                    }
                })
                .map_err(err)?,
            None => DiagnosticsFormat::Text,
        };
        let target = scanned
            .last(OptionId::Target)
            .map(|occurrence| {
                nia_target::TargetConfig::parse(value(occurrence))
                    .map_err(|error| err(error.to_string()))
            })
            .transpose()?;
        Ok(Self {
            resource_root: scanned
                .last(OptionId::ResourceRoot)
                .map(|occurrence| PathBuf::from(value(occurrence))),
            target,
            module_map,
            optimization,
            profile,
            timings,
            timing_trace,
            timing_format,
            timing_format_explicit: scanned.has(OptionId::TimingsFormat),
            diagnostics_format,
            diagnostics_format_explicit: scanned.has(OptionId::DiagnosticsFormat),
        })
    }

    fn into_cli(self, command: CliCommand) -> Cli {
        Cli {
            resource_root: self.resource_root,
            target: self.target,
            module_map: self.module_map,
            optimization: self.optimization,
            profile: self.profile,
            timings: self.timings,
            timing_trace: self.timing_trace,
            timing_format: self.timing_format,
            timing_format_explicit: self.timing_format_explicit,
            diagnostics_format: self.diagnostics_format,
            diagnostics_format_explicit: self.diagnostics_format_explicit,
            command,
        }
    }
}

/// A value-taking option's value; the scanner guarantees one is present.
fn value(occurrence: &Occurrence) -> &str {
    occurrence
        .value
        .as_deref()
        .expect("value-taking options carry a value")
}

fn parse_package_options(scanned: &Scanned, topic: HelpTopic) -> CliResult<PackageOptions> {
    let err = |message: String| CliError::new(message, topic);
    if scanned.has(OptionId::Workspace) && scanned.has(OptionId::Package) {
        return Err(err(
            "`--workspace` cannot be combined with `--package`".to_string()
        ));
    }
    let jobs = scanned
        .last(OptionId::Jobs)
        .map(|occurrence| parse_jobs(value(occurrence)).map_err(err))
        .transpose()?;
    let progress = match scanned.last(OptionId::Progress) {
        Some(occurrence) => parse_progress_mode(value(occurrence)).map_err(err)?,
        None => ProgressMode::Auto,
    };
    let quiet = scanned.count(OptionId::Quiet);
    let verbose = scanned.count(OptionId::Verbose);
    if quiet > 0 && verbose > 0 {
        return Err(err(
            "`--quiet` and `--verbose` cannot be used together".to_string()
        ));
    }
    if verbose > 3 {
        return Err(err("verbosity may not exceed `-vvv`".to_string()));
    }
    if quiet > 0 && progress == ProgressMode::Json {
        return Err(err(
            "`--quiet` cannot be combined with `--progress=json`".to_string()
        ));
    }
    Ok(PackageOptions {
        root: scanned
            .last(OptionId::Root)
            .map(|occurrence| PathBuf::from(value(occurrence))),
        packages: scanned
            .all(OptionId::Package)
            .map(|occurrence| value(occurrence).to_owned())
            .collect(),
        workspace_all: scanned.has(OptionId::Workspace),
        locked: scanned.has(OptionId::Locked),
        offline: scanned.has(OptionId::Offline),
        jobs,
        quiet: u8::from(quiet > 0),
        verbose: verbose as u8,
        progress,
    })
}

fn parse_build(scanned: &Scanned) -> CliResult<CliCommand> {
    let topic = HelpTopic::Build;
    let package = parse_package_options(scanned, topic)?;
    let step = match scanned.positionals.as_slice() {
        [] => None,
        [step] if !scanned.ended_options && looks_like_path_argument(step) => {
            return Err(CliError::new(
                format!("package path `{step}` must be passed with `--root`"),
                topic,
            ));
        }
        [step] => Some(step.clone()),
        [_, extra, ..] => {
            return Err(CliError::new(
                format!("unexpected argument `{extra}` for `nia build`"),
                topic,
            ));
        }
    };
    let link_time_optimization = match scanned.last(OptionId::Lto) {
        Some(occurrence) => parse_link_time_optimization(value(occurrence))
            .map_err(|message| CliError::new(message, topic))?,
        None => nia_driver::LinkTimeOptimization::Off,
    };
    Ok(CliCommand::Build {
        package,
        step,
        link_time_optimization,
        dry_run: scanned.has(OptionId::DryRun),
        explain: scanned.has(OptionId::Explain),
        print_plan: scanned.has(OptionId::PrintPlan),
        print_cache: scanned.has(OptionId::PrintCache),
        print_observations: scanned.has(OptionId::PrintObservations),
        list: scanned.has(OptionId::List),
        keep_going: scanned.has(OptionId::KeepGoing),
    })
}

fn parse_update(scanned: &Scanned) -> CliCommand {
    CliCommand::Update {
        root: scanned
            .last(OptionId::Root)
            .map(|occurrence| PathBuf::from(value(occurrence))),
        names: scanned.positionals.clone(),
    }
}

fn parse_clean(scanned: &Scanned) -> CliResult<CliCommand> {
    if let Some(argument) = scanned.positionals.first() {
        return Err(CliError::new(
            format!("unexpected argument `{argument}` for `nia clean`"),
            HelpTopic::Clean,
        ));
    }
    Ok(CliCommand::Clean {
        root: scanned
            .last(OptionId::Root)
            .map(|occurrence| PathBuf::from(value(occurrence))),
    })
}

fn single_path(scanned: &Scanned, topic: HelpTopic) -> CliResult<String> {
    match scanned.positionals.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err(CliError::new(
            format!("missing source path for {}", topic_name(topic)),
            topic,
        )),
        [_, extra, ..] => Err(CliError::new(
            format!("unexpected argument `{extra}` for {}", topic_name(topic)),
            topic,
        )),
    }
}

fn runtime(scanned: &Scanned, default: RuntimeMode, topic: HelpTopic) -> CliResult<RuntimeMode> {
    match scanned.last(OptionId::Runtime) {
        Some(occurrence) => {
            parse_runtime(value(occurrence)).map_err(|message| CliError::new(message, topic))
        }
        None => Ok(default),
    }
}

fn cache_dir(scanned: &Scanned) -> Option<PathBuf> {
    scanned
        .last(OptionId::CacheDir)
        .map(|occurrence| PathBuf::from(value(occurrence)))
}

fn parse_check(scanned: &Scanned) -> CliResult<CliCommand> {
    let topic = HelpTopic::Check;
    Ok(CliCommand::Check {
        path: single_path(scanned, topic)?,
        opt_report: scanned.has(OptionId::OptReport),
        runtime: runtime(scanned, RuntimeMode::Bare, topic)?,
        cache_dir: cache_dir(scanned),
    })
}

fn parse_emit(scanned: &Scanned, kind: EmitKind) -> CliResult<CliCommand> {
    let topic = HelpTopic::EmitKind(kind);
    let path = single_path(scanned, topic)?;
    let target = match kind {
        EmitKind::Tokens => EmitTarget::Tokens,
        EmitKind::Ast => EmitTarget::Ast,
        EmitKind::Checked => EmitTarget::Checked {
            runtime: runtime(scanned, RuntimeMode::Bare, topic)?,
        },
        EmitKind::Backend => EmitTarget::Backend {
            runtime: runtime(scanned, RuntimeMode::Bare, topic)?,
        },
        EmitKind::Llvm => EmitTarget::Llvm {
            runtime: runtime(scanned, RuntimeMode::Bare, topic)?,
        },
        EmitKind::Obj => EmitTarget::Obj(parse_emit_obj(scanned, topic)?),
        EmitKind::Exe => EmitTarget::Exe(Box::new(parse_emit_exe(scanned, topic)?)),
    };
    Ok(CliCommand::Emit {
        path,
        target,
        opt_report: scanned.has(OptionId::OptReport),
    })
}

fn parse_emit_obj(scanned: &Scanned, topic: HelpTopic) -> CliResult<EmitObjOptions> {
    let output = match (
        scanned.last(OptionId::Output),
        scanned.last(OptionId::OutDir),
    ) {
        (Some(_), Some(_)) => {
            return Err(CliError::new(
                "use either `-o` or `--out-dir`, not both",
                topic,
            ));
        }
        (Some(output), None) => ObjectOutput::Single(Some(PathBuf::from(value(output)))),
        (None, Some(directory)) => ObjectOutput::Directory(PathBuf::from(value(directory))),
        (None, None) => ObjectOutput::Single(None),
    };
    Ok(EmitObjOptions {
        output,
        runtime: runtime(scanned, RuntimeMode::Bare, topic)?,
        cache_dir: cache_dir(scanned),
    })
}

fn parse_emit_exe(scanned: &Scanned, topic: HelpTopic) -> CliResult<EmitExeOptions> {
    let err = |message: String| CliError::new(message, topic);
    if runtime(scanned, RuntimeMode::Freestanding, topic)? != RuntimeMode::Freestanding {
        return Err(err(
            "`nia emit --exe` supports only `--runtime freestanding`".to_string(),
        ));
    }
    let mut link_options = nia_linker::LinkOptions::default();
    let mut explicit_linker_program = false;
    let mut form = None::<nia_linker::ExecutableForm>;
    let mut interpreter = None::<String>;
    // Accumulating link options keep their command-line order.
    for occurrence in &scanned.options {
        let text = || value(occurrence).to_string();
        match occurrence.id {
            OptionId::LinkArg => link_options.raw_args.push(text()),
            OptionId::LibraryPath => link_options = link_options.add_library_path(text()),
            OptionId::Library => link_options = link_options.add_library(text()),
            OptionId::Rpath => link_options = link_options.add_rpath(text()),
            OptionId::Executable => {
                form = Some(parse_executable_form(value(occurrence)).map_err(err)?)
            }
            OptionId::Interpreter => interpreter = Some(text()),
            OptionId::Linker => {
                link_options.linker.program = text();
                explicit_linker_program = true;
            }
            OptionId::LinkerFlavor => {
                let flavor = parse_linker_flavor(value(occurrence)).map_err(err)?;
                select_linker_flavor(&mut link_options.linker, flavor, explicit_linker_program);
            }
            _ => {}
        }
    }
    // An interpreter exists only for a dynamic executable, whatever the order
    // in which the options appear.
    link_options.form = match (form, interpreter) {
        (Some(nia_linker::ExecutableForm::Dynamic(_)), Some(path)) => Some(
            nia_linker::ExecutableForm::Dynamic(nia_linker::Interpreter::Path(path)),
        ),
        (_, Some(_)) => {
            return Err(err(
                "`--interpreter` requires `--executable dynamic`".to_string()
            ));
        }
        (form, None) => form,
    };
    let link_time_optimization = match scanned.last(OptionId::Lto) {
        Some(occurrence) => parse_link_time_optimization(value(occurrence)).map_err(err)?,
        None => nia_driver::LinkTimeOptimization::Off,
    };
    Ok(EmitExeOptions {
        output: scanned
            .last(OptionId::Output)
            .map(|occurrence| PathBuf::from(value(occurrence))),
        cache_dir: cache_dir(scanned),
        link_options,
        link_time_optimization,
    })
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

fn looks_like_path_argument(arg: &str) -> bool {
    matches!(arg, "." | "..") || arg.contains('/') || arg.contains('\\')
}

fn parse_jobs(value: &str) -> Result<NonZeroUsize, String> {
    value
        .parse::<usize>()
        .ok()
        .and_then(NonZeroUsize::new)
        .ok_or_else(|| format!("invalid job count `{value}`; expected a positive integer"))
}

fn parse_progress_mode(value: &str) -> Result<ProgressMode, String> {
    match value {
        "auto" => Ok(ProgressMode::Auto),
        "plain" => Ok(ProgressMode::Plain),
        "none" => Ok(ProgressMode::None),
        "json" => Ok(ProgressMode::Json),
        _ => Err(format!(
            "invalid progress mode `{value}`; expected auto, plain, json, or none"
        )),
    }
}

fn parse_optimization(level: Option<&str>) -> Result<NiaOptimizationLevel, String> {
    match level {
        None | Some("2") => Ok(NiaOptimizationLevel::O2),
        Some("0") => Ok(NiaOptimizationLevel::O0),
        Some("1") => Ok(NiaOptimizationLevel::O1),
        Some("3") => Ok(NiaOptimizationLevel::O3),
        Some("s") => Ok(NiaOptimizationLevel::Os),
        Some("z") => Ok(NiaOptimizationLevel::Oz),
        Some(level) => Err(format!(
            "unknown optimization level `-O{level}`; expected -O0, -O1, -O2, -O3, -Os, or -Oz"
        )),
    }
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

pub(crate) fn default_profile_optimization(profile: BuildProfile) -> NiaOptimizationLevel {
    match profile {
        BuildProfile::Debug => NiaOptimizationLevel::O0,
        BuildProfile::Release => NiaOptimizationLevel::O2,
    }
}

fn parse_timings(mode: Option<&str>) -> Result<nia_driver::TimingMode, String> {
    match mode {
        None | Some("summary") => Ok(nia_driver::TimingMode::Summary),
        Some("detail") => Ok(nia_driver::TimingMode::Detail),
        Some(mode) => Err(format!(
            "unknown timings mode `--timings={mode}`; expected --timings, --timings=summary, or --timings=detail"
        )),
    }
}

fn parse_timing_trace(value: &str) -> Result<TimingTrace, String> {
    match value {
        "off" => Ok(TimingTrace::Off),
        "events" => Ok(TimingTrace::Events),
        _ => Err(format!(
            "unknown timing trace mode `--timing-trace={value}`; expected --timing-trace=off or --timing-trace=events"
        )),
    }
}

/// Parses `text` or `json`, returning whether JSON was selected.
fn parse_text_or_json(value: &str, what: &str) -> Result<bool, String> {
    match value {
        "text" => Ok(false),
        "json" => Ok(true),
        _ => Err(format!("unknown {what} `{value}`; expected text or json")),
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

pub(crate) fn parse_link_time_optimization(
    value: &str,
) -> Result<nia_driver::LinkTimeOptimization, String> {
    match value {
        "off" => Ok(nia_driver::LinkTimeOptimization::Off),
        "thin" => Ok(nia_driver::LinkTimeOptimization::Thin),
        "full" => Ok(nia_driver::LinkTimeOptimization::Full),
        _ => Err(format!(
            "unknown LTO mode `{value}`; expected `off`, `thin`, or `full`"
        )),
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
            "unknown linker flavor `{value}`; expected `gnu`, `lld`, `lld-link`, `ld64.lld`, or `self-hosted-elf`"
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
    let path = crate::resolve_module_map_source(path)?;
    map.try_insert(name, SourcePath::new(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> CliResult<CliAction> {
        parse_cli(args.iter().map(|arg| (*arg).to_string()).collect())
    }

    fn run(args: &[&str]) -> Cli {
        match parse(args) {
            Ok(CliAction::Run(cli)) => *cli,
            Ok(action) => panic!("{args:?}: expected a command, got {action:?}"),
            Err(error) => panic!("{args:?}: {}", error.message),
        }
    }

    fn error(args: &[&str]) -> String {
        match parse(args) {
            Err(error) => error.message,
            Ok(action) => panic!("{args:?}: expected an error, got {action:?}"),
        }
    }

    fn assert_errors(cases: &[(&[&str], &str)]) {
        for (args, expected) in cases {
            let message = error(args);
            assert!(message.contains(expected), "{args:?}: {message}");
        }
    }

    fn package(cli: &Cli) -> &PackageOptions {
        match &cli.command {
            CliCommand::Build { package, .. } => package,
            command => panic!("expected build, got {command:?}"),
        }
    }

    #[test]
    fn global_options_never_take_another_options_value() {
        for args in [
            &["build", "--root", "-O"][..],
            &["-O3", "build", "--root", "-O"],
            &["build", "--root=-O"],
        ] {
            let cli = run(args);
            assert_eq!(
                package(&cli).root.as_deref(),
                Some(std::path::Path::new("-O")),
                "{args:?}"
            );
        }
        let cli = run(&["check", "--cache-dir", "-h", "main.nia"]);
        assert!(matches!(
            &cli.command,
            CliCommand::Check { cache_dir: Some(dir), path, .. }
                if dir == std::path::Path::new("-h") && path == "main.nia"
        ));
    }

    #[test]
    fn global_options_may_precede_or_follow_the_command() {
        for args in [
            &[
                "-O2",
                "--target",
                "x86_64-unknown-linux",
                "check",
                "main.nia",
            ][..],
            &[
                "check",
                "main.nia",
                "-O2",
                "--target",
                "x86_64-unknown-linux",
            ],
            &["check", "-O2", "main.nia", "--target=x86_64-unknown-linux"],
        ] {
            let cli = run(args);
            assert_eq!(cli.optimization, NiaOptimizationLevel::O2, "{args:?}");
            assert!(cli.target.is_some(), "{args:?}");
        }
    }

    #[test]
    fn double_dash_makes_the_rest_positional() {
        let cli = run(&["check", "--", "-strange.nia"]);
        assert!(matches!(&cli.command, CliCommand::Check { path, .. } if path == "-strange.nia"));
        let cli = run(&["build", "--", "a/b"]);
        assert!(
            matches!(&cli.command, CliCommand::Build { step: Some(step), .. } if step == "a/b")
        );
        assert!(matches!(
            parse(&["check", "--", "-h"]),
            Ok(CliAction::Run(_))
        ));
    }

    #[test]
    fn profiles_default_the_optimization_level_and_repeats_keep_the_last() {
        let cli = run(&["build"]);
        assert_eq!(cli.profile, BuildProfile::Debug);
        assert_eq!(cli.optimization, NiaOptimizationLevel::O0);
        let cli = run(&["build", "--release"]);
        assert_eq!(cli.profile, BuildProfile::Release);
        assert_eq!(cli.optimization, NiaOptimizationLevel::O2);
        let cli = run(&["--release", "-O0", "build"]);
        assert_eq!(cli.optimization, NiaOptimizationLevel::O0);
        let cli = run(&["--release", "--debug", "-O2", "-O3", "build"]);
        assert_eq!(cli.profile, BuildProfile::Debug);
        assert_eq!(cli.optimization, NiaOptimizationLevel::O3);
        let cli = run(&[
            "--target",
            "x86_64-unknown-linux",
            "--target",
            "aarch64-unknown-linux",
            "build",
        ]);
        assert_eq!(
            cli.target.map(|target| target.to_string()).as_deref(),
            Some("aarch64-unknown-linux")
        );
        assert_errors(&[
            (&["--profile", "test", "build"], "expected debug or release"),
            (&["-O9", "check", "x"], "unknown optimization level `-O9`"),
        ]);
    }

    #[test]
    fn build_package_options_accept_every_jobs_spelling() {
        for command in ["build"] {
            for jobs in [
                &["-j3"][..],
                &["-j", "3"],
                &["-j=3"],
                &["--jobs=3"],
                &["--jobs", "3"],
            ] {
                let mut args = vec![command];
                args.extend_from_slice(jobs);
                let cli = run(&args);
                assert_eq!(
                    package(&cli).jobs.map(NonZeroUsize::get),
                    Some(3),
                    "{args:?}"
                );
            }
            for (args, expected) in [
                (&["-j"][..], "missing value after `-j`"),
                (&["--jobs=0"], "expected a positive integer"),
                (&["-jx"], "expected a positive integer"),
                (&["--root="], "`--root` cannot be empty"),
                (&["--root", ""], "`--root` cannot be empty"),
                (&["-q", "-v"], "cannot be used together"),
                (&["-q", "--progress", "json"], "cannot be combined"),
                (&["-vvvv"], "may not exceed `-vvv`"),
                (&["-v", "-vvv"], "may not exceed `-vvv`"),
                (&["-qv"], "unknown"),
                (&["--progress=fast"], "invalid progress mode"),
                (&["-M", "a=b"], "is not an option of"),
            ] {
                let mut full = vec![command];
                full.extend_from_slice(args);
                let message = error(&full);
                assert!(message.contains(expected), "{full:?}: {message}");
            }
            let cli = run(&[command, "-v", "-vv", "--progress", "plain"]);
            assert_eq!(package(&cli).verbose, 3);
            assert_eq!(package(&cli).progress, ProgressMode::Plain);
            assert_eq!(package(&run(&[command, "-qq"])).quiet, 1);
        }
    }

    #[test]
    fn workspace_package_selection_options_are_typed_and_repeatable() {
        let cli = run(&["build", "-p", "core", "--package=cli"]);
        assert_eq!(package(&cli).packages, ["core", "cli"]);
        assert!(!package(&cli).workspace_all);

        let cli = run(&["build", "--workspace"]);
        assert!(package(&cli).workspace_all);
        assert!(package(&cli).packages.is_empty());

        assert_errors(&[
            (
                &["build", "--workspace", "-p", "core"],
                "cannot be combined",
            ),
            (&["build", "--package="], "cannot be empty"),
        ]);
    }

    #[test]
    fn build_options_are_typed_and_positional_paths_need_root() {
        let cli = run(&[
            "build",
            "--dry-run",
            "--explain",
            "--print-plan",
            "--print-cache",
            "--print-observations",
            "--lto",
            "thin",
            "check",
        ]);
        assert!(matches!(
            &cli.command,
            CliCommand::Build {
                dry_run: true,
                explain: true,
                print_plan: true,
                print_cache: true,
                print_observations: true,
                link_time_optimization: nia_driver::LinkTimeOptimization::Thin,
                step: Some(step),
                ..
            } if step == "check"
        ));
        assert_errors(&[
            (&["build", "."], "must be passed with `--root`"),
            (&["build", "a", "b"], "unexpected argument `b`"),
            (&["build", "--lto=fat"], "expected `off`, `thin`, or `full`"),
            (&["check", "--lto=thin", "a.nia"], "belongs to `nia build`"),
        ]);
    }

    #[test]
    fn list_and_keep_going_are_build_options() {
        let cli = run(&["build", "--list"]);
        assert!(matches!(
            &cli.command,
            CliCommand::Build {
                list: true,
                keep_going: false,
                step: None,
                ..
            }
        ));
        for flag in ["-k", "--keep-going"] {
            let cli = run(&["build", "test", flag]);
            assert!(matches!(
                &cli.command,
                CliCommand::Build { keep_going: true, step: Some(step), .. } if step == "test"
            ));
        }
        assert_errors(&[
            (&["test"], "unknown command `test`"),
            (
                &["check", "--keep-going", "a.nia"],
                "belongs to `nia build`",
            ),
        ]);
    }

    #[test]
    fn lock_options_and_update_are_typed() {
        let cli = run(&["build", "--locked", "--offline"]);
        assert!(matches!(
            &cli.command,
            CliCommand::Build { package, .. } if package.locked && package.offline
        ));
        let cli = run(&["update", "regex", "json", "--root", "app"]);
        assert!(matches!(
            &cli.command,
            CliCommand::Update { root: Some(root), names }
                if root == std::path::Path::new("app") && names == &["regex", "json"]
        ));
        assert!(matches!(
            &run(&["update"]).command,
            CliCommand::Update { root: None, names } if names.is_empty()
        ));
    }

    #[test]
    fn clean_takes_only_a_root() {
        assert!(matches!(
            run(&["clean"]).command,
            CliCommand::Clean { root: None }
        ));
        assert!(matches!(
            run(&["clean", "--root", "workspace"]).command,
            CliCommand::Clean { root: Some(root) } if root == std::path::Path::new("workspace")
        ));
        assert_errors(&[
            (&["clean", "unexpected"], "unexpected argument"),
            (&["clean", "--migrate"], "unknown `nia clean` option"),
            (&["check", "--root", "x", "a.nia"], "belongs to"),
        ]);
    }

    #[test]
    fn diagnostics_format_accepts_both_forms_and_the_last_wins() {
        for args in [
            &["--diagnostics-format=json", "check", "x"][..],
            &["check", "x", "--diagnostics-format", "json"],
            &[
                "--diagnostics-format=text",
                "check",
                "x",
                "--diagnostics-format=json",
            ],
        ] {
            assert_eq!(
                run(args).diagnostics_format,
                DiagnosticsFormat::Json,
                "{args:?}"
            );
            let strings = args
                .iter()
                .map(|arg| (*arg).to_string())
                .collect::<Vec<_>>();
            assert_eq!(
                requested_diagnostics_format(&strings),
                DiagnosticsFormat::Json
            );
        }
        assert_errors(&[(
            &["--diagnostics-format=xml", "check", "x"],
            "expected text or json",
        )]);
    }

    #[test]
    fn emit_options_are_validated_during_parsing() {
        let cli = run(&[
            "emit",
            "--exe",
            "main.nia",
            "-o",
            "out",
            "--lto=full",
            "-L",
            "lib",
            "-lfoo",
            "--link-arg",
            "-O3",
            "--interpreter=/loader",
            "--executable",
            "dynamic",
        ]);
        let CliCommand::Emit {
            target: EmitTarget::Exe(options),
            path,
            ..
        } = &cli.command
        else {
            panic!("expected emit --exe");
        };
        assert_eq!(path, "main.nia");
        assert_eq!(options.output.as_deref(), Some(std::path::Path::new("out")));
        assert_eq!(
            options.link_time_optimization,
            nia_driver::LinkTimeOptimization::Full
        );
        // `-O3` is the linker argument's value, not an optimization level.
        assert_eq!(options.link_options.raw_args, vec!["-O3".to_string()]);
        assert_eq!(cli.optimization, NiaOptimizationLevel::O0);
        assert_eq!(
            options.link_options.form,
            Some(nia_linker::ExecutableForm::Dynamic(
                nia_linker::Interpreter::Path("/loader".to_string())
            ))
        );
        assert_errors(&[
            (
                &["emit", "--exe", "x", "--bogus"],
                "unknown `nia emit --exe` option `--bogus`",
            ),
            (
                &["emit", "--exe", "x", "--interpreter", "/l"],
                "requires `--executable dynamic`",
            ),
            (
                &["emit", "--exe", "x", "--runtime", "bare"],
                "supports only `--runtime freestanding`",
            ),
            (
                &["emit", "--exe", "x", "--executable", "pie"],
                "unknown executable form",
            ),
            (
                &["emit", "--exe", "x", "--linker-flavor", "x"],
                "or `self-hosted-elf`",
            ),
            (
                &["emit", "--obj", "x", "-o", "a", "--out-dir", "b"],
                "not both",
            ),
            (
                &["emit", "--obj", "x", "--lto=thin"],
                "belongs to `nia build`, `nia emit --exe`",
            ),
            (
                &["emit", "--tokens", "x", "--runtime", "bare"],
                "is not an option of `nia emit --tokens`",
            ),
            (
                &["emit", "--llvm", "x", "extra"],
                "unexpected argument `extra`",
            ),
            (
                &["emit", "--llvm", "--backend", "x"],
                "exactly one emit target",
            ),
            (&["emit", "obj", "x"], "use `--obj`"),
            (&["emit", "x"], "missing emit target flag"),
            (&["emit", "--llvm"], "missing source path"),
        ]);
        assert!(matches!(
            parse(&["emit"]),
            Ok(CliAction::Help(HelpTopic::Emit))
        ));
        assert!(matches!(
            run(&["emit", "--obj", "x", "--out-dir", "d"]).command,
            CliCommand::Emit {
                target: EmitTarget::Obj(EmitObjOptions {
                    output: ObjectOutput::Directory(_),
                    ..
                }),
                ..
            }
        ));
    }

    #[test]
    fn help_and_version_are_recognized_wherever_an_option_may_appear() {
        for (args, topic) in [
            (&[][..], HelpTopic::Main),
            (&["-h"], HelpTopic::Main),
            (&["-O2", "--help"], HelpTopic::Main),
            (&["build", "-h"], HelpTopic::Build),
            (&["clean", "-h"], HelpTopic::Clean),
            (
                &["emit", "--exe", "--help"],
                HelpTopic::EmitKind(EmitKind::Exe),
            ),
            (
                &["help", "emit", "--obj"],
                HelpTopic::EmitKind(EmitKind::Obj),
            ),
        ] {
            assert!(
                matches!(parse(args), Ok(CliAction::Help(found)) if found == topic),
                "{args:?}"
            );
        }
        for args in [
            &["-V"][..],
            &["--version", "--help"],
            &["-V", "build"],
            &["build", "--version"],
        ] {
            assert!(matches!(parse(args), Ok(CliAction::Version)), "{args:?}");
        }
        assert_errors(&[
            (&["-O2"], "missing command"),
            (&["--timings"], "missing command"),
            (&["frobnicate"], "unknown command `frobnicate`"),
            (&["--bogus", "build"], "unknown option `--bogus`"),
            (&["--root", "x", "build"], "must follow the command"),
            (&["help", "build", "extra"], "unknown help topic `extra`"),
            (&["help", "emit", "exe"], "use `nia help emit --exe`"),
            (
                &["help", "emit", "--exe", "--obj"],
                "unknown help topic `--obj`",
            ),
            (&["help", "nonsense"], "unknown help topic `nonsense`"),
        ]);
    }
}
