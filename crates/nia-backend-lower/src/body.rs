// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::{ModuleLowerer, generic_inst_base};
use nia_ast::{
    ArrayElements, BindingStmt, Block, Expr, ExprKind, ForHeader, ForInit, IndexArg, ItemKind,
    SliceRange, Stmt, StmtKind, SwitchArmBody, SwitchPattern, UnaryOp,
};
use nia_backend_ir::{
    BuiltinConst, PlaceBase, PlaceElem, TypedArrayElements, TypedBinding, TypedBody, TypedCallee,
    TypedExpr, TypedExprKind, TypedFieldInit, TypedFor, TypedForHeader, TypedForInit, TypedLocal,
    TypedLocalKind, TypedPlace, TypedSliceRange, TypedStmt, TypedStmtKind, TypedSwitch,
    TypedSwitchArm, TypedSwitchArmBody, TypedSwitchPattern,
};
use nia_body_check::BuiltinValue;
use nia_defs::DefKind;
use nia_ids::{GlobalDefId, LocalId, TyId};
use nia_local_resolve::{LocalKind, LocalUse};
use nia_span::Span;
use nia_ty::{ArrayLenTy, PrimitiveTy, TyKind};
use nia_value_resolve::ValueNameResolution;

use crate::literals::{decode_string_literal, numeric_literal_body};

mod asm;

