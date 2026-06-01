// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use crate::ModuleLowerer;
use nia_function_ir::{
    FunctionArrayElements, FunctionBlock, FunctionBlockId, FunctionBody, FunctionCallee,
    FunctionDeferBody, FunctionExpr, FunctionExprKind, FunctionFieldInit, FunctionForHeader,
    FunctionInlineAsm, FunctionOp, FunctionPlace, FunctionPlaceBase, FunctionPlaceElem,
    FunctionRange, FunctionSliceRange, FunctionTerminator,
};
use nia_opt::{OptimizationDepth, OptimizationPolicy};

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn optimize_function_body(&mut self, mut body: FunctionBody) -> FunctionBody {
        BackendOptPipeline::for_policy(&self.optimization).run(&mut body);
        body
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendOptPass {
    SimplifySameTypeCasts,
    RemoveNoopLocalStores,
    RemovePureExprOps,
    FoldConstantBoolBranches,
    MergeEmptyJumpBlocks,
    RemoveUnreachableBlocks,
    OptimizeDeferBodies,
}

impl BackendOptPass {
    fn name(self) -> &'static str {
        match self {
            Self::SimplifySameTypeCasts => "simplify-same-type-casts",
            Self::RemoveNoopLocalStores => "remove-noop-local-stores",
            Self::RemovePureExprOps => "remove-pure-expr-ops",
            Self::FoldConstantBoolBranches => "fold-constant-bool-branches",
            Self::MergeEmptyJumpBlocks => "merge-empty-jump-blocks",
            Self::RemoveUnreachableBlocks => "remove-unreachable-blocks",
            Self::OptimizeDeferBodies => "optimize-defer-bodies",
        }
    }

    fn run(self, body: &mut FunctionBody) -> bool {
        match self {
            Self::SimplifySameTypeCasts => simplify_same_type_casts_in_blocks(&mut body.blocks),
            Self::RemoveNoopLocalStores => remove_noop_local_stores(&mut body.blocks),
            Self::RemovePureExprOps => remove_pure_expr_ops(&mut body.blocks),
            Self::FoldConstantBoolBranches => fold_constant_bool_branches(&mut body.blocks),
            Self::MergeEmptyJumpBlocks => merge_empty_jump_blocks(body),
            Self::RemoveUnreachableBlocks => remove_unreachable_blocks(body),
            Self::OptimizeDeferBodies => optimize_defer_bodies(&mut body.blocks),
        }
    }
}

struct BackendOptPipeline {
    passes: &'static [BackendOptPass],
}

impl BackendOptPipeline {
    fn for_policy(policy: &OptimizationPolicy) -> Self {
        if matches!(
            policy.simplify_cfg,
            OptimizationDepth::Cheap | OptimizationDepth::Full | OptimizationDepth::Aggressive
        ) {
            Self { passes: O1_PASSES }
        } else {
            Self { passes: &[] }
        }
    }

    fn run(&self, body: &mut FunctionBody) -> bool {
        let mut changed = false;
        for pass in self.passes {
            debug_assert!(!pass.name().is_empty());
            changed |= pass.run(body);
        }
        changed
    }
}

const O1_PASSES: &[BackendOptPass] = &[
    BackendOptPass::SimplifySameTypeCasts,
    BackendOptPass::RemoveNoopLocalStores,
    BackendOptPass::RemovePureExprOps,
    BackendOptPass::FoldConstantBoolBranches,
    BackendOptPass::MergeEmptyJumpBlocks,
    BackendOptPass::RemoveUnreachableBlocks,
    BackendOptPass::OptimizeDeferBodies,
];

fn remove_pure_expr_ops(blocks: &mut [FunctionBlock]) -> bool {
    let mut changed = false;
    for block in blocks {
        let before = block.ops.len();
        block.ops.retain(|op| !is_pure_expr_op(op));
        changed |= block.ops.len() != before;
        for op in &mut block.ops {
            if let FunctionOp::Defer(body) = op {
                changed |= remove_pure_expr_ops(&mut body.blocks);
            }
        }
    }
    changed
}

