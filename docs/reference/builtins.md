# Built-in Operations

`std::builtin` declares the operations the compiler implements directly. They
are ordinary declarations called with ordinary path, generic-argument, and call
syntax, either fully qualified (`std::builtin::size[T]()`) or after
`using std::builtin;` (`builtin::size[T]()`). The built-in traits that
`std::builtin` also declares (operators, `Deref`, `Index`, `Len`, `Iterable`,
`IntoError`, and others) are specified in the
[language reference](language.md#built-in-traits).

A function marked `const fun` below can also run during
[constant evaluation](language.md#constant-evaluation).

## Layout

| Declaration | Result |
| --- | --- |
| `const fun size[T]() usize where T: Sized` | size of `T` in bytes |
| `const fun align[T]() usize where T: Sized` | alignment of `T` in bytes |
| `const fun offset[T](field: &[char]) usize` | byte offset of a struct or union field |

`size` and `align` require `T: Sized`; generic code states the bound in its
`where` clause. The `offset` argument is a string literal naming the field;
every union field has offset 0. With a concrete type argument these calls are
compile-time constants, usable in array lengths and `const` initializers; a
`static` initializer uses them through a named `const`, as below. The values
come from the [ABI](abi.md).

```nia
using std::builtin;

struct Header {
    magic: u32,
    length: u16,
}

const header_size: usize = builtin::size[Header]();
const length_offset: usize = builtin::offset[Header]("length");
static scratch: [u8; header_size] = [0; header_size];

fun bytes_per[T]() usize
where T: Sized {
    builtin::size[T]() + builtin::align[T]()
}
```

## Control And Compile-Time Data

| Declaration | Meaning |
| --- | --- |
| `const fun trap() never` | terminates execution; a compile-time error during constant evaluation |
| `const fun error(message: &[char]) never` | reports `message` as a compile-time error; const-only |
| `const fun embed(path: &[char]) [u8; N]` | the bytes of a file; const-only |

`trap` executes the target's trap instruction at runtime. `error` takes a
string literal or another constant string. A `const fun` that contains `error`
cannot be called from runtime code.

`embed` reads a file at compile time. The path is a string literal resolved
relative to the source file that contains the call, not the working directory.
It may appear only in a `const` initializer or a `const fun` body; a `static`
that needs the bytes is initialized from a named `const`. The bytes are not
parsed.

```nia
using std::builtin;

const fun non_zero(value: u32) u32 {
    if value == 0 {
        builtin::error("value must not be zero");
    }
    value
}

const divisor: u32 = non_zero(8);

fun divide(value: u32, by: u32) u32 {
    if by == 0 {
        builtin::trap();
    }
    value / by
}
```

## Memory

| Declaration | Meaning |
| --- | --- |
| `memcpy[T](dest: &mut [T], source: &[T]) ()` | copies elements between non-overlapping slices |
| `memmove[T](dest: &mut [T], source: &[T]) ()` | copies elements between possibly overlapping slices |
| `memset(dest: &mut [u8], value: u8) ()` | sets every byte of a slice |
| `loadUnaligned[T](ptr: &u8) T` | reads a `T` from a byte address with no alignment requirement |
| `storeUnaligned[T](ptr: &mut u8, value: T) ()` | writes a `T` to a byte address with no alignment requirement |

`memcpy` and `memmove` copy the common prefix of the two slices as raw element
representation. The source must be initialized and readable, the destination
writable. `memcpy` copies forward and requires that no overlap changes a source
element before it is read; `memmove` permits any overlap. The destination
argument must be a slice expression such as `&mut buffer[..]`; an array pointer
is not converted here. The element type must be `Sized`, as must `T` loaded by
`loadUnaligned`; generic callers state that bound in their `where` clause.

`loadUnaligned` relaxes only alignment. At least `size[T]()` initialized bytes
must be readable at `ptr`, and `T` must be `Sized`. `storeUnaligned` is its
counterpart: at least `size[T]()` bytes must be writable at `ptr`. It is an
ordinary `@[inlineAlways]` function that copies the value's bytes with
`memcpy`; the fixed length compiles to one unaligned store at every
optimization level.

```nia
using std::builtin;

fun copy_header(source: &[u8]) u32 {
    let mut buffer: [u8; 8] = [0; 8];
    builtin::memset(&mut buffer[..], 0);
    builtin::memcpy[u8](&mut buffer[..], source);
    builtin::loadUnaligned[u32](&buffer[1])
}
```

These are low-level primitives; the standard library's slice operations
([`std::slice`](../../lib/README.md)) provide checked copying.

## Bit Counting

| Declaration | Result |
| --- | --- |
| `const fun ctz[T](value: T) T` | number of trailing zero bits |
| `const fun clz[T](value: T) T` | number of leading zero bits |
| `const fun popcount[T](value: T) T` | number of one bits |

`T` is an integer type and the result has the same type. `ctz` and `clz` of `0`
return the bit width of `T`, at runtime and during constant evaluation.

```nia
const trailing: u32 = std::builtin::ctz[u32](8);     // 3
const leading: u8 = std::builtin::clz[u8](0);        // 8
```

## Saturating Arithmetic

| Declaration | Result |
| --- | --- |
| `const fun saturatingAdd[T](lhs: T, rhs: T) T` | `lhs + rhs`, clamped to the range of `T` |
| `const fun saturatingSub[T](lhs: T, rhs: T) T` | `lhs - rhs`, clamped to the range of `T` |

`T` is an integer type or an integer vector such as `u8x16`; a vector is
computed lane by lane. A result past either end of the range becomes that end
instead of wrapping or trapping. The type argument may be left out: `T` is then
the type of the first operand, or of the second when the first is an untyped
numeric literal and the second is not.

```nia
const full: u8 = std::builtin::saturatingAdd[u8](250, 10);   // 255
const floor: i8 = std::builtin::saturatingSub(-100i8, 100);  // -128
```

On x86-64, `u8x16` and `i16x8` operands compile to single instructions such as
`paddusb` and `psubsw`; scalars compile to an add and a conditional move.

## SIMD

| Declaration | Meaning |
| --- | --- |
| `const fun splat[V](value: Lane) V` | a vector with every lane set to `value` |
| `const fun extract[V](vector: V, index: usize) Lane` | one lane |
| `const fun insert[V](vector: V, index: usize, value: Lane) V` | a copy with one lane replaced |
| `const fun bitmask[V](mask: V) usize` | packs a `boolxN` mask, lane 0 in the least significant bit |
| `const fun shuffle[V, N: usize](first: V, second: V, indices: [usize; N]) Lane x N` | lanes chosen from `first` and `second` |
| `const fun lookup16(table: u8x16, indices: u8x16) u8x16` | lane `i` is `table[indices[i]]`, or 0 when that index is 16 or more |

`V` is a concrete [vector type](language.md#simd-vector-types) and `Lane` its
lane type; these functions do not accept a generic `V`, even one bounded by
`Simd`. `splat` requires its type argument; `extract` and `insert` infer it
from the vector. An out-of-range lane index is a compile-time error during
constant evaluation and traps at runtime before any lane is accessed. `bitmask`
accepts at most as many lanes as the target pointer has bits.

`shuffle` reads its operands as one sequence of `2L` lanes, where `L` is the
lane count of `V`: indices below `L` select lanes of `first`, and indices from
`L` to `2L - 1` select lane `index - L` of `second`. `indices` must be known at
compile time, as a literal or a `const`; an index of `2L` or more is a
compile-time error. The result has `V`'s lane type and one lane per index, so
it may be narrower or wider than `V`. Pass the same vector twice to rearrange a
single vector.

```nia
using std::builtin;

fun interleaveLow(a: u8x16, b: u8x16) u8x16 {
    builtin::shuffle(a, b, [0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23])
}

fun highHalf(v: i32x4) i32x2 {
    builtin::shuffle(v, v, [2, 3])
}
```

`lookup16` is the run-time counterpart of `shuffle` for bytes: the indices are
a vector, so each lane can choose a different table entry. An index of 16 or
more gives 0 for that lane instead of wrapping, so an index vector can mark
lanes to clear. A typical use classifies bytes by their low four bits:

```nia
using std::builtin;

// Lanes holding an ASCII digit `'0'..='9'` become 1; all others become 0.
fun digitLanes(bytes: u8x16) u8x16 {
    // Entry `n` is 1 when a byte with high four bits 3 and low four bits `n`
    // is a digit.
    let digits: [u8; 16] = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0];
    let table = builtin::loadUnaligned[u8x16](&digits[0]);
    let low = bytes & builtin::splat[u8x16](15u8);
    let high = bytes >> builtin::splat[u8x16](4u8);
    // A byte whose high four bits are not 3 gets an index of 16 or more,
    // so its lane becomes 0.
    let outside = (high ^ builtin::splat[u8x16](3u8)) << builtin::splat[u8x16](4u8);
    builtin::lookup16(table, low | outside)
}
```

On AArch64 `lookup16` is a single `tbl`. On x86-64 it becomes `pshufb` and a
mask when the CPU has SSSE3; the default baseline CPU does not, so it is
computed lane by lane there.

```nia
using std::builtin;

fun count_matches(data: u8x16, needle: u8) usize {
    let mask = data == builtin::splat[u8x16](needle);
    builtin::popcount[usize](builtin::bitmask(mask))
}
```

## Atomics

| Declaration | Meaning |
| --- | --- |
| `atomicLoad[T](ptr: &T, order: usize) T` | atomic load |
| `atomicStore[T](ptr: &mut T, value: T, order: usize) ()` | atomic store |
| `atomicRmw[T](ptr: &mut T, op: usize, value: T, order: usize) T` | read-modify-write; returns the previous value |
| `cmpxchgStrong[T](ptr: &mut T, expected: T, desired: T, success: usize, failure: usize) ?T` | compare-and-exchange |
| `cmpxchgWeak[T](...) ?T` | compare-and-exchange that may fail spuriously |
| `fence(order: usize) ()` | atomic fence |

`atomicLoad` also accepts a `&mut T`. `cmpxchgStrong` and `cmpxchgWeak` return
`null` when the exchange happened and `?old` with the current value when it did
not.

`order` and `op` must be compile-time integer constants. `std::atomic` names
them:

| Orderings | Value | | Operations | Value |
| --- | --- | --- | --- | --- |
| `Unordered` | 0 | | `Xchg` | 0 |
| `Monotonic` | 1 | | `Add` | 1 |
| `Acquire` | 2 | | `Sub` | 2 |
| `Release` | 3 | | `And` | 3 |
| `AcqRel` | 4 | | `Nand` | 4 |
| `SeqCst` | 5 | | `Or` | 5 |
| | | | `Xor` | 6 |
| | | | `Max`, `Min` | 7, 8 |
| | | | `UMax`, `UMin` | 9, 10 |

Allowed orderings:

| Operation | Orderings |
| --- | --- |
| load | `Unordered`, `Monotonic`, `Acquire`, `SeqCst` |
| store | `Unordered`, `Monotonic`, `Release`, `SeqCst` |
| read-modify-write, compare-and-exchange success | `Monotonic`, `Acquire`, `Release`, `AcqRel`, `SeqCst` |
| compare-and-exchange failure | `Monotonic` with any success ordering; `Acquire` when success is `Acquire`, `AcqRel`, or `SeqCst`; `SeqCst` when success is `SeqCst` |
| fence | `Acquire`, `Release`, `AcqRel`, `SeqCst` |

The atomic value types are `bool`, `char`, integers, enums, and ordinary object
pointers, each no wider than the target pointer; `u64` is therefore not atomic
on `x86-unknown-linux`. Volatile pointers, floating-point
types, slices, structs, arrays, and unions are not atomic value types.
`std::atomic::Atomic[T]` wraps these operations in named methods.

```nia
using std::atomic;
using std::builtin;

fun bump(counter: &mut u32) u32 {
    let previous = builtin::atomicRmw[u32](counter, atomic::Add, 1, atomic::AcqRel);
    builtin::fence(atomic::SeqCst);
    previous + builtin::atomicLoad[u32](counter, atomic::Acquire)
}
```

## Inline Assembly

```nia
fun exit_linux(code: usize) isize {
    let mut result: isize = 0;
    std::builtin::asm(.{
        code:
            b"mov rax, 60\n"
            b"syscall",
        outputs: .{ rax: result },
        inputs: .{
            rax: 60usize,
            rdi: code,
        },
        clobbers: [b"rcx", b"r11", b"memory"],
        options: [b"volatile"],
    });
    result
}
```

`asm(config: AsmConfig) ()` emits target assembly. Its argument is an
`AsmConfig` literal, normally written with the omitted-type form `.{ ... }`;
`inputs` and `outputs` are `AsmInputs` and `AsmOutputs` literals written the
same way.

| Field | Contents |
| --- | --- |
| `code` | a byte string literal, passed unchanged to the target assembler |
| `inputs` | register name or class mapped to an expression |
| `outputs` | register name or class mapped to an assignable place |
| `clobbers` | an array of byte string literals naming clobbered registers and `b"memory"` |
| `options` | a byte string literal or an array of them; the only option is `b"volatile"` |

In `inputs` and `outputs`, `reg` selects a general-purpose register class,
`freg` a floating-point register class, and any other name, such as `rax` or
`rdi`, a fixed register. In `code`, `$0`, `$1`, ... name the operands, outputs
first and then inputs, in the order they are written. An operand is a scalar,
a pointer, a function pointer, or an enum value, represented as in the
[ABI](abi.md#inline-assembly); aggregates, slices, vectors, optionals, and
error unions are rejected. The assembly text itself is opaque to the type
checker.

The assembler accepts all of its directives. On x86 each `asm` call starts in
Intel syntax without register prefixes; `.att_syntax prefix` switches syntax
until the end of that call. Other architectures have a single syntax. A
[`@[naked]`](language.md#naked-functions) function's `asm` calls cannot have
`inputs` or `outputs`.

## Characters And Source Locations

| Declaration | Meaning |
| --- | --- |
| `const fun charFromU32(value: u32) ?char` | the `char` for a Unicode scalar value, or `null` |
| `const fun callerLocation() SourceLocation` | the current source location, or the tracked caller's |

`callerLocation` is also available as `std::callerLocation`, and its result
type is `std::SourceLocation`, with `file()`, `line()`, and `column()`
accessors. Its interaction with `@[trackCaller]` is described in the
[language reference](language.md#caller-location).

## Target Constants

`std::builtin::target` declares the target facts as constants:

| Constant | Type |
| --- | --- |
| `arch` | `&[char]` |
| `vendor` | `&[char]` |
| `os` | `&[char]` |
| `env` | `&[char]` |
| `endian` | `&[char]` |
| `pointerWidth` | `usize` |

They have the same values as the names in
[`@[if]` conditions](language.md#conditional-compilation); the values per target
are listed in [Target Facts](platform-support.md#target-facts).

```nia
using std::builtin::target;

const pointer_bytes: usize = target::pointerWidth / 8;
```

The module also answers what `hasFeature` answers in `@[if]` conditions:

| Declaration | Result |
| --- | --- |
| `enum CpuFeature` | one variant per [CPU feature](platform-support.md#cpu-levels): `Sse2`, `Ssse3`, `Sse4_1`, `Avx2`, `Avx512bw`, `Neon`, and the others in that table |
| `const fun hasFeature(feature: CpuFeature) bool` | whether the code being built may use `feature` |

The answer depends on the CPU level of the build, so it is one constant
throughout a build and folds away. Unlike `@[if hasFeature(...)]`, code that
branches on it is checked for every level; use it for differences that are only
values. The feature is an enum, so a misspelled name is a compile-time error.

```nia
using std::builtin::target;

const vector_bytes: usize = if target::hasFeature(.Avx2) { 32 } else { 16 };
```

## Built-In Methods

These methods exist on structural types without an import. `len()` comes from
the `std::builtin::Len` trait, whose implementations, like every trait
implementation, need no `using`. A visible user method with the same name
takes priority.

| Method | Receiver | Result |
| --- | --- | --- |
| `len()` | array, array pointer, slice | element count, from `Len` |
| `start()` | `a..b`, `a..=b`, `a..` | the start bound |
| `end()` | `a..b`, `a..=b`, `..b`, `..=b` | the end bound |
| `ptr()` | `&[T]` or `&mut [T]` | `&T` to the first element |
| `ptrMut()` | `&mut [T]` | `&mut T` to the first element |

Array receivers use the same automatic address and array-to-slice conversions
as ordinary slice arguments: `bytes.ptr()` and `b"text\0".ptr()` return a
pointer to the first element. `bytes.ptrMut()` requires a writable array place;
it cannot address an immutable binding or a temporary as mutable. Explicit
array pointers remain accepted. An empty slice has a data pointer, but
dereferencing it is invalid.

```nia
fun first_bytes(data: &mut [u8]) usize {
    let read: &u8 = data.ptr();
    let mut view = data;
    let write: &mut u8 = view.ptrMut();
    write.* = read.*;
    let r = 0usize..view.len();
    r.end() - r.start()
}
```
