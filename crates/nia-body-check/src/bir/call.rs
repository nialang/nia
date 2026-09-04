// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{BodyChecker, generic_inst_base};
use nia_ast::{Expr, ExprKind, UnaryOp};
use nia_body_ir::{BuiltinOperator, BuiltinPlaceMethod, TypedCallee, TypedExpr, TypedExprKind};
use nia_ids::{GlobalDefId, InternedTyId, ReceiverKind};
use nia_sema_ir::{BracketSuffixResolution, ResolvedCall};
use nia_symbol::{SymbolId, SymbolMap};
use nia_ty::{ConstGenericArg, TyKind};
use nia_value_resolve::ValueNameResolution;

impl<'a> BodyChecker<'a> {
    // Method syntax and associated-call syntax share one resolved-call model.
    // Lowering must separate the receiver from explicit value arguments before
    // applying substituted parameter types, then reconstruct the receiver in
    // the callee with the mutability required by its receiver kind.
    pub(super) fn lower_callee(
        &mut self,
        call: &Expr,
        callee: &Expr,
        args: &[Expr],
    ) -> TypedCallee {
        if let Some(resolved) = self.resolved_call(call) {
            return self.lower_resolved_callee(call, callee, args, resolved);
        }
        if let Some(reference) = self.function_reference(callee) {
            if reference.args.is_empty() {
                return TypedCallee::Function(reference.def_id);
            }
            return TypedCallee::FunctionInstance {
                def_id: reference.def_id,
                arg_module_id: reference.arg_module_id,
                args: reference.args.clone(),
                const_args: reference.const_args.clone(),
            };
        }
        if let Some(def_id) = self.qualified_value(callee)
            && matches!(
                self.global_def_kind(def_id),
                Some(nia_defs::DefKind::Function | nia_defs::DefKind::Method)
            )
        {
            return TypedCallee::Function(def_id);
        }
        if matches!(callee.kind, ExprKind::BracketSuffix { .. })
            && matches!(
                self.bracket_suffix_resolution(callee),
                Some(BracketSuffixResolution::GenericCall)
            )
        {
            return TypedCallee::FunctionPointer(Box::new(self.lower_expr(callee)));
        }
        if let ExprKind::Ident(_) = generic_inst_base(callee).kind
            && let Some(ValueNameResolution::Def(def_id)) =
                self.value_name(generic_inst_base(callee))
        {
            let def_id = self.global_def_id(def_id);
            if matches!(
                self.global_def_kind(def_id),
                Some(nia_defs::DefKind::Function | nia_defs::DefKind::Method)
            ) {
                return TypedCallee::Function(def_id);
            }
        }
        TypedCallee::FunctionPointer(Box::new(self.lower_expr(callee)))
    }

    pub(super) fn lower_call_args(
        &mut self,
        call: &Expr,
        callee: &Expr,
        args: &[Expr],
    ) -> Vec<TypedExpr> {
        // Associated-call syntax carries its receiver in `args[0]`, whereas
        // method syntax carries it in the callee lhs. `TypedCallee` owns that
        // receiver in both cases, so only the remaining args are value params.
        let skip_first_arg = self
            .resolved_call(call)
            .is_some_and(|resolved| self.call_args_start_with_method_receiver(callee, resolved));
        let value_args = if skip_first_arg && !args.is_empty() {
            &args[1..]
        } else {
            args
        };
        let param_tys = self
            .resolved_call(call)
            .and_then(|resolved| self.lowered_explicit_call_arg_tys(callee, resolved))
            .or_else(|| self.lowered_call_param_tys_from_callee(callee));
        let Some(param_tys) = param_tys else {
            return value_args
                .iter()
                .map(|arg| self.lower_expr_with_checked_ty(arg))
                .collect();
        };
        value_args
            .iter()
            .enumerate()
            .map(|(index, arg)| {
                let expected = param_tys.get(index).copied().or_else(|| {
                    self.node_pointer_array_to_slice_coercions
                        .get(&arg.node_key)
                        .map(|coercion| coercion.slice_ty)
                });
                let expected = expected.and_then(|ty| self.non_error_ty(ty));
                self.lower_expr_with_ty(arg, expected)
            })
            .collect()
    }

