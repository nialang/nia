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
  structs, unions, function pointers, and C-style enums;
- simple type generics for functions, structs, and methods, implemented by
  monomorphization;
- methods declared in `extend` blocks;
- traits implemented by `extend Type : Trait` blocks;
- expression-oriented blocks and `if`;
- C-style enums with namespaces and `switch` without fallthrough;
- recursive optional/error-union and value patterns through `switch` and
  if-pattern expressions;
- compile-time value bindings with `const`;
- `defer` for scope cleanup;
- C ABI interop through `extern`;
- explicit file modules declared with `module`, `using`, and `pub using`;
- a small visibility model based on `pub`;
- freestanding executable startup through the standard library, with object and
  LLVM output available for custom build flows.

The current core language keeps these systems outside the language surface:

- garbage collection;
- exceptions;
- a borrow checker;
- algebraic data types;
- implicit allocation;
- a hidden runtime startup model;
- package management as part of the core language.

## 2. Source Files And Programs

Nia source files use the `.nia` extension.

A compilation unit is UTF-8 text. Source locations are tracked with byte offsets
and reported through source spans.

An executable is started by the standard library. The compiler loads the entry
source as the reserved `root` module and injects a standard-library package
startup facade; that facade selects the target startup implementation and calls
the public user entry function through `root::main`. The startup facade is an
implementation detail, not a public `std` API.

The current user entry contract is intentionally single-shaped:

```nia
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
```

Returning `!{}` means process success. `process::ExitCode` is an open enum
backed by `i32`; `process::ExitCode::Success` names status `0`, and the
standard-library constructor for an unnamed status is `process::exit(code)`.
The language also permits an explicit `code as process::ExitCode` cast because
`ExitCode` is an open integer-backed enum, but ordinary executable code uses
the constructor so the conversion remains visible and searchable at one API
boundary. Returning an error payload such as
`process::exit(1)!` asks the startup layer to terminate with that exit status:

```nia
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    process::exit(1)!
}
```

Nia distinguishes two execution models:

- executable emission: the driver injects the standard-library package startup
  facade for the selected runtime. The current default is freestanding startup
  linked without CRT startup; the current target implementation is Linux
  x86_64. The user entry remains the Nia-level
  `root::main(process::Init) process::ExitCode!void` contract.
- bare/object/IR emission: no startup logic is injected and `main` is not
  required. The compiler
  emits LLVM IR or object files for an external build system, custom entry
  symbol, linker script, or freestanding runtime.

Other Nia functions named `main` use normal Nia internal symbol naming unless
they are declared `extern`. The compiler does not export the root user `main`
as the C ABI entry point; that responsibility belongs to the injected startup
facade.
The `std::start` module path is reserved for the injected standard-library
runtime and is not visible to user packages.

The current standard library surface is intentionally small. Standard-library
files are modules, so module-shaped APIs are imported by their file paths, such
as `using std::process;` or `using std::io;`. The root `std` file is a curated
facade for selected direct names; it currently exposes `std::iter`,
`std::ArrayList`, `std::HashMap`, `std::String`, and `std::Atomic`.
Containers are organized under `std::collections` internally, but the root
facade keeps the ordinary user-facing entry points at `std::ArrayList` and
`std::HashMap`.
`ArrayList` uses lower-camel initialized-value APIs. Capacity growth is exposed
through `reserve`, `reserveExact`, and `ensureTotalCapacity`; `truncate` changes
length, while `shrinkToFit` and `shrinkToCapacity` never discard elements.
Public operations do not expose uninitialized element slots or capacity as a
live `&mut [T]`.
`HashMap::init()` creates an empty map directly with a randomized seed;
`initContext(context)` does the same for a custom hash/equality context.
Those direct forms trap if the runtime cannot establish its default randomized
hashing policy.
Programs that can recover from unavailable system randomness use `tryInit()`
or `tryInitContext(context)` and match `HashMapInitError`. `initSeed(seed)` and
`initContextSeed(context, seed)` are the explicit deterministic constructors
for reproducible hashing. The ordinary constructors do not require callers to
name an OS provider error.
`HashMapLookupContext[K, Q]` permits lookup with a query-view type distinct from
the stored key. The default context implements `String` lookup by borrowed
`&[char]`; `containsKeyBy`, `getBy`, `getMutBy`, `getEntryBy`, `getEntryMutBy`,
`getKeyBy`, and `removeEntryBy` use that provider without allocating a temporary
owned string. `insert` returns `mem::Error!?HashMapReplacement`, while
`insertAssumeCapacity` returns the optional directly. When an equal key already
exists, its payload contains the rejected incoming key together with the
replaced stored value. `insertIfAbsent` similarly returns
`mem::Error!?HashMapEntry`, and its assume-capacity form returns the optional
rejected entry directly; that entry contains both incoming values when no
insertion occurs. The former `put`, `fetch_put`, and `put_if_absent` entry
points are absent because they discarded allocating incoming keys or values.
`getOrInsert(allocator, key, value)` returns `HashMapGetOrInsertResult`, which
provides the stored key/value references and returns an equal rejected incoming
entry through `intoRejected()`. `getOrInsertAssumeCapacity` has the same result
without allocation failure. The former no-value `get_or_put` operations are
absent because they exposed an uninitialized stored value.
`drain()` returns a `HashMapDrain` iterator of owned `HashMapEntry` values.
Each `next()` removes exactly the entry it returns; exhausting the iterator
leaves the map empty while retaining reusable capacity.
Reviewed map APIs use lower-camel names without compatibility aliases. `clear`
retains capacity, `deinit` releases allocation, `reserve` accepts additional
capacity, and `ensureTotalCapacity` accepts an absolute capacity floor.

- `std::process` defines the executable entry payload, the open-enum process
  exit value, and `exit` for constructing exit values.
- `std::process` exposes program arguments and environment entries as
  `Arg`/`EnvVar` values. These values provide `len()`, `bytes()`, `isEmpty()`,
  `cstring()`, and `rawPtr()`; they also implement `std::fmt::Format`, so they can
  be printed directly. `Args` provides `program()`, `get(index)`, `iter()`,
  `skipProgram()`, and `rawArgv()`; `ArgsIter` implements `Iterator` with
  `Item = Arg` and also provides `remaining()`. `Env` provides `get(index)`,
  `iter()`, and `rawEnvp()`; `EnvIter` implements `Iterator` with
  `Item = EnvVar` and also provides `remaining()`. `Init.rawArgv()` and
  `Init.rawEnvp()` forward the original host-provided pointer arrays when a
  reviewed low-level process boundary requires them.
- `Command::init(path, env)` borrows a scalar `PathView` and initially inherits
  the startup environment view. `withArguments` borrows scalar argument slices;
  `withEnvironment` borrows an exact replacement slice of scalar
  `EnvEntry::init(name, value)` values, and `withoutEnvironment` selects an
  empty environment. These modes do not merge environment entries.
  `withStdin`, `withStdout`, `withStderr`, and `withCwd` return configured
  command values. `spawn` and `run` encode the path, arguments, and exact
  environment into temporary NUL-terminated UTF-8 storage, insert the
  executable path as argv[0], and reject embedded NUL scalars before invoking
  the OS. `spawnWithAllocator` and `runWithAllocator` provide the same lowering
  with caller-controlled temporary allocation. `spawnRaw` is the explicit
  low-level pointer-array boundary.
- `process::Error` is a closed payload enum. `Allocation(mem::Error)` and
  `Path(fs::PathError)` preserve lowering failures;
  `ArgumentContainsNul(index)` identifies the zero-based element of the
  `withArguments` slice. `Environment { index, cause }` identifies an invalid
  exact environment entry; `EnvEntryError` distinguishes an empty name, `=` or
  NUL in a name, NUL in a value, and a duplicate name carrying the first entry
  index. Validation finishes before transient allocation or spawn.
  `SpawnSetup(process::SystemError)` and `Spawn(process::SpawnError)` preserve
  command configuration and native spawn failures. `SpawnError` separates
  `Setup`, `Stdio`, `Cwd`, and `Exec`, and every stage retains its
  `process::SystemError`.
  `Wait`, `TryWait`, and `Kill` retain their `process::SystemError`, while
  `Close { stream, cause }` retains both a `StdStream` identity and an
  `io::Error`. The executable inserted as argv[0] is not
  counted as an argument index. There are no flat compatibility variants for
  these errors.
- `Child` transfers role-specific `ChildStdin`, `ChildStdout`, and `ChildStderr`
  values with `takeStdin`, `takeStdout`, and `takeStderr`. Child stdin implements
  `io::Writer`; child stdout and stderr implement `io::Reader`. Each role owns
  an invalidatable handle, provides idempotent `close`, reports later access as
  `io::Error::Closed`, and offers `buffered(buffer)` without requiring callers
  to spell a generic adapter type. A taken pipe remains owned by the caller
  across `wait` or `tryWait`; the child closes only untaken pipes. `Child` also
  provides `wait`, `tryWait`, `kill`, and `killWith`. `Term`
  classifies results through `kind`, `code`, `succeeded`, `exitCode`, and
  `signalCode`. Raw OS wait-status conversion is not a public process API.
- `process::Error.asExitCode()` derives the exit value from the retained cause:
  allocation, path, spawn, and OS errors keep their standard numeric category;
  argument and environment validation errors map to invalid input. Build errors
  retain and format the same nested process cause instead of replacing it with
  a generic process failure.
- `std::process` also extends `std::fs.Error` and `std::mem.Error` with
  `asExitCode` and `exit` for explicitly returning standard library errors as
  process exit codes from executable entries. It also extends `std::fs.Error!T`
  and `std::mem.Error!T` with `exit`, which maps the error side to `ExitCode!T`
  so callers can write `io_call().exit().?`.
- `std::cstring` defines `CStringView`, a non-owning view of a NUL-terminated byte
  sequence. It provides `fromPtrUnchecked` for trusted external C string pointers,
  checked `fromBytes` for NUL-terminated byte slices such as `b"name\0"`,
  `rawPtr`, `len`, `bytes`, and `isEmpty`, and implements `std::fmt::Format`
  as raw byte output. `fromBytes` returns `CStringError!CStringView` and
  distinguishes zero-length input (`EmptyInput`), a missing terminator, and an
  interior NUL. The empty C string `b"\0"` is valid.
  `CStringView` does not imply UTF-8 validity and is not required for FFI APIs
  that can operate directly on `&u8`. A `fromPtrUnchecked` caller must ensure
  the pointer remains valid and reaches an accessible NUL byte for every view
  operation.
- The package-private `os` module defines the target-dispatched provider used
  by typed std services.
  Page mapping, path/file operations, raw file handles, random data, spawn/wait,
  signals, and
  process termination are package-private implementation capabilities rather
  than an alternate public API. Process signatures own their `SystemError`,
  `SpawnError`, and `ProcessId` values; `os::SpawnError` and `os::ProcessId`
  are package-private.
  `process::ProcessId` exposes only `raw`; raw `FileHandle` is package-private
  and is not a user adaptation path.
- `std::io` defines `Reader` and `Writer` traits plus fixed-buffer adapters.
  Their reviewed convenience methods are `readExact`, `writeAll`, `writeByte`,
  `endOfStream`, `shortWrite`, and `discardBuffered`; the former snake-case
  spellings are absent.
  `FileReader::stdin(buffer)`, `FileWriter::stdout(buffer)`,
  `FileWriter::stderr(buffer)`, and `File.reader/writer(buffer)` hide platform
  handles and need no runtime backend object. `process::Init` consequently
  carries only argument and environment startup views; it has no `io()`
  capability plumbing.
- `std::debug` defines low-friction diagnostic printing to stderr. Its
  `print` helper traps if the stderr write or flush fails; use `std::io` and
  explicit error propagation for application stdout or recoverable I/O.
