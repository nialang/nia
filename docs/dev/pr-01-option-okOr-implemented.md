# PR #1: Add `okOr` and `okOrElse` to Option

> Historical implementation report. The Option APIs and tests are present in
> the current tree; the compiler-bug discussion below records initial
> hypotheses and is superseded by `closure-inference.md`.

## Status
✅ **IMPLEMENTED** - Tests pass, compiler bugs documented

## Summary

Added two new methods to `?T` (Option) for converting optional values to error unions:

```nia
extend[Value] ?Value {
    /// Convert Some to success, null to the given error.
    pub fun okOr[Error](self, error: Error) Error!Value {
        if self is ?value { !value } else { error! }
    }

    /// Convert Some to success, null to an error from a lazy supplier.
    pub fun okOrElse[Error](self, supplier: &Fun() Error) Error!Value {
        if self is ?value { !value } else { supplier()! }
    }
}
```

## Changes

### Files Modified
- `lib/std/option.nia` - Added `okOr` and `okOrElse` methods
- `crates/nia-cli/tests/std_option_ok_or.rs` - 6 comprehensive test cases

### Files Created
- `docs/dev/bug-enum-shorthand-generics.md` - Documents two critical compiler bugs discovered

## Compiler Bugs Discovered

### Bug #1: Enum Variant Shorthand in Generic Contexts

**Symptom**: `.Invalid` shorthand doesn't work in generic method arguments

```nia
// FAILS:
some.okOr(.Invalid)  // error[E0301]: omitted member requires a call or enum expected type

// WORKAROUND:
some.okOr(mem::Error::Invalid)  // Works
```

**Root cause**: Compiler doesn't substitute inferred generic type before checking enum variant shorthand

**Impact**: High - Reduces ergonomics of error handling combinators

### Bug #2: Cannot Infer Generic from Closure Return

**Symptom**: Even with explicit return types, closures don't inform generic inference

```nia
// FAILS:
none.okOrElse(\-> mem::Error::Leak)  // error[E0301]: cannot infer generic parameter

// WORKAROUND:
none.okOrElse[mem::Error](\-> mem::Error::Leak)  // Works
// OR use function reference:
fun makeError() mem::Error { mem::Error::Leak }
none.okOrElse(&makeError)  // Works
```

**Root cause**: Compiler doesn't analyze closure bodies to extract type information before attempting generic inference

**Impact**: High - Makes `okOrElse` much less ergonomic than intended

## Test Coverage

All tests pass with workarounds applied:

1. ✅ `option_ok_or_converts_some_to_success` - Basic Some case
2. ✅ `option_ok_or_converts_null_to_error` - Basic null case
3. ✅ `option_ok_or_else_with_lazy_evaluation` - Lazy error creation
4. ✅ `option_ok_or_propagates_with_try` - Error propagation with `.?`
5. ✅ `option_ok_or_chains_with_map` - Chaining with other combinators
6. ✅ `option_ok_or_works_with_different_error_types` - Custom error enums

## Usage Examples

### Basic Conversion

```nia
let config: ?Config = loadConfig();
let config = config.okOr(Error::ConfigNotFound).?;
```

### With Propagation

```nia
fun getUserName(id: i32) Error!String {
    let user = findUser(id).okOr(Error::UserNotFound).?;
    !user.name
}
```

### Chaining

```nia
input
    .parse()
    .map(\x -> x * 2)
    .okOr(Error::Invalid).?
```

### Lazy Error Creation

```nia
fun expensiveError() Error {
    // Only computed if value is null
    Error::FromExpensiveComputation
}

let result = maybeValue.okOrElse(&expensiveError);
```

## Next Steps

### Immediate (This PR)
- [x] Implement `okOr` and `okOrElse`
- [x] Write comprehensive tests
- [x] Document compiler bugs
- [x] Apply workarounds in tests

### Follow-up (Separate Issues)
- [ ] Fix Bug #1: Enum shorthand in generic contexts
- [ ] Fix Bug #2: Generic inference from closure returns
- [ ] Add similar methods to `E!T` (Result): `orElse`, `okOr`, etc.
- [ ] Update documentation examples once bugs are fixed

### Medium-Term Refactorings
- [ ] Refactor hash algorithms to use `okOr` pattern
- [ ] Refactor pool allocator to use `okOr` pattern
- [ ] Extract `BlockBuffer[N]` abstraction
- [ ] Deduplicate SipHash variants

## Verification

```bash
# Run option tests
cargo test -p nia-cli --test std_option_ok_or --no-default-features --target x86_64-pc-windows-msvc

# Run full test suite
cargo test --workspace --no-default-features --target x86_64-pc-windows-msvc
```

## Compatibility

- **Breaking**: No
- **Additions only**: Yes
- **Standard library API**: Yes (0.x allows changes)

## References

- Rust: `Option::ok_or` and `Option::ok_or_else`
- Scala: `Option.toRight` and `Option.toLeft`
- Haskell: `maybe` (similar pattern)

---

**Date**: 2026-10-28
**Implemented by**: Claude Opus 5.5
