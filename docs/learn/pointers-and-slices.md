# Pointers, Arrays, And Slices

Nia is a systems language: programs work with addresses and contiguous memory
directly. This chapter explains pointers, Nia's automatic addresses, arrays,
slices, and text, and then the one responsibility that comes with all of
them: keeping pointers valid.

Nia does not have a borrow checker, lifetimes, or a garbage collector. A
pointer is a plain value, like an address in C. The type system tracks whether
a pointer is read-only or writable and what it points to; it does not track
how long the memory lives. That part is the program's job.

## Pointers

| Type | Meaning |
| --- | --- |
| `&T` | a read-only pointer to a `T` |
| `&mut T` | a writable pointer to a `T` |

`&value` takes a read-only address and `&mut value` a writable one. `p.*`
reads or writes what `p` points to:

```nia fragment
let mut count = 1;
let reader = &count;
let writer = &mut count;
writer.* += 1;
_ = reader.*;
```

Writing through a read-only pointer is an error
(`assignment target is not assignable: pointer is read-only`), and so is
taking `&mut` of a `let` binding. A `&mut T` can be passed where a `&T` is
expected; every other pointer conversion is an explicit cast.

Field access and method calls look through a pointer by themselves: if `p` is
a `&Point`, then `p.x` and `p.lengthSquared()` work without `p.*`.

There is no null pointer. A pointer that may be absent is an optional,
`?&T`, which the [next chapter](errors.md) covers. There is no pointer
arithmetic either; cast to `usize`, compute, and cast back when you really
need it.

## Automatic Addresses

Most code never writes `&`. When a function expects `&T` and you pass a `T`,
Nia takes the address for you:

```nia
struct Point {
    x: i32,
    y: i32,
}

fun lengthSquared(p: &Point) i32 {
    p.x * p.x + p.y * p.y
}

fun translate(p: &mut Point, dx: i32) () {
    p.x += dx;
}

fun demo() i32 {
    let mut point = Point { x: 3, y: 4 };
    translate(point, 1);          // passes &mut point
    lengthSquared(point)          // passes &point
}
```

The rules are the same for arguments, initializers, return values, and method
receivers:

- Where `&T` is expected, any value of type `T` works, including a temporary
  such as `lengthSquared(Point { x: 1, y: 2 })`. A temporary lives until the
  end of the enclosing block.
- Where `&mut T` is expected, the value must be a writable place: a `let mut`
  variable, one of its fields or elements, or something reached through a
  writable pointer. Passing a `let` binding or a temporary is an error,
  because writing to it would either break the `let` promise or be lost.
- At most one `&` is added: a `T` never becomes a `&&T`.

Write `&` yourself when you want a pointer as a value of its own, such as
`let view = &point;`, where nothing says a pointer is expected.

> **Coming from C:** passing `point` to a `&Point` parameter does not copy the
> struct; it passes its address, exactly as `&point` would. The function can
> only write through it if the parameter is `&mut`.

## Arrays

`[T; N]` is an array of `N` values of type `T`. The length is part of the
type and known at compile time:

```nia fragment
let primes = [2, 3, 5, 7];        // [i32; 4]
let zeros: [u8; 16] = [0; 16];    // sixteen zeros
let empty: [i32; 0] = [];
_ = primes[0] + primes[3];
_ = zeros;
_ = empty;
```

An array is a value, like a struct. Assigning it or passing it by value copies
every element. `primes[i]` indexes it, and indexes are `usize`.

Indexing does not check bounds. Reading past the end of an array is a bug that
the language does not catch, as in C. When an index might be out of range, use
the checked `get`, which returns an optional pointer (it comes from
`std::slice`, introduced below):

```nia
using std::slice;

fun third(values: [i32; 4]) i32 {
    match values.get(2) {
        ?value => value.*,
        null => 0,
    }
}
```

## Slices

A slice, `&[T]` or `&mut [T]`, is a pointer to a run of elements together with
their count. It is how functions accept any number of elements:

```nia
fun sum(values: &[i32]) i32 {
    let mut total = 0;
    for &value in values {
        total += value;
    }
    total
}

fun fill(values: &mut [i32], value: i32) () {
    for index in 0..values.len() {
        values[index] = value;
    }
}

fun demo() i32 {
    let numbers = [1, 2, 3, 4, 5];
    let mut buffer = [0; 4];

    let all = sum(numbers);               // the whole array, as a slice
    let middle = sum(&numbers[1..4]);     // elements 1, 2, and 3
    fill(buffer, 7);                      // a writable slice of `buffer`
    fill(&mut buffer[2..], 9);            // only the last two elements
    all + middle + sum(buffer)
}
```