- `std::fmt` defines the formatting protocol used by writer `.print(...)`.
  Format arguments are passed as a
  slice of trait-object handles, usually written as an addressed array literal
  such as `&[&value, &count]`; array pointers coerce to the expected slice.
  Checked writer
  printing reports `fmt::Error::MissingArgument`, `ExtraArgument`,
  `InvalidTemplate`, or `Write`.
  Primitive integers, `bool`, `char`, character slices, pointers, generic
  slices `[T]` where `T: fmt::Format`, `std::ArrayList[T]`, and `std::HashMap`
  implement the formatting protocol where applicable.
  Format placeholders are positional. `{}` uses display formatting. Extended
  placeholders begin with `{:...}` and support alignment/fill (`{:>5}`,
  `{:<5}`, `{:^5}`, `{:_>5}`), text precision by character count (`{:.3}`,
  `{:>8.3}`), dynamic width and precision from following `usize` arguments
  (`{:<{}}`, `{:>{}.{}}`), integer signs (`{:+}`), alternate integer prefixes (`{:#x}`,
  `{:#b}`, `{:#o}`), integer presentations (`{:x}`, `{:X}`, `{:b}`, `{:o}`),
  zero padding (`{:05}`, `{:#08x}`), pointer formatting (`{:p}`), and escaped
  braces (`{{` and `}}`). The old shorthand forms such as `{x}` are invalid.
- `std::parse` owns textual value parsing independently of formatting.
  `parse::value[T](input)` and `parse::radix[T](input, radix)` use the
  `parse::From[Input]` protocol. Primitive integers and `bool` parse from
  character text, byte text, C-string views, and process argument/environment
  views without separate byte-specific entry points. Integer parsing accepts
  decimal plus `0x`, `0b`, and `0o` prefixes; radix parsing accepts bare digits
  in radix 2 through 36. Failures are `parse::Error::Empty`, `InvalidDigit`,
  `InvalidSign`, `Overflow`, `InvalidRadix`, or `InvalidValue`. Custom
  `parse::From` implementations select their own associated error type;
  radix parsing is a separate `parse::FromRadix` capability.
- `std::mem` defines the `Allocator` trait plus `Layout` and `Block`, the
  explicit allocation contract used by standard containers. A block must be
  freed with the allocator that produced it, and with the current layout carried
  by that block. `resize` and `remap` may change the size but preserve the
  allocation's alignment and either keep the same pointer with an updated size
  or fail without moving the allocation; `realloc` may allocate a new block,
  copy the shared prefix, and free the old block. `allocBytes`, `allocSlice`,
  and `freeSlice` are the convenience allocation forms. `Layout.isEmpty` and
  `Block.isEmpty` test zero-sized storage; `Block.asSlice[T]` exposes a typed
  view whose element count is derived from the block size.
- `ArrayList`, `HashMap`, and `String` do not store an allocator. Every
  operation that may allocate, remap, or release their current backing storage
  takes one explicitly, immediately after the receiver, and it must be the same
  allocator that produced that non-empty storage. Read-only and capacity-
  preserving operations take no allocator. An owned copy or clone establishes
  new provenance from its target allocator and may therefore use a different
  allocator from the source.
- `std::mem.PageAllocator` maps each allocation through the OS page layer. It is
  useful as a low-level backing allocator, not as the default container
  allocator for many small objects.
- `std::mem.FixedBufferAllocator` allocates from caller-provided storage and can
  be reset as a whole. It is useful for examples, stack-backed scratch work, and
  bounded programs where out-of-memory is part of normal control flow.
- `std::mem.ArenaAllocator` provides region-style allocation over a child
  allocator. `reset` invalidates every block, slice, and container backing
  allocation obtained from that arena while retaining reusable capacity;
  `deinit` invalidates those allocations and releases the retained capacity.
  Only copied scalar values should be kept across either call.
- `std::mem.GeneralPurposeAllocator` is the ordinary heap allocator currently
  provided by the standard library. It uses small-allocation slabs plus larger
  child-backed allocations, performs basic invalid-free and double-free checks,
  and is single-threaded/external-synchronization by contract. `deinit` frees
  allocator-owned backing memory and returns `DeinitStatus::Leak` if any
  allocations were still live at shutdown. `DeinitStatus.ok()` maps a clean
  shutdown to `void` and a leak to `std::mem.Error::Invalid`; the common
  `deinit().ok().?` form checks both deallocation errors and leak status with
  one propagation point. Use `deinitWithoutLeakCheck` only when that cleanup
  is intentionally unchecked. Wrap a GPA later in synchronization primitives
  rather than sharing one instance concurrently.
- `std::atomic` defines `Atomic[T]`, ordering constants, and ordering-specific
  load/store/read-modify-write/compare-exchange/fence helpers. It is a thin
  standard-library facade over the compiler atomic builtins.
- `std::slice` defines `SliceIter` and `SliceIterMut`. Borrowed slices are
  iterable directly with `for`; both `&[T]` and `&mut [T]` yield `&T` through
  that shared protocol. Use `.iterMut()` explicitly when the loop needs `&mut
  T` items. Slices whose elements implement `Eq[T]` also provide sequence
  equality, prefix/suffix, contiguous search, and borrowed `SliceSplit`
  iteration; `Ord[T]` enables lexicographic comparison.
- `std::cmp` defines the closed `Ordering` result used by lexicographic slice
  comparison: `Less`, `Equal`, and `Greater`.
- `std::iter` defines iterator support types. Native range values such as
  `0usize..len`, `1i64..4i64`, `2usize..=4usize`, and `5usize..` are iterable
  directly; the backing iterator types live under `std::iter` as `Range`,
  `RangeInclusive`, and `RangeFrom`. Their `Step` trait is implemented for the
  built-in integer types that have representable `MAX` values.

The package-private std OS provider is Nia-defined, not libc. Platform-specific
syscall backends are package-internal implementation details. A future
`std::c` can model optional libc linkage without becoming the default
executable runtime.

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
module
mut
never
not
null
or
pub
return
struct
switch
trait
true
type
using
let mut
void
where
```

Primitive type names such as `i32`, `u8`, `usize`, `bool`, `char`, `void`, and
`never` are reserved type names. A standalone `!` is reserved for error-union
syntax, not logical negation or the never type.

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
let mut x: u8 = 1;
let mut n: usize = 10;
let mut chained = 10i32;
```

The suffix selects the literal's type before contextual inference. The literal
value must fit in that type. Underscores may separate digits and do not affect
the value. Radix prefixes and underscores may be combined with suffixes:

```nia
let mut mask = 0xffu8;
let mut bits = 0b1010_0000u8;
let mut mode = 0o755usize;
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
let mut x: f32 = 1.5;
let mut y = 1.5f32;
let mut z = 1.0e-3f64;
```

The suffix selects the literal's type before contextual inference. The value
must be finite and representable in that type. Integer suffixes are invalid on
floating literals.

For built-in binary arithmetic, bitwise, comparison, and equality operations
whose operands share a numeric type, a concrete operand supplies type context
for an unsuffixed numeric literal on either side. An explicit suffix is never
overridden by the peer operand. Shift counts are checked independently and do
not determine the type of the value being shifted.

Scalar integer division and remainder trap when the divisor is zero. Signed
scalar division and remainder also trap for `MIN / -1` and `MIN % -1`, since
the mathematical quotient is not representable in the operand type. During
constant evaluation these traps are source diagnostics; runtime lowering checks
the same conditions before emitting the LLVM integer operation. Floating-point
division and remainder follow their floating-point semantics and are not
covered by this scalar integer trap rule.

During constant evaluation, integer addition, subtraction, multiplication, and
negation are checked at every operation against the concrete operand type. The
same scalar runtime operations trap on overflow before an LLVM operation can
produce poison. An overflowing const intermediate is a source diagnostic even
when a later operation would return the final value to range. The concrete type
comes from ordinary semantic inference, including expected types, instantiated
generic parameters, the defining module of an imported `const fn`, and the
target width of `usize` and `isize`; literal spelling and the compiler host
integer width do not define
these rules. A constant integer shift count must be non-negative and smaller
than the concrete left operand width. The same count rule applies to scalar
runtime shifts: a negative signed count or a count greater than or equal to the
concrete left operand width traps before the shift executes. Left shift
additionally requires the mathematical result to remain representable in the
left operand type, diagnosing during constant evaluation and trapping at
runtime. Right shift is arithmetic for signed left operands and logical for
unsigned left operands. The count's integer type does not change either the
result type or the right-shift mode. Integer-vector overflow and shift behavior
remains a separate lane-wise design boundary.

An expected optional or error-union type supplies context to its constructed
payload. For example, `return ?0` in a function returning `?usize`, `!1` where
`E!u8` is expected, and `2!` where `u8!i32` is expected infer `usize`, `u8`, and
`u8` respectively. The wrapper does not force an otherwise contextual numeric
literal to default to `i32`.

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

String literals are fixed-length Unicode scalar arrays. `"..."` has type
`[N]char`.

Byte string literals:

```nia
b"nia"
b"nia\0"
```

Byte string literals are fixed-length byte arrays. `b"..."` has type `[N]u8`.
NUL-terminated byte sequences are written explicitly, for example `b"nia\0"`.

Adjacent quoted string literals with the same prefix are concatenated into one
literal:

```nia
"hello, " "world"
b"ni" b"a\0"
```

This is source-level literal concatenation, not runtime string or array
concatenation. Mixed literal families are invalid:

```nia
"hello" b"world" // invalid
```

String-family array values do not implicitly decay to slices. Take an explicit
address when a slice is expected; the resulting pointer-to-array uses the
ordinary pointer-array-to-slice coercion:

```nia
let text: &[char] = &"nia";
let bytes: &[u8] = &b"nia";
```

The same rule applies to named arrays:

```nia
let stable = b"nia";
let stable_bytes: &[u8] = &stable;
```

Multiline string literals use consecutive lines beginning with `\\`. Byte
multiline string literals use `b\\` on the first line; continuation lines still
use `\\`:

```nia
\\mov rax, 60
\\syscall

b\\mov rax, 60
\\syscall
```

For multiline strings, indentation before the delimiter is ignored, the delimiter
itself is not part of the string, and the text after the delimiter is copied as
is. Adjacent lines are joined with `\n`; no extra newline is appended after the
last line. Escape sequences are not interpreted inside multiline string lines.
The prefix selects the same type family as the quoted form: `[N]char` or
`[N]u8`.

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
never
```

`void` means the expression or function produces no meaningful value. If a
function declaration omits its return type, the return type is `void`.

`never` marks expressions that never produce a normal value, such as `return`,
`break`, `continue`, and calls to functions returning `never`. Never expressions
may be used where ordinary values are expected because control flow does not
continue to the use site. `never` may be used as a function return type or
function pointer return type. It is not a valid variable, field, parameter, or
array element type.

### 4.2 Pointers

Ptr types:

```nia
&T
&mut T
^T
^mut T
```

`&T` is a read-only object pointer. `&mut T` is a writable object pointer.
`^T` is a read-only volatile object pointer. `^mut T` is a writable volatile
object pointer. Whitespace is insignificant: `& T` parses as `&T`, and `^ T`
parses as `^T`, not as different pointer kinds.

Pointers are ordinary values. Nia has no borrow checker. Read-only and writable
pointers are different types. Ptr conversions must be explicit:

```nia
let mut addr = ptr as usize;
let mut ptr2 = addr as &u8;
```

Address-of and dereference syntax:

```nia
let mut value = 1;
let mut p = &value;
let mut mp = &mut value;
let mut x = p.*;
mp.* = 1;
```

Dereferencing `^T` or `^mut T` has the same source shape as ordinary pointer
dereference, but the memory access is volatile. A read through a volatile
pointer emits a volatile load. A write through `^mut T` emits a volatile store.
`^T` cannot be written through.

```nia
fn read_reg(reg: ^u32) u32 {
    reg.*
}

fn write_reg(reg: ^mut u32, value: u32) void {
    reg.* = value;
}
```

`&place` takes a read-only pointer to a place. `&mut place` takes a writable
pointer to a writable place. Identifiers, field access, array indexing, slice
indexing, and pointer dereference may be places. Field access and indexing into
an aggregate value inherit place-ness from their left-hand side. Indexing a
pointer or slice value is instead an indirect place: the pointer or slice
expression need not itself be a place. Its pointee mutability determines whether
the indexed place is writable.

When the operand is a typed value expression rather than a place, address-of
materializes a block-scoped temporary object and returns a pointer to that
temporary. The temporary has the expression's runtime value type. `void` and
`never` expressions cannot be materialized.

When the pointee type is a trait name, `&Trait[...]` and `&mut Trait[...]`
denote trait object pointers, not thin object pointers. A trait object is a Nia
fat pointer carrying an object pointer plus implementation metadata. Bare
`Trait[...]` remains a trait type for bounds and projections; it is not a valid
value, field, parameter, or array element type.

```nia
trait Source {
    type Item;
}

