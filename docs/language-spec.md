# Nia Language Specification

Status: normative language reference

This document defines the current Nia language. It is intended to be the main
reference for users, implementers, tests, and maintenance work. Version
numbers are tracked by Git tags and release history, not by this file name.

Nia is a small systems programming language. It keeps the directness of C while
removing declaration syntax baggage, hidden runtime policy, and large semantic
systems that would make the language hard to implement and maintain.

## 1. Language Scope

Nia provides:

- a small statically typed language for systems programming;
- direct memory-oriented types: integers, booleans, arrays, pointers, slices,
  structs, unions, function pointers, and C-style enums;
- simple type generics for functions, structs, and methods, implemented by
  monomorphization;
- methods declared in `extend` blocks;
- traits implemented by `extend Type : Trait` blocks;
- expression-oriented blocks and `if`;
- C-style enums with namespaces and `match` without fallthrough;
- recursive optional/error-union and value patterns through `match` and
  if-pattern expressions;
- compile-time value bindings with `const`;
- `defer` for scope cleanup;
- C ABI interop through `extern`;
- explicit file modules declared with `module`, `using`, and `pub using`;
- a small visibility model based on `pub`;
- freestanding executable startup through the standard library, with object and
  LLVM output available for custom build flows.

The current core language keeps these systems outside the language surface:

- garbage collection;
- exceptions;
- general algebraic data types. Nia provides nominal enums, including tuple and
  named payload variants, but does not provide a general structural algebraic
  data type system. Enum layout, ABI, and exhaustiveness remain explicit and
  are fixed by each declaration;
- an ownership and lifetime borrow checker. Nia does not track aliasing or
  infer lifetimes, and it has no RAII destructor protocol. It does perform the
  flow-sensitive checks needed to reject clearly invalid programs, including
  callable-view and captured-address escape analysis, so the absence of a borrow
  checker is not an absence of static checking;
- implicit allocation. No core form allocates: the language has no built-in
  allocator, no owning container, and no growable type, and no expression,
  literal, aggregate, or control-flow construct acquires storage on its own.
  Allocation is ordinary library code that takes an explicit `mem::Allocator`
  argument and returns a typed failure. This is a deliberate constraint rather
  than an omission: because the core syntax and semantics never presuppose a
  heap, the same language runs on bare metal and in deeply embedded targets
  without a reduced dialect;
- a hidden runtime startup model. Startup is explicit instead: the standard
  library provides the injected facade selected per target, the entry contract
  is a visible `process::Init` parameter, and `--runtime bare` omits injection
  entirely;
- package management and a registry as part of the core language. The
  toolchain-owned build system is developed in this repository and is a library
  and command surface rather than language grammar.

## 2. Source Files And Programs

Nia source files use the `.nia` extension.

A compilation unit is UTF-8 text. Source locations are tracked with byte offsets
and reported through source spans.

An executable is started by the standard library. The compiler loads the entry
source as the reserved `root` module and injects a standard-library package
startup facade; that facade selects the target startup implementation and calls
the public user entry function through `root::main`. The startup facade is an
implementation detail, not a public `std` API.

The current user entry contract is intentionally single-shaped:

```nia
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    !()
}
```

Returning `!()` means process success. `process::ExitCode` is an open enum
backed by `i32`; `process::ExitCode::Success` names status `0`, and the
standard-library constructor for an unnamed status is `process::exit(code)`.
The language also permits an explicit `code as process::ExitCode` cast because
`ExitCode` is an open integer-backed enum, but ordinary executable code uses
the constructor so the conversion remains visible and searchable at one API
boundary. Returning an error payload such as
`process::exit(1)!` asks the startup layer to terminate with that exit status:

```nia
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    process::exit(1)!
}
```

Nia distinguishes two execution models:

- executable emission: the driver injects the standard-library package startup
  facade for the selected runtime. The current default is freestanding startup
  linked without CRT startup; the current target implementation is Linux
  x86_64. The user entry remains the Nia-level
  `root::main(process::Init) process::ExitCode!()` contract.
- bare/object/IR emission: no startup logic is injected and `main` is not
  required. The compiler
  emits LLVM IR or object files for an external build system, custom entry
  symbol, linker script, or freestanding runtime.

Other Nia functions named `main` use normal Nia internal symbol naming unless
they are declared `extern`. The compiler does not export the root user `main`
as the C ABI entry point; that responsibility belongs to the injected startup
facade.
The `std::start` module path is reserved for the injected standard-library
runtime and is not visible to user packages.

The standard library is a toolchain component rather than part of the core
language grammar. Its current facade, ownership, allocation, and error
contracts live in [`lib/README.md`](../lib/README.md) and the owning
`lib/std` source modules.

## 3. Lexical Structure

### 3.1 Comments

Nia supports line comments:

```nia
// comment until end of line
```

Block comments are not part of the language.

### 3.2 Identifiers

Identifiers start with an ASCII letter or `_`. Later characters may be ASCII
letters, digits, or `_`.

Identifiers themselves are ASCII-only. This does not restrict character or
string literal contents.

### 3.3 Keywords

The following words are reserved:

```text
and
as
bool
break
const
continue
defer
else
enum
extern
extend
false
fn
for
if
module
mut
never
not
null
or
pub
return
struct
match
trait
true
type
using
let mut
opaque
where
```

Primitive type names such as `i32`, `u8`, `usize`, `bool`, `char`, and `never`,
plus the `opaque` incomplete pointer target, are reserved type names. `()` is
the built-in unit type and zero-element tuple; it has no identifier spelling.
A capitalized `Fn` followed by `(` is the contextual callable-interface type
constructor, not an ordinary type path.
A standalone `!` is reserved for error-union syntax, not logical negation or
the never type.

### 3.4 Literals

Integer literals:

```nia
0
42
0xff
0b1010
0o755
1_000_000
```

Integer literal type is inferred from context. Without context, a decimal
integer literal defaults to `i32`. Integer literals may use an explicit suffix:

```text
i8 i16 i32 i64 i128 isize
u8 u16 u32 u64 u128 usize
```

```nia
let mut x: u8 = 1;
let mut n: usize = 10;
let mut chained = 10i32;
```

The suffix selects the literal's type before contextual inference. The literal
value must fit in that type. An underscore may appear only between two digits
valid for the literal's radix and does not affect the value. Radix prefixes and
underscores may be combined with suffixes:

```nia
let mut mask = 0xffu8;
let mut bits = 0b1010_0000u8;
let mut mode = 0o755usize;
```

Floating-point literals:

```nia
1.0
1e3
1_000.5
1.0e-3
```

Floating literal type is inferred from context. Without context, it defaults to
`f64`. Floating literals may use `f32` or `f64` suffixes:

```nia
let mut x: f32 = 1.5;
let mut y = 1.5f32;
let mut z = 1.0e-3f64;
```

The suffix selects the literal's type before contextual inference. The value
must be finite and representable in that type. Integer suffixes are invalid on
floating literals.

For built-in binary arithmetic, bitwise, comparison, and equality operations
whose operands share a numeric type, a concrete operand supplies type context
for an unsuffixed numeric literal on either side. An explicit suffix is never
overridden by the peer operand. Shift counts are checked independently and do
not determine the type of the value being shifted.

Scalar integer division and remainder trap when the divisor is zero. Signed
scalar division and remainder also trap for `MIN / -1` and `MIN % -1`, since
the mathematical quotient is not representable in the operand type. During
constant evaluation these traps are source diagnostics; runtime lowering checks
the same conditions before emitting the LLVM integer operation. Floating-point
division and remainder follow their floating-point semantics and are not
covered by this scalar integer trap rule.

During constant evaluation, integer addition, subtraction, multiplication, and
negation are checked at every operation against the concrete operand type. The
same scalar runtime operations trap on overflow before an LLVM operation can
produce poison. An overflowing const intermediate is a source diagnostic even
when a later operation would return the final value to range. The concrete type
comes from ordinary semantic inference, including expected types, instantiated
generic parameters, the defining module of an imported `const fn`, and the
target width of `usize` and `isize`; literal spelling and the compiler host
integer width do not define
these rules. A constant integer shift count must be non-negative and smaller
than the concrete left operand width. The same count rule applies to scalar
runtime shifts: a negative signed count or a count greater than or equal to the
concrete left operand width traps before the shift executes. Left shift
additionally requires the mathematical result to remain representable in the
left operand type, diagnosing during constant evaluation and trapping at
runtime. Right shift is arithmetic for signed left operands and logical for
unsigned left operands. The count's integer type does not change either the
result type or the right-shift mode. Integer-vector overflow and shift behavior
remains a separate lane-wise design boundary.

An expected optional or error-union type supplies context to its constructed
payload. For example, `return ?0` in a function returning `?usize`, `!1` where
`E!u8` is expected, and `2!` where `u8!i32` is expected infer `usize`, `u8`, and
`u8` respectively. The wrapper does not force an otherwise contextual numeric
literal to default to `i32`.

Boolean literals:

```nia
true
false
```

Byte character literals:

```nia
b'a'
b'\n'
b'\0'
```

`b'...'` has type `u8`. After escape decoding it must contain exactly one byte.

Character literals:

```nia
'a'
'\n'
'N'
```

`'...'` has type `char`. `char` represents a Unicode scalar value.

String literals:

```nia
"nia"
"中a"
"hello\n"
```

String literals are fixed-length Unicode scalar arrays. `"..."` has type
`[char; N]`.

Byte string literals:

```nia
b"nia"
b"nia\0"
```

Byte string literals are fixed-length byte arrays. `b"..."` has type `[u8; N]`.
NUL-terminated byte sequences are written explicitly, for example `b"nia\0"`.

Adjacent quoted string literals with the same prefix are concatenated into one
literal:

```nia
"hello, " "world"
b"ni" b"a\0"
```

This is source-level literal concatenation, not runtime string or array
concatenation. Mixed literal families are invalid:

```nia
"hello" b"world" // invalid
```

String-family array values do not implicitly decay to slices. Take an explicit
address when a slice is expected; the resulting pointer-to-array uses the
ordinary pointer-array-to-slice coercion:

```nia
let text: &[char] = &"nia";
let bytes: &[u8] = &b"nia";
```

The same rule applies to named arrays:

```nia
let stable = b"nia";
let stable_bytes: &[u8] = &stable;
```

Multiline string literals use consecutive lines beginning with `\\`. Byte
multiline string literals use `b\\` on the first line; continuation lines still
use `\\`:

```nia
\\mov rax, 60
\\syscall

b\\mov rax, 60
\\syscall
```

For multiline strings, indentation before the delimiter is ignored, the delimiter
itself is not part of the string, and the text after the delimiter is copied as
is. Adjacent lines are joined with `\n`; no extra newline is appended after the
last line. Escape sequences are not interpreted inside multiline string lines.
The prefix selects the same type family as the quoted form: `[char; N]` or
`[u8; N]`.

Multiline string literals do not participate in adjacent literal concatenation.
Use adjacent quoted literals when a long single-line literal should be split
across source lines, and use multiline literals when the literal value should
contain real line breaks.

The literal:

```nia
"nia"
```

is equivalent to:

```nia
['n', 'i', 'a']
```

The literal:

```nia
b"nia\0"
```

is equivalent to:

```nia
[b'n', b'i', b'a', b'\0']
```

## 4. Types

### 4.1 Primitive Types

Integer types:

```text
i8 i16 i32 i64 i128 isize
u8 u16 u32 u64 u128 usize
```

Floating-point types:

```text
f32 f64
```

Other primitive types:

```text
bool
char
never
```

`()` is the unit type. It has exactly one value, written `()`, and is also the
zero-element tuple. If a function declaration omits its return type, the return
type is `()`; `return;` is the corresponding explicit return.

Tuple types are written `(T0, T1, ...)`; a trailing comma distinguishes a
one-element tuple `(T,)` from grouping. Tuple values and patterns use the same
syntax. A tuple projection `.N` selects a canonical decimal position label
(`.0`, `.1`, `.10`); it is a static tuple position, not a general const
expression. Projection preserves place, assignment, address-of, and mutability
semantics, and out-of-range positions are diagnosed statically.

`opaque` is an incomplete type accepted only as a direct pointer target. It has
no values, fields, layout, or dereference operation. `&opaque`, `&mut opaque`,
`^opaque`, and `^mut opaque` are erased pointer forms used at low-level
boundaries.

`never` marks expressions that never produce a normal value, such as `return`,
`break`, `continue`, and calls to functions returning `never`. Never expressions
may be used where ordinary values are expected because control flow does not
continue to the use site. `never` may be used as a function return type or
function pointer return type. It is not a valid variable, field, parameter, or
array element type.

### 4.2 Pointers

Ptr types:

```nia
&T
&mut T
^T
^mut T
```

`&T` is a read-only object pointer. `&mut T` is a writable object pointer.
`^T` is a read-only volatile object pointer. `^mut T` is a writable volatile
object pointer. Whitespace is insignificant: `& T` parses as `&T`, and `^ T`
parses as `^T`, not as different pointer kinds.

Pointers are ordinary values. Nia has no borrow checker. Read-only and writable
pointers are different types. Ptr conversions must be explicit:

```nia
let mut addr = ptr as usize;
let mut ptr2 = addr as &u8;
```

Address-of and dereference syntax:

```nia
let mut value = 1;
let mut p = &value;
let mut mp = &mut value;
let mut x = p.*;
mp.* = 1;
```

Dereferencing `^T` or `^mut T` has the same source shape as ordinary pointer
dereference, but the memory access is volatile. A read through a volatile
pointer emits a volatile load. A write through `^mut T` emits a volatile store.
`^T` cannot be written through.

```nia
fn read_reg(reg: ^u32) u32 {
    reg.*
}

fn write_reg(reg: ^mut u32, value: u32) () {
    reg.* = value;
}
```

`&place` takes a read-only pointer to a place. `&mut place` takes a writable
pointer to a writable place. Identifiers, field access, array indexing, slice
indexing, and pointer dereference may be places. Field access and indexing into
an aggregate value inherit place-ness from their left-hand side. Indexing a
pointer or slice value is instead an indirect place: the pointer or slice
expression need not itself be a place. Its pointee mutability determines whether
the indexed place is writable.