    fn call_args_start_with_method_receiver(&self, callee: &Expr, resolved: ResolvedCall) -> bool {
        matches!(
            resolved,
            ResolvedCall::Method { .. }
                | ResolvedCall::TraitMethod { .. }
                | ResolvedCall::BuiltinFunction { .. }
                | ResolvedCall::BuiltinTraitMethod { .. }
                | ResolvedCall::BuiltinMethod { .. }
                | ResolvedCall::BuiltinPlaceMethod { .. }
        ) && !self.callee_has_receiver_lhs(callee)
    }

    pub(super) fn lower_builtin_trait_method_call_args(
        &mut self,
        callee: &Expr,
        args: &[Expr],
    ) -> Vec<TypedExpr> {
        if let Some(receiver) = self.lower_receiver_expr(callee) {
            return std::iter::once(receiver)
                .chain(args.iter().map(|arg| self.lower_expr_with_checked_ty(arg)))
                .collect();
        }
        let Some((receiver, value_args)) = args.split_first() else {
            return Vec::new();
        };
        std::iter::once(self.lower_expr_with_checked_ty(receiver))
            .chain(
                value_args
                    .iter()
                    .map(|arg| self.lower_expr_with_checked_ty(arg)),
            )
            .collect()
    }

    fn lowered_explicit_call_arg_tys(
        &mut self,
        _callee: &Expr,
        resolved: ResolvedCall,
    ) -> Option<Vec<nia_ids::InternedTyId>> {
        let has_receiver_param = matches!(
            resolved,
            ResolvedCall::Method { .. } | ResolvedCall::TraitMethod { .. }
        );
        let params = self.lowered_call_param_tys(resolved)?;
        if has_receiver_param {
            Some(params.into_iter().skip(1).collect())
        } else {
            Some(params)
        }
    }

    fn lowered_call_param_tys_from_callee(
        &mut self,
        callee: &Expr,
    ) -> Option<Vec<nia_ids::InternedTyId>> {
        self.qualified_callee_signature(callee)
            .or_else(|| self.direct_callee_signature(callee))
            .map(|resolved| {
                resolved
                    .signature
                    .params
                    .into_iter()
                    .map(|param| param.ty)
                    .collect()
            })
            .or_else(|| self.lowered_function_pointer_param_tys(callee))
    }

    fn lowered_function_pointer_param_tys(
        &mut self,
        callee: &Expr,
    ) -> Option<Vec<nia_ids::InternedTyId>> {
        let callee_ty = self.expr_ty(callee)?;
        let callee_ty = self.normalize_projection(callee_ty);
        match self.interner.get(callee_ty).cloned() {
            Some(
                TyKind::FunctionPointer { params, .. }
                | TyKind::ClosureState { params, .. }
                | TyKind::Callable { params, .. },
            ) => Some(params),
            _ => None,
        }
    }

    pub(super) fn lower_expr_with_checked_ty(&mut self, expr: &Expr) -> TypedExpr {
        if self.global_const_use(expr).is_some()
            || self.qualified_value(expr).is_some_and(|def_id| {
                matches!(self.global_def_kind(def_id), Some(nia_defs::DefKind::Const))
            })
        {
            let ty = self.expr_ty(expr);
            return self.lower_expr_with_ty(expr, ty);
        }
        self.lower_expr(expr)
    }

