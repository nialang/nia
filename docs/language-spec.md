# Nia Language Specification

Status: normative language reference

This document defines the current Nia language. It is intended to be the main
reference for users, implementers, tests, and future maintenance work. Version
numbers are tracked by Git tags and release history, not by this file name.

Nia is a small systems programming language. It keeps the directness of C while
removing declaration syntax baggage, hidden runtime policy, and large semantic
systems that would make the language hard to implement and maintain.

## 1. Language Scope

Nia provides:

- a small statically typed language for systems programming;
- direct memory-oriented types: integers, booleans, arrays, pointers, slices,
  structs, function pointers, and C-style enums;
- simple type generics for functions, structs, and methods, implemented by
  monomorphization;
- methods declared in `extend` blocks;
- traits implemented by `extend Type : Trait` blocks;
- expression-oriented blocks and `if`;
- C-style enums with namespaces and `switch` without fallthrough;
- compile-time value bindings with `comptime`;
- `defer` for scope cleanup;
- C ABI interop through `extern`;
- file-level modules with explicit `import`, `using`, and `pub using`;
- a small visibility model based on `pub`;
- host execution by default, with object/LLVM output available for bare or
  custom build flows.

The current core language keeps these systems outside the language surface:

- garbage collection;
- exceptions;
- a borrow checker;
- algebraic data types;
- pattern matching;
- implicit allocation;
- a hidden runtime startup model;
- package management as part of the core language.

## 2. Source Files And Programs

Nia source files use the `.nia` extension.

A compilation unit is UTF-8 text. Source locations are tracked with byte offsets
and reported through source spans.

A hosted executable uses a root `main` function. The minimal form is:

```nia
fn main() i32 {
    0
}
```

CLI-style programs may receive arguments from the host C runtime:

```nia
fn main(argc: i32, argv: &const &const u8) i32 {
    0
}
```

Nia distinguishes two execution models:

- `host`: the default model. The program relies on libc and the platform C
  runtime. Platform startup code calls the exported `main` symbol, and the
  compiler driver may perform host linking.
- `bare`: no startup logic is injected and `main` is not required. The compiler
  emits LLVM IR or object files for an external build system, custom entry
  symbol, linker script, or freestanding runtime.

In hosted mode, only a non-generic root `main` with one of the accepted hosted
entry signatures is exported as the C ABI symbol `main`:

```nia
fn main() i32
fn main(argc: i32, argv: &const &const u8) i32
```

Other Nia functions, including imported functions named `main`, use normal Nia
internal symbol naming unless they are declared `extern`.

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
comptime
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
import
mut
or
pub
return
struct
switch
trait
true
type
using
var
void
where
```

Primitive type names such as `i32`, `u8`, `usize`, `bool`, `char`, and `void`
are reserved type names. `!` is the never type marker, not a keyword.

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
var x: u8 = 1;
var n: usize = 10;
var chained = 10i32;
```

The suffix selects the literal's type before contextual inference. The literal
value must fit in that type. Underscores may separate digits and do not affect
the value. Radix prefixes and underscores may be combined with suffixes:

```nia
var mask = 0xffu8;
var bits = 0b1010_0000u8;
var mode = 0o755usize;
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
var x: f32 = 1.5;
var y = 1.5f32;
var z = 1.0e-3f64;
```

The suffix selects the literal's type before contextual inference. The value
must be finite and representable in that type. Integer suffixes are invalid on
floating literals.

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

String literals are not `String`, pointers, or slices. A string literal is fixed
length Unicode scalar array syntax, with type `[N]char`.

Byte string literals:

```nia
b"nia"
b"nia\0"
```

Byte string literals are fixed length byte-array syntax, with type `[N]u8`.

C string literals:

```nia
c"nia"
```

C string literals are byte string literals with one trailing NUL byte appended.
`c"nia"` has type `[4]u8` and is equivalent to `b"nia\0"`. Interior NUL bytes
are allowed; the syntax only appends one trailing NUL.

Adjacent quoted string literals with the same prefix are concatenated into one
literal:

```nia
"hello, " "world"
b"ni" b"a\0"
c"hello, " c"world"
```

This is source-level literal concatenation, not runtime string or array
concatenation. Mixed literal families are invalid:

```nia
"hello" b"world" // invalid
b"hello" c"world" // invalid
"hello" c"world" // invalid
```

For adjacent C string literals, the trailing NUL is appended once to the final
combined byte sequence. For example, `c"foo" c"bar"` has type `[7]u8` and is
equivalent to `b"foobar\0"`.

