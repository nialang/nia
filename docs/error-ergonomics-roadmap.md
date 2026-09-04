# Error Ergonomics Roadmap

Status: queued design and language/library project

This roadmap records two related but independent error-handling ergonomics
proposals for Nia:

1. an `inspectError` result combinator for observing and re-propagating an
   unchanged error;
2. `and`-chained pattern conditions such as
   `if first() is ?x and second(x) is ?y { ... }`.

The current priority is a complete native-style review of `lib/std`, including
error handling, omitted constructors, pattern use, naming, ownership edges,
and real user workflows. This roadmap must not start implementation while that
review is still discovering existing std contracts. The review should first
record actual repeated call sites and rejected simplifications; that evidence
decides whether either proposal earns implementation work.

The roadmap does not change the existing contracts for `.?`, `mapError`,
`orElse`, `CleanupAccumulator`, or ordinary `if`/`match`. When this project
closes, durable decisions move to `docs/language-spec.md`, `docs/architecture.md`,
`docs/project-conventions.md`, and the relevant `lib/README.md` or std facade;
this file is then deleted.

## Review Findings So Far

- Omitted `.init(...)` is stable in ordinary runtime return positions and in
  nested runtime error constructors when the enclosing result type is known.
- Nested omitted enum constructors are not yet accepted by all `const fn`
  paths. Until const-context propagation is fixed and tested, those paths keep
  explicit constructors; this is a compiler boundary, not a new language rule.
- Generic error-union returns still keep explicit error constructors where
  provider selection or the enclosing error type is not locally unambiguous.
- A trait method whose return type is an associated `Error` type cannot use an
  omitted enum member, even when the concrete implementation sets that type;
  concrete methods can use the omission, while trait-facing methods retain the
  qualified constructor.
- `match` remains intentional for rollback, retained-owner cleanup, error
  accumulation, and multi-value decoding. Pure extraction branches are the
  only candidates for migration to `if ... is` during the std audit.

## 1. Existing Contracts

- `E!T` is Nia's native error-union value. `.?` propagates its error and
  performs at most one reviewed `IntoError` conversion selected by the enclosing
  return type.
- `std::result::map` and `andThen` transform or continue the success arm;
  `mapError` transforms only the error arm; `orElse` performs fallible recovery.
- `if value is pattern` and an equivalent `match` arm have the same value,
  evaluation, and ownership semantics. Pattern bindings are scoped to the
  successful branch.
- Fallible cleanup is explicit. A failed release retains its owner for retry;
  `CleanupAccumulator` attempts independent operations without short-circuiting
  and reports the first failure after the pass.
- Callable views are borrowed, synchronous, and non-owning unless an explicit
  owner such as `mem::CallableAllocation` is used. No proposal may infer a heap,
  lifetime extension, or destructor from a callback.

## 2. Proposal A: `inspectError`

### Intended shape

```nia
operation()
    .inspectError(&cause -> log(cause))
    .?;
```

The operation is a candidate result extension, not a current API. Its intended
contract is:

- evaluate the receiver exactly once;
- invoke a borrowed synchronous `&Fn(Source) ()` only on the error arm;
- return the original `Source!Value` unchanged;
- allocate nothing and preserve the concrete error type;
- never own or retain the error or callback.

The callback is intentionally infallible. A fallible callback would require an
explicit error-priority policy and belongs to cleanup/recovery APIs rather than
an observation combinator.

### Evidence required

The std review must identify repeated branches that only observe, log, count, or
trace an error before returning that same error. Branches that add context,
retain owners, perform rollback, accumulate cleanup failures, or choose a
recovery path do not count as evidence for this API.

### Non-goals

- no replacement for direct `.?` propagation;
- no fallible `inspectError` callback or implicit flattening;
- no error boxing, type erasure, allocation, or dynamic dispatch;
- no associated-type `Try` protocol as a prerequisite;
- no use in teardown, rollback, or owner-retention paths.

## 3. Proposal B: Pattern Condition Chains

### Intended shape

```nia
if first.find(name) is ?old and second.find(name) is ?new {
    use(old, new);
}
```

