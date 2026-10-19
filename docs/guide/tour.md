# A Tour Of Nia

This tour walks through the language from simple values to generics and
modules. Each complete program builds with `nia emit --exe` and checks with
`nia check <file> --runtime freestanding`; the output shown is what it prints.
The [language reference](../reference/language.md) has the complete rules.

Every program starts at the standard entry point:

```nia
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    !()
}
```

`init` gives access to arguments and the environment. The return type
`ExitCode!()` is an error union: `!()` is success, and `process::ExitCode(1)!`
exits with status 1. The examples print with `std::io::debugPrint`, which takes
a format string and a slice of pointers to the values. Formatting
implementations are discovered from the value and trait packages automatically;
no `using std::fmt;` declaration is required.

## Values And Types

```nia
using std::io;
using std::fmt;
using std::process;
using std::iter;

const limit: usize = 3;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;

    let answer = 42;            // i32: unsuffixed integers default to i32
    let ratio = 0.5;            // f64
    let mask: u8 = 0xff;        // the annotation types the literal
    let big = 1_000_000u64;     // a suffix fixes the type directly
    let ready = true;
    let letter = 'n';           // char, a Unicode scalar value

    let mut count = 0usize;
    count += limit;

    let wide = answer as i64 + 1;   // numeric conversions are explicit
    let code = letter as u32;

    let pair = (answer, ready);     // a tuple
    let first = pair.0;

    let answer = answer + 1;        // shadowing introduces a new binding
    _ = ratio;
    _ = mask;
    _ = big;

    io::debugPrint(&"answer={} wide={} code={} count={} first={}\n", &[
        &answer, &wide, &code, &count, &first,
    ]).?;
    !()
}
```

```text
answer=43 wide=43 code=110 count=3 first=42
```

- Integer types are `i8` through `i128`, `u8` through `u128`, `isize`, and
  `usize`. Floating-point types are `f32` and `f64`. Other primitives are
  `bool`, `char` (a Unicode scalar value), and the unit type `()`.
- A literal takes its type from context. Without context, integers are `i32`
  and floats are `f64`. A suffix such as `10u8` or `1.5f32` fixes the type.
- `let` bindings cannot be assigned again; `let mut` bindings can. `const`
  names a compile-time value with no storage, usable in types such as
  `[i32; limit]`.
- Conversions are always explicit with `as`. Integers never convert to `char`.
- A statement cannot silently drop a value. Write `_ = value;` to discard one.
- Integer overflow, division by zero, and out-of-range shifts trap at runtime
  and are compile errors in constant evaluation.

## Functions And Control Flow

```nia
using std::io;
using std::fmt;
using std::process;

fn square(x: i32) i32 {
    x * x
}

fn abs(x: i32) i32 {
    if x < 0 {
        return -x;
    }
    x
}

fn grade(score: i32) [char; 4] {
    if score >= 60 { "pass" } else { "fail" }
}

fn sumTo(limit: i32) i32 {
    let mut total = 0;
    for i in 1..=limit {
        total += i;
    }
    total
}

fn firstPowerAbove(threshold: i32) i32 {
    let mut value = 1;
    while value <= threshold {
        value *= 2;
    }
    value
}

fn countOdd(limit: i32) i32 {
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let block = {
        let base = square(3);
        base + abs(-1)
    };
    let label = grade(block);
    let total = sumTo(10);
    let power = firstPowerAbove(100);
    let odd = countOdd(9);
    io::debugPrint(&"block={} label={} total={} power={} odd={}\n", &[
        &block, &label, &total, &power, &odd,
    ]).?;
    !()
}
```

```text
block=10 label=fail total=55 power=128 odd=5
```

The last expression of a block, written without a semicolon, is its value.
That applies to function bodies, `if` branches, and plain `{ ... }` blocks.
`return` exits early. A function without a return type returns `()`.

Loops are `for pattern in iterable`, `while condition`, and `loop`, with
`break` and `continue`. Ranges are written `a..b` (exclusive), `a..=b`
(inclusive), `a..`, `..b`, and `..`. Loops are statements and produce no value.

Logical operators are the keywords `and`, `or`, and `not`. `~` is bitwise not.

## Structs And Methods