fn is_pure_expr_op(op: &FunctionOp) -> bool {
    matches!(op, FunctionOp::Expr(expr) if is_pure_discardable_expr(expr))
}

fn is_pure_discardable_expr(expr: &FunctionExpr) -> bool {
    match &expr.kind {
        FunctionExprKind::Error => false,
        FunctionExprKind::Integer(_)
        | FunctionExprKind::Float(_)
        | FunctionExprKind::String(_)
        | FunctionExprKind::ByteString(_)
        | FunctionExprKind::Char(_)
        | FunctionExprKind::ByteChar(_)
        | FunctionExprKind::Bool(_)
        | FunctionExprKind::Local(_)
        | FunctionExprKind::Global(_)
        | FunctionExprKind::Function(_)
        | FunctionExprKind::FunctionInstance { .. }
        | FunctionExprKind::EnumVariant(_)
        | FunctionExprKind::BuiltinValue(_) => true,
        FunctionExprKind::Discard(expr) => is_pure_discardable_expr(expr),
        FunctionExprKind::Range(_)
        | FunctionExprKind::ArrayLiteral { .. }
        | FunctionExprKind::StructLiteral { .. }
        | FunctionExprKind::UnionLiteral { .. }
        | FunctionExprKind::Unary { .. }
        | FunctionExprKind::Binary { .. }
        | FunctionExprKind::Cast { .. }
        | FunctionExprKind::InlineAsm(_)
        | FunctionExprKind::CStringPointer { .. }
        | FunctionExprKind::AddrOf(_)
        | FunctionExprKind::Assign { .. }
        | FunctionExprKind::TraitObjectUpcast { .. }
        | FunctionExprKind::TraitObjectCoercion { .. }
        | FunctionExprKind::Call { .. }
        | FunctionExprKind::Field { .. }
        | FunctionExprKind::Index { .. }
        | FunctionExprKind::Slice { .. } => false,
    }
}

fn remove_noop_local_stores(blocks: &mut [FunctionBlock]) -> bool {
    let mut changed = false;
    for block in blocks {
        let before = block.ops.len();
        block.ops.retain(|op| !is_noop_local_store(op));
        changed |= block.ops.len() != before;
        for op in &mut block.ops {
            if let FunctionOp::Defer(body) = op {
                changed |= remove_noop_local_stores(&mut body.blocks);
            }
        }
    }
    changed
}

fn is_noop_local_store(op: &FunctionOp) -> bool {
    matches!(
        op,
        FunctionOp::StoreLocal {
            local_id,
            value:
                FunctionExpr {
                    kind: FunctionExprKind::Local(value_local),
                    ..
                },
            ..
        } if local_id == value_local
    )
}

fn simplify_same_type_casts_in_blocks(blocks: &mut [FunctionBlock]) -> bool {
    let mut changed = false;
    for block in blocks {
        for op in &mut block.ops {
            changed |= simplify_same_type_casts_in_op(op);
        }
        changed |= simplify_same_type_casts_in_terminator(&mut block.terminator);
    }
    changed
}

fn simplify_same_type_casts_in_op(op: &mut FunctionOp) -> bool {
    match op {
        FunctionOp::Binding(binding) => {
            if let Some(value) = &mut binding.value {
                simplify_same_type_casts_in_expr(value)
            } else {
                false
            }
        }
        FunctionOp::StoreLocal { value, .. } | FunctionOp::Expr(value) => {
            simplify_same_type_casts_in_expr(value)
        }
        FunctionOp::Defer(body) => simplify_same_type_casts_in_blocks(&mut body.blocks),
    }
}

