// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::ModuleLowerer;
use nia_defs::VisibleExtensionTarget;
use nia_function_ir::{
    FunctionArrayElements, FunctionAsmInput, FunctionAsmOutput, FunctionBinding,
    FunctionBuiltinOperator, FunctionBuiltinOperatorOp, FunctionCallee, FunctionDeferBody,
    FunctionExpr, FunctionExprKind, FunctionFieldInit, FunctionForHeader, FunctionInlineAsm,
    FunctionOp, FunctionPlace, FunctionPlaceBase, FunctionPlaceElem, FunctionSliceRange,
    FunctionTerminator,
};
use nia_ids::{GlobalDefId, InternedTyId, TraitId};
use nia_ty::{PrimitiveTy, TyKind};

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
                FunctionExprKind::Len(inner) => FunctionExprKind::Len(Box::new(
                    self.resolve_builtin_operator_calls_in_expr(*inner),
                )),
                FunctionExprKind::Ptr(inner) => FunctionExprKind::Ptr(Box::new(
                    self.resolve_builtin_operator_calls_in_expr(*inner),
                )),
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
            },
            elems: place
                .elems
                .into_iter()
                .map(|elem| match elem {
                    FunctionPlaceElem::Field(field) => FunctionPlaceElem::Field(field),
                    FunctionPlaceElem::Index(expr) => FunctionPlaceElem::Index(Box::new(
                        self.resolve_builtin_operator_calls_in_expr(*expr),
                    )),
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
        &self,
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
        &self,
        operator: FunctionBuiltinOperator,
        args: Vec<FunctionExpr>,
    ) -> FunctionExprKind {
        let [receiver] = args.as_slice() else {
            return FunctionExprKind::Call {
                callee: FunctionCallee::BuiltinOperator(operator),
                args,
            };
        };
        if self.is_primitive_builtin_unary_operator_impl(operator.trait_id, receiver.ty) {
            return FunctionExprKind::Call {
                callee: FunctionCallee::BuiltinOperator(operator),
                args,
            };
        }
        if let Some((def_id, method_args)) =
            self.resolve_builtin_operator_impl_method(operator, receiver.ty)
        {
            let [receiver] = <[FunctionExpr; 1]>::try_from(args).ok().expect(
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
        &self,
        operator: FunctionBuiltinOperator,
        args: Vec<FunctionExpr>,
    ) -> FunctionExprKind {
        let [lhs, rhs] = args.as_slice() else {
            return FunctionExprKind::Call {
                callee: FunctionCallee::BuiltinOperator(operator),
                args,
            };
        };
        if self.is_primitive_builtin_binary_operator_impl(operator.trait_id, lhs.ty, rhs.ty) {
            return FunctionExprKind::Call {
                callee: FunctionCallee::BuiltinOperator(operator),
                args,
            };
        }
        if let Some((def_id, method_args)) =
            self.resolve_builtin_operator_impl_method(operator, lhs.ty)
        {
            let [lhs, rhs] = <[FunctionExpr; 2]>::try_from(args).ok().expect(
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

    fn resolve_builtin_operator_impl_method(
        &self,
        operator: FunctionBuiltinOperator,
        lhs_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let method_name = builtin_operator_method_name(operator);
        let candidates = self
            .input
            .extensions
            .targets()
            .iter()
            .filter_map(|target| {
                self.builtin_operator_impl_method_for_target(
                    target,
                    operator.trait_id,
                    method_name,
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
        method_name: &str,
        lhs_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        if !self.extension_type_pattern_matches(target.target_ty, lhs_ty) {
            return None;
        }
        let method = target.methods.iter().find(|method| {
            method.name == method_name && method.trait_id == Some(TraitId::Builtin(trait_id))
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

    fn is_primitive_builtin_unary_operator_impl(
        &self,
        trait_id: nia_ids::BuiltinTrait,
        self_ty: InternedTyId,
    ) -> bool {
        match trait_id {
            nia_ids::BuiltinTrait::Neg => self.is_numeric(self_ty),
            nia_ids::BuiltinTrait::BitNot => self.is_integer(self_ty),
            nia_ids::BuiltinTrait::Not => self.types_match(self_ty, self.bool_ty()),
            _ => false,
        }
    }

    fn is_primitive_builtin_binary_operator_impl(
        &self,
        trait_id: nia_ids::BuiltinTrait,
        lhs_ty: InternedTyId,
        rhs_ty: InternedTyId,
    ) -> bool {
        match trait_id {
            nia_ids::BuiltinTrait::Add
            | nia_ids::BuiltinTrait::Sub
            | nia_ids::BuiltinTrait::Mul
            | nia_ids::BuiltinTrait::Div
            | nia_ids::BuiltinTrait::Rem => {
                self.types_match(lhs_ty, rhs_ty) && self.is_numeric(lhs_ty)
            }
            nia_ids::BuiltinTrait::BitAnd
            | nia_ids::BuiltinTrait::BitOr
            | nia_ids::BuiltinTrait::BitXor => {
                self.types_match(lhs_ty, rhs_ty) && self.is_integer(lhs_ty)
            }
            nia_ids::BuiltinTrait::Shl | nia_ids::BuiltinTrait::Shr => {
                self.is_integer(lhs_ty) && self.is_integer(rhs_ty)
            }
            nia_ids::BuiltinTrait::Eq => {
                self.types_match(lhs_ty, rhs_ty)
                    && (self.is_numeric(lhs_ty)
                        || self.types_match(lhs_ty, self.bool_ty())
                        || self.is_pointer(lhs_ty)
                        || self.is_enum(lhs_ty))
            }
            nia_ids::BuiltinTrait::Ord => {
                self.types_match(lhs_ty, rhs_ty) && self.is_numeric(lhs_ty)
            }
            _ => false,
        }
    }

    fn bool_ty(&self) -> InternedTyId {
        self.interner.primitive(PrimitiveTy::Bool)
    }

    fn is_numeric(&self, ty: InternedTyId) -> bool {
        self.is_integer(ty)
            || matches!(
                self.ty_kind(ty),
                Some(TyKind::Primitive(PrimitiveTy::F32 | PrimitiveTy::F64))
            )
    }

    fn is_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.ty_kind(ty),
            Some(TyKind::Primitive(
                PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize
                    | PrimitiveTy::U8
                    | PrimitiveTy::U16
                    | PrimitiveTy::U32
                    | PrimitiveTy::U64
                    | PrimitiveTy::U128
                    | PrimitiveTy::Usize
            ))
        )
    }

    fn is_pointer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.ty_kind(ty),
            Some(TyKind::Pointer { .. } | TyKind::FunctionPointer { .. })
        )
    }

    fn is_enum(&self, ty: InternedTyId) -> bool {
        matches!(
            self.ty_kind(ty),
            Some(TyKind::Nominal { def_id, .. }) if self.input.signatures.enums.contains_key(&def_id.def_id)
        )
    }
}

fn builtin_operator_method_name(operator: FunctionBuiltinOperator) -> &'static str {
    match (operator.trait_id, operator.op) {
        (nia_ids::BuiltinTrait::Add, _) => "add",
        (nia_ids::BuiltinTrait::Sub, _) => "sub",
        (nia_ids::BuiltinTrait::Mul, _) => "mul",
        (nia_ids::BuiltinTrait::Div, _) => "div",
        (nia_ids::BuiltinTrait::Rem, _) => "rem",
        (nia_ids::BuiltinTrait::Neg, _) => "neg",
        (nia_ids::BuiltinTrait::Not, _) => "not",
        (nia_ids::BuiltinTrait::BitNot, _) => "bit_not",
        (nia_ids::BuiltinTrait::BitAnd, _) => "bit_and",
        (nia_ids::BuiltinTrait::BitOr, _) => "bit_or",
        (nia_ids::BuiltinTrait::BitXor, _) => "bit_xor",
        (nia_ids::BuiltinTrait::Shl, _) => "shl",
        (nia_ids::BuiltinTrait::Shr, _) => "shr",
        (nia_ids::BuiltinTrait::Eq, FunctionBuiltinOperatorOp::Binary(nia_ast::BinaryOp::Eq)) => {
            "eq"
        }
        (nia_ids::BuiltinTrait::Eq, FunctionBuiltinOperatorOp::Binary(nia_ast::BinaryOp::Ne)) => {
            "ne"
        }
        (nia_ids::BuiltinTrait::Ord, FunctionBuiltinOperatorOp::Binary(nia_ast::BinaryOp::Lt)) => {
            "lt"
        }
        (nia_ids::BuiltinTrait::Ord, FunctionBuiltinOperatorOp::Binary(nia_ast::BinaryOp::Le)) => {
            "le"
        }
        (nia_ids::BuiltinTrait::Ord, FunctionBuiltinOperatorOp::Binary(nia_ast::BinaryOp::Gt)) => {
            "gt"
        }
        (nia_ids::BuiltinTrait::Ord, FunctionBuiltinOperatorOp::Binary(nia_ast::BinaryOp::Ge)) => {
            "ge"
        }
        (nia_ids::BuiltinTrait::Sized, _) => "sized",
        _ => "invalid",
    }
}
