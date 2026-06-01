// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use crate::ModuleLowerer;
use nia_function_ir::{
    FunctionArrayElements, FunctionBlock, FunctionBlockId, FunctionBody, FunctionCallee,
    FunctionDeferBody, FunctionExpr, FunctionExprKind, FunctionFieldInit, FunctionForHeader,
    FunctionInlineAsm, FunctionLocalKind, FunctionOp, FunctionPlace, FunctionPlaceBase,
    FunctionPlaceElem, FunctionRange, FunctionSliceRange, FunctionTerminator,
};
use nia_ids::GlobalDefId;
use nia_ids::LocalId;
use nia_opt::{OptimizationDepth, OptimizationPolicy};

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn optimize_function_body(
        &mut self,
        function: GlobalDefId,
        is_instance: bool,
        type_arg_count: usize,
        mut body: FunctionBody,
    ) -> FunctionBody {
        for pass in BackendOptPipeline::for_policy(&self.optimization).run(&mut body) {
            self.optimization_report
                .changed_passes
                .push(crate::BackendOptimizationChange {
                    module_id: self.input.module_id,
                    function,
                    pass,
                    is_instance,
                    type_arg_count,
                });
        }
        body
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendOptPass {
    SimplifySameTypeCasts,
    RemoveNoopLocalStores,
    RemovePureExprOps,
    PropagateLocalCopies,
    RemoveOverwrittenLocalStores,
    RemoveNeverReadLocalStores,
    RemoveUnusedLocalBindings,
    FoldConstantBoolBranches,
    FoldConstantSwitches,
    SimplifyTrivialBranches,
    SimplifySameTargetSwitches,
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
            Self::PropagateLocalCopies => "propagate-local-copies",
            Self::RemoveOverwrittenLocalStores => "remove-overwritten-local-stores",
            Self::RemoveNeverReadLocalStores => "remove-never-read-local-stores",
            Self::RemoveUnusedLocalBindings => "remove-unused-local-bindings",
            Self::FoldConstantBoolBranches => "fold-constant-bool-branches",
            Self::FoldConstantSwitches => "fold-constant-switches",
            Self::SimplifyTrivialBranches => "simplify-trivial-branches",
            Self::SimplifySameTargetSwitches => "simplify-same-target-switches",
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
            Self::PropagateLocalCopies => propagate_local_copies(body),
            Self::RemoveOverwrittenLocalStores => remove_overwritten_local_stores(&mut body.blocks),
            Self::RemoveNeverReadLocalStores => remove_never_read_local_stores(body),
            Self::RemoveUnusedLocalBindings => remove_unused_local_bindings(body),
            Self::FoldConstantBoolBranches => fold_constant_bool_branches(&mut body.blocks),
            Self::FoldConstantSwitches => fold_constant_switches(&mut body.blocks),
            Self::SimplifyTrivialBranches => simplify_trivial_branches(&mut body.blocks),
            Self::SimplifySameTargetSwitches => simplify_same_target_switches(&mut body.blocks),
            Self::MergeEmptyJumpBlocks => merge_empty_jump_blocks(body),
            Self::RemoveUnreachableBlocks => remove_unreachable_blocks(body),
            Self::OptimizeDeferBodies => optimize_defer_bodies(&mut body.blocks),
        }
    }

    fn enabled_by(self, policy: &OptimizationPolicy) -> bool {
        match self {
            Self::SimplifySameTypeCasts | Self::RemoveNoopLocalStores | Self::RemovePureExprOps => {
                at_least(policy.dead_code_elim, OptimizationDepth::Cheap)
            }
            Self::PropagateLocalCopies => {
                at_least(policy.local_copy_prop, OptimizationDepth::Full)
                    || (policy.prefer_size
                        && at_least(policy.local_copy_prop, OptimizationDepth::Cheap))
            }
            Self::RemoveOverwrittenLocalStores
            | Self::RemoveNeverReadLocalStores
            | Self::RemoveUnusedLocalBindings => {
                at_least(policy.dead_code_elim, OptimizationDepth::Full)
            }
            Self::FoldConstantBoolBranches => at_least(policy.const_fold, OptimizationDepth::Cheap),
            Self::FoldConstantSwitches => {
                at_least(policy.const_fold, OptimizationDepth::Full)
                    && at_least(policy.simplify_cfg, OptimizationDepth::Full)
            }
            Self::SimplifyTrivialBranches
            | Self::MergeEmptyJumpBlocks
            | Self::RemoveUnreachableBlocks
            | Self::OptimizeDeferBodies => at_least(policy.simplify_cfg, OptimizationDepth::Cheap),
            Self::SimplifySameTargetSwitches => {
                at_least(policy.simplify_cfg, OptimizationDepth::Full)
            }
        }
    }
}

struct BackendOptPipeline {
    passes: Vec<BackendOptPass>,
}

impl BackendOptPipeline {
    fn for_policy(policy: &OptimizationPolicy) -> Self {
        let passes = ORDERED_BACKEND_PASSES
            .iter()
            .copied()
            .filter(|pass| pass.enabled_by(policy))
            .collect();
        Self { passes }
    }

    fn run(&self, body: &mut FunctionBody) -> Vec<&'static str> {
        let mut changed_passes = Vec::new();
        for pass in &self.passes {
            let name = pass.name();
            debug_assert!(!name.is_empty());
            if (*pass).run(body) {
                changed_passes.push(name);
            }
        }
        changed_passes
    }
}

const ORDERED_BACKEND_PASSES: &[BackendOptPass] = &[
    BackendOptPass::SimplifySameTypeCasts,
    BackendOptPass::RemoveNoopLocalStores,
    BackendOptPass::RemovePureExprOps,
    BackendOptPass::PropagateLocalCopies,
    BackendOptPass::RemoveOverwrittenLocalStores,
    BackendOptPass::RemoveNeverReadLocalStores,
    BackendOptPass::RemoveUnusedLocalBindings,
    BackendOptPass::FoldConstantBoolBranches,
    BackendOptPass::FoldConstantSwitches,
    BackendOptPass::SimplifyTrivialBranches,
    BackendOptPass::SimplifySameTargetSwitches,
    BackendOptPass::MergeEmptyJumpBlocks,
    BackendOptPass::RemoveUnreachableBlocks,
    BackendOptPass::OptimizeDeferBodies,
];

#[cfg(test)]
const O1_PASSES: &[BackendOptPass] = &[
    BackendOptPass::SimplifySameTypeCasts,
    BackendOptPass::RemoveNoopLocalStores,
    BackendOptPass::RemovePureExprOps,
    BackendOptPass::FoldConstantBoolBranches,
    BackendOptPass::SimplifyTrivialBranches,
    BackendOptPass::MergeEmptyJumpBlocks,
    BackendOptPass::RemoveUnreachableBlocks,
    BackendOptPass::OptimizeDeferBodies,
];

fn at_least(depth: OptimizationDepth, minimum: OptimizationDepth) -> bool {
    optimization_depth_rank(depth) >= optimization_depth_rank(minimum)
}

fn optimization_depth_rank(depth: OptimizationDepth) -> u8 {
    match depth {
        OptimizationDepth::Disabled => 0,
        OptimizationDepth::Required => 1,
        OptimizationDepth::Cheap => 2,
        OptimizationDepth::Full => 3,
        OptimizationDepth::Aggressive => 4,
    }
}

#[cfg(test)]
const O2_PASSES: &[BackendOptPass] = &[
    BackendOptPass::SimplifySameTypeCasts,
    BackendOptPass::RemoveNoopLocalStores,
    BackendOptPass::RemovePureExprOps,
    BackendOptPass::PropagateLocalCopies,
    BackendOptPass::RemoveOverwrittenLocalStores,
    BackendOptPass::RemoveNeverReadLocalStores,
    BackendOptPass::RemoveUnusedLocalBindings,
    BackendOptPass::FoldConstantBoolBranches,
    BackendOptPass::FoldConstantSwitches,
    BackendOptPass::SimplifyTrivialBranches,
    BackendOptPass::SimplifySameTargetSwitches,
    BackendOptPass::MergeEmptyJumpBlocks,
    BackendOptPass::RemoveUnreachableBlocks,
    BackendOptPass::OptimizeDeferBodies,
];

#[derive(Debug)]
struct FunctionCfg {
    blocks_by_id: HashMap<FunctionBlockId, usize>,
    predecessors: HashMap<FunctionBlockId, Vec<FunctionBlockId>>,
}

impl FunctionCfg {
    fn new(blocks: &[FunctionBlock]) -> Self {
        let blocks_by_id = blocks
            .iter()
            .enumerate()
            .map(|(index, block)| (block.id, index))
            .collect::<HashMap<_, _>>();
        let mut predecessors: HashMap<FunctionBlockId, Vec<FunctionBlockId>> = HashMap::new();
        for block in blocks {
            predecessors.entry(block.id).or_default();
            for target in terminator_referenced_blocks(&block.terminator) {
                if blocks_by_id.contains_key(&target) {
                    predecessors.entry(target).or_default().push(block.id);
                }
            }
        }
        Self {
            blocks_by_id,
            predecessors,
        }
    }

    fn block(&self, id: FunctionBlockId) -> Option<usize> {
        self.blocks_by_id.get(&id).copied()
    }

    fn predecessors(&self, id: FunctionBlockId) -> &[FunctionBlockId] {
        self.predecessors.get(&id).map(Vec::as_slice).unwrap_or(&[])
    }

    fn referenced_blocks(&self, terminator: &FunctionTerminator) -> Vec<FunctionBlockId> {
        terminator_referenced_blocks(terminator)
            .into_iter()
            .filter(|id| self.blocks_by_id.contains_key(id))
            .collect()
    }

    fn reachable_from(
        &self,
        blocks: &[FunctionBlock],
        entry: FunctionBlockId,
    ) -> HashSet<FunctionBlockId> {
        let mut reachable = HashSet::new();
        let mut stack = vec![entry];
        while let Some(id) = stack.pop() {
            if !reachable.insert(id) {
                continue;
            }
            let Some(index) = self.block(id) else {
                continue;
            };
            stack.extend(self.referenced_blocks(&blocks[index].terminator));
        }
        reachable
    }
}

#[derive(Debug)]
struct DeferCfg {
    blocks_by_id: HashMap<FunctionBlockId, usize>,
}

impl DeferCfg {
    fn new(blocks: &[FunctionBlock]) -> Self {
        Self {
            blocks_by_id: blocks
                .iter()
                .enumerate()
                .map(|(index, block)| (block.id, index))
                .collect(),
        }
    }

