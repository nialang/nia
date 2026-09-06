# Compiler Maintenance

This document defines the principles used to evolve the compiler without
weakening its architecture or losing observable correctness. It is a short
maintenance contract, not a second architecture or implementation manual.

Detailed ownership and behavior live with their owners:

- [architecture.md](architecture.md) describes compiler phases, products, and
  crate boundaries.
- [nia-abi.md](nia-abi.md) describes representation, layout, and ABI rules.
- [`lib/README.md`](../lib/README.md) describes standard-library ownership and
  runtime-facing contracts.
- [`crates/nia-build/README.md`](../crates/nia-build/README.md) describes build
  plans, execution, caching, and publication.
- [`maintain/performance.md`](../maintain/performance.md) describes workload
  baselines and performance evidence.

## Ownership And Boundaries

Every durable fact has one owner and one source of truth. Ownership follows
the lifetime and mutation authority of the fact, not the crate that happens to
mention it or the wrapper type used to store it. A product may be consumed by
many phases, but its owner defines its identity, validation, storage policy,
and publication boundary.

An architectural change is complete when the new contract is the only active
contract. Remove the obsolete entry point, identity, fallback, duplicate
source of truth, and old/new dual path. A compatibility adapter is appropriate
only at one explicit migration boundary with a planned end; it is not a
permanent second API.

Keep phase boundaries visible in the data flow. Pass typed identities and
validated immutable products between phases, and give each consumer only the
capabilities it needs. Driver code coordinates requests; it does not recreate
semantic fixed points already owned by compiler queries or reinterpret an
earlier phase's source representation.

Identity has two layers. Session-local handles may be compact and efficient,
but anything persisted, compared across processes, or exposed in a stable
artifact needs an explicit canonical representation. Compatibility, ABI,
persisted-format, and cache namespaces use the shared registry and owner-local
fingerprint domains described by the relevant subsystem.

When a proposed aggregate, shared cache, or new crate boundary has no clear
consumer or ownership authority, keep the narrower product and defer the
abstraction. Size, file count, and crate count are useful investigation
signals, not design goals.

## Failure And Correctness

Expected failures use typed results and the project's diagnostic channels.
User-facing errors remain distinguishable from internal compiler errors (ICEs),
and an ICE reaches the designated boundary with its context intact. A panic is
not an alternate user-error transport.

Diagnostics and other persistent products use stable source identity and
validated spans. Session-local handles, allocation addresses, and debug output
are not persistence formats. Encoders and decoders validate their complete
identity, schema, bounds, and trailing input; malformed or incompatible data
is retired rather than interpreted optimistically.

Fallible operations are transactional at their ownership boundary. A failure
must leave every still-live resource reachable by its owner, preserve the
primary error, and provide a defined retry or cleanup path. Cleanup attempts
independent resources even after one release fails. A partially completed
operation is not published as an empty or reusable value merely because its
logical length is zero.

Semantic checks and later lowering must agree on the same accepted value set.
Recovery values, unchecked metadata, and host-language truncation are not
substitutes for a source- or target-level contract. When a rule changes an
error boundary, cover both the ordinary diagnostic result and the intended
internal-invariant boundary with focused tests.

## Incremental And Concurrent Execution

The typed query and fact graph is the single dependency and invalidation source.
Mutable input goes through dependency recording, and source revisions retire
obsolete products only after active work has quiesced. A persistent cache is an
additional storage layer, not a replacement for in-memory dependency tracking.

Incremental recomputation must be equivalent to clean recomputation for the
same inputs. Tests for invalidation should exercise edit sequences and compare
observable products, rather than asserting only that a cache was hit. A
persistent product is justified by a measured dependency cut and a stable
cross-session identity, not merely because its value can be serialized.

Parallel work uses the session-owned executor and explicit resource accounting.
Each task has a clear partition identity, cancellation behavior, ownership of
mutable state, and immutable shared context. Results merge deterministically,
independent of completion order. CPU and native-memory limits are shared by
the compiler's workers and inherited build tools; a worker count alone is not
a memory policy.

Tests use the same public compiler, build, and LLVM contracts as production.
The test harness may reserve a bounded compiler or build session, but it does
not change production semantics. Resource limits derive from the effective
host and cgroup capabilities, with conservative behavior when a metric is
unavailable.

## Evidence And Acceptance

Acceptance is defined before a broad migration begins. It describes observable
behavior, ownership, and failure boundaries, not only the existence of a new
type or API.

Validate from narrow to broad:

1. owner-level tests for the changed contract;
2. direct consumer and cross-phase tests;
3. workspace checks, formatting, and strict Clippy;
4. relevant end-to-end, build, runtime, or performance workflows.

Completion requires the superseded model to be absent, focused tests to pass,
and the affected end-to-end path to run. Structural searches are appropriate
when removal of an old symbol, environment switch, fallback, or duplicate
source of truth is part of the acceptance criteria.

Performance conclusions use the complete workload path, repeated samples, and
compatible resource identity. Deterministic query, cache, allocation, and
codegen counters should be understood before noisy wall-time changes. Hosted
infrastructure, artifact publication, cache reuse, linking, and cross-run
comparisons require evidence from the actual managed workflow; a local config
check cannot stand in for that evidence. See
[`maintain/performance.md`](../maintain/performance.md) for the measurement
protocol.

Failed experiments are useful evidence while they are being evaluated. Once a
direction is rejected, remove its production schemas, readers, counters,
adapters, and fallback paths. Keep the explanation in the bounded project
notes or Git history, not in dormant implementation branches.

## Tests And Review

Choose a test shape that makes the contract visible. Data-driven suites are
well suited to repeated compiler matrices, resource classes, edits,
diagnostics, and output comparisons. Hand-written tests remain appropriate for
dynamic repository and toolchain contracts, process and filesystem behavior,
allocator and container semantics, startup/runtime boundaries, and other cases
where opaque metadata would hide the behavior under review.

Review changes at the boundary they affect. A file or crate split is justified
by stable data or algorithm ownership and a manageable collaboration surface;
a merge requires the same review of consumers, dependency direction, public
types, and cycle risk. Add a shared abstraction only when it removes a real
ownership or dependency problem.

Focused tests should run before broad gates. Compiler, LLVM, build, and
generated-process tests use the repository's resource-accounted harness, and
multi-command sessions retain one explicit owner for their temporary state.
The owning crate or subsystem documents any additional invariant that a test
must enforce.

## Documentation Lifecycle

Stable implementation rules belong beside the implementation that owns them.
When a change establishes a durable architectural or API rule, update that
owner document in the same delivery batch. Keep this document limited to
principles that apply across compiler subsystems.

Temporary sequencing, completion notes, rejected designs, and dated
measurements belong in a bounded roadmap, generated evidence, or Git history.
When a roadmap closes, move any still-valid rule into its stable owner document
and remove the progress log from the active contract. A completed roadmap may
be deleted once its acceptance is closed, its architecture is documented, and
its historical evidence remains recoverable in Git.

Build-system and standard-library work follows the same boundaries but has its
own owners. Consult [`crates/nia-build/README.md`](../crates/nia-build/README.md),
[`lib/README.md`](../lib/README.md), and the relevant `lib/std` or Rust module
before changing those contracts. Compiler changes required by that work still
use the result, diagnostic, identity, and evidence rules above.