When the operand is a typed value expression rather than a place, address-of
materializes a block-scoped temporary object and returns a pointer to that
temporary. The temporary has the expression's runtime value type. `never`
expressions cannot be materialized; unit expressions may be materialized as
zero-sized temporaries.

When the pointee type is a trait name, `&Trait[...]` and `&mut Trait[...]`
denote trait object pointers, not thin object pointers. A trait object is a Nia
fat pointer carrying an object pointer plus implementation metadata. Bare
`Trait[...]` remains a trait type for bounds and projections; it is not a valid
value, field, parameter, or array element type.

```nia
trait Source {
    type Item;
}

fn consume(source: &Source[Item = i32]) () {}
```

Trait object syntax uses the same bracket list as trait bounds: positional trait
arguments come first, followed by associated type bindings. Binding names must
be associated types declared by the trait, and a single object type may not bind
the same associated type more than once.

Materialized rvalue references are ordinary block-scoped temporaries. Bind a
value to local or global storage when a stable address with a chosen lifetime is
needed:

```nia
fn make_i32() i32 {
    42
}

let mut point = Point { x: 10, y: 20 };
let mut p: &Point = &point;
let mut temp = &Point { x: 1, y: 2 };
let mut answer = &42i32;
let mut returned = &make_i32();

let hello = b"hello\0";
_ = &hello[0];
```

### 4.3 Arrays

Fixed-length array type:

```nia
[T; N]
```

Array length may be written explicitly in type syntax, or inferred with `_` when
an array literal provides the element count:

```nia
let mut xs: [i32; _] = [1, 2, 3];
let mut name: [u8; _] = b"nia";
```

`[T; _]` is only valid in contexts initialized by an array literal or string
literal. After inference the real type is `[T; N]`.

Array literals:

```nia
[1, 2, 3]
[b'n', b'i', b'a', b'\0']
```

When an array literal initializes a binding without a type annotation, the
binding type is inferred from the literal:

```nia
let mut xs = [1, 2, 3]; // [i32; 3]
```

Array inference is expression-general. Every element constrains one shared
element type, while the literal shape supplies the length. An expected array
type is applied to every element when one is available:

```nia
let mut a = [1i64, 2, 3]; // [i64; 3]
let mut b = [1, 2i64, 3]; // [i64; 3]
consume_i64_array([1, 2, 3]);
```

A numeric suffix is a type constraint, not a conversion, and may appear on any
element. Unsuffixed numeric literals are defaulted only after explicit suffixes
and independently typed expressions have been considered. Incompatible
constraints remain an error; for example, `[1u8, 2u64]` does not choose a wider
integer type.

Context-dependent expressions such as `null` are checked after another element
or the expected type establishes the shared element type. An empty array has no
such constraint and therefore requires an expected type:

```nia
let mut empty: [i32; 0] = [];
let mut unknown = []; // error: element type cannot be inferred
```

Array literals have no expression-level type prefix. Use an expected type or an
element constraint instead. Borrowed temporary arrays use the same inference
path as all other array expressions:

```nia
let mut s = &([1i32, 2, 3])[..];
```

Repeated array literals use semicolon syntax:

```nia
[0; 4]
[1; 8]
```

The left side is the repeated value. The right side is the repeat count and must
be a compile-time `usize` value. If the expected array type has an explicit
length, the repeat count must match it.

Arrays may be nested:

```nia
let mut matrix: [[i32; 3]; 2] = [
    [1, 2, 3],
    [4, 5, 6],
];

let mut zeros: [[i32; 3]; 2] = [[0; 3]; 2];
```

Array indexing:

```nia
let mut x = arr[0];
arr[1] = 42;
```

Array length is part of the type. Ordinary array values do not decay
implicitly; slice views come from array pointers or from the string-family
literal rules described below.

### 4.4 Slices

Slices are length-carrying references. Slice types are written only through
reference forms:

```nia
&[T]
&mut [T]
```

`&[T]` is a read-only contiguous range. `&mut [T]` is a writable contiguous
range. The language does not expose slice fields; an implementation may represent
a slice as `{ ptr, len }`, where `ptr` points to the first element and `len` has
type `usize`.

Slices may be constructed by combining range indexing with address-of:

```nia
let mut hello = "hello";
let mut s = &hello[..];       // &[char]
let mut t = &hello[0..2];     // &[char]
let mut u = &hello[0..=1];    // &[char]
let mut v = &hello[1..];      // &[char]
let mut w = &hello[..3];      // &[char]
let mut x = &hello[..=3];     // &[char]
```

Writable slices use `&mut` and require a writable base place:

```nia
let mut xs: [i32; 4] = [1, 2, 3, 4];
let mut s = &mut xs[1..3]; // &mut [i32]
s[0] = 10;
```

Bare range indexing is not a value expression:

```nia
xs[..]; // error; use &xs[..] or &mut xs[..]
```

An array pointer may be implicitly converted to a full-range slice when the
expected type is exactly `&[T]` or `&mut [T]`. `&[T; N]` converts to `&[T]`;
`&mut [T; N]` converts to `&mut [T]`, and may also be used where read-only
`&[T]` is expected.

```nia
fn read(xs: &[i32]) i32 {
    xs[0]
}

fn write(xs: &mut [i32]) {
    xs[0] = 10;
}

let mut arr: [i32; 3] = [1, 2, 3];
let mut ro: &[i32] = &arr;
let mut rw: &mut [i32] = &mut arr;
read(&arr);
write(&mut arr);
read(&[1, 2, 3]);
write(&mut [1, 2, 3]);
```

Array literals can still be used by taking their address. The usual rvalue
materialization rule used by address-of creates a block-scoped temporary array.
Ordinary array values do not convert directly:

```nia
let mut arr: [i32; 3] = [1, 2, 3];
read(arr);      // error
read([1, 2, 3]); // error
```

String and byte string literal expressions have type `&[char; N]` and `&[u8; N]`.
When a slice is expected, the ordinary pointer-array-to-slice coercion can
produce `&[char]` or `&[u8]`. Method resolution applies the same coercion when
`&[T; N]` or `&mut [T; N]` has no matching method but the corresponding slice does.
Methods defined for the fixed-length array remain more direct and take
priority. The selected method's receiver kind controls the final coercion, so a
read-only array pointer cannot call a mutable slice method.

Range forms:

```nia
start..end
start..=end
start..
..end
..=end
..
```

Range forms also exist as structural types:

```nia
usize..usize
usize..=usize
usize..
..usize
..=usize
..
```

There is no nominal built-in `Range` type. A range type is identified by its
shape and, for bounded forms, by one integer bound type. `T..U` and `T..=U`
require `T` and `U` to be the same integer type.

Range expressions are runtime values with these structural range types. Built-in
slice construction requires every explicit range bound to have type `usize`;
unsuffixed integer literals in slice bounds are inferred as `usize`. Omitted
slice bounds are interpreted by the slice operation, not by rewriting the range
value to `0` or `usize::MAX`.

Bounded range values expose their present bounds through inherent compiler-backed
methods. `a..b` and `a..=b` provide both `start()` and `end()`; `a..` provides
only `start()`; `..b` and `..=b` provide only `end()`; `..` provides neither.
Each method returns the range's bound type. These accessors have no public trait
identity or associated-type projection. An ordinary visible extension method
with the same name takes priority over the compiler fallback. Range values do
not implement `Len`.

Nia does not provide built-in runtime bounds checks. The programmer is
responsible for ensuring that the selected memory range is valid.

With `std::slice` loaded, slices provide checked library operations for
ordinary data-dependent access. `get(index)` and `getMut(index)` return an
optional element reference. `first`/`firstMut` and `last`/`lastMut` do the same
for the endpoints. `getRange(start, end)` and `getRangeMut(start, end)` use a
half-open range and return an optional slice; they return `null` when
`start > end` or `end > len`, while `len, len` is a valid empty range. These
methods validate before using the native indexing or slicing operation.

Direct `slice[index]` and `&slice[start..end]` remain the explicit unchecked
language primitives. The standard library does not duplicate them under an
`unchecked` method name. A single read-only checked branch can use
`if slice.get(index) is ?value`; mutable optional references use an explicitly
mutable pattern such as `match slice.getMut(index) { mut ?value => ... }`.

When `T: Eq[T]`, `equals`, `startsWith`, `endsWith`, `find`, and `contains`
operate on complete contiguous element sequences. `find` returns the first
matching element index as `?usize`. An empty needle matches at zero, including
against an empty slice; a needle longer than the receiver does not match.
Mutable slice values can call these read-only receiver methods directly: method
resolution applies the same `&mut [T]` to `&[T]` coercion accepted at ordinary
typed boundaries.

`split(separator)` returns an allocation-free `std::slice::SliceSplit[T]` whose
iterator items are borrowed `&[T]` segments. Matching is left-to-right and
non-overlapping. Leading, trailing, and adjacent separators therefore produce
empty segments; an empty receiver produces one empty segment. An empty
separator performs no split and yields the complete receiver once, because
ordinary element iteration already has the direct `for &item in slice`
spelling. The iterator borrows both slices, so the receiver and separator must
remain valid and unchanged until iteration ends. `SliceSplit` is intentionally
not double-ended: reverse matching of self-overlapping multi-element separators
requires a separate boundary/search model rather than hidden rescanning.

Scalar text provides `replaceAll(allocator, needle, replacement)` on both
borrowed `[char]` and owned `String` receivers. It returns a new owned `String`
and never mutates or consumes the source. Matches use the same left-to-right,
non-overlapping boundaries as `split`; an empty needle performs no replacement
and returns an independent copy. The implementation computes the exact output
length before allocation, allocates once, and returns `mem::Error::OutOfMemory`
without a partial result. `replacement` may borrow from the source because the
source remains unchanged while the independent result is built.

A borrowed text sequence `&[&[char]]` provides
`join(allocator, separator)`. The result is an independent `String`; input
parts and the separator remain borrowed and unchanged. Empty input produces an
empty string, one part produces an independent copy without a separator, and
an empty separator concatenates the parts. Join scans the repeatable input
slice once to compute the exact scalar length and then once to fill a single
allocation. Length overflow and allocation failure both report
`mem::Error::OutOfMemory` without a partial result. A literal collection uses
one contextual annotation:

```nia
let parts: [&[char]; 3] = [&"build", &"λ", owned.text()];
let mut joined = (&parts).join(allocator, &"/").?;
```

`copyFrom(source)` is the ordinary slice copy operation. It copies
`min(receiver.len(), source.len())` initialized element representations,
handles overlapping ranges, and returns that element count. The return value
makes a short destination observable and cannot be ignored implicitly.
`copyFrom` is a shallow value copy; it does not call a user cloning or cleanup
protocol. For `T: Ord[T]`, `compare(other)` compares elements
lexicographically and returns `std::cmp::Ordering`. If their common prefix is
equal, the shorter slice compares less.

The base of a slice construction may be an array, another slice, or a
single-element pointer:

```nia
let mut arr: [i32; 4] = [1, 2, 3, 4];
let mut a = &arr[..];      // len = 4

let mut b = &a[1..3];      // slice from slice

let mut x: i32 = 10;
let mut p = &x;
let mut c = &p[..];        // len = 1
let mut d = &p[0..1];      // len = 1
let mut e = &p[0..12];     // allowed; programmer owns the length claim
```

For `&T` and `&mut T`, an omitted upper bound uses a base length of 1. An
explicit upper bound uses the explicit range length.

`slice[index]` accesses an element. Indexing `&[T]` produces an addressable but
non-writable place. Indexing `&mut [T]` produces a writable place even when the
slice is returned by a call or another value expression; only the referenced
element is assigned, not the slice value.

### 4.5 Optional And Error Union Types

Optional types are written `?T`. `null` constructs the empty value and `?value`
constructs the present value:

```nia
let mut a: ?i32 = ?10i32;
let mut b: ?i32 = null;
```

`null` and `?value` require an expected optional type when the full optional
type cannot otherwise be inferred. When an expected `?T` is present, it is also
the expected type of `value`.

Error union types are written `E!T`, where `E` is the error value type and `T`
is the success value type. `!value` constructs the success case and `error!`
constructs the error case:

```nia
let mut ok: i32!i32 = !10i32;
let mut err: i32!i32 = 2i32!;
```

Both error-union constructors require an expected `E!T` type. A binding such as
`let mut x = !10i32;` is invalid because the error type `E` cannot be inferred from
the success value alone.

The postfix propagation operator `.?` unwraps an optional or error union inside a
function. For `?T`, `value.?` returns `T` on the present path and returns `null`
from the current function on the empty path. For `Source!T`, `value.?` returns
`T` on the success path. If the current function returns `Target!U`, the error
path is propagated directly when `Source` and `Target` are the same type.
Otherwise the compiler requires one applicable
`Source: std::error::IntoError[Target]` implementation and calls
`into_error` only on the error path. Optional propagation requires an optional
function return type and never uses `IntoError`.

```nia
fn read(value: i32!i32) i32!i32 {
    let mut x = value.?;
    !x
}
```

`IntoError` is the standard infallible error-conversion protocol:

```nia
pub trait IntoError[Target] {
    const fn into_error(self) Target;
}
```

The target error type comes from the enclosing function return type, so callers
normally write `operation().?` rather than an explicit conversion. Resolution
uses the ordinary trait solver, including the current function's `where`
predicates and normal impl specificity rules. An unsatisfied or ambiguous goal
is a type error. Conversion is exactly one step: the compiler does not search
for chains such as `Source -> Intermediate -> Target`. The source expression is
evaluated once, the conversion is skipped on success, and the compiler does not
introduce allocation or type erasure. An `IntoError` implementation is expected
to be an infallible error mapping; conversions that add values not present in
the source, such as an operation name or path, remain explicit adapters.
Diagnostics distinguish a missing direct implementation from a malformed
`IntoError` protocol and from a rejected two-step chain. A protocol witness must
provide a value receiver with exactly the `into_error(self) Target` shape; a
source-to-intermediate and intermediate-to-target pair is still rejected rather
than executed as multiple conversions.

