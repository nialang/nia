// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

use crate::BodyChecker;
use nia_defs::{DefId, DefKind};
use nia_ids::{GlobalDefId, InternedTyId};
use nia_item_signatures::{FunctionSignature, TraitImplSignature};
use nia_span::Span;
use nia_ty::{BuiltinTrait, TraitId, TyKind};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TraitObligation {
    self_ty: InternedTyId,
    trait_id: TraitId,
    trait_args: Vec<InternedTyId>,
}

impl<'a> BodyChecker<'a> {
    pub(crate) fn current_context_proves_trait_obligation(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
    ) -> bool {
        let obligations = self
            .current_def_id
            .and_then(|def_id| (def_id.module_id == self.defs.module_id).then_some(def_id.def_id))
            .and_then(|def_id| {
                let signature = self.signatures.functions.get(&def_id)?.clone();
                Some(self.function_signature_trait_obligations(def_id, &signature))
            })
            .unwrap_or_default();
        let required = TraitObligation {
            self_ty,
            trait_id,
            trait_args,
        };
        self.proves_trait_obligation(&obligations, &required)
    }

    pub(crate) fn check_function_signature_projection_obligations(
        &mut self,
        def_id: DefId,
        signature: &FunctionSignature,
    ) {
        let obligations = self.function_signature_trait_obligations(def_id, signature);
        for param in &signature.params {
            self.check_type_projection_obligations(param.span, param.ty, &obligations);
        }
        self.check_type_projection_obligations(signature.span, signature.return_type, &obligations);
        for predicate in &signature.where_predicates {
            self.check_type_projection_obligations(predicate.span, predicate.ty, &obligations);
            for bound in &predicate.bounds {
                self.check_type_projection_obligations(
                    predicate.span,
                    bound.trait_ty,
                    &obligations,
                );
                for binding in &bound.associated_type_bindings {
                    self.check_type_projection_obligations(binding.span, binding.ty, &obligations);
                }
            }
        }
    }

    fn function_signature_trait_obligations(
        &mut self,
        def_id: DefId,
        signature: &FunctionSignature,
    ) -> Vec<TraitObligation> {
        let mut obligations = Vec::new();
        if let Some(trait_obligation) = self.method_trait_obligation(def_id) {
            self.push_trait_obligation_with_supertraits(&mut obligations, trait_obligation);
        }
        for predicate in &signature.where_predicates {
            for bound in &predicate.bounds {
                self.push_trait_obligation_from_bound(
                    &mut obligations,
                    predicate.ty,
                    bound.trait_ty,
                );
            }
        }
        obligations
    }

    fn method_trait_obligation(&mut self, def_id: DefId) -> Option<TraitObligation> {
        let method = self.defs.defs.get(def_id)?;
        match method.kind {
            DefKind::TraitMethod => {
                let trait_id = GlobalDefId {
                    module_id: self.defs.module_id,
                    def_id: method.parent?,
                };
                let trait_signature = self.resolved_trait_signature(trait_id)?;
                let trait_args = trait_signature
                    .generics
                    .iter()
                    .map(|generic| self.interner.intern(TyKind::GenericParam(generic.clone())))
                    .collect();
                Some(TraitObligation {
                    self_ty: self
                        .interner
                        .intern(TyKind::GenericParam("Self".to_string())),
                    trait_id: TraitId::Source(trait_id),
                    trait_args,
                })
            }
            DefKind::Method => {
                let method_id = self.global_def_id(def_id);
                let trait_id = self.extension_trait_id_for_method(method_id)?;
                let target_ty = self.method_owner_type(def_id)?;
                let impl_signature = self.trait_impl_signature_for_method(method_id)?.clone();
                let trait_args = self.trait_impl_signature_args(&impl_signature, trait_id)?;
                Some(TraitObligation {
                    self_ty: target_ty,
                    trait_id,
                    trait_args,
                })
            }
            _ => None,
        }
    }

