# Pattern Matching Design And Audit Notes

This document records the proof obligations and implementation choices behind
Nia's pattern usefulness and exhaustiveness checks. It is intentionally more
detailed than the language specification: the specification describes the user
model, while this document explains why the implementation is conservative and
where an audit should look for mismatches.

## References

The primary reference is Luc Maranget, "Warnings for pattern matching",
*Journal of Functional Programming* 17(3), 2007, pp. 387-421,
[doi:10.1017/S0956796806006223](https://doi.org/10.1017/S0956796806006223).
The paper defines the usefulness relation used to detect both unreachable
patterns and missing cases. Nia follows its constructor-matrix structure, but
extends the domain model for scalar intervals, open enums, and patterns whose
matching behavior is intentionally opaque.

For the later lowering boundary, see Luc Maranget, "Compiling Pattern Matching
to Good Decision Trees", ML 2008, pp. 17-24,
[doi:10.1145/1411304.1411314](https://doi.org/10.1145/1411304.1411314).
Nia does not claim to implement that paper's decision-tree compiler here:
`nia-pattern-analysis` answers coverage questions only. Runtime lowering owns
the actual tag tests, field projections, and branch construction.

The user-facing comparison points are the [Rust Reference pattern
chapter](https://doc.rust-lang.org/reference/patterns.html) and the
[OCaml pattern matching manual](https://ocaml.org/manual/patterns.html). They
are useful syntax and behavior references, not normative sources for Nia's
implementation.

## Ownership And Data Flow

The implementation is deliberately split into three layers:

1. `nia-pattern-analysis` is pure. It receives canonical constructor ids,
   constructor field types, scalar bounds, and normalized patterns. It has no
   AST, resolver, diagnostic, or lowering dependency.
2. `nia-body-check/src/patterns/analysis.rs` converts checked runtime AST
   patterns and normalized types into the pure representation.
3. `nia-const-check/src/analyzer/switch_patterns/coverage.rs` performs the
   equivalent conversion for resolved const IR. Both adapters call the same
   pure functions and format their own source-aware diagnostics.

For every switch arm, the front end first performs ordinary type checking and
constant resolution. It then asks:

```text
useful_witness(previous_matrix, current_pattern, target_type)
```

If the result is `None`, the arm is unreachable and is not added to the matrix.
If it returns a witness, the normalized arm is appended. After all arms, the
front end asks whether a wildcard query is useful. A returned witness is the
missing-pattern diagnostic; `None` means exhaustive.

This ordering matters. Invalid, out-of-domain, or otherwise opaque source
patterns must not accidentally contribute coverage merely because they were
present in the source.

## Constructor Matrices

A matrix is a list of rows, one row per previous pattern and one column per
matched value. A query is another row whose usefulness is being tested. The
algorithm repeatedly examines the first column:

- A constructor pattern specializes the matrix to that constructor and replaces
  the constructor with its fields.
- A wildcard query considers each constructor in the domain and recursively
  specializes the corresponding matrix.
- The default matrix retains rows whose first pattern can match values outside
  the known constructor set. It is used for wildcard queries over open domains.

Constructor ids are semantic identities, not display names. Constructor field
types and field order must be supplied in the same canonical order consumed by
runtime/const lowering. This is the central soundness invariant:

```text
analysis constructor fields[i] == lowered constructor field_defs[i]
```

If this correspondence changes, coverage can be reported for a value that the
generated matcher does not actually destructure. The adapters therefore use
declaration order and insert wildcard children for omitted named fields.

The algorithm has an early wildcard-row shortcut. A wildcard row covers the
remaining product directly, which both matches the matrix semantics and stops
recursive types from being expanded indefinitely.

## Domain Classes

`Domain` has four cases:

- `Finite(constructors)`: every constructor is known. This models closed enums,
  tuples, pointers, optionals, error unions, and nominal structs. An empty
  constructor list models an uninhabited type.
- `Open(constructors)`: the listed constructors are known, but unnamed values
  may exist. Open enums therefore require a wildcard even when all named
  variants appear in the matrix.
- `Scalar { min, max, complete }`: integer and boolean values are represented by
  intervals. Endpoints partition the domain without enumerating every integer.
  `complete = false` means the adapter cannot represent the entire backing
  domain precisely, so a wildcard is still required for exhaustiveness.
- `Opaque`: no sound finite interpretation is available. Opaque expressions
  remain useful unless shadowed, but never prove exhaustiveness.

Scalar patterns are clipped and validated against the target domain before they
reach the matrix. Empty ranges and reversed ranges are rejected by the front
end. The pure crate rejects scalar patterns for non-scalar domains and never
uses an out-of-domain interval as coverage.

The current deliberate conservative case is `u128` (and target-dependent wide
`usize` representations that cannot be losslessly represented by the analysis
integer). Such switches must provide `_`; this avoids claiming completeness
from a truncated mathematical representation.

## Nominal Rest Patterns

`Point { x, .. }` and `Event::Resize { width, .. }` are source syntax only. The
parser records the terminal rest marker, then the typed adapters expand every
omitted declaration field to `Wildcard` in declaration order. No synthetic
field or fake constructor is introduced. Consequently:

- `Point { .. }` is irrefutable for the `Point` constructor;
- `Event::Resize { .. }` covers every payload of `Resize`, but not other event
  variants;
- without `..`, every declaration field must be named;
- duplicate, unknown, or non-terminal `..` forms are rejected before analysis.

This expansion is also what keeps const pattern matching, runtime matching, and
lowered field indices in agreement.

## Soundness And Diagnostic Rules

The following rules are intentional and should be preserved in refactors:

- A fully shadowed arm is an error, even if its body has effects or exits.
- Every `switch`, including effect-only switches, is exhaustive before lowering;
  this prevents an uninitialized switch-result temporary from being reached.
- A missing witness is an explanation, not a new accepted syntax. It is a
  concrete sub-pattern produced by the usefulness query and must be valid for
  the target domain.
- Opaque and invalid patterns do not establish coverage.
- Open domains always retain an unknown remainder until a wildcard covers it.
- Constructor ids and arities are validated; an adapter failure becomes a
  diagnostic rather than silently degrading to exhaustive.
- Const evaluation remains path-driven. The shared matrix check protects static
  const-switch typing, while whole-function control-flow soundness remains the
  responsibility of `nia-body-check`.

## Audit Checklist

When changing pattern syntax, type lowering, or switch lowering, check all of
the following layers:

- parser AST representation and recovery diagnostics;
- runtime and const pattern normalization;
- constructor identity and declaration field order;
- finite/open/opaque domain classification;
- scalar clipping and target-width representation;
- usefulness before matrix insertion;
- missing witness formatting;
- irrefutable `let`/`for` restrictions;
- runtime lowering and const evaluator field projections;
- parser, body-check, const-check, lowering, and executable tests.

The pure algorithm tests cover cross-product coverage, nested constructors,
shadowing, boolean/scalar holes, open domains, opaque patterns, range clipping,
and incomplete wide scalar representations. The adapter tests additionally
cover nominal rest patterns and runtime/const parity.
