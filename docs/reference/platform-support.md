# Platform Support

Nia is maintainer-tested. It has no formal support tiers: a configuration that
is built and tested is known to work, but that is not a minimum-version promise
or a guarantee for similar hosts and targets.

## Hosts And Targets

The host is the platform where the `nia` compiler runs. The target is the
platform that generated LLVM IR, object files, and executables are for. The two
are independent: a host can run the compiler without being a supported
executable target, and LLVM may emit objects for a target that Nia provides no
startup or linker integration for.

The global `--target <name>` option selects a target. Without it the compiler
targets its own host; a host outside the target list must name a target
explicitly. Generated code uses the target's baseline CPU, never the build
host's CPU features, so an artifact runs on every machine of its target.
Native object emission is also limited by the targets built into the LLVM that
the compiler links.

## Targets

| Target | Platform | Freestanding startup |
| --- | --- | --- |
| `x86_64-unknown-linux` | Linux x86_64 | yes |
| `x86-unknown-linux` | Linux i686 (experimental) | yes |
| `aarch64-unknown-linux` | Linux AArch64 | yes |
| `x86_64-pc-windows-msvc` | Windows x64 | yes |
| `x86_64-apple-macos` | macOS Intel | yes |
| `aarch64-apple-macos` | macOS Apple silicon | yes |

Every executable links with LLD for its target. Windows links import
`kernel32` and `bcryptprimitives` through import descriptions shipped with the
toolchain, so they need no Windows SDK.

## Target Facts

Each target defines six facts. Conditional compilation reads them by name in
`@[if ...]` (see [Attributes](language.md#attributes)), and programs can read
them as constants from `std::builtin::target` (`arch`, `vendor`, `os`, `env`,
`endian`, `pointerWidth`):

| Target | `arch` | `vendor` | `os` | `env` | `endian` | `pointerWidth` |
| --- | --- | --- | --- | --- | --- | --- |
| `x86_64-unknown-linux` | `"x86_64"` | `"unknown"` | `"linux"` | `""` | `"little"` | `64` |
| `x86-unknown-linux` | `"x86"` | `"unknown"` | `"linux"` | `""` | `"little"` | `32` |
| `aarch64-unknown-linux` | `"aarch64"` | `"unknown"` | `"linux"` | `""` | `"little"` | `64` |
| `x86_64-pc-windows-msvc` | `"x86_64"` | `"pc"` | `"windows"` | `"msvc"` | `"little"` | `64` |
| `x86_64-apple-macos` | `"x86_64"` | `"apple"` | `"macos"` | `""` | `"little"` | `64` |
| `aarch64-apple-macos` | `"aarch64"` | `"apple"` | `"macos"` | `""` | `"little"` | `64` |

`env` names the platform ABI flavor, not a C library: Linux targets link no
libc, and `msvc` selects the Windows x64 ABI without the MSVC runtime.

```nia
using std::builtin::target;
using std::process;

@[if os == "windows"]
fn separator() char {
    '\\'
}

@[if os != "windows"]
fn separator() char {
    '/'
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    if target::pointerWidth != 64 and target::pointerWidth != 32 {
        return process::ExitCode(1)!;
    }
    _ = separator();
    !()
}
```

## Runtime Model

Nia programs depend on the lowest operating-system interface that each
platform keeps stable, and on nothing above it by default. The toolchain adds
no C runtime: no CRT startup object, no C `main` convention for user code, and
no implicit libc. A C library is an ordinary dependency that a package links
explicitly, like any other native library, and Nia code uses it through
`extern` declarations (see [C interop](../guide/c-interop.md)). System
functions follow the same rule: the toolchain describes only the functions its
own sources call, and a program that declares any other links that library
explicitly.

The runtime selection decides who owns process entry, not whether a C library
is present:

- `freestanding`: the toolchain supplies process entry for an operating system.
  Its startup adapts the platform entry ABI, builds `process::Init`, and calls
  `pub fn main(init: process::Init) process::ExitCode!()`.
- `bare`: the toolchain supplies no entry and assumes no operating system. The
  program or its link configuration defines entry, as kernels, firmware, and
  embedded images do.

C's "hosted" environment, a libc plus a `main` called by the C runtime, has no
counterpart in this model.

| Platform | Stable boundary | Nia startup and system access | Not used by default |
| --- | --- | --- | --- |
| Linux | Kernel system-call ABI | `_start` from the kernel entry stack; self-relocating static-pie; direct system calls | libc, CRT objects, an interpreter |
| Windows | Documented Win32 API in system DLLs | Nia entry called by the loader; `kernel32` and `bcryptprimitives` | MSVC CRT, UCRT, undocumented exports |
| macOS | `libSystem` system interfaces | `main` entry called by `dyld` through `LC_MAIN`; `libSystem` system calls | `crt1.o`, C stdio and allocation |

macOS keeps neither its system-call numbers nor its system-call ABI stable, and
every macOS process starts through `dyld`, so `libSystem` is the stable
boundary there, in the same role that `kernel32` has on Windows. Nia uses its
POSIX system interfaces, such as `open`, `mmap`, `fork`, and `execve`, and
leaves its C library facilities to programs that link them explicitly. The
macOS entry symbol `main` (`_main` in Mach-O) is Nia startup, not a C runtime
`main`.

## Tested Configurations

| Configuration | Compiler host | Executables run | Notes |
| --- | --- | --- | --- |
| Linux x86_64 (Ubuntu 24.04-class, current Fedora) | full test suite | freestanding executables and standard library | release archive published |
| Linux AArch64 | native release build and CLI smoke test; cross-target executable tests under user-mode QEMU | selected freestanding executables | release archive published |
| Windows x86_64, MSVC toolchain | full test suite | freestanding executables, including child processes with redirected streams | release archive published |
| macOS Intel and Apple silicon | executable and standard-library suites | native executables | release archives published |
| Linux i686 target | cross-linked and executed in the Linux matrix | selected freestanding executables | target output; no separate compiler host archive |

Darwin executables are cross-linked from Linux and Windows and run on the
matching macOS Intel and Apple silicon runners in the build correctness matrix.

## Toolchain Policy

- Rust: the newest stable release; there is no pinned version or MSRV.
- LLVM: the default LLVM release of the newest stable Fedora (LLVM 23.1).
  Development builds may use a host LLVM installation; release builds use a
  pinned LLVM and are self-contained.
- Linker: LLD for every target, bundled in release archives.
- Target sysroots, CRT objects, libc, and dynamic loaders are platform inputs
  that a program brings in explicitly, not general toolchain requirements.

When a new Rust or LLVM release exposes a failure, the fix is made on the new
toolchain rather than by holding an older one. Build environment details are
in [Development setup](../dev/setup.md).

## Current Limits

Nia does not support:

- POSIX signal semantics on Windows: `Signal::Term` and `Signal::Kill` are
  termination requests implemented with `TerminateProcess`, and the result
  reports the Windows exit code rather than a signal;
- a complete bare-metal build workflow;
- LLVM-backed wasm32 compilation or wasm32 executables;
- every possible LLVM installation layout.