Automatic conversion is available during const evaluation only when the
selected `into_error` witness is a `const fn`. Const evaluation performs that
call only on the failure edge, just like runtime lowering; exact-type and
optional propagation do not need a witness. A runtime-only `into_error`
implementation is rejected in const code rather than executed as a fallback.

Error unions also provide the explicit `mapError` extension:

```nia
let mapped = operation().mapError(&\[context] cause: SourceError -> {
    _ = context;
    _ = cause;
    TargetError::Unknown
});
```

`Source!T::mapError` evaluates its receiver once, returns `!value` unchanged
on success, and invokes the borrowed `&Fn(Source) Target` view only on the
error path. The callback is synchronous and cannot be retained by the
operation; it receives no inferred lifetime extension or allocator ownership.
The target error type is inferred from the callback's explicit return type,
including when the callback is an inline closure. `mapError` is an explicit
mapping operation and is not an additional automatic propagation conversion.
Callable-view calls are runtime-only, so `mapError` is not available in const
evaluation.

Fallible recovery uses `orElse`:

```nia
let recovered = operation().orElse(&\[fallback] cause: SourceError -> {
    if canRecover(cause) {
        !fallback
    } else {
        TargetError::Unavailable!
    }
});
```

`Source!Value::orElse` also evaluates its receiver once and skips the callback
on success. On failure, its callback returns `Target!Value`, so it may recover
with a success value or replace the error. The success type remains exactly
`Value`; callbacks that produce another success type are rejected. The result
has one error-union layer, with no implicit conversion or recursive flattening.
Like `mapError`, `orElse` borrows a readonly synchronous callable and is not
available during const evaluation.

For cleanup that must continue after an individual release failure, the
standard library provides `std::error::cleanup::CleanupAccumulator[Failure]`
(re-exported as `std::error::CleanupAccumulator[Failure]`). It keeps the first
failure without allocating and never short-circuits a later `attempt`:

```nia
let mut cleanup = std::error::CleanupAccumulator[ReleaseError]::init();
cleanup.attempt(first.deinit());
cleanup.attempt(second.deinit());
cleanup.finish();
```

`attempt` always evaluates its argument and records only the first error.
`finish` returns that error after all attempts, or `!()` when every attempt
succeeded. The accumulator does not own or detach resources: each cleanup
operation must retain a failed owner so a later call can retry it. Use the
accumulator for teardown/error aggregation, not for ordinary error propagation
where `.?` and `orElse` intentionally short-circuit or recover.

Patterns can destructure optional and error-union values:

```nia
if maybe is ?x {
    x
} else {
    0
}

match result {
    !x => x,
    err! => err,
}

match nested {
    ?!value => value,
    ?err! => err,
    null => 0,
}
```

`?pattern` matches the present optional case and matches `pattern` against the
payload. `null` matches the empty optional case. `!pattern` matches the
error-union success case. `pattern!` matches the error-union error case.
Patterns may be nested, so `?!value` matches an optional present value whose
payload is an error-union success value, while `?err!` matches the nested error
case. Ptr binding forms such as `let &value = ptr;` and `for &value in
items` are binding destructuring forms too, but they are parsed on local/loop
bindings through the irrefutable subset of the same pattern model. `match`
accepts the refutable forms shown above as well as pointer patterns.

Nominal patterns destructure structs:

```nia
let Point { x, y: renamed } = point;

match point {
    Point { x: 0, y } => y,
    Point { x, y: 0 } => x,
    Point { x, y } => x + y,
}
```

A field written as `x` is shorthand for `x: x`; `field: _` discards that
field. Fields may appear in any source order. Every declared field is required
unless the pattern ends with `..`, which explicitly ignores all omitted fields:

```nia
let Point { x, .. } = point;

match point {
    Point { x: 0, .. } => 0,
    Point { x, .. } => x,
}
```

`..` may appear at most once and must be the final named field. It is available
only in nominal aggregate patterns; tuple and slice rest patterns are not part of
the language. Struct patterns use the nominal constructor (`Point { ... }`), not
an anonymous `{ ... }` pattern.

Enum variants may carry tuple or named payloads. Their patterns use the same
payload shape as construction, and the enum owner may be omitted when the
scrutinee supplies an expected enum type:

```nia
match color {
    .Red => 0,
    .Data(value) => value,
    .Resize { width, height } => width + height,
}
```

### 4.6 Structs

Struct declaration:

```nia
struct String {
    ptr: &u8,
    len: usize,
}
```

Struct value construction:

```nia
let mut s = String { ptr: &bytes[0], len: 3 };
```

Every struct literal carries a nominal type prefix. Anonymous construction such
as `let mut p = { x: 10, y: 20 };` is a block expression, not aggregate
construction, and is rejected when a value is expected. A field whose value is
the same-name local may use shorthand: `Point { x, y }` means
`Point { x: x, y: y }`. Inside an `extend` block, `Self { ... }` names the
extended type.

When an expected nominal type is already available, the owner may be omitted:

```nia
let mut p: Point = .{ x: 10, y: 20 };
```

The omitted form is contextual only. A bare `.{ ... }` without an expected
nominal type is rejected, and fields are checked exactly as in an explicit
`Point { ... }` literal.

```nia
fn sum(point: Point) i32 {
    point.x + point.y
}

let mut p = Point { x: 10, y: 20 };
let mut total = sum(Point { x: 1, y: 2 });
```

Nominal construction composes with references and qualified paths:

```nia
let mut p = Point { x: 10, y: 20 };
let mut q = Point { x: 1, y: 2 };
let mut ptr = &Point { x: 3, y: 4 }; // &(Point { ... }) as read-only
```

`const` uses the same nominal construction rule. Const evaluation does not
create a separate anonymous structural type system:

```nia
struct Config { width: usize }
const config = Config { width: 4usize };
const width: usize = config.width;
```

Field access:

```nia
let mut len = s.len;
s.len = 4;
```

Struct field order is source order. Ordinary `struct` uses Nia layout rules.
`extern struct` uses C ABI-visible layout for C interop:

```nia
extern struct CPoint {
    x: i32,
    y: i32,
}
```

### 4.7 Unions

Union declaration:

```nia
union Bits {
    i: i32,
    f: f32,
}
```

Union value construction uses the same aggregate literal syntax as structs, but
exactly one field must be initialized:

```nia
let mut bits = Bits { i: 42 };
```

Union field access is explicit:

```nia
let mut n = bits.i;
bits.f = 1.0;
```

Reading a union field other than the field most recently written is a low-level
reinterpretation operation. The programmer is responsible for ensuring that the
active field and access type are meaningful for the target ABI and program
invariants.

Generic unions are allowed:

```nia
union Slot[T] {
    value: T,
    empty: u8,
}
```

Ordinary `union` uses Nia layout rules. `extern union` uses C ABI-visible union
layout for C interop:

```nia
extern union CValue {
    i: i32,
    f: f32,
}
```

Union size is the maximum field size rounded up to the maximum field alignment.
Every field has offset zero. See `docs/nia-abi.md` for ABI details and C interop
restrictions.

### 4.8 Function Types

Function declaration:

```nia
fn add(a: i32, b: i32) i32 {
    a + b
}
```

If the return type is omitted, it is `()`:

```nia
static mut log_total: i32 = 0;

fn log(value: i32) {
    log_total += value;
}
```

Use `static` rather than `const` for data that must have a stable
address, such as data passed across an explicit foreign ABI boundary.

Function pointer type:

```nia
&fn(i32, i32) i32
```

Callable interface types use a capitalized `Fn`:

```nia
Fn(i32, i32) i32
&Fn(i32, i32) i32
&mut Fn(i32, i32) i32
```

`Fn(Args...) Return` describes the unsized interface shared by concrete closure
states with the same parameter and return types. Omitting `Return` means `()`.
Callable interfaces cannot be variadic. A bare `Fn(...)` has no value layout
and cannot be used as an ordinary value, field, parameter, or array element;
it must be viewed through `&Fn(...)` or `&mut Fn(...)`.

