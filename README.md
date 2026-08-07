# Nia

**A tiny language that can actually work.**

Nia is a small systems programming language and `nia` is its compiler. It is
pre-1.0 and under active design, with an implementation that favors clear
semantics, predictable compilation phases, and a compact language surface.

The project is intentionally narrow: this repository contains the compiler,
language documentation, a small standard library, and teaching examples.
The toolchain-owned build system is developed in this repository. A future
package manager and registry remain separate projects.

## A Small Example

```nia
using std::process;
using std::slice;

struct Point {
    x: i32,
    y: i32,
}

extend Point {
    fn len2(&self) i32 {
        self.x * self.x + self.y * self.y
    }
}

fn sum(xs: &[i32]) i32 {
    let mut total = 0;
    for &value in xs {
        total += value;
    }
    total
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;

    let mut point: Point = { x: 3, y: 4 };
    let mut also_point = Point { x: 5, y: 12 };
    let mut values = [_]i32[point.len2(), also_point.len2(), 7];
    let mut slice_view = &([3]i32[1, 2, 3])[..];

    if point.len2() + sum(&values) + sum(slice_view) != 232 {
        return process::exit(1)!;
    }

    !{}
}
```

Most aggregate literals use the expected type from a binding, argument, or
return position. Explicit forms such as `Point { ... }` and `[_]i32[...]` are
available when the literal needs to stand on its own.
Array pointers coerce to slices at argument boundaries, so `sum(&values)` is
the usual style when a function expects `&[T]`; `&values[..]` is the explicit
range-slice form.

## Design Goals

- Keep the language small enough to understand as a whole.
- Provide the low-level control expected from a systems language.
- Make host and bare-metal use cases explicit instead of hiding startup and
  linking behavior behind one model.
- Keep the compiler modular so syntax, semantic checks, lowering, and codegen
  remain easy to inspect and evolve.

## Quick Start

Nia is a Rust workspace. It currently builds against LLVM through
`llvm-sys = 221.0.1`, so a compatible LLVM 22.1 installation with
`llvm-config` on `PATH` is required.

Build the compiler with:

```sh
cargo build --workspace
```

If LLVM was installed or switched after a failed build, clear the cached
`llvm-sys` probe result before trying again:

```sh
cargo clean -p llvm-sys
cargo build --workspace
```

For non-standard LLVM layouts, point `llvm-sys` at the install prefix:

```sh
LLVM_SYS_221_PREFIX=/path/to/llvm-22.1 cargo build --workspace
```

Run the compiler from the workspace with:

```sh
cargo run -p nia-cli -- --resource-root lib --help
```

For everyday use, build the release binary and create a symlink in
`~/.local/bin`:

```sh
cargo build --release -p nia-cli
mkdir -p "$HOME/.local/bin"
ln -sf "$PWD/target/release/nia" "$HOME/.local/bin/nia"
```

Make sure `~/.local/bin` is on `PATH`, then verify the command:

```sh
nia --resource-root "$PWD/lib" --version
nia --resource-root "$PWD/lib" check examples/00_minimal.nia --runtime freestanding
```

This explicit source-tree workflow is the recommended pre-1.0 installation
path. The compiler never infers a checkout path; `--resource-root lib` selects
the versioned resource tree deliberately. Keep the repository in place after
creating the symlink. To update:

```sh
git pull
cargo build --release -p nia-cli
```

`cargo install` is not the recommended workflow yet because it installs only the
compiler binary and does not create the required installed tree. An installed
toolchain resolves resources as `../lib/nia` relative to `bin/nia`; that tree
must contain `toolchain.meta`, `std.nia`, and the std/runtime sources. Packaging
and installation automation remains future work.

The compiler binary is named `nia`. Its core pipeline commands are:

```text
nia build [step] [--root dir]
nia check <file.nia> [--runtime bare|freestanding] [--opt-report]
nia emit --tokens <file.nia>
nia emit --ast <file.nia>
nia emit --checked <file.nia> [--runtime bare|freestanding] [--opt-report]
nia emit --backend <file.nia> [--runtime bare|freestanding] [--opt-report]
nia emit --llvm <file.nia> [--runtime bare|freestanding] [--opt-report]
nia emit --obj <file.nia> [-o file.o | --out-dir dir] [--runtime bare|freestanding] [--opt-report]
nia emit --exe <file.nia> [-o executable] [--runtime freestanding] [--link-arg arg] [--opt-report]
```

