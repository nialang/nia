# Package Versioning and Registry Design

This is the implementation contract for package identity, dependency resolution, and registry distribution. Nia is pre-release software: the new rules apply uniformly and obsolete manifest or lockfile formats are rejected.

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

The initial resolver selects one version per `(namespace, name)` across the complete selected graph, including build, runtime, and selected development scopes. Intersect all requirements and select the highest stable version satisfying them. Prereleases are eligible only when a requirement explicitly admits that prerelease. On conflict, report each requiring package, its constraint, and its dependency chain. Reject aliases duplicated within a manifest; aliases may differ from package names. Multiple versions of the same identity are unsupported.

## Registry

Support a configured default registry named `nia` alongside path and Git sources. A versioned index maps `(namespace, package, version)` to an immutable archive URL and cryptographic digest. Require HTTPS for the public registry. The index and archive format are versioned and validated strictly; unknown schema versions and malformed records fail closed. The registry distributes packages but does not define package semantics. Publishing, namespace ownership, account management, private registry authentication, and signing are separate work.

Verify archive digests before extraction, bound metadata and archive sizes, reject path traversal and escaping symlinks, and publish cache entries atomically only after validation. Registry source identity remains distinct from Git/path identity. Credentials must never enter manifests, lockfiles, cache keys, or diagnostics.

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
Indexes are limited to 1 MiB and 100,000 records. Archives are limited to
256 MiB, 100,000 entries, and 4096 bytes per path. The complete archive is
digest-checked before decoding, and extraction validates the complete tree,
writes to a newly allocated staging directory, and atomically publishes it.

## Lockfile

`nia.lock` is authoritative for every selected source. Registry entries record registry identity, package identity, selected SemVer version, archive digest, and source fingerprint. They cannot depend on mutable index responses after locking. `--locked` resolves only from lock entries and cached artifacts and fails before network access if required data is unavailable. `--offline` uses cached registry metadata and artifacts only. `nia update` refreshes registry selections within requirements and Git selectors as today; unchanged resolution leaves lockfile bytes untouched.

Introduce the new lock schema with registry support and reject every older schema. Do not dual-read, auto-migrate, or rewrite legacy lockfiles. Since Nia is still in development, users regenerate a lockfile using the new resolver.

## Implementation order

1. Enforce strict SemVer package identity for every source and provide parsed version/requirement value types.
2. Define registry dependency manifest types and canonical graph fingerprints.
3. Implement deterministic single-version solving and dependency-chain conflict diagnostics.
4. Specify and validate the versioned registry index and archive format.
5. Implement registry fetching, digest verification, atomic cache publication, the new lock schema, and `--locked`/`--offline` behavior.
6. Complete update behavior, documentation, CLI reporting, and end-to-end tests.

The manifest and lockfile rules have no compatibility mode. Changes needed to make the package model coherent should be made directly while the project is unreleased.