`&Fn(...)` is a readonly callable view and `&mut Fn(...)` is a writable
callable view. Both are `Sized`, non-owning two-word values containing state and
entry metadata. Readonly and writable views have distinct type identity. They
never allocate, free, or extend the lifetime of the referenced closure state.
The concrete field and entry ABI is documented in
[`docs/nia-abi.md`](nia-abi.md#81-callable-interface-and-closure-entry-representation).

A callable view is constructed explicitly by taking the address of a concrete
closure while a callable-view type supplies the expected signature:

```nia
let offset = \[base] value: i32 -> { base + value };
let view: &Fn(i32) i32 = &offset;

let mut counter = \[base] value: i32 -> { base + value };
let mutableView: &mut Fn(i32) i32 = &mut counter;

view(1);
mutableView(2);
```

A closure starts with `\`, optionally followed by an explicit capture list,
then a comma-separated parameter list and `->` followed by exactly one
expression:

```nia
\value -> value + 1
\[offset] value -> value + offset
\[owned, &shared, &mut writable] left, right -> {
    writable.* = left + right;
    owned + shared.* + writable.*
}
```

Capture entries have the forms `[name]`, `[&name]`, and `[&mut name]`, for value,
readonly-pointer, and mutable-pointer capture respectively. Each entry names an
existing local; bind computed values to locals before capturing them. Pointer
captures remain explicit pointers inside the body and therefore use `.*` when
dereferenced. An empty capture list is omitted. The expression after `->` may be
a block expression, so closures need no separate statement-body grammar.

Because the body is a full expression, consecutive closure expressions group
to the right. Currying can therefore be written without parentheses. Captures
remain explicit at every closure boundary; an inner closure that returns with
outer parameters in its state names those parameters in its own capture list:

```nia
let make = \x: i32, y: i32 -> \[x, y] z: i32 -> x * y + z;
let add = make(2, 3);
add(4)
```

Here the outer body is the complete `\[x, y] z -> ...` expression. Omitting
`[x, y]` is not implicit capture: a closure body may only refer to its
parameters, locals, explicit captures, and module or static values. A method
receiver likewise does not cross a closure boundary implicitly. Bind `self` to
a named local outside the closure and capture that local when it is needed.

The parameter and return types must match the closure signature structurally.
Taking a mutable address may construct either a writable or readonly view;
taking a readonly address cannot construct a writable view. The conversion is
attached to the address expression itself: storing `&closure` in an
intermediate pointer and later assigning that pointer to `&Fn(...)` does not
perform an implicit conversion. Calling a view uses ordinary call syntax.

#### Closure parameter inference

Closure parameters may omit their type when the surrounding expression gives a
callable signature or when later calls provide enough constraints:

```nia
fn apply(callback: &Fn(i32) i32, value: i32) i32 {
    callback(value)
}

fn main() i32 {
    let identity = \value -> { value };
    apply(&identity, 1)
}
```

The body checker collects constraints through locals, calls, nested closures,
tuples, pointers, conditionals, assignments, and ordinary callable shapes
before checking the body with the resolved signature. Explicit parameter type
annotations remain valid and act as constraints; the return type is inferred
from the expression and callable context. Conflicting constraints are
diagnosed. A closure is monomorphic: using one closure value
with incompatible argument types is a type error, rather than implicit
let-polymorphism. If no callable context or body constraint determines a
parameter type, the compiler reports that parameter as unresolved and an
annotation or a callable context is required. Inference is function-local and
temporary; unresolved inference identities never enter semantic facts, cached
queries, ABI/layout data, or persisted types.

Inference cannot invent a type that appears in neither the closure body nor
its callable context. For example, a callback that constructs only the success
side of `Error!Value` leaves `Error` unconstrained; annotate the receiving
binding or otherwise provide an expected callable/result type. This context is
the replacement for a separate closure return annotation.

Callable views are distinct from `&fn(...)`. The latter is the existing thin,
one-word function pointer and carries no state. A no-capture closure may be
converted directly to that thin pointer when a matching `&fn(...)` expected
type guides a readonly address expression:

```nia
let increment = \value: i32 -> { value + 1 };
let pointer: &fn(i32) i32 = &increment;

pointer(2);
```

The parameter and return types must match structurally. Only the direct
`&closure` form performs this conversion; `&mut closure` and an intermediate
closure-state pointer do not. A closure with any capture cannot become a thin
function pointer because its entry requires state. Such a conversion is an
error and should instead use a matching `&Fn(...)` callable view. This bounded
no-capture conversion does not make `&fn(...)` and `&Fn(...)`
representation-equivalent type families.

Native LLVM emission implements this conversion with a generated zero-state
adapter: the adapter has the ordinary thin function-pointer ABI and supplies a
private state token when it calls the concrete closure entry.

Callable views are non-owning and stack-backed by default. A view created from a
concrete closure must remain within the lexical lifetime of that closure state:
it may be called and copied into local aggregates, but it cannot be returned,
stored through a pointer or global, passed to a call that may retain it, or
assigned into an outer scope after the closure state's scope ends. The compiler
closure escape stage follows this provenance through direct function summaries;
function-pointer, dynamic-dispatch, and unknown calls are treated as potentially
retaining. No allocator-backed escaping owner is implied by `&Fn` or `&mut Fn`.
If a closure captures a pointer whose storage is a local or temporary stack
address, the closure state likewise cannot be returned, stored through memory,
or passed to a retaining call. Ordinary raw-pointer flow is outside this rule;
the restriction begins when the address becomes part of closure state.

The standard library provides an explicit owner for callers that need a
callable view to outlive the lexical scope of its closure state. `mem::Allocator`
allocates raw storage, while `Allocator::allocValue[T](value)` constructs an
`mem::Allocated[T]` handle containing the typed storage pointer plus the
returned allocator Block layout and release range required to free the original
block. The Block layout remains distinct from `Layout::of[T]`: a custom
allocator may attach real storage to a zero-sized typed value.
The operation is library code: the compiler does not select an allocator,
insert heap operations, or attach an implicit destructor to `T`. A caller may
obtain `valueMut()`, create the explicit callable view, and move that view into
`Allocated::intoCallable`, producing `mem::CallableAllocation[V]` for a sized
callable-view type such as `&mut Fn(i32) i32`:

```nia
let mut allocated = allocator.allocValue(\[base] value: i32 -> { base + value }).?;
let mut state = allocated.valueMut();
let callback: &mut Fn(i32) i32 = &mut state.*;
let mut owner = allocated.intoCallable(callback);
let result = owner.callback()(8);
owner.deinit(&mut allocator).?;
```

`CallableAllocation` is still an ordinary value. It carries the original
allocator release range through the explicit owner transfer; it does not make
the callable view immortal, infer ownership for captured pointers, or make
allocation failure disappear. `deinit` is explicit and must receive the allocator that
produced the block; cleanup errors remain typed. Copying the owner does not
duplicate the block: copies alias the same allocation, so callers must arrange
that only one logical owner performs `deinit`. A view returned by `callback()`
becomes invalid when that owner is released. The raw storage contract is
caller-managed because Nia has no ownership checker. `allocValue`
evaluates the value before the allocator call; this is an abstract
value-construction rule, not a requirement to materialize a temporary closure
object and copy it to the block. LLVM may keep captures in SSA and store them
directly into the caller-provided destination, although no unoptimized ABI
boundary promises zero-copy behavior.

A function declaration name is a function item, not an ordinary runtime value.
Function items cannot be used bare:

```nia
fn add(a: i32, b: i32) i32 {
    a + b
}

let mut f = add; // error
```

Addressing a function item with `&` creates a function pointer:

```nia
let mut f = & add;                  // &fn(i32, i32) i32
let mut g: &fn(i32, i32) i32 = & add;   // allowed
```

Generic functions must be explicitly instantiated before taking a function
pointer:

```nia
fn id[T](x: T) T {
    x
}

let mut f = & id[i32]; // &fn(i32) i32
let mut g = & id;      // error
```

`& function_item` is a specific function-item address rule. It does not
require the function item to be a place. `&function_item` is not allowed.

## 5. Declarations

### 5.1 Attributes

Attributes are AST marks written before an item, statement, or aggregate field:

```nia
@[link_name("runtime_start")]
extern fn start(argc: i32) i32;

struct Header {
    @[offset(0)]
    magic: u32,
}
```

Attribute syntax is `@[name]` or `@[path.name(args...)]`. Attribute names use
identifier path segments separated by `.`. Attribute arguments use normal
expression syntax and are stored with the AST node.

`@[if condition]` is the language-defined conditional compilation attribute.
It may be attached to items and statements. The condition language is separate
from ordinary Nia expressions and from `const` evaluation. It accepts boolean,
integer, and string literals; names `arch`, `vendor`, `os`, `env`, `abi`,
`endian`, and `pointer_width`; unary `not`; binary `and`, `or`, `==`, and `!=`;
and parentheses.

```nia
@[if os == "linux" and arch == "x86_64"]
pub(pkg) module freestanding;

fn word() usize {
    @[if pointer_width == 64]
    return 8;
    4
}
```

The whole file must still parse, so inactive declarations and statements must
be syntactically valid Nia. After parsing, inactive conditional items and
statements are removed for the active target before later semantic phases.
Invalid names, types, imports, or calls in inactive code are not diagnosed for
that target. Multi-target validation is expected to run the compiler for each
target a project supports.

Attributes are the only source syntax introduced by `@`. A bare `@foo` is not
an expression form; compiler-backed functions are declared by `std::builtin`
and called through ordinary paths such as `std::builtin::size[T]()`. The
standard-library declarations use `@[builtin(...)]` to identify their compiler
contract, while every AST attribute uses the bracketed `@[...]` form.

The compiler also injects a reserved `builtin` module root. It is a normal
module for name resolution and imports, but its source is generated from the
active target rather than read from disk:

```nia
using builtin;

const word_bits: usize = builtin::pointer_width;
const target_os = builtin::os;
```

The initial `builtin` surface exposes `arch`, `vendor`, `os`, `env`, `abi`,
`endian`, and `pointer_width` as public const values. These values share the
same target facts used by `@[if ...]`.

The parser accepts attributes on top-level items and on `struct`/`union` fields.
An unknown attribute is reserved and has no language-defined effect until this
specification assigns one. Unknown attributes do not change visibility, ABI,
layout, symbol names, type checking, or code generation.

ABI selection is still expressed by declarations. In particular, Nia does not
use `@[repr(C)]`: `extern struct` and `extern union` are the C ABI aggregate
forms. Nia also does not currently define `@[export]`; C ABI symbol definitions
are written as `extern fn` definitions.

### 5.2 Functions

```nia
fn name(param: Type, other: Type) ReturnType {
    body
}
```

When a function returns a non-`()` type, the tail expression of its body block
is the return value:

```nia
fn square(x: i32) i32 {
    x * x
}
```

Explicit early return is allowed:

```nia
fn abs(x: i32) i32 {
    if x < 0 {
        return -x;
    }

    x
}
```

### 5.3 Extern Declarations

`extern` uses an external ABI and external symbol name. Item modifier order is
visibility first and `extern` second:

```nia
pub extern fn foreign_log(message: &u8);
```

`extern pub fn` is not valid syntax.

An `extern fn` without a body declares an external C ABI symbol:

```nia
pub extern fn foreign_log(message: &u8);
```

An `extern fn` with a body defines a C ABI-visible symbol in the current module:

```nia
pub extern fn add(a: i32, b: i32) i32 {
    a + b
}
```

Both forms use the source function name as the symbol name and do not use Nia
internal mangling.

Extern global bindings declare external symbols:

```nia
extern static errno: i32;
extern static mut global_counter: usize;
```

Extern functions default to return type `()` when no return type is written.
Variadic functions are only allowed as body-less `extern fn` declarations.

Nia does not provide `extern { ... }` blocks or explicit ABI strings. All
`extern` functions, globals, and structs use the C ABI.

### 5.4 Type Aliases

```nia
type Byte = u8;
type CString = &u8;
```

Type aliases do not create new nominal types.

### 5.5 Enums

Nia enums define named variants. Variants may be unit-like or carry tuple/named
payloads; unit-like enums may also specify a backing integer type.

```nia
enum Color: u8 {
    Black,
    White,
    Red,
}
```

If the backing type is omitted, it defaults to `i32`:

```nia
enum Color {
    Black,
    White,
}
```

Enum variants are accessed through the enum namespace:

```nia
let mut c = Color::Black;
```

In an expression whose expected type is an enum, the owner may be omitted:
`let mut c: Color = .Black;`. Tuple and named payload variants use the same
form, such as `let event: Event = .Data(42);` and
`let event: Event = .Resize { width: 1, height: 2 };`.

If no explicit value is written, variant values start at `0` and increase by
one. Explicit integer values are allowed:

```nia
enum ErrorCode: i32 {
    Ok = 0,
    NotFound = 404,
    Internal = 500,
}
```

Enum values may be explicitly cast to their backing integer type. Integers do not
implicitly mix with enums. Closed enums do not allow integer-to-enum casts,
because not every backing integer is necessarily a named variant.

Open enums use a final `_` marker in the enum body:

```nia
enum Flag: u32 {
    A,
    B,
    _,
}
```

`_` is not a variant, cannot be written as `Flag::_`, cannot have a value, and
must be the last item in the enum body. An open enum means every value of the
backing integer type is a valid value of the enum, including unnamed values.
Integers may be explicitly cast to open enums:

```nia
let mut flag: Flag = 3 as Flag;
```

`match` is the canonical multi-arm matching expression. It matches scalar,
enum, and nominal struct values and may recursively destructure their fields,
optional values, and error-union values:

```nia
let mut value = match c {
    Color::Black => return 0;
    Color::White => 1,
    Color::Red => 2,
};
```

As with `if`, a `match` may also be used as an expression statement. `match`
has no fallthrough. `_` is the default arm:

```nia
match code {
    ErrorCode::Ok => return 0;
    _ => return 1;
}
```

An arm may list multiple patterns separated by commas. Integer switches also
support closed range patterns with both endpoints present:

```nia
match value {
    0, 1 => return 0;
    2..5 => return 1;   // 2, 3, 4
    5..=7 => return 2;  // 5, 6, 7
    _ => return 3;
}
```

Open-ended match range patterns are not supported; use `_` for the fallback
case. Range pattern endpoints must be compile-time integer constants. Empty
ranges are rejected. Partially overlapping ranges are allowed when they still
match new values; a pattern wholly covered by earlier arms is unreachable.

Optional and error-union patterns use the same recursive matcher:

```nia
match value {
    ?x => x,
    null => 0,
}

match result {
    !x => x,
    err! => err,
}

match nested {
    ?!value => value,
    ?err! => err,
    null => 0,
}
```

`?pattern` matches the payload of a present optional. `!pattern` matches the
success payload of an error union. `pattern!` matches the error payload of an
error union. `null` matches the empty optional case. `_` and a bare binding are
catch-all patterns. These patterns may nest across optional and error-union
layers, and pointer patterns such as `&value` use ordinary Nia pointer-copy
semantics.

Struct patterns use one nominal field syntax. Field shorthand binds a same-named
local, while an explicit subpattern can rename, discard, or recursively match the
field:

```nia
match point {
    Point { x: 0, y } => y,
    Point { .. } => 0,
}
```

Without a terminal `..`, all fields are required. Duplicate or unknown fields
are always rejected. Omitted fields under `..` are wildcards, so `Point { .. }`
is irrefutable for `Point`.

A bare identifier in a pattern always introduces a binding. Named constant and
enum value patterns must be syntactically explicit: use a qualified path such as
`Color::Red`, or parenthesize an expression such as `(local_constant)`. This
rule does not depend on capitalization or name-resolution results. An arm may
list multiple alternatives only when none of them binds a value, because every
entry edge to an arm body must define the same locals.

Match expression arms must produce compatible value types unless an arm exits
through `return`, `break`, or `continue`. Every `match`, including one used
only for effects, must be exhaustive; write `_ => {}` when intentionally doing
nothing for remaining values. Exhaustiveness is computed across recursive
product patterns rather than independently per field, and diagnostics include
one missing-pattern witness. Open enums require `_`, even if every currently
named variant is covered.

### 5.6 Let And Const Bindings

`let` is an immutable binding, not a general compile-time execution mechanism
and not macro substitution.

```nia
let name = "nia";
let mask: u32 = 0xff;
```

Use `const` for named compile-time values:

```nia
const size: usize = 16;
```

Local `let` bindings cannot be assigned after declaration:

```nia
let x = 1;
x = 2; // error
```

`let x: T;` is a valid local declaration. Like `let mut x: T;`, it creates
uninitialized automatic storage of type `T`; the difference is that the binding
cannot be assigned and cannot form writable `&T`:

```nia
let p: Point;

p.inspect();     // allowed if the receiver is Point or &Point
_ = &p;          // allowed
p.init();        // error if init requires &mut Point
p = { };         // error
_ = &mut p;      // error
```

Nia does not perform definite initialization analysis. Reading uninitialized
storage is the programmer's responsibility. `let` only controls binding-level
assignment and whether a writable pointer may be taken from the binding; it does
not provide deep immutability and does not prove that the value was initialized.

`static` declarations require explicit types when they have no initializer.
Non-extern uninitialized `static` declarations create static storage initialized
to zero. Extern `static` declarations without initializers only declare external
symbols.

Top-level `static` creates immutable global static storage. Implementations
should place it in read-only data where possible:

```nia
static hello: [u8; 7] = b"hello\n\0";
```

Top-level `static` initializers must be expressible as static initialization data.
They do not execute arbitrary compile-time programs:

```nia
static a = 1 + 2;           // allowed: integer static expression
static hello: [u8; 3] = b"hi\0"; // allowed: byte-array static data
static p = &hello[0];       // allowed: global static address
const lanes: u8x4 = std::builtin::splat[u8x4](3);
static laneCopy: u8x4 = lanes; // allowed: named const SIMD data
const pair: (i32, bool) = (7, true);
static pairCopy: (i32, bool) = pair; // allowed: named const tuple data
static bad = { 1 + 2 };     // error: block execution is not static data
```

Contexts requiring compile-time values, such as non-literal array lengths, read
`const` bindings rather than `static` storage.

`const` creates a compile-time value binding with no runtime storage and no
address:

```nia
const width: usize = 4;

fn first_value() i32 {
    const local_width: usize = width;
    let mut xs: [i32; local_width] = [1, 2, 3, 4];
    xs[0]
}
```

`const` may appear at module, associated-value, and local binding positions. A
`const` binding must have an initializer. Its initializer must be evaluable
with the current compile-time value evaluator. Current compile-time values cover
integer, boolean, string, array, struct, and ABI-scalar union literal values;
struct and supported union field access;
casts that preserve the underlying value; boolean `not`, `and`, and `or`;
equality comparisons between matching primitive const value kinds; simple
integer arithmetic and bit operations; and references to other visible
`const` bindings. Cyclic `const` dependencies are errors.

Top-level `pub const` bindings participate in normal module visibility and
may be used through imports:

```nia
// config.nia
pub const width: usize = 4;

// main.nia
using root::config;
let mut xs: [i32; config::width] = [1, 2, 3, 4];
```

Taking the address of a `const` binding is invalid because it has no runtime
storage.

Struct const values are ordinary field-keyed const values:

```nia
struct Point {
    x: usize,
    y: usize,
}

const p: Point = Point{x: 2, y: 3};
const width: usize = p.x + p.y;
```

Const union values preserve target storage semantics rather than behaving like
single-field structs. Integer, floating-point, `bool`, and `char` fields use the
artifact target's widths and endianness. Fixed arrays recursively composed from
supported types encode each element in array order with the same target rules.
Nominal structs recursively composed from supported fields use their substituted
artifact layout, including field reordering and offsets. Their field bytes are
initialized, while inter-field and trailing padding remains uninitialized.
Nested unions recursively preserve the same raw bytes and initialization state;
they do not acquire an active-field tag or field-construction identity.
SIMD vectors preserve lane order and artifact endianness. Numeric lane payloads
are stored consecutively, boolean mask lanes are bit-packed with lane 0 as the
least significant bit, and vector allocation-tail padding remains
uninitialized. Target-sized integer lanes use the artifact pointer width.
Reading the struct itself decodes only its fields; reinterpreting it through a
union field that covers padding is an uninitialized-storage error. Reading any
other union field otherwise decodes the same bytes as runtime union access. A
write changes only the bytes occupied by the selected field. Bytes outside the
field used for initial construction are uninitialized rather than implicitly
zero. Invalid `bool` or `char` representations inside an array or struct are
diagnosed at the containing element or field.

The current const ABI codec supports scalars, pointers, fixed arrays, SIMD
vectors, nominal structs, and untagged unions recursively composed from those
types.
Nominal type and const arguments are substituted by declaration parameter kind
and order; semantically equal const expressions and literal arguments identify
the same concrete field type. Pointer storage is represented by a typed
artifact-width relocation, never by encoding a host address into the byte
buffer. Reading a pointer field requires one exact relocation; integer bytes
cannot fabricate a pointer, and reading relocation-bearing storage through a
scalar or vector field is rejected. Relocations survive recursive aggregate and
nested-union copies and participate in pointer lifetime validation. Partially
overwriting a relocation leaves its unwritten fragment uninitialized; it does
not expose placeholder bytes as an integer representation. Relocations retain
their promoted-allocation identity and typed pointee through body and function
IR, including imported reachability and artifact fingerprinting. A relocation
to a scalar, fixed array, string, byte-string, SIMD-vector, nominal struct, or
untagged union constant materializes at runtime through one readonly allocation
per source origin; equal contents at distinct origins do not imply pointer
equality. Nested relocations in fixed arrays, structs, and unions are preserved
as pointer-valued artifact relocations at their ABI offsets. Initialized bytes
retain their values, while union and struct padding remains uninitialized; no
host address is serialized into constant storage. Zero-sized promoted
allocations retain the same source-origin pointer identity even though their
pointee type has no value bytes; this does not change the pointee's size or
alignment. Readonly const array, string, and slice storage uses the same rule:
frozen pointers retain their expression origin, while compiler-provided string
constants use their defining const item. Repeated runtime uses therefore share
one allocation, and equal contents from distinct definitions do not imply
pointer equality. Imported generic `const fn` calls preserve that provenance
when pointer-bearing union values cross arguments and returns; generic
instantiation does not create a new allocation identity for provenance carried
into or returned unchanged from the call. A promotion whose source expression
is itself inside a generic function template is instead owned by the concrete
function instance, because substitution may change its pointee type or
initializer. A `static` address is distinct from promoted readonly const
storage even when both allocations have the same contents and defining module.
Other unsupported field kinds are still rejected in a `const fn` declaration.
Ordinary runtime unions retain the full semantics described in section 4.7.

Conditional source selection is expressed with `@[if ...]`, not with
`const`. `const` is reserved for compile-time values and functions.

`const fn` declares a function that is valid during constant evaluation. It is
not a const-eval-only function kind: the same function may be called from
runtime code, where it is lowered and executed as an ordinary runtime
function. A constant expression may call `const fn`, but may not call an
ordinary `fn`. This gives one implementation a dual-stage contract rather than
separate compile-time and runtime definitions.

The const-capability contract is checked at the declaration, independently of
whether the function is used. Tail expressions, explicit returns, expression
statements, and all source branches must use const-capable operations and agree
with the declared types. Branch selection controls evaluation, not semantic
validity: an ordinary `fn` call in an unselected branch is still invalid, while
a const-capable operation such as `std::builtin::error` may remain in a branch
that is not selected for a particular call.

`std::builtin::trap()` is a dual-stage const-capable operation returning
`never`. Reaching it during constant evaluation produces a source diagnostic at
the call; reaching the same operation at runtime terminates through the target
trap primitive. Merely declaring it in a branch does not execute it.
`std::builtin::error(message)` remains distinct: it is a const-only operation
whose evaluated const string becomes the diagnostic message.

Constant evaluation may use ordinary `let mut` locals for loops, accumulation,
aggregate construction, and destructuring. Const-function bindings use the same
irrefutable pattern rules as runtime bindings. A type annotation constrains the
whole pattern, and `let mut PATTERN` makes every local bound by that pattern
mutable:

```nia
const fn adjustedSum(point: Point) i32 {
    let mut Point { x, y }: Point = point;
    x += 1;
    x + y
}
```

Each call receives fresh local state. Taking the
address of a local creates a transient place pointer to that call's allocation;
dereferencing it reads the current allocation value rather than a snapshot, and
pointer equality compares allocation plus projection identity rather than
pointee contents. That state cannot modify a module or associated `const`, has
no host address or cross-query identity, and a pointer to it cannot escape into
the returned const value:

```nia
const fn width() usize {
    let mut value: usize = 0;
    while value < 4 {
        value += 1;
    }
    value
}

const arrayWidth: usize = width();
```

A pointer received from the caller may be returned unchanged because its
allocation outlives the callee frame. Taking a pointer to a value expression
inside a function or block creates a temporary allocation owned by that scope;
it may be passed to a nested call and used while the scope is live, but it may
not escape. A module or local `const` initializer may instead directly promote a
read-only value expression into a frozen allocation with stable source
provenance. Writable const promotion is rejected. These rules apply recursively when pointers
are stored in arrays, structs, optionals, error unions, or enum payloads.
Mutable write-through remains outside the current const pointer capability;
mutable receiver writeback is a separate call contract.

Pointer-containing untagged unions are const-capable through typed relocations.
A pointer is never converted to host address bytes or a fabricated compile-time
integer. Relocation-bearing union values cannot yet cross into runtime code;
that boundary requires IR and backend relocation materialization.

Calls are staged by their use site:

```nia
const fn double(value: usize) usize {
    value * 2
}

const arrayLen: usize = double(5); // evaluated at compile time

fn runtimeDouble(value: usize) usize {
    double(value) // emitted as a runtime call
}
```

Function pointer types describe an ordinary runtime call signature; they do not
currently carry a const-callable capability. Runtime code may take the address
of a `const fn` and call it indirectly, and that reference makes the target a
runtime-reachable function. Constant evaluation may call the same definition
directly, but may neither form a function pointer value nor call through one.
Supporting const indirect calls would require an explicit const-callable
function type or an equivalent statically checked capability, not inference
from a pointer value's local origin.

Constant evaluation is resource bounded. One outer evaluation currently has a
1,000,000-step budget and a maximum const-function call depth of 256; an
individual `while` or `loop` is additionally limited to 100,000 iterations.
Nested calls and loops consume the same outer step budget. Exceeding a limit is
a source diagnostic at the active expression or call site, not a runtime stack
overflow or an indefinitely running compiler. These limits constrain compiler
execution and do not make a non-terminating const expression valid.

### 5.7 Static Storage

Top-level `static` declarations create global static storage. `static mut`
declares writable global storage.

```nia
static mut a = 1;

fn bump() i32 {
    a = a + 1;
    0
}
```

Static declarations may infer their type from an initializer or write it
explicitly:

```nia
static mut hello = "hello\n";
static mut counter: i32 = 0;
static banner = b"nia";
```

Non-extern initialized `static` declarations must satisfy static initialization
rules. A bare global value does not automatically become an address:

```nia
static mut target: i32 = 1;
static mut p: &i32 = &target; // allowed
static mut q: &i32 = target;  // error
```

Top-level static declarations are visible inside the same module. Cross-module visibility
is controlled by the module system.

## 6. Local Bindings And Assignment

`let mut` introduces mutable bindings. `let` introduces immutable bindings with
storage. `const` introduces an immutable compile-time value binding with no
runtime storage or address. `const` is itself the declaration keyword, so
`const let` is invalid. There is no `const mut`; mutation during constant
evaluation uses transient `let mut` locals inside a `const fn` or const
initializer block.

Inferred type declaration:

```nia
let mut x = 1;
let mut name = "nia";
```

Explicit type declaration:

```nia
let mut x: i32 = 1;
let mut name: [u8; 4] = b"nia\0";
```

Assignment to an existing place:

```nia
x = x + 1;
arr[0] = 7;
obj.field = 2;
```

Compound assignment:

```nia
x += 1;
x -= 1;
x *= 2;
x /= 2;
```

`let mut` introduces a mutable binding. `let` introduces an immutable binding.
`let mut x: T;` and `let x: T;` are valid
uninitialized declarations. A declaration without an explicit type must have an
initializer.

Local bindings may use pointer destructuring:

```nia
let &x = ptr;
let mut &mut y: &mut i32 = mut_ptr;
let &(left, right): &(i32, i32) = pair_ptr;
```

`let &x = ptr` requires `ptr: &T` and binds `x: T`. `let mut &mut y = ptr`
requires `ptr: &mut T` and binds a mutable local `y: T`. Pointer patterns compose
with other patterns, so `&(left, right)` matches `&(L, R)` and binds `left: L`
and `right: R`.

In `let pattern: Type = value`, `Type` always describes the input matched by the
whole pattern. It therefore constrains both `value` and the recursive pattern;
`let &x: &T = ptr` is the annotated form, while `let &x: T = ptr` is a type
mismatch. This single rule also applies to tuple and nested pointer patterns.
Pointer-destructuring local bindings require an initializer. Local and loop
bindings accept only the irrefutable subset of the pattern language. This
includes exhaustive struct patterns whose recursive explicit fields are all
irrefutable, such as `let Point { x, y } = point;` and `let Point { x, .. } =
point;`; enum-variant patterns and struct patterns containing
value/range/optional cases remain refutable and are rejected in a binding.

## 7. Statements And Semicolons

Nia uses semicolons for statement boundaries.

Statements requiring semicolons:

```nia
let mut x = 1;
x = x + 1;
record(x);
return x;
break;
continue;
```

Block-shaped control flow used as a standalone statement does not need a trailing
semicolon. The recommended rule is:

- ordinary expression statements need `;`;
- `if`, if-pattern expressions, `for`, and `match` used as standalone statements
  do not need `;`;
- a block tail expression does not use `;`.

## 8. Expressions

### 8.1 Blocks

Blocks are expressions:

```nia
{
    let mut x = 1;
    x + 1
}
```

A block with a tail expression has the tail expression type. A block without a
tail expression has type `()`.

### 8.2 If

`if` is an expression:

```nia
let mut result = if score >= 60 {
    "pass"
} else {
    "fail"
};
```

When an `if` expression is used as a value, it must have both branches and the
branches must have compatible types. When `if` is used only for control flow,
`else` may be omitted and the expression type is `()`.

An if-pattern expression performs one refutable match with the
binding/destructuring pattern language:

```nia
if result is !value {
    use(value);
} else {
    recover();
}
```

The matched expression is evaluated once. Bindings are scoped to the successful
branch, and `mut` belongs inside the pattern, for example
`if maybe is mut ?value { ... }`. A non-exhaustive value-producing if-pattern
requires `else`; an effect-only if-pattern may omit it.

Use `match` for multiple refutable alternatives:

```nia
match result {
    !value => use(value),
    err! => {
        return map_error(err)!;
    },
}
```

### 8.3 Loops

Nia has three loop forms: `for-in`, `while`, and `loop`.

Iterator loop:

```nia
for item in iter {
    consume(item);
}
```

`for pattern in expr` requires `expr` to implement the builtin `Iterable`
trait. `Iterable::Iter` must implement the builtin `Iterator` trait, and its
`Iterator::Item` must equal `Iterable::Item`. The loop calls the builtin
`Iterable::iter` provider and then repeatedly calls the builtin
`Iterator::next` provider. It does not perform ordinary method lookup for
methods with those names and does not bind to any standard-library module path.
Types expose collection-specific iteration by implementing `Iterable`; an
`Iterator` also satisfies `Iterable` intrinsically with itself as `Iter`.

Inside a `const fn`, every selected user-provided `Iterable::iter` and
`Iterator::next` witness must itself be declared `const fn`. This is checked
when the containing const function is declared, even when it is unused. The
intrinsic `Iterator: Iterable` adaptation has no function witness to check.
An inherent method named `iter` or `next` does not satisfy this requirement,
because `for-in` dispatches through the builtin traits rather than ordinary
method lookup.

The builtin iteration protocol is:

```nia
trait Iterable {
    type Item;
    type Iter;
    fn iter(&self) Iter;
}

trait Iterator {
    type Item;
    fn next(&mut self) ?Item;
}
```

`Item` may be any ordinary item type, including `&T`, `&mut T`, `?T`, or
`E!T`. Fallible iteration is not a separate `for` protocol: an iterator can
choose `Item = E!T`, and the loop body decides whether to handle or propagate
each item.

The loop pattern may be a value binding, a pointer binding, a mutable pointer
binding, or a discard:

```nia
for x in values {}
for &x in pointer_values {}
for &mut x in mutable_pointer_values {}
for _ in values {}
```

`&x` and `&mut x` are pointer patterns. They require the iterator item type to
be `&T` or `&mut T` respectively, and bind `x` as the pointed-to `T` value.
Use `for x in iter` to bind the iterator item itself without destructuring it.

For-in bindings do not support type annotations. Write the iterator expression
so that its item type is clear:

```nia
using std;

let mut total: usize = 0;
for i in 0usize..len {
    total += i;
}

let mut wide_total: i64 = 0;
for i in 1i64..4i64 {
    wide_total += i;
}
```

Condition loop:

```nia
while a > b {
    a -= 1;
}
```

Infinite loop:

```nia
loop {
    // work
}
```

`break` exits the nearest loop. `continue` starts the next iteration.

Loops are statements and do not produce values.

### 8.4 Defer

`defer` registers an expression to run when the current block exits:

```nia
{
    let mut file = open(path);
    defer close(file);

    work(file);
}
```

The deferred expression may be a call or a block:

```nia
defer {
    flush(file);
    close(file);
};
```

Multiple `defer` statements in the same block run in last-in-first-out order.
Normal block exit, `return`, `break`, and `continue` all run already registered
defers for exited scopes.

The deferred expression is evaluated as an ordinary delayed statement. It must
have type `()` or `never`. If cleanup returns a non-`()` value, discard it
explicitly:

```nia
defer {
    _ = flush(file);
    close(file);
};
```

Control flow inside a deferred expression runs when the deferred expression runs.
`return` may return from the current function, `.?` may propagate from the
current function, and `break` or `continue` may target an enclosing loop when the
`defer` statement is registered inside that loop context. If a deferred
expression changes control flow, that new control flow overrides the exit path
that caused the defer to run.

Nia has no exceptions, so `defer` has no exception-unwind semantics.

### 8.5 Operators

Unary operators:

```text
-       numeric negation
not     boolean not
~       bitwise not
&       writable address
&mut    writable address
&       read-only address
.*      pointer dereference
```

Binary operators:

```text
* / %
+ -
<< >>
< <= > >=
== !=
&
^
|
and
or
```

Assignment operators:

```text
= += -= *= /= %= <<= >>= &= ^= |=
```

Casts:

```nia
value as Type
```

Numeric casts are explicit. `char as u32` is allowed and returns the Unicode
scalar value. Integer-to-`char` casts are not allowed because they would require
a runtime Unicode scalar validity check.

Operator precedence is organized so assignment is lower than logical operators,
while calls, indexing, and field access are higher than unary operators.

Nia has no built-in pointer arithmetic. Convert to an integer type explicitly,
perform the arithmetic, and convert back explicitly when needed.

Arithmetic, bitwise, shift, comparison, and selected unary operators are type
checked through core language operator traits. These traits are always available
by name and are not provided by a source module:

```nia
Add[Rhs]
Sub[Rhs]
Mul[Rhs]
Div[Rhs]
Rem[Rhs]
BitAnd[Rhs]
BitOr[Rhs]
BitXor[Rhs]
Shl[Rhs]
Shr[Rhs]
Neg
Not
BitNot
Eq[Rhs]
Ord[Rhs]
```

Arithmetic, bitwise, shift, `Neg`, and `BitNot` have an `Output` associated
type. For example, `a + b` requires the bound `Lhs: Add[Rhs]` and has type
`[Lhs as Add[Rhs]]::Output`. `Not`, `Eq`, and `Ord` return `bool`.
Generic code that uses an operator must state the needed capability explicitly:

```nia
fn add_same[T](a: T, b: T) T
where T: Add[T, Output = T] {
    a + b
}

fn same[T](a: T, b: T) bool
where T: Eq[T] {
    a == b
}
```

Primitive numeric, integer, boolean, pointer, and enum implementations are
compiler-known only for the operations they support. Lowering represents these
operators as builtin operator calls; backend lowering either keeps a primitive
builtin operator call or dispatches to the visible trait implementation method.
LLVM code generation emits the corresponding primitive operation after generic
instantiation.

### 8.6 Calls, Indexing, Fields, And Methods

Function call:

```nia
add(1, 2)
```

Indexing:

```nia
arr[0]
```

Field access:

```nia
value.field
```

Method selection:

```nia
value.method(arg)
```

Method selection statically resolves to a visible method declared in an `extend`
block for the receiver type. `.` does not imply dynamic dispatch. Field function
pointer calls remain possible; they are used only if no receiver method matches.

Field access and method selection support limited automatic dereference. If `p`
has type `&Point` or `&mut Point`, then:

```nia
p.x
p.move(1, 2)
```

may dereference `p` for field access or receiver matching. This automatic
dereference is limited to fields, methods, and receiver matching. It is not a
general implicit conversion.

Expression statements may not silently discard non-`()` and non-`never` values.
Discard explicitly with `_`. Discarding a `()` expression is also valid:

```nia
vec.push(2);      // error if push returns a non-unit value
_ = vec.push(2);  // allowed
_ = log("done");  // allowed even if log returns ()
abort();          // allowed if abort returns never
```

### 8.7 Compiler-Backed Operations

Nia exposes compiler-backed operations as ordinary declarations in
`std::builtin`. Call sites use normal path, generic-argument, and call syntax;
there is no separate `@name(...)` builtin expression syntax. The compiler
recognizes the standard-library declarations through their internal
`@[builtin(...)]` contract.

The current surface is:

```nia
std::builtin::size[T]()
std::builtin::align[T]()
std::builtin::offset[T]("field")
std::builtin::error("message")
std::builtin::embed("path")
value.len()
range.start()
range.end()
slice.ptr()
slice.ptrMut()
std::builtin::load_unaligned[T](ptr)
std::builtin::splat[Vec](value)
std::builtin::extract(vector, index)
std::builtin::insert(vector, index, value)
std::builtin::bitmask(mask)
std::builtin::ctz[T](value)
std::builtin::clz[T](value)
std::builtin::popcount[T](value)
std::builtin::atomic_load[T](ptr, order)
std::builtin::atomic_store[T](ptr, value, order)
std::builtin::atomic_rmw[T](ptr, op, value, order)
std::builtin::cmpxchg_strong[T](ptr, expected, desired, success, failure)
std::builtin::cmpxchg_weak[T](ptr, expected, desired, success, failure)
std::builtin::fence(order)
std::builtin::asm(std::builtin::AsmConfig {...})
```

`std::builtin::size[T]()` returns the ABI size of `T` in bytes as `usize`.

`std::builtin::align[T]()` returns the ABI alignment of `T` in bytes as
`usize`.

`std::builtin::offset[T]("field")` returns the ABI byte offset of a struct or
union field as `usize`. The field name must be a string literal. For unions,
every field has offset `0`.

`std::builtin::embed("path")` reads a file during compile-time evaluation and
returns its contents as a byte array value. The path argument must be a string
literal and is resolved relative to the source file that contains the call,
not the process working directory. `std::builtin::embed` is only valid in a
`const` expression context; it does not parse or macro-expand the embedded
bytes.

`std::builtin::size[T]()` and `std::builtin::align[T]()` require `T: Sized`.
For concrete layout-known types this predicate is compiler-proven. In generic
code it must be written in the `where` clause:

```nia
fn bytes[T]() usize
where T: Sized {
    std::builtin::size[T]() + std::builtin::align[T]()
}
```

When their type argument is concrete, `std::builtin::size[T]()`,
`std::builtin::align[T]()`, and `std::builtin::offset[T]("field")` are
compile-time known values and may appear in ordinary expressions. `size` and
`align` may also appear in array lengths and static initializers. In generic
code these calls remain layout values until the generic function is
instantiated. A concrete layout-builtin array length participates in
const-generic inference just like a literal or evaluated const expression,
with the same result for compile-time and runtime calls.

`value.len()` calls the ordinary source-defined `Len` trait method. The loader
makes `Len` available as a demand-loaded prelude trait when source uses the
method or names the trait, so the common call requires no explicit import.
`std::builtin` defines ordinary implementations for `[T; N]` and `[T]`. The array
body returns the const generic `N`; the slice body uses the narrow `sliceLen`
representation intrinsic to read runtime slice metadata. The compiler does not
assign `Len` a builtin trait identity. User types implement the same trait with
`const fn len(&self) usize`, usable from both const and runtime calls.

`range.start()` and `range.end()` are inherent compiler-backed operations on
structural range types. They are available only for range shapes that carry the
requested bound and return that bound's integer type. They do not introduce a
trait obligation or an associated `Output` type; ordinary visible extensions
remain higher-priority method candidates.

`slice.ptr()` and `slice.ptrMut()` are inherent compiler-backed projections of
slice data pointers. `ptr()` accepts read-only or writable slices and returns
`&T`; `ptrMut()` requires a writable slice and returns `&mut T`. They introduce
no trait obligation or associated `Target` projection. Ordinary visible
extensions named `ptr` or `ptrMut` remain higher-priority method candidates.

Array values do not expose these methods. An existing `&[T; N]` or `&mut [T; N]`
receiver may use the ordinary array-pointer-to-slice coercion, so
`b"name\0".ptr()` is valid and explicitly produces a pointer to the first
byte. Runtime projection also preserves the slice data pointer for an empty
slice, although dereferencing it is invalid. During const evaluation, non-empty
array-backed slices preserve frozen or place provenance and may use both
methods. Empty const slices are currently rejected because the const pointer
representation cannot yet encode an allocation-base/dangling element pointer;
the evaluator must not fabricate a pointee value.

`std::builtin::load_unaligned[T](ptr)` reads a `T` from a byte pointer with
alignment 1. `ptr` must have type `&u8` or `&mut u8`, and `T` must be `Sized`.
The caller is responsible for ensuring that at least
`std::builtin::size[T]()` readable bytes are available at `ptr`; the operation
only relaxes alignment, not bounds or initialization.

`std::builtin::memcpy[T](destination, source)` and
`std::builtin::memmove[T](destination, source)` copy the common slice prefix as
raw element representation and return `()`. Both require initialized,
readable source elements and writable destination elements. `memcpy` copies
forward and requires the copied ranges not to overlap in a way that changes a
later source element; `memmove` selects a safe direction and permits overlap.
`std::builtin::memset(destination, byte)` fills every byte of a mutable `u8`
slice. These are compiler primitives for std and low-level code. Ordinary code
uses `slice.copyFrom`, whose count result exposes short copies and whose
implementation selects the overlap-safe primitive.

SIMD vector builtins operate on primitive vector types such as `u8x16` and
`boolx16`. `std::builtin::splat[Vec](value)` constructs a vector whose lanes all
contain `value`; `Vec` must be a SIMD vector type and `value` must have its lane
type. `std::builtin::extract(vector, index)` reads one lane, and
`std::builtin::insert(vector, index, value)` returns a copy of `vector` with one
lane replaced. Lane indexes are integer values. An out-of-range index is a
const diagnostic during constant evaluation and traps before accessing a lane
at runtime.

Vector storage uses its native bit width. The store width is the byte-rounded
total lane width, including one bit per boolean mask lane. Its ABI alignment is
the next power of two of the store width, and its allocation size is rounded to
that alignment. `splat`, `extract`, `insert`, and `bitmask` are dual-stage
`const fn` builtins: the same operation may execute during constant evaluation
or lower to runtime SIMD instructions.

Vector comparisons return boolean mask vectors such as `boolx16`.
`std::builtin::bitmask` packs a boolean mask vector into `usize`, with lane 0 in
the least significant bit. It currently supports masks up to 64 lanes.

Integer-vector addition, subtraction, multiplication, and negation apply the
scalar checked operation independently to every lane. If any lane's
mathematical result is not representable in the lane type, the whole vector
operation traps. Boolean mask vectors are not numeric vectors and do not
support arithmetic or negation; their bitwise operations remain available.
Integer-vector division and remainder likewise trap the whole operation if any
lane has a zero divisor, or if any signed lane evaluates `MIN / -1` or
`MIN % -1`. Only after every lane passes these checks does the lane-wise LLVM
operation execute. Integer-vector shifts use a count vector of the same type
and lane count as the left operand; a uniform count is expressed explicitly by
splatting a scalar. Counts are checked per lane, and any negative signed count
or count at least as wide as the lane type traps the whole operation. Left
shift also traps when any lane's mathematical result is not representable.
Right shift is arithmetic for signed lane types and logical for unsigned lane
types. Boolean mask vectors do not support shifts.

Bit-counting builtins operate on integer primitive types.
`std::builtin::ctz[T](value)` returns the number of trailing zero bits,
`std::builtin::clz[T](value)` returns the number of leading zero bits, and
`std::builtin::popcount[T](value)` returns the number of set bits. The argument
and result both have type `T`. `std::builtin::ctz[T](0)` and
`std::builtin::clz[T](0)` are defined to return the bit width of `T`.

Atomic builtins provide the low-level primitive operations behind `std::atomic`.
Their `order` and `op` arguments must be compile-time integer constants. The
standard library exposes named constants for these values:

```text
ordering: Unordered=0, Monotonic=1, Acquire=2, Release=3, AcqRel=4, SeqCst=5
rmw op:   Xchg=0, Add=1, Sub=2, And=3, Nand=4, Or=5, Xor=6,
          Max=7, Min=8, UMax=9, UMin=10
```

`std::builtin::atomic_load[T]` takes `&T` or `&mut T` and returns `T`.
`std::builtin::atomic_store[T]` takes `&mut T` and returns `()`.
`std::builtin::atomic_rmw[T]` takes `&mut T`, applies an atomic
read-modify-write operation, and returns the previous value.
`std::builtin::cmpxchg_strong[T]` and `std::builtin::cmpxchg_weak[T]` return
`null` on success or `?old_value` on failure. `std::builtin::fence(order)` emits
an atomic fence and returns `()`.

The current supported atomic value types are bool, integer, enum, and ordinary
object pointer types whose width does not exceed the target pointer width.
Volatile pointer types are not atomic value types. Floating point, slices,
structs, arrays, and unions are not atomic value types. Legal ordering sets
follow the operation kind: loads allow Unordered, Monotonic, Acquire, and SeqCst;
stores allow Unordered, Monotonic, Release, and SeqCst; read-modify-write and
cmpxchg success orderings allow Monotonic, Acquire, Release, AcqRel, and SeqCst;
cmpxchg failure orderings allow Monotonic, Acquire, and SeqCst and must not be
stronger than the success ordering; fences allow Acquire, Release, AcqRel, and
SeqCst.

`std::builtin::asm(std::builtin::AsmConfig {...})` is the inline assembly escape hatch for syscalls,
special registers, port I/O, CPU instructions, and freestanding runtime glue.
Its argument must be an `AsmConfig` literal. It returns `()`. The config,
input, and output types are compiler contracts without runtime layout:

```nia
fn syscall1(sys_num: usize, arg1: usize) isize {
    let mut ret: isize = 0;
    std::builtin::asm(std::builtin::AsmConfig {
        code:
            b\\syscall
        ,
        outputs: std::builtin::AsmOutputs { rax: ret },
        inputs: std::builtin::AsmInputs {
            rax: sys_num,
            rdi: arg1,
        },
        clobbers: [b"rcx", b"r11", b"memory"],
        options: [b"volatile"],
    });
    ret
}
```

`code` must be a byte string literal. `inputs` and `outputs` are typed literals
that map register classes or fixed registers to expressions or places. `reg`
means a general register class. `freg` means a floating-point register class.
Other field names are fixed registers such as `rax` and `rdi`. Output values
must be assignable places. `clobbers` is an array of byte string literals.
`options` is a byte string literal or an array of byte string literals;
currently only `b"volatile"` is defined.

Inline assembly is target-specific. The compiler communicates with the optimizer
only through explicit inputs, outputs, clobbers, and options; it does not
understand the semantics of the assembly string. Larger startup code, complex
ABI glue, or independently maintainable assembly modules should still be
expressed through `extern` plus object/archive/linker input.

Unknown `@...` forms are reserved.

## 9. Generics

Nia supports type generics.

Generic functions:

```nia
fn id[T](x: T) T {
    x
}
```

Generic structs:

```nia
struct Pair[T, U] {
    first: T,
    second: U,
}
```

Generic type instantiation uses `[]`:

```nia
let mut p = Pair[i32, bool] { first: 1, second: true };
```

Expression generic instantiation also uses `[]` for explicit type arguments in
function-item, method, or other expression positions:

```nia
let mut x = id[i32](1);
let mut f = & id[i32];
```

In expressions, `expr[...]` is parsed uniformly and disambiguated semantically.
It is a generic instantiation if `expr` denotes a generic function, generic
method, or type prefix and the bracket arguments are valid type arguments in
that context. It is indexing if `expr` is an array, slice, or pointer and the
bracket contains a single expression. If the bracket contains `..`, it is range
indexing.

```nia
xs[i]       // index
xs[1..3]    // range index
id[i32](x) // explicit generic function call
```

Generic declarations may use a `where` clause after the parameter list, target
type, or generic parameter list:

```nia
fn eq[T](a: &T, b: &T) bool
where T: Eq[T] {
    a == b
}

struct Box[T] where T: Eq[T] {
    value: T,
}
```

Generics are implemented by monomorphization. Type parameters have no runtime
representation. The generic surface is explicit type parameters on functions,
structs, unions, traits, and methods. Local `let` bindings cannot declare
generic parameters; array length is part of array type syntax.

Omitting the bracket list on a generic function call requests inference for
every generic parameter. Once a bracket list is written, it must contain one
slot for every declared type or const parameter; a shorter explicit prefix is
an error. Write `_` in a slot that should still be inferred:

```nia
fn select[T, U](value: T, context: U) T {
    _ = context;
    value
}
fn count[T, N: usize](values: [T; N]) usize { N }

select[i32, _](1, true)
count[_, _]([1, 2, 3, 4])
```

This makes inference intent explicit and prevents generic declaration order
from becoming an implicit call-site API.

Method generic parameter names must not shadow a generic parameter declared by
the enclosing trait or `extend` target. This keeps the two generic scopes
distinct in signatures and diagnostics; choose a different name for the
method parameter instead.

## 10. Methods

Methods are declared in `extend` blocks. An `extend` block attaches associated
functions and receiver methods to its target type:

```nia
struct Vec2 {
    x: i32,
    y: i32,
}

extend Vec2 {
    fn len2(&self) i32 {
        self.x * self.x + self.y * self.y
    }
}
```

Method call:

```nia
let mut v = Vec2 { x: 3, y: 4 };
let mut n = v.len2();
```

Methods are ordinary functions associated with a concrete nominal type. Receiver
forms:

```nia
fn method(&self, ...)
fn method(&mut self, ...)
fn method(self, ...)
fn method(...)
```

Receiver meaning:

- `&self` passes the receiver as read-only `&Type`;
- `&mut self` passes the receiver as writable `&mut Type`;
- `self` passes the receiver by value;
- no `self` means the function is an associated function called as
  `Type::method(...)`.

The target of `extend` may be any visible extendable value type, including an
imported type:

```nia
using root::math;

extend math::Point {
    fn len2(&self) i32 {
        4
    }
}
```

Concrete structural targets use the same type grammar, including nested slice
pointees such as `extend [&[char]] { ... }`.

`extend` itself is not marked `pub`. Method visibility is written on the method:

```nia
extend Point {
    pub fn len2(&self) i32 {
        self.x * self.x + self.y * self.y
    }
}
```

Public extension methods from loaded visible modules participate in method
lookup. `module` declarations load child modules and `using` opens already
visible module namespaces or package roots. Private extension methods are
visible only in their defining module. If multiple visible
extension methods provide the same method name for the same receiver type, the
call is ambiguous.

Receiver methods may also be called through their associated type path by
passing the receiver explicitly:

```nia
let mut n = Vec2::len2(& v);
```

The same associated path can be used as an unbound function item when forming a
function pointer. This does not capture a receiver; the receiver remains the
first explicit function parameter:

```nia
let mut f: &fn(&Vec2) i32 = &Vec2::len2;
let mut n = f(&v);
```

Generic structs may have methods:

```nia
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn get(&self) T {
        self.value
    }
}
```

Methods may have their own type parameters. Explicit generic method calls use
`[]`:

```nia
extend[T] Box[T] {
    fn replace[U](&self, value: U) U {
        value
    }
}

