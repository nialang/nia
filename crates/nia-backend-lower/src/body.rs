// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{ModuleLowerer, generic_inst_base};
use nia_ast::{
    ArrayElements, BindingStmt, Block, Expr, ExprKind, ForHeader, ForInit, IndexArg, SliceRange,
    Stmt, StmtKind, SwitchArmBody, SwitchPattern, UnaryOp,
};
use nia_backend_ir::{
    BuiltinConst, PlaceBase, PlaceElem, TypedArrayElements, TypedBinding, TypedBody, TypedCallee,
    TypedExpr, TypedExprKind, TypedFieldInit, TypedFor, TypedForHeader, TypedForInit, TypedLocal,
    TypedLocalKind, TypedPlace, TypedSliceRange, TypedStmt, TypedStmtKind, TypedSwitch,
    TypedSwitchArm, TypedSwitchArmBody, TypedSwitchPattern,
};
use nia_body_check::BuiltinValue;
use nia_defs::DefKind;
use nia_ids::{GlobalDefId, LocalId};
use nia_local_resolve::{LocalKind, LocalUse};
use nia_span::Span;
use nia_ty::TyKind;
use nia_value_resolve::ValueNameResolution;

use crate::literals::decode_string_literal;

mod asm;

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn lower_body(&mut self, block: &Block) -> TypedBody {
        let stmts = block
            .stmts
            .iter()
            .filter(|stmt| !matches!(stmt.kind, StmtKind::Using(_)))
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
                self.current_param_locals.contains(id)
                    || (body_span.start <= local.span.start && local.span.end <= body_span.end)
            })
            .map(|(id, local)| TypedLocal {
                id,
                name: local.name.clone(),
                kind: match local.kind {
                    LocalKind::Param => TypedLocalKind::Param,
                    LocalKind::Binding => TypedLocalKind::Binding,
                    LocalKind::ConstBinding => TypedLocalKind::ConstBinding,
                },
                ty: self.local_ty(id).unwrap_or_else(|| self.error_ty()),
                span: local.span,
            })
            .collect()
    }

    fn lower_stmt(&mut self, stmt: &Stmt) -> TypedStmt {
        let kind = match &stmt.kind {
            StmtKind::Using(_) => {
                unreachable!("using statements should be filtered out before lowering")
            }
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
                            TypedForInit::Binding(self.lower_binding_stmt(*span, binding))
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
            ExprKind::Integer(text) => TypedExprKind::Integer(text.clone()),
            ExprKind::Float(text) => TypedExprKind::Float(text.clone()),
            ExprKind::String(text) => {
                TypedExprKind::String(decode_string_literal(text).unwrap_or_default())
            }
            ExprKind::Char(text) => TypedExprKind::Char(text.clone()),
            ExprKind::ByteChar(text) => TypedExprKind::ByteChar(text.clone()),
            ExprKind::Bool(value) => TypedExprKind::Bool(*value),
            ExprKind::Ident(_) => self.lower_ident_expr(expr),
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
            Some(LocalUse::Local(local)) => TypedExprKind::Local(*local),
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
                            _ => TypedExprKind::Error,
                        }
                    }
                    _ => TypedExprKind::Error,
                }
            }
            _ => TypedExprKind::Error,
        }
    }

    fn def_kind_of(&self, global_id: GlobalDefId) -> Option<DefKind> {
        self.input
            .all_defs
            .iter()
            .find(|defs| defs.module_id == global_id.module_id)
            .and_then(|defs| defs.defs.get(global_id.def_id))
            .map(|def| def.kind)
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
                count: nia_const_eval::eval_array_len_text(&count.text).unwrap_or(0),
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
            && let Some(method_id) = self.single_method_def(struct_id, name)
        {
            if struct_args.is_empty() {
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
        let (struct_id, struct_args) = self.receiver_base_type(receiver.ty)?;
        let method_id = self.single_method_def(struct_id, name)?;
        let mut args = struct_args;
        args.extend(type_args);
        Some(TypedCallee::Method {
            def_id: method_id,
            args,
            receiver: Box::new(receiver),
        })
    }

    fn single_method_def(&self, struct_id: GlobalDefId, name: &str) -> Option<GlobalDefId> {
        let methods = self.input.extensions.methods(struct_id, name);
        match methods.as_slice() {
            [method] => Some(*method),
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
