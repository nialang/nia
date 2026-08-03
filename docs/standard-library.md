# Nia Standard Library Architecture

Status: Phase C build-host foundation complete

The current standard library demonstrates usable capabilities, but most of its
public APIs predate a deliberate stability review. Existing code is therefore
audit input, not an automatic compatibility requirement.

## 1. Design Premise

The std/build project does not blindly rewrite 15,000 lines and does not freeze
whatever the bootstrap happens to call. Work proceeds from the build-host slice
outward. Each retained API must have an explicit layer, ownership/lifetime
contract, error model, naming/surface decision, and conformance evidence.

Current experimental build and std APIs may be changed or removed without a
compatibility shim. Historical spellings and duplicate facades belong in Git,
not permanent adapters.

Standard-library reconstruction is also language-usage research. Long-standing
APIs are not successful merely because they compile or expose the required
primitive: representative Nia programs must be able to discover and compose
them without routinely bypassing `std`, rebuilding the abstraction locally, or
dropping to platform providers. Each reviewed slice starts from a user workflow,
records the idiomatic Nia spelling it enables, and treats awkward ownership,
error propagation, cleanup, or conversion at adjacent APIs as design evidence
for the next slice.

Native and compiler-backed operations are inputs to that work, not the default
public shape. A capability remains an ordinary Nia implementation when the
language can express it clearly. `@[builtin]`, inline assembly, raw pointers,
and platform calls stay at the narrowest layer that requires them; high-level
APIs are judged by whether users can avoid those layers in ordinary programs.

## 2. Target Layers

```text
builtin and language core
  -> runtime and platform providers
  -> allocation and data structures
  -> host services: path, fs, io, process, environment
  -> build-host contracts
  -> ordinary high-level library facilities
```

The root `std` facade exports intentional user contracts. Provider modules stay
package-private. There is no public `os` shortcut around typed host services;
platform-specific syscalls and descriptors belong below fs/io/process.
Ordinary programs that do not use build must not load `std::build` or its
providers.

## 3. Build-Host Dependency And Maturity Matrix

| Slice | Current use by build | Maturity finding | Disposition |
| --- | --- | --- | --- |
| `builtin`, primitive/layout/control | language and ABI foundation | retain candidate; compiler/runtime contract needs direct conformance | retain and version with toolchain resources |
| `mem` allocators/layout | runner allocation and owned collections | manual allocator plumbing is pervasive; rollback/deinit and allocation failure need explicit contracts | retain capability, redesign ownership where plan values escape; ordinary slice copy/compare no longer lives here |
| `collections::ArrayList` | steps, edges, modules, targets, path decoding | reviewed owned extraction and initialized-value mutation surface; allocator repeats at every allocating call | retain narrow initialized-value surface; finish common allocator protocol |
| `string` and `unicode` | names, UTF-8 path conversion, formatting | borrowed scalar text is `&[char]`; retained plan text must cross an explicit copy boundary | borrowed `&[char]` and owned `String` accepted; finish allocator design |
| `slice` and iterators | graph scans, argv/import construction | borrowed iteration and core checked access reviewed; direct indexing and range slicing are the language's unchecked primitives | retain direct iteration, optional checked access, and minimal adapters; continue specialized operation audit |
| `fmt` | runner diagnostics and telemetry | useful formatting core; template misuse collapses to `Internal` in build | retain capability; separate programmer-format errors from I/O diagnostics |
| `fs` path/file/options | package/build/cache paths and generated files | reviewed scalar ownership and typed encoding boundary; fixed encoded path capacity and relative/root policy remain | retain path roles; redesign roots, OS representation, and contextual file errors |
| `io` | stdout/stderr/files | blocking file/standard-stream adapters now hide raw handles and require only caller storage; Reader/Writer naming and child-pipe errors are reviewed | retain direct blocking adapters and generic buffering; finish filesystem error/context and cleanup matrix |
| `process` args/env/command | runner context and compiler subprocess | typed commands, environments, child-pipe roles, lifecycle, process-owned identity, and structured spawn/system causes exist | retire from BuildPlan boundary; continue lifecycle and raw-boundary audit before accepting the service facade |
| `os` Linux provider | path capacity, descriptors, randomness, process and I/O providers | the root module, operations, errors, handles, and process identity are package-private | keep provider private; expose host capabilities only through typed service facades |
| `build` | graph declaration and execution | callback executor, raw argv, index-only handles, coarse errors | bootstrap-only; replace with builder plus immutable plan |

