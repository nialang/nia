// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_defs::DefKind;
use nia_executable_reachability::{
    ExecutableReachability, ExecutableRootDefs, ReachableModuleInput,
    compute_executable_reachability_with_seed, extend_executable_reachability_from_checked_module,
    filter_semantic_facts_for_reachable_items,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

#[derive(Clone)]
pub(super) struct CompilerQueryProviders {
    pub(super) checked_program: fn(&QueryDb<CompilerContext>) -> CheckedProgram,
    pub(super) entry_checked_program: fn(&QueryDb<CompilerContext>) -> CheckedProgram,
    pub(super) codegen_program: fn(&QueryDb<CompilerContext>) -> CodegenProgram,
    pub(super) module_graph: fn(&QueryDb<CompilerContext>) -> ModuleGraph,
    pub(super) parse_ok_module_ids: fn(&QueryDb<CompilerContext>) -> Vec<ModuleId>,
    pub(super) module_item_tree: fn(&QueryDb<CompilerContext>, ModuleId) -> ModuleItemTree,
    pub(super) active_module_item_tree:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ActiveModuleItemTree,
    pub(super) full_module_item_tree: fn(&QueryDb<CompilerContext>, ModuleId) -> ModuleItemTree,
    pub(super) full_active_module_item_tree:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ActiveModuleItemTree,
    pub(super) module_defs: fn(&QueryDb<CompilerContext>, ModuleId) -> DefCollection,
    pub(super) full_module_defs: fn(&QueryDb<CompilerContext>, ModuleId) -> DefCollection,
    pub(super) defs_by_module: fn(&QueryDb<CompilerContext>) -> Vec<DefCollection>,
    pub(super) public_surface: fn(&QueryDb<CompilerContext>) -> PublicSurfaceQueryValue,
    pub(super) type_resolution: fn(&QueryDb<CompilerContext>, ModuleId) -> TypeResolution,
    pub(super) declaration_type_resolution:
        fn(&QueryDb<CompilerContext>, ModuleId) -> TypeResolution,
    pub(super) signature_type_resolution:
        fn(&QueryDb<CompilerContext>, ModuleId, nia_item_tree::SignatureItemSet) -> TypeResolution,
    pub(super) type_lowering: fn(&QueryDb<CompilerContext>, ModuleId) -> TypeLowering,
    pub(super) declaration_type_lowering: fn(&QueryDb<CompilerContext>, ModuleId) -> TypeLowering,
    pub(super) signature_type_lowering:
        fn(&QueryDb<CompilerContext>, ModuleId, nia_item_tree::SignatureItemSet) -> TypeLowering,
    pub(super) item_signatures: fn(&QueryDb<CompilerContext>, ModuleId) -> ItemSignatures,
    pub(super) signature_item_signatures:
        fn(&QueryDb<CompilerContext>, ModuleId, nia_item_tree::SignatureItemSet) -> ItemSignatures,
    pub(super) type_normalization: fn(&QueryDb<CompilerContext>, ModuleId) -> TypeNormalization,
    pub(super) layout_type_normalization:
        fn(&QueryDb<CompilerContext>, ModuleId) -> TypeNormalization,
    pub(super) signature_type_normalization: fn(
        &QueryDb<CompilerContext>,
        ModuleId,
        nia_item_tree::SignatureItemSet,
    ) -> TypeNormalization,
    pub(super) program_body_function_signatures:
        fn(&QueryDb<CompilerContext>) -> Arc<ProgramBodyFunctionSignatures>,
    pub(super) program_body_value_signatures:
        fn(&QueryDb<CompilerContext>) -> Arc<ProgramBodyValueSignatures>,
    pub(super) program_body_type_signatures:
        fn(&QueryDb<CompilerContext>) -> Arc<ProgramBodyTypeSignatures>,
    pub(super) program_body_trait_signatures:
        fn(&QueryDb<CompilerContext>) -> Arc<ProgramBodyTraitSignatures>,
    pub(super) program_trait_solving_signatures:
        fn(&QueryDb<CompilerContext>) -> Arc<ProgramTraitSolvingSignatures>,
    pub(super) program_trait_impl_signatures:
        fn(&QueryDb<CompilerContext>) -> Arc<Vec<nia_item_signatures::ProgramTraitImplSignature>>,
    pub(super) program_visible_type_signatures:
        fn(&QueryDb<CompilerContext>) -> Arc<ProgramVisibleTypeSignatures>,
    pub(super) program_backend_signatures:
        fn(&QueryDb<CompilerContext>) -> Arc<ProgramBackendSignatures>,
    pub(super) program_abi_signatures:
        fn(&QueryDb<CompilerContext>) -> Arc<ProgramAbiSignaturesValue>,
    pub(super) extension_method_index: fn(&QueryDb<CompilerContext>) -> ExtensionMethodIndexValue,
    pub(super) extension_method_set: fn(&QueryDb<CompilerContext>) -> ExtensionMethodSetValue,
    pub(super) extension_associated_values:
        fn(&QueryDb<CompilerContext>) -> ExtensionAssociatedValuesValue,
    pub(super) extension_methods: fn(&QueryDb<CompilerContext>) -> ExtensionMethodsValue,
    pub(super) visible_extensions:
        fn(&QueryDb<CompilerContext>, ModuleId) -> VisibleExtensionsValue,
    pub(super) value_resolution: fn(&QueryDb<CompilerContext>, ModuleId) -> ValueResolution,
    pub(super) local_resolution: fn(&QueryDb<CompilerContext>, ModuleId) -> LocalResolution,
    pub(super) semantic_use_table:
        fn(&QueryDb<CompilerContext>, ModuleId) -> nia_sema_ir::SemanticUseTable,
    pub(super) comptime_module: fn(&QueryDb<CompilerContext>, ModuleId) -> ComptimeModuleLowering,
    pub(super) comptime: fn(&QueryDb<CompilerContext>, ModuleId) -> ComptimeCheck,
    pub(super) comptime_array_lengths:
        fn(&QueryDb<CompilerContext>, ModuleId) -> nia_comptime_check::ComptimeArrayLengths,
    pub(super) comptime_enum_values:
        fn(&QueryDb<CompilerContext>, ModuleId) -> nia_comptime_check::ComptimeEnumValues,
    pub(super) comptime_values:
        fn(&QueryDb<CompilerContext>, ModuleId) -> nia_comptime_check::ComptimeValues,
    pub(super) comptime_typed_facts:
        fn(&QueryDb<CompilerContext>, ModuleId) -> nia_comptime_check::ComptimeTypedFacts,
    pub(super) layouts: fn(&QueryDb<CompilerContext>, ModuleId) -> nia_layout::Layouts,
    pub(super) abi_check: fn(&QueryDb<CompilerContext>, ModuleId) -> nia_abi_check::AbiCheck,
    pub(super) static_check:
        fn(&QueryDb<CompilerContext>, ModuleId) -> nia_static_check::StaticCheck,
    pub(super) flow_check: fn(&QueryDb<CompilerContext>, ModuleId) -> nia_flow_check::FlowCheck,
    pub(super) body_check: fn(&QueryDb<CompilerContext>, ModuleId) -> nia_body_check::BodyCheck,
    pub(super) checked_module: fn(&QueryDb<CompilerContext>, ModuleId) -> CheckedModule,
    pub(super) checked_modules: fn(&QueryDb<CompilerContext>) -> Vec<CheckedModule>,
    pub(super) monomorphization:
        fn(&QueryDb<CompilerContext>) -> nia_monomorphize::Monomorphization,
    pub(super) backend_lowering:
        fn(&QueryDb<CompilerContext>) -> nia_backend_lower::BackendLowering,
}

impl Default for CompilerQueryProviders {
    fn default() -> Self {
        Self {
            checked_program: provide_checked_program,
            entry_checked_program: provide_entry_checked_program,
            codegen_program: provide_codegen_program,
            module_graph: provide_module_graph,
            parse_ok_module_ids: provide_parse_ok_module_ids,
            module_item_tree: provide_module_item_tree,
            active_module_item_tree: provide_active_module_item_tree,
            full_module_item_tree: provide_full_module_item_tree,
            full_active_module_item_tree: provide_full_active_module_item_tree,
            module_defs: provide_module_defs,
            full_module_defs: provide_full_module_defs,
            defs_by_module: provide_defs_by_module,
            public_surface: provide_public_surface,
            type_resolution: provide_type_resolution,
            declaration_type_resolution: provide_declaration_type_resolution,
            signature_type_resolution: provide_signature_type_resolution,
            type_lowering: provide_type_lowering,
            declaration_type_lowering: provide_declaration_type_lowering,
            signature_type_lowering: provide_signature_type_lowering,
            item_signatures: provide_item_signatures,
            signature_item_signatures: provide_signature_item_signatures,
            type_normalization: provide_type_normalization,
            layout_type_normalization: provide_layout_type_normalization,
            signature_type_normalization: provide_signature_type_normalization,
            program_body_function_signatures: provide_program_body_function_signatures,
            program_body_value_signatures: provide_program_body_value_signatures,
            program_body_type_signatures: provide_program_body_type_signatures,
            program_body_trait_signatures: provide_program_body_trait_signatures,
            program_trait_solving_signatures: provide_program_trait_solving_signatures,
            program_trait_impl_signatures: provide_program_trait_impl_signatures,
            program_visible_type_signatures: provide_program_visible_type_signatures,
            program_backend_signatures: provide_program_backend_signatures,
            program_abi_signatures: provide_program_abi_signatures,
            extension_method_index: provide_extension_method_index,
            extension_method_set: provide_extension_method_set,
            extension_associated_values: provide_extension_associated_values,
            extension_methods: provide_extension_methods,
            visible_extensions: provide_visible_extensions,
            value_resolution: provide_value_resolution,
            local_resolution: provide_local_resolution,
            semantic_use_table: provide_semantic_use_table,
            comptime_module: provide_comptime_module,
            comptime: provide_comptime,
            comptime_array_lengths: provide_comptime_array_lengths,
            comptime_enum_values: provide_comptime_enum_values,
            comptime_values: provide_comptime_values,
            comptime_typed_facts: provide_comptime_typed_facts,
            layouts: provide_layouts,
            abi_check: provide_abi_check,
            static_check: provide_static_check,
            flow_check: provide_flow_check,
            body_check: provide_body_check,
            checked_module: provide_checked_module,
            checked_modules: provide_checked_modules,
            monomorphization: provide_monomorphization,
            backend_lowering: provide_backend_lowering,
        }
    }
}

pub(super) fn provide_checked_program(db: &QueryDb<CompilerContext>) -> CheckedProgram {
    time_provider(db.query(CompilerTimingsQuery), "checked_program", || {
        let graph = db.query(ModuleGraphQuery);
        let optimization = db.query(CompilerOptimizationQuery);
        let mut diagnostics = early_program_diagnostics(db);
        let diagnostic_modules = db.query(CheckedModulesQuery);
        diagnostics.extend(checked_module_diagnostics(db, &diagnostic_modules));
        CheckedProgram {
            graph,
            optimization,
            modules: diagnostic_modules,
            diagnostics,
        }
    })
}

pub(super) fn provide_entry_checked_program(db: &QueryDb<CompilerContext>) -> CheckedProgram {
    time_provider(
        db.query(CompilerTimingsQuery),
        "entry_checked_program",
        || {
            let graph = db.query(ModuleGraphQuery);
            let optimization = db.query(CompilerOptimizationQuery);
            let mut diagnostics = early_program_diagnostics(db);
            let diagnostic_modules = db.query(ExecutableCheckedModulesQuery);
            diagnostics.extend(checked_module_diagnostics(db, &diagnostic_modules));
            CheckedProgram {
                graph,
                optimization,
                modules: diagnostic_modules,
                diagnostics,
            }
        },
    )
}

pub(super) fn provide_codegen_program(db: &QueryDb<CompilerContext>) -> CodegenProgram {
    time_provider(db.query(CompilerTimingsQuery), "codegen_program", || {
        let graph = db.query(ModuleGraphQuery);
        let optimization = db.query(CompilerOptimizationQuery);
        let mut diagnostics = early_program_diagnostics(db);
        let modules = checked_modules_for_codegen(db);
        let diagnostic_modules =
            if db.query(CompilerRuntimeQuery) == RuntimeModel::FreestandingExecutable {
                checked_modules_for_diagnostics(db)
            } else {
                modules.clone()
            };
        diagnostics.extend(checked_module_diagnostics(db, &diagnostic_modules));
        if !diagnostics.is_empty() {
            return CodegenProgram {
                graph,
                optimization,
                modules,
                monomorphization: empty_monomorphization(),
                backend_lowering: empty_backend_lowering(optimization),
                diagnostics,
            };
        }
        let monomorphization = db.query(MonomorphizationQuery);
        diagnostics.extend(monomorphization_diagnostics(&modules, &monomorphization));
        if !diagnostics.is_empty() {
            return CodegenProgram {
                graph,
                optimization,
                modules,
                monomorphization,
                backend_lowering: empty_backend_lowering(optimization),
                diagnostics,
            };
        }
        let backend_lowering = db.query(BackendLoweringQuery);
        diagnostics.extend(backend_lowering_diagnostics(&modules, &backend_lowering));
        CodegenProgram {
            graph,
            optimization,
            modules,
            monomorphization,
            backend_lowering,
            diagnostics,
        }
    })
}

fn empty_monomorphization() -> nia_monomorphize::Monomorphization {
    nia_monomorphize::Monomorphization {
        instances: Vec::new(),
        type_interners: HashMap::new(),
        diagnostics: Vec::new(),
    }
}

fn empty_backend_lowering(optimization: OptimizationPolicy) -> nia_backend_lower::BackendLowering {
    nia_backend_lower::BackendLowering {
        program: nia_backend_ir::BackendProgram {
            modules: Vec::new(),
        },
        optimization,
        optimization_report: nia_backend_lower::BackendOptimizationReport::default(),
        diagnostics: Vec::new(),
    }
}

fn time_provider<T>(timings: TimingMode, name: &str, f: impl FnOnce() -> T) -> T {
    if !timings.detail() {
        return f();
    }
    let start = Instant::now();
    let result = f();
    eprintln!("query timing {name}: {:.3}s", start.elapsed().as_secs_f64());
    result
}

fn time_module_provider<T>(
    db: &QueryDb<CompilerContext>,
    name: &str,
    module_id: ModuleId,
    f: impl FnOnce() -> T,
) -> T {
    let timings = db.query(CompilerTimingsQuery);
    if !timings.detail() {
        return f();
    }
    let path = db.context().path_for_module(module_id);
    let start = Instant::now();
    let result = f();
    eprintln!(
        "query timing {name}[{module_id:?} {}]: {:.3}s",
        path.as_str(),
        start.elapsed().as_secs_f64()
    );
    result
}

pub(super) fn provide_module_graph(db: &QueryDb<CompilerContext>) -> ModuleGraph {
    db.context().module_graph()
}

pub(super) fn provide_parse_ok_module_ids(db: &QueryDb<CompilerContext>) -> Vec<ModuleId> {
    db.query(LoadedModulesQuery)
        .into_iter()
        .filter(|module_id| {
            let parse_errors = db.query(ModuleParseErrorsQuery(*module_id));
            parse_errors.is_empty()
        })
        .collect()
}

pub(super) fn provide_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ModuleItemTree {
    db.query(ModuleItemTreeInputQuery(module_id))
}

pub(super) fn provide_active_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ActiveModuleItemTree {
    let _raw_item_tree = db.query(ModuleItemTreeQuery(module_id));
    db.query(ActiveModuleItemTreeInputQuery(module_id))
}

pub(super) fn provide_full_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ModuleItemTree {
    db.query(FullModuleItemTreeInputQuery(module_id))
}

pub(super) fn provide_full_active_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ActiveModuleItemTree {
    let _raw_item_tree = db.query(FullModuleItemTreeQuery(module_id));
    db.query(FullActiveModuleItemTreeInputQuery(module_id))
}

pub(super) fn provide_module_defs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> DefCollection {
    let item_tree = db.query(ActiveModuleItemTreeQuery(module_id));
    nia_defs::collect_module_defs_from_active_item_tree(module_id, &item_tree)
}

pub(super) fn provide_full_module_defs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> DefCollection {
    let item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
    nia_defs::collect_module_defs_from_active_item_tree(module_id, &item_tree)
}

pub(super) fn provide_defs_by_module(db: &QueryDb<CompilerContext>) -> Vec<DefCollection> {
    db.query_many(
        db.query(ParseOkModuleIdsQuery)
            .into_iter()
            .map(ModuleDefsQuery),
    )
}

pub(super) fn provide_public_surface(db: &QueryDb<CompilerContext>) -> PublicSurfaceQueryValue {
    time_provider(db.query(CompilerTimingsQuery), "public_surface", || {
        let defs = db.query(DefsByModuleQuery);
        let graph = db.query(ModuleGraphQuery);
        let (surfaces, using_scopes, diagnostics) = compute_public_surfaces(&defs, &graph);
        PublicSurfaceQueryValue {
            surfaces,
            using_scopes,
            diagnostics,
        }
    })
}

pub(super) fn provide_type_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeResolution {
    time_module_provider(db, "type_resolution", module_id, || {
        let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
        let defs = db.query(FullModuleDefsQuery(module_id));
        let program_defs = |module_id| Some(db.query(FullModuleDefsQuery(module_id)));
        let graph = db.query(ModuleGraphQuery);
        let public = db.query(PublicSurfaceQuery);
        let empty_using = ModuleUsingScope::default();
        let using_scope = public.using_scopes.get(&module_id).unwrap_or(&empty_using);
        nia_type_resolve::resolve_module_types_from_active_item_tree(
            &active_item_tree,
            &defs,
            nia_type_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&graph),
            },
            &public.surfaces,
            using_scope,
        )
    })
}

pub(super) fn provide_declaration_type_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeResolution {
    time_module_provider(db, "declaration_type_resolution", module_id, || {
        let active_item_tree = db.query(DeclarationActiveModuleItemTreeQuery(module_id));
        let defs = db.query(ModuleDefsQuery(module_id));
        let program_defs = |module_id| Some(db.query(ModuleDefsQuery(module_id)));
        let graph = db.query(ModuleGraphQuery);
        let public = db.query(PublicSurfaceQuery);
        let empty_using = ModuleUsingScope::default();
        let using_scope = public.using_scopes.get(&module_id).unwrap_or(&empty_using);
        nia_type_resolve::resolve_module_types_from_active_item_tree(
            &active_item_tree,
            &defs,
            nia_type_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&graph),
            },
            &public.surfaces,
            using_scope,
        )
    })
}

pub(super) fn provide_signature_type_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> TypeResolution {
    time_module_provider(db, "signature_type_resolution", module_id, || {
        let active_item_tree = db.query(SignatureItemTreeQuery(module_id, set));
        let defs = db.query(ModuleDefsQuery(module_id));
        let program_defs = |module_id| Some(db.query(ModuleDefsQuery(module_id)));
        let graph = db.query(ModuleGraphQuery);
        let public = db.query(PublicSurfaceQuery);
        let empty_using = ModuleUsingScope::default();
        let using_scope = public.using_scopes.get(&module_id).unwrap_or(&empty_using);
        nia_type_resolve::resolve_module_declaration_types_from_active_item_tree(
            &active_item_tree,
            &defs,
            nia_type_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&graph),
            },
            &public.surfaces,
            using_scope,
        )
    })
}

pub(super) fn provide_type_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeLowering {
    let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
    let type_resolution = db.query(TypeResolutionQuery(module_id));
    let program_defs = |module_id| Some(db.query(FullModuleDefsQuery(module_id)));
    nia_type_lower::lower_module_types_from_active_item_tree(
        module_id,
        &active_item_tree,
        &type_resolution,
        nia_type_lower::ProgramDefsContext {
            defs: Some(&program_defs),
        },
    )
}

pub(super) fn provide_declaration_type_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeLowering {
    let active_item_tree = db.query(DeclarationActiveModuleItemTreeQuery(module_id));
    let type_resolution = db.query(DeclarationTypeResolutionQuery(module_id));
    let program_defs = |module_id| Some(db.query(ModuleDefsQuery(module_id)));
    nia_type_lower::lower_module_types_from_active_item_tree(
        module_id,
        &active_item_tree,
        &type_resolution,
        nia_type_lower::ProgramDefsContext {
            defs: Some(&program_defs),
        },
    )
}

