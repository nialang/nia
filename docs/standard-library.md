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
package-private. The public `os` surface is not a shortcut around typed host
services; platform-specific syscalls and descriptors belong below fs/io/process.
Ordinary programs that do not use build must not load `std::build` or its
providers.

## 3. Build-Host Dependency And Maturity Matrix

| Slice | Current use by build | Maturity finding | Disposition |
| --- | --- | --- | --- |
| `builtin`, primitive/layout/control | language and ABI foundation | retain candidate; compiler/runtime contract needs direct conformance | retain and version with toolchain resources |
| `mem` allocators/copy/layout | runner allocation and owned collections | manual allocator plumbing is pervasive; rollback/deinit and allocation failure need explicit contracts | retain capability, redesign ownership where plan values escape |
| `collections::ArrayList` | steps, edges, modules, targets, path decoding | unchecked operations and duplicated raw/list surfaces are not classified; allocator repeats at every call | audit checked vs explicitly unchecked APIs; narrow public surface |
| `string` and `unicode` | names, UTF-8 path conversion, formatting | borrowed `StringView` and retained plan text have no frozen-plan lifetime contract | introduce explicit owned/borrowed roles before plan freeze |
| `slice` and iterators | graph scans, argv/import construction | mostly foundational, but trapping convenience methods need checked/unchecked classification | retain minimal core after conformance |
| `fmt` | runner diagnostics and telemetry | useful formatting core; template misuse collapses to `Internal` in build | retain capability; separate programmer-format errors from I/O diagnostics |
| `fs` path/file/options | package/build/cache paths and generated files | coarse `Invalid/TooLong/Io`; fixed encoded path capacity; relative/root policy implicit | redesign typed path ownership, roots, and contextual errors |
| `io` | stdout/stderr/files | public runtime type exposes `os::Error`; buffering/flush cleanup semantics need a matrix | keep host-service facade, hide OS provider and test partial writes |
| `process` args/env/command | runner context and compiler subprocess | raw argv/envp and coarse spawn/wait errors leak bootstrap mechanics | retire from BuildPlan boundary; expose typed process action/service |
| `os` Linux facade | path capacity, descriptors, process and I/O providers | broad public low-level facade is a layer violation for build scripts | keep package-private platform provider; review intentional unsafe API separately |
| `build` | graph declaration and execution | callback executor, raw argv, index-only handles, coarse errors | bootstrap-only; replace with builder plus immutable plan |

Direct `std::build` source imports currently reach `collections`, `process`,
`fmt`, `fs`, `io`, `mem`, `os`, `slice`, and `string`; path/string conversion
also pulls Unicode behavior. The conservative source-declared facade/provider
closure is recorded in `std-build-host-dependencies.json` and checked by
`tools/std_build_host_audit.py`. It currently contains 93 modules: broad facade
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
current existence of `[char]`, `StringView`, `StringBuf`, Unicode helpers, and
formatting traits does not by itself establish a coherent string API.

Compiler-known traits receive the same scrutiny. Operators and structural place
semantics may require compiler participation, but convenience methods do not
automatically require builtin trait identity. `Len`, `Start`, `End`, `Ptr`,
`PtrMut`, and `Char` must each justify why an ordinary trait with
compiler-provided structural implementations or intrinsic bodies is
insufficient. Their current cross-compiler implementation is audit evidence,
not a reason to preserve it.

The current audit sets this design direction:

- borrowed scalar text is provisionally `&[char]`; `StringView` survives only if
  it gains a checked nominal invariant that a slice does not express;
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

The current `utf8_decode_first` optional result is not an acceptable final error
contract because empty input, truncation, invalid leading bytes, invalid
continuations, overlong forms, and invalid scalar values are different states.
Likewise, converting all text/path failures directly into `fs::Error` loses
information needed by build diagnostics and process boundaries.

Initial convenience-trait dispositions are deliberately asymmetric. `Char` is
a Unicode conversion API rather than polymorphic language dispatch.
`Start`/`End` are range accessors. `Len` and `Ptr`/`PtrMut` need compiler-created
implementations for structural arrays, slices, or data pointers, but that does
not require builtin trait identity. Each migration must trace symbol identity,
type/projection solving, const evaluation, reachability, backend dispatch, and
LLVM before deleting its builtin declaration.

## 5. Ownership And Error Baseline

Build-plan text and paths must be owned by the builder/plan arena or copied into
the serialized plan. A `StringView`, `PathView`, module import, or target name
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
refer to explicitly stable storage. The same rule applies to `StringView`,
`PathView`, target text, and every future frozen-plan field.

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

The current bootstrap can declare a distinct artifact target, but its legacy
callback executor still launches the public CLI, which has no explicit target
selection surface. It therefore rejects execution when host and artifact target
differ instead of silently producing a host artifact. Phase D/E typed compiler
actions replace that bootstrap boundary; no temporary arbitrary-target setter
is exposed from `ExecutableOptions`.

Build command assembly has no import-count or argv-count sentinel. Module-map
arguments are UTF-8 encoded into allocator-owned contiguous storage; offsets are
collected while the buffer may grow, and raw pointers are created only after
encoding is complete. Both the build argument list and the process-level
`argv`/null terminator list are allocator-backed and use conditional cleanup.
Allocation failure is `OutOfMemory` through the process/build error path, while
embedded NUL remains an invalid target input. The OS path byte limit remains a
separate explicit filesystem boundary, not an argv capacity limit.

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