In an expected `&u8` or `&const u8` context, a C string literal may be coerced to
a pointer to its first byte. This creates a block-scoped array temporary; it does
not promote the literal to static storage. The coercion is specific to C string
literals and does not apply to byte string literals or arbitrary `[N]u8` arrays.

Multiline string literals use consecutive lines beginning with `\\`. Byte and C
multiline string literals use `b\\` or `c\\` on the first line; continuation
lines still use `\\`:

```nia
\\mov rax, 60
\\syscall

b\\mov rax, 60
\\syscall

c\\mov rax, 60
\\syscall
```

For multiline strings, indentation before the delimiter is ignored, the delimiter
itself is not part of the string, and the text after the delimiter is copied as
is. Adjacent lines are joined with `\n`; no extra newline is appended after the
last line. Escape sequences are not interpreted inside multiline string lines.
The prefix selects the same type family as the quoted form: `[N]char`, `[N]u8`,
or NUL-terminated `[N + 1]u8`.

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
void
!
```

`void` means the expression or function produces no meaningful value. If a
function declaration omits its return type, the return type is `void`.

`!` is the never type. It marks expressions that never produce a normal value,
such as `return`, `break`, `continue`, and calls to functions returning `!`.
Never expressions may be used where ordinary values are expected because control
flow does not continue to the use site. `!` may be used as a function return type
or function pointer return type. It is not a valid variable, field, parameter, or
array element type.

### 4.2 Pointers

Pointer types:

```nia
&T
&const T
```

`&T` is a writable object pointer. `&const T` is a read-only object pointer.

Pointers are ordinary values. Nia has no borrow checker. `&T` and `&const T` are
different types. Pointer conversions must be explicit:

```nia
var addr = ptr as usize;
var ptr2 = addr as &u8;
```

Address-of and dereference syntax:

```nia
var value = 1;
var p = &value;
var cp = &const value;
var x = p.*;
p.* = 1;
```

`&place` requires a writable place. `&const place` requires an addressable place.
Identifiers, field access, array indexing, slice indexing, and pointer
dereference may be places. Field access and indexing inherit place-ness from
their left-hand side.

Literals, ordinary call results, arithmetic results, and struct rvalues are not
places and cannot be addressed directly. Bind them to a local or global object
when a stable address is needed:

```nia
var point: Point = { x: 10, y: 20 };
var p: &Point = &point;

const hello = c"hello";
_ = &const hello[0];
```

### 4.3 Arrays

Fixed-length array type:

```nia
[N]T
```

Array length may be written explicitly in type syntax, or inferred with `_` when
an array literal provides the element count:

```nia
var xs: [_]i32 = [1, 2, 3];
var name: [_]u8 = c"nia";
```

`[_]T` is only valid in contexts initialized by an array literal or string
literal. After inference the real type is `[N]T`.

Array literals:

```nia
[1, 2, 3]
[b'n', b'i', b'a', b'\0']
```

When an array literal initializes a binding without a type annotation, the
binding type is inferred from the literal:

```nia
var xs = [1, 2, 3]; // [3]i32
```

In other expression contexts, the expected type must still supply the array
element type and length.

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
var matrix: [2][3]i32 = [
    [1, 2, 3],
    [4, 5, 6],
];

var zeros: [2][3]i32 = [[0; 3]; 2];
```

Array indexing:

```nia
var x = arr[0];
arr[1] = 42;
```

Array length is part of the type. Arrays do not decay implicitly except through
the array-to-slice conversion rule.

### 4.4 Slices

Slices are length-carrying references. Slice types are written only through
reference forms:

```nia
&[T]
&const [T]
```

`&[T]` is a writable contiguous range. `&const [T]` is a read-only contiguous
range. The language does not expose slice fields; an implementation may represent
a slice as `{ ptr, len }`, where `ptr` points to the first element and `len` has
type `usize`.

Slices may be constructed by combining range indexing with address-of:

```nia
var hello = "hello";
var s = &const hello[..];       // &const [char]
var t = &const hello[0..2];     // &const [char]
var u = &const hello[0..=1];    // &const [char]
var v = &const hello[1..];      // &const [char]
var w = &const hello[..3];      // &const [char]
var x = &const hello[..=3];     // &const [char]
```

Writable slices use `&` and require a writable base place:

```nia
var xs: [4]i32 = [1, 2, 3, 4];
var s = &xs[1..3]; // &[i32]
s[0] = 10;
```

