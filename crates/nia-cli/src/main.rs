// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::Instant,
};

use nia_diagnostic::{Diagnostic, render_diagnostic};
use nia_imports::{ModuleMap, ROOT_MODULE_MAP_NAME};
use nia_opt::{
    InlineThreshold, NiaOptimizationLevel, OptimizationDepth, OptimizationPolicy,
    SpecializationPolicy,
};
use nia_parser::ParseError;
use nia_source::SourcePath;

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
    Check {
        path: String,
        opt_report: bool,
        exe: bool,
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
    let (path, source) = match read_source_for_command(&cli.command) {
        Ok((path, source)) => (path, source),
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };

    match cli.command {
        CliCommand::Check {
            opt_report, exe, ..
        } => run_check(
            &path,
            &source,
            cli.module_map,
            cli.optimization,
            cli.timings,
            opt_report,
            exe,
        ),
        CliCommand::Emit {
            target, opt_report, ..
        } => run_emit(
            &path,
            &source,
            target,
            cli.module_map,
            cli.optimization,
            cli.timings,
            opt_report,
        ),
    }
}

fn read_source_for_command(command: &CliCommand) -> Result<(String, String), String> {
    let path = match command {
        CliCommand::Check { path, .. } | CliCommand::Emit { path, .. } => path,
    };
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(err) => {
            return Err(format!("failed to read `{path}`: {err}"));
        }
    };
    Ok((path.clone(), source))
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
        if native_emit_option_takes_path(&arg) {
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

fn native_emit_option_takes_path(arg: &str) -> bool {
    arg == "-o" || arg == "--out-dir"
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
        [command] if command == "check" => HelpTopic::Check,
        [command] if command == "emit" => HelpTopic::Emit,
        [command, target] if command == "emit" && emit_target_flag(target).is_some() => {
            HelpTopic::Emit
        }
        _ => HelpTopic::Main,
    }
}