pub(super) fn provide_signature_type_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> TypeLowering {
    let active_item_tree = db.query(SignatureItemTreeQuery(module_id, set));
    let type_resolution = db.query(SignatureTypeResolutionQuery(module_id, set));
    let program_defs = |module_id| Some(db.query(ModuleDefsQuery(module_id)));
    nia_type_lower::lower_module_declaration_types_from_active_item_tree(
        module_id,
        &active_item_tree,
        &type_resolution,
        nia_type_lower::ProgramDefsContext {
            defs: Some(&program_defs),
        },
    )
}

pub(super) fn provide_item_signatures(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ItemSignatures {
    let active_item_tree = db.query(DeclarationActiveModuleItemTreeQuery(module_id));
    let defs = db.query(ModuleDefsQuery(module_id));
    let type_lowering = db.query(DeclarationTypeLoweringQuery(module_id));
    nia_item_signatures::collect_item_signatures_from_active_item_tree(
        &active_item_tree,
        &defs,
        &type_lowering,
    )
}

pub(super) fn provide_signature_item_signatures(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> ItemSignatures {
    let active_item_tree = db.query(SignatureItemTreeQuery(module_id, set));
    let defs = db.query(ModuleDefsQuery(module_id));
    let type_lowering = db.query(SignatureTypeLoweringQuery(module_id, set));
    nia_item_signatures::collect_item_signatures_from_active_item_tree(
        &active_item_tree,
        &defs,
        &type_lowering,
    )
}

pub(super) fn provide_type_normalization(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeNormalization {
    let type_lowering = db.query(TypeLoweringQuery(module_id));
    let item_signatures = db.query(ItemSignaturesQuery(module_id));
    nia_type_normalize::normalize_module_types(module_id, &type_lowering.interner, &item_signatures)
}

pub(super) fn provide_layout_type_normalization(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeNormalization {
    let type_lowering = db.query(TypeLoweringQuery(module_id));
    let item_signatures = db.query(ItemSignaturesQuery(module_id));
    nia_type_normalize::normalize_module_types(module_id, &type_lowering.interner, &item_signatures)
}

pub(super) fn provide_signature_type_normalization(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> TypeNormalization {
    let type_lowering = db.query(SignatureTypeLoweringQuery(module_id, set));
    let item_signatures = db.query(SignatureItemSignaturesQuery(module_id, set));
    nia_type_normalize::normalize_module_types(module_id, &type_lowering.interner, &item_signatures)
}

pub(super) fn provide_program_body_function_signatures(
    db: &QueryDb<CompilerContext>,
) -> Arc<ProgramBodyFunctionSignatures> {
    time_provider(
        db.query(CompilerTimingsQuery),
        "program_body_function_signatures",
        || {
            let inputs =
                module_signature_inputs_for(db, nia_item_tree::SignatureItemSet::Functions);
            let modules = inputs.modules();
            let trait_solving = db.query(ProgramTraitSolvingSignaturesQuery);
            Arc::new(ProgramBodyFunctionSignatures {
                functions: collect_program_functions_excluding(
                    &modules,
                    &trait_solving.invalid_trait_impl_method_ids,
                ),
            })
        },
    )
}

pub(super) fn provide_program_body_value_signatures(
    db: &QueryDb<CompilerContext>,
) -> Arc<ProgramBodyValueSignatures> {
    time_provider(
        db.query(CompilerTimingsQuery),
        "program_body_value_signatures",
        || {
            let inputs = module_signature_inputs_for(db, nia_item_tree::SignatureItemSet::Values);
            let modules = inputs.modules();
            Arc::new(ProgramBodyValueSignatures {
                globals: collect_program_globals(&modules),
                comptimes: collect_program_comptimes(&modules),
            })
        },
    )
}

pub(super) fn provide_program_body_type_signatures(
    db: &QueryDb<CompilerContext>,
) -> Arc<ProgramBodyTypeSignatures> {
    time_provider(
        db.query(CompilerTimingsQuery),
        "program_body_type_signatures",
        || {
            let inputs = module_signature_inputs_for(db, nia_item_tree::SignatureItemSet::Types);
            let modules = inputs.modules();
            Arc::new(ProgramBodyTypeSignatures {
                structs: collect_program_structs(&modules),
                unions: collect_program_unions(&modules),
                enums: collect_program_enums(&modules),
                type_aliases: collect_program_type_aliases(&modules),
            })
        },
    )
}

pub(super) fn provide_program_body_trait_signatures(
    db: &QueryDb<CompilerContext>,
) -> Arc<ProgramBodyTraitSignatures> {
    time_provider(
        db.query(CompilerTimingsQuery),
        "program_body_trait_signatures",
        || {
            let inputs = module_signature_inputs_for(db, nia_item_tree::SignatureItemSet::Traits);
            let modules = inputs.modules();
            let trait_solving = db.query(ProgramTraitSolvingSignaturesQuery);
            Arc::new(ProgramBodyTraitSignatures {
                traits: collect_program_traits(&modules),
                trait_impls: trait_solving.trait_impls.clone(),
            })
        },
    )
}

struct ProgramSignatureInputs {
    module_ids: Vec<ModuleId>,
    type_lowerings: Vec<TypeLowering>,
    item_signatures: Vec<ItemSignatures>,
    defs: Vec<DefCollection>,
}

impl ProgramSignatureInputs {
    fn modules(&self) -> Vec<ModuleSignatureInput<'_>> {
        self.module_ids
            .iter()
            .copied()
            .zip(self.type_lowerings.iter())
            .zip(self.item_signatures.iter())
            .zip(self.defs.iter())
            .map(
                |(((module_id, lowering), signatures), defs)| ModuleSignatureInput {
                    module_id,
                    defs,
                    lowering,
                    signatures,
                },
            )
            .collect()
    }
}

struct ExtensionSignatureInputs {
    trait_inputs: ProgramSignatureInputs,
    function_signatures: Vec<ItemSignatures>,
    type_signatures: Vec<ItemSignatures>,
    normalizations: Vec<TypeNormalization>,
}

impl ExtensionSignatureInputs {
    fn modules(&self) -> Vec<ModuleSignatureInput<'_>> {
        self.trait_inputs.modules()
    }

    fn extension_modules(&self) -> Vec<ExtensionModuleInput<'_>> {
        self.trait_inputs
            .module_ids
            .iter()
            .zip(self.trait_inputs.defs.iter())
            .zip(self.trait_inputs.type_lowerings.iter())
            .zip(self.trait_inputs.item_signatures.iter())
            .zip(self.function_signatures.iter())
            .zip(self.type_signatures.iter())
            .zip(self.normalizations.iter())
            .map(
                |(
                    (
                        ((((module_id, defs), lowering), signatures), function_signatures),
                        type_signatures,
                    ),
                    normalization,
                )| {
                    ExtensionModuleInput {
                        module_id: *module_id,
                        defs,
                        lowering,
                        signatures,
                        function_signatures,
                        type_signatures,
                        normalization,
                    }
                },
            )
            .collect()
    }
}

struct ExtensionMethodIndexInputs {
    trait_inputs: ProgramSignatureInputs,
    normalizations: Vec<TypeNormalization>,
}

impl ExtensionMethodIndexInputs {
    fn modules(&self) -> Vec<ExtensionMethodIndexModuleInput<'_>> {
        self.trait_inputs
            .module_ids
            .iter()
            .zip(self.trait_inputs.defs.iter())
            .zip(self.trait_inputs.type_lowerings.iter())
            .zip(self.trait_inputs.item_signatures.iter())
            .zip(self.normalizations.iter())
            .map(
                |((((module_id, defs), lowering), signatures), normalization)| {
                    ExtensionMethodIndexModuleInput {
                        module_id: *module_id,
                        defs,
                        lowering,
                        signatures,
                        normalization,
                    }
                },
            )
            .collect()
    }
}

fn module_signature_inputs_for(
    db: &QueryDb<CompilerContext>,
    set: nia_item_tree::SignatureItemSet,
) -> ProgramSignatureInputs {
    let module_ids = db.query(ParseOkModuleIdsQuery);
    let type_lowerings = module_ids
        .iter()
        .copied()
        .map(|module_id| db.query(SignatureTypeLoweringQuery(module_id, set)))
        .collect::<Vec<_>>();
    let item_signatures = module_ids
        .iter()
        .copied()
        .map(|module_id| db.query(SignatureItemSignaturesQuery(module_id, set)))
        .collect::<Vec<_>>();
    let defs = module_ids
        .iter()
        .copied()
        .map(|module_id| db.query(ModuleDefsQuery(module_id)))
        .collect::<Vec<_>>();
    ProgramSignatureInputs {
        module_ids,
        type_lowerings,
        item_signatures,
        defs,
    }
}

fn extension_signature_inputs(db: &QueryDb<CompilerContext>) -> ExtensionSignatureInputs {
    let timings = db.query(CompilerTimingsQuery);
    let trait_inputs = time_provider(timings, "extension_signature_inputs.traits", || {
        module_signature_inputs_for(db, nia_item_tree::SignatureItemSet::Traits)
    });
    let function_signatures = time_provider(
        timings,
        "extension_signature_inputs.extension_functions",
        || {
            trait_inputs
                .module_ids
                .iter()
                .copied()
                .map(|module_id| {
                    db.query(SignatureItemSignaturesQuery(
                        module_id,
                        nia_item_tree::SignatureItemSet::ExtensionFunctions,
                    ))
                })
                .collect::<Vec<_>>()
        },
    );
    let type_signatures = time_provider(timings, "extension_signature_inputs.types", || {
        trait_inputs
            .module_ids
            .iter()
            .copied()
            .map(|module_id| {
                db.query(SignatureItemSignaturesQuery(
                    module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ))
            })
            .collect::<Vec<_>>()
    });
    let normalizations = time_provider(
        timings,
        "extension_signature_inputs.trait_normalizations",
        || {
            trait_inputs
                .module_ids
                .iter()
                .copied()
                .map(|module_id| {
                    db.query(SignatureTypeNormalizationQuery(
                        module_id,
                        nia_item_tree::SignatureItemSet::Traits,
                    ))
                })
                .collect::<Vec<_>>()
        },
    );
    ExtensionSignatureInputs {
        trait_inputs,
        function_signatures,
        type_signatures,
        normalizations,
    }
}

fn extension_method_index_inputs(db: &QueryDb<CompilerContext>) -> ExtensionMethodIndexInputs {
    let timings = db.query(CompilerTimingsQuery);
    let trait_inputs = time_provider(timings, "extension_method_index.inputs.traits", || {
        module_signature_inputs_for(db, nia_item_tree::SignatureItemSet::Traits)
    });
    let normalizations = time_provider(
        timings,
        "extension_method_index.inputs.trait_normalizations",
        || {
            trait_inputs
                .module_ids
                .iter()
                .copied()
                .map(|module_id| {
                    db.query(SignatureTypeNormalizationQuery(
                        module_id,
                        nia_item_tree::SignatureItemSet::Traits,
                    ))
                })
                .collect::<Vec<_>>()
        },
    );
    ExtensionMethodIndexInputs {
        trait_inputs,
        normalizations,
    }
}

pub(super) fn provide_program_trait_solving_signatures(
    db: &QueryDb<CompilerContext>,
) -> Arc<ProgramTraitSolvingSignatures> {
    time_provider(
        db.query(CompilerTimingsQuery),
        "program_trait_solving_signatures",
        || {
            let inputs = extension_signature_inputs(db);
            let modules = inputs.modules();
            let extension_modules = inputs.extension_modules();
            let invalid_trait_impl_method_ids =
                crate::program_signatures::collect_invalid_trait_impl_method_ids(
                    &extension_modules,
                );
            Arc::new(ProgramTraitSolvingSignatures {
                enums: collect_program_enums(&modules),
                trait_impls: crate::program_signatures::collect_valid_program_trait_impls(
                    &extension_modules,
                ),
                invalid_trait_impl_method_ids,
            })
        },
    )
}

pub(super) fn provide_program_trait_impl_signatures(
    db: &QueryDb<CompilerContext>,
) -> Arc<Vec<nia_item_signatures::ProgramTraitImplSignature>> {
    time_provider(
        db.query(CompilerTimingsQuery),
        "program_trait_impl_signatures",
        || {
            let trait_inputs =
                module_signature_inputs_for(db, nia_item_tree::SignatureItemSet::Traits);
            let trait_modules = trait_inputs.modules();
            Arc::new(collect_program_trait_impls(&trait_modules))
        },
    )
}

pub(super) fn provide_program_visible_type_signatures(
    db: &QueryDb<CompilerContext>,
) -> Arc<ProgramVisibleTypeSignatures> {
    time_provider(
        db.query(CompilerTimingsQuery),
        "program_visible_type_signatures",
        || {
            let inputs = module_signature_inputs_for(db, nia_item_tree::SignatureItemSet::Types);
            let modules = inputs.modules();
            Arc::new(ProgramVisibleTypeSignatures {
                type_aliases: collect_program_type_aliases(&modules),
            })
        },
    )
}

fn executable_program_signatures_without_functions(
    db: &QueryDb<CompilerContext>,
) -> ProgramExecutableSignatures {
    let value_inputs = module_signature_inputs_for(db, nia_item_tree::SignatureItemSet::Values);
    let value_modules = value_inputs.modules();
    let type_inputs = module_signature_inputs_for(db, nia_item_tree::SignatureItemSet::Types);
    let type_modules = type_inputs.modules();
    let trait_inputs = module_signature_inputs_for(db, nia_item_tree::SignatureItemSet::Traits);
    let trait_modules = trait_inputs.modules();
    ProgramExecutableSignatures {
        functions: HashMap::new(),
        globals: collect_program_globals(&value_modules),
        comptimes: collect_program_comptimes(&value_modules),
        structs: collect_program_structs(&type_modules),
        unions: collect_program_unions(&type_modules),
        enums: collect_program_enums(&type_modules),
        type_aliases: collect_program_type_aliases(&type_modules),
        traits: collect_program_traits(&trait_modules),
        trait_impls: collect_program_trait_impls(&trait_modules),
    }
}

fn executable_program_trait_impls(
    db: &QueryDb<CompilerContext>,
) -> Vec<nia_item_signatures::ProgramTraitImplSignature> {
    time_provider(
        db.query(CompilerTimingsQuery),
        "executable_program_trait_impls",
        || (*db.query(ProgramTraitImplSignaturesQuery)).clone(),
    )
}

fn executable_program_functions_for_modules(
    db: &QueryDb<CompilerContext>,
    module_ids: impl IntoIterator<Item = ModuleId>,
) -> HashMap<GlobalDefId, ProgramFunctionSignature> {
    module_ids
        .into_iter()
        .flat_map(|module_id| {
            let signatures = db.query(SignatureItemSignaturesQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Functions,
            ));
            let defs = db.query(ModuleDefsQuery(module_id));
            let interner = db
                .query(SignatureTypeLoweringQuery(
                    module_id,
                    nia_item_tree::SignatureItemSet::Functions,
                ))
                .interner;
            signatures
                .functions
                .into_iter()
                .map(move |(def_id, signature)| {
                    let global_def_id = GlobalDefId { module_id, def_id };
                    let name = defs
                        .defs
                        .get(def_id)
                        .map(|def| def.name.clone())
                        .unwrap_or_else(|| format!("def{}", def_id.0));
                    (
                        global_def_id,
                        ProgramFunctionSignature {
                            name,
                            signature,
                            interner: interner.clone(),
                        },
                    )
                })
        })
        .collect()
}

pub(super) fn provide_program_backend_signatures(
    db: &QueryDb<CompilerContext>,
) -> Arc<ProgramBackendSignatures> {
    time_provider(
        db.query(CompilerTimingsQuery),
        "program_backend_signatures",
        || {
            let function_inputs =
                module_signature_inputs_for(db, nia_item_tree::SignatureItemSet::Functions);
            let function_modules = function_inputs.modules();
            let type_inputs =
                module_signature_inputs_for(db, nia_item_tree::SignatureItemSet::Types);
            let type_modules = type_inputs.modules();
            let trait_inputs =
                module_signature_inputs_for(db, nia_item_tree::SignatureItemSet::Traits);
            let trait_modules = trait_inputs.modules();
            let trait_solving = db.query(ProgramTraitSolvingSignaturesQuery);
            Arc::new(ProgramBackendSignatures {
                functions: collect_program_functions_excluding(
                    &function_modules,
                    &trait_solving.invalid_trait_impl_method_ids,
                ),
                structs: collect_program_structs(&type_modules),
                unions: collect_program_unions(&type_modules),
                enums: trait_solving.enums.clone(),
                traits: collect_program_traits(&trait_modules),
                type_aliases: collect_program_type_aliases(&type_modules),
                trait_impls: trait_solving.trait_impls.clone(),
            })
        },
    )
}

pub(super) fn provide_program_abi_signatures(
    db: &QueryDb<CompilerContext>,
) -> Arc<ProgramAbiSignaturesValue> {
    time_provider(
        db.query(CompilerTimingsQuery),
        "program_abi_signatures",
        || {
            let mut structs = HashMap::new();
            let mut unions = HashMap::new();
            let mut enums = HashMap::new();
            for module_id in db.query(ParseOkModuleIdsQuery) {
                let signatures = db.query(SignatureItemSignaturesQuery(
                    module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ));
                structs.extend(
                    signatures
                        .structs
                        .into_iter()
                        .map(|(def_id, signature)| (GlobalDefId { module_id, def_id }, signature)),
                );
                unions.extend(
                    signatures
                        .unions
                        .into_iter()
                        .map(|(def_id, signature)| (GlobalDefId { module_id, def_id }, signature)),
                );
                enums.extend(
                    signatures
                        .enums
                        .into_iter()
                        .map(|(def_id, signature)| (GlobalDefId { module_id, def_id }, signature)),
                );
            }
            Arc::new(ProgramAbiSignaturesValue {
                structs,
                unions,
                enums,
            })
        },
    )
}

pub(super) fn provide_extension_method_set(
    db: &QueryDb<CompilerContext>,
) -> ExtensionMethodSetValue {
    time_provider(
        db.query(CompilerTimingsQuery),
        "extension_method_set",
        || {
            let timings = db.query(CompilerTimingsQuery);
            let inputs = time_provider(timings, "extension_method_set.inputs", || {
                extension_signature_inputs(db)
            });
            let inputs = time_provider(timings, "extension_method_set.input_modules", || {
                inputs.extension_modules()
            });
            let trait_solving = db.query(ProgramTraitSolvingSignaturesQuery);
            let (methods, diagnostics) =
                time_provider(timings, "extension_method_set.collect", || {
                    collect_extension_methods(&inputs, &trait_solving.trait_impls)
                });
            Arc::new(ExtensionMethodSetQueryValue {
                methods,
                diagnostics,
            })
        },
    )
}

pub(super) fn provide_extension_method_index(
    db: &QueryDb<CompilerContext>,
) -> ExtensionMethodIndexValue {
    time_provider(
        db.query(CompilerTimingsQuery),
        "extension_method_index",
        || {
            let timings = db.query(CompilerTimingsQuery);
            let inputs = time_provider(timings, "extension_method_index.inputs", || {
                extension_method_index_inputs(db)
            });
            let inputs = time_provider(timings, "extension_method_index.input_modules", || {
                inputs.modules()
            });
            let methods = time_provider(timings, "extension_method_index.collect", || {
                collect_extension_method_index(&inputs)
            });
            Arc::new(ExtensionMethodIndexQueryValue { methods })
        },
    )
}

pub(super) fn provide_extension_associated_values(
    db: &QueryDb<CompilerContext>,
) -> ExtensionAssociatedValuesValue {
    time_provider(
        db.query(CompilerTimingsQuery),
        "extension_associated_values",
        || {
            let inputs = extension_method_index_inputs(db);
            let inputs = inputs.modules();
            let (values, diagnostics) = collect_extension_associated_value_index(&inputs);
            Arc::new(ExtensionAssociatedValuesQueryValue {
                values,
                diagnostics,
            })
        },
    )
}

pub(super) fn provide_extension_methods(db: &QueryDb<CompilerContext>) -> ExtensionMethodsValue {
    time_provider(db.query(CompilerTimingsQuery), "extension_methods", || {
        let method_set = db.query(ExtensionMethodSetQuery);
        let associated_values = db.query(ExtensionAssociatedValuesQuery);
        let mut diagnostics = method_set.diagnostics.clone();
        diagnostics.extend(associated_values.diagnostics.iter().cloned());
        Arc::new(ExtensionMethodsQueryValue {
            methods: method_set.methods.clone(),
            associated_values: associated_values.values.clone(),
            diagnostics,
        })
    })
}