The first version should support a left-to-right chain of pattern clauses:

```text
if expression is pattern (and expression is pattern)* { ... }
```

Each target is evaluated once. A failed clause short-circuits to `else` or the
normal merge point. Bindings from successful earlier clauses are visible in
later targets and in the final body; bindings are not visible in `else`.
There is no hidden move, destruction, allocation, or lifetime extension.

The feature should work in both effect-only and value-producing if expressions:

```nia
let value = if readA() is ?a and readB(a) is ?b {
    combine(a, b)
} else {
    fallback()
};
```

The parser and typed IR should represent the chain explicitly rather than
pretending that `is` is an ordinary boolean operator. Lowering can then reuse
the existing target materialization, `pattern_condition`, and
`lower_pattern_binding` machinery for each clause.

### Initial restrictions

- only `and` chains of `expression is pattern` clauses;
- no binding-producing `or` chains;
- no implicit conversion of the chain into a tuple, optional, or error wrapper;
- no change to ordinary boolean `and` precedence or short-circuit semantics;
- mixing arbitrary boolean predicates into the chain is a later design question.

The `or` restriction is deliberate: different alternatives do not naturally
produce one common set of bindings. A later proposal may define explicit
binding rules if real code requires it.

## 4. Design Questions

1. Does the std audit contain enough pure error-observation branches to justify
   `inspectError`, or are existing `mapError`, `orElse`, and explicit branches
   clearer in practice?
2. Should `inspectError` be a `std::result` extension, an `std::error` extension,
   or a facade re-export from one canonical owner?
3. Should pattern chains initially allow only `is` clauses, or also ordinary
   boolean predicates that can read earlier bindings?
4. How should chain diagnostics identify the failing clause and the scope of
   each binding?
5. Do const evaluation, callable restrictions, defer edges, closure provenance,
   and executable-facts collection need explicit chain products, or can the
   chain be lowered before those boundaries without losing source identity?
6. What runtime and LLVM tests prove one evaluation per target and unchanged
   cleanup/defer ordering when a later clause fails?

## 5. Implementation Waves

1. Complete the std native-style review. Inventory candidate observation
   branches and multi-pattern nesting, and record rejected cases with their
   ownership/error reason.
2. Write parser, body-check, lowering, const, closure, executable-facts, and
   codegen decision records for the selected syntax. Add minimal fixtures before
   changing public APIs.
3. If justified, implement `inspectError` as one result extension with focused
   callable and single-evaluation tests. Keep it unavailable in const code
   unless an explicit const-call contract is separately accepted.
4. Implement pattern chains as an explicit AST/BIR product, reusing existing
   pattern condition and binding lowering. Add diagnostics and scope tests for
   optional, error-union, nested, and value-producing chains.
5. Add runtime/LLVM tests for short-circuit order, side effects, defer order,
   error propagation, and closure/ownership provenance. Migrate only reviewed
   std and example call sites where readability improves.
6. Move stable semantics to the language and std documentation, remove rejected
   experimental paths, run the full acceptance matrix, and retire this roadmap.

## 6. Acceptance Matrix

- Evidence documents show the selected API or syntax solves repeated real code,
  not merely an aesthetic symmetry.
- Parser and body-check tests cover valid forms, malformed chains, binding scope,
  expected types, and diagnostics.
- Runtime tests prove single evaluation, left-to-right short-circuiting, exact
  error preservation/conversion, and unchanged defer order.
- Const tests explicitly accept or reject each form; no runtime-only callable is
  accidentally admitted into const evaluation.
- Closure and executable-facts tests preserve callable provenance and all
  reachable functions without special-casing the std.
- LLVM/backend validation tests reject malformed products and verify the ABI for
  success and failure paths.
- std tests cover cleanup, rollback, retained owners, allocation behavior, and
  contextual errors; no candidate migration weakens those contracts.
- `cargo fmt --all -- --check`, workspace check/clippy/tests, focused CLI and
  std integration tests, relevant full-driver tests, and `git diff --check`
  pass before retirement.
