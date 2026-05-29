// SPDX-License-Identifier: GPL-3.0-or-later
use crate::ModuleLowerer;
use nia_ast::{
    ArrayElements, AssignOp, BindingStmt, Block, Expr, ExprKind, ForHeader, ForInit, IndexArg,
    ItemKind, SliceRange, Stmt, StmtKind, SwitchArmBody, SwitchPattern, UnaryOp,
};
use nia_body_ir::{
    BracketSuffixResolution, BuiltinConst, BuiltinValue, PlaceBase, PlaceElem, ResolvedCall,
    TypedArrayElements, TypedBinding, TypedBody, TypedCallee, TypedExpr, TypedExprKind,
    TypedFieldInit, TypedFor, TypedForHeader, TypedForInit, TypedLocal, TypedLocalKind, TypedPlace,
    TypedSliceRange, TypedStmt, TypedStmtKind, TypedSwitch, TypedSwitchArm, TypedSwitchArmBody,
    TypedSwitchPattern,
};
use nia_defs::DefKind;
use nia_diagnostic::Diagnostic;
use nia_ids::LocalId;
use nia_local_resolve::{LocalKind, LocalUse};
use nia_span::Span;
use nia_ty::TyKind;
use nia_value_resolve::ValueNameResolution;

use crate::literals::{
    decode_byte_string_literal, decode_c_string_literal, decode_char_literal,
    decode_string_literal, numeric_literal_body,
};

mod asm;