Direct `std::build` source imports currently reach `collections`, `process`,
`fmt`, `fs`, `io`, `mem`, `os`, `slice`, and `string`; path/string conversion
also pulls Unicode behavior. The conservative source-declared facade/provider
closure is recorded in `std-build-host-dependencies.json` and checked by
`tools/std_build_host_audit.py`. It currently contains 96 modules: broad facade
declarations make almost the entire std tree reachable from the build host,
including hash-map, math, and low-level Linux providers that build does not
conceptually require. This is not a claim that loader demand executes every
module and is not a module-count optimization target. It exists to expose
forbidden conceptual layer edges. Phase C performance and isolation decisions
use the loader's observed semantic, body, and backend closures instead.

The Phase D builder-owner identity adds the reviewed `atomic` facade to this
source closure. It supplies a process-local monotonic owner id for live handles;
it is neither stable plan identity nor evidence that ordinary collection users
execute atomic or build code.

Public facade imports remain written for humans. Equivalent broad and narrow
public `using` forms must select the same provider, semantic, body, and backend
work for the same referenced items; rewriting std code to name package-private
providers is not accepted as an optimization. Package roots and reexport
facades stay shallow until concrete paths or provider demands activate them.
Semantic provider activation supplies that provider's own import scope without
turning semantic-only dependencies into body-check or backend work.

## 4. API Review Rules

Every reviewed API receives one or more findings:

- `retain`: layer and semantics are suitable after conformance;
- `layer violation`: implementation/provider detail is public at the wrong
  level;
- `ownership/lifetime issue`: borrowed or allocator-owned state can outlive its
  source or has unclear cleanup;
- `error-model issue`: ordinary failure loses context, aliases unrelated causes,
  or traps;
- `naming/surface issue`: duplicate, overly broad, inconsistent, or misleading
  contract;
- `bootstrap-only/retire`: capability is represented in the target design but
  the current API must be physically removed.

An unchecked operation is allowed only when its name and preconditions make the
invariant explicit and its callers have already validated it. Accidental panic
from ordinary path, input, allocation, process, or collection failure is not an
unchecked contract and must become a typed error. Genuine impossible compiler
or runtime invariants use the project ICE boundary rather than ad hoc panic in
normal std logic.

Repository std code is also part of the public design evidence. It should use
the same type inference and error idioms expected from user packages rather
than preserving bootstrap-era explicitness:

- numeric process failure statuses are constructed with `process::exit(code)`;
  direct casts to `process::ExitCode` are confined to that conversion boundary
  and explicit enum-cast conformance tests;
- `.exit().?` is used when a reviewed std error mapping is being propagated
  through an executable entry;
- a numeric literal suffix is omitted when a parameter, return type, field,
  place, operator, or peer expression already supplies its type;
- suffixes remain when they define ABI or layout width, serialization or bit
  arithmetic, intentional mixed-width behavior, otherwise ambiguous literals,
  or the behavior under test.

This is a semantic review rule, not a formatter rewrite. Removing a suffix must
leave an authoritative inference source, while retaining one must communicate
information that the surrounding type context does not already say.

The std review also treats language ergonomics as a system rather than a list of
working primitives. Text must be sampled end to end from string literals and
UTF-8 conversion through borrowed/owned values, formatting, mutation,
comparison, paths, OS boundaries, allocation failure, and defer cleanup. The
current existence of `[char]`, `String`, Unicode helpers, and
formatting traits does not by itself establish a coherent string API.

Compiler-known traits receive the same scrutiny. Operators and structural place
semantics may require compiler participation, but convenience methods do not
automatically require builtin trait identity. `Len`, `Start`, `End`, `Ptr`,
`PtrMut`, and `Char` must each justify why an ordinary trait with
compiler-provided structural implementations or intrinsic bodies is
insufficient. Their current cross-compiler implementation is audit evidence,
not a reason to preserve it.

The current audit sets this design direction:

- borrowed scalar text is `&[char]`; the redundant one-field `StringView` is
  retired because it enforced no invariant beyond the slice;
