# Platform Support

Nia is currently maintainer-tested rather than platform-supported.

The compiler is developed and tested primarily on the maintainer's local Linux
environment. Other systems may work, but they are not currently part of a
documented support guarantee.

## Host and Target

Platform support has two separate meanings:

- host platform: where `nia` itself runs;
- target platform: where generated LLVM IR, object files, or executables are
  expected to run.

Nia should eventually support both compiler-host and target-runtime workflows,
but these are different promises. A host platform can run `nia` without being a
supported target, and a target can be emitted as object code without `nia`
providing a full executable or linking workflow for it.

## Current Status

Current support is intentionally narrow:

- `nia` is primarily tested in the maintainer's local Linux environment;
- LLVM is required through the Rust `llvm-sys` dependency;
- executable emission uses an injected standard-library package startup facade
  and currently has a freestanding Linux x86_64 implementation that invokes the
  target linker without CRT startup;
- native object emission depends on the LLVM target configuration available to
  the local toolchain;
- the repository defines managed `ubuntu-24.04` x86_64 correctness and
  performance workflows that install LLVM 22 and exercise the complete
  compiler/LLVM workload suites;
- cross compilation is not documented as a supported workflow yet.

This does not mean other hosts or targets cannot work. It means the project does
not yet claim support for them.

## Toolchain Update Policy

The project follows the maintainer's current Fedora development environment
rather than defining old minimum compiler versions. Rust tracks the newest
stable channel without a repository pin or MSRV. LLVM tracks the default LLVM
release supplied by the newest stable Fedora release; the `llvm-sys` binding,
`LLVM_SYS_*_PREFIX` name, linker package, and managed workflow installation move
together when Fedora changes that identity. The Ubuntu hosted runners install
the matching LLVM release from `apt.llvm.org`; Ubuntu is the execution venue,
not the version authority.

Every managed run reports its Rust/Cargo/Clippy/rustfmt identity and
`llvm-config --version`. A new upstream stable release may legitimately expose
new diagnostics. Maintainers reproduce those diagnostics after updating the
local toolchain and fix the code rather than weakening strict lint policy.

## Known Maintainer Environment

This is the current known working environment snapshot, not a minimum
requirement or support guarantee.

Snapshot date: 2026-08-09.

- OS: Fedora Linux 44 (WSL)
- Architecture: x86_64
- libc: GNU libc 2.43
- Rust: rustc 1.97.1
- Cargo: cargo 1.97.1
- default executable linker: `ld` resolves to `/usr/sbin/ld`
- linker implementation: GNU ld 2.46
- LLVM config tool: `/usr/sbin/llvm-config`
- LLVM version: 22.1.6
- LLVM host target: x86_64-redhat-linux-gnu
- Rust LLVM binding dependency: `llvm-sys = 221.0.1`

The local LLVM installation reports these built targets:

```text
AArch64 AMDGPU ARM AVR BPF Hexagon Lanai LoongArch Mips MSP430 NVPTX PowerPC
RISCV Sparc SPIRV SystemZ VE WebAssembly X86 XCore
```

## Support Tiers

The project may use these tiers as platform work becomes more formal:

- Tier 0: maintainer-tested local environment. This is the current state.
- Tier 1: regularly tested hosts and targets with expected working compiler,
  object emission, and freestanding executable emission.
- Tier 2: expected to build or emit code, but not tested as strictly.
- Planned or unknown: platforms that may be desirable but have no support
  commitment yet.

No platform is Tier 1 yet. The managed performance workflow is an architecture
and regression guard, not a general host/target support promise.

## Not Guaranteed Yet

The following are not currently guaranteed:

- Windows host support;
- macOS host support;
- stable target triple selection;
- stable cross compilation behavior;
- a complete bare-metal build story;
- compatibility with every LLVM installation layout.

## Future Work

Expected platform work includes:

- keeping the Rust and Fedora-derived LLVM identity current and observable;
- adding CI for at least one Linux host;
- making target selection explicit in the CLI;
- making executable linker selection target-aware beyond the current
  `NIA_LINKER` override;
- documenting freestanding workflows across more targets once runtime
  boundaries are more complete.
