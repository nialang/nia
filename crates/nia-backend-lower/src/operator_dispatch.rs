// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::ModuleLowerer;
use nia_defs::VisibleExtensionTarget;
use nia_function_ir::{
    FunctionArrayElements, FunctionAsmInput, FunctionAsmOutput, FunctionBinding,
    FunctionBuiltinOperator, FunctionBuiltinOperatorOp, FunctionCallee, FunctionDeferBody,
    FunctionExpr, FunctionExprKind, FunctionFieldInit, FunctionForHeader, FunctionInlineAsm,
    FunctionOp, FunctionPlace, FunctionPlaceBase, FunctionPlaceElem, FunctionRange,
    FunctionSliceRange, FunctionTerminator,
};
use nia_ids::{BuiltinTrait, BuiltinTraitMethod, GlobalDefId, InternedTyId, TraitId};
use nia_trait_solve::{TraitGoal, TraitResolution, TraitSolverContext};

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn resolve_builtin_operator_calls_in_body(
        &mut self,
        body: nia_function_ir::FunctionBody,
    ) -> nia_function_ir::FunctionBody {
        nia_function_ir::FunctionBody {
            span: body.span,
            locals: body.locals,
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
                        .map(|op| self.resolve_builtin_operator_calls_in_op(op))
                        .collect(),
                    terminator: self.resolve_builtin_operator_calls_in_terminator(block.terminator),
                })
                .collect(),
            entry: body.entry,
            ty: body.ty,
        }
    }

    fn resolve_builtin_operator_calls_in_defer_body(
        &mut self,
        body: FunctionDeferBody,
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
                        .map(|op| self.resolve_builtin_operator_calls_in_op(op))
                        .collect(),
                    terminator: self.resolve_builtin_operator_calls_in_terminator(block.terminator),
                })
                .collect(),
            entry: body.entry,
        }
    }

    fn resolve_builtin_operator_calls_in_op(&mut self, op: FunctionOp) -> FunctionOp {
        match op {
            FunctionOp::Binding(binding) => {
                FunctionOp::Binding(self.resolve_builtin_operator_calls_in_binding(binding))
            }
            FunctionOp::StoreLocal {
                local_id,
                value,
                span,
            } => FunctionOp::StoreLocal {
                local_id,
                value: self.resolve_builtin_operator_calls_in_expr(value),
                span,
            },
            FunctionOp::Expr(expr) => {
                FunctionOp::Expr(self.resolve_builtin_operator_calls_in_expr(expr))
            }
            FunctionOp::Defer(body) => {
                FunctionOp::Defer(self.resolve_builtin_operator_calls_in_defer_body(body))
            }
        }
    }

    fn resolve_builtin_operator_calls_in_binding(
        &mut self,
        binding: FunctionBinding,
    ) -> FunctionBinding {
        FunctionBinding {
            local_id: binding.local_id,
            name: binding.name,
            ty: binding.ty,
            value: binding
                .value
                .map(|value| self.resolve_builtin_operator_calls_in_expr(value)),
            is_const: binding.is_const,
        }
    }

    fn resolve_builtin_operator_calls_in_terminator(
        &mut self,
        terminator: FunctionTerminator,
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
                cond: self.resolve_builtin_operator_calls_in_expr(cond),
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
                target: self.resolve_builtin_operator_calls_in_expr(target),
                arms: arms
                    .into_iter()
                    .map(|arm| nia_function_ir::FunctionSwitchArm {
                        pattern: self.resolve_builtin_operator_calls_in_expr(arm.pattern),
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
                header: self.resolve_builtin_operator_calls_in_for_header(header),
                body,
                continue_target,
                break_target,
                span,
            },
            FunctionTerminator::Return { value, span } => FunctionTerminator::Return {
                value: value.map(|value| self.resolve_builtin_operator_calls_in_expr(value)),
                span,
            },
            FunctionTerminator::Tail { value, span } => FunctionTerminator::Tail {
                value: value.map(|value| self.resolve_builtin_operator_calls_in_expr(value)),
                span,
            },
        }
    }

    fn resolve_builtin_operator_calls_in_for_header(
        &mut self,
        header: FunctionForHeader,
    ) -> FunctionForHeader {
        match header {
            FunctionForHeader::Infinite => FunctionForHeader::Infinite,
            FunctionForHeader::Condition(expr) => {
                FunctionForHeader::Condition(self.resolve_builtin_operator_calls_in_expr(expr))
            }
            FunctionForHeader::CStyle { cond } => FunctionForHeader::CStyle {
                cond: cond.map(|cond| Box::new(self.resolve_builtin_operator_calls_in_expr(*cond))),
            },
        }
    }

    fn resolve_builtin_operator_calls_in_expr(&mut self, expr: FunctionExpr) -> FunctionExpr {
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
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
                    FunctionExprKind::FunctionInstance { def_id, args }
                }
                FunctionExprKind::EnumVariant(def_id) => FunctionExprKind::EnumVariant(def_id),
                FunctionExprKind::BuiltinValue(value) => FunctionExprKind::BuiltinValue(value),
                FunctionExprKind::Range(range) => {
                    FunctionExprKind::Range(self.resolve_builtin_operator_calls_in_range(range))
                }
                FunctionExprKind::InlineAsm(asm) => {
                    FunctionExprKind::InlineAsm(FunctionInlineAsm {
                        code: asm.code,
                        inputs: asm
                            .inputs
                            .into_iter()
                            .map(|input| FunctionAsmInput {
                                constraint: input.constraint,
                                value: self.resolve_builtin_operator_calls_in_expr(input.value),
                                span: input.span,
                            })
                            .collect(),
                        outputs: asm
                            .outputs
                            .into_iter()
                            .map(|output| FunctionAsmOutput {
                                constraint: output.constraint,
                                place: self.resolve_builtin_operator_calls_in_place(output.place),
                                span: output.span,
                            })
                            .collect(),
                        clobbers: asm.clobbers,
                        options: asm.options,
                    })
                }
                FunctionExprKind::CStringPointer { array, is_const } => {
                    FunctionExprKind::CStringPointer {
                        array: Box::new(self.resolve_builtin_operator_calls_in_expr(*array)),
                        is_const,
                    }
                }
                FunctionExprKind::ArrayLiteral { elems } => FunctionExprKind::ArrayLiteral {
                    elems: self.resolve_builtin_operator_calls_in_array_elements(elems),
                },
                FunctionExprKind::StructLiteral { def_id, fields } => {
                    FunctionExprKind::StructLiteral {
                        def_id,
                        fields: fields
                            .into_iter()
                            .map(|field| FunctionFieldInit {
                                field: field.field,
                                name: field.name,
                                value: self.resolve_builtin_operator_calls_in_expr(field.value),
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
                            value: self.resolve_builtin_operator_calls_in_expr(field.value),
                            span: field.span,
                        }),
                    }
                }
                FunctionExprKind::Unary { op, expr } => FunctionExprKind::Unary {
                    op,
                    expr: Box::new(self.resolve_builtin_operator_calls_in_expr(*expr)),
                },
                FunctionExprKind::AddrOf(place) => {
                    FunctionExprKind::AddrOf(self.resolve_builtin_operator_calls_in_place(place))
                }
                FunctionExprKind::Binary { lhs, op, rhs } => FunctionExprKind::Binary {
                    lhs: Box::new(self.resolve_builtin_operator_calls_in_expr(*lhs)),
                    op,
                    rhs: Box::new(self.resolve_builtin_operator_calls_in_expr(*rhs)),
                },
                FunctionExprKind::Assign { place, op, rhs } => FunctionExprKind::Assign {
                    place: self.resolve_builtin_operator_calls_in_place(place),
                    op,
                    rhs: Box::new(self.resolve_builtin_operator_calls_in_expr(*rhs)),
                },
                FunctionExprKind::Discard(expr) => FunctionExprKind::Discard(Box::new(
                    self.resolve_builtin_operator_calls_in_expr(*expr),
                )),
                FunctionExprKind::Cast { expr, ty } => FunctionExprKind::Cast {
                    expr: Box::new(self.resolve_builtin_operator_calls_in_expr(*expr)),
                    ty,
                },
                FunctionExprKind::TraitObjectUpcast {
                    expr,
                    source_ty,
                    target_ty,
                } => FunctionExprKind::TraitObjectUpcast {
                    expr: Box::new(self.resolve_builtin_operator_calls_in_expr(*expr)),
                    source_ty,
                    target_ty,
                },
                FunctionExprKind::TraitObjectCoercion {
                    expr,
                    target_ty,
                    self_ty,
                } => FunctionExprKind::TraitObjectCoercion {
                    expr: Box::new(self.resolve_builtin_operator_calls_in_expr(*expr)),
                    target_ty,
                    self_ty,
                },
                FunctionExprKind::Call { callee, args } => {
                    let callee = self.resolve_builtin_operator_calls_in_callee(callee);
                    let args = args
                        .into_iter()
                        .map(|arg| self.resolve_builtin_operator_calls_in_expr(arg))
                        .collect::<Vec<_>>();
                    match callee {
                        FunctionCallee::BuiltinOperator(operator) => {
                            self.dispatch_builtin_operator_call(operator, args)
                        }
                        FunctionCallee::BuiltinPlaceMethod {
                            trait_id,
                            method,
                            self_ty,
                            trait_args,
                            receiver,
                        } => self.dispatch_builtin_place_method_call(
                            trait_id, method, self_ty, trait_args, *receiver, args,
                        ),
                        callee => FunctionExprKind::Call { callee, args },
                    }
                }
                FunctionExprKind::Field { lhs, field } => FunctionExprKind::Field {
                    lhs: Box::new(self.resolve_builtin_operator_calls_in_expr(*lhs)),
                    field,
                },
                FunctionExprKind::Index { lhs, index } => FunctionExprKind::Index {
                    lhs: Box::new(self.resolve_builtin_operator_calls_in_expr(*lhs)),
                    index: Box::new(self.resolve_builtin_operator_calls_in_expr(*index)),
                },
                FunctionExprKind::Slice {
                    lhs,
                    range,
                    is_const,
                } => FunctionExprKind::Slice {
                    lhs: Box::new(self.resolve_builtin_operator_calls_in_expr(*lhs)),
                    range: self.resolve_builtin_operator_calls_in_slice_range(range),
                    is_const,
                },
            },
        }
    }

    fn resolve_builtin_operator_calls_in_callee(
        &mut self,
        callee: FunctionCallee,
    ) -> FunctionCallee {
        match callee {
            FunctionCallee::Function(def_id) => FunctionCallee::Function(def_id),
            FunctionCallee::FunctionInstance { def_id, args } => {
                FunctionCallee::FunctionInstance { def_id, args }
            }
            FunctionCallee::Method {
                def_id,
                args,
                receiver,
            } => FunctionCallee::Method {
                def_id,
                args,
                receiver: Box::new(self.resolve_builtin_operator_calls_in_expr(*receiver)),
            },
            FunctionCallee::TraitMethod {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
                receiver,
            } => FunctionCallee::TraitMethod {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
                receiver: Box::new(self.resolve_builtin_operator_calls_in_expr(*receiver)),
            },
            FunctionCallee::BuiltinPlaceMethod {
                trait_id,
                method,
                self_ty,
                trait_args,
                receiver,
            } => FunctionCallee::BuiltinPlaceMethod {
                trait_id,
                method,
                self_ty,
                trait_args,
                receiver: Box::new(self.resolve_builtin_operator_calls_in_expr(*receiver)),
            },
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
                object_ty,
                trait_id,
                method_id,
                method_name,
                trait_args,
                slot,
                params,
                return_type,
                receiver: Box::new(self.resolve_builtin_operator_calls_in_expr(*receiver)),
            },
            FunctionCallee::BuiltinOperator(operator) => FunctionCallee::BuiltinOperator(operator),
            FunctionCallee::FunctionPointer(expr) => FunctionCallee::FunctionPointer(Box::new(
                self.resolve_builtin_operator_calls_in_expr(*expr),
            )),
        }
    }

    fn resolve_builtin_operator_calls_in_place(&mut self, place: FunctionPlace) -> FunctionPlace {
        FunctionPlace {
            span: place.span,
            ty: place.ty,
            base: match place.base {
                FunctionPlaceBase::Local(local_id) => FunctionPlaceBase::Local(local_id),
                FunctionPlaceBase::Global(def_id) => FunctionPlaceBase::Global(def_id),
                FunctionPlaceBase::Deref(expr) => FunctionPlaceBase::Deref(Box::new(
                    self.resolve_builtin_operator_calls_in_expr(*expr),
                )),
                FunctionPlaceBase::Error => FunctionPlaceBase::Error,
            },
            elems: place
                .elems
                .into_iter()
                .map(|elem| match elem {
                    FunctionPlaceElem::Field(field) => FunctionPlaceElem::Field(field),
                    FunctionPlaceElem::Index(expr) => FunctionPlaceElem::Index(Box::new(
                        self.resolve_builtin_operator_calls_in_expr(*expr),
                    )),
                    FunctionPlaceElem::Error => FunctionPlaceElem::Error,
                })
                .collect(),
        }
    }

    fn resolve_builtin_operator_calls_in_slice_range(
        &mut self,
        range: FunctionSliceRange,
    ) -> FunctionSliceRange {
        FunctionSliceRange {
            start: range
                .start
                .map(|start| Box::new(self.resolve_builtin_operator_calls_in_expr(*start))),
            end: range
                .end
                .map(|end| Box::new(self.resolve_builtin_operator_calls_in_expr(*end))),
            inclusive: range.inclusive,
        }
    }

    fn resolve_builtin_operator_calls_in_range(&mut self, range: FunctionRange) -> FunctionRange {
        FunctionRange {
            start: range
                .start
                .map(|start| Box::new(self.resolve_builtin_operator_calls_in_expr(*start))),
            end: range
                .end
                .map(|end| Box::new(self.resolve_builtin_operator_calls_in_expr(*end))),
            inclusive: range.inclusive,
        }
    }

    fn resolve_builtin_operator_calls_in_array_elements(
        &mut self,
        elems: FunctionArrayElements,
    ) -> FunctionArrayElements {
        match elems {
            FunctionArrayElements::List(elems) => FunctionArrayElements::List(
                elems
                    .into_iter()
                    .map(|elem| self.resolve_builtin_operator_calls_in_expr(elem))
                    .collect(),
            ),
            FunctionArrayElements::Repeat { value, count } => FunctionArrayElements::Repeat {
                value: Box::new(self.resolve_builtin_operator_calls_in_expr(*value)),
                count,
            },
        }
    }

    fn dispatch_builtin_operator_call(
        &mut self,
        operator: FunctionBuiltinOperator,
        args: Vec<FunctionExpr>,
    ) -> FunctionExprKind {
        match operator.op {
            FunctionBuiltinOperatorOp::Unary(_) => {
                self.dispatch_builtin_unary_operator(operator, args)
            }
            FunctionBuiltinOperatorOp::Binary(_) => {
                self.dispatch_builtin_binary_operator(operator, args)
            }
        }
    }

    fn dispatch_builtin_unary_operator(
        &mut self,
        operator: FunctionBuiltinOperator,
        args: Vec<FunctionExpr>,
    ) -> FunctionExprKind {
        let [receiver] = args.as_slice() else {
            return FunctionExprKind::Call {
                callee: FunctionCallee::BuiltinOperator(operator),
                args,
            };
        };
        if matches!(
            self.builtin_operator_resolution(operator.trait_id, receiver.ty, &[]),
            TraitResolution::Intrinsic(_)
        ) {
            return FunctionExprKind::Call {
                callee: FunctionCallee::BuiltinOperator(operator),
                args,
            };
        }
        if let Some((def_id, method_args)) =
            self.resolve_builtin_operator_impl_method(operator, receiver.ty, &[])
        {
            let [receiver] = <[FunctionExpr; 1]>::try_from(args).expect(
                "builtin unary operator call arity was checked before dispatching to method call",
            );
            return FunctionExprKind::Call {
                callee: FunctionCallee::Method {
                    def_id,
                    args: method_args,
                    receiver: Box::new(receiver),
                },
                args: Vec::new(),
            };
        }
        FunctionExprKind::Call {
            callee: FunctionCallee::BuiltinOperator(operator),
            args,
        }
    }

    fn dispatch_builtin_binary_operator(
        &mut self,
        operator: FunctionBuiltinOperator,
        args: Vec<FunctionExpr>,
    ) -> FunctionExprKind {
        let [lhs, rhs] = args.as_slice() else {
            return FunctionExprKind::Call {
                callee: FunctionCallee::BuiltinOperator(operator),
                args,
            };
        };
        if matches!(
            self.builtin_operator_resolution(operator.trait_id, lhs.ty, &[rhs.ty]),
            TraitResolution::Intrinsic(_)
        ) {
            return FunctionExprKind::Call {
                callee: FunctionCallee::BuiltinOperator(operator),
                args,
            };
        }
        if let Some((def_id, method_args)) =
            self.resolve_builtin_operator_impl_method(operator, lhs.ty, &[rhs.ty])
        {
            let [lhs, rhs] = <[FunctionExpr; 2]>::try_from(args).expect(
                "builtin binary operator call arity was checked before dispatching to method call",
            );
            return FunctionExprKind::Call {
                callee: FunctionCallee::Method {
                    def_id,
                    args: method_args,
                    receiver: Box::new(lhs),
                },
                args: vec![rhs],
            };
        }
        FunctionExprKind::Call {
            callee: FunctionCallee::BuiltinOperator(operator),
            args,
        }
    }

    fn dispatch_builtin_place_method_call(
        &self,
        trait_id: BuiltinTrait,
        method: BuiltinTraitMethod,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        receiver: FunctionExpr,
        args: Vec<FunctionExpr>,
    ) -> FunctionExprKind {
        if let Some((def_id, method_args)) =
            self.resolve_builtin_place_impl_method(trait_id, &trait_args, method, self_ty)
        {
            FunctionExprKind::Call {
                callee: FunctionCallee::Method {
                    def_id,
                    args: method_args,
                    receiver: Box::new(receiver),
                },
                args,
            }
        } else {
            FunctionExprKind::Call {
                callee: FunctionCallee::BuiltinPlaceMethod {
                    trait_id,
                    method,
                    self_ty,
                    trait_args,
                    receiver: Box::new(receiver),
                },
                args,
            }
        }
    }

    fn resolve_builtin_operator_impl_method(
        &self,
        operator: FunctionBuiltinOperator,
        lhs_ty: InternedTyId,
        trait_args: &[InternedTyId],
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let method = operator.method()?;
        let candidates = self
            .input
            .extensions
            .targets()
            .iter()
            .filter_map(|target| {
                self.builtin_operator_impl_method_for_target(
                    target,
                    operator.trait_id,
                    trait_args,
                    method.name(),
                    lhs_ty,
                )
            })
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [candidate] => Some(candidate.clone()),
            _ => None,
        }
    }

    fn builtin_operator_impl_method_for_target(
        &self,
        target: &VisibleExtensionTarget,
        trait_id: nia_ids::BuiltinTrait,
        trait_args: &[InternedTyId],
        method_name: &str,
        lhs_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        if !self.extension_type_pattern_matches(target.target_ty, lhs_ty) {
            return None;
        }
        let method = target.methods.iter().find(|method| {
            method.name == method_name
                && method.trait_id == Some(TraitId::Builtin(trait_id))
                && method.trait_args.len() == trait_args.len()
                && method
                    .trait_args
                    .iter()
                    .zip(trait_args)
                    .all(|(actual, expected)| self.types_match(*actual, *expected))
        })?;
        let mut substitutions = HashMap::new();
        self.match_extension_type_pattern(target.target_ty, lhs_ty, &mut substitutions)
            .then(|| {
                let args = self
                    .generic_params_in_extension_ty(target.target_ty)
                    .iter()
                    .filter_map(|generic| substitutions.get(generic).copied())
                    .collect::<Vec<_>>();
                (method.def_id, args)
            })
    }

    fn resolve_builtin_place_impl_method(
        &self,
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
                self.dispatch_builtin_place_impl_method_for_target(
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

    fn dispatch_builtin_place_impl_method_for_target(
        &self,
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
            method.name == method_name
                && method.trait_id == Some(TraitId::Builtin(trait_id))
                && method.trait_args.len() == trait_args.len()
                && method
                    .trait_args
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

    fn builtin_operator_resolution(
        &mut self,
        trait_id: BuiltinTrait,
        self_ty: InternedTyId,
        trait_args: &[InternedTyId],
    ) -> TraitResolution {
        let context = TraitSolverContext {
            normalization: self.input.type_normalization,
            trait_impls: self.input.trait_impls,
            layouts: Some(self.input.layouts),
            local_module_id: self.input.module_id,
            local_enums: &self.input.signatures.enums,
            program_enums: Some(self.input.program_enums),
        };
        let mut solver = context.solver(&mut self.interner, &[]);
        solver.resolve(TraitGoal {
            self_ty,
            trait_id: TraitId::Builtin(trait_id),
            trait_args: trait_args.to_vec(),
        })
    }
}
