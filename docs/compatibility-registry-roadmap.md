# Compatibility Registry Roadmap

Status: active

This project establishes one auditable control plane for Nia release,
toolchain, ABI, persisted-format, and fingerprint identities. It does not decide
whether the first public release will be `0.1.0`, create a compatibility epoch,
or reset existing schema numbers. Current identities remain unchanged until a
separate release proposal accepts that reset.

## Ownership Model

- The workspace package version is the sole release-version source.
- `nia-compat` is a dependency-free leaf crate that owns cross-component
  toolchain and ABI identities plus the registry of persisted format and cache
  namespace identities.
- Encoders, decoders, bounds, checksums, corruption handling, and retirement
  remain in their product-owning crates.
- Fingerprint domains remain beside their hash inputs, but must use one typed,
  auditable spelling and explicit version.
- In-memory sentinels, OS constants, sequence counters, enum tags, and resource
  limits are not compatibility identities and remain local.
- `lib/toolchain.meta` is generated resource data, not an independent authority.

## Phase 1: Compatibility And Format Registry

Status: complete

- Add `nia-compat` and migrate compiler version, resource layout, std, build
  protocol, mangling ABI, LLVM codegen ABI, persisted magic, and cache namespace
  identities.
- Generate and check `lib/toolchain.meta` from the registry.
- Remove owner-local duplicate identity constants.
- Add structural audits for release-version duplication and fingerprint-domain
  spelling.
- Replace current-version prose that can drift with registry references, while
  preserving behavioral documentation beside the owning implementation.

Acceptance:

- `rg` finds no duplicate definition of a registered identity outside
  `nia-compat`.
- Every persisted namespace derives its `vN` component from one registered
  schema and every registered format name and magic is unique.
- A stale `lib/toolchain.meta` fails normal workspace tests and the explicit
  registry check command.
- Existing toolchain, build-plan, cache, corruption, and relocation tests pass
  without compatibility readers or aliases.

## Phase 2: Typed Fingerprint Domains

Status: active

- Replace raw literal domain strings with typed owner-local constants.
- Audit domain spelling, version suffixes, deliberate reuse, and uniqueness of
  distinct semantic domains.
- Require a domain-version change whenever the hashed input contract changes.

Acceptance:

- Production fingerprint builders receive typed domains rather than ad hoc
  literals.
- The audit rejects unversioned domains and conflicting reuse.
- Incremental, relocation, cache invalidation, and clean/warm differential tests
  remain green.

## Phase 3: Release Reset Decision

Only a later release proposal may introduce a compatibility epoch or renumber
schemas. If accepted, it must define the legacy tag/branch, cache namespace
reset, generated manifest, baseline regeneration, source/API compatibility
policy, and complete removal of development-era readers and identities.

## Completion

This roadmap can be retired when all phases have accepted code and tests,
current numeric identities have no hand-maintained documentation duplicates,
and release/reset policy has either been implemented or explicitly deferred to
a separately owned release proposal.