pub(super) fn provide_visible_extensions(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> VisibleExtensionsValue {
    let graph = db.query(ModuleGraphQuery);
    let defs = |module_id| Some(db.query(ModuleDefsQuery(module_id)));
    let public = db.query(PublicSurfaceQuery);
    let empty_using = ModuleUsingScope::default();
    let using_scope = public.using_scopes.get(&module_id).unwrap_or(&empty_using);
    let extension_method_normalization = |module_id| {
        Some(db.query(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        )))
    };
    let visible_type_signatures = db.query(ProgramVisibleTypeSignaturesQuery);
    let extension_methods = db.query(ExtensionMethodIndexQuery);
    let associated_values = db.query(ExtensionAssociatedValuesQuery);
    let trait_impls = db.query(ProgramTraitImplSignaturesQuery);
    Arc::new(visible_extensions_for_module(VisibleExtensionsInput {
        module_id,
        graph: &graph,
        using_scope,
        using_scopes: &public.using_scopes,
        public_surfaces: &public.surfaces,
        defs: &defs,
        normalizations: &extension_method_normalization,
        visible_type_signatures: VisibleTypeSignatures {
            type_aliases: &visible_type_signatures.type_aliases,
        },
        extensions: &extension_methods.methods,
        associated_values: &associated_values.values,
        trait_impls: trait_impls.as_slice(),
    }))
}

pub(super) fn provide_value_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ValueResolution {
    time_module_provider(db, "value_resolution", module_id, || {
        let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
        let defs = db.query(FullModuleDefsQuery(module_id));
        let program_defs = |module_id| Some(db.query(FullModuleDefsQuery(module_id)));
        let graph = db.query(ModuleGraphQuery);
        let public = db.query(PublicSurfaceQuery);
        let empty_using = ModuleUsingScope::default();
        let using_scope = public.using_scopes.get(&module_id).unwrap_or(&empty_using);
        let visible_extensions = db.query(VisibleExtensionsQuery(module_id));
        nia_value_resolve::resolve_module_values_from_active_item_tree_with_extensions(
            &active_item_tree,
            &defs,
            nia_value_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&graph),
            },
            &public.surfaces,
            using_scope,
            &visible_extensions.methods,
            &visible_extensions.interner,
        )
    })
}

pub(super) fn provide_local_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> LocalResolution {
    let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
    let defs = db.query(FullModuleDefsQuery(module_id));
    let values = db.query(ValueResolutionQuery(module_id));
    nia_local_resolve::resolve_module_locals_from_active_item_tree_with_origins(
        &active_item_tree,
        &defs,
        &values,
        None,
        &nia_node_id::NodeOriginTable::default(),
    )
}

pub(super) fn provide_semantic_use_table(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_sema_ir::SemanticUseTable {
    let values = db.query(ValueResolutionQuery(module_id));
    let locals = db.query(LocalResolutionQuery(module_id));
    let type_lowering = db.query(TypeLoweringQuery(module_id));
    let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
    semantic_use_table_from_resolution_inputs(
        module_id,
        &active_item_tree,
        &values,
        &locals,
        &type_lowering,
    )
}

fn semantic_use_table_from_resolution_inputs(
    module_id: ModuleId,
    active_item_tree: &ActiveModuleItemTree,
    values: &ValueResolution,
    locals: &LocalResolution,
    type_lowering: &TypeLowering,
) -> nia_sema_ir::SemanticUseTable {
    semantic_use_table_from_resolution_inputs_with_const_expr_values(
        module_id,
        active_item_tree,
        values,
        None,
        None,
        locals,
        type_lowering,
    )
}

fn semantic_use_table_from_resolution_inputs_with_const_expr_values(
    module_id: ModuleId,
    active_item_tree: &ActiveModuleItemTree,
    values: &ValueResolution,
    const_expr_value_resolution: Option<&ValueResolution>,
    const_expr_value_resolution_ids: Option<&HashSet<GlobalConstExprId>>,
    locals: &LocalResolution,
    type_lowering: &TypeLowering,
) -> nia_sema_ir::SemanticUseTable {
    let mut builder = nia_sema_ir::SemanticUseTable::builder();

    for (key, local_use) in &locals.node_uses {
        if let nia_local_resolve::LocalUse::Local(local_id) = local_use {
            builder.insert_node_local_value_use(key.clone(), *local_id);
        }
    }
    builder.extend_node_global_value_uses(
        values
            .node_qualified_values
            .iter()
            .map(|(key, global_id)| (key.clone(), *global_id)),
    );
    builder.extend_node_builtin_associated_values(
        values
            .node_builtin_associated_values
            .iter()
            .map(|(key, value)| (key.clone(), *value)),
    );
    if let Some(const_expr_value_resolution) = const_expr_value_resolution {
        let const_expr_nodes =
            const_expr_node_keys(&type_lowering.const_exprs, const_expr_value_resolution_ids);
        builder.extend_node_global_value_uses(
            const_expr_value_resolution
                .node_qualified_values
                .iter()
                .filter(|(key, _)| const_expr_nodes.contains(*key))
                .map(|(key, global_id)| (key.clone(), *global_id)),
        );
        builder.extend_node_builtin_associated_values(
            const_expr_value_resolution
                .node_builtin_associated_values
                .iter()
                .filter(|(key, _)| const_expr_nodes.contains(*key))
                .map(|(key, value)| (key.clone(), *value)),
        );
        for (key, resolution) in &const_expr_value_resolution.node_names {
            if !const_expr_nodes.contains(key) {
                continue;
            }
            match resolution {
                nia_value_resolve::ValueNameResolution::Def(def_id) => {
                    builder.insert_node_global_value_use(
                        key.clone(),
                        GlobalDefId {
                            module_id,
                            def_id: *def_id,
                        },
                    );
                }
                nia_value_resolve::ValueNameResolution::External(global_id) => {
                    builder.insert_node_global_value_use(key.clone(), *global_id);
                }
                nia_value_resolve::ValueNameResolution::Module
                | nia_value_resolve::ValueNameResolution::LocalDeferred
                | nia_value_resolve::ValueNameResolution::Error => {}
            }
        }
    }
    for (key, resolution) in &values.node_names {
        match resolution {
            nia_value_resolve::ValueNameResolution::Def(def_id) => {
                builder.insert_node_global_value_use(
                    key.clone(),
                    GlobalDefId {
                        module_id,
                        def_id: *def_id,
                    },
                );
            }
            nia_value_resolve::ValueNameResolution::External(global_id) => {
                builder.insert_node_global_value_use(key.clone(), *global_id);
            }
            nia_value_resolve::ValueNameResolution::Module
            | nia_value_resolve::ValueNameResolution::LocalDeferred
            | nia_value_resolve::ValueNameResolution::Error => {}
        }
    }
    builder.extend_node_local_defs(
        locals
            .node_local_defs
            .iter()
            .map(|(key, local_id)| (key.clone(), *local_id)),
    );
    builder.extend_node_type_uses(
        type_lowering.versioned_type_uses_from_active_item_tree(&active_item_tree),
    );
    builder.finish()
}

fn const_expr_node_keys(
    const_exprs: &HashMap<GlobalConstExprId, nia_ast::Expr>,
    ids: Option<&HashSet<GlobalConstExprId>>,
) -> HashSet<nia_node_id::VersionedNodeKey> {
    struct ExprNodeCollector {
        keys: HashSet<nia_node_id::VersionedNodeKey>,
    }

    impl<'ast> nia_ast_walk::Visitor<'ast> for ExprNodeCollector {
        fn visit_expr(&mut self, expr: &'ast nia_ast::Expr) {
            self.keys.insert(expr.node_key.clone());
            nia_ast_walk::walk_expr(self, expr);
        }
    }

    let mut collector = ExprNodeCollector {
        keys: HashSet::new(),
    };
    for (id, expr) in const_exprs {
        if ids.is_some_and(|ids| !ids.contains(id)) {
            continue;
        }
        nia_ast_walk::Visitor::visit_expr(&mut collector, expr);
    }
    collector.keys
}

fn needed_const_exprs_for_active_item_tree(
    active_item_tree: &ActiveModuleItemTree,
    type_lowering: &TypeLowering,
) -> HashSet<GlobalConstExprId> {
    if type_lowering.const_exprs.is_empty() {
        return HashSet::new();
    }
    let candidate_ids = type_lowering
        .const_exprs
        .keys()
        .copied()
        .collect::<HashSet<_>>();
    let mut out = HashSet::new();
    let mut seen = HashSet::new();
    for (_, ty) in type_lowering.versioned_type_uses_from_active_item_tree(active_item_tree) {
        collect_array_len_const_exprs_in_ty(
            &type_lowering.interner,
            ty,
            &candidate_ids,
            &mut out,
            &mut seen,
        );
        if out.len() == candidate_ids.len() {
            break;
        }
    }
    out
}

fn const_expr_subset_for_ids(
    const_exprs: &HashMap<GlobalConstExprId, nia_ast::Expr>,
    ids: &HashSet<GlobalConstExprId>,
) -> HashMap<GlobalConstExprId, nia_ast::Expr> {
    const_exprs
        .iter()
        .filter_map(|(id, expr)| ids.contains(id).then_some((*id, expr.clone())))
        .collect()
}

fn collect_array_len_const_exprs_in_ty(
    interner: &nia_ty::TyInterner,
    ty: InternedTyId,
    candidate_ids: &HashSet<GlobalConstExprId>,
    out: &mut HashSet<GlobalConstExprId>,
    seen: &mut HashSet<InternedTyId>,
) {
    if !seen.insert(ty) {
        return;
    }
    match interner.get(ty) {
        Some(TyKind::Array { len, elem }) => {
            collect_array_len_const_exprs_in_len(interner, len, candidate_ids, out, seen);
            collect_array_len_const_exprs_in_ty(interner, *elem, candidate_ids, out, seen);
        }
        Some(
            TyKind::Optional { elem }
            | TyKind::Pointer { elem, .. }
            | TyKind::VolatilePointer { elem, .. }
            | TyKind::Slice { elem, .. }
            | TyKind::SlicePointee { elem },
        ) => {
            collect_array_len_const_exprs_in_ty(interner, *elem, candidate_ids, out, seen);
        }
        Some(TyKind::ErrorUnion { error, value }) => {
            collect_array_len_const_exprs_in_ty(interner, *error, candidate_ids, out, seen);
            collect_array_len_const_exprs_in_ty(interner, *value, candidate_ids, out, seen);
        }
        Some(TyKind::Range {
            bound: Some(bound), ..
        }) => {
            collect_array_len_const_exprs_in_ty(interner, *bound, candidate_ids, out, seen);
        }
        Some(TyKind::FunctionPointer {
            params,
            return_type,
            ..
        }) => {
            for param in params {
                collect_array_len_const_exprs_in_ty(interner, *param, candidate_ids, out, seen);
            }
            collect_array_len_const_exprs_in_ty(interner, *return_type, candidate_ids, out, seen);
        }
        Some(TyKind::Nominal { args, .. }) => {
            for arg in args {
                collect_array_len_const_exprs_in_ty(interner, *arg, candidate_ids, out, seen);
            }
        }
        Some(TyKind::BuiltinTrait { args, .. }) => {
            for arg in args {
                collect_array_len_const_exprs_in_ty(interner, *arg, candidate_ids, out, seen);
            }
        }
        Some(
            TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            }
            | TyKind::TraitObjectPointee {
                trait_args,
                associated_type_bindings,
                ..
            },
        ) => {
            for arg in trait_args {
                collect_array_len_const_exprs_in_ty(interner, *arg, candidate_ids, out, seen);
            }
            for binding in associated_type_bindings {
                collect_array_len_const_exprs_in_ty(interner, binding.ty, candidate_ids, out, seen);
            }
        }
        Some(TyKind::Projection {
            self_ty,
            trait_args,
            ..
        }) => {
            collect_array_len_const_exprs_in_ty(interner, *self_ty, candidate_ids, out, seen);
            for arg in trait_args {
                collect_array_len_const_exprs_in_ty(interner, *arg, candidate_ids, out, seen);
            }
        }
        Some(
            TyKind::Range { bound: None, .. }
            | TyKind::Error
            | TyKind::ComptimeOnly
            | TyKind::GenericParam(_)
            | TyKind::Primitive(_)
            | TyKind::Vector { .. },
        )
        | None => {}
    }
}

fn collect_array_len_const_exprs_in_len(
    interner: &nia_ty::TyInterner,
    len: &ArrayLenTy,
    candidate_ids: &HashSet<GlobalConstExprId>,
    out: &mut HashSet<GlobalConstExprId>,
    seen: &mut HashSet<InternedTyId>,
) {
    match len {
        ArrayLenTy::ConstExpr(id) => {
            if candidate_ids.contains(id) {
                out.insert(*id);
            }
        }
        ArrayLenTy::Builtin { ty, .. } => {
            collect_array_len_const_exprs_in_ty(interner, *ty, candidate_ids, out, seen);
        }
        ArrayLenTy::Infer | ArrayLenTy::ConstValue(_) => {}
    }
}

fn active_item_tree_for_body_check_filter(
    module_id: ModuleId,
    defs: &DefCollection,
    active_item_tree: &ActiveModuleItemTree,
    filter: nia_body_check::BodyCheckFilter<'_>,
) -> ActiveModuleItemTree {
    ActiveModuleItemTree::new(
        active_item_tree
            .items
            .iter()
            .cloned()
            .map(|mut item| {
                filter_item_tree_node_for_body_check(module_id, defs, &mut item, filter);
                item
            })
            .collect(),
        active_item_tree.inactive_spans.clone(),
    )
}

fn filter_item_tree_node_for_body_check(
    module_id: ModuleId,
    defs: &DefCollection,
    item: &mut nia_item_tree::ItemTreeNode,
    filter: nia_body_check::BodyCheckFilter<'_>,
) {
    match &mut item.kind {
        nia_item_tree::ItemTreeNodeKind::Function(function) => {
            if !function.is_comptime
                && !body_check_filter_includes_function(module_id, defs, &function.node_key, filter)
            {
                function.body = None;
            }
        }
        nia_item_tree::ItemTreeNodeKind::Binding(binding) => {
            if !binding.is_comptime
                && !body_check_filter_includes_global(module_id, defs, &binding.node_key, filter)
            {
                binding.value = None;
            }
        }
        nia_item_tree::ItemTreeNodeKind::Trait(item_trait) => {
            for method in &mut item_trait.methods {
                if !method.function.is_comptime
                    && !body_check_filter_includes_function(
                        module_id,
                        defs,
                        &method.function.node_key,
                        filter,
                    )
                {
                    method.function.body = None;
                }
            }
        }
        nia_item_tree::ItemTreeNodeKind::Extend(extend) => {
            for method in &mut extend.methods {
                if !method.function.is_comptime
                    && !body_check_filter_includes_function(
                        module_id,
                        defs,
                        &method.function.node_key,
                        filter,
                    )
                {
                    method.function.body = None;
                }
            }
        }
        nia_item_tree::ItemTreeNodeKind::Module(_)
        | nia_item_tree::ItemTreeNodeKind::Using(_)
        | nia_item_tree::ItemTreeNodeKind::Struct(_)
        | nia_item_tree::ItemTreeNodeKind::Union(_)
        | nia_item_tree::ItemTreeNodeKind::Enum(_)
        | nia_item_tree::ItemTreeNodeKind::TypeAlias(_) => {}
    }
}

fn body_check_filter_includes_function(
    module_id: ModuleId,
    defs: &DefCollection,
    node_key: &nia_node_id::VersionedNodeKey,
    filter: nia_body_check::BodyCheckFilter<'_>,
) -> bool {
    body_check_filter_includes_def(module_id, defs, node_key, filter, true)
}

fn body_check_filter_includes_global(
    module_id: ModuleId,
    defs: &DefCollection,
    node_key: &nia_node_id::VersionedNodeKey,
    filter: nia_body_check::BodyCheckFilter<'_>,
) -> bool {
    body_check_filter_includes_def(module_id, defs, node_key, filter, false)
}

fn body_check_filter_includes_def(
    module_id: ModuleId,
    defs: &DefCollection,
    node_key: &nia_node_id::VersionedNodeKey,
    filter: nia_body_check::BodyCheckFilter<'_>,
    is_function: bool,
) -> bool {
    let Some(def_id) = defs.def_nodes.get(node_key) else {
        return true;
    };
    let global_def_id = GlobalDefId { module_id, def_id };
    match filter {
        nia_body_check::BodyCheckFilter::All => true,
        nia_body_check::BodyCheckFilter::ReachableFunctions(functions) => {
            !is_function || functions.contains(&global_def_id)
        }
        nia_body_check::BodyCheckFilter::ReachableItems {
            functions,
            globals,
            already_checked_functions,
            already_checked_globals,
        } => {
            if is_function {
                functions.contains(&global_def_id)
                    && already_checked_functions
                        .is_none_or(|checked| !checked.contains(&global_def_id))
            } else {
                globals.contains(&global_def_id)
                    && already_checked_globals
                        .is_none_or(|checked| !checked.contains(&global_def_id))
            }
        }
    }
}

