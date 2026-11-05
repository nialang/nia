# Optionals And Errors

Nia has no exceptions and no null pointers. A value that may be missing has an
*optional* type, and an operation that can fail returns an *error union*: the
possibility of absence or failure is part of the type, and the compiler makes
the caller deal with it. This chapter introduces both, then the patterns the
standard library uses, from propagating a failure to cleaning up after one.
The [language reference](../reference/language.md#propagation) has the exact
rules.

Programs that define `main` are complete; build one with `nia emit --exe` and
run it to see the output shown after it. The later sections use closures and
allocators, which [Closures](closures.md) and [Memory](memory.md) explain
fully; the examples here use them only in simple forms.

## Optional Values

`?T` holds either a `T` or nothing. `?value` builds the present case and `null`
the empty one. `null` needs the optional type from context, such as an
annotation; `?value` takes it from the value:

```nia fragment
let found: ?i32 = ?42;
let missing: ?i32 = null;
_ = found;
_ = missing;
```

An optional cannot be used as a plain value. Take it apart with a pattern:
`?name` matches a present value and binds it, and `null` matches the empty
case.

```nia
fun orZero(value: ?i32) i32 {
    match value {
        ?number => number,
        null => 0,
    }
}

fun doubled(value: ?i32) i32 {
    if value is ?number { number * 2 } else { 0 }
}
```

A pointer that may be absent is `?&T`. This replaces C's null pointer: a plain
`&T` always points somewhere, so code never has to wonder whether to check.

Lookups are the usual source of optionals. A slice's `get` returns `?&T`, a
pointer to the element if the index is in range:

```nia
using std::slice;

fun first(values: &[i32]) ?i32 {
    if values.get(0) is ?element {
        ?element.*
    } else {
        null
    }
}
```

## Error Unions

`E!T` holds either a success value of type `T` or an error of type `E`.
`!value` builds a success and `error!` a failure; read `!` on the left as "ok"
and on the right as "failed with". Patterns use the same shapes: `!value`
matches a success and `error!` a failure. Optionals and error unions nest, as
in `?(E!T)`, and so do their patterns: `?!value`.

### Choosing Between Them

Use an optional when absence is an ordinary answer that needs no explanation,
as in a lookup. Use an error union when the caller needs to know why an
operation failed.

`E` can be any type. Most modules declare one enum with a variant per cause,
and a variant may carry details. An operation with nothing to return on
success returns `E!()` and succeeds with `!()`.

```nia
using std::io;
using std::process;

enum ParseError {
    Empty,
    BadDigit(char),
    TooLarge,
}

fun parseU8(text: &[char]) ParseError!u8 {
    if text.len() == 0 {
        return ParseError::Empty!;
    }
    let mut value: u32 = 0;
    for &ch in text {
        if ch < '0' or ch > '9' {
            return ParseError::BadDigit(ch)!;
        }
        value = value * 10 + (ch as u32 - '0' as u32);
        if value > 255 {
            return ParseError::TooLarge!;
        }
    }
    !(value as u8)
}

fun describe(text: &[char]) i32 {
    match parseU8(text) {
        !value => value as i32,
        ParseError::Empty! => -1,
        ParseError::BadDigit(_)! => -2,
        ParseError::TooLarge! => -3,
    }
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    io::debugPrint("{} {} {} {}\n", [
        describe("42"), describe(""), describe("4x"), describe("300"),
    ]).?;
    !()
}
```

```text
42 -1 -2 -3
```

A `match` on an error union must cover the success case and every error it
can hold. `if result is !value { ... }` and `if result is error! { ... }` test
one case.

## Propagating With `.?`

Most code does not handle a failure where it happens; it passes the failure to
its caller. `.?` does that: on success it yields the value, and on failure it
returns the error from the current function. On an optional it returns `null`
instead.

When the error types differ, `.?` converts the error with an
`std::builtin::IntoError` implementation. An application-level error type
usually wraps the errors of the modules it calls:

```nia
using std::io;
using std::option;
using std::process;
using std::slice;

enum ParseError {
    Empty,
    BadDigit(char),
}

enum ConfigError {
    Parse(ParseError),
    PortOutOfRange(u32),
}

extend ParseError : std::builtin::IntoError[ConfigError] {
    fun intoError(self) ConfigError {
        ConfigError::Parse(self)
    }
}

// Lets `.?` in `main` turn a configuration error into an exit status.
extend ConfigError : std::builtin::IntoError[process::ExitCode] {
    fun intoError(self) process::ExitCode {
        match self {
            .Parse(_) => process::ExitCode(2),
            .PortOutOfRange(_) => process::ExitCode(3),
        }
    }
}

fun parseNumber(text: &[char]) ParseError!u32 {
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

fun parsePort(text: &[char]) ConfigError!u16 {
    let port = parseNumber(text).?;     // a ParseError becomes a ConfigError
    if port > 65535 {
        return ConfigError::PortOutOfRange(port)!;
    }
    !(port as u16)
}

fun firstDigit(text: &[char]) ?u32 {
    let first = text.get(0).?;          // returns null for an empty slice
    if first.* < '0' or first.* > '9' {
        return null;
    }
    ?(first.* as u32 - '0' as u32)
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let port = parsePort("8080").?;
    let failed = if parsePort("80a") is ConfigError::Parse(.BadDigit(ch))! { ch } else { '-' };
    let digit = if firstDigit("7") is ?value { value } else { 0 };
    let none = firstDigit("").isNull();
    io::debugPrint("port={} failed={} digit={} none={}\n", [port, failed, digit, none]).?;
    !()
}
```

```text
port=8080 failed=a digit=7 none=true
```

`main` returns `process::ExitCode!()`, so `.?` in `main` needs the error to
convert to an exit status. The standard library's input, output, file, and
process errors already do, but not every library error: a number-parsing
error, for example, must be handled or converted first. The second
`IntoError` implementation above gives `ConfigError` the ability, choosing a
status for each cause.

`extend ParseError : std::builtin::IntoError[ConfigError]` implements a
*trait*, a set of methods the compiler knows to call; here `.?` calls
`intoError`. [Generics And Traits](generics-and-traits.md) covers traits; for
now, this is the pattern to copy.

## Optional Values As Errors

An operation that returns `?T` often means "failed" in a particular caller.
`okOr` turns an optional into an error union with the error you choose, and
`okOrElse` computes the error only when it is needed. The standard library
uses this for checked arithmetic, whose operations return `null` on overflow:

```nia
using std::io;
using std::mem;
using std::option;
using std::process;
using std::result;

fun totalLength(parts: &[usize]) mem::Error!usize {
    let mut total: usize = 0;
    for &part in parts {
        total = total.checkedAdd(part).okOr(mem::Error::OutOfMemory).?;
    }
    !total
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let small = totalLength([3, 4, 5]).?;
    let overflow = totalLength([usize::MAX, 1]).isError();
    io::debugPrint("small={} overflow={}\n", [small, overflow]).?;
    !()
}
```

```text
small=12 overflow=true
```

## Transforming Results

The helpers on optionals and error unions change one side of a result and
pass the other side through. Several take a closure, an inline function
written `\value -> value * 2`:

| Helper | On | Effect |
| --- | --- | --- |
| `map(f)` | `?T`, `E!T` | applies `f` to the value |
| `andThen(f)` | `?T`, `E!T` | applies `f`, which itself returns a result |
| `mapError(f)` | `E!T` | converts the error with `f` |
| `orElse(f)` | `E!T` | replaces a failure with the result of `f` |
| `inspectError(f)` | `E!T` | calls `f` with the error and returns the result unchanged |
| `isPresent()`, `isNull()` | `?T` | tests the case |
| `isSuccess()`, `isError()` | `E!T` | tests the case |

`mapError` accepts an enum variant constructor directly, which keeps a
wrapping conversion to one word:

```nia
using std::io;
using std::process;
using std::result;

enum ReadError {
    Missing,
}

enum LoadError {
    Read(ReadError),
}

fun readSetting(present: bool) ReadError!i32 {
    if present { !40 } else { ReadError::Missing! }
}

fun loadSetting(present: bool) LoadError!i32 {
    let value = readSetting(present).mapError(LoadError::Read).?;
    !(value + 2)
}

fun doubledSetting() ReadError!i32 {
    readSetting(true).map(\value -> value * 2)
}

fun settingOrZero(present: bool) ReadError!i32 {
    readSetting(present).orElse(\cause -> {
        _ = cause;
        !0
    })
}

fun valueOr(result: ReadError!i32, fallback: i32) i32 {
    if result is !value { value } else { fallback }
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let loaded = if loadSetting(true) is !value { value } else { -1 };
    let doubled = valueOr(doubledSetting(), -1);
    let fallback = valueOr(settingOrZero(false), -1);
    let missing = loadSetting(false).isError();
    io::debugPrint("loaded={} doubled={} fallback={} missing={}\n", [
        loaded, doubled, fallback, missing,
    ]).?;
    !()
}
```

```text
loaded=42 doubled=80 fallback=0 missing=true
```

## Cleaning Up

`defer` runs an expression when its block exits by any path, including `.?`,
so cleanup written right after an acquisition also runs when a later step
fails. Cleanup can fail too: the standard library's `deinit` and `close`
operations return errors, and `defer resource.deinit(allocator).?;`
reports such a failure from the function.

When one owner releases several resources, every release must be attempted
even after one fails. `std::error::CleanupAccumulator` collects the first
failure without stopping, and `cleanupAfter` combines an operation's result
with the result of its cleanup, keeping the operation's failure as the one
reported:

```nia
using std::error;
using std::io;
using std::process;
using std::result;

enum CloseError {
    Busy,
}

struct Handle {
    id: i32,
    busy: bool,
}

extend Handle {
    fun close(&mut self) CloseError!() {
        if self.busy {
            return CloseError::Busy!;
        }
        self.id = 0;
        !()
    }
}

struct Pair {
    left: Handle,
    right: Handle,
}

extend Pair {
    // Attempts both releases; the first failure is reported.
    fun close(&mut self) CloseError!() {
        let mut cleanup: error::CleanupAccumulator[CloseError] = .init();
        cleanup.attempt(self.left.close());
        cleanup.attempt(self.right.close());
        cleanup.finish()
    }
}

fun useAndClose(handle: &mut Handle) CloseError!i32 {
    let id = handle.id;
    let closed = handle.close();
    (!id).cleanupAfter(closed)
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut pair = Pair {
        left: Handle { id: 1, busy: true },
        right: Handle { id: 2, busy: false },
    };
    let failed = pair.close().isError();
    let rightClosed = pair.right.id == 0;

    let mut handle = Handle { id: 7, busy: false };
    let used = if useAndClose(handle) is !id { id } else { -1 };
    io::debugPrint("failed={} rightClosed={} used={}\n", [failed, rightClosed, used]).?;
    !()
}
```

```text
failed=true rightClosed=true used=7
```

The left handle is still open after the failed `close`, so its owner can retry.
Standard-library owners follow the same rule: after a failed `deinit` a value is
cleanup-only, and calling `deinit` again retries what is left.

## Leaving State Unchanged On Failure

An operation that modifies a value in several steps should leave it as it was
when a step fails, so the caller never sees half an update. The simplest way is
to do the fallible work first and change the value only once nothing can fail.
When a value must change as you go, record what to restore and undo it in a
`defer` unless the operation finished:

```nia
using std::io;
using std::mem;
using std::process;
using std::result;

fun appendAll(
    list: &mut std::ArrayList[i32],
    allocator: &mut mem::Allocator,
    values: &[i32],
) mem::Error!() {
    let original = list.len();
    let mut committed = false;
    defer if not committed {
        list.truncate(original);
    };
    for &value in values {
        if value < 0 {
            return mem::Error::Invalid!;
        }
        list.push(allocator, value).?;
    }
    committed = true;
    !()
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut page: mem::PageAllocator = .init();
    let mut allocator: mem::GeneralPurposeAllocator = .init(page);
    defer allocator.deinit().?;

    let mut list: std::ArrayList[i32] = .init();
    defer list.deinit(allocator).?;
    appendAll(list, allocator, [1, 2]).?;
    let rejected = appendAll(list, allocator, [3, -1, 4]).isError();
    io::debugPrint("len={} rejected={}\n", [list.len(), rejected]).?;
    !()
}
```

```text
len=2 rejected=true
```

The failed call pushed `3` before it found `-1`, and the `defer` removed it
again. Rejecting a negative value reuses `mem::Error::Invalid` only to keep the
example short; a real API would declare its own error type.

## Summary

- `?T` is a value that may be absent; `E!T` is a success or an error. Both are
  taken apart with patterns: `?x`, `null`, `!x`, `err!`.
- `.?` unwraps a success or returns the failure from the current function,
  converting the error through `IntoError` when the types differ.
- `okOr`, `map`, `mapError`, and the other helpers in `std::option` and
  `std::result` reshape results; import the module whose helper you call.
- `defer` runs cleanup on every exit path, `CleanupAccumulator` attempts every
  release, and an update that can fail midway should restore what it changed.

Next, [Memory And Allocators](memory.md).
