# Nia Language Server

`nia-lsp` is a Rust language server that uses the compiler's loader and
semantic queries. Its current editor features are live diagnostics,
definition and type-definition navigation, workspace symbol search,
trait implementation navigation, call hierarchy, hover at resolved names, workspace references,
occurrence highlighting in the current file, completion for
local names, module members, receiver fields and methods, signature help for
source functions and resolved methods, parameter name and inferred local type hints,
diagnostic quick fixes, document formatting, outline, folding, and semantic
highlighting. Callable completions, hovers, and signature help include
declaration signatures and adjacent triple-slash documentation. Named field accesses
also support definition navigation and references. Receiver completion
checks method names from declarations in the current analysis graph.
When an unqualified name has no visible completion, the server searches
available modules and declared dependencies for imports. It checks each
completed name and added `using` together against the compiler before
returning the edits.
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
Those entries use the published build profile and CPU defaults alongside
each compiler action's target, runtime, and test mode. The server analyzes
manifest entries and warns when the build plan is corrupt or stale, or when a
build script has yet to publish a plan.

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
In clients that send file rename requests, renaming a declared module beside
its original file also updates the declaration and bound references. Include
the module's child directory in the same operation when present. A batch
can rename several independent modules in their respective directories.
Case-only renames follow the same checks.
Generated files are read-only to the LSP and their producer owns changes
to the generated declarations.

A generator can publish a `.nia.map` file beside its generated `.nia` file
in the package's build output directory. The configured command must declare
both paths as outputs and list the mapped package-relative files as inputs.
The map uses version 1 JSON. Both digests are lowercase BLAKE3 hex strings of
the exact UTF-8 file bytes. Ranges are half-open UTF-8 byte offsets. Multiple
generated ranges can point to the same input range. Generated ranges must
be disjoint.

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

Definition and type-definition navigation use the map when both snapshots
match the producer's digests. Byte-identical ranges preserve the location
within the mapped text. Transformed ranges link to the entire input range.
Unsaved changes to either file are checked against the same digests.

Workspace references and rename share an index for the current document
revision. Reference results can include resolved sources from entries
that have diagnostics. Workspace requests run independently of interactive
analysis and stop at the next cancellation checkpoint when the input changes.
Workspace symbol searches accept names and qualifiers such as
`app::geometry::Point` and include declarations in unsaved buffers.

Formatting uses the lossless syntax tree to normalize indentation, operator
spacing, and trailing whitespace. It preserves comments, literals, and existing
line breaks. Range formatting applies edits to the lines intersecting the
selection. The editor sends its indentation setting. The CLI
command `nia fmt path/to/file.nia` prints the formatted text to stdout with
four-space indentation.

For a repeatable latency sample, run the stdio benchmark with Python 3.9+
and a release server binary. It reports request round trips, versioned
diagnostic arrival, and a semantic highlight fetch after diagnostics. It
also records rename, cancellation, and peak resident memory. Benchmark
edits stay in the editor session.

```sh
python tools/lsp-latency.py --server target/release/nia-lsp --resource-root lib \
  --workspace lib/std --source lib/std/collections/array_list/list.nia --symbol ArrayList \
  --configuration "release"
```

Set `NIA_LSP_TIMINGS=summary` for a JSON timing report on stderr at
shutdown. Use `detail` to include individual compiler query timings.

Build from the repository root with `cargo build --locked -p nia-lsp`.
On Windows, use an x64 Developer PowerShell with the configured LLVM
installation and add
`--no-default-features --features llvm-static --target x86_64-pc-windows-msvc`.
Run the resulting server with `--resource-root` pointing at the checkout's
`lib` directory. An executable placed in a toolchain's `bin` can locate
the adjacent resources without an argument.

## VS Code

Put the release archive's `bin` directory on `PATH` or set
`nia.server.path` to the installed server executable. Build a VSIX from
`editors/vscode` with `npm ci` and `npx @vscode/vsce package --skip-license`.
Use **Extensions: Install from VSIX…** to install the resulting file. For an
uninstalled checkout build, set `nia.server.resourceRoot` to its `lib` directory.
The repository license is available here. [LICENSE.md](../LICENSE.md)

The extension uses the existing Nia square brand icon. Select **Nia File Icons**
from **Preferences: File Icon Theme** to enable it for `.nia` files.
The extension enables semantic highlighting for Nia by default.

## Neovim

With Neovim 0.11 or later, add the client package to your runtime path and
enable its LSP configuration. Put the release archive's `bin` on `PATH`.
The file type enables LSP folding and Neovim's semantic token highlighting.

```lua
vim.opt.runtimepath:prepend("/path/to/nia/editors/neovim")
vim.lsp.enable("nia")
```

For an uninstalled checkout build, override the command before enabling
the server. An icon plugin that uses glyphs can use the Nia brand color.

```lua
vim.lsp.config("nia", {
  cmd = { "/path/to/nia-lsp", "--resource-root", "/path/to/nia/lib" },
})
require("nvim-web-devicons").set_icon({
  nia = { icon = "󰅩", color = "#b8235a", name = "Nia" },
})
```

## Vim

Add `/path/to/nia/editors/vim` to `runtimepath` before `filetype plugin on`.
Put the release toolchain's `bin` on `PATH`. For semantic highlighting and
folding with coc.nvim, add this to your `coc-settings.json`.

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

Use `:Fold` in a Nia buffer with coc.nvim. With vim-devicons, set
`g:WebDevIconsUnicodeDecorateFileNodesExtensionSymbols['nia']` to
`'󰅩'` after loading the icon plugin.
