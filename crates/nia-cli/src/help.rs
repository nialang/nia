// SPDX-License-Identifier: GPL-3.0-or-later
use std::io::IsTerminal;

use super::HelpTopic;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HelpStyle {
    Plain,
    Color,
}

impl HelpStyle {
    pub(crate) fn for_stdout() -> Self {
        Self::for_stream(std::io::stdout().is_terminal())
    }

    pub(crate) fn for_stderr() -> Self {
        Self::for_stream(std::io::stderr().is_terminal())
    }

    fn for_stream(is_terminal: bool) -> Self {
        if is_terminal && std::env::var_os("NO_COLOR").is_none() {
            Self::Color
        } else {
            Self::Plain
        }
    }
}

pub(crate) fn help_text(topic: HelpTopic, style: HelpStyle) -> String {
    render_help(help_doc(topic), style)
}

pub(crate) fn error_help_text(topic: HelpTopic, style: HelpStyle) -> String {
    let doc = help_doc(topic);
    let mut out = String::new();
    push_lines(&mut out, "Usage", doc.usage, style);
    out.push_str("For more information, run `");
    out.push_str(help_command(topic));
    out.push_str("`.\n");
    out
}

struct HelpDoc {
    title: &'static str,
    about: &'static str,
    usage: &'static [&'static str],
    commands: &'static [HelpRow],
    targets: &'static [HelpRow],
    options: &'static [HelpRow],
    examples: &'static [&'static str],
}

#[derive(Clone, Copy)]
struct HelpRow {
    left: &'static str,
    right: &'static str,
}

