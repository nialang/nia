// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

impl<'a> ModuleLowerer<'a> {
    pub(super) fn instantiate_defer_body(
        &mut self,
        body: FunctionDeferBody,
        substitutions: TypeSubstitutionId,
    ) -> FunctionDeferBody {
        FunctionDeferBody {
            span: body.span,
            scopes: body.scopes,
            blocks: body
                .blocks
                .into_iter()
                .map(|block| nia_function_ir::FunctionBlock {
                    id: block.id,
                    scope: block.scope,
                    span: block.span,
                    ops: block
                        .ops
                        .into_iter()
                        .map(|op| self.instantiate_op(op, substitutions))
                        .collect(),
                    terminator: self.instantiate_terminator(block.terminator, substitutions),
                })
                .collect(),
            entry: body.entry,
        }
    }

    pub(super) fn instantiate_op(
        &mut self,
        op: FunctionOp,
        substitutions: TypeSubstitutionId,
    ) -> FunctionOp {
        match op {
            FunctionOp::Binding(binding) => {
                FunctionOp::Binding(self.instantiate_binding(binding, substitutions))
            }
            FunctionOp::StoreLocal {
                local_id,
                value,
                span,
            } => FunctionOp::StoreLocal {
                local_id,
                value: self.instantiate_expr(value, substitutions),
                span,
            },
            FunctionOp::MemoryIntrinsic(memory) => {
                FunctionOp::MemoryIntrinsic(Box::new(nia_function_ir::FunctionMemoryIntrinsic {
                    span: memory.span,
                    op: memory.op,
                    elem_ty: self.instantiate_ty_with_id(memory.elem_ty, substitutions),
                    dest: self.instantiate_expr(memory.dest, substitutions),
                    source: match memory.source {
                        nia_function_ir::FunctionMemoryIntrinsicSource::Slice(source) => {
                            nia_function_ir::FunctionMemoryIntrinsicSource::Slice(
                                self.instantiate_expr(source, substitutions),
                            )
                        }
                        nia_function_ir::FunctionMemoryIntrinsicSource::Byte(value) => {
                            nia_function_ir::FunctionMemoryIntrinsicSource::Byte(
                                self.instantiate_expr(value, substitutions),
                            )
                        }
                    },
                }))
            }
            FunctionOp::Expr(expr) => FunctionOp::Expr(self.instantiate_expr(expr, substitutions)),
            FunctionOp::Defer(body) => {
                FunctionOp::Defer(self.instantiate_defer_body(body, substitutions))
            }
        }
    }

    pub(super) fn instantiate_binding(
        &mut self,
        binding: FunctionBinding,
        substitutions: TypeSubstitutionId,
    ) -> FunctionBinding {
        FunctionBinding {
            local_id: binding.local_id,
            name: binding.name,
            ty: self.instantiate_ty_with_id(binding.ty, substitutions),
            value: binding
                .value
                .map(|value| self.instantiate_expr(value, substitutions)),
            is_let: binding.is_let,
        }
    }

    pub(super) fn instantiate_terminator(
        &mut self,
        terminator: FunctionTerminator,
        substitutions: TypeSubstitutionId,
    ) -> FunctionTerminator {
        match terminator {
            FunctionTerminator::Error { span } => FunctionTerminator::Error { span },
            FunctionTerminator::Branch { target, span } => {
                FunctionTerminator::Branch { target, span }
            }
            FunctionTerminator::Next { target, span } => FunctionTerminator::Next { target, span },
            FunctionTerminator::If {
                cond,
                then_target,
                else_target,
                span,
            } => FunctionTerminator::If {
                cond: self.instantiate_expr(cond, substitutions),
                then_target,
                else_target,
                span,
            },
            FunctionTerminator::Switch {
                target,
                arms,
                default,
                fallback,
                span,
            } => FunctionTerminator::Switch {
                target: self.instantiate_expr(target, substitutions),
                arms: arms
                    .into_iter()
                    .map(|arm| nia_function_ir::FunctionSwitchArm {
                        pattern: self.instantiate_expr(arm.pattern, substitutions),
                        target: arm.target,
                    })
                    .collect(),
                default,
                fallback,
                span,
            },
            FunctionTerminator::Try {
                value,
                kind,
                error_conversion,
                success_local,
                success_target,
                span,
            } => FunctionTerminator::Try {
                value: self.instantiate_expr(value, substitutions),
                kind,
                error_conversion: error_conversion
                    .map(|conversion| Box::new(self.instantiate_expr(*conversion, substitutions))),
                success_local,
                success_target,
                span,
            },
            FunctionTerminator::Loop {
                header,
                body,
                continue_target,
                break_target,
                span,
            } => FunctionTerminator::Loop {
                header: self.instantiate_for_header(header, substitutions),
                body,
                continue_target,
                break_target,
                span,
            },
            FunctionTerminator::Return { value, span } => FunctionTerminator::Return {
                value: value.map(|value| self.instantiate_expr(value, substitutions)),
                span,
            },
            FunctionTerminator::Tail { value, span } => FunctionTerminator::Tail {
                value: value.map(|value| self.instantiate_expr(value, substitutions)),
                span,
            },
        }
    }

    pub(super) fn instantiate_for_header(
        &mut self,
        header: FunctionForHeader,
        substitutions: TypeSubstitutionId,
    ) -> FunctionForHeader {
        match header {
            FunctionForHeader::Infinite => FunctionForHeader::Infinite,
            FunctionForHeader::Condition(expr) => {
                FunctionForHeader::Condition(Box::new(self.instantiate_expr(*expr, substitutions)))
            }
        }
    }

