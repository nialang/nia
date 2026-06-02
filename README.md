# Nia

**A tiny language that can actually work.**

Nia is a small systems programming language and `niac` is its compiler. The
project is intentionally narrow: the main repository contains the compiler
implementation and the language documentation. The standard library, package
manager, and build system are expected to live as separate projects.

Nia is pre-1.0 and under active design. The implementation favors clear
semantics, predictable compilation phases, and a compact language surface over
compatibility with earlier experimental syntax.

## Goals

- Keep the language small enough to understand as a whole.
- Provide the low-level control expected from a systems language.
- Make host and bare-metal use cases explicit instead of hiding startup and
  linking behavior behind one model.
- Keep the compiler modular so syntax, semantic checks, lowering, and codegen
  remain easy to inspect and evolve.

## Repository Layout

- [docs/language-spec.md](docs/language-spec.md): the Nia language
  specification.
- [docs/architecture.md](docs/architecture.md): compiler architecture and phase
  boundaries.
- [docs/project-conventions.md](docs/project-conventions.md): maintenance rules
  for this repository.
- [docs/contributing.md](docs/contributing.md): contribution expectations.
- [docs/ai-usage.md](docs/ai-usage.md): AI-assisted work policy.
- [docs/platform-support.md](docs/platform-support.md): current platform
  support status.
- [crates/nia-cli](crates/nia-cli): the `niac` command-line compiler frontend.
- `crates/nia-*`: compiler libraries used by `niac`.

Documentation file names are not versioned. Release history and versioned
language states are tracked through Git tags.

## Building

Nia is a Rust workspace. Build the compiler with:

```sh
cargo build --workspace
```

Run the compiler from the workspace with:

```sh
cargo run -p nia-cli -- --help
```

The compiler binary is named `niac`.

## Platform Status

Nia is currently maintainer-tested rather than platform-supported. It is
developed and tested primarily on the maintainer's local Linux environment.
Broader host and target platform support is planned, but not guaranteed yet.

See [docs/platform-support.md](docs/platform-support.md).

## Compiler Commands

`niac` supports the core compiler pipeline commands:

```text
niac lex <file.nia>
niac parse <file.nia>
niac check <file.nia> [--emit-opt-report]
niac emit backend <file.nia> [--emit-opt-report]
niac emit llvm <file.nia> [--emit-opt-report]
niac emit obj <file.nia> [-o file.o | --out-dir dir] [--emit-opt-report]
niac emit exe <file.nia> [-o executable] [--emit-opt-report]
```

Module aliases can be supplied with `-M name=path`.
Optimization levels are `-O0`, `-O1`, `-O2`, `-O3`, `-Os`, and `-Oz`; `-O`
means `-O2`. `niac check <file.nia> --emit-opt-report` prints the active
optimization policy to stdout. Emit commands write the same report to stderr
when `--emit-opt-report` is supplied.

## Testing

Before opening or merging compiler changes, run the local release gate:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Do not add lint suppressions just to pass Clippy. Fix the code instead, or
document and review a narrow exception.

For a release point, also confirm the CLI version:

```sh
cargo run -p nia-cli -- --version
```

## License

The `niac` compiler implementation in this repository is licensed under
`GPL-3.0-or-later`. See [LICENSE.md](LICENSE.md) for the exact repository
license scope.