- there will be one public owned/mutable scalar-text type, not parallel view,
  buffer, and string wrappers with equivalent semantics;
- arbitrary bytes, validated UTF-8, scalar text, C strings, and OS paths remain
  separate roles with explicit, typed conversion failures;
- adjacent and multiline literals are compile-time text construction; runtime
  concatenation must have one obvious append/format path shared by borrowed and
  owned text;
- scalar length and encoded byte length are named and tested separately;
- allocator ownership remains visible, but construction, growth, transfer, and
  `defer` cleanup must form one reviewable protocol rather than requiring users
  to reconstruct ownership at each call.

`unicode::decodeUtf8First` now returns
`Utf8DecodeError!Utf8Decode`. `Empty`, `Truncated`, `InvalidLeadingByte`,
`InvalidContinuation`, `Overlong`, and `InvalidScalar` are distinct values;
the former optional decoder is absent and has no compatibility alias.
`String::fromUtf8(allocator, bytes)` and
`String::appendUtf8(allocator, bytes)` are the whole-buffer owned boundaries
and return `TextError`: empty bytes are valid empty text, non-empty invalid
input retains its `Utf8DecodeError`, and allocation failure remains a separate
payload. Both validate and count before allocating or changing visible length;
failed construction returns no partial value, while failed append preserves the
original text.
`String::appendFormat(allocator, template, args)` formats into temporary
bytes, validates those bytes through the same UTF-8 append boundary, and only
then commits scalar text. Its `TextFormatError` distinguishes `Format`,
`Allocation`, and `InvalidUtf8`; the narrower `TextError` returned by UTF-8
construction and append contains only the latter two cases those operations can
produce. A failed call preserves the original text.
Temporary-buffer release completes before commit; a release failure takes
precedence and an infallible conditional `defer` restores the old scalar
length. The current ordinary spelling imports `std::fmt` and gives the
heterogeneous argument array one type annotation, for example
`let args: [2]&fmt::Format = [&value, &ch]`, before passing `&args`.
Generic slices whose elements implement `Eq[T]` provide `equals`, `startsWith`,
`endsWith`, `find`, and `contains` as allocation-free ordered-sequence
operations. Borrowed `[char]` therefore gets the scalar-text vocabulary from
the slice API, while `String` delegates to its borrowed view rather than
maintaining a second search implementation. `find` returns the first element
index as `?usize`; an empty needle is found at index zero, so it is also
contained and is both a prefix and suffix. Content equality is named explicitly
instead of treating reference equality as sequence equality.
With `std::hash` imported, `[char]` and `String` implement `Hash[H]` for
every `Hasher`. The hash commits the scalar count followed by each scalar value;
it is not a hash of an incidental UTF-8 encoding. `String` also implements
content `Eq[String]`, and equal borrowed/owned scalar sequences produce the
same hash. `HashMapLookupContext[K, Q]` separates stored-key and query-view
types. The default context supports `String` keys queried by `&[char]` through
`containsKeyBy`, `getBy`, `getMutBy`, `getEntryBy`, `getEntryMutBy`, `getKeyBy`,
and `removeEntryBy`; these operations do not allocate. A slice variable infers
the query type directly. A literal currently needs one method annotation, for
example `map.getBy[&[char]](&"name")`, because provider impl patterns cannot
match `&[N]char` as a trait argument and generic calls do not apply the slice
coercion before inference.

`removeEntryBy` returns the stored key so allocating ownership can be released.
After error propagation, `insert` yields `?HashMapReplacement`. A new key
produces `null`; an equal stored key keeps its original key allocation and the
replacement payload contains both the rejected incoming key and the replaced
stored value. `insertIfAbsent` similarly yields `?HashMapEntry`, whose payload
is the complete incoming entry when the key was already present. The
assume-capacity variants return those optionals directly with the same ownership
results. This makes `if result is ?replacement` the ordinary single-branch
cleanup spelling and removes the former `put`/`fetch_put`/`put_if_absent`
surfaces that silently lost owned inputs.
Returned entries transfer their fields with `intoKey()` and `intoValue()`;
mutable entry views expose `valueMut()`. The former snake-case methods are
absent rather than duplicated.