pub(super) fn provide_comptime_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ComptimeModuleLowering {
    let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
    let defs = db.query(FullModuleDefsQuery(module_id));
    let values = db.query(ValueResolutionQuery(module_id));
    let locals = db.query(LocalResolutionQuery(module_id));
    let semantic_uses = db.query(SemanticUseTableQuery(module_id));
    let type_lowering = db.query(TypeLoweringQuery(module_id));
    let source_path = db.query(ModulePathQuery(module_id));
    nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
        active_item_tree: &active_item_tree,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
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
        let comptime = with_comptime_input(db, module_id, |input, module| {
            let mut comptime = nia_comptime_check::check_module_comptime_with_all_phases(
                input,
                array_lengths,
                enum_values,
                values,
                typed_facts,
            );
            comptime.diagnostics.extend(module.diagnostics.clone());
            comptime
        });
        comptime
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

fn with_comptime_input_and_program_signatures<T>(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    program_signatures_override: Option<&ProgramExecutableSignatures>,
    f: impl FnOnce(nia_comptime_check::ComptimeInput<'_>, &ComptimeModuleLowering) -> T,
) -> T {
    let module = db.query(ComptimeModuleQuery(module_id));
    let defs = db.query(FullModuleDefsQuery(module_id));
    let program_module = |module_id| Some(db.query(ComptimeModuleQuery(module_id)).module);
    let program_source_path = |module_id| Some(db.query(ModulePathQuery(module_id)));
    let program_defs = |module_id| Some(db.query(FullModuleDefsQuery(module_id)));
    let program_type_normalization = |module_id| Some(db.query(TypeNormalizationQuery(module_id)));
    let value_type_normalization = |module_id| {
        Some(db.query(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Values,
        )))
    };
    let trait_solving_signatures;
    let (program_enums, trait_impls) = if let Some(signatures) = program_signatures_override {
        (&signatures.enums, signatures.trait_impls.as_slice())
    } else {
        trait_solving_signatures = db.query(ProgramTraitSolvingSignaturesQuery);
        (
            &trait_solving_signatures.enums,
            trait_solving_signatures.trait_impls.as_slice(),
        )
    };
    let item_signatures_for_module = |module_id| Some(db.query(ItemSignaturesQuery(module_id)));
    let value_signatures_for_module = |module_id| {
        Some(db.query(SignatureItemSignaturesQuery(
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
    let type_normalization = db.query(TypeNormalizationQuery(module_id));
    let target = db.query(CompilerTargetQuery);
    f(
        nia_comptime_check::ComptimeInput {
            module: &module.module,
            defs: &defs,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
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
                program_enums,
                trait_impls,
                visible_extensions: Some(&visible_extensions_for_module),
            },
        },
        &module,
    )
}

pub(super) fn provide_layouts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_layout::Layouts {
    time_module_provider(db, "layouts", module_id, || {
        let defs = db.query(FullModuleDefsQuery(module_id));
        let type_normalization = db.query(LayoutTypeNormalizationQuery(module_id));
        let item_signatures = db.query(ItemSignaturesQuery(module_id));
        let array_lengths = db.query(ComptimeArrayLengthsQuery(module_id));
        let layout_query = |module_id| Some(db.query(LayoutsQuery(module_id)));
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
                layouts: Some(&layout_query),
                array_lengths: Some(&program_array_lengths),
                ..Default::default()
            },
        )
    })
}

pub(super) fn provide_abi_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_abi_check::AbiCheck {
    let defs = db.query(FullModuleDefsQuery(module_id));
    let function_lowering = db.query(SignatureTypeLoweringQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
    ));
    let function_signatures = db.query(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
    ));
    let type_lowering = db.query(SignatureTypeLoweringQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ));
    let type_signatures = db.query(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ));
    let value_lowering = db.query(SignatureTypeLoweringQuery(
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
    let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
    let defs = db.query(FullModuleDefsQuery(module_id));
    let values = db.query(ValueResolutionQuery(module_id));
    let locals = db.query(LocalResolutionQuery(module_id));
    let semantic_uses = db.query(SemanticUseTableQuery(module_id));
    let signatures = db.query(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Values,
    ));
    let comptime = db.query(ComptimeValuesQuery(module_id));
    let program_defs = |module_id| Some(db.query(FullModuleDefsQuery(module_id)));
    let program_comptime_values = |module_id| Some(db.query(ComptimeValuesQuery(module_id)));
    nia_static_check::check_module_static_initializers_with_signatures(
        nia_static_check::StaticCheckPreciseInput {
            active_item_tree: &active_item_tree,
            defs: &defs,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
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
    let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
    let type_lowering = db.query(SignatureTypeLoweringQuery(
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

pub(super) fn provide_body_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_body_check::BodyCheck {
    time_module_provider(db, "body_check", module_id, || {
        body_check_with_filter(db, module_id, nia_body_check::BodyCheckFilter::All)
    })
}

fn body_check_with_filter(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    filter: nia_body_check::BodyCheckFilter<'_>,
) -> nia_body_check::BodyCheck {
    body_check_with_filter_and_layouts(db, module_id, filter, None, None, None)
}

fn body_check_resolution_inputs_for_filter(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    filter: nia_body_check::BodyCheckFilter<'_>,
    context: BodyCheckResolutionContext<'_>,
) -> BodyCheckResolutionInputs {
    match filter {
        nia_body_check::BodyCheckFilter::All => BodyCheckResolutionInputs {
            active_item_tree: context.active_item_tree,
            values: db.query(ValueResolutionQuery(module_id)),
            locals: db.query(LocalResolutionQuery(module_id)),
            semantic_uses: db.query(SemanticUseTableQuery(module_id)),
        },
        _ => {
            let filtered_active_item_tree = time_module_provider(
                db,
                "executable_body_check.filter_item_tree",
                module_id,
                || {
                    active_item_tree_for_body_check_filter(
                        module_id,
                        context.defs,
                        &context.active_item_tree,
                        filter,
                    )
                },
            );
            let program_defs = |module_id| Some(db.query(FullModuleDefsQuery(module_id)));
            let public = time_module_provider(
                db,
                "executable_body_check.public_surface",
                module_id,
                || db.query(PublicSurfaceQuery),
            );
            let empty_using = ModuleUsingScope::default();
            let using_scope = public.using_scopes.get(&module_id).unwrap_or(&empty_using);
            let visible_extensions = time_module_provider(
                db,
                "executable_body_check.visible_extensions",
                module_id,
                || db.query(VisibleExtensionsQuery(module_id)),
            );
            let filtered_values = time_module_provider(
                db,
                "executable_body_check.value_resolution",
                module_id,
                || {
                    nia_value_resolve::resolve_module_values_from_active_item_tree_with_extensions(
                        &filtered_active_item_tree,
                        context.defs,
                        nia_value_resolve::ProgramDefsContext {
                            defs: Some(&program_defs),
                            graph: Some(&db.query(ModuleGraphQuery)),
                        },
                        &public.surfaces,
                        using_scope,
                        &visible_extensions.methods,
                        &visible_extensions.interner,
                    )
                },
            );
            let filtered_locals = time_module_provider(
                db,
                "executable_body_check.local_resolution",
                module_id,
                || {
                    nia_local_resolve::resolve_module_locals_from_filtered_active_item_tree_with_origins(
                        &filtered_active_item_tree,
                        &context.active_item_tree,
                        context.defs,
                        &filtered_values,
                        Some(context.source_version),
                        context.origins,
                    )
                },
            );
            let filtered_semantic_uses =
                time_module_provider(db, "executable_body_check.semantic_uses", module_id, || {
                    let needed_const_exprs = needed_const_exprs_for_active_item_tree(
                        &filtered_active_item_tree,
                        context.lowered,
                    );
                    let const_expr_value_resolution = time_module_provider(
                        db,
                        "executable_body_check.const_expr_value_resolution",
                        module_id,
                        || {
                            nia_value_resolve::resolve_module_values_from_exprs_with_extensions(
                                context.lowered.const_exprs.iter().filter_map(|(id, expr)| {
                                    needed_const_exprs.contains(id).then_some(expr.clone())
                                }),
                                context.defs,
                                nia_value_resolve::ProgramDefsContext {
                                    defs: Some(&program_defs),
                                    graph: Some(&db.query(ModuleGraphQuery)),
                                },
                                &public.surfaces,
                                using_scope,
                                &visible_extensions.methods,
                                &visible_extensions.interner,
                            )
                        },
                    );
                    semantic_use_table_from_resolution_inputs_with_const_expr_values(
                        module_id,
                        &filtered_active_item_tree,
                        &filtered_values,
                        Some(&const_expr_value_resolution),
                        Some(&needed_const_exprs),
                        &filtered_locals,
                        context.lowered,
                    )
                });
            BodyCheckResolutionInputs {
                active_item_tree: filtered_active_item_tree,
                values: filtered_values,
                locals: filtered_locals,
                semantic_uses: filtered_semantic_uses,
            }
        }
    }
}

struct BodyCheckResolutionInputs {
    active_item_tree: ActiveModuleItemTree,
    values: ValueResolution,
    locals: LocalResolution,
    semantic_uses: nia_sema_ir::SemanticUseTable,
}

struct BodyCheckWithResolutionInputs {
    body_check: nia_body_check::BodyCheck,
    inputs: BodyCheckResolutionInputs,
    stored_inputs: Option<BodyCheckResolutionInputs>,
    comptime: Option<ComptimeCheck>,
}

struct BodyCheckResolutionContext<'a> {
    source_version: nia_source::SourceVersion,
    origins: &'a nia_node_id::NodeOriginTable,
    active_item_tree: ActiveModuleItemTree,
    defs: &'a DefCollection,
    lowered: &'a TypeLowering,
}

#[derive(Clone)]
struct ExecutableCheckedModuleState {
    module: CheckedModule,
    checked_functions: HashSet<GlobalDefId>,
    checked_globals: HashSet<GlobalDefId>,
}

struct BodyCheckComptimeInputs {
    module: ComptimeModuleLowering,
    array_lengths: nia_comptime_check::ComptimeArrayLengths,
    enum_values: nia_comptime_check::ComptimeEnumValues,
    values: nia_comptime_check::ComptimeValues,
    typed_facts: nia_comptime_check::ComptimeTypedFacts,
}

impl BodyCheckComptimeInputs {
    fn into_check(self) -> ComptimeCheck {
        ComptimeCheck {
            interner: self.typed_facts.interner,
            values: self.values.values,
            typed_values: self.typed_facts.typed_values,
            enum_values: self.enum_values.values,
            typed_enum_values: self.enum_values.typed_values,
            array_lengths: self.array_lengths.values,
            diagnostics: self.typed_facts.diagnostics,
        }
    }
}

fn filtered_comptime_global_initializer_for_body_check(
    db: &QueryDb<CompilerContext>,
    global_id: GlobalDefId,
) -> Option<nia_comptime_ir::ResolvedComptimeExpr> {
    let defs = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.defs",
        global_id.module_id,
        || db.query(FullModuleDefsQuery(global_id.module_id)),
    );
    let source_path = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.source_path",
        global_id.module_id,
        || db.query(ModulePathQuery(global_id.module_id)),
    );
    let active_item_tree = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.active_item_tree",
        global_id.module_id,
        || db.query(FullActiveModuleItemTreeQuery(global_id.module_id)),
    );
    let filtered_active_item_tree = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.filter_item_tree",
        global_id.module_id,
        || {
            active_item_tree_for_body_check_filter(
                global_id.module_id,
                &defs,
                &active_item_tree,
                nia_body_check::BodyCheckFilter::ReachableItems {
                    functions: &HashSet::new(),
                    globals: &HashSet::from([global_id]),
                    already_checked_functions: None,
                    already_checked_globals: None,
                },
            )
        },
    );
    let program_defs = |module_id| Some(db.query(FullModuleDefsQuery(module_id)));
    let public = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.public_surface",
        global_id.module_id,
        || db.query(PublicSurfaceQuery),
    );
    let empty_using = ModuleUsingScope::default();
    let using_scope = public
        .using_scopes
        .get(&global_id.module_id)
        .unwrap_or(&empty_using);
    let source_version = db.query(ModuleSourceVersionQuery(global_id.module_id));
    let origins = db.query(ModuleOriginsQuery(global_id.module_id));
    let lowered = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.type_lowering",
        global_id.module_id,
        || db.query(TypeLoweringQuery(global_id.module_id)),
    );
    let needed_const_exprs = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.needed_const_exprs",
        global_id.module_id,
        || needed_const_exprs_for_active_item_tree(&filtered_active_item_tree, &lowered),
    );
    let const_expr_value_resolution = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.const_expr_value_resolution",
        global_id.module_id,
        || {
            let visible_extensions = db.query(VisibleExtensionsQuery(global_id.module_id));
            nia_value_resolve::resolve_module_values_from_exprs_with_extensions(
                lowered.const_exprs.iter().filter_map(|(id, expr)| {
                    needed_const_exprs.contains(id).then_some(expr.clone())
                }),
                &defs,
                nia_value_resolve::ProgramDefsContext {
                    defs: Some(&program_defs),
                    graph: Some(&db.query(ModuleGraphQuery)),
                },
                &public.surfaces,
                using_scope,
                &visible_extensions.methods,
                &visible_extensions.interner,
            )
        },
    );
    let filtered_const_exprs = const_expr_subset_for_ids(&lowered.const_exprs, &needed_const_exprs);
    let lower_with_values = |values: ValueResolution| {
        let locals = time_module_provider(
            db,
            "executable_body_check.comptime.global_initializer.local_resolution",
            global_id.module_id,
            || {
                nia_local_resolve::resolve_module_locals_from_filtered_active_item_tree_with_origins(
                    &filtered_active_item_tree,
                    &active_item_tree,
                    &defs,
                    &values,
                    Some(source_version),
                    &origins,
                )
            },
        );
        let semantic_uses = time_module_provider(
            db,
            "executable_body_check.comptime.global_initializer.semantic_uses",
            global_id.module_id,
            || {
                semantic_use_table_from_resolution_inputs_with_const_expr_values(
                    global_id.module_id,
                    &filtered_active_item_tree,
                    &values,
                    Some(&const_expr_value_resolution),
                    Some(&needed_const_exprs),
                    &locals,
                    &lowered,
                )
            },
        );
        let lowered = time_module_provider(
            db,
            "executable_body_check.comptime.global_initializer.lower_module",
            global_id.module_id,
            || {
                nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
                    active_item_tree: &filtered_active_item_tree,
                    defs: &defs,
                    values: &values,
                    locals: &locals,
                    semantic_uses: &semantic_uses,
                    const_exprs: &filtered_const_exprs,
                    source_path: &source_path,
                })
            },
        );
        lowered
            .module
            .global_initializers()
            .get(&global_id)
            .cloned()
    };
    let values_without_extensions = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.value_resolution",
        global_id.module_id,
        || {
            nia_value_resolve::resolve_module_values_from_active_item_tree(
                &filtered_active_item_tree,
                &defs,
                nia_value_resolve::ProgramDefsContext {
                    defs: Some(&program_defs),
                    graph: Some(&db.query(ModuleGraphQuery)),
                },
                &public.surfaces,
                using_scope,
            )
        },
    );
    if let Some(initializer) = lower_with_values(values_without_extensions) {
        return Some(initializer);
    }
    let visible_extensions = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.visible_extensions",
        global_id.module_id,
        || db.query(VisibleExtensionsQuery(global_id.module_id)),
    );
    let values = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.value_resolution_with_extensions",
        global_id.module_id,
        || {
            nia_value_resolve::resolve_module_values_from_active_item_tree_with_extensions(
                &filtered_active_item_tree,
                &defs,
                nia_value_resolve::ProgramDefsContext {
                    defs: Some(&program_defs),
                    graph: Some(&db.query(ModuleGraphQuery)),
                },
                &public.surfaces,
                using_scope,
                &visible_extensions.methods,
                &visible_extensions.interner,
            )
        },
    );
    lower_with_values(values)
}

fn comptime_inputs_for_body_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    defs: &DefCollection,
    source_path: &SourcePath,
    signatures: &ItemSignatures,
    normalization: &TypeNormalization,
    lowered: &TypeLowering,
    inputs: &BodyCheckResolutionInputs,
    program_signatures_override: Option<&ProgramExecutableSignatures>,
    global_initializer_cache: Option<
        &RefCell<HashMap<GlobalDefId, Option<nia_comptime_ir::ResolvedComptimeExpr>>>,
    >,
) -> BodyCheckComptimeInputs {
    let needed_const_exprs =
        needed_const_exprs_for_active_item_tree(&inputs.active_item_tree, lowered);
    let filtered_const_exprs = const_expr_subset_for_ids(&lowered.const_exprs, &needed_const_exprs);
    let module = time_module_provider(
        db,
        "executable_body_check.comptime.lower_module",
        module_id,
        || {
            nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
                active_item_tree: &inputs.active_item_tree,
                defs,
                values: &inputs.values,
                locals: &inputs.locals,
                semantic_uses: &inputs.semantic_uses,
                const_exprs: &filtered_const_exprs,
                source_path,
            })
        },
    );
    let program_module = |module_id| Some(db.query(ComptimeModuleQuery(module_id)).module);
    let program_source_path = |module_id| Some(db.query(ModulePathQuery(module_id)));
    let program_defs = |module_id| Some(db.query(FullModuleDefsQuery(module_id)));
    let program_type_normalization = |module_id| Some(db.query(TypeNormalizationQuery(module_id)));
    let value_type_normalization = |module_id| {
        Some(db.query(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Values,
        )))
    };
    let trait_solving_signatures;
    let (program_enums, trait_impls) = if let Some(signatures) = program_signatures_override {
        (&signatures.enums, signatures.trait_impls.as_slice())
    } else {
        trait_solving_signatures = db.query(ProgramTraitSolvingSignaturesQuery);
        (
            &trait_solving_signatures.enums,
            trait_solving_signatures.trait_impls.as_slice(),
        )
    };
    let item_signatures_for_module = |module_id| Some(db.query(ItemSignaturesQuery(module_id)));
    let value_signatures_for_module = |module_id| {
        Some(db.query(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Values,
        )))
    };
    let visible_extensions_for_module =
        |module_id| Some(db.query(VisibleExtensionsQuery(module_id)).methods.clone());
    let program_global_initializer = |global_id| {
        if let Some(cache) = global_initializer_cache {
            if !cache.borrow().contains_key(&global_id) {
                let initializer =
                    filtered_comptime_global_initializer_for_body_check(db, global_id);
                cache.borrow_mut().insert(global_id, initializer);
            }
            return cache.borrow().get(&global_id).cloned().flatten();
        }
        filtered_comptime_global_initializer_for_body_check(db, global_id)
    };
    let target = db.query(CompilerTargetQuery);
    let comptime_input = nia_comptime_check::ComptimeInput {
        module: &module.module,
        defs,
        values: &inputs.values,
        locals: &inputs.locals,
        semantic_uses: &inputs.semantic_uses,
        signatures,
        interner: &normalization.interner,
        normalized: &normalization.normalized,
        target: &target,
        source_path,
        program: nia_comptime_check::ComptimeProgramContext {
            module: Some(&program_module),
            source_path: Some(&program_source_path),
            defs: Some(&program_defs),
            type_normalizations: Some(&program_type_normalization),
            value_type_normalizations: Some(&value_type_normalization),
            signatures: Some(&item_signatures_for_module),
            value_signatures: Some(&value_signatures_for_module),
            comptime_values: None,
            global_initializer: Some(&program_global_initializer),
            program_enums,
            trait_impls,
            visible_extensions: Some(&visible_extensions_for_module),
        },
    };
    let mut array_lengths = time_module_provider(
        db,
        "executable_body_check.comptime.array_lengths",
        module_id,
        || nia_comptime_check::compute_module_comptime_array_lengths(comptime_input),
    );
    array_lengths.diagnostics.extend(module.diagnostics.clone());
    let enum_values = time_module_provider(
        db,
        "executable_body_check.comptime.enum_values",
        module_id,
        || {
            nia_comptime_check::compute_module_comptime_enum_values(
                comptime_input,
                array_lengths.clone(),
            )
        },
    );
    let values = time_module_provider(
        db,
        "executable_body_check.comptime.values",
        module_id,
        || {
            nia_comptime_check::compute_module_comptime_values(
                comptime_input,
                array_lengths.clone(),
                enum_values.clone(),
            )
        },
    );
    let typed_facts = time_module_provider(
        db,
        "executable_body_check.comptime.typed_facts",
        module_id,
        || {
            nia_comptime_check::compute_module_comptime_typed_facts(
                comptime_input,
                array_lengths.clone(),
                enum_values.clone(),
                values.clone(),
            )
        },
    );
    BodyCheckComptimeInputs {
        module,
        array_lengths,
        enum_values,
        values,
        typed_facts,
    }
}

fn body_check_with_filter_and_layouts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    filter: nia_body_check::BodyCheckFilter<'_>,
    layouts: Option<nia_layout::Layouts>,
    program_layouts_override: Option<&dyn Fn(ModuleId) -> Option<nia_layout::Layouts>>,
    program_signatures_override: Option<&ProgramExecutableSignatures>,
) -> nia_body_check::BodyCheck {
    body_check_with_filter_and_layouts_with_inputs(
        db,
        module_id,
        filter,
        layouts,
        program_layouts_override,
        program_signatures_override,
        None,
        None,
        None,
        None,
    )
    .body_check
}