#[derive(Clone, Copy)]
struct MethodCandidate {
    target_ty: TyId,
    method_id: GlobalDefId,
}

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
            StmtKind::Switch(switch) => TypedStmtKind::Switch(TypedSwitch {
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
            }),
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
        TypedBinding {
            local_id,
            name: binding.name.clone(),
            ty: self.local_ty(local_id).unwrap_or_else(|| {
                binding
                    .value
                    .as_ref()
                    .and_then(|value| self.expr_ty(value))
                    .unwrap_or_else(|| self.error_ty())
            }),
            value: binding.value.as_ref().map(|value| self.lower_expr(value)),
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

    fn lower_expr(&mut self, expr: &Expr) -> TypedExpr {
        self.lower_expr_with_ty(expr, None)
    }

    fn lower_expr_with_ty(&mut self, expr: &Expr, forced_ty: Option<nia_ids::TyId>) -> TypedExpr {
        if forced_ty.is_none()
            && let Some(coercion) = self
                .input
                .body_check
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
            ExprKind::String(text) => {
                TypedExprKind::String(decode_string_literal(text).unwrap_or_default())
            }
            ExprKind::Char(text) => TypedExprKind::Char(text.clone()),
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
                match self.input.body_check.builtin_values.get(&expr.span) {
                    Some(BuiltinValue::Usize(value)) => {
                        TypedExprKind::BuiltinValue(BuiltinConst::Usize(*value))
                    }
                    None => TypedExprKind::Error,
                }
            }
            ExprKind::BracketSuffix { callee, args } => {
                let base = generic_inst_base(callee);
                if let Some(def_id) = self.input.values.qualified_values.get(&base.span).copied() {
                    TypedExprKind::FunctionInstance {
                        def_id,
                        args: args
                            .iter()
                            .filter_map(|arg| {
                                arg.ty.as_ref().map(|ty| self.ty_for_type_span(ty.span))
                            })
                            .collect(),
                    }
                } else if args.len() == 1 {
                    if let Some(index) = args.first().and_then(|arg| arg.expr.as_ref()) {
                        TypedExprKind::Index {
                            lhs: Box::new(self.lower_expr(callee)),
                            index: Box::new(self.lower_expr(index)),
                        }
                    } else {
                        TypedExprKind::Error
                    }
                } else {
                    TypedExprKind::Error
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
                if let ExprKind::Builtin { name, .. } = &callee.kind
                    && args.len() == 1
                {
                    match name.as_str() {
                        "len" => TypedExprKind::Len(Box::new(self.lower_expr(&args[0]))),
                        "ptr" => TypedExprKind::Ptr(Box::new(self.lower_expr(&args[0]))),
                        "asm" => self.lower_inline_asm(&args[0]),
                        _ => TypedExprKind::Call {
                            callee: self.lower_callee(callee),
                            args: args.iter().map(|arg| self.lower_expr(arg)).collect(),
                        },
                    }
                } else {
                    TypedExprKind::Call {
                        callee: self.lower_callee(callee),
                        args: args.iter().map(|arg| self.lower_expr(arg)).collect(),
                    }
                }
            }
            ExprKind::Qualified { lhs, name } => {
                if let Some(variant) = self.enum_variant_for_qualified(lhs, name) {
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
        };
        TypedExpr {
            span: expr.span,
            ty,
            kind,
        }
    }

    fn empty_struct_literal_expr(&self, ty: nia_ids::TyId, block: &Block) -> bool {
        if !block.stmts.is_empty() || block.tail.is_some() {
            return false;
        }
        let Some(TyKind::Nominal { def_id, .. }) = self.input.body_check.interner.get(ty) else {
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
                count: nia_comptime_engine::eval_array_len_text(&count.text).unwrap_or(0),
            },
        }
    }

    fn lower_callee(&mut self, callee: &Expr) -> TypedCallee {
        let base = generic_inst_base(callee);
        if let Some(def_id) = self.input.values.qualified_values.get(&base.span).copied() {
            return TypedCallee::Function(def_id);
        }
        if let ExprKind::BracketSuffix {
            callee: inner,
            args,
        } = &callee.kind
            && let Some(def_id) = self.input.values.qualified_values.get(&inner.span).copied()
        {
            return TypedCallee::FunctionInstance {
                def_id,
                args: args
                    .iter()
                    .filter_map(|arg| arg.ty.as_ref().map(|ty| self.ty_for_type_span(ty.span)))
                    .collect(),
            };
        }
        if let ExprKind::Qualified { lhs, name } = &base.kind
            && let Some((struct_id, struct_args)) = self.type_prefix_instance(lhs)
            && let Some(method_id) =
                self.single_method_def_for_target(struct_id, &struct_args, name)
        {
            if struct_args.is_empty() || !self.method_has_effective_generics(method_id) {
                return TypedCallee::Function(method_id);
            }
            return TypedCallee::FunctionInstance {
                def_id: method_id,
                args: struct_args,
            };
        }
        if let Some(method) = self.lower_method_callee(callee) {
            return method;
        }
        if let ExprKind::Ident(_) = &base.kind
            && let Some(global_id) = self.input.values.qualified_values.get(&base.span).copied()
            && matches!(
                self.def_kind_of(global_id),
                Some(DefKind::Function | DefKind::Method)
            )
        {
            return TypedCallee::Function(global_id);
        }
        if let ExprKind::Ident(_) = &base.kind
            && let Some(ValueNameResolution::Def(def_id)) = self.input.values.names.get(&base.span)
            && self.input.signatures.functions.contains_key(def_id)
        {
            return TypedCallee::Function(self.global_def_id(*def_id));
        }
        TypedCallee::FunctionPointer(Box::new(self.lower_expr(callee)))
    }

    fn lower_method_callee(&mut self, callee: &Expr) -> Option<TypedCallee> {
        let (field_callee, type_args) = match &callee.kind {
            ExprKind::Field { .. } => (callee, Vec::new()),
            ExprKind::BracketSuffix { callee, args } => (
                callee.as_ref(),
                args.iter()
                    .filter_map(|arg| arg.ty.as_ref().map(|ty| self.ty_for_type_span(ty.span)))
                    .collect(),
            ),
            _ => return None,
        };
        let ExprKind::Field { lhs, name } = &field_callee.kind else {
            return None;
        };
        let receiver = self.lower_expr(lhs);
        let (method_id, mut args) = self.single_method_for_receiver(receiver.ty, name)?;
        args.extend(type_args);
        Some(TypedCallee::Method {
            def_id: method_id,
            args,
            receiver: Box::new(receiver),
        })
    }

    fn single_method_def_for_target(
        &self,
        struct_id: GlobalDefId,
        struct_args: &[TyId],
        name: &str,
    ) -> Option<GlobalDefId> {
        let mut candidates = self.methods_for_nominal_target(struct_id, name);
        if !struct_args.is_empty() || self.type_prefix_has_no_generics(struct_id) {
            let target_ty = self.nominal_ty(struct_id, struct_args)?;
            candidates.retain(|candidate| {
                self.match_type_pattern(candidate.target_ty, target_ty, &mut HashMap::new())
            });
        }
        let candidates = self.most_specific_candidates(&candidates);
        match candidates.as_slice() {
            [method] => Some(method.method_id),
            _ => None,
        }
    }

    fn method_has_effective_generics(&self, def_id: GlobalDefId) -> bool {
        let own_generics = self
            .input
            .defs
            .defs
            .get(def_id.def_id)
            .map(|def| def.generics.as_slice())
            .unwrap_or(&[]);
        !self.effective_generics(def_id, own_generics).is_empty()
    }

    fn type_prefix_has_no_generics(&self, def_id: GlobalDefId) -> bool {
        let Some(signature) = self.input.signatures.structs.get(&def_id.def_id) else {
            return false;
        };
        signature.generics.is_empty()
    }

    fn nominal_ty(&self, def_id: GlobalDefId, args: &[TyId]) -> Option<TyId> {
        self.input
            .body_check
            .interner
            .iter()
            .find_map(|(ty_id, ty)| {
                matches!(
                    ty,
                    TyKind::Nominal {
                        def_id: ty_def,
                        args: ty_args,
                    } if *ty_def == def_id && ty_args == args
                )
                .then_some(ty_id)
            })
    }

    fn methods_for_nominal_target(
        &self,
        struct_id: GlobalDefId,
        name: &str,
    ) -> Vec<MethodCandidate> {
        self.input
            .extensions
            .all_methods_named(name)
            .into_iter()
            .filter_map(|(target_ty, method_id)| {
                matches!(
                    self.ty_kind(target_ty),
                    Some(TyKind::Nominal { def_id, .. }) if *def_id == struct_id
                )
                .then_some(MethodCandidate {
                    target_ty,
                    method_id,
                })
            })
            .collect()
    }

    fn single_method_for_receiver(
        &self,
        receiver_ty: TyId,
        name: &str,
    ) -> Option<(GlobalDefId, Vec<TyId>)> {
        let receiver_ty = self.normalize_ty(receiver_ty);
        let mut candidates = self.method_candidates_for_receiver(receiver_ty, name);
        candidates = self.most_specific_candidates(&candidates);
        match candidates.as_slice() {
            [candidate] => {
                let mut substitutions = HashMap::new();
                self.match_type_pattern(candidate.target_ty, receiver_ty, &mut substitutions)
                    .then(|| {
                        (
                            candidate.method_id,
                            self.extension_target_instance_args(
                                candidate.target_ty,
                                &substitutions,
                            ),
                        )
                    })
            }
            _ => None,
        }
    }

    fn method_candidates_for_receiver(
        &self,
        mut receiver_ty: TyId,
        name: &str,
    ) -> Vec<MethodCandidate> {
        loop {
            let candidates = self
                .input
                .extensions
                .all_methods_named(name)
                .into_iter()
                .filter_map(|(target_ty, method_id)| {
                    self.match_type_pattern(target_ty, receiver_ty, &mut HashMap::new())
                        .then_some(MethodCandidate {
                            target_ty,
                            method_id,
                        })
                })
                .collect::<Vec<_>>();
            if !candidates.is_empty() {
                return candidates;
            }
            match self.ty_kind(receiver_ty) {
                Some(TyKind::Pointer { elem, .. }) => {
                    receiver_ty = self.normalize_ty(*elem);
                }
                _ => return Vec::new(),
            }
        }
    }

    fn extension_target_instance_args(
        &self,
        target_ty: TyId,
        substitutions: &HashMap<String, TyId>,
    ) -> Vec<TyId> {
        self.generic_params_in_ty(target_ty)
            .iter()
            .filter_map(|generic| substitutions.get(generic).copied())
            .collect()
    }

    fn match_type_pattern(
        &self,
        pattern: TyId,
        actual: TyId,
        substitutions: &mut HashMap<String, TyId>,
    ) -> bool {
        let pattern = self.normalize_ty(pattern);
        let actual = self.normalize_ty(actual);
        match self.ty_kind(pattern) {
            Some(TyKind::GenericParam(name)) => {
                if let Some(existing) = substitutions.get(name).copied() {
                    self.types_match(existing, actual)
                } else {
                    substitutions.insert(name.clone(), actual);
                    true
                }
            }
            Some(TyKind::Pointer {
                is_const: pattern_const,
                elem: pattern_elem,
            }) => matches!(
                self.ty_kind(actual),
                Some(TyKind::Pointer { is_const, elem })
                    if is_const == pattern_const
                        && self.match_type_pattern(*pattern_elem, *elem, substitutions)
            ),
            Some(TyKind::Slice {
                is_const: pattern_const,
                elem: pattern_elem,
            }) => matches!(
                self.ty_kind(actual),
                Some(TyKind::Slice { is_const, elem })
                    if is_const == pattern_const
                        && self.match_type_pattern(*pattern_elem, *elem, substitutions)
            ),
            Some(TyKind::Array {
                len: pattern_len,
                elem: pattern_elem,
            }) => match self.ty_kind(actual) {
                Some(TyKind::Array { len, elem }) if self.array_lens_match(pattern_len, len) => {
                    self.match_type_pattern(*pattern_elem, *elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::FunctionPointer {
                params: pattern_params,
                return_type: pattern_return,
                is_variadic: pattern_variadic,
            }) => match self.ty_kind(actual) {
                Some(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                }) if pattern_variadic == is_variadic && pattern_params.len() == params.len() => {
                    pattern_params.iter().zip(params).all(|(pattern, actual)| {
                        self.match_type_pattern(*pattern, *actual, substitutions)
                    }) && self.match_type_pattern(*pattern_return, *return_type, substitutions)
                }
                _ => false,
            },
            Some(TyKind::Nominal {
                def_id: pattern_def,
                args: pattern_args,
            }) => match self.ty_kind(actual) {
                Some(TyKind::Nominal { def_id, args })
                    if pattern_def == def_id && pattern_args.len() == args.len() =>
                {
                    pattern_args.iter().zip(args).all(|(pattern, actual)| {
                        self.match_type_pattern(*pattern, *actual, substitutions)
                    })
                }
                _ => false,
            },
            Some(TyKind::Primitive(_)) | Some(TyKind::Error) | None => {
                self.types_match(pattern, actual)
            }
        }
    }

    fn most_specific_candidates(&self, candidates: &[MethodCandidate]) -> Vec<MethodCandidate> {
        candidates
            .iter()
            .copied()
            .filter(|candidate| {
                !candidates.iter().any(|other| {
                    other.method_id != candidate.method_id
                        && self.strictly_more_specific(other.target_ty, candidate.target_ty)
                })
            })
            .collect()
    }

    fn strictly_more_specific(&self, specific: TyId, general: TyId) -> bool {
        self.pattern_subsumes(general, specific) && !self.pattern_subsumes(specific, general)
    }

    fn pattern_subsumes(&self, general: TyId, specific: TyId) -> bool {
        self.pattern_subsumes_inner(general, specific, &mut HashMap::new())
    }

    fn pattern_subsumes_inner(
        &self,
        general: TyId,
        specific: TyId,
        substitutions: &mut HashMap<String, TyId>,
    ) -> bool {
        let general = self.normalize_ty(general);
        let specific = self.normalize_ty(specific);
        match self.ty_kind(general) {
            Some(TyKind::GenericParam(name)) => {
                if let Some(existing) = substitutions.get(name).copied() {
                    self.patterns_equivalent(existing, specific)
                } else {
                    substitutions.insert(name.clone(), specific);
                    true
                }
            }
            Some(TyKind::Primitive(general_primitive)) => matches!(
                self.ty_kind(specific),
                Some(TyKind::Primitive(specific_primitive)) if general_primitive == specific_primitive
            ),
            Some(TyKind::Pointer {
                is_const: general_const,
                elem: general_elem,
            }) => matches!(
                self.ty_kind(specific),
                Some(TyKind::Pointer {
                    is_const: specific_const,
                    elem: specific_elem,
                }) if general_const == specific_const
                    && self.pattern_subsumes_inner(*general_elem, *specific_elem, substitutions)
            ),
            Some(TyKind::Slice {
                is_const: general_const,
                elem: general_elem,
            }) => matches!(
                self.ty_kind(specific),
                Some(TyKind::Slice {
                    is_const: specific_const,
                    elem: specific_elem,
                }) if general_const == specific_const
                    && self.pattern_subsumes_inner(*general_elem, *specific_elem, substitutions)
            ),
            Some(TyKind::Array {
                len: general_len,
                elem: general_elem,
            }) => match self.ty_kind(specific) {
                Some(TyKind::Array {
                    len: specific_len,
                    elem: specific_elem,
                }) if self.array_lens_match(general_len, specific_len) => {
                    self.pattern_subsumes_inner(*general_elem, *specific_elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::FunctionPointer {
                params: general_params,
                return_type: general_return,
                is_variadic: general_variadic,
            }) => match self.ty_kind(specific) {
                Some(TyKind::FunctionPointer {
                    params: specific_params,
                    return_type: specific_return,
                    is_variadic: specific_variadic,
                }) if general_variadic == specific_variadic
                    && general_params.len() == specific_params.len() =>
                {
                    general_params
                        .iter()
                        .zip(specific_params)
                        .all(|(general, specific)| {
                            self.pattern_subsumes_inner(*general, *specific, substitutions)
                        })
                        && self.pattern_subsumes_inner(
                            *general_return,
                            *specific_return,
                            substitutions,
                        )
                }
                _ => false,
            },
            Some(TyKind::Nominal {
                def_id: general_def,
                args: general_args,
            }) => match self.ty_kind(specific) {
                Some(TyKind::Nominal {
                    def_id: specific_def,
                    args: specific_args,
                }) if general_def == specific_def && general_args.len() == specific_args.len() => {
                    general_args
                        .iter()
                        .zip(specific_args)
                        .all(|(general, specific)| {
                            self.pattern_subsumes_inner(*general, *specific, substitutions)
                        })
                }
                _ => false,
            },
            Some(TyKind::Error) | None => false,
        }
    }

    fn patterns_equivalent(&self, left: TyId, right: TyId) -> bool {
        self.pattern_subsumes(left, right) && self.pattern_subsumes(right, left)
    }

    fn types_match(&self, expected: TyId, actual: TyId) -> bool {
        let expected = self.normalize_ty(expected);
        let actual = self.normalize_ty(actual);
        let never = self.input.body_check.interner.primitive(PrimitiveTy::Never);
        expected == actual || never == actual
    }

    fn normalize_ty(&self, ty: TyId) -> TyId {
        if ty.module_id == self.input.type_normalization.interner.module_id() {
            self.input.type_normalization.normalize(ty)
        } else {
            ty
        }
    }

    pub(crate) fn ty_kind(&self, ty: TyId) -> Option<&TyKind> {
        if ty.module_id == self.input.body_check.interner.module_id() {
            return self.input.body_check.interner.get(ty);
        }
        if let Some(extension_interner) = self.input.extension_interner
            && ty.module_id == extension_interner.module_id()
        {
            return extension_interner.get(ty);
        }
        None
    }

    fn array_lens_match(&self, expected: &ArrayLenTy, actual: &ArrayLenTy) -> bool {
        if expected == actual {
            return true;
        }
        let expected = self.array_len_value(expected).ok();
        let actual = self.array_len_value(actual).ok();
        expected.is_some() && expected == actual
    }

    fn array_len_value(&self, len: &ArrayLenTy) -> Result<u64, String> {
        match len {
            ArrayLenTy::ConstExpr { text, span } => self
                .input
                .comptime
                .array_lengths
                .get(span)
                .copied()
                .or_else(|| nia_comptime_engine::eval_array_len_text(text).ok())
                .ok_or_else(|| format!("array length `{text}` was not evaluated by comptime")),
            ArrayLenTy::Builtin { name, ty } => {
                let ty = self.input.type_normalization.normalize(*ty);
                let Some(layout) = self.input.layouts.types.get(&ty) else {
                    return Err(format!(
                        "cannot compute layout for array length builtin `@{name}`"
                    ));
                };
                match name.as_str() {
                    "size" => Ok(layout.size),
                    "align" => Ok(layout.align),
                    _ => Err(format!("unsupported array length builtin `@{name}`")),
                }
            }
            ArrayLenTy::Infer => Err("array length is not concrete".to_string()),
        }
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
        let Some(LocalUse::Local(local_id)) = self.input.locals.uses.get(&expr.span) else {
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
                StmtKind::Binding(binding) => {
                    if self.input.locals.local_defs.get(&stmt.span).copied() == Some(local_id) {
                        return binding.value.as_ref();
                    }
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
                StmtKind::Switch(switch) => {
                    for arm in &switch.arms {
                        let value = match &arm.body {
                            SwitchArmBody::Block(block) => {
                                self.local_comptime_value_in_block(local_id, block)
                            }
                            SwitchArmBody::Stmt(stmt) => {
                                self.local_comptime_value_in_stmt(local_id, stmt)
                            }
                            SwitchArmBody::Expr(_) => None,
                        };
                        if value.is_some() {
                            return value;
                        }
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
            StmtKind::Switch(switch) => switch.arms.iter().find_map(|arm| match &arm.body {
                SwitchArmBody::Block(block) => self.local_comptime_value_in_block(local_id, block),
                SwitchArmBody::Stmt(stmt) => self.local_comptime_value_in_stmt(local_id, stmt),
                SwitchArmBody::Expr(_) => None,
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
                let base = self.lower_place_inner(callee, elems);
                if args.len() == 1
                    && let Some(index) = args.first().and_then(|arg| arg.expr.as_ref())
                {
                    elems.push(PlaceElem::Index(Box::new(self.lower_expr(index))));
                }
                base
            }
            _ => PlaceBase::Local(LocalId(u32::MAX)),
        }
    }
}
