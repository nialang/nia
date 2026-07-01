# Builtin Source Model Draft

## Status

The first builtin-source pass is mostly complete:

- `std::builtin` is the canonical public home for compiler-backed operations.
- Builtin traits and functions have source declarations under `lib/std/builtin`.
- Builtin modules are split by semantic area and re-exported from
  `std::builtin`.
- `@[builtin]` is accepted on `extend` declarations, bodyless builtin methods are
  accepted, and builtin impl declarations are skipped as ordinary user impls.
- Descriptor/source consistency tests guard builtin functions, builtin traits,
  builtin associated types/comptimes, and the currently declared builtin impl
  markers.
- `memcpy`, `memmove`, and `memset` now have source-level signatures instead of
  `void` argument placeholders.
- Trait associated comptime requirements are parsed, signed, checked on impls,
  and usable through `[T as Trait]::Name` projections.
- `Simd` and `SimdMask` are source-declared builtin traits over native vector
  types. SIMD builtin function declarations now use `[V as Simd]::Lane` instead
  of `void` placeholders.

This draft now tracks the remaining design holes rather than the completed
module split.

## Completed: SIMD Source Shape

Nia already has first-class native vector types such as `u8x16`, `i32x4`, and
`boolx8`. The compiler represents these as vector types directly; they should
not be replaced by a second source-level `Vector[T, N]` type constructor.

The problem is not the lack of vector types. The problem is expressing generic
constraints over an arbitrary vector type:

```nia
std::builtin::splat[u8x16](7u8)
std::builtin::extract(vector, index)
std::builtin::insert(vector, index, value)
std::builtin::bitmask(mask)
```

The desired source model is a builtin trait over existing vector types:

```nia
@[builtin("Simd")]
pub trait Simd {
    type Lane;
    comptime Lanes: usize;
}

@[builtin("SimdMask")]
pub trait SimdMask : Simd {}
```

Then the SIMD functions can be declared without `void` placeholders:

```nia
@[builtin("splat")]
pub fn splat[V](value: [V as Simd]::Lane) V
where V: Simd;

@[builtin("extract")]
pub fn extract[V](vector: V, index: usize) [V as Simd]::Lane
where V: Simd;

@[builtin("insert")]
pub fn insert[V](vector: V, index: usize, value: [V as Simd]::Lane) V
where V: Simd;

@[builtin("bitmask")]
pub fn bitmask[V](vector: V) usize
where V: SimdMask;
```

`SimdMask` should probably be a separate builtin trait rather than only
`Simd[Lane = bool]`, because `bitmask` also has a semantic lane-count limit.
Today that limit is `Lanes <= 64`.

Implemented projection syntax:

```nia
[V as Simd]::Lanes
```

This mirrors associated type projection. The type checker currently gives
builtin associated comptime projections their declared type. Lowering concrete
`[u8x16 as Simd]::Lanes` to a comptime value remains separate future work,
needed before lane-count constraints can be expressed in source signatures.

## Open: Inline Assembly Config

`std::builtin::asm` still uses a `void` parameter placeholder because the
argument is not an ordinary runtime value. The checker currently accepts only an
untyped struct literal with compiler-validated fields.

Do not change this to `&void`: that would suggest runtime reference semantics
where none exist. The cleaner long-term shape is probably an opaque builtin
config type:

```nia
@[builtin("AsmConfig")]
pub type AsmConfig;

@[builtin("asm")]
pub fn asm(config: AsmConfig) void;
```

That should remain lower priority than the SIMD/associated-comptime design.

## Open: Primitive Associated Values

Primitive scalar types remain language primitives, but associated values such as
`i32::MIN` and `i32::MAX` should get source-visible declarations. Existing
`extend` associated comptime values are likely the right source shape:

```nia
@[builtin("i32")]
extend i32 {
    pub comptime MIN: i32 = -2147483648i32;
    pub comptime MAX: i32 = 2147483647i32;
}
```

This should be revisited after trait associated comptime values are designed, so
primitive inherent comptime values and trait associated comptime values share
one coherent model.
