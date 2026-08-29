# Compiler Audit And Soundness Roadmap

Status: active temporary roadmap

This roadmap governs the pre-0.1.0 whole-repository audit. It is intentionally
temporary: the implementation, tests, and durable maintenance rules belong in
their owning crates and stable documentation. Keep this file while work is in
progress so that architectural debt, review coverage, and residual risk remain
visible. Delete it only after every acceptance item in Section 10 is closed.

The roadmap is an audit and hardening project, not a version-release plan. It
does not change Nia's planned `0.1.0` milestone or introduce a compatibility
promise for pre-0.1.0 syntax and behavior.

## 1. Objectives

The project must leave Nia with:

- sound phase boundaries from source through LLVM and executable output;
- one owner for each identity, substitution, diagnostic, cache product, and
  execution policy;
- implementation files split when they expose independent algorithms or data
  ownership, without cosmetic fragmentation;
- complete propagation of type, const, lifetime, layout, and provenance facts;
- tests that exercise both ordinary behavior and invariant/error boundaries;
- professional Rustdoc and focused comments for non-obvious algorithms;
- explicit residual-risk records for behavior that cannot yet be verified;
- clean, reproducible, resource-controlled tests on constrained and larger
  machines.

The project does not preserve removed syntax or old implementation paths merely
for historical compatibility. Any compatibility adapter must be a bounded
migration boundary with a deletion condition.

## 2. Working Rules

Each phase follows the same loop:

1. Map the owner, consumers, identity, and current tests before editing.
2. Review the complete data flow, not only the first failing call site.
3. Repair the owning abstraction and remove obsolete duplicate paths.
4. Add focused unit/property/regression tests and the relevant end-to-end test.
5. Add Rustdoc or a short algorithm comment where a reviewer would otherwise
   need to reconstruct an invariant from several functions.
6. Run the narrow owner tests, affected consumer tests, workspace check,
   formatting, and the applicable integration/resource gate.
7. Update this roadmap and commit a coherent batch using Conventional Commit
   style before moving to an unrelated phase.

Tests must use libtest's normal concurrency and the shared resource accounting
from `nia-test-support`. Do not make `--test-threads=1` the acceptance path.

## 3. Coverage Baseline

The initial inventory found approximately 639 Rust files and 321k Rust code
lines. Tokei reports only about 1.1k Rust comment lines; even including the
embedded documentation bucket, the repository is below one percent comments.
This is an investigation signal, not a target ratio. Comments are required
where they explain invariants, not where they restate syntax.

The following areas have received recent focused changes, but are **not** yet
considered fully audited:

- const-generic identity propagation through semantic facts, monomorphization,
  trait dispatch, backend lowering, and executable reachability;
- pattern analysis, nominal patterns, arrays, tuple structs, and `match`;
- closure syntax and initial closure type inference;
- selected const/static initialization paths;
- query cycle detection and test-process resource accounting.

## 4. Phase A: Type, Trait, And Body Soundness (P0)

Owner crates: `nia-body-check`, `nia-trait-solve`, `nia-program-signatures`,
`nia-type-resolve`, `nia-type-lower`, `nia-type-normalize`, `nia-value-resolve`,
`nia-local-resolve`.

Primary files to audit first:

- `nia-body-check/src/inference.rs`;
- `nia-body-check/src/type_support.rs`;
- `nia-body-check/src/projection_obligations.rs`;
- `nia-body-check/src/calls/`;
- `nia-body-check/src/trait_objects.rs`;
- `nia-body-check/src/aggregates.rs`;
- `nia-trait-solve/src/`;
- `nia-program-signatures/src/analysis/`.

Required review topics:

- HM-style constraint generation, unification, occurs checks, rollback, and
  inference failure recovery;
- interleaved type and const generic parameter ordering at every substitution
  boundary;
- trait candidate filtering, specialization ordering, ambiguity, negative
  results, and visibility/module ownership;
- where predicates, supertraits, projection normalization, associated types,
  associated values, and recursive obligations;
- trait-object construction, upcasting, vtable identity, and dynamic calls;
- contextual inference for arrays, aggregates, closures, calls, and returns;
- pattern typing and binding-mode propagation;
- place typing, mutability, pointer coercions, temporary lifetimes, and error
  union/optional propagation;
- diagnostics for each rejected candidate and each unresolved obligation.

Architecture actions:

- split files only at stable ownership boundaries such as inference state,
  projection solving, aggregate typing, and call resolution;
- make substitution helpers canonical and remove local reimplementations;
- document the constraint lattice, candidate ordering, and cycle termination;
- add tests for repeated generic parameters, mixed type/const parameters,
  nested closures, projection cycles, ambiguous impls, and cross-module traits.

Acceptance:

- clean and incremental body/signature checks agree on all added matrices;
- no trait or projection path silently drops type, const, or module context;
- diagnostics identify the actual failed obligation without changing valid
  program acceptance;
- owner tests plus `nia-compiler-query` generic/trait suites pass.

## 5. Phase B: Layout, ABI, Backend IR, And LLVM Safety (P0)

Owner crates: `nia-layout`, `nia-abi-check`, `nia-backend-ir`,
`nia-backend-lower`, `nia-function-ir`, `nia-function-lower`, `nia-function-opt`,
`nia-llvm`, `nia-codegen-llvm`, `nia-mangle`.

Required review topics:

- layout and alignment for primitives, arrays, tuples, nominal structs,
  tuple structs, enums, unions, zero-sized values, closures, and trait objects;
- overflow, target pointer width, endianness assumptions, field offsets, and
  discriminant representation;
- ABI classification for direct/indirect parameters, hidden return storage,
  variadics, extern functions, function pointers, and dynamic trait calls;
- Body IR and backend IR validation, terminator completeness, unreachable
  blocks, ownership of promoted allocations, and static data;
- optimizer preservation of evaluation order, side effects, volatile/atomic
  operations, inline assembly, and defer bodies;
- LLVM handle lifetime, context ownership, null returns, disposal, target data
  layout, object buffers, and typed GEP/cast preconditions;
- deterministic codegen indexes, fingerprints, mangling, vtable emission, and
  incremental object reuse.

Mandatory LLVM boundary tests must cover malformed/empty returns, target-data
  failure, zero-sized values, aggregate returns, vtable calls, and at least two
  pointer widths. Unsafe wrapper preconditions belong in Rustdoc at the wrapper,
  not only in higher-level code.

Acceptance:

- every backend entry point has a documented invariant or explicit `Result`;
- backend validation rejects malformed IR before LLVM receives it;
- representative programs produce identical ABI/codegen results under clean
  and incremental paths;
- `nia-codegen-llvm` and linker integration tests pass on the supported target.

## 6. Phase C: Const, Static, Closure, Flow, And IR Semantics (P0)

Owner crates: `nia-const-ir`, `nia-const-eval`, `nia-const-check`,
`nia-static-check`, `nia-static-ir`, `nia-closure-check`, `nia-flow-check`,
`nia-body-ir`.

Required review topics:

- distinction between Nia `const` compile-time evaluation and `static` storage;
- evaluation frames, recursion, assignment writeback, error state rollback,
  overflow, target-dependent values, and const-generic expressions;
- pointer provenance, address-taking, promotion, local static ownership,
  initialization order, dependency cycles, and nested declarations;
- compile-time rejection of IO/asm/runtime-only operations;
- closure escape fixpoints, captured addresses, callable views, stack-backed
  storage, recursive/mutually recursive closures, and error provenance;
- flow reachability, match/if-pattern coverage, defer control flow, and exact
  typed Body IR invariants.

Acceptance:

- static and const tests explicitly cover the boundaries above, not only happy
  paths;
- closure safety has direct unit tests in addition to compiler-query tests;
- every Body IR variant is covered by walking, validation, lowering, and
  reachability tests;
- no optimizer or const evaluator drops an observable side effect.

## 7. Phase D: Query, Cache, Loader, Import, And Build State (P1)

Owner crates: `nia-query`, `nia-compiler-query`, `nia-loader-query`,
`nia-imports`, `nia-public-surface`, `nia-provider-summary`, `nia-build`,
`nia-source`, `nia-toolchain`, `nia-linker`, `nia-driver`, `nia-cli`.

Required review topics:

- query slot lifecycle, nested execution, cycle detection, cancellation,
  invalidation, retirement, and executor shutdown;
- clean/incremental red-green equivalence and randomized edit sequences;
- persistent cache schema, stable identity, fingerprints, corruption, bounds,
  truncation/trailing bytes, and verification replacement;
- module graph revisions, forks, facades, reexports, provider activation, and
  path-independent identities;
- build-plan canonical encoding, action cache keys, process groups, output
  recovery, stale locks, output races, and cancellation;
- command paths, target/toolchain selection, environment propagation, linker
  failures, and user-facing diagnostics;
- memory/CPU/jobserver accounting under low memory, cgroups, WSL, and nested
  compiler/build/runtime processes.

Acceptance:

- all persisted products have roundtrip, corruption, stale-identity, and
  replacement tests;
- concurrent query/build state machines have deterministic race-focused tests;
- clean and incremental compiler outputs are equivalent for representative
  multi-module and cross-process workloads;
- low-memory tests demonstrate bounded behavior without requiring serialized
  libtest execution.

## 8. Phase E: Frontend, Identity, Diagnostics, And Structural Boundaries (P1)

Owner crates: `nia-lexer`, `nia-syntax`, `nia-ast`, `nia-ast-walk`, `nia-parser`,
`nia-item-tree`, `nia-defs`, `nia-ids`, `nia-node-id`, `nia-symbol`,
`nia-symbol-table`, `nia-span`, `nia-diagnostic`, `nia-ice`.

Required review topics:

- parser recovery, speculative rollback, delimiter errors, precedence,
  closure/array/aggregate ambiguity, and stable node origins;
- lexical edge cases, UTF-8 boundaries, comments, literals, and source spans;
- stable identity allocation, generation/stale-handle behavior, symbol
  collision handling, and cross-revision source mapping;
- diagnostic ownership, ordering, spans, ICE boundaries, and persistent
  diagnostic identity;
- public API visibility and crate dependency direction.

Acceptance:

- parser tests cover every current syntax family and representative malformed
  input with stable diagnostics;
- identity tests cover revision replacement, stale handles, collision errors,
  and deterministic hashing;
- structural reports show no accidental dependency-cycle or ownership leak;
- large files are split only where the resulting owner API is narrower.

## 9. Phase F: Standard Library And Runtime (P1)

This is a separate product surface even when compiler changes are required.
Review `lib/std` in ownership order:

1. `builtin`, `atomic`, memory and allocators;
2. startup/runtime and process/syscall layers;
3. `io`, `fs`, paths, and error conversion;
4. collections, hashing, iterators, strings, formatting, and parsing;
5. build API and plan protocol.

Required evidence includes real compile/run tests, allocator and ownership
tests, syscall/ABI tests, error-path tests, target-specific behavior, and
build-host protocol roundtrips. Do not use compiler-only acceptance as a
substitute for runtime evidence.

## 10. Cross-Cutting Documentation And Test Work

Every touched algorithm must gain concise Rustdoc or an inline invariant
comment. At minimum, document:

- substitution and inference invariants;
- pattern usefulness/exhaustiveness matrix semantics;
- trait candidate ordering and cycle termination;
- layout/ABI classification rules;
- const evaluator state transitions;
- closure provenance and escape lattice;
- query/cache state machines;
- resource budget derivation and process ownership;
- LLVM wrapper safety preconditions;
- stable identity and persistence formats.

Tests must be added at the owner boundary first, then at the nearest end-to-end
consumer. Repeated matrices should be data-driven; dynamic process, filesystem,
allocator, startup, and runtime behavior should remain explicit integration
tests.

## 11. Progress Tracking

This section records implementation batches, not a fixed-size completion
percentage. The batch ledger grows whenever the audit discovers another
owner-boundary or regression matrix, so `completed / total` is intentionally
not a reliable project percentage: completing work can increase both numbers.
Track the fixed acceptance checklist below separately from this expandable
ledger, and report the two dimensions together.

Current snapshot (2026-08-29):

- Latest implementation batch: 702 completed entries in this ledger.
- Fixed acceptance items: 1 of 8 completed (the seven unchecked entries at the
  end of this section).
- The implementation ledger is evidence of covered batches; phase completion
  requires the corresponding fixed acceptance item plus the cross-cutting
  documentation, test-gap, and final validation evidence.

Use this section as the only temporary progress ledger. Update it in the same
commit as the corresponding implementation batch, and update the fixed
acceptance item only when its phase-wide evidence is complete.

- [x] Repository inventory and initial risk map.
- [x] Trait const arguments preserved through executable reachability.
- [x] Focused reachability identity regression test.
- [x] Initial Phase A owner map: local-resolution ordering, inference DAG
      boundary, and trait/projection cycle guards reviewed.
- [x] Canonicalized const-to-array-length substitution and rejected negative
      signed lengths at the type boundary.
- [x] Preserved const arguments and associated-type bindings during body-check
      trait-obligation deduplication; supertrait audit remains open.
- [x] Phase B LLVM target-layout and object-buffer null/empty boundaries
      hardened; broader backend audit remains open.
- [x] Unix build stale-lock reclamation made race-resistant with inode locks;
      non-Unix fallback and broader build/cache state audit remain open.
- [x] Build lock owner records and Linux process-start probes now use small
      stream-enforced protocol budgets. Oversized cache lock files cannot drive
      unbounded `read_to_string` allocations or smuggle a valid owner prefix,
      and owner Drop will not remove an oversized replacement record.
- [x] Query memory accounting now bounds every Linux meminfo/cgroup pseudo-file
      read on the opened stream and rejects cgroup membership paths containing
      non-normal components. Oversized kernel/container records degrade to the
      conservative memory fallback without unbounded allocation or escaping
      the expected cgroup mount.
- [x] Unix `nia-test-support` process-slot reclamation now retains owner-file
      locks, preventing stale slot cleanup from deleting a successor permit.
- [x] Phase A owner regression evidence: `nia-body-check` 228 libtest cases
      pass after obligation-identity hardening.
- [x] Resource-pool owner evidence: `nia-test-support` 17 libtest cases and
      strict Clippy pass.
- [x] Strict `-D warnings` dependency gate is clean for `nia-test-support` and
      `nia-build`; removed the `FunctionTerminator` and `TypedMatchArmBody`
      large-enum variants by boxing their uncommon/larger payloads.
- [x] Build lock acquisition now waits through the create/reclaimer inode-lock
      race; the full `nia-build` suite (189 passed, 1 ignored) covers concurrent
      publishers and stale-owner recovery.
- [x] Reachability trait-method expansion groups the concrete trait instance
      inputs into a named context, keeping the helper interface narrow without
      suppressing `too_many_arguments` diagnostics.
- [x] Persistent signature-cache publication enforces the same bounded entry
      size used by decoders, preventing oversized type/signature products from
      being written only to become guaranteed cache misses on the next load.
- [x] Persistent signature-cache reads enforce that shared 64 MiB budget on
      the opened stream as well as file metadata, so all six products reject
      oversized or concurrently growing entries without an unbounded
      `fs::read` allocation.
- [x] Persistent signature-cache publishers and corruption retirement now use
      per-key OS file locks. Retirement repeats the bounded read and deletes
      only the same observed bytes (or a still-oversized record), while
      non-replacing publication rechecks ownership under the lock; stale
      readers cannot remove or overwrite a concurrent replacement.
- [x] Loader frontend-cache publication now enforces its read-side size bound,
      and permission, I/O, and other publication failures remain visible to
      callers rather than being treated as generic concurrent-writer success.
- [x] Loader frontend-cache publication and retirement now share per-key OS
      locks on every persisted product. Publication rechecks the winner before
      rename on all platforms, while corrupt/invalidated observations are
      deleted only if the same bounded bytes remain installed; concurrent
      replacements are preserved.
- [x] Phase E syntax trees normalize caller-supplied token streams to one
      terminal EOF, discard unreachable post-EOF tokens, and make arbitrary
      cursor lookahead overflow-safe; syntax and parser owner suites pass.
- [x] Phase E node maps and origin tables compare and merge by stable locator
      across retired/reacquired handle generations, restoring `Eq`
      transitivity and preventing duplicate logical keys in one product.
- [x] Phase E symbol-table construction now has one initialization contract:
      `Default` and `new` both install the verified collision-free well-known
      symbol registry used by parsing, diagnostics, and persistence.
- [x] Stable diagnostic bundles enforce their 64 MiB budget incrementally and
      cap initial allocations derived from untrusted sequence lengths; all 222
      compiler-query persistence and incremental consumer tests pass.
- [x] Diagnostic reports now deduplicate by the complete structured diagnostic
      in linear expected time, preserving distinct secondary labels, notes,
      help, related locations, and internal debug evidence.
- [x] Phase E lexer error recovery consumes unsupported Unicode by scalar, so
      every byte span remains a UTF-8 boundary and lossless syntax trees retain
      the exact source even though language identifiers remain ASCII.
- [x] Builtin registries now expose exhaustive variant sets with bidirectional
      name/descriptor coverage tests; persisted builtin function and const tags
      use explicit append-only numbers rather than mutable `ALL` ordering.
- [x] Phase E canonical AST traversal now exposes pattern and generic-parameter
      callbacks, reaches binding/for patterns and every const generic type, and
      makes embedded pattern expressions available to semantic/reachability
      visitors while preserving const generic types in loader dependency
      discovery and versioned type-use products.
- [x] Phase C static lowering now treats newly emitted diagnostics as a failed
      product transaction: recovery `StaticInit::Zero` values are discarded
      from Full, FactsOnly, and StaticInitOnly Body IR paths, with a reachable
      rejected-initializer regression test and an explicit IR invariant.
- [x] Phase E caller-supplied syntax token streams normalize a terminal EOF's
      span to the actual source boundary, preserving root/cursor offsets even
      when external lexers provide a stale EOF location.
- [x] Phase C flow audit documented the ownership boundary between conservative
      syntactic return joins and typed pattern-matrix exhaustiveness; nominal
      constructors and literal/range coverage remain intentionally delegated
      to `nia-body-check::patterns`.
- [x] Phase C pattern analysis now validates non-empty queries before applying
      the empty-matrix witness shortcut, so uninhabited domains and invalid
      scalar/constructor combinations cannot be silently accepted; stable
      witness shape is preserved and matrix boundary tests cover the contract.
- [x] Phase C empty-matrix pattern validation now terminates on recursive type
      domains using an inductive type-path guard; cycle-only constructors remain
      uninhabited while finite-base recursive domains remain accepted, with
      independent-column regression coverage.
- [x] Phase C recursive pattern inhabitation now guards only unconstrained
      wildcard expansion instead of conflating an explicitly selected outer
      constructor with a nested occurrence of the same type. Mutable optional
      pointer patterns over recursive allocator nodes can reach their nested
      `null` base case without weakening rejection of cycle-only domains; owner
      and standard-library build-case regressions cover both boundaries.
- [x] Phase C backend planning now distinguishes source functions that merely
      have checked bodies from functions already selected by executable
      reachability. Vtables discovered after the frontend plan route late
      cross-module implementation methods back to their defining module, with
      owner and allocator-backed executable regressions covering the closure.
- [x] Phase C codegen readiness now treats dynamic trait calls as declaration
      edges to every planned candidate vtable and slot target. The owner
      directory indexes both exact object tables and supertrait segments, so
      definition validation cannot race ahead of cross-module function
      signatures; owner, upcast-boundary, and allocator executable regressions
      cover the publication-order invariant.
- [x] Phase B dynamic-call ABI validation now checks the selected method slot
      and target signature in every concrete vtable for an object view, rather
      than accepting whichever table was indexed first. Multiple-self
      regressions keep malformed later targets and publication-order changes
      from bypassing backend validation.
- [x] Phase B dynamic-call ABI validation now checks direct object tables and
      upcast source tables as one runtime candidate set. Source-table slots are
      rebased from the object view's principal trait segment, so a direct table
      cannot hide a malformed non-zero-offset upcast target or misclassify an
      unrelated table that merely shares a supertrait.
- [x] Phase A local HM inference now performs a symmetric occurs-check and
      transactional union rollback, preventing self-referential structural
      terms and late tuple/callable conflicts from poisoning later constraints;
      the self-reference diagnostic path has a no-recursion regression test.
- [x] Phase A associated-type projection cycle guards release their active key
      on missing source items as well as successful/recursive paths; the trait
      solver regression suite now checks this cleanup before resolving a valid
      sibling projection.
- [x] Phase B LLVM vtable emission, dynamic dispatch, and trait-object upcasts
      use checked conversions for array lengths and GEP indices; oversized
      entry/slot counts and slot-plus-one overflow now become diagnostics rather
      than truncating host `usize` values, with boundary tests at `u32::MAX`.
- [x] Phase B LLVM wrappers now validate serialized/object buffer starts before
      forming slices, handle verifier failures without dereferencing null error
      messages, reclaim partial bitcode modules and failed emission buffers, and
      propagate null target-machine triples instead of silently accepting an
      unconfigured module.
- [x] Phase B backend function validation now checks boolean control-flow
      conditions, integer switch targets and arm type equality, declared return
      values against the enclosing function signature, and `Never`-terminating
      bodies without rejecting valid defer-tail or trap lowering; malformed
      terminator contracts have a pre-LLVM regression test.
- [x] Phase B backend value validation now checks tuple and index projection
      result types plus tuple, array, repeat-array, and direct aggregate
      field initializer contracts before LLVM. A malformed-IR matrix covers
      element types, arity/count metadata, and selected-value types.
- [x] Phase A/B atomic validation now enforces pointer-width value types,
      pointee and write-permission contracts, operand/result shapes, RMW type
      restrictions, and the per-op LLVM ordering sets before emission.
      Cmpxchg failure ordering uses the actual acquire/release partial order,
      rejecting incomparable `Release`/`Acquire` pairs at both source and
      backend boundaries; focused matrices cover malformed producer IR.
- [x] Phase A/F atomic primitive admission now derives `isize` and `usize`
      widths from the configured target instead of assuming LP64. A direct
      32/64-bit owner regression proves pointer-sized atomics remain legal on
      ILP32 while fixed `i64` remains rejected by the 32-bit width gate.
- [x] Phase F general-purpose allocation no longer resizes a tracked large
      allocation to an empty block in place. The default `realloc` path now
      creates the canonical empty block and frees the old large owner; the
      existing real executable allocator regression verifies empty state and
      clean deinitialization after the transition.
- [x] Phase F Linux spawn error reporting now transfers its complete fixed-size
      stage/errno record across the close-on-exec pipe, retries interrupted
      reads and writes, and reaps every failed child through an EINTR-safe wait
      loop. Existing exec/cwd failure and repeated pipe-cleanup executables cover
      the parent/child protocol's observable paths.
- [x] Phase F public child cleanup now attempts stdin, stdout, and stderr closes
      even after one close fails, retaining only the first error. Exited-state
      caching can no longer strand a later owned descriptor after cleanup
      short-circuits; wait/EOF/pipe executable regressions cover the state path.
- [x] Phase F public reader/writer adapters now reject transfer counts larger
      than their supplied slices before advancing cursors, buffers, or limits.
      Trait defaults, buffered and limited adapters, file handles, and child
      pipes share the same boundary contract; a malicious implementation
      regression proves invalid counts return errors without corrupting pending
      buffered state.
- [x] Phase F hash-map cleanup now detaches only allocation slots whose fallible
      release succeeds and retains failed control/key/value owners for a later
      `deinit` retry. A failing allocator regression proves the first cleanup
      attempts every slot and the retry releases only the residual owner before
      resetting the map.
- [x] Phase F hash-map assume-capacity insertion now uses logical spare capacity
      after deletion even when its physical empty-slot growth budget is zero.
      The budget remains saturated if probing reaches an empty slot before a
      tombstone; a full-table delete/insert/reserve executable regression proves
      the insertion does not trap or underflow later capacity accounting.
- [x] Phase F build-plan encoding now checks aggregate and derived graph counts
      before narrowing them to wire integers, and rejects oversized generated
      payload lengths before writing their prefixes. Dependency validation also
      guards its derived indegree and producer counts against host arithmetic
      overflow.
- [x] Phase F build-host plan re-encoding now enforces the 64 MiB total budget
      before each Rust output-buffer growth. The writer retains the first
      attempted-size error and suppresses later writes, with a direct owner
      regression proving an over-budget field cannot transiently enlarge the
      serialized buffer.
- [x] Phase F build graph cleanup now retains every containing list whose owned
      element cleanup fails, including nested run arguments, command inputs,
      environments, and module imports. Fault-allocator regressions prove both
      step and module residual owners remain reachable and are fully released
      by a second `Build::deinit` attempt.
- [x] Phase F build-plan decoding no longer reserves typed Rust list capacity
      from a runner-controlled count before parsing any item. A deterministic
      large-element regression proves a maximum accepted count followed by
      truncation returns `Truncated` instead of overflowing or allocating
      `count * size_of(T)` capacity.
- [x] Phase F build-plan draft publication now marks its descriptor cleanup
      complete before the consuming `File::close` call. A close syscall failure
      cannot trigger a second `BadFd` defer attempt that masks the original
      error; the consuming-close invariant is documented at the std boundary.
- [x] Phase B bulk-memory validation now checks mutable destination slices,
      element metadata, copy/move source slices, and byte-only set operations
      before LLVM extracts fat-pointer fields or computes byte counts. The
      Function IR operation documents this producer/consumer contract, and a
      malformed-IR matrix covers each independent mismatch.
- [x] Phase B low-level builtin validation now checks unaligned-load metadata
      and byte pointers plus SIMD splat/lane/bitmask, integer bit-intrinsic,
      and Unicode scalar conversion operand/result contracts before typed LLVM
      casts, extracts, calls, or aggregate writes. Function IR documents the
      per-variant contract and a malformed-IR matrix covers all mismatch axes.
      Bitmask lane limits and result widening now follow the target `usize`
      width instead of assuming LP64.
- [x] Phase B operator validation now checks unary dereference/reference shape,
      numeric and integer operand classes, compatible binary operands, logical
      bool-only operations, comparison masks, and result types before LLVM
      selects scalar/vector or short-circuit builders. A malformed operator
      matrix covers arithmetic, comparison, logical, shift, and dereference
      boundaries.
- [x] Phase B cast validation now checks target/result metadata, numeric
      scalar-versus-vector shape, enum/integer and pointer/integer categories,
      volatile-pointer support, and rejects malformed casts before LLVM chooses
      typed pointer, float, or integer builders.
- [x] Phase B tagged-union expression validation now checks Optional/ErrorUnion
      constructor payloads, discriminant `u8` projections, and branch payload
      projections before LLVM extracts aggregate fields. Field projections now
      enforce selected-field result types, while slices validate source/result
      element identity, readonly propagation, and integer range bounds; focused
      malformed-IR matrices cover each independent mismatch.
- [x] Phase B enum expression validation now checks variant representation,
      payload arity/types, backing-width tags, and payload projection owner,
      index, and result contracts before LLVM computes enum offsets. The
      malformed-IR matrix covers scalar and payload-bearing enum paths.
- [x] Phase B scalar, character, boolean, string, byte-string, and null
      literal validation now checks target primitive/array shape and known
      lengths before LLVM literal builders receive a mismatched value type;
      malformed literal cases are included in the backend IR matrix.
- [x] Phase B range validation now ties lower/upper bound presence, inclusive
      metadata, payload types, and bound projections to `RangeTyKind` before
      LLVM constructs or extracts range structs. Malformed full, one-sided,
      two-sided, and projection cases extend the backend IR matrix.
- [x] Phase B builtin-value validation now requires `usize` results for target
      layout and field-offset constants, representable layout operands, valid
      aggregate field ownership, and integer-like const-eval results before
      LLVM chooses the result integer type.
- [x] Phase B promoted static-array pointers now validate their array payload,
      complete-array pointee type, readonly metadata, and origin module before
      LLVM publishes promoted storage; malformed producer cases cover each
      independent metadata mismatch.
- [x] Phase B global and generic-global-instance expressions now validate their
      typed view against published storage metadata, including only the
      source-approved mutable-to-readonly qualifier coercion, before LLVM loads
      the selected global.
- [x] Phase B place validation now derives each addressable path from its real
      local/global/deref base, checks pointer and integer-index preconditions,
      and verifies final projection plus address-of result types before LLVM
      forms typed GEPs or loads.
- [x] Phase B assignment validation now requires a unit result, an exactly
      typed writable local/global/deref target, and plain or compound RHS
      contracts matching the store and builtin binary operation LLVM emits.
- [x] Phase B closure-view validation now ties callable state pointers and
      non-capturing function-pointer adapters to the owner-qualified generated
      closure entry, checking identity, mutability direction, and full ABI
      signatures before LLVM materializes either view.
- [x] Phase B index-expression validation now rejects non-integer indices and
      non-indexable bases before LLVM computes element addresses, while still
      checking the selected element result type for valid arrays, pointers, and
      slices.
- [x] Phase B trait-object expression validation now checks upcast/coercion
      source and result identities, readonly direction, concrete data-element
      types, and required owner-indexed vtables before LLVM rewrites fat-pointer
      metadata.
- [x] Phase B effect-expression validation now requires unit-typed discards and
      rejects residual `Try` expressions that function lowering failed to
      rewrite into explicit CFG terminators before backend consumption.
- [x] Cross-cutting Function IR Rustdoc now records flat CFG/local/scope
      ownership, closure-entry ABI tables, promoted union relocation
      invariants, and inline-assembly operand ordering at the shared producer
      and backend consumer boundary.
- [x] Phase B layout fat-pointer sizing now uses checked target arithmetic and
      rejects zero alignment or `pointer_size * 2` overflow instead of wrapping
      a malformed target description; helper-level boundary tests cover both
      failure modes.
- [x] Phase B extern ABI checking recursively validates fields of imported
      extern structs, closing the cross-module path that previously accepted a
      nominally extern aggregate without checking nested bool/Nia-only types;
      an imported-struct regression test covers the boundary.
- [x] Phase B LLVM aggregate, inline-assembly, and parameter-slot lowering now
      use checked `usize` to `u32` conversions for tuple/field indices and
      extracted outputs, returning diagnostics instead of truncating malformed
      backend metadata.
- [x] Phase B function optimization now treats nested defer exits as structural
      edges of their enclosing CFG, protects address-observable local stores
      from alias-unsound overwrite elimination, and retains function
      relocations rooted by generic local-static instances; owner regressions
      and an optimized defer LLVM consumer test cover these boundaries.
- [x] Phase B function optimization now documents its valid-IR/effect
      preservation contract and debug-validates the body before optimization
      and after every enabled pass, attributing structural invariant failures
      to their introducing pass without adding release-build overhead.
- [x] Phase B cross-function optimization now preserves dynamic receiver ABI
      metadata during trait-call devirtualization, derives inline substitution
      order from the backend parameter contract rather than incidental local
      table order, and shares the canonical `FunctionInstanceKey`; const-generic
      instance propagation has a two-key regression test.
