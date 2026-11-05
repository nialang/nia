# Generics And Traits

Generic code works for many types at once, and traits describe what those
types must be able to do. Nia's generics are compiled per type, like C++
templates or Rust generics: each use with a concrete type produces its own
code, with no runtime cost. Trait objects provide dynamic dispatch where it is
wanted instead.

## Generic Functions

Type parameters go in brackets after the name:

```nia
fun first[T](values: &[T]) T
where T: Sized
{
    values[0]
}

fun pair[T, U](left: T, right: U) (T, U) {
    (left, right)
}

fun demo() i32 {
    let a = first([3, 4, 5]);        // T is i32, inferred
    let b = first[u8]([1, 2]);       // T given explicitly
    let (c, _) = pair(a, true);
    c + b as i32
}
```

Type arguments are usually inferred from the arguments and the expected
result. When they cannot be, write them in brackets after the name, all of
them or `_` for those to infer: `pair[i32, _](1, true)`.

## Bounds

A generic function may only do with a `T` what its bounds promise. Without
them, even `a + b` or `a < b` is an error, because not every type has those
operators. Bounds go in a `where` clause, joined with `+`:

```nia
fun largest[T](values: &[T]) T
where T: Ord[T] + Sized
{
    let mut best = values[0];
    for &value in values {
        if value > best {
            best = value;
        }
    }
    best
}
```

- `Ord[T]` provides `<`, `<=`, `>`, and `>=`; `Eq[T]` provides `==` and `!=`;
  `Add[T, Output = T]` provides `+` returning a `T`; and so on for the other
  operators.
- `Sized` says the type has a size known at compile time. Copying a `T` out of
  a slice, as `for &value in values` does, or asking for
  `std::builtin::size[T]()` needs it.

The compiler checks the generic body once against its bounds, and each call
against the bounds for its concrete types:

```text
error[E0301]: trait bound not satisfied: Point: Ord[Point]
```

## Generic Types

Structs, unions, type aliases, and traits take type parameters the same way.
Methods for a generic type declare the parameters after `extend`:

```nia
struct Pair[T] {
    first: T,
    second: T,
}

extend[T] Pair[T] {
    fun swap(self) Pair[T] {
        Pair[T] { first: self.second, second: self.first }
    }
}

fun swapped() i32 {
    let p = Pair[i32] { first: 1, second: 2 }.swap();
    p.first
}
```

A parameter can also be a compile-time value, written `Name: Type`. Arrays use
this for their length:

```nia
struct Buffer[N: usize] {
    bytes: [u8; N],
}

fun capacity[N: usize](buffer: &Buffer[N]) usize {
    _ = buffer;
    N
}
```

## Traits

A trait declares methods that types can implement. A method with a body is a
default that implementations may keep:

```nia
trait Shape {
    fun area(&self) i32;

    fun isLarge(&self) bool {
        self.area() > 100
    }
}

struct Square {
    side: i32,
}

struct Rect {
    width: i32,
    height: i32,
}

extend Square : Shape {
    fun area(&self) i32 {
        self.side * self.side
    }
}

extend Rect : Shape {
    fun area(&self) i32 {
        self.width * self.height
    }

    fun isLarge(&self) bool {
        self.width > 10 or self.height > 10
    }
}

fun countLarge[S](shapes: &[S]) i32
where S: Shape + Sized
{
    let mut count = 0;
    for &shape in shapes {
        if shape.isLarge() {
            count += 1;
        }
    }
    count
}
```

`extend Type : Trait` implements a trait. The block contains exactly the
trait's members: every required method and nothing else. Other methods go in a
plain `extend Type` block.

Trait methods need no import: wherever the trait is visible, its
implementations are, so `square.area()` works in any module that can see
`Shape`. To keep this unambiguous, an implementation must live in the package
that defines the type or in the package that defines the trait.

`trait Child : Parent` declares a supertrait: a `T: Child` bound also provides
`Parent`'s methods. Each trait is still implemented separately.

## Operators Are Traits

