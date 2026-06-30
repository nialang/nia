# Builtin Source Model Draft

## Direction

`std` is part of the language surface. Compiler-backed operations should have
source-visible declarations under `std::builtin` so their API shape, names, and
documentation live with the program instead of only in Rust descriptors.

The canonical public home is `std::builtin`. `std::prelude` may re-export names
that should be ambient language vocabulary, but canonical documentation should
remain in `std::builtin`.

## Builtin Traits

Builtin traits are declared with trait-level `@[builtin("Name")]`:

```nia
@[builtin("Iterator")]
pub trait Iterator {
    type Item;

    fn next(&mut self) ?[Self as Iterator]::Item;
}
```

Method-level builtin markers such as `@[builtin("Iterator::next")]` are not
needed while the containing trait is already marked. A method marker should only
be introduced later if a real semantic split requires per-method compiler
identity.

The Rust descriptor remains the compiler's semantic authority for now. Directly
generating `nia-ids` descriptors from `lib/std/builtin.nia` would tie the lowest
ID crate to parser/std loading and complicate bootstrap. The near-term guard is
a consistency test that compares source declarations with Rust descriptors:
trait names, generic counts, associated types, supertraits, required methods,
receiver kinds, and method names.

Longer term, a small declarative builtin spec could generate both Rust
descriptors and std declarations, but `nia-ids` should not parse full std source
as part of normal compilation.

## Builtin Methods And Intrinsic Impl Declarations

Suffix methods such as `.len()`, `.ptr()`, `.ptr_mut()`, `.start()`, `.end()`,
`.char()`, `.iter()`, and `.next()` should be represented by builtin traits in
`std::builtin`, and their compiler-provided implementations should eventually
be represented by source-visible builtin `extend` declarations.

The target shape is roughly:

```nia
@[builtin("Len")]
pub trait Len {
    fn len(&self) usize;
}

@[builtin("array.Len")]
extend[T, comptime N: usize] [N]T : Len {
    fn len(&self) usize;
}
```

That requires first-class support for `@[builtin]` on `extend` blocks and
bodyless methods in builtin extend declarations. This is intentionally separate
from the current module split.

## Primitive Types And Associated Values

Primitive scalar types remain language primitives, but their associated values
such as `i32::MIN` and `i32::MAX` should also get a source-visible home. The
current implementation resolves them as compiler builtin associated values.

A likely source shape is a builtin inherent extension:

```nia
@[builtin("i32")]
extend i32 {
    pub comptime MIN: i32;
    pub comptime MAX: i32;
}
```

If existing `extend` associated comptime values can express this, prefer that
over adding new `primitive` item syntax. If they cannot, design the missing
language feature explicitly instead of encoding primitive docs as comments.

## Module Layout

`std::builtin` should be split by semantic area and re-exported from the root:

- `ops`: operator traits.
- `marker`: marker traits such as `Sized` and `Unsized`.
- `place`: deref, indexing, slicing, pointer, length, range-bound, and char
  traits.
- `iter`: iteration traits.
- `layout`: `size`, `align`, and `offset`.
- `mem`: memory intrinsics.
- `simd`: SIMD intrinsics.
- `bits`: bit-counting intrinsics.
- `atomic`: atomic intrinsics.
- `control`: `trap`, `error`, and embedding/control builtins.
- `primitive`: future source declarations for primitive associated values.

The root module should remain a facade so `std::builtin::size` and prelude
re-exports keep a stable, simple public path.
