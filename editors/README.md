# Nia Editor Support

VS Code, Vim, and Neovim use the same Nia parser for syntax highlighting and
folding. Multiline declarations retain their signature and `where` clause
when folded. Nested bodies, aggregates, `match` expressions, and groups of
top-level imports can also be folded.

The release archive includes the editor integrations in `editors/` and
`nia-editor` in `bin/`. Put `bin/` on the editor's `PATH`, or configure the
executable path as shown below. To build the helper from a checkout, run
`cargo build --locked --release -p nia-editor`. On Windows use
`--target x86_64-pc-windows-msvc`. The helper builds without LLVM.

## VS Code

To try the extension from a checkout or a release archive, open a folder
with `code --extensionDevelopmentPath=/path/to/nia/editors/vscode`.
For regular use, install the extension directory under your VS Code
extensions folder as `nialang.nia-0.3.0`, or package it from inside
`editors/vscode` with `npx @vscode/vsce package --skip-license` and install the resulting
`.vsix` with `code --install-extension`.

Set `nia.editorPath` in your user settings if the helper is outside `PATH`.
The language contributes an SVG file icon to themes that use language icons.
Select **Nia File Icons** under **Preferences: File Icon Theme** when your
current theme hides language icons.
The TextMate grammar gives immediate lexical colors while the syntax provider
computes the structural roles. Theme authors can customize the `primitive`,
`propagation`, and `dereference` semantic token types.

## Vim And Neovim

Add `editors/vim` to `runtimepath` in your `.vimrc` or `init.vim` before
startup plugins load:

```vim
set runtimepath+=/path/to/nia/editors/vim
let g:nia_editor_path = '/path/to/nia/bin/nia-editor'
```

Vim needs jobs, channels, timers, and `prop_add_list()`. Neovim uses its job
API and buffer highlights. Change the `Nia_*` highlight groups to customize
colors, and set `g:nia_folding = 0` to keep your own fold method.

For file icons, the plugin registers a configurable `N` glyph with
vim-devicons and nvim-web-devicons when they are loaded at startup. Set
`g:nia_file_icon` to a glyph supported by your font. With a lazy-loaded icon
plugin, call `nia#icons()` after it loads.

## Helper Protocol

The helper runs as a local subprocess over standard input and output. Each
NDJSON request contains an integer `id` and a UTF-8 `text` snapshot. The
response echoes the ID and returns `result` or `error`. The result contains
token tuples `[line, byteColumn, byteLength, utf16Column, utf16Length, kind]`
and fold tuples `[firstLine, lastLine, kind]`. Positions are zero-based and
the last line of a fold is inclusive. The clients and helper are released
together.
