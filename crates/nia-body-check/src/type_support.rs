// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use crate::literals::{
    byte_string_literal_len, c_string_literal_len, float_literal_text, has_numeric_literal_suffix,
    integer_literal_value, integer_range, numeric_literal_suffix, parse_float_literal,
    string_literal_char_len,
};
use nia_ast::{Expr, ExprKind, UnaryOp};
use nia_body_ir::{ArrayToSliceCoercion, CStringPointerCoercion};
use nia_defs::{DefId, DefKind};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalConstExprId, InternedTyId};
use nia_span::Span;
use nia_ty::{ArrayLenTy, PrimitiveTy, TyKind};
use std::collections::HashMap;

impl<'a> BodyChecker<'a> {
    pub(crate) fn normalize_projection(&mut self, ty: InternedTyId) -> InternedTyId {
        let ty = self.normalization.normalize(ty);
        match self.interner.get(ty).cloned() {
            Some(TyKind::Pointer { is_const, elem }) => {
                let elem = self.normalize_projection(elem);
                self.interner.intern(TyKind::Pointer { is_const, elem })
            }
            Some(TyKind::Slice { is_const, elem }) => {
                let elem = self.normalize_projection(elem);
                self.interner.intern(TyKind::Slice { is_const, elem })
            }
            Some(TyKind::Array { len, elem }) => {
                let elem = self.normalize_projection(elem);
                self.interner.intern(TyKind::Array { len, elem })
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                let params = params
                    .into_iter()
                    .map(|param| self.normalize_projection(param))
                    .collect();
                let return_type = self.normalize_projection(return_type);
                self.interner.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                })
            }
            Some(TyKind::Nominal { def_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.normalize_projection(arg))
                    .collect();
                self.interner.intern(TyKind::Nominal { def_id, args })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            }) => {
                let self_ty = self.normalize_projection(self_ty);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.normalize_projection(arg))
                    .collect::<Vec<_>>();
                self.resolve_associated_type_projection_from_current_bounds(
                    self_ty,
                    trait_id,
                    &trait_args,
                    &name,
                )
                .or_else(|| {
                    self.resolve_associated_type_projection(self_ty, trait_id, &trait_args, &name)
                })
                    .unwrap_or_else(|| {
                        self.interner.intern(TyKind::Projection {
                            self_ty,
                            trait_id,
                            trait_args,
                            name,
                        })
                    })
            }
            Some(TyKind::Error | TyKind::Primitive(_) | TyKind::GenericParam(_)) | None => ty,
        }
    }

    fn resolve_associated_type_projection_from_current_bounds(
        &mut self,
        self_ty: InternedTyId,
        trait_id: nia_ids::GlobalDefId,
        trait_args: &[InternedTyId],
        name: &str,
    ) -> Option<InternedTyId> {
        let current_def_id = self.current_def_id?;
        let signature = self.current_function_signature_in_active_interner(current_def_id)?;
        let mut matches = Vec::new();
        for predicate in signature.where_predicates {
            if !self.types_equivalent_without_projection_resolution(predicate.ty, self_ty) {
                continue;
            }
            for bound in predicate.bounds {
                let Some(TyKind::Nominal {
                    def_id: bound_trait_id,
                    args: bound_trait_args,
                }) = self.interner.get(self.normalization.normalize(bound.trait_ty)).cloned()
                else {
                    continue;
                };
                if bound_trait_id != trait_id
                    || bound_trait_args.len() != trait_args.len()
                    || !bound_trait_args.iter().zip(trait_args).all(|(bound, required)| {
                        self.types_equivalent_without_projection_resolution(*bound, *required)
                    })
                {
                    continue;
                }
                matches.extend(
                    bound
                        .associated_type_bindings
                        .iter()
                        .filter(|binding| binding.name == name)
                        .map(|binding| binding.ty),
                );
            }
        }
        match matches.as_slice() {
            [ty] => Some(*ty),
            _ => None,
        }
    }

    fn current_function_signature_in_active_interner(
        &mut self,
        current_def_id: nia_ids::GlobalDefId,
    ) -> Option<nia_item_signatures::FunctionSignature> {
        if current_def_id.module_id == self.defs.module_id {
            let signature = self.signatures.functions.get(&current_def_id.def_id)?.clone();
            let source = self.normalization.interner.clone();
            Some(self.import_function_signature_from(&source, &signature))
        } else {
            Some(self.resolved_function_signature(current_def_id)?.signature)
        }
    }

    fn resolve_associated_type_projection(
        &mut self,
        self_ty: InternedTyId,
        trait_id: nia_ids::GlobalDefId,
        trait_args: &[InternedTyId],
        name: &str,
    ) -> Option<InternedTyId> {
        let impls = self.program_trait_impls.to_vec();
        let mut matches = Vec::new();
        for impl_signature in impls {
            if impl_signature.trait_id != trait_id {
                continue;
            }
            let target_ty =
                self.import_type_from(&impl_signature.interner, impl_signature.target_ty);
            let impl_trait_args = impl_signature
                .trait_args
                .iter()
                .map(|arg| self.import_type_from(&impl_signature.interner, *arg))
                .collect::<Vec<_>>();
            if !self.types_match(target_ty, self_ty)
                || impl_trait_args.len() != trait_args.len()
                || !impl_trait_args
                    .iter()
                    .zip(trait_args)
                    .all(|(actual, expected)| self.types_match(*actual, *expected))
            {
                continue;
            }
            let Some(associated_type) = impl_signature
                .associated_types
                .iter()
                .find(|associated_type| associated_type.name == name)
            else {
                continue;
            };
            matches.push(self.import_type_from(&impl_signature.interner, associated_type.ty));
        }
        match matches.as_slice() {
            [ty] => Some(*ty),
            _ => None,
        }
    }

    pub(crate) fn expect_type(
        &mut self,
        span: Span,
        expected: InternedTyId,
        actual: InternedTyId,
        context: &str,
    ) {
        if expected == self.error() || actual == self.error() || self.types_match(expected, actual)
        {
            return;
        }
        self.diagnostics.push(Diagnostic::error(
            span,
            format!(
                "type mismatch in {context}: expected {}, got {}",
                self.ty_name(expected),
                self.ty_name(actual)
            ),
        ));
    }

    pub(crate) fn expect_expr_type(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        actual: InternedTyId,
        context: &str,
    ) {
        if let Some(coerced) = self.coerce_c_string_to_pointer(expr, expected, actual) {
            self.record_expr_type(expr.span, coerced);
            return;
        }
        if let Some(coerced) = self.coerce_array_to_slice(expr, expected, actual) {
            self.record_expr_type(expr.span, coerced);
            return;
        }
        if !has_numeric_literal_suffix(expr)
            && self.check_integer_literal_range(expr, expected, context)
        {
            self.materialize_literal_expr_type(expr, expected);
            return;
        }
        if !has_numeric_literal_suffix(expr)
            && self.check_float_literal_target(expr, expected, context)
        {
            self.materialize_literal_expr_type(expr, expected);
            return;
        }
        self.expect_type(expr.span, expected, actual, context);
    }

    pub(crate) fn array_expected_from_slice_expected(
        &mut self,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let expected = self.normalization.normalize(expected?);
        match self.interner.get(expected) {
            Some(TyKind::Slice { elem, .. }) => Some(self.interner.intern(TyKind::Array {
                len: ArrayLenTy::Infer,
                elem: *elem,
            })),
            _ => None,
        }
    }

    pub(crate) fn coerce_array_to_slice(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        let expected = self.normalization.normalize(expected);
        let actual = self.normalization.normalize(actual);
        let Some(TyKind::Slice {
            is_const,
            elem: expected_elem,
        }) = self.interner.get(expected)
        else {
            return None;
        };
        let is_const = *is_const;
        let expected_elem = *expected_elem;
        let Some(TyKind::Array {
            elem: actual_elem, ..
        }) = self.interner.get(actual)
        else {
            return None;
        };
        let actual_elem = *actual_elem;
        if !self.types_match(expected_elem, actual_elem) {
            return None;
        }
        if self.is_place_expr(expr) {
            if is_const {
                self.check_addressable(expr, "array-to-slice source");
            } else {
                self.check_assignable(expr, "array-to-slice source");
            }
        }
        self.record_array_to_slice_coercion(
            expr.span,
            ArrayToSliceCoercion {
                array_ty: actual,
                slice_ty: expected,
                is_const,
            },
        );
        Some(expected)
    }

    pub(crate) fn coerce_c_string_to_pointer(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        if !matches!(expr.kind, ExprKind::CString(_)) {
            return None;
        }
        let expected = self.normalization.normalize(expected);
        let actual = self.normalization.normalize(actual);
        let Some(TyKind::Pointer {
            is_const,
            elem: expected_elem,
        }) = self.interner.get(expected)
        else {
            return None;
        };
        let is_const = *is_const;
        let expected_elem = *expected_elem;
        let Some(TyKind::Array {
            elem: actual_elem, ..
        }) = self.interner.get(actual)
        else {
            return None;
        };
        if !self.types_match(expected_elem, *actual_elem)
            || !self.types_match(expected_elem, self.primitive(PrimitiveTy::U8))
        {
            return None;
        }
        self.record_c_string_pointer_coercion(
            expr.span,
            CStringPointerCoercion {
                array_ty: actual,
                pointer_ty: expected,
                is_const,
            },
        );
        Some(expected)
    }

    fn materialize_literal_expr_type(&mut self, expr: &Expr, ty: InternedTyId) {
        self.record_expr_type(expr.span, ty);
        if let ExprKind::Unary {
            op: UnaryOp::Neg,
            expr: inner,
        } = &expr.kind
        {
            self.record_expr_type(inner.span, ty);
        }
    }

    pub(crate) fn check_integer_literal_range(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        context: &str,
    ) -> bool {
        let Some(value) = integer_literal_value(expr) else {
            return false;
        };
        let expected = self.normalization.normalize(expected);
        let Some(TyKind::Primitive(primitive)) = self.interner.get(expected) else {
            return false;
        };
        let Some((min, max)) = integer_range(*primitive) else {
            return false;
        };
        if value < min || value > max {
            self.diagnostics.push(Diagnostic::error(
                expr.span,
                format!(
                    "integer literal {value} is out of range for {} in {context}",
                    self.ty_name(expected)
                ),
            ));
        }
        true
    }

    pub(crate) fn check_integer_literal_enum_backing_range(
        &mut self,
        expr: &Expr,
        expected_enum: InternedTyId,
        context: &str,
    ) -> bool {
        let Some(value) = integer_literal_value(expr) else {
            return false;
        };
        let Some(enum_id) = self.enum_global_def_id(expected_enum) else {
            return false;
        };
        let Some(signature) = self
            .resolved_enum_signature(enum_id)
            .map(|resolved| resolved.signature)
        else {
            return false;
        };
        let backing_type = signature.backing_type;
        let backing_type = self.normalization.normalize(backing_type);
        let Some(TyKind::Primitive(primitive)) = self.interner.get(backing_type) else {
            return false;
        };
        let Some((min, max)) = integer_range(*primitive) else {
            return false;
        };
        if value < min || value > max {
            self.diagnostics.push(Diagnostic::error(
                expr.span,
                format!(
                    "integer literal {value} is out of range for {} backing type in {context}",
                    self.ty_name(expected_enum)
                ),
            ));
        }
        true
    }

    pub(crate) fn check_float_literal_target(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        context: &str,
    ) -> bool {
        let Some(text) = float_literal_text(expr) else {
            return false;
        };
        let expected = self.normalization.normalize(expected);
        let Some(TyKind::Primitive(primitive)) = self.interner.get(expected) else {
            return false;
        };
        match primitive {
            PrimitiveTy::F32 => {
                if !parse_float_literal::<f32>(text) {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        format!("float literal `{text}` is out of range for F32 in {context}"),
                    ));
                }
                true
            }
            PrimitiveTy::F64 => {
                if !parse_float_literal::<f64>(text) {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        format!("float literal `{text}` is out of range for F64 in {context}"),
                    ));
                }
                true
            }
            _ => false,
        }
    }

    pub(crate) fn numeric_literal_has_suffix(&self, expr: &Expr) -> bool {
        has_numeric_literal_suffix(expr)
    }

    pub(crate) fn report_invalid_numeric_literal_suffix(&mut self, expr: &Expr, kind: &str) {
        let suffix = numeric_literal_suffix_for_expr(expr).unwrap_or("<unknown>");
        self.diagnostics.push(Diagnostic::error(
            expr.span,
            format!("invalid {kind} literal suffix `{suffix}`"),
        ));
    }

    pub(crate) fn expect_integer(&mut self, span: Span, actual: InternedTyId, context: &str) {
        if actual == self.error() || self.is_integer(actual) {
            return;
        }
        self.diagnostics.push(Diagnostic::error(
            span,
            format!(
                "type mismatch in {context}: expected integer, got {}",
                self.ty_name(actual)
            ),
        ));
    }

    pub(crate) fn types_match(&self, expected: InternedTyId, actual: InternedTyId) -> bool {
        let mut checker = self.clone_for_type_compare();
        checker.types_match_normalized(expected, actual)
    }

    fn types_match_normalized(&mut self, expected: InternedTyId, actual: InternedTyId) -> bool {
        let expected = self.normalize_projection(expected);
        let actual = self.normalize_projection(actual);
        if self.is_never(actual) {
            return true;
        }
        if expected == actual {
            return true;
        }
        match (
            self.interner.get(expected).cloned(),
            self.interner.get(actual).cloned(),
        ) {
            (
                Some(TyKind::Array {
                    len: ArrayLenTy::Infer,
                    elem: expected_elem,
                }),
                Some(TyKind::Array {
                    elem: actual_elem, ..
                }),
            ) if self.types_match_normalized(expected_elem, actual_elem) => true,
            (
                Some(TyKind::Array {
                    len: expected_len,
                    elem: expected_elem,
                }),
                Some(TyKind::Array {
                    len: actual_len,
                    elem: actual_elem,
                }),
            ) if self.types_match_normalized(expected_elem, actual_elem) => {
                let Ok(expected_len) = self.array_len_value(Span::default(), &expected_len) else {
                    return false;
                };
                let Ok(actual_len) = self.array_len_value(Span::default(), &actual_len) else {
                    return false;
                };
                expected_len == actual_len
            }
            (
                Some(TyKind::Projection {
                    self_ty: expected_self,
                    trait_id: expected_trait,
                    trait_args: expected_args,
                    name: expected_name,
                }),
                Some(TyKind::Projection {
                    self_ty: actual_self,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    name: actual_name,
                }),
            ) => {
                expected_trait == actual_trait
                    && expected_name == actual_name
                    && expected_args.len() == actual_args.len()
                    && self.types_match_normalized(expected_self, actual_self)
                    && expected_args
                        .iter()
                        .zip(actual_args.iter())
                        .all(|(expected, actual)| self.types_match_normalized(*expected, *actual))
            }
            _ => false,
        }
    }

    fn clone_for_type_compare(&self) -> BodyChecker<'a> {
        BodyChecker {
            source_version: self.source_version,
            origins: self.origins,
            module: self.module,
            defs: self.defs,
            program: self.program,
            values: self.values,
            locals: self.locals,
            interner: self.interner.clone(),
            type_uses: self.type_uses,
            signatures: self.signatures,
            normalization: self.normalization,
            comptime: self.comptime,
            layouts: self.layouts,
            extensions: self.extensions,
            program_functions: self.program_functions,
            program_globals: self.program_globals,
            program_comptimes: self.program_comptimes,
            program_structs: self.program_structs,
            program_unions: self.program_unions,
            program_enums: self.program_enums,
            program_traits: self.program_traits,
            program_trait_impls: self.program_trait_impls,
            program_comptime: self.program_comptime,
            expr_types: HashMap::new(),
            bracket_suffix_resolutions: HashMap::new(),
            array_to_slice_coercions: HashMap::new(),
            c_string_pointer_coercions: HashMap::new(),
            builtin_values: HashMap::new(),
            resolved_calls: HashMap::new(),
            function_references: HashMap::new(),
            node_expr_types: HashMap::new(),
            node_bracket_suffix_resolutions: HashMap::new(),
            node_array_to_slice_coercions: HashMap::new(),
            node_c_string_pointer_coercions: HashMap::new(),
            node_builtin_values: HashMap::new(),
            node_resolved_calls: HashMap::new(),
            node_function_references: HashMap::new(),
            generic_instantiations: Vec::new(),
            function_bodies: HashMap::new(),
            global_inits: HashMap::new(),
            local_types: HashMap::new(),
            global_types: HashMap::new(),
            comptime_types: HashMap::new(),
            diagnostics: Vec::new(),
            current_return: self.current_return,
            current_def_id: self.current_def_id,
            current_param_locals: self.current_param_locals.clone(),
        }
    }

    pub(crate) fn materialize_inferred_array_type(
        &self,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        match (self.interner.get(expected), self.interner.get(actual)) {
            (
                Some(TyKind::Array {
                    len: ArrayLenTy::Infer,
                    elem: expected_elem,
                }),
                Some(TyKind::Array {
                    elem: actual_elem, ..
                }),
            ) if self.types_match(*expected_elem, *actual_elem) => Some(actual),
            _ => None,
        }
    }

    pub(crate) fn def_id_for_span(&mut self, span: Span, expected: DefKind) -> Option<DefId> {
        let def_id = self.defs.def_spans.get(span)?;
        let def = self.defs.defs.get(def_id)?;
        if def.kind == expected {
            Some(def_id)
        } else {
            None
        }
    }

    pub(crate) fn ty_for_span(&self, span: Span) -> InternedTyId {
        self.type_uses
            .get(&span)
            .copied()
            .unwrap_or_else(|| self.error())
    }

    pub(crate) fn layout_of(&self, ty: InternedTyId) -> Option<nia_layout::TypeLayout> {
        let ty = self.normalization.normalize(ty);
        self.layouts.types.get(&ty).cloned()
    }

    pub(crate) fn array_len_value(&self, span: Span, len: &ArrayLenTy) -> Result<u64, String> {
        match len {
            ArrayLenTy::ConstValue(value) => Ok(*value),
            ArrayLenTy::ConstExpr(id) => self
                .array_len_const_expr_value(*id)
                .ok_or_else(|| "array length was not evaluated by comptime".to_string()),
            ArrayLenTy::Builtin { name, ty } => {
                let Some(layout) = self.layout_of(*ty) else {
                    return Err(format!(
                        "cannot compute layout for array length builtin `@{name}`"
                    ));
                };
                match name.as_str() {
                    "size" => Ok(layout.size),
                    "align" => Ok(layout.align),
                    _ => Err(format!("unsupported array length builtin `@{name}`")),
                }
            }
            ArrayLenTy::Infer => Err(format!("array length at {span:?} is not concrete")),
        }
    }

    pub(crate) fn ty_name(&self, ty: InternedTyId) -> String {
        match self.interner.get(ty) {
            Some(TyKind::Primitive(primitive)) => primitive_ty_name(*primitive).to_string(),
            Some(TyKind::Pointer { is_const, elem }) => {
                let const_part = if *is_const { "const " } else { "" };
                format!("&{const_part}{}", self.ty_name(*elem))
            }
            Some(TyKind::Slice { is_const, elem }) => {
                let const_part = if *is_const { "const " } else { "" };
                format!("&{const_part}[{}]", self.ty_name(*elem))
            }
            Some(TyKind::Array { len, elem }) => {
                format!("[{}]{}", self.array_len_name(len), self.ty_name(*elem))
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                let mut params = params
                    .iter()
                    .map(|param| self.ty_name(*param))
                    .collect::<Vec<_>>();
                if *is_variadic {
                    params.push("...".to_string());
                }
                let return_part = if self.is_void(*return_type) {
                    String::new()
                } else {
                    format!(" {}", self.ty_name(*return_type))
                };
                format!("&const fn({}){return_part}", params.join(", "))
            }
            Some(TyKind::Nominal { def_id, args }) => self.nominal_ty_name(*def_id, args),
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            }) => {
                let self_ty = self.ty_name(*self_ty);
                let trait_name = self.nominal_ty_name(*trait_id, trait_args);
                format!("[{self_ty} as {trait_name}]::{name}")
            }
            Some(TyKind::GenericParam(name)) => name.clone(),
            Some(TyKind::Error) => "<error type>".to_string(),
            None => "<unknown type>".to_string(),
        }
    }

    fn array_len_name(&self, len: &ArrayLenTy) -> String {
        match len {
            ArrayLenTy::Infer => "_".to_string(),
            ArrayLenTy::ConstValue(value) => value.to_string(),
            ArrayLenTy::ConstExpr(id) => self
                .array_len_const_expr_value(*id)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<unevaluated const>".to_string()),
            ArrayLenTy::Builtin { name, ty } => format!("@{name}[{}]()", self.ty_name(*ty)),
        }
    }

    fn array_len_const_expr_value(&self, id: GlobalConstExprId) -> Option<u64> {
        if id.module_id == self.defs.module_id {
            return self.comptime.array_lengths.get(&id).copied();
        }
        self.program_comptime
            .get(&id.module_id)
            .and_then(|comptime| comptime.array_lengths.get(&id).copied())
    }

    pub(crate) fn nominal_ty_name(
        &self,
        def_id: nia_ids::GlobalDefId,
        args: &[InternedTyId],
    ) -> String {
        let base = self
            .defs_for_module(def_id.module_id)
            .and_then(|defs| defs.defs.get(def_id.def_id))
            .map(|def| def.name.clone())
            .unwrap_or_else(|| "<unknown type>".to_string());
        if args.is_empty() {
            base
        } else {
            let args = args
                .iter()
                .map(|arg| self.ty_name(*arg))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{base}[{args}]")
        }
    }

    pub(crate) fn primitive(&self, primitive: PrimitiveTy) -> InternedTyId {
        self.interner.primitive(primitive)
    }

    pub(crate) fn string_literal_type(&mut self, literal: &nia_ast::StringLiteral) -> InternedTyId {
        let len = string_literal_char_len(literal).unwrap_or(0);
        self.interner.intern(TyKind::Array {
            len: ArrayLenTy::ConstValue(len as u64),
            elem: self.primitive(PrimitiveTy::Char),
        })
    }

    pub(crate) fn byte_string_literal_type(
        &mut self,
        literal: &nia_ast::StringLiteral,
    ) -> InternedTyId {
        let len = byte_string_literal_len(literal).unwrap_or(0);
        self.interner.intern(TyKind::Array {
            len: ArrayLenTy::ConstValue(len as u64),
            elem: self.primitive(PrimitiveTy::U8),
        })
    }

    pub(crate) fn c_string_literal_type(
        &mut self,
        literal: &nia_ast::StringLiteral,
    ) -> InternedTyId {
        let len = c_string_literal_len(literal).unwrap_or(0);
        self.interner.intern(TyKind::Array {
            len: ArrayLenTy::ConstValue(len as u64),
            elem: self.primitive(PrimitiveTy::U8),
        })
    }

    pub(crate) fn void(&self) -> InternedTyId {
        self.primitive(PrimitiveTy::Void)
    }

    pub(crate) fn never(&self) -> InternedTyId {
        self.primitive(PrimitiveTy::Never)
    }

    pub(crate) fn is_void(&self, ty: InternedTyId) -> bool {
        ty == self.void()
    }

    pub(crate) fn is_never(&self, ty: InternedTyId) -> bool {
        self.normalization.normalize(ty) == self.never()
    }

    pub(crate) fn is_invalid_temporary_type(&self, ty: InternedTyId) -> bool {
        self.is_void(ty) || self.is_never(ty)
    }

    pub(crate) fn is_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(ty),
            Some(TyKind::Primitive(
                PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize
                    | PrimitiveTy::U8
                    | PrimitiveTy::U16
                    | PrimitiveTy::U32
                    | PrimitiveTy::U64
                    | PrimitiveTy::U128
                    | PrimitiveTy::Usize
            ))
        )
    }

    pub(crate) fn is_char(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(ty),
            Some(TyKind::Primitive(PrimitiveTy::Char))
        )
    }

    pub(crate) fn is_u32(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(ty),
            Some(TyKind::Primitive(PrimitiveTy::U32))
        )
    }

    pub(crate) fn is_numeric(&self, ty: InternedTyId) -> bool {
        self.is_integer(ty)
            || matches!(
                self.interner.get(ty),
                Some(TyKind::Primitive(PrimitiveTy::F32 | PrimitiveTy::F64))
            )
    }

    pub(crate) fn is_pointer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalization.normalize(ty)),
            Some(TyKind::Pointer { .. } | TyKind::FunctionPointer { .. })
        )
    }

    pub(crate) fn is_pointer_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalization.normalize(ty)),
            Some(TyKind::Primitive(PrimitiveTy::Usize | PrimitiveTy::Isize))
        )
    }

    pub(crate) fn is_enum(&self, ty: InternedTyId) -> bool {
        self.enum_global_def_id(ty).is_some()
    }

    pub(crate) fn is_open_enum(&self, ty: InternedTyId) -> bool {
        let Some(enum_id) = self.enum_global_def_id(ty) else {
            return false;
        };
        if enum_id.module_id == self.defs.module_id {
            self.signatures
                .enums
                .get(&enum_id.def_id)
                .is_some_and(|signature| signature.is_open)
        } else {
            self.program_enums
                .get(&enum_id)
                .is_some_and(|program_enum| program_enum.signature.is_open)
        }
    }

    pub(crate) fn bool(&self) -> InternedTyId {
        self.primitive(PrimitiveTy::Bool)
    }

    pub(crate) fn i32(&self) -> InternedTyId {
        self.primitive(PrimitiveTy::I32)
    }

    pub(crate) fn f64(&self) -> InternedTyId {
        self.primitive(PrimitiveTy::F64)
    }

    pub(crate) fn error(&self) -> InternedTyId {
        self.interner.error()
    }
}

fn numeric_literal_suffix_for_expr(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::Integer(text) | ExprKind::Float(text) => numeric_literal_suffix(text),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => numeric_literal_suffix_for_expr(expr),
        _ => None,
    }
}

fn primitive_ty_name(primitive: PrimitiveTy) -> &'static str {
    match primitive {
        PrimitiveTy::I8 => "i8",
        PrimitiveTy::I16 => "i16",
        PrimitiveTy::I32 => "i32",
        PrimitiveTy::I64 => "i64",
        PrimitiveTy::I128 => "i128",
        PrimitiveTy::Isize => "isize",
        PrimitiveTy::U8 => "u8",
        PrimitiveTy::U16 => "u16",
        PrimitiveTy::U32 => "u32",
        PrimitiveTy::U64 => "u64",
        PrimitiveTy::U128 => "u128",
        PrimitiveTy::Usize => "usize",
        PrimitiveTy::F32 => "f32",
        PrimitiveTy::F64 => "f64",
        PrimitiveTy::Bool => "bool",
        PrimitiveTy::Char => "char",
        PrimitiveTy::Void => "void",
        PrimitiveTy::Never => "!",
    }
}