- [x] Phase B generic function materialization now enforces its 4096-instance
      limit across the complete module fixed point, including instances from
      earlier discovery rounds, with saturating boundary coverage.
- [x] Phase B backend validation now rejects duplicate or invalid ABI parameter
      mappings for functions and closure entries before LLVM, while respecting
      the flat local table's nested-closure parameters; source and generic
      closure ABI materialization now share one owner helper and consumer
      coverage.
- [x] Phase B LLVM function-instance reference validation now uses the canonical
      `nia-function-ir::FunctionInstanceKey` for its cache, avoiding a second
      backend-only identity representation.
- [x] Phase B backend lowering and LLVM value lookup now use the canonical
      `FunctionInstanceKey` for function-instance deduplication and negative
      lookup caching; tuple aliases remain only where the identity has a
      deliberately different shape (aggregate layout or LLVM value context).
- [x] Phase B backend validation now distinguishes structural type descriptors
      from runtime slots and requires layouts for function/closure returns,
      expression and place values, body results, and callable signatures before
      LLVM classification; an opaque-return regression covers the boundary.
- [x] Phase B backend validation now derives fallback pointer/fat-pointer/vector
      layouts from the module target instead of LP64, rejects LLVM-inexpressible
      array lengths, and verifies static array/string/repeat initializer counts
      before allocation or constant construction; 32-bit and oversized-repeat
      regressions cover the boundary.
- [x] Phase B LLVM lowering now derives `usize`/`isize`, layout-builtin values,
      slice lengths, and memory-loop counters from the target pointer width
      rather than an LP64 constant; a 32-bit function ABI regression verifies
      both the signature and returned value width.
- [x] Phase B LLVM call argument materialization now rejects fixed-arity
      mismatches instead of silently truncating through iterator `zip`, while
      variadic function pointers preserve and emit all tail arguments in source
      order; an indirect variadic call regression covers the ABI path.
- [x] Phase B backend validation now verifies dynamic trait-call vtable slots
      against emitted method identity, including vtables used by supertrait
      upcasts; malformed-slot and valid-upcast regressions cover both sides of
      the pre-LLVM boundary.
- [x] Phase B backend validation now checks dynamic trait-call receiver,
      argument, result, parameter, and return contracts against the canonical
      function or function-instance signature selected by the vtable; malformed
      ABI metadata and the existing dynamic-dispatch matrix cover the boundary.
- [x] Phase B backend validation now checks direct, function-instance, method,
      callable-view, function-pointer, builtin-method, and builtin-operator call
      contracts before LLVM; generic method instances also preserve their named
      extern and variadic flags instead of swapping positional tuple fields.
- [x] Phase B closure-entry calls now resolve through the exact enclosing
      source/function-instance owner and validate generated state-pointer,
      parameter, and return ABI metadata before LLVM; missing-entry and malformed
      call regressions cover the owner boundary.
- [x] Phase B backend validation now checks builtin slice length/pointer and
      range-bound receiver/result contracts and rejects unresolved iterator
      builtins before LLVM instead of relying on emitter-side type failures.
- [x] Phase B LLVM emission APIs now document the validated backend boundary,
      incremental readiness protocol, partial-output diagnostic contract,
      optimization mapping, and attributable object-cache fingerprints.
- [x] Phase B backend lowering APIs now document the program-wide planning,
      unique-owner assignment, parallel module finalization, readiness
      publication, and deterministic collection contracts.
- [x] Phase B source-function lowering now verifies AST/signature parameter
      arity before pairing source metadata with ABI types, rejecting stale
      phase products instead of silently truncating either side through `zip`.
- [x] Phase B function-instance and function-local-static substitution now
      rebuilds separate type/const maps from declaration-kind metadata, so
      interleaved parameters and nested const-generic references retain exact
      argument order and concrete const identity through backend IR; extension
      methods use their effective impl/method generic ownership metadata.
- [x] Phase B trait-object vtables now retain const arguments in their explicit
      table and per-entry payloads, impl/default-method instance identities,
      fingerprints, and dependency membership; kind-aware supertrait expansion
      propagates interleaved type/const substitutions and keys cycle guards by
      the complete trait instance. Pre-LLVM validation and LLVM slot selection
      distinguish absolute source-object slots from relative upcast slots and
      disambiguate repeated const-generic supertraits. Driver and LLVM
      regressions cover impl, default-method, supertrait dispatch, and an
      upcast across two instances of the same const-generic supertrait.
- [x] Phase B backend validation now requires trait-object vtable slot metadata
      to equal physical entry order and validates every per-entry trait type and
      const-argument type, preventing malformed but otherwise unreferenced
      tables from reaching LLVM array emission.
- [x] Phase A/B type aliases now retain declaration-order generic kinds in
      signatures and persistent caches, then rebuild separate type/const
      substitutions through one documented owner helper. Normalization and
      layout regressions cover interleaved type/const aliases end to end.
- [x] Phase B extern ABI checking now expands local and imported const-generic
      aliases with the canonical type/const substitution mapping before
      classifying nested fields, so diagnostics describe the forbidden concrete
      representation instead of rejecting valid alias argument structure;
      imported generic extern-struct fields are likewise instantiated before
      recursive ABI classification.
- [x] Phase B layout-root closure now substitutes aggregate fields with the
      canonical recursive type/const algorithm; mixed generic structs and
      unions no longer skip all nested roots or retain symbolic array lengths.
- [x] Phase A explicit-supertrait validation now substitutes and matches the
      complete trait instance, including const arguments and their types;
      `Base[8]` can no longer satisfy the required parent of `Child[4]`.
- [x] Phase A trait-object coercion now selects impl/default methods with full
      type/const patterns and records const-bearing semantic instantiations;
      specialization and executable dependency facts retain the vtable trait
      instance instead of emitting empty const vectors.
- [x] Phase B backend trait-method owner assumptions now rebuild type and const
      arguments from declaration-order generic kinds; specialization ordering
      also rejects mismatched type/const arities instead of accepting prefixes.
- [x] Phase C const union scalar encoding now rejects zero-width,
      non-byte-aligned, and wider-than-128-bit integer ABI descriptors before
      shifts or fixed-buffer slicing; malformed scalar and vector lanes have
      direct evaluator regressions.
- [x] Phase C const integer comparison now orders mixed signed/unsigned values
      without narrowing large `u128` inputs, while typed bitwise-not uses
      overflow-free sign extension and rejects invalid width metadata.
- [x] Phase C closure summaries now preserve capture-slot identity within known
      closure states, so selecting one captured value cannot pull unrelated
      captures into return or escape facts; flattened cross-function closure
      states retain a documented conservative fallback.
- [x] Phase C closure call analysis now follows runtime callee/receiver-before-
      argument evaluation order, preventing callee-side assignments from
      hiding stack-backed arguments from escape summaries and diagnostics.
- [x] Phase C closure loop analysis now includes repeated `while` condition
      effects in the backedge fixed point, so condition-side provenance updates
      after the first iteration reach the body and exit environment.
- [x] Phase C closure type filtering now distinguishes active structural cycles
      from repeated sibling types, preventing value-only tuples and error
      unions from spuriously retaining callable or captured-address provenance.
- [x] Phase C closure defer analysis now delays registered expressions until
      scope exit and applies them in LIFO order after the tail value, matching
      Function IR and exposing later callable-state updates to deferred escapes.
- [x] Phase C closure return and error-propagation paths now replay all active
      lexical defers against an isolated exit environment before recording the
      exit, so later fallthrough overwrites cannot hide deferred escapes.
- [x] Phase C closure assignment analysis now evaluates dereference/index place
      effects before the RHS, matching Function IR and preventing destination
      expressions from hiding the callable provenance actually stored.
- [x] Phase D compiler check/emit action-cache metadata now shares a 64 MiB
      read/write bound; oversized entries are detected from file metadata,
      retired under the mutation lock, and never read into unbounded buffers.
- [x] Phase D output-transaction recovery now enforces its journal limit on the
      opened byte stream as well as pre-read metadata, closing the file-growth
      race that could turn a nominally bounded recovery read into `fs::read`.
- [x] Phase D exact-key generated-file cache reads now derive their byte budget
      from the identity's payload length, enforce it across lookup, collision,
      and retirement paths, and reject publication payloads inconsistent with
      the identity.
- [x] Phase D generated-file invalidation scans now validate variable-size
      records with a fixed streaming buffer, bound persisted identity fields by
      the canonical build-plan string limit, and materialize payload bytes only
      for an exact entry published after the direct miss; corruption retirement
      revalidates under the mutation lock without an unbounded reread.
- [x] Phase D regular-file external-command tool/input identities now stream
      fingerprints from one opened handle and reject length growth or truncation;
      Unix declared inputs also use no-follow opens.
- [x] Phase D external-command directory identities now plan only sorted entry
      metadata, then stream the registered recursive byte format through one
      fixed buffer. File payloads and nested encodings no longer accumulate in
      memory, and a compatibility regression proves cache-key parity.
- [x] Phase D staged external-command output snapshots now open each regular
      file once and enforce the observed length on the byte stream, closing the
      metadata/growth race. The later streaming publication path retains these
      handles and their stable checksums instead of buffering output bytes.
- [x] Phase D generated-file publication now compares an existing destination
      through one no-follow handle, an exact expected-length budget, and a fixed
      buffer; unchanged-output detection no longer uses an unbounded `fs::read`.
- [x] Phase D external-command invalidation scans now bound identity metadata
      by canonical plan limits, checksum arbitrarily large output payloads with
      one fixed buffer, and retain only candidate fingerprints. Corrupt records
      are revalidated by the same streaming parser under the mutation lock so a
      concurrent valid replacement is not retired.
- [x] Phase D exact external-command cache hits now retain an opened entry plus
      bounded output offsets instead of materializing payloads. Restoration and
      immutable-output comparison stream from that handle with checksum
      revalidation, and exact corruption retirement no longer rereads the whole
      record under the mutation lock.
- [x] Phase D external-command cache publication now retains opened staged
      output handles with stable length/checksum identities, writes the existing
      envelope directly to its staged file, and compares publication collisions
      byte-for-byte through fixed buffers. Output snapshots and whole-record
      encoding/collision allocations are no longer proportional to payload size.
- [x] Phase D executable static-archive inputs now derive both compiler-emit
      and linker cache identities from one no-follow regular-file handle using
      fixed buffers and exact observed-length checks. Link orchestration no
      longer retains every archive in memory, and compatibility regressions
      preserve both registered fingerprint domains.
- [x] Phase D native object work-product records now parse bounded canonical
      keys and stream payload validation, invalidation scans, publication, and
      collision comparison through fixed buffers without materializing the
      complete cache envelope. Per-entry mutation locks preserve an immutable
      valid winner and retire corrupt observations only while identical bytes
      remain installed; exact hits allocate only the payload required by the
      codegen cache interface.
- [x] Phase D static-archive cache capture, publication, invalidation, collision
      comparison, and restoration now stream arbitrarily large payloads through
      fixed buffers from stable opened handles. The existing v1 record format
      is preserved, checksum failures cannot install partial outputs, and
      per-entry mutation locks prevent Unix rename replacement or stale-reader
      retirement from discarding a valid immutable winner.
- [x] Phase D executable link-result caching now applies the same fixed-buffer
      capture, validation, invalidation, publication-collision, and stale
      retirement protocol while preserving the v3 envelope and v2 checksum
      domain. Cache hits stream directly into an atomic staged executable,
      verify integrity before installation, and restore executable permissions
      without coordinator allocations proportional to the binary size.
- [x] Phase D static-archive orchestration now streams the archive tool's output
      into a sibling staged destination with exact opened-length validation and
      atomic installation. The Driver no longer materializes the temporary
      archive with `fs::read`, and missing, growing, or truncated tool outputs
      leave an existing destination untouched.
- [x] Phase B backend function and concrete function-instance values now carry
      the exact published source-level function-pointer signature. Validation
      rejects non-pointer result types plus parameter, return, and variadic
      mismatches before LLVM materializes the referenced symbol.
- [x] Phase B inline assembly now validates its unit effect result, scalar
      operand boundary, canonical constraints and clobbers, plus exact writable
      output storage before constructing an LLVM inline-assembly call.
- [x] Phase B local value expressions now resolve their body-local storage type
      and reject unrelated or readonly-to-mutable views while preserving the
      source-approved mutable-to-readonly pointer, volatile-pointer, and slice
      coercions represented directly in function IR.
- [x] Phase B struct and union literals now bind their definition identity and
      aggregate kind to the nominal expression type. Struct literals additionally
      require every declared field exactly once, preventing LLVM from loading a
      partially initialized aggregate allocation.
- [x] Phase B switch terminators now admit only directly representable constant
      integer patterns and reject duplicate target-width bit patterns before
      LLVM. Checked negative integer patterns retain their lowering-generated
      signed spelling through codegen, including the `i128::MIN` boundary.
- [x] Phase B function-expression literals now validate their decoded payloads
      as well as their types: integer spellings and target-width ranges, finite
      float ranges, byte-character syntax, and Unicode scalar invariants are
      rejected before LLVM constant construction can fail or truncate them.
- [x] Phase B static initializers now validate scalar variant/type identity,
      finite float and Unicode scalar payloads, typed char/byte arrays, and null
      pointer destinations before module-level LLVM constant emission. Address
      initializers also require pointer-like destination types before relocation
      lookup and path validation.
- [x] Phase B static function addresses now require an exact source-level
      function-pointer signature, including semantic parameter types, return
      type, and variadic status, before LLVM can erase mismatches with a
      constant pointer bitcast.
- [x] Phase B static global addresses now require data-pointer destinations
      whose pointee type matches the selected global/path and whose mutability
      cannot expose immutable storage as writable.
- [x] Phase B static aggregate initializers now require every struct field
      exactly once and exactly one declared union field, preventing missing or
      duplicate fields from being hidden by layout-ordered constant emission.
- [x] Phase B LLVM inline-assembly and shared basic type/value wrappers now
      reject null FFI results before constructing typed handles or querying
      their LLVM kind. Builder memory, call, and control-flow operations apply
      the same check before typed-handle construction or follow-up calls such
      as switch-case insertion. Atomic, arithmetic, conversion, and fence
      builders now use the same guard, so fallible wrapper APIs produce
      diagnostics instead of panicking or passing null back into LLVM.
- [x] Phase B LLVM enum and string attribute construction now rejects null
      handles at creation, and function declaration propagates attribute
      failures before an invalid handle can reach `LLVMAddAttributeAtIndex`.
- [x] Phase B LLVM debug-info builder creation now propagates a null DIBuilder
      result through `LlvmResult`, preventing metadata calls from starting with
      an invalid builder handle.
- [x] Phase B LLVM debug metadata factories and declare insertion now validate
      null metadata/instruction returns before constructing typed handles.
- [x] Phase C function-IR scope unwinding now rejects control-flow edges whose
      source and destination have unrelated root scopes, while preserving
      child/ancestor and function-return defer ordering.
- [x] Phase B LLVM context/module allocation wrappers now check module,
      function, global, and basic-block handles before typed construction;
      null-result helper tests and owner-level error contracts prevent FFI
      allocation failures from becoming wrapper-constructor panics.
- [x] Phase B LLVM function return-type inspection now rejects a null type
      before querying its kind, with a shared classifier and regression test
      covering the fallible wrapper boundary.
- [x] Phase B LLVM call-result classification now turns a null `LLVMTypeOf`
      result into the call site's existing fallible value channel before kind
      inspection, with a focused malformed-FFI regression test.
- [x] Phase B LLVM target lookup and object emission now dispose optional
      owned diagnostic messages on every successful and null-result path, so
      future C API behavior cannot leak messages outside the wrapper boundary.
- [x] Phase B Windows LLVM IR printing now removes partially written temporary
      output on failure and disposes an unexpected owned success message,
      closing both filesystem and C allocation ownership paths.
- [x] Cross-cutting LLVM Rustdoc now documents the public error/handle traits,
      predicates, atomic/linkage controls, context ownership, module transfer
      and linking contracts, target identities, object emission, and facade
      boundaries. `cargo rustdoc -p nia-llvm -- -W missing-docs` reduced the
      crate baseline from 382 warnings to 249 without suppressing the lint;
      typed value and builder method coverage remains open.
- [x] Cross-cutting LLVM typed-handle Rustdoc now identifies every generated
      type/value wrapper, shared enum variant, metadata alias, and the
      function/global/block/instruction/phi/call ownership views. The same
      unsuppressed missing-docs audit fell from 249 warnings to 183; primitive
      type/value operations and builder methods remain open.
- [x] Phase B LLVM module-flag attachment now validates value-to-metadata
      conversion before calling the infallible attachment API, returns the
      failure through `LlvmResult`, and has focused null-metadata coverage plus
      Rustdoc for the debug-version and flag ownership contracts.
- [x] Cross-cutting LLVM primitive type/value Rustdoc now covers function and
      aggregate construction, constant and undef semantics, typed extraction,
      shape queries, and GEP/value views. The unsuppressed missing-docs audit
      fell from 181 warnings to 95, all now isolated to the builder API.
- [x] Cross-cutting LLVM builder Rustdoc now covers insertion/debug state,
      memory and CFG operations, unsafe GEP contracts, atomic orderings,
      signed/unsigned arithmetic, scalar/vector operations, and conversions.
      `cargo rustdoc -p nia-llvm -- -D missing-docs` is now clean, reducing the
      crate's audited baseline from 382 warnings to zero without suppression.
- [x] Cross-cutting backend-IR Rustdoc now documents the program/module views,
      write-once concurrent module store, single-consumer readiness protocol,
      and preplanned owner directory invariants. Its unsuppressed missing-docs
      baseline fell from 279 warnings to 244; partition and payload schemas
      remain open, while `nia-backend-lower` is already clean.
- [x] Phase B backend partition Rustdoc now explains transient versus stable
      unit identities, canonical dependency and linker ordering, deterministic
      four-bucket splitting, and closure/vtable ownership. Existing invariant
      tests cover allocation-independent keys, duplicate rejection, stable
      membership, and malformed linker order; the unsuppressed backend-IR
      missing-docs baseline fell from 244 warnings to 212.
- [x] Cross-cutting backend-IR payload Rustdoc now documents module ownership,
      layout snapshots, closure-entry ABI, generic substitutions, static
      payloads, function parameters, and trait-object dispatch records. The
      crate's strict `missing-docs` audit is now clean; the remaining backend-IR
      schema review is limited to any newly introduced public types.
- [x] Phase C function-IR core Rustdoc now documents stable block/scope
      identities, local and defer ownership, terminator edge contracts,
      propagation tags, expression typing, reference identity keys, and scope
      unwinding APIs. Function-IR tests (15) and strict Clippy pass; its
      unsuppressed missing-docs baseline fell from 402 to 223, with the large
      expression/callee payload surface remaining for follow-up batches.
- [x] Phase C function-IR expression Rustdoc now documents aggregate and
      relocation payloads, atomic ordering, inline assembly, trait-object and
      closure coercions, callee ABI fields, and typed place projections.
      `cargo rustdoc -p nia-function-ir -- -D missing-docs` is clean; the same
      15 validation/reference tests and strict Clippy remain green.
- [x] Phase C function-lower public boundary Rustdoc now documents typed-store
      ownership, lowering diagnostics, closure extraction, and the validation
      contract of `lower_function_body`. Its strict `missing-docs` audit is
      clean; 55 lowering/validation/scope/match/loop tests and strict Clippy
      pass.
- [x] Phase C function-opt CFG/purity audit now documents the conservative
      discardability lattice, fixed-point empty-jump merging, and reachability
      retention for defer-only control-flow references. Existing optimization
      matrices cover effect preservation, scope boundaries, nested defers, and
      policy ordering; 54 tests, strict Clippy, and `-D missing-docs` pass.
- [x] Phase C body-IR foundation Rustdoc now documents module/body ownership,
      locals, statement/control-flow records, loop and match schemas, pattern
      constructors, and typed expression identity. The unsuppressed body-IR
      missing-docs baseline fell from 436 to 287 warnings; the expression
      payload, call, memory, atomic, and place schemas remain for follow-up
      batches.
- [x] Phase C body-IR expression Rustdoc now documents aggregate construction,
      closure captures, try conversion metadata, union relocations, slices,
      ranges, trait-object coercions, and nested control-flow expressions. The
      unsuppressed body-IR missing-docs baseline fell from 287 to 166 warnings;
      call, memory, atomic, and place payloads remain for the next batch.
- [x] Phase C body-IR call/memory/place Rustdoc now documents builtin queries,
      inline assembly operands, memory intrinsic overlap contracts, atomic
      ordering, resolved callee identities, and typed place projections.
      `cargo rustdoc -p nia-body-ir -- -D missing-docs` is clean and body-ir
      Clippy/test checks pass.
- [x] Phase C static-IR Rustdoc now documents target-independent initializer
      variants, aggregate field identity, address paths, and precise reference
      extraction semantics for zero-length repeats and generic function
      addresses. `nia-static-ir` strict Rustdoc, 3 tests, and Clippy pass.
- [x] Phase C closure-check provenance audit now documents the finite summary
      lattice, origin transfer rules, known/unknown call handling, and isolated
      defer exit environments. Existing recursive, loop, capture, assignment,
      and LIFO-defer matrices remain green (8 tests), with strict Clippy and
      Rustdoc clean.
- [x] Phase C static-check boundary audit now documents active-revision
      filtering, static-vs-const representation rules, cross-module lookup
      ownership, and shared const-evaluation step budgeting. The existing 10
      static initializer acceptance/rejection tests, strict Clippy, and
      Rustdoc checks pass.
- [x] Phase C const-eval budget audit now documents outer-session reset and
      nested-session sharing, charge-before-execution semantics, recursion
      rejection without state mutation, balanced cleanup, and the interpreter
      loop cap. All 39 const-eval tests and strict Clippy pass; its unsuppressed
      missing-docs baseline is 161 warnings for follow-up schema batches.
- [x] Phase C const-check query and cache boundary audit now documents the
      monotonic array-length, enum, value, and typed-fact phase pipeline,
      cross-module provider ownership, execution-frame inputs, and cached
      result schemas. All 34 const-check tests, strict Clippy, and strict
      Rustdoc pass.
- [x] Phase C const-eval value and storage Rustdoc now documents allocation and
      frozen-pointer identity, live place projections, target ABI schemas,
      initialization masks, exact pointer relocations, and union write
      preservation rules. The unsuppressed missing-docs baseline fell from 161
      warnings to 59; all 39 tests and strict Clippy pass.
- [x] Phase C const-eval environment and execution-boundary Rustdoc now covers
      session hooks, lexical and function frames, early versus resolved name
      authority, typed preparation hooks, iterator/call dispatch, projected
      place writeback, and literal decoding. `nia-const-eval` strict Rustdoc is
      now clean; all 39 tests and strict Clippy continue to pass.
- [x] Phase C flow-check public boundary Rustdoc now documents active-revision
      and reachability filtering, syntax-versus-typed exhaustiveness ownership,
      conservative loop fallthrough, and continued diagnostics inside
      unreachable syntax. Its 14 control-flow/pattern tests, strict Clippy, and
      strict Rustdoc pass.
- [x] Phase C const-IR resolved-flow and expression Rustdoc now documents
      assignment paths, iterator/match payloads, pattern constructors, resolved
      expression variants, range/slice bounds, array repeat forms, and field
      initializers. Its unsuppressed missing-docs baseline fell from 547 to 349
      warnings; all 15 tests and strict Clippy pass, while type-argument and
      early-IR schemas remain.
- [x] Phase C const-IR early/resolved identity schema Rustdoc now documents
      early names, optional semantic ids, generic and associated targets,
      operator/range payloads, lowering input providers, and all public early
      statement/pattern fields. `nia-const-ir` strict Rustdoc is now clean;
      its 15 tests and strict Clippy pass.
- [x] Phase C body-check input/product boundary audit now documents cached const
      phase views, program providers, reachability/product filters, local and
      program signature scopes, diagnostic ownership, and incremental reuse.
      Pattern-matrix adapters now explain conservative opaque fallback and
      nominal rest-field expansion; provider revision/request APIs are now
      documented as well. `nia-body-check` strict Rustdoc is clean; its 230
      tests, strict Clippy, and format checks pass.
- [x] Phase C typed Body IR lexical-body walking now has direct preorder
      regressions for statement loops, block/if/if-pattern branches, expressions
      nested in patterns, and all three match-arm body forms. Closure captures
      remain in the enclosing function walk while closure bodies are explicitly
      excluded as separate function boundaries.
- [x] Phase C function-lowering input validation now checks `for` item patterns
      before consuming the iterator/body, closing the sole statement-path gap
      where a nested recovery expression could bypass malformed Body IR
      rejection and reach pattern lowering.
- [x] Phase C closure discovery and provenance analysis now traverse expression
      and range patterns across bindings, loops, if-patterns, and match arms;
      nested closures in those runtime-evaluated pattern operands no longer
      degrade to unknown-call summaries.
- [x] Phase C executable reachability now includes `for` item patterns when
      collecting function/global references, closing the last statement-level
      omission alongside existing binding, if-pattern, and match pattern
      traversal.
- [x] Phase C executable-facts Rustdoc now documents typed-IR versus semantic-
      fact authority, per-item merge schemas, trait method/vtable instance
      identity, sparse/dense collection strategy, and semantic-fact filtering
      ownership. Strict Rustdoc is clean.
- [x] Phase C executable-reachability input/result contracts now document root
      ownership, parse-valid module filtering, lazy signature callbacks,
      type-only module retention, per-module projections, and fixed-point body
      statistics.
- [x] Phase C reachability owner-state regressions now prove item insertion
      retains owner modules, pending modules enqueue once, and per-module body
      projection excludes type-only owners and compile-time-only globals.
- [x] Phase C semantic-fact filtering now has an owner matrix proving only
      requested function facts and global type entries survive reachability
      projection; node-wide facts remain intentionally preserved.
- [x] Phase C typed-body walking now directly covers bodies hidden in place
      dereference/index projections, expression callees, inline-asm operands,
      union relocations, memory intrinsics, and atomic compare-exchange
      operands, with strict preorder assertions.
- [x] Phase C executable-facts reachability now has a matching hidden-container
      matrix proving function/global references survive assignment places,
      function-pointer callees, inline asm, union relocations, memory intrinsics,
      and atomic operands.
- [x] Phase C function-lowering malformed-input coverage now rejects recovery
      expressions hidden in places, function-pointer callees, inline-asm input
      and output operands, union relocations, memory destinations, and atomic
      pointers before Function IR construction.
- [x] Phase C flow-check boundary coverage now traverses return-value
      expressions before termination and resets loop-control scope across
      closure bodies, preventing hidden diagnostics and cross-function
      `break`/`continue` targets.
- [x] Phase C flow reachability now propagates termination through eager
      conditions and expression operands while preserving a fallthrough path
      for short-circuit logical RHS operands; a container matrix covers calls,
      unary/binary operators, `if`, and `while` conditions.
- [x] Phase C function-tail flow now uses the normal expression traversal and
      caches block/statement outcomes for return-value analysis, exposing
      diagnostics in tail calls, closure bodies, and deferred block tails
      without reporting nested match diagnostics twice.
- [x] Phase C static checking and lowering now preserve negative floating-point
      literals as backend-representable `StaticInit::Float` values instead of
      routing every unary negation through integer const evaluation; an LLVM
      integration regression covers both `f32` and `f64` globals.
- [x] Phase C consecutive closure expressions now have parser and LLVM
      regressions proving right-nested currying and nested closure-state
      returns. Local resolution enforces explicit capture boundaries before
      Body IR, replacing late missing-local backend failures with source-level
      diagnostics when an inner closure omits an outer local from its captures.
- [x] Phase C method receivers now obey the same explicit closure boundary as
      named locals; direct `self` uses diagnose the required bind-and-capture
      form before Body IR instead of reaching backend entries as missing locals.
- [x] Phase C function-lowering input validation now checks closure-state
      identity, capture and parameter arity/types, return ABI, distinct slot
      locals, and direct closure-callee shape before entry extraction. Malformed
      typed products return diagnostics instead of reaching closure lowering's
      assertions; Body IR Rustdoc records capture-alias versus body-local
      ownership.
- [x] Phase C closure entry extraction now rejects duplicate `ClosureId`
      definitions within one typed function body instead of silently retaining
      the first definition through the lowering entry cache.
- [x] Phase B Function IR closure-entry validation now requires the body result
      type to equal the declared ABI return and requires `state_param` plus
      `params` to enumerate every parameter local exactly once, preventing
      cached or hand-built IR from exposing an uninitialized hidden parameter.
- [x] Phase B function lowering now removes nested closure aliases and
      parameters from the enclosing entry's flat local table. Curried closures
      retain separate ABI-owned local tables instead of presenting inner
      parameters as uninitialized storage in the outer generated function.
- [x] Phase B backend validation now independently requires closure state types
      to match the entry identity and callable signature and rejects unmapped
      closure-body parameter locals, covering direct/cached Backend IR inputs
      that do not pass through function lowering.
- [x] Phase B closure-entry keys now validate that source and instantiated
      owners match the source `ClosureId` and resolve to an emitted backend
      function in the same published BackendModule before owner-relative calls
      consume them. Partition planning keeps dangling cached owners
      deterministic so validation can diagnose them instead of panicking at
      the pre-LLVM boundary.
- [x] Phase B closure-entry symbols are checked against deterministic
      owner-derived mangling before LLVM declaration, preventing cached IR from
      aliasing an unrelated function name while retaining a valid entry key.
- [x] Phase A generic inference now applies the same callable mutability
      relation as coercion: mutable closure state may infer a readonly
      `&Fn(T) T` target, while readonly state cannot satisfy a mutable target.
- [x] Phase A inferred closure, callable, and tuple signatures now require
      matching arity before contributing generic substitutions, preventing
      malformed closures from partially selecting a concrete instance before
      the authoritative argument check reports the structural mismatch.
- [x] Phase A closure-signature inference now preserves the `&` versus `&mut`
      address operator when matching canonical callable views, so readonly
      closure addresses cannot seed substitutions for mutable `&mut Fn`
      candidates before authoritative argument checking.
- [x] Phase A closure-signature inference now probes the complete recursive
      type shape before committing substitutions; nested tuple/callable
      mismatches cannot leak a valid prefix into later arguments. Function,
      method-candidate, and thin-function-pointer regressions cover the shared
      inference path.
- [x] Phase A transactional closure inference now mirrors the ordinary generic
      inferencer across ranges, nominal/builtin-trait types, trait objects and
      associated bindings, and projections. Range and generic trait-object
      regressions ensure the safety probe does not suppress valid inference.
- [x] Phase B codegen validation rejects duplicate concrete closure-entry keys
      before ProgramIndex overwrite semantics can alias multiple definitions
      onto one generated LLVM entry identity.