Bare range indexing is not a value expression:

```nia
xs[..]; // error; use &const xs[..] or &xs[..]
```

An array value may be implicitly converted to a full-range slice when the
expected type is exactly `&[T]` or `&const [T]`. This is the only array decay
rule:

```nia
fn read(xs: &const [i32]) i32 {
    xs[0]
}

fn write(xs: &[i32]) {
    xs[0] = 10;
}

var ro: &const [i32] = [1, 2, 3];
var rw: &[i32] = [1, 2, 3];
read([1, 2, 3]);
write([1, 2, 3]);
```

If the source expression is an array place, conversion to `&const [T]` requires
an addressable place; conversion to `&[T]` requires a writable place. Array and
string literals used by this conversion create block-scoped array temporaries.
That temporary materialization is only for array-to-slice conversion and does not
make ordinary `&rvalue` valid.

Range forms:

```nia
start..end
start..=end
start..
..end
..=end
..
```

Range bounds must be integer expressions. Nia does not provide built-in runtime
bounds checks. The programmer is responsible for ensuring that the selected
memory range is valid.

The base of a slice construction may be an array, another slice, or a
single-element pointer:

```nia
var arr: [4]i32 = [1, 2, 3, 4];
var a = &arr[..];      // len = 4

var b = &a[1..3];      // slice from slice

var x: i32 = 10;
var p = &x;
var c = &p[..];        // len = 1
var d = &p[0..1];      // len = 1
var e = &p[0..12];     // allowed; programmer owns the length claim
```

For `&T` and `&const T`, an omitted upper bound uses a base length of 1. An
explicit upper bound uses the explicit range length.

`slice[index]` accesses an element. Indexing `&const [T]` produces an addressable
but non-writable place. Indexing `&[T]` produces a writable place.

### 4.5 Structs

Struct declaration:

```nia
struct String {
    ptr: &const u8,
    len: usize,
}
```

Struct value construction:

```nia
var s: String = { ptr: &const bytes[0], len: 3 };
```

Struct literals do not carry a nominal type by themselves. The expected type must
provide the struct type. Anonymous struct inference such as
`var p = { x: 10, y: 20 };` is not supported.

Field access:

```nia
var len = s.len;
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

### 4.6 Function Types

Function declaration:

```nia
fn add(a: i32, b: i32) i32 {
    a + b
}
```

If the return type is omitted, it is `void`:

```nia
const log_fmt = c"value=%d\n";

fn log(value: i32) {
    printf(&const log_fmt[0], value);
}
```

Use top-level `const` rather than `comptime` for data that must have a stable
address, such as a C format string.

Function pointer type:

```nia
&const fn(i32, i32) i32
```

A function declaration name is a function item, not an ordinary runtime value.
Function items cannot be used bare:

```nia
fn add(a: i32, b: i32) i32 {
    a + b
}

var f = add; // error
```

Addressing a function item with `&const` creates a function pointer:

```nia
var f = &const add;                  // &const fn(i32, i32) i32
var g: &const fn(i32, i32) i32 = &const add;   // allowed
```

Generic functions must be explicitly instantiated before taking a function
pointer:

```nia
fn id[T](x: T) T {
    x
}