    fn lowered_call_param_tys(
        &mut self,
        resolved: ResolvedCall,
    ) -> Option<Vec<nia_ids::InternedTyId>> {
        match resolved {
            ResolvedCall::BuiltinFunction { .. } => None,
            ResolvedCall::Function(def_id) => Some(
                self.resolved_function_signature(def_id)?
                    .signature
                    .params
                    .into_iter()
                    .map(|param| param.ty)
                    .collect(),
            ),
            ResolvedCall::FunctionInstance {
                def_id,
                arg_module_id: _,
                args,
                const_args,
            } => {
                let signature = self.resolved_function_signature(def_id)?.signature;
                let (mut substitutions, const_substitutions) =
                    self.generic_substitutions_and_consts_for_def(def_id, &args, &const_args);
                if substitutions.len() < args.len() {
                    // Some callable definitions inherit effective type
                    // parameters from their owner. Def-local parameters alone
                    // cannot map every resolved argument in that case.
                    let effective_generics = self.effective_generics_for_def(def_id);
                    substitutions = self.generic_substitutions(&effective_generics, &args);
                }
                Some(
                    signature
                        .params
                        .into_iter()
                        .map(|param| {
                            self.substitute_generics_and_consts(
                                param.ty,
                                &substitutions,
                                &const_substitutions,
                            )
                        })
                        .collect(),
                )
            }
            ResolvedCall::Method {
                def_id,
                args,
                const_args,
                receiver_kind: _,
            } => {
                let signature = self.resolved_function_signature(def_id)?.signature;
                let (mut substitutions, const_substitutions) =
                    self.method_substitutions(def_id, &signature, &args, &const_args);
                let receiver_ty = signature
                    .params
                    .first()
                    .and_then(|param| param.receiver)
                    .and_then(|receiver| {
                        let self_ty =
                            self.method_self_target_ty(def_id, &signature, &substitutions)?;
                        let receiver_ty = self.receiver_ty_for_target(self_ty, receiver);
                        self.non_error_ty(receiver_ty)
                    });
                Some(self.substituted_call_param_tys(
                    &signature.params,
                    &mut substitutions,
                    &const_substitutions,
                    receiver_ty,
                    receiver_ty,
                ))
            }
            ResolvedCall::TraitMethod {
                trait_id,
                method_id: _,
                method_name,
                self_ty,
                trait_args,
                trait_const_args,
                args,
                const_args,
                receiver_kind,
            } => {
                let signature = self.trait_method_signature(trait_id, &method_name)?;
                let (mut substitutions, const_substitutions) = self
                    .generic_substitutions_for_trait_call(
                        trait_id,
                        &signature,
                        &trait_args,
                        &trait_const_args,
                        &args,
                        &const_args,
                    );
                let receiver_ty = signature
                    .params
                    .first()
                    .and_then(|param| param.receiver)
                    .and_then(|receiver| {
                        let receiver_ty = self.receiver_ty_for_target(self_ty, receiver);
                        self.non_error_ty(receiver_ty)
                    })
                    .or_else(|| {
                        let receiver_ty = self.receiver_ty_for_target(self_ty, receiver_kind);
                        self.non_error_ty(receiver_ty)
                    });
                Some(self.substituted_call_param_tys(
                    &signature.params,
                    &mut substitutions,
                    &const_substitutions,
                    receiver_ty,
                    Some(self_ty),
                ))
            }
            ResolvedCall::TraitAssociatedFunction {
                trait_id,
                method_id: _,
                method_name,
                self_ty,
                trait_args,
                trait_const_args,
                args,
                const_args,
            } => {
                let signature = self.trait_method_signature(trait_id, &method_name)?;
                let (mut substitutions, const_substitutions) = self
                    .generic_substitutions_for_trait_call(
                        trait_id,
                        &signature,
                        &trait_args,
                        &trait_const_args,
                        &args,
                        &const_args,
                    );
                Some(self.substituted_call_param_tys(
                    &signature.params,
                    &mut substitutions,
                    &const_substitutions,
                    None,
                    Some(self_ty),
                ))
            }
            ResolvedCall::DynamicTraitMethod { params, .. } => Some(params),
            _ => None,
        }
    }

