// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::{
    ArrayElements, AssignOp, BindingStmt, Block, Expr, ExprKind, ForHeader, ForInit, IndexArg,
    SliceRange, Stmt, StmtKind, SwitchArmBody, SwitchPattern, UnaryOp,
};
use nia_body_ir::{
    BracketSuffixResolution, BuiltinConst, BuiltinOperator, BuiltinOperatorOp, BuiltinPlaceMethod,
    BuiltinValue, PlaceBase, PlaceElem, ResolvedCall, TypedArrayElements, TypedBinding, TypedBody,
    TypedCallee, TypedExpr, TypedExprKind, TypedFieldInit, TypedFor, TypedForHeader, TypedForInit,
    TypedLocal, TypedLocalKind, TypedPlace, TypedRange, TypedSliceRange, TypedStmt, TypedStmtKind,
    TypedSwitch, TypedSwitchArm, TypedSwitchArmBody, TypedSwitchPattern,
};
use nia_ids::{BuiltinReceiverKind, BuiltinTraitMethod, TraitId};
use nia_local_resolve::{LocalKind, LocalUse};
use nia_span::Span;
use nia_trait_solve::TraitResolution;
use nia_ty::{BuiltinTrait, TyKind};
use nia_value_resolve::ValueNameResolution;

use crate::literals::{
    decode_byte_string_literal, decode_c_string_literal, decode_char_literal,
    decode_string_literal, numeric_literal_body,
};

mod asm;

impl<'a> BodyChecker<'a> {
    pub(crate) fn lower_body(&mut self, block: &Block) -> TypedBody {
        let stmts = block
            .stmts
            .iter()
            .filter(|stmt| {
                !matches!(
                    stmt.kind,
                    StmtKind::Using(_)
                        | StmtKind::Binding(BindingStmt {
                            is_comptime: true,
                            ..
                        })
                )
            })
            .map(|stmt| self.lower_stmt(stmt))
            .collect();
        let tail = block
            .tail
            .as_ref()
            .map(|tail| Box::new(self.lower_expr(tail)));
        let ty = tail
            .as_ref()
            .map(|tail| tail.ty)
            .unwrap_or_else(|| self.void());
        TypedBody {
            span: block.span,
            locals: self.lower_locals(block.span),
            stmts,
            tail,
            ty,
        }
    }

    fn lower_locals(&self, body_span: Span) -> Vec<TypedLocal> {
        self.locals
            .locals
            .iter()
            .filter(|(id, local)| {
                local.kind != LocalKind::ComptimeBinding
                    && (self.current_param_locals.contains(id)
                        || (body_span.start <= local.span.start && local.span.end <= body_span.end))
            })
            .map(|(id, local)| TypedLocal {
                id,
                name: local.name.clone(),
                kind: match local.kind {
                    LocalKind::Param => TypedLocalKind::Param,
                    LocalKind::Binding => TypedLocalKind::Binding,
                    LocalKind::ConstBinding => TypedLocalKind::ConstBinding,
                    LocalKind::ComptimeBinding => unreachable!("comptime locals are not lowered"),
                },
                ty: self
                    .local_types
                    .get(&id)
                    .copied()
                    .unwrap_or_else(|| self.error()),
                span: local.span,
            })
            .collect()
    }

    fn lower_stmt(&mut self, stmt: &Stmt) -> TypedStmt {
        let kind = match &stmt.kind {
            StmtKind::Using(_) => TypedStmtKind::Expr(TypedExpr {
                span: stmt.span,
                ty: self.error(),
                kind: TypedExprKind::Error,
            }),
            StmtKind::Binding(binding) => self
                .lower_binding_stmt(stmt.span, binding)
                .map(TypedStmtKind::Binding)
                .unwrap_or_else(|| TypedStmtKind::Expr(self.error_expr(stmt.span))),
            StmtKind::Expr(expr) => TypedStmtKind::Expr(self.lower_expr(expr)),
            StmtKind::Return(value) => {
                TypedStmtKind::Return(value.as_ref().map(|value| self.lower_expr(value)))
            }
            StmtKind::Break => TypedStmtKind::Break,
            StmtKind::Continue => TypedStmtKind::Continue,
            StmtKind::Defer(expr) => TypedStmtKind::Defer(self.lower_expr(expr)),
            StmtKind::For(for_stmt) => TypedStmtKind::For(Box::new(TypedFor {
                header: self.lower_for_header(&for_stmt.header),
                body: self.lower_body(&for_stmt.body),
            })),
        };
        TypedStmt {
            span: stmt.span,
            kind,
        }
    }

    fn lower_binding_stmt(&mut self, span: Span, binding: &BindingStmt) -> Option<TypedBinding> {
        let local_id = self.locals.local_defs.get(&span).copied()?;
        let ty = self.local_types.get(&local_id).copied().unwrap_or_else(|| {
            binding.ty.as_ref().map_or_else(
                || {
                    binding
                        .value
                        .as_ref()
                        .and_then(|value| self.expr_types.get(&value.span).copied())
                        .unwrap_or_else(|| self.error())
                },
                |ty| self.ty_for_span(ty.span),
            )
        });
        Some(TypedBinding {
            local_id,
            name: binding.name.clone(),
            ty,
            value: binding.value.as_ref().map(|value| {
                if matches!(
                    &value.kind,
                    ExprKind::Block(block) if block.stmts.is_empty() && block.tail.is_none()
                ) {
                    self.lower_expr_with_ty(value, Some(ty))
                } else {
                    self.lower_expr(value)
                }
            }),
            is_const: binding.is_const,
        })
    }

