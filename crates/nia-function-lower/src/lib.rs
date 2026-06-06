// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{BinaryOp, UnaryOp};
use nia_body_ir::{
    AsmOption, BuiltinConst, BuiltinMethod, BuiltinPlaceMethod, PlaceBase, PlaceElem,
    TypedArrayElements, TypedBinding, TypedBody, TypedCallee, TypedExpr, TypedExprKind, TypedForIn,
    TypedForIterator, TypedInlineAsm, TypedLocal, TypedLocalKind, TypedLoop,
    TypedMemoryIntrinsicSource, TypedPlace, TypedRange, TypedSliceRange, TypedStmt, TypedStmtKind,
    TypedSwitch, TypedSwitchArmBody, TypedSwitchPattern, TypedWhile,
};
use nia_ids::{InternedTyId, LocalId, ModuleId};
use nia_span::Span;
use nia_ty::{TyInterner, TyKind};

use nia_function_ir::{
    FunctionArrayElements, FunctionAsmInput, FunctionAsmOption, FunctionAsmOutput, FunctionBinding,
    FunctionBlock, FunctionBlockId, FunctionBody, FunctionBuiltinMethod, FunctionBuiltinOperator,
    FunctionBuiltinOperatorOp, FunctionBuiltinValue, FunctionCallee, FunctionDeferBody,
    FunctionExpr, FunctionExprKind, FunctionFieldInit, FunctionForHeader, FunctionInlineAsm,
    FunctionLocal, FunctionLocalKind, FunctionMemoryIntrinsic, FunctionMemoryIntrinsicOp,
    FunctionMemoryIntrinsicSource, FunctionOp, FunctionPlace, FunctionPlaceBase, FunctionPlaceElem,
    FunctionRange, FunctionRangeBound, FunctionScope, FunctionScopeId, FunctionSliceRange,
    FunctionSwitchArm, FunctionTerminator, FunctionTryKind,
};

mod expr;
mod flow;
mod support;

#[cfg(test)]
mod tests;

pub fn lower_function_body(body: &TypedBody) -> FunctionBody {
    FunctionLowerer::new(ModuleId(0), None).lower_body(body)
}

pub fn lower_function_body_with_interner(body: &TypedBody, interner: &TyInterner) -> FunctionBody {
    FunctionLowerer::new(interner.interner_id(), Some(interner)).lower_body(body)
}

struct FunctionLowerer {
    next_block: u32,
    next_scope: u32,
    next_temp_local: u32,
    module_id: ModuleId,
    temp_locals: Vec<FunctionLocal>,
    scopes: Vec<FunctionScope>,
    loop_targets: Vec<LoopTargetIds>,
    interner: Option<TyInterner>,
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

impl FunctionLowerer {
    fn new(module_id: ModuleId, interner: Option<&TyInterner>) -> Self {
        Self {
            next_block: 0,
            next_scope: 0,
            next_temp_local: 0,
            module_id,
            temp_locals: Vec::new(),
            scopes: Vec::new(),
            loop_targets: Vec::new(),
            interner: interner.cloned(),
        }
    }

    fn lower_body(&mut self, body: &TypedBody) -> FunctionBody {
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
}
