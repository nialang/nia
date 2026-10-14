# Contributing to Nia

Nia is a maintainer-led project. Contributions, issues, and design discussions
are welcome; final language and implementation decisions rest with the project
maintainer. The project values a small coherent language over consensus-driven
feature growth, so a technically valid change may still be declined if it does
not fit the direction of Nia.

Related developer documents:

- [Development setup](setup.md): toolchains, Windows MSVC, environment variables.
- [Testing](testing.md): gates, test layout, and the resource-accounted harness.
- [Compiler architecture](architecture.md): phases, products, and crate index.
- [Build system](../../crates/nia-build/README.md): build plans, execution, and caches.

## Project Status

Compatibility with earlier experimental syntax is not a goal. Removed behavior
does not receive migration paths, compatibility tests, or diagnostics that
exist only to explain old spellings. Tests reflect the current language: a
removed spelling may be tested as an ordinary rejection when it marks an
important boundary, never as a compatibility feature.

### Releases

Formal releases are minor baselines named `0.x.0`, published from a `vX.Y.0`
tag that matches the `nia-cli` package version; the release workflow rejects
other tag shapes. Between releases, `main` carries the next minor development
version (for example `0.2.0-dev`) across ordinary commits. Patch releases are
reserved for important fixes to a released minor line and are maintained on a
dedicated release branch. The build and cache protocol epoch follows the
release line; see [Protocol identity](../../crates/nia-build/README.md#protocol-identity).

## Before Contributing

Open a discussion before starting work on:

- language syntax or semantics;
- compiler architecture or crate boundaries;
- ABI, linking, code generation, or runtime model changes;
- broad test rewrites or documentation structure changes.

Small bug fixes, focused tests, typo fixes, and local documentation improvements
can usually be proposed directly.

## Change Routing

Start a change at the owner of the contract it modifies:

| Change | Owner document | Implementation owner |
| --- | --- | --- |
| Language syntax or semantics | [Language reference](../reference/language.md) | The owning compiler phase ([architecture](architecture.md)) |
| Compiler phase or crate boundary | [Architecture](architecture.md) | The affected crate |
| ABI, layout, or runtime representation | [ABI reference](../reference/abi.md), [ABI internals](abi-internals.md) | Layout and backend crates |
| Build API or build execution | [`crates/nia-build/README.md`](../../crates/nia-build/README.md) | `lib/std/build/`, `crates/nia-build/`, `crates/nia-build-plan/` |
| Standard-library API | [`lib/README.md`](../../lib/README.md) | The owning `lib/std` module |
| Targets and platform support | [Platform support](../reference/platform-support.md) | `crates/nia-target-config/`, `lib/` startup resources |
| Repository audit or performance baseline | [`maintain/README.md`](../../maintain/README.md), [`maintain/performance.md`](../../maintain/performance.md) | `maintain/` |

## Code Standards

- Follow the existing crate boundaries and local patterns. Prefer small,
  reviewable changes over broad rewrites.
- Run the gates in [Testing](testing.md#gates) before submitting.
- Do not add `allow` or `expect` attributes to bypass lints; fix the code. The
  project follows the newest Rust stable toolchain, and new compiler or Clippy
  diagnostics are maintenance work fixed at their source.
- Configure compiler behavior with CLI flags or typed API options, not hidden
  environment variables. New `NIA_*` variables must be added to the
  [environment-variable registry](setup.md#environment-variables).

## Engineering Principles

These rules apply across compiler subsystems. Detailed ownership rules live
with their owners (architecture, ABI, build-system, and standard-library docs).

### Ownership and boundaries

Every durable fact has one owner and one source of truth. Ownership follows the
lifetime and mutation authority of the fact, not the crate that happens to
mention it. A product may have many consumers, but its owner defines its
identity, validation, storage policy, and publication boundary.

An architectural change is complete only when the new contract is the only
active one: remove the obsolete entry point, fallback, duplicate source of
truth, and old/new dual path. A compatibility adapter is acceptable only at one
explicit migration boundary with a planned end. Rejected experiments leave no
dormant schemas, readers, counters, or fallback paths behind.

Keep phase boundaries visible in the data flow. Pass typed identities and
validated immutable products between phases, and give each consumer only the
capabilities it needs. Driver code coordinates requests; it does not recreate
semantic fixed points owned by compiler queries. Session-local handles may be
compact, but anything persisted, compared across processes, or exposed in a
stable artifact needs an explicit canonical representation. When a proposed
aggregate, shared cache, or crate split has no clear consumer or ownership
authority, keep the narrower product.

### Failure, ICEs, and diagnostics

Expected failures use typed results and the project's diagnostic channels.
User errors stay distinguishable from internal compiler errors (ICEs), and an
ICE reaches its designated boundary with context intact. A panic is not an
alternate user-error transport.

Diagnostics preserve root-cause identity and phase gating. Known source
locations take priority over generated or fallback locations; derived recovery
errors are suppressed only when an emitted root exists, while independent
errors stay visible. Diagnostics remain structured through semantic and query
phases; terminal formatting belongs to the reporting boundary.

Fallible operations are transactional at their ownership boundary: a failure
leaves every live resource reachable by its owner, preserves the primary error,
and has a defined retry or cleanup path. Persistent records are validated
completely (identity, schema, bounds, trailing input) and read through a
no-follow regular-file boundary; malformed, redirected, or incompatible data is
a miss or typed failure, never interpreted optimistically. Semantic checks and
later lowering agree on the same accepted value set.

### Incremental equals clean

The typed query and fact graph is the single dependency and invalidation
source. A persistent cache is an additional storage layer, not a replacement for
in-memory dependency tracking. Incremental recomputation must be observably
equivalent to clean recomputation for the same inputs; invalidation tests
exercise edit sequences and compare products rather than asserting a cache hit.
A persistent product needs a measured dependency cut and a stable cross-session
identity.

### Concurrency

Parallel work uses the session-owned executor and explicit resource accounting.
Each task has a partition identity, cancellation behavior, owned mutable state,
and immutable shared context. Results merge deterministically, independent of
completion order. CPU and native-memory limits are shared by compiler workers
and inherited build tools; a worker count alone is not a memory policy.

## Where Documentation Lives

User documentation (`docs/guide/`, `docs/reference/`, `lib/README.md`) describes
the language and toolchain without compiler internals. Developer documentation
(`docs/dev/`, crate READMEs, `maintain/`) owns contributor policy and
subsystem contracts. Crate-level `//!` docs own a crate's internal invariants,
and source comments explain local reasoning. Each rule has one owner document,
updated in the same change that establishes it; other documents link to it.
Dated measurements and completion notes belong in Git history, not in docs.

## Review

Pull requests should be understandable by a human reviewer. Explain the design
reasoning for non-trivial changes, especially when behavior changes across
compiler phases. Review changes at the boundary they affect: a file or crate
split needs stable data or algorithm ownership, and a shared abstraction must
remove a real ownership or dependency problem.

The maintainer may ask for changes, split a proposal into smaller steps, or
close a proposal that does not fit the project direction.

## AI Usage

AI-assisted work is welcome. AI is a tool like an editor or search engine, and
using it does not change who is responsible for the work: tools do not own
code, people do.

- The submitter is responsible for the result. Do not submit code, tests, or
  documentation you cannot understand, explain, review, and maintain.
- AI-assisted code is reviewed line by line by a human before submission, not
  skimmed as a final diff.
- The submitter is responsible for provenance and licensing. AI must not be used
  to launder copyrighted code, incompatibly licensed material, private source,
  or text copied without permission.
- Routine assistance needs no label. When a substantial part of a change was
  produced with AI assistance, mention it briefly in the pull request so
  reviewers can focus their validation.
- AI-assisted work meets the same standard as any contribution: it fits the
  language design and compiler architecture, includes tests, passes the gates,
  and contains nothing the author cannot justify. If the submitter cannot
  explain part of a change when asked, the change is not ready.

The same rules apply to the maintainer's own use of AI tools.

## Licensing

Compiler implementation contributions are made under `GPL-3.0-or-later`, as
described in [LICENSE.md](../../LICENSE.md). Documentation has no separate
license grant unless its owning file states one explicitly.
