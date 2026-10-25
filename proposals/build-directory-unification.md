# Build Directory Unification

## Status

**Proposal** — not yet implemented

## Summary

Unify the current scattered build directories (`.nia-build/`, `.nia-cache/`, `.nia/packages/`) into a single consolidated structure under `build/`, aligning with established toolchain conventions (Rust's `target/`, Go's module cache) and simplifying maintenance, cleanup, and `.gitignore` management.

## Motivation

The current layout spreads build artifacts across three top-level directories:

```
project/
├── .nia/
│   └── packages/          # Downloaded dependencies
├── .nia-build/            # Build outputs (runner, plan, temporaries)
└── .nia-cache/            # All caches (runner, configure, action, compiler, test)
```

**Problems:**

1. **Cognitive overhead:** Users must remember three directories with similar names but different purposes
2. **Cleanup friction:** `nia clean` must coordinate across multiple roots; manual cleanup requires knowing all three
3. **`.gitignore` verbosity:** Three patterns where one would suffice
4. **Inconsistent naming:** Mixed convention (`.nia-*` vs `.nia/`)
5. **Discovery:** New users don't know which directory holds what

## Design

### Proposed Layout

Consolidate everything under a single `build/` directory:

```
build/
├── out/                   # Build outputs (current .nia-build/)
│   ├── runner/
│   ├── build-plan.bin
│   └── .build-* (transient files)
├── cache/                 # All caching layers (current .nia-cache/)
│   ├── runner/
│   ├── configure/
│   ├── action/
│   ├── compiler/
│   └── test/
└── packages/              # Downloaded dependencies (current .nia/packages/)
    ├── git/
    ├── registry/
    └── local/
```

**Rationale for `build/` as the root:**

- Already in `.gitignore` (line 2)
- Clear semantic boundary: everything build-related lives here
- Familiar to users coming from other ecosystems
- Single ignore pattern: `/build/`

### Cache Structure

The existing cache layers remain logically separate but physically unified under `build/cache/`:

| Layer | Purpose | Invalidation boundary |
|-------|---------|----------------------|
| `runner/` | Compiled `build.nia` executables | Runner source, stdlib, toolchain, host target |
| `configure/` | Frozen plans with observation manifests | Runner identity, package root observations |
| `action/` | Generated files, external command results, compiler references | Action inputs, tool identity, declared dependencies |
| `compiler/` | Objects, archives, link results (driver/linker owned) | Source manifests, optimization, targets |
| `test/` | Test completion witnesses | Test executable, inputs, environment |

**No merging of cache layers** — each has distinct lifecycle and correctness contracts. The unification is physical (directory hierarchy), not semantic.

### Migration Path

1. **Detection:** On first invocation after the change, detect old directories and:
   - Warn: "Build directories have moved. Run `nia clean --migrate` to move cached data or `nia clean --legacy` to remove old directories."
   - If both old and new exist, prefer new and ignore old (idempotent)

2. **Migration command:** `nia clean --migrate`
   - Move `.nia-cache/` → `build/cache/`
   - Move `.nia/packages/` → `build/packages/`
   - Remove `.nia-build/` (transient, not worth migrating)
   - Remove now-empty `.nia/` if it held only `packages/`

3. **Cleanup command:** `nia clean --legacy`
   - Remove `.nia-build/`, `.nia-cache/`, `.nia/` entirely

4. **Init behavior:** `nia init` generates updated `.gitignore`:
   ```gitignore
   /target/
   /build/
   ```
   (Remove `/nia-build/`, `/.nia-cache/`, `/.nia/` patterns)

### Affected Components

**Core paths (23 Rust files, ~159 matches):**

- `crates/nia-build/src/lib.rs`: `resolve_build_invocation()` — lines 2651-2654
  - Change `package_root.join(".nia-build")` → `package_root.join("build/out")`
  - Change `package_root.join(".nia-cache")` → `package_root.join("build/cache")`

- `crates/nia-package-metadata/src/lib.rs`: line 1067
  - Change `state_root.join(".nia/packages")` → `state_root.join("build/packages")`
  - Also transaction lock at line 2068: `.nia/packages.lock` → `build/packages.lock`

**Documentation (3 files, ~22 matches):**

- `docs/guide/build-scripts.md`: lines 65, 181, 187, 242
- `docs/dev/architecture.md`
- `docs/dev/testing.md`

**CLI surface:**

- `nia clean` — add `--migrate`, `--legacy` flags
- `nia init` — update generated `.gitignore`
- Help text and examples