```nia
using std::io;
using std::fmt;
using std::process;

struct Point {
    x: i32,
    y: i32,
}

extend Point {
    // An associated function: called as `Point::init(...)`.
    fn init(x: i32, y: i32) Point {
        Point { x, y }
    }

    // `&self` receives a read-only pointer to the receiver.
    fn lengthSquared(&self) i32 {
        self.x * self.x + self.y * self.y
    }

    // `&mut self` may modify the receiver.
    fn translate(&mut self, dx: i32, dy: i32) () {
        self.x += dx;
        self.y += dy;
    }

    // `self` takes the receiver by value.
    fn swapped(self) Point {
        Self { x: self.y, y: self.x }
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut p = Point::init(3, 4);
    p.translate(1, 1);
    let q: Point = .{ x: 1, y: 2 };   // the owner is taken from the annotation
    let r = q.swapped();
    let Point { x, y } = r;           // destructuring
    let length = p.lengthSquared();
    io::debugPrint(&"length={} x={} y={}\n", &[&length, &x, &y]).?;
    !()
}
```

```text
length=41 x=2 y=1
```

Methods live in `extend` blocks. The receiver form decides how a method sees
its value: `&self` (read-only pointer), `&mut self` (writable pointer), `self`
(by value), or no receiver for an associated function such as `Point::init`.
Inside `extend`, `Self` names the extended type.

Struct literals always name their type, as in `Point { x: 1, y: 2 }`. When the
type is already known, `.{ x: 1, y: 2 }` leaves it out, and `Point { x, y }`
uses same-named locals as field values. Calling a method on a pointer
dereferences it automatically; otherwise dereference explicitly with `p.*`.

## Enums And Match

```nia
using std::io;
using std::fmt;
using std::process;

enum Band: u8 {
    Low,
    Middle,
    High,
}

enum Shape {
    Circle(i32),
    Rect { width: i32, height: i32 },
    Empty,
}

fn classify(score: i32) Band {
    match score {
        0..60 => .Low,
        60..90 => .Middle,
        _ => .High,
    }
}

fn area(shape: Shape) i32 {
    match shape {
        .Circle(radius) => 3 * radius * radius,
        .Rect { width, height } => width * height,
        .Empty => 0,
    }
}

fn describe(value: i32) i32 {
    match value {
        0, 1 => 10,
        2..=4 => 20,
        _ => 30,
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let band = classify(75);
    let code = band as u8;
    let shapes = [Shape::Circle(2), .Rect { width: 3, height: 4 }, .Empty];
    let mut total = 0;
    for &shape in &shapes[..] {
        total += area(shape);
    }
    let size = describe(3);
    if band is .Middle {
        io::debugPrint(&"band={} total={} size={}\n", &[&code, &total, &size]).?;
    }
    !()
}
```

```text
band=1 total=24 size=20
```

An enum variant can be a plain name, carry a tuple payload such as
`Circle(i32)`, or carry named fields. Plain-name enums may choose a backing
integer type (`enum Band: u8`) and convert to it with `as`. When the enum type
is known, `.Middle` leaves out the `Band::` prefix.

`match` has no fallthrough and must be exhaustive; `_` matches anything left.
An arm can list several patterns separated by commas, and integer ranges with
both ends. `if value is pattern { ... }` tests one pattern; its bindings are
visible only in the `if` branch. A bare name in a pattern always introduces a
binding, so constants are matched through a path such as `Color::Red`.

## Arrays, Slices, And Strings

```nia
using std::io;
using std::fmt;
using std::process;

fn sum(values: &[i32]) i32 {
    let mut total = 0;
    for &value in values {
        total += value;
    }
    total
}

fn fill(values: &mut [i32], value: i32) () {
    for index in 0..values.len() {
        values[index] = value;
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let numbers: [i32; 5] = [1, 2, 3, 4, 5];
    let mut zeros = [0; 4];              // [i32; 4], every element 0

    let all = sum(&numbers);             // &[i32; 5] coerces to &[i32]
    let middle = &numbers[1..4];         // elements 1, 2, 3
    let partial = sum(middle);

    fill(&mut zeros[2..], 7);
    let filled = sum(&zeros);

    let text = "héllo";                  // [char; 5]
    let bytes = b"nia\0";                // [u8; 4], NUL written explicitly
    let view: &[char] = &text;
    let textLength = view.len();
    let byteLength = bytes.len();

    let third = match view.get(9) {      // checked access returns ?&char
        ?value => value.*,
        null => '?',
    };

    io::debugPrint(&"all={} partial={} filled={} text={} bytes={} third={}\n", &[
        &all, &partial, &filled, &textLength, &byteLength, &third,
    ]).?;
    !()
}
```

