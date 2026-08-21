# Compiler Maintenance Contract

Status: normative compiler maintenance policy

This document records the engineering discipline that must survive individual
compiler roadmaps. It complements the implemented architecture described in
[architecture.md](architecture.md), the repository rules in
[project-conventions.md](project-conventions.md), and the measurement contract
in [performance.md](performance.md).

Temporary roadmaps may define sequencing and acceptance for a bounded project.
They are not permanent architecture documents. When a roadmap closes, durable
rules belong here or in the relevant reference document; progress logs remain
available through Git history.

## 1. Root-Cause Changes

Compiler maintenance must repair the owning abstraction and its complete data
flow, not only the first failing call site.

- A migration is complete only when the obsolete entry point, identity,
  fallback, duplicate source of truth, and old/new dual path are physically
  removed.
- A compatibility adapter may exist only at one explicit, advancing migration
  boundary. Its deletion belongs to the same bounded project; it must not become
  a permanent public API.
- A large call surface is evidence that a contract crosses many owners. It is
  not a reason to weaken the target architecture or preserve the old contract.
- Do not fix ownership problems with broad `Clone`, `Arc`, interior mutability,
  side stores, or service-locator callbacks unless that is the intended
  long-term ownership model.
- Do not add driver special cases, hidden environment switches, cache exceptions,
  or test-only production behavior to make an architectural migration appear
  complete.

The preferred sequence is to identify the owner, the consumers, the stable
identity, and the only source of truth; change that contract; follow compiler
errors through every affected consumer; then delete the superseded path.

## 2. Failure And Diagnostic Discipline

Invalid source, unsupported input, query cycles, I/O failures, cache corruption,
and other expected failures use explicit result and diagnostic channels.

- Query failures propagate through `QueryResult` or another typed result. Panic
  and unwind are not ordinary error transport.
- User-facing compiler failures are registered in the compiler or loader
  diagnostic store and are carried through stable bundle handles where query
  products need to refer to them.
- Semantic query values should not own repeated diagnostic vectors. Algorithms
  may use short-lived local buffers while constructing the canonical bundle.
- Panic is reserved for genuine internal invariants and ICEs. It must reach the
  designated ICE boundary rather than being caught and reinterpreted as a user
  error.
- Persistent diagnostic data must use stable source identity and validated
  spans. It must not serialize session-local source, module, revision, or bundle
  handles.

An error-path change is not accepted until tests cover both the ordinary
diagnostic result and the invariant/ICE boundary it intentionally leaves.

## 3. Identity, Ownership, And Products

Hot-path identities are typed, compact, and session-local. Persistent identity
is a separate canonical representation.

Cross-component release, toolchain, ABI, persisted-format, and cache-namespace
identities are owned by the dependency-free `nia-compat` registry. Product
owners retain their encoders, decoders, bounds, checksums, and corruption
policy, but must consume registry values directly rather than exporting aliases.
`lib/toolchain.meta` is generated registry data, and the workspace package
version is the only release-version input.

Fingerprint domains are owner-local `nia_query::FingerprintDomain` constants,
not raw builder strings or a central list detached from the hashed inputs. Each
distinct input contract has one domain declaration; deliberate reuse shares
that declaration. Changing the encoded inputs or their meaning requires a
domain-version increment, and the compatibility audit rejects malformed,
duplicated, or inline production domains.

A public-version reset, compatibility epoch, or development-schema renumbering
requires its own release proposal after audit and representative project
testing. The registry preserves current values until that proposal is accepted;
it does not make a reset implicit.

- Never serialize a session-local index or infer stable identity from allocation
  order, module number, query slot, pointer value, or debug formatting.
- Reclaiming stores use owner/index/generation or an equivalent stale-handle
  boundary. Append-only session stores never reuse an index for a different
  meaning.
- A query product has one clear owner and a storage policy chosen by the query
  declaration, not by a caller selecting between owned and shared APIs.
- Large immutable products stay cache-owned and are borrowed or referenced.
  Products that must be optimized or consumed uniquely move through an owned
  query boundary instead of being deeply cloned.
- Backend products contain backend facts and stable semantic handles, not
  snapshots of semantic stores. Compiler phases receive only the capabilities
  required for their work.
