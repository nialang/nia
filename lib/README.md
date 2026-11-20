# Standard Library

This directory is the toolchain resource tree. `std/` is the standard library
that every program can import as `std`. `runtime/` is a private package that
provides process startup and the support routines of freestanding
executables; it is linked in only when a program uses Nia startup. Both are
ordinary packages: each `nia.toml` names its root file through
`[library].path` and carries the compiler version. `imports/` describes the
system libraries the library itself calls, so no platform SDK is needed to
link them. `toolchain.meta` records the compiler version and resource protocol
this tree belongs to.

## Modules

Each module below is a public facade declared in `std/root.nia`. Common types
are also re-exported at the root, such as `std::String`, `std::ArrayList`,
`std::HashMap`, `std::HashSet`, `std::TreeMap`, `std::TreeSet`, `std::Path`,
`std::PathView`, `std::CString`, `std::CStringView`, `std::Atomic`, and
`std::SourceLocation`.

| Module | Purpose |
| --- | --- |
| `std::atomic` | `Atomic[T]`, memory orderings, and fences |
| `std::build` | Declarative build-script API: packages, modules, artifacts, steps, and test suites |
| `std::builtin` | Compiler-backed intrinsics (layout, bits, memory, SIMD, atomics, `asm`); see [Built-in Operations](../docs/reference/builtins.md) |
| `std::checksum` | `Adler32`, `Crc32`, and `Crc32C` checksums for detecting accidental corruption |
| `std::cmp` | `Ordering`, `min`, and `max` |
| `std::collections` | Allocator-explicit `ArrayList`, `HashMap`, `HashSet`, ordered `TreeMap`, and `TreeSet` with borrowed iterators |
| `std::cpu` | Runtime CPU feature detection, including operating-system vector-state support |
| `std::crypto` | Cryptographic digests: `Sha256` |
| `std::cstring` | NUL-terminated `CString` owners and `CStringView` borrows for C interop |
| `std::error` | Cleanup combinators: the `cleanupAfter` method of error unions and `CleanupAccumulator` |
| `std::fmt` | `Format` trait, `Formatter`, format specs, and checked template printing |
| `std::fs` | `Path`/`PathView`, files, directories, metadata, and directory iteration |
| `std::hash` | `Hash` and `Hasher` traits and the non-cryptographic hashers `Wyhash`, `SipHash13`, `SipHash24`, `XxHash64`, `Fnv1a32`, and `Fnv1a64` |
| `std::io` | `Reader`/`Writer` traits, fixed and buffered adapters, file readers/writers, bounded `readToEnd`, `debugPrint` |
| `std::iter` | `DoubleEndedIterator`, ranges, lazy adapters (`map`, `filter`, `filterMap`, `take`, `skip`, `stepBy`, `takeWhile`, `skipWhile`, `enumerate`, `zip`, `chain`, `copied`, `rev`), and consumers such as `fold`, `find`, `any`, `min`, and `forEach` |
| `std::math` | Checked arithmetic traits (`CheckedAdd`, `CheckedMul`, ...); `BinaryFloat` for IEEE-754 bit access and classification; `isPowerOfTwo` and `alignForward` on `usize` |
| `std::mem` | `Allocator` trait, `Block`/`Layout`, page, fixed-buffer, arena, general-purpose, and pool allocators, typed owners |
| `std::option` | Operations on optional values (`?T`), and equality of optionals |
| `std::os` | Low-level operating-system services, and the native interfaces `os::linux::syscall`, `os::windows::bindings`, and `os::macos::bindings` with their platform calling conventions and error results |
| `std::parse` | `From` and `FromRadix` parsing traits and their `Error`; correctly rounded `f32`/`f64` parsing from text or bytes |
| `std::process` | `process::Init`, arguments, environment, commands, child processes, exit codes |
| `std::result` | Operations on error unions (`E!T`) |
| `std::slice` | Borrowed slice operations, iterators, copying, splitting, sorting, binary search, bounds, and partition points |
| `std::source` | `SourceLocation` and `callerLocation` |
| `std::string` | Owned Unicode text (`String`) and text utilities |
| `std::test` | Assertions, an explicit in-process test runner, and `FailingAllocator` fault injection |
| `std::time` | Monotonic `Instant` readings and `Duration` spans for measuring elapsed time |
| `std::unicode` | UTF-8 decoding and scalar-value validation |

