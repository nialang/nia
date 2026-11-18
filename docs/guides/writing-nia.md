# Writing Idiomatic Nia

This guide is about writing Nia well: how to organize code, report and pass
on errors, iterate, lay out a file, and comment it, so that it reads the way
the standard library reads. It assumes the [Learning Nia](../learn/README.md)
chapters; each section links to the chapter or reference section that owns
the rules it relies on, and states only how to use them.

The standard library is written to be read. When this guide does not cover a
question, find where std or the [json](https://github.com/nialang/json)
library does the same thing and write it that way.

## Imports Follow What You Call

A trait method needs no import: `xs.len()`, `xs.iter()`, `it.next()`,
`a.checkedAdd(b)`, and every operator work wherever the value does. Every other method from
another module is an *extension method*, visible only where its module is
imported. Most of std's everyday operations are extension methods:

```nia
using std::iter;    // filter, count, map, fold, ...
using std::option;  // okOr, map, isNull, ...
using std::slice;   // equals, startsWith, get, copyFrom, ...

// The words of at least `min` letters. The closure captures `min`
// explicitly.
fun countLong(words: &[&[char]], min: usize) usize {
    words.iter().filter(\[min] word -> word.len() >= min).count()
}

// The first letter of `answer`, or `?` when it is empty.
fun initial(answer: &[char]) char {
    if answer.get(0) is ?letter { letter.* } else { '?' }
}

fun isAffirmative(answer: &[char]) bool {
    answer.equals("yes") or answer.startsWith("y")
}
```

Import the module of every extension method a file calls, even when the call
happens to compile without it: what another module imports never reaches this
one. A method whose module is not imported is an unknown method to the
compiler, which suggests a `using`;
[Extension Methods](../learn/modules.md#extension-methods) lists the modules
of std's common ones.

`nia check` reports every import that nothing needs, including one that only
made extension methods visible that no call uses: remove it.

## Errors Are a System, Not a `match`

Nia's error handling is built from a few parts that compose: a typed error per
module, conversion between error types, combinators that reshape a result,
and `.?`, which passes a failure to the caller. Code that uses them reads as a
sequence of steps; code that ignores them turns every call into a `match`.
[Optionals And Errors](../learn/errors.md) explains each part.

**Give each module its own error type, and keep the cause.** A variant per
reason for failing, carrying the error it came from:

```nia
using std::io;
using std::parse;
using std::process;
using std::result;

// Why a port could not be read.
enum PortError {
    // The text is not a number; keeps the parser's reason.
    Syntax(parse::Error),
    // The number is above 65535.
    TooLarge(u32),
}

fun parsePort(text: &[char]) PortError!u16 {
    let value = text.parse[u32]().mapError(.Syntax).?;
    if value > 65535 {
        return .TooLarge(value)!;
    }
    !(value as u16)
}
```

`mapError(.Syntax)` wraps the parser's error in this module's variant, and
`.?` passes it on. An enum variant is a function from its payload, so it needs
no closure, and the expected type lets it be written without its enum.

**Convert where the conversion is always the same.** When every error of one
type becomes the same variant of another, implement `IntoError` once and let
`.?` convert:

```nia continued
// Every port error ends the program with status 2.
extend PortError : std::builtin::IntoError[process::ExitCode] {
    fun intoError(self) process::ExitCode {
        _ = self;
        process::ExitCode(2)
    }
}
```

Use `mapError` where the variant depends on the call, and `IntoError` where
it does not. A package can use `IntoError` to convert a dependency's error to
its own error type. See [Where Implementations Live](../reference/language.md#where-implementations-live)
for the full rule.

**Examine failures at the boundary.** Functions in the middle pass errors on
with `.?`. The place that decides what a failure means, usually `main` or a
request handler, is where a `match` on the error belongs:

```nia continued
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let port = parsePort("8080").?;
    match parsePort("99999") {
        !_ => {},
        .TooLarge(value)! => io::debugPrint("{} is too large\n", [value]).?,
        .Syntax(_)! => io::debugPrint("not a number\n", []).?,
    }
    io::debugPrint("port {}\n", [port]).?;
    !()
}
```

```text
99999 is too large
port 8080
```

A function that matches every result only to return its error again is
`.?` written by hand. `okOr`, `mapError`, `orElse`, and `andThen` turn
most of the remaining matches into one call each; the
[table](../learn/errors.md#transforming-results) lists them.

**Clean up with `defer`, and keep cleanup failures.** Release a resource right
after acquiring it, with `defer`, so that every exit releases it. Cleanup can
fail too, and the failure should reach the caller:

```nia ignore
defer list.deinit(allocator).?;
```

Discard a cleanup result with `defer _ = list.deinit(allocator);` only where
it truly cannot matter, such as an allocator that a test tears down. An owner
that releases several resources attempts all of them with
`error::CleanupAccumulator`; see
[Cleaning Up](../learn/errors.md#cleaning-up).

## Optionals

An optional is read with `is` in a condition, reshaped with a combinator, or
turned into an error with `okOr`:

```nia
using std::iter;
using std::option;
using std::slice;

// The first even value, doubled; `null` when there is none.
fun firstEvenDoubled(values: &[i32]) ?i32 {
    let first = values.iter().find(\value -> value.* % 2 == 0).?;
    ?(first.* * 2)
}

fun describe(age: ?u8) u8 {
    if age is ?years { years } else { 0 }
}

fun nextAge(age: ?u8) ?u8 {
    age.map(\years -> years + 1)
}

fun isKnown(age: ?u8) bool {
    age != null
}
```

`.?` on an optional returns `null` from a function that returns an optional.
Optionals compare with `==` when their values do, so `age == ?30` and
`age == null` need no `match`.

## Iteration

`for` iterates anything `Iterable`: ranges, slices, and std's collections. A
slice yields pointers to its elements, which a `&x` pattern reads:

```nia
using std::iter;
using std::slice;

fun total(values: &[i32]) i32 {
    let mut sum = 0;
    for &value in values {
        sum += value;
    }
    sum
}
```

**Chain adapters for a computation over a sequence.** `std::iter` has lazy
adapters, `map`, `filter`, `filterMap`, `take`, `skip`, `stepBy`,
`takeWhile`, `skipWhile`, `enumerate`, `zip`, `chain`, `copied`, and `rev`,
and consumers such as `fold`, `count`, `find`, `any`, `all`, `min`, and
`max`:

```nia
using std::iter;
using std::slice;

// The sum of the squares of the values above `floor`.
fun squaresAbove(values: &[i32], floor: i32) i32 {
    values
        .iter()
        .copied()
        .filter(\[floor] value -> value > floor)
        .map(\value -> value * value)
        .fold(0, \sum, square -> sum + square)
}

// The position of the first negative value.
fun firstNegative(values: &[i32]) ?usize {
    values.iter().position(\value -> value.* < 0)
}
```

`copied` reads a slice iterator's `&T` items out as `T`. A loop is still the
clearer choice when the body updates several values at once or stops on a
condition that the adapters would have to thread through.

**Make your own sequences iterable.** A type that yields values implements
`Iterator`; a view whose contents can be walked implements `Iterable`, so
callers write `for x in view`:

```nia
// The numbers from `next` down to one.
struct Countdown {
    next: u32,
}

extend Countdown : Iterator {
    type Item = u32;

    fun next(&mut self) ?u32 {
        if self.next == 0 {
            return null;
        }
        self.next -= 1;
        ?(self.next + 1)
    }
}

// A view of a launch sequence, which counts down from its start.
struct Launch {
    start: u32,
}

extend Launch : Iterable {
    type Item = u32;
    type Iter = Countdown;

    fun iter(&self) Countdown {
        .{ next: self.start }
    }
}

fun launchSum(launch: Launch) u32 {
    let mut sum: u32 = 0;
    for step in launch {
        sum += step;
    }
    sum
}
```

Every iterator then has the adapters of `std::iter` as well.

## Closures and Callables

Write closures without types: they come from where the closure is used.
Captures are explicit, by copy as `[x]` or by pointer as `[&x]` and
`[&mut x]`; [Closures](../learn/closures.md) covers them.

**Borrow a callback the call does not keep; own one that is stored.** A
parameter `&Fun(A) R` borrows the callback for the call, and the caller passes
a closure directly, as `\x -> ...`. A type that keeps a callback, as a lazy
adapter does, takes it by value through a generic bound:

```nia
// Calls `visit` for every value; the callback is borrowed for the call.
fun eachDoubled(values: &[i32], visit: &Fun(i32) ()) () {
    for &value in values {
        visit(value * 2);
    }
}

// Keeps a callback to call later; the callback is owned.
struct Retry[F]
where F: Fun() bool
{
    attempt: F,
    limit: u32,
}

extend[F] Retry[F]
where F: Fun() bool
{
    // Whether one of `limit` attempts succeeded.
    fun run(&self) bool {
        for _ in 0..self.limit {
            if self.attempt() {
                return true;
            }
        }
        false
    }
}
```

**Pass a function or a constructor itself.** A function, a method, and an
enum variant are callables: `mapError(.Syntax)` and `map(square)` need no
closure that only forwards its argument.

## Owners, Views, and Allocators

Nia never allocates or frees behind your back, so ownership is part of how
code reads. [Memory And Allocators](../learn/memory.md) explains it; the
habits that follow from it are:

- **Pass the allocator to every call that allocates, grows, or releases**, and
  release a value with the allocator that produced it.
- **Pair `init` with `deinit`, the second right after the first in a
  `defer`.** Defers run in reverse, so the allocator, created first, is
  released last.
- **Construct from context.** `let mut list: std::ArrayList[u8] = .init();`
  names the type once.
- **Take views, return owners.** A function that only reads takes `&[T]` or
  another view and leaves ownership with the caller; a function that returns
  an owner leaves the caller to release it.
- **Keep a literal that outlives the call in a `static`.** A string literal is
  an array that lives in its function's frame; a `&[char]` that must outlive
  the call points into a `static`.

```nia
using std::io;
using std::mem;
using std::process;

static greeting = "hello";

// The greeting, which outlives every call.
fun greetingText() &[char] {
    greeting
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut page: mem::PageAllocator = .init();
    let mut allocator: mem::GeneralPurposeAllocator = .init(page);
    defer allocator.deinit().?;

    let mut bytes: std::ArrayList[u8] = .init();
    defer bytes.deinit(allocator).?;
    bytes.push(allocator, 1).?;
    io::debugPrint("{} {}\n", [greetingText(), bytes.len()]).?;
    !()
}
```

```text
hello 1
```

## Abstractions Instead of Repetition

Nia gives a few general building blocks rather than a feature per task, and
expects code to be built from them. When two functions differ only in a type,
a step, or a policy, write the shared part once:

- **Write against a trait, not a type.** json writes its output once, to any
  `io::Writer`, and the same code then writes to a buffer, a file, or a
  writer that only counts bytes.
- **Build on the smallest primitive.** A trait with one required method and
  default methods built on it, as `io::Writer` builds `writeAll` on `write`,
  asks each implementation for the least.
- **Configure with a builder.** A value of settings with `with` methods,
  each returning a changed copy, reads at the call site and keeps defaults in
  one place.
- **Turn repeated cases into a table.** Many calls that differ only in their
  data become an array of cases and one loop.

```nia
using std::io;

// The settings of a report; `init` holds the defaults.
struct ReportOptions {
    width: usize,
    header: bool,
}

extend ReportOptions {
    fun init() ReportOptions {
        .{ width: 80, header: true }
    }

    fun withWidth(self, width: usize) ReportOptions {
        let mut options = self;
        options.width = width;
        options
    }
}

// Writes `rows` to any writer: a file, a buffer, or a counter.
fun writeReport[W](
    output: &mut W,
    rows: &[&[u8]],
    options: ReportOptions,
) [W as io::Writer]::Error!()
where W: io::Writer
{
    if options.header {
        output.writeAll(b"report\n").?;
    }
    for &row in rows {
        let shown = if row.len() > options.width { &row[0..options.width] } else { row };
        output.writeAll(shown).?;
        output.writeAll(b"\n").?;
    }
    !()
}

fun narrowReport() ReportOptions {
    ReportOptions::init().withWidth(40)
}
```

## Layout

Nia has no formatter yet; std's layout is the style.

- **Lines stay within 100 columns.**
- **One statement per line.** An `if` whose branches are single expressions,
  as `if c { a } else { b }`, may stay on one line; one that returns or
  assigns does not.
- **Break a long signature or call with one parameter or argument per line**
  and a trailing comma, as `writeReport` above is.
- **Break a long condition before `and` or `or`**, indented, with the opening
  `{` on its own line.
- **Break a long chain before each `.`**, as `squaresAbove` above is. A
  trailing `.?` stays with the call it follows.
- **Put `where` on its own line, and the body's `{` on the next.**
- **Split a long literal into adjacent literals**, which are one literal, at a
  boundary that means something: a line of output, a field of a record.
- **Separate the phases of a long function with a blank line** and give each
  a comment that names it.

```nia
// The text a configuration reader expects, one member per line.
static config = b"{"
    b"\"name\":\"api\","
    b"\"port\":8080"
    b"}";

fun accepts(name: &[u8], port: u16, limit: u16) bool {
    if name.len() != 0
        and port != 0
        and port <= limit
    {
        return true;
    }
    false
}
```

## Comments

- **Start each file with what it is for**, then a blank line, then the
  `using` block.
- **Say what each item is or returns**, in a sentence that starts with the
  thing: "The first even value, doubled", "Writes `rows` to any writer".
- **Explain why, not what.** The code says what it does; a comment says
  what it relies on, what it rules out, or why the obvious way is wrong.
- **Name the phases of a long function**, as a heading for the lines that
  follow.
- **Say what each test pins down**, not how it is written.

## Tests

[Testing](../learn/testing.md) covers suites, assertions, and runners. The
style that keeps suites readable:

- **Give each test a comment that states the property** it checks.
- **Write the cases of one property as a table** and one loop, with a comment
  on each case that is there for a reason.
- **Factor repeated setup into small helpers** that return `test::Error!T`,
  marked `@[trackCaller]` so a failure reports the test's line, not the
  helper's.

```nia
using std::process;
using std::test;

// Whether `text` reads as a yes.
fun isYes(text: &[char]) bool {
    text.len() != 0 and (text[0] == 'y' or text[0] == 'Y')
}

// Every spelling of yes is accepted, and nothing else.
fun acceptsYes() test::Error!() {
    // (answer, whether it is a yes)
    let cases: [(&[char], bool); 4] = [
        ("yes", true),
        ("Y", true),
        // An empty answer is not a yes.
        ("", false),
        ("no", false),
    ];
    for &(answer, expected) in &cases[..] {
        test::expectEqual(isYes(answer), expected).?;
    }
    !()
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let cases = [test::Case::init("accepts yes", acceptsYes)];
    if not test::Runner::init(cases).run().isSuccessful() {
        return process::ExitCode(1)!;
    }
    !()
}
```

## Habits From Other Languages

Nia looks like Rust in places and like C in others, and is neither. These
habits from them either fail to compile or compile to something else; the
[overview](../learn/README.md) lists the larger differences.

| Habit | In Nia |
| --- | --- |
| `Result<T, E>` | `E!T`: the error type comes first. `?T` is an optional. |
| `x?` | `x.?` |
| `*p` | `p.*` |
| `&&`, `\|\|`, `!b` | `and`, `or`, `not b`. `!` builds and matches error unions. |
| `flags & mask == 0` | `(flags & mask) == 0`. `==` binds tighter than `&`, `\|`, and `^`. |
| `&a == &b` to compare values | `a == b`. `==` on pointers compares addresses. |
| `a == "yes"` on a slice | `a.equals("yes")`, with `using std::slice;`. Slices have no `==`. |
| `fn f<T: Ord>` | `fun f[T](...) where T: Ord[T]`. In brackets, `N: usize` declares a const parameter. |
| `compute();` with an unused result | `_ = compute();`. Dropping a value silently is an error. |
| Returning `"text"` as `&[char]` | Return a `static`. A literal is an array in the caller's frame. |
| `let x = 5;` then using `x` as `usize` | `let x: usize = 5;`. A literal takes its type from its own context, `i32` by default. |
| `?a + 1` for "present `a + 1`" | `?(a + 1)`. `?` and `!` bind tighter than binary operators. |
| `for x in array` | `for &x in &array[..]`. Arrays are not iterable; slices yield `&T`. |
| Wrapping overflow, `x << n` dropping bits | Overflow and bits shifted out trap in every profile; use `wrappingAdd`, `truncatingShl`, or `checkedAdd`. |
| `impl`, `fn`, `->` | `extend`, `fun`, and the return type after `)`. |
| `/* comment */` | `// comment`. There are no block comments. |
| `MAX_ITEMS`, `snake_case()` | `maxItems`, `camelCase()`; types and variants are `PascalCase`, files `snake_case`. |
