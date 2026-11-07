# Packages And Dependencies

A Nia project is a package: a directory with a `nia.toml` manifest that names
the package, says what it builds, and lists the packages it depends on. This
guide walks through creating a package, building and testing it, adding
dependencies, locking their versions, and grouping packages into a workspace.

Most projects need nothing beyond `nia.toml`. A package that generates files,
runs external tools, or needs custom steps adds a `build.nia` script, described
in [Custom Builds](build-scripts.md). Every manifest key is listed in the
[manifest reference](../reference/manifest.md), and every `nia` command option
in the [command-line reference](../reference/cli.md).

## Creating A Package

`nia new` creates a package from a template:

```sh
nia new hello
cd hello
```

```text
hello/
├── .gitignore
├── nia.toml
├── lib/
│   └── root.nia
├── src/
│   └── main.nia
└── tests/
    └── unit.nia
```

The default template is a complete application with three parts:

- `lib/root.nia` is the package's library: code that other parts of the
  package, and other packages, can use.
- `src/main.nia` is an executable that uses the library.
- `tests/unit.nia` is a test suite for the library.

`nia new core --lib` creates only a library and its tests, and
`nia new tool --exe` only an executable and its tests. `nia new` refuses a
destination that already exists.

`nia init` turns an existing directory, by default the current one, into a
package. It writes the same files and keeps an existing `.gitignore`. It never
overwrites anything: it refuses to run if the directory already has a
`nia.toml` or a file the template would write, such as `src/main.nia`. Both
commands take `--name` when the package name should differ from the directory
name. A package name is a Nia identifier, so `my_app` is valid and `my-app` is
not.

The generated manifest describes those three parts:

```toml
[package]
name = "hello"
version = "0.1.0"
protocol-epoch = 3

[library]
path = "lib/root.nia"

[[executable]]
name = "hello"
path = "src/main.nia"

[[test]]
name = "unit"
path = "tests/unit.nia"
```

- `[package]` identifies the package. `version` is a SemVer version.
  `protocol-epoch` names the compiler release line the package was written
  for; a compiler from another line rejects the package with a clear error
  instead of failing in less obvious ways.
- `[library]` names the file that roots the library's module tree.
- Each `[[executable]]` and `[[test]]` names an artifact and the file that roots
  it.

The generated `.gitignore` excludes `/build/`, the directory where Nia keeps
everything it produces. Commit everything else, including `nia.lock` once it
appears.

## How The Parts Fit Together

The library, each executable, and each test suite are separate module trees.
An executable or a test opens the library by the package name, exactly as
another package would:

```nia ignore
// src/main.nia
using std::io;
using std::process;
using hello;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    io::debugPrint("{}\n", [hello::greeting()]).?;
    !()
}
```

Because the trees are separate, the executable sees only the library's `pub`
items, and `pub(root)` items stay inside the library. Each tree loads its own
child modules: with `path = "lib/root.nia"`, a `pub module util;` in the
library loads `lib/util.nia`. A source file can belong to only one tree, so if
the library and the executable share a directory, declare the child module in
the library and reach it from the executable with `using hello::util;`.

