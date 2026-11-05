# Learning Nia

These chapters teach Nia from the first program to packages and tests. Read
them in order; each builds on the ones before it, and every program in them
compiles and prints what it shows.

| Chapter | You learn |
| --- | --- |
| 1. [Getting Started](getting-started.md) | installing Nia, a first program, reading errors, a first package |
| 2. [Values, Types, And Functions](basics.md) | bindings, numbers, functions, blocks, loops, `defer` |
| 3. [Structs, Enums, And Patterns](structs-and-enums.md) | your own types, methods, `match`, patterns |
| 4. [Pointers, Arrays, And Slices](pointers-and-slices.md) | pointers, automatic addresses, arrays, slices, text, keeping pointers valid |
| 5. [Optionals And Errors](errors.md) | `?T`, `E!T`, `.?`, error conversion, cleanup on failure |
| 6. [Memory And Allocators](memory.md) | allocators, containers, owners, leaks |
| 7. [Generics And Traits](generics-and-traits.md) | generic code, bounds, traits, operators, iterators, trait objects |
| 8. [Closures](closures.md) | captures, callable views, function pointers |
| 9. [Modules And Visibility](modules.md) | files and modules, `using`, visibility, extension methods, facades |
| 10. [Testing](testing.md) | test suites, assertions, reporting, test-only code |

After these, the [guides](../README.md#guides) cover packages and
dependencies, custom builds, cross-compilation, and C interop, and the
[examples](../../examples/README.md) are complete programs to read. The
[language reference](../reference/language.md) states every rule exactly.

## Nia At A Glance

```nia
using std::io;
using std::process;

struct Point {
    x: i32,
    y: i32,
}

extend Point {
    fun lengthSquared(&self) i32 {
        self.x * self.x + self.y * self.y
    }
}

fun sum(values: &[i32]) i32 {
    let mut total = 0;
    for &value in values {
        total += value;
    }
    total
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let point = Point { x: 3, y: 4 };
    let values = [point.lengthSquared(), 7, 13];
    io::debugPrint("sum={}\n", [sum(values)]).?;
    !()
}
```

```text
sum=45
```

Nia is a small systems language for the same work as C: operating systems,
embedded software, tools, and libraries. Its type system is strong and its
inference is thorough, but it trusts the programmer with memory. If you know C
or Rust, these are the differences to expect:

| Topic | Nia |
| --- | --- |
| Memory safety | No borrow checker, lifetimes, or garbage collector. Pointers are plain values; keeping them valid is the program's job. Only closures are checked, so they cannot outlive their frame. |
| Addresses | Passing a value where `&T` is expected takes its address automatically, so `&` is rarely written. `&mut T` needs a writable place. |
| Allocation | Never implicit. Containers take an allocator on every call that allocates, and are released with `deinit`, usually through `defer`. |
| Destruction | No destructors or move semantics. Values are copied; owners are released explicitly. |
| Errors | No exceptions. `?T` for absence, `E!T` for failure, `.?` to propagate. Every result must be used or discarded with `_ =`. |
| Text | A string literal is an array of `char`, not a pointer, and has no NUL terminator. |
| Conversions | No implicit numeric conversions; `as` converts explicitly. Overflow traps. |
| Booleans | `and`, `or`, `not`. `!` belongs to error unions. |
| Methods | Declared in `extend` blocks. Trait methods are always visible; other extension methods need a `using` of their module. |
| Closures | `\x -> x + 1`, with explicit captures such as `[name]` or `[&mut name]`. |
| Runtime | No C runtime or libc. Startup is Nia's own; C libraries are ordinary dependencies. |
