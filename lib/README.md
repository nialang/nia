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
- Errors retain their domain cause. `IntoError` is for reviewed, infallible
  propagation; contextual operation, path, subject, and cleanup information is
  attached by the owning module.
- Callback parameters are borrowed for the duration of a call unless an API
  explicitly returns an owner such as `mem::CallableAllocation`.

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