- [x] Phase A const-generic call inference now traverses arrays, pointer/slice
      families, tuples, optionals, error unions, ranges, callable forms,
      nominal/builtin-trait types, trait objects and associated bindings, and
      projections. A complete compatibility probe precedes staged inference,
      so an incompatible outer or later-nested shape cannot leak a partial
      const substitution into instance selection.
- [x] Phase A normalized type matching now compares tuple, optional, and error-
      union structure recursively instead of requiring equivalent reconstructed
      types to retain the same interned identity. Nested const substitutions
      cover the independently interned tuple path.
- [x] Phase A structural type equivalence now covers reconstructed builtin
      traits, trait-object pointees, and closure states in body checking, while
      the cross-store `nia-ty` contract also covers const-only, vector, and
      closure-state variants. Associated bindings compare as unordered keyed
      sets, and a dual-store regression prevents intern identity from masking
      future variant omissions.
- [x] Phase A type-match cache invalidation now follows unevaluated const
      expressions through nominal/trait/projection const arguments, associated
      binding keys, and closure-state components, preventing an early negative
      comparison from surviving after its nested const value becomes available.
- [x] Phase A call inference now preserves recursive type-parameter discovery
      through mixed type/const nominal arguments, trait-object const argument
      types, associated-binding keys, and projection const argument types.
      Owner regressions cover nominal and associated-binding-key failures that
      previously stopped before recording an otherwise inferable type.
- [x] Phase A method/impl type-pattern matching now treats trait-object
      associated binding keys and values as one transactional candidate. A
      failed binding alternative cannot leak substitutions or prevent a later
      compatible binding from being selected; method argument matching covers
      multiple associated bindings with independent inferred type parameters.
- [x] Phase A trait-solver candidate filtering now matches each trait-object
      associated binding's key and value transactionally, preserving type and
      const substitutions while trying later compatible candidates.
- [x] Phase A `nia-ty` and executable-reachability trait-object equivalence now
      compares associated binding values as part of the key candidate, rather
      than rejecting a reordered binding set after selecting the first key-only
      match. Cross-store and reachability regressions cover the reordered case.
- [x] Phase B backend instance matching now traverses trait-object associated
      binding keys and values together, preserving extension-method type
      substitutions while trying compatible bindings in either trait-object
      form. The backend-lowering owner suite remains green after the fix.
- [x] Phase A body-check generic inference now matches associated binding keys
      and values transactionally for both type and const inference. A
      value-incompatible first candidate cannot seed substitutions or hide a
      later compatible binding; overlapping generic-key candidate regressions
      cover the generic call path.
- [x] Phase C const-execution type inference now probes associated binding
      keys and values with cloned type substitutions, retaining the first
      useful diagnostic only after every compatible candidate is exhausted.
      Const-evaluation trait-object inference covers distinct associated-
      binding instances with the same member name.
- [x] Phase A body-check and trait-solver structural equivalence now tests
      associated binding key and value together in the same candidate. Reordered
      bindings with overlapping keys cannot stop at a value-incompatible first
      match.
- [x] Phase A associated-binding equivalence now consumes each actual binding
      at most once. Body-check, trait-solver, type-store, and reachability
      comparisons reject mismatched duplicate-key multiplicities instead of
      reusing one successful candidate.
- [x] Cross-cutting Phase A type-resolution Rustdoc now documents the
      resolution product, name categories, provider context, and all public
      active-tree/module entry points. `nia-type-resolve` strict Rustdoc,
      seven owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase A type-lowering Rustdoc now documents canonical
      storage, const-expression preservation, provider context, versioned type
      uses, and declaration-versus-full lowering entry points. `nia-type-lower`
      strict Rustdoc, 17 owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase A value-resolution Rustdoc now documents stable node
      products, value categories, associated-value providers, module context,
      and module/expression entry points. `nia-value-resolve` strict Rustdoc,
      four owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase A local-resolution Rustdoc now documents stable
      local products, lexical binding categories, type-prefix identities,
      filtered active-tree context, and origin-aware entry points.
      `nia-local-resolve` strict Rustdoc, 17 owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase A type-normalization Rustdoc now documents alias
      expansion inputs, normalized identity products, recursive-cycle
      diagnostics, and explicit-input normalization scope. `nia-type-normalize`
      strict Rustdoc, nine owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase B ABI-check Rustdoc now documents imported and local
      signature-family views plus diagnostic ownership. `nia-abi-check` strict
      Rustdoc, six owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase B layout Rustdoc now documents target-dependent
      aggregate schemas, concrete instance keys, cross-module providers, root
      selection, and checked ABI helper contracts. `nia-layout` strict Rustdoc,
      26 owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase E target-configuration Rustdoc now documents host and
      explicit target identities, active-item-tree pruning, inactive-span
      preservation, symbol-aware condition evaluation, and diagnostic
      ownership. `nia-target-config` strict Rustdoc, two owner tests, and strict
      Clippy pass.
- [x] Cross-cutting Phase B mangling Rustdoc now documents stable module hashes,
      sanitized symbol components, base and concrete-instance names, and
      resolver-driven type/const encoding. `nia-mangle` strict Rustdoc, eight
      owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase E symbol-table Rustdoc now documents stable content
      identities, collision rejection, known-symbol initialization, shared
      resolver views, and unresolved display fallback. `nia-symbol-table`
      strict Rustdoc, three owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase E public-surface Rustdoc now documents exported
      surfaces, using-scope products, module-owned diagnostics, and the stable
      type-exposure reverse index. `nia-public-surface` strict Rustdoc, three
      owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase C literal-decoding Rustdoc now documents numeric
      suffix/radix evaluation, Unicode and byte character decoding, string
      concatenation, and checked literal-length contracts. `nia-literals`
      strict Rustdoc, eight owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase C IR-name Rustdoc now documents promoted-allocation
      identities, receiver/source/generated/temporary local categories, and
      stable internal storage names. `nia-ir-names` strict Rustdoc and strict
      Clippy pass; the owner contains no unit tests.
- [x] Cross-cutting Phase A provider-summary Rustdoc now documents extension
      target classification, conservative nominal candidates, deterministic
      provider indexes, associated-item queries, and facade visibility filters.
      `nia-provider-summary` strict Rustdoc, 11 owner tests, and strict Clippy
      pass.
- [x] Cross-cutting Phase D imports Rustdoc now documents reserved module roots,
      source/module stable identities, graph snapshots and handles, declaration
      resolution, visibility checks, and child-path derivation. `nia-imports`
      strict Rustdoc, five owner tests, and strict Clippy pass.
- [x] Cross-cutting infrastructure hash Rustdoc now documents the deterministic
      non-cryptographic hasher and its map/set adapters. `nia-hash` strict
      Rustdoc and strict Clippy pass; the owner contains no unit tests.
- [x] Cross-cutting infrastructure span Rustdoc now defines half-open source
      ranges, saturating length, and empty-range semantics. `nia-span` strict
      Rustdoc and strict Clippy pass; the owner contains no unit tests.
- [x] Cross-cutting Phase D source Rustdoc now documents relocation-stable
      logical identities versus physical paths, session-local ids, monotonic
      revisions, exact-version lookup, and concurrent current-snapshot storage.
      `nia-source` strict Rustdoc, nine owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase E item-tree Rustdoc now documents conditional item
      selection, inactive ranges, versioned node identity, declaration versus
      shallow-definition comparison, signature-consumer filtering, and AST
      projection. `nia-item-tree` strict Rustdoc, ten owner tests, and strict
      Clippy pass.
- [x] Cross-cutting Phase E node-identity Rustdoc now documents versioned
      span/child-path locators, store-scoped compact handles, revision
      retirement, locator-based map equality/merge, and transactional AST
      origin tables. `nia-node-id` strict Rustdoc, 18 owner tests, and strict
      Clippy pass.
- [x] Cross-cutting Phase D query Rustdoc now documents declarative provider,
      storage, fingerprint, and registry contracts; shared versus owned access;
      red/green validation; cross-database sessions; completion-order streams;
      quiescent retirement; bounded task execution; and process-wide LLVM
      memory permits. `nia-query` strict Rustdoc, 81 owner tests, and strict
      Clippy pass.
- [x] Cross-cutting Phase D toolchain Rustdoc now documents relocatable resource
      discovery, versioned manifest compatibility, path-independent identity
      fingerprints, host versus artifact targets, required standard-library and
      runtime resources, and structured discovery failures. `nia-toolchain`
      strict Rustdoc, four owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase D compatibility-registry Rustdoc now documents the
      central ownership of generated ABI versions, toolchain schemas, persisted
      payload magic/schema identities, versioned cache namespaces, build output
      recovery formats, and compiler/link/archive work products. `nia-compat`
      strict Rustdoc, three owner tests, and strict Clippy pass.
- [x] Phase D link/archive result identity now has explicit contracts for
      logical versus physical inputs, component fingerprints, conservative
      cache disabling for opaque external inputs, linker/archive-tool bytes,
      target and toolchain identity, deterministic options, and exact stale
      attribution. Driver cache lookup regressions exercise every input,
      toolchain, target, linker/tool, and option component. `nia-linker` strict
      Rustdoc and 26 owner tests plus all 635 `nia-driver` owner tests and strict
      Clippy pass.
- [x] Cross-cutting Phase D loader Rustdoc now documents source-manifest
      identity, session sharing, source revision retirement, provider-demand
      updates, persistent frontend cache verification, and target/runtime
      request boundaries. The frontend cache owner suite also fixes the
      namespace-root regression and retains bounded corruption retirement;
      `nia-loader-query` strict Rustdoc, 94 owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase D build Rustdoc now documents runner bootstrap and
      invocation ownership, logical plan identities, semantic freeze and
      canonical codec boundaries, action-cache outcomes, deterministic
      scheduling, atomic plan handoff, and recoverable output publication.
      The existing boundary matrix covers bounded/corrupt cache records,
      truncation and trailing bytes, cross-process cache publication, stale
      locks, cancellation, process groups, and interrupted transactions;
      `nia-build` strict Rustdoc, its owner suite (211 passed, 1 ignored), and
      strict Clippy pass.
- [x] Cross-cutting Phase D compiler-query Rustdoc now documents loader fact
      ownership, provider-demand revision/fixed-point updates, compiler session
      compatibility, check/codegen entry points, completion-order backend
      finalization, source/module fingerprints, target/toolchain/runtime cache
      namespaces, product-specific cache keys, and diagnostic-store ownership.
      The 238 owner tests include randomized clean/incremental consistency plus
      persisted-product corruption and verification replacement; strict
      Rustdoc and strict Clippy pass.
- [x] Cross-cutting Phase D CLI crate documentation now records typed option
      translation, stdout/stderr and native-output ownership, toolchain/build
      dispatch, timing policy, and the process ICE boundary. Strict Rustdoc,
      three parser/ICE owner tests, the command-case matrix, and strict Clippy
      pass. The build-case matrix passed 13 cases but encountered a transient
      test-resource slot lock failure; its exact retry passed, while a clean
      whole-matrix rerun remains part of final resource evidence.
- [x] Phase D cross-process test-resource slot creation now waits through the
      brief interval in which a concurrent stale-slot reclaimer can lock the
      newly visible owner inode. The creator locks before writing its owner
      record, so normal contention is no longer reported as a fatal corrupt
      permit. A deterministic lock-order regression, all 18
      `nia-test-support` owner tests, strict Clippy, and the complete concurrent
      CLI build-case matrix (14 passed) cover the repaired boundary.
- [x] Phase D representative build baseline completed its default three
      independent samples on Linux/WSL2 with an 8.18 GB effective memory limit.
      All 21 ordered states and all 102 per-sample acceptance checks passed:
      clean and warm lookup cardinality, source and module-map typed
      invalidation, compiler-object and link reuse, corruption of five action
      entries, recovered warm reuse, and failed-action isolation. The generated
      schema-v3 evidence retained machine resources, process measurements, and
      every counter. The later schema-v5 and 3 GiB/no-swap workspace batches
      close the independent-recomputation and constrained-memory gaps recorded
      by this original run.
- [x] Phase D representative build baseline schema v5 now recomputes source-
      edited and module-map-edited fixtures in separate cold-cache workspaces,
      then compares both emitted executables byte-for-byte through fixed 64 KiB
      buffers. Its default three-sample run covered 27 ordered states and 135
      acceptance checks; all six incremental/clean comparisons matched both
      artifacts, every state had a distinct child-process PID, and the named
      `source-app` comparison records both `src/main.nia` and its mapped helper
      module. Focused acceptance and buffer-boundary tests preserve missing,
      mismatched, and falsely warm evidence. The complete 3 GiB/no-swap
      workspace run below supplies the constrained-memory evidence.
- [x] Phase D low-memory test scheduling now reuses an active compiler session's
      memory reservation for sequential emitted-runtime commands while retaining
      an independent runtime scheduling slot. The nested-session regression,
      `nia-test-support` owner suite (19 passed), strict Clippy, and focused CLI
      build cases pass; the complete constrained workspace run below verifies
      the same accounting under default libtest concurrency.
- [x] Backend dynamic-trait validation now accepts a runtime trait-object call
      when no concrete object vtable is materialized in the closed executable,
      while still validating every published candidate's slot and ABI. The
      standard-library debug error executable regression covers an empty
      formatter-object slice and its `IntoError` dynamic calls.
- [x] Backend place validation now preserves address-of and projection
      contracts for compiler-generated pointer-qualified place views. The
      process argument/iterator executable regressions that exercise indirect
      return values pass, alongside the complete 43-case process suite and
      strict Clippy for the affected crates.
- [x] Link-cache acceptance now counts persisted `.link` records separately
      from their durable per-record mutation locks. The complete linker case
      matrix passes in ordinary execution; the constrained workspace run also
      reached this matrix without resource failure before exposing a separate
      later standard-library validation regression.
- [x] Backend projection compatibility now applies readonly-view coercion
      recursively through nested pointer and slice elements. The standard
      `ArrayList` executable regression covering mutable/readonly iterator views
      passes in ordinary and 3 GiB/no-swap execution (5 passed, 822.5 MiB
      peak), preserving strict rejection of readonly-to-mutable misuse.
- [x] Phase D direct in-process `nia-driver` unit tests now retain the shared
      compiler resource session instead of bypassing it, while explicit case
      sessions are reused without double charging. Under 3 GiB/no-swap, the
      default-concurrency 635-test owner suite changed from an OOM at the exact
      3 GiB limit to 635/635 passing with a 1.1 GiB peak. The reentrant-session
      regression, 20 `nia-test-support` tests, workspace check, formatting, and
      strict Clippy for both owner crates pass.
- [x] The complete all-feature workspace test suite now passes with normal
      libtest concurrency under a 3 GiB cgroup memory limit and no swap. The
      single-job build/test run completed without an OOM and reached a 2.1 GiB
      peak, closing the constrained-workspace resource evidence that exposed
      and verified the nested-session and in-process-driver fixes above.
- [x] Phase D query-executor shutdown now closes admission before draining its
      queue and joining the worker set. A direct owner regression proves that
      dropping the final session handle completes every already accepted task;
      the stable architecture record now states this lifetime contract alongside
      nested execution and the process-wide CPU budget.
- [x] Phase D linker and archive-tool identity now streams each opened tool
      executable through a fixed 64 KiB buffer instead of allocating the full
      file. Declared-length streaming preserves the existing cache fingerprint,
      rejects truncation or growth, and retains conservative cache disabling for
      an unreadable linker. The 27 `nia-linker` owner tests, including a
      multi-buffer equivalence regression, formatting, and strict Clippy pass.
- [x] Phase D Linux host dynamic-linker discovery now reads the fixed ELF header
      and individual program headers by offset instead of allocating the entire
      host executable. Checked table arithmetic and a 4 KiB `PT_INTERP` limit
      reject truncated, out-of-file, overflowing, or oversized records. The 28
      `nia-linker` owner tests include valid and oversized interpreter fixtures;
      formatting and strict Clippy pass.
- [x] Phase D Linux default-library discovery now canonicalizes and visits each
      `ld.so.conf` input once, sorts include matches before recursion, and bounds
      individual/aggregate bytes, file count, and directory matches. Oversized
      files and include cycles cannot drive unbounded work or make link-result
      target identity depend on filesystem enumeration order. The 30
      `nia-linker` owner tests, formatting, and strict Clippy pass.
- [x] Phase D toolchain resource-manifest reads now enforce a 64 KiB stream
      budget before UTF-8 and field parsing. Metadata rejects known oversized
      files without allocation, while the `max + 1` read catches concurrent
      growth and prevents a valid compatibility prefix from being accepted.
      All five `nia-toolchain` owner tests, formatting, and strict Clippy pass.
- [x] Phase D filesystem source loading now has one 64 MiB stream budget owned
      by `nia-source` and shared by loader queries, CLI entry inspection, and
      diagnostic source recovery. Known oversized files are rejected before
      allocation, concurrent growth cannot hide behind a valid UTF-8 prefix,
      and rejected inputs are not installed in the source database. The ten
      `nia-source`, 94 `nia-loader-query`, and three CLI owner tests, affected
      strict Clippy, consumer checks, and formatting pass.
- [x] Phase D source identity normalization now preserves unresolved leading
      `..` components in relative paths while clamping absolute paths at their
      root. Parent-relative sources no longer read a different physical file or
      collide with the corresponding child-path identity. The focused matrix
      covers nested resolution, repeated parents, absolute roots, and distinct
      `SourcePath` identities, including actual parent/child file reads. All 12
      `nia-source` and 94 `nia-loader-query` tests, strict Clippy, workspace
      check, and formatting pass.
- [x] Phase D `ld.so.conf` canonical identities now serve only cycle/duplicate
      suppression; relative includes remain anchored to the visible config
      pathname, including when that file is a symlink. The Unix regression
      preserves deterministic traversal without silently changing established
      include resolution. All 31 `nia-linker` tests, strict Clippy, workspace
      check, and formatting pass.
- [x] Phase D persisted compiler signature products now have direct stale-
      identity matrices at their codec owner boundary. Valid envelopes for
      signature type resolution, signature type lowering, item signatures,
      extension validation diagnostics, executable value-reference edges, and
      check certificates reject every duplicated key, namespace, stable
      module/entry, program-source/input, signature-set, source-length, body-
      owner, and scope field independently. These tests complement each
      product's roundtrip/corruption coverage and the shared storage tests for
      atomic publication and replacement-preserving retirement. All 244
      `nia-compiler-query` owner tests pass.
- [x] Phase D loader dependency manifests now directly verify their complete
      persisted identity instead of relying only on derived cache paths. A
      valid checksummed record installed at the expected path is retired when
      its duplicated key, namespace, stable module, or source fingerprint is
      stale; the existing cross-session roundtrip, corruption recovery,
      semantic verification replacement, and shared concurrent replacement
      tests complete this product's persistence evidence. All 95
      `nia-loader-query` owner tests pass.
- [x] Phase D compiler-check and compiler-emit action caches now have direct
      owner tests for the complete corruption-replacement lifecycle. A corrupt
      exact record is retired and republished, while a delayed retirement using
      the old observed bytes preserves the newly valid entry. Compiler-emit
      additionally retains its explicit reference-binding replacement
      protocol; these tests complement canonical roundtrips, component-exact
      stale identity/invalidation matrices, and bounded oversized retirement.
      The stable maintenance rules now require publication and retirement to
      share a per-path mutation lock and revalidate the observed winner.
- [x] Phase D query/build concurrency evidence now closes two lifecycle gaps:
      deterministic parallel and cross-session cycle tests assert that the
      process-wide wait-for graph retains no edges or frames for participating
      nodes after failure, and a retirement-transaction panic test proves RAII reopens query
      admission for later work. Existing scheduler tests retain canonical
      failure-position cancellation, active-wave draining, and dependent
      suppression; query owner tests now cover 83 cases with the new lifecycle
      assertions.
- [x] Phase D representative clean/incremental evidence is now self-describing
      in build baseline schema v5. Every ordered state records the PID of its
      separately spawned `nia build` process, acceptance requires nine distinct
      state processes, and artifact comparisons separately require the
      multi-module `source-app` (entry plus mapped helper module) to match its
      independent cold-workspace recomputation. The existing source-edit and
      module-map-edit comparisons therefore provide explicit multi-module and
      cross-process evidence rather than relying on fixture inspection.
- [x] Phase D fixed acceptance is closed across all four required gates. The
      persisted-product inventory has roundtrip, corruption, complete stale-
      identity, and replacement coverage at its codec/cache owners; query and
      build lifecycle races have deterministic owner tests; schema-v5 baseline
      samples prove independent-process multi-module incremental/clean artifact
      equivalence; and the all-feature workspace passes at normal libtest
      concurrency under a 3 GiB/no-swap cgroup with a 2.1 GiB recorded peak.
      The same constrained command was rerun successfully on the schema-v5
      acceptance head, including 14/14 CLI build cases, 635/635 driver tests,
      and 83/83 query tests. Required owner crates also have strict
      Rustdoc/Clippy and focused boundary evidence recorded above; broader final
      validation remains tracked separately.
- [x] Phase A where-bound validation now preserves associated-type bindings
      through generic/method candidate filtering, final function and method
      call checks, and recursive nominal-type obligations. Concrete projection
      mismatches fail with binding-specific call diagnostics, while matching
      bindings remain accepted. Impl-based reverse inference additionally
      rejects phase products whose trait type-argument arity differs from the
      requested goal instead of accepting a truncated `zip`. Body-check owner
      regressions cover accepted and rejected bindings, nominal instantiations,
      and a deliberately malformed upstream impl signature; trait-solver owner
      tests and body-check strict Clippy remain green.
- [x] Phase A where-bound reverse inference now derives unresolved type
      parameters from the selected impl's associated type definitions, after
      substituting type and const parameters captured from the impl target.
      Target, trait arguments, const arguments, and all associated bindings are
      matched as one candidate, so a nested binding mismatch cannot leak an
      earlier partial substitution. Owner regressions cover successful
      `Source[Item = Item]` inference and rejection of `(Item, Item)` against
      `(i32, bool)`; all 252 body-check and 244 compiler-query tests plus strict
      body-check Clippy pass.
- [x] Phase A supertrait obligations now retain associated-type bindings from
      source collection through item-signature type roots, schema-v10 cache
      roundtrips, program-signature projection assumptions, body-check
      supertrait expansion, and explicit parent-impl validation. Default trait
      methods resolve a bound parent projection, an impl with the wrong parent
      associated type is rejected, and a clean/incremental edit matrix produces
      identical diagnostics. All 253 body-check, 245 compiler-query, and 636
      driver tests pass together with the owner/signature-cache suites, strict
      Clippy, and workspace checks.
- [x] Phase A body-check method inference and overload specialization now
      compare trait-object associated bindings as a complete unordered
      bijection. Candidate probes backtrack with isolated type/const
      substitutions, and one actual binding cannot satisfy multiple pattern
      bindings; obligation deduplication uses the same order-independent,
      single-consumption rule. All 253 body-check tests, 42 focused driver
      associated-type tests, and strict body-check Clippy pass.
- [x] Phase A trait-solver impl matching and ordinary-call shape filtering now
      apply the same associated-binding bijection rule. Impl selection
      backtracks transactionally to find a complete binding permutation and
      records only its winning generic substitutions; fast call filtering
      cannot reuse one actual binding to admit a malformed candidate. Direct
      solver regressions cover both backtracking and rejected reuse; all 11
      trait-solver and 253 body-check tests plus strict affected Clippy pass.
- [x] Phase A/B backend extension instantiation now preserves the frontend's
      associated-binding set semantics. Trait-object patterns backtrack over a
      complete one-to-one binding permutation before committing generic
      substitutions, and backend type equality cannot reuse the first matching
      key for duplicate bindings. All 112 backend-lower and 264 LLVM codegen
      owner/consumer tests plus strict backend-lower Clippy pass.
- [x] Phase A trait objects now inherit associated-type equalities declared by
      their supertrait graph. A bare `&Child` for
      `Child : Parent[Item = i32]` is object safe for parent projection methods
      and upcasts to `&Parent[Item = i32]`; a `bool` target and an unrelated
      parent binding remain rejected. The traversal substitutes child type and
      const arguments, is cycle guarded, and feeds object safety, dynamic call
      normalization, and upcast validation. Clean/incremental edits agree, and
      all 253 body-check, 246 compiler-query, 638 driver, and 265 LLVM codegen
      tests plus strict affected Clippy pass.
- [x] Phase A supertrait validation now expands the complete inherited
      associated-binding graph and rejects conflicting equalities for the same
      parent trait instance. Repeated constraints with the same right-hand side
      remain valid, including diamond inheritance; distinct right-hand sides
      produce a binding-specific diagnostic. Direct and diamond conflict
      regressions pass with 639 driver tests, 246 compiler-query tests, the
      workspace check, formatting, and strict affected Clippy.
- [x] Phase A trait-object supertrait reachability now uses a path-local cycle
      guard. DFS nodes are removed on every return, including missing-signature
      diagnostics, so a failed branch cannot suppress a later sibling path.
      Body-check (253), focused supertrait driver (23), workspace check, and
      strict body-check Clippy pass.
- [x] Phase A trait-object upcast binding validation now performs a complete
      one-to-one backtracking match instead of committing the first compatible
      source binding. This keeps upcast identity aligned with body-check,
      solver, specialization, and backend binding-set semantics. Body-check
      and four focused driver upcast tests pass.
- [x] Phase A trait-object upcast coverage now includes a diamond graph whose
      later branch contributes the only associated-type binding. The regression
      proves that an earlier unbound sibling cannot hide the later `Item = i32`
      constraint; the focused driver matrix passes alongside the existing
      body-check and workspace gates.
- [x] Phase A trait-witness visibility now uses the canonical module-graph
      visibility predicate for direct imported traits, including `pub(pkg)`.
      Same-package trait impl witnesses remain usable while package-external
      callers are rejected; the regression matrix passes with 641 driver tests,
      246 compiler-query tests, and strict affected Clippy.
- [x] Phase A trait-object object-safety validation now rejects source traits
      with builtin `Sized` supertraits. An erased object cannot satisfy the
      statically-known-layout requirement; the body-check owner reports the
      object-safety diagnostic before vtable construction. The focused driver
      regression passes with 642 driver tests, 253 body-check tests, and 246
      compiler-query tests.
- [x] Phase A trait-object object-safety validation now rejects source traits
      whose builtin supertraits expose methods or associated items without a
      defined object-level vtable contract. `Iterator` is covered by a focused
      rejection regression; marker-only builtin bounds retain their existing
      semantics. Driver, body-check, query, workspace, formatting, and strict
      Clippy gates remain green.
- [x] Phase A trait-object semantic instantiation facts now traverse the complete
      source-supertrait graph used by backend vtable expansion. Each inherited
      default method receives substituted type/const arguments, while a
      path-local `(trait, args, const_args)` guard terminates recursive graphs;
      builtin supertraits remain outside this source-vtable traversal because
      unsupported object-level contracts are rejected earlier. The body-check
      regression covers a const-generic inherited default method and all 254
      body-check tests pass.
- [x] Phase A trait-object semantic instantiation collection now deduplicates
      repeated concrete source-supertrait instances reached through diamond
      inheritance while retaining the path-local cycle guard. Backend vtable
      slot expansion remains path-shaped, but frontend dependency facts contain
      one generic instance per concrete method identity; a const-generic
      diamond regression verifies four identities rather than recording the
      shared base twice.
- [x] Phase A trait-object object-safety traversal now carries declaration-order
      type and const substitutions into inherited method and supertrait checks.
      Its cycle identity includes the complete trait instance, while a separate
      expanded set avoids duplicate diagnostics/work on diamond siblings. A
      const-generic inherited-supertrait regression covers the corrected owner;
      body-check and focused driver tests remain green.
- [x] Phase A object-safety type reconstruction now preserves nominal const
      arguments (including their argument types) while recursively normalizing
      nested object-safe types. This keeps erased method signatures aligned with
      the same const-bearing nominal identity used by trait-object bindings and
      vtable lowering; body-check, driver, query, and workspace gates remain
      green.
- [x] Phase A where-bound candidate substitution now preserves nominal const
      arguments and recursively substitutes their argument types instead of
      rebuilding every nominal as type-only. Existing const-generic where-bound
      regressions for integer, bool, and char identities pass alongside the full
      body-check and driver suites.
- [x] Phase B backend trait-method fallback and concrete-implementation checks
      now retain trait const arguments through default-method self selection and
      source trait-goal solving, and default `FunctionCallee` payloads preserve
      those arguments as well. Static const-generic default dispatch therefore
      uses the same complete identity as frontend facts and vtable lowering;
      the const-generic default-method codegen regression and 112 backend-lower
      tests pass.
- [x] Phase B backend trait-method fallback now carries the same const identity
      through operator-dispatch fallback paths as ordinary function-body
      instantiation. Both method and associated-function fallback callees retain
      substituted trait const arguments after solver selection, closing the
      parallel consumer boundary under the existing const-generic dispatch
      regression and backend-lower suite.
- [x] Phase A source trait-object validation now rejects associated values/consts
      when no object-level vtable contract exists. Source vtables contain method
      slots only, so accepting an erased trait with an associated value would
      make the object type promise an unmaterialized item; a focused driver
      regression covers the diagnostic while marker-only builtin policy remains
      unchanged.
- [x] Phase B monomorphization type-depth accounting now traverses const
      argument types in nominal values, trait objects, projections, and the
      concrete instance enqueue gate. Deep type structure carried only by a
      `ConstGenericArg::ty` can no longer bypass the convergence limit; a direct
      recursive-instance regression covers the previously unbounded path.
- [x] Phase B layout-root collection now retains const argument types from
      generic instantiations, trait objects, associated bindings, projections,
      and nominal identities. These types can own aggregates that require layout
      materialization even when their const values are scalar; a focused query
      regression covers trait-object and binding const metadata roots.
- [x] Phase B backend recursive type filters and aggregate instance collectors
      now traverse `ConstGenericArg::ty` alongside ordinary type arguments.
      Function-instance admission covers generic, projection, error, and depth
      checks, while top-level type registration and struct/union discovery
      retain const metadata roots for nested aggregate materialization;
      backend-lower's focused regression and full 113-test suite pass.
- [x] Phase B LLVM declaration readiness now retains owners of unevaluated
      `ConstGenericValue::ConstExpr` values embedded in nominal, trait-object,
      associated-binding, and projection metadata. Backend IR validation also
      recursively checks every const-argument type; the declaration-membership
      owner regression and codegen validation gates pass.
- [x] Phase C executable reachability's type-only owner projection now retains
      modules referenced by `ConstGenericValue::ConstExpr` and
      `ArrayLenTy::ConstExpr`, including generic instantiations, function
      references, trait calls, nominal/object/projection types, and associated
      bindings. A direct fact-owner regression covers both metadata forms.
