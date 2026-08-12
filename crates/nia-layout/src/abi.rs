//! Stateless ABI layout arithmetic shared by semantic layout consumers.

use nia_ty::PrimitiveTy;

use super::{TargetDataLayout, TypeLayout};

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

pub fn array_layout(element: &TypeLayout, len: u64) -> Option<TypeLayout> {
    Some(TypeLayout {
        size: element.size.checked_mul(len)?,
        align: element.align,
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
        size: align_to(store_size, align),
        align,
    })
}

pub fn union_layout_from_fields<'a>(
    fields: impl IntoIterator<Item = &'a TypeLayout>,
) -> TypeLayout {
    let mut max_size = 0u64;
    let mut max_align = 1u64;
    for field in fields {
        max_size = max_size.max(field.size);
        max_align = max_align.max(field.align);
    }
    TypeLayout {
        size: align_to(max_size, max_align),
        align: max_align,
    }
}

pub(super) fn align_to(value: u64, align: u64) -> u64 {
    if align <= 1 {
        value
    } else {
        value.div_ceil(align) * align
    }
}

pub(super) fn tagged_union_layout(payloads: &[TypeLayout]) -> TypeLayout {
    let tag = TypeLayout { size: 1, align: 1 };
    tagged_union_layout_with_tag(&tag, payloads)
}

pub(super) fn tagged_union_layout_with_tag(
    tag: &TypeLayout,
    payloads: &[TypeLayout],
) -> TypeLayout {
    let payload_size = payloads.iter().map(|layout| layout.size).max().unwrap_or(0);
    let payload_align = payloads
        .iter()
        .map(|layout| layout.align)
        .max()
        .unwrap_or(1);
    let align = tag.align.max(payload_align);
    let payload_offset = align_to(tag.size, payload_align);
    TypeLayout {
        size: align_to(payload_offset.saturating_add(payload_size), align),
        align,
    }
}