For a fallible insertion, ownership remains with the caller until the method
returns success; callers transferring allocating named values use a conditional
`defer` and mark transfer only after success, as elsewhere in reviewed std.
Map teardown still does not recursively deinitialize owned keys or values. The
reviewed `getOrInsert` entry operation takes an explicit initial value and
returns the stored key/value references plus `intoRejected()` for the complete
incoming entry when an equal key already exists. Its assume-capacity form has
the same ownership result. The former value-taking operation and the
uninitialized no-value `get_or_put` operations are physically absent. Maintained
String conformance explicitly takes back rejected keys from replacement,
if-absent, and entry insertion, deinitializes them, then removes and
deinitializes the stored key. Deep element cleanup, a future lazy construction
entry API, and the common allocator protocol remain open.

`HashMap::drain()` is the explicit bulk ownership-transfer path. Its iterator
scans the bucket array once and returns owned `HashMapEntry` values, so callers
can use ordinary `for`, `intoKey()`/`intoValue()`, and type-specific `deinit`
operations. Each produced entry is removed immediately; stopping early leaves
unvisited entries in the map. Exhaustion restores the empty control table and
retains capacity for reuse. `ArrayList::pop()` provides the corresponding
owned-element extraction primitive for lists. Neither container stores an
allocator or infers element cleanup.

`ArrayList` exposes only initialized-value operations. `push`, `appendSlice`,
`insert`, `insertSlice`, and `replaceRange` finish initialization before
returning; the former public slot-growth, arbitrary `resize`, allocated-storage,
and unused-capacity slice APIs are absent. `truncate` is the only
length-discarding operation. `shrinkToFit` and `shrinkToCapacity` may release
storage but never remove elements. `clear` retains allocation and `deinit`
releases it; the duplicate cleanup and append spellings have no aliases.
`reserveExact` remains because, unlike `reserve`, it deliberately avoids
geometric growth.

Borrowed slices implement `Iterable` directly. `for &value in items` is the
ordinary read-only spelling for both `&[T]` and `&mut [T]`; the shared
`Iterable::iter(&self)` contract never manufactures mutable element references.
Call `iterMut()` explicitly to receive `&mut T`. `SliceIter` and
`SliceIterMut` expose `len`, `isEmpty`, and the ordinary iterator protocol, but
their raw-pointer constructors are private. `rev()` is one generic extension
of `DoubleEndedIterator`, rather than a copy on every slice and range iterator.
The protocol and range APIs use lower-camel `nextBack`, `forwardChecked`,
`backwardChecked`, and `fromBounds` names; former snake-case spellings are not
aliases.

Borrowed `ArrayList` parameters follow the same rule: both `&ArrayList[T]` and
`&mut ArrayList[T]` are directly iterable and yield `&T`. Mutable element
iteration remains the explicit `iterMut()` operation.

Slices expose optional `get`, `getMut`, `first`, `firstMut`, `last`, and
`lastMut` accessors. `getRange` and `getRangeMut` validate a half-open
`start, end` pair, accept an empty range at `len`, and reject reversed or
out-of-bounds ranges before invoking native slicing. Direct indexing and range
syntax remain the explicit unchecked primitives because the language performs
no runtime bounds check; std does not add duplicate `getUnchecked` or
`sliceUnchecked` methods. Maintained code uses `if access is ?value` for one
read-only branch and `switch` with `mut ?value` when classifying a mutable
optional reference.

For `T: Eq[T]`, slices also expose `equals`, `startsWith`, `endsWith`, `find`,
and `contains`. Search is for a complete contiguous slice, returns the first
matching element index, and treats an empty needle as present at zero. These
methods work directly on both `&[T]` and `&mut [T]`; a mutable slice may call a
read-only slice receiver through the ordinary mutable-to-read-only coercion.
The former `mem::equal(left, right)` helper is physically absent rather than an
alias for `left.equals(right)`.

