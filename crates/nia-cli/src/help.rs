// SPDX-License-Identifier: GPL-3.0-or-later
//! Help text. Option rows come from the option table in [`crate::args`], so
//! every topic lists exactly the options its command accepts.

use std::io::IsTerminal;

use crate::args::{self, EmitKind, OptionSpec};
use crate::cli::HelpTopic;

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
    let doc = help_doc(topic);
    let mut out = String::new();
    push_title(&mut out, doc.title, style);
    out.push_str(doc.about);
    out.push_str("\n\n");
    push_lines(&mut out, "Usage", doc.usage, style);
    push_rows(&mut out, "Commands", doc.commands, style);
    let targets = (topic == HelpTopic::Emit)
        .then(emit_target_rows)
        .unwrap_or_default();
    push_rows(&mut out, "Targets", &targets, style);
    let options = option_rows(topic);
    push_rows(&mut out, "Options", &options, style);
    if topic == HelpTopic::Emit {
        out.push_str(
            "Each target accepts its own options; run `nia help emit --<target>` to list them.\n\n",
        );
    }
    push_lines(&mut out, "Examples", doc.examples, style);
    out
}

pub(crate) fn error_help_text(topic: HelpTopic, style: HelpStyle) -> String {
    let doc = help_doc(topic);
    let mut out = String::new();
    push_lines(&mut out, "Usage", doc.usage, style);
    out.push_str("For more information, run `");
    out.push_str(&help_command(topic));
    out.push_str("`.\n");
    out
}

pub(crate) fn help_command(topic: HelpTopic) -> String {
    match topic {
        HelpTopic::Main => "nia help".to_string(),
        HelpTopic::Build => "nia help build".to_string(),
        HelpTopic::Test => "nia help test".to_string(),
        HelpTopic::Check => "nia help check".to_string(),
        HelpTopic::Emit => "nia help emit".to_string(),
        HelpTopic::EmitKind(kind) => format!("nia help emit {}", kind.flag()),
    }
}

/// One rendered help row.
type Row = (&'static str, &'static str);

struct HelpDoc {
    title: &'static str,
    about: &'static str,
    usage: &'static [&'static str],
    commands: &'static [Row],
    examples: &'static [&'static str],
}

fn option_rows(topic: HelpTopic) -> Vec<Row> {
    args::help_rows(topic.scope(), topic == HelpTopic::Main)
        .map(|spec: &OptionSpec| (spec.usage, spec.about))
        .collect()
}

fn emit_target_rows() -> Vec<Row> {
    EmitKind::ALL
        .into_iter()
        .filter_map(|kind| {
            args::OPTIONS
                .iter()
                .find(|spec| spec.id == args::OptionId::EmitTarget(kind))
                .map(|spec| (kind.flag(), spec.about))
        })
        .collect()
}

fn help_doc(topic: HelpTopic) -> HelpDoc {
    match topic {
        HelpTopic::Main => HelpDoc {
            title: "Nia compiler",
            about: "Build, check, and inspect Nia programs.",
            usage: &["nia <command> [options]", "nia help [command]"],
            commands: &[
                ("build [step]", "build the current package"),
                ("test", "run package tests"),
                ("check <path>", "check a package or source file"),
                ("emit --<target> <path>", "inspect or write compiler output"),
                ("help [command]", "show command help"),
            ],
            examples: &["nia build", "nia check src/main.nia"],
        },
        HelpTopic::Build => HelpDoc {
            title: "nia build",
            about: "Build a package with its build.nia script.",
            usage: &["nia build [step] [--root <dir>] [-p <name> ... | --workspace] [options]"],
            commands: &[],
            examples: &[
                "nia build",
                "nia build check",
                "nia build install --root tools/example",
                "nia build --workspace",
            ],
        },
        HelpTopic::Test => HelpDoc {
            title: "nia test",
            about: "Build and run package test suites.",
            usage: &[
                "nia test [--root <dir>] [-p <name> ... | --workspace] [--filter <text>] [--list] [--fail-fast] [options]",
            ],
            commands: &[],
            examples: &["nia test", "nia test --filter parser", "nia test --list"],
        },
        HelpTopic::Check => HelpDoc {
            title: "nia check",
            about: "Check a package or source file without producing an artifact.",
            usage: &["nia check <path> [options]"],
            commands: &[],
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
            examples: &[
                "nia emit --ast src/main.nia",
                "nia emit --exe src/main.nia -o build/main",
            ],
        },
        HelpTopic::EmitKind(kind) => emit_kind_doc(kind),
    }
}