    fn trait_impl_signature_for_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<&TraitImplSignature> {
        if method_id.module_id != self.defs.module_id {
            return None;
        }
        self.module.items.iter().find_map(|item| {
            let nia_ast::ItemKind::Extend(extend) = &item.kind else {
                return None;
            };
            let has_method = extend.methods.iter().any(|method| {
                self.defs
                    .def_spans
                    .get(method.function.span)
                    .is_some_and(|def_id| def_id == method_id.def_id)
            });
            if !has_method {
                return None;
            }
            self.signatures
                .trait_impls
                .iter()
                .find(|signature| signature.span == extend.target.span)
        })
    }

    fn trait_impl_signature_args(
        &mut self,
        impl_signature: &TraitImplSignature,
        trait_id: TraitId,
    ) -> Option<Vec<InternedTyId>> {
        let trait_ty = self.normalization.normalize(impl_signature.trait_ty?);
        match self.interner.get(trait_ty) {
            Some(TyKind::Nominal { def_id, args }) if TraitId::Source(*def_id) == trait_id => {
                Some(args.clone())
            }
            Some(TyKind::BuiltinTrait { trait_id: id, args })
                if TraitId::Builtin(*id) == trait_id =>
            {
                Some(args.clone())
            }
            _ => None,
        }
    }

    fn push_trait_obligation_from_bound(
        &mut self,
        obligations: &mut Vec<TraitObligation>,
        self_ty: InternedTyId,
        bound: InternedTyId,
    ) {
        let bound = self.normalization.normalize(bound);
        let Some((trait_id, args)) = self.trait_id_and_args(bound) else {
            return;
        };
        self.push_trait_obligation_with_supertraits(
            obligations,
            TraitObligation {
                self_ty,
                trait_id,
                trait_args: args,
            },
        );
    }

    pub(crate) fn is_trait_def_id(&self, def_id: GlobalDefId) -> bool {
        self.defs_for_module(def_id.module_id)
            .and_then(|defs| defs.defs.get(def_id.def_id))
            .is_some_and(|def| def.kind == DefKind::Trait)
    }

    fn push_trait_obligation_with_supertraits(
        &mut self,
        obligations: &mut Vec<TraitObligation>,
        obligation: TraitObligation,
    ) {
        self.push_trait_obligation_with_supertraits_inner(
            obligations,
            obligation,
            &mut HashSet::new(),
        );
    }

    fn push_trait_obligation_with_supertraits_inner(
        &mut self,
        obligations: &mut Vec<TraitObligation>,
        obligation: TraitObligation,
        visited: &mut HashSet<(TraitId, Vec<InternedTyId>)>,
    ) {
        let key = (obligation.trait_id, obligation.trait_args.clone());
        if !visited.insert(key) {
            return;
        }
        if !obligations
            .iter()
            .any(|existing| self.trait_obligations_equivalent(existing, &obligation))
        {
            obligations.push(obligation.clone());
        }
        match obligation.trait_id {
            TraitId::Builtin(BuiltinTrait::Deref) => self
                .push_trait_obligation_with_supertraits_inner(
                    obligations,
                    TraitObligation {
                        self_ty: obligation.self_ty,
                        trait_id: TraitId::Builtin(BuiltinTrait::DerefConst),
                        trait_args: Vec::new(),
                    },
                    visited,
                ),
            TraitId::Builtin(BuiltinTrait::Index) => self
                .push_trait_obligation_with_supertraits_inner(
                    obligations,
                    TraitObligation {
                        self_ty: obligation.self_ty,
                        trait_id: TraitId::Builtin(BuiltinTrait::IndexConst),
                        trait_args: obligation.trait_args.clone(),
                    },
                    visited,
                ),
            TraitId::Builtin(BuiltinTrait::Slice) => self
                .push_trait_obligation_with_supertraits_inner(
                    obligations,
                    TraitObligation {
                        self_ty: obligation.self_ty,
                        trait_id: TraitId::Builtin(BuiltinTrait::SliceConst),
                        trait_args: Vec::new(),
                    },
                    visited,
                ),
            TraitId::Builtin(_) => {}
            TraitId::Source(source_trait_id) => {
                let Some(trait_signature) = self.resolved_trait_signature(source_trait_id) else {
                    return;
                };
                let substitutions =
                    self.generic_substitutions(&trait_signature.generics, &obligation.trait_args);
                for supertrait in &trait_signature.supertraits {
                    let supertrait = self.substitute_generics(*supertrait, &substitutions);
                    let supertrait = self.normalization.normalize(supertrait);
                    let Some(TyKind::Nominal {
                        def_id: supertrait_id,
                        args: supertrait_args,
                    }) = self.interner.get(supertrait).cloned()
                    else {
                        continue;
                    };
                    self.push_trait_obligation_with_supertraits_inner(
                        obligations,
                        TraitObligation {
                            self_ty: obligation.self_ty,
                            trait_id: TraitId::Source(supertrait_id),
                            trait_args: supertrait_args,
                        },
                        visited,
                    );
                }
            }
        }
    }

