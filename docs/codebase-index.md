# Nia Codebase Index

Status: maintainer navigation reference

This index maps repository paths to their architectural owners. It complements
`architecture.md`, which defines the contracts in detail. Update this file when
a crate changes responsibility or a new top-level subsystem is added; do not
use it as a second specification.

## 1. Main Data Flow

```text
source and toolchain layout
  -> loader -> lexer -> parser -> item/public surface
  -> definitions/imports -> types/signatures/traits -> values/locals
  -> const/static/layout/ABI -> body and flow checking
  -> function IR -> executable reachability -> monomorphization
  -> backend IR -> LLVM objects -> linker -> Driver/CLI

build.nia -> std::build mutable builder -> canonical BuildPlan bytes
  -> nia-build validation/coordinator -> typed Driver or process actions
```

The central orchestration owners are `nia-loader-query` for source/module
discovery, `nia-compiler-query` and `nia-query` for compiler products and
execution, `nia-driver` for top-level compiler requests, and `nia-build` for
frozen build-plan execution. Analysis crates do not load files or call later
backends directly.

## 2. Crate Ownership

| Area | Crates | Primary responsibility |
| --- | --- | --- |
| Foundation | `nia-span`, `nia-source`, `nia-node-id`, `nia-ids`, `nia-hash`, `nia-symbol`, `nia-symbol-table` | Source, syntax-node, semantic, and symbol identity |
| Configuration | `nia-compat`, `nia-target-config`, `nia-toolchain`, `nia-opt`, `nia-timing`, `nia-test-support` | Compatibility identities, targets, relocatable resources, optimization policy, telemetry, test resource limits |
| Diagnostics | `nia-diagnostic`, `nia-ice` | User diagnostics and the internal-error boundary |
| Syntax | `nia-lexer`, `nia-syntax`, `nia-ast`, `nia-ast-walk`, `nia-parser`, `nia-literals`, `nia-item-tree` | Tokens through stable declaration-oriented syntax products |
| Module surface | `nia-provider-summary`, `nia-public-surface`, `nia-defs`, `nia-imports` | Lazy providers, visibility, definitions, and import graphs |
| Query kernel | `nia-query`, `nia-compiler-query`, `nia-loader-query` | Scheduling, typed query storage, invalidation, module loading, persistent frontend products |
| Type/signature | `nia-ty`, `nia-type-resolve`, `nia-type-lower`, `nia-item-signatures`, `nia-program-signatures`, `nia-type-normalize`, `nia-trait-solve` | Type identity, declaration signatures, normalization, and trait selection |
| Name resolution | `nia-value-resolve`, `nia-local-resolve` | Global value paths and function-local bindings |
| Const/static/ABI | `nia-sema`, `nia-sema-ir`, `nia-const-ir`, `nia-const-eval`, `nia-const-check`, `nia-static-check`, `nia-static-ir`, `nia-layout`, `nia-abi-check` | Shared semantic checks, compile-time values, static storage, layout, and ABI validation |
| Pattern analysis | `nia-pattern-analysis` | Pure typed-pattern usefulness, exhaustiveness, scalar partitioning, and missing witnesses shared by const and runtime checks |
| Bodies and function IR | `nia-flow-check`, `nia-body-check`, `nia-body-ir`, `nia-closure-check`, `nia-function-ir`, `nia-function-lower`, `nia-function-opt`, `nia-ir-names` | Typed bodies, places/control flow, closure escape validation, lowering, and function-local optimization |
| Executable closure | `nia-executable-facts`, `nia-executable-reachability`, `nia-monomorphize`, `nia-mangle` | Reachable program facts, generic instances, and stable symbols |
| Backend | `nia-backend-ir`, `nia-backend-lower`, `nia-llvm`, `nia-codegen-llvm` | Backend ownership boundary, lowering, LLVM facade, objects and work products |
| Product surface | `nia-linker`, `nia-driver`, `nia-build`, `nia-cli` | Linking, typed compiler requests, build coordination/cache, and the `nia` binary |

## 3. High-Value Entry Points

- `crates/nia-cli/src/main.rs`: CLI parsing, toolchain resolution, command and
  ICE boundaries.
- `crates/nia-compat/src/lib.rs`: release, toolchain, ABI, persisted-format, and
  cache-namespace identity registry.
- `crates/nia-driver/src/pipeline.rs`: typed check/emit requests and the
  end-to-end compiler product pipeline.
- `crates/nia-loader-query/src/lib.rs`: loader facade, source manifests, and
  module/provider activation.
- `crates/nia-compiler-query/src/lib.rs`: session-owned compiler query facade.
- `crates/nia-build/src/plan.rs`: stable plan model and semantic freeze.
- `crates/nia-build/src/plan/codec.rs`: canonical versioned protocol.
- `crates/nia-build/src/coordinator.rs`: selected closure, scheduling, typed
  action execution, and diagnostics.
