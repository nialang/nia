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
                    left: "lex <file.nia>",
                    right: "tokenize a source file",
                },
                HelpRow {
                    left: "parse <file.nia>",
                    right: "parse and print the AST",
                },
                HelpRow {
                    left: "check <file.nia>",
                    right: "run semantic checks",
                },
                HelpRow {
                    left: "emit <target> <file.nia>",
                    right: "write compiler output",
                },
            ],
            options: GLOBAL_OPTIONS,
            examples: &[
                "nia check src/main.nia",
                "nia -O2 emit obj src/main.nia --out-dir build/obj",
                "nia emit obj src/main.nia --out-dir build/obj -M std=/usr/share/nia/std.nia",
                "nia emit exe src/main.nia -o build/main -M share=share/share.nia",
            ],
            notes: &["Use `nia help <command>` for command-specific details."],
        },
        HelpTopic::Lex => HelpDoc {
            title: "nia lex",
            about: "Tokenize a source file and print token kinds with byte spans.",
            usage: &["nia lex <file.nia> [options]"],
            commands: &[],
            options: GLOBAL_OPTIONS,
            examples: &["nia lex src/main.nia"],
            notes: &[],
        },
        HelpTopic::Parse => HelpDoc {
            title: "nia parse",
            about: "Parse a source file and print the AST.",
            usage: &["nia parse <file.nia> [options]"],
            commands: &[],
            options: GLOBAL_OPTIONS,
            examples: &["nia parse src/main.nia"],
            notes: &["Parse diagnostics are rendered after the AST."],
        },
        HelpTopic::Check => HelpDoc {
            title: "nia check",
            about: "Run the frontend and semantic checking pipeline.",
            usage: &["nia check <file.nia> [--opt-report] [options]"],
            commands: &[],
            options: &[
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
                    right: "map an import root; may appear anywhere",
                },
                HelpRow {
                    left: "-h, --help",
                    right: "show this help text",
                },
            ],
            examples: &[
                "nia check src/main.nia",
                "nia -O1 check src/main.nia --opt-report",
                "nia check src/main.nia -M std=/usr/share/nia/std.nia",
            ],
            notes: &[],
        },
        HelpTopic::Emit => HelpDoc {
            title: "nia emit",
            about: "Run checking and write compiler output.",
            usage: &["nia emit <target> <file.nia> [options]"],
            commands: &[
                HelpRow {
                    left: "backend <file.nia>",
                    right: "write optimized backend IR to stdout",
                },
                HelpRow {
                    left: "llvm <file.nia>",
                    right: "write LLVM IR to stdout",
                },
                HelpRow {
                    left: "obj <file.nia>",
                    right: "write native object file(s)",
                },
                HelpRow {
                    left: "exe <file.nia>",
                    right: "link a hosted executable",
                },
            ],
            options: GLOBAL_OPTIONS,
            examples: &[
                "nia emit backend src/main.nia",
                "nia emit llvm src/main.nia",
                "nia emit obj src/main.nia --out-dir build/obj",
                "nia emit exe src/main.nia -o build/main",
            ],
            notes: &["Use `nia help emit <target>` for target-specific options."],
        },
        HelpTopic::EmitBackend => HelpDoc {
            title: "nia emit backend",
            about: "Run checking and write optimized backend IR to stdout.",
            usage: &["nia emit backend <file.nia> [--opt-report] [options]"],
            commands: &[],
            options: &[
                HelpRow {
                    left: "--opt-report",
                    right: "print backend optimization policy, enabled passes, change count, and changes to stderr",
                },
                HelpRow {
                    left: OPTIMIZATION_OPTION_HELP,
                    right: "set optimization level; -O means -O2",
                },
                HelpRow {
                    left: "-M, --module <name=path>",
                    right: "map an import root; may appear anywhere",
                },
                HelpRow {
                    left: "-h, --help",
                    right: "show this help text",
                },
            ],
            examples: &[
                "nia emit backend src/main.nia",
                "nia -O2 emit backend src/main.nia --opt-report",
            ],
            notes: &["The optimization report is written to stderr so stdout remains backend IR."],
        },
        HelpTopic::EmitLlvm => HelpDoc {
            title: "nia emit llvm",
            about: "Run checking and write LLVM IR for all codegen units to stdout.",
            usage: &["nia emit llvm <file.nia> [--opt-report] [options]"],
            commands: &[],
            options: &[
                HelpRow {
                    left: "--opt-report",
                    right: "print backend optimization policy, enabled passes, change count, and changes to stderr",
                },
                HelpRow {
                    left: OPTIMIZATION_OPTION_HELP,
                    right: "set optimization level; -O means -O2",
                },
                HelpRow {
                    left: "-M, --module <name=path>",
                    right: "map an import root; may appear anywhere",
                },
                HelpRow {
                    left: "-h, --help",
                    right: "show this help text",
                },
            ],
            examples: &[
                "nia emit llvm src/main.nia",
                "nia -O2 emit llvm src/main.nia --opt-report",
            ],
            notes: &["The optimization report is written to stderr so stdout remains LLVM IR."],
        },
        HelpTopic::EmitObj => HelpDoc {
            title: "nia emit obj",
            about: "Run checking and write native object output.",
            usage: &[
                "nia emit obj <file.nia> [-o <file.o> | --out-dir <dir>] [--opt-report] [options]",
            ],
            commands: &[],
            options: &[
                HelpRow {
                    left: "--opt-report",
                    right: "print backend optimization policy, enabled passes, change count, and changes to stderr",
                },
                HelpRow {
                    left: "-o <file.o>",
                    right: "write a single object file",
                },
                HelpRow {
                    left: "--out-dir <dir>",
                    right: "write one object per codegen unit",
                },
                HelpRow {
                    left: OPTIMIZATION_OPTION_HELP,
                    right: "set optimization level; -O means -O2",
                },
                HelpRow {
                    left: "-M, --module <name=path>",
                    right: "map an import root; may appear anywhere",
                },
                HelpRow {
                    left: "-h, --help",
                    right: "show this help text",
                },
            ],
            examples: &[
                "nia emit obj src/main.nia -o build/main.o",
                "nia -Os emit obj src/main.nia -o build/main.o --opt-report",
                "nia emit obj src/main.nia --out-dir build/obj -M std=/usr/share/nia/std.nia",
            ],
            notes: &[
                "The optimization report is written to stderr so object output remains file-only.",
                "Use --out-dir when a program emits multiple codegen units.",
                "-o is accepted only when one object file is produced.",
                "Missing parent directories for -o and --out-dir are created automatically.",
            ],
        },
        HelpTopic::EmitExe => HelpDoc {
            title: "nia emit exe",
            about: "Write native objects to a temporary directory, then invoke the host C linker.",
            usage: &["nia emit exe <file.nia> [-o <executable>] [--opt-report] [options]"],
            commands: &[],
            options: &[
                HelpRow {
                    left: "--opt-report",
                    right: "print backend optimization policy, enabled passes, change count, and changes to stderr",
                },
                HelpRow {
                    left: "-o <executable>",
                    right: "write executable to this path",
                },
                HelpRow {
                    left: OPTIMIZATION_OPTION_HELP,
                    right: "set optimization level; -O means -O2",
                },
                HelpRow {
                    left: "-M, --module <name=path>",
                    right: "map an import root; may appear anywhere",
                },
                HelpRow {
                    left: "-h, --help",
                    right: "show this help text",
                },
            ],
            examples: &[
                "nia emit exe src/main.nia -o build/main",
                "nia -Oz emit exe src/main.nia -o build/main --opt-report",
                "nia emit exe src/main.nia -M share=share/share.nia",
            ],
            notes: &[
                "The optimization report is written to stderr so the executable path remains the only file output.",
                "The linker is selected with CC, or `cc` when CC is not set.",
                "Missing parent directories for -o are created automatically.",
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
        right: "map an import root; may appear anywhere",
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
