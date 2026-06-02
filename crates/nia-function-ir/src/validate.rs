// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

use nia_ids::LocalId;
use nia_span::Span;

use crate::{
    FunctionArrayElements, FunctionBinding, FunctionBlock, FunctionBlockId, FunctionBody,
    FunctionCallee, FunctionDeferBody, FunctionExpr, FunctionExprKind, FunctionForHeader,
    FunctionLocal, FunctionOp, FunctionPlace, FunctionPlaceBase, FunctionPlaceElem, FunctionScope,
    FunctionScopeId, FunctionTerminator,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionIrError {
    pub span: Span,
    pub message: String,
}

impl FunctionIrError {
    fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }
}

pub fn validate_function_body(body: &FunctionBody) -> Result<(), FunctionIrError> {
    FunctionIrValidator::new(&body.locals, &body.scopes, &body.blocks, body.entry)
        .validate_body(body.span)?;
    for block in &body.blocks {
        for op in &block.ops {
            if let FunctionOp::Defer(defer_body) = op {
                validate_function_defer_body(&body.locals, defer_body)?;
            }
        }
    }
    Ok(())
}

pub fn validate_function_defer_body(
    enclosing_locals: &[FunctionLocal],
    body: &FunctionDeferBody,
) -> Result<(), FunctionIrError> {
    // Defer bodies execute in the enclosing function's local namespace: captures are plain
    // local references, while control-flow scopes are private to this deferred mini-body.
    // Keeping that split explicit here prevents codegen-only failures when nested defer
    // lowering changes either side of the representation.
    FunctionIrValidator::new(enclosing_locals, &body.scopes, &body.blocks, body.entry)
        .validate_defer_body(body.span)?;
    for block in &body.blocks {
        for op in &block.ops {
            if let FunctionOp::Defer(defer_body) = op {
                validate_function_defer_body(enclosing_locals, defer_body)?;
            }
        }
    }
    Ok(())
}

struct FunctionIrValidator<'a> {
    locals: &'a [FunctionLocal],
    scopes: &'a [FunctionScope],
    blocks: &'a [FunctionBlock],
    entry: FunctionBlockId,
    local_ids: HashSet<LocalId>,
    scope_ids: HashSet<FunctionScopeId>,
    block_ids: HashSet<FunctionBlockId>,
}

impl<'a> FunctionIrValidator<'a> {
    fn new(
        locals: &'a [FunctionLocal],
        scopes: &'a [FunctionScope],
        blocks: &'a [FunctionBlock],
        entry: FunctionBlockId,
    ) -> Self {
        Self {
            locals,
            scopes,
            blocks,
            entry,
            local_ids: locals.iter().map(|local| local.id).collect(),
            scope_ids: scopes.iter().map(|scope| scope.id).collect(),
            block_ids: blocks.iter().map(|block| block.id).collect(),
        }
    }

    fn validate_body(&self, span: Span) -> Result<(), FunctionIrError> {
        self.validate_body_shape(span)?;
        for block in self.blocks {
            for op in &block.ops {
                self.validate_op(op)?;
            }
            self.validate_terminator(&block.terminator)?;
        }
        Ok(())
    }

    fn validate_defer_body(&self, span: Span) -> Result<(), FunctionIrError> {
        self.validate_body_shape(span)?;
        for block in self.blocks {
            for op in &block.ops {
                self.validate_op(op)?;
            }
            self.validate_defer_terminator(&block.terminator)?;
        }
        Ok(())
    }

    fn validate_body_shape(&self, span: Span) -> Result<(), FunctionIrError> {
        self.validate_unique_locals()?;
        self.validate_unique_scopes()?;
        self.validate_unique_blocks()?;
        self.require_block(self.entry, span, "function entry block")?;
        for scope in self.scopes {
            if let Some(parent) = scope.parent {
                self.require_scope(parent, scope.span, "scope parent")?;
                if parent == scope.id {
                    return Err(FunctionIrError::new(
                        scope.span,
                        "scope cannot parent itself",
                    ));
                }
            }
            self.validate_scope_parent_chain(scope)?;
        }
        for block in self.blocks {
            self.require_scope(block.scope, block.span, "block scope")?;
        }
        Ok(())
    }

