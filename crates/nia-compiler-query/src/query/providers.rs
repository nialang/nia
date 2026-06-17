// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::RuntimeModel;
use nia_body_ir::{
    PlaceBase, PlaceElem, TypedArrayElements, TypedAtomic, TypedBody, TypedCallee, TypedExpr,
    TypedExprKind, TypedInlineAsm, TypedMemoryIntrinsicSource, TypedPattern, TypedPatternKind,
    TypedPlace, TypedStmt, TypedStmtKind, TypedSwitchArmBody,
};
use nia_defs::DefKind;
use nia_ids::{InternedTyId, TraitId};
use nia_sema_ir::{FunctionSemanticFacts, SemanticFacts};
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
        modules: checked_modules_for_codegen(db),
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
        body_check_with_filter(db, module_id, nia_body_check::BodyCheckFilter::All)
    })
}

fn body_check_with_filter(
    db: &QueryDb<DriverContext>,
    module_id: ModuleId,
    filter: nia_body_check::BodyCheckFilter<'_>,
) -> nia_body_check::BodyCheck {
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
            filter,
        },
        body_timing_mode(db.context().timings),
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
    db: &QueryDb<DriverContext>,
    module_id: ModuleId,
) -> CheckedModule {
    time_module_provider(db, "checked_module", module_id, || {
        checked_module_with_body_check(db, module_id, db.query(BodyCheckQuery(module_id)))
    })
}

fn checked_module_with_body_check(
    db: &QueryDb<DriverContext>,
    module_id: ModuleId,
    body_check: nia_body_check::BodyCheck,
) -> CheckedModule {
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
        body_ir: body_check.ir,
        semantic_uses: db.query(SemanticUseTableQuery(module_id)),
        semantic_facts: body_check.facts,
        body_diagnostics: body_check.diagnostics,
    }
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

pub(super) fn provide_executable_checked_modules(
    db: &QueryDb<DriverContext>,
) -> Vec<CheckedModule> {
    time_provider(db.context().timings, "executable_checked_modules", || {
        if db.context().loaded.runtime != RuntimeModel::FreestandingExecutable {
            return db.query(CheckedModulesQuery);
        }
        time_provider(
            db.context().timings,
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
    })
}

fn executable_checked_modules_inner(db: &QueryDb<DriverContext>) -> Vec<CheckedModule> {
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

    let mut reachable_functions = executable_root_functions(&graph, &defs_by_id);
    let mut reachable_modules = reachable_functions
        .iter()
        .map(|def_id| def_id.module_id)
        .collect::<HashSet<_>>();
    add_reachable_module(graph.root(), &mut reachable_modules, &mut VecDeque::new());

    let parse_ok_set = parse_ok.iter().copied().collect::<HashSet<_>>();
    loop {
        let before = (reachable_functions.len(), reachable_modules.len());
        let mut reachable_traits = HashSet::new();
        let current_reachable_modules = reachable_modules.clone();
        for checked in checked_by_id
            .values()
            .filter(|module| current_reachable_modules.contains(&module.id))
        {
            let mut pending_modules = VecDeque::new();
            extend_reachable_functions_from_bodies(
                checked,
                &program_signatures,
                &mut reachable_functions,
                &mut reachable_modules,
                &mut pending_modules,
            );
            collect_reachable_body_trait_ids(checked, &reachable_functions, &mut reachable_traits);
            collect_reachable_fact_owner_modules(
                checked,
                &reachable_functions,
                &mut reachable_modules,
                &mut pending_modules,
                &mut reachable_traits,
            );
        }
        let mut pending_modules = VecDeque::new();
        extend_reachable_functions_from_traits(
            &program_signatures,
            &extension_methods.methods,
            &reachable_traits,
            &reachable_modules,
            &mut reachable_functions,
            &mut pending_modules,
        );
        while let Some(module_id) = pending_modules.pop_front() {
            if !parse_ok_set.contains(&module_id) {
                continue;
            }
            reachable_modules.insert(module_id);
        }
        if before == (reachable_functions.len(), reachable_modules.len()) {
            break;
        }
    }

    if db.context().timings.detail() {
        let checked_module_count = checked_by_id.len();
        let checked_body_count = checked_by_id
            .values()
            .map(|module| module.body_ir.function_bodies.len())
            .sum::<usize>();
        let reachable_body_count = checked_by_id
            .values()
            .map(|module| {
                module
                    .body_ir
                    .function_bodies
                    .keys()
                    .filter(|def_id| reachable_functions.contains(def_id))
                    .count()
            })
            .sum::<usize>();
        eprintln!(
            "query timing executable_checked_modules.reachable: modules={} functions={} bodies={} full_bodies={}",
            reachable_modules.len(),
            reachable_functions.len(),
            reachable_body_count,
            checked_body_count
        );
        eprintln!(
            "query timing executable_checked_modules.checked: modules={checked_module_count}"
        );
    }

    parse_ok
        .into_iter()
        .filter(|module_id| reachable_modules.contains(module_id))
        .filter_map(|module_id| checked_by_id.get(&module_id))
        .cloned()
        .map(|module| filter_checked_module_for_codegen(module, &reachable_functions))
        .collect()
}

fn executable_root_functions(
    graph: &ModuleGraph,
    defs_by_id: &HashMap<ModuleId, DefCollection>,
) -> HashSet<GlobalDefId> {
    let mut roots = HashSet::new();
    if let Some(main) = named_function(defs_by_id, graph.root(), "main") {
        roots.insert(main);
    }
    if let Some(start_module) = freestanding_start_module(graph)
        && let Some(start) = named_function(defs_by_id, start_module, "_start")
    {
        roots.insert(start);
        roots.extend(module_functions(defs_by_id, start_module));
    }
    roots
}

fn module_functions(
    defs_by_id: &HashMap<ModuleId, DefCollection>,
    module_id: ModuleId,
) -> impl Iterator<Item = GlobalDefId> + '_ {
    defs_by_id
        .get(&module_id)
        .into_iter()
        .flat_map(move |defs| {
            defs.defs.iter().filter_map(move |(def_id, def)| {
                (def.kind == DefKind::Function).then_some(GlobalDefId { module_id, def_id })
            })
        })
}

