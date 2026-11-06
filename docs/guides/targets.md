# Targets And Runtimes

A Nia compiler can build programs for any supported platform, not only the one
it runs on. This guide covers choosing a target, writing code that differs
between platforms, build profiles, and programs that run without an operating
system. The list of targets and their facts is in
[Platform support](../reference/platform-support.md).

## Host And Target

The *host* is the machine running `nia`; the *target* is the platform the
program is built for. They are independent: every release archive can build
for every supported target, because it bundles LLD and describes the system
libraries Nia links, so no platform SDK or cross toolchain is needed.

`--target <name>` selects the target:

```sh
nia emit --exe hello.nia -o hello --target x86_64-unknown-linux
nia build --target aarch64-apple-macos
nia check src/main.nia --runtime freestanding --target x86-unknown-linux
```

The names cover Linux on x86_64, i686, and AArch64, Windows x64, and macOS on
Intel and Apple silicon;
[Platform support](../reference/platform-support.md#targets) lists each one
with its facts.

Without `--target`, Nia builds for the host. Generated code uses the target's
baseline CPU, so an executable runs on every machine of its target.

With `nia build --target`, the package's artifacts are built for that target.
Steps that run programs during the build, such as test suites and the
programs that build steps invoke, are built for the host so they can run. A
program built for another platform cannot run on the host, so the manifest's
`run-<name>` steps exist only when the target is the host; `nia build --list`
shows which steps a target has. A build script can also pick a target for
each artifact with `forTarget`; see [Targets](build-scripts.md#targets) in
Custom Builds.

## Platform-Specific Code

`@[if condition]` keeps an item or statement only when the condition holds for
the target. The condition reads the target's facts, `os`, `arch`, `vendor`,
`env`, `endian`, and `pointerWidth`, and combines them with `==`, `!=`,
`and`, `or`, and `not`:

```nia
using std::builtin::target;
using std::io;
using std::process;

@[if os == "windows"]
fun separator() char {
    '\\'
}

@[if os != "windows"]
fun separator() char {
    '/'
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let sep = separator();
    let width = target::pointerWidth;
    io::debugPrint("separator={} pointerWidth={}\n", [sep, width]).?;
    !()
}
```

On Windows x64 this prints:

```text
separator=\ pointerWidth=64
```

Each target sees exactly one `separator`. The same facts are available to
ordinary code as constants in `std::builtin::target`, which suits differences
that are only values: an `if target::pointerWidth == 64` branch is still
checked on every target, while `@[if ...]` removes code before checking.

Inactive code must still parse, but it is not type-checked for that target.
A mistake in Windows-only code therefore goes unnoticed in a Linux build; check
a program that has platform-specific code once per target:

```sh
nia check src/main.nia --runtime freestanding --target x86_64-pc-windows-msvc
nia check src/main.nia --runtime freestanding --target x86_64-unknown-linux
```

Platform-specific interfaces in the standard library live under
`std::os::linux`, `std::os::windows`, and `std::os::macos`. Most programs use
the portable modules instead, `std::fs`, `std::process`, and `std::io`, which
work the same on every target.

## Build Profiles

A build is either a `debug` build, the default, or a `release` build, selected
with `--release` (or `--profile release`). The profile sets the default
optimization level, `-O0` for debug and `-O2` for release, which `-O0`
through `-O3`, `-Os`, and `-Oz` override.

`@[debug]` and `@[release]` keep an item or statement only in that profile,
which is useful for extra checks or logging during development:

```nia
fun process(value: i32) i32 {
    @[debug]
    _ = std::io::debugPrint("process({})\n", [value]);
    value * 2
}
```

The call names `std::io::debugPrint` in full instead of importing `std::io`.
A release build drops the statement, and an import used only by it would then
be reported as unused.

Integer overflow and the other [traps](../reference/language.md#unchecked-operations-and-traps)
behave the same in both profiles; the profile changes optimization, not
meaning. `@[test]` similarly keeps code only in test builds; see
[Testing](../learn/testing.md#testing-the-library).

## Runtimes

The runtime decides who starts the program:

- `freestanding`, the default for executables, links Nia's startup code. It
  receives control from the operating system, builds `process::Init`, and
  calls `pub fun main(init: process::Init) process::ExitCode!()`. It uses only
  the operating system's stable interface, with no C library.
- `bare` links no startup code and assumes no operating system. The program,
  or its link configuration, defines the entry point. This is for kernels,
  firmware, and other images that start themselves.

| Where | Freestanding | Bare |
| --- | --- | --- |
| `nia.toml` artifact | default | `runtime = "bare"` |
| build script | default | `.withRuntime(build::Runtime::Bare)` |
| `nia check`, `nia emit` | `--runtime freestanding` | default |
| `nia emit --exe` | always | not supported |

In a bare program `main` is an ordinary function, and nothing builds
`process::Init`. The standard library's modules that call the operating
system, such as `std::fs` and `std::process`, still compile but only work when
the program actually runs under that system. Allocators that need no
operating system, such as `mem::FixedBufferAllocator`, work anywhere. The
per-platform details of each runtime are in the
[runtime model](../reference/platform-support.md#runtime-model).