fn consume(source: &Source[Item = i32]) void {}
```

Trait object syntax uses the same bracket list as trait bounds: positional trait
arguments come first, followed by associated type bindings. Binding names must
be associated types declared by the trait, and a single object type may not bind
the same associated type more than once.

Materialized rvalue references are ordinary block-scoped temporaries. Bind a
value to local or global storage when a stable address with a chosen lifetime is
needed:

```nia
fn make_i32() i32 {
    42
}

let mut point: Point = { x: 10, y: 20 };
let mut p: &Point = &point;
let mut temp = &Point { x: 1, y: 2 };
let mut answer = &42i32;
let mut returned = &make_i32();

let hello = b"hello\0";
_ = &hello[0];
```

### 4.3 Arrays

Fixed-length array type:

```nia
[N]T
```

Array length may be written explicitly in type syntax, or inferred with `_` when
an array literal provides the element count:

```nia
let mut xs: [_]i32 = [1, 2, 3];
let mut name: [_]u8 = b"nia";
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
let mut xs = [1, 2, 3]; // [3]i32
```

In other expression contexts, the expected type must still supply the array
element type and length.

An array literal may also carry an explicit array type prefix:

```nia
[3]i32[1, 2, 3]
[_]i32[1, 2, 3]
[2][2]i32[
    [1, 2],
    [3, 4],
]
```

The prefix supplies the expected array type for the literal. `[_]T[...]` infers
the length from the literal elements. This form is useful when the literal stands
alone, when nested array shape should be explicit, or when the literal is
immediately materialized, for example:

```nia
let mut s = & ([3]i32[1, 2, 3])[..];
```

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
let mut matrix: [2][3]i32 = [
    [1, 2, 3],
    [4, 5, 6],
];

let mut zeros: [2][3]i32 = [[0; 3]; 2];
```

Array indexing:

```nia
let mut x = arr[0];
arr[1] = 42;
```

Array length is part of the type. Ordinary array values do not decay
implicitly; slice views come from array pointers or from the string-family
literal rules described below.

### 4.4 Slices

Slices are length-carrying references. Slice types are written only through
reference forms:

```nia
&[T]
&mut [T]
```

`&[T]` is a read-only contiguous range. `&mut [T]` is a writable contiguous
range. The language does not expose slice fields; an implementation may represent
a slice as `{ ptr, len }`, where `ptr` points to the first element and `len` has
type `usize`.

Slices may be constructed by combining range indexing with address-of:

```nia
let mut hello = "hello";
let mut s = &hello[..];       // &[char]
let mut t = &hello[0..2];     // &[char]
let mut u = &hello[0..=1];    // &[char]
let mut v = &hello[1..];      // &[char]
let mut w = &hello[..3];      // &[char]
let mut x = &hello[..=3];     // &[char]
```

Writable slices use `&mut` and require a writable base place:

```nia
let mut xs: [4]i32 = [1, 2, 3, 4];
let mut s = &mut xs[1..3]; // &mut [i32]
s[0] = 10;
```

Bare range indexing is not a value expression:

```nia
xs[..]; // error; use &xs[..] or &mut xs[..]
```

An array pointer may be implicitly converted to a full-range slice when the
expected type is exactly `&[T]` or `&mut [T]`. `&[N]T` converts to `&[T]`;
`&mut [N]T` converts to `&mut [T]`, and may also be used where read-only
`&[T]` is expected.

```nia
fn read(xs: &[i32]) i32 {
    xs[0]
}

fn write(xs: &mut [i32]) {
    xs[0] = 10;
}

let mut arr: [3]i32 = [1, 2, 3];
let mut ro: &[i32] = &arr;
let mut rw: &mut [i32] = &mut arr;
read(&arr);
write(&mut arr);
read(&[1, 2, 3]);
write(&mut [1, 2, 3]);
```

Array literals can still be used by taking their address. The usual rvalue
materialization rule used by address-of creates a block-scoped temporary array.
Ordinary array values do not convert directly:

```nia
let mut arr: [3]i32 = [1, 2, 3];
read(arr);      // error
read([1, 2, 3]); // error
```

String and byte string literal expressions have type `&[N]char` and `&[N]u8`.
When a slice is expected, the ordinary pointer-array-to-slice coercion can
produce `&[char]` or `&[u8]`. Method resolution applies the same coercion when
`&[N]T` or `&mut [N]T` has no matching method but the corresponding slice does.
Methods defined for the fixed-length array remain more direct and take
priority. The selected method's receiver kind controls the final coercion, so a
read-only array pointer cannot call a mutable slice method.

Range forms:

```nia
start..end
start..=end
start..
..end
..=end
..
```

Range forms also exist as structural types:

```nia
usize..usize
usize..=usize
usize..
..usize
..=usize
..
```

There is no nominal built-in `Range` type. A range type is identified by its
shape and, for bounded forms, by one integer bound type. `T..U` and `T..=U`
require `T` and `U` to be the same integer type.

Range expressions are runtime values with these structural range types. Built-in
slice construction requires every explicit range bound to have type `usize`;
unsuffixed integer literals in slice bounds are inferred as `usize`. Omitted
slice bounds are interpreted by the slice operation, not by rewriting the range
value to `0` or `usize::MAX`.

Bounded range values expose their present bounds through the builtin `Start`
and `End` traits. `a..b` and `a..=b` implement both `Start` and `End`;
`a..` implements only `Start`; `..b` and `..=b` implement only `End`; `..`
implements neither. Range values do not implement `Len`.

Nia does not provide built-in runtime bounds checks. The programmer is
responsible for ensuring that the selected memory range is valid.

With `std::slice` loaded, slices provide checked library operations for
ordinary data-dependent access. `get(index)` and `getMut(index)` return an
optional element reference. `first`/`firstMut` and `last`/`lastMut` do the same
for the endpoints. `getRange(start, end)` and `getRangeMut(start, end)` use a
half-open range and return an optional slice; they return `null` when
`start > end` or `end > len`, while `len, len` is a valid empty range. These
methods validate before using the native indexing or slicing operation.

Direct `slice[index]` and `&slice[start..end]` remain the explicit unchecked
language primitives. The standard library does not duplicate them under an
`unchecked` method name. A single read-only checked branch can use
`if slice.get(index) is ?value`; mutable optional references use an explicitly
mutable pattern such as `switch slice.getMut(index) { mut ?value => ... }`.

When `T: Eq[T]`, `equals`, `startsWith`, `endsWith`, `find`, and `contains`
operate on complete contiguous element sequences. `find` returns the first
matching element index as `?usize`. An empty needle matches at zero, including
against an empty slice; a needle longer than the receiver does not match.
Mutable slice values can call these read-only receiver methods directly: method
resolution applies the same `&mut [T]` to `&[T]` coercion accepted at ordinary
typed boundaries.

`split(separator)` returns an allocation-free `std::slice::SliceSplit[T]` whose
iterator items are borrowed `&[T]` segments. Matching is left-to-right and
non-overlapping. Leading, trailing, and adjacent separators therefore produce
empty segments; an empty receiver produces one empty segment. An empty
separator performs no split and yields the complete receiver once, because
ordinary element iteration already has the direct `for &item in slice`
spelling. The iterator borrows both slices, so the receiver and separator must
remain valid and unchanged until iteration ends. `SliceSplit` is intentionally
not double-ended: reverse matching of self-overlapping multi-element separators
requires a separate boundary/search model rather than hidden rescanning.

Scalar text provides `replaceAll(allocator, needle, replacement)` on both
borrowed `[char]` and owned `String` receivers. It returns a new owned `String`
and never mutates or consumes the source. Matches use the same left-to-right,
non-overlapping boundaries as `split`; an empty needle performs no replacement
and returns an independent copy. The implementation computes the exact output
length before allocation, allocates once, and returns `mem::Error::OutOfMemory`
without a partial result. `replacement` may borrow from the source because the
source remains unchanged while the independent result is built.

A borrowed text sequence `&[&[char]]` provides
`join(allocator, separator)`. The result is an independent `String`; input
parts and the separator remain borrowed and unchanged. Empty input produces an
empty string, one part produces an independent copy without a separator, and
an empty separator concatenates the parts. Join scans the repeatable input
slice once to compute the exact scalar length and then once to fill a single
allocation. Length overflow and allocation failure both report
`mem::Error::OutOfMemory` without a partial result. A literal collection uses
one contextual annotation:

```nia
let parts: [3]&[char] = [&"build", &"λ", owned.text()];
let mut joined = (&parts).join(allocator, &"/").?;
```

`copyFrom(source)` is the ordinary slice copy operation. It copies
`min(receiver.len(), source.len())` initialized element representations,
handles overlapping ranges, and returns that element count. The return value
makes a short destination observable and cannot be ignored implicitly.
`copyFrom` is a shallow value copy; it does not call a user cloning or cleanup
protocol. For `T: Ord[T]`, `compare(other)` compares elements
lexicographically and returns `std::cmp::Ordering`. If their common prefix is
equal, the shorter slice compares less.

The base of a slice construction may be an array, another slice, or a
single-element pointer:

```nia
let mut arr: [4]i32 = [1, 2, 3, 4];
let mut a = &arr[..];      // len = 4

let mut b = &a[1..3];      // slice from slice

let mut x: i32 = 10;
let mut p = &x;
let mut c = &p[..];        // len = 1
let mut d = &p[0..1];      // len = 1
let mut e = &p[0..12];     // allowed; programmer owns the length claim
```

For `&T` and `&mut T`, an omitted upper bound uses a base length of 1. An
explicit upper bound uses the explicit range length.

`slice[index]` accesses an element. Indexing `&[T]` produces an addressable but
non-writable place. Indexing `&mut [T]` produces a writable place even when the
slice is returned by a call or another value expression; only the referenced
element is assigned, not the slice value.

### 4.5 Optional And Error Union Types

Optional types are written `?T`. `null` constructs the empty value and `?value`
constructs the present value:

```nia
let mut a: ?i32 = ?10i32;
let mut b: ?i32 = null;
```

`null` and `?value` require an expected optional type when the full optional
type cannot otherwise be inferred. When an expected `?T` is present, it is also
the expected type of `value`.

Error union types are written `E!T`, where `E` is the error value type and `T`
is the success value type. `!value` constructs the success case and `error!`
constructs the error case:

```nia
let mut ok: i32!i32 = !10i32;
let mut err: i32!i32 = 2i32!;
```

Both error-union constructors require an expected `E!T` type. A binding such as
`let mut x = !10i32;` is invalid because the error type `E` cannot be inferred from
the success value alone.

The postfix propagation operator `.?` unwraps an optional or error union inside a
function. For `?T`, `value.?` returns `T` on the present path and returns `null`
from the current function on the empty path. For `E!T`, `value.?` returns `T` on
the success path and propagates the error value from the current function on the
error path. Optional propagation requires an optional function return type. Error
propagation requires an error-union function return type with the same error
type.

```nia
fn read(value: i32!i32) i32!i32 {
    let mut x = value.?;
    !x
}
```

Patterns can destructure optional and error-union values:

```nia
if maybe is ?x {
    x
} else {
    0
}

switch result {
    !x => x,
    err! => err,
}

switch nested {
    ?!value => value,
    ?err! => err,
    null => 0,
}
```

`?pattern` matches the present optional case and matches `pattern` against the
payload. `null` matches the empty optional case. `!pattern` matches the
error-union success case. `pattern!` matches the error-union error case.
Patterns may be nested, so `?!value` matches an optional present value whose
payload is an error-union success value, while `?err!` matches the nested error
case. Ptr binding forms such as `let &value = ptr;` and `for &value in
items` are binding destructuring forms too, but they are parsed on local/loop
bindings through the irrefutable subset of the same pattern model. `switch`
accepts the refutable forms shown above as well as pointer patterns.