fn simplify_same_type_casts_in_terminator(terminator: &mut FunctionTerminator) -> bool {
    match terminator {
        FunctionTerminator::If { cond, .. } => simplify_same_type_casts_in_expr(cond),
        FunctionTerminator::Switch { target, arms, .. } => {
            let mut changed = simplify_same_type_casts_in_expr(target);
            for arm in arms {
                changed |= simplify_same_type_casts_in_expr(&mut arm.pattern);
            }
            changed
        }
        FunctionTerminator::Loop { header, .. } => match header {
            FunctionForHeader::Condition(cond) => simplify_same_type_casts_in_expr(cond),
            FunctionForHeader::CStyle { cond } => {
                if let Some(cond) = cond {
                    simplify_same_type_casts_in_expr(cond)
                } else {
                    false
                }
            }
            FunctionForHeader::Infinite => false,
        },
        FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
            if let Some(value) = value {
                simplify_same_type_casts_in_expr(value)
            } else {
                false
            }
        }
        FunctionTerminator::Error { .. }
        | FunctionTerminator::Branch { .. }
        | FunctionTerminator::Next { .. } => false,
    }
}

fn simplify_same_type_casts_in_expr(expr: &mut FunctionExpr) -> bool {
    let mut changed = simplify_same_type_casts_in_expr_children(expr);
    if let FunctionExprKind::Cast { expr: inner, ty } = &mut expr.kind
        && inner.ty == *ty
    {
        let mut inner = (**inner).clone();
        inner.span = expr.span;
        *expr = inner;
        changed = true;
    }
    changed
}

fn simplify_same_type_casts_in_expr_children(expr: &mut FunctionExpr) -> bool {
    match &mut expr.kind {
        FunctionExprKind::Error
        | FunctionExprKind::Integer(_)
        | FunctionExprKind::Float(_)
        | FunctionExprKind::String(_)
        | FunctionExprKind::ByteString(_)
        | FunctionExprKind::Char(_)
        | FunctionExprKind::ByteChar(_)
        | FunctionExprKind::Bool(_)
        | FunctionExprKind::Local(_)
        | FunctionExprKind::Global(_)
        | FunctionExprKind::Function(_)
        | FunctionExprKind::FunctionInstance { .. }
        | FunctionExprKind::EnumVariant(_)
        | FunctionExprKind::BuiltinValue(_) => false,
        FunctionExprKind::Range(range) => simplify_same_type_casts_in_range(range),
        FunctionExprKind::InlineAsm(asm) => simplify_same_type_casts_in_inline_asm(asm),
        FunctionExprKind::CStringPointer { array, .. } => simplify_same_type_casts_in_expr(array),
        FunctionExprKind::ArrayLiteral { elems } => {
            simplify_same_type_casts_in_array_elements(elems)
        }
        FunctionExprKind::StructLiteral { fields, .. } => {
            let mut changed = false;
            for field in fields {
                changed |= simplify_same_type_casts_in_field_init(field);
            }
            changed
        }
        FunctionExprKind::UnionLiteral { field, .. } => {
            simplify_same_type_casts_in_field_init(field)
        }
        FunctionExprKind::Unary { expr, .. }
        | FunctionExprKind::Discard(expr)
        | FunctionExprKind::Cast { expr, .. }
        | FunctionExprKind::TraitObjectUpcast { expr, .. }
        | FunctionExprKind::TraitObjectCoercion { expr, .. } => {
            simplify_same_type_casts_in_expr(expr)
        }
        FunctionExprKind::AddrOf(place) => simplify_same_type_casts_in_place(place),
        FunctionExprKind::Binary { lhs, rhs, .. } => {
            simplify_same_type_casts_in_expr(lhs) | simplify_same_type_casts_in_expr(rhs)
        }
        FunctionExprKind::Assign { place, rhs, .. } => {
            simplify_same_type_casts_in_place(place) | simplify_same_type_casts_in_expr(rhs)
        }
        FunctionExprKind::Call { callee, args } => {
            let mut changed = simplify_same_type_casts_in_callee(callee);
            for arg in args {
                changed |= simplify_same_type_casts_in_expr(arg);
            }
            changed
        }
        FunctionExprKind::Field { lhs, .. } => simplify_same_type_casts_in_expr(lhs),
        FunctionExprKind::Index { lhs, index } => {
            simplify_same_type_casts_in_expr(lhs) | simplify_same_type_casts_in_expr(index)
        }
        FunctionExprKind::Slice { lhs, range, .. } => {
            simplify_same_type_casts_in_expr(lhs) | simplify_same_type_casts_in_slice_range(range)
        }
    }
}

