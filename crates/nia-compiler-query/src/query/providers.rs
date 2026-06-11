// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::RuntimeModel;
use nia_defs::DefKind;
use nia_ids::{InternedTyId, TraitId};
use nia_sema_ir::ResolvedCall;
use nia_ty::{AssociatedTypeBindingTy, TyInterner, TyKind};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

#[derive(Clone)]
pub(super) struct CompilerQueryProviders {
    pub(super) checked_program: fn(&QueryDb<DriverContext>) -> CheckedProgram,
    pub(super) module_graph: fn(&QueryDb<DriverContext>) -> ModuleGraph,
    pub(super) parse_ok_module_ids: fn(&QueryDb<DriverContext>) -> Vec<ModuleId>,
    pub(super) loaded_module: fn(&QueryDb<DriverContext>, ModuleId) -> LoadedModule,
    pub(super) module_item_tree: fn(&QueryDb<DriverContext>, ModuleId) -> ModuleItemTree,
    pub(super) active_module_item_tree:
        fn(&QueryDb<DriverContext>, ModuleId) -> ActiveModuleItemTree,
    pub(super) module_defs: fn(&QueryDb<DriverContext>, ModuleId) -> DefCollection,
    pub(super) defs_by_module: fn(&QueryDb<DriverContext>) -> Vec<DefCollection>,
    pub(super) program_defs_by_id: fn(&QueryDb<DriverContext>) -> ProgramDefsById,
    pub(super) public_surface: fn(&QueryDb<DriverContext>) -> PublicSurfaceQueryValue,
    pub(super) type_resolution: fn(&QueryDb<DriverContext>, ModuleId) -> TypeResolution,
    pub(super) type_lowering: fn(&QueryDb<DriverContext>, ModuleId) -> TypeLowering,
    pub(super) program_type_lowerings: fn(&QueryDb<DriverContext>) -> ProgramTypeLowerings,
    pub(super) item_signatures: fn(&QueryDb<DriverContext>, ModuleId) -> ItemSignatures,
    pub(super) program_item_signatures: fn(&QueryDb<DriverContext>) -> ProgramItemSignaturesById,
    pub(super) type_normalization: fn(&QueryDb<DriverContext>, ModuleId) -> TypeNormalization,
    pub(super) program_type_normalizations:
        fn(&QueryDb<DriverContext>) -> ProgramTypeNormalizations,
    pub(super) program_signatures: fn(&QueryDb<DriverContext>) -> ProgramSignaturesValue,
    pub(super) extension_methods: fn(&QueryDb<DriverContext>) -> ExtensionMethodsValue,
    pub(super) visible_extensions: fn(&QueryDb<DriverContext>, ModuleId) -> VisibleExtensionsValue,
    pub(super) value_resolution: fn(&QueryDb<DriverContext>, ModuleId) -> ValueResolution,
    pub(super) local_resolution: fn(&QueryDb<DriverContext>, ModuleId) -> LocalResolution,
    pub(super) semantic_use_table:
        fn(&QueryDb<DriverContext>, ModuleId) -> nia_sema_ir::SemanticUseTable,
    pub(super) comptime_module: fn(&QueryDb<DriverContext>, ModuleId) -> ComptimeModuleLowering,
    pub(super) program_comptime_modules: fn(&QueryDb<DriverContext>) -> ProgramComptimeModules,
    pub(super) comptime: fn(&QueryDb<DriverContext>, ModuleId) -> ComptimeCheck,
    pub(super) program_comptime: fn(&QueryDb<DriverContext>) -> ProgramComptimeById,
    pub(super) layouts: fn(&QueryDb<DriverContext>, ModuleId) -> nia_layout::Layouts,
    pub(super) abi_check: fn(&QueryDb<DriverContext>, ModuleId) -> nia_abi_check::AbiCheck,
    pub(super) static_check: fn(&QueryDb<DriverContext>, ModuleId) -> nia_static_check::StaticCheck,
    pub(super) flow_check: fn(&QueryDb<DriverContext>, ModuleId) -> nia_flow_check::FlowCheck,
    pub(super) body_check: fn(&QueryDb<DriverContext>, ModuleId) -> nia_body_check::BodyCheck,
    pub(super) body_ir: fn(&QueryDb<DriverContext>, ModuleId) -> nia_body_ir::BodyIr,
    pub(super) semantic_facts: fn(&QueryDb<DriverContext>, ModuleId) -> nia_sema_ir::SemanticFacts,
    pub(super) body_diagnostics: fn(&QueryDb<DriverContext>, ModuleId) -> Vec<Diagnostic>,
    pub(super) function_bodies: fn(&QueryDb<DriverContext>, ModuleId) -> LoweredFunctionBodies,
    pub(super) checked_module: fn(&QueryDb<DriverContext>, ModuleId) -> CheckedModule,
    pub(super) checked_modules: fn(&QueryDb<DriverContext>) -> Vec<CheckedModule>,
    pub(super) monomorphization: fn(&QueryDb<DriverContext>) -> nia_monomorphize::Monomorphization,
    pub(super) backend_lowering: fn(&QueryDb<DriverContext>) -> nia_backend_lower::BackendLowering,
    pub(super) program_diagnostics: fn(&QueryDb<DriverContext>) -> Vec<ProgramDiagnostic>,
}

