// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_abi_check::check_module_abi;
use nia_body_check::{BodyCheckInput, check_module_bodies_with_program_signatures_and_layouts};
use nia_defs::{DefKind, VisibleExtensionMethod, VisibleExtensionMethods, collect_module_defs};
use nia_flow_check::check_module_flow;
use nia_function_ir::{
    FunctionArrayElements, FunctionBlockId, FunctionExpr, FunctionExprKind, FunctionOp,
    FunctionTerminator,
};
use nia_function_lower::{FunctionTypeContext, lower_function_body};
use nia_ids::{DefId, GlobalDefId, LocalId, ModuleIdAllocator};
use nia_item_signatures::{
    ItemSignatureInput, ItemSignatureSource, ProgramFunctionSignature, collect_item_signatures,
};
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_local_resolve::resolve_module_locals;
use nia_node_id::NodeOriginTable;
use nia_parser::parse_module_with_symbols;
use nia_program_signatures::{ProgramSignatureContext, ProgramSignatureLookup};
use nia_sema_ir::SemanticUseTable;
use nia_source::SourcePath;
use nia_static_ir::StaticInit;
use nia_symbol::SymbolId;
use nia_symbol_table::SymbolTable;
use nia_type_lower::{
    ProgramDefsContext, TypeLowering, TypeLoweringContext, lower_module_types_with_context,
};
use nia_type_normalize::normalize_module_types;
use nia_type_resolve::resolve_module_types_with_symbols;
use nia_value_resolve::resolve_module_values;
use std::collections::HashMap;
use std::sync::Arc;

mod test_fixture;
use test_fixture::*;
mod test_assertions;
use test_assertions::*;
mod program_signature_fixture;
use program_signature_fixture::*;
mod program_facts_fixture;
use program_facts_fixture::*;

mod cfg_and_scalar_passes;
mod diagnostics;
mod finalization_contracts;
mod inlining_and_cross_function;
mod local_optimizations;
mod lowering;
mod reachability_and_instances;
mod static_initializers;

fn lower_source(source: &str) -> TestBackendLowering {
    let lowering = lower_source_with_const_mutation(source, |_, _| {});
    assert!(
        lowering.diagnostics.is_empty(),
        "{:?}",
        lowering.diagnostics
    );
    lowering
}

