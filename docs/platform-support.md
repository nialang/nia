# Platform Support

Nia is maintainer-tested rather than released with formal platform support
tiers. A passing environment is evidence for that configuration, not a minimum
version promise or a guarantee for similar hosts and targets.

## Host And Target Boundaries

Host and target support are separate:

- the host is the platform where the `nia` compiler runs;
- the target is the platform described by generated LLVM IR, object files, or
  executables.

A host may run the compiler without being a supported executable target. An
LLVM target may accept object emission without Nia providing startup code,
linker integration, or a tested runtime for it.

## Maintained Configurations

The repository is exercised in these environments:

- the maintainer's current Fedora Linux x86_64 environment;
- managed `ubuntu-24.04` x86_64 correctness and performance workflows using
  LLVM 22; and
- freestanding Linux x86_64 executable tests using the standard-library startup
  facade and a target linker without CRT startup; and
- experimental i686 workspace compilation and selected freestanding executable
  tests using the standard-library `int 0x80` startup/syscall facade.

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

Ubuntu hosted runners install the matching LLVM release from `apt.llvm.org`.
Ubuntu is an execution venue, not the version authority. Managed runs report
the resolved Rust, Cargo, Clippy, rustfmt, and `llvm-config` identities so a
failure is tied to its actual toolchain rather than a copied environment
snapshot in this document.

New upstream stable releases may expose diagnostics or build failures. The
maintenance policy is to reproduce and fix those failures on the current
toolchain, not to weaken strict lint or test policy to preserve an older
environment.

## Current Limits

The project does not claim support for:

- Windows or macOS compiler hosts;
- stable target-triple selection or cross-compilation behavior;
- complete freestanding executable startup outside Linux x86_64 and experimental
  i686 coverage;
- a complete bare-metal build workflow;
- target-aware linker selection beyond the `NIA_LINKER` override; or
- every valid LLVM installation layout.

Changes to one of these boundaries require implementation, maintained tests,
and an explicit support-policy update. Incidental success does not change the
support claim.