var f = &const id[i32]; // &const fn(i32) i32
var g = &const id;      // error
```

`&const function_item` is a specific function-item address rule. It does not
require the function item to be a place. `&function_item` is not allowed.

## 5. Declarations

### 5.1 Functions

```nia
fn name(param: Type, other: Type) ReturnType {
    body
}
```

When a function returns a non-`void` type, the tail expression of its body block
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

### 5.2 Extern Declarations

`extern` uses an external ABI and external symbol name. Item modifier order is
visibility first and `extern` second:

```nia
pub extern fn printf(fmt: &const u8, ...);
```

`extern pub fn` is not valid syntax.

An `extern fn` without a body declares an external C ABI symbol:

```nia
pub extern fn printf(fmt: &const u8, ...);
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
extern const errno: i32;
extern var global_counter: usize;
```

Extern functions default to return type `void` when no return type is written.
Variadic functions are only allowed as body-less `extern fn` declarations.

Nia does not provide `extern { ... }` blocks or explicit ABI strings. All
`extern` functions, globals, and structs use the C ABI.

### 5.3 Type Aliases

```nia
type Byte = u8;
type CString = &const u8;
```

Type aliases do not create new nominal types.

### 5.4 Enums

Nia enums are C-style named integer sets. They are not algebraic data types.

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
var c = Color::Black;
```

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
var flag: Flag = 3 as Flag;
```

`switch` is an expression that may match integers and enums:

```nia
var value = switch c {
    Color::Black => return 0;
    Color::White => 1,
    Color::Red => 2,
};
```

As with `if`, a `switch` may also be used as an expression statement. `switch`
has no fallthrough. `_` is the default arm:

```nia
switch code {
    ErrorCode::Ok => return 0;
    _ => return 1;
}
```

Switch expression arms must produce compatible value types unless an arm exits
through `return`, `break`, or `continue`. Switches over closed enums must cover
all variants or provide `_`. Switches over open enums must provide `_`, even if
every named variant is covered.

### 5.5 Const And Comptime Bindings

`const` is an immutable binding, not a general compile-time execution mechanism
and not macro substitution.

```nia
const name = "nia";
const mask: u32 = 0xff;
```

Use `comptime` for named compile-time values:

```nia
comptime size: usize = 16;
```

Local `const` bindings cannot be assigned after declaration:

```nia
const x = 1;
x = 2; // error
```

`const x: T;` is a valid local declaration. Like `var x: T;`, it creates
uninitialized automatic storage of type `T`; the difference is that the binding
cannot be assigned and cannot form writable `&T`:

```nia
const p: Point;

p.inspect();     // allowed if the receiver is Point or &const Point
_ = &const p;    // allowed
p.init();        // error if init requires &Point
p = { };         // error
_ = &p;          // error
```

Nia does not perform definite initialization analysis. Reading uninitialized
storage is the programmer's responsibility. `const` only controls binding-level
assignment and writable borrowing; it does not provide deep immutability and does
not prove that the value was initialized.

Top-level uninitialized bindings require explicit types. Non-extern top-level
uninitialized bindings create static storage initialized to zero. Extern
top-level uninitialized bindings only declare external symbols.

Top-level `const` creates immutable global static storage. Implementations should
place it in read-only data where possible:

```nia
const hello = c"hello\n";
```

Top-level `const` initializers must be expressible as static initialization data.
They do not execute arbitrary compile-time programs:

```nia
const a = 1 + 2;           // allowed: integer static expression
const hello = c"hi";       // allowed: byte-array static data
const p = &const hello[0]; // allowed: global static address
const bad = { 1 + 2 };     // error: block execution is not static data
```

Contexts requiring compile-time values, such as non-literal array lengths, read
`comptime` bindings rather than top-level static `const` storage.

`comptime` creates a compile-time value binding with no runtime storage and no
address:

```nia
comptime width: usize = 4;

fn main() i32 {
    comptime local_width: usize = width;
    var xs: [local_width]i32 = [1, 2, 3, 4];
    xs[0]
}
```

`comptime` may appear wherever `var` or `const` binding syntax is accepted. A
`comptime` binding must have an initializer. Its initializer must be evaluable
with the current compile-time value evaluator. Current compile-time values cover
integer literals, casts that preserve the underlying value, simple arithmetic and
bit operations, and references to other visible `comptime` bindings. Cyclic
`comptime` dependencies are errors.

Top-level `pub comptime` bindings participate in normal module visibility and
may be used through imports:

```nia
// config.nia
pub comptime width: usize = 4;

// main.nia
import .config;
var xs: [config::width]i32 = [1, 2, 3, 4];
```

Taking the address of a `comptime` binding is invalid because it has no storage.

### 5.6 Global Storage

Top-level value bindings create global static storage. There is no `static`
keyword.

```nia
var a = 1;

fn main() i32 {
    a = a + 1;
    0
}
```

Top-level bindings may infer their type or write it explicitly:

```nia
var hello = "hello\n";
var counter: i32 = 0;
const banner = c"nia";
```

Non-extern top-level initialized bindings must satisfy static initialization
rules. A bare global value does not automatically become an address:

```nia
var target: i32 = 1;
var p: &i32 = &target; // allowed
var q: &i32 = target;  // error
```

Top-level bindings are visible inside the same module. Cross-module visibility
is controlled by the module system.

## 6. Local Bindings And Assignment

`var` introduces mutable bindings. `const` introduces immutable bindings with
storage. `comptime` introduces immutable compile-time value bindings with no
storage.

Inferred type declaration:

```nia
var x = 1;
var name = "nia";
```

Explicit type declaration:

```nia
var x: i32 = 1;
var name: [4]u8 = c"nia";
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

