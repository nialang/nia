// SPDX-License-Identifier: GPL-3.0-or-later
//! Table-driven command-line option scanning.
//!
//! Every option the CLI accepts is described once in an [`OptionSpec`]. The
//! same table drives scanning, value handling, and the help rows, so an
//! option cannot be parsed without being documented or documented without
//! being parsed.
//!
//! Scanning follows one set of rules for every command:
//!
//! - `--name value` and `--name=value` are equivalent for every option that
//!   takes a value; a short option `-x value` may also be written `-xvalue`.
//! - The argument after a value-taking option is always its value, even when
//!   it starts with `-`, so `--filter -O` filters by `-O`.
//! - `--` ends option scanning; every later argument is positional.
//! - A repeated single-value option keeps its last value. Repeatable options
//!   (`-M`, `-L`, `-l`, `--link-arg`, `--rpath`) accumulate.
//! - An empty value is rejected for every option.
//! - `-h`/`--help` is a request for help only where an option may appear.

use std::fmt;

/// How an option consumes the arguments that follow it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Arity {
    /// A switch that takes no value.
    Flag,
    /// An option that takes exactly one value.
    Value,
    /// An option whose value is optional and may only be attached with `=`.
    OptionalValue,
}

/// Commands an option belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Scope(u16);

impl Scope {
    pub(crate) const GLOBAL: Self = Self(1);
    pub(crate) const BUILD: Self = Self(1 << 1);
    pub(crate) const CHECK: Self = Self(1 << 3);
    /// `emit --tokens` and `emit --ast`.
    pub(crate) const EMIT_INSPECT: Self = Self(1 << 4);
    /// `emit --checked`.
    pub(crate) const EMIT_CHECKED: Self = Self(1 << 5);
    /// `emit --backend` and `emit --llvm`.
    pub(crate) const EMIT_CODEGEN: Self = Self(1 << 6);
    pub(crate) const EMIT_OBJ: Self = Self(1 << 7);
    pub(crate) const EMIT_EXE: Self = Self(1 << 8);
    pub(crate) const UPDATE: Self = Self(1 << 9);
    /// Every emit kind, used before the kind is known.
    pub(crate) const EMIT_ANY: Self = Self::EMIT_INSPECT
        .or(Self::EMIT_CHECKED)
        .or(Self::EMIT_CODEGEN)
        .or(Self::EMIT_OBJ)
        .or(Self::EMIT_EXE);

    pub(crate) const fn or(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub(crate) const fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

/// The product `nia emit` writes, selected by a target flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EmitKind {
    Tokens,
    Ast,
    Checked,
    Backend,
    Llvm,
    Obj,
    Exe,
}

impl EmitKind {
    pub(crate) const ALL: [Self; 7] = [
        Self::Tokens,
        Self::Ast,
        Self::Checked,
        Self::Backend,
        Self::Llvm,
        Self::Obj,
        Self::Exe,
    ];

    pub(crate) fn flag(self) -> &'static str {
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

    /// The options scope of this kind.
    pub(crate) fn scope(self) -> Scope {
        match self {
            Self::Tokens | Self::Ast => Scope::EMIT_INSPECT,
            Self::Checked => Scope::EMIT_CHECKED,
            Self::Backend | Self::Llvm => Scope::EMIT_CODEGEN,
            Self::Obj => Scope::EMIT_OBJ,
            Self::Exe => Scope::EMIT_EXE,
        }
    }

    pub(crate) fn from_flag(arg: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.flag() == arg)
    }

    /// Matches a target written without its leading dashes, such as `exe`.
    pub(crate) fn from_bare_name(arg: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.flag().strip_prefix("--") == Some(arg))
    }
}

/// Stable identity of a recognized option.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum OptionId {
    EmitTarget(EmitKind),
    Help,
    Version,
    Optimization,
    Debug,
    Release,
    Profile,
    Module,
    Timings,
    TimingsFormat,
    TimingTrace,
    DiagnosticsFormat,
    ResourceRoot,
    Target,
    Root,
    Package,
    Workspace,
    Locked,
    Offline,
    Jobs,
    Quiet,
    Verbose,
    Progress,
    Lto,
    DryRun,
    Explain,
    PrintPlan,
    PrintCache,
    PrintObservations,
    List,
    KeepGoing,
    Runtime,
    OptReport,
    CacheDir,
    Output,
    OutDir,
    LinkArg,
    Linker,
    LinkerFlavor,
    Executable,
    Interpreter,
    LibraryPath,
    Library,
    Rpath,
}