### 4.6 Structs

Struct declaration:

```nia
struct String {
    ptr: &u8,
    len: usize,
}
```

Struct value construction:

```nia
let mut s: String = { ptr: & bytes[0], len: 3 };
```

The common struct literal form does not carry a nominal type by itself. The
expected type must provide the struct type. Anonymous struct inference such as
`let mut p = { x: 10, y: 20 };` is not supported.

```nia
fn sum(point: Point) i32 {
    point.x + point.y
}

let mut p: Point = { x: 10, y: 20 };
let mut total = sum({ x: 1, y: 2 });
```

A struct literal may also carry an explicit nominal type prefix:

```nia
let mut p = Point { x: 10, y: 20 };
let mut q = Point{x: 1, y: 2};
let mut ptr = &Point { x: 3, y: 4 }; // &(Point { ... }) as read-only
```

In a `const` value context, an untyped struct literal may also be used as a
structural compile-time-only value. This does not create an anonymous runtime
struct type:

```nia
const config = { width: 4usize };
const width: usize = config.width;
```

Field access:

```nia
let mut len = s.len;
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

### 4.6 Unions

Union declaration:

```nia
union Bits {
    i: i32,
    f: f32,
}
```

Union value construction uses the same aggregate literal syntax as structs, but
exactly one field must be initialized:

```nia
let mut bits: Bits = { i: 42 };
```

Union field access is explicit:

```nia
let mut n = bits.i;
bits.f = 1.0;
```

Reading a union field other than the field most recently written is a low-level
reinterpretation operation. The programmer is responsible for ensuring that the
active field and access type are meaningful for the target ABI and program
invariants.

Generic unions are allowed:

```nia
union Slot[T] {
    value: T,
    empty: u8,
}
```

Ordinary `union` uses Nia layout rules. `extern union` uses C ABI-visible union
layout for C interop:

```nia
extern union CValue {
    i: i32,
    f: f32,
}
```

Union size is the maximum field size rounded up to the maximum field alignment.
Every field has offset zero. See `docs/nia-abi.md` for ABI details and C interop
restrictions.

### 4.7 Function Types

Function declaration:

```nia
fn add(a: i32, b: i32) i32 {
    a + b
}
```

If the return type is omitted, it is `void`:

```nia
static mut log_total: i32 = 0;

fn log(value: i32) {
    log_total += value;
}
```

Use `static` rather than `const` for data that must have a stable
address, such as data passed across an explicit foreign ABI boundary.

Function pointer type:

```nia
&fn(i32, i32) i32
```

A function declaration name is a function item, not an ordinary runtime value.
Function items cannot be used bare:

```nia
fn add(a: i32, b: i32) i32 {
    a + b
}

let mut f = add; // error
```

Addressing a function item with `&` creates a function pointer:

```nia
let mut f = & add;                  // &fn(i32, i32) i32
let mut g: &fn(i32, i32) i32 = & add;   // allowed
```

Generic functions must be explicitly instantiated before taking a function
pointer:

```nia
fn id[T](x: T) T {
    x
}

let mut f = & id[i32]; // &fn(i32) i32
let mut g = & id;      // error
```

`& function_item` is a specific function-item address rule. It does not
require the function item to be a place. `&function_item` is not allowed.

## 5. Declarations

### 5.1 Attributes

Attributes are AST marks written before an item, statement, or aggregate field:

```nia
@[link_name("runtime_start")]
extern fn start(argc: i32) i32;

struct Header {
    @[offset(0)]
    magic: u32,
}
```

Attribute syntax is `@[name]` or `@[path.name(args...)]`. Attribute names use
identifier path segments separated by `.`. Attribute arguments use normal
expression syntax and are stored with the AST node.

`@[if condition]` is the language-defined conditional compilation attribute.
It may be attached to items and statements. The condition language is separate
from ordinary Nia expressions and from `const` evaluation. It accepts boolean,
integer, and string literals; names `arch`, `vendor`, `os`, `env`, `abi`,
`endian`, and `pointer_width`; unary `not`; binary `and`, `or`, `==`, and `!=`;
and parentheses.

```nia
@[if os == "linux" and arch == "x86_64"]
pub(pkg) module freestanding;

fn word() usize {
    @[if pointer_width == 64]
    return 8;
    4
}
```

The whole file must still parse, so inactive declarations and statements must
be syntactically valid Nia. After parsing, inactive conditional items and
statements are removed for the active target before later semantic phases.
Invalid names, types, imports, or calls in inactive code are not diagnosed for
that target. Multi-target validation is expected to run the compiler for each
target a project supports.

Attributes are intentionally separate from builtin expressions. `@foo` remains
reserved for builtin expression forms such as `@size[T]()` and `@error(...)`.
AST attributes must use the bracketed `@[...]` form.

The compiler also injects a reserved `builtin` module root. It is a normal
module for name resolution and imports, but its source is generated from the
active target rather than read from disk:

```nia
using builtin;

const word_bits: usize = builtin::pointer_width;
const target_os = builtin::os;
```

The initial `builtin` surface exposes `arch`, `vendor`, `os`, `env`, `abi`,
`endian`, and `pointer_width` as public const values. These values share the
same target facts used by `@[if ...]`.

The parser accepts attributes on top-level items and on `struct`/`union` fields.
An unknown attribute is reserved and has no language-defined effect until this
specification assigns one. Unknown attributes do not change visibility, ABI,
layout, symbol names, type checking, or code generation.

ABI selection is still expressed by declarations. In particular, Nia does not
use `@[repr(C)]`: `extern struct` and `extern union` are the C ABI aggregate
forms. Nia also does not currently define `@[export]`; C ABI symbol definitions
are written as `extern fn` definitions.

### 5.2 Functions

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

### 5.3 Extern Declarations

`extern` uses an external ABI and external symbol name. Item modifier order is
visibility first and `extern` second:

```nia
pub extern fn foreign_log(message: &u8);
```

`extern pub fn` is not valid syntax.

An `extern fn` without a body declares an external C ABI symbol:

```nia
pub extern fn foreign_log(message: &u8);
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
extern static errno: i32;
extern static mut global_counter: usize;
```

Extern functions default to return type `void` when no return type is written.
Variadic functions are only allowed as body-less `extern fn` declarations.

Nia does not provide `extern { ... }` blocks or explicit ABI strings. All
`extern` functions, globals, and structs use the C ABI.

### 5.4 Type Aliases

```nia
type Byte = u8;
type CString = &u8;
```

Type aliases do not create new nominal types.

### 5.5 Enums

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
let mut c = Color::Black;
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
let mut flag: Flag = 3 as Flag;
```

`switch` is the canonical multi-arm matching expression. It matches scalar and
enum values and may recursively destructure optional and error-union values:

```nia
let mut value = switch c {
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

An arm may list multiple patterns separated by commas. Integer switches also
support closed range patterns with both endpoints present:

```nia
switch value {
    0, 1 => return 0;
    2..5 => return 1;   // 2, 3, 4
    5..=7 => return 2;  // 5, 6, 7
    _ => return 3;
}
```

Open-ended switch range patterns are not supported; use `_` for the fallback
case. Range pattern endpoints must be compile-time integer constants. Empty
ranges and overlapping integer patterns are rejected.

Optional and error-union patterns use the same recursive matcher:

```nia
switch value {
    ?x => x,
    null => 0,
}

switch result {
    !x => x,
    err! => err,
}

switch nested {
    ?!value => value,
    ?err! => err,
    null => 0,
}
```

`?pattern` matches the payload of a present optional. `!pattern` matches the
success payload of an error union. `pattern!` matches the error payload of an
error union. `null` matches the empty optional case. `_` and a bare binding are
catch-all patterns. These patterns may nest across optional and error-union
layers, and pointer patterns such as `&value` use ordinary Nia pointer-copy
semantics.

A bare identifier in a pattern always introduces a binding. Named constant and
enum value patterns must be syntactically explicit: use a qualified path such as
`Color::Red`, or parenthesize an expression such as `(local_constant)`. This
rule does not depend on capitalization or name-resolution results. An arm may
list multiple alternatives only when none of them binds a value, because every
entry edge to an arm body must define the same locals.

Switch expression arms must produce compatible value types unless an arm exits
through `return`, `break`, or `continue`. Switches over closed enums must cover
all variants or provide `_`. Switches over open enums must provide `_`, even if
every named variant is covered.

### 5.6 Let And Const Bindings

`let` is an immutable binding, not a general compile-time execution mechanism
and not macro substitution.

```nia
let name = "nia";
let mask: u32 = 0xff;
```

Use `const` for named compile-time values:

```nia
const size: usize = 16;
```

Local `let` bindings cannot be assigned after declaration:

```nia
let x = 1;
x = 2; // error
```

`let x: T;` is a valid local declaration. Like `let mut x: T;`, it creates
uninitialized automatic storage of type `T`; the difference is that the binding
cannot be assigned and cannot form writable `&T`:

```nia
let p: Point;

p.inspect();     // allowed if the receiver is Point or &Point
_ = &p;          // allowed
p.init();        // error if init requires &mut Point
p = { };         // error
_ = &mut p;      // error
```

Nia does not perform definite initialization analysis. Reading uninitialized
storage is the programmer's responsibility. `let` only controls binding-level
assignment and whether a writable pointer may be taken from the binding; it does
not provide deep immutability and does not prove that the value was initialized.

`static` declarations require explicit types when they have no initializer.
Non-extern uninitialized `static` declarations create static storage initialized
to zero. Extern `static` declarations without initializers only declare external
symbols.

Top-level `static` creates immutable global static storage. Implementations
should place it in read-only data where possible:

```nia
static hello: [7]u8 = b"hello\n\0";
```

Top-level `static` initializers must be expressible as static initialization data.
They do not execute arbitrary compile-time programs:

```nia
static a = 1 + 2;           // allowed: integer static expression
static hello: [3]u8 = b"hi\0"; // allowed: byte-array static data
static p = &hello[0];       // allowed: global static address
static bad = { 1 + 2 };     // error: block execution is not static data
```

Contexts requiring compile-time values, such as non-literal array lengths, read
`const` bindings rather than `static` storage.

`const` creates a compile-time value binding with no runtime storage and no
address:

```nia
const width: usize = 4;

fn first_value() i32 {
    const local_width: usize = width;
    let mut xs: [local_width]i32 = [1, 2, 3, 4];
    xs[0]
}
```

`const` may appear at module, associated-value, and local binding positions. A
`const` binding must have an initializer. Its initializer must be evaluable
with the current compile-time value evaluator. Current compile-time values cover
integer, boolean, string, array, and struct literal values; struct field access;
casts that preserve the underlying value; boolean `not`, `and`, and `or`;
equality comparisons between matching primitive const value kinds; simple
integer arithmetic and bit operations; and references to other visible
`const` bindings. Cyclic `const` dependencies are errors.

Top-level `pub const` bindings participate in normal module visibility and
may be used through imports:

```nia
// config.nia
pub const width: usize = 4;

// main.nia
using root::config;
let mut xs: [config::width]i32 = [1, 2, 3, 4];
```

Taking the address of a `const` binding is invalid because it has no runtime
storage.

Struct const values are ordinary field-keyed const values:

```nia
struct Point {
    x: usize,
    y: usize,
}

