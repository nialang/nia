// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_backend_ir::{
    BackendEnum, BackendEnumVariant, BackendEnumVariantPayload, BackendField, BackendModule,
};
use nia_diagnostic::Diagnostic;
use nia_ids::{DefId, GlobalDefId, InternedTyId};
use nia_layout::{EnumLayout, EnumVariantLayout, FieldLayout, StructLayout, TypeLayout};
use nia_ty::TyKind;

use super::BackendValidator;

enum LayoutRecompute {
    Expected {
        layout: TypeLayout,
        source: LayoutSource,
    },
    Unavailable,
    Forbidden,
    Invalid,
}

#[derive(Clone, Copy)]
enum LayoutSource {
    Abi,
    Structural,
    Nominal,
}

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
            let Some(kind) = self.ty_kind(*ty).cloned() else {
                self.invalid_type_layout(*ty, "type belongs to a different compilation session");
                continue;
            };
            let expected = self.recompute_published_type_layout(&kind);
            let LayoutRecompute::Expected {
                layout: expected,
                source,
            } = expected
            else {
                match expected {
                    LayoutRecompute::Forbidden => {
                        self.invalid_type_layout(*ty, "type cannot publish an ABI layout");
                    }
                    LayoutRecompute::Invalid => {
                        self.invalid_type_layout(
                            *ty,
                            "type and target cannot produce a valid layout",
                        );
                    }
                    LayoutRecompute::Unavailable | LayoutRecompute::Expected { .. } => {}
                }
                continue;
            };
            if *layout != expected {
                let message = match source {
                    LayoutSource::Abi => "layout does not match its type and target",
                    LayoutSource::Structural => {
                        "structural layout does not match its component types and target"
                    }
                    LayoutSource::Nominal => {
                        "nominal layout does not match its aggregate layout product"
                    }
                };
                self.invalid_type_layout(*ty, message);
            }
        }
    }

    fn recompute_published_type_layout(&self, kind: &TyKind) -> LayoutRecompute {
        let expected = match kind {
            TyKind::Primitive(primitive) => {
                Some(nia_layout::primitive_layout(*primitive, self.target))
            }
            TyKind::Vector { elem, lanes } => nia_layout::vector_layout(*elem, *lanes, self.target),
            TyKind::Pointer { .. }
            | TyKind::VolatilePointer { .. }
            | TyKind::FunctionPointer { .. } => Some(TypeLayout {
                size: self.target.pointer_size,
                align: self.target.pointer_align,
            }),
            TyKind::Slice { .. } | TyKind::TraitObject { .. } | TyKind::Callable { .. } => {
                nia_layout::fat_pointer_layout(self.target)
            }
            TyKind::Tuple(fields) => {
                let Some(layouts) = self.component_layouts(fields) else {
                    return LayoutRecompute::Unavailable;
                };
                return nia_layout::sequential_layout(&layouts)
                    .map(|layout| LayoutRecompute::Expected {
                        layout,
                        source: LayoutSource::Structural,
                    })
                    .unwrap_or(LayoutRecompute::Invalid);
            }
            TyKind::ClosureState { captures, .. } => {
                let Some(layouts) = self.component_layouts(captures) else {
                    return LayoutRecompute::Unavailable;
                };
                return nia_layout::sequential_layout(&layouts)
                    .map(|layout| LayoutRecompute::Expected {
                        layout,
                        source: LayoutSource::Structural,
                    })
                    .unwrap_or(LayoutRecompute::Invalid);
            }
            TyKind::Array { len, elem } => {
                let Some(len) = self.array_len_value(len) else {
                    return LayoutRecompute::Unavailable;
                };
                let Some(elem) = self.layout_of(*elem) else {
                    return LayoutRecompute::Unavailable;
                };
                return nia_layout::array_layout(&elem, len)
                    .map(|layout| LayoutRecompute::Expected {
                        layout,
                        source: LayoutSource::Structural,
                    })
                    .unwrap_or(LayoutRecompute::Invalid);
            }
            TyKind::Range { kind, bound } => {
                let bound = match bound {
                    Some(bound) => {
                        let Some(layout) = self.layout_of(*bound) else {
                            return LayoutRecompute::Unavailable;
                        };
                        Some(layout)
                    }
                    None => None,
                };
                return nia_layout::range_layout(*kind, bound.as_ref())
                    .map(|layout| LayoutRecompute::Expected {
                        layout,
                        source: LayoutSource::Structural,
                    })
                    .unwrap_or(LayoutRecompute::Invalid);
            }
            TyKind::Optional { elem } => {
                let Some(elem) = self.layout_of(*elem) else {
                    return LayoutRecompute::Unavailable;
                };
                return nia_layout::tagged_union_layout(&[elem])
                    .map(|layout| LayoutRecompute::Expected {
                        layout,
                        source: LayoutSource::Structural,
                    })
                    .unwrap_or(LayoutRecompute::Invalid);
            }
            TyKind::ErrorUnion { error, value } => {
                let Some(error) = self.layout_of(*error) else {
                    return LayoutRecompute::Unavailable;
                };
                let Some(value) = self.layout_of(*value) else {
                    return LayoutRecompute::Unavailable;
                };
                return nia_layout::tagged_union_layout(&[error, value])
                    .map(|layout| LayoutRecompute::Expected {
                        layout,
                        source: LayoutSource::Structural,
                    })
                    .unwrap_or(LayoutRecompute::Invalid);
            }
            TyKind::Nominal {
                def_id,
                args,
                const_args,
            } => {
                let layout = self.nominal_layout_product(*def_id, args, const_args);
                let Some(layout) = layout else {
                    return LayoutRecompute::Unavailable;
                };
                return LayoutRecompute::Expected {
                    layout,
                    source: LayoutSource::Nominal,
                };
            }
            TyKind::Opaque
            | TyKind::SlicePointee { .. }
            | TyKind::TraitObjectPointee { .. }
            | TyKind::CallablePointee { .. }
            | TyKind::BuiltinType(_)
            | TyKind::BuiltinTrait { .. }
            | TyKind::GenericParam(_)
            | TyKind::SelfParam
            | TyKind::ConstOnly
            | TyKind::Error => return LayoutRecompute::Forbidden,
            _ => return LayoutRecompute::Unavailable,
        };
        expected
            .map(|layout| LayoutRecompute::Expected {
                layout,
                source: LayoutSource::Abi,
            })
            .unwrap_or(LayoutRecompute::Invalid)
    }

    fn component_layouts(&self, fields: &[InternedTyId]) -> Option<Vec<TypeLayout>> {
        fields.iter().map(|field| self.layout_of(*field)).collect()
    }

    fn nominal_layout_product(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) -> Option<TypeLayout> {
        if args.is_empty() && const_args.is_empty() {
            return self
                .index
                .struct_layout(def_id)
                .map(|layout| layout.layout.clone())
                .or_else(|| {
                    self.index
                        .union_layout(def_id)
                        .map(|layout| layout.layout.clone())
                })
                .or_else(|| {
                    self.index
                        .enum_layout(def_id)
                        .map(|layout| layout.layout.clone())
                });
        }

        self.index
            .struct_instance_layout(def_id, args, const_args)
            .map(|layout| layout.layout.clone())
            .or_else(|| {
                self.index
                    .union_instance_layout(def_id, args, const_args)
                    .map(|layout| layout.layout.clone())
            })
            .or_else(|| {
                self.index.struct_instance_layouts(def_id).find_map(|item| {
                    (self.same_type_args(&item.key.args, args)
                        && self.same_const_args(&item.key.const_args, const_args))
                    .then_some(item.layout.layout.clone())
                })
            })
            .or_else(|| {
                self.index.union_instance_layouts(def_id).find_map(|item| {
                    (self.same_type_args(&item.key.args, args)
                        && self.same_const_args(&item.key.const_args, const_args))
                    .then_some(item.layout.layout.clone())
                })
            })
    }

    pub(super) fn validate_aggregate_layout_products(&mut self, module: &BackendModule) {
        let mut seen = HashSet::new();
        for (def_id, layout) in &module.layouts.structs {
            if !seen.insert(*def_id) {
                self.invalid_aggregate_layout(*def_id, "struct layout identity is duplicated");
                continue;
            }
            let Some(item) = module.structs.iter().find(|item| item.def_id == *def_id) else {
                continue;
            };
            self.validate_aggregate_layout(*def_id, &item.fields, item.is_extern, false, layout);
        }

        seen.clear();
        for (def_id, layout) in &module.layouts.unions {
            if !seen.insert(*def_id) {
                self.invalid_aggregate_layout(*def_id, "union layout identity is duplicated");
                continue;
            }
            let Some(item) = module.unions.iter().find(|item| item.def_id == *def_id) else {
                continue;
            };
            self.validate_aggregate_layout(*def_id, &item.fields, item.is_extern, true, layout);
        }

        let mut seen_instances = HashSet::new();
        for (key, layout) in &module.layouts.struct_instances {
            self.validate_aggregate_instance_key(key);
            if !seen_instances.insert(key) {
                self.invalid_aggregate_layout(
                    key.def_id,
                    "struct instance layout identity is duplicated",
                );
                continue;
            }
            let Some(item) = module.struct_instances.iter().find(|item| {
                item.def_id == key.def_id
                    && item.args == key.args
                    && item.const_args == key.const_args
            }) else {
                continue;
            };
            self.validate_aggregate_layout(key.def_id, &item.fields, item.is_extern, false, layout);
        }

        seen_instances.clear();
        for (key, layout) in &module.layouts.union_instances {
            self.validate_aggregate_instance_key(key);
            if !seen_instances.insert(key) {
                self.invalid_aggregate_layout(
                    key.def_id,
                    "union instance layout identity is duplicated",
                );
                continue;
            }
            let Some(item) = module.union_instances.iter().find(|item| {
                item.def_id == key.def_id
                    && item.args == key.args
                    && item.const_args == key.const_args
            }) else {
                continue;
            };
            self.validate_aggregate_layout(key.def_id, &item.fields, item.is_extern, true, layout);
        }
    }

    fn validate_aggregate_instance_key(&mut self, key: &nia_backend_ir::BackendStructInstanceKey) {
        for ty in &key.args {
            self.validate_type(*ty, nia_span::Span::default());
        }
        for arg in &key.const_args {
            self.validate_type(arg.ty, nia_span::Span::default());
        }
    }

    fn validate_aggregate_layout(
        &mut self,
        def_id: GlobalDefId,
        declared_fields: &[BackendField],
        is_extern: bool,
        is_union: bool,
        physical: &StructLayout,
    ) {
        if !valid_type_layout(&physical.layout) {
            self.invalid_aggregate_layout(def_id, "total layout is invalid");
        }
        for field in &physical.fields {
            if !valid_type_layout(&field.layout) {
                self.invalid_aggregate_layout(def_id, "field layout is invalid");
            }
            if field
                .offset
                .checked_add(field.layout.size)
                .is_none_or(|end| end > physical.layout.size)
            {
                self.invalid_aggregate_layout(def_id, "field extends beyond aggregate storage");
            }
        }

        let Some(expected) =
            self.expected_aggregate_layout(def_id, declared_fields, is_extern, is_union)
        else {
            return;
        };
        if physical.fields.len() != expected.fields.len() {
            self.invalid_aggregate_layout(
                def_id,
                "field layout count does not match the declaration",
            );
        }
        for (field, expected_field) in physical.fields.iter().zip(&expected.fields) {
            if field.def_id != expected_field.def_id {
                self.invalid_aggregate_layout(
                    def_id,
                    "field identity does not match physical declaration order",
                );
            }
            if field.layout != expected_field.layout {
                self.invalid_aggregate_layout(
                    def_id,
                    "field layout does not match its declared type",
                );
            }
            if field.offset != expected_field.offset {
                self.invalid_aggregate_layout(
                    def_id,
                    "field offset does not match aggregate placement",
                );
            }
        }
        if physical.layout != expected.layout {
            self.invalid_aggregate_layout(
                def_id,
                "total layout does not match its declared fields",
            );
        }
    }

    fn expected_aggregate_layout(
        &mut self,
        def_id: GlobalDefId,
        declared_fields: &[BackendField],
        is_extern: bool,
        is_union: bool,
    ) -> Option<StructLayout> {
        let mut fields = Vec::with_capacity(declared_fields.len());
        for (source_index, field) in declared_fields.iter().enumerate() {
            if field.def_id.module_id != def_id.module_id {
                self.invalid_aggregate_layout(
                    def_id,
                    "field does not belong to its aggregate module",
                );
            }
            let Some(layout) = self.layout_of(field.ty) else {
                self.invalid_aggregate_layout(def_id, "field type has no runtime layout");
                return None;
            };
            if !valid_type_layout(&layout) {
                self.invalid_aggregate_layout(def_id, "field type layout is invalid");
                return None;
            }
            fields.push((source_index, field.def_id.def_id, layout));
        }

        if is_union {
            let max_size = fields
                .iter()
                .map(|(_, _, layout)| layout.size)
                .max()
                .unwrap_or(0);
            let max_align = fields
                .iter()
                .map(|(_, _, layout)| layout.align)
                .max()
                .unwrap_or(1);
            let Some(size) = align_to(max_size, max_align) else {
                self.invalid_aggregate_layout(def_id, "union layout overflowed");
                return None;
            };
            return Some(StructLayout {
                layout: TypeLayout {
                    size,
                    align: max_align,
                },
                fields: fields
                    .into_iter()
                    .map(|(_, def_id, layout)| FieldLayout {
                        def_id,
                        offset: 0,
                        layout,
                    })
                    .collect(),
            });
        }

        if !is_extern {
            fields.sort_by(|left, right| {
                right
                    .2
                    .align
                    .cmp(&left.2.align)
                    .then_with(|| right.2.size.cmp(&left.2.size))
                    .then_with(|| left.0.cmp(&right.0))
            });
        }
        let mut placed = Vec::with_capacity(fields.len());
        let mut offset = 0u64;
        let mut max_align = 1u64;
        for (_, field_id, layout) in fields {
            let Some(field_offset) = align_to(offset, layout.align) else {
                self.invalid_aggregate_layout(def_id, "struct field placement overflowed");
                return None;
            };
            let Some(next_offset) = field_offset.checked_add(layout.size) else {
                self.invalid_aggregate_layout(def_id, "struct field placement overflowed");
                return None;
            };
            max_align = max_align.max(layout.align);
            placed.push(FieldLayout {
                def_id: field_id,
                offset: field_offset,
                layout,
            });
            offset = next_offset;
        }
        let Some(size) = align_to(offset, max_align) else {
            self.invalid_aggregate_layout(def_id, "struct layout overflowed");
            return None;
        };
        Some(StructLayout {
            layout: TypeLayout {
                size,
                align: max_align,
            },
            fields: placed,
        })
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

    fn invalid_aggregate_layout(&mut self, def_id: GlobalDefId, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            nia_span::Span::default(),
            format!("backend IR aggregate layout {def_id:?} has an invalid contract: {message}"),
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
