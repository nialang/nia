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
) -> Option<(TypeLayout, Vec<EnumFieldLayout>)> {
    let mut placed = Vec::with_capacity(fields.len());
    let mut offset = 0u64;
    let mut max_align = 1u64;
    for field in fields {
        offset = align_to(offset, field.layout.align)?;
        placed.push(EnumFieldLayout {
            def_id: field.def_id,
            offset,
            layout: field.layout.clone(),
        });
        offset = offset.checked_add(field.layout.size)?;
        max_align = max_align.max(field.layout.align);
    }
    Some((
        TypeLayout {
            size: align_to(offset, max_align)?,
            align: max_align,
        },
        placed,
    ))
}

/// Places fields in the physical order selected by the layout computer.
///
/// Nia structs may be reordered before this step while extern structs preserve
/// source order. The returned list therefore owns the physical offsets used by
/// lowering and codegen.
pub(super) fn place_struct_fields(fields: Vec<PendingFieldLayout>) -> Option<StructLayout> {
    let mut placed = Vec::with_capacity(fields.len());
    let mut offset = 0u64;
    let mut max_align = 1u64;
    for field in fields {
        offset = align_to(offset, field.layout.align)?;
        placed.push(FieldLayout {
            def_id: field.def_id,
            offset,
            layout: field.layout.clone(),
        });
        offset = offset.checked_add(field.layout.size)?;
        max_align = max_align.max(field.layout.align);
    }
    let layout = TypeLayout {
        size: align_to(offset, max_align)?,
        align: max_align,
    };
    Some(StructLayout {
        layout,
        fields: placed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(def_id: u64, size: u64, align: u64) -> PendingFieldLayout {
        PendingFieldLayout {
            def_id: DefId(def_id),
            source_index: def_id as usize,
            layout: TypeLayout { size, align },
        }
    }

    #[test]
    fn struct_placement_rejects_field_end_and_tail_padding_overflow() {
        assert!(place_struct_fields(vec![field(1, u64::MAX, 1), field(2, 1, 1)]).is_none());
        assert!(place_struct_fields(vec![field(1, u64::MAX - 1, 8)]).is_none());
    }

    #[test]
    fn enum_payload_placement_rejects_field_end_overflow() {
        let fields = vec![
            PendingEnumFieldLayout {
                def_id: None,
                layout: TypeLayout {
                    size: u64::MAX,
                    align: 1,
                },
            },
            PendingEnumFieldLayout {
                def_id: None,
                layout: TypeLayout { size: 1, align: 1 },
            },
        ];
        assert!(place_enum_fields(fields).is_none());
    }
}