There is no `let` keyword. `var` introduces a mutable binding. `const`
introduces an immutable binding. `var x: T;` and `const x: T;` are valid
uninitialized declarations. A declaration without an explicit type must have an
initializer.

## 7. Statements And Semicolons

Nia uses semicolons for statement boundaries.

Statements requiring semicolons:

```nia
const int_fmt = c"%d\n";

var x = 1;
x = x + 1;
printf(&const int_fmt[0], x);
return x;
break;
continue;
```

Block-shaped control flow used as a standalone statement does not need a trailing
semicolon. The recommended rule is:

- ordinary expression statements need `;`;
- `if` and `for` used as standalone statements do not need `;`;
- a block tail expression does not use `;`.

## 8. Expressions

### 8.1 Blocks

Blocks are expressions:

```nia
{
    var x = 1;
    x + 1
}
```

A block with a tail expression has the tail expression type. A block without a
tail expression has type `void`.

### 8.2 If

`if` is an expression:

```nia
var result = if score >= 60 {
    "pass"
} else {
    "fail"
};
```

When an `if` expression is used as a value, it must have both branches and the
branches must have compatible types. When `if` is used only for control flow,
`else` may be omitted and the expression type is `void`.

### 8.3 For

Nia has one loop keyword: `for`.

Three-part loop:

```nia
for var i = 0; i < 10; i += 1 {
    printf(&const int_fmt[0], i);
}
```

Condition loop:

```nia
for a > b {
    a -= 1;
}
```

Infinite loop:

```nia
for {
    // work
}
```

`break` exits the nearest loop. `continue` jumps to the continuation step; if
there is no continuation step, it starts the next iteration.

`for` is a statement and does not produce a value.

### 8.4 Defer

`defer` registers an expression to run when the current block exits:

```nia
{
    var file = open(path);
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

The deferred expression must complete normally and have type `void`. If cleanup
returns a non-`void` value, discard it explicitly:

```nia
defer {
    _ = flush(file);
    close(file);
};
```

`return`, `break`, and `continue` are not allowed inside deferred expressions.
Nia has no exceptions, so `defer` has no exception-unwind semantics.

### 8.5 Operators

Unary operators:

```text
-       numeric negation
!       boolean not
&       writable address
&const  read-only address
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
has type `&Point` or `&const Point`, then:

```nia
p.x
p.move(1, 2)
```

may dereference `p` for field access or receiver matching. This automatic
dereference is limited to fields, methods, and receiver matching. It is not a
general implicit conversion.

Expression statements may not silently discard non-`void` and non-`!` values.
Discard explicitly with `_`. Discarding a `void` expression is also valid:

```nia
vec.push(2);      // error if push returns non-void
_ = vec.push(2);  // allowed
_ = log("done");  // allowed even if log returns void
abort();          // allowed if abort returns !
```

### 8.7 Builtins

Nia provides a small builtin surface:

```nia
@size[T]()
@align[T]()
@len(value)
@ptr(slice)
@asm({...})
```

`@size[T]()` returns the ABI size of `T` in bytes as `usize`.

`@align[T]()` returns the ABI alignment of `T` in bytes as `usize`.

`@size[T]()` and `@align[T]()` are compile-time known values and may appear in
array lengths, static initializers, and ordinary expressions.

`@len(value)` returns array or slice length as `usize`. For `[N]T`, it returns
`N`. For `&[T]` and `&const [T]`, it returns the runtime slice length.

`@ptr(slice)` returns the slice element pointer. `@ptr(&[T])` has type `&T`.
`@ptr(&const [T])` has type `&const T`.

`@asm({...})` is the inline assembly escape hatch for syscalls, special
registers, port I/O, CPU instructions, and freestanding runtime glue. Its
argument must be a struct-literal configuration. It returns `void`. The fields
are compiler-consumed metadata, not a runtime struct:

```nia
fn syscall1(sys_num: usize, arg1: usize) isize {
    var ret: isize = 0;
    @asm({
        code:
            b\\syscall
        ,
        outputs: { rax: ret },
        inputs: {
            rax: sys_num,
            rdi: arg1,
        },
        clobbers: [b"rcx", b"r11", b"memory"],
        options: [b"volatile"],
    });
    ret
}
```

