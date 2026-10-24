# Getting Started

This guide installs the `nia` toolchain, runs a first program, and sets up a
package with a build script and tests.

## Install

### Release archive

Archives for the supported compiler hosts are published on the
[GitHub Releases](https://github.com/nialang/nia/releases) page:

| Host | Archive |
| --- | --- |
| Linux x86_64 | `nia-<version>-linux-x86_64.tar.gz` |
| Linux AArch64 | `nia-<version>-linux-aarch64.tar.gz` |
| Windows x86_64 | `nia-<version>-windows-x86_64.zip` |
| macOS Intel | `nia-<version>-darwin-x86_64.tar.gz` |
| macOS Apple silicon | `nia-<version>-darwin-aarch64.tar.gz` |

Unpack an archive as a complete prefix and keep its `bin`, `lib`, and `libexec`
directories together:

```sh
tar -xzf nia-<version>-linux-x86_64.tar.gz
./nia-<version>-linux-x86_64/bin/nia --version
```

```text
nia-<version>-linux-x86_64/
├── bin/nia
├── lib/toolchain.meta
├── lib/std/
└── libexec/ld.lld
```

Each archive contains the compiler, standard library, and host LLD linker. No
system LLVM installation is needed. Add `bin` to `PATH` to run `nia` directly.

Windows archives contain `bin/nia.exe` and `libexec/ld.lld.exe`; Unix archives
contain `bin/nia` and `libexec/ld.lld`. The bundled linker selects the target
flavor automatically, so a host archive can generate any supported target
listed in [Platform support](../reference/platform-support.md#targets).

### Build from source

Source builds are useful for development. With Rust stable, LLVM 23.1
(`llvm-config` on `PATH` or `LLVM_SYS_231_PREFIX` set), and `ld.lld` available
on `PATH` or through `NIA_LLD`:

```sh
cargo build --release -p nia-cli
target/release/nia --resource-root "$PWD/lib" --version
```

A compiler built from source finds its standard library through
`--resource-root`; point it at the checkout's `lib` directory. The examples
below omit the option for brevity. The `tools/nia-dev` launcher in the checkout
runs `target/release/nia` with the checkout's `lib` resources already
selected.

The full toolchain setup, including Windows MSVC builds and troubleshooting, is
in [Development setup](../dev/setup.md).

## Hello, world

Save this as `hello.nia`:

```nia
using std::io;
using std::process;
using process::{Init, ExitCode};

pub fun main(init: Init) ExitCode!() {
    _ = init;
    io::debugPrint(&"hello from nia\n", &[]).?;
    !()
}
```

`main` receives the process context (`init`, which carries arguments and the
environment) and returns `ExitCode!()`: `!()` means success, and an error value
such as `ExitCode(1)!` exits with that status. `.?` propagates a failure from
`debugPrint` out of `main`.

Check the program, then build and run it:

```sh
nia check hello.nia --runtime freestanding
nia emit --exe hello.nia -o hello
./hello
```

```text
hello from nia
```

`nia check` validates a program without producing output. Its default runtime
is `bare`, so pass `--runtime freestanding` to check a program against the
standard `main` contract. `nia emit --exe` always builds a freestanding
executable.

## A first package

A package is a directory with a `nia.toml` manifest that names it, declares
its dependencies, and lists what it builds. A typical layout:

```text
hello/
├── nia.toml
├── src/
│   └── main.nia
└── tests/
    └── main.nia
```

`nia.toml` names the package and declares an executable and a test suite:

```toml
[package]
name = "hello"
version = "0.1.0"

[[executable]]
name = "hello"
path = "src/main.nia"

[[test]]
name = "unit"
path = "tests/main.nia"
```

Dependencies go in `[dependencies]`, for example
`json = { git = "https://example.com/json.git", tag = "v1.0.0" }`; see
[Package manifest](build-scripts.md#package-manifest).

`src/main.nia` holds the hello-world program above. Each entry becomes a step
named after it, an executable also gets a `run-<name>` step, and `all`, the
default, builds every executable and runs every suite. A package that needs
generated files, commands, probes, or custom steps adds a `build.nia`; see
[Build scripts](build-scripts.md).

`tests/main.nia` is an ordinary program that runs test cases with `std::test`
and exits with a failure status when one fails:

```nia
using std::process;
using std::test;

fun addsNumbers() test::Error!() {
    test::expectEqual(2 + 2, 4).?;
    !()
}

fun measuresText() test::Error!() {
    test::expect("nia".len() == 3).?;
    !()
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let cases: [test::Case; 2] = [
        .init(&"adds numbers", &addsNumbers),
        .init(&"measures text", &measuresText),
    ];
    let summary = test::Runner::init(&cases).run();
    if not summary.isSuccessful() {
        return process::ExitCode(1)!;
    }
    !()
}
```

## Commands

| Command | What it does |
| --- | --- |
| `nia build [step]` | Runs the named step, or the default step when none is named. |
| `nia update [name ...]` | Refreshes Git revisions and registry versions in `nia.lock`. |
| `nia check <path>` | Type-checks a source file or package directory without producing output. |
| `nia emit --<kind> <path>` | Prints or writes one compiler output. |
| `nia help [command]` | Shows a command's options; `nia help emit --exe` shows the options of one emit kind. |

From anywhere inside the package:

```sh
nia build            # default step `all`: builds hello and runs the suites
nia build run-hello  # builds, then runs the executable
nia build --dry-run  # configures and validates the plan without running it
nia build test       # builds and runs every test suite
nia build --list     # lists the steps without running them
```

`nia build` searches upward from the current directory for the package's
`nia.toml` or `build.nia`; `--root <dir>` starts the search elsewhere. Outputs are written
under `.nia-build/` in the package root, and reusable results are kept in
`.nia-cache/`. A test suite passes
when its executable exits with status 0; a suite that passed is not rerun until
its inputs change. `nia build <suite>` runs one suite, and `--keep-going` runs
the remaining suites after one fails.

The build reports progress on stderr: a status line and a final summary by
default, `-q` to silence it, and `-v` through `-vvv` for more detail. See
[Progress output](build-scripts.md#progress-output).

`nia check` and `nia emit` take a source file or a directory. A directory names
its `main.nia`, or, for a library package, the `[library].path` of its
`nia.toml`. Either file roots a module tree, and its child modules sit beside
it:

```sh
nia check src --runtime freestanding
nia emit --llvm src/main.nia
nia emit --exe src/main.nia -o hello
```

`emit` accepts `--tokens`, `--ast`, `--checked`, `--backend`, and `--llvm` to
print intermediate forms, `--obj` to write object files, and `--exe` to write
an executable. `-M <name>=<path>` maps an extra package name to a source file
or to a package directory's `[library].path`, so `using name;` can find it; a
build script declares its
packages instead, so `nia build` does not accept `-M`.

Global options apply to every command and may come before or after it:
`-O0` through `-O3`, `-Os`, `-Oz` (`-O` means `-O2`), `--debug` / `--release`,
and `--target <name>` to build for another
[supported target](../reference/platform-support.md#targets).

Every option that takes a value accepts both `--option value` and
`--option=value`, and the next argument is always the value, even when it
starts with `-`. When an option is repeated, the last value wins; `-M`, `-L`,
`-l`, `--link-arg`, and `--rpath` accumulate instead. `--` ends the options, so
later arguments are taken literally. A malformed command line exits with
status 2; a failed check, build, or test exits with status 1.

## Runtimes

The runtime decides who owns process entry. `freestanding` links Nia's startup
code, which builds `process::Init` and calls `pub fun main(init) ExitCode!()`;
it uses only the operating system's stable interface and no C library. `bare`
links no startup and assumes no operating system, for kernels, firmware, and
other images that define their own entry. Executables in a build script are
freestanding unless they select `build::Runtime::Bare`. The per-platform
details are in [Platform support](../reference/platform-support.md#runtime-model).

## Next steps

- [Language tour](tour.md): the language from simple values to generics and
  modules.
- [Build scripts](build-scripts.md): everything `build.nia` can declare.
- [Examples](../../examples/README.md): complete programs to read and run.
- [Standard library](../../lib/README.md): library modules and conventions.
- [C interop](c-interop.md): calling C and linking native libraries.
- [Language reference](../reference/language.md): the complete rules.