/// One accepted option and its help row.
pub(crate) struct OptionSpec {
    pub(crate) id: OptionId,
    /// Spellings, e.g. `["-j", "--jobs"]`. Short spellings come first.
    pub(crate) names: &'static [&'static str],
    pub(crate) arity: Arity,
    pub(crate) scope: Scope,
    /// Left column of the help row.
    pub(crate) usage: &'static str,
    /// Right column of the help row.
    pub(crate) about: &'static str,
}

/// Emit kinds that run the compiler past parsing.
const EMIT_COMPILED: Scope = Scope::EMIT_CHECKED
    .or(Scope::EMIT_CODEGEN)
    .or(Scope::EMIT_OBJ)
    .or(Scope::EMIT_EXE);
/// Emit kinds that produce optimized code.
const EMIT_OPTIMIZED: Scope = Scope::EMIT_CODEGEN.or(Scope::EMIT_OBJ).or(Scope::EMIT_EXE);

/// Every option, in the order help lists them. An empty `usage` hides an
/// entry from the option rows: emit target flags are listed as targets, and
/// `--release`/`--profile` share the `--debug` row.
pub(crate) const OPTIONS: &[OptionSpec] = &[
    OptionSpec {
        id: OptionId::EmitTarget(EmitKind::Tokens),
        names: &["--tokens"],
        arity: Arity::Flag,
        scope: Scope::EMIT_ANY,
        usage: "",
        about: "print source tokens",
    },
    OptionSpec {
        id: OptionId::EmitTarget(EmitKind::Ast),
        names: &["--ast"],
        arity: Arity::Flag,
        scope: Scope::EMIT_ANY,
        usage: "",
        about: "print the parsed syntax tree",
    },
    OptionSpec {
        id: OptionId::EmitTarget(EmitKind::Checked),
        names: &["--checked"],
        arity: Arity::Flag,
        scope: Scope::EMIT_ANY,
        usage: "",
        about: "print the checked program",
    },
    OptionSpec {
        id: OptionId::EmitTarget(EmitKind::Backend),
        names: &["--backend"],
        arity: Arity::Flag,
        scope: Scope::EMIT_ANY,
        usage: "",
        about: "print optimized Nia backend IR",
    },
    OptionSpec {
        id: OptionId::EmitTarget(EmitKind::Llvm),
        names: &["--llvm"],
        arity: Arity::Flag,
        scope: Scope::EMIT_ANY,
        usage: "",
        about: "print LLVM IR",
    },
    OptionSpec {
        id: OptionId::EmitTarget(EmitKind::Obj),
        names: &["--obj"],
        arity: Arity::Flag,
        scope: Scope::EMIT_ANY,
        usage: "",
        about: "write native object files",
    },
    OptionSpec {
        id: OptionId::EmitTarget(EmitKind::Exe),
        names: &["--exe"],
        arity: Arity::Flag,
        scope: Scope::EMIT_ANY,
        usage: "",
        about: "build a freestanding executable",
    },
    OptionSpec {
        id: OptionId::Root,
        names: &["--root"],
        arity: Arity::Value,
        scope: Scope::BUILD.or(Scope::UPDATE),
        usage: "--root <dir>",
        about: "start package discovery here instead of the current directory",
    },
    OptionSpec {
        id: OptionId::Package,
        names: &["-p", "--package"],
        arity: Arity::Value,
        scope: Scope::BUILD,
        usage: "-p, --package <name>",
        about: "select a workspace member; may be repeated",
    },
    OptionSpec {
        id: OptionId::Workspace,
        names: &["--workspace"],
        arity: Arity::Flag,
        scope: Scope::BUILD,
        usage: "--workspace",
        about: "select every workspace member",
    },
    OptionSpec {
        id: OptionId::Locked,
        names: &["--locked"],
        arity: Arity::Flag,
        scope: Scope::BUILD,
        usage: "--locked",
        about: "fail instead of changing nia.lock",
    },
    OptionSpec {
        id: OptionId::Offline,
        names: &["--offline"],
        arity: Arity::Flag,
        scope: Scope::BUILD,
        usage: "--offline",
        about: "use cached Git and registry dependencies without fetching",
    },
    OptionSpec {
        id: OptionId::List,
        names: &["--list"],
        arity: Arity::Flag,
        scope: Scope::BUILD,
        usage: "--list",
        about: "list the package's steps and their kinds without running any",
    },
    OptionSpec {
        id: OptionId::KeepGoing,
        names: &["-k", "--keep-going"],
        arity: Arity::Flag,
        scope: Scope::BUILD,
        usage: "-k, --keep-going",
        about: "keep running actions that do not depend on a failed one",
    },
    OptionSpec {
        id: OptionId::Jobs,
        names: &["-j", "--jobs"],
        arity: Arity::Value,
        scope: Scope::BUILD,
        usage: "-j, --jobs <count>",
        about: "limit concurrent build actions",
    },
    OptionSpec {
        id: OptionId::DryRun,
        names: &["--dry-run"],
        arity: Arity::Flag,
        scope: Scope::BUILD,
        usage: "--dry-run",
        about: "configure and freeze the plan without executing actions",
    },
    OptionSpec {
        id: OptionId::Explain,
        names: &["--explain"],
        arity: Arity::Flag,
        scope: Scope::BUILD,
        usage: "--explain",
        about: "show selected graph and configure-cache inputs",
    },
    OptionSpec {
        id: OptionId::PrintPlan,
        names: &["--print-plan"],
        arity: Arity::Flag,
        scope: Scope::BUILD,
        usage: "--print-plan",
        about: "print a deterministic frozen-plan summary",
    },
    OptionSpec {
        id: OptionId::PrintCache,
        names: &["--print-cache"],
        arity: Arity::Flag,
        scope: Scope::BUILD,
        usage: "--print-cache",
        about: "print runner, configure, and action cache decisions",
    },
    OptionSpec {
        id: OptionId::PrintObservations,
        names: &["--print-observations"],
        arity: Arity::Flag,
        scope: Scope::BUILD,
        usage: "--print-observations",
        about: "print configuration filesystem observations",
    },
    OptionSpec {
        id: OptionId::Quiet,
        names: &["-q", "--quiet"],
        arity: Arity::Flag,
        scope: Scope::BUILD,
        usage: "-q, --quiet",
        about: "suppress build progress output",
    },
    OptionSpec {
        id: OptionId::Verbose,
        names: &["-v", "--verbose"],
        arity: Arity::Flag,
        scope: Scope::BUILD,
        usage: "-v, -vv, -vvv, --verbose",
        about: "show action, scheduler, and cache detail; repeat for more",
    },
    OptionSpec {
        id: OptionId::Progress,
        names: &["--progress"],
        arity: Arity::Value,
        scope: Scope::BUILD,
        usage: "--progress <auto|plain|json|none>",
        about: "select progress transport",
    },
    OptionSpec {
        id: OptionId::Output,
        names: &["-o"],
        arity: Arity::Value,
        scope: Scope::EMIT_OBJ.or(Scope::EMIT_EXE),
        usage: "-o <file>",
        about: "write the output to this path",
    },
    OptionSpec {
        id: OptionId::OutDir,
        names: &["--out-dir"],
        arity: Arity::Value,
        scope: Scope::EMIT_OBJ,
        usage: "--out-dir <dir>",
        about: "write one object file per codegen unit",
    },
    OptionSpec {
        id: OptionId::Runtime,
        names: &["--runtime"],
        arity: Arity::Value,
        scope: Scope::CHECK.or(EMIT_COMPILED),
        usage: "--runtime <bare|freestanding>",
        about: "select the program runtime (default: bare; executables require freestanding)",
    },
    OptionSpec {
        id: OptionId::CacheDir,
        names: &["--cache-dir"],
        arity: Arity::Value,
        scope: Scope::CHECK.or(Scope::EMIT_OBJ).or(Scope::EMIT_EXE),
        usage: "--cache-dir <path>",
        about: "reuse compiler artifacts from this directory",
    },
    OptionSpec {
        id: OptionId::OptReport,
        names: &["--opt-report"],
        arity: Arity::Flag,
        scope: Scope::CHECK.or(EMIT_OPTIMIZED),
        usage: "--opt-report",
        about: "print the optimization report to stderr",
    },
    OptionSpec {
        id: OptionId::Lto,
        names: &["--lto"],
        arity: Arity::Value,
        scope: Scope::BUILD.or(Scope::EMIT_EXE),
        usage: "--lto <off|thin|full>",
        about: "select final executable link-time optimization",
    },
    OptionSpec {
        id: OptionId::LinkArg,
        names: &["--link-arg"],
        arity: Arity::Value,
        scope: Scope::EMIT_EXE,
        usage: "--link-arg <arg>",
        about: "pass an argument to the linker; may be repeated",
    },
    OptionSpec {
        id: OptionId::Linker,
        names: &["--linker"],
        arity: Arity::Value,
        scope: Scope::EMIT_EXE,
        usage: "--linker <program>",
        about: "use this linker program (default: LLD for the target)",
    },
    OptionSpec {
        id: OptionId::LinkerFlavor,
        names: &["--linker-flavor"],
        arity: Arity::Value,
        scope: Scope::EMIT_EXE,
        usage: "--linker-flavor <flavor>",
        about: "gnu, lld, or self-hosted-elf for ELF, lld-link for COFF, ld64.lld for Mach-O",
    },
    OptionSpec {
        id: OptionId::Executable,
        names: &["--executable"],
        arity: Arity::Value,
        scope: Scope::EMIT_EXE,
        usage: "--executable <static-pie|static|dynamic>",
        about: "select the ELF executable form (default: static-pie)",
    },
    OptionSpec {
        id: OptionId::Interpreter,
        names: &["--interpreter"],
        arity: Arity::Value,
        scope: Scope::EMIT_EXE,
        usage: "--interpreter <path>",
        about: "use this interpreter for a dynamic executable",
    },
    OptionSpec {
        id: OptionId::LibraryPath,
        names: &["-L", "--library-path"],
        arity: Arity::Value,
        scope: Scope::EMIT_EXE,
        usage: "-L, --library-path <dir>",
        about: "add a native library search path; may be repeated",
    },
    OptionSpec {
        id: OptionId::Library,
        names: &["-l", "--library"],
        arity: Arity::Value,
        scope: Scope::EMIT_EXE,
        usage: "-l, --library <name>",
        about: "link a native library; may be repeated",
    },
    OptionSpec {
        id: OptionId::Rpath,
        names: &["--rpath"],
        arity: Arity::Value,
        scope: Scope::EMIT_EXE,
        usage: "--rpath <path>",
        about: "add a runtime library search path (dynamic only); may be repeated",
    },
    OptionSpec {
        id: OptionId::Optimization,
        names: &["-O"],
        arity: Arity::OptionalValue,
        scope: Scope::GLOBAL,
        usage: "-O, -O0, -O1, -O2, -O3, -Os, -Oz",
        about: "set the optimization level (-O means -O2)",
    },
    OptionSpec {
        id: OptionId::Debug,
        names: &["--debug"],
        arity: Arity::Flag,
        scope: Scope::GLOBAL,
        usage: "--debug, --release, --profile <debug|release>",
        about: "select the build profile (default: debug)",
    },
    OptionSpec {
        id: OptionId::Release,
        names: &["--release"],
        arity: Arity::Flag,
        scope: Scope::GLOBAL,
        usage: "",
        about: "",
    },
    OptionSpec {
        id: OptionId::Profile,
        names: &["--profile"],
        arity: Arity::Value,
        scope: Scope::GLOBAL,
        usage: "",
        about: "",
    },
    OptionSpec {
        id: OptionId::Module,
        names: &["-M", "--module"],
        arity: Arity::Value,
        scope: Scope::CHECK.or(Scope::EMIT_ANY),
        usage: "-M, --module <name=path>",
        about: "map a package name to a source root; may be repeated",
    },
    OptionSpec {
        id: OptionId::Timings,
        names: &["--timings"],
        arity: Arity::OptionalValue,
        scope: Scope::GLOBAL,
        usage: "--timings[=summary|detail]",
        about: "print compiler timings",
    },
    OptionSpec {
        id: OptionId::TimingsFormat,
        names: &["--timings-format"],
        arity: Arity::Value,
        scope: Scope::GLOBAL,
        usage: "--timings-format <text|json>",
        about: "select the timing output format",
    },
    OptionSpec {
        id: OptionId::TimingTrace,
        names: &["--timing-trace"],
        arity: Arity::Value,
        scope: Scope::GLOBAL,
        usage: "--timing-trace <off|events>",
        about: "print individual timing events",
    },
    OptionSpec {
        id: OptionId::DiagnosticsFormat,
        names: &["--diagnostics-format"],
        arity: Arity::Value,
        scope: Scope::GLOBAL,
        usage: "--diagnostics-format <text|json>",
        about: "select human or machine-readable diagnostics",
    },
    OptionSpec {
        id: OptionId::ResourceRoot,
        names: &["--resource-root"],
        arity: Arity::Value,
        scope: Scope::GLOBAL,
        usage: "--resource-root <path>",
        about: "use resources from this toolchain directory",
    },
    OptionSpec {
        id: OptionId::Target,
        names: &["--target"],
        arity: Arity::Value,
        scope: Scope::GLOBAL,
        usage: "--target <name>",
        about: "compile artifacts for a maintained target (default: host)",
    },
    OptionSpec {
        id: OptionId::Help,
        names: &["-h", "--help"],
        arity: Arity::Flag,
        scope: Scope::GLOBAL,
        usage: "-h, --help",
        about: "show help",
    },
    OptionSpec {
        id: OptionId::Version,
        names: &["-V", "--version"],
        arity: Arity::Flag,
        scope: Scope::GLOBAL,
        usage: "-V, --version",
        about: "show the compiler version",
    },
];

