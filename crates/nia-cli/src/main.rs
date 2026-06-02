// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

use nia_diagnostic::{Diagnostic, render_diagnostic};
use nia_ids::ModuleId;
use nia_imports::ModuleMap;
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
            println!("niac {}", env!("CARGO_PKG_VERSION"));
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
    command: CliCommand,
}

#[derive(Debug)]
enum CliCommand {
    Lex {
        path: String,
    },
    Parse {
        path: String,
    },
    Check {
        path: String,
        emit_opt_report: bool,
    },
    EmitBackend {
        path: String,
        emit_opt_report: bool,
    },
    EmitLlvm {
        path: String,
        emit_opt_report: bool,
    },
    EmitObj {
        path: String,
        args: Vec<String>,
        emit_opt_report: bool,
    },
    EmitExe {
        path: String,
        args: Vec<String>,
        emit_opt_report: bool,
    },
}

enum CliAction {
    Help(HelpTopic),
    Version,
    Run(Cli),
}

#[derive(Clone, Copy)]
enum HelpTopic {
    Main,
    Lex,
    Parse,
    Check,
    Emit,
    EmitBackend,
    EmitLlvm,
    EmitObj,
    EmitExe,
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
        CliCommand::Lex { .. } => run_lex(&source),
        CliCommand::Parse { .. } => run_parse(&path, &source),
        CliCommand::Check {
            emit_opt_report, ..
        } => run_check(
            &path,
            &source,
            cli.module_map,
            cli.optimization,
            emit_opt_report,
        ),
        CliCommand::EmitLlvm {
            emit_opt_report, ..
        } => run_emit_llvm(
            &path,
            &source,
            cli.module_map,
            cli.optimization,
            emit_opt_report,
        ),
        CliCommand::EmitBackend {
            emit_opt_report, ..
        } => run_emit_backend(
            &path,
            &source,
            cli.module_map,
            cli.optimization,
            emit_opt_report,
        ),
        CliCommand::EmitObj {
            args,
            emit_opt_report,
            ..
        } => run_emit_obj(
            &path,
            &source,
            args,
            cli.module_map,
            cli.optimization,
            emit_opt_report,
        ),
        CliCommand::EmitExe {
            args,
            emit_opt_report,
            ..
        } => run_emit_exe(
            &path,
            &source,
            args,
            cli.module_map,
            cli.optimization,
            emit_opt_report,
        ),
    }
}

fn read_source_for_command(command: &CliCommand) -> Result<(String, String), String> {
    let path = match command {
        CliCommand::Lex { path }
        | CliCommand::Parse { path }
        | CliCommand::Check { path, .. }
        | CliCommand::EmitBackend { path, .. }
        | CliCommand::EmitLlvm { path, .. }
        | CliCommand::EmitObj { path, .. }
        | CliCommand::EmitExe { path, .. } => path,
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
            command,
        })),
    }
}

struct GlobalOptions {
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
}