    fn method_substitutions(
        &mut self,
        def_id: nia_ids::GlobalDefId,
        signature: &nia_item_signatures::FunctionSignature,
        args: &[nia_ids::InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) -> (
        SymbolMap<nia_ids::InternedTyId>,
        SymbolMap<nia_ty::ConstGenericArg>,
    ) {
        let mut result = self.generic_substitutions_and_consts_for_def(def_id, args, const_args);
        let generics = self.effective_generics_for_def(def_id);
        if result.0.len() < args.len() {
            result.0 = self.generic_substitutions(&generics, args);
            for (generic, arg) in signature
                .generics
                .iter()
                .zip(args.iter().skip(generics.len()))
            {
                result.0.entry(*generic).or_insert(*arg);
            }
        }
        result
    }

    fn method_self_target_ty(
        &mut self,
        def_id: nia_ids::GlobalDefId,
        signature: &nia_item_signatures::FunctionSignature,
        substitutions: &SymbolMap<nia_ids::InternedTyId>,
    ) -> Option<nia_ids::InternedTyId> {
        if let Some(owner_ty) = self.method_owner_type_by_global(def_id) {
            let ty = self.substitute_generics(owner_ty, substitutions);
            let ty = self.normalize_projection(ty);
            let ty = self.normalize_aliases_in_type(ty);
            return self.non_error_ty(ty);
        }
        let ty = signature.params.first()?.ty;
        let ty = self.substitute_generics(ty, substitutions);
        self.non_error_ty(ty)
    }

    fn generic_substitutions_for_trait_call(
        &mut self,
        trait_id: nia_ids::GlobalDefId,
        method_signature: &nia_item_signatures::FunctionSignature,
        trait_args: &[nia_ids::InternedTyId],
        trait_const_args: &[nia_ty::ConstGenericArg],
        method_args: &[nia_ids::InternedTyId],
        method_const_args: &[nia_ty::ConstGenericArg],
    ) -> (
        SymbolMap<nia_ids::InternedTyId>,
        SymbolMap<nia_ty::ConstGenericArg>,
    ) {
        let trait_signature = self.resolved_trait_signature(trait_id);
        let trait_generics = trait_signature
            .as_ref()
            .map(|signature| signature.generics.clone())
            .unwrap_or_default();
        let mut substitutions = self.generic_substitutions(&trait_generics, trait_args);
        for (name, arg) in method_signature
            .generic_params
            .iter()
            .filter_map(|param| {
                matches!(
                    param.kind,
                    nia_item_signatures::GenericParamSignatureKind::Type
                )
                .then_some(param.name)
            })
            .zip(method_args.iter().copied())
        {
            substitutions.insert(name, arg);
        }
        let (_, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(trait_id, trait_args, trait_const_args);
        let mut const_substitutions = const_substitutions;
        for (name, arg) in method_signature
            .generic_params
            .iter()
            .filter_map(|param| {
                matches!(
                    param.kind,
                    nia_item_signatures::GenericParamSignatureKind::Const { .. }
                )
                .then_some(param.name)
            })
            .zip(method_const_args.iter().cloned())
        {
            const_substitutions.insert(name, arg);
        }
        (substitutions, const_substitutions)
    }

    fn trait_method_signature(
        &mut self,
        trait_id: nia_ids::GlobalDefId,
        method_name: &SymbolId,
    ) -> Option<nia_item_signatures::FunctionSignature> {
        self.resolved_trait_signature(trait_id)?
            .methods
            .into_iter()
            .find(|method| &method.name == method_name)
            .map(|method| method.signature)
    }

    fn substituted_call_param_tys(
        &mut self,
        params: &[nia_item_signatures::ParamSignature],
        substitutions: &mut SymbolMap<nia_ids::InternedTyId>,
        const_substitutions: &SymbolMap<nia_ty::ConstGenericArg>,
        receiver_ty: Option<nia_ids::InternedTyId>,
        self_ty: Option<nia_ids::InternedTyId>,
    ) -> Vec<nia_ids::InternedTyId> {
        params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let ty = if index == 0 {
                    receiver_ty.unwrap_or_else(|| match self_ty {
                        Some(self_ty) => self.substitute_generics_and_consts_with_self(
                            param.ty,
                            substitutions,
                            const_substitutions,
                            self_ty,
                        ),
                        None => self.substitute_generics_and_consts(
                            param.ty,
                            substitutions,
                            const_substitutions,
                        ),
                    })
                } else {
                    match self_ty {
                        Some(self_ty) => self.substitute_generics_and_consts_with_self(
                            param.ty,
                            substitutions,
                            const_substitutions,
                            self_ty,
                        ),
                        None => self.substitute_generics_and_consts(
                            param.ty,
                            substitutions,
                            const_substitutions,
                        ),
                    }
                };
                let ty = self.normalize_projection(ty);
                self.normalize_aliases_in_type(ty)
            })
            .collect()
    }

