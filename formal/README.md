# Nia Formal Models

This directory holds Lean 4 models of selected compiler contracts. A model
states a rule precisely and proves its properties; it does not verify the Rust
implementation. Each model is tied to the compiler through a manifest that the
Lean model and a compiler test both read, so the two cannot silently cover
different cases.

The directory is a Lake package. CI builds it in
[`.github/workflows/formal.yml`](../.github/workflows/formal.yml), and the
toolchain is pinned in [`lean-toolchain`](lean-toolchain).

## Models

| Module | Contract | Compiler test |
| --- | --- | --- |
| [`NiaFormal/ValueInference.lean`](NiaFormal/ValueInference.lean) | Contextual typing of value construction | [`value_inference_matrix.rs`](../crates/nia-driver/src/tests/value_inference_matrix.rs) |
| [`NiaFormal/ModuleValueTypes.lean`](NiaFormal/ModuleValueTypes.lean) | Bounded growth of inferred global and const type facts | [`semantic_diagnostics.rs`](../crates/nia-compiler-query/src/query/tests/semantic_diagnostics.rs) |
| [`NiaFormal/ModuleVisibility.lean`](NiaFormal/ModuleVisibility.lean) | Explicit extension visibility and separate trait-witness closure | [`imports.rs`](../crates/nia-driver/src/tests/imports.rs) |
| [`NiaFormal/ScopedExports.lean`](NiaFormal/ScopedExports.lean) | Restricted name exports and authorized provider publication | [`using.rs`](../crates/nia-driver/src/tests/using.rs) |
| [`NiaFormal/ReachabilityRetraction.lean`](NiaFormal/ReachabilityRetraction.lean) | Exactness of over-delete/rederive after dependency-edge changes | Pending reachability integration tests |

### Value construction inference

The model starts after name resolution. It describes how an expected value
type reaches the leaves of a constructor expression: a stack of frames (tuple
elements, array elements, `?x`, `!x`, `x!`, blocks, branches, implicit
addresses, array-to-slice views, aggregate fields, and enum payloads) projects
the expected type down to a literal, and the literal is typed in that context.
It proves that:

- rebuilding a projected frame stack yields the expected type again, so
  contextual inference never changes the type it was given;
- unsuffixed integers default to `i32` and floats to `f64`, `null` needs an
  optional context, and a literal in an incompatible context is rejected;
- string and byte-string literals reach slice contexts through an implicit
  address;
- the source spelling of a constructor (typed, qualified, omitted, or through
  an alias) does not change the resolved nominal identity, and declarations in
  different modules stay distinct.

[`NiaFormal/value_inference_matrix.json`](NiaFormal/value_inference_matrix.json)
lists the literal forms, constructor forms, AST expression forms, and compiler
cases. The links are:

1. The Lean build checks that every inventory in the model equals the
   corresponding manifest list (`native_decide` examples at the end of the
   module).
2. The compiler test checks that its expression cases equal
   `compiler_expression_cases` and that every constructor form maps to one of
   them, then compiles every case in both a runtime function and a `const fun`.
3. The Rust inference pass classifies `ExprKind` with an exhaustive match, so
   a new AST variant cannot compile without a classification.

The third link does not reach the manifest: nothing checks that
`ast_expression_forms` names exactly the `ExprKind` variants, so a new variant
must be added to the manifest and the model by hand.

### Module value type facts

Globals and top-level constants keep different initialization rules: globals
need runtime static storage, while constants are compile-time values. Their
runtime value types now use one module-level inference query and dependency
fixed point. The Lean model proves the finite bound for facts that only grow;
it does not model the compiler's body checker or cross-module query graph.

[`NiaFormal/module_value_types_matrix.json`](NiaFormal/module_value_types_matrix.json)
lists the declaration kinds and dependency cases exercised by compiler tests:
forward const use, a const chain, cross-module const lookup, incremental
invalidation, source-level cycle reporting, and forward global inference. The
Lean model checks the inventory, and the compiler tests read the same file.

### Module visibility

`ModuleVisibility.lean` models the source-visible extension set separately
from trait implementation witnesses. Extension methods follow the caller's
direct module imports, explicit `pub using` module/provider re-exports, provider
edges published by the facade of a directly imported package, and provider
demands recorded for that caller. Imports and provider demands belonging to an
imported module do not become caller imports. Importing a nominal type does not
implicitly import external extension providers; only members declared in the
nominal type's owner module use the separate nominal-owner lookup. Trait
witnesses remain a transitive closure of their own provider edges. The model
proves these sets have distinct propagation rules and that public re-exports are
followed transitively without hard-coding a package such as `std`.

