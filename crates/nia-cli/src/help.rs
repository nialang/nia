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

struct HelpDoc {
    title: &'static str,
    about: &'static str,
    usage: &'static [&'static str],
    commands: &'static [HelpRow],
    options: &'static [HelpRow],
    examples: &'static [&'static str],
    notes: &'static [&'static str],
}

#[derive(Clone, Copy)]
struct HelpRow {
    left: &'static str,
    right: &'static str,
}

fn help_doc(topic: HelpTopic) -> HelpDoc {
    match topic {
        HelpTopic::Main => HelpDoc {
            title: "Nia compiler driver",
            about: "Compile, inspect, and check Nia source files.",
            usage: &["nia [options] <command> [args]", "nia help [command]"],
            commands: &[
                HelpRow {
                    left: "build [step]",
                    right: "run a package build graph from build.nia",
                },
                HelpRow {
                    left: "check <file.nia>",
                    right: "run semantic checks",
                },
                HelpRow {
                    left: "emit --<target> <file.nia>",
                    right: "write compiler output or inspection data",
                },
            ],
            options: GLOBAL_OPTIONS,
            examples: &[
                "nia build",
                "nia build install --root .",
                "nia check src/main.nia",
                "nia emit --ast src/main.nia",
                "nia -O2 emit --obj src/main.nia --out-dir build/obj",
                "nia emit --exe src/main.nia -o build/main -M share=share/share.nia",
            ],
            notes: &["Use `nia help <command>` for command-specific details."],
        },
        HelpTopic::Build => HelpDoc {
            title: "nia build",
            about: "Run a package build script from the Nia toolchain.",
            usage: &["nia build [step] [--root <dir>] [options]"],
            commands: &[],
            options: &[
                HelpRow {
                    left: "--root <dir>",
                    right: "select the package root search start; defaults to the current directory",
                },
                HelpRow {
                    left: TIMINGS_OPTION_HELP,
                    right: "reserved for build stage timings",
                },
                HelpRow {
                    left: "-h, --help",
                    right: "show this help text",
                },
            ],
            examples: &[
                "nia build",
                "nia build check",
                "nia build install --root tools/example",
            ],
            notes: &[
                "`nia build` searches for build.nia in the selected directory and then each parent directory.",
                "build.nia is compiled and run as ordinary Nia code through a toolchain-owned runner.",
                "std::build::Build exposes package_root(), build_dir(), cache_dir(), and toolchain_executable() so scripts do not guess toolchain paths.",
                "std::build::Build::check_exe(path) runs the current toolchain as `nia check --exe <path>` from the package root.",
                "Build outputs belong under .nia-build/ and reusable package or compiler cache entries belong under .nia-cache/.",
                "The build runner lives in the Nia toolchain instead of a separate paw binary or C API bridge.",
            ],
        },
        HelpTopic::Check => HelpDoc {
            title: "nia check",
            about: "Run the frontend and semantic checking pipeline.",
            usage: &["nia check <file.nia> [--exe | --runtime <runtime>] [--opt-report] [options]"],
            commands: &[],
            options: &[
                HelpRow {
                    left: "--exe",
                    right: "alias for --runtime freestanding",
                },
                HelpRow {
                    left: "--runtime <bare|freestanding>",
                    right: "select checking runtime; bare is the default, freestanding injects the executable startup runtime",
                },
                HelpRow {
                    left: "--opt-report",
                    right: "print backend optimization policy, enabled passes, change count, and changes",
                },
                HelpRow {
                    left: OPTIMIZATION_OPTION_HELP,
                    right: "set optimization level; -O means -O2",
                },
                HelpRow {
                    left: "-M, --module <name=path>",
                    right: "map a module root; may appear anywhere; `entry`, `pkg`, and `builtin` are reserved",
                },
                HelpRow {
                    left: TIMINGS_OPTION_HELP,
                    right: "print compiler stage timings to stderr; detail also includes query timings",
                },
                HelpRow {
                    left: "-h, --help",
                    right: "show this help text",
                },
            ],
            examples: &[
                "nia check src/main.nia",
                "nia check --exe src/main.nia",
                "nia check src/main.nia --runtime freestanding",
                "nia -O1 check src/main.nia --opt-report",
                "nia check src/main.nia -M std=/usr/share/nia/std.nia",
            ],
            notes: &["Timing reports are written to stderr."],
        },
        HelpTopic::Emit => HelpDoc {
            title: "nia emit",
            about: "Run a selected compiler output stage.",
            usage: &[
                "nia emit --tokens <file.nia> [options]",
                "nia emit --ast <file.nia> [options]",
                "nia emit --checked <file.nia> [--opt-report] [options]",
                "nia emit --backend <file.nia> [--opt-report] [options]",
                "nia emit --llvm <file.nia> [--opt-report] [options]",
                "nia emit --obj <file.nia> [-o <file.o> | --out-dir <dir>] [--runtime <runtime>] [--opt-report] [options]",
                "nia emit --exe <file.nia> [-o <executable>] [--runtime freestanding] [link options] [--opt-report] [options]",
            ],
            commands: &[],
            options: &[
                HelpRow {
                    left: "--tokens",
                    right: "tokenize and print token kinds with byte spans",
                },
                HelpRow {
                    left: "--ast",
                    right: "parse and print the AST",
                },
                HelpRow {
                    left: "--checked",
                    right: "run checking and print the checked program",
                },
                HelpRow {
                    left: "--backend",
                    right: "write optimized backend IR to stdout",
                },
                HelpRow {
                    left: "--llvm",
                    right: "write LLVM IR to stdout",
                },
                HelpRow {
                    left: "--obj",
                    right: "write native object file(s)",
                },
                HelpRow {
                    left: "--exe",
                    right: "link a freestanding executable",
                },
                HelpRow {
                    left: "--runtime <bare|freestanding>",
                    right: "select runtime for --obj; bare is the default, --exe supports freestanding",
                },
                HelpRow {
                    left: "--link-arg <arg>",
                    right: "pass an extra argument to the executable linker; may appear multiple times",
                },
                HelpRow {
                    left: "--linker <program>",
                    right: "select the executable linker program for --exe",
                },
                HelpRow {
                    left: "--linker-flavor <gnu|lld|self-hosted-elf>",
                    right: "select how Nia translates structured link options",
                },
                HelpRow {
                    left: "--dynamic-linker <auto|none|path>",
                    right: "link a dynamic executable with an ELF interpreter policy",
                },
                HelpRow {
                    left: "--no-dynamic-linker",
                    right: "link a dynamic executable without an ELF interpreter",
                },
                HelpRow {
                    left: "-L, --library-path <dir>",
                    right: "add a native library search path for --exe",
                },
                HelpRow {
                    left: "-l, --library <name>",
                    right: "link a native library by name for --exe",
                },
                HelpRow {
                    left: "--rpath <path>",
                    right: "add a runtime library search path for --exe",
                },
                HelpRow {
                    left: "-o <file.o>",
                    right: "write a single object file for --obj, or executable for --exe",
                },
                HelpRow {
                    left: "--out-dir <dir>",
                    right: "write one object per codegen unit for --obj",
                },
                HelpRow {
                    left: "--opt-report",
                    right: "print backend optimization policy, enabled passes, change count, and changes to stderr for checked/backend/llvm/obj/exe",
                },
                HelpRow {
                    left: OPTIMIZATION_OPTION_HELP,
                    right: "set optimization level; -O means -O2",
                },
                HelpRow {
                    left: "-M, --module <name=path>",
                    right: "map a module root; may appear anywhere; `entry`, `pkg`, and `builtin` are reserved",
                },
                HelpRow {
                    left: TIMINGS_OPTION_HELP,
                    right: "print compiler stage timings to stderr; detail also includes query timings",
                },
                HelpRow {
                    left: "-h, --help",
                    right: "show this help text",
                },
            ],
            examples: &[
                "nia emit --tokens src/main.nia",
                "nia emit --ast src/main.nia",
                "nia -O2 emit --backend src/main.nia --opt-report",
                "nia emit --llvm src/main.nia",
                "nia emit --obj src/main.nia --out-dir build/obj",
                "nia emit --obj src/main.nia --runtime freestanding -o build/startup.o",
                "nia emit --exe src/main.nia -o build/main",
            ],
            notes: &[
                "Use exactly one emit target flag.",
                "The optimization report is written to stderr so stdout remains inspection output and native targets remain file-only.",
                "Timing reports are written to stderr so stdout remains inspection output and native targets remain file-only.",
                "`emit --obj` defaults to the bare runtime and does not inject startup code.",
                "Use --out-dir when --obj emits multiple codegen units.",
                "-o for --obj is accepted only when one object file is produced.",
                "The linker is selected with NIA_LINKER, or the target default linker when NIA_LINKER is not set.",
                "For --linker-flavor lld, an explicit --linker wins; otherwise NIA_LLD or PATH is used to find ld.lld.",
                "The default executable runtime is freestanding and enters through the injected standard-library startup facade; the current implementation is Linux x86_64.",
                "Missing parent directories for -o and --out-dir are created automatically.",
            ],
        },
    }
}