    pub(super) fn instantiate_expr(
        &mut self,
        expr: FunctionExpr,
        substitutions: TypeSubstitutionId,
    ) -> FunctionExpr {
        let span = expr.span;
        let ty = self.instantiate_ty_with_id(expr.ty, substitutions);
        FunctionExpr {
            span,
            ty,
            kind: match expr.kind {
                FunctionExprKind::Error => {
                    crate::input::unreachable_invalid_function_ir("FunctionExprKind::Error")
                }
                FunctionExprKind::Integer(text) => FunctionExprKind::Integer(text),
                FunctionExprKind::Float(text) => FunctionExprKind::Float(text),
                FunctionExprKind::String(scalars) => FunctionExprKind::String(scalars),
                FunctionExprKind::ByteString(bytes) => FunctionExprKind::ByteString(bytes),
                FunctionExprKind::Char(value) => FunctionExprKind::Char(value),
                FunctionExprKind::ByteChar(text) => FunctionExprKind::ByteChar(text),
                FunctionExprKind::Bool(value) => FunctionExprKind::Bool(value),
                FunctionExprKind::Null => FunctionExprKind::Null,
                FunctionExprKind::Local(local) => FunctionExprKind::Local(local),
                FunctionExprKind::ConstGeneric(arg) => {
                    let arg = self.instantiate_const_generic_arg(&arg, substitutions);
                    self.const_generic_expr_from_arg(arg)
                }
                FunctionExprKind::Global(def_id) => {
                    if let Some((arg_module_id, args, const_args)) =
                        self.global_instance_args_for_def(def_id, substitutions)
                    {
                        FunctionExprKind::GlobalInstance {
                            def_id,
                            arg_module_id,
                            args,
                            const_args,
                        }
                    } else {
                        FunctionExprKind::Global(def_id)
                    }
                }
                FunctionExprKind::GlobalInstance {
                    def_id,
                    arg_module_id: _,
                    args,
                    const_args,
                } => FunctionExprKind::GlobalInstance {
                    def_id,
                    arg_module_id: self.current_arg_module_id(),
                    args: {
                        let instantiated_args = args
                            .into_iter()
                            .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
                            .collect::<Vec<_>>();
                        self.canonicalize_instance_args(&instantiated_args)
                    },
                    const_args: const_args
                        .into_iter()
                        .map(|arg| {
                            self.instantiate_const_generic_arg_with_id(
                                &arg,
                                substitutions,
                                &mut Vec::new(),
                            )
                        })
                        .collect(),
                },
                FunctionExprKind::Function(def_id) => {
                    let args = self.effective_instance_type_args_for_def(def_id, substitutions);
                    let const_args =
                        self.effective_instance_const_args_for_def(def_id, substitutions);
                    if let (Some(args), Some(const_args)) = (args, const_args)
                        && (!args.is_empty() || !const_args.is_empty())
                    {
                        FunctionExprKind::FunctionInstance {
                            def_id,
                            arg_module_id: self.current_arg_module_id(),
                            self_arg: None,
                            args,
                            const_args,
                        }
                    } else {
                        FunctionExprKind::Function(def_id)
                    }
                }
                FunctionExprKind::FunctionInstance {
                    def_id,
                    arg_module_id: _,
                    self_arg,
                    args,
                    const_args,
                } => {
                    let self_arg = self_arg
                        .map(|self_arg| self.instantiate_ty_with_id(self_arg, substitutions));
                    let args = args
                        .into_iter()
                        .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
                        .collect::<Vec<_>>();
                    FunctionExprKind::FunctionInstance {
                        def_id,
                        arg_module_id: self.current_arg_module_id(),
                        self_arg: self.canonicalize_instance_self_arg(self_arg),
                        args: self.canonicalize_instance_args(&args),
                        const_args: const_args
                            .into_iter()
                            .map(|arg| {
                                self.instantiate_const_generic_arg_with_id(
                                    &arg,
                                    substitutions,
                                    &mut Vec::new(),
                                )
                            })
                            .collect(),
                    }
                }
                FunctionExprKind::EnumVariant { variant, fields } => {
                    FunctionExprKind::EnumVariant {
                        variant,
                        fields: fields
                            .into_iter()
                            .map(|field| self.instantiate_expr(field, substitutions))
                            .collect(),
                    }
                }
                FunctionExprKind::EnumVariantTag(variant) => {
                    FunctionExprKind::EnumVariantTag(variant)
                }
                FunctionExprKind::EnumTag { value } => FunctionExprKind::EnumTag {
                    value: Box::new(self.instantiate_expr(*value, substitutions)),
                },
                FunctionExprKind::EnumPayloadField {
                    value,
                    variant,
                    field,
                } => FunctionExprKind::EnumPayloadField {
                    value: Box::new(self.instantiate_expr(*value, substitutions)),
                    variant,
                    field,
                },
                FunctionExprKind::BuiltinValue(value) => FunctionExprKind::BuiltinValue(
                    self.instantiate_builtin_value(value, substitutions),
                ),
                FunctionExprKind::Trap => FunctionExprKind::Trap,
                FunctionExprKind::Range(range) => {
                    FunctionExprKind::Range(self.instantiate_range(range, substitutions))
                }
                FunctionExprKind::RangeBound { range, bound } => FunctionExprKind::RangeBound {
                    range: Box::new(self.instantiate_expr(*range, substitutions)),
                    bound,
                },
                FunctionExprKind::InlineAsm(asm) => {
                    FunctionExprKind::InlineAsm(FunctionInlineAsm {
                        code: asm.code,
                        inputs: asm
                            .inputs
                            .into_iter()
                            .map(|input| FunctionAsmInput {
                                constraint: input.constraint,
                                value: self.instantiate_expr(input.value, substitutions),
                                span: input.span,
                            })
                            .collect(),
                        outputs: asm
                            .outputs
                            .into_iter()
                            .map(|output| FunctionAsmOutput {
                                constraint: output.constraint,
                                place: self.instantiate_place(output.place, substitutions),
                                span: output.span,
                            })
                            .collect(),
                        clobbers: asm.clobbers,
                        options: asm.options,
                    })
                }
                FunctionExprKind::Atomic(atomic) => {
                    FunctionExprKind::Atomic(self.instantiate_atomic(atomic, substitutions))
                }
                FunctionExprKind::StaticArrayPointer {
                    allocation,
                    array,
                    is_readonly,
                } => FunctionExprKind::StaticArrayPointer {
                    allocation,
                    array: Box::new(self.instantiate_expr(*array, substitutions)),
                    is_readonly,
                },
                FunctionExprKind::ArrayLiteral { elems } => FunctionExprKind::ArrayLiteral {
                    elems: self.instantiate_array_elements(elems, substitutions),
                },
                FunctionExprKind::Tuple(elems) => FunctionExprKind::Tuple(
                    elems
                        .into_iter()
                        .map(|elem| self.instantiate_expr(elem, substitutions))
                        .collect(),
                ),
                FunctionExprKind::TupleField { value, index } => FunctionExprKind::TupleField {
                    value: Box::new(self.instantiate_expr(*value, substitutions)),
                    index,
                },
                FunctionExprKind::StructLiteral { def_id, fields } => {
                    FunctionExprKind::StructLiteral {
                        def_id,
                        fields: fields
                            .into_iter()
                            .map(|field| FunctionFieldInit {
                                field: field.field,
                                name: field.name,
                                value: self.instantiate_expr(field.value, substitutions),
                                span: field.span,
                            })
                            .collect(),
                    }
                }
                FunctionExprKind::UnionLiteral { def_id, field } => {
                    FunctionExprKind::UnionLiteral {
                        def_id,
                        field: Box::new(FunctionFieldInit {
                            field: field.field,
                            name: field.name,
                            value: self.instantiate_expr(field.value, substitutions),
                            span: field.span,
                        }),
                    }
                }
                FunctionExprKind::UnionStorageLiteral { bytes, relocations } => {
                    FunctionExprKind::UnionStorageLiteral {
                        bytes,
                        relocations: relocations
                            .into_iter()
                            .map(|relocation| nia_function_ir::FunctionUnionRelocation {
                                offset: relocation.offset,
                                width: relocation.width,
                                allocation: relocation.allocation,
                                pointee: Box::new(
                                    self.instantiate_expr(*relocation.pointee, substitutions),
                                ),
                            })
                            .collect(),
                    }
                }
                FunctionExprKind::Unary { op, expr } => FunctionExprKind::Unary {
                    op,
                    expr: Box::new(self.instantiate_expr(*expr, substitutions)),
                },
                FunctionExprKind::OptionalSome { expr } => FunctionExprKind::OptionalSome {
                    expr: Box::new(self.instantiate_expr(*expr, substitutions)),
                },
                FunctionExprKind::ErrorOk { expr } => FunctionExprKind::ErrorOk {
                    expr: Box::new(self.instantiate_expr(*expr, substitutions)),
                },
                FunctionExprKind::ErrorErr { expr } => FunctionExprKind::ErrorErr {
                    expr: Box::new(self.instantiate_expr(*expr, substitutions)),
                },
                FunctionExprKind::TaggedUnionTag { expr } => FunctionExprKind::TaggedUnionTag {
                    expr: Box::new(self.instantiate_expr(*expr, substitutions)),
                },
                FunctionExprKind::TaggedUnionPayload { expr } => {
                    FunctionExprKind::TaggedUnionPayload {
                        expr: Box::new(self.instantiate_expr(*expr, substitutions)),
                    }
                }
                FunctionExprKind::Try { expr } => FunctionExprKind::Try {
                    expr: Box::new(self.instantiate_expr(*expr, substitutions)),
                },
                FunctionExprKind::LoadUnaligned { ty, ptr } => FunctionExprKind::LoadUnaligned {
                    ty: self.instantiate_ty_with_id(ty, substitutions),
                    ptr: Box::new(self.instantiate_expr(*ptr, substitutions)),
                },
                FunctionExprKind::Splat { value } => FunctionExprKind::Splat {
                    value: Box::new(self.instantiate_expr(*value, substitutions)),
                },
                FunctionExprKind::Bitmask { vector } => FunctionExprKind::Bitmask {
                    vector: Box::new(self.instantiate_expr(*vector, substitutions)),
                },
                FunctionExprKind::BitIntrinsic { op, value } => FunctionExprKind::BitIntrinsic {
                    op,
                    value: Box::new(self.instantiate_expr(*value, substitutions)),
                },
                FunctionExprKind::CharFromU32 { value } => FunctionExprKind::CharFromU32 {
                    value: Box::new(self.instantiate_expr(*value, substitutions)),
                },
                FunctionExprKind::ExtractElement { vector, index } => {
                    FunctionExprKind::ExtractElement {
                        vector: Box::new(self.instantiate_expr(*vector, substitutions)),
                        index: Box::new(self.instantiate_expr(*index, substitutions)),
                    }
                }
                FunctionExprKind::InsertElement {
                    vector,
                    index,
                    value,
                } => FunctionExprKind::InsertElement {
                    vector: Box::new(self.instantiate_expr(*vector, substitutions)),
                    index: Box::new(self.instantiate_expr(*index, substitutions)),
                    value: Box::new(self.instantiate_expr(*value, substitutions)),
                },
                FunctionExprKind::AddrOf(place) => {
                    FunctionExprKind::AddrOf(self.instantiate_place(place, substitutions))
                }
                FunctionExprKind::Binary { lhs, op, rhs } => FunctionExprKind::Binary {
                    lhs: Box::new(self.instantiate_expr(*lhs, substitutions)),
                    op,
                    rhs: Box::new(self.instantiate_expr(*rhs, substitutions)),
                },
                FunctionExprKind::Assign { place, op, rhs } => FunctionExprKind::Assign {
                    place: self.instantiate_place(place, substitutions),
                    op,
                    rhs: Box::new(self.instantiate_expr(*rhs, substitutions)),
                },
                FunctionExprKind::Discard(expr) => {
                    FunctionExprKind::Discard(Box::new(self.instantiate_expr(*expr, substitutions)))
                }
                FunctionExprKind::Cast { expr, ty } => FunctionExprKind::Cast {
                    expr: Box::new(self.instantiate_expr(*expr, substitutions)),
                    ty: self.instantiate_ty_with_id(ty, substitutions),
                },
                FunctionExprKind::TraitObjectUpcast {
                    expr,
                    source_ty,
                    target_ty,
                } => FunctionExprKind::TraitObjectUpcast {
                    expr: Box::new(self.instantiate_expr(*expr, substitutions)),
                    source_ty: self.instantiate_ty_with_id(source_ty, substitutions),
                    target_ty: self.instantiate_ty_with_id(target_ty, substitutions),
                },
                FunctionExprKind::TraitObjectCoercion {
                    expr,
                    target_ty,
                    self_ty,
                } => FunctionExprKind::TraitObjectCoercion {
                    expr: Box::new(self.instantiate_expr(*expr, substitutions)),
                    target_ty: self.instantiate_ty_with_id(target_ty, substitutions),
                    self_ty: self.instantiate_ty_with_id(self_ty, substitutions),
                },
                FunctionExprKind::CallableCoercion { state, closure_id } => {
                    FunctionExprKind::CallableCoercion {
                        state: Box::new(self.instantiate_expr(*state, substitutions)),
                        closure_id,
                    }
                }
                FunctionExprKind::ClosureFunctionPointer { closure_id } => {
                    FunctionExprKind::ClosureFunctionPointer { closure_id }
                }
                FunctionExprKind::Call { callee, args } => {
                    let args = args
                        .into_iter()
                        .map(|arg| self.instantiate_expr(arg, substitutions))
                        .collect::<Vec<_>>();
                    let callee = self.instantiate_callee(callee, substitutions);
                    if let FunctionCallee::BuiltinPlaceMethod {
                        trait_id,
                        method,
                        self_ty,
                        trait_args,
                        receiver,
                        ..
                    } = &callee
                        && let Some(intrinsic_expr) = self
                            .lower_intrinsic_builtin_place_method_call(
                                *trait_id,
                                *method,
                                *self_ty,
                                trait_args,
                                receiver.as_ref().clone(),
                                &args,
                            )
                    {
                        return intrinsic_expr;
                    }
                    if let FunctionCallee::BuiltinPlaceMethod {
                        trait_id,
                        method,
                        self_ty,
                        trait_args,
                        receiver,
                    } = callee
                    {
                        let resolution =
                            self.resolve_builtin_trait_goal(self_ty, trait_id, trait_args.clone());
                        match resolution {
                            TraitResolution::User(_) => {
                                let resolved = self.resolve_builtin_place_method_impl(
                                    trait_id,
                                    &trait_args,
                                    method,
                                    self_ty,
                                );
                                if let Some((def_id, target_args)) = resolved {
                                    return FunctionExpr {
                                        span,
                                        ty,
                                        kind: FunctionExprKind::Call {
                                            callee: FunctionCallee::Method {
                                                def_id,
                                                arg_module_id: self.current_arg_module_id(),
                                                self_arg: None,
                                                args: target_args,
                                                const_args: Vec::new(),
                                                receiver_kind: self
                                                    .receiver_kind_for_method_or_diagnose(
                                                        def_id,
                                                        receiver.span,
                                                    ),
                                                receiver,
                                            },
                                            args,
                                        },
                                    };
                                }
                            }
                            TraitResolution::Assumed(_) => {
                                return FunctionExpr {
                                    span,
                                    ty,
                                    kind: FunctionExprKind::Call {
                                        callee: FunctionCallee::BuiltinPlaceMethod {
                                            trait_id,
                                            method,
                                            self_ty,
                                            trait_args,
                                            receiver,
                                        },
                                        args,
                                    },
                                };
                            }
                            TraitResolution::Intrinsic(_) => {
                                return FunctionExpr {
                                    span,
                                    ty,
                                    kind: FunctionExprKind::Call {
                                        callee: FunctionCallee::BuiltinPlaceMethod {
                                            trait_id,
                                            method,
                                            self_ty,
                                            trait_args,
                                            receiver,
                                        },
                                        args,
                                    },
                                };
                            }
                            TraitResolution::Unsatisfied | TraitResolution::Ambiguous => {}
                        }
                        if self.builtin_trait_method_call_requires_concrete_impl(
                            self_ty,
                            trait_id,
                            &trait_args,
                            &[],
                        ) {
                            self.diagnostics
                                .push(nia_diagnostic::Diagnostic::user_error_at(
                                    nia_diagnostic::codes::LLVM_CODEGEN,
                                    receiver.span,
                                    "no visible implementation found for builtin place method call",
                                ));
                        }
                        return FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Call {
                                callee: FunctionCallee::BuiltinPlaceMethod {
                                    trait_id,
                                    method,
                                    self_ty,
                                    trait_args,
                                    receiver,
                                },
                                args,
                            },
                        };
                    }
                    FunctionExprKind::Call { callee, args }
                }
                FunctionExprKind::Field { lhs, field } => FunctionExprKind::Field {
                    lhs: Box::new(self.instantiate_expr(*lhs, substitutions)),
                    field,
                },
                FunctionExprKind::Index { lhs, index } => FunctionExprKind::Index {
                    lhs: Box::new(self.instantiate_expr(*lhs, substitutions)),
                    index: Box::new(self.instantiate_expr(*index, substitutions)),
                },
                FunctionExprKind::Slice {
                    lhs,
                    range,
                    is_readonly,
                } => FunctionExprKind::Slice {
                    lhs: Box::new(self.instantiate_expr(*lhs, substitutions)),
                    range: self.instantiate_slice_range(range, substitutions),
                    is_readonly,
                },
            },
        }
    }

    fn instantiate_atomic(
        &mut self,
        atomic: nia_function_ir::FunctionAtomic,
        substitutions: TypeSubstitutionId,
    ) -> nia_function_ir::FunctionAtomic {
        match atomic {
            nia_function_ir::FunctionAtomic::Load { ty, ptr, order } => {
                nia_function_ir::FunctionAtomic::Load {
                    ty: self.instantiate_ty_with_id(ty, substitutions),
                    ptr: Box::new(self.instantiate_expr(*ptr, substitutions)),
                    order,
                }
            }
            nia_function_ir::FunctionAtomic::Store {
                ty,
                ptr,
                value,
                order,
            } => nia_function_ir::FunctionAtomic::Store {
                ty: self.instantiate_ty_with_id(ty, substitutions),
                ptr: Box::new(self.instantiate_expr(*ptr, substitutions)),
                value: Box::new(self.instantiate_expr(*value, substitutions)),
                order,
            },
            nia_function_ir::FunctionAtomic::Rmw {
                ty,
                ptr,
                op,
                value,
                order,
            } => nia_function_ir::FunctionAtomic::Rmw {
                ty: self.instantiate_ty_with_id(ty, substitutions),
                ptr: Box::new(self.instantiate_expr(*ptr, substitutions)),
                op,
                value: Box::new(self.instantiate_expr(*value, substitutions)),
                order,
            },
            nia_function_ir::FunctionAtomic::Cmpxchg {
                ty,
                ptr,
                expected,
                desired,
                success,
                failure,
                weak,
            } => nia_function_ir::FunctionAtomic::Cmpxchg {
                ty: self.instantiate_ty_with_id(ty, substitutions),
                ptr: Box::new(self.instantiate_expr(*ptr, substitutions)),
                expected: Box::new(self.instantiate_expr(*expected, substitutions)),
                desired: Box::new(self.instantiate_expr(*desired, substitutions)),
                success,
                failure,
                weak,
            },
            nia_function_ir::FunctionAtomic::Fence { order } => {
                nia_function_ir::FunctionAtomic::Fence { order }
            }
        }
    }

    pub(super) fn instantiate_callee(
        &mut self,
        callee: FunctionCallee,
        substitutions: TypeSubstitutionId,
    ) -> FunctionCallee {
        match callee {
            FunctionCallee::ClosureEntry { closure_id, state } => FunctionCallee::ClosureEntry {
                closure_id,
                state: Box::new(self.instantiate_expr(*state, substitutions)),
            },
            FunctionCallee::Function(def_id) => {
                let args = self.effective_instance_type_args_for_def(def_id, substitutions);
                let const_args = self.effective_instance_const_args_for_def(def_id, substitutions);
                if let (Some(args), Some(const_args)) = (args, const_args)
                    && (!args.is_empty() || !const_args.is_empty())
                {
                    FunctionCallee::FunctionInstance {
                        def_id,
                        arg_module_id: self.current_arg_module_id(),
                        self_arg: None,
                        args,
                        const_args,
                    }
                } else {
                    FunctionCallee::Function(def_id)
                }
            }
            FunctionCallee::FunctionInstance {
                def_id,
                arg_module_id: _,
                self_arg,
                args,
                const_args,
            } => {
                let self_arg =
                    self_arg.map(|self_arg| self.instantiate_ty_with_id(self_arg, substitutions));
                let args = args
                    .into_iter()
                    .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
                    .collect::<Vec<_>>();
                let const_args = const_args
                    .iter()
                    .map(|arg| {
                        self.instantiate_const_generic_arg_with_id(
                            arg,
                            substitutions,
                            &mut Vec::new(),
                        )
                    })
                    .collect::<Vec<_>>();
                FunctionCallee::FunctionInstance {
                    def_id,
                    arg_module_id: self.current_arg_module_id(),
                    self_arg: self.canonicalize_instance_self_arg(self_arg),
                    args: self.canonicalize_instance_args(&args),
                    const_args: const_args
                        .into_iter()
                        .map(|arg| {
                            self.instantiate_const_generic_arg_with_id(
                                &arg,
                                substitutions,
                                &mut Vec::new(),
                            )
                        })
                        .collect(),
                }
            }
            FunctionCallee::Method {
                def_id,
                arg_module_id: _,
                self_arg,
                args,
                const_args,
                receiver_kind,
                receiver,
            } => {
                let self_arg =
                    self_arg.map(|self_arg| self.instantiate_ty_with_id(self_arg, substitutions));
                let args = args
                    .into_iter()
                    .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
                    .collect::<Vec<_>>();
                let args = if args.is_empty() {
                    self.effective_instance_type_args_for_def(def_id, substitutions)
                        .unwrap_or_default()
                } else {
                    self.canonicalize_instance_args(&args)
                };
                let const_args = const_args
                    .into_iter()
                    .map(|arg| {
                        self.instantiate_const_generic_arg_with_id(
                            &arg,
                            substitutions,
                            &mut Vec::new(),
                        )
                    })
                    .collect();
                FunctionCallee::Method {
                    def_id,
                    arg_module_id: self.current_arg_module_id(),
                    self_arg: self.canonicalize_instance_self_arg(self_arg),
                    args,
                    const_args,
                    receiver_kind,
                    receiver: Box::new(self.instantiate_expr(*receiver, substitutions)),
                }
            }
            FunctionCallee::TraitMethod {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                trait_const_args,
                args,
                const_args,
                receiver_kind,
                receiver,
            } => {
                let self_ty = self.instantiate_ty_with_id(self_ty, substitutions);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
                    .collect::<Vec<_>>();
                let trait_const_args = trait_const_args
                    .iter()
                    .map(|arg| {
                        self.instantiate_const_generic_arg_with_id(
                            arg,
                            substitutions,
                            &mut Vec::new(),
                        )
                    })
                    .collect::<Vec<_>>();
                let args = args
                    .into_iter()
                    .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
                    .collect::<Vec<_>>();
                let receiver = Box::new(self.instantiate_expr(*receiver, substitutions));
                if let Some((def_id, target_args, target_const_args)) = self
                    .resolve_trait_method_impl(
                        trait_id,
                        &trait_args,
                        &trait_const_args,
                        method_id,
                        &method_name,
                        self_ty,
                    )
                {
                    let mut instance_args = target_args;
                    instance_args.extend(args);
                    let mut instance_const_args = target_const_args;
                    instance_const_args.extend(const_args);
                    FunctionCallee::Method {
                        def_id,
                        arg_module_id: self.current_arg_module_id(),
                        self_arg: None,
                        args: instance_args,
                        const_args: instance_const_args,
                        receiver_kind,
                        receiver,
                    }
                } else if self.trait_method_has_default(method_id)
                    && self.trait_method_call_is_concrete(
                        self_ty,
                        &trait_args,
                        &trait_const_args,
                        &args,
                    )
                {
                    let default_self_ty = self.default_trait_method_self_arg(
                        trait_id,
                        &trait_args,
                        &trait_const_args,
                        self_ty,
                    );
                    let mut instance_args = trait_args.clone();
                    instance_args.extend(args);
                    let mut instance_const_args = trait_const_args.clone();
                    instance_const_args.extend(const_args);
                    FunctionCallee::Method {
                        def_id: method_id,
                        arg_module_id: self.current_arg_module_id(),
                        self_arg: Some(default_self_ty),
                        args: instance_args,
                        const_args: instance_const_args,
                        receiver_kind,
                        receiver,
                    }
                } else {
                    if self.trait_method_call_requires_concrete_impl(
                        self_ty,
                        trait_id,
                        &trait_args,
                        &trait_const_args,
                        &args,
                    ) {
                        let method_name = self.symbol_name(method_name);
                        self.diagnostics
                            .push(nia_diagnostic::Diagnostic::user_error(nia_diagnostic::codes::LLVM_CODEGEN,
                                format!(
                                    "no visible implementation found for trait method call `{method_name}`"
                                ),
                            )
                            .primary(receiver.span, format!("no implementation matched `{method_name}` for this receiver"))
                            .debug("trait_id", trait_id)
                            .finish());
                    }
                    FunctionCallee::TraitMethod {
                        trait_id,
                        method_id,
                        method_name,
                        self_ty,
                        trait_args,
                        trait_const_args,
                        args,
                        const_args,
                        receiver_kind,
                        receiver,
                    }
                }
            }
            FunctionCallee::TraitAssociatedFunction {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                trait_const_args,
                args,
                const_args,
            } => {
                let self_ty = self.instantiate_ty_with_id(self_ty, substitutions);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
                    .collect::<Vec<_>>();
                let trait_const_args = trait_const_args
                    .iter()
                    .map(|arg| {
                        self.instantiate_const_generic_arg_with_id(
                            arg,
                            substitutions,
                            &mut Vec::new(),
                        )
                    })
                    .collect::<Vec<_>>();
                let args = args
                    .into_iter()
                    .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
                    .collect::<Vec<_>>();
                let const_args = const_args
                    .iter()
                    .map(|arg| {
                        self.instantiate_const_generic_arg_with_id(
                            arg,
                            substitutions,
                            &mut Vec::new(),
                        )
                    })
                    .collect::<Vec<_>>();
                if let Some((def_id, target_args, target_const_args)) = self
                    .resolve_trait_method_impl(
                        trait_id,
                        &trait_args,
                        &trait_const_args,
                        method_id,
                        &method_name,
                        self_ty,
                    )
                {
                    let mut instance_args = target_args;
                    instance_args.extend(args);
                    let mut instance_const_args = target_const_args;
                    instance_const_args.extend(const_args);
                    if instance_args.is_empty() && instance_const_args.is_empty() {
                        FunctionCallee::Function(def_id)
                    } else {
                        FunctionCallee::FunctionInstance {
                            def_id,
                            arg_module_id: self.current_arg_module_id(),
                            self_arg: None,
                            args: instance_args,
                            const_args: instance_const_args,
                        }
                    }
                } else if self.trait_method_has_default(method_id)
                    && self.trait_method_call_is_concrete(
                        self_ty,
                        &trait_args,
                        &trait_const_args,
                        &args,
                    )
                {
                    let default_self_ty = self.default_trait_method_self_arg(
                        trait_id,
                        &trait_args,
                        &trait_const_args,
                        self_ty,
                    );
                    let mut instance_args = trait_args.clone();
                    instance_args.extend(args);
                    let mut instance_const_args = trait_const_args.clone();
                    instance_const_args.extend(const_args);
                    FunctionCallee::FunctionInstance {
                        def_id: method_id,
                        arg_module_id: self.current_arg_module_id(),
                        self_arg: Some(default_self_ty),
                        args: instance_args,
                        const_args: instance_const_args,
                    }
                } else {
                    if self.trait_method_call_requires_concrete_impl(
                        self_ty,
                        trait_id,
                        &trait_args,
                        &trait_const_args,
                        &args,
                    ) {
                        let method_name = self.symbol_name(method_name);
                        self.diagnostics
                            .push(nia_diagnostic::Diagnostic::user_error(nia_diagnostic::codes::LLVM_CODEGEN,
                                format!(
                                    "no visible implementation found for trait associated function call `{method_name}`"
                                ),
                            )
                            .finish());
                    }
                    FunctionCallee::TraitAssociatedFunction {
                        trait_id,
                        method_id,
                        method_name,
                        self_ty,
                        trait_args,
                        trait_const_args,
                        args,
                        const_args,
                    }
                }
            }
            FunctionCallee::BuiltinPlaceMethod {
                trait_id,
                method,
                self_ty,
                trait_args,
                receiver,
            } => {
                let self_ty = self.instantiate_ty_with_id(self_ty, substitutions);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
                    .collect::<Vec<_>>();
                let receiver = Box::new(self.instantiate_expr(*receiver, substitutions));
                FunctionCallee::BuiltinPlaceMethod {
                    trait_id,
                    method,
                    self_ty,
                    trait_args,
                    receiver,
                }
            }
            FunctionCallee::BuiltinMethod {
                method,
                self_ty,
                receiver,
            } => FunctionCallee::BuiltinMethod {
                method,
                self_ty: self.instantiate_ty_with_id(self_ty, substitutions),
                receiver: Box::new(self.instantiate_expr(*receiver, substitutions)),
            },
            FunctionCallee::DynamicTraitMethod {
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
                receiver,
            } => FunctionCallee::DynamicTraitMethod {
                object_ty: self.instantiate_ty_with_id(object_ty, substitutions),
                trait_id,
                method_id,
                method_name,
                trait_args: trait_args
                    .into_iter()
                    .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
                    .collect(),
                trait_const_args: trait_const_args
                    .into_iter()
                    .map(|arg| {
                        self.instantiate_const_generic_arg_with_id(
                            &arg,
                            substitutions,
                            &mut Vec::new(),
                        )
                    })
                    .collect(),
                slot,
                params: params
                    .into_iter()
                    .map(|param| self.instantiate_ty_with_id(param, substitutions))
                    .collect(),
                return_type: self.instantiate_ty_with_id(return_type, substitutions),
                receiver_kind,
                receiver: Box::new(self.instantiate_expr(*receiver, substitutions)),
            },
            FunctionCallee::BuiltinOperator(operator) => FunctionCallee::BuiltinOperator(operator),
            FunctionCallee::Callable(expr) => {
                FunctionCallee::Callable(Box::new(self.instantiate_expr(*expr, substitutions)))
            }
            FunctionCallee::FunctionPointer(expr) => FunctionCallee::FunctionPointer(Box::new(
                self.instantiate_expr(*expr, substitutions),
            )),
        }
    }

    pub(super) fn instantiate_builtin_value(
        &mut self,
        value: nia_function_ir::FunctionBuiltinValue,
        substitutions: TypeSubstitutionId,
    ) -> nia_function_ir::FunctionBuiltinValue {
        match value {
            nia_function_ir::FunctionBuiltinValue::Layout { builtin, ty } => {
                let ty = self.instantiate_ty_with_id(ty, substitutions);
                if let Some(layout) = self.layout_of(ty) {
                    let value = match builtin {
                        LayoutBuiltin::Size => layout.size,
                        LayoutBuiltin::Align => layout.align,
                    };
                    nia_function_ir::FunctionBuiltinValue::Usize(value)
                } else {
                    nia_function_ir::FunctionBuiltinValue::Layout { builtin, ty }
                }
            }
            nia_function_ir::FunctionBuiltinValue::FieldOffset { ty, field } => {
                let ty = self.instantiate_ty_with_id(ty, substitutions);
                if let Some(offset) = self.field_offset(ty, field) {
                    nia_function_ir::FunctionBuiltinValue::Usize(offset)
                } else {
                    nia_function_ir::FunctionBuiltinValue::FieldOffset { ty, field }
                }
            }
            nia_function_ir::FunctionBuiltinValue::Usize(value) => {
                nia_function_ir::FunctionBuiltinValue::Usize(value)
            }
            nia_function_ir::FunctionBuiltinValue::Int(value) => {
                nia_function_ir::FunctionBuiltinValue::Int(value)
            }
        }
    }

    pub(crate) fn trait_method_has_default(&self, method_id: GlobalDefId) -> bool {
        self.trait_context
            .trait_methods_with_defaults
            .contains(&method_id)
            || self
                .shared
                .program_trait_methods_with_defaults
                .contains(&method_id)
    }

    pub(crate) fn resolve_trait_method_impl(
        &mut self,
        trait_id: GlobalDefId,
        trait_args: &[InternedTyId],
        trait_const_args: &[nia_ty::ConstGenericArg],
        trait_method_id: GlobalDefId,
        trait_method_name: &SymbolId,
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>, Vec<nia_ty::ConstGenericArg>)> {
        let self_ty = self.canonicalize_instance_arg(self_ty);
        let trait_args = self.canonicalize_instance_args(trait_args);
        let trait_const_args = trait_const_args
            .iter()
            .map(|arg| self.canonicalize_instance_const_arg(arg))
            .collect::<Vec<_>>();
        let trait_method_name = self
            .method_symbol_for_def(trait_method_id)
            .unwrap_or(*trait_method_name);
        let key = crate::ExtensionTraitMethodKey {
            trait_id: TraitId::Source(trait_id),
            method_name: trait_method_name,
            trait_arg_count: trait_args.len(),
            trait_const_arg_count: trait_const_args.len(),
        };
        if let Some(candidate) =
            self.current_impl_trait_method(&key, &trait_args, &trait_const_args, self_ty)
        {
            return Some((candidate.0, candidate.1, Vec::new()));
        }
        if let Some(candidate) =
            self.selected_user_trait_method_impl(&key, &trait_args, &trait_const_args, self_ty)
        {
            return Some(candidate);
        }
        if let Some(pointee) = self.pointer_elem_ty(self_ty)
            && let Some(candidate) =
                self.selected_user_trait_method_impl(&key, &trait_args, &trait_const_args, pointee)
        {
            return Some(candidate);
        }
        if let Some(candidate) =
            self.global_trait_method_impl_candidate(&key, &trait_args, &trait_const_args, self_ty)
        {
            return Some((candidate.0, candidate.1, Vec::new()));
        }
        if let Some(pointee) = self.pointer_elem_ty(self_ty)
            && let Some(candidate) = self.global_trait_method_impl_candidate(
                &key,
                &trait_args,
                &trait_const_args,
                pointee,
            )
        {
            return Some((candidate.0, candidate.1, Vec::new()));
        }
        let candidates = self.program_extension_trait_method_candidates(&key);
        let candidates = candidates
            .iter()
            .filter_map(|candidate| {
                self.trait_impl_method_for_candidate(
                    candidate,
                    &trait_args,
                    &trait_const_args,
                    self_ty,
                )
                .map(|resolved| (candidate, resolved))
            })
            .collect::<Vec<_>>();
        let candidates = self.unique_trait_impl_method_candidates(candidates);
        let candidates = self.filter_more_specific_trait_impl_method_candidates(candidates);
        match candidates.as_slice() {
            [(_, candidate)] => Some((candidate.0, candidate.1.clone(), Vec::new())),
            _ => {
                let pointee = self.pointer_elem_ty(self_ty)?;
                let candidates = self.program_extension_trait_method_candidates(&key);
                let candidates = candidates
                    .iter()
                    .filter_map(|candidate| {
                        self.trait_impl_method_for_candidate(
                            candidate,
                            &trait_args,
                            &trait_const_args,
                            pointee,
                        )
                        .map(|resolved| (candidate, resolved))
                    })
                    .collect::<Vec<_>>();
                let candidates = self.unique_trait_impl_method_candidates(candidates);
                let candidates = self.filter_more_specific_trait_impl_method_candidates(candidates);
                match candidates.as_slice() {
                    [(_, candidate)] => Some((candidate.0, candidate.1.clone(), Vec::new())),
                    _ => None,
                }
            }
        }
    }

    fn unique_trait_impl_method_candidates<'b>(
        &self,
        candidates: Vec<(
            &'b crate::ExtensionTraitMethodCandidate,
            (GlobalDefId, Vec<InternedTyId>),
        )>,
    ) -> Vec<(
        &'b crate::ExtensionTraitMethodCandidate,
        (GlobalDefId, Vec<InternedTyId>),
    )> {
        let mut unique = Vec::new();
        for candidate in candidates {
            if unique.iter().any(
                |existing: &(
                    &'b crate::ExtensionTraitMethodCandidate,
                    (GlobalDefId, Vec<InternedTyId>),
                )| existing.1 == candidate.1,
            ) {
                continue;
            }
            unique.push(candidate);
        }
        unique
    }

    fn filter_more_specific_trait_impl_method_candidates<'b>(
        &mut self,
        candidates: Vec<(
            &'b crate::ExtensionTraitMethodCandidate,
            (GlobalDefId, Vec<InternedTyId>),
        )>,
    ) -> Vec<(
        &'b crate::ExtensionTraitMethodCandidate,
        (GlobalDefId, Vec<InternedTyId>),
    )> {
        candidates
            .iter()
            .filter(|(candidate, _)| {
                !candidates.iter().any(|(other, _)| {
                    other.method_def_id != candidate.method_def_id
                        && self.extension_trait_method_candidate_more_specific(other, candidate)
                })
            })
            .cloned()
            .collect()
    }

    fn extension_trait_method_candidate_more_specific(
        &mut self,
        specific: &crate::ExtensionTraitMethodCandidate,
        general: &crate::ExtensionTraitMethodCandidate,
    ) -> bool {
        // Candidate buckets normally enforce these arities. Keep the
        // comparison total for stale or independently constructed products:
        // `zip` must never turn a proper prefix into a specialization proof.
        if !extension_trait_candidate_arities_match(specific, general) {
            return false;
        }
        let specific_target =
            self.normalized_type_from_module(specific.module_id, specific.target_ty);
        let general_target = self.normalized_type_from_module(general.module_id, general.target_ty);
        let target_subsumes = self.extension_pattern_subsumes(general_target, specific_target);
        let mut any_strict =
            self.extension_pattern_strictly_more_specific(specific_target, general_target);
        let args_subsume = specific.trait_args.iter().zip(&general.trait_args).all(
            |(specific_arg, general_arg)| {
                let specific_arg =
                    self.normalized_type_from_module(specific.module_id, *specific_arg);
                let general_arg = self.normalized_type_from_module(general.module_id, *general_arg);
                any_strict |=
                    self.extension_pattern_strictly_more_specific(specific_arg, general_arg);
                self.extension_pattern_subsumes(general_arg, specific_arg)
            },
        );
        let const_args_subsume = specific
            .trait_const_args
            .iter()
            .zip(&general.trait_const_args)
            .all(|(specific, general)| {
                let specific = self.canonicalize_instance_const_arg(specific);
                let general = self.canonicalize_instance_const_arg(general);
                matches!(general.value, nia_ty::ConstGenericValue::GenericParam(_))
                    || (self.types_match(specific.ty, general.ty)
                        && match (&specific.value, &general.value) {
                            (
                                nia_ty::ConstGenericValue::Int(specific),
                                nia_ty::ConstGenericValue::Int(general),
                            ) => specific.bits() == general.bits(),
                            (specific, general) => specific == general,
                        })
            });
        let const_strict = specific
            .trait_const_args
            .iter()
            .zip(&general.trait_const_args)
            .any(|(specific, general)| {
                matches!(general.value, nia_ty::ConstGenericValue::GenericParam(_))
                    && !matches!(specific.value, nia_ty::ConstGenericValue::GenericParam(_))
            });
        target_subsumes && args_subsume && const_args_subsume && (any_strict || const_strict)
    }

    fn extension_pattern_strictly_more_specific(
        &mut self,
        specific: InternedTyId,
        general: InternedTyId,
    ) -> bool {
        self.extension_pattern_subsumes(general, specific)
            && !self.extension_pattern_subsumes(specific, general)
    }

    fn extension_pattern_subsumes(
        &mut self,
        general: InternedTyId,
        specific: InternedTyId,
    ) -> bool {
        self.match_extension_type_pattern(general, specific, &mut SymbolMap::default())
    }

    pub(super) fn instantiate_place(
        &mut self,
        place: FunctionPlace,
        substitutions: TypeSubstitutionId,
    ) -> FunctionPlace {
        FunctionPlace {
            span: place.span,
            ty: self.instantiate_ty_with_id(place.ty, substitutions),
            base: match place.base {
                FunctionPlaceBase::Local(local_id) => FunctionPlaceBase::Local(local_id),
                FunctionPlaceBase::Global(def_id) => {
                    if let Some((arg_module_id, args, const_args)) =
                        self.global_instance_args_for_def(def_id, substitutions)
                    {
                        FunctionPlaceBase::GlobalInstance {
                            def_id,
                            arg_module_id,
                            args,
                            const_args,
                        }
                    } else {
                        FunctionPlaceBase::Global(def_id)
                    }
                }
                FunctionPlaceBase::GlobalInstance {
                    def_id,
                    arg_module_id: _,
                    args,
                    const_args,
                } => FunctionPlaceBase::GlobalInstance {
                    def_id,
                    arg_module_id: self.current_arg_module_id(),
                    args: {
                        let instantiated_args = args
                            .into_iter()
                            .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
                            .collect::<Vec<_>>();
                        self.canonicalize_instance_args(&instantiated_args)
                    },
                    const_args: const_args
                        .into_iter()
                        .map(|arg| {
                            self.instantiate_const_generic_arg_with_id(
                                &arg,
                                substitutions,
                                &mut Vec::new(),
                            )
                        })
                        .collect(),
                },
                FunctionPlaceBase::Deref(expr) => {
                    FunctionPlaceBase::Deref(Box::new(self.instantiate_expr(*expr, substitutions)))
                }
                FunctionPlaceBase::Error => {
                    crate::input::unreachable_invalid_function_ir("FunctionPlaceBase::Error")
                }
            },
            elems: place
                .elems
                .into_iter()
                .map(|elem| match elem {
                    FunctionPlaceElem::Field(field) => FunctionPlaceElem::Field(field),
                    FunctionPlaceElem::TupleField(index) => FunctionPlaceElem::TupleField(index),
                    FunctionPlaceElem::Index(expr) => FunctionPlaceElem::Index(Box::new(
                        self.instantiate_expr(*expr, substitutions),
                    )),
                    FunctionPlaceElem::Error => {
                        crate::input::unreachable_invalid_function_ir("FunctionPlaceElem::Error")
                    }
                })
                .collect(),
        }
    }

    pub(super) fn instantiate_slice_range(
        &mut self,
        range: FunctionSliceRange,
        substitutions: TypeSubstitutionId,
    ) -> FunctionSliceRange {
        FunctionSliceRange {
            start: range
                .start
                .map(|start| Box::new(self.instantiate_expr(*start, substitutions))),
            end: range
                .end
                .map(|end| Box::new(self.instantiate_expr(*end, substitutions))),
            inclusive: range.inclusive,
        }
    }

    pub(super) fn instantiate_range(
        &mut self,
        range: FunctionRange,
        substitutions: TypeSubstitutionId,
    ) -> FunctionRange {
        FunctionRange {
            start: range
                .start
                .map(|start| Box::new(self.instantiate_expr(*start, substitutions))),
            end: range
                .end
                .map(|end| Box::new(self.instantiate_expr(*end, substitutions))),
            inclusive: range.inclusive,
        }
    }

    pub(super) fn instantiate_array_elements(
        &mut self,
        elems: FunctionArrayElements,
        substitutions: TypeSubstitutionId,
    ) -> FunctionArrayElements {
        match elems {
            FunctionArrayElements::List(elems) => FunctionArrayElements::List(
                elems
                    .into_iter()
                    .map(|elem| self.instantiate_expr(elem, substitutions))
                    .collect(),
            ),
            FunctionArrayElements::Repeat { value, count } => FunctionArrayElements::Repeat {
                value: Box::new(self.instantiate_expr(*value, substitutions)),
                count: self.instantiate_array_len(count, substitutions),
            },
        }
    }
}

