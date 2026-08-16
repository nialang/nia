// SPDX-License-Identifier: GPL-3.0-or-later
//! Policy-driven, semantics-preserving optimization of Nia function IR.
//!
//! The optimizer runs after function lowering and before backend-specific
//! lowering. Passes may remove locals and blocks, but must preserve the
//! structural invariants checked by [`nia_function_ir::validate_function_body`]
//! as well as evaluation order and observable effects. Debug builds validate
//! those structural invariants at the pipeline boundary and after every pass.

use nia_function_ir::FunctionBody;
#[cfg(debug_assertions)]
use nia_function_ir::validate_function_body;
use nia_ids::InternedTyId;
use nia_opt::{OptimizationDepth, OptimizationPolicy};

mod passes;
#[cfg(test)]
mod tests;

use passes::*;

/// Lists the function-pass names enabled by an optimization policy in their
/// execution order.
pub fn enabled_function_passes(policy: &OptimizationPolicy) -> Vec<&'static str> {
    FunctionOptPipeline::for_policy(policy)
        .passes
        .iter()
        .map(|pass| pass.name())
        .collect()
}

/// Inputs for [`optimize_function_body`].
pub struct FunctionOptInput<'a, F> {
    /// Structurally valid function IR to optimize.
    pub body: FunctionBody,
    /// Capabilities and depth selected for this compilation.
    pub policy: &'a OptimizationPolicy,
    /// Target-aware zero-sized-type predicate used by storage cleanup.
    pub is_zero_sized: F,
}

/// Optimized IR and the ordered names of passes that changed it.
pub struct FunctionOptOutput {
    /// Function body after all enabled passes have run.
    pub body: FunctionBody,
    /// Names of enabled passes that reported at least one transformation.
    pub changed_passes: Vec<&'static str>,
}

/// Optimizes a structurally valid function body according to `input.policy`.
///
/// Callers must reject malformed input with
/// [`nia_function_ir::validate_function_body`] before this boundary. In debug
/// builds this function checks that precondition and revalidates after every
/// enabled pass, attributing invariant breakage to the pass that introduced it.
pub fn optimize_function_body<F>(input: FunctionOptInput<'_, F>) -> FunctionOptOutput
where
    F: Fn(InternedTyId) -> bool + Copy,
{
    let mut body = input.body;
    debug_validate_function_body(&body, "optimizer input");
    let changed_passes =
        FunctionOptPipeline::for_policy(input.policy).run(&mut body, input.is_zero_sized);
    FunctionOptOutput {
        body,
        changed_passes,
    }
}

#[cfg(debug_assertions)]
fn debug_validate_function_body(body: &FunctionBody, stage: &str) {
    if let Err(error) = validate_function_body(body) {
        panic!(
            "Nia ICE: invalid function IR at {stage}: {} at {:?}",
            error.message, error.span
        );
    }
}

#[cfg(not(debug_assertions))]
fn debug_validate_function_body(_body: &FunctionBody, _stage: &str) {}

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
            let changed = (*pass).run(body, is_zero_sized);
            debug_validate_function_body(body, name);
            if changed {
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
