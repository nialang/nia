// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn validate_function_lowering_input(
    body: &TypedBody,
    types: &FunctionTypeContext<'_>,
) -> Result<(), FunctionLoweringDiagnostic> {
    BodyInputValidator { types }.validate_effect_body(body)
}

struct BodyInputValidator<'context, 'store> {
    types: &'context FunctionTypeContext<'store>,
}

impl BodyInputValidator<'_, '_> {
    fn validate_effect_body(&self, body: &TypedBody) -> Result<(), FunctionLoweringDiagnostic> {
        for stmt in &body.stmts {
            self.validate_stmt(stmt)?;
        }
        if let Some(tail) = &body.tail {
            self.validate_tail_expr(tail)?;
        }
        Ok(())
    }

    fn validate_value_body(&self, body: &TypedBody) -> Result<(), FunctionLoweringDiagnostic> {
        for stmt in &body.stmts {
            self.validate_stmt(stmt)?;
        }
        if let Some(tail) = &body.tail {
            self.validate_tail_value_result(tail)?;
        }
        Ok(())
    }

    fn validate_stmt(&self, stmt: &TypedStmt) -> Result<(), FunctionLoweringDiagnostic> {
        match &stmt.kind {
            TypedStmtKind::Binding(binding) => {
                if let Some(value) = &binding.value {
                    self.validate_value_expr(value)?;
                }
            }
            TypedStmtKind::PatternBinding(binding) => {
                self.validate_pattern(&binding.pattern)?;
                self.validate_value_expr(&binding.value)?;
            }
            TypedStmtKind::Expr(expr) | TypedStmtKind::Defer(expr) => {
                self.validate_effect_expr(expr)?;
            }
            TypedStmtKind::Return(value) => {
                if let Some(value) = value {
                    self.validate_value_expr(value)?;
                }
            }
            TypedStmtKind::ForIn(for_stmt) => {
                // Keep validation in the same structural order as lowering: the
                // item pattern is consumed at loop entry and may itself contain
                // value expressions (including recovery nodes).
                self.validate_pattern(&for_stmt.pattern)?;
                self.validate_value_expr(&for_stmt.iter)?;
                self.validate_effect_body(&for_stmt.body)?;
            }
            TypedStmtKind::While(while_stmt) => {
                self.validate_value_expr(&while_stmt.cond)?;
                self.validate_effect_body(&while_stmt.body)?;
            }
            TypedStmtKind::Loop(loop_stmt) => self.validate_effect_body(&loop_stmt.body)?,
            TypedStmtKind::Break | TypedStmtKind::Continue => {}
        }
        Ok(())
    }

    fn validate_tail_expr(&self, expr: &TypedExpr) -> Result<(), FunctionLoweringDiagnostic> {
        if self.expr_is_effect_only(expr) {
            self.validate_effect_expr(expr)
        } else {
            self.validate_value_expr(expr)
        }
    }

    fn validate_tail_value_result(
        &self,
        expr: &TypedExpr,
    ) -> Result<(), FunctionLoweringDiagnostic> {
        if self.expr_is_terminating_effect(expr) {
            self.validate_effect_expr(expr)
        } else {
            self.validate_value_expr(expr)
        }
    }