fn body_check_with_filter_and_layouts_with_inputs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    filter: nia_body_check::BodyCheckFilter<'_>,
    layouts: Option<nia_layout::Layouts>,
    program_layouts_override: Option<&dyn Fn(ModuleId) -> Option<nia_layout::Layouts>>,
    program_signatures_override: Option<&ProgramExecutableSignatures>,
    resolution_inputs: Option<BodyCheckResolutionInputs>,
    stored_inputs: Option<BodyCheckResolutionInputs>,
    seed_interner: Option<nia_ty::TyInterner>,
    global_initializer_cache: Option<
        &RefCell<HashMap<GlobalDefId, Option<nia_comptime_ir::ResolvedComptimeExpr>>>,
    >,
) -> BodyCheckWithResolutionInputs {
    let source_version = db.query(ModuleSourceVersionQuery(module_id));
    let origins = db.query(ModuleOriginsQuery(module_id));
    let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
    let defs = db.query(FullModuleDefsQuery(module_id));
    let program_defs = |module_id| Some(db.query(FullModuleDefsQuery(module_id)));
    let lowered = db.query(TypeLoweringQuery(module_id));
    let inputs = resolution_inputs.unwrap_or_else(|| {
        body_check_resolution_inputs_for_filter(
            db,
            module_id,
            filter,
            BodyCheckResolutionContext {
                source_version,
                origins: &origins,
                active_item_tree,
                defs: &defs,
                lowered: &lowered,
            },
        )
    });
    let source_path = db.query(ModulePathQuery(module_id));
    let signatures = body_local_item_signatures(db, module_id, &lowered);
    let normalization = db.query(TypeNormalizationQuery(module_id));
    let program_type_normalization = |module_id| Some(db.query(TypeNormalizationQuery(module_id)));
    let extension_method_normalization = |module_id| {
        Some(db.query(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        )))
    };
    let mut filtered_comptime_inputs = None;
    let full_comptime_values;
    let full_comptime_array_lengths;
    let full_comptime_typed_facts;
    let full_comptime_module;
    let (body_comptime, comptime_module) = match filter {
        nia_body_check::BodyCheckFilter::All => {
            full_comptime_values = db.query(ComptimeValuesQuery(module_id));
            full_comptime_array_lengths = db.query(ComptimeArrayLengthsQuery(module_id));
            full_comptime_typed_facts = db.query(ComptimeTypedFactsQuery(module_id));
            full_comptime_module = db.query(ComptimeModuleQuery(module_id));
            (
                nia_body_check::BodyComptime::from_phases(
                    &full_comptime_values,
                    &full_comptime_array_lengths,
                    &full_comptime_typed_facts,
                ),
                &full_comptime_module.module,
            )
        }
        _ => {
            filtered_comptime_inputs = Some(time_module_provider(
                db,
                "executable_body_check.comptime_inputs",
                module_id,
                || {
                    comptime_inputs_for_body_check(
                        db,
                        module_id,
                        &defs,
                        &source_path,
                        &signatures,
                        &normalization,
                        &lowered,
                        &inputs,
                        program_signatures_override,
                        global_initializer_cache,
                    )
                },
            ));
            let filtered = filtered_comptime_inputs
                .as_ref()
                .expect("filtered comptime inputs must be initialized");
            (
                nia_body_check::BodyComptime::from_phases(
                    &filtered.values,
                    &filtered.array_lengths,
                    &filtered.typed_facts,
                ),
                &filtered.module.module,
            )
        }
    };
    let layouts = layouts.unwrap_or_else(|| db.query(LayoutsQuery(module_id)));
    let program_layouts = |module_id| {
        program_layouts_override
            .and_then(|program_layouts| program_layouts(module_id))
            .or_else(|| Some(db.query(LayoutsQuery(module_id))))
    };
    let extensions = db.query(VisibleExtensionsQuery(module_id));
    let extension_methods = db.query(ExtensionMethodIndexQuery);
    let program_function_signature = |def_id: GlobalDefId| {
        db.query(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ))
        .functions
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramFunctionSignature {
            name: db
                .query(ModuleDefsQuery(def_id.module_id))
                .defs
                .get(def_id.def_id)
                .map(|def| def.name.clone())
                .unwrap_or_else(|| format!("def{}", def_id.def_id.0)),
            signature,
            interner: db
                .query(SignatureTypeLoweringQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Functions,
                ))
                .interner,
        })
    };
    let empty_program_functions = HashMap::new();
    let program_function_signatures;
    let program_value_signatures;
    let program_type_signatures;
    let program_trait_signatures;
    let (program_functions, program_values, program_types, program_traits) =
        if let Some(signatures) = program_signatures_override {
            (
                &empty_program_functions,
                nia_body_check::BodyProgramValueSignatures {
                    globals: &signatures.globals,
                    comptimes: &signatures.comptimes,
                },
                nia_body_check::BodyProgramTypeSignatures {
                    structs: &signatures.structs,
                    unions: &signatures.unions,
                    enums: &signatures.enums,
                    type_aliases: &signatures.type_aliases,
                },
                nia_body_check::BodyProgramTraitSignatures {
                    traits: &signatures.traits,
                    trait_impls: &signatures.trait_impls,
                },
            )
        } else {
            program_function_signatures = db.query(ProgramBodyFunctionSignaturesQuery);
            program_value_signatures = db.query(ProgramBodyValueSignaturesQuery);
            program_type_signatures = db.query(ProgramBodyTypeSignaturesQuery);
            program_trait_signatures = db.query(ProgramBodyTraitSignaturesQuery);
            (
                &program_function_signatures.functions,
                program_value_signatures.body_maps(),
                program_type_signatures.body_maps(),
                program_trait_signatures.body_maps(),
            )
        };
    let item_signatures_for_module = |module_id| Some(db.query(ItemSignaturesQuery(module_id)));
    let executable_program_comptime_array_lengths =
        RefCell::new(HashMap::<ModuleId, nia_comptime_check::ComptimeArrayLengths>::new());
    let executable_program_comptime_values =
        RefCell::new(HashMap::<ModuleId, nia_comptime_check::ComptimeValues>::new());
    let program_comptime_array_lengths = |module_id| {
        if let Some(signatures) = program_signatures_override {
            if !executable_program_comptime_array_lengths
                .borrow()
                .contains_key(&module_id)
            {
                let array_lengths = with_comptime_input_and_program_signatures(
                    db,
                    module_id,
                    Some(signatures),
                    |input, module| {
                        let mut array_lengths =
                            nia_comptime_check::compute_module_comptime_array_lengths(input);
                        array_lengths.diagnostics.extend(module.diagnostics.clone());
                        array_lengths
                    },
                );
                executable_program_comptime_array_lengths
                    .borrow_mut()
                    .insert(module_id, array_lengths);
            }
            return executable_program_comptime_array_lengths
                .borrow()
                .get(&module_id)
                .cloned();
        }
        Some(db.query(ComptimeArrayLengthsQuery(module_id)))
    };
    let program_comptime_values = |module_id| {
        if let Some(signatures) = program_signatures_override {
            if !executable_program_comptime_values
                .borrow()
                .contains_key(&module_id)
            {
                let array_lengths = program_comptime_array_lengths(module_id)?;
                let enum_values = with_comptime_input_and_program_signatures(
                    db,
                    module_id,
                    Some(signatures),
                    |input, module| {
                        let mut enum_values =
                            nia_comptime_check::compute_module_comptime_enum_values(
                                input,
                                array_lengths.clone(),
                            );
                        enum_values.diagnostics.extend(module.diagnostics.clone());
                        enum_values
                    },
                );
                let values = with_comptime_input_and_program_signatures(
                    db,
                    module_id,
                    Some(signatures),
                    |input, module| {
                        let mut values = nia_comptime_check::compute_module_comptime_values(
                            input,
                            array_lengths,
                            enum_values,
                        );
                        values.diagnostics.extend(module.diagnostics.clone());
                        values
                    },
                );
                executable_program_comptime_values
                    .borrow_mut()
                    .insert(module_id, values);
            }
            return executable_program_comptime_values
                .borrow()
                .get(&module_id)
                .cloned();
        }
        Some(db.query(ComptimeValuesQuery(module_id)))
    };
    let program_comptime_module = |module_id| Some(db.query(ComptimeModuleQuery(module_id)).module);
    let program_visible_extensions =
        |module_id| Some(db.query(VisibleExtensionsQuery(module_id)).methods.clone());
    let run_body_check = |inputs: &BodyCheckResolutionInputs,
                          body_comptime: nia_body_check::BodyComptime<'_>,
                          comptime_module: &nia_comptime_ir::ResolvedComptimeModule,
                          filter: nia_body_check::BodyCheckFilter<'_>| {
        nia_body_check::check_module_bodies_with_program_signatures_and_layouts_with_timings(
            nia_body_check::BodyCheckInput {
                source_version: Some(source_version),
                source_path: &source_path,
                origins: &origins,
                active_item_tree: &inputs.active_item_tree,
                defs: &defs,
                values: &inputs.values,
                locals: &inputs.locals,
                semantic_uses: &inputs.semantic_uses,
                lowered: &lowered,
                signatures: nia_body_check::BodyLocalSignatures::from_item_signatures(&signatures),
                comptime_signatures: &signatures,
                normalization: &normalization,
                seed_interner: seed_interner.clone(),
                target: &db.query(CompilerTargetQuery),
                comptime: body_comptime,
                comptime_module,
                layouts: &layouts,
                extensions: &extensions.methods,
                program_extension_methods: &extension_methods.methods,
                extension_interner: Some(&extensions.interner),
                program: nia_body_check::BodyProgramContext {
                    defs: Some(&program_defs),
                    type_normalizations: Some(&program_type_normalization),
                    extension_type_normalizations: Some(&extension_method_normalization),
                    signatures: Some(&item_signatures_for_module),
                    layouts: Some(&program_layouts),
                    visible_extensions: Some(&program_visible_extensions),
                },
                program_functions,
                program_function_signature: Some(
                    &program_function_signature
                        as &dyn Fn(GlobalDefId) -> Option<ProgramFunctionSignature>,
                ),
                program_values,
                program_types,
                program_traits,
                function_scope: nia_body_check::FunctionCheckScope::ProgramSignatures,
                program_comptime: nia_body_check::ProgramComptimeMaps {
                    values: &program_comptime_values,
                    array_lengths: &program_comptime_array_lengths,
                    module: &program_comptime_module,
                },
                filter,
            },
            body_timing_mode(db.query(CompilerTimingsQuery)),
        )
    };
    let body_check = run_body_check(&inputs, body_comptime, comptime_module, filter);
    let (body_check, stored_inputs, stored_comptime_inputs) = match (filter, stored_inputs) {
        (
            nia_body_check::BodyCheckFilter::ReachableItems {
                functions, globals, ..
            },
            Some(stored_inputs),
        ) => {
            let mut body_check = body_check;
            let mut final_functions = functions.iter().copied().collect::<HashSet<_>>();
            let mut current_inputs = stored_inputs;
            let mut current_comptime_inputs = None;
            loop {
                let before = final_functions.len();
                final_functions.extend(body_check.ir.function_bodies.keys().copied());
                if final_functions.len() == before {
                    break;
                }
                let final_filter = nia_body_check::BodyCheckFilter::ReachableItems {
                    functions: &final_functions,
                    globals,
                    already_checked_functions: None,
                    already_checked_globals: None,
                };
                let final_inputs = body_check_resolution_inputs_for_filter(
                    db,
                    module_id,
                    final_filter,
                    BodyCheckResolutionContext {
                        source_version,
                        origins: &origins,
                        active_item_tree: db.query(FullActiveModuleItemTreeQuery(module_id)),
                        defs: &defs,
                        lowered: &lowered,
                    },
                );
                let final_comptime_inputs = time_module_provider(
                    db,
                    "executable_body_check.final_comptime_inputs",
                    module_id,
                    || {
                        comptime_inputs_for_body_check(
                            db,
                            module_id,
                            &defs,
                            &source_path,
                            &signatures,
                            &normalization,
                            &lowered,
                            &final_inputs,
                            program_signatures_override,
                            global_initializer_cache,
                        )
                    },
                );
                let final_body_comptime = nia_body_check::BodyComptime::from_phases(
                    &final_comptime_inputs.values,
                    &final_comptime_inputs.array_lengths,
                    &final_comptime_inputs.typed_facts,
                );
                body_check = run_body_check(
                    &final_inputs,
                    final_body_comptime,
                    &final_comptime_inputs.module.module,
                    final_filter,
                );
                current_inputs = final_inputs;
                current_comptime_inputs = Some(final_comptime_inputs);
            }
            (body_check, Some(current_inputs), current_comptime_inputs)
        }
        (_, stored_inputs) => (body_check, stored_inputs, None),
    };
    BodyCheckWithResolutionInputs {
        body_check,
        inputs,
        stored_inputs,
        comptime: stored_comptime_inputs
            .or(filtered_comptime_inputs)
            .map(BodyCheckComptimeInputs::into_check),
    }
}

fn body_local_item_signatures(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    lowered: &TypeLowering,
) -> ItemSignatures {
    let defs = db.query(FullModuleDefsQuery(module_id));
    let functions = collect_body_signature_subset(
        db,
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
        &defs,
        lowered,
    );
    let extension_functions = collect_body_signature_subset(
        db,
        module_id,
        nia_item_tree::SignatureItemSet::ExtensionFunctions,
        &defs,
        lowered,
    );
    let values = collect_body_signature_subset(
        db,
        module_id,
        nia_item_tree::SignatureItemSet::Values,
        &defs,
        lowered,
    );
    let types = collect_body_signature_subset(
        db,
        module_id,
        nia_item_tree::SignatureItemSet::Types,
        &defs,
        lowered,
    );
    let traits = collect_body_signature_subset(
        db,
        module_id,
        nia_item_tree::SignatureItemSet::Traits,
        &defs,
        lowered,
    );
    let mut function_signatures = functions.functions;
    function_signatures.extend(extension_functions.functions);
    let mut diagnostics = functions.diagnostics;
    diagnostics.extend(extension_functions.diagnostics);
    diagnostics.extend(values.diagnostics);
    diagnostics.extend(types.diagnostics);
    diagnostics.extend(traits.diagnostics);
    ItemSignatures {
        functions: function_signatures,
        structs: types.structs,
        unions: types.unions,
        traits: traits.traits,
        trait_impls: traits.trait_impls,
        enums: types.enums,
        type_aliases: types.type_aliases,
        globals: values.globals,
        comptimes: values.comptimes,
        diagnostics,
    }
}

fn collect_body_signature_subset(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
    defs: &DefCollection,
    lowered: &TypeLowering,
) -> ItemSignatures {
    let active_item_tree = db.query(SignatureItemTreeQuery(module_id, set));
    nia_item_signatures::collect_item_signatures_from_active_item_tree(
        &active_item_tree,
        defs,
        lowered,
    )
}

fn executable_layouts_for_reachable_items(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
    array_length_cache: Option<
        &RefCell<HashMap<ModuleId, nia_comptime_check::ComptimeArrayLengths>>,
    >,
    program_signatures_override: Option<&ProgramExecutableSignatures>,
) -> nia_layout::Layouts {
    time_module_provider(db, "executable_layouts", module_id, || {
        let defs = db.query(FullModuleDefsQuery(module_id));
        let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
        let type_lowering = db.query(TypeLoweringQuery(module_id));
        let type_normalization = db.query(LayoutTypeNormalizationQuery(module_id));
        let item_signatures = db.query(ItemSignaturesQuery(module_id));
        let program_struct = |def_id: GlobalDefId| {
            db.query(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ))
            .structs
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramStructSignature {
                signature,
                interner: db
                    .query(SignatureTypeLoweringQuery(
                        def_id.module_id,
                        nia_item_tree::SignatureItemSet::Types,
                    ))
                    .interner,
            })
        };
        let program_union = |def_id: GlobalDefId| {
            db.query(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ))
            .unions
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramUnionSignature {
                signature,
                interner: db
                    .query(SignatureTypeLoweringQuery(
                        def_id.module_id,
                        nia_item_tree::SignatureItemSet::Types,
                    ))
                    .interner,
            })
        };
        let program_enum = |def_id: GlobalDefId| {
            db.query(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ))
            .enums
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramEnumSignature {
                signature,
                interner: db
                    .query(SignatureTypeLoweringQuery(
                        def_id.module_id,
                        nia_item_tree::SignatureItemSet::Types,
                    ))
                    .interner,
            })
        };
        let program_type_alias = |def_id: GlobalDefId| {
            db.query(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ))
            .type_aliases
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramTypeAliasSignature {
                signature,
                interner: db
                    .query(SignatureTypeLoweringQuery(
                        def_id.module_id,
                        nia_item_tree::SignatureItemSet::Types,
                    ))
                    .interner,
            })
        };
        let executable_array_lengths = |id: nia_ids::GlobalConstExprId| {
            if let Some(array_length_cache) = array_length_cache {
                if !array_length_cache.borrow().contains_key(&id.module_id) {
                    let array_lengths = with_comptime_input_and_program_signatures(
                        db,
                        id.module_id,
                        program_signatures_override,
                        |input, module| {
                            let mut array_lengths =
                                nia_comptime_check::compute_module_comptime_array_lengths(input);
                            array_lengths.diagnostics.extend(module.diagnostics.clone());
                            array_lengths
                        },
                    );
                    array_length_cache
                        .borrow_mut()
                        .insert(id.module_id, array_lengths);
                }
                return array_length_cache
                    .borrow()
                    .get(&id.module_id)
                    .and_then(|array_lengths| array_lengths.values.get(&id).copied());
            }
            Some(db.query(ComptimeArrayLengthsQuery(id.module_id)))
                .and_then(|array_lengths| array_lengths.values.get(&id).copied())
        };
        let (layout_interner, roots) =
            time_module_provider(db, "executable_layouts.roots", module_id, || {
                let mut layout_interner = type_normalization.interner.clone();
                let roots = executable_layout_roots(
                    module_id,
                    &mut layout_interner,
                    &item_signatures,
                    &program_struct,
                    &program_union,
                    type_lowering
                        .versioned_type_uses_from_active_item_tree(&active_item_tree)
                        .into_iter()
                        .map(|(_, ty)| ty),
                    reachable_functions,
                    reachable_globals,
                );
                (layout_interner, roots)
            });
        let layouts = time_module_provider(db, "executable_layouts.compute", module_id, || {
            nia_layout::compute_layouts_for_roots_with_program_context(
                nia_layout::LayoutComputationInput {
                    defs: &defs,
                    interner: &layout_interner,
                    signatures: &item_signatures,
                    normalized: &type_normalization.normalized,
                    array_lengths: &executable_array_lengths,
                    target: nia_layout::TargetDataLayout::LP64,
                    program: nia_layout::ProgramLayoutContext {
                        array_lengths: Some(&executable_array_lengths),
                        struct_: Some(&program_struct),
                        union: Some(&program_union),
                        enum_: Some(&program_enum),
                        type_alias: Some(&program_type_alias),
                        ..Default::default()
                    },
                },
                nia_layout::LayoutRoots {
                    types: &roots.types,
                    structs: &roots.structs,
                    unions: &roots.unions,
                },
            )
        });
        layouts
    })
}

fn executable_program_layouts<'a>(
    db: &'a QueryDb<CompilerContext>,
    cache: &'a RefCell<HashMap<ModuleId, nia_layout::Layouts>>,
    reachable_functions: &'a HashSet<GlobalDefId>,
    reachable_globals: &'a HashSet<GlobalDefId>,
    array_length_cache: Option<
        &'a RefCell<HashMap<ModuleId, nia_comptime_check::ComptimeArrayLengths>>,
    >,
    program_signatures_override: Option<&'a ProgramExecutableSignatures>,
) -> impl Fn(ModuleId) -> Option<nia_layout::Layouts> + 'a {
    move |module_id| {
        if let Some(layouts) = cache.borrow().get(&module_id).cloned() {
            return Some(layouts);
        }
        let layouts = executable_layouts_for_reachable_items(
            db,
            module_id,
            reachable_functions,
            reachable_globals,
            array_length_cache,
            program_signatures_override,
        );
        cache.borrow_mut().insert(module_id, layouts.clone());
        Some(layouts)
    }
}

fn rooted_layouts_for_checked_module(
    db: &QueryDb<CompilerContext>,
    module: &CheckedModule,
    program_layouts_override: Option<&dyn Fn(ModuleId) -> Option<nia_layout::Layouts>>,
    program_array_lengths_override: Option<&dyn Fn(nia_ids::GlobalConstExprId) -> Option<u64>>,
) -> nia_layout::Layouts {
    let item_signatures = db.query(ItemSignaturesQuery(module.id));
    let roots = checked_module_layout_roots(module);
    let array_lengths = &module.comptime.array_lengths;
    let local_array_lengths = |id| array_lengths.get(&id).copied();
    let layout_query = |module_id| {
        program_layouts_override
            .and_then(|program_layouts| program_layouts(module_id))
            .or_else(|| Some(db.query(LayoutsQuery(module_id))))
    };
    let program_array_lengths = |id: nia_ids::GlobalConstExprId| {
        program_array_lengths_override
            .and_then(|array_lengths| array_lengths(id))
            .or_else(|| {
                Some(db.query(ComptimeArrayLengthsQuery(id.module_id)))
                    .and_then(|array_lengths| array_lengths.values.get(&id).copied())
            })
    };
    nia_layout::compute_layouts_for_roots_with_program_context(
        nia_layout::LayoutComputationInput {
            defs: &module.defs,
            interner: &module.type_normalization.interner,
            signatures: &item_signatures,
            normalized: &module.type_normalization.normalized,
            array_lengths: &local_array_lengths,
            target: nia_layout::TargetDataLayout::LP64,
            program: nia_layout::ProgramLayoutContext {
                layouts: Some(&layout_query),
                array_lengths: Some(&program_array_lengths),
                ..Default::default()
            },
        },
        nia_layout::LayoutRoots {
            types: &roots.types,
            structs: &roots.structs,
            unions: &roots.unions,
        },
    )
}

fn executable_layout_roots(
    module_id: ModuleId,
    interner: &mut nia_ty::TyInterner,
    signatures: &ItemSignatures,
    program_struct: &dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>,
    program_union: &dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>,
    type_uses: impl IntoIterator<Item = InternedTyId>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
) -> CollectedLayoutRoots {
    let mut roots = LayoutRootCollector::with_program(interner, program_struct, program_union);
    for ty in type_uses {
        roots.add(ty);
    }
    for function_id in reachable_functions
        .iter()
        .copied()
        .filter(|def_id| def_id.module_id == module_id)
    {
        if let Some(signature) = signatures.functions.get(&function_id.def_id) {
            for param in &signature.params {
                roots.add(param.ty);
            }
            roots.add(signature.return_type);
        }
    }
    for impl_signature in &signatures.trait_impls {
        if impl_signature.methods.iter().any(|method| {
            reachable_functions.contains(&GlobalDefId {
                module_id,
                def_id: method.def_id,
            })
        }) {
            roots.add(impl_signature.target_ty);
        }
    }
    for global_id in reachable_globals
        .iter()
        .copied()
        .filter(|def_id| def_id.module_id == module_id)
    {
        if let Some(signature) = signatures.globals.get(&global_id.def_id)
            && let Some(ty) = signature.explicit_type
        {
            roots.add(ty);
        }
    }
    roots.finish()
}