let mut box = Box[i32] { value: 1 };
let mut x = box.replace[bool](true);
```

Associated paths use the same brackets for generic type and method arguments:

```nia
let mut make: &fn(i32) Box[i32] = & Box[i32]::make;
let mut replace: &fn(&Box[i32], bool) bool =
    &Box[i32]::replace[bool];
let mut y = Box[i32]::replace[bool](&box, true);
```

Structural extension targets use `[type]::name` as their explicit associated
path:

```nia
extend[T] &T {
    fn null(self) bool {
        self as usize == 0
    }

    fn zero() usize {
        0usize
    }
}

let mut p: &u8 = [&u8]::zero() as &u8;
let mut n = [&u8]::null(p);
let mut f: &fn(&u8) bool = &[&u8]::null;
```

`[type]::name` is only an associated target path. It does not introduce a
short name for the method, and it does not capture a receiver.

## 11. Traits

Traits define required method signatures and associated type outputs for static
dispatch:

```nia
trait Show {
    fn show(&self) i32;
}
```

`Self` names the implementing type inside a trait declaration and inside an
`extend` block. It is not a general type name outside those contexts.

A type implements a trait with an `extend Type : Trait` block:

```nia
struct Point {
    x: i32,
}

extend Point : Eq {
    fn eq(&self, other: &Point) bool {
        self.x == other.x
    }

    fn ne(&self, other: &Point) bool {
        self.x != other.x
    }
}
```

Some trait names are builtin capability traits. They are still implemented with
ordinary trait implementation syntax, but their names and required members are
defined by the language. Operator expressions are checked through these traits:

| Operator | Required trait | Result |
| --- | --- | --- |
| `a + b` | `Add[Rhs]` | `Output` |
| `a - b` | `Sub[Rhs]` | `Output` |
| `a * b` | `Mul[Rhs]` | `Output` |
| `a / b` | `Div[Rhs]` | `Output` |
| `a % b` | `Rem[Rhs]` | `Output` |
| `-a` | `Neg` | `Output` |
| `not a` | `Not` | `bool` |
| `~a` | `BitNot` | `Output` |
| `a & b` | `BitAnd[Rhs]` | `Output` |
| `a | b` | `BitOr[Rhs]` | `Output` |
| `a ^ b` | `BitXor[Rhs]` | `Output` |
| `a << b` | `Shl[Rhs]` | `Output` |
| `a >> b` | `Shr[Rhs]` | `Output` |
| `a == b`, `a != b` | `Eq[Rhs]` | `bool` |
| `a < b`, `a <= b`, `a > b`, `a >= b` | `Ord[Rhs]` | `bool` |

`not` is boolean logical not. `~` is bitwise not. Primitive implementations are
compiler-proven for the primitive types that support the operation. Non-primitive
operator support comes from visible `extend Type : Trait[...]` implementations.

Builtin operator traits have fixed method names. Binary arithmetic and bitwise
traits use `add`, `sub`, `mul`, `div`, `rem`, `bit_and`, `bit_or`, `bit_xor`,
`shl`, and `shr`. Unary traits use `neg`, `not`, and `bit_not`. `Eq` requires
both `eq` and `ne`; `Ord` requires `lt`, `le`, `gt`, and `ge`.

`Sized`, `Deref`, `DerefMut`, `Index`, `IndexMut`, `Slice`, and `SliceMut` are
also builtin capability traits. Their names and required
members are fixed by the language:

```nia
trait Sized {}

