// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

use nia_backend_ir::{BackendEnum, BackendEnumVariantPayload, BackendModule};
use nia_diagnostic::Diagnostic;
use nia_ids::GlobalDefId;
use nia_layout::{EnumLayout, TypeLayout};
use nia_ty::TyKind;

use super::BackendValidator;

impl BackendValidator<'_> {
    pub(super) fn validate_enum_layout_products(&mut self, module: &BackendModule) {
        for item in &module.enums {
            self.validate_enum_discriminants(item);
        }

        let mut seen = HashSet::new();
        for (def_id, layout) in &module.layouts.enums {
            if !seen.insert(*def_id) {
                self.invalid_enum_layout(*def_id, "layout identity is duplicated");
                continue;
            }
            let Some(item) = module.enums.iter().find(|item| item.def_id == *def_id) else {
                self.invalid_enum_layout(*def_id, "layout has no matching enum definition");
                continue;
            };
            self.validate_enum_layout_header(item, layout);
        }
    }

    fn validate_enum_layout_header(&mut self, item: &BackendEnum, layout: &EnumLayout) {
        let Some(TyKind::Primitive(primitive)) = self.ty_kind(item.backing_type).cloned() else {
            return;
        };
        if !primitive.is_integer() {
            return;
        }
        let expected_tag = nia_layout::primitive_layout(primitive, self.target);
        if layout.tag != expected_tag {
            self.invalid_enum_layout(item.def_id, "tag layout does not match the backing type");
        }
        if layout.variants.len() != item.variants.len() {
            self.invalid_enum_layout(
                item.def_id,
                "variant layout count does not match the enum declaration",
            );
            return;
        }
        for (declared, physical) in item.variants.iter().zip(&layout.variants) {
            if declared.def_id.module_id != item.def_id.module_id
                || declared.def_id.def_id != physical.def_id
            {
                self.invalid_enum_layout(
                    item.def_id,
                    "variant layout identity does not match declaration order",
                );
            }
            if !valid_type_layout(&physical.payload) {
                self.invalid_enum_layout(item.def_id, "variant payload layout is invalid");
            }
        }

        let has_payload = item.variants.iter().any(|variant| match &variant.payload {
            BackendEnumVariantPayload::Unit => false,
            BackendEnumVariantPayload::Tuple(fields) => !fields.is_empty(),
            BackendEnumVariantPayload::Named(fields) => !fields.is_empty(),
        });
        let expected = expected_enum_storage(&expected_tag, layout, has_payload);
        let Some((expected_layout, expected_offset)) = expected else {
            self.invalid_enum_layout(item.def_id, "layout arithmetic overflowed");
            return;
        };
        if layout.payload_offset != expected_offset {
            self.invalid_enum_layout(
                item.def_id,
                "payload offset does not match tag and payload alignment",
            );
        }
        if layout.layout != expected_layout {
            self.invalid_enum_layout(
                item.def_id,
                "total layout does not match tag and variant payloads",
            );
        }
    }

    fn invalid_enum_layout(&mut self, def_id: GlobalDefId, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            nia_span::Span::default(),
            format!("backend IR enum layout {def_id:?} has an invalid contract: {message}"),
        ));
    }
}

fn expected_enum_storage(
    tag: &TypeLayout,
    layout: &EnumLayout,
    has_payload: bool,
) -> Option<(TypeLayout, Option<u64>)> {
    if !valid_type_layout(tag) {
        return None;
    }
    if !has_payload {
        return Some((tag.clone(), None));
    }
    let payload_size = layout
        .variants
        .iter()
        .map(|variant| variant.payload.size)
        .max()
        .unwrap_or(0);
    let payload_align = layout
        .variants
        .iter()
        .map(|variant| variant.payload.align)
        .max()
        .unwrap_or(1);
    if payload_align == 0 {
        return None;
    }
    let payload_offset = align_to(tag.size, payload_align)?;
    let align = tag.align.max(payload_align);
    let size = align_to(payload_offset.checked_add(payload_size)?, align)?;
    Some((TypeLayout { size, align }, Some(payload_offset)))
}

fn valid_type_layout(layout: &TypeLayout) -> bool {
    layout.align.is_power_of_two() && layout.size.is_multiple_of(layout.align)
}

fn align_to(value: u64, align: u64) -> Option<u64> {
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

#[cfg(test)]
mod tests {
    use super::{align_to, valid_type_layout};
    use nia_layout::TypeLayout;

    #[test]
    fn enum_layout_arithmetic_rejects_invalid_alignment_and_overflow() {
        assert!(!valid_type_layout(&TypeLayout { size: 8, align: 0 }));
        assert!(!valid_type_layout(&TypeLayout { size: 8, align: 3 }));
        assert!(!valid_type_layout(&TypeLayout { size: 7, align: 4 }));
        assert_eq!(align_to(9, 8), Some(16));
        assert_eq!(align_to(u64::MAX, 8), None);
    }
}