- [x] Phase C semantic-input const-expression pruning now traverses trait-object
      and pointee const args, associated-binding type/const args, and projection
      const args, including recursively nested argument types. A direct query
      provider regression proves these trait metadata positions retain their
      active `GlobalConstExprId` inputs.
- [x] Phase C extension-trait signature type-module discovery now retains source
      trait/nominal owners, const-expression owners from nominal/object/binding/
      projection metadata, and array-length expression owners. A focused query
      provider regression covers nested trait metadata and both const-expression
      owner forms before provider expansion.
- [x] Phase A extension-trait signature discovery now also retains a source
      trait referenced only by an associated-type binding identity. Binding
      type traversal cannot expose that non-type owner, so the focused owner
      regression uses a distinct module to keep this dependency explicit.
- [x] Phase C closure escape analysis now has a compiler-query regression for
      mutually recursive functions. The fixed-point summaries converge across
      both call edges and preserve a returned stack-backed callable diagnostic;
      the focused `nia-compiler-query` test passes.
- [x] Phase C early const iteration now has an owner-level regression proving
      that early IR evaluates the iterable expression but stops with the
      witness-dispatch boundary diagnostic instead of silently executing a
      `for-in` body. Architecture documentation records that only resolved
      const IR may drive semantic `Iterable`/`Iterator` witnesses.
- [x] Phase C resolved const iteration now has direct evaluator regressions for
      iterator-state writeback, terminal `null` observation, per-item lexical
      scopes, and binding-error rollback. Both normal exhaustion and a failed
      item binding restore the enclosing evaluation environment.
- [x] Phase C const dependency-cycle recovery now has owner and compiler-query
      regressions proving that cyclic members produce no values while a later
      independent initializer still publishes its value and typed fact. Query
      diagnostics remain separate from that usable semantic product.
- [x] Cross-cutting `nia-ids` Rustdoc now documents session-owned module and
      definition identities, qualified type-store handles, visibility/trait
      identity forms, and builtin type anchors. Strict rustdoc now reaches the
      next undocumented registry section (`BuiltinTrait`) without suppressing
      the remaining backlog.
- [x] Cross-cutting `nia-ids` Rustdoc now documents builtin operator/iteration
      trait identities, value-builtin identity, and target-configuration
      const-value identity. Strict rustdoc advances to `BuiltinFunction`, with
      the remaining registry backlog still explicit.
- [x] Cross-cutting `nia-ids` Rustdoc now documents builtin functions, layout
      queries, trait method signatures, receiver modes, associated members, and
      supertrait descriptors. `cargo rustdoc -p nia-ids --lib -- -D
      missing-docs` now passes for the complete crate.
- [x] Cross-cutting `nia-diagnostic` Rustdoc now documents the registered code
      catalog, diagnostic schema/builder/report APIs, stable bundle codec, and
      store-qualified bundle lifecycle. `cargo rustdoc -p nia-diagnostic --lib
      -- -D missing-docs` passes for the complete crate.
- [x] Cross-cutting `nia-ast-walk` Rustdoc now documents the structural Visitor
      callback contract and every public `walk_*` entry point. Strict rustdoc
      passes for the complete traversal crate without embedding semantic policy.
- [x] Cross-cutting `nia-syntax` Rustdoc now documents the lossless green/red
      tree, token cursor, source-versioned identities, text edits, and partial
      reparse boundary. `cargo rustdoc -p nia-syntax --lib -- -D missing-docs`,
      the 11-case owner suite, and strict Clippy pass for the complete crate.
- [x] Cross-cutting `nia-parser` Rustdoc now documents the parser crate contract
      and structured parse-error fields. `cargo rustdoc -p nia-parser --lib --
      -D missing-docs`, all 118 parser tests, and strict Clippy pass.
- [x] Cross-cutting `nia-symbol` Rustdoc now documents stable symbol identities,
      resolver helpers, builtin conversion, and the generated well-known symbol
      registry. Parameterized macro docs cover every exported constant;
      `cargo rustdoc -p nia-symbol --lib -- -D missing-docs`, both owner tests,
      and strict Clippy pass.
- [x] Cross-cutting `nia-ice` Rustdoc now documents structured ICE capture,
      panic-location handling, diagnostic conversion, and actionable rendering.
      `cargo rustdoc -p nia-ice --lib -- -D missing-docs`, all four owner tests,
      and strict Clippy pass.
- [x] Cross-cutting `nia-ast` Rustdoc now documents the complete expression,
      statement, pattern, item, and type syntax model, including generic
      type-or-const ambiguity and declaration-identity helpers. Strict rustdoc,
      the (currently empty) owner libtest suite, and strict Clippy pass.
- [x] Cross-cutting `nia-pattern-analysis` Rustdoc now documents canonical
      pattern/domain models and all validation errors. Strict rustdoc, all 13
      usefulness/exhaustiveness tests, and strict Clippy pass.
- [x] Cross-cutting `nia-opt` Rustdoc now documents the complete user-level to
      pass-policy matrix, including required work, pass depth, inlining,
      specialization, deduplication, and size preference. Strict rustdoc, all
      three policy tests, and strict Clippy pass.
- [x] Cross-cutting `nia-sema` Rustdoc now documents diagnostic-neutral array
      length, arity, and field-set checks and their structured result schemas.
      Strict rustdoc, all three owner tests, and strict Clippy pass.
- [x] Cross-cutting `nia-test-support` Rustdoc now documents temporary directory
      ownership, weighted compiler/build and runtime resource sessions, command
      timeout helpers, and consuming case-manifest/path validation. Strict
      rustdoc, all 20 resource/fixture tests, and strict Clippy pass.
- [x] Cross-cutting `nia-timing` Rustdoc now documents allocator registration,
      live-byte snapshots/windows, timing modes/options, event emission,
      aggregation, collector serialization, and stderr reports. Strict rustdoc,
      all 13 collector/accounting tests, and strict Clippy pass.
- [x] Cross-cutting `nia-lexer` Rustdoc now documents significant/lossless token
      schemas, every literal/keyword/punctuation/operator kind, recoverable
      lexical errors, tokenizer construction, and terminal EOF streams. Strict
      rustdoc, all 10 lexer tests, and strict Clippy pass.
- [x] Cross-cutting `nia-item-signatures` Rustdoc now documents module
      declaration signatures, program wrappers, generic type/const substitution,
      trait implementation schemas/indexing, and collector inputs. Strict
      rustdoc, all 12 signature/builtin/type-store contract tests, and strict
      Clippy pass.
- [x] Cross-cutting `nia-program-signatures` Rustdoc now documents global
      signature lookup/context products, module-qualified fact collection,
      extension validation/indexing, and visibility-aware trait-witness closure.
      Strict rustdoc, both owner tests, and strict Clippy pass; architecture
      records that this layer indexes existing declaration facts without
      reparsing source or owning body semantics.
- [x] Cross-cutting `nia-sema-ir` Rustdoc now documents semantic-use tables,
      module versus function fact ownership, node-store freeze/merge/rehoming,
      builtin values and coercions, generic instantiations, and every resolved
      call dispatch identity. Strict rustdoc, all six owner tests, and strict
      Clippy pass; a focused regression proves module-fact retention removes
      only function-owned staging entries while preserving both owner products.
- [x] Cross-cutting `nia-defs` Rustdoc now documents stable structural
      definition identities, namespace/member products, reduced public-surface
      facts, source-preserving using schemas, and extension declaration,
      callability, and trait-witness indexes. Extension visibility iteration now
      deduplicates repeated imported/current module inputs at the owner boundary;
      all eight owner tests, both `nia-program-signatures` consumer tests,
      strict rustdoc, and strict Clippy pass.
- [x] Cross-cutting `nia-driver` Rustdoc now documents inspection outputs,
      diagnostic and optimization report adapters, request builders, source
      manifest pairing, cache environment/reference wire schemas, artifact
      ownership, `DriverOutput`, and structured `DriverError` categories. Strict
      rustdoc, all 645 driver owner tests, strict Clippy, and formatting pass;
      architecture records that orchestration owns request/cache/artifact
      boundaries while semantic meaning remains in query and analysis crates.
- [x] Phase C `nia-flow-check` reachable filtering now has an owner regression
      keyed by the complete `(ModuleId, DefId)` identity. A reachability-pruned
      function is skipped as a whole, so its missing-return and duplicate-pattern
      diagnostics cannot enter the executable product while the selected
      function remains checked. The owner suite (20 tests), strict Rustdoc, and
      strict Clippy pass; architecture records eager/short-circuit, loop, defer,
      and closure control-flow conservatism.
- [x] Phase C executable-facts collection now preserves concrete generic and
      const arguments when a `TypedExprKind::FunctionInstance` is used as a
      value, matching the direct generic-callee path. The owner regression checks
      the exact `GenericInstantiation`; a compiler-query backend regression
      proves `&identity[i32]` reaches the concrete function-instance plan and
      final Backend IR.
- [x] Phase C static executable-fact collection now consumes the complete
      `StaticInit::value_refs` identity, preserving generic, const, and receiver
      arguments for function-pointer values stored in globals. The owner
      regression checks the `AddrOfFunction` mapping, and a compiler-query
      regression proves a static `&identity[i32]` reaches the concrete backend
      instance plan and final Backend IR.
- [x] Phase C facts-only static initializer summaries now retain the same
      complete `FunctionBodyRefs` identity as typed static IR, instead of
      reducing generic function addresses to bare functions. The body-check
      owner regression checks the facts-only product, while compiler-query
      conversion consumes the shared summary shape for reachability.
- [x] Batch 316 preserves const-generic function-pointer identities through
      static initializer IR, backend instance planning, LLVM validation and
      lookup, and fingerprints. Owner regressions cover typed static refs and
      facts-only body refs; compiler-query coverage verifies the const argument
      survives executable static init, backend planning, and final Backend IR.
- [x] Batch 317 preserves method and trait-method const-generic identities from
      semantic calls through Body IR, executable facts, reachability, backend
      dispatch, function-instance planning, and LLVM fingerprints. Owner tests
      cover a direct const-generic extension method; compiler-query coverage
      verifies the method const argument survives planning and final Backend IR.
- [x] Batch 318 closes the parallel typed-IR executable-facts path for method
      identities. Direct methods, trait methods, and trait-associated functions
      now retain their own type/const arguments when typed callees are scanned,
      matching semantic-fact extraction; the owner regression covers direct and
      trait method const arguments.
- [x] Batch 319 closes the Function IR value-reference gap for unresolved trait
      callees. Trait methods and trait-associated functions now retain method
      const-argument type roots during pre-instantiation discovery; concrete
      instance identities remain deferred until backend trait resolution can
      supply the implementation module and complete `FunctionInstanceKey`.
- [x] Batch 320 preserves receiver identity in executable generic trait
      closure deduplication. Reachability now keeps `self_arg` in its visited
      generic-instantiation key, so otherwise equal method type/const arguments
      with different receivers each expand their own trait predicates and
      default-method witnesses.
- [x] Batch 321 closes the LLVM extern-method instance predicate. The ABI
      classification and extern lookup paths now treat method const arguments
      as sufficient to select `FunctionInstance` metadata, matching type and
      receiver substitutions; the owner regression covers the const-only case.
- [x] Batch 322 closes the remaining LLVM extern-method lookup bypass. The
      extern predicate now forwards the method's complete const-argument vector
      when selecting instance metadata; a handcrafted Backend IR regression
      verifies a const-only extern method keeps the C ABI declaration and call.
- [x] Batch 323 preserves const identity for extension-method function pointers.
      Body-check resolution now carries target type and const substitutions into
      function references, BIR keeps const-only references as concrete instances,
      and backend templates/instances canonicalize const arguments so the
      source-visible receiver signature matches the emitted LLVM function.
- [x] Batch 324 closes the supertrait obligation cycle-guard alias bypass.
      Supertrait expansion now compares visited goals through semantic
      obligation equivalence rather than raw interned type handles; the diamond
      regression routes one branch through a generic type alias and still emits
      one coherent concrete obligation set.
- [x] Batch 325 aligns trait-object supertrait traversal with semantic cycle
      guards. Binding collection and target-supertrait checks now compare
      normalized type arguments and const arguments semantically, so aliases do
      not reopen a visited path or produce order-dependent upcast metadata.
- [x] Batch 326 closes the trait-solver layout fallback const-identity bypass.
      Nominal layout keys now compare const argument types and values
      semantically across module-owned interners and integer signedness, with an
      owner regression proving `Sized` lookup finds the equivalent layout.
- [x] Batch 327 closes the trait-solver active-goal identity bypass. Recursive
      where-clause resolution now tracks a semantic goal path instead of a raw
      `HashSet<TraitGoal>`; a cycle regression uses equivalent signed/unsigned
      const spellings and still rejects the non-inductive proof.
- [x] Batch 328 closes the associated-type projection cycle-key identity bypass.
      Projection recursion now tracks a path-local vector and compares the full
      normalized goal semantically, including type and const arguments. An owner
      regression proves equivalent signed/unsigned projection goals cannot reopen
      an active path.
- [x] Batch 329 closes the body-check projection-normalization identity bypass.
      Frontend projection normalization now uses a path-local semantic key rather
      than raw interned handles and const hashing. A recursive associated-type
      normalization regression confirms equivalent projection paths terminate.
- [x] Batch 330 closes the program-signatures supertrait-assumption identity
      bypass. Source supertrait expansion now guards the complete `TraitGoal`
      semantically, including `self_ty` and const values, and restores path-local
      sibling traversal. Signature type equivalence also compares integer const
      values by semantic bits across module-owned interners.
- [x] Batch 331 closes the trait-solver equivalence active-pair identity bypass.
      Projection-resolving type equivalence now tracks a path-local pair vector
      and compares non-resolving structural shapes semantically, including const
      values. An owner regression covers equivalent rebuilt projection pairs and
      verifies the caller's active path remains balanced.
- [x] Batch 332 closes the body-check trait-object traversal identity bypass.
      Vtable and object-safety guards now compare complete trait instances by
      structural type and semantic const identity, while dynamic method search
      adds a semantic expanded set alongside its path-local guard. A diamond
      regression with equivalent signed/unsigned const spellings proves dynamic
      dispatch remains unambiguous and records one generic vtable instance.
- [x] Batch 333 closes the backend supertrait-vtable traversal identity bypass.
      Backend type matching now compares nested nominal, trait-object,
      projection, and associated-binding const arguments semantically. Vtable
      expansion retains its path-local recursion guard and adds a semantic
      expanded set, while exact cache/output keys remain unchanged. A codegen
      diamond regression proves equivalent const spellings emit one inherited
      trait segment and stable slots.
- [x] Batch 334 closes the backend extension-candidate const identity bypass.
      Concrete impl candidate matching now canonicalizes const arguments,
      compares their types semantically, and compares integer values by bits;
      generic pattern parameters remain wildcards. This keeps extension and
      trait-resolution selection aligned with backend vtable identity rules.
- [x] Batch 335 closes the backend extension type-pattern identity bypass.
      Nominal, trait-object, trait-object-pointee, projection, and associated
      binding pattern branches now use the same semantic const-pattern relation
      instead of raw const vectors. The extension-specificity regression now
      runs through codegen, proving backend selection sees normalized const
      expressions as the same concrete instance.
- [x] Batch 336 closes the executable-reachability const witness identity
      bypass. Cross-store extension pattern matching and repeated const
      substitutions now compare integer values by semantic bits while retaining
      typed structural comparison; unresolved const-expression IDs remain exact
      because this phase has no evaluator. A reachability regression accepts
      equivalent signed/unsigned integer spellings but still rejects conflicting
      repeated values.
- [x] Batch 337 closes the LLVM vtable validation const identity bypass. Vtable
      payload validation now reuses the backend semantic const-argument helper
      instead of raw value equality. The malformed-IR regression uses equivalent
      signed/unsigned integer payload spellings and still reports independent
      slot and missing-function errors without a false trait-argument mismatch.
- [x] Batch 338 closes the LLVM dynamic-slot const identity bypass. Dynamic-call
      validator matching and module-codegen vtable slot/upcast lookup now use
      semantic integer const comparison instead of raw values. Existing malformed
      dynamic-call coverage plus a focused slot-matcher regression prove slot
      selection remains stable for equivalent signed/unsigned spellings.
- [x] Batch 339 closes the remaining LLVM aggregate/function-instance fallback
      const identity bypasses. Layout, struct/union instance, and function
      instance fallback scans now share one semantic const matcher, as do
      aggregate/layout/static-initializer validation fallbacks, while exact
      cache keys and declaration ownership remain unchanged. Backend function
      reference validation uses the same fallback relation, and the existing
      shared matcher regression covers signed/unsigned integer equivalence.
- [x] Batch 340 closes the program-signatures projection-context identity
      bypass. Impl-context projection substitution now compares self/type/const
      arguments structurally, including cross-module rebuilt handles and integer
      bits, while retaining exact declaration and cache identity. An owner test
      covers equivalent signed/unsigned const arguments across two type-store
      appenders.
- [x] Batch 341 closes the backend-lower projection active-guard identity
      bypass. Associated-type instantiation now uses a path-local semantic stack
      instead of a raw `HashSet<ProjectionInstantiationKey>`, so equivalent
      rebuilt handles and integer const spellings cannot reopen recursion while
      sibling projections remain independent. Exact type-instantiation cache
      keys are unchanged.
- [x] Batch 342 closes the monomorphization projection active-guard identity
      bypass. Monomorphization now uses a path-local semantic projection stack
      with structural type comparison and integer-bit const matching, while
      unresolved const-expression ids remain exact. Exact `MonoInstanceKey`
      deduplication, symbols, and type-instantiation cache keys remain unchanged;
      an owner regression covers rebuilt cross-module handles and signed/unsigned
      equivalent const spellings.
- [x] Batch 343 closes the backend-lower vtable-owner payload identity bypass.
      Exact `(self type, object type)` vtable owner keys remain unchanged, while
      duplicate payload validation compares nested type arguments structurally
      and integer const values by bits. Function/module, method/slot, and
      unresolved const-expression identities remain exact; a focused owner
      regression covers signed/unsigned equivalent payload spellings.
- [x] Batch 344 closes the LLVM declaration-membership const identity bypass.
      Function and global instance definition detection now reuses module-codegen
      semantic type/const matching for receiver and generic payloads, while
      exact definition and argument-module identities remain unchanged. This
      prevents semantically equivalent const spellings from becoming false
      external declarations; the full LLVM unit suite and strict Clippy pass.
- [x] Batch 345 closes the program-signatures trait-impl deduplication const
      identity bypass. Associated-projection assumptions and trait-impl
      candidate checks now reuse `const_args_equivalent_in_store`, preserving
      exact unresolved expression identities while matching integer values by
      semantic bits. Program-signatures tests and strict Clippy pass.
- [x] Batch 346 closes the backend extension-specificity const identity bypass.
      Candidate subsumption now canonicalizes concrete const arguments, compares
      their types semantically, and compares integer values by bits while
      retaining generic-parameter wildcards and exact unresolved expressions.
      Backend-lower tests and strict Clippy pass.
- [x] Batch 347 closes the executable-reachability trait-closure guard identity
      bypass. Recursive supertrait expansion now uses a path-local guard that
      includes receiver, type, and const arguments with structural type and
      integer-bit comparison. Exact reachable method/vtable identities remain
      unchanged; rebuilt-handle/integer-equivalence regression and executable
      reachability consumer tests pass.
- [x] Batch 348 closes the body-check const-pattern identity bypass. Method and
      trait-object pattern matching now compares const types structurally and
      integer values by bits, while preserving generic wildcards and exact
      unresolved expression IDs. Repeated inferred substitutions use the same
      relation; the full body-check suite and strict Clippy pass.
- [x] Batch 349 closes the LLVM struct-GEP safety boundary and removes an
      unused pointer-difference API. `Builder::build_struct_gep` is now an
      explicit unsafe wrapper whose Rustdoc requires the actual struct layout,
      field bounds, and pointer provenance; every codegen caller enters it only
      after backend projection checks. The uncalled `build_ptr_diff` wrapper and
      its FFI import were deleted instead of retaining an unverifiable API.
      LLVM/codegen tests, workspace check, formatting, and strict Clippy pass.
- [x] Batch 350 closes the LLVM aggregate extract/insert safety boundary.
      `Builder::build_extract_value` and `build_insert_value` now inspect the
      physical aggregate type, reject out-of-range struct/array indices, and
      reject insertion values whose LLVM type differs from the selected field
      before entering the FFI. Malformed-index and type-mismatch wrapper tests
      plus the full LLVM/codegen suites, workspace check, and strict Clippy pass.
- [x] Batch 351 closes the LLVM select/vector builder type boundary. Select
      conditions now require scalar `i1` or shape-matched `i1` vectors with
      identical arms; vector extract/insert operations validate integer indices
      and element types; shuffle inputs must match and masks must use `i32`
      lanes. Wrapper regressions cover arm, element, and mask mismatches, and
      the full LLVM/codegen suites, workspace check, and strict Clippy pass.
- [x] Batch 352 closes the LLVM integer cast width boundary. Scalar and vector
      `zext`/`sext`/`trunc` wrappers now validate integer lane shapes and widths,
      requiring wider extension targets and narrower truncation targets before
      LLVM receives the operation. Regression tests cover invalid same-width
      extension and widening truncation; LLVM/codegen suites, workspace check,
      formatting, and strict Clippy pass.
- [x] Batch 353 removes three unused LLVM memory intrinsic wrappers. The dead
      `build_memcpy`, `build_memmove`, and `build_memset` APIs and their FFI
      imports were deleted rather than retaining unchecked alignment,
      provenance, and size preconditions. Codegen continues through its
      validated byte-loop implementation; LLVM/codegen suites, workspace
      check, formatting, and strict Clippy pass.
- [x] Batch 354 closes the LLVM switch-case boundary. `Builder::build_switch`
      now validates all case values before instruction creation, requiring
      constant integers with exactly the selector's LLVM type. Wrapper tests
      cover mismatched types and runtime values; LLVM/codegen suites, workspace
      check, formatting, and strict Clippy pass.
- [x] Batch 355 closes the LLVM atomic builder boundary. Atomic RMW now rejects
      unordered/non-atomic orderings and invalid operand kinds for each opcode;
      compare-exchange validates matching expected/desired types and legal
      success/failure ordering combinations before FFI entry. Wrapper tests
      cover a floating RMW operand and invalid cmpxchg ordering; LLVM/codegen
      suites, workspace check, formatting, and strict Clippy pass.
- [x] Batch 356 closes the LLVM fence ordering boundary. `build_fence` now
      rejects unordered, monotonic, and non-atomic orderings before
      `LLVMBuildFence`, matching the backend validator's acquire/release,
      acquire-release, and sequentially-consistent contract. A malformed
      monotonic-fence regression plus LLVM/codegen suites, workspace check,
      formatting, and strict Clippy pass.
- [x] Batch 357 closes the LLVM call-argument boundary. `build_call` and
      `build_indirect_call` now validate fixed arity, variadic minimum arity,
      and every fixed argument's LLVM type against the `FunctionType` before
      entering `LLVMBuildCall2`; opaque indirect-callee provenance remains in
      backend validation. Wrapper regressions cover fixed-arity, fixed-type,
      and variadic-underflow failures, with LLVM/codegen suites, workspace
      check, formatting, and strict Clippy passing.
- [x] Batch 358 closes the LLVM bitcast type boundary. `build_bit_cast` now
      requires first-class source/target types, equal known widths, matching
      fixed/scalable vector classes, and matching pointer address spaces before
      `LLVMBuildBitCast`; target-dependent pointer widths remain outside this
      wrapper. Regressions cover a mismatched-width cast and the SIMD bool-mask
      packing path, with LLVM/codegen suites, workspace check, formatting, and
      strict Clippy passing.
- [x] Batch 359 removes non-first-class type handles from the safe LLVM builder
      surface. Alloca, load, volatile-load, phi, and bitcast destinations now
      require the existing `BasicType` marker, so void/function signature types
      cannot enter those instructions; the codegen place-load helper adopts the
      same bound. LLVM/codegen suites, workspace check, formatting, and strict
      Clippy pass with no compatibility shim retained.
- [x] Batch 360 closes the LLVM conditional-branch condition boundary.
      `build_conditional_branch` now requires a scalar one-bit integer condition
      before `LLVMBuildCondBr`; the broad `IntValue` handle is no longer treated
      as boolean proof. A non-`i1` regression plus LLVM/codegen suites,
      workspace check, formatting, and strict Clippy pass.
- [x] Batch 361 closes the LLVM return-shape boundary. `build_return` now
      derives the enclosing function signature from the insertion block and
      rejects void/value mismatches, missing non-void values, and mismatched
      return LLVM types before `LLVMBuildRet` or `LLVMBuildRetVoid`. Wrapper
      regressions cover all three malformed shapes, with LLVM/codegen suites,
      workspace check, formatting, and strict Clippy passing.
- [x] Batch 362 closes the LLVM binary-operand boundary. Typed and shared
      integer/floating arithmetic, bitwise, shift, and comparison builders now
      require identical operand LLVM types and the correct integer/floating
      scalar-or-vector category before FFI entry; shared integer equality and
      inequality retain LLVM's valid same-address-space pointer comparison
      contract. Regressions cover a scalar width mismatch, an integer value
      routed to floating arithmetic, and pointer equality, with LLVM/codegen
      suites, workspace check, formatting, and strict Clippy passing.
- [x] Batch 363 closes the LLVM unary-operand boundary. Shared integer
      negation/bitwise-not and floating negation now validate scalar/vector
      operand categories before FFI entry; typed unary builders retain their
      existing narrow value handles. Regressions cover both cross-category
      failures, with LLVM/codegen suites, workspace check, formatting, and
      strict Clippy passing.
- [x] Batch 364 closes the typed LLVM floating-comparison boundary.
      `build_float_compare` now reuses the binary operand validator so both
      operands must have identical floating scalar/vector LLVM types before
      `LLVMBuildFCmp`; a precision-mismatch regression plus LLVM/codegen suites,
      workspace check, formatting, and strict Clippy pass.
- [x] Batch 365 closes the LLVM CFG-target boundary. Unconditional/conditional
      branches and switches now require every destination block to belong to
      the current insertion block's function before FFI entry. A cross-function
      target regression plus LLVM/codegen suites, workspace check, formatting,
      and strict Clippy pass.
- [x] Batch 366 removes the unchecked LLVM phi-incoming API. `PhiValue::add_incoming`
      is now fallible, requiring incoming values to match the phi type and
      incoming blocks to belong to the phi's function; the logical-expression
      codegen caller propagates failures. Type and cross-function block
      regressions plus LLVM/codegen suites, workspace check, formatting, and
      strict Clippy pass.
- [x] Batch 367 closes the LLVM global-initializer type boundary.
      `GlobalValue::set_initializer` is now fallible and compares the value's
      LLVM type with the declared global type before `LLVMSetInitializer`; all
      codegen callers propagate wrapper failures. A mismatched-initializer
      regression plus LLVM/codegen suites, workspace check, formatting, and
      strict Clippy pass.
- [x] Batch 368 removes the ignored LLVM global address-space argument.
      `Module::add_global` now calls `LLVMAddGlobalInAddressSpace` when an
      address space is supplied, and `PointerType::address_space` exposes the
      resulting value. An address-space preservation regression plus
      LLVM/codegen suites, workspace check, formatting, and strict Clippy pass.
- [x] Batch 369 closes the LLVM alignment boundary. Global/instruction
      alignment setters and aligned loads now require a non-zero power-of-two
      byte alignment and return `LlvmResult`; codegen propagates setter failures.
      Regressions cover invalid global and load alignments, with LLVM/codegen
      suites, workspace check, formatting, and strict Clippy passing.
- [x] Batch 370 closes the LLVM named-struct constant boundary.
      `StructType::const_named_struct` is now fallible and validates physical
      field count plus every value's LLVM type before `LLVMConstNamedStruct`;
      all codegen callers propagate wrapper diagnostics. Wrapper regressions
      cover count and field-type mismatches, with LLVM/codegen suites,
      workspace check, formatting, and strict Clippy passing.
- [x] Batch 371 closes the LLVM typed constant-array boundary.
      Integer, floating, pointer, struct, and nested-array constructors now
      validate every element's LLVM type before `LLVMConstArray2` and return
      `LlvmResult`; static and vtable initialization propagate failures.
      Wrapper regressions cover an element-type mismatch, with LLVM/codegen
      suites, workspace check, formatting, and strict Clippy passing.
- [x] Batch 372 closes the LLVM instruction-flag opcode boundary.
      Volatile setters now accept only load/store/atomic-RMW/compare-exchange
      instructions, weak setters only compare-exchange, and both return
      `LlvmResult`; builder callers propagate failures. Wrapper regressions
      cover invalid integer-add targets. LLVM/codegen suites, workspace check,
      formatting, and strict Clippy pass.
- [x] Batch 373 closes the LLVM atomic-ordering setter boundary.
      `InstructionValue::set_atomic_ordering` now validates load/store
      direction, atomic-RMW and compare-exchange ordering classes, and the
      current compare-exchange failure relationship before `LLVMSetOrdering`;
      atomic codegen callers propagate failures. Wrapper regressions cover an
      acquire store and an incompatible compare-exchange update, with
      LLVM/codegen suites, workspace check, formatting, and strict Clippy pass.
- [x] Batch 374 closes the LLVM named-struct body boundary.
      `StructType::set_body` is now fallible and checks LLVM opaque/literal
      state, allowing only one definition of a named opaque struct before
      `LLVMStructSetBody`; declaration lowering propagates failures. Wrapper
      regressions cover literal and redefinition attempts, with LLVM/codegen
      suites, workspace check, formatting, and strict Clippy passing.
- [x] Batch 375 removes the LLVM vector-constant error classification bypass.
      `VectorType::const_vector` now reports lane-count and lane-type
      mismatches as `LlvmError::Error` and propagates element type-inspection
      failures instead of collapsing them into ICEs. Wrapper regressions cover
      both shape errors, with LLVM/codegen suites, workspace check, formatting,
      and strict Clippy passing.
- [x] Batch 376 closes the LLVM instruction-alignment opcode boundary.
      `InstructionValue::set_alignment` now accepts only alloca, load, store,
      atomic-RMW, and compare-exchange opcodes before `LLVMSetAlignment`;
      global alignment remains separately validated. A wrapper regression covers
      an invalid integer-add target, with LLVM/codegen suites, workspace check,
      formatting, and strict Clippy passing.
- [x] Batch 377 closes the LLVM custom integer type boundary.
      `Context::custom_width_int_type` now rejects zero bit widths and returns
      `LlvmResult`, while `i128_type` uses the dedicated LLVM constructor;
      computed-width codegen callers propagate failures. A context regression
      covers zero width, with LLVM/codegen suites, workspace check, formatting,
      and strict Clippy passing.
