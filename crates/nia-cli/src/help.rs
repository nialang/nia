// SPDX-License-Identifier: GPL-3.0-or-later
use super::HelpTopic;

pub(crate) fn help_text(topic: HelpTopic) -> &'static str {
    match topic {
        HelpTopic::Main => {
            "Nia compiler driver

Usage:
  niac [options] <command> [args]
  niac help [command]

Commands:
  lex <file.nia>                 tokenize a source file
  parse <file.nia>               parse and print the AST
  check <file.nia>               run semantic checks
  emit llvm <file.nia>           write LLVM IR to stdout
  emit obj <file.nia> [options]  write native object file(s)
  emit exe <file.nia> [options]  link a hosted executable

Global options:
  -M, --module <name=path>       map an import root
  -h, --help                     show help
  -V, --version                  show version

Examples:
  niac check src/main.nia
  niac -M std=/usr/share/nia/std.nia emit obj src/main.nia --out-dir build/obj
  niac emit exe src/main.nia -o build/main
"
        }
        HelpTopic::Lex => {
            "Usage:
  niac [options] lex <file.nia>

Tokenize a source file and print token kinds with byte spans.
"
        }
        HelpTopic::Parse => {
            "Usage:
  niac [options] parse <file.nia>

Parse a source file and print the AST. Parse diagnostics are rendered after the AST.
"
        }
        HelpTopic::Check => {
            "Usage:
  niac [options] check <file.nia>

Run the full frontend and semantic checking pipeline.
"
        }
        HelpTopic::Emit => {
            "Usage:
  niac [options] emit <target> <file.nia> [target-options]

Targets:
  llvm                          write LLVM IR to stdout
  obj                           write native object file(s)
  exe                           link a hosted executable

Examples:
  niac emit llvm src/main.nia
  niac emit obj src/main.nia --out-dir build/obj
  niac emit exe src/main.nia -o build/main
"
        }
        HelpTopic::EmitLlvm => {
            "Usage:
  niac [options] emit llvm <file.nia>

Run checking and write LLVM IR for all codegen units to stdout.
"
        }
        HelpTopic::EmitObj => {
            "Usage:
  niac [options] emit obj <file.nia> [-o <file.o> | --out-dir <dir>]

Write native object output.

Options:
  -o <file.o>                   write a single object file
  --out-dir <dir>               write one object per codegen unit

Notes:
  A program may lower to multiple codegen units. Use --out-dir when multiple
  object files are expected. -o is accepted only for single-unit output.
"
        }
        HelpTopic::EmitExe => {
            "Usage:
  niac [options] emit exe <file.nia> [-o <executable>]

Write native objects to a temporary directory, then invoke the host C linker.

Options:
  -o <executable>               output executable path
"
        }
    }
}
