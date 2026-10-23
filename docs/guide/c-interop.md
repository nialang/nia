# C Interop

Nia calls C and C calls Nia through `extern` items, which use the C ABI of the
target. This guide walks through the common patterns with one small C library.
The exact rules are in the [ABI reference](../reference/abi.md#the-c-abi-boundary)
and the declaration syntax is in the
[language reference](../reference/language.md#declarations).

The toolchain does not link a C runtime or libc. A C library is a native
dependency that you link explicitly, like any other.

## The C Side

The examples below call this file, `geom.c`. It uses only freestanding headers,
so it needs no C runtime:

```c
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>

struct Point { int32_t x, y; };

struct Point point_scale(struct Point p, int32_t factor) {
    struct Point r = { p.x * factor, p.y * factor };
    return r;
}
void point_translate(struct Point *p, int32_t dx, int32_t dy) { p->x += dx; p->y += dy; }
int32_t divide(int32_t a, int32_t b, int32_t *quotient, int32_t *remainder) {
    if (b == 0) return 0;
    *quotient = a / b; *remainder = a % b; return 1;
}
size_t byte_length(const char *s) { size_t n = 0; while (s[n]) n++; return n; }
uint32_t sum_bytes(const uint8_t *data, size_t len) {
    uint32_t total = 0; for (size_t i = 0; i < len; i++) total += data[i]; return total;
}
void for_each(const int32_t *values, size_t len, void (*visit)(int32_t, void *), void *context) {
    for (size_t i = 0; i < len; i++) visit(values[i], context);
}
const char *lookup(int32_t id) { return id == 1 ? "one" : NULL; }
int64_t sum_ints(int32_t count, ...) {
    va_list ap; va_start(ap, count); int64_t total = 0;
    for (int32_t i = 0; i < count; i++) total += va_arg(ap, int32_t);
    va_end(ap); return total;
}
int32_t counter = 0;
int32_t nia_square(int32_t);
int32_t call_back_into_nia(int32_t x) { return nia_square(x); }
```

## Declaring And Calling A C Function

A bodyless `extern fun` declares a C function. Its symbol is the source name;
use `@[linkName("...")]` when the Nia name should differ. An omitted return
type means `()` (C `void`).

```nia
extern fun byte_length(s: &u8) usize;

@[linkName("byte_length")]
extern fun cStringLength(s: &u8) usize;
```

Calls look like any other call. Visibility comes first and `extern` second:
`pub extern fun`, never `extern pub fun`.

## C Strings

String and byte-string literals are arrays, not pointers. Write a
NUL-terminated byte string and pass a pointer to its first byte:

```nia
let length = byte_length((&b"hello\0").ptr());
```

`&b"hello\0"` is a `&[u8; 6]`; `.ptr()` gives the `&u8` C expects. The array
lives only as long as the enclosing expression or frame, so a C function must
not keep the pointer. Put the bytes in a `static` when C needs a stable
address. `std::cstring` provides `CString` and `CStringView` for building and
reading NUL-terminated strings at runtime.

## Structs By Value And By Pointer

Only `extern struct` has C layout. Declare it with the same fields, in the same
order, as the C struct:

```nia
extern struct Point {
    x: i32,
    y: i32,
}

extern fun point_scale(p: Point, factor: i32) Point;
extern fun point_translate(p: &mut Point, dx: i32, dy: i32);
```

```nia
let scaled = point_scale(Point { x: 1, y: 2 }, 3);

let mut p = Point { x: 0, y: 0 };
point_translate(&mut p, 4, 5);
```

By-value structs follow the target's C calling convention, including register
passing and hidden return pointers. Use `&T` for a C `const T *` and `&mut T`
for a `T *` the callee writes. An ordinary Nia `struct` cannot be passed by
value; its fields may be reordered.

## Out-Parameters

Pass `&mut` to a local:

```nia
extern fun divide(a: i32, b: i32, quotient: &mut i32, remainder: &mut i32) i32;

let mut quotient: i32 = 0;
let mut remainder: i32 = 0;
if divide(7, 2, &mut quotient, &mut remainder) != 0 {
    // quotient == 3, remainder == 1
}
```

## Nullable Pointers

Nia pointers have no separate nullable form at the C boundary, and `?&T`
cannot cross it. Declare the pointer type and compare against a null pointer
written as `0usize as &T`:

```nia
extern fun lookup(id: i32) &u8;

let name = lookup(1);
if name != 0usize as &u8 {
    let first = name.*;
}
```

Pass `0usize as &T` (or `0usize as &mut T`) to give C a null argument.

## Slices

A Nia slice is a pointer and a length, but it is not a C type. Pass the two
parts separately:

```nia
extern fun sum_bytes(data: &u8, len: usize) u32;

let bytes: [u8; 3] = [1, 2, 3];
let view: &[u8] = &bytes[..];
let total = sum_bytes(view.ptr(), view.len());
```

For an array, `&array[0]` is the element pointer. A C pointer and length
become a slice by range-indexing the pointer: `&ptr[0..len]`. Nia does not
check that the range is valid.

## Callbacks

A C function pointer type is `&extern fun(...)`. Only an `extern fun` produces
one, so write the callback as an `extern fun` definition. Closures and ordinary
`&fun(...)` pointers use the Nia convention and cannot be passed to C.

Pass state through C's `void *` as `&mut opaque` (or `&opaque` for
`const void *`). Erasure and recovery are explicit casts:

```nia
extern fun for_each(values: &i32, len: usize, visit: &extern fun(i32, &mut opaque), context: &mut opaque);

extern fun add_to_total(value: i32, context: &mut opaque) {
    let total = context as &mut i64;
    total.* += value as i64;
}

let values: [i32; 3] = [10, 20, 30];
let mut total: i64 = 0;
for_each(&values[0], 3, &add_to_total, &mut total as &mut opaque);
```

## Variadic Functions

A bodyless `extern fun` may end with `...` after at least one fixed parameter.
Extra arguments receive C's default promotions: integers narrower than 32 bits
widen to 32 bits and `f32` widens to `f64`.

```nia
extern fun sum_ints(count: i32, ...) i64;

let six = sum_ints(3, 1, 2, 3);
```

Nia cannot define a variadic function, and a variadic function cannot be taken
as a function pointer. Calling `printf` works the same way once the C library
that provides it is linked.

## Globals

`extern static` declares C data. The type is required:

```nia
extern static mut counter: i32;

counter += 1;
```

`@[linkName("...")]` renames the referenced symbol. Nia cannot export data to
C; export a function that returns a pointer instead.

## Linking A C Library

Native link inputs are options of `nia emit --exe`:

| Option | Effect |
| --- | --- |
| `-L <dir>`, `--library-path <dir>` | add a library search directory |
| `-l <name>`, `--library <name>` | link a library |
| `--link-arg <arg>` | pass one argument to the linker, such as an object file |
| `--executable dynamic`, `--interpreter <path>`, `--rpath <path>` | link a dynamic ELF executable against shared libraries (Linux) |

With `geom.c` compiled to an object or archive, and the Nia program from
[the complete example](#complete-example) in `main.nia`:

```text
clang -ffreestanding -c geom.c -o geom.o
nia emit --exe main.nia --link-arg geom.o -o main

# or, from a static library in ./libs
nia emit --exe main.nia -L libs -l geom -o main
```

On Windows, compile with `clang --target=x86_64-pc-windows-msvc` and use the
`.obj` or `.lib` file the same way.

Build scripts link Nia static archives through
`ExecutableOptions::withStaticArchives`; see [build scripts](build-scripts.md).

### Windows Import Libraries

The toolchain ships import descriptions only for the Win32 functions its own
standard library and runtime call. Any other Win32 function is an ordinary
native dependency: declare it and link its import library explicitly.

```nia
extern fun GetCurrentProcessId() u32;
```

```text
nia emit --exe main.nia -L "C:/Program Files (x86)/Windows Kits/10/Lib/<version>/um/x64" -l kernel32 -o main.exe
```

Without the import library the link fails with
`lld-link: error: undefined symbol: GetCurrentProcessId`.

## Exporting Nia Functions To C

An `extern fun` with a body is a C-callable definition. Its symbol is the source
name, or `@[exportName("...")]`:

```nia
extern struct Pair {
    a: i32,
    b: i32,
}

fun clamp(value: i32, low: i32, high: i32) i32 {
    if value < low { low } else if value > high { high } else { value }
}

pub extern fun pair_sum(p: Pair) i32 {
    p.a + p.b
}

@[exportName("nia_clamp")]
pub extern fun clampExport(value: i32, low: i32, high: i32) i32 {
    clamp(value, low, high)
}
```

Emit an object without Nia startup and link it from C with the C toolchain:

```text
nia emit --obj mathlib.nia -o mathlib.o
cc main.c mathlib.o -o main
```

```c
#include <stdint.h>
struct Pair { int32_t a, b; };
int32_t pair_sum(struct Pair p);
int32_t nia_clamp(int32_t value, int32_t low, int32_t high);
```

`nia emit --obj` defaults to `--runtime bare`, so the object contains no
process entry. Only `extern` definitions are visible outside the linked image;
ordinary Nia functions such as `clamp` stay internal.

In a Nia executable, every top-level `pub extern fun` definition of the entry
package is exported and kept even when Nia never calls it, so linked C code can
call back into the program (`nia_square` in the complete example). The name
`_start` is reserved for the toolchain's process entry.

## C And Nia Type Mapping

| C | Nia |
| --- | --- |
| `int8_t`, `signed char` | `i8` |
| `uint8_t`, `unsigned char` | `u8` |
| `char` (as a byte) | `u8` or `i8`, matching the target's `char` signedness |
| `int16_t`, `short` | `i16` |
| `uint16_t`, `unsigned short` | `u16` |
| `int32_t`, `int` | `i32` |
| `uint32_t`, `unsigned int` | `u32` |
| `int64_t`, `long long` | `i64` |
| `uint64_t`, `unsigned long long` | `u64` |
| `long` / `unsigned long` | `i64` / `u64` on Linux and macOS (LP64); `i32` / `u32` on Windows (LLP64) and `x86-unknown-linux` |
| `size_t`, `uintptr_t` | `usize` |
| `ptrdiff_t`, `intptr_t`, `ssize_t` | `isize` |
| `__int128` | `i128` (not on `x86-unknown-linux`) |
| `float`, `double` | `f32`, `f64` |
| `bool`, `_Bool` | `u8` |
| `wchar_t` | `u16` on Windows, `i32` elsewhere |
| `const T *` / `T *` | `&T` / `&mut T` |
| `volatile T *` | `^T` / `^mut T` |
| `const void *` / `void *` | `&opaque` / `&mut opaque` |
| `const char *` string | `&u8` |
| `R (*)(A)` | `&extern fun(A) R` |
| `struct S` | `extern struct S` |
| `union U *` | `&mut U`, with `U` declared as `extern union U` |
| `enum E` | its integer type, usually `i32` |
| `void` return | `()`, or omit the return type |
| `T arr[N]` struct field | `arr: [T; N]` |

Nia has no packed or over-aligned struct form, and no C bit-fields.

## Common Errors

`nia check`, `nia emit`, and builds report boundary errors as `E0501` on the
offending declaration, whether or not anything calls it:

| Message | Fix |
| --- | --- |
| ``extern parameter cannot use `bool` directly`` | use `u8` or `i32` and convert with a comparison |
| ``extern parameter cannot use `char` directly`` | use `u32` and convert explicitly |
| `extern parameter cannot use normal Nia struct by value` | declare an `extern struct`, or pass a pointer |
| `extern parameter cannot use nia slice directly` | pass `slice.ptr()` and `slice.len()` |
| `extern parameter cannot use optional by value` | use a pointer compared with `0usize as &T` |
| `extern parameter cannot use enum directly; use its backing integer type` | pass the backing integer |
| `extern parameter cannot use array by value` | pass `&array[0]` |
| `extern parameter cannot use tuple by value` | use an `extern struct` |
| ``extern parameter cannot use `()` `` | remove the parameter; `()` is only valid as a return type |
| `extern parameter must use a C function pointer` | write the type as `&extern fun(...)` and pass an `extern fun` |
| `extern parameter cannot use empty struct by value` | add a field, or pass a pointer |
| ``extern parameter cannot use a 128-bit integer on `x86-unknown-linux` `` | split the value into two 64-bit integers |
| `extern parameter cannot use union by value` | pass a pointer to an `extern union` |
| ``extern return type cannot use `never` directly`` | return `()` |
| `extern variadic function definition is not supported` | define a fixed-parameter function |
| `extern parameter cannot use variadic function pointer` | call the variadic function directly |
| `` `@[exportName]` requires an extern function definition `` | use `linkName` on a declaration |
| `` `@[linkName]` is only valid on an external function declaration `` | use `exportName` on a definition |
| ``external export `_start` is reserved for the compiler-provided runtime`` | choose another name |
| ``expected `struct`, `union`, `fun`, or `static` after `extern` `` | write `pub extern fun`, not `extern pub fun` |

Type mismatches at call sites are ordinary type errors:

| Message | Fix |
| --- | --- |
| `expected &extern fun(i32, &mut opaque), got &fun(i32, &mut opaque)` | define the callback as `extern fun` |
| `expected &u8, got &[u8; 6]` | add `.ptr()` to the array reference |
| `expected &mut opaque, got &mut i64` | cast: `&mut value as &mut opaque` |

## Complete Example

This program exercises every pattern above against `geom.c`:

```nia
using std::process;
using process::{Init, ExitCode};

extern struct Point {
    x: i32,
    y: i32,
}

extern fun point_scale(p: Point, factor: i32) Point;
extern fun point_translate(p: &mut Point, dx: i32, dy: i32);
extern fun divide(a: i32, b: i32, quotient: &mut i32, remainder: &mut i32) i32;
extern fun byte_length(s: &u8) usize;
extern fun sum_bytes(data: &u8, len: usize) u32;
extern fun for_each(values: &i32, len: usize, visit: &extern fun(i32, &mut opaque), context: &mut opaque);
extern fun lookup(id: i32) &u8;
extern fun sum_ints(count: i32, ...) i64;
extern static mut counter: i32;
extern fun call_back_into_nia(x: i32) i32;

extern fun add_to_total(value: i32, context: &mut opaque) {
    let total = context as &mut i64;
    total.* += value as i64;
}

pub extern fun nia_square(x: i32) i32 {
    x * x
}

pub fun main(init: Init) ExitCode!() {
    _ = init;
    let scaled = point_scale(Point { x: 1, y: 2 }, 3);
    if scaled.x != 3 or scaled.y != 6 { return ExitCode(1)!; }

    let mut p = Point { x: 0, y: 0 };
    point_translate(&mut p, 4, 5);
    if p.x != 4 or p.y != 5 { return ExitCode(2)!; }

    let mut quotient: i32 = 0;
    let mut remainder: i32 = 0;
    if divide(7, 2, &mut quotient, &mut remainder) == 0 { return ExitCode(3)!; }
    if quotient != 3 or remainder != 1 { return ExitCode(4)!; }

    if byte_length((&b"hello\0").ptr()) != 5 { return ExitCode(5)!; }

    let bytes: [u8; 3] = [1, 2, 3];
    let view: &[u8] = &bytes[..];
    if sum_bytes(view.ptr(), view.len()) != 6 { return ExitCode(6)!; }

    let values: [i32; 3] = [10, 20, 30];
    let mut total: i64 = 0;
    for_each(&values[0], 3, &add_to_total, &mut total as &mut opaque);
    if total != 60 { return ExitCode(7)!; }

    if lookup(2) != 0usize as &u8 { return ExitCode(8)!; }
    let one = lookup(1);
    if one == 0usize as &u8 or one.* != b'o' { return ExitCode(9)!; }

    if sum_ints(3, 1, 2, 3) != 6 { return ExitCode(10)!; }

    counter += 1;
    if counter != 1 { return ExitCode(11)!; }

    if call_back_into_nia(7) != 49 { return ExitCode(12)!; }
    !()
}
```

The program exits with status 0 when every exchange succeeds.