fn help_doc(topic: HelpTopic) -> HelpDoc {
    match topic {
        HelpTopic::Main => HelpDoc {
            title: "Nia compiler",
            about: "Build, check, and inspect Nia programs.",
            usage: &["nia <command> [options]", "nia help [command]"],
            commands: &[
                HelpRow {
                    left: "build [step]",
                    right: "build the current package",
                },
                HelpRow {
                    left: "test",
                    right: "run package tests",
                },
                HelpRow {
                    left: "check <path>",
                    right: "check a package or source file",
                },
                HelpRow {
                    left: "emit --<target> <path>",
                    right: "inspect or write compiler output",
                },
                HelpRow {
                    left: "help [command]",
                    right: "show command help",
                },
            ],
            targets: &[],
            options: GLOBAL_OPTIONS,
            examples: &["nia build", "nia check src/main.nia"],
        },
        HelpTopic::Build => HelpDoc {
            title: "nia build",
            about: "Build a package with its build.nia script.",
            usage: &["nia build [step] [--root <dir>] [options]"],
            commands: &[],
            targets: &[],
            options: &[
                HelpRow {
                    left: "--root <dir>",
                    right: "start package discovery here instead of the current directory",
                },
                HelpRow {
                    left: "-j, --jobs <count>",
                    right: "limit concurrent build actions",
                },
                OPTIMIZATION_ROW,
                PROFILE_ROW,
                MODULE_ROW,
                TIMINGS_ROW,
                TIMING_FORMAT_ROW,
                TIMING_TRACE_ROW,
                RESOURCE_ROW,
                HelpRow {
                    left: "-h, --help",
                    right: "show this help",
                },
            ],
            examples: &[
                "nia build",
                "nia build check",
                "nia build install --root tools/example",
            ],
        },
        HelpTopic::Test => HelpDoc {
            title: "nia test",
            about: "Build and run package test suites.",
            usage: &["nia test [--root <dir>] [--filter <text>] [--list] [--fail-fast] [options]"],
            commands: &[],
            targets: &[],
            options: &[
                HelpRow {
                    left: "--root <dir>",
                    right: "start package discovery here instead of the current directory",
                },
                HelpRow {
                    left: "--filter <text>",
                    right: "select suites whose names contain this text",
                },
                HelpRow {
                    left: "--list",
                    right: "list selected suites without running them",
                },
                HelpRow {
                    left: "--fail-fast",
                    right: "stop after the first failed suite",
                },
                HelpRow {
                    left: "-j, --jobs <count>",
                    right: "limit concurrent test and build actions",
                },
                OPTIMIZATION_ROW,
                PROFILE_ROW,
                MODULE_ROW,
                TIMINGS_ROW,
                TIMING_FORMAT_ROW,
                TIMING_TRACE_ROW,
                RESOURCE_ROW,
                HelpRow {
                    left: "-h, --help",
                    right: "show this help",
                },
            ],
            examples: &["nia test", "nia test --filter parser", "nia test --list"],
        },
        HelpTopic::Check => HelpDoc {
            title: "nia check",
            about: "Check a package or source file without producing an artifact.",
            usage: &["nia check <path> [options]"],
            commands: &[],
            targets: &[],
            options: &[
                HelpRow {
                    left: "--runtime <bare|freestanding>",
                    right: "select the program runtime (default: bare)",
                },
                HelpRow {
                    left: "--opt-report",
                    right: "print the optimization report",
                },
                HelpRow {
                    left: "--cache-dir <path>",
                    right: "reuse compiler artifacts from this directory",
                },
                OPTIMIZATION_ROW,
                PROFILE_ROW,
                MODULE_ROW,
                TIMINGS_ROW,
                TIMING_FORMAT_ROW,
                TIMING_TRACE_ROW,
                RESOURCE_ROW,
                HelpRow {
                    left: "-h, --help",
                    right: "show this help",
                },
            ],
            examples: &[
                "nia check .",
                "nia check src/main.nia --runtime freestanding",
            ],
        },
        HelpTopic::Emit => HelpDoc {
            title: "nia emit",
            about: "Inspect or write one compiler output.",
            usage: &["nia emit --<target> <path> [options]"],
            commands: &[],
            targets: EMIT_TARGETS,
            options: &[HELP_ROW],
            examples: &[
                "nia emit --ast src/main.nia",
                "nia emit --exe src/main.nia -o build/main",
            ],
        },
        HelpTopic::EmitTokens => inspection_help(
            "nia emit --tokens",
            "Print source tokens and byte spans.",
            TOKENS_USAGE,
            TOKENS_EXAMPLES,
        ),
        HelpTopic::EmitAst => inspection_help(
            "nia emit --ast",
            "Print the parsed syntax tree.",
            AST_USAGE,
            AST_EXAMPLES,
        ),
        HelpTopic::EmitChecked => HelpDoc {
            title: "nia emit --checked",
            about: "Print the checked program.",
            usage: &["nia emit --checked <path> [options]"],
            commands: &[],
            targets: &[],
            options: &[
                RUNTIME_ROW,
                OPTIMIZATION_ROW,
                PROFILE_ROW,
                MODULE_ROW,
                TIMINGS_ROW,
                TIMING_FORMAT_ROW,
                TIMING_TRACE_ROW,
                RESOURCE_ROW,
                HELP_ROW,
            ],
            examples: &["nia emit --checked src/main.nia"],
        },
        HelpTopic::EmitBackend => backend_help(
            "nia emit --backend",
            "Print optimized Nia backend IR.",
            BACKEND_USAGE,
            BACKEND_EXAMPLES,
        ),
        HelpTopic::EmitLlvm => backend_help(
            "nia emit --llvm",
            "Print LLVM IR.",
            LLVM_USAGE,
            LLVM_EXAMPLES,
        ),
        HelpTopic::EmitObj => HelpDoc {
            title: "nia emit --obj",
            about: "Write native object files.",
            usage: &["nia emit --obj <path> [-o <file> | --out-dir <dir>] [options]"],
            commands: &[],
            targets: &[],
            options: &[
                HelpRow {
                    left: "-o <file>",
                    right: "write a single object file",
                },
                HelpRow {
                    left: "--out-dir <dir>",
                    right: "write one file per codegen unit",
                },
                RUNTIME_ROW,
                HelpRow {
                    left: "--cache-dir <path>",
                    right: "reuse compiler artifacts from this directory",
                },
                OPT_REPORT_ROW,
                OPTIMIZATION_ROW,
                PROFILE_ROW,
                MODULE_ROW,
                TIMINGS_ROW,
                TIMING_FORMAT_ROW,
                TIMING_TRACE_ROW,
                RESOURCE_ROW,
                HELP_ROW,
            ],
            examples: &[
                "nia emit --obj src/main.nia -o build/main.o",
                "nia emit --obj src/main.nia --out-dir build/obj",
            ],
        },
        HelpTopic::EmitExe => HelpDoc {
            title: "nia emit --exe",
            about: "Build a freestanding executable.",
            usage: &["nia emit --exe <path> [-o <file>] [options]"],
            commands: &[],
            targets: &[],
            options: &[
                HelpRow {
                    left: "-o <file>",
                    right: "write the executable to this path",
                },
                HelpRow {
                    left: "--cache-dir <path>",
                    right: "reuse compiler artifacts from this directory",
                },
                HelpRow {
                    left: "--link-arg <arg>",
                    right: "pass an argument to the linker; may be repeated",
                },
                HelpRow {
                    left: "--linker <program>",
                    right: "use this linker program",
                },
                HelpRow {
                    left: "--linker-flavor <gnu|lld|self-hosted-elf>",
                    right: "select the linker command style",
                },
                HelpRow {
                    left: "--dynamic-linker <auto|none|path>",
                    right: "select the ELF interpreter",
                },
                HelpRow {
                    left: "--no-dynamic-linker",
                    right: "omit the ELF interpreter",
                },
                HelpRow {
                    left: "-L, --library-path <dir>",
                    right: "add a native library search path",
                },
                HelpRow {
                    left: "-l, --library <name>",
                    right: "link a native library",
                },
                HelpRow {
                    left: "--rpath <path>",
                    right: "add a runtime library search path",
                },
                OPT_REPORT_ROW,
                RUNTIME_EXE_ROW,
                OPTIMIZATION_ROW,
                PROFILE_ROW,
                MODULE_ROW,
                TIMINGS_ROW,
                TIMING_FORMAT_ROW,
                TIMING_TRACE_ROW,
                RESOURCE_ROW,
                HELP_ROW,
            ],
            examples: &[
                "nia emit --exe src/main.nia -o build/main",
                "nia emit --exe src/main.nia -L vendor/lib -lfoo",
            ],
        },
    }
}

