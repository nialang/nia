# Platform Support

Nia is maintainer-tested rather than released with formal platform support
tiers. A passing environment is evidence for that configuration, not a minimum
version promise or a guarantee for similar hosts and targets.

## Host And Target Boundaries

The maintained targets are `x86_64-unknown-linux`, `x86-unknown-linux`,
`aarch64-unknown-linux`, `x86_64-pc-windows-msvc`, `x86_64-apple-macos`, and
`aarch64-apple-macos`. The global `--target <name>` option selects one;
without it the compiler targets its own host. A host outside this list must
name a target explicitly. Generated code uses the target's baseline CPU, never
the build host's CPU features, so an artifact runs on every machine of its
target.

Host and target support are separate:

- the host is the platform where the `nia` compiler runs;
- the target is the platform described by generated LLVM IR, object files, or
  executables.

A host may run the compiler without being a supported executable target. An
LLVM target may accept object emission without Nia providing startup code,
linker integration, or a tested runtime for it.

Runtime selection is a separate axis: Nia's `freestanding` injects Nia startup,
and `bare` omits it. Neither choice alone specifies system-library dependencies
or corresponds to C's hosted/freestanding environment classification.

## Runtime Model

Nia programs depend on the lowest operating-system interface that each platform
keeps stable, and on nothing above it by default. The toolchain does not add a
C runtime: there is no CRT startup object, no C `main` convention for user
code, and no implicit libc. A C library is an ordinary dependency that a
package links explicitly, like any other native library; Nia code can then use
it through `extern` declarations. The same holds for system functions: the
toolchain supplies import descriptions for only the functions its own sources
call, and a program declaring any other links that library explicitly.

The two runtime selections describe who owns process entry, not whether a C
library is present:

- `freestanding`: the Nia toolchain supplies process entry for an operating
  system. Its startup adapts the platform entry ABI, builds `process::Init`, and
  calls the user entry contract.
- `bare`: the toolchain supplies no entry and assumes no operating system. The
  program or its link configuration defines entry, as kernels, firmware, and
  embedded images do.

C's "hosted" environment, a libc plus a `main` called by the C runtime, has no
counterpart in this model.

The stable boundary differs by platform, so the interface Nia binds differs too:

| Platform | Stable boundary | Nia startup and system access | Not used by default |
| --- | --- | --- | --- |
| Linux | Kernel system-call ABI | `_start` from the kernel entry stack; direct system calls | libc, CRT objects |
| Windows | Documented Win32 API in system DLLs | Nia entry called by the loader; `kernel32` and `bcryptprimitives` | MSVC CRT, UCRT, undocumented exports |
| Darwin | `libSystem` system interfaces | `main` entry called by `dyld` through `LC_MAIN`; `libSystem` system calls | `crt1.o`, C stdio and allocation |

Darwin does not keep its system-call numbers or ABI stable, and a macOS process
always starts through `dyld`; `libSystem` is therefore the stable boundary, in
the same role that `kernel32` has on Windows. Nia uses its POSIX
system-interface functions, such as `open`, `mmap`, and `posix_spawn`, and
leaves its C library facilities to programs that link them explicitly. The
Darwin entry symbol is `main` only because that is the name `dyld` and the
linker expect; it is Nia runtime startup, not a C runtime `main`.

## Maintained Configurations

The repository is exercised in these environments:

- the maintainer's current Fedora Linux x86_64 environment;
- the maintainer's Windows x86_64 MSVC host for workspace compilation,
  cache/publication, compiler-host tests, and the maintained Windows freestanding
  executable workflow; and
- managed `ubuntu-24.04` x86_64 correctness and performance workflows using
  LLVM 23; and
- a managed `windows-2022` x86_64 correctness workflow that builds the pinned
  static LLVM 23.1.2 prefix and runs formatting, strict Clippy, the complete
  workspace test suite, and a release compiler build under MSVC; and
- freestanding Linux x86_64 executable tests using toolchain runtime startup
  source and a target linker without CRT startup; and
- freestanding Windows x86_64 executable tests using the toolchain startup,
  `lld-link`, and direct Win32 process/file output primitives; and
- experimental i686 workspace compilation and selected freestanding executable
  tests using runtime-owned `int 0x80` startup/syscall source.

The i686 LLVM-backed matrix requires an LLVM installation whose headers include
the target-width configuration (for the current LLVM 23 packages,
`LLVM_SYS_231_PREFIX=/usr/lib/llvm23`). A distro prefix that exposes only the
x86_64 `llvm-config` headers is not sufficient for 32-bit C-wrapper builds.

Native object emission is limited by the targets built into the selected LLVM
installation. The managed Linux workflows are architecture and regression
guards; they do not establish a general Linux, host, or target compatibility
promise.

## Toolchain Policy

Rust follows the newest stable channel without a repository pin or MSRV. LLVM
follows the default LLVM release in the newest stable Fedora release. The
`llvm-sys` dependency family, `LLVM_SYS_*_PREFIX` environment name, linker
package, and managed workflow installation move together when that LLVM
identity changes.

Development builds may use a host LLVM installation. Release builds use the
pinned LLVM toolchain and are checked for a self-contained LLVM dependency.
Target sysroots, CRT objects, libc, and the target dynamic loader remain
platform inputs rather than general host requirements.

Ubuntu hosted runners install the matching LLVM release from `apt.llvm.org`.
The Windows hosted runner builds the pinned static LLVM prefix with
`tools/llvm/build-static.ps1` and caches it by that script's contents.
Hosted runners are execution venues, not the version authority. Managed runs
report the resolved Rust, Cargo, Clippy, rustfmt, and `llvm-config` identities
so a failure is tied to its actual toolchain rather than a copied environment
snapshot in this document.

New upstream stable releases may expose diagnostics or build failures. The
maintenance policy is to reproduce and fix those failures on the current
toolchain, not to weaken strict lint or test policy to preserve an older
environment.

## Current Limits

The project does not claim support for:

- complete Windows filesystem/process standard-library coverage. Windows
  freestanding startup, child creation with redirected standard streams,
  explicit environments, waiting, and termination have executable coverage;
  broader filesystem behavior and Windows-specific process semantics remain
  staged and require focused tests before they are treated as maintained;
- Windows does not expose POSIX child signals. Nia accepts `Signal::Term` and
  `Signal::Kill` as termination requests and implements both with
  `TerminateProcess`, reporting the resulting Windows exit code rather than a
  POSIX signal termination;
- macOS compiler hosts;
- Windows release archives. Windows compilers are built from source; only the
  Linux x86_64 archive is packaged and published;
- cross-linked macOS executables. `--target` selects any maintained target for
  checking, IR, and object emission from every maintained host, and the linker
  flavor follows the artifact target; Mach-O linking is not implemented until
  the platform roadmap's D1 row closes;
- complete freestanding executable startup outside Linux x86_64, Windows x86_64,
  and experimental i686 coverage;
- a complete bare-metal build workflow;
- LLVM-backed wasm32 compilation or executable runtime support. The pure-Rust
  frontend, semantic, query, and maintenance owners are checked for wasm32,
  while the `llvm-sys` C API dependency is intentionally outside that target
  boundary; or
- every valid LLVM installation layout.

Changes to one of these boundaries require implementation, maintained tests,
and an explicit support-policy update. Incidental success does not change the
support claim.
