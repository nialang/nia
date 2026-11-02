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

## Building

Install Lean with [elan](https://github.com/leanprover/elan); it selects the
pinned toolchain. Then build from this directory:

```sh
cd formal
lake build
```

The manifest is declared as a Lake input, so editing it rebuilds the model.
The compiler side runs with the driver tests:

```sh
cargo test -p nia-driver value_inference_matrix
```

## Changing a model

- Changing a rule: update the model and its proofs, the manifest, and the
  compiler test in the same change, and build both sides.
- Adding a compiler case: add it to the test's case table and to
  `compiler_expression_cases` in the same order.
- Adding a model: add a module under `NiaFormal/`, import it from
  [`NiaFormal.lean`](NiaFormal.lean), declare any file it reads with
  `include_str` as an `[[input_file]]` in [`lakefile.toml`](lakefile.toml) and
  list it in the library's `needs`, add the compiler test that reads the same
  manifest, and add the paths the model depends on to the workflow's filters.

`native_decide` trusts the Lean compiler's evaluation of the decision
procedure; the inventory checks rely on it, the theorems do not.