- [x] Batch 378 closes the LLVM fixed-vector type boundary.
      `BasicTypeEnum::vector_type` now rejects zero lane counts and returns
      `LlvmResult`; all module/function lowering and wrapper test callers
      propagate or explicitly unwrap valid construction. A wrapper regression
      covers zero lanes, with LLVM/codegen suites, workspace check, formatting,
      and strict Clippy passing.
- [x] Batch 379 closes the LLVM fixed-array construction boundary.
      `array_type` now checks the `LLVMArrayType2` handle and returns
      `LlvmResult`; all codegen callers propagate failures. Zero-length arrays
      remain explicitly supported by LLVM and have a wrapper regression, with
      LLVM/codegen suites, workspace check, formatting, and strict Clippy
      passing.
- [x] Batch 380 removes LLVM array-length truncation in the typed wrapper.
      `ArrayType::len` now returns the full `u64` value from
      `LLVMGetArrayLength2`; source-level codegen retains its explicit `u32`
      conversion checks. Wrapper and codegen suites, workspace check,
      formatting, and strict Clippy pass.
- [x] Batch 381 closes the LLVM tuple-projection index boundary.
      Value projections now checked-convert their source `usize` index to
      LLVM's `u32` width before `LLVMBuildExtractValue`, matching the existing
      place-projection guard. A conversion regression covers the representable
      and overflow cases, with LLVM/codegen suites, workspace check, formatting,
      and strict Clippy passing.
- [x] Batch 382 closes the LLVM constant-bitcast boundary.
      Integer constants now require equal source/target widths and pointer
      constants equal address spaces before `LLVMConstBitCast`; both wrappers
      reject null results and static-initializer callers propagate failures.
      Wrapper regressions cover both mismatches, with LLVM/codegen suites,
      workspace check, formatting, and strict Clippy passing.
- [x] Batch 383 closes the LLVM function-type construction boundary.
      All `fn_type` constructors now return `LlvmResult`, check parameter-count
      conversion to LLVM's `u32` width, and reject null LLVM handles. ABI,
      inline-assembly, compiler-builtin, and wrapper-test callers propagate or
      explicitly unwrap construction results. A parameter-count regression plus
      LLVM/codegen suites, workspace check, formatting, and strict Clippy pass.
- [x] Batch 384 closes the LLVM constant-GEP construction boundary.
      Constant and in-bounds constant GEP wrappers now check index-count width
      and LLVM result handles, returning `LlvmResult`; static address lowering
      propagates failures. An index-count regression covers the conversion
      boundary, with LLVM/codegen suites, workspace check, formatting, and
      strict Clippy passing.
- [x] Batch 385 closes remaining LLVM builder slice-count truncation.
      GEP indices, call arguments, and switch cases now checked-convert to
      LLVM's `u32` width before FFI. Switch target validation also avoids an
      unchecked `case_count + 1` capacity expression. A shared conversion
      regression plus LLVM/codegen suites, workspace check, formatting, and
      strict Clippy pass.
- [x] Batch 386 closes fallible LLVM wrapper slice-count truncation.
      Struct-body fields, phi incoming edges, and string-attribute key/value
      lengths now use the shared checked `u32` conversion before FFI; existing
      width regressions cover the shared helper, with LLVM/codegen suites,
      workspace check, formatting, and strict Clippy passing.
- [x] Batch 387 closes debug-info composite element-count truncation.
      DIBuilder struct and union metadata constructors now checked-convert
      element counts to LLVM's `u32` width before FFI. LLVM/codegen suites,
      workspace check, formatting, and strict Clippy pass.
- [x] Batch 388 closes the LLVM context-local struct type boundary.
      `Context::struct_type` now checks field-count conversion to LLVM's
      `u32` width and rejects a null `LLVMStructTypeInContext` result. All
      module/function lowering and wrapper-test callers propagate or explicitly
      unwrap valid construction results. LLVM/codegen suites, workspace check,
      formatting, and strict Clippy pass.
- [x] Batch 389 closes LLVM aggregate constant result-handle gaps.
      Typed array, vector, named-struct, and context byte-string constructors
      now reject null LLVM values before typed wrapping; `Context::const_string`
      returns `LlvmResult` and all codegen callers propagate it. A wrapper
      regression covers the shared null-value guard, with LLVM/codegen suites,
      workspace check, formatting, and strict Clippy passing.
- [x] Batch 390 closes LLVM function-value signature inspection.
      `FunctionValue::get_type` now rejects a null `LLVMGlobalGetValueType`
      result and returns `LlvmResult`; direct-call and return validation
      propagate the failure. A wrapper regression covers the null type guard,
      with LLVM/codegen suites, workspace check, formatting, and strict Clippy
      passing.
- [x] Batch 391 closes the LLVM named opaque-struct allocation boundary.
      `Context::opaque_struct_type` now checks the `LLVMStructCreateNamed`
      handle before typed construction and retains its fallible API. A context
      regression covers the shared null-handle guard, with LLVM/codegen suites,
      workspace check, formatting, and strict Clippy passing.
- [x] Batch 392 closes LLVM instruction-builder allocation.
      `Context::create_builder` now returns `LlvmResult` and validates the
      `LLVMCreateBuilderInContext` handle; function, adapter, and compiler
      builtin lowering propagate failures, while wrapper tests explicitly
      unwrap valid test builders. A context regression covers the null builder
      guard, with LLVM/codegen suites, workspace check, formatting, and strict
      Clippy passing.
- [x] Batch 393 closes the LLVM context-root allocation boundary.
      `Context::create` now returns `LlvmResult` and validates the
      `LLVMContextCreate` handle; the compiler-builtin object path propagates
      the failure, while wrapper tests explicitly unwrap valid contexts. A
      context regression covers the null root guard, with LLVM/codegen suites,
      workspace check, formatting, and strict Clippy passing.
- [x] Batch 394 closes the shared aggregate zero-constant boundary.
      `BasicTypeEnum::const_zero` now routes array and struct constants through
      the checked `BasicValueEnum` constructor instead of assertion-based typed
      wrappers. A shared zero-path regression covers both aggregate categories,
      with LLVM/codegen suites, workspace check, formatting, and strict Clippy
      passing.
- [x] Batch 395 closes typed value type inspection.
      Integer, float, pointer, struct, and array `get_type` methods now validate
      `LLVMTypeOf` through the checked basic-type conversion and return
      `LlvmResult`; wrapper bitcast checks and codegen callers propagate the
      diagnostic. A typed-query regression covers all five value categories,
      with LLVM/codegen suites, workspace check, formatting, and strict Clippy
      passing.
- [x] Batch 396 closes typed aggregate zero/undefined values.
      Struct and array `const_zero`/`get_undef` methods now validate LLVM
      results through `require_value`; aggregate function lowering and promoted
      storage propagate the diagnostic, while wrapper tests explicitly cover
      both aggregate categories. LLVM/codegen suites, workspace check,
      formatting, and strict Clippy pass.
- [x] Batch 397 closes scalar typed undefined values.
      Integer, floating-point, and pointer `get_undef` methods now validate
      `LLVMGetUndef` through the shared null-result guard and return
      `LlvmResult`; a wrapper regression covers all three scalar categories,
      with workspace check, formatting, and strict Clippy passing.
- [x] Batch 398 closes scalar zero/null constructors.
      Integer and floating-point `const_zero`, plus pointer `const_zero` and
      `const_null`, now validate LLVM result handles and return `LlvmResult`;
      compiler builtins, function lowering, static initialization, and wrapper
      tests propagate or explicitly unwrap valid values. LLVM/codegen suites,
      workspace check, formatting, and strict Clippy pass.
- [x] Batch 399 closes integer-to-pointer constant conversion.
      `PointerType::const_int_to_ptr` now validates `LLVMConstIntToPtr` before
      constructing a typed pointer and returns `LlvmResult`; a wrapper
      regression covers the checked conversion, with workspace check,
      formatting, and strict Clippy passing.
- [x] Batch 400 closes wide integer and floating literal constants.
      `IntType::const_u128` validates both native-width and arbitrary-precision
      LLVM constants, while `FloatType::const_float` validates `LLVMConstReal`;
      literal/static-init lowering propagates the result and a wrapper regression
      covers both constructors. LLVM/codegen suites, workspace check, formatting,
      and strict Clippy pass.
- [x] Batch 401 closes ordinary integer constant construction.
      `IntType::const_int` now validates `LLVMConstInt` and returns `LlvmResult`;
      function, aggregate, memory, atomic, call, static-init, and builtin
      lowering propagate the checked result. A wrapper regression covers native
      and wide integer types, with LLVM/codegen suites, workspace check,
      formatting, and strict Clippy passing.
- [x] Batch 402 closes LLVM constant-array count truncation.
      Integer, float, pointer, struct, and nested-array `const_array` methods
      now checked-convert host lengths to LLVM's `u64` count ABI through
      `checked_u64_count`; a width regression covers the shared helper, with
      LLVM/codegen suites, workspace check, formatting, and strict Clippy pass.
- [x] Batch 403 removes duplicate LLVM `u32` count conversions.
      Struct-body and phi-incoming wrappers now use the shared
      `checked_u32_count` helper directly, eliminating parallel conversion and
      error paths while preserving the existing LLVM/codegen coverage.
- [x] Batch 404 closes codegen host-width truncation at LLVM `i64` indices.
      Array-literal element indices and promoted byte-segment lengths now use
      checked `usize -> u64` conversions before LLVM integer constants; the
      affected codegen suite, workspace check, formatting, and strict Clippy
      pass.
- [x] Batch 405 closes codegen repeat-count narrowing.
      Static repeated initializers now checked-convert semantic `u64` counts to
      host `usize` before materializing byte strings or LLVM constant arrays;
      a focused conversion regression covers the host-width boundary, with
      codegen tests, workspace check, formatting, and strict Clippy pass.
- [x] Batch 406 closes const-check collection-length truncation.
      Typed const array validation, inferred list/byte-string lengths, and
      character-string matching now checked-convert host collection counts to
      semantic `u64` lengths, failing closed instead of truncating; const-check
      owner tests, workspace check, formatting, and strict Clippy pass.
- [x] Batch 407 closes cross-host codegen bucket identity drift.
      Backend partition buckets now reduce definition ids and stable symbol
      hashes in `u64` before converting to host `usize`; a high-bit regression
      proves 32-bit and 64-bit hosts select identical buckets, with backend IR,
      LLVM/codegen suites, workspace check, formatting, and strict Clippy pass.
- [x] Batch 408 closes body-check and backend static collection-length drift.
      Array literal inference, const-value materialization, string literal
      typing, and static-initializer repeat compression now checked-convert host
      lengths to semantic `u64`, failing closed or preserving the original form
      when the conversion is unavailable; body-check/backend-lower/LLVM suites,
      workspace check, formatting, and strict Clippy pass.
- [x] Batch 409 closes LLVM vector lane host-width narrowing.
      Promoted vector constants now checked-convert semantic `u32` lane counts
      to host `usize` before materialization, with a conversion regression and
      LLVM/codegen suites, workspace check, formatting, and strict Clippy pass.
- [x] Batch 410 closes backend target-`usize` const narrowing.
      Function-body instantiation now checked-converts `u128` const-generic
      payloads before storing canonical `u64` `usize` values, routing overflow
      through the invalid-IR boundary; backend-lower/LLVM suites, workspace
      check, formatting, and strict Clippy pass.
- [x] Batch 411 closes const vector lane host-width narrowing.
      Const-check vector validation and `splat` evaluation, plus body-check
      const-vector materialization, now checked-convert semantic lane counts to
      host `usize`; const/body/LLVM suites, workspace check, formatting, and
      strict Clippy pass.
- [x] Batch 412 removes the public unchecked formatting and C-string view
      compatibility paths. Formatting now has one checked template dispatcher;
      `CStringView` stores its validated length, bounded `fromBytes` and owned
      views avoid repeated raw-pointer scans, and only the package-private
      process-startup adapter accepts the runtime's NUL-terminated argv/envp
      pointers. `nia-cli` formatting/process executable suites and the loader
      facade regressions, workspace check, and strict Clippy validation pass.
- [x] Batch 413 narrows process startup's raw `argv`/`envp` constructors to the
      standard-library package boundary. `Init`, `Args`, and `Env` can still be
      produced by the freestanding startup runtime and expose their validated
      high-level views, but ordinary Nia code cannot fabricate an arbitrary
      process ABI record through public raw-pointer constructors.
- [x] Batch 414 removes the public process raw-execution compatibility path.
      `process::spawnRaw`, startup raw pointer getters, and argument/environment
      view raw getters are deleted; `Command` is now the sole maintained public
      spawn path, with process tests covering the same success, exit, and error
      behavior through its checked ownership boundary.
- [x] Batch 415 records the startup environment-vector length once when the
      package-owned `Env` view is created. `len`, lookup, and iteration now use
      that stable count instead of repeatedly traversing the raw `envp` array;
      process executable coverage and workspace/strict-Clippy checks pass.
- [x] Batch 416 hardens Linux `read`, `write`, `getdents64`, and `getrandom`
      wrappers against successful return counts larger than the supplied slice.
      Such malformed ABI results now fail with `Errno::Io` before any caller
      cursor advances; workspace and strict Clippy checks pass.
- [x] Batch 417 makes formatting width and precision parsing overflow-safe.
      Decimal template fields now use checked target-`usize` accumulation and
      reject overflow as `InvalidTemplate`; executable regressions cover both
      width and precision overflow before writer dispatch.
- [x] Batch 418 hardens `SliceIter` and `SliceIterMut` element address
      arithmetic. `next` and `nextBack` now checked-multiply the index by the
      element size and checked-add the base pointer before yielding a reference;
      impossible arithmetic returns `null` instead of wrapping an address.
- [x] Batch 419 hardens the shared `ArenaChunk` payload-base calculation.
      Header-size pointer addition is now checked and all allocation/growth and
      ownership consumers fail closed when the host address cannot represent
      the derived payload base.
- [x] Batch 420 hardens general-purpose allocator metadata pointer arithmetic.
      Small-page slot-byte multiplication and slot address derivation, plus
      large-header user-pointer offsets, now use checked operations; malformed
      metadata fails ownership/free/lookup closed.
- [x] Batch 421 hardens Wyhash incremental state arithmetic. Total input length
      now saturates on host overflow and 48-byte chunk boundaries use checked
      index addition before slicing; hash vectors, hash-map, and string runtime
      suites remain green.
- [x] Batch 422 hardens the unbounded `DiscardingWriter` byte counter. Accepted
      writes still report their exact slice length, while cumulative `len()`
      now saturates on host overflow rather than wrapping to a smaller value.
- [x] Batch 423 hardens build package-handle allocation. The root-reserved
      `packages.len() + 1` index now uses checked host arithmetic and reports a
      structured build memory failure instead of wrapping into an existing
      package identity.
- [x] Batch 424 closes build owner-ID wraparound. The monotonic atomic owner
      counter now has a process-wide exhaustion latch; once the `usize` identity
      space reaches its boundary, new Build initialization returns an internal
      initialization error instead of reusing a prior owner identity.
- [x] Batch 425 hardens Linux page-mapping range trimming. Mapped-range ends
      and aligned suffix addresses now use checked host pointer arithmetic;
      unrepresentable virtual ranges unmap the complete allocation and return
      `OutOfRange` before partial trimming.
- [x] Batch 426 hardens hash-map probing at the raw cursor boundary. Invalid
      bucket shapes no longer derive a wrapped mask, and valid quadratic group
      steps use explicit modulo addition so host-width overflow cannot alter a
      control-group index. Existing collision, tombstone, rehash, clone, and
      assume-capacity executable coverage remains green.
- [x] Batch 427 hardens freestanding Linux x86_64 startup address derivation.
      `argc + 2`, word-size multiplication, and initial-stack address additions
      are checked before constructing `argv`/`envp`; an unrepresentable startup
      layout exits with status 127 instead of publishing wrapped raw pointers.
      Freestanding loader/codegen coverage and the process executable matrix
      remain green.
- [x] Batch 428 removes terminal-index wraparound from build path validation.
      The plan encoder and build graph now scan only in-slice separators and
      validate the final component after the loop, preserving rejection of
      empty/current/parent components without incrementing `len` at the end.
      Build-plan owner and standard build-case coverage remain green.
- [x] Batch 429 hardens UTF-8 view state transitions. Decoded byte widths now
      pass through checked in-slice advancement, scalar counts use checked
      accumulation, and iterators fail closed if advancement or remaining-count
      subtraction is not representable. Existing Unicode, string, formatting,
      and process text workflows preserve their behavior.
- [x] Batch 430 centralizes Linux `dirent64` record validation. Entry decoding
      and next-offset advancement now share checked record/type/name bounds;
      empty or unterminated names are rejected instead of treating padding as
      filename bytes. The complete filesystem executable matrix preserves real
      directory iteration and error behavior.
- [x] Batch 431 validates Linux `statx` timestamp payloads. Optional access time
      and required modification/status-change times now reject nanosecond fields
      outside `[0, 1_000_000_000)` before public filesystem metadata is built.
      Filesystem metadata and the complete executable matrix remain green.
- [x] Batch 432 validates Linux `getcwd` success payloads. Zero or oversized
      return counts and results lacking the promised trailing NUL now return
      `Io` before an unterminated path slice can reach filesystem consumers.
      Filesystem executable coverage remains green.
- [x] Batch 433 validates Linux `wait4` identities. Blocking and non-blocking
      waits now require a positive return matching the requested child pid
      before publishing `WaitStatus`; mismatched success is reported as `Io`.
      Process spawn/wait executable coverage remains green.
- [x] Batch 434 hardens Linux syscall error conversion. Raw returns are
      required to be negative and within the errno range before negation and
      narrowing; non-negative, minimum-integer, and oversized magnitudes fail
      closed as `Io`.
- [x] Batch 435 centralizes Linux descriptor and fork-result validation.
      Successful syscall returns now require non-negative `i32`-representable
      descriptors, forked child ids require positive `i32`-representable pids,
      and kernel-filled pipe descriptors use the same checked boundary.
- [x] Batch 436 centralizes Linux zero-return syscall validation. Close,
      truncate, permission, sync, directory mutation, process, memory, and
      metadata calls now require an exact zero success; `dup2` requires its
      returned descriptor to equal the requested destination.
- [x] Batch 437 closes the remaining Linux `pipe2` success boundary. The
      syscall must return exactly zero before its kernel-filled descriptor pair
      is consumed; positive anomalies now fail as `Io`.
- [x] Batch 438 closes the Linux anonymous `mmap` null-address boundary. A zero
      success return is rejected before conversion to a mutable reference;
      non-zero mappings retain the existing checked range-trimming behavior.
- [x] Batch 439 removes repeated process-startup view construction. `Init`
      now materializes `Args` and `Env` once and stores those validated views;
      `argc()`, `args()`, and `env()` no longer retain or rescan raw startup
      pointers after the package-owned boundary.
- [x] Batch 440 hardens empty Linux process vectors. Null argv/envp vectors are
      represented as empty views, and null individual entries are rejected
      before C-string construction rather than being dereferenced.
- [x] Batch 441 makes single allocation-owner cleanup retryable and removes an
      unreachable public constructor path. `Allocated` and
      `CallableAllocation` now retain pointer/size/alignment state until
      allocator release succeeds, while `mem::allocValue` is exposed from the
      public facade and covered by a release-failure/retry executable case.
- [x] Batch 442 hardens general-purpose allocator metadata growth. Small-slot
      size doubling and page used-count increments now use checked arithmetic
      and reject impossible transitions before mutating allocator state.
- [x] Batch 443 preserves invalid-transfer error identity through generic I/O
      adapters. `BufferedReader`, `BufferedWriter`, and `LimitedReader` now
      forward the wrapped implementation's `invalidRead` or `invalidWrite`
      classification while retaining the existing pre-mutation count checks;
      the malicious transfer-count executable distinguishes these errors from
      ordinary end-of-stream and short-write failures through every adapter.
- [x] Batch 444 makes hash-map rehash replacement storage retryable. Replaced
      control, key, and value allocations are retained as map-owned retired
      slices until each fallible release succeeds; later rehashes retry that
      retired group before publishing another table, and `deinit` attempts both
      active and retired owners. The HashMap executable regression proves an
      injected old-control free failure leaves all entries usable and releases
      the residual owner during final cleanup.
- [x] Batch 445 makes slice iterator address checks transactional. `nextBack`
      now commits its remaining-length decrement only after checked offset and
      address derivation succeeds, matching the existing `next` behavior for
      both immutable and mutable iterators; an impossible address cannot drop a
      pending element.
- [x] Batch 446 closes the remaining large-allocation base offset check.
      `GeneralPurposeAllocator::allocLarge` now checked-adds the backing base
      and header size before alignment; a malformed child-allocator address is
      released and reported as `OutOfMemory` instead of wrapping into metadata.
- [x] Batch 447 makes ArrayList replacement cleanup retryable. Growth, shrink,
      and `intoOwnedSlice` retain a temporary replacement slice when the old
      owner release fails and the replacement release also fails; `deinit`
      attempts active and retired slices independently. An allocator regression
      proves both allocations remain reachable and are eventually released.
- [x] Batch 448 gives ArrayList self-alias copies a distinct cleanup owner.
      `appendSlice`, `insertSlice`, and `replaceRange` now retain failed
      temporary copies separately from replacement storage, and cleanup attempts
      both owner classes. Self-alias and overlapping free-failure executable
      regressions prove no temporary allocation is lost.
- [x] Batch 449 preserves temporary Linux descriptor close errors. Contained
      create, delete, rename, and metadata operations now evaluate the primary
      syscall before explicitly closing every temporary parent/metadata handle.
      A shared `Errno!T` combiner retains the primary operation error, otherwise
      returns the first close error while still attempting later closes; the
      rename second-parent-open failure also retains its old-parent cleanup.
      The complete 23-case filesystem executable matrix remains green.
- [x] Batch 450 preserves Linux spawn-handshake close errors. The parent now
      closes the consumed error-pipe read descriptor exactly once after the
      handshake result is known. EOF plus close failure is surfaced as setup
      error, while an incomplete/read/child-reported error remains primary;
      child reaping stays owned by the outer spawn path. The complete 43-case
      process executable matrix remains green.
- [x] Batch 451 retains general-purpose allocator rollback owners. Malformed
      small/large backing metadata no longer drops a child block when its
      fallible cleanup fails; a pending owner is visible to capacity/emptiness,
      retried before later allocation, and retried by `deinit`. The malformed
      large-header executable now injects a first-free failure and proves the
      pending block is released by cleanup retry.
- [x] Batch 452 preserves primary string-format errors over staging cleanup.
      `TextFormatWriter::finish` now returns the format/UTF-8 error when its
      temporary byte-buffer deinit also fails, while still exposing cleanup
      allocation errors after successful formatting. The process executable
      string matrix covers overlapping invalid-UTF8 plus cleanup failure and
      rollback of the destination text.
- [x] Batch 453 retains failed string-format staging owners. `String` now keeps
      a pending byte-buffer owner when `free` fails before releasing the writer
      backing; `appendFormat` retries that owner before staging new output and
      `deinit` attempts both text and pending bytes while retaining the first
      error. The executable regression allocator fails before release and covers
      retry by a later format plus final cleanup of an invalid-UTF8 primary path.
- [x] Batch 454 makes default allocator reallocation rollback explicit.
      `Allocator::realloc` now returns `ReallocError`; a double-free failure
      retains the replacement `Block` and both error identities instead of
      dropping that owner, while the public `mem` facade exposes the typed
      contract. The allocator executable matrix
      covers both a recoverable old-free failure and a two-owner rollback whose
      replacement and original blocks are subsequently released.
- [x] Batch 455 removes HashMap's multi-allocation rollback surface. Control
      bytes, keys, and values now occupy one checked, maximum-aligned storage
      block; rehash transfers one retired block and `deinit` retries that single
      owner without silently dropping ctrl/key/value allocations. The complete
      seven-case HashMap executable matrix now covers one-block rehash and
      cleanup counts, including unit-sized generic fields.
- [x] Batch 456 makes over-aligned Linux mappings single-owner. The page mapper
      now retains the complete mmap range while exposing an aligned interior
      pointer; `mem::Block` carries the release pointer/length and
      `PageAllocator::free` unmaps that range once. Prefix/suffix best-effort
      unmaps and their swallowed cleanup errors are removed; the allocator
      executable matrix still passes the over-aligned mapping/realloc cases.
- [x] Batch 457 preserves typed slice allocation owners. `Allocator::allocSlice`
      now returns `SliceAllocation[T]`, whose borrowed views and `deinit` retain
      the original `Block`; `ArrayList` and `String` transfer that owner without
      reconstructing release metadata. Replacement and self-alias rollback
      paths retain their real `Block` owners, and raw-slice ownership
      constructors plus `freeSlice` are removed. The focused allocator,
      ArrayList, intrinsic, and string executable matrices pass; the full
      driver build-case ledger remains independently tracked below.
- [x] Batch 458 removes stale allocator API usage from executable fixtures.
      Closure cases now call the documented `mem::allocValue` facade after the
      package-private allocator extension was retired. Their backend-finalization
      baselines, along with the dual-stage const, payload-enum, and trait-object
      cases affected by the public std demand graph, now match the current
      implementation. The complete `nia-driver --lib` suite passes 646/646.
- [x] Batch 459 carries complete allocator release metadata through single
      typed owners. `Allocated` and `CallableAllocation` now retain the
      original block's release pointer and length across construction,
      callable transfer, and retryable `deinit`; release addresses use the
      explicit raw-integer boundary so closure escape analysis sees only the
      callable view. They no longer reconstruct release ownership from the
      value pointer and logical layout after an over-aligned mapping.
- [x] Batch 460 preserves release owners across allocator layout transitions.
      `Block::withLayout` is now the canonical successful resize/remap operation
      for the trait default, page, general-purpose, arena, and fixed-buffer
      allocators, including resized arena backing chunks. An over-aligned
      delegating allocator regression exercises the default remap path and
      releases the complete original page mapping.
- [x] Batch 461 collapses ArrayList ownership to explicit `Block` state. The
      active allocation is owned only by `storageBlock`; failed replacement and
      self-alias cleanup retain `replacementBlock` and `aliasBlock` respectively.
      Duplicate slice/capacity fields and raw-slice `Block` reconstruction are
      removed, while the allocator, ArrayList, intrinsic, and string matrices
      pass (4/4, 12/12, 6/6, 4/4, and 3/3 respectively).
- [x] Batch 462 restores unpublished Linux mapping cleanup. If complete-range
      or aligned-address validation fails after `mmap` succeeds, one canonical
      rejection helper unmaps the full release range before returning
      `OutOfRange`; a `munmap` failure is propagated rather than swallowed.
      Prefix/suffix trimming remains absent, so the mapping still has one owner.
- [x] Batch 463 makes typed slice cleanup owner-driven. `SliceAllocation::deinit`
      now frees its complete `Block` even when the logical view length is zero,
      then clears the owner only after successful release. A custom allocator
      regression covers a non-empty release range attached to a zero-length
      slice.
- [x] Batch 464 makes single allocation owner retirement complete. Successful
      `Allocated`/`CallableAllocation` deinit and `intoCallable` transfer now
      clear release pointer and length together with logical size, preventing
      repeated cleanup or source-owner cleanup after transfer from releasing
      the same block twice.
- [x] Batch 465 makes ArrayList empty-state cleanup owner-driven. A present
      `storageBlock` is released during `deinit` and `intoOwnedSlice` even when
      logical length, capacity, or element size is zero; empty-state transitions
      now clear the owner only after that release succeeds. An allocator-backed
      executable regression proves both adopted empty lists and empty ownership
      transfers release a non-empty block exactly once.
- [x] Batch 466 makes ArrayList empty-state replacement owner-driven. The
      allocation lookup returns a present `storageBlock` before considering
      logical capacity, so growing an adopted zero-length allocation replaces
      and releases its real old Block instead of a synthesized empty Block. The
      custom allocator regression grows such a list, preserves its new element,
      and proves both old and new owners are released exactly once.
- [x] Batch 467 closes String's formatting-staging transfer gap. When staging
      cleanup previously failed, `String::intoOwnedSlice` now retries and
      retires the pending format owner before transferring the text owner; a
      failed retry leaves the source intact. The process executable regression
      covers failed and successful retries, then verifies both allocations are
      released.
- [x] Batch 468 preserves allocator-returned layouts in single typed owners.
      `Allocated` and `CallableAllocation` now carry the actual Block size and
      alignment rather than rebuilding them from the typed value layout, so a
      zero-sized value backed by a non-empty custom allocation is released
      exactly once before and after callable transfer. The allocator executable
      regression covers both owner forms and repeated cleanup.
- [x] Batch 469 removes lossy process-exit conversion for allocator rollback
      owners. `ReallocError::Rollback` carries a live replacement Block, so its
      `IntoError[ExitCode]`, `asExitCode`, and error-union `exit` adapters are
      removed instead of discarding that owner. Realloc success fixtures now
      match every error variant and attempt both original and replacement frees;
      compile-fail coverage keeps all three convenience conversions absent.
- [x] Batch 470 removes allocator-owned filesystem path staging. Scalar paths
      now encode directly into `os::maxPathBytes` stack storage, and native
      create/delete/rename splitting uses independently terminated stack
      buffers, so fallible temporary cleanup cannot replace a primary operation
      error or discard a successful handle. Oversized paths fail before the
      syscall as contextual `PathError::TooLong`; obsolete `*WithAllocator`
      entry points and the unreachable `OperationError::Allocation` arm are
      removed and locked out by compile-fail coverage.
- [x] Batch 471 makes process command staging an explicit retryable owner.
      `Command::spawn` now returns a `SpawnAttempt` that retains all five native
      staging lists and its pending child or primary spawn error until `finish`
      releases every allocation. Custom-allocator attempts take the allocator
      again on retry; path and argument UTF-8 encode directly into final staging
      without temporary C strings. Fault injection proves five simultaneous
      free failures remain reachable and the original success/error outcome is
      published only after cleanup; owner-discarding `run` shortcuts are removed.
- [x] Batch 472 makes build dependency-validation scratch explicit owners.
      `Build` retains indegree and ready-list staging across validation calls,
      attempts both cleanup releases, preserves cycle/validation failures over
      cleanup errors, and retries failed scratch frees on the next pass or
      `Build::deinit`. A fault-injected cycle regression covers two simultaneous
      scratch free failures and complete eventual release.
- [x] Batch 473 makes plan-draft encoding and publication owner-driven.
      The encoded byte backing remains attached to `Build` across encoder
      failures and file publication. Writer, flush, sync, and close stages keep
      the first operation error while still attempting later cleanup; failed
      encoding cleanup remains retryable on the next draft or `Build::deinit`.
- [x] Batch 474 makes build initialization a retryable owner transaction.
      `Build::init` returns `BuildInitAttempt`, which retains every partial path,
      target component, requested step, primary error, and allocator reference
      until `finish` can complete cleanup or transfer the initialized graph.
      The former fallible rollback defers and direct `Error!Build` surface are
      removed; fault injection proves a retained-free failure can be retried
      before the original allocation error is published.
