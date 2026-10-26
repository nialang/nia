<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/brand/nia-lockup-dark.svg">
    <img src="assets/brand/nia-lockup.svg" alt="Nia" width="190">
  </picture>
</h1>

Nia is a small systems programming language with an LLVM-backed compiler and a
compact standard library. The language is statically typed, expression-oriented,
and explicit about data representation, resource ownership, modules, and
runtime entry points.

This repository contains the compiler, the standard library, the package build
system, documentation, and examples. Nia is pre-1.0: the language and toolchain
evolve together, and breaking changes land without compatibility shims.

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
    fun lengthSquared(&self) i32 {
        self.x * self.x + self.y * self.y
    }
}

fun sum(values: &[i32]) i32 {
    let mut total = 0;
    for &value in values {
        total += value;
    }
    total
}

pub fun main(init: Init) ExitCode!() {
    _ = init;

    let point = Point { x: 3, y: 4 };
    let values = [point.lengthSquared(), 7, 13];

    if sum(&values) != 45 {
        return process::ExitCode(1)!;
    }

    io::debugPrint(&"Nia is running\n", &[]).?;
    !()
}
```

This small program demonstrates nominal structs, extension methods, slices,
arrays, loops, fallible calls, and the standard freestanding entry contract.
The [language tour](docs/guide/tour.md) and the [examples](examples/README.md)
develop these features through complete programs.

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

## Get Started

Release archives for Linux x86_64 and AArch64, Windows x86_64, macOS Intel, and
macOS Apple silicon are published on the
[GitHub Releases](https://github.com/nialang/nia/releases) page. Unpack an
archive as a complete prefix and keep its `bin`, `lib`, and `libexec`
directories together; it bundles the standard library and LLD, so no system
LLVM installation is needed. The archive name identifies the compiler host,
while `--target` selects any supported artifact target.

```sh
tar -xzf nia-<version>-linux-x86_64.tar.gz
./nia-<version>-linux-x86_64/bin/nia --version
```

The published archive is the normal installation path. A compiler can also be
built from source; [Development setup](docs/dev/setup.md) covers the required
Rust, LLVM, and LLD inputs:

```sh
cargo build --release -p nia-cli
target/release/nia --resource-root "$PWD/lib" check examples/hello.nia --runtime freestanding
```

[Getting started](docs/guide/getting-started.md) walks through a first package
and the `nia build`, `nia check`, and `nia emit` commands. Run
`nia help <command>` for a command's options, or `nia help emit --exe` for one
emit kind.

## Documentation

The [documentation index](docs/README.md) lists everything by reader. The main
entry points are:

- [Language tour](docs/guide/tour.md) and [language reference](docs/reference/language.md)
- [Build scripts](docs/guide/build-scripts.md) and [C interop](docs/guide/c-interop.md)
- [Standard library](lib/README.md) and [ABI reference](docs/reference/abi.md)
- [Platform support](docs/reference/platform-support.md)
- [Contributing](docs/dev/contributing.md) and [compiler architecture](docs/dev/architecture.md)

## Repository Layout

- `crates/nia-cli/`: the `nia` command-line frontend.
- `crates/nia-*/`: compiler, query, build-system, backend, and support libraries.
- `lib/`: the standard library and runtime resources.
- `docs/`: user guide, reference, and contributor documentation.
- `examples/`: complete Nia programs arranged as reading material.
- `benchmarks/`: performance workloads.
- `maintain/`: repository audits, fixtures, and performance infrastructure.
- `tools/`: the development launcher, LLVM toolchain, and release scripts.

## Support And Status

Nia is maintainer-tested. Release archives cover Linux x86_64 and AArch64,
Windows x86_64, macOS Intel, and macOS Apple silicon. The compiler also
generates executables for the Linux, Windows, and macOS targets listed in the
platform reference.
[Platform support](docs/reference/platform-support.md) describes the exact
boundary.

## License

The compiler implementation is licensed under `GPL-3.0-or-later`. Repository
documentation has the separate scope described in [LICENSE.md](LICENSE.md).
