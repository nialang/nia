# Values, Types, And Functions

This chapter covers the parts of Nia that every program uses: bindings,
numbers and other simple values, functions, blocks, and loops. Nia is
statically typed and expression-oriented: every value has a type known at
compile time, and most constructs, including blocks and `if`, produce values.

The programs here are complete. Save one as a file and run it with
`nia emit --exe file.nia -o program` followed by `./program`; the expected
output is shown after it.

## Bindings

`let` gives a name to a value. A `let` binding cannot be assigned again; a
`let mut` binding can:

```nia fragment
let answer = 42;
let mut count = 0;
count += 1;
count = count + answer;
```

Assigning to a `let` binding is an error:

```text
error[E0301]: assignment target is not assignable: local is let
```

A later `let` with the same name *shadows* the earlier binding. The new
binding may even have a different type, and the old one is still visible in
its own initializer:

```nia fragment
let text = "42";
let text = text.len();   // a usize now
_ = text;
```

Nia never discards a value silently. A statement that computes something must
use it, return it, or discard it explicitly with `_ =`:

```nia fragment
_ = 1 + 2;
```

The same rule applies to function results, which is why the first chapter
needed `_ = init;` for the unused process context.

## Numbers

| Type | Values |
| --- | --- |
| `i8`, `i16`, `i32`, `i64`, `i128` | signed integers of that many bits |
| `u8`, `u16`, `u32`, `u64`, `u128` | unsigned integers |
| `isize`, `usize` | pointer-sized integers; `usize` is the type of lengths and indexes |
| `f32`, `f64` | IEEE-754 floating point |

An integer literal takes its type from where it is used. With no other
information it is an `i32`, and a floating-point literal is an `f64`. A
suffix fixes the type directly:

```nia fragment
let a = 7;              // i32
let b: u8 = 200;        // u8, from the annotation
let c = 1_000_000u64;   // u64, from the suffix
let d = 0xffu8;         // hexadecimal; also 0b1010 and 0o755
let e = 2.5;            // f64
let f = 1.5f32;         // f32
_ = a + 1;
_ = b;
_ = c;
_ = d;
_ = e;
_ = f;
```

A literal must fit its type: `let x: u8 = 300;` is a compile error.

Nia never converts between numeric types implicitly, not even from a smaller
integer to a larger one. Arithmetic needs both operands to have the same type,
and `as` converts explicitly:

```nia fragment
let small: i32 = 1;
let large: i64 = 2;
let sum = small as i64 + large;
_ = sum;
```

Without the `as`, the compiler reports the mismatch:

```text
error[E0301]: type mismatch in binary operator: expected i64, got i32
```

Integer overflow, division by zero, and shifting by too many bits are not
silent. They *trap*: the program stops immediately, in debug and release
builds alike, and in constant expressions they are compile errors. Use the
checked operations in `std::math`, such as `a.checkedAdd(b)`, which return an
optional result instead, when overflow is an expected case.

> **Coming from C:** there are no implicit integer promotions or conversions,
> and signed overflow is a trap, not undefined behavior.

## Booleans, Characters, And Unit

`bool` has the values `true` and `false`. The logical operators are the
keywords `and`, `or`, and `not`, which evaluate their right side only when
needed. `!` is not a logical operator in Nia; it is reserved for error unions.

```nia fragment
let ready = true;
let empty = false;
let go = ready and not empty;
_ = go;
```

`char` is a Unicode scalar value, written `'a'`, `'中'`, or with an escape such
as `'\n'`. `as` converts a `char` to `u32`, but integers never convert to
`char`, because most integers are not valid characters; use
`std::builtin::charFromU32`, which returns an optional value. `b'a'` is a byte,
a `u8`.

`()` is the unit type, with the single value `()`. A function that returns
nothing returns `()`.

## Tuples

A tuple groups a fixed number of values of possibly different types. Its
elements are read by position:

```nia fragment
let pair = (42, true);
let first = pair.0;
let (number, flag) = pair;   // destructuring
_ = first;
_ = number;
_ = flag;
```

## Functions

A function lists typed parameters and its return type after them:

```nia
fun square(x: i32) i32 {
    x * x
}

fun clamp(value: i32, low: i32, high: i32) i32 {
    if value < low {
        return low;
    }
    if value > high {
        return high;
    }
    value
}

fun log(message: [char; 5]) {
    _ = message;
}
```

The body is a block, and the block's last expression, written *without* a
semicolon, is the result. `return` leaves early. A function without a return
type, like `log`, returns `()`.

The semicolon matters. `x * x;` with a semicolon is a statement that produces
nothing, so the function falls off its end without a value:

```text
error[E0501]: non-unit function does not return on all reachable paths
```

