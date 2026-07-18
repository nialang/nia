// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn provide_const_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ConstModuleLowering {
    let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
    let defs = db.get(FullModuleDefsQuery(module_id));
    let values = db.get(ValueResolutionQuery(module_id));
    let locals = db.get(LocalResolutionQuery(module_id));
    let semantic_uses = db.get(SemanticUseTableQuery(module_id));
    let type_lowering = db.get(TypeLoweringQuery(module_id));
    let signatures = db.query(ItemSignaturesQuery(module_id));
    let source_path = db.query(ModulePathQuery(module_id));
    let symbols = db.context().symbols();
    nia_const_check::lower_module_const(nia_const_check::ConstModuleInput {
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

pub(super) fn provide_const(db: &QueryDb<CompilerContext>, module_id: ModuleId) -> ConstCheck {
    time_module_provider(db, "const", module_id, || {
        let array_lengths = db.query(ConstArrayLengthsQuery(module_id));
        let enum_values = db.query(ConstEnumValuesQuery(module_id));
        let values = db.query(ConstValuesQuery(module_id));
        let typed_facts = db.query(ConstTypedFactsQuery(module_id));

        with_const_input(db, module_id, |_input, module| {
            let mut const_eval = nia_const_check::check_module_const_with_all_phases(
                array_lengths,
                enum_values,
                values,
                typed_facts,
            );
            const_eval.diagnostics.extend(module.diagnostics.clone());
            const_eval
        })
    })
}

pub(super) fn provide_const_array_lengths(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_const_check::ConstArrayLengths {
    with_const_input(db, module_id, |input, module| {
        let mut array_lengths = nia_const_check::compute_module_const_array_lengths(input);
        array_lengths.diagnostics.extend(module.diagnostics.clone());
        array_lengths
    })
}

pub(super) fn provide_const_enum_values(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_const_check::ConstEnumValues {
    let array_lengths = db.query(ConstArrayLengthsQuery(module_id));
    with_const_input(db, module_id, |input, module| {
        let mut enum_values =
            nia_const_check::compute_module_const_enum_values(input, array_lengths);
        enum_values.diagnostics.extend(module.diagnostics.clone());
        enum_values
    })
}

pub(super) fn provide_const_values(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_const_check::ConstValues {
    let array_lengths = db.query(ConstArrayLengthsQuery(module_id));
    let enum_values = db.query(ConstEnumValuesQuery(module_id));
    with_const_input(db, module_id, |input, module| {
        let mut values =
            nia_const_check::compute_module_const_values(input, array_lengths, enum_values);
        values.diagnostics.extend(module.diagnostics.clone());
        values
    })
}

pub(super) fn provide_const_typed_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_const_check::ConstTypedFacts {
    let array_lengths = db.query(ConstArrayLengthsQuery(module_id));
    let enum_values = db.query(ConstEnumValuesQuery(module_id));
    let values = db.query(ConstValuesQuery(module_id));
    with_const_input(db, module_id, |input, _module| {
        nia_const_check::compute_module_const_typed_facts(input, array_lengths, enum_values, values)
    })
}

fn with_const_input<T>(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    f: impl FnOnce(nia_const_check::ConstInput<'_>, &ConstModuleLowering) -> T,
) -> T {
    with_const_input_and_program_facts(db, module_id, None, |_| false, f)
}

pub(super) fn with_const_input_and_program_facts<T>(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    non_function_signatures_override: Option<&ProgramExecutableNonFunctionSignatures>,
    use_signature_facts_for: impl Fn(ModuleId) -> bool,
    f: impl FnOnce(nia_const_check::ConstInput<'_>, &ConstModuleLowering) -> T,
) -> T {
    let module = db.query(ConstModuleQuery(module_id));
    let defs = db.get(FullModuleDefsQuery(module_id));
    let program_module = |module_id| {
        if use_signature_facts_for(module_id) {
            return Some(signature_const_module_lowering(db, module_id).module);
        }
        Some(db.query(ConstModuleQuery(module_id)).module)
    };
    let program_source_path = |module_id| Some(db.query(ModulePathQuery(module_id)));
    let program_defs = |module_id| Some(db.get(FullModuleDefsQuery(module_id)));
    let program_type_normalization = |module_id| {
        if use_signature_facts_for(module_id) {
            return Some(db.get(SignatureTypeNormalizationQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Types,
            )));
        }
        Some(db.get(TypeNormalizationQuery(module_id)))
    };
    let local_trait_impls = non_function_signatures_override
        .is_none()
        .then(|| db.get(VisibleTraitImplsQuery(module_id)));
    let trait_impls_for_module = |requested_module_id| {
        if requested_module_id == module_id {
            return non_function_signatures_override
                .map(|signatures| signatures.trait_impls.clone())
                .or_else(|| {
                    local_trait_impls
                        .as_ref()
                        .map(|signatures| signatures.trait_impls.clone())
                });
        }
        if let Some(signatures) = non_function_signatures_override {
            return Some(signatures.trait_impls.clone());
        }
        Some(
            db.query(VisibleTraitImplsQuery(requested_module_id))
                .trait_impls
                .clone(),
        )
    };
    let program_is_enum = |def_id: GlobalDefId| {
        non_function_signatures_override
            .is_some_and(|signatures| signatures.enums.contains_key(&def_id))
            || db
                .get(SignatureItemSignaturesQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ))
                .enums
                .contains_key(&def_id.def_id)
    };
    let item_signatures_for_module = |module_id| {
        if use_signature_facts_for(module_id) {
            return Some(db.get(SignatureItemSignaturesQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Types,
            )));
        }
        Some(db.get(ItemSignaturesQuery(module_id)))
    };
    let value_signatures_for_module = |module_id| {
        Some(db.get(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Values,
        )))
    };
    let local_visible_extensions = db.get(VisibleExtensionsQuery(module_id));
    let visible_extensions_for_module = |requested_module_id| {
        if requested_module_id == module_id {
            return Some(local_visible_extensions.methods.clone());
        }
        Some(
            db.query(VisibleExtensionsQuery(requested_module_id))
                .methods
                .clone(),
        )
    };
    let values = db.get(ValueResolutionQuery(module_id));
    let locals = db.get(LocalResolutionQuery(module_id));
    let semantic_uses = db.get(SemanticUseTableQuery(module_id));
    let source_path = db.query(ModulePathQuery(module_id));
    let item_signatures = db.query(ItemSignaturesQuery(module_id));
    let type_lowering = db.get(TypeLoweringQuery(module_id));
    let type_normalization = db.get(TypeNormalizationQuery(module_id));
    let target = db.query(CompilerTargetQuery);
    let symbols = db.context().symbols();
    let input = nia_const_check::ConstInput {
        type_store: &db.context().type_store,
        module: &module.module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        symbols: &symbols,
        lowered: &type_lowering,
        signatures: &item_signatures,
        normalization: &type_normalization,
        target: &target,
        source_path: &source_path,
        program: nia_const_check::ConstProgramContext {
            module: Some(&program_module),
            source_path: Some(&program_source_path),
            defs: Some(&program_defs),
            type_normalizations: Some(&program_type_normalization),
            signatures: Some(&item_signatures_for_module),
            value_signatures: Some(&value_signatures_for_module),
            const_values: None,
            global_initializer: None,
            program_is_enum: Some(&program_is_enum),
            trait_impls_for_module: Some(&trait_impls_for_module),
            visible_extensions: Some(&visible_extensions_for_module),
        },
    };
    f(input, &module)
}
