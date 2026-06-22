// SPDX-License-Identifier: GPL-3.0-or-later
use std::{env, fs, path::PathBuf, process::ExitCode, time::Instant};

use nia_driver::{
    BUILTIN_MODULE_MAP_NAME, ENTRY_MODULE_MAP_NAME, ModuleMap, NiaOptimizationLevel,
    PACKAGE_MODULE_MAP_NAME, Runtime, SourcePath,
};

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
        !matches!(self, Self::Tokens | Self::Ast)
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
        check_with_driver(path, module_map, optimization, timings, runtime)
    });
    let program = match checked_program_from_output(output, path, source) {
        Ok(program) => program,
        Err(code) => return code,
    };
    if opt_report {
        print_optimization_report(&program);
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
    nia_driver::Driver::new().check(
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
        EmitTarget::Checked => {
            run_emit_checked(path, source, module_map, optimization, timings, opt_report)
        }
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
    opt_report: bool,
) -> ExitCode {
    let output = time_stage(timings, "check", || {
        check_with_driver(path, module_map, optimization, timings, Runtime::Bare)
    });
    let program = match checked_program_from_output(output, path, source) {
        Ok(program) => program,
        Err(code) => return code,
    };
    if opt_report {
        print_optimization_report_to_stderr(&program);
    }
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
    let output = time_stage(timings, "check", || {
        check_with_driver(path, module_map, optimization, timings, Runtime::Bare)
    });
    let program = match checked_program_from_output(output, path, source) {
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
    let output = time_stage(timings, "check", || {
        check_with_driver(path, module_map, optimization, timings, Runtime::Bare)
    });
    let program = match checked_program_from_output(output, path, source) {
        Ok(program) => program,
        Err(code) => return code,
    };
    if opt_report {
        print_optimization_report_to_stderr(&program);
    }
    let output = time_stage(timings, "emit_llvm_ir", || {
        nia_driver::Driver::new().emit_llvm_ir_from_checked(&program)
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
    let output = time_stage(timings, "check", || {
        check_with_driver(path, module_map, optimization, timings, options.runtime)
    });
    let program = match checked_program_from_output(output, path, source) {
        Ok(program) => program,
        Err(code) => return code,
    };
    if opt_report {
        print_optimization_report_to_stderr(&program);
    }
    let output = time_stage(timings, "emit_native_objects", || {
        nia_driver::Driver::new().emit_native_objects_from_checked(&program)
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
    let output = time_stage(timings, "check_exe", || {
        check_with_driver(
            path,
            module_map,
            optimization,
            timings,
            Runtime::Freestanding,
        )
    });
    let program = match checked_program_from_output(output, path, source) {
        Ok(program) => program,
        Err(code) => return code,
    };
    if opt_report {
        print_optimization_report_to_stderr(&program);
    }
    let output = time_stage(timings, "emit_native_objects", || {
        nia_driver::Driver::new().emit_native_objects_from_checked(&program)
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
            options.output,
            options.link_options,
        )
    });
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
    link_options: nia_linker::LinkOptions,
}

fn parse_emit_exe_options(source: &str, args: Vec<String>) -> Result<EmitExeOptions, String> {
    let mut output = None::<PathBuf>;
    let mut link_options = nia_linker::LinkOptions::default();
    let mut explicit_linker_program = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
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

fn print_optimization_report(program: &nia_driver::CheckedProgram) {
    print!("{}", nia_driver::optimization_report(program));
}

fn print_optimization_report_to_stderr(program: &nia_driver::CheckedProgram) {
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
}