Functions are private to their module unless marked `pub`; modules come in
[a later chapter](modules.md).

## Blocks Are Expressions

Any `{ ... }` block is an expression whose value is its last expression. This
lets a computation keep its temporary names to itself:

```nia fragment
let area = {
    let width = 3;
    let height = 4;
    width * height
};
_ = area;
```

`if` is an expression too. Used as a value, it needs an `else`, and both
branches must have the same type:

```nia fragment
let n = 7;
let parity = if n % 2 == 0 { 0 } else { 1 };
_ = parity;
```

An `if` without `else` has type `()`, so it cannot be used where another type
is expected. The condition must be a `bool`; there is no truthiness for
numbers or pointers.

## Loops

Nia has three loops. `for` walks over anything iterable, such as a range;
`while` repeats while a condition holds; `loop` repeats until a `break`.

```nia
using std::io;
using std::process;

fun sumTo(limit: i32) i32 {
    let mut total = 0;
    for i in 1..=limit {
        total += i;
    }
    total
}

fun firstPowerOfTwoAbove(threshold: i32) i32 {
    let mut value = 1;
    while value <= threshold {
        value *= 2;
    }
    value
}

fun countOdd(limit: i32) i32 {
    let mut odd = 0;
    let mut i = 0;
    loop {
        i += 1;
        if i > limit {
            break;
        }
        if i % 2 == 0 {
            continue;
        }
        odd += 1;
    }
    odd
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    io::debugPrint("sum={} power={} odd={}\n", [
        sumTo(10), firstPowerOfTwoAbove(100), countOdd(9),
    ]).?;
    !()
}
```

```text
sum=55 power=128 odd=5
```

Ranges come in several shapes: `a..b` excludes `b`, `a..=b` includes it, and
`a..` has no end. Loops are statements and produce no value; `break` and
`continue` take no label or value and act on the innermost loop.

## Defer

`defer expression;` schedules an expression to run when the enclosing block
exits, however it exits: by reaching its end, `return`, `break`, `continue`,
or an error leaving through `.?`. Several defers in one block run in reverse
order, last registered first:

```nia
using std::io;
using std::process;

fun record(log: &mut [i32], index: usize, value: i32) () {
    log[index] = value;
}

fun steps(log: &mut [i32]) () {
    defer record(log, 0, 1);    // runs last
    defer record(log, 1, 2);    // runs second
    record(log, 2, 3);          // runs first
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut log = [0; 3];
    steps(log);
    io::debugPrint("order: {} {} {}\n", [log[2], log[1], log[0]]).?;
    !()
}
```

```text
order: 3 2 1
```

`defer` is how Nia programs release resources: write the release right after
acquiring the resource, and it happens on every path out of the block. The
[memory chapter](memory.md) relies on this throughout. A deferred expression
has type `()`; it may itself use `.?`.

## Constants And Statics

`const` names a value computed at compile time. It has no storage of its own
and can be used anywhere a compile-time value is needed, such as an array
length:

```nia
const width: usize = 4;
const area: usize = width * width;

fun grid() [i32; area] {
    [0; area]
}
```

`static` declares storage that exists for the whole run of the program, with
a fixed address. `static mut` storage can be written. Use a `static` for data
that must outlive any function call, such as text that a function returns a
pointer to, as the first chapter's library did.

## Putting It Together

```nia
using std::io;
using std::process;

const limit: i32 = 20;

fun isPrime(n: i32) bool {
    if n < 2 {
        return false;
    }
    let mut divisor = 2;
    while divisor * divisor <= n {
        if n % divisor == 0 {
            return false;
        }
        divisor += 1;
    }
    true
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut count = 0;
    let mut largest = 0;
    for n in 0..limit {
        if isPrime(n) {
            count += 1;
            largest = n;
        }
    }
    io::debugPrint("primes below {}: {} (largest {})\n", [limit, count, largest]).?;
    !()
}
```

```text
primes below 20: 8 (largest 19)
```

## Summary

- `let` binds immutably, `let mut` mutably; a value is never discarded
  silently, so use `_ =` when you mean to drop one.
- Literals take their type from context; there are no implicit numeric
  conversions, and overflow traps.
- Logical operators are `and`, `or`, and `not`.
- Blocks and `if` are expressions; the last expression without a semicolon is
  the value.
- `for`, `while`, and `loop` are statements.
- `defer` runs an expression on every exit from its block, in reverse order.

The [language reference](../reference/language.md) has the exact rules for
[literals](../reference/language.md#lexical-structure),
[types](../reference/language.md#types), and
[statements](../reference/language.md#statements-and-expressions). Next,
[Structs, Enums, And Patterns](structs-and-enums.md) builds your own types.