fn extension_trait_candidate_arities_match(
    specific: &crate::ExtensionTraitMethodCandidate,
    general: &crate::ExtensionTraitMethodCandidate,
) -> bool {
    specific.trait_args.len() == general.trait_args.len()
        && specific.trait_const_args.len() == general.trait_const_args.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate_with_arities(
        type_arg_count: usize,
        const_arg_count: usize,
    ) -> crate::ExtensionTraitMethodCandidate {
        let mut module_ids = nia_ids::ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let type_store = nia_ty::TypeStore::new();
        let append = type_store.append_for_module(module_id);
        let ty = append.intern(TyKind::Primitive(nia_ty::PrimitiveTy::Usize));
        crate::ExtensionTraitMethodCandidate {
            module_id,
            target_ty: ty,
            method_def_id: GlobalDefId {
                module_id,
                def_id: nia_ids::DefId(0),
            },
            trait_args: vec![ty; type_arg_count],
            trait_const_args: vec![
                nia_ty::ConstGenericArg {
                    ty,
                    value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(0)),
                };
                const_arg_count
            ],
            where_predicates: Vec::new(),
            effective_generics: Vec::new(),
        }
    }

    #[test]
    fn trait_candidate_arity_rejects_type_and_const_prefixes() {
        let complete = candidate_with_arities(2, 2);
        let short_types = candidate_with_arities(1, 2);
        let short_consts = candidate_with_arities(2, 1);

        assert!(extension_trait_candidate_arities_match(
            &complete, &complete
        ));
        assert!(!extension_trait_candidate_arities_match(
            &complete,
            &short_types
        ));
        assert!(!extension_trait_candidate_arities_match(
            &complete,
            &short_consts
        ));
    }
}