fn named_function(
    defs_by_id: &HashMap<ModuleId, DefCollection>,
    module_id: ModuleId,
    name: &str,
) -> Option<GlobalDefId> {
    defs_by_id.get(&module_id).and_then(|defs| {
        defs.defs.iter().find_map(|(def_id, def)| {
            (def.kind == DefKind::Function && def.name == name)
                .then_some(GlobalDefId { module_id, def_id })
        })
    })
}

fn extend_reachable_functions_from_bodies(
    module: &CheckedModule,
    program_signatures: &ProgramSignatures,
    reachable_functions: &mut HashSet<GlobalDefId>,
    reachable_modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    for def_id in typed_body_callees(module, reachable_functions) {
        add_reachable_function(
            def_id,
            program_signatures,
            reachable_functions,
            reachable_modules,
            pending_modules,
        );
    }
}

fn typed_body_callees(
    module: &CheckedModule,
    reachable_functions: &HashSet<GlobalDefId>,
) -> Vec<GlobalDefId> {
    let mut refs = TypedBodyRefs::default();
    for (def_id, body) in &module.body_ir.function_bodies {
        if reachable_functions.contains(def_id) {
            collect_typed_body_refs(body, &mut refs);
        }
    }
    refs.functions.into_iter().collect()
}

fn collect_reachable_body_trait_ids(
    module: &CheckedModule,
    reachable_functions: &HashSet<GlobalDefId>,
    traits: &mut HashSet<TraitId>,
) {
    let mut refs = TypedBodyRefs::default();
    for (def_id, body) in &module.body_ir.function_bodies {
        if reachable_functions.contains(def_id) {
            collect_typed_body_refs(body, &mut refs);
        }
    }
    traits.extend(refs.traits);
}

#[derive(Default)]
struct TypedBodyRefs {
    functions: HashSet<GlobalDefId>,
    traits: HashSet<TraitId>,
}

fn collect_typed_body_refs(body: &TypedBody, refs: &mut TypedBodyRefs) {
    for stmt in &body.stmts {
        collect_typed_stmt_refs(stmt, refs);
    }
    if let Some(tail) = body.tail.as_deref() {
        collect_typed_expr_refs(tail, refs);
    }
}

fn collect_typed_stmt_refs(stmt: &TypedStmt, refs: &mut TypedBodyRefs) {
    match &stmt.kind {
        TypedStmtKind::Binding(binding) => {
            if let Some(value) = &binding.value {
                collect_typed_expr_refs(value, refs);
            }
        }
        TypedStmtKind::Expr(expr) | TypedStmtKind::Defer(expr) => {
            collect_typed_expr_refs(expr, refs);
        }
        TypedStmtKind::Return(value) => {
            if let Some(value) = value {
                collect_typed_expr_refs(value, refs);
            }
        }
        TypedStmtKind::ForIn(for_in) => {
            refs.traits
                .insert(TraitId::Builtin(nia_ty::BuiltinTrait::Iterator));
            collect_typed_expr_refs(&for_in.iter, refs);
            collect_typed_body_refs(&for_in.body, refs);
        }
        TypedStmtKind::While(while_loop) => {
            collect_typed_expr_refs(&while_loop.cond, refs);
            collect_typed_body_refs(&while_loop.body, refs);
        }
        TypedStmtKind::Loop(loop_body) => collect_typed_body_refs(&loop_body.body, refs),
        TypedStmtKind::Break | TypedStmtKind::Continue => {}
    }
}