impl<'a> ModuleLowerer<'a> {
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
            .unwrap_or_else(|| self.void_ty());
        TypedBody {
            span: block.span,
            locals: self.lower_locals(block.span),
            stmts,
            tail,
            ty,
        }
    }

    fn lower_locals(&self, body_span: Span) -> Vec<TypedLocal> {
        self.input
            .locals
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
                ty: self.local_ty(id).unwrap_or_else(|| self.error_ty()),
                span: local.span,
            })
            .collect()
    }

    fn lower_stmt(&mut self, stmt: &Stmt) -> TypedStmt {
        let kind = match &stmt.kind {
            StmtKind::Using(_) => TypedStmtKind::Expr(TypedExpr {
                span: stmt.span,
                ty: self.error_ty(),
                kind: TypedExprKind::Error,
            }),
            StmtKind::Binding(binding) => {
                TypedStmtKind::Binding(self.lower_binding_stmt(stmt.span, binding))
            }
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

    fn lower_binding_stmt(&mut self, span: Span, binding: &BindingStmt) -> TypedBinding {
        let local_id = self
            .input
            .locals
            .local_defs
            .get(&span)
            .copied()
            .unwrap_or(LocalId(u32::MAX));
        let ty = self.local_ty(local_id).unwrap_or_else(|| {
            binding.ty.as_ref().map_or_else(
                || {
                    binding
                        .value
                        .as_ref()
                        .and_then(|value| self.expr_ty(value))
                        .unwrap_or_else(|| self.error_ty())
                },
                |ty| self.ty_for_type_span(ty.span),
            )
        });
        TypedBinding {
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
                                    ty: self.void_ty(),
                                    kind: TypedExprKind::Error,
                                })
                            } else {
                                TypedForInit::Binding(self.lower_binding_stmt(*span, binding))
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

    fn lower_expr(&mut self, expr: &Expr) -> TypedExpr {
        self.lower_expr_with_ty(expr, None)
    }

    fn lower_expr_with_ty(
        &mut self,
        expr: &Expr,
        forced_ty: Option<nia_ids::InternedTyId>,
    ) -> TypedExpr {
        if forced_ty.is_none()
            && let Some(coercion) = self
                .input
                .body_check
                .ir
                .c_string_pointer_coercions
                .get(&expr.span)
                .copied()
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
            && let Some(coercion) = self
                .input
                .body_check
                .ir
                .array_to_slice_coercions
                .get(&expr.span)
                .copied()
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
            .or_else(|| self.expr_ty(expr))
            .unwrap_or_else(|| self.error_ty());
        if let Some(def_id) = self.comptime_global_id_for_expr(expr) {
            if self.comptime_global_stack.contains(&def_id) {
                return TypedExpr {
                    span: expr.span,
                    ty,
                    kind: TypedExprKind::Error,
                };
            }
            if let Some(binding) = self.comptime_binding_for(def_id)
                && let Some(value) = &binding.value
            {
                self.comptime_global_stack.push(def_id);
                let mut lowered = self.lower_expr(value);
                self.comptime_global_stack.pop();
                lowered.span = expr.span;
                lowered.ty = ty;
                return lowered;
            }
        }
        if let Some(variant_id) = self.qualified_enum_variant(expr) {
            return TypedExpr {
                span: expr.span,
                ty,
                kind: TypedExprKind::EnumVariant(variant_id),
            };
        }
        if let Some(def_id) = self.input.values.qualified_values.get(&expr.span).copied() {
            let kind = if self.input.signatures.functions.contains_key(&def_id.def_id) {
                TypedExprKind::Function(def_id)
            } else {
                TypedExprKind::Global(def_id)
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
                if let Some(local_id) = self.local_comptime_id(expr) {
                    if self.comptime_local_stack.contains(&local_id) {
                        return TypedExpr {
                            span: expr.span,
                            ty,
                            kind: TypedExprKind::Error,
                        };
                    }
                    let Some(value) = self.local_comptime_value(expr).cloned() else {
                        return TypedExpr {
                            span: expr.span,
                            ty,
                            kind: TypedExprKind::Error,
                        };
                    };
                    self.comptime_local_stack.push(local_id);
                    let mut lowered = self.lower_expr(&value);
                    self.comptime_local_stack.pop();
                    lowered.span = expr.span;
                    lowered.ty = ty;
                    return lowered;
                }
                self.lower_ident_expr(expr)
            }
            ExprKind::Builtin { .. } => {
                match self.input.body_check.ir.builtin_values.get(&expr.span) {
                    Some(BuiltinValue::Usize(value)) => {
                        TypedExprKind::BuiltinValue(BuiltinConst::Usize(*value))
                    }
                    None => TypedExprKind::Error,
                }
            }
            ExprKind::TypeTarget { .. } => TypedExprKind::Error,
            ExprKind::BracketSuffix { callee, args } => {
                match self.bracket_suffix_resolution(expr.span) {
                    Some(BracketSuffixResolution::Index) => {
                        if let Some(index) = args.first().and_then(|arg| arg.expr.as_ref()) {
                            TypedExprKind::Index {
                                lhs: Box::new(self.lower_expr(callee)),
                                index: Box::new(self.lower_expr(index)),
                            }
                        } else {
                            TypedExprKind::Error
                        }
                    }
                    Some(BracketSuffixResolution::GenericCall) => {
                        if let Some(reference) =
                            self.input.body_check.ir.function_references.get(&expr.span)
                        {
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
                let field = self
                    .field_def_for_base_ty(lhs_expr.ty, name)
                    .unwrap_or_else(|| self.global_error_def());
                TypedExprKind::Field {
                    lhs: Box::new(lhs_expr),
                    field,
                }
            }
            ExprKind::ArrayLiteral { elems } => TypedExprKind::ArrayLiteral {
                elems: self.lower_array_elements(elems),
            },
            ExprKind::StructLiteral { fields } => {
                let def_id = self.nominal_global_def(ty);
                let def_id = def_id.unwrap_or_else(|| self.global_error_def());
                if self.input.signatures.unions.contains_key(&def_id.def_id) {
                    let field = fields.first().map(|field| TypedFieldInit {
                        field: self
                            .field_def_for_struct_ty(ty, &field.name)
                            .unwrap_or_else(|| self.global_error_def()),
                        name: field.name.clone(),
                        value: self.lower_expr(&field.value),
                        span: field.span,
                    });
                    TypedExprKind::UnionLiteral {
                        def_id,
                        field: Box::new(field.unwrap_or_else(|| TypedFieldInit {
                            field: self.global_error_def(),
                            name: String::new(),
                            value: TypedExpr {
                                span: expr.span,
                                ty: self.error_ty(),
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
                                field: self
                                    .field_def_for_struct_ty(ty, &field.name)
                                    .unwrap_or_else(|| self.global_error_def()),
                                name: field.name.clone(),
                                value: self.lower_expr(&field.value),
                                span: field.span,
                            })
                            .collect(),
                    }
                }
            }
            ExprKind::Unary { op, expr: inner } => {
                if let ExprKind::Index {
                    lhs,
                    index: IndexArg::Range(range),
                } = &inner.kind
                    && matches!(op, UnaryOp::Ref | UnaryOp::RefConst)
                {
                    TypedExprKind::Slice {
                        lhs: Box::new(self.lower_expr(lhs)),
                        range: self.lower_slice_range(range),
                        is_const: matches!(op, UnaryOp::RefConst),
                    }
                } else if matches!(op, UnaryOp::Ref | UnaryOp::RefConst)
                    && let Some(function_item) = self.lower_function_item_ref(inner)
                {
                    TypedExprKind::Unary {
                        op: *op,
                        expr: Box::new(TypedExpr {
                            span: inner.span,
                            ty: self.expr_ty(inner).unwrap_or_else(|| self.error_ty()),
                            kind: function_item,
                        }),
                    }
                } else {
                    TypedExprKind::Unary {
                        op: *op,
                        expr: Box::new(self.lower_expr(inner)),
                    }
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
                ty: self.ty_for_type_span(ty.span),
            },
            ExprKind::Call { callee, args } => {
                if let ExprKind::Builtin { name, .. } = &callee.kind {
                    match (name.as_str(), args.as_slice()) {
                        (_, []) => self.lower_expr(callee).kind,
                        ("len", [arg]) => TypedExprKind::Len(Box::new(self.lower_expr(arg))),
                        ("ptr", [arg]) => TypedExprKind::Ptr(Box::new(self.lower_expr(arg))),
                        ("asm", [arg]) => self.lower_inline_asm(arg),
                        _ => TypedExprKind::Call {
                            callee: self.lower_callee(expr.span, callee),
                            args: args.iter().map(|arg| self.lower_expr(arg)).collect(),
                        },
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
                    let field = self
                        .field_def_for_base_ty(lhs_expr.ty, name)
                        .unwrap_or_else(|| self.global_error_def());
                    TypedExprKind::Field {
                        lhs: Box::new(lhs_expr),
                        field,
                    }
                }
            }
            ExprKind::Index { lhs, index } => match index {
                IndexArg::Expr(index) => TypedExprKind::Index {
                    lhs: Box::new(self.lower_expr(lhs)),
                    index: Box::new(self.lower_expr(index)),
                },
                IndexArg::Range(range) => TypedExprKind::Slice {
                    lhs: Box::new(self.lower_expr(lhs)),
                    range: self.lower_slice_range(range),
                    is_const: true,
                },
            },
            ExprKind::Block(block) if self.empty_struct_literal_expr(ty, block) => {
                TypedExprKind::StructLiteral {
                    def_id: self
                        .nominal_global_def(ty)
                        .unwrap_or_else(|| self.global_error_def()),
                    fields: Vec::new(),
                }
            }
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

    fn empty_struct_literal_expr(&self, ty: nia_ids::InternedTyId, block: &Block) -> bool {
        if !block.stmts.is_empty() || block.tail.is_some() {
            return false;
        }
        let Some(TyKind::Nominal { def_id, .. }) = self.input.body_check.ir.interner.get(ty) else {
            return false;
        };
        self.input
            .signatures
            .structs
            .get(&def_id.def_id)
            .is_some_and(|signature| signature.fields.is_empty())
    }

    fn lower_ident_expr(&self, expr: &Expr) -> TypedExprKind {
        match self.input.locals.uses.get(&expr.span) {
            Some(LocalUse::Local(local)) => {
                if self
                    .input
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
                if let Some(variant_id) =
                    self.input.values.qualified_values.get(&expr.span).copied()
                    && self.input.values.variant_enums.contains_key(&expr.span)
                {
                    return TypedExprKind::EnumVariant(variant_id);
                }
                if let Some(global_id) = self.input.values.qualified_values.get(&expr.span).copied()
                {
                    match self.def_kind_of(global_id) {
                        Some(DefKind::Function | DefKind::Method) => {
                            return TypedExprKind::Function(global_id);
                        }
                        Some(DefKind::Global) => return TypedExprKind::Global(global_id),
                        Some(DefKind::Comptime) => return TypedExprKind::Error,
                        _ => return TypedExprKind::Error,
                    }
                }
                match self.input.values.names.get(&expr.span) {
                    Some(ValueNameResolution::Def(def_id)) => {
                        match self.input.defs.defs.get(*def_id).map(|def| def.kind) {
                            Some(DefKind::Function | DefKind::Method) => {
                                TypedExprKind::Function(self.global_def_id(*def_id))
                            }
                            Some(DefKind::Global) => {
                                TypedExprKind::Global(self.global_def_id(*def_id))
                            }
                            Some(DefKind::Comptime) => TypedExprKind::Error,
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
        let reference = self
            .input
            .body_check
            .ir
            .function_references
            .get(&expr.span)?;
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
        match nia_comptime_engine::eval_array_len_expr(count, self) {
            Ok(value) => value,
            Err(err) => {
                self.diagnostics.push(Diagnostic::error(
                    err.span,
                    format!("invalid repeat count: {}", err.message),
                ));
                0
            }
        }
    }

    fn lower_callee(&mut self, call_span: Span, callee: &Expr) -> TypedCallee {
        if let Some(resolved) = self.input.body_check.ir.resolved_calls.get(&call_span) {
            return self.lower_resolved_callee(callee, resolved);
        }
        TypedCallee::FunctionPointer(Box::new(self.lower_expr(callee)))
    }

    fn lower_resolved_callee(&mut self, callee: &Expr, resolved: &ResolvedCall) -> TypedCallee {
        match resolved {
            ResolvedCall::Function(def_id) => TypedCallee::Function(*def_id),
            ResolvedCall::FunctionInstance { def_id, args } => TypedCallee::FunctionInstance {
                def_id: *def_id,
                args: args.clone(),
            },
            ResolvedCall::Method { def_id, args } => TypedCallee::Method {
                def_id: *def_id,
                args: args.clone(),
                receiver: Box::new(
                    self.lower_receiver_expr(callee)
                        .unwrap_or_else(|| self.lower_expr(callee)),
                ),
            },
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

    pub(crate) fn local_comptime_value(&self, expr: &Expr) -> Option<&Expr> {
        let Some(LocalUse::Local(local_id)) = self.input.locals.uses.get(&expr.span) else {
            return None;
        };
        let local = self.input.locals.locals.get(*local_id)?;
        if local.kind != LocalKind::ComptimeBinding {
            return None;
        }
        self.local_comptime_binding_value(*local_id, self.input.module)
    }

    fn local_comptime_id(&self, expr: &Expr) -> Option<LocalId> {
        self.local_comptime_id_for_span(expr.span)
    }

    pub(crate) fn local_comptime_id_for_span(&self, span: Span) -> Option<LocalId> {
        let Some(LocalUse::Local(local_id)) = self.input.locals.uses.get(&span) else {
            return None;
        };
        let local = self.input.locals.locals.get(*local_id)?;
        (local.kind == LocalKind::ComptimeBinding).then_some(*local_id)
    }

    fn local_comptime_binding_value<'b>(
        &self,
        local_id: LocalId,
        module: &'b nia_ast::Module,
    ) -> Option<&'b Expr> {
        module.items.iter().find_map(|item| match &item.kind {
            ItemKind::Function(function) => function
                .body
                .as_ref()
                .and_then(|body| self.local_comptime_value_in_block(local_id, body)),
            ItemKind::Extend(extend) => extend.methods.iter().find_map(|method| {
                method
                    .function
                    .body
                    .as_ref()
                    .and_then(|body| self.local_comptime_value_in_block(local_id, body))
            }),
            _ => None,
        })
    }

    fn local_comptime_value_in_block<'b>(
        &self,
        local_id: LocalId,
        block: &'b Block,
    ) -> Option<&'b Expr> {
        for stmt in &block.stmts {
            match &stmt.kind {
                StmtKind::Binding(binding)
                    if self.input.locals.local_defs.get(&stmt.span).copied() == Some(local_id) =>
                {
                    return binding.value.as_ref();
                }
                StmtKind::For(for_stmt) => {
                    if let ForHeader::CStyle { init, .. } = &for_stmt.header
                        && let Some(init) = init
                        && let ForInit::Binding { span, binding } = &**init
                        && self.input.locals.local_defs.get(span).copied() == Some(local_id)
                    {
                        return binding.value.as_ref();
                    }
                    if let Some(value) =
                        self.local_comptime_value_in_block(local_id, &for_stmt.body)
                    {
                        return Some(value);
                    }
                }
                StmtKind::Expr(expr) => {
                    if let Some(value) = self.local_comptime_value_in_expr(local_id, expr) {
                        return Some(value);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn local_comptime_value_in_stmt<'b>(
        &self,
        local_id: LocalId,
        stmt: &'b Stmt,
    ) -> Option<&'b Expr> {
        match &stmt.kind {
            StmtKind::Binding(binding)
                if self.input.locals.local_defs.get(&stmt.span).copied() == Some(local_id) =>
            {
                binding.value.as_ref()
            }
            StmtKind::For(for_stmt) => self.local_comptime_value_in_block(local_id, &for_stmt.body),
            StmtKind::Expr(expr) => self.local_comptime_value_in_expr(local_id, expr),
            _ => None,
        }
    }

    fn local_comptime_value_in_expr<'b>(
        &self,
        local_id: LocalId,
        expr: &'b Expr,
    ) -> Option<&'b Expr> {
        match &expr.kind {
            ExprKind::Block(block) => self.local_comptime_value_in_block(local_id, block),
            ExprKind::If {
                then_branch,
                else_branch,
                ..
            } => self
                .local_comptime_value_in_block(local_id, then_branch)
                .or_else(|| {
                    else_branch.as_ref().and_then(|else_branch| {
                        self.local_comptime_value_in_expr(local_id, else_branch)
                    })
                }),
            ExprKind::Switch(switch) => switch.arms.iter().find_map(|arm| match &arm.body {
                SwitchArmBody::Block(block) => self.local_comptime_value_in_block(local_id, block),
                SwitchArmBody::Stmt(stmt) => self.local_comptime_value_in_stmt(local_id, stmt),
                SwitchArmBody::Expr(expr) => self.local_comptime_value_in_expr(local_id, expr),
            }),
            _ => None,
        }
    }

    pub(crate) fn lower_place(&mut self, expr: &Expr) -> TypedPlace {
        let ty = self.expr_ty(expr).unwrap_or_else(|| self.error_ty());
        let mut elems = Vec::new();
        let base = self.lower_place_inner(expr, &mut elems);
        TypedPlace {
            span: expr.span,
            ty,
            base,
            elems,
        }
    }

    fn lower_place_inner(&mut self, expr: &Expr, elems: &mut Vec<PlaceElem>) -> PlaceBase {
        if self.input.values.variant_enums.contains_key(&expr.span) {
            return PlaceBase::Local(LocalId(u32::MAX));
        }
        if let Some(def_id) = self.input.values.qualified_values.get(&expr.span).copied() {
            return PlaceBase::Global(def_id);
        }
        match &expr.kind {
            ExprKind::Ident(_) => match self.input.locals.uses.get(&expr.span) {
                Some(LocalUse::Local(local)) => PlaceBase::Local(*local),
                Some(LocalUse::ModuleValue) => match self.input.values.names.get(&expr.span) {
                    Some(ValueNameResolution::Def(def_id)) => {
                        PlaceBase::Global(self.global_def_id(*def_id))
                    }
                    _ => PlaceBase::Local(LocalId(u32::MAX)),
                },
                _ => PlaceBase::Local(LocalId(u32::MAX)),
            },
            ExprKind::Unary {
                op: nia_ast::UnaryOp::Deref,
                expr,
            } => PlaceBase::Deref(Box::new(self.lower_expr(expr))),
            ExprKind::Field { lhs, name } | ExprKind::Qualified { lhs, name } => {
                let base = self.lower_place_inner(lhs, elems);
                let lhs_ty = self.expr_ty(lhs).unwrap_or_else(|| self.error_ty());
                let field = self
                    .field_def_for_base_ty(lhs_ty, name)
                    .unwrap_or_else(|| self.global_error_def());
                elems.push(PlaceElem::Field(field));
                base
            }
            ExprKind::Index { lhs, index } => {
                let base = self.lower_place_inner(lhs, elems);
                if let IndexArg::Expr(index) = index {
                    elems.push(PlaceElem::Index(Box::new(self.lower_expr(index))));
                }
                base
            }
            ExprKind::BracketSuffix { callee, args } => {
                if matches!(
                    self.bracket_suffix_resolution(expr.span),
                    Some(BracketSuffixResolution::Index)
                ) {
                    let base = self.lower_place_inner(callee, elems);
                    if let Some(index) = args.first().and_then(|arg| arg.expr.as_ref()) {
                        elems.push(PlaceElem::Index(Box::new(self.lower_expr(index))));
                    }
                    base
                } else {
                    PlaceBase::Local(LocalId(u32::MAX))
                }
            }
            _ => PlaceBase::Local(LocalId(u32::MAX)),
        }
    }
}
