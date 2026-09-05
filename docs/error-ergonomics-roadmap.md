# Error Ergonomics Roadmap

Status: completed

This roadmap records two related but independent error-handling ergonomics
proposals for Nia:

1. an `inspectError` result combinator for observing and re-propagating an
   unchanged error;
2. `and`-chained pattern conditions such as
   `if first() is ?x and second(x) is ?y { ... }`.

The std review established the repeated pattern-condition use cases and the
implementation now proceeds with the chain proposal. `inspectError` remains a
separate follow-up until call-site evidence justifies its API.

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
- An omitted enum member does not provide enough expected type information for
  an associated-function call nested inside it (for example,
  `.Setup(.fromOs(cause))`). Keep the receiver qualified in that shape until
  contextual typing can propagate through both products.
- An omitted struct constructor followed immediately by an associated method
  call (for example, `.init(name, root).forHost()`) currently loses its
  nominal expected type. Keep the constructor qualified in that chained shape;
  ordinary omitted construction remains preferred when the surrounding
  expression already supplies the type.
- `is` pattern conditions currently parse only as standalone `if` conditions;
  combining one with existing `and`, `or`, or `not` boolean expressions is
  rejected by the parser. Keep compound enum predicates in their explicit
  comparison form until Proposal B has an explicit AST and precedence rule.
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
if first.find(name) is ?old and second.find(name) is ?new and old.isValid() {
    use(old, new);
}
```

This is Nia's equivalent of Rust's `if let` chains, expressed with the
language's existing `is` pattern condition rather than introducing a new
expression-level `let`. The first version supports a left-to-right chain
beginning with a pattern clause; later clauses may be patterns or ordinary
boolean predicates:

```text
if expression is pattern (and (expression is pattern | boolean-expression))* block [else expression]
```

The `and` separators belong to this condition grammar. Ordinary predicates,
including `and not predicate`, are valid after a pattern. Binding-producing
`or`, `not value is pattern`, and parenthesized subchains are rejected;
ordinary boolean conditions without a pattern remain ordinary `if` expressions.
This keeps binding-producing alternatives explicit through `match`.

The intended accepted form is therefore:

```nia
if acquire() is ?resource and resource.isReady() and not resource.hidden {
    use(resource);
}
```

These are outside the first grammar, with diagnostics rather than a fallback
to ordinary boolean parsing:

```text
if acquire() is ?resource or other() is ?fallback { ... }
if (acquire() is ?resource and resource.isReady()) { ... }
if acquire() is ?resource and not resource.isReady() is true { ... }
```

### 3.1 Semantic Contract

The chain is a short-circuit sequence, not a boolean expression that happens to
contain patterns. For clauses `C1 ... Cn`, evaluation is equivalent to this
control-flow shape, where each `Ci` is entered only after all preceding clauses
have matched:

```text
evaluate target 1 once
if target 1 matches pattern 1 {
    bind pattern 1
    evaluate target 2 once
    if target 2 matches pattern 2 {
        bind pattern 2
        ... body ...
    } else {
        ... else ...
    }
} else {
    ... else ...
}
```

This model defines the observable behavior:

- targets run strictly left-to-right and at most once each;
- a failed clause stops evaluation of all later targets;
- a successful binding is visible in later targets and in the body;
- no successful-clause binding is visible in `else`;
- a target temporary and its bindings leave through the same ordinary Nia
  cleanup paths as an equivalent nested block;
- the compiler adds no move, clone, destructor, lifetime extension, allocation,
  or error conversion beyond the existing single-clause `is` semantics.

For an effect-only `if`, an omitted `else` keeps the existing behavior and
produces `()`. For a value-producing `if`, an `else` is required unless the
whole conjunction is proven exhaustive. An exhaustive first pattern does not
make a chain exhaustive if a later clause can fail.

Pattern checking is performed independently for each target type. Rust rejects
irrefutable patterns in let-chain conditions, but Nia currently permits an
irrefutable pattern in a standalone `if ... is`; whether the chain should keep
that useful binding form or require at least one refutable pattern is an
explicit design decision, not an accidental parser consequence. Whichever
policy is chosen must be diagnosed per clause rather than collapsed into one
synthetic boolean diagnostic.

### 3.2 Ownership And Cleanup

The lowering must materialize each reached target exactly once before testing
its pattern, as the current `IfPattern` lowering does. It must not eagerly
evaluate all targets into a tuple or hidden aggregate: that would change
side-effect order, temporary lifetime, and failure cleanup. A later target may
read an earlier binding, so the chain's nested scopes must preserve those
bindings until the body or the selected `else` path has completed.

The implementation may use nested control-flow blocks internally, but this is
an implementation strategy only. It must preserve the source-level clause
spans and must not expose the synthetic nested scopes to name resolution or
diagnostic wording.

When a later clause fails, earlier target values are cleaned up while leaving
the `else` branch with no access to their bindings. A `defer`, fallible cleanup,
retained owner, or error-union value in a target follows the same rules as it
would in explicitly nested `if ... is` statements. This is the critical Nia
soundness boundary: a chain is only a syntax-level composition of already
specified operations, never an ownership shortcut.

### 3.3 Rustc-Derived Constraints

The Rust implementation and tests provide useful invariants, but not a source
grammar for Nia. In particular, rustc's condition handling:

- parses a left-associated `&&` tree and visits each RHS condition in order;
- checks each `let` pattern for refutability and records its source scope;
- keeps bindings available to later RHS conditions and the then branch only;
- rejects `||` in a let-chain because alternatives do not share one binding
  environment;
- rejects parenthesized let subchains in the chain context to avoid precedence
  and scope ambiguities;
- validates drop order with MIR tests where a later condition succeeds or
  fails.

The relevant local reference points are rustc's `CondChecker` and
`LetChainsPolicy` in `compiler/rustc_parse/src/parser/expr.rs`, the
`visit_land`/`visit_land_rhs` chain walk in
`compiler/rustc_mir_build/src/thir/pattern/check_match.rs`, and the UI/MIR
tests under `tests/ui/rfcs/rfc-2497-if-let-chains/`,
`tests/ui/binding/irrefutable-in-let-chains.rs`, and
`tests/ui/mir/mir_let_chains_drop_order.rs`.

Nia intentionally differs in three ways: it uses `and` and `is`, it has no
edition gate, and ordinary predicates are represented explicitly alongside
pattern clauses. These differences preserve Nia's explicit manual-memory model
while retaining Rust's sequencing, scope, refutability, and cleanup invariants.

### 3.4 Compiler Representation

The parser should produce an explicit `IfPatternChain` node containing ordered
`{ target, pattern }` clauses. It must not encode the chain as a boolean
`and`, because boolean lowering loses pattern bindings and can evaluate a
target in the wrong scope. Body checking should type-check each target after
the preceding clause bindings have been installed, then type-check the shared
then/else result as an ordinary if expression.

Typed BIR should retain the ordered clauses and their typed patterns. Runtime
lowering can implement the contract as nested existing pattern-condition
blocks, but the source chain identity and per-clause spans must remain available
for diagnostics, const evaluation, closure provenance, and executable-facts
collection. Const evaluation should execute the same ordered clauses directly
or through an equivalent nested representation; it must not evaluate an
unreached target while probing later patterns.

The implementation must update every owner of `IfPattern`: AST walking,
conditional-compilation pruning, const lowering/evaluation, body checking,
typed BIR validation, function lowering, closure/escape analysis, and backend
input validation. No std-only special case is acceptable.

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
- no parenthesized subchains or implicit conversion into a boolean value;
- no implicit conversion of the chain into a tuple, optional, or error wrapper;
- no change to ordinary boolean `and` precedence or short-circuit semantics;
- mixing arbitrary boolean predicates into the chain is a later design question.

The `or` restriction is deliberate: different alternatives do not naturally
produce one common set of bindings. The parenthesis restriction is likewise a
first-version grammar boundary, not a claim that grouping could never be made
sound. A later proposal may define explicit binding rules if real code
requires either extension.

## 4. Design Questions

1. Does the std audit contain enough pure error-observation branches to justify
   `inspectError`, or are existing `mapError`, `orElse`, and explicit branches
   clearer in practice?
2. Should `inspectError` be a `std::result` extension, an `std::error` extension,
   or a facade re-export from one canonical owner?
3. After the first version is proven, is there a real need for ordinary boolean
   predicates that can read earlier bindings, or is an all-pattern chain clearer?
4. How should chain diagnostics identify the failing clause and the scope of
   each binding, including the chosen policy for irrefutable patterns?
5. Which explicit chain products are needed by const evaluation, callable
   restrictions, defer edges, closure provenance, and executable-facts
   collection?
6. What runtime and LLVM tests prove one evaluation per target and unchanged
   cleanup/defer ordering when a later clause fails?

## 5. Implementation Waves

1. Complete the std native-style review. Inventory candidate observation
   branches and multi-pattern nesting, and record rejected cases with their
   ownership/error reason.
2. Write parser, body-check, lowering, const, closure, executable-facts, and
   codegen decision records for the selected syntax. Add minimal fixtures before
   changing public APIs. The semantic and representation contract in Proposal B
   is the baseline those records must preserve.
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
- Parser tests reject `or`, `not`, parenthesized subchains, missing `is`, missing
  patterns, and a second clause whose separator is not `and`.
- Runtime tests prove single evaluation, left-to-right short-circuiting, exact
  error preservation/conversion, unchanged defer order, and cleanup of target
  temporaries when a later clause fails.
- Scope tests prove earlier bindings are usable by later targets and the body,
  but are unavailable in `else`; duplicate binding and irrefutable-pattern
  diagnostics identify the relevant clause.
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
