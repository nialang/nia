// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{BinaryOp, UnaryOp};
use nia_body_ir::{
    AsmOption, BuiltinConst, BuiltinMethod, BuiltinPlaceMethod, PlaceBase, PlaceElem,
    TypedArrayElements, TypedAtomic, TypedBinding, TypedBody, TypedCallee, TypedExpr,
    TypedExprKind, TypedForIn, TypedIfPattern, TypedInlineAsm, TypedLocal, TypedLocalKind,
    TypedLoop, TypedMemoryIntrinsicSource, TypedPattern, TypedPatternKind, TypedPlace, TypedRange,
    TypedSliceRange, TypedStmt, TypedStmtKind, TypedSwitch, TypedSwitchArmBody, TypedWhile,
};
use nia_ids::{InternedTyId, LocalId, ModuleId};
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyKind, TypeStore, TypeStoreAppend};

use nia_function_ir::{
    AtomicOrder, AtomicRmwOp, FunctionArrayElements, FunctionAsmInput, FunctionAsmOption,
    FunctionAsmOutput, FunctionAtomic, FunctionBinding, FunctionBitIntrinsicOp, FunctionBlock,
    FunctionBlockId, FunctionBody, FunctionBuiltinMethod, FunctionBuiltinOperator,
    FunctionBuiltinOperatorOp, FunctionBuiltinValue, FunctionCallee, FunctionDeferBody,
    FunctionErrorUnionTag, FunctionExpr, FunctionExprKind, FunctionFieldInit, FunctionForHeader,
    FunctionInlineAsm, FunctionLocal, FunctionLocalKind, FunctionMemoryIntrinsic,
    FunctionMemoryIntrinsicOp, FunctionMemoryIntrinsicSource, FunctionOp, FunctionOptionalTag,
    FunctionPlace, FunctionPlaceBase, FunctionPlaceElem, FunctionRange, FunctionScope,
    FunctionScopeId, FunctionSliceRange, FunctionSwitchArm, FunctionTerminator, FunctionTryKind,
    GeneratedLocalName, LocalName, validate_function_body,
};

mod expr;
mod flow;
mod input;
mod support;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionLoweringDiagnostic {
    pub span: Span,
    pub message: String,
}

impl From<nia_function_ir::FunctionIrError> for FunctionLoweringDiagnostic {
    fn from(error: nia_function_ir::FunctionIrError) -> Self {
        Self {
            span: error.span,
            message: error.message,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoweredFunctionBody {
    pub body: FunctionBody,
}

pub struct FunctionTypeContext<'a> {
    store: &'a TypeStore,
    append: TypeStoreAppend,
}

impl<'a> FunctionTypeContext<'a> {
    pub fn for_module(store: &'a TypeStore, module_id: ModuleId) -> Self {
        Self {
            store,
            append: store.append_for_module(module_id),
        }
    }

    fn get(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.store.get(ty)
    }

    fn intern(&self, kind: TyKind) -> InternedTyId {
        self.append.intern(kind)
    }
}

pub fn lower_function_body(
    module_id: ModuleId,
    body: &TypedBody,
    types: FunctionTypeContext<'_>,
) -> Result<LoweredFunctionBody, FunctionLoweringDiagnostic> {
    input::validate_function_lowering_input(body)?;
    let mut lowerer = FunctionLowerer::new(module_id, types);
    let body = lowerer.lower_body(body);
    validate_function_body(&body).map_err(FunctionLoweringDiagnostic::from)?;
    Ok(LoweredFunctionBody { body })
}

struct FunctionLowerer<'a> {
    next_block: u32,
    next_scope: u32,
    next_temp_local: u32,
    module_id: ModuleId,
    temp_locals: Vec<FunctionLocal>,
    scopes: Vec<FunctionScope>,
    loop_targets: Vec<LoopTargetIds>,
    types: FunctionTypeContext<'a>,
}

#[derive(Debug, Clone, Copy)]
struct LoopTargetIds {
    break_target: FunctionBlockId,
    continue_target: FunctionBlockId,
}

#[derive(Debug, Clone, Copy)]
enum Fallthrough {
    Tail,
    Branch(FunctionBlockId),
    StoreThenBranch {
        local_id: LocalId,
        target: FunctionBlockId,
    },
}

struct StatementIf<'a> {
    cond: &'a TypedExpr,
    then_branch: &'a TypedBody,
    else_branch: Option<&'a TypedExpr>,
}

impl<'a> FunctionLowerer<'a> {
    fn new(module_id: ModuleId, types: FunctionTypeContext<'a>) -> Self {
        Self {
            next_block: 0,
            next_scope: 0,
            next_temp_local: 0,
            module_id,
            temp_locals: Vec::new(),
            scopes: Vec::new(),
            loop_targets: Vec::new(),
            types,
        }
    }

    fn lower_body(&mut self, body: &TypedBody) -> FunctionBody {
        self.reset_function_state();
        // Function IR keeps one flat local table per function body. Nested
        // source bodies still have their own scopes and blocks, but their
        // locals must be visible to validation and later codegen by id.
        self.next_temp_local = self.next_available_local(body);
        let root_scope = self.alloc_scope(None, body.span);
        let entry = self.alloc_block();
        let mut blocks = Vec::new();
        let mut locals = Vec::new();
        self.lower_body_into(body, entry, root_scope, &mut blocks, Fallthrough::Tail);
        self.collect_body_locals(body, &mut locals);
        locals.extend(self.temp_locals.clone());
        FunctionBody {
            span: body.span,
            locals,
            scopes: self.scopes.clone(),
            blocks,
            entry,
            ty: body.ty,
        }
    }

    fn is_never_ty(&self, ty: InternedTyId) -> bool {
        matches!(
            self.types.get(ty),
            Some(TyKind::Primitive(PrimitiveTy::Never))
        )
    }

    fn expr_lowers_as_effect_only(&self, expr: &TypedExpr) -> bool {
        self.is_never_ty(expr.ty)
            || matches!(
                expr.kind,
                TypedExprKind::MemoryIntrinsic(_)
                    | TypedExprKind::InlineAsm(_)
                    | TypedExprKind::Trap
                    | TypedExprKind::Discard(_)
            )
            || matches!(
                &expr.kind,
                TypedExprKind::Atomic(atomic) if Self::atomic_lowers_as_effect_only(atomic)
            )
    }

    fn expr_lowers_as_terminating_effect(&self, expr: &TypedExpr) -> bool {
        self.is_never_ty(expr.ty) || matches!(expr.kind, TypedExprKind::Trap)
    }

    fn atomic_lowers_as_effect_only(atomic: &TypedAtomic) -> bool {
        matches!(
            atomic,
            TypedAtomic::Store { .. } | TypedAtomic::Fence { .. }
        )
    }

    fn reset_function_state(&mut self) {
        self.next_block = 0;
        self.next_scope = 0;
        self.next_temp_local = 0;
        self.temp_locals.clear();
        self.scopes.clear();
        self.loop_targets.clear();
    }
}