fn collect_typed_expr_refs(expr: &TypedExpr, refs: &mut TypedBodyRefs) {
    match &expr.kind {
        TypedExprKind::Function(def_id)
        | TypedExprKind::FunctionInstance { def_id, .. }
        | TypedExprKind::Field { field: def_id, .. } => {
            refs.functions.insert(*def_id);
        }
        TypedExprKind::Range(range) => {
            if let Some(start) = range.start.as_deref() {
                collect_typed_expr_refs(start, refs);
            }
            if let Some(end) = range.end.as_deref() {
                collect_typed_expr_refs(end, refs);
            }
        }
        TypedExprKind::InlineAsm(asm) => collect_typed_inline_asm_refs(asm, refs),
        TypedExprKind::MemoryIntrinsic(memory) => {
            collect_typed_expr_refs(&memory.dest, refs);
            match &memory.source {
                TypedMemoryIntrinsicSource::Slice(source)
                | TypedMemoryIntrinsicSource::Byte(source) => collect_typed_expr_refs(source, refs),
            }
        }
        TypedExprKind::Atomic(atomic) => collect_typed_atomic_refs(atomic, refs),
        TypedExprKind::LoadUnaligned { ptr, .. } => collect_typed_expr_refs(ptr, refs),
        TypedExprKind::Splat { value } | TypedExprKind::Bitmask { vector: value } => {
            collect_typed_expr_refs(value, refs);
        }
        TypedExprKind::ExtractElement { vector, index } => {
            collect_typed_expr_refs(vector, refs);
            collect_typed_expr_refs(index, refs);
        }
        TypedExprKind::InsertElement {
            vector,
            index,
            value,
        } => {
            collect_typed_expr_refs(vector, refs);
            collect_typed_expr_refs(index, refs);
            collect_typed_expr_refs(value, refs);
        }
        TypedExprKind::BitIntrinsic { value, .. }
        | TypedExprKind::StaticArrayPointer { array: value, .. }
        | TypedExprKind::Unary { expr: value, .. }
        | TypedExprKind::OptionalSome { expr: value }
        | TypedExprKind::ErrorOk { expr: value }
        | TypedExprKind::ErrorErr { expr: value }
        | TypedExprKind::Try { expr: value }
        | TypedExprKind::Discard(value)
        | TypedExprKind::Cast { expr: value, .. }
        | TypedExprKind::TraitObjectUpcast { expr: value, .. }
        | TypedExprKind::TraitObjectCoercion { expr: value, .. } => {
            collect_typed_expr_refs(value, refs);
        }
        TypedExprKind::ArrayLiteral { elems } => match elems {
            TypedArrayElements::List(elems) => {
                for elem in elems {
                    collect_typed_expr_refs(elem, refs);
                }
            }
            TypedArrayElements::Repeat { value, .. } => collect_typed_expr_refs(value, refs),
        },
        TypedExprKind::StructLiteral { fields, .. } => {
            for field in fields {
                collect_typed_expr_refs(&field.value, refs);
            }
        }
        TypedExprKind::UnionLiteral { field, .. } => {
            collect_typed_expr_refs(&field.value, refs);
        }
        TypedExprKind::Binary { lhs, rhs, .. } => {
            collect_typed_expr_refs(lhs, refs);
            collect_typed_expr_refs(rhs, refs);
        }
        TypedExprKind::Assign { place, rhs, .. } => {
            collect_typed_place_refs(place, refs);
            collect_typed_expr_refs(rhs, refs);
        }
        TypedExprKind::Call { callee, args } => {
            collect_typed_callee_refs(callee, refs);
            for arg in args {
                collect_typed_expr_refs(arg, refs);
            }
        }
        TypedExprKind::Index { lhs, index } => {
            collect_typed_expr_refs(lhs, refs);
            collect_typed_expr_refs(index, refs);
        }
        TypedExprKind::Slice { lhs, range, .. } => {
            collect_typed_expr_refs(lhs, refs);
            if let Some(start) = range.start.as_deref() {
                collect_typed_expr_refs(start, refs);
            }
            if let Some(end) = range.end.as_deref() {
                collect_typed_expr_refs(end, refs);
            }
        }
        TypedExprKind::Block(body) => collect_typed_body_refs(body, refs),
        TypedExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_typed_expr_refs(cond, refs);
            collect_typed_body_refs(then_branch, refs);
            if let Some(else_branch) = else_branch.as_deref() {
                collect_typed_expr_refs(else_branch, refs);
            }
        }
        TypedExprKind::Switch(switch) => {
            collect_typed_expr_refs(&switch.target, refs);
            for arm in &switch.arms {
                for pattern in &arm.patterns {
                    collect_typed_switch_pattern_refs(pattern, refs);
                }
                match &arm.body {
                    TypedSwitchArmBody::Expr(expr) => collect_typed_expr_refs(expr, refs),
                    TypedSwitchArmBody::Stmt(stmt) => collect_typed_stmt_refs(stmt, refs),
                    TypedSwitchArmBody::Block(body) => collect_typed_body_refs(body, refs),
                }
            }
        }
        TypedExprKind::IfPattern(if_pattern) => {
            collect_typed_expr_refs(&if_pattern.target, refs);
            for arm in &if_pattern.arms {
                collect_typed_pattern_refs(&arm.pattern, refs);
                collect_typed_body_refs(&arm.body, refs);
            }
            if let Some(else_branch) = if_pattern.else_branch.as_deref() {
                collect_typed_expr_refs(else_branch, refs);
            }
        }
        TypedExprKind::Error
        | TypedExprKind::Integer(_)
        | TypedExprKind::Float(_)
        | TypedExprKind::String(_)
        | TypedExprKind::ByteString(_)
        | TypedExprKind::Char(_)
        | TypedExprKind::ByteChar(_)
        | TypedExprKind::Bool(_)
        | TypedExprKind::Null
        | TypedExprKind::Local(_)
        | TypedExprKind::Global(_)
        | TypedExprKind::EnumVariant(_)
        | TypedExprKind::BuiltinValue(_)
        | TypedExprKind::Trap => {}
    }
}

