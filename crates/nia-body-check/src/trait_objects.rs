// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::Expr;
use nia_body_ir::{TraitObjectCoercion, TraitObjectUpcast};
use nia_diagnostic::Diagnostic;
use nia_ids::InternedTyId;
use nia_span::Span;
use nia_trait_solve::{TraitGoal, TraitSolverContext};
use nia_ty::{TraitId, TyKind};

impl<'a> BodyChecker<'a> {
    pub(crate) fn coerce_trait_object_to_supertrait(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        let expected = self.normalization.normalize(expected);
        let actual = self.normalization.normalize(actual);
        let Some(TyKind::TraitObject {
            is_const: expected_const,
            trait_id: expected_trait,
            trait_args: expected_args,
            associated_type_bindings: expected_bindings,
        }) = self.interner.get(expected).cloned()
        else {
            return None;
        };
        let Some(TyKind::TraitObject {
            is_const: actual_const,
            trait_id: actual_trait,
            trait_args: actual_args,
            associated_type_bindings: actual_bindings,
        }) = self.interner.get(actual).cloned()
        else {
            return None;
        };
        if self.types_match(expected, actual)
            || expected_const != actual_const
            || !expected_bindings.is_empty()
            || !actual_bindings.is_empty()
            || !self.trait_object_has_supertrait(
                actual_trait,
                &actual_args,
                expected_trait,
                &expected_args,
            )
        {
            return None;
        }
        self.record_trait_object_upcast(
            expr.span,
            TraitObjectUpcast {
                source_ty: actual,
                target_ty: expected,
            },
        );
        Some(expected)
    }

