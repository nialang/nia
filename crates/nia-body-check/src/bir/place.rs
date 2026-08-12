// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::{Expr, ExprKind, IndexArg, SliceRange, UnaryOp};
use nia_body_ir::{
    BuiltinPlaceMethod, PlaceBase, PlaceElem, TypedCallee, TypedExpr, TypedExprKind, TypedPlace,
};
use nia_ids::{BuiltinTraitMethod, ReceiverKind, TraitId};
use nia_local_resolve::LocalUse;
use nia_sema_ir::BracketSuffixResolution;
use nia_span::Span;
use nia_symbol::known;
use nia_trait_solve::TraitResolution;
use nia_ty::{BuiltinTrait, TyKind};
use nia_value_resolve::ValueNameResolution;

impl<'a> BodyChecker<'a> {
    // A place is represented as one storage base followed by projections in
    // source order. Overloaded dereference and indexing replace that chain
    // with a dereferenced trait-call result at the point where they occur.
    pub(crate) fn lower_place(&mut self, expr: &Expr) -> TypedPlace {
        let ty = self.expr_ty(expr).unwrap_or_else(|| self.error());
        let mut elems = Vec::new();
        let base = self.lower_place_inner(expr, &mut elems, true);
        TypedPlace {
            span: expr.span,
            ty,
            base,
            elems,
        }
    }

    fn lower_place_inner(
        &mut self,
        expr: &Expr,
        elems: &mut Vec<PlaceElem>,
        mutable: bool,
    ) -> PlaceBase {
        if self.variant_enum(expr).is_some() {
            return PlaceBase::Error;
        }
        if let Some(def_id) = self.qualified_value(expr)
            && !matches!(self.global_def_kind(def_id), Some(nia_defs::DefKind::Const))
        {
            return PlaceBase::Global(def_id);
        }
        match &expr.kind {
            ExprKind::Ident(_) | ExprKind::SelfValue => match self.local_use(expr) {
                Some(LocalUse::Local(local)) => PlaceBase::Local(local),
                Some(LocalUse::Static(global_id)) => PlaceBase::Global(global_id),
                Some(LocalUse::ModuleValue) => match self.value_name(expr) {
                    Some(ValueNameResolution::Def(def_id))
                        if !matches!(
                            self.defs.defs.get(def_id).map(|def| def.kind),
                            Some(nia_defs::DefKind::Const)
                        ) =>
                    {
                        PlaceBase::Global(self.global_def_id(def_id))
                    }
                    _ => PlaceBase::Error,
                },
                _ => PlaceBase::Error,
            },
            ExprKind::Unary {
                op: nia_ast::UnaryOp::Deref,
                expr,
            } => {
                let ty = self.expr_runtime_ty(expr);
                if let Some(pointer) = self.lower_builtin_deref_method_call(expr, ty, mutable) {
                    PlaceBase::Deref(Box::new(pointer))
                } else {
                    PlaceBase::Deref(Box::new(self.lower_expr_with_ty(expr, Some(ty))))
                }
            }
            ExprKind::Field { lhs, name } | ExprKind::Qualified { lhs, name } => {
                let base = self.lower_place_inner(lhs, elems, mutable);
                let lhs_ty = self.expr_ty(lhs).unwrap_or_else(|| self.error());
                let field = self
                    .field_def_for_base_ty(lhs_ty, name)
                    .map(PlaceElem::Field)
                    .unwrap_or(PlaceElem::Error);
                elems.push(field);
                base
            }
            ExprKind::TupleField { lhs, index } => {
                let base = self.lower_place_inner(lhs, elems, mutable);
                elems.push(PlaceElem::TupleField(*index));
                base
            }
            ExprKind::Index { lhs, index } => {
                if let IndexArg::Expr(index) = index {
                    let lhs_ty = self.expr_runtime_ty(lhs);
                    let index_ty = self.expr_ty(index).unwrap_or_else(|| self.error());
                    if self.indirect_index_base(lhs).is_some() {
                        return self.lower_indirect_index_place_base(
                            expr, lhs, index, lhs_ty, index_ty, mutable,
                        );
                    }
                    if let Some(pointer) =
                        self.lower_builtin_index_method_call(lhs, index, lhs_ty, index_ty, mutable)
                    {
                        return PlaceBase::Deref(Box::new(pointer));
                    }
                    let base = self.lower_place_inner(lhs, elems, mutable);
                    elems.push(PlaceElem::Index(Box::new(self.lower_expr(index))));
                    return base;
                }
                PlaceBase::Error
            }
            ExprKind::BracketSuffix { callee, args } => {
                if matches!(
                    self.bracket_suffix_resolution(expr),
                    Some(BracketSuffixResolution::Index)
                ) {
                    if let Some(index) = args.first().and_then(|arg| arg.expr.as_ref()) {
                        let lhs_ty = self.expr_ty(callee).unwrap_or_else(|| self.error());
                        let index_ty = self.expr_ty(index).unwrap_or_else(|| self.error());
                        if self.indirect_index_base(callee).is_some() {
                            return self.lower_indirect_index_place_base(
                                expr, callee, index, lhs_ty, index_ty, mutable,
                            );
                        }
                        if let Some(pointer) = self.lower_builtin_index_method_call(
                            callee, index, lhs_ty, index_ty, mutable,
                        ) {
                            return PlaceBase::Deref(Box::new(pointer));
                        }
                        let base = self.lower_place_inner(callee, elems, mutable);
                        elems.push(PlaceElem::Index(Box::new(self.lower_expr(index))));
                        return base;
                    }
                    PlaceBase::Error
                } else {
                    PlaceBase::Error
                }
            }
            _ => PlaceBase::Error,
        }
    }