fn simplify_same_type_casts_in_inline_asm(asm: &mut FunctionInlineAsm) -> bool {
    let mut changed = false;
    for input in &mut asm.inputs {
        changed |= simplify_same_type_casts_in_expr(&mut input.value);
    }
    for output in &mut asm.outputs {
        changed |= simplify_same_type_casts_in_place(&mut output.place);
    }
    changed
}

fn simplify_same_type_casts_in_array_elements(elems: &mut FunctionArrayElements) -> bool {
    match elems {
        FunctionArrayElements::List(elems) => {
            let mut changed = false;
            for elem in elems {
                changed |= simplify_same_type_casts_in_expr(elem);
            }
            changed
        }
        FunctionArrayElements::Repeat { value, .. } => simplify_same_type_casts_in_expr(value),
    }
}

fn simplify_same_type_casts_in_field_init(field: &mut FunctionFieldInit) -> bool {
    simplify_same_type_casts_in_expr(&mut field.value)
}

fn simplify_same_type_casts_in_callee(callee: &mut FunctionCallee) -> bool {
    match callee {
        FunctionCallee::Method { receiver, .. }
        | FunctionCallee::TraitMethod { receiver, .. }
        | FunctionCallee::DynamicTraitMethod { receiver, .. }
        | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
        | FunctionCallee::FunctionPointer(receiver) => simplify_same_type_casts_in_expr(receiver),
        FunctionCallee::Function(_)
        | FunctionCallee::FunctionInstance { .. }
        | FunctionCallee::BuiltinOperator(_) => false,
    }
}

fn simplify_same_type_casts_in_place(place: &mut FunctionPlace) -> bool {
    let mut changed = false;
    if let FunctionPlaceBase::Deref(expr) = &mut place.base {
        changed |= simplify_same_type_casts_in_expr(expr);
    }
    for elem in &mut place.elems {
        if let FunctionPlaceElem::Index(expr) = elem {
            changed |= simplify_same_type_casts_in_expr(expr);
        }
    }
    changed
}

fn simplify_same_type_casts_in_slice_range(range: &mut FunctionSliceRange) -> bool {
    let mut changed = false;
    if let Some(start) = &mut range.start {
        changed |= simplify_same_type_casts_in_expr(start);
    }
    if let Some(end) = &mut range.end {
        changed |= simplify_same_type_casts_in_expr(end);
    }
    changed
}

fn simplify_same_type_casts_in_range(range: &mut FunctionRange) -> bool {
    let mut changed = false;
    if let Some(start) = &mut range.start {
        changed |= simplify_same_type_casts_in_expr(start);
    }
    if let Some(end) = &mut range.end {
        changed |= simplify_same_type_casts_in_expr(end);
    }
    changed
}

fn fold_constant_bool_branches(blocks: &mut [FunctionBlock]) -> bool {
    let mut changed = false;
    for block in blocks {
        if let FunctionTerminator::If {
            cond,
            then_target,
            else_target,
            span,
        } = &block.terminator
            && let FunctionExprKind::Bool(value) = cond.kind
        {
            block.terminator = FunctionTerminator::Branch {
                target: if value { *then_target } else { *else_target },
                span: *span,
            };
            changed = true;
        }
    }
    changed
}

fn merge_empty_jump_blocks(body: &mut FunctionBody) -> bool {
    let mut any_changed = false;
    loop {
        let empty_jumps = empty_jump_targets(&body.blocks, Some(body.entry));
        if empty_jumps.is_empty() {
            break;
        }
        let mut changed = false;
        for block in &mut body.blocks {
            changed |= retarget_terminator(&mut block.terminator, &empty_jumps);
        }
        if !changed {
            break;
        }
        any_changed = true;
    }
    any_changed
}

fn empty_jump_targets(
    blocks: &[FunctionBlock],
    protected_entry: Option<FunctionBlockId>,
) -> HashMap<FunctionBlockId, FunctionBlockId> {
    let blocks_by_id = blocks
        .iter()
        .map(|block| (block.id, block))
        .collect::<HashMap<_, _>>();
    blocks
        .iter()
        .filter(|block| Some(block.id) != protected_entry && block.ops.is_empty())
        .filter_map(|block| {
            let target = jump_target(&block.terminator)?;
            let target_block = blocks_by_id.get(&target)?;
            (target != block.id && target_block.scope == block.scope).then_some((block.id, target))
        })
        .collect()
}

