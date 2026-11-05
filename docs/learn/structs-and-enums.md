# Structs, Enums, And Patterns

Programs model their data with their own types. This chapter covers the three
kinds Nia provides, structs, enums, and unions, then methods, which attach
behavior to types, and pattern matching, which takes values apart.

## Structs

A struct groups named fields:

```nia
struct Point {
    x: i32,
    y: i32,
}

fun origin() Point {
    Point { x: 0, y: 0 }
}

fun moved(p: Point, dx: i32) Point {
    let x = p.x + dx;
    Point { x, y: p.y }   // `x` is short for `x: x`
}
```

A struct literal names its type and sets every field; leaving one out is an
error (`missing struct field 'y'`). Fields are read and written with `.`, and
writing needs a `let mut` binding. Fields carry no visibility of their own:
wherever the struct is visible, so are its fields.

When the type is already known from context, the literal can leave the name
out with `.{ ... }`:

```nia continued
fun corner() Point {
    let p: Point = .{ x: 3, y: 4 };
    p
}
```

A tuple struct has positional fields, read as `.0`, `.1`, and so on. It is
useful for giving an existing type a distinct meaning:

```nia
struct Meters(f64)

fun half(distance: Meters) Meters {
    Meters(distance.0 / 2.0)
}
```

Nia picks the memory layout of an ordinary struct and may reorder its fields.
`extern struct` keeps C's layout; see [C interop](../guides/c-interop.md).

## Methods

Methods live in `extend` blocks, separate from the type's declaration:

```nia
struct Point {
    x: i32,
    y: i32,
}

extend Point {
    // No receiver: an associated function, called as `Point::init(...)`.
    fun init(x: i32, y: i32) Point {
        Point { x, y }
    }

    // `&self`: reads the receiver through a read-only pointer.
    fun lengthSquared(&self) i32 {
        self.x * self.x + self.y * self.y
    }

    // `&mut self`: may modify the receiver.
    fun translate(&mut self, dx: i32, dy: i32) () {
        self.x += dx;
        self.y += dy;
    }

    // `self`: takes the receiver by value, a copy.
    fun swapped(self) Self {
        Self { x: self.y, y: self.x }
    }
}

fun demo() i32 {
    let mut p = Point::init(3, 4);
    p.translate(1, 1);
    let q = p.swapped();
    q.lengthSquared()
}
```

The receiver form says how the method sees its value:

| Receiver | The method gets | Call needs |
| --- | --- | --- |
| `&self` | a read-only pointer to the value | any value |
| `&mut self` | a writable pointer | a `let mut` binding or another writable place |
| `self` | a copy of the value | any value |
| none | nothing; an associated function | `Type::name(...)` |

Calling `p.translate(1, 1)` passes `p`'s address automatically; the caller
never writes `&p`. Calling it on a `let` binding fails with
`receiver is not assignable: local is let`. Inside `extend`, `Self` names the
extended type. A method can also be called through a pointer to the value:
`ptr.lengthSquared()` works when `ptr` is a `&Point`.

Values in Nia are plain data. Passing a struct by value copies it, and there
are no constructors or destructors; an `init` associated function is only a
naming convention, which the standard library follows.

> **Coming from Rust:** `extend` plays the role of `impl`, but there is no
> borrow checker and no move semantics. A value passed by value is copied, and
> the original stays usable.

## Enums

An enum is a value that is exactly one of several variants. A variant can be a
plain name, carry positional data, or carry named fields:

```nia
enum Shape {
    Circle(i32),
    Rect { width: i32, height: i32 },
    Empty,
}

fun shapes() [Shape; 3] {
    [Shape::Circle(2), Shape::Rect { width: 3, height: 4 }, .Empty]
}
```

Variants are named through the enum, `Shape::Circle(2)`. Where the expected
type is already an enum, the enum name can be left out: `.Empty`. This is the
same omission as `.{ ... }` for structs.

An enum whose variants carry no data can choose an integer *backing type* and
explicit values, and converts to an integer with `as`:

```nia
enum Level: u8 {
    Low = 1,
    Middle,
    High = 10,
}

fun code(level: Level) u8 {
    level as u8   // Low is 1, Middle 2, High 10
}
```

Without a backing type, variants are numbered from 0 in a `u8`. Integers do
not convert back to an ordinary enum, because most integers are not valid
variants. An *open* enum, whose body ends with `_`, accepts every value of its
backing type; it is useful for numbers defined by a file format or protocol.

## Matching