fn collect_typed_callee_refs(callee: &TypedCallee, refs: &mut TypedBodyRefs) {
    match callee {
        TypedCallee::Function(def_id) | TypedCallee::FunctionInstance { def_id, .. } => {
            refs.functions.insert(*def_id);
        }
        TypedCallee::Method {
            def_id, receiver, ..
        } => {
            refs.functions.insert(*def_id);
            collect_typed_expr_refs(receiver, refs);
        }
        TypedCallee::TraitMethod {
            trait_id,
            method_id,
            receiver,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.traits.insert(TraitId::Source(*trait_id));
            collect_typed_expr_refs(receiver, refs);
        }
        TypedCallee::TraitAssociatedFunction {
            trait_id,
            method_id,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.traits.insert(TraitId::Source(*trait_id));
        }
        TypedCallee::DynamicTraitMethod {
            trait_id,
            method_id,
            receiver,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.traits.insert(*trait_id);
            collect_typed_expr_refs(receiver, refs);
        }
        TypedCallee::BuiltinMethod { receiver, .. } | TypedCallee::FunctionPointer(receiver) => {
            collect_typed_expr_refs(receiver, refs);
        }
        TypedCallee::BuiltinOperator(operator) => {
            refs.traits.insert(TraitId::Builtin(operator.trait_id));
        }
        TypedCallee::BuiltinPlaceMethod(method) => {
            refs.traits.insert(TraitId::Builtin(method.trait_id));
            collect_typed_expr_refs(&method.receiver, refs);
        }
    }
}

fn collect_typed_pattern_refs(pattern: &TypedPattern, refs: &mut TypedBodyRefs) {
    match &pattern.kind {
        TypedPatternKind::OptionalSome(pattern)
        | TypedPatternKind::ErrorOk(pattern)
        | TypedPatternKind::ErrorErr(pattern) => collect_typed_pattern_refs(pattern, refs),
        TypedPatternKind::Expr(expr) => collect_typed_expr_refs(expr, refs),
        TypedPatternKind::Range { start, end, .. } => {
            collect_typed_expr_refs(start, refs);
            collect_typed_expr_refs(end, refs);
        }
        TypedPatternKind::Wildcard
        | TypedPatternKind::Bind { .. }
        | TypedPatternKind::OptionalNull => {}
    }
}

