// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

use nia_driver::{
    BUILTIN_MODULE_MAP_NAME, ENTRY_MODULE_MAP_NAME, ModuleMap, NiaOptimizationLevel,
    PACKAGE_MODULE_MAP_NAME, Runtime, SourcePath,
};
use nia_loader_query::{EntryRuntime, LoadRequest};

static EMIT_EXE_CACHE_STAGE_ID: AtomicU64 = AtomicU64::new(0);
const EMIT_EXE_ARTIFACT_FINGERPRINT_VERSION: &str = "nia-emit-exe-artifact-v1";
const EMIT_EXE_ARTIFACT_MANIFEST_VERSION: &str = "nia-emit-exe-artifact-manifest-v1";

mod help;

use help::{HelpStyle, help_text};

fn main() -> ExitCode {
    nia_ice::install_panic_hook();
    run_with_ice_boundary(run_main, |ice| eprintln!("{}", ice.render_message()))
}

fn run_with_ice_boundary(
    f: impl FnOnce() -> ExitCode,
    report: impl FnOnce(&nia_ice::Ice),
) -> ExitCode {
    match nia_ice::catch_ice(f) {
        Ok(code) => code,
        Err(ice) => {
            report(&ice);
            ExitCode::FAILURE
        }
    }
}

fn run_main() -> ExitCode {
    match parse_cli(env::args().skip(1).collect()) {
        Ok(CliAction::Help(topic)) => {
            print!("{}", help_text(topic, HelpStyle::for_stdout()));
            ExitCode::SUCCESS
        }
        Ok(CliAction::Version) => {
            println!("nia {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(CliAction::Run(cli)) => run_cli(cli),
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

#[derive(Debug)]
struct Cli {
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    timings: nia_driver::TimingMode,
    command: CliCommand,
}

#[derive(Debug)]
enum CliCommand {
    Build {
        root: Option<PathBuf>,
        step: Option<String>,
    },
    Check {
        path: String,
        opt_report: bool,
        runtime: Runtime,
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
    Checked,
    Backend,
    Llvm,
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
    match cli.command {
        CliCommand::Build { root, step } => run_build(root, step, cli.timings),
        CliCommand::Check {
            path,
            opt_report,
            runtime,
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
                cli.optimization,
                cli.timings,
                opt_report,
                runtime,
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
                cli.module_map,
                cli.optimization,
                cli.timings,
                opt_report,
            )
        }
    }
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
            module_map: global_options.module_map,
            optimization: global_options.optimization,
            timings: global_options.timings,
            command,
        })),
    }
}

struct GlobalOptions {
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    timings: nia_driver::TimingMode,
}

fn extract_global_options(
    args: Vec<String>,
    help: HelpTopic,
) -> Result<(Vec<String>, GlobalOptions), CliError> {
    let mut map = ModuleMap::new();
    let mut optimization = NiaOptimizationLevel::default();
    let mut timings = nia_driver::TimingMode::Off;
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
        remaining.push(arg);
    }
    Ok((
        remaining,
        GlobalOptions {
            module_map: map,
            optimization,
            timings,
        },
    ))
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
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--root=") {
            if value.is_empty() {
                return Err(CliError::new("`--root` cannot be empty", HelpTopic::Build));
            }
            root = Some(PathBuf::from(value));
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
    Ok(CliCommand::Build { root, step })
}

