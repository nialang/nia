//! Stateless ABI layout arithmetic shared by semantic layout consumers.

use nia_ty::{PrimitiveTy, RangeTyKind};

use super::{TargetDataLayout, TypeLayout};

/// Computes the scalar representation for one primitive type.
pub fn primitive_layout(primitive: PrimitiveTy, target: TargetDataLayout) -> TypeLayout {
    let (size, align) = match primitive {
        PrimitiveTy::I8 | PrimitiveTy::U8 | PrimitiveTy::Bool => (1, 1),
        PrimitiveTy::I16 | PrimitiveTy::U16 => (2, 2),
        PrimitiveTy::I32 | PrimitiveTy::U32 | PrimitiveTy::F32 | PrimitiveTy::Char => (4, 4),
        PrimitiveTy::I64 | PrimitiveTy::U64 | PrimitiveTy::F64 => (8, 8),
        PrimitiveTy::I128 | PrimitiveTy::U128 => (16, 16),
        PrimitiveTy::Isize | PrimitiveTy::Usize => (target.pointer_size, target.pointer_align),
        PrimitiveTy::Never => (0, 1),
    };
    TypeLayout { size, align }
}

/// Computes the two-word representation used by fat pointers and callable
/// views. A malformed target description must not wrap `pointer_size * 2` or
/// produce a layout with zero alignment.
pub fn fat_pointer_layout(target: TargetDataLayout) -> Option<TypeLayout> {
    (target.pointer_align != 0).then_some(TypeLayout {
        size: target.pointer_size.checked_mul(2)?,
        align: target.pointer_align,
    })
}

/// Computes a contiguous array representation with checked arithmetic.
pub fn array_layout(element: &TypeLayout, len: u64) -> Option<TypeLayout> {
    if element.align == 0 {
        return None;
    }
    Some(TypeLayout {
        size: element.size.checked_mul(len)?,
        align: element.align,
    })
}

/// Computes the product layout implied by a range kind.
///
/// A full range has no bound storage, one-sided ranges store one bound, and
/// two-sided ranges store both. A missing bound is only valid for `Full`.
pub fn range_layout(kind: RangeTyKind, bound: Option<&TypeLayout>) -> Option<TypeLayout> {
    let field_count = match kind {
        RangeTyKind::Exclusive | RangeTyKind::Inclusive => 2,
        RangeTyKind::From | RangeTyKind::To | RangeTyKind::ToInclusive => 1,
        RangeTyKind::Full => 0,
    };
    match (field_count, bound) {
        (0, None) => Some(TypeLayout { size: 0, align: 1 }),
        (0, Some(_)) | (_, None) => None,
        (_, Some(bound)) => array_layout(bound, field_count),
    }
}

/// Computes the layout of fields placed sequentially in declaration order.
///
/// This is the common ABI rule for tuples, closure environments, and other
/// anonymous product types whose consumers do not need individual offsets.
pub fn sequential_layout<'a>(
    fields: impl IntoIterator<Item = &'a TypeLayout>,
) -> Option<TypeLayout> {
    let mut size = 0u64;
    let mut max_align = 1u64;
    for field in fields {
        size = align_to(size, field.align)?.checked_add(field.size)?;
        max_align = max_align.max(field.align);
    }
    Some(TypeLayout {
        size: align_to(size, max_align)?,
        align: max_align,
    })
}

/// Computes the storage contract for a native vector.
///
/// Boolean lanes occupy one bit; other lanes use their primitive storage
/// width. Allocation is byte-rounded, aligned to the next power of two, and
/// finally padded to that alignment so all layout consumers agree with LLVM.
pub fn vector_layout(
    element: PrimitiveTy,
    lanes: u32,
    target: TargetDataLayout,
) -> Option<TypeLayout> {
    if !element.is_vector_element() || lanes == 0 {
        return None;
    }
    let lane_bits = if element == PrimitiveTy::Bool {
        1
    } else {
        primitive_layout(element, target).size.checked_mul(8)?
    };
    let store_bits = lane_bits.checked_mul(u64::from(lanes))?;
    let store_size = store_bits.checked_add(7)?.checked_div(8)?;
    let align = store_size.checked_next_power_of_two()?;
    Some(TypeLayout {
        size: align_to(store_size, align)?,
        align,
    })
}