fn jump_target(terminator: &FunctionTerminator) -> Option<FunctionBlockId> {
    match terminator {
        FunctionTerminator::Branch { target, .. } | FunctionTerminator::Next { target, .. } => {
            Some(*target)
        }
        FunctionTerminator::Error { .. }
        | FunctionTerminator::If { .. }
        | FunctionTerminator::Switch { .. }
        | FunctionTerminator::Loop { .. }
        | FunctionTerminator::Return { .. }
        | FunctionTerminator::Tail { .. } => None,
    }
}

fn retarget_terminator(
    terminator: &mut FunctionTerminator,
    empty_jumps: &HashMap<FunctionBlockId, FunctionBlockId>,
) -> bool {
    match terminator {
        FunctionTerminator::Branch { target, .. } | FunctionTerminator::Next { target, .. } => {
            retarget_block_id(target, empty_jumps)
        }
        FunctionTerminator::If {
            then_target,
            else_target,
            ..
        } => {
            let then_changed = retarget_block_id(then_target, empty_jumps);
            let else_changed = retarget_block_id(else_target, empty_jumps);
            then_changed || else_changed
        }
        FunctionTerminator::Switch {
            arms,
            default,
            fallback,
            ..
        } => {
            let mut changed = false;
            for arm in arms {
                changed |= retarget_block_id(&mut arm.target, empty_jumps);
            }
            if let Some(default) = default {
                changed |= retarget_block_id(default, empty_jumps);
            }
            changed |= retarget_block_id(fallback, empty_jumps);
            changed
        }
        FunctionTerminator::Loop {
            body,
            continue_target,
            break_target,
            ..
        } => {
            let body_changed = retarget_block_id(body, empty_jumps);
            let continue_changed = retarget_block_id(continue_target, empty_jumps);
            let break_changed = retarget_block_id(break_target, empty_jumps);
            body_changed || continue_changed || break_changed
        }
        FunctionTerminator::Error { .. }
        | FunctionTerminator::Return { .. }
        | FunctionTerminator::Tail { .. } => false,
    }
}

fn retarget_block_id(
    target: &mut FunctionBlockId,
    empty_jumps: &HashMap<FunctionBlockId, FunctionBlockId>,
) -> bool {
    let mut changed = false;
    let mut seen = HashSet::new();
    while let Some(next) = empty_jumps.get(target).copied() {
        if !seen.insert(*target) {
            break;
        }
        *target = next;
        changed = true;
    }
    changed
}

fn remove_unreachable_blocks(body: &mut FunctionBody) -> bool {
    let reachable = reachable_blocks(body);
    if reachable.len() == body.blocks.len() {
        return false;
    }
    body.blocks.retain(|block| reachable.contains(&block.id));
    true
}

fn reachable_blocks(body: &FunctionBody) -> HashSet<FunctionBlockId> {
    reachable_block_ids(&body.blocks, body.entry)
}

fn reachable_defer_blocks(body: &FunctionDeferBody) -> HashSet<FunctionBlockId> {
    reachable_block_ids(&body.blocks, body.entry)
}

fn reachable_block_ids(
    blocks: &[FunctionBlock],
    entry: FunctionBlockId,
) -> HashSet<FunctionBlockId> {
    let blocks_by_id = blocks
        .iter()
        .map(|block| (block.id, block))
        .collect::<HashMap<_, _>>();
    let mut reachable = HashSet::new();
    let mut stack = vec![entry];
    while let Some(id) = stack.pop() {
        if !reachable.insert(id) {
            continue;
        }
        let Some(block) = blocks_by_id.get(&id) else {
            continue;
        };
        stack.extend(terminator_referenced_blocks(&block.terminator));
    }
    reachable
}