trait Deref {
    type Target;
    fn deref(&self) &[Self as Deref]::Target;
}

trait DerefMut : Deref {
    type Target;
    fn deref_mut(&mut self) &mut [Self as DerefMut]::Target;
}

trait Index[I] {
    type Output;
    fn index(&self, index: I) &[Self as Index[I]]::Output;
}

trait IndexMut[I] : Index[I] {
    type Output;
    fn index_mut(&mut self, index: I) &mut [Self as IndexMut[I]]::Output;
}

trait Slice[R] {
    type Output;
    fn slice(&self, range: R) [Self as Slice[R]]::Output;
}

trait SliceMut[R] : Slice[R] {
    type Output;
    fn slice_mut(&mut self, range: R) [Self as SliceMut[R]]::Output;
}

```

The compiler proves builtin implementations for primitive operations,
layout-known types, pointers, arrays, and slices where the operation is native
to the language. User implementations of builtin traits are allowed when they
do not overlap a compiler-proven implementation. For example, a custom
container may implement `Slice[..]`, but `[T; N]` may not provide a manual
`Slice[..]` implementation because array slicing is already
compiler-proven. Range bound and slice data-pointer access are instead inherent
operations limited to their structural representation shapes. User types
expose their own ordinary inherent or trait methods without claiming
language-owned `Start`/`End` or `Ptr`/`PtrMut` capabilities.

Index expressions lower through `Index` or `IndexMut`; slice expressions
lower through `Slice` or `SliceMut`. Native array, pointer, and slice
implementations require integer indices or range types whose bounds are
`usize`.

A trait implementation block is only for implementing the named trait. It must
not contain methods that are not members of that trait. Inherent methods still
use ordinary `extend Type { ... }` blocks.

Trait method declarations may include bodies. A body is a default method body.
If an implementation omits a method that has a default body, calls to that
method instantiate the default body with `Self` set to the implementing type:

```nia
trait Comparable {
    fn same(&self, other: &Self) bool;

