// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_executable_reachability::{
    ReachableModuleInput, compute_executable_reachability,
    filter_semantic_facts_for_reachable_functions,
};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

#[derive(Clone)]
pub(super) struct CompilerQueryProviders {
    pub(super) checked_program: fn(&QueryDb<CompilerContext>) -> CheckedProgram,
    pub(super) module_graph: fn(&QueryDb<CompilerContext>) -> ModuleGraph,
    pub(super) parse_ok_module_ids: fn(&QueryDb<CompilerContext>) -> Vec<ModuleId>,
    pub(super) module_item_tree: fn(&QueryDb<CompilerContext>, ModuleId) -> ModuleItemTree,
    pub(super) active_module_item_tree:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ActiveModuleItemTree,
    pub(super) module_defs: fn(&QueryDb<CompilerContext>, ModuleId) -> DefCollection,
    pub(super) defs_by_module: fn(&QueryDb<CompilerContext>) -> Vec<DefCollection>,
    pub(super) program_defs_by_id: fn(&QueryDb<CompilerContext>) -> ProgramDefsById,
    pub(super) public_surface: fn(&QueryDb<CompilerContext>) -> PublicSurfaceQueryValue,
    pub(super) type_resolution: fn(&QueryDb<CompilerContext>, ModuleId) -> TypeResolution,
    pub(super) type_lowering: fn(&QueryDb<CompilerContext>, ModuleId) -> TypeLowering,
    pub(super) program_type_lowerings: fn(&QueryDb<CompilerContext>) -> ProgramTypeLowerings,
    pub(super) item_signatures: fn(&QueryDb<CompilerContext>, ModuleId) -> ItemSignatures,
    pub(super) program_item_signatures: fn(&QueryDb<CompilerContext>) -> ProgramItemSignaturesById,
    pub(super) type_normalization: fn(&QueryDb<CompilerContext>, ModuleId) -> TypeNormalization,
    pub(super) program_type_normalizations:
        fn(&QueryDb<CompilerContext>) -> ProgramTypeNormalizations,
    pub(super) program_signatures: fn(&QueryDb<CompilerContext>) -> ProgramSignaturesValue,
    pub(super) extension_methods: fn(&QueryDb<CompilerContext>) -> ExtensionMethodsValue,
    pub(super) visible_extensions:
        fn(&QueryDb<CompilerContext>, ModuleId) -> VisibleExtensionsValue,
    pub(super) value_resolution: fn(&QueryDb<CompilerContext>, ModuleId) -> ValueResolution,
    pub(super) local_resolution: fn(&QueryDb<CompilerContext>, ModuleId) -> LocalResolution,
    pub(super) semantic_use_table:
        fn(&QueryDb<CompilerContext>, ModuleId) -> nia_sema_ir::SemanticUseTable,
    pub(super) comptime_module: fn(&QueryDb<CompilerContext>, ModuleId) -> ComptimeModuleLowering,
    pub(super) program_comptime_modules: fn(&QueryDb<CompilerContext>) -> ProgramComptimeModules,
    pub(super) comptime: fn(&QueryDb<CompilerContext>, ModuleId) -> ComptimeCheck,
    pub(super) program_comptime: fn(&QueryDb<CompilerContext>) -> ProgramComptimeById,
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
    pub(super) program_diagnostics: fn(&QueryDb<CompilerContext>) -> Vec<ProgramDiagnostic>,
}

impl Default for CompilerQueryProviders {
    fn default() -> Self {
        Self {
            checked_program: provide_checked_program,
            module_graph: provide_module_graph,
            parse_ok_module_ids: provide_parse_ok_module_ids,
            module_item_tree: provide_module_item_tree,
            active_module_item_tree: provide_active_module_item_tree,
            module_defs: provide_module_defs,
            defs_by_module: provide_defs_by_module,
            program_defs_by_id: provide_program_defs_by_id,
            public_surface: provide_public_surface,
            type_resolution: provide_type_resolution,
            type_lowering: provide_type_lowering,
            program_type_lowerings: provide_program_type_lowerings,
            item_signatures: provide_item_signatures,
            program_item_signatures: provide_program_item_signatures,
            type_normalization: provide_type_normalization,
            program_type_normalizations: provide_program_type_normalizations,
            program_signatures: provide_program_signatures,
            extension_methods: provide_extension_methods,
            visible_extensions: provide_visible_extensions,
            value_resolution: provide_value_resolution,
            local_resolution: provide_local_resolution,
            semantic_use_table: provide_semantic_use_table,
            comptime_module: provide_comptime_module,
            program_comptime_modules: provide_program_comptime_modules,
            comptime: provide_comptime,
            program_comptime: provide_program_comptime,
            layouts: provide_layouts,
            abi_check: provide_abi_check,
            static_check: provide_static_check,
            flow_check: provide_flow_check,
            body_check: provide_body_check,
            checked_module: provide_checked_module,
            checked_modules: provide_checked_modules,
            monomorphization: provide_monomorphization,
            backend_lowering: provide_backend_lowering,
            program_diagnostics: provide_program_diagnostics,
        }
    }
}

