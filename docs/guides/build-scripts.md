# Custom Builds

Most packages build from their manifest alone: `nia.toml` declares the
library, executables, and test suites, and `nia build` derives the steps; see
[Packages and dependencies](packages.md). A package that needs more adds a
`build.nia` file at its root. Typical reasons are:

- a step that runs a program with fixed arguments, or after the tests pass;
- source files generated during the build, such as a version module;
- external tools, such as a code generator or an asset compressor;
- artifacts for other targets, object files, static archives, or native
  libraries to link;
- reading the environment, such as an environment variable or a `VERSION`
  file, to decide what to build.

This guide starts with a small script that extends a manifest package, then
covers each part of the `std::build` API that scripts use.

## A First Script

Start from the default package, created with `nia new greet`, and make its
program greet each of its arguments:

```nia ignore
// src/main.nia
using std::io;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    for arg in init.args().skipProgram() {
        io::debugPrint("hello, {}\n", [arg]).?;
    }
    !()
}
```

The manifest's `run-greet` step runs the program without arguments, so it
prints nothing. A script can declare fixed arguments and make that step wait
until the tests have passed. Arguments supplied after `--` are appended to
those fixed arguments for that invocation:

```nia
// build.nia
using std::build;

pub fun build(b: &mut build::Build) build::Error!() {
    let exe = b.executable("greet").?;
    let arguments: [&[char]; 2] = ["Ada", "Grace"];
    let options = build::RunOptions::init(exe).withArguments(arguments);
    let greetAll = b.addRunExecutableStep("greet-all", options).?;
    let test = b.step("test").?;
    b.dependOn(greetAll, test).?;
    !()
}
```

The script is one public function, `build`, which receives the build being
declared. Line by line:

- `b.executable("greet")` returns a handle to the executable that `nia.toml`
  already declares. The manifest's artifacts and steps exist before the script
  runs, so the script can build on them.
- `RunOptions::init(exe).withArguments(arguments)` describes how to run it.
  Options are built with an `init` constructor followed by `with*` methods,
  each of which returns an updated copy.
- `addRunExecutableStep` adds a step named `greet-all`.
- `b.step("test")` finds the manifest's `test` step, and `dependOn` makes
  `greet-all` run after it.

Every call returns an error union, and `.?` passes a failure back to `nia build`,
which reports it. The new step appears beside the derived ones:

```sh
nia build --list
```

```text
all           aggregate (default)
greet         compiler-emit
greet-all     run-executable
run-greet     run-executable
test          aggregate
unit          test-executable
```

```sh
nia build greet-all
```

```text
hello, Ada
hello, Grace
```

`nia build greet-all` builds the executable and the test suite, runs the
suite, and only then runs the program.

## How A Build Runs

`build.nia` is an ordinary Nia program. `nia build` compiles it for the host
and runs it, and the script declares modules, artifacts, commands, generated
files, tests, and named steps through `std::build`. These calls only record a
build graph. Nothing is compiled, run, or written while the script executes.

When the script returns, the graph is frozen and validated as a whole:
duplicate names, unknown handles, dependency cycles, conflicting outputs, and
invalid paths are errors even in parts that the invocation does not select.
`nia build` then executes only the selected step and the steps it depends on.

