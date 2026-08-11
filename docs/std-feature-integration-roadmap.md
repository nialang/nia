# Standard Library Tuple And Closure Integration Roadmap

Status: active bounded standard-library design and conformance project

This roadmap audits the experimental standard library now that tuples and
closures are implemented in the language and compiler. It owns new std API
design, source migration, ownership/lifetime review, and conformance evidence;
it does not reopen the completed tuple/closure compiler roadmaps. Compiler work
needed to support a selected std contract is a dependency with its own focused
acceptance, not a reason to add progress to an old roadmap.

## 1. Evidence From The Current Std

- `std::iter` currently provides `Iterator`, `DoubleEndedIterator`, `Take`,
  `Rev`, `take`, `count`, and `rev`, but no closure-based `map`, `filter`,
  `inspect`, `fold`, or fallible adapter.
- `std::collections::hash_map` uses named `HashMapEntry`, `HashMapEntryRef`,
  `HashMapEntryMut`, and `HashMapGetOrInsertResult` structs for paired results.
  Their fields encode stable ownership and mutability contracts and are not
  automatically replaceable by tuples.
- `std::fmt::debug::print` accepts `&[&fmt::Format]`; formatting templates and
  writer methods are ordinary typed error paths rather than callable pipelines.
- `std::mem::Allocated` and `CallableAllocation` are explicit storage/owner
  products. A callable view is non-owning; deinitialization remains explicit.
- `.?` and `IntoError` remove former explicit `.exit().?` call-site adapters;
  `std::error::mapError` now owns explicit synchronous error-arm mapping, while
  `std::error::orElse` owns synchronous failure recovery or replacement without
  changing the success type. Success-side fallible combinators remain
  unreviewed.

These observations are design input, not a mandate to rewrite every named
struct or add callbacks to every loop. Each migration must demonstrate a real
user workflow, a simpler ownership/error contract, and no unwanted provider
closure.

### Current Delivery

The first std slice adds synchronous `Iterator::forEach`, which accepts a
borrowed `&Fn(Item) ()` and does not retain callable state. Its closure lifetime
therefore ends at the call boundary; lazy adapters and owned callable storage
remain separate design work. Error-union `mapError` is tracked in the separate
error-handling roadmap because its target/error semantics are a language/std
boundary rather than an iterator concern. Its conformance case uses a tuple
success payload, establishing tuple preservation through a real generic std
operation without replacing ownership-sensitive named result structs.
The error roadmap's next std slice adds `orElse` with the same borrowed callable
lifetime. Its callback returns one explicit `Target!Value` layer, so it can
recover or replace a failure while tuple success values remain unchanged.

## 2. Candidate API Families

### Iterator adapters

Evaluate concrete generic adapters whose callable parameter is inferred from a
closure expression and whose returned iterator owns only the source and the
callable value:

- `map` for `Fn(Item) Output`;
- `filter` for `Fn(Item) bool` or a clearly specified borrowed-item form;
- `fold`/`tryFold` as eager operations;
- `tryMap` only after its item-success and iterator-error ownership are reviewed;
  error-union `mapError` and `orElse` do not implicitly decide how a lazy
  iterator stores or combines those values.

The first implementation may use a borrowed callable view for a synchronous
operation, but a lazy adapter must define how its callable state lives across
`next` calls. It must not accept a stack-backed view that can outlive its
closure. Capturing mutable state and escaping callable owners are separate
proposals unless the existing explicit `CallableAllocation` contract suffices.

### Collection algorithms

Review closure-based `ArrayList`/slice operations such as `retain`, `find`,
`position`, `sortBy`, or `partition`. Prefer iterator adapters when they avoid
duplicate traversal ownership. Any in-place operation must state aliasing and
mutation behavior for `&T`, `&mut T`, and captured state.

### Tuple-shaped results

Use tuples where a result is genuinely positional, local, and does not carry
independent ownership or named semantic roles. Candidate sites include internal
iterator bounds and small split/index helpers. Keep named structs for public
entry handles, ownership-transfer results, OS records, and values whose fields
need discoverable names or may grow.

