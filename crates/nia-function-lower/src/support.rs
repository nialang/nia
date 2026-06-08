// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) struct LoweringContext<'a> {
    pub(super) scope: FunctionScopeId,
    pub(super) current: &'a mut FunctionBlockId,
    pub(super) ops: &'a mut Vec<FunctionOp>,
    pub(super) blocks: &'a mut Vec<FunctionBlock>,
}

pub(super) struct SwitchPatternConditionContext<'a> {
    pub(super) scope: FunctionScopeId,
    pub(super) current: &'a mut FunctionBlockId,
    pub(super) ops: &'a mut Vec<FunctionOp>,
    pub(super) blocks: &'a mut Vec<FunctionBlock>,
    pub(super) bool_ty: InternedTyId,
}

pub(super) struct SwitchValueArmContext<'a> {
    pub(super) span: Span,
    pub(super) scope: FunctionScopeId,
    pub(super) entry: FunctionBlockId,
    pub(super) result_local: LocalId,
    pub(super) merge_target: FunctionBlockId,
    pub(super) blocks: &'a mut Vec<FunctionBlock>,
    pub(super) patterns: &'a [TypedSwitchPattern],
    pub(super) target: &'a FunctionExpr,
}

pub(super) struct SwitchStmtArmContext<'a> {
    pub(super) span: Span,
    pub(super) scope: FunctionScopeId,
    pub(super) entry: FunctionBlockId,
    pub(super) merge_target: FunctionBlockId,
    pub(super) blocks: &'a mut Vec<FunctionBlock>,
    pub(super) patterns: &'a [TypedSwitchPattern],
    pub(super) target: &'a FunctionExpr,
}

impl FunctionLowerer {
    pub(super) fn try_kind(&self, ty: InternedTyId) -> Option<FunctionTryKind> {
        let interner = self.interner.as_ref()?;
        match interner.get(ty) {
            Some(TyKind::Optional { .. }) => Some(FunctionTryKind::Optional),
            Some(TyKind::ErrorUnion { .. }) => Some(FunctionTryKind::ErrorUnion),
            _ => None,
        }
    }