`match` compares a value against patterns, in order, and runs the first arm
that matches:

```nia
using std::io;
using std::process;

enum Shape {
    Circle(i32),
    Rect { width: i32, height: i32 },
    Empty,
}

fun area(shape: Shape) i32 {
    match shape {
        .Circle(radius) => 3 * radius * radius,
        .Rect { width, height } => width * height,
        .Empty => 0,
    }
}

fun grade(score: i32) [char; 1] {
    match score {
        90..=100 => "A",
        75..90 => "B",
        0, 1, 2 => "?",
        _ => "C",
    }
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let shapes = [Shape::Circle(2), .Rect { width: 3, height: 4 }, .Empty];
    let mut total = 0;
    for &shape in &shapes[..] {
        total += area(shape);
    }
    io::debugPrint("total={} grade={}\n", [total, grade(80)]).?;
    !()
}
```

```text
total=24 grade=B
```

An array is not iterable itself; `&shapes[..]` takes a slice of all of it,
which is. Slices are the subject of the next chapter.

A `match` must be exhaustive: every possible value must be covered, or the
compiler names a missing case:

```text
error[E0301]: non-exhaustive matched, missing pattern: `Rect { width: _, height: _ }`
```

This is what makes enums safe to extend: adding a variant points out every
`match` that needs to handle it. `_` matches anything left over, and an arm
that earlier arms already cover is an error.

`match` is an expression; every arm's value must have the same type, except
arms that leave with `return`, `break`, or `continue`.

### Patterns

| Pattern | Matches |
| --- | --- |
| `_` | anything, without binding it |
| `name` | anything, binding it to `name` |
| `42`, `'a'`, `true` | that integer, `char`, or `bool` |
| `1..5`, `1..=5` | an integer in the range (excluding or including the end) |
| `0, 1, 2` | any of several patterns (in one arm, without bindings) |
| `.Variant`, `Enum::Variant(p)`, `.Variant { field, .. }` | an enum variant and its data |
| `Point { x: 0, y }`, `Point { x, .. }` | a struct by its fields; `..` skips the rest |
| `(a, _)` | a tuple element by element |
| `&p` | a pointer, matching what it points to against `p`; `for &x in slice` uses this to copy each element |

A bare name in a pattern always introduces a new binding. To compare against
a constant, wrap it in parentheses or name it through a path:

```nia
const limit: i32 = 100;

fun atLimit(value: i32) bool {
    match value {
        (limit) => true,
        _ => false,
    }
}
```

Writing `limit => true` would bind any value to a new name `limit`, and the
`_` arm after it would be reported as unreachable.

### Testing One Pattern With `is`

When only one shape matters, `if value is pattern` tests it, and its bindings
are visible inside the `if` branch:

```nia
enum Shape {
    Circle(i32),
    Rect { width: i32, height: i32 },
    Empty,
}

fun rectArea(shape: Shape) i32 {
    if shape is .Rect { width, height } and width > 0 {
        width * height
    } else {
        0
    }
}
```

Conditions chain with `and`, and each clause sees the bindings of the clauses
before it. A chain with `is` cannot use `or` or `not`; use `match` for
alternatives.

### Destructuring In `let`

A `let` can take apart a value whose pattern always matches:

```nia
struct Point {
    x: i32,
    y: i32,
}

fun sum(point: Point, pair: (i32, i32)) i32 {
    let Point { x, y } = point;
    let (a, b) = pair;
    x + y + a + b
}
```

## Unions

A `union` stores one of several fields in the same memory, without recording
which. Reading a field other than the one last written reinterprets the bytes,
so unions are for low-level code such as C interop and binary formats. An
enum is the safe choice whenever the program needs to know which case it has.
The [reference](../reference/language.md#unions) has the details.

## Summary

- Structs group named or positional fields; `.{ ... }` and `.Variant` leave out
  a type name that context already supplies.
- Methods live in `extend` blocks; the receiver form, `&self`, `&mut self`, or
  `self`, says how the method sees the value, and the caller's `&` is implicit.
- Values are plain data, copied when passed by value, with no destructors.
- Enums hold one variant at a time; `match` must cover every variant, and `is`
  tests a single pattern.
- A bare name in a pattern binds; compare against a constant with `(name)`.

The reference covers [structs](../reference/language.md#structs),
[enums](../reference/language.md#enums),
[methods](../reference/language.md#methods), and
[patterns](../reference/language.md#patterns). Next,
[Pointers, Arrays, And Slices](pointers-and-slices.md) covers how Nia refers
to memory.
