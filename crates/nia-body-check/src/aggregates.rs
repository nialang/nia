// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use crate::{BodyChecker, ResolvedEnumSignature, ResolvedStructSignature};
use nia_ast::{Expr, ExprKind};
use nia_defs::DefId;
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, TyId};
use nia_item_signatures::{EnumSignature, StructSignature};
use nia_span::Span;
use nia_ty::{ArrayLenTy, TyKind};

impl<'a> BodyChecker<'a> {
    pub(crate) fn check_array_literal(
        &mut self,
        span: Span,
        expected: Option<TyId>,
        elems: &nia_ast::ArrayElements,
    ) -> TyId {
        let Some(array_ty) = expected else {
            self.diagnostics.push(Diagnostic::error(
                span,
                "array literal requires an expected array type; add a type annotation",
            ));
            for elem in array_literal_values(elems) {
                self.check_expr(elem);
            }
            return self.error();
        };
        let (len, elem_ty) = match self.interner.get(array_ty) {
            Some(TyKind::Array { len, elem }) => (len.clone(), *elem),
            Some(TyKind::Error) | None => return self.error(),
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    "array literal type is not an array",
                ));
                return self.error();
            }
        };
        for elem in array_literal_values(elems) {
            let actual = self.check_expr_with_expected(elem, Some(elem_ty));
            self.expect_expr_type(elem, elem_ty, actual, "array literal element");
        }
        self.check_array_literal_len(span, len, elem_ty, elems)
    }

    fn check_array_literal_len(
        &mut self,
        span: Span,
        len: ArrayLenTy,
        elem_ty: TyId,
        elems: &nia_ast::ArrayElements,
    ) -> TyId {
        match len {
            ArrayLenTy::Infer => {
                let inferred = match elems {
                    nia_ast::ArrayElements::List(elems) => elems.len().to_string(),
                    nia_ast::ArrayElements::Repeat { count, .. } => {
                        match nia_const_eval::eval_array_len_text(&count.text) {
                            Ok(value) => value.to_string(),
                            Err(err) => {
                                self.diagnostics.push(Diagnostic::error(
                                    count.span,
                                    format!(
                                        "array repeat count is not a valid constant: {}",
                                        err.message
                                    ),
                                ));
                                count.text.clone()
                            }
                        }
                    }
                };
                self.interner.intern(TyKind::Array {
                    len: ArrayLenTy::ConstExpr(inferred),
                    elem: elem_ty,
                })
            }
            expected @ (ArrayLenTy::ConstExpr(_) | ArrayLenTy::Builtin { .. }) => {
                match explicit_array_literal_len(elems) {
                    Ok(Some(actual)) => match self.array_len_value(span, &expected) {
                        Ok(expected) => {
                            if expected != actual {
                                self.diagnostics.push(Diagnostic::error(
                                    span,
                                    format!(
                                        "array literal length mismatch: expected {expected}, got {actual}"
                                    ),
                                ));
                            }
                        }
                        Err(err) => self.diagnostics.push(Diagnostic::error(
                            span,
                            format!("array length is not a valid constant: {}", err.message),
                        )),
                    },
                    Ok(None) => {}
                    Err(err) => {
                        self.diagnostics.push(Diagnostic::error(
                            span,
                            format!(
                                "array repeat count is not a valid constant: {}",
                                err.message
                            ),
                        ));
                    }
                }
                self.interner.intern(TyKind::Array {
                    len: expected,
                    elem: elem_ty,
                })
            }
        }
    }

    pub(crate) fn check_struct_literal(
        &mut self,
        span: Span,
        expected: Option<TyId>,
        fields: &[nia_ast::FieldInit],
    ) -> TyId {
        let Some(struct_ty) = expected else {
            self.diagnostics.push(Diagnostic::error(
                span,
                "struct literal requires an expected struct type; add a type annotation",
            ));
            for field in fields {
                self.check_expr(&field.value);
            }
            return self.error();
        };
        let (def_id, args) = match self.interner.get(struct_ty) {
            Some(TyKind::Nominal { def_id, args }) => (*def_id, args.clone()),
            Some(TyKind::Error) | None => return self.error(),
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    "struct literal type is not nominal",
                ));
                return self.error();
            }
        };
        let Some(resolved) = self.resolved_struct_signature(def_id) else {
            self.diagnostics
                .push(Diagnostic::error(span, "struct signature not found"));
            return self.error();
        };
        let generics = resolved.signature.generics.clone();
        let signature_fields = resolved.signature.fields.clone();
        let substitutions = self.generic_substitutions(&generics, &args);
        let field_tys: HashMap<&str, TyId> = signature_fields
            .iter()
            .map(|field| {
                (
                    field.name.as_str(),
                    self.substitute_generics(field.ty, &substitutions),
                )
            })
            .collect();
        let mut seen_fields = HashSet::new();
        for field in fields {
            if !seen_fields.insert(field.name.as_str()) {
                self.diagnostics.push(Diagnostic::error(
                    field.span,
                    format!("duplicate struct field `{}`", field.name),
                ));
            }
            if let Some(expected) = field_tys.get(field.name.as_str()).copied() {
                let actual = self.check_expr_with_expected(&field.value, Some(expected));
                self.expect_expr_type(&field.value, expected, actual, "struct literal field");
            } else {
                self.check_expr(&field.value);
                self.diagnostics.push(Diagnostic::error(
                    field.span,
                    format!("unknown struct field `{}`", field.name),
                ));
            }
        }
        for field in &signature_fields {
            if !seen_fields.contains(field.name.as_str()) {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!("missing struct field `{}`", field.name),
                ));
            }
        }
        struct_ty
    }

    pub(crate) fn check_field_access(&mut self, span: Span, lhs: &Expr, name: &str) -> TyId {
        if self.values.qualified_values.contains_key(&span) {
            return self
                .qualified_global_type(span)
                .unwrap_or_else(|| self.error());
        }
        if let Some(ty) = self.check_enum_variant_access(span, lhs, name) {
            return ty;
        }
        let lhs_ty = self.check_expr(lhs);
        let Some((def_id, args)) = self.field_base_type(lhs_ty) else {
            if lhs_ty != self.error() {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    "field access base is not a struct value or pointer to struct",
                ));
            }
            return self.error();
        };
        let Some(resolved) = self.resolved_struct_signature(def_id) else {
            self.diagnostics
                .push(Diagnostic::error(span, "struct signature not found"));
            return self.error();
        };
        let generics = resolved.signature.generics.clone();
        let fields = resolved.signature.fields.clone();
        let Some(field) = fields.iter().find(|field| field.name == name) else {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("unknown struct field `{name}`"),
            ));
            return self.error();
        };
        let substitutions = self.generic_substitutions(&generics, &args);
        self.substitute_generics(field.ty, &substitutions)
    }

    pub(crate) fn qualified_global_type(&mut self, span: Span) -> Option<TyId> {
        let def_id = self.values.qualified_values.get(&span).copied()?;
        if def_id.module_id == self.defs.module_id {
            return self.global_types.get(&def_id.def_id).copied();
        }
        let program_signature = self.program_globals.get(&def_id)?.clone();
        let ty = program_signature
            .signature
            .explicit_type
            .unwrap_or_else(|| self.error());
        Some(self.import_type_from(&program_signature.interner, ty))
    }

    pub(crate) fn resolved_struct_signature(
        &mut self,
        def_id: GlobalDefId,
    ) -> Option<ResolvedStructSignature> {
        if def_id.module_id == self.defs.module_id {
            let signature = self.signatures.structs.get(&def_id.def_id)?.clone();
            return Some(ResolvedStructSignature { signature });
        }
        let program_signature = self.program_structs.get(&def_id)?.clone();
        let signature = StructSignature {
            generics: program_signature.signature.generics,
            fields: program_signature
                .signature
                .fields
                .into_iter()
                .map(|field| nia_item_signatures::FieldSignature {
                    def_id: field.def_id,
                    name: field.name,
                    ty: self.import_type_from(&program_signature.interner, field.ty),
                    span: field.span,
                })
                .collect(),
            is_extern: program_signature.signature.is_extern,
            span: program_signature.signature.span,
        };
        Some(ResolvedStructSignature { signature })
    }

    pub(crate) fn resolved_enum_signature(
        &mut self,
        def_id: GlobalDefId,
    ) -> Option<ResolvedEnumSignature> {
        if def_id.module_id == self.defs.module_id {
            let signature = self.signatures.enums.get(&def_id.def_id)?.clone();
            return Some(ResolvedEnumSignature { signature });
        }
        let program_signature = self.program_enums.get(&def_id)?.clone();
        let signature = EnumSignature {
            backing_type: self.import_type_from(
                &program_signature.interner,
                program_signature.signature.backing_type,
            ),
            is_open: program_signature.signature.is_open,
            variants: program_signature.signature.variants,
            span: program_signature.signature.span,
        };
        Some(ResolvedEnumSignature { signature })
    }

    pub(crate) fn check_enum_variant_access(
        &mut self,
        span: Span,
        lhs: &Expr,
        name: &str,
    ) -> Option<TyId> {
        let enum_id = self.type_prefix_def_id(lhs)?;
        if !self.is_enum_def(enum_id) {
            return None;
        }
        let Some(variants) = self.enum_variant_scope(enum_id) else {
            self.diagnostics
                .push(Diagnostic::error(span, "enum member scope not found"));
            return Some(self.error());
        };
        if !variants
            .iter()
            .any(|(variant_name, _)| variant_name == name)
        {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("unknown enum variant `{name}`"),
            ));
            return Some(self.error());
        }
        Some(self.interner.intern(TyKind::Nominal {
            def_id: enum_id,
            args: Vec::new(),
        }))
    }

    pub(crate) fn enum_global_def_id(&self, ty: TyId) -> Option<GlobalDefId> {
        let ty = self.normalization.normalize(ty);
        let Some(TyKind::Nominal { def_id, .. }) = self.interner.get(ty) else {
            return None;
        };
        let def_id = *def_id;
        if self.is_enum_def(def_id) {
            Some(def_id)
        } else {
            None
        }
    }

    pub(crate) fn is_enum_def(&self, enum_id: GlobalDefId) -> bool {
        if enum_id.module_id == self.defs.module_id {
            self.signatures.enums.contains_key(&enum_id.def_id)
        } else {
            self.program_enums.contains_key(&enum_id)
        }
    }

    pub(crate) fn enum_variant_scope(&self, enum_id: GlobalDefId) -> Option<Vec<(String, DefId)>> {
        let target_defs = self
            .all_defs
            .iter()
            .find(|defs| defs.module_id == enum_id.module_id)?;
        let scope = target_defs.scopes.enum_members.get(&enum_id.def_id)?;
        Some(
            scope
                .variants
                .entries()
                .map(|(name, def_id)| (name.to_string(), def_id))
                .collect(),
        )
    }

    pub(crate) fn enum_variant_info(&mut self, expr: &Expr) -> Option<(GlobalDefId, DefId)> {
        let ExprKind::Qualified { lhs, name } = &expr.kind else {
            return None;
        };
        let enum_id = self.type_prefix_def_id(lhs)?;
        if !self.is_enum_def(enum_id) {
            return None;
        }
        let scope = self.enum_variant_scope(enum_id)?;
        let variant_id = scope
            .iter()
            .find(|(variant_name, _)| variant_name == name)
            .map(|(_, def_id)| *def_id)?;
        Some((enum_id, variant_id))
    }

    pub(crate) fn check_enum_switch_exhaustive(
        &mut self,
        span: Span,
        enum_id: GlobalDefId,
        has_default: bool,
        covered_variants: &HashSet<DefId>,
    ) {
        if has_default {
            return;
        }
        let Some(resolved) = self.resolved_enum_signature(enum_id) else {
            return;
        };
        if resolved.signature.is_open {
            self.diagnostics.push(Diagnostic::error(
                span,
                "non-exhaustive open enum switch, missing `_`",
            ));
            return;
        }
        let names_and_defs: Vec<(String, DefId)> = resolved
            .signature
            .variants
            .iter()
            .map(|variant| (variant.name.clone(), variant.def_id))
            .collect();
        let missing: Vec<&str> = names_and_defs
            .iter()
            .filter(|(_, def_id)| !covered_variants.contains(def_id))
            .map(|(name, _)| name.as_str())
            .collect();
        if !missing.is_empty() {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "non-exhaustive enum switch, missing: {}",
                    missing.join(", ")
                ),
            ));
        }
    }

    fn field_base_type(&self, ty: TyId) -> Option<(GlobalDefId, Vec<TyId>)> {
        match self.interner.get(ty) {
            Some(TyKind::Nominal { def_id, args }) => Some((*def_id, args.clone())),
            Some(TyKind::Pointer { elem, .. }) => self.field_base_type(*elem),
            _ => None,
        }
    }
}

fn array_literal_values(elems: &nia_ast::ArrayElements) -> Vec<&Expr> {
    match elems {
        nia_ast::ArrayElements::List(elems) => elems.iter().collect(),
        nia_ast::ArrayElements::Repeat { value, .. } => vec![value],
    }
}

fn explicit_array_literal_len(
    elems: &nia_ast::ArrayElements,
) -> Result<Option<u64>, nia_const_eval::ConstEvalError> {
    Ok(match elems {
        nia_ast::ArrayElements::List(elems) => Some(elems.len() as u64),
        nia_ast::ArrayElements::Repeat { count, .. } => {
            Some(nia_const_eval::eval_array_len_text(&count.text)?)
        }
    })
}