fn collect_typed_switch_pattern_refs(
    pattern: &nia_body_ir::TypedSwitchPattern,
    refs: &mut TypedBodyRefs,
) {
    match &pattern.kind {
        nia_body_ir::TypedSwitchPatternKind::Expr(expr) => collect_typed_expr_refs(expr, refs),
        nia_body_ir::TypedSwitchPatternKind::Range { start, end, .. } => {
            collect_typed_expr_refs(start, refs);
            collect_typed_expr_refs(end, refs);
        }
        nia_body_ir::TypedSwitchPatternKind::Wildcard
        | nia_body_ir::TypedSwitchPatternKind::CheckedInt { .. }
        | nia_body_ir::TypedSwitchPatternKind::CheckedIntRange { .. } => {}
    }
}

fn collect_typed_atomic_refs(atomic: &TypedAtomic, refs: &mut TypedBodyRefs) {
    match atomic {
        TypedAtomic::Load { ptr, .. } => collect_typed_expr_refs(ptr, refs),
        TypedAtomic::Store { ptr, value, .. } | TypedAtomic::Rmw { ptr, value, .. } => {
            collect_typed_expr_refs(ptr, refs);
            collect_typed_expr_refs(value, refs);
        }
        TypedAtomic::Cmpxchg {
            ptr,
            expected,
            desired,
            ..
        } => {
            collect_typed_expr_refs(ptr, refs);
            collect_typed_expr_refs(expected, refs);
            collect_typed_expr_refs(desired, refs);
        }
        TypedAtomic::Fence { .. } => {}
    }
}

fn collect_typed_inline_asm_refs(asm: &TypedInlineAsm, refs: &mut TypedBodyRefs) {
    for input in &asm.inputs {
        collect_typed_expr_refs(&input.value, refs);
    }
    for output in &asm.outputs {
        collect_typed_place_refs(&output.place, refs);
    }
}

fn collect_typed_place_refs(place: &TypedPlace, refs: &mut TypedBodyRefs) {
    match &place.base {
        PlaceBase::Deref(expr) => collect_typed_expr_refs(expr, refs),
        PlaceBase::Local(_) | PlaceBase::Global(_) | PlaceBase::Error => {}
    }
    for elem in &place.elems {
        match elem {
            PlaceElem::Index(expr) => collect_typed_expr_refs(expr, refs),
            PlaceElem::Field(_) | PlaceElem::Error => {}
        }
    }
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
        semantic_facts_for_reachable_functions(module.semantic_facts, reachable_functions);
    module
}

fn semantic_facts_for_reachable_functions(
    facts: SemanticFacts,
    reachable_functions: &HashSet<GlobalDefId>,
) -> SemanticFacts {
    let mut reachable_facts = SemanticFacts::default();
    reachable_facts.global_types = facts.global_types;
    for def_id in reachable_functions {
        let Some(function_facts) = facts.function_facts.get(def_id) else {
            continue;
        };
        reachable_facts
            .local_types
            .extend(function_facts.local_types.clone());
        reachable_facts
            .generic_instantiations
            .extend(function_facts.generic_instantiations.clone());
        reachable_facts
            .node_expr_types
            .extend(function_facts.node_expr_types.clone());
        reachable_facts
            .node_bracket_suffix_resolutions
            .extend(function_facts.node_bracket_suffix_resolutions.clone());
        reachable_facts
            .node_array_to_slice_coercions
            .extend(function_facts.node_array_to_slice_coercions.clone());
        reachable_facts
            .node_pointer_array_to_slice_coercions
            .extend(function_facts.node_pointer_array_to_slice_coercions.clone());
        reachable_facts
            .node_trait_object_coercions
            .extend(function_facts.node_trait_object_coercions.clone());
        reachable_facts
            .node_trait_object_upcasts
            .extend(function_facts.node_trait_object_upcasts.clone());
        reachable_facts
            .node_comptime_if_selections
            .extend(function_facts.node_comptime_if_selections.clone());
        reachable_facts
            .node_builtin_values
            .extend(function_facts.node_builtin_values.clone());
        reachable_facts
            .node_array_repeat_counts
            .extend(function_facts.node_array_repeat_counts.clone());
        reachable_facts
            .node_switch_pattern_values
            .extend(function_facts.node_switch_pattern_values.clone());
        reachable_facts
            .node_resolved_calls
            .extend(function_facts.node_resolved_calls.clone());
        reachable_facts
            .node_function_references
            .extend(function_facts.node_function_references.clone());
    }
    reachable_facts.generic_instantiations.extend(
        facts
            .generic_instantiations
            .into_iter()
            .filter(|instantiation| instantiation.source_def_id.is_none()),
    );
    reachable_facts.node_builtin_associated_values = facts.node_builtin_associated_values;
    reachable_facts.function_facts = facts
        .function_facts
        .into_iter()
        .filter(|(def_id, _)| reachable_functions.contains(def_id))
        .collect();
    reachable_facts
}

