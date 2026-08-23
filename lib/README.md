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
- `String::intoOwnedSlice` first retires any pending formatting staging owner;
  a successful transfer therefore leaves no second allocation behind in the
  source string.
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
- `Build::init` returns a `BuildInitAttempt` that owns every partially retained
  path, target component, and requested-step string. `finish` must be retried
  after a cleanup failure; it exposes the complete `Build` or primary
  initialization error only when partial cleanup is complete.
- Package graph insertion reserves the containing list first and initializes a
  `Build`-owned pending record. A failed package field release remains attached
  for retry by the next insertion or `Build::deinit`; no local rollback defer
  can discard it.
- Object and static-archive insertion likewise reserve their target list and
  retain partial name/output-name owners in dedicated `Build` pending slots.
  Failed releases are retried before the next matching insertion or deinit.
- Executable insertion extends the pending target to cover its static-archive
  handle-list backing. Cleanup attempts the list and both strings, retains every
  failed owner, and blocks the next executable insertion until retry succeeds.
- Module insertion owns the partial module and all appended import slots on
  `Build`. The outer and nested lists reserve before initialization; failed
  import name/path cleanup retains both fields and the containing imports
  backing for recursive retry.
- Generated-file and uncacheable insertion keep a kind-correct pending `Step`
  on `Build`. The steps list reserves first; step name, output/contents, or
  description owners remain reachable across failed rollback cleanup.
- Run/test insertion appends empty argument string owners to a reserved nested
  list before initialization. If dependency insertion fails after step commit,
  rollback moves the popped step into the pending slot; cleanup failure cannot
  replace the dependency error or discard its name/arguments.
- Executable and static-archive install steps retain their step name and
  destination in kind-specific pending payloads. Producer-edge failure remains
  primary while rollback keeps both strings reachable for retry.
- External-command insertion reserves both nested lists and appends kind-correct
  argument/environment owners before string retention. Multi-producer edge
  failure removes all newly added edges, preserves the primary error, and moves
  the command step into pending ownership for recursive cleanup retry.
- Aggregate, check, and emit step names use the same pre-reserved pending step.
  Emit-executable archive edges and external-command producer edges use explicit
  dependency-suffix rollback. The build implementation has no fallible cleanup
  defer that can hide an owner or replace a primary error.
- Build dependency validation stores its indegree and ready-list scratch owners
  on `Build`. Both releases are attempted after every validation pass; a
  validation error remains primary and failed scratch frees remain retryable on
  the next pass or `Build::deinit`.
- Build-plan encoding stores its byte backing on `Build` until publication and
  cleanup complete. Writer, flush, sync, and close errors keep the first
  operation error; a failed backing free remains retryable for the next draft
  or `Build::deinit`.
- File close paths treat the descriptor as consumed before the OS close result;
  cleanup defers must not issue a second close after a close error.
- `FileReader`, `FileWriter`, and `DirIterator` borrow their `File` or `Dir`
  owner and resolve its live handle before accessing the underlying descriptor.
  They never retain a copied descriptor that could silently target a different
  object after close and OS descriptor reuse. Callers must keep the borrowed
  owner storage alive and stable for the adapter lifetime.
- Temporary filesystem descriptors are closed after their owning operation. A
  failed operation remains the primary error; after success, the first close
  error is returned while every later descriptor is still attempted.
- Filesystem syscall paths encode into `os::maxPathBytes` caller-stack storage.
  Scalar and split native paths return `PathError::TooLong` before a syscall
  when that bound is exceeded; they do not create hidden allocator owners whose
  fallible cleanup could replace the operation result.
- Spawn handshake descriptors are consumed and closed exactly once after the
  handshake result is known. EOF plus close failure is reported as setup error;
  an earlier handshake/read error remains the primary cause.
- Linux spawn setup keeps all four pipe pairs in one resource transaction.
  Every return path closes all untransferred ends; successful child identity
  and public stdin/stdout/stderr ends transfer out before that cleanup. Because
  close consumes a descriptor, a close error must not replace the primary spawn
  error or turn a successfully executed but now unowned child into an error.
- Failed-child reaping is part of `SpawnAttempt` ownership. A non-interrupted
  `wait4` failure returns the spawn stage, original cause, reap cause, and pid
  while retaining that same native attempt for the next `finish`; the original
  spawn error is exposed only after reaping completes. `ECHILD` confirms that
  no waitable pid owner remains and therefore completes cleanup.
- `Command::spawn` returns a `SpawnAttempt` owner. Its `finish` operation tries
  every argv/envp/path staging release and returns a cleanup error while keeping
  the pending `Child` or spawn error attached for retry; only complete cleanup
  exposes that outcome. Custom-allocator attempts require the same allocator on
  each retry. Owner-discarding `run` shortcuts are not part of the API.
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
  This owner-carrying error has no generic process-exit conversion; callers must
  match it and retire the original and replacement owners explicitly.
- Successful allocator remap changes only a block's logical layout. Default and
  concrete remap paths preserve the original release pointer and length, as do
  arena backing-chunk resizes.
- Arena cleanup detaches used and free chunk lists, attempts every child-block
  release, and links each failed owner back into the arena free list. A later
  `deinit` retries only those residual chunks; one failed release must not hide
  a later chunk or erase retained capacity.
- General-purpose allocator cleanup independently walks small pages and large
  headers, then releases any pending rollback block. Each failed owner returns
  to its matching state slot, so capacity/used/emptiness remain truthful and a
  retry never revisits successfully released owner classes.
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
  typed owners. `Allocated` and `CallableAllocation` retain the returned Block
  layout and complete allocator release range across callable transfer,
  including non-empty Blocks for zero-sized values; successful deinit and
  transfer clear that state so cleanup is single-shot. Implementation modules
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