fn parse_check_command(args: Vec<String>) -> Result<CliCommand, CliError> {
    if has_help_flag(&args) {
        return Err(CliError::help(HelpTopic::Check));
    }
    let mut path = None;
    let mut opt_report = false;
    let mut exe = false;
    for arg in args {
        match arg.as_str() {
            "--opt-report" => opt_report = true,
            "--exe" => exe = true,
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
        exe,
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
            _ if native_emit_option_takes_path(&arg) => {
                preserve_next = true;
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
    if name == ROOT_MODULE_MAP_NAME {
        return Err(format!(
            "`{ROOT_MODULE_MAP_NAME}` is a compiler-reserved module root"
        ));
    }
    if path.is_empty() {
        return Err(format!("module map `{name}` has empty path"));
    }
    map.insert(name, SourcePath::new(path));
    Ok(())
}

fn run_lex(source: &str) -> ExitCode {
    for token in nia_lexer::tokenize(source) {
        println!("{:?} {}..{}", token.kind, token.span.start, token.span.end);
    }
    ExitCode::SUCCESS
}

fn run_parse(path: &str, source: &str) -> ExitCode {
    let (module, errors) = nia_parser::parse_module(source);
    println!("{module:#?}");
    if !errors.is_empty() {
        print_parse_errors(path, source, &errors);
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
    exe: bool,
) -> ExitCode {
    let program = time_stage(timings, "check", || {
        if exe {
            nia_driver::check_freestanding_executable_with_map_options_and_timings(
                path,
                module_map,
                optimization,
                timings,
            )
        } else {
            nia_driver::check_program_with_map_options_and_timings(
                path,
                module_map,
                optimization,
                timings,
            )
        }
    });
    if !program.diagnostics.is_empty() {
        print_program_diagnostics(path, source, &program);
        return ExitCode::FAILURE;
    }
    if opt_report {
        print_optimization_report(&program);
    }
    ExitCode::SUCCESS
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
    let program = time_stage(timings, "check", || {
        nia_driver::check_program_with_map_options_and_timings(
            path,
            module_map,
            optimization,
            timings,
        )
    });
    if !program.diagnostics.is_empty() {
        print_program_diagnostics(path, source, &program);
        return ExitCode::FAILURE;
    }
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
    let program = time_stage(timings, "check", || {
        nia_driver::check_program_with_map_options_and_timings(
            path,
            module_map,
            optimization,
            timings,
        )
    });
    if !program.diagnostics.is_empty() {
        print_program_diagnostics(path, source, &program);
        return ExitCode::FAILURE;
    }
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
    let program = time_stage(timings, "check", || {
        nia_driver::check_program_with_map_options_and_timings(
            path,
            module_map,
            optimization,
            timings,
        )
    });
    if !program.diagnostics.is_empty() {
        print_program_diagnostics(path, source, &program);
        return ExitCode::FAILURE;
    }
    if opt_report {
        print_optimization_report_to_stderr(&program);
    }
    let output = time_stage(timings, "emit_llvm_ir", || {
        nia_codegen_llvm::emit_llvm_ir_with_options(
            &program.backend_lowering.program,
            codegen_options(program.optimization),
        )
    });
    if !output.diagnostics.is_empty() {
        eprintln!("codegen diagnostics:");
        for diagnostic in &output.diagnostics {
            eprintln!("{}", render_diagnostic(path, source, diagnostic));
        }
        return ExitCode::FAILURE;
    }
    for module in output.modules {
        print!("{}", module.ir);
    }
    ExitCode::SUCCESS
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
    let program = time_stage(timings, "check", || {
        nia_driver::check_program_with_map_options_and_timings(
            path,
            module_map,
            optimization,
            timings,
        )
    });
    if !program.diagnostics.is_empty() {
        print_program_diagnostics(path, source, &program);
        return ExitCode::FAILURE;
    }
    if opt_report {
        print_optimization_report_to_stderr(&program);
    }
    let output = time_stage(timings, "emit_native_objects", || {
        nia_codegen_llvm::emit_native_objects(
            &program.backend_lowering.program,
            codegen_options(program.optimization),
        )
    });
    if !output.diagnostics.is_empty() {
        print_codegen_diagnostics(path, source, &output.diagnostics);
        return ExitCode::FAILURE;
    }

    match options {
        EmitObjOptions::Single(path) => {
            if output.modules.len() != 1 {
                eprintln!(
                    "`-o` can only be used when the program has one codegen unit; use `--out-dir`"
                );
                return ExitCode::FAILURE;
            }
            if let Err(err) = write_output_file(&path, &output.modules[0].bytes) {
                eprintln!("failed to write `{}`: {err}", path.display());
                return ExitCode::FAILURE;
            }
        }
        EmitObjOptions::Directory(dir) => {
            if let Err(err) = fs::create_dir_all(&dir) {
                eprintln!("failed to create `{}`: {err}", dir.display());
                return ExitCode::FAILURE;
            }
            for (index, module) in output.modules.iter().enumerate() {
                let path = dir.join(object_file_name(index, &module.name));
                if let Err(err) = write_output_file(&path, &module.bytes) {
                    eprintln!("failed to write `{}`: {err}", path.display());
                    return ExitCode::FAILURE;
                }
            }
        }
    }
    ExitCode::SUCCESS
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
    let output_path = match parse_emit_exe_options(path, args) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let program = time_stage(timings, "check_exe", || {
        nia_driver::check_freestanding_executable_with_map_options_and_timings(
            path,
            module_map,
            optimization,
            timings,
        )
    });
    if !program.diagnostics.is_empty() {
        print_program_diagnostics(path, source, &program);
        return ExitCode::FAILURE;
    }
    if opt_report {
        print_optimization_report_to_stderr(&program);
    }
    let output = time_stage(timings, "emit_native_objects", || {
        nia_codegen_llvm::emit_native_objects(
            &program.backend_lowering.program,
            codegen_options(program.optimization),
        )
    });
    if !output.diagnostics.is_empty() {
        print_codegen_diagnostics(path, source, &output.diagnostics);
        return ExitCode::FAILURE;
    }

    let temp = TempDir::new("nia_emit_exe");
    if let Err(err) = fs::create_dir_all(temp.path()) {
        eprintln!("failed to create `{}`: {err}", temp.path().display());
        return ExitCode::FAILURE;
    }
    let Some(object_paths) = time_stage(timings, "write_temp_objects", || {
        let mut object_paths = Vec::new();
        for (index, module) in output.modules.iter().enumerate() {
            let object_path = temp.path().join(object_file_name(index, &module.name));
            if let Err(err) = write_output_file(&object_path, &module.bytes) {
                eprintln!("failed to write `{}`: {err}", object_path.display());
                return None;
            }
            object_paths.push(object_path);
        }
        Some(object_paths)
    }) else {
        return ExitCode::FAILURE;
    };
    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(err) = fs::create_dir_all(parent)
    {
        eprintln!("failed to create `{}`: {err}", parent.display());
        return ExitCode::FAILURE;
    }

    let linker = executable_linker();
    let status = time_stage(timings, "link_executable", || {
        Command::new(&linker.program)
            .args(&linker.args_before_objects)
            .args(&object_paths)
            .args(&linker.args_after_objects)
            .arg("-o")
            .arg(&output_path)
            .status()
    });
    match status {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => {
            eprintln!("linker `{}` failed with status {status}", linker.program);
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("failed to run linker `{}`: {err}", linker.program);
            ExitCode::FAILURE
        }
    }
}

struct ExecutableLinker {
    program: String,
    args_before_objects: Vec<String>,
    args_after_objects: Vec<String>,
}

fn executable_linker() -> ExecutableLinker {
    if let Ok(program) = env::var("NIA_LINKER")
        && !program.is_empty()
    {
        return ExecutableLinker {
            program,
            args_before_objects: Vec::new(),
            args_after_objects: vec!["-e".to_string(), "_start".to_string()],
        };
    }
    ExecutableLinker {
        program: "ld".to_string(),
        args_before_objects: Vec::new(),
        args_after_objects: vec!["-e".to_string(), "_start".to_string()],
    }
}

fn codegen_options(optimization: OptimizationPolicy) -> nia_codegen_llvm::LlvmCodegenOptions {
    nia_codegen_llvm::LlvmCodegenOptions { optimization }
}

enum EmitObjOptions {
    Single(PathBuf),
    Directory(PathBuf),
}

fn parse_emit_obj_options(source: &str, args: Vec<String>) -> Result<EmitObjOptions, String> {
    let mut output = None::<PathBuf>;
    let mut out_dir = None::<PathBuf>;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
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
            _ => return Err(format!("unknown `nia emit --obj` option `{arg}`")),
        }
    }
    match (output, out_dir) {
        (Some(_), Some(_)) => Err("use either `-o` or `--out-dir`, not both".to_string()),
        (Some(path), None) => Ok(EmitObjOptions::Single(path)),
        (None, Some(path)) => Ok(EmitObjOptions::Directory(path)),
        (None, None) => Ok(EmitObjOptions::Single(default_output_path(source, "o"))),
    }
}

fn parse_emit_exe_options(source: &str, args: Vec<String>) -> Result<PathBuf, String> {
    let mut output = None::<PathBuf>;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-o" => {
                let Some(path) = iter.next() else {
                    return Err("missing path after `-o`".to_string());
                };
                output = Some(PathBuf::from(path));
            }
            _ => return Err(format!("unknown `nia emit --exe` option `{arg}`")),
        }
    }
    Ok(output.unwrap_or_else(|| default_output_path(source, env::consts::EXE_EXTENSION)))
}