fn inspection_help(
    title: &'static str,
    about: &'static str,
    usage: &'static [&'static str],
    examples: &'static [&'static str],
) -> HelpDoc {
    HelpDoc {
        title,
        about,
        usage,
        commands: &[],
        targets: &[],
        options: &[
            OPTIMIZATION_ROW,
            PROFILE_ROW,
            MODULE_ROW,
            TIMINGS_ROW,
            TIMING_FORMAT_ROW,
            TIMING_TRACE_ROW,
            RESOURCE_ROW,
            HELP_ROW,
        ],
        examples,
    }
}

fn backend_help(
    title: &'static str,
    about: &'static str,
    usage: &'static [&'static str],
    examples: &'static [&'static str],
) -> HelpDoc {
    HelpDoc {
        title,
        about,
        usage,
        commands: &[],
        targets: &[],
        options: &[
            RUNTIME_ROW,
            OPT_REPORT_ROW,
            OPTIMIZATION_ROW,
            PROFILE_ROW,
            MODULE_ROW,
            TIMINGS_ROW,
            TIMING_FORMAT_ROW,
            TIMING_TRACE_ROW,
            RESOURCE_ROW,
            HELP_ROW,
        ],
        examples,
    }
}

fn help_command(topic: HelpTopic) -> &'static str {
    match topic {
        HelpTopic::Main => "nia help",
        HelpTopic::Build => "nia help build",
        HelpTopic::Test => "nia help test",
        HelpTopic::Check => "nia help check",
        HelpTopic::Emit => "nia help emit",
        HelpTopic::EmitTokens => "nia help emit --tokens",
        HelpTopic::EmitAst => "nia help emit --ast",
        HelpTopic::EmitChecked => "nia help emit --checked",
        HelpTopic::EmitBackend => "nia help emit --backend",
        HelpTopic::EmitLlvm => "nia help emit --llvm",
        HelpTopic::EmitObj => "nia help emit --obj",
        HelpTopic::EmitExe => "nia help emit --exe",
    }
}

const EMIT_TARGETS: &[HelpRow] = &[
    HelpRow {
        left: "--tokens",
        right: "print source tokens",
    },
    HelpRow {
        left: "--ast",
        right: "print the parsed syntax tree",
    },
    HelpRow {
        left: "--checked",
        right: "print the checked program",
    },
    HelpRow {
        left: "--backend",
        right: "print optimized Nia backend IR",
    },
    HelpRow {
        left: "--llvm",
        right: "print LLVM IR",
    },
    HelpRow {
        left: "--obj",
        right: "write native object files",
    },
    HelpRow {
        left: "--exe",
        right: "build a freestanding executable",
    },
];

const GLOBAL_OPTIONS: &[HelpRow] = &[
    OPTIMIZATION_ROW,
    PROFILE_ROW,
    MODULE_ROW,
    TIMINGS_ROW,
    TIMING_FORMAT_ROW,
    TIMING_TRACE_ROW,
    RESOURCE_ROW,
    HelpRow {
        left: "-h, --help",
        right: "show help",
    },
    HelpRow {
        left: "-V, --version",
        right: "show version",
    },
];

