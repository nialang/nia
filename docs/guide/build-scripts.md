# Build Scripts

A package describes what it builds in a `build.nia` file at its root. This
guide covers the `std::build` API that the script uses and the `nia build` and
`nia test` commands that run it. [Getting started](getting-started.md) shows a
minimal package.

## How A Build Runs

`build.nia` is an ordinary Nia program. `nia build` compiles it for the host,
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

## The Build Function

The script defines one public function:

```nia
using std::build;
using std::fs;

pub fn build(b: &mut build::Build) build::Error!() {
    let root = b.addModule(
        build::ModuleOptions::init(&"planner", fs::PathView::init(&"src/main.nia")),
    ).?;
    let exe = b.addExecutable(build::ExecutableOptions::init(&"planner", root)).?;
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
| `ModuleOptions::fromPackage(name, package, path)` | a file in another package declared with `addPackage` |
| `ModuleOptions::fromBuild(name, buildPath)` | a generated file under `.nia-build/` |

`withOptimization(build::OptimizationMode::O2)` fixes the optimization level
for one module (`O0`, `O1`, `O2`, `O3`, `Os`, `Oz`). Other modules use the
level selected on the command line (`-O2`, `--release`, and so on).

`withImports` lists the extra names that the module's source can open with
`using`. Each `ModuleImport` maps an import name to a root file:

| Constructor | Imported file |
| --- | --- |
| `ModuleImport::init(name, path)` | a file in the root package |
| `ModuleImport::fromPackage(name, package, path)` | a file in another package |
| `ModuleImport::fromBuild(name, buildPath)` | a generated file under `.nia-build/` |

The same import list can be passed to several modules, for example an
application and its tests. Each module still declares its own imports.

## Packages

`b.rootPackage()` is the package that contains `build.nia`. `addPackage`
declares another source tree, such as a vendored library, by name and
package-relative root:

```nia
let mathlib = b.addPackage(
    build::PackageOptions::init(&"mathlib", fs::PathView::init(&"vendor/mathlib")),
).?;
let imports = [
    build::ModuleImport::fromPackage(&"mathlib", mathlib, fs::PathView::init(&"lib.nia")),
];
```

A module with these imports can then write `using mathlib;`. Inside the
imported package, `pkg::...` refers to that package's own modules.

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
| `addEmitExecutableStep(name, exe)` | builds the executable into `.nia-build/` |
| `addEmitObjectStep(name, object)` | writes the object files |
| `addEmitStaticArchiveStep(name, archive)` | writes the static archive |
| `addCheckExecutableStep(name, exe)` | type-checks the executable without producing it |
| `addRunExecutableStep(name, RunOptions)` | builds and runs an executable |
| `addInstallExecutableStep(name, exe, buildPath)` | copies the executable to a path under `.nia-build/` |
| `addInstallStaticArchiveStep(name, archive, buildPath)` | copies the archive to a path under `.nia-build/` |
| `addGeneratedFileStep(name, buildPath, bytes)` | writes a file with fixed contents under `.nia-build/` |
| `addExternalCommandStep(name, ExternalCommandOptions)` | runs a program |
| `addAggregateStep(name)` | does nothing itself; groups dependencies |
| `addTestSuite(name, module)` | builds and runs a test executable; see [Tests](#tests) |

`RunOptions::init(exe).withArguments(&args)` passes arguments to the program.
A run step needs an emit step for the same executable, and depends on it
automatically.

`b.dependOn(step, dependency)` makes `step` run after `dependency`.
`b.setDefaultStep(step)` chooses what `nia build` runs without a step name; a
script that has no default step can still be used with an explicit step name.

Paths written with `build::BuildPathView::init(path)` are relative to
`.nia-build/`. A module or import whose root is a generated file depends on
the step that writes it, so it needs no explicit `dependOn`.

This script combines a second package, a generated module, several steps, and
an aggregate default:

```nia
using std::build;
using std::fs;