    fn error_expr(&mut self, span: Span) -> TypedExpr {
        TypedExpr {
            span,
            ty: self.error(),
            kind: TypedExprKind::Error,
        }
    }

    fn lower_for_header(&mut self, header: &ForHeader) -> TypedForHeader {
        match header {
            ForHeader::Infinite => TypedForHeader::Infinite,
            ForHeader::Condition(cond) => TypedForHeader::Condition(self.lower_expr(cond)),
            ForHeader::CStyle { init, cond, step } => TypedForHeader::CStyle {
                init: init.as_ref().map(|init| {
                    Box::new(match &**init {
                        ForInit::Binding { span, binding } => {
                            if binding.is_comptime {
                                TypedForInit::Expr(TypedExpr {
                                    span: *span,
                                    ty: self.void(),
                                    kind: TypedExprKind::Error,
                                })
                            } else {
                                self.lower_binding_stmt(*span, binding)
                                    .map(TypedForInit::Binding)
                                    .unwrap_or_else(|| TypedForInit::Expr(self.error_expr(*span)))
                            }
                        }
                        ForInit::Expr(expr) => TypedForInit::Expr(self.lower_expr(expr)),
                    })
                }),
                cond: cond.as_ref().map(|cond| Box::new(self.lower_expr(cond))),
                step: step.as_ref().map(|step| Box::new(self.lower_expr(step))),
            },
        }
    }

    fn lower_switch(&mut self, switch: &nia_ast::SwitchStmt) -> TypedSwitch {
        TypedSwitch {
            target: self.lower_expr(&switch.target),
            arms: switch
                .arms
                .iter()
                .map(|arm| TypedSwitchArm {
                    pattern: match &arm.pattern {
                        SwitchPattern::Default => TypedSwitchPattern::Default,
                        SwitchPattern::Expr(expr) => {
                            TypedSwitchPattern::Expr(self.lower_expr(expr))
                        }
                    },
                    body: match &arm.body {
                        SwitchArmBody::Expr(expr) => {
                            TypedSwitchArmBody::Expr(self.lower_expr(expr))
                        }
                        SwitchArmBody::Stmt(stmt) => {
                            TypedSwitchArmBody::Stmt(Box::new(self.lower_stmt(stmt)))
                        }
                        SwitchArmBody::Block(block) => {
                            TypedSwitchArmBody::Block(Box::new(self.lower_body(block)))
                        }
                    },
                    span: arm.span,
                })
                .collect(),
        }
    }

    pub(crate) fn lower_expr(&mut self, expr: &Expr) -> TypedExpr {
        self.lower_expr_with_ty(expr, None)
    }