```text
all=15 partial=9 filled=14 text=5 bytes=4 third=?
```

- `[T; N]` is a fixed-length array value; `[value; N]` repeats one value.
- `&[T]` and `&mut [T]` are slices: a pointer plus a length. `&array` converts
  to a slice where a slice is expected, and `&array[a..b]` takes a sub-range.
- A string literal `"..."` is an array of `char`, and `b"..."` is an array of
  `u8`. Take `&` to get a slice. There is no implicit NUL terminator.
- Indexing with `values[i]` is unchecked. `get(i)` returns an optional pointer
  instead, and the standard library provides further checked operations such
  as `copyFrom` and `equals` on slices.
- `for &value in slice` copies each element. To write elements, index the
  slice.

## Optionals And Errors

```nia
using std::io;
using std::fmt;
using std::process;

enum ParseError {
    Empty,
    BadDigit(char),
}

// `?T` holds either a value (`?value`) or `null`.
fn firstChar(text: &[char]) ?char {
    match text.get(0) {
        ?ch => ?ch.*,
        null => null,
    }
}

// `.?` on an optional returns `null` from this function when empty.
fn firstTwice(text: &[char]) ?u32 {
    let ch = firstChar(text).?;
    ?(ch as u32 * 2)
}

// `E!T` holds either a success value (`!value`) or an error (`error!`).
fn parseDigits(text: &[char]) ParseError!u32 {
    if text.len() == 0 {
        return ParseError::Empty!;
    }
    let mut value: u32 = 0;
    for &ch in text {
        if ch < '0' or ch > '9' {
            return ParseError::BadDigit(ch)!;
        }
        value = value * 10 + (ch as u32 - '0' as u32);
    }
    !value
}

// `.?` on an error union returns the error from this function.
fn sumOf(left: &[char], right: &[char]) ParseError!u32 {
    let a = parseDigits(left).?;
    let b = parseDigits(right).?;
    !(a + b)
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let total = match sumOf(&"40", &"2") {
        !value => value,
        ParseError::Empty! => return process::ExitCode(1)!,
        ParseError::BadDigit(_)! => return process::ExitCode(2)!,
    };
    if sumOf(&"4x", &"2") is !_ {
        return process::ExitCode(3)!;
    }
    let doubled = if firstTwice(&"A") is ?value { value } else { 0 };
    let missing = if firstTwice(&"") is ?_ { 1 } else { 0 };
    io::debugPrint(&"total={} doubled={} missing={}\n", &[&total, &doubled, &missing]).?;
    !()
}
```

```text
total=42 doubled=130 missing=0
```

Nia has no exceptions. A value that may be absent has type `?T`, built with
`?value` or `null`. An operation that can fail returns `E!T`, built with
`!value` for success and `error!` for failure; `E` can be any type, often an
enum of failure reasons.

The postfix `.?` unwraps the success value or returns early from the current
function: `null` from a function returning an optional, or the error from a
function returning an error union. When the two error types differ, `.?`
converts through a `std::builtin::IntoError` implementation.

Patterns take these values apart: `?x` and `null` for optionals, `!x` and
`err!` for error unions, nested as deep as needed (`?!x`).

## Defer And Explicit Allocation

```nia
using std::io;
using std::fmt;
using std::mem;
using std::process;

struct Log {
    entries: [i32; 4],
    length: usize,
}

extend Log {
    fn record(&mut self, value: i32) () {
        self.entries[self.length] = value;
        self.length += 1;
    }
}

fn cleanupOrder(log: &mut Log) () {
    defer log.record(1);
    defer log.record(2);
    log.record(3);
}   // on exit: record(2), then record(1)

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut log = Log { entries: [0; 4], length: 0 };
    cleanupOrder(&mut log);

    // Allocation is explicit: the program chooses an allocator and passes it
    // to every operation that allocates or frees.
    let mut page: mem::PageAllocator = .init();
    let mut allocator: mem::GeneralPurposeAllocator = .init(&mut page);
    defer allocator.deinit().?;

    let mut values: std::ArrayList[i32] = .init();
    defer values.deinit(&mut allocator).?;
    values.push(&mut allocator, 10).?;
    values.push(&mut allocator, 20).?;

    let mut text = std::String::init();
    defer text.deinit(&mut allocator).?;
    text.append(&mut allocator, &"nia").?;

    let count = values.len();
    let length = text.len();
    io::debugPrint(&"log={} {} {} values={} text={}\n", &[
        &log.entries[0], &log.entries[1], &log.entries[2], &count, &length,
    ]).?;
    !()
}
```

