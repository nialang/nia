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

Before submitting compiler changes, run:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Libtest keeps its normal platform-selected concurrency. Tests that create a
complete compiler, LLVM, or build session additionally share a cross-process
memory budget derived from effective CPU and memory limits. The test budget is
at most half of visible memory, build commands are charged at twice the weight
of ordinary compiler work, and new work waits while system or cgroup available
memory is under pressure. Machine categories do not select separate test paths:
WSL uses the Linux VM's visible resources, containers and constrained rental
hosts use the tightest inherited cgroup limit, and bare Linux hosts use system
resources. If memory cannot be detected, compiler-heavy tests run serially.
This keeps the default command conservative without private environment
variables or a workspace-wide libtest thread restriction.

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
- `NIA_QUERY_THREADS`: developer tuning knob for query worker parallelism.
- `NIA_DEBUG_EXEC_REACHABILITY`: developer debug output for executable
  reachability.
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

Documentation licensing is intentionally separate from the compiler
implementation license unless a future document explicitly says otherwise.