    fn block(&self, id: FunctionBlockId) -> Option<usize> {
        self.blocks_by_id.get(&id).copied()
    }

    fn referenced_blocks(&self, terminator: &FunctionTerminator) -> Vec<FunctionBlockId> {
        terminator_referenced_blocks(terminator)
            .into_iter()
            .filter(|id| self.blocks_by_id.contains_key(id))
            .collect()
    }

    fn reachable_from(
        &self,
        blocks: &[FunctionBlock],
        entry: FunctionBlockId,
    ) -> HashSet<FunctionBlockId> {
        let mut reachable = HashSet::new();
        let mut stack = vec![entry];
        while let Some(id) = stack.pop() {
            if !reachable.insert(id) {
                continue;
            }
            let Some(index) = self.block(id) else {
                continue;
            };
            stack.extend(self.referenced_blocks(&blocks[index].terminator));
        }
        reachable
    }
}

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

fn remove_unused_local_bindings(body: &mut FunctionBody) -> bool {
    let mut changed = false;
    while remove_unused_local_bindings_once(body) {
        changed = true;
    }
    changed
}

fn remove_unused_local_bindings_once(body: &mut FunctionBody) -> bool {
    let referenced_locals = collect_referenced_locals(body);
    let removable_locals = body
        .locals
        .iter()
        .filter(|local| matches!(local.kind, FunctionLocalKind::Binding))
        .filter(|local| !referenced_locals.contains(&local.id))
        .map(|local| local.id)
        .collect::<HashSet<_>>();
    if removable_locals.is_empty() {
        return false;
    }

    let mut changed = remove_unused_local_binding_ops(&mut body.blocks, &removable_locals);
    let before = body.locals.len();
    body.locals
        .retain(|local| !removable_locals.contains(&local.id));
    changed |= body.locals.len() != before;
    changed
}

fn remove_unused_local_binding_ops(
    blocks: &mut [FunctionBlock],
    removable_locals: &HashSet<LocalId>,
) -> bool {
    let mut changed = false;
    for block in blocks {
        let mut replacement_ops = Vec::with_capacity(block.ops.len());
        for op in block.ops.drain(..) {
            match op {
                FunctionOp::Binding(binding) if removable_locals.contains(&binding.local_id) => {
                    changed = true;
                    if let Some(value) = binding.value
                        && !is_pure_discardable_expr(&value)
                    {
                        replacement_ops.push(FunctionOp::Expr(value));
                    }
                }
                FunctionOp::Defer(mut body) => {
                    changed |= remove_unused_local_binding_ops(&mut body.blocks, removable_locals);
                    replacement_ops.push(FunctionOp::Defer(body));
                }
                other => replacement_ops.push(other),
            }
        }
        block.ops = replacement_ops;
    }
    changed
}

fn remove_overwritten_local_stores(blocks: &mut [FunctionBlock]) -> bool {
    let mut changed = false;
    for block in blocks {
        changed |= remove_overwritten_local_stores_in_block(block);
    }
    changed
}

fn remove_overwritten_local_stores_in_block(block: &mut FunctionBlock) -> bool {
    let mut changed = false;
    let mut replacement_ops = Vec::with_capacity(block.ops.len());
    let mut pending_stores = HashMap::<LocalId, usize>::new();
    for op in block.ops.drain(..) {
        let read_locals = collect_read_locals_in_current_op(&op);
        for local_id in read_locals {
            pending_stores.remove(&local_id);
        }

        match op {
            FunctionOp::StoreLocal {
                local_id,
                value,
                span,
            } => {
                if let Some(previous_index) = pending_stores.insert(local_id, replacement_ops.len())
                {
                    changed = true;
                    preserve_store_value_if_needed(&mut replacement_ops[previous_index]);
                }
                replacement_ops.push(Some(FunctionOp::StoreLocal {
                    local_id,
                    value,
                    span,
                }));
            }
            FunctionOp::Defer(mut body) => {
                changed |= remove_overwritten_local_stores(&mut body.blocks);
                pending_stores.clear();
                replacement_ops.push(Some(FunctionOp::Defer(body)));
            }
            other => replacement_ops.push(Some(other)),
        }
    }
    block.ops = replacement_ops.into_iter().flatten().collect();
    changed
}

fn preserve_store_value_if_needed(op: &mut Option<FunctionOp>) {
    let Some(FunctionOp::StoreLocal { value, .. }) = op.take() else {
        return;
    };
    if !is_pure_discardable_expr(&value) {
        *op = Some(FunctionOp::Expr(value));
    }
}

fn remove_never_read_local_stores(body: &mut FunctionBody) -> bool {
    let read_locals = collect_read_locals(body);
    remove_never_read_local_stores_in_blocks(&mut body.blocks, &read_locals)
}

fn remove_never_read_local_stores_in_blocks(
    blocks: &mut [FunctionBlock],
    read_locals: &HashSet<LocalId>,
) -> bool {
    let mut changed = false;
    for block in blocks {
        let mut replacement_ops = Vec::with_capacity(block.ops.len());
        for op in block.ops.drain(..) {
            match op {
                FunctionOp::StoreLocal {
                    local_id, value, ..
                } if !read_locals.contains(&local_id) => {
                    changed = true;
                    if !is_pure_discardable_expr(&value) {
                        replacement_ops.push(FunctionOp::Expr(value));
                    }
                }
                FunctionOp::Defer(mut body) => {
                    changed |=
                        remove_never_read_local_stores_in_blocks(&mut body.blocks, read_locals);
                    replacement_ops.push(FunctionOp::Defer(body));
                }
                other => replacement_ops.push(other),
            }
        }
        block.ops = replacement_ops;
    }
    changed
}

fn propagate_local_copies(body: &mut FunctionBody) -> bool {
    let local_tys = body
        .locals
        .iter()
        .map(|local| (local.id, local.ty))
        .collect::<HashMap<_, _>>();
    let unstable_locals = collect_place_locals_in_body(body);
    let cfg = FunctionCfg::new(&body.blocks);
    let mut changed = false;
    let mut input_copies = HashMap::<FunctionBlockId, HashMap<LocalId, LocalId>>::new();
    let mut stack = vec![body.entry];
    let mut visited = HashSet::new();
    while let Some(block_id) = stack.pop() {
        if !visited.insert(block_id) {
            continue;
        }
        let Some(index) = cfg.block(block_id) else {
            continue;
        };
        let mut copies = input_copies.remove(&block_id).unwrap_or_default();
        changed |= propagate_local_copies_in_block(
            &mut body.blocks[index],
            &local_tys,
            &unstable_locals,
            &mut copies,
        );
        for successor in cfg.referenced_blocks(&body.blocks[index].terminator) {
            let preds = cfg.predecessors(successor);
            if preds.len() == 1 && preds[0] == block_id {
                input_copies.insert(successor, copies.clone());
            }
            stack.push(successor);
        }
    }
    changed
}

fn propagate_local_copies_in_block(
    block: &mut FunctionBlock,
    local_tys: &HashMap<LocalId, nia_ids::InternedTyId>,
    unstable_locals: &HashSet<LocalId>,
    copies: &mut HashMap<LocalId, LocalId>,
) -> bool {
    let mut changed = false;
    for op in &mut block.ops {
        match op {
            FunctionOp::Binding(binding) => {
                if let Some(value) = &mut binding.value {
                    changed |= rewrite_local_copies_in_expr(value, copies);
                }
                invalidate_local_copy(copies, binding.local_id);
                if let Some(value) = &binding.value
                    && let Some(source) = copy_source_from_expr(value, copies)
                    && can_copy_local(binding.local_id, source, local_tys, unstable_locals)
                {
                    copies.insert(binding.local_id, source);
                }
            }
            FunctionOp::StoreLocal {
                local_id, value, ..
            } => {
                changed |= rewrite_local_copies_in_expr(value, copies);
                invalidate_local_copy(copies, *local_id);
                if let Some(source) = copy_source_from_expr(value, copies)
                    && can_copy_local(*local_id, source, local_tys, unstable_locals)
                {
                    copies.insert(*local_id, source);
                }
            }
            FunctionOp::Expr(_) | FunctionOp::Defer(_) => {
                copies.clear();
            }
        }
    }
    changed |= rewrite_local_copies_in_terminator(&mut block.terminator, copies);
    changed
}

fn copy_source_from_expr(
    expr: &FunctionExpr,
    copies: &HashMap<LocalId, LocalId>,
) -> Option<LocalId> {
    let FunctionExprKind::Local(local_id) = expr.kind else {
        return None;
    };
    Some(resolve_copy_source(local_id, copies))
}

fn can_copy_local(
    dest: LocalId,
    source: LocalId,
    local_tys: &HashMap<LocalId, nia_ids::InternedTyId>,
    unstable_locals: &HashSet<LocalId>,
) -> bool {
    dest != source
        && !unstable_locals.contains(&dest)
        && !unstable_locals.contains(&source)
        && local_tys.get(&dest).is_some()
        && local_tys.get(&dest) == local_tys.get(&source)
}

fn invalidate_local_copy(copies: &mut HashMap<LocalId, LocalId>, local_id: LocalId) {
    copies.remove(&local_id);
    copies.retain(|dest, source| *dest != local_id && *source != local_id);
}

fn rewrite_local_copies_in_terminator(
    terminator: &mut FunctionTerminator,
    copies: &HashMap<LocalId, LocalId>,
) -> bool {
    match terminator {
        FunctionTerminator::If { cond, .. } => rewrite_local_copies_in_expr(cond, copies),
        FunctionTerminator::Switch { target, arms, .. } => {
            let mut changed = rewrite_local_copies_in_expr(target, copies);
            for arm in arms {
                changed |= rewrite_local_copies_in_expr(&mut arm.pattern, copies);
            }
            changed
        }
        FunctionTerminator::Loop { header, .. } => match header {
            FunctionForHeader::Condition(cond) => rewrite_local_copies_in_expr(cond, copies),
            FunctionForHeader::CStyle { cond } => cond
                .as_deref_mut()
                .is_some_and(|cond| rewrite_local_copies_in_expr(cond, copies)),
            FunctionForHeader::Infinite => false,
        },
        FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => value
            .as_mut()
            .is_some_and(|value| rewrite_local_copies_in_expr(value, copies)),
        FunctionTerminator::Error { .. }
        | FunctionTerminator::Branch { .. }
        | FunctionTerminator::Next { .. } => false,
    }
}

