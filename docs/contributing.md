# Contributing to Nia

Nia is a maintainer-led personal project. Contributions, issues, and design
discussions are welcome, but final language and implementation decisions rest
with the project maintainer.

This project values a small coherent language more than consensus-driven feature
growth. A technically valid change may still be rejected if it does not fit the
direction of Nia.

## Project Status

Nia's first maintained release line is active. Compatibility with earlier
experimental syntax is not a goal. Removed behavior should not receive
migration paths, compatibility tests, or diagnostics that exist only to explain
old spellings.

Formal releases are minor baselines named `0.x.0`. After a release, `main`
advances to the next minor development identifier and keeps that version across
ordinary feature and maintenance commits. Patch releases are reserved for
important fixes to an already released minor line and are maintained from a
dedicated release branch when needed. A formal release is prepared by changing
the development identifier to its final `0.x.0` value and pushing the matching
`vX.Y.0` tag; the release workflow rejects other tag shapes.

## Before Contributing

Open a discussion before starting work on:

- language syntax or semantics;
- compiler architecture or crate boundaries;
- ABI, linking, code generation, or runtime model changes;
- broad test rewrites or documentation structure changes.

Small bug fixes, focused tests, typo fixes, and local documentation improvements
can usually be proposed directly.

## Change Routing

Start a change with the owner of the contract it modifies:

| Change | Primary owner |
| --- | --- |
| Language syntax or semantics | `language-spec.md` and the owning compiler phase |
| Compiler phase or crate boundary | `architecture.md` and the affected crate |
| ABI, layout, or runtime representation | `nia-abi.md` and the layout/backend owners |
| Build API or build execution | `lib/std/build/` and `crates/nia-build/` |
| Standard-library API | The owning `lib/std` module and [`lib/README.md`](../lib/README.md) |
| Repository audit or performance baseline | [`maintain/`](../maintain/) |

Keep detailed behavior and ownership rules beside their implementation owner;
use this document for contribution policy rather than as a second architecture
index.

## Code Standards

Compiler changes should follow the existing crate boundaries and local patterns.
Prefer small, reviewable changes over broad rewrites.
Architectural migrations must additionally follow the completion, ownership,
diagnostic, incremental, and evidence rules in
[compiler-maintenance.md](compiler-maintenance.md).

Before submitting compiler changes, run:

