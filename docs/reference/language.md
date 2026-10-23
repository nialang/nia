# Nia Language Reference

This document defines the syntax and meaning of Nia source code. It is a
reference, not a tutorial; the [language tour](../guide/tour.md) introduces the
same material in order. Data representation and the C boundary are specified in
the [ABI reference](abi.md), and the operations in `std::builtin` are listed in
the [builtins reference](builtins.md).

Nia is statically typed and has no garbage collector, exceptions, implicit
allocation, ownership checker, or destructors. Allocation and resource
management are ordinary library operations.

Contents:

1. [Overview](#overview)
2. [Programs](#programs)
3. [Lexical Structure](#lexical-structure)
4. [Types](#types)
5. [Declarations](#declarations)
6. [Patterns](#patterns)
7. [Statements And Expressions](#statements-and-expressions)
8. [Closures](#closures)
9. [Constant Evaluation](#constant-evaluation)
10. [Generics](#generics)
11. [Methods](#methods)
12. [Traits](#traits)
13. [Modules And Visibility](#modules-and-visibility)
14. [Unchecked Operations And Traps](#unchecked-operations-and-traps)
15. [Pointers At The C Boundary](#pointers-at-the-c-boundary)

## Overview

A complete program that uses most of the core forms:

```nia
using std::process;

struct Pair[T, U] {
    first: T,
    second: U,
}

struct Vec2 {
    x: i32,
    y: i32,
}

extend Vec2 {
    fun len2(&self) i32 {
        self.x * self.x + self.y * self.y
    }
}

fun add(a: i32, b: i32) i32 {
    a + b
}

fun sum(xs: &[i32]) i32 {
    let mut total = 0;
    for &value in xs {
        total += value;
    }
    total
}

fun score(answer: Pair[i32, i32]) i32 {
    match answer.first {
        0..10 => answer.second,
        10..=42 => answer.first + answer.second,
        _ => 0,
    }
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;

    let v = Vec2 { x: 3, y: 4 };
    let values = [add(40, 2), v.len2(), 7];
    let pair = Pair[i32, i32] { first: values[0], second: sum(&values) };

    if score(pair) != 116 {
        return process::ExitCode(1)!;
    }

    !()
}
```

## Programs

Nia source files use the `.nia` extension and contain UTF-8 text. Each file is
one [module](#modules-and-visibility).

Program startup is supplied by the selected runtime; see the
[runtime model](platform-support.md#runtime-model). With the `freestanding`
runtime the entry package declares a public `main` with this signature:

```nia
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    !()
}
```

Returning `!()` reports success. Returning an error such as
`process::ExitCode(1)!` terminates the process with that status. Under the
`bare` runtime no entry contract applies, and a function named `main` is an
ordinary function.

A package's `build.nia` is an ordinary Nia program that uses `std::build`; see
[Build Scripts](../guide/build-scripts.md).

## Lexical Structure

### Comments And Whitespace

A line comment starts with `//` and runs to the end of the line. There are no
block comments. Whitespace separates tokens and is otherwise insignificant,
with one exception: in a type, generic arguments must follow the type name
directly, so `Pair[i32]` is a type while `Pair [i32]` is not.

### Identifiers

An identifier starts with an ASCII letter or `_` and continues with ASCII
letters, digits, or `_`. A lone `_` is not an identifier; it is the wildcard
and inference token. Identifiers are ASCII-only; character and string literal
contents are not restricted.

### Keywords

These words are reserved and cannot be used as identifiers:

```text
and       as        bool      break     const     continue  defer
else      enum      extend    extern    false     for       fun
if        in        is        let       loop      match     module
mut       never     not       null      opaque    or        pkg
pub       return    self      Self      static    struct    super
trait     true      type      union     using     where     while
```

The other primitive type names (`i32`, `u8`, `usize`, `f64`, `char`, and so on)
are not keywords; they are predeclared type names. `Fun` followed by `(` is the
[callable view](#callable-views) type constructor. `()` is the unit type and has
no keyword spelling.

### Integer Literals

```nia
0
42
0xff
0b1010
0o755
1_000_000
```

Prefixes are `0x` (hexadecimal), `0b` (binary), and `0o` (octal). An `_` may
appear only between two digits valid for the radix. A literal may end with a
type suffix:

```text
i8 i16 i32 i64 i128 isize
u8 u16 u32 u64 u128 usize
```

```nia
let mask = 0xffu8;
let bits = 0b1010_0000u8;
let mode = 0o755usize;
```

A suffix fixes the literal's type; the value must fit in it. An unsuffixed
integer literal takes its type from context and defaults to `i32` when no
context applies.

### Floating-Point Literals

```nia
1.0
1e3
1_000.5
1.0e-3
```

A floating-point literal has a fractional part, an exponent, or both, and may
end with `f32` or `f64`. `1.5f32` and `1.0e-3f64` are valid; `1f32` is not,
because `1` is an integer literal and `f32` is not an integer suffix. The value
must be finite in its type. An unsuffixed floating-point literal takes its type
from context and defaults to `f64`.

### Numeric Literal Typing

For a built-in binary arithmetic, bitwise, comparison, or equality operator
whose operands share a numeric type, a typed operand supplies the type of an
unsuffixed literal on either side. A suffix is never overridden by the other
operand. The count of a shift is typed independently and does not influence the
type of the shifted value.

An expected optional or error-union type passes its payload type to the
constructed value: in a function returning `?usize`, `return ?0;` makes `0` a
`usize`. Wrapping does not force the literal to default to `i32`.

### Character And String Literals

| Form | Example | Type |
| --- | --- | --- |
| Character | `'a'`, `'\n'`, `'中'` | `char` (one Unicode scalar value) |
| Byte character | `b'a'`, `b'\0'` | `u8` (must decode to exactly one byte) |
| String | `"nia"`, `"中a"` | `[char; N]`, N = number of scalar values |
| Byte string | `b"nia\0"` | `[u8; N]`, N = number of bytes |

String literals are arrays, not pointers. `"nia"` is the same value as
`['n', 'i', 'a']`, and `b"nia\0"` is the same value as
`[b'n', b'i', b'a', b'\0']`. Non-ASCII characters inside a byte string or byte
character are encoded as UTF-8. A string literal cannot contain a raw line
break; write `\n`.

Escape sequences:

| Escape | Meaning |
| --- | --- |
| `\n` | line feed (U+000A) |
| `\r` | carriage return (U+000D) |
| `\t` | horizontal tab (U+0009) |
| `\\` | backslash |
| `\'` | single quote |
| `\"` | double quote |
| `\0` | NUL (U+0000) |
| `\xHH` | exactly two hex digits; a byte in byte literals, scalar U+00HH otherwise |
| `\u{H...}` | one to six hex digits naming a Unicode scalar value; UTF-8 bytes in byte literals |

Any other character after `\` is an error.

Adjacent string literals with the same prefix form one literal. This is
source-level concatenation, and mixing `"..."` with `b"..."` is an error:

```nia
let greeting =
    "hello, "
    "world\n";
let bytes =
    b"ni"
    b"a\0";
```

A string literal does not convert to a slice by itself. Take its address; the
array-pointer-to-slice coercion then applies:

```nia
let text: &[char] = &"nia";
let bytes: &[u8] = &b"nia";
```

### Boolean And Null Literals

`true` and `false` have type `bool`. `null` is the empty [optional](#optional-and-error-union-types)
value and needs an expected optional type.

## Types

### Primitive Types

| Type | Values |
| --- | --- |
| `i8` `i16` `i32` `i64` `i128` `isize` | signed integers; `isize` has the target pointer width |
| `u8` `u16` `u32` `u64` `u128` `usize` | unsigned integers; `usize` has the target pointer width |
| `f32` `f64` | IEEE-754 binary32 and binary64 |
| `bool` | `true`, `false` |
| `char` | a Unicode scalar value |
| `()` | the unit value `()` |
| `never` | no values |

Every integer type has associated constants `MIN` and `MAX`, such as
`i32::MAX`.

`()` is also the zero-element tuple. A function without a written return type
returns `()`.

`never` is the type of expressions that do not complete normally: `return`,
`break`, `continue`, a `loop` with no reachable exit, and calls to functions
returning `never`. A `never` expression is accepted wherever a value is
expected. `never` is valid as a function or function-pointer return type; it is
not valid as the type of a variable, field, parameter, or array element.

### SIMD Vector Types

A vector type name is a lane type, `x`, and a decimal lane count of at least 1:
`u8x16`, `i32x4`, `f32x8`, `boolx16`. Lane types are the integer types, `f32`,
`f64`, and `bool`. Vector names are predeclared like the scalar names.

- Integer and floating-point vectors support the arithmetic operators lane by
  lane. Integer vectors also support the bitwise operators and shifts; a shift
  count is a vector of the same type (splat a scalar for a uniform count).
- Comparisons produce a mask vector `boolxN` with the same lane count.
- `boolxN` mask vectors support the bitwise operators but not arithmetic,
  negation, or shifts.

Integer-vector arithmetic follows the scalar [trap rules](#unchecked-operations-and-traps)
for every lane: if any lane traps, the whole operation traps. Vectors are
created and inspected with `splat`, `extract`, `insert`, and `bitmask` from
[`std::builtin`](builtins.md#simd). Layout is in the
[ABI reference](abi.md#simd-vectors).

```nia
using std::builtin;

fun lanes() usize {
    let a: u8x4 = builtin::splat[u8x4](3);
    let b = builtin::insert(a, 1, 7);
    let sum = a + b;
    let mask: boolx4 = sum == builtin::splat[u8x4](6);
    builtin::bitmask(mask)
}
```

### Tuples

Tuple types are written `(T0, T1, ...)`. A one-element tuple needs a trailing
comma, `(T,)`; without it the parentheses only group. Tuple values and tuple
patterns use the same syntax. `.0`, `.1`, `.10` select an element by position;
the position is a decimal literal without leading zeros, and an out-of-range
position is an error. An element selection is a place when its base is a place.

```nia
let pair = (1, true);
let (number, flag) = pair;
let single = (5,);
let first = pair.0 + single.0;
```

### Pointers

| Type | Meaning |
| --- | --- |
| `&T` | read-only pointer |
| `&mut T` | writable pointer |
| `^T` | read-only volatile pointer |
| `^mut T` | writable volatile pointer |

Pointers are plain values; Nia has no borrow checker and does not track
lifetimes. Read-only and writable pointers are distinct types. `&mut T` is
accepted where `&T` is expected; every other pointer conversion is an explicit
[cast](#casts). There is no pointer arithmetic: cast to `usize`, compute, and
cast back.

`&place` takes a read-only pointer and `&mut place` a writable one; `&mut`
requires a writable place. `p.*` dereferences. Reads and writes through `^T`
and `^mut T` are volatile accesses; `^T` cannot be written through.

```nia
fun read_reg(reg: ^u32) u32 {
    reg.*
}

fun write_reg(reg: ^mut u32, value: u32) {
    reg.* = value;
}
```

Taking the address of a value that is not a place, such as `&42i32` or
`&make_point()`, creates a temporary that lives until the end of the enclosing
block and yields a pointer to it. A `never` expression cannot be addressed.

`opaque` is an incomplete type usable only as a pointer target: `&opaque`,
`&mut opaque`, `^opaque`, `^mut opaque`. It has no values, fields, or
dereference.

### Arrays

`[T; N]` is an array of `N` elements; the length is a compile-time `usize`
and part of the type. Array literals list elements or repeat one value:

```nia
let xs = [1, 2, 3];                        // [i32; 3]
let wide = [1i64, 2, 3];                   // [i64; 3]
let inferred: [i32; _] = [4, 5, 6];        // `_` takes the literal's length
let zeros: [[i32; 3]; 2] = [[0; 3]; 2];    // [value; count]
let empty: [i32; 0] = [];
```

All elements share one type. A suffix or typed element fixes it, and an
expected array type applies to every element; conflicting element types are an
error (`[1u8, 2u64]` does not widen). An empty literal needs an expected type.
The repeat count must be a compile-time `usize`. `[T; _]` is valid only where
an array or string literal supplies the length.

Arrays never convert implicitly to slices or pointers. `arr[i]` indexes an
array; it is a place when `arr` is a place.

### Slices

`&[T]` and `&mut [T]` are pointers to a run of elements together with its
length. `[T]` itself is unsized and appears only behind a pointer or as an
`extend` target. A slice is formed by range-indexing a base and taking the
address:

```nia
let mut xs: [i32; 4] = [1, 2, 3, 4];
let all = &xs[..];          // &[i32], length 4
let middle = &xs[1..3];     // &[i32], length 2
let tail = &mut xs[2..];    // &mut [i32], length 2
tail[0] = 10;
```

The base may be an array, a slice, or a pointer to a single element. For a
pointer base, an omitted upper bound means length 1, and an explicit range is
taken as given (`&p[0..12]` claims twelve elements; the program is responsible
for that claim). Range bounds in slicing have type `usize`, and unsuffixed
literals in them are inferred as `usize`.

A writable slice requires writable elements: slicing an array value with
`&mut` needs a writable array place, and slicing through a pointer or slice
follows the pointee, not the binding that holds it. `&mut` on a `&[T]` base is
an error. A bare range index such as `xs[..]` is not a value; write `&xs[..]` or
`&mut xs[..]`.

Indexing a slice yields a place: read-only for `&[T]`, writable for `&mut [T]`,
even when the slice itself comes from a call.

When the expected type is exactly `&[T]` or `&mut [T]`, a pointer to an array
converts to a slice of the whole array: `&[T; N]` to `&[T]`, and `&mut [T; N]`
to `&mut [T]` or `&[T]`. Array values do not convert. Method calls apply the
same conversion when the array pointer has no matching method of its own but
the slice type does; a read-only array pointer still cannot call a method that
needs `&mut [T]`.

```nia
fun first(xs: &[i32]) i32 {
    xs[0]
}

fun clear(xs: &mut [i32]) {
    xs[0] = 0;
}

fun use_arrays() i32 {
    let mut arr = [1, 2, 3];
    clear(&mut arr);
    first(&arr) + first(&[4, 5, 6])
}
```

Indexing and slicing do not check bounds; see
[Unchecked Operations](#unchecked-operations-and-traps). The standard library
provides checked slice operations.

### Ranges

Range expressions and range types share six shapes:

| Expression | Type | `start()` | `end()` |
| --- | --- | --- | --- |
| `a..b` | `T..T` | yes | yes |
| `a..=b` | `T..=T` | yes | yes |
| `a..` | `T..` | yes | no |
| `..b` | `..T` | no | yes |
| `..=b` | `..=T` | no | yes |
| `..` | `..` | no | no |

There is no nominal range type. Both bounds of a bounded range have the same
integer type. Ranges are ordinary values; `start()` and `end()` return the
bound. An omitted slice bound is supplied by the slicing operation, not by
rewriting the range. `a..b`, `a..=b`, and `a..` are iterable in `for` loops.

```nia
fun span() usize {
    let r = 2usize..5;
    r.end() - r.start()
}
```

### Optional And Error Union Types

`?T` is an optional `T`. `?value` constructs the present case and `null` the
empty case. Both need the full optional type from context; an expected `?T`
is also the expected type of `value`.

`E!T` is an error union with error type `E` and success type `T`. `!value`
constructs the success case and `value!` the error case. Both constructors
require an expected `E!T`; `let x = !10i32;` is an error because `E` is unknown.

```nia
let present: ?i32 = ?10;
let absent: ?i32 = null;
let ok: u8!i32 = !10;
let failed: u8!i32 = 2!;
```

In types, `?` binds tighter than `!`, and `!` groups to the right:
`?E!T` is `(?E)!T`, and `A!B!C` is `A!(B!C)`. Use parentheses for other
groupings, such as `?(E!T)`.

Values are inspected with [patterns](#patterns) and propagated with
[`.?`](#propagation).

### Structs, Unions, And Enums

Struct, union, and enum types are nominal and introduced by
[declarations](#declarations). Type aliases do not create new types.

### Function Items And Function Pointers

The name of a declared function is a function item: a zero-sized value that
names one definition, or one instance of a generic definition. Calling it is a
direct call. A function item can be stored in a local:

```nia
fun add(a: i32, b: i32) i32 {
    a + b
}

fun id[T](x: T) T {
    x
}

fun call_items() i32 {
    let f = add;
    let g = id[i32];
    f(1, 2) + g(3)
}
```

`&fun(A, B) R` is a thin function pointer. `&name` turns a function item into
one; a generic function must be instantiated first (`&id[i32]`, not `&id`).
An omitted return type means `()`. Only the read-only form exists; `&mut fun`
is an error. `&extern fun(A, B) R` is a pointer to a function with the C calling
convention, created by `&` on an `extern fun`; `extern` appears only in the
type. Function pointer types describe runtime calls and are never callable
during constant evaluation.

```nia
fun pointers() i32 {
    let p = &add;                       // &fun(i32, i32) i32
    let q: &fun(i32) i32 = &id[i32];
    p(1, 2) + q(3)
}
```

Non-unit enum variants and receiver methods also have function-item forms; see
[Enums](#enums) and [Methods](#methods).

### Callable Views

`Fun(A, B) R` is the unsized interface shared by all [closures](#closures) with
parameter types `A, B` and return type `R`; an omitted `R` means `()`. It is
used only behind a pointer:

| Type | Meaning |
| --- | --- |
| `&Fun(A) R` | read-only callable view |
| `&mut Fun(A) R` | writable callable view |

A view is a non-owning value that refers to a closure's state; it never
allocates, frees, or extends the life of that state. `Fun` types cannot be
variadic. Views are created only by taking the address of a closure where a
view type is expected; see [Closures](#closures) for the rules. `&Fun(...)` and
`&fun(...)` are different types. The representation is in the
[ABI reference](abi.md#callable-views-and-closures).

### Trait Objects

When the pointee of `&` or `&mut` is a trait, the pointer is a trait object:
`&Trait`, `&mut Trait`, or with arguments and associated-type bindings
`&Source[Item = i32]`. A bare trait name is not a value type. Trait objects
dispatch dynamically; see [Trait Objects](#trait-object-coercion-and-dispatch).

## Declarations

A source file is a sequence of items. Every item may be preceded by
[attributes](#attributes) and a [visibility](#visibility) modifier:

| Item | Form |
| --- | --- |
| Function | `fun name(params) Return { body }`, `const fun ...` |
| External function | `extern fun name(params) Return;` or with a body |
| Struct | `struct Name { field: T, ... }`, `struct Name(T, ...)`, `extern struct` |
| Union | `union Name { field: T, ... }`, `extern union` |
| Enum | `enum Name: Backing { Variant, ... }` |
| Type alias | `type Name = T;` |
| Trait | `trait Name { ... }` ([Traits](#traits)) |
| Extension | `extend Type { ... }`, `extend Type : Trait { ... }` ([Methods](#methods)) |
| Static | `static name: T = value;`, `static mut`, `extern static` |
| Constant | `const name: T = value;` |
| Module | `module name;` ([Modules](#modules-and-visibility)) |
| Import | `using path;` ([Modules](#modules-and-visibility)) |

Top-level `let` is not allowed; global storage uses `static`.

### Functions

```nia
fun square(x: i32) i32 {
    x * x
}

fun abs(x: i32) i32 {
    if x < 0 {
        return -x;
    }
    x
}
```

Parameters are `name: Type`. The return type follows the parameter list; when
omitted it is `()`. The body is a [block](#blocks), and its tail expression is
the return value. `return expr;` returns early, and `return;` returns `()`.
`const fun` declares a function that can also run during
[constant evaluation](#constant-evaluation). Generic parameters and `where`
clauses are described in [Generics](#generics).

### External Functions And Statics

`extern` declares or defines a symbol with the C ABI of the target. Visibility
comes first: `pub extern fun`, never `extern pub fun`.

```nia
extern fun puts(text: &u8) i32;              // declared, defined elsewhere
extern fun printf(format: &u8, ...) i32;     // variadic declaration
extern fun notify(code: i32);                // returns ()

pub extern fun nia_add(a: i32, b: i32) i32 { // defined here, callable from C
    a + b
}

extern static errno: i32;
extern static mut shared_counter: usize;
```

- A bodyless `extern fun` declares a foreign function; with a body it defines a
  C-callable function. Both use the source name as the symbol name.
- `@[linkName("symbol")]` renames the symbol a bodyless `extern fun` or an
  `extern static` refers to. `@[exportName("symbol")]` renames the symbol an
  `extern fun` definition exports. Neither applies to ordinary functions, and
  there is no `noMangle` attribute; ordinary functions always use Nia's internal
  names.
- `...` makes a declaration variadic. Only bodyless `extern fun` declarations
  can be variadic.
- `extern static` declares a foreign global and requires a type; it has no
  initializer.
- `extern fun` cannot be `const` or generic.
- In an executable, every top-level `pub extern fun` definition of the entry
  package is exported even if Nia code never calls it.

There are no `extern { ... }` blocks and no ABI strings. Which types may cross
the boundary, symbol naming, and argument promotion for variadic calls are
specified in the [ABI reference](abi.md#the-c-abi-boundary); the
[C interop guide](../guide/c-interop.md) shows the patterns.

### Structs

```nia
struct Point {
    x: i32,
    y: i32,
}

struct Meters(f64)
```

A struct has named fields, or positional fields in the tuple form
`struct Name(T, ...)` (at least one). Fields have no visibility modifier.
Ordinary struct layout is chosen by Nia and may differ from declaration order;
`extern struct` uses C layout in declaration order (see
[ABI](abi.md#structs)). Tuple structs cannot be `extern`.

Construction names the type. A field initializer `x` is shorthand for `x: x`.
Inside an `extend` block `Self { ... }` names the extended type. When an
expected type is known, the name may be omitted with `.{ ... }`:

```nia
fun points() i32 {
    let x = 1;
    let p = Point { x, y: 2 };
    let q: Point = .{ x: 3, y: 4 };
    let m = Meters(2.5);
    p.x + q.y + m.0 as i32
}
```

A bare `{ x: 1 }` is a block, not a struct literal. Fields are accessed with
`value.field` (and tuple-struct fields with `.0`, `.1`); access through a
pointer dereferences automatically.

### Unions

```nia
union Bits {
    i: i32,
    f: f32,
}
```

A union literal initializes exactly one field: `Bits { i: 42 }`. Unions are
untagged: writing a field overwrites the shared storage, and reading a
different field reinterprets the same bytes. Such a read is meaningful only
when the program knows the representation is valid. Unions may be generic.
`extern union` uses C layout. Size and field offsets are in the
[ABI reference](abi.md#unions).

### Enums

```nia
enum Color: u8 {
    Black,
    White,
    Red = 10,
}

enum Event {
    Quit,
    Data(i32),
    Resize { width: i32, height: i32 },
}
```

An enum lists variants. A variant is unit-like, carries a tuple payload
(`Data(i32)`), or carries named fields (`Resize { ... }`). Unit-like variants
may have explicit integer values (`Red = 10`); otherwise values start at 0 and
increase by one. The backing integer type follows `:` and defaults to `i32`.

Variants are named through the enum: `Color::Black`, `Event::Data(1)`,
`Event::Resize { width: 1, height: 2 }`. Where the expected type is an enum,
the owner may be omitted: `.Black`, `.Data(1)`, `.Resize { width: 1, height: 2 }`.

An enum value casts to any integer type. Integers do not cast to an ordinary
(closed) enum. An open enum ends its body with `_`; every value of its backing
type is then a valid value, integers cast to it, and every `match` on it needs a
`_` arm:

```nia
enum Flag: u32 {
    A,
    B,
    _,
}

fun flag_value() u32 {
    let flag: Flag = 3 as Flag;
    flag as u32
}
```

`_` must be last, has no value, and cannot be named as `Flag::_`.

Every non-unit variant is also a constructor function. `&Event::Data` (or
`&.Data` with an expected type) is a thin function pointer taking the payload
fields in order and returning the enum. `&mut` cannot form a constructor
pointer, unit variants are not constructors, and constructor pointers cannot be
used in constant evaluation.

```nia
fun constructors() Event {
    let data: &fun(i32) Event = &Event::Data;
    let resize: &fun(i32, i32) Event = &.Resize;
    _ = data(1);
    resize(640, 480)
}
```

### Type Aliases

```nia
type Byte = u8;
type Ptr[T] = &T;
```

An alias names an existing type and may take generic parameters. It does not
create a new nominal type.

### Statics

`static` declares storage with a fixed address that lives for the whole
program. `static mut` storage is writable. Statics may appear at top level or
in a block; a block-level static is visible only in that block.

```nia
static greeting: [u8; 6] = b"hello\0";
static mut counter: i32 = 0;
static first_byte = &greeting[0];
static buffer: [u8; 64];

fun bump() i32 {
    counter += 1;
    counter
}
```

The type may be inferred from the initializer. A static without an initializer
needs a type and is zero-initialized (`extern static` instead refers to foreign
storage). Initializers must be static data: literals, arithmetic on constants,
aggregates of static data, addresses of other statics, and named `const`
values. They cannot run code, so `static bad = { 1 + 2 };` is an error. A static
does not decay to its address; `static q: &i32 = target;` is an error where
`&target` is meant.

Use `static`, not `const`, when data needs an address, for example when it is
passed to foreign code.

### Constants

`const` binds a compile-time value. It has no storage and no address, so `&`
on a constant is an error. The initializer is evaluated by
[constant evaluation](#constant-evaluation), and cyclic constants are errors.

```nia
const width: usize = 4;
const area: usize = width * width;

struct Config {
    width: usize,
}

const config = Config { width: 8 };
const config_width: usize = config.width;
```

Constants may appear at top level, as [associated constants](#associated-constants),
and in blocks. There is no `const mut` and no `const let`. Contexts that need a
compile-time value, such as array lengths, read constants, not statics.

### Local Bindings

Inside a block:

| Form | Meaning |
| --- | --- |
| `let pattern = value;` | immutable binding with storage |
| `let mut pattern = value;` | mutable binding |
| `let name: T;` / `let mut name: T;` | uninitialized storage of type `T` |
| `const name: T = value;` | compile-time value, no storage |
| `static name: T = value;` | block-scoped static storage |
| `using path;` | block-scoped import |

A `let` binding cannot be assigned and cannot be the target of `&mut`; it does
not make the pointed-to data immutable. A declaration without a type needs an
initializer. `let mut` applies to every name bound by the pattern. A type
annotation describes the value matched by the whole pattern, so
`let &x: &i32 = ptr;` is correct and `let &x: i32 = ptr;` is a type error.
Binding patterns must be [irrefutable](#patterns).

```nia
fun bindings(ptr: &i32, pair: &(i32, i32)) i32 {
    let a = 1;
    let mut b: i32 = 2;
    b += a;
    let &c = ptr;
    let &(left, right): &(i32, i32) = pair;
    let a = a + left;       // shadows the earlier `a`
    a + b + c + right
}
```

A later binding may reuse an earlier name in the same or an enclosing scope; it
shadows the earlier binding from the end of its own declaration, so its
initializer still sees the old one. Duplicate names inside one pattern or one
parameter list are errors.

Nia does not check that storage is initialized before it is read; reading
uninitialized storage is the program's responsibility.

### Attributes

An attribute is written `@[name]`, `@[name(args...)]`, or with a dotted path
`@[a.b(args...)]`, before an item, a statement, a struct or union field, or a
method in a trait or `extend` block. Arguments are expressions. `@` has no
other use; there are no `@name` expressions. Conditional, profile, and test
attributes select only whole items and statements.

| Attribute | Applies to | Effect |
| --- | --- | --- |
| `@[if condition]` | items, statements | keeps the code only when the condition holds for the target |
| `@[debug]`, `@[release]` | items, statements | keeps the code only in that build profile |
| `@[test]` | items, statements | keeps the code only in test compilations |
| `@[linkName("s")]` | bodyless `extern fun`, `extern static` | symbol referenced |
| `@[exportName("s")]` | `extern fun` with a body | symbol exported |
| `@[trackCaller]` | non-`extern` functions and trait methods | caller location tracking |
| `@[naked]` | `extern fun` definitions | no compiler-generated prologue or epilogue |

An unrecognized attribute on a function, static, constant, trait, or type alias
is an error. On struct declarations, fields, and statements an unrecognized
attribute has no effect.

#### Conditional Compilation

The condition of `@[if ...]` is a small language of its own, separate from
Nia expressions and constant evaluation. It accepts `true`, `false`, integer and
string literals, the target fact names below, `not`, `and`, `or`, `==`, `!=`,
and parentheses.

| Name | Type | Meaning |
| --- | --- | --- |
| `arch` | string | CPU architecture, such as `"x86_64"` or `"aarch64"` |
| `vendor` | string | target vendor, such as `"unknown"`, `"pc"`, `"apple"` |
| `os` | string | operating system, such as `"linux"`, `"windows"`, `"macos"` |
| `env` | string | platform ABI flavor, such as `"msvc"`, or `""` |
| `endian` | string | byte order, such as `"little"` |
| `pointerWidth` | integer | pointer width in bits |

The values for each supported target are listed in
[Target Facts](platform-support.md#target-facts). The same facts are available
to ordinary code as constants in `std::builtin::target`.

```nia
@[if os == "linux" and arch == "x86_64"]
fun platform_name() &[char] {
    &"linux-x86_64"
}

@[if not (os == "linux" and arch == "x86_64")]
fun platform_name() &[char] {
    &"other"
}

fun word_size() usize {
    @[if pointerWidth == 64]
    return 8;
    4
}
```

The whole file must parse, so inactive code must be syntactically valid. Inactive
items and statements are removed before name resolution and type checking;
errors inside them are not reported for that target. Code meant for several
targets is checked once per target.

#### Build Profiles And Tests

`@[debug]` and `@[release]` keep an item or statement only in the matching
build profile; the default profile is `debug`. An item may carry at most one
profile attribute. `@[test]` keeps code only when compiling tests, in either
profile. Profile and test selection is independent of target facts,
optimization level, and runtime.

#### Caller Location

Inside a `@[trackCaller]` function, `std::callerLocation()` returns the
location of the outermost call that entered the chain of tracked calls. In an
untracked function it returns the location of the `callerLocation()` call
itself. A trait implementation inherits `@[trackCaller]` from the trait method
declaration and cannot add it when the declaration lacks it. Tracked functions
cannot be converted to `&fun(...)` pointers or callable views. `callerLocation`
is usable in constant evaluation. The result is a `std::SourceLocation` with a
file identity and one-based line and column numbers (columns count Unicode
scalar values).

```nia
@[trackCaller]
fun here() std::SourceLocation {
    std::callerLocation()
}

fun report() u32 {
    let location = here();   // the location of this call
    location.line()
}
```

#### Naked Functions

A `@[naked]` `extern fun` has a body that consists of inline assembly only. The
compiler emits no prologue, epilogue, frame, or spills, so its
[`asm`](builtins.md#inline-assembly) calls cannot have `inputs` or `outputs`.
The assembly must not return into compiled code; it names other symbols
directly. Naked functions serve process entry and helpers whose register
contract no calling convention describes.

## Patterns

Patterns appear in `match` arms, in `is` conditions, and in `let` and `for`
bindings. Binding positions accept only irrefutable patterns; `match` and `is`
accept all of them.

| Pattern | Matches | Irrefutable |
| --- | --- | --- |
| `_` | anything, binds nothing | yes |
| `name` | anything, binds `name` | yes |
| `mut name` | anything, binds a mutable `name` | yes |
| `(p0, p1, ...)` | a tuple, element by element | if all elements are |
| `&p`, `&mut p` | a `&T` / `&mut T`, matching `p` against the pointee | if `p` is |
| `Name { f: p, g, .. }` | a struct by fields | if all listed fields are |
| `Name(p0, ...)` | a tuple struct | if all elements are |
| `Enum::V`, `.V` | a unit variant | no |
| `Enum::V(p, ...)`, `.V(p, ...)` | a tuple variant | no |
| `Enum::V { f: p, .. }`, `.V { ... }` | a named-field variant | no |
| `?p` | a present optional whose payload matches `p` | no |
| `null` | an empty optional | no |
| `!p` | an error-union success whose value matches `p` | no |
| `p!` | an error-union error whose value matches `p` | no |
| literal | an equal scalar value (`0`, `'a'`, `true`) | no |
| `a..b`, `a..=b` | an integer in the closed range | no |
| `path::NAME`, `(expr)` | a value equal to a named constant | no |

A bare identifier always introduces a binding, whatever its spelling. To match
against a named constant or variant, use a qualified path (`Color::Red`), the
contextual form (`.Red`), or parentheses (`(limit)`).

Struct patterns name the type. Field `x` is shorthand for `x: x`; `field: _`
ignores one field. Fields may appear in any order. All fields must be listed
unless the pattern ends with `..`, which may appear once, last. `..` exists only
in struct and named-variant patterns; tuple and slice patterns have no rest
form. Unknown or repeated fields are errors.

Optional and error-union patterns nest: in a `?(E!T)` scrutinee, `?!v` matches
a present success and `?e!` a present error.

Range patterns need both endpoints, which must be compile-time integer
constants. Empty ranges are errors. Use `_` instead of an open-ended range.

```nia
struct Point {
    x: i32,
    y: i32,
}

enum Shape {
    Dot,
    Circle(i32),
    Rect { width: i32, height: i32 },
}

const limit: i32 = 100;

fun classify(value: i32) i32 {
    match value {
        0, 1 => 0,
        2..5 => 1,          // 2, 3, 4
        5..=7 => 2,         // 5, 6, 7
        (limit) => 3,
        _ => 4,
    }
}

fun area(shape: Shape) i32 {
    match shape {
        .Dot => 0,
        .Circle(r) => 3 * r * r,
        .Rect { width, height } => width * height,
    }
}

fun axis(point: Point) i32 {
    match point {
        Point { x: 0, y } => y,
        Point { x, y: 0 } => x,
        Point { x, .. } => x,
    }
}

fun nested(value: ?(u8!i32)) i32 {
    match value {
        ?!v => v,
        ?e! => e as i32,
        null => 0,
    }
}
```

In bindings, pointer and struct patterns destructure without a `match`:

```nia
fun swap_sum(pair: &(i32, i32), point: Point) i32 {
    let &(a, b) = pair;
    let Point { x, .. } = point;
    a + b + x
}
```

### Exhaustiveness

Every `match` must be exhaustive, including one used only for its effects;
write `_ => {}` to ignore the remaining values. Exhaustiveness is computed over
nested patterns, and an error names one missing pattern. An arm that earlier
arms already cover is an error. A `match` on an open enum always needs `_`.

An arm may list several patterns separated by commas, but only if none of them
binds a name.

## Statements And Expressions

### Statements

| Statement | Form |
| --- | --- |
| Binding | `let`, `let mut`, `const`, `static` ([Local Bindings](#local-bindings)) |
| Import | `using path;` |
| Expression | `expr;` |
| Return | `return;`, `return expr;` |
| Loop control | `break;`, `continue;` |
| Cleanup | `defer expr;` |
| Loops | `for pattern in expr { }`, `while condition { }`, `loop { }` |

Statements end with `;`, except that loops, blocks, `if`, and `match` used as
statements need none. A block's final expression without `;` is its tail.

An expression statement must not silently drop a value. If the value is
neither `()` nor `never`, discard it explicitly with `_ =`:

```nia
fun compute() i32 {
    1
}

fun discard() {
    _ = compute();      // `compute();` would be an error
}
```

Assignment `place = value` and compound assignment
(`+= -= *= /= %= <<= >>= &= ^= |=`) require a writable place: a `let mut`
binding, a `static mut`, a field or element of a writable place, or an element
or pointee reached through a writable pointer or slice.

### Blocks

`{ statements tail }` is an expression. Its value is the tail expression, or
`()` when there is none. A block whose end is unreachable, for example one that
ends in a `loop` without `break`, has type `never`.

### If

```nia
fun grade(score: i32) &[char] {
    if score >= 90 {
        &"excellent"
    } else if score >= 60 {
        &"pass"
    } else {
        &"fail"
    }
}
```

The condition is a `bool`. Used as a value, an `if` needs an `else` and
compatible branch types. Without `else` its type is `()`.

### Pattern Conditions

`expr is pattern` inside an `if` condition tests one refutable pattern. The
expression is evaluated once, and bindings are visible in the `then` branch
only. `mut` goes inside the pattern: `if maybe is mut ?value { ... }`.

```nia
fun first_or_zero(maybe: ?i32) i32 {
    if maybe is ?value {
        value
    } else {
        0
    }
}
```

Conditions may be chained with `and`. Clauses run left to right and stop at the
first failure; each clause sees the bindings of earlier `is` clauses, and none
of them are visible in `else`. Plain boolean clauses may come before, between,
or after `is` clauses.

```nia
fun ready_width(enabled: bool, maybe: ?Point) i32 {
    if enabled and maybe is ?p and p.x > 0 and not (p.y == 0) {
        p.x
    } else {
        0
    }
}
```

A chain that contains `is` cannot use `or`, cannot be negated
(`not value is pattern`), and cannot be split with parentheses; use `match` for
alternatives. `is` is valid only in `if` conditions.

### Match

```nia
fun code(color: Color) i32 {
    match color {
        Color::Black => 0,
        Color::White => 1,
        _ => return -1,
    }
}
```

`match` tests arms in order and runs the first that matches; there is no
fallthrough. Arms are separated by `,`. An arm body is an expression, a block,
or one of `return expr`, `break`, `continue`, `defer expr` written without `;`.
Arm values must have compatible types, except arms that diverge. See
[Patterns](#patterns) for patterns and exhaustiveness.

### Loops

```nia
fun loops(xs: &[i32]) i32 {
    let mut total = 0;
    for &value in xs {
        total += value;
    }
    for i in 0..3 {
        total += i;
    }
    let mut n = 3;
    while n > 0 {
        n -= 1;
    }
    loop {
        if total > 100 {
            break;
        }
        total += 10;
    }
    total
}
```

Loops are statements and produce no value. `break` leaves the innermost loop
and `continue` starts its next iteration; neither takes a label or a value. A
`loop` without a reachable `break` diverges. A `break` run by a `defer` counts
as an exit; a `break` inside a nested loop or a closure body is not an exit of
the enclosing loop.

`for pattern in expr` requires `expr` to implement `Iterable`, whose `Iter`
type implements `Iterator` with the same `Item`. The loop calls `iter` once
and then `next` until it returns `null`. It uses the [built-in
traits](#built-in-traits) directly, not ordinary method lookup, so inherent
methods named `iter` or `next` do not take part. An `Iterator` is itself
`Iterable`. The loop pattern is irrefutable and takes no type annotation.

The standard library implements `Iterable` for slices (`[T]`, `&[T]`, and
`&mut [T]`, all with item `&T`) and for the ranges `a..b`, `a..=b`, and `a..`.
Arrays are iterated through a slice: `for &x in &arr[..]`. In generic code, a
slice of `T` is iterable when `T: Sized` holds. An item type may itself be an
optional or error union; fallible iteration needs no separate protocol.

### Defer

`defer expr;` registers `expr` to run when the enclosing block exits, whether by
reaching its end, `return`, `break`, `continue`, or `.?` propagation. Defers in
one block run in reverse registration order. The deferred expression must have
type `()` or `never`; discard other values with `_ =`. A block form still ends
with `;`:

```nia
fun deferred() i32 {
    let mut steps = 0;
    {
        defer steps += 1;
        defer {
            steps *= 10;
        };
    }
    steps      // 1: the block runs `steps *= 10` first, then `steps += 1`
}
```

Control flow inside a deferred expression happens when it runs: `return` and
`.?` leave the current function, and `break` or `continue` target a loop that
encloses the `defer` statement. Such a jump replaces the exit that triggered the
deferred code. Nia has no exceptions, so there is no unwinding.

### Operators And Precedence

From lowest to highest precedence:

| Level | Operators | Associativity |
| --- | --- | --- |
| 1 | `=` `+=` `-=` `*=` `/=` `%=` `<<=` `>>=` `&=` `^=` `\|=` | right |
| 2 | `..` `..=` (range) | none |
| 3 | `or` | left |
| 4 | `and` | left |
| 5 | `\|` | left |
| 6 | `^` | left |
| 7 | `&` | left |
| 8 | `==` `!=` | left |
| 9 | `<` `<=` `>` `>=` | left |
| 10 | `<<` `>>` | left |
| 11 | `+` `-` | left |
| 12 | `*` `/` `%` | left |
| 13 | `as` | left |
| 14 | prefix `-` `~` `&` `&mut` `?` `!` | right |
| 15 | postfix: call `f(...)`, `[...]`, `.field`, `.0`, `.*`, `.?`, `::name`, error constructor `value!` | left |

`not` is a prefix operator whose operand extends over equality and tighter
operators: `not a == b` means `not (a == b)`, and `not a and b` means
`(not a) and b`. Comparison operators are left-associative, so `a < b < c` is
`(a < b) < c` and usually a type error.

| Operator | Meaning |
| --- | --- |
| `-x` | numeric negation |
| `~x` | bitwise complement |
| `not x` | boolean negation |
| `&x`, `&mut x` | address of a place (or of a temporary) |
| `x.*` | dereference |
| `?x`, `!x`, `x!` | optional, success, and error constructors |
| `x.?` | [propagation](#propagation) |
| `+ - * / %` | arithmetic |
| `& \| ^ << >>` | bitwise and shifts |
| `== != < <= > >=` | equality and ordering |
| `and`, `or` | short-circuit boolean operators |

There are no `&&`, `||`, or `!x` boolean operators; `!` is reserved for error
unions.

Operators on primitive types are built in. Every operator is also checked
through its [operator trait](#built-in-traits), so user types gain operators by
implementing the trait, and generic code must state the bound it uses:

```nia
fun sum_of[T](a: T, b: T) T
where T: Add[T, Output = T] {
    a + b
}
```

For a user type, `a + b` requires `A: Add[B]` and has type
`[A as Add[B]]::Output`; `a == b` requires `A: Eq[B]` and has type `bool`.

### Casts

`value as Type` converts explicitly. No other numeric or pointer conversion is
implicit. The valid casts are:

| From | To |
| --- | --- |
| any type | the same type |
| integer or floating-point type | any integer or floating-point type |
| numeric vector | numeric vector |
| `char` | `u32` |
| enum | any integer type |
| integer | open enum |
| pointer (`&T`, `&mut T`, `^T`, `^mut T`, `&fun`) | any pointer type |
| pointer | `usize`, `isize` |
| `usize`, `isize` | any pointer type |

`bool` does not cast to integers, integers do not cast to `char` (use
`std::builtin::charFromU32`, which returns `?char`), and integers do not cast to
closed enums. A pointer cast to or from any integer type other than `usize` or
`isize` is an error. The null pointer is `0usize as &T`.

### Calls, Fields, And Methods

`f(args)` calls a function item, function pointer, callable view, or closure.
`value.field` reads a field; `value.method(args)` calls a method resolved
statically from visible `extend` blocks (see [Methods](#methods)). When no
method matches, a field holding a function pointer may be called the same way.

Field access and method receivers dereference pointers automatically: if `p` is
a `&Point` or `&mut Point`, `p.x` and `p.len2()` work. This is the only implicit
dereference.

`expr[...]` is an index, a range index, or an explicit generic instantiation.
It is an instantiation when `expr` names a generic function, method, or type and
the brackets hold type or constant arguments; it is indexing when `expr` is an
array, slice, pointer, or a type implementing `Index`, and the brackets hold one
expression; brackets containing `..` index a range.

### Propagation

`expr.?` unwraps an optional or error union inside a function:

- For `?T`, it yields the payload, or returns `null` from the current function,
  whose return type must be optional.
- For `E!T`, it yields the success value, or returns the error from the
  current function, whose return type must be `F!U`. When `E` and `F` are the
  same type the error is returned unchanged. Otherwise exactly one
  implementation `E: std::builtin::IntoError[F]` must apply, and its
  `intoError` converts the error. Conversions are never chained.

The operand is evaluated once, conversion happens only on the error path, and
nothing is allocated.

```nia
struct ParseError {
    code: i32,
}

struct AppError {
    code: i32,
}

extend ParseError : std::builtin::IntoError[AppError] {
    const fun intoError(self) AppError {
        AppError { code: self.code }
    }
}

fun parse(text: &[char]) ParseError!i32 {
    if text.len() == 0 {
        return ParseError { code: 1 }!;
    }
    !(text.len() as i32)
}

fun run() AppError!i32 {
    let value = parse(&"42").?;
    !(value * 2)
}
```

`IntoError` is declared as:

```nia
pub trait IntoError[Target] {
    const fun intoError(self) Target;
}
```

During constant evaluation, conversion works only when the selected
`intoError` is a `const fun`.

## Closures

A closure is written `\` then an optional capture list, a comma-separated
parameter list, `->`, and one expression:

```nia
fun closures() i32 {
    let offset = 10;
    let mut log = 0;
    let inc = \value: i32 -> value + 1;
    let shift = \[offset] value: i32 -> value + offset;
    let record = \[&mut log] value: i32 -> {
        log.* += value;
        log.*
    };
    inc(1) + shift(2) + record(3)
}
```

The body is a single expression, which may be a block. A closure body extends
as far as possible, so nested closures group to the right; this allows
currying without parentheses.

Captures are explicit:

| Capture | Inside the body |
| --- | --- |
| `[name]` | a copy of the local `name` |
| `[&name]` | a `&T` pointer to `name` (use `.*`) |
| `[&mut name]` | a `&mut T` pointer to `name` (use `.*`) |

Each capture names an existing local or parameter; bind a computed value to a
local first. An empty capture list is omitted. A body may refer only to its
parameters, its own locals, its captures, and module-level items and statics.
Nothing is captured implicitly, including `self`: bind it to a local and
capture that. An inner closure that needs an outer parameter captures it
explicitly:

```nia
fun curried() i32 {
    let make = \x: i32, y: i32 -> \[x, y] z: i32 -> x * y + z;
    let add = make(2, 3);
    add(4)
}
```

### Parameter Inference

A parameter type may be omitted when it can be determined from the closure's
uses in the same function body: an expected callable type, a direct call, or a
later address conversion to a callable view at a function or method call.
Generic callable parameters participate in the same inference. Return types are
inferred from the body and context; a closure has no return annotation. A
closure is monomorphic, so all uses must agree on its parameter types. A
parameter type that appears in neither the body nor any callable context cannot
be inferred and must be annotated.

```nia
fun apply(callback: &Fun(i32) i32, value: i32) i32 {
    callback(value)
}

fun inferred() i32 {
    let double = \value -> value * 2;
    let increment = \value -> value + 1;
    let direct = double(3);
    direct + apply(&increment, 1)
}
```

### Conversion To Views And Function Pointers

A closure is a value with its own anonymous type. It converts in two ways, both
through a `&` or `&mut` expression applied to the closure value while the target
type is expected:

- To a callable view `&Fun(...)` or `&mut Fun(...)`. `&closure` gives only a
  read-only view; `&mut closure` gives either kind. Parameter and return types
  must match.
- To a thin `&fun(...)` pointer, only for a closure without captures and only
  with `&closure`.

Storing a callable view first and converting it later is not a conversion. A
closure with captures cannot become a `&fun(...)`.

```nia
fun views() i32 {
    let base = 5;
    let add_base = \[base] value: i32 -> base + value;
    let view: &Fun(i32) i32 = &add_base;

    let increment = \value: i32 -> value + 1;
    let pointer: &fun(i32) i32 = &increment;

    view(1) + pointer(2)
}
```

### Lifetime Of Views

Views and closure state are stack-backed. A view may be called and copied into
local aggregates while the closure is live, but it cannot be returned, stored
through a pointer or in a static, assigned to an outer scope, or passed to an
operation that may retain it. The same holds for a closure whose state captures
the address of a local or temporary. A longer-lived callable needs an explicit
owner from a library.

## Constant Evaluation

Constant evaluation runs at compile time for `const` initializers, array
lengths, repeat counts, range-pattern endpoints, const generic arguments, and
calls made from those contexts. It uses the same types and values as runtime
code: nominal structs, arrays, tuples, enums, and unions, with
target-dependent sizes taken from the [ABI](abi.md). Reads of uninitialized
storage and invalid representations are diagnosed.

`const fun` marks a function callable during constant evaluation; it is also an
ordinary runtime function. A constant context may call `const fun`s but not
ordinary `fun`s. Each call is staged by its context:

```nia
const fun double(value: usize) usize {
    value * 2
}

const table_len: usize = double(5);    // evaluated at compile time

fun runtime_double(value: usize) usize {
    double(value)                       // an ordinary runtime call
}
```

A `const fun` is checked when declared, whether or not it is used: every branch
must use only const-capable operations, even branches a particular call never
takes. Inside a `const fun`, `let mut` locals, loops, aggregates, and
destructuring work as at runtime, and each call has fresh locals:

```nia
const fun count_up(limit: usize) usize {
    let mut value: usize = 0;
    while value < limit {
        value += 1;
    }
    value
}

const fun shifted_sum(point: Point) i32 {
    let mut Point { x, y }: Point = point;
    x += 1;
    x + y
}
```

Pointers in constant evaluation follow these rules:

- A pointer to a local is valid while the local is live and cannot escape the
  call in the resulting constant.
- A pointer received from the caller may be returned when its target outlives
  the callee.
- A pointer to a temporary may be passed to a nested call while the temporary
  is live. A read-only value addressed in a `const` initializer is promoted to
  storage that lasts as long as the initializer; writable promotion is an error.
- Pointers are never converted to host addresses, and integers never become
  pointers.

Calls through function pointers or callable views are not constant
evaluation. In a `const fun`, a `for` loop requires the selected `Iterable::iter`
and `Iterator::next` implementations to be `const fun`.

`std::builtin::trap()` returns `never`; reached during constant evaluation it is
a compile-time error at the call, and at runtime it traps.
`std::builtin::error(message)` makes `message` a compile-time error when it is
reached. Either may appear in a branch that a particular evaluation does not
take. `error` is const-only: a `const fun` that contains it cannot be called
from runtime code. The
layout builtins `size`, `align`, and `offset` are constants when their type
argument is concrete and can be used in array lengths and static initializers;
`std::builtin::embed` is valid only in constant evaluation. See the
[builtins reference](builtins.md).

```nia
const fun checked_width(width: i32) i32 {
    if width < 0 {
        std::builtin::error("width must not be negative");
    }
    width
}

const width: i32 = checked_width(4);
```

Integer arithmetic in constant evaluation follows the
[trap rules](#unchecked-operations-and-traps), reporting a compile-time error
where runtime code would trap. Each intermediate result is checked in its
concrete type, so `i32::MAX + 1 - 1` is an error. `usize` and `isize` have the
width of the artifact target, not of the compiler host.

An implementation may limit the resources a constant evaluation uses; reaching a
limit is reported as an error and never changes a result.

## Generics

Functions, structs, unions, type aliases, and traits take generic parameters in
brackets after the name, and `extend` takes them after the keyword
(`extend[T] Box[T]`). Enums are not generic.

```nia
fun identity[T](x: T) T {
    x
}

struct Pair[T, U] {
    first: T,
    second: U,
}

fun length[T, N: usize](values: [T; N]) usize {
    _ = values;
    N
}

struct Buffer[N: usize] {
    data: [u8; N],
}
```

A parameter written `Name` is a type parameter. A parameter written
`Name: Type` is a const parameter, a compile-time value of that type usable in
types and expressions; array lengths use `usize` const parameters. Generic
parameters exist only at compile time.

Generic types are instantiated with brackets directly after the name:
`Pair[i32, bool]`, `Buffer[64]`. Calls may infer every argument or list all of
them; a partial list is an error, and `_` asks for inference in one position:

```nia
fun choose[T, U](value: T, other: U) T {
    _ = other;
    value
}

fun generic_calls() usize {
    let a = choose(1, true);              // all inferred
    let b = choose[i32, _](2, false);     // `U` inferred
    let c = length[_, _]([1, 2, 3]);
    _ = a + b;
    c + length([4, 5])
}
```

Requirements on parameters go in a `where` clause after the signature or type
header. Predicates are separated by commas, and several bounds on one parameter
are joined with `+`:

```nia
fun describe[T, U](value: &T, other: &U) bool
where T: Ord[U] + Show, U: Eq[U] {
    _ = other;
    value.show() == 0
}
```

Generic code may use only what its bounds provide, including operators (see
[Traits](#traits)). `std::builtin::size[T]()` and `align[T]()` need
`T: Sized`. A method's generic parameters must not reuse a name declared by the
enclosing trait or `extend` block.

## Methods

An `extend` block attaches associated functions, methods, and associated
constants to a type:

```nia
struct Vec2 {
    x: i32,
    y: i32,
}

extend Vec2 {
    const ZERO: Vec2 = Vec2 { x: 0, y: 0 };

    pub fun new(x: i32, y: i32) Self {
        Self { x, y }
    }

    pub fun len2(&self) i32 {
        self.x * self.x + self.y * self.y
    }

    pub fun shift(&mut self, dx: i32) {
        self.x += dx;
    }

    pub fun into_x(self) i32 {
        self.x
    }
}
```

| First parameter | Receiver passed as |
| --- | --- |
| `&self` | `&Self` |
| `&mut self` | `&mut Self` |
| `self` | `Self` by value |
| none | associated function, called as `Type::name(...)` |

`Self` names the extended type inside the block (and the implementing type
inside a trait). `extend` blocks contain `fun`, `const fun`, and `const` members;
inherent blocks cannot define associated types, and `let` members are not
allowed. Visibility is written on each member.

Method calls resolve statically; `.` never implies dynamic dispatch except on
[trait objects](#trait-object-coercion-and-dispatch). Candidates are the
methods of visible `extend` blocks for the receiver type: public methods from
modules reached through `using`, and private methods in their own module. Two
visible methods with the same name for the same receiver type make a call
ambiguous.

A nominal type also exposes public inherent members declared in its defining
module, including through a qualified type path. This does not bring that
module's imports or extensions on other targets into scope.

```nia
fun methods() i32 {
    let mut v = Vec2::new(3, 4);
    v.shift(1);
    let by_path = Vec2::len2(&v);         // receiver passed explicitly
    let f: &fun(&Vec2) i32 = &Vec2::len2;  // unbound method as a function pointer
    by_path + f(&v) + Vec2::ZERO.into_x()
}
```

### Extension Targets

The target may be any visible type, including an imported one
(`extend math::Point { ... }`) and structural types such as pointers, slices,
arrays, and ranges. Generic targets declare their parameters after `extend`:

```nia
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fun get(&self) T {
        self.value
    }

    fun swap_in[U](&self, other: U) U {
        other
    }
}

extend[T] &T {
    fun is_null(self) bool {
        self as usize == 0
    }
}

fun boxes() bool {
    let b = Box[i32] { value: 1 };
    let flag = b.swap_in[bool](true);
    let p: &fun(&Box[i32], bool) bool = &Box[i32]::swap_in[bool];
    _ = p(&b, flag);
    [&u8]::is_null(0usize as &u8)
}
```

A structural target is named in a path by wrapping it in brackets:
`[&u8]::is_null`, `[&[char]]::name`. This is only a path to the target's
members; it neither captures a receiver nor creates a short name.

Implementations of source-defined traits are package-owned facts. An
implementation for a nominal type may be declared only in the package that
defines the type or in the package that defines the trait. Primitive and
structural types have no defining package, so their implementations must be
declared in the trait's package. The compiler searches those owner packages
directly; a `using` declaration does not change which implementations exist or
make a third package eligible. Compiler-defined [built-in traits](#built-in-traits)
follow the language's intrinsic-implementation rules.

When two implementation headers overlap, a strictly more-specific header may
specialize the general one and is selected for matching types. A header is
more specific when every type and const-argument combination it matches also
matches the general header, but the reverse does not hold. Declaration order
does not affect selection. Duplicate unconditional headers are rejected.
Conditional implementations are filtered by their `where` requirements, and
multiple applicable candidates without a unique most-specific header are
ambiguous. Ordinary inherent extensions and
extension methods remain scoped by `using` as described above.

### Contextual Receiver Inference

When a call chain starts with an omitted associated call, or contains a
generic intermediate result, the expected type of the whole chain can determine
the receiver type:

```nia
struct Builder {
    width: i32,
}

struct Product {
    width: i32,
}

extend Builder {
    fun init() Builder {
        Builder { width: 1 }
    }

    fun finish(self) Product {
        Product { width: self.width }
    }
}

fun build() i32 {
    let product: Product = .init().finish();
    product.width
}
```

This applies only when the receiver cannot be typed on its own. The candidate
methods with the requested names, from visible `extend` blocks and trait
implementations, must together identify exactly one receiver type that satisfies
the exact expected result, the arguments, and all bounds; otherwise the chain is
ambiguous and needs an annotation. The inference uses type equality, not
coercions, does not invert associated-type projections, and stays within one
expression.

## Traits

A trait declares methods, associated types, and associated constants that
implementing types provide:

```nia
trait Show {
    fun show(&self) i32;
}

trait Comparable {
    fun same(&self, other: &Self) bool;

    fun different(&self, other: &Self) bool {
        not self.same(other)
    }
}
```

A method with a body is a default; an implementation that omits it uses the
default with `Self` set to the implementing type. Trait members take no
visibility.

`extend Type : Trait { ... }` implements a trait. The block contains only
members of that trait: every required method and every associated type and
constant the trait declares, and nothing else. Inherent methods go in a plain
`extend Type { ... }` block.

```nia
struct Point {
    x: i32,
}

extend Point : Show {
    fun show(&self) i32 {
        self.x
    }
}

extend Point : Comparable {
    fun same(&self, other: &Point) bool {
        self.x == other.x
    }
}
```

### Supertraits

`trait Child : Parent + Other { ... }` declares supertraits. A bound
`T: Child` also makes the parent methods available, and default methods of
`Child` may call them. Implementing `Child` does not implement `Parent`; each
implementation is written separately.

```nia
trait Ordered : Comparable {
    fun less(&self, other: &Self) bool;
}

extend Point : Ordered {
    fun less(&self, other: &Point) bool {
        self.x < other.x
    }
}

fun distinct_and_less[T](a: &T, b: &T) bool
where T: Ordered {
    a.different(b) and a.less(b)
}
```

### Associated Types

An associated type is an output chosen by each implementation. It is projected
with the explicit form `[T as Trait]::Name`; there is no `T::Name` shorthand.
Associated types take no generic parameters and are defined only in trait
implementations.

```nia
trait Source {
    type Item;

    fun get(&self) [Self as Source]::Item;
}

trait Mapper[A, B] {
    type C;
    type D;

    fun map(&self, a: A, b: B) [Self as Mapper[A, B]]::C;
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    type Item = i32;

    fun get(&self) i32 {
        self.value
    }
}

fun read_any[T](source: &T) [T as Source]::Item
where T: Source {
    source.get()
}
```

A bound may fix associated types after the positional trait arguments:
`T: Add[T, Output = T]`, `T: Mapper[i32, bool, C = i32, D = bool]`. Each name must
be an associated type of the trait and may be bound once. The binding is part
of that bound, not a separate equality.

```nia
fun mapped[T](value: &T) i32
where T: Mapper[i32, bool, C = i32, D = bool] {
    value.map(1, true)
}
```

### Associated Constants

A trait declares `const NAME: Type;` without a value, and each implementation
supplies `const NAME: Type = value;`. Inherent `extend` blocks may also define
associated constants. They are read as `Type::NAME`.

```nia
trait Lanes {
    const COUNT: usize;
}

extend Point : Lanes {
    const COUNT: usize = 1;
}
```

### Resolution

In generic code a trait method call is resolved through the bound that
provides it; for a concrete instantiation the selected implementation supplies
the body and the associated types. User implementations may use strict
specialization, but they may not overlap the intrinsic implementations the
language provides for primitive and structural types, such as equality on
pointers.

### Trait Object Coercion And Dispatch

A pointer to a concrete type converts to a trait object pointer (`&Trait`,
`&mut Trait`, `&Trait[Item = i32]`) when the type implements the trait with
matching associated-type bindings. Calls through a trait object dispatch
dynamically. Methods callable on an object must take a `&self` or `&mut self`
receiver and have no generic parameters of their own. Built-in traits cannot be
used as objects.

```nia
fun total(source: &Source[Item = i32]) i32 {
    source.get()
}

fun dispatch() i32 {
    let counter = Counter { value: 3 };
    total(&counter)
}
```

A trait object converts to an object of a declared supertrait with the same
mutability. If the target binds associated types, the source must carry the
same bindings; bindings for supertraits other than the first use projection
keys, as in `&Child[[Self as A]::Item = i32, [Self as B]::Item = usize]`.

```nia
trait Parent {}
trait Child : Parent {}

fun accept(parent: &Parent) {
    _ = parent;
}

fun forward(child: &Child) {
    accept(child)
}
```

### Built-In Traits

The language gives these traits fixed meanings. They are declared in
`std::builtin`, and all except `IntoError` are visible by name in every module
without an import; `IntoError` is written `std::builtin::IntoError`. They are implemented
with ordinary `extend Type : Trait` blocks. Their method names are fixed.

Operator traits. Binary operators take both operands by value:

| Expression | Trait | Method | Result |
| --- | --- | --- | --- |
| `a + b` | `Add[Rhs]` | `add(self, rhs: Rhs)` | `Output` |
| `a - b` | `Sub[Rhs]` | `sub` | `Output` |
| `a * b` | `Mul[Rhs]` | `mul` | `Output` |
| `a / b` | `Div[Rhs]` | `div` | `Output` |
| `a % b` | `Rem[Rhs]` | `rem` | `Output` |
| `a & b` | `BitAnd[Rhs]` | `bitAnd` | `Output` |
| `a \| b` | `BitOr[Rhs]` | `bitOr` | `Output` |
| `a ^ b` | `BitXor[Rhs]` | `bitXor` | `Output` |
| `a << b` | `Shl[Rhs]` | `shl` | `Output` |
| `a >> b` | `Shr[Rhs]` | `shr` | `Output` |
| `-a` | `Neg` | `neg(self)` | `Output` |
| `~a` | `BitNot` | `bitNot(self)` | `Output` |
| `not a` | `Not` | `logicalNot(self)` | `bool` |
| `a == b`, `a != b` | `Eq[Rhs]` | `eq(self, rhs: Rhs)`, `ne` | `bool` |
| `a < b`, `<=`, `>`, `>=` | `Ord[Rhs]` | `lt`, `le`, `gt`, `ge` | `bool` |

```nia
struct Meters {
    value: i32,
}

extend Meters : Add[Meters] {
    type Output = Meters;

    fun add(self, rhs: Meters) Meters {
        Meters { value: self.value + rhs.value }
    }
}

extend Meters : Eq[Meters] {
    fun eq(self, rhs: Meters) bool {
        self.value == rhs.value
    }

    fun ne(self, rhs: Meters) bool {
        self.value != rhs.value
    }
}

fun same_length(a: Meters, b: Meters) bool {
    a + b == b + a
}
```

Place traits:

```nia
trait Deref {
    type Target;
    fun deref(&self) &[Self as Deref]::Target;
}

trait DerefMut : Deref {
    type Target;
    fun derefMut(&mut self) &mut [Self as DerefMut]::Target;
}

trait Index[R] {
    type Output;
    fun index(&self, index: R) &[Self as Index[R]]::Output;
}

trait IndexMut[R] : Index[R] {
    type Output;
    fun indexMut(&mut self, index: R) &mut [Self as IndexMut[R]]::Output;
}

trait Slice[R] {
    type Output;
    fun slice(&self, range: R) [Self as Slice[R]]::Output;
}

trait SliceMut[R] : Slice[R] {
    type Output;
    fun sliceMut(&mut self, range: R) [Self as SliceMut[R]]::Output;
}
```

`value.*` uses `Deref`/`DerefMut` for non-pointer types, `value[i]` uses
`Index`/`IndexMut`, and `&value[range]` uses `Slice`/`SliceMut`. The mutable
traits declare their own associated type, so an implementation defines it in
both blocks. For arrays, pointers, and slices the built-in implementations take
integer indexes and ranges with `usize` bounds.

Other built-in traits:

| Trait | Meaning |
| --- | --- |
| `Sized` | the type has a known size; required by `size[T]()` and `align[T]()` |
| `Unsized` | marker for types without a fixed size |
| `Len` | `const fun len(&self) usize`; implemented for arrays and slices, so `xs.len()` works on both |
| `Iterable`, `Iterator` | the [`for` loop](#loops) protocol (below) |
| `IntoError[Target]` | error conversion for [`.?`](#propagation) |

```nia
trait Iterable {
    type Item;
    type Iter;
    fun iter(&self) [Self as Iterable]::Iter;
}

trait Iterator {
    type Item;
    fun next(&mut self) ?[Self as Iterator]::Item;
}
```

## Modules And Visibility

Each `.nia` file is one module. Modules are loaded explicitly: `module name;`
declares a child module of the current one, and `using` only opens names that
the module graph already contains; it never discovers files. How a child
declaration maps to a file is defined by the toolchain; see
[Build Scripts](../guide/build-scripts.md).

```nia
module math;            // private child module
pub module geometry;    // child module that is part of this module's public surface
```

### Paths

Paths separate segments with `::`. These roots are reserved:

| Root | Names |
| --- | --- |
| `entry` | the package being compiled |
| `pkg` | the package that contains the current module |
| `std` | the standard library |
| `self` | the current module |
| `super` | the parent module |

Other package names are supplied by the toolchain.

### Using

`using` brings a module, item, or enum variant into scope. At top level it is
visible in the whole file; in a block, only in that block.

```nia
using std::process;                       // a module
using entry::math as m;                   // renamed
using math::add;                          // one item
using math::{add, sub as minus};          // a group with renames
using math::*;                            // the module's public surface
using palette::Color::Red;                // one enum variant
using palette::Color::*;                  // every variant
using {math, palette::Color};             // a group of paths
using entry::a::{b::c::f, d::{g, h::Kind::*}};  // nested groups
```

`using module::*` imports every public item of the module, its public child
modules, and everything it re-exports. `using Enum::*` imports every variant.
Importing the same name twice into one namespace and scope is an error,
whether by name or by wildcard. Imported items enter the namespace of their
kind: functions, statics, constants, and enum variants are values; structs,
unions, enums, traits, and type aliases are types.

### Re-Exports

`pub using` at module top level makes the selected names part of the current
module's public surface. It accepts the same forms as `using`. Users may then
reach an item through the re-exporting module (`facade::add`) or through a
re-exported module (`facade::impl::add`). Re-exports may chain.

```nia
// facade.nia
using entry::impl;
pub using impl;                  // the module itself
pub using impl::{frob as run};   // one item, renamed
pub using impl::Color::*;        // every variant
```

### Visibility

Items are private to their module unless marked:

| Modifier | Visible to |
| --- | --- |
| none | the declaring module |
| `pub(super)` | the parent module and its descendants |
| `pub(pkg)` | the whole current package |
| `pub` | every module that can name it |

Visibility applies to `module`, `using` (re-export), `fun`, `const fun`,
`extern fun`, `struct`, `union`, `enum`, `trait`, `type`, `static`,
`extern static`, and `const` items, and to `fun` and `const` members of
`extend` blocks. Fields, trait members, and associated type definitions take
no visibility. Statements cannot be `pub`.

## Unchecked Operations And Traps

Nia leaves some operations unchecked and traps on others. A trap terminates the
program through the target's trap instruction; during constant evaluation the
same condition is a compile-time error.

Unchecked: the program must guarantee validity, and violating it is undefined.

- Array, slice, and pointer indexing and slicing, including an explicit range
  on a pointer base.
- Dereferencing a pointer, including one produced by a cast or from an integer.
- Reading a union field other than the one last written.
- Reading uninitialized storage (`let x: T;`).
- Letting a pointer outlive the storage it points to.

Trapping at runtime:

| Operation | Traps when |
| --- | --- |
| integer `+`, `-`, `*`, unary `-` | the mathematical result does not fit the type |
| integer `/`, `%` | the divisor is zero, or a signed operation is `MIN / -1` or `MIN % -1` |
| `<<`, `>>` | the count is negative or at least the bit width of the left operand |
| `<<` | the shifted result does not fit the type |
| `std::builtin::extract`, `insert` | the lane index is out of range |
| `std::builtin::trap()` | always |

Right shift is arithmetic for signed left operands and logical for unsigned
ones; the count's type affects neither the result type nor the mode. For
integer vectors each rule applies per lane, and any failing lane traps the
whole operation. Floating-point arithmetic follows IEEE-754 and never traps.

## Pointers At The C Boundary

String literals are arrays, not pointers, and there is no implicit array-to-
pointer decay. To pass a C string, write the terminating `\0` and pass a pointer
to the first byte:

```nia
extern fun puts(text: &u8) i32;

fun greet() {
    _ = puts((&b"hello\0").ptr());
}
```

`(&array).ptr()` converts the array pointer to a slice and returns its data
pointer; `&bytes[0]` is equivalent. A literal evaluated inside a function gets
no static storage, so a pointer into it must not outlive the enclosing block.
Use a `static` for data that foreign code keeps.

The types that may cross the boundary, layout, calling conventions, and symbol
names are specified in the [ABI reference](abi.md#the-c-abi-boundary). The
[C interop guide](../guide/c-interop.md) covers declaring, linking, and
exporting functions.
