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

Use this section as the only temporary progress ledger. Update it in the same
commit as the corresponding implementation batch.

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
- [x] Loader frontend-cache publication now enforces its read-side size bound
      and only treats `AlreadyExists` as a benign concurrent-writer outcome;
      permission, I/O, and other rename failures remain visible to callers.
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
- [ ] Phase A: type, trait, and body soundness.
- [ ] Phase B: layout, ABI, backend IR, and LLVM safety.
- [ ] Phase C: const, static, closure, flow, and IR semantics.
- [ ] Phase D: query, cache, loader, import, and build state.
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
