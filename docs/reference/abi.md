# Nia ABI Reference

This reference describes how Nia values are represented in memory, how
functions and data are named in object files, and which values may cross the C
ABI boundary. The syntax of `extern` declarations is defined in the
[language reference](language.md#declarations); a practical walkthrough is in
[C interop](../guide/c-interop.md).

## ABI Domains

Nia has two ABI domains:

| Domain | Used by | Layout and calling convention | Symbol names |
| --- | --- | --- | --- |
| Nia ABI | ordinary functions, structs, unions, enums, globals | chosen by the compiler; struct fields may be reordered | internal, compiler-generated |
| C ABI | `extern fun`, `extern struct`, `extern union`, `extern static` | the C ABI of the artifact target | the source name, or `linkName` / `exportName` |

```nia
// Nia ABI: fields may be reordered, the symbol name is internal.
struct Pair {
    a: u8,
    b: i64,
}

fun sum(pair: Pair) i64 {
    pair.b + pair.a as i64
}

// C ABI: declaration-order layout, C calling convention, symbol `log_message`.
extern fun log_message(message: &u8);

extern struct CPoint {
    x: i32,
    y: i32,
}
```

The domains are separate. An ordinary `struct` never promises C layout, `pub`
never implies C layout or an unmangled symbol, and an `extern struct` never
receives Nia field reordering. Crossing from one domain to the other is always
explicit: the compiler rejects a Nia-only representation at a C boundary
rather than adapting it.

## Stability

The C ABI boundary is the only stable binary interface. Its rules follow the C
ABI of each target and do not change between compiler versions.

The Nia ABI is unstable. Layouts, parameter passing, and symbol names described
for Nia-owned types in this document are informative: they explain what the
compiler does, but programs must not depend on them, and objects produced by
different compiler versions cannot be linked together. Within one compiler
version, every module of a program uses the same Nia ABI.

Use `std::builtin::size[T]()`, `std::builtin::align[T]()`, and
`std::builtin::offset[T]("field")` to query a layout instead of hard-coding it.

Optimization never changes layout, ABI classification, or symbol identity. Every
optimization level produces the same sizes, alignments, field offsets,
parameter and return passing, static data layout, and symbol names.

## Target Data Model

Layout is computed from the target, never from the host. Nia has no C `long`
type, so the difference between the LP64 (Linux, macOS) and LLP64 (Windows)
C data models does not affect any Nia type; it only affects how C's `long`
maps to Nia (see [C type mapping](../guide/c-interop.md#c-and-nia-type-mapping)).

| Target | C data model | Pointer and `usize` | `i64`/`u64`/`f64` alignment |
| --- | --- | --- | --- |
| `x86_64-unknown-linux` | LP64 | 8 bytes | 8 |
| `aarch64-unknown-linux` | LP64 | 8 bytes | 8 |
| `x86_64-apple-macos` | LP64 | 8 bytes | 8 |
| `aarch64-apple-macos` | LP64 | 8 bytes | 8 |
| `x86_64-pc-windows-msvc` | LLP64 | 8 bytes | 8 |
| `x86-unknown-linux` | ILP32 (System V i386) | 4 bytes | 4 |

Nia-owned and `extern` aggregates use the same primitive alignments, so a
scalar has the same placement in both.

## Primitives

| Type | Size | Alignment |
| --- | --- | --- |
| `i8`, `u8`, `bool` | 1 | 1 |
| `i16`, `u16` | 2 | 2 |
| `i32`, `u32`, `f32`, `char` | 4 | 4 |
| `i64`, `u64`, `f64` | 8 | 8, or 4 on `x86-unknown-linux` |
| `i128`, `u128` | 16 | 16 |
| `isize`, `usize` | pointer size | pointer alignment |
| `()` | 0 | 1 |
| `never` | 0 | 1 |

`()` is a zero-sized type (ZST) with one value. `never` has no values; its
zero-sized layout only lets diverging expressions be handled uniformly.
Function items (a function name used as a value before `&` is applied) are also
zero-sized.

Crosses the C boundary: integers and floats yes; `bool` and `char` no (use an
integer type); `i128`/`u128` yes except on `x86-unknown-linux`, whose C ABI has
no 128-bit integer; `()` only as a return type; `never` no.

## Pointers

Every thin pointer is one pointer-sized scalar:

```text
&T  &mut T  ^T  ^mut T  &opaque  &mut opaque  ^opaque  ^mut opaque
```

`&opaque` erases the pointee type; it cannot be dereferenced, and erasure is
an explicit cast (`&value as &opaque`). The null pointer is written as a cast
from zero, such as `0usize as &u8`.

Volatile pointers `^T` and `^mut T` have the same representation as ordinary
pointers. Volatility is a property of each access: loads and stores through
them are volatile memory operations. It does not change size, alignment, or
parameter passing.

Crosses the C boundary: yes, including volatile pointers. The compiler does not
inspect the pointee, so a pointer to a Nia-owned type is accepted; it is only
meaningful if the C side treats it as an opaque handle.

## Function Pointers

Function pointers are pointer-sized scalars. The calling convention is part of
the type:

| Type | Produced by | Convention |
| --- | --- | --- |
| `&fun(A) R` | `&name` of an ordinary function | Nia |
| `&extern fun(A) R` | `&name` of an `extern fun` | target C |

A function pointer type consists of its parameter types, return type, variadic
marker, and convention. Functions marked `@[trackCaller]` cannot be converted to
function pointers.

Crosses the C boundary: `&extern fun(...)` yes; `&fun(...)` no; variadic function
pointer types no (declare the variadic function directly with `extern fun`).

## Callable Views And Closures

A closure value is an anonymous closure state whose fields are its captured
values, laid out in capture order. `Fun(A) R` is unsized and has no value
layout.

`&Fun(A) R` and `&mut Fun(A) R` are two-word, non-owning views:

```text
callable view = { state pointer, entry pointer }
```

Both words are pointer-sized, so a view is 16 bytes with 8-byte alignment on a
64-bit target. A view never allocates, frees, or extends the lifetime of the
closure state. A pointer to a view, such as `&mut &Fun(i32) i32`, is a thin
pointer to the 16-byte view.

Crosses the C boundary: no. Pass an `&extern fun(...)` together with an explicit
context pointer instead.

## Arrays

`[T; N]` stores `N` elements contiguously:

```text
size  = N * size(T)
align = align(T)
```

Arrays of zero-sized elements have size 0.

Crosses the C boundary: not by value. Arrays are allowed as `extern struct`
fields; pass a pointer to the first element (`&array[0]`) as a parameter.

## Slices

`&[T]` and `&mut [T]` are two-word descriptors:

```text
slice = { pointer to first element, usize element count }
```

A slice is 16 bytes with 8-byte alignment on a 64-bit target. The fields are
not source-level members; use `.ptr()` and `.len()`.

Trait object pointers (`&Trait`) are also two words: a data pointer and a
vtable pointer.

Crosses the C boundary: no. Pass `slice.ptr()` and `slice.len()` as separate
arguments.

## Optionals And Error Unions

Optionals and error unions are tagged aggregates: a one-byte tag followed by
payload storage aligned for the largest payload.

```text
?T   = { tag: u8, payload: T }
E!T  = { tag: u8, payload: storage for E or T }
```

There is no null-pointer niche: `?&T` is a tag plus a pointer (16 bytes on a
64-bit target), not a nullable pointer.

Crosses the C boundary: no. A nullable C pointer is an ordinary pointer that
is compared with `0usize as &T`.

## Structs

An ordinary `struct` uses Nia layout. Source field order is the declaration
order used by names, initializers, and diagnostics; physical order is chosen by
the compiler. The algorithm is informative and may change:

```text
sort fields by:
  1. descending alignment
  2. descending size
  3. ascending declaration index
place fields in that order with alignment padding;
round the size up to the largest field alignment
```

For example, `struct Mixed { a: u8, b: i64, c: u8 }` places `b` at offset 0,
`a` at 8, and `c` at 9, for a size of 16. Tuple structs and tuples keep
declaration order.

An `extern struct` uses C layout: fields stay in declaration order, each
aligned to its C alignment, with the size rounded up to the largest field
alignment. The same fields declared as `extern struct CMixed` have offsets 0,
8, and 16 and a size of 24 on x86_64. There is no packed or over-aligned
struct form.

`extern struct` fields must themselves be C-compatible: integers, floats,
pointers, `&extern fun(...)`, nested `extern struct` values, and fixed-length
arrays of those.

Crosses the C boundary: `extern struct` yes, by value or by pointer; ordinary
`struct` and tuples no by value.

### Empty Structs And Zero-Sized Fields

An empty struct is a ZST with size 0 and alignment 1. Zero-sized fields
consume no bytes, and a struct whose fields are all zero-sized is itself
zero-sized.

Crosses the C boundary: no; an empty aggregate has no portable C
representation.

## Unions

Unions are untagged. Every field is at offset 0:

```text
size  = max(size(field)) rounded up to align
align = max(align(field))
```

Empty unions are not part of Nia. `extern union` uses the C union layout of
the target, which is the same rule.

Crosses the C boundary: not by value, including `extern union`. Pass a pointer
to an `extern union`.

## Enums

A unit-only enum is represented exactly as its backing integer type (`i32` if
none is written). A payload-bearing enum is a tagged aggregate: the backing
integer tag followed by payload storage aligned for the largest variant, with
each variant's fields in declaration order.

Taking the address of a payload variant, such as `&Event::Data`, yields an
ordinary `&fun(...)` pointer whose parameters are the payload fields.

Crosses the C boundary: no. Pass the backing integer and convert explicitly.

## SIMD Vectors

A vector type such as `u8x16` stores its lanes at their native bit width;
boolean mask lanes use one bit each. The store width is the total lane width
rounded up to whole bytes; alignment is the next power of two of that width,
and the size is the store width rounded up to the alignment. `u8x16` is 16
bytes and `boolx16` is 2 bytes.

Crosses the C boundary: not by value. Pass a pointer.

## The C ABI Boundary

Every `extern fun` declaration or definition, `extern static`, and
`extern struct` field is a C ABI boundary. The compiler validates each type at
the boundary and rejects anything without a C representation:

| At the boundary | Status | Use instead |
| --- | --- | --- |
| integers, `f32`, `f64` | allowed | |
| `i128`/`u128` on `x86-unknown-linux` | rejected | two 64-bit integers |
| pointers, volatile pointers, `&opaque` | allowed | |
| `&extern fun(...)` | allowed | |
| `extern struct` by value | allowed | |
| `()` return | allowed | |
| `()` parameter, tuples | rejected | `extern struct` or pointer |
| `bool` | rejected | `u8` or `i32` |
| `char` | rejected | `u32` |
| `never` return | rejected | `()` |
| ordinary or empty `struct` by value | rejected | `extern struct` or pointer |
| union by value, including `extern union` | rejected | pointer to `extern union` |
| enum by value | rejected | backing integer |
| array by value (outside a struct field) | rejected | pointer to first element |
| slice, trait object, callable view | rejected | pointer plus length or context |
| optional, error union, range | rejected | explicit `extern struct` |
| SIMD vector by value | rejected | pointer |
| `&fun(...)` (Nia convention) | rejected | `&extern fun(...)` |
| variadic function pointer type | rejected | direct `extern fun` declaration |
| generic `extern fun` | rejected | concrete signatures |
| variadic `extern fun` with a body | rejected | bodyless declaration |
| variadic `extern fun` without a fixed parameter | rejected | add a fixed parameter |
| `extern fun` inside `extend` (method) | rejected | free function |
| `extern static` without a type | rejected | write the type |

### Calling Conventions

Every C boundary uses the C calling convention of the artifact target, as clang
implements it:

| Target | Convention |
| --- | --- |
| `x86_64-unknown-linux`, `x86_64-apple-macos` | System V AMD64 |
| `x86-unknown-linux` | System V i386 |
| `x86_64-pc-windows-msvc` | Microsoft x64 |
| `aarch64-unknown-linux` | AAPCS64 |
| `aarch64-apple-macos` | Apple arm64 (AAPCS64 variant) |

The convention classifies each argument and the result from its C byte layout.
Small aggregates travel in registers, larger ones by hidden pointer, and small
integers receive the sign or zero extension the convention requires. Calls,
declarations, and `extern fun` definitions use one classification, so both
sides of a boundary agree.

Arguments beyond the fixed parameters of a variadic declaration receive C's
default argument promotions: integers narrower than 32 bits widen to 32 bits
with the extension of their own signedness, and `f32` widens to `f64`.

### Executable Exports

An executable's C interface is every top-level `pub extern fun` definition of
its entry package. Those definitions are compiled and kept even when no Nia
code calls them, so linked foreign code can call them by name. Other
functions, including non-`pub` `extern` definitions, are compiled only when
reachable.

The symbol `_start` is reserved for the toolchain's process entry; a program
cannot export it.

## Static Data

Top-level `static` and `static mut` declarations use the representation of
their type. Struct initializers are written in source order and stored in
physical order; padding is not addressable. A zero-sized static needs no
storage.

`extern static` and `extern static mut` declare data defined elsewhere, under
the source name or `@[linkName("...")]`. Their type must be written and must
be C-compatible. Nia cannot export a static: `@[exportName]` applies only to
function definitions.

## Symbols

| Item | Symbol |
| --- | --- |
| function, method, global | internal Nia name |
| generic function or method instance | internal Nia name including the type arguments |
| bodyless `extern fun`, `extern static` | the source name, or `@[linkName("...")]` |
| `extern fun` definition | the source name, or `@[exportName("...")]` |

Internal Nia names are deterministic for a given compiler version and contain
no version, layout, or calling-convention information. Their format is not part
of the language. There is no attribute that gives an ordinary function a C
name; write an `extern fun` definition instead.

Only `extern` symbols are visible outside the linked image. Every internal
symbol, including generic instances, closure entries, and vtables, has hidden
visibility: it links between the objects of one program but is never exported
from a shared object or preempted. A program's external interface is therefore
exactly its `extern` items.

Objects use the platform's position-independent code model on Linux and macOS.
Data that holds addresses, such as vtables, is placed where load-time
relocation can write it, so Nia objects link into position-independent
executables and shared objects without text relocations.

## Inline Assembly

Inline assembly operands use the ABI representation of their types. See
[`std::builtin::asm`](builtins.md) for the operand rules.