/// The help rows for `scope`: its own options, then the global options that
/// apply to every command. `-V` is listed only on the main help. For the
/// `nia emit` overview (`EMIT_ANY`), only options every emit kind accepts are
/// listed; each kind's help lists the rest.
pub(crate) fn help_rows(scope: Scope, main: bool) -> impl Iterator<Item = &'static OptionSpec> {
    let own = OPTIONS.iter().filter(move |spec| {
        !spec.scope.contains(Scope::GLOBAL)
            && if scope == Scope::EMIT_ANY {
                spec.scope.0 & Scope::EMIT_ANY.0 == Scope::EMIT_ANY.0
            } else {
                spec.scope.contains(scope)
            }
    });
    let global = OPTIONS.iter().filter(move |spec| {
        spec.scope.contains(Scope::GLOBAL) && (main || spec.id != OptionId::Version)
    });
    own.chain(global).filter(|spec| !spec.usage.is_empty())
}

/// A recognized option occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Occurrence {
    pub(crate) id: OptionId,
    /// The spelling the user wrote, without any `=value`.
    pub(crate) name: String,
    pub(crate) value: Option<String>,
}

/// The result of scanning one command's arguments.
#[derive(Debug, Default)]
pub(crate) struct Scanned {
    pub(crate) options: Vec<Occurrence>,
    pub(crate) positionals: Vec<String>,
    /// Whether `--` ended option scanning.
    pub(crate) ended_options: bool,
}