fn checked_module_layout_roots(module: &CheckedModule) -> CollectedLayoutRoots {
    let mut interner = module.type_normalization.interner.clone();
    let mut roots = LayoutRootCollector::new(&mut interner);
    collect_semantic_layout_roots(&module.semantic_facts, &mut roots);
    roots.finish()
}

fn collect_semantic_layout_roots(
    semantic_facts: &nia_sema_ir::SemanticFacts,
    roots: &mut LayoutRootCollector<'_>,
) {
    for ty in semantic_facts.global_types.values().copied() {
        roots.add(ty);
    }
    for facts in semantic_facts.function_facts.values() {
        for ty in facts.local_types.values().copied() {
            roots.add(ty);
        }
        for ty in facts.node_expr_types.values().copied() {
            roots.add(ty);
        }
        for instantiation in &facts.generic_instantiations {
            for ty in &instantiation.args {
                roots.add(*ty);
            }
        }
        for coercion in facts.node_array_to_slice_coercions.values() {
            roots.add(coercion.array_ty);
            roots.add(coercion.slice_ty);
        }
        for coercion in facts.node_pointer_array_to_slice_coercions.values() {
            roots.add(coercion.pointer_ty);
            roots.add(coercion.array_ty);
            roots.add(coercion.slice_ty);
        }
        for coercion in facts.node_trait_object_coercions.values() {
            roots.add(coercion.source_ty);
            roots.add(coercion.target_ty);
        }
        for upcast in facts.node_trait_object_upcasts.values() {
            roots.add(upcast.source_ty);
            roots.add(upcast.target_ty);
        }
        for value in facts.node_builtin_values.values() {
            collect_builtin_value_layout_roots(value, roots);
        }
    }
    for ty in semantic_facts.node_expr_types.values().copied() {
        roots.add(ty);
    }
    for instantiation in &semantic_facts.generic_instantiations {
        for ty in &instantiation.args {
            roots.add(*ty);
        }
    }
    for value in semantic_facts.node_builtin_values.values() {
        collect_builtin_value_layout_roots(value, roots);
    }
}

fn collect_builtin_value_layout_roots(
    value: &nia_sema_ir::BuiltinValue,
    roots: &mut LayoutRootCollector<'_>,
) {
    match value {
        nia_sema_ir::BuiltinValue::Layout { ty, .. }
        | nia_sema_ir::BuiltinValue::FieldOffset { ty, .. } => roots.add(*ty),
        nia_sema_ir::BuiltinValue::Int(_) | nia_sema_ir::BuiltinValue::Usize(_) => {}
    }
}

struct LayoutRootCollector<'a> {
    interner: &'a mut nia_ty::TyInterner,
    program_struct: Option<&'a dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>>,
    program_union: Option<&'a dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>>,
    seen: HashSet<InternedTyId>,
    types: Vec<InternedTyId>,
    seen_structs: HashSet<nia_defs::DefId>,
    structs: Vec<nia_defs::DefId>,
    seen_global_structs: HashSet<GlobalDefId>,
    global_structs: Vec<GlobalDefId>,
    seen_unions: HashSet<nia_defs::DefId>,
    unions: Vec<nia_defs::DefId>,
    seen_global_unions: HashSet<GlobalDefId>,
    global_unions: Vec<GlobalDefId>,
}

impl<'a> LayoutRootCollector<'a> {
    fn new(interner: &'a mut nia_ty::TyInterner) -> Self {
        Self {
            interner,
            program_struct: None,
            program_union: None,
            seen: HashSet::new(),
            types: Vec::new(),
            seen_structs: HashSet::new(),
            structs: Vec::new(),
            seen_global_structs: HashSet::new(),
            global_structs: Vec::new(),
            seen_unions: HashSet::new(),
            unions: Vec::new(),
            seen_global_unions: HashSet::new(),
            global_unions: Vec::new(),
        }
    }

    fn with_program(
        interner: &'a mut nia_ty::TyInterner,
        program_struct: &'a dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>,
        program_union: &'a dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>,
    ) -> Self {
        let mut collector = Self::new(interner);
        collector.program_struct = Some(program_struct);
        collector.program_union = Some(program_union);
        collector
    }

    fn add(&mut self, ty: InternedTyId) {
        if !self.seen.insert(ty) {
            return;
        }
        self.types.push(ty);
        match self.interner.get(ty).cloned() {
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::VolatilePointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem })
            | Some(TyKind::Optional { elem }) => self.add(elem),
            Some(TyKind::Array { len, elem }) => {
                self.add_array_len(len);
                self.add(elem);
            }
            Some(TyKind::Range { bound, .. }) => {
                if let Some(bound) = bound {
                    self.add(bound);
                }
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                for param in params {
                    self.add(param);
                }
                self.add(return_type);
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                self.add(error);
                self.add(value);
            }
            Some(TyKind::Nominal { def_id, args }) => {
                self.add_global_struct(def_id);
                self.add_global_union(def_id);
                for arg in &args {
                    self.add(*arg);
                }
                self.add_nominal_fields(def_id, &args);
            }
            Some(TyKind::BuiltinTrait { args, .. })
            | Some(TyKind::TraitObject {
                trait_args: args, ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_args: args, ..
            }) => {
                for arg in args {
                    self.add(arg);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.add(self_ty);
                for arg in trait_args {
                    self.add(arg);
                }
            }
            Some(TyKind::Primitive(_))
            | Some(TyKind::Vector { .. })
            | Some(TyKind::Error)
            | Some(TyKind::ComptimeOnly)
            | Some(TyKind::GenericParam(_))
            | None => {}
        }
    }

    fn add_nominal_fields(&mut self, def_id: GlobalDefId, args: &[InternedTyId]) {
        if def_id.module_id == self.interner.interner_id().module_id() {
            return;
        }
        if let Some(program_struct) = self.program_struct
            && let Some(signature) = program_struct(def_id)
        {
            let signature = self.import_program_struct_signature(signature);
            self.add_aggregate_fields(&signature.generics, &signature.fields, args);
            return;
        }
        if let Some(program_union) = self.program_union
            && let Some(signature) = program_union(def_id)
        {
            let signature = self.import_program_union_signature(signature);
            self.add_aggregate_fields(&signature.generics, &signature.fields, args);
        }
    }

    fn import_program_struct_signature(
        &mut self,
        signature: ProgramStructSignature,
    ) -> StructSignature {
        StructSignature {
            generics: signature.signature.generics,
            where_predicates: signature.signature.where_predicates,
            fields: signature
                .signature
                .fields
                .into_iter()
                .map(|mut field| {
                    field.ty =
                        nia_ty::import_type_into(self.interner, &signature.interner, field.ty);
                    field
                })
                .collect(),
            is_extern: signature.signature.is_extern,
            span: signature.signature.span,
        }
    }

    fn import_program_union_signature(
        &mut self,
        signature: ProgramUnionSignature,
    ) -> UnionSignature {
        UnionSignature {
            generics: signature.signature.generics,
            where_predicates: signature.signature.where_predicates,
            fields: signature
                .signature
                .fields
                .into_iter()
                .map(|mut field| {
                    field.ty =
                        nia_ty::import_type_into(self.interner, &signature.interner, field.ty);
                    field
                })
                .collect(),
            is_extern: signature.signature.is_extern,
            span: signature.signature.span,
        }
    }

    fn add_aggregate_fields(
        &mut self,
        generics: &[String],
        fields: &[nia_item_signatures::FieldSignature],
        args: &[InternedTyId],
    ) {
        if generics.len() != args.len() {
            return;
        }
        let substitutions = generics
            .iter()
            .cloned()
            .zip(args.iter().copied())
            .collect::<HashMap<_, _>>();
        for field in fields {
            let field_ty = self.substitute_generics(field.ty, &substitutions);
            self.add(field_ty);
        }
    }

    fn substitute_generics(
        &mut self,
        ty: InternedTyId,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> InternedTyId {
        match self.interner.get(ty).cloned() {
            Some(TyKind::GenericParam(name)) => substitutions.get(&name).copied().unwrap_or(ty),
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let elem = self.substitute_generics(elem, substitutions);
                self.intern(TyKind::Pointer { is_readonly, elem })
            }
            Some(TyKind::VolatilePointer { is_readonly, elem }) => {
                let elem = self.substitute_generics(elem, substitutions);
                self.intern(TyKind::VolatilePointer { is_readonly, elem })
            }
            Some(TyKind::Slice { is_readonly, elem }) => {
                let elem = self.substitute_generics(elem, substitutions);
                self.intern(TyKind::Slice { is_readonly, elem })
            }
            Some(TyKind::SlicePointee { elem }) => {
                let elem = self.substitute_generics(elem, substitutions);
                self.intern(TyKind::SlicePointee { elem })
            }
            Some(TyKind::Array { len, elem }) => {
                let len = self.substitute_array_len_generics(len, substitutions);
                let elem = self.substitute_generics(elem, substitutions);
                self.intern(TyKind::Array { len, elem })
            }
            Some(TyKind::Range { kind, bound }) => {
                let bound = bound.map(|bound| self.substitute_generics(bound, substitutions));
                self.intern(TyKind::Range { kind, bound })
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                let params = params
                    .into_iter()
                    .map(|param| self.substitute_generics(param, substitutions))
                    .collect();
                let return_type = self.substitute_generics(return_type, substitutions);
                self.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                })
            }
            Some(TyKind::Optional { elem }) => {
                let elem = self.substitute_generics(elem, substitutions);
                self.intern(TyKind::Optional { elem })
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let error = self.substitute_generics(error, substitutions);
                let value = self.substitute_generics(value, substitutions);
                self.intern(TyKind::ErrorUnion { error, value })
            }
            Some(TyKind::Nominal { def_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.substitute_generics(arg, substitutions))
                    .collect();
                self.intern(TyKind::Nominal { def_id, args })
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.substitute_generics(arg, substitutions))
                    .collect();
                self.intern(TyKind::BuiltinTrait { trait_id, args })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            }) => {
                let self_ty = self.substitute_generics(self_ty, substitutions);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_generics(arg, substitutions))
                    .collect();
                self.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args,
                    name,
                })
            }
            Some(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_generics(arg, substitutions))
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .into_iter()
                    .map(|binding| nia_ty::AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| self.substitute_generics(arg, substitutions))
                            .collect(),
                        name: binding.name,
                        ty: self.substitute_generics(binding.ty, substitutions),
                    })
                    .collect();
                self.intern(TyKind::TraitObject {
                    is_readonly,
                    trait_id,
                    trait_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_generics(arg, substitutions))
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .into_iter()
                    .map(|binding| nia_ty::AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| self.substitute_generics(arg, substitutions))
                            .collect(),
                        name: binding.name,
                        ty: self.substitute_generics(binding.ty, substitutions),
                    })
                    .collect();
                self.intern(TyKind::TraitObjectPointee {
                    trait_id,
                    trait_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::Primitive(_))
            | Some(TyKind::Vector { .. })
            | Some(TyKind::Error)
            | Some(TyKind::ComptimeOnly)
            | None => ty,
        }
    }

    fn substitute_array_len_generics(
        &mut self,
        len: nia_ty::ArrayLenTy,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> nia_ty::ArrayLenTy {
        match len {
            nia_ty::ArrayLenTy::Builtin { builtin, ty } => nia_ty::ArrayLenTy::Builtin {
                builtin,
                ty: self.substitute_generics(ty, substitutions),
            },
            nia_ty::ArrayLenTy::Infer
            | nia_ty::ArrayLenTy::ConstValue(_)
            | nia_ty::ArrayLenTy::ConstExpr(_) => len,
        }
    }

    fn intern(&mut self, kind: TyKind) -> InternedTyId {
        self.interner.intern(kind)
    }

    fn add_struct(&mut self, def_id: nia_defs::DefId) {
        if self.seen_structs.insert(def_id) {
            self.structs.push(def_id);
        }
    }

    fn add_global_struct(&mut self, def_id: GlobalDefId) {
        if def_id.module_id == self.interner.interner_id().module_id() {
            self.add_struct(def_id.def_id);
        }
        if self.seen_global_structs.insert(def_id) {
            self.global_structs.push(def_id);
        }
    }

    fn add_union(&mut self, def_id: nia_defs::DefId) {
        if self.seen_unions.insert(def_id) {
            self.unions.push(def_id);
        }
    }

    fn add_global_union(&mut self, def_id: GlobalDefId) {
        if def_id.module_id == self.interner.interner_id().module_id() {
            self.add_union(def_id.def_id);
        }
        if self.seen_global_unions.insert(def_id) {
            self.global_unions.push(def_id);
        }
    }

    fn add_array_len(&mut self, len: nia_ty::ArrayLenTy) {
        if let nia_ty::ArrayLenTy::Builtin { ty, .. } = len {
            self.add(ty);
        }
    }

    fn finish(self) -> CollectedLayoutRoots {
        CollectedLayoutRoots {
            types: self.types,
            structs: self.structs,
            unions: self.unions,
        }
    }

    fn finish_global(self) -> CollectedGlobalLayoutRoots {
        CollectedGlobalLayoutRoots {
            structs: self.global_structs,
            unions: self.global_unions,
        }
    }
}

struct CollectedLayoutRoots {
    types: Vec<InternedTyId>,
    structs: Vec<nia_defs::DefId>,
    unions: Vec<nia_defs::DefId>,
}

struct CollectedGlobalLayoutRoots {
    structs: Vec<GlobalDefId>,
    unions: Vec<GlobalDefId>,
}

fn body_timing_mode(timings: TimingMode) -> nia_body_check::BodyTimingMode {
    if timings.detail() {
        nia_body_check::BodyTimingMode::Detail
    } else {
        nia_body_check::BodyTimingMode::Off
    }
}

pub(super) fn provide_checked_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> CheckedModule {
    time_module_provider(db, "checked_module", module_id, || {
        checked_module_with_body_and_flow_check(
            db,
            module_id,
            db.query(BodyCheckQuery(module_id)),
            db.query(FlowCheckQuery(module_id)),
            None,
        )
    })
}

fn checked_module_with_body_and_flow_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    body_check: nia_body_check::BodyCheck,
    flow_check: nia_flow_check::FlowCheck,
    layouts: Option<nia_layout::Layouts>,
) -> CheckedModule {
    let path = db.query(ModulePathQuery(module_id));
    CheckedModule {
        id: module_id,
        path,
        defs: db.query(FullModuleDefsQuery(module_id)),
        type_resolution: db.query(TypeResolutionQuery(module_id)),
        type_lowering: db.query(TypeLoweringQuery(module_id)),
        value_resolution: db.query(ValueResolutionQuery(module_id)),
        local_resolution: db.query(LocalResolutionQuery(module_id)),
        type_normalization: db.query(TypeNormalizationQuery(module_id)),
        comptime: db.query(ComptimeQuery(module_id)),
        static_check: db.query(StaticCheckQuery(module_id)),
        layouts: layouts.unwrap_or_else(|| db.query(LayoutsQuery(module_id))),
        abi_check: db.query(AbiCheckQuery(module_id)),
        flow_check,
        body_ir: body_check.ir,
        semantic_uses: db.query(SemanticUseTableQuery(module_id)),
        semantic_facts: body_check.facts,
        executable_reachable_globals: None,
        executable_reachable_structs: None,
        executable_reachable_unions: None,
        executable_type_only: false,
        body_diagnostics: body_check.diagnostics,
    }
}

fn executable_checked_module_with_body_and_flow_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    body_check: BodyCheckWithResolutionInputs,
    flow_check: nia_flow_check::FlowCheck,
    layouts: nia_layout::Layouts,
) -> CheckedModule {
    let BodyCheckWithResolutionInputs {
        body_check,
        inputs: body_inputs,
        stored_inputs,
        comptime,
    } = body_check;
    let body_inputs = stored_inputs.unwrap_or(body_inputs);
    CheckedModule {
        id: module_id,
        path: db.query(ModulePathQuery(module_id)),
        defs: db.query(FullModuleDefsQuery(module_id)),
        type_resolution: db.query(TypeResolutionQuery(module_id)),
        type_lowering: db.query(TypeLoweringQuery(module_id)),
        value_resolution: body_inputs.values,
        local_resolution: body_inputs.locals,
        type_normalization: db.query(TypeNormalizationQuery(module_id)),
        comptime: comptime.unwrap_or_else(|| db.query(ComptimeQuery(module_id))),
        static_check: nia_static_check::StaticCheck {
            diagnostics: Vec::new(),
        },
        layouts,
        abi_check: nia_abi_check::AbiCheck {
            diagnostics: Vec::new(),
        },
        flow_check,
        body_ir: body_check.ir,
        semantic_uses: body_inputs.semantic_uses,
        semantic_facts: body_check.facts,
        executable_reachable_globals: None,
        executable_reachable_structs: None,
        executable_reachable_unions: None,
        executable_type_only: false,
        body_diagnostics: body_check.diagnostics,
    }
}

fn extend_executable_checked_module_state(
    state: &mut ExecutableCheckedModuleState,
    increment: CheckedModule,
    checked_functions: HashSet<GlobalDefId>,
    checked_globals: HashSet<GlobalDefId>,
) {
    state
        .module
        .value_resolution
        .node_names
        .extend(increment.value_resolution.node_names);
    state
        .module
        .value_resolution
        .node_qualified_values
        .extend(increment.value_resolution.node_qualified_values);
    state
        .module
        .value_resolution
        .node_builtin_associated_values
        .extend(increment.value_resolution.node_builtin_associated_values);
    state
        .module
        .value_resolution
        .node_variant_enums
        .extend(increment.value_resolution.node_variant_enums);
    state
        .module
        .value_resolution
        .node_qualified_type_prefixes
        .extend(increment.value_resolution.node_qualified_type_prefixes);
    state
        .module
        .value_resolution
        .node_builtins
        .extend(increment.value_resolution.node_builtins);
    state
        .module
        .value_resolution
        .diagnostics
        .extend(increment.value_resolution.diagnostics);

    state
        .module
        .local_resolution
        .node_local_defs
        .extend(increment.local_resolution.node_local_defs);
    state
        .module
        .local_resolution
        .node_uses
        .extend(increment.local_resolution.node_uses);
    state
        .module
        .local_resolution
        .diagnostics
        .extend(increment.local_resolution.diagnostics);

    state
        .module
        .semantic_uses
        .node_value_uses
        .extend(increment.semantic_uses.node_value_uses);
    state
        .module
        .semantic_uses
        .node_builtin_associated_values
        .extend(increment.semantic_uses.node_builtin_associated_values);
    state
        .module
        .semantic_uses
        .node_local_defs
        .extend(increment.semantic_uses.node_local_defs);
    state
        .module
        .semantic_uses
        .node_type_uses
        .extend(increment.semantic_uses.node_type_uses);

    merge_executable_interner_snapshot(
        &mut state.module.comptime.interner,
        increment.comptime.interner,
        "comptime",
    );
    state
        .module
        .comptime
        .values
        .extend(increment.comptime.values);
    state
        .module
        .comptime
        .typed_values
        .extend(increment.comptime.typed_values);
    state
        .module
        .comptime
        .enum_values
        .extend(increment.comptime.enum_values);
    state
        .module
        .comptime
        .typed_enum_values
        .extend(increment.comptime.typed_enum_values);
    state
        .module
        .comptime
        .array_lengths
        .extend(increment.comptime.array_lengths);
    state
        .module
        .comptime
        .diagnostics
        .extend(increment.comptime.diagnostics);

    merge_executable_interner_snapshot(
        &mut state.module.body_ir.interner,
        increment.body_ir.interner,
        "body",
    );
    state
        .module
        .body_ir
        .function_bodies
        .extend(increment.body_ir.function_bodies);
    state
        .module
        .body_ir
        .global_inits
        .extend(increment.body_ir.global_inits);
    state
        .module
        .semantic_facts
        .global_types
        .extend(increment.semantic_facts.global_types);
    state
        .module
        .semantic_facts
        .generic_instantiations
        .extend(increment.semantic_facts.generic_instantiations);
    state
        .module
        .semantic_facts
        .function_facts
        .extend(increment.semantic_facts.function_facts);
    state
        .module
        .semantic_facts
        .node_expr_types
        .extend(increment.semantic_facts.node_expr_types);
    state
        .module
        .semantic_facts
        .node_bracket_suffix_resolutions
        .extend(increment.semantic_facts.node_bracket_suffix_resolutions);
    state
        .module
        .semantic_facts
        .node_array_to_slice_coercions
        .extend(increment.semantic_facts.node_array_to_slice_coercions);
    state
        .module
        .semantic_facts
        .node_pointer_array_to_slice_coercions
        .extend(
            increment
                .semantic_facts
                .node_pointer_array_to_slice_coercions,
        );
    state
        .module
        .semantic_facts
        .node_trait_object_coercions
        .extend(increment.semantic_facts.node_trait_object_coercions);
    state
        .module
        .semantic_facts
        .node_trait_object_upcasts
        .extend(increment.semantic_facts.node_trait_object_upcasts);
    state
        .module
        .semantic_facts
        .node_builtin_values
        .extend(increment.semantic_facts.node_builtin_values);
    state
        .module
        .semantic_facts
        .node_builtin_associated_values
        .extend(increment.semantic_facts.node_builtin_associated_values);
    state
        .module
        .semantic_facts
        .node_array_repeat_counts
        .extend(increment.semantic_facts.node_array_repeat_counts);
    state
        .module
        .semantic_facts
        .node_switch_pattern_values
        .extend(increment.semantic_facts.node_switch_pattern_values);
    state
        .module
        .semantic_facts
        .node_resolved_calls
        .extend(increment.semantic_facts.node_resolved_calls);
    state
        .module
        .semantic_facts
        .node_function_references
        .extend(increment.semantic_facts.node_function_references);
    state
        .module
        .body_diagnostics
        .extend(increment.body_diagnostics);
    state
        .module
        .flow_check
        .diagnostics
        .extend(increment.flow_check.diagnostics);
    state.module.layouts = increment.layouts;
    state.checked_functions.extend(checked_functions);
    state.checked_globals.extend(checked_globals);
}