Tuple migrations must preserve field order, mutability, borrow provenance, ABI,
and pattern readability. They must not be performed only to demonstrate tuple
syntax.

### Formatting and callbacks

Evaluate whether formatting argument packs, writer visitors, and debug helpers
benefit from tuples or callable views. A callback API is acceptable only when
the callback has a concrete synchronous lifetime or an explicit owner. Do not
introduce a hidden executor, dynamic dispatch, allocator side store, or
type-erased callable to make an API look generic.

## 3. Design Rules

- Every candidate records its layer, public signature, ownership of input and
  callable state, error type, aliasing rules, and provider/source closure.
- Borrowed `Fn` views are valid only for the invocation or explicitly bounded
  lexical operation. A lazy iterator must own a sized callable value or use an
  explicitly owned `CallableAllocation`; `CallableAllocation::deinit` remains
  the sole release boundary.
- No API infers allocation, destruction, or lifetime extension from a closure
  expression. No callback is stored in a collection without a stated storage
  policy.
- Errors remain concrete and composable. `IntoError` is for infallible
  cause-preserving conversion; contextual operation/path errors stay explicit.
- Tuple values are not a substitute for a named public contract. Use a tuple
  only when positional meaning is stable and clear at every call site.
- Public facade imports must select the same semantic/body/backend provider
  work as equivalent narrow imports. New adapters must not widen build-host
  dependencies by accident.
- Maintained std source should use contextual type inference, direct `for`,
  direct `.?`, and lower-camel names; old spellings are removed, not aliased.

## 4. Delivery Waves

1. Build a workflow inventory and decision table for iterator transforms,
   collection algorithms, tuple candidates, formatting callbacks, and error
   combinators. Reject candidates whose only evidence is API symmetry.
2. Select one low-risk synchronous closure operation and one tuple-shaped
   internal result. Specify signatures, callable lifetime, ownership, error
   behavior, and provider-demand expectations before implementation.
3. Implement the selected generic types/traits in `lib/std`, add parser/body,
   runtime, and ABI tests only where compiler behavior is a dependency, and
   migrate representative examples and build-host call sites.
4. Expand to additional adapters and public results only when conformance
   demonstrates real ergonomic improvement. Keep named ownership-sensitive
   structs and explicit allocator APIs intact.
5. Move durable decisions to `standard-library.md`, update the language and
   architecture references for any compiler dependency, run the full acceptance
   matrix, and delete this roadmap.

## 5. Acceptance Matrix

- Each retained adapter has positive tests for no-capture and capturing
  callables, nested composition, single invocation/evaluation, mutation and
  aliasing boundaries, and source/closure cleanup.
- Lazy adapters cannot retain a borrowed closure view past its lexical scope;
  owned callable tests prove exactly one explicit deinitialization boundary.
- Error-producing adapters cover success, failure, conversion, partial output,
  and cleanup without collapsing concrete causes.
- Tuple candidates have tests for construction, projections/patterns, inferred
  types, borrow/mutability behavior, and a documented reason a named struct was
  or was not retained.
- Build-host audit proves equivalent facade forms and new std APIs do not
  activate unrelated providers or violate host/target separation.
- End-to-end examples demonstrate at least one iterator pipeline, one
  ownership-sensitive named result, one positional tuple result, and one direct
  error propagation path using the accepted idioms.
- `cargo fmt --all -- --check`, workspace check/clippy/tests, relevant CLI and
  std-build integration tests, Python tool tests, and `git diff --check` pass.
- Stable docs contain only the selected APIs and current ownership/error rules;
  rejected experiments and temporary adapters are physically absent.

## 6. Explicitly Separate Follow-On Work

Placement construction, true mutable-capture semantics, type-erased callable
owners, automatic destruction, asynchronous callback storage, and broad
collection algorithm suites each require a new design document. They are not
implicit acceptance items for this roadmap.