- [x] Batch 475 makes package graph insertion a Build-owned transaction. The
      package list reserves capacity before field retention, and a partial
      package remains in a pending `Build` slot until initialization transfers
      it or cleanup succeeds. Fault injection covers root allocation failure,
      simultaneous name-release failure, retry through the next insertion, and
      complete final graph cleanup.
- [x] Batch 476 makes simple artifact-target insertion owner-driven. Object and
      static-archive lists reserve before retention, while dedicated `Build`
      pending records own partially initialized names and output names until an
      infallible append transfers them. Fault injection covers allocation plus
      retained-free failure, retry through each matching insertion, and final
      graph cleanup for both target kinds.
- [x] Batch 477 makes executable-target insertion owner-driven. Its `Build`
      pending record retains name, output-name, and static-archive handle-list
      backing until an infallible post-reserve append transfers the complete
      target. Fault injection fails the nested list allocation, rejects both
      string releases, verifies neither cleanup is skipped, then retries the
      insertion and final graph cleanup successfully.
- [x] Batch 478 makes module and import insertion recursively owner-driven. The
      module and imports lists reserve before empty import records are appended
      and initialized in place; the partial module remains attached to `Build`
      across any nested failure. The detached `cloneModuleImports` rollback
      chain is removed. Fault injection preserves outer and nested list backing
      plus two failed import field owners, then retries insertion and final
      cleanup successfully.
- [x] Batch 479 introduces kind-correct Build-owned pending steps for generated
      files and uncacheable actions. The step list reserves before step name and
      payload initialization, and only an infallible append transfers the union
      record. Fault injection preserves simultaneous generated name/output
      owners and an uncacheable name owner across failed cleanup, proves every
      injected release is attempted, and retries both insertions successfully.
- [x] Batch 480 makes run/test payload construction and dependency rollback
      owner-driven. Argument slots are attached to a reserved nested list before
      string initialization; `rollbackLastStep` moves a popped committed step
      into `Build.pendingStep` before cleanup. Explicit operation/cleanup
      composition preserves dependency OOM over three simultaneous release
      failures, retains two arguments plus their list and step name, and proves
      the next insertion can recursively retire them.
- [x] Batch 481 makes executable and static-archive install steps pending-owner
      transactions. Their kind-specific empty payloads retain step name and
      destination before commit, and the shared explicit producer-dependency
      completion path preserves edge errors over rollback cleanup. Fault
      injection covers dependency OOM plus two simultaneous string-release
      failures, retry, and final cleanup; the target suite covers both kinds.
- [x] Batch 482 makes external-command steps recursively owner-driven. Program,
      working directory, argument and environment lists, and their nested
      records are attached before string retention; the three detached rollback
      defer layers are removed. Multi-producer edge insertion is an explicit
      suffix transaction preserving its primary error. Fault injection covers
      argument-value and environment-value allocation failures, multiple
      simultaneous nested releases, containing-list retention, retry, and
      complete final cleanup.
- [x] Batch 483 unifies all remaining build steps on pending ownership.
      Aggregate, check, and emit records reserve the steps list before name
      retention and transfer through an infallible append. Emit-executable
      archive dependencies use explicit suffix rollback with primary-error
      precedence. The obsolete generic local `Step::init` transaction and final
      two fallible cleanup defers are removed; `lib/std/build` now contains no
      `defer`, while ownership and target suites retain full coverage.
- [x] Batch 484 makes Linux spawn-pipe cleanup one explicit resource
      transaction. All four pipe pairs remain attached to `SpawnResources`
      until successful child/public ends transfer or an error path consumes
      every untransferred end. Spawn and handshake errors remain primary, while
      post-exec cleanup cannot manufacture a plain error that loses the live
      child owner. The complete 45-case process executable suite remains green.
- [x] Batch 485 makes failed-child reaping a retryable spawn owner. A non-EINTR
      `wait4` failure returns the primary stage/cause, reap cause, and pid while
      retaining the native attempt for the next `finish`; the original spawn
      error is published only after cleanup completes, and `ECHILD` is the
      explicit no-owner terminal state. A seccomp regression rejects real
      `wait4` calls and proves two retries retain the same positive pid; all 46
      process executable cases pass.
- [x] Batch 486 proves arena multi-list cleanup retains every failed chunk
      owner. A fault-injected child allocator creates independent used and free
      chunks, rejects both releases, and verifies the first `deinit` attempts
      both while retained capacity remains visible; the retry releases only the
      two residual chunks and clears the arena. All 13 allocator executable
      cases pass.
- [x] Batch 487 proves general-purpose allocator destruction covers all owner
      classes. A mixed child allocator creates a small page, large header, and
      malformed pending rollback block, rejects all three cleanup releases, and
      verifies each remains reflected in capacity/used/emptiness. The retry
      releases only those residual owners and reaches canonical empty state;
      all 14 allocator executable cases pass.
- [x] Batch 488 removes copied descriptor identity from filesystem adapters.
      `FileReader`, `FileWriter`, and `DirIterator` now borrow their owning
      `File` or `Dir` and resolve its live handle before descriptor access, so
      owner close returns `BadFd` even after the kernel reuses the descriptor
      for a different file or directory. Directory refill cursor/end state
      commits only after a successful read, preventing replay of an exhausted
      batch on retry. A real descriptor-reuse executable verifies writer
      buffering, reader state, directory iteration, and both replacement
      objects; all 25 filesystem executable cases pass.
- [x] Batch 489 makes owned-path component joins self-alias safe. A component
      borrowed from the destination text is recorded as a logical offset before
      capacity growth and reconstructed from replacement storage afterward;
      separator and component commit only through assume-capacity operations
      once the complete length is reserved. No temporary allocation owner or
      owner-dropping cleanup path is introduced. An executable forces allocator
      relocation while joining `path.text()` and verifies `base/base`.
- [x] Batch 490 makes scalar path encoding an output transaction. A validation
      pass checks embedded NULs, total UTF-8 byte arithmetic, and trailing-NUL
      capacity before caller storage is mutated; only a valid complete path
      reaches the write pass. Sentinel-backed executable cases prove both a
      mid-path NUL and a buffer lacking only terminator space leave every output
      byte unchanged.
- [x] Batch 491 validates formatter radices before computation or output. All
      public signed/unsigned radix entry points now reject values outside
      `{2, 8, 10, 16}` before digit division, sign, prefix, or padding, closing
      radix-zero traps, radix-one non-termination, and partial-output failures.
      `FormatAlignment` now completes the public `FormatSpec` facade contract.
      A counting-writer executable covers radices 0, 1, and 3 and proves every
      rejection leaves output length zero.
- [x] Batch 492 validates public formatting specs before output. Because
      presentation and alignment are open enums, `FormatSpec::validate` now
      rejects unnamed discriminants, and every spec-aware formatter entry point
      performs that check before padding, prefixes, signs, or content can reach
      the writer. The generic slice override observes the same contract instead
      of bypassing the trait default. A counting-writer executable constructs
      malformed presentation and alignment values and proves both failures
      leave output length zero.
- [x] Batch 493 closes builtin witnesses reached through generic impl
      predicates. After a concrete extension implementation matches,
      substituted source and builtin bounds now share the canonical recursive
      trait/supertrait method expansion; the old builtin fallback incorrectly
      registered the triggering outer method name and could filter out the
      actual operator implementation. A two-module executable combines a
      source `Step` bound with builtin `Ord`, emits and runs the concrete
      instance, and proves LLVM declaration readiness receives every selected
      function owner instead of reporting a missing-owner ICE. The existing
      cross-module source-trait driver regression now reaches backend codegen.
- [x] Batch 494 completes the public range-iterator construction surface.
      `Range`, `RangeInclusive`, and `RangeFrom` now expose their canonical
      `init` constructors while keeping builtin-range `fromBounds` adapters
      private. A real executable directly constructs all three with a custom
      `Step + StepBack + Ord + Eq` type, exercises forward/backward iteration,
      inclusive empty-state initialization, and open-ended exhaustion, and
      proves the generic witness closure reaches LLVM and runtime successfully.
- [x] Batch 495 validates custom range-step transitions. Bounded forward and
      backward iteration now requires every `Step` or `StepBack` result to move
      strictly in the requested direction and remain within the live endpoint;
      missing, stalled, or crossing candidates exhaust the range instead of
      repeating or yielding an out-of-range value. A real executable covers
      half-open and inclusive ranges, both directions, mixed consumption,
      canonical exhausted state, stalled steps, and missing predecessors.
- [x] Batch 496 completes builtin integer range stepping. `u128` now implements
      both `Step` and `StepBack`, matching every other signed and unsigned
      integer primitive. A real executable exercises direct and range-literal
      construction, mixed forward/backward half-open and inclusive iteration,
      and fused open-ended exhaustion at `u128::MAX`, proving endpoint checks
      avoid overflow through LLVM and runtime execution.
- [x] Batch 497 completes the public owned-iterator construction surface.
      `Take` and `Rev` now expose canonical `init` constructors for their
      private owned state, while borrowed and raw-backed iterators remain
      constructible only through their source containers. A real executable
      directly constructs both generic adapters and exercises limit exhaustion
      plus mixed forward/backward reversal through LLVM and runtime execution.
- [x] Batch 498 closes numeric-separator grammar at both source and phase-product
      boundaries. The lexer now accepts `_` only between radix-valid digits,
      consumes malformed spellings and suffixes as one `InvalidNumber` token,
      and propagates that error through parser and CLI diagnostics. Numeric
      decoders independently reject misplaced separators before normalization,
      with data-driven valid/invalid owner matrices and a real `nia check`
      failure regression.
- [x] Batch 499 centralizes numeric body/suffix ownership in `nia-literals`.
      Const checking no longer mistakes a float fraction or exponent for its
      suffix and therefore preserves inferred `f32` constants through driver
      checking. Body and const checking consume the same splitter, decoders
      reject suffixes outside their integer/float sets, duplicate scanners are
      removed, and owner plus driver regressions cover both float forms.
- [x] Batch 500 preserves the complete source integer magnitude. Literal
      decoding now returns `u128`, signs remain unary syntax, and semantic,
      const, optimizer, target-condition, backend-validation, and LLVM paths
      carry unsigned values or `IntConst` without an intermediate `i128`
      narrowing. One target-aware `IntConst` representability contract replaces
      duplicate truncated range helpers. Endpoint owner tests retain negative
      `i128::MIN` and narrow overflow behavior, while a real ELF compiles and
      runs decimal and hexadecimal `u128::MAX` through const and runtime paths.
- [x] Batch 501 closes the signed source-literal endpoint in const evaluation.
      Unary negation now consumes unsigned magnitude bits before signed
      projection, accepting exactly `2^127` as `i128::MIN` while retaining
      overflow for a second negation, larger magnitudes, unsigned targets, and
      narrower signed types. Function lowering canonicalizes the endpoint call
      before backend range validation. Owner endpoint tests and a real ELF
      cover const and runtime `i128::MIN` alongside the full-width `u128`
      boundary.
- [x] Batch 502 makes const float-to-integer casts preserve the full target
      range. Width-derived half-open powers-of-two replace rounded floating
      views of integer maxima, and casts now construct target-signed `IntConst`
      values without a saturating `i128` intermediate. Owner boundary tests
      cover signed, unsigned, malformed-width, and exclusive endpoints; the
      real 128-bit ELF observes the correct embedded `2^127f64 as u128` const
      result.
- [x] Batch 503 owns unsigned float-to-`u128` LLVM libcalls in the freestanding
      compiler-builtins unit. Reachable `f32` and `f64` casts request
      `__fixunssfti` and `__fixunsdfti` through fingerprinted symbol flags; the
      emitted helpers split at `2^64`, use native `u64` conversions, and cannot
      recursively depend on themselves. A codegen owner regression proves both
      casts share exactly one synthetic object, while a real ELF links and runs
      small, mixed-high/low, `f32`, and `f64` conversions without host runtime
      libraries.
- [x] Batch 504 completes the paired signed float-to-`i128` compiler-builtins
      owner. Reachable `f32`/`f64` casts request `__fixsfti`/`__fixdfti`; the
      shared magnitude implementation selects sign after reconstructing both
      64-bit halves. Owner coverage proves all four symbols share one synthetic
      object, and the real ELF covers signed small values, `f32`, and the exact
      `i128::MIN` endpoint.
- [x] Batch 505 migrates the arena allocator example to the canonical owned
      slice surface. `SliceAllocation` remains an explicit allocation owner
      rather than regaining legacy direct indexing; mutation and observation go
      through `asMutSlice` and `asSlice`, and the runnable example verifies the
      stored bytes after arena reset. The repository-wide example acceptance
      suite once again checks every maintained example successfully.
- [x] Batch 506 closes the reverse wide-integer conversion builtin boundary.
      Reachable `u128`/`i128` to `f32`/`f64` casts now request the paired
      `__floatuntisf`, `__floatuntidf`, `__floattisf`, and `__floattidf` symbols
      in the synthetic compiler-builtins unit. The definitions construct IEEE
      fields from native-width integer operations with ties-to-even rounding,
      preserve signed magnitude endpoints including `i128::MIN`, and cannot
      recursively lower through the conversion they own. Owner coverage proves
      all eight wide conversion symbols share one fingerprinted object, while a
      freestanding executable covers zero, maximum, endpoint, and rounding
      behavior without a host runtime dependency.
- [x] Batch 507 closes resolved const/runtime float precision divergence.
      Resolved `f32` arithmetic, comparisons, unary negation, and compound
      assignments now execute with binary32 operands and results at every
      operation boundary; `f64` retains binary64 behavior. The analyzer owns
      expression and assignment-target precision facts, owner numeric tests
      cover halfway-adjacent rounding and canonical comparisons, and a driver
      regression verifies binary and compound `f32` values before integer casts.
- [x] Batch 508 closes static initializer target-type propagation.
      Body checking now threads each checked destination type through scalar
      and aggregate static initializer lowering. Integer target propagation
      restores signedness while preserving complete `IntConst` bits, whereas
      explicit casts alone perform destination-width masking. The existing
      const-to-`i32` global regression now passes alongside the full 278-test
      driver const-evaluation suite.
- [x] Batch 509 preserves resolved compound-assignment integer semantics.
      Compound assignments now query the concrete target leaf through field and
      index paths and apply its width and signedness before writeback, keeping
      narrow overflow diagnostics aligned with runtime behavior. A driver
      regression covers local, field, and indexed `u8 += 1u8` targets; the early,
      unresolved evaluator retains its intentionally type-free arithmetic path.
- [x] Batch 510 aligns scalar const values with static initializer products.
      Static admission and Body lowering now accept and preserve named integer,
      float, and boolean const payloads across local, module, imported, and
      nested-static bindings. Recovery `StaticInit::Zero` cannot be published
      anywhere in a Body-lowered initializer tree; owner tests and a driver
      codegen regression cover the complete scalar boundary.
- [x] Batch 511 materializes representable aggregate const values as static
      data. Static admission and Body lowering now recurse through arrays and
      nominal structs while rejecting tuple values that lack a `StaticInit`
      variant. Direct and named struct paths share canonical type/const generic
      field substitution, and every integer leaf regains its checked target
      signedness. Owner regressions cover admission, recovery-product absence,
      imported and local consts, nested arrays, and const-generic structs; a
      driver codegen regression verifies the published initializer trees.
- [x] Batch 512 restores the standard-library filesystem provider boundary.
      `Dir`/`File` handle borrowing, transfer, and close operations now live
      with their owning types in `fs/types.nia`, so `DirIterator` and adapter
      bodies do not depend on a sibling implementation module merely to resolve
      a package-visible handle method. The const error-conversion helpers in
      `os.nia` are explicitly `const fn`, preserving the `SpawnError` mapping
      contract. Linux filesystem-layout, freestanding startup, and file-reader
      and writer provider regressions pass again.
- [x] Batch 513 closes the terminal facade re-export processing gap. A used
      `Always` path that resolves an item through a public facade now processes
      the terminal source module just like a declared child path, keeping
      namespace and explicit-item import spellings on the same semantic
      provider closure without eagerly loading unrelated siblings. Loader
      coverage locks the `std::mem`/`builtin::layout` case, and the equivalent
      text-workflow driver regression now passes.
- [x] Batch 514 makes trait-pattern array inference transactional. A generic
      array length binding is now committed only after the element pattern also
      matches, so a rejected candidate cannot leak a partial const substitution
      into later impl probing. The trait-solver owner regression covers the
      length-bind/element-mismatch boundary.
- [x] Batch 515 gives timing output one mode owner. Counters reached stderr
      with timings disabled because `emit_counter` had no mode gate and
      `collect_to_stderr` installed a collector unconditionally, leaving the
      `TimingMode::Off` decision without an owner. The collection scope now
      owns it: `TimingSession` retains its mode and `record_timing_event`
      returns a three-state record that keeps a discarding active scope
      distinct from an unscoped event. This also closed a second leak where
      `--timings-format=json` alone printed a full report. Five regressions
      cover both directions at the owner and CLI boundaries.
- [x] Batch 518 removes the unassertable refusal guard added by batch 517. The
      `debug_assert` at the pipeline's static-initializer boundary required a
      refused initializer to have reported, which turned the spec's documented
      `static p: &i32 = target;` error into an internal error: its diagnostic
      comes from type checking, so the refusal legitimately adds nothing. A
      second form, asserting this checker's list is non-empty, failed likewise
      because `nia-static-check` and type checking own separate diagnostic
      stores. The invariant is enforced at the point of refusal instead, which
      is a local property of one function; recurrence is guarded by the owner
      regressions rather than structurally.
      `nia-codegen-llvm`'s `rejects_bare_global_as_pointer_initializer` caught
      it, so a two-crate run was not sufficient evidence for batch 517.
- [x] Batch 517 rejects unrepresentable named const values in static
      initializers. A named `const` whose kind has no `StaticInit` equivalent
      was published as zeroed static storage with no diagnostic; a tuple read
      `0` instead of `1`, a present optional read as `null`, and an enum held a
      value outside its declared variant set. The named-const lowering path
      returned `None` without reporting, its caller could not distinguish that
      from "not a named const", and the pipeline consumed the recovery guard's
      refusal as a plain absence — which the language zero-initializes.
      Lowering now reports at the point of refusal, naming the kind, and the
      pipeline accepts a refusal only when it carried a diagnostic, so a later
      `ConstValue` variant without a `StaticInit` equivalent trips an assertion
      instead of zeroing a global. Both documents already required this. The
      owner regression now requires the diagnostic rather than only an empty
      `global_inits`, which is the assertion that let the defect pass.
- [x] Batch 516 corrects the specification's excluded-systems list. The list
      named systems that are part of the language surface today, and four
      `Event::Resize { ... }` pattern examples used a spelling the parser
      rejects at the enum declaration, present only in prose and never in a
      test, example, or std module. Payload variants are recorded as planned
      with their design deliberately open.
- [x] Batch 519 restores eager `const fn` capability validation on the
      entry/executable query path. Executable checking still evaluates and
      materializes only reachable bodies, but checked-module assembly now
      merges the existing declaration-only body check into const diagnostics.
      Exact overlap with reachable body or const-evaluation diagnostics is
      removed structurally. A query-owner matrix proves unused, runtime-only,
      and const-reached invalid declarations each report exactly once without
      becoming executable roots.
- [x] Batch 520 closes source-trait builtin-supertrait validation. A source
      trait such as `Child : Iterator` previously caused an implementation of
      `Child` to skip the explicit `Iterator` witness check because validation
      only unpacked nominal supertrait types. Supertrait validation now uses the
      canonical trait-id/argument decoder for source and builtin forms, keeps
      associated-binding checks intact, and reports the builtin trait name.
      The intrinsic `Sized` witness remains implicit for concrete sized targets,
      matching the standard-library `AsciiUnit : Sized` contract.
      The driver supertrait matrix covers the missing-witness regression while
      the owner signature suite remains green.
- [x] Batch 521 closes module-owner aliasing in canonical layout products.
      `Layouts` now records the module that owns its local definition maps, and
      every nominal type, struct/union field, and enum lookup rejects a
      `GlobalDefId` from another module before consulting the local `DefId`.
      An owner regression uses equal local definition numbers from distinct
      modules to prove foreign ABI/layout queries cannot read the wrong
      aggregate representation.
- [x] Batch 522 removes the duplicate module owner from backend layout
      conversion. `BackendLayouts::from_module_layouts` now qualifies ordinary
      aggregates and concrete instances exclusively with the source
      `Layouts.module_id`; callers can no longer relabel one module's local
      layout maps with another module identity. The internal instance-key
      constructor is no longer public, and a backend-IR owner regression locks
      both conversion paths to the canonical product owner.
- [x] Batch 523 closes module-owner loss in LLVM aggregate field queries.
      `ModuleCodegen::field_index` and `field_offset` now reattach each
      layout-local field slot to the aggregate's `GlobalDefId.module_id` before
      comparing it with a backend field identity. Equal local field numbers
      from another module can no longer produce a valid GEP index or offset;
      the owner helper regression covers both accepted and rejected identities.
- [x] Batch 524 closes backend aggregate-member owner drift. Declaration
      validation now rejects struct/union fields, named enum payload fields,
      and enum variants whose `GlobalDefId.module_id` differs from the owning
      aggregate. LLVM enum-variant lookup also preserves the complete owner
      identity instead of matching only a local variant slot. The backend
      regression exercises a foreign field with the same local number before
      LLVM receives the malformed module.
- [x] Batch 525 closes ordinary backend-definition owner drift. Declaration
      validation now requires non-instantiated functions, globals, structs,
      unions, enums, and their nominal layout keys to belong to the source
      `BackendModule`; generic instances retain their separate materialization
      owner contract. An owner regression and an end-to-end malformed Backend
      IR case prove a foreign definition is rejected before LLVM emission.
- [x] Batch 526 closes the publication-order aliasing window for ordinary
      backend identities. `ProgramIndex` now filters nominal item and layout
      positions by the requested `GlobalDefId.module_id` before returning them;
      a malformed foreign key published ahead of validation cannot shadow a
      valid module's lookup. Instance indexes intentionally retain their
      separate actual-materialization owner contract, with a direct index
      regression covering both accepted and rejected positions.
- [x] Batch 527 makes declaration validation total over stale ordinary-item
      membership. Function and global owner queries now apply the same module
      filter as item queries, and function/global/struct/union declaration
      validation turns any missing item or published owner into
      `INVALID_BACKEND_IR` instead of reaching an `expect` panic. Generic
      instance ownership remains unchanged; the diagnostic contract regression
      locks the recoverable failure boundary.
- [x] Batch 528 makes enum discriminant range checking target-owned. Const
      checking now evaluates `isize`/`usize` backing ranges from the artifact
      pointer width instead of the compiler host, preventing an ILP32 enum from
      accepting a 64-bit implicit tag. Backend validation independently
      requires an integer backing type and proves every effective discriminant
      representable before LLVM constant construction can truncate it. Source
      and malformed Backend IR regressions cover the 32-bit boundary.
- [x] Batch 529 validates enum layout headers before LLVM. Every published enum
      layout now has a unique matching definition and declaration-ordered
      variant identities; its tag must equal the target layout of the integer
      backing type, and its payload offset and total storage are recomputed from
      checked payload size/alignment facts. A partitioned malformed Backend IR
      regression combines a four-byte backing type with a forged one-byte tag
      and byte-one payload offset, proving both inconsistencies stop before
      LLVM construction.
- [x] Batch 530 validates enum payload field layouts before LLVM. Tuple and
      named payloads now require declaration-matching field counts, identities,
      target type layouts, aligned offsets, tail-padded variant storage, and
      in-bounds field extents. Enum payload offsets and total storage are now
      derived from those recomputed variant layouts rather than trusting the
      published payload metadata. The partitioned malformed Backend IR
      regression forges named-field identity, representation, placement,
      bounds, and payload size and proves every mismatch is diagnosed before
      LLVM byte-offset stores or projections.
- [x] Batch 531 validates independently reproducible type layout products before
      LLVM. Repeated module-local type layout keys must agree and every layout
      must have a valid padded size/alignment contract. Primitive, vector, thin
      pointer, function-pointer, slice, trait-object, and callable layouts are
      recomputed from canonical type identity and the artifact target, so a
      published entry cannot override LLVM's scalar or pointer ABI. The
      malformed partition regression publishes a repeated `u8` key whose
      second value forges an eight-byte representation and proves both the
      conflicting value and target mismatch stop before LLVM.
- [x] Batch 532 validates ordinary struct and union layout products before
      LLVM. Layouts paired with executable Backend IR definitions now rebuild
      every field representation from its runtime type, preserve declaration
      order for extern structs and unions, reproduce Nia struct
      alignment/size ordering, overlay union fields at byte zero, and check
      field identities, offsets, bounds, tail padding, and total storage.
      Type-only orphan layouts remain valid inputs for cross-module queries.
      The malformed partition regression forges a Nia struct in source order
      instead of physical order and proves its identities and offsets are
      rejected before LLVM GEP or promoted-constant construction.
- [x] Batch 533 extends aggregate layout validation to materialized generic
      struct and union instances. Exact instance layout keys must be unique,
      every type and const argument type must belong to the active compilation
      session, and layouts paired with a matching Backend IR materialization
      are rebuilt from its substituted fields using the same struct/union ABI
      rules as ordinary definitions. Type-only instance layouts remain valid
      for nominal queries. The malformed partition regression publishes a
      duplicate generic struct key with undersized storage and proves the key,
      field bound, and total-layout violations all stop before LLVM.
- [x] Batch 534 extends independently reproducible type layout validation to
      tuples, closure states, arrays, ranges, optionals, and error unions.
      Published structural layouts are rebuilt from their component layouts,
      array lengths, and the artifact target instead of trusting the root
      publication; missing external const or component facts preserve sparse
      publication, while invalid layout arithmetic is rejected. The malformed
      partition regression publishes an undersized `(i16, i32)` tuple layout
      and proves it cannot override LLVM's structural ABI.
- [x] Batch 535 binds published nominal type layouts to their detailed
      aggregate products. Ordinary struct, union, and enum nominal values now
      reuse the matching definition layout, while concrete generic nominal
      values use exact or canonically equivalent struct/union instance keys.
      Missing type-only products remain sparse and do not create a publication
      ordering requirement. The malformed partition regression forges both an
      ordinary struct nominal value and a generic struct-instance nominal value
      and proves neither can override its checked aggregate representation.
- [x] Batch 536 rejects layout products for descriptor-only and unresolved
      types. Opaque and unsized pointee descriptors, builtin types/traits,
      generic and Self parameters, const-only and error types can no longer
      acquire a forged runtime ABI through `BackendLayouts::types`; projection
      and nominal alias products remain sparse because normalization context is
      intentionally outside Backend IR. Validator layout fallback now agrees
      with layout computation and LLVM lowering that builtin traits have no
      by-value representation. The malformed partition regression covers both
      a forged builtin-trait product and an unpublished builtin trait in a
      runtime return position.
- [x] Batch 537 validates the artifact target layout independently of published
      type products. Every Backend IR validation path now requires a supported
      byte-addressable 8- through 128-bit pointer width with the corresponding
      pointer alignment before target-derived fallback layouts or LLVM integer
      types are constructed. A declaration-only empty-module regression uses a
      four-byte pointer with two-byte alignment and proves the target contract
      is rejected even when no function or type layout can expose it
      indirectly.
- [x] Batch 538 enforces one artifact target across the complete BackendProgram.
      Every validator compares its target with the complete module store, and
      batch plus readiness emitters run a program preflight before LLVM,
      fingerprint, cache, or compiler-builtin tasks can produce artifacts. A
      pair of empty declaration-only LP64 and ILP32 modules proves individually
      valid target layouts cannot share a codegen program or its global
      type/layout indexes.
- [x] Batch 539 independently reproduces the source foreign-variadic
      declaration contract at the Backend IR boundary. Ordinary functions and
      concrete function instances now require every extern variadic signature
      to retain at least one fixed parameter and reject a locally emitted body,
      using the payload's actual body presence rather than its current codegen
      partition role. A malformed extern variadic definition with no fixed
      parameters proves both violations stop before LLVM variadic function
      type construction.
- [x] Batch 540 independently reproduces the source `@[naked]` attribute
      contract at the Backend IR boundary. Ordinary functions and concrete
      instances now reject a forged naked attribute unless they are extern
      definitions carrying a body, preventing invalid LLVM function
      attributes from being attached to declarations or normal Nia functions.
      A malformed non-extern declaration regression proves the attribute is
      diagnosed before LLVM module construction.
- [x] Batch 541 independently reproduces the source C ABI type contract at
      the Backend IR boundary. Extern function parameters/returns, globals,
      extern struct fields, nested function-pointer signatures, and materialized
      extern aggregate instances now reject Nia-only representations such as
      `bool`, tuples, optionals, variadic function pointers, and `char` before
      LLVM function or global types are constructed. Exact and canonically
      equivalent generic instance keys share the same field walk. Pointer forms remain
      opaque to this by-value classifier, preserving valid `&opaque` and other
      C pointer declarations; malformed extern declarations are covered by a
      pre-LLVM regression.
- [x] Batch 542 binds concrete function-instance ABI metadata back to its
      source template. Extern, variadic, and function-attribute flags on an
      instance must match the generic declaration that produced it, so forged
      instances cannot change calling convention or attach `naked` after
      materialization. A generic-template/instance regression covers all three
      metadata dimensions before LLVM declaration emission.
- [x] Batch 543 binds concrete struct and union instance `is_extern` metadata
      back to their source templates. Since aggregate layout reconstruction
      uses this flag to choose foreign declaration order versus native Nia
      placement, forged instance flags are now rejected before layout and LLVM
      type emission; a paired generic struct/union regression covers the
      boundary.
- [x] Batch 544 binds substitution-invariant instance structure back to source
      templates. Function instances must retain the source name and ordered
      parameter local/name/receiver metadata; struct and union instances must
      retain the aggregate name plus ordered field identities and names. Types
      remain validated after substitution by the existing runtime/layout
      checks. Forged function and paired aggregate regressions prove instance
      materialization cannot silently reshape declarations before LLVM.
- [x] Batch 545 validates every concrete function, struct, and union instance
      key independently of references and layout publication. Self/type
      arguments plus const-argument types must belong to the active type-store
      session, and total type/const arity must match an available source
      template. Forged function/union arities and a cross-session struct key
      prove stale or truncated identities stop before fingerprinting or LLVM
      emission.
- [x] Batch 546 extends unconditional instance-key validation to generic local
      statics and binds reconstructible global-instance metadata to its source
      global. Type and const-argument types must belong to the active session;
      name, mutability, and initializer presence must be preserved, and an
      extern global can never be materialized as local instance storage. A
      cross-session forged instance regression covers every available contract
      before fingerprint or LLVM global emission. Generic arity remains owned
      by the enclosing function because BackendGlobal intentionally carries no
      parent/generic signature.