const p: Point = Point{x: 2, y: 3};
const width: usize = p.x + p.y;
```

Conditional source selection is expressed with `@[if ...]`, not with
`const`. `const` is reserved for compile-time values and functions.

`const fn` declares a function that is valid during constant evaluation. It is
not a const-eval-only function kind: the same function may be called from
runtime code, where it is lowered and executed as an ordinary runtime
function. A constant expression may call `const fn`, but may not call an
ordinary `fn`. This gives one implementation a dual-stage contract rather than
separate compile-time and runtime definitions.

The const-capability contract is checked at the declaration, independently of
whether the function is used. Tail expressions, explicit returns, expression
statements, and all source branches must use const-capable operations and agree
with the declared types. Branch selection controls evaluation, not semantic
validity: an ordinary `fn` call in an unselected branch is still invalid, while
a const-capable operation such as `std::builtin::error` may remain in a branch
that is not selected for a particular call.

`std::builtin::trap()` is a dual-stage const-capable operation returning
`never`. Reaching it during constant evaluation produces a source diagnostic at
the call; reaching the same operation at runtime terminates through the target
trap primitive. Merely declaring it in a branch does not execute it.
`std::builtin::error(message)` remains distinct: it is a const-only operation
whose evaluated const string becomes the diagnostic message.

Constant evaluation may use ordinary `let mut` locals for loops, accumulation,
and aggregate construction. Each call receives fresh local state. That state
cannot modify a module or associated `const`, has no observable address or
cross-query identity, and cannot escape into the returned const value:

```nia
const fn width() usize {
    let mut value: usize = 0;
    while value < 4 {
        value += 1;
    }
    value
}

const arrayWidth: usize = width();
```

Calls are staged by their use site:

```nia
const fn double(value: usize) usize {
    value * 2
}

const arrayLen: usize = double(5); // evaluated at compile time

fn runtimeDouble(value: usize) usize {
    double(value) // emitted as a runtime call
}
```

Function pointer types describe an ordinary runtime call signature; they do not
currently carry a const-callable capability. Runtime code may take the address
of a `const fn` and call it indirectly, and that reference makes the target a
runtime-reachable function. Constant evaluation may call the same definition
directly, but may neither form a function pointer value nor call through one.
Supporting const indirect calls would require an explicit const-callable
function type or an equivalent statically checked capability, not inference
from a pointer value's local origin.

Constant evaluation is resource bounded. One outer evaluation currently has a
1,000,000-step budget and a maximum const-function call depth of 256; an
individual `while` or `loop` is additionally limited to 100,000 iterations.
Nested calls and loops consume the same outer step budget. Exceeding a limit is
a source diagnostic at the active expression or call site, not a runtime stack
overflow or an indefinitely running compiler. These limits constrain compiler
execution and do not make a non-terminating const expression valid.

### 5.7 Static Storage

Top-level `static` declarations create global static storage. `static mut`
declares writable global storage.

```nia
static mut a = 1;

fn bump() i32 {
    a = a + 1;
    0
}
```

Static declarations may infer their type from an initializer or write it
explicitly:

```nia
static mut hello = "hello\n";
static mut counter: i32 = 0;
static banner = b"nia";
```

Non-extern initialized `static` declarations must satisfy static initialization
rules. A bare global value does not automatically become an address:

```nia
static mut target: i32 = 1;
static mut p: &i32 = &target; // allowed
static mut q: &i32 = target;  // error
```

Top-level static declarations are visible inside the same module. Cross-module visibility
is controlled by the module system.

## 6. Local Bindings And Assignment

`let mut` introduces mutable bindings. `let` introduces immutable bindings with
storage. `const` introduces an immutable compile-time value binding with no
runtime storage or address. `const` is itself the declaration keyword, so
`const let` is invalid. There is no `const mut`; mutation during constant
evaluation uses transient `let mut` locals inside a `const fn` or const
initializer block.

Inferred type declaration:

```nia
let mut x = 1;
let mut name = "nia";
```

Explicit type declaration:

```nia
let mut x: i32 = 1;
let mut name: [4]u8 = b"nia\0";
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

`let mut` introduces a mutable binding. `let` introduces an immutable binding.
`let mut x: T;` and `let x: T;` are valid
uninitialized declarations. A declaration without an explicit type must have an
initializer.

Local bindings may use pointer destructuring:

```nia
let &x = ptr;
let mut &mut y: i32 = mut_ptr;
```

`let &x = ptr` requires `ptr: &T` and binds `x: T`. `let mut &mut y: T = ptr`
requires `ptr: &mut T` and binds `y: T`. A type annotation names the bound value
type after destructuring, not the pointer input type. Ptr-destructuring
local bindings require an initializer. This syntax is separate from the
refutable optional/error-union patterns accepted by `switch` and if-pattern
expressions. Local and loop bindings accept only the irrefutable pattern subset.

## 7. Statements And Semicolons

Nia uses semicolons for statement boundaries.

Statements requiring semicolons:

```nia
let mut x = 1;
x = x + 1;
record(x);
return x;
break;
continue;
```

Block-shaped control flow used as a standalone statement does not need a trailing
semicolon. The recommended rule is:

- ordinary expression statements need `;`;
- `if`, if-pattern expressions, `for`, and `switch` used as standalone statements
  do not need `;`;
- a block tail expression does not use `;`.

## 8. Expressions

### 8.1 Blocks

Blocks are expressions:

```nia
{
    let mut x = 1;
    x + 1
}
```

A block with a tail expression has the tail expression type. A block without a
tail expression has type `void`.

### 8.2 If

`if` is an expression:

```nia
let mut result = if score >= 60 {
    "pass"
} else {
    "fail"
};
```

When an `if` expression is used as a value, it must have both branches and the
branches must have compatible types. When `if` is used only for control flow,
`else` may be omitted and the expression type is `void`.

An if-pattern expression performs one refutable match with the
binding/destructuring pattern language:

```nia
if result is !value {
    use(value);
} else {
    recover();
}
```

The matched expression is evaluated once. Bindings are scoped to the successful
branch, and `mut` belongs inside the pattern, for example
`if maybe is mut ?value { ... }`. A non-exhaustive value-producing if-pattern
requires `else`; an effect-only if-pattern may omit it.

Use `switch` for multiple refutable alternatives:

```nia
switch result {
    !value => use(value),
    err! => {
        return map_error(err)!;
    },
}
```

### 8.3 Loops

Nia has three loop forms: `for-in`, `while`, and `loop`.

Iterator loop:

```nia
for item in iter {
    consume(item);
}
```

`for pattern in expr` requires `expr` to implement the builtin `Iterable`
trait. `Iterable::Iter` must implement the builtin `Iterator` trait, and its
`Iterator::Item` must equal `Iterable::Item`. The loop calls the builtin
`Iterable::iter` provider and then repeatedly calls the builtin
`Iterator::next` provider. It does not perform ordinary method lookup for
methods with those names and does not bind to any standard-library module path.
Types expose collection-specific iteration by implementing `Iterable`; an
`Iterator` also satisfies `Iterable` intrinsically with itself as `Iter`.

The builtin iteration protocol is:

```nia
trait Iterable {
    type Item;
    type Iter;
    fn iter(&self) Iter;
}

trait Iterator {
    type Item;
    fn next(&mut self) ?Item;
}
```

`Item` may be any ordinary item type, including `&T`, `&mut T`, `?T`, or
`E!T`. Fallible iteration is not a separate `for` protocol: an iterator can
choose `Item = E!T`, and the loop body decides whether to handle or propagate
each item.

The loop pattern may be a value binding, a pointer binding, a mutable pointer
binding, or a discard:

```nia
for x in values {}
for &x in pointer_values {}
for &mut x in mutable_pointer_values {}
for _ in values {}
```

`&x` and `&mut x` are pointer patterns. They require the iterator item type to
be `&T` or `&mut T` respectively, and bind `x` as the pointed-to `T` value.
Use `for x in iter` to bind the iterator item itself without destructuring it.

For-in bindings do not support type annotations. Write the iterator expression
so that its item type is clear:

```nia
using std;

let mut total: usize = 0;
for i in 0usize..len {
    total += i;
}

let mut wide_total: i64 = 0;
for i in 1i64..4i64 {
    wide_total += i;
}
```

Condition loop:

```nia
while a > b {
    a -= 1;
}
```

Infinite loop:

```nia
loop {
    // work
}
```

`break` exits the nearest loop. `continue` starts the next iteration.

Loops are statements and do not produce values.

### 8.4 Defer

`defer` registers an expression to run when the current block exits:

```nia
{
    let mut file = open(path);
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

The deferred expression is evaluated as an ordinary delayed statement. It must
have type `void` or `never`. If cleanup returns a non-`void` value, discard it
explicitly:

```nia
defer {
    _ = flush(file);
    close(file);
};
```

Control flow inside a deferred expression runs when the deferred expression runs.
`return` may return from the current function, `.?` may propagate from the
current function, and `break` or `continue` may target an enclosing loop when the
`defer` statement is registered inside that loop context. If a deferred
expression changes control flow, that new control flow overrides the exit path
that caused the defer to run.

Nia has no exceptions, so `defer` has no exception-unwind semantics.

### 8.5 Operators

Unary operators:

```text
-       numeric negation
not     boolean not
~       bitwise not
&       writable address
&mut    writable address
&       read-only address
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

Arithmetic, bitwise, shift, comparison, and selected unary operators are type
checked through core language operator traits. These traits are always available
by name and are not provided by a source module:

```nia
Add[Rhs]
Sub[Rhs]
Mul[Rhs]
Div[Rhs]
Rem[Rhs]
BitAnd[Rhs]
BitOr[Rhs]
BitXor[Rhs]
Shl[Rhs]
Shr[Rhs]
Neg
Not
BitNot
Eq[Rhs]
Ord[Rhs]
```

Arithmetic, bitwise, shift, `Neg`, and `BitNot` have an `Output` associated
type. For example, `a + b` requires the bound `Lhs: Add[Rhs]` and has type
`[Lhs as Add[Rhs]]::Output`. `Not`, `Eq`, and `Ord` return `bool`.
Generic code that uses an operator must state the needed capability explicitly:

```nia
fn add_same[T](a: T, b: T) T
where T: Add[T, Output = T] {
    a + b
}

fn same[T](a: T, b: T) bool
where T: Eq[T] {
    a == b
}
```

Primitive numeric, integer, boolean, pointer, and enum implementations are
compiler-known only for the operations they support. Lowering represents these
operators as builtin operator calls; backend lowering either keeps a primitive
builtin operator call or dispatches to the visible trait implementation method.
LLVM code generation emits the corresponding primitive operation after generic
instantiation.

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
has type `&Point` or `&mut Point`, then:

```nia
p.x
p.move(1, 2)
```

may dereference `p` for field access or receiver matching. This automatic
dereference is limited to fields, methods, and receiver matching. It is not a
general implicit conversion.

Expression statements may not silently discard non-`void` and non-`never` values.
Discard explicitly with `_`. Discarding a `void` expression is also valid:

```nia
vec.push(2);      // error if push returns non-void
_ = vec.push(2);  // allowed
_ = log("done");  // allowed even if log returns void
abort();          // allowed if abort returns never
```

### 8.7 Builtins

Nia provides a small builtin surface:

```nia
@size[T]()
@align[T]()
@offset[T]("field")
@error("message")
@embed("path")
value.len()
range.start()
range.end()
slice.ptr()
slice.ptr_mut()
@load_unaligned[T](ptr)
@splat[Vec](value)
@extract(vector, index)
@insert(vector, index, value)
@bitmask(mask)
@ctz[T](value)
@clz[T](value)
@popcount[T](value)
@atomic_load[T](ptr, order)
@atomic_store[T](ptr, value, order)
@atomic_rmw[T](ptr, op, value, order)
@cmpxchg_strong[T](ptr, expected, desired, success, failure)
@cmpxchg_weak[T](ptr, expected, desired, success, failure)
@fence(order)
@asm({...})
```

`@size[T]()` returns the ABI size of `T` in bytes as `usize`.

`@align[T]()` returns the ABI alignment of `T` in bytes as `usize`.

`@offset[T]("field")` returns the ABI byte offset of a struct or union field
as `usize`. The field name must be a string literal. For unions, every field
has offset `0`.

`@embed("path")` reads a file during compile-time evaluation and returns its
contents as a byte array value. The path argument must be a string literal and
is resolved relative to the source file that contains the `@embed` call, not the
process working directory. `@embed` is only valid in a `const` expression
context; it does not parse or macro-expand the embedded bytes.

`@size[T]()` and `@align[T]()` require `T: Sized`. For concrete layout-known
types this predicate is compiler-proven. In generic code it must be written in
the `where` clause:

```nia
fn bytes[T]() usize
where T: Sized {
    @size[T]() + @align[T]()
}
```

