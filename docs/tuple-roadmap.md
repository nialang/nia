# Tuple And Unit Roadmap

Status: active bounded compiler project

This roadmap owns the introduction of first-class tuple products, the replacement
of C-influenced `void` with the unit type `()`, and the separation of erased
pointer targets into the incomplete type `opaque`. It follows
[compiler-maintenance.md](compiler-maintenance.md) and is deleted when the
acceptance matrix is complete and the durable contract has moved into the
language, ABI, architecture, and maintenance documentation.

## 1. Semantic Contract

Nia uses the following cardinality model:

```text
|never| = 0
|()| = 1
|(T,)| = |T|
|(T1, ..., Tn)| = product of the element cardinalities
```

- `never` is the uninhabited zero type.
- `()` is the unit type, its only value and pattern, and the zero-element tuple.
- `(T,)` is a one-element tuple and is not definitionally equal to `T`.
- `(T1, ..., Tn)` is an ordered product. Element order is part of type identity,
  layout, mangling, serialization, and source behavior.
- Parentheses without a comma continue to group: `(T)` is `T` and `(value)` is
  `value`. The trailing comma distinguishes one-element tuples.
- Tuples do not implicitly pack from or unpack into arrays, structs, function
  arguments, or other tuple shapes.
- `void` is removed from the language without an alias or compatibility path.
  Omitted function returns become `()` and error-union success uses `!()`.
- `()` is a first-class sized zero-sized type with size 0 and alignment 1. It may
  be bound, stored, passed to generics, addressed, and used as an aggregate field
  or element.
- `opaque` is a distinct incomplete type used only as a direct pointer target.
  It has no values, layout, fields, or dereference operation. Explicit casts to
  `&opaque`, `&mut opaque`, `^opaque`, and `^mut opaque` erase pointee type while
  preserving pointer mutability and volatility rules. It is never equivalent to
  `()` or `never`.
- `never` remains impossible to construct. Diverging expressions coerce to an
  expected type; no value operation may read, store, or manufacture `never`.

## 2. Source Behavior

```nia
fn implicit_unit() {
    ()
}

fn explicit_unit() () {
    ()
}

fn fallible() Error!() {
    !()
}

let unit: () = ();
let single: (i32,) = (42,);
let pair: (i32, bool) = (42, true);
let (x, y) = pair;
pair.0 = 10;
```

- Tuple projection uses decimal fields `.0`, `.1`, and so on, checked against
  the tuple arity. Projection preserves ordinary place, mutability, borrow, and
  assignment behavior.
- Tuple patterns recurse through the existing pattern system in `let`, `for`,
  `if ... is`, and `switch`. Pattern arity must equal tuple arity.
- `let mut (x, y)` recursively marks all bindings mutable. `let (mut x, y)` is
  selective. Mutability belongs to bindings, not tuple types.
- Destructuring assignment is outside this project.
- There is no language-level tuple arity cap.
- Automatic structural equality, hashing, and ordering are outside this
  project. Libraries may later provide fixed-arity generic trait implementations.
- Empty blocks continue to evaluate to `()`. The old spelling `!{}` is removed
  from maintained sources and documentation, but a block remains a valid success
  operand because blocks are ordinary expressions.

## 3. ABI And Layout

- Tuple layout places fields in source order using the ordinary aggregate
  padding and alignment algorithm. `()` has size 0 and alignment 1.
- Nia ABI signatures and mangling encode tuple arity and every element type.
- C ABI rejects tuple parameters, tuple fields in extern aggregates, and
  non-empty tuple returns. A `()` return lowers to C `void` and has no runtime
  return value. Unit parameters are rejected.
- `opaque` is accepted only behind a pointer. Opaque pointers use the ordinary
  pointer ABI representation and cannot be dereferenced.
- LLVM lowering represents material tuple values and places consistently with
  semantic layout. Unit operations do not require an LLVM basic value.

## 4. Ownership By Phase