[`NiaFormal/visibility_matrix.json`](NiaFormal/visibility_matrix.json) lists
the modeled rules and the driver cases that exercise import leakage,
nominal-type imports, and explicit re-exports. The Lean build checks the rule
inventory; the driver tests read the same JSON and check that their cases match
the manifest. This still does not prove
the Rust loader and query implementation correct, so the end-to-end cases are
required alongside the model.

### Scoped exports

The extension-visibility model starts with resolved provider edges. It does not
model access restrictions on exported names. [ScopedExports.lean](NiaFormal/ScopedExports.lean)
covers that separate contract after host-path resolution: alternative routes are
a union, each route requires all its restrictions, and re-exporting a selected
name cannot remove that name's earlier restriction. It proves that intersecting
routes requires both scopes, removing subsumed routes preserves access, and
enum variants retain their enum's scope. It distinguishes item/module access
restrictions and proves that provider reachability requires a permitted incoming
publication route or a direct import. Lookup classification distinguishes absent
names from access denial. Provider scope projection removes namespace declaration
restrictions while preserving every earlier re-export directive restriction.

A host namespace only needs to be accessible to the exporting module. A public
member selected from a private implementation module retains the member's own
scope. Publishing a provider grants an explicit capability under the publishing
directive's scope and earlier publication restrictions; it does not grant access
to the private module namespace. Source selection uses the exporting module's
ordinary access permissions for every selector; publishing a name retains its
access routes instead of requiring unrestricted visibility.

[export_scope_matrix.json](NiaFormal/export_scope_matrix.json)
connects these rules to cross-package driver regressions in
[using.rs](../crates/nia-driver/src/tests/using.rs) for restricted values,
types, enum aliases, alias chains, private implementation facades, and restricted
provider publication. The algebra accepts an arbitrary grant predicate; it does
not prove the Rust ancestry predicate, loader scheduling, or query implementation.
Those remain covered by compiler unit, integration, and invalidation tests.

The rule inventory is also exported by `nia-defs::ExportScope` and compared with
the same JSON in the driver test. Adding a scoped-export rule therefore requires
updating the implementation inventory, the Rust manifest assertion, and the Lean
inventory check together.

### Incremental reachability retraction

`ReachabilityRetraction.lean` models an edge graph whose changed outgoing
dependencies are owned by a known set of source nodes. It defines the affected
region as every node reachable from a changed source through either the old or
new graph, retains old reachable nodes outside that region, and rederives the
region from roots and edges crossing from retained nodes. The theorem proves
this result equals clean reachability in the new graph. Using the union graph
for the affected region covers both removed and added edges, including cycles
and nodes with multiple paths of support.

The theorem is an algorithm contract, not a proof that compiler reachability
facts currently retain complete edge provenance. The executable closure also
contains trait, vtable, generic-instance, and module-owner facts. Rust
integration must establish their source ownership before using localized
retraction; until then the compiler keeps its conservative full reset.

## Building

Install Lean with [elan](https://github.com/leanprover/elan); it selects the
pinned toolchain. Then build from this directory:

```sh
cd formal
lake build
```

The manifest is declared as a Lake input, so editing it rebuilds the model.
Manifest changes also trigger the build/std workflow, which runs the Rust
matrix tests; changes to the checker sources trigger the formal workflow.
The compiler side runs with the driver tests:

```sh
cargo test -p nia-driver value_inference_matrix
```

## Changing a model

- Changing a rule: update the model and its proofs, the manifest, and the
  compiler test in the same change, and build both sides.
- Adding a compiler case: add it to the test's case table and the corresponding
  manifest inventory in the same order.
- Adding a model: add a module under `NiaFormal/`, import it from
  [`NiaFormal.lean`](NiaFormal.lean), declare any file it reads with
  `include_str` as an `[[input_file]]` in [`lakefile.toml`](lakefile.toml) and
  list it in the library's `needs`, add the compiler test that reads the same
  manifest, and add the paths the model depends on to the workflow's filters.

`native_decide` trusts the Lean compiler's evaluation of the decision
procedure; the inventory checks rely on it, the theorems do not.