    fn validate_scope_parent_chain(&self, scope: &FunctionScope) -> Result<(), FunctionIrError> {
        let mut seen = HashSet::new();
        let mut current = Some(scope.id);
        while let Some(scope_id) = current {
            if !seen.insert(scope_id) {
                return Err(FunctionIrError::new(
                    scope.span,
                    "scope parent chain contains a cycle",
                ));
            }
            let Some(scope) = self.scopes.iter().find(|scope| scope.id == scope_id) else {
                return Ok(());
            };
            current = scope.parent;
        }
        Ok(())
    }

    fn validate_unique_locals(&self) -> Result<(), FunctionIrError> {
        let mut seen = HashSet::new();
        for local in self.locals {
            if !seen.insert(local.id) {
                return Err(FunctionIrError::new(
                    local.span,
                    "duplicate function local id",
                ));
            }
        }
        Ok(())
    }

    fn validate_unique_scopes(&self) -> Result<(), FunctionIrError> {
        let mut seen = HashSet::new();
        for scope in self.scopes {
            if !seen.insert(scope.id) {
                return Err(FunctionIrError::new(
                    scope.span,
                    "duplicate function scope id",
                ));
            }
        }
        Ok(())
    }

    fn validate_unique_blocks(&self) -> Result<(), FunctionIrError> {
        let mut seen = HashSet::new();
        for block in self.blocks {
            if !seen.insert(block.id) {
                return Err(FunctionIrError::new(
                    block.span,
                    "duplicate function block id",
                ));
            }
        }
        Ok(())
    }

    fn validate_op(&self, op: &FunctionOp) -> Result<(), FunctionIrError> {
        match op {
            FunctionOp::Binding(binding) => {
                self.require_local(binding.local_id, binding.value_span(), "binding local")?;
                if let Some(value) = &binding.value {
                    self.validate_expr(value)?;
                }
            }
            FunctionOp::StoreLocal {
                local_id,
                value,
                span,
            } => {
                self.require_local(*local_id, *span, "store local")?;
                self.validate_expr(value)?;
            }
            FunctionOp::Expr(expr) => self.validate_expr(expr)?,
            FunctionOp::Defer(_) => {}
        }
        Ok(())
    }

