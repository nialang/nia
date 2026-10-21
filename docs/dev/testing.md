# Testing

Toolchain prerequisites for the tests (LLD, clang 23, i686 multilib, QEMU) are
listed in [Development setup](setup.md).

## Gates

Run these before submitting a compiler change. They match the managed
correctness workflows in `.github/workflows/build-std.yml`.

Linux:

```sh
cargo maintain check
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Windows, from an x64 Developer PowerShell (see [setup](setup.md#windows-msvc)):

```powershell
cargo maintain check
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --no-default-features --target x86_64-pc-windows-msvc -- -D warnings
cargo test --workspace --no-default-features --target x86_64-pc-windows-msvc
```

The explicit `--target` keeps the static-CRT flags off host build scripts;
artifacts land under `target/x86_64-pc-windows-msvc/`.

`cargo maintain check` runs the fast repository audits described in
[`maintain/README.md`](../../maintain/README.md); Cargo owns compilation,
Clippy, and tests. CI additionally checks the workspace for
`i686-unknown-linux-gnu` and the portable semantic crates for
`wasm32-unknown-unknown`, runs Darwin suites in `.github/workflows/platform.yml`,
and cross-links and executes probe programs between Linux and Windows.

## Focused Before Broad

Validate from narrow to broad: owner-level tests for the changed contract, then
direct consumer and cross-phase tests, then the workspace gates, then any
relevant end-to-end, build, runtime, or performance workflow. Run a single
integration suite with, for example:

```sh
cargo test -p nia-cli --test build_cases
cargo test -p nia-cli --test commands -- readme
```

On Windows, add `--no-default-features --target x86_64-pc-windows-msvc`.

### Untrusted parser properties

`nia-git` uses stable `proptest` suites for parser boundaries that consume
network, repository, or credential data. The suites cover packet lines, loose
and packed objects, delta instructions, HTTP responses and chunk framing, and
SSH wire, cipher, key exchange, host-key, private-key, and agent messages.
They are bounded randomized tests, not a coverage-guided fuzz campaign or a
security proof.

Run the focused suite with the default sample count:

```sh
cargo test -p nia-git --lib properties
```

Increase the sample count when changing a parser boundary. CI uses 4096 cases
per property test:

```sh
PROPTEST_CASES=4096 cargo test -p nia-git --lib properties
```

On PowerShell, set `$env:PROPTEST_CASES = '4096'` for the command. Proptest
stores minimized failing cases under `proptest-regressions/`; keep those files
with the owning parser when a failure is reproducible. They replay automatically
on subsequent runs. To repeat a randomized run, set `PROPTEST_RNG_SEED` to the
same unsigned 64-bit integer before both runs (for example, `12345`). CI runs
the suite independently on Linux and Windows and uploads regression files.

Parser resource limits belong to the module that owns the input. File and
transport readers share a bounded-read helper; malformed data and limit
violations return `InvalidData`, while filesystem and transport errors retain
their original error kinds. The current limits are:

| Input | Limit |
| --- | --- |
| HTTP, Git TCP, or SSH response | 128 MiB |
| Local pack or compressed loose object | 128 MiB |
| Decoded object or delta result | 64 MiB per object |
| Objects in a pack | 1,000,000 |
| Streamed packet-line advertisement, including flush | 16 MiB |
| Loose reference / packed references | 4 KiB / 16 MiB |
| SSH identity / known hosts | 1 MiB / 16 MiB |
| SSH agent identities | 1,024 |
| Checkout directory depth / entries | 64 / 1,000,000 |
| Checkout decoded tree and blob bytes, counting repeated objects | 512 MiB |

Checkout uses an explicit traversal stack and creates each tree path
exclusively. The destination must be missing or empty, with real directory
ancestors. The caller owns it throughout checkout and discards partial output
on failure. Tree entries cannot name Git metadata, traverse directories, or
use Windows device names and alternate data streams. These checks do not
protect against another process concurrently replacing destination paths.
The per-object bounds do not yet bound all decoded objects retained by a pack.

## Where Tests Live

| Location | Contents |
| --- | --- |
| `#[cfg(test)]` modules or `src/tests/` in each crate | Unit and owner tests for the owning crate and module. |
| `crates/nia-cli/tests/*.rs` | End-to-end suites for the `nia` binary: commands, `emit_exe_*` executable behavior, `std_*` standard-library behavior, C ABI and interop, cross targets, build system (`build_cases`, `std_build_*`), toolchain relocation. |
| `crates/nia-cli/tests/cases/` | Data-driven fixture trees (`build`, `commands`, `linker`, `optimization`), each described by a `case.meta` manifest. |
| `crates/nia-driver/tests/cases/` | Driver fixtures for check, codegen, object, executable, LLVM, and persistent-cache cases. |
| `crates/nia-test-support/` | Shared harness: resource accounting, command helpers, owned scratch directories, case manifests. |
| `maintain/tests/`, `maintain/fixtures/` | Maintenance tool contracts and fixtures. |
| `benchmarks/` | Performance workloads; see [`maintain/performance.md`](../../maintain/performance.md). |

Choose a test shape that makes the contract visible. Data-driven suites suit
repeated compiler matrices, edits, diagnostics, and output comparisons.
Hand-written tests suit process, filesystem, toolchain, allocator, and
startup/runtime contracts, where opaque metadata would hide the behavior under
review. Invalidation tests exercise edit sequences and compare observable
products against a clean run, not only cache hits.

## Test Profile

The `test` profile uses `opt-level = 1` because integration tests run Nia as a
compiler, not just a command parser; debug assertions and overflow checks stay
enabled. The `dev` and `test` profiles keep line tables instead of full
variable debug information to limit disk use. Cargo does not garbage-collect
old target variants, so prune `target/` manually when it grows.

## Resource-Accounted Harness

Libtest keeps its normal platform-selected concurrency. Tests that start a
compiler, LLVM backend, generated build runner, nested build, or other
subprocess go through `nia-test-support` instead of spawning bare commands:

- Run child commands with the `CommandExt` helpers
  (`output_timeout_for_compiler`, `output_timeout_for_build`,
  `output_timeout_for_runtime`, `output_timeout_in_session`). They apply
  timeouts, bounded output, and process-tree cleanup.
- A test that issues several compiler or build commands holds one session from
  `acquire_test_resources(TestWorkload::Compiler | TestWorkload::Build)`.
  Nested helpers reuse it rather than charging twice. Unit tests that construct
  a `nia-driver` `Driver` directly acquire the same boundary.
- Scratch storage has an owner. Use `nia_test_support::test_dir`, whose guard
  removes the whole tree on drop, including during unwinding. Shared helpers
  never return an unowned `PathBuf` under the system temporary directory.

Compiler and build work share a cross-process memory budget derived from the
effective CPU and memory limits (cgroup limits in containers and rental hosts,
the VM's resources under WSL, system resources on bare hosts). There are no
per-machine test profiles or private environment switches; if memory cannot be
detected, compiler-heavy tests run serially. The accounting details are
documented in `crates/nia-test-support/src/lib.rs`. Set
`NIA_TEST_RESOURCE_TRACE=1` to print permit waits and per-command peaks when
tuning a new host.

Tests use the same public compiler, build, and LLVM contracts as production;
the harness bounds resources but does not change production semantics.