pub(super) fn provide_checked_program(db: &QueryDb<CompilerContext>) -> CheckedProgram {
    time_provider(db.query(CompilerTimingsQuery), "checked_program", || {
        CheckedProgram {
            graph: db.query(ModuleGraphQuery),
            optimization: db.query(CompilerOptimizationQuery),
            modules: checked_modules_for_codegen(db),
            monomorphization: db.query(MonomorphizationQuery),
            backend_lowering: db.query(BackendLoweringQuery),
            diagnostics: db.query(ProgramDiagnosticsQuery),
        }
    })
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

pub(super) fn provide_module_defs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> DefCollection {
    let item_tree = db.query(ActiveModuleItemTreeQuery(module_id));
    nia_defs::collect_module_defs_from_active_item_tree(module_id, &item_tree)
}

pub(super) fn provide_defs_by_module(db: &QueryDb<CompilerContext>) -> Vec<DefCollection> {
    db.query_many(
        db.query(ParseOkModuleIdsQuery)
            .into_iter()
            .map(ModuleDefsQuery),
    )
}

pub(super) fn provide_program_defs_by_id(db: &QueryDb<CompilerContext>) -> ProgramDefsById {
    Arc::new(
        db.query(ParseOkModuleIdsQuery)
            .into_iter()
            .map(|module_id| (module_id, db.query(ModuleDefsQuery(module_id))))
            .collect(),
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
        let active_item_tree = db.query(ActiveModuleItemTreeQuery(module_id));
        let defs = db.query(ModuleDefsQuery(module_id));
        let program_defs = defs_by_module_id(db);
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

pub(super) fn provide_type_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeLowering {
    let active_item_tree = db.query(ActiveModuleItemTreeQuery(module_id));
    let type_resolution = db.query(TypeResolutionQuery(module_id));
    let program_defs = defs_by_module_id(db);
    nia_type_lower::lower_module_types_from_active_item_tree(
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
    let active_item_tree = db.query(ActiveModuleItemTreeQuery(module_id));
    let defs = db.query(ModuleDefsQuery(module_id));
    let type_lowering = db.query(TypeLoweringQuery(module_id));
    nia_item_signatures::collect_item_signatures_from_active_item_tree(
        &active_item_tree,
        &defs,
        &type_lowering,
    )
}

pub(super) fn provide_program_item_signatures(
    db: &QueryDb<CompilerContext>,
) -> ProgramItemSignaturesById {
    time_provider(
        db.query(CompilerTimingsQuery),
        "program_item_signatures",
        || {
            Arc::new(
                db.query(ParseOkModuleIdsQuery)
                    .into_iter()
                    .map(|module_id| (module_id, db.query(ItemSignaturesQuery(module_id))))
                    .collect(),
            )
        },
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

pub(super) fn provide_program_type_lowerings(
    db: &QueryDb<CompilerContext>,
) -> ProgramTypeLowerings {
    time_provider(
        db.query(CompilerTimingsQuery),
        "program_type_lowerings",
        || {
            Arc::new(
                db.query(ParseOkModuleIdsQuery)
                    .into_iter()
                    .map(|module_id| (module_id, db.query(TypeLoweringQuery(module_id))))
                    .collect(),
            )
        },
    )
}

pub(super) fn provide_program_type_normalizations(
    db: &QueryDb<CompilerContext>,
) -> ProgramTypeNormalizations {
    time_provider(
        db.query(CompilerTimingsQuery),
        "program_type_normalizations",
        || {
            Arc::new(
                db.query(ParseOkModuleIdsQuery)
                    .into_iter()
                    .map(|module_id| (module_id, db.query(TypeNormalizationQuery(module_id))))
                    .collect(),
            )
        },
    )
}

pub(super) fn provide_program_signatures(db: &QueryDb<CompilerContext>) -> ProgramSignaturesValue {
    time_provider(db.query(CompilerTimingsQuery), "program_signatures", || {
        let module_ids = db.query(ParseOkModuleIdsQuery);
        let type_lowerings = module_ids
            .iter()
            .copied()
            .map(|module_id| db.query(TypeLoweringQuery(module_id)))
            .collect::<Vec<_>>();
        let item_signatures = module_ids
            .iter()
            .copied()
            .map(|module_id| db.query(ItemSignaturesQuery(module_id)))
            .collect::<Vec<_>>();
        let defs = module_ids
            .iter()
            .copied()
            .map(|module_id| db.query(ModuleDefsQuery(module_id)))
            .collect::<Vec<_>>();
        let modules = module_ids
            .iter()
            .copied()
            .zip(type_lowerings.iter())
            .zip(item_signatures.iter())
            .zip(defs.iter())
            .map(
                |(((module_id, lowering), signatures), defs)| ModuleSignatureInput {
                    module_id,
                    defs,
                    lowering,
                    signatures,
                },
            )
            .collect::<Vec<_>>();
        Arc::new(ProgramSignatures {
            functions: collect_program_functions(&modules),
            globals: collect_program_globals(&modules),
            comptimes: collect_program_comptimes(&modules),
            structs: collect_program_structs(&modules),
            unions: collect_program_unions(&modules),
            enums: collect_program_enums(&modules),
            traits: collect_program_traits(&modules),
            type_aliases: crate::program_signatures::collect_program_type_aliases(&modules),
            trait_impls: crate::program_signatures::collect_program_trait_impls(&modules),
        })
    })
}

pub(super) fn provide_extension_methods(db: &QueryDb<CompilerContext>) -> ExtensionMethodsValue {
    time_provider(db.query(CompilerTimingsQuery), "extension_methods", || {
        let module_ids = db.query(ParseOkModuleIdsQuery);
        let modules = module_ids
            .iter()
            .copied()
            .map(|module_id| ExtensionModuleAstInput {
                id: module_id,
                ast: db.query(ModuleAstQuery(module_id)),
            })
            .collect::<Vec<_>>();
        let defs = module_ids
            .iter()
            .copied()
            .map(|module_id| db.query(ModuleDefsQuery(module_id)))
            .collect::<Vec<_>>();
        let type_lowerings = module_ids
            .iter()
            .copied()
            .map(|module_id| db.query(TypeLoweringQuery(module_id)))
            .collect::<Vec<_>>();
        let item_signatures = module_ids
            .iter()
            .copied()
            .map(|module_id| db.query(ItemSignaturesQuery(module_id)))
            .collect::<Vec<_>>();
        let normalizations = db.query(ProgramTypeNormalizationsQuery);
        let inputs = modules
            .iter()
            .zip(defs.iter())
            .zip(type_lowerings.iter())
            .zip(item_signatures.iter())
            .zip(module_ids.iter())
            .map(
                |((((module, defs), lowering), signatures), module_id)| ExtensionModuleInput {
                    module,
                    defs,
                    lowering,
                    signatures,
                    normalization: normalizations
                        .get(module_id)
                        .expect("missing type normalization"),
                },
            )
            .collect::<Vec<_>>();
        let (methods, mut diagnostics) = collect_extension_methods(&inputs);
        let (associated_values, associated_value_diagnostics) =
            collect_extension_associated_values(&inputs);
        diagnostics.extend(associated_value_diagnostics);
        Arc::new(ExtensionMethodsQueryValue {
            methods,
            associated_values,
            diagnostics,
        })
    })
}

pub(super) fn provide_visible_extensions(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> VisibleExtensionsValue {
    let graph = db.query(ModuleGraphQuery);
    let defs = defs_by_module_id(db);
    let public = db.query(PublicSurfaceQuery);
    let empty_using = ModuleUsingScope::default();
    let using_scope = public.using_scopes.get(&module_id).unwrap_or(&empty_using);
    let normalizations = db.query(ProgramTypeNormalizationsQuery);
    let program_signatures = db.query(ProgramSignaturesQuery);
    let extensions = db.query(ExtensionMethodsQuery);
    Arc::new(visible_extensions_for_module(VisibleExtensionsInput {
        module_id,
        graph: &graph,
        using_scope,
        public_surfaces: &public.surfaces,
        defs_by_module: &defs,
        normalizations: &normalizations,
        program_signatures: program_signatures.maps(),
        extensions: &extensions.methods,
        associated_values: &extensions.associated_values,
    }))
}

pub(super) fn provide_value_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ValueResolution {
    time_module_provider(db, "value_resolution", module_id, || {
        let active_item_tree = db.query(ActiveModuleItemTreeQuery(module_id));
        let defs = db.query(ModuleDefsQuery(module_id));
        let program_defs = defs_by_module_id(db);
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
    let active_item_tree = db.query(ActiveModuleItemTreeQuery(module_id));
    let defs = db.query(ModuleDefsQuery(module_id));
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
        type_lowering
            .node_type_uses
            .iter()
            .map(|(key, ty)| (key.clone(), *ty)),
    );
    builder.finish()
}

pub(super) fn provide_comptime_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ComptimeModuleLowering {
    let module = db.query(ModuleAstQuery(module_id));
    let defs = db.query(ModuleDefsQuery(module_id));
    let values = db.query(ValueResolutionQuery(module_id));
    let locals = db.query(LocalResolutionQuery(module_id));
    let semantic_uses = db.query(SemanticUseTableQuery(module_id));
    let type_lowering = db.query(TypeLoweringQuery(module_id));
    nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
        module: &module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        const_exprs: &type_lowering.const_exprs,
    })
}

pub(super) fn provide_program_comptime_modules(
    db: &QueryDb<CompilerContext>,
) -> ProgramComptimeModules {
    let ids = db.query(ParseOkModuleIdsQuery);
    let modules = db.query_many(ids.iter().copied().map(ComptimeModuleQuery));
    Arc::new(
        ids.into_iter()
            .zip(modules.into_iter().map(|lowering| lowering.module))
            .collect(),
    )
}

pub(super) fn provide_comptime(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ComptimeCheck {
    time_module_provider(db, "comptime", module_id, || {
        let module = db.query(ComptimeModuleQuery(module_id));
        let defs = db.query(ModuleDefsQuery(module_id));
        let program_modules = db.query(ProgramComptimeModulesQuery);
        let program_defs = db.query(ProgramDefsByIdQuery);
        let program_type_lowerings = db.query(ProgramTypeLoweringsQuery);
        let program_type_normalizations = db.query(ProgramTypeNormalizationsQuery);
        let program_signatures = db.query(ProgramSignaturesQuery);
        let program_item_signatures = db.query(ProgramItemSignaturesQuery);
        let values = db.query(ValueResolutionQuery(module_id));
        let locals = db.query(LocalResolutionQuery(module_id));
        let semantic_uses = db.query(SemanticUseTableQuery(module_id));
        let item_signatures = db.query(ItemSignaturesQuery(module_id));
        let type_normalization = db.query(TypeNormalizationQuery(module_id));
        let mut comptime =
            nia_comptime_check::check_module_comptime(nia_comptime_check::ComptimeInput {
                module: &module.module,
                defs: &defs,
                values: &values,
                locals: &locals,
                semantic_uses: &semantic_uses,
                signatures: &item_signatures,
                interner: &type_normalization.interner,
                normalized: &type_normalization.normalized,
                target: &db.query(CompilerTargetQuery),
                program: nia_comptime_check::ComptimeProgramContext {
                    modules: Some(&program_modules),
                    defs: Some(&program_defs),
                    type_lowerings: Some(&program_type_lowerings),
                    type_normalizations: Some(&program_type_normalizations),
                    signatures: Some(&program_item_signatures),
                    trait_impls: &program_signatures.trait_impls,
                },
            });
        comptime.diagnostics.extend(module.diagnostics);
        comptime
    })
}

pub(super) fn provide_program_comptime(db: &QueryDb<CompilerContext>) -> ProgramComptimeById {
    time_provider(db.query(CompilerTimingsQuery), "program_comptime", || {
        let ids = db.query(ParseOkModuleIdsQuery);
        let comptimes = db.query_many(ids.iter().copied().map(ComptimeQuery));
        Arc::new(ids.into_iter().zip(comptimes).collect())
    })
}

pub(super) fn provide_layouts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_layout::Layouts {
    time_module_provider(db, "layouts", module_id, || {
        let defs = db.query(ModuleDefsQuery(module_id));
        let type_normalization = db.query(TypeNormalizationQuery(module_id));
        let item_signatures = db.query(ItemSignaturesQuery(module_id));
        let comptime = db.query(ComptimeQuery(module_id));
        let layout_query = |module_id| Some(db.query(LayoutsQuery(module_id)));
        let local_array_lengths = |id| comptime.array_lengths.get(&id).copied();
        let program_array_lengths = |id: nia_ids::GlobalConstExprId| {
            Some(db.query(ComptimeQuery(id.module_id)))
                .and_then(|comptime| comptime.array_lengths.get(&id).copied())
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
    let defs = db.query(ModuleDefsQuery(module_id));
    let type_lowering = db.query(TypeLoweringQuery(module_id));
    let item_signatures = db.query(ItemSignaturesQuery(module_id));
    let program = db.query(ProgramSignaturesQuery);
    let program_structs = program
        .structs
        .iter()
        .map(|(def_id, signature)| (*def_id, signature.signature.clone()))
        .collect();
    let program_unions = program
        .unions
        .iter()
        .map(|(def_id, signature)| (*def_id, signature.signature.clone()))
        .collect();
    let program_enums = program
        .enums
        .iter()
        .map(|(def_id, signature)| (*def_id, signature.signature.clone()))
        .collect();
    nia_abi_check::check_module_abi_with_program_signatures(
        &defs,
        &type_lowering.interner,
        &item_signatures,
        nia_abi_check::ProgramAbiSignatures {
            structs: &program_structs,
            unions: &program_unions,
            enums: &program_enums,
        },
    )
}

pub(super) fn provide_static_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_static_check::StaticCheck {
    let module = db.query(ModuleAstQuery(module_id));
    let defs = db.query(ModuleDefsQuery(module_id));
    let values = db.query(ValueResolutionQuery(module_id));
    let locals = db.query(LocalResolutionQuery(module_id));
    let semantic_uses = db.query(SemanticUseTableQuery(module_id));
    let signatures = db.query(ItemSignaturesQuery(module_id));
    let comptime = db.query(ComptimeQuery(module_id));
    let program_defs = db.query(ProgramDefsByIdQuery);
    let program_comptime = db.query(ProgramComptimeQuery);
    nia_static_check::check_module_static_initializers(nia_static_check::StaticCheckInput {
        module: &module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        signatures: &signatures,
        comptime: &comptime,
        program_defs: &program_defs,
        program_comptime: &program_comptime,
        target: &db.query(CompilerTargetQuery),
    })
}

pub(super) fn provide_flow_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_flow_check::FlowCheck {
    let module = db.query(ModuleAstQuery(module_id));
    let type_lowering = db.query(TypeLoweringQuery(module_id));
    let signatures = db.query(ItemSignaturesQuery(module_id));
    nia_flow_check::check_module_flow(&module, &type_lowering.interner, &signatures)
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
    let source_version = db.query(ModuleSourceVersionQuery(module_id));
    let origins = db.query(ModuleOriginsQuery(module_id));
    let module = db.query(ModuleAstQuery(module_id));
    let defs = db.query(ModuleDefsQuery(module_id));
    let program_defs = defs_by_module_id(db);
    let values = db.query(ValueResolutionQuery(module_id));
    let locals = db.query(LocalResolutionQuery(module_id));
    let semantic_uses = db.query(SemanticUseTableQuery(module_id));
    let lowered = db.query(TypeLoweringQuery(module_id));
    let program_type_lowerings = db.query(ProgramTypeLoweringsQuery);
    let signatures = db.query(ItemSignaturesQuery(module_id));
    let normalization = db.query(TypeNormalizationQuery(module_id));
    let program_type_normalizations = db.query(ProgramTypeNormalizationsQuery);
    let comptime = db.query(ComptimeQuery(module_id));
    let comptime_module = db.query(ComptimeModuleQuery(module_id));
    let layouts = db.query(LayoutsQuery(module_id));
    let program_layouts = |module_id| Some(db.query(LayoutsQuery(module_id)));
    let extensions = db.query(VisibleExtensionsQuery(module_id));
    let extension_methods = db.query(ExtensionMethodsQuery);
    let program_signatures = db.query(ProgramSignaturesQuery);
    let program_item_signatures = db.query(ProgramItemSignaturesQuery);
    let program_comptime = db.query(ProgramComptimeQuery);
    let program_comptime_modules = db.query(ProgramComptimeModulesQuery);
    nia_body_check::check_module_bodies_with_program_signatures_and_layouts_with_timings(
        nia_body_check::BodyCheckInput {
            source_version: Some(source_version),
            origins: &origins,
            module: &module,
            defs: &defs,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
            lowered: &lowered,
            signatures: &signatures,
            normalization: &normalization,
            target: &db.query(CompilerTargetQuery),
            comptime: &comptime,
            comptime_module: &comptime_module.module,
            layouts: &layouts,
            extensions: &extensions.methods,
            program_extension_methods: &extension_methods.methods,
            extension_interner: Some(&extensions.interner),
            program: nia_body_check::BodyProgramContext {
                defs: Some(&program_defs),
                type_lowerings: Some(&program_type_lowerings),
                type_normalizations: Some(&program_type_normalizations),
                signatures: Some(&program_item_signatures),
                layouts: Some(&program_layouts),
            },
            program_signatures: program_signatures.maps(),
            program_comptime: nia_body_check::ProgramComptimeMaps {
                comptimes: &program_comptime,
                modules: &program_comptime_modules,
            },
            filter,
        },
        body_timing_mode(db.query(CompilerTimingsQuery)),
    )
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
        checked_module_with_body_check(db, module_id, db.query(BodyCheckQuery(module_id)))
    })
}

fn checked_module_with_body_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    body_check: nia_body_check::BodyCheck,
) -> CheckedModule {
    let path = db.query(ModulePathQuery(module_id));
    CheckedModule {
        id: module_id,
        path,
        defs: db.query(ModuleDefsQuery(module_id)),
        type_resolution: db.query(TypeResolutionQuery(module_id)),
        type_lowering: db.query(TypeLoweringQuery(module_id)),
        value_resolution: db.query(ValueResolutionQuery(module_id)),
        local_resolution: db.query(LocalResolutionQuery(module_id)),
        item_signatures: db.query(ItemSignaturesQuery(module_id)),
        type_normalization: db.query(TypeNormalizationQuery(module_id)),
        comptime: db.query(ComptimeQuery(module_id)),
        static_check: db.query(StaticCheckQuery(module_id)),
        layouts: db.query(LayoutsQuery(module_id)),
        abi_check: db.query(AbiCheckQuery(module_id)),
        flow_check: db.query(FlowCheckQuery(module_id)),
        body_ir: body_check.ir,
        semantic_uses: db.query(SemanticUseTableQuery(module_id)),
        semantic_facts: body_check.facts,
        body_diagnostics: body_check.diagnostics,
    }
}

pub(super) fn provide_checked_modules(db: &QueryDb<CompilerContext>) -> Vec<CheckedModule> {
    time_provider(db.query(CompilerTimingsQuery), "checked_modules", || {
        time_provider(
            db.query(CompilerTimingsQuery),
            "checked_modules.shared_inputs",
            || {
                let _ = db.query(ProgramTypeLoweringsQuery);
                let _ = db.query(ProgramItemSignaturesQuery);
                let _ = db.query(ProgramTypeNormalizationsQuery);
                let _ = db.query(ProgramSignaturesQuery);
                let _ = db.query(ExtensionMethodsQuery);
                let _ = db.query(ProgramComptimeModulesQuery);
                let _ = db.query(ProgramComptimeQuery);
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
            if db.query(CompilerRuntimeQuery) != RuntimeModel::FreestandingExecutable {
                return db.query(CheckedModulesQuery);
            }
            time_provider(
                db.query(CompilerTimingsQuery),
                "executable_checked_modules.shared_inputs",
                || {
                    let _ = db.query(ProgramTypeLoweringsQuery);
                    let _ = db.query(ProgramItemSignaturesQuery);
                    let _ = db.query(ProgramTypeNormalizationsQuery);
                    let _ = db.query(ProgramSignaturesQuery);
                    let _ = db.query(ExtensionMethodsQuery);
                    let _ = db.query(ProgramComptimeModulesQuery);
                    let _ = db.query(ProgramComptimeQuery);
                },
            );
            executable_checked_modules_inner(db)
        },
    )
}

fn executable_checked_modules_inner(db: &QueryDb<CompilerContext>) -> Vec<CheckedModule> {
    let parse_ok = db.query(ParseOkModuleIdsQuery);
    let graph = db.query(ModuleGraphQuery);
    let defs_by_id = defs_by_module_id(db);
    let program_signatures = db.query(ProgramSignaturesQuery);
    let extension_methods = db.query(ExtensionMethodsQuery);
    let checked_modules = db.query(CheckedModulesQuery);
    let checked_by_id = checked_modules
        .into_iter()
        .map(|module| (module.id, module))
        .collect::<HashMap<_, _>>();
    let reachable_inputs = checked_by_id
        .values()
        .map(|module| ReachableModuleInput {
            module_id: module.id,
            body_ir: &module.body_ir,
            item_signatures: &module.item_signatures,
            semantic_facts: &module.semantic_facts,
            type_lowering: &module.type_lowering,
            type_normalization: &module.type_normalization,
        })
        .collect::<Vec<_>>();
    let reachability = compute_executable_reachability(
        &parse_ok,
        &graph,
        &defs_by_id,
        program_signatures.maps(),
        &extension_methods.methods,
        &reachable_inputs,
    );

    if db.query(CompilerTimingsQuery).detail() {
        eprintln!(
            "query timing executable_checked_modules.reachable: modules={} functions={} bodies={} full_bodies={}",
            reachability.modules.len(),
            reachability.functions.len(),
            reachability.stats.reachable_bodies,
            reachability.stats.checked_bodies
        );
        eprintln!(
            "query timing executable_checked_modules.checked: modules={}",
            reachability.stats.checked_modules
        );
    }

    parse_ok
        .into_iter()
        .filter(|module_id| reachability.modules.contains(module_id))
        .filter_map(|module_id| checked_by_id.get(&module_id))
        .cloned()
        .map(|module| filter_checked_module_for_codegen(module, &reachability.functions))
        .collect()
}

fn filter_checked_module_for_codegen(
    mut module: CheckedModule,
    reachable_functions: &HashSet<GlobalDefId>,
) -> CheckedModule {
    module
        .body_ir
        .function_bodies
        .retain(|def_id, _| reachable_functions.contains(def_id));
    module.semantic_facts =
        filter_semantic_facts_for_reachable_functions(module.semantic_facts, reachable_functions);
    module
}

pub(super) fn provide_monomorphization(
    db: &QueryDb<CompilerContext>,
) -> nia_monomorphize::Monomorphization {
    time_provider(db.query(CompilerTimingsQuery), "monomorphization", || {
        let checked_modules = checked_modules_for_codegen(db);
        let program_signatures = db.query(ProgramSignaturesQuery);
        let function_bodies = function_bodies_from_checked_modules(&checked_modules);
        nia_monomorphize::collect_monomorphizations(
            &checked_modules
                .iter()
                .map(|module| MonomorphizeModuleInput {
                    module_id: module.id,
                    defs: &module.defs,
                    interner: &function_bodies
                        .get(&module.id)
                        .expect("missing lowered function bodies")
                        .interner,
                    normalization: &module.type_normalization,
                    comptime: &module.comptime,
                    const_exprs: &module.type_lowering.const_exprs,
                    layouts: Some(&module.layouts),
                    local_enums: &module.item_signatures.enums,
                    program_enums: &program_signatures.enums,
                    trait_impls: &program_signatures.trait_impls,
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

fn function_bodies_from_checked_modules(
    checked_modules: &[CheckedModule],
) -> HashMap<ModuleId, LoweredFunctionBodies> {
    checked_modules
        .iter()
        .map(|module| {
            let (interner, bodies) = nia_function_lower::lower_function_bodies_with_interner(
                module.body_ir.function_bodies.iter(),
                &module.body_ir.interner,
            );
            (module.id, LoweredFunctionBodies { interner, bodies })
        })
        .collect()
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
    let (module_asts, visible_extensions, extension_methods, function_bodies) = time_provider(
        db.query(CompilerTimingsQuery),
        "backend_lowering.inputs",
        || {
            let module_asts = checked_modules
                .iter()
                .map(|checked_module| db.query(ModuleAstQuery(checked_module.id)))
                .collect::<Vec<_>>();
            let visible_extensions = checked_modules
                .iter()
                .map(|checked_module| db.query(VisibleExtensionsQuery(checked_module.id)))
                .collect::<Vec<_>>();
            let extension_methods = db.query(ExtensionMethodsQuery);
            let function_bodies_by_id = function_bodies_from_checked_modules(&checked_modules);
            let function_bodies = checked_modules
                .iter()
                .map(|checked_module| {
                    function_bodies_by_id
                        .get(&checked_module.id)
                        .expect("missing lowered function bodies")
                        .clone()
                })
                .collect::<Vec<_>>();
            (
                module_asts,
                visible_extensions,
                extension_methods,
                function_bodies,
            )
        },
    );
    let indexes = time_provider(
        db.query(CompilerTimingsQuery),
        "backend_lowering.indexes",
        || build_backend_lowering_indexes(&checked_modules, &visible_extensions, &function_bodies),
    );
    let program_defs = db.query(ProgramDefsByIdQuery);
    let program_signatures = db.query(ProgramSignaturesQuery);
    let inputs = time_provider(
        db.query(CompilerTimingsQuery),
        "backend_lowering.module_inputs",
        || {
            build_backend_lowering_module_inputs(BackendLoweringModuleInputsInput {
                checked_modules: &checked_modules,
                module_asts: &module_asts,
                visible_extensions: &visible_extensions,
                function_bodies: &function_bodies,
                extension_methods: &extension_methods,
                program_defs: program_defs.as_ref(),
                program_signatures: &program_signatures,
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

pub(super) fn provide_program_diagnostics(db: &QueryDb<CompilerContext>) -> Vec<ProgramDiagnostic> {
    time_provider(
        db.query(CompilerTimingsQuery),
        "program_diagnostics",
        || provide_program_diagnostics_inner(db),
    )
}

fn provide_program_diagnostics_inner(db: &QueryDb<CompilerContext>) -> Vec<ProgramDiagnostic> {
    let mut diagnostics = db.query(ProgramLoadDiagnosticsQuery);
    for module_id in db.query(LoadedModulesQuery) {
        let parse_errors = db.query(ModuleParseErrorsQuery(module_id));
        let path = db.query(ModulePathQuery(module_id));
        for error in &parse_errors {
            diagnostics.push(ProgramDiagnostic {
                path: path.clone(),
                diagnostic: Diagnostic::user_error_at("E0201", error.span, error.message.clone()),
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
    let first_path = db
        .query(ParseOkModuleIdsQuery)
        .first()
        .map(|module_id| db.query(ModulePathQuery(*module_id)))
        .unwrap_or_else(|| SourcePath::new("<unknown>"));
    diagnostics.extend(
        db.query(ExtensionMethodsQuery)
            .diagnostics
            .iter()
            .cloned()
            .map(|diagnostic| ProgramDiagnostic {
                path: first_path.clone(),
                diagnostic,
            }),
    );

    let checked_modules = db.query(CheckedModulesQuery);
    for checked in &checked_modules {
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
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.item_signatures.diagnostics,
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

    let monomorphization = db.query(MonomorphizationQuery);
    diagnostics.extend(
        monomorphization
            .diagnostics
            .iter()
            .cloned()
            .map(|diagnostic| ProgramDiagnostic {
                path: path_for_diagnostic_span(
                    &checked_modules,
                    diagnostic.primary_span().unwrap_or_default(),
                ),
                diagnostic,
            }),
    );
    let backend_lowering = db.query(BackendLoweringQuery);
    diagnostics.extend(
        backend_lowering
            .diagnostics
            .iter()
            .cloned()
            .map(|diagnostic| ProgramDiagnostic {
                path: path_for_diagnostic_span(
                    &checked_modules,
                    diagnostic.primary_span().unwrap_or_default(),
                ),
                diagnostic,
            }),
    );
    diagnostics
}
