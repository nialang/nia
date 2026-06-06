// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{BuiltinTraitGoalKey, ExtensionTraitMethodCandidate, ExtensionTraitMethodKey};

impl<'a> ModuleLowerer<'a> {
    pub(super) fn current_impl_trait_method(
        &mut self,
        key: &ExtensionTraitMethodKey,
        trait_args: &[InternedTyId],
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let current = self.current_instantiated_function?;
        if current.module_id != self.input.module_id {
            return None;
        }
        let current_impl_index = *self.trait_impls_by_method.get(&current)?;
        let impl_signature = self.input.trait_impls.get(current_impl_index)?;
        if impl_signature.trait_id != key.trait_id {
            return None;
        }
        let impl_interner = impl_signature.interner.clone();
        let target_ty =
            nia_ty::import_type_into(&mut self.interner, &impl_interner, impl_signature.target_ty);
        let mut substitutions = std::collections::HashMap::new();
        if !self.match_extension_type_pattern(target_ty, self_ty, &mut substitutions) {
            return None;
        }
        if impl_signature.trait_args.len() != trait_args.len() {
            return None;
        }
        let impl_trait_args = impl_signature
            .trait_args
            .iter()
            .map(|arg| nia_ty::import_type_into(&mut self.interner, &impl_interner, *arg))
            .collect::<Vec<_>>();
        if !impl_trait_args.iter().zip(trait_args).all(|(left, right)| {
            self.match_extension_type_pattern(*left, *right, &mut substitutions)
        }) {
            return None;
        }
        self.input
            .extensions
            .targets()
            .iter()
            .flat_map(|target| target.methods.iter())
            .find(|method| {
                method.is_trait_witness
                    && self
                        .trait_impls_by_method
                        .get(&method.def_id)
                        .is_some_and(|impl_index| *impl_index == current_impl_index)
                    && method.trait_id == Some(key.trait_id)
                    && method.name == key.method_name
                    && method.trait_args.len() == key.trait_arg_count
            })
            .map(|method| {
                let args = method
                    .impl_generics
                    .iter()
                    .filter_map(|generic| substitutions.get(generic).copied())
                    .collect();
                (method.def_id, args)
            })
    }

    pub(super) fn trait_impl_method_for_candidate(
        &mut self,
        candidate: &ExtensionTraitMethodCandidate,
        trait_args: &[InternedTyId],
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let substitutions =
            self.match_extension_trait_impl_candidate(candidate, trait_args, self_ty)?;
        let args = self
            .candidate_impl_generics(candidate)
            .iter()
            .filter_map(|generic| substitutions.get(generic).copied())
            .collect::<Vec<_>>();
        Some((candidate.method_def_id, args))
    }