pub fn build(b: &mut build::Build) build::Error!() {
    let mathlib = b.addPackage(
        build::PackageOptions::init(&"mathlib", fs::PathView::init(&"vendor/mathlib")),
    ).?;
    let imports = [
        build::ModuleImport::init(&"greeting", fs::PathView::init(&"src/greeting.nia")),
        build::ModuleImport::fromPackage(&"mathlib", mathlib, fs::PathView::init(&"lib.nia")),
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

`src/main.nia` opens the three imports by name:

```nia
using std::fmt;
using std::io;
using std::process;
using greeting;
using mathlib;
using version;

pub fn main(init: process::Init) process::ExitCode!() {
    let name = match init.args().get(1) {
        ?argument => argument,
        null => return process::ExitCode(2)!,
    };
    let prefix = greeting::prefix();
    let squared = mathlib::square(7);
    let text = version::text;
    io::debugPrint(&"{}{} {} {}\n", &[&prefix, &name, &squared, &text]).?;
    !()
}
```

`nia build run` prints `hello world 49 1.0`, and `nia build` leaves
`.nia-build/app` and `.nia-build/dist/app`.

## External Commands

`ExternalCommandOptions::search(program)` runs a program found on the host
`PATH`. On Windows the name includes its extension, such as `cmd.exe`. The
command runs in the package root unless `withPackageWorkingDirectory(path)`
names a package-relative directory, and its standard input is closed.

Arguments are typed so that the build knows which files a command reads and
writes:

| Argument | Passed as | Effect |
| --- | --- | --- |
| `CommandArgument::literal(text)` | the text | none |
| `CommandArgument::packageInput(package, path)` | a file path | the file is an input |
| `CommandArgument::buildInput(buildPath)` | a path under `.nia-build/` | the file is an input; its producer runs first |
| `CommandArgument::buildOutput(buildPath)` | a path under `.nia-build/` | the command must write it |
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
command reads and writes. This step upper-cases a package file into the build
directory and reruns only when `input.txt` changes:

```nia
using std::build;
using std::fs;

pub fn build(b: &mut build::Build) build::Error!() {
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
target and registers it as a test suite. `nia test` builds and runs every
suite. A suite passes when its executable exits with status 0, and it runs
with the package root as its working directory. A suite that passed is not run
again until its executable or inputs change. A suite is also a step, so
`nia build <name>` runs one suite.

| Command | Effect |
| --- | --- |
| `nia test` | runs every suite |
| `nia test --list` | prints suite names without running them |
| `nia test --filter <text>` | runs suites whose names contain the text |
| `nia test --fail-fast` | stops after the first failing suite |

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
| `runner.runWith(&mut reporter)` | also passes each `CaseResult` to a `&mut Fn(test::CaseResult) ()` closure |

A failed assertion records the source location of the failing call, which
`CaseResult::error()` returns. `RunSummary` exposes `total()`, `passed()`,
`failed()`, `skipped()`, and `isSuccessful()`.

```nia
using std::fmt;
using std::io;
using std::process;
using std::test;

fn addsNumbers() test::Error!() {
    test::expectEqual(2 + 2, 4).?;
    !()
}

fn findsText() test::Error!() {
    let text = "hello";
    test::expect(text.len() == 5).?;
    test::expectNotEqual(text[0], 'j').?;
    !()
}

pub fn main(init: process::Init) process::ExitCode!() {
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

pub fn build(b: &mut build::Build) build::Error!() {
    let release = match b.probeEnv(&"APP_RELEASE").? {
        ?value => value.len() != 0,
        null => false,
    };
    let mut versionBuffer: [u8; 64] = [0; 64];
    let version = b.probeFile(fs::PathView::init(&"VERSION"), &mut versionBuffer).?;

    let options = build::ModuleOptions::init(&"app", fs::PathView::init(&"src/main.nia"));
    let root = b.addModule(
        if release { options.withOptimization(build::OptimizationMode::O2) } else { options },
    ).?;
    let exe = b.addExecutable(build::ExecutableOptions::init(&"app", root)).?;
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

`nia build` searches upward from the current directory for `build.nia`, or
starts at `--root <dir>`. It runs `step`, or the default step when no step is
named, after the steps it depends on.

| Option | Effect |
| --- | --- |
| `--dry-run` | configures and validates the graph without running any step |
| `--explain` | shows the selected step, targets, action closure, and configure-cache decision |
| `--print-plan` | prints the frozen graph: modules, artifacts, actions, and steps |
| `--print-cache` | prints configure and action cache hits and misses |
| `--print-observations` | prints the facts recorded by probes |
| `-j, --jobs <count>` | limits concurrent actions |
| `--target <name>` | sets the artifact target |
| `-O<level>`, `--release` | sets the default optimization level |
| `--lto <off\|thin\|full>` | selects link-time optimization for executables |

Outputs go to `.nia-build/` in the package root: emitted artifacts under their
output names, plus installed and generated files at the paths the script gave.
Reusable results (the compiled script, recorded configurations, compiler
products, and command results) are kept in `.nia-cache/`.
