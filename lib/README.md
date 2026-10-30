# Nia Standard Library

This directory is the toolchain resource tree. `std/` is the standard library
that every program can import as `std`. `runtime/` is a private package that
provides process startup and is linked in only when a program uses Nia startup.
Both are ordinary packages: each `nia.toml` names its root file through
`[library].path` and carries the compiler version.
`imports/` describes the system libraries the library itself calls, so no
platform SDK is needed to link them. `toolchain.meta` records the compiler
version and resource protocol this tree belongs to.

## Modules

Each module below is a public facade declared in `std/root.nia`. Common types
are also re-exported at the root: `std::String`, `std::ArrayList`,
`std::HashMap`, `std::Path`, `std::PathView`, `std::CString`,
`std::CStringView`, `std::Atomic`, `std::SourceLocation`.

| Module | Purpose |
| --- | --- |
| `std::atomic` | `Atomic[T]`, memory orderings, and fences |
| `std::build` | Declarative build-script API: packages, modules, artifacts, steps, and test suites |
| `std::builtin` | Compiler-backed intrinsics (layout, bits, memory, SIMD, atomics, `asm`); see [builtins reference](../docs/reference/builtins.md) |
| `std::cmp` | `Ordering` and `max` |
| `std::collections` | Allocator-explicit `ArrayList`, `HashMap`, `HashSet`, ordered `TreeMap`, and `TreeSet` with borrowed iterators |
| `std::cstring` | NUL-terminated `CString` owners and `CStringView` borrows for C interop |
| `std::error` | Cleanup combinators: `cleanupAfter` and `CleanupAccumulator` |
| `std::fmt` | `Format` trait, `Formatter`, format specs, and checked template printing |
| `std::fs` | `Path`/`PathView`, files, directories, metadata, and directory iteration |
| `std::hash` | `Hash` and `Hasher` traits and the Wyhash hasher |
| `std::io` | `Reader`/`Writer` traits, fixed and buffered adapters, file readers/writers, `debugPrint` |
| `std::iter` | Iterator adapters, ranges, and eager consumers such as `forEach` and `fold` |
| `std::math` | Checked arithmetic traits (`CheckedAdd`, `CheckedMul`, ...) |
| `std::mem` | `Allocator` trait, `Block`/`Layout`, page, fixed-buffer, arena, and general-purpose allocators, typed owners |
| `std::option` | Operations on optional values (`?T`) |
| `std::parse` | `From` and `FromRadix` parsing traits |
| `std::process` | `process::Init`, arguments, environment, commands, child processes, exit codes |
| `std::result` | Operations on error unions (`E!T`) |
| `std::slice` | Borrowed slice operations, iterators, splitting, sorting, binary search, bounds, and partition points |
| `std::source` | `SourceLocation` and `callerLocation` |
| `std::string` | Owned Unicode text (`String`) and text utilities |
| `std::test` | Assertions and an explicit in-process test runner |
| `std::unicode` | UTF-8 decoding and scalar-value validation |

## Conventions

`std::hash::Wyhash` accepts incremental input without depending on write
boundaries. Its cumulative length is mixed modulo 2^64 on every target, and
`finish` does not consume or reset the state. A length wrap never returns the
hasher to its short-input path. This defines the streaming extension beyond
the single-slice length accepted by the reference one-shot algorithm.

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

## Testing

`std::test` provides assertions (`expect`, `expectEqual`, `expectNotEqual`,
`fail`) that return `test::Error`, and a runner. A `Case` pairs a name with a
test function; `Runner` borrows a table of cases, runs each one, and returns a
`RunSummary`. It continues after failures unless built with `withFailFast()`,
and it does not allocate or discover files.

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
        .init(&"adds numbers", &addsNumbers),
        .init(&"compares text", &comparesText),
    ];
    let summary = test::Runner::init(&cases[..]).run();
    if not summary.isSuccessful() {
        return process::ExitCode(1)!;
    }
    !()
}
```

A build script registers each suite with `Build::addTestSuite`. A suite is an
ordinary host executable whose root module may import any number of test
modules, using the same module imports as an application. See
[Build scripts](../docs/guide/build-scripts.md).

## Windows

The library calls Windows directly and does not depend on the C runtime.
Import descriptions are shipped only for the Win32 functions the toolchain's
own sources use. A program that declares another Win32 function links that
function's import library explicitly, like any native dependency
(`-L <dir> -l <library>`). `Signal::Term` and `Signal::Kill` both terminate the
child with `TerminateProcess`; see
[Platform support](../docs/reference/platform-support.md).

`process::Command` finds its program the same way on every platform: a path
that contains a separator names that file, relative to the child's working
directory, and a bare name is searched in the `PATH` of the child's
environment. On Windows a bare name without an extension also tries each
`PATHEXT` extension, so `Command::init(fs::PathView::init(&"git"), env)` finds
`git.exe`.

## API reference

The source files are the API reference. Each facade under `std/` states its
boundary in a header comment, and the owning module documents type-specific
lifetime, error, and cleanup contracts.

## Maintaining the library

The repository audits that guard these sources are described in
[Repository maintenance](../maintain/README.md#standard-library-audits), and
conformance tests in [Testing](../docs/dev/testing.md).
