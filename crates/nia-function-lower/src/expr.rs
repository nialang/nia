// SPDX-License-Identifier: GPL-3.0-or-later
use super::support::{LoweringContext, MatchValueArmContext, PatternConditionContext};
use super::*;

impl FunctionLowerer<'_> {
    fn ensure_closure_entry(
        &mut self,
        closure_id: ClosureId,
        state_ty: InternedTyId,
        captures: &[nia_body_ir::TypedClosureCapture],
        params: &[LocalId],
        body: &TypedBody,
    ) {
        // Body IR local tables are source-function-wide. Record the nested
        // function's aliases and parameters before lowering so `lower_body`
        // can keep them out of the enclosing Function IR local table.
        self.nested_closure_locals
            .extend(body.locals.iter().map(|local| local.id));
        if self
            .closure_entries
            .iter()
            .any(|entry| entry.closure_id == closure_id)
        {
            return;
        }
        let (capture_types, param_types, return_type) = match self.types.get(state_ty) {
            Some(TyKind::ClosureState {
                closure_id: actual_id,
                captures,
                params,
                return_type,
            }) if *actual_id == closure_id => (captures.clone(), params.clone(), *return_type),
            _ => unreachable!("typed closure expression must have its closure-state type"),
        };
        debug_assert_eq!(capture_types.len(), captures.len());
        debug_assert_eq!(param_types.len(), params.len());

        let state_ptr_ty = self.types.intern(TyKind::Pointer {
            is_readonly: true,
            elem: state_ty,
        });
        let state_param = LocalId(self.next_available_local(body));
        let capture_fields = captures
            .iter()
            .zip(capture_types)
            .enumerate()
            .map(|(index, (capture, ty))| (capture.local_id, ClosureCaptureField { index, ty }))
            .collect();
        let mut entry_lowerer = FunctionLowerer::new(self.module_id, self.types.clone());
        entry_lowerer.closure_state = Some(ClosureStateContext {
            state_ty,
            state_ptr_ty,
            state_param,
            captures: capture_fields,
        });
        let entry_body = entry_lowerer.lower_body(body);
        self.closure_entries.extend(entry_lowerer.closure_entries);
        self.closure_entries.push(FunctionClosureEntry {
            closure_id,
            state_ty,
            state_param,
            params: params.to_vec(),
            return_type,
            body: entry_body,
        });
    }

    fn closure_capture_expr(&self, local_id: LocalId, span: Span) -> Option<FunctionExprKind> {
        let context = self.closure_state.as_ref()?;
        let capture = context.captures.get(&local_id)?;
        let state = FunctionExpr {
            span,
            ty: context.state_ty,
            kind: FunctionExprKind::Unary {
                op: UnaryOp::Deref,
                expr: Box::new(FunctionExpr {
                    span,
                    ty: context.state_ptr_ty,
                    kind: FunctionExprKind::Local(context.state_param),
                }),
            },
        };
        Some(FunctionExprKind::TupleField {
            value: Box::new(state),
            index: capture.index,
        })
    }

    pub(super) fn lower_value_expr(
        &mut self,
        expr: &TypedExpr,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionExpr {
        let kind = match &expr.kind {
            TypedExprKind::Block(body) => {
                return self.lower_value_block_expr(expr, body, scope, current, ops, blocks);
            }
            TypedExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let mut context = LoweringContext {
                    scope,
                    current,
                    ops,
                    blocks,
                };
                return self.lower_value_if_expr(
                    expr,
                    cond,
                    then_branch,
                    else_branch.as_deref(),
                    &mut context,
                );
            }
            TypedExprKind::IfPattern(if_pattern) => {
                let mut context = LoweringContext {
                    scope,
                    current,
                    ops,
                    blocks,
                };
                return self.lower_value_if_pattern_expr(expr, if_pattern, &mut context);
            }
            TypedExprKind::IfPatternChain(chain) => {
                return self
                    .lower_value_if_pattern_chain_expr(expr, chain, scope, current, ops, blocks);
            }
            TypedExprKind::Match(matched) => {
                return self.lower_value_match_expr(expr, matched, scope, current, ops, blocks);
            }
            TypedExprKind::Range(range) => {
                FunctionExprKind::Range(self.lower_range(range, scope, current, ops, blocks))
            }
            TypedExprKind::MemoryIntrinsic(_) => unreachable!(
                "function lowering input validation rejects memory intrinsics in value position"
            ),
            TypedExprKind::Atomic(atomic) => {
                FunctionExprKind::Atomic(self.lower_atomic(atomic, scope, current, ops, blocks))
            }
            TypedExprKind::LoadUnaligned { ty, ptr } => FunctionExprKind::LoadUnaligned {
                ty: *ty,
                ptr: Box::new(self.lower_value_expr(ptr, scope, current, ops, blocks)),
            },
            TypedExprKind::Splat { value } => FunctionExprKind::Splat {
                value: Box::new(self.lower_value_expr(value, scope, current, ops, blocks)),
            },
            TypedExprKind::ExtractElement { vector, index } => FunctionExprKind::ExtractElement {
                vector: Box::new(self.lower_value_expr(vector, scope, current, ops, blocks)),
                index: Box::new(self.lower_value_expr(index, scope, current, ops, blocks)),
            },
            TypedExprKind::InsertElement {
                vector,
                index,
                value,
            } => FunctionExprKind::InsertElement {
                vector: Box::new(self.lower_value_expr(vector, scope, current, ops, blocks)),
                index: Box::new(self.lower_value_expr(index, scope, current, ops, blocks)),
                value: Box::new(self.lower_value_expr(value, scope, current, ops, blocks)),
            },
            TypedExprKind::Bitmask { vector } => FunctionExprKind::Bitmask {
                vector: Box::new(self.lower_value_expr(vector, scope, current, ops, blocks)),
            },
            TypedExprKind::BitIntrinsic { op, value } => FunctionExprKind::BitIntrinsic {
                op: self.lower_bit_intrinsic_op(*op),
                value: Box::new(self.lower_value_expr(value, scope, current, ops, blocks)),
            },
            TypedExprKind::CharFromU32 { value } => FunctionExprKind::CharFromU32 {
                value: Box::new(self.lower_value_expr(value, scope, current, ops, blocks)),
            },
            TypedExprKind::StaticArrayPointer {
                allocation,
                array,
                is_readonly,
            } => FunctionExprKind::StaticArrayPointer {
                allocation: *allocation,
                array: Box::new(self.lower_value_expr(array, scope, current, ops, blocks)),
                is_readonly: *is_readonly,
            },
            TypedExprKind::ArrayLiteral { elems } => FunctionExprKind::ArrayLiteral {
                elems: self.lower_array_elements(elems, scope, current, ops, blocks),
            },
            TypedExprKind::Tuple(elems) => FunctionExprKind::Tuple(
                elems
                    .iter()
                    .map(|elem| self.lower_value_expr(elem, scope, current, ops, blocks))
                    .collect(),
            ),
            TypedExprKind::StructLiteral { def_id, fields } => FunctionExprKind::StructLiteral {
                def_id: *def_id,
                fields: fields
                    .iter()
                    .map(|field| FunctionFieldInit {
                        field: field.field,
                        name: field.name.clone(),
                        value: self.lower_value_expr(&field.value, scope, current, ops, blocks),
                        span: field.span,
                    })
                    .collect(),
            },
            TypedExprKind::UnionLiteral { def_id, field } => FunctionExprKind::UnionLiteral {
                def_id: *def_id,
                field: Box::new(FunctionFieldInit {
                    field: field.field,
                    name: field.name.clone(),
                    value: self.lower_value_expr(&field.value, scope, current, ops, blocks),
                    span: field.span,
                }),
            },
            TypedExprKind::UnionStorageLiteral { bytes, relocations } => {
                FunctionExprKind::UnionStorageLiteral {
                    bytes: bytes.clone(),
                    relocations: relocations
                        .iter()
                        .map(|relocation| nia_function_ir::FunctionUnionRelocation {
                            offset: relocation.offset,
                            width: relocation.width,
                            allocation: relocation.allocation,
                            pointee: Box::new(self.lower_value_expr(
                                &relocation.pointee,
                                scope,
                                current,
                                ops,
                                blocks,
                            )),
                        })
                        .collect(),
                }
            }
            TypedExprKind::Unary { op, expr: inner }
                if matches!(op, UnaryOp::Ref | UnaryOp::RefReadOnly)
                    && matches!(inner.kind, TypedExprKind::Slice { .. }) =>
            {
                return self.lower_value_expr(inner, scope, current, ops, blocks);
            }
            TypedExprKind::Unary { op, expr: inner }
                if matches!(op, UnaryOp::Ref | UnaryOp::RefReadOnly)
                    && !matches!(
                        inner.kind,
                        TypedExprKind::Function(_) | TypedExprKind::FunctionInstance { .. }
                    ) =>
            {
                FunctionExprKind::AddrOf(self.lower_expr_place(inner, scope, current, ops, blocks))
            }
            TypedExprKind::Unary { op, expr: inner }
                if matches!(op, UnaryOp::Ref | UnaryOp::RefReadOnly)
                    && matches!(
                        inner.kind,
                        TypedExprKind::Function(_) | TypedExprKind::FunctionInstance { .. }
                    ) =>
            {
                let mut inner = self.lower_value_expr(inner, scope, current, ops, blocks);
                if let Some(function_pointer_ty) = self.function_pointer_pointee_ty(expr.ty) {
                    inner.ty = function_pointer_ty;
                }
                FunctionExprKind::Unary {
                    op: *op,
                    expr: Box::new(inner),
                }
            }
            TypedExprKind::Unary { op, expr: inner } => FunctionExprKind::Unary {
                op: *op,
                expr: Box::new(self.lower_value_expr(inner, scope, current, ops, blocks)),
            },
            TypedExprKind::OptionalSome { expr: inner } => FunctionExprKind::OptionalSome {
                expr: Box::new(self.lower_value_expr(inner, scope, current, ops, blocks)),
            },
            TypedExprKind::ErrorOk { expr: inner } => FunctionExprKind::ErrorOk {
                expr: Box::new(self.lower_value_expr(inner, scope, current, ops, blocks)),
            },
            TypedExprKind::ErrorErr { expr: inner } => FunctionExprKind::ErrorErr {
                expr: Box::new(self.lower_value_expr(inner, scope, current, ops, blocks)),
            },
            TypedExprKind::Try { expr: inner, .. } => {
                return self.lower_try_expr(expr, inner, scope, current, ops, blocks);
            }
            TypedExprKind::Binary { lhs, op, rhs } => FunctionExprKind::Binary {
                lhs: Box::new(self.lower_value_expr(lhs, scope, current, ops, blocks)),
                op: *op,
                rhs: Box::new(self.lower_value_expr(rhs, scope, current, ops, blocks)),
            },
            TypedExprKind::Assign { place, op, rhs } => FunctionExprKind::Assign {
                place: self.lower_place(place, scope, current, ops, blocks),
                op: *op,
                rhs: Box::new(self.lower_value_expr(rhs, scope, current, ops, blocks)),
            },
            TypedExprKind::Discard(inner) => FunctionExprKind::Discard(Box::new(
                self.lower_value_expr(inner, scope, current, ops, blocks),
            )),
            TypedExprKind::Cast { expr: inner, ty } => FunctionExprKind::Cast {
                expr: Box::new(self.lower_value_expr(inner, scope, current, ops, blocks)),
                ty: *ty,
            },
            TypedExprKind::TraitObjectUpcast {
                expr: inner,
                source_ty,
                target_ty,
            } => FunctionExprKind::TraitObjectUpcast {
                expr: Box::new(self.lower_value_expr(inner, scope, current, ops, blocks)),
                source_ty: *source_ty,
                target_ty: *target_ty,
            },
            TypedExprKind::TraitObjectCoercion {
                expr: inner,
                target_ty,
                self_ty,
            } => FunctionExprKind::TraitObjectCoercion {
                expr: Box::new(self.lower_value_expr(inner, scope, current, ops, blocks)),
                target_ty: *target_ty,
                self_ty: *self_ty,
            },
            TypedExprKind::CallableCoercion { state, closure_id } => {
                FunctionExprKind::CallableCoercion {
                    state: Box::new(self.lower_value_expr(state, scope, current, ops, blocks)),
                    closure_id: *closure_id,
                }
            }
            TypedExprKind::FunctionCallable { function } => FunctionExprKind::FunctionCallable {
                function: Box::new(self.lower_value_expr(function, scope, current, ops, blocks)),
            },
            TypedExprKind::ClosureFunctionPointer { closure_id } => {
                FunctionExprKind::ClosureFunctionPointer {
                    closure_id: *closure_id,
                }
            }
            TypedExprKind::Call {
                callee: TypedCallee::BuiltinOperator(operator),
                args,
            } if matches!(
                operator.op,
                nia_sema_ir::BuiltinOperatorOp::Unary(UnaryOp::Neg)
            ) && operator.trait_id == nia_ty::BuiltinTrait::Neg
                && let [inner] = args.as_slice()
                && matches!(
                    self.types.get(inner.ty),
                    Some(TyKind::Primitive(PrimitiveTy::I128))
                )
                && let TypedExprKind::Integer(text) = &inner.kind
                && nia_literals::eval_int_literal(text) == Ok(1_u128 << 127) =>
            {
                FunctionExprKind::Integer(i128::MIN.to_string())
            }
            TypedExprKind::Call { callee, args } => FunctionExprKind::Call {
                callee: self.lower_callee(callee, scope, current, ops, blocks),
                args: args
                    .iter()
                    .map(|arg| self.lower_value_expr(arg, scope, current, ops, blocks))
                    .collect(),
            },
            TypedExprKind::Field { lhs, field } => FunctionExprKind::Field {
                lhs: Box::new(self.lower_value_expr(lhs, scope, current, ops, blocks)),
                field: *field,
            },
            TypedExprKind::TupleField { lhs, index } => FunctionExprKind::TupleField {
                value: Box::new(self.lower_value_expr(lhs, scope, current, ops, blocks)),
                index: *index,
            },
            TypedExprKind::Index { lhs, index } => FunctionExprKind::Index {
                lhs: Box::new(self.lower_value_expr(lhs, scope, current, ops, blocks)),
                index: Box::new(self.lower_value_expr(index, scope, current, ops, blocks)),
            },
            TypedExprKind::Slice {
                lhs,
                range,
                is_readonly,
            } => FunctionExprKind::Slice {
                lhs: Box::new(self.lower_value_expr(lhs, scope, current, ops, blocks)),
                range: self.lower_slice_range(range, scope, current, ops, blocks),
                is_readonly: *is_readonly,
            },
            TypedExprKind::InlineAsm(asm) => {
                FunctionExprKind::InlineAsm(self.lower_inline_asm(asm, scope, current, ops, blocks))
            }
            TypedExprKind::Error => unreachable!(
                "function lowering input validation rejects error expressions before lowering"
            ),
            TypedExprKind::Closure {
                closure_id,
                captures,
                params,
                body,
            } => {
                self.ensure_closure_entry(*closure_id, expr.ty, captures, params, body);
                FunctionExprKind::Tuple(
                    captures
                        .iter()
                        .map(|capture| {
                            self.lower_value_expr(&capture.value, scope, current, ops, blocks)
                        })
                        .collect(),
                )
            }
            TypedExprKind::Integer(text) => FunctionExprKind::Integer(text.clone()),
            TypedExprKind::Float(text) => FunctionExprKind::Float(text.clone()),
            TypedExprKind::String(scalars) => FunctionExprKind::String(scalars.clone()),
            TypedExprKind::ByteString(bytes) => FunctionExprKind::ByteString(bytes.clone()),
            TypedExprKind::Char(value) => FunctionExprKind::Char(*value),
            TypedExprKind::ByteChar(text) => FunctionExprKind::ByteChar(text.clone()),
            TypedExprKind::Bool(value) => FunctionExprKind::Bool(*value),
            TypedExprKind::Null => FunctionExprKind::Null,
            TypedExprKind::Local(local_id) => {
                if let Some(capture) = self.closure_capture_expr(*local_id, expr.span) {
                    capture
                } else {
                    FunctionExprKind::Local(*local_id)
                }
            }
            TypedExprKind::Global(def_id) => FunctionExprKind::Global(*def_id),
            TypedExprKind::ConstGeneric(arg) => FunctionExprKind::ConstGeneric(arg.clone()),
            TypedExprKind::Function(def_id) => FunctionExprKind::Function(*def_id),
            TypedExprKind::EnumConstructor(variant) => FunctionExprKind::EnumConstructor(*variant),
            TypedExprKind::FunctionInstance {
                def_id,
                arg_module_id,
                args,
                const_args,
            } => FunctionExprKind::FunctionInstance {
                def_id: *def_id,
                arg_module_id: *arg_module_id,
                self_arg: None,
                args: args.clone(),
                const_args: const_args.clone(),
            },
            TypedExprKind::EnumVariant { variant, fields } => FunctionExprKind::EnumVariant {
                variant: *variant,
                fields: fields
                    .iter()
                    .map(|field| self.lower_value_expr(field, scope, current, ops, blocks))
                    .collect(),
            },
            TypedExprKind::BuiltinValue(value) => {
                FunctionExprKind::BuiltinValue(Self::lower_builtin_value(value))
            }
            TypedExprKind::CallerLocation(location) => {
                FunctionExprKind::CallerLocation(location.clone())
            }
            TypedExprKind::Trap => FunctionExprKind::Trap,
        };
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind,
        }
    }

    fn lower_try_expr(
        &mut self,
        expr: &TypedExpr,
        inner: &TypedExpr,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionExpr {
        let TypedExprKind::Try {
            error_conversion, ..
        } = &expr.kind
        else {
            unreachable!("try lowering requires a try expression")
        };
        let mut value = self.lower_value_expr(inner, scope, current, ops, blocks);
        let error_conversion = error_conversion.as_ref().map(|conversion| {
            let input_local = self.alloc_temp_local(inner.span, inner.ty);
            ops.push(FunctionOp::StoreLocal {
                local_id: input_local,
                value: value.clone(),
                span: inner.span,
            });
            value = FunctionExpr {
                span: inner.span,
                ty: inner.ty,
                kind: FunctionExprKind::Local(input_local),
            };
            let receiver = FunctionExpr {
                span: expr.span,
                ty: conversion.source_ty,
                kind: FunctionExprKind::TaggedUnionPayload {
                    expr: Box::new(value.clone()),
                },
            };
            FunctionExpr {
                span: expr.span,
                ty: conversion.target_ty,
                kind: FunctionExprKind::Call {
                    callee: FunctionCallee::TraitMethod {
                        trait_id: conversion.trait_id,
                        method_id: conversion.method_id,
                        method_name: conversion.method_name,
                        self_ty: conversion.source_ty,
                        trait_args: conversion.trait_args.clone(),
                        trait_const_args: Vec::new(),
                        args: Vec::new(),
                        const_args: Vec::new(),
                        receiver_kind: conversion.receiver_kind,
                        receiver: Box::new(receiver),
                    },
                    args: Vec::new(),
                },
            }
        });
        let local = self.alloc_temp_local(expr.span, expr.ty);
        let success_target = self.alloc_block();
        let kind = self
            .try_kind(inner.ty)
            .expect("try operand kind validated before function lowering");
        self.finish_block(
            blocks,
            *current,
            scope,
            expr.span,
            std::mem::take(ops),
            FunctionTerminator::Try {
                value,
                kind,
                error_conversion: error_conversion.map(Box::new),
                success_local: local,
                success_target,
                span: expr.span,
            },
        );
        *current = success_target;
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind: FunctionExprKind::Local(local),
        }
    }

    pub(super) fn lower_defer_expr(&mut self, expr: &TypedExpr) -> FunctionDeferBody {
        let scope_start = self.scopes.len();
        let root_scope = self.alloc_scope(None, expr.span);
        let entry = self.alloc_block();
        let mut current = entry;
        let mut ops = Vec::new();
        let mut blocks = Vec::new();
        self.lower_effect_expr(expr, root_scope, &mut current, &mut ops, &mut blocks);
        self.finish_block(
            &mut blocks,
            current,
            root_scope,
            expr.span,
            ops,
            FunctionTerminator::Tail {
                value: None,
                span: expr.span,
            },
        );
        let scopes = self.scopes[scope_start..].to_vec();
        self.scopes.truncate(scope_start);
        FunctionDeferBody {
            span: expr.span,
            scopes,
            blocks,
            entry,
        }
    }

    pub(super) fn lower_effect_expr(
        &mut self,
        expr: &TypedExpr,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) {
        match &expr.kind {
            TypedExprKind::Block(body) => {
                let body_entry = self.alloc_block();
                let after_block = self.alloc_block();
                self.finish_block(
                    blocks,
                    *current,
                    scope,
                    expr.span,
                    std::mem::take(ops),
                    FunctionTerminator::Next {
                        target: body_entry,
                        span: expr.span,
                    },
                );
                let body_scope = self.alloc_scope(Some(scope), body.span);
                self.lower_body_into(
                    body,
                    body_entry,
                    body_scope,
                    blocks,
                    Fallthrough::Branch(after_block),
                );
                *current = after_block;
            }
            TypedExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.lower_if_expr_stmt(
                expr.span,
                StatementIf {
                    cond,
                    then_branch,
                    else_branch: else_branch.as_deref(),
                },
                scope,
                current,
                ops,
                blocks,
            ),
            TypedExprKind::IfPattern(if_pattern) => {
                self.lower_if_pattern_expr_stmt(expr.span, if_pattern, scope, current, ops, blocks);
            }
            TypedExprKind::IfPatternChain(chain) => {
                self.lower_if_pattern_chain_expr_stmt(
                    expr.span, chain, scope, current, ops, blocks,
                );
            }
            TypedExprKind::Match(matched) => {
                self.lower_match_expr_stmt(expr.span, matched, scope, current, ops, blocks);
            }
            TypedExprKind::MemoryIntrinsic(memory) => {
                let op = self.lower_memory_intrinsic_op(memory, scope, current, ops, blocks);
                ops.push(op);
            }
            TypedExprKind::Atomic(atomic) => {
                let expr = FunctionExpr {
                    span: expr.span,
                    ty: expr.ty,
                    kind: FunctionExprKind::Atomic(
                        self.lower_atomic(atomic, scope, current, ops, blocks),
                    ),
                };
                ops.push(FunctionOp::Expr(expr));
            }
            _ => {
                let expr = self.lower_value_expr(expr, scope, current, ops, blocks);
                ops.push(FunctionOp::Expr(expr));
            }
        }
    }

    pub(super) fn store_expr_result_or_effect(
        &mut self,
        expr: &TypedExpr,
        local_id: LocalId,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> bool {
        if self.expr_lowers_as_terminating_effect(expr) {
            self.lower_effect_expr(expr, scope, current, ops, blocks);
            self.finish_block(
                blocks,
                *current,
                scope,
                expr.span,
                std::mem::take(ops),
                FunctionTerminator::Error { span: expr.span },
            );
            return true;
        }

        if self.expr_lowers_as_effect_only(expr) {
            self.lower_effect_expr(expr, scope, current, ops, blocks);
            return false;
        }

        let value = self.lower_value_expr(expr, scope, current, ops, blocks);
        ops.push(FunctionOp::StoreLocal {
            local_id,
            value,
            span: expr.span,
        });
        false
    }

    pub(super) fn lower_memory_intrinsic_op(
        &mut self,
        memory: &nia_body_ir::TypedMemoryIntrinsic,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionOp {
        let op = match memory.op {
            nia_body_ir::MemoryIntrinsicOp::Copy => FunctionMemoryIntrinsicOp::Copy,
            nia_body_ir::MemoryIntrinsicOp::Move => FunctionMemoryIntrinsicOp::Move,
            nia_body_ir::MemoryIntrinsicOp::Set => FunctionMemoryIntrinsicOp::Set,
        };
        let dest = self.lower_value_expr(&memory.dest, scope, current, ops, blocks);
        let source = match &memory.source {
            nia_body_ir::TypedMemoryIntrinsicSource::Slice(source) => {
                FunctionMemoryIntrinsicSource::Slice(
                    self.lower_value_expr(source, scope, current, ops, blocks),
                )
            }
            nia_body_ir::TypedMemoryIntrinsicSource::Byte(value) => {
                FunctionMemoryIntrinsicSource::Byte(
                    self.lower_value_expr(value, scope, current, ops, blocks),
                )
            }
        };
        FunctionOp::MemoryIntrinsic(Box::new(FunctionMemoryIntrinsic {
            span: memory.dest.span,
            op,
            elem_ty: memory.elem_ty,
            dest,
            source,
        }))
    }

    fn lower_atomic(
        &mut self,
        atomic: &TypedAtomic,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionAtomic {
        match atomic {
            TypedAtomic::Load { ty, ptr, order } => FunctionAtomic::Load {
                ty: *ty,
                ptr: Box::new(self.lower_value_expr(ptr, scope, current, ops, blocks)),
                order: lower_atomic_order(*order),
            },
            TypedAtomic::Store {
                ty,
                ptr,
                value,
                order,
            } => FunctionAtomic::Store {
                ty: *ty,
                ptr: Box::new(self.lower_value_expr(ptr, scope, current, ops, blocks)),
                value: Box::new(self.lower_value_expr(value, scope, current, ops, blocks)),
                order: lower_atomic_order(*order),
            },
            TypedAtomic::Rmw {
                ty,
                ptr,
                op,
                value,
                order,
            } => FunctionAtomic::Rmw {
                ty: *ty,
                ptr: Box::new(self.lower_value_expr(ptr, scope, current, ops, blocks)),
                op: lower_atomic_rmw_op(*op),
                value: Box::new(self.lower_value_expr(value, scope, current, ops, blocks)),
                order: lower_atomic_order(*order),
            },
            TypedAtomic::Cmpxchg {
                ty,
                ptr,
                expected,
                desired,
                success,
                failure,
                weak,
            } => FunctionAtomic::Cmpxchg {
                ty: *ty,
                ptr: Box::new(self.lower_value_expr(ptr, scope, current, ops, blocks)),
                expected: Box::new(self.lower_value_expr(expected, scope, current, ops, blocks)),
                desired: Box::new(self.lower_value_expr(desired, scope, current, ops, blocks)),
                success: lower_atomic_order(*success),
                failure: lower_atomic_order(*failure),
                weak: *weak,
            },
            TypedAtomic::Fence { order } => FunctionAtomic::Fence {
                order: lower_atomic_order(*order),
            },
        }
    }

    pub(super) fn lower_value_block_expr(
        &mut self,
        expr: &TypedExpr,
        body: &TypedBody,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionExpr {
        let local = self.alloc_temp_local(expr.span, expr.ty);
        let body_entry = self.alloc_block();
        let merge_target = self.alloc_block();
        self.finish_block(
            blocks,
            *current,
            scope,
            expr.span,
            std::mem::take(ops),
            FunctionTerminator::Next {
                target: body_entry,
                span: expr.span,
            },
        );
        let body_scope = self.alloc_scope(Some(scope), body.span);
        self.lower_body_into(
            body,
            body_entry,
            body_scope,
            blocks,
            Fallthrough::StoreThenBranch {
                local_id: local,
                target: merge_target,
            },
        );
        *current = merge_target;
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind: FunctionExprKind::Local(local),
        }
    }

    pub(super) fn lower_value_if_expr(
        &mut self,
        expr: &TypedExpr,
        cond: &TypedExpr,
        then_branch: &TypedBody,
        else_branch: Option<&TypedExpr>,
        context: &mut LoweringContext<'_>,
    ) -> FunctionExpr {
        let cond = self.lower_value_expr(
            cond,
            context.scope,
            context.current,
            context.ops,
            context.blocks,
        );
        let local = self.alloc_temp_local(expr.span, expr.ty);
        let then_target = self.alloc_block();
        let else_target = self.alloc_block();
        let merge_target = self.alloc_block();
        self.finish_block(
            context.blocks,
            *context.current,
            context.scope,
            expr.span,
            std::mem::take(context.ops),
            FunctionTerminator::If {
                cond,
                then_target,
                else_target,
                span: expr.span,
            },
        );

        let then_scope = self.alloc_scope(Some(context.scope), then_branch.span);
        self.lower_body_into(
            then_branch,
            then_target,
            then_scope,
            context.blocks,
            Fallthrough::StoreThenBranch {
                local_id: local,
                target: merge_target,
            },
        );

        let mut else_current = else_target;
        let mut else_ops = Vec::new();
        let mut else_terminated = false;
        if let Some(else_branch) = else_branch {
            else_terminated = self.store_expr_result_or_effect(
                else_branch,
                local,
                context.scope,
                &mut else_current,
                &mut else_ops,
                context.blocks,
            );
        }
        if !else_terminated {
            self.finish_block(
                context.blocks,
                else_current,
                context.scope,
                else_branch.map(|expr| expr.span).unwrap_or(expr.span),
                else_ops,
                FunctionTerminator::Branch {
                    target: merge_target,
                    span: else_branch.map(|expr| expr.span).unwrap_or(expr.span),
                },
            );
        }

        *context.current = merge_target;
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind: FunctionExprKind::Local(local),
        }
    }

    pub(super) fn lower_value_if_pattern_expr(
        &mut self,
        expr: &TypedExpr,
        if_pattern: &TypedIfPattern,
        context: &mut LoweringContext<'_>,
    ) -> FunctionExpr {
        let target_value = self.lower_value_expr(
            &if_pattern.target,
            context.scope,
            context.current,
            context.ops,
            context.blocks,
        );
        let target_local = self.alloc_temp_local(if_pattern.target.span, if_pattern.target.ty);
        context.ops.push(FunctionOp::StoreLocal {
            local_id: target_local,
            value: target_value,
            span: if_pattern.target.span,
        });
        let target = FunctionExpr {
            span: if_pattern.target.span,
            ty: if_pattern.target.ty,
            kind: FunctionExprKind::Local(target_local),
        };
        let result_local = self.alloc_temp_local(expr.span, expr.ty);
        let merge_target = self.alloc_block();
        let then_target = self.alloc_block();
        let else_target = if if_pattern.else_branch.is_some() {
            self.alloc_block()
        } else {
            merge_target
        };
        let mut condition_context = PatternConditionContext {
            scope: context.scope,
            current: context.current,
            ops: context.ops,
            blocks: context.blocks,
            bool_ty: if_pattern.bool_ty,
        };
        let cond = self
            .pattern_condition(&target, &if_pattern.pattern, &mut condition_context)
            .unwrap_or(FunctionExpr {
                span: if_pattern.pattern.span,
                ty: if_pattern.bool_ty,
                kind: FunctionExprKind::Bool(true),
            });
        self.finish_block(
            context.blocks,
            *context.current,
            context.scope,
            if_pattern.pattern.span,
            std::mem::take(context.ops),
            FunctionTerminator::If {
                cond,
                then_target,
                else_target,
                span: if_pattern.pattern.span,
            },
        );
        let then_scope = self.alloc_scope(Some(context.scope), if_pattern.then_branch.span);
        let mut then_ops = Vec::new();
        self.lower_pattern_binding(&if_pattern.pattern, &target, &mut then_ops);
        let body_entry = if then_ops.is_empty() {
            then_target
        } else {
            let body_entry = self.alloc_block();
            self.finish_block(
                context.blocks,
                then_target,
                then_scope,
                if_pattern.then_branch.span,
                then_ops,
                FunctionTerminator::Branch {
                    target: body_entry,
                    span: if_pattern.then_branch.span,
                },
            );
            body_entry
        };
        self.lower_body_into(
            &if_pattern.then_branch,
            body_entry,
            then_scope,
            context.blocks,
            Fallthrough::StoreThenBranch {
                local_id: result_local,
                target: merge_target,
            },
        );

        if let Some(else_branch) = &if_pattern.else_branch {
            let mut else_current = else_target;
            let mut else_ops = Vec::new();
            let else_terminated = self.store_expr_result_or_effect(
                else_branch,
                result_local,
                context.scope,
                &mut else_current,
                &mut else_ops,
                context.blocks,
            );
            if !else_terminated {
                self.finish_block(
                    context.blocks,
                    else_current,
                    context.scope,
                    else_branch.span,
                    else_ops,
                    FunctionTerminator::Branch {
                        target: merge_target,
                        span: else_branch.span,
                    },
                );
            }
        }

        *context.current = merge_target;
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind: FunctionExprKind::Local(result_local),
        }
    }

    pub(super) fn lower_value_if_pattern_chain_expr(
        &mut self,
        expr: &TypedExpr,
        chain: &nia_body_ir::TypedIfPatternChain,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionExpr {
        let result_local = self.alloc_temp_local(expr.span, expr.ty);
        let merge_target = self.alloc_block();
        let failure_target = if chain.else_branch.is_some() {
            self.alloc_block()
        } else {
            merge_target
        };
        let then_scope = self.alloc_scope(Some(scope), chain.then_branch.span);
        let (success_target, success_ops) = self.lower_if_pattern_chain_entry(
            &chain.clauses,
            scope,
            then_scope,
            failure_target,
            current,
            ops,
            blocks,
        );
        let body_entry = if success_ops.is_empty() {
            success_target
        } else {
            let body_entry = self.alloc_block();
            self.finish_block(
                blocks,
                success_target,
                then_scope,
                chain.then_branch.span,
                success_ops,
                FunctionTerminator::Branch {
                    target: body_entry,
                    span: chain.then_branch.span,
                },
            );
            body_entry
        };
        self.lower_body_into(
            &chain.then_branch,
            body_entry,
            then_scope,
            blocks,
            Fallthrough::StoreThenBranch {
                local_id: result_local,
                target: merge_target,
            },
        );
        if let Some(else_branch) = &chain.else_branch {
            let mut else_current = failure_target;
            let mut else_ops = Vec::new();
            let terminated = self.store_expr_result_or_effect(
                else_branch,
                result_local,
                scope,
                &mut else_current,
                &mut else_ops,
                blocks,
            );
            if !terminated {
                self.finish_block(
                    blocks,
                    else_current,
                    scope,
                    else_branch.span,
                    else_ops,
                    FunctionTerminator::Branch {
                        target: merge_target,
                        span: else_branch.span,
                    },
                );
            }
        }
        *current = merge_target;
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind: FunctionExprKind::Local(result_local),
        }
    }

    pub(super) fn lower_value_match_expr(
        &mut self,
        expr: &TypedExpr,
        matched: &TypedMatch,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionExpr {
        if self.match_requires_pattern_chain(matched) {
            let mut context = LoweringContext {
                scope,
                current,
                ops,
                blocks,
            };
            return self.lower_value_match_expr_as_chain(expr, matched, &mut context);
        }
        let target = self.lower_value_expr(&matched.target, scope, current, ops, blocks);
        let target = self.direct_match_target(matched, target);
        let local = self.alloc_temp_local(expr.span, expr.ty);
        let merge_target = self.alloc_block();
        let mut arms = Vec::new();
        let mut lowered_arms = Vec::new();
        let mut default = None;
        for arm in &matched.arms {
            let arm_target = self.alloc_block();
            for pattern in &arm.patterns {
                match &pattern.kind {
                    TypedPatternKind::Expr(pattern) => arms.push(FunctionSwitchArm {
                        pattern: self.lower_value_expr(pattern, scope, current, ops, blocks),
                        target: arm_target,
                    }),
                    TypedPatternKind::CheckedInt { value } => arms.push(FunctionSwitchArm {
                        pattern: self.checked_int_pattern_expr(*value, pattern.ty, pattern.span),
                        target: arm_target,
                    }),
                    TypedPatternKind::Nominal { .. } => {
                        arms.push(FunctionSwitchArm {
                            pattern: self
                                .direct_enum_match_pattern(pattern)
                                .expect("payload enum patterns require condition-chain lowering"),
                            target: arm_target,
                        });
                    }
                    TypedPatternKind::Wildcard => default = Some(arm_target),
                    TypedPatternKind::Range { .. }
                    | TypedPatternKind::CheckedIntRange { .. }
                    | TypedPatternKind::Bind { .. }
                    | TypedPatternKind::Pointer(_)
                    | TypedPatternKind::MutPointer(_)
                    | TypedPatternKind::OptionalSome(_)
                    | TypedPatternKind::OptionalNull
                    | TypedPatternKind::ErrorOk(_)
                    | TypedPatternKind::ErrorErr(_)
                    | TypedPatternKind::Tuple(_) => {}
                }
            }
            lowered_arms.push((arm_target, arm));
        }

        self.finish_block(
            blocks,
            *current,
            scope,
            expr.span,
            std::mem::take(ops),
            FunctionTerminator::Switch {
                target,
                arms,
                default,
                fallback: merge_target,
                span: expr.span,
            },
        );

        for (arm_target, arm) in lowered_arms {
            let arm_scope = self.alloc_scope(Some(scope), arm.span);
            let mut arm_context = MatchValueArmContext {
                span: arm.span,
                scope: arm_scope,
                entry: arm_target,
                result_local: local,
                merge_target,
                blocks,
            };
            self.lower_value_match_arm_body(&arm.body, &mut arm_context);
        }

        *current = merge_target;
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind: FunctionExprKind::Local(local),
        }
    }

    fn lower_value_match_expr_as_chain(
        &mut self,
        expr: &TypedExpr,
        matched: &TypedMatch,
        context: &mut LoweringContext<'_>,
    ) -> FunctionExpr {
        let target_value = self.lower_value_expr(
            &matched.target,
            context.scope,
            context.current,
            context.ops,
            context.blocks,
        );
        let target_local = self.alloc_temp_local(matched.target.span, matched.target.ty);
        context.ops.push(FunctionOp::StoreLocal {
            local_id: target_local,
            value: target_value,
            span: matched.target.span,
        });
        let target = FunctionExpr {
            span: matched.target.span,
            ty: matched.target.ty,
            kind: FunctionExprKind::Local(target_local),
        };
        let result_local = self.alloc_temp_local(expr.span, expr.ty);
        let merge_target = self.alloc_block();
        let mut lowered_arms = Vec::new();
        let mut tests = Vec::new();
        let mut default = merge_target;
        for arm in &matched.arms {
            let arm_target = self.alloc_block();
            for pattern in &arm.patterns {
                if matches!(&pattern.kind, TypedPatternKind::Wildcard) {
                    default = arm_target;
                } else {
                    tests.push((pattern, arm_target));
                }
            }
            lowered_arms.push((arm_target, arm));
        }
        let check_blocks = tests.iter().map(|_| self.alloc_block()).collect::<Vec<_>>();
        let first_target = check_blocks.first().copied().unwrap_or(default);
        self.finish_block(
            context.blocks,
            *context.current,
            context.scope,
            expr.span,
            std::mem::take(context.ops),
            FunctionTerminator::Branch {
                target: first_target,
                span: expr.span,
            },
        );
        for (index, ((pattern, arm_target), check_block)) in
            tests.iter().zip(check_blocks.iter()).enumerate()
        {
            let mut check_ops = Vec::new();
            let mut check_current = *check_block;
            let mut condition_context = PatternConditionContext {
                scope: context.scope,
                current: &mut check_current,
                ops: &mut check_ops,
                blocks: context.blocks,
                bool_ty: matched.bool_ty,
            };
            let cond = self
                .pattern_condition(&target, pattern, &mut condition_context)
                .unwrap_or(FunctionExpr {
                    span: expr.span,
                    ty: matched.bool_ty,
                    kind: FunctionExprKind::Bool(true),
                });
            let else_target = check_blocks.get(index + 1).copied().unwrap_or(default);
            self.finish_block(
                context.blocks,
                check_current,
                context.scope,
                expr.span,
                check_ops,
                FunctionTerminator::If {
                    cond,
                    then_target: *arm_target,
                    else_target,
                    span: expr.span,
                },
            );
        }
        for (arm_target, arm) in lowered_arms {
            let arm_scope = self.alloc_scope(Some(context.scope), arm.span);
            let arm_entry = arm.patterns.first().map_or(arm_target, |pattern| {
                self.lower_pattern_arm_entry(
                    pattern,
                    &target,
                    arm_scope,
                    arm_target,
                    arm.span,
                    context.blocks,
                )
            });
            let mut arm_context = MatchValueArmContext {
                span: arm.span,
                scope: arm_scope,
                entry: arm_entry,
                result_local,
                merge_target,
                blocks: context.blocks,
            };
            self.lower_value_match_arm_body(&arm.body, &mut arm_context);
        }
        *context.current = merge_target;
        FunctionExpr {
            span: expr.span,
            ty: expr.ty,
            kind: FunctionExprKind::Local(result_local),
        }
    }

    pub(super) fn lower_value_match_arm_body(
        &mut self,
        body: &TypedMatchArmBody,
        context: &mut MatchValueArmContext<'_>,
    ) {
        match body {
            TypedMatchArmBody::Expr(expr) => {
                let mut current = context.entry;
                let mut ops = Vec::new();
                let terminated = self.store_expr_result_or_effect(
                    expr,
                    context.result_local,
                    context.scope,
                    &mut current,
                    &mut ops,
                    context.blocks,
                );
                if !terminated {
                    self.finish_block(
                        context.blocks,
                        current,
                        context.scope,
                        context.span,
                        ops,
                        FunctionTerminator::Branch {
                            target: context.merge_target,
                            span: context.span,
                        },
                    );
                }
            }
            TypedMatchArmBody::Stmt(stmt) => {
                let mut current = context.entry;
                let mut ops = Vec::new();
                if !self.lower_stmt_into(
                    stmt,
                    context.scope,
                    &mut current,
                    &mut ops,
                    context.blocks,
                ) {
                    self.finish_block(
                        context.blocks,
                        current,
                        context.scope,
                        context.span,
                        ops,
                        FunctionTerminator::Branch {
                            target: context.merge_target,
                            span: context.span,
                        },
                    );
                }
            }
            TypedMatchArmBody::Block(body) => {
                self.lower_body_into(
                    body,
                    context.entry,
                    context.scope,
                    context.blocks,
                    Fallthrough::StoreThenBranch {
                        local_id: context.result_local,
                        target: context.merge_target,
                    },
                );
            }
        }
    }

    pub(super) fn lower_array_elements(
        &mut self,
        elems: &TypedArrayElements,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionArrayElements {
        match elems {
            TypedArrayElements::List(elems) => FunctionArrayElements::List(
                elems
                    .iter()
                    .map(|elem| self.lower_value_expr(elem, scope, current, ops, blocks))
                    .collect(),
            ),
            TypedArrayElements::Repeat { value, count } => FunctionArrayElements::Repeat {
                value: Box::new(self.lower_value_expr(value, scope, current, ops, blocks)),
                count: count.clone(),
            },
        }
    }

    pub(super) fn lower_callee(
        &mut self,
        callee: &TypedCallee,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionCallee {
        match callee {
            TypedCallee::Tracked { callee, location } => FunctionCallee::Tracked {
                callee: Box::new(self.lower_callee(callee, scope, current, ops, blocks)),
                location: location.clone(),
            },
            TypedCallee::Closure(callee) => {
                let closure_id = match self.types.get(callee.ty) {
                    Some(TyKind::ClosureState { closure_id, .. }) => *closure_id,
                    _ => unreachable!("typed closure callee must have closure-state type"),
                };
                let state_ptr_ty = self.types.intern(TyKind::Pointer {
                    is_readonly: true,
                    elem: callee.ty,
                });
                let state = FunctionExpr {
                    span: callee.span,
                    ty: state_ptr_ty,
                    kind: FunctionExprKind::AddrOf(
                        self.lower_expr_place(callee, scope, current, ops, blocks),
                    ),
                };
                FunctionCallee::ClosureEntry {
                    closure_id,
                    state: Box::new(state),
                }
            }
            TypedCallee::Function(def_id) => FunctionCallee::Function(*def_id),
            TypedCallee::FunctionInstance {
                def_id,
                arg_module_id,
                args,
                const_args,
            } => FunctionCallee::FunctionInstance {
                def_id: *def_id,
                arg_module_id: *arg_module_id,
                self_arg: None,
                args: args.clone(),
                const_args: const_args.clone(),
            },
            TypedCallee::Method {
                def_id,
                args,
                receiver_kind,
                receiver,
                const_args,
            } => FunctionCallee::Method {
                def_id: *def_id,
                arg_module_id: self.module_id,
                self_arg: None,
                args: args.clone(),
                const_args: const_args.clone(),
                receiver_kind: *receiver_kind,
                receiver: Box::new(self.lower_value_expr(receiver, scope, current, ops, blocks)),
            },
            TypedCallee::TraitMethod {
                trait_id,
                method_id,
                implementation_method: _,
                method_name,
                self_ty,
                trait_args,
                trait_const_args,
                args,
                const_args,
                receiver_kind,
                receiver,
            } => FunctionCallee::TraitMethod {
                trait_id: *trait_id,
                method_id: *method_id,
                method_name: *method_name,
                self_ty: *self_ty,
                trait_args: trait_args.clone(),
                trait_const_args: trait_const_args.clone(),
                args: args.clone(),
                const_args: const_args.clone(),
                receiver_kind: *receiver_kind,
                receiver: Box::new(self.lower_value_expr(receiver, scope, current, ops, blocks)),
            },
            TypedCallee::TraitAssociatedFunction {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                trait_const_args,
                args,
                const_args,
            } => FunctionCallee::TraitAssociatedFunction {
                trait_id: *trait_id,
                method_id: *method_id,
                method_name: *method_name,
                self_ty: *self_ty,
                trait_args: trait_args.clone(),
                trait_const_args: trait_const_args.clone(),
                args: args.clone(),
                const_args: const_args.clone(),
            },
            TypedCallee::DynamicTraitMethod {
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
                object_ty: *object_ty,
                trait_id: *trait_id,
                method_id: *method_id,
                method_name: *method_name,
                trait_args: trait_args.clone(),
                trait_const_args: trait_const_args.clone(),
                slot: *slot,
                params: params.clone(),
                return_type: *return_type,
                receiver_kind: *receiver_kind,
                receiver: Box::new(self.lower_value_expr(receiver, scope, current, ops, blocks)),
            },
            TypedCallee::BuiltinMethod {
                method,
                self_ty,
                receiver,
            } => FunctionCallee::BuiltinMethod {
                method: Self::lower_builtin_method(*method),
                self_ty: *self_ty,
                receiver: Box::new(self.lower_value_expr(receiver, scope, current, ops, blocks)),
            },
            TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                trait_id,
                method,
                self_ty,
                trait_args,
                receiver,
            }) => FunctionCallee::BuiltinPlaceMethod {
                trait_id: *trait_id,
                method: *method,
                self_ty: *self_ty,
                trait_args: trait_args.clone(),
                receiver: Box::new(self.lower_value_expr(receiver, scope, current, ops, blocks)),
            },
            TypedCallee::BuiltinOperator(operator) => {
                FunctionCallee::BuiltinOperator(FunctionBuiltinOperator {
                    trait_id: operator.trait_id,
                    op: match operator.op {
                        nia_sema_ir::BuiltinOperatorOp::Unary(op) => {
                            FunctionBuiltinOperatorOp::Unary(op)
                        }
                        nia_sema_ir::BuiltinOperatorOp::Binary(op) => {
                            FunctionBuiltinOperatorOp::Binary(op)
                        }
                    },
                })
            }
            TypedCallee::Callable(expr) => FunctionCallee::Callable(Box::new(
                self.lower_value_expr(expr, scope, current, ops, blocks),
            )),
            TypedCallee::FunctionPointer(expr) => FunctionCallee::FunctionPointer(Box::new(
                self.lower_value_expr(expr, scope, current, ops, blocks),
            )),
        }
    }

    fn lower_builtin_method(method: BuiltinMethod) -> FunctionBuiltinMethod {
        match method {
            BuiltinMethod::SliceLen => FunctionBuiltinMethod::SliceLen,
            BuiltinMethod::SlicePtr => FunctionBuiltinMethod::SlicePtr,
            BuiltinMethod::SlicePtrMut => FunctionBuiltinMethod::SlicePtrMut,
            BuiltinMethod::Start => FunctionBuiltinMethod::Start,
            BuiltinMethod::End => FunctionBuiltinMethod::End,
            BuiltinMethod::Iter => FunctionBuiltinMethod::Iter,
        }
    }

    pub(super) fn lower_slice_range(
        &mut self,
        range: &TypedSliceRange,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionSliceRange {
        FunctionSliceRange {
            start: range
                .start
                .as_ref()
                .map(|expr| Box::new(self.lower_value_expr(expr, scope, current, ops, blocks))),
            end: range
                .end
                .as_ref()
                .map(|expr| Box::new(self.lower_value_expr(expr, scope, current, ops, blocks))),
            inclusive: range.inclusive,
        }
    }

    pub(super) fn lower_range(
        &mut self,
        range: &TypedRange,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionRange {
        FunctionRange {
            start: range
                .start
                .as_ref()
                .map(|expr| Box::new(self.lower_value_expr(expr, scope, current, ops, blocks))),
            end: range
                .end
                .as_ref()
                .map(|expr| Box::new(self.lower_value_expr(expr, scope, current, ops, blocks))),
            inclusive: range.inclusive,
        }
    }

    pub(super) fn lower_inline_asm(
        &mut self,
        asm: &TypedInlineAsm,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionInlineAsm {
        FunctionInlineAsm {
            code: asm.code.clone(),
            inputs: asm
                .inputs
                .iter()
                .map(|input| FunctionAsmInput {
                    constraint: input.constraint.clone(),
                    value: self.lower_value_expr(&input.value, scope, current, ops, blocks),
                    span: input.span,
                })
                .collect(),
            outputs: asm
                .outputs
                .iter()
                .map(|output| FunctionAsmOutput {
                    constraint: output.constraint.clone(),
                    place: self.lower_place(&output.place, scope, current, ops, blocks),
                    span: output.span,
                })
                .collect(),
            clobbers: asm.clobbers.clone(),
            options: asm.options.iter().map(Self::lower_asm_option).collect(),
        }
    }
}

impl FunctionLowerer<'_> {
    fn lower_bit_intrinsic_op(
        &self,
        op: nia_body_ir::TypedBitIntrinsicOp,
    ) -> FunctionBitIntrinsicOp {
        match op {
            nia_body_ir::TypedBitIntrinsicOp::Ctz => FunctionBitIntrinsicOp::Ctz,
            nia_body_ir::TypedBitIntrinsicOp::Clz => FunctionBitIntrinsicOp::Clz,
            nia_body_ir::TypedBitIntrinsicOp::Popcount => FunctionBitIntrinsicOp::Popcount,
        }
    }

    fn function_pointer_pointee_ty(&self, ty: InternedTyId) -> Option<InternedTyId> {
        match self.types.get(ty) {
            Some(TyKind::FunctionPointer { .. }) => Some(ty),
            Some(TyKind::Pointer { elem, .. })
                if matches!(self.types.get(*elem), Some(TyKind::FunctionPointer { .. })) =>
            {
                Some(*elem)
            }
            _ => None,
        }
    }
}

fn lower_atomic_order(order: nia_body_ir::AtomicOrder) -> AtomicOrder {
    match order {
        nia_body_ir::AtomicOrder::Unordered => AtomicOrder::Unordered,
        nia_body_ir::AtomicOrder::Monotonic => AtomicOrder::Monotonic,
        nia_body_ir::AtomicOrder::Acquire => AtomicOrder::Acquire,
        nia_body_ir::AtomicOrder::Release => AtomicOrder::Release,
        nia_body_ir::AtomicOrder::AcqRel => AtomicOrder::AcqRel,
        nia_body_ir::AtomicOrder::SeqCst => AtomicOrder::SeqCst,
    }
}

fn lower_atomic_rmw_op(op: nia_body_ir::AtomicRmwOp) -> AtomicRmwOp {
    match op {
        nia_body_ir::AtomicRmwOp::Xchg => AtomicRmwOp::Xchg,
        nia_body_ir::AtomicRmwOp::Add => AtomicRmwOp::Add,
        nia_body_ir::AtomicRmwOp::Sub => AtomicRmwOp::Sub,
        nia_body_ir::AtomicRmwOp::And => AtomicRmwOp::And,
        nia_body_ir::AtomicRmwOp::Nand => AtomicRmwOp::Nand,
        nia_body_ir::AtomicRmwOp::Or => AtomicRmwOp::Or,
        nia_body_ir::AtomicRmwOp::Xor => AtomicRmwOp::Xor,
        nia_body_ir::AtomicRmwOp::Max => AtomicRmwOp::Max,
        nia_body_ir::AtomicRmwOp::Min => AtomicRmwOp::Min,
        nia_body_ir::AtomicRmwOp::UMax => AtomicRmwOp::UMax,
        nia_body_ir::AtomicRmwOp::UMin => AtomicRmwOp::UMin,
    }
}
