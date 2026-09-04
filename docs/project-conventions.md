# Nia Project Conventions

This document records repository-level conventions for Nia's pre-1.0 releases.
It is about how the project is maintained, not about user-facing language
syntax.

## No Historical Compatibility Surface

Nia is pre-1.0 and still changing. Temporary syntax and behavior that existed
during development are not part of the language contract unless they are present
in the current language specification.

Do not keep special parser paths, diagnostics, examples, or tests whose main
purpose is to explain an old Nia spelling. If a removed spelling is written
today, it should be treated like any other invalid syntax.

Examples:

- Do not keep migration hints for removed generic-call spellings such as
  `callee::[T]`.
- Do not keep positive tests for syntax that is no longer in the language.
- If an old construct now represents an important boundary, test it as a normal
  rejection of the current language, not as a compatibility case.

## Test Intent

Tests should document the current language and compiler behavior.

Use positive tests for accepted syntax and semantics. Use negative tests for
current semantic boundaries and diagnostics that matter to users. Avoid keeping
tests only because they increase test count or preserve development history.

When reviewing tests, delete obsolete cases or rewrite them into explicit
current-language rejection tests.

## User-Representative Nia Source

Nia source in standard-library modules, examples, benchmarks, and integration
fixtures should resemble ordinary user code. Do not add numeric suffixes merely
to make compiler tests more explicit when the surrounding signature, field,
place, or expression already determines the type. Such annotations reduce the
repository's routine coverage of contextual inference and can hide regressions.
Expected optional and error-union return types also provide payload context, so
write `return ?0`, `!1`, or `2!` rather than suffixing the payload solely to
repeat the wrapper's declared type.

Keep explicit literal types where width is the contract: ABI and layout values,
serialization, hashes and bit operations, overflow boundaries, mixed-width
arithmetic, otherwise unconstrained literals, and tests specifically about
literal typing or casts.

When a binding already states the complete nominal type, prefer an omitted
constructor whose result is determined by that expected type:

```nia
let mut cleanup: CleanupAccumulator[Error] = .init();
```

This is especially useful for generic constructors with a long type name. The
type is written once as the binding contract, while `.init()` makes contextual
construction explicit. Keep `Type::init()` when the value is passed without an
expected type, when inference would be ambiguous, or when the nominal owner is
important at the call site.

Executable fixtures return numeric failures with `process::exit(code)!` and use
direct `.?` propagation when the source error has a reviewed
`IntoError[process::ExitCode]` implementation. Explicit `.exit()` remains for
code that needs the converted error union as a value rather than immediately
propagating it. A direct `as process::ExitCode` is for the implementation of
that conversion or a test explicitly exercising an enum cast, not ordinary
control flow.

Write aggregate type information once in ordinary code. Aggregate literals own
their nominal type prefix; omit a duplicate left-hand annotation unless the
binding intentionally checks a wider contract:

```nia
let point = Point { x: 10, y: 20 };
let values: [i32; 3] = [1, 2, 3];
```

Keep the nominal prefix when a nominal aggregate stands alone or is nested.
Arrays have no expression-level type prefix; use expected-type context or an
element suffix when a standalone literal needs an explicit constraint:

```nia
consume(Point { x: 10, y: 20 });
let values = [1i64, 2, 3];
```

Do not spell the same inferred array element type on both sides unless a test is
specifically about contextual array checking or inferred array lengths.

When a call or binding already expects a slice, take the array's address and let
the pointer-array-to-slice coercion apply:

```nia
consume(&values);
let writable: &mut [i32] = &mut values;
```

Use `&values[..]` for an intentionally explicit whole-slice value and range
syntax for an actual subrange. Maintained examples should exercise adjacent and
multiline strings, nominal aggregate literals, pointer-array coercion, and
`if ... is`; parser-only coverage does not establish a usable idiom.

When a value implements `Iterable`, write `for pattern in value` directly.
Call `.iter()` when an iterator must be named or passed to an adapter, and call
`.iterMut()` when mutation of borrowed elements is the intended contract. Do
not make ordinary collection scans spell out a provider method that `for`
already expresses.

Use an effect-only if-pattern when an optional or error-union branch performs
one action and the other case is intentionally empty. For example, duplicate
validation should use `if find(name) is ?_ { return invalid!; }` rather than a
two-arm `match` with an empty `null` arm. Keep `match` when multiple cases are
meaningful or when the expression produces a value; use `.?`, `mapError`, or
`orElse` for error-union propagation and transformation rather than manually
destructuring a single error path.

Use `std::option::{isPresent,isNull}` only when a boolean must be composed or
returned; use `if value is ?payload` when the branch consumes the payload. Use
optional/error-union `map` and `andThen` when a callback pipeline makes the
data flow shorter and clearer, not merely to replace an already direct branch.

## Pattern Control Flow and Fallible Cleanup

`if value is pattern` and a `match` arm with the same pattern have the same
value and ownership semantics. The `if` form is a one-branch test: the pattern
bindings exist only in the `then` branch, while the unmatched case continues to
the `else` branch (or the merge point when no `else` is present). Choose
`match` when several cases are meaningful or when the expression is easier to
read as an exhaustive decision. Do not choose `match` to obtain a different
ownership behavior; Nia has no hidden move or destructor associated with either
form.

Error-union patterns use `!payload` for the success arm and `error!` (or a
named payload such as `cause!`) for the error arm. Optional patterns use
`?payload` and `null`. Keep the condition direction and the state transition
separate when rewriting a `match` as an `if`:

```nia
// Equivalent control flow for a single error branch.
if operation() is cause! {
    return cause!;
}
useSuccessfulResult();
```

Before writing this explicit branch, check whether it only propagates the same
error. The postfix propagation operator is the canonical form for that case:

```nia
operation().?;
let value = operationReturningAValue().?;
```

`.?` performs the direct propagation or the reviewed one-step `IntoError`
conversion required by the enclosing return type. Use an explicit `if` or
`match` only when the error must be observed, logged, retained for retry,
conditionally recovered, or combined with another result. Use `mapError` for a
pure error transformation and `orElse` for recovery; do not introduce a helper
whose only job is to spell `if error { return error; }`.

For a fallible teardown, a failed release must leave its owner reachable so the
caller can retry it. Only a completely successful teardown may mark an attempt
inactive or clear the owner. This is an explicit state machine, not an implicit
ownership rule:

```text
active + cleanup failure  -> active   + cleanup error (retry is valid)
active + cleanup success  -> inactive + primary result
inactive                  -> internal error
```

The following form preserves that ordering while avoiding an empty success
binding:

```nia
pub fn finish(&mut self) Error!Build {
    if not self.active {
        return Error::Internal(Operation::Initialize)!;
    }
    if self.primary is ?primary {
        if self.build.deinit() is cleanup! {
            return cleanup!;
        }
        self.active = false;
        primary!
    } else {
        self.active = false;
        !self.build
    }
}
```

Putting `active = false` in the cleanup-error branch is incorrect: it makes a
failed release look final and prevents the required retry. The same invariant
applies to collections, allocators, file adapters, and any type that retains a
failed `Block`, path, descriptor, or nested owner. Use
`CleanupAccumulator[Failure]` when independent releases must all be attempted;
retain each failed owner in its type-specific slot and return the first cleanup
error after the pass completes.
