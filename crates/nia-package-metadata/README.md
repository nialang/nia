# nia-package-metadata

`nia-package-metadata` owns package identity, `nia.toml` manifests and
workspaces, dependency graph resolution, the registry client and cache, and
`nia.lock`. [`nia-build`](../nia-build/) calls it before running a build
script; Git checkouts go through [`nia-git`](../nia-git/). It also owns the
stable module, definition, and type identities that compiler-owned sections
embed; those encoders are documented in the crate's `//!` docs.

This README is the implementation contract for versions, resolution, the
registry formats, the lockfile, and package source identity. The user-facing
manifest syntax is in
[Build scripts](../../docs/guide/build-scripts.md#package-manifest).

| Source | Owns |
| --- | --- |
| [`src/lib.rs`](src/lib.rs) | Identities, manifests and workspaces, source fingerprints, graph resolution and the version solver, fetching, `nia.lock`, and `nia update` |
| [`src/registry.rs`](src/registry.rs) | Registry endpoints, index and archive validation, HTTPS fetch, and the validated cache |

## Package versions

Every package identity uses a strict SemVer 2.0.0 version, regardless of whether its source is a workspace member, path, Git checkout, or registry archive. Parse versions into a SemVer value at package metadata boundaries; do not retain an unchecked string as the identity. Prerelease and build metadata follow SemVer. The compiler's own `0.3.0-dev` version and the standard-library suffix are valid SemVer values.

Package release versions and source selectors are separate concepts. A package version identifies the package contents' declared release. Git `rev`, `tag`, and `branch` select source revisions and never substitute for, or relax, the package version. All path and Git manifests must declare valid SemVer too.

## Dependency declarations and resolution

Dependencies select exactly one explicit source: `path`, `git`, or `registry`. Registry dependencies require an explicit SemVer requirement:

```toml
[dependencies]
json = { registry = "nia", package = "acme/json", version = "^1.4" }
```

Use standard SemVer requirement syntax; do not infer a compatibility operator when it is omitted. Path dependencies remain direct local sources. Git source selectors retain their current semantics. Registry dependencies select immutable published package versions. No source fallback or implicit override is permitted.

The initial resolver selects one version per `(namespace, name)` across the complete selected graph, including all selected workspace members, build, runtime, and selected development scopes. Intersect all requirements and select the highest stable version satisfying them. SemVer build metadata remains part of package identity but does not affect precedence; when candidates have equal precedence, select the lexicographically greatest canonical version text so the result is independent of index record order. Prereleases are eligible only when a requirement explicitly admits that prerelease. On conflict, report each requiring package, its constraint, and its dependency chain. Reject aliases duplicated within a manifest; aliases may differ from package names. Multiple versions of the same identity are unsupported.

## Registry

Registries are named HTTPS endpoints alongside path and Git sources; `nia` is
the conventional default name. Only the root `nia.toml` (the workspace root in
a workspace) configures endpoints; a member `[registries]` table is rejected,
because it would otherwise be silently ignored:

```toml
[registries]
nia = "https://packages.example.test"
```

A versioned index maps `(namespace, package, version)` to an immutable archive
URL and cryptographic digest. Require HTTPS for the public registry. The index
and archive format are versioned and validated strictly; unknown schema
versions and malformed records fail closed. The registry distributes packages
but does not define package semantics. Publishing, namespace ownership,
account management, private registry authentication, and signing are separate
work.

Verify archive digests before extraction, bound metadata and archive sizes, reject path traversal and escaping symlinks, and publish cache entries atomically only after validation. Registry source identity remains distinct from Git/path identity. Within one resolved graph, a `(namespace, package)` identity must come from exactly one named registry; requesting it from multiple registries is an error rather than combining unrelated indexes. Credentials must never enter manifests, lockfiles, cache keys, or diagnostics.

The v1 index is a bounded TOML document with `schema_version = 1` and a
`packages` array. Each record contains `namespace`, `name`, `version`,
`archive_url`, and `archive_digest`. Package segments are lower-case ASCII
letters, digits, and `-`, and records are strictly sorted by their textual
`(namespace, name, version)` tuple. Versions are strict SemVer, archive URLs
are HTTPS without credentials, query, or fragment components, and digests use
`blake3:` followed by 64 lower-case hexadecimal characters. Unknown fields,
schema versions, duplicate records, and records outside the size or count
limits are rejected.

The v1 archive is an uncompressed binary stream: eight bytes `NIAPKG\0\0`,
little-endian `u32` schema `1`, and little-endian `u32` entry count. Every
entry contains a `u8` kind (`0` file, `1` directory), a little-endian `u32`
UTF-8 path length, the path bytes, a little-endian `u64` content length, and
the content bytes. Entries are strictly sorted by unique normalized relative
path. Paths cannot contain separators that escape the destination, empty,
`.` or `..` segments, backslashes, NULs, or colons. Directories have zero
content length; links and unknown entry kinds are not part of the format.
Every path component must also be valid on Windows: reject reserved device
names, `.git` aliases, trailing dots or spaces, control characters, invalid
filename punctuation, and components exceeding either 255 UTF-8 bytes or 255
UTF-16 code units. Reject paths that collide under Unicode lowercase folding,
including collisions in implicit parent directories, so one archive has the
same tree identity on case-sensitive and case-insensitive filesystems.
Indexes are limited to 1 MiB and 100,000 records. Archives are limited to
256 MiB, 100,000 entries, and 4096 bytes per path. The complete archive is
digest-checked before decoding, and extraction validates the complete tree,
writes to a newly allocated staging directory, and atomically publishes it.

## Lockfile

`nia.lock` schema version 2 is authoritative for every selected source. Each
entry has a tagged source record. Path records contain the owning package
identity, declared relative path, target package identity, and source
fingerprint. Git records contain the canonical URL, selector, resolved commit,
and source fingerprint. Registry records contain
registry identity, package namespace/name, selected SemVer version, archive
digest, and source fingerprint. They cannot depend on mutable index responses
after locking. A package coordinate can be bound to only one registry in a
lockfile. Schema version 1 and any unknown schema are rejected without
migration. `--locked` resolves only from lock entries and cached artifacts and
fails before network access if required data is unavailable. `--offline` uses
cached registry metadata and artifacts only. `nia update` refreshes registry
selections within requirements and Git selectors; the CLI reports registry
version changes separately from Git revision changes. Unchanged resolution
leaves lockfile bytes untouched. A workspace update holds the package state
lock across all member resolutions, intersects their registry constraints in
one graph, and either publishes the complete result or restores the original
lock before releasing the lock.
Rollback restores the original bytes, including formatting, or removes a
newly published lockfile when no lockfile existed before the update. The
transaction also covers reading the published result for change reporting.

Older lock schemas are not read, migrated, or rewritten; users regenerate the
lockfile with the current resolver.

## Graph restarts

Registry graph resolution may restart from the root when a later dependency
constraint changes an already selected release. Each restart carries explicit
candidate overrides and is bounded; the resolver reports a non-converging graph
as an error instead of accepting multiple versions of one registry package.
Cached locked releases contribute constraints to the same graph as newly
selected releases. A graph restart applies its candidate overrides before
reusing a cached selection, so adding a compatible requirement can select a
lower release without repeatedly restoring the previous locked version.

## Compatibility

The manifest, index, archive, and lockfile rules have no compatibility mode.
While Nia is unreleased, a change that makes the package model coherent is made
directly, and obsolete formats stay rejected.

## Source identity

A package source identity walks only the selected roots, skips excluded and
symlinked entries, and streams file contents into BLAKE3 fingerprints. The
walk is iterative and rejects source trees with more than one million regular
files before publishing an identity. Manifest source-root and exclude paths are
normalized as slash-separated, package-relative lexical paths before they enter
the identity, so redundant `.` components and separators cannot create a second
identity for one selection; parent, absolute, drive-prefixed, and backslash
paths are rejected. Fingerprints frame both the count and the entries of each
source-root and exclusion list.

Canonical package text escapes each namespace, name, and version component
before joining them as `namespace/name@version`. Percent signs, delimiters,
control bytes, and non-ASCII UTF-8 bytes use uppercase percent encoding, so
distinct component tuples cannot share a text identity. Local `nia.toml`
manifests are limited to 1 MiB; readers consume at most one byte beyond that
limit and reject oversized input before decoding UTF-8.