impl Scanned {
    /// The last value of a single-value option.
    pub(crate) fn last(&self, id: OptionId) -> Option<&Occurrence> {
        self.options
            .iter()
            .rev()
            .find(|occurrence| occurrence.id == id)
    }

    /// Every occurrence of an option, in order.
    pub(crate) fn all(&self, id: OptionId) -> impl Iterator<Item = &Occurrence> {
        self.options
            .iter()
            .filter(move |occurrence| occurrence.id == id)
    }

    pub(crate) fn has(&self, id: OptionId) -> bool {
        self.options.iter().any(|occurrence| occurrence.id == id)
    }

    pub(crate) fn count(&self, id: OptionId) -> usize {
        self.all(id).count()
    }
}

/// A scanning failure, reported against the command's help topic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScanError {
    Unknown { option: String },
    OutOfScope { option: String },
    MissingValue { option: String },
    EmptyValue { option: String },
    UnexpectedValue { option: String },
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { option } => write!(f, "unknown option `{option}`"),
            Self::OutOfScope { option } => write!(f, "option `{option}` is not valid here"),
            Self::MissingValue { option } => write!(f, "missing value after `{option}`"),
            Self::EmptyValue { option } => write!(f, "`{option}` cannot be empty"),
            Self::UnexpectedValue { option } => write!(f, "`{option}` does not take a value"),
        }
    }
}

