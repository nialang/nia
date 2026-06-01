// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

struct BuiltinAssociatedPlaceMethodCall<'a> {
    span: Span,
    target_ty: InternedTyId,
    name: &'a str,
    method: BuiltinTraitMethod,
    args: &'a [Expr],
    expected: Option<InternedTyId>,
}

impl<'a> BodyChecker<'a> {
    pub(in crate::calls::methods) fn check_builtin_trait_method_call_with_receiver_ty(
        &mut self,
        call: &MethodCall<'_>,
    ) -> Option<InternedTyId> {
        let method = BuiltinTraitMethod::from_name(call.name)?;
        if call.type_args.is_some() {
            self.diagnostics.push(Diagnostic::error(
                call.span,
                "builtin trait methods do not take method generic arguments",
            ));
            for arg in call.args {
                self.check_expr(arg);
            }
            return Some(self.error());
        }
        if method.is_place_method() {
            return self.check_builtin_trait_place_method_call_with_receiver_ty(
                call,
                method.trait_id(),
                method,
            );
        }
        let op = BuiltinOperatorOp::from_method(method)?;
        let trait_id = method.trait_id();
        let Some(trait_args) = self.check_builtin_trait_method_value_args(
            call.span,
            trait_id,
            method,
            call.receiver_ty,
            call.args,
        ) else {
            return Some(self.error());
        };
        if !self.current_context_proves_trait_obligation(
            call.receiver_ty,
            TraitId::Builtin(trait_id),
            trait_args.clone(),
        ) {
            return None;
        }
        let output = self.builtin_trait_method_output(call.receiver_ty, trait_id, trait_args);
        if let Some(expected) = call.expected {
            self.expect_type(call.span, expected, output, "builtin trait method call");
        }
        self.record_resolved_call(call.span, ResolvedCall::BuiltinTraitMethod { trait_id, op });
        Some(output)
    }