- An array converts to a slice of the whole array where a slice is expected,
  through the same automatic address. A `&mut [T]` needs a `let mut` array.
- `&values[a..b]` takes a sub-slice; `a..`, `..b`, and `..` leave out an end.
  Here the `&` is required, because the range alone is not a value.
- `values.len()` is the element count. Arrays have `len()` too.
- `for &value in values` copies each element out. To change elements, index
  the slice. An array is not iterable itself; loop over `&array[..]`.

Calling a slice method on an array also works directly: `numbers.len()` and
`numbers.get(2)` view the array as a slice for the call.

## Text

A string literal is an array of `char`, not a pointer:

```nia fragment
let word = "héllo";        // [char; 5]: five Unicode scalar values
let bytes = b"nia\0";      // [u8; 4]: bytes, with the NUL written explicitly
let text: &[char] = word;  // a slice of the array
_ = text.len();
_ = bytes.len();
```

Functions that read text take `&[char]`, and a literal can be passed to them
directly. There is no implicit NUL terminator; C strings are covered in
[C interop](../guides/c-interop.md#c-strings). Comparing text, searching it,
and similar operations are slice methods in `std::slice`:

```nia
using std::slice;

fun isYes(answer: &[char]) bool {
    answer.equals("yes") or answer.equals("y")
}
```

Text that grows, such as text built at runtime, is a `std::String`, which
owns its memory through an allocator; [Memory](memory.md) shows how.

## Keeping Pointers Valid

A pointer is valid while the memory it points to exists. Nia checks the type
of every pointer but not this, so it is worth knowing where memory ends:

| Memory | Lives until |
| --- | --- |
| a local variable or parameter | the function returns |
| a temporary, such as a literal or an expression's result | the end of the enclosing block |
| a `static` | the program ends |
| an element of a container | the container is freed, or grows and moves its elements |

The most common mistake is returning the address of something that dies with
the function. Both of these compile, and both return a pointer to memory that
no longer exists:

```nia
fun localAddress() &i32 {
    let value = 42;
    &value          // `value` ends when the function returns
}

fun badGreeting() &[char] {
    "hello"         // the literal is a temporary of this function
}
```

Return the value itself, or keep the data somewhere that outlives the call:

```nia
static greeting: [char; 5] = "hello";

fun goodGreeting() &[char] {
    greeting        // static storage lives as long as the program
}

fun greetingValue() [char; 5] {
    "hello"         // a copy of the array, returned by value
}
```

The same applies to pointers kept in a struct or passed to code that stores
them. The one place Nia does check lifetimes is closures, which the
[closures chapter](closures.md) explains.

> **Coming from Rust:** there is nothing like a borrow checker. Read-only and
> writable pointers can point to the same value at the same time. The design
> trusts the programmer and keeps the language small; the
> [memory chapter](memory.md) shows the conventions that keep ownership clear.

## Putting It Together

```nia
using std::io;
using std::process;
using std::slice;

fun countVowels(text: &[char]) usize {
    let vowels = "aeiou";
    let mut count: usize = 0;
    for &ch in text {
        for &vowel in &vowels[..] {
            if ch == vowel {
                count += 1;
            }
        }
    }
    count
}

fun reverse(values: &mut [char]) () {
    let n = values.len();
    for i in 0..n / 2 {
        let other = n - 1 - i;
        let saved = values[i];
        values[i] = values[other];
        values[other] = saved;
    }
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut word = "systems";
    let vowels = countVowels(word);
    reverse(word);
    let palindrome = word.equals("smetsys");
    io::debugPrint("vowels={} reversed={} matches={}\n", [vowels, word, palindrome]).?;
    !()
}
```

```text
vowels=1 reversed=smetsys matches=true
```

## Summary

- `&T` and `&mut T` are plain pointers; `.*` dereferences, and fields and
  methods look through pointers by themselves.
- Pass values where pointers are expected and Nia takes the address; a
  writable pointer needs a writable place.
- Arrays are values with a compile-time length; slices are pointer-and-length
  views; string literals are `char` arrays.
- Indexing is unchecked; `get` is the checked alternative.
- Nia does not track how long memory lives. Never return or keep a pointer to
  a local or a temporary.

The reference covers [pointers](../reference/language.md#pointers),
[arrays](../reference/language.md#arrays), and
[slices](../reference/language.md#slices). Next,
[Optionals And Errors](errors.md).
