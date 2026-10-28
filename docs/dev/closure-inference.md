# Generic Callable Closure Inference

This note specifies the contract between generic-call inference, closure
body checking, and callable-state validation. It describes Nia's equality-based
inference and direct error conversion rules; it does not import Rust's closure
or error semantics.

## Call Constraint

Suppose a callee declares a generic callable parameter `F` and a predicate
`F: Fun(A) R`. Let `θ` be the single substitution environment for that call.
The parameter type, predicate, and callee result are instantiated through the
same `θ`; the callable bound is not a second generic environment.

For a closure argument `c = \x -> b`, inference records these constraints:

```text
parameter(c) = Aθ
type(b)       = Rθ
type(c)       satisfies Fθ
```

The first two constraints provide context and infer generic variables shared by
the signature. The last constraint is checked after substitution and verifies
the concrete closure state and callable mutability. A bound never substitutes
for the declared generic parameter when deciding whether the argument is
admissible.

## Explicit Callable View

An explicit callable-view parameter has a different representation and a
different admissibility check:

```nia
fun consume(callback: &Fun(A) R) R { ... }
```

Its expected callable context is the invocation shape `Fun(A) R`, while the
parameter itself is the runtime-erased `&Fun(A) R` view. For a closure argument
`c`, checking the argument follows this contract:

```text
Γ ⊢ c ⇐ &Fun(A) R
    body(c) is checked with parameter types A and expected result R
    closure state(c) is checked/coerced to the requested callable view
```

The context supplies the entire `R`, including its outer constructor. When
`R = E!V`, both payload constructors receive their proper component context:
`!value` checks `value` against `V`, and `error!` checks against `E`. Branches,
blocks, and conditionals preserve that same expected result. This is true for
an inline closure and for a local closure whose monomorphic signature is
inferred from this use. Writing `&` on the closure expression does not create a
second inference rule; it selects the source-level view conversion that is
checked after its closure signature is known.

The callable invocation shape and runtime view must not be unified as if they
were the same type. The first contextualizes the closure body; the second
checks the argument's admissibility, including capture state and mutability.
An implementation that uses the expected `&Fun` only after checking the
closure body is too late. An implementation that accepts the callable shape in
place of the view is too permissive.

The corresponding error-propagation rule is:

```text
Γ; return E_target!R ⊢ e.? : V
    if Γ ⊢ e : E_source!V
    and E_source = E_target or IntoError[E_source, E_target] is satisfied
```

For an optional operand the analogous rule requires an optional return
boundary and preserves its payload. In the error-union rule, `IntoError` is an
obligation at this particular propagation boundary, never an equality between
`E_source` and `E_target`. Consequently, contextualizing a callback's return
with `E_target!V` must not rewrite the operand's error type or cause the
conversion obligation to disappear.

These rules separate three questions that had been easy to conflate: which
signature checks the closure body, which concrete closure state satisfies the
callable parameter, and which conversion discharges `.?`. The local inference
pass may add only the justified structural equalities; ordinary body checking
and trait solving remain authoritative for coercions and `IntoError`.

For `R = Target!Value`, the body is checked in that error-union context. Thus
`!value` checks its payload against `Valueθ`, and `error!` checks against
`Targetθ`. A nominal error constructor in the closure body may determine
`Target`; another argument or the receiver may determine `Value`. The inferred
closure signature is then checked against the same instantiated bound.

## Propagation Constraint

For `e.?`, if the enclosing function result is `Optional[R]`, the operand is
constrained to `Optional[T]` and the expression yields `T`. If the enclosing
function result is `E_target!R`, the operand is constrained to `E_source!T`
and the expression yields `T`:

```text
e : E_source!T       IntoError[E_source, E_target] is required
e.? : T              when the enclosing result is E_target!R
```

The error sides are intentionally not unified. Nia checks one direct
`IntoError` conversion at the propagation boundary after ordinary type
inference. This keeps generic closure inference from changing propagation or
conversion semantics.

The return boundary selects the propagation family, not the payload type:
`E_target!R` does **not** imply `T = R`. For example, propagating an
`E_source!i64` local inside an `E_target!bool` function is valid when the
conversion exists and the function subsequently returns `!true`. The
expected type of the `.?` expression supplies `T`; the enclosing return
boundary supplies only optional/error-union shape and the conversion target.

The local solver represents this distinction as a deferred relation:

```text
Try(operand, payload, return_boundary)

return_boundary = ?R       => operand = ?payload
return_boundary = E_t!R    => operand = fresh(E_s)!payload
operand = ?T              => payload = T
operand = E_s!T           => payload = T
```

