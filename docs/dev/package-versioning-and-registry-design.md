# Package Versioning and Registry Design

This document proposes the next package-management design stage. It is a
design boundary, not an implementation specification: resolver and manifest
changes should be split into reviewed tasks after the policy decisions below
are accepted.

## Current behavior

`nia.toml` already requires a package version string and `PackageId` includes
that string in compiler identity. The version is currently validated as a
bounded, non-empty string; it is not parsed as SemVer and dependencies cannot
state version requirements.

Dependencies currently select exactly one source: a relative path or a Git
URL. A Git dependency may select one `rev`, `tag`, or `branch`, or follow
`HEAD`. `nia.lock` records the dependency alias, canonical source, selector,
resolved commit, and content hash. The cache is keyed by the content hash.
This makes a Git checkout reproducible after resolution, but provides no
version-based discovery or compatibility policy.

The graph currently resolves each declaration under a dependency-qualified
logical root. The compiler package identity includes namespace, package name,
and version, while build module roots are exposed under the dependency alias.
Two package versions could therefore have distinct compiler identities, but
the build layer can reject colliding module roots. The resolver has no global
version selection or conflict explanation today.

## Version requirements

Use SemVer 2.0.0 for published package versions and registry requirements.
Require normalized `major.minor.patch` versions for releases, permit prerelease
and build metadata according to SemVer, and reject malformed or abbreviated
versions in package manifests intended for publication. Existing local and
Git packages should retain their current free-form `package.version` behavior
initially, so this change does not silently break existing manifests. A later
manifest edition can opt into strict package-version validation if uniform
validation is required.

Registry dependency declarations should use an explicit requirement field:

```toml
[dependencies]
json = { registry = "nia", version = "^1.4" }
```

Use standard SemVer requirement syntax and make the default compatibility
operator explicit in documentation (`^` is not implied when omitted). Require
a requirement for registry sources. Keep Git `rev`, `tag`, and `branch`
selectors as source-selection controls; do not reinterpret them as SemVer.
Path dependencies remain local overrides and do not enter version solving.

## Version unification

For the first registry resolver, select one version of a package identity
(`namespace`, `name`) across the complete selected graph, including build,
runtime, and selected development scopes. Intersect all requirements and choose
the highest stable version satisfying them; use prereleases only when a
requirement explicitly admits that prerelease. If the intersection is empty,
report every requiring package and its constraint. This gives deterministic
resolution and prevents the current module-root layer from receiving two
incompatible packages under one dependency name.

Keep dependency aliases as the names visible to Nia source code and build
scripts. Allow an alias to differ from the package's published name, but reject
two declarations with the same alias in one package manifest. Do not support
multiple versions of one package identity in the initial registry release.
Supporting side-by-side versions later would need explicit aliasing and module
root rules through the compiler and build-script interfaces, not only a
resolver change.

## Registry and Git relationship

Add registry sources alongside, not in place of, Git and path sources. Registry
packages are the versioned, discoverable path for ordinary releases. Git
remains useful for unreleased code, forks, and pinned source dependencies;
path remains the local development override. No implicit fallback from a
missing registry package to Git is allowed.

Start with one configured default registry named `nia` and a versioned index
that maps `(namespace, package, version)` to an immutable archive URL and
cryptographic digest. The registry service is a distribution and discovery
layer; it does not own package semantics. Keep the index format and transport
replaceable so private registries can be added without changing dependency
identity. Defer publishing commands, namespace ownership policy, and registry
accounts to separate designs.

The registry archive digest is checked before extraction. Package source
identity is then checked using the existing manifest and source-selection
rules. Registry downloads use the existing cache area but have a distinct
source identity so equal bytes from unrelated registries cannot alias unless
the lock explicitly records them as the same source.

## Lockfile behavior

Keep `nia.lock` as the authoritative selected graph for all source types.
Registry lock entries should record registry identity, namespace, package
name, selected SemVer version, archive digest, and source fingerprint. They
must not depend on a mutable index response after locking. `--locked` must
resolve entirely from lock entries and cached artifacts, failing before a
network request if either is unavailable. `--offline` may use cached registry
metadata and artifacts only.

`nia update` should refresh registry selections within their declared
requirements and update Git selectors as it does now. An unchanged resolution
must leave the lockfile bytes untouched. A lockfile schema bump should be
introduced only with registry entries; existing Git-only lockfiles should
remain readable and should not need rewriting just because SemVer support is
added.

## Security and trust boundaries

Require HTTPS for the default public registry. Verify the index and archive
digests before parsing or extraction, bound metadata and archive sizes, reject
path traversal and symlinks that escape the package root, and publish cache
entries atomically only after validation. Registry credentials must use a
separate host-scoped credential lookup and must never enter manifests,
lockfiles, cache keys, or diagnostics. Private registry authentication and
signing policy need a focused follow-up before implementation.

## Proposed implementation order

1. Accept the version and unification policy, especially the no-side-by-side
   rule for the first release.
2. Add strict SemVer value and requirement parsing with isolated unit tests.
3. Extend manifest types and canonical graph fingerprints for registry
   declarations while leaving Git and path semantics unchanged.
4. Implement deterministic single-version solving with dependency-chain
   conflict diagnostics and adversarial graph tests.
5. Design and validate the registry index and archive format before wiring
   network access.
6. Add registry fetching, digest verification, atomic cache publication,
   lockfile entries, and `--locked`/`--offline` behavior.
7. Add update behavior, documentation, CLI reporting, and end-to-end tests.

## Decisions to confirm

- Use SemVer 2.0.0 requirements for registry dependencies while preserving
  current free-form versions for existing path and Git packages during the
  transition.
- Unify one selected version per `(namespace, name)` across the selected graph
  in the first registry release; reject incompatible constraints with the
  full dependency chain.
- Add a default `nia` registry beside existing Git and path sources, with
  immutable archives and digest-checked index records.
- Keep the registry transport and index replaceable, and defer publishing,
  namespace governance, private registry authentication, and signing policy
  to follow-up designs.