Mutable slices expose `copyFrom(source) -> usize`. It copies the common prefix,
handles overlapping source and destination ranges, and returns the number of
elements copied. Nia rejects an implicitly discarded non-`void` result, so a
caller either uses the count or writes an explicit `_ =` where surrounding
bounds already prove an exact copy. The operation is a shallow value copy; it
does not invoke a cloning or cleanup protocol for element-owned resources.
For `T: Ord[T]`, `compare(other)` performs lexicographic comparison and returns
`std::cmp::Ordering::{Less, Equal, Greater}`. The former
`mem::copy_forwards`, `mem::copy_backwards`, `mem::order`, and `mem::Order`
surfaces are physically absent, with no aliases.

`ArrayList::asMutSlice` exposes only its initialized logical length, never the
allocation capacity. Its checked element access delegates to the slice methods
so list and slice bounds semantics have one implementation owner.

The reviewed map surface uses lower-camel names throughout public and provider
code. `clear` retains allocation, `deinit` releases it, `reserve` guarantees
additional insertion capacity, and `removeEntry` transfers an owned key/value
pair. The duplicate `clearRetainingCapacity`, `clearAndFree`, `reserveExact`,
`fetchRemove`, and `getKeyValue` entry points are absent rather than retained as
aliases. `ensureUnusedCapacity` is an implementation detail; the distinct
public `ensureTotalCapacity` operation establishes an absolute capacity floor.
`init()` and `initContext(context)` are the ordinary direct empty-map
constructors and obtain a randomized hash seed, trapping if that runtime
policy cannot be established. The recoverable forms are
`tryInit()` and `tryInitContext(context)`, which preserve randomness failures
as `HashMapInitError`. Deterministic callers use `initSeed(seed)` or
`initContextSeed(context, seed)` explicitly.

The one owned scalar-text type is named `String`, not `StringBuf`: it stores
Unicode scalars and cannot serve as an arbitrary byte buffer. Its reviewed
construction and ownership methods use `initCapacity`, `fromOwnedSlice`,
`fromSlice`, `textMut`, `isEmpty`, `ensureTotalCapacity`, and `intoOwnedSlice`.
`text()` is the only read-only borrowed view. No compatibility type alias or
duplicate `as_slice()` accessor is retained.

`PathView` remains a nominal borrowed scalar path and `PathBuf` owns a `String`.
`PathBuf::fromView` reports allocation failure as `mem::Error`, while
`PathBuf::fromUtf8` preserves `TextError`; callers no longer provide an
ArrayList scratch buffer or receive a collapsed `fs::Error::Invalid`.
`joinComponent` is the sole component-join spelling and reserves its full
mutation before changing visible text. `encode` is the sole checked OS-byte
conversion and returns `PathError::ContainsNul` or `PathError::TooLong`.
`EncodedPath` has no public unchecked constructor. Former snake-case and
duplicate copy, join, and encode entry points have no aliases.

Initial convenience-trait dispositions are deliberately asymmetric. `Char` is
a Unicode conversion API rather than polymorphic language dispatch.
`Start`/`End` are range accessors. `Len` and `Ptr`/`PtrMut` need compiler-created
implementations for structural arrays, slices, or data pointers, but that does
not require builtin trait identity. Each migration must trace symbol identity,
type/projection solving, const evaluation, reachability, backend dispatch, and
LLVM before deleting its builtin declaration.

### 4.1 Text, Path, And Process Error-Flow Matrix

| Role | Current public representation | Conversion boundary | Current failure owner | Reconstruction status |
| --- | --- | --- | --- | --- |
| borrowed scalar text | `&[char]` | literals, slices, format/build/path input | none for borrowing | native slice role accepted; no nominal wrapper |
| owned mutable scalar text | `String` over `ArrayList[char]` | copy/append/UTF-8/format with explicit allocator | `mem::Error`, `TextError`, `TextFormatError` | name, mutation, equality/search/hash and borrowed map lookup accepted; collection cleanup and common allocator protocol remain open |
| arbitrary bytes | `&[u8]` / `&mut [u8]` | I/O and raw process/OS buffers | owning I/O/process API | retained as non-text; no implicit UTF-8 meaning |
| UTF-8 sequence | borrowed bytes decoded one scalar at a time | `decodeUtf8First`, `String::fromUtf8`, `String::appendUtf8` | `Utf8DecodeError`, `TextError` | scalar and owned whole-buffer conversion accepted; nominal validated view remains open |
| C string | `CStringView` over NUL-terminated bytes | `fromBytes`; `fromPtrUnchecked` at trusted pointer boundaries | `CStringError` (`EmptyInput`, `MissingTerminator`, `InteriorNul`) | checked slice construction accepted; owned C-string design remains open |
| filesystem path | `PathView` / `PathBuf` over scalar text; `EncodedPath` at OS calls | typed UTF-8 ownership and checked OS-byte encoding | `TextError`, `mem::Error`, then `PathError`; file calls map to `fs::Error` | scalar ownership and encoding accepted; OS-native representation, roots, and richer file context remain open |
| process argument/environment and pipes | `Arg` / `EnvVar` byte views; borrowed scalar arguments; exact `EnvEntry` values; role-specific owned child pipes | `Command` typed argv/envp lowering; inherited/exact/empty environment modes; `ChildStdin: Writer`; `ChildStdout/ChildStderr: Reader`; explicit `spawnRaw` | closed `process::Error` preserves lowering/spawn/lifecycle causes; pipe operations use `io::Error`, invalidatable close state, and stream identity during child cleanup | typed command, environment, and pipe ownership accepted; process-owned identity/spawn/wait causes remain open |

