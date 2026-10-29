# Compiler Bug: Type Inference in Generic Contexts

> Initial hypotheses retained for context. The critical-bug classification was
> not confirmed: enum shorthand needs an unambiguous expected type, and the
> closure inference issue is now covered by `closure-inference.md` and compiler
> regression tests.

## Status
**Historical hypothesis** - This initial severity assessment was not confirmed

## Bug #1: Enum Variant Shorthand in Generic Contexts

### Description
Enum variant shorthand (`.Variant`) fails to resolve when used as an argument to a generic method where the enum type is a type parameter.

### Reproduction

```nia
using std::mem;

fun testShorthand() mem::Error!i32 {
    let some: ?i32 = ?42;
    // ERROR: "omitted member requires a call or enum expected type"
    let result = some.okOr(.Invalid);
    result
}
```

### Error Message
```
error[E0301]: omitted member requires a call or enum expected type
  --> test-okOr-bug.nia:6:28
  |
6 |     let result = some.okOr(.Invalid);
  |                            ^^^^^^^^
```

### Workaround
Use fully-qualified enum variant:

```nia
fun testExplicit() mem::Error!i32 {
    let some: ?i32 = ?42;
    // WORKS: Explicitly specify the error type
    let result = some.okOr(mem::Error::Invalid);
    result
}
```

## Bug #2: Cannot Infer Generic Parameter from Closure Return

### Description
When calling `okOrElse`, the compiler cannot infer the `Error` type parameter from the closure's return type, even though that return type is explicitly stated.

### Reproduction

```nia
using std::mem;

fun testOkOrElse() mem::Error!i32 {
    let none: ?i32 = null;
    // ERROR: "cannot infer generic parameter `Error`"
    let result = none.okOrElse(\-> mem::Error::Leak);
    result
}
```

### Error Message
```
error[E0301]: cannot infer generic parameter `Error`
  --> main.nia:4:18
  |
4 |     let result = none.okOrElse(\-> mem::Error::Leak);
  |                  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
note: `Error` is not determined by the call arguments, expected result, or other generic constraints
help: provide an explicit type argument for `Error` (for example `function[Type](...)`), or add a type annotation that determines it
```

### Workaround
Explicitly provide the type parameter:

```nia
fun testOkOrElseExplicit() mem::Error!i32 {
    let none: ?i32 = null;
    // WORKS: Explicitly specify the Error parameter
    let result = none.okOrElse[mem::Error](\-> mem::Error::Leak);
    result
}
```

### Root Cause
The compiler doesn't analyze the closure body to determine its return type **before** attempting to infer the method's generic parameters. The inference order is:

1. Attempt to infer `Error` from call site context
2. Check closure against inferred parameter type

But it should be:

1. Analyze closure's explicit return type (`-> mem::Error::Leak`)
2. Use that to infer `Error = mem::Error`
3. Check full call validity

This is similar to Rust's closure inference issues, but worse because Nia's closures **explicitly state** their return types.

## Context

The method signatures:
```nia
extend[Value] ?Value {
    pub fun okOr[Error](self, error: Error) Error!Value {
        if self is ?value { !value } else { error! }
    }

    pub fun okOrElse[Error](self, fallback: \-> Error) Error!Value {
        if self is ?value { !value } else { fallback()! }
    }
}
```

## Root Cause (Hypothesis)

Both bugs stem from the same issue in type inference/expected type propagation:

1. **Bug #1**: When checking the argument `error: Error`, the compiler has `Error` as a type variable but doesn't substitute it with the inferred concrete type before checking enum variant shorthand.

2. **Bug #2**: The compiler doesn't analyze closure bodies (even with explicit return types) to extract type information for generic parameter inference.

## Fix Location

Likely in these compiler crates:
- `nia-body-check` - Expression type checking and closure inference
- `nia-type-resolve` or `nia-value-resolve` - Name resolution with expected types
- Type inference - Order of inference (should infer from closure returns first)

## Impact

**High** - These patterns are fundamental to ergonomic error handling:

```nia
// Expected (clean, idiomatic):
value.checkedAdd(offset).okOr(.OutOfMemory).?
heavy_data.okOrElse(\-> .NotFound).?

// Current (ugly, verbose):
value.checkedAdd(offset).okOr(mem::Error::OutOfMemory).?
heavy_data.okOrElse[MyError](\-> MyError::NotFound).?
```

Without these fixes, the combinators lose much of their ergonomic value.

## Temporary Workarounds in stdlib

Until fixed:
1. Use fully-qualified enum variants with `okOr` (`mem::Error::Invalid` not `.Invalid`)
2. Provide explicit type parameters for `okOrElse` (`okOrElse[mem::Error]`)

## Test Cases

- `test-okOr-bug.nia` - Minimal repro for bug #1
- `crates/nia-cli/tests/std_option_ok_or.rs` - Full test suite with workarounds

## Priority

**P0** - Blocks ergonomic use of Option/Result combinators