const OPTIMIZATION_ROW: HelpRow = HelpRow {
    left: "-O, -O0, -O1, -O2, -O3, -Os, -Oz",
    right: "set the optimization level (-O means -O2)",
};
const PROFILE_ROW: HelpRow = HelpRow {
    left: "--profile <debug|release|test>",
    right: "select the build profile",
};
const MODULE_ROW: HelpRow = HelpRow {
    left: "-M, --module <name=path>",
    right: "map a package name to a source root",
};
const TIMINGS_ROW: HelpRow = HelpRow {
    left: "--timings[=summary|detail]",
    right: "print compiler timings",
};
const TIMING_FORMAT_ROW: HelpRow = HelpRow {
    left: "--timings-format=<text|json>",
    right: "select the timing output format",
};
const TIMING_TRACE_ROW: HelpRow = HelpRow {
    left: "--timing-trace <off|events>",
    right: "print individual timing events",
};
const RESOURCE_ROW: HelpRow = HelpRow {
    left: "--resource-root <path>",
    right: "use resources from this toolchain directory",
};
const RUNTIME_ROW: HelpRow = HelpRow {
    left: "--runtime <bare|freestanding>",
    right: "select the program runtime (default: bare)",
};
const RUNTIME_EXE_ROW: HelpRow = HelpRow {
    left: "--runtime freestanding",
    right: "select the executable runtime",
};
const OPT_REPORT_ROW: HelpRow = HelpRow {
    left: "--opt-report",
    right: "print the optimization report",
};
const HELP_ROW: HelpRow = HelpRow {
    left: "-h, --help",
    right: "show this help",
};

const TOKENS_USAGE: &[&str] = &["nia emit --tokens <path> [options]"];
const TOKENS_EXAMPLES: &[&str] = &["nia emit --tokens src/main.nia"];
const AST_USAGE: &[&str] = &["nia emit --ast <path> [options]"];
const AST_EXAMPLES: &[&str] = &["nia emit --ast src/main.nia"];
const BACKEND_USAGE: &[&str] = &["nia emit --backend <path> [options]"];
const BACKEND_EXAMPLES: &[&str] = &["nia emit --backend src/main.nia"];
const LLVM_USAGE: &[&str] = &["nia emit --llvm <path> [options]"];
const LLVM_EXAMPLES: &[&str] = &["nia emit --llvm src/main.nia"];

fn render_help(doc: HelpDoc, style: HelpStyle) -> String {
    let mut out = String::new();
    push_title(&mut out, doc.title, style);
    out.push_str(doc.about);
    out.push_str("\n\n");
    push_lines(&mut out, "Usage", doc.usage, style);
    push_rows(&mut out, "Commands", doc.commands, style);
    push_rows(&mut out, "Targets", doc.targets, style);
    push_rows(&mut out, "Options", doc.options, style);
    push_lines(&mut out, "Examples", doc.examples, style);
    out
}

fn push_title(out: &mut String, title: &str, style: HelpStyle) {
    out.push_str(&paint(title, StylePart::Title, style));
    out.push('\n');
}

fn push_lines(out: &mut String, heading: &str, lines: &[&str], style: HelpStyle) {
    if lines.is_empty() {
        return;
    }
    push_heading(out, heading, style);
    for line in lines {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
}

fn push_rows(out: &mut String, heading: &str, rows: &[HelpRow], style: HelpStyle) {
    if rows.is_empty() {
        return;
    }
    push_heading(out, heading, style);
    let width = rows.iter().map(|row| row.left.len()).max().unwrap_or(0);
    for row in rows {
        out.push_str("  ");
        out.push_str(&paint(row.left, StylePart::Usage, style));
        out.push_str(&" ".repeat(width.saturating_sub(row.left.len()) + 2));
        out.push_str(row.right);
        out.push('\n');
    }
    out.push('\n');
}

fn push_heading(out: &mut String, heading: &str, style: HelpStyle) {
    out.push_str(&paint(heading, StylePart::Heading, style));
    out.push_str(":\n");
}

enum StylePart {
    Title,
    Heading,
    Usage,
}

fn paint(text: &str, part: StylePart, style: HelpStyle) -> String {
    if style == HelpStyle::Plain {
        return text.to_string();
    }
    let code = match part {
        StylePart::Title => "1;36",
        StylePart::Heading => "1",
        StylePart::Usage => "32",
    };
    format!("\x1b[{code}m{text}\x1b[0m")
}