When their type argument is concrete, `@size[T]()`, `@align[T]()`, and
`@offset[T]("field")` are compile-time known values and may appear in ordinary
expressions. `@size[T]()` and `@align[T]()` may also appear in array lengths and
static initializers. In generic code these builtins remain layout values until
the generic function is instantiated.

`value.len()` calls the built-in `Len` trait method. Arrays and slices have
compiler-proven `Len` implementations; for `[N]T`, it returns `N`; for `&[T]`
and `&mut [T]`, it returns the runtime slice length. User types may implement
`Len` when they do not overlap compiler-proven array or slice implementations.

`range.start()` and `range.end()` call the built-in `Start` and `End` trait
methods. They are available only for range shapes that carry the requested
bound and return that bound's integer type.

`slice.ptr()` and `slice.ptr_mut()` call the built-in `Ptr`
and `PtrMut` trait methods. `&[T]` and `&mut [T]` have compiler-proven
`Ptr` implementations. Mutable slices also have compiler-proven
`PtrMut` implementations, whose `ptr_mut()` method returns `&mut T`. Arrays
intentionally do not implement `Ptr` or `PtrMut`; form a slice first
with `&array[..]`. A pointer to an array may coerce to a slice at the receiver
of these built-in place methods, so `b"name\0".ptr()` is valid and
explicitly produces a pointer to the first byte. User types may implement these
traits for custom contiguous storage abstractions, but may not overlap
compiler-proven slice implementations.

`@load_unaligned[T](ptr)` reads a `T` from a byte pointer with alignment 1.
`ptr` must have type `&u8` or `&mut u8`, and `T` must be `Sized`. The caller is
responsible for ensuring that at least `@size[T]()` readable bytes are available
at `ptr`; the builtin only relaxes alignment, not bounds or initialization.

`std::builtin::memcpy[T](destination, source)` and
`std::builtin::memmove[T](destination, source)` copy the common slice prefix as
raw element representation and return `void`. Both require initialized,
readable source elements and writable destination elements. `memcpy` copies
forward and requires the copied ranges not to overlap in a way that changes a
later source element; `memmove` selects a safe direction and permits overlap.
`std::builtin::memset(destination, byte)` fills every byte of a mutable `u8`
slice. These are compiler primitives for std and low-level code. Ordinary code
uses `slice.copyFrom`, whose count result exposes short copies and whose
implementation selects the overlap-safe primitive.

SIMD vector builtins operate on primitive vector types such as `u8x16` and
`boolx16`. `@splat[Vec](value)` constructs a vector whose lanes all contain
`value`; `Vec` must be a SIMD vector type and `value` must have its lane type.
`@extract(vector, index)` reads one lane, and `@insert(vector, index, value)`
returns a copy of `vector` with one lane replaced. Lane indexes are integer
values. Out-of-range indexes have backend-defined behavior.

Vector comparisons return boolean mask vectors such as `boolx16`. `@bitmask`
packs a boolean mask vector into `usize`, with lane 0 in the least significant
bit. It currently supports masks up to 64 lanes.

Bit-counting builtins operate on integer primitive types. `@ctz[T](value)`
returns the number of trailing zero bits, `@clz[T](value)` returns the number
of leading zero bits, and `@popcount[T](value)` returns the number of set bits.
The argument and result both have type `T`. `@ctz[T](0)` and `@clz[T](0)` are
defined to return the bit width of `T`.

Atomic builtins provide the low-level primitive operations behind `std::atomic`.
Their `order` and `op` arguments must be compile-time integer constants. The
standard library exposes named constants for these values:

```text
ordering: Unordered=0, Monotonic=1, Acquire=2, Release=3, AcqRel=4, SeqCst=5
rmw op:   Xchg=0, Add=1, Sub=2, And=3, Nand=4, Or=5, Xor=6,
          Max=7, Min=8, UMax=9, UMin=10
```

`@atomic_load[T]` takes `&T` or `&mut T` and returns `T`.
`@atomic_store[T]` takes `&mut T` and returns `void`.
`@atomic_rmw[T]` takes `&mut T`, applies an atomic read-modify-write operation,
and returns the previous value. `@cmpxchg_strong[T]` and
`@cmpxchg_weak[T]` return `null` on success or `?old_value` on failure.
`@fence(order)` emits an atomic fence and returns `void`.

The current supported atomic value types are bool, integer, enum, and ordinary
object pointer types whose width does not exceed the target pointer width.
Volatile pointer types are not atomic value types. Floating point, slices,
structs, arrays, and unions are not atomic value types. Legal ordering sets
follow the operation kind: loads allow Unordered, Monotonic, Acquire, and SeqCst;
stores allow Unordered, Monotonic, Release, and SeqCst; read-modify-write and
cmpxchg success orderings allow Monotonic, Acquire, Release, AcqRel, and SeqCst;
cmpxchg failure orderings allow Monotonic, Acquire, and SeqCst and must not be
stronger than the success ordering; fences allow Acquire, Release, AcqRel, and
SeqCst.

`@asm({...})` is the inline assembly escape hatch for syscalls, special
registers, port I/O, CPU instructions, and freestanding runtime glue. Its
argument must be a struct-literal configuration. It returns `void`. The fields
are compiler-consumed metadata, not a runtime struct:

```nia
fn syscall1(sys_num: usize, arg1: usize) isize {
    let mut ret: isize = 0;
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
let mut p: Pair[i32, bool] = { first: 1, second: true };
```

Expression generic instantiation also uses `[]` for explicit type arguments in
function-item, method, or other expression positions:

```nia
let mut x = id[i32](1);
let mut f = & id[i32];
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
fn eq[T](a: &T, b: &T) bool
where T: Eq[T] {
    a == b
}

struct Box[T] where T: Eq[T] {
    value: T,
}
```

Generics are implemented by monomorphization. Type parameters have no runtime
representation. The current generic surface is explicit type parameters on
functions, structs, unions, traits, and methods. User-declared let generics
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
    fn len2(&self) i32 {
        self.x * self.x + self.y * self.y
    }
}
```

Method call:

```nia
let mut v: Vec2 = { x: 3, y: 4 };
let mut n = v.len2();
```

Methods are ordinary functions associated with a concrete nominal type. Receiver
forms:

```nia
fn method(&self, ...)
fn method(&mut self, ...)
fn method(self, ...)
fn method(...)
```

Receiver meaning:

- `&self` passes the receiver as read-only `&Type`;
- `&mut self` passes the receiver as writable `&mut Type`;
- `self` passes the receiver by value;
- no `self` means the function is an associated function called as
  `Type::method(...)`.

The target of `extend` may be any visible extendable value type, including an
imported type:

```nia
using root::math;

extend math::Point {
    fn len2(&self) i32 {
        4
    }
}
```

Concrete structural targets use the same type grammar, including nested slice
pointees such as `extend [&[char]] { ... }`.

`extend` itself is not marked `pub`. Method visibility is written on the method:

```nia
extend Point {
    pub fn len2(&self) i32 {
        self.x * self.x + self.y * self.y
    }
}
```

Public extension methods from loaded visible modules participate in method
lookup. `module` declarations load child modules and `using` opens already
visible module namespaces or package roots. Private extension methods are
visible only in their defining module. If multiple visible
extension methods provide the same method name for the same receiver type, the
call is ambiguous.

Receiver methods may also be called through their associated type path by
passing the receiver explicitly:

```nia
let mut n = Vec2::len2(& v);
```

The same associated path can be used as an unbound function item when forming a
function pointer. This does not capture a receiver; the receiver remains the
first explicit function parameter:

```nia
let mut f: &fn(&Vec2) i32 = &Vec2::len2;
let mut n = f(&v);
```

Generic structs may have methods:

```nia
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn get(&self) T {
        self.value
    }
}
```

Methods may have their own type parameters. Explicit generic method calls use
`[]`:

```nia
extend[T] Box[T] {
    fn replace[U](&self, value: U) U {
        value
    }
}

let mut box: Box[i32] = { value: 1 };
let mut x = box.replace[bool](true);
```

Associated paths use the same brackets for generic type and method arguments:

```nia
let mut make: &fn(i32) Box[i32] = & Box[i32]::make;
let mut replace: &fn(&Box[i32], bool) bool =
    &Box[i32]::replace[bool];
let mut y = Box[i32]::replace[bool](&box, true);
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

let mut p: &u8 = [&u8]::zero() as &u8;
let mut n = [&u8]::null(p);
let mut f: &fn(&u8) bool = &[&u8]::null;
```

`[type]::name` is only an associated target path. It does not introduce a
short name for the method, and it does not capture a receiver.

## 11. Traits

Traits define required method signatures and associated type outputs for static
dispatch:

```nia
trait Show {
    fn show(&self) i32;
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
    fn eq(&self, other: &Point) bool {
        self.x == other.x
    }

    fn ne(&self, other: &Point) bool {
        self.x != other.x
    }
}
```

Some trait names are builtin capability traits. They are still implemented with
ordinary trait implementation syntax, but their names and required members are
defined by the language. Operator expressions are checked through these traits:

| Operator | Required trait | Result |
| --- | --- | --- |
| `a + b` | `Add[Rhs]` | `Output` |
| `a - b` | `Sub[Rhs]` | `Output` |
| `a * b` | `Mul[Rhs]` | `Output` |
| `a / b` | `Div[Rhs]` | `Output` |
| `a % b` | `Rem[Rhs]` | `Output` |
| `-a` | `Neg` | `Output` |
| `not a` | `Not` | `bool` |
| `~a` | `BitNot` | `Output` |
| `a & b` | `BitAnd[Rhs]` | `Output` |
| `a | b` | `BitOr[Rhs]` | `Output` |
| `a ^ b` | `BitXor[Rhs]` | `Output` |
| `a << b` | `Shl[Rhs]` | `Output` |
| `a >> b` | `Shr[Rhs]` | `Output` |
| `a == b`, `a != b` | `Eq[Rhs]` | `bool` |
| `a < b`, `a <= b`, `a > b`, `a >= b` | `Ord[Rhs]` | `bool` |

`not` is boolean logical not. `~` is bitwise not. Primitive implementations are
compiler-proven for the primitive types that support the operation. Non-primitive
operator support comes from visible `extend Type : Trait[...]` implementations.

Builtin operator traits have fixed method names. Binary arithmetic and bitwise
traits use `add`, `sub`, `mul`, `div`, `rem`, `bit_and`, `bit_or`, `bit_xor`,
`shl`, and `shr`. Unary traits use `neg`, `not`, and `bit_not`. `Eq` requires
both `eq` and `ne`; `Ord` requires `lt`, `le`, `gt`, and `ge`.

`Sized`, `Deref`, `DerefMut`, `Index`, `IndexMut`, `Slice`, `SliceMut`,
`Ptr`, `PtrMut`, and `Char` are also builtin capability traits. Their names and
required members are fixed by the language:

```nia
trait Sized {}

trait Deref {
    type Target;
    fn deref(&self) &[Self as Deref]::Target;
}

trait DerefMut : Deref {
    type Target;
    fn deref_mut(&mut self) &mut [Self as DerefMut]::Target;
}

trait Index[I] {
    type Output;
    fn index(&self, index: I) &[Self as Index[I]]::Output;
}

trait IndexMut[I] : Index[I] {
    type Output;
    fn index_mut(&mut self, index: I) &mut [Self as IndexMut[I]]::Output;
}

trait Slice[R] {
    type Output;
    fn slice(&self, range: R) [Self as Slice[R]]::Output;
}

trait SliceMut[R] : Slice[R] {
    type Output;
    fn slice_mut(&mut self, range: R) [Self as SliceMut[R]]::Output;
}

trait Len {
    fn len(&self) usize;
}

trait Start {
    type Output;
    fn start(&self) [Self as Start]::Output;
}

trait End {
    type Output;
    fn end(&self) [Self as End]::Output;
}

trait Ptr {
    type Target;
    fn ptr(&self) &[Self as Ptr]::Target;
}

trait PtrMut : Ptr {
    type Target;
    fn ptr_mut(&mut self) &mut [Self as PtrMut]::Target;
}