The library entry is the package's public face. Anything users need is
reachable from it, either declared there or re-exported with `pub using`. A
type's methods count too: if they live in a separate file, re-export that
file's module as described under
[Re-Exports](../reference/language.md#re-exports).

## Building, Running, And Testing

Every manifest artifact becomes a build step:

```sh
nia build --list
```

```text
all           aggregate (default)
hello         compiler-emit
run-hello     external-command
test          aggregate
unit          test-executable
```

| Command | What it does |
| --- | --- |
| `nia build` | runs the default step, `all`: builds every executable and runs every test suite |
| `nia build hello` | builds the `hello` executable into `build/out/hello` |
| `nia build run-hello` | builds `hello`, then runs it |
| `nia build test` | builds and runs every test suite |
| `nia build unit` | builds and runs the `unit` suite only |
| `nia build --release` | builds with optimizations (`-O2`) |
| `nia clean` | removes build outputs and caches |

`nia build` searches upward from the current directory for the package, so it
works from any subdirectory. Results are cached: a step whose inputs have not
changed is not repeated, and a test suite that passed is not rerun until its
code changes. Writing tests is covered in [Testing](../learn/testing.md).

A run step starts the program without extra arguments by default. Append
arguments after `--` to pass them to a selected run step, for example
`nia build run-hello -- --version`. Arguments declared in `build.nia` come
first; command-line arguments follow them. To run an executable without its
build step, invoke the built program directly from `build/out/`.

Names in `[[executable]]` and `[[test]]` must be unique across both tables, and
cannot be `all`, `test`, or start with `run-`, because those names belong to
the derived steps. An artifact may set `runtime = "bare"` to build without the
standard process entry; see [Targets And Runtimes](targets.md).

## Adding Dependencies

`nia add` records a dependency in the manifest:

```sh
nia add core --path ../core
```

```toml
[dependencies]
core = { path = "../core" }
```

The dependency's name is how the package's code imports it: `using core;`
opens the dependency's `[library].path`. A dependency name is a Nia identifier
and is local to the package that declares it, so two packages may use the same
name for different dependencies. Importing a package that has no `[library]`
is an error; declaring it as a dependency is not.

### Where A Dependency Comes From

A dependency names exactly one source:

| Source | `nia add` | Manifest entry |
| --- | --- | --- |
| Local path | `--path ../json` | `{ path = "../json" }`, relative to the declaring package |
| Git | `--git <url>` with optional `--tag`, `--branch`, or `--rev` | `{ git = "<url>", tag = "v1.0.0" }` |
| Registry | `--registry nia --package acme/json --version ^1.4` | `{ registry = "nia", package = "acme/json", version = "^1.4" }` |
| Workspace | — | `{ workspace = true }`; see [Workspaces](#workspaces) |

A path dependency must already be a valid package; `nia add` checks it.
Git and registry sources are fetched by the next build.

### Who Can Use A Dependency

The table a dependency appears in is its scope. `nia add --scope` selects it:

| Table | `--scope` | Available to |
| --- | --- | --- |
| `[dependencies]` | `runtime` (default) | the library, executables, and tests |
| `[dev-dependencies]` | `dev` | test suites only |
| `[build-dependencies]` | `build` | `build.nia` only |
| `[optional-dependencies]` | `optional` | only modules that a build script explicitly imports it into |
| `[lazy-dependencies]` | `lazy` | the same as `[dependencies]` |

A name must be unique across all of a package's dependency tables. Every
declared dependency, whichever table it is in, is resolved and locked before a
build starts, but its modules are read only when code being compiled imports
it. An optional dependency is reached through
`ModuleImport::fromPackage` in a build script; see
[Dependencies In A Script](build-scripts.md#dependencies-in-a-script).

### Git Dependencies

A Git dependency takes at most one selector: `rev` (a full 40-digit commit
hash), `tag`, or `branch`. Without one, it follows the remote `HEAD`. The
lockfile records the commit it resolved to, so a branch or `HEAD` dependency
moves only when you run `nia update`.

Nia fetches Git sources itself, without running `git`, from `https://`,
`ssh://`, SCP-style `git@host:path`, `git://`, `http://`, and `file:///` URLs.
Private repositories use the setup Git already has: over HTTPS, the configured
Git credential helper supplies a username and password; over SSH, the keys in
the running SSH agent are offered, and the server is checked against
`known_hosts`. URLs cannot embed credentials. The
[manifest reference](../reference/manifest.md#git-sources) describes
authentication, proxies, and the supported key types in detail.

### Registry Dependencies

A registry dependency names a configured registry, a `namespace/name` package,
and a SemVer requirement. The requirement must state its operator: `^1.4`,
`~1.4.2`, `>=1.1, <2`, `=1.4.2`, or `1.*`; a bare `1.4` is rejected rather than
read as `^1.4`.

Registries are configured by name in the root manifest (the workspace root in
a workspace):

```toml
[registries]
nia = "https://packages.example.test"
```

Nia has no built-in registry; `nia` is only a conventional name. Endpoints must
be `https://` URLs without credentials, a query, or a fragment. Registry names
and package segments use lower-case ASCII letters, digits, and `-`. A workspace
member cannot declare `[registries]`.

Nia selects one version of each registry package across the whole dependency
graph: the highest version that satisfies every requirement on it.
Prereleases are selected only when a requirement names a prerelease.
Conflicting requirements fail with each requiring package, its requirement,
and its dependency chain. A package can be requested from only one registry
within a graph. Archives are checked against the digest in the registry index
before they are unpacked.

## The Lockfile

The first build records exactly which source each dependency resolved to in
`nia.lock`, next to the root `nia.toml`: the content hash of a path dependency,
the commit and content hash of a Git dependency, and the version, archive
digest, and content hash of a registry dependency. Later builds use the locked
sources and reject cached content that no longer matches its recorded hash.
Entries that nothing in the package or workspace still uses are removed on the
next build. Commit `nia.lock` so every checkout builds the same dependencies.

Every `nia build` keeps the lockfile current, including `nia build --list` and
`nia build --dry-run`, which run nothing else, so a new dependency is locked
the first time any of them runs.

| Command | Effect |
| --- | --- |
| `nia update` | refreshes every locked Git selector and registry version, and reports revision changes separately from version changes |
| `nia update json regex` | refreshes only the named dependencies, which must be locked Git or registry dependencies |
| `nia build --locked` | fails instead of adding, changing, or removing a lock entry; use it in CI |
| `nia build --offline` | uses cached Git checkouts, registry indexes, and archives without fetching |

## Workspaces

A workspace groups related packages under one root manifest, so they share a
lockfile, a build directory, and common settings:

```sh
nia new mono --workspace --member app --member core
```

Each `--member` adds one package under `packages/`. Without `--member`, the
workspace starts with one member named after the workspace, here
`packages/mono`.

```text
mono/
├── .gitignore
├── nia.toml
└── packages/
    ├── app/
    │   ├── nia.toml
    │   ├── lib/root.nia
    │   ├── src/main.nia
    │   └── tests/unit.nia
    └── core/
        └── ...
```

```toml
# mono/nia.toml
[workspace]
members = ["packages/*"]

[workspace.package]
namespace = "local"
version = "0.1.0"
protocol-epoch = 3
```

```toml
# mono/packages/app/nia.toml
[package]
name = "app"
namespace = { workspace = true }
version = { workspace = true }
protocol-epoch = { workspace = true }
```

Members are ordinary packages with their own manifests. `{ workspace = true }`
inherits a field from `[workspace.package]`. Dependencies can be shared the
same way: declare them once under `[workspace.dependencies]` and write
`json = { workspace = true }` in a member's dependency table. An inherited
dependency contains only `workspace = true`, and a workspace dependency's
path is relative to the workspace root.

To add a member, create a package inside the workspace with `nia new` or
`nia init`. If the member patterns already cover its path, such as
`packages/tools` with `packages/*`, it is a member immediately; otherwise Nia
appends its path to `members`. At a virtual workspace root, `nia add` needs
`--root packages/app` to choose which member's manifest to edit.

`members` lists member directories and accepts glob patterns such as
`packages/*`. `default-members`, in the same form, narrows what `nia build`
builds at the root; without it, the root builds every member. `exclude` lists
directories under the root that hold independent packages rather than members.
A package under the workspace directory that is neither a member nor excluded
cannot be built: `nia build` inside it fails with
`package ... is not a member of workspace`. A nested workspace cannot be a
member of its parent. The root manifest may also declare a `[package]` of its
own; one without it is a virtual workspace. The
[manifest reference](../reference/manifest.md#workspace) lists every key.

`nia build` inside a member builds that member. At the root it builds
`default-members`; `--workspace` selects every member, and
`-p app` (repeatable) selects members by name. Members build in manifest
order, and the first failure stops the build unless `--keep-going` is given.

## Sharing A Package

Nia has no publish command. A package is shared as a Git repository, which
others add with `nia add <name> --git <url>`, or through a registry that
serves it. To vendor a dependency, copy it into the repository and depend on
it by path; `nia.lock` records its content hash, so an edit to the vendored
copy shows up as a lockfile change.

## The Build Directory

Everything Nia writes lives in one `build/` directory beside the root
`nia.toml`, the workspace root in a workspace. Executables land in
`build/out/`, fetched dependencies in `build/packages/` and `build/registry/`,
and reusable results in `build/cache/`. `nia clean` removes the outputs and
caches but keeps fetched dependencies. The
[command-line reference](../reference/cli.md#the-build-directory) lists each
directory.

A package's source files are its whole directory except `.git`, `nia.lock`, and
the `build/`, `dist/`, and `target/` directories directly under its root. The
manifest's `[source]` table can narrow or widen this; see the
[manifest reference](../reference/manifest.md#source).