    fn check_type_projection_obligations(
        &mut self,
        span: Span,
        ty: InternedTyId,
        obligations: &[TraitObligation],
    ) {
        let ty = self.normalization.normalize(ty);
        match self.interner.get(ty).cloned() {
            Some(TyKind::Pointer { elem, .. } | TyKind::Slice { elem, .. }) => {
                self.check_type_projection_obligations(span, elem, obligations);
            }
            Some(TyKind::Array { elem, .. }) => {
                self.check_type_projection_obligations(span, elem, obligations);
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                for param in params {
                    self.check_type_projection_obligations(span, param, obligations);
                }
                self.check_type_projection_obligations(span, return_type, obligations);
            }
            Some(TyKind::Nominal { args, .. }) => {
                for arg in args {
                    self.check_type_projection_obligations(span, arg, obligations);
                }
            }
            Some(TyKind::BuiltinTrait { args, .. }) => {
                for arg in args {
                    self.check_type_projection_obligations(span, arg, obligations);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                ..
            }) => {
                self.check_type_projection_obligations(span, self_ty, obligations);
                for arg in &trait_args {
                    self.check_type_projection_obligations(span, *arg, obligations);
                }
                let required = TraitObligation {
                    self_ty,
                    trait_id,
                    trait_args,
                };
                if !self.proves_trait_obligation(obligations, &required) {
                    self.diagnostics.push(nia_diagnostic::Diagnostic::error(
                        span,
                        format!(
                            "trait bound not satisfied: {}: {}",
                            self.ty_name(required.self_ty),
                            self.trait_ty_name(required.trait_id, &required.trait_args)
                        ),
                    ));
                }
            }
            Some(TyKind::Error | TyKind::Primitive(_) | TyKind::GenericParam(_)) | None => {}
        }
    }

    fn proves_trait_obligation(
        &mut self,
        obligations: &[TraitObligation],
        required: &TraitObligation,
    ) -> bool {
        obligations
            .iter()
            .any(|obligation| self.trait_obligations_equivalent(obligation, required))
            || self.trait_obligation_has_matching_impl(required)
    }

    fn trait_obligation_has_matching_impl(&mut self, required: &TraitObligation) -> bool {
        let impls = self.program_trait_impls.to_vec();
        for impl_signature in impls {
            if impl_signature.trait_id != required.trait_id {
                continue;
            }
            let target_ty =
                self.import_type_from(&impl_signature.interner, impl_signature.target_ty);
            let target_ty = self.normalization.normalize(target_ty);
            if !self.types_equivalent_without_projection_resolution(target_ty, required.self_ty) {
                continue;
            }
            let trait_args = impl_signature
                .trait_args
                .iter()
                .map(|arg| {
                    let arg = self.import_type_from(&impl_signature.interner, *arg);
                    self.normalization.normalize(arg)
                })
                .collect::<Vec<_>>();
            if trait_args.len() == required.trait_args.len()
                && trait_args
                    .iter()
                    .zip(&required.trait_args)
                    .all(|(actual, required)| {
                        self.types_equivalent_without_projection_resolution(*actual, *required)
                    })
            {
                return true;
            }
        }
        self.builtin_trait_obligation_has_matching_impl(required)
    }