/// Computes a union representation with all fields overlaid at offset zero.
pub fn union_layout_from_fields<'a>(
    fields: impl IntoIterator<Item = &'a TypeLayout>,
) -> Option<TypeLayout> {
    let mut max_size = 0u64;
    let mut max_align = 1u64;
    for field in fields {
        max_size = max_size.max(field.size);
        max_align = max_align.max(field.align);
    }
    Some(TypeLayout {
        size: align_to(max_size, max_align)?,
        align: max_align,
    })
}

pub(super) fn align_to(value: u64, align: u64) -> Option<u64> {
    if align == 0 {
        return None;
    }
    let remainder = value % align;
    if remainder == 0 {
        Some(value)
    } else {
        value.checked_add(align - remainder)
    }
}

/// Computes a tagged-union representation with a one-byte discriminant.
pub fn tagged_union_layout(payloads: &[TypeLayout]) -> Option<TypeLayout> {
    let tag = TypeLayout { size: 1, align: 1 };
    tagged_union_layout_with_tag(&tag, payloads)
}

pub(super) fn tagged_union_layout_with_tag(
    tag: &TypeLayout,
    payloads: &[TypeLayout],
) -> Option<TypeLayout> {
    let payload_size = payloads.iter().map(|layout| layout.size).max().unwrap_or(0);
    let payload_align = payloads
        .iter()
        .map(|layout| layout.align)
        .max()
        .unwrap_or(1);
    let align = tag.align.max(payload_align);
    let payload_offset = align_to(tag.size, payload_align)?;
    Some(TypeLayout {
        size: align_to(payload_offset.checked_add(payload_size)?, align)?,
        align,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_alignment_rejects_rounding_overflow() {
        assert_eq!(align_to(u64::MAX, 8), None);
        assert_eq!(align_to(u64::MAX, 1), Some(u64::MAX));
        assert_eq!(align_to(9, 8), Some(16));
        assert_eq!(align_to(9, 0), None);
    }

    #[test]
    fn aggregate_abi_helpers_reject_size_overflow() {
        assert_eq!(
            sequential_layout([
                &TypeLayout {
                    size: u64::MAX,
                    align: 1,
                },
                &TypeLayout { size: 1, align: 1 },
            ]),
            None
        );
        assert_eq!(
            union_layout_from_fields([&TypeLayout {
                size: u64::MAX,
                align: 8,
            }]),
            None
        );
        assert_eq!(
            union_layout_from_fields([&TypeLayout {
                size: u64::MAX,
                align: 1,
            }]),
            Some(TypeLayout {
                size: u64::MAX,
                align: 1,
            })
        );
        assert_eq!(
            tagged_union_layout_with_tag(
                &TypeLayout { size: 1, align: 1 },
                &[TypeLayout {
                    size: u64::MAX,
                    align: 1,
                }],
            ),
            None
        );
    }

    #[test]
    fn array_layout_rejects_zero_alignment() {
        assert_eq!(array_layout(&TypeLayout { size: 4, align: 0 }, 2,), None);
    }

    #[test]
    fn fat_pointer_layout_rejects_malformed_target_arithmetic() {
        assert_eq!(
            fat_pointer_layout(TargetDataLayout::LP64),
            Some(TypeLayout { size: 16, align: 8 })
        );
        assert_eq!(
            fat_pointer_layout(TargetDataLayout {
                pointer_size: u64::MAX,
                pointer_align: 8,
            }),
            None
        );
        assert_eq!(
            fat_pointer_layout(TargetDataLayout {
                pointer_size: 8,
                pointer_align: 0,
            }),
            None
        );
    }

    #[test]
    fn range_layout_tracks_the_number_of_stored_bounds() {
        let bound = TypeLayout { size: 8, align: 8 };
        for kind in [RangeTyKind::Exclusive, RangeTyKind::Inclusive] {
            assert_eq!(
                range_layout(kind, Some(&bound)),
                Some(TypeLayout { size: 16, align: 8 })
            );
        }
        for kind in [RangeTyKind::From, RangeTyKind::To, RangeTyKind::ToInclusive] {
            assert_eq!(
                range_layout(kind, Some(&bound)),
                Some(TypeLayout { size: 8, align: 8 })
            );
        }
        assert_eq!(
            range_layout(RangeTyKind::Full, None),
            Some(TypeLayout { size: 0, align: 1 })
        );
        assert_eq!(range_layout(RangeTyKind::From, None), None);
        assert_eq!(range_layout(RangeTyKind::Full, Some(&bound)), None);
    }
}
