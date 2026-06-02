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

Nia should eventually support both hosted and bare workflows, but these are
different promises. A host platform can run `nia` without being a supported
target, and a target can be emitted as object code without `nia` providing a
full executable or linking workflow for it.

## Current Status

Current support is intentionally narrow:

- `nia` is primarily tested in the maintainer's local Linux environment;
- LLVM is required through the Rust `llvm-sys` dependency;
- hosted executable emission relies on the system C toolchain and currently
  invokes `cc`;
- native object emission depends on the LLVM target configuration available to
  the local toolchain;
- cross compilation is not documented as a supported workflow yet.

This does not mean other hosts or targets cannot work. It means the project does
not yet claim support for them.

## Known Maintainer Environment

This is the current known working environment snapshot, not a minimum
requirement or support guarantee.

Snapshot date: 2026-05-25.

- OS: Fedora release 44 (Forty Four)
- Architecture: x86_64
- libc: GNU libc 2.43
- Rust: rustc 1.95.0
- Cargo: cargo 1.95.0
- C compiler driver: `cc` resolves to `/usr/sbin/cc`
- C compiler implementation: `/usr/bin/gcc`
- C compiler version: GCC 16.1.1 20260515 (Red Hat 16.1.1-2)
- LLVM config tool: `/usr/sbin/llvm-config`
- LLVM version: 22.1.5
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
  object emission, and hosted executable emission.
- Tier 2: expected to build or emit code, but not tested as strictly.
- Planned or unknown: platforms that may be desirable but have no support
  commitment yet.

No platform is Tier 1 yet.

## Not Guaranteed Yet

The following are not currently guaranteed:

- Windows host support;
- macOS host support;
- stable target triple selection;
- stable cross compilation behavior;
- libc-free executable generation;
- a complete bare-metal build story;
- compatibility with every LLVM installation layout.

## Future Work

Expected platform work includes:

- documenting exact Rust, LLVM, linker, and system toolchain requirements;
- adding CI for at least one Linux host;
- making target selection explicit in the CLI;
- separating host executable linking from bare object emission more clearly;
- documenting freestanding workflows once export symbols and runtime boundaries
  are more complete.