    pub(super) fn lower_builtin_deref_method_call(
        &mut self,
        receiver: &Expr,
        receiver_ty: nia_ids::InternedTyId,
        mutable: bool,
    ) -> Option<TypedExpr> {
        // Place mutability selects both the trait contract and the pointer
        // constness returned by the synthetic call. A write context must never
        // be lowered through the readonly trait and then cast back to mutable.
        let (trait_id, method, target_const) = if mutable {
            (BuiltinTrait::DerefMut, BuiltinTraitMethod::DerefMut, false)
        } else {
            (BuiltinTrait::Deref, BuiltinTraitMethod::Deref, true)
        };
        let resolution = self.current_context_resolve_trait_obligation(
            receiver_ty,
            TraitId::Builtin(trait_id),
            Vec::new(),
        );
        if !matches!(
            resolution,
            TraitResolution::User(_) | TraitResolution::Assumed(_)
        ) {
            return None;
        }
        let target = self.interner.intern(TyKind::Projection {
            self_ty: receiver_ty,
            trait_id: TraitId::Builtin(trait_id),
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            name: known::TARGET,
        });
        let target = self.normalize_projection(target);
        let pointer_ty = self.interner.intern(TyKind::Pointer {
            is_readonly: target_const,
            elem: target,
        });
        Some(TypedExpr {
            span: receiver.span,
            ty: pointer_ty,
            kind: TypedExprKind::Call {
                callee: TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                    trait_id,
                    method,
                    self_ty: receiver_ty,
                    trait_args: Vec::new(),
                    receiver: Box::new(self.lower_builtin_place_method_receiver(
                        receiver,
                        receiver_ty,
                        method,
                    )),
                }),
                args: Vec::new(),
            },
        })
    }

    pub(super) fn lower_non_intrinsic_builtin_index_method_call(
        &mut self,
        receiver: &Expr,
        index: &Expr,
        receiver_ty: nia_ids::InternedTyId,
        index_ty: nia_ids::InternedTyId,
    ) -> Option<TypedExpr> {
        let trait_args = vec![index_ty];
        let resolution = self.current_context_resolve_trait_obligation(
            receiver_ty,
            TraitId::Builtin(BuiltinTrait::Index),
            trait_args.clone(),
        );
        if !matches!(
            resolution,
            TraitResolution::User(_) | TraitResolution::Assumed(_)
        ) {
            return None;
        }
        let output = self.interner.intern(TyKind::Projection {
            self_ty: receiver_ty,
            trait_id: TraitId::Builtin(BuiltinTrait::Index),
            trait_args: trait_args.clone(),
            trait_const_args: Vec::new(),
            name: known::OUTPUT,
        });
        let output = self.normalize_projection(output);
        let pointer_ty = self.interner.intern(TyKind::Pointer {
            is_readonly: true,
            elem: output,
        });
        Some(TypedExpr {
            span: receiver.span,
            ty: pointer_ty,
            kind: TypedExprKind::Call {
                callee: TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                    trait_id: BuiltinTrait::Index,
                    method: BuiltinTraitMethod::Index,
                    self_ty: receiver_ty,
                    trait_args,
                    receiver: Box::new(self.lower_builtin_place_method_receiver(
                        receiver,
                        receiver_ty,
                        BuiltinTraitMethod::Index,
                    )),
                }),
                args: vec![self.lower_expr_with_ty(index, Some(index_ty))],
            },
        })
    }

    fn lower_builtin_index_method_call(
        &mut self,
        receiver: &Expr,
        index: &Expr,
        receiver_ty: nia_ids::InternedTyId,
        index_ty: nia_ids::InternedTyId,
        mutable: bool,
    ) -> Option<TypedExpr> {
        let (trait_id, method, output_const) = if mutable {
            (BuiltinTrait::IndexMut, BuiltinTraitMethod::IndexMut, false)
        } else {
            (BuiltinTrait::Index, BuiltinTraitMethod::Index, true)
        };
        let trait_args = vec![index_ty];
        let resolution = self.current_context_resolve_trait_obligation(
            receiver_ty,
            TraitId::Builtin(trait_id),
            trait_args.clone(),
        );
        if !matches!(
            resolution,
            TraitResolution::User(_) | TraitResolution::Assumed(_)
        ) {
            return None;
        }
        let output = self.interner.intern(TyKind::Projection {
            self_ty: receiver_ty,
            trait_id: TraitId::Builtin(trait_id),
            trait_args: trait_args.clone(),
            trait_const_args: Vec::new(),
            name: known::OUTPUT,
        });
        let output = self.normalize_projection(output);
        let pointer_ty = self.interner.intern(TyKind::Pointer {
            is_readonly: output_const,
            elem: output,
        });
        Some(TypedExpr {
            span: receiver.span,
            ty: pointer_ty,
            kind: TypedExprKind::Call {
                callee: TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                    trait_id,
                    method,
                    self_ty: receiver_ty,
                    trait_args,
                    receiver: Box::new(self.lower_builtin_place_method_receiver(
                        receiver,
                        receiver_ty,
                        method,
                    )),
                }),
                args: vec![self.lower_expr(index)],
            },
        })
    }

    fn lower_indirect_index_place_base(
        &mut self,
        expr: &Expr,
        receiver: &Expr,
        index: &Expr,
        receiver_ty: nia_ids::InternedTyId,
        index_ty: nia_ids::InternedTyId,
        mutable: bool,
    ) -> PlaceBase {
        let output_ty = self.expr_ty(expr).unwrap_or_else(|| self.error());
        let indexed = TypedExpr {
            span: expr.span,
            ty: output_ty,
            kind: TypedExprKind::Index {
                lhs: Box::new(self.lower_expr_with_ty(receiver, Some(receiver_ty))),
                index: Box::new(self.lower_expr_with_ty(index, Some(index_ty))),
            },
        };
        let pointer_ty = self.interner.intern(TyKind::Pointer {
            is_readonly: !mutable,
            elem: output_ty,
        });
        PlaceBase::Deref(Box::new(TypedExpr {
            span: expr.span,
            ty: pointer_ty,
            kind: TypedExprKind::Unary {
                op: if mutable {
                    UnaryOp::Ref
                } else {
                    UnaryOp::RefReadOnly
                },
                expr: Box::new(indexed),
            },
        }))
    }

    pub(super) fn lower_slice_expr_readonly(
        &mut self,
        lhs: &Expr,
        range: &SliceRange,
        expr_span: Span,
    ) -> TypedExprKind {
        self.lower_slice_expr(lhs, range, true, expr_span)
    }

    pub(super) fn lower_slice_expr(
        &mut self,
        lhs: &Expr,
        range: &SliceRange,
        is_readonly: bool,
        expr_span: Span,
    ) -> TypedExprKind {
        let lhs_ty = self.expr_ty(lhs).unwrap_or_else(|| self.error());
        let range_ty = self.check_slice_range_bounds(range);
        let (trait_id, method) = if is_readonly {
            (BuiltinTrait::Slice, BuiltinTraitMethod::Slice)
        } else {
            (BuiltinTrait::SliceMut, BuiltinTraitMethod::SliceMut)
        };
        let resolution = self.current_context_resolve_trait_obligation(
            lhs_ty,
            TraitId::Builtin(trait_id),
            vec![range_ty],
        );
        if !matches!(
            resolution,
            TraitResolution::User(_) | TraitResolution::Assumed(_)
        ) {
            return TypedExprKind::Slice {
                lhs: Box::new(self.lower_expr(lhs)),
                range: self.lower_slice_range(range),
                is_readonly,
            };
        }
        TypedExprKind::Call {
            callee: TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                trait_id,
                method,
                self_ty: lhs_ty,
                trait_args: vec![range_ty],
                receiver: Box::new(self.lower_builtin_place_method_receiver(lhs, lhs_ty, method)),
            }),
            args: vec![self.lower_range_as_expr(range, range_ty, expr_span)],
        }
    }

    fn lower_range_as_expr(
        &mut self,
        range: &SliceRange,
        ty: nia_ids::InternedTyId,
        fallback_span: Span,
    ) -> TypedExpr {
        TypedExpr {
            span: self.slice_range_span(range, fallback_span),
            ty,
            kind: TypedExprKind::Range(self.lower_range(range)),
        }
    }

    fn slice_range_span(&self, range: &SliceRange, fallback_span: Span) -> Span {
        match (&range.start, &range.end) {
            (Some(start), Some(end)) => Span::new(start.span.start, end.span.end),
            (Some(start), None) => start.span,
            (None, Some(end)) => end.span,
            // `SliceRange` has no delimiter span of its own. Preserve the
            // enclosing index span for `[..]` instead of manufacturing an
            // unrelated default source position.
            (None, None) => fallback_span,
        }
    }

    fn lower_builtin_place_method_receiver(
        &mut self,
        receiver: &Expr,
        receiver_ty: nia_ids::InternedTyId,
        method: BuiltinTraitMethod,
    ) -> TypedExpr {
        // Place methods describe the receiver they need independently from
        // their ordinary callable signature, so honor that policy before
        // constructing the explicit reference in body IR.
        let receiver_kind = method
            .place_receiver_kind()
            .unwrap_or_else(|| method.receiver_kind());
        match receiver_kind {
            ReceiverKind::RefReadOnly => {
                let pointer_ty = self.interner.intern(TyKind::Pointer {
                    is_readonly: true,
                    elem: receiver_ty,
                });
                TypedExpr {
                    span: receiver.span,
                    ty: pointer_ty,
                    kind: TypedExprKind::Unary {
                        op: UnaryOp::RefReadOnly,
                        expr: Box::new(self.lower_expr_with_ty(receiver, Some(receiver_ty))),
                    },
                }
            }
            ReceiverKind::Ref => {
                let pointer_ty = self.interner.intern(TyKind::Pointer {
                    is_readonly: false,
                    elem: receiver_ty,
                });
                TypedExpr {
                    span: receiver.span,
                    ty: pointer_ty,
                    kind: TypedExprKind::Unary {
                        op: UnaryOp::Ref,
                        expr: Box::new(self.lower_expr_with_ty(receiver, Some(receiver_ty))),
                    },
                }
            }
            ReceiverKind::Value => self.lower_expr_with_ty(receiver, Some(receiver_ty)),
        }
    }

    pub(super) fn lower_typed_builtin_place_method_receiver(
        &mut self,
        receiver: &TypedExpr,
        receiver_ty: nia_ids::InternedTyId,
        method: BuiltinTraitMethod,
    ) -> TypedExpr {
        let receiver_kind = method
            .place_receiver_kind()
            .unwrap_or_else(|| method.receiver_kind());
        match receiver_kind {
            ReceiverKind::RefReadOnly => {
                let pointer_ty = self.interner.intern(TyKind::Pointer {
                    is_readonly: true,
                    elem: receiver_ty,
                });
                TypedExpr {
                    span: receiver.span,
                    ty: pointer_ty,
                    kind: TypedExprKind::Unary {
                        op: UnaryOp::RefReadOnly,
                        expr: Box::new(receiver.clone()),
                    },
                }
            }
            ReceiverKind::Ref => {
                let pointer_ty = self.interner.intern(TyKind::Pointer {
                    is_readonly: false,
                    elem: receiver_ty,
                });
                TypedExpr {
                    span: receiver.span,
                    ty: pointer_ty,
                    kind: TypedExprKind::Unary {
                        op: UnaryOp::Ref,
                        expr: Box::new(receiver.clone()),
                    },
                }
            }
            ReceiverKind::Value => receiver.clone(),
        }
    }
}
