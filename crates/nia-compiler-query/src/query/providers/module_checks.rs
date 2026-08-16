// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn provide_layouts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ModuleLayouts> {
    time_module_provider(db, "layouts", module_id, || {
        let defs = full_module_defs_semantic(db, module_id)?;
        let type_lowering = type_lowering_semantic(db, module_id)?;
        let type_normalization = db.get(LayoutTypeNormalizationQuery(module_id))?;
        let item_signatures = item_signatures_semantic(db, module_id)?;
        let array_lengths = db.get(ConstArrayLengthsQuery(module_id))?;
        let target = compiler_target_data_layout(db)?;
        let symbols = db.context().symbols();
        let query_failure = RefCell::new(None);
        let mut root_types = item_signatures.type_roots();
        root_types.extend(type_lowering.explicit_type_roots());
        root_types.sort_unstable();
        root_types.dedup();
        let layout_query = |module_id| {
            capture_query_failure(&query_failure, db.get(SignatureLayoutsQuery(module_id)))
                .map(|layouts| Arc::clone(&layouts.semantic))
        };
        let local_array_lengths = |id| array_lengths.values.get(&id).copied();
        let program_array_lengths = |id: nia_ids::GlobalConstExprId| {
            capture_query_failure(&query_failure, db.get(ConstArrayLengthsQuery(id.module_id)))
                .and_then(|array_lengths| array_lengths.values.get(&id).copied())
        };
        let layouts =
            nia_layout::compute_layouts_with_program_context(nia_layout::LayoutComputationInput {
                type_store: &db.context().type_store,
                defs: &defs,
                signatures: &item_signatures,
                root_types: &root_types,
                normalized: &type_normalization.normalized,
                array_lengths: &local_array_lengths,
                target,
                program: nia_layout::ProgramLayoutContext {
                    symbols: Some(&symbols),
                    layouts: Some(&layout_query),
                    array_lengths: Some(&program_array_lengths),
                    ..Default::default()
                },
            });
        match query_failure.into_inner() {
            Some(error) => Err(error),
            None => Ok(store_module_layouts(db.context(), layouts)),
        }
    })
}

pub(super) fn provide_signature_layouts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ModuleLayouts> {
    Ok(store_module_layouts(
        db.context(),
        signature_layouts_for_types(db, module_id, None)?,
    ))
}

pub(super) fn provide_abi_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ModuleAbiCheck> {
    let defs = full_module_defs_semantic(db, module_id)?;
    let function_signatures = db.get(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
    ))?;
    let type_signatures = db.get(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ))?;
    let value_signatures = db.get(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Values,
    ))?;
    let program = db.get(ProgramAbiSignaturesQuery)?;
    let mut abi_check = nia_abi_check::check_module_abi_families_with_program_signatures(
        &defs,
        db.context().type_store(),
        nia_abi_check::ModuleAbiSignatures {
            functions: &function_signatures.semantic.functions,
            structs: &type_signatures.semantic.structs,
            unions: &type_signatures.semantic.unions,
            enums: &type_signatures.semantic.enums,
            type_aliases: &type_signatures.semantic.type_aliases,
            globals: &value_signatures.semantic.globals,
        },
        nia_abi_check::ProgramAbiSignatures {
            structs: &program.structs,
            unions: &program.unions,
            enums: &program.enums,
            type_aliases: &program.type_aliases,
        },
    );
    let diagnostics = std::mem::take(&mut abi_check.diagnostics);
    Ok(ModuleAbiCheck {
        semantic: Arc::new(abi_check),
        diagnostics: db.context().diagnostic_store.bundle(diagnostics),
    })
}

pub(super) fn provide_static_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ModuleStaticCheck> {
    let active_item_tree = db.get(FullActiveModuleItemTreeQuery(module_id))?;
    let defs = full_module_defs_semantic(db, module_id)?;
    let values = value_resolution_semantic(db, module_id)?;
    let locals = local_resolution_semantic(db, module_id)?;
    let semantic_uses = db.get(SemanticUseTableQuery(module_id))?;
    let symbols = db.context().symbols();
    let signatures = db.get(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Values,
    ))?;
    let const_eval = db.get(ConstValuesQuery(module_id))?;
    let query_failure = RefCell::new(None);
    let program_defs =
        |module_id| capture_query_failure(&query_failure, full_module_defs_semantic(db, module_id));
    let program_const_values =
        |module_id| capture_query_failure(&query_failure, db.get(ConstValuesQuery(module_id)));
    let mut static_check = nia_static_check::check_module_static_initializers_with_signatures(
        nia_static_check::StaticCheckPreciseInput {
            active_item_tree: &active_item_tree,
            defs: &defs,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
            symbols: &symbols,
            signatures: nia_static_check::StaticCheckSignatures {
                globals: &signatures.semantic.globals,
            },
            const_eval: &const_eval,
            program_defs: &program_defs,
            program_const: &program_const_values,
            target: db.get(CompilerTargetQuery)?.as_ref(),
        },
    );
    match query_failure.into_inner() {
        Some(error) => Err(error),
        None => {
            let diagnostics = std::mem::take(&mut static_check.diagnostics);
            Ok(ModuleStaticCheck {
                semantic: Arc::new(static_check),
                diagnostics: db.context().diagnostic_store.bundle(diagnostics),
            })
        }
    }
}

pub(super) fn provide_flow_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ModuleFlowCheck> {
    let active_item_tree = db.get(FullActiveModuleItemTreeQuery(module_id))?;
    let signatures = db.get(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
    ))?;
    let mut flow_check = nia_flow_check::check_active_module_flow_with_signatures(
        &active_item_tree,
        db.context().type_store(),
        nia_flow_check::FlowCheckSignatures {
            functions: &signatures.semantic.functions,
        },
    );
    let diagnostics = std::mem::take(&mut flow_check.diagnostics);
    Ok(ModuleFlowCheck {
        semantic: Arc::new(flow_check),
        diagnostics: db.context().diagnostic_store.bundle(diagnostics),
    })
}