    fn builtin_trait_obligation_has_matching_impl(&mut self, required: &TraitObligation) -> bool {
        let TraitId::Builtin(trait_id) = required.trait_id else {
            return false;
        };
        let self_ty = self.normalization.normalize(required.self_ty);
        match trait_id {
            BuiltinTrait::Add
            | BuiltinTrait::Sub
            | BuiltinTrait::Mul
            | BuiltinTrait::Div
            | BuiltinTrait::Rem => {
                let [rhs_ty] = required.trait_args.as_slice() else {
                    return false;
                };
                let rhs_ty = self.normalization.normalize(*rhs_ty);
                self.types_equivalent_without_projection_resolution(self_ty, rhs_ty)
                    && self.is_numeric(self_ty)
            }
            BuiltinTrait::BitAnd | BuiltinTrait::BitOr | BuiltinTrait::BitXor => {
                let [rhs_ty] = required.trait_args.as_slice() else {
                    return false;
                };
                let rhs_ty = self.normalization.normalize(*rhs_ty);
                self.types_equivalent_without_projection_resolution(self_ty, rhs_ty)
                    && self.is_integer(self_ty)
            }
            BuiltinTrait::Shl | BuiltinTrait::Shr => {
                let [rhs_ty] = required.trait_args.as_slice() else {
                    return false;
                };
                let rhs_ty = self.normalization.normalize(*rhs_ty);
                self.is_integer(self_ty) && self.is_integer(rhs_ty)
            }
            BuiltinTrait::Neg => required.trait_args.is_empty() && self.is_numeric(self_ty),
            BuiltinTrait::BitNot => required.trait_args.is_empty() && self.is_integer(self_ty),
            BuiltinTrait::Not => {
                required.trait_args.is_empty()
                    && self.types_equivalent_without_projection_resolution(self_ty, self.bool())
            }
            BuiltinTrait::Eq => {
                let [rhs_ty] = required.trait_args.as_slice() else {
                    return false;
                };
                let rhs_ty = self.normalization.normalize(*rhs_ty);
                self.types_equivalent_without_projection_resolution(self_ty, rhs_ty)
                    && (self.is_numeric(self_ty)
                        || self
                            .types_equivalent_without_projection_resolution(self_ty, self.bool())
                        || self.is_pointer(self_ty)
                        || self.is_enum(self_ty))
            }
            BuiltinTrait::Ord => {
                let [rhs_ty] = required.trait_args.as_slice() else {
                    return false;
                };
                let rhs_ty = self.normalization.normalize(*rhs_ty);
                self.types_equivalent_without_projection_resolution(self_ty, rhs_ty)
                    && self.is_numeric(self_ty)
            }
            BuiltinTrait::Sized => {
                required.trait_args.is_empty() && self.layout_of(self_ty).is_some()
            }
            BuiltinTrait::DerefConst => {
                required.trait_args.is_empty()
                    && (self.current_context_has_source_trait_obligation(required)
                        || self.builtin_deref_target_ty(self_ty).is_some())
            }
            BuiltinTrait::Deref => {
                required.trait_args.is_empty()
                    && (self.current_context_has_source_trait_obligation(required)
                        || self.builtin_mut_deref_target_ty(self_ty).is_some())
            }
            BuiltinTrait::IndexConst => {
                let [index_ty] = required.trait_args.as_slice() else {
                    return false;
                };
                self.current_context_has_source_trait_obligation(required)
                    || self.builtin_index_output_ty(self_ty, *index_ty).is_some()
            }
            BuiltinTrait::Index => {
                let [index_ty] = required.trait_args.as_slice() else {
                    return false;
                };
                self.current_context_has_source_trait_obligation(required)
                    || self
                        .builtin_mut_index_output_ty(self_ty, *index_ty)
                        .is_some()
            }
            BuiltinTrait::SliceConst => {
                required.trait_args.is_empty()
                    && (self.current_context_has_source_trait_obligation(required)
                        || self.builtin_slice_output_ty(self_ty, false).is_some())
            }
            BuiltinTrait::Slice => {
                required.trait_args.is_empty()
                    && (self.current_context_has_source_trait_obligation(required)
                        || self.builtin_mut_slice_output_ty(self_ty).is_some())
            }
        }
    }

    fn current_context_has_source_trait_obligation(&mut self, required: &TraitObligation) -> bool {
        self.current_def_id
            .and_then(|def_id| (def_id.module_id == self.defs.module_id).then_some(def_id.def_id))
            .and_then(|def_id| {
                let signature = self.signatures.functions.get(&def_id)?.clone();
                Some(self.function_signature_trait_obligations(def_id, &signature))
            })
            .is_some_and(|obligations| {
                obligations
                    .iter()
                    .any(|obligation| self.trait_obligations_equivalent(obligation, required))
            })
    }

