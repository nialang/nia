# Nia

Nia is a small systems programming language with an LLVM-backed compiler and a
compact standard library. The language is statically typed, expression-oriented,
and explicit about data representation, resource ownership, modules, and
runtime entry points.

The repository contains the compiler, standard library, build system, language
documentation, examples, and their test suites. The `0.1.x` line is Nia's first
maintained release series; the language and toolchain continue to evolve
together, with compatibility changes called out in release notes.

## Example

```nia
using std::io;
using std::process;
using process::{Init, ExitCode};

struct Point {
    x: i32,
    y: i32,
}

extend Point {
    fn lengthSquared(&self) i32 {
        self.x * self.x + self.y * self.y
    }
}

fn sum(values: &[i32]) i32 {
    let mut total = 0;
    for &value in values {
        total += value;
    }
    total
}

pub fn main(init: Init) ExitCode!() {
    _ = init;

    let point = Point { x: 3, y: 4 };
    let values = [point.lengthSquared(), 7, 13];

    if sum(&values) != 45 {
        return process::exit(1)!;
    }

    io::debugPrint(&"Nia is running\n", &[]).?;
    !()
}
```

This small program demonstrates nominal structs, extension methods, slices,
arrays, loops, fallible calls, and the standard freestanding entry contract.
The [examples](examples/README.md) directory develops these features through
complete programs.

## Language At A Glance

Nia provides:

- integers, booleans, arrays, pointers, slices, structs, unions, enums, and
  function pointers;
- generic functions, structs, methods, and traits;
- `extend` blocks for methods and trait implementations;
- expression-oriented blocks, `if`, `match`, loops, and `defer`;
- optional and error-union values with pattern matching and propagation;
- explicit modules, visibility, and package roots;
- compile-time values, C ABI declarations, and LLVM code generation.

Allocation and resource lifetime are ordinary library operations. The toolchain
owns startup, linking, package builds, and runtime selection.

## Build The Compiler

The compiler is a Rust workspace and currently uses LLVM 22.1 through
`llvm-sys`. Install Rust stable, LLVM 22.1, and make `llvm-config` available on
`PATH`, then run:

```sh
cargo build --workspace
```

For an LLVM installation outside the standard layout, set its prefix while
building:

```sh
LLVM_SYS_221_PREFIX=/path/to/llvm-22.1 cargo build --workspace
```

Run the compiler directly from the checkout with the versioned standard-library
resources:

```sh
cargo run -p nia-cli -- --resource-root lib check examples/hello.nia \
  --runtime freestanding
```

For repeated use, build the release binary and keep the checkout's `lib`
directory beside it:

```sh
cargo build --release -p nia-cli
target/release/nia --resource-root "$PWD/lib" --version
```

To use the latest compiler built from this checkout without repeating the
resource-root option, run the development launcher:

```sh
cargo build --release -p nia-cli
tools/nia-dev check examples/hello.nia --runtime freestanding
```

The launcher always pairs `target/release/nia` with this checkout's `lib`
resources, so it is suitable for testing changes before the next release.

## Install A Release

Linux x86_64 release archives are published on the
[GitHub Releases](https://github.com/nialang/nia/releases) page. Unpack an
archive as a complete prefix and keep its `bin`, `lib`, and `libexec`
directories together:

```sh
tar -xzf nia-<version>-linux-x86_64.tar.gz
./nia-<version>-linux-x86_64/bin/nia --version
```

The archive uses a portable-prefix layout with no second project directory
inside the prefix:

```text
nia-<version>-linux-x86_64/
├── bin/nia
├── lib/toolchain.meta
├── lib/std/
└── libexec/ld.lld
```

The archive includes the compiler, standard-library resources, and bundled
`ld.lld`; no system LLVM installation is needed to run the packaged compiler.

## Command Workflows

The executable is named `nia`. Use `nia help` or `nia help <command>` for the
complete option set.

```text
nia build [step] [--root <dir>]
nia test [--root <dir>] [--filter <text>] [--list] [--fail-fast]
nia check <file-or-package> [--runtime bare|freestanding]
nia emit --<target> <file-or-package>
```

`build` discovers a package's `build.nia`, writes build outputs under
`.nia-build/`, and reuses entries from `.nia-cache/`. A package can be checked
or emitted from a source file or directory; `main.nia` is the executable entry
when present, while `pkg.nia` describes a library package. Use `--root` when
package discovery should start from a different directory.

`test` builds and runs the package's registered test suites. Use `--list` to
inspect the suites first, `--filter` to select matching suites, and
`--fail-fast` to stop after the first failure.

`check` performs validation only. The `--runtime` option selects the
source/runtime boundary used during checking. `emit` can
print tokens, AST, checked data, backend IR, or LLVM IR, and can write native
objects or freestanding executables:

```sh
nia --resource-root "$PWD/lib" check examples/hello.nia \
  --runtime freestanding
nia --resource-root "$PWD/lib" emit --llvm examples/hello.nia
nia --resource-root "$PWD/lib" emit --exe examples/hello.nia -o build/hello
```

Global options include optimization levels (`-O0` through `-Oz`), build
profiles (`--debug`, `--release`, and `--profile`), module aliases, timings, and
the resource root. The CLI help is the authoritative syntax reference.

## Documentation

Language and library users should start here:

- [Language specification](docs/language-spec.md): syntax and semantics.
- [Examples](examples/README.md): a guided sequence of complete programs.
- [Standard library](lib/README.md): library modules and runtime-facing APIs.
- [Build scripts](lib/std/build.nia): the package build-script API.
- [ABI reference](docs/nia-abi.md): representation, layout, and calling rules.

Compiler and repository contributors should use:

- [Compiler architecture](docs/architecture.md): phases, products, and crate
  boundaries.
- [Build system](crates/nia-build/README.md): package build plans, execution,
  caching, and publication.
- [Contributing](docs/contributing.md): change routing and review workflow.
- [Compiler maintenance](docs/compiler-maintenance.md): cross-cutting
  correctness and ownership principles.
- [Platform support](docs/platform-support.md): maintained hosts, targets, and
  toolchain policy.
- [Performance maintenance](maintain/performance.md): reproducible workloads
  and baseline comparisons.

## Repository Layout

- `crates/nia-cli/`: the `nia` command-line frontend.
- `crates/nia-*/`: compiler, query, backend, and support libraries.
- `lib/`: the standard library and runtime resources.
- `docs/`: language, ABI, architecture, platform, and project documentation.
- `examples/`: complete Nia programs arranged as reading material.
- `maintain/`: repository audits, fixtures, and performance infrastructure.
- `.github/workflows/`: managed compiler, standard-library, and performance
  validation.

## Support And Status

Nia 0.1.x is maintainer-tested. The supported release package targets Linux
x86_64 on Ubuntu 24.04-class systems; the maintained development environments
also include the current Fedora Linux x86_64 setup. Freestanding executable
coverage currently targets Linux x86_64, with experimental i686 coverage.

The detailed support boundary, LLVM requirements, and target notes live in
[Platform Support](docs/platform-support.md).

## License

The compiler implementation is licensed under `GPL-3.0-or-later`. Repository
documentation has the separate scope described in [LICENSE.md](LICENSE.md).
