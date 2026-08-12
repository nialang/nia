//! Physical field placement after semantic field layouts are known.

use nia_defs::DefId;

use super::{EnumFieldLayout, FieldLayout, StructLayout, TypeLayout, abi::align_to};

#[derive(Debug, Clone)]
pub(super) struct PendingFieldLayout {
    pub(super) def_id: DefId,
    pub(super) source_index: usize,
    pub(super) layout: TypeLayout,
}

#[derive(Debug, Clone)]
pub(super) struct PendingEnumFieldLayout {
    pub(super) def_id: Option<DefId>,
    pub(super) layout: TypeLayout,
}

/// Places enum payload fields in declared order and tail-pads the payload to
/// its maximum field alignment.
pub(super) fn place_enum_fields(
    fields: Vec<PendingEnumFieldLayout>,
) -> (TypeLayout, Vec<EnumFieldLayout>) {
    let mut placed = Vec::with_capacity(fields.len());
    let mut offset = 0u64;
    let mut max_align = 1u64;
    for field in fields {
        offset = align_to(offset, field.layout.align);
        placed.push(EnumFieldLayout {
            def_id: field.def_id,
            offset,
            layout: field.layout.clone(),
        });
        offset = offset.saturating_add(field.layout.size);
        max_align = max_align.max(field.layout.align);
    }
    (
        TypeLayout {
            size: align_to(offset, max_align),
            align: max_align,
        },
        placed,
    )
}

/// Places fields in the physical order selected by the layout computer.
///
/// Nia structs may be reordered before this step while extern structs preserve
/// source order. The returned list therefore owns the physical offsets used by
/// lowering and codegen.
pub(super) fn place_struct_fields(fields: Vec<PendingFieldLayout>) -> StructLayout {
    let mut placed = Vec::with_capacity(fields.len());
    let mut offset = 0u64;
    let mut max_align = 1u64;
    for field in fields {
        offset = align_to(offset, field.layout.align);
        placed.push(FieldLayout {
            def_id: field.def_id,
            offset,
            layout: field.layout.clone(),
        });
        offset = offset.saturating_add(field.layout.size);
        max_align = max_align.max(field.layout.align);
    }
    let layout = TypeLayout {
        size: align_to(offset, max_align),
        align: max_align,
    };
    StructLayout {
        layout,
        fields: placed,
    }
}