trait Char {
    fn char(self) ?char;
}
```

The compiler proves builtin implementations for primitive operations,
layout-known types, pointers, arrays, and slices where the operation is native
to the language. User implementations of builtin traits are allowed when they
do not overlap a compiler-proven implementation. For example, a custom
container may implement `Slice[..]`, but `[N]T` may not provide a manual
`Slice[..]` implementation because array slicing is already
compiler-proven. Custom range-like types may implement `Start` and `End`,
while compiler-proven structural range implementations cannot be overlapped.

Index expressions lower through `Index` or `IndexMut`; slice expressions
lower through `Slice` or `SliceMut`. Native array, pointer, and slice
implementations require integer indices or range types whose bounds are
`usize`.

A trait implementation block is only for implementing the named trait. It must
not contain methods that are not members of that trait. Inherent methods still
use ordinary `extend Type { ... }` blocks.

Trait method declarations may include bodies. A body is a default method body.
If an implementation omits a method that has a default body, calls to that
method instantiate the default body with `Self` set to the implementing type:

```nia
trait Comparable {
    fn same(&self, other: &Self) bool;

    fn different(&self, other: &Self) bool {
        not self.same(other)
    }
}
```

Traits may declare supertraits after `:`. Multiple supertraits are joined with
`+`:

```nia
trait Ordered : Comparable {
    fn lt(&self, other: &Self) bool;
}

trait IndexedSource[I] : Source + Index[I] {
    fn valid_index(&self, index: I) bool;
}
```

If `Child : Parent`, then a `where T: Child` bound also makes `Parent` methods
available on `T`, and default methods in `Child` may call `Parent` methods.
Implementing `Child` does not implicitly implement `Parent`; the parent trait
implementation must be written explicitly:

```nia
extend Point : Eq {
    fn eq(&self, other: &Point) bool {
        self.x == other.x
    }

    fn ne(&self, other: &Point) bool {
        self.x != other.x
    }
}

extend Point : Ord {
    fn lt(&self, other: &Point) bool {
        self.x < other.x
    }

    fn le(&self, other: &Point) bool {
        self.x <= other.x
    }

    fn gt(&self, other: &Point) bool {
        self.x > other.x
    }

    fn ge(&self, other: &Point) bool {
        self.x >= other.x
    }
}
```

Traits may declare associated types. An associated type is a named type output
selected by the implementing `Self` type and the trait's explicit generic
arguments:

```nia
trait Source {
    type Item;

    fn get(&self) [Self as Source]::Item;
}

trait Mapper[A, B] {
    type C;
    type D;

    fn map(&self, a: A, b: B) [Self as Mapper[A, B]]::C;
}
```

Every trait implementation must define every associated type required by the
trait, and it may not define associated types that the trait does not declare:

```nia
struct Counter {
    value: i32,
}

extend Counter : Source {
    type Item = i32;

    fn get(&self) i32 {
        self.value
    }
}
```

Associated type definitions are only valid in trait implementation blocks.
Ordinary inherent `extend Type { ... }` blocks cannot contain `type` members.

Associated type projections are written explicitly:

```nia
[T as Source]::Item
[Self as Mapper[A, B]]::C
```

Nia does not support shorthand projection syntax such as `T::Item` or
`Self::Item`. Path syntax with `::` remains ordinary type or associated function
path syntax; it does not infer an associated type projection. Associated types
also do not take their own generic parameters in this version. Generic
associated type families are reserved for future design.

Trait bounds may bind associated types in the same bracketed trait argument
list. Positional trait arguments come first, followed by named associated type
bindings:

```nia
fn add_same[T](a: &T, b: T) T
where T: Add[T, Output = T] {
    a.add(b)
}

fn mapped[T](value: &T) i32
where T: Mapper[i32, bool, C = i32, D = bool] {
    value.map_c(1, true)
}
```

`T: Add[T, Output = T]` selects the `Add[T]` implementation for `T` and binds
that selected implementation's `Output` associated type to `T`. The binding is
part of the trait bound; it is not a separate global type equality predicate.
Binding names must be associated types declared by the trait, and a single
bound may not bind the same associated type more than once.

Trait bounds are written in `where` clauses:

```nia
fn same[T](a: &T, b: &T) bool
where T: Eq[T] {
    a == b
}
```

Within a single predicate, multiple trait bounds are joined with `+`; commas
separate predicates:

```nia
fn ordered_show[T, U](value: &T, other: &U) bool
where T: Ord[U] + Show, U: Eq[U] {
    value.show() == 0
}
```

The current compiler parses and lowers trait bounds, validates trait
implementation blocks, and supports receiver-method calls through generic
`where` bounds. A call such as `a.same(b)` in
`fn same[T](...) where T: Comparable` is
kept as a trait-method obligation in the generic body and resolved to the
visible concrete implementation when the generic function is instantiated.
Default methods may call other methods from the same trait; those calls are
resolved through the visible concrete implementation when available, or through
another default body. Associated type projections are normalized through the
current function's associated type bound bindings or through the visible
concrete trait implementation during checking and monomorphization. Supertrait
method obligations are resolved through the same visible concrete implementation
lookup.

Trait object pointer types are represented as fat pointers carrying an erased
object pointer and implementation metadata. A concrete pointer may be coerced to
a trait object pointer when the pointed-to type satisfies the selected trait and
associated type bindings:

```nia
trait Source {
    fn get(&self) i32;
}

fn read(source: & Source) i32 {
    source.get()
}
```

`source.get()` is a dynamic trait method call through the object's vtable.
Object-safe trait object methods must be receiver methods and may not have
method-level type parameters. By-value trait object receiver calls are rejected.
Builtin trait objects are not supported in this version.

A trait object pointer may be coerced to a supertrait object pointer when the
target trait is a declared supertrait of the source trait and mutability
matches:

```nia
trait Parent {}
trait Child : Parent {}

fn accept(parent: & Parent) void {}

fn use_child(child: & Child) void {
    accept(child)
}
```

The compiler records this as a trait-object upcast, not as a plain bitcast, and
remaps metadata to the target supertrait's vtable region. When the target
supertrait object binds associated types, the source object type must carry
matching bindings. Bindings for non-primary supertraits use explicit projection
keys:

```nia
fn use_child(
    child: & Child[
        [Self as FatherA]::Item = i32,
        [Self as FatherB]::Item = usize,
    ],
) void {}
```

## 12. Modules

Each `.nia` file is one module. Module loading is explicit: a source file may
declare child modules with `module name;` or `pub module name;`. A `using` item
opens names that are already available through the current module graph or a
module-map pkg root; it does not implicitly discover files.

A compilation has one reserved entry package named `root`. The reserved
`pkg` root names the current module's own package. The CLI also provides a
default `std` pkg root and accepts additional package roots with
`-M name=path` or `--module name=path`. One `-M` entry is one package. Package
roots are lazy: they are loaded when referenced by `using`, or when executable
emission injects the standard startup contract.

### 12.1 Module Declarations

A module declaration loads a child module under the current module stem:

```nia
module math;
pub module geom;
```

For an entry file at `src/app/main.nia`, `module math;` loads
`src/app/math.nia`. If `math.nia` declares `module ops;`, that declaration loads
`src/app/math/ops.nia`. Nia intentionally has no `mod.rs` form.

The child module's logical name is taken from its declaration site. A file may
change the logical child name it exposes by declaring a nested module itself:

```nia
// src/app/main.nia
module foo;

// src/app/foo.nia
module zoo; // loads src/app/foo/zoo.nia
```

`pub module name;` makes the child module namespace part of the current module's
public surface. Plain `module name;` loads the child for the current module but
keeps the namespace private to the package visibility rules.

### 12.2 Pkg Roots And Paths

The reserved path roots are:

- `root`, the compilation entry package, regardless of the current module;
- `pkg`, the current module's pkg root;
- `std`, the standard-library pkg root unless overridden with `-M std=...`;
- any additional pkg root supplied with `-M name=path`.

Examples:

```bash
nia check src/main.nia -M std=/usr/share/nia/std.nia -M math=vendor/math.nia
```

```nia
using std;
using std::io;
using math;
using pkg::internal;
```

A mapped root file is the pkg root module. Tail segments select declared
child modules below that root. If `std` maps to `/usr/share/nia/std.nia`, then
`using std::io;` refers to the `io` child declared by that root and backed by
`/usr/share/nia/std/io.nia`.

Within a loaded module, `self` names the current module and `super` names the
parent module. `pkg` names the current pkg root, while `root` still
names the compilation entry package:

```nia
// src/app/foo/zoo.nia
using super::helper;
using pkg::internal;
using root::config;
```

Package implementation code should use `pkg::...` for absolute references
to its own modules. For example, the standard library's implementation modules
use `pkg::io` and `pkg::process`; user packages still refer to those
public modules as `std::io` and `std::process`.

### 12.3 Using

`using` shortens visible namespaces in the current scope. It can reference a
pkg root, a loaded module namespace, an item namespace, or an enum namespace.
It does not load arbitrary files; child files become modules only through their
parent's `module` declaration.

Supported forms:

```nia
using std;
using std::process;
using pkg::internal;
using root::math as m;
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
  top-level definitions, public module namespaces, public module namespaces
  re-exported with `pub using`, and public items re-exported with `pub using`.
- `using Enum::*` imports all variants of that enum.

Top-level `using` is visible throughout the file. Block-local `using` is visible
only in that block and its children. Duplicate imported names in the same
namespace and same scope are errors, whether they come from explicit selections
or wildcards.

Imported items enter the namespace matching their actual category: functions and
globals enter the value namespace; structs, enums, and type aliases enter the
type namespace; enum variants enter the value namespace while preserving their
enum identity.

### 12.4 Pub Using

`pub using` re-exports selected items as part of the current module's public
surface:

```nia
// facade.nia
using root::impl;
pub using impl;
pub using impl::add;
pub using impl::{frob as do_frob};
pub using impl::*;

using root::palette;
pub using {impl, impl::add, palette::Color};
pub using palette::Color;
pub using palette::Color::Red;
pub using palette::Color::*;
```

