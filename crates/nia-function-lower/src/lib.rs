// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{BinaryOp, UnaryOp};
use nia_body_ir::{
    AsmOption, BuiltinConst, BuiltinMethod, BuiltinPlaceMethod, PlaceBase, PlaceElem,
    TypedArrayElements, TypedAtomic, TypedBinding, TypedBody, TypedCallee, TypedExpr,
    TypedExprKind, TypedForIn, TypedIfPattern, TypedInlineAsm, TypedLocal, TypedLocalKind,
    TypedLoop, TypedMemoryIntrinsicSource, TypedPattern, TypedPatternKind, TypedPlace, TypedRange,
    TypedSliceRange, TypedStmt, TypedStmtKind, TypedSwitch, TypedSwitchArmBody, TypedSwitchPattern,
    TypedSwitchPatternKind, TypedWhile,
};
use nia_ids::{BuiltinTraitMethod, InternedTyId, LocalId, ModuleId};
use nia_span::Span;
use nia_ty::{BuiltinTrait, PrimitiveTy, TyInterner, TyKind};

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
    validate_function_body,
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

pub fn lower_function_body(body: &TypedBody) -> Result<FunctionBody, FunctionLoweringDiagnostic> {
    input::validate_function_lowering_input(body)?;
    let body = FunctionLowerer::new(ModuleId(0), None).lower_body(body);
    validate_function_body(&body).map_err(FunctionLoweringDiagnostic::from)?;
    Ok(body)
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoweredFunctionBody {
    pub interner: TyInterner,
    pub body: FunctionBody,
}

pub fn lower_function_body_with_interner(
    module_id: ModuleId,
    body: &TypedBody,
    interner: &TyInterner,
) -> Result<LoweredFunctionBody, FunctionLoweringDiagnostic> {
    input::validate_function_lowering_input(body)?;
    let mut lowerer = FunctionLowerer::new(module_id, Some(interner));
    let body = lowerer.lower_body(body);
    validate_function_body(&body).map_err(FunctionLoweringDiagnostic::from)?;
    Ok(LoweredFunctionBody {
        interner: lowerer.finish_interner(),
        body,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoweredFunctionBodies {
    pub interner: TyInterner,
    pub bodies: std::collections::HashMap<nia_ids::GlobalDefId, FunctionBody>,
    pub diagnostics: Vec<FunctionLoweringDiagnostic>,
}

pub fn lower_function_bodies_with_interner<'a>(
    module_id: ModuleId,
    bodies: impl IntoIterator<Item = (&'a nia_ids::GlobalDefId, &'a TypedBody)>,
    interner: &TyInterner,
) -> Result<LoweredFunctionBodies, Vec<FunctionLoweringDiagnostic>> {
    let mut lowerer = FunctionLowerer::new(module_id, Some(interner));
    let mut bodies = bodies.into_iter().collect::<Vec<_>>();
    bodies.sort_by_key(|(def_id, _)| **def_id);
    let mut lowered_bodies = std::collections::HashMap::new();
    let mut diagnostics = Vec::new();
    for (def_id, body) in bodies {
        if let Err(error) = input::validate_function_lowering_input(body) {
            diagnostics.push(error);
            continue;
        }
        let lowered = lowerer.lower_body(body);
        match validate_function_body(&lowered) {
            Ok(()) => {
                lowered_bodies.insert(*def_id, lowered);
            }
            Err(error) => diagnostics.push(FunctionLoweringDiagnostic::from(error)),
        }
    }
    let lowered = LoweredFunctionBodies {
        interner: lowerer.finish_interner(),
        bodies: lowered_bodies,
        diagnostics,
    };
    if lowered.diagnostics.is_empty() {
        Ok(lowered)
    } else {
        Err(lowered.diagnostics)
    }
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
            self.interner.as_ref().and_then(|interner| interner.get(ty)),
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

    fn finish_interner(&mut self) -> TyInterner {
        self.interner
            .take()
            .unwrap_or_else(|| TyInterner::new(self.module_id))
    }
}