### Compatibility

**Breaking change:** Existing checkouts will have stale directories. Users must either:
- Let them coexist (no functional impact, just clutter)
- Run `nia clean --migrate` once
- Manually remove old directories

**No protocol changes:** Cache formats, lockfile schema, and handoff protocols remain unchanged. Only physical paths change.

**Cross-version:** An old `nia` binary will recreate old directories if run in a migrated tree. This is acceptable during the transition window.

## Alternatives Considered

### Alternative A: Use `.nia/` as the Root

```
.nia/
├── build/
├── cache/
└── packages/
```

**Pros:** Keeps the `.nia` namespace
**Cons:** Hidden directory less discoverable; users expect `build/` or `target/` to be visible

### Alternative B: Match Rust Exactly

Use `target/` instead of `build/`:

```
target/
├── debug/
├── release/
└── ...
```

**Rejected:** `target/` is already gitignored and used by Rust tooling in the repo. Nia's build outputs don't map 1:1 to Rust's profile-based layout, so forcing the analogy would be confusing.

### Alternative C: Keep Three Directories, Just Rename

Make names consistent without unifying:
- `.nia-build/` → `.nia/build/`
- `.nia-cache/` → `.nia/cache/`
- (`.nia/packages/` unchanged)

**Rejected:** Doesn't solve the cleanup or `.gitignore` verbosity problems.

## Open Questions

1. **Cache sharing across packages:** Currently `artifact_cache_dir` can be shared via `NIA_TEST_SHARED_ARTIFACT_CACHE` (test-support only). Should this become a user-facing feature? If so, how does `build/cache/` in one package relate to another's?
   - **Lean toward:** Keep cache local-by-default. Shared cache can still read from `$HOME/.nia/cache/` if we add a config later, but the proposal focuses on per-package defaults.

2. **Lock file location:** Should `nia.lock` move to `build/nia.lock`?
   - **Lean toward:** No. `nia.lock` is committed and belongs at the root. `build/` is for gitignored ephemeral state.

3. **Transient files:** The current design uses `runner-{pid}-{seq}` temporaries. Should these move to `build/tmp/` or remain in `build/out/`?
   - **Lean toward:** Keep in `build/out/`. They're already cleaned up by `nia build` internally.

## Implementation Plan

### Phase 1: Path Constants

1. Define new constants in `nia-build` and `nia-package-metadata`:
   ```rust
   pub const BUILD_ROOT: &str = "build";
   pub const BUILD_OUT_DIR: &str = "build/out";
   pub const BUILD_CACHE_DIR: &str = "build/cache";
   pub const BUILD_PACKAGES_DIR: &str = "build/packages";
   ```

2. Update `resolve_build_invocation()` and `PackageGraph::with_lock_and_policy()`

### Phase 2: Migration Logic

1. Add detection in `resolve_build_invocation()`:
   ```rust
   fn detect_legacy_directories(package_root: &Path) -> LegacyLayout {
       // Check for .nia-build/, .nia-cache/, .nia/packages/
   }
   ```

2. Implement `nia clean --migrate` and `nia clean --legacy`

3. Add one-time warning on detection (suppressible with env var for CI)

### Phase 3: CLI and Documentation

1. Update `nia init` template
2. Rewrite documentation references
3. Update example `.gitignore` snippets

### Phase 4: Testing

1. Test migration from old layout to new
2. Test clean-slate init with new layout
3. Test behavior when both layouts coexist
4. Update fixture paths in `nia-cli/tests/`

### Phase 5: Deprecation Notice

1. Add release note to changelog
2. Update getting-started guide to mention migration for existing users

## Testing Strategy

- **Unit:** Path resolution tests in `nia-build` and `nia-package-metadata`
- **Integration:** `nia-cli/tests/build_cases.rs` — verify all caches land in `build/`
- **Migration:** Construct old-layout fixture, run `nia clean --migrate`, verify new layout
- **Coexistence:** Ensure new `nia` ignores old directories when both exist

## Success Criteria

1. All build artifacts land in `build/` in a fresh init
2. Migration from old layout completes without data loss
3. `.gitignore` requires only `/build/`
4. Documentation reflects new structure
5. Test suite passes with new paths
6. Windows and Linux CI both pass

## Future Extensions

- `nia clean --cache` — remove `build/cache/` only
- `nia clean --packages` — remove `build/packages/` only
- Shared cache config: `NIA_CACHE_DIR` to override `build/cache/`
- Per-cache-layer size limits and LRU eviction (separate proposal)
