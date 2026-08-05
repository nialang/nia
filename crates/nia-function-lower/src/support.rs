// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) struct LoweringContext<'a> {
    pub(super) scope: FunctionScopeId,
    pub(super) current: &'a mut FunctionBlockId,
    pub(super) ops: &'a mut Vec<FunctionOp>,
    pub(super) blocks: &'a mut Vec<FunctionBlock>,
}

pub(super) struct PatternConditionContext<'a> {
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
}

pub(super) struct SwitchStmtArmContext<'a> {
    pub(super) span: Span,
    pub(super) scope: FunctionScopeId,
    pub(super) entry: FunctionBlockId,
    pub(super) merge_target: FunctionBlockId,
    pub(super) blocks: &'a mut Vec<FunctionBlock>,
}

impl FunctionLowerer<'_> {
    pub(super) fn try_kind(&self, ty: InternedTyId) -> Option<FunctionTryKind> {
        match self.types.get(ty) {
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
                PlaceBase::Error => unreachable!(
                    "function lowering input validation rejects error places before lowering"
                ),
            },
            elems: place
                .elems
                .iter()
                .map(|elem| match elem {
                    PlaceElem::Field(field) => FunctionPlaceElem::Field(*field),
                    PlaceElem::Index(index) => FunctionPlaceElem::Index(Box::new(
                        self.lower_value_expr(index, scope, current, ops, blocks),
                    )),
                    PlaceElem::Error => unreachable!(
                        "function lowering input validation rejects error place elements before lowering"
                    ),
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
            name: LocalName::temporary(local_id.0),
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
        let value = binding
            .value
            .as_ref()
            .map(|value| self.lower_value_expr(value, scope, current, ops, blocks));
        FunctionBinding {
            local_id: binding.local_id,
            name: binding.name,
            ty: binding.ty,
            value,
            is_let: !binding.is_mutable,
        }
    }

    pub(super) fn alloc_temp_local(&mut self, span: Span, ty: InternedTyId) -> LocalId {
        let id = LocalId(self.next_temp_local);
        self.next_temp_local += 1;
        self.temp_locals.push(FunctionLocal {
            id,
            name: LocalName::temporary(id.0),
            kind: FunctionLocalKind::MutableBinding,
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

    pub(super) fn switch_requires_pattern_chain(&self, switch: &TypedSwitch) -> bool {
        switch.arms.iter().any(|arm| {
            arm.patterns.iter().any(|pattern| {
                !matches!(
                    &pattern.kind,
                    TypedPatternKind::Expr(_)
                        | TypedPatternKind::CheckedInt { .. }
                        | TypedPatternKind::Wildcard
                ) && !matches!(
                    &pattern.kind,
                    TypedPatternKind::EnumVariant { fields, .. } if fields.is_empty()
                )
            })
        })
    }

    pub(super) fn direct_switch_target(
        &self,
        switch: &TypedSwitch,
        target: FunctionExpr,
    ) -> FunctionExpr {
        let Some(backing_type) =
            switch
                .arms
                .iter()
                .flat_map(|arm| &arm.patterns)
                .find_map(|pattern| match &pattern.kind {
                    TypedPatternKind::EnumVariant {
                        backing_type,
                        fields,
                        ..
                    } if fields.is_empty() => Some(*backing_type),
                    _ => None,
                })
        else {
            return target;
        };
        FunctionExpr {
            span: target.span,
            ty: backing_type,
            kind: FunctionExprKind::EnumTag {
                value: Box::new(target),
            },
        }
    }

    pub(super) fn direct_enum_switch_pattern(
        &self,
        pattern: &TypedPattern,
    ) -> Option<FunctionExpr> {
        let TypedPatternKind::EnumVariant {
            variant,
            backing_type,
            fields,
        } = &pattern.kind
        else {
            return None;
        };
        fields.is_empty().then_some(FunctionExpr {
            span: pattern.span,
            ty: *backing_type,
            kind: FunctionExprKind::EnumVariantTag(*variant),
        })
    }

    pub(super) fn pattern_condition(
        &mut self,
        target: &FunctionExpr,
        pattern: &TypedPattern,
        context: &mut PatternConditionContext<'_>,
    ) -> Option<FunctionExpr> {
        match &pattern.kind {
            TypedPatternKind::Wildcard | TypedPatternKind::Bind { .. } => None,
            TypedPatternKind::Pointer(inner) | TypedPatternKind::MutPointer(inner) => {
                self.pattern_condition(target, inner, context)
            }
            TypedPatternKind::OptionalSome(inner) => {
                let tag = self.tagged_union_tag_condition(
                    target,
                    pattern.span,
                    context.bool_ty,
                    FunctionOptionalTag::Some.discriminant(),
                );
                self.pattern_payload_condition(target, inner, tag, context)
            }
            TypedPatternKind::OptionalNull => Some(self.tagged_union_tag_condition(
                target,
                pattern.span,
                context.bool_ty,
                FunctionOptionalTag::Null.discriminant(),
            )),
            TypedPatternKind::ErrorOk(inner) => {
                let tag = self.tagged_union_tag_condition(
                    target,
                    pattern.span,
                    context.bool_ty,
                    FunctionErrorUnionTag::Ok.discriminant(),
                );
                self.pattern_payload_condition(target, inner, tag, context)
            }
            TypedPatternKind::ErrorErr(inner) => {
                let tag = self.tagged_union_tag_condition(
                    target,
                    pattern.span,
                    context.bool_ty,
                    FunctionErrorUnionTag::Err.discriminant(),
                );
                self.pattern_payload_condition(target, inner, tag, context)
            }
            TypedPatternKind::EnumVariant {
                variant,
                backing_type,
                fields,
            } => {
                let tag = FunctionExpr {
                    span: pattern.span,
                    ty: *backing_type,
                    kind: FunctionExprKind::EnumTag {
                        value: Box::new(target.clone()),
                    },
                };
                let expected = FunctionExpr {
                    span: pattern.span,
                    ty: *backing_type,
                    kind: FunctionExprKind::EnumVariantTag(*variant),
                };
                let mut condition = self.switch_eq_condition(&tag, expected, context.bool_ty);
                for (field, field_pattern) in fields.iter().enumerate() {
                    let payload = FunctionExpr {
                        span: field_pattern.span,
                        ty: field_pattern.ty,
                        kind: FunctionExprKind::EnumPayloadField {
                            value: Box::new(target.clone()),
                            variant: *variant,
                            field,
                        },
                    };
                    if let Some(field_condition) =
                        self.pattern_condition(&payload, field_pattern, context)
                    {
                        condition = FunctionExpr {
                            span: pattern.span,
                            ty: context.bool_ty,
                            kind: FunctionExprKind::Binary {
                                lhs: Box::new(condition),
                                op: BinaryOp::And,
                                rhs: Box::new(field_condition),
                            },
                        };
                    }
                }
                Some(condition)
            }
            TypedPatternKind::Expr(pattern) => {
                let pattern = self.lower_value_expr(
                    pattern,
                    context.scope,
                    context.current,
                    context.ops,
                    context.blocks,
                );
                Some(self.switch_eq_condition(target, pattern, context.bool_ty))
            }
            TypedPatternKind::CheckedInt { value } => {
                let pattern = self.checked_int_pattern_expr(*value, pattern.ty, pattern.span);
                Some(self.switch_eq_condition(target, pattern, context.bool_ty))
            }
            TypedPatternKind::Range {
                start,
                end,
                inclusive,
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
                    pattern.span,
                    context.bool_ty,
                ))
            }
            TypedPatternKind::CheckedIntRange {
                start,
                end,
                inclusive,
            } => {
                let start = self.checked_int_pattern_expr(*start, pattern.ty, pattern.span);
                let end = self.checked_int_pattern_expr(*end, pattern.ty, pattern.span);
                Some(self.switch_range_condition(
                    target,
                    start,
                    end,
                    *inclusive,
                    pattern.span,
                    context.bool_ty,
                ))
            }
        }
    }

    fn pattern_payload_condition(
        &mut self,
        target: &FunctionExpr,
        inner: &TypedPattern,
        tag: FunctionExpr,
        context: &mut PatternConditionContext<'_>,
    ) -> Option<FunctionExpr> {
        let payload = self.tagged_union_payload_expr(target, inner.ty, inner.span);
        if let Some(payload_condition) = self.pattern_condition(&payload, inner, context) {
            Some(FunctionExpr {
                span: inner.span,
                ty: context.bool_ty,
                kind: FunctionExprKind::Binary {
                    lhs: Box::new(tag),
                    op: BinaryOp::And,
                    rhs: Box::new(payload_condition),
                },
            })
        } else {
            Some(tag)
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

    pub(super) fn lower_pattern_binding(
        &self,
        pattern: &TypedPattern,
        target: &FunctionExpr,
        ops: &mut Vec<FunctionOp>,
    ) {
        match &pattern.kind {
            TypedPatternKind::Bind { local_id, .. } => {
                ops.push(FunctionOp::StoreLocal {
                    local_id: *local_id,
                    value: target.clone(),
                    span: pattern.span,
                });
            }
            TypedPatternKind::Pointer(inner) | TypedPatternKind::MutPointer(inner) => {
                let value = FunctionExpr {
                    span: target.span,
                    ty: inner.ty,
                    kind: FunctionExprKind::Unary {
                        op: nia_ast::UnaryOp::Deref,
                        expr: Box::new(target.clone()),
                    },
                };
                self.lower_pattern_binding(inner, &value, ops);
            }
            TypedPatternKind::OptionalSome(inner)
            | TypedPatternKind::ErrorOk(inner)
            | TypedPatternKind::ErrorErr(inner) => {
                let payload = self.tagged_union_payload_expr(target, inner.ty, inner.span);
                self.lower_pattern_binding(inner, &payload, ops);
            }
            TypedPatternKind::EnumVariant {
                variant, fields, ..
            } => {
                for (field, field_pattern) in fields.iter().enumerate() {
                    let payload = FunctionExpr {
                        span: field_pattern.span,
                        ty: field_pattern.ty,
                        kind: FunctionExprKind::EnumPayloadField {
                            value: Box::new(target.clone()),
                            variant: *variant,
                            field,
                        },
                    };
                    self.lower_pattern_binding(field_pattern, &payload, ops);
                }
            }
            TypedPatternKind::Wildcard
            | TypedPatternKind::OptionalNull
            | TypedPatternKind::Expr(_)
            | TypedPatternKind::CheckedInt { .. }
            | TypedPatternKind::Range { .. }
            | TypedPatternKind::CheckedIntRange { .. } => {}
        }
    }

    pub(super) fn lower_pattern_arm_entry(
        &mut self,
        pattern: &TypedPattern,
        target: &FunctionExpr,
        scope: FunctionScopeId,
        entry: FunctionBlockId,
        span: Span,
        blocks: &mut Vec<FunctionBlock>,
    ) -> FunctionBlockId {
        let mut ops = Vec::new();
        self.lower_pattern_binding(pattern, target, &mut ops);
        if ops.is_empty() {
            return entry;
        }
        let body_entry = self.alloc_block();
        self.finish_block(
            blocks,
            entry,
            scope,
            span,
            ops,
            FunctionTerminator::Branch {
                target: body_entry,
                span,
            },
        );
        body_entry
    }

    pub(super) fn tagged_union_payload_expr(
        &self,
        target: &FunctionExpr,
        ty: InternedTyId,
        span: Span,
    ) -> FunctionExpr {
        FunctionExpr {
            span,
            ty,
            kind: FunctionExprKind::TaggedUnionPayload {
                expr: Box::new(target.clone()),
            },
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
            .types
            .intern(TyKind::Primitive(nia_ty::PrimitiveTy::U8));
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
            TypedExprKind::IfPattern(if_pattern) => {
                self.collect_body_locals(&if_pattern.then_branch, locals);
                if let Some(else_branch) = &if_pattern.else_branch {
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
            name: local.name,
            kind: Self::lower_local_kind(local.kind),
            ty: local.ty,
            span: local.span,
        }
    }

    pub(super) fn lower_local_kind(kind: TypedLocalKind) -> FunctionLocalKind {
        match kind {
            TypedLocalKind::Param => FunctionLocalKind::Param,
            TypedLocalKind::MutableBinding => FunctionLocalKind::MutableBinding,
            TypedLocalKind::ImmutableBinding => FunctionLocalKind::ImmutableBinding,
        }
    }

    pub(super) fn lower_builtin_value(value: &BuiltinConst) -> FunctionBuiltinValue {
        match value {
            BuiltinConst::Usize(value) => FunctionBuiltinValue::Usize(*value),
            BuiltinConst::Layout { builtin, ty } => FunctionBuiltinValue::Layout {
                builtin: *builtin,
                ty: *ty,
            },
            BuiltinConst::FieldOffset { ty, field } => FunctionBuiltinValue::FieldOffset {
                ty: *ty,
                field: *field,
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
                        visit_pattern(&for_stmt.pattern, max_id);
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
                TypedExprKind::StaticArrayPointer { array: inner, .. }
                | TypedExprKind::Unary { expr: inner, .. }
                | TypedExprKind::OptionalSome { expr: inner }
                | TypedExprKind::ErrorOk { expr: inner }
                | TypedExprKind::ErrorErr { expr: inner }
                | TypedExprKind::Try { expr: inner }
                | TypedExprKind::LoadUnaligned { ptr: inner, .. }
                | TypedExprKind::Splat { value: inner }
                | TypedExprKind::Bitmask { vector: inner }
                | TypedExprKind::BitIntrinsic { value: inner, .. }
                | TypedExprKind::CharFromU32 { value: inner }
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
                TypedExprKind::EnumVariant { fields, .. } => {
                    for field in fields {
                        visit_expr(field, max_id);
                    }
                }
                TypedExprKind::UnionLiteral { field, .. } => visit_expr(&field.value, max_id),
                TypedExprKind::Binary { lhs, rhs, .. }
                | TypedExprKind::Index { lhs, index: rhs }
                | TypedExprKind::ExtractElement {
                    vector: lhs,
                    index: rhs,
                } => {
                    visit_expr(lhs, max_id);
                    visit_expr(rhs, max_id);
                }
                TypedExprKind::InsertElement {
                    vector,
                    index,
                    value,
                } => {
                    visit_expr(vector, max_id);
                    visit_expr(index, max_id);
                    visit_expr(value, max_id);
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
                            visit_pattern(pattern, max_id);
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
                                    visit_pattern(&for_stmt.pattern, max_id);
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
                TypedExprKind::IfPattern(if_pattern) => {
                    visit_expr(&if_pattern.target, max_id);
                    visit_pattern(&if_pattern.pattern, max_id);
                    visit_body(&if_pattern.then_branch, max_id);
                    if let Some(else_branch) = &if_pattern.else_branch {
                        visit_expr(else_branch, max_id);
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
                TypedExprKind::Atomic(atomic) => match atomic {
                    nia_body_ir::TypedAtomic::Load { ptr, .. } => visit_expr(ptr, max_id),
                    nia_body_ir::TypedAtomic::Store { ptr, value, .. }
                    | nia_body_ir::TypedAtomic::Rmw { ptr, value, .. } => {
                        visit_expr(ptr, max_id);
                        visit_expr(value, max_id);
                    }
                    nia_body_ir::TypedAtomic::Cmpxchg {
                        ptr,
                        expected,
                        desired,
                        ..
                    } => {
                        visit_expr(ptr, max_id);
                        visit_expr(expected, max_id);
                        visit_expr(desired, max_id);
                    }
                    nia_body_ir::TypedAtomic::Fence { .. } => {}
                },
                TypedExprKind::Error
                | TypedExprKind::Integer(_)
                | TypedExprKind::Float(_)
                | TypedExprKind::String(_)
                | TypedExprKind::ByteString(_)
                | TypedExprKind::Char(_)
                | TypedExprKind::ByteChar(_)
                | TypedExprKind::Bool(_)
                | TypedExprKind::Null
                | TypedExprKind::ConstGeneric(_)
                | TypedExprKind::Global(_)
                | TypedExprKind::Function(_)
                | TypedExprKind::FunctionInstance { .. }
                | TypedExprKind::UnionStorageLiteral { .. }
                | TypedExprKind::BuiltinValue(_)
                | TypedExprKind::Trap => {}
            }
        }

        pub(super) fn visit_pattern(pattern: &TypedPattern, max_id: &mut u32) {
            match &pattern.kind {
                TypedPatternKind::Pointer(pattern)
                | TypedPatternKind::MutPointer(pattern)
                | TypedPatternKind::OptionalSome(pattern)
                | TypedPatternKind::ErrorOk(pattern)
                | TypedPatternKind::ErrorErr(pattern) => visit_pattern(pattern, max_id),
                TypedPatternKind::EnumVariant { fields, .. } => {
                    for field in fields {
                        visit_pattern(field, max_id);
                    }
                }
                TypedPatternKind::Expr(pattern) => visit_expr(pattern, max_id),
                TypedPatternKind::Range { start, end, .. } => {
                    visit_expr(start, max_id);
                    visit_expr(end, max_id);
                }
                TypedPatternKind::Wildcard
                | TypedPatternKind::Bind { .. }
                | TypedPatternKind::OptionalNull
                | TypedPatternKind::CheckedInt { .. }
                | TypedPatternKind::CheckedIntRange { .. } => {}
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
                | TypedCallee::TraitAssociatedFunction { .. }
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
