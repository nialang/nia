# Build Scripts

A package describes what it builds in a `build.nia` file at its root. This
guide covers the `std::build` API that the script uses and the `nia build`
command that runs it. [Getting started](getting-started.md) shows a minimal
package.

## How A Build Runs

A package declares its executables and test suites in `nia.toml`; see
[Manifest artifacts](#manifest-artifacts). `build.nia` is optional and adds
whatever the manifest cannot say. It is an ordinary Nia program. `nia build` compiles it for the host,
runs it, and the script declares modules, artifacts, commands, generated files,
tests, and named steps through `std::build`. These calls only record a build
graph. Nothing is compiled, run, or written while the script executes.

When the script returns, the graph is frozen and validated as a whole:
duplicate names, unknown handles, dependency cycles, conflicting outputs, and
invalid paths are errors even in parts that the invocation does not select.
`nia build` then executes only the selected step and the steps it depends on.

A script cannot register callbacks, pass raw compiler arguments, or give a
command interactive input. Its result depends only on the package, the host and
artifact targets, and the facts it records through
[configuration probes](#configuration-probes), so the same inputs always
produce the same graph. When none of those inputs change, the next invocation
reuses the recorded graph without running the script again.

## Package Manifest

Before compiling `build.nia`, Nia reads the package's `nia.toml` and resolves
its dependencies. The manifest defines package identity, dependency inputs,
and the executables and test suites the package builds. A root package with
only `build.nia` builds as a package with no dependencies, but every
dependency must have a `nia.toml`.

```toml
[package]
name = "app"
version = "0.1.0"

[library]
path = "src/root.nia"

[build-dependencies]
json = { path = "vendor/json" }

[dependencies]
regex = { git = "https://example.com/regex.git", tag = "v1.2.0" }

[dev-dependencies]
fixtures = { path = "tests/fixtures" }
```

| Key | Meaning |
| --- | --- |
| `package.name` | required package name |
| `package.namespace` | identity namespace; defaults to `local` |
| `package.version` | required strict SemVer identity, or explicitly inherited from `workspace.package.version` |
| `package.protocol-epoch` | rejects compilers from another release line when set |
| `library.path` | the file that dependent packages open with `using <dependency>;` and the package's own artifacts open with `using <package-name>;` |
| `source.roots`, `source.exclude` | package-relative files that belong to the package; default the whole directory |
| `[[executable]]`, `[[test]]` | artifacts built without a script; see [Manifest artifacts](#manifest-artifacts) |

A package's files always leave out `.git` and `nia.lock`, and leave out
`build/`, `dist/`, and `target/` directly under the package root. Name one of
those three in `source.roots` to include it.

A dependency's scope is the table it appears in:

| Table | Available to |
| --- | --- |
| `[build-dependencies]` | `build.nia` only |
| `[dependencies]` | package artifacts and their tests |
| `[dev-dependencies]` | test suites only |
| `[optional-dependencies]` | artifact modules only when the build plan explicitly imports them |
| `[lazy-dependencies]` | artifact module resolution; source is loaded only when imported |

A dependency name is local to the package that declares it and must be unique
across its dependency tables. Runtime and development artifact sources open an
available dependency under its manifest name, `using regex;`. Optional
dependencies are not installed as general module roots; a build plan must add
an explicit `ModuleImport::fromPackage` for an artifact module to use one.
Lazy dependencies are available to runtime module resolution, while Nia's
module loader reads their source only when the selected module closure imports
them. Neither scope eagerly loads every source file in the dependency.

The name resolves to the dependency's `[library].path`, so a package that other
packages import must declare one; the file must lie inside its source roots.
The library entry roots its own module tree, so its child modules sit beside
it: with `path = "src/root.nia"`, `pub module util;` loads `src/util.nia`. Each
dependency resolves its own dependencies from its own manifest, so two packages
can use one name for different packages.

A package's own artifacts and tests open its library the same way, by the
package name: in package `hello`, `using hello;` opens `[library].path`. The
library and each executable are separate module trees, so an executable sees
only the library's `pub` items and `pub(root)` stays inside the library. When
`src/root.nia` and `src/main.nia` share a directory, a `module util;` in both
would load `src/util.nia` into two trees, which is an error: the library
declares it and the executable writes `using hello::util;`. A dependency cannot
also use the package's own name.

### Dependency sources

A dependency names exactly one source:

| Source | Form |
| --- | --- |
| Local path | `{ path = "vendor/json" }`, relative to the declaring package |
| Git | `{ git = "<url>" }` plus at most one of `rev`, `tag`, or `branch` |
| Registry | `{ registry = "nia", package = "acme/json", version = "^1.4" }`; see [Registry dependencies](#registry-dependencies) |
| Workspace | `{ workspace = true }`; see [Workspaces](#workspaces) |

`rev` must be a full 40-digit commit hash. Without a selector, Git sources
follow the remote `HEAD`. Nia fetches Git sources itself and accepts
`https://`, `http://`, `git://`, `ssh://`, SCP-style `git@host:path`, and
`file:///` URLs. URLs cannot embed credentials. `http://` and `git://` are
neither encrypted nor authenticated; prefer HTTPS or SSH.

For HTTPS, Nia first requests the repository anonymously. When the server
answers `401`, Nia asks `git credential fill` for a username and password
matching the repository host and path and retries with HTTP Basic
authentication; public repositories never reach the credential helper.
Accepted credentials are reported back with `git credential approve`, and
rejected ones with `git credential reject`, so the helper keeps only working
credentials.
This uses the configured Git credential helper (such as Git Credential
Manager); the Git executable is needed only for this lookup, while Nia still
performs the fetch itself. Credentials are held in memory for the request and
are not written to the manifest, lockfile, or cache. Nia does not send
credentials over plain HTTP.

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

Git fetches request a shallow history with depth 1, which includes the selected
commit and the files needed to check it out without downloading its ancestors.

### Registry dependencies

A registry dependency names a configured registry, a `namespace/name` package,
and a SemVer requirement. The requirement must state its operator: `^1.4`,
`~1.4.2`, `>=1.1, <2`, `=1.4.2`, or `1.*`; a bare `1.4` is rejected rather than
read as `^1.4`. Registry dependencies cannot also carry `path`, `git`, or Git
selector fields.

Registries are configured by name in the root `nia.toml`, the workspace root in
a workspace; a workspace member that declares `[registries]` is rejected:

```toml
[registries]
nia = "https://packages.example.test"
```

Endpoints must be `https://` URLs without credentials, a query, or a fragment.
Registry names and package segments use lower-case ASCII letters, digits, and
`-`.

Nia selects one version of each registry package across the whole graph,
including every selected workspace member and dependency scope: the highest
version that satisfies every requirement on it. Prereleases are selected only
when a requirement names a prerelease. Conflicting requirements fail with each
requiring package, its requirement, and its dependency chain. A package can be
requested from only one registry within a graph.

Archives are checked against the digest in the registry index before they are
unpacked, and their package trees are cached only after the whole tree
validates.

The first build records each Git dependency's commit and content hash, and each
registry dependency's selected version, archive digest, and content hash, in
`nia.lock` next to the root `nia.toml` (the workspace root in a workspace).
Later builds reuse the locked source and reject cached content that no longer
matches its recorded hash.
Entries that no manifest in the package or workspace still reaches, in any
dependency scope, are removed on the next build. Commit `nia.lock`; everything
else Nia writes is under `build/` (see [Build directory](#build-directory)).

`nia update` refreshes every locked Git selector and registry version, then
reports revision changes separately from version changes. `nia update <name>
...` refreshes only those dependencies. It resolves every package that shares
the lock file, so running it from one workspace member updates the whole
workspace. `nia build` accepts `--locked`,
which fails instead of adding, changing, or removing a lock entry, and
`--offline`, which uses cached Git checkouts, registry indexes, and registry
archives only.
CI builds should pass `--locked`.

## Build directory

Nia writes everything except `nia.lock` to one `build/` directory beside the
root `nia.toml`, the workspace root in a workspace, so a single `build/` line in
`.gitignore` covers it:

| Path | Contents |
| --- | --- |
| `build/out/` | build outputs; `build/out/<member path>/` for a workspace member |
| `build/cache/` | reusable results: the compiled script, configurations, compiler products, command results |
| `build/tmp/` | files of builds in progress |
| `build/packages/` | fetched Git checkouts and registry packages |
| `build/registry/` | downloaded registry indexes and archives |

Workspace members share `build/cache/`, `build/packages/`, and
`build/registry/`. `nia clean` removes `build/out/`, `build/cache/`, and
`build/tmp/`, and keeps fetched dependencies. It runs from anywhere inside a
package and refuses to run outside one.

## Workspaces

A workspace groups packages under one root `nia.toml`. The root can be virtual
or can also declare a package. Members remain ordinary packages with their own
`nia.toml` and `build.nia` files:

```toml
[workspace]
members = ["crates/core", "crates/cli"]
default-members = ["crates/cli"]
exclude = ["vendor"]

[workspace.package]
namespace = "acme"
version = "0.1.0"

[workspace.dependencies]
json = { path = "vendor/json" }
```

Members opt into shared identity fields and dependencies with
`{ workspace = true }`. A workspace dependency's path is rooted at the
workspace directory. For example, a member can place `json = { workspace =
true }` under `[dependencies]` or `[build-dependencies]` to select the shared
source in that scope. An inherited dependency must contain only
`workspace = true`; adding source, package, version, or Git selector fields is
an error.

Member and default-member paths accept glob patterns such as `crates/*`.
Exclusions apply to matching directories and their descendants. A package
under the workspace directory must be listed as a member or explicitly
excluded; an excluded package builds independently. A nested workspace
cannot also be a member of its parent workspace.

Running `nia build` inside a member selects that package. Running
from the workspace root selects `default-members`, or all members when that
field is omitted. `--workspace` selects every member, and repeatable
`-p`/`--package <name>` selects named members. Workspace members share the
root `nia.lock` and the workspace root's `build/`; see
[Build directory](#build-directory).

Selected members build in manifest order, and the first failing member stops
the build. With `--keep-going`, the remaining members still build and the
build fails after they finish.

## Manifest Artifacts

```toml
[[executable]]
name = "planner"
path = "src/main.nia"

[[test]]
name = "unit"
path = "tests/main.nia"
runtime = "freestanding"
```

| Key | Meaning |
| --- | --- |
| `name` | artifact and step name; unique across both tables |
| `path` | the artifact's root module file, relative to the package root |
| `runtime` | `freestanding` (default) or `bare` |

Each `[[executable]]` becomes an emit step `<name>` and a run step
`run-<name>`; each `[[test]]` becomes a suite, as `addTestSuite` makes. `all`
depends on every emit step and suite and is the default step. A manifest
artifact cannot be named `all`, `test`, or `run-<anything>`, since those names
belong to the derived steps.

The manifest artifacts are declared before `build.nia` runs, so the script can
extend them: `b.executable(name)` returns an artifact's handle, and
`b.step(name)` returns a step's handle, such as `test` or `run-planner`.
`setDefaultStep` replaces `all` as the default.

## The Build Function

The script defines one public function:

```nia
using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {
    let planner = b.addModule(
        build::ModuleOptions::init(&"planner", fs::PathView::init(&"src/main.nia")),
    ).?;
    let exe = b.addExecutable(build::ExecutableOptions::init(&"planner", planner)).?;
    let emit = b.addEmitExecutableStep(&"planner", exe).?;
    b.setDefaultStep(emit).?;
    !()
}
```

Every `add*` call returns a handle (`ModuleHandle`, `ExecutableHandle`,
`StepHandle`, and so on) or a `build::Error`, which `.?` passes back to the
runner. Paths given as `fs::PathView` are relative to the package root. Names
are plain identifiers such as `planner` or `run-tests`; they cannot contain
path separators.

Options are built with an `init` constructor followed by `with*` methods, each
of which returns an updated copy:

```nia
build::ExecutableOptions::init(&"planner", root)
    .withOutputName(&"planner-cli")
    .forTarget(b.hostTarget())
```

## Modules

A module is a root source file plus the names it can import.

| Constructor | Root source |
| --- | --- |
| `ModuleOptions::init(name, path)` | a file in the root package |
| `ModuleOptions::fromPackage(name, package, path)` | a file in another package selected with `addDependency` or declared with `addPackage` |
| `ModuleOptions::fromBuild(name, buildPath)` | a generated file under `build/out/` |

`withOptimization(build::OptimizationMode::O2)` fixes the optimization level
for one module (`O0`, `O1`, `O2`, `O3`, `Os`, `Oz`). Other modules use the
level selected on the command line (`-O2`, `--release`, and so on).

`withImports` lists the extra names that the module's source can open with
`using`. Each `ModuleImport` maps an import name to a root file:

| Constructor | Imported file |
| --- | --- |
| `ModuleImport::init(name, path)` | a file in the root package |
| `ModuleImport::fromPackage(name, package, path)` | a file in another package |
| `ModuleImport::fromBuild(name, buildPath)` | a generated file under `build/out/` |

The same import list can be passed to several modules, for example an
application and its tests. Each module still declares its own imports.

## Packages

`b.rootPackage()` is the package that contains `build.nia`. A dependency with
a `[library].path` needs nothing in the script; see
[Package manifest](#package-manifest). Use `addDependency` when the script
needs a handle to a dependency package itself, for example to import a file
other than its library entry or to pass one of its files to a command. It
takes the dependency name from `[dependencies]` or `[dev-dependencies]`, so
path and Git dependencies use the same API; only test suites can compile a
development dependency's sources. Build
dependencies are rejected because `build.nia` already imports them directly:

```toml
[dependencies]
mathlib = { path = "vendor/mathlib" }
```

```nia
let mathlib = b.addDependency(&"mathlib").?;
let imports = [
    build::ModuleImport::fromPackage(&"mathlib", mathlib, fs::PathView::init(&"root.nia")),
];
```

A module with these imports can then write `using mathlib;`. Inside the
imported package, `root::...` refers to that package's own modules.
`addPackage(PackageOptions)` is reserved for temporary local source trees
that are not declared or managed by `nia.toml`.

## Artifacts

| Call | Produces |
| --- | --- |
| `addExecutable(ExecutableOptions)` | a linked executable |
| `addObject(ObjectOptions)` | native object files |
| `addStaticArchive(StaticArchiveOptions)` | a static library archive |

Each options type takes `init(name, rootModule)` and supports
`withOutputName(fileName)` and `forTarget(target)`. The output name defaults to
the artifact name and must be a plain file name. Executables also support:

- `withStaticArchives(&[archive, ...])` to link static archives declared in the
  same script;
- `withRuntime(build::Runtime::Bare)` to link no startup code. Executables are
  `build::Runtime::Freestanding` by default; see
  [the runtime model](../reference/platform-support.md#runtime-model).

Object files and static archives do not select a runtime.

### Artifact Configuration

All three artifact option types support the same output-changing configuration
methods. Each method returns a new options value, so configuration is part of
the artifact declaration and is frozen into the build plan:

| Method | Meaning |
| --- | --- |
| `withLinkage(Linkage::Static)` / `StaticPie` / `Dynamic` | Selects the native linkage form. Executables default to `StaticPie`; objects and archives default to `Static`. |
| `withDebugInfo(DebugInfo::None)` / `LineTables` / `Full` | Selects emitted debug metadata. |
| `withPanicStrategy(PanicStrategy::Abort)` / `Unwind` | Selects panic lowering for the artifact. |
| `withLto(LtoMode::Off)` / `Thin` / `Full` | Selects the final-link optimization policy. |
| `withDefines(&[ArtifactDefine])` | Adds the ordered conditional-compilation symbols passed to the compiler. Use `ArtifactDefine::init(name)` for each symbol. |
| `withSystemLibraries(&[SystemLibrary])` | Adds ordered native libraries for the linker. Use `SystemLibrary::init(name, mode)` with `Default`, `Static`, or `Dynamic` search mode. |

For example:

```nia
let app = b.addExecutable(
    build::ExecutableOptions::init(&"app", root)
        .withLinkage(build::Linkage::Dynamic)
        .withDebugInfo(build::DebugInfo::LineTables)
        .withPanicStrategy(build::PanicStrategy::Abort)
        .withLto(build::LtoMode::Thin)
        .withDefines(&[build::ArtifactDefine::init(&"APP_FEATURE")])
        .withSystemLibraries(&[
            build::SystemLibrary::init(&"m", build::SystemLibraryMode::Default),
        ]),
).?;
```

The configuration is artifact-owned rather than an invocation-global compiler
flag. Changing any configured field invalidates the affected check, emit, and
link actions; `--print-plan` and `--explain` show the frozen values.

### Targets

An artifact without `forTarget` is built for the invocation's artifact target:
the host, or the target given with `nia build --target <name>`.

| Call | Returns |
| --- | --- |
| `b.hostTarget()` | the target the build runs on, for tools that run during the build |
| `b.artifactTarget()` | the invocation's artifact target |
| `b.target(name)` | a [supported target](../reference/platform-support.md#targets) by name, or an error |

A `TargetView` exposes `name()`, `arch()`, `vendor()`, `os()`, `env()`,
`endian()`, `pointerWidth()`, and `equals(other)`. The script can branch on
them, for example `if b.hostTarget().os().equals(&"windows") { ... }`.

## Steps

Steps are the units that `nia build <step>` runs. Each step has a unique name.

| Call | The step |
| --- | --- |
| `addEmitExecutableStep(name, exe)` | builds the executable into `build/out/` |
| `addEmitObjectStep(name, object)` | writes the object files |
| `addEmitStaticArchiveStep(name, archive)` | writes the static archive |
| `addCheckExecutableStep(name, exe)` | type-checks the executable without producing it |
| `addRunExecutableStep(name, RunOptions)` | builds and runs an executable |
| `addInstallExecutableStep(name, exe, buildPath)` | copies the executable to a path under `build/out/` |
| `addInstallStaticArchiveStep(name, archive, buildPath)` | copies the archive to a path under `build/out/` |
| `addGeneratedFileStep(name, buildPath, bytes)` | writes a file with fixed contents under `build/out/` |
| `addExternalCommandStep(name, ExternalCommandOptions)` | runs a program |
| `addAggregateStep(name)` | does nothing itself; groups dependencies |
| `addTestSuite(name, module)` | builds and runs a test executable; see [Tests](#tests) |

`RunOptions::init(exe).withArguments(&args)` passes arguments to the program.
A run step needs an emit step for the same executable, and depends on it
automatically.

`b.dependOn(step, dependency)` makes `step` run after `dependency`.
`b.setDefaultStep(step)` chooses what `nia build` runs without a step name; a
script that has no default step can still be used with an explicit step name.
`nia build --list` prints every step with the kind of work it does, marking
the default, without running any.

Paths written with `build::BuildPathView::init(path)` are relative to
`build/out/`. Use slash-separated relative paths without `.` or `..` components,
backslashes, colons, or NUL characters. Colons are excluded because Windows
interprets them as drive-prefix or alternate-data-stream syntax. A module or
import whose root is a generated file depends on the step that writes it, so it
needs no explicit `dependOn`.

This script combines a second package, a generated module, several steps, and
an aggregate default:

```nia
using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {
    let mathlib = b.addDependency(&"mathlib").?;
    let imports = [
        build::ModuleImport::init(&"greeting", fs::PathView::init(&"src/greeting.nia")),
        build::ModuleImport::fromPackage(&"mathlib", mathlib, fs::PathView::init(&"root.nia")),
        build::ModuleImport::fromBuild(&"version", build::BuildPathView::init(&"gen/version.nia")),
    ];
    let app = b.addModule(
        build::ModuleOptions::init(&"app", fs::PathView::init(&"src/main.nia"))
            .withImports(&imports),
    ).?;
    let exe = b.addExecutable(build::ExecutableOptions::init(&"app", app)).?;

    _ = b.addGeneratedFileStep(
        &"generate-version",
        build::BuildPathView::init(&"gen/version.nia"),
        &b"pub const text: [char; 3] = \"1.0\";\n",
    ).?;
    let emit = b.addEmitExecutableStep(&"app", exe).?;
    _ = b.addCheckExecutableStep(&"check", exe).?;
    let runArgs: [&[char]; 1] = [&"world"];
    _ = b.addRunExecutableStep(&"run", build::RunOptions::init(exe).withArguments(&runArgs)).?;
    let install = b.addInstallExecutableStep(&"install", exe, build::BuildPathView::init(&"dist/app")).?;

    let all = b.addAggregateStep(&"all").?;
    b.dependOn(all, emit).?;
    b.dependOn(all, install).?;
    b.setDefaultStep(all).?;

    let tests = b.addModule(
        build::ModuleOptions::init(&"tests", fs::PathView::init(&"tests/main.nia"))
            .withImports(&imports),
    ).?;
    _ = b.addTestSuite(&"unit", tests).?;
    !()
}
```

`src/greeting.nia` and `vendor/mathlib/root.nia` are ordinary modules:

```nia
// src/greeting.nia
pub const prefix: [char; 6] = "hello ";
```

```nia
// vendor/mathlib/root.nia
pub fun square(value: i32) i32 {
    value * value
}
```

`src/main.nia` opens the three imports by name:

```nia
using std::io;
using std::process;
using greeting;
using mathlib;
using version;

pub fun main(init: process::Init) process::ExitCode!() {
    let name = match init.args().get(1) {
        ?argument => argument,
        null => return process::ExitCode(2)!,
    };
    let prefix = greeting::prefix;
    let squared = mathlib::square(7);
    let text = version::text;
    io::debugPrint(&"{}{} {} {}\n", &[&prefix, &name, &squared, &text]).?;
    !()
}
```

`nia build run` prints `hello world 49 1.0`, and `nia build` leaves
`build/out/app` and `build/out/dist/app`.

## External Commands

`ExternalCommandOptions::search(program)` runs a program found on the `PATH`
of the command's environment, falling back to the host's when the command
does not declare one. A name containing a separator is relative to the
working directory instead. On Windows a name without an extension also tries
each `PATHEXT` extension, so `search(&"cmd")` finds `cmd.exe`; probes and
`std::process::Command` follow the same rules. The
command runs in the package root unless `withPackageWorkingDirectory(path)`
names a package-relative directory, and its standard input is closed.

Arguments are typed so that the build knows which files a command reads and
writes:

| Argument | Passed as | Effect |
| --- | --- | --- |
| `CommandArgument::literal(text)` | the text | none |
| `CommandArgument::packageInput(package, path)` | a file path | the file is an input |
| `CommandArgument::buildInput(buildPath)` | a path under `build/out/` | the file is an input; its producer runs first |
| `CommandArgument::buildOutput(buildPath)` | a path under `build/out/` | the command must write it |
| `CommandArgument::artifactInput(exe)` | the executable's path | the executable is built first |
| `CommandArgument::objectInput(object)` | the object output path | the objects are built first |
| `CommandArgument::staticArchiveInput(archive)` | the archive's path | the archive is built first |

The remaining options:

- `withEnvironment(&[CommandEnvironmentInput::init(name, value), ...])` sets
  variables; `withClearedEnvironment()` starts from an empty environment
  instead of inheriting the caller's.
- `withStdin(argument)` feeds a declared file to standard input.
- `withResourceClass(build::ActionResourceClass::Cpu)` or `::Io` tells the
  scheduler what the command mostly uses, so it can run next to other work.
  The default, `Conservative`, runs the command alone.

A command reruns on every build unless it selects a cache policy:

| Option | Reused when |
| --- | --- |
| `withDeclaredInputCache()` | the declared inputs and the explicit environment are unchanged; needs `withClearedEnvironment()` and at least one `buildOutput` |
| `withToolAndInputCache()` | additionally, the program itself is unchanged; clears the environment |
| `withFullEnvironmentCache()` | the declared inputs and the whole inherited environment are unchanged; cannot be combined with `withEnvironment` |

A cache policy is a promise that the typed arguments describe every file the
command reads and writes. A cacheable command runs from a temporary working
directory containing only its declared inputs, so a relative read of an
undeclared file fails. Links and input trees beyond the depth, entry, or byte
limits are rejected. Only `InputPath` arguments and declared stdin are mapped
into that view; literal arguments keep their exact text. Declared outputs still
go to staged build paths. This is
not an OS-level sandbox: absolute paths and paths outside that working view are
not intercepted, so commands requiring them must remain uncacheable. This step
upper-cases a package file into the build directory and reruns only when
`input.txt` changes. It needs a POSIX `sh` and `tr` on the `PATH`:

```nia
using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {
    let package = b.rootPackage();
    let arguments = [
        build::CommandArgument::literal(&"-c"),
        build::CommandArgument::literal(&"tr a-z A-Z < \"$1\" > \"$2\""),
        build::CommandArgument::literal(&"upper"),
        build::CommandArgument::packageInput(package, fs::PathView::init(&"input.txt")),
        build::CommandArgument::buildOutput(build::BuildPathView::init(&"upper.txt")),
    ];
    let upper = b.addExternalCommandStep(
        &"upper",
        build::ExternalCommandOptions::search(&"sh")
            .withArguments(&arguments)
            .withClearedEnvironment()
            .withDeclaredInputCache(),
    ).?;
    b.setDefaultStep(upper).?;
    !()
}
```

## Tests

`addTestSuite(name, module)` builds `module` as an executable for the host
target and registers it as a test suite. The suite is a test compilation:
`@[test]` items are kept and development dependencies are visible, in the
suite's whole module tree. Other artifacts in the same build, including one
built from the same module, are ordinary compilations. A suite passes when its
executable exits with status 0, and it runs with the package root as its
working directory. A suite that passed is not run again until its executable
or inputs change.

A suite is a step named after it, and the first suite also declares a `test`
step that depends on every suite.
Tests run through `nia build`:

| Command | Effect |
| --- | --- |
| `nia build test` | runs every suite, stopping at the first failure |
| `nia build test --keep-going` | runs every suite and reports all failures |
| `nia build <suite>` | runs one suite |
| `nia build --list` | lists the steps, suites included, without running them |

A suite is one program, so it may import any number of test modules. Inside
it, `std::test` provides assertions that return `test::Error!()` and a runner:

| API | Purpose |
| --- | --- |
| `test::expect(condition)` | fails when `condition` is false |
| `test::expectEqual(actual, expected)` | fails when the values differ (`Eq` bound) |
| `test::expectNotEqual(actual, expected)` | fails when the values are equal |
| `test::fail()` | always fails |
| `test::Case::init(name, &function)` | one named case; the function returns `test::Error!()` |
| `test::Runner::init(&cases)` | runs a borrowed case table; `.withFailFast()` skips cases after a failure |
| `runner.run()` | runs the cases and returns a `RunSummary` |
| `runner.runWith(&mut reporter)` | also passes each `CaseResult` to a `&mut Fun(test::CaseResult) ()` closure |

A failed assertion records the source location of the failing call, which
`CaseResult::error()` returns. `RunSummary` exposes `total()`, `passed()`,
`failed()`, `skipped()`, and `isSuccessful()`.

```nia
using std::io;
using std::process;
using std::test;

fun addsNumbers() test::Error!() {
    test::expectEqual(2 + 2, 4).?;
    !()
}

fun findsText() test::Error!() {
    let text = "hello";
    test::expect(text.len() == 5).?;
    test::expectNotEqual(text[0], 'j').?;
    !()
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let cases = [
        test::Case::init(&"adds numbers", &addsNumbers),
        test::Case::init(&"finds text", &findsText),
    ];
    let mut failures = 0;
    let summary = test::Runner::init(&cases).runWith(&mut \[&mut failures] result: test::CaseResult -> {
        if result.isFailed() {
            failures.* += 1;
            _ = io::debugPrint(&"failed: {}\n", &[result.name()]);
        }
    });
    if not summary.isSuccessful() {
        return process::ExitCode(1)!;
    }
    !()
}
```

## Configuration Probes

A script that depends on the outside world reads it through a probe. Each probe
returns its result and records it; the next `nia build` compares the recorded
facts with the current ones and runs the script again when any of them
differs, including a value that was missing and now exists. Reads that bypass
probes, such as opening files directly, are not tracked.

| Call | Records |
| --- | --- |
| `b.probeEnv(name)` | an environment variable: `?&[u8]`, `null` when unset |
| `b.probeFile(path, &mut buffer)` | the contents of a package file that fits in `buffer` (at most 1 MiB) |
| `b.probeFilePresence(path)` | whether a package-relative path is a regular file: `bool`; creating or removing the file reruns configuration |
| `b.probeDirectory(path, &mut buffer)` | the entries (names, kinds, sizes) of a package directory |
| `b.probeCommand(program, &args, &mut buffer)` | the standard output and exit status of a command run in the package root |
| `b.probeTool(program, &mut buffer)` | the output of `program --version` and the program's identity |
| `b.probeGitState(&mut buffer)` | the output of `git status --porcelain=v1 --branch` |

Buffers are caller-owned byte arrays, and a result larger than its buffer is an
error. Commands run by probes get no standard input, their standard error is
discarded, and they cannot write build outputs.

```nia
using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {
    let release = match b.probeEnv(&"APP_RELEASE").? {
        ?value => value.len() != 0,
        null => false,
    };
    let mut versionBuffer: [u8; 64] = [0; 64];
    let version = b.probeFile(fs::PathView::init(&"VERSION"), &mut versionBuffer).?;

    let options = build::ModuleOptions::init(&"app", fs::PathView::init(&"src/main.nia"));
    let app = b.addModule(
        if release { options.withOptimization(build::OptimizationMode::O2) } else { options },
    ).?;
    let exe = b.addExecutable(build::ExecutableOptions::init(&"app", app)).?;
    let emit = b.addEmitExecutableStep(&"build", exe).?;
    _ = b.addGeneratedFileStep(&"version", build::BuildPathView::init(&"version.txt"), version).?;
    b.setDefaultStep(emit).?;
    !()
}
```

Setting `APP_RELEASE` or editing `VERSION` makes the next build configure
again; otherwise the recorded graph is reused.

`b.packageRoot()`, `b.buildDir()`, `b.cacheDir()`, and `b.requestedStep()` (the
step named on the command line, or `null`) are also available. Errors from
other `std` APIs inside the script convert to `build::Error` with
`.withBuildContext(operation, subject)`.

## Running `nia build`

```text
nia build [step] [--root <dir>] [options]
```

`nia build` searches upward from the current directory for `nia.toml` or
`build.nia`, or starts at `--root <dir>`. It runs `step`, or the default step when no step is
named, after the steps it depends on.

| Option | Effect |
| --- | --- |
| `-p, --package <name>` | selects a workspace member; repeatable |
| `--workspace` | selects every workspace member |
| `--locked` | fails instead of changing `nia.lock` |
| `--offline` | uses cached Git checkouts, registry indexes, and registry archives only |
| `--dry-run` | configures and validates the graph, and predicts action-cache decisions without running build actions |
| `--explain` | shows the selected step, targets, action closure, and configure-cache decision |
| `--print-plan` | prints the frozen graph and selection context, including module imports, artifact outputs, action outputs, and step dependencies |
| `--print-cache` | prints configure and action cache hits and misses; with `--dry-run`, reports predicted outcomes without restoring outputs or publishing records |
| `--print-observations` | prints the facts recorded by probes |
| `--list` | prints every step and its kind without running any |
| `-k, --keep-going` | after a failure, keeps running the actions that do not depend on it |
| `-j, --jobs <count>` | limits concurrent actions |
| `--target <name>` | sets the artifact target |
| `--debug`, `--release`, `--profile <debug\|release>` | selects the build profile; `--release` defaults to `-O2` |
| `-O<level>` | sets the default optimization level, overriding the profile's |
| `--lto <off\|thin\|full>` | selects link-time optimization for executables |

Text action-cache records show the stable outcome and reason, publication
status, declared outputs, and any invalidated identity components. JSON records
expose the same decision as structured fields.

Outputs go to `build/out/` ([Build directory](#build-directory)): emitted
artifacts under their output names, plus installed and generated files at the
paths the script gave.

### Progress output

`nia build` reports progress on stderr. The products you ask for
(`--print-plan`, `--print-cache`, `--print-observations`, `--explain`,
`--list`) go to stdout, as does the standard output of external
commands and test suites in text modes. A failed command's report names its
exit status; it repeats the command's last output only with
`--diagnostics-format=json`, where that output was captured instead of shown.

Each action the selected steps need is counted once, and ends with one outcome:

| Outcome | Meaning |
| --- | --- |
| cached | the result was reused from `build/cache/` |
| executed | the action ran |
| no-work | the step has nothing of its own to do, such as an aggregate |
| failed | the action failed |
| cancelled | the action did not run because another one failed |

The build finishes with a summary such as
`Finished 3 actions in 982ms (1 cached, 1 executed, 1 no-work)`, with failed
and cancelled counts added when there are any. A `--dry-run` or
`--list` ends with `Configured build in ... (N selected actions)`.

A run step, or any external command that declares no outputs, gets the
terminal instead: the summary of what finished so far comes before it,
followed by `Running root:run`, and nothing is printed after the program
unless the build fails. `nia build run` therefore ends with the program's own
output, as `cargo run` does. From `-v`, progress keeps reporting every action
and closes with the full summary.

Repeat `-v` for more detail. Each level includes the one before it:

| Level | Text output |
| --- | --- |
| `-q`, `--quiet` | no progress; diagnostics and requested products remain |
| default | the summary, or the summary and `Running` line before a run step |
| `-v` | whether the build runner and plan were reused, then one line per action in plan order, such as `[1/3] Cached root:app (compiler-emit, 699ms)`; a no-work action is shown as `Complete` |
| `-vv` | waves of ready actions, each action's start with its queue and resource-permit wait, waits for dependencies, locks, and resource permits, and resource-permit usage |
| `-vvv` | the cache decision on each completion line, such as `; cache miss: not found`, and the compiler's checking, code generation, and linking activities |

`-q` cannot be combined with `-v`, and the level stops at `-vvv`.

`--progress` (also written `--progress <mode>`) selects the output form:

| Mode | Output |
| --- | --- |
| `auto` (default) | a live status that redraws in place, when stderr is a terminal, `TERM` is not `dumb`, and `CI` is unset; plain text otherwise. With `--diagnostics-format=json`, no progress |
| `plain` | append-only text lines |
| `json` | JSON Lines, described below |
| `none` | no progress |

The live status is one line, `Building [completed/total actions, n active]`
followed by the first active action and the elapsed time. From `-vv` it becomes
a panel of up to eight rows: the wave and counts, resource-permit usage with
the number of active, queued, and dependency-blocked actions, then the pending
actions in plan order with their waits. It first appears after 250ms and
redraws at most every 100ms, so short builds print only the summary.

### JSON progress

`--progress=json` makes stderr entirely machine-readable: diagnostics default
to JSON, and timings to JSON when `--timings` is on. Combining it with an
explicit text format, or with `-q`, is an error, as is `--progress=plain` with
`--diagnostics-format=json`. External command output is captured rather than
forwarded.

Every line is one object with `"kind": "build-progress"`, `"schema_version": 1`,
`event`, `sequence`, and `elapsed_ms`. `sequence` increases but may skip
numbers. Actions are written as `{"package": ..., "name": ...}`, and an action's
`ordinal` is its 1-based position in plan order.

| Event | Level | Fields |
| --- | --- | --- |
| `build-started` | default | none |
| `build-finished`, `build-failed` | default | `completed`, `total` (omitted when the build failed before selecting actions) |
| `phase-started`, `phase-finished` | default | `phase`: `resolve`, `prepare`, `runner`, `configure`, `preview`, or `execute`; finish adds `duration_ms`, `success` |
| `selection-ready` | default | `total`, `actions` in plan order |
| `action-finished` | default | `action`, `ordinal`, `action_kind`, `outcome`, `cache` (for cacheable actions), `duration_ms`, `completed`, `total` |
| `phase-cache-decision` | `-v` | `phase`, `reused` |
| `action-started` | `-v` | `action`, `resource_class`, `wave`, `scheduler_wait_ms`, `resource_wait_ms` |
| `action-cache-decision` | `-v` | `action`, `cache` |
| `wave-started` | `-vv` | `wave` (0-based), `actions` |
| `action-queued` | `-vv` | `action`, `resource_class`, `wave` |
| `action-wait-started`, `action-wait-finished`, `phase-wait-started`, `phase-wait-finished` | `-vv` | `wait`, then `outcome` and `duration_ms` |
| `resource-usage-changed` | `-vv` | `capacity`, `used`, `generation` |
| `action-described` | `-vvv` | `action_kind`, `inputs`, `outputs`, `total_inputs`, `total_outputs` |
| `action-activity-started`, `action-activity-finished` | `-vvv` | `activity`: `checking`, `codegen`, `linking`, or `compiling and linking`; finish adds `duration_ms`, `success` |

`outcome` on `action-finished` is `cached`, `executed`, `nowork`, `failed`, or
`cancelled`. `cache` is `{"status": "hit"}` or `{"status": "miss", "reason":
...}` with `reason` `not_found`, `invalidated`, `uncacheable`, `corrupt`,
`read_error`, or `write_error`. `wait` is `dependency`, `resource permit`, `output lock`, or
`cache lock`, and a wait's `outcome` is `acquired`, `cancelled`, or `failed`.
`resource_class` is `cpu`, `io`, or `conservative`. Paths in `inputs` and
`outputs` are `{"root": {"kind": ...}, "path": ...}`, relative to a package,
the build or cache directory, the toolchain, or an artifact. Progress never
includes process arguments, environment values, or generated file contents.
Later releases may add fields within schema version 1.
