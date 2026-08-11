# Contributing to Nia

Nia is a maintainer-led personal project. Contributions, issues, and design
discussions are welcome, but final language and implementation decisions rest
with the project maintainer.

This project values a small coherent language more than consensus-driven feature
growth. A technically valid change may still be rejected if it does not fit the
direction of Nia.

## Project Status

Nia is pre-1.0 and under active design. Compatibility with earlier experimental
syntax is not a goal. Removed behavior should not receive migration paths,
compatibility tests, or diagnostics that exist only to explain old spellings.

## Before Contributing

Open a discussion before starting work on:

- language syntax or semantics;
- compiler architecture or crate boundaries;
- ABI, linking, code generation, or runtime model changes;
- broad test rewrites or documentation structure changes.

Small bug fixes, focused tests, typo fixes, and local documentation improvements
can usually be proposed directly.

## Code Standards

Compiler changes should follow the existing crate boundaries and local patterns.
Prefer small, reviewable changes over broad rewrites.
Architectural migrations must additionally follow the completion, ownership,
diagnostic, incremental, and evidence rules in
[compiler-maintenance.md](compiler-maintenance.md).

Before submitting compiler changes, run:

```sh
rustup toolchain install stable --component clippy --component rustfmt
npm ci --prefix tools --ignore-scripts
python3 -m tools check
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
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
do not select separate test paths: WSL uses the Linux VM's visible resources,
containers and constrained rental hosts use the tightest inherited cgroup
limit, and bare Linux hosts use system resources. If memory cannot be detected,
compiler-heavy tests run serially. This keeps the default command conservative
without private environment variables or a workspace-wide libtest thread
restriction.

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

- `NIA_LINKER`: user-facing override for the executable linker.
- `NIA_LLD`: user-facing override for the `ld.lld` executable used by the lld
  linker flavor.
- `NO_COLOR`: standard terminal convention respected by CLI help rendering.
- `LLVM_SYS_221_PREFIX`: `llvm-sys` build-time override for non-standard LLVM
  installations.

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
