# Nia Standard Library

`std.nia` is the public root facade. Public submodule facades re-export the
supported API, while provider and implementation modules remain package-private.
Importing a facade does not by itself select every provider; loader tests keep
that dependency closure explicit.

The library follows a small set of ownership rules:

- Owned values such as `String`, `fs::Path`, and collections use explicit
  allocators for allocation, mutation, and release. Nia does not add implicit
  destruction or hidden allocator state to these values.
- Borrowed forms use views such as `&[T]`, `fs::PathView`, and `CStringView`.
  A view never extends the lifetime of its backing storage.
- Ownership-transfer constructors consume an existing owner. Copying
  constructors accept an allocator and preserve typed allocation or validation
  failures.
- Fallible cleanup attempts every independently owned resource and retains each
  allocation whose release fails. A partially completed collection `deinit` is
  cleanup-only state: retry `deinit` with the same allocator rather than using
  the value again.
- Hash-map control bytes, keys, and values share one aligned storage block.
  Rehash keeps the replaced block as a map-owned retired allocation until its
  release succeeds; a failed release leaves the active table usable but
  requires cleanup retry before a later rehash or final `deinit`.
- ArrayList replacement growth and shrink retain a failed temporary replacement
  `Block` for `deinit` retry instead of dropping it after an old-owner failure;
  the active allocation keeps its owner in `storageBlock`, even when the
  logical length or capacity is zero. Later growth replaces that stored owner,
  not an empty block inferred from the old logical capacity.
- ArrayList self-aliasing append, insert, and replace operations retain failed
  temporary-copy `Block`s separately from replacement storage. Borrowed slice
  views and capacities are never used to recreate release ownership.
- Hash-map capacity counts logical entries, including slots made reusable by
  deletion. `insertAssumeCapacity` therefore requires `len < capacity`, not an
  unused physical empty-slot budget; it may reuse either an empty or deleted
  control slot without allocating.
- Build-plan collection counts and derived dependency/input/output counts are
  checked before narrowing to the fixed-width protocol fields. Generated-file
  payload lengths are checked before their length prefixes are published.
- Build graph cleanup releases a containing list only after all owning elements
  in that list have been released. Failed nested strings, paths, arguments, or
  imports remain reachable for a later `Build::deinit` retry.
- File close paths treat the descriptor as consumed before the OS close result;
  cleanup defers must not issue a second close after a close error.
- Temporary filesystem descriptors are closed after their owning operation. A
  failed operation remains the primary error; after success, the first close
  error is returned while every later descriptor is still attempted.
- Spawn handshake descriptors are consumed and closed exactly once after the
  handshake result is known. EOF plus close failure is reported as setup error;
  an earlier handshake/read error remains the primary cause.
- Errors retain their domain cause. `IntoError` is for reviewed, infallible
  propagation; contextual operation, path, subject, and cleanup information is
  attached by the owning module.
- Generic I/O adapters preserve the wrapped reader or writer's invalid-transfer
  error identity while rejecting impossible byte counts before mutating state.
- Slice iterators validate checked element offsets and addresses before moving
  their front or back bounds; an impossible address returns `null` without
  consuming the pending element.
- General-purpose large allocations validate the backing base plus header offset
  before publishing allocator metadata, rejecting malformed child addresses.
- General-purpose allocator rollback retains a pending child block if malformed
  metadata is rejected but its release fails; later allocation or `deinit`
  retries that owner and keeps it visible in capacity/emptiness accounting.
- Over-aligned page allocations may expose an interior aligned pointer, while
  `mem::Block` retains the complete mmap release range for one exact `free`.
  Post-`mmap` validation failures release that unpublished range before
  returning an error.
- Default `Allocator::realloc` reports a typed `ReallocError`. When both the old
  and replacement releases fail, its `Rollback` variant carries the replacement
  block and cleanup error so callers can retry the still-owned allocations.
- Successful allocator remap changes only a block's logical layout. Default and
  concrete remap paths preserve the original release pointer and length, as do
  arena backing-chunk resizes.
- `Allocator::allocSlice` returns a `SliceAllocation[T]` owner instead of a raw
  slice. `asSlice` and `asMutSlice` borrow its view, `deinit` releases the exact
  `Block` even for a zero-length view, and `ArrayList`/`String` owner-transfer
  APIs consume that typed owner.
  Raw-slice ownership constructors are intentionally absent.
- Staged string formatting preserves format and UTF-8 errors over temporary
  writer cleanup failures; cleanup allocation errors are reported only when
  formatting itself succeeds. A backing whose `free` failed before release is
  transferred to the destination as a pending owner, retried before the next
  format and during `String::deinit`.
- Callback parameters are borrowed for the duration of a call unless an API
  explicitly returns an owner such as `mem::CallableAllocation`.
- `mem::allocValue` is the public construction boundary for allocator-backed
  typed owners. `Allocated` and `CallableAllocation` retain the complete
  allocator release range across callable transfer; successful deinit and
  transfer clear that range so cleanup is single-shot. Implementation modules
  do not hide the only entry point.

The source files are the API reference: facades state their boundaries and the
owning modules document type-specific lifetime, error, and cleanup contracts.
Language-level behavior is specified in `docs/language-spec.md`; implemented
compiler boundaries are described in `docs/architecture.md`, and Rust build
ownership lives in [`crates/nia-build/README.md`](../crates/nia-build/README.md)
and its source modules.

Standard-library conformance lives primarily in the `emit_exe_*.rs` and
`std_*.rs` tests under `crates/nia-cli/tests/`, plus the full-pipeline tests under
`crates/nia-driver/src/tests/`. The build-host provider audit is maintained by
`cargo maintain audit std-build-host`; its enforced fixture is
`maintain/fixtures/std-build-host-dependencies.json`. Run the audit without options
to check it, or use `--print` to inspect a deliberate closure update.
