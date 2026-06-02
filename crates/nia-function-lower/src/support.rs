// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

impl FunctionLowerer {
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
            _ => FunctionPlace {
                span: expr.span,
                ty: expr.ty,
                base: FunctionPlaceBase::Error,
                elems: Vec::new(),
            },
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
        FunctionBinding {
            local_id: binding.local_id,
            name: binding.name.clone(),
            ty: binding.ty,
            value: binding
                .value
                .as_ref()
                .map(|value| self.lower_value_expr(value, scope, current, ops, blocks)),
            is_const: binding.is_const,
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
                        *max_id = (*max_id).max(for_stmt.local_id.0.saturating_add(1));
                        visit_for_iterator(&for_stmt.iter, max_id);
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
                        if let TypedSwitchPattern::Expr(pattern) = &arm.pattern {
                            visit_expr(pattern, max_id);
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
                                    *max_id = (*max_id).max(for_stmt.local_id.0.saturating_add(1));
                                    visit_for_iterator(&for_stmt.iter, max_id);
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
                TypedExprKind::Error
                | TypedExprKind::Integer(_)
                | TypedExprKind::Float(_)
                | TypedExprKind::String(_)
                | TypedExprKind::ByteString(_)
                | TypedExprKind::Char(_)
                | TypedExprKind::ByteChar(_)
                | TypedExprKind::Bool(_)
                | TypedExprKind::Global(_)
                | TypedExprKind::Function(_)
                | TypedExprKind::FunctionInstance { .. }
                | TypedExprKind::EnumVariant(_)
                | TypedExprKind::BuiltinValue(_) => {}
            }
        }

        pub(super) fn visit_for_iterator(iter: &TypedForIterator, max_id: &mut u32) {
            match iter {
                TypedForIterator::Range(range) => {
                    visit_expr(&range.expr, max_id);
                }
                TypedForIterator::Expr(expr) => visit_expr(expr, max_id),
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