```text
log=3 2 1 values=2 text=3
```

`defer` runs an expression when the enclosing block exits, by any path:
falling off the end, `return`, `break`, `continue`, or `.?`. Several defers run
in reverse order. The deferred expression must have type `()`, and it may
itself use `.?`.

Nia never allocates implicitly. Containers such as `std::ArrayList`,
`std::HashMap`, and `std::String` take an allocator argument on every call that
may allocate or free, and must be released with `deinit` before their
allocator is torn down. Writing `defer value.deinit(&mut allocator).?;` right
after creating a value keeps that order correct. `std::mem` provides
`PageAllocator`, `GeneralPurposeAllocator`, `ArenaAllocator`, and
`FixedBufferAllocator`.

## Generics And Traits

```nia
using std::io;
using std::fmt;
using std::process;

// A generic struct and generic methods.
struct Pair[T] {
    first: T,
    second: T,
}

extend[T] Pair[T] {
    fn swap(self) Pair[T] {
        Pair[T] { first: self.second, second: self.first }
    }
}

// A trait with one required method and one default method.
trait Shape {
    fn area(&self) i32;

    fn isLarge(&self) bool {
        self.area() > 100
    }
}

struct Square {
    side: i32,
}

extend Square : Shape {
    fn area(&self) i32 {
        self.side * self.side
    }
}

struct Money {
    cents: i32,
}

// Operators are traits too: `a + b` needs `Add`.
extend Money : Add[Money] {
    type Output = Money;

    fn add(self, rhs: Money) Money {
        Money { cents: self.cents + rhs.cents }
    }
}

// Bounds are written in a `where` clause; generic code may only use what
// its bounds promise.
fn total[T](values: &[T], start: T) T
where T: Add[T, Output = T] + Sized
{
    let mut sum = start;
    for &value in values {
        sum = sum + value;
    }
    sum
}

fn describe[S](shape: &S) i32
where S: Shape
{
    if shape.isLarge() { 1 } else { 0 }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let pair = Pair[i32] { first: 1, second: 2 }.swap();
    let ints = total(&[1, 2, 3], 0);
    let prices = [Money { cents: 250 }, Money { cents: 199 }];
    let bill = total(&prices, Money { cents: 0 });
    let large = describe(&Square { side: 11 });
    io::debugPrint(&"first={} ints={} cents={} large={}\n", &[
        &pair.first, &ints, &bill.cents, &large,
    ]).?;
    !()
}
```

```text
first=2 ints=6 cents=449 large=1
```

Type parameters are written in brackets, as in `Pair[T]`, `fn total[T]`, and
`extend[T] Pair[T]`. Type arguments are usually inferred; `Pair[i32]` or
`total[i32](...)` states them explicitly.

A trait declares methods that types implement in `extend Type : Trait`
blocks. A method with a body in the trait is a default that implementations
may leave out. Dispatch is static.

Generic code may use only what its `where` clause promises. Operators are
traits: `a + b` requires `Add`, `==` requires `Eq`, `<` requires `Ord`, and
implementing them gives a user type the operator. Iterating a `&[T]` in
generic code requires the `Sized` bound on `T`.

## Closures

```nia
using std::io;
using std::fmt;
using std::process;
using std::iter;

// `&Fn(i32) i32` is a non-owning view of any closure with that signature.
fn applyTwice(f: &Fn(i32) i32, value: i32) i32 {
    f(f(value))
}

fn double(x: i32) i32 {
    x * 2
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;

    let increment = \x -> x + 1;
    let step = 10;
    let addStep = \[step] x -> x + step;           // captures `step` by value

    let mut calls = 0;
    let counted = \[&mut calls] x -> {            // captures a writable pointer
        calls.* += 1;
        x
    };

    let a = applyTwice(&increment, 0);
    let b = applyTwice(&addStep, 1);
    let c = applyTwice(&counted, 5);

    // A plain function pointer has no captured state.
    let pointer: &fn(i32) i32 = &double;
    let d = pointer(21);

    // Iterator adapters take closures too.
    let squares = (1..5).iter().fold(0, &\total, value -> total + value * value);

    io::debugPrint(&"a={} b={} c={} calls={} d={} squares={}\n", &[
        &a, &b, &c, &calls, &d, &squares,
    ]).?;
    !()
}
```

