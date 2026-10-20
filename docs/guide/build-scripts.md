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

Before compiling `build.nia`, Nia reads the package's `nia.toml`. Dependency
scope is declared outside the build script: `[build-dependencies]` is available
only to the host build runner, `[dependencies]` is available to package targets,
and `[dev-dependencies]` is selected by test builds. A build dependency package
exports its build-time module through `[library].path`:

```toml
[package]
name = "app"
version = "0.1.0"

[build-dependencies]
json = { path = "vendor/json" }

[dependencies]
regex = { git = "https://example.com/regex.git", rev = "<commit>" }
```

The runner resolves these declarations before compiling the script and exposes
the declared build libraries by dependency name. The script still owns the
dynamic build graph; the manifest only defines package identity and dependency
inputs.

Git dependencies support HTTPS and SSH without invoking a system Git client.
SSH sources use `ssh://` or SCP-style URLs and verify server keys against
`~/.ssh/known_hosts` (or `%USERPROFILE%/.ssh/known_hosts` on Windows). Nia
authenticates with `~/.ssh/id_ed25519` (or `%USERPROFILE%/.ssh/id_ed25519` on
Windows); the key must be unencrypted. Plain and hashed OpenSSH host entries are
supported. Unknown or changed host keys are rejected.

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

`src/greeting.nia` and `vendor/mathlib/lib.nia` are ordinary modules:

```nia
// src/greeting.nia
pub const prefix: [char; 6] = "hello ";
```

```nia
// vendor/mathlib/lib.nia
pub fn square(value: i32) i32 {
    value * value
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
    let prefix = greeting::prefix;
    let squared = mathlib::square(7);
    let text = version::text;
    io::debugPrint(&"{}{} {} {}\n", &[&prefix, &name, &squared, &text]).?;
    !()
}
```

`nia build run` prints `hello world 49 1.0`, and `nia build` leaves
`.nia-build/app` and `.nia-build/dist/app`.

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
directory and reruns only when `input.txt` changes. It needs a POSIX `sh` and
`tr` on the `PATH`:

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

`nia test` takes the same `-q`, `-v`, and `--progress` options as `nia build`;
see [Progress output](#progress-output).

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

### Progress output

`nia build` and `nia test` report progress on stderr. The products you ask for
(`--print-plan`, `--print-cache`, `--print-observations`, `--explain`,
`nia test --list`) go to stdout, as does the standard output of external
commands and test suites in text modes. A failed command's report names its
exit status; it repeats the command's last output only with
`--diagnostics-format=json`, where that output was captured instead of shown.

Each action the selected steps need is counted once, and ends with one outcome:

| Outcome | Meaning |
| --- | --- |
| cached | the result was reused from `.nia-cache/` |
| executed | the action ran |
| no-work | the step has nothing of its own to do, such as an aggregate |
| failed | the action failed |
| cancelled | the action did not run because another one failed |

The build finishes with a summary such as
`Finished 3 actions in 982ms (1 cached, 1 executed, 1 no-work)`, with failed
and cancelled counts added when there are any. A `--dry-run` or
`nia test --list` ends with `Configured build in ... (N selected actions)`.

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
