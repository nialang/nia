# Nia Language Server

`nia-lsp` is a Rust language server that uses the compiler's loader and
semantic queries. Its current editor features are live diagnostics,
definition navigation, hover at resolved names, references within the current
analysis graph, occurrence highlighting in the current file, completion for
local names, module members, receiver fields and methods, signature help for
source functions and resolved methods,
document outline, folding, and semantic highlighting. Named field accesses
also support definition navigation and references. Receiver completion
checks method names from declarations in the current analysis graph.
Definition and reference lookup cover resolved type names, module and block
`using` selectors for types and values, and module alias bindings.
Outline and folding use the current buffer's syntax tree, including unsaved
changes. Folds include bodies, aggregate members, multiline parameter lists
and constraints, grouped imports, and consecutive line comments.
Highlighting uses both the current syntax tree and compiler symbol resolution.
Clients that support semantic token refresh can show syntax highlights while
the compiler resolves symbols. Colors follow the editor theme.

The server uses the nearest package manifest and its workspace settings to
select library, executable, test, and build-script sources. Each package gets
its own dependency aliases. The editor reads local path dependencies and
installed Git and registry sources recorded in `nia.lock`. Dependency
installation and lockfile updates are handled by the build tools.

Clients with watched-file support trigger reanalysis when Nia sources,
manifests, lockfiles, or the configured build plan change on disk. Open
buffers take precedence over disk contents. The server reads the last
configured `build-plan.bin` to discover additional compiler entries and
imports, including generated modules that the build has materialized.

Rename supports local bindings, top-level items, explicit import and
re-export aliases, named fields, enum variants, inherent methods, and
source trait members. Renaming a trait member updates its implementations.
Associated type and constant renames also cover projections, and associated
type renames update named bindings in trait bounds.
Field rename expands shorthand initializers and patterns
when their local variable keeps its original name. Renaming the Nia name
of an `extern` function or static preserves its native symbol with
`linkName` or `exportName`.

Clients that support LSP resource rename operations can rename declared
modules. The workspace edit updates paths in source and moves the module's
`.nia` file and its child directory. The server checks the workspace's
library, executable, test, and build-script entries, including unopened
files. It rejects edits that would break compilation, change binding, or
require changing a generated file or a source outside the workspace.
Generated files are read-only to the LSP and their producer owns changes
to the generated declarations.

Build from the repository root with `cargo build --locked -p nia-lsp`.
On Windows, use an x64 Developer PowerShell with the configured LLVM
installation and add
`--no-default-features --features llvm-static --target x86_64-pc-windows-msvc`.
Run the resulting server with `--resource-root` pointing at the checkout's
`lib` directory. An executable placed in a toolchain's `bin` can locate
the adjacent resources without an argument.

## VS Code

Run `npm ci` and `npm run copy-icon` in `editors/vscode`, then start VS Code
with `--extensionDevelopmentPath` pointing at that directory. Set
`nia.server.path` to the server binary and `nia.server.resourceRoot` to
the absolute path to `lib` in your user settings. For an installable package,
run `npx @vscode/vsce package --skip-license` in the same directory. The
repository license is in [LICENSE.md](../LICENSE.md).

The extension uses the existing Nia square brand icon. Select **Nia File Icons**
from **Preferences: File Icon Theme** to enable it for `.nia` files.
The extension enables semantic highlighting for Nia by default.

## Neovim

With Neovim 0.11 or later, register the file type and use the built-in client.
Neovim uses its `@lsp.type.*` and `@lsp.mod.*` highlight groups.

```lua
vim.filetype.add({ extension = { nia = "nia" } })
vim.lsp.config("nia", {
  cmd = { "/path/to/nia-lsp", "--resource-root", "/path/to/nia/lib" },
  filetypes = { "nia" },
  root_markers = { "nia.toml", ".git" },
})
vim.lsp.enable("nia")

vim.api.nvim_create_autocmd("LspAttach", {
  callback = function(event)
    local client = vim.lsp.get_client_by_id(event.data.client_id)
    if client and client.name == "nia" and client:supports_method("textDocument/foldingRange") then
      for _, win in ipairs(vim.fn.win_findbuf(event.buf)) do
        vim.wo[win].foldmethod = "expr"
        vim.wo[win].foldexpr = "v:lua.vim.lsp.foldexpr()"
      end
    end
  end,
})
```

## Vim

Use an LSP client such as vim-lsp or coc.nvim. Register `.nia` as file type
`nia` and configure the client to launch
`nia-lsp --resource-root /path/to/nia/lib` over standard input and output.
Semantic highlighting requires a client with LSP semantic token support, such
as coc.nvim.
