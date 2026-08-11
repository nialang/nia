# Nia Maintenance

This directory owns the repository maintenance subsystem. Its `nia-maintain`
Rust binary is a normal workspace member: the root `Cargo.lock`, rustfmt,
Clippy, and workspace tests own its dependency and correctness boundaries. The
repository Cargo alias runs it without a second language runtime or package
manager.

Commands are grouped by purpose:

```text
cargo maintain audit compatibility
cargo maintain audit std-build-host
cargo maintain report crate-boundaries
cargo maintain baseline compiler
cargo maintain baseline compare <baseline.json> <candidate.json>
cargo maintain baseline build
cargo maintain check
```

`audit` commands enforce repository invariants and return no report on success.
`report` commands produce deterministic maintainer evidence without changing
the repository. `baseline` commands own repeatable measurements and their
machine-readable schemas. `check` runs every fast repository audit; Cargo owns
compilation, Clippy, and tests, and the command does not run compiler or build
baselines.

Fixtures belong in `maintain/fixtures/`, integration contracts in
`maintain/tests/`, and implementation tests beside their owning Rust
modules. Do not add another top-level executable; add a typed command to
`nia-maintain` whose domain module can be tested directly.