- `crates/nia-build/src/action_cache.rs`: generated-file cache and shared cache
  vocabulary; its submodules own compiler and external-command records.
- `crates/nia-build/src/output_recovery.rs`: journaled multi-output recovery.
- `crates/nia-pattern-analysis/src/lib.rs`: constructor-matrix usefulness,
  exhaustiveness, scalar-domain partitioning, and witness generation.
- `crates/nia-body-check/src/patterns/analysis.rs`: runtime typed-pattern and
  type-domain adapter for the shared matrix analysis.
- `crates/nia-const-check/src/analyzer/match_patterns/coverage.rs`: static
  const-match adapter and missing-witness formatting.
- `lib/std/build/core.nia`, `types.nia`, and `plan.nia`: public build-script API,
  owned mutable records, validation, and Nia-side plan encoding.
- `maintain/`: repository-local structural audits, crate reports, performance
  and build baselines, and their enforced fixtures.

## 4. Standard Library

`lib/std.nia` is the facade. The implementation is organized as:

- compiler contracts in `lib/std/builtin/`;
- allocation and memory in `lib/std/mem/`;
- collections in `lib/std/collections/`;
- text, parsing, formatting, hashing, and iteration in `string.nia`,
  `unicode.nia`, `parse.nia`, `parse/`, `fmt/`, `hash/`, and `iter/`;
- I/O, paths, files, and processes in `io/`, `fs/`, and `process/`;
- the platform facade and Linux implementation in `os.nia` and `os/linux/`;
- startup/runtime injection in `start/`;
- build-host declarations and protocol encoding in `build/`.

The durable facade, ownership, and API rules live beside the implementation in
[`lib/README.md`](../lib/README.md) and the owning `lib/std` modules. Broad `using std` spelling
is not evidence that all facade modules perform semantic or backend work;
loader/provider closure tests own that guarantee.

## 5. Tests And Workloads

- Unit tests normally live beside each owning crate. Large crates split them
  into `src/tests/` or query-specific test modules.
- `crates/nia-cli/tests/command_cases.rs` owns CLI syntax and diagnostic cases.
- `crates/nia-cli/tests/build_cases.rs` owns the production `build.nia` matrix.
- `crates/nia-cli/tests/std_*.rs` own standard-library compile/run and failure
  conformance.
- `crates/nia-driver/src/tests/` owns full compiler pipeline and persistent
  product behavior.
- `crates/nia-codegen-llvm/src/tests/` owns LLVM shape and runtime codegen.
- `examples/` contains maintained user-facing language idioms.
- `benchmarks/` contains repeatable performance and build-state workloads;
  `cargo maintain baseline ...` owns their maintained runners. They are not
  ordinary unit tests.

Compiler-, LLVM-, build-, and generated-process tests must use
`nia-test-support` resource accounting. Libtest remains the test scheduler;
the shared harness limits compiler and runtime process concurrency separately,
then applies one cross-process memory budget to both classes. Whole-test
sessions cover multi-command workloads, while command helpers classify isolated
compiler, build, and runtime processes. The normal local gate starts with
`cargo maintain check`, followed by `cargo fmt --check`, strict workspace
Clippy, and `cargo test --workspace`; focused owners should run first.

## 6. Change Routing

| Change | Start at | Also inspect |
| --- | --- | --- |
| Token or grammar | `nia-lexer`, `nia-parser`, `nia-ast` | language spec, item tree, syntax tests |
| Type or trait semantics | type/signature owner and `nia-body-check` | const evaluation, layout, backend lowering, language/ABI docs |
| Runtime representation | `nia-layout`, `nia-abi-check`, `nia-backend-ir` | monomorphization, LLVM, runtime tests, ABI reference |
| Incremental identity | query/product owner | source identity, invalidation reason, clean/warm equivalence |
| Compatibility identity | `nia-compat` | owning encoder/decoder, generated toolchain manifest, structural audit |
| Compiler command | `nia-driver`, then `nia-cli` | diagnostics, help, command cases |
| Build API/protocol | `lib/std/build/`, then `nia-build::plan` | codec, coordinator, cache identity, production build fixture |
| Build output/cache | `nia-build::coordinator` or `action_cache` | output locks, recovery journal, corruption and race tests |
| Standard-library API | owning `lib/std` layer | facade/provider isolation, allocator/error cleanup, examples |
| Toolchain layout | `nia-toolchain` | CLI, Driver, loader, cache domains, relocation tests |
| Repository audit or baseline | `maintain/` | owning fixture, workflow contract, affected compiler or std subsystem |

For architectural work, read `project-conventions.md`,
`compiler-maintenance.md`, the relevant crate README, and the implementation
facade before editing. One owner must remain responsible for each identity,
diagnostic, cache product, and execution policy.