- Lexer/parser/AST own `opaque`, tuple type/value/pattern syntax, grouping
  disambiguation, tuple projection tokens, and syntax identity.
- Type lowering and `nia-ty` own tuple interning, substitution, normalization,
  display, traversal, stable identity, and the incomplete `opaque` boundary.
- Resolution and body checking own tuple inference, expected-type propagation,
  projection places, pattern arity and binding types, and invalid opaque use.
- Const IR/check/eval own tuple constants, projections, patterns, and the unit
  value without a `void` compatibility representation.
- Layout, ABI, function IR, LLVM codegen, mangling, summaries, query products,
  and persistent caches own complete tuple structural support and schema bumps.
- Standard library, examples, benchmarks, fixtures, and reference documentation
  own the repository-wide source migration.

## 5. Implementation Waves

1. Add tuple and `opaque` syntax/type identities, make `()` the omitted return
   type, establish unit values, and remove the primitive `void` model and token.
   Migrate erased pointer tests to `opaque` and reject incomplete values.
2. Implement tuple expression inference, expected-type checking, projections,
   place semantics, layout, and ordinary runtime lowering.
3. Implement recursive tuple patterns and binding types in every supported
   pattern context, including const evaluation where those contexts apply.
4. Complete tuple support in function IR, LLVM, ABI validation, mangling,
   provider summaries, incremental identities, and persistent caches.
5. Migrate standard library, examples, benchmarks, tests, and generated source
   from `void`/`!{}` to `()`/`!()`; remove obsolete diagnostics and searches.
6. Record the durable language and ABI contract, run broad acceptance, delete
   this roadmap, and commit the completed project.

Waves may be combined into a commit when their owning dependency graph cannot
be usefully validated separately. No commit is made merely to snapshot a
partially compiling transition.

## 6. Diagnostic Contract

Diagnostics must distinguish:

- missing commas in one-element tuple types, values, and patterns;
- tuple arity mismatch and out-of-range tuple projection;
- tuple/non-tuple mismatch without implicit packing;
- tuple use at a C ABI boundary;
- direct `opaque` values, fields, arrays, returns, or parameters;
- dereference of an opaque pointer;
- obsolete `void`, which lexes as an ordinary unresolved name after removal.

Diagnostics use source spans and the normal typed diagnostic channels. Invalid
source must not panic or create backend-only failures.

## 7. Acceptance Matrix

The project is complete only when all of the following are true:

- Parser tests cover unit, grouping, singleton/multi-element tuples, trailing
  commas, tuple patterns, projections, and malformed forms.
- Type tests prove ordered identity, singleton distinction, recursive
  substitution/normalization, expected-type inference, and no implicit packing.
- Body and flow tests cover bind/store/address/generic use of `()`, tuple places,
  mutation, nested patterns, arity diagnostics, and diverging expressions.
- Const tests cover unit and tuple construction, projection, nesting, and
  pattern matching without a legacy void representation.
- Layout/ABI tests cover zero and nonzero tuples, padding, nested ZSTs, opaque
  pointers, C-unit return lowering, and rejection of all unsupported C shapes.
- LLVM execution tests cover tuple construction, projection reads/writes,
  calls/returns, nesting, and unit success values.
- Incremental and signature-cache tests prove tuple and opaque identities survive
  round trips and invalidate on element order/type changes.
- Maintained Nia sources contain no `void` type spelling and prefer `!()` for
  unit success construction.
- Production Rust contains no `TypeKind::Void`, `PrimitiveTy::Void`, legacy void
  serialization tag, compatibility alias, or void-only materialization path.
- Stable language, ABI, architecture, and standard-library documentation describe
  the implemented contract and contain no stale void semantics.
- Focused suites and all repository gates pass:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
python3 -m unittest discover -s tools -p 'test*.py'
git diff --check
```

## 8. Retirement

Delete this file in the final project commit after every acceptance item is
closed and the stable reference documents own the durable semantics. Follow-on
work on structural anonymous records, closure environments, tuple trait
implementations, or destructuring assignment is explicitly separate.
