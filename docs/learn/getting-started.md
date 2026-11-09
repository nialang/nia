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
  environment variables. This program does not need it; `_ = init;` says so
  explicitly. The compiler does not require it, but the standard library and
  these chapters write it to show the parameter is unused on purpose.
- The return type `process::ExitCode!()` is an *error union*: the function
  either succeeds with the unit value `()` or fails with an `ExitCode`. Nia has
  no exceptions; failure is part of the return type.
- `io::debugPrint` writes a formatted message to standard error. Its first
  argument is a template, and `{}` placeholders take their values from the
  array that follows, here empty.
- Writing to a stream can fail, so `debugPrint` returns an error union too.
  The postfix `.?` either unwraps a success or returns the error from `main`
  right away. The standard library's input and output errors convert to an
  exit status, so a failed write ends the program with a failure status.
- `!()` is the success value: `!` wraps a value as the success case of an
  error union, and `()` is the unit value. It is the last expression of the
  block without a semicolon, so it is the function's result.

## Printing Values

`debugPrint`'s `{}` placeholders take values from the array that follows, in
order. A placeholder can carry a format specification directly inside its braces:

```nia
using std::io;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let answer = 42;
    let name = "nia";
    io::debugPrint("answer={} padded={>5} hex={#x}\n", [answer, answer, answer]).?;
    io::debugPrint("name={} short={.2}\n", [name, name]).?;
    !()
}
```

```text
answer=42 padded=   42 hex=0x2a
name=nia short=ni
```

| Specification | Effect |
| --- | --- |
| `{5}` | pad to width 5; without an alignment, the padding goes before the value |
| `{>5}`, `{<5}`, `{^5}` | pad to width 5, aligned right, left, or centered |
| `{_>5}` | pad with `_` instead of spaces |
| `{05}`, `{+}` | pad with zeros; always show the sign |
| `{x}`, `{X}`, `{b}`, `{o}`, `{#x}` | hexadecimal, binary, octal; `#` adds the prefix |
| `{.2}` | at most 2 characters of text, or 2 digits after a float's decimal point |
| `{*}`, `{.*}` | width or precision taken from the next value, a `usize` |
| `{{`, `}}` | a literal `{` or `}` |

The parts combine in a fixed order, flags, width, precision, then the
presentation letter, as in `{_>6.3}` or `{#06x}`. The
[standard library overview](../../lib/README.md#formatting) lists every part
and which types accept it.

The compiler does not check templates, by design: `std::fmt` is an ordinary
library with no special support from the compiler, and to the compiler a
template is just an array of `char` like any other text. Mistakes therefore
surface when the program runs. A malformed placeholder, or a value count that
does not match the placeholders, makes `debugPrint` fail without printing
anything, and `.?` then ends the program with exit status 22. A program that
unexpectedly prints nothing and exits with 22 has a template mistake.

Integers, floating-point numbers, `bool`, `char`, and text can be printed. A
float prints in the shortest form that reads back as the same value, so `0.1`
prints as `0.1`. An infinity or NaN cannot be printed: passing one makes
`debugPrint` fail with `fmt::Error::NonFinite` (exit status 33). The values
in the array are named variables or fields. An expression such as `answer + 1`
or a literal such as `42` cannot be written there directly; bind it to a name
first.

`debugPrint` writes to standard error, which suits diagnostics and these
chapters' examples. A program's real output goes to standard output through a
buffered writer:

```nia
using std::fmt::template;
using std::io;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut buffer: [u8; 256] = [0; 256];
    let mut stdout = io::FileWriter::stdout(buffer);
    let count = 3;
    stdout.print("count={}\n", [count]).?;
    stdout.flush().?;
    !()
}
```

```text
count=3
```

The writer collects output in `buffer` and writes it out when the buffer fills
or `flush` is called; always flush before the program ends. `print` takes the
same templates as `debugPrint`, and comes from `std::fmt::template`, which the
program imports.

## Command-Line Arguments

`init.args()` gives the program's arguments. `skipProgram()` leaves out the
first one, the program's own name, and each argument can be parsed into a
number:

```nia
using std::fmt::template;
using std::io;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    let mut buffer: [u8; 256] = [0; 256];
    let mut stdout = io::FileWriter::stdout(buffer);

    let mut total: i64 = 0;
    let mut skipped: usize = 0;
    for arg in init.args().skipProgram() {
        if arg.parse[i64]() is !value {
            total += value;
        } else {
            skipped += 1;
        }
    }

    stdout.print("total={} skipped={}\n", [total, skipped]).?;
    stdout.flush().?;
    !()
}
```

```sh
nia emit --exe sum.nia -o sum
./sum 10 20 x 12
```

```text
total=42 skipped=1
```

`arg.parse[i64]()` returns an error union, which `is !value` tests; the
[errors chapter](errors.md) explains both. `init.args().get(1)` reads one
argument as an optional, and `init.env()` gives the environment variables.

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
entry contract. In Nia, *freestanding* means the program starts through Nia's
own startup code, which talks to the operating system directly without a C
library; it is what ordinary programs use. Without the option, `nia check`
treats `main` as an ordinary function, which is what kernels and other
programs without an operating system need. `nia emit --exe` always builds the
standard entry. [Targets And Runtimes](../guides/targets.md#runtimes) explains
both runtimes.

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
// stays valid after the call.
static greetingText: [char; 14] = "hello from nia";

pub fun greeting() &[char] {
    greetingText
}
```

`greeting` returns `&[char]`, a *slice*: a pointer to some characters plus
their count. The characters must outlive the call, so they are kept in a
`static`, which exists for the whole run of the program; returning the
literal itself is rejected, because it is a temporary of the call. Beyond such
direct cases Nia does not track how long memory lives; making sure a pointer
stays valid is part of writing the program, and
[Pointers, Arrays, And Slices](pointers-and-slices.md) explains what to watch
for.

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