    fn validate_terminator(&self, terminator: &FunctionTerminator) -> Result<(), FunctionIrError> {
        for successor in terminator.successors() {
            self.require_block(successor, terminator.span(), "terminator successor")?;
        }
        match terminator {
            FunctionTerminator::If { cond, .. } => self.validate_expr(cond)?,
            FunctionTerminator::Switch { target, arms, .. } => {
                self.validate_expr(target)?;
                for arm in arms {
                    self.validate_expr(&arm.pattern)?;
                }
            }
            FunctionTerminator::Loop { header, .. } => self.validate_for_header(header)?,
            FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
                if let Some(value) = value {
                    self.validate_expr(value)?;
                }
            }
            FunctionTerminator::Error { .. }
            | FunctionTerminator::Branch { .. }
            | FunctionTerminator::Next { .. } => {}
        }
        Ok(())
    }

    fn validate_defer_terminator(
        &self,
        terminator: &FunctionTerminator,
    ) -> Result<(), FunctionIrError> {
        match terminator {
            FunctionTerminator::Return { span, .. } => {
                return Err(FunctionIrError::new(
                    *span,
                    "`return` is not valid in defer function IR",
                ));
            }
            _ => self.validate_terminator(terminator)?,
        }
        Ok(())
    }

    fn validate_for_header(&self, header: &FunctionForHeader) -> Result<(), FunctionIrError> {
        match header {
            FunctionForHeader::Infinite => Ok(()),
            FunctionForHeader::Condition(cond) => self.validate_expr(cond),
        }
    }

    fn validate_expr(&self, expr: &FunctionExpr) -> Result<(), FunctionIrError> {
        match &expr.kind {
            FunctionExprKind::Local(local_id) => {
                self.require_local(*local_id, expr.span, "local expression")?
            }
            FunctionExprKind::CStringPointer { array: inner, .. }
            | FunctionExprKind::Unary { expr: inner, .. }
            | FunctionExprKind::Discard(inner)
            | FunctionExprKind::Cast { expr: inner, .. }
            | FunctionExprKind::TraitObjectUpcast { expr: inner, .. }
            | FunctionExprKind::TraitObjectCoercion { expr: inner, .. } => {
                self.validate_expr(inner)?
            }
            FunctionExprKind::Range(range) => {
                if let Some(start) = &range.start {
                    self.validate_expr(start)?;
                }
                if let Some(end) = &range.end {
                    self.validate_expr(end)?;
                }
            }
            FunctionExprKind::AddrOf(place) => self.validate_place(place)?,
            FunctionExprKind::ArrayLiteral { elems } => match elems {
                FunctionArrayElements::List(elems) => {
                    for elem in elems {
                        self.validate_expr(elem)?;
                    }
                }
                FunctionArrayElements::Repeat { value, .. } => self.validate_expr(value)?,
            },
            FunctionExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    self.validate_expr(&field.value)?;
                }
            }
            FunctionExprKind::UnionLiteral { field, .. } => self.validate_expr(&field.value)?,
            FunctionExprKind::Binary { lhs, rhs, .. }
            | FunctionExprKind::Index { lhs, index: rhs } => {
                self.validate_expr(lhs)?;
                self.validate_expr(rhs)?;
            }
            FunctionExprKind::Assign { place, rhs, .. } => {
                self.validate_place(place)?;
                self.validate_expr(rhs)?;
            }
            FunctionExprKind::Call { callee, args } => {
                self.validate_callee(callee)?;
                for arg in args {
                    self.validate_expr(arg)?;
                }
            }
            FunctionExprKind::Field { lhs, .. } => self.validate_expr(lhs)?,
            FunctionExprKind::Slice { lhs, range, .. } => {
                self.validate_expr(lhs)?;
                if let Some(start) = &range.start {
                    self.validate_expr(start)?;
                }
                if let Some(end) = &range.end {
                    self.validate_expr(end)?;
                }
            }
            FunctionExprKind::InlineAsm(asm) => {
                for input in &asm.inputs {
                    self.validate_expr(&input.value)?;
                }
                for output in &asm.outputs {
                    self.validate_place(&output.place)?;
                }
            }
            FunctionExprKind::Error
            | FunctionExprKind::Integer(_)
            | FunctionExprKind::Float(_)
            | FunctionExprKind::String(_)
            | FunctionExprKind::ByteString(_)
            | FunctionExprKind::Char(_)
            | FunctionExprKind::ByteChar(_)
            | FunctionExprKind::Bool(_)
            | FunctionExprKind::Global(_)
            | FunctionExprKind::Function(_)
            | FunctionExprKind::FunctionInstance { .. }
            | FunctionExprKind::EnumVariant(_)
            | FunctionExprKind::BuiltinValue(_) => {}
        }
        Ok(())
    }

    fn validate_callee(&self, callee: &FunctionCallee) -> Result<(), FunctionIrError> {
        match callee {
            FunctionCallee::Method { receiver, .. }
            | FunctionCallee::TraitMethod { receiver, .. }
            | FunctionCallee::BuiltinMethod { receiver, .. }
            | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
            | FunctionCallee::DynamicTraitMethod { receiver, .. }
            | FunctionCallee::FunctionPointer(receiver) => self.validate_expr(receiver),
            FunctionCallee::Function(_)
            | FunctionCallee::FunctionInstance { .. }
            | FunctionCallee::BuiltinOperator(_) => Ok(()),
        }
    }

    fn validate_place(&self, place: &FunctionPlace) -> Result<(), FunctionIrError> {
        match &place.base {
            FunctionPlaceBase::Local(local_id) => {
                self.require_local(*local_id, place.span, "place local")?
            }
            FunctionPlaceBase::Deref(expr) => self.validate_expr(expr)?,
            FunctionPlaceBase::Global(_) | FunctionPlaceBase::Error => {}
        }
        for elem in &place.elems {
            match elem {
                FunctionPlaceElem::Index(index) => self.validate_expr(index)?,
                FunctionPlaceElem::Field(_) | FunctionPlaceElem::Error => {}
            }
        }
        Ok(())
    }

    fn require_local(
        &self,
        local_id: LocalId,
        span: Span,
        what: &str,
    ) -> Result<(), FunctionIrError> {
        if self.local_ids.contains(&local_id) {
            Ok(())
        } else {
            Err(FunctionIrError::new(
                span,
                format!("{what} references missing local `{}`", local_id.0),
            ))
        }
    }

    fn require_scope(
        &self,
        scope: FunctionScopeId,
        span: Span,
        what: &str,
    ) -> Result<(), FunctionIrError> {
        if self.scope_ids.contains(&scope) {
            Ok(())
        } else {
            Err(FunctionIrError::new(
                span,
                format!("{what} references missing scope `{}`", scope.0),
            ))
        }
    }

    fn require_block(
        &self,
        block: FunctionBlockId,
        span: Span,
        what: &str,
    ) -> Result<(), FunctionIrError> {
        if self.block_ids.contains(&block) {
            Ok(())
        } else {
            Err(FunctionIrError::new(
                span,
                format!("{what} references missing block `{}`", block.0),
            ))
        }
    }
}