`code` must be a byte string literal. `inputs` and `outputs` are struct literals
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
var p: Pair[i32, bool] = { first: 1, second: true };
```

Expression generic instantiation also uses `[]` for explicit type arguments in
function-item, method, or other expression positions:

```nia
var x = id[i32](1);
var f = &const id[i32];
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
fn eq[T](a: &const T, b: &const T) bool
where T: Eq {
    a.eq(b)
}

struct Box[T] where T: Eq {
    value: T,
}
```

Generics are implemented by monomorphization. Type parameters have no runtime
representation. The current generic surface is explicit type parameters on
functions, structs, unions, traits, and methods. User-declared const generics
are reserved for future design; array length is part of array type syntax.

## 10. Methods

Methods are declared in `extend` blocks. An `extend` block attaches associated
functions and receiver methods to its target type:

```nia
struct Vec2 {
    x: i32,
    y: i32,
}

extend Vec2 {
    fn len2(&const self) i32 {
        self.x * self.x + self.y * self.y
    }
}
```

Method call:

```nia
var v: Vec2 = { x: 3, y: 4 };
var n = v.len2();
```

Methods are ordinary functions associated with a concrete nominal type. Receiver
forms:

```nia
fn method(&self, ...)
fn method(&const self, ...)
fn method(self, ...)
fn method(...)
```

Receiver meaning:

- `&self` borrows the receiver as writable `&Type`;
- `&const self` borrows the receiver as read-only `&const Type`;
- `self` passes the receiver by value;
- no `self` means the function is an associated function called as
  `Type::method(...)`.

The target of `extend` may be any visible extendable value type, including an
imported type:

```nia
import .math;

extend math::Point {
    fn len2(&const self) i32 {
        4
    }
}
```

`extend` itself is not marked `pub`. Method visibility is written on the method:

```nia
extend Point {
    pub fn len2(&const self) i32 {
        self.x * self.x + self.y * self.y
    }
}
```

Public extension methods from transitively imported modules participate in
method lookup. `import` controls this module capability propagation; `using`
only introduces shorter names for items that are already visible. Private
extension methods are visible only in their defining module. If multiple visible
extension methods provide the same method name for the same receiver type, the
call is ambiguous.

Receiver methods may also be called through their associated type path by
passing the receiver explicitly:

```nia
var n = Vec2::len2(&const v);
```

The same associated path can be used as an unbound function item when forming a
function pointer. This does not capture a receiver; the receiver remains the
first explicit function parameter:

```nia
var f: &const fn(&const Vec2) i32 = &const Vec2::len2;
var n = f(&v);
```

Generic structs may have methods:

```nia
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn get(&const self) T {
        self.value
    }
}
```

Methods may have their own type parameters. Explicit generic method calls use
`[]`:

```nia
extend[T] Box[T] {
    fn replace[U](&const self, value: U) U {
        value
    }
}

var box: Box[i32] = { value: 1 };
var x = box.replace[bool](true);
```

Associated paths use the same brackets for generic type and method arguments:

```nia
var make: &const fn(i32) Box[i32] = &const Box[i32]::make;
var replace: &const fn(&const Box[i32], bool) bool =
    &const Box[i32]::replace[bool];
var y = Box[i32]::replace[bool](&const box, true);
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

var p: &u8 = [&u8]::zero() as &u8;
var n = [&u8]::null(p);
var f: &const fn(&u8) bool = &const [&u8]::null;
```

`[type]::name` is only an associated target path. It does not introduce a
short name for the method, and it does not capture a receiver.

## 11. Traits

Traits define required method signatures for static dispatch:

```nia
trait Eq {
    fn eq(&const self, other: &const Self) bool;
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
    fn eq(&const self, other: &const Self) bool {
        self.x == other.x
    }
}
```

A trait implementation block is only for implementing the named trait. It must
not contain methods that are not members of that trait. Inherent methods still
use ordinary `extend Type { ... }` blocks.

Trait method declarations may include bodies. A body is a default method body.
If an implementation omits a method that has a default body, calls to that
method instantiate the default body with `Self` set to the implementing type:

```nia
trait Eq {
    fn eq(&const self, other: &const Self) bool;

