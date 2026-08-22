# nia-build

`nia-build` owns the Rust side of `nia build`. It resolves one package
invocation, compiles and runs the package's `build.nia`, validates the emitted
plan, and executes the selected dependency closure. Package registries, version
solving, downloads, publication, and network trust are outside this crate.

The execution boundary is deliberately split:

```text
nia build
  -> resolve BuildInvocation and ToolchainLayout
  -> compile and run build.nia for the host
  -> std::build::Build validates and encodes a draft
  -> BuildPlan::decode validates and freezes the graph
  -> coordinator executes the selected closure
  -> typed Driver, linker, process, and filesystem operations
```

The generated runner is a configuration process. It can inspect its injected
package, toolchain, host, artifact-target, optimization, and requested-step
values and construct a graph through `std::build`; it cannot execute graph
actions. The decoded immutable `BuildPlan` is the coordinator's only execution
input.

## Source Ownership

- [`src/lib.rs`](src/lib.rs) owns package discovery, invocation paths, runner
  generation and execution, plan handoff, and top-level build diagnostics.
- [`src/runner_config.rs`](src/runner_config.rs) owns the private bounded
  configuration passed to the generated runner.
- [`src/plan.rs`](src/plan.rs) owns stable keys, typed logical paths, the
  immutable plan model, and semantic freeze validation.
- [`src/plan/actions.rs`](src/plan/actions.rs) owns action-local semantic
  validation, including typed artifact use and external command contracts.
- [`src/plan/dependencies.rs`](src/plan/dependencies.rs) owns producer and
  dependency closure validation for modules, artifacts, actions, and steps.
- [`src/plan/codec.rs`](src/plan/codec.rs) owns the registered binary plan
  protocol. Decoding always returns through semantic freeze.
- [`src/plan/handoff.rs`](src/plan/handoff.rs) owns durable canonical plan
  publication.
- [`src/coordinator.rs`](src/coordinator.rs) owns selected-closure scheduling
  and typed action execution.
- [`src/action_cache.rs`](src/action_cache.rs) and its child modules own
  build-action records and invalidation reasons. Compiler object, archive, and
  link products remain owned by `nia-driver` and `nia-linker` caches.
- [`src/output_recovery.rs`](src/output_recovery.rs) owns recoverable
  multi-output publication, while [`src/lock.rs`](src/lock.rs) owns
  cross-process coordination for equal logical outputs.
- [`src/resources.rs`](src/resources.rs) maps declared action resource classes
  onto inherited query-session capacity.

The public build-script API and its ownership rules live with the Nia sources
in [`lib/std/build.nia`](../../lib/std/build.nia) and
[`lib/std/build/`](../../lib/std/build/). Language-visible build behavior is
specified in [`docs/language-spec.md`](../../docs/language-spec.md); the outer
CLI and compiler boundary are described in
[`docs/architecture.md`](../../docs/architecture.md).

## Plan Contract

`BuildPlan` contains stable package, module, artifact, action, and step keys. A
logical path carries a typed package, build, cache, toolchain, or artifact root;
physical checkout and installation paths are resolved only for the current
invocation. The plan contains no callback, allocator, process handle, borrowed
runner storage, or raw compiler argument vector.

The std builder rejects foreign handles before encoding. `BuildPlan::freeze`
then canonicalizes graph order and rejects invalid names and paths, duplicate
identities, unresolved references, dependency cycles, conflicting output
ownership, target mismatches, and missing producer closure. Protocol decoding
repeats these semantic checks rather than trusting runner-produced bytes.

The Nia encoder bounds every collection count by the registered `maxItems`
limit before narrowing it to the wire `u32` representation. Aggregate counts
and derived dependency/input/output counts use checked addition, so malformed
in-memory state is rejected instead of wrapping into a smaller protocol value.
Generated-file payload lengths are likewise checked before their fixed-width
length prefix is written.

The Rust encoder applies the 64 MiB plan budget at each raw write. Once a write
would exceed that budget it retains the first `TooLarge` error and stops growing
the output buffer, rather than allocating the complete oversized encoding and
rejecting it only at `finish`.

The decoder does not reserve typed list capacity from a runner-controlled count.
It grows a list only after each item has consumed and validated its bytes, so a
truncated count prefix cannot amplify a tiny draft into `count * size_of(T)` of
host allocation.

The registered compatibility identities in `nia-compat` are the only schema
and namespace authorities. Changing an encoded field or persistent cache input
requires the corresponding registered schema or owner-local fingerprint domain
to advance.

## Execution And Publication

The coordinator executes deterministic readiness waves through `QuerySession`,
so build actions share the process jobserver and compiler resource budgets.
`Cpu` and `Io` actions reserve one action slot; `Conservative` actions reserve
the complete action capacity. `--jobs` can reduce ready-action parallelism but
does not create a separate executor or replace LLVM memory backpressure.

Actions publish only declared build-root outputs. Equal logical destinations
share a cross-process lock; unrelated destinations may proceed independently.
File and directory outputs use staged same-filesystem transactions. The
versioned journal and prepared marker make interrupted publication recoverable
without guessing which partial destination is valid. Corrupt or contradictory
recovery state is a typed failure.

Generated-file and eligible external-command entries store complete validated
payloads. Compiler-check entries record only a prior successful zero-diagnostic
check against a complete source manifest. Compiler-emit entries bind build
identity to a Driver-owned executable cache reference rather than duplicating
the executable. Cache I/O and corruption produce explicit misses and never
replace the ordinary correctness path.

## Verification

Focused owner tests should run before broad workspace gates. Tests that start a
compiler, LLVM backend, generated runner, nested build, or subprocess must use
the repository's resource-accounted harness and bounded process-tree cleanup.
The principal end-to-end contracts live in
[`nia-cli/tests/build_cases.rs`](../nia-cli/tests/build_cases.rs); the
representative clean/warm/edit/corruption workload lives under
[`benchmarks/build/`](../../benchmarks/build/) and is run through
`cargo maintain baseline build`. The maintenance command is documented in
[`maintain/README.md`](../../maintain/README.md).

Historical migration matrices, dated measurements, and hosted-run identifiers
belong in Git history and generated evidence, not in this maintained contract.