fn lower_source_with_const_mutation(
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

fn lower_source_with_body_mutation_and_optimization(
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

fn lower_source_with_body_mutation_const_mutation_and_optimization(
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

fn lower_source_with_body_mutation_extensions_const_mutation_and_optimization(
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
        optimization,
    )
}

fn lower_source_with_body_check_mutation_and_optimization(
    source: &str,
    mut mutate_body: impl FnMut(&mut nia_function_ir::FunctionBody),
    mutate_extensions: impl FnOnce(
        &mut VisibleExtensionMethods,
        &nia_defs::DefCollection,
        &nia_ty::TypeStore,
        &TypeLowering,
        &ItemSignatures,
    ),
    mutate_const: impl FnOnce(&mut nia_const_check::ConstCheck, &TypeLowering),
    mutate_body_check: impl FnOnce(
        &mut nia_body_check::BodyCheck,
        &nia_ast::Module,
        &nia_defs::DefCollection,
        &ItemSignatures,
        &nia_ty::TypeStoreAppend,
    ),
    optimization: nia_opt::OptimizationPolicy,
) -> TestBackendLowering {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let symbols = SymbolTable::new();
    let (module, errors) = parse_module_with_symbols(source, symbols.clone());
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(module_id, &module);
    let type_resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let type_store = nia_ty::TypeStore::new();
    let program_defs =
        |candidate| (candidate == module_id).then(|| std::sync::Arc::new(defs.clone()));
    let type_lowering = lower_module_types_with_context(
        module_id,
        &module,
        &type_resolved,
        TypeLoweringContext::from_program_defs(
            &type_store,
            ProgramDefsContext {
                defs: Some(&program_defs),
            },
        )
        .with_symbols(&symbols),
    );
    let signatures = collect_item_signatures(ItemSignatureInput {
        source: ItemSignatureSource::Module(&module),
        defs: &defs,
        lowered: &type_lowering,
        type_store: &type_store,
        symbols: None,
    });
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    let active_item_tree = active_item_tree(&module);
    let semantic_uses = semantic_use_table(
        module_id,
        &values,
        &locals,
        &type_lowering,
        &active_item_tree,
    );
    let normalization_input = type_lowering.explicit_type_roots();
    let normalization = normalize_module_types(nia_type_normalize::TypeNormalizationInput {
        module_id,
        type_store: &type_store,
        input_ids: &normalization_input,
        signatures: &signatures,
    });
    let target = nia_target_config::TargetConfig::host();
    let source_path = SourcePath::new("/tmp/nia-backend-lower-test/main.nia");
    let const_module = nia_const_check::lower_module_const(nia_const_check::ConstModuleInput {
        active_item_tree: &active_item_tree,
        defs: &defs,
        signatures: &signatures,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        symbols: &symbols,
        const_exprs: &type_lowering.const_exprs,
        source_path: &source_path,
    });
    assert!(
        const_module.diagnostics.is_empty(),
        "{:?}",
        const_module.diagnostics
    );
    let const_input = nia_const_check::ConstInput {
        type_store: &type_store,
        module: &const_module.module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        symbols: &symbols,
        lowered: &type_lowering,
        signatures: &signatures,
        normalization: &normalization,
        target: &target,
        source_path: &source_path,
        program: nia_const_check::ConstProgramContext::empty(),
    };
    let const_eval = nia_const_check::check_module_const(const_input);
    let root_types = signatures.type_roots();
    let layouts =
        nia_layout::compute_layouts_with_program_context(nia_layout::LayoutComputationInput {
            type_store: &type_store,
            defs: &defs,
            signatures: &signatures,
            root_types: &root_types,
            normalized: &normalization.normalized,
            array_lengths: &|id| const_eval.array_lengths.get(&id).copied(),
            target: nia_layout::TargetDataLayout::LP64,
            program: nia_layout::ProgramLayoutContext::default(),
        });
    let const_array_lengths = nia_const_check::ConstArrayLengths {
        values: const_eval.array_lengths.clone(),
        diagnostics: Vec::new(),
    };
    let const_values = nia_const_check::ConstValues {
        values: const_eval.values.clone(),
        typed_values: const_eval.typed_values.clone(),
        diagnostics: Vec::new(),
    };
    let const_typed_facts = nia_const_check::ConstTypedFacts {
        typed_values: const_eval.typed_values.clone(),
        diagnostics: Vec::new(),
    };
    let body_const = nia_body_check::BodyConst::from_phases(
        &const_values,
        &const_array_lengths,
        &const_typed_facts,
    );
    let mut extensions = VisibleExtensionMethods::default();
    mutate_extensions(
        &mut extensions,
        &defs,
        &type_store,
        &type_lowering,
        &signatures,
    );
    let origins = NodeOriginTable::default();
    let program_signatures = EmptyBodyProgramSignatures::new();
    let mut body_check = check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
        type_store: &type_store,
        source_version: None,
        source_path: &source_path,
        symbols: &symbols,
        origins: &origins,
        active_item_tree: &active_item_tree,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        lowered: &type_lowering,
        signatures: nia_body_check::BodyLocalSignatures::from_item_signatures(&signatures),
        const_signatures: &signatures,
        normalization: &normalization,
        seed: None,
        target: &target,
        const_eval: body_const,
        const_module: &const_module.module,
        layouts: &layouts,
        extensions: &extensions,
        lazy_extensions: None,
        program_extension_methods: &nia_defs::ExtensionMethods::default(),
        program: nia_body_check::BodyProgramContext::empty(),
        program_signatures: program_signatures.context(),
        function_scope: nia_body_check::FunctionCheckScope::LocalModule,
        program_const: nia_body_check::ProgramConstMaps::empty(),
        filter: nia_body_check::BodyCheckFilter::All,
        product: nia_body_check::BodyCheckProduct::Full,
        prechecked: None,
    });
    assert!(
        body_check.diagnostics.is_empty(),
        "{:?}",
        body_check.diagnostics
    );
    let body_types = type_store.append_for_module(module_id);
    mutate_body_check(&mut body_check, &module, &defs, &signatures, &body_types);
    let function_bodies = body_check
        .ir
        .function_bodies
        .iter()
        .map(|(def_id, body)| {
            let mut body = lower_function_body(
                module_id,
                body,
                FunctionTypeContext::for_module(&type_store, module_id),
            )
            .expect("valid typed body")
            .body;
            mutate_body(&mut body);
            (*def_id, body)
        })
        .collect::<HashMap<_, _>>();
    let trait_impl_index = nia_item_signatures::ProgramTraitImplIndex::default();
    let semantic_instantiations = body_check
        .facts
        .iter_generic_instantiations()
        .cloned()
        .collect::<Vec<_>>();
    let monomorphization = nia_monomorphize::collect_monomorphizations(
        &[nia_monomorphize::MonomorphizeModuleInput {
            module_id,
            defs: &defs,
            normalization: &normalization,
            const_eval: &const_eval,
            const_expr_summaries: &type_lowering.const_expr_summaries,
            layouts: Some(&layouts),
            local_enums: &signatures.enums,
            program_enums: &HashMap::new(),
            trait_impls: &[],
            trait_impl_index: &trait_impl_index,
            instantiations: &semantic_instantiations,
        }],
        &type_store,
    );
    assert!(
        monomorphization.diagnostics.is_empty(),
        "{:?}",
        monomorphization.diagnostics
    );
    let function_instance_plan = backend_function_instance_plan(&monomorphization);
    let mut const_eval = const_eval;
    mutate_const(&mut const_eval, &type_lowering);
    let const_array_lengths = nia_const_check::ConstArrayLengths {
        values: const_eval.array_lengths.clone(),
        diagnostics: Vec::new(),
    };
    let program_const = HashMap::from([(module_id, &const_array_lengths)]);
    let const_enum_values = const_enum_values_from_check(&const_eval);
    let program_function_bodies = function_bodies
        .iter()
        .map(|(def_id, body)| (*def_id, body))
        .collect::<HashMap<_, _>>();
    let program_static_inits = body_check
        .ir
        .global_inits
        .iter()
        .map(|(def_id, init)| (*def_id, init.as_ref()))
        .collect::<HashMap<_, _>>();
    let program =
        TestBackendProgramFacts::new(program_const, program_function_bodies, program_static_inits);

    let input = BackendLowerModuleInput {
        module_id,
        source_identity: nia_source::SourceIdentity::new("main"),
        module_name: "main".to_string(),
        symbols: &symbols,
        active_item_tree: &active_item_tree,
        defs: &defs,
        values: &values,
        locals: &locals,
        type_lowering: &type_lowering,
        signatures: &signatures,
        type_normalization: &normalization,
        semantic_facts: &body_check.facts,
        extensions: &extensions,
        const_array_lengths: &const_array_lengths,
        const_enum_values: &const_enum_values,
        layouts: &layouts,
        roots: BackendFunctionRoots::Public,
        reachable_functions: None,
        reachable_globals: None,
        reachable_structs: None,
        reachable_unions: None,
        function_instance_plan: &function_instance_plan,
        program: &program,
    };
    let lowering = lower_backend_program(&[input], &type_store, optimization);
    TestBackendLowering {
        lowering,
        module_id,
        type_store,
    }
}