Scalar count is `&[char].len()` or the owned text length. UTF-8 byte count is
the sum of each scalar's encoded `Utf8::len()` and is not interchangeable with
scalar count. A NUL scalar is valid scalar text but is rejected when crossing a
C-string or encoded-path boundary. Invalid UTF-8 is a decode failure before a
path exists; OS path rejection occurs after representation conversion.

## 5. Ownership And Error Baseline

Build-plan text and paths must be owned by the builder/plan arena or copied into
the serialized plan. A borrowed text slice, `PathView`, module import, or target name
cannot survive plan freeze merely because its current build-script stack frame
happens to remain alive during recursive execution.

The bootstrap builder now enforces this boundary directly. Public
`ModuleOptions`, `ExecutableOptions`, and `ModuleImport` values are borrowed
call descriptors; `Build` copies every retained root, step name, module path,
import name/path, target name, and output name into owned buffers. Multi-stage
construction registers conditional `defer` rollback before allocation and marks
ownership transferred only after collection insertion succeeds. Deep cleanup
runs in reverse order, attempts all releases, and returns its first error. The
rollback defer propagates cleanup failure, which deliberately overrides the
original exit path under Nia's defer semantics rather than hiding a failed
release. The
reviewed build methods, fields, parameters, and examples use `lowerCamelCase`;
the former spellings have no aliases.

Cleanup conformance distinguishes ownership release from allocator cursor
rewind. A fixed-buffer allocator may successfully free every live block yet
retain alignment padding because `Block` does not encode the pre-allocation
cursor. Code that needs transactional rewind must use a separately specified
checkpoint/restore operation; it must not infer an old cursor from the aligned
block address. Builder failure tests therefore inject allocation failures into
an allocator that counts active blocks, while exact fixed/arena checkpoint
semantics remain a separate reviewed API decision.

Fault allocators distinguish an owned non-empty allocation from an empty block
used as a collection-growth sentinel. A release fault injected into every
`free`, including empty blocks, can fire during ordinary capacity growth and
falsely appear to test rollback. Cleanup tests therefore inject only at the
owned release boundary and assert the complete contextual error as well as
zero live allocations.

Borrowed views receive no inferred lifetime from Nia. A function-local string
literal is an array value in that frame; returning `&[char]` obtained from it
creates a dangling view after return even when the source text looks constant.
Formatting helpers either write literal text while their frame is active or
refer to explicitly stable storage. The same rule applies to scalar-text
slices, `PathView`, target text, and every future frozen-plan field.

Allocator failure, invalid UTF-8, invalid path encoding, unavailable files,
partial I/O, spawn setup, process exit, cleanup, and formatting output are
ordinary typed failures. Errors retain operation, subject/path/action, and
underlying category long enough for the build coordinator to issue one
structured diagnostic. `Invalid`, `Report`, `Internal`, or an exit code alone
is not the target error contract.

The bootstrap build error model now carries `ErrorOperation`, `ErrorSubject`,
and an exact domain `ErrorCause`. Memory, formatting, filesystem, process, and
child termination values are preserved rather than translated into parallel
coarse build variants. This is the minimum accepted build-host error shape;
the immutable plan adds stable package/action/artifact subjects without
restoring aliases such as `InvalidTarget`, `CommandFailed`, or bare
`OutOfMemory`.