Operators on your own types come from the same mechanism. `a + b` calls
`Add`, `a == b` calls `Eq`, `a < b` calls `Ord`:

```nia
struct Money {
    cents: i32,
}

extend Money : Add[Money] {
    type Output = Money;

    fun add(self, rhs: Money) Money {
        Money { cents: self.cents + rhs.cents }
    }
}

extend Money : Eq[Money] {
    fun eq(self, rhs: Money) bool {
        self.cents == rhs.cents
    }

    fun ne(self, rhs: Money) bool {
        self.cents != rhs.cents
    }
}

fun total(prices: &[Money]) Money {
    let mut sum = Money { cents: 0 };
    for &price in prices {
        sum = sum + price;
    }
    sum
}
```

A struct has no `==` until it implements `Eq`; Nia never derives equality or
other behavior on its own. The full list of operator traits is in the
[reference](../reference/language.md#built-in-traits).

## Associated Types

A trait can let each implementation choose a type, such as the element type of
an iterator. Code refers to it with the explicit form `[T as Trait]::Name`, and
bounds can fix it: `I: Iterator[Item = i32]`.

Implementing the built-in `Iterator` trait makes a type usable in `for`:

```nia
using std::io;
using std::process;

struct Countdown {
    remaining: i32,
}

extend Countdown : Iterator {
    type Item = i32;

    fun next(&mut self) ?i32 {
        if self.remaining == 0 {
            return null;
        }
        self.remaining -= 1;
        ?(self.remaining + 1)
    }
}

fun sumAll[I](items: I) i32
where I: Iterator[Item = i32]
{
    let mut total = 0;
    for value in items {
        total += value;
    }
    total
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let countdown = Countdown { remaining: 3 };
    let mut digits = 0;
    for n in countdown {
        digits = digits * 10 + n;
    }
    io::debugPrint("digits={} sum={}\n", [digits, sumAll(Countdown { remaining: 4 })]).?;
    !()
}
```

```text
digits=321 sum=10
```

`next` returns `null` when the iteration is over. A struct literal cannot
appear directly after `in`, because the `{` would be read as the loop body;
bind it to a name first, as above.

## Trait Objects

A generic function is compiled once per type. When one piece of code must
handle values of different types at runtime, such as a list of shapes of mixed
kinds, use a trait object instead. `&Trait` is a pointer to any value that
implements `Trait`, and calls through it are dispatched at runtime:

```nia
trait Shape {
    fun area(&self) i32;
}

struct Square {
    side: i32,
}

struct Rect {
    width: i32,
    height: i32,
}

extend Square : Shape {
    fun area(&self) i32 {
        self.side * self.side
    }
}

extend Rect : Shape {
    fun area(&self) i32 {
        self.width * self.height
    }
}

fun largestArea(shapes: &[&Shape]) i32 {
    let mut largest = 0;
    for &shape in shapes {
        if shape.area() > largest {
            largest = shape.area();
        }
    }
    largest
}

fun demo() i32 {
    let square = Square { side: 9 };
    let rect = Rect { width: 4, height: 30 };
    let mixed: [&Shape; 2] = [square, rect];
    largestArea(mixed)
}
```

A value converts to a trait object where one is expected, through the usual
automatic address. A trait can be used this way only when every method takes
`&self` or `&mut self` and has no type parameters of its own.

| Use | When |
| --- | --- |
| `fun f[T](x: &T) where T: Shape` | the type is known at each call; fastest, one copy of the code per type |
| `fun f(x: &Shape)` | values of different types meet in one place at runtime |

## Summary

- `[T]` parameters make functions and types generic; `where` bounds say what
  the generic code may do with a `T`.
- Traits declare methods; `extend Type : Trait` implements them. Trait methods
  need no import.
- Operators are traits, and nothing is derived automatically.
- Associated types are written `[T as Trait]::Name`; implementing `Iterator`
  makes a type work with `for`.
- `&Trait` gives dynamic dispatch when static generics do not fit.

The reference covers [generics](../reference/language.md#generics) and
[traits](../reference/language.md#traits). Next, [Closures](closures.md).
