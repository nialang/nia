# Error Handling Patterns

This guide demonstrates idiomatic error handling in Nia, based on patterns from the standard library.

## Core Principles

Nia has no exceptions. Operations that can fail return:
- `?T` for optional values (present `?value` or `null`)
- `E!T` for error unions (success `!value` or error `error!`)

Error handling is explicit, composable, and never hidden in control flow.

## Pattern 1: Propagation with `.?`

The `.?` operator unwraps success or returns early. This is the primary error-handling pattern:

```nia
fun parseAndValidate(text: &[char]) ParseError!i32 {
    let value = parseDigits(text).?;  // returns early on error
    if value < 0 {
        return ParseError::Invalid!;
    }
    !value
}
```

For optionals, `.?` returns `null` from the enclosing function:

```nia
fun getFirst(items: &[i32]) ?i32 {
    let value = items.get(0).?;  // returns null if get returns null
    ?value.*
}
```

## Pattern 2: Checked Arithmetic with Option Helpers

Stdlib uses option helpers like `.okOr()` to convert from optional to error union. This is the idiomatic way to handle overflow:

```nia
// From lib/std/string.nia
fun appendUtf8View(&mut self, allocator: &mut mem::Allocator, view: unicode::Utf8View) mem::Error!() {
    let oldLen = self.len();
    let newLen = oldLen.checkedAdd(view.scalarCount()).okOr(mem::Error::OutOfMemory).?;
    self.text.ensureTotalCapacity(allocator, newLen).?;
    // ...
    !()
}
```

Key pattern: `checkedAdd()` returns `?usize`, `.okOr()` converts to `mem::Error!usize`, then `.?` propagates.

## Pattern 3: Pattern Matching for Branching

When you need different behavior for different outcomes, use `if-is` or `match`:

```nia
// Simple optional check
if self.storageBlock is ?block {
    allocator.free(block).?;
}

// Error union with different paths
match operation() {
    !value => processSuccess(value),
    SpecificError::OutOfMemory! => handleOom(),
    error! => handleOtherError(error),
}
```

## Pattern 4: Presence Checks

Use dedicated methods for boolean queries:

```nia
// Good: explicit presence check
fun containsKey(&self, key: &K) bool {
    self.findIndex(key, self.hashKey(key)).isPresent()
}

// Don't write:
// if self.findIndex(key, hash) is ?_ { true } else { false }
```

Standard library provides `.isPresent()`, `.isNull()` for optionals and `.isSuccess()`, `.isError()` for error unions.

## Pattern 5: Multiple Cleanup with CleanupAccumulator

When multiple cleanup operations can fail, use `CleanupAccumulator` to collect errors:

```nia
// From lib/std/string.nia
fun deinit(&mut self, allocator: &mut mem::Allocator) mem::Error!() {
    let mut cleanup: error::CleanupAccumulator[mem::Error] = .init();
    cleanup.attempt(self.text.deinit(allocator));
    cleanup.attempt(self.formatBytes.deinit(allocator));
    cleanup.finish()
}
```

This ensures all cleanup is attempted even if some fail, and reports the first error.

## Pattern 6: Transactional Updates with `defer`

Use `defer` to roll back partial state on error:

```nia
// From lib/std/string.nia
fun appendFormat(/* ... */) TextFormatError!() {
    let oldLen = self.len();
    let mut committed = false;
    defer if not committed {
        self.text.truncate(oldLen);  // rollback on error
    };
    
    // ... complex operations that may fail ...
    
    committed = true;  // mark success
    !()
}
```

If any intermediate operation fails and propagates with `.?`, the `defer` restores the old state.

## Pattern 7: Error Conversion

Convert between error types with `.mapError()`:

```nia
// From lib/std/mem/allocator.nia
fun realloc(&mut self, block: Block, newLayout: Layout) ReallocError!Block {
    let newBlock = self.alloc(newLayout)
        .mapError(.Allocation).?;  // convert mem::Error to ReallocError::Allocation
    // ...
}
```

## Pattern 8: Result Transformation

Transform success values while preserving errors:

```nia
extend[Value, Source] Source!Value {
    pub fun map[Mapped, Mapper](self, mapper: Mapper) Source!Mapped
    where Mapper: Fun(Value) Mapped
    {
        match self {
            !value => !mapper(value),
            error! => error!,
        }
    }
}
```

This allows chaining: `result.map(\x -> x * 2).map(\x -> x + 1).?`

## Pattern 9: Checking Presence Before Use

Don't check and then use. Use pattern matching to check and bind in one step:

```nia
// Good: check and bind together
if maybeValue is ?value {
    use(value);
}

// Good: early return
let value = maybeValue.?;
use(value);

// Avoid: separate check and unwrap
if maybeValue.isPresent() {
    use(maybeValue.?);  // .? could still be null in other paths
}
```

## Pattern 10: Empty Error Return

Functions that can fail but have no success value return `E!()`:

```nia
fun validate(input: &[u8]) ValidationError!() {
    if input.len() == 0 {
        return ValidationError::Empty!;
    }
    !()  // success with unit value
}
```

## Anti-Patterns to Avoid

**Don't ignore errors:**
```nia
// Bad
_ = operation().?;  // .? can't be discarded, it's control flow

// Good
operation().?;  // propagate
_ = operation();  // or explicitly ignore the whole result
```

**Don't check-then-act:**
```nia
// Bad: race between check and use
if map.containsKey(&key) {
    let value = map.get(&key).?;  // key could be gone
}

// Good: get returns optional, use it
if map.get(&key) is ?value {
    use(value);
}
```

**Don't manually construct option/error wrappers:**
```nia
// Bad
if condition { ?value } else { null }

// Good: use standard library helpers
someResult.okOr(error)
```

## Summary

Nia's error handling is:
- **Explicit**: errors are values, not hidden control flow
- **Composable**: `.?`, `.map()`, `.okOr()` chain naturally
- **Zero-cost**: error unions are plain tagged unions
- **Transactional**: `defer` enables rollback on any exit path

Follow stdlib patterns for idiomatic, robust error handling.
