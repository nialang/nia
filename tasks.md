# Query Pipeline Completion Tasks

This branch is complete only after the compiler pipeline is query-shaped end to end, with driver
kept as a thin facade and temporary query migration scaffolding removed.

## 1. Provider-based compiler query layer

- [ ] Add an explicit provider table to `nia-compiler-query`.
- [ ] Move query execution bodies behind providers instead of hard-coding pass calls directly in
      `impl QueryKey`.
- [ ] Keep default providers assembled in `nia-compiler-query`, not `nia-driver`.
- [ ] Add focused tests proving provider override is possible for at least one small query.
- [ ] Commit this stage.

Acceptance:

- `nia-driver` still only loads sources and calls the compiler query facade.
- Existing check/codegen tests still pass.
- No pass semantics are moved into `nia-query`; it remains a generic query runtime.

## 2. Query-driven loading boundary

- [ ] Move source loading, parsing, import discovery, and module graph construction into
      query-shaped units.
- [ ] Introduce source/parse/import/module graph queries or equivalent typed keys.
- [ ] Keep filesystem access at the loader/interface boundary and keep compiler semantic queries
      independent from CLI concerns.
- [ ] Preserve current public `load_program` and `check_program` APIs as thin wrappers.
- [ ] Commit this stage.

Acceptance:

- Loading no longer relies on one opaque BFS function as the only source of module state.
- Parse diagnostics and import diagnostics still point at the correct source paths.
- Existing multi-module tests still pass.

## 3. Remove bulk program-input dependencies from passes

- [ ] Audit `all_defs`, `all_modules`, `program_*` maps, and cross-module callback inputs.
- [ ] Replace the highest-risk pass inputs with query context/provider traits where practical.
- [ ] Prioritize comptime/body-check/layout/type/value resolve paths because recent bugs clustered
      there.
- [ ] Keep pass crates independent; they should consume explicit contexts, not depend on
      `nia-compiler-query`.
- [ ] Commit this stage.

Acceptance:

- Cross-module lookups are expressed as typed provider/query access, not ad hoc Vec scans where
  that matters for correctness.
- Imported nominal types, imported comptime values, and imported array lengths remain covered by
  check plus emit tests.

## 4. Query runtime hardening

- [ ] Add query descriptions suitable for diagnostics/profiling.
- [ ] Improve cycle diagnostics beyond panic-only reporting.
- [ ] Add dependency recording hooks that can support later invalidation.
- [ ] Decide whether cache specialization or sharding is needed now; implement only if the current
      architecture benefits.
- [ ] Commit this stage.

Acceptance:

- Query cycles produce actionable diagnostics or structured errors at compiler-query boundaries.
- The runtime remains generic and does not know Nia compiler semantics.

## 5. Final cleanup

- [ ] Remove temporary compatibility helpers and obsolete comments.
- [ ] Re-run architecture scans for query logic in `nia-driver` and stale bulk program plumbing.
- [ ] Run `cargo test`.
- [ ] Run `cargo clippy --all-targets -- -D warnings`.
- [ ] Delete this file.
- [ ] Commit final cleanup.

Acceptance:

- Working tree is clean after the final commit.
- This branch has no `tasks.md`.
- The query pipeline is clean enough to serve as the 0.2.x compiler pipeline baseline.