    fn lower_expr_with_ty(
        &mut self,
        expr: &Expr,
        forced_ty: Option<nia_ids::InternedTyId>,
    ) -> TypedExpr {
        if forced_ty.is_none()
            && let Some(coercion) = self.c_string_pointer_coercions.get(&expr.span).copied()
        {
            return TypedExpr {
                span: expr.span,
                ty: coercion.pointer_ty,
                kind: TypedExprKind::CStringPointer {
                    array: Box::new(self.lower_expr_with_ty(expr, Some(coercion.array_ty))),
                    is_const: coercion.is_const,
                },
            };
        }
        if forced_ty.is_none()
            && let Some(coercion) = self.array_to_slice_coercions.get(&expr.span).copied()
        {
            return TypedExpr {
                span: expr.span,
                ty: coercion.slice_ty,
                kind: TypedExprKind::Slice {
                    lhs: Box::new(self.lower_expr_with_ty(expr, Some(coercion.array_ty))),
                    range: TypedSliceRange {
                        start: None,
                        end: None,
                        inclusive: false,
                    },
                    is_const: coercion.is_const,
                },
            };
        }
        let ty = forced_ty
            .or_else(|| self.expr_types.get(&expr.span).copied())
            .unwrap_or_else(|| self.error());
        if let Some(def_id) = self.global_comptime_use(expr.span) {
            return self.lower_comptime_value_expr(
                expr.span,
                ty,
                self.global_comptime_value(def_id),
            );
        }
        if let Some(variant_id) = self.qualified_enum_variant(expr) {
            return TypedExpr {
                span: expr.span,
                ty,
                kind: TypedExprKind::EnumVariant(variant_id),
            };
        }
        if let Some(def_id) = self.values.qualified_values.get(&expr.span).copied() {
            let kind = match self.global_def_kind(def_id) {
                Some(nia_defs::DefKind::Function | nia_defs::DefKind::Method) => {
                    TypedExprKind::Function(def_id)
                }
                Some(nia_defs::DefKind::Global) => TypedExprKind::Global(def_id),
                Some(nia_defs::DefKind::Comptime) => {
                    return self.lower_comptime_value_expr(
                        expr.span,
                        ty,
                        self.global_comptime_value(def_id),
                    );
                }
                _ => TypedExprKind::Error,
            };
            return TypedExpr {
                span: expr.span,
                ty,
                kind,
            };
        }
        let kind = match &expr.kind {
            ExprKind::Error | ExprKind::Raw(_) | ExprKind::Underscore => TypedExprKind::Error,
            ExprKind::Integer(text) => {
                TypedExprKind::Integer(numeric_literal_body(text).to_string())
            }
            ExprKind::Float(text) => TypedExprKind::Float(numeric_literal_body(text).to_string()),
            ExprKind::String(literal) => {
                TypedExprKind::String(decode_string_literal(literal).unwrap_or_default())
            }
            ExprKind::ByteString(literal) => {
                TypedExprKind::ByteString(decode_byte_string_literal(literal).unwrap_or_default())
            }
            ExprKind::CString(literal) => {
                TypedExprKind::ByteString(decode_c_string_literal(literal).unwrap_or_default())
            }
            ExprKind::Char(text) => TypedExprKind::Char(decode_char_literal(text).unwrap_or(0)),
            ExprKind::ByteChar(text) => TypedExprKind::ByteChar(text.clone()),
            ExprKind::Bool(value) => TypedExprKind::Bool(*value),
            ExprKind::Ident(_) => {
                if let Some(local_id) = self.local_comptime_use(expr.span) {
                    return self.lower_comptime_value_expr(
                        expr.span,
                        ty,
                        self.comptime
                            .values
                            .get(&nia_comptime_check::ComptimeKey::Local(local_id))
                            .cloned(),
                    );
                }
                self.lower_ident_expr(expr)
            }
            ExprKind::Builtin { .. } => match self.builtin_values.get(&expr.span) {
                Some(BuiltinValue::Usize(value)) => {
                    TypedExprKind::BuiltinValue(BuiltinConst::Usize(*value))
                }
                Some(BuiltinValue::Layout { builtin, ty }) => {
                    TypedExprKind::BuiltinValue(BuiltinConst::Layout {
                        builtin: *builtin,
                        ty: *ty,
                    })
                }
                None => TypedExprKind::Error,
            },
            ExprKind::TypeTarget { .. } => TypedExprKind::Error,
            ExprKind::BracketSuffix { callee, args } => {
                match self.bracket_suffix_resolution(expr.span) {
                    Some(BracketSuffixResolution::Index) => {
                        if let Some(index) = args.first().and_then(|arg| arg.expr.as_ref()) {
                            self.lower_index_read_expr(callee, index)
                                .unwrap_or_else(|| TypedExprKind::Index {
                                    lhs: Box::new(self.lower_expr(callee)),
                                    index: Box::new(self.lower_expr(index)),
                                })
                        } else {
                            TypedExprKind::Error
                        }
                    }
                    Some(BracketSuffixResolution::GenericCall) => {
                        if let Some(reference) = self.function_references.get(&expr.span) {
                            TypedExprKind::FunctionInstance {
                                def_id: reference.def_id,
                                args: reference.args.clone(),
                            }
                        } else {
                            TypedExprKind::Error
                        }
                    }
                    Some(BracketSuffixResolution::TypePrefixInstantiation) | None => {
                        TypedExprKind::Error
                    }
                }
            }
            ExprKind::Field { lhs, name } => {
                let lhs_expr = self.lower_expr(lhs);
                self.field_def_for_base_ty(lhs_expr.ty, name)
                    .map(|field| TypedExprKind::Field {
                        lhs: Box::new(lhs_expr),
                        field,
                    })
                    .unwrap_or(TypedExprKind::Error)
            }
            ExprKind::ArrayLiteral { elems } => TypedExprKind::ArrayLiteral {
                elems: self.lower_array_elements(elems),
            },
            ExprKind::StructLiteral { fields } => {
                let Some(def_id) = self.nominal_global_def(ty) else {
                    return TypedExpr {
                        span: expr.span,
                        ty,
                        kind: TypedExprKind::Error,
                    };
                };
                if self.is_union_def(def_id) {
                    let field = fields.first().map(|field| TypedFieldInit {
                        field: self.field_def_for_struct_ty(ty, &field.name),
                        name: field.name.clone(),
                        value: self.lower_expr(&field.value),
                        span: field.span,
                    });
                    TypedExprKind::UnionLiteral {
                        def_id,
                        field: Box::new(field.unwrap_or_else(|| TypedFieldInit {
                            field: None,
                            name: String::new(),
                            value: TypedExpr {
                                span: expr.span,
                                ty: self.error(),
                                kind: TypedExprKind::Error,
                            },
                            span: expr.span,
                        })),
                    }
                } else {
                    TypedExprKind::StructLiteral {
                        def_id,
                        fields: fields
                            .iter()
                            .map(|field| TypedFieldInit {
                                field: self.field_def_for_struct_ty(ty, &field.name),
                                name: field.name.clone(),
                                value: self.lower_expr(&field.value),
                                span: field.span,
                            })
                            .collect(),
                    }
                }
            }
            ExprKind::Unary { op, expr: inner }
                if let Some(trait_id) = BuiltinOperatorOp::Unary(*op).trait_id() =>
            {
                TypedExprKind::Call {
                    callee: TypedCallee::BuiltinOperator(BuiltinOperator {
                        trait_id,
                        op: BuiltinOperatorOp::Unary(*op),
                    }),
                    args: vec![self.lower_expr(inner)],
                }
            }
            ExprKind::Unary { op, expr: inner } => {
                if let ExprKind::Index {
                    lhs,
                    index: IndexArg::Range(range),
                } = &inner.kind
                    && matches!(op, UnaryOp::Ref | UnaryOp::RefConst)
                {
                    self.lower_slice_expr(lhs, range, matches!(op, UnaryOp::RefConst))
                } else if matches!(op, UnaryOp::Ref | UnaryOp::RefConst)
                    && let Some(function_item) = self.lower_function_item_ref(inner)
                {
                    TypedExprKind::Unary {
                        op: *op,
                        expr: Box::new(TypedExpr {
                            span: inner.span,
                            ty: self
                                .expr_types
                                .get(&inner.span)
                                .copied()
                                .unwrap_or_else(|| self.error()),
                            kind: function_item,
                        }),
                    }
                } else if matches!(op, UnaryOp::Deref)
                    && let Some(pointer) = self.lower_deref_read_pointer(inner)
                {
                    TypedExprKind::Unary {
                        op: *op,
                        expr: Box::new(pointer),
                    }
                } else {
                    TypedExprKind::Unary {
                        op: *op,
                        expr: Box::new(self.lower_expr(inner)),
                    }
                }
            }
            ExprKind::Binary { lhs, op, rhs }
                if let Some(trait_id) = BuiltinOperatorOp::Binary(*op).trait_id() =>
            {
                TypedExprKind::Call {
                    callee: TypedCallee::BuiltinOperator(BuiltinOperator {
                        trait_id,
                        op: BuiltinOperatorOp::Binary(*op),
                    }),
                    args: vec![self.lower_expr(lhs), self.lower_expr(rhs)],
                }
            }
            ExprKind::Binary { lhs, op, rhs } => TypedExprKind::Binary {
                lhs: Box::new(self.lower_expr(lhs)),
                op: *op,
                rhs: Box::new(self.lower_expr(rhs)),
            },
            ExprKind::Assign {
                lhs,
                op: AssignOp::Assign,
                rhs,
            } if matches!(lhs.kind, ExprKind::Underscore) => {
                TypedExprKind::Discard(Box::new(self.lower_expr(rhs)))
            }
            ExprKind::Assign { lhs, op, rhs } => TypedExprKind::Assign {
                place: self.lower_place(lhs),
                op: *op,
                rhs: Box::new(self.lower_expr(rhs)),
            },
            ExprKind::Cast { expr: inner, ty } => TypedExprKind::Cast {
                expr: Box::new(self.lower_expr(inner)),
                ty: self.ty_for_span(ty.span),
            },
            ExprKind::Call { callee, args } => {
                if let ExprKind::Builtin { name, .. } = &callee.kind {
                    match (name.as_str(), args.as_slice()) {
                        (_, []) => self.lower_expr(callee).kind,
                        ("asm", [arg]) => self.lower_inline_asm(arg),
                        _ => TypedExprKind::Call {
                            callee: self.lower_callee(expr.span, callee),
                            args: args.iter().map(|arg| self.lower_expr(arg)).collect(),
                        },
                    }
                } else if let Some(ResolvedCall::BuiltinPlaceMethod {
                    trait_id,
                    method,
                    self_ty,
                    trait_args,
                }) = self.resolved_calls.get(&expr.span).cloned()
                {
                    let receiver = self
                        .lower_receiver_expr(callee)
                        .unwrap_or_else(|| self.lower_expr(callee));
                    TypedExprKind::Call {
                        callee: TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                            trait_id,
                            method,
                            self_ty,
                            trait_args,
                            receiver: Box::new(self.lower_typed_builtin_place_method_receiver(
                                &receiver, self_ty, method,
                            )),
                        }),
                        args: args.iter().map(|arg| self.lower_expr(arg)).collect(),
                    }
                } else if let Some(ResolvedCall::BuiltinTraitMethod { trait_id, op }) =
                    self.resolved_calls.get(&expr.span).cloned()
                {
                    let lowered_args = if let Some(receiver) = self.lower_receiver_expr(callee) {
                        std::iter::once(receiver)
                            .chain(args.iter().map(|arg| self.lower_expr(arg)))
                            .collect()
                    } else {
                        args.iter().map(|arg| self.lower_expr(arg)).collect()
                    };
                    TypedExprKind::Call {
                        callee: TypedCallee::BuiltinOperator(BuiltinOperator { trait_id, op }),
                        args: lowered_args,
                    }
                } else {
                    TypedExprKind::Call {
                        callee: self.lower_callee(expr.span, callee),
                        args: args.iter().map(|arg| self.lower_expr(arg)).collect(),
                    }
                }
            }
            ExprKind::Qualified { lhs, name } => {
                if let Some(variant) = self
                    .qualified_enum_variant(expr)
                    .or_else(|| self.enum_variant_for_qualified(lhs, name))
                {
                    TypedExprKind::EnumVariant(variant)
                } else {
                    let lhs_expr = self.lower_expr(lhs);
                    self.field_def_for_base_ty(lhs_expr.ty, name)
                        .map(|field| TypedExprKind::Field {
                            lhs: Box::new(lhs_expr),
                            field,
                        })
                        .unwrap_or(TypedExprKind::Error)
                }
            }
            ExprKind::Index { lhs, index } => match index {
                IndexArg::Expr(index) => {
                    self.lower_index_read_expr(lhs, index)
                        .unwrap_or_else(|| TypedExprKind::Index {
                            lhs: Box::new(self.lower_expr(lhs)),
                            index: Box::new(self.lower_expr(index)),
                        })
                }
                IndexArg::Range(range) => self.lower_slice_read_expr(lhs, range),
            },
            ExprKind::Range(range) => TypedExprKind::Range(self.lower_range(range)),
            ExprKind::Block(block) if self.empty_struct_literal_expr(ty, block) => self
                .nominal_global_def(ty)
                .map(|def_id| TypedExprKind::StructLiteral {
                    def_id,
                    fields: Vec::new(),
                })
                .unwrap_or(TypedExprKind::Error),
            ExprKind::Block(block) => TypedExprKind::Block(self.lower_body(block)),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => TypedExprKind::If {
                cond: Box::new(self.lower_expr(cond)),
                then_branch: self.lower_body(then_branch),
                else_branch: else_branch
                    .as_ref()
                    .map(|else_branch| Box::new(self.lower_expr(else_branch))),
            },
            ExprKind::Switch(switch) => TypedExprKind::Switch(Box::new(self.lower_switch(switch))),
        };
        TypedExpr {
            span: expr.span,
            ty,
            kind,
        }
    }

    fn lower_comptime_value_expr(
        &self,
        span: Span,
        ty: nia_ids::InternedTyId,
        value: Option<nia_comptime_check::ComptimeValue>,
    ) -> TypedExpr {
        match value {
            Some(nia_comptime_check::ComptimeValue::Int(value)) => TypedExpr {
                span,
                ty,
                kind: TypedExprKind::BuiltinValue(BuiltinConst::Int(value)),
            },
            None => TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Error,
            },
        }
    }

    pub(crate) fn global_comptime_value(
        &self,
        def_id: nia_ids::GlobalDefId,
    ) -> Option<nia_comptime_check::ComptimeValue> {
        self.global_comptime_value_for_env(def_id, &mut Vec::new())
    }

    fn global_comptime_value_for_env(
        &self,
        def_id: nia_ids::GlobalDefId,
        active: &mut Vec<nia_ids::GlobalDefId>,
    ) -> Option<nia_comptime_check::ComptimeValue> {
        self.comptime
            .values
            .get(&nia_comptime_check::ComptimeKey::Global(def_id))
            .cloned()
            .or_else(|| self.eval_global_comptime_value(def_id, active))
    }

    fn eval_global_comptime_value(
        &self,
        def_id: nia_ids::GlobalDefId,
        active: &mut Vec<nia_ids::GlobalDefId>,
    ) -> Option<nia_comptime_check::ComptimeValue> {
        if active.contains(&def_id) {
            return None;
        }
        let value = self.global_comptime_initializer(def_id)?.clone();
        active.push(def_id);
        let mut env = BirComptimeEnv {
            checker: self,
            module_id: def_id.module_id,
            active,
        };
        let value = nia_comptime_engine::eval_expr(&value, &mut env).ok();
        active.pop();
        value
    }

    fn global_comptime_initializer(&self, def_id: nia_ids::GlobalDefId) -> Option<&Expr> {
        let defs = self.defs_for_module(def_id.module_id)?;
        let module = self.module_for_module(def_id.module_id)?;
        module.items.iter().find_map(|item| {
            let nia_ast::ItemKind::Binding(binding) = &item.kind else {
                return None;
            };
            if !binding.is_comptime {
                return None;
            }
            let item_def_id = defs.def_spans.get(item.span)?;
            (item_def_id == def_id.def_id)
                .then_some(binding.value.as_ref())
                .flatten()
        })
    }

    fn global_comptime_id_in_module(
        &self,
        module_id: nia_ids::ModuleId,
        span: Span,
    ) -> Option<nia_ids::GlobalDefId> {
        if let Some(global_id) = self.values.qualified_values.get(&span).copied()
            && self.global_def_kind(global_id) == Some(nia_defs::DefKind::Comptime)
        {
            return Some(global_id);
        }
        let module_defs = self.defs_for_module(module_id)?;
        let nia_value_resolve::ValueNameResolution::Def(def_id) = self.values.names.get(&span)?
        else {
            return None;
        };
        let def = module_defs.defs.get(*def_id)?;
        (def.kind == nia_defs::DefKind::Comptime).then_some(nia_ids::GlobalDefId {
            module_id,
            def_id: *def_id,
        })
    }

    fn empty_struct_literal_expr(&mut self, ty: nia_ids::InternedTyId, block: &Block) -> bool {
        if !block.stmts.is_empty() || block.tail.is_some() {
            return false;
        }
        let Some(TyKind::Nominal { def_id, .. }) = self.interner.get(ty) else {
            return false;
        };
        self.resolved_struct_signature(*def_id)
            .is_some_and(|resolved| resolved.signature.fields.is_empty())
    }

    pub(crate) fn bracket_suffix_resolution(&self, span: Span) -> Option<BracketSuffixResolution> {
        self.bracket_suffix_resolutions.get(&span).copied()
    }

    fn lower_deref_read_pointer(&mut self, expr: &Expr) -> Option<TypedExpr> {
        let ty = self
            .expr_types
            .get(&expr.span)
            .copied()
            .unwrap_or_else(|| self.error());
        self.lower_builtin_deref_method_call(expr, ty, false)
    }

    fn lower_index_read_expr(&mut self, lhs: &Expr, index: &Expr) -> Option<TypedExprKind> {
        let lhs_ty = self
            .expr_types
            .get(&lhs.span)
            .copied()
            .unwrap_or_else(|| self.error());
        let index_ty = self
            .expr_types
            .get(&index.span)
            .copied()
            .unwrap_or_else(|| self.error());
        let pointer = self.lower_builtin_index_method_call(lhs, index, lhs_ty, index_ty, false)?;
        Some(TypedExprKind::Unary {
            op: UnaryOp::Deref,
            expr: Box::new(pointer),
        })
    }

    fn nominal_global_def(&self, ty: nia_ids::InternedTyId) -> Option<nia_ids::GlobalDefId> {
        match self.interner.get(ty) {
            Some(TyKind::Nominal { def_id, .. }) => Some(*def_id),
            _ => None,
        }
    }

    pub(crate) fn field_def_for_struct_ty(
        &self,
        ty: nia_ids::InternedTyId,
        name: &str,
    ) -> Option<nia_ids::GlobalDefId> {
        let def_id = self.nominal_global_def(ty)?;
        self.field_def_for_nominal(def_id, name)
    }

    pub(crate) fn field_def_for_base_ty(
        &self,
        ty: nia_ids::InternedTyId,
        name: &str,
    ) -> Option<nia_ids::GlobalDefId> {
        let base = self.receiver_base_type(ty)?;
        self.field_def_for_nominal(base.def_id, name)
    }

    fn field_def_for_nominal(
        &self,
        def_id: nia_ids::GlobalDefId,
        name: &str,
    ) -> Option<nia_ids::GlobalDefId> {
        let defs = self.defs_for_module(def_id.module_id)?;
        defs.scopes
            .struct_members
            .get(&def_id.def_id)
            .and_then(|members| members.fields.get(name))
            .or_else(|| {
                defs.scopes
                    .union_members
                    .get(&def_id.def_id)
                    .and_then(|members| members.fields.get(name))
            })
            .map(|field| nia_ids::GlobalDefId {
                module_id: def_id.module_id,
                def_id: field,
            })
    }

    fn qualified_enum_variant(&self, expr: &Expr) -> Option<nia_ids::GlobalDefId> {
        self.values
            .variant_enums
            .contains_key(&expr.span)
            .then(|| self.values.qualified_values.get(&expr.span).copied())
            .flatten()
    }

    fn enum_variant_for_qualified(
        &mut self,
        lhs: &Expr,
        name: &str,
    ) -> Option<nia_ids::GlobalDefId> {
        let enum_id = self.type_prefix_def_id(lhs)?;
        if !self.is_enum_def(enum_id) {
            return None;
        }
        let scope = self.enum_variant_scope(enum_id)?;
        let variant_id = scope
            .iter()
            .find(|(variant_name, _)| variant_name == name)
            .map(|(_, def_id)| *def_id)?;
        Some(nia_ids::GlobalDefId {
            module_id: enum_id.module_id,
            def_id: variant_id,
        })
    }

    fn lower_ident_expr(&self, expr: &Expr) -> TypedExprKind {
        match self.locals.uses.get(&expr.span) {
            Some(LocalUse::Local(local)) => {
                if self
                    .locals
                    .locals
                    .get(*local)
                    .is_some_and(|local| local.kind == LocalKind::ComptimeBinding)
                {
                    return TypedExprKind::Error;
                }
                TypedExprKind::Local(*local)
            }
            Some(LocalUse::ModuleValue) => {
                if let Some(variant_id) = self.values.qualified_values.get(&expr.span).copied()
                    && self.values.variant_enums.contains_key(&expr.span)
                {
                    return TypedExprKind::EnumVariant(variant_id);
                }
                if let Some(global_id) = self.values.qualified_values.get(&expr.span).copied() {
                    match self.global_def_kind(global_id) {
                        Some(nia_defs::DefKind::Function | nia_defs::DefKind::Method) => {
                            return TypedExprKind::Function(global_id);
                        }
                        Some(nia_defs::DefKind::Global) => return TypedExprKind::Global(global_id),
                        Some(nia_defs::DefKind::Comptime) => return TypedExprKind::Error,
                        _ => return TypedExprKind::Error,
                    }
                }
                match self.values.names.get(&expr.span) {
                    Some(ValueNameResolution::Def(def_id)) => {
                        match self.defs.defs.get(*def_id).map(|def| def.kind) {
                            Some(nia_defs::DefKind::Function | nia_defs::DefKind::Method) => {
                                TypedExprKind::Function(self.global_def_id(*def_id))
                            }
                            Some(nia_defs::DefKind::Global) => {
                                TypedExprKind::Global(self.global_def_id(*def_id))
                            }
                            Some(nia_defs::DefKind::Comptime) => TypedExprKind::Error,
                            _ => TypedExprKind::Error,
                        }
                    }
                    _ => TypedExprKind::Error,
                }
            }
            _ => TypedExprKind::Error,
        }
    }

    fn lower_function_item_ref(&mut self, expr: &Expr) -> Option<TypedExprKind> {
        let reference = self.function_references.get(&expr.span)?;
        if reference.args.is_empty() {
            Some(TypedExprKind::Function(reference.def_id))
        } else {
            Some(TypedExprKind::FunctionInstance {
                def_id: reference.def_id,
                args: reference.args.clone(),
            })
        }
    }

    fn lower_slice_range(&mut self, range: &SliceRange) -> TypedSliceRange {
        TypedSliceRange {
            start: range
                .start
                .as_ref()
                .map(|start| Box::new(self.lower_expr(start))),
            end: range.end.as_ref().map(|end| Box::new(self.lower_expr(end))),
            inclusive: range.inclusive,
        }
    }

    fn lower_range(&mut self, range: &SliceRange) -> TypedRange {
        TypedRange {
            start: range
                .start
                .as_ref()
                .map(|start| Box::new(self.lower_expr(start))),
            end: range.end.as_ref().map(|end| Box::new(self.lower_expr(end))),
            inclusive: range.inclusive,
        }
    }

    fn lower_array_elements(&mut self, elems: &ArrayElements) -> TypedArrayElements {
        match elems {
            ArrayElements::List(elems) => {
                TypedArrayElements::List(elems.iter().map(|elem| self.lower_expr(elem)).collect())
            }
            ArrayElements::Repeat { value, count } => TypedArrayElements::Repeat {
                value: Box::new(self.lower_expr(value)),
                count: self.lower_array_repeat_count(count),
            },
        }
    }

    pub(crate) fn lower_array_repeat_count(&mut self, count: &Expr) -> u64 {
        nia_comptime_engine::eval_array_len_expr(count, self).unwrap_or(0)
    }

    fn lower_callee(&mut self, call_span: Span, callee: &Expr) -> TypedCallee {
        if let Some(resolved) = self.resolved_calls.get(&call_span).cloned() {
            return self.lower_resolved_callee(callee, resolved);
        }
        TypedCallee::FunctionPointer(Box::new(self.lower_expr(callee)))
    }

    fn lower_resolved_callee(&mut self, callee: &Expr, resolved: ResolvedCall) -> TypedCallee {
        match resolved {
            ResolvedCall::Function(def_id) => TypedCallee::Function(def_id),
            ResolvedCall::FunctionInstance { def_id, args } => {
                TypedCallee::FunctionInstance { def_id, args }
            }
            ResolvedCall::Method { def_id, args } => TypedCallee::Method {
                def_id,
                args,
                receiver: Box::new(
                    self.lower_receiver_expr(callee)
                        .unwrap_or_else(|| self.lower_expr(callee)),
                ),
            },
            ResolvedCall::TraitMethod {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
            } => TypedCallee::TraitMethod {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
                receiver: Box::new(
                    self.lower_receiver_expr(callee)
                        .unwrap_or_else(|| self.lower_expr(callee)),
                ),
            },
            ResolvedCall::BuiltinTraitMethod { trait_id, op } => {
                TypedCallee::BuiltinOperator(BuiltinOperator { trait_id, op })
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
            ResolvedCall::FunctionPointer => {
                TypedCallee::FunctionPointer(Box::new(self.lower_expr(callee)))
            }
        }
    }

    fn lower_receiver_expr(&mut self, callee: &Expr) -> Option<TypedExpr> {
        let field_callee = match &callee.kind {
            ExprKind::Field { .. } => callee,
            ExprKind::BracketSuffix {
                callee: generic_callee,
                ..
            } if matches!(
                self.bracket_suffix_resolution(callee.span),
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
        Some(self.lower_expr(lhs))
    }

    pub(crate) fn lower_place(&mut self, expr: &Expr) -> TypedPlace {
        let ty = self
            .expr_types
            .get(&expr.span)
            .copied()
            .unwrap_or_else(|| self.error());
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
        if self.values.variant_enums.contains_key(&expr.span) {
            return PlaceBase::Error;
        }
        if let Some(def_id) = self.values.qualified_values.get(&expr.span).copied() {
            return PlaceBase::Global(def_id);
        }
        match &expr.kind {
            ExprKind::Ident(_) => match self.locals.uses.get(&expr.span) {
                Some(LocalUse::Local(local)) => PlaceBase::Local(*local),
                Some(LocalUse::ModuleValue) => match self.values.names.get(&expr.span) {
                    Some(ValueNameResolution::Def(def_id)) => {
                        PlaceBase::Global(self.global_def_id(*def_id))
                    }
                    _ => PlaceBase::Error,
                },
                _ => PlaceBase::Error,
            },
            ExprKind::Unary {
                op: nia_ast::UnaryOp::Deref,
                expr,
            } => {
                let ty = self
                    .expr_types
                    .get(&expr.span)
                    .copied()
                    .unwrap_or_else(|| self.error());
                if let Some(pointer) = self.lower_builtin_deref_method_call(expr, ty, mutable) {
                    PlaceBase::Deref(Box::new(pointer))
                } else {
                    PlaceBase::Deref(Box::new(self.lower_expr(expr)))
                }
            }
            ExprKind::Field { lhs, name } | ExprKind::Qualified { lhs, name } => {
                let base = self.lower_place_inner(lhs, elems, mutable);
                let lhs_ty = self
                    .expr_types
                    .get(&lhs.span)
                    .copied()
                    .unwrap_or_else(|| self.error());
                let field = self
                    .field_def_for_base_ty(lhs_ty, name)
                    .map(PlaceElem::Field)
                    .unwrap_or(PlaceElem::Error);
                elems.push(field);
                base
            }
            ExprKind::Index { lhs, index } => {
                if let IndexArg::Expr(index) = index {
                    let lhs_ty = self
                        .expr_types
                        .get(&lhs.span)
                        .copied()
                        .unwrap_or_else(|| self.error());
                    let index_ty = self
                        .expr_types
                        .get(&index.span)
                        .copied()
                        .unwrap_or_else(|| self.error());
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
                    self.bracket_suffix_resolution(expr.span),
                    Some(BracketSuffixResolution::Index)
                ) {
                    if let Some(index) = args.first().and_then(|arg| arg.expr.as_ref()) {
                        let lhs_ty = self
                            .expr_types
                            .get(&callee.span)
                            .copied()
                            .unwrap_or_else(|| self.error());
                        let index_ty = self
                            .expr_types
                            .get(&index.span)
                            .copied()
                            .unwrap_or_else(|| self.error());
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

    fn lower_builtin_deref_method_call(
        &mut self,
        receiver: &Expr,
        receiver_ty: nia_ids::InternedTyId,
        mutable: bool,
    ) -> Option<TypedExpr> {
        let (trait_id, method, target_const) = if mutable {
            (BuiltinTrait::Deref, BuiltinTraitMethod::Deref, false)
        } else {
            (
                BuiltinTrait::DerefConst,
                BuiltinTraitMethod::DerefConst,
                true,
            )
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
            name: BuiltinTrait::TARGET_ASSOC_TYPE.to_string(),
        });
        let target = self.normalize_projection(target);
        let pointer_ty = self.interner.intern(TyKind::Pointer {
            is_const: target_const,
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

    fn lower_builtin_index_method_call(
        &mut self,
        receiver: &Expr,
        index: &Expr,
        receiver_ty: nia_ids::InternedTyId,
        index_ty: nia_ids::InternedTyId,
        mutable: bool,
    ) -> Option<TypedExpr> {
        let (trait_id, method, output_const) = if mutable {
            (BuiltinTrait::Index, BuiltinTraitMethod::Index, false)
        } else {
            (
                BuiltinTrait::IndexConst,
                BuiltinTraitMethod::IndexConst,
                true,
            )
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
            name: BuiltinTrait::OUTPUT_ASSOC_TYPE.to_string(),
        });
        let output = self.normalize_projection(output);
        let pointer_ty = self.interner.intern(TyKind::Pointer {
            is_const: output_const,
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

    fn lower_slice_read_expr(&mut self, lhs: &Expr, range: &SliceRange) -> TypedExprKind {
        self.lower_slice_expr(lhs, range, true)
    }

    fn lower_slice_expr(
        &mut self,
        lhs: &Expr,
        range: &SliceRange,
        is_const: bool,
    ) -> TypedExprKind {
        let lhs_ty = self
            .expr_types
            .get(&lhs.span)
            .copied()
            .unwrap_or_else(|| self.error());
        let range_ty = self.check_slice_range_bounds(range);
        let (trait_id, method) = if is_const {
            (BuiltinTrait::SliceConst, BuiltinTraitMethod::SliceConst)
        } else {
            (BuiltinTrait::Slice, BuiltinTraitMethod::Slice)
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
                is_const,
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
            args: vec![self.lower_range_as_expr(range, range_ty)],
        }
    }

    fn lower_range_as_expr(&mut self, range: &SliceRange, ty: nia_ids::InternedTyId) -> TypedExpr {
        TypedExpr {
            span: self.slice_range_span(range),
            ty,
            kind: TypedExprKind::Range(self.lower_range(range)),
        }
    }

    fn slice_range_span(&self, range: &SliceRange) -> Span {
        match (&range.start, &range.end) {
            (Some(start), Some(end)) => Span::new(start.span.start, end.span.end),
            (Some(start), None) => start.span,
            (None, Some(end)) => end.span,
            (None, None) => Span::default(),
        }
    }

    fn lower_builtin_place_method_receiver(
        &mut self,
        receiver: &Expr,
        receiver_ty: nia_ids::InternedTyId,
        method: BuiltinTraitMethod,
    ) -> TypedExpr {
        let receiver_kind = method
            .place_receiver_kind()
            .unwrap_or_else(|| method.receiver_kind());
        match receiver_kind {
            BuiltinReceiverKind::RefConst => {
                let pointer_ty = self.interner.intern(TyKind::Pointer {
                    is_const: true,
                    elem: receiver_ty,
                });
                TypedExpr {
                    span: receiver.span,
                    ty: pointer_ty,
                    kind: TypedExprKind::Unary {
                        op: UnaryOp::RefConst,
                        expr: Box::new(self.lower_expr(receiver)),
                    },
                }
            }
            BuiltinReceiverKind::Ref => {
                let pointer_ty = self.interner.intern(TyKind::Pointer {
                    is_const: false,
                    elem: receiver_ty,
                });
                TypedExpr {
                    span: receiver.span,
                    ty: pointer_ty,
                    kind: TypedExprKind::Unary {
                        op: UnaryOp::Ref,
                        expr: Box::new(self.lower_expr(receiver)),
                    },
                }
            }
            BuiltinReceiverKind::Value => self.lower_expr(receiver),
        }
    }

    fn lower_typed_builtin_place_method_receiver(
        &mut self,
        receiver: &TypedExpr,
        receiver_ty: nia_ids::InternedTyId,
        method: BuiltinTraitMethod,
    ) -> TypedExpr {
        let receiver_kind = method
            .place_receiver_kind()
            .unwrap_or_else(|| method.receiver_kind());
        match receiver_kind {
            BuiltinReceiverKind::RefConst => {
                let pointer_ty = self.interner.intern(TyKind::Pointer {
                    is_const: true,
                    elem: receiver_ty,
                });
                TypedExpr {
                    span: receiver.span,
                    ty: pointer_ty,
                    kind: TypedExprKind::Unary {
                        op: UnaryOp::RefConst,
                        expr: Box::new(receiver.clone()),
                    },
                }
            }
            BuiltinReceiverKind::Ref => {
                let pointer_ty = self.interner.intern(TyKind::Pointer {
                    is_const: false,
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
            BuiltinReceiverKind::Value => receiver.clone(),
        }
    }
}

struct BirComptimeEnv<'a, 'b> {
    checker: &'a BodyChecker<'a>,
    module_id: nia_ids::ModuleId,
    active: &'b mut Vec<nia_ids::GlobalDefId>,
}

impl nia_comptime_engine::ComptimeEnv for BirComptimeEnv<'_, '_> {
    fn resolve_ident(
        &mut self,
        span: Span,
        name: &str,
    ) -> Result<nia_comptime_check::ComptimeValue, nia_comptime_engine::ComptimeError> {
        let def_id = self
            .checker
            .global_comptime_id_in_module(self.module_id, span)
            .ok_or_else(|| nia_comptime_engine::ComptimeError {
                span,
                message: format!("comptime expression can only use comptime bindings: `{name}`"),
            })?;
        self.checker
            .global_comptime_value_for_env(def_id, self.active)
            .ok_or_else(|| nia_comptime_engine::ComptimeError {
                span,
                message: format!("failed to evaluate comptime value `{name}`"),
            })
    }
}