    pub(crate) fn coerce_pointer_to_trait_object(
        &mut self,
        expr: &Expr,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Option<InternedTyId> {
        let expected = self.normalization.normalize(expected);
        let actual = self.normalization.normalize(actual);
        let Some(TyKind::TraitObject {
            is_const: expected_const,
            trait_id,
            trait_args,
            associated_type_bindings,
        }) = self.interner.get(expected).cloned()
        else {
            return None;
        };
        let Some(TyKind::Pointer {
            is_const: actual_const,
            elem: self_ty,
        }) = self.interner.get(actual).cloned()
        else {
            return None;
        };
        if !expected_const && actual_const {
            return None;
        }
        if !self.is_object_safe_trait_object(expr.span, trait_id, &trait_args) {
            return None;
        }
        if !self.trait_object_bindings_match_impl(
            self_ty,
            trait_id,
            &trait_args,
            &associated_type_bindings,
        ) {
            return None;
        }
        self.record_trait_object_coercion(
            expr.span,
            TraitObjectCoercion {
                source_ty: actual,
                target_ty: expected,
            },
        );
        Some(expected)
    }

    pub(crate) fn trait_object_coercion_self_ty(
        &mut self,
        source_ty: InternedTyId,
    ) -> InternedTyId {
        let source_ty = self.normalization.normalize(source_ty);
        match self.interner.get(source_ty).cloned() {
            Some(TyKind::Pointer { elem, .. }) => elem,
            _ => source_ty,
        }
    }

    pub(crate) fn trait_object_bindings_match_impl(
        &mut self,
        self_ty: InternedTyId,
        trait_id: nia_ty::TraitId,
        trait_args: &[InternedTyId],
        associated_type_bindings: &[(String, InternedTyId)],
    ) -> bool {
        let assumptions = self.current_trait_goals();
        let context = TraitSolverContext {
            normalization: self.normalization,
            trait_impls: self.program_trait_impls,
            layouts: Some(self.layouts),
            local_module_id: self.defs.module_id,
            local_enums: &self.signatures.enums,
            program_enums: Some(self.program_enums),
        };
        let proven = {
            let mut solver = context.solver(&mut self.interner, &assumptions);
            solver.proves(TraitGoal {
                self_ty,
                trait_id,
                trait_args: trait_args.to_vec(),
            })
        };
        if !proven {
            return false;
        }
        associated_type_bindings.iter().all(|(name, expected)| {
            self.resolve_associated_type_projection(self_ty, trait_id, trait_args, name)
                .is_some_and(|actual| self.types_match(actual, *expected))
        })
    }

    pub(crate) fn is_object_safe_trait_object(
        &mut self,
        span: Span,
        trait_id: nia_ty::TraitId,
        trait_args: &[InternedTyId],
    ) -> bool {
        let nia_ty::TraitId::Source(source_trait_id) = trait_id else {
            self.diagnostics.push(Diagnostic::error(
                span,
                "builtin trait objects are not supported yet",
            ));
            return false;
        };
        let Some(trait_signature) = self.resolved_trait_signature(source_trait_id) else {
            self.diagnostics.push(Diagnostic::error(
                span,
                "trait object refers to unknown trait",
            ));
            return false;
        };
        let mut ok = true;
        let self_ty = self
            .interner
            .intern(TyKind::GenericParam("Self".to_string()));
        self.check_object_safe_trait_signature(
            span,
            source_trait_id,
            &trait_signature,
            trait_args,
            self_ty,
            &mut Vec::new(),
            &mut ok,
        );
        ok
    }

    pub(crate) fn check_object_safe_type(&mut self, span: Span, ty: InternedTyId) {
        let ty = self.normalization.normalize(ty);
        match self.interner.get(ty).cloned() {
            Some(TyKind::Pointer { elem, .. }) | Some(TyKind::Slice { elem, .. }) => {
                self.check_object_safe_type(span, elem);
            }
            Some(TyKind::Array { elem, .. }) => self.check_object_safe_type(span, elem),
            Some(TyKind::Range { bound, .. }) => {
                if let Some(bound) = bound {
                    self.check_object_safe_type(span, bound);
                }
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                for param in params {
                    self.check_object_safe_type(span, param);
                }
                self.check_object_safe_type(span, return_type);
            }
            Some(TyKind::Nominal { args, .. }) | Some(TyKind::BuiltinTrait { args, .. }) => {
                for arg in args {
                    self.check_object_safe_type(span, arg);
                }
            }
            Some(TyKind::TraitObject {
                trait_id,
                trait_args,
                associated_type_bindings,
                ..
            }) => {
                self.is_object_safe_trait_object(span, trait_id, &trait_args);
                for arg in trait_args {
                    self.check_object_safe_type(span, arg);
                }
                for (_, ty) in associated_type_bindings {
                    self.check_object_safe_type(span, ty);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.check_object_safe_type(span, self_ty);
                for arg in trait_args {
                    self.check_object_safe_type(span, arg);
                }
            }
            Some(TyKind::Error | TyKind::Primitive(_) | TyKind::GenericParam(_)) | None => {}
        }
    }

    fn check_object_safe_trait_signature(
        &mut self,
        span: Span,
        trait_id: nia_ids::GlobalDefId,
        trait_signature: &nia_item_signatures::TraitSignature,
        trait_args: &[InternedTyId],
        self_ty: InternedTyId,
        visiting: &mut Vec<nia_ids::GlobalDefId>,
        ok: &mut bool,
    ) {
        if visiting.contains(&trait_id) {
            return;
        }
        visiting.push(trait_id);
        for method in &trait_signature.methods {
            if method
                .signature
                .params
                .first()
                .is_none_or(|param| param.receiver.is_none())
            {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "trait `{}` is not object safe because method `{}` has no receiver",
                        self.nominal_ty_name(trait_id, trait_args),
                        method.name
                    ),
                ));
                *ok = false;
            }
            if !method.signature.generics.is_empty() {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "trait `{}` is not object safe because method `{}` has method generics",
                        self.nominal_ty_name(trait_id, trait_args),
                        method.name
                    ),
                ));
                *ok = false;
            }
            if method
                .signature
                .params
                .first()
                .is_some_and(|param| param.receiver == Some(nia_ast::ReceiverKind::Value))
            {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "trait `{}` is not object safe because method `{}` takes `self` by value",
                        self.nominal_ty_name(trait_id, trait_args),
                        method.name
                    ),
                ));
                *ok = false;
            }
            let substitutions = self.generic_substitutions(&trait_signature.generics, trait_args);
            for param in method.signature.params.iter().skip(1) {
                let ty = self.substitute_generics(param.ty, &substitutions);
                if self.type_mentions_self(ty, self_ty) {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!(
                            "trait `{}` is not object safe because method `{}` mentions `Self` outside the receiver",
                            self.nominal_ty_name(trait_id, trait_args),
                            method.name
                        ),
                    ));
                    *ok = false;
                }
            }
            let return_type =
                self.substitute_generics(method.signature.return_type, &substitutions);
            if self.type_mentions_self(return_type, self_ty) {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "trait `{}` is not object safe because method `{}` returns `Self`",
                        self.nominal_ty_name(trait_id, trait_args),
                        method.name
                    ),
                ));
                *ok = false;
            }
        }
        let substitutions = self.generic_substitutions(&trait_signature.generics, trait_args);
        for supertrait in &trait_signature.supertraits {
            let supertrait = self.substitute_generics(*supertrait, &substitutions);
            let Some(TyKind::Nominal {
                def_id: supertrait_id,
                args: supertrait_args,
            }) = self
                .interner
                .get(self.normalization.normalize(supertrait))
                .cloned()
            else {
                continue;
            };
            let Some(supertrait_signature) = self.resolved_trait_signature(supertrait_id) else {
                continue;
            };
            self.check_object_safe_trait_signature(
                span,
                supertrait_id,
                &supertrait_signature,
                &supertrait_args,
                self_ty,
                visiting,
                ok,
            );
        }
        visiting.pop();
    }

    fn type_mentions_self(&mut self, ty: InternedTyId, self_ty: InternedTyId) -> bool {
        let ty = self.normalization.normalize(ty);
        if ty == self_ty {
            return true;
        }
        match self.interner.get(ty).cloned() {
            Some(TyKind::Pointer { elem, .. }) | Some(TyKind::Slice { elem, .. }) => {
                self.type_mentions_self(elem, self_ty)
            }
            Some(TyKind::Array { elem, .. }) => self.type_mentions_self(elem, self_ty),
            Some(TyKind::Range { bound, .. }) => {
                bound.is_some_and(|bound| self.type_mentions_self(bound, self_ty))
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                params
                    .into_iter()
                    .any(|param| self.type_mentions_self(param, self_ty))
                    || self.type_mentions_self(return_type, self_ty)
            }
            Some(TyKind::Nominal { args, .. }) | Some(TyKind::BuiltinTrait { args, .. }) => args
                .into_iter()
                .any(|arg| self.type_mentions_self(arg, self_ty)),
            Some(TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            }) => {
                trait_args
                    .into_iter()
                    .any(|arg| self.type_mentions_self(arg, self_ty))
                    || associated_type_bindings
                        .into_iter()
                        .any(|(_, ty)| self.type_mentions_self(ty, self_ty))
            }
            Some(TyKind::Projection {
                self_ty: projection_self,
                trait_args,
                ..
            }) => {
                self.type_mentions_self(projection_self, self_ty)
                    || trait_args
                        .into_iter()
                        .any(|arg| self.type_mentions_self(arg, self_ty))
            }
            Some(TyKind::Error | TyKind::Primitive(_) | TyKind::GenericParam(_)) | None => false,
        }
    }

    fn trait_object_has_supertrait(
        &mut self,
        source_trait: TraitId,
        source_args: &[InternedTyId],
        target_trait: TraitId,
        target_args: &[InternedTyId],
    ) -> bool {
        self.trait_object_has_supertrait_inner(
            source_trait,
            source_args,
            target_trait,
            target_args,
            &mut Vec::new(),
        )
    }

    fn trait_object_has_supertrait_inner(
        &mut self,
        source_trait: TraitId,
        source_args: &[InternedTyId],
        target_trait: TraitId,
        target_args: &[InternedTyId],
        visited: &mut Vec<(TraitId, Vec<InternedTyId>)>,
    ) -> bool {
        if source_trait == target_trait
            && source_args.len() == target_args.len()
            && source_args
                .iter()
                .zip(target_args.iter())
                .all(|(source, target)| self.types_match(*source, *target))
        {
            return true;
        }
        if visited
            .iter()
            .any(|(trait_id, args)| *trait_id == source_trait && args == source_args)
        {
            return false;
        }
        visited.push((source_trait, source_args.to_vec()));
        match source_trait {
            TraitId::Builtin(trait_id) => trait_id.supertraits().iter().any(|supertrait| {
                let supertrait_args = if supertrait.preserves_trait_args {
                    source_args.to_vec()
                } else {
                    Vec::new()
                };
                self.trait_object_has_supertrait_inner(
                    TraitId::Builtin(supertrait.trait_id),
                    &supertrait_args,
                    target_trait,
                    target_args,
                    visited,
                )
            }),
            TraitId::Source(source_trait_id) => {
                let Some(signature) = self.resolved_trait_signature(source_trait_id) else {
                    return false;
                };
                let substitutions = self.generic_substitutions(&signature.generics, source_args);
                signature.supertraits.iter().any(|supertrait| {
                    let supertrait = self.substitute_generics(*supertrait, &substitutions);
                    let supertrait = self.normalization.normalize(supertrait);
                    match self.interner.get(supertrait).cloned() {
                        Some(TyKind::Nominal { def_id, args }) => self
                            .trait_object_has_supertrait_inner(
                                TraitId::Source(def_id),
                                &args,
                                target_trait,
                                target_args,
                                visited,
                            ),
                        Some(TyKind::BuiltinTrait { trait_id, args }) => self
                            .trait_object_has_supertrait_inner(
                                TraitId::Builtin(trait_id),
                                &args,
                                target_trait,
                                target_args,
                                visited,
                            ),
                        _ => false,
                    }
                })
            }
        }
    }
}
