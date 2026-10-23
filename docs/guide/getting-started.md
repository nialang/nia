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

A package is a directory with a `build.nia` script and, usually, a `nia.toml`
manifest that names it and declares its dependencies. A typical layout:

```text
hello/
├── nia.toml
├── build.nia
├── src/
│   └── main.nia
└── tests/
    └── main.nia
```

`nia.toml` names the package:

```toml
[package]
name = "hello"
version = "0.1.0"
```

Dependencies go in `[dependencies]`, for example
`json = { git = "https://example.com/json.git", tag = "v1.0.0" }`; see
[Package manifest](build-scripts.md#package-manifest).

`src/main.nia` holds the hello-world program above. `build.nia` declares what
the package builds:

```nia
using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {
    let app = b.addModule(
        build::ModuleOptions::init(&"hello", fs::PathView::init(&"src/main.nia")),
    ).?;
    let exe = b.addExecutable(build::ExecutableOptions::init(&"hello", app)).?;
    let emit = b.addEmitExecutableStep(&"build", exe).?;
    _ = b.addRunExecutableStep(&"run", build::RunOptions::init(exe)).?;
    b.setDefaultStep(emit).?;

    let tests = b.addModule(
        build::ModuleOptions::init(&"tests", fs::PathView::init(&"tests/main.nia")),
    ).?;
    _ = b.addTestSuite(&"unit", tests).?;
    !()
}
```

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
| `nia build [step]` | Runs `build.nia`, then the named step, or the default step when none is named. |
| `nia test` | Builds and runs every registered test suite. |
| `nia update [name ...]` | Refreshes Git revisions and registry versions in `nia.lock`. |
| `nia check <path>` | Type-checks a source file or package directory without producing output. |
| `nia emit --<kind> <path>` | Prints or writes one compiler output. |
| `nia help [command]` | Shows a command's options; `nia help emit --exe` shows the options of one emit kind. |

From anywhere inside the package:

```sh
nia build            # default step: writes .nia-build/hello
nia build run        # builds, then runs the executable
nia build --dry-run  # configures and validates the plan without running it
nia test             # builds and runs the "unit" suite
nia test --list      # lists suites without running them
```

`nia build` and `nia test` search upward from the current directory for
`build.nia`; `--root <dir>` starts the search elsewhere. Outputs are written
under `.nia-build/` in the package root, and reusable results are kept in
`.nia-cache/`. A test suite passes
when its executable exits with status 0; a suite that passed is not rerun until
its inputs change. `--filter <text>` selects suites by name and `--fail-fast`
stops after the first failure.

Both commands report progress on stderr: a status line and a final summary by
default, `-q` to silence it, and `-v` through `-vvv` for more detail. See
[Progress output](build-scripts.md#progress-output).

`nia check` and `nia emit` take a source file or a directory. A directory must
contain `main.nia` (an executable entry) or `pkg.nia` (a library package):

```sh
nia check src --runtime freestanding
nia emit --llvm src/main.nia
nia emit --exe src/main.nia -o hello
```

`emit` accepts `--tokens`, `--ast`, `--checked`, `--backend`, and `--llvm` to
print intermediate forms, `--obj` to write object files, and `--exe` to write
an executable. `-M <name>=<path>` maps an extra package name to a source file
or directory, so `using name;` can find it; a build script declares its
packages instead, so `nia build` and `nia test` do not accept `-M`.

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