A script cannot register callbacks, pass raw compiler arguments, or give a
command interactive input. Its result depends only on the package, the host and
artifact targets, and the facts it records through
[configuration probes](#reading-the-environment), so the same inputs always
produce the same graph. When none of those inputs change, the next invocation
reuses the recorded graph without running the script again. Editing a source
file of any package in the build, or selecting a different step, profile, or
optimization level, runs the script again.

## Declaring Artifacts In The Script

A script can also declare artifacts that the manifest does not, which is
needed when an artifact imports a generated file or another package's file by
a custom name. A script that declares its own executable from scratch looks
like this:

```nia
using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {
    let planner = b.addModule(
        build::ModuleOptions::init("planner", fs::PathView::init("src/main.nia")),
    ).?;
    let exe = b.addExecutable(build::ExecutableOptions::init("planner", planner)).?;
    let emit = b.addEmitExecutableStep("planner", exe).?;
    b.setDefaultStep(emit).?;
    !()
}
```

The three calls build on each other: a module is a root source file, an
executable links a module, and a step is what `nia build <step>` runs. Every
`add*` call returns a handle (`ModuleHandle`, `ExecutableHandle`,
`StepHandle`, and so on). Paths given as `fs::PathView` are relative to the
package root. Names are plain identifiers such as `planner` or `run-tests`;
they cannot contain path separators. `setDefaultStep` replaces `all` as the
step that a plain `nia build` runs.

Options take further `with*` methods in a chain:

```nia ignore
build::ExecutableOptions::init("planner", planner)
    .withOutputName("planner-cli")
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

## Dependencies In A Script

A dependency's library needs nothing in the script: code opens it with
`using <name>;`, as [Adding Dependencies](packages.md#adding-dependencies)
describes. The script needs a handle to a dependency only to do something
else with it, such as importing a file other than its library entry, passing
one of its files to a command, or importing an optional dependency at all.

`b.addDependency(name)` returns that handle. It accepts any name from
`[dependencies]`, `[dev-dependencies]`, `[optional-dependencies]`, or
`[lazy-dependencies]`, whatever the source, path, Git, or registry. A
development dependency's sources can still be compiled only into test suites.
Names from `[build-dependencies]` are rejected, because `build.nia` imports
those directly. `b.rootPackage()` is the handle of the package that contains
`build.nia`.

```toml
[dependencies]
mathlib = { path = "vendor/mathlib" }
```

```nia ignore
let mathlib = b.addDependency("mathlib").?;
let imports = [
    build::ModuleImport::fromPackage("mathlib", mathlib, fs::PathView::init("root.nia")),
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

- `withStaticArchives([archive, ...])` to link static archives declared in the
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
| `withDebugInfo(DebugInfo::None)` / `LineTables` / `Full` | Records the requested debug metadata in the plan. |
| `withPanicStrategy(PanicStrategy::Abort)` / `Unwind` | Records the requested panic strategy in the plan. |
| `withLto(LtoMode::Off)` / `Thin` / `Full` | Selects the final-link optimization policy. |
| `withDefines(&[ArtifactDefine])` | Records ordered symbols in the plan. Use `ArtifactDefine::init(name)` for each symbol. |
| `withSystemLibraries(&[SystemLibrary])` | Adds ordered native libraries for the linker. Use `SystemLibrary::init(name, mode)` with `Default`, `Static`, or `Dynamic` search mode. |

For example:

```nia ignore
let app = b.addExecutable(
    build::ExecutableOptions::init("app", app)
        .withLinkage(build::Linkage::Dynamic)
        .withDebugInfo(build::DebugInfo::LineTables)
        .withPanicStrategy(build::PanicStrategy::Abort)
        .withLto(build::LtoMode::Thin)
        .withDefines([build::ArtifactDefine::init("APP_FEATURE")])
        .withSystemLibraries([
            build::SystemLibrary::init("m", build::SystemLibraryMode::Default),
        ]),
).?;
```

Linkage, LTO, and system libraries change how the artifact is linked; linkage
applies to Linux executables. Debug information, the panic strategy, and
defines are recorded in the plan and its cache identity, but the compiler does
not act on them. Changing any configured field invalidates the affected check,
emit, and link actions, and `--print-plan` shows the frozen values.

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
them, for example `if b.hostTarget().os().equals("windows") { ... }`.

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

`RunOptions::init(exe).withArguments(args)` passes arguments to the program.
A run step needs an emit step for the same executable, and depends on it
automatically; manifest executables already have one. Additional invocation
arguments can be supplied with `nia build <run-step> -- <args...>` and are
appended after the arguments configured in `RunOptions`.
These arguments are applied after configuration: changing them reuses the
configured plan and compiled executable, but still runs the program with the
new arguments. They are not available to `build.nia`.

`b.dependOn(step, dependency)` makes `step` run after `dependency`.
`b.setDefaultStep(step)` chooses what `nia build` runs without a step name; a
script that has no default step can still be used with an explicit step name.

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
    let mathlib = b.addDependency("mathlib").?;
    let imports = [
        build::ModuleImport::init("greeting", fs::PathView::init("src/greeting.nia")),
        build::ModuleImport::fromPackage("mathlib", mathlib, fs::PathView::init("root.nia")),
        build::ModuleImport::fromBuild("version", build::BuildPathView::init("gen/version.nia")),
    ];
    let app = b.addModule(
        build::ModuleOptions::init("app", fs::PathView::init("src/main.nia"))
            .withImports(imports),
    ).?;
    let exe = b.addExecutable(build::ExecutableOptions::init("app", app)).?;

    _ = b.addGeneratedFileStep(
        "generate-version",
        build::BuildPathView::init("gen/version.nia"),
        b"pub const text: [char; 3] = \"1.0\";\n",
    ).?;
    let emit = b.addEmitExecutableStep("app", exe).?;
    _ = b.addCheckExecutableStep("check", exe).?;
    let runArgs: [&[char]; 1] = ["world"];
    _ = b.addRunExecutableStep("run", build::RunOptions::init(exe).withArguments(runArgs)).?;
    let install = b.addInstallExecutableStep("install", exe, build::BuildPathView::init("dist/app")).?;

    let all = b.addAggregateStep("all").?;
    b.dependOn(all, emit).?;
    b.dependOn(all, install).?;
    b.setDefaultStep(all).?;

    let tests = b.addModule(
        build::ModuleOptions::init("tests", fs::PathView::init("tests/main.nia"))
            .withImports(imports),
    ).?;
    _ = b.addTestSuite("unit", tests).?;
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

```nia ignore
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
    io::debugPrint("{}{} {} {}\n", [prefix, name, squared, text]).?;
    !()
}
```

`nia build run` prints `hello world 49 1.0`, and `nia build` leaves
`build/out/app` and `build/out/dist/app`.

## Running External Commands

`ExternalCommandOptions::search(program)` runs a program found on the `PATH`
of the command's environment, falling back to the host's when the command
does not declare one. A name containing a separator is relative to the
working directory instead. On Windows a name without an extension also tries
each `PATHEXT` extension, so `search("cmd")` finds `cmd.exe`; probes and
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

- `withEnvironment([CommandEnvironmentInput::init(name, value), ...])` sets
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
| `withToolAndInputCache()` | the declared inputs are unchanged; clears the environment |
| `withFullEnvironmentCache()` | the declared inputs and the whole inherited environment are unchanged; cannot be combined with `withEnvironment` |

Every cache policy also compares the contents of the program itself. A cache
policy is a promise that the typed arguments describe every file the command
reads and writes. A cacheable command runs from a temporary working
directory containing only its declared inputs, so a relative read of an
undeclared file fails. Links and input trees beyond the depth, entry, or byte
limits are rejected. Only the typed input arguments (`packageInput`,
`buildInput`, `artifactInput`, `objectInput`, `staticArchiveInput`) and declared
stdin are mapped into that view; literal arguments keep their exact text.
Declared outputs still go to staged build paths, and a cacheable command that
creates, changes, or deletes any other file in the view fails. Every external
command is stopped after seven minutes. This is
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
        build::CommandArgument::literal("-c"),
        build::CommandArgument::literal("tr a-z A-Z < \"$1\" > \"$2\""),
        build::CommandArgument::literal("upper"),
        build::CommandArgument::packageInput(package, fs::PathView::init("input.txt")),
        build::CommandArgument::buildOutput(build::BuildPathView::init("upper.txt")),
    ];
    let upper = b.addExternalCommandStep(
        "upper",
        build::ExternalCommandOptions::search("sh")
            .withArguments(arguments)
            .withClearedEnvironment()
            .withDeclaredInputCache(),
    ).?;
    b.setDefaultStep(upper).?;
    !()
}
```

## Tests

Suites declared with `[[test]]` in `nia.toml` need nothing in the script.
`addTestSuite(name, module)` declares one from the script instead, for
example to give a suite its own module imports. It builds `module` as an
executable for the host target and registers it as a test suite. The suite is
a test compilation: `@[test]` items are kept and development dependencies are
visible, in the suite's whole module tree. Other artifacts in the same build,
including one built from the same module, are ordinary compilations.

A suite is a step named after it, and the first suite also declares a `test`
step that depends on every suite. A suite passes when its executable exits with
status 0, runs with the package root as its working directory, and is not run
again until its executable or inputs change. Writing the suite program itself
is covered in [Testing](../learn/testing.md).

## Reading The Environment

A script that depends on the outside world reads it through a *configuration
probe*. Each probe
records what it observed, and returns what the table shows; the next `nia build` compares the recorded
facts with the current ones and runs the script again when any of them
differs, including a value that was missing and now exists. Reads that bypass
probes, such as opening files directly, are not tracked.

| Call | Records |
| --- | --- |
| `b.probeEnv(name)` | an environment variable: `?&[u8]`, `null` when unset |
| `b.probeFile(path, buffer)` | the contents of a package file that fits in `buffer` (at most 1 MiB) |
| `b.probeFilePresence(path)` | whether a package-relative path is a regular file: `bool`; creating or removing the file reruns configuration |
| `b.probeDirectory(path, buffer)` | the entries (names, kinds, sizes) of a package directory; returns nothing |
| `b.probeCommand(program, args, buffer)` | the standard output and exit status of a command run in the package root; returns the output (at most 64 KiB, 256 arguments) |
| `b.probeTool(program, buffer)` | the output of `program --version` and the program's identity |
| `b.probeGitState(buffer)` | the output of `git status --porcelain=v1 --branch` |

Buffers are caller-owned byte arrays, and a result larger than its buffer is an
error. Commands run by probes get no standard input, their standard error is
discarded, and they cannot write build outputs.

```nia
using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {
    let release = match b.probeEnv("APP_RELEASE").? {
        ?value => value.len() != 0,
        null => false,
    };
    let mut versionBuffer: [u8; 64] = [0; 64];
    let version = b.probeFile(fs::PathView::init("VERSION"), versionBuffer).?;

    let options = build::ModuleOptions::init("app", fs::PathView::init("src/main.nia"));
    let app = b.addModule(
        if release { options.withOptimization(build::OptimizationMode::O2) } else { options },
    ).?;
    let exe = b.addExecutable(build::ExecutableOptions::init("app", app)).?;
    let emit = b.addEmitExecutableStep("build", exe).?;
    _ = b.addGeneratedFileStep("version", build::BuildPathView::init("version.txt"), version).?;
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

## Summary

- `build.nia` defines `pub fun build(b: &mut build::Build) build::Error!()`,
  which records a build graph; nothing runs while it executes.
- The manifest's artifacts and steps exist first: `b.executable(name)` and
  `b.step(name)` return their handles, so a script can extend them.
- Modules, artifacts, and steps are declared with `add*` calls and connected
  with `dependOn`; `setDefaultStep` picks what a plain `nia build` runs.
- External commands take typed arguments so the build knows their inputs and
  outputs, and rerun on every build unless they choose a cache policy.
- Probes are the only tracked way to read the environment; the script reruns
  when a recorded fact changes.

The full API is documented in the source of
[`std::build`](../../lib/std/build.nia).