fn terminator_referenced_blocks(terminator: &FunctionTerminator) -> Vec<FunctionBlockId> {
    match terminator {
        FunctionTerminator::Error { .. }
        | FunctionTerminator::Return { .. }
        | FunctionTerminator::Tail { .. } => Vec::new(),
        FunctionTerminator::Branch { target, .. } | FunctionTerminator::Next { target, .. } => {
            vec![*target]
        }
        FunctionTerminator::If {
            then_target,
            else_target,
            ..
        } => vec![*then_target, *else_target],
        FunctionTerminator::Switch {
            arms,
            default,
            fallback,
            ..
        } => {
            let mut targets = arms.iter().map(|arm| arm.target).collect::<Vec<_>>();
            if let Some(default) = default {
                targets.push(*default);
            }
            targets.push(*fallback);
            targets
        }
        FunctionTerminator::Loop {
            body,
            continue_target,
            break_target,
            ..
        } => vec![*body, *continue_target, *break_target],
    }
}

fn optimize_defer_bodies(blocks: &mut [FunctionBlock]) -> bool {
    let mut changed = false;
    for block in blocks {
        for op in &mut block.ops {
            if let FunctionOp::Defer(body) = op {
                changed |= fold_constant_bool_branches(&mut body.blocks);
                changed |= merge_empty_defer_jump_blocks(body);
                changed |= remove_unreachable_defer_blocks(body);
                changed |= optimize_defer_bodies(&mut body.blocks);
            }
        }
    }
    changed
}

fn merge_empty_defer_jump_blocks(body: &mut FunctionDeferBody) -> bool {
    let mut any_changed = false;
    loop {
        let empty_jumps = empty_jump_targets(&body.blocks, Some(body.entry));
        if empty_jumps.is_empty() {
            break;
        }
        let mut changed = false;
        for block in &mut body.blocks {
            changed |= retarget_terminator(&mut block.terminator, &empty_jumps);
        }
        if !changed {
            break;
        }
        any_changed = true;
    }
    any_changed
}