fn rewrite_local_copies_in_expr(
    expr: &mut FunctionExpr,
    copies: &HashMap<LocalId, LocalId>,
) -> bool {
    match &mut expr.kind {
        FunctionExprKind::Local(local_id) => {
            let source = resolve_copy_source(*local_id, copies);
            if source == *local_id {
                false
            } else {
                *local_id = source;
                true
            }
        }
        FunctionExprKind::Range(range) => rewrite_local_copies_in_range(range, copies),
        FunctionExprKind::InlineAsm(asm) => rewrite_local_copies_in_inline_asm(asm, copies),
        FunctionExprKind::CStringPointer { array, .. } => {
            rewrite_local_copies_in_expr(array, copies)
        }
        FunctionExprKind::ArrayLiteral { elems } => {
            rewrite_local_copies_in_array_elements(elems, copies)
        }
        FunctionExprKind::StructLiteral { fields, .. } => {
            let mut changed = false;
            for field in fields {
                changed |= rewrite_local_copies_in_expr(&mut field.value, copies);
            }
            changed
        }
        FunctionExprKind::UnionLiteral { field, .. } => {
            rewrite_local_copies_in_expr(&mut field.value, copies)
        }
        FunctionExprKind::Unary { expr, .. }
        | FunctionExprKind::Discard(expr)
        | FunctionExprKind::Cast { expr, .. }
        | FunctionExprKind::TraitObjectUpcast { expr, .. }
        | FunctionExprKind::TraitObjectCoercion { expr, .. } => {
            rewrite_local_copies_in_expr(expr, copies)
        }
        FunctionExprKind::AddrOf(_) => false,
        FunctionExprKind::Binary { lhs, rhs, .. } => {
            rewrite_local_copies_in_expr(lhs, copies) | rewrite_local_copies_in_expr(rhs, copies)
        }
        FunctionExprKind::Assign { rhs, .. } => rewrite_local_copies_in_expr(rhs, copies),
        FunctionExprKind::Call { callee, args } => {
            let mut changed = rewrite_local_copies_in_callee(callee, copies);
            for arg in args {
                changed |= rewrite_local_copies_in_expr(arg, copies);
            }
            changed
        }
        FunctionExprKind::Field { lhs, .. } => rewrite_local_copies_in_expr(lhs, copies),
        FunctionExprKind::Index { lhs, index } => {
            rewrite_local_copies_in_expr(lhs, copies) | rewrite_local_copies_in_expr(index, copies)
        }
        FunctionExprKind::Slice { lhs, range, .. } => {
            rewrite_local_copies_in_expr(lhs, copies)
                | rewrite_local_copies_in_slice_range(range, copies)
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
        | FunctionExprKind::BuiltinValue(_) => false,
    }
}

fn rewrite_local_copies_in_inline_asm(
    asm: &mut FunctionInlineAsm,
    copies: &HashMap<LocalId, LocalId>,
) -> bool {
    let mut changed = false;
    for input in &mut asm.inputs {
        changed |= rewrite_local_copies_in_expr(&mut input.value, copies);
    }
    changed
}

fn rewrite_local_copies_in_array_elements(
    elems: &mut FunctionArrayElements,
    copies: &HashMap<LocalId, LocalId>,
) -> bool {
    match elems {
        FunctionArrayElements::List(elems) => {
            let mut changed = false;
            for elem in elems {
                changed |= rewrite_local_copies_in_expr(elem, copies);
            }
            changed
        }
        FunctionArrayElements::Repeat { value, .. } => rewrite_local_copies_in_expr(value, copies),
    }
}

fn rewrite_local_copies_in_callee(
    callee: &mut FunctionCallee,
    copies: &HashMap<LocalId, LocalId>,
) -> bool {
    match callee {
        FunctionCallee::Method { receiver, .. }
        | FunctionCallee::TraitMethod { receiver, .. }
        | FunctionCallee::DynamicTraitMethod { receiver, .. }
        | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
        | FunctionCallee::FunctionPointer(receiver) => {
            rewrite_local_copies_in_expr(receiver, copies)
        }
        FunctionCallee::Function(_)
        | FunctionCallee::FunctionInstance { .. }
        | FunctionCallee::BuiltinOperator(_) => false,
    }
}

fn rewrite_local_copies_in_slice_range(
    range: &mut FunctionSliceRange,
    copies: &HashMap<LocalId, LocalId>,
) -> bool {
    let mut changed = false;
    if let Some(start) = &mut range.start {
        changed |= rewrite_local_copies_in_expr(start, copies);
    }
    if let Some(end) = &mut range.end {
        changed |= rewrite_local_copies_in_expr(end, copies);
    }
    changed
}

fn rewrite_local_copies_in_range(
    range: &mut FunctionRange,
    copies: &HashMap<LocalId, LocalId>,
) -> bool {
    let mut changed = false;
    if let Some(start) = &mut range.start {
        changed |= rewrite_local_copies_in_expr(start, copies);
    }
    if let Some(end) = &mut range.end {
        changed |= rewrite_local_copies_in_expr(end, copies);
    }
    changed
}

fn resolve_copy_source(local_id: LocalId, copies: &HashMap<LocalId, LocalId>) -> LocalId {
    let mut current = local_id;
    let mut seen = HashSet::new();
    while let Some(next) = copies.get(&current).copied() {
        if !seen.insert(current) {
            break;
        }
        current = next;
    }
    current
}

fn collect_place_locals_in_body(body: &FunctionBody) -> HashSet<LocalId> {
    let mut locals = HashSet::new();
    collect_place_locals_in_blocks(&body.blocks, &mut locals);
    locals
}

fn collect_place_locals_in_blocks(blocks: &[FunctionBlock], locals: &mut HashSet<LocalId>) {
    for block in blocks {
        for op in &block.ops {
            collect_place_locals_in_op(op, locals);
        }
        collect_place_locals_in_terminator(&block.terminator, locals);
    }
}

fn collect_place_locals_in_op(op: &FunctionOp, locals: &mut HashSet<LocalId>) {
    match op {
        FunctionOp::Binding(binding) => {
            if let Some(value) = &binding.value {
                collect_place_locals_in_expr(value, locals);
            }
        }
        FunctionOp::StoreLocal {
            local_id, value, ..
        } => {
            locals.insert(*local_id);
            collect_place_locals_in_expr(value, locals);
        }
        FunctionOp::Expr(expr) => collect_place_locals_in_expr(expr, locals),
        FunctionOp::Defer(body) => collect_place_locals_in_blocks(&body.blocks, locals),
    }
}

fn collect_place_locals_in_terminator(
    terminator: &FunctionTerminator,
    locals: &mut HashSet<LocalId>,
) {
    match terminator {
        FunctionTerminator::If { cond, .. } => collect_place_locals_in_expr(cond, locals),
        FunctionTerminator::Switch { target, arms, .. } => {
            collect_place_locals_in_expr(target, locals);
            for arm in arms {
                collect_place_locals_in_expr(&arm.pattern, locals);
            }
        }
        FunctionTerminator::Loop { header, .. } => match header {
            FunctionForHeader::Condition(cond) => collect_place_locals_in_expr(cond, locals),
            FunctionForHeader::CStyle { cond } => {
                if let Some(cond) = cond {
                    collect_place_locals_in_expr(cond, locals);
                }
            }
            FunctionForHeader::Infinite => {}
        },
        FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
            if let Some(value) = value {
                collect_place_locals_in_expr(value, locals);
            }
        }
        FunctionTerminator::Error { .. }
        | FunctionTerminator::Branch { .. }
        | FunctionTerminator::Next { .. } => {}
    }
}

fn collect_place_locals_in_expr(expr: &FunctionExpr, locals: &mut HashSet<LocalId>) {
    match &expr.kind {
        FunctionExprKind::Range(range) => collect_place_locals_in_range(range, locals),
        FunctionExprKind::InlineAsm(asm) => collect_place_locals_in_inline_asm(asm, locals),
        FunctionExprKind::CStringPointer { array, .. } => {
            collect_place_locals_in_expr(array, locals)
        }
        FunctionExprKind::ArrayLiteral { elems } => {
            collect_place_locals_in_array_elements(elems, locals)
        }
        FunctionExprKind::StructLiteral { fields, .. } => {
            for field in fields {
                collect_place_locals_in_expr(&field.value, locals);
            }
        }
        FunctionExprKind::UnionLiteral { field, .. } => {
            collect_place_locals_in_expr(&field.value, locals)
        }
        FunctionExprKind::Unary { expr, .. }
        | FunctionExprKind::Discard(expr)
        | FunctionExprKind::Cast { expr, .. }
        | FunctionExprKind::TraitObjectUpcast { expr, .. }
        | FunctionExprKind::TraitObjectCoercion { expr, .. } => {
            collect_place_locals_in_expr(expr, locals);
        }
        FunctionExprKind::AddrOf(place) => collect_place_locals_in_place(place, locals),
        FunctionExprKind::Binary { lhs, rhs, .. } | FunctionExprKind::Index { lhs, index: rhs } => {
            collect_place_locals_in_expr(lhs, locals);
            collect_place_locals_in_expr(rhs, locals);
        }
        FunctionExprKind::Assign { place, rhs, .. } => {
            collect_place_locals_in_place(place, locals);
            collect_place_locals_in_expr(rhs, locals);
        }
        FunctionExprKind::Call { callee, args } => {
            collect_place_locals_in_callee(callee, locals);
            for arg in args {
                collect_place_locals_in_expr(arg, locals);
            }
        }
        FunctionExprKind::Field { lhs, .. } => collect_place_locals_in_expr(lhs, locals),
        FunctionExprKind::Slice { lhs, range, .. } => {
            collect_place_locals_in_expr(lhs, locals);
            collect_place_locals_in_slice_range(range, locals);
        }
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
        | FunctionExprKind::BuiltinValue(_) => {}
    }
}

fn collect_place_locals_in_inline_asm(asm: &FunctionInlineAsm, locals: &mut HashSet<LocalId>) {
    for input in &asm.inputs {
        collect_place_locals_in_expr(&input.value, locals);
    }
    for output in &asm.outputs {
        collect_place_locals_in_place(&output.place, locals);
    }
}

fn collect_place_locals_in_array_elements(
    elems: &FunctionArrayElements,
    locals: &mut HashSet<LocalId>,
) {
    match elems {
        FunctionArrayElements::List(elems) => {
            for elem in elems {
                collect_place_locals_in_expr(elem, locals);
            }
        }
        FunctionArrayElements::Repeat { value, .. } => collect_place_locals_in_expr(value, locals),
    }
}

fn collect_place_locals_in_callee(callee: &FunctionCallee, locals: &mut HashSet<LocalId>) {
    match callee {
        FunctionCallee::Method { receiver, .. }
        | FunctionCallee::TraitMethod { receiver, .. }
        | FunctionCallee::DynamicTraitMethod { receiver, .. }
        | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
        | FunctionCallee::FunctionPointer(receiver) => {
            collect_place_locals_in_expr(receiver, locals)
        }
        FunctionCallee::Function(_)
        | FunctionCallee::FunctionInstance { .. }
        | FunctionCallee::BuiltinOperator(_) => {}
    }
}