/// Scans `args` against the options available in `scope`.
///
/// Global options are always available. An option that exists but belongs to
/// a different command is reported as [`ScanError::OutOfScope`], so the
/// caller can say where it does belong.
pub(crate) fn scan(args: &[String], scope: Scope) -> Result<Scanned, ScanError> {
    scan_from(args, scope, Scanned::default())
}

/// Continues a scan whose earlier part produced `scanned`.
pub(crate) fn scan_from(
    args: &[String],
    scope: Scope,
    mut scanned: Scanned,
) -> Result<Scanned, ScanError> {
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        index += 1;
        if arg == "--" {
            scanned.positionals.extend(args[index..].iter().cloned());
            scanned.ended_options = true;
            break;
        }
        if !arg.starts_with('-') || arg == "-" {
            scanned.positionals.push(arg.clone());
            continue;
        }
        let (spec, name, attached) = match lookup(arg) {
            Some(found) => found,
            None => {
                return Err(ScanError::Unknown {
                    option: display_name(arg),
                });
            }
        };
        if !spec.scope.contains(Scope::GLOBAL) && !spec.scope.contains(scope) {
            return Err(ScanError::OutOfScope {
                option: name.to_string(),
            });
        }
        let value = match (spec.arity, attached) {
            (Arity::Flag, None) => None,
            (Arity::Flag, Some(_)) => {
                return Err(ScanError::UnexpectedValue {
                    option: name.to_string(),
                });
            }
            (Arity::OptionalValue, attached) => attached,
            (Arity::Value, Some(value)) => Some(value),
            (Arity::Value, None) => {
                let Some(value) = args.get(index) else {
                    return Err(ScanError::MissingValue {
                        option: name.to_string(),
                    });
                };
                index += 1;
                Some(value.clone())
            }
        };
        if value.as_deref() == Some("") {
            return Err(ScanError::EmptyValue {
                option: name.to_string(),
            });
        }
        push_occurrence(&mut scanned, spec.id, name, value, arg);
    }
    Ok(scanned)
}

