# Closures

A closure is an anonymous function written inline. Nia closures are small and
explicit: they capture exactly the variables they list, they live on the stack
like any other local value, and the compiler checks that no pointer to one
outlives it. This chapter covers writing closures, passing them to functions,
and the three ways a function can accept something callable.

## Writing A Closure

A closure is `\`, its parameters, `->`, and one expression, which may be a
block:

```nia fragment
let increment = \x -> x + 1;
let add = \a, b -> a + b;
let describe = \x: i32 -> {
    let doubled = x * 2;
    doubled + 1
};
_ = increment(1) + add(2, 3) + describe(4);
```

A closure is called like a function. Its parameter types are inferred from how
it is used, here from the calls; annotate a parameter (`\x: i32 -> ...`) only
when no use determines it, which the compiler reports:

```text
error[E0301]: cannot infer closure parameter `x`; add a type annotation or provide a callable context
```

There is no return type annotation; the result type comes from the body.

A closure has one fixed signature. Using the same closure as both
`Fun(i32) i32` and `Fun(bool) bool` is an error, and the message points at
both uses:

```text
error[E0301]: closure uses have conflicting types
  --> main.nia:6:7
  |
6 |     b(id)
  |       ^^ this closure use conflicts with the earlier use
  ::: main.nia:5:9
  |
5 |     _ = a(id);
  |         ----- the closure was first used here
```

## Captures

A closure body sees its parameters, its own locals, and module-level items.
Any local of the surrounding function must be listed in brackets before the
parameters:

| Capture | Inside the body |
| --- | --- |
| `[name]` | a copy of `name`, taken when the closure is created |
| `[&name]` | a read-only pointer to `name`; read it with `name.*` |
| `[&mut name]` | a writable pointer to `name`; update it with `name.* += 1` |

```nia
using std::io;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let step = 10;
    let addStep = \[step] x -> x + step;      // copies `step`

    let mut calls = 0;
    let counted = \[&mut calls] x -> {        // points at `calls`
        calls.* += 1;
        x * 2
    };

    let a = addStep(1);
    let b = counted(5);
    let c = counted(6);
    io::debugPrint("a={} b={} c={} calls={}\n", [a, b, c, calls]).?;
    !()
}
```

```text
a=11 b=10 c=12 calls=2
```

Using a local without capturing it is an error. The first message only says
the name is unknown inside the closure; the second explains why:

```text
error[E0201]: unknown value `k`
error[E0302]: local `k` is not captured by this closure; add it to the closure capture list
```

Nothing is captured implicitly, including `self` inside a method: bind it to a
local and capture that. A copied capture is read-only inside the closure;
capture a pointer to change the original. An inner closure that needs an outer
closure's parameter captures it explicitly, as in
`\x -> \[x] y -> x + y`.

> **Coming from Rust:** captures are spelled out rather than inferred, and a
> pointer capture is an ordinary `&T` or `&mut T` that the body dereferences.

## Passing Closures To Functions

A function can accept a callable in three ways:

| Parameter | Accepts | Cost |
| --- | --- | --- |
| `f: &Fun(i32) i32` | any closure, function, or method with that signature | an indirect call through a two-word view |
| `f: F` with `where F: Fun(i32) i32` | any of the same, by value | a direct call; the function is compiled once per closure |
| `f: &fun(i32) i32` | a function or a closure without captures | an indirect call through a plain function pointer |

The first, a *callable view*, is the usual choice. A view points at the
closure's state; passing a closure, or a function name, where a view is
expected creates one:

```nia
using std::io;
using std::process;

fun applyTwice(f: &Fun(i32) i32, value: i32) i32 {
    f(f(value))
}

fun applyGeneric[F](f: F, value: i32) i32
where F: Fun(i32) i32
{
    f(value)
}

fun applyPointer(f: &fun(i32) i32, value: i32) i32 {
    f(value)
}

fun double(x: i32) i32 {
    x * 2
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let offset = 3;
    let shifted = \[offset] x -> x + offset;

    let a = applyTwice(shifted, 1);             // a closure as a view
    let b = applyTwice(double, 1);              // a function as a view
    let c = applyGeneric(\x -> x * x, 5);       // an inline closure, generic
    let d = applyPointer(double, 7);            // a function pointer
    let e = applyPointer(\x -> x - 1, 7);       // a capture-free closure
    io::debugPrint("a={} b={} c={} d={} e={}\n", [a, b, c, d, e]).?;
    !()
}
```

```text
a=7 b=4 c=25 d=14 e=6
```

`&mut Fun(A) R` is a writable view. Some standard-library APIs, such as
`test::Runner::runWith`, take one; forming it needs the closure in a `let mut`
binding, as with any writable pointer. Most code takes the read-only `&Fun`.

A plain function pointer, `&fun(A) R`, has no room for captured state, so a
closure with captures cannot become one:

```text
error[E0301]: capturing closures cannot be converted to thin function pointers
```

Methods and enum constructors are callables too: `Point::lengthSquared` can be
passed where a `&Fun(&Point) i32` is expected, and `Event::Data` where a
`&Fun(i32) Event` is.

## Closures Do Not Outlive Their Frame

A closure's state lives in the function that creates it, like any local. A
view or a captured pointer refers to that frame, so Nia checks that neither
escapes it. Returning a view, storing it in a static, or passing it to a
function that keeps it is an error:

```text
error[E0301]: stack-backed callable view cannot be returned; use it only while its closure state is live
```

This is the only lifetime check in Nia; ordinary pointers are the program's
responsibility, as the [pointers chapter](pointers-and-slices.md) explains.
Closures get the check because their captures make dangling state easy to
create without noticing. A callable that must outlive its frame needs an
explicit owner from a library, such as `mem::CallableAllocation`.

## Closures With Iterators

The iterator adapters in `std::iter` take closures, which is where inference
pays off: the parameter types come from the iterator.

```nia
using std::io;
using std::iter;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let squares = (1..5).iter().map(\value -> value * value);
    let total = squares.fold(0, \sum, value -> sum + value);
    let firstBig = (1..10).iter().position(\value -> value * value > 20);
    let index = if firstBig is ?i { i } else { 0 };
    io::debugPrint("total={} firstBig={}\n", [total, index]).?;
    !()
}
```

```text
total=30 firstBig=4
```

`map` is lazy: it produces a new iterator that applies the closure as values
are requested. `fold`, `position`, `count`, and `forEach` consume the
iterator. These are extension methods of `std::iter`, so the program imports
that module.

## Summary

- `\params -> expression` writes a closure; parameter types are inferred from
  its uses, and every use must agree on one signature.
- Captures are explicit: `[x]` copies, `[&x]` and `[&mut x]` point.
- Accept callables as `&Fun(A) R` views by default, as `F: Fun(A) R` generics
  for direct calls, or as `&fun(A) R` pointers when no state is involved.
- Functions, methods, and constructors can be passed wherever closures can.
- A view or a closure's captured pointers never leave the creating frame.

The reference covers [closures](../reference/language.md#closures) and
[callable views](../reference/language.md#callable-views). Next,
[Modules And Visibility](modules.md).