fn collect_place_locals_in_place(place: &FunctionPlace, locals: &mut HashSet<LocalId>) {
    if let FunctionPlaceBase::Local(local_id) = &place.base {
        locals.insert(*local_id);
    }
    for elem in &place.elems {
        if let FunctionPlaceElem::Index(index) = elem {
            collect_place_locals_in_expr(index, locals);
        }
    }
}

fn collect_place_locals_in_slice_range(range: &FunctionSliceRange, locals: &mut HashSet<LocalId>) {
    if let Some(start) = &range.start {
        collect_place_locals_in_expr(start, locals);
    }
    if let Some(end) = &range.end {
        collect_place_locals_in_expr(end, locals);
    }
}

fn collect_place_locals_in_range(range: &FunctionRange, locals: &mut HashSet<LocalId>) {
    if let Some(start) = &range.start {
        collect_place_locals_in_expr(start, locals);
    }
    if let Some(end) = &range.end {
        collect_place_locals_in_expr(end, locals);
    }
}

fn collect_read_locals(body: &FunctionBody) -> HashSet<LocalId> {
    let mut locals = HashSet::new();
    collect_read_locals_in_blocks(&body.blocks, &mut locals);
    locals
}

fn collect_read_locals_in_current_op(op: &FunctionOp) -> HashSet<LocalId> {
    let mut locals = HashSet::new();
    match op {
        FunctionOp::StoreLocal { value, .. } => collect_read_locals_in_expr(value, &mut locals),
        other => collect_read_locals_in_op(other, &mut locals),
    }
    locals
}

fn collect_read_locals_in_blocks(blocks: &[FunctionBlock], locals: &mut HashSet<LocalId>) {
    for block in blocks {
        for op in &block.ops {
            collect_read_locals_in_op(op, locals);
        }
        collect_read_locals_in_terminator(&block.terminator, locals);
    }
}

fn collect_read_locals_in_op(op: &FunctionOp, locals: &mut HashSet<LocalId>) {
    match op {
        FunctionOp::Binding(binding) => {
            if let Some(value) = &binding.value {
                collect_read_locals_in_expr(value, locals);
            }
        }
        FunctionOp::StoreLocal { value, .. } => collect_read_locals_in_expr(value, locals),
        FunctionOp::Expr(expr) => collect_read_locals_in_expr(expr, locals),
        FunctionOp::Defer(body) => collect_read_locals_in_blocks(&body.blocks, locals),
    }
}

fn collect_read_locals_in_terminator(
    terminator: &FunctionTerminator,
    locals: &mut HashSet<LocalId>,
) {
    match terminator {
        FunctionTerminator::If { cond, .. } => collect_read_locals_in_expr(cond, locals),
        FunctionTerminator::Switch { target, arms, .. } => {
            collect_read_locals_in_expr(target, locals);
            for arm in arms {
                collect_read_locals_in_expr(&arm.pattern, locals);
            }
        }
        FunctionTerminator::Loop { header, .. } => match header {
            FunctionForHeader::Condition(cond) => collect_read_locals_in_expr(cond, locals),
            FunctionForHeader::CStyle { cond } => {
                if let Some(cond) = cond {
                    collect_read_locals_in_expr(cond, locals);
                }
            }
            FunctionForHeader::Infinite => {}
        },
        FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
            if let Some(value) = value {
                collect_read_locals_in_expr(value, locals);
            }
        }
        FunctionTerminator::Error { .. }
        | FunctionTerminator::Branch { .. }
        | FunctionTerminator::Next { .. } => {}
    }
}