    pub(super) fn lower_place(
        &mut self,
        place: &TypedPlace,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionPlace {
        FunctionPlace {
            span: place.span,
            ty: place.ty,
            base: match &place.base {
                PlaceBase::Local(local_id) => FunctionPlaceBase::Local(*local_id),
                PlaceBase::Global(def_id) => FunctionPlaceBase::Global(*def_id),
                PlaceBase::Deref(expr) => FunctionPlaceBase::Deref(Box::new(
                    self.lower_value_expr(expr, scope, current, ops, blocks),
                )),
                PlaceBase::Error => FunctionPlaceBase::Error,
            },
            elems: place
                .elems
                .iter()
                .map(|elem| match elem {
                    PlaceElem::Field(field) => FunctionPlaceElem::Field(*field),
                    PlaceElem::Index(index) => FunctionPlaceElem::Index(Box::new(
                        self.lower_value_expr(index, scope, current, ops, blocks),
                    )),
                    PlaceElem::Error => FunctionPlaceElem::Error,
                })
                .collect(),
        }
    }

    pub(super) fn lower_expr_place(
        &mut self,
        expr: &TypedExpr,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionPlace {
        match &expr.kind {
            TypedExprKind::Global(def_id) => FunctionPlace {
                span: expr.span,
                ty: expr.ty,
                base: FunctionPlaceBase::Global(*def_id),
                elems: Vec::new(),
            },
            TypedExprKind::Local(local_id) => FunctionPlace {
                span: expr.span,
                ty: expr.ty,
                base: FunctionPlaceBase::Local(*local_id),
                elems: Vec::new(),
            },
            TypedExprKind::Unary {
                op: UnaryOp::Deref,
                expr: inner,
            } => FunctionPlace {
                span: expr.span,
                ty: expr.ty,
                base: FunctionPlaceBase::Deref(Box::new(
                    self.lower_value_expr(inner, scope, current, ops, blocks),
                )),
                elems: Vec::new(),
            },
            TypedExprKind::Field { lhs, field } => {
                let mut place = self.lower_expr_place(lhs, scope, current, ops, blocks);
                place.span = expr.span;
                place.ty = expr.ty;
                place.elems.push(FunctionPlaceElem::Field(*field));
                place
            }
            TypedExprKind::Index { lhs, index } => {
                let mut place = self.lower_expr_place(lhs, scope, current, ops, blocks);
                place.span = expr.span;
                place.ty = expr.ty;
                place.elems.push(FunctionPlaceElem::Index(Box::new(
                    self.lower_value_expr(index, scope, current, ops, blocks),
                )));
                place
            }
            _ => self.materialize_expr_place(expr, scope, current, ops, blocks),
        }
    }

    fn materialize_expr_place(
        &mut self,
        expr: &TypedExpr,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionPlace {
        let local_id = self.alloc_temp_local(expr.span, expr.ty);
        let value = self.lower_value_expr(expr, scope, current, ops, blocks);
        ops.push(FunctionOp::Binding(FunctionBinding {
            local_id,
            name: format!("fir.tmp.{}", local_id.0),
            ty: expr.ty,
            value: Some(value),
            is_let: false,
        }));
        FunctionPlace {
            span: expr.span,
            ty: expr.ty,
            base: FunctionPlaceBase::Local(local_id),
            elems: Vec::new(),
        }
    }

    pub(super) fn lower_binding(
        &mut self,
        binding: &TypedBinding,
        scope: FunctionScopeId,
        current: &mut FunctionBlockId,
        ops: &mut Vec<FunctionOp>,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionBinding {
        let value = binding.value.as_ref().map(|value| {
            let value = self.lower_value_expr(value, scope, current, ops, blocks);
            self.lower_binding_pattern_value(binding.pattern_kind, binding.ty, value)
        });
        FunctionBinding {
            local_id: binding.local_id,
            name: binding.name.clone(),
            ty: binding.ty,
            value,
            is_let: binding.is_let,
        }
    }

    pub(super) fn lower_binding_pattern_value(
        &mut self,
        pattern_kind: nia_ast::ForPatternKind,
        binding_ty: InternedTyId,
        value: FunctionExpr,
    ) -> FunctionExpr {
        match pattern_kind {
            nia_ast::ForPatternKind::Value => value,
            nia_ast::ForPatternKind::Pointer | nia_ast::ForPatternKind::MutPointer => {
                FunctionExpr {
                    span: value.span,
                    ty: binding_ty,
                    kind: FunctionExprKind::Unary {
                        op: nia_ast::UnaryOp::Deref,
                        expr: Box::new(value),
                    },
                }
            }
        }
    }

    pub(super) fn alloc_temp_local(&mut self, span: Span, ty: InternedTyId) -> LocalId {
        let id = LocalId(self.next_temp_local);
        self.next_temp_local += 1;
        self.temp_locals.push(FunctionLocal {
            id,
            name: format!("fir.tmp.{}", id.0),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        });
        id
    }

    pub(super) fn alloc_block(&mut self) -> FunctionBlockId {
        let id = FunctionBlockId(self.next_block);
        self.next_block += 1;
        id
    }

    pub(super) fn switch_has_range_patterns(&self, switch: &TypedSwitch) -> bool {
        switch.arms.iter().any(|arm| {
            arm.patterns.iter().any(|pattern| {
                !matches!(
                    pattern,
                    TypedSwitchPattern::Expr(_)
                        | TypedSwitchPattern::CheckedInt { .. }
                        | TypedSwitchPattern::Default
                )
            })
        })
    }

    pub(super) fn switch_pattern_binding(
        &self,
        pattern: &TypedSwitchPattern,
    ) -> Option<(LocalId, InternedTyId, Span)> {
        match pattern {
            TypedSwitchPattern::OptionalSome {
                local_id, ty, span, ..
            }
            | TypedSwitchPattern::ErrorOk {
                local_id, ty, span, ..
            }
            | TypedSwitchPattern::ErrorErr {
                local_id, ty, span, ..
            } => Some((*local_id, *ty, *span)),
            _ => None,
        }
    }

    pub(super) fn switch_pattern_condition(
        &mut self,
        target: &FunctionExpr,
        pattern: &TypedSwitchPattern,
        context: &mut SwitchPatternConditionContext<'_>,
    ) -> Option<FunctionExpr> {
        match pattern {
            TypedSwitchPattern::Default => None,
            TypedSwitchPattern::OptionalSome { span, .. } => Some(self.tagged_union_tag_condition(
                target,
                *span,
                context.bool_ty,
                FunctionOptionalTag::Some.discriminant(),
            )),
            TypedSwitchPattern::OptionalNull { span } => Some(self.tagged_union_tag_condition(
                target,
                *span,
                context.bool_ty,
                FunctionOptionalTag::Null.discriminant(),
            )),
            TypedSwitchPattern::ErrorOk { span, .. } => Some(self.tagged_union_tag_condition(
                target,
                *span,
                context.bool_ty,
                FunctionErrorUnionTag::Ok.discriminant(),
            )),
            TypedSwitchPattern::ErrorErr { span, .. } => Some(self.tagged_union_tag_condition(
                target,
                *span,
                context.bool_ty,
                FunctionErrorUnionTag::Err.discriminant(),
            )),
            TypedSwitchPattern::Expr(pattern) => {
                let pattern = self.lower_value_expr(
                    pattern,
                    context.scope,
                    context.current,
                    context.ops,
                    context.blocks,
                );
                Some(self.switch_eq_condition(target, pattern, context.bool_ty))
            }
            TypedSwitchPattern::CheckedInt { value, ty, span } => {
                let pattern = self.checked_int_pattern_expr(*value, *ty, *span);
                Some(self.switch_eq_condition(target, pattern, context.bool_ty))
            }
            TypedSwitchPattern::Range {
                start,
                end,
                inclusive,
                span,
            } => {
                let start = self.lower_value_expr(
                    start,
                    context.scope,
                    context.current,
                    context.ops,
                    context.blocks,
                );
                let end = self.lower_value_expr(
                    end,
                    context.scope,
                    context.current,
                    context.ops,
                    context.blocks,
                );
                Some(self.switch_range_condition(
                    target,
                    start,
                    end,
                    *inclusive,
                    *span,
                    context.bool_ty,
                ))
            }
            TypedSwitchPattern::CheckedIntRange {
                start,
                end,
                inclusive,
                ty,
                span,
            } => {
                let start = self.checked_int_pattern_expr(*start, *ty, *span);
                let end = self.checked_int_pattern_expr(*end, *ty, *span);
                Some(self.switch_range_condition(
                    target,
                    start,
                    end,
                    *inclusive,
                    *span,
                    context.bool_ty,
                ))
            }
        }
    }

    fn switch_eq_condition(
        &self,
        target: &FunctionExpr,
        pattern: FunctionExpr,
        bool_ty: InternedTyId,
    ) -> FunctionExpr {
        FunctionExpr {
            span: pattern.span,
            ty: bool_ty,
            kind: FunctionExprKind::Binary {
                lhs: Box::new(target.clone()),
                op: BinaryOp::Eq,
                rhs: Box::new(pattern),
            },
        }
    }

    fn switch_range_condition(
        &self,
        target: &FunctionExpr,
        start: FunctionExpr,
        end: FunctionExpr,
        inclusive: bool,
        span: Span,
        bool_ty: InternedTyId,
    ) -> FunctionExpr {
        let lower = FunctionExpr {
            span,
            ty: bool_ty,
            kind: FunctionExprKind::Binary {
                lhs: Box::new(target.clone()),
                op: BinaryOp::Ge,
                rhs: Box::new(start),
            },
        };
        let upper = FunctionExpr {
            span,
            ty: bool_ty,
            kind: FunctionExprKind::Binary {
                lhs: Box::new(target.clone()),
                op: if inclusive {
                    BinaryOp::Le
                } else {
                    BinaryOp::Lt
                },
                rhs: Box::new(end),
            },
        };
        FunctionExpr {
            span,
            ty: bool_ty,
            kind: FunctionExprKind::Binary {
                lhs: Box::new(lower),
                op: BinaryOp::And,
                rhs: Box::new(upper),
            },
        }
    }

    pub(super) fn checked_int_pattern_expr(
        &self,
        value: i128,
        ty: InternedTyId,
        span: Span,
    ) -> FunctionExpr {
        FunctionExpr {
            span,
            ty,
            kind: FunctionExprKind::Integer(value.to_string()),
        }
    }

    pub(super) fn lower_switch_pattern_bindings(
        &self,
        patterns: &[TypedSwitchPattern],
        target: &FunctionExpr,
        ops: &mut Vec<FunctionOp>,
    ) {
        for pattern in patterns {
            let Some((local_id, ty, span)) = self.switch_pattern_binding(pattern) else {
                continue;
            };
            ops.push(FunctionOp::StoreLocal {
                local_id,
                value: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::TaggedUnionPayload {
                        expr: Box::new(target.clone()),
                    },
                },
                span,
            });
        }
    }

    pub(super) fn tagged_union_tag_condition(
        &self,
        target: &FunctionExpr,
        span: Span,
        bool_ty: InternedTyId,
        tag: u8,
    ) -> FunctionExpr {
        let tag_ty = self
            .interner
            .as_ref()
            .map(|interner| interner.primitive(nia_ty::PrimitiveTy::U8))
            .unwrap_or(target.ty);
        FunctionExpr {
            span,
            ty: bool_ty,
            kind: FunctionExprKind::Binary {
                lhs: Box::new(FunctionExpr {
                    span,
                    ty: tag_ty,
                    kind: FunctionExprKind::TaggedUnionTag {
                        expr: Box::new(target.clone()),
                    },
                }),
                op: BinaryOp::Eq,
                rhs: Box::new(FunctionExpr {
                    span,
                    ty: tag_ty,
                    kind: FunctionExprKind::Integer(tag.to_string()),
                }),
            },
        }
    }

    pub(super) fn alloc_scope(
        &mut self,
        parent: Option<FunctionScopeId>,
        span: Span,
    ) -> FunctionScopeId {
        let id = FunctionScopeId(self.next_scope);
        self.next_scope += 1;
        self.scopes.push(FunctionScope { id, parent, span });
        id
    }

    pub(super) fn collect_body_locals(&self, body: &TypedBody, locals: &mut Vec<FunctionLocal>) {
        self.extend_unique_locals(&body.locals, locals);
        self.collect_nested_body_locals(body, locals);
    }

    pub(super) fn collect_nested_body_locals(
        &self,
        body: &TypedBody,
        locals: &mut Vec<FunctionLocal>,
    ) {
        for stmt in &body.stmts {
            match &stmt.kind {
                TypedStmtKind::ForIn(for_stmt) => self.collect_body_locals(&for_stmt.body, locals),
                TypedStmtKind::While(while_stmt) => {
                    self.collect_body_locals(&while_stmt.body, locals)
                }
                TypedStmtKind::Loop(loop_stmt) => self.collect_body_locals(&loop_stmt.body, locals),
                TypedStmtKind::Expr(expr) => self.collect_expr_locals(expr, locals),
                TypedStmtKind::Return(Some(expr)) | TypedStmtKind::Defer(expr) => {
                    self.collect_expr_locals(expr, locals)
                }
                TypedStmtKind::Binding(_)
                | TypedStmtKind::Return(None)
                | TypedStmtKind::Break
                | TypedStmtKind::Continue => {}
            }
        }
        if let Some(tail) = &body.tail {
            self.collect_expr_locals(tail, locals);
        }
    }

    pub(super) fn collect_expr_locals(&self, expr: &TypedExpr, locals: &mut Vec<FunctionLocal>) {
        match &expr.kind {
            TypedExprKind::Block(body) => self.collect_body_locals(body, locals),
            TypedExprKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                self.collect_body_locals(then_branch, locals);
                if let Some(else_branch) = else_branch {
                    self.collect_expr_locals(else_branch, locals);
                }
            }
            TypedExprKind::Switch(switch) => {
                for arm in &switch.arms {
                    match &arm.body {
                        TypedSwitchArmBody::Expr(expr) => self.collect_expr_locals(expr, locals),
                        TypedSwitchArmBody::Stmt(stmt) => {
                            if let TypedStmtKind::Expr(expr) = &stmt.kind {
                                self.collect_expr_locals(expr, locals);
                            }
                        }
                        TypedSwitchArmBody::Block(body) => self.collect_body_locals(body, locals),
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn extend_unique_locals(
        &self,
        source: &[TypedLocal],
        target: &mut Vec<FunctionLocal>,
    ) {
        for local in source {
            if !target.iter().any(|existing| existing.id == local.id) {
                target.push(Self::lower_local(local));
            }
        }
    }

    pub(super) fn lower_local(local: &TypedLocal) -> FunctionLocal {
        FunctionLocal {
            id: local.id,
            name: local.name.clone(),
            kind: Self::lower_local_kind(local.kind),
            ty: local.ty,
            span: local.span,
        }
    }

    pub(super) fn lower_local_kind(kind: TypedLocalKind) -> FunctionLocalKind {
        match kind {
            TypedLocalKind::Param => FunctionLocalKind::Param,
            TypedLocalKind::Binding => FunctionLocalKind::Binding,
            TypedLocalKind::ConstBinding => FunctionLocalKind::ConstBinding,
        }
    }

    pub(super) fn lower_builtin_value(value: &BuiltinConst) -> FunctionBuiltinValue {
        match value {
            BuiltinConst::Usize(value) => FunctionBuiltinValue::Usize(*value),
            BuiltinConst::Layout { builtin, ty } => FunctionBuiltinValue::Layout {
                builtin: *builtin,
                ty: *ty,
            },
            BuiltinConst::Int(value) => FunctionBuiltinValue::Int(*value),
        }
    }

    pub(super) fn lower_asm_option(option: &AsmOption) -> FunctionAsmOption {
        match option {
            AsmOption::Volatile => FunctionAsmOption::Volatile,
        }
    }

    pub(super) fn next_available_local(&self, body: &TypedBody) -> u32 {
        // Temporaries are allocated after every source local referenced by the
        // typed body, including locals hidden inside expression blocks, switch
        // arms, and defer bodies. This keeps generated locals disjoint from
        // ids assigned before function lowering.
        pub(super) fn visit_body(body: &TypedBody, max_id: &mut u32) {
            for local in &body.locals {
                *max_id = (*max_id).max(local.id.0.saturating_add(1));
            }
            for stmt in &body.stmts {
                match &stmt.kind {
                    TypedStmtKind::ForIn(for_stmt) => {
                        if let Some(binding) = &for_stmt.binding {
                            *max_id = (*max_id).max(binding.local_id.0.saturating_add(1));
                        }
                        visit_expr(&for_stmt.iter, max_id);
                        visit_body(&for_stmt.body, max_id);
                    }
                    TypedStmtKind::While(while_stmt) => {
                        visit_expr(&while_stmt.cond, max_id);
                        visit_body(&while_stmt.body, max_id);
                    }
                    TypedStmtKind::Loop(loop_stmt) => visit_body(&loop_stmt.body, max_id),
                    TypedStmtKind::Expr(expr)
                    | TypedStmtKind::Return(Some(expr))
                    | TypedStmtKind::Defer(expr) => visit_expr(expr, max_id),
                    TypedStmtKind::Binding(binding) => {
                        *max_id = (*max_id).max(binding.local_id.0.saturating_add(1));
                        if let Some(value) = &binding.value {
                            visit_expr(value, max_id);
                        }
                    }
                    TypedStmtKind::Return(None)
                    | TypedStmtKind::Break
                    | TypedStmtKind::Continue => {}
                }
            }
            if let Some(tail) = &body.tail {
                visit_expr(tail, max_id);
            }
        }

        pub(super) fn visit_expr(expr: &TypedExpr, max_id: &mut u32) {
            match &expr.kind {
                TypedExprKind::Local(local_id) => {
                    *max_id = (*max_id).max(local_id.0.saturating_add(1));
                }
                TypedExprKind::CStringPointer { array: inner, .. }
                | TypedExprKind::Unary { expr: inner, .. }
                | TypedExprKind::OptionalSome { expr: inner }
                | TypedExprKind::ErrorOk { expr: inner }
                | TypedExprKind::ErrorErr { expr: inner }
                | TypedExprKind::Try { expr: inner }
                | TypedExprKind::Discard(inner)
                | TypedExprKind::Cast { expr: inner, .. }
                | TypedExprKind::TraitObjectUpcast { expr: inner, .. }
                | TypedExprKind::TraitObjectCoercion { expr: inner, .. } => {
                    visit_expr(inner, max_id)
                }
                TypedExprKind::Range(range) => {
                    if let Some(start) = &range.start {
                        visit_expr(start, max_id);
                    }
                    if let Some(end) = &range.end {
                        visit_expr(end, max_id);
                    }
                }
                TypedExprKind::ArrayLiteral { elems } => match elems {
                    TypedArrayElements::List(elems) => {
                        for elem in elems {
                            visit_expr(elem, max_id);
                        }
                    }
                    TypedArrayElements::Repeat { value, .. } => visit_expr(value, max_id),
                },
                TypedExprKind::StructLiteral { fields, .. } => {
                    for field in fields {
                        visit_expr(&field.value, max_id);
                    }
                }
                TypedExprKind::UnionLiteral { field, .. } => visit_expr(&field.value, max_id),
                TypedExprKind::Binary { lhs, rhs, .. }
                | TypedExprKind::Index { lhs, index: rhs } => {
                    visit_expr(lhs, max_id);
                    visit_expr(rhs, max_id);
                }
                TypedExprKind::Assign { place, rhs, .. } => {
                    visit_place(place, max_id);
                    visit_expr(rhs, max_id);
                }
                TypedExprKind::Call { callee, args } => {
                    visit_callee(callee, max_id);
                    for arg in args {
                        visit_expr(arg, max_id);
                    }
                }
                TypedExprKind::Field { lhs, .. } => visit_expr(lhs, max_id),
                TypedExprKind::Slice { lhs, range, .. } => {
                    visit_expr(lhs, max_id);
                    if let Some(start) = &range.start {
                        visit_expr(start, max_id);
                    }
                    if let Some(end) = &range.end {
                        visit_expr(end, max_id);
                    }
                }
                TypedExprKind::Block(body) => visit_body(body, max_id),
                TypedExprKind::If {
                    cond,
                    then_branch,
                    else_branch,
                } => {
                    visit_expr(cond, max_id);
                    visit_body(then_branch, max_id);
                    if let Some(else_branch) = else_branch {
                        visit_expr(else_branch, max_id);
                    }
                }
                TypedExprKind::Switch(switch) => {
                    visit_expr(&switch.target, max_id);
                    for arm in &switch.arms {
                        for pattern in &arm.patterns {
                            match pattern {
                                TypedSwitchPattern::Default => {}
                                TypedSwitchPattern::OptionalSome { .. }
                                | TypedSwitchPattern::OptionalNull { .. }
                                | TypedSwitchPattern::ErrorOk { .. }
                                | TypedSwitchPattern::ErrorErr { .. }
                                | TypedSwitchPattern::CheckedInt { .. }
                                | TypedSwitchPattern::CheckedIntRange { .. } => {}
                                TypedSwitchPattern::Expr(pattern) => visit_expr(pattern, max_id),
                                TypedSwitchPattern::Range { start, end, .. } => {
                                    visit_expr(start, max_id);
                                    visit_expr(end, max_id);
                                }
                            }
                        }
                        match &arm.body {
                            TypedSwitchArmBody::Expr(expr) => visit_expr(expr, max_id),
                            TypedSwitchArmBody::Stmt(stmt) => match &stmt.kind {
                                TypedStmtKind::Expr(expr)
                                | TypedStmtKind::Return(Some(expr))
                                | TypedStmtKind::Defer(expr) => visit_expr(expr, max_id),
                                TypedStmtKind::Binding(binding) => {
                                    *max_id = (*max_id).max(binding.local_id.0.saturating_add(1));
                                    if let Some(value) = &binding.value {
                                        visit_expr(value, max_id);
                                    }
                                }
                                TypedStmtKind::ForIn(for_stmt) => {
                                    if let Some(binding) = &for_stmt.binding {
                                        *max_id =
                                            (*max_id).max(binding.local_id.0.saturating_add(1));
                                    }
                                    visit_expr(&for_stmt.iter, max_id);
                                    visit_body(&for_stmt.body, max_id);
                                }
                                TypedStmtKind::While(while_stmt) => {
                                    visit_expr(&while_stmt.cond, max_id);
                                    visit_body(&while_stmt.body, max_id);
                                }
                                TypedStmtKind::Loop(loop_stmt) => {
                                    visit_body(&loop_stmt.body, max_id)
                                }
                                TypedStmtKind::Return(None)
                                | TypedStmtKind::Break
                                | TypedStmtKind::Continue => {}
                            },
                            TypedSwitchArmBody::Block(body) => visit_body(body, max_id),
                        }
                    }
                }
                TypedExprKind::InlineAsm(asm) => {
                    for input in &asm.inputs {
                        visit_expr(&input.value, max_id);
                    }
                    for output in &asm.outputs {
                        visit_place(&output.place, max_id);
                    }
                }
                TypedExprKind::MemoryIntrinsic(memory) => {
                    visit_expr(&memory.dest, max_id);
                    match &memory.source {
                        TypedMemoryIntrinsicSource::Slice(source)
                        | TypedMemoryIntrinsicSource::Byte(source) => {
                            visit_expr(source, max_id);
                        }
                    }
                }
                TypedExprKind::Error
                | TypedExprKind::Integer(_)
                | TypedExprKind::Float(_)
                | TypedExprKind::String(_)
                | TypedExprKind::ByteString(_)
                | TypedExprKind::Char(_)
                | TypedExprKind::ByteChar(_)
                | TypedExprKind::Bool(_)
                | TypedExprKind::Null
                | TypedExprKind::Global(_)
                | TypedExprKind::Function(_)
                | TypedExprKind::FunctionInstance { .. }
                | TypedExprKind::EnumVariant(_)
                | TypedExprKind::BuiltinValue(_)
                | TypedExprKind::Trap => {}
            }
        }

        pub(super) fn visit_callee(callee: &TypedCallee, max_id: &mut u32) {
            match callee {
                TypedCallee::Method { receiver, .. }
                | TypedCallee::TraitMethod { receiver, .. }
                | TypedCallee::DynamicTraitMethod { receiver, .. }
                | TypedCallee::BuiltinMethod { receiver, .. }
                | TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod { receiver, .. })
                | TypedCallee::FunctionPointer(receiver) => {
                    visit_expr(receiver, max_id);
                }
                TypedCallee::Function(_)
                | TypedCallee::FunctionInstance { .. }
                | TypedCallee::BuiltinOperator(_) => {}
            }
        }

        pub(super) fn visit_place(place: &TypedPlace, max_id: &mut u32) {
            if let PlaceBase::Local(local_id) = place.base {
                *max_id = (*max_id).max(local_id.0.saturating_add(1));
            }
            if let PlaceBase::Deref(expr) = &place.base {
                visit_expr(expr, max_id);
            }
            for elem in &place.elems {
                if let PlaceElem::Index(index) = elem {
                    visit_expr(index, max_id);
                }
            }
        }

        let mut max_id = 0;
        visit_body(body, &mut max_id);
        max_id
    }
}