fn merge_executable_interner_snapshot(
    current: &mut nia_ty::TyInterner,
    increment: nia_ty::TyInterner,
    source: &str,
) {
    if current.interner_id() != increment.interner_id() {
        *current = increment;
    } else if current.is_prefix_of(&increment) {
        *current = increment;
    } else if increment.is_prefix_of(current) {
    } else {
        panic!(
            "Nia ICE: executable {source} type interner snapshots share id {:?} but are not prefix-compatible",
            current.interner_id()
        );
    }
}

fn executable_signature_checked_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    layouts: nia_layout::Layouts,
) -> CheckedModule {
    let type_normalization = db.query(TypeNormalizationQuery(module_id));
    CheckedModule {
        id: module_id,
        path: db.query(ModulePathQuery(module_id)),
        defs: db.query(FullModuleDefsQuery(module_id)),
        type_resolution: db.query(TypeResolutionQuery(module_id)),
        type_lowering: db.query(TypeLoweringQuery(module_id)),
        value_resolution: ValueResolution {
            node_names: HashMap::new(),
            node_qualified_values: HashMap::new(),
            node_builtin_associated_values: HashMap::new(),
            node_variant_enums: HashMap::new(),
            node_qualified_type_prefixes: HashMap::new(),
            node_builtins: HashMap::new(),
            diagnostics: Vec::new(),
        },
        local_resolution: nia_local_resolve::LocalResolution {
            locals: nia_local_resolve::LocalMap::default(),
            node_local_defs: HashMap::new(),
            node_uses: HashMap::new(),
            diagnostics: Vec::new(),
        },
        type_normalization: type_normalization.clone(),
        comptime: ComptimeCheck::default(),
        static_check: nia_static_check::StaticCheck {
            diagnostics: Vec::new(),
        },
        layouts,
        abi_check: nia_abi_check::AbiCheck {
            diagnostics: Vec::new(),
        },
        flow_check: nia_flow_check::FlowCheck {
            diagnostics: Vec::new(),
        },
        body_ir: nia_body_ir::BodyIr {
            interner: type_normalization.interner,
            function_bodies: HashMap::new(),
            global_inits: HashMap::new(),
        },
        semantic_uses: nia_sema_ir::SemanticUseTable::default(),
        semantic_facts: nia_sema_ir::SemanticFacts::default(),
        executable_reachable_globals: Some(HashSet::new()),
        executable_reachable_structs: None,
        executable_reachable_unions: None,
        executable_type_only: true,
        body_diagnostics: Vec::new(),
    }
}

fn extend_module_functions_from_filtered_value_refs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    mut module_functions: HashSet<GlobalDefId>,
    module_globals: &HashSet<GlobalDefId>,
    checked_functions: Option<&HashSet<GlobalDefId>>,
) -> (HashSet<GlobalDefId>, BodyCheckResolutionInputs) {
    let source_version =
        time_module_provider(db, "extend_value_refs.source_version", module_id, || {
            db.query(ModuleSourceVersionQuery(module_id))
        });
    let origins = time_module_provider(db, "extend_value_refs.origins", module_id, || {
        db.query(ModuleOriginsQuery(module_id))
    });
    let active_item_tree =
        time_module_provider(db, "extend_value_refs.active_item_tree", module_id, || {
            db.query(FullActiveModuleItemTreeQuery(module_id))
        });
    let defs = time_module_provider(db, "extend_value_refs.defs", module_id, || {
        db.query(FullModuleDefsQuery(module_id))
    });
    let lowered = time_module_provider(db, "extend_value_refs.type_lowering", module_id, || {
        db.query(TypeLoweringQuery(module_id))
    });
    let signatures = time_module_provider(db, "extend_value_refs.signatures", module_id, || {
        db.query(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ))
    });

    let resolution_inputs =
        loop {
            let filter = nia_body_check::BodyCheckFilter::ReachableItems {
                functions: &module_functions,
                globals: module_globals,
                already_checked_functions: None,
                already_checked_globals: None,
            };
            let inputs =
                time_module_provider(db, "extend_value_refs.resolution_inputs", module_id, || {
                    body_check_resolution_inputs_for_filter(
                        db,
                        module_id,
                        filter,
                        BodyCheckResolutionContext {
                            source_version,
                            origins: &origins,
                            active_item_tree: active_item_tree.clone(),
                            defs: &defs,
                            lowered: &lowered,
                        },
                    )
                });
            let mut changed = false;
            time_module_provider(db, "extend_value_refs.scan_refs", module_id, || {
                for def_id in
                    inputs
                        .values
                        .node_names
                        .values()
                        .filter_map(|resolution| match resolution {
                            nia_value_resolve::ValueNameResolution::Def(def_id) => Some(*def_id),
                            nia_value_resolve::ValueNameResolution::External(_)
                            | nia_value_resolve::ValueNameResolution::Module
                            | nia_value_resolve::ValueNameResolution::LocalDeferred
                            | nia_value_resolve::ValueNameResolution::Error => None,
                        })
                        .chain(inputs.values.node_qualified_values.values().filter_map(
                            |global_id| {
                                (global_id.module_id == module_id).then_some(global_id.def_id)
                            },
                        ))
                {
                    let Some(def) = defs.defs.get(def_id) else {
                        continue;
                    };
                    if !matches!(
                        def.kind,
                        DefKind::Function | DefKind::Method | DefKind::TraitMethod
                    ) {
                        continue;
                    }
                    let Some(signature) = signatures.functions.get(&def_id) else {
                        continue;
                    };
                    if signature.is_comptime || !signature.has_body {
                        continue;
                    }
                    let global_id = GlobalDefId { module_id, def_id };
                    if checked_functions.is_some_and(|checked| checked.contains(&global_id)) {
                        continue;
                    }
                    changed |= module_functions.insert(global_id);
                }
            });
            if !changed {
                break inputs;
            }
        };

    (module_functions, resolution_inputs)
}

pub(super) fn provide_checked_modules(db: &QueryDb<CompilerContext>) -> Vec<CheckedModule> {
    time_provider(db.query(CompilerTimingsQuery), "checked_modules", || {
        time_provider(
            db.query(CompilerTimingsQuery),
            "checked_modules.shared_inputs",
            || {
                let _ = db.query(ProgramBodyFunctionSignaturesQuery);
                let _ = db.query(ProgramBodyValueSignaturesQuery);
                let _ = db.query(ProgramBodyTypeSignaturesQuery);
                let _ = db.query(ProgramBodyTraitSignaturesQuery);
                let _ = db.query(ExtensionMethodsQuery);
            },
        );
        db.query_many(
            db.query(ParseOkModuleIdsQuery)
                .into_iter()
                .map(CheckedModuleQuery),
        )
    })
}

pub(super) fn provide_executable_checked_modules(
    db: &QueryDb<CompilerContext>,
) -> Vec<CheckedModule> {
    time_provider(
        db.query(CompilerTimingsQuery),
        "executable_checked_modules",
        || {
            time_provider(
                db.query(CompilerTimingsQuery),
                "executable_checked_modules.shared_inputs",
                || {
                    let _ = db.query(ExtensionMethodIndexQuery);
                },
            );
            executable_checked_modules_inner(db)
        },
    )
}

fn executable_checked_modules_inner(db: &QueryDb<CompilerContext>) -> Vec<CheckedModule> {
    let parse_ok = db.query(ParseOkModuleIdsQuery);
    let graph = db.query(ModuleGraphQuery);
    let mut program_signatures = None::<ProgramExecutableSignatures>;
    let extension_methods = db.query(ExtensionMethodIndexQuery);
    let executable_array_length_cache =
        RefCell::new(HashMap::<ModuleId, nia_comptime_check::ComptimeArrayLengths>::new());
    let function_signature = |def_id: GlobalDefId| {
        db.query(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ))
        .functions
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramFunctionSignature {
            name: db
                .query(ModuleDefsQuery(def_id.module_id))
                .defs
                .get(def_id.def_id)
                .map(|def| def.name.clone())
                .unwrap_or_else(|| format!("def{}", def_id.def_id.0)),
            signature,
            interner: db
                .query(SignatureTypeLoweringQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Functions,
                ))
                .interner,
        })
    };
    let struct_signature = |def_id: GlobalDefId| {
        db.query(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))
        .structs
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramStructSignature {
            signature,
            interner: db
                .query(SignatureTypeLoweringQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ))
                .interner,
        })
    };
    let union_signature = |def_id: GlobalDefId| {
        db.query(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))
        .unions
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramUnionSignature {
            signature,
            interner: db
                .query(SignatureTypeLoweringQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ))
                .interner,
        })
    };
    let trait_signature = |def_id: GlobalDefId| {
        db.query(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ))
        .traits
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramTraitSignature {
            signature,
            interner: db
                .query(SignatureTypeLoweringQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Traits,
                ))
                .interner,
        })
    };
    let trait_default_method = |def_id: GlobalDefId| {
        let signatures = db.query(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ));
        signatures
            .traits
            .iter()
            .find_map(|(trait_def_id, signature)| {
                signature
                    .methods
                    .iter()
                    .any(|method| method.def_id == def_id.def_id && method.has_default)
                    .then(|| {
                        (
                            GlobalDefId {
                                module_id: def_id.module_id,
                                def_id: *trait_def_id,
                            },
                            ProgramTraitSignature {
                                signature: signature.clone(),
                                interner: db
                                    .query(SignatureTypeLoweringQuery(
                                        def_id.module_id,
                                        nia_item_tree::SignatureItemSet::Traits,
                                    ))
                                    .interner,
                            },
                        )
                    })
            })
    };
    let named_function = |module_id, name: &str| {
        let defs = db.query(FullModuleDefsQuery(module_id));
        defs.defs.iter().find_map(|(def_id, def)| {
            (def.kind == DefKind::Function && def.name == name)
                .then_some(GlobalDefId { module_id, def_id })
        })
    };
    let module_functions = |module_id| {
        let defs = db.query(FullModuleDefsQuery(module_id));
        defs.defs
            .iter()
            .filter_map(|(def_id, def)| {
                (def.kind == DefKind::Function).then_some(GlobalDefId { module_id, def_id })
            })
            .collect::<Vec<_>>()
    };
    let mut checked_by_id = HashMap::<ModuleId, ExecutableCheckedModuleState>::new();
    let mut reachability_seed = None::<ExecutableReachability>;
    let program_trait_impls = executable_program_trait_impls(db);
    let executable_global_initializer_cache = RefCell::new(HashMap::<
        GlobalDefId,
        Option<nia_comptime_ir::ResolvedComptimeExpr>,
    >::new());
    let reachability = loop {
        let reachable_inputs = time_provider(
            db.query(CompilerTimingsQuery),
            "executable_checked_modules.inputs",
            || {
                checked_by_id
                    .values()
                    .map(|state| ReachableModuleInput {
                        module_id: state.module.id,
                        body_ir: &state.module.body_ir,
                        semantic_facts: &state.module.semantic_facts,
                        type_lowering: &state.module.type_lowering,
                        type_normalization: &state.module.type_normalization,
                    })
                    .collect::<Vec<_>>()
            },
        );
        let mut reachability = time_provider(
            db.query(CompilerTimingsQuery),
            "executable_checked_modules.reachability_compute",
            || {
                compute_executable_reachability_with_seed(
                    reachability_seed.as_ref(),
                    &parse_ok,
                    &graph,
                    ExecutableRootDefs {
                        named_function: &named_function,
                        module_functions: &module_functions,
                    },
                    nia_executable_reachability::ExecutableSignatureIndex {
                        function: &function_signature,
                        struct_: &struct_signature,
                        union: &union_signature,
                        trait_: &trait_signature,
                        trait_default_method: &trait_default_method,
                    },
                    &extension_methods.methods,
                    &program_trait_impls,
                    &reachable_inputs,
                )
            },
        );
        let mut stale = time_provider(
            db.query(CompilerTimingsQuery),
            "executable_checked_modules.stale_select",
            || {
                parse_ok
                    .iter()
                    .copied()
                    .filter(|module_id| reachability.modules.contains(module_id))
                    .filter(|module_id| match checked_by_id.get(module_id) {
                        Some(state) => {
                            reachability.functions.iter().any(|def_id| {
                                def_id.module_id == *module_id
                                    && !state.checked_functions.contains(def_id)
                            }) || reachability.globals.iter().any(|def_id| {
                                def_id.module_id == *module_id
                                    && !state.checked_globals.contains(def_id)
                            })
                        }
                        None => true,
                    })
                    .collect::<VecDeque<_>>()
            },
        );
        if stale.is_empty() {
            break reachability;
        }
        let mut queued_stale = stale.iter().copied().collect::<HashSet<_>>();
        while let Some(module_id) = stale.pop_front() {
            queued_stale.remove(&module_id);
            let already_checked_functions = checked_by_id
                .get(&module_id)
                .map(|state| &state.checked_functions);
            let already_checked_globals = checked_by_id
                .get(&module_id)
                .map(|state| &state.checked_globals);
            let module_functions = reachability
                .functions
                .iter()
                .copied()
                .filter(|def_id| def_id.module_id == module_id)
                .filter(|def_id| {
                    already_checked_functions.is_none_or(|checked| !checked.contains(def_id))
                })
                .collect::<HashSet<_>>();
            let module_globals = reachability
                .globals
                .iter()
                .copied()
                .filter(|def_id| def_id.module_id == module_id)
                .filter(|def_id| {
                    already_checked_globals.is_none_or(|checked| !checked.contains(def_id))
                })
                .collect::<HashSet<_>>();
            let (module_functions, resolution_inputs) = time_module_provider(
                db,
                "executable_checked_modules.extend_value_refs",
                module_id,
                || {
                    extend_module_functions_from_filtered_value_refs(
                        db,
                        module_id,
                        module_functions,
                        &module_globals,
                        already_checked_functions,
                    )
                },
            );
            reachability
                .functions
                .extend(module_functions.iter().copied());
            let filter = nia_body_check::BodyCheckFilter::ReachableItems {
                functions: &module_functions,
                globals: &module_globals,
                already_checked_functions: already_checked_functions,
                already_checked_globals: already_checked_globals,
            };
            let program_signatures = time_provider(
                db.query(CompilerTimingsQuery),
                "executable_checked_modules.program_signatures",
                || {
                    program_signatures
                        .get_or_insert_with(|| executable_program_signatures_without_functions(db))
                },
            );
            let layouts = executable_layouts_for_reachable_items(
                db,
                module_id,
                &reachability.functions,
                &reachability.globals,
                Some(&executable_array_length_cache),
                Some(&*program_signatures),
            );
            let seed_interner = checked_by_id
                .get(&module_id)
                .map(|state| state.module.body_ir.interner.clone());
            let body_check = {
                let program_layout_cache = RefCell::new(HashMap::new());
                program_layout_cache
                    .borrow_mut()
                    .insert(module_id, layouts.clone());
                let executable_program_layouts = executable_program_layouts(
                    db,
                    &program_layout_cache,
                    &reachability.functions,
                    &reachability.globals,
                    Some(&executable_array_length_cache),
                    Some(&*program_signatures),
                );
                time_module_provider(db, "executable_body_check", module_id, || {
                    let execution_inputs = body_check_resolution_inputs_for_filter(
                        db,
                        module_id,
                        nia_body_check::BodyCheckFilter::All,
                        BodyCheckResolutionContext {
                            source_version: db.query(ModuleSourceVersionQuery(module_id)),
                            origins: &db.query(ModuleOriginsQuery(module_id)),
                            active_item_tree: db.query(FullActiveModuleItemTreeQuery(module_id)),
                            defs: &db.query(FullModuleDefsQuery(module_id)),
                            lowered: &db.query(TypeLoweringQuery(module_id)),
                        },
                    );
                    body_check_with_filter_and_layouts_with_inputs(
                        db,
                        module_id,
                        filter,
                        Some(layouts.clone()),
                        Some(&executable_program_layouts),
                        Some(&*program_signatures),
                        Some(execution_inputs),
                        Some(resolution_inputs),
                        seed_interner,
                        Some(&executable_global_initializer_cache),
                    )
                })
            };
            let module = time_module_provider(
                db,
                "executable_checked_modules.module_assembly",
                module_id,
                || {
                    executable_checked_module_with_body_and_flow_check(
                        db,
                        module_id,
                        body_check,
                        nia_flow_check::FlowCheck {
                            diagnostics: Vec::new(),
                        },
                        layouts,
                    )
                },
            );
            let checked_this_round = module
                .body_ir
                .function_bodies
                .keys()
                .copied()
                .collect::<HashSet<_>>();
            reachability
                .functions
                .extend(module.body_ir.function_bodies.keys().copied());
            let flow_check = executable_flow_check(db, module_id, &checked_this_round);
            let mut module = module;
            module.flow_check = flow_check;
            let checked_module_inputs = time_provider(
                db.query(CompilerTimingsQuery),
                "executable_checked_modules.checked_module_inputs",
                || {
                    checked_by_id
                        .values()
                        .map(|state| ReachableModuleInput {
                            module_id: state.module.id,
                            body_ir: &state.module.body_ir,
                            semantic_facts: &state.module.semantic_facts,
                            type_lowering: &state.module.type_lowering,
                            type_normalization: &state.module.type_normalization,
                        })
                        .collect::<Vec<_>>()
                },
            );
            let changed = time_module_provider(
                db,
                "executable_checked_modules.reachability_extend",
                module_id,
                || {
                    extend_executable_reachability_from_checked_module(
                        &mut reachability,
                        nia_executable_reachability::ExecutableSignatureIndex {
                            function: &function_signature,
                            struct_: &struct_signature,
                            union: &union_signature,
                            trait_: &trait_signature,
                            trait_default_method: &trait_default_method,
                        },
                        &extension_methods.methods,
                        &program_trait_impls,
                        ReachableModuleInput {
                            module_id: module.id,
                            body_ir: &module.body_ir,
                            semantic_facts: &module.semantic_facts,
                            type_lowering: &module.type_lowering,
                            type_normalization: &module.type_normalization,
                        },
                        &checked_module_inputs,
                    )
                },
            );
            time_module_provider(
                db,
                "executable_checked_modules.state_merge",
                module_id,
                || match checked_by_id.get_mut(&module_id) {
                    Some(state) => extend_executable_checked_module_state(
                        state,
                        module,
                        checked_this_round,
                        module_globals,
                    ),
                    None => {
                        checked_by_id.insert(
                            module_id,
                            ExecutableCheckedModuleState {
                                module,
                                checked_functions: checked_this_round,
                                checked_globals: module_globals,
                            },
                        );
                    }
                },
            );
            if changed {
                time_provider(
                    db.query(CompilerTimingsQuery),
                    "executable_checked_modules.stale_enqueue",
                    || {
                        for candidate in parse_ok.iter().copied() {
                            if !reachability.modules.contains(&candidate)
                                || queued_stale.contains(&candidate)
                            {
                                continue;
                            }
                            let needs_check = match checked_by_id.get(&candidate) {
                                Some(state) => {
                                    reachability.functions.iter().any(|def_id| {
                                        def_id.module_id == candidate
                                            && !state.checked_functions.contains(def_id)
                                    }) || reachability.globals.iter().any(|def_id| {
                                        def_id.module_id == candidate
                                            && !state.checked_globals.contains(def_id)
                                    })
                                }
                                None => true,
                            };
                            if needs_check {
                                queued_stale.insert(candidate);
                                stale.push_back(candidate);
                            }
                        }
                    },
                );
            }
        }
        reachability_seed = Some(reachability);
    };

    let parse_ok_modules = parse_ok;
    let mut codegen_modules = time_provider(
        db.query(CompilerTimingsQuery),
        "executable_checked_modules.final.codegen_modules",
        || {
            parse_ok_modules
                .iter()
                .copied()
                .filter(|module_id| reachability.modules.contains(module_id))
                .filter_map(|module_id| checked_by_id.get(&module_id))
                .map(|state| state.module.clone())
                .collect::<Vec<_>>()
        },
    );
    let codegen_layout_cache = time_provider(
        db.query(CompilerTimingsQuery),
        "executable_checked_modules.final.layout_cache",
        || {
            RefCell::new(
                codegen_modules
                    .iter()
                    .map(|module| (module.id, module.layouts.clone()))
                    .collect::<HashMap<_, _>>(),
            )
        },
    );
    let program_signatures = time_provider(
        db.query(CompilerTimingsQuery),
        "executable_checked_modules.final.program_signatures",
        || {
            program_signatures
                .get_or_insert_with(|| executable_program_signatures_without_functions(db))
        },
    );
    let executable_program_layouts = executable_program_layouts(
        db,
        &codegen_layout_cache,
        &reachability.functions,
        &reachability.globals,
        Some(&executable_array_length_cache),
        Some(&*program_signatures),
    );
    let type_only_modules = time_provider(
        db.query(CompilerTimingsQuery),
        "executable_checked_modules.final.type_only_modules",
        || {
            parse_ok_modules
                .iter()
                .copied()
                .filter(|module_id| reachability.type_modules.contains(module_id))
                .filter(|module_id| !reachability.modules.contains(module_id))
                .map(|module_id| {
                    let layouts = executable_program_layouts(module_id)
                        .unwrap_or_else(|| db.query(LayoutsQuery(module_id)));
                    executable_signature_checked_module(db, module_id, layouts)
                })
                .collect::<Vec<_>>()
        },
    );
    codegen_modules.extend(type_only_modules);
    let codegen_array_lengths = time_provider(
        db.query(CompilerTimingsQuery),
        "executable_checked_modules.final.array_lengths",
        || {
            codegen_modules
                .iter()
                .map(|module| (module.id, module.comptime.array_lengths.clone()))
                .collect::<HashMap<_, _>>()
        },
    );
    let executable_program_array_lengths = |id: nia_ids::GlobalConstExprId| {
        codegen_array_lengths
            .get(&id.module_id)
            .and_then(|array_lengths| array_lengths.get(&id).copied())
            .or_else(|| {
                executable_array_length_cache
                    .borrow()
                    .get(&id.module_id)
                    .and_then(|array_lengths| array_lengths.values.get(&id).copied())
            })
    };
    codegen_modules = time_provider(
        db.query(CompilerTimingsQuery),
        "executable_checked_modules.final.filter_codegen",
        || {
            codegen_modules
                .into_iter()
                .map(|module| {
                    filter_checked_module_for_codegen(
                        module,
                        db,
                        &reachability.functions,
                        &reachability.globals,
                        Some(&executable_program_layouts),
                        Some(&executable_program_array_lengths),
                    )
                })
                .collect::<Vec<_>>()
        },
    );
    let aggregate_roots = time_provider(
        db.query(CompilerTimingsQuery),
        "executable_checked_modules.final.aggregate_roots",
        || {
            executable_reachable_aggregate_roots(
                &struct_signature,
                &union_signature,
                &codegen_modules,
            )
        },
    );
    time_provider(
        db.query(CompilerTimingsQuery),
        "executable_checked_modules.final.store_aggregate_roots",
        || {
            for module in &mut codegen_modules {
                module.executable_reachable_structs = Some(aggregate_roots.structs.clone());
                module.executable_reachable_unions = Some(aggregate_roots.unions.clone());
            }
        },
    );
    codegen_modules
}