    pub(in crate::calls::methods) fn check_builtin_trait_associated_method_call(
        &mut self,
        span: Span,
        target_ty: InternedTyId,
        name: &str,
        method_type_args: Option<&[BracketArg]>,
        args: &[Expr],
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let method = BuiltinTraitMethod::from_name(name)?;
        if method_type_args.is_some() {
            self.diagnostics.push(Diagnostic::error(
                span,
                "builtin trait methods do not take method generic arguments",
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return Some(self.error());
        }
        if method.is_place_method() {
            return self.check_builtin_trait_associated_place_method_call(
                BuiltinAssociatedPlaceMethodCall {
                    span,
                    target_ty,
                    name,
                    method,
                    args,
                    expected,
                },
            );
        }
        let op = BuiltinOperatorOp::from_method(method)?;
        let trait_id = method.trait_id();
        let Some((receiver, value_args)) = args.split_first() else {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("receiver method `{name}` requires a receiver argument"),
            ));
            return Some(self.error());
        };
        let receiver_ty = self.check_expr_with_expected(receiver, Some(target_ty));
        self.expect_expr_type(receiver, target_ty, receiver_ty, "receiver argument");
        let Some(trait_args) = self
            .check_builtin_trait_method_value_args(span, trait_id, method, target_ty, value_args)
        else {
            return Some(self.error());
        };
        if !self.current_context_proves_trait_obligation(
            target_ty,
            TraitId::Builtin(trait_id),
            trait_args.clone(),
        ) {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "trait bound not satisfied: {}: {}",
                    self.ty_name(target_ty),
                    self.builtin_trait_ty_name(trait_id, &trait_args)
                ),
            ));
        }
        let output = self.builtin_trait_method_output(target_ty, trait_id, trait_args);
        if let Some(expected) = expected {
            self.expect_type(span, expected, output, "builtin trait method call");
        }
        self.record_resolved_call(span, ResolvedCall::BuiltinTraitMethod { trait_id, op });
        Some(output)
    }

    fn check_builtin_trait_place_method_call_with_receiver_ty(
        &mut self,
        call: &MethodCall<'_>,
        trait_id: BuiltinTrait,
        method: BuiltinTraitMethod,
    ) -> Option<InternedTyId> {
        let Some(trait_args) = self
            .check_builtin_trait_place_method_value_args(call.span, trait_id, method, call.args)
        else {
            return Some(self.error());
        };
        if matches!(method.receiver_kind(), BuiltinReceiverKind::Ref) {
            self.check_receiver_match(call.receiver, call.receiver_ty, ReceiverKind::Ref);
        }
        if !self.current_context_proves_trait_obligation(
            call.receiver_ty,
            TraitId::Builtin(trait_id),
            trait_args.clone(),
        ) {
            return None;
        }
        let output =
            self.builtin_trait_place_method_output(call.receiver_ty, trait_id, trait_args.clone());
        if let Some(expected) = call.expected {
            self.expect_type(call.span, expected, output, "builtin trait method call");
        }
        self.record_resolved_call(
            call.span,
            ResolvedCall::BuiltinPlaceMethod {
                trait_id,
                method,
                self_ty: call.receiver_ty,
                trait_args,
            },
        );
        Some(output)
    }

    fn check_builtin_trait_associated_place_method_call(
        &mut self,
        call: BuiltinAssociatedPlaceMethodCall<'_>,
    ) -> Option<InternedTyId> {
        let trait_id = call.method.trait_id();
        let Some((receiver, value_args)) = call.args.split_first() else {
            self.diagnostics.push(Diagnostic::error(
                call.span,
                format!(
                    "receiver method `{}` requires a receiver argument",
                    call.name
                ),
            ));
            return Some(self.error());
        };
        let Some(trait_args) = self.check_builtin_trait_place_method_value_args(
            call.span,
            trait_id,
            call.method,
            value_args,
        ) else {
            return Some(self.error());
        };
        let receiver_ty = self.check_expr_with_expected(receiver, Some(call.target_ty));
        self.expect_expr_type(receiver, call.target_ty, receiver_ty, "receiver argument");
        if matches!(call.method.receiver_kind(), BuiltinReceiverKind::Ref) {
            self.check_receiver_match(receiver, call.target_ty, ReceiverKind::Ref);
        }
        if !self.current_context_proves_trait_obligation(
            call.target_ty,
            TraitId::Builtin(trait_id),
            trait_args.clone(),
        ) {
            self.diagnostics.push(Diagnostic::error(
                call.span,
                format!(
                    "trait bound not satisfied: {}: {}",
                    self.ty_name(call.target_ty),
                    self.builtin_trait_ty_name(trait_id, &trait_args)
                ),
            ));
        }
        let output =
            self.builtin_trait_place_method_output(call.target_ty, trait_id, trait_args.clone());
        if let Some(expected) = call.expected {
            self.expect_type(call.span, expected, output, "builtin trait method call");
        }
        self.record_resolved_call(
            call.span,
            ResolvedCall::BuiltinPlaceMethod {
                trait_id,
                method: call.method,
                self_ty: call.target_ty,
                trait_args,
            },
        );
        Some(output)
    }

    fn check_builtin_trait_place_method_value_args(
        &mut self,
        span: Span,
        trait_id: BuiltinTrait,
        method: BuiltinTraitMethod,
        args: &[Expr],
    ) -> Option<Vec<InternedTyId>> {
        let value_param_count = method.param_count().saturating_sub(1);
        self.check_call_arg_count(span, args.len(), value_param_count, false);
        if args.len() != value_param_count {
            for arg in args {
                self.check_expr(arg);
            }
            return None;
        }
        match trait_id {
            BuiltinTrait::SliceConst | BuiltinTrait::Slice => {
                let range = args.first()?;
                let range_ty = self.check_expr(range);
                Some(vec![range_ty])
            }
            _ => {
                for arg in args {
                    self.check_expr(arg);
                }
                Some(Vec::new())
            }
        }
    }

    fn check_builtin_trait_method_value_args(
        &mut self,
        span: Span,
        trait_id: BuiltinTrait,
        method: BuiltinTraitMethod,
        self_ty: InternedTyId,
        args: &[Expr],
    ) -> Option<Vec<InternedTyId>> {
        let value_param_count = method.param_count().saturating_sub(1);
        self.check_call_arg_count(span, args.len(), value_param_count, false);
        if args.len() != value_param_count {
            for arg in args {
                self.check_expr(arg);
            }
            return None;
        }
        match method.param_count() {
            1 => Some(Vec::new()),
            2 => {
                let rhs = args.first()?;
                let rhs_expected = if self.is_numeric_literal_expr(rhs) {
                    Some(self_ty)
                } else {
                    None
                };
                let rhs_ty = self.check_expr_with_expected(rhs, rhs_expected);
                if let Some(expected) = rhs_expected {
                    self.expect_expr_type(rhs, expected, rhs_ty, "call argument");
                }
                let rhs_ty = self.expr_types.get(&rhs.span).copied().unwrap_or(rhs_ty);
                Some(vec![rhs_ty])
            }
            _ => {
                for arg in args {
                    self.check_expr(arg);
                }
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "builtin trait method `{}` has unsupported arity",
                        method.name()
                    ),
                ));
                None
            }
        }
        .map(|trait_args| {
            if matches!(
                trait_id,
                BuiltinTrait::Neg | BuiltinTrait::Not | BuiltinTrait::BitNot
            ) {
                Vec::new()
            } else {
                trait_args
            }
        })
    }

    fn builtin_trait_method_output(
        &mut self,
        self_ty: InternedTyId,
        trait_id: BuiltinTrait,
        trait_args: Vec<InternedTyId>,
    ) -> InternedTyId {
        if matches!(
            trait_id,
            BuiltinTrait::Not | BuiltinTrait::Eq | BuiltinTrait::Ord
        ) {
            return self.bool();
        }
        let output = self.interner.intern(TyKind::Projection {
            self_ty,
            trait_id: TraitId::Builtin(trait_id),
            trait_args,
            name: BuiltinTrait::OUTPUT_ASSOC_TYPE.to_string(),
        });
        self.normalize_projection(output)
    }

    fn builtin_trait_place_method_output(
        &mut self,
        self_ty: InternedTyId,
        trait_id: BuiltinTrait,
        trait_args: Vec<InternedTyId>,
    ) -> InternedTyId {
        match trait_id {
            BuiltinTrait::Len => self.primitive(nia_ty::PrimitiveTy::Usize),
            BuiltinTrait::SliceConst | BuiltinTrait::Slice => {
                let output = self.interner.intern(TyKind::Projection {
                    self_ty,
                    trait_id: TraitId::Builtin(trait_id),
                    trait_args,
                    name: BuiltinTrait::OUTPUT_ASSOC_TYPE.to_string(),
                });
                self.normalize_projection(output)
            }
            BuiltinTrait::GetPtrConst | BuiltinTrait::GetPtr => {
                let target = self.interner.intern(TyKind::Projection {
                    self_ty,
                    trait_id: TraitId::Builtin(trait_id),
                    trait_args: Vec::new(),
                    name: BuiltinTrait::TARGET_ASSOC_TYPE.to_string(),
                });
                let target = self.normalize_projection(target);
                self.interner.intern(TyKind::Pointer {
                    is_const: matches!(trait_id, BuiltinTrait::GetPtrConst),
                    elem: target,
                })
            }
            _ => self.error(),
        }
    }
}