`nia build` is the package-build entry point for `build.nia`. It is owned by the
Nia toolchain rather than a separate bootstrap builder. The current command
resolves package roots, compiles `build.nia` through a toolchain-owned build
runner, and reserves `.nia-build/` for build output and `.nia-cache/` for
reusable package or compiler cache entries. Those directories are created before
the build script runs. `build.nia` is ordinary Nia code and can use the standard
library. The `std::build::Build` value passed to
`build.nia` exposes `packageRoot()`, `buildDir()`, `cacheDir()`, and
`toolchainExecutable()` so build scripts use toolchain-owned paths explicitly.
`hostTarget()` and `artifactTarget()` expose borrowed `TargetView` descriptors
whose text is owned by `Build`, so build scripts can configure declarations
without confusing host services with artifact runtime facts.
`rootPackage()` returns the root package's typed handle.
`addPackage(PackageOptions::init(name, relativeRoot))` declares another local
package below that root, and `CommandArgument::packageInput(handle, path)`
declares a tracked external-tool input from the selected package. Package roots
are canonical relative paths; this build API does not resolve versions, access
a registry, or download packages.
The build API includes `addModule(ModuleOptions::init(name, rootSource))`
for declaring a root source module. Modules inherit the `nia build -O*`
invocation mode, while `withOptimization(mode)` is an explicit per-module
override. `addExecutable(ExecutableOptions::init(name, rootModule))` declares a
freestanding artifact for the invocation's artifact target;
`withOutputName(name)` customizes its output name. `addCheckExecutableStep` and
`addEmitExecutableStep` add compiler-backed graph steps. Those steps use
typed Driver requests and emit executable artifacts to
`.nia-build/<output-name-or-target-name>` without hand-written subprocess setup.
`addAggregateStep(name)` groups dependencies without executing work.
`addGeneratedFileStep(name, BuildPathView::init(path), contents)` atomically
publishes bytes under `.nia-build/`; `ModuleOptions::fromBuild` consumes such a
build-rooted source without aliasing it as a package path.
`addRunExecutableStep(name, RunOptions::init(executable))` runs a declared
artifact and automatically depends on its existing emit step;
`RunOptions::withArguments` supplies retained arguments. Builder calls copy
retained text, paths, imports, and run arguments, so local input arrays may
leave scope before execution. `setDefaultStep(step)`
selects the step used by `nia build` when no step name is passed; otherwise
users must request a named step explicitly.

`addExternalCommandStep(name, ExternalCommandOptions::search(program))`
declares a searched external tool. External commands default to the
`ActionResourceClass::Conservative` resource class, which runs without another
same-wave action because undeclared tool resource use is not assumed safe.
Tools with a known profile can opt into `ActionResourceClass::Cpu` or
`ActionResourceClass::Io` through `withResourceClass`. `nia build -j N` and
`--jobs N` limit ready build actions without increasing the inherited process
capacity or replacing compiler and LLVM resource accounting.

The runner only configures and encodes the immutable plan. The Rust coordinator
validates the plan before executing its selected dependency closure; no callback
or raw compiler command bridge remains. This is still an experimental API, not
a compatibility promise, and the build-host standard-library surface remains
under API and layering review.

The bounded Phase A build baseline can be run explicitly after building a
release compiler:

```sh
python3 tools/build_baseline.py
```

By default it runs clean, warm, source-edit, module-map-edit, and failed-action
states sequentially in each of three fresh isolated workspaces. It refuses to
start under memory pressure, enforces subprocess timeouts, and writes
machine-readable runner compile/run/plan timings, cache acceptance checks, and
median/p95 wall/RSS evidence under `target/nia-build-baseline/`.

Module aliases can be supplied with `-M name=path` or
`--module name=path`. Optimization options and `--timings[=summary|detail]`
are global options and may appear before or after the command.

## Examples

Check every top-level example from the repository root with:

```sh
for file in examples/*.nia; do cargo run -p nia-cli -- --resource-root lib check "$file" --runtime freestanding; done
cargo run -p nia-cli -- --resource-root lib check examples/modules/main.nia --runtime freestanding
```

