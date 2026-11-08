// SPDX-License-Identifier: GPL-3.0-or-later
//! Enum expression and payload validation contracts.

use super::*;
use std::collections::HashMap;

impl BackendValidator<'_> {
    pub(super) fn validate_enum_variant_expr(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        variant: nia_ids::GlobalDefId,
        fields: &[FunctionExpr],
        span: Span,
    ) {
        for field in fields {
            self.validate_expr(field);
        }
        let Some(info) = self.index.enum_variant_info(variant) else {
            self.validate_enum_variant_ref(
                variant,
                span,
                "backend IR expression references missing enum variant",
            );
            return;
        };
        self.validate_enum_variant_value(
            info.owner.backing_type,
            info.variant.value,
            info.index,
            span,
        );
        let owner = info.owner.def_id;
        let backing_type = info.owner.backing_type;
        let payload = info.variant.payload.clone();
        let Some(layout) = self.index.enum_layout_for_type(owner, result_ty) else {
            self.invalid_enum(span, "variant owner has no enum layout");
            return;
        };
        // Fieldless enums use their backing integer directly; payload-bearing
        // enums use the nominal tagged aggregate. Derive that distinction from
        // the declared fields as well as the offset metadata so a scalar enum
        // remains valid when its layout has no payload fields.
        let has_payload = layout
            .variants
            .iter()
            .any(|variant| !variant.fields.is_empty());
        let result_matches = if has_payload {
            matches!(
                self.index.ty_kind(result_ty),
                Some(TyKind::Nominal { def_id, .. }) if *def_id == owner
            )
        } else {
            self.same_type(result_ty, backing_type)
                || matches!(
                    self.index.ty_kind(result_ty),
                    Some(TyKind::Nominal { def_id, .. }) if *def_id == owner
                )
        };
        if !result_matches {
            self.invalid_enum(
                span,
                "variant result type does not match its enum representation",
            );
        }

        let expected_fields = Self::enum_payload_types(&payload);
        if fields.len() != expected_fields.len() {
            self.invalid_enum(
                span,
                "variant payload field count does not match its declaration",
            );
        }
        for (field, expected_ty) in fields.iter().zip(expected_fields) {
            if !self.enum_field_type_matches(owner, result_ty, expected_ty, field.ty) {
                self.invalid_enum(
                    field.span,
                    "variant payload field type does not match its declaration",
                );
            }
        }
    }

    pub(super) fn validate_enum_constructor(
        &mut self,
        function_pointer_ty: nia_ids::InternedTyId,
        variant: nia_ids::GlobalDefId,
        span: Span,
    ) {
        let Some(info) = self.index.enum_variant_info(variant) else {
            self.validate_enum_variant_ref(
                variant,
                span,
                "backend IR expression references missing enum constructor variant",
            );
            return;
        };
        let expected_params = match &info.variant.payload {
            nia_backend_ir::BackendEnumVariantPayload::Unit => {
                self.invalid_enum(
                    span,
                    "unit variant cannot be used as a constructor function",
                );
                return;
            }
            nia_backend_ir::BackendEnumVariantPayload::Tuple(fields) => fields.clone(),
            nia_backend_ir::BackendEnumVariantPayload::Named(fields) => {
                fields.iter().map(|field| field.ty).collect()
            }
        };
        let owner_id = info.owner.def_id;
        let Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
            ..
        }) = self.ty_kind(function_pointer_ty).cloned()
        else {
            self.invalid_enum(span, "constructor value is not a function pointer");
            return;
        };
        if is_variadic {
            self.invalid_enum(span, "constructor function pointer is variadic");
        }
        if params.len() != expected_params.len()
            || !params
                .iter()
                .zip(expected_params)
                .all(|(actual, expected)| {
                    self.enum_field_type_matches(owner_id, return_type, expected, *actual)
                })
        {
            self.invalid_enum(
                span,
                "constructor function parameters do not match the variant payload",
            );
        }
        if !matches!(
            self.ty_kind(return_type),
            Some(TyKind::Nominal { def_id, .. }) if *def_id == owner_id
        ) {
            self.invalid_enum(
                span,
                "constructor function return type does not match the variant owner",
            );
        }
    }

    pub(super) fn validate_enum_variant_tag(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        variant: nia_ids::GlobalDefId,
        span: Span,
    ) {
        let Some(info) = self.index.enum_variant_info(variant) else {
            self.validate_enum_variant_ref(
                variant,
                span,
                "backend IR expression references missing enum variant tag",
            );
            return;
        };
        self.validate_enum_variant_value(
            info.owner.backing_type,
            info.variant.value,
            info.index,
            span,
        );
        if !self.same_type(result_ty, info.owner.backing_type) {
            self.invalid_enum(
                span,
                "variant tag result does not match the enum backing type",
            );
        }
    }

    fn validate_enum_variant_value(
        &mut self,
        backing_type: nia_ids::InternedTyId,
        explicit_value: Option<i128>,
        index: usize,
        span: Span,
    ) {
        let value = explicit_value.unwrap_or(index as i128);
        let Some(TyKind::Primitive(primitive)) = self.ty_kind(backing_type) else {
            return;
        };
        let pointer_width = self
            .target
            .pointer_size
            .checked_mul(8)
            .and_then(|bits| u32::try_from(bits).ok())
            .unwrap_or(0);
        if !nia_ty::IntConst::from_i128(value).fits_primitive_int(*primitive, pointer_width) {
            self.invalid_enum(span, "variant value is outside its enum backing type");
        }
    }

    pub(super) fn validate_enum_tag(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        value: &FunctionExpr,
        span: Span,
    ) {
        self.validate_expr(value);
        let (owner, expected_ty) = match self.index.ty_kind(value.ty) {
            Some(TyKind::Nominal { def_id, .. }) => {
                let Some(item) = self.index.enum_item(*def_id) else {
                    self.invalid_enum(span, "tag input nominal type is not an enum");
                    return;
                };
                (Some(*def_id), item.backing_type)
            }
            Some(TyKind::Primitive(primitive)) if primitive.is_integer() => (None, value.ty),
            _ => {
                self.invalid_enum(
                    span,
                    "tag input is not an enum value or integer representation",
                );
                return;
            }
        };
        if let Some(owner) = owner {
            if !self.enum_field_type_matches(owner, value.ty, expected_ty, result_ty) {
                self.invalid_enum(span, "tag result does not match the enum backing type");
            }
        } else {
            if !self.same_type(result_ty, expected_ty) {
                self.invalid_enum(span, "tag result does not match the enum backing type");
            }
        }
    }

    pub(super) fn validate_enum_payload_field(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        value: &FunctionExpr,
        variant: nia_ids::GlobalDefId,
        field: usize,
        span: Span,
    ) {
        self.validate_expr(value);
        let Some(info) = self.index.enum_variant_info(variant) else {
            self.validate_enum_variant_ref(
                variant,
                span,
                "backend IR expression references missing enum payload variant",
            );
            return;
        };
        let owner = info.owner.def_id;
        let payload = info.variant.payload.clone();
        if !matches!(
            self.index.ty_kind(value.ty),
            Some(TyKind::Nominal { def_id, .. }) if *def_id == owner
        ) {
            self.invalid_enum(
                span,
                "payload projection input does not match the variant owner",
            );
        }
        if !self
            .index
            .enum_layout_for_type(owner, value.ty)
            .is_some_and(|layout| {
                layout
                    .variants
                    .iter()
                    .any(|variant| !variant.fields.is_empty())
            })
        {
            self.invalid_enum(span, "payload projection enum has no payload storage");
        }
        let fields = Self::enum_payload_types(&payload);
        let Some(expected_ty) = fields.get(field).copied() else {
            self.invalid_enum(span, "payload projection field index is out of bounds");
            return;
        };
        if !self.enum_field_type_matches(owner, value.ty, expected_ty, result_ty) {
            self.invalid_enum(
                span,
                "payload projection result does not match its field type",
            );
        }
    }

    fn enum_payload_types(
        payload: &nia_backend_ir::BackendEnumVariantPayload,
    ) -> Vec<nia_ids::InternedTyId> {
        match payload {
            nia_backend_ir::BackendEnumVariantPayload::Unit => Vec::new(),
            nia_backend_ir::BackendEnumVariantPayload::Tuple(fields) => fields.clone(),
            nia_backend_ir::BackendEnumVariantPayload::Named(fields) => {
                fields.iter().map(|field| field.ty).collect()
            }
        }
    }

    fn enum_field_type_matches(
        &self,
        owner: nia_ids::GlobalDefId,
        instance_ty: nia_ids::InternedTyId,
        expected: nia_ids::InternedTyId,
        actual: nia_ids::InternedTyId,
    ) -> bool {
        let Some((substitutions, const_substitutions)) =
            self.index.enum_item(owner).and_then(|item| {
                self.index.ty_kind(instance_ty).and_then(|kind| {
                    let TyKind::Nominal {
                        def_id,
                        args,
                        const_args,
                    } = kind
                    else {
                        return None;
                    };
                    (*def_id == owner).then(|| {
                        let types = item
                            .generics
                            .iter()
                            .copied()
                            .zip(args.iter().copied())
                            .collect::<HashMap<_, _>>();
                        let consts = item
                            .const_generics
                            .iter()
                            .copied()
                            .zip(const_args.iter().cloned())
                            .collect::<HashMap<_, _>>();
                        (types, consts)
                    })
                })
            })
        else {
            return false;
        };
        let append = self.index.type_store().append_for_module(owner.module_id);
        let instantiated = nia_ty::substitute_ty(
            self.index.type_store(),
            &append,
            expected,
            &|name| substitutions.get(name).copied(),
            &|name| const_substitutions.get(name).cloned(),
            None,
        )
        .ok();
        instantiated.is_some_and(|instantiated| self.same_type(instantiated, actual))
    }

    pub(super) fn invalid_enum(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR enum expression has an invalid contract: {message}"),
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nia_backend_ir::{
        BackendConstFacts, BackendEnum, BackendEnumVariant, BackendEnumVariantPayload,
        BackendLayouts, BackendModule, BackendModuleStore,
    };
    use nia_ids::{DefId, ModuleIdAllocator};
    use nia_layout::TargetDataLayout;
    use nia_source::SourceIdentity;
    use nia_symbol::{SymbolId, stable_hash};
    use nia_ty::{
        ArrayLenTy, ConstGenericArg, ConstGenericValue, IntConst, PrimitiveTy, TyKind, TypeStore,
    };

    use crate::program_index::ProgramIndex;

    use super::BackendValidator;

    #[test]
    fn enum_payload_contract_substitutes_nested_types_and_const_lengths() {
        let module_id = ModuleIdAllocator::new()
            .expect("create module ID allocator")
            .allocate()
            .expect("allocate module ID");
        let type_store = Arc::new(TypeStore::new().expect("create type store"));
        let interner = type_store.append_for_module(module_id);
        let i32_ty = interner.primitive(PrimitiveTy::I32).expect("intern i32");
        let i64_ty = interner.primitive(PrimitiveTy::I64).expect("intern i64");
        let usize_ty = interner
            .primitive(PrimitiveTy::Usize)
            .expect("intern usize");
        let type_param = SymbolId::from_stable_hash(stable_hash("T"));
        let const_param = SymbolId::from_stable_hash(stable_hash("N"));
        let generic_ty = interner
            .intern(TyKind::GenericParam(type_param))
            .expect("intern generic parameter");
        let generic_pointer = interner
            .intern(TyKind::Pointer {
                is_readonly: true,
                elem: generic_ty,
            })
            .expect("intern generic pointer");
        let generic_array = interner
            .intern(TyKind::Array {
                len: ArrayLenTy::GenericParam(const_param),
                elem: generic_ty,
            })
            .expect("intern generic array");
        let expected_payload = interner
            .intern(TyKind::Tuple(vec![generic_pointer, generic_array]))
            .expect("intern generic nested payload");
        let enum_id = nia_ids::GlobalDefId {
            module_id,
            def_id: DefId(0),
        };
        let variant_id = nia_ids::GlobalDefId {
            module_id,
            def_id: DefId(1),
        };
        let enum_instance = interner
            .intern(TyKind::Nominal {
                def_id: enum_id,
                args: vec![i32_ty],
                const_args: vec![ConstGenericArg {
                    ty: usize_ty,
                    value: ConstGenericValue::Int(IntConst::unsigned(3)),
                }],
            })
            .expect("intern concrete enum instance");
        let pointer_i32 = interner
            .intern(TyKind::Pointer {
                is_readonly: true,
                elem: i32_ty,
            })
            .expect("intern concrete pointer");
        let pointer_i64 = interner
            .intern(TyKind::Pointer {
                is_readonly: true,
                elem: i64_ty,
            })
            .expect("intern mismatched pointer");
        let array_i32_3 = interner
            .intern(TyKind::Array {
                len: ArrayLenTy::ConstValue(3),
                elem: i32_ty,
            })
            .expect("intern concrete array");
        let array_i32_4 = interner
            .intern(TyKind::Array {
                len: ArrayLenTy::ConstValue(4),
                elem: i32_ty,
            })
            .expect("intern mismatched array length");
        let matching_payload = interner
            .intern(TyKind::Tuple(vec![pointer_i32, array_i32_3]))
            .expect("intern matching payload");
        let mismatched_element = interner
            .intern(TyKind::Tuple(vec![pointer_i64, array_i32_3]))
            .expect("intern mismatched nested element");
        let mismatched_length = interner
            .intern(TyKind::Tuple(vec![pointer_i32, array_i32_4]))
            .expect("intern mismatched payload length");
        drop(interner);

        let module = BackendModule {
            id: module_id,
            source_identity: SourceIdentity::new("generic-enum-contract.nia"),
            symbol_package_identity: "test/generic-enum@0".into(),
            name: "generic-enum-contract".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: TargetDataLayout::LP64,
                types: Vec::new(),
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                enum_instances: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: vec![BackendEnum {
                def_id: enum_id,
                name: SymbolId::from_stable_hash(stable_hash("Packet")),
                generics: vec![type_param],
                const_generics: vec![const_param],
                backing_type: i32_ty,
                variants: vec![BackendEnumVariant {
                    def_id: variant_id,
                    name: SymbolId::from_stable_hash(stable_hash("Nested")),
                    value: None,
                    payload: BackendEnumVariantPayload::Tuple(vec![expected_payload]),
                    span: nia_span::Span::default(),
                }],
                span: nia_span::Span::default(),
            }],
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: Vec::new(),
            function_instances: Vec::new(),
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        };
        let store =
            Arc::new(BackendModuleStore::new([module_id]).expect("create backend module store"));
        store.publish(module).expect("publish backend module");
        let (index, mut publisher) = ProgramIndex::new(store, type_store);
        publisher.publish(module_id).expect("publish module index");
        let validator = BackendValidator::new(&index, TargetDataLayout::LP64);

        assert!(validator.enum_field_type_matches(
            enum_id,
            enum_instance,
            expected_payload,
            matching_payload,
        ));
        assert!(!validator.enum_field_type_matches(
            enum_id,
            enum_instance,
            expected_payload,
            mismatched_element,
        ));
        assert!(!validator.enum_field_type_matches(
            enum_id,
            enum_instance,
            expected_payload,
            mismatched_length,
        ));
    }
}