```nia
// main.nia
using root::facade;
pub fn color() facade::Color {
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

### 12.5 Visibility

Modules and declarations are private by default. Public APIs are marked with
`pub`, and restricted visibility can be written as `pub(super)` or
`pub(pkg)`. `pub(super)` exposes the item to the parent module and its
children. `pub(pkg)` exposes it within the package selected by one `-M`
entry or by the reserved `root`/`std` packages.
Restricted visibility is still cross-module visibility. Multi-object emission
must retain a `pub(super)` or `pub(pkg)` definition for callers in its visible
scope; only a truly private definition is eligible for module-local dead-code
elimination without whole-program reference evidence.

`pub` may be applied to `module`, `fn`, `struct`, `enum`, `type`, `let`, `let mut`,
`extern` declarations, and `using`. Nia has no separate `mod` or `use` syntax.
Package management is outside the language specification.

## 13. ABI, Runtime, And Symbols

Nia does not require a garbage collector, exception runtime, async runtime, or
hidden allocator.

Extern interop uses the C ABI and is an explicit boundary, not Nia's default
runtime model:

```nia
extern fn foreign_log(message: &u8);
```

When calling C string APIs, use `b"...\0"` and pass an explicit pointer to the
first byte:

```nia
foreign_log((&b"hello\n\0").ptr());
```

String and byte string literals are array values, not pointers. There is no
implicit `[N]u8` to `&u8` decay; take an explicit address and then use `ptr()`
when an ABI requires an element pointer.

A literal evaluated in a function does not acquire static storage merely
because its contents are compile-time text. An address or slice derived from
that array must not escape the lifetime of the containing value or frame. Nia
does not infer or check a borrow lifetime for the pointer; callers that need a
longer-lived view must place the array in storage with that lifetime or copy it
into owned storage.

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
- `&T` encodes as `ptr_read__<T>`;
- `&mut T` encodes as `ptr__<T>`;
- `^T` encodes as `vptr_read__<T>`;
- `^mut T` encodes as `vptr__<T>`;
- trait object pointers encode as `trait_obj__<trait>...` or
  `trait_obj_read__<trait>...`;
- `[N]T` encodes as `arr__<len>__<elem>`;
- function pointers encode as `fnptr__pc<N>__<p1>__...__ret__<ret>`, with
  `__variadic` appended for variadic function pointers;
- optional types encode as `opt__<T>`;
- error union types encode as `err_union__<E>__<T>`;
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
- arrays, slices, pointers, optional types, error union types, structs, unions,
  C-style enums, and function types;
- `let mut`, `let`, and `const` bindings;
- expression blocks and tail expressions;
- `if` expressions;
- the three `for` forms;
- `defer`;
- `switch` and enum exhaustiveness checks;
- `@size[T]()`, `@align[T]()`, `value.len()`, `range.start()`, `range.end()`, `slice.ptr()`, and `@asm({...})`;
- explicit `module` declarations, module-map package roots, and `using`;
- global static storage from top-level `static mut` and `static`;
- top-level `pub` visibility;
- `extern` C declarations, definitions, and calls;
- generic functions and structs via monomorphization;
- methods declared through `extend`;
- trait declarations, associated types, and direct trait implementation checks;
- lowering to a typed backend IR;
- LLVM IR or object emission;
- freestanding executable emission for Linux x86_64 when a target linker is
  available.

Borrowed scalar text uses the language-native `&[char]` representation.
`std::fs::PathView` remains a nominal borrowed path over `&[char]`, while
`std::String` and `std::fs::PathBuf` own caller-allocated storage.
`String` is the sole public owned/mutable scalar-text type; `StringBuf` is not a
compatibility alias. `String::fromSlice(allocator, text)` copies borrowed scalar
text, and `text()` returns its canonical read-only `&[char]` view.
`std::String::fromUtf8(allocator, bytes)` performs typed whole-buffer
conversion: empty bytes are valid empty text, invalid non-empty sequences return
`std::TextError::InvalidUtf8`, and allocation failures return
`std::TextError::Allocation`.
`String::appendUtf8(allocator, bytes)` has the same error model and preserves
the original scalar text when validation or allocation fails.
`String::reserve(allocator, additional)` reserves an additional scalar count.
After that capacity is established, `pushAssumeCapacity(ch)` and
`appendAssumeCapacity(text)` perform allocator-free mutations; their explicit
precondition is that the complete result fits the existing capacity.
`String::appendFormat(allocator, template, args)` formats through a temporary
byte buffer and then performs typed UTF-8 append. It returns
`TextFormatError`, which distinguishes `Format`, `InvalidUtf8`, and
`Allocation`, and preserves the original text for formatting, UTF-8,
allocation, or temporary-buffer cleanup failure. `TextError` contains only the
`InvalidUtf8` and `Allocation` cases that UTF-8 construction and append can
actually produce.
Borrowed `[char]` receives `equals`, `startsWith`, `endsWith`, `find`, and
`contains` from the generic slice sequence API. `String` exposes the same
scalar-text content operations and borrowed `split(separator)` iterator by
delegating to its borrowed view. Borrowed `[char]` and `String` also expose
allocator-explicit `replaceAll`, which creates independent owned text.
Borrowed text sequences expose allocator-explicit `join`, also producing an
independent `String`. `find`
returns the first matching scalar index as `?usize`. An empty needle matches at
index zero, and these operations are allocation-free.
`[char]` and `String` implement `std::hash::Hash[H]` when `H` implements
`Hasher`. Text hashing writes the scalar count followed by each scalar value;
`String` delegates to that borrowed representation and implements content
`Eq[String]`, so equal owned text has equal hash output.
`String::fromOwnedSlice` adopts an allocator-owned scalar slice without
changing its allocator provenance, and `intoOwnedSlice(allocator)` transfers
the exact initialized allocation out while emptying the string. `PathBuf` does
not repeat that low-level adoption boundary: `PathBuf::fromString` transfers an
owned string, while `fromView` and `fromUtf8` allocate and copy.
`PathBuf::fromView(allocator, path)` copies a borrowed path and reports
`mem::Error`; `PathBuf::fromUtf8(allocator, bytes)` preserves `TextError` rather
than collapsing decoding and allocation failures into filesystem errors.
`joinComponent(allocator, text)` reserves the complete mutation before changing
visible text, so allocation failure preserves the original path. Pure PathBuf
construction, mutation, and release report `mem::Error`.
`PathView::encode(storage)` and `PathBuf::encode(storage)` are the sole checked
OS-byte conversion operations. They return `PathError::ContainsNul` for an
embedded NUL and `PathError::TooLong` when the caller's storage cannot contain
the encoded bytes plus their terminator. `EncodedPath` can only be constructed
by these checked operations; it exposes bytes with and without the terminating
NUL for immediate OS calls.

The CLI surface is:

```text
nia build [step] [--root dir]
nia check <file.nia> [--runtime bare|freestanding] [--opt-report]
nia emit --tokens <file.nia>
nia emit --ast <file.nia>
nia emit --checked <file.nia> [--runtime bare|freestanding] [--opt-report]
nia emit --backend <file.nia> [--runtime bare|freestanding] [--opt-report]
nia emit --llvm <file.nia> [--runtime bare|freestanding] [--opt-report]
nia emit --obj <file.nia> [-o file.o | --out-dir dir] [--runtime bare|freestanding] [--opt-report]
nia emit --exe <file.nia> [-o executable] [--runtime freestanding] [--link-arg arg] [--opt-report]
```

`nia build` is reserved as the toolchain-owned package build entry point for
`build.nia`. It resolves package roots before build execution and keeps build
outputs under `.nia-build/` separate from reusable `.nia-cache/` entries.
Both directories are created before the build script runs. `build.nia` is
compiled and run as ordinary Nia code through a generated toolchain-owned
runner, so it can use the standard library. The runner passes a
`std::build::Build` value with explicit `packageRoot()`, `buildDir()`,
`cacheDir()`, and `toolchainExecutable()` accessors instead of requiring scripts
to infer those paths. It only configures, validates, and encodes an immutable
plan; the Rust coordinator validates that frozen plan and executes the selected
dependency closure.
`Build::hostTarget()` and `Build::artifactTarget()` return borrowed
`TargetView` descriptors containing architecture, vendor, OS, environment, ABI,
endian, and pointer width. Their text remains valid for the lifetime of the
`Build`; the generated runner's temporary decoding buffers are not retained.
`Build::addModule(ModuleOptions::init(name, rootSource))` declares a
package-rooted source module and returns a module handle.
`ModuleOptions::fromBuild(name, BuildPathView::init(path))` instead declares a
build-rooted source, including a source produced by a generated-file action,
without aliasing its logical identity as a package path.
`ModuleOptions::withOptimization(mode)` overrides the default optimization
mode. `Build::addExecutable(ExecutableOptions::init(name, rootModule))`
declares an executable artifact and returns an executable handle;
`ExecutableOptions::withOutputName(name)` and
`ExecutableOptions::withRuntime(runtime)` customize it.
`Build::addCheckExecutableStep(name, target)` adds a graph step that checks
that artifact through the freestanding executable runtime with the package root
as the working directory. `Build::addEmitExecutableStep(name, target)` adds
a graph step that emits the artifact to
`.nia-build/<output-name-or-target-name>` through a typed Driver request.
`Build::addAggregateStep(name)` declares a dependency-only graph node.
`Build::addGeneratedFileStep(name, BuildPathView::init(path), contents)`
declares atomic publication of the supplied bytes under `.nia-build/`.
`Build::addRunExecutableStep(name, RunOptions::init(executable))` declares an
outputless external-command action whose program is the typed executable
artifact and whose working directory is the package root. The executable must
already have an emit step; the builder adds that producer dependency and plan
freeze verifies it independently.
`RunOptions::withArguments(arguments)` supplies `&[&[char]]` arguments.
`Build::addExternalCommandStep(name,
ExternalCommandOptions::search(program))` declares a searched external program.
`ExternalCommandOptions::withArguments` and `withWorkingDirectory` declare its
arguments and package-relative working directory. Its resource class defaults
to `ActionResourceClass::Conservative`, which reserves all ready-action capacity
because an otherwise unknown process cannot safely overlap other build work.
`withResourceClass(ActionResourceClass::Cpu)` and
`withResourceClass(ActionResourceClass::Io)` explicitly declare actions that
consume one action slot. Unknown open-enum values are invalid build input.
Names, paths, imports, arguments, and options passed to these methods are
borrowed only for the duration of the call. `Build` copies retained values into
allocator-owned storage, so local arrays may leave scope before a selected step
executes.
Module imports and run arguments have no legacy fixed-count limit. Their encoded
bytes and retained lists use the build allocator; allocation failure is reported
as `build::Error::OutOfMemory`, not as an invalid target or panic.
`Build::setDefaultStep(step)` selects the graph step used by `nia build` when
no step name is passed. If a script registers steps but does not set a default,
`nia build` without an explicit step exits with an invalid build-script error.

Module-map options are accepted before or after the command:

```text
-M name=path
-Mname=path
-M=name=path
--module name=path
--module=name=path
```

Optimization options are accepted before or after the command:

```text
-O
-O0
-O1
-O2
-O3
-Os
-Oz
```

Timing options are accepted before or after the command:

```text
--timings
--timings=summary
--timings=detail
```

`-O` means `-O2`. `nia check <file.nia> --opt-report` prints the active
optimization policy and backend optimization report to stdout. `nia check
<file.nia> --runtime freestanding` checks with the same startup runtime that
`emit --exe` injects, including the public
`root::main(process::Init) process::ExitCode!void` entry contract. Emit commands
write the same report to stderr when
`--opt-report` is supplied, so stdout remains backend IR or LLVM IR and native
emit targets remain file-only.
Timing reports are written to stderr; `--timings=detail` also includes
aggregated query timings. Raw timing events are available through the explicit
`--timing-trace=events` diagnostic option.

`emit --obj` defaults to the bare runtime and writes one object per backend
codegen unit without injecting startup code. `emit --obj --runtime freestanding`
checks and lowers with the same startup injection used by executable emission.
`-o` is only valid for a single-unit program. Multi-unit output uses
`--out-dir`. `emit --exe` writes temporary objects and invokes the target linker
without CRT startup. Extra linker arguments are passed with repeated
`--link-arg` options. The linker is selected with `NIA_LINKER`; if it is unset,
the target default linker is used.
Native emit commands create missing output directories: `emit --obj -o
build/main.o`, `emit --obj --out-dir build/obj`, and `emit --exe -o build/main`
all create `build` or `build/obj` when needed. This applies only to compiler
output paths, not to input source files or module-map paths.

`build` is reserved for an external build system. The current CLI does not
provide `run`; use `emit --exe` and execute the result.

## 15. Reserved Future Design Areas

The following areas are intentionally outside the current language surface.
They are reserved for future design and should not be treated as specified by
this document:

- associated type families and full trait obligation solving;
- payload-carrying algebraic data types beyond current enums;
- aggregate destructuring patterns beyond current optional and error-union
  patterns;
- closures;
- package management semantics;
- LSP semantics;
- large standard-library layering;
- volatile pointer families;
- SIMD as a language primitive;
- user-defined attributes and arbitrary compiler intrinsics;
- macros;
- general compile-time execution beyond the current `const` value subset.

## 16. Example

```nia
using std;
using std::process;

struct Pair[T, U] {
    first: T,
    second: U,
}

struct Vec2 {
    x: i32,
    y: i32,
}

extend Vec2 {
    fn len2(&self) i32 {
        self.x * self.x + self.y * self.y
    }
}

fn add(a: i32, b: i32) i32 {
    a + b
}

fn sum(xs: &[i32]) i32 {
    let mut total = 0;
    for &value in xs {
        total += value;
    }
    total
}

fn score(answer: Pair[i32, i32]) i32 {
    switch answer.first {
        0..10 => answer.second,
        10..=42 => answer.first + answer.second,
        _ => 0,
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;

    let mut v: Vec2 = { x: 3, y: 4 };
    let mut values = [_]i32[add(40, 2), v.len2(), 7];
    let mut pair: Pair[i32, i32] = { first: values[0], second: sum(&values) };

    if score(pair) != 116 {
        return process::exit(1)!;
    }

    !{}
}
```