fn default_output_path(source: &str, extension: &str) -> PathBuf {
    let mut path = PathBuf::from(source);
    path.set_extension(extension);
    path
}

fn write_output_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)
}

fn object_file_name(index: usize, module_name: &str) -> String {
    let stem = Path::new(module_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("module");
    let clean = stem
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("{index:04}_{clean}.o")
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let mut path = env::temp_dir();
        path.push(format!(
            "{prefix}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn print_parse_errors(path: &str, source: &str, errors: &[ParseError]) {
    eprintln!("parse errors:");
    for error in errors {
        let diagnostic = Diagnostic::user_error_at("E0103", error.span, error.message.clone());
        eprintln!("{}", render_diagnostic(path, source, &diagnostic));
    }
}

fn print_program_diagnostics(path: &str, source: &str, program: &nia_driver::CheckedProgram) {
    eprintln!("diagnostics:");
    for diagnostic in &program.diagnostics {
        let loaded_source = if diagnostic.path.as_str() == path {
            None
        } else {
            fs::read_to_string(diagnostic.path.as_str()).ok()
        };
        let source = loaded_source.as_deref().unwrap_or(source);
        eprintln!(
            "{}",
            render_diagnostic(diagnostic.path.as_str(), source, &diagnostic.diagnostic)
        );
    }
}

fn print_codegen_diagnostics(path: &str, source: &str, diagnostics: &[nia_diagnostic::Diagnostic]) {
    eprintln!("codegen diagnostics:");
    for diagnostic in diagnostics {
        eprintln!("{}", render_diagnostic(path, source, diagnostic));
    }
}

fn print_optimization_report(program: &nia_driver::CheckedProgram) {
    print_optimization_report_with(program, |line| println!("{line}"));
}

fn print_optimization_report_to_stderr(program: &nia_driver::CheckedProgram) {
    print_optimization_report_with(program, |line| eprintln!("{line}"));
}

fn print_optimization_report_with(
    program: &nia_driver::CheckedProgram,
    mut print_line: impl FnMut(String),
) {
    let report = &program.backend_lowering.optimization_report;
    let policy = program.optimization;
    print_line("backend optimization report:".to_string());
    print_line(format!(
        "  policy level={} simplify_cfg={} const_fold={} dead_code_elim={} \
         local_copy_prop={} inline={} specialize={} dedup_monomorphized_instances={} \
         prefer_size={} llvm_codegen={} llvm_size={}",
        optimization_level_name(policy.level),
        optimization_depth_name(policy.simplify_cfg),
        optimization_depth_name(policy.const_fold),
        optimization_depth_name(policy.dead_code_elim),
        optimization_depth_name(policy.local_copy_prop),
        inline_threshold_name(policy.inline_threshold),
        specialization_policy_name(policy.specialize_generics),
        policy.dedup_monomorphized_instances,
        policy.prefer_size,
        nia_codegen_llvm::llvm_codegen_optimization_level(policy.level).name(),
        nia_codegen_llvm::llvm_codegen_size_policy(policy.level).name()
    ));
    print_line(format!(
        "  enabled_module_passes={}",
        enabled_passes_name(&report.enabled_module_passes)
    ));
    print_line(format!(
        "  enabled_function_passes={}",
        enabled_passes_name(&report.enabled_function_passes)
    ));
    print_line(format!(
        "  enabled_global_passes={}",
        enabled_passes_name(&report.enabled_global_passes)
    ));
    print_line(format!("  changes={}", report.changed_passes.len()));
    if report.changed_passes.is_empty() {
        print_line("  no changes".to_string());
        return;
    }
    for change in &report.changed_passes {
        match change {
            nia_driver::BackendOptimizationChange::Function {
                function,
                pass,
                is_instance,
                type_arg_count,
                ..
            } => {
                let instance = if *is_instance { " instance" } else { "" };
                print_line(format!(
                    "  m{}::d{}{} {} type_args={}",
                    function.module_id.0, function.def_id.0, instance, pass, type_arg_count
                ));
            }
            nia_driver::BackendOptimizationChange::Global { global, pass, .. } => {
                print_line(format!(
                    "  m{}::d{} global {}",
                    global.module_id.0, global.def_id.0, pass
                ));
            }
        }
    }
}

fn enabled_passes_name(passes: &[&'static str]) -> String {
    if passes.is_empty() {
        "none".to_string()
    } else {
        passes.join(",")
    }
}

fn optimization_level_name(level: NiaOptimizationLevel) -> &'static str {
    match level {
        NiaOptimizationLevel::O0 => "O0",
        NiaOptimizationLevel::O1 => "O1",
        NiaOptimizationLevel::O2 => "O2",
        NiaOptimizationLevel::O3 => "O3",
        NiaOptimizationLevel::Os => "Os",
        NiaOptimizationLevel::Oz => "Oz",
    }
}

fn optimization_depth_name(depth: OptimizationDepth) -> &'static str {
    match depth {
        OptimizationDepth::Disabled => "disabled",
        OptimizationDepth::Required => "required",
        OptimizationDepth::Cheap => "cheap",
        OptimizationDepth::Full => "full",
        OptimizationDepth::Aggressive => "aggressive",
    }
}

fn inline_threshold_name(threshold: InlineThreshold) -> &'static str {
    match threshold {
        InlineThreshold::Never => "never",
        InlineThreshold::Minimal => "minimal",
        InlineThreshold::Size => "size",
        InlineThreshold::Small => "small",
        InlineThreshold::Normal => "normal",
        InlineThreshold::Aggressive => "aggressive",
    }
}

fn specialization_policy_name(policy: SpecializationPolicy) -> &'static str {
    match policy {
        SpecializationPolicy::RequiredOnly => "required-only",
        SpecializationPolicy::SizeAware => "size-aware",
        SpecializationPolicy::Normal => "normal",
        SpecializationPolicy::Aggressive => "aggressive",
    }
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
