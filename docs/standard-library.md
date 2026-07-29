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
| `build` | graph declaration and execution | callback executor, raw argv, fixed buffers, index-only handles, coarse errors | bootstrap-only; replace with builder plus immutable plan |

Direct `std::build` source imports currently reach `collections`, `process`,
`fmt`, `fs`, `io`, `mem`, `os`, `slice`, and `string`; path/string conversion
also pulls Unicode behavior. The conservative source-declared facade/provider
closure is recorded in `std-build-host-dependencies.json` and checked by
`tools/std_build_host_audit.py`. It currently contains 92 modules: broad facade
declarations make almost the entire std tree reachable from the build host,
including hash-map, math, and low-level Linux providers that build does not
conceptually require. This is not a claim that loader demand executes every
module; it is evidence that the current layer boundary cannot prove a narrow
host subset. Phase C must reduce both the declared closure and the observed
loader closure rather than stabilize this accidental breadth.

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
