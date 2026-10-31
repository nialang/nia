# Nia Examples

Complete programs demonstrating Nia language features and standard library usage. Each example is self-contained and builds with `nia emit --exe`.

## Learning Path

### Beginner: Core Language Features

Start here to understand Nia's syntax and basic concepts.

- **`hello.nia`** - Minimal executable with the standard entry point
- **`data.nia`** - Arrays, slices, string literals, `const`, and `static` storage
- **`basics.nia`** - Functions, structs, methods, enums, `match`, ranges, loops, and `defer`

**What you'll learn:** Entry points, value types, control flow, pattern matching, and resource cleanup with `defer`.

### Intermediate: Standard Library and Patterns

Once comfortable with the language, explore standard library patterns.

- **`errors.nia`** - Optional values (`?T`), error unions (`E!T`), pattern matching, and `.?` propagation
- **`generic.nia`** - Trait implementations, operator overloading, generic bounds, and borrowed iterator closures
- **`collections.nia`** - Explicit allocators, `ArrayList`, `HashMap`, mutation, `defer`-based cleanup
- **`io.nia`** - Formatted output through `std::io`
- **`process.nia`** - Starting child processes with piped streams (POSIX hosts only)
- **`modules/`** - Multi-file programs with public modules, imports, and re-exported APIs

**What you'll learn:** Error handling without exceptions, explicit memory management, allocator ownership, trait-based generics, and module organization.

### Advanced: Language Semantics

Deep dives into advanced language features.

- **`closure_inference/`** - Generic callable closure inference without annotations

**What you'll learn:** How Nia infers closure types through generic callable parameters.

## Running Examples

Check a single example:
```sh
cargo run -p nia-cli -- --resource-root lib check examples/beginner/hello.nia --runtime freestanding
```

Build and run:
```sh
cargo run -p nia-cli -- --resource-root lib emit --exe examples/beginner/hello.nia -o build/hello
./build/hello
```

Check all beginner examples:
```sh
for file in examples/beginner/*.nia; do
  cargo run -p nia-cli -- --resource-root lib check "$file" --runtime freestanding
done
```

Multi-file examples need their main entry point:
```sh
cargo run -p nia-cli -- --resource-root lib check examples/intermediate/modules/main.nia --runtime freestanding
```

## Key Patterns Demonstrated

### Resource Management
All examples follow the same pattern: acquire resources, register `defer` cleanup in reverse order, use resources, let `defer` clean up automatically on any exit path.

```nia
let mut allocator: mem::GeneralPurposeAllocator = .init(&mut page);
defer allocator.deinit().?;

let mut values: std::ArrayList[i32] = .init();
defer values.deinit(&mut allocator).?;
```

### Error Handling
Nia has no exceptions. Operations return `?T` for optionals or `E!T` for error unions. Use `.?` to propagate errors up the call stack:

```nia
fun parseDigits(text: &[char]) ParseError!u32 {
    if text.len() == 0 {
        return ParseError::Empty!;
    }
    // ... parse and return !value on success
}

fun caller() ParseError!() {
    let value = parseDigits(&"42").?;  // propagates errors
    !()
}
```

### Explicit Allocation
Containers take an allocator argument for every operation that may allocate:

```nia
values.push(&mut allocator, 10).?;
scores.insert(&mut allocator, key, value).?;
```

### Closures and Iterators
Closures capture explicitly and compose through iterator adapters:

```nia
let step = 10;
let addStep = \[step] x -> x + step;  // captures by value

let squares = (1..5).iter().map(\value -> value * value);
let sum = squares.fold(0, &\total, value -> total + value);
```

## What These Examples Are Not

These are readable, complete programs that demonstrate current language and library features. They are not:
- Exhaustive language tests (see compiler test suites)
- Performance benchmarks
- Edge case demonstrations
- Historical feature snapshots

The [language tour](../docs/guide/tour.md) provides a narrative introduction. The [language reference](../docs/reference/language.md) defines complete rules.
