// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::BuiltinTraitGoalKey;

impl<'a> ModuleLowerer<'a> {
    pub(super) fn trait_impl_method_for_target(
        &mut self,
        target: &VisibleExtensionTarget,
        trait_id: GlobalDefId,
        trait_args: &[InternedTyId],
        method_name: &str,
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        if !self.extension_type_pattern_matches(target.target_ty, self_ty) {
            return None;
        }
        let method = target.methods.iter().find(|method| {
            let method_trait_args = method
                .trait_args
                .iter()
                .map(|arg| self.import_extension_type(*arg))
                .collect::<Vec<_>>();
            method.name == method_name
                && method.trait_id == Some(TraitId::Source(trait_id))
                && method_trait_args.len() == trait_args.len()
                && method_trait_args
                    .iter()
                    .zip(trait_args)
                    .all(|(actual, expected)| self.types_match(*actual, *expected))
        })?;
        let mut substitutions = HashMap::new();
        self.match_extension_type_pattern(target.target_ty, self_ty, &mut substitutions)
            .then(|| {
                let args = self
                    .generic_params_in_extension_ty(target.target_ty)
                    .iter()
                    .filter_map(|generic| substitutions.get(generic).copied())
                    .collect::<Vec<_>>();
                (method.def_id, args)
            })
    }

    pub(super) fn resolve_builtin_place_method_impl(
        &mut self,
        trait_id: BuiltinTrait,
        trait_args: &[InternedTyId],
        method: BuiltinTraitMethod,
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let candidates = self
            .input
            .extensions
            .targets()
            .iter()
            .filter_map(|target| {
                self.builtin_place_impl_method_for_target(
                    target,
                    trait_id,
                    trait_args,
                    method.name(),
                    self_ty,
                )
            })
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [candidate] => Some(candidate.clone()),
            _ => None,
        }
    }

    pub(super) fn builtin_place_impl_method_for_target(
        &mut self,
        target: &VisibleExtensionTarget,
        trait_id: BuiltinTrait,
        trait_args: &[InternedTyId],
        method_name: &str,
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        if !self.extension_type_pattern_matches(target.target_ty, self_ty) {
            return None;
        }
        let method = target.methods.iter().find(|method| {
            let method_trait_args = method
                .trait_args
                .iter()
                .map(|arg| self.import_extension_type(*arg))
                .collect::<Vec<_>>();
            method.name == method_name
                && method.trait_id == Some(TraitId::Builtin(trait_id))
                && method_trait_args.len() == trait_args.len()
                && method_trait_args
                    .iter()
                    .zip(trait_args)
                    .all(|(actual, expected)| self.types_match(*actual, *expected))
        })?;
        let mut substitutions = HashMap::new();
        self.match_extension_type_pattern(target.target_ty, self_ty, &mut substitutions)
            .then(|| {
                let args = self
                    .generic_params_in_extension_ty(target.target_ty)
                    .iter()
                    .filter_map(|generic| substitutions.get(generic).copied())
                    .collect::<Vec<_>>();
                (method.def_id, args)
            })
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
                BuiltinTrait::DerefConst | BuiltinTrait::Deref,
                BuiltinTraitMethod::DerefConst | BuiltinTraitMethod::Deref,
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
                        is_const: matches!(trait_id, BuiltinTrait::DerefConst),
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
                BuiltinTrait::IndexConst | BuiltinTrait::Index,
                BuiltinTraitMethod::IndexConst | BuiltinTraitMethod::Index,
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
                        is_const: matches!(trait_id, BuiltinTrait::IndexConst),
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
                BuiltinTrait::SliceConst | BuiltinTrait::Slice,
                BuiltinTraitMethod::SliceConst | BuiltinTraitMethod::Slice,
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
                Some(FunctionExpr {
                    span: range.span,
                    ty: self.resolve_associated_type_projection(
                        self_ty,
                        TraitId::Builtin(trait_id),
                        trait_args,
                        BuiltinTrait::OUTPUT_ASSOC_TYPE,
                    )?,
                    kind: FunctionExprKind::Slice {
                        lhs: Box::new(base),
                        range: self.range_expr_to_slice_range(range)?,
                        is_const: matches!(trait_id, BuiltinTrait::SliceConst),
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
