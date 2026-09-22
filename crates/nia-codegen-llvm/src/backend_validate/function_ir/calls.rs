// SPDX-License-Identifier: GPL-3.0-or-later
//! Call, ABI, vtable, and inline-assembly validation contracts.

use super::*;

impl BackendValidator<'_> {
    pub(super) fn validate_callee(
        &mut self,
        callee: &FunctionCallee,
        call_args: &[FunctionExpr],
        call_result_ty: nia_ids::InternedTyId,
        span: Span,
    ) {
        self.validate_callee_with_tracking(callee, call_args, call_result_ty, span, false);
    }

    fn validate_callee_with_tracking(
        &mut self,
        callee: &FunctionCallee,
        call_args: &[FunctionExpr],
        call_result_ty: nia_ids::InternedTyId,
        span: Span,
        has_tracked_metadata: bool,
    ) {
        match callee {
            FunctionCallee::Tracked { callee, .. } => {
                if matches!(callee.as_ref(), FunctionCallee::Tracked { .. }) {
                    self.invalid_call_contract(
                        span,
                        "tracked-caller",
                        "tracked-caller metadata cannot be nested",
                    );
                    return;
                }
                if !self.callee_uses_tracked_caller_abi(callee) {
                    self.invalid_call_contract(
                        span,
                        "tracked-caller",
                        "tracked-caller metadata targets a function without the tracked-caller ABI",
                    );
                }
                self.validate_callee_with_tracking(callee, call_args, call_result_ty, span, true)
            }
            FunctionCallee::ClosureEntry { closure_id, state } => {
                self.validate_expr(state);
                let Some(owner) = self.current_closure_owner.clone() else {
                    self.invalid_call_contract(
                        span,
                        "closure-entry",
                        "call has no enclosing closure owner",
                    );
                    return;
                };
                let key = nia_backend_ir::BackendClosureEntryKey {
                    closure_id: *closure_id,
                    owner,
                };
                let Some(entry) = self.index.closure_entry(&key) else {
                    self.invalid_call_contract(
                        span,
                        "closure-entry",
                        "call references a missing generated entry",
                    );
                    return;
                };
                let state_pointer_type = entry.abi.state_pointer_type;
                let params = entry.abi.params.clone();
                let return_type = entry.abi.return_type;
                if !self.same_type(state.ty, state_pointer_type) {
                    self.invalid_call_contract(
                        span,
                        "closure-entry",
                        "state pointer type does not match generated entry ABI",
                    );
                }
                self.validate_typed_call_signature(TypedCallContract {
                    kind: "closure-entry",
                    args: call_args,
                    params: &params,
                    return_type,
                    is_variadic: false,
                    result_ty: call_result_ty,
                    span,
                });
            }
            FunctionCallee::Function(def_id) => {
                if !has_tracked_metadata && self.callee_uses_tracked_caller_abi(callee) {
                    self.invalid_call_contract(
                        span,
                        "tracked-caller",
                        "tracked-caller function call is missing caller metadata",
                    );
                }
                self.validate_function_ref(
                    *def_id,
                    span,
                    "backend IR call references missing function",
                );
                if let Some(signature) = self.function_call_signature(*def_id) {
                    self.validate_call_signature(
                        "function",
                        call_args,
                        &signature,
                        call_result_ty,
                        span,
                    );
                }
            }
            FunctionCallee::FunctionInstance {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
            } => {
                if !has_tracked_metadata && self.callee_uses_tracked_caller_abi(callee) {
                    self.invalid_call_contract(
                        span,
                        "tracked-caller",
                        "tracked-caller function call is missing caller metadata",
                    );
                }
                let instance = FunctionInstanceRef {
                    def_id: *def_id,
                    arg_module_id: *arg_module_id,
                    self_arg: *self_arg,
                    args,
                    const_args,
                };
                self.validate_function_instance_ref(
                    instance,
                    span,
                    "backend IR call references missing function instance",
                );
                if let Some(signature) = self.function_instance_call_signature(instance) {
                    self.validate_call_signature(
                        "function-instance",
                        call_args,
                        &signature,
                        call_result_ty,
                        span,
                    );
                }
            }
            FunctionCallee::Method {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
                receiver_kind,
                receiver,
            } => {
                if !has_tracked_metadata && self.callee_uses_tracked_caller_abi(callee) {
                    self.invalid_call_contract(
                        span,
                        "tracked-caller",
                        "tracked-caller method call is missing caller metadata",
                    );
                }
                self.validate_expr(receiver);
                let signature = if self_arg.is_none() && args.is_empty() && const_args.is_empty() {
                    self.validate_function_ref(
                        *def_id,
                        span,
                        "backend IR method call references missing function",
                    );
                    self.function_call_signature(*def_id)
                } else {
                    let instance = FunctionInstanceRef {
                        def_id: *def_id,
                        arg_module_id: *arg_module_id,
                        self_arg: *self_arg,
                        args,
                        const_args,
                    };
                    self.validate_function_instance_ref(
                        instance,
                        span,
                        "backend IR method call references missing function instance",
                    );
                    self.function_instance_call_signature(instance)
                };
                if let Some(signature) = signature {
                    self.validate_method_call_signature(
                        call_args,
                        call_result_ty,
                        *receiver_kind,
                        &signature,
                        span,
                    );
                }
            }
            FunctionCallee::DynamicTraitMethod {
                object_ty,
                trait_id,
                method_id,
                trait_args,
                trait_const_args,
                slot,
                params,
                return_type,
                receiver_kind,
                receiver,
                ..
            } => {
                self.validate_type(*object_ty, span);
                self.validate_runtime_type(*return_type, span);
                for param in params {
                    self.validate_runtime_type(*param, span);
                }
                self.validate_expr(receiver);
                self.validate_dynamic_trait_call(DynamicTraitCallContract {
                    object_ty: *object_ty,
                    trait_id: *trait_id,
                    method_id: *method_id,
                    trait_args,
                    trait_const_args,
                    slot: *slot,
                    params,
                    return_type: *return_type,
                    receiver_kind: *receiver_kind,
                    receiver,
                    args: call_args,
                    result_ty: call_result_ty,
                    tracks_caller: has_tracked_metadata,
                    span,
                });
            }
            FunctionCallee::BuiltinMethod {
                method,
                self_ty,
                receiver,
            } => {
                self.validate_type(*self_ty, span);
                self.validate_expr(receiver);
                self.validate_builtin_method_receiver(*method, *self_ty, receiver, span);
                if !call_args.is_empty() {
                    self.invalid_call_contract(
                        span,
                        "builtin-method",
                        "builtin methods do not accept value arguments",
                    );
                }
                self.validate_builtin_method_call(*method, *self_ty, call_result_ty, span);
            }
            FunctionCallee::BuiltinTraitMethodCall {
                trait_id,
                method,
                self_ty,
                trait_args,
                receiver,
            } => {
                self.validate_type(*self_ty, span);
                for arg in trait_args {
                    self.validate_type(*arg, span);
                }
                self.validate_expr(receiver);
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    span,
                    format!(
                        "backend IR call contains unresolved builtin place method {trait_id:?}::{method:?}"
                    ),
                ));
            }
            FunctionCallee::TraitMethod {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
                receiver,
                ..
            } => {
                self.validate_type(*self_ty, span);
                for arg in trait_args.iter().chain(args) {
                    self.validate_type(*arg, span);
                }
                self.validate_expr(receiver);
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    span,
                    format!(
                        "backend IR call contains unresolved trait method `{}` {method_id:?} on trait {trait_id:?}",
                        mangle_symbol_id(*method_name)
                    ),
                ));
            }
            FunctionCallee::TraitAssociatedFunction {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
                ..
            } => {
                self.validate_type(*self_ty, span);
                for arg in trait_args.iter().chain(args) {
                    self.validate_type(*arg, span);
                }
                self.diagnostics.push(Diagnostic::internal_error_at(nia_diagnostic::codes::INVALID_BACKEND_IR,
                    span,
                    format!(
                        "backend IR call contains unresolved trait associated function `{}` {method_id:?} on trait {trait_id:?}",
                        mangle_symbol_id(*method_name)
                    ),
                ));
            }
            FunctionCallee::Callable(expr) => {
                self.validate_expr(expr);
                let Some(TyKind::Callable {
                    params,
                    return_type,
                    ..
                }) = self.index.ty_kind(expr.ty).cloned()
                else {
                    self.invalid_call_contract(
                        span,
                        "callable",
                        "callee expression does not have callable type",
                    );
                    return;
                };
                self.validate_typed_call_signature(TypedCallContract {
                    kind: "callable",
                    args: call_args,
                    params: &params,
                    return_type,
                    is_variadic: false,
                    result_ty: call_result_ty,
                    span,
                });
            }
            FunctionCallee::FunctionPointer(expr) => {
                self.validate_expr(expr);
                let Some(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                }) = self.index.ty_kind(expr.ty).cloned()
                else {
                    self.invalid_call_contract(
                        span,
                        "function-pointer",
                        "callee expression does not have function-pointer type",
                    );
                    return;
                };
                self.validate_typed_call_signature(TypedCallContract {
                    kind: "function-pointer",
                    args: call_args,
                    params: &params,
                    return_type,
                    is_variadic,
                    result_ty: call_result_ty,
                    span,
                });
            }
            // Intrinsic value operators are intentionally selected in LLVM codegen; backend
            // lowering only rewrites them when a source-level extension method wins dispatch.
            FunctionCallee::BuiltinOperator(operator) => {
                let Some(method) = operator.op.method() else {
                    self.invalid_call_contract(
                        span,
                        "builtin-operator",
                        "operator has no builtin trait dispatch contract",
                    );
                    return;
                };
                if method.trait_id() != operator.trait_id {
                    self.invalid_call_contract(
                        span,
                        "builtin-operator",
                        "operator trait does not match its operation",
                    );
                }
                let expected = match operator.op {
                    nia_function_ir::FunctionBuiltinOperatorOp::Unary(_) => 1,
                    nia_function_ir::FunctionBuiltinOperatorOp::Binary(_) => 2,
                };
                if call_args.len() != expected {
                    self.invalid_call_contract(
                        span,
                        "builtin-operator",
                        "argument count does not match operator arity",
                    );
                } else {
                    match operator.op {
                        nia_function_ir::FunctionBuiltinOperatorOp::Unary(op) => {
                            self.validate_unary(call_result_ty, op, &call_args[0], span);
                        }
                        nia_function_ir::FunctionBuiltinOperatorOp::Binary(op) => {
                            self.validate_binary_contract(
                                call_result_ty,
                                call_args[0].ty,
                                op,
                                call_args[1].ty,
                                span,
                            );
                        }
                    }
                }
            }
        }
    }

    fn validate_builtin_method_call(
        &mut self,
        method: nia_function_ir::FunctionBuiltinMethod,
        self_ty: nia_ids::InternedTyId,
        result_ty: nia_ids::InternedTyId,
        span: Span,
    ) {
        match method {
            nia_function_ir::FunctionBuiltinMethod::SliceLen => {
                if !matches!(
                    self.index.ty_kind(self_ty),
                    Some(TyKind::Array { .. } | TyKind::Slice { .. })
                ) {
                    self.invalid_call_contract(
                        span,
                        "builtin-method",
                        "len receiver type is not an array or slice",
                    );
                }
                if !matches!(
                    self.index.ty_kind(result_ty),
                    Some(TyKind::Primitive(PrimitiveTy::Usize))
                ) {
                    self.invalid_call_contract(
                        span,
                        "builtin-method",
                        "len result type is not usize",
                    );
                }
            }
            nia_function_ir::FunctionBuiltinMethod::SlicePtr
            | nia_function_ir::FunctionBuiltinMethod::SlicePtrMut => {
                let Some(TyKind::Slice { is_readonly, elem }) = self.index.ty_kind(self_ty) else {
                    self.invalid_call_contract(
                        span,
                        "builtin-method",
                        "pointer receiver type is not a slice",
                    );
                    return;
                };
                let expected_readonly = method == nia_function_ir::FunctionBuiltinMethod::SlicePtr;
                if !expected_readonly && *is_readonly {
                    self.invalid_call_contract(
                        span,
                        "builtin-method",
                        "mutable pointer method has a readonly slice receiver",
                    );
                }
                if !matches!(
                    self.index.ty_kind(result_ty),
                    Some(TyKind::Pointer {
                        is_readonly,
                        elem: result_elem,
                    }) if *is_readonly == expected_readonly
                        && self.same_type(*result_elem, *elem)
                ) {
                    self.invalid_call_contract(
                        span,
                        "builtin-method",
                        "slice pointer result type does not match its receiver",
                    );
                }
            }
            nia_function_ir::FunctionBuiltinMethod::Start
            | nia_function_ir::FunctionBuiltinMethod::End => {
                let Some(TyKind::Range { kind, bound }) = self.index.ty_kind(self_ty) else {
                    self.invalid_call_contract(
                        span,
                        "builtin-method",
                        "range-bound receiver type is not a range",
                    );
                    return;
                };
                let has_bound = match method {
                    nia_function_ir::FunctionBuiltinMethod::Start => kind.has_start_bound(),
                    nia_function_ir::FunctionBuiltinMethod::End => kind.has_end_bound(),
                    _ => false,
                };
                if !has_bound {
                    self.invalid_call_contract(
                        span,
                        "builtin-method",
                        "range receiver does not contain the requested bound",
                    );
                }
                if bound.is_none_or(|bound| !self.same_type(bound, result_ty)) {
                    self.invalid_call_contract(
                        span,
                        "builtin-method",
                        "range-bound result type does not match its receiver",
                    );
                }
            }
            nia_function_ir::FunctionBuiltinMethod::Iter => self.invalid_call_contract(
                span,
                "builtin-method",
                "iter must be resolved before LLVM codegen",
            ),
        }
    }

    fn validate_builtin_method_receiver(
        &mut self,
        method: nia_function_ir::FunctionBuiltinMethod,
        self_ty: nia_ids::InternedTyId,
        receiver: &FunctionExpr,
        span: Span,
    ) {
        let receiver_matches = self.same_type(receiver.ty, self_ty)
            || match self.index.ty_kind(receiver.ty) {
                Some(TyKind::Pointer { is_readonly, elem }) if self.same_type(*elem, self_ty) => {
                    if method == nia_function_ir::FunctionBuiltinMethod::SlicePtrMut && *is_readonly
                    {
                        self.invalid_call_contract(
                            span,
                            "builtin-method",
                            "mutable pointer method has a readonly receiver pointer",
                        );
                    }
                    true
                }
                _ => false,
            };
        if !receiver_matches {
            self.invalid_call_contract(
                span,
                "builtin-method",
                "receiver type does not match builtin method metadata",
            );
        }
    }

    pub(super) fn function_call_signature(
        &self,
        def_id: nia_ids::GlobalDefId,
    ) -> Option<CallTargetSignature> {
        self.index
            .function(def_id)
            .map(|function| CallTargetSignature {
                params: function.params.clone(),
                return_type: function.return_type,
                is_variadic: function.is_variadic,
            })
    }

    fn callee_uses_tracked_caller_abi(&self, callee: &FunctionCallee) -> bool {
        use nia_backend_ir::BackendFunctionAttribute::TrackCaller;

        match callee {
            FunctionCallee::Function(def_id) => self
                .index
                .function(*def_id)
                .is_some_and(|function| function.attributes.contains(&TrackCaller)),
            FunctionCallee::FunctionInstance {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
            } => self
                .index
                .function_instance(*def_id, *arg_module_id, *self_arg, args, const_args)
                .or_else(|| {
                    self.index.function_instances_for(*def_id).find(|item| {
                        self.same_optional_type(item.self_arg, *self_arg)
                            && self.same_type_args(&item.args, args)
                            && self.same_const_args(&item.const_args, const_args)
                    })
                })
                .is_some_and(|function| function.attributes.contains(&TrackCaller)),
            FunctionCallee::Method {
                def_id,
                self_arg,
                args,
                const_args,
                ..
            } if self_arg.is_none() && args.is_empty() && const_args.is_empty() => self
                .index
                .function(*def_id)
                .is_some_and(|function| function.attributes.contains(&TrackCaller)),
            FunctionCallee::Method {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
                ..
            } => self
                .index
                .function_instance(*def_id, *arg_module_id, *self_arg, args, const_args)
                .or_else(|| {
                    self.index.function_instances_for(*def_id).find(|item| {
                        self.same_optional_type(item.self_arg, *self_arg)
                            && self.same_type_args(&item.args, args)
                            && self.same_const_args(&item.const_args, const_args)
                    })
                })
                .is_some_and(|function| function.attributes.contains(&TrackCaller)),
            FunctionCallee::DynamicTraitMethod { .. } => true,
            FunctionCallee::Tracked { .. }
            | FunctionCallee::ClosureEntry { .. }
            | FunctionCallee::TraitMethod { .. }
            | FunctionCallee::TraitAssociatedFunction { .. }
            | FunctionCallee::BuiltinTraitMethodCall { .. }
            | FunctionCallee::BuiltinMethod { .. }
            | FunctionCallee::BuiltinOperator(_)
            | FunctionCallee::Callable(_)
            | FunctionCallee::FunctionPointer(_) => false,
        }
    }

    pub(super) fn function_instance_call_signature(
        &self,
        instance: FunctionInstanceRef<'_>,
    ) -> Option<CallTargetSignature> {
        let FunctionInstanceRef {
            def_id,
            arg_module_id,
            self_arg,
            args,
            const_args,
        } = instance;
        self.index
            .function_instance(def_id, arg_module_id, self_arg, args, const_args)
            .or_else(|| {
                self.index.function_instances_for(def_id).find(|item| {
                    self.same_optional_type(item.self_arg, self_arg)
                        && self.same_type_args(&item.args, args)
                        && self.same_const_args(&item.const_args, const_args)
                })
            })
            .map(|function| CallTargetSignature {
                params: function.params.clone(),
                return_type: function.return_type,
                is_variadic: function.is_variadic,
            })
    }

    pub(super) fn validate_function_value_signature(
        &mut self,
        kind: &'static str,
        value_ty: nia_ids::InternedTyId,
        signature: &CallTargetSignature,
        span: Span,
    ) {
        let Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        }) = self.index.ty_kind(value_ty)
        else {
            self.invalid_function_value_contract(
                span,
                kind,
                "value type is not a function pointer",
            );
            return;
        };

        // Function-pointer types describe the source-visible signature. In
        // particular, a method receiver may have a pointer-shaped `passing_ty`
        // at the LLVM boundary while retaining its semantic `local_ty` here.
        if params.len() != signature.params.len()
            || params
                .iter()
                .zip(&signature.params)
                .any(|(actual, expected)| !self.same_type(*actual, expected.local_ty))
        {
            self.invalid_function_value_contract(
                span,
                kind,
                "parameter types do not match the published signature",
            );
        }
        if !self.same_type(*return_type, signature.return_type) {
            self.invalid_function_value_contract(
                span,
                kind,
                "return type does not match the published signature",
            );
        }
        if *is_variadic != signature.is_variadic {
            self.invalid_function_value_contract(
                span,
                kind,
                "variadic flag does not match the published signature",
            );
        }
    }

    pub(super) fn validate_inline_asm(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        asm: &nia_function_ir::FunctionInlineAsm,
        span: Span,
    ) {
        if !matches!(self.ty_kind(result_ty), Some(kind) if kind.is_unit()) {
            self.invalid_inline_asm(span, "expression result type is not unit");
        }
        for input in &asm.inputs {
            self.validate_expr(&input.value);
            if !self.is_inline_asm_operand_type(input.value.ty) {
                self.invalid_inline_asm(input.span, "input operand type is not scalar");
            }
            if !Self::is_inline_asm_input_constraint(&input.constraint) {
                self.invalid_inline_asm(input.span, "input constraint is not canonical");
            }
        }
        for output in &asm.outputs {
            let selected_ty = self.validate_place(&output.place);
            if !self.is_inline_asm_operand_type(output.place.ty) {
                self.invalid_inline_asm(output.span, "output operand type is not scalar");
            }
            if selected_ty.is_some_and(|ty| !self.same_type(output.place.ty, ty)) {
                self.invalid_inline_asm(output.span, "output type is only a readonly storage view");
            }
            if !self.place_is_writable(&output.place) {
                self.invalid_inline_asm(output.span, "output storage is not writable");
            }
            if !Self::is_inline_asm_output_constraint(&output.constraint) {
                self.invalid_inline_asm(output.span, "output constraint is not canonical");
            }
        }
        for clobber in &asm.clobbers {
            if !Self::is_inline_asm_register_name(clobber) {
                self.invalid_inline_asm(span, "clobber name contains constraint syntax");
            }
        }
    }

    fn is_inline_asm_operand_type(&self, ty: nia_ids::InternedTyId) -> bool {
        match self.ty_kind(ty) {
            Some(TyKind::Primitive(primitive)) => *primitive != PrimitiveTy::Never,
            Some(
                TyKind::Pointer { .. }
                | TyKind::VolatilePointer { .. }
                | TyKind::FunctionPointer { .. },
            ) => true,
            // A payload-free enum lowers to its integer tag. Payload enums and
            // all other aggregates would ask LLVM inline assembly to carry a
            // struct value directly, which its constraint interface forbids.
            Some(TyKind::Nominal { def_id, .. }) => {
                self.index.enum_item(*def_id).is_some_and(|item| {
                    item.variants.iter().all(|variant| {
                        matches!(
                            variant.payload,
                            nia_backend_ir::BackendEnumVariantPayload::Unit
                        )
                    })
                })
            }
            _ => false,
        }
    }

    fn is_inline_asm_input_constraint(constraint: &str) -> bool {
        matches!(constraint, "r" | "f")
            || constraint
                .strip_prefix('{')
                .and_then(|constraint| constraint.strip_suffix('}'))
                .is_some_and(Self::is_inline_asm_register_name)
    }

    fn is_inline_asm_output_constraint(constraint: &str) -> bool {
        constraint
            .strip_prefix('=')
            .is_some_and(Self::is_inline_asm_input_constraint)
    }

    fn is_inline_asm_register_name(name: &str) -> bool {
        !name.is_empty()
            && !name.chars().any(|character| {
                character.is_whitespace()
                    || character.is_control()
                    || matches!(character, '{' | '}' | ',')
            })
    }

    fn invalid_inline_asm(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR inline assembly has an invalid contract: {message}"),
        ));
    }

    fn invalid_function_value_contract(
        &mut self,
        span: Span,
        kind: &'static str,
        message: &'static str,
    ) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR {kind} value has an invalid signature contract: {message}"),
        ));
    }

    fn validate_method_call_signature(
        &mut self,
        args: &[FunctionExpr],
        result_ty: nia_ids::InternedTyId,
        receiver_kind: nia_ids::ReceiverKind,
        signature: &CallTargetSignature,
        span: Span,
    ) {
        let Some(target_receiver) = signature.params.first() else {
            self.invalid_call_contract(
                span,
                "method",
                "target signature has no receiver parameter",
            );
            return;
        };
        if target_receiver.receiver != Some(receiver_kind) {
            self.invalid_call_contract(
                span,
                "method",
                "receiver kind does not match target signature",
            );
        }
        let value_signature = CallTargetSignature {
            params: signature.params[1..].to_vec(),
            return_type: signature.return_type,
            is_variadic: signature.is_variadic,
        };
        self.validate_call_signature("method", args, &value_signature, result_ty, span);
    }

    fn validate_call_signature(
        &mut self,
        kind: &'static str,
        args: &[FunctionExpr],
        signature: &CallTargetSignature,
        result_ty: nia_ids::InternedTyId,
        span: Span,
    ) {
        let param_tys = signature
            .params
            .iter()
            .map(|param| param.passing_ty)
            .collect::<Vec<_>>();
        self.validate_typed_call_signature(TypedCallContract {
            kind,
            args,
            params: &param_tys,
            return_type: signature.return_type,
            is_variadic: signature.is_variadic,
            result_ty,
            span,
        });
    }

    fn validate_typed_call_signature(&mut self, call: TypedCallContract<'_>) {
        let TypedCallContract {
            kind,
            args,
            params,
            return_type,
            is_variadic,
            result_ty,
            span,
        } = call;
        if args.len() < params.len() || (!is_variadic && args.len() != params.len()) {
            self.invalid_call_contract(
                span,
                kind,
                "argument count does not match target signature",
            );
        }
        if args
            .iter()
            .zip(params)
            .any(|(arg, param)| !self.call_argument_type_matches(arg.ty, *param))
        {
            self.invalid_call_contract(span, kind, "argument type does not match target signature");
        }
        if !self.same_type(result_ty, return_type) {
            self.invalid_call_contract(span, kind, "result type does not match target signature");
        }
    }

    fn invalid_call_contract(&mut self, span: Span, kind: &'static str, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR {kind} call has an invalid ABI contract: {message}"),
        ));
    }

    fn call_argument_type_matches(
        &self,
        actual: nia_ids::InternedTyId,
        expected: nia_ids::InternedTyId,
    ) -> bool {
        if self.same_type(actual, expected) {
            return true;
        }
        let Some(TyKind::Pointer {
            is_readonly: expected_readonly,
            elem: expected_elem,
        }) = self.index.ty_kind(expected)
        else {
            return false;
        };
        match self.index.ty_kind(actual) {
            Some(TyKind::Pointer {
                is_readonly: actual_readonly,
                elem: actual_elem,
            }) => {
                (*expected_readonly || !*actual_readonly)
                    && self.same_type(*actual_elem, *expected_elem)
            }
            _ => self.same_type(actual, *expected_elem),
        }
    }

    fn validate_dynamic_trait_call(&mut self, call: DynamicTraitCallContract<'_>) {
        let DynamicTraitCallContract {
            object_ty,
            trait_id,
            method_id,
            trait_args,
            trait_const_args,
            slot,
            params,
            return_type,
            receiver_kind,
            receiver,
            args,
            result_ty,
            tracks_caller,
            span,
        } = call;
        if !matches!(
            self.index.ty_kind(object_ty),
            Some(TyKind::TraitObject { .. })
        ) {
            self.invalid_dynamic_trait_call(span, "object type is not a trait object");
        }
        if !self.same_type(receiver.ty, object_ty) {
            self.invalid_dynamic_trait_call(
                span,
                "receiver type does not match its trait-object type",
            );
        }
        for arg in trait_args {
            self.validate_type(*arg, span);
        }
        for arg in trait_const_args {
            self.validate_const_arg(arg, span);
        }
        if !self.same_type(result_ty, return_type) {
            self.invalid_dynamic_trait_call(
                span,
                "expression result type does not match its return metadata",
            );
        }
        if args.len() != params.len() {
            self.invalid_dynamic_trait_call(
                span,
                "argument count does not match its parameter metadata",
            );
        }
        if args
            .iter()
            .zip(params)
            .any(|(arg, param)| !self.call_argument_type_matches(arg.ty, *param))
        {
            self.invalid_dynamic_trait_call(
                span,
                "argument type does not match its parameter metadata",
            );
        }

        let Some(targets) = self.validate_dynamic_trait_slots(
            object_ty,
            VtableTraitInstance {
                trait_id,
                args: trait_args,
                const_args: trait_const_args,
            },
            method_id,
            slot,
            span,
        ) else {
            return;
        };
        for target in targets {
            if self.dynamic_trait_target_tracks_caller(&target) != Some(tracks_caller) {
                self.invalid_dynamic_trait_call(
                    span,
                    "caller metadata does not match the vtable target ABI",
                );
            }
            self.validate_dynamic_trait_target(&target, params, return_type, receiver_kind, span);
        }
    }

    fn dynamic_trait_target_tracks_caller(
        &self,
        target: &nia_backend_ir::BackendTraitObjectVtableFunction,
    ) -> Option<bool> {
        use nia_backend_ir::BackendFunctionAttribute::TrackCaller;

        match target {
            nia_backend_ir::BackendTraitObjectVtableFunction::Function(def_id) => self
                .index
                .function(*def_id)
                .map(|function| function.attributes.contains(&TrackCaller)),
            nia_backend_ir::BackendTraitObjectVtableFunction::FunctionInstance {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
            } => self
                .index
                .function_instance(*def_id, *arg_module_id, *self_arg, args, const_args)
                .map(|function| function.attributes.contains(&TrackCaller)),
        }
    }

    fn validate_dynamic_trait_target(
        &mut self,
        target: &nia_backend_ir::BackendTraitObjectVtableFunction,
        params: &[nia_ids::InternedTyId],
        return_type: nia_ids::InternedTyId,
        receiver_kind: nia_ids::ReceiverKind,
        span: Span,
    ) {
        let Some((target_params, target_return_type)) = self.dynamic_trait_target_signature(target)
        else {
            self.invalid_dynamic_trait_call(span, "vtable slot references a missing function");
            return;
        };
        let Some(target_receiver) = target_params.first() else {
            self.invalid_dynamic_trait_call(span, "vtable target has no receiver parameter");
            return;
        };
        if target_receiver.receiver != Some(receiver_kind) {
            self.invalid_dynamic_trait_call(
                span,
                "receiver kind does not match the vtable target signature",
            );
        }
        let target_value_params = &target_params[1..];
        if target_value_params.len() != params.len()
            || target_value_params
                .iter()
                .zip(params)
                .any(|(target, param)| !self.same_type(target.passing_ty, *param))
        {
            self.invalid_dynamic_trait_call(
                span,
                "parameter metadata does not match the vtable target signature",
            );
        }
        if !self.same_type(target_return_type, return_type) {
            self.invalid_dynamic_trait_call(
                span,
                "return metadata does not match the vtable target signature",
            );
        }
    }

    fn validate_dynamic_trait_slots(
        &mut self,
        object_ty: nia_ids::InternedTyId,
        trait_instance: VtableTraitInstance<'_>,
        method_id: nia_ids::GlobalDefId,
        slot: usize,
        span: Span,
    ) -> Option<Vec<nia_backend_ir::BackendTraitObjectVtableFunction>> {
        // The slot is part of the typed call contract, not merely an indexing
        // hint. Calls on the original object use absolute slots in its complete
        // table; explicitly upcast views use slots relative to the target
        // supertrait segment. Keep those two representations distinct so a
        // malformed slot cannot turn into an unchecked LLVM GEP. Every
        // concrete table is checked: selecting the first table would make ABI
        // validation depend on module publication order.
        let exact_vtables = self
            .index
            .trait_object_vtables_for_object_ty(object_ty)
            .filter(|vtable| self.same_type(vtable.key.object_ty, object_ty))
            .chain(
                self.index
                    .trait_object_vtables_for_trait(trait_instance.trait_id)
                    .filter(|vtable| self.same_type(vtable.key.object_ty, object_ty)),
            )
            .collect::<Vec<_>>();
        let mut targets = Vec::new();
        for vtable in &exact_vtables {
            let Some(entry) =
                self.dynamic_trait_slot_entry(vtable, trait_instance, method_id, slot)
            else {
                self.invalid_dynamic_trait_slot(span);
                return None;
            };
            if !targets.contains(&entry.function) {
                targets.push(entry.function.clone());
            }
        }

        // An explicitly upcast receiver names the target trait-object type but
        // retains a pointer into a source vtable. A direct table and one or
        // more such source tables can coexist, so both sets are runtime
        // candidates. The source-table offset is anchored at the object view's
        // principal trait segment, not the trait that happened to declare the
        // called method; otherwise calls to that principal trait's supertraits
        // would be validated against the wrong relative slot.
        let object_trait_instance = self.vtable_trait_instance_for_object_ty(object_ty)?;
        for vtable in self
            .index
            .trait_object_vtables()
            .filter(|vtable| !self.same_type(vtable.key.object_ty, object_ty))
        {
            let Some(first_slot) =
                self.first_vtable_slot_for_trait_instance(vtable, object_trait_instance)
            else {
                continue;
            };
            let Some(entry) = self.dynamic_trait_upcast_slot_entry(
                vtable,
                trait_instance,
                method_id,
                first_slot,
                slot,
            ) else {
                self.invalid_dynamic_trait_slot(span);
                return None;
            };
            if !targets.contains(&entry.function) {
                targets.push(entry.function.clone());
            }
        }
        // A dynamic call can consume a trait object supplied at runtime even
        // when this closed program never constructs a concrete object of that
        // type. Validate every materialized runtime candidate, but do not
        // require one merely to validate the call contract itself.
        Some(targets)
    }

    fn invalid_dynamic_trait_slot(&mut self, span: Span) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            "backend IR dynamic trait call has an invalid vtable method slot",
        ));
    }

    fn dynamic_trait_slot_entry<'a>(
        &self,
        vtable: &'a nia_backend_ir::BackendTraitObjectVtable,
        trait_instance: VtableTraitInstance<'_>,
        method_id: nia_ids::GlobalDefId,
        slot: usize,
    ) -> Option<&'a nia_backend_ir::BackendTraitObjectVtableEntry> {
        vtable.entries.iter().find(|entry| {
            self.vtable_entry_matches_trait_instance(entry, trait_instance)
                && entry.method_id == method_id
                && entry.slot == slot
        })
    }

    fn dynamic_trait_upcast_slot_entry<'a>(
        &self,
        vtable: &'a nia_backend_ir::BackendTraitObjectVtable,
        trait_instance: VtableTraitInstance<'_>,
        method_id: nia_ids::GlobalDefId,
        first_slot: usize,
        slot: usize,
    ) -> Option<&'a nia_backend_ir::BackendTraitObjectVtableEntry> {
        vtable.entries.iter().find(|entry| {
            self.vtable_entry_matches_trait_instance(entry, trait_instance)
                && entry.method_id == method_id
                && entry.slot.checked_sub(first_slot) == Some(slot)
        })
    }

    fn vtable_trait_instance_for_object_ty(
        &self,
        object_ty: nia_ids::InternedTyId,
    ) -> Option<VtableTraitInstance<'_>> {
        let TyKind::TraitObject {
            trait_id,
            trait_args,
            trait_const_args,
            ..
        } = self.index.ty_kind(object_ty)?
        else {
            return None;
        };
        Some(VtableTraitInstance {
            trait_id: *trait_id,
            args: trait_args,
            const_args: trait_const_args,
        })
    }

    fn first_vtable_slot_for_trait_instance(
        &self,
        vtable: &nia_backend_ir::BackendTraitObjectVtable,
        trait_instance: VtableTraitInstance<'_>,
    ) -> Option<usize> {
        vtable
            .entries
            .iter()
            .filter(|entry| self.vtable_entry_matches_trait_instance(entry, trait_instance))
            .map(|entry| entry.slot)
            .min()
    }

    fn vtable_entry_matches_trait_instance(
        &self,
        entry: &nia_backend_ir::BackendTraitObjectVtableEntry,
        trait_instance: VtableTraitInstance<'_>,
    ) -> bool {
        entry.trait_id == trait_instance.trait_id
            && self.same_type_args(&entry.trait_args, trait_instance.args)
            && self.same_const_args(&entry.trait_const_args, trait_instance.const_args)
    }

    fn dynamic_trait_target_signature(
        &self,
        target: &nia_backend_ir::BackendTraitObjectVtableFunction,
    ) -> Option<(Vec<nia_backend_ir::BackendParam>, nia_ids::InternedTyId)> {
        match target {
            nia_backend_ir::BackendTraitObjectVtableFunction::Function(def_id) => self
                .index
                .function(*def_id)
                .map(|function| (function.params.clone(), function.return_type)),
            nia_backend_ir::BackendTraitObjectVtableFunction::FunctionInstance {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
            } => self
                .index
                .function_instance(*def_id, *arg_module_id, *self_arg, args, const_args)
                .map(|function| (function.params.clone(), function.return_type)),
        }
    }

    fn invalid_dynamic_trait_call(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR dynamic trait call has an invalid ABI contract: {message}"),
        ));
    }
}