fn emit_kind_doc(kind: EmitKind) -> HelpDoc {
    match kind {
        EmitKind::Tokens => HelpDoc {
            title: "nia emit --tokens",
            about: "Print source tokens and byte spans.",
            usage: &["nia emit --tokens <path> [options]"],
            commands: &[],
            examples: &["nia emit --tokens src/main.nia"],
        },
        EmitKind::Ast => HelpDoc {
            title: "nia emit --ast",
            about: "Print the parsed syntax tree.",
            usage: &["nia emit --ast <path> [options]"],
            commands: &[],
            examples: &["nia emit --ast src/main.nia"],
        },
        EmitKind::Checked => HelpDoc {
            title: "nia emit --checked",
            about: "Print the checked program.",
            usage: &["nia emit --checked <path> [options]"],
            commands: &[],
            examples: &["nia emit --checked src/main.nia"],
        },
        EmitKind::Backend => HelpDoc {
            title: "nia emit --backend",
            about: "Print optimized Nia backend IR.",
            usage: &["nia emit --backend <path> [options]"],
            commands: &[],
            examples: &["nia emit --backend src/main.nia"],
        },
        EmitKind::Llvm => HelpDoc {
            title: "nia emit --llvm",
            about: "Print LLVM IR.",
            usage: &["nia emit --llvm <path> [options]"],
            commands: &[],
            examples: &["nia emit --llvm src/main.nia"],
        },
        EmitKind::Obj => HelpDoc {
            title: "nia emit --obj",
            about: "Write native object files.",
            usage: &["nia emit --obj <path> [-o <file> | --out-dir <dir>] [options]"],
            commands: &[],
            examples: &[
                "nia emit --obj src/main.nia -o build/main.o",
                "nia emit --obj src/main.nia --out-dir build/obj",
            ],
        },
        EmitKind::Exe => HelpDoc {
            title: "nia emit --exe",
            about: "Build a freestanding executable.",
            usage: &["nia emit --exe <path> [-o <file>] [options]"],
            commands: &[],
            examples: &[
                "nia emit --exe src/main.nia -o build/main",
                "nia emit --exe src/main.nia -L vendor/lib -lfoo",
            ],
        },
    }
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

fn push_rows(out: &mut String, heading: &str, rows: &[Row], style: HelpStyle) {
    if rows.is_empty() {
        return;
    }
    push_heading(out, heading, style);
    let width = rows.iter().map(|(left, _)| left.len()).max().unwrap_or(0);
    for (left, right) in rows {
        out.push_str("  ");
        out.push_str(&paint(left, StylePart::Usage, style));
        out.push_str(&" ".repeat(width.saturating_sub(left.len()) + 2));
        out.push_str(right);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{OPTIONS, Scope};

    fn topics() -> Vec<HelpTopic> {
        let mut topics = vec![
            HelpTopic::Main,
            HelpTopic::Build,
            HelpTopic::Test,
            HelpTopic::Check,
            HelpTopic::Emit,
        ];
        topics.extend(EmitKind::ALL.map(HelpTopic::EmitKind));
        topics
    }

    /// Every option a command accepts is listed in its help, and every listed
    /// option is accepted: both views come from the same table.
    #[test]
    fn help_lists_exactly_the_options_each_command_accepts() {
        for topic in topics() {
            let text = help_text(topic, HelpStyle::Plain);
            for spec in OPTIONS {
                if spec.usage.is_empty() {
                    continue;
                }
                // The emit overview lists only options every emit target accepts.
                let accepted = spec.scope.contains(Scope::GLOBAL)
                    || if topic == HelpTopic::Emit {
                        EmitKind::ALL
                            .into_iter()
                            .all(|kind| spec.scope.contains(kind.scope()))
                    } else {
                        spec.scope.contains(topic.scope())
                    };
                let listed = text.contains(spec.usage);
                let expected = accepted
                    && (topic == HelpTopic::Main || spec.id != args::OptionId::Version)
                    && (topic != HelpTopic::Main || spec.scope.contains(Scope::GLOBAL));
                assert_eq!(listed, expected, "{topic:?}: `{}`", spec.usage);
            }
        }
    }

    #[test]
    fn emit_help_lists_targets_and_points_to_target_help() {
        let text = help_text(HelpTopic::Emit, HelpStyle::Plain);
        for kind in EmitKind::ALL {
            assert!(text.contains(kind.flag()), "{text}");
        }
        assert!(text.contains("nia help emit --<target>"), "{text}");
    }
}