- Trait obligations remain complete across phase and consumer boundaries. The
  self type, trait identity, type and const arguments, and associated-type
  bindings are one semantic product; candidate filters and final validators
  must not silently reduce it to a bare trait goal. Consumers of impl-signature
  products check exact argument arity before pairwise matching so malformed or
  stale upstream facts fail closed instead of benefiting from truncated `zip`
  comparisons. Inference probes clone their substitution state and commit it
  only after the complete impl header and associated binding set matches; a
  failed nested binding must not leak a partial generic inference. Associated
  binding vectors are unordered semantic sets: comparisons consume every
  candidate at most once and backtrack over compatible keys, since a greedy
  first match can reject a later valid permutation. This applies to fast shape
  filters and specialization ordering as well as final impl selection; a
  permissive prefilter is still unsound when it changes which candidate wins.
  Backend extension instantiation and backend type equivalence preserve this
  rule so code generation cannot select a weaker instance than body checking.
- Supertrait declarations are persisted as complete trait obligations too. Any
  associated-type binding attached to a supertrait travels through collection,
  type-root discovery, cache encoding, body assumptions, impl validation, and
  trait-object object-safety/method/upcast consumers. A child object inherits
  declared supertrait equalities; callers need not repeat them in its object
  spelling, while unrelated or incompatible parent bindings remain rejected.
  Adding such a field requires a persisted-format version change so old entries
  fail closed instead of being read with a shifted positional layout.
- Aggregate whole-program products require evidence that a real consumer needs
  the aggregate. Prefer item-, body-, module-, or codegen-unit-owned products
  when they preserve the dependency boundary.

Ownership is judged by lifetime and mutation authority, not by whether a type is
wrapped in `Arc` or stored in a different crate.

## 4. Query And Incremental Correctness

The typed query/fact graph is the only dependency and invalidation truth source.

- Mutable input cannot bypass dependency recording.
- Driver orchestration must not reproduce a semantic fixed point already owned
  by compiler queries.
- A source update retires obsolete current-revision entries after quiescence.
  The current cache, dependency graph, slot tables, and locators must not retain
  revision history as an accidental compatibility feature.
- Incremental results must remain equivalent to clean recomputation. Randomized
  edit sequences and clean/incremental differential tests are preferred for
  cross-query invalidation contracts.
- Representative build acceptance must make its process and module boundaries
  machine-checkable. Each incremental and independent-clean state runs in a
  fresh compiler process, records that process identity, and compares at least
  one executable whose source graph contains more than one module.
- Concurrent slot, cycle, invalidation, and red-green state machines require
  deterministic tests; model or race-focused tests are required where ordinary
  examples cannot exercise the transition safely.
- Query wait-for edges are temporary state, not dependency history. Cycle
  detection must release every edge/frame on both normal wait completion and
  cycle failure, and retirement admission must be reopened by RAII after a
  callback panic. Build schedulers likewise keep cancellation tied to canonical
  action position, wait for the active wave, and never dispatch dependents after
  a failure.
- Cache keys include schema, compiler, target, options, stable input identity,
  and domain separation appropriate to the product. Entries repeat and validate
  their identity, reject truncation and trailing data, and retire corruption
  rather than attempting a compatibility decode.
- Persistent cache retirement and publication for one content-addressed path
  use the same mutation lock. A reader may remove a corrupt record only when the
  bounded bytes or oversized state it observed still occupy that path; it must
  preserve a valid replacement published before retirement acquires the lock.
  Immutable publishers revalidate an existing winner under that lock rather
  than silently overwriting it.
- Verification mode recomputes the product and replaces a well-formed but
  semantically stale artifact. A cache hit is not proof of correctness.

Do not persist a query merely because its value is serializable. A persistent
product is worthwhile only when it cuts a measured dependency chain without
smuggling revision-owned state across sessions.

## 5. Concurrency And Resources

Parallelism follows ownership and resource accounting.

- Query providers do not create unmanaged operating-system threads. They submit
  work to the session-owned persistent executor.
- All compiler batches share the process CPU budget, including Cargo or GNU Make
  jobserver capacity when inherited.
- LLVM work additionally obeys process-wide memory backpressure. Worker count is
  not a substitute for a memory budget.
- Parallel tasks own their mutable result and borrow an immutable `Send + Sync`
  context. Results merge in a deterministic order independent of completion
  order.
- Tests use the same public compiler and LLVM contracts as production. The
  integration harness may reserve an explicit compiler or build resource
  session, but unit tests must not change production API semantics.
- Test resources derive from effective CPU, memory, and cgroup limits. Hidden
  machine categories and undocumented limit variables are forbidden.

