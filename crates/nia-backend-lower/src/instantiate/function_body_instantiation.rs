// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

impl<'a> ModuleLowerer<'a> {
    pub(super) fn instantiate_defer_body(
        &mut self,
        body: FunctionDeferBody,
        substitutions: &HashMap<String, InternedTyId>,
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
        substitutions: &HashMap<String, InternedTyId>,
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
            FunctionOp::Expr(expr) => FunctionOp::Expr(self.instantiate_expr(expr, substitutions)),
            FunctionOp::Defer(body) => {
                FunctionOp::Defer(self.instantiate_defer_body(body, substitutions))
            }
        }
    }

    pub(super) fn instantiate_binding(
        &mut self,
        binding: FunctionBinding,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> FunctionBinding {
        FunctionBinding {
            local_id: binding.local_id,
            name: binding.name,
            ty: self.instantiate_ty(binding.ty, substitutions),
            value: binding
                .value
                .map(|value| self.instantiate_expr(value, substitutions)),
            is_const: binding.is_const,
        }
    }

    pub(super) fn instantiate_terminator(
        &mut self,
        terminator: FunctionTerminator,
        substitutions: &HashMap<String, InternedTyId>,
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
        substitutions: &HashMap<String, InternedTyId>,
    ) -> FunctionForHeader {
        match header {
            FunctionForHeader::Infinite => FunctionForHeader::Infinite,
            FunctionForHeader::Condition(expr) => {
                FunctionForHeader::Condition(self.instantiate_expr(expr, substitutions))
            }
            FunctionForHeader::CStyle { cond } => FunctionForHeader::CStyle {
                cond: cond.map(|cond| Box::new(self.instantiate_expr(*cond, substitutions))),
            },
        }
    }

    pub(super) fn instantiate_expr(
        &mut self,
        expr: FunctionExpr,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> FunctionExpr {
        FunctionExpr {
            span: expr.span,
            ty: self.instantiate_ty(expr.ty, substitutions),
            kind: match expr.kind {
                FunctionExprKind::Error => FunctionExprKind::Error,
                FunctionExprKind::Integer(text) => FunctionExprKind::Integer(text),
                FunctionExprKind::Float(text) => FunctionExprKind::Float(text),
                FunctionExprKind::String(scalars) => FunctionExprKind::String(scalars),
                FunctionExprKind::ByteString(bytes) => FunctionExprKind::ByteString(bytes),
                FunctionExprKind::Char(value) => FunctionExprKind::Char(value),
                FunctionExprKind::ByteChar(text) => FunctionExprKind::ByteChar(text),
                FunctionExprKind::Bool(value) => FunctionExprKind::Bool(value),
                FunctionExprKind::Local(local) => FunctionExprKind::Local(local),
                FunctionExprKind::Global(def_id) => FunctionExprKind::Global(def_id),
                FunctionExprKind::Function(def_id) => FunctionExprKind::Function(def_id),
                FunctionExprKind::FunctionInstance { def_id, args } => {
                    FunctionExprKind::FunctionInstance {
                        def_id,
                        args: args
                            .into_iter()
                            .map(|arg| self.instantiate_ty(arg, substitutions))
                            .collect(),
                    }
                }
                FunctionExprKind::EnumVariant(def_id) => FunctionExprKind::EnumVariant(def_id),
                FunctionExprKind::BuiltinValue(value) => FunctionExprKind::BuiltinValue(
                    self.instantiate_builtin_value(value, substitutions),
                ),
                FunctionExprKind::Range(range) => {
                    FunctionExprKind::Range(self.instantiate_range(range, substitutions))
                }
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
                FunctionExprKind::CStringPointer { array, is_const } => {
                    FunctionExprKind::CStringPointer {
                        array: Box::new(self.instantiate_expr(*array, substitutions)),
                        is_const,
                    }
                }
                FunctionExprKind::ArrayLiteral { elems } => FunctionExprKind::ArrayLiteral {
                    elems: self.instantiate_array_elements(elems, substitutions),
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
                FunctionExprKind::Unary { op, expr } => FunctionExprKind::Unary {
                    op,
                    expr: Box::new(self.instantiate_expr(*expr, substitutions)),
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
                    ty: self.instantiate_ty(ty, substitutions),
                },
                FunctionExprKind::TraitObjectUpcast {
                    expr,
                    source_ty,
                    target_ty,
                } => FunctionExprKind::TraitObjectUpcast {
                    expr: Box::new(self.instantiate_expr(*expr, substitutions)),
                    source_ty: self.instantiate_ty(source_ty, substitutions),
                    target_ty: self.instantiate_ty(target_ty, substitutions),
                },
                FunctionExprKind::TraitObjectCoercion {
                    expr,
                    target_ty,
                    self_ty,
                } => FunctionExprKind::TraitObjectCoercion {
                    expr: Box::new(self.instantiate_expr(*expr, substitutions)),
                    target_ty: self.instantiate_ty(target_ty, substitutions),
                    self_ty: self.instantiate_ty(self_ty, substitutions),
                },
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
                        && matches!(
                            self.resolve_builtin_trait_goal(
                                *self_ty,
                                *trait_id,
                                trait_args.clone()
                            ),
                            TraitResolution::Intrinsic(_)
                        )
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
                        match self.resolve_builtin_trait_goal(self_ty, trait_id, trait_args.clone())
                        {
                            TraitResolution::User(_) => {
                                if let Some((def_id, target_args)) = self
                                    .resolve_builtin_place_method_impl(
                                        trait_id,
                                        &trait_args,
                                        method,
                                        self_ty,
                                    )
                                {
                                    return FunctionExpr {
                                        span: expr.span,
                                        ty: expr.ty,
                                        kind: FunctionExprKind::Call {
                                            callee: FunctionCallee::Method {
                                                def_id,
                                                args: target_args,
                                                receiver,
                                            },
                                            args,
                                        },
                                    };
                                }
                            }
                            TraitResolution::Intrinsic(_) => {
                                return FunctionExpr {
                                    span: expr.span,
                                    ty: expr.ty,
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
                            TraitResolution::Assumed(_)
                            | TraitResolution::Unsatisfied
                            | TraitResolution::Ambiguous => {}
                        }
                        self.diagnostics.push(nia_diagnostic::Diagnostic::error(
                            receiver.span,
                            "no visible implementation found for builtin place method call",
                        ));
                        return FunctionExpr {
                            span: expr.span,
                            ty: expr.ty,
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
                    is_const,
                } => FunctionExprKind::Slice {
                    lhs: Box::new(self.instantiate_expr(*lhs, substitutions)),
                    range: self.instantiate_slice_range(range, substitutions),
                    is_const,
                },
            },
        }
    }

    pub(super) fn instantiate_callee(
        &mut self,
        callee: FunctionCallee,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> FunctionCallee {
        match callee {
            FunctionCallee::Function(def_id) => FunctionCallee::Function(def_id),
            FunctionCallee::FunctionInstance { def_id, args } => FunctionCallee::FunctionInstance {
                def_id,
                args: args
                    .into_iter()
                    .map(|arg| self.instantiate_ty(arg, substitutions))
                    .collect(),
            },
            FunctionCallee::Method {
                def_id,
                args,
                receiver,
            } => FunctionCallee::Method {
                def_id,
                args: args
                    .into_iter()
                    .map(|arg| self.instantiate_ty(arg, substitutions))
                    .collect(),
                receiver: Box::new(self.instantiate_expr(*receiver, substitutions)),
            },
            FunctionCallee::TraitMethod {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
                receiver,
            } => {
                let self_ty = self.instantiate_ty(self_ty, substitutions);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.instantiate_ty(arg, substitutions))
                    .collect::<Vec<_>>();
                let args = args
                    .into_iter()
                    .map(|arg| self.instantiate_ty(arg, substitutions))
                    .collect::<Vec<_>>();
                let receiver = Box::new(self.instantiate_expr(*receiver, substitutions));
                if let Some((def_id, target_args)) = self.resolve_trait_method_impl(
                    trait_id,
                    &trait_args,
                    method_id,
                    &method_name,
                    self_ty,
                ) {
                    let mut instance_args = target_args;
                    instance_args.extend(args);
                    FunctionCallee::Method {
                        def_id,
                        args: instance_args,
                        receiver,
                    }
                } else if self.trait_method_has_default(method_id) {
                    let mut instance_args = vec![self_ty];
                    instance_args.extend(trait_args.iter().copied());
                    instance_args.extend(args);
                    FunctionCallee::Method {
                        def_id: method_id,
                        args: instance_args,
                        receiver,
                    }
                } else {
                    self.diagnostics.push(nia_diagnostic::Diagnostic::error(
                        receiver.span,
                        "no visible implementation found for trait method call",
                    ));
                    FunctionCallee::TraitMethod {
                        trait_id,
                        method_id,
                        method_name,
                        self_ty,
                        trait_args,
                        args,
                        receiver,
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
                let self_ty = self.instantiate_ty(self_ty, substitutions);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.instantiate_ty(arg, substitutions))
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
            FunctionCallee::DynamicTraitMethod {
                object_ty,
                trait_id,
                method_id,
                method_name,
                trait_args,
                slot,
                params,
                return_type,
                receiver,
            } => FunctionCallee::DynamicTraitMethod {
                object_ty: self.instantiate_ty(object_ty, substitutions),
                trait_id,
                method_id,
                method_name,
                trait_args: trait_args
                    .into_iter()
                    .map(|arg| self.instantiate_ty(arg, substitutions))
                    .collect(),
                slot,
                params: params
                    .into_iter()
                    .map(|param| self.instantiate_ty(param, substitutions))
                    .collect(),
                return_type: self.instantiate_ty(return_type, substitutions),
                receiver: Box::new(self.instantiate_expr(*receiver, substitutions)),
            },
            FunctionCallee::BuiltinOperator(operator) => FunctionCallee::BuiltinOperator(operator),
            FunctionCallee::FunctionPointer(expr) => FunctionCallee::FunctionPointer(Box::new(
                self.instantiate_expr(*expr, substitutions),
            )),
        }
    }

    pub(super) fn instantiate_builtin_value(
        &mut self,
        value: nia_function_ir::FunctionBuiltinValue,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> nia_function_ir::FunctionBuiltinValue {
        match value {
            nia_function_ir::FunctionBuiltinValue::Layout { builtin, ty } => {
                let ty = self.instantiate_ty(ty, substitutions);
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
            nia_function_ir::FunctionBuiltinValue::Usize(value) => {
                nia_function_ir::FunctionBuiltinValue::Usize(value)
            }
            nia_function_ir::FunctionBuiltinValue::Int(value) => {
                nia_function_ir::FunctionBuiltinValue::Int(value)
            }
        }
    }

    pub(crate) fn trait_method_has_default(&self, method_id: GlobalDefId) -> bool {
        self.input
            .signatures
            .traits
            .values()
            .flat_map(|signature| signature.methods.iter())
            .any(|method| {
                GlobalDefId {
                    module_id: self.input.module_id,
                    def_id: method.def_id,
                } == method_id
                    && method.has_default
            })
    }

    pub(crate) fn resolve_trait_method_impl(
        &mut self,
        trait_id: GlobalDefId,
        trait_args: &[InternedTyId],
        trait_method_id: GlobalDefId,
        trait_method_name: &str,
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let trait_method_name = self
            .input
            .defs
            .defs
            .get(trait_method_id.def_id)
            .filter(|_| trait_method_id.module_id == self.input.module_id)
            .map(|def| def.name.clone())
            .or_else(|| {
                self.input
                    .extensions
                    .targets()
                    .iter()
                    .flat_map(|target| target.methods.iter())
                    .find(|method| method.def_id == trait_method_id)
                    .map(|method| method.name.clone())
            })
            .unwrap_or_else(|| trait_method_name.to_string());
        let candidates = self
            .input
            .extensions
            .targets()
            .iter()
            .filter_map(|target| {
                self.trait_impl_method_for_target(
                    target,
                    trait_id,
                    trait_args,
                    &trait_method_name,
                    self_ty,
                )
            })
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [candidate] => Some(candidate.clone()),
            _ => None,
        }
    }

    pub(super) fn instantiate_place(
        &mut self,
        place: FunctionPlace,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> FunctionPlace {
        FunctionPlace {
            span: place.span,
            ty: self.instantiate_ty(place.ty, substitutions),
            base: match place.base {
                FunctionPlaceBase::Local(local_id) => FunctionPlaceBase::Local(local_id),
                FunctionPlaceBase::Global(def_id) => FunctionPlaceBase::Global(def_id),
                FunctionPlaceBase::Deref(expr) => {
                    FunctionPlaceBase::Deref(Box::new(self.instantiate_expr(*expr, substitutions)))
                }
                FunctionPlaceBase::Error => FunctionPlaceBase::Error,
            },
            elems: place
                .elems
                .into_iter()
                .map(|elem| match elem {
                    FunctionPlaceElem::Field(field) => FunctionPlaceElem::Field(field),
                    FunctionPlaceElem::Index(expr) => FunctionPlaceElem::Index(Box::new(
                        self.instantiate_expr(*expr, substitutions),
                    )),
                    FunctionPlaceElem::Error => FunctionPlaceElem::Error,
                })
                .collect(),
        }
    }

    pub(super) fn instantiate_slice_range(
        &mut self,
        range: FunctionSliceRange,
        substitutions: &HashMap<String, InternedTyId>,
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
        substitutions: &HashMap<String, InternedTyId>,
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
        substitutions: &HashMap<String, InternedTyId>,
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
                count,
            },
        }
    }
}
