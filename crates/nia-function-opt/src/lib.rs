// SPDX-License-Identifier: GPL-3.0-or-later
use nia_function_ir::FunctionBody;
use nia_ids::InternedTyId;
use nia_opt::{OptimizationDepth, OptimizationPolicy};

mod passes;
#[cfg(test)]
mod tests;

use passes::*;

pub fn enabled_function_passes(policy: &OptimizationPolicy) -> Vec<&'static str> {
    FunctionOptPipeline::for_policy(policy)
        .passes
        .iter()
        .map(|pass| pass.name())
        .collect()
}

pub struct FunctionOptInput<'a, F> {
    pub body: FunctionBody,
    pub policy: &'a OptimizationPolicy,
    pub is_zero_sized: F,
}

pub struct FunctionOptOutput {
    pub body: FunctionBody,
    pub changed_passes: Vec<&'static str>,
}

pub fn optimize_function_body<F>(input: FunctionOptInput<'_, F>) -> FunctionOptOutput
where
    F: Fn(InternedTyId) -> bool + Copy,
{
    let mut body = input.body;
    let changed_passes =
        FunctionOptPipeline::for_policy(input.policy).run(&mut body, input.is_zero_sized);
    FunctionOptOutput {
        body,
        changed_passes,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FunctionOptPass {
    SimplifySameTypeCasts,
    RemoveNoopLocalStores,
    RemovePureExprOps,
    RemoveZstLocalRuntimeOps,
    PropagateLocalCopies,
    PropagateLocalConstants,
    SimplifyConstantLogicalExprs,
    RemoveOverwrittenLocalStores,
    RemoveNeverReadLocalStores,
    RemoveUnusedTempBindings,
    RemoveUnusedLocalBindings,
    FoldConstantBoolBranches,
    FoldConstantSwitches,
    SimplifyTrivialBranches,
    SimplifySameTargetSwitches,
    MergeEmptyJumpBlocks,
    RemoveUnreachableBlocks,
    OptimizeDeferBodies,
}

impl FunctionOptPass {
    fn name(self) -> &'static str {
        match self {
            Self::SimplifySameTypeCasts => "simplify-same-type-casts",
            Self::RemoveNoopLocalStores => "remove-noop-local-stores",
            Self::RemovePureExprOps => "remove-pure-expr-ops",
            Self::RemoveZstLocalRuntimeOps => "remove-zst-local-runtime-ops",
            Self::PropagateLocalCopies => "propagate-local-copies",
            Self::PropagateLocalConstants => "propagate-local-constants",
            Self::SimplifyConstantLogicalExprs => "simplify-constant-logical-exprs",
            Self::RemoveOverwrittenLocalStores => "remove-overwritten-local-stores",
            Self::RemoveNeverReadLocalStores => "remove-never-read-local-stores",
            Self::RemoveUnusedTempBindings => "remove-unused-temp-bindings",
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

    fn run(
        self,
        body: &mut FunctionBody,
        is_zero_sized: impl Fn(InternedTyId) -> bool + Copy,
    ) -> bool {
        match self {
            Self::SimplifySameTypeCasts => simplify_same_type_casts_in_blocks(&mut body.blocks),
            Self::RemoveNoopLocalStores => remove_noop_local_stores(&mut body.blocks),
            Self::RemovePureExprOps => remove_pure_expr_ops(&mut body.blocks),
            Self::RemoveZstLocalRuntimeOps => remove_zst_local_runtime_ops(body, is_zero_sized),
            Self::PropagateLocalCopies => propagate_local_copies(body),
            Self::PropagateLocalConstants => propagate_local_constants(body),
            Self::SimplifyConstantLogicalExprs => simplify_constant_logical_exprs(body),
            Self::RemoveOverwrittenLocalStores => remove_overwritten_local_stores(body),
            Self::RemoveNeverReadLocalStores => remove_never_read_local_stores(body),
            Self::RemoveUnusedTempBindings => remove_unused_temp_bindings(body),
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
                policy.dead_code_elim.at_least(OptimizationDepth::Cheap)
            }
            Self::RemoveZstLocalRuntimeOps => {
                policy.dead_code_elim.at_least(OptimizationDepth::Cheap)
            }
            Self::PropagateLocalCopies => {
                policy.local_copy_prop.at_least(OptimizationDepth::Full)
                    || (policy.prefer_size
                        && policy.local_copy_prop.at_least(OptimizationDepth::Cheap))
            }
            Self::PropagateLocalConstants => {
                policy.const_fold.at_least(OptimizationDepth::Aggressive)
                    && policy
                        .local_copy_prop
                        .at_least(OptimizationDepth::Aggressive)
                    && !policy.prefer_size
            }
            Self::SimplifyConstantLogicalExprs => {
                policy.const_fold.at_least(OptimizationDepth::Cheap)
            }
            Self::RemoveOverwrittenLocalStores
            | Self::RemoveNeverReadLocalStores
            | Self::RemoveUnusedLocalBindings => {
                policy.dead_code_elim.at_least(OptimizationDepth::Full)
            }
            Self::RemoveUnusedTempBindings => {
                policy.dead_code_elim.at_least(OptimizationDepth::Cheap)
            }
            Self::FoldConstantBoolBranches => policy.const_fold.at_least(OptimizationDepth::Cheap),
            Self::FoldConstantSwitches => {
                policy.const_fold.at_least(OptimizationDepth::Full)
                    && policy.simplify_cfg.at_least(OptimizationDepth::Full)
            }
            Self::SimplifyTrivialBranches
            | Self::MergeEmptyJumpBlocks
            | Self::RemoveUnreachableBlocks
            | Self::OptimizeDeferBodies => policy.simplify_cfg.at_least(OptimizationDepth::Cheap),
            Self::SimplifySameTargetSwitches => {
                policy.simplify_cfg.at_least(OptimizationDepth::Full)
            }
        }
    }
}

struct FunctionOptPipeline {
    passes: Vec<FunctionOptPass>,
}

impl FunctionOptPipeline {
    fn for_policy(policy: &OptimizationPolicy) -> Self {
        let passes = ORDERED_FUNCTION_PASSES
            .iter()
            .copied()
            .filter(|pass| pass.enabled_by(policy))
            .collect();
        Self { passes }
    }

    fn run(
        &self,
        body: &mut FunctionBody,
        is_zero_sized: impl Fn(InternedTyId) -> bool + Copy,
    ) -> Vec<&'static str> {
        let mut changed_passes = Vec::new();
        for pass in &self.passes {
            let name = pass.name();
            debug_assert!(!name.is_empty());
            if (*pass).run(body, is_zero_sized) {
                changed_passes.push(name);
            }
        }
        changed_passes
    }
}

const ORDERED_FUNCTION_PASSES: &[FunctionOptPass] = &[
    FunctionOptPass::SimplifySameTypeCasts,
    FunctionOptPass::RemoveNoopLocalStores,
    FunctionOptPass::RemovePureExprOps,
    FunctionOptPass::RemoveZstLocalRuntimeOps,
    FunctionOptPass::PropagateLocalCopies,
    FunctionOptPass::PropagateLocalConstants,
    FunctionOptPass::SimplifyConstantLogicalExprs,
    FunctionOptPass::RemoveOverwrittenLocalStores,
    FunctionOptPass::RemoveNeverReadLocalStores,
    FunctionOptPass::RemoveUnusedTempBindings,
    FunctionOptPass::RemoveUnusedLocalBindings,
    FunctionOptPass::FoldConstantBoolBranches,
    FunctionOptPass::FoldConstantSwitches,
    FunctionOptPass::SimplifyTrivialBranches,
    FunctionOptPass::SimplifySameTargetSwitches,
    FunctionOptPass::MergeEmptyJumpBlocks,
    FunctionOptPass::RemoveUnreachableBlocks,
    FunctionOptPass::OptimizeDeferBodies,
];

#[cfg(test)]
const O1_PASSES: &[FunctionOptPass] = &[
    FunctionOptPass::SimplifySameTypeCasts,
    FunctionOptPass::RemoveNoopLocalStores,
    FunctionOptPass::RemovePureExprOps,
    FunctionOptPass::RemoveZstLocalRuntimeOps,
    FunctionOptPass::SimplifyConstantLogicalExprs,
    FunctionOptPass::RemoveUnusedTempBindings,
    FunctionOptPass::FoldConstantBoolBranches,
    FunctionOptPass::SimplifyTrivialBranches,
    FunctionOptPass::MergeEmptyJumpBlocks,
    FunctionOptPass::RemoveUnreachableBlocks,
    FunctionOptPass::OptimizeDeferBodies,
];

#[cfg(test)]
const O2_PASSES: &[FunctionOptPass] = &[
    FunctionOptPass::SimplifySameTypeCasts,
    FunctionOptPass::RemoveNoopLocalStores,
    FunctionOptPass::RemovePureExprOps,
    FunctionOptPass::RemoveZstLocalRuntimeOps,
    FunctionOptPass::PropagateLocalCopies,
    FunctionOptPass::SimplifyConstantLogicalExprs,
    FunctionOptPass::RemoveOverwrittenLocalStores,
    FunctionOptPass::RemoveNeverReadLocalStores,
    FunctionOptPass::RemoveUnusedTempBindings,
    FunctionOptPass::RemoveUnusedLocalBindings,
    FunctionOptPass::FoldConstantBoolBranches,
    FunctionOptPass::FoldConstantSwitches,
    FunctionOptPass::SimplifyTrivialBranches,
    FunctionOptPass::SimplifySameTargetSwitches,
    FunctionOptPass::MergeEmptyJumpBlocks,
    FunctionOptPass::RemoveUnreachableBlocks,
    FunctionOptPass::OptimizeDeferBodies,
];

#[cfg(test)]
const O3_PASSES: &[FunctionOptPass] = &[
    FunctionOptPass::SimplifySameTypeCasts,
    FunctionOptPass::RemoveNoopLocalStores,
    FunctionOptPass::RemovePureExprOps,
    FunctionOptPass::RemoveZstLocalRuntimeOps,
    FunctionOptPass::PropagateLocalCopies,
    FunctionOptPass::PropagateLocalConstants,
    FunctionOptPass::SimplifyConstantLogicalExprs,
    FunctionOptPass::RemoveOverwrittenLocalStores,
    FunctionOptPass::RemoveNeverReadLocalStores,
    FunctionOptPass::RemoveUnusedTempBindings,
    FunctionOptPass::RemoveUnusedLocalBindings,
    FunctionOptPass::FoldConstantBoolBranches,
    FunctionOptPass::FoldConstantSwitches,
    FunctionOptPass::SimplifyTrivialBranches,
    FunctionOptPass::SimplifySameTargetSwitches,
    FunctionOptPass::MergeEmptyJumpBlocks,
    FunctionOptPass::RemoveUnreachableBlocks,
    FunctionOptPass::OptimizeDeferBodies,
];