    fn ne(&const self, other: &const Self) bool {
        !self.eq(other)
    }
}
```

Trait bounds are written in `where` clauses:

```nia
fn same[T](a: &const T, b: &const T) bool
where T: Eq {
    a.eq(b)
}
```

The current compiler parses and lowers trait bounds, validates trait
implementation blocks, and supports receiver-method calls through generic
`where` bounds. A call such as `a.eq(b)` in `fn same[T](...) where T: Eq` is
kept as a trait-method obligation in the generic body and resolved to the
visible concrete implementation when the generic function is instantiated.
Default methods may call other methods from the same trait; those calls are
resolved through the visible concrete implementation when available, or through
another default body. Associated types are reserved for later implementation
work.

## 12. Modules

Each `.nia` file is a module. Import resolution always produces one concrete
source file path; directories and packages are not modules by themselves.

### 12.1 Import

`import` makes another `.nia` file available to the current file. The
dot-separated import path is a portable spelling of a source file path. It has
the same module meaning as directly naming that resolved file path. Leading dots
determine the starting directory:

- `.` starts at the current file directory;
- `..` starts one directory above;
- remaining identifier segments are appended as directories, and the final
  segment gets the `.nia` extension.

For example, `import .math.ops;` resolves to a concrete file such as
`math/ops.nia` relative to the current module directory. The compiler does not
interpret `math` as a package module unless a separate import resolves to
`math.nia`.

Relative imports do not support `...` or deeper parent traversal. Use a mapped
root module when code needs to cross more than one parent boundary.

For a file at `src/app/main.nia`:

```nia
import .math;          // src/app/math.nia
import .math.ops;      // src/app/math/ops.nia
import ..lib;          // src/lib.nia
```

A bare import such as `import math;` is not relative. Its first segment is
resolved through an external module map. The CLI registers map entries with
`-M name=path` or `--module name=path`. Module map options may appear before or
after the command:

```bash
niac check src/main.nia -M std=/usr/share/nia/std.nia
```

```nia
import std;            // /usr/share/nia/std.nia
import std.io;         // /usr/share/nia/std/io.nia
```

When a mapped root has extra path segments, the root file path is treated as the
root module file and the tail segments select files below the root stem. If
`std` maps to `/usr/share/nia/std.nia`, then `import std.io;` resolves to
`/usr/share/nia/std/io.nia`. This is a deterministic path mapping; it does not
depend on whether the host filesystem permits a file and a directory with the
same stem.

Unmapped bare imports are errors. Bare imports do not fall back to relative file
lookup.

The local import alias defaults to the last path segment. `as` renames it:

```nia
import .math as m;
import .lib.math as math;
```

Imported declarations are accessed through the alias:

```nia
var x = math::add(1, 2);
```

The import graph may contain cycles. A cycle is not an error by itself:

```nia
// a.nia
import .b;

// b.nia
import .a;
```

Modules in a cycle remain separate modules. Cross-module references still use
explicit import aliases, and `pub` still controls item visibility. If a cycle
causes a concrete semantic problem, such as a recursive type alias, recursive
layout, invalid re-export chain, or recursive generic expansion, that problem is
diagnosed by the relevant compiler phase.

`pub` cannot be applied to `import`. Import aliases are local to the current
file. Re-export imported items with `pub using`.

### 12.2 Using

`using` shortens already visible namespaces in the current scope. It does not
load files or organize modules; `import` remains the only operation that loads a
module file. Hosts are:

- a module namespace: `using mod::name`, including module namespaces re-exported
  by `pub using`;
- an enum type namespace: `using Enum::Variant` or
  `using mod::Enum::Variant`.

Supported forms:

```nia
using math;
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
  top-level definitions, public module namespaces re-exported with `pub using`,
  and public items re-exported with `pub using`. It does not import enum
  variants unless those variants are explicitly part of the module public
  surface.
- `using Enum::*` imports all variants of that enum.

Top-level `using` is visible throughout the file. Block-local `using` is visible
only in that block and its children. Duplicate imported names in the same
namespace and same scope are errors, whether they come from explicit imports or
wildcards.

`using` does not load files. The root module namespace must already be imported
or made visible by another `using`.

Imported items enter the namespace matching their actual category: functions and
globals enter the value namespace; structs, enums, and type aliases enter the
type namespace; enum variants enter the value namespace while preserving their
enum identity.

### 12.3 Pub Using

`pub using` re-exports selected items as part of the current module's public
surface:

```nia
// facade.nia
import .impl;
pub using impl;
pub using impl::add;
pub using impl::{frob as do_frob};
pub using impl::*;

import .palette;
pub using {impl, impl::add, palette::Color};
pub using palette::Color;
pub using palette::Color::Red;
pub using palette::Color::*;
```

