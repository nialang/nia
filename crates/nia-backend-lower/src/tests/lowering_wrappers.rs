// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn lower_source(source: &str) -> TestBackendLowering {
    let lowering = lower_source_with_const_mutation(source, |_, _| {});
    assert!(
        lowering.diagnostics.is_empty(),
        "{:?}",
        lowering.diagnostics
    );
    lowering
}

pub(super) fn lower_source_with_const_mutation(
    source: &str,
    mutate_const: impl FnOnce(&mut nia_const_check::ConstCheck, &TypeLowering),
) -> TestBackendLowering {
    lower_source_with_body_mutation_const_mutation_and_optimization(
        source,
        |_| {},
        mutate_const,
        nia_opt::OptimizationPolicy::default(),
    )
}

pub(super) fn lower_source_with_body_mutation_and_optimization(
    source: &str,
    mutate_body: impl FnMut(&mut nia_function_ir::FunctionBody),
    optimization: nia_opt::OptimizationPolicy,
) -> TestBackendLowering {
    lower_source_with_body_mutation_extensions_const_mutation_and_optimization(
        source,
        mutate_body,
        |_, _, _, _, _| {},
        |_, _| {},
        optimization,
    )
}

pub(super) fn lower_source_with_body_mutation_const_mutation_and_optimization(
    source: &str,
    mutate_body: impl FnMut(&mut nia_function_ir::FunctionBody),
    mutate_const: impl FnOnce(&mut nia_const_check::ConstCheck, &TypeLowering),
    optimization: nia_opt::OptimizationPolicy,
) -> TestBackendLowering {
    lower_source_with_body_mutation_extensions_const_mutation_and_optimization(
        source,
        mutate_body,
        |_, _, _, _, _| {},
        mutate_const,
        optimization,
    )
}

pub(super) fn lower_source_with_body_mutation_extensions_const_mutation_and_optimization(
    source: &str,
    mutate_body: impl FnMut(&mut nia_function_ir::FunctionBody),
    mutate_extensions: impl FnOnce(
        &mut VisibleExtensionMethods,
        &nia_defs::DefCollection,
        &nia_ty::TypeStore,
        &TypeLowering,
        &ItemSignatures,
    ),
    mutate_const: impl FnOnce(&mut nia_const_check::ConstCheck, &TypeLowering),
    optimization: nia_opt::OptimizationPolicy,
) -> TestBackendLowering {
    lower_source_with_body_check_mutation_and_optimization(
        source,
        mutate_body,
        mutate_extensions,
        mutate_const,
        |_, _, _, _, _| {},
        |_, _| {},
        optimization,
    )
}

pub(super) fn lower_source_with_signature_mutation(
    source: &str,
    mutate_signatures: impl FnOnce(&mut ItemSignatures, &nia_defs::DefCollection),
) -> TestBackendLowering {
    lower_source_with_body_check_mutation_and_optimization(
        source,
        |_| {},
        |_, _, _, _, _| {},
        |_, _| {},
        |_, _, _, _, _| {},
        mutate_signatures,
        nia_opt::OptimizationPolicy::default(),
    )
}