## 6. Host And Target Separation

`build.nia` and its std dependencies execute for the toolchain host. Artifact
targets are data passed to compiler actions. Host filesystem handles, process
environment, executable paths, and startup/runtime modules never describe the
artifact target implicitly.

The build-host std slice is versioned with the toolchain resource layout. Target
runtime/startup resources are selected independently. This separation is a
prerequisite for cross-target declarations even while the only Tier 0 runtime
is Linux x86_64 freestanding.

`Build::hostTarget()` and `Build::artifactTarget()` return `TargetView` values
over builder-owned storage. The view contains the exact toolchain target fields:
architecture, vendor, OS, environment, ABI, endian, and pointer width. The
generated runner transports both descriptors independently and `Build::init`
deep-copies them; a view supplied by runner-local decoding storage is never
retained. The backing `TargetStorage` and executable target role are
package-private implementation details, not a second public target model.

The build API can declare a distinct artifact target. The runner retains and
encodes that descriptor, while the Rust coordinator executes typed compiler
actions through a target-specific Driver; no callback executor or reconstructed
public CLI invocation remains. No temporary arbitrary-target setter is exposed
from `ExecutableOptions`.

Build command assembly has no import-count or argv-count sentinel. Module-map
arguments are UTF-8 encoded into allocator-owned contiguous storage; offsets are
collected while the buffer may grow, and raw pointers are created only after
encoding is complete. Both the build argument list and the process-level
`argv`/null terminator list are allocator-backed and use conditional cleanup.
Allocation failure remains `mem::Error::OutOfMemory` inside
`process::Error::Allocation` and then the build error path, while embedded NUL
retains its command-argument index in `process::Error::ArgumentContainsNul`.
The OS path byte limit remains a separate `PathError` payload, not an argv
capacity limit.

External build tools use typed arguments rather than interpolated host paths.
`CommandArgument::literal` carries opaque text, `packageInput` and `buildInput`
declare tracked inputs, and `buildOutput` declares an output whose argument is
replaced by a coordinator-owned staging path. The builder copies the program,
working directory, and every argument before returning. Partial allocation is
rolled back through the same propagating `defer` cleanup used by other retained
plan values.

The publication contract accepts zero or multiple build-rooted regular-file
outputs. Each `buildOutput` argument receives a distinct same-filesystem staging
path. The coordinator validates and syncs the complete produced set while
holding every destination lock, backs up accepted files, installs all new
values, syncs affected parents, and marks the transaction accepted only after
the set is complete. Spawn, timeout, exit, cancellation, missing/invalid output,
argument resolution, and pre-acceptance publication failures retire staging;
partial installation rolls back in reverse order, restoring old files and
removing paths that were previously absent. Separate directory entries are not
claimed to switch simultaneously. The protocol retains separate input/output
declarations and typed argument bindings so fingerprinting and execution cannot
disagree about which paths a tool receives.

## 7. Proposal Decision Gates

Language proposals that affect error propagation, ownership/borrowing,
comptime, reflection, module/resource loading, or host capabilities may change
the right std shape. They should be summarized during Phase A and attached to
the affected matrix rows. Decided semantics become dependencies; undecided
semantics block only stabilization of the affected API, not unrelated telemetry,
relocation, or conformance work.

## 8. Conformance Boundary

Phase C maintains direct fixtures for the exact allocation, collection, string,
Unicode, path, fs, I/O, process, and formatting operations needed by build.
Tests cover success, invalid input, unavailable resources, partial I/O/process
failure, allocation failure, rollback, and cleanup. Full compiler/build
executions remain in the resource-accounted integration harness; small library
semantics stay cheap and deterministic.

The accepted matrix includes invalid and missing filesystem paths, partial
buffered writes, partial exact reads, process exit/spawn-stage errors, repeated
failed-spawn handle cleanup, allocator failure and rollback, owned string/path
retention, formatting failures, and host/artifact target separation. Loader and
Driver tests separately prove that ordinary public imports do not select build,
hash-map, or OS provider work merely because those modules appear in the
conservative source-declared build-host closure.
