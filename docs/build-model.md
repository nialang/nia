# Nia Build Model

This document records the maintained ownership and correctness boundaries of
the build system. It is a tracked contract document; experimental migration
plans remain local in `docs/build-roadmap.md`.

## Invocation boundaries

`nia build` resolves a `BuildRequest` into an invocation-local
`BuildInvocation`. The invocation owns physical package, build, cache, runner,
and handoff paths. The host runner compiles and executes `build.nia` only to
construct a graph through `std::build`; it never executes graph actions.

The runner emits a draft protocol payload. Decoding always performs the same
semantic freeze and validation as an in-process plan. The selected, validated
`BuildPlan` is then passed to the coordinator, which schedules and executes
the dependency closure.

## Three current cache layers

* The runner cache stores a checked host executable. Its identity includes the
  generated runner source, build script, standard-library source tree,
  toolchain identity, host target, runner profile/mode, link-time policy, and
  build protocol. Artifact optimization is intentionally excluded because it
  belongs to artifact actions.
* The action cache stores validated generated files and eligible external
  command results, plus references to compiler-owned products. Cache failures
  are explicit misses and cannot replace the ordinary execution path.
* Compiler and linker product caches remain owned by `nia-driver` and
  `nia-linker`. `nia-build` records the action contract that refers to those
  products, but does not duplicate their formats.

The durable `build-plan.bin` handoff is an observation of the last completed
configuration. It is atomically published and decoded through semantic
validation, but every invocation configures and freezes a fresh plan. It is
never accepted as an execution shortcut.

## Plan invariants

`BuildPlan` contains typed package, module, artifact, action, and step keys.
Logical paths carry an explicit package/build/cache/toolchain/artifact root;
physical paths are resolved for the current invocation. Freeze and decode
reject duplicate identities, unresolved handles, cycles, target mismatches,
conflicting outputs, invalid names, and incomplete producer closure.

The coordinator executes only the validated selected closure. Host and
artifact targets are checked before scheduling. Outputs are declared and
published transactionally, with cross-process locking for equal logical
destinations and recoverable staged state.

## Evolution rule

New configure observations, package identities, and dependency policies must
be represented as typed, versioned inputs at the appropriate boundary. They
must not make the durable plan an implicit source of execution truth, conflate
package metadata with ABI/type metadata, or bypass freeze-time validation.
