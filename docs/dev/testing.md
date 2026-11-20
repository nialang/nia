# Testing

Toolchain prerequisites for the tests (LLD, clang 23, i686 multilib, QEMU) are
listed in [Development Setup](setup.md).

## Gates

Run these before submitting a compiler change. They match the correctness jobs
in `.github/workflows/build-std.yml`.

Linux:

```sh
cargo maintain check
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Windows, from an x64 Developer PowerShell (see
[Development Setup](setup.md#windows-msvc)):

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
Clippy, and tests. CI does more than the gates:

- `build-std.yml` also checks the workspace for `i686-unknown-linux-gnu` and
  the portable semantic crates for `wasm32-unknown-unknown`, cross-links probe
  programs for Windows and Darwin on Linux and for Linux and Darwin on Windows,
  and runs each probe on a runner of its target system.
- `platform.yml` runs the package, executable, and standard-library suites on
  Darwin and smoke-tests the packaged Darwin language server.
- `formal.yml` runs `lake build` in `formal/` when a
  [formal model](../../formal/README.md) or the code tied to it changes. Run
  it locally for such a change too.
- `git-parsers.yml` and `git-fuzz.yml` exercise the `nia-git` parser
  boundaries; see [Untrusted Parser Properties](#untrusted-parser-properties).
- `performance.yml` compares compiler performance against a reference
  revision; see [`maintain/performance.md`](../../maintain/performance.md).

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

## Where Tests Live

| Location | Contents |
| --- | --- |
| `#[cfg(test)]` modules or `src/tests/` in each crate | Unit and owner tests for the owning crate and module. |
| `crates/nia-cli/tests/*.rs` | End-to-end suites for the `nia` binary: commands, `emit_exe_*` executable behavior, `std_*` standard-library behavior, C ABI and interop, cross targets, build system (`build_cases`, `std_build_*`), toolchain relocation, examples (`examples_run`), and documentation code blocks (`documentation`). |
| `crates/nia-cli/tests/cases/` | Data-driven fixture trees (`build`, `commands`, `linker`, `optimization`), each described by a `case.meta` manifest. |
| `crates/nia-driver/tests/cases/` | Driver fixtures for check, codegen, object, executable, LLVM, and persistent-cache cases. A check case's `main.snap` holds its diagnostics; `NIA_UPDATE_SNAPSHOTS=1` rewrites the changed ones instead of failing, for review as a diff. |
| `crates/nia-lsp/src/` | Language-server unit tests; see [Language Server](#language-server) for the stdio tools. |
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
compiler, not only as a command parser; debug assertions and overflow checks
stay enabled. The `dev` and `test` profiles keep line tables instead of full
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
effective CPU and memory limits: cgroup limits in containers and rental hosts,
the VM's resources under WSL, and system memory on bare Linux, Windows, and
macOS hosts. There are no per-machine test profiles or private environment
switches; a host whose memory cannot be read runs compiler and runtime work
one at a time. The accounting details are documented in
`crates/nia-test-support/src/lib.rs`. Set `NIA_TEST_RESOURCE_TRACE=1` to print
pool sizes, permit waits, and per-command peaks when tuning a new host.

`nia build` commands from the harness share one compiler-cache root per `nia`
binary, under the system temporary directory, so the build runner and the
standard library are compiled once per test run rather than once per test
package. Configure, action, and suite caches stay in the test package's (or
test workspace's) own `build/cache/`, so tests of cache behavior see only their
own records.

Tests use the same public compiler, build, and LLVM contracts as production;
the harness bounds resources but does not change production semantics.

## Language Server

`cargo test -p nia-lsp` runs the server's unit tests, which drive analysis
sessions and the protocol loop in process. The Python tools in `tools/` speak
to a built server over stdio; they need Python 3.9 or later. The user-facing
feature set they check is described in
[`editors/README.md`](../../editors/README.md).

- `tools/lsp-smoke.py <server>` exercises a packaged server end to end:
  editing and navigation, module and package moves, build-plan entries,
  source-mapped generated code, and long expressions. Pass the `bin/nia-lsp`
  of an unpacked release archive, which finds its own `lib`. The release
  workflow runs it for every platform archive, and `platform.yml` runs it on
  Darwin.
- `tools/lsp-incremental-audit.py <server> --resource-root lib` checks that
  incremental analysis equals clean analysis: after each edit it compares the
  server's results with a fresh session on the same snapshot. `--workload`
  selects `workspace`, `array-list`, or `dependencies`; `--semantic-tokens`
  also compares published semantic tokens; `--cancel-retry` cancels a
  definition request after each edit and checks the retry; `--sample-rss`
  records memory after each snapshot.
- `tools/lsp-latency.py` measures request round trips, versioned diagnostic
  arrival, and a semantic-token fetch after diagnostics, plus rename,
  cancellation, and peak resident memory. Use a release server. Benchmark
  edits stay in the editor session and are never written to disk:

  ```sh
  python tools/lsp-latency.py --server target/release/nia-lsp --resource-root lib \
    --workspace lib/std --source lib/std/collections/array_list/list.nia --symbol ArrayList
  ```

  To measure member completion and the diagnostics that follow each edit,
  replace `--symbol ArrayList` with `--member "self.len() == 0" self isEmpty`.
  Each repetition types the member one character at a time and checks that
  the server offers it at every prefix; the report separates the first `.`
  request of the session from later ones and lists each prefix and diagnostic
  latency. `--repeat 3` gives a short run. `--revision` and `--configuration`
  only label the report with the commit and build settings of the server.

Set `NIA_LSP_TIMINGS=summary` to make the server print a JSON timing report
on stderr at shutdown; `NIA_LSP_TIMINGS=detail` adds individual compiler query
timings.

The editor clients have their own smoke tests, which the release workflow runs
against the packaged server and VSIX:

- `tools/lsp-vscode-smoke.py` installs the VSIX in a temporary VS Code
  profile and checks language detection, the file icon, semantic tokens,
  folding, and a module move with its child directory. Pass the VS Code
  desktop executable, the server from an unpacked release, and the VSIX; add
  `--cli` when the VS Code CLI is not `code` on `PATH`. Run it in a desktop
  session:

  ```sh
  python tools/lsp-vscode-smoke.py --code /path/to/Code /path/to/bin/nia-lsp /path/to/nia.vsix
  ```

- `tools/lsp-neovim-smoke.lua` runs Neovim's built-in LSP client headless
  against the server and checks highlighting, folding, cross-file rename, and
  definition navigation after saving:

  ```sh
  nvim --headless -u NONE -i NONE -l tools/lsp-neovim-smoke.lua /path/to/bin/nia-lsp
  ```

## Untrusted Parser Properties

`nia-git` uses stable `proptest` suites for parser boundaries that consume
network, repository, or credential data. The suites cover packet lines, loose
and packed objects, delta instructions, HTTP responses and chunk framing, and
SSH wire, cipher, key exchange, host-key, private-key, and agent messages.
They are bounded randomized tests, not a coverage-guided fuzz campaign or a
security proof.

Run the suite with the default sample count:

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
with the owning parser when a failure is reproducible. They replay
automatically on later runs. To repeat a randomized run, set
`PROPTEST_RNG_SEED` to the same unsigned 64-bit integer before both runs (for
example, `12345`). `.github/workflows/git-parsers.yml` runs the suite on Linux
and Windows and uploads regression files.

Coverage-guided targets live in `crates/nia-git/fuzz/` and run weekly in
`.github/workflows/git-fuzz.yml` on nightly Rust. They cover packet lines,
loose and packed objects, delta instructions, HTTP response framing, SSH agent
identities, SSH KEXINIT, OpenSSH private identities, and `known_hosts`. To run
a target locally, install `cargo-fuzz` and nightly Rust, then run from that
directory:

```sh
cargo +nightly fuzz run packet_lines -- -max_total_time=120 -max_len=1048576
```

Minimized crash inputs are written under `fuzz/artifacts/`; promote a
confirmed regression to the owning parser's deterministic tests before fixing
it.

The input limits and protocol checks these suites exercise are specified in
the [`nia-git` README](../../crates/nia-git/README.md); a change to a limit
updates that table and the owning module's tests together.
