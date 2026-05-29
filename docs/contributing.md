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

Do not add `allow` or `expect` attributes to bypass lints. Fix the code instead.

Tests should reflect the current language. A removed spelling may be tested as a
normal rejection if it marks an important boundary, but it should not be treated
as a compatibility feature.

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