impl FunctionBinding {
    fn value_span(&self) -> Span {
        self.value
            .as_ref()
            .map(|value| value.span)
            .unwrap_or_default()
    }
}

impl FunctionTerminator {
    fn span(&self) -> Span {
        match self {
            FunctionTerminator::Error { span }
            | FunctionTerminator::Branch { span, .. }
            | FunctionTerminator::Next { span, .. }
            | FunctionTerminator::If { span, .. }
            | FunctionTerminator::Switch { span, .. }
            | FunctionTerminator::Loop { span, .. }
            | FunctionTerminator::Return { span, .. }
            | FunctionTerminator::Tail { span, .. } => *span,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_body(scopes: Vec<FunctionScope>) -> FunctionBody {
        let span = Span::default();
        let interner = nia_ty::TyInterner::new(nia_ids::ModuleId(0));
        let ty = interner.primitive(nia_ty::PrimitiveTy::Void);
        FunctionBody {
            span,
            locals: Vec::new(),
            scopes,
            blocks: vec![FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Tail { value: None, span },
            }],
            entry: FunctionBlockId(0),
            ty,
        }
    }

    #[test]
    fn rejects_scope_parent_cycles() {
        let body = empty_body(vec![
            FunctionScope {
                id: FunctionScopeId(0),
                parent: Some(FunctionScopeId(1)),
                span: Span::default(),
            },
            FunctionScope {
                id: FunctionScopeId(1),
                parent: Some(FunctionScopeId(0)),
                span: Span::default(),
            },
        ]);

        let error = validate_function_body(&body).expect_err("scope cycle should fail");

        assert!(
            error
                .message
                .contains("scope parent chain contains a cycle"),
            "{error:?}"
        );
    }

    #[test]
    fn rejects_return_in_defer_body() {
        let span = Span::default();
        let defer_body = FunctionDeferBody {
            span,
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span,
            }],
            blocks: vec![FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            }],
            entry: FunctionBlockId(0),
        };

        let error =
            validate_function_defer_body(&[], &defer_body).expect_err("defer return should fail");

        assert!(
            error.message.contains("not valid in defer function IR"),
            "{error:?}"
        );
    }
}