fn collect_read_locals_in_expr(expr: &FunctionExpr, locals: &mut HashSet<LocalId>) {
    match &expr.kind {
        FunctionExprKind::Local(local_id) => {
            locals.insert(*local_id);
        }
        FunctionExprKind::Range(range) => collect_read_locals_in_range(range, locals),
        FunctionExprKind::InlineAsm(asm) => collect_read_locals_in_inline_asm(asm, locals),
        FunctionExprKind::CStringPointer { array, .. } => {
            collect_read_locals_in_expr(array, locals)
        }
        FunctionExprKind::ArrayLiteral { elems } => {
            collect_read_locals_in_array_elements(elems, locals)
        }
        FunctionExprKind::StructLiteral { fields, .. } => {
            for field in fields {
                collect_read_locals_in_expr(&field.value, locals);
            }
        }
        FunctionExprKind::UnionLiteral { field, .. } => {
            collect_read_locals_in_expr(&field.value, locals)
        }
        FunctionExprKind::Unary { expr, .. }
        | FunctionExprKind::Discard(expr)
        | FunctionExprKind::Cast { expr, .. }
        | FunctionExprKind::TraitObjectUpcast { expr, .. }
        | FunctionExprKind::TraitObjectCoercion { expr, .. } => {
            collect_read_locals_in_expr(expr, locals);
        }
        FunctionExprKind::AddrOf(place) => collect_read_locals_in_place(place, locals),
        FunctionExprKind::Binary { lhs, rhs, .. } | FunctionExprKind::Index { lhs, index: rhs } => {
            collect_read_locals_in_expr(lhs, locals);
            collect_read_locals_in_expr(rhs, locals);
        }
        FunctionExprKind::Assign { place, rhs, .. } => {
            collect_read_locals_in_place(place, locals);
            collect_read_locals_in_expr(rhs, locals);
        }
        FunctionExprKind::Call { callee, args } => {
            collect_read_locals_in_callee(callee, locals);
            for arg in args {
                collect_read_locals_in_expr(arg, locals);
            }
        }
        FunctionExprKind::Field { lhs, .. } => collect_read_locals_in_expr(lhs, locals),
        FunctionExprKind::Slice { lhs, range, .. } => {
            collect_read_locals_in_expr(lhs, locals);
            collect_read_locals_in_slice_range(range, locals);
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
}

fn collect_read_locals_in_inline_asm(asm: &FunctionInlineAsm, locals: &mut HashSet<LocalId>) {
    for input in &asm.inputs {
        collect_read_locals_in_expr(&input.value, locals);
    }
    for output in &asm.outputs {
        collect_read_locals_in_place(&output.place, locals);
    }
}

fn collect_read_locals_in_array_elements(
    elems: &FunctionArrayElements,
    locals: &mut HashSet<LocalId>,
) {
    match elems {
        FunctionArrayElements::List(elems) => {
            for elem in elems {
                collect_read_locals_in_expr(elem, locals);
            }
        }
        FunctionArrayElements::Repeat { value, .. } => collect_read_locals_in_expr(value, locals),
    }
}

fn collect_read_locals_in_callee(callee: &FunctionCallee, locals: &mut HashSet<LocalId>) {
    match callee {
        FunctionCallee::Method { receiver, .. }
        | FunctionCallee::TraitMethod { receiver, .. }
        | FunctionCallee::DynamicTraitMethod { receiver, .. }
        | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
        | FunctionCallee::FunctionPointer(receiver) => {
            collect_read_locals_in_expr(receiver, locals)
        }
        FunctionCallee::Function(_)
        | FunctionCallee::FunctionInstance { .. }
        | FunctionCallee::BuiltinOperator(_) => {}
    }
}

fn collect_read_locals_in_place(place: &FunctionPlace, locals: &mut HashSet<LocalId>) {
    match &place.base {
        FunctionPlaceBase::Local(local_id) => {
            // Treat place bases as reads for now: this pass only deletes stores
            // whose target has no value/place use anywhere in the lowered body.
            locals.insert(*local_id);
        }
        FunctionPlaceBase::Deref(expr) => collect_read_locals_in_expr(expr, locals),
        FunctionPlaceBase::Global(_) | FunctionPlaceBase::Error => {}
    }
    for elem in &place.elems {
        if let FunctionPlaceElem::Index(index) = elem {
            collect_read_locals_in_expr(index, locals);
        }
    }
}

fn collect_read_locals_in_slice_range(range: &FunctionSliceRange, locals: &mut HashSet<LocalId>) {
    if let Some(start) = &range.start {
        collect_read_locals_in_expr(start, locals);
    }
    if let Some(end) = &range.end {
        collect_read_locals_in_expr(end, locals);
    }
}

fn collect_read_locals_in_range(range: &FunctionRange, locals: &mut HashSet<LocalId>) {
    if let Some(start) = &range.start {
        collect_read_locals_in_expr(start, locals);
    }
    if let Some(end) = &range.end {
        collect_read_locals_in_expr(end, locals);
    }
}

fn collect_referenced_locals(body: &FunctionBody) -> HashSet<LocalId> {
    let mut refs = HashSet::new();
    collect_referenced_locals_in_blocks(&body.blocks, &mut refs);
    refs
}

fn collect_referenced_locals_in_blocks(
    blocks: &[FunctionBlock],
    refs: &mut HashSet<nia_ids::LocalId>,
) {
    for block in blocks {
        for op in &block.ops {
            collect_referenced_locals_in_op(op, refs);
        }
        collect_referenced_locals_in_terminator(&block.terminator, refs);
    }
}

fn collect_referenced_locals_in_op(op: &FunctionOp, refs: &mut HashSet<nia_ids::LocalId>) {
    match op {
        FunctionOp::Binding(binding) => {
            if let Some(value) = &binding.value {
                collect_referenced_locals_in_expr(value, refs);
            }
        }
        FunctionOp::StoreLocal {
            local_id, value, ..
        } => {
            refs.insert(*local_id);
            collect_referenced_locals_in_expr(value, refs);
        }
        FunctionOp::Expr(expr) => collect_referenced_locals_in_expr(expr, refs),
        FunctionOp::Defer(body) => collect_referenced_locals_in_blocks(&body.blocks, refs),
    }
}

fn collect_referenced_locals_in_terminator(
    terminator: &FunctionTerminator,
    refs: &mut HashSet<nia_ids::LocalId>,
) {
    match terminator {
        FunctionTerminator::If { cond, .. } => collect_referenced_locals_in_expr(cond, refs),
        FunctionTerminator::Switch { target, arms, .. } => {
            collect_referenced_locals_in_expr(target, refs);
            for arm in arms {
                collect_referenced_locals_in_expr(&arm.pattern, refs);
            }
        }
        FunctionTerminator::Loop { header, .. } => match header {
            FunctionForHeader::Condition(cond) => collect_referenced_locals_in_expr(cond, refs),
            FunctionForHeader::CStyle { cond } => {
                if let Some(cond) = cond {
                    collect_referenced_locals_in_expr(cond, refs);
                }
            }
            FunctionForHeader::Infinite => {}
        },
        FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
            if let Some(value) = value {
                collect_referenced_locals_in_expr(value, refs);
            }
        }
        FunctionTerminator::Error { .. }
        | FunctionTerminator::Branch { .. }
        | FunctionTerminator::Next { .. } => {}
    }
}

fn collect_referenced_locals_in_expr(expr: &FunctionExpr, refs: &mut HashSet<nia_ids::LocalId>) {
    match &expr.kind {
        FunctionExprKind::Local(local_id) => {
            refs.insert(*local_id);
        }
        FunctionExprKind::Range(range) => collect_referenced_locals_in_range(range, refs),
        FunctionExprKind::InlineAsm(asm) => collect_referenced_locals_in_inline_asm(asm, refs),
        FunctionExprKind::CStringPointer { array, .. } => {
            collect_referenced_locals_in_expr(array, refs)
        }
        FunctionExprKind::ArrayLiteral { elems } => {
            collect_referenced_locals_in_array_elements(elems, refs)
        }
        FunctionExprKind::StructLiteral { fields, .. } => {
            for field in fields {
                collect_referenced_locals_in_expr(&field.value, refs);
            }
        }
        FunctionExprKind::UnionLiteral { field, .. } => {
            collect_referenced_locals_in_expr(&field.value, refs)
        }
        FunctionExprKind::Unary { expr, .. }
        | FunctionExprKind::Discard(expr)
        | FunctionExprKind::Cast { expr, .. }
        | FunctionExprKind::TraitObjectUpcast { expr, .. }
        | FunctionExprKind::TraitObjectCoercion { expr, .. } => {
            collect_referenced_locals_in_expr(expr, refs);
        }
        FunctionExprKind::AddrOf(place) => collect_referenced_locals_in_place(place, refs),
        FunctionExprKind::Binary { lhs, rhs, .. } => {
            collect_referenced_locals_in_expr(lhs, refs);
            collect_referenced_locals_in_expr(rhs, refs);
        }
        FunctionExprKind::Assign { place, rhs, .. } => {
            collect_referenced_locals_in_place(place, refs);
            collect_referenced_locals_in_expr(rhs, refs);
        }
        FunctionExprKind::Call { callee, args } => {
            collect_referenced_locals_in_callee(callee, refs);
            for arg in args {
                collect_referenced_locals_in_expr(arg, refs);
            }
        }
        FunctionExprKind::Field { lhs, .. } => collect_referenced_locals_in_expr(lhs, refs),
        FunctionExprKind::Index { lhs, index } => {
            collect_referenced_locals_in_expr(lhs, refs);
            collect_referenced_locals_in_expr(index, refs);
        }
        FunctionExprKind::Slice { lhs, range, .. } => {
            collect_referenced_locals_in_expr(lhs, refs);
            collect_referenced_locals_in_slice_range(range, refs);
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
}

fn collect_referenced_locals_in_inline_asm(
    asm: &FunctionInlineAsm,
    refs: &mut HashSet<nia_ids::LocalId>,
) {
    for input in &asm.inputs {
        collect_referenced_locals_in_expr(&input.value, refs);
    }
    for output in &asm.outputs {
        collect_referenced_locals_in_place(&output.place, refs);
    }
}

fn collect_referenced_locals_in_array_elements(
    elems: &FunctionArrayElements,
    refs: &mut HashSet<nia_ids::LocalId>,
) {
    match elems {
        FunctionArrayElements::List(elems) => {
            for elem in elems {
                collect_referenced_locals_in_expr(elem, refs);
            }
        }
        FunctionArrayElements::Repeat { value, .. } => {
            collect_referenced_locals_in_expr(value, refs)
        }
    }
}

fn collect_referenced_locals_in_callee(
    callee: &FunctionCallee,
    refs: &mut HashSet<nia_ids::LocalId>,
) {
    match callee {
        FunctionCallee::Method { receiver, .. }
        | FunctionCallee::TraitMethod { receiver, .. }
        | FunctionCallee::DynamicTraitMethod { receiver, .. }
        | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
        | FunctionCallee::FunctionPointer(receiver) => {
            collect_referenced_locals_in_expr(receiver, refs);
        }
        FunctionCallee::Function(_)
        | FunctionCallee::FunctionInstance { .. }
        | FunctionCallee::BuiltinOperator(_) => {}
    }
}

fn collect_referenced_locals_in_place(place: &FunctionPlace, refs: &mut HashSet<nia_ids::LocalId>) {
    match &place.base {
        FunctionPlaceBase::Local(local_id) => {
            refs.insert(*local_id);
        }
        FunctionPlaceBase::Deref(expr) => collect_referenced_locals_in_expr(expr, refs),
        FunctionPlaceBase::Global(_) | FunctionPlaceBase::Error => {}
    }
    for elem in &place.elems {
        if let FunctionPlaceElem::Index(index) = elem {
            collect_referenced_locals_in_expr(index, refs);
        }
    }
}

fn collect_referenced_locals_in_slice_range(
    range: &FunctionSliceRange,
    refs: &mut HashSet<nia_ids::LocalId>,
) {
    if let Some(start) = &range.start {
        collect_referenced_locals_in_expr(start, refs);
    }
    if let Some(end) = &range.end {
        collect_referenced_locals_in_expr(end, refs);
    }
}

fn collect_referenced_locals_in_range(range: &FunctionRange, refs: &mut HashSet<nia_ids::LocalId>) {
    if let Some(start) = &range.start {
        collect_referenced_locals_in_expr(start, refs);
    }
    if let Some(end) = &range.end {
        collect_referenced_locals_in_expr(end, refs);
    }
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
        for op in &mut block.ops {
            if let FunctionOp::Defer(body) = op {
                changed |= fold_constant_bool_branches(&mut body.blocks);
            }
        }
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

fn fold_constant_switches(blocks: &mut [FunctionBlock]) -> bool {
    let mut changed = false;
    for block in blocks {
        for op in &mut block.ops {
            if let FunctionOp::Defer(body) = op {
                changed |= fold_constant_switches(&mut body.blocks);
            }
        }
        if let FunctionTerminator::Switch {
            target,
            arms,
            default,
            fallback,
            span,
        } = &block.terminator
            && let Some(target_value) = switch_constant_value(target)
            && let Some(selected) = constant_switch_target(&target_value, arms, *default, *fallback)
        {
            block.terminator = FunctionTerminator::Branch {
                target: selected,
                span: *span,
            };
            changed = true;
        }
    }
    changed
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SwitchConstantValue {
    Integer(i128),
    Bool(bool),
    Char(u32),
    Byte(u8),
    EnumVariant(nia_ids::GlobalDefId),
}

fn constant_switch_target(
    target: &SwitchConstantValue,
    arms: &[nia_function_ir::FunctionSwitchArm],
    default: Option<FunctionBlockId>,
    fallback: FunctionBlockId,
) -> Option<FunctionBlockId> {
    let patterns = arms
        .iter()
        .map(|arm| switch_constant_value(&arm.pattern))
        .collect::<Option<Vec<_>>>()?;
    for (arm, pattern) in arms.iter().zip(patterns) {
        if pattern == *target {
            return Some(arm.target);
        }
    }
    Some(default.unwrap_or(fallback))
}

fn switch_constant_value(expr: &FunctionExpr) -> Option<SwitchConstantValue> {
    match &expr.kind {
        FunctionExprKind::Integer(text) => nia_comptime_engine::eval_int_literal(text)
            .ok()
            .map(SwitchConstantValue::Integer),
        FunctionExprKind::Bool(value) => Some(SwitchConstantValue::Bool(*value)),
        FunctionExprKind::Char(value) => Some(SwitchConstantValue::Char(*value)),
        FunctionExprKind::ByteChar(text) => {
            decode_byte_char_literal(text).map(SwitchConstantValue::Byte)
        }
        FunctionExprKind::EnumVariant(def_id) => Some(SwitchConstantValue::EnumVariant(*def_id)),
        FunctionExprKind::Error
        | FunctionExprKind::Float(_)
        | FunctionExprKind::String(_)
        | FunctionExprKind::ByteString(_)
        | FunctionExprKind::Local(_)
        | FunctionExprKind::Global(_)
        | FunctionExprKind::Function(_)
        | FunctionExprKind::FunctionInstance { .. }
        | FunctionExprKind::BuiltinValue(_)
        | FunctionExprKind::Discard(_)
        | FunctionExprKind::Range(_)
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
        | FunctionExprKind::Slice { .. } => None,
    }
}

fn decode_byte_char_literal(text: &str) -> Option<u8> {
    let inner = text.strip_prefix("b'")?.strip_suffix('\'')?;
    let mut chars = inner.chars();
    let ch = match chars.next()? {
        '\\' => match chars.next()? {
            'n' => b'\n',
            'r' => b'\r',
            't' => b'\t',
            '\\' => b'\\',
            '\'' => b'\'',
            '"' => b'"',
            '0' => b'\0',
            'x' => {
                let hi = chars.next()?.to_digit(16)?;
                let lo = chars.next()?.to_digit(16)?;
                ((hi << 4) | lo) as u8
            }
            _ => return None,
        },
        ch if ch.is_ascii() => ch as u8,
        _ => return None,
    };
    chars.next().is_none().then_some(ch)
}

fn simplify_trivial_branches(blocks: &mut [FunctionBlock]) -> bool {
    let mut changed = false;
    for block in blocks {
        if let FunctionTerminator::If {
            cond,
            then_target,
            else_target,
            span,
        } = &block.terminator
            && then_target == else_target
            && is_pure_discardable_expr(cond)
        {
            block.terminator = FunctionTerminator::Branch {
                target: *then_target,
                span: *span,
            };
            changed = true;
        }
    }
    changed
}

fn simplify_same_target_switches(blocks: &mut [FunctionBlock]) -> bool {
    let mut changed = false;
    for block in blocks {
        for op in &mut block.ops {
            if let FunctionOp::Defer(body) = op {
                changed |= simplify_same_target_switches(&mut body.blocks);
            }
        }
        if let FunctionTerminator::Switch {
            target,
            arms,
            default,
            fallback,
            span,
        } = &block.terminator
            && is_pure_discardable_expr(target)
            && arms
                .iter()
                .all(|arm| is_pure_discardable_expr(&arm.pattern))
            && switch_targets_same(arms, *default, *fallback)
        {
            block.terminator = FunctionTerminator::Branch {
                target: *fallback,
                span: *span,
            };
            changed = true;
        }
    }
    changed
}

fn switch_targets_same(
    arms: &[nia_function_ir::FunctionSwitchArm],
    default: Option<FunctionBlockId>,
    fallback: FunctionBlockId,
) -> bool {
    arms.iter().all(|arm| arm.target == fallback) && default.is_none_or(|target| target == fallback)
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
    let cfg = FunctionCfg::new(blocks);
    blocks
        .iter()
        .filter(|block| Some(block.id) != protected_entry && block.ops.is_empty())
        .filter(|block| !cfg.predecessors(block.id).is_empty())
        .filter_map(|block| {
            let target = jump_target(&block.terminator)?;
            let target_block = &blocks[cfg.block(target)?];
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
    FunctionCfg::new(&body.blocks).reachable_from(&body.blocks, body.entry)
}

fn reachable_defer_blocks(body: &FunctionDeferBody) -> HashSet<FunctionBlockId> {
    DeferCfg::new(&body.blocks).reachable_from(&body.blocks, body.entry)
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
                changed |= simplify_trivial_branches(&mut body.blocks);
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
    use nia_opt::NiaOptimizationLevel;
    use nia_span::Span;

    #[test]
    fn o0_pipeline_has_no_optional_backend_passes() {
        let pipeline = BackendOptPipeline::for_policy(&NiaOptimizationLevel::O0.policy());

        assert!(pipeline.passes.is_empty());
    }

    #[test]
    fn o1_pipeline_keeps_canonical_pass_order() {
        let pipeline = BackendOptPipeline::for_policy(&NiaOptimizationLevel::O1.policy());

        assert_eq!(pipeline.passes.as_slice(), O1_PASSES);
    }

    #[test]
    fn o2_family_pipeline_starts_from_o1_cleanup_passes() {
        for level in [
            NiaOptimizationLevel::O2,
            NiaOptimizationLevel::O3,
            NiaOptimizationLevel::Os,
            NiaOptimizationLevel::Oz,
        ] {
            let pipeline = BackendOptPipeline::for_policy(&level.policy());

            assert_eq!(pipeline.passes.as_slice(), O2_PASSES);
            for pass in O1_PASSES {
                assert!(pipeline.passes.contains(pass), "{level:?} missing {pass:?}");
            }
            assert!(
                pipeline
                    .passes
                    .contains(&BackendOptPass::PropagateLocalCopies)
            );
            assert!(
                pipeline
                    .passes
                    .contains(&BackendOptPass::RemoveUnusedLocalBindings)
            );
        }
    }

    #[test]
    fn backend_pipeline_is_selected_from_policy_capabilities() {
        let policy = nia_opt::OptimizationPolicy {
            level: NiaOptimizationLevel::O2,
            simplify_cfg: OptimizationDepth::Required,
            const_fold: OptimizationDepth::Cheap,
            dead_code_elim: OptimizationDepth::Full,
            local_copy_prop: OptimizationDepth::Disabled,
            inline_threshold: nia_opt::InlineThreshold::Never,
            specialize_generics: nia_opt::SpecializationPolicy::RequiredOnly,
            dedup_monomorphized_instances: false,
            prefer_size: false,
        };
        let pipeline = BackendOptPipeline::for_policy(&policy);

        assert_eq!(
            pipeline.passes,
            vec![
                BackendOptPass::SimplifySameTypeCasts,
                BackendOptPass::RemoveNoopLocalStores,
                BackendOptPass::RemovePureExprOps,
                BackendOptPass::RemoveOverwrittenLocalStores,
                BackendOptPass::RemoveNeverReadLocalStores,
                BackendOptPass::RemoveUnusedLocalBindings,
                BackendOptPass::FoldConstantBoolBranches,
            ]
        );
    }

    #[test]
    fn constant_bool_branch_folding_is_selected_from_const_fold_policy() {
        let policy = nia_opt::OptimizationPolicy {
            level: NiaOptimizationLevel::O2,
            simplify_cfg: OptimizationDepth::Cheap,
            const_fold: OptimizationDepth::Disabled,
            dead_code_elim: OptimizationDepth::Disabled,
            local_copy_prop: OptimizationDepth::Disabled,
            inline_threshold: nia_opt::InlineThreshold::Never,
            specialize_generics: nia_opt::SpecializationPolicy::RequiredOnly,
            dedup_monomorphized_instances: false,
            prefer_size: false,
        };
        let pipeline = BackendOptPipeline::for_policy(&policy);

        assert!(
            !pipeline
                .passes
                .contains(&BackendOptPass::FoldConstantBoolBranches)
        );
        assert!(
            pipeline
                .passes
                .contains(&BackendOptPass::SimplifyTrivialBranches)
        );

        let policy = nia_opt::OptimizationPolicy {
            simplify_cfg: OptimizationDepth::Required,
            const_fold: OptimizationDepth::Cheap,
            ..policy
        };
        let pipeline = BackendOptPipeline::for_policy(&policy);

        assert!(
            pipeline
                .passes
                .contains(&BackendOptPass::FoldConstantBoolBranches)
        );
        assert!(
            !pipeline
                .passes
                .contains(&BackendOptPass::SimplifyTrivialBranches)
        );
    }

    #[test]
    fn same_target_switch_simplification_requires_full_cfg_policy() {
        let policy = nia_opt::OptimizationPolicy {
            level: NiaOptimizationLevel::O2,
            simplify_cfg: OptimizationDepth::Cheap,
            const_fold: OptimizationDepth::Disabled,
            dead_code_elim: OptimizationDepth::Disabled,
            local_copy_prop: OptimizationDepth::Disabled,
            inline_threshold: nia_opt::InlineThreshold::Never,
            specialize_generics: nia_opt::SpecializationPolicy::RequiredOnly,
            dedup_monomorphized_instances: false,
            prefer_size: false,
        };
        let pipeline = BackendOptPipeline::for_policy(&policy);

        assert!(
            !pipeline
                .passes
                .contains(&BackendOptPass::SimplifySameTargetSwitches)
        );

        let policy = nia_opt::OptimizationPolicy {
            simplify_cfg: OptimizationDepth::Full,
            ..policy
        };
        let pipeline = BackendOptPipeline::for_policy(&policy);

        assert!(
            pipeline
                .passes
                .contains(&BackendOptPass::SimplifySameTargetSwitches)
        );
    }

    #[test]
    fn constant_switch_folding_requires_full_const_and_cfg_policy() {
        let policy = nia_opt::OptimizationPolicy {
            level: NiaOptimizationLevel::O2,
            simplify_cfg: OptimizationDepth::Full,
            const_fold: OptimizationDepth::Cheap,
            dead_code_elim: OptimizationDepth::Disabled,
            local_copy_prop: OptimizationDepth::Disabled,
            inline_threshold: nia_opt::InlineThreshold::Never,
            specialize_generics: nia_opt::SpecializationPolicy::RequiredOnly,
            dedup_monomorphized_instances: false,
            prefer_size: false,
        };
        let pipeline = BackendOptPipeline::for_policy(&policy);

        assert!(
            !pipeline
                .passes
                .contains(&BackendOptPass::FoldConstantSwitches)
        );

        let policy = nia_opt::OptimizationPolicy {
            const_fold: OptimizationDepth::Full,
            simplify_cfg: OptimizationDepth::Cheap,
            ..policy
        };
        let pipeline = BackendOptPipeline::for_policy(&policy);

        assert!(
            !pipeline
                .passes
                .contains(&BackendOptPass::FoldConstantSwitches)
        );

        let policy = nia_opt::OptimizationPolicy {
            const_fold: OptimizationDepth::Full,
            simplify_cfg: OptimizationDepth::Full,
            ..policy
        };
        let pipeline = BackendOptPipeline::for_policy(&policy);

        assert!(
            pipeline
                .passes
                .contains(&BackendOptPass::FoldConstantSwitches)
        );
    }

    #[test]
    fn propagates_local_copies_within_one_block() {
        let span = Span::default();
        let ty = test_ty();
        let mut body = test_body(vec![FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: vec![
                FunctionOp::Binding(nia_function_ir::FunctionBinding {
                    local_id: LocalId(0),
                    name: "source".to_string(),
                    ty,
                    value: Some(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Integer("1".to_string()),
                    }),
                    is_const: false,
                }),
                FunctionOp::Binding(nia_function_ir::FunctionBinding {
                    local_id: LocalId(1),
                    name: "copy".to_string(),
                    ty,
                    value: Some(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Local(LocalId(0)),
                    }),
                    is_const: false,
                }),
            ],
            terminator: FunctionTerminator::Tail {
                value: Some(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(1)),
                }),
                span,
            },
        }]);
        body.locals = vec![
            nia_function_ir::FunctionLocal {
                id: LocalId(0),
                name: "source".to_string(),
                kind: FunctionLocalKind::Binding,
                ty,
                span,
            },
            nia_function_ir::FunctionLocal {
                id: LocalId(1),
                name: "copy".to_string(),
                kind: FunctionLocalKind::Binding,
                ty,
                span,
            },
        ];

        assert!(propagate_local_copies(&mut body));

        let FunctionTerminator::Tail {
            value: Some(value), ..
        } = &body.blocks[0].terminator
        else {
            panic!("expected tail value");
        };
        assert!(matches!(value.kind, FunctionExprKind::Local(LocalId(0))));
        validate_function_body(&body).expect("copy-propagated body should remain valid");
    }

    #[test]
    fn does_not_propagate_locals_used_as_places() {
        let span = Span::default();
        let ty = test_ty();
        let mut body = test_body(vec![FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: vec![
                FunctionOp::Binding(nia_function_ir::FunctionBinding {
                    local_id: LocalId(0),
                    name: "source".to_string(),
                    ty,
                    value: Some(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Integer("1".to_string()),
                    }),
                    is_const: false,
                }),
                FunctionOp::Binding(nia_function_ir::FunctionBinding {
                    local_id: LocalId(1),
                    name: "copy".to_string(),
                    ty,
                    value: Some(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Local(LocalId(0)),
                    }),
                    is_const: false,
                }),
                FunctionOp::StoreLocal {
                    local_id: LocalId(1),
                    value: FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Integer("2".to_string()),
                    },
                    span,
                },
            ],
            terminator: FunctionTerminator::Tail {
                value: Some(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(1)),
                }),
                span,
            },
        }]);
        body.locals = vec![
            nia_function_ir::FunctionLocal {
                id: LocalId(0),
                name: "source".to_string(),
                kind: FunctionLocalKind::Binding,
                ty,
                span,
            },
            nia_function_ir::FunctionLocal {
                id: LocalId(1),
                name: "copy".to_string(),
                kind: FunctionLocalKind::Binding,
                ty,
                span,
            },
        ];

        assert!(!propagate_local_copies(&mut body));

        let FunctionTerminator::Tail {
            value: Some(value), ..
        } = &body.blocks[0].terminator
        else {
            panic!("expected tail value");
        };
        assert!(matches!(value.kind, FunctionExprKind::Local(LocalId(1))));
        validate_function_body(&body).expect("unpropagated body should remain valid");
    }

    #[test]
    fn removes_unused_local_bindings_to_fixed_point() {
        let span = Span::default();
        let ty = test_ty();
        let mut body = test_body(vec![FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: vec![
                FunctionOp::Binding(nia_function_ir::FunctionBinding {
                    local_id: LocalId(0),
                    name: "a".to_string(),
                    ty,
                    value: Some(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Integer("1".to_string()),
                    }),
                    is_const: false,
                }),
                FunctionOp::Binding(nia_function_ir::FunctionBinding {
                    local_id: LocalId(1),
                    name: "b".to_string(),
                    ty,
                    value: Some(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Local(LocalId(0)),
                    }),
                    is_const: false,
                }),
            ],
            terminator: FunctionTerminator::Return { value: None, span },
        }]);
        body.locals = vec![
            nia_function_ir::FunctionLocal {
                id: LocalId(0),
                name: "a".to_string(),
                kind: FunctionLocalKind::Binding,
                ty,
                span,
            },
            nia_function_ir::FunctionLocal {
                id: LocalId(1),
                name: "b".to_string(),
                kind: FunctionLocalKind::Binding,
                ty,
                span,
            },
        ];

        assert!(remove_unused_local_bindings(&mut body));

        assert!(body.locals.is_empty());
        assert!(body.blocks[0].ops.is_empty());
        validate_function_body(&body).expect("DCE body should remain valid");
    }

    #[test]
    fn preserves_effects_from_unused_local_binding_initializer() {
        let span = Span::default();
        let ty = test_ty();
        let mut body = test_body(vec![FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: vec![FunctionOp::Binding(nia_function_ir::FunctionBinding {
                local_id: LocalId(0),
                name: "unused".to_string(),
                ty,
                value: Some(FunctionExpr {
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
                is_const: false,
            })],
            terminator: FunctionTerminator::Return { value: None, span },
        }]);
        body.locals = vec![nia_function_ir::FunctionLocal {
            id: LocalId(0),
            name: "unused".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        }];

        assert!(remove_unused_local_bindings(&mut body));

        assert!(body.locals.is_empty());
        assert!(matches!(
            body.blocks[0].ops.as_slice(),
            [FunctionOp::Expr(FunctionExpr {
                kind: FunctionExprKind::Call { .. },
                ..
            })]
        ));
        validate_function_body(&body).expect("effect-preserving DCE body should remain valid");
    }

    #[test]
    fn removes_never_read_local_store_with_pure_value() {
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
                    kind: FunctionExprKind::Integer("1".to_string()),
                },
                span,
            }],
            terminator: FunctionTerminator::Return { value: None, span },
        }]);
        body.locals = vec![nia_function_ir::FunctionLocal {
            id: LocalId(0),
            name: "unused".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        }];

        assert!(remove_never_read_local_stores(&mut body));

        assert!(body.blocks[0].ops.is_empty());
        validate_function_body(&body).expect("dead-store body should remain valid");
    }

    #[test]
    fn preserves_effects_from_never_read_local_store_value() {
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
                    kind: FunctionExprKind::Call {
                        callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                            module_id: nia_ids::ModuleId(0),
                            def_id: nia_ids::DefId(0),
                        }),
                        args: Vec::new(),
                    },
                },
                span,
            }],
            terminator: FunctionTerminator::Return { value: None, span },
        }]);
        body.locals = vec![nia_function_ir::FunctionLocal {
            id: LocalId(0),
            name: "unused".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        }];

        assert!(remove_never_read_local_stores(&mut body));

        assert!(matches!(
            body.blocks[0].ops.as_slice(),
            [FunctionOp::Expr(FunctionExpr {
                kind: FunctionExprKind::Call { .. },
                ..
            })]
        ));
        validate_function_body(&body)
            .expect("effect-preserving dead-store body should remain valid");
    }

    #[test]
    fn preserves_stores_to_read_locals() {
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
                    kind: FunctionExprKind::Integer("1".to_string()),
                },
                span,
            }],
            terminator: FunctionTerminator::Tail {
                value: Some(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                }),
                span,
            },
        }]);
        body.locals = vec![nia_function_ir::FunctionLocal {
            id: LocalId(0),
            name: "used".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        }];

        assert!(!remove_never_read_local_stores(&mut body));

        assert!(matches!(
            body.blocks[0].ops.as_slice(),
            [FunctionOp::StoreLocal {
                local_id: LocalId(0),
                ..
            }]
        ));
        validate_function_body(&body).expect("preserved-store body should remain valid");
    }

    #[test]
    fn removes_local_store_overwritten_before_read() {
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
                        kind: FunctionExprKind::Integer("1".to_string()),
                    },
                    span,
                },
                FunctionOp::StoreLocal {
                    local_id: LocalId(0),
                    value: FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Integer("2".to_string()),
                    },
                    span,
                },
            ],
            terminator: FunctionTerminator::Tail {
                value: Some(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                }),
                span,
            },
        }]);
        body.locals = vec![nia_function_ir::FunctionLocal {
            id: LocalId(0),
            name: "target".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        }];

        assert!(remove_overwritten_local_stores(&mut body.blocks));

        assert!(matches!(
            body.blocks[0].ops.as_slice(),
            [FunctionOp::StoreLocal {
                local_id: LocalId(0),
                value: FunctionExpr {
                    kind: FunctionExprKind::Integer(value),
                    ..
                },
                ..
            }] if value == "2"
        ));
        validate_function_body(&body).expect("overwritten-store body should remain valid");
    }

    #[test]
    fn preserves_effects_from_overwritten_local_store_value() {
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
                        kind: FunctionExprKind::Call {
                            callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                                module_id: nia_ids::ModuleId(0),
                                def_id: nia_ids::DefId(0),
                            }),
                            args: Vec::new(),
                        },
                    },
                    span,
                },
                FunctionOp::StoreLocal {
                    local_id: LocalId(0),
                    value: FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Integer("2".to_string()),
                    },
                    span,
                },
            ],
            terminator: FunctionTerminator::Tail {
                value: Some(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                }),
                span,
            },
        }]);
        body.locals = vec![nia_function_ir::FunctionLocal {
            id: LocalId(0),
            name: "target".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        }];

        assert!(remove_overwritten_local_stores(&mut body.blocks));

        assert!(matches!(
            body.blocks[0].ops.as_slice(),
            [
                FunctionOp::Expr(FunctionExpr {
                    kind: FunctionExprKind::Call { .. },
                    ..
                }),
                FunctionOp::StoreLocal {
                    local_id: LocalId(0),
                    ..
                },
            ]
        ));
        validate_function_body(&body)
            .expect("effect-preserving overwritten-store body should remain valid");
    }

    #[test]
    fn preserves_local_store_read_before_overwrite() {
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
                        kind: FunctionExprKind::Integer("1".to_string()),
                    },
                    span,
                },
                FunctionOp::Expr(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                }),
                FunctionOp::StoreLocal {
                    local_id: LocalId(0),
                    value: FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Integer("2".to_string()),
                    },
                    span,
                },
            ],
            terminator: FunctionTerminator::Tail {
                value: Some(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                }),
                span,
            },
        }]);
        body.locals = vec![nia_function_ir::FunctionLocal {
            id: LocalId(0),
            name: "target".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        }];

        assert!(!remove_overwritten_local_stores(&mut body.blocks));

        assert_eq!(
            body.blocks[0]
                .ops
                .iter()
                .filter(|op| matches!(op, FunctionOp::StoreLocal { .. }))
                .count(),
            2
        );
        validate_function_body(&body).expect("read-before-overwrite body should remain valid");
    }

    #[test]
    fn cfg_indexes_blocks_and_structural_predecessors() {
        let span = Span::default();
        let body = test_body(vec![
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
        ]);

        let cfg = FunctionCfg::new(&body.blocks);

        assert_eq!(cfg.block(FunctionBlockId(2)), Some(2));
        assert_eq!(
            cfg.predecessors(FunctionBlockId(2)),
            &[FunctionBlockId(0), FunctionBlockId(1)]
        );
        assert_eq!(
            cfg.reachable_from(&body.blocks, body.entry),
            HashSet::from([
                FunctionBlockId(0),
                FunctionBlockId(1),
                FunctionBlockId(2),
                FunctionBlockId(3),
            ])
        );
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
    fn folds_constant_bool_branches_inside_defer_bodies() {
        let span = Span::default();
        let ty = test_ty();
        let mut body = test_body(vec![FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: vec![FunctionOp::Defer(nia_function_ir::FunctionDeferBody {
                span,
                scopes: vec![FunctionScope {
                    id: FunctionScopeId(0),
                    parent: None,
                    span,
                }],
                blocks: vec![
                    FunctionBlock {
                        id: FunctionBlockId(10),
                        scope: FunctionScopeId(0),
                        span,
                        ops: Vec::new(),
                        terminator: FunctionTerminator::If {
                            cond: FunctionExpr {
                                span,
                                ty,
                                kind: FunctionExprKind::Bool(true),
                            },
                            then_target: FunctionBlockId(11),
                            else_target: FunctionBlockId(12),
                            span,
                        },
                    },
                    FunctionBlock {
                        id: FunctionBlockId(11),
                        scope: FunctionScopeId(0),
                        span,
                        ops: Vec::new(),
                        terminator: FunctionTerminator::Error { span },
                    },
                    FunctionBlock {
                        id: FunctionBlockId(12),
                        scope: FunctionScopeId(0),
                        span,
                        ops: Vec::new(),
                        terminator: FunctionTerminator::Error { span },
                    },
                ],
                entry: FunctionBlockId(10),
            })],
            terminator: FunctionTerminator::Return { value: None, span },
        }]);

        assert!(fold_constant_bool_branches(&mut body.blocks));

        let FunctionOp::Defer(defer_body) = &body.blocks[0].ops[0] else {
            panic!("expected defer op");
        };
        assert!(matches!(
            defer_body.blocks[0].terminator,
            FunctionTerminator::Branch {
                target: FunctionBlockId(11),
                ..
            }
        ));
        validate_function_body(&body).expect("folded defer body should remain valid");
    }

    #[test]
    fn simplifies_same_target_if_with_pure_condition() {
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
                        kind: FunctionExprKind::Local(LocalId(0)),
                    },
                    then_target: FunctionBlockId(1),
                    else_target: FunctionBlockId(1),
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
        body.locals = vec![nia_function_ir::FunctionLocal {
            id: LocalId(0),
            name: "cond".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        }];

        assert!(simplify_trivial_branches(&mut body.blocks));

        assert_eq!(
            body.blocks[0].terminator.successors(),
            vec![FunctionBlockId(1)]
        );
        assert!(matches!(
            body.blocks[0].terminator,
            FunctionTerminator::Branch {
                target: FunctionBlockId(1),
                ..
            }
        ));
        validate_function_body(&body).expect("trivial-branch body should remain valid");
    }

    #[test]
    fn preserves_same_target_if_with_effectful_condition() {
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
                        kind: FunctionExprKind::Call {
                            callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                                module_id: nia_ids::ModuleId(0),
                                def_id: nia_ids::DefId(0),
                            }),
                            args: Vec::new(),
                        },
                    },
                    then_target: FunctionBlockId(1),
                    else_target: FunctionBlockId(1),
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

        assert!(!simplify_trivial_branches(&mut body.blocks));

        assert!(matches!(
            body.blocks[0].terminator,
            FunctionTerminator::If {
                then_target: FunctionBlockId(1),
                else_target: FunctionBlockId(1),
                ..
            }
        ));
        validate_function_body(&body).expect("effectful-condition body should remain valid");
    }

    #[test]
    fn simplifies_same_target_switch_with_pure_target() {
        let span = Span::default();
        let ty = test_ty();
        let mut body = test_body(vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Switch {
                    target: FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Local(LocalId(0)),
                    },
                    arms: vec![
                        nia_function_ir::FunctionSwitchArm {
                            pattern: FunctionExpr {
                                span,
                                ty,
                                kind: FunctionExprKind::Integer("1".to_string()),
                            },
                            target: FunctionBlockId(1),
                        },
                        nia_function_ir::FunctionSwitchArm {
                            pattern: FunctionExpr {
                                span,
                                ty,
                                kind: FunctionExprKind::Integer("2".to_string()),
                            },
                            target: FunctionBlockId(1),
                        },
                    ],
                    default: Some(FunctionBlockId(1)),
                    fallback: FunctionBlockId(1),
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
        body.locals = vec![nia_function_ir::FunctionLocal {
            id: LocalId(0),
            name: "target".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        }];

        assert!(simplify_same_target_switches(&mut body.blocks));

        assert!(matches!(
            body.blocks[0].terminator,
            FunctionTerminator::Branch {
                target: FunctionBlockId(1),
                ..
            }
        ));
        validate_function_body(&body).expect("same-target switch body should remain valid");
    }

    #[test]
    fn folds_constant_switch_to_matching_arm() {
        let span = Span::default();
        let ty = test_ty();
        let mut body = test_body(vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Switch {
                    target: FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Integer("0x2".to_string()),
                    },
                    arms: vec![
                        nia_function_ir::FunctionSwitchArm {
                            pattern: FunctionExpr {
                                span,
                                ty,
                                kind: FunctionExprKind::Integer("1".to_string()),
                            },
                            target: FunctionBlockId(1),
                        },
                        nia_function_ir::FunctionSwitchArm {
                            pattern: FunctionExpr {
                                span,
                                ty,
                                kind: FunctionExprKind::Integer("2".to_string()),
                            },
                            target: FunctionBlockId(2),
                        },
                    ],
                    default: Some(FunctionBlockId(3)),
                    fallback: FunctionBlockId(4),
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

        assert!(fold_constant_switches(&mut body.blocks));

        assert!(matches!(
            body.blocks[0].terminator,
            FunctionTerminator::Branch {
                target: FunctionBlockId(2),
                ..
            }
        ));
        validate_function_body(&body).expect("constant switch body should remain valid");
    }

    #[test]
    fn folds_constant_switch_to_default_or_fallback() {
        let span = Span::default();
        let ty = test_ty();
        let mut with_default = test_body(vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Switch {
                    target: FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Bool(false),
                    },
                    arms: vec![nia_function_ir::FunctionSwitchArm {
                        pattern: FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Bool(true),
                        },
                        target: FunctionBlockId(1),
                    }],
                    default: Some(FunctionBlockId(2)),
                    fallback: FunctionBlockId(3),
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
            FunctionBlock {
                id: FunctionBlockId(3),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
        ]);

        assert!(fold_constant_switches(&mut with_default.blocks));
        assert!(matches!(
            with_default.blocks[0].terminator,
            FunctionTerminator::Branch {
                target: FunctionBlockId(2),
                ..
            }
        ));

        let mut without_default = test_body(vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Switch {
                    target: FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Char('b' as u32),
                    },
                    arms: vec![nia_function_ir::FunctionSwitchArm {
                        pattern: FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Char('a' as u32),
                        },
                        target: FunctionBlockId(1),
                    }],
                    default: None,
                    fallback: FunctionBlockId(2),
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

        assert!(fold_constant_switches(&mut without_default.blocks));
        assert!(matches!(
            without_default.blocks[0].terminator,
            FunctionTerminator::Branch {
                target: FunctionBlockId(2),
                ..
            }
        ));
    }

    #[test]
    fn preserves_constant_switch_when_any_pattern_is_not_constant() {
        let span = Span::default();
        let ty = test_ty();
        let mut body = test_body(vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Switch {
                    target: FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Integer("2".to_string()),
                    },
                    arms: vec![
                        nia_function_ir::FunctionSwitchArm {
                            pattern: FunctionExpr {
                                span,
                                ty,
                                kind: FunctionExprKind::Integer("1".to_string()),
                            },
                            target: FunctionBlockId(1),
                        },
                        nia_function_ir::FunctionSwitchArm {
                            pattern: FunctionExpr {
                                span,
                                ty,
                                kind: FunctionExprKind::Call {
                                    callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                                        module_id: nia_ids::ModuleId(0),
                                        def_id: nia_ids::DefId(0),
                                    }),
                                    args: Vec::new(),
                                },
                            },
                            target: FunctionBlockId(2),
                        },
                    ],
                    default: Some(FunctionBlockId(3)),
                    fallback: FunctionBlockId(3),
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
            FunctionBlock {
                id: FunctionBlockId(3),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
        ]);

        assert!(!fold_constant_switches(&mut body.blocks));

        assert!(matches!(
            body.blocks[0].terminator,
            FunctionTerminator::Switch { .. }
        ));
        validate_function_body(&body).expect("unfolded switch body should remain valid");
    }

    #[test]
    fn preserves_same_target_switch_with_effectful_target() {
        let span = Span::default();
        let ty = test_ty();
        let mut body = test_body(vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Switch {
                    target: FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Call {
                            callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                                module_id: nia_ids::ModuleId(0),
                                def_id: nia_ids::DefId(0),
                            }),
                            args: Vec::new(),
                        },
                    },
                    arms: vec![nia_function_ir::FunctionSwitchArm {
                        pattern: FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Integer("1".to_string()),
                        },
                        target: FunctionBlockId(1),
                    }],
                    default: Some(FunctionBlockId(1)),
                    fallback: FunctionBlockId(1),
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

        assert!(!simplify_same_target_switches(&mut body.blocks));

        assert!(matches!(
            body.blocks[0].terminator,
            FunctionTerminator::Switch {
                fallback: FunctionBlockId(1),
                ..
            }
        ));
        validate_function_body(&body).expect("effectful-target switch body should remain valid");
    }

    #[test]
    fn preserves_same_target_switch_with_effectful_pattern() {
        let span = Span::default();
        let ty = test_ty();
        let mut body = test_body(vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Switch {
                    target: FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Local(LocalId(0)),
                    },
                    arms: vec![nia_function_ir::FunctionSwitchArm {
                        pattern: FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Call {
                                callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                                    module_id: nia_ids::ModuleId(0),
                                    def_id: nia_ids::DefId(0),
                                }),
                                args: Vec::new(),
                            },
                        },
                        target: FunctionBlockId(1),
                    }],
                    default: Some(FunctionBlockId(1)),
                    fallback: FunctionBlockId(1),
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
        body.locals = vec![nia_function_ir::FunctionLocal {
            id: LocalId(0),
            name: "target".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        }];

        assert!(!simplify_same_target_switches(&mut body.blocks));

        assert!(matches!(
            body.blocks[0].terminator,
            FunctionTerminator::Switch {
                fallback: FunctionBlockId(1),
                ..
            }
        ));
        validate_function_body(&body).expect("effectful-pattern switch body should remain valid");
    }

    #[test]
    fn simplifies_same_target_switches_inside_defer_bodies() {
        let span = Span::default();
        let ty = test_ty();
        let mut body = test_body(vec![FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: vec![FunctionOp::Defer(nia_function_ir::FunctionDeferBody {
                span,
                scopes: vec![FunctionScope {
                    id: FunctionScopeId(0),
                    parent: None,
                    span,
                }],
                blocks: vec![
                    FunctionBlock {
                        id: FunctionBlockId(10),
                        scope: FunctionScopeId(0),
                        span,
                        ops: Vec::new(),
                        terminator: FunctionTerminator::Switch {
                            target: FunctionExpr {
                                span,
                                ty,
                                kind: FunctionExprKind::Local(LocalId(0)),
                            },
                            arms: vec![nia_function_ir::FunctionSwitchArm {
                                pattern: FunctionExpr {
                                    span,
                                    ty,
                                    kind: FunctionExprKind::Integer("1".to_string()),
                                },
                                target: FunctionBlockId(11),
                            }],
                            default: Some(FunctionBlockId(11)),
                            fallback: FunctionBlockId(11),
                            span,
                        },
                    },
                    FunctionBlock {
                        id: FunctionBlockId(11),
                        scope: FunctionScopeId(0),
                        span,
                        ops: Vec::new(),
                        terminator: FunctionTerminator::Error { span },
                    },
                ],
                entry: FunctionBlockId(10),
            })],
            terminator: FunctionTerminator::Return { value: None, span },
        }]);
        body.locals = vec![nia_function_ir::FunctionLocal {
            id: LocalId(0),
            name: "target".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        }];

        assert!(simplify_same_target_switches(&mut body.blocks));

        let FunctionOp::Defer(defer_body) = &body.blocks[0].ops[0] else {
            panic!("expected defer op");
        };
        assert!(matches!(
            defer_body.blocks[0].terminator,
            FunctionTerminator::Branch {
                target: FunctionBlockId(11),
                ..
            }
        ));
        validate_function_body(&body).expect("defer switch body should remain valid");
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
