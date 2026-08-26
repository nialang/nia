// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_backend_ir::{BackendEnum, BackendEnumVariant, BackendEnumVariantPayload, BackendModule};
use nia_diagnostic::Diagnostic;
use nia_ids::{DefId, GlobalDefId, InternedTyId};
use nia_layout::{EnumLayout, EnumVariantLayout, TypeLayout};
use nia_ty::TyKind;

use super::BackendValidator;

impl BackendValidator<'_> {
    pub(super) fn validate_type_layout_products(&mut self, module: &BackendModule) {
        let mut seen = HashMap::new();
        for (ty, layout) in &module.layouts.types {
            if seen
                .insert(*ty, layout)
                .is_some_and(|existing| existing != layout)
            {
                self.invalid_type_layout(*ty, "duplicate layout values conflict");
            }
            if !valid_type_layout(layout) {
                self.invalid_type_layout(*ty, "size and alignment are not a valid ABI layout");
            }
            let Some(kind) = self.ty_kind(*ty) else {
                self.invalid_type_layout(*ty, "type belongs to a different compilation session");
                continue;
            };
            let expected = match kind {
                TyKind::Primitive(primitive) => {
                    Some(nia_layout::primitive_layout(*primitive, self.target))
                }
                TyKind::Vector { elem, lanes } => {
                    nia_layout::vector_layout(*elem, *lanes, self.target)
                }
                TyKind::Pointer { .. }
                | TyKind::VolatilePointer { .. }
                | TyKind::FunctionPointer { .. } => Some(TypeLayout {
                    size: self.target.pointer_size,
                    align: self.target.pointer_align,
                }),
                TyKind::Slice { .. } | TyKind::TraitObject { .. } | TyKind::Callable { .. } => {
                    nia_layout::fat_pointer_layout(self.target)
                }
                _ => continue,
            };
            let Some(expected) = expected else {
                self.invalid_type_layout(*ty, "target layout arithmetic overflowed");
                continue;
            };
            if *layout != expected {
                self.invalid_type_layout(*ty, "layout does not match its type and target");
            }
        }
    }

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
        let mut expected_payloads = Vec::with_capacity(item.variants.len());
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
            let Some(expected_payload) =
                self.validate_enum_variant_payload(item.def_id, declared, physical)
            else {
                return;
            };
            expected_payloads.push(expected_payload);
        }

        let has_payload = item.variants.iter().any(|variant| match &variant.payload {
            BackendEnumVariantPayload::Unit => false,
            BackendEnumVariantPayload::Tuple(fields) => !fields.is_empty(),
            BackendEnumVariantPayload::Named(fields) => !fields.is_empty(),
        });
        let expected = expected_enum_storage(&expected_tag, &expected_payloads, has_payload);
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

    fn validate_enum_variant_payload(
        &mut self,
        enum_id: GlobalDefId,
        declared: &BackendEnumVariant,
        physical: &EnumVariantLayout,
    ) -> Option<TypeLayout> {
        let declared_fields: Vec<(Option<DefId>, InternedTyId)> = match &declared.payload {
            BackendEnumVariantPayload::Unit => Vec::new(),
            BackendEnumVariantPayload::Tuple(fields) => {
                fields.iter().map(|ty| (None, *ty)).collect()
            }
            BackendEnumVariantPayload::Named(fields) => fields
                .iter()
                .map(|field| {
                    if field.def_id.module_id != enum_id.module_id {
                        self.invalid_enum_layout(
                            enum_id,
                            "payload field definition does not belong to the enum module",
                        );
                    }
                    (Some(field.def_id.def_id), field.ty)
                })
                .collect(),
        };
        if physical.fields.len() != declared_fields.len() {
            self.invalid_enum_layout(
                enum_id,
                "payload field count does not match the variant declaration",
            );
        }

        for field in &physical.fields {
            if !valid_type_layout(&field.layout) {
                self.invalid_enum_layout(enum_id, "payload field layout is invalid");
            }
            if field
                .offset
                .checked_add(field.layout.size)
                .is_none_or(|end| end > physical.payload.size)
            {
                self.invalid_enum_layout(enum_id, "payload field extends beyond variant storage");
            }
        }

        let mut offset = 0u64;
        let mut max_align = 1u64;
        for (index, (expected_id, ty)) in declared_fields.iter().enumerate() {
            let Some(expected_layout) = self.layout_of(*ty) else {
                self.invalid_enum_layout(enum_id, "payload field type has no runtime layout");
                return None;
            };
            if !valid_type_layout(&expected_layout) {
                self.invalid_enum_layout(enum_id, "payload field type layout is invalid");
                return None;
            }
            let Some(expected_offset) = align_to(offset, expected_layout.align) else {
                self.invalid_enum_layout(enum_id, "payload field placement overflowed");
                return None;
            };
            if let Some(field) = physical.fields.get(index) {
                if field.def_id != *expected_id {
                    self.invalid_enum_layout(
                        enum_id,
                        "payload field identity does not match declaration order",
                    );
                }
                if field.offset != expected_offset {
                    self.invalid_enum_layout(
                        enum_id,
                        "payload field offset does not match declaration layout",
                    );
                }
                if field.layout != expected_layout {
                    self.invalid_enum_layout(
                        enum_id,
                        "payload field layout does not match its declared type",
                    );
                }
            }
            let Some(next_offset) = expected_offset.checked_add(expected_layout.size) else {
                self.invalid_enum_layout(enum_id, "payload field placement overflowed");
                return None;
            };
            offset = next_offset;
            max_align = max_align.max(expected_layout.align);
        }

        let Some(size) = align_to(offset, max_align) else {
            self.invalid_enum_layout(enum_id, "variant payload layout overflowed");
            return None;
        };
        let expected_payload = TypeLayout {
            size,
            align: max_align,
        };
        if physical.payload != expected_payload {
            self.invalid_enum_layout(
                enum_id,
                "variant payload layout does not match its declared fields",
            );
        }
        Some(expected_payload)
    }

    fn invalid_enum_layout(&mut self, def_id: GlobalDefId, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            nia_span::Span::default(),
            format!("backend IR enum layout {def_id:?} has an invalid contract: {message}"),
        ));
    }

    fn invalid_type_layout(&mut self, ty: InternedTyId, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            nia_span::Span::default(),
            format!("backend IR type layout {ty:?} has an invalid contract: {message}"),
        ));
    }
}

fn expected_enum_storage(
    tag: &TypeLayout,
    payloads: &[TypeLayout],
    has_payload: bool,
) -> Option<(TypeLayout, Option<u64>)> {
    if !valid_type_layout(tag) {
        return None;
    }
    if !has_payload {
        return Some((tag.clone(), None));
    }
    let payload_size = payloads
        .iter()
        .map(|payload| payload.size)
        .max()
        .unwrap_or(0);
    let payload_align = payloads
        .iter()
        .map(|payload| payload.align)
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
