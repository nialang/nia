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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;

    let mut point = Point { x: 3, y: 4 };
    let mut also_point = Point { x: 5, y: 12 };
    let mut values = [point.len2(), also_point.len2(), 7];
    let mut slice_view = &([1, 2, 3])[..];

    if point.len2() + sum(&values) + sum(slice_view) != 232 {
        return process::exit(1)!;
    }

    !()
}
```

Nominal aggregate literals name their type, as in `Point { ... }`. Array
literals instead infer their element type from an expected type or from their
elements; a suffix such as `[1i64, 2, 3]` supplies an explicit constraint when
the literal stands on its own.
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

Development follows the newest Rust stable release and the default LLVM release
in the newest stable Fedora environment; these are moving maintainer baselines,
not minimum-version promises. Update Rust stable before running the repository's
strict validation commands. Hosted Ubuntu workflows install the matching LLVM
release explicitly.

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

`cargo install` installs only the compiler binary, so it is not a complete Nia
installation. An installed toolchain resolves resources as `../lib/nia`
relative to `bin/nia`; that tree must contain `toolchain.meta`, `std/pkg.nia`, and
the std/runtime sources. The repository does not provide packaging automation
for that installed layout.

The compiler binary is named `nia`. Its core pipeline commands are:

```text
nia build [step|dir] [--root dir]
nia check <file.nia> [--runtime bare|freestanding] [--opt-report]
nia emit --tokens <file.nia>
nia emit --ast <file.nia>
nia emit --checked <file.nia> [--runtime bare|freestanding] [--opt-report]
nia emit --backend <file.nia> [--runtime bare|freestanding] [--opt-report]
nia emit --llvm <file.nia> [--runtime bare|freestanding] [--opt-report]
nia emit --obj <file.nia> [-o file.o | --out-dir dir] [--runtime bare|freestanding] [--opt-report]
nia emit --exe <file.nia> [-o executable] [--runtime freestanding] [--link-arg arg] [--opt-report]
```

`nia check` and `nia emit` also accept a package directory. They select
`main.nia` when present and otherwise use `pkg.nia` for a library-only package.

`nia build` discovers and runs the package's `build.nia`, creates `.nia-build/`
for outputs and `.nia-cache/` for reusable entries, and executes the selected or
default step. A directory argument such as `nia build .` is shorthand for
`--root .`; package discovery stops at a `pkg.nia` boundary, so a child package
cannot silently inherit a parent package's build script. For example:

```sh
nia --resource-root "$PWD/lib" build --root .
nia --resource-root "$PWD/lib" build .
```

The build-script API is documented beside [`lib/std/build.nia`](lib/std/build.nia).
Rust-side plan validation, scheduling, caching, publication, and maintained
build workloads are documented in
[`crates/nia-build/README.md`](crates/nia-build/README.md).

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
`pub fn main(process::Init) process::ExitCode!()`. They print visible results
with the fallible `std::debug::print(...).?` boundary, and
`03_stdout.nia` shows explicit stdout output through `std::io` and `std::fmt`.

## Documentation

- [docs/language-spec.md](docs/language-spec.md): the Nia language
  specification.
- [docs/nia-abi.md](docs/nia-abi.md): ABI and layout rules.
- [docs/architecture.md](docs/architecture.md): compiler architecture and phase
  boundaries.
- [docs/codebase-index.md](docs/codebase-index.md): maintainer index for crate
  ownership, build/std entry points, tests, and common change paths.
- [crates/nia-build/README.md](crates/nia-build/README.md): build invocation,
  plan, coordinator, cache, output-publication, and test ownership.
- [lib/README.md](lib/README.md): standard-library facade, ownership, error,
  callback, and conformance boundaries.
- [docs/compiler-maintenance.md](docs/compiler-maintenance.md): compiler change
  discipline, acceptance rules, and roadmap-retirement policy.
- [docs/const-evaluation-roadmap.md](docs/const-evaluation-roadmap.md): planned
  const-evaluation extensions and their compiler ownership boundaries.
- [docs/platform-support.md](docs/platform-support.md): current platform
  support status.
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
- [maintain/](maintain/): repository maintenance subsystem for audits, crate
  reports, fixtures, and compiler/build baselines.
- [.github/workflows/build-std.yml](.github/workflows/build-std.yml): managed
  build/std correctness matrix for clean/warm/edit/corruption/failure builds,
  relocation, installed artifact execution, and workspace validation.
- [.github/workflows/performance.yml](.github/workflows/performance.yml): managed
  LLVM performance guard and main-branch baseline artifact retention.

## Platform Status

Nia is maintainer-tested rather than released with formal platform support
tiers. The maintainer's Fedora Linux x86_64 environment and managed Ubuntu 24.04
x86_64 workflows are evidence for those configurations, not a general host or
target compatibility promise.

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