fn extract_global_options(
    args: Vec<String>,
    help: HelpTopic,
) -> Result<(Vec<String>, GlobalOptions), CliError> {
    let mut map = ModuleMap::new();
    let mut optimization = NiaOptimizationLevel::default();
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
        remaining.push(arg);
    }
    Ok((
        remaining,
        GlobalOptions {
            module_map: map,
            optimization,
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
        "lex" => parse_source_command("lex", rest, HelpTopic::Lex, |path| CliCommand::Lex { path })
            .map(ParsedCommand::Run),
        "parse" => parse_source_command("parse", rest, HelpTopic::Parse, |path| {
            CliCommand::Parse { path }
        })
        .map(ParsedCommand::Run),
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
        [command] if command == "lex" => HelpTopic::Lex,
        [command] if command == "parse" => HelpTopic::Parse,
        [command] if command == "check" => HelpTopic::Check,
        [command] if command == "emit" => HelpTopic::Emit,
        [command, target] if command == "emit" && target == "backend" => HelpTopic::EmitBackend,
        [command, target] if command == "emit" && target == "llvm" => HelpTopic::EmitLlvm,
        [command, target] if command == "emit" && target == "obj" => HelpTopic::EmitObj,
        [command, target] if command == "emit" && target == "exe" => HelpTopic::EmitExe,
        _ => HelpTopic::Main,
    }
}

fn parse_source_command(
    command: &str,
    args: Vec<String>,
    help: HelpTopic,
    build: impl FnOnce(String) -> CliCommand,
) -> Result<CliCommand, CliError> {
    if has_help_flag(&args) {
        return Err(CliError::help(help));
    }
    let mut args = args.into_iter();
    let Some(path) = args.next() else {
        return Err(CliError::new(
            format!("missing source file for `niac {command}`"),
            help,
        ));
    };
    if let Some(extra) = args.next() {
        return Err(CliError::new(
            format!("unexpected argument `{extra}` for `niac {command}`"),
            help,
        ));
    }
    Ok(build(path))
}

fn parse_check_command(args: Vec<String>) -> Result<CliCommand, CliError> {
    if has_help_flag(&args) {
        return Err(CliError::help(HelpTopic::Check));
    }
    let mut path = None;
    let mut emit_opt_report = false;
    for arg in args {
        match arg.as_str() {
            "--emit-opt-report" => emit_opt_report = true,
            _ if path.is_none() => path = Some(arg),
            _ => {
                return Err(CliError::new(
                    format!("unexpected argument `{arg}` for `niac check`"),
                    HelpTopic::Check,
                ));
            }
        }
    }
    let Some(path) = path else {
        return Err(CliError::new(
            "missing source file for `niac check`",
            HelpTopic::Check,
        ));
    };
    Ok(CliCommand::Check {
        path,
        emit_opt_report,
    })
}

fn parse_emit_command(args: Vec<String>) -> Result<CliCommand, CliError> {
    if args.is_empty() {
        return Err(CliError::help(HelpTopic::Emit));
    }
    if args.len() == 1 && has_help_flag(&args) {
        return Err(CliError::help(HelpTopic::Emit));
    }
    let mut args = args.into_iter();
    let Some(target) = args.next() else {
        return Err(CliError::help(HelpTopic::Emit));
    };
    let rest = args.collect::<Vec<_>>();
    if has_help_flag(&rest) {
        return Err(CliError::help(match target.as_str() {
            "backend" => HelpTopic::EmitBackend,
            "llvm" => HelpTopic::EmitLlvm,
            "obj" => HelpTopic::EmitObj,
            "exe" => HelpTopic::EmitExe,
            _ => HelpTopic::Emit,
        }));
    }
    match target.as_str() {
        "backend" => parse_emit_backend_command(rest),
        "llvm" => parse_emit_llvm_command(rest),
        "obj" => parse_emit_with_options(
            "emit obj",
            rest,
            HelpTopic::EmitObj,
            |path, args, emit_opt_report| CliCommand::EmitObj {
                path,
                args,
                emit_opt_report,
            },
        ),
        "exe" => parse_emit_with_options(
            "emit exe",
            rest,
            HelpTopic::EmitExe,
            |path, args, emit_opt_report| CliCommand::EmitExe {
                path,
                args,
                emit_opt_report,
            },
        ),
        _ => Err(CliError::new(
            format!("unknown emit target `{target}`"),
            HelpTopic::Emit,
        )),
    }
}

fn parse_emit_backend_command(args: Vec<String>) -> Result<CliCommand, CliError> {
    if has_help_flag(&args) {
        return Err(CliError::help(HelpTopic::EmitBackend));
    }
    let mut path = None;
    let mut emit_opt_report = false;
    for arg in args {
        match arg.as_str() {
            "--emit-opt-report" => emit_opt_report = true,
            _ if path.is_none() => path = Some(arg),
            _ => {
                return Err(CliError::new(
                    format!("unexpected argument `{arg}` for `niac emit backend`"),
                    HelpTopic::EmitBackend,
                ));
            }
        }
    }
    let Some(path) = path else {
        return Err(CliError::new(
            "missing source file for `niac emit backend`",
            HelpTopic::EmitBackend,
        ));
    };
    Ok(CliCommand::EmitBackend {
        path,
        emit_opt_report,
    })
}

fn parse_emit_llvm_command(args: Vec<String>) -> Result<CliCommand, CliError> {
    if has_help_flag(&args) {
        return Err(CliError::help(HelpTopic::EmitLlvm));
    }
    let mut path = None;
    let mut emit_opt_report = false;
    for arg in args {
        match arg.as_str() {
            "--emit-opt-report" => emit_opt_report = true,
            _ if path.is_none() => path = Some(arg),
            _ => {
                return Err(CliError::new(
                    format!("unexpected argument `{arg}` for `niac emit llvm`"),
                    HelpTopic::EmitLlvm,
                ));
            }
        }
    }
    let Some(path) = path else {
        return Err(CliError::new(
            "missing source file for `niac emit llvm`",
            HelpTopic::EmitLlvm,
        ));
    };
    Ok(CliCommand::EmitLlvm {
        path,
        emit_opt_report,
    })
}

fn parse_emit_with_options(
    command: &str,
    args: Vec<String>,
    help: HelpTopic,
    build: impl FnOnce(String, Vec<String>, bool) -> CliCommand,
) -> Result<CliCommand, CliError> {
    if has_help_flag(&args) {
        return Err(CliError::help(help));
    }
    let mut emit_opt_report = false;
    let mut path = None;
    let mut target_args = Vec::new();
    let mut preserve_next = false;
    for arg in args {
        if path.is_none() {
            if arg == "--emit-opt-report" {
                emit_opt_report = true;
                continue;
            }
            path = Some(arg);
            continue;
        }
        if preserve_next {
            preserve_next = false;
            target_args.push(arg);
            continue;
        }
        if arg == "--emit-opt-report" {
            emit_opt_report = true;
            continue;
        }
        if native_emit_option_takes_path(&arg) {
            preserve_next = true;
        }
        target_args.push(arg);
    }
    let Some(path) = path else {
        return Err(CliError::new(
            format!("missing source file for `niac {command}`"),
            help,
        ));
    };
    Ok(build(path, target_args, emit_opt_report))
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
    emit_opt_report: bool,
) -> ExitCode {
    let program = nia_driver::check_program_with_map_and_options(path, module_map, optimization);
    if !program.diagnostics.is_empty() {
        print_program_diagnostics(path, source, &program);
        return ExitCode::FAILURE;
    }
    if emit_opt_report {
        print_optimization_report(&program);
    }
    ExitCode::SUCCESS
}

fn run_emit_backend(
    path: &str,
    source: &str,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    emit_opt_report: bool,
) -> ExitCode {
    let program = nia_driver::check_program_with_map_and_options(path, module_map, optimization);
    if !program.diagnostics.is_empty() {
        print_program_diagnostics(path, source, &program);
        return ExitCode::FAILURE;
    }
    if emit_opt_report {
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
    emit_opt_report: bool,
) -> ExitCode {
    let program = nia_driver::check_program_with_map_and_options(path, module_map, optimization);
    if !program.diagnostics.is_empty() {
        print_program_diagnostics(path, source, &program);
        return ExitCode::FAILURE;
    }
    if emit_opt_report {
        print_optimization_report_to_stderr(&program);
    }
    let output = nia_codegen_llvm::emit_llvm_ir_with_options(
        &program.backend_lowering.program,
        codegen_options(program.graph.root(), false, program.optimization),
    );
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
    emit_opt_report: bool,
) -> ExitCode {
    let options = match parse_emit_obj_options(path, args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let program = nia_driver::check_program_with_map_and_options(path, module_map, optimization);
    if !program.diagnostics.is_empty() {
        print_program_diagnostics(path, source, &program);
        return ExitCode::FAILURE;
    }
    if emit_opt_report {
        print_optimization_report_to_stderr(&program);
    }
    let output = nia_codegen_llvm::emit_native_objects(
        &program.backend_lowering.program,
        hosted_codegen_options(program.graph.root(), program.optimization),
    );
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
    emit_opt_report: bool,
) -> ExitCode {
    let output_path = match parse_emit_exe_options(path, args) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let program = nia_driver::check_program_with_map_and_options(path, module_map, optimization);
    if !program.diagnostics.is_empty() {
        print_program_diagnostics(path, source, &program);
        return ExitCode::FAILURE;
    }
    if emit_opt_report {
        print_optimization_report_to_stderr(&program);
    }
    let output = nia_codegen_llvm::emit_native_objects(
        &program.backend_lowering.program,
        hosted_codegen_options(program.graph.root(), program.optimization),
    );
    if !output.diagnostics.is_empty() {
        print_codegen_diagnostics(path, source, &output.diagnostics);
        return ExitCode::FAILURE;
    }

    let temp = TempDir::new("nia_emit_exe");
    if let Err(err) = fs::create_dir_all(temp.path()) {
        eprintln!("failed to create `{}`: {err}", temp.path().display());
        return ExitCode::FAILURE;
    }
    let mut object_paths = Vec::new();
    for (index, module) in output.modules.iter().enumerate() {
        let object_path = temp.path().join(object_file_name(index, &module.name));
        if let Err(err) = write_output_file(&object_path, &module.bytes) {
            eprintln!("failed to write `{}`: {err}", object_path.display());
            return ExitCode::FAILURE;
        }
        object_paths.push(object_path);
    }
    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(err) = fs::create_dir_all(parent)
    {
        eprintln!("failed to create `{}`: {err}", parent.display());
        return ExitCode::FAILURE;
    }

    let linker = env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let status = Command::new(&linker)
        .args(&object_paths)
        .arg("-o")
        .arg(&output_path)
        .status();
    match status {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => {
            eprintln!("linker `{linker}` failed with status {status}");
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("failed to run linker `{linker}`: {err}");
            ExitCode::FAILURE
        }
    }
}

fn hosted_codegen_options(
    root_module: ModuleId,
    optimization: OptimizationPolicy,
) -> nia_codegen_llvm::LlvmCodegenOptions {
    codegen_options(root_module, true, optimization)
}

fn codegen_options(
    root_module: ModuleId,
    hosted_entry: bool,
    optimization: OptimizationPolicy,
) -> nia_codegen_llvm::LlvmCodegenOptions {
    nia_codegen_llvm::LlvmCodegenOptions {
        root_module: Some(root_module),
        hosted_entry,
        optimization,
    }
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
            _ => return Err(format!("unknown `niac emit obj` option `{arg}`")),
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
            _ => return Err(format!("unknown `niac emit exe` option `{arg}`")),
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
        let diagnostic = Diagnostic::error(error.span, error.message.clone());
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
