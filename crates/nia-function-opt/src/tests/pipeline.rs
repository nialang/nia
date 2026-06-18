use super::*;

#[test]
fn o0_pipeline_has_no_optional_backend_passes() {
    let pipeline = FunctionOptPipeline::for_policy(&NiaOptimizationLevel::O0.policy());

    assert!(pipeline.passes.is_empty());
}

#[test]
fn o1_pipeline_keeps_canonical_pass_order() {
    let pipeline = FunctionOptPipeline::for_policy(&NiaOptimizationLevel::O1.policy());

    assert_eq!(pipeline.passes.as_slice(), O1_PASSES);
}

#[test]
fn o2_family_pipeline_starts_from_o1_cleanup_passes() {
    for level in [
        NiaOptimizationLevel::O2,
        NiaOptimizationLevel::Os,
        NiaOptimizationLevel::Oz,
    ] {
        let pipeline = FunctionOptPipeline::for_policy(&level.policy());

        assert_eq!(pipeline.passes.as_slice(), O2_PASSES);
        for pass in O1_PASSES {
            assert!(pipeline.passes.contains(pass), "{level:?} missing {pass:?}");
        }
        assert!(
            pipeline
                .passes
                .contains(&FunctionOptPass::PropagateLocalCopies)
        );
        assert!(
            pipeline
                .passes
                .contains(&FunctionOptPass::RemoveUnusedLocalBindings)
        );
    }
}

#[test]
fn o3_pipeline_adds_aggressive_constant_propagation() {
    let pipeline = FunctionOptPipeline::for_policy(&NiaOptimizationLevel::O3.policy());

    assert_eq!(pipeline.passes.as_slice(), O3_PASSES);
    assert!(
        pipeline
            .passes
            .contains(&FunctionOptPass::PropagateLocalConstants)
    );
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
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert_eq!(
        pipeline.passes,
        vec![
            FunctionOptPass::SimplifySameTypeCasts,
            FunctionOptPass::RemoveNoopLocalStores,
            FunctionOptPass::RemovePureExprOps,
            FunctionOptPass::RemoveZstLocalRuntimeOps,
            FunctionOptPass::SimplifyConstantLogicalExprs,
            FunctionOptPass::RemoveOverwrittenLocalStores,
            FunctionOptPass::RemoveNeverReadLocalStores,
            FunctionOptPass::RemoveUnusedTempBindings,
            FunctionOptPass::RemoveUnusedLocalBindings,
            FunctionOptPass::FoldConstantBoolBranches,
        ]
    );
}

#[test]
fn constant_logical_expr_simplification_is_selected_from_const_fold_policy() {
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
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert!(
        !pipeline
            .passes
            .contains(&FunctionOptPass::SimplifyConstantLogicalExprs)
    );

    let policy = nia_opt::OptimizationPolicy {
        const_fold: OptimizationDepth::Cheap,
        ..policy
    };
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert!(
        pipeline
            .passes
            .contains(&FunctionOptPass::SimplifyConstantLogicalExprs)
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
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert!(
        !pipeline
            .passes
            .contains(&FunctionOptPass::FoldConstantBoolBranches)
    );
    assert!(
        pipeline
            .passes
            .contains(&FunctionOptPass::SimplifyTrivialBranches)
    );

    let policy = nia_opt::OptimizationPolicy {
        simplify_cfg: OptimizationDepth::Required,
        const_fold: OptimizationDepth::Cheap,
        ..policy
    };
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert!(
        pipeline
            .passes
            .contains(&FunctionOptPass::FoldConstantBoolBranches)
    );
    assert!(
        !pipeline
            .passes
            .contains(&FunctionOptPass::SimplifyTrivialBranches)
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
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert!(
        !pipeline
            .passes
            .contains(&FunctionOptPass::SimplifySameTargetSwitches)
    );

    let policy = nia_opt::OptimizationPolicy {
        simplify_cfg: OptimizationDepth::Full,
        ..policy
    };
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert!(
        pipeline
            .passes
            .contains(&FunctionOptPass::SimplifySameTargetSwitches)
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
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert!(
        !pipeline
            .passes
            .contains(&FunctionOptPass::FoldConstantSwitches)
    );

    let policy = nia_opt::OptimizationPolicy {
        const_fold: OptimizationDepth::Full,
        simplify_cfg: OptimizationDepth::Cheap,
        ..policy
    };
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert!(
        !pipeline
            .passes
            .contains(&FunctionOptPass::FoldConstantSwitches)
    );

    let policy = nia_opt::OptimizationPolicy {
        const_fold: OptimizationDepth::Full,
        simplify_cfg: OptimizationDepth::Full,
        ..policy
    };
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert!(
        pipeline
            .passes
            .contains(&FunctionOptPass::FoldConstantSwitches)
    );
}

#[test]
fn local_constant_propagation_requires_aggressive_non_size_policy() {
    for level in [
        NiaOptimizationLevel::O0,
        NiaOptimizationLevel::O1,
        NiaOptimizationLevel::O2,
        NiaOptimizationLevel::Os,
        NiaOptimizationLevel::Oz,
    ] {
        let pipeline = FunctionOptPipeline::for_policy(&level.policy());

        assert!(
            !pipeline
                .passes
                .contains(&FunctionOptPass::PropagateLocalConstants),
            "{level:?} unexpectedly enables local constant propagation"
        );
    }

    let pipeline = FunctionOptPipeline::for_policy(&NiaOptimizationLevel::O3.policy());

    assert!(
        pipeline
            .passes
            .contains(&FunctionOptPass::PropagateLocalConstants)
    );
}
