# Getting Started

This chapter installs Nia, walks through a first program line by line, shows
how to read the compiler's errors, and sets up a package with an executable, a
library, and tests. It is the first chapter of [Learning Nia](README.md).

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

## Your First Program

Save this as `hello.nia`:

```nia
using std::io;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    io::debugPrint("hello from nia\n", []).?;
    !()
}
```

Build it into an executable and run it:

```sh
nia emit --exe hello.nia -o hello
./hello
```

```text
hello from nia
```

Every line of this program says something about how Nia works:

- `using std::io;` makes the standard library's `io` module available under the
  name `io`. Nothing is imported implicitly: code names every module it uses.
- `pub fun main(...)` declares the program's entry point. It must be public,
  because the toolchain's startup code calls it from outside this module.
- `init: process::Init` is the process context: command-line arguments and
  environment variables. This program does not need it, and Nia does not allow
  a value to be silently ignored, so `_ = init;` discards it on purpose.
- The return type `process::ExitCode!()` is an *error union*: the function
  either succeeds with the unit value `()` or fails with an `ExitCode`. Nia has
  no exceptions; failure is part of the return type.
- `io::debugPrint` writes a formatted message to standard error. Its first
  argument is a template, and `{}` placeholders take their values from the
  array that follows, here empty.
- Writing to a stream can fail, so `debugPrint` returns an error union too.
  The postfix `.?` either unwraps a success or returns the error from `main`
  right away. The standard library's errors convert to an exit status, so a
  failed write ends the program with a failure status.
- `!()` is the success value: `!` wraps a value as the success case of an
  error union, and `()` is the unit value. It is the last expression of the
  block without a semicolon, so it is the function's result.

## Reading Compiler Errors

`nia check` type-checks a program without building it. Remove the `.?` after
`debugPrint` and check again:

```sh
nia check hello.nia --runtime freestanding
```

```text
error[E0301]: non-unit expression result is discarded; assign it to `_` explicitly
  --> hello.nia:6:5
  |
6 |     io::debugPrint("hello from nia\n", []);
  |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^

summary: 1 error, 0 warnings
```

An error names a code, the problem, and the exact source range. Here the call
returns an error union and the statement would drop it, which Nia rejects so
that failures are never lost by accident. Handle it with `.?`, inspect it, or
discard it deliberately with `_ = ...;`.

Type errors also say what was expected and what was found:

```text
error[E0301]: type mismatch in binding initializer: expected i32, got [char; 5]
  --> hello.nia:6:22
  |
6 |     let count: i32 = "three";
  |                      ^^^^^^^
note: this context requires `i32`, but the expression produces `[char; 5]`
```

A string literal is an array of characters, not a pointer, so its type shows
its length. Later chapters return to this.

`--runtime freestanding` tells `nia check` to check `main` against the standard
entry contract. Without it, `nia check` treats `main` as an ordinary function,
which is what kernels and other programs without an operating system need.
`nia emit --exe` always builds the standard entry.

## Your First Package

Real programs are packages. Create one:

```sh
nia new hello
cd hello
```

```text
hello/
├── .gitignore
├── nia.toml
├── lib/root.nia
├── src/main.nia
└── tests/unit.nia
```

`nia.toml` is the manifest. It names the package and lists what it builds: a
library (`lib/root.nia`), an executable (`src/main.nia`), and a test suite
(`tests/unit.nia`).

The library holds the code worth sharing:

```nia
// A public API shared by the executable and the test suite.

// The text lives in static storage, so the slice that `greeting` returns
// stays valid after the call. Returning the literal itself would point at a
// temporary that ends with the function.
static greetingText: [char; 14] = "hello from nia";

pub fun greeting() &[char] {
    greetingText
}
```

`greeting` returns `&[char]`, a *slice*: a pointer to some characters plus
their count. The characters must outlive the call, so they are kept in a
`static`, which exists for the whole run of the program. Nia does not track
how long memory lives; making sure a pointer stays valid is part of writing
the program, and [Pointers, Arrays, And Slices](pointers-and-slices.md)
explains what to watch for.

The executable imports the library by the package's name:

```nia ignore
using std::io;
using std::process;
using hello;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    io::debugPrint("{}\n", [hello::greeting()]).?;
    !()
}
```

Build and run it from anywhere inside the package:

```sh
nia build run-hello
```

```text
hello from nia
```

`nia build` builds what the manifest describes and keeps the results under
`build/`. The first build compiles the standard library and takes a little
longer; later builds reuse everything that has not changed.

| Command | What it does |
| --- | --- |
| `nia build` | builds the executable and runs the tests |
| `nia build run-hello` | builds the executable, then runs it |
| `nia build test` | builds and runs the tests |
| `nia build --release` | builds with optimizations |
| `nia build --list` | lists the available steps |

The test suite is an ordinary program that checks the library and reports a
failure through its exit status; [Testing](testing.md) explains how to write
tests. Change `greetingText` to `"hello, world!!"` and run `nia build test` to
see a failing test.

Most `nia` commands in these chapters work the same from a checkout of the
compiler: run `tools/nia-dev`, or pass `--resource-root lib` to a compiler
built with Cargo.

## Where Next

[Values, Types, And Functions](basics.md) starts the language itself. The
[examples](../../examples/README.md) are short complete programs to read
alongside the chapters.
