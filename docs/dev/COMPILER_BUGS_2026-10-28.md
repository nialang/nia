# Compiler Bugs Discovered 2026-10-28

> Historical investigation notes, not current compiler status. The closure
> inference path was subsequently implemented and regression-tested; see
> `closure-inference.md`. The enum shorthand case remains expected-type
> dependent, as recorded below.

This document tracks critical compiler bugs discovered during Option type API testing.

## Status: Historical snapshot; see the note above for current status

---

## Bug #1: Enum variant shorthand fails in generic contexts

### Severity
**NOT A BUG** - The compiler is correctly rejecting ambiguous shorthand syntax.

### Description
When using the enum variant shorthand syntax (`.Invalid`) as an argument to a generic method, type inference fails with:
```
error: cannot infer the enum that Invalid belongs to from here
```

This was initially thought to be a bug, but investigation revealed that **multiple enums** have an `Invalid` variant:
- `mem::Error.Invalid` at `lib/std/mem/types.nia:4`
- `build::error::Error.Invalid` at `lib/std/build/error.nia:71`

The compiler cannot determine which enum is intended without additional type context. Since the parameter type is a generic `Error` parameter (not yet substituted), the compiler has no way to know which enum the user means.

### Root Cause Analysis
When checking method arguments before generic inference completes:

1. The parameter type is `GenericParam(Error)` — not yet substituted
2. The compiler calls `generic_argument_expected` (`check.rs:2536-2541`) which returns `None` for unresolved generics
3. The `.Invalid` shorthand is checked with `expected = None`
4. Without an expected type, the shorthand resolver (`expr.rs:157-189`) cannot determine which enum `Invalid` belongs to
5. The error is raised

**However**, this is correct behavior because `.Invalid` is genuinely ambiguous:
- Multiple enums in std have an `Invalid` variant
- The generic parameter `Error` is not constrained to any specific enum type
- Without substitution, there's no way to know which enum the user intended

### Reproduction
```nia
using std::mem;
using std::option::Option;

pub fun example() mem::Error!() {
    val maybe: ?u32 = null;
    // This correctly fails - ".Invalid" is ambiguous:
    val result = maybe.okOr(.Invalid)?;
    !()
}
```

### Correct Usage
The user should specify which enum they mean:
```nia
val result = maybe.okOr(mem::Error.Invalid)?;
```

Or use explicit type parameters:
```nia
val result = maybe.okOr[mem::Error](.Invalid)?;
```

### Recommendation
**Close as "working as intended"**. The shorthand syntax is designed for cases where the expected type is known. In genuinely ambiguous contexts, the full enum name should be used.

The compiler could potentially improve the error message to suggest using the full enum name or explicit type parameters.

---

## Bug #2: Closure return type doesn't infer generic parameters

### Severity
**HIGH** - Makes `okOrElse` and similar closure-accepting generic methods unusable.

### Description
When passing a closure to a generic method where the closure's return type should infer the generic parameter, the compiler fails with:
```
error: cannot infer type arguments from here
```

### Root Cause
The closure inference in `nia-body-check/src/calls/function_calls.rs` (`infer_generics_from_closure_signature` at line 1258) uses `self.inferred_closure_signature(expr)` which comes from a pre-pass. However, this pre-pass may not have the closure's return type fully resolved yet, especially when the closure body has explicit type annotations like `\-> mem::Error { ... }`.

### Reproduction
```nia
using std::mem;
using std::option::Option;

pub fun example() mem::Error!() {
    val maybe: ?u32 = null;
    // This FAILS:
    val result = maybe.okOrElse(\-> mem::Error {
        mem::Error.Invalid
    })?;
    !()
}
```

Where `okOrElse` is defined as:
```nia
pub fun okOrElse[Error](self, supplier: &Fun() Error) Error!Value {
    // ...
}
```

### Expected Behavior
The compiler should:
1. Check the closure `\-> mem::Error { mem::Error.Invalid }`
2. Infer its type as `Fun() mem::Error`
3. Match `&Fun() mem::Error` against pattern `&Fun() Error`
4. Substitute `Error = mem::Error`
5. Check the method call with `Error = mem::Error`

### Current Behavior
The compiler appears to not extract the explicit return type annotation from the closure during generic inference.

### Workaround
Use explicit type parameter:
```nia
val result = maybe.okOrElse[mem::Error](\-> mem::Error {
    mem::Error.Invalid
})?;
```

### Impact
- Makes `okOrElse` essentially unusable without explicit type parameters
- Breaks expected generic inference patterns
- Inconsistent with other closure inference (where parameter types are inferred)

### Fix Strategy
Options:
1. **Extract explicit annotations**: When a closure has `\-> Type`, use that type immediately for generic inference
2. **Multi-pass inference**: Re-check closure return types after body checking
3. **Eager annotation resolution**: Resolve explicit type annotations before the closure pre-pass

---

## Testing Status

### Test File Created
`crates/nia-cli/tests/option_api_tests.rs` - Comprehensive test suite for Option API

### Passing Tests
- ✅ `option_some_unwrap` - Basic `.?` unwrap
- ✅ `option_basic` - Constructor patterns
- ✅ `option_pattern_matching` - `is ?value` patterns
- ✅ `option_map_and_or` - `map`, `and`, `or` methods
- ✅ `option_propagation` - `.?` propagation
- ✅ `option_explicit_error` - Fully-qualified error variants (workaround)

### Blocked Tests
- ❌ `option_error_shorthand` - Blocked by Bug #1
- ❌ `option_closure_inference` - Blocked by Bug #2

---

## Next Steps

1. **Investigate normalization**: Understand why `normalize()` doesn't substitute generics
2. **Check substitution maps**: Find where generic substitutions are stored during method resolution
3. **Design fix**: Decide between deferred checking vs. better normalization
4. **Implement fix**: Apply to both bugs (they may share root cause)
5. **Validate**: Ensure fix doesn't break existing inference
6. **Unblock tests**: Re-enable blocked test cases

---

## References

- Option API: `lib/std/option.nia`
- Test suite: `crates/nia-cli/tests/option_api_tests.rs`
- Enum variant checking: `crates/nia-body-check/src/aggregates.rs:1278`
- Generic inference: `crates/nia-body-check/src/calls/function_calls.rs:1258`
- Closure inference: `crates/nia-body-check/src/inference.rs:99`
