# Nia Language Server

`nia-lsp` is the Nia language server. It analyzes code with the same loader
and semantic queries as the compiler, so its diagnostics and name resolution
match `nia check`. This page describes what the server does, how it finds
your project, and how to set it up in VS Code, Neovim, and Vim.

## Features

- **Diagnostics.** Compiler diagnostics for open files, updated as you type.
- **Navigation.** Go to definition and type definition, find trait
  implementations, find references across the workspace, highlight
  occurrences in the current file, browse the call hierarchy, and search
  workspace symbols by name or qualified path such as `app::geometry::Point`,
  including declarations in unsaved buffers. Definition and references also
  work on named field accesses, resolved type names, module and block `using`
  selectors, and module aliases.
- **Completion.** Completion offers local names, names visible in the current
  module, members of a module path (`module::`), and fields and methods after
  `.` on a receiver. It does not yet complete members of a type path
  (`Type::`) or enum variants written with the type omitted (`.Variant`).
  When an unqualified name has no visible completion, the server searches the
  available modules and declared dependencies, and offers the name together
  with the `using` it needs; each such pair is checked against the compiler
  before it is offered. Callable items show their signature and adjacent
  `///` documentation.
- **Signature help.** The signature, documentation, and active parameter for
  calls of source functions and resolved methods. Calls of associated
  functions written as `Type::f(` are not covered.
- **Hover.** Hover on a resolved name shows its declaration: the signature
  and adjacent `///` documentation for a callable, and the declaration line
  for other items. Hover does not show inferred types; inlay hints do.
- **Inlay hints.** Parameter names at call sites and the inferred types of
  local bindings.
- **Structure.** Document outline, folding, and expand-selection use the
  syntax tree of the current buffer, including unsaved changes. Folds cover
  bodies, aggregate members, multiline parameter lists and constraints,
  grouped imports, and runs of line comments. Expand-selection steps through
  enclosing expressions, statements, and declarations.
- **Semantic highlighting.** Tokens come from the syntax tree and from
  compiler name resolution. Clients that support semantic token refresh show
  syntax highlights first and refine them once names are resolved. Colors
  follow the editor theme.
- **Code actions.** Quick fixes apply compiler suggestions and add missing
  imports. **Organize imports** sorts each run of `using` directives on
  consecutive lines, at module level and inside blocks, and the names inside
  each `using` group. It leaves `pub using` directives, directives with
  attributes, directives that share a line with other code, and directives
  directly below a comment in place. It is offered only when the file parses
  and the reordered file has no new errors and binds every name to the same
  declaration.
- **Rename** and **formatting**, described below.

## Projects

The server uses the nearest package manifest and its workspace settings to
select library, executable, test, and build-script sources. Each package keeps
its own dependency aliases. The server reads local path dependencies and the
installed Git and registry sources recorded in `nia.lock`; installing
dependencies and updating the lockfile are left to `nia build` and
`nia update`.

The server also reads the last configured build plan, `build-plan.bin`, to
discover additional compiler entries and imports, including generated modules
the build has already written. Those entries use the plan's build profile and
CPU defaults together with each compiler action's target, runtime, and test
mode. The server warns when the plan is corrupt or stale, or when a build
script has not published a plan yet.

Open buffers take precedence over files on disk. Clients that support watched
files trigger reanalysis when Nia sources, manifests, lockfiles, or the build
plan change on disk. In clients without watched-file support, the server
rereads manifests and the build plan only when a Nia buffer is saved.

Workspace references and rename share one index per document revision. They
run separately from interactive analysis, and an edit cancels them at their
next cancellation point. References include resolved uses in entries that
have diagnostics.

## Rename

Rename covers local bindings, top-level items, explicit import and re-export
aliases, named fields, enum variants, inherent methods, and source trait
members. Renaming a trait member also renames it in every implementation.
Renaming an associated type or constant updates its projections, and renaming
an associated type updates named bindings in trait bounds. Renaming a field
expands shorthand initializers and patterns whose local variable keeps the
old name. Renaming the Nia name of an `extern` function or static keeps its
native symbol by adding `linkName` or `exportName`.

Before returning a rename, the server applies it to a copy of the workspace
and re-analyzes every library, executable, test, and build-script entry that
can see the edited files, including files that are not open. It rejects the
rename when the edited code reports a new error, when any use would bind to a
different declaration, or when the edit would have to change a generated file
or a source outside the workspace. The check covers only what that analysis
sees:

- a rename in a file that already has a syntax error is not fully verified;
- code in inactive `@[if]` regions, such as release-only code, is not
  checked;
- a package that depends on the renamed one is found only when it is open or
  is a member of the same `[workspace]`.

### Renaming Modules

Clients that support LSP resource operations can rename a declared module:
the edit updates paths in source and moves the module's `.nia` file and its
child directory. In clients that send file rename requests, renaming a
module's file within its directory also updates the module declaration and
the references bound to it. Include the module's child directory in the same
operation when it has one. One operation can rename several independent
modules, each in its own directory.

Moving a module under another declaring module of the same package moves its
declaration and updates its resolved paths. The server checks the moved
source, including any unsaved buffer, in the destination namespace.
Case-only renames go through the same checks.

