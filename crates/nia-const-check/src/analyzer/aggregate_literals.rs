// SPDX-License-Identifier: GPL-3.0-or-later
use super::ty_substitution::substitute_ty_generics;
use super::*;

impl Analyzer<'_> {
    pub(super) fn resolved_const_aggregate_literal_type(
        &mut self,
        span: Span,
        fields: &[ResolvedConstFieldInit],
        expected: InternedTyId,
    ) -> Option<ConstValueType> {
        // Literal type arguments are resolved in the generic function's type
        // context. During const execution, instantiate that nominal type before
        // deriving field types; otherwise `Union[T] { value }` compares the
        // concrete argument against the stale generic `T`.
        let expected = self.substitute_ty_generics(expected);
        let Some((def_id, expected_args, expected_const_args)) =
            self.expected_nominal_parts(expected)
        else {
            for field in fields {
                let _ = self.resolved_const_expr_type(field.value(), None);
            }
            if self.const_runtime_type_is_known(expected)
                && !matches!(self.ty_kind(expected), Some(TyKind::ConstOnly))
            {
                self.push_const_type_error(
                    span,
                    "const struct literal expected type is not a struct",
                );
            }
            return None;
        };
        if self.def_kind_of(def_id) == Some(DefKind::Union) {
            return self.resolved_const_union_literal_type(
                span,
                fields,
                expected,
                def_id,
                &expected_args,
                &expected_const_args,
            );
        }
        if self.def_kind_of(def_id) != Some(DefKind::Struct) {
            for field in fields {
                let _ = self.resolved_const_expr_type(field.value(), None);
            }
            return None;
        }
        let signature = self.struct_signature_for(def_id)?;
        let field_tys =
            self.const_struct_field_types(&signature, &expected_args, &expected_const_args)?;
        let field_set = check_required_field_set(
            fields
                .iter()
                .map(|field| NamedField::new(field.span(), *field.name_symbol())),
            signature.fields.iter().map(|field| field.name),
        );
        let fields_are_valid = field_set.is_valid();
        for field in &field_set.duplicate_fields {
            let name = self.symbol_name(field.name);
            self.push_const_type_error(
                field.span,
                &format!("duplicate const struct field `{name}`"),
            );
        }
        for field in &field_set.unknown_fields {
            let name = self.symbol_name(field.name);
            self.push_const_type_error(field.span, &format!("unknown const struct field `{name}`"));
        }
        for name in &field_set.missing_fields {
            let name = self.symbol_name(*name);
            self.push_const_type_error(span, &format!("missing const struct field `{name}`"));
        }

        // Infer substitutions from every field before validating any field
        // against the final instantiated type. A later field may resolve a
        // generic that appears in an earlier field's expected type.
        let mut substitutions = SymbolMap::default();
        let mut actual_fields = Vec::with_capacity(fields.len());
        for field in fields {
            let Some(expected_field) = field_tys.get(field.name_symbol()).copied() else {
                let _ = self.resolved_const_expr_type(field.value(), None);
                actual_fields.push((None, false));
                continue;
            };
            let diagnostics_before_field = self.diagnostics.len();
            let actual_field =
                self.resolved_const_contextual_value_type(field.value(), expected_field);
            if let Some(actual_field) = actual_field {
                let _ = self.probe_type_generic_inference(
                    span,
                    expected_field,
                    actual_field,
                    &mut substitutions,
                );
            }
            actual_fields.push((
                actual_field,
                self.diagnostics.len() != diagnostics_before_field,
            ));
        }

        let mut types_match = true;
        for (field, (actual_field, field_has_diagnostic)) in fields.iter().zip(actual_fields) {
            let Some(raw_expected_field) = field_tys.get(field.name_symbol()).copied() else {
                continue;
            };
            let expected_field =
                self.substitute_current_ty_generics(raw_expected_field, &substitutions)?;
            let actual_field = actual_field
                .filter(|actual| !self.type_contains_generic(*actual))
                .or_else(|| {
                    if field_has_diagnostic {
                        None
                    } else {
                        self.resolved_const_arg_runtime_type(field.value(), Some(expected_field))
                    }
                });
            let Some(actual_field) = actual_field else {
                continue;
            };
            if !self.const_function_types_match(expected_field, actual_field) {
                if self.const_runtime_type_is_known(expected_field)
                    && self.const_runtime_type_is_known(actual_field)
                {
                    self.push_const_type_error(
                        field.value().span(),
                        "const struct literal field does not match its expected type",
                    );
                }
                types_match = false;
            }
        }
        if !fields_are_valid || !types_match {
            return None;
        }
        self.substitute_nominal_args(def_id, expected_args, expected_const_args, &substitutions)
            .map(ConstValueType::Runtime)
    }

    fn resolved_const_union_literal_type(
        &mut self,
        span: Span,
        fields: &[ResolvedConstFieldInit],
        expected: InternedTyId,
        def_id: GlobalDefId,
        expected_args: &[InternedTyId],
        expected_const_args: &[ConstGenericArg],
    ) -> Option<ConstValueType> {
        let signature = self.union_signature_for(def_id)?;
        let field_tys =
            self.const_union_field_types(&signature, expected_args, expected_const_args)?;
        if fields.len() != 1 {
            for field in fields {
                let _ = self.resolved_const_expr_type(field.value(), None);
            }
            self.push_const_type_error(
                span,
                &format!(
                    "const union literal requires exactly one field, got {}",
                    fields.len()
                ),
            );
            return None;
        }
        let field = &fields[0];
        let Some(expected_field) = field_tys.get(field.name_symbol()).copied() else {
            let _ = self.resolved_const_expr_type(field.value(), None);
            let name = self.symbol_name(*field.name_symbol());
            self.push_const_type_error(
                field.span(),
                &format!("unknown const union field `{name}`"),
            );
            return None;
        };
        let actual = self.resolved_const_contextual_value_type(field.value(), expected_field);
        if let Some(actual) = actual
            && !self.const_function_types_match(expected_field, actual)
        {
            self.push_const_type_error(
                field.value().span(),
                "const union literal field does not match its expected type",
            );
            return None;
        }
        Some(ConstValueType::Runtime(expected))
    }

    pub(super) fn const_nominal_aggregate_field_type(
        &mut self,
        ty: InternedTyId,
        name: &SymbolId,
    ) -> Option<InternedTyId> {
        let (def_id, args, const_args) = self.expected_nominal_parts(ty)?;
        match self.def_kind_of(def_id)? {
            DefKind::Struct => self
                .struct_signature_for(def_id)
                .and_then(|signature| {
                    self.const_struct_field_types(&signature, &args, &const_args)
                })?
                .get(name)
                .copied(),
            DefKind::Union => self
                .union_signature_for(def_id)
                .and_then(|signature| self.const_union_field_types(&signature, &args, &const_args))?
                .get(name)
                .copied(),
            _ => None,
        }
    }

    pub(super) fn resolved_const_contextual_value_type(
        &mut self,
        value: &ResolvedConstExpr,
        expected: InternedTyId,
    ) -> Option<InternedTyId> {
        self.resolved_const_arg_runtime_type(value, Some(expected))
            .filter(|ty| !self.type_contains_generic(*ty))
            .or_else(|| self.resolved_const_arg_runtime_type(value, None))
    }

    fn substitute_current_ty_generics(
        &mut self,
        ty: InternedTyId,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> Option<InternedTyId> {
        let current_module = self.current_execution_module_id();
        let types = self.type_contexts.get(&current_module)?;
        Some(substitute_ty_generics(types, ty, &|generic| {
            substitutions.get(generic).copied()
        }))
    }

    pub(super) fn expected_nominal_parts(
        &self,
        ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>, Vec<ConstGenericArg>)> {
        match self.ty_kind(ty)? {
            TyKind::Nominal {
                def_id,
                args,
                const_args,
            } => Some((def_id, args, const_args)),
            _ => None,
        }
    }

    pub(super) fn struct_signature_for(
        &self,
        def_id: GlobalDefId,
    ) -> Option<nia_item_signatures::StructSignature> {
        self.signatures_for_module(def_id.module_id)?
            .as_ref()
            .structs
            .get(&def_id.def_id)
            .cloned()
    }

    pub(super) fn union_signature_for(
        &self,
        def_id: GlobalDefId,
    ) -> Option<nia_item_signatures::UnionSignature> {
        self.signatures_for_module(def_id.module_id)?
            .as_ref()
            .unions
            .get(&def_id.def_id)
            .cloned()
    }

    pub(super) fn const_union_field_types(
        &mut self,
        signature: &nia_item_signatures::UnionSignature,
        expected_args: &[InternedTyId],
        expected_const_args: &[ConstGenericArg],
    ) -> Option<SymbolMap<InternedTyId>> {
        self.const_aggregate_field_types(
            &signature.generic_params,
            &signature.fields,
            expected_args,
            expected_const_args,
        )
    }

    pub(super) fn const_struct_field_types(
        &mut self,
        signature: &nia_item_signatures::StructSignature,
        expected_args: &[InternedTyId],
        expected_const_args: &[ConstGenericArg],
    ) -> Option<SymbolMap<InternedTyId>> {
        self.const_aggregate_field_types(
            &signature.generic_params,
            &signature.fields,
            expected_args,
            expected_const_args,
        )
    }

    fn const_aggregate_field_types(
        &mut self,
        generic_params: &[nia_item_signatures::GenericParamSignature],
        signature_fields: &[nia_item_signatures::FieldSignature],
        expected_args: &[InternedTyId],
        expected_const_args: &[ConstGenericArg],
    ) -> Option<SymbolMap<InternedTyId>> {
        let current_module = self.current_execution_module_id();
        let expected_args = expected_args
            .iter()
            .copied()
            .map(|arg| self.type_for_module_or_none(arg, current_module))
            .collect::<Option<Vec<_>>>()?;
        let expected_const_args = expected_const_args
            .iter()
            .cloned()
            .map(|mut arg| {
                arg.ty = self.type_for_module_or_none(arg.ty, current_module)?;
                Some(arg)
            })
            .collect::<Option<Vec<_>>>()?;
        let mut type_index = 0;
        let mut const_index = 0;
        let mut substitutions = SymbolMap::default();
        let mut const_substitutions = SymbolMap::default();
        for param in generic_params {
            match param.kind {
                GenericParamSignatureKind::Type => {
                    substitutions.insert(param.name, *expected_args.get(type_index)?);
                    type_index += 1;
                }
                GenericParamSignatureKind::Const { .. } => {
                    const_substitutions
                        .insert(param.name, expected_const_args.get(const_index)?.clone());
                    const_index += 1;
                }
            }
        }
        if type_index != expected_args.len() || const_index != expected_const_args.len() {
            return None;
        }
        let mut fields = SymbolMap::default();
        for field in signature_fields {
            let canonical = self.type_for_module_or_none(field.ty, current_module)?;
            let ty = {
                let types = self.type_contexts.get(&current_module)?;
                nia_ty::substitute_ty(
                    types.store,
                    &types.append,
                    canonical,
                    &|generic| substitutions.get(generic).copied(),
                    &|generic| const_substitutions.get(generic).cloned(),
                    None,
                )
            };
            fields.insert(field.name, ty);
        }
        Some(fields)
    }

    fn substitute_nominal_args(
        &mut self,
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> Option<InternedTyId> {
        let current_module = self.current_execution_module_id();
        let args = {
            let types = self.type_contexts.get(&current_module)?;
            args.into_iter()
                .map(|arg| {
                    substitute_ty_generics(types, arg, &|generic| {
                        substitutions.get(generic).copied()
                    })
                })
                .collect()
        };
        self.type_contexts.get(&current_module).map(|types| {
            types.intern(TyKind::Nominal {
                def_id,
                args,
                const_args,
            })
        })
    }
}
