# Nia Language Server

`nia-lsp` is a Rust language server that uses the compiler's loader and
semantic queries. Its current editor features are live diagnostics,
definition navigation, hover at resolved names, references within the current
analysis graph, completion for local names and module members, and signature
help for source functions and resolved methods.

The server reads the nearest `nia.toml` for a library root and declared test
entry points. Workspace dependency resolution, generated modules, receiver
member completion, rename, semantic highlighting, and folding are future work.

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

## Neovim

With Neovim 0.11 or later, register the file type and use the built-in client.

```lua
vim.filetype.add({ extension = { nia = "nia" } })
vim.lsp.config("nia", {
  cmd = { "/path/to/nia-lsp", "--resource-root", "/path/to/nia/lib" },
  filetypes = { "nia" },
  root_markers = { "nia.toml", ".git" },
})
vim.lsp.enable("nia")
```

## Vim

Use an LSP client such as vim-lsp or coc.nvim. Register `.nia` as file type
`nia` and configure the client to launch
`nia-lsp --resource-root /path/to/nia/lib` over standard input and output.
