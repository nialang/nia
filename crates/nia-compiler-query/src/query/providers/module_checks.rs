// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn provide_layouts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_layout::Layouts {
    time_module_provider(db, "layouts", module_id, || {
        let defs = db.query_shared(FullModuleDefsQuery(module_id));
        let type_normalization = db.query(LayoutTypeNormalizationQuery(module_id));
        let item_signatures = db.query(ItemSignaturesQuery(module_id));
        let array_lengths = db.query(ComptimeArrayLengthsQuery(module_id));
        let symbols = db.context().symbols();
        let layout_query = |module_id| Some(db.query(SignatureLayoutsQuery(module_id)));
        let local_array_lengths = |id| array_lengths.values.get(&id).copied();
        let program_array_lengths = |id: nia_ids::GlobalConstExprId| {
            Some(db.query(ComptimeArrayLengthsQuery(id.module_id)))
                .and_then(|array_lengths| array_lengths.values.get(&id).copied())
        };
        nia_layout::compute_layouts_with_program_context(
            &defs,
            &type_normalization.interner,
            &item_signatures,
            &type_normalization.normalized,
            &local_array_lengths,
            nia_layout::TargetDataLayout::LP64,
            nia_layout::ProgramLayoutContext {
                symbols: Some(&symbols),
                layouts: Some(&layout_query),
                array_lengths: Some(&program_array_lengths),
                ..Default::default()
            },
        )
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
    let defs = db.query_shared(FullModuleDefsQuery(module_id));
    let function_lowering = db.query_shared(SignatureTypeLoweringQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
    ));
    let function_signatures = db.query(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
    ));
    let type_lowering = db.query_shared(SignatureTypeLoweringQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ));
    let type_signatures = db.query(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ));
    let value_lowering = db.query_shared(SignatureTypeLoweringQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Values,
    ));
    let value_signatures = db.query(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Values,
    ));
    let program = db.query(ProgramAbiSignaturesQuery);
    nia_abi_check::check_module_abi_families_with_program_signatures(
        &defs,
        nia_abi_check::ModuleAbiSignatures {
            functions: &function_signatures.functions,
            function_interner: &function_lowering.interner,
            structs: &type_signatures.structs,
            unions: &type_signatures.unions,
            enums: &type_signatures.enums,
            type_interner: &type_lowering.interner,
            globals: &value_signatures.globals,
            value_interner: &value_lowering.interner,
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
    let active_item_tree = db.query_shared(FullActiveModuleItemTreeQuery(module_id));
    let defs = db.query_shared(FullModuleDefsQuery(module_id));
    let values = db.query(ValueResolutionQuery(module_id));
    let locals = db.query(LocalResolutionQuery(module_id));
    let semantic_uses = db.query(SemanticUseTableQuery(module_id));
    let symbols = db.context().symbols();
    let signatures = db.query(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Values,
    ));
    let comptime = db.query(ComptimeValuesQuery(module_id));
    let program_defs = |module_id| Some(db.query_shared(FullModuleDefsQuery(module_id)));
    let program_comptime_values = |module_id| Some(db.query(ComptimeValuesQuery(module_id)));
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
            comptime: &comptime,
            program_defs: &program_defs,
            program_comptime: &program_comptime_values,
            target: &db.query(CompilerTargetQuery),
        },
    )
}

pub(super) fn provide_flow_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_flow_check::FlowCheck {
    let active_item_tree = db.query_shared(FullActiveModuleItemTreeQuery(module_id));
    let type_lowering = db.query_shared(SignatureTypeLoweringQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
    ));
    let signatures = db.query(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
    ));
    nia_flow_check::check_active_module_flow_with_signatures(
        &active_item_tree,
        &type_lowering.interner,
        nia_flow_check::FlowCheckSignatures {
            functions: &signatures.functions,
        },
    )
}