fn parse_check_command(args: Vec<String>) -> Result<CliCommand, CliError> {
    if has_help_flag(&args) {
        return Err(CliError::help(HelpTopic::Check));
    }
    let mut path = None;
    let mut opt_report = false;
    let mut runtime = Runtime::Bare;
    let mut explicit_runtime = None::<Runtime>;
    let mut saw_exe = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--runtime=") {
            explicit_runtime = Some(parse_runtime(value).map_err(|message| {
                CliError::new(format!("{message} for `nia check`"), HelpTopic::Check)
            })?);
            if matches!(explicit_runtime, Some(Runtime::Bare)) && saw_exe {
                return Err(CliError::new(
                    "`--runtime bare` cannot be combined with `--exe`",
                    HelpTopic::Check,
                ));
            }
            runtime = explicit_runtime.expect("runtime just set");
            continue;
        }
        match arg.as_str() {
            "--opt-report" => opt_report = true,
            "--exe" => {
                if matches!(explicit_runtime, Some(Runtime::Bare)) {
                    return Err(CliError::new(
                        "`--exe` cannot be combined with `--runtime bare`",
                        HelpTopic::Check,
                    ));
                }
                saw_exe = true;
                runtime = Runtime::Freestanding;
            }
            "--runtime" => {
                let Some(value) = iter.next() else {
                    return Err(CliError::new(
                        "missing runtime after `--runtime`",
                        HelpTopic::Check,
                    ));
                };
                explicit_runtime = Some(parse_runtime(&value).map_err(|message| {
                    CliError::new(format!("{message} for `nia check`"), HelpTopic::Check)
                })?);
                if matches!(explicit_runtime, Some(Runtime::Bare)) && saw_exe {
                    return Err(CliError::new(
                        "`--runtime bare` cannot be combined with `--exe`",
                        HelpTopic::Check,
                    ));
                }
                runtime = explicit_runtime.expect("runtime just set");
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
        ParsedEmitTarget::Checked => EmitTarget::Checked,
        ParsedEmitTarget::Backend => EmitTarget::Backend,
        ParsedEmitTarget::Llvm => EmitTarget::Llvm,
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
    if matches!(
        name,
        ENTRY_MODULE_MAP_NAME | PACKAGE_MODULE_MAP_NAME | BUILTIN_MODULE_MAP_NAME
    ) {
        return Err(format!("`{name}` is a compiler-reserved module root"));
    }
    if path.is_empty() {
        return Err(format!("module map `{name}` has empty path"));
    }
    map.insert(name, SourcePath::new(path));
    Ok(())
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

fn run_check(
    path: &str,
    source: &str,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    timings: nia_driver::TimingMode,
    opt_report: bool,
    runtime: Runtime,
) -> ExitCode {
    let output = time_stage(timings, "check", || {
        check_with_driver(path, module_map.clone(), optimization, timings, runtime)
    });
    match checked_program_from_output(output, path, source) {
        Ok(_) => {}
        Err(code) => return code,
    }
    if opt_report {
        let output = time_stage(timings, "codegen", || {
            codegen_with_driver(path, module_map, optimization, timings, runtime)
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
) -> nia_driver::DriverOutput<nia_driver::CheckedProgram> {
    nia_driver::Driver::new().check_entry(
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
        Ok(program) => Ok(program),
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
    path: &str,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    timings: nia_driver::TimingMode,
    runtime: Runtime,
) -> nia_driver::DriverOutput<nia_driver::CodegenProgram> {
    nia_driver::Driver::new().codegen(
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
        Ok(program) => Ok(program),
        Err(error) => {
            eprint!(
                "{}",
                nia_driver::render_driver_error(&error, Some(path), Some(source))
            );
            Err(ExitCode::FAILURE)
        }
    }
}

fn run_emit(
    path: &str,
    source: &str,
    target: EmitTarget,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    timings: nia_driver::TimingMode,
    opt_report: bool,
) -> ExitCode {
    match target {
        EmitTarget::Tokens => time_stage(timings, "lex", || run_lex(source)),
        EmitTarget::Ast => time_stage(timings, "parse", || run_parse(path, source)),
        EmitTarget::Checked => run_emit_checked(path, source, module_map, optimization, timings),
        EmitTarget::Backend => {
            run_emit_backend(path, source, module_map, optimization, timings, opt_report)
        }
        EmitTarget::Llvm => {
            run_emit_llvm(path, source, module_map, optimization, timings, opt_report)
        }
        EmitTarget::Obj { args } => run_emit_obj(
            path,
            source,
            args,
            module_map,
            optimization,
            timings,
            opt_report,
        ),
        EmitTarget::Exe { args } => run_emit_exe(
            path,
            source,
            args,
            module_map,
            optimization,
            timings,
            opt_report,
        ),
    }
}

fn run_build(
    root: Option<PathBuf>,
    step: Option<String>,
    timings: nia_driver::TimingMode,
) -> ExitCode {
    let mut request = nia_build::BuildRequest::new();
    if let Some(root) = root {
        request = request.with_root(root);
    }
    if let Some(step) = step {
        request = request.with_step(step);
    }
    request = request.with_timings(timings);
    match nia_build::run_build(request) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn time_stage<T>(timings: nia_driver::TimingMode, name: &str, f: impl FnOnce() -> T) -> T {
    if !timings.enabled() {
        return f();
    }
    let start = Instant::now();
    let result = f();
    eprintln!("timing {name}: {:.3}s", start.elapsed().as_secs_f64());
    result
}

fn run_emit_checked(
    path: &str,
    source: &str,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    timings: nia_driver::TimingMode,
) -> ExitCode {
    let output = time_stage(timings, "check", || {
        check_with_driver(path, module_map, optimization, timings, Runtime::Bare)
    });
    let program = match checked_program_from_output(output, path, source) {
        Ok(program) => program,
        Err(code) => return code,
    };
    println!("{program:#?}");
    ExitCode::SUCCESS
}

fn run_emit_backend(
    path: &str,
    source: &str,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    timings: nia_driver::TimingMode,
    opt_report: bool,
) -> ExitCode {
    let output = time_stage(timings, "codegen", || {
        codegen_with_driver(path, module_map, optimization, timings, Runtime::Bare)
    });
    let program = match codegen_program_from_output(output, path, source) {
        Ok(program) => program,
        Err(code) => return code,
    };
    if opt_report {
        print_optimization_report_to_stderr(&program);
    }
    println!("{:#?}", program.backend_lowering.program);
    ExitCode::SUCCESS
}

fn run_emit_llvm(
    path: &str,
    source: &str,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    timings: nia_driver::TimingMode,
    opt_report: bool,
) -> ExitCode {
    let output = time_stage(timings, "codegen", || {
        codegen_with_driver(path, module_map, optimization, timings, Runtime::Bare)
    });
    let program = match codegen_program_from_output(output, path, source) {
        Ok(program) => program,
        Err(code) => return code,
    };
    if opt_report {
        print_optimization_report_to_stderr(&program);
    }
    let output = time_stage(timings, "emit_llvm_ir", || {
        nia_driver::Driver::new().emit_llvm_ir_from_codegen(&program)
    });
    match output.result {
        Ok(artifact) => {
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

fn run_emit_obj(
    path: &str,
    source: &str,
    args: Vec<String>,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    timings: nia_driver::TimingMode,
    opt_report: bool,
) -> ExitCode {
    let options = match parse_emit_obj_options(path, args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let output = time_stage(timings, "codegen", || {
        codegen_with_driver(path, module_map, optimization, timings, options.runtime)
    });
    let program = match codegen_program_from_output(output, path, source) {
        Ok(program) => program,
        Err(code) => return code,
    };
    if opt_report {
        print_optimization_report_to_stderr(&program);
    }
    let output = time_stage(timings, "emit_native_objects", || {
        nia_driver::Driver::new().emit_native_objects_from_codegen(&program)
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
    let output = nia_driver::Driver::new()
        .write_native_objects_from_artifact(&objects, options.output.into_driver_output());
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

fn run_emit_exe(
    path: &str,
    source: &str,
    args: Vec<String>,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    timings: nia_driver::TimingMode,
    opt_report: bool,
) -> ExitCode {
    let options = match parse_emit_exe_options(path, args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let cache_entry = if let Some(cache_dir) = &options.cache_dir {
        let entry = time_stage(timings, "emit_exe_cache_fingerprint", || {
            emit_exe_artifact_cache_entry(
                path,
                module_map.clone(),
                optimization,
                &options,
                cache_dir,
            )
        });
        let entry = match entry {
            Ok(cache) => cache,
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::FAILURE;
            }
        };
        if time_stage(timings, "emit_exe_cache_restore", || {
            restore_emit_exe_artifact_cache(&entry, &options.output)
        }) {
            return ExitCode::SUCCESS;
        }
        Some(entry)
    } else {
        None
    };
    let output = time_stage(timings, "codegen_exe", || {
        codegen_with_driver(
            path,
            module_map,
            optimization,
            timings,
            Runtime::Freestanding,
        )
    });
    let program = match codegen_program_from_output(output, path, source) {
        Ok(program) => program,
        Err(code) => return code,
    };
    if opt_report {
        print_optimization_report_to_stderr(&program);
    }
    let output = time_stage(timings, "emit_native_objects", || {
        nia_driver::Driver::new().emit_native_objects_from_codegen(&program)
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
    let output = time_stage(timings, "link_executable", || {
        nia_driver::Driver::new().link_executable_from_objects(
            &objects,
            options.output.clone(),
            options.link_options.clone(),
        )
    });
    match output.result {
        Ok(artifact) => {
            if let Some(cache) = cache_entry {
                if let Err(error) = time_stage(timings, "emit_exe_cache_publish", || {
                    publish_emit_exe_artifact_cache(&artifact.path, &cache)
                }) {
                    eprintln!("{error}");
                    return ExitCode::FAILURE;
                }
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

#[derive(Debug, Clone)]
struct EmitExeArtifactCacheEntry {
    executable: PathBuf,
    cache_dir: PathBuf,
    snapshot: Option<EmitExeArtifactCacheSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EmitExeArtifactCacheSnapshot {
    request_hash: String,
    fingerprint: String,
    inputs: Vec<EmitExeArtifactCacheInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EmitExeArtifactCacheInput {
    path: String,
    generated: bool,
    content_len: u64,
    content_hash: String,
}

fn emit_exe_artifact_cache_entry(
    path: &str,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    options: &EmitExeOptions,
    cache_dir: &Path,
) -> Result<EmitExeArtifactCacheEntry, String> {
    let request_hash = emit_exe_artifact_request_hash(path, &module_map, optimization, options);
    if let Some(snapshot) = restore_emit_exe_artifact_manifest(cache_dir, &request_hash)? {
        let executable = emit_exe_artifact_cache_path(cache_dir, &snapshot.fingerprint);
        return Ok(EmitExeArtifactCacheEntry {
            executable,
            cache_dir: cache_dir.to_path_buf(),
            snapshot: Some(snapshot),
        });
    }
    let Some(inputs) = loaded_emit_exe_module_inputs(path, module_map)? else {
        let fingerprint = emit_exe_artifact_fingerprint(&request_hash, &[]);
        return Ok(EmitExeArtifactCacheEntry {
            executable: emit_exe_artifact_cache_path(cache_dir, &fingerprint),
            cache_dir: cache_dir.to_path_buf(),
            snapshot: None,
        });
    };
    let fingerprint = emit_exe_artifact_fingerprint(&request_hash, &inputs);
    let snapshot = EmitExeArtifactCacheSnapshot {
        request_hash,
        fingerprint: fingerprint.clone(),
        inputs,
    };
    Ok(EmitExeArtifactCacheEntry {
        executable: emit_exe_artifact_cache_path(cache_dir, &fingerprint),
        cache_dir: cache_dir.to_path_buf(),
        snapshot: Some(snapshot),
    })
}

fn emit_exe_artifact_request_hash(
    path: &str,
    module_map: &ModuleMap,
    optimization: NiaOptimizationLevel,
    options: &EmitExeOptions,
) -> String {
    let mut hash = StableFingerprint::new();
    hash.string(EMIT_EXE_ARTIFACT_FINGERPRINT_VERSION);
    hash.string(env!("CARGO_PKG_VERSION"));
    hash.string(path);
    hash.string(&format!("{:?}", optimization));
    hash.string(&format!("{:?}", nia_driver::DriverConfig::default().target));
    hash.string(&format!("{:?}", options.link_options));
    let mut module_entries = module_map
        .entries()
        .map(|(name, path)| (name.to_string(), path.as_str().to_string()))
        .collect::<Vec<_>>();
    module_entries.sort();
    for (name, path) in module_entries {
        hash.string(&name);
        hash.string(&path);
    }
    hash.finish()
}

fn emit_exe_artifact_fingerprint(
    request_hash: &str,
    inputs: &[EmitExeArtifactCacheInput],
) -> String {
    let mut hash = StableFingerprint::new();
    hash.string(request_hash);
    for input in inputs {
        hash.string(&input.path);
        hash.string(if input.generated { "generated" } else { "file" });
        hash.string(&input.content_len.to_string());
        hash.string(&input.content_hash);
    }
    hash.finish()
}

fn emit_exe_artifact_cache_path(cache_dir: &Path, fingerprint: &str) -> PathBuf {
    cache_dir
        .join("artifacts")
        .join("executables")
        .join(fingerprint)
        .join("app")
}

fn loaded_emit_exe_module_inputs(
    path: &str,
    module_map: ModuleMap,
) -> Result<Option<Vec<EmitExeArtifactCacheInput>>, String> {
    let loaded = nia_loader_query::load_program_request(
        LoadRequest::new(path)
            .with_module_map(module_map)
            .with_entry_runtime(EntryRuntime::Freestanding),
    );
    if !loaded.diagnostics.is_empty() {
        return Ok(None);
    }
    let mut modules = loaded
        .modules
        .iter()
        .map(|module| module.path.as_str().to_string())
        .collect::<Vec<_>>();
    modules.sort();
    modules.dedup();
    let mut inputs = Vec::with_capacity(modules.len());
    for module_path in modules {
        if module_path.starts_with("<nia:") {
            inputs.push(EmitExeArtifactCacheInput {
                path: module_path,
                generated: true,
                content_len: 0,
                content_hash: content_hash("<generated>"),
            });
            continue;
        }
        match fs::read_to_string(&module_path) {
            Ok(source) => inputs.push(EmitExeArtifactCacheInput {
                path: module_path,
                generated: false,
                content_len: source.len() as u64,
                content_hash: content_hash(&source),
            }),
            Err(error) => {
                return Err(format!(
                    "failed to read `{module_path}` for executable cache fingerprint: {error}"
                ));
            }
        }
    }
    Ok(Some(inputs))
}

fn restore_emit_exe_artifact_manifest(
    cache_dir: &Path,
    request_hash: &str,
) -> Result<Option<EmitExeArtifactCacheSnapshot>, String> {
    let Some(snapshot) = read_emit_exe_artifact_manifest(cache_dir, request_hash)? else {
        return Ok(None);
    };
    for input in &snapshot.inputs {
        let current = current_emit_exe_artifact_input(input)?;
        if current.content_len != input.content_len || current.content_hash != input.content_hash {
            return Ok(None);
        }
    }
    let fingerprint = emit_exe_artifact_fingerprint(&snapshot.request_hash, &snapshot.inputs);
    if fingerprint == snapshot.fingerprint {
        Ok(Some(snapshot))
    } else {
        Ok(None)
    }
}

fn current_emit_exe_artifact_input(
    input: &EmitExeArtifactCacheInput,
) -> Result<EmitExeArtifactCacheInput, String> {
    if input.generated {
        return Ok(EmitExeArtifactCacheInput {
            path: input.path.clone(),
            generated: true,
            content_len: 0,
            content_hash: content_hash("<generated>"),
        });
    }
    let source = fs::read_to_string(&input.path).map_err(|error| {
        format!(
            "failed to read `{}` for executable cache manifest: {error}",
            input.path
        )
    })?;
    Ok(EmitExeArtifactCacheInput {
        path: input.path.clone(),
        generated: false,
        content_len: source.len() as u64,
        content_hash: content_hash(&source),
    })
}

fn emit_exe_artifact_manifest_path(cache_dir: &Path, request_hash: &str) -> PathBuf {
    cache_dir
        .join("artifacts")
        .join("executables")
        .join("manifests")
        .join(request_hash)
}

fn read_emit_exe_artifact_manifest(
    cache_dir: &Path,
    request_hash: &str,
) -> Result<Option<EmitExeArtifactCacheSnapshot>, String> {
    let path = emit_exe_artifact_manifest_path(cache_dir, request_hash);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed to read executable cache manifest `{}`: {error}",
                path.display()
            ));
        }
    };
    Ok(parse_emit_exe_artifact_manifest(&text)
        .filter(|snapshot| snapshot.request_hash == request_hash))
}

fn save_emit_exe_artifact_manifest(
    cache: &EmitExeArtifactCacheEntry,
    snapshot: &EmitExeArtifactCacheSnapshot,
) -> Result<(), String> {
    let path = emit_exe_artifact_manifest_path(&cache.cache_dir, &snapshot.request_hash);
    let parent = path
        .parent()
        .ok_or_else(|| "invalid executable cache manifest path".to_string())?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create executable cache manifest directory `{}`: {error}",
            parent.display()
        )
    })?;
    let staged = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        EMIT_EXE_CACHE_STAGE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&staged, format_emit_exe_artifact_manifest(snapshot)).map_err(|error| {
        format!(
            "failed to write executable cache manifest `{}`: {error}",
            staged.display()
        )
    })?;
    match fs::rename(&staged, &path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&staged);
            Err(format!(
                "failed to publish executable cache manifest `{}`: {error}",
                path.display()
            ))
        }
    }
}

fn format_emit_exe_artifact_manifest(snapshot: &EmitExeArtifactCacheSnapshot) -> String {
    let mut text = String::new();
    text.push_str(EMIT_EXE_ARTIFACT_MANIFEST_VERSION);
    text.push('\n');
    text.push_str("request\t");
    text.push_str(&snapshot.request_hash);
    text.push('\n');
    text.push_str("fingerprint\t");
    text.push_str(&snapshot.fingerprint);
    text.push('\n');
    for input in &snapshot.inputs {
        text.push_str("input\t");
        text.push_str(if input.generated { "generated" } else { "file" });
        text.push('\t');
        text.push_str(&input.content_len.to_string());
        text.push('\t');
        text.push_str(&input.content_hash);
        text.push('\t');
        text.push_str(&input.path);
        text.push('\n');
    }
    text
}

fn parse_emit_exe_artifact_manifest(text: &str) -> Option<EmitExeArtifactCacheSnapshot> {
    let mut lines = text.lines();
    (lines.next()? == EMIT_EXE_ARTIFACT_MANIFEST_VERSION).then_some(())?;
    let request_hash = lines.next()?.strip_prefix("request\t")?.to_string();
    let fingerprint = lines.next()?.strip_prefix("fingerprint\t")?.to_string();
    if request_hash.is_empty() || fingerprint.is_empty() {
        return None;
    }
    let mut inputs = Vec::new();
    for line in lines {
        let mut fields = line.splitn(5, '\t');
        (fields.next()? == "input").then_some(())?;
        let generated = match fields.next()? {
            "generated" => true,
            "file" => false,
            _ => return None,
        };
        let content_len = fields.next()?.parse().ok()?;
        let content_hash = fields.next()?.to_string();
        let path = fields.next()?.to_string();
        if content_hash.is_empty() || path.is_empty() {
            return None;
        }
        inputs.push(EmitExeArtifactCacheInput {
            path,
            generated,
            content_len,
            content_hash,
        });
    }
    Some(EmitExeArtifactCacheSnapshot {
        request_hash,
        fingerprint,
        inputs,
    })
}

fn restore_emit_exe_artifact_cache(cache: &EmitExeArtifactCacheEntry, output: &Path) -> bool {
    if !cache.executable.is_file() {
        return false;
    }
    let Some(parent) = output.parent() else {
        return fs::copy(&cache.executable, output).is_ok();
    };
    if !parent.as_os_str().is_empty() && fs::create_dir_all(parent).is_err() {
        return false;
    }
    let staged = output.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        EMIT_EXE_CACHE_STAGE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    if fs::copy(&cache.executable, &staged).is_err() {
        let _ = fs::remove_file(&staged);
        return false;
    }
    if make_executable_like(&staged, &cache.executable).is_err() {
        let _ = fs::remove_file(&staged);
        return false;
    }
    match fs::rename(&staged, output) {
        Ok(()) => true,
        Err(_) => {
            let _ = fs::remove_file(&staged);
            false
        }
    }
}

fn publish_emit_exe_artifact_cache(
    output: &Path,
    cache: &EmitExeArtifactCacheEntry,
) -> Result<(), String> {
    if cache.executable.is_file() {
        return Ok(());
    }
    let parent = cache
        .executable
        .parent()
        .ok_or_else(|| "invalid executable cache path".to_string())?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create executable cache directory `{}`: {error}",
            parent.display()
        )
    })?;
    let staged = cache.executable.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        EMIT_EXE_CACHE_STAGE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::copy(output, &staged).map_err(|error| {
        format!(
            "failed to copy executable `{}` into cache `{}`: {error}",
            output.display(),
            staged.display()
        )
    })?;
    make_executable_like(&staged, output).map_err(|error| {
        format!(
            "failed to set executable cache permissions `{}`: {error}",
            staged.display()
        )
    })?;
    match fs::rename(&staged, &cache.executable) {
        Ok(()) => Ok(()),
        Err(error) if cache.executable.is_file() => {
            let _ = fs::remove_file(&staged);
            let _ = error;
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&staged);
            Err(format!(
                "failed to publish executable cache `{}`: {error}",
                cache.executable.display()
            ))
        }
    }?;
    if let Some(snapshot) = &cache.snapshot {
        save_emit_exe_artifact_manifest(cache, snapshot)?;
    }
    Ok(())
}

fn make_executable_like(path: &Path, source: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::metadata(source)?.permissions())?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        let _ = source;
    }
    Ok(())
}

struct StableFingerprint {
    state: u64,
}

impl StableFingerprint {
    fn new() -> Self {
        Self {
            state: 0xcbf29ce484222325,
        }
    }

    fn string(&mut self, text: &str) {
        self.bytes(&(text.len() as u64).to_le_bytes());
        self.bytes(text.as_bytes());
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.state ^= u64::from(*byte);
            self.state = self.state.wrapping_mul(0x100000001b3);
        }
    }

    fn finish(self) -> String {
        format!("{:016x}", self.state)
    }
}

fn content_hash(text: &str) -> String {
    let mut hash = StableFingerprint::new();
    hash.string(text);
    hash.finish()
}

struct EmitObjOptions {
    output: EmitObjOutput,
    runtime: Runtime,
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
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--runtime=") {
            runtime = parse_runtime(value)
                .map_err(|message| format!("{message} for `nia emit --obj`"))?;
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
            _ => return Err(format!("unknown `nia emit --obj` option `{arg}`")),
        }
    }
    let output = match (output, out_dir) {
        (Some(_), Some(_)) => Err("use either `-o` or `--out-dir`, not both".to_string()),
        (Some(path), None) => Ok(EmitObjOutput::Single(path)),
        (None, Some(path)) => Ok(EmitObjOutput::Directory(path)),
        (None, None) => Ok(EmitObjOutput::Single(default_output_path(source, "o"))),
    }?;
    Ok(EmitObjOptions { output, runtime })
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
            || panic!("Nia ICE: forced failure"),
            |ice| message = ice.render_message(),
        );

        assert_eq!(code, ExitCode::FAILURE);
        assert!(message.contains("internal compiler error: forced failure"));
        assert!(message.contains("Please report it"));
    }

    #[test]
    fn emit_exe_artifact_cache_fingerprint_tracks_loaded_source_graph() {
        let root = temp_root("emit_exe_artifact_cache_fingerprint_tracks_loaded_source_graph");
        std::fs::create_dir_all(root.join("src")).expect("create src");
        std::fs::write(
            root.join("src/main.nia"),
            r#"
module helper;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    _ = helper::value();
    !{}
}
"#,
        )
        .expect("write main");
        std::fs::write(root.join("src/helper.nia"), "pub fn value() i32 { 1 }\n")
            .expect("write helper");
        let source = root.join("src/main.nia").to_string_lossy().into_owned();
        let cache_dir = root.join(".nia-cache");
        let options = EmitExeOptions {
            output: root.join(".nia-build/app"),
            cache_dir: Some(cache_dir.clone()),
            link_options: nia_linker::LinkOptions::default(),
        };

        let before = emit_exe_artifact_cache_entry(
            &source,
            ModuleMap::default(),
            NiaOptimizationLevel::default(),
            &options,
            &cache_dir,
        )
        .expect("fingerprint before");
        std::fs::write(root.join("src/helper.nia"), "pub fn value() i32 { 2 }\n")
            .expect("edit helper");
        let after = emit_exe_artifact_cache_entry(
            &source,
            ModuleMap::default(),
            NiaOptimizationLevel::default(),
            &options,
            &cache_dir,
        )
        .expect("fingerprint after");

        assert_ne!(before.executable, after.executable);
    }

    #[test]
    fn emit_exe_artifact_cache_fingerprint_ignores_unloaded_package_sources() {
        let root =
            temp_root("emit_exe_artifact_cache_fingerprint_ignores_unloaded_package_sources");
        std::fs::create_dir_all(root.join("src")).expect("create src");
        std::fs::write(
            root.join("src/main.nia"),
            r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
        )
        .expect("write main");
        std::fs::write(root.join("src/unused.nia"), "pub fn value() i32 { 1 }\n")
            .expect("write unused");
        let source = root.join("src/main.nia").to_string_lossy().into_owned();
        let cache_dir = root.join(".nia-cache");
        let options = EmitExeOptions {
            output: root.join(".nia-build/app"),
            cache_dir: Some(cache_dir.clone()),
            link_options: nia_linker::LinkOptions::default(),
        };

        let before = emit_exe_artifact_cache_entry(
            &source,
            ModuleMap::default(),
            NiaOptimizationLevel::default(),
            &options,
            &cache_dir,
        )
        .expect("fingerprint before");
        std::fs::write(root.join("src/unused.nia"), "pub fn value() i32 { 2 }\n")
            .expect("edit unused");
        let after = emit_exe_artifact_cache_entry(
            &source,
            ModuleMap::default(),
            NiaOptimizationLevel::default(),
            &options,
            &cache_dir,
        )
        .expect("fingerprint after");

        assert_eq!(before.executable, after.executable);
    }

    #[test]
    fn emit_exe_artifact_manifest_restores_unchanged_fingerprint() {
        let root = temp_root("emit_exe_artifact_manifest_restores_unchanged_fingerprint");
        std::fs::create_dir_all(root.join("src")).expect("create src");
        std::fs::write(
            root.join("src/main.nia"),
            r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
        )
        .expect("write main");
        let source = root.join("src/main.nia").to_string_lossy().into_owned();
        let cache_dir = root.join(".nia-cache");
        let options = EmitExeOptions {
            output: root.join(".nia-build/app"),
            cache_dir: Some(cache_dir.clone()),
            link_options: nia_linker::LinkOptions::default(),
        };
        let before = emit_exe_artifact_cache_entry(
            &source,
            ModuleMap::default(),
            NiaOptimizationLevel::default(),
            &options,
            &cache_dir,
        )
        .expect("fingerprint before");
        let snapshot = before.snapshot.clone().expect("snapshot");
        save_emit_exe_artifact_manifest(&before, &snapshot).expect("save manifest");

        let after = emit_exe_artifact_cache_entry(
            &source,
            ModuleMap::default(),
            NiaOptimizationLevel::default(),
            &options,
            &cache_dir,
        )
        .expect("fingerprint after");

        assert_eq!(before.executable, after.executable);
    }

    #[test]
    fn emit_exe_artifact_manifest_rejects_changed_loaded_source() {
        let root = temp_root("emit_exe_artifact_manifest_rejects_changed_loaded_source");
        std::fs::create_dir_all(root.join("src")).expect("create src");
        std::fs::write(
            root.join("src/main.nia"),
            r#"
module helper;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    _ = helper::value();
    !{}
}
"#,
        )
        .expect("write main");
        std::fs::write(root.join("src/helper.nia"), "pub fn value() i32 { 1 }\n")
            .expect("write helper");
        let source = root.join("src/main.nia").to_string_lossy().into_owned();
        let cache_dir = root.join(".nia-cache");
        let options = EmitExeOptions {
            output: root.join(".nia-build/app"),
            cache_dir: Some(cache_dir.clone()),
            link_options: nia_linker::LinkOptions::default(),
        };
        let before = emit_exe_artifact_cache_entry(
            &source,
            ModuleMap::default(),
            NiaOptimizationLevel::default(),
            &options,
            &cache_dir,
        )
        .expect("fingerprint before");
        let snapshot = before.snapshot.clone().expect("snapshot");
        save_emit_exe_artifact_manifest(&before, &snapshot).expect("save manifest");

        std::fs::write(root.join("src/helper.nia"), "pub fn value() i32 { 2 }\n")
            .expect("edit helper");
        let after = emit_exe_artifact_cache_entry(
            &source,
            ModuleMap::default(),
            NiaOptimizationLevel::default(),
            &options,
            &cache_dir,
        )
        .expect("fingerprint after");

        assert_ne!(before.executable, after.executable);
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("nia-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create temp root");
        root
    }
}
