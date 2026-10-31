# Builtins Reference

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
compile-time constants, usable in array lengths and static initializers. The
values come from the [ABI](abi.md).

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
| `embed(path)` | the bytes of a file as a `[u8; N]` value; const-only |

`trap` executes the target's trap instruction at runtime. `error` takes a
string literal or another constant string. A `const fun` that contains `error`
cannot be called from runtime code.

`embed` reads a file at compile time. The path is a string literal resolved
relative to the source file that contains the call, not the working directory.
It may appear only in a constant context, and the bytes are not parsed.

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

`memcpy` and `memmove` copy the common prefix of the two slices as raw element
representation. The source must be initialized and readable, the destination
writable. `memcpy` copies forward and requires that no overlap changes a source
element before it is read; `memmove` permits any overlap. The destination
argument must be a slice expression such as `&mut buffer[..]`; an array pointer
is not converted here. The element type must be `Sized`, as must `T` loaded by
`loadUnaligned`; generic callers state that bound in their `where` clause.

`loadUnaligned` relaxes only alignment. At least `size[T]()` initialized bytes
must be readable at `ptr`, and `T` must be `Sized`.

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

## SIMD

| Declaration | Meaning |
| --- | --- |
| `const fun splat[V](value: Lane) V` | a vector with every lane set to `value` |
| `const fun extract[V](vector: V, index: usize) Lane` | one lane |
| `const fun insert[V](vector: V, index: usize, value: Lane) V` | a copy with one lane replaced |
| `const fun bitmask[V](mask: V) usize` | packs a `boolxN` mask, lane 0 in the least significant bit |

`V` is a [vector type](language.md#simd-vector-types) and `Lane` its lane
type. `splat` usually needs its type argument; `extract` and `insert` infer it
from the vector. An out-of-range lane index is a compile-time error during
constant evaluation and traps at runtime before any lane is accessed. The
largest lane count `bitmask` accepts is target-defined.

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
| compare-and-exchange failure | `Monotonic`, `Acquire`, `SeqCst`, and no stronger than the success ordering |
| fence | `Acquire`, `Release`, `AcqRel`, `SeqCst` |

The atomic value types are `bool`, integers, enums, and ordinary object
pointers no wider than the target pointer. Volatile pointers, floating-point
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
| `options` | a byte string literal or an array of them; the available options are target-defined |

In `inputs` and `outputs`, `reg` selects a general-purpose register class,
`freg` a floating-point register class, and any other name, such as `rax` or
`rdi`, a fixed register. Operand types follow the target's assembly and
[ABI](abi.md#inline-assembly) conventions; the assembly text is opaque to the
type checker.

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

An array pointer reaches `ptr()` through the array-to-slice conversion, so
`(&bytes).ptr()` works, but an array value does not: `b"text\0".ptr()` is an
error. An empty slice has a data pointer, but dereferencing it is invalid.

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