### Generated Sources

A generator can publish a `.nia.map` file beside a generated `.nia` file in
the package's build output directory. Navigation then leads from the generated
code to the input it came from, and rename can edit that input. The generating
command must declare both files as outputs, and each mapped input must be one
of its declared inputs or lie inside a declared input directory.

The map is version 1 JSON:

```json
{
  "version": 1,
  "generatedDigest": "<digest of generated.nia>",
  "mappings": [
    {
      "generated": [20, 26],
      "sourcePath": "input.nia",
      "sourceDigest": "<digest of input.nia>",
      "source": [8, 14]
    }
  ]
}
```

Digests are lowercase hexadecimal BLAKE3 hashes of the exact UTF-8 file
bytes. Ranges are non-empty, half-open UTF-8 byte offsets that start and end
on character boundaries. `sourcePath` is relative to the package root; set
`"sourceRoot": "build"` on a mapping whose input is itself a file in the build
output directory. Several generated ranges may point to the same input range,
but generated ranges must not overlap.

Definition, type definition, references, and implementation navigation use
the map while both files still match their digests; unsaved changes to either
file are checked against the same digests. A range whose generated text is a
byte-for-byte copy of its input keeps the exact position inside the copy; any
other range leads to the whole input range.

To let rename edit the input, set `"editable": true` on a byte-for-byte
mapping whose generator carries identifier changes in the input through to its
output. Map every copy of the renamed text, including copies in other
generated Nia outputs. The server checks the projected generated sources with
the compiler and sends edits to the input files and their handwritten callers;
the next build regenerates the outputs.

## Formatting

Document and range formatting use the lossless syntax tree. Formatting sets
the indentation of each line from its nesting depth, puts single spaces
around infix operators, and removes trailing whitespace. It keeps comments,
literals, and existing line breaks. Because indentation follows nesting depth
alone, a continuation line of a wrapped expression is moved to its nesting
level; the continuation indentation used in the standard library is not kept.
A file with a syntax error is not formatted.

Range formatting changes only the lines that intersect the selection. The
editor's indentation setting selects spaces or tabs. On the command line,
`nia fmt path/to/file.nia` prints the formatted file to standard output with
four-space indentation.

## Installing

Release archives contain `nia-lsp` beside `nia` in `bin`. A server in a
toolchain's `bin` finds the toolchain's `lib` resources on its own.

To build the server from a checkout, run `cargo build --locked -p nia-lsp`
from the repository root. On Windows, build from an x64 Developer PowerShell
with LLVM configured as described in
[Development Setup](../docs/dev/setup.md#windows-msvc), and add
`--no-default-features --features llvm-static --target x86_64-pc-windows-msvc`.
Start a checkout build with `--resource-root` pointing at the checkout's `lib`
directory.

### VS Code

Download `nia-<version>.vsix` and the matching toolchain archive for your
platform from [Nia releases](https://github.com/nialang/nia/releases).
Install the extension with **Extensions: Install from VSIX…**. Put the
unpacked toolchain's `bin` directory on `PATH`, or set `nia.server.path` to
the server executable.

To build the VSIX from a checkout, run `npm ci` and then
`npm exec --yes --package=@vscode/vsce@4.1.1 -- vsce package` in
`editors/vscode` with Node.js 24. For a checkout build of `nia-lsp`, set
`nia.server.resourceRoot` to the checkout's `lib` directory.

The extension enables semantic highlighting for Nia by default. To show the
Nia icon on `.nia` files, select **Nia File Icons** in
**Preferences: File Icon Theme**.

### Neovim

With Neovim 0.11 or later, add the client package to your runtime path and
enable its LSP configuration. Put the release archive's `bin` on `PATH`. The
file type enables LSP folding and Neovim's semantic token highlighting.

```lua
vim.opt.runtimepath:prepend("/path/to/nia/editors/neovim")
vim.lsp.enable("nia")
```

For a checkout build, override the command before enabling the server. An
icon plugin that uses glyphs can use the Nia brand color.

```lua
vim.lsp.config("nia", {
  cmd = { "/path/to/nia-lsp", "--resource-root", "/path/to/nia/lib" },
})
require("nvim-web-devicons").set_icon({
  nia = { icon = "󰅩", color = "#b8235a", name = "Nia" },
})
```

### Vim

Add `/path/to/nia/editors/vim` to `runtimepath` before `filetype plugin on`,
and put the release toolchain's `bin` on `PATH`. For semantic highlighting
and folding with coc.nvim, add this to your `coc-settings.json`:

```json
{
  "languageserver": {
    "nia": {
      "command": "nia-lsp",
      "filetypes": ["nia"],
      "rootPatterns": ["nia.toml", "build.nia", ".git"]
    }
  },
  "semanticTokens.enable": true
}
```

With coc.nvim, use `:Fold` in a Nia buffer to fold it. With vim-devicons,
set `g:WebDevIconsUnicodeDecorateFileNodesExtensionSymbols['nia']` to
`'󰅩'` after loading the icon plugin.

Server tests, smoke tests for each editor client, and latency measurements are
described in [Testing](../docs/dev/testing.md#language-server).