    pub(super) fn resolve_builtin_place_method_impl(
        &mut self,
        trait_id: BuiltinTrait,
        trait_args: &[InternedTyId],
        method: BuiltinTraitMethod,
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let key = ExtensionTraitMethodKey {
            trait_id: TraitId::Builtin(trait_id),
            method_name: method.name().to_string(),
            trait_arg_count: trait_args.len(),
        };
        let candidates = self.extension_trait_method_candidates(&key);
        let candidates = candidates
            .iter()
            .filter_map(|candidate| {
                self.builtin_place_impl_method_for_candidate(candidate, trait_args, self_ty)
            })
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [candidate] => Some(candidate.clone()),
            _ => None,
        }
    }

    pub(super) fn builtin_place_impl_method_for_candidate(
        &mut self,
        candidate: &ExtensionTraitMethodCandidate,
        trait_args: &[InternedTyId],
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let substitutions =
            self.match_extension_trait_impl_candidate(candidate, trait_args, self_ty)?;
        let args = self
            .candidate_impl_generics(candidate)
            .iter()
            .filter_map(|generic| substitutions.get(generic).copied())
            .collect::<Vec<_>>();
        Some((candidate.method_def_id, args))
    }

    pub(super) fn lower_intrinsic_builtin_place_method_call(
        &mut self,
        trait_id: BuiltinTrait,
        method: BuiltinTraitMethod,
        self_ty: InternedTyId,
        trait_args: &[InternedTyId],
        receiver: FunctionExpr,
        args: &[FunctionExpr],
    ) -> Option<FunctionExpr> {
        let receiver_span = receiver.span;
        match (trait_id, method, trait_args, args) {
            (
                BuiltinTrait::DerefRead | BuiltinTrait::Deref,
                BuiltinTraitMethod::DerefRead | BuiltinTraitMethod::Deref,
                [],
                [],
            ) => {
                if !matches!(
                    self.resolve_builtin_trait_goal(self_ty, trait_id, Vec::new()),
                    TraitResolution::Intrinsic(_)
                ) {
                    return None;
                }
                let elem = self.pointer_elem_ty(self_ty)?;
                let receiver_ptr = FunctionExpr {
                    span: receiver_span,
                    ty: self_ty,
                    kind: FunctionExprKind::Unary {
                        op: nia_ast::UnaryOp::Deref,
                        expr: Box::new(receiver),
                    },
                };
                Some(FunctionExpr {
                    span: receiver_ptr.span,
                    ty: self.interner.intern(TyKind::Pointer {
                        is_readonly: matches!(trait_id, BuiltinTrait::DerefRead),
                        elem,
                    }),
                    kind: FunctionExprKind::AddrOf(FunctionPlace {
                        span: receiver_ptr.span,
                        ty: elem,
                        base: FunctionPlaceBase::Deref(Box::new(receiver_ptr)),
                        elems: Vec::new(),
                    }),
                })
            }
            (
                BuiltinTrait::IndexRead | BuiltinTrait::Index,
                BuiltinTraitMethod::IndexRead | BuiltinTraitMethod::Index,
                [index_ty],
                [index],
            ) => {
                if !matches!(
                    self.resolve_builtin_trait_goal(self_ty, trait_id, vec![*index_ty]),
                    TraitResolution::Intrinsic(_)
                ) {
                    return None;
                }
                let elem = self.index_elem_ty(self_ty)?;
                let base = FunctionExpr {
                    span: receiver_span,
                    ty: self_ty,
                    kind: FunctionExprKind::Unary {
                        op: nia_ast::UnaryOp::Deref,
                        expr: Box::new(receiver),
                    },
                };
                Some(FunctionExpr {
                    span: index.span,
                    ty: self.interner.intern(TyKind::Pointer {
                        is_readonly: matches!(trait_id, BuiltinTrait::IndexRead),
                        elem,
                    }),
                    kind: FunctionExprKind::AddrOf(FunctionPlace {
                        span: index.span,
                        ty: elem,
                        base: FunctionPlaceBase::Deref(Box::new(base)),
                        elems: vec![FunctionPlaceElem::Index(Box::new(index.clone()))],
                    }),
                })
            }
            (
                BuiltinTrait::SliceRead | BuiltinTrait::Slice,
                BuiltinTraitMethod::SliceRead | BuiltinTraitMethod::Slice,
                [_range_ty],
                [range],
            ) => {
                if !matches!(
                    self.resolve_builtin_trait_goal(self_ty, trait_id, trait_args.to_vec()),
                    TraitResolution::Intrinsic(_)
                ) {
                    return None;
                }
                let base = FunctionExpr {
                    span: receiver_span,
                    ty: self_ty,
                    kind: FunctionExprKind::Unary {
                        op: nia_ast::UnaryOp::Deref,
                        expr: Box::new(receiver),
                    },
                };
                let substitutions = self.empty_type_substitution_id();
                let mut active_projections = std::collections::HashSet::new();
                Some(FunctionExpr {
                    span: range.span,
                    ty: self.resolve_associated_type_projection(
                        self_ty,
                        TraitId::Builtin(trait_id),
                        trait_args,
                        BuiltinTrait::OUTPUT_ASSOC_TYPE,
                        substitutions,
                        &mut active_projections,
                    )?,
                    kind: FunctionExprKind::Slice {
                        lhs: Box::new(base),
                        range: self.range_expr_to_slice_range(range)?,
                        is_readonly: matches!(trait_id, BuiltinTrait::SliceRead),
                    },
                })
            }
            _ => None,
        }
    }

    pub(super) fn resolve_builtin_trait_goal(
        &mut self,
        self_ty: InternedTyId,
        trait_id: BuiltinTrait,
        trait_args: Vec<InternedTyId>,
    ) -> TraitResolution {
        let key = BuiltinTraitGoalKey {
            self_ty,
            trait_id,
            trait_args,
        };
        if let Some(resolution) = self.builtin_trait_resolutions.get(&key) {
            return resolution.clone();
        }
        let context = TraitSolverContext {
            normalization: self.input.type_normalization,
            trait_impls: self.input.trait_impls,
            layouts: Some(self.input.layouts),
            local_module_id: self.input.module_id,
            local_enums: &self.input.signatures.enums,
            program_enums: Some(self.input.program_enums),
        };
        let mut solver = context.solver(&mut self.interner, &[]);
        let resolution = solver.resolve(TraitGoal {
            self_ty: key.self_ty,
            trait_id: TraitId::Builtin(key.trait_id),
            trait_args: key.trait_args.clone(),
        });
        self.builtin_trait_resolutions
            .insert(key, resolution.clone());
        resolution
    }

    pub(super) fn extension_trait_method_candidates(
        &self,
        key: &ExtensionTraitMethodKey,
    ) -> Vec<ExtensionTraitMethodCandidate> {
        let mut out = self
            .extension_trait_method_candidates
            .get(key)
            .cloned()
            .unwrap_or_default();
        if let Some((_, instance_candidates)) = &self.instance_extension_trait_method_candidates {
            for candidate in instance_candidates.get(key).cloned().unwrap_or_default() {
                if !out
                    .iter()
                    .any(|existing| existing.method_def_id == candidate.method_def_id)
                {
                    out.push(candidate);
                }
            }
        }
        out
    }

    pub(super) fn program_extension_trait_method_candidates(
        &self,
        key: &ExtensionTraitMethodKey,
    ) -> Vec<ExtensionTraitMethodCandidate> {
        let candidates = self
            .program_extension_trait_method_candidates
            .get(key)
            .cloned()
            .unwrap_or_default();
        if candidates.is_empty() {
            return self.extension_trait_method_candidates(key);
        }
        candidates
    }

    pub(super) fn pointer_elem_ty(&self, ty: InternedTyId) -> Option<InternedTyId> {
        match self.ty_kind(ty) {
            Some(TyKind::Pointer { elem, .. }) => Some(*elem),
            _ => None,
        }
    }

    pub(super) fn index_elem_ty(&self, ty: InternedTyId) -> Option<InternedTyId> {
        match self.ty_kind(ty) {
            Some(TyKind::Array { elem, .. })
            | Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::Slice { elem, .. }) => Some(*elem),
            _ => None,
        }
    }

    pub(super) fn range_expr_to_slice_range(
        &self,
        range: &FunctionExpr,
    ) -> Option<FunctionSliceRange> {
        match &range.kind {
            FunctionExprKind::Range(range) => Some(FunctionSliceRange {
                start: range.start.clone(),
                end: range.end.clone(),
                inclusive: range.inclusive,
            }),
            _ => None,
        }
    }
}