fn extend_reachable_functions_from_traits(
    program_signatures: &ProgramSignatures,
    extension_methods: &nia_defs::ExtensionMethods,
    reachable_traits: &HashSet<TraitId>,
    reachable_modules: &HashSet<ModuleId>,
    reachable_functions: &mut HashSet<GlobalDefId>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    let mut modules = reachable_modules.clone();
    for trait_id in reachable_traits {
        let TraitId::Source(trait_def) = trait_id else {
            continue;
        };
        if !reachable_modules.contains(&trait_def.module_id) {
            continue;
        }
        let Some(trait_signature) = program_signatures.traits.get(trait_def) else {
            continue;
        };
        for method in &trait_signature.signature.methods {
            if method.has_default {
                add_reachable_function(
                    GlobalDefId {
                        module_id: trait_def.module_id,
                        def_id: method.def_id,
                    },
                    program_signatures,
                    reachable_functions,
                    &mut modules,
                    pending_modules,
                );
            }
        }
    }
    for method in extension_methods.all_methods() {
        if method
            .trait_id
            .is_some_and(|trait_id| reachable_traits.contains(&trait_id))
        {
            add_reachable_function(
                method.def_id,
                program_signatures,
                reachable_functions,
                &mut modules,
                pending_modules,
            );
        }
    }
}

fn add_reachable_function(
    def_id: GlobalDefId,
    program_signatures: &ProgramSignatures,
    reachable_functions: &mut HashSet<GlobalDefId>,
    reachable_modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    let Some(signature) = program_signatures.functions.get(&def_id) else {
        add_reachable_module(def_id.module_id, reachable_modules, pending_modules);
        return;
    };
    if signature.signature.is_comptime || !signature.signature.has_body {
        add_reachable_module(def_id.module_id, reachable_modules, pending_modules);
        return;
    }
    if reachable_functions.insert(def_id) {
        add_reachable_module(def_id.module_id, reachable_modules, pending_modules);
    }
}