- [x] Batch 547 validates external symbol metadata before LLVM. Extern
      functions and globals must publish a nonempty, NUL-free link name, while
      non-extern items cannot override their compiler-owned symbol; extern
      functions also cannot retain generic parameters. The checks run before
      generic-template validation returns, preventing malformed symbol and
      fingerprint metadata from bypassing the Backend IR boundary. A combined
      regression covers missing, forged, empty, and NUL-containing link names.
- [x] Batch 548 validates generated instance and closure symbols before LLVM.
      Concrete function, global, struct, and union instances plus closure
      entries now reject empty or NUL-containing symbols before LLVM declaration
      APIs receive them. Existing instance metadata regressions cover malformed
      generated symbols across every materialized declaration category.
- [x] Batch 549 makes generated symbol uniqueness a program preflight
      invariant. Non-extern functions, globals, and concrete value instances
      cannot reuse one linker symbol; struct and union instance type names
      cannot collide within one LLVM context. Closure symbols remain governed
      by their stronger owner-derived identity check, and extern link names
      remain independently repeatable across modules. A malformed program
      crosses instance categories with shared value and type names and is
      rejected before artifact acceptance.
- [x] Batch 550 binds generated instance symbols to their concrete identity.
      Function and global instances reproduce the contextual module suffix;
      function receivers preserve their dedicated `self` encoding; aggregate
      instances reproduce the canonical type/const mangle using published
      source identities, nominal names, and array-length facts whenever all
      mangle inputs are representable in Backend IR. Sparse readiness and
      source identities intentionally absent from Backend IR defer rather than
      guessing a name. The cross-category malformed-symbol regression now
      proves every instance kind rejects a unique but forged name independently
      of collision checks.
- [x] Batch 551 scopes template-local promoted allocations to their concrete
      function instance. The module registry and link-once symbol include the
      already-validated owner identity when an allocation origin lies inside a
      monomorphized template, preventing two type or const substitutions from
      silently sharing the first emitted pointee or initializer. Promotions
      originating outside that template retain their source module/span
      identity, so imported and passed-in frozen const provenance remains
      shared. A hand-built Backend IR regression materializes distinct const
      initializers at one template span and proves both globals survive LLVM
      emission.
- [x] Batch 552 makes promoted-allocation reuse validate the emitted constant,
      not only its pointee type. Registry entries retain the canonical LLVM
      initializer and reject a repeated owner/allocation key whose value
      differs, preventing malformed Backend IR from silently discarding every
      initializer after the first. The violation is classified as internal
      `INVALID_BACKEND_IR`; a same-owner array-pointer regression forces two
      conflicting constants through materialization and verifies rejection.
- [x] Batch 553 closes the external/generated LLVM value-namespace boundary.
      Program preflight now reserves ordinary compiler-owned functions and
      globals, concrete instances, and validated closure-entry symbols before
      scanning extern link names. Same-kind extern declarations remain
      repeatable across modules, while extern function/global kind conflicts
      duplicate extern function definitions, and any extern claim on a
      compiler-owned symbol are rejected before artifact acceptance rather
      than relying on LLVM's identity-changing numeric rename. A combined
      regression covers both generated categories, the extern cross-kind and
      multiple-definition collisions, and allowed same-kind declarations.
- [x] Batch 554 extends native symbol preflight through the synthetic
      compiler-builtins unit. The builtin collector exposes the exact external
      definitions selected by reachable wide integer and float-conversion IR;
      native emission rejects extern functions or globals that claim one of
      those active symbols before cache lookup or builtin object creation.
      Helper-like names remain available when the corresponding builtin is not
      requested. A source-to-object regression covers both the conditional
      allowance and function/global collisions with the requested unsigned
      division and remainder helpers.
- [x] Batch 555 repairs backend target-layout iteration across the three module
      readiness states. A backend module is registered before lowering writes
      its payload and written before the index publishes it; partition
      definition validation deliberately runs inside the first window. Target
      agreement is a whole-store property, so iteration still spans written but
      unpublished modules, while a registered-but-unwritten slot is skipped
      instead of indexed. Previously it was indexed unconditionally, so any
      partition that became ready before the last module finished lowering
      aborted codegen with an `INVALID_BACKEND_IR` ICE; the multi-module LLVM
      driver case failed roughly two runs in three. A direct owner regression
      publishes one of two registered modules and pins the skip, beside the
      existing test that pins the written-but-unpublished span.
- [x] Batch 556 extends native symbol preflight through trait-object dispatch
      tables. Vtable globals are externally visible, so program preflight now
      reproduces the emitter's vtable symbol for every published table and
      reserves it in the same value namespace as functions, globals, concrete
      instances, and closure entries. The reproduction mirrors the emitter's
      constant const-expression resolver rather than the richer instance
      mangling, defers when a mangle input is absent from Backend IR, and
      rejects two distinct vtable identities that resolve to one symbol. A
      source-level regression proves an extern global can no longer claim a
      live vtable symbol; without the reservation LLVM silently renamed the
      table to `<symbol>.1` and left every dispatch site following the renamed
      identity while the extern kept the requested linker name.
- [x] Batch 557 stops Backend IR validation from treating an unpublished module
      as a missing one. Nominal type owners and static-array-pointer origins are
      pure identity checks and now require only registration, which holds for
      the whole session. Array-length const-expression owners and union-storage
      relocations read module payloads, so they reject an unregistered owner but
      defer while a registered owner is still unwritten. Previously all four
      used the publication accessor, so whichever partition was validated first
      reported every such owner belonging to a later module as missing. The
      backend module store gained an explicit `is_registered` predicate and the
      program index separate registered/written accessors, with an owner
      regression pinning all three states independently. The complete
      `std_hash_map` CLI suite now passes 7 of 7, having failed 7 of 7 before
      this batch and batch 555.
- [x] Batch 558 restores the registration-based nominal-owner guard after a
      temporary regression in the backend validator. The validator now accepts
      nominal identities for both registered-but-unwritten and written-but-
      unpublished owners while still rejecting foreign module ids. Focused
      readiness tests cover all three module states; the `nia-codegen-llvm`
      library suite (306 tests), workspace check, and strict Clippy pass.
- [x] Batch 559 adds the missing owner-state matrix for array-length
      const-expressions. Validation now has direct coverage for registered but
      unwritten owners (defer), written but unpublished owners (read payload),
      and foreign owners (reject), matching the readiness contract used by the
      backend validator.
- [x] Batch 560 closes the LLVM bitcode input boundary. Empty serialized input
      is rejected before `LLVMParseBitcodeInContext2`; without this preflight,
      LLVM treats an empty buffer as a fatal parser error and terminates the
      process instead of returning a recoverable diagnostic. An owner-level
      regression pins the typed error path.
- [x] Batch 561 extends the LLVM bitcode boundary to truncated and
      non-bitcode buffers. `Context::parse_bitcode_module` now recognizes both
      LLVM-documented raw and wrapped signatures before entering the C parser,
      rejecting empty, short, and wrong-magic input through one typed error
      path. This prevents obvious malformed buffers from reaching LLVM's fatal
      short-header diagnostic while preserving both supported bitcode forms.
- [x] Batch 562 closes the LLVM debug-info subrange handle boundary.
      `DebugInfoBuilder::create_array_type` now checks the metadata returned by
      `LLVMDIBuilderGetOrCreateSubrange` before passing it to the array-type
      constructor, keeping a null intermediate out of LLVM's metadata graph.
      A real DI array construction regression covers the successful path.
- [x] Batch 563 adds direct target-machine boundary coverage. Invalid target
      triples now have an owner-level regression that requires a typed error,
      while a minimal native module configures target data/triple and emits a
      non-empty object buffer through the same wrapper used by codegen. This
      pins both target construction failure handling and successful buffer
      ownership without relying on a larger compiler fixture.
- [x] Batch 564 diagnoses the previously ambiguous workspace-test timeout.
      A resource-traced `configured_success` run acquired its build session and
      launched `nia build`; the long phase was the real configured `build check`
      compiler work, not a permit wait or executor permission failure. A full
      workspace run now returns an explicit shell timeout (`124`) when its
      external 180-second budget expires before all CLI cases finish. No test
      serialization or resource-accounting change is justified by this
      evidence; retain the 180-second invocation as an insufficient ad-hoc
      budget rather than changing test concurrency or resource accounting.
- [x] Batch 565 repairs lossless syntax delimiter ownership under malformed
      input. Green-tree construction now closes a delimiter only when the
      closing token matches its opener; mismatched closers remain in the active
      node so unmatched delimiters, spans, and child paths stay structurally
      faithful for parser recovery and incremental origins. A `([)]` regression
      covers the nested mismatch and exact source reconstruction. `nia-syntax`
      (12 tests) and `nia-parser` (119 tests) pass with strict Clippy.
- [x] Batch 566 completes the build-case timeout diagnosis with a proportionate
      gate. The full `nia-cli --test build_cases` suite passes 14/14 in 318.67s
      under a 1800-second external budget, including the previously long
      configured build. The earlier 180-second workspace command therefore
      expired before the suite could finish; no executor-permission or resource
      permit defect was observed. Full-workspace acceptance should use a budget
      that reflects this measured build workload rather than serializing tests.
- [x] Batch 567 closes the full workspace test evidence gap. With the normal
      libtest concurrency and shared resource accounting, `cargo test
      --workspace --no-fail-fast` completed with exit code 0 under a 1800-second
      outer budget. This run covered the previously slow CLI build cases,
      compiler-query suites, runtime executable tests, cache/loader tests, and
      all workspace doctests; no test failures were reported. The old 180-second
      timeout was solely an inadequate external command budget.
- [x] Batch 568 repairs nested LLVM array-constant construction. The typed
      `ArrayType::const_array` wrapper now validates each operand against the
      outer array type and passes that same type to `LLVMConstArray2`; using the
      inner element type previously rejected valid nested arrays and could send
      an invalid element contract across the FFI boundary. A real `[[i32; 2];
      2]` owner regression covers the constructed value type; `nia-llvm` and
      `nia-codegen-llvm` suites plus strict Clippy pass.
- [x] Batch 569 aligns fixed-vector array initializers across backend validation
      and LLVM lowering. `VectorType` now owns a checked constant-array
      constructor, and static initializer lowering dispatches fixed-vector
      elements through it instead of rejecting an array shape already admitted
      by Backend IR validation. Owner coverage checks the LLVM value type, while
      a complete BackendProgram regression emits a `[2 x <4 x i32>]` global;
      workspace check, both owner/consumer suites, and strict Clippy pass.
- [x] Batch 570 completes named-const SIMD materialization in static data.
      `StaticInit::Vector` preserves lane identity independently from array and
      repeat aggregates through const lowering, generic instantiation,
      reachability, fingerprints, Backend IR validation, and LLVM constant
      emission. Simplification folds only all-zero vectors, retaining repeated
      nonzero lanes as vectors. Backend validation rejects wrong lane counts and
      primitive lane kinds before LLVM; a source-level regression emits named
      vector constants both directly and inside an array, while hand-built IR
      coverage pins malformed-lane rejection and zero-vector array emission.
      The affected lowering, body-check, and LLVM suites, workspace check, and
      strict Clippy pass.
- [x] Batch 571 completes positional tuple materialization in static data and
      aligns the static checker's const-value admission with the materializer.
      `StaticInit::Tuple` preserves positional identity independently from
      arrays, vectors, and nominal fields through direct and named-const
      lowering, generic instantiation, reachability, fingerprints,
      optimization, Backend IR validation, and LLVM struct constants. Only an
      all-zero tuple folds to `Zero`; repeated nonzero positions remain a
      tuple. Static checking now recursively admits tuple and fixed-vector const
      values when every leaf is representable, closing the SIMD admission gap
      left outside batch 570. A source-level regression emits direct, named,
      array-contained, and zero-sized tuples, preserving the backend's `{}`
      canonical representation for zero-sized values. Malformed Backend IR
      coverage rejects both arity and element-type mismatches before LLVM.
      `nia-static-check` (14 tests), `nia-static-ir` (4), `nia-body-check` (266),
      `nia-backend-lower` (116), and `nia-codegen-llvm` (311) pass with strict
      Clippy and workspace check.
- [x] Batch 572 restores the LLVM DIBuilder subroutine-type contract and makes
      it explicit in the typed API. LLVM reserves metadata slot zero for the
      return type (`null` for `void`), followed by parameter types; treating
      the list as empty discarded the required void-return marker. The wrapper
      now accepts return and parameter types separately, emits them in that
      order, and checks their combined `u32` count. Owner regressions attach
      void and typed signatures to real `DISubprogram`s, inspect the referenced
      metadata list, and cover the count boundary. The `nia-llvm` owner suite,
      workspace check, strict Clippy, formatting, and diff checks pass.
- [x] Batch 573 closes the LLVM DIBuilder array-length boundary. Debug array
      construction now rejects negative element counts before passing them to
      the signed subrange API, preventing malformed DWARF ranges from crossing
      the typed wrapper. An owner-level regression pins the recoverable error;
      the existing successful subrange construction remains covered.
- [x] Batch 574 closes the LLVM instruction-query opcode boundary.
      `InstructionValue::get_allocated_type` now requires an `alloca` opcode
      before calling LLVM's allocation-specific inspection API. Owner-level
      regressions cover both rejection of an integer `add` and successful type
      recovery from a real `alloca`; the affected LLVM suite, workspace check,
      strict Clippy, Rustdoc, formatting, and diff checks pass.
- [x] Batch 575 closes LLVM attribute context-lifetime ownership. `Attribute`
      now carries the lifetime of the `Context` arena that allocated its raw
      handle, and context creation plus function attachment preserve that
      lifetime through their signatures. A compile-fail owner regression proves
      an attribute cannot escape a dropped context; the LLVM owner and codegen
      consumer suites, workspace check, strict Clippy/Rustdoc, formatting, and
      diff checks pass.
- [x] Batch 576 closes LLVM DIBuilder module-lifetime ownership.
      `DebugInfoBuilder` now carries a distinct borrow of the `Module` that
      created it, in addition to the module's context lifetime, so finalize and
      disposal cannot run after the owning module has been freed. A compile-fail
      owner regression proves a builder cannot escape a local module; LLVM and
      codegen checks plus strict doctests cover the updated public boundary.
- [x] Batch 577 closes a Phase A object-safety recursion gap. Object-safety
      traversal now visits builtin array-length types plus every const-argument
      type in nominal values, trait objects, projections, and associated
      bindings. `Self` hidden in `std::builtin::size[Self]()` previously let an
      ABI-dependent method enter a trait object; the checker now rejects it,
      while a concrete `u32` layout length remains accepted. Erased-type
      reconstruction recursively normalizes the same metadata. Focused driver
      regressions cover both rejection and the valid control case;
      `nia-body-check` (266 tests), the trait-object driver suite (29), and
      `nia-compiler-query` (255) pass with workspace check and strict Clippy.
- [x] Batch 578 makes object-safety projection binding lookup compare complete
      const-generic trait instances. The object context now retains its root
      trait const arguments, and projection normalization compares const
      arguments alongside trait identity and type arguments for root and
      inherited bindings. A pair of `Slot[1]`/`Slot[2]` regressions proves that
      `Item = Self` on only the second instance is rejected while distinct
      concrete bindings remain object safe. `nia-body-check` (266 tests), the
      trait-object driver suite (31), and `nia-compiler-query` (255) pass with
      workspace check and strict Clippy.
- [x] Batch 579 closes backend recursive-filter coverage for array layout
      metadata. Backend instance depth, generic-parameter, unresolved-
      projection, and error walkers now recurse into the type operand of
      `ArrayLenTy::Builtin` in addition to the array element. Owner coverage
      proves all three filter classes detect generic, projection, and error
      operands; `nia-backend-lower` (117 tests), workspace check, and strict
      Clippy pass.
- [x] Batch 580 closes the monomorphization depth recursion gap for array
      layout metadata. `ty_exceeds_instance_depth` now visits the type operand
      of `ArrayLenTy::Builtin`, so deeply nested types hidden behind `size[T]`
      or `align[T]` cannot bypass the convergence diagnostic. A direct owner
      regression covers the previously accepted path; `nia-monomorphize` (13
      tests), workspace check, and strict Clippy pass.
- [x] Batch 581 closes const-check generic-presence recursion for array layout
      metadata. `type_contains_generic_inner` now visits builtin array-length
      operand types plus nominal, trait-object, projection, and associated
      binding const-argument types, so an expected `size[T]` array remains
      eligible for inference from later arguments. A focused const-check
      regression proves the premature `cannot infer ... T` diagnostic is gone;
      `nia-const-check` (40 tests), workspace check, and strict Clippy pass.
- [x] Batch 582 closes body-check generic-shape recursion for array layout
      metadata. `type_contains_generic_param` now visits the type operand of
      `ArrayLenTy::Builtin`, preventing an incomplete `size[T]` expected array
      from forcing premature literal layout evaluation. Focused regressions
      cover successful inference, a real length mismatch, and the unresolved
      generic diagnostic without the misleading layout error; `nia-body-check`
      (269 tests), workspace check, and strict Clippy pass.
- [x] Batch 583 closes backend aggregate-instance recursion for array layout
      metadata. Struct/union instance collectors and module type registration
      now visit `ArrayLenTy::Builtin.ty`, retaining nominal layout products
      referenced only through `size[...]`/`align[...]`. A lowering regression
      covers a nested struct used solely by array layout metadata;
      `nia-backend-lower` (118 tests), workspace check, and strict Clippy pass.
- [x] Batch 584 closes program-signature substitution of array layout metadata.
      `substitute_type` now recursively substitutes `ArrayLenTy::Builtin.ty`,
      so generic trait and extension signatures do not retain stale type
      operands inside `size[T]`/`align[T]`. An owner regression verifies the
      lowered array metadata uses the concrete substitution;
      `nia-program-signatures` (5 tests), workspace check, and strict Clippy
      pass.
- [x] Batch 585 closes body-check inference through array layout operands.
      Generic shape matching and array type inference now recurse through
      equivalent `ArrayLenTy::Builtin` operands, allowing `size[T]`/`align[T]`
      to provide the sole type evidence while preserving real mismatch
      diagnostics. A focused generic-inference regression covers this path;
      `nia-body-check` (270 tests), workspace check, and strict Clippy pass.
- [x] Batch 586 closes backend extension-pattern matching for array layout
      operands. Extension target matching now compares equivalent builtin array
      lengths structurally and stages any type substitutions, so `size[T]` /
      `align[T]` patterns cannot miss a receiver or leak bindings after a later
      element mismatch. The backend-lower owner suite (118 tests), workspace
      check, and strict Clippy pass.
- [x] Batch 587 closes backend extension-pattern recursion through nominal
      const-argument types. Generic-presence and bound checks now visit
      `ConstGenericArg.ty` for nominal patterns, keeping hidden type parameters
      visible to matching and preventing stale substitutions. The
      `nia-backend-lower` owner suite (118 tests), workspace check, and strict
      Clippy pass.
- [x] Batch 588 closes body-check method-pattern matching for array layout
      operands. Array patterns now compare equal `ArrayLenTy::Builtin` kinds by
      recursively matching their operand types and commit type/const
      substitutions only after the element also matches. A method regression
      infers an extension target `[u8; size[T]()]` from a concrete receiver;
      `nia-body-check` (271 tests), workspace check, and strict Clippy pass.
- [x] Batch 589 closes trait-solver impl-pattern matching for array layout
      operands. User impl candidates now recursively match type operands of
      equivalent `ArrayLenTy::Builtin` lengths within the existing staged
      substitution transaction. A solver regression proves type-generic impl
      selection through `size[T]`; `nia-trait-solve` (17 tests), workspace
      check, and strict Clippy pass.
- [x] Batch 590 closes executable-reachability recovery through array layout
      operands. Reachable extension matching now recursively compares the type
      operands of equivalent `ArrayLenTy::Builtin` values using each side's
      `TypeStore`, recovering hidden type substitutions without cross-store
      handle assumptions. An owner regression covers generic recovery;
      `nia-executable-reachability` (15 tests), workspace check, and strict
      Clippy pass.
- [x] Batch 591 closes method-specificity ordering for array layout operands.
      Structural subsumption now recursively matches the type operands of
      equivalent `ArrayLenTy::Builtin` values, so a concrete
      `[u8; size[i32]()]` extension outranks a generic `[u8; size[T]()]`
      extension instead of producing a false ambiguity. The owner regression
      proves the concrete candidate is selected; `nia-body-check` (272 tests),
      workspace check, and strict Clippy pass.
- [x] Batch 592 closes const-call generic inference through array layout
      operands. Const generic substitution now recurses through equivalent
      `ArrayLenTy::Builtin` operands, allowing a `size[T]` parameter to infer
      `T` from a typed `size[i32]` argument before the final substituted-type
      validation. An owner regression covers operand-only evidence;
      `nia-const-check` (41 tests), workspace check, and strict Clippy pass.
- [x] Batch 593 closes backend vtable equivalence for array layout operands.
      Vtable payload comparison now structurally compares operand types for
      equivalent `ArrayLenTy::Builtin` values, so distinct but semantically
      equal nominal const representations do not produce false conflicts. The
      backend owner regression covers signed/unsigned const metadata;
      `nia-backend-lower` (119 tests), workspace check, and strict Clippy pass.
- [x] Batch 594 closes monomorphization projection-guard equivalence for array
      layout operands. Projection-key comparison now recursively compares
      operand types for equivalent `ArrayLenTy::Builtin` values, preserving
      deduplication when nominal const metadata has distinct but equal
      representations. The owner regression covers signed/unsigned const
      metadata; `nia-monomorphize` (14 tests), workspace check, and strict
      Clippy pass.
- [x] Batch 595 closes backend aggregate layout-product matching for
      structurally equal const arguments. Materialized struct and union
      declarations now use the validator's structural type/const equivalence
      instead of raw vector equality, so signed/unsigned representations with
      equal bits cannot cause malformed aggregate products to be skipped. An
      owner regression proves the mismatched field layout is diagnosed through
      the equivalent const metadata; `nia-codegen-llvm` (312 tests), workspace
      check, strict Clippy, formatting, and diff checks pass.
- [x] Batch 596 closes backend semantic array matching for layout builtin
      operands. `ModuleLowerer::types_match` now recursively compares operand
      types when both lengths use the same `size`/`align` builtin, while
      preserving exact matching for other length forms. The owner helper test
      covers recursive operand dispatch and builtin discrimination;
      `nia-backend-lower` (120 tests), workspace check, strict Clippy,
      formatting, and diff checks pass.
- [x] Batch 597 closes body-check projection-obligation equivalence for layout
      builtin operands. Structural equivalence now recursively compares the
      operand types of same-kind `size`/`align` lengths, preventing rebuilt
      projection keys from diverging on semantically equal array metadata while
      preserving exact identity for other length forms. The owner helper test
      covers recursive dispatch and builtin discrimination; `nia-body-check`
      (273 tests), workspace check, strict Clippy, formatting, and diff checks
      pass.
- [x] Batch 598 closes LLVM static function-address instance lookup for
      structurally equal type arguments. The validator fallback now uses its
      structural type equivalence alongside canonical const comparison, so an
      equivalent nominal argument cannot make an instance ABI mismatch escape
      validation. An owner regression uses distinct but equivalent nominal
      handles and a deliberately mismatched parameter ABI;
      `nia-codegen-llvm` (313 tests), workspace check, strict Clippy, formatting,
      and diff checks pass.
- [x] Batch 599 closes backend semantic matching for trait-object pointee
      identities. `ModuleLowerer::types_match` now compares the trait identity,
      recursively matched type and const arguments, and unordered associated
      type bindings instead of treating rebuilt `TraitObjectPointee` handles as
      unequal leaves. The owner regression verifies dispatch across all three
      structural payload classes and mismatch short-circuiting;
      `nia-backend-lower` (121 tests), workspace check, strict Clippy,
      formatting, and diff checks pass.
- [x] Batch 600 closes trait-solver structural equivalence for array layout
      builtin operands. Both the shared `TypeEquivalence` adapter and the
      projection-aware array-length relation now recursively compare operands
      of equal `size`/`align` kinds, so projection guards cannot diverge on
      nominal operands with equal const bits but distinct representations. A
      focused owner regression covers equality and builtin discrimination;
      `nia-trait-solve` (18 tests), workspace check, strict Clippy, formatting,
      and diff checks pass.
- [x] Batch 601 closes body-check projection equivalence for trait-object
      views. Projection-obligation structural comparison now handles both
      readonly `TraitObject` values and `TraitObjectPointee` values, recursively
      comparing trait arguments, const metadata, and unordered associated-type
      bindings instead of treating these rebuilt identities as mismatches;
      `nia-body-check` (273 tests), workspace check, strict Clippy, formatting,
      and diff checks pass.
- [x] Batch 602 closes trait-solver projection equivalence recursion for
      container and identity-only type variants. Projection-aware comparison
      now handles tuples, closure states, opaque/builtin/self types, and nested
      projection operands consistently with the shared type equivalence layer;
      a tuple-nested projection regression proves semantic const metadata is
      preserved through recursive comparison. `nia-trait-solve` (19 tests),
      workspace check, strict Clippy, formatting, and diff checks pass.
- [x] Batch 603 closes trait-solver layout lookup recursion for composite keys.
      Layout interner equivalence now walks tuple and closure-state payloads,
      including nested capture/parameter/return types, so rebuilt layout keys
      with semantically equal const metadata remain discoverable. A tuple
      layout lookup regression covers cross-representation nominal arguments;
      `nia-trait-solve` (20 tests), workspace check, strict Clippy, formatting,
      and diff checks pass.
- [x] Batch 604 closes body-check projection-obligation recursion for composite
      associated types. Structural comparison now handles tuples, closure
      states, and identity-only variants consistently with type matching;
      focused where-bound regressions cover tuple-associated outputs and
      nominal const metadata inside tuples. `nia-body-check` (275 tests),
      workspace check, strict Clippy, formatting, and diff checks pass.
- [x] Batch 605 closes backend instantiation type matching for omitted
      `TyKind` shapes. `ModuleLowerer::types_match` now compares tuples,
      closure states, volatile pointers, slice pointees, and identity-only
      variants recursively after substitution; the backend-lower owner suite
      (121 tests), workspace check, strict Clippy, formatting, and diff checks
      pass.
- [x] Batch 606 closes executable-reachability cross-store equivalence for
      composite payloads. `TypedTyRef` structural comparison now handles
      opaque, tuple, and closure-state types recursively, preserving semantic
      matching when cached signatures rebuild nested handles. A dual-store
      tuple regression covers signed/unsigned const representations;
      `nia-executable-reachability` (16 tests), workspace check, strict
      Clippy, formatting, and diff checks pass.
- [x] Batch 607 makes trait-solver impl-pattern inference transactional across
      every recursive type shape. Matching now stages substitutions at each
      recursion boundary and commits them only after the complete composite
      candidate succeeds, so tuple, callable, and nominal mismatches cannot
      leak bindings discovered by earlier fields. A focused tuple regression
      covers the rollback contract; `nia-trait-solve` (21 tests), workspace
      check, strict Clippy, formatting, and diff checks pass.
- [x] Batch 608 closes backend vtable owner identity across rebuilt handles.
      Final module ownership now compares complete vtable keys through
      structural type equivalence before retaining the lexicographically
      stable owner, so semantically equal nominal and const-generic spellings
      cannot emit duplicate tables. An owner regression covers signed/unsigned
      const representations across module-owned interners;
      `nia-backend-lower` (122 tests), workspace check, strict Clippy,
      formatting, and diff checks pass.
- [x] Batch 609 closes backend aggregate-instance owner identity across rebuilt
      handles. Struct and union instance ownership now compares semantic
      nominal keys and recursively equivalent field payloads before retaining
      the deterministic source owner, preventing duplicate materializations
      from module-local interner representations. A cross-module owner
      regression covers structurally rebuilt type arguments and field types;
      `nia-backend-lower` (123 tests), workspace check, strict Clippy,
      formatting, and diff checks pass.
- [x] Batch 610 closes const-check associated-binding inference reuse. Const
      execution now matches pattern and actual associated-type bindings as a
      backtracking bijection, preserving per-candidate substitutions and
      preventing one actual binding from satisfying multiple obligations. A
      regression rejects an otherwise accepted trait object whose second
      binding is incompatible; `nia-const-check` (42 tests), workspace check,
      strict Clippy, formatting, and diff checks pass.
- [x] Batch 611 hardens the shared `nia-ty` equivalence contract. The default
      const-generic comparison now treats integer values by semantic bits,
      matching specialized owner adapters and preventing newly added
      equivalence implementations from silently reverting to representation
      equality. A dual-store nominal regression covers signed/unsigned values;
      `nia-ty` tests, workspace check, strict Clippy, formatting, and diff
      checks pass.
- [x] Batch 612 closes LLVM ProgramIndex instance lookup across rebuilt handles.
      Struct, union, and global instance owner queries now retain exact-key fast
      paths plus definition-grouped semantic fallbacks, recursively comparing
      type and integer const arguments while preserving a global's argument
      module identity. A cross-module global regression covers rebuilt `i32` and
      `usize` handles with signed/unsigned const spellings; `nia-codegen-llvm`
      (314 tests), workspace check, strict Clippy, formatting, and diff checks
      pass.
- [x] Batch 613 extends LLVM ProgramIndex semantic lookup to function
      instances. Function and owner queries now fall back by definition while
      matching the optional receiver, type arguments, integer const bits, and
      argument-module identity across rebuilt handles. The cross-module
      regression covers both global and function instances; `nia-codegen-llvm`
      (314 tests), workspace check, strict Clippy, formatting, and diff checks
      pass.
- [x] Batch 614 completes LLVM ProgramIndex semantic lookup for aggregate
      products. Struct/union instances and their layout records now retain
      definition-grouped fallbacks alongside exact keys, so rebuilt handles
      resolve consistently through declaration, validation, and ABI paths. The
      cross-module regression covers struct and union entities plus layouts;
      `nia-codegen-llvm` (314 tests), workspace check, strict Clippy, formatting,
      and diff checks pass.
- [x] Batch 615 closes LLVM ProgramIndex type-layout lookup across rebuilt
      handles. `type_layout` now keeps its exact map fast path and scans indexed
      candidates through structural `TypeEquivalence` when a composite handle
      comes from another interner. The owner regression covers a rebuilt
      primitive handle; `nia-codegen-llvm` (314 tests), workspace check, strict
      Clippy, formatting, and diff checks pass.
- [x] Batch 616 closes LLVM ProgramIndex trait-object vtable owner lookup
      across rebuilt handles. Vtable ownership now keeps exact key lookup plus a
      complete structural comparison of receiver and object types, preventing
      dispatch misses when type interning is rebuilt. The owner regression uses
      equivalent trait-object handles from separate appenders;
      `nia-codegen-llvm` (314 tests), workspace check, strict Clippy, formatting,
      and diff checks pass.
