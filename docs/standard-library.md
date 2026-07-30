# Nia Standard Library Architecture

Status: Phase A dependency and API-maturity contract

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
`tools/std_build_host_audit.py`. It currently contains 92 modules: broad facade
declarations make almost the entire std tree reachable from the build host,
including hash-map, math, and low-level Linux providers that build does not
conceptually require. This is not a claim that loader demand executes every
module and is not a module-count optimization target. It exists to expose
forbidden conceptual layer edges. Phase C performance and isolation decisions
use the loader's observed semantic, body, and backend closures instead.

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

Allocator failure, invalid UTF-8, invalid path encoding, unavailable files,
partial I/O, spawn setup, process exit, cleanup, and formatting output are
ordinary typed failures. Errors retain operation, subject/path/action, and
underlying category long enough for the build coordinator to issue one
structured diagnostic. `Invalid`, `Report`, `Internal`, or an exit code alone
is not the target error contract.

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

Phase C adds direct fixtures for the exact allocation, collection, string,
Unicode, path, fs, I/O, process, and formatting operations needed by build.
Tests cover success, invalid input, unavailable resources, partial I/O/process
failure, allocation failure, rollback, and cleanup. Full compiler/build
executions remain in the resource-accounted integration harness; small library
semantics stay cheap and deterministic.
