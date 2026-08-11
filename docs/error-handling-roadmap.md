# Error Handling And Propagation Roadmap

Status: active bounded language and standard-library design project

This roadmap owns the next review of Nia error handling. It starts from the
implemented error-union and `IntoError` contracts and decides which ergonomic
and const-capable operations are worth adding. It does not reopen the completed
tuple, closure, or build/standard-library roadmaps. When this project closes,
durable decisions move to [language-spec.md](language-spec.md),
[architecture.md](architecture.md), and [standard-library.md](standard-library.md),
then this file is deleted.

## 1. Current Contract

- Error unions are written `E!T`; `!value` and `error!` require an expected
  union type.
- `.?` unwraps an optional or error union. Optional propagation returns `null`
  and never invokes error conversion.
- Error propagation requires an error-union return type. Equal error types are
  propagated directly. Different error types select exactly one ordinary trait
  implementation `Source: std::error::IntoError[Target]`.
- The source expression is evaluated once and conversion runs only on the
  failure edge. Trait resolution uses ordinary where-predicates and ambiguity
  rules; conversion chains and implicit allocation/type erasure are forbidden.
- Runtime body checking records the conversion as a resolved trait call;
  typed BIR, Function IR, executable reachability, defer ordering, and LLVM
  lowering preserve that failure-edge call.
- Const checking accepts automatic `IntoError` conversion only when the selected
  witness is a `const fn`. The evaluator invokes it on the failure edge and
  preserves exact-type and optional propagation without a witness. Runtime-only
  witnesses receive a dedicated const diagnostic.
- `std::error` currently exposes `IntoError[Target]` and the explicit
  `Source!T::cast_error()` helper. Contextual mappings that add operation/path
  information remain explicit adapters.

The owning implementation boundaries are `check_try_expr` and
`resolve_into_error_conversion` in `crates/nia-body-check/src/expr.rs`, the
`TypedTryErrorConversion` product in `crates/nia-body-check/src/bir.rs`, and the
try lowering/codegen paths under `crates/nia-function-lower` and
`crates/nia-codegen-llvm`. Const behavior is owned by `crates/nia-const-check`.

### Current Delivery

The first implementation slice adds `Source!T::mapError` as an explicit,
synchronous borrowed-callable operation. Generic inference now derives a
callable signature from a direct `&closure` pointer, so an inline closure's
explicit return type supplies the target error without a result annotation.
Runtime conformance proves success bypasses the mapper and failure invokes it
once; the preserved success payload is a tuple, covering the complete generic
error-union/callable/tuple lowering path. The second slice adds const
`IntoError` propagation, upgrades the standard protocol and existing standard
witnesses to `const fn`, and proves both tuple-success preservation and
failure-only conversion through emitted code. Fallible mapping and richer
recovery operations remain undecided and are not silently included.

## 2. Decisions And Open Questions

Each selected API has a written decision and a representative source example.
Open questions block only the behavior they affect.

1. Resolved: automatic `IntoError` conversion is available in const code when
   the selected witness is a `const fn`. The proof is ordinary unique trait
   resolution plus the selected function signature's const capability. Runtime
   and const evaluation both perform one failure-edge call; no runtime-only
   fallback exists.
2. The canonical infallible error-mapping API is the error-union method
   `mapError`. It maps only the error arm, preserves the success value,
   evaluates the source once, and borrows its callable for the synchronous call.
   A parallel free function or trait operation is not retained.
3. `mapError` accepts a borrowed readonly `Fn` view and cannot retain it. It
   does not take a no-capture-only function pointer or an owned
   `CallableAllocation`; those shapes would add restrictions or ownership that
   a synchronous mapping does not need. Capturing mutability remains outside
   this project.
4. How are fallible mappings represented? A `mapError` callback returning
   `Target` is infallible. A callback returning `Target!Target2` would require
   a specified flattening operation and must not be smuggled in as an implicit
   conversion.
5. Which standard conversions are safe to provide? `IntoError` may preserve or
   narrow an existing cause, but it must not invent operation/path context,
   allocate, or erase the source. Contextual errors continue through named
   constructors or explicit adapters.
6. What diagnostics distinguish missing, malformed, ambiguous, non-const, and
   lifetime-invalid conversion callables? Diagnostics must identify the source
   and target error types and point at the propagation or mapping expression.

## 3. Non-Goals

- No multi-hop conversion search, implicit boxing, dynamic error object, or
  type-erased callable owner.
- No placement-construction or automatic destruction semantics for closures.
- No mutable-capture semantics, closure lifetime inference, or new borrow rules.
- No replacement of explicit contextual errors (`OperationError`, path/action
  causes, spawn-stage causes) with a broad universal error.
- No reopening of tuple construction/projection or closure ABI/LLVM work.

## 4. Implementation Waves

1. Add design fixtures and a decision matrix for runtime/const propagation,
   exact conversion, `IntoError`, and proposed mapping APIs. Record current
   diagnostics and ensure the matrix is independent of std implementation
   details.
2. Const conversion is accepted and implemented through const semantic call
   checking, an invocation-local failure-edge witness, and resolved evaluation.
   Validate const eligibility, generic substitutions, deterministic diagnostics,
   and cache fingerprints as additional witnesses are added.
3. Implement the selected mapping operation at one explicit semantic boundary.
   Lower success and failure paths separately, preserve single evaluation and
   defer order, and carry callable view/owner provenance through BIR, Function
   IR, executable facts, ABI validation, and LLVM.
4. Add reviewed std conversions and migrate only call sites whose context is
   preserved. Remove duplicate helpers when the new operation is proven to own
   the same contract; keep explicit contextual adapters.
5. Update stable docs, end-to-end examples, and conformance tests. Delete this
   roadmap only after every acceptance item is closed.

## 5. Diagnostics And Safety

Diagnostics must distinguish:

- propagation from a non-optional/non-error-union operand;
- propagation from an error union into a non-error-union return;
- missing, malformed, chained, or ambiguous `IntoError` implementations;
- const conversion rejected because the trait call or callable is not const;
- a mapping callback whose signature, mutability, or lifetime is incompatible;
- a contextual conversion that must be explicit because it adds information.

All ordinary failures remain typed diagnostics. Panic is reserved for internal
invariants at the existing ICE boundary. The runtime and const paths must not
silently diverge in evaluation count, error-arm selection, or cleanup order.

## 6. Acceptance Matrix

- Parser/type/body tests cover every selected spelling, expected-type inference,
  exact propagation, one-step conversion, missing/ambiguous conversion, and
  callback signature errors.
- Runtime tests prove success skips conversion, failure converts exactly once,
  the source is evaluated once, and defers run in the specified order.
- Const tests prove accepted const mappings, reject runtime-only calls, and
  preserve exact-type propagation. Incremental tests cover the conversion
  witness and mapping callable in fingerprints and invalidation.
- LLVM/backend validation tests reject malformed failure-edge products and prove
  the converted error reaches the caller in the required ABI representation.
- std conformance tests preserve concrete causes, allocation behavior, cleanup,
  and contextual operation/path information.
- `language-spec.md`, `architecture.md`, `standard-library.md`, examples, and
  maintained source agree with the chosen contract; rejected alternatives are
  absent from production code and current documentation.
- Focused tests, `cargo fmt --all -- --check`, workspace check/clippy/tests,
  Python tool tests, and `git diff --check` pass before retirement.

## 7. Retirement

Retire this roadmap only after the semantic decision record, implementation,
tests, and stable documentation are complete. Follow-on work such as a dynamic
error object, richer recovery combinators, or mutable callable captures gets a
new bounded proposal.