fn push_occurrence(
    scanned: &mut Scanned,
    id: OptionId,
    name: &str,
    value: Option<String>,
    arg: &str,
) {
    // `-vv`/`-vvv` and `-qq` are repeated switches, not values.
    let repeat = match id {
        OptionId::Verbose | OptionId::Quiet if !arg.starts_with("--") => arg.len() - 1,
        _ => 1,
    };
    for _ in 0..repeat {
        scanned.options.push(Occurrence {
            id,
            name: name.to_string(),
            value: value.clone(),
        });
    }
}

/// Finds the spec for one argument and splits off an attached value.
fn lookup(arg: &str) -> Option<(&'static OptionSpec, &'static str, Option<String>)> {
    if let Some(long) = arg.strip_prefix("--") {
        let (name, value) = match long.split_once('=') {
            Some((name, value)) => (name, Some(value.to_string())),
            None => (long, None),
        };
        return OPTIONS.iter().find_map(|spec| {
            spec.names
                .iter()
                .find(|candidate| candidate.strip_prefix("--") == Some(name))
                .map(|candidate| (spec, *candidate, value.clone()))
        });
    }
    // Short options: `-O<level>` is one switch; `-vvv` and `-qq` repeat;
    // otherwise `-x` takes a value attached as `-xvalue` or `-x=value`.
    let short = &arg[..2.min(arg.len())];
    let rest = &arg[short.len()..];
    let spec = OPTIONS.iter().find(|spec| spec.names.contains(&short))?;
    let name = *spec.names.iter().find(|candidate| **candidate == short)?;
    match spec.id {
        // `-O` alone means `-O2`; `-O3` carries its level in the remainder.
        OptionId::Optimization => Some((spec, name, (!rest.is_empty()).then(|| rest.to_string()))),
        OptionId::Verbose | OptionId::Quiet => {
            let repeated = rest
                .chars()
                .all(|character| Some(character) == short.chars().nth(1));
            repeated.then_some((spec, name, None))
        }
        _ if rest.is_empty() => Some((spec, name, None)),
        _ if spec.arity == Arity::Value => {
            let value = rest.strip_prefix('=').unwrap_or(rest);
            Some((spec, name, Some(value.to_string())))
        }
        _ => None,
    }
}