    fn trait_obligations_equivalent(
        &self,
        left: &TraitObligation,
        right: &TraitObligation,
    ) -> bool {
        left.trait_id == right.trait_id
            && left.trait_args.len() == right.trait_args.len()
            && self.types_equivalent_without_projection_resolution(left.self_ty, right.self_ty)
            && left
                .trait_args
                .iter()
                .zip(&right.trait_args)
                .all(|(left, right)| {
                    self.types_equivalent_without_projection_resolution(*left, *right)
                })
    }

    pub(crate) fn types_equivalent_without_projection_resolution(
        &self,
        left: InternedTyId,
        right: InternedTyId,
    ) -> bool {
        let left = self.normalization.normalize(left);
        let right = self.normalization.normalize(right);
        if left == right {
            return true;
        }
        match (self.interner.get(left), self.interner.get(right)) {
            (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
            (
                Some(TyKind::Pointer {
                    is_const: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::Pointer {
                    is_const: right_const,
                    elem: right_elem,
                }),
            )
            | (
                Some(TyKind::Slice {
                    is_const: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::Slice {
                    is_const: right_const,
                    elem: right_elem,
                }),
            ) => {
                left_const == right_const
                    && self.types_equivalent_without_projection_resolution(*left_elem, *right_elem)
            }
            (
                Some(TyKind::Array {
                    len: left_len,
                    elem: left_elem,
                }),
                Some(TyKind::Array {
                    len: right_len,
                    elem: right_elem,
                }),
            ) => {
                left_len == right_len
                    && self.types_equivalent_without_projection_resolution(*left_elem, *right_elem)
            }
            (
                Some(TyKind::FunctionPointer {
                    params: left_params,
                    return_type: left_return,
                    is_variadic: left_variadic,
                }),
                Some(TyKind::FunctionPointer {
                    params: right_params,
                    return_type: right_return,
                    is_variadic: right_variadic,
                }),
            ) => {
                left_variadic == right_variadic
                    && left_params.len() == right_params.len()
                    && left_params.iter().zip(right_params).all(|(left, right)| {
                        self.types_equivalent_without_projection_resolution(*left, *right)
                    })
                    && self
                        .types_equivalent_without_projection_resolution(*left_return, *right_return)
            }
            (
                Some(TyKind::Nominal {
                    def_id: left_def,
                    args: left_args,
                }),
                Some(TyKind::Nominal {
                    def_id: right_def,
                    args: right_args,
                }),
            ) => {
                left_def == right_def
                    && left_args.len() == right_args.len()
                    && left_args.iter().zip(right_args).all(|(left, right)| {
                        self.types_equivalent_without_projection_resolution(*left, *right)
                    })
            }
            (
                Some(TyKind::BuiltinTrait {
                    trait_id: left_trait,
                    args: left_args,
                }),
                Some(TyKind::BuiltinTrait {
                    trait_id: right_trait,
                    args: right_args,
                }),
            ) => {
                left_trait == right_trait
                    && left_args.len() == right_args.len()
                    && left_args.iter().zip(right_args).all(|(left, right)| {
                        self.types_equivalent_without_projection_resolution(*left, *right)
                    })
            }
            (
                Some(TyKind::Projection {
                    self_ty: left_self,
                    trait_id: left_trait,
                    trait_args: left_args,
                    name: left_name,
                }),
                Some(TyKind::Projection {
                    self_ty: right_self,
                    trait_id: right_trait,
                    trait_args: right_args,
                    name: right_name,
                }),
            ) => {
                left_trait == right_trait
                    && left_name == right_name
                    && left_args.len() == right_args.len()
                    && self.types_equivalent_without_projection_resolution(*left_self, *right_self)
                    && left_args.iter().zip(right_args).all(|(left, right)| {
                        self.types_equivalent_without_projection_resolution(*left, *right)
                    })
            }
            (Some(TyKind::GenericParam(left)), Some(TyKind::GenericParam(right))) => left == right,
            _ => false,
        }
    }
}
