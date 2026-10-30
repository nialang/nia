# Standard Library Refactor Review

Status: active working checklist. Remove or archive after the file-by-file review is complete.
Scope: every `*.nia` source file under `lib/std/` (161 files at creation).

A checked item means the current file has received a read-only review of its behavior, error/ownership boundaries, Nia idioms, module responsibilities, and test coverage. Findings are recorded below with source locations; a checked item does not mean no follow-up is needed.

Potential language/compiler issues require a minimal reproducer and a contract comparison before a std workaround is considered. Implementations stay untouched during each review batch unless a confirmed correctness defect needs immediate repair.

## Findings

### H1 — Store SHA-256 round constants as protocol data

- Files: `lib/std/crypto/hash/sha256.nia`
- Evidence: `roundConstant` maps every index from 0 through 63 and returns zero for all other values; its only call is inside the 64-round compression loop.
- Direction: replace the dispatch match with a 64-entry `u32` constant table and direct indexed lookup. This makes the defined domain explicit and removes a fallback that could hide an invalid caller.
- Validation: existing FIPS vectors, multi-block input, streaming boundary sweep, and million-byte vector.
- Status: completed in `9f4fc81ba`.

### H2 — Preserve element-oriented serialization in wide integer hashing

- Files: `lib/std/hash/impls.nia`
- Evidence: the `u128` implementation writes exactly 16 little-endian bytes by indexing a fixed local array while shifting the value.
- Direction: iterate over the mutable destination bytes and assign each successive low byte. Keep the explicit width and endianness visible.
- Validation: existing wide positive and signed `i128` vectors.
- Status: completed in `9f4fc81ba`.

### H3 — Revisit Wyhash streaming length saturation

- Files: `lib/std/hash/wyhash.nia`, `crates/nia-cli/tests/std_hash_map.rs`
- Evidence: the official one-shot API accepts `size_t len` and mixes its conversion to `u64`. It defines no cumulative overflow policy. Zig std and `wyhash-final4` use ordinary `usize` addition; `wyhash-rs` v1 uses ordinary `u64` addition, but its write-boundary semantics differ. None of the inspected implementations saturates; their ordinary additions do not establish an explicit, mode-independent overflow contract either. See the source comparison below.
- Result: Nia explicitly defines cumulative length modulo 2^64, matching the word mixed into finalization and keeping streams independent of pointer width. A separate monotonic `longInput` flag prevents wrapped lengths from re-entering the short-input path. Saturating to `usize::MAX` is removed rather than retained as an alternative. The chunk loop compares remaining bytes before adding its bounded chunk size.
- Validation: an executable test seeds the actual implementation near `u64::MAX`, then checks 66 tail lengths against an independent one-shot final4 reference with the final length word overridden. It also checks repeated finish, continuation, 25 wrapping-add boundary pairs, and accumulation across 2^32. Ordinary vectors, hash-map, and string tests pass. Follow-up runtime experiments execute real 4 GiB-plus streams on Windows x64 and Linux i686, comparing 15 checkpoints and two chunk schedules against official final4 C one-shot results. The i686 runs pass at `-O0` and `-O2`, as does the seeded overflow source. See [runtime validation](wyhash-runtime-validation.md) for reproducible commands, observations, and the remaining seeded-only 2^64 boundary.
- Status: completed; fixed-width contract and overflow regression supersede the initial machine-word implementation in `21646b4fe`.

Source comparison inspected on 2026-10-30:

| Implementation | Length handling | What the source establishes |
| --- | --- | --- |
| [Official C](https://github.com/wangyi-fudan/wyhash/blob/e4764a0b637d34d3421a7760affada9288b625a8/wyhash.h) | One-shot `size_t`, converted through final `u64` mixing | No streaming overflow contract; current default secrets differ from Nia's final4 vectors |
| [Zig std](https://github.com/ziglang/zig/blob/738d2be9d6b6ef3ff3559130c05159ef53336224/lib/std/hash/wyhash.zig) | `total_len: usize`, ordinary `+=` | No saturation or explicit wrapping; checked modes can reject overflow; length also controls the short branch |
| [Rust wyhash-final4](https://github.com/thynson/wyhash-final4/blob/b1d06c5337cf2c41483664997912f6b9b4407ea2/src/generics.rs) | `StreamedWyHasher.len: usize`, ordinary `+=`, cast to `u64` at finish | No saturation; overflow behavior depends on Rust overflow checks; length also controls the short branch |
| [Rust wyhash-rs v1](https://github.com/eldruin/wyhash-rs/blob/acb4e4f5bbd2ba2216ac8ca1a6309513d251db5f/src/v1/traits.rs) | `size: u64`, ordinary `+=` | Fixed algorithm width, no saturation; a different algorithm version and write-boundary contract, not a final4 streaming oracle |

This is a source survey, not evidence that production workloads exercise overflow. Modulo 2^64 is Nia's explicit extension for unbounded streaming, not a promise retroactively attributed to the official one-shot algorithm. A 32-bit `size_t` limits a single input's addressable size; it need not limit cumulative input across calls.

### H4 — Express iterator terminal consumers with `for`

- Files: `lib/std/iter.nia`
- Evidence: `count`, `forEach`, `fold`, `tryFold`, and `position` each manually repeat `while true` plus `next()` optional-state handling.
- Direction: iterate the owned iterator directly with `for`; keep accumulator updates, callback borrowing, `.?` propagation, and early return in each consumer.
- Validation: `std_iter` covers borrowed/capturing callbacks, lazy map, early error stop, position, and custom iterator implementations; `std_slice` covers split counts.
- Status: completed; iterator test suites pass.

### H5 — Consider a focused slice sorting module

- Files: `lib/std/slice.nia` (724 lines)
- Evidence: the file contains general slice access/search/split APIs, iterator implementations, and a self-contained stable rotation merge plus unstable heap sort.
- Direction: consider moving sort helpers and sort methods behind a dedicated provider module after confirming provider-loading and visibility behavior. Indexing in comparison/sort/search remains intentional because it represents positions and bounds.
- Result: moved sorting/search helpers and the comparator/`Ord` extensions to `lib/std/slice/order.nia`; `slice.nia` retains slice access, splitting, and iteration. The explicit sibling import keeps the existing `std::slice` extension scope intact.
- Validation: with `first`, `order.nia` can appear as a candidate in the compiler graph but is absent from the final loaded modules and from semantic, body, and backend work. With `sort`, it is loaded and enters all three work sets. Both namespace and aliased imports produce the same work sets in each case.
- Status: completed as a source and lazy-loading split; the earlier claim that selecting `std::slice` eagerly loads the ordering provider was incorrect and has been withdrawn.

### H6 — Share integer presentation decoding

- Files: `lib/std/fmt/primitive.nia`
- Evidence: signed and unsigned formatting repeat the same five presentation-to-radix branches, differing only in the writer operation.
- Result: decode the radix once through a private helper; keep signed/unsigned flag validation and output dispatch at their call sites. Uppercase remains an explicit property of `UpperHex`. Unsupported and unknown presentations still fail before output.
- Validation: all 18 `emit_exe_io` tests pass, including primitive radices, sign/prefix padding, pointers, template errors, and invalid open-enum values.
- Status: completed.

### H7 — Use a bit-mask power-of-two predicate

- Files: `lib/std/math/alignment.nia`
- Evidence: `isPowerOfTwo` divided a nonzero `usize` repeatedly by two until it reached one. The result only depends on whether the value has exactly one set bit, so the loop obscured the invariant and made the common alignment check proportional to the integer width.
- Result: keep the zero guard and use the standard `value & (value - 1) == 0` predicate. This stays within Nia's checked arithmetic because the subtraction is performed only after the zero case is excluded.
- Validation: `emit_exe_can_use_std_math_usize_helpers` now exercises zero, one, a large power of two, the maximum non-power-of-two value, and alignment overflow. The existing checked integer helper test still passes.
- Status: completed.

### H8 — Keep checked arithmetic contracts explicit

- Files: `lib/std/math/checked.nia`
- Evidence: the seven traits intentionally separate operations and expose an associated `Output`, allowing generic callers to require one operation and implementations to choose the result type.
- Result: document this contract at the trait boundary; merging the methods would make otherwise independent operations mandatory together.
- Validation: the checked integer executable test now also exercises generic `CheckedNeg` and `CheckedAbs` dispatch for every signed primitive and the `MIN % -1` boundary.
- Status: completed as a documentation review; no compiler or standard-library correctness bug confirmed.

### H9 — Separate external command policy from owned command storage

- Files: `lib/std/build/commands.nia` (552 lines)
- Evidence: the module combines typed argument constructors, cache/environment/command validation, value-style option builders, borrowed-to-owned conversion, and retryable allocator cleanup for retained command records.
- Direction: review the build sibling modules and public provider closure before splitting these responsibilities. Preserve the existing `ExternalCommandOptions` method scope and the validation entry points used by `Build`.
- Validation: existing command-cache, build-target, ownership-failure, configured-build, and invalid-diagnostic tests cover the behavior that a later split must retain.
- Status: review complete; structural split queued for the build-module refactor pass.

### H10 — Keep logical build paths free of host-specific syntax

- Files: `lib/std/build/paths.nia`, `lib/std/build/plan.nia`, `crates/nia-build-plan/src/lib.rs`
- Evidence: both path validators accepted `:`. On Windows, joining a logical component such as `C:outside` can replace the typed root with a drive-relative path; colons also select alternate data streams. Local package roots are joined component by component, so std-only validation would leave the protocol boundary exposed to hand-built or decoded plans.
- Result: reject colons in the std build-path API, plan encoder, and canonical `LogicalPath` constructor, and document the portable path contract. This is a build path-model defect, not a compiler or lazy-loading defect.
- Validation: all 84 `nia-build-plan` unit tests and the Nia build-target conformance test pass, including `C:/outside` and `result.txt:stream` rejection.
- Status: completed.

### H11 — Separate artifact registration from step orchestration

- Files: `lib/std/build/core.nia`, `lib/std/build/artifacts.nia`
- Evidence: `core.nia` combined package and module declarations, all step constructors, executable/object/archive registration, target lookup, and complete-graph validation. Artifact registration is a cohesive state transition from borrowed options to pending owned records and uses validation/query providers already separated from `Build` mutation.
- Result: moved target views and lookup plus executable/object/static-archive registration, including pending-record cleanup, into `artifacts.nia`. `core.nia` retains package/module declarations, manifest orchestration, and dependency-graph validation; step APIs live in `step_registration.nia`.
- Validation: build-target, ownership rollback, build-case, and command-cache integration tests pass; no compiler defect or behavior change was found.
- Status: completed.

### H12 — Separate step registration from graph validation

- Files: `lib/std/build/core.nia`, `lib/std/build/step_registration.nia`
- Evidence: `core.nia` combined step constructors, producer lookup and edge insertion, retryable pending-step cleanup, package/module declarations, and graph validation. Step registration is a cohesive mutation boundary; complete graph validation belongs beside its own scratch-state lifecycle.
- Result: moved public step APIs and their pending-record/rollback helpers into `step_registration.nia`; `core.nia` now retains package and module declarations, manifest orchestration, lookups, and whole-graph validation. The facade and lifecycle provider import registration explicitly.
- Validation: build-target and ownership rollback tests, all six test-step tests, all 20 build-case tests, and formatting checks pass. The first post-split test run caught missing explicit module imports; those compile-time integration issues were fixed before the passing rerun.
- Status: completed.

### H13 — Enforce probe record bounds at the shared append boundary

- Files: `lib/std/build/probes.nia`, `crates/nia-cli/tests/std_build_probes.rs`
- Evidence: most probe entry points checked `maxObservationRecords` locally, but `probeGitState` and `probeTool` delegated directly to `runProbe`; the common `appendObservationRecord` accepted another record unconditionally. The byte cap still limited storage, but the advertised record-count bound was not a global invariant.
- Result: enforce the count limit immediately before every observation append and strengthen the file-presence test to assert the `absent`/`present` step names. During review, `!(state == 1)` was confirmed to be Nia's success constructor around a boolean expression, so it was retained rather than misread as logical negation.
- Validation: all nine `std_build_probes` tests pass, including command/tool/git probes and the strengthened presence semantics.
- Status: completed.

### H14 — Keep draft envelope limits aligned with the host decoder

- Files: `lib/std/build/plan.nia`, `crates/nia-build-plan/src/observations.rs`
- Evidence: the runner bounded plan bytes and observation bytes independently, but the host's `MAX_DRAFT_BYTES` includes the 48-byte envelope header. A plan and observation payload at both advertised maxima would therefore be emitted above the host limit.
- Result: check the combined payload against the complete 80 MiB draft budget before allocating and publishing the envelope. The Nia build model intentionally uses one name for a step and its co-located action; `writeStep` now documents that valid subset of the host model.
- Validation: build-plan codec and Nia build-target/plan integration suites pass; no compiler or lazy-loading defect was involved.
- Status: completed.

### H15 — Keep build record initialization and handle domains explicit

- Files: `lib/std/build/steps.nia`, `lib/std/build/targets.nia`, `lib/std/build/types.nia`, `lib/std/build/validation.nia`
- Evidence: step payload initialization repeated the same owned-name transition in every variant; target option builders intentionally remain value-style because each artifact kind has a different valid field set. Package handles use index `0` for the root and `1..count` for retained dependencies, while all other handles use `0..count-1`.
- Result: route every step payload initializer through the existing `initializeName` boundary. Keep the target option copies and validation split explicit; collapsing them would either expose invalid fields or erase the package/root index distinction.
- Validation: build-target, build-case, test-step, ownership, and plan integration suites pass. No compiler or handle-domain defect was confirmed.
- Status: completed.

### H16 — Preserve nested artifact configuration owners on cleanup failure

- Files: `lib/std/build/targets.nia`, `lib/std/build/records_cleanup.nia`, `lib/std/build/lifecycle.nia`, `lib/std/build.nia`, `crates/nia-cli/tests/std_build_ownership.rs`
- Evidence: all three artifact record destructors released the defines/system-library list backing even if a nested string failed to release, making that owner unreachable on retry. A new fault-injection regression reproduced a residual allocation after the second `deinit`. The previous tests did not populate these lists. The fixture also exposed missing public facade exports for the documented configuration types.
- Result: generalize the existing record cleanup mechanism into `OwnedBuildValue` and `releaseOwnedValues`, carrying error subject context through nested values. Artifact configuration lists now use the same reverse-order, retain-on-failure rule as Build's record tables. Remove the old trait/helper names and export the seven public configuration types through `std::build`.
- Validation: for executable, object, and static-archive records, inject one through four consecutive release failures with two defines and two system libraries; a retry must release every allocation. This test failed with exit code 61 before the cleanup fix and passes afterwards. Existing rollback scenarios remain covered.
- Status: completed; supersedes H15's initial conclusion that target record teardown had no defect.

### Formatting review conclusions

- Reviewed the facade and all six formatting providers. The existing separation between contracts, specifications, byte/text output, integer rendering, primitive implementations, and template parsing is cohesive.
- Keep the open-enum validation matches in `FormatSpec::validate` and the alignment output matches: they enumerate supported states or select distinct effects. They are not optional/result plumbing.
- Keep the template parser's indices: they represent character positions and argument-consumption order. Its decimal accumulation already composes checked operations with `andThen` and converts the error once.
- No compiler defect was confirmed in this batch. Remaining counter-based output loops are candidates for a later iterator-focused pass; no further provider split is justified by this review.

### Math review conclusions

- `math.nia` is a clear facade over alignment, checked-operation contracts, and signed/unsigned providers. Keep the concrete arithmetic per primitive: its overflow predicates depend on that primitive's bounds, and factoring them would hide the proof immediately before the trapping operator.
- The unsigned provider imported `CheckedNeg` and `CheckedAbs` without implementing or using either trait; those names were removed from its import group.
- Checked signed operations explicitly cover every trapping case before evaluating the operator. Their formulas matched exhaustive `i8` operand pairs and representative boundaries for wider widths; no arithmetic defect was confirmed.
- Added executable checks for `isPowerOfTwo` and the least-tested `CheckedNeg`, `CheckedAbs`, and `MIN % -1` boundaries.

### Build dependency and query review conclusions

- `Step.index` is assigned from `steps.len()` before registration, survives append unchanged, and rollback only removes the final step. Generated-source producer lookup can use this canonical identity directly instead of maintaining a second position counter.
- Keep the executable, object, and static-archive emit lookups explicit: each validates a distinct `StepKind` and reads a different payload variant. Combining them would weaken the type boundary without removing meaningful policy duplication.
- Name lookups are simple typed scans over different record lists; callback-based factoring would add abstraction without clarifying behavior. Dependency membership is likewise a direct directed-edge query.
- Generated-source producer lookup excludes the consumer, accepts generated-file and external-command build outputs, and returns no producer for both missing and ambiguous matches. Existing build-plan tests cover the downstream producer closure; the std integration test covers the missing-producer path. No compiler or lazy-loading issue was observed.
- Simplified the consumer exclusion to compare `step.index` directly. Targeted build-target tests and formatting checks pass.
- Status: review complete.

### Build diagnostics, error, and handle review conclusions

- Keep the two diagnostic entry points separate: one formats a structured `Error`, the other reports a requested step name directly. Their shared writer setup is small, while the buffer sizes and context labels are meaningfully different.
- Split the large error provider at its presentation boundary. `error.nia` now owns error types, operation context, and exit-code policy; `error_format.nia` owns `Format` implementations and nested cause text. The build facade explicitly imports the formatter provider so the public `Error` retains its formatting behavior.
- Keep variant matches in the formatter: they translate open OS and library error enums to stable diagnostic text, and fallback arms intentionally cover unknown values. Similar-looking system-error mappings consume distinct Nia enum types and should remain explicit.
- Handle accessors repeat a tiny shape across nominal capability types; a generic trait would add an import and abstraction burden without simplifying their typed use. `BuildInitAttempt::finish` correctly preserves the primary initialization error while keeping cleanup retryable.
- The build-host closure snapshot was already missing the artifact and step-registration modules added by the preceding split; it now includes those existing providers and the new formatter provider. Error diagnostics and build-target integration tests pass; no compiler defect was found.
- Status: review complete.

### Build lifecycle, modules, and ownership review conclusions

- `Build::init` keeps partial ownership inside `BuildInitAttempt`; `finish` preserves the primary initialization error and leaves cleanup failures retryable. `Build::deinit` attempts every scratch, pending, retained, and path release, while record-list cleanup runs in reverse order.
- Module options distinguish package-rooted and build-rooted paths explicitly. Import validation checks stable names, build-path validity, and duplicate aliases before the retained module is published; position counters also provide precise failing-import subjects.
- Found an owner-identity exhaustion race: `fetch-add` could wrap `usize::MAX` while a delayed caller that had already observed the separate exhaustion flag still entered the allocator, allowing an old ID to be reused. Replaced the split state with a strong compare-exchange loop and a permanent `usize::MAX` sentinel, so allocation stops before wraparound and contending callers retry from the observed counter value.
- Validation: build ownership and target-isolation integration tests pass; the atomic executable test confirms the compare-exchange optional-result contract. No compiler change was needed.
- Status: review complete.

### Build record cleanup review conclusions

- Seven typed cleanup functions duplicated the same reverse-order traversal and the invariant that list storage is released only after every element succeeds. They now use `releaseOwnedValues` and the internal `OwnedBuildValue` capability; each record type keeps its own deinitializer and error context. The same mechanism protects nested artifact configuration lists (H16).
- Kept the collection subject as an explicit argument so collection-release failures retain their precise diagnostics. The generic element bound explicitly includes `Sized`, which is required by `ArrayList[T]`; the compiler diagnostic identified the missing contract, not a compiler defect.
- Failure-injection ownership, build-target, all 20 build-case, and all six test-step tests pass. Formatting and repository audits pass.
- Status: review complete.

### Review conclusions

- The earlier exploratory suggestions to extract `BlockBuffer` and unify SipHash variants are already implemented.
- Keep index-based loops where an index is the algorithm's word, lane, or serialization position; replace only cases where iteration expresses intent more directly.
- Checked widening and narrowing in hash arithmetic are necessary under Nia's trapping integer semantics and remain explicit reference code.

### H17 ? Resolve foreign Sized facts independently of local layout roots

- Trigger: `nia-driver::tests::builtin_traits::len_needs_no_import` failed even on the earlier `468212d8d` baseline. That comparison established provenance, not a resolution.
- Root cause: trait solving treated the current module's layout roots as a complete account of sized types. Types reached only through another module's slice/pointer fields need not have a local layout root. This falsely rejected `Sized` prerequisites, removed entire `ArrayList` method groups from selection, and cascaded into unrelated std diagnostics.
- Result: the shared solver context now accepts the existing lazy module-layout provider. When local evidence is absent, nominal sizedness queries the defining module using the complete type and const argument identity. Body constraints, associated type/const projection, and trait-object coercion all supply this capability. Const evaluation supplies its existing layout producer through the same solver input, including nested `Sized` prerequisites during associated-item resolution. Layout computation still owns representation validity; no nominal declaration is presumed sized and no std import or bound is added to hide the error.
- Validation: the original test passes. A separate two-module regression covers direct bounds, nested impl prerequisites, associated type/const projection and trait objects in all-module and entry analysis and code generation (the associated constant is evaluated in a const declaration). A temporary Windows x64 executable checks the combined result at `-O0` and `-O2`. Solver tests cover owner-only queries, both solver constructors, const instance identity, open/missing instances, local ID collisions, and unsized pointees.
- Status: completed in `2437ce0bf`; validation and independent baseline failures are recorded in [Sized/provider validation](sized-provider-validation.md).

### H18 ? Dispatch generic intrinsic calls after resolving the signature

- Trigger: the H17 code-generation regression exposed a second error when `std::builtin::size` was imported under a bare name: the call checked successfully but produced an error expression during function lowering.
- Root cause: early generic-call paths for bare identifiers and existing function references instantiated the signature as an ordinary function. Only later syntax-specific branches recognized builtin attributes, so intrinsic value facts were missing.
- Result: builtin dispatch now lives at the common instantiated-signature call entry. The redundant later branch checks are removed, and a focused code-generation regression uses renamed `size` and `align` imports inside a generic function.
- Status: completed in `7a2ac4032`; covered by the H17 executable probe as well as the dedicated regression.

### H19 ? Materialize runtime associated-constant projections

- Trigger: a raw runtime projection such as `[Payload as Project]::Tag` and a
  generic projection such as `[Boxed[T] as PairProject]::Value` reached backend
  lowering without a complete value path. Deferred generic initializers also
  failed when they called a foreign builtin signature.
- Root cause: the projection was preserved too late for concrete values and the
  backend had no substitution-aware evaluator for deferred associated-constant
  initializers. Foreign function signatures were also incomplete in that
  evaluator, so `size[T]()` was rejected as non-const.
- Result: Body IR and Function IR preserve the projection through instantiation;
  backend lowering evaluates the selected initializer with its impl substitutions,
  merges ordinary and extension function signatures, and recursively materializes
  scalar, tuple, array, vector, struct, enum, union-storage, optional,
  error-union, range, and frozen array-pointer values. The instantiated
  regression bodies assert that
  scalar and aggregate projections contain no residual projection nodes.
- Validation: the focused regression, 130 backend-lower tests, 304
  compiler-query tests, and 748 driver tests (746 passed, 2 ignored) pass.
- Status: completed in the current repair batch.

### H20 - Keep set iteration types aligned with custom hash contexts

- Files: `lib/std/collections/hash_set.nia`, `lib/std/collections/tree_set.nia`, `lib/std/collections.nia`, `crates/nia-cli/tests/std_hash_set.rs`
- Evidence: `HashSetWithContext[K, C]` exposed its iterator only through the underlying hash-map provider, while the public `HashSetIter[K]` alias hard-coded the default context. Generic callers could iterate a custom-context set, but could not name the corresponding iterator through the set facade. Set insertion and removal also used single-arm pattern matches for optional replacement values, and `TreeSet` repeated the same optional-state plumbing.
- Result: add `HashSetIterWithContext[K, C]` and make `HashSetIter[K]` its default-context alias; use the alias in the `Iterable` implementation and re-export it from `std::collections`. Express insertion and removal results through `Option::isNull` and `Option::isPresent`, with the option provider imported explicitly by `TreeSet`. The public facade now preserves the context parameter instead of leaking the hash-map implementation detail.
- Validation: the hash-set executable regression now creates a custom-context `HashSetWithContext`, iterates it through the public `Iterable` contract, and verifies both elements. Existing hash-set and tree-map/tree-set executable tests pass.
- Status: completed in the current std collections batch.

### Collections array-list review

- Reviewed `lib/std/collections/array_list.nia` and its `list`, `raw`, and `format` providers. The split follows actual ownership boundaries: the facade exports the list type, `list.nia` owns allocation and mutation, `raw.nia` contains the two pointer-to-slice primitives, and `format.nia` supplies the optional formatting capability.
- Keep the explicit allocator and cleanup state in `list.nia`. `storageBlock`, `replacementBlock`, and `aliasBlock` represent distinct owners needed for retryable failure paths; collapsing them into capacity-derived state would reintroduce the ownership defects already covered by the fault-injection tests.
- Keep the checked arithmetic and trap matches in assume-capacity helpers. Their contracts are infallible only after the caller has established capacity, so the match makes impossible overflow and capacity violations trap at the unsafe boundary rather than silently changing the list state.
- The existing array-list executable suites cover growth, aliasing, shrink/replace rollback, zero-sized elements, owned-slice transfer, cleanup retries, and allocator integration. No compiler or standard-library correctness defect was confirmed in this review.
- Status: review complete; no source change required.

### H21 - Use optional predicates for presence-only branches

- Files: `lib/std/collections/tree_map.nia`, `lib/std/build/probes.nia`
- Evidence: `TreeMap::containsKey` and build-environment probe encoding converted an optional value to a boolean with a two-arm `if` that carried no branch-specific work.
- Result: use `Option::isPresent()` directly. The change preserves the distinction between presence and payload extraction while making the intent visible and keeping the option provider explicit at both call sites.
- Validation: all three `std_tree_map` tests and all nine `std_build_probes` tests pass.
- Status: completed.

### H22 - Propagate checked length arithmetic through the option API

- Files: `lib/std/cstring.nia`, `lib/std/string.nia`
- Evidence: constructors and text-composition helpers converted every `checkedAdd`/`checkedMul` result with a three-arm match even though the null arm always produced the same allocation error. This obscured the arithmetic contract and repeated error plumbing in code users are likely to study.
- Result: import the option provider and use `okOr(mem::Error::OutOfMemory).?`; the existing `IntoError` implementations preserve the public `CStringBuildError`, `TextError`, and `TextFormatError` domains at the function boundary. The checked operation remains adjacent to the conversion, so overflow behavior stays explicit.
- Validation: the CString/process integration executable and all three `std_string` tests pass.
- Status: completed.

### H23 - Keep path arithmetic in the option combinator path

- Files: `lib/std/fs/path.nia`
- Evidence: path encoding, component joining, and alias-range validation repeated the same checked arithmetic matches even though each null arm mapped directly to the enclosing path or memory error.
- Result: import the option provider and use `okOr(...).?` for the conversion. The alias check still returns an optional offset for the valid/invalid range distinction, while arithmetic overflow remains a typed `mem::Error::Invalid`.
- Validation: all 21 `emit_exe_fs` tests pass, including bounded path encoding, relative-path validation, owned path construction, and invalid-path cases.
- Status: completed.

### H24 - Preserve the established limit after a bounded read

- Files: `lib/std/io/limit.nia`, `lib/std/io/fixed_buffer.nia`
- Evidence: `LimitedReader::read` re-probed its optional limit after the wrapped reader returned, even though the limited branch had already established the same state and no operation could change it. That nested match added noise around the key invariant `n <= count <= remaining`.
- Result: update the limit directly from the branch's `remaining` binding after validating the wrapped reader's transfer count. Keep `DiscardingWriter`'s saturating length behavior and fixed-buffer subtraction checks explicit because they define their adapter contracts.
- Validation: all 18 `emit_exe_io` tests pass, including limited-reader, invalid-transfer, partial-read, and fixed-buffer cases.
- Status: completed.

### H25 - Express environment lookup as iterator consumption

- Files: `lib/std/process/env.nia`, `lib/std/process/args.nia`
- Evidence: `Env::find` manually spelled the iterator terminal loop with `loop` and a `match` even though the iterator already satisfies the standard `Iterator` contract. Argument and environment value wrappers otherwise share a deliberate borrowed-view boundary and parsing delegation.
- Result: use a `for` consumer for environment lookup, preserving early return on the first matching name and the existing empty/missing entry behavior. Keep `Args`'s explicit `program` and `skipProgram` entry points because they describe process-specific ownership of argv[0].
- Validation: both process argument/environment executable tests pass.
- Status: completed.

### H26 - Use the option predicate for hash-map query presence

- Files: `lib/std/collections/hash_map/map.nia`
- Evidence: `containsKeyBy` converted the result of a generic query lookup into a boolean with an empty two-arm match. The query path already returns an optional bucket index and the option provider is part of the map's explicit imports.
- Result: return `isPresent()` directly, preserving the lookup context and avoiding a branch that discarded the index in both arms.
- Validation: all nine non-ignored `std_hash_map` tests pass; the 8 GiB streaming test remains intentionally ignored.
- Status: completed as a focused map review item; the remaining hash-map providers stay queued for the full module review.

### H27 - Express cleanup accumulator state through Option

- Files: `lib/std/error/cleanup.nia`
- Evidence: `CleanupAccumulator::isClean` converted the optional first failure to a boolean with a branch that did no work in either arm.
- Result: import the option provider and use `isNull()` directly. Failure capture and first-failure ordering remain explicit in `attempt` and `finish` because they carry ownership and error precedence semantics.
- Validation: all nine `std_error` tests and the build ownership rollback test pass.
- Status: completed.

### H28 - Compose process command byte lengths with checked Option results

- Files: `lib/std/process/command.nia`
- Evidence: command argv and environment staging repeated the same checked UTF-8 length matches, mapping every overflow to `Error::Allocation(OutOfMemory)` even though `mem::Error` already has the enclosing `IntoError[Error]` conversion.
- Result: import the option provider and use `okOr(mem::Error::OutOfMemory).?` in the shared text and environment append paths. Validation of NULs, duplicate names, and staging order remains separate from arithmetic propagation.
- Validation: all 26 `emit_exe_process` tests pass.
- Status: completed as a focused command-provider cleanup; the remaining process providers stay queued for full review.

### H29 - Compose Windows environment block sizing with checked Option results

- Files: `lib/std/os/windows/command.nia`
- Evidence: environment block sizing repeated compact checked-arithmetic matches for UTF-8 entry length, cumulative capacity, and UTF-16 allocation size. Every null arm mapped directly to the same `Errno::TooLong` error.
- Result: import the option provider and use `okOr(Errno::TooLong).?` at each sizing boundary. The environment pointer walk and command-line quoting loops remain explicit because they encode Windows ABI rules.
- Validation: the three Windows process executable tests covering spawn environment, pipe ownership, and termination semantics pass.
- Status: completed as a focused platform-provider cleanup; the remaining Windows providers stay queued for full review.

### H30 - Keep fixed-buffer allocation arithmetic in the checked Option path

- Files: `lib/std/mem/fixed_buffer_allocator.nia`
- Evidence: the allocation path expanded three checked arithmetic results into identical matches whose only failure action was `Error::OutOfMemory`. The ownership predicates in `ownsBlock` and `isLastAllocation` still need boolean fallback handling and remain explicit.
- Result: import the option provider and use `okOr(Error::OutOfMemory).?` for current-address, alignment, and end-offset calculation. The allocator still commits `endIndex` only after the complete bounds check succeeds.
- Validation: all four `std_mem_allocator_core` tests and all 14 `std_mem_allocators` tests pass.
- Status: completed as a focused allocator cleanup; the remaining memory providers stay queued for full review.

### H31 - Compose arena chunk sizing through the checked Option API

- Files: `lib/std/mem/arena_allocator.nia`
- Evidence: chunk allocation and minimum payload sizing expanded checked additions into matches whose only failure action was `Error::OutOfMemory`. The other matches in the provider either represent saturating capacity accounting or convert an optional address into a boolean ownership decision.
- Result: use `okOr(Error::OutOfMemory).?` at the two error-returning sizing boundaries while retaining saturating list totals and pointer-range checks as explicit invariants.
- Validation: all four `std_mem_allocator_core` tests and all 14 `std_mem_allocators` tests pass.
- Status: completed.

### H32 - Compose large-allocation header sizing through checked Option results

- Files: `lib/std/mem/general_purpose_allocator.nia`
- Evidence: the large-allocation path expanded the header-plus-alignment and total backing-size checks into matches whose only failure action was `Error::OutOfMemory`. The later address checks intentionally retain explicit matches because a failed alignment calculation must first retain the unpublished backing block for retry.
- Result: use `okOr(Error::OutOfMemory).?` for the pure sizing steps while preserving the rollback-aware address validation and owner retention path.
- Validation: all 14 `std_mem_allocators` tests pass, including small and large allocations, over-aligned reallocations, wrapped-header rejection, invalid free/resize, and owner-class cleanup retries.
- Status: completed as a focused general-purpose allocator cleanup; the remaining provider review stays queued.

### H33 - Keep platform page mapping length checks uniform

- Files: `lib/std/os/linux/mem.nia`, `lib/std/os/macos.nia`, `lib/std/os/windows/mem.nia`
- Evidence: each platform's page mapper performed the same checked addition of the page-aligned length and over-alignment slack, mapping overflow directly to its platform `OutOfRange` error through a local match.
- Result: use the existing option provider and `okOr(Errno::OutOfRange).?` consistently across Linux, Darwin, and Windows. Mapping-address validation and rollback unmapping remain explicit because those paths retain an already acquired OS mapping before returning the error.
- Validation: both maintained-target cross-compilation tests pass, as do the three Windows process executable tests.
- Status: completed as a focused platform memory cleanup; the remaining OS providers stay queued for full review.

### Memory allocator contract review

- Reviewed `lib/std/mem/allocator.nia` and `lib/std/mem/page_allocator.nia`. The allocator trait keeps remapping separate from allocation and preserves each block's release range through logical layout changes. `realloc` makes the old-free/replacement-free rollback state explicit and does not hide either owner after a failed cleanup.
- `SliceAllocation` is the typed owner boundary for allocated slices; its borrowed views never reconstruct ownership, and `deinit` resets the owner only after the exact block is released. `PageAllocator` follows the same empty-block and release-range rules while mapping only the OS errors that belong to the memory facade.
- No arithmetic, ownership, compiler, or provider-loading defect was confirmed. No source change was required in this review.
- Status: review complete.

### Memory layout and ownership boundary review

- Reviewed `lib/std/mem/allocation.nia`, `lib/std/mem/error.nia`, `lib/std/mem/layout.nia`, and `lib/std/mem/types.nia`. `LayoutBuilder` is already the single checked composition boundary: `checkedAppend` computes alignment and size before mutating the builder, and failed appends leave it unchanged.
- `Allocated`, `CallableAllocation`, and `Block` retain the allocator's actual release pointer and release range instead of reconstructing them from typed views. The sentinel state after ownership transfer is deliberate and keeps repeated cleanup from freeing the same allocation.
- `Block::asSlice` handles zero-sized elements before division; `Block::empty` preserves the layout alignment for empty blocks. No arithmetic, ownership, compiler, or provider-loading defect was confirmed in this review.
- Status: review complete; only the fixed-buffer allocation arithmetic cleanup in H30 required source changes.

### Filesystem provider review

- Reviewed `lib/std/fs/convert.nia`, `lib/std/fs/file.nia`, `lib/std/fs/options.nia`, and `lib/std/fs/types.nia` together with the filesystem executable suite. `convert.nia` keeps explicit enum translation and error-context wrappers; its fallback arms preserve forward compatibility for open OS enums. `options.nia` is a focused value-constructor provider with validation at the OS boundary.
- `types.nia` keeps descriptor ownership and invalid-handle transitions in the capability types. `borrowHandle`/`takeHandle` are intentionally separate: close consumes the handle before the syscall so a close failure cannot trigger a second close. `DirIterator` retains the borrowed directory and reports per-entry errors without losing the iterator close path.
- `file.nia` keeps path encoding on the caller stack and applies the operation context at every syscall boundary. Parent splitting is shared between delete/create/rename paths and preserves the lexical relative-path contract; no safe abstraction would remove the meaningful per-operation parameters.
- Existing filesystem tests cover descriptor cleanup, path bounds, symlink policy, rename/delete containment, metadata, directory iteration, and allocator-rejection APIs. No compiler or standard-library correctness defect was confirmed in this review.
- Status: review complete; only the arithmetic cleanup in H23 required source changes.

## File checklist

### `build`

- [x] `lib/std/build/commands.nia`
- [x] `lib/std/build/core.nia`
- [x] `lib/std/build/artifacts.nia`
- [x] `lib/std/build/step_registration.nia`
- [x] `lib/std/build/dependencies.nia`
- [x] `lib/std/build/diagnostics.nia`
- [x] `lib/std/build/error.nia`
- [x] `lib/std/build/error_format.nia`
- [x] `lib/std/build/handles.nia`
- [x] `lib/std/build/lifecycle.nia`
- [x] `lib/std/build/modules.nia`
- [x] `lib/std/build/ownership.nia`
- [x] `lib/std/build/records_cleanup.nia`
- [x] `lib/std/build/paths.nia`
- [x] `lib/std/build/plan.nia`
- [x] `lib/std/build/probes.nia`
- [x] `lib/std/build/queries.nia`
- [x] `lib/std/build/steps.nia`
- [x] `lib/std/build/targets.nia`
- [x] `lib/std/build/types.nia`
- [x] `lib/std/build/validation.nia`

### `builtin`

- [ ] `lib/std/builtin/atomic.nia`
- [ ] `lib/std/builtin/bits.nia`
- [ ] `lib/std/builtin/control.nia`
- [ ] `lib/std/builtin/error.nia`
- [ ] `lib/std/builtin/iter.nia`
- [ ] `lib/std/builtin/layout.nia`
- [ ] `lib/std/builtin/marker.nia`
- [ ] `lib/std/builtin/mem.nia`
- [ ] `lib/std/builtin/ops.nia`
- [ ] `lib/std/builtin/place.nia`
- [ ] `lib/std/builtin/primitive.nia`
- [ ] `lib/std/builtin/simd.nia`
- [ ] `lib/std/builtin/target.nia`

### `checksum`

- [x] `lib/std/checksum/adler32.nia`
- [x] `lib/std/checksum/crc.nia`

### `collections`

- [x] `lib/std/collections/array_list.nia`
- [ ] `lib/std/collections/hash_map.nia`
- [x] `lib/std/collections/hash_set.nia`
- [ ] `lib/std/collections/tree_map.nia`
- [x] `lib/std/collections/tree_set.nia`

### `collections/array_list`

- [x] `lib/std/collections/array_list/format.nia`
- [x] `lib/std/collections/array_list/list.nia`
- [x] `lib/std/collections/array_list/raw.nia`

### `collections/hash_map`

- [ ] `lib/std/collections/hash_map/context.nia`
- [ ] `lib/std/collections/hash_map/entries.nia`
- [ ] `lib/std/collections/hash_map/format.nia`
- [ ] `lib/std/collections/hash_map/iterators.nia`
- [ ] `lib/std/collections/hash_map/map.nia`
- [ ] `lib/std/collections/hash_map/raw.nia`
- [ ] `lib/std/collections/hash_map/types.nia`

### `crypto`

- [x] `lib/std/crypto/hash.nia`

### `crypto/hash`

- [x] `lib/std/crypto/hash/sha256.nia`

### `error`

- [x] `lib/std/error/cleanup.nia`

### `fmt`

- [x] `lib/std/fmt/formatter.nia`
- [x] `lib/std/fmt/integer.nia`
- [x] `lib/std/fmt/primitive.nia`
- [x] `lib/std/fmt/spec.nia`
- [x] `lib/std/fmt/template.nia`
- [x] `lib/std/fmt/traits.nia`

### `fs`

- [x] `lib/std/fs/convert.nia`
- [x] `lib/std/fs/file.nia`
- [x] `lib/std/fs/options.nia`
- [x] `lib/std/fs/path.nia`
- [x] `lib/std/fs/types.nia`

### `hash`

- [x] `lib/std/hash/block_buffer.nia`
- [x] `lib/std/hash/fnv.nia`
- [x] `lib/std/hash/impls.nia`
- [x] `lib/std/hash/siphash.nia`
- [x] `lib/std/hash/traits.nia`
- [x] `lib/std/hash/wyhash.nia`
- [x] `lib/std/hash/xxhash.nia`

### `io`

- [ ] `lib/std/io/buffered.nia`
- [ ] `lib/std/io/debug.nia`
- [ ] `lib/std/io/error.nia`
- [ ] `lib/std/io/file_adapter.nia`
- [x] `lib/std/io/fixed_buffer.nia`
- [x] `lib/std/io/limit.nia`
- [ ] `lib/std/io/traits.nia`

### `iter`

- [x] `lib/std/iter/range.nia`

### `math`

- [x] `lib/std/math/alignment.nia`
- [x] `lib/std/math/checked.nia`
- [x] `lib/std/math/signed.nia`
- [x] `lib/std/math/unsigned.nia`

### `mem`

- [x] `lib/std/mem/allocation.nia`
- [x] `lib/std/mem/allocator.nia`
- [x] `lib/std/mem/arena_allocator.nia`
- [x] `lib/std/mem/error.nia`
- [x] `lib/std/mem/fixed_buffer_allocator.nia`
- [ ] `lib/std/mem/general_purpose_allocator.nia`
- [x] `lib/std/mem/layout.nia`
- [x] `lib/std/mem/page_allocator.nia`
- [ ] `lib/std/mem/pool_allocator.nia`
- [x] `lib/std/mem/types.nia`

### `os`

- [ ] `lib/std/os/linux.nia`
- [ ] `lib/std/os/macos.nia`
- [ ] `lib/std/os/windows.nia`

### `os/linux`

- [ ] `lib/std/os/linux/aarch64.nia`
- [ ] `lib/std/os/linux/fd.nia`
- [x] `lib/std/os/linux/mem.nia`
- [ ] `lib/std/os/linux/process.nia`
- [ ] `lib/std/os/linux/random.nia`
- [ ] `lib/std/os/linux/stat.nia`
- [ ] `lib/std/os/linux/syscall.nia`
- [ ] `lib/std/os/linux/types.nia`
- [ ] `lib/std/os/linux/x86.nia`
- [ ] `lib/std/os/linux/x86_64.nia`

### `os/linux/aarch64`

- [ ] `lib/std/os/linux/aarch64/syscall.nia`

### `os/linux/x86`

- [ ] `lib/std/os/linux/x86/syscall.nia`

### `os/linux/x86_64`

- [ ] `lib/std/os/linux/x86_64/syscall.nia`

### `os/macos`

- [ ] `lib/std/os/macos/bindings.nia`

### `os/windows`

- [ ] `lib/std/os/windows/bindings.nia`
- [ ] `lib/std/os/windows/command.nia`
- [ ] `lib/std/os/windows/directory.nia`
- [ ] `lib/std/os/windows/encoding.nia`
- [ ] `lib/std/os/windows/error.nia`
- [ ] `lib/std/os/windows/fd.nia`
- [x] `lib/std/os/windows/mem.nia`
- [ ] `lib/std/os/windows/path.nia`
- [ ] `lib/std/os/windows/process.nia`
- [ ] `lib/std/os/windows/random.nia`
- [ ] `lib/std/os/windows/stat.nia`
- [ ] `lib/std/os/windows/types.nia`

### `os/windows/process`

- [ ] `lib/std/os/windows/process/executable.nia`
- [ ] `lib/std/os/windows/process/stdio.nia`

### `parse`

- [ ] `lib/std/parse/core.nia`

### `process`

- [x] `lib/std/process/args.nia`
- [ ] `lib/std/process/child.nia`
- [ ] `lib/std/process/command.nia`
- [ ] `lib/std/process/convert.nia`
- [x] `lib/std/process/env.nia`
- [ ] `lib/std/process/exit_code.nia`
- [ ] `lib/std/process/init.nia`
- [ ] `lib/std/process/pipe.nia`
- [ ] `lib/std/process/term.nia`
- [ ] `lib/std/process/types.nia`

### `string`

- [ ] `lib/std/string/hash.nia`

### `slice`

- [x] `lib/std/slice/order.nia`

### `top-level`

- [ ] `lib/std/atomic.nia`
- [ ] `lib/std/build.nia`
- [ ] `lib/std/builtin.nia`
- [x] `lib/std/checksum.nia`
- [ ] `lib/std/cmp.nia`
- [x] `lib/std/collections.nia`
- [x] `lib/std/crypto.nia`
- [x] `lib/std/cstring.nia`
- [ ] `lib/std/error.nia`
- [x] `lib/std/fmt.nia`
- [ ] `lib/std/fs.nia`
- [x] `lib/std/hash.nia`
- [ ] `lib/std/io.nia`
- [x] `lib/std/iter.nia`
- [x] `lib/std/math.nia`
- [ ] `lib/std/mem.nia`
- [x] `lib/std/option.nia`
- [ ] `lib/std/os.nia`
- [ ] `lib/std/parse.nia`
- [ ] `lib/std/process.nia`
- [x] `lib/std/result.nia`
- [ ] `lib/std/root.nia`
- [x] `lib/std/slice.nia`
- [ ] `lib/std/source.nia`
- [x] `lib/std/string.nia`
- [ ] `lib/std/test.nia`
- [ ] `lib/std/unicode.nia`