    fn lower_resolved_callee(
        &mut self,
        call: &Expr,
        callee: &Expr,
        call_args: &[Expr],
        resolved: ResolvedCall,
    ) -> TypedCallee {
        let tracks_caller = self.resolved_call_tracks_caller(&resolved);
        let lowered = match resolved {
            ResolvedCall::BuiltinFunction { .. } => {
                TypedCallee::FunctionPointer(Box::new(self.error_expr(callee.span)))
            }
            ResolvedCall::Function(def_id) => TypedCallee::Function(def_id),
            ResolvedCall::FunctionInstance {
                def_id,
                arg_module_id,
                args,
                const_args,
            } => TypedCallee::FunctionInstance {
                def_id,
                arg_module_id,
                args,
                const_args,
            },
            ResolvedCall::Method {
                def_id,
                args,
                const_args,
                receiver_kind,
            } => {
                let receiver_ty = self
                    .lowered_call_param_tys(ResolvedCall::Method {
                        def_id,
                        args: args.clone(),
                        const_args: const_args.clone(),
                        receiver_kind,
                    })
                    .and_then(|tys| tys.first().copied());
                TypedCallee::Method {
                    def_id,
                    args,
                    const_args,
                    receiver_kind,
                    receiver: Box::new(self.lower_method_receiver_expr(
                        callee,
                        call_args,
                        receiver_kind,
                        receiver_ty,
                    )),
                }
            }
            ResolvedCall::TraitMethod {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                trait_const_args,
                args,
                const_args,
                receiver_kind,
            } => {
                let implementation_method = self.selected_trait_method_implementation(
                    trait_id,
                    method_name,
                    self_ty,
                    &trait_args,
                    &trait_const_args,
                );
                let receiver_ty = self
                    .lowered_call_param_tys(ResolvedCall::TraitMethod {
                        trait_id,
                        method_id,
                        method_name,
                        self_ty,
                        trait_args: trait_args.clone(),
                        trait_const_args: trait_const_args.clone(),
                        args: args.clone(),
                        const_args: const_args.clone(),
                        receiver_kind,
                    })
                    .and_then(|tys| tys.first().copied());
                TypedCallee::TraitMethod {
                    trait_id,
                    method_id,
                    implementation_method,
                    method_name,
                    self_ty,
                    trait_args,
                    trait_const_args,
                    args,
                    const_args,
                    receiver_kind,
                    receiver: Box::new(self.lower_method_receiver_expr(
                        callee,
                        call_args,
                        receiver_kind,
                        receiver_ty,
                    )),
                }
            }
            ResolvedCall::TraitAssociatedFunction {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                trait_const_args,
                args,
                const_args,
            } => TypedCallee::TraitAssociatedFunction {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                trait_const_args,
                args,
                const_args,
            },
            ResolvedCall::DynamicTraitMethod {
                object_ty,
                trait_id,
                method_id,
                method_name,
                trait_args,
                trait_const_args,
                slot,
                params,
                return_type,
                receiver_kind,
            } => TypedCallee::DynamicTraitMethod {
                object_ty,
                trait_id,
                method_id,
                method_name,
                trait_args,
                trait_const_args,
                slot,
                params,
                return_type,
                receiver_kind,
                receiver: Box::new(
                    self.lower_receiver_expr(callee)
                        .unwrap_or_else(|| self.lower_expr(callee)),
                ),
            },
            ResolvedCall::BuiltinTraitMethod { trait_id, op, .. } => {
                TypedCallee::BuiltinOperator(BuiltinOperator { trait_id, op })
            }
            ResolvedCall::BuiltinMethod { method, self_ty } => {
                let receiver = self
                    .lower_receiver_expr(callee)
                    .unwrap_or_else(|| self.lower_expr(callee));
                TypedCallee::BuiltinMethod {
                    method,
                    self_ty,
                    receiver: Box::new(receiver),
                }
            }
            ResolvedCall::BuiltinPlaceMethod {
                trait_id,
                method,
                self_ty,
                trait_args,
            } => {
                let receiver = self
                    .lower_receiver_expr(callee)
                    .unwrap_or_else(|| self.lower_expr(callee));
                TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                    trait_id,
                    method,
                    self_ty,
                    trait_args,
                    receiver: Box::new(
                        self.lower_typed_builtin_place_method_receiver(&receiver, self_ty, method),
                    ),
                })
            }
            ResolvedCall::Closure => TypedCallee::Closure(Box::new(self.lower_expr(callee))),
            ResolvedCall::Callable => TypedCallee::Callable(Box::new(self.lower_expr(callee))),
            ResolvedCall::FunctionPointer => {
                TypedCallee::FunctionPointer(Box::new(self.lower_expr(callee)))
            }
        };
        if tracks_caller {
            TypedCallee::Tracked {
                callee: Box::new(lowered),
                location: nia_source::SourceLocation::at(
                    &self.source_path.identity(),
                    self.source_text,
                    call.span.start,
                ),
            }
        } else {
            lowered
        }
    }

    fn resolved_call_tracks_caller(&mut self, resolved: &ResolvedCall) -> bool {
        let attributes = match resolved {
            ResolvedCall::Function(def_id)
            | ResolvedCall::FunctionInstance { def_id, .. }
            | ResolvedCall::Method { def_id, .. } => self
                .resolved_function_signature(*def_id)
                .map(|resolved| resolved.signature.attributes),
            ResolvedCall::TraitMethod {
                trait_id,
                method_name,
                ..
            }
            | ResolvedCall::TraitAssociatedFunction {
                trait_id,
                method_name,
                ..
            } => self
                .trait_method_signature(*trait_id, method_name)
                .map(|signature| signature.attributes),
            ResolvedCall::DynamicTraitMethod {
                trait_id: nia_ty::TraitId::Source(trait_id),
                method_name,
                ..
            } => self
                .trait_method_signature(*trait_id, method_name)
                .map(|signature| signature.attributes),
            _ => None,
        };
        attributes.is_some_and(|attributes| {
            attributes.iter().any(|attribute| {
                matches!(
                    attribute,
                    nia_item_signatures::FunctionAttribute::TrackCaller
                )
            })
        })
    }

    fn selected_trait_method_implementation(
        &mut self,
        trait_id: GlobalDefId,
        method_name: nia_symbol::SymbolId,
        self_ty: InternedTyId,
        trait_args: &[InternedTyId],
        trait_const_args: &[ConstGenericArg],
    ) -> Option<GlobalDefId> {
        let resolution = self.current_context_resolve_trait_obligation_with_const_args(
            self_ty,
            nia_ty::TraitId::Source(trait_id),
            trait_args.to_vec(),
            trait_const_args.to_vec(),
        );
        let nia_trait_solve::TraitResolution::User(user_impl) = resolution else {
            return None;
        };
        let implementation = self.program_trait_impls.get(user_impl.impl_index)?;
        self.with_visible_extensions(|extensions| {
            extensions.all_trait_witnesses_named(&method_name)
        })
        .into_iter()
        .find_map(|(_, method)| {
            (method.def_id.module_id == implementation.module_id
                && method.impl_id == implementation.impl_id
                && method.trait_id == Some(nia_ty::TraitId::Source(trait_id)))
            .then_some(method.def_id)
        })
    }

    fn lower_receiver_expr(&mut self, callee: &Expr) -> Option<TypedExpr> {
        self.lower_receiver_expr_with_ty(callee, None)
    }

    fn callee_has_receiver_lhs(&self, callee: &Expr) -> bool {
        match &callee.kind {
            ExprKind::Field { .. } => true,
            ExprKind::BracketSuffix {
                callee: generic_callee,
                ..
            } if matches!(
                self.bracket_suffix_resolution(callee),
                Some(BracketSuffixResolution::GenericCall)
            ) =>
            {
                self.callee_has_receiver_lhs(generic_callee)
            }
            _ => false,
        }
    }

    fn lower_receiver_expr_with_ty(
        &mut self,
        callee: &Expr,
        expected: Option<nia_ids::InternedTyId>,
    ) -> Option<TypedExpr> {
        let field_callee = match &callee.kind {
            ExprKind::Field { .. } => callee,
            ExprKind::BracketSuffix {
                callee: generic_callee,
                ..
            } if matches!(
                self.bracket_suffix_resolution(callee),
                Some(BracketSuffixResolution::GenericCall)
            ) =>
            {
                generic_callee.as_ref()
            }
            _ => return None,
        };
        let lhs = match &field_callee.kind {
            ExprKind::Field { lhs, .. } => lhs,
            _ => return None,
        };
        Some(self.lower_expr_with_ty(lhs, expected))
    }

    fn lower_method_receiver_expr(
        &mut self,
        callee: &Expr,
        call_args: &[Expr],
        receiver_kind: ReceiverKind,
        receiver_ty: Option<nia_ids::InternedTyId>,
    ) -> TypedExpr {
        let receiver_expr_ty =
            receiver_ty.map(|ty| self.receiver_expr_ty_for_lowering(callee, ty, receiver_kind));
        if self.callee_has_receiver_lhs(callee)
            && let Some(receiver) = self.lower_receiver_expr_with_ty(callee, receiver_expr_ty)
        {
            return receiver;
        }
        if let Some(receiver) = call_args.first() {
            return self.lower_expr_with_ty(receiver, receiver_expr_ty);
        }
        self.lower_expr_with_ty(callee, receiver_expr_ty)
    }

    fn receiver_expr_ty_for_lowering(
        &mut self,
        callee: &Expr,
        receiver_ty: nia_ids::InternedTyId,
        receiver_kind: ReceiverKind,
    ) -> nia_ids::InternedTyId {
        match receiver_kind {
            ReceiverKind::Value => receiver_ty,
            ReceiverKind::RefReadOnly | ReceiverKind::Ref => {
                if let Some(actual_ty) = self.receiver_lhs_expr_ty(callee)
                    && self
                        .receiver_expr_already_matches_lowered_receiver_ty(receiver_ty, actual_ty)
                {
                    return receiver_ty;
                }
                match self.interner.get(self.normalization.normalize(receiver_ty)) {
                    Some(TyKind::Pointer { elem, .. } | TyKind::VolatilePointer { elem, .. }) => {
                        *elem
                    }
                    _ => receiver_ty,
                }
            }
        }
    }

    fn receiver_lhs_expr_ty(&mut self, callee: &Expr) -> Option<nia_ids::InternedTyId> {
        let field_callee = match &callee.kind {
            ExprKind::Field { .. } => callee,
            ExprKind::BracketSuffix {
                callee: generic_callee,
                ..
            } if matches!(
                self.bracket_suffix_resolution(callee),
                Some(BracketSuffixResolution::GenericCall)
            ) =>
            {
                generic_callee.as_ref()
            }
            _ => return None,
        };
        let ExprKind::Field { lhs, .. } = &field_callee.kind else {
            return None;
        };
        self.expr_ty(lhs)
    }

    fn receiver_expr_already_matches_lowered_receiver_ty(
        &mut self,
        receiver_ty: nia_ids::InternedTyId,
        actual_ty: nia_ids::InternedTyId,
    ) -> bool {
        if self.types_match(receiver_ty, actual_ty) {
            return true;
        }
        let receiver_ty = self.normalization.normalize(receiver_ty);
        let actual_ty = self.normalization.normalize(actual_ty);
        match (
            self.interner.get(receiver_ty).cloned(),
            self.interner.get(actual_ty).cloned(),
        ) {
            (
                Some(TyKind::Pointer {
                    is_readonly: expected_readonly,
                    elem: expected_elem,
                }),
                Some(TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
            ) => {
                // A mutable pointer can satisfy a readonly receiver, but the
                // reverse would manufacture write access during lowering.
                (expected_readonly || !actual_readonly)
                    && self.types_match(expected_elem, actual_elem)
            }
            _ => false,
        }
    }

    pub(super) fn lower_builtin_call_receiver(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        receiver_ty: Option<nia_ids::InternedTyId>,
    ) -> (TypedExpr, Vec<TypedExpr>) {
        if let Some(receiver) = self.lower_receiver_expr_with_ty(callee, receiver_ty) {
            return (
                receiver,
                args.iter()
                    .map(|arg| self.lower_expr_with_checked_ty(arg))
                    .collect(),
            );
        }
        if let Some((receiver, args)) = args.split_first() {
            return (
                self.lower_expr_with_checked_ty(receiver),
                args.iter()
                    .map(|arg| self.lower_expr_with_checked_ty(arg))
                    .collect(),
            );
        }
        (self.lower_expr(callee), Vec::new())
    }

    pub(super) fn lower_builtin_value_call_receiver(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        self_ty: nia_ids::InternedTyId,
    ) -> (TypedExpr, Vec<TypedExpr>) {
        let (mut receiver, args) = self.lower_builtin_call_receiver(callee, args, None);
        while !self.types_match(receiver.ty, self_ty) {
            let Some(TyKind::Pointer { elem, .. }) = self
                .interner
                .get(self.normalization.normalize(receiver.ty))
                .cloned()
            else {
                break;
            };
            receiver = TypedExpr {
                span: receiver.span,
                ty: elem,
                kind: TypedExprKind::Unary {
                    op: UnaryOp::Deref,
                    expr: Box::new(receiver),
                },
            };
        }
        (receiver, args)
    }
}
