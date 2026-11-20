<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/brand/nia-lockup-dark.svg">
    <img src="assets/brand/nia-lockup.svg" alt="Nia" width="190">
  </picture>
</h1>

Nia is a small systems programming language with an LLVM-based compiler and a
compact standard library. It is statically typed and expression-oriented, and
it is explicit about data representation, resource ownership, modules, and how
a program starts. Instead of a dedicated feature for each task, Nia provides
a few general building blocks, such as traits, generics, and error unions,
that compose.

This repository contains the compiler, the standard library, the package
build system, the language server, documentation, and examples.

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

    if sum(values) != 45 {
        return process::ExitCode(1)!;
    }

    io::debugPrint("Nia is running\n", []).?;
    !()
}
```

The program declares a struct with a method, passes an array to a function
that takes a slice, and returns an error union from `main`: `!()` is success,
`ExitCode(1)!` a failure, and `.?` passes the failure of a fallible call on to
the caller. The [Learning Nia](docs/learn/README.md) chapters and the
[examples](examples/README.md) develop these features through complete
programs.

## Features

The language:

- signed and unsigned integers from 8 to 128 bits, floating-point numbers,
  `char`, `bool`, tuples, arrays, slices, SIMD vectors, pointers, structs,
  unions, and enums with payloads;
- optionals `?T` and error unions `E!T`, with `.?` propagation, combinators,
  and typed errors that convert between modules;
- generic functions and types over types and constants, traits with
  associated types and default methods, operator traits, and trait objects;
- methods and trait implementations in `extend` blocks, and extension methods
  that each module imports explicitly;
- closures with explicit captures, callable views, and function pointers;
- explicit allocation: allocators are values passed to every call that
  allocates, and `defer` releases resources on every exit;
- compile-time constants, and conditional compilation on target facts and CPU
  features;
- C interop through `extern` declarations that follow the target's C ABI.

The toolchain:

- `nia build`, which builds packages with path, Git, and registry
  dependencies, a lockfile, workspaces, and optional build scripts written in
  Nia;
- cross-compilation from any host to Linux (x86_64, i686, AArch64), Windows
  x64, and macOS (Intel, Apple silicon), with a bundled LLD linker and no
  platform SDK or C runtime;
- `nia check`, `nia fmt`, and `nia-lsp`, a language server for VS Code,
  Neovim, and Vim.

## Get Started

Release archives for Linux x86_64 and AArch64, Windows x86_64, and macOS on
Intel and Apple silicon are published on the
[GitHub Releases](https://github.com/nialang/nia/releases) page. Each archive
contains the compiler, the language server, the standard library, and LLD.
Unpack it and keep its directories together:

```sh
tar -xzf nia-<version>-linux-x86_64.tar.gz
./nia-<version>-linux-x86_64/bin/nia --version
```

[Getting Started](docs/learn/getting-started.md) covers installation,
including building the compiler from source, and walks through a first
program and a first package. `nia help <command>` lists a command's options,
and `nia help emit --exe` those of one output kind.

[Nia Language Server](editors/README.md) describes what `nia-lsp` does and
how to set it up in VS Code, Neovim, and Vim.

## Documentation

The [documentation index](docs/README.md) lists every document. The main
entry points are:

- [Learning Nia](docs/learn/README.md), the chapters that teach the language
  in order, and the [Language Reference](docs/reference/language.md);
- the guides [Packages And Dependencies](docs/guides/packages.md),
  [Custom Builds](docs/guides/build-scripts.md),
  [Targets And Runtimes](docs/guides/targets.md), and
  [C Interop](docs/guides/c-interop.md);
- [Writing Idiomatic Nia](docs/guides/writing-nia.md), and the
  [writing-nia skill](skills/README.md) that gives the same guidance to an AI
  coding assistant;
- the [Standard Library](lib/README.md), the
  [ABI Reference](docs/reference/abi.md), and
  [Platform Support](docs/reference/platform-support.md);
- for contributors, [Contributing](docs/dev/contributing.md) and the
  [compiler architecture](docs/dev/architecture.md).

## Status

Nia is pre-1.0. The `main` branch is the 0.3.0 development line, and
`nia --version` prints `0.3.0-dev` for a build of it. The language and the
toolchain evolve together, and breaking changes land without compatibility
shims.

Nia is maintainer-tested and has no formal support tiers.
[Platform Support](docs/reference/platform-support.md) lists the supported
targets and the configurations that are built and tested.

## Repository Layout

- `crates/nia-cli/`: the `nia` command-line frontend.
- `crates/nia-*/`: the compiler, query, build-system, backend, language
  server, and support libraries.
- `lib/`: the standard library and runtime resources.
- `docs/`: chapters, guides, references, and contributor documentation.
- `examples/`: complete Nia programs to read in order.
- `editors/`: the VS Code, Neovim, and Vim clients of the language server.
- `skills/`: instructions for AI coding assistants that write Nia.
- `benchmarks/`: performance workloads.
- `maintain/`: repository audits, fixtures, and performance infrastructure.
- `formal/`: Lean models of compiler contracts.
- `tools/`: the development launcher, LLVM toolchain, and release scripts.
- `assets/brand/`: the logo and icons.

## License

The compiler implementation is licensed under `GPL-3.0-or-later`. Programs
compiled with `nia` are not covered by that license merely because `nia`
compiled them. The documentation has a separate scope;
[LICENSE.md](LICENSE.md) describes what each license covers.