fn filter_checked_module_for_codegen(
    mut module: CheckedModule,
    db: &QueryDb<CompilerContext>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
    program_layouts_override: Option<&dyn Fn(ModuleId) -> Option<nia_layout::Layouts>>,
    program_array_lengths_override: Option<&dyn Fn(nia_ids::GlobalConstExprId) -> Option<u64>>,
) -> CheckedModule {
    module
        .body_ir
        .function_bodies
        .retain(|def_id, _| reachable_functions.contains(def_id));
    module
        .body_ir
        .global_inits
        .retain(|def_id, _| reachable_globals.contains(def_id));
    module.semantic_facts = filter_semantic_facts_for_reachable_items(
        module.semantic_facts,
        reachable_functions,
        reachable_globals,
    );
    module.layouts = rooted_layouts_for_checked_module(
        db,
        &module,
        program_layouts_override,
        program_array_lengths_override,
    );
    module.executable_reachable_globals = Some(reachable_globals.clone());
    module
}

fn executable_reachable_aggregate_roots(
    struct_signature: &dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>,
    union_signature: &dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>,
    modules: &[CheckedModule],
) -> ExecutableReachableAggregateRoots {
    let mut structs = HashSet::new();
    let mut unions = HashSet::new();
    for module in modules {
        let mut interner = module.type_normalization.interner.clone();
        let mut roots =
            LayoutRootCollector::with_program(&mut interner, struct_signature, union_signature);
        collect_semantic_layout_roots(&module.semantic_facts, &mut roots);
        let roots = roots.finish_global();
        structs.extend(roots.structs);
        unions.extend(roots.unions);
    }
    ExecutableReachableAggregateRoots { structs, unions }
}

struct ExecutableReachableAggregateRoots {
    structs: HashSet<GlobalDefId>,
    unions: HashSet<GlobalDefId>,
}

fn executable_flow_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    reachable_functions: &HashSet<GlobalDefId>,
) -> nia_flow_check::FlowCheck {
    time_module_provider(db, "executable_flow_check", module_id, || {
        let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
        let type_lowering = db.query(SignatureTypeLoweringQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ));
        let signatures = db.query(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ));
        nia_flow_check::check_active_module_flow_with_signatures_and_filter(
            &active_item_tree,
            &type_lowering.interner,
            nia_flow_check::FlowCheckSignatures {
                functions: &signatures.functions,
            },
            nia_flow_check::FlowCheckFilter::ReachableFunctions {
                module_id,
                functions: reachable_functions,
            },
        )
    })
}

pub(super) fn provide_monomorphization(
    db: &QueryDb<CompilerContext>,
) -> nia_monomorphize::Monomorphization {
    time_provider(db.query(CompilerTimingsQuery), "monomorphization", || {
        let checked_modules = checked_modules_for_codegen(db);
        let runtime = db.query(CompilerRuntimeQuery);
        let executable_signatures;
        let trait_solving_signatures;
        let (program_enums, trait_impls) = if runtime == RuntimeModel::FreestandingExecutable {
            executable_signatures = executable_program_signatures_without_functions(db);
            (
                &executable_signatures.enums,
                executable_signatures.trait_impls.as_slice(),
            )
        } else {
            trait_solving_signatures = db.query(ProgramTraitSolvingSignaturesQuery);
            (
                &trait_solving_signatures.enums,
                trait_solving_signatures.trait_impls.as_slice(),
            )
        };
        let local_signatures = checked_modules
            .iter()
            .map(|module| (module.id, db.query(ItemSignaturesQuery(module.id))))
            .collect::<HashMap<_, _>>();
        let function_bodies = function_bodies_from_checked_modules(db, &checked_modules);
        nia_monomorphize::collect_monomorphizations(
            &checked_modules
                .iter()
                .zip(function_bodies.iter())
                .map(|(module, function_bodies)| MonomorphizeModuleInput {
                    module_id: module.id,
                    defs: &module.defs,
                    interner: &function_bodies.interner,
                    normalization: &module.type_normalization,
                    comptime: &module.comptime,
                    const_expr_summaries: &module.type_lowering.const_expr_summaries,
                    layouts: Some(&module.layouts),
                    local_enums: &local_signatures
                        .get(&module.id)
                        .expect("monomorphization signatures must exist for checked module")
                        .enums,
                    program_enums,
                    trait_impls,
                    instantiations: &module.semantic_facts.generic_instantiations,
                })
                .collect::<Vec<_>>(),
        )
    })
}

fn checked_modules_for_codegen(db: &QueryDb<CompilerContext>) -> Vec<CheckedModule> {
    if db.query(CompilerRuntimeQuery) == RuntimeModel::FreestandingExecutable {
        db.query(ExecutableCheckedModulesQuery)
    } else {
        db.query(CheckedModulesQuery)
    }
}

fn checked_modules_for_diagnostics(db: &QueryDb<CompilerContext>) -> Vec<CheckedModule> {
    if db.query(CompilerRuntimeQuery) == RuntimeModel::FreestandingExecutable {
        return db.query(ExecutableCheckedModulesQuery);
    }
    let graph = db.query(ModuleGraphQuery);
    db.query_many(
        db.query(ParseOkModuleIdsQuery)
            .into_iter()
            .filter(|module_id| {
                graph.get(*module_id).is_some_and(|node| {
                    node.module_path.package == nia_imports::ENTRY_MODULE_MAP_NAME
                })
            })
            .map(CheckedModuleQuery),
    )
}

fn function_bodies_from_checked_modules(
    db: &QueryDb<CompilerContext>,
    checked_modules: &[CheckedModule],
) -> Vec<LoweredFunctionBodies> {
    time_provider(
        db.query(CompilerTimingsQuery),
        "function_bodies_from_checked_modules",
        || {
            checked_modules
                .iter()
                .map(|module| {
                    let lowered = nia_function_lower::lower_function_bodies_with_interner(
                        module.id,
                        module.body_ir.function_bodies.iter(),
                        &module.body_ir.interner,
                    )
                    .unwrap_or_else(|diagnostics| {
                        nia_function_lower::LoweredFunctionBodies {
                            interner: module.body_ir.interner.clone(),
                            bodies: HashMap::new(),
                            diagnostics,
                        }
                    });
                    LoweredFunctionBodies {
                        interner: lowered.interner,
                        bodies: lowered.bodies,
                        diagnostics: lowered.diagnostics,
                    }
                })
                .collect()
        },
    )
}

pub(super) fn provide_backend_lowering(
    db: &QueryDb<CompilerContext>,
) -> nia_backend_lower::BackendLowering {
    time_provider(db.query(CompilerTimingsQuery), "backend_lowering", || {
        provide_backend_lowering_inner(db)
    })
}

fn provide_backend_lowering_inner(
    db: &QueryDb<CompilerContext>,
) -> nia_backend_lower::BackendLowering {
    let all_checked_modules = checked_modules_for_codegen(db);
    let monomorphization = db.query(MonomorphizationQuery);
    let checked_modules = all_checked_modules;
    let (
        all_visible_extensions,
        active_item_trees,
        item_signatures,
        comptime_array_lengths,
        comptime_enum_values,
        visible_extensions,
        extension_methods,
        function_bodies,
    ) = time_provider(
        db.query(CompilerTimingsQuery),
        "backend_lowering.inputs",
        || {
            let timings = db.query(CompilerTimingsQuery);
            let all_visible_extensions = time_provider(
                timings,
                "backend_lowering.inputs.all_visible_extensions",
                || {
                    checked_modules
                        .iter()
                        .map(|module| (module.id, db.query(VisibleExtensionsQuery(module.id))))
                        .collect::<Vec<_>>()
                },
            );
            let active_item_trees =
                time_provider(timings, "backend_lowering.inputs.active_item_trees", || {
                    checked_modules
                        .iter()
                        .map(|checked_module| {
                            db.query(FullActiveModuleItemTreeQuery(checked_module.id))
                        })
                        .collect::<Vec<_>>()
                });
            let item_signatures =
                time_provider(timings, "backend_lowering.inputs.item_signatures", || {
                    checked_modules
                        .iter()
                        .map(|checked_module| db.query(ItemSignaturesQuery(checked_module.id)))
                        .collect::<Vec<_>>()
                });
            let comptime_array_lengths = checked_modules
                .iter()
                .map(|checked_module| nia_comptime_check::ComptimeArrayLengths {
                    interner: checked_module.comptime.interner.clone(),
                    values: checked_module.comptime.array_lengths.clone(),
                    diagnostics: checked_module.comptime.diagnostics.clone(),
                })
                .collect::<Vec<_>>();
            let comptime_enum_values = checked_modules
                .iter()
                .map(|checked_module| nia_comptime_check::ComptimeEnumValues {
                    interner: checked_module.comptime.interner.clone(),
                    values: checked_module.comptime.enum_values.clone(),
                    typed_values: checked_module.comptime.typed_enum_values.clone(),
                    diagnostics: checked_module.comptime.diagnostics.clone(),
                })
                .collect::<Vec<_>>();
            let visible_extensions = time_provider(
                timings,
                "backend_lowering.inputs.visible_extensions",
                || {
                    checked_modules
                        .iter()
                        .map(|checked_module| db.query(VisibleExtensionsQuery(checked_module.id)))
                        .collect::<Vec<_>>()
                },
            );
            let extension_methods =
                time_provider(timings, "backend_lowering.inputs.extension_methods", || {
                    db.query(ExtensionMethodIndexQuery)
                });
            let function_bodies = function_bodies_from_checked_modules(db, &checked_modules);
            (
                all_visible_extensions,
                active_item_trees,
                item_signatures,
                comptime_array_lengths,
                comptime_enum_values,
                visible_extensions,
                extension_methods,
                function_bodies,
            )
        },
    );
    let function_lowering_diagnostics =
        function_lowering_diagnostics(&checked_modules, &function_bodies);
    if !function_lowering_diagnostics.is_empty() {
        return nia_backend_lower::BackendLowering {
            diagnostics: function_lowering_diagnostics
                .into_iter()
                .map(|program_diagnostic| program_diagnostic.diagnostic)
                .collect(),
            ..empty_backend_lowering(db.query(CompilerOptimizationQuery))
        };
    }
    let indexes = time_provider(
        db.query(CompilerTimingsQuery),
        "backend_lowering.indexes",
        || {
            build_backend_lowering_indexes(
                &all_visible_extensions,
                &checked_modules,
                &comptime_array_lengths,
                &function_bodies,
            )
        },
    );
    let program_defs = |module_id| Some(db.query(FullModuleDefsQuery(module_id)));
    let mut executable_program_signatures;
    let executable_program_functions;
    let backend_program_signatures;
    let program_signatures =
        if db.query(CompilerRuntimeQuery) == RuntimeModel::FreestandingExecutable {
            executable_program_signatures = executable_program_signatures_without_functions(db);
            executable_program_functions = executable_program_functions_for_modules(
                db,
                checked_modules.iter().map(|module| module.id),
            );
            executable_program_signatures.functions = executable_program_functions;
            executable_program_signatures.codegen_maps()
        } else {
            backend_program_signatures = db.query(ProgramBackendSignaturesQuery);
            backend_program_signatures.codegen_maps()
        };
    let inputs = time_provider(
        db.query(CompilerTimingsQuery),
        "backend_lowering.module_inputs",
        || {
            build_backend_lowering_module_inputs(BackendLoweringModuleInputsInput {
                checked_modules: &checked_modules,
                runtime: db.query(CompilerRuntimeQuery),
                active_item_trees: &active_item_trees,
                item_signatures: &item_signatures,
                comptime_array_lengths: &comptime_array_lengths,
                comptime_enum_values: &comptime_enum_values,
                visible_extensions: &visible_extensions,
                function_bodies: &function_bodies,
                extension_methods: &extension_methods,
                program_defs: &program_defs,
                program_signatures,
                indexes: &indexes,
            })
        },
    );
    time_provider(
        db.query(CompilerTimingsQuery),
        "backend_lowering.lower_backend_program",
        || {
            nia_backend_lower::lower_backend_program_with_timings(
                &inputs,
                &monomorphization,
                db.query(CompilerOptimizationQuery),
                backend_timing_mode(db.query(CompilerTimingsQuery)),
            )
        },
    )
}

fn backend_timing_mode(timings: TimingMode) -> nia_backend_lower::BackendTimingMode {
    if timings.detail() {
        nia_backend_lower::BackendTimingMode::Detail
    } else {
        nia_backend_lower::BackendTimingMode::Off
    }
}

fn early_program_diagnostics(db: &QueryDb<CompilerContext>) -> Vec<ProgramDiagnostic> {
    let mut diagnostics = db.query(ProgramLoadDiagnosticsQuery);
    for module_id in db.query(LoadedModulesQuery) {
        let parse_errors = db.query(ModuleParseErrorsQuery(module_id));
        let path = db.query(ModulePathQuery(module_id));
        for error in &parse_errors {
            diagnostics.push(ProgramDiagnostic {
                path: path.clone(),
                diagnostic: Diagnostic::user_error_at(
                    codes::PARSE,
                    error.span,
                    error.message.clone(),
                ),
            });
        }
    }
    let public = db.query(PublicSurfaceQuery);
    for (module_id, diagnostic) in public.diagnostics {
        diagnostics.push(ProgramDiagnostic {
            path: db.query(ModulePathQuery(module_id)),
            diagnostic,
        });
    }
    if db.query(CompilerRuntimeQuery) != RuntimeModel::FreestandingExecutable {
        let first_path = db
            .query(ParseOkModuleIdsQuery)
            .first()
            .map(|module_id| db.query(ModulePathQuery(*module_id)))
            .unwrap_or_else(synthetic_diagnostic_path);
        diagnostics.extend(
            db.query(ExtensionMethodSetQuery)
                .diagnostics
                .iter()
                .chain(db.query(ExtensionAssociatedValuesQuery).diagnostics.iter())
                .cloned()
                .map(|diagnostic| ProgramDiagnostic {
                    path: first_path.clone(),
                    diagnostic,
                }),
        );
    }
    diagnostics
}

fn checked_module_diagnostics(
    db: &QueryDb<CompilerContext>,
    checked_modules: &[CheckedModule],
) -> Vec<ProgramDiagnostic> {
    let mut diagnostics = Vec::new();
    for checked in checked_modules {
        diagnostics.extend(module_diagnostics(&checked.path, &checked.defs.diagnostics));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.type_resolution.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.type_lowering.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.value_resolution.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.local_resolution.diagnostics,
        ));
        let item_signatures = db.query(ItemSignaturesQuery(checked.id));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &item_signatures.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.type_normalization.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.comptime.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.static_check.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.layouts.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.abi_check.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.flow_check.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(&checked.path, &checked.body_diagnostics));
    }
    diagnostics
}

fn monomorphization_diagnostics(
    checked_modules: &[CheckedModule],
    monomorphization: &nia_monomorphize::Monomorphization,
) -> Vec<ProgramDiagnostic> {
    monomorphization
        .diagnostics
        .iter()
        .cloned()
        .map(|diagnostic| ProgramDiagnostic {
            path: path_for_diagnostic_span(
                checked_modules,
                diagnostic.primary_span().unwrap_or_default(),
            ),
            diagnostic,
        })
        .collect()
}

fn function_lowering_diagnostics(
    checked_modules: &[CheckedModule],
    function_bodies: &[LoweredFunctionBodies],
) -> Vec<ProgramDiagnostic> {
    checked_modules
        .iter()
        .zip(function_bodies.iter())
        .flat_map(|(module, lowered)| {
            lowered
                .diagnostics
                .iter()
                .map(|diagnostic| ProgramDiagnostic {
                    path: module.path.clone(),
                    diagnostic: Diagnostic::internal_error_at(
                        codes::INVALID_FUNCTION_IR,
                        diagnostic.span,
                        diagnostic.message.clone(),
                    ),
                })
        })
        .collect()
}

fn backend_lowering_diagnostics(
    checked_modules: &[CheckedModule],
    backend_lowering: &nia_backend_lower::BackendLowering,
) -> Vec<ProgramDiagnostic> {
    backend_lowering
        .diagnostics
        .iter()
        .cloned()
        .map(|diagnostic| ProgramDiagnostic {
            path: path_for_diagnostic_span(
                checked_modules,
                diagnostic.primary_span().unwrap_or_default(),
            ),
            diagnostic,
        })
        .collect()
}