    fn different(&self, other: &Self) bool {
        not self.same(other)
    }
}
```

Traits may declare supertraits after `:`. Multiple supertraits are joined with
`+`:

```nia
trait Ordered : Comparable {
    fn lt(&self, other: &Self) bool;
}

trait IndexedSource[I] : Source + Index[I] {
    fn valid_index(&self, index: I) bool;
}
```

If `Child : Parent`, then a `where T: Child` bound also makes `Parent` methods
available on `T`, and default methods in `Child` may call `Parent` methods.
Implementing `Child` does not implicitly implement `Parent`; the parent trait
implementation must be written explicitly:

```nia
extend Point : Eq {
    fn eq(&self, other: &Point) bool {
        self.x == other.x
    }

    fn ne(&self, other: &Point) bool {
        self.x != other.x
    }
}

extend Point : Ord {
    fn lt(&self, other: &Point) bool {
        self.x < other.x
    }

    fn le(&self, other: &Point) bool {
        self.x <= other.x
    }

    fn gt(&self, other: &Point) bool {
        self.x > other.x
    }

    fn ge(&self, other: &Point) bool {
        self.x >= other.x
    }
}
```

Traits may declare associated types. An associated type is a named type output
selected by the implementing `Self` type and the trait's explicit generic
arguments:

```nia
trait Source {
    type Item;

    fn get(&self) [Self as Source]::Item;
}

trait Mapper[A, B] {
    type C;
    type D;

    fn map(&self, a: A, b: B) [Self as Mapper[A, B]]::C;
}
```

Every trait implementation must define every associated type required by the
trait, and it may not define associated types that the trait does not declare:

```nia
struct Counter {
    value: i32,
}

extend Counter : Source {
    type Item = i32;

    fn get(&self) i32 {
        self.value
    }
}
```

Associated type definitions are only valid in trait implementation blocks.
Ordinary inherent `extend Type { ... }` blocks cannot contain `type` members.

Associated type projections are written explicitly:

```nia
[T as Source]::Item
[Self as Mapper[A, B]]::C
```

Nia does not support shorthand projection syntax such as `T::Item` or
`Self::Item`. Path syntax with `::` remains ordinary type or associated function
path syntax; it does not infer an associated type projection. Associated types
cannot declare their own generic parameters.

Trait bounds may bind associated types in the same bracketed trait argument
list. Positional trait arguments come first, followed by named associated type
bindings:

```nia
fn add_same[T](a: &T, b: T) T
where T: Add[T, Output = T] {
    a.add(b)
}

