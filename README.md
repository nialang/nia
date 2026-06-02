# Nia

**A tiny language that can actually work.**

Nia is a small systems programming language and `nia` is its compiler. It is
pre-1.0 and under active design, with an implementation that favors clear
semantics, predictable compilation phases, and a compact language surface.

The project is intentionally narrow: this repository contains the compiler,
language documentation, and runnable examples. The standard library, package
manager, and build system are expected to live as separate projects.

## A Small Example

```nia
extern fn printf(fmt: &const u8, ...);

struct Point {
    x: i32,
    y: i32,
}

extend Point {
    fn len2(&const self) i32 {
        self.x * self.x + self.y * self.y
    }
}

fn sum(xs: &const [i32]) i32 {
    var total = 0;
    for i in 0usize..xs.len() {
        total = total + xs[i];
    }
    total
}

fn main() i32 {
    var point: Point = { x: 3, y: 4 };
    var also_point = Point { x: 5, y: 12 };
    var values = [_]i32[point.len2(), also_point.len2(), 7];
    var borrowed = &const ([3]i32[1, 2, 3])[..];

    printf(c"point=%d values=%d borrowed=%d\n",
        point.len2(),
        sum(&const values[..]),
        sum(borrowed),
    );
    0
}
```

Most aggregate literals use the expected type from a binding, argument, or
return position. Explicit forms such as `Point { ... }` and `[_]i32[...]` are
available when the literal needs to stand on its own.

## Design Goals

- Keep the language small enough to understand as a whole.
- Provide the low-level control expected from a systems language.
- Make host and bare-metal use cases explicit instead of hiding startup and
  linking behavior behind one model.
- Keep the compiler modular so syntax, semantic checks, lowering, and codegen
  remain easy to inspect and evolve.

## Quick Start

Nia is a Rust workspace. Build the compiler with:

```sh
cargo build --workspace
```

Run the compiler from the workspace with:

```sh
cargo run -p nia-cli -- --help
```

The compiler binary is named `nia`. Its core pipeline commands are:

```text
nia lex <file.nia>
nia parse <file.nia>
nia check <file.nia> [--opt-report]
nia emit backend <file.nia> [--opt-report]
nia emit llvm <file.nia> [--opt-report]
nia emit obj <file.nia> [-o file.o | --out-dir dir] [--opt-report]
nia emit exe <file.nia> [-o executable] [--opt-report]
```

Module aliases can be supplied with `-M name=path`.

## Examples

Check every top-level example from the repository root with:

```sh
for file in examples/*.nia; do cargo run -p nia-cli -- check "$file"; done
cargo run -p nia-cli -- check examples/modules/main.nia
```

See [examples/README.md](examples/README.md) for the reading order. The examples
cover hosted programs, arrays and slices, structs and enums, control flow,
functions, generics, traits, trait objects, comptime, inline assembly, and
multi-file modules.

## Documentation

- [docs/language-spec.md](docs/language-spec.md): the Nia language
  specification.
- [docs/nia-abi.md](docs/nia-abi.md): ABI and layout rules.
- [docs/architecture.md](docs/architecture.md): compiler architecture and phase
  boundaries.
- [docs/platform-support.md](docs/platform-support.md): current platform
  support status.
- [docs/project-conventions.md](docs/project-conventions.md): maintenance rules
  for this repository.
- [docs/contributing.md](docs/contributing.md): contribution expectations.
- [docs/ai-usage.md](docs/ai-usage.md): AI-assisted work policy.

Documentation file names are not versioned. Release history and versioned
language states are tracked through Git tags.

## Repository Layout

- [crates/nia-cli](crates/nia-cli): the `nia` command-line compiler frontend.
- `crates/nia-*`: compiler libraries used by `nia`.
- [docs/](docs/): language, ABI, architecture, platform, and maintenance docs.
- [examples/](examples/): runnable teaching programs for the current language
  surface.

## Platform Status

Nia is currently maintainer-tested rather than platform-supported. It is
developed and tested primarily on the maintainer's local Linux environment.
Broader host and target platform support is planned, but not guaranteed yet.

See [docs/platform-support.md](docs/platform-support.md).

Optimization levels are `-O0`, `-O1`, `-O2`, `-O3`, `-Os`, and `-Oz`; `-O`
means `-O2`. `nia check <file.nia> --opt-report` prints the active
optimization policy to stdout. Emit commands write the same report to stderr
when `--opt-report` is supplied.

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

The `nia` compiler implementation in this repository is licensed under
`GPL-3.0-or-later`. See [LICENSE.md](LICENSE.md) for the exact repository
license scope.
