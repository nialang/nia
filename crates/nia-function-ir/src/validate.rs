// SPDX-License-Identifier: GPL-3.0-or-later
//! Structural validation for function IR producer/consumer boundaries.
//!
//! Validation rejects recovery nodes, dangling ids, malformed scope graphs,
//! effect-only expressions in value positions, and invalid closure ABI local
//! metadata.
//! Nominal type and definition lookup remains the backend validator's job,
//! because this crate intentionally has no program/type-store dependency.

use std::collections::HashSet;

use nia_ids::LocalId;
use nia_span::Span;

use crate::{
    FunctionArrayElements, FunctionAtomic, FunctionBinding, FunctionBlock, FunctionBlockId,
    FunctionBody, FunctionCallee, FunctionClosureEntry, FunctionDeferBody, FunctionExpr,
    FunctionExprKind, FunctionForHeader, FunctionLocal, FunctionLocalKind,
    FunctionMemoryIntrinsicSource, FunctionOp, FunctionPlace, FunctionPlaceBase, FunctionPlaceElem,
    FunctionScope, FunctionScopeId, FunctionTerminator,
};

#[derive(Debug, Clone, PartialEq, Eq)]
/// A structural function-IR validation failure with its source span.
pub struct FunctionIrError {
    /// Narrowest source span available for the rejected invariant.
    pub span: Span,
    /// Stable human-readable invariant description.
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

/// Validates a complete function body, including recursively nested defers.
pub fn validate_function_body(body: &FunctionBody) -> Result<(), FunctionIrError> {
    FunctionIrValidator::new(&body.locals, &body.scopes, &body.blocks, body.entry)
        .validate_body(body.span)?;
    let body_block_ids = body
        .blocks
        .iter()
        .map(|block| block.id)
        .collect::<HashSet<_>>();
    for block in &body.blocks {
        for op in &block.ops {
            if let FunctionOp::Defer(defer_body) = op {
                validate_function_defer_body_with_outer_blocks(
                    &body.locals,
                    defer_body,
                    &body_block_ids,
                )?;
            }
        }
    }
    Ok(())
}

/// Validates a generated closure entry body and its ABI-facing local metadata.
///
/// Pointer shape and nominal closure-state identity require a [`nia_ty::TypeStore`]
/// and remain backend validation responsibilities. This check covers the
/// self-contained invariants required to keep backend lowering from indexing
/// missing or contradictory locals.
pub fn validate_function_closure_entry(
    entry: &FunctionClosureEntry,
) -> Result<(), FunctionIrError> {
    validate_function_body(&entry.body)?;
    if entry.body.ty != entry.return_type {
        return Err(FunctionIrError::new(
            entry.body.span,
            "closure entry body type does not match its declared return type",
        ));
    }
    let state = require_closure_param_local(&entry.body, entry.state_param, "state parameter")?;
    if state.kind != FunctionLocalKind::Param {
        return Err(FunctionIrError::new(
            state.span,
            "closure entry state local is not a parameter",
        ));
    }

    let mut seen = HashSet::from([entry.state_param]);
    for param in &entry.params {
        if !seen.insert(*param) {
            return Err(FunctionIrError::new(
                entry.body.span,
                "closure entry parameter list contains a duplicate local",
            ));
        }
        let local = require_closure_param_local(&entry.body, *param, "parameter")?;
        if local.kind != FunctionLocalKind::Param {
            return Err(FunctionIrError::new(
                local.span,
                "closure entry parameter local is not a parameter",
            ));
        }
    }
    if let Some(unmapped) = entry
        .body
        .locals
        .iter()
        .find(|local| local.kind == FunctionLocalKind::Param && !seen.contains(&local.id))
    {
        return Err(FunctionIrError::new(
            unmapped.span,
            format!(
                "closure entry body contains unmapped parameter local `{}`",
                unmapped.id.0
            ),
        ));
    }
    Ok(())
}

fn require_closure_param_local<'a>(
    body: &'a FunctionBody,
    id: LocalId,
    what: &str,
) -> Result<&'a FunctionLocal, FunctionIrError> {
    body.locals
        .iter()
        .find(|local| local.id == id)
        .ok_or_else(|| {
            FunctionIrError::new(
                body.span,
                format!("closure entry {what} references missing local `{}`", id.0),
            )
        })
}