```text
a=2 b=21 c=5 calls=2 d=42 squares=30
```

A closure is `\parameters -> expression`. It sees only its parameters and
module-level items unless it captures locals explicitly: `[name]` copies the
value, `[&name]` captures a read-only pointer, and `[&mut name]` a writable
one, used through `.*` in the body.

Parameter types are inferred from how the closure is used. Written directly
where a callable is expected, as in the `fold` call, it needs no annotations.
A closure bound with `let` can use the same inference when a later `&` address
conversion supplies its callable type; an annotation is needed only when the
body and all callable uses leave a parameter unconstrained.

A function that accepts any closure takes a callable view, `&Fn(Args) Return`
or `&mut Fn(Args) Return`, made with `&closure`. A view points at the closure's
state: it never allocates and must not outlive the closure, so the compiler
rejects returning it or storing it past the closure's scope. Ordinary pointers
carry no such check; keeping them valid is the program's job.
`&fn(Args) Return` is a plain function pointer; named functions and closures
without captures convert to it.

## Modules And Visibility

Each file is a module. A file loads its child modules with `module name;`, and
`using` brings names into scope. A program with this layout:

```text
main.nia
geometry.nia
geometry/shapes.nia
```

`main.nia`:

```nia
module geometry;

using std::io;
using std::fmt;
using std::process;
using geometry::{Point, origin};
using geometry::shapes::Rect;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let corner = Point { x: 3, y: 4 };
    let rect = Rect { min: origin(), max: corner };
    let area = rect.area();
    io::debugPrint(&"area={}\n", &[&area]).?;
    !()
}
```

`geometry.nia`:

```nia
pub module shapes;

pub struct Point {
    x: i32,
    y: i32,
}

pub fn origin() Point {
    Point { x: 0, y: 0 }
}

// Private: visible only inside this module.
fn clamp(value: i32) i32 {
    if value < 0 { 0 } else { value }
}

// Visible anywhere in the current package.
pub(pkg) fn span(low: i32, high: i32) i32 {
    clamp(high - low)
}
```

`geometry/shapes.nia`:

```nia
using super::{Point, span};

pub struct Rect {
    min: Point,
    max: Point,
}

extend Rect {
    pub fn area(&self) i32 {
        span(self.min.x, self.max.x) * span(self.min.y, self.max.y)
    }
}
```

```text
area=12
```

Children of the entry file sit next to it; children of `geometry.nia` live in
`geometry/`. Items are private to their module by default. `pub` exports an
item, `pub(super)` exposes it to the parent module, and `pub(pkg)` to the whole
package. `pub module` makes a child module part of the public surface, and
`pub using` re-exports names from another module.

`using` paths start from a module in scope or from a root: `std` for the
standard library, `pkg` for the current package, `entry` for the program's
entry package, `self` and `super` for the current and parent modules, or a
package name declared by the build script. `using` accepts groups
(`using std::{io, fmt};`), renames (`using geometry::origin as zero;`), and
wildcards (`using geometry::*;`).

## Conditional Compilation

`@[if condition]` keeps an item or statement only for matching targets. The
condition reads target facts such as `os`, `arch`, and `pointerWidth`; the
same facts are constants in `std::builtin::target`:

```nia
using std::builtin::target;
using std::io;
using std::fmt;
using std::process;

@[if os == "windows"]
fn platformName() [char; 7] {
    "windows"
}

@[if os != "windows"]
fn platformName() [char; 5] {
    "posix"
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let name = platformName();
    let width = target::pointerWidth;
    io::debugPrint(&"platform={} pointerWidth={}\n", &[&name, &width]).?;
    !()
}
```

On Windows x64 this prints:

```text
platform=windows pointerWidth=64
```

Inactive code must still parse but is not type-checked, so check a
multi-target program once per target with `--target`. The facts for each
target are listed in [Platform support](../reference/platform-support.md#target-facts).
`@[debug]`, `@[release]`, and `@[test]` select code by build profile and test
compilation.

## Where Next

- [Language reference](../reference/language.md): every rule in full,
  including unions, pointers, constant evaluation, and attributes.
- [C interop](c-interop.md): `extern` declarations, linking native libraries,
  and exporting functions.
- [Build scripts](build-scripts.md): packages, tests, and build steps.
- [Standard library](../../lib/README.md): collections, I/O, files,
  processes, formatting, and allocators.
- [Examples](../../examples/README.md): longer complete programs.