pub(super) fn provide_monomorphization(
    db: &QueryDb<DriverContext>,
) -> nia_monomorphize::Monomorphization {
    time_provider(db.context().timings, "monomorphization", || {
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

fn checked_modules_for_codegen(db: &QueryDb<DriverContext>) -> Vec<CheckedModule> {
    if db.context().loaded.runtime == RuntimeModel::FreestandingExecutable {
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
    db: &QueryDb<DriverContext>,
) -> nia_backend_lower::BackendLowering {
    time_provider(db.context().timings, "backend_lowering", || {
        provide_backend_lowering_inner(db)
    })
}

fn provide_backend_lowering_inner(
    db: &QueryDb<DriverContext>,
) -> nia_backend_lower::BackendLowering {
    let all_checked_modules = checked_modules_for_codegen(db);
    let monomorphization = db.query(MonomorphizationQuery);
    let checked_modules = all_checked_modules;
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
                            roots: backend_function_roots(),
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

fn backend_function_roots() -> nia_backend_lower::BackendFunctionRoots {
    nia_backend_lower::BackendFunctionRoots::FunctionBodies
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

fn collect_reachable_fact_owner_modules(
    checked: &CheckedModule,
    reachable_functions: &HashSet<GlobalDefId>,
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut HashSet<TraitId>,
) {
    let mut type_ids = Vec::new();
    for def_id in reachable_functions
        .iter()
        .filter(|def_id| def_id.module_id == checked.id)
    {
        let Some(function_facts) = checked.semantic_facts.function_facts.get(def_id) else {
            continue;
        };
        collect_function_fact_owner_modules(
            function_facts,
            modules,
            pending_modules,
            traits,
            &mut type_ids,
        );
    }
    collect_module_signature_owner_type_ids(&checked.item_signatures, &mut type_ids);
    collect_ty_ids_owner_modules(
        type_ids,
        &checked.body_ir.interner,
        &checked.type_lowering.interner,
        &checked.type_normalization.interner,
        modules,
        pending_modules,
        traits,
    );
}

fn collect_module_signature_owner_type_ids(
    signatures: &ItemSignatures,
    type_ids: &mut Vec<InternedTyId>,
) {
    for signature in signatures.structs.values() {
        type_ids.extend(signature.fields.iter().map(|field| field.ty));
        collect_where_predicate_type_ids(&signature.where_predicates, type_ids);
    }
    for signature in signatures.unions.values() {
        type_ids.extend(signature.fields.iter().map(|field| field.ty));
        collect_where_predicate_type_ids(&signature.where_predicates, type_ids);
    }
    for signature in signatures.type_aliases.values() {
        type_ids.push(signature.target);
    }
    for signature in signatures.enums.values() {
        type_ids.push(signature.backing_type);
    }
    for signature in &signatures.trait_impls {
        type_ids.push(signature.target_ty);
        if let Some(trait_ty) = signature.trait_ty {
            type_ids.push(trait_ty);
        }
        collect_where_predicate_type_ids(&signature.where_predicates, type_ids);
        type_ids.extend(
            signature
                .associated_types
                .iter()
                .map(|associated| associated.ty),
        );
    }
    for signature in signatures.globals.values() {
        if let Some(ty) = signature.explicit_type {
            type_ids.push(ty);
        }
    }
}

fn collect_where_predicate_type_ids(
    predicates: &[nia_defs::WherePredicateSignature],
    type_ids: &mut Vec<InternedTyId>,
) {
    for predicate in predicates {
        type_ids.push(predicate.ty);
        for bound in &predicate.bounds {
            type_ids.push(bound.trait_ty);
            type_ids.extend(
                bound
                    .associated_type_bindings
                    .iter()
                    .map(|binding| binding.ty),
            );
        }
    }
}

fn collect_function_fact_owner_modules(
    facts: &FunctionSemanticFacts,
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut HashSet<TraitId>,
    type_ids: &mut Vec<InternedTyId>,
) {
    type_ids.extend(facts.local_types.values().copied());
    type_ids.extend(facts.node_expr_types.values().copied());
    for instantiation in &facts.generic_instantiations {
        add_reachable_module(instantiation.def_id.module_id, modules, pending_modules);
        type_ids.extend(instantiation.args.iter().copied());
    }
    for coercion in facts.node_array_to_slice_coercions.values() {
        type_ids.extend([coercion.array_ty, coercion.slice_ty]);
    }
    for coercion in facts.node_pointer_array_to_slice_coercions.values() {
        type_ids.extend([coercion.pointer_ty, coercion.array_ty, coercion.slice_ty]);
    }
    for coercion in facts.node_trait_object_coercions.values() {
        type_ids.extend([coercion.source_ty, coercion.target_ty]);
    }
    for upcast in facts.node_trait_object_upcasts.values() {
        type_ids.extend([upcast.source_ty, upcast.target_ty]);
    }
    for value in facts.node_builtin_values.values() {
        if let nia_sema_ir::BuiltinValue::Layout { ty, .. } = value {
            type_ids.push(*ty);
        }
    }
    for call in facts.node_resolved_calls.values() {
        collect_resolved_call_owner_modules(call, modules, pending_modules, traits, type_ids);
    }
    for reference in facts.node_function_references.values() {
        add_reachable_module(reference.def_id.module_id, modules, pending_modules);
        add_reachable_module(reference.arg_module_id, modules, pending_modules);
        type_ids.extend(reference.args.iter().copied());
    }
}

fn collect_resolved_call_owner_modules(
    call: &nia_sema_ir::ResolvedCall,
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut HashSet<TraitId>,
    type_ids: &mut Vec<InternedTyId>,
) {
    match call {
        nia_sema_ir::ResolvedCall::Function(def_id) => {
            add_reachable_module(def_id.module_id, modules, pending_modules);
        }
        nia_sema_ir::ResolvedCall::FunctionInstance {
            def_id,
            arg_module_id,
            args,
        } => {
            add_reachable_module(def_id.module_id, modules, pending_modules);
            add_reachable_module(*arg_module_id, modules, pending_modules);
            type_ids.extend(args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::Method { def_id, args, .. } => {
            add_reachable_module(def_id.module_id, modules, pending_modules);
            type_ids.extend(args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::TraitMethod {
            trait_id,
            method_id,
            self_ty,
            trait_args,
            args,
            ..
        } => {
            add_reachable_module(method_id.module_id, modules, pending_modules);
            collect_trait_id_owner_module(
                TraitId::Source(*trait_id),
                modules,
                pending_modules,
                traits,
            );
            type_ids.push(*self_ty);
            type_ids.extend(trait_args.iter().copied());
            type_ids.extend(args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::TraitAssociatedFunction {
            trait_id,
            method_id,
            self_ty,
            trait_args,
            args,
            ..
        } => {
            add_reachable_module(method_id.module_id, modules, pending_modules);
            collect_trait_id_owner_module(
                TraitId::Source(*trait_id),
                modules,
                pending_modules,
                traits,
            );
            type_ids.push(*self_ty);
            type_ids.extend(trait_args.iter().copied());
            type_ids.extend(args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::DynamicTraitMethod {
            object_ty,
            trait_id,
            method_id,
            trait_args,
            params,
            return_type,
            ..
        } => {
            add_reachable_module(method_id.module_id, modules, pending_modules);
            collect_trait_id_owner_module(*trait_id, modules, pending_modules, traits);
            type_ids.push(*object_ty);
            type_ids.extend(trait_args.iter().copied());
            type_ids.extend(params.iter().copied());
            type_ids.push(*return_type);
        }
        nia_sema_ir::ResolvedCall::BuiltinTraitMethod { trait_id, .. } => {
            traits.insert(TraitId::Builtin(*trait_id));
        }
        nia_sema_ir::ResolvedCall::BuiltinMethod { self_ty, .. } => {
            type_ids.push(*self_ty);
        }
        nia_sema_ir::ResolvedCall::BuiltinPlaceMethod {
            trait_id,
            self_ty,
            trait_args,
            ..
        } => {
            traits.insert(TraitId::Builtin(*trait_id));
            type_ids.push(*self_ty);
            type_ids.extend(trait_args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::FunctionPointer => {}
    }
}

fn collect_ty_ids_owner_modules(
    tys: impl IntoIterator<Item = InternedTyId>,
    body_interner: &TyInterner,
    type_lowering_interner: &TyInterner,
    normalization_interner: &TyInterner,
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut HashSet<TraitId>,
) {
    let mut pending = tys.into_iter().collect::<VecDeque<_>>();
    let mut seen = HashSet::new();
    while let Some(ty_id) = pending.pop_front() {
        add_reachable_module(ty_id.interner_id, modules, pending_modules);
        if !seen.insert(ty_id) {
            continue;
        }
        let Some(ty) = body_interner
            .get(ty_id)
            .or_else(|| type_lowering_interner.get(ty_id))
            .or_else(|| normalization_interner.get(ty_id))
        else {
            continue;
        };
        collect_ty_owner_modules(ty, &mut pending, modules, pending_modules, traits);
    }
}

fn collect_ty_owner_modules(
    ty: &TyKind,
    type_ids: &mut VecDeque<InternedTyId>,
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut HashSet<TraitId>,
) {
    match ty {
        TyKind::Nominal { def_id, args } => {
            add_reachable_module(def_id.module_id, modules, pending_modules);
            type_ids.extend(args.iter().copied());
        }
        TyKind::Pointer { elem, .. }
        | TyKind::Slice { elem, .. }
        | TyKind::SlicePointee { elem }
        | TyKind::Optional { elem } => {
            type_ids.push_back(*elem);
        }
        TyKind::Array { len, elem } => {
            type_ids.push_back(*elem);
            collect_array_len_owner_modules(len, type_ids);
        }
        TyKind::Range { bound, .. } => {
            if let Some(bound) = bound {
                type_ids.push_back(*bound);
            }
        }
        TyKind::FunctionPointer {
            params,
            return_type,
            ..
        } => {
            type_ids.extend(params.iter().copied());
            type_ids.push_back(*return_type);
        }
        TyKind::ErrorUnion { error, value } => {
            type_ids.push_back(*error);
            type_ids.push_back(*value);
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
            type_ids.extend(trait_args.iter().copied());
            collect_associated_binding_owner_modules(
                associated_type_bindings,
                type_ids,
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
            type_ids.push_back(*self_ty);
            collect_trait_id_owner_module(*trait_id, modules, pending_modules, traits);
            type_ids.extend(trait_args.iter().copied());
        }
        TyKind::BuiltinTrait { args, .. } => type_ids.extend(args.iter().copied()),
        TyKind::Error
        | TyKind::ComptimeOnly
        | TyKind::Primitive(_)
        | TyKind::Vector { .. }
        | TyKind::GenericParam(_) => {}
    }
}

fn collect_array_len_owner_modules(
    len: &nia_ty::ArrayLenTy,
    type_ids: &mut VecDeque<InternedTyId>,
) {
    if let nia_ty::ArrayLenTy::Builtin { ty, .. } = len {
        type_ids.push_back(*ty);
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
    type_ids: &mut VecDeque<InternedTyId>,
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut HashSet<TraitId>,
) {
    for binding in bindings {
        if let Some(trait_id) = binding.trait_id {
            collect_trait_id_owner_module(trait_id, modules, pending_modules, traits);
        }
        type_ids.extend(binding.trait_args.iter().copied());
        type_ids.push_back(binding.ty);
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