The last two rules can connect the payload even while the return boundary is
unknown. None equates `E_s` with `E_t`, infers an error conversion, or checks
whether a propagation boundary is admissible. Ordinary body checking owns
those checks. If the source error remains unconstrained, the solver leaves it
unknown: it cannot choose the enclosing target error just to make a call
instantiable.

## Context Publication

A solved closure signature alone is insufficient. Consider:

```nia
fun fail[Value]() SourceError!Value { SourceError::Failed! }

// The later use supplies `Fun(i32) SourceError!i64` to this local.
let fallback = \cause -> !(fail().?);
consume(fallback)
```

The graph connects the callable result's `i64` to the propagation payload,
then to `Value` in this particular call to `fail`. Ordinary body checking
must consume `SourceError!i64` as context for that call. Rechecking it without
the solved context would lose `Value`; using the enclosing error type instead
would incorrectly change the error conversion rule.

The function-local product therefore includes both partial closure signatures
and fully resolved call-result contexts. A call-result context is keyed by
its source node and is never installed as an expression's checked type or a
resolved call. It is used only when the ordinary checker has no immediate
expected type. The ordinary call checker still infers and validates generic
arguments, selects the callee, checks predicates and arguments, and records
its actual result and instance. Unresolved generic, const-generic, callable,
or recovery types are not published as canonical call contexts.

Functions containing propagation use this same inference pass even when they
contain no closures. Inferring `Value` through `fail().?` does not depend on
adding an unrelated closure to the function.

## Phase Contract

1. The function-local constraint pass preserves tuple, pointer, callable,
   optional, and error-union shapes. Each `.?` records its operand, value term,
   and enclosing return term. Its structural constraint is discharged after
   the full body has contributed call and closure contexts, so a local closure
   can receive return context from a later callable use. Propagation and
   operator obligations share one worklist: either can make the other ready.
2. Generic call inference creates one fresh inference term per callee generic
   and reuses those terms across parameters, callable bounds, and the result.
3. Publish partial closure signatures and concrete call-result contexts.
   Closure bodies receive the substituted callable bound as their expected
   signature. Generic parameter inference consumes the resulting partial
   closure signature without publishing unresolved terms as canonical types.
4. The ordinary body checker validates the closure against the instantiated
   generic argument, applies Nia's normal coercions, and checks `IntoError` at
   each propagation boundary.
5. Backend lowering receives only concrete closure state and ABI types. An
   unresolved closure owner or mismatched entry ABI is a compiler error, not a
   reason to guess a generic substitution. `Self` is part of a trait-default
   closure's owner instance even when that owner has no explicit type or const
   generic arguments.

   Source closure entries run the same body instantiation and builtin-trait
   lowering as source functions. Their resulting bodies enter dependency
   discovery before the reachability worklist is drained; a symbolic
   `IntoError` call must not be copied directly into executable backend IR.

The constraint pass only adds equalities justified by source signatures and
expression forms. It does not solve trait obligations or conversions. The
source error component of a propagated operand remains independent from the
enclosing target error component. Failed structural unification is
transactional so partial matches cannot leak substitutions.

A block that cannot fall through contributes its `return` expressions to the
enclosing return boundary, but contributes no value equality to a branch join.
The constraint pass reuses the ordinary checker's flow analysis for this
decision. In particular, `{ return value; }` and `{ loop {} }` cannot constrain
an unrelated `match` payload to `()`. A falling-through block without a tail
still contributes `()` normally.

The worklist terminates because every productive round discharges at least
one of the finitely many collected obligations; a round that discharges none
stops. Unknown obligations remain unknown rather than receiving a fallback
type. Only justified equalities are added, so later contexts refine an
existing monomorphic signature instead of creating a second instance of a
local closure. These are implementation invariants and typing rules, not a
claim of a machine-checked soundness proof for Nia's entire type system.

## Regression Coverage

The body-check tests cover generic functions, generic extension methods, trait
methods, and trait default methods. They verify inference of nominal closure
results without parameter annotations, preservation of `IntoError` as a
separate obligation, optional and error-union propagation in local closures
contextualized by later uses, generic call and method results beneath `.?`,
propagation without closures, and rejection of an unconstrained source error.
They also cover structural pattern bindings and calls in `match`, pattern
conditions and pattern-condition chains, propagation whose payload differs
from the enclosing success type, and interdependent operator/propagation
obligations. Diverging branches cannot impose unit on contextual call payloads.
Runtime-erased views still fail static `F: Fun(...)` bounds.
The executable `std_error` regression checks an actual source-to-target error
conversion through both generic and borrowed callbacks, plus a closure in a
trait default method. The streaming hash test also exercises the default
allocator reallocation path through full code generation and execution.