fn remove_unreachable_defer_blocks(body: &mut FunctionDeferBody) -> bool {
    let reachable = reachable_defer_blocks(body);
    if reachable.len() == body.blocks.len() {
        return false;
    }
    body.blocks.retain(|block| reachable.contains(&block.id));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_function_ir::{
        FunctionBlock, FunctionScope, FunctionScopeId, FunctionTerminator, validate_function_body,
    };
    use nia_ids::LocalId;
    use nia_opt::OptimizationLevel;
    use nia_span::Span;

    #[test]
    fn o0_pipeline_has_no_optional_backend_passes() {
        let pipeline = BackendOptPipeline::for_policy(&OptimizationLevel::O0.policy());

        assert!(pipeline.passes.is_empty());
    }

    #[test]
    fn o1_pipeline_keeps_canonical_pass_order() {
        let pipeline = BackendOptPipeline::for_policy(&OptimizationLevel::O1.policy());

        assert_eq!(pipeline.passes, O1_PASSES);
    }

    #[test]
    fn removes_blocks_unreachable_from_entry() {
        let span = Span::default();
        let mut body = test_body(vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Next {
                    target: FunctionBlockId(1),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(1),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
            FunctionBlock {
                id: FunctionBlockId(2),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
        ]);

        remove_unreachable_blocks(&mut body);

        assert_eq!(
            body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
            vec![FunctionBlockId(0), FunctionBlockId(1)]
        );
        validate_function_body(&body).expect("optimized function body should remain valid");
    }

    #[test]
    fn preserves_blocks_referenced_by_reachable_loop_terminators() {
        let span = Span::default();
        let mut body = test_body(vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Loop {
                    header: nia_function_ir::FunctionForHeader::Infinite,
                    body: FunctionBlockId(1),
                    continue_target: FunctionBlockId(2),
                    break_target: FunctionBlockId(3),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(1),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Branch {
                    target: FunctionBlockId(2),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(2),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Branch {
                    target: FunctionBlockId(0),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(3),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
            FunctionBlock {
                id: FunctionBlockId(4),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
        ]);

        remove_unreachable_blocks(&mut body);

        assert_eq!(
            body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
            vec![
                FunctionBlockId(0),
                FunctionBlockId(1),
                FunctionBlockId(2),
                FunctionBlockId(3),
            ]
        );
        validate_function_body(&body).expect("optimized loop body should remain valid");
    }

    #[test]
    fn merges_empty_jump_blocks_within_the_same_scope() {
        let span = Span::default();
        let mut body = test_body(vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Next {
                    target: FunctionBlockId(1),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(1),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Next {
                    target: FunctionBlockId(2),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(2),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
        ]);

        merge_empty_jump_blocks(&mut body);
        remove_unreachable_blocks(&mut body);

        assert_eq!(
            body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
            vec![FunctionBlockId(0), FunctionBlockId(2)]
        );
        assert_eq!(
            body.blocks[0].terminator.successors(),
            vec![FunctionBlockId(2)]
        );
        validate_function_body(&body).expect("merged function body should remain valid");
    }

    #[test]
    fn folds_constant_bool_if_to_selected_branch() {
        let span = Span::default();
        let ty = test_ty();
        let mut body = test_body(vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::If {
                    cond: nia_function_ir::FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Bool(false),
                    },
                    then_target: FunctionBlockId(1),
                    else_target: FunctionBlockId(2),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(1),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
            FunctionBlock {
                id: FunctionBlockId(2),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
        ]);

        fold_constant_bool_branches(&mut body.blocks);
        remove_unreachable_blocks(&mut body);

        assert_eq!(
            body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
            vec![FunctionBlockId(0), FunctionBlockId(2)]
        );
        assert_eq!(
            body.blocks[0].terminator.successors(),
            vec![FunctionBlockId(2)]
        );
        validate_function_body(&body).expect("folded function body should remain valid");
    }

    #[test]
    fn removes_same_type_cast_wrappers_recursively() {
        let span = Span::default();
        let ty = test_ty();
        let mut expr = FunctionExpr {
            span,
            ty,
            kind: FunctionExprKind::Discard(Box::new(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Cast {
                    expr: Box::new(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Local(LocalId(0)),
                    }),
                    ty,
                },
            })),
        };

        simplify_same_type_casts_in_expr(&mut expr);

        let FunctionExprKind::Discard(inner) = expr.kind else {
            panic!("expected discard wrapper");
        };
        assert!(matches!(inner.kind, FunctionExprKind::Local(LocalId(0))));
    }

    #[test]
    fn preserves_casts_that_change_type() {
        let span = Span::default();
        let source_ty = test_ty();
        let target_ty = nia_ids::InternedTyId::new(
            nia_ids::ModuleId(0),
            nia_ids::TyInternerIndex::from_interner_index(1),
        );
        let mut expr = FunctionExpr {
            span,
            ty: target_ty,
            kind: FunctionExprKind::Cast {
                expr: Box::new(FunctionExpr {
                    span,
                    ty: source_ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                }),
                ty: target_ty,
            },
        };

        simplify_same_type_casts_in_expr(&mut expr);

        assert!(matches!(expr.kind, FunctionExprKind::Cast { .. }));
    }

    #[test]
    fn removes_noop_local_store_ops() {
        let span = Span::default();
        let ty = test_ty();
        let mut body = test_body(vec![FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: vec![
                FunctionOp::StoreLocal {
                    local_id: LocalId(0),
                    value: FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Local(LocalId(0)),
                    },
                    span,
                },
                FunctionOp::StoreLocal {
                    local_id: LocalId(0),
                    value: FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Local(LocalId(1)),
                    },
                    span,
                },
            ],
            terminator: FunctionTerminator::Return { value: None, span },
        }]);

        remove_noop_local_stores(&mut body.blocks);

        assert_eq!(body.blocks[0].ops.len(), 1);
        assert!(matches!(
            body.blocks[0].ops[0],
            FunctionOp::StoreLocal {
                local_id: LocalId(0),
                value: FunctionExpr {
                    kind: FunctionExprKind::Local(LocalId(1)),
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn removes_noop_local_store_after_same_type_cast_simplification() {
        let span = Span::default();
        let ty = test_ty();
        let mut body = test_body(vec![FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: vec![FunctionOp::StoreLocal {
                local_id: LocalId(0),
                value: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Cast {
                        expr: Box::new(FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Local(LocalId(0)),
                        }),
                        ty,
                    },
                },
                span,
            }],
            terminator: FunctionTerminator::Return { value: None, span },
        }]);

        simplify_same_type_casts_in_blocks(&mut body.blocks);
        remove_noop_local_stores(&mut body.blocks);

        assert!(body.blocks[0].ops.is_empty());
    }

    #[test]
    fn removes_pure_expr_ops_but_preserves_effectful_expr_ops() {
        let span = Span::default();
        let ty = test_ty();
        let mut body = test_body(vec![FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: vec![
                FunctionOp::Expr(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Discard(Box::new(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Local(LocalId(0)),
                    })),
                }),
                FunctionOp::Expr(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Call {
                        callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                            module_id: nia_ids::ModuleId(0),
                            def_id: nia_ids::DefId(0),
                        }),
                        args: Vec::new(),
                    },
                }),
            ],
            terminator: FunctionTerminator::Return { value: None, span },
        }]);

        remove_pure_expr_ops(&mut body.blocks);

        assert_eq!(body.blocks[0].ops.len(), 1);
        assert!(matches!(
            body.blocks[0].ops[0],
            FunctionOp::Expr(FunctionExpr {
                kind: FunctionExprKind::Call { .. },
                ..
            })
        ));
    }

    #[test]
    fn does_not_merge_entry_block_even_when_it_is_an_empty_jump() {
        let span = Span::default();
        let mut body = test_body(vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Next {
                    target: FunctionBlockId(1),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(1),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
        ]);

        merge_empty_jump_blocks(&mut body);
        remove_unreachable_blocks(&mut body);

        assert_eq!(
            body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
            vec![FunctionBlockId(0), FunctionBlockId(1)]
        );
        validate_function_body(&body).expect("entry-preserving merge should remain valid");
    }

    #[test]
    fn does_not_merge_empty_jump_blocks_across_scope_boundaries() {
        let span = Span::default();
        let mut body = test_body_with_scopes(
            vec![
                FunctionScope {
                    id: FunctionScopeId(0),
                    parent: None,
                    span,
                },
                FunctionScope {
                    id: FunctionScopeId(1),
                    parent: Some(FunctionScopeId(0)),
                    span,
                },
            ],
            vec![
                FunctionBlock {
                    id: FunctionBlockId(0),
                    scope: FunctionScopeId(0),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::Next {
                        target: FunctionBlockId(1),
                        span,
                    },
                },
                FunctionBlock {
                    id: FunctionBlockId(1),
                    scope: FunctionScopeId(1),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::Next {
                        target: FunctionBlockId(2),
                        span,
                    },
                },
                FunctionBlock {
                    id: FunctionBlockId(2),
                    scope: FunctionScopeId(0),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::Return { value: None, span },
                },
            ],
        );

        merge_empty_jump_blocks(&mut body);
        remove_unreachable_blocks(&mut body);

        assert_eq!(
            body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
            vec![FunctionBlockId(0), FunctionBlockId(1), FunctionBlockId(2)]
        );
        assert_eq!(
            body.edge_exited_scopes(FunctionBlockId(1), FunctionBlockId(2)),
            Some(vec![FunctionScopeId(1)])
        );
        validate_function_body(&body).expect("scope-preserving merge should remain valid");
    }

    fn test_body(blocks: Vec<FunctionBlock>) -> FunctionBody {
        test_body_with_scopes(
            vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span: Span::default(),
            }],
            blocks,
        )
    }

    fn test_body_with_scopes(
        scopes: Vec<FunctionScope>,
        blocks: Vec<FunctionBlock>,
    ) -> FunctionBody {
        FunctionBody {
            span: Span::default(),
            locals: Vec::new(),
            scopes,
            blocks,
            entry: FunctionBlockId(0),
            ty: test_ty(),
        }
    }

    fn test_ty() -> nia_ids::InternedTyId {
        nia_ids::InternedTyId::new(
            nia_ids::ModuleId(0),
            nia_ids::TyInternerIndex::from_interner_index(0),
        )
    }
}