/// Validates a standalone defer mini-CFG against its enclosing local table.
///
/// Standalone validation has no outer block namespace. Nested defer bodies in
/// a full function are validated by [`validate_function_body`], which supplies
/// the enclosing block ids needed by already-lowered non-local exits.
pub fn validate_function_defer_body(
    enclosing_locals: &[FunctionLocal],
    body: &FunctionDeferBody,
) -> Result<(), FunctionIrError> {
    validate_function_defer_body_with_outer_blocks(enclosing_locals, body, &HashSet::new())
}

fn validate_function_defer_body_with_outer_blocks(
    enclosing_locals: &[FunctionLocal],
    body: &FunctionDeferBody,
    outer_block_ids: &HashSet<FunctionBlockId>,
) -> Result<(), FunctionIrError> {
    // Defer bodies execute in the enclosing function's local namespace: captures are plain
    // local references, while control-flow scopes are private to this deferred mini-body.
    // Keeping that split explicit here prevents codegen-only failures when nested defer
    // lowering changes either side of the representation.
    FunctionIrValidator::new(enclosing_locals, &body.scopes, &body.blocks, body.entry)
        .validate_defer_body(body.span, outer_block_ids)?;
    let mut nested_outer_block_ids = outer_block_ids.clone();
    nested_outer_block_ids.extend(body.blocks.iter().map(|block| block.id));
    for block in &body.blocks {
        for op in &block.ops {
            if let FunctionOp::Defer(defer_body) = op {
                validate_function_defer_body_with_outer_blocks(
                    enclosing_locals,
                    defer_body,
                    &nested_outer_block_ids,
                )?;
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

    fn validate_defer_body(
        &self,
        span: Span,
        outer_block_ids: &HashSet<FunctionBlockId>,
    ) -> Result<(), FunctionIrError> {
        self.validate_body_shape(span)?;
        for block in self.blocks {
            for op in &block.ops {
                self.validate_op(op)?;
            }
            self.validate_defer_terminator(&block.terminator, outer_block_ids)?;
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
                    self.validate_value_expr(value)?;
                }
            }
            FunctionOp::StoreLocal {
                local_id,
                value,
                span,
            } => {
                self.require_local(*local_id, *span, "store local")?;
                self.validate_value_expr(value)?;
            }
            FunctionOp::MemoryIntrinsic(memory) => {
                self.validate_value_expr(&memory.dest)?;
                match &memory.source {
                    FunctionMemoryIntrinsicSource::Slice(source)
                    | FunctionMemoryIntrinsicSource::Byte(source) => {
                        self.validate_value_expr(source)?
                    }
                }
            }
            FunctionOp::Expr(expr) => self.validate_expr(expr)?,
            FunctionOp::Defer(_) => {}
        }
        Ok(())
    }

    fn validate_terminator(&self, terminator: &FunctionTerminator) -> Result<(), FunctionIrError> {
        for target in terminator.referenced_blocks() {
            self.require_block(target, terminator.span(), "terminator")?;
        }
        match terminator {
            FunctionTerminator::If { cond, .. } => self.validate_value_expr(cond)?,
            FunctionTerminator::Switch { target, arms, .. } => {
                self.validate_value_expr(target)?;
                for arm in arms {
                    self.validate_value_expr(&arm.pattern)?;
                }
            }
            FunctionTerminator::Try {
                value,
                kind,
                error_conversion,
                success_local,
                ..
            } => {
                self.validate_value_expr(value)?;
                if matches!(kind, crate::FunctionTryKind::Optional) && error_conversion.is_some() {
                    return Err(FunctionIrError::new(
                        value.span,
                        "optional propagation cannot carry an error conversion",
                    ));
                }
                if let Some(conversion) = error_conversion {
                    self.validate_value_expr(conversion)?;
                }
                self.require_local(*success_local, value.span, "try success local")?;
            }
            FunctionTerminator::Loop { header, .. } => self.validate_for_header(header)?,
            FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
                if let Some(value) = value {
                    self.validate_value_expr(value)?;
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
        outer_block_ids: &HashSet<FunctionBlockId>,
    ) -> Result<(), FunctionIrError> {
        for target in terminator.referenced_blocks() {
            if !self.block_ids.contains(&target) && !outer_block_ids.contains(&target) {
                return Err(FunctionIrError::new(
                    terminator.span(),
                    format!("terminator references missing block `{}`", target.0),
                ));
            }
        }
        self.validate_terminator_payload(terminator)?;
        Ok(())
    }

    fn validate_terminator_payload(
        &self,
        terminator: &FunctionTerminator,
    ) -> Result<(), FunctionIrError> {
        match terminator {
            FunctionTerminator::If { cond, .. } => self.validate_value_expr(cond)?,
            FunctionTerminator::Switch { target, arms, .. } => {
                self.validate_value_expr(target)?;
                for arm in arms {
                    self.validate_value_expr(&arm.pattern)?;
                }
            }
            FunctionTerminator::Try {
                value,
                kind,
                error_conversion,
                success_local,
                ..
            } => {
                self.validate_value_expr(value)?;
                if matches!(kind, crate::FunctionTryKind::Optional) && error_conversion.is_some() {
                    return Err(FunctionIrError::new(
                        value.span,
                        "optional propagation cannot carry an error conversion",
                    ));
                }
                if let Some(conversion) = error_conversion {
                    self.validate_value_expr(conversion)?;
                }
                self.require_local(*success_local, value.span, "try success local")?;
            }
            FunctionTerminator::Loop { header, .. } => self.validate_for_header(header)?,
            FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
                if let Some(value) = value {
                    self.validate_value_expr(value)?;
                }
            }
            FunctionTerminator::Error { .. }
            | FunctionTerminator::Branch { .. }
            | FunctionTerminator::Next { .. } => {}
        }
        Ok(())
    }

    fn validate_for_header(&self, header: &FunctionForHeader) -> Result<(), FunctionIrError> {
        match header {
            FunctionForHeader::Infinite => Ok(()),
            FunctionForHeader::Condition(cond) => self.validate_value_expr(cond),
        }
    }

    fn validate_value_expr(&self, expr: &FunctionExpr) -> Result<(), FunctionIrError> {
        if Self::is_effect_only_expr(expr) {
            return Err(FunctionIrError::new(
                expr.span,
                "effect-only expression used where a value is required",
            ));
        }
        self.validate_expr(expr)
    }

    fn is_effect_only_expr(expr: &FunctionExpr) -> bool {
        matches!(
            expr.kind,
            FunctionExprKind::Trap | FunctionExprKind::InlineAsm(_) | FunctionExprKind::Discard(_)
        ) || matches!(
            &expr.kind,
            FunctionExprKind::Atomic(atomic) if Self::atomic_is_effect_only(atomic)
        )
    }

    fn atomic_is_effect_only(atomic: &FunctionAtomic) -> bool {
        matches!(
            atomic,
            FunctionAtomic::Store { .. } | FunctionAtomic::Fence { .. }
        )
    }

    fn validate_expr(&self, expr: &FunctionExpr) -> Result<(), FunctionIrError> {
        match &expr.kind {
            FunctionExprKind::Local(local_id) => {
                self.require_local(*local_id, expr.span, "local expression")?
            }
            FunctionExprKind::StaticArrayPointer {
                array: inner,
                is_readonly,
                ..
            } => {
                if !is_readonly {
                    return Err(FunctionIrError::new(
                        expr.span,
                        "static array pointer must be readonly",
                    ));
                }
                self.validate_value_expr(inner)?;
            }
            FunctionExprKind::Unary { expr: inner, .. }
            | FunctionExprKind::OptionalSome { expr: inner }
            | FunctionExprKind::ErrorOk { expr: inner }
            | FunctionExprKind::ErrorErr { expr: inner }
            | FunctionExprKind::TaggedUnionTag { expr: inner }
            | FunctionExprKind::TaggedUnionPayload { expr: inner }
            | FunctionExprKind::Try { expr: inner }
            | FunctionExprKind::LoadUnaligned { ptr: inner, .. }
            | FunctionExprKind::Splat { value: inner }
            | FunctionExprKind::Bitmask { vector: inner }
            | FunctionExprKind::BitIntrinsic { value: inner, .. }
            | FunctionExprKind::CharFromU32 { value: inner }
            | FunctionExprKind::Cast { expr: inner, .. }
            | FunctionExprKind::TraitObjectUpcast { expr: inner, .. }
            | FunctionExprKind::TraitObjectCoercion { expr: inner, .. } => {
                self.validate_value_expr(inner)?
            }
            FunctionExprKind::CallableCoercion { state, .. } => self.validate_value_expr(state)?,
            FunctionExprKind::FunctionCallable { function } => {
                self.validate_value_expr(function)?
            }
            FunctionExprKind::Discard(inner) => self.validate_expr(inner)?,
            FunctionExprKind::Range(range) => {
                if let Some(start) = &range.start {
                    self.validate_value_expr(start)?;
                }
                if let Some(end) = &range.end {
                    self.validate_value_expr(end)?;
                }
            }
            FunctionExprKind::RangeBound { range, .. } => self.validate_value_expr(range)?,
            FunctionExprKind::AddrOf(place) => self.validate_place(place)?,
            FunctionExprKind::ArrayLiteral { elems } => match elems {
                FunctionArrayElements::List(elems) => {
                    for elem in elems {
                        self.validate_value_expr(elem)?;
                    }
                }
                FunctionArrayElements::Repeat { value, .. } => self.validate_value_expr(value)?,
            },
            FunctionExprKind::Tuple(elems) => {
                for elem in elems {
                    self.validate_value_expr(elem)?;
                }
            }
            FunctionExprKind::TupleField { value, .. } => self.validate_value_expr(value)?,
            FunctionExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    self.validate_value_expr(&field.value)?;
                }
            }
            FunctionExprKind::UnionLiteral { field, .. } => {
                self.validate_value_expr(&field.value)?
            }
            FunctionExprKind::UnionStorageLiteral { bytes, relocations } => {
                let mut previous_end = 0usize;
                for relocation in relocations {
                    if relocation.width == 0 {
                        return Err(FunctionIrError::new(
                            expr.span,
                            "union storage relocation has zero width",
                        ));
                    }
                    let Some(end) = relocation.offset.checked_add(relocation.width) else {
                        return Err(FunctionIrError::new(
                            expr.span,
                            "union storage relocation range overflows",
                        ));
                    };
                    if end > bytes.len() {
                        return Err(FunctionIrError::new(
                            expr.span,
                            "union storage relocation is out of bounds",
                        ));
                    }
                    if relocation.offset < previous_end {
                        return Err(FunctionIrError::new(
                            expr.span,
                            "union storage relocations overlap or are not sorted",
                        ));
                    }
                    if bytes[relocation.offset..end].iter().any(Option::is_none) {
                        return Err(FunctionIrError::new(
                            expr.span,
                            "union storage relocation covers uninitialized bytes",
                        ));
                    }
                    self.validate_value_expr(&relocation.pointee)?;
                    previous_end = end;
                }
            }
            FunctionExprKind::Binary { lhs, rhs, .. }
            | FunctionExprKind::Index { lhs, index: rhs }
            | FunctionExprKind::ExtractElement {
                vector: lhs,
                index: rhs,
            } => {
                self.validate_value_expr(lhs)?;
                self.validate_value_expr(rhs)?;
            }
            FunctionExprKind::InsertElement {
                vector,
                index,
                value,
            } => {
                self.validate_value_expr(vector)?;
                self.validate_value_expr(index)?;
                self.validate_value_expr(value)?;
            }
            FunctionExprKind::Assign { place, rhs, .. } => {
                self.validate_place(place)?;
                self.validate_value_expr(rhs)?;
            }
            FunctionExprKind::Call { callee, args } => {
                self.validate_callee(callee)?;
                for arg in args {
                    self.validate_value_expr(arg)?;
                }
            }
            FunctionExprKind::Field { lhs, .. } => self.validate_value_expr(lhs)?,
            FunctionExprKind::Slice { lhs, range, .. } => {
                self.validate_value_expr(lhs)?;
                if let Some(start) = &range.start {
                    self.validate_value_expr(start)?;
                }
                if let Some(end) = &range.end {
                    self.validate_value_expr(end)?;
                }
            }
            FunctionExprKind::InlineAsm(asm) => {
                for input in &asm.inputs {
                    self.validate_value_expr(&input.value)?;
                }
                for output in &asm.outputs {
                    self.validate_place(&output.place)?;
                }
            }
            FunctionExprKind::Atomic(atomic) => self.validate_atomic(atomic)?,
            FunctionExprKind::EnumVariant { fields, .. } => {
                for field in fields {
                    self.validate_value_expr(field)?;
                }
            }
            FunctionExprKind::EnumTag { value }
            | FunctionExprKind::EnumPayloadField { value, .. } => {
                self.validate_value_expr(value)?;
            }
            FunctionExprKind::Error => {
                return Err(FunctionIrError::new(
                    expr.span,
                    "error expression escaped into function IR",
                ));
            }
            FunctionExprKind::Integer(_)
            | FunctionExprKind::Float(_)
            | FunctionExprKind::String(_)
            | FunctionExprKind::ByteString(_)
            | FunctionExprKind::Char(_)
            | FunctionExprKind::ByteChar(_)
            | FunctionExprKind::Bool(_)
            | FunctionExprKind::Null
            | FunctionExprKind::ConstGeneric(_)
            | FunctionExprKind::Global(_)
            | FunctionExprKind::GlobalInstance { .. }
            | FunctionExprKind::Function(_)
            | FunctionExprKind::FunctionInstance { .. }
            | FunctionExprKind::ClosureFunctionPointer { .. }
            | FunctionExprKind::EnumVariantTag(_)
            | FunctionExprKind::BuiltinValue(_)
            | FunctionExprKind::CallerLocation(_)
            | FunctionExprKind::Trap => {}
        }
        Ok(())
    }

    fn validate_atomic(&self, atomic: &FunctionAtomic) -> Result<(), FunctionIrError> {
        match atomic {
            FunctionAtomic::Load { ptr, .. } => self.validate_value_expr(ptr),
            FunctionAtomic::Store { ptr, value, .. } | FunctionAtomic::Rmw { ptr, value, .. } => {
                self.validate_value_expr(ptr)?;
                self.validate_value_expr(value)
            }
            FunctionAtomic::Cmpxchg {
                ptr,
                expected,
                desired,
                ..
            } => {
                self.validate_value_expr(ptr)?;
                self.validate_value_expr(expected)?;
                self.validate_value_expr(desired)
            }
            FunctionAtomic::Fence { .. } => Ok(()),
        }
    }

    fn validate_callee(&self, callee: &FunctionCallee) -> Result<(), FunctionIrError> {
        match callee {
            FunctionCallee::Tracked { callee, .. } => self.validate_callee(callee),
            FunctionCallee::ClosureEntry {
                state: receiver, ..
            }
            | FunctionCallee::Method { receiver, .. }
            | FunctionCallee::TraitMethod { receiver, .. }
            | FunctionCallee::BuiltinMethod { receiver, .. }
            | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
            | FunctionCallee::DynamicTraitMethod { receiver, .. }
            | FunctionCallee::Callable(receiver)
            | FunctionCallee::FunctionPointer(receiver) => self.validate_value_expr(receiver),
            FunctionCallee::Function(_)
            | FunctionCallee::FunctionInstance { .. }
            | FunctionCallee::TraitAssociatedFunction { .. }
            | FunctionCallee::BuiltinOperator(_) => Ok(()),
        }
    }

    fn validate_place(&self, place: &FunctionPlace) -> Result<(), FunctionIrError> {
        match &place.base {
            FunctionPlaceBase::Local(local_id) => {
                self.require_local(*local_id, place.span, "place local")?
            }
            FunctionPlaceBase::Deref(expr) => self.validate_value_expr(expr)?,
            FunctionPlaceBase::Error => {
                return Err(FunctionIrError::new(
                    place.span,
                    "error place escaped into function IR",
                ));
            }
            FunctionPlaceBase::Global(_) | FunctionPlaceBase::GlobalInstance { .. } => {}
        }
        for elem in &place.elems {
            match elem {
                FunctionPlaceElem::Index(index) => self.validate_value_expr(index)?,
                FunctionPlaceElem::Error => {
                    return Err(FunctionIrError::new(
                        place.span,
                        "error place element escaped into function IR",
                    ));
                }
                FunctionPlaceElem::Field(_) | FunctionPlaceElem::TupleField(_) => {}
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
            | FunctionTerminator::Try { span, .. }
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
        let ty = test_ty();
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

    fn test_ty() -> nia_ids::InternedTyId {
        let (type_store, module_id) = test_type_fixture();
        type_store
            .append_for_module(*module_id)
            .intern(nia_ty::TyKind::Tuple(Vec::new()))
    }

    fn test_type_fixture() -> &'static (nia_ty::TypeStore, nia_ids::ModuleId) {
        static TYPE_STORE: std::sync::OnceLock<(nia_ty::TypeStore, nia_ids::ModuleId)> =
            std::sync::OnceLock::new();
        TYPE_STORE.get_or_init(|| {
            let mut module_ids = nia_ids::ModuleIdAllocator::new();
            (nia_ty::TypeStore::new(), module_ids.allocate())
        })
    }

    fn sym(text: &str) -> nia_symbol::SymbolId {
        nia_symbol::SymbolId::from_stable_hash(nia_symbol::stable_hash(text))
    }

    fn closure_entry() -> FunctionClosureEntry {
        let span = Span::default();
        let ty = test_ty();
        let mut module_ids = nia_ids::ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let mut body = empty_body(vec![FunctionScope {
            id: FunctionScopeId(0),
            parent: None,
            span,
        }]);
        body.locals = vec![
            FunctionLocal {
                id: LocalId(0),
                name: crate::LocalName::named(sym("state")),
                kind: FunctionLocalKind::Param,
                ty,
                span,
            },
            FunctionLocal {
                id: LocalId(1),
                name: crate::LocalName::named(sym("value")),
                kind: FunctionLocalKind::Param,
                ty,
                span,
            },
        ];
        FunctionClosureEntry {
            closure_id: nia_ids::ClosureId {
                owner: nia_ids::GlobalDefId {
                    module_id,
                    def_id: nia_ids::DefId(1),
                },
                ordinal: 0,
            },
            state_ty: ty,
            state_param: LocalId(0),
            params: vec![LocalId(1)],
            return_type: ty,
            body,
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
    fn rejects_dangling_structural_terminator_targets() {
        let span = Span::default();
        let scope = FunctionScope {
            id: FunctionScopeId(0),
            parent: None,
            span,
        };
        let mut loop_body = empty_body(vec![scope.clone()]);
        loop_body.blocks[0].terminator = FunctionTerminator::Loop {
            header: FunctionForHeader::Infinite,
            body: FunctionBlockId(0),
            continue_target: FunctionBlockId(99),
            break_target: FunctionBlockId(0),
            span,
        };
        let loop_error = validate_function_body(&loop_body)
            .expect_err("loop continue metadata must reference a real block");
        assert!(loop_error.message.contains("missing block `99`"));

        let mut switch_body = empty_body(vec![scope]);
        switch_body.blocks[0].terminator = FunctionTerminator::Switch {
            target: FunctionExpr {
                span,
                ty: test_ty(),
                kind: FunctionExprKind::Integer("0".to_string()),
            },
            arms: Vec::new(),
            default: Some(FunctionBlockId(0)),
            fallback: FunctionBlockId(99),
            span,
        };
        let switch_error = validate_function_body(&switch_body)
            .expect_err("inactive switch fallback must remain structurally valid");
        assert!(switch_error.message.contains("missing block `99`"));
    }

    #[test]
    fn validates_closure_entry_local_contract() {
        validate_function_closure_entry(&closure_entry()).expect("well-formed closure entry");

        let mut missing = closure_entry();
        missing.params = vec![LocalId(9)];
        let error = validate_function_closure_entry(&missing)
            .expect_err("closure ABI parameter must name a body local");
        assert!(error.message.contains("missing local `9`"));

        let mut duplicate = closure_entry();
        duplicate.params = vec![LocalId(1), LocalId(1)];
        let error = validate_function_closure_entry(&duplicate)
            .expect_err("closure ABI parameters must be unique");
        assert!(error.message.contains("duplicate local"));

        let mut wrong_kind = closure_entry();
        wrong_kind.body.locals[0].kind = FunctionLocalKind::ImmutableBinding;
        let error = validate_function_closure_entry(&wrong_kind)
            .expect_err("closure state local must be an ABI parameter");
        assert!(error.message.contains("state local is not a parameter"));

        let mut wrong_return = closure_entry();
        let (type_store, module_id) = test_type_fixture();
        wrong_return.body.ty = type_store
            .append_for_module(*module_id)
            .intern(nia_ty::TyKind::Error);
        let error = validate_function_closure_entry(&wrong_return)
            .expect_err("closure body and ABI return types must agree");
        assert!(error.message.contains("declared return type"));

        let mut unmapped = closure_entry();
        unmapped.body.locals.push(FunctionLocal {
            id: LocalId(2),
            name: crate::LocalName::named(sym("hidden")),
            kind: FunctionLocalKind::Param,
            ty: test_ty(),
            span: Span::default(),
        });
        let error = validate_function_closure_entry(&unmapped)
            .expect_err("every closure body parameter must have an ABI mapping");
        assert!(error.message.contains("unmapped parameter local `2`"));
    }

    #[test]
    fn accepts_return_in_defer_body() {
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

        validate_function_defer_body(&[], &defer_body).expect("defer return should be valid");
    }

    #[test]
    fn rejects_defer_branch_to_missing_outer_block() {
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
                terminator: FunctionTerminator::Branch {
                    target: FunctionBlockId(99),
                    span,
                },
            }],
            entry: FunctionBlockId(0),
        };

        let error = validate_function_defer_body(&[], &defer_body)
            .expect_err("missing outer block should fail");
        assert!(
            error.message.contains("references missing block `99`"),
            "{error:?}"
        );
    }

    #[test]
    fn rejects_effect_only_expression_in_value_position() {
        let span = Span::default();
        let ty = test_ty();
        for kind in [
            FunctionExprKind::Trap,
            FunctionExprKind::Atomic(FunctionAtomic::Store {
                ty,
                ptr: Box::new(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("0".to_string()),
                }),
                value: Box::new(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("1".to_string()),
                }),
                order: crate::AtomicOrder::Monotonic,
            }),
            FunctionExprKind::Atomic(FunctionAtomic::Fence {
                order: crate::AtomicOrder::SeqCst,
            }),
        ] {
            let body = FunctionBody {
                span,
                locals: vec![FunctionLocal {
                    id: LocalId(0),
                    name: crate::LocalName::named(sym("value")),
                    kind: crate::FunctionLocalKind::MutableBinding,
                    ty,
                    span,
                }],
                scopes: vec![FunctionScope {
                    id: FunctionScopeId(0),
                    parent: None,
                    span,
                }],
                blocks: vec![FunctionBlock {
                    id: FunctionBlockId(0),
                    scope: FunctionScopeId(0),
                    span,
                    ops: vec![FunctionOp::StoreLocal {
                        local_id: LocalId(0),
                        value: FunctionExpr { span, ty, kind },
                        span,
                    }],
                    terminator: FunctionTerminator::Tail { value: None, span },
                }],
                entry: FunctionBlockId(0),
                ty,
            };

            let error =
                validate_function_body(&body).expect_err("effect-only expr cannot produce a value");

            assert!(
                error
                    .message
                    .contains("effect-only expression used where a value is required"),
                "{error:?}"
            );
        }
    }

    #[test]
    fn rejects_error_expression() {
        let span = Span::default();
        let ty = test_ty();
        let body = FunctionBody {
            span,
            locals: Vec::new(),
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span,
            }],
            blocks: vec![FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: vec![FunctionOp::Expr(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Error,
                })],
                terminator: FunctionTerminator::Tail { value: None, span },
            }],
            entry: FunctionBlockId(0),
            ty,
        };

        let error = validate_function_body(&body).expect_err("error expr should fail");

        assert!(
            error.message.contains("error expression escaped"),
            "{error:?}"
        );
    }

    fn body_with_union_storage(
        bytes: Vec<Option<u8>>,
        relocations: Vec<crate::FunctionUnionRelocation>,
    ) -> FunctionBody {
        let span = Span::new(1, 9);
        let ty = test_ty();
        FunctionBody {
            span,
            locals: Vec::new(),
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span,
            }],
            blocks: vec![FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: vec![FunctionOp::Expr(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::UnionStorageLiteral { bytes, relocations },
                })],
                terminator: FunctionTerminator::Tail { value: None, span },
            }],
            entry: FunctionBlockId(0),
            ty,
        }
    }

    fn relocation(
        offset: usize,
        width: usize,
        pointee_kind: FunctionExprKind,
    ) -> crate::FunctionUnionRelocation {
        let mut module_ids = nia_ids::ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let span = Span::new(2, 4);
        crate::FunctionUnionRelocation {
            offset,
            width,
            allocation: crate::PromotedAllocationId::new(module_id, span),
            pointee: Box::new(FunctionExpr {
                span,
                ty: test_ty(),
                kind: pointee_kind,
            }),
        }
    }

    #[test]
    fn accepts_well_formed_union_storage_relocation() {
        let body = body_with_union_storage(
            vec![Some(0); 8],
            vec![relocation(0, 8, FunctionExprKind::Integer("1".to_string()))],
        );

        validate_function_body(&body).expect("well-formed relocation should be valid");
    }

    #[test]
    fn rejects_union_storage_relocation_over_uninitialized_bytes() {
        let mut bytes = vec![Some(0); 8];
        bytes[3] = None;
        let body = body_with_union_storage(
            bytes,
            vec![relocation(0, 8, FunctionExprKind::Integer("1".to_string()))],
        );

        let error =
            validate_function_body(&body).expect_err("relocation storage must remain initialized");

        assert!(
            error.message.contains("covers uninitialized bytes"),
            "{error:?}"
        );
    }

    #[test]
    fn rejects_malformed_union_storage_relocation_ranges() {
        let cases = [
            (
                vec![relocation(0, 0, FunctionExprKind::Integer("1".to_string()))],
                "zero width",
            ),
            (
                vec![relocation(
                    usize::MAX,
                    2,
                    FunctionExprKind::Integer("1".to_string()),
                )],
                "range overflows",
            ),
            (
                vec![relocation(4, 8, FunctionExprKind::Integer("1".to_string()))],
                "out of bounds",
            ),
            (
                vec![
                    relocation(0, 4, FunctionExprKind::Integer("1".to_string())),
                    relocation(2, 4, FunctionExprKind::Integer("2".to_string())),
                ],
                "overlap or are not sorted",
            ),
        ];

        for (relocations, expected) in cases {
            let body = body_with_union_storage(vec![Some(0); 8], relocations);
            let error = validate_function_body(&body)
                .expect_err("malformed relocation range should be rejected");
            assert!(error.message.contains(expected), "{error:?}");
        }
    }

    #[test]
    fn validates_union_storage_relocation_pointee() {
        let body = body_with_union_storage(
            vec![Some(0); 8],
            vec![relocation(0, 8, FunctionExprKind::Error)],
        );

        let error = validate_function_body(&body)
            .expect_err("invalid relocation pointee cannot bypass validation");

        assert!(
            error.message.contains("error expression escaped"),
            "{error:?}"
        );
    }
}