Wrapping an existing aggregate loop in a parallel iterator does not establish a
task model. Partition identity, readiness, ownership, cancellation, memory
limits, and deterministic merge behavior must be explicit first.

## 6. Evidence And Acceptance

Every non-trivial compiler project defines acceptance before broad migration.
Acceptance must describe observable architecture and behavior, not only that new
types or APIs exist.

- Completion requires the old model to be absent, relevant structural searches
  to be clean, focused tests to pass, and the affected end-to-end path to run.
- Validate from narrow to broad: owner tests, consumer tests, workspace check,
  strict all-target/all-feature Clippy, formatting, then relevant integration or
  performance gates.
- External infrastructure acceptance requires external evidence. A local config
  check cannot substitute for an actual hosted run, artifact download, cache
  reuse, linker execution, or cross-run comparison.
- Performance conclusions use the complete workload path, repeated samples, and
  compatible resource identity. Deterministic query, codegen-unit, cache, and
  allocation counters should be interpreted before noisy wall time.
- Never report progress from an isolated cache hit if another consumer
  immediately rebuilds the same raw dependency chain. Measure the end-to-end
  execution cut.
- Failed experiments are valuable evidence, but rejected schemas, readers,
  counters, adapters, and fallback paths are removed completely. Preserve the
  lesson in documentation or commit history, not dormant production code.

Changes should be grouped into meaningful dependency-complete batches rather
than one-symbol commits. Several coherent implementation waves may remain
together while that batch is still advancing; do not create commits merely to
snapshot partial movement. Once the delivery batch passes its relevant gates,
commit it with a descriptive `feat: ...` subject before reporting or handing off
the work, and do not carry it into an unrelated batch. Do not mix unrelated
cleanup into the commit merely because a broad validation command exposed it.
Temporary execution progress belongs in its bounded project roadmap; durable
ownership, validation, and maintenance lessons must be moved into the relevant
stable document as part of the batch that establishes them.

## 7. Boundary And Test Reviews

Source line count, file count, crate count, and test count are investigation
signals, not architecture goals.

- Split a file when it exposes stable algorithm or data ownership with a narrow
  collaboration surface. Do not move code only to reduce a line count.
- Merge a crate only after reviewing production consumers, dependency direction,
  stable public types, and cycle risk. A small shared leaf crate can be the
  correct boundary.
- Data-driven suites should express repeated compiler matrices, resource class,
  inputs, edits, expected diagnostics, and outputs. They should not hide complex
  runtime or standard-library behavior inside opaque metadata.
- Keep hand-written tests for dynamic repository/toolchain contracts and for
  process, filesystem, I/O, allocator, container, startup, runtime, and standard
  library semantics when those behaviors are the subject of the test.
- Tests and documentation describe the current contract. Historical behavior
  belongs in Git, not compatibility fixtures or stale debug switches.

## 8. Build And Standard Library Work

Compiler architecture is an input to build-system and standard-library work,
but it is not a substitute for their design.

Before a substantial build or standard-library project begins, create a
separate bounded design and acceptance document covering its own ownership,
error model, cache and reproducibility rules, runtime/ABI interaction, public
surface, migration boundary, tests, and performance evidence. Do not append
build or standard-library feature work to an already closed compiler roadmap or
reuse compiler completion percentages to describe it.

Durable build ownership and API decisions live beside their owners in
[`crates/nia-build/README.md`](../crates/nia-build/README.md), the relevant Rust
module, and [`lib/std/build.nia`](../lib/std/build.nia). Other standard-library
decisions likewise live in [`lib/README.md`](../lib/README.md) and the relevant
`lib/std` facade or module. Current std APIs are not retained merely because
they made the bootstrap run, and full build sessions remain resource-accounted
integration work rather than ordinary unit tests.

Any compiler changes required by that work still follow this maintenance
contract. Ordinary build or standard-library errors must continue through the
project's explicit diagnostic and result systems rather than introducing panic
paths or private control channels.

## 9. Roadmap Retirement

A roadmap can be deleted when all of the following are true:

1. Every stated acceptance item is closed with code, test, or external evidence.
2. Implemented architecture is documented in its stable reference document.
3. Durable maintenance rules and lessons have been migrated out of progress
   notes.
4. Follow-on projects with materially different scope are explicitly separated.
5. Git history retains the detailed sequence, rejected experiments, and
   intermediate measurements.

Deleting a completed roadmap prevents historical percentages and temporary
sequencing from becoming false current policy. It does not erase the work or
its evidence.