```nia
// main.nia
import .facade;
fn main() facade::Color {
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

### 12.4 Visibility

Modules are private by default. Only top-level declarations marked `pub` can be
accessed by other modules through qualified paths or `using`:

```nia
pub fn add(a: i32, b: i32) i32 {
    a + b
}

fn hidden() i32 {
    0
}
```

`pub` may be applied to `fn`, `struct`, `enum`, `type`, `const`, `var`, `extern`
declarations, and `using`. It may not be applied to `import`.

Nia has no `mod` or `use` syntax. Package management is outside the language
specification.

## 13. ABI, Runtime, And Symbols

Nia does not require a garbage collector, exception runtime, async runtime, or
hidden allocator.

Extern interop uses the C ABI:

```nia
extern fn printf(fmt: &const u8, ...);
```

When calling C string APIs, use `c"..."` to produce NUL-terminated byte arrays:

```nia
printf(c"hello\n");
```

String, byte string, and C string literals are array values, not places. They
may be passed through array-to-slice conversion when a slice is expected. C
string literals may also be passed directly when `&u8` or `&const u8` is
expected; this produces a pointer to a block-scoped temporary. If a stable C
string address is required, bind the C string to top-level `const` storage.

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

- primitive types use their names, such as `i32`, `u8`, `bool`, `void`;
- `&T` encodes as `ptr__<T>`;
- `&const T` encodes as `ptr_const__<T>`;
- `[N]T` encodes as `arr__<len>__<elem>`;
- function pointers encode as `fnptr__pc<N>__<p1>__...__ret__<ret>`, with
  `__variadic` appended for variadic function pointers;
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
- arrays, slices, pointers, structs, C-style enums, and function types;
- `var`, `const`, and `comptime` bindings;
- expression blocks and tail expressions;
- `if` expressions;
- the three `for` forms;
- `defer`;
- `switch` and enum exhaustiveness checks;
- `@size[T]()`, `@align[T]()`, `@len(value)`, `@ptr(slice)`, and `@asm({...})`;
- relative file imports and module-map bare imports;
- global static storage from top-level `var` and `const`;
- top-level `pub` visibility;
- `extern` C declarations, definitions, and calls;
- generic functions and structs via monomorphization;
- methods declared through `extend`;
- trait declarations and direct trait implementation checks;
- lowering to a typed backend IR;
- LLVM IR or object emission;
- hosted executable emission when host linking is available.

The CLI surface is:

```text
niac lex <file.nia>
niac parse <file.nia>
niac check <file.nia>
niac emit llvm <file.nia>
niac emit obj <file.nia> [-o file.o | --out-dir dir]
niac emit exe <file.nia> [-o executable]
```

Module-map options are accepted before or after the command:

```text
-M name=path
--module name=path
```

`emit obj` writes one object per backend codegen unit. `-o` is only valid for a
single-unit program. Multi-unit output uses `--out-dir`. `emit exe` writes
temporary objects and invokes the host C linker.

`build` is reserved for an external build system. The current CLI does not
provide `run`; use `emit exe` and execute the result.

## 15. Reserved Future Design Areas

The following areas are intentionally outside the current language surface.
They are reserved for future design and should not be treated as specified by
this document:

- associated type families and full trait obligation solving;
- payload-carrying algebraic data types beyond current enums;
- pattern matching beyond current switch expressions;
- closures;
- builtin option/result syntax;
- package management semantics;
- LSP semantics;
- large standard-library layering;
- volatile pointer families;
- SIMD as a language primitive;
- user-defined attributes and arbitrary compiler intrinsics;
- macros;
- general compile-time execution beyond the current `comptime` value subset.

## 16. Example

```nia
extern fn printf(fmt: &const u8, ...);

const hello_fmt = c"hello, %s\n";
const not_answer_fmt = c"not answer\n";
const len2_fmt = c"len2=%d\n";

struct String {
    ptr: &const u8,
    len: usize,
}

struct Pair[T, U] {
    first: T,
    second: U,
}

struct Vec2 {
    x: i32,
    y: i32,
}

extend Vec2 {
    fn len2(&const self) i32 {
        self.x * self.x + self.y * self.y
    }
}

fn add(a: i32, b: i32) i32 {
    a + b
}

fn main() i32 {
    var name = c"nia";
    var x = add(40, 2);

    if x == 42 {
        printf(&const hello_fmt[0], &const name[0]);
    } else {
        printf(&const not_answer_fmt[0]);
    }

    var v: Vec2 = { x: 3, y: 4 };
    printf(&const len2_fmt[0], v.len2());

    0
}
```
