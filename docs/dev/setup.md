# Development Setup

The compiler is a Rust workspace that links LLVM 23.1 through `llvm-sys`.
This page covers building it from a checkout on Linux and Windows. Gate
commands and test rules are in [Testing](testing.md).

## Requirements

| Tool | Requirement |
| --- | --- |
| Rust | Newest stable toolchain with `clippy` and `rustfmt`. The project does not pin a release or promise an MSRV; update stable before interpreting gate results. |
| LLVM | 23.1 with `llvm-config` on `PATH`, or its prefix in `LLVM_SYS_231_PREFIX`. |
| LLD | Used to link executables for every target: `ld.lld` (ELF), `lld-link` (COFF), `ld64.lld` (Mach-O), found on `PATH` or through `NIA_LLD`. |
| clang 23 | Needed by the C ABI and C interoperability tests, which compare `extern` declarations with clang's and compile their C halves with it. |

```sh
rustup toolchain install stable --component clippy --component rustfmt
```

## Linux

Install LLVM 23.1 development files, LLD, and clang. Distribution packages often
install LLD only in the versioned LLVM `bin` directory, such as
`/usr/lib/llvm-23/bin` on Ubuntu (apt.llvm.org packages `llvm-23-dev`,
`lld-23`, `clang-23`):

```sh
export PATH=/usr/lib/llvm-23/bin:$PATH
cargo build --workspace
```

For an LLVM installation outside the standard layout, set its prefix:

```sh
LLVM_SYS_231_PREFIX=/path/to/llvm-23.1 cargo build --workspace
```

On Linux x86_64 the workspace tests also build and run executables for the
other Linux targets: i686 natively, which needs a 32-bit C toolchain
(`gcc-multilib`) for the C interoperability tests, and aarch64 under user-mode
QEMU (`qemu-user-static`).

## Windows (MSVC)

Use an LLVM prefix built with the static MSVC CRT (`/MT`, or
`CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`). The repository's
[`.cargo/config.toml`](../../.cargo/config.toml) enables Rust's `crt-static`
for `x86_64-pc-windows-msvc`, and the C++ LLVM bridge follows it. LLVM archive
linkage and CRT linkage are separate choices: `llvm-config --shared-mode` does
not tell you which CRT a prefix uses. [`tools/llvm/build-static.ps1`](../../tools/llvm/README.md)
builds a suitable pinned prefix.

Set the LLVM prefix and its `bin` directory once at user scope so new
PowerShell, Cargo, and IDE processes inherit them:

```powershell
$llvm = 'C:\Users\you\tools\llvm-23.1.2'
[Environment]::SetEnvironmentVariable('LLVM_SYS_231_PREFIX', $llvm, 'User')
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$llvmBin = Join-Path $llvm 'bin'
if (($userPath -split ';') -notcontains $llvmBin) {
  [Environment]::SetEnvironmentVariable('Path', "$userPath;$llvmBin", 'User')
}
```

Restart existing terminals and editors after changing user environment
variables. If an official LLVM Windows archive reports library paths from its
build machine, point the prefix at a corrected installation (or one built with
`build-static.ps1`) rather than relying on a temporary process wrapper.

Build from an x64 Developer PowerShell for Visual Studio, or a shell initialized
by `VsDevCmd.bat -arch=x64 -host_arch=x64`, so `cl`, `link`, `LIB`, and
`INCLUDE` come from the installed MSVC and Windows SDK:

```powershell
cargo build -p nia-cli --no-default-features --features llvm-static --target x86_64-pc-windows-msvc
```

The executable is `target/x86_64-pc-windows-msvc/debug/nia.exe`. Keep the
explicit `--target`: it applies the CRT flags to target code only. Without it,
`llvm-sys`'s build script also receives those flags, requests bundled static
Windows system libraries, and can fail because `psapi.lib` is installed but not
in rustc's archive search path. The explicit target avoids global SDK
`-Lnative` flags or 8.3 short paths.

### Windows troubleshooting

Distinguish missing installations from shell setup:

- A missing LLVM prefix or MSVC C++/Windows SDK component needs installation or
  selection of the correct prefix. Check the actual installation, not only
  whether `cl.exe` is on `PATH`; Rust's native build tooling can discover
  installed MSVC tools itself.
- The developer shell supplies version-matched tool paths, `LIB`, and `INCLUDE`
  for external compiler, linker, and archive commands. A normal shell lacking
  these variables does not prove the tools are absent.
- Running processes keep their inherited environment. A user-level setting
  changed after a terminal or editor started does not reach its child commands.
  Inspect both scopes when they disagree.
- `RUSTFLAGS` and `CARGO_ENCODED_RUSTFLAGS` override the repository's target
  flags. Do not persist CRT flags or versioned VS/SDK paths there, and check for
  stale overrides before attributing a failure to missing tools.

## WSL

WSL builds follow the Linux instructions. Tests and the static LLVM build size
their parallelism from the Linux VM's visible CPU and memory; on a VM with
limited memory, lower `LLVM_BUILD_JOBS` for `tools/llvm/build-static.sh` or
raise the WSL memory and swap allocation.

## Running the Compiler From a Checkout

Run the compiler with the checkout's standard-library resources:

```sh
cargo run -p nia-cli -- --resource-root lib check examples/hello.nia --runtime freestanding
```

For repeated use, build the release binary and use the development launcher,
which always pairs `target/release/nia` with this checkout's `lib`:

```sh
cargo build --release -p nia-cli
tools/nia-dev check examples/hello.nia --runtime freestanding
```

`tools/nia-dev` is a Bash script; on Windows run the binary directly with
`--resource-root lib`.

## Release Builds

A release compiler uses the pinned static LLVM prefix, not the default
development feature:

```sh
tools/llvm/build-static.sh
tools/release/build.sh
```

See [`tools/llvm/README.md`](../../tools/llvm/README.md) and
[`tools/release/README.md`](../../tools/release/README.md) for overrides and
Linux packaging.

## Environment Variables

Normal compiler behavior is configured with CLI flags or typed API options.
The variables the toolchain and test harness read are registered here; do not add a new
`NIA_*` variable without adding it to this table and deciding whether it should
be a CLI/API option instead.

| Variable | Scope | Effect |
| --- | --- | --- |
| `NIA_LINKER` | User | Overrides the executable linker. The program follows the artifact target's system argument contract: GNU for ELF, `lld-link` for COFF, `ld64.lld` for Mach-O. |
| `NIA_LLD` | User | Overrides the discovered LLD executable. A generic multi-flavor LLD binary receives the target's `-flavor`; dedicated drivers such as `lld-link` use their own command line. |
| `NIA_AR` | User | Overrides the static archive tool; otherwise `llvm-ar`, then `ar`, is discovered. |
| `NO_COLOR` | User | Standard terminal convention respected by CLI help and diagnostics rendering. |
| `LLVM_SYS_231_PREFIX` | Build | `llvm-sys` override for a non-standard LLVM installation. |
| `NIA_TEST_RESOURCE_TRACE=1` | Tests | Prints permit waits and the peak process count/RSS of each managed test command. Useful when tuning a new host; it does not change scheduling or limits. |

Test-only fixture variables such as `NIA_TEST_ENV` may appear inside tests that
verify process-environment behavior.
