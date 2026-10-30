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

- Files: `lib/std/hash/wyhash.nia`, `docs/architecture.md`
- Evidence: `update` saturates the cumulative `usize` length on overflow, and `finishValue` folds that value into the digest. The policy was explicitly introduced as overflow hardening and documented in the architecture notes.
- Question: should streaming length follow Wyhash's low-64-bit length input modulo the algorithm width, or should the Nia API promise saturation after an unrepresentable cumulative stream length?
- Direction: preserve current behavior during this pass; settle the contract and a testable length abstraction before changing it.
- Status: open design review, no confirmed bug.

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
- Result: moved target lookup and executable/object/static-archive registration, including pending-record cleanup, into `artifacts.nia`. `core.nia` retains package/module/step orchestration and dependency-graph validation; the public facade imports both providers, and lifecycle cleanup imports the artifact provider explicitly.
- Validation: build-target, ownership rollback, build-case, and command-cache integration tests pass; no compiler defect or behavior change was found.
- Status: completed.

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

### Review conclusions

- The earlier exploratory suggestions to extract `BlockBuffer` and unify SipHash variants are already implemented.
- Keep index-based loops where an index is the algorithm's word, lane, or serialization position; replace only cases where iteration expresses intent more directly.
- Checked widening and narrowing in hash arithmetic are necessary under Nia's trapping integer semantics and remain explicit reference code.

## File checklist

### `build`

- [x] `lib/std/build/commands.nia`
- [x] `lib/std/build/core.nia`
- [x] `lib/std/build/artifacts.nia`
- [ ] `lib/std/build/dependencies.nia`
- [ ] `lib/std/build/diagnostics.nia`
- [ ] `lib/std/build/error.nia`
- [ ] `lib/std/build/handles.nia`
- [ ] `lib/std/build/lifecycle.nia`
- [ ] `lib/std/build/modules.nia`
- [ ] `lib/std/build/ownership.nia`
- [x] `lib/std/build/paths.nia`
- [ ] `lib/std/build/plan.nia`
- [ ] `lib/std/build/probes.nia`
- [ ] `lib/std/build/queries.nia`
- [ ] `lib/std/build/records_cleanup.nia`
- [ ] `lib/std/build/steps.nia`
- [ ] `lib/std/build/targets.nia`
- [ ] `lib/std/build/types.nia`
- [ ] `lib/std/build/validation.nia`

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

- [ ] `lib/std/collections/array_list.nia`
- [ ] `lib/std/collections/hash_map.nia`
- [ ] `lib/std/collections/hash_set.nia`
- [ ] `lib/std/collections/tree_map.nia`
- [ ] `lib/std/collections/tree_set.nia`

### `collections/array_list`

- [ ] `lib/std/collections/array_list/format.nia`
- [ ] `lib/std/collections/array_list/list.nia`
- [ ] `lib/std/collections/array_list/raw.nia`

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

- [ ] `lib/std/error/cleanup.nia`

### `fmt`

- [x] `lib/std/fmt/formatter.nia`
- [x] `lib/std/fmt/integer.nia`
- [x] `lib/std/fmt/primitive.nia`
- [x] `lib/std/fmt/spec.nia`
- [x] `lib/std/fmt/template.nia`
- [x] `lib/std/fmt/traits.nia`

### `fs`

- [ ] `lib/std/fs/convert.nia`
- [ ] `lib/std/fs/file.nia`
- [ ] `lib/std/fs/options.nia`
- [ ] `lib/std/fs/path.nia`
- [ ] `lib/std/fs/types.nia`

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
- [ ] `lib/std/io/fixed_buffer.nia`
- [ ] `lib/std/io/limit.nia`
- [ ] `lib/std/io/traits.nia`

### `iter`

- [x] `lib/std/iter/range.nia`

### `math`

- [x] `lib/std/math/alignment.nia`
- [x] `lib/std/math/checked.nia`
- [x] `lib/std/math/signed.nia`
- [x] `lib/std/math/unsigned.nia`

### `mem`

- [ ] `lib/std/mem/allocation.nia`
- [ ] `lib/std/mem/allocator.nia`
- [ ] `lib/std/mem/arena_allocator.nia`
- [ ] `lib/std/mem/error.nia`
- [ ] `lib/std/mem/fixed_buffer_allocator.nia`
- [ ] `lib/std/mem/general_purpose_allocator.nia`
- [ ] `lib/std/mem/layout.nia`
- [ ] `lib/std/mem/page_allocator.nia`
- [ ] `lib/std/mem/pool_allocator.nia`
- [ ] `lib/std/mem/types.nia`

### `os`

- [ ] `lib/std/os/linux.nia`
- [ ] `lib/std/os/macos.nia`
- [ ] `lib/std/os/windows.nia`

### `os/linux`

- [ ] `lib/std/os/linux/aarch64.nia`
- [ ] `lib/std/os/linux/fd.nia`
- [ ] `lib/std/os/linux/mem.nia`
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
- [ ] `lib/std/os/windows/mem.nia`
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

- [ ] `lib/std/process/args.nia`
- [ ] `lib/std/process/child.nia`
- [ ] `lib/std/process/command.nia`
- [ ] `lib/std/process/convert.nia`
- [ ] `lib/std/process/env.nia`
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
- [ ] `lib/std/collections.nia`
- [x] `lib/std/crypto.nia`
- [ ] `lib/std/cstring.nia`
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
- [ ] `lib/std/string.nia`
- [ ] `lib/std/test.nia`
- [ ] `lib/std/unicode.nia`