`std::cpu::supports(feature: builtin::target::CpuFeature) bool` reports whether
the running process can use a CPU feature. Build-guaranteed features fold to
`true` in optimized code; other features use a cached runtime query. On x86,
AVX-family features require operating-system register-state support as well as
hardware support. See
[Runtime CPU Queries](../docs/guides/targets.md#runtime-cpu-queries).

## Conventions

- Explicit allocators. Owned values such as `String`, `fs::Path`, and the
  collections take the allocator at every call that allocates, grows, copies,
  or releases. There is no implicit destruction and no hidden allocator state.
  Release a value with the allocator that produced it.
- Owners and views. Borrowed forms are views: `&[T]`, `fs::PathView`,
  `CStringView`. A view never extends the lifetime of its storage.
  Ownership-transfer constructors consume an existing owner; copying
  constructors take an allocator. `Allocator::allocSlice` returns a
  `SliceAllocation[T]` owner rather than a raw slice, and `mem::allocValue`
  returns an `Allocated[T]` owner.
- Fallible cleanup. `deinit` and `close` return errors. Cleanup attempts every
  owned resource and keeps whatever failed to release. After a failed `deinit`,
  the value is cleanup-only: retry `deinit` with the same allocator instead of
  using it again. Attempt-style owners (`Command::spawn` returns a
  `SpawnAttempt`, `Build::init` returns a `BuildInitAttempt`) expose their
  result from `finish` only after cleanup completes; retry `finish` on a cleanup
  error. `close` consumes the handle even when it reports an error, so never
  close twice.
- Leak reporting. `GeneralPurposeAllocator::deinit` reports
  `mem::Error::Leak` when user allocations are still live; the allocator is
  empty at that point and needs no retry. `deinitWithoutLeakCheck` skips the
  diagnostic. A typical owner uses `defer gpa.deinit().?;`.
- Error domains. Each module has its own error type that keeps the underlying
  cause, and owning modules attach context such as the operation and path.
  `IntoError` converts errors for `.?` propagation. `std::error::cleanupAfter`
  combines an operation with its cleanup while keeping the operation failure
  primary.
- Borrowed callbacks. Callback parameters are borrowed for the duration of the
  call unless an API explicitly returns an owner such as
  `mem::CallableAllocation`.

## Formatting

`io::debugPrint` and the `print` method that `using std::fmt::template;` adds
to every `io::Writer` take a template and an array of `&fmt::Format` values.
The template is plain text with placeholders; each placeholder formats the
next value from the array.

| Template text | Output |
| --- | --- |
| `{}` | the next value, in its default form |
| `{spec}` | the next value, formatted by `spec` |
| `{{`, `}}` | a literal `{` or `}` |

A `spec` has up to four parts, each optional, in this order:

| Part | Written | Effect |
| --- | --- | --- |
| flags | `<`, `>`, `^`; a fill character before one of them, such as `_>`; `0`; `+`; `#`, in any order | alignment within the width; the fill character (a space by default); `0` pads with zeros, right-aligned; `+` always shows the sign; `#` adds the `0x`, `0b`, or `0o` prefix |
| width | digits, or `*` | the minimum width; padding uses the fill character |
| precision | `.` and digits, or `.*` | text, `bool`, and `char` are truncated; floats use that many fractional digits and round to nearest, ties to even |
| presentation | `x`, `X`, `b`, `o`, `p` | lower or upper hexadecimal, binary, octal, or a pointer's address |

Without an alignment, padding goes before the value, so `{5}` right-aligns
numbers and text alike. Dynamic width and precision take their numbers from
the array as `usize`, after the value they apply to: `{^*.*}` reads the value,
then the width, then the precision. Writing `*` rather than a nested
placeholder keeps `{{` and `}}` unambiguous: they always mean literal braces.

| Value | Accepts |
| --- | --- |
| integers | flags, width, and every presentation except `p` |
| `f32`, `f64` | sign, alignment, fill, width, precision, and `#`; precision is fixed fractional digits, and `#` forces a decimal point |
| text, `char`, `bool` | alignment, fill, width, and precision |
| a slice of formattable values | `{}` only; prints `[a, b, c]` |
| a pointer `ptr`, passed as `&ptr` | alignment, fill, width, and `p`; prints the address. Passing `ptr` itself formats the value it points to |
| a type of your own | `{}` only, unless the type implements `formatSpec` to accept more |

`Formatter::writeFloat` writes a finite value in its shortest round-trip form.
`writeFloatSpec` takes a `FormatSpec` with the same float options as
templates. These generic methods let custom formatters write float fields
directly. A non-finite value returns `NonFinite` (exit status 33), and an
unsupported specification returns `InvalidTemplate`. Both fail before writing;
a writer failure can leave partial output.

Templates are checked while printing, not by the compiler: formatting is
ordinary library code, and a template is an ordinary character array. A
malformed template or an unsupported part fails with
`fmt::Error::InvalidTemplate`, and too few or too many values with
`MissingArgument` or `ExtraArgument`; all three convert to exit status 22 in
`main`. Output written before the failure stays written. `debugPrint` buffers
512 bytes before it writes, so a failure within its first 512 bytes of output
writes nothing. Implementing `fmt::Format` for a type of your own is shown in
[Printing Your Own Types](../docs/learn/generics-and-traits.md#printing-your-own-types).

## Numeric Parsing

Import `std::parse` to use `"1.5".parse[f64]()` or `b"1.5".parse[f32]()`, as
for integers. All numeric `From` implementations use `parse::Error`. Float
syntax accepts `[-]digits[.digits][e[+-]digits]` and `[-].digits`, where the
exponent marker is `e` or `E`. An empty input returns `Empty`, a leading `+`
returns `InvalidSign`, malformed syntax (including whitespace, non-ASCII
digits, and NaN and infinity spellings) returns `InvalidDigit`, and overflow
returns `Overflow`. Underflow rounds to a subnormal or signed zero. Both widths
round to nearest, ties to even.

`f32` and `f64` have `isNan()`, `isInfinite()`, `isFinite()`, `signBit()`,
and `toBits()` without an import. Bits use the format's own width: `toBits`
returns a `u32` for `f32` and a `u64` for `f64`, and `[f32]::fromBits(bits)`
and `[f64]::fromBits(bits)` restore every bit, including NaN payloads. These
methods belong to `math::BinaryFloat`; name it to write code that is generic
over both widths. Only `f32` and `f64` implement it, because parsing and
formatting rely on a layout description that the standard library keeps to
itself.

## Testing

`std::test` provides assertions that return `test::Error`: `expect`,
`expectEqual`, `expectNotEqual`, `fail`, and `expectNull`, and `expectError`
and `expectSome`, which also return the error or value they check for. Any
other error propagated out of a test with `.?` fails it at that point, as
`test::Error::Propagated` with the location; `mem::Error` is kept as
`test::Error::Memory`, which retains the cause. Errors that carry rollback
resources must be handled before propagation.

A `Case` pairs a name with a test function. `Runner` borrows a table of cases,
runs each one, and returns a `RunSummary`; `runWith` also passes each result
to a reporter callback. The runner continues after failures unless built with
`withFailFast()`, and it does not allocate or discover files.

```nia
using std::process;
using std::test;

fun addsNumbers() test::Error!() {
    test::expectEqual(2 + 2, 4).?;
    !()
}

fun comparesText() test::Error!() {
    test::expect("nia".len() == 3)
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let cases: [test::Case; 2] = [
        .init("adds numbers", addsNumbers),
        .init("compares text", comparesText),
    ];
    let summary = test::Runner::init(cases).run();
    if not summary.isSuccessful() {
        return process::ExitCode(1)!;
    }
    !()
}
```

A package declares each suite as a `[[test]]` in its manifest, or a build
script registers it with `Build::addTestSuite`. A suite is an ordinary host
executable whose root module may import any number of test modules, using the
same module imports as an application. See
[Testing](../docs/learn/testing.md), which also shows `FailingAllocator`
testing an allocation failure.

## Windows

The library calls Windows directly and does not depend on the C runtime.
Import descriptions are shipped only for the Win32 functions the toolchain's
own sources use. A program that declares another Win32 function links that
function's import library explicitly, like any native dependency
(`-L <dir> -l <library>`). Process signals on Windows are described in
[Platform Support](../docs/reference/platform-support.md#current-limits).

`process::Command` finds its program the same way on every platform: a path
that contains a separator names that file, relative to the child's working
directory, and a bare name is searched in the `PATH` of the child's
environment. On Windows a bare name without an extension also tries each
`PATHEXT` extension, so `Command::init(fs::PathView::init("git"), env)` finds
`git.exe`.

## API Reference

The source files are the API reference. Each facade under `std/` states its
boundary in a header comment, and the owning module documents type-specific
lifetime, error, and cleanup contracts.

## Maintaining The Library

The repository audits that guard these sources are described in
[Repository maintenance](../maintain/README.md#repository-audits), and
conformance tests in [Testing](../docs/dev/testing.md).