```sh
rustup toolchain install stable --component clippy --component rustfmt
cargo maintain check
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

On Windows, use an LLVM prefix built with the static MSVC CRT (`/MT`, or
`CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`). The repository's MSVC target
configuration enables Rust's `crt-static`, and the C++ bridge follows it.
LLVM archive linkage and CRT linkage are separate choices: `llvm-config
--shared-mode` does not establish which CRT an LLVM prefix uses.

Run the native gates from an x64 Developer PowerShell for Visual Studio (or a
shell initialized by `VsDevCmd.bat -arch=x64 -host_arch=x64`):

```powershell
cargo maintain check
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --no-default-features --target x86_64-pc-windows-msvc -- -D warnings
cargo test --workspace --no-default-features --target x86_64-pc-windows-msvc
```

An explicit `--target` keeps the CRT flags on target code, separate from host
build scripts. With the current `llvm-sys`, implicit host builds also apply those
flags to its build script, which then requests bundled static Windows system
libraries; `psapi.lib` can be installed yet unavailable in rustc's archive search
path. Specifying the target avoids needing global SDK `-Lnative` flags or 8.3
short paths. Target artifacts are under `target/x86_64-pc-windows-msvc/`.

Distinguish missing installations from shell setup when diagnosing Windows:

- A missing LLVM prefix or MSVC C++/Windows SDK component needs installation or
  selection of the correct installed prefix. Check the actual installation,
  not only whether `cl.exe` appears on `PATH`: Rust's native build tooling can
  discover installed MSVC tools itself.
- The developer shell supplies version-matched tool paths, `LIB`, and `INCLUDE`
  for external compiler/linker/archive commands. A normal shell lacking these
  variables does not by itself prove that the tools are absent.
- Existing processes retain their inherited environment. A user-level setting
  changed after the terminal or editor started is not evidence that its child
  commands received the setting. Inspect both scopes when they disagree.
- `RUSTFLAGS` and `CARGO_ENCODED_RUSTFLAGS` override the repository's target
  flags. Avoid persisting project CRT flags or versioned VS/SDK paths there;
  inspect stale overrides before attributing a failure to missing tools.

The maintained workflow initializes Visual Studio directly. No production
compiler code depends on a repository-local environment bootstrap script.

For a release compiler, build the pinned static LLVM prefix first and invoke
`tools/release/build.sh`. Do not use the default development feature for a
release artifact:

```sh
tools/llvm/build-static.sh
tools/release/build.sh
```

The repository intentionally follows the newest Rust stable toolchain rather
than pinning a release or promising an MSRV. Update stable before interpreting
local broad-gate results; new compiler and Clippy diagnostics are maintenance
work to fix at their source, not lints to suppress. The managed workflows print
the complete Rust, Cargo, Clippy, and rustfmt identity used for each run.

Libtest keeps its normal platform-selected concurrency. Independently
schedulable tests that create a complete compiler, LLVM, or build session share
a cross-process memory budget derived from effective CPU and memory limits. The
test budget is at most half of visible memory, nested build commands are charged
at twice the scheduling weight of ordinary compiler work, and new work waits
while system or cgroup available memory is under pressure. Machine categories
do not select separate test paths. A runtime command issued sequentially from
an active compiler/build test keeps its independent runtime process slot but
reuses the larger compiler memory reservation, so an exactly full low-memory
budget cannot deadlock itself after the compiler child has exited. WSL uses the
Linux VM's visible resources,
containers and constrained rental hosts use the tightest inherited cgroup
limit, and bare Linux hosts use system resources. If memory cannot be detected,
compiler-heavy tests run serially. This keeps the default command conservative
without private environment variables or a workspace-wide libtest thread
restriction.
Unit tests that construct a `nia-driver` `Driver` directly acquire the same
compiler-session boundary, including frontend/driver work that never launches
an external compiler process. A surrounding case session is reused instead of
being charged twice.

The `test` profile uses `opt-level = 1` because integration tests execute Nia as
a compiler, not merely as a command parser. Debug assertions and overflow
checks remain enabled. The `dev` and `test` profiles retain line tables instead
of full variable debug information so repeated compiler variants consume less
disk. Cargo does not garbage-collect old workspace target variants; a checkout
that predates this profile may need an explicit `cargo clean` once after useful
diagnostic artifacts have been preserved.

Test scratch storage must have an owner. CLI tests use
`nia_test_support::test_dir`, whose guard removes the complete tree on drop,
including failure unwinding. Do not return an unowned `std::path::PathBuf`
rooted in the system temporary directory from a shared Rust test helper.

Do not add `allow` or `expect` attributes to bypass lints. Fix the code instead.

Tests should reflect the current language. A removed spelling may be tested as a
normal rejection if it marks an important boundary, but it should not be treated
as a compatibility feature.

## Environment Variables

Nia's normal compiler behavior should be configured with CLI flags or typed API
options, not hidden environment variables. Environment variables that remain in
the tree are grouped here so their role stays explicit:

- `NIA_LINKER`: user-facing override for the executable linker. Windows
  overrides use the COFF `lld-link` argument contract; Linux overrides use the
  GNU contract.
- `NIA_LLD`: user-facing override for the `ld.lld` or `lld-link` executable
  selected by the platform linker flavor.
- `NO_COLOR`: standard terminal convention respected by CLI help rendering.
- `LLVM_SYS_231_PREFIX`: `llvm-sys` build-time override for non-standard LLVM
  installations.
- `NIA_TEST_RESOURCE_TRACE=1`: test-only diagnostics for permit waits and the
  peak process count/RSS observed for each managed command. It is useful when
  tuning a new host; it does not change libtest scheduling or resource limits.

Test-only fixture variables such as `NIA_TEST_ENV` may appear inside tests that
verify process environment behavior. Do not add new `NIA_*` variables without
documenting them in this section and deciding whether the feature should be a
CLI/API option instead.

## Review

Pull requests should be understandable by a human reviewer. Explain the design
reasoning for non-trivial changes, especially when behavior changes across
compiler phases.

The maintainer may ask for changes, split a proposal into smaller steps, or
close a proposal that does not fit the project direction.

## Licensing

Compiler implementation contributions are made under `GPL-3.0-or-later`, as
described in the repository [LICENSE.md](../LICENSE.md).

Documentation has no separate license grant unless its owning file states one
explicitly.