fn mapped[T](value: &T) i32
where T: Mapper[i32, bool, C = i32, D = bool] {
    value.map_c(1, true)
}
```

`T: Add[T, Output = T]` selects the `Add[T]` implementation for `T` and binds
that selected implementation's `Output` associated type to `T`. The binding is
part of the trait bound; it is not a separate global type equality predicate.
Binding names must be associated types declared by the trait, and a single
bound may not bind the same associated type more than once.

Trait bounds are written in `where` clauses:

```nia
fn same[T](a: &T, b: &T) bool
where T: Eq[T] {
    a == b
}
```

Within a single predicate, multiple trait bounds are joined with `+`; commas
separate predicates:

```nia
fn ordered_show[T, U](value: &T, other: &U) bool
where T: Ord[U] + Show, U: Eq[U] {
    value.show() == 0
}
```

The current compiler parses and lowers trait bounds, validates trait
implementation blocks, and supports receiver-method calls through generic
`where` bounds. A call such as `a.same(b)` in
`fn same[T](...) where T: Comparable` is
kept as a trait-method obligation in the generic body and resolved to the
visible concrete implementation when the generic function is instantiated.
Default methods may call other methods from the same trait; those calls are
resolved through the visible concrete implementation when available, or through
another default body. Associated type projections are normalized through the
current function's associated type bound bindings or through the visible
concrete trait implementation during checking and monomorphization. Supertrait
method obligations are resolved through the same visible concrete implementation
lookup.

Trait object pointer types are represented as fat pointers carrying an erased
object pointer and implementation metadata. A concrete pointer may be coerced to
a trait object pointer when the pointed-to type satisfies the selected trait and
associated type bindings:

```nia
trait Source {
    fn get(&self) i32;
}

fn read(source: & Source) i32 {
    source.get()
}
```

`source.get()` is a dynamic trait method call through the object's vtable.
Object-safe trait object methods must be receiver methods and may not have
method-level type parameters. By-value trait object receiver calls are rejected.
Builtin trait objects are not supported in this version.

A trait object pointer may be coerced to a supertrait object pointer when the
target trait is a declared supertrait of the source trait and mutability
matches:

```nia
trait Parent {}
trait Child : Parent {}

fn accept(parent: & Parent) () {}

fn use_child(child: & Child) () {
    accept(child)
}
```

The compiler records this as a trait-object upcast, not as a plain bitcast, and
remaps metadata to the target supertrait's vtable region. When the target
supertrait object binds associated types, the source object type must carry
matching bindings. Bindings for non-primary supertraits use explicit projection
keys:

```nia
fn use_child(
    child: & Child[
        [Self as FatherA]::Item = i32,
        [Self as FatherB]::Item = usize,
    ],
) () {}
```

## 12. Modules

Each `.nia` file is one module. Module loading is explicit: a source file may
declare child modules with `module name;` or `pub module name;`. A `using` item
opens names that are already available through the current module graph or a
module-map pkg root; it does not implicitly discover files.

A compilation has one reserved entry package named `root`. The reserved
`pkg` root names the current module's own package. The CLI also provides a
default `std` pkg root and accepts additional package roots with
`-M name=path` or `--module name=path`. One `-M` entry is one package. Package
roots are lazy: they are loaded when referenced by `using`, or when executable
emission injects the standard startup contract.

### 12.1 Module Declarations

A module declaration loads a child module under the current module stem:

```nia
module math;
pub module geom;
```

For an entry file at `src/app/main.nia`, `module math;` loads
`src/app/math.nia`. If `math.nia` declares `module ops;`, that declaration loads
`src/app/math/ops.nia`. Nia intentionally has no `mod.rs` form.

The child module's logical name is taken from its declaration site. A file may
change the logical child name it exposes by declaring a nested module itself:

```nia
// src/app/main.nia
module foo;

// src/app/foo.nia
module zoo; // loads src/app/foo/zoo.nia
```

`pub module name;` makes the child module namespace part of the current module's
public surface. Plain `module name;` loads the child for the current module but
keeps the namespace private to the package visibility rules.

### 12.2 Pkg Roots And Paths

The reserved path roots are:

- `root`, the compilation entry package, regardless of the current module;
- `pkg`, the current module's pkg root;
- `std`, the standard-library pkg root unless overridden with `-M std=...`;
- any additional pkg root supplied with `-M name=path`.

Examples:

```bash
nia check src/main.nia -M std=/usr/share/nia/std.nia -M math=vendor/math.nia
```

```nia
using std;
using std::io;
using math;
using pkg::internal;
```

A mapped root file is the pkg root module. Tail segments select declared
child modules below that root. If `std` maps to `/usr/share/nia/std.nia`, then
`using std::io;` refers to the `io` child declared by that root and backed by
`/usr/share/nia/std/io.nia`.

Within a loaded module, `self` names the current module and `super` names the
parent module. `pkg` names the current pkg root, while `root` still
names the compilation entry package:

```nia
// src/app/foo/zoo.nia
using super::helper;
using pkg::internal;
using root::config;
```

Package implementation code should use `pkg::...` for absolute references
to its own modules. For example, the standard library's implementation modules
use `pkg::io` and `pkg::process`; user packages still refer to those
public modules as `std::io` and `std::process`.

### 12.3 Using

`using` shortens visible namespaces in the current scope. It can reference a
pkg root, a loaded module namespace, an item namespace, or an enum namespace.
It does not load arbitrary files; child files become modules only through their
parent's `module` declaration.

Supported forms:

```nia
using std;
using std::process;
using pkg::internal;
using root::math as m;
using math::add;
using math::add as plus;
using math::{add, sub as minus};
using math::{add, sub as minus, Operator::*};
using math::*;
using {math, math::add, palette::Color::Red};

using Color::Red;
using Color::{Red, Black as Dark};
using Color::*;
using palette::Color::Red;
using palette::Color::*;
using root::a::{b::c::foo, d::e::{f::goo, g}, h::Color::*};
```

Grouped `using` accepts names, renames, module selectors, and enum-member
selectors. Nested groups may use arbitrary-depth namespace paths.

`using mod;` brings the module namespace itself into scope. If `facade` publicly
re-exports a module namespace with `pub using impl;`, then `using facade::*;`
also brings `impl` into scope.

Wildcard rules:

- `using mod::*` imports the module's complete public surface: direct public
  top-level definitions, public module namespaces, public module namespaces
  re-exported with `pub using`, and public items re-exported with `pub using`.
- `using Enum::*` imports all variants of that enum.

Top-level `using` is visible throughout the file. Block-local `using` is visible
only in that block and its children. Duplicate imported names in the same
namespace and same scope are errors, whether they come from explicit selections
or wildcards.

Imported items enter the namespace matching their actual category: functions and
globals enter the value namespace; structs, enums, and type aliases enter the
type namespace; enum variants enter the value namespace while preserving their
enum identity.

### 12.4 Pub Using

`pub using` re-exports selected items as part of the current module's public
surface:

```nia
// facade.nia
using root::impl;
pub using impl;
pub using impl::add;
pub using impl::{frob as do_frob};
pub using impl::*;

using root::palette;
pub using {impl, impl::add, palette::Color};
pub using palette::Color;
pub using palette::Color::Red;
pub using palette::Color::*;
```

```nia
// main.nia
using root::facade;
pub fn color() facade::Color {
    facade::Red
}
```

`pub using` is only valid at module top level.

The selected namespace member must be visible to the current module. Visibility
includes the source module's direct public definitions, public module
namespaces, and items it re-exported with `pub using`. Re-export chains may be
transitive. Cycles are resolved through the module graph; concrete semantic
cycles are diagnosed by the phase that observes them.

`pub using impl;` re-exports the module namespace itself. Downstream modules may
name re-exported items through the re-exporting module path, such as
`facade::add`, or through a re-exported module namespace, such as
`facade::impl::add`.

Wildcard `pub using mod::*` has the same expansion rule as `using mod::*`.
Wildcard `pub using Enum::*` re-exports every enum variant.

### 12.5 Visibility

Modules and declarations are private by default. Public APIs are marked with
`pub`, and restricted visibility can be written as `pub(super)` or
`pub(pkg)`. `pub(super)` exposes the item to the parent module and its
children. `pub(pkg)` exposes it within the package selected by one `-M`
entry or by the reserved `root`/`std` packages.
Restricted visibility is still cross-module visibility. Multi-object emission
must retain a `pub(super)` or `pub(pkg)` definition for callers in its visible
scope; only a truly private definition is eligible for module-local dead-code
elimination without whole-program reference evidence.

`pub` may be applied to `module`, `fn`, `struct`, `enum`, `type`, `let`, `let mut`,
`extern` declarations, and `using`. Nia has no separate `mod` or `use` syntax.
Package management is outside the language specification.

## 13. ABI, Runtime, And Symbols

Nia does not require a garbage collector, exception runtime, async runtime, or
hidden allocator.

Extern interop uses the C ABI and is an explicit boundary, not Nia's default
runtime model:

```nia
extern fn foreign_log(message: &u8);
```

When calling C string APIs, use `b"...\0"` and pass an explicit pointer to the
first byte:

```nia
foreign_log((&b"hello\n\0").ptr());
```

String and byte string literals are array values, not pointers. There is no
implicit `[u8; N]` to `&u8` decay; take an explicit address and then use `ptr()`
when an ABI requires an element pointer.

A literal evaluated in a function does not acquire static storage merely
because its contents are compile-time text. An address or slice derived from
that array must not escape the lifetime of the containing value or frame. Nia
does not infer or check a borrow lifetime for the pointer; callers that need a
longer-lived view must place the array in storage with that lifetime or copy it
into owned storage.

### 13.1 Internal Symbol Names

Nia uses deterministic, readable internal symbol names. The format is not meant
to be compatible with C++ or Rust. It is meant to make debugging, linking, and
monomorphized instances traceable.

The following symbols use internal naming:

- non-extern functions, methods, globals, and struct definitions;
- concrete generic function and method instances;
- type encodings inside instance names.

`extern` functions and globals do not use internal mangling. Their symbol name is
the source declaration name. An `extern fn` with a body is also unmangled.

Base symbol format:

```text
nia__m<M>__d<D>__<name>
```

`M` is the module id, `D` is the definition id, and `<name>` is the source name
after symbol sanitization. Sanitization keeps ASCII letters, digits, and `_`;
other characters become `_`; an empty result becomes `_`.

Generic function or method instances append an instance suffix:

```text
<base>__inst__<arg1>__<arg2>__...
```

Type encoding rules:

- primitive types use their names, such as `i32`, `u8`, `bool`, `never`;
- unit uses `unit`; opaque uses `opaque`; tuple types encode as
  `tuple__len__<arity>__<element encodings>`;
- `&T` encodes as `ptr_read__<T>`;
- `&mut T` encodes as `ptr__<T>`;
- `^T` encodes as `vptr_read__<T>`;
- `^mut T` encodes as `vptr__<T>`;
- trait object pointers encode as `trait_obj__<trait>...` or
  `trait_obj_read__<trait>...`;
- `[T; N]` encodes as `arr__<len>__<elem>`;
- function pointers encode as `fnptr__pc<N>__<p1>__...__ret__<ret>`, with
  `__variadic` appended for variadic function pointers;
- readonly callable views encode as
  `callable_read__pc<N>__<p1>__...__ret__<ret>` and writable callable views as
  `callable__pc<N>__<p1>__...__ret__<ret>`;
- unsized callable interfaces encode as
  `callable_pointee__pc<N>__<p1>__...__ret__<ret>`;
- optional types encode as `opt__<T>`;
- error union types encode as `err_union__<E>__<T>`;
- nominal types encode as `nom__<base>` and, with arguments, as
  `nom__<base>__argc<N>__<arg1>__...`;
- generic parameters encode as `gen__<name>`;
- error types encode as `ty_error`.

Array length encodings include:

- inferred length: `infer`;
- literal or expression length: `len__<text>`;
- builtin length: `builtin__<name>__<ty>`.

The rules should stay readable, deterministic, and structurally explicit.

## 14. Required Compiler Surface

A conforming Nia compiler supports:

- lexing and parsing `.nia` files;
- source-span diagnostics;
- primitive type checking;
- arrays, slices, pointers, optional types, error union types, structs, unions,
  C-style enums, and function types;
- `let mut`, `let`, and `const` bindings;
- expression blocks and tail expressions;
- `if` expressions;
- the three `for` forms;
- `defer`;
- `match` and enum exhaustiveness checks;
- `std::builtin::size[T]()`, `std::builtin::align[T]()`, `value.len()`,
  `range.start()`, `range.end()`, `slice.ptr()`, `slice.ptrMut()`, and
  `std::builtin::asm(std::builtin::AsmConfig {...})`;
- explicit `module` declarations, module-map package roots, and `using`;
- global static storage from top-level `static mut` and `static`;
- top-level `pub` visibility;
- `extern` C declarations, definitions, and calls;
- generic functions and structs via monomorphization;
- methods declared through `extend`;
- trait declarations, associated types, and direct trait implementation checks;
- lowering to a typed backend IR;
- LLVM IR or object emission;
- freestanding executable emission for Linux x86_64, with experimental i686
  support, when a target linker is available.

Standard-library APIs are specified beside their owning `lib/std` facades.
The `nia` command surface is documented by its help output and the repository
README, while build-script APIs and execution ownership live in
[`lib/std/build.nia`](../lib/std/build.nia) and
[`crates/nia-build/README.md`](../crates/nia-build/README.md). These toolchain
surfaces are not additional language grammar.

## 15. Outside The Language Contract

This specification defines only the syntax and semantics stated in its current
sections. An unmentioned construct has no reserved syntax or implied behavior.
Unsupported attributes, intrinsics, macro forms, generic forms, patterns, and
compile-time operations are ordinary invalid source unless a current section
defines them.

Package management, editor protocols, compiler command spelling, build-script
APIs, and standard-library APIs belong to their owning toolchain or library
interfaces. They do not become language grammar through omission from this
specification.

## 16. Example

```nia
using std;
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
    fn len2(&self) i32 {
        self.x * self.x + self.y * self.y
    }
}

fn add(a: i32, b: i32) i32 {
    a + b
}

fn sum(xs: &[i32]) i32 {
    let mut total = 0;
    for &value in xs {
        total += value;
    }
    total
}

fn score(answer: Pair[i32, i32]) i32 {
    match answer.first {
        0..10 => answer.second,
        10..=42 => answer.first + answer.second,
        _ => 0,
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;

    let mut v = Vec2 { x: 3, y: 4 };
    let mut values = [add(40, 2), v.len2(), 7];
    let mut pair = Pair[i32, i32] { first: values[0], second: sum(&values) };

    if score(pair) != 116 {
        return process::exit(1)!;
    }

    !()
}
```
