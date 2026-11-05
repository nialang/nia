# Packages And Dependencies

A Nia project is a package: a directory with a `nia.toml` manifest that names
the package, says what it builds, and lists the packages it depends on. This
guide walks through creating a package, building and testing it, adding
dependencies, locking their versions, and grouping packages into a workspace.

Most projects need nothing beyond `nia.toml`. A package that generates files,
runs external tools, or needs custom steps adds a `build.nia` script, described
in [Build scripts](build-scripts.md). Every manifest key is listed in the
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
`nia new tool --exe` only an executable and its tests. `nia init` creates the
same files in an existing directory without replacing any that already exist;
it defaults to the current directory. Both take `--name` when the package name
should differ from the directory name. A package name is a Nia identifier, so
`my_app` is valid and `my-app` is not.

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
unit.compile  compiler-emit
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
name for different dependencies. A package that others depend on must have a
`[library]`.

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

A name must be unique across all of a package's dependency tables. No
dependency is loaded eagerly: Nia reads a dependency's source only when a
module being compiled imports it. An optional dependency is reached through
`ModuleImport::fromPackage` in a build script; see
[Packages](build-scripts.md#packages).

### Git Dependencies

A Git dependency takes at most one selector: `rev` (a full 40-digit commit
hash), `tag`, or `branch`. Without one, it follows the remote `HEAD`. Nia
fetches Git sources itself, with a shallow fetch of the selected commit, and
accepts `https://`, `http://`, `git://`, `ssh://`, SCP-style `git@host:path`,
and `file:///` URLs. URLs cannot embed credentials. `http://` and `git://` are
neither encrypted nor authenticated; prefer HTTPS or SSH.

For HTTPS, Nia first requests the repository anonymously. When the server
answers `401`, Nia asks `git credential fill` for a username and password
matching the repository host and path and retries with HTTP Basic
authentication; public repositories never reach the credential helper.
Accepted credentials are reported back with `git credential approve`, and
rejected ones with `git credential reject`, so the helper keeps only working
credentials. This uses the configured Git credential helper (such as Git
Credential Manager); the Git executable is needed only for this lookup, while
Nia still performs the fetch itself. Credentials are held in memory for the
request and are not written to the manifest, lockfile, or cache. Nia does not
send credentials over plain HTTP.

HTTP Git sources honor `HTTP_PROXY` and `http_proxy`; HTTPS sources honor
`HTTPS_PROXY` and `https_proxy`. Both fall back to `ALL_PROXY` or `all_proxy`,
and `NO_PROXY` or `no_proxy` can bypass the proxy for listed hosts or domains,
optionally restricted to a port. Proxies must be unauthenticated `http://`
URLs. HTTPS requests use CONNECT and validate TLS against the repository host;
repository credentials are sent only inside that encrypted connection.

SSH sources verify the server key against `~/.ssh/known_hosts` (or
`%USERPROFILE%\.ssh\known_hosts` on Windows), accepting plain and hashed
OpenSSH entries, and reject unknown or changed host keys. Nia offers the
Ed25519, ECDSA P-256, and RSA (with RSA-SHA2-512 signatures) identities in the
running SSH agent (`SSH_AUTH_SOCK` on Unix-like systems or the OpenSSH agent
named pipe on Windows) in the order the agent lists them, as `ssh-add -l`
shows, followed by an unencrypted `~/.ssh/id_ed25519`. Like OpenSSH, it asks
the server whether it accepts each key before signing, and it offers at most
five keys per connection to stay under the server's default attempt limit. A
key the agent declines to sign, for example a declined `ssh-add -c`
confirmation, is skipped for the next one. When authentication fails, the
error lists each offered key's fingerprint as `ssh-keygen -l` prints it and
what happened to it. Only public-key authentication is supported, so a server
that also requires a password or other method fails after accepting the key.
Other key types and encrypted key files are not read directly; load a supported
key into the agent instead.

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

| Command | Effect |
| --- | --- |
| `nia update` | refreshes every locked Git selector and registry version, and reports revision changes separately from version changes |
| `nia update json regex` | refreshes only the named dependencies |
| `nia build --locked` | fails instead of adding, changing, or removing a lock entry; use it in CI |
| `nia build --offline` | uses cached Git checkouts, registry indexes, and archives without fetching |

## Workspaces

A workspace groups related packages under one root manifest, so they share a
lockfile, a build directory, and common settings:

```sh
nia new mono --workspace --member app --member core
```

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

| Field | Meaning |
| --- | --- |
| `members` | member directories; glob patterns such as `packages/*` are allowed |
| `default-members` | members built when `nia build` runs at the root; all members when omitted |
| `exclude` | directories under the root that are independent packages, not members |

Every package under the workspace directory must be a member or excluded, and
a nested workspace cannot be a member of its parent. The root manifest may also
declare a `[package]` of its own; one without it is a virtual workspace.

`nia build` inside a member builds that member. At the root it builds
`default-members`; `--workspace` selects every member, and
`-p app` (repeatable) selects members by name. Members build in manifest
order, and the first failure stops the build unless `--keep-going` is given.

## The Build Directory

Everything Nia writes lives in one `build/` directory beside the root
`nia.toml` (the workspace root in a workspace):

| Path | Contents |
| --- | --- |
| `build/out/` | build outputs; `build/out/<member path>/` for a workspace member |
| `build/cache/` | reusable results: compiled build scripts, configurations, compiler products, command results |
| `build/tmp/` | files of builds in progress |
| `build/packages/` | fetched Git checkouts and registry packages |
| `build/registry/` | downloaded registry indexes and archives |

`nia clean` removes `build/out/`, `build/cache/`, and `build/tmp/`, and keeps
fetched dependencies. Run from a workspace member, it removes only that
member's outputs but the whole workspace's shared caches. It refuses to run
outside a package.

A package's source files are its whole directory except `.git`, `nia.lock`, and
the `build/`, `dist/`, and `target/` directories directly under its root. The
manifest's `[source]` table can narrow or widen this; see the
[manifest reference](../reference/manifest.md#source).
