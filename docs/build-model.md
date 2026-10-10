# Nia Build Model

This document records the maintained ownership and correctness boundaries of
the build system. Experimental sequencing belongs in bounded project notes or
Git history, while this file records only the active contract.

## Invocation boundaries

`nia build` resolves a `BuildRequest` into an invocation-local
`BuildInvocation`. The invocation owns physical package, build, cache, runner,
and handoff paths. The host runner compiles and executes `build.nia` only to
construct a graph through `std::build`; it never executes graph actions.

The runner emits a draft protocol payload. Decoding always performs the same
semantic freeze and validation as an in-process plan. The selected, validated
`BuildPlan` is then passed to the coordinator, which schedules and executes
the dependency closure.

## Current cache layers

* The runner cache stores a checked host executable. Its identity includes the
  generated runner source, build script, standard-library source tree,
  toolchain identity, host target, runner profile/mode, link-time policy, and
  build protocol. Artifact optimization is intentionally excluded because it
  belongs to artifact actions.
* The configure cache stores a frozen plan with its bounded observation
  manifest. Reuse requires the runner and invocation identities to match,
  every non-poisoned observation to recompute, and the plan to pass semantic
  decode validation again.
  Candidate scans are bounded; an oversized configure namespace is treated as
  a cache miss and falls back to runner configuration.
  Filesystem observations include content, metadata, directory entries, and
  explicit presence facts. A missing optional path is a stable `absent` fact,
  so creating or removing that path invalidates the configure record.
  Environment observations likewise distinguish an absent variable from an
  explicitly empty value and from each UTF-8 value.
* The action cache stores validated generated files and eligible external
  command results, plus references to compiler-owned products. Cache failures
  are explicit misses and cannot replace the ordinary execution path.
  External command policies distinguish uncacheable, declared-input,
  tool-and-input, and bounded full-environment modes. Full-environment mode
  rejects explicit overrides and skips publication when the inherited snapshot
  exceeds its size bound.
  External commands run with their standard input closed; the current command
  plan has no inherited-stdin escape hatch. A future file-backed stdin input
  must be added as a typed plan field and included in command identity before
  interactive or streamed command input can be supported.
* Compiler and linker product caches remain owned by `nia-driver` and
  `nia-linker`. `nia-build` records the action contract that refers to those
  products, but does not duplicate their formats.
* Successful test suites use an independent completion cache. Its identity
  includes the test executable, source input manifest, declared inputs,
  effective environment, toolchain, and action contract. It stores no output
  payload; failed or cancelled suites never publish a record, and cache
  failures fall back to running the suite.

The configure cache stores a validated plan together with the observations that
produced it. A later invocation may reuse that record only after its
observations match the current package root and the plan is decoded through
semantic validation. A missing, changed, poisoned, or corrupt record falls
back to runner configuration. The durable `build-plan.bin` handoff remains an
observation of the last completed configuration: it is atomically published
and never accepted as an unchecked execution shortcut.

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

Package metadata resolution records each existing dependency checkout root,
including roots whose edge was filtered from the selected package graph. When
the coordinator constructs a compiler request, it matches build-plan package
roots to those manifest edges and filters module-map roots by dependency
class. Runtime dependencies are available to ordinary artifact checks;
test-only and development dependencies require test mode; optional and lazy
dependencies require their explicit build selections; build-only dependencies
are not exposed to artifact compilation. Generated build roots and package
roots not declared in `nia.package` retain their existing plan behavior.

Package source identities walk only the selected roots, skip excluded and
symlinked entries, and stream file contents into their BLAKE3 fingerprints.
The walk is iterative and rejects source trees with more than one million
regular files before publishing an identity.

`nia build --explain` reports each declared dependency's selected-graph
resolution, the number of its modules represented in the frozen plan, and
whether the selected action closure owns or references that package. Module
counts describe plan declarations; closure use follows action and step package
ownership plus typed module, artifact, and package-path references. Canonical
checkout paths used to join metadata edges to plan roots remain invocation-local
and are not serialized in dependency entries. JSON dependency entries expose
these states as `resolved`, `loaded_modules`, and `used`;
`resolved_dependency_count` counts selected dependency edges, while
`resolved_count` counts selected package nodes including the root.

JSON inspection records are newline-delimited and deterministic. `build-plan`
contains canonical package, module, artifact, action, and step nodes with
targets and protocol epoch. `build-explanation` embeds that plan, configuration
observations, declared tools, package-graph decisions, and selected closure.
`action-cache` reports a stable decision object with status, miss reason,
component invalidations, declared outputs, and publication state. Dry runs emit
the same action records with `not_consulted` and `not_attempted` states and do
not consult or publish any action cache.

The runner draft carries bounded file-content, file-presence, directory,
environment, command, Git, and tool evidence. The host rechecks each reported
value before publishing a configure-cache record. Command-like observations
resolve the executable using the host `PATH`, execute its canonical path, and
include that path and a streamed content fingerprint in the observation
identity, alongside the request, status, and bounded output. They are not
represented as package-relative filesystem facts.

## Evolution rule

New configure observations, package identities, and dependency policies must
be represented as typed, versioned inputs at the appropriate boundary. They
must not make the durable plan an implicit source of execution truth, conflate
package metadata with ABI/type metadata, or bypass freeze-time validation.
