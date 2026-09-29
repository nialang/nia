<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Platform Roadmap

The 0.3.0 line turns Nia from a Linux x86_64 compiler with a Windows port into
a target matrix: every maintained target is selected explicitly, compiled
from any maintained host, linked without a platform SDK, and executed in a
managed environment that runs that target natively.

This roadmap is bounded. It closes when every ledger row below is verified;
durable rules then move to their owner documents
([platform support](platform-support.md), [architecture](architecture.md),
[ABI](nia-abi.md)) and this file is deleted.

Nia keeps no compatibility baggage before 1.0. Code that assumed a single
host-equals-target Linux x86_64 configuration is replaced, not wrapped: no
string target fields beside a typed model, no host `cfg!` decisions standing
in for target decisions, no fallback to the host when a target is unknown.

## Decisions

- Target matrix: `x86_64-linux`, `x86-linux`, `aarch64-linux`,
  `x86_64-windows`, `x86_64-macos`, `aarch64-macos`.
- Conditional compilation spells Darwin as `os == "macos"`.
- macOS minimum deployment target is 15.0 for both architectures: the oldest
  release that hosted CI can execute, one Mach-O fixup format (chained fixups),
  and no `dyld_stub_binder`. x86_64 macOS execution is verified on the Intel
  runner while one exists; afterwards it keeps cross-link structural checks.
- Darwin startup exports `main`, called by `dyld` through `LC_MAIN`, as Zig and
  Rust do. It is Nia runtime startup; no `crt1.o` or C runtime `main`.
- Windows randomness uses `ProcessPrng` from `bcryptprimitives.dll`; the
  undocumented `SystemFunction036` export and the `advapi32` link are removed.
- Import descriptions for system libraries (`kernel32`/`bcryptprimitives`
  module definitions, `libSystem` text stubs) are toolchain resources listing
  only the symbols the standard library uses. Linking needs no platform SDK.
- aarch64 Linux is the first aarch64 target because it can be executed under
  user-mode QEMU on a Linux host; Darwin arm64 then reuses its code generation
  and ABI work.

## Acceptance Ledger

`Open` rows are not started, `Active` rows are in progress, `Verified` rows have
owner tests and end-to-end evidence for their stated boundary.

| ID | State | Boundary and owner | Required evidence for closure |
| --- | --- | --- | --- |
| T1 | Active | Typed target model: `nia-target`, `nia-target-config` | One typed target (architecture, OS, environment, object format, OS version) owns triple parsing and printing, pointer width, endianness and the condition vocabulary. String target fields, `TargetConfig::host()` string guessing and duplicate `TargetSpec` conversions are gone. |
| T2 | Open | Target selection: CLI, build plan, `std::build` | `--target <triple>` on every compiling command; build scripts can request an artifact target; the frozen plan carries typed targets. Unknown triples are user diagnostics listing the supported set. |
| T3 | Open | Codegen follows the artifact target: `nia-llvm`, `nia-codegen-llvm` | Every target in the matrix initializes and creates its target machine from the artifact target, with a target-appropriate baseline CPU. No codegen call site reads the host triple, CPU or features. Host-`cfg!` decisions in compiler builtins become target decisions. |
| T4 | Open | Linker flavor from the target: `nia-linker`, `nia-toolchain` | ELF, COFF and Mach-O flavors are chosen by the artifact target; bundled or overridden lld binaries resolve by flavor. Import descriptions replace SDK import libraries; `advapi32` is removed. |
| T5 | Open | Cross-execution evidence | Linux and Windows hosts each build and run the other's executables where a runner exists; Linux runs i686 natively and aarch64 under QEMU. |
| A1 | Open | C ABI lowering: `nia-abi-check`, codegen | By-value `extern struct` arguments and returns, `sret`/`byval`, small-integer extension and variadic default promotion follow SysV x86_64, Win64 and AAPCS64 (including Apple arm64). Differential tests compare against Clang's IR for the same C declarations. |
| R1 | Open | aarch64 Linux runtime and std | Startup, system calls and inline assembly dialect for aarch64 Linux; the executable test suite passes under QEMU. |
| D1 | Open | Darwin link: `nia-linker` | Mach-O links with `-arch`, `-platform_version macos 15.0`, `-e _main` and the bundled `libSystem` stub; arm64 output is ad-hoc signed; every undefined symbol is satisfied by the stub. |
| D2 | Open | Darwin runtime and std | `runtime/start/freestanding/macos/*` and `std/os/macos/*` implement the platform contract through `libSystem` system interfaces, errno through `__error()`, and the x86_64 `$INODE64` symbol variants. |
| D3 | Open | Darwin execution evidence | A managed macOS job runs the executable and standard-library suites on arm64 macOS and on Intel macOS while a runner exists. |