const GLOBAL_OPTIONS: &[HelpRow] = &[
    HelpRow {
        left: OPTIMIZATION_OPTION_HELP,
        right: "set optimization level; -O means -O2",
    },
    HelpRow {
        left: "-M, --module <name=path>",
        right: "map a module root; may appear anywhere; `entry`, `pkg`, and `builtin` are reserved",
    },
    HelpRow {
        left: TIMINGS_OPTION_HELP,
        right: "print compiler stage timings to stderr; use detail for query timings",
    },
    HelpRow {
        left: "-h, --help",
        right: "show help",
    },
    HelpRow {
        left: "-V, --version",
        right: "show version",
    },
];

const OPTIMIZATION_OPTION_HELP: &str = "-O, -O0, -O1, -O2, -O3, -Os, -Oz";
const TIMINGS_OPTION_HELP: &str = "--timings[=summary|detail]";

fn render_help(doc: HelpDoc, style: HelpStyle) -> String {
    let mut out = String::new();
    push_title(&mut out, doc.title, style);
    out.push_str(doc.about);
    out.push_str("\n\n");
    push_lines(&mut out, "Usage", doc.usage, style);
    push_rows(&mut out, "Commands", doc.commands, style);
    push_rows(&mut out, "Options", doc.options, style);
    push_lines(&mut out, "Examples", doc.examples, style);
    push_lines(&mut out, "Notes", doc.notes, style);
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
