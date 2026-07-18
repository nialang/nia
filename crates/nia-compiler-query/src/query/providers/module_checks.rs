// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn provide_layouts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_layout::Layouts {
    time_module_provider(db, "layouts", module_id, || {
        let defs = db.get(FullModuleDefsQuery(module_id));
        let type_lowering = db.query(TypeLoweringQuery(module_id));
        let type_normalization = db.query(LayoutTypeNormalizationQuery(module_id));
        let item_signatures = db.query(ItemSignaturesQuery(module_id));
        let array_lengths = db.query(ConstArrayLengthsQuery(module_id));
        let symbols = db.context().symbols();
        let mut root_types = item_signatures.type_roots();
        root_types.extend(type_lowering.explicit_type_roots());
        root_types.sort_unstable();
        root_types.dedup();
        let layout_query = |module_id| Some(db.query(SignatureLayoutsQuery(module_id)));
        let local_array_lengths = |id| array_lengths.values.get(&id).copied();
        let program_array_lengths = |id: nia_ids::GlobalConstExprId| {
            Some(db.query(ConstArrayLengthsQuery(id.module_id)))
                .and_then(|array_lengths| array_lengths.values.get(&id).copied())
        };
        nia_layout::compute_layouts_with_program_context(nia_layout::LayoutComputationInput {
            type_store: &db.context().type_store,
            defs: &defs,
            signatures: &item_signatures,
            root_types: &root_types,
            normalized: &type_normalization.normalized,
            array_lengths: &local_array_lengths,
            target: nia_layout::TargetDataLayout::LP64,
            program: nia_layout::ProgramLayoutContext {
                symbols: Some(&symbols),
                layouts: Some(&layout_query),
                array_lengths: Some(&program_array_lengths),
                ..Default::default()
            },
        })
    })
}

pub(super) fn provide_signature_layouts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_layout::Layouts {
    signature_layouts_for_types(db, module_id, None)
}

pub(super) fn provide_abi_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_abi_check::AbiCheck {
    let defs = db.get(FullModuleDefsQuery(module_id));
    let function_signatures = db.get(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
    ));
    let type_signatures = db.get(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ));
    let value_signatures = db.get(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Values,
    ));
    let program = db.query(ProgramAbiSignaturesQuery);
    nia_abi_check::check_module_abi_families_with_program_signatures(
        &defs,
        db.context().type_store(),
        nia_abi_check::ModuleAbiSignatures {
            functions: &function_signatures.functions,
            structs: &type_signatures.structs,
            unions: &type_signatures.unions,
            enums: &type_signatures.enums,
            globals: &value_signatures.globals,
        },
        nia_abi_check::ProgramAbiSignatures {
            structs: &program.structs,
            unions: &program.unions,
            enums: &program.enums,
        },
    )
}

pub(super) fn provide_static_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_static_check::StaticCheck {
    let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
    let defs = db.get(FullModuleDefsQuery(module_id));
    let values = db.query(ValueResolutionQuery(module_id));
    let locals = db.query(LocalResolutionQuery(module_id));
    let semantic_uses = db.query(SemanticUseTableQuery(module_id));
    let symbols = db.context().symbols();
    let signatures = db.get(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Values,
    ));
    let const_eval = db.query(ConstValuesQuery(module_id));
    let program_defs = |module_id| Some(db.get(FullModuleDefsQuery(module_id)));
    let program_const_values = |module_id| Some(db.query(ConstValuesQuery(module_id)));
    nia_static_check::check_module_static_initializers_with_signatures(
        nia_static_check::StaticCheckPreciseInput {
            active_item_tree: &active_item_tree,
            defs: &defs,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
            symbols: &symbols,
            signatures: nia_static_check::StaticCheckSignatures {
                globals: &signatures.globals,
            },
            const_eval: &const_eval,
            program_defs: &program_defs,
            program_const: &program_const_values,
            target: &db.query(CompilerTargetQuery),
        },
    )
}

pub(super) fn provide_flow_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_flow_check::FlowCheck {
    let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
    let signatures = db.get(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
    ));
    nia_flow_check::check_active_module_flow_with_signatures(
        &active_item_tree,
        db.context().type_store(),
        nia_flow_check::FlowCheckSignatures {
            functions: &signatures.functions,
        },
    )
}