impl Default for CompilerQueryProviders {
    fn default() -> Self {
        Self {
            checked_program: provide_checked_program,
            module_graph: provide_module_graph,
            parse_ok_module_ids: provide_parse_ok_module_ids,
            loaded_module: provide_loaded_module,
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
            body_ir: provide_body_ir,
            semantic_facts: provide_semantic_facts,
            body_diagnostics: provide_body_diagnostics,
            function_bodies: provide_function_bodies,
            checked_module: provide_checked_module,
            checked_modules: provide_checked_modules,
            monomorphization: provide_monomorphization,
            backend_lowering: provide_backend_lowering,
            program_diagnostics: provide_program_diagnostics,
        }
    }
}

pub(super) fn provide_checked_program(db: &QueryDb<DriverContext>) -> CheckedProgram {
    time_provider(db.context().timings, "checked_program", || CheckedProgram {
        graph: db.query(ModuleGraphQuery),
        optimization: db.context().optimization,
        modules: db.query(CheckedModulesQuery),
        monomorphization: db.query(MonomorphizationQuery),
        backend_lowering: db.query(BackendLoweringQuery),
        diagnostics: db.query(ProgramDiagnosticsQuery),
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
    db: &QueryDb<DriverContext>,
    name: &str,
    module_id: ModuleId,
    f: impl FnOnce() -> T,
) -> T {
    let timings = db.context().timings;
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

pub(super) fn provide_module_graph(db: &QueryDb<DriverContext>) -> ModuleGraph {
    db.context().loaded.graph.clone()
}

pub(super) fn provide_parse_ok_module_ids(db: &QueryDb<DriverContext>) -> Vec<ModuleId> {
    db.context()
        .loaded
        .modules
        .iter()
        .filter(|module| module.parse_errors.is_empty())
        .map(|module| module.id)
        .collect()
}

pub(super) fn provide_loaded_module(
    db: &QueryDb<DriverContext>,
    module_id: ModuleId,
) -> LoadedModule {
    db.context()
        .loaded_module(module_id)
        .unwrap_or_else(|| {
            db.invalid_input(
                &LoadedModuleQuery(module_id),
                format!("missing loaded module {module_id:?}"),
            )
        })
        .clone()
}

pub(super) fn provide_module_item_tree(
    db: &QueryDb<DriverContext>,
    module_id: ModuleId,
) -> ModuleItemTree {
    let loaded = db.query(LoadedModuleQuery(module_id));
    loaded.item_tree
}

pub(super) fn provide_active_module_item_tree(
    db: &QueryDb<DriverContext>,
    module_id: ModuleId,
) -> ActiveModuleItemTree {
    let _raw_item_tree = db.query(ModuleItemTreeQuery(module_id));
    let loaded = db.query(LoadedModuleQuery(module_id));
    loaded.active_item_tree
}

pub(super) fn provide_module_defs(
    db: &QueryDb<DriverContext>,
    module_id: ModuleId,
) -> DefCollection {
    let item_tree = db.query(ActiveModuleItemTreeQuery(module_id));
    nia_defs::collect_module_defs_from_active_item_tree(module_id, &item_tree)
}

pub(super) fn provide_defs_by_module(db: &QueryDb<DriverContext>) -> Vec<DefCollection> {
    db.query_many(
        db.query(ParseOkModuleIdsQuery)
            .into_iter()
            .map(ModuleDefsQuery),
    )
}

pub(super) fn provide_program_defs_by_id(db: &QueryDb<DriverContext>) -> ProgramDefsById {
    Arc::new(
        db.query(ParseOkModuleIdsQuery)
            .into_iter()
            .map(|module_id| (module_id, db.query(ModuleDefsQuery(module_id))))
            .collect(),
    )
}

pub(super) fn provide_public_surface(db: &QueryDb<DriverContext>) -> PublicSurfaceQueryValue {
    time_provider(db.context().timings, "public_surface", || {
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
    db: &QueryDb<DriverContext>,
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
    db: &QueryDb<DriverContext>,
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
    db: &QueryDb<DriverContext>,
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
    db: &QueryDb<DriverContext>,
) -> ProgramItemSignaturesById {
    time_provider(db.context().timings, "program_item_signatures", || {
        Arc::new(
            db.query(ParseOkModuleIdsQuery)
                .into_iter()
                .map(|module_id| (module_id, db.query(ItemSignaturesQuery(module_id))))
                .collect(),
        )
    })
}

pub(super) fn provide_type_normalization(
    db: &QueryDb<DriverContext>,
    module_id: ModuleId,
) -> TypeNormalization {
    let type_lowering = db.query(TypeLoweringQuery(module_id));
    let item_signatures = db.query(ItemSignaturesQuery(module_id));
    nia_type_normalize::normalize_module_types(module_id, &type_lowering.interner, &item_signatures)
}

pub(super) fn provide_program_type_lowerings(db: &QueryDb<DriverContext>) -> ProgramTypeLowerings {
    time_provider(db.context().timings, "program_type_lowerings", || {
        Arc::new(
            db.query(ParseOkModuleIdsQuery)
                .into_iter()
                .map(|module_id| (module_id, db.query(TypeLoweringQuery(module_id))))
                .collect(),
        )
    })
}

pub(super) fn provide_program_type_normalizations(
    db: &QueryDb<DriverContext>,
) -> ProgramTypeNormalizations {
    time_provider(db.context().timings, "program_type_normalizations", || {
        Arc::new(
            db.query(ParseOkModuleIdsQuery)
                .into_iter()
                .map(|module_id| (module_id, db.query(TypeNormalizationQuery(module_id))))
                .collect(),
        )
    })
}

pub(super) fn provide_program_signatures(db: &QueryDb<DriverContext>) -> ProgramSignaturesValue {
    time_provider(db.context().timings, "program_signatures", || {
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

pub(super) fn provide_extension_methods(db: &QueryDb<DriverContext>) -> ExtensionMethodsValue {
    time_provider(db.context().timings, "extension_methods", || {
        let module_ids = db.query(ParseOkModuleIdsQuery);
        let modules = module_ids
            .iter()
            .copied()
            .map(|module_id| db.query(LoadedModuleQuery(module_id)))
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
    db: &QueryDb<DriverContext>,
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
    db: &QueryDb<DriverContext>,
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
    db: &QueryDb<DriverContext>,
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
    db: &QueryDb<DriverContext>,
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
    db: &QueryDb<DriverContext>,
    module_id: ModuleId,
) -> ComptimeModuleLowering {
    let loaded = db.query(LoadedModuleQuery(module_id));
    let defs = db.query(ModuleDefsQuery(module_id));
    let values = db.query(ValueResolutionQuery(module_id));
    let locals = db.query(LocalResolutionQuery(module_id));
    let semantic_uses = db.query(SemanticUseTableQuery(module_id));
    let type_lowering = db.query(TypeLoweringQuery(module_id));
    nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
        module: &loaded.module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        const_exprs: &type_lowering.const_exprs,
    })
}

pub(super) fn provide_program_comptime_modules(
    db: &QueryDb<DriverContext>,
) -> ProgramComptimeModules {
    let ids = db.query(ParseOkModuleIdsQuery);
    let modules = db.query_many(ids.iter().copied().map(ComptimeModuleQuery));
    Arc::new(
        ids.into_iter()
            .zip(modules.into_iter().map(|lowering| lowering.module))
            .collect(),
    )
}

pub(super) fn provide_comptime(db: &QueryDb<DriverContext>, module_id: ModuleId) -> ComptimeCheck {
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
                target: &db.context().target,
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

pub(super) fn provide_program_comptime(db: &QueryDb<DriverContext>) -> ProgramComptimeById {
    time_provider(db.context().timings, "program_comptime", || {
        let ids = db.query(ParseOkModuleIdsQuery);
        let comptimes = db.query_many(ids.iter().copied().map(ComptimeQuery));
        Arc::new(ids.into_iter().zip(comptimes).collect())
    })
}

pub(super) fn provide_layouts(
    db: &QueryDb<DriverContext>,
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
    db: &QueryDb<DriverContext>,
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
    db: &QueryDb<DriverContext>,
    module_id: ModuleId,
) -> nia_static_check::StaticCheck {
    let loaded = db.query(LoadedModuleQuery(module_id));
    let defs = db.query(ModuleDefsQuery(module_id));
    let values = db.query(ValueResolutionQuery(module_id));
    let locals = db.query(LocalResolutionQuery(module_id));
    let semantic_uses = db.query(SemanticUseTableQuery(module_id));
    let signatures = db.query(ItemSignaturesQuery(module_id));
    let comptime = db.query(ComptimeQuery(module_id));
    let program_defs = db.query(ProgramDefsByIdQuery);
    let program_comptime = db.query(ProgramComptimeQuery);
    nia_static_check::check_module_static_initializers(nia_static_check::StaticCheckInput {
        module: &loaded.module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        signatures: &signatures,
        comptime: &comptime,
        program_defs: &program_defs,
        program_comptime: &program_comptime,
        target: &db.context().target,
    })
}

pub(super) fn provide_flow_check(
    db: &QueryDb<DriverContext>,
    module_id: ModuleId,
) -> nia_flow_check::FlowCheck {
    let loaded = db.query(LoadedModuleQuery(module_id));
    let type_lowering = db.query(TypeLoweringQuery(module_id));
    let signatures = db.query(ItemSignaturesQuery(module_id));
    nia_flow_check::check_module_flow(&loaded.module, &type_lowering.interner, &signatures)
}

pub(super) fn provide_body_check(
    db: &QueryDb<DriverContext>,
    module_id: ModuleId,
) -> nia_body_check::BodyCheck {
    time_module_provider(db, "body_check", module_id, || {
        let loaded = db.query(LoadedModuleQuery(module_id));
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
                source_version: Some(loaded.source_version),
                origins: &loaded.origins,
                module: &loaded.module,
                defs: &defs,
                values: &values,
                locals: &locals,
                semantic_uses: &semantic_uses,
                lowered: &lowered,
                signatures: &signatures,
                normalization: &normalization,
                target: &db.context().target,
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
            },
            body_timing_mode(db.context().timings),
        )
    })
}

fn body_timing_mode(timings: TimingMode) -> nia_body_check::BodyTimingMode {
    if timings.detail() {
        nia_body_check::BodyTimingMode::Detail
    } else {
        nia_body_check::BodyTimingMode::Off
    }
}

pub(super) fn provide_function_bodies(
    db: &QueryDb<DriverContext>,
    module_id: ModuleId,
) -> LoweredFunctionBodies {
    time_module_provider(db, "function_bodies", module_id, || {
        let body_ir = db.query(BodyIrQuery(module_id));
        let (interner, bodies) = nia_function_lower::lower_function_bodies_with_interner(
            body_ir.function_bodies.iter(),
            &body_ir.interner,
        );
        LoweredFunctionBodies { interner, bodies }
    })
}

pub(super) fn provide_body_ir(
    db: &QueryDb<DriverContext>,
    module_id: ModuleId,
) -> nia_body_ir::BodyIr {
    db.query(BodyCheckQuery(module_id)).ir
}

pub(super) fn provide_semantic_facts(
    db: &QueryDb<DriverContext>,
    module_id: ModuleId,
) -> nia_sema_ir::SemanticFacts {
    db.query(BodyCheckQuery(module_id)).facts
}

pub(super) fn provide_body_diagnostics(
    db: &QueryDb<DriverContext>,
    module_id: ModuleId,
) -> Vec<Diagnostic> {
    db.query(BodyCheckQuery(module_id)).diagnostics
}

pub(super) fn provide_checked_module(
    db: &QueryDb<DriverContext>,
    module_id: ModuleId,
) -> CheckedModule {
    time_module_provider(db, "checked_module", module_id, || {
        let loaded = db.query(LoadedModuleQuery(module_id));
        CheckedModule {
            id: loaded.id,
            path: loaded.path,
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
            body_ir: db.query(BodyIrQuery(module_id)),
            semantic_uses: db.query(SemanticUseTableQuery(module_id)),
            semantic_facts: db.query(SemanticFactsQuery(module_id)),
            body_diagnostics: db.query(BodyDiagnosticsQuery(module_id)),
        }
    })
}

pub(super) fn provide_checked_modules(db: &QueryDb<DriverContext>) -> Vec<CheckedModule> {
    time_provider(db.context().timings, "checked_modules", || {
        time_provider(
            db.context().timings,
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

pub(super) fn provide_monomorphization(
    db: &QueryDb<DriverContext>,
) -> nia_monomorphize::Monomorphization {
    time_provider(db.context().timings, "monomorphization", || {
        let checked_modules = db.query(CheckedModulesQuery);
        let program_signatures = db.query(ProgramSignaturesQuery);
        let function_bodies = checked_modules
            .iter()
            .map(|module| (module.id, db.query(FunctionBodiesQuery(module.id))))
            .collect::<HashMap<_, _>>();
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

pub(super) fn provide_backend_lowering(
    db: &QueryDb<DriverContext>,
) -> nia_backend_lower::BackendLowering {
    time_provider(db.context().timings, "backend_lowering", || {
        provide_backend_lowering_inner(db)
    })
}

fn provide_backend_lowering_inner(
    db: &QueryDb<DriverContext>,
) -> nia_backend_lower::BackendLowering {
    let all_checked_modules = db.query(CheckedModulesQuery);
    let monomorphization = db.query(MonomorphizationQuery);
    let checked_modules = backend_checked_modules(db, all_checked_modules, &monomorphization);
    let (loaded_modules, visible_extensions, extension_methods, function_bodies) =
        time_provider(db.context().timings, "backend_lowering.inputs", || {
            let loaded_modules = checked_modules
                .iter()
                .map(|checked_module| db.query(LoadedModuleQuery(checked_module.id)))
                .collect::<Vec<_>>();
            let visible_extensions = checked_modules
                .iter()
                .map(|checked_module| db.query(VisibleExtensionsQuery(checked_module.id)))
                .collect::<Vec<_>>();
            let extension_methods = db.query(ExtensionMethodsQuery);
            let function_bodies = checked_modules
                .iter()
                .map(|checked_module| db.query(FunctionBodiesQuery(checked_module.id)))
                .collect::<Vec<_>>();
            (
                loaded_modules,
                visible_extensions,
                extension_methods,
                function_bodies,
            )
        });
    let (program_extensions, program_type_interners, program_function_bodies) =
        time_provider(db.context().timings, "backend_lowering.indexes", || {
            let program_extensions = checked_modules
                .iter()
                .zip(visible_extensions.iter())
                .map(|(checked_module, visible_extensions)| {
                    (
                        checked_module.id,
                        (&visible_extensions.methods, &visible_extensions.interner),
                    )
                })
                .collect::<HashMap<_, _>>();
            let program_type_interners = checked_modules
                .iter()
                .zip(function_bodies.iter())
                .map(|(checked_module, lowered)| (checked_module.id, &lowered.interner))
                .collect::<HashMap<_, _>>();
            let program_function_bodies = function_bodies
                .iter()
                .flat_map(|lowered| {
                    lowered
                        .bodies
                        .iter()
                        .map(|(def_id, body)| (*def_id, body.clone()))
                })
                .collect::<HashMap<_, _>>();
            (
                program_extensions,
                program_type_interners,
                program_function_bodies,
            )
        });
    let program_defs = db.query(ProgramDefsByIdQuery);
    let program_signatures = db.query(ProgramSignaturesQuery);
    let inputs = time_provider(
        db.context().timings,
        "backend_lowering.module_inputs",
        || {
            checked_modules
                .iter()
                .zip(loaded_modules.iter())
                .zip(visible_extensions.iter())
                .zip(function_bodies.iter())
                .map(
                    |(((checked_module, loaded_module), visible_extensions), function_bodies)| {
                        BackendLowerModuleInput {
                            module_id: checked_module.id,
                            module_name: checked_module.path.as_str().to_string(),
                            module: &loaded_module.module,
                            defs: &checked_module.defs,
                            extensions: &visible_extensions.methods,
                            values: &checked_module.value_resolution,
                            locals: &checked_module.local_resolution,
                            type_lowering: &checked_module.type_lowering,
                            signatures: &checked_module.item_signatures,
                            type_normalization: &checked_module.type_normalization,
                            body_ir: &checked_module.body_ir,
                            function_interner: &function_bodies.interner,
                            semantic_facts: &checked_module.semantic_facts,
                            comptime: &checked_module.comptime,
                            layouts: &checked_module.layouts,
                            function_bodies: &function_bodies.bodies,
                            program_function_bodies: &program_function_bodies,
                            extension_interner: Some(&visible_extensions.interner),
                            program_extension_methods: &extension_methods.methods,
                            program_extensions: &program_extensions,
                            program_defs: &program_defs,
                            program_type_interners: &program_type_interners,
                            program_functions: &program_signatures.functions,
                            program_structs: &program_signatures.structs,
                            program_unions: &program_signatures.unions,
                            program_enums: &program_signatures.enums,
                            program_traits: &program_signatures.traits,
                            trait_impls: &program_signatures.trait_impls,
                        }
                    },
                )
                .collect::<Vec<_>>()
        },
    );
    time_provider(
        db.context().timings,
        "backend_lowering.lower_backend_program",
        || {
            nia_backend_lower::lower_backend_program_with_timings(
                &inputs,
                &monomorphization,
                db.context().optimization,
                backend_timing_mode(db.context().timings),
            )
        },
    )
}

fn backend_checked_modules(
    db: &QueryDb<DriverContext>,
    checked_modules: Vec<CheckedModule>,
    monomorphization: &nia_monomorphize::Monomorphization,
) -> Vec<CheckedModule> {
    if db.context().loaded.runtime != RuntimeModel::FreestandingExecutable {
        return checked_modules;
    }
    let reachable = executable_backend_modules(db, &checked_modules, monomorphization);
    if reachable.is_empty() {
        return checked_modules;
    }
    checked_modules
        .into_iter()
        .filter(|module| reachable.contains(&module.id))
        .collect()
}

fn executable_backend_modules(
    db: &QueryDb<DriverContext>,
    checked_modules: &[CheckedModule],
    monomorphization: &nia_monomorphize::Monomorphization,
) -> HashSet<ModuleId> {
    let mut functions_by_def = HashMap::new();
    let mut named_functions = HashMap::<(ModuleId, &str), GlobalDefId>::new();
    for module in checked_modules {
        for (def_id, def) in module.defs.defs.iter() {
            if def.kind != DefKind::Function {
                continue;
            }
            let global = GlobalDefId {
                module_id: module.id,
                def_id,
            };
            functions_by_def.insert(global, module);
            named_functions.insert((module.id, def.name.as_str()), global);
        }
    }

    let graph = db.query(ModuleGraphQuery);
    let mut pending = VecDeque::new();
    if let Some(root_main) = named_functions.get(&(graph.root(), "main")).copied() {
        pending.push_back(root_main);
    }
    if let Some(start_module) = freestanding_start_module(&graph)
        && let Some(start) = named_functions.get(&(start_module, "_start")).copied()
    {
        pending.push_back(start);
    }

    let mut reachable_modules = HashSet::new();
    let mut pending_modules = VecDeque::new();
    let mut expanded_modules = HashSet::new();
    let mut reachable_traits = HashSet::new();
    let mut functions_seen = HashSet::new();
    let checked_by_id = checked_modules
        .iter()
        .map(|module| (module.id, module))
        .collect::<HashMap<_, _>>();
    while let Some(function) = pending.pop_front() {
        if !functions_seen.insert(function) {
            continue;
        }
        add_reachable_module(
            function.module_id,
            &mut reachable_modules,
            &mut pending_modules,
        );
        expand_reachable_module_owners(
            function.module_id,
            &checked_by_id,
            &mut reachable_modules,
            &mut pending_modules,
            &mut expanded_modules,
            &mut reachable_traits,
        );
        let Some(module) = functions_by_def.get(&function).copied() else {
            continue;
        };
        for callee in semantic_fact_callees(module) {
            if functions_by_def.contains_key(&callee) {
                pending.push_back(callee);
            } else {
                add_reachable_module(
                    callee.module_id,
                    &mut reachable_modules,
                    &mut pending_modules,
                );
            }
        }
    }

    add_reachable_module(graph.root(), &mut reachable_modules, &mut pending_modules);
    for instance in &monomorphization.instances {
        add_reachable_module(
            instance.def_id.module_id,
            &mut reachable_modules,
            &mut pending_modules,
        );
        collect_ty_ids_owner_modules(&instance.args, &mut reachable_modules, &mut pending_modules);
    }
    while let Some(module_id) = pending_modules.pop_front() {
        expand_reachable_module_owners(
            module_id,
            &checked_by_id,
            &mut reachable_modules,
            &mut pending_modules,
            &mut expanded_modules,
            &mut reachable_traits,
        );
    }
    let program_signatures = db.query(ProgramSignaturesQuery);
    for trait_impl in &program_signatures.trait_impls {
        if reachable_traits.contains(&trait_impl.trait_id)
            || trait_id_module_is_reachable(trait_impl.trait_id, &reachable_modules)
        {
            add_reachable_module(
                trait_impl.module_id,
                &mut reachable_modules,
                &mut pending_modules,
            );
        }
    }
    while let Some(module_id) = pending_modules.pop_front() {
        expand_reachable_module_owners(
            module_id,
            &checked_by_id,
            &mut reachable_modules,
            &mut pending_modules,
            &mut expanded_modules,
            &mut reachable_traits,
        );
    }
    reachable_modules
}

fn trait_id_module_is_reachable(trait_id: TraitId, reachable_modules: &HashSet<ModuleId>) -> bool {
    match trait_id {
        TraitId::Source(def_id) => reachable_modules.contains(&def_id.module_id),
        TraitId::Builtin(_) => true,
    }
}

fn add_reachable_module(
    module_id: ModuleId,
    reachable_modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    if reachable_modules.insert(module_id) {
        pending_modules.push_back(module_id);
    }
}

fn expand_reachable_module_owners(
    module_id: ModuleId,
    checked_modules: &HashMap<ModuleId, &CheckedModule>,
    reachable_modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    expanded_modules: &mut HashSet<ModuleId>,
    reachable_traits: &mut HashSet<TraitId>,
) {
    if !expanded_modules.insert(module_id) {
        return;
    }
    let Some(module) = checked_modules.get(&module_id).copied() else {
        return;
    };
    expand_reachable_module(module, reachable_modules, pending_modules, reachable_traits);
}

fn expand_reachable_module(
    module: &CheckedModule,
    reachable_modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    reachable_traits: &mut HashSet<TraitId>,
) {
    for def_id in semantic_fact_owner_defs(module) {
        add_reachable_module(def_id.module_id, reachable_modules, pending_modules);
    }
    for def_id in semantic_fact_callees(module) {
        add_reachable_module(def_id.module_id, reachable_modules, pending_modules);
    }
    collect_semantic_fact_trait_ids(module, reachable_traits);
    collect_interner_owner_modules(
        &module.type_normalization.interner,
        reachable_modules,
        pending_modules,
        reachable_traits,
    );
    collect_interner_owner_modules(
        &module.body_ir.interner,
        reachable_modules,
        pending_modules,
        reachable_traits,
    );
}

fn freestanding_start_module(graph: &ModuleGraph) -> Option<ModuleId> {
    graph.module_id_for_module_path(&nia_imports::ModulePath {
        package: nia_imports::STD_MODULE_MAP_NAME.to_string(),
        segments: vec![
            "start".to_string(),
            "freestanding".to_string(),
            "linux".to_string(),
            "x86_64".to_string(),
        ],
    })
}

fn semantic_fact_callees(module: &CheckedModule) -> Vec<GlobalDefId> {
    let mut callees = Vec::new();
    callees.extend(
        module
            .semantic_facts
            .node_function_references
            .values()
            .map(|reference| reference.def_id),
    );
    callees.extend(
        module
            .semantic_facts
            .generic_instantiations
            .iter()
            .map(|instantiation| instantiation.def_id),
    );
    for call in module.semantic_facts.node_resolved_calls.values() {
        match call {
            ResolvedCall::Function(def_id)
            | ResolvedCall::FunctionInstance { def_id, .. }
            | ResolvedCall::Method { def_id, .. }
            | ResolvedCall::TraitMethod {
                method_id: def_id, ..
            }
            | ResolvedCall::DynamicTraitMethod {
                method_id: def_id, ..
            } => {
                callees.push(*def_id);
            }
            ResolvedCall::BuiltinTraitMethod { .. }
            | ResolvedCall::BuiltinMethod { .. }
            | ResolvedCall::BuiltinPlaceMethod { .. }
            | ResolvedCall::FunctionPointer => {}
        }
    }
    callees
}

fn semantic_fact_owner_defs(module: &CheckedModule) -> Vec<GlobalDefId> {
    let mut owners = Vec::new();
    owners.extend(
        module
            .semantic_uses
            .node_value_uses
            .values()
            .filter_map(|value| match value {
                nia_sema_ir::SemanticValueUse::Global(def_id) => Some(*def_id),
                nia_sema_ir::SemanticValueUse::Local(_) => None,
            }),
    );
    for call in module.semantic_facts.node_resolved_calls.values() {
        match call {
            ResolvedCall::Function(def_id)
            | ResolvedCall::FunctionInstance { def_id, .. }
            | ResolvedCall::Method { def_id, .. } => owners.push(*def_id),
            ResolvedCall::TraitMethod {
                trait_id,
                method_id,
                ..
            } => {
                owners.push(*trait_id);
                owners.push(*method_id);
            }
            ResolvedCall::DynamicTraitMethod { method_id, .. } => owners.push(*method_id),
            ResolvedCall::BuiltinTraitMethod { .. }
            | ResolvedCall::BuiltinMethod { .. }
            | ResolvedCall::BuiltinPlaceMethod { .. }
            | ResolvedCall::FunctionPointer => {}
        }
    }
    owners
}

fn collect_semantic_fact_trait_ids(module: &CheckedModule, traits: &mut HashSet<TraitId>) {
    for call in module.semantic_facts.node_resolved_calls.values() {
        match call {
            ResolvedCall::TraitMethod { trait_id, .. } => {
                traits.insert(TraitId::Source(*trait_id));
            }
            ResolvedCall::DynamicTraitMethod { trait_id, .. } => {
                traits.insert(*trait_id);
            }
            ResolvedCall::BuiltinTraitMethod { trait_id, .. }
            | ResolvedCall::BuiltinPlaceMethod { trait_id, .. } => {
                traits.insert(TraitId::Builtin(*trait_id));
            }
            ResolvedCall::Function(_)
            | ResolvedCall::FunctionInstance { .. }
            | ResolvedCall::Method { .. }
            | ResolvedCall::BuiltinMethod { .. }
            | ResolvedCall::FunctionPointer => {}
        }
    }
}

fn collect_interner_owner_modules(
    interner: &TyInterner,
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut HashSet<TraitId>,
) {
    for (_, ty) in interner.iter() {
        collect_ty_owner_modules(ty, modules, pending_modules, traits);
    }
}

fn collect_ty_owner_modules(
    ty: &TyKind,
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut HashSet<TraitId>,
) {
    match ty {
        TyKind::Nominal { def_id, args } => {
            add_reachable_module(def_id.module_id, modules, pending_modules);
            for arg in args {
                add_reachable_module(arg.interner_id, modules, pending_modules);
            }
        }
        TyKind::Pointer { elem, .. }
        | TyKind::Slice { elem, .. }
        | TyKind::SlicePointee { elem }
        | TyKind::Array { elem, .. }
        | TyKind::Optional { elem } => {
            add_reachable_module(elem.interner_id, modules, pending_modules);
        }
        TyKind::Range { bound, .. } => {
            if let Some(bound) = bound {
                add_reachable_module(bound.interner_id, modules, pending_modules);
            }
        }
        TyKind::FunctionPointer {
            params,
            return_type,
            ..
        } => {
            for param in params {
                add_reachable_module(param.interner_id, modules, pending_modules);
            }
            add_reachable_module(return_type.interner_id, modules, pending_modules);
        }
        TyKind::ErrorUnion { error, value } => {
            add_reachable_module(error.interner_id, modules, pending_modules);
            add_reachable_module(value.interner_id, modules, pending_modules);
        }
        TyKind::TraitObject {
            trait_id,
            trait_args,
            associated_type_bindings,
            ..
        }
        | TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            associated_type_bindings,
        } => {
            collect_trait_id_owner_module(*trait_id, modules, pending_modules, traits);
            collect_ty_ids_owner_modules(trait_args, modules, pending_modules);
            collect_associated_binding_owner_modules(
                associated_type_bindings,
                modules,
                pending_modules,
                traits,
            );
        }
        TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            ..
        } => {
            add_reachable_module(self_ty.interner_id, modules, pending_modules);
            collect_trait_id_owner_module(*trait_id, modules, pending_modules, traits);
            collect_ty_ids_owner_modules(trait_args, modules, pending_modules);
        }
        TyKind::BuiltinTrait { args, .. } => {
            collect_ty_ids_owner_modules(args, modules, pending_modules)
        }
        TyKind::Error
        | TyKind::ComptimeOnly
        | TyKind::Primitive(_)
        | TyKind::Vector { .. }
        | TyKind::GenericParam(_) => {}
    }
}

fn collect_ty_ids_owner_modules(
    tys: &[InternedTyId],
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    for ty in tys {
        add_reachable_module(ty.interner_id, modules, pending_modules);
    }
}

fn collect_trait_id_owner_module(
    trait_id: TraitId,
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut HashSet<TraitId>,
) {
    traits.insert(trait_id);
    if let TraitId::Source(def_id) = trait_id {
        add_reachable_module(def_id.module_id, modules, pending_modules);
    }
}

fn collect_associated_binding_owner_modules(
    bindings: &[AssociatedTypeBindingTy],
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut HashSet<TraitId>,
) {
    for binding in bindings {
        if let Some(trait_id) = binding.trait_id {
            collect_trait_id_owner_module(trait_id, modules, pending_modules, traits);
        }
        collect_ty_ids_owner_modules(&binding.trait_args, modules, pending_modules);
        add_reachable_module(binding.ty.interner_id, modules, pending_modules);
    }
}

fn backend_timing_mode(timings: TimingMode) -> nia_backend_lower::BackendTimingMode {
    if timings.detail() {
        nia_backend_lower::BackendTimingMode::Detail
    } else {
        nia_backend_lower::BackendTimingMode::Off
    }
}

pub(super) fn provide_program_diagnostics(db: &QueryDb<DriverContext>) -> Vec<ProgramDiagnostic> {
    time_provider(db.context().timings, "program_diagnostics", || {
        provide_program_diagnostics_inner(db)
    })
}

fn provide_program_diagnostics_inner(db: &QueryDb<DriverContext>) -> Vec<ProgramDiagnostic> {
    let mut diagnostics = db.context().loaded.diagnostics.clone();
    for loaded_module in &db.context().loaded.modules {
        for error in &loaded_module.parse_errors {
            diagnostics.push(ProgramDiagnostic {
                path: loaded_module.path.clone(),
                diagnostic: Diagnostic::user_error_at("E0201", error.span, error.message.clone()),
            });
        }
    }
    let public = db.query(PublicSurfaceQuery);
    for (module_id, diagnostic) in public.diagnostics {
        diagnostics.push(ProgramDiagnostic {
            path: db.context().path_for_module(module_id),
            diagnostic,
        });
    }
    let first_path = db
        .query(ParseOkModuleIdsQuery)
        .first()
        .map(|module_id| db.context().path_for_module(*module_id))
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
