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

## CPU-Specific Code

Newer processors of one target add instructions, and the
[CPU level](../reference/platform-support.md#cpu-levels) of a build says which
the code may use. `hasFeature("name")` in a condition tests the level, so one
source can carry a path for a wider vector unit next to a portable one:

```nia
using std::builtin::target;

@[if arch == "x86_64"]
@[if hasFeature("avx2")]
fun chunkBytes() usize {
    32
}

@[if not (arch == "x86_64" and hasFeature("avx2"))]
fun chunkBytes() usize {
    16
}

// The same answer as a value, for code that differs only in a number.
const unroll: usize = if target::hasFeature(.Avx2) { 4 } else { 2 };
```

The level comes from the build, never from the machine compiling it, so a
default build is portable and takes the 16-byte path. Select a level with
`--cpu`, or declare one for release builds in the manifest:

```sh
nia build --cpu x86_64-v3
```

```toml
[profile.release]
cpu = "x86_64-v3"
```

A build for a level fails with an illegal instruction on an older processor, so
choose one only when every machine that runs the program has it. As with
platform-specific code, check each level a program supports:
`nia check src/main.nia --cpu x86_64-v3`.

### Runtime CPU Queries

`std::cpu::supports` answers whether the running process can use a feature:

```nia
using std::cpu;

pub fun canUseAvx2() bool {
    cpu::supports(.Avx2)
}
```

It takes the same `CpuFeature` enum as `target::hasFeature`. When the build
level already guarantees a feature, the answer is `true` and optimized calls
fold away. Otherwise, x86 detection checks CPUID and, for AVX, FMA, F16C, and
AVX-512, the operating system's register-state support. The result is cached
without allocation. Features of another architecture are `false`. AArch64's
current feature, `Neon`, is guaranteed by its baseline.

A positive runtime query does not change how the surrounding function is
compiled. Use `target::hasFeature` for choices fixed by the build and
`cpu::supports` when the decision depends on the machine running the program.

### Function CPU Features

Use a separate function when a runtime decision should select code generated
for a different instruction set:

```nia
using std::builtin;

@[if arch == "x86" or arch == "x86_64"]
@[cpuFeatures("avx2")]
fun doubleAvx2(value: u8x32) u8x32 {
    builtin::saturatingAdd(value, value)
}

pub fun doubled(value: u8x32) u8x32 {
    @[if arch == "x86" or arch == "x86_64"]
    if std::cpu::supports(.Avx2) {
        return doubleAvx2(value);
    }
    builtin::saturatingAdd(value, value)
}
```

`cpu::supports` also checks the operating system's vector-state support. The
attribute changes code generation for `doubleAvx2`; it does not change
`@[if hasFeature(...)]` inside that function. The
[language reference](../reference/language.md#function-cpu-features) defines
closure inheritance, inlining, and debug entry checks. A release build relies
on the explicit branch to avoid calling an unsupported function.

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
