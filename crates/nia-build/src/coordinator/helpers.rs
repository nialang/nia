// SPDX-License-Identifier: GPL-3.0-or-later
//! Frozen-plan lookup, target conversion, and typed consistency errors.

use super::*;

pub(super) fn validate_invocation_targets(
    plan: &BuildPlan,
    invocation: &BuildInvocation,
) -> Result<(), CoordinatorError> {
    for (role, found, expected) in [
        (
            "host",
            plan.host_target(),
            target_spec(invocation.toolchain.host_target()),
        ),
        (
            "artifact",
            plan.artifact_target(),
            target_spec(invocation.toolchain.artifact_target()),
        ),
    ] {
        if found != &expected {
            return Err(CoordinatorError::TargetMismatch(Box::new(TargetMismatch {
                role,
                expected,
                found: found.clone(),
            })));
        }
    }
    Ok(())
}

pub(super) fn find_step(steps: &[crate::PlanStep], key: &StepKey) -> Option<usize> {
    steps.binary_search_by(|step| step.key.cmp(key)).ok()
}

pub(super) fn find_action<'a>(
    actions: &'a [PlanAction],
    key: &ActionKey,
) -> Option<&'a PlanAction> {
    actions
        .binary_search_by(|action| action.key.cmp(key))
        .ok()
        .map(|index| &actions[index])
}

pub(super) fn find_module<'a>(
    modules: &'a [PlanModule],
    key: &ModuleKey,
) -> Option<&'a PlanModule> {
    modules
        .binary_search_by(|module| module.key.cmp(key))
        .ok()
        .map(|index| &modules[index])
}

pub(super) fn find_artifact<'a>(
    artifacts: &'a [PlanArtifact],
    key: &ArtifactKey,
) -> Option<&'a PlanArtifact> {
    artifacts
        .binary_search_by(|artifact| artifact.key.cmp(key))
        .ok()
        .map(|index| &artifacts[index])
}

pub(super) fn inconsistent(owner: impl Into<String>, missing: String) -> CoordinatorError {
    CoordinatorError::InconsistentPlan {
        owner: owner.into(),
        missing,
    }
}

pub(super) fn unsupported(action: &PlanAction, kind: &'static str) -> CoordinatorError {
    CoordinatorError::UnsupportedAction {
        action: action.key.clone(),
        kind,
    }
}

pub(super) fn optimization(mode: OptimizationMode) -> NiaOptimizationLevel {
    match mode {
        OptimizationMode::O0 => NiaOptimizationLevel::O0,
        OptimizationMode::O1 => NiaOptimizationLevel::O1,
        OptimizationMode::O2 => NiaOptimizationLevel::O2,
        OptimizationMode::O3 => NiaOptimizationLevel::O3,
        OptimizationMode::Os => NiaOptimizationLevel::Os,
        OptimizationMode::Oz => NiaOptimizationLevel::Oz,
    }
}

pub(super) fn runtime_mode(runtime: Runtime) -> DriverRuntime {
    match runtime {
        Runtime::Bare => DriverRuntime::Bare,
        Runtime::Freestanding => DriverRuntime::Freestanding,
    }
}

pub(super) fn target_spec(target: &TargetConfig) -> TargetSpec {
    TargetSpec {
        arch: target.arch.clone(),
        vendor: target.vendor.clone(),
        os: target.os.clone(),
        env: target.env.clone(),
        abi: target.abi.clone(),
        endian: target.endian.clone(),
        pointer_width: target.pointer_width,
    }
}

pub(super) fn target_config(target: &TargetSpec) -> TargetConfig {
    TargetConfig {
        arch: target.arch.clone(),
        vendor: target.vendor.clone(),
        os: target.os.clone(),
        env: target.env.clone(),
        abi: target.abi.clone(),
        endian: target.endian.clone(),
        pointer_width: target.pointer_width,
    }
}

pub(super) fn display_target(target: &TargetSpec) -> String {
    format!(
        "{}-{}-{}-{}-{} ({}-bit {})",
        target.arch,
        target.vendor,
        target.os,
        target.env,
        target.abi,
        target.pointer_width,
        target.endian
    )
}