    fn validate_effect_expr(&self, expr: &TypedExpr) -> Result<(), FunctionLoweringDiagnostic> {
        self.reject_error_expr(expr)?;
        match &expr.kind {
            TypedExprKind::Block(body) => self.validate_effect_body(body),
            TypedExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.validate_value_expr(cond)?;
                self.validate_effect_body(then_branch)?;
                if let Some(else_branch) = else_branch {
                    self.validate_effect_expr(else_branch)?;
                }
                Ok(())
            }
            TypedExprKind::IfPattern(if_pattern) => {
                self.validate_value_expr(&if_pattern.target)?;
                self.validate_pattern(&if_pattern.pattern)?;
                self.validate_effect_body(&if_pattern.then_branch)?;
                if let Some(else_branch) = &if_pattern.else_branch {
                    self.validate_effect_expr(else_branch)?;
                }
                Ok(())
            }
            TypedExprKind::Match(matched) => {
                self.validate_value_expr(&matched.target)?;
                for arm in &matched.arms {
                    for pattern in &arm.patterns {
                        self.validate_pattern(pattern)?;
                    }
                    self.validate_match_arm_effect_body(&arm.body)?;
                }
                Ok(())
            }
            TypedExprKind::Trap => Ok(()),
            TypedExprKind::InlineAsm(asm) => self.validate_inline_asm(asm),
            TypedExprKind::MemoryIntrinsic(memory) => self.validate_memory_intrinsic(memory),
            TypedExprKind::Atomic(atomic) if self.atomic_is_effect_only(atomic) => {
                self.validate_atomic(atomic)
            }
            TypedExprKind::Discard(inner) => self.validate_value_expr(inner),
            _ => self.validate_value_expr(expr),
        }
    }

    fn validate_value_expr(&self, expr: &TypedExpr) -> Result<(), FunctionLoweringDiagnostic> {
        self.reject_error_expr(expr)?;
        if self.expr_is_effect_only(expr) {
            return Err(FunctionLoweringDiagnostic {
                span: expr.span,
                message: format!(
                    "{} expression used where a value is required",
                    self.effect_only_expr_name(expr)
                ),
            });
        }
        match &expr.kind {
            TypedExprKind::Block(body) => self.validate_value_body(body),
            TypedExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.validate_value_expr(cond)?;
                self.validate_value_body(then_branch)?;
                if let Some(else_branch) = else_branch {
                    self.validate_tail_value_result(else_branch)?;
                }
                Ok(())
            }
            TypedExprKind::IfPattern(if_pattern) => {
                self.validate_value_expr(&if_pattern.target)?;
                self.validate_pattern(&if_pattern.pattern)?;
                self.validate_value_body(&if_pattern.then_branch)?;
                if let Some(else_branch) = &if_pattern.else_branch {
                    self.validate_tail_value_result(else_branch)?;
                }
                Ok(())
            }
            TypedExprKind::Match(matched) => {
                self.validate_value_expr(&matched.target)?;
                for arm in &matched.arms {
                    for pattern in &arm.patterns {
                        self.validate_pattern(pattern)?;
                    }
                    self.validate_match_arm_value_body(&arm.body)?;
                }
                Ok(())
            }
            TypedExprKind::Range(range) => {
                if let Some(start) = &range.start {
                    self.validate_value_expr(start)?;
                }
                if let Some(end) = &range.end {
                    self.validate_value_expr(end)?;
                }
                Ok(())
            }
            TypedExprKind::InlineAsm(asm) => self.validate_inline_asm(asm),
            TypedExprKind::Atomic(atomic) => self.validate_atomic(atomic),
            TypedExprKind::LoadUnaligned { ptr, .. } => self.validate_value_expr(ptr),
            TypedExprKind::Splat { value } => self.validate_value_expr(value),
            TypedExprKind::ExtractElement { vector, index } => {
                self.validate_value_expr(vector)?;
                self.validate_value_expr(index)
            }
            TypedExprKind::InsertElement {
                vector,
                index,
                value,
            } => {
                self.validate_value_expr(vector)?;
                self.validate_value_expr(index)?;
                self.validate_value_expr(value)
            }
            TypedExprKind::Bitmask { vector } => self.validate_value_expr(vector),
            TypedExprKind::BitIntrinsic { value, .. } => self.validate_value_expr(value),
            TypedExprKind::CharFromU32 { value } => self.validate_value_expr(value),
            TypedExprKind::StaticArrayPointer { array, .. } => self.validate_value_expr(array),
            TypedExprKind::ArrayLiteral { elems } => self.validate_array_elements(elems),
            TypedExprKind::Tuple(elems) => {
                for elem in elems {
                    self.validate_value_expr(elem)?;
                }
                Ok(())
            }
            TypedExprKind::Closure {
                closure_id,
                captures,
                params,
                body,
            } => {
                self.validate_closure_contract(expr, *closure_id, captures, params, body)?;
                for capture in captures {
                    self.validate_value_expr(&capture.value)?;
                }
                self.validate_value_body(body)
            }
            TypedExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    self.validate_value_expr(&field.value)?;
                }
                Ok(())
            }
            TypedExprKind::UnionLiteral { field, .. } => self.validate_value_expr(&field.value),
            TypedExprKind::Unary { expr, .. }
            | TypedExprKind::OptionalSome { expr }
            | TypedExprKind::ErrorOk { expr }
            | TypedExprKind::ErrorErr { expr }
            | TypedExprKind::Try { expr, .. }
            | TypedExprKind::Cast { expr, .. }
            | TypedExprKind::TraitObjectUpcast { expr, .. }
            | TypedExprKind::TraitObjectCoercion { expr, .. } => self.validate_value_expr(expr),
            TypedExprKind::CallableCoercion { state, .. } => self.validate_value_expr(state),
            TypedExprKind::Binary { lhs, rhs, .. } | TypedExprKind::Index { lhs, index: rhs } => {
                self.validate_value_expr(lhs)?;
                self.validate_value_expr(rhs)
            }
            TypedExprKind::Assign { place, rhs, .. } => {
                self.validate_place(place)?;
                self.validate_value_expr(rhs)
            }
            TypedExprKind::Call { callee, args } => {
                self.validate_callee(callee)?;
                for arg in args {
                    self.validate_value_expr(arg)?;
                }
                Ok(())
            }
            TypedExprKind::Field { lhs, .. } | TypedExprKind::TupleField { lhs, .. } => {
                self.validate_value_expr(lhs)
            }
            TypedExprKind::EnumVariant { fields, .. } => {
                for field in fields {
                    self.validate_value_expr(field)?;
                }
                Ok(())
            }
            TypedExprKind::Slice { lhs, range, .. } => {
                self.validate_value_expr(lhs)?;
                self.validate_slice_range(range)
            }
            TypedExprKind::Integer(_)
            | TypedExprKind::Float(_)
            | TypedExprKind::String(_)
            | TypedExprKind::ByteString(_)
            | TypedExprKind::Char(_)
            | TypedExprKind::ByteChar(_)
            | TypedExprKind::Bool(_)
            | TypedExprKind::Null
            | TypedExprKind::ConstGeneric(_)
            | TypedExprKind::Local(_)
            | TypedExprKind::Global(_)
            | TypedExprKind::Function(_)
            | TypedExprKind::FunctionInstance { .. }
            | TypedExprKind::ClosureFunctionPointer { .. }
            | TypedExprKind::BuiltinValue(_)
            | TypedExprKind::Trap
            | TypedExprKind::MemoryIntrinsic(_)
            | TypedExprKind::Discard(_)
            | TypedExprKind::Error => Ok(()),
            TypedExprKind::UnionStorageLiteral { relocations, .. } => {
                for relocation in relocations {
                    self.validate_value_expr(&relocation.pointee)?;
                }
                Ok(())
            }
        }
    }

    fn validate_match_arm_effect_body(
        &self,
        body: &TypedMatchArmBody,
    ) -> Result<(), FunctionLoweringDiagnostic> {
        match body {
            TypedMatchArmBody::Expr(expr) => self.validate_effect_expr(expr),
            TypedMatchArmBody::Stmt(stmt) => self.validate_stmt(stmt),
            TypedMatchArmBody::Block(body) => self.validate_effect_body(body),
        }
    }

    fn validate_match_arm_value_body(
        &self,
        body: &TypedMatchArmBody,
    ) -> Result<(), FunctionLoweringDiagnostic> {
        match body {
            TypedMatchArmBody::Expr(expr) => self.validate_tail_value_result(expr),
            TypedMatchArmBody::Stmt(stmt) => self.validate_stmt(stmt),
            TypedMatchArmBody::Block(body) => self.validate_value_body(body),
        }
    }

    fn validate_pattern(&self, pattern: &TypedPattern) -> Result<(), FunctionLoweringDiagnostic> {
        match &pattern.kind {
            TypedPatternKind::Pointer(inner)
            | TypedPatternKind::MutPointer(inner)
            | TypedPatternKind::OptionalSome(inner)
            | TypedPatternKind::ErrorOk(inner)
            | TypedPatternKind::ErrorErr(inner) => self.validate_pattern(inner),
            TypedPatternKind::Nominal {
                constructor,
                fields,
            } => {
                if let TypedNominalPatternConstructor::Struct { field_defs } = constructor
                    && field_defs.len() != fields.len()
                {
                    return Err(FunctionLoweringDiagnostic {
                        span: pattern.span,
                        message: "typed struct pattern field metadata is inconsistent".to_string(),
                    });
                }
                for field in fields {
                    self.validate_pattern(field)?;
                }
                Ok(())
            }
            TypedPatternKind::Tuple(patterns) => {
                for pattern in patterns {
                    self.validate_pattern(pattern)?;
                }
                Ok(())
            }
            TypedPatternKind::Expr(expr) => self.validate_value_expr(expr),
            TypedPatternKind::Range { start, end, .. } => {
                self.validate_value_expr(start)?;
                self.validate_value_expr(end)
            }
            TypedPatternKind::Wildcard
            | TypedPatternKind::Bind { .. }
            | TypedPatternKind::OptionalNull
            | TypedPatternKind::CheckedInt { .. }
            | TypedPatternKind::CheckedIntRange { .. } => Ok(()),
        }
    }

    fn validate_array_elements(
        &self,
        elems: &TypedArrayElements,
    ) -> Result<(), FunctionLoweringDiagnostic> {
        match elems {
            TypedArrayElements::List(elems) => {
                for elem in elems {
                    self.validate_value_expr(elem)?;
                }
                Ok(())
            }
            TypedArrayElements::Repeat { value, .. } => self.validate_value_expr(value),
        }
    }

    fn validate_inline_asm(&self, asm: &TypedInlineAsm) -> Result<(), FunctionLoweringDiagnostic> {
        for input in &asm.inputs {
            self.validate_value_expr(&input.value)?;
        }
        for output in &asm.outputs {
            self.validate_place(&output.place)?;
        }
        Ok(())
    }

    fn validate_memory_intrinsic(
        &self,
        memory: &nia_body_ir::TypedMemoryIntrinsic,
    ) -> Result<(), FunctionLoweringDiagnostic> {
        self.validate_value_expr(&memory.dest)?;
        match &memory.source {
            TypedMemoryIntrinsicSource::Slice(source)
            | TypedMemoryIntrinsicSource::Byte(source) => self.validate_value_expr(source),
        }
    }

    fn validate_atomic(&self, atomic: &TypedAtomic) -> Result<(), FunctionLoweringDiagnostic> {
        match atomic {
            TypedAtomic::Load { ptr, .. } => self.validate_value_expr(ptr),
            TypedAtomic::Store { ptr, value, .. } | TypedAtomic::Rmw { ptr, value, .. } => {
                self.validate_value_expr(ptr)?;
                self.validate_value_expr(value)
            }
            TypedAtomic::Cmpxchg {
                ptr,
                expected,
                desired,
                ..
            } => {
                self.validate_value_expr(ptr)?;
                self.validate_value_expr(expected)?;
                self.validate_value_expr(desired)
            }
            TypedAtomic::Fence { .. } => Ok(()),
        }
    }

    fn validate_callee(&self, callee: &TypedCallee) -> Result<(), FunctionLoweringDiagnostic> {
        match callee {
            TypedCallee::Closure(callee) => {
                self.validate_value_expr(callee)?;
                if !matches!(self.types.get(callee.ty), Some(TyKind::ClosureState { .. })) {
                    return Err(FunctionLoweringDiagnostic {
                        span: callee.span,
                        message: "closure callee does not have a closure-state type".to_string(),
                    });
                }
                Ok(())
            }
            TypedCallee::Method { receiver, .. }
            | TypedCallee::TraitMethod { receiver, .. }
            | TypedCallee::DynamicTraitMethod { receiver, .. }
            | TypedCallee::BuiltinMethod { receiver, .. }
            | TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod { receiver, .. })
            | TypedCallee::Callable(receiver)
            | TypedCallee::FunctionPointer(receiver) => self.validate_value_expr(receiver),
            TypedCallee::Function(_)
            | TypedCallee::FunctionInstance { .. }
            | TypedCallee::TraitAssociatedFunction { .. }
            | TypedCallee::BuiltinOperator(_) => Ok(()),
        }
    }

    fn validate_closure_contract(
        &self,
        expr: &TypedExpr,
        closure_id: ClosureId,
        captures: &[nia_body_ir::TypedClosureCapture],
        params: &[LocalId],
        body: &TypedBody,
    ) -> Result<(), FunctionLoweringDiagnostic> {
        // This is the ABI shape consumed by `ensure_closure_entry`, not a
        // second type-checking pass. Validate every field that lowering zips
        // or maps by position so malformed recovery products cannot turn its
        // internal assertions into a process panic.
        let Some(TyKind::ClosureState {
            closure_id: type_closure_id,
            captures: capture_types,
            params: param_types,
            return_type,
        }) = self.types.get(expr.ty)
        else {
            return Err(FunctionLoweringDiagnostic {
                span: expr.span,
                message: "closure expression does not have a closure-state type".to_string(),
            });
        };
        if *type_closure_id != closure_id {
            return Err(FunctionLoweringDiagnostic {
                span: expr.span,
                message: "closure expression identity does not match its closure-state type"
                    .to_string(),
            });
        }
        if capture_types.len() != captures.len() {
            return Err(FunctionLoweringDiagnostic {
                span: expr.span,
                message: "closure capture count does not match its closure-state type".to_string(),
            });
        }
        if param_types.len() != params.len() {
            return Err(FunctionLoweringDiagnostic {
                span: expr.span,
                message: "closure parameter count does not match its closure-state type"
                    .to_string(),
            });
        }
        if *return_type != body.ty {
            return Err(FunctionLoweringDiagnostic {
                span: body.span,
                message: "closure body type does not match its closure-state return type"
                    .to_string(),
            });
        }
        let mut capture_locals = std::collections::HashSet::with_capacity(captures.len());
        for (capture, expected) in captures.iter().zip(capture_types) {
            if !capture_locals.insert(capture.local_id) {
                return Err(FunctionLoweringDiagnostic {
                    span: expr.span,
                    message: "closure capture locals must be unique".to_string(),
                });
            }
            if capture.value.ty != *expected {
                return Err(FunctionLoweringDiagnostic {
                    span: capture.value.span,
                    message: "closure capture type does not match its closure-state field"
                        .to_string(),
                });
            }
        }
        let mut param_locals = std::collections::HashSet::with_capacity(params.len());
        for (param, expected) in params.iter().zip(param_types) {
            if !param_locals.insert(*param) || capture_locals.contains(param) {
                return Err(FunctionLoweringDiagnostic {
                    span: body.span,
                    message: "closure capture and parameter locals must be distinct".to_string(),
                });
            }
            if body
                .locals
                .iter()
                .find(|local| local.id == *param)
                .is_none_or(|local| local.ty != *expected)
            {
                return Err(FunctionLoweringDiagnostic {
                    span: body.span,
                    message: "closure parameter local does not match its closure-state signature"
                        .to_string(),
                });
            }
        }
        Ok(())
    }

    fn validate_place(&self, place: &TypedPlace) -> Result<(), FunctionLoweringDiagnostic> {
        match &place.base {
            PlaceBase::Deref(expr) => self.validate_value_expr(expr)?,
            PlaceBase::Error => {
                return Err(FunctionLoweringDiagnostic {
                    span: place.span,
                    message: "error place escaped into function lowering input".to_string(),
                });
            }
            PlaceBase::Local(_) | PlaceBase::Global(_) => {}
        }
        for elem in &place.elems {
            match elem {
                PlaceElem::Index(index) => self.validate_value_expr(index)?,
                PlaceElem::Error => {
                    return Err(FunctionLoweringDiagnostic {
                        span: place.span,
                        message: "error place element escaped into function lowering input"
                            .to_string(),
                    });
                }
                PlaceElem::Field(_) | PlaceElem::TupleField(_) => {}
            }
        }
        Ok(())
    }

    fn validate_slice_range(
        &self,
        range: &TypedSliceRange,
    ) -> Result<(), FunctionLoweringDiagnostic> {
        if let Some(start) = &range.start {
            self.validate_value_expr(start)?;
        }
        if let Some(end) = &range.end {
            self.validate_value_expr(end)?;
        }
        Ok(())
    }

    fn reject_error_expr(&self, expr: &TypedExpr) -> Result<(), FunctionLoweringDiagnostic> {
        if matches!(expr.kind, TypedExprKind::Error) {
            return Err(FunctionLoweringDiagnostic {
                span: expr.span,
                message: "error expression escaped into function lowering input".to_string(),
            });
        }
        Ok(())
    }

    fn expr_is_effect_only(&self, expr: &TypedExpr) -> bool {
        matches!(
            expr.kind,
            TypedExprKind::Trap
                | TypedExprKind::InlineAsm(_)
                | TypedExprKind::MemoryIntrinsic(_)
                | TypedExprKind::Discard(_)
        ) || matches!(
            &expr.kind,
            TypedExprKind::Atomic(atomic) if self.atomic_is_effect_only(atomic)
        )
    }

    fn expr_is_terminating_effect(&self, expr: &TypedExpr) -> bool {
        matches!(expr.kind, TypedExprKind::Trap)
    }

    fn effect_only_expr_name(&self, expr: &TypedExpr) -> &'static str {
        match &expr.kind {
            TypedExprKind::Trap => "trap",
            TypedExprKind::InlineAsm(_) => "inline asm",
            TypedExprKind::MemoryIntrinsic(_) => "memory intrinsic",
            TypedExprKind::Discard(_) => "discard",
            TypedExprKind::Atomic(atomic) if self.atomic_is_effect_only(atomic) => match atomic {
                TypedAtomic::Store { .. } => "atomic store",
                TypedAtomic::Fence { .. } => "atomic fence",
                _ => "atomic",
            },
            _ => "effect-only",
        }
    }

    fn atomic_is_effect_only(&self, atomic: &TypedAtomic) -> bool {
        matches!(
            atomic,
            TypedAtomic::Store { .. } | TypedAtomic::Fence { .. }
        )
    }
}
