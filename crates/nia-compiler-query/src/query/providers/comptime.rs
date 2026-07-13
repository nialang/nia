// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn provide_comptime_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ComptimeModuleLowering {
    let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
    let defs = db.query_shared(FullModuleDefsQuery(module_id));
    let values = db.query(ValueResolutionQuery(module_id));
    let locals = db.query(LocalResolutionQuery(module_id));
    let semantic_uses = db.query(SemanticUseTableQuery(module_id));
    let type_lowering = db.query(TypeLoweringQuery(module_id));
    let signatures = db.query(ItemSignaturesQuery(module_id));
    let source_path = db.query(ModulePathQuery(module_id));
    let symbols = db.context().symbols();
    nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
        active_item_tree: &active_item_tree,
        defs: &defs,
        signatures: &signatures,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        symbols: &symbols,
        const_exprs: &type_lowering.const_exprs,
        source_path: &source_path,
    })
}

pub(super) fn provide_comptime(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ComptimeCheck {
    time_module_provider(db, "comptime", module_id, || {
        let array_lengths = db.query(ComptimeArrayLengthsQuery(module_id));
        let enum_values = db.query(ComptimeEnumValuesQuery(module_id));
        let values = db.query(ComptimeValuesQuery(module_id));
        let typed_facts = db.query(ComptimeTypedFactsQuery(module_id));

        with_comptime_input(db, module_id, |input, module| {
            let mut comptime = nia_comptime_check::check_module_comptime_with_all_phases(
                input,
                array_lengths,
                enum_values,
                values,
                typed_facts,
            );
            comptime.diagnostics.extend(module.diagnostics.clone());
            comptime
        })
    })
}

pub(super) fn provide_comptime_array_lengths(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_comptime_check::ComptimeArrayLengths {
    with_comptime_input(db, module_id, |input, module| {
        let mut array_lengths = nia_comptime_check::compute_module_comptime_array_lengths(input);
        array_lengths.diagnostics.extend(module.diagnostics.clone());
        array_lengths
    })
}

pub(super) fn provide_comptime_enum_values(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_comptime_check::ComptimeEnumValues {
    let array_lengths = db.query(ComptimeArrayLengthsQuery(module_id));
    with_comptime_input(db, module_id, |input, module| {
        let mut enum_values =
            nia_comptime_check::compute_module_comptime_enum_values(input, array_lengths);
        enum_values.diagnostics.extend(module.diagnostics.clone());
        enum_values
    })
}

pub(super) fn provide_comptime_values(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_comptime_check::ComptimeValues {
    let array_lengths = db.query(ComptimeArrayLengthsQuery(module_id));
    let enum_values = db.query(ComptimeEnumValuesQuery(module_id));
    with_comptime_input(db, module_id, |input, module| {
        let mut values =
            nia_comptime_check::compute_module_comptime_values(input, array_lengths, enum_values);
        values.diagnostics.extend(module.diagnostics.clone());
        values
    })
}

pub(super) fn provide_comptime_typed_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_comptime_check::ComptimeTypedFacts {
    let array_lengths = db.query(ComptimeArrayLengthsQuery(module_id));
    let enum_values = db.query(ComptimeEnumValuesQuery(module_id));
    let values = db.query(ComptimeValuesQuery(module_id));
    with_comptime_input(db, module_id, |input, _module| {
        nia_comptime_check::compute_module_comptime_typed_facts(
            input,
            array_lengths,
            enum_values,
            values,
        )
    })
}

fn with_comptime_input<T>(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    f: impl FnOnce(nia_comptime_check::ComptimeInput<'_>, &ComptimeModuleLowering) -> T,
) -> T {
    with_comptime_input_and_program_signatures(db, module_id, None, f)
}

pub(super) fn with_comptime_input_and_program_signatures<T>(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    non_function_signatures_override: Option<&ProgramExecutableNonFunctionSignatures>,
    f: impl FnOnce(nia_comptime_check::ComptimeInput<'_>, &ComptimeModuleLowering) -> T,
) -> T {
    let module = db.query(ComptimeModuleQuery(module_id));
    let defs = db.query_shared(FullModuleDefsQuery(module_id));
    let program_module = |module_id| Some(db.query(ComptimeModuleQuery(module_id)).module);
    let program_source_path = |module_id| Some(db.query(ModulePathQuery(module_id)));
    let program_defs = |module_id| Some(db.query_shared(FullModuleDefsQuery(module_id)));
    let program_type_normalization = |module_id| Some(db.query(TypeNormalizationQuery(module_id)));
    let value_type_normalization = |module_id| {
        Some(db.query(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Values,
        )))
    };
    let trait_impls_for_module = |module_id| {
        if let Some(signatures) = non_function_signatures_override {
            return Some(signatures.trait_impls.clone());
        }
        Some(
            db.query(VisibleTraitImplsQuery(module_id))
                .trait_impls
                .clone(),
        )
    };
    let program_is_enum = |def_id: GlobalDefId| {
        non_function_signatures_override
            .is_some_and(|signatures| signatures.enums.contains_key(&def_id))
            || db
                .query_shared(SignatureItemSignaturesQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ))
                .enums
                .contains_key(&def_id.def_id)
    };
    let item_signatures_for_module =
        |module_id| Some(db.query_shared(ItemSignaturesQuery(module_id)));
    let value_signatures_for_module = |module_id| {
        Some(db.query_shared(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Values,
        )))
    };
    let visible_extensions_for_module =
        |module_id| Some(db.query(VisibleExtensionsQuery(module_id)).methods.clone());
    let values = db.query(ValueResolutionQuery(module_id));
    let locals = db.query(LocalResolutionQuery(module_id));
    let semantic_uses = db.query(SemanticUseTableQuery(module_id));
    let source_path = db.query(ModulePathQuery(module_id));
    let item_signatures = db.query(ItemSignaturesQuery(module_id));
    let type_lowering = db.query(TypeLoweringQuery(module_id));
    let type_normalization = db.query(TypeNormalizationQuery(module_id));
    let target = db.query(CompilerTargetQuery);
    let symbols = db.context().symbols();
    f(
        nia_comptime_check::ComptimeInput {
            module: &module.module,
            defs: &defs,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
            symbols: &symbols,
            lowered: &type_lowering,
            signatures: &item_signatures,
            interner: &type_normalization.interner,
            normalized: &type_normalization.normalized,
            target: &target,
            source_path: &source_path,
            program: nia_comptime_check::ComptimeProgramContext {
                module: Some(&program_module),
                source_path: Some(&program_source_path),
                defs: Some(&program_defs),
                type_normalizations: Some(&program_type_normalization),
                value_type_normalizations: Some(&value_type_normalization),
                signatures: Some(&item_signatures_for_module),
                value_signatures: Some(&value_signatures_for_module),
                comptime_values: None,
                global_initializer: None,
                program_is_enum: Some(&program_is_enum),
                trait_impls_for_module: Some(&trait_impls_for_module),
                visible_extensions: Some(&visible_extensions_for_module),
            },
        },
        &module,
    )
}