/// The option spelling to name in an error, without any attached value.
fn display_name(arg: &str) -> String {
    match arg.split_once('=') {
        Some((name, _)) if arg.starts_with("--") => name.to_string(),
        _ => arg.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn values_follow_their_option_even_when_they_start_with_a_dash() {
        let scanned = scan(&args(&["--root", "-O", "-j", "-3"]), Scope::BUILD).unwrap();
        assert_eq!(
            scanned.last(OptionId::Root).unwrap().value.as_deref(),
            Some("-O")
        );
        assert_eq!(
            scanned.last(OptionId::Jobs).unwrap().value.as_deref(),
            Some("-3")
        );
        assert!(!scanned.has(OptionId::Optimization));
    }

    #[test]
    fn attached_and_separate_values_are_equivalent() {
        for form in [
            &["--diagnostics-format", "json"][..],
            &["--diagnostics-format=json"],
        ] {
            let scanned = scan(&args(form), Scope::CHECK).unwrap();
            assert_eq!(
                scanned
                    .last(OptionId::DiagnosticsFormat)
                    .unwrap()
                    .value
                    .as_deref(),
                Some("json")
            );
        }
        for form in [&["-j4"][..], &["-j", "4"], &["-j=4"], &["--jobs=4"]] {
            let scanned = scan(&args(form), Scope::BUILD).unwrap();
            assert_eq!(
                scanned.last(OptionId::Jobs).unwrap().value.as_deref(),
                Some("4")
            );
        }
    }

    #[test]
    fn double_dash_ends_option_scanning() {
        let scanned = scan(&args(&["--", "-h", "--root"]), Scope::CHECK).unwrap();
        assert_eq!(scanned.positionals, args(&["-h", "--root"]));
        assert!(!scanned.has(OptionId::Help));
    }

    #[test]
    fn help_is_an_option_only_where_an_option_may_appear() {
        let scanned = scan(&args(&["--cache-dir", "-h", "main.nia"]), Scope::CHECK).unwrap();
        assert!(!scanned.has(OptionId::Help));
        assert_eq!(scanned.positionals, args(&["main.nia"]));
    }

    #[test]
    fn verbosity_counts_repeated_switches() {
        let scanned = scan(&args(&["-v", "-vv", "--verbose"]), Scope::BUILD).unwrap();
        assert_eq!(scanned.count(OptionId::Verbose), 4);
        assert_eq!(
            scan(&args(&["-vx"]), Scope::BUILD).unwrap_err(),
            ScanError::Unknown {
                option: "-vx".to_string()
            }
        );
    }

    #[test]
    fn empty_missing_and_misplaced_values_are_reported() {
        assert_eq!(
            scan(&args(&["--root="]), Scope::BUILD).unwrap_err(),
            ScanError::EmptyValue {
                option: "--root".to_string()
            }
        );
        assert_eq!(
            scan(&args(&["--root", ""]), Scope::BUILD).unwrap_err(),
            ScanError::EmptyValue {
                option: "--root".to_string()
            }
        );
        assert_eq!(
            scan(&args(&["--root"]), Scope::BUILD).unwrap_err(),
            ScanError::MissingValue {
                option: "--root".to_string()
            }
        );
        assert_eq!(
            scan(&args(&["--dry-run=yes"]), Scope::BUILD).unwrap_err(),
            ScanError::UnexpectedValue {
                option: "--dry-run".to_string()
            }
        );
        assert_eq!(
            scan(&args(&["--list"]), Scope::CHECK).unwrap_err(),
            ScanError::OutOfScope {
                option: "--list".to_string()
            }
        );
    }

    #[test]
    fn every_option_has_a_unique_spelling() {
        let mut seen = std::collections::HashSet::new();
        for spec in OPTIONS {
            for name in spec.names {
                assert!(seen.insert(*name), "`{name}` is listed twice");
            }
        }
    }
}