- [x] Batch 617 extends LLVM ProgramIndex vtable equivalence to value and
      iteration APIs. `trait_object_vtable` and
      `trait_object_vtables_for_object_ty` now share the complete structural
      key fallback used by owner lookup, so rebuilt object handles remain
      discoverable by declaration and dispatch consumers. The existing
      cross-interner vtable regression covers all three APIs;
      `nia-codegen-llvm` (314 tests), workspace check, strict Clippy, formatting,
      and diff checks pass.
- [x] Batch 618 closes body-check associated-binding inference reuse. Generic
      type and const inference now assigns pattern and actual bindings through
      a backtracking bijection, preserving per-candidate substitutions until a
      complete permutation succeeds. A trait-object regression proves a
      generic binding yields an earlier candidate so a later concrete binding
      can use it; `nia-body-check` (276 tests), workspace check, strict Clippy,
      formatting, and diff checks pass.
- [x] Batch 619 closes executable-reachability pattern substitution leakage.
      Extension target matching now treats every recursive type pattern as a
      transaction, so late tuple/nominal/array/callable/trait-object
      mismatches cannot publish partial type, const, or array-length bindings.
      A tuple late-mismatch regression verifies the maps remain unchanged;
      `nia-executable-reachability` (17 tests), workspace check, strict Clippy,
      formatting, and diff checks pass.
- [x] Batch 620 closes generic trait-reachability recursion-key poisoning.
      The path-local same-definition guard now runs before the complete
      instantiation enters `visited`, so a deferred recursive instance can be
      expanded from a later sibling branch instead of being lost permanently.
      The owner regression verifies active definitions leave visited keys
      untouched; `nia-executable-reachability` (18 tests), recursive driver
      tests, workspace check, strict Clippy, formatting, and diff checks pass.
- [x] Batch 621 closes backend-lower associated-binding order sensitivity.
      Trait-object and projection type comparison now matches duplicate
      associated binding keys through a complete backtracking bijection instead
      of a greedy first candidate. Reordered equal bindings remain equivalent,
      while repeated values cannot hide a missing sibling; `nia-backend-lower`
      (124 tests), workspace check, strict Clippy, formatting, and diff checks
      pass.
- [x] Batch 622 closes ProgramIndex array-length equivalence across rebuilt
      const-expression handles. Evaluated `ConstExpr` values now match
      equivalent `ConstValue` and foreign expressions through backend module
      const facts, while unresolved expressions retain identity-only behavior;
      the owner regression covers both positive and negative cases.
- [x] Batch 623 closes backend-lower owner deduplication for evaluated array
      lengths. Aggregate-instance and trait-object-vtable equivalence now share
      lowered modules' const-array facts, so distinct `ConstExpr` handles with
      equal values select one deterministic owner; a cross-module vtable
      regression covers the rebuilt-handle case.
- [x] Batch 624 closes monomorphization projection-cycle equivalence for
      evaluated array lengths. The recursive projection guard now consults
      const-array facts from all participating modules, so rebuilt
      `ConstExpr` handles with equal values converge to one guard key; the
      owner regression covers the cross-module array case.
- [x] Batch 625 closes trait-solver structural equivalence for evaluated array
      lengths. The shared `TypeEquivalence` adapter now delegates non-builtin
      array lengths through the solver's const-expression evaluator, aligning
      structural array comparison with const-generic value comparison; owner
      tests cover evaluated and unresolved expression behavior.
- [x] Batch 626 closes trait-solver layout-interner equivalence for evaluated
      const expressions. Layout-backed `Sized` lookup now compares array
      lengths and nominal const arguments through the solver evaluator, so
      rebuilt handles match only when values resolve; owner tests cover
      cross-module evaluated and unresolved cases.
- [x] Batch 627 closes trait-solver shared const-argument equivalence for
      evaluated expressions. Nominal const arguments in structural type
      comparison now use the configured evaluator, aligning projection and
      trait-goal guards with array-length semantics; owner tests cover
      cross-module evaluated and unresolved cases.
- [x] Batch 628 closes LLVM ProgramIndex nominal const-expression lookup.
      Definition and instance fallback comparisons now reuse published module
      array-length facts for expression-valued const arguments, including
      integer spellings and foreign expressions; unresolved expressions retain
      identity-only behavior in the owner regression.
- [x] Batch 629 closes backend-lower nominal const-expression deduplication.
      Aggregate-instance and trait-object-vtable owner comparisons now reuse
      all lowered modules' array-length facts for nested const arguments;
      evaluated cross-module handles match while unresolved expressions remain
      distinct in the owner regression.
- [x] Batch 630 closes monomorphization nominal const-expression guards.
      Projection-instantiation key comparison now reuses each module's
      evaluated array-length facts for nested const arguments, including
      integer spellings and foreign expressions; unresolved handles remain
      distinct in the owner regression.
- [x] Batch 631 closes program-signature nominal const-expression equivalence.
      Signature comparison now consumes active lowering summaries for literal
      array-length const arguments, allowing evaluated expression handles to
      match integer and foreign-expression spellings while store-only and
      unresolved comparisons remain conservative.
- [x] Batch 632 closes LLVM module-codegen const-expression matching.
      Declaration, generic-instance, and vtable-entry lookups now reuse
      published array-length facts for expression-valued const arguments;
      the evaluator-aware helper matches integer/foreign spellings while its
      no-facts wrapper remains identity-only.
- [x] Batch 633 closes body-check projection array-length equivalence for
      evaluated const expressions. Projection and trait-object structural
      comparisons now consume module/program array-length facts, allowing
      rebuilt expression handles to match equivalent values while unresolved
      expressions remain distinct.
- [x] Batch 634 closes LLVM backend-validator nominal const-expression
      matching. Aggregate, function, global, and vtable reference validation
      now reuses published array-length facts, so evaluated expression handles
      match equivalent integer/foreign spellings while unresolved handles stay
      distinct.
- [x] Batch 635 closes backend-lower direct array-type const-expression
      matching. Generic type matching now consumes program array-length facts
      for `TyKind::Array` metadata, aligning direct arrays with nominal const
      arguments while preserving identity-only behavior for unresolved handles.
- [x] Batch 636 closes backend-lower extension array-pattern const-expression
      matching. Extension target recovery now applies program array-length
      facts to non-builtin array lengths, allowing evaluated expression handles
      to match integer and foreign spellings while unresolved patterns remain
      identity-only.
- [x] Batch 637 closes type-lower local equivalence for evaluated const
      expressions. Range-bound and nested nominal comparisons now reuse the
      lowering owner's `ConstExprSummary` literal lengths, allowing distinct
      handles to match only when they resolve to the same value; unresolved
      expressions remain identity-only. `nia-type-lower` (19 tests), owner
      check, strict Clippy, formatting, and diff checks pass.
- [x] Batch 638 closes program-signature supertrait guard equivalence for
      evaluated const expressions. Recursive assumption expansion now compares
      trait goals through active lowering summaries, so rebuilt cross-module
      expression handles cannot bypass the path-local cycle guard; the owner
      regression covers equal evaluated values while unresolved handles remain
      conservative.
- [x] Batch 639 closes the layout array alignment boundary. The public
      `array_layout` helper now rejects zero-alignment element layouts instead
      of returning an invalid `TypeLayout`; the owner regression covers the
      malformed input contract.
- [x] Batch 640 closes the extern-struct unresolved array-length ABI boundary.
      Both the frontend ABI checker and LLVM backend validator now reject
      `Infer` and `GenericParam` field lengths before C layout/codegen; the
      `nia-abi-check` owner regression covers generic extern-struct fields.
- [x] Batch 641 closes the backend builtin-operator metadata/type boundary.
      LLVM backend validation now rejects operators whose trait id does not own
      the selected operation, unsupported non-dispatch operators, and operand
      or result types that violate the direct unary/binary operator contracts;
      malformed call regressions cover trait mismatch and result-shape errors.
- [x] Batch 642 closes the backend builtin-method receiver boundary. Validator
      checks now require each builtin method receiver to match its `self_ty`
      through a direct value or pointer view and reject readonly pointers for
      mutable slice methods; malformed call coverage exercises the mismatch.
- [x] Batch 643 closes the backend function-reference result boundary. Unary
      function-item references now require an exact `FunctionPointer` result
      type instead of accepting ordinary data pointers hidden by LLVM opaque
      pointer types; malformed reference IR has a focused regression.
- [x] Batch 644 closes the backend static-integer range boundary. `StaticInit::Int`
      values are now checked for signedness, primitive width, Unicode scalar
      validity, and target pointer width before LLVM constant construction;
      malformed bool/char/32-bit integer initializers have owner regressions.
- [x] Batch 645 closes the backend vector-static integer lane boundary. Vector
      `IntConst` lanes now reuse scalar signedness and width checks, preventing
      LLVM lane truncation from accepting malformed values; an out-of-range
      vector lane has a focused backend regression.
- [x] Batch 646 closes the backend function-reference signature boundary.
      Function-item address expressions now require the result pointer's full
      function signature to match the referenced item, preventing opaque LLVM
      pointers from hiding incompatible function-pointer metadata.
- [x] Batch 647 closes the backend builtin-integer range boundary. Function
      builtin integer values now validate signedness, primitive width,
      Unicode scalar validity, and target pointer width before LLVM constant
      construction; an out-of-range `bool` value has a focused backend
      regression.
- [x] Batch 648 closes the backend builtin-`usize` width boundary. Function
      builtin `usize` values now fit the configured target pointer width
      before LLVM constant construction; a 32-bit target rejects a `u64`-wide
      builtin value with focused owner coverage.
- [x] Batch 649 closes the backend enum-discriminant range boundary. Enum
      variant values now fit their declared backing primitive before LLVM tag
      construction; malformed explicit discriminants have focused enum
      expression coverage.
- [x] Batch 650 aligns static signed bit-pattern validation with builtin and
      literal lowering. Target-width encodings such as `i32::MIN` are accepted
      while out-of-range static integer initializers remain rejected.
- [x] Batch 651 closes the Function IR function-instance const-type boundary.
      Function-instance references now validate every const generic argument's
      type handle before lookup and LLVM lowering; a foreign-session handle
      has focused owner regression coverage.
- [x] Batch 652 closes the Function IR const-expression owner boundary.
      Const generic arguments now validate `GlobalConstExprId` ownership before
      instance lookup and LLVM lowering: missing modules are rejected, registered
      but unwritten owners remain readiness-deferred, and written owners must
      publish an evaluated array-length fact; a foreign owner has focused
      Function IR regression coverage.
- [x] Batch 653 closes module-level instance and layout-key const-expression
      ownership. Struct, union, global, and function instance metadata plus
      aggregate layout keys now use the same const-argument validator before
      lookup and ABI checks, rejecting missing owners while preserving readiness
      deferral for registered but unwritten modules; function-instance metadata
      has focused foreign-owner coverage.
- [x] Batch 654 closes the backend unresolved-const-value boundary. Const
      generic arguments now reject `GenericParam` values before instance,
      layout, and type lookup, while preserving resolved integer, boolean,
      character, and evaluated-expression values; the shared validator has a
      focused generic-parameter regression.
- [x] Batch 655 closes declaration-membership const-expression readiness.
      Standalone instance metadata, trait-object vtables, vtable entries, and
      referenced function/global instances now add every const-expression
      owner to the dependency closure before lookup; an instance metadata
      regression proves LLVM preparation remains pending until that owner is
      published.
- [x] Batch 656 closes declaration-membership stable type-key collisions.
      Vtable ordering keys now resolve evaluated array-length expressions from
      their owning module's const facts instead of collapsing every expression
      to zero; a focused regression proves distinct evaluated lengths retain
      distinct deterministic keys.
- [x] Batch 657 closes trait-object vtable symbol collisions for const arrays.
      Backend preflight and LLVM emission now encode each `ConstExpr` array
      length from its owner module's evaluated facts instead of mapping every
      expression to `0`; a focused regression proves distinct array lengths
      produce distinct linker symbols.
- [x] Batch 658 closes LLVM trait-object slot lookup fallbacks. Dynamic method
      dispatch and trait-object upcast metadata lookup now propagate an LLVM
      codegen diagnostic when the required vtable metadata or method slot is
      absent or inconsistent, instead of silently substituting the caller slot
      or offset `0`. Existing dynamic-call and upcast matrices continue to
      cover valid direct, inherited, and const-generic supertrait paths.
- [x] Batch 659 closes the typed LLVM struct-GEP boundary. The builder now
      checks that the supplied pointee is a struct and that its field index is
      in range before entering `LLVMBuildStructGEP2`; wrapper regressions cover
      scalar pointees and out-of-bounds fields.
- [x] Batch 660 closes the function-lowering try-kind boundary. Typed `try`
      operands are now required to be `Optional` or `ErrorUnion` before CFG
      construction; malformed primitive operands receive a lowering diagnostic
      instead of silently becoming optional propagation, and a focused input
      regression covers the refusal.
- [x] Batch 661 removes the LLVM enum payload-offset fallback. Enum variant
      emission already rejects fieldless storage before addressing payloads and
      now asserts the validated offset invariant instead of substituting byte
      offset `0` if metadata is internally inconsistent.
- [x] Batch 662 closes backend method receiver metadata fallbacks. Builtin
      operator, place-method, extension-method, and instantiated dispatch now
      report `INVALID_BACKEND_IR` when a resolved method lacks receiver mode
      metadata, instead of silently treating it as a value receiver. Recovery
      retains a value mode only after recording the fatal lowering diagnostic.
- [x] Batch 663 closes the LLVM method receiver ABI classification fallback.
      Method calls now report a codegen diagnostic if receiver classification
      unexpectedly produces no ABI entry, instead of interpreting the empty
      result as an omitted receiver and continuing with a mismatched call.
- [x] Batch 664 closes LLVM declaration-membership panic fallbacks. Missing
      structs, unions, functions, globals, aggregate/function instances, and
      trait-object vtables now return codegen diagnostics from declaration and
      definition setup instead of relying on panic recovery at the outer LLVM
      boundary.
- [x] Batch 665 closes declaration-membership closure and type lookup panics.
      Missing declaration owners and type handles now produce
      `INVALID_BACKEND_IR` diagnostics that propagate through LLVM partition
      readiness, rather than aborting membership construction; focused
      readiness and closure regressions preserve pending behavior for valid
      unpublished dependencies.
- [x] Batch 666 closes backend partition declaration lookup panics. Missing
      function, global, aggregate, or trait-object instance records in a
      declaration membership now produce `INVALID_BACKEND_IR` diagnostics and
      allow validation to continue; a stale function-instance membership
      regression covers the diagnostic boundary.
- [x] Batch 667 closes published-owner readiness assertions. A declaration
      closure that names an owner whose module is published but whose payload
      lacks the requested item now returns `INVALID_BACKEND_IR` instead of
      panicking; the structural-error regression now asserts the diagnostic.
- [x] Batch 668 closes the fingerprint declaration precondition. Source-unit
      fingerprinting now revalidates declaration membership and propagates
      stale-item diagnostics before encoding, so direct incremental/cache
      callers cannot trigger declaration lookup panics; a focused stale
      fingerprint regression covers the returned error.
- [x] Batch 669 closes foreign backend owner dispatch panics. Items discovered
      for a module absent from the final module plan now produce
      `INVALID_BACKEND_IR` diagnostics and are skipped while valid owners
      continue lowering; the finalization contract regression asserts the
      diagnostic instead of expecting a panic.
- [x] Batch 670 closes backend symbol-mangling source-identity panics. Missing
      module identities now produce one deduplicated `INVALID_BACKEND_IR`
      diagnostic per module and use a deterministic placeholder only for
      recovery, preserving lowering progress for other items; focused helper
      coverage verifies stable recovery and diagnostic deduplication.
- [x] Batch 671 closes promoted artifact byte-segment panic recovery. A
      malformed initialized segment containing an absent byte now returns an
      LLVM codegen diagnostic instead of calling `expect`; focused unit tests
      cover both rejection and byte-preserving success behavior.
- [x] Batch 672 closes backend instance type-handle panics. Type normalization
      and const-argument canonicalization now report deduplicated
      `INVALID_BACKEND_IR` diagnostics and recover with the existing error type
      when a session handle is missing; focused tests cover the diagnostic
      contract while normal generic and trait matrices remain green.
- [x] Batch 673 closes closure-entry ABI local lookup panics. Source and
      generic closure materialization now reports `INVALID_BACKEND_IR` and
      skips malformed entries when a state or parameter local is absent,
      preventing invalid ABI metadata from entering later phases.
- [x] Batch 674 closes compiler-builtin scan publication assumptions. Builtin
      discovery now skips registered modules whose backend payload is not yet
      published, preserving already visible modules and avoiding an ICE during
      readiness windows; focused coverage exercises the unpublished-module
      state.
- [x] Batch 675 closes monomorphization source-identity panic recovery. Generic
      symbol generation now records a deduplicated `INVALID_BACKEND_IR`
      diagnostic and uses a deterministic module placeholder when a referenced
      source identity is absent; focused coverage verifies instance symbols
      remain recoverable while the malformed input is reported.
- [x] Batch 676 closes shared mangling missing-type panics. The common type
      mangler now emits an explicit, deterministic `ty_missing` recovery
      encoding for stale `InternedTyId` handles, allowing validator layers to
      report malformed backend state without an earlier ICE; focused tests lock
      the recovery string and its stability.
- [x] Batch 677 closes Function IR reference-walk recovery panics. Value
      reference traversal now marks encountered error expressions and place
      elements while preserving other dependencies, and LLVM declaration
      membership converts that marker into `INVALID_BACKEND_IR`; focused IR
      and membership regressions cover the diagnostic boundary.
- [x] Batch 678 closes LLVM fingerprint stale-handle panics. Fingerprint
      encoding now uses deterministic recovery encodings for missing modules
      and type handles, preserving the existing valid-input byte stream while
      allowing malformed incremental inputs to complete without an ICE;
      focused encoder coverage verifies stable recovery output.
- [x] Batch 679 closes declaration-membership stable-key panics. Definition and
      nominal type sorting now uses deterministic missing-module recovery keys
      instead of aborting when stale ownership metadata reaches ordering;
      focused membership coverage verifies the recovered definition key.
- [x] Batch 680 closes function-optimizer validation ICEs. The optimizer now
      returns structural validation failures explicitly, preserves the original
      body as its recovery product, and backend lowering records the failure as
      `INVALID_BACKEND_IR`; focused coverage verifies malformed input does not
      panic or get rewritten.
- [x] Batch 681 closes the LLVM module-mangling stale-identity panic. Module
      codegen now uses the same deterministic `<missing-module-N>` recovery
      path as backend and fingerprint manglers when a malformed type or symbol
      references an absent module; valid source identities retain their exact
      mangling, with focused stability and distinctness coverage.
- [x] Batch 682 closes the LLVM declaration-task missing-module panic. A stale
      or inconsistent declaration validation task now returns one
      `INVALID_BACKEND_IR` diagnostic instead of dereferencing an absent
      `ProgramIndex` module; the normal published-module path and diagnostic
      code are covered by a focused regression.
- [x] Batch 683 removes backend function-instance fixed-point `Vec::last`
      assumptions. Successful instance materialization now returns its exact
      appended index to discovery, so closure/body scanning cannot panic if the
      append contract changes; existing 128 backend-lowering tests remain green.
- [x] Batch 684 closes resolved const pattern binding `local_id` assumptions.
      Malformed evaluator bindings now return the same stable unresolved-local
      diagnostic as early bindings instead of panicking; direct pattern and
      function-pattern regressions cover both binding paths.
- [x] Batch 685 closes const-check stale type-handle panics. Active type
      lookup now recovers missing store slots as the existing `TyKind::Error`
      sentinel, preserving downstream diagnostics for union, builtin, and
      generic paths; a focused missing-slot regression locks recovery.
- [x] Batch 686 closes frontend item-signature fingerprint span panics.
      Malformed or stale function-body spans now use a deterministic recovery
      fingerprint instead of asserting or indexing invalid UTF-8/source
      ranges; valid body-elision fingerprints remain unchanged.
- [x] Batch 687 removes trait-selection candidate-count panic recovery.
      Unique user-implementation selection now handles an unexpectedly empty
      iterator explicitly, preserving `Unsatisfied` as the conservative result
      while retaining normal ambiguity and unique-selection behavior.
- [x] Batch 688 closes const generic target-range panic recovery. Const-check
      now reports a stable `ConstError` when an `isize`/`usize` generic argument
      is evaluated under an unsupported target pointer width, preserving the
      source span and avoiding an internal range `expect`; a malformed 256-bit
      target regression covers the recovery path.
- [x] Batch 689 closes const-check cross-module type-handle panic recovery.
      Generic type inference now reports span-preserving `ConstError` values
      when a target module context is unavailable or a type handle belongs to a
      foreign store, replacing the previous `expect`/`assert` boundary; focused
      validation coverage locks both malformed-input diagnostics.
- [x] Batch 690 closes LLVM declaration-membership instance sorting panics.
      Struct, union, function, and global instance keys are validated against
      the published program index before deterministic sorting; missing records
      now produce `INVALID_BACKEND_IR` diagnostics instead of reaching sorting
      `unwrap` calls, with a focused stale function-instance regression.
- [x] Batch 691 closes body-check target-layout panic recovery. Body-check
      orchestration now rejects unsupported target pointer widths with a stable
      `TARGET_CONFIG` diagnostic and an empty recovery product instead of
      calling a layout `expect`; owner coverage preserves valid 64-bit layout
      construction and exercises malformed zero/129-bit targets.
- [x] Batch 692 closes parser using-path symbol recovery. Namespace segments
      now propagate symbol-interning failures through the parser's existing
      diagnostic path instead of unwrapping an impossible conversion after
      token classification; existing nested and deep using-path regressions
      preserve valid parsing.
- [x] Batch 693 closes backend instance-owner conflict panic recovery. Program
      lowering now records `INVALID_BACKEND_IR` diagnostics when semantically
      equal aggregate-instance or vtable keys carry conflicting payloads,
      preserving deterministic ownership and allowing centralized validation
      to report malformed products instead of aborting in `assert!`.
- [x] Batch 694 closes parser generic-argument replay panic recovery. Ambiguous
      type/const generic argument reparsing now records a syntax diagnostic and
      skips to the next argument boundary when the selected candidate cannot be
      replayed, instead of unwrapping an assumed-success parse result; malformed
      generic argument coverage preserves parser progress.
- [x] Batch 695 closes parser generic-argument origin transaction recovery.
      Ambiguous type/const generic arguments now reparse both accepted
      interpretations after speculative rollback, preserving their Type and
      Expr origin entries instead of retaining node keys whose origins were
      discarded; a focused type-alias regression verifies both source-versioned
      locators remain available.
- [x] Batch 696 closes body-check foreign type-handle panic recovery. Body
      checking now treats missing or cross-store interned type handles as the
      existing `TyKind::Error` sentinel, allowing established conservative
      diagnostics to continue without dereferencing an invalid store entry; a
      focused owner regression covers both local and foreign handles.
- [x] Batch 697 closes body-check method-pattern foreign-handle panic recovery.
      Structural method candidate matching now routes missing or cross-store
      general pattern handles through the same `TyKind::Error` sentinel and
      conservatively rejects the candidate instead of panicking; the owner
      suite remains green alongside the shared foreign-handle regression.
- [x] Batch 698 closes parser unversioned-tree identity panic recovery. Public
      syntax trees constructed without a source version now use the parser's
      reserved synthetic identity when publishing AST origins, while mixed
      source versions remain rejected; a focused unversioned-tree regression
      verifies source-versioned origin lookup.
- [x] Batch 699 audits backend module publication/readiness, deterministic
      program indexing, and incremental link-input construction. Duplicate
      owners, unregistered or repeated publication, single-consumer readiness,
      stale partition identities, and unsorted link inputs are all internal
      producer/state-machine contracts with explicit validation before the
      public consumers reach them; no malformed external product currently
      bypasses those checks, so no API broadening or recovery shim is justified.
- [x] Batch 700 audits function-IR optimization, const evaluation budgets,
      static-initializer classification, closure escape fixed-point analysis,
      and flow termination tracking. Optimizer entry and pass boundaries retain
      structural validation failures with an unchanged recovery body; const
      session/call limits distinguish bounded evaluation errors from unbalanced
      cleanup bugs; closure defer stacks and flow traversal maintain their
      established lexical invariants. Owner suites and strict Clippy pass with
      no additional malformed-input recovery gap identified.
- [x] Batch 707 audits query executor lifecycle and cross-thread wait safety.
      Worker shutdown closes admission before draining accepted work and joins
      every non-current worker; nested execution reuses the active executor and
      process budget to avoid self-deadlock; wait-for edges are released by RAII
      across cycle, panic, and retirement paths. The `nia-query` owner suite
      (83 tests) and strict `cargo clippy -p nia-query --lib -- -D warnings`
      pass, with no externally reachable lifecycle or resource-retirement gap
      identified.
- [x] Batch 708 records backend/LLVM boundary evidence. `nia-llvm` (94 unit
      tests plus 2 compile-fail docs) and `nia-codegen-llvm` (333 tests) pass;
      strict library Clippy is clean. Null-handle conversion, operand/shape
      validation, target and bitcode failures, debug metadata construction,
      declaration membership, and module-index publication all retain checked
      diagnostics or validated internal contracts. No new externally reachable
      backend safety gap was identified, so Phases B and C remain open pending
      their complete acceptance matrices and cross-module evidence.
- [x] Batch 709 records real executable standard-library process evidence.
      The `nia-cli` `emit_exe_process` suite (47 tests) passes, covering startup
      argv/env exposure, exact and inherited environments, cwd and exec-stage
      failures, spawn/reap retry ownership, repeated wait/try-wait, stdin
      lifetime, all stdio pipe modes, and descriptor-alias cleanup when parent
      standard descriptors are closed. No additional externally reachable
      process or startup-runtime defect was identified; Phase F remains open
      pending its allocator/filesystem and target-matrix acceptance evidence.
- [x] Batch 710 records real executable filesystem and allocator evidence.
      The `nia-cli` `emit_exe_fs` suite (25 tests) and standard allocator suites
      (18 tests) pass. Coverage includes capability-relative path and symlink
      enforcement, directory iteration, non-UTF-8 native paths, file metadata
      and handle ownership, plus fixed-buffer, arena, and general-purpose
      allocator resize/remap/realloc/free failure recovery, forged-header
      rejection, and container integration. No additional externally reachable
      filesystem or allocator defect was identified; Phase F remains open only
      for its remaining target-matrix and final acceptance evidence.
- [x] Batch 711 records a workspace compilation gate. `cargo check
      --workspace --all-targets` passes across every compiler, backend, query,
      runtime-support, driver, and CLI crate in the workspace. This confirms
      the accumulated audit changes compose at the full Rust workspace boundary;
      cross-target builds, full workspace tests, formatting, and final resource
      validation remain separate acceptance requirements.
- [x] Batch 701 audits standard-library allocator, ArrayList, and HashMap
      arithmetic and ownership boundaries. Public range operations validate
      indices before private stable-replacement helpers, probing calls only
      derive set-bit indexes from non-empty masks and valid power-of-two
      capacities, and allocator address/capacity arithmetic is checked while
      foreign or forged blocks are rejected. No new runtime recovery defect was
      identified in this pass; Phase F still requires real target/runtime
      integration evidence before acceptance.
- [x] Batch 702 records consumer evidence for the preceding contract audits:
      `nia-driver` (655 tests), `nia-linker` (31 tests), and the backend
      IR/lowering/LLVM suites (484 tests) pass, including cache corruption and
      replacement, cross-module const generics, malformed LLVM-boundary IR,
      zero-sized values, vtables, static archives, and std IO/process paths.
      No consumer bypass of the reviewed validation boundaries was observed.
- [x] Batch 703 closes Linux spawn descriptor-alias cleanup. When a child
      inherits closed standard descriptors, `pipe2` can place a pipe end at
      fd 0, 1, or 2; after `dup2`, per-pipe cleanup now preserves every
      configured standard target instead of closing an aliased target from a
      different pipe. A real `emit --exe` regression closes the host child's
      standard descriptors and verifies piped stdout remains readable.
- [x] Batch 704 audits startup argument/environment vectors and Linux
      directory iteration. Runtime-owned `Init` receives ABI-terminated
      vectors, while public accessors cap indices and tolerate null entries;
      directory parsing rejects short, oversized, unterminated, and
      overflowed records before exposing borrowed names. No additional
      externally reachable recovery gap was identified; malformed records
      remain deterministic `Invalid` iterator results.
- [x] Batch 705 closes parser custom-token origin panic recovery. The public
      syntax-tree parser now falls back to a span-based synthetic locator when
      caller-supplied token spans cannot form a complete child-path range or
      cross source revisions, preserving normal child-path identities while
      keeping malformed reconstruction input diagnostic and non-panicking.
      The parser owner suite (123 tests) and syntax suite (12 tests) pass.
- [x] Batch 706 records Phase A owner evidence after the frontend recovery
      fix. `nia-body-check` (279), `nia-trait-solve` (24),
      `nia-program-signatures` (7), `nia-type-lower` (19), and
      `nia-local-resolve` (17) tests pass with strict `-D warnings` Clippy.
      Production `expect`/`unreachable` sites in these owners remain guarded
      internal contracts or exhaustive enum branches; no user-input recovery
      gap was identified, so Phase A remains open pending its full acceptance
      matrix and cross-module evidence.
- [ ] Phase A: type, trait, and body soundness.
- [ ] Phase B: layout, ABI, backend IR, and LLVM safety.
- [ ] Phase C: const, static, closure, flow, and IR semantics.
- [x] Phase D: query, cache, loader, import, and build state.
- [ ] Phase E: frontend, identity, diagnostics, and structural boundaries.
- [ ] Phase F: standard library and runtime.
- [ ] Cross-cutting Rustdoc/comment completion and test-gap closure.
- [ ] Final clean/incremental, workspace, integration, resource, and external
      target evidence.

## 12. Roadmap Retirement

Delete this file only when:

1. Every phase has a committed implementation/test result or an explicitly
   recorded, accepted residual risk in the owning stable documentation.
2. Obsolete APIs, compatibility shims, duplicate identities, and temporary
   audit fixtures are removed.
3. Owner tests, affected consumer tests, workspace check, formatting, strict
   Clippy, relevant integration tests, and resource-controlled workloads pass.
4. Durable architectural rules and lessons have moved to `docs/architecture.md`,
   `docs/compiler-maintenance.md`, the relevant crate README, or `lib/README.md`.
5. A final audit confirms no phase is merely marked complete because it was
   recently touched by an unrelated commit.

The deletion itself must be a dedicated `docs:` commit after the final
acceptance batch; Git history retains this roadmap and its intermediate work.