See [examples/README.md](examples/README.md) for the reading order. The example
source files are the main tutorial material and include inline comments for Nia
syntax and standard-library idioms. They cover real Nia executables, arrays and
slices, structs and enums, control flow, standard-library I/O, collections,
generics, traits, error handling, and multi-file imports. They use the current
executable entry contract:
`pub fn main(process::Init) process::ExitCode!void`. They print visible results
with `std::debug::print`, and `03_stdout.nia` shows explicit stdout output through
`std::io` and `std::fmt`.

## Documentation

- [docs/language-spec.md](docs/language-spec.md): the Nia language
  specification.
- [docs/nia-abi.md](docs/nia-abi.md): ABI and layout rules.
- [docs/architecture.md](docs/architecture.md): compiler architecture and phase
  boundaries.
- [docs/codebase-index.md](docs/codebase-index.md): maintainer index for crate
  ownership, build/std entry points, tests, and common change paths.
- [docs/build-system.md](docs/build-system.md): build ownership, plan/action
  contracts, current-case migration, and resource discipline.
- [docs/standard-library.md](docs/standard-library.md): std layering,
  build-host dependency audit, and API maturity rules.
- [docs/compiler-maintenance.md](docs/compiler-maintenance.md): compiler change
  discipline, acceptance rules, and roadmap-retirement policy.
- [build-std-roadmap.md](build-std-roadmap.md): active build-system,
  toolchain-layout, and standard-library architecture roadmap.
- [docs/platform-support.md](docs/platform-support.md): current platform
  support status.
- [docs/project-conventions.md](docs/project-conventions.md): maintenance rules
  for this repository.
- [docs/contributing.md](docs/contributing.md): contribution expectations.
- [docs/performance.md](docs/performance.md): reproducible compiler workloads
  and machine-readable performance baselines.
- [docs/ai-usage.md](docs/ai-usage.md): AI-assisted work policy.

Documentation file names are not versioned. Release history and versioned
language states are tracked through Git tags.

## Repository Layout

- [crates/nia-cli](crates/nia-cli): the `nia` command-line compiler frontend.
- `crates/nia-*`: compiler libraries used by `nia`.
- [docs/](docs/): language, ABI, architecture, platform, and maintenance docs.
- [examples/](examples/): small executable programs for the current language
  and standard-library surface.
- [benchmarks/](benchmarks/): fixed compiler performance workloads.
- [tools/perf.py](tools/perf.py): release compiler baseline runner; use
  [tools/perf_compare.py](tools/perf_compare.py) for resource-aware comparisons.
- [.github/workflows/performance.yml](.github/workflows/performance.yml): managed
  LLVM performance guard and main-branch baseline artifact retention.

## Platform Status

Nia is currently maintainer-tested rather than platform-supported. It is
developed and tested primarily on the maintainer's local Linux environment.
Broader host and target platform support is planned, but not guaranteed yet.

See [docs/platform-support.md](docs/platform-support.md).

Optimization levels are `-O0`, `-O1`, `-O2`, `-O3`, `-Os`, and `-Oz`; `-O`
means `-O2`. `nia check <file.nia> --opt-report` prints the active
optimization policy and backend optimization report to stdout. `nia check
<file.nia> --runtime freestanding` checks with the same startup runtime that
`emit --exe` injects.
`emit --obj` defaults to the bare runtime and can opt into startup injection
with `--runtime freestanding`. Emit commands write the same report to stderr
when `--opt-report` is supplied.

## Testing

Before opening or merging compiler changes, run the local release gate:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test
```

Compiler and CLI integration tests share an automatic resource budget and
remove their scoped scratch trees when each test ends. The test profile uses
light optimization because these tests execute the compiler as the program
under test; line-table debug information keeps artifacts inspectable without
the full-debug target growth. Environment variables used by the project are cataloged in
[docs/contributing.md](docs/contributing.md).

Do not add lint suppressions just to pass Clippy. Fix the code instead, or
document and review a narrow exception.

For a release point, also confirm the CLI version:

```sh
cargo run -p nia-cli -- --version
```

## License

The `nia` compiler implementation in this repository is licensed under
`GPL-3.0-or-later`. See [LICENSE.md](LICENSE.md) for the exact repository
license scope.
