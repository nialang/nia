// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_defs::{DefId, DefKind};
use nia_executable_reachability::{
    ExecutableExtensionIndex, ExecutableRootDefs, IncrementalExecutableReachability,
    compute_executable_reachability_incremental_with_timings,
    extend_incremental_executable_reachability_from_checked_module_with_timings,
    filter_semantic_facts_for_reachable_items,
};
use nia_program_signatures::{
    ProgramSignatureContext, ProgramSignatureMaps, ProgramSignatureResolvers,
    ProgramTraitMethodIndex,
};
use nia_symbol::{SymbolId, SymbolMap};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

mod extension_providers;
mod program_signatures;
mod signature_comptime;

use self::extension_providers::*;
use self::program_signatures::*;
use self::signature_comptime::{
    provide_signature_comptime_module, signature_comptime_array_lengths,
    signature_comptime_module_lowering, signature_comptime_values, signature_layouts_for_types,
    with_type_signature_comptime_input,
};

struct BodyProgramSignatureLookup<'a> {
    functions: &'a dyn Fn(GlobalDefId) -> Option<ProgramFunctionSignature>,
    fallback: ProgramSignatureResolvers<'a>,
    maps: Option<ProgramSignatureMaps<'a>>,
}

impl nia_program_signatures::ProgramSignatureLookup for BodyProgramSignatureLookup<'_> {
    fn function(&self, def_id: GlobalDefId) -> Option<ProgramFunctionSignature> {
        (self.functions)(def_id)
    }

    fn global(&self, def_id: GlobalDefId) -> Option<ProgramGlobalSignature> {
        self.maps
            .and_then(|maps| maps.global(def_id))
            .or_else(|| self.fallback.global(def_id))
    }

    fn comptime(&self, def_id: GlobalDefId) -> Option<ProgramComptimeSignature> {
        self.maps
            .and_then(|maps| maps.comptime(def_id))
            .or_else(|| self.fallback.comptime(def_id))
    }

    fn struct_(&self, def_id: GlobalDefId) -> Option<ProgramStructSignature> {
        self.maps
            .and_then(|maps| maps.struct_(def_id))
            .or_else(|| self.fallback.struct_(def_id))
    }

    fn union(&self, def_id: GlobalDefId) -> Option<ProgramUnionSignature> {
        self.maps
            .and_then(|maps| maps.union(def_id))
            .or_else(|| self.fallback.union(def_id))
    }

    fn enum_(&self, def_id: GlobalDefId) -> Option<ProgramEnumSignature> {
        self.maps
            .and_then(|maps| maps.enum_(def_id))
            .or_else(|| self.fallback.enum_(def_id))
    }

    fn trait_(&self, def_id: GlobalDefId) -> Option<ProgramTraitSignature> {
        self.maps
            .and_then(|maps| maps.trait_(def_id))
            .or_else(|| self.fallback.trait_(def_id))
    }

    fn type_alias(&self, def_id: GlobalDefId) -> Option<ProgramTypeAliasSignature> {
        self.maps
            .and_then(|maps| maps.type_alias(def_id))
            .or_else(|| self.fallback.type_alias(def_id))
    }

    fn trait_ids_with_method_named(&self, name: &SymbolId) -> Vec<GlobalDefId> {
        if let Some(maps) = self.maps {
            return maps.trait_ids_with_method_named(name);
        }
        self.fallback.trait_ids_with_method_named(name)
    }

    fn trait_owning_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<(GlobalDefId, ProgramTraitSignature)> {
        self.maps
            .and_then(|maps| maps.trait_owning_method(method_id))
            .or_else(|| self.fallback.trait_owning_method(method_id))
    }
}

struct LazyAssociatedValueResolver<'a> {
    visible_extensions: &'a dyn Fn() -> VisibleExtensionsValue,
    cache: RefCell<Option<VisibleExtensionsValue>>,
}

impl<'a> LazyAssociatedValueResolver<'a> {
    fn new(visible_extensions: &'a dyn Fn() -> VisibleExtensionsValue) -> Self {
        Self {
            visible_extensions,
            cache: RefCell::new(None),
        }
    }

    fn visible_extensions(&self) -> VisibleExtensionsValue {
        if let Some(visible_extensions) = self.cache.borrow().as_ref() {
            return visible_extensions.clone();
        }
        let visible_extensions = (self.visible_extensions)();
        *self.cache.borrow_mut() = Some(visible_extensions.clone());
        visible_extensions
    }

    fn target_matches(
        interner: &nia_ty::TyInterner,
        target_ty: InternedTyId,
        target: nia_value_resolve::AssociatedValueTarget,
    ) -> bool {
        match target {
            nia_value_resolve::AssociatedValueTarget::Primitive(primitive) => {
                matches!(interner.get(target_ty), Some(TyKind::Primitive(found)) if *found == primitive)
            }
            nia_value_resolve::AssociatedValueTarget::Nominal(type_id) => {
                matches!(interner.get(target_ty), Some(TyKind::Nominal { def_id, .. }) if *def_id == type_id)
            }
        }
    }
}

impl nia_value_resolve::AssociatedValueResolver for LazyAssociatedValueResolver<'_> {
    fn associated_value(
        &self,
        target: nia_value_resolve::AssociatedValueTarget,
        name: &SymbolId,
    ) -> Option<GlobalDefId> {
        let visible_extensions = self.visible_extensions();
        let mut matches = Vec::new();
        for extension_target in visible_extensions.methods.targets() {
            if !Self::target_matches(
                &visible_extensions.interner,
                extension_target.target_ty,
                target,
            ) {
                continue;
            }
            for value in &extension_target.associated_values {
                if &value.name == name {
                    matches.push(value.def_id);
                }
            }
        }
        matches.sort();
        matches.dedup();
        let [def_id] = matches.as_slice() else {
            return None;
        };
        Some(*def_id)
    }
}

#[derive(Clone)]
pub(super) struct CompilerQueryProviders {
    pub(super) checked_program: fn(&QueryDb<CompilerContext>) -> CheckedProgram,
    pub(super) entry_checked_program: fn(&QueryDb<CompilerContext>) -> CheckedProgram,
    pub(super) codegen_program: fn(&QueryDb<CompilerContext>) -> CodegenProgram,
    pub(super) module_graph: fn(&QueryDb<CompilerContext>) -> ModuleGraph,
    pub(super) parse_ok_module_ids: fn(&QueryDb<CompilerContext>) -> Vec<ModuleId>,
    pub(super) semantic_module_ids: fn(&QueryDb<CompilerContext>) -> Vec<ModuleId>,
    pub(super) module_item_tree: fn(&QueryDb<CompilerContext>, ModuleId) -> ModuleItemTree,
    pub(super) active_module_item_tree:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ActiveModuleItemTree,
    pub(super) full_module_item_tree: fn(&QueryDb<CompilerContext>, ModuleId) -> ModuleItemTree,
    pub(super) full_active_module_item_tree:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ActiveModuleItemTree,
    pub(super) module_defs: fn(&QueryDb<CompilerContext>, ModuleId) -> DefCollection,
    pub(super) full_module_defs: fn(&QueryDb<CompilerContext>, ModuleId) -> DefCollection,
    pub(super) defs_by_module: fn(&QueryDb<CompilerContext>) -> Vec<DefCollection>,
    pub(super) public_surfaces: fn(&QueryDb<CompilerContext>) -> PublicSurfacesValue,
    pub(super) public_using_scopes: fn(&QueryDb<CompilerContext>) -> PublicUsingScopesValue,
    pub(super) module_using_scope: fn(&QueryDb<CompilerContext>, ModuleId) -> ModuleUsingScope,
    pub(super) type_exposure_index: fn(&QueryDb<CompilerContext>) -> TypeExposureIndexValue,
    pub(super) type_resolution: fn(&QueryDb<CompilerContext>, ModuleId) -> TypeResolution,
    pub(super) declaration_type_resolution:
        fn(&QueryDb<CompilerContext>, ModuleId) -> TypeResolution,
    pub(super) signature_type_resolution:
        fn(&QueryDb<CompilerContext>, ModuleId, nia_item_tree::SignatureItemSet) -> TypeResolution,
    pub(super) signature_comptime_type_resolution:
        fn(&QueryDb<CompilerContext>, ModuleId) -> TypeResolution,
    pub(super) type_lowering: fn(&QueryDb<CompilerContext>, ModuleId) -> TypeLowering,
    pub(super) declaration_type_lowering: fn(&QueryDb<CompilerContext>, ModuleId) -> TypeLowering,
    pub(super) signature_type_lowering:
        fn(&QueryDb<CompilerContext>, ModuleId, nia_item_tree::SignatureItemSet) -> TypeLowering,
    pub(super) signature_comptime_type_lowering:
        fn(&QueryDb<CompilerContext>, ModuleId) -> TypeLowering,
    pub(super) item_signatures: fn(&QueryDb<CompilerContext>, ModuleId) -> ItemSignatures,
    pub(super) signature_item_signatures:
        fn(&QueryDb<CompilerContext>, ModuleId, nia_item_tree::SignatureItemSet) -> ItemSignatures,
    pub(super) signature_comptime_item_signatures:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ItemSignatures,
    pub(super) type_normalization: fn(&QueryDb<CompilerContext>, ModuleId) -> TypeNormalization,
    pub(super) layout_type_normalization:
        fn(&QueryDb<CompilerContext>, ModuleId) -> TypeNormalization,
    pub(super) signature_type_normalization: fn(
        &QueryDb<CompilerContext>,
        ModuleId,
        nia_item_tree::SignatureItemSet,
    ) -> TypeNormalization,
    pub(super) signature_comptime_type_normalization:
        fn(&QueryDb<CompilerContext>, ModuleId) -> TypeNormalization,
    pub(super) signature_comptime_module:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ComptimeModuleLowering,
    pub(super) program_signature_module_ids:
        fn(&QueryDb<CompilerContext>, nia_item_tree::SignatureItemSet) -> Vec<ModuleId>,
    pub(super) program_signature_module_eligibility:
        fn(&QueryDb<CompilerContext>, ModuleId, nia_item_tree::SignatureItemSet) -> bool,
    pub(super) module_program_signature_facts: fn(
        &QueryDb<CompilerContext>,
        ModuleId,
        nia_item_tree::SignatureItemSet,
    ) -> ModuleProgramSignatureFactsValue,
    pub(super) module_abi_signature_facts:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ModuleAbiSignatureFactsValue,
    pub(super) extension_provider_summary:
        fn(&QueryDb<CompilerContext>, ModuleId) -> nia_provider_summary::ProviderSummary,
    pub(super) extension_provider_discovery_index:
        fn(&QueryDb<CompilerContext>) -> ExtensionProviderDiscoveryIndexValue,
    pub(super) extension_provider_module_ids: fn(&QueryDb<CompilerContext>) -> Vec<ModuleId>,
    pub(super) extension_provider_module_eligibility:
        fn(&QueryDb<CompilerContext>, ModuleId) -> bool,
    pub(super) extension_signature_module_input:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ExtensionSignatureModuleInputValue,
    pub(super) extension_trait_solving_module_facts:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ExtensionTraitSolvingModuleFactsValue,
    pub(super) program_trait_solving_signatures:
        fn(&QueryDb<CompilerContext>) -> Arc<ProgramTraitSolvingSignatures>,
    pub(super) program_trait_method_index:
        fn(&QueryDb<CompilerContext>) -> Arc<ProgramTraitMethodIndex>,
    pub(super) program_visible_type_signatures:
        fn(&QueryDb<CompilerContext>) -> Arc<ProgramVisibleTypeSignatures>,
    pub(super) program_backend_signatures:
        fn(&QueryDb<CompilerContext>) -> Arc<ProgramBackendSignatures>,
    pub(super) program_abi_signatures:
        fn(&QueryDb<CompilerContext>) -> Arc<ProgramAbiSignaturesValue>,
    pub(super) extension_provider_module_facts:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ExtensionProviderModuleFactsValue,
    pub(super) extension_provider_validation_facts:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ExtensionProviderValidationFactsValue,
    pub(super) extension_provider_nominal_module_facts:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ExtensionProviderNominalModuleFactsValue,
    pub(super) extension_provider_nominal_candidate_modules:
        fn(
            &QueryDb<CompilerContext>,
            ExtensionProviderNominalTargetNames,
        ) -> ExtensionProviderNominalCandidateModulesValue,
    pub(super) extension_provider_nominal_modules_for_targets:
        fn(
            &QueryDb<CompilerContext>,
            ExtensionProviderNominalTargets,
            ModuleId,
        ) -> ExtensionProviderNominalModulesForTargetsValue,
    pub(super) extension_method_index: fn(&QueryDb<CompilerContext>) -> ExtensionMethodIndexValue,
    pub(super) extension_trait_signature_index:
        fn(&QueryDb<CompilerContext>) -> ExtensionTraitSignatureIndexValue,
    pub(super) visible_extensions:
        fn(&QueryDb<CompilerContext>, ModuleId) -> VisibleExtensionsValue,
    pub(super) visible_trait_impls:
        fn(&QueryDb<CompilerContext>, ModuleId) -> VisibleTraitImplsValue,
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
    pub(super) signature_layouts: fn(&QueryDb<CompilerContext>, ModuleId) -> nia_layout::Layouts,
    pub(super) abi_check: fn(&QueryDb<CompilerContext>, ModuleId) -> nia_abi_check::AbiCheck,
    pub(super) static_check:
        fn(&QueryDb<CompilerContext>, ModuleId) -> nia_static_check::StaticCheck,
    pub(super) flow_check: fn(&QueryDb<CompilerContext>, ModuleId) -> nia_flow_check::FlowCheck,
    pub(super) body_check: fn(&QueryDb<CompilerContext>, ModuleId) -> nia_body_check::BodyCheck,
    pub(super) checked_module: fn(&QueryDb<CompilerContext>, ModuleId) -> CheckedModule,
    pub(super) checked_module_ids: fn(&QueryDb<CompilerContext>) -> Vec<ModuleId>,
    #[cfg(test)]
    pub(super) monomorphization:
        fn(&QueryDb<CompilerContext>) -> nia_monomorphize::Monomorphization,
    #[cfg(test)]
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
            semantic_module_ids: provide_semantic_module_ids,
            module_item_tree: provide_module_item_tree,
            active_module_item_tree: provide_active_module_item_tree,
            full_module_item_tree: provide_full_module_item_tree,
            full_active_module_item_tree: provide_full_active_module_item_tree,
            module_defs: provide_module_defs,
            full_module_defs: provide_full_module_defs,
            defs_by_module: provide_defs_by_module,
            public_surfaces: provide_public_surfaces,
            public_using_scopes: provide_public_using_scopes,
            module_using_scope: provide_module_using_scope,
            type_exposure_index: provide_type_exposure_index,
            type_resolution: provide_type_resolution,
            declaration_type_resolution: provide_declaration_type_resolution,
            signature_type_resolution: provide_signature_type_resolution,
            signature_comptime_type_resolution: provide_signature_comptime_type_resolution,
            type_lowering: provide_type_lowering,
            declaration_type_lowering: provide_declaration_type_lowering,
            signature_type_lowering: provide_signature_type_lowering,
            signature_comptime_type_lowering: provide_signature_comptime_type_lowering,
            item_signatures: provide_item_signatures,
            signature_item_signatures: provide_signature_item_signatures,
            signature_comptime_item_signatures: provide_signature_comptime_item_signatures,
            type_normalization: provide_type_normalization,
            layout_type_normalization: provide_layout_type_normalization,
            signature_type_normalization: provide_signature_type_normalization,
            signature_comptime_type_normalization: provide_signature_comptime_type_normalization,
            signature_comptime_module: provide_signature_comptime_module,
            program_signature_module_ids: provide_program_signature_module_ids,
            program_signature_module_eligibility: provide_program_signature_module_eligibility,
            module_program_signature_facts: provide_module_program_signature_facts,
            module_abi_signature_facts: provide_module_abi_signature_facts,
            extension_provider_summary: provide_extension_provider_summary,
            extension_provider_discovery_index: provide_extension_provider_discovery_index,
            extension_provider_module_ids: provide_extension_provider_module_ids,
            extension_provider_module_eligibility: provide_extension_provider_module_eligibility,
            extension_signature_module_input: provide_extension_signature_module_input,
            extension_trait_solving_module_facts: provide_extension_trait_solving_module_facts,
            program_trait_solving_signatures: provide_program_trait_solving_signatures,
            program_trait_method_index: provide_program_trait_method_index,
            program_visible_type_signatures: provide_program_visible_type_signatures,
            program_backend_signatures: provide_program_backend_signatures,
            program_abi_signatures: provide_program_abi_signatures,
            extension_provider_module_facts: provide_extension_provider_module_facts,
            extension_provider_validation_facts: provide_extension_provider_validation_facts,
            extension_provider_nominal_module_facts:
                provide_extension_provider_nominal_module_facts,
            extension_provider_nominal_candidate_modules:
                provide_extension_provider_nominal_candidate_modules,
            extension_provider_nominal_modules_for_targets:
                provide_extension_provider_nominal_modules_for_targets,
            extension_method_index: provide_extension_method_index,
            extension_trait_signature_index: provide_extension_trait_signature_index,
            visible_extensions: provide_visible_extensions,
            visible_trait_impls: provide_visible_trait_impls,
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
            signature_layouts: provide_signature_layouts,
            abi_check: provide_abi_check,
            static_check: provide_static_check,
            flow_check: provide_flow_check,
            body_check: provide_body_check,
            checked_module: provide_checked_module,
            checked_module_ids: provide_checked_module_ids,
            #[cfg(test)]
            monomorphization: provide_monomorphization,
            #[cfg(test)]
            backend_lowering: provide_backend_lowering,
        }
    }
}

pub(super) fn provide_checked_program(db: &QueryDb<CompilerContext>) -> CheckedProgram {
    time_provider(db.context().timings(), "checked_program", || {
        let graph = db.query(ModuleGraphQuery);
        let optimization = db.query(CompilerOptimizationQuery);
        let mut diagnostics = early_program_diagnostics(db);
        let diagnostic_modules = materialize_checked_modules(db, db.query(CheckedModuleIdsQuery));
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
    time_provider(db.context().timings(), "entry_checked_program", || {
        let graph = db.query(ModuleGraphQuery);
        let optimization = db.query(CompilerOptimizationQuery);
        let mut diagnostics = early_program_diagnostics(db);
        let diagnostic_modules = checked_modules_for_diagnostics(db);
        diagnostics.extend(checked_module_diagnostics(db, &diagnostic_modules));
        CheckedProgram {
            graph,
            optimization,
            modules: diagnostic_modules,
            diagnostics,
        }
    })
}

pub(super) fn provide_codegen_program(db: &QueryDb<CompilerContext>) -> CodegenProgram {
    time_provider(db.context().timings(), "codegen_program", || {
        let graph = db.query(ModuleGraphQuery);
        let optimization = db.query(CompilerOptimizationQuery);
        let mut diagnostics = early_program_diagnostics(db);
        let modules = checked_modules_for_codegen(db);
        diagnostics.extend(checked_module_diagnostics(db, &modules));
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
        let monomorphization = time_provider(db.context().timings(), "monomorphization", || {
            monomorphization_for_checked_modules(db, &modules)
        });
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
        let backend_lowering = time_provider(db.context().timings(), "backend_lowering", || {
            provide_backend_lowering_inner_for_modules(db, &monomorphization, &modules)
        });
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
    nia_timing::time_query(timings, name, f)
}

fn time_module_provider<T>(
    db: &QueryDb<CompilerContext>,
    name: &str,
    module_id: ModuleId,
    f: impl FnOnce() -> T,
) -> T {
    let timings = db.context().timings();
    if !timings.detail() {
        return f();
    }
    let path = db.context().path_for_module(module_id);
    nia_timing::time_query(
        timings,
        &format!("{name}[{module_id:?} {}]", path.as_str()),
        f,
    )
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

pub(super) fn provide_semantic_module_ids(db: &QueryDb<CompilerContext>) -> Vec<ModuleId> {
    let graph = db.query_shared(ModuleGraphQuery);
    let entry = graph.entry();
    db.query(ParseOkModuleIdsQuery)
        .into_iter()
        .filter(|module_id| {
            graph
                .get(*module_id)
                .is_some_and(|node| *module_id == entry || node.process_used_paths)
        })
        .collect()
}

pub(super) fn provide_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ModuleItemTree {
    db.query(ModuleItemTreeInputQuery(module_id))
        .as_ref()
        .clone()
}

pub(super) fn provide_active_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ActiveModuleItemTree {
    let _raw_item_tree = db.query_shared(ModuleItemTreeQuery(module_id));
    db.query(ActiveModuleItemTreeInputQuery(module_id))
        .as_ref()
        .clone()
}

pub(super) fn provide_full_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ModuleItemTree {
    db.query(FullModuleItemTreeInputQuery(module_id))
        .as_ref()
        .clone()
}

pub(super) fn provide_full_active_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ActiveModuleItemTree {
    let _raw_item_tree = db.query_shared(FullModuleItemTreeQuery(module_id));
    db.query(FullActiveModuleItemTreeInputQuery(module_id))
        .as_ref()
        .clone()
}

pub(super) fn provide_module_defs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> DefCollection {
    let item_tree = db.query_shared(ActiveModuleItemTreeQuery(module_id));
    let symbols = db.context().symbols();
    nia_defs::collect_module_defs_from_active_item_tree_with_symbols(
        module_id, &item_tree, &symbols,
    )
}

pub(super) fn provide_full_module_defs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> DefCollection {
    let item_tree = db.query_shared(FullActiveModuleItemTreeQuery(module_id));
    let symbols = db.context().symbols();
    nia_defs::collect_module_defs_from_active_item_tree_with_symbols(
        module_id, &item_tree, &symbols,
    )
}

pub(super) fn provide_defs_by_module(db: &QueryDb<CompilerContext>) -> Vec<DefCollection> {
    db.query_many(
        db.query(ParseOkModuleIdsQuery)
            .into_iter()
            .map(ModuleDefsQuery),
    )
}

pub(super) fn provide_public_surfaces(db: &QueryDb<CompilerContext>) -> PublicSurfacesValue {
    time_provider(db.context().timings(), "public_surfaces", || {
        let defs = db.query(DefsByModuleQuery);
        let graph = db.query_shared(ModuleGraphQuery);
        let symbols = db.context().symbols();
        let exports = compute_exported_public_surfaces_with_symbols(&defs, &graph, &symbols);
        Arc::new(PublicSurfacesQueryValue {
            surfaces: exports.surfaces,
            diagnostics: exports.diagnostics,
        })
    })
}

pub(super) fn provide_public_using_scopes(db: &QueryDb<CompilerContext>) -> PublicUsingScopesValue {
    time_provider(db.context().timings(), "public_using_scopes", || {
        let defs = db.query(DefsByModuleQuery);
        let graph = db.query_shared(ModuleGraphQuery);
        let surfaces = db.query(PublicSurfacesQuery);
        let symbols = db.context().symbols();
        let scopes = compute_using_scopes_from_surfaces_with_symbols(
            &defs,
            &graph,
            &surfaces.surfaces,
            &symbols,
        );
        Arc::new(PublicUsingScopesQueryValue {
            using_scopes: scopes.using_scopes,
            diagnostics: scopes.diagnostics,
        })
    })
}

pub(super) fn provide_module_using_scope(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ModuleUsingScope {
    db.query(PublicUsingScopesQuery)
        .using_scopes
        .get(&module_id)
        .cloned()
        .unwrap_or_default()
}

pub(super) fn provide_type_exposure_index(db: &QueryDb<CompilerContext>) -> TypeExposureIndexValue {
    time_provider(db.context().timings(), "type_exposure_index", || {
        let defs = db.query(DefsByModuleQuery);
        let public_surfaces = db.query(PublicSurfacesQuery);
        let public_using_scopes = db.query(PublicUsingScopesQuery);
        Arc::new(TypeExposureIndex::from_defs_surfaces_and_using_scopes(
            &defs,
            &public_surfaces.surfaces,
            &public_using_scopes.using_scopes,
        ))
    })
}

pub(super) fn provide_type_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeResolution {
    time_module_provider(db, "type_resolution", module_id, || {
        let active_item_tree = db.query_shared(FullActiveModuleItemTreeQuery(module_id));
        let defs = db.query_shared(FullModuleDefsQuery(module_id));
        let program_defs = |module_id| Some(db.query_shared(FullModuleDefsQuery(module_id)));
        let graph = db.query_shared(ModuleGraphQuery);
        let public_surfaces = db.query(PublicSurfacesQuery);
        let using_scope = db.query(ModuleUsingScopeQuery(module_id));
        let symbols = db.context().symbols();
        nia_type_resolve::resolve_module_types_from_active_item_tree_with_symbols(
            &active_item_tree,
            &defs,
            nia_type_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&graph),
            },
            &public_surfaces.surfaces,
            &using_scope,
            &symbols,
        )
    })
}

pub(super) fn provide_declaration_type_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeResolution {
    time_module_provider(db, "declaration_type_resolution", module_id, || {
        let active_item_tree = db.query_shared(DeclarationActiveModuleItemTreeQuery(module_id));
        let defs = db.query_shared(ModuleDefsQuery(module_id));
        let program_defs = |module_id| Some(db.query_shared(ModuleDefsQuery(module_id)));
        let graph = db.query_shared(ModuleGraphQuery);
        let public_surfaces = db.query(PublicSurfacesQuery);
        let using_scope = db.query(ModuleUsingScopeQuery(module_id));
        let symbols = db.context().symbols();
        nia_type_resolve::resolve_module_types_from_active_item_tree_with_symbols(
            &active_item_tree,
            &defs,
            nia_type_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&graph),
            },
            &public_surfaces.surfaces,
            &using_scope,
            &symbols,
        )
    })
}

pub(super) fn provide_signature_type_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> TypeResolution {
    time_module_provider(db, "signature_type_resolution", module_id, || {
        let active_item_tree = db.query_shared(SignatureItemTreeQuery(module_id, set));
        let defs = db.query_shared(ModuleDefsQuery(module_id));
        let program_defs = |module_id| Some(db.query_shared(ModuleDefsQuery(module_id)));
        let graph = db.query_shared(ModuleGraphQuery);
        let public_surfaces = db.query(PublicSurfacesQuery);
        let using_scope = db.query(ModuleUsingScopeQuery(module_id));
        let symbols = db.context().symbols();
        nia_type_resolve::resolve_module_declaration_types_from_active_item_tree_with_symbols(
            &active_item_tree,
            &defs,
            nia_type_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&graph),
            },
            &public_surfaces.surfaces,
            &using_scope,
            &symbols,
        )
    })
}

pub(super) fn provide_signature_comptime_type_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeResolution {
    time_module_provider(db, "signature_comptime_type_resolution", module_id, || {
        let active_item_tree = db.query(SignatureComptimeItemTreeQuery(module_id));
        let defs = db.query_shared(ModuleDefsQuery(module_id));
        let program_defs = |module_id| Some(db.query_shared(ModuleDefsQuery(module_id)));
        let graph = db.query_shared(ModuleGraphQuery);
        let public_surfaces = db.query(PublicSurfacesQuery);
        let using_scope = db.query(ModuleUsingScopeQuery(module_id));
        let symbols = db.context().symbols();
        nia_type_resolve::resolve_module_types_from_active_item_tree_with_symbols(
            &active_item_tree,
            &defs,
            nia_type_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&graph),
            },
            &public_surfaces.surfaces,
            &using_scope,
            &symbols,
        )
    })
}

pub(super) fn provide_type_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeLowering {
    let active_item_tree = db.query_shared(FullActiveModuleItemTreeQuery(module_id));
    let type_resolution = db.query(TypeResolutionQuery(module_id));
    let program_defs = |module_id| Some(db.query_shared(FullModuleDefsQuery(module_id)));
    let symbols = db.context().symbols();
    nia_type_lower::lower_module_types_from_active_item_tree_with_context(
        module_id,
        &active_item_tree,
        &type_resolution,
        nia_type_lower::TypeLoweringContext::from_program_defs(
            nia_type_lower::ProgramDefsContext {
                defs: Some(&program_defs),
            },
        )
        .with_symbols(&symbols),
    )
}

pub(super) fn provide_declaration_type_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeLowering {
    let active_item_tree = db.query_shared(DeclarationActiveModuleItemTreeQuery(module_id));
    let type_resolution = db.query(DeclarationTypeResolutionQuery(module_id));
    let program_defs = |module_id| Some(db.query_shared(ModuleDefsQuery(module_id)));
    let symbols = db.context().symbols();
    nia_type_lower::lower_module_types_from_active_item_tree_with_context(
        module_id,
        &active_item_tree,
        &type_resolution,
        nia_type_lower::TypeLoweringContext::from_program_defs(
            nia_type_lower::ProgramDefsContext {
                defs: Some(&program_defs),
            },
        )
        .with_symbols(&symbols),
    )
}

pub(super) fn provide_signature_type_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> TypeLowering {
    let active_item_tree = db.query_shared(SignatureItemTreeQuery(module_id, set));
    let type_resolution = db.query(SignatureTypeResolutionQuery(module_id, set));
    let program_defs = |module_id| Some(db.query_shared(ModuleDefsQuery(module_id)));
    let symbols = db.context().symbols();
    nia_type_lower::lower_module_declaration_types_from_active_item_tree_with_context(
        module_id,
        &active_item_tree,
        &type_resolution,
        nia_type_lower::TypeLoweringContext::from_program_defs(
            nia_type_lower::ProgramDefsContext {
                defs: Some(&program_defs),
            },
        )
        .with_symbols(&symbols),
    )
}

pub(super) fn provide_signature_comptime_type_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeLowering {
    let active_item_tree = db.query(SignatureComptimeItemTreeQuery(module_id));
    let type_resolution = db.query(SignatureComptimeTypeResolutionQuery(module_id));
    let program_defs = |module_id| Some(db.query_shared(ModuleDefsQuery(module_id)));
    let symbols = db.context().symbols();
    nia_type_lower::lower_module_types_from_active_item_tree_with_context(
        module_id,
        &active_item_tree,
        &type_resolution,
        nia_type_lower::TypeLoweringContext::from_program_defs(
            nia_type_lower::ProgramDefsContext {
                defs: Some(&program_defs),
            },
        )
        .with_symbols(&symbols),
    )
}

pub(super) fn provide_item_signatures(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ItemSignatures {
    let active_item_tree = db.query_shared(DeclarationActiveModuleItemTreeQuery(module_id));
    let defs = db.query_shared(ModuleDefsQuery(module_id));
    let type_lowering = db.query(DeclarationTypeLoweringQuery(module_id));
    let symbols = db.context().symbols();
    nia_item_signatures::collect_item_signatures_from_active_item_tree_with_symbols(
        &active_item_tree,
        &defs,
        &type_lowering,
        &symbols,
    )
}

pub(super) fn provide_signature_item_signatures(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> ItemSignatures {
    let active_item_tree = db.query_shared(SignatureItemTreeQuery(module_id, set));
    let defs = db.query_shared(ModuleDefsQuery(module_id));
    let type_lowering = db.query_shared(SignatureTypeLoweringQuery(module_id, set));
    let symbols = db.context().symbols();
    nia_item_signatures::collect_item_signatures_from_active_item_tree_with_symbols(
        &active_item_tree,
        &defs,
        &type_lowering,
        &symbols,
    )
}

pub(super) fn provide_signature_comptime_item_signatures(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ItemSignatures {
    let active_item_tree = db.query(SignatureComptimeItemTreeQuery(module_id));
    let defs = db.query_shared(ModuleDefsQuery(module_id));
    let type_lowering = db.query(SignatureComptimeTypeLoweringQuery(module_id));
    let symbols = db.context().symbols();
    nia_item_signatures::collect_item_signatures_from_active_item_tree_with_symbols(
        &active_item_tree,
        &defs,
        &type_lowering,
        &symbols,
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
    let type_lowering = db.query_shared(SignatureTypeLoweringQuery(module_id, set));
    let item_signatures = db.query(SignatureItemSignaturesQuery(module_id, set));
    nia_type_normalize::normalize_module_types(module_id, &type_lowering.interner, &item_signatures)
}

pub(super) fn provide_signature_comptime_type_normalization(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeNormalization {
    let type_lowering = db.query(SignatureComptimeTypeLoweringQuery(module_id));
    let item_signatures = db.query(SignatureComptimeItemSignaturesQuery(module_id));
    nia_type_normalize::normalize_module_types(module_id, &type_lowering.interner, &item_signatures)
}

pub(super) fn provide_value_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ValueResolution {
    time_module_provider(db, "value_resolution", module_id, || {
        let active_item_tree = db.query_shared(FullActiveModuleItemTreeQuery(module_id));
        let defs = db.query_shared(FullModuleDefsQuery(module_id));
        let program_defs = |module_id| Some(db.query_shared(FullModuleDefsQuery(module_id)));
        let graph = db.query_shared(ModuleGraphQuery);
        let public_surfaces = db.query(PublicSurfacesQuery);
        let using_scope = db.query(ModuleUsingScopeQuery(module_id));
        let visible_extensions = || db.query(VisibleExtensionsQuery(module_id));
        let associated_values = LazyAssociatedValueResolver::new(&visible_extensions);
        let symbols = db.context().symbols();
        nia_value_resolve::resolve_module_values_from_active_item_tree_with_associated_values_and_symbols(
            &active_item_tree,
            &defs,
            nia_value_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&graph),
            },
            &public_surfaces.surfaces,
            &using_scope,
            Some(&associated_values),
            Some(&symbols),
        )
    })
}

pub(super) fn provide_local_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> LocalResolution {
    let active_item_tree = db.query_shared(FullActiveModuleItemTreeQuery(module_id));
    let defs = db.query_shared(FullModuleDefsQuery(module_id));
    let values = db.query(ValueResolutionQuery(module_id));
    let symbols = db.context().symbols();
    nia_local_resolve::resolve_module_locals_from_active_item_tree_with_origins_and_symbols(
        &active_item_tree,
        &defs,
        &values,
        None,
        &nia_node_id::NodeOriginTable::default(),
        &symbols,
    )
}

pub(super) fn provide_semantic_use_table(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> nia_sema_ir::SemanticUseTable {
    let values = db.query(ValueResolutionQuery(module_id));
    let locals = db.query(LocalResolutionQuery(module_id));
    let type_resolution = db.query(TypeResolutionQuery(module_id));
    let type_lowering = db.query(TypeLoweringQuery(module_id));
    let active_item_tree = db.query_shared(FullActiveModuleItemTreeQuery(module_id));
    let needed_const_exprs =
        needed_const_exprs_for_active_item_tree(&active_item_tree, &type_lowering);
    let const_expr_value_resolution = if needed_const_exprs.is_empty() {
        None
    } else {
        let defs = db.query_shared(FullModuleDefsQuery(module_id));
        let public_surfaces = db.query(PublicSurfacesQuery);
        let using_scope = db.query(ModuleUsingScopeQuery(module_id));
        let visible_extensions = || db.query(VisibleExtensionsQuery(module_id));
        let associated_values = LazyAssociatedValueResolver::new(&visible_extensions);
        let program_defs = |module_id| Some(db.query_shared(FullModuleDefsQuery(module_id)));
        let symbols = db.context().symbols();
        Some(
            nia_value_resolve::resolve_module_values_from_exprs_with_associated_values_and_symbols(
                type_lowering.const_exprs.iter().filter_map(|(id, expr)| {
                    needed_const_exprs.contains(id).then_some(expr.clone())
                }),
                &defs,
                nia_value_resolve::ProgramDefsContext {
                    defs: Some(&program_defs),
                    graph: Some(&db.query_shared(ModuleGraphQuery)),
                },
                &public_surfaces.surfaces,
                &using_scope,
                Some(&associated_values),
                Some(&symbols),
            ),
        )
    };
    semantic_use_table_from_resolution_inputs_with_const_expr_values(
        module_id,
        &active_item_tree,
        &values,
        const_expr_value_resolution.as_ref(),
        Some(&needed_const_exprs),
        &locals,
        &type_resolution,
        &type_lowering,
    )
}

fn semantic_use_table_from_resolution_inputs_with_const_expr_values(
    module_id: ModuleId,
    active_item_tree: &ActiveModuleItemTree,
    values: &ValueResolution,
    const_expr_value_resolution: Option<&ValueResolution>,
    const_expr_value_resolution_ids: Option<&HashSet<GlobalConstExprId>>,
    locals: &LocalResolution,
    type_resolution: &TypeResolution,
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
    builder.extend_node_associated_comptime_projections(
        associated_comptime_projections_from_active_item_tree(active_item_tree, type_lowering),
    );
    builder.extend_node_associated_comptime_projections(
        associated_comptime_projections_from_const_exprs(
            &type_lowering.const_exprs,
            None,
            type_lowering,
        ),
    );
    builder.extend_node_const_generic_uses(
        type_resolution
            .node_const_generic_names
            .iter()
            .map(|(key, name)| (key.clone(), name.clone())),
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
        builder.extend_node_associated_comptime_projections(
            associated_comptime_projections_from_const_exprs(
                &type_lowering.const_exprs,
                const_expr_value_resolution_ids,
                type_lowering,
            ),
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

fn associated_comptime_projections_from_active_item_tree(
    active_item_tree: &ActiveModuleItemTree,
    type_lowering: &TypeLowering,
) -> Vec<(
    nia_node_id::VersionedNodeKey,
    nia_sema_ir::AssociatedComptimeProjection,
)> {
    let mut collector = AssociatedComptimeProjectionCollector {
        type_lowering,
        projections: Vec::new(),
    };
    for item in &active_item_tree.items {
        collector.visit_item_tree_node(item);
    }
    collector.projections
}

fn associated_comptime_projections_from_const_exprs(
    const_exprs: &HashMap<GlobalConstExprId, nia_ast::Expr>,
    ids: Option<&HashSet<GlobalConstExprId>>,
    type_lowering: &TypeLowering,
) -> Vec<(
    nia_node_id::VersionedNodeKey,
    nia_sema_ir::AssociatedComptimeProjection,
)> {
    let mut collector = AssociatedComptimeProjectionCollector {
        type_lowering,
        projections: Vec::new(),
    };
    for (id, expr) in const_exprs {
        if ids.is_some_and(|ids| !ids.contains(id)) {
            continue;
        }
        nia_ast_walk::Visitor::visit_expr(&mut collector, expr);
    }
    collector.projections
}

struct AssociatedComptimeProjectionCollector<'a> {
    type_lowering: &'a TypeLowering,
    projections: Vec<(
        nia_node_id::VersionedNodeKey,
        nia_sema_ir::AssociatedComptimeProjection,
    )>,
}

impl AssociatedComptimeProjectionCollector<'_> {
    fn visit_item_tree_node(&mut self, item: &nia_item_tree::ItemTreeNode) {
        match &item.kind {
            nia_item_tree::ItemTreeNodeKind::Function(function) => {
                if let Some(body) = &function.body {
                    nia_ast_walk::Visitor::visit_block(self, body);
                }
            }
            nia_item_tree::ItemTreeNodeKind::Binding(binding) => {
                if let Some(value) = &binding.value {
                    nia_ast_walk::Visitor::visit_expr(self, value);
                }
            }
            nia_item_tree::ItemTreeNodeKind::Trait(item_trait) => {
                for method in &item_trait.methods {
                    if let Some(body) = &method.function.body {
                        nia_ast_walk::Visitor::visit_block(self, body);
                    }
                }
            }
            nia_item_tree::ItemTreeNodeKind::Extend(extend) => {
                for associated_value in &extend.associated_values {
                    if let Some(value) = &associated_value.binding.value {
                        nia_ast_walk::Visitor::visit_expr(self, value);
                    }
                }
                for method in &extend.methods {
                    if let Some(body) = &method.function.body {
                        nia_ast_walk::Visitor::visit_block(self, body);
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

    fn record_projection(
        &mut self,
        expr: &nia_ast::Expr,
        target: &nia_ast::TypeRef,
        trait_ref: &nia_ast::TypeRef,
        name: &SymbolId,
    ) {
        let Some(self_ty) = self.type_lowering.ty_for_key(&target.node_key) else {
            return;
        };
        let Some(trait_ty) = self.type_lowering.ty_for_key(&trait_ref.node_key) else {
            return;
        };
        let Some((trait_id, trait_args, trait_const_args)) =
            self.trait_id_and_args_from_ty(trait_ty)
        else {
            return;
        };
        self.projections.push((
            expr.node_key.clone(),
            nia_sema_ir::AssociatedComptimeProjection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name: name.clone(),
            },
        ));
    }

    fn trait_id_and_args_from_ty(
        &self,
        ty: InternedTyId,
    ) -> Option<(
        nia_ty::TraitId,
        Vec<InternedTyId>,
        Vec<nia_ty::ConstGenericArg>,
    )> {
        match self.type_lowering.interner.get(ty)? {
            nia_ty::TyKind::Nominal {
                def_id,
                args,
                const_args,
            } => Some((
                nia_ty::TraitId::Source(*def_id),
                args.clone(),
                const_args.clone(),
            )),
            nia_ty::TyKind::BuiltinTrait { trait_id, args } => Some((
                nia_ty::TraitId::Builtin(*trait_id),
                args.clone(),
                Vec::new(),
            )),
            nia_ty::TyKind::TraitObject {
                trait_id,
                trait_args,
                trait_const_args,
                ..
            }
            | nia_ty::TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                ..
            } => Some((*trait_id, trait_args.clone(), trait_const_args.clone())),
            _ => None,
        }
    }
}

impl<'ast> nia_ast_walk::Visitor<'ast> for AssociatedComptimeProjectionCollector<'_> {
    fn visit_expr(&mut self, expr: &'ast nia_ast::Expr) {
        if let nia_ast::ExprKind::Qualified { lhs, name } = &expr.kind
            && let nia_ast::ExprKind::TraitTarget { ty, trait_ref } = &lhs.kind
        {
            self.record_projection(expr, ty, trait_ref, name);
        }
        nia_ast_walk::walk_expr(self, expr);
    }
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
        Some(TyKind::Nominal {
            args, const_args, ..
        }) => {
            for arg in args {
                collect_array_len_const_exprs_in_ty(interner, *arg, candidate_ids, out, seen);
            }
            for arg in const_args {
                collect_array_len_const_exprs_in_ty(interner, arg.ty, candidate_ids, out, seen);
                collect_array_len_const_exprs_in_const_arg(arg, candidate_ids, out);
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
            | TyKind::SelfParam
            | TyKind::GenericParam(_)
            | TyKind::Primitive(_)
            | TyKind::BuiltinType(_)
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
        ArrayLenTy::Infer | ArrayLenTy::GenericParam(_) | ArrayLenTy::ConstValue(_) => {}
    }
}

fn collect_array_len_const_exprs_in_const_arg(
    arg: &nia_ty::ConstGenericArg,
    candidate_ids: &HashSet<GlobalConstExprId>,
    out: &mut HashSet<GlobalConstExprId>,
) {
    if let nia_ty::ConstGenericValue::ConstExpr(id) = arg.value
        && candidate_ids.contains(&id)
    {
        out.insert(id);
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

fn signature_type_interner(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> nia_ty::TyInterner {
    db.query_shared(SignatureTypeLoweringQuery(module_id, set))
        .interner
        .clone()
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
    let active_item_tree = db.query_shared(FullActiveModuleItemTreeQuery(module_id));
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
        if let Some(signatures) = program_signatures_override {
            return Some(signatures.trait_impls.clone());
        }
        Some(
            db.query(VisibleTraitImplsQuery(module_id))
                .trait_impls
                .clone(),
        )
    };
    let program_is_enum = |def_id: GlobalDefId| {
        program_signatures_override.is_some_and(|signatures| signatures.enums.contains_key(&def_id))
            || db
                .query(SignatureItemSignaturesQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ))
                .enums
                .contains_key(&def_id.def_id)
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
            let filtered_active_item_tree = Arc::new(time_module_provider(
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
            ));
            let program_defs = |module_id| Some(db.query_shared(FullModuleDefsQuery(module_id)));
            let public_surfaces = time_module_provider(
                db,
                "executable_body_check.public_surfaces",
                module_id,
                || db.query(PublicSurfacesQuery),
            );
            let using_scope = time_module_provider(
                db,
                "executable_body_check.module_using_scope",
                module_id,
                || db.query(ModuleUsingScopeQuery(module_id)),
            );
            let visible_extensions = || {
                time_module_provider(
                    db,
                    "executable_body_check.visible_extensions",
                    module_id,
                    || db.query(VisibleExtensionsQuery(module_id)),
                )
            };
            let associated_values = LazyAssociatedValueResolver::new(&visible_extensions);
            let symbols = db.context().symbols();
            let filtered_values = time_module_provider(
                db,
                "executable_body_check.value_resolution",
                module_id,
                || {
                    nia_value_resolve::resolve_module_values_from_active_item_tree_with_associated_values_and_symbols(
                        &filtered_active_item_tree,
                        context.defs,
                        nia_value_resolve::ProgramDefsContext {
                            defs: Some(&program_defs),
                            graph: Some(&db.query_shared(ModuleGraphQuery)),
                        },
                        &public_surfaces.surfaces,
                        &using_scope,
                        Some(&associated_values),
                        Some(&symbols),
                    )
                },
            );
            let filtered_locals = time_module_provider(
                db,
                "executable_body_check.local_resolution",
                module_id,
                || {
                    nia_local_resolve::resolve_module_locals_from_filtered_active_item_tree_with_origins_and_symbols(
                        &filtered_active_item_tree,
                        &context.active_item_tree,
                        context.defs,
                        &filtered_values,
                        Some(context.source_version),
                        context.origins,
                        &symbols,
                    )
                },
            );
            let filtered_semantic_uses = time_module_provider(
                db,
                "executable_body_check.semantic_uses",
                module_id,
                || {
                    let needed_const_exprs = needed_const_exprs_for_active_item_tree(
                        &filtered_active_item_tree,
                        context.lowered,
                    );
                    let const_expr_value_resolution = time_module_provider(
                        db,
                        "executable_body_check.const_expr_value_resolution",
                        module_id,
                        || {
                            nia_value_resolve::resolve_module_values_from_exprs_with_associated_values_and_symbols(
                                context.lowered.const_exprs.iter().filter_map(|(id, expr)| {
                                    needed_const_exprs.contains(id).then_some(expr.clone())
                                }),
                                context.defs,
                                nia_value_resolve::ProgramDefsContext {
                                    defs: Some(&program_defs),
                                    graph: Some(&db.query_shared(ModuleGraphQuery)),
                                },
                                &public_surfaces.surfaces,
                                &using_scope,
                                Some(&associated_values),
                                Some(&symbols),
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
                        context.type_resolution,
                        context.lowered,
                    )
                },
            );
            BodyCheckResolutionInputs {
                active_item_tree: filtered_active_item_tree,
                values: filtered_values,
                locals: filtered_locals,
                semantic_uses: filtered_semantic_uses,
            }
        }
    }
}

#[derive(Clone)]
struct BodyCheckResolutionInputs {
    active_item_tree: Arc<ActiveModuleItemTree>,
    values: ValueResolution,
    locals: LocalResolution,
    semantic_uses: nia_sema_ir::SemanticUseTable,
}

struct BodyCheckWithResolutionInputs {
    body_check: nia_body_check::BodyCheck,
    inputs: BodyCheckResolutionInputs,
    comptime: Option<ComptimeCheck>,
}

struct BodyCheckResolutionContext<'a> {
    source_version: nia_source::SourceVersion,
    origins: &'a nia_node_id::NodeOriginTable,
    active_item_tree: Arc<ActiveModuleItemTree>,
    defs: &'a DefCollection,
    type_resolution: &'a TypeResolution,
    lowered: &'a TypeLowering,
}

struct LocalExecutableValueRefs<'a> {
    module_id: ModuleId,
    defs: &'a DefCollection,
    values: &'a ValueResolution,
    signatures: &'a HashMap<DefId, nia_item_signatures::FunctionSignature>,
}

struct BodyCheckComptimeInputs {
    module: ComptimeModuleLowering,
    array_lengths: nia_comptime_check::ComptimeArrayLengths,
    enum_values: nia_comptime_check::ComptimeEnumValues,
    values: nia_comptime_check::ComptimeValues,
    typed_facts: nia_comptime_check::ComptimeTypedFacts,
}

#[derive(Clone, Copy)]
struct ExecutableFactMode<'a> {
    program_signatures: Option<&'a ProgramExecutableSignatures>,
    reachable_body_modules: Option<&'a HashSet<ModuleId>>,
}

impl<'a> ExecutableFactMode<'a> {
    fn full() -> Self {
        Self {
            program_signatures: None,
            reachable_body_modules: None,
        }
    }

    fn executable(
        program_signatures: &'a ProgramExecutableSignatures,
        reachable_body_modules: &'a HashSet<ModuleId>,
    ) -> Self {
        Self {
            program_signatures: Some(program_signatures),
            reachable_body_modules: Some(reachable_body_modules),
        }
    }

    fn signature_facts_for(self, module_id: ModuleId) -> bool {
        self.program_signatures.is_some()
            && !self
                .reachable_body_modules
                .is_some_and(|modules| modules.contains(&module_id))
    }
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
        || db.query_shared(FullActiveModuleItemTreeQuery(global_id.module_id)),
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
    let program_defs = |module_id| Some(db.query_shared(FullModuleDefsQuery(module_id)));
    let public_surfaces = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.public_surfaces",
        global_id.module_id,
        || db.query(PublicSurfacesQuery),
    );
    let using_scope = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.module_using_scope",
        global_id.module_id,
        || db.query(ModuleUsingScopeQuery(global_id.module_id)),
    );
    let source_version = db.query(ModuleSourceVersionQuery(global_id.module_id));
    let origins = db.query(ModuleOriginsQuery(global_id.module_id));
    let lowered = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.type_lowering",
        global_id.module_id,
        || db.query(TypeLoweringQuery(global_id.module_id)),
    );
    let type_resolution = db.query(TypeResolutionQuery(global_id.module_id));
    let signatures = db.query(ItemSignaturesQuery(global_id.module_id));
    let needed_const_exprs = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.needed_const_exprs",
        global_id.module_id,
        || needed_const_exprs_for_active_item_tree(&filtered_active_item_tree, &lowered),
    );
    let symbols = db.context().symbols();
    let const_expr_value_resolution = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.const_expr_value_resolution",
        global_id.module_id,
        || {
            let visible_extensions = || db.query(VisibleExtensionsQuery(global_id.module_id));
            let associated_values = LazyAssociatedValueResolver::new(&visible_extensions);
            nia_value_resolve::resolve_module_values_from_exprs_with_associated_values_and_symbols(
                lowered.const_exprs.iter().filter_map(|(id, expr)| {
                    needed_const_exprs.contains(id).then_some(expr.clone())
                }),
                &defs,
                nia_value_resolve::ProgramDefsContext {
                    defs: Some(&program_defs),
                    graph: Some(&db.query_shared(ModuleGraphQuery)),
                },
                &public_surfaces.surfaces,
                &using_scope,
                Some(&associated_values),
                Some(&symbols),
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
                nia_local_resolve::resolve_module_locals_from_filtered_active_item_tree_with_origins_and_symbols(
                    &filtered_active_item_tree,
                    &active_item_tree,
                    &defs,
                    &values,
                    Some(source_version),
                    &origins,
                    &symbols,
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
                    &type_resolution,
                    &lowered,
                )
            },
        );
        let lowered = time_module_provider(
            db,
            "executable_body_check.comptime.global_initializer.lower_module",
            global_id.module_id,
            || {
                let symbols = db.context().symbols();
                nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
                    active_item_tree: &filtered_active_item_tree,
                    defs: &defs,
                    signatures: &signatures,
                    values: &values,
                    locals: &locals,
                    semantic_uses: &semantic_uses,
                    symbols: &symbols,
                    const_exprs: &filtered_const_exprs,
                    source_path: &source_path,
                })
            },
        );
        lowered
            .module
            .global_initializers()
            .get(&global_id)
            .or_else(|| {
                lowered
                    .module
                    .deferred_global_initializers()
                    .get(&global_id)
            })
            .cloned()
    };
    let values = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.value_resolution",
        global_id.module_id,
        || {
            let visible_extensions = || {
                time_module_provider(
                    db,
                    "executable_body_check.comptime.global_initializer.visible_extensions",
                    global_id.module_id,
                    || db.query(VisibleExtensionsQuery(global_id.module_id)),
                )
            };
            let associated_values = LazyAssociatedValueResolver::new(&visible_extensions);
            let symbols = db.context().symbols();
            nia_value_resolve::resolve_module_values_from_active_item_tree_with_associated_values_and_symbols(
                &filtered_active_item_tree,
                &defs,
                nia_value_resolve::ProgramDefsContext {
                    defs: Some(&program_defs),
                    graph: Some(&db.query_shared(ModuleGraphQuery)),
                },
                &public_surfaces.surfaces,
                &using_scope,
                Some(&associated_values),
                Some(&symbols),
            )
        },
    );
    lower_with_values(values)
}

fn executable_program_global_initializer(
    db: &QueryDb<CompilerContext>,
    global_id: GlobalDefId,
    fact_mode: ExecutableFactMode<'_>,
) -> Option<nia_comptime_ir::ResolvedComptimeExpr> {
    if fact_mode.signature_facts_for(global_id.module_id) {
        let module = signature_comptime_module_lowering(db, global_id.module_id).module;
        return module
            .global_initializers()
            .get(&global_id)
            .or_else(|| module.deferred_global_initializers().get(&global_id))
            .cloned();
    }
    filtered_comptime_global_initializer_for_body_check(db, global_id)
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
    fact_mode: ExecutableFactMode<'_>,
    global_initializer_cache: Option<
        &RefCell<HashMap<GlobalDefId, Option<nia_comptime_ir::ResolvedComptimeExpr>>>,
    >,
    comptime_module_cache: Option<&RefCell<HashMap<ModuleId, ComptimeModuleLowering>>>,
) -> BodyCheckComptimeInputs {
    let needed_const_exprs =
        needed_const_exprs_for_active_item_tree(&inputs.active_item_tree, lowered);
    let filtered_const_exprs = const_expr_subset_for_ids(&lowered.const_exprs, &needed_const_exprs);
    let lower_module = || {
        time_module_provider(
            db,
            "executable_body_check.comptime.lower_module",
            module_id,
            || {
                let symbols = db.context().symbols();
                nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
                    active_item_tree: &inputs.active_item_tree,
                    defs,
                    signatures,
                    values: &inputs.values,
                    locals: &inputs.locals,
                    semantic_uses: &inputs.semantic_uses,
                    symbols: &symbols,
                    const_exprs: &filtered_const_exprs,
                    source_path,
                })
            },
        )
    };
    let module = if let Some(cache) = comptime_module_cache {
        if !cache.borrow().contains_key(&module_id) {
            let module = lower_module();
            cache.borrow_mut().insert(module_id, module);
        }
        cache
            .borrow()
            .get(&module_id)
            .expect("cached comptime module lowering must exist")
            .clone()
    } else {
        lower_module()
    };
    let program_module = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return Some(signature_comptime_module_lowering(db, module_id).module);
        }
        Some(db.query(ComptimeModuleQuery(module_id)).module)
    };
    let program_source_path = |module_id| Some(db.query(ModulePathQuery(module_id)));
    let program_defs = |module_id| Some(db.query_shared(FullModuleDefsQuery(module_id)));
    let program_type_normalization = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return Some(db.query(SignatureTypeNormalizationQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Types,
            )));
        }
        Some(db.query(TypeNormalizationQuery(module_id)))
    };
    let value_type_normalization = |module_id| {
        Some(db.query(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Values,
        )))
    };
    let trait_impls_for_module = |module_id| {
        if let Some(signatures) = fact_mode.program_signatures {
            return Some(signatures.trait_impls.clone());
        }
        Some(
            db.query(VisibleTraitImplsQuery(module_id))
                .trait_impls
                .clone(),
        )
    };
    let program_is_enum = |def_id: GlobalDefId| {
        fact_mode
            .program_signatures
            .is_some_and(|signatures| signatures.enums.contains_key(&def_id))
            || db
                .query(SignatureItemSignaturesQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ))
                .enums
                .contains_key(&def_id.def_id)
    };
    let item_signatures_for_module = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return Some(db.query(SignatureItemSignaturesQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Types,
            )));
        }
        Some(db.query(ItemSignaturesQuery(module_id)))
    };
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
                let initializer = executable_program_global_initializer(db, global_id, fact_mode);
                cache.borrow_mut().insert(global_id, initializer);
            }
            return cache.borrow().get(&global_id).cloned().flatten();
        }
        executable_program_global_initializer(db, global_id, fact_mode)
    };
    let target = db.query(CompilerTargetQuery);
    let symbols = db.context().symbols();
    let comptime_input = nia_comptime_check::ComptimeInput {
        module: &module.module,
        defs,
        values: &inputs.values,
        locals: &inputs.locals,
        semantic_uses: &inputs.semantic_uses,
        symbols: &symbols,
        lowered,
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
            program_is_enum: Some(&program_is_enum),
            trait_impls_for_module: Some(&trait_impls_for_module),
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
        match program_signatures_override {
            Some(program_signatures) => ExecutableFactMode {
                program_signatures: Some(program_signatures),
                reachable_body_modules: None,
            },
            None => ExecutableFactMode::full(),
        },
        None,
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
    fact_mode: ExecutableFactMode<'_>,
    resolution_inputs: Option<BodyCheckResolutionInputs>,
    seed_interner: Option<nia_ty::TyInterner>,
    global_initializer_cache: Option<
        &RefCell<HashMap<GlobalDefId, Option<nia_comptime_ir::ResolvedComptimeExpr>>>,
    >,
    comptime_module_cache: Option<&RefCell<HashMap<ModuleId, ComptimeModuleLowering>>>,
    program_function_signature_cache: Option<
        &RefCell<HashMap<GlobalDefId, ProgramFunctionSignature>>,
    >,
) -> BodyCheckWithResolutionInputs {
    let source_version = db.query(ModuleSourceVersionQuery(module_id));
    let origins = db.query(ModuleOriginsQuery(module_id));
    let active_item_tree = db.query_shared(FullActiveModuleItemTreeQuery(module_id));
    let defs = db.query_shared(FullModuleDefsQuery(module_id));
    let program_defs = |module_id| Some(db.query_shared(FullModuleDefsQuery(module_id)));
    let type_resolution = db.query(TypeResolutionQuery(module_id));
    let lowered = db.query(TypeLoweringQuery(module_id));
    let executable_reachable_filter = matches!(
        filter,
        nia_body_check::BodyCheckFilter::ReachableItems { .. }
            | nia_body_check::BodyCheckFilter::ReachableFunctions(_)
    );
    let filtered_inputs = resolution_inputs.unwrap_or_else(|| {
        let input_filter = if executable_reachable_filter {
            nia_body_check::BodyCheckFilter::All
        } else {
            filter
        };
        body_check_resolution_inputs_for_filter(
            db,
            module_id,
            input_filter,
            BodyCheckResolutionContext {
                source_version,
                origins: &origins,
                active_item_tree,
                defs: &defs,
                type_resolution: &type_resolution,
                lowered: &lowered,
            },
        )
    });
    let inputs = &filtered_inputs;
    let source_path = db.query(ModulePathQuery(module_id));
    let signatures = body_local_item_signatures(db, module_id, &lowered);
    let normalization = db.query(TypeNormalizationQuery(module_id));
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
                        inputs,
                        fact_mode,
                        global_initializer_cache,
                        comptime_module_cache,
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
            .or_else(|| Some(db.query(SignatureLayoutsQuery(module_id))))
    };
    let extensions = db.query(VisibleExtensionsQuery(module_id));
    let empty_program_extension_methods = nia_defs::ExtensionMethods::default();
    let program_extension_methods = &empty_program_extension_methods;
    let program_extension_method_by_id = |def_id: GlobalDefId| {
        db.query(ExtensionMethodIndexQuery)
            .methods
            .method_by_id(def_id)
            .cloned()
    };
    let program_extension_methods_named = |name: &SymbolId| {
        db.query(ExtensionMethodIndexQuery)
            .methods
            .methods_named(name)
            .cloned()
            .collect::<Vec<_>>()
    };
    let program_type_normalization = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return Some(db.query(SignatureTypeNormalizationQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Types,
            )));
        }
        Some(db.query(TypeNormalizationQuery(module_id)))
    };
    let local_program_function_signature_cache =
        RefCell::new(HashMap::<GlobalDefId, ProgramFunctionSignature>::new());
    let program_function_signature = |def_id: GlobalDefId| {
        if let Some(cache) = program_function_signature_cache
            && let Some(signature) = cache.borrow().get(&def_id)
        {
            return Some(signature.clone());
        }
        if let Some(signature) = local_program_function_signature_cache.borrow().get(&def_id) {
            return Some(signature.clone());
        }
        db.query(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ))
        .functions
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| {
            let signature = ProgramFunctionSignature {
                name: db
                    .query(ModuleDefsQuery(def_id.module_id))
                    .defs
                    .get(def_id.def_id)
                    .map(|def| def.name.clone())
                    .unwrap_or_default(),
                signature,
                interner: signature_type_interner(
                    db,
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Functions,
                ),
            };
            if let Some(cache) = program_function_signature_cache {
                cache.borrow_mut().insert(def_id, signature.clone());
            } else {
                local_program_function_signature_cache
                    .borrow_mut()
                    .insert(def_id, signature.clone());
            }
            signature
        })
    };
    let program_global_signature = |def_id: GlobalDefId| {
        db.query(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Values,
        ))
        .globals
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramGlobalSignature {
            signature,
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Values,
            ),
        })
    };
    let program_comptime_signature = |def_id: GlobalDefId| {
        db.query(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Values,
        ))
        .comptimes
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramComptimeSignature {
            signature,
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Values,
            ),
        })
    };
    let program_struct_signature = |def_id: GlobalDefId| {
        db.query(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))
        .structs
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramStructSignature {
            signature,
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ),
        })
    };
    let program_union_signature = |def_id: GlobalDefId| {
        db.query(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))
        .unions
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramUnionSignature {
            signature,
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ),
        })
    };
    let program_enum_signature = |def_id: GlobalDefId| {
        db.query(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))
        .enums
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramEnumSignature {
            signature,
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ),
        })
    };
    let program_trait_signature = |def_id: GlobalDefId| {
        db.query(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ))
        .traits
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramTraitSignature {
            signature,
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Traits,
            ),
        })
    };
    let program_type_alias_signature = |def_id: GlobalDefId| {
        db.query(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))
        .type_aliases
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramTypeAliasSignature {
            signature,
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ),
        })
    };
    let program_traits_by_method_name = |name: &SymbolId| {
        db.query(ProgramTraitMethodIndexQuery)
            .trait_ids_with_method_named(name)
    };
    let program_trait_owning_method = |method_id: GlobalDefId| {
        db.query(ProgramTraitMethodIndexQuery)
            .trait_owning_method_id(method_id)
            .and_then(|trait_id| {
                program_trait_signature(trait_id).map(|signature| (trait_id, signature))
            })
    };
    let resolver_program_signatures = ProgramSignatureResolvers {
        function: &program_function_signature,
        global: &program_global_signature,
        comptime: &program_comptime_signature,
        struct_: &program_struct_signature,
        union: &program_union_signature,
        enum_: &program_enum_signature,
        trait_: &program_trait_signature,
        type_alias: &program_type_alias_signature,
        trait_ids_with_method_named: &program_traits_by_method_name,
        trait_owning_method: &program_trait_owning_method,
    };
    let map_program_signatures =
        fact_mode
            .program_signatures
            .map(|signatures| ProgramSignatureMaps {
                functions: &signatures.functions,
                globals: &signatures.globals,
                comptimes: &signatures.comptimes,
                structs: &signatures.structs,
                unions: &signatures.unions,
                enums: &signatures.enums,
                traits: &signatures.traits,
                type_aliases: &signatures.type_aliases,
                trait_method_index: &signatures.trait_method_index,
            });
    let program_signature_lookup = BodyProgramSignatureLookup {
        functions: &program_function_signature,
        fallback: resolver_program_signatures,
        maps: map_program_signatures,
    };
    let visible_trait_impls;
    let program_signatures = if let Some(signatures) = fact_mode.program_signatures {
        ProgramSignatureContext::new(&program_signature_lookup, &signatures.trait_impls)
    } else {
        visible_trait_impls = db.query(VisibleTraitImplsQuery(module_id));
        ProgramSignatureContext::new(&program_signature_lookup, &visible_trait_impls.trait_impls)
    };
    let item_signatures_for_module = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return Some(db.query(SignatureItemSignaturesQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Types,
            )));
        }
        Some(db.query(ItemSignaturesQuery(module_id)))
    };
    let executable_program_comptime_array_lengths =
        RefCell::new(HashMap::<ModuleId, nia_comptime_check::ComptimeArrayLengths>::new());
    let executable_program_comptime_values =
        RefCell::new(HashMap::<ModuleId, nia_comptime_check::ComptimeValues>::new());
    let program_comptime_array_lengths = |module_id| {
        if let Some(signatures) = fact_mode.program_signatures {
            if !executable_program_comptime_array_lengths
                .borrow()
                .contains_key(&module_id)
            {
                let array_lengths = if fact_mode.signature_facts_for(module_id) {
                    signature_comptime_array_lengths(db, module_id, Some(signatures))
                } else {
                    with_comptime_input_and_program_signatures(
                        db,
                        module_id,
                        Some(signatures),
                        |input, module| {
                            let mut array_lengths =
                                nia_comptime_check::compute_module_comptime_array_lengths(input);
                            array_lengths.diagnostics.extend(module.diagnostics.clone());
                            array_lengths
                        },
                    )
                };
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
        if let Some(signatures) = fact_mode.program_signatures {
            if !executable_program_comptime_values
                .borrow()
                .contains_key(&module_id)
            {
                let values = if fact_mode.signature_facts_for(module_id) {
                    signature_comptime_values(db, module_id, Some(signatures))
                } else {
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
                    with_comptime_input_and_program_signatures(
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
                    )
                };
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
    let program_comptime_module = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return Some(signature_comptime_module_lowering(db, module_id).module);
        }
        Some(db.query(ComptimeModuleQuery(module_id)).module)
    };
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
                symbols: &db.context().symbols(),
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
                program_extension_methods,
                extension_interner: Some(&extensions.interner),
                program: nia_body_check::BodyProgramContext {
                    defs: Some(&program_defs),
                    type_normalizations: Some(&program_type_normalization),
                    extension_type_normalizations: Some(&extension_method_normalization),
                    signatures: Some(&item_signatures_for_module),
                    layouts: Some(&program_layouts),
                    visible_extensions: Some(&program_visible_extensions),
                    extension_method_by_id: Some(&program_extension_method_by_id),
                    extension_methods_named: Some(&program_extension_methods_named),
                },
                program_signatures,
                function_scope: nia_body_check::FunctionCheckScope::ProgramSignatures,
                program_comptime: nia_body_check::ProgramComptimeMaps {
                    values: &program_comptime_values,
                    array_lengths: &program_comptime_array_lengths,
                    module: &program_comptime_module,
                },
                filter,
            },
            db.context().timings(),
        )
    };
    let body_check = run_body_check(inputs, body_comptime, comptime_module, filter);
    let stored_inputs = match filter {
        nia_body_check::BodyCheckFilter::ReachableItems {
            globals,
            already_checked_functions,
            already_checked_globals,
            ..
        } => {
            let checked_functions = body_check.checked_functions.clone();
            let stored_filter = nia_body_check::BodyCheckFilter::ReachableItems {
                functions: &checked_functions,
                globals,
                already_checked_functions,
                already_checked_globals,
            };
            body_check_resolution_inputs_for_filter(
                db,
                module_id,
                stored_filter,
                BodyCheckResolutionContext {
                    source_version,
                    origins: &origins,
                    active_item_tree: db.query_shared(FullActiveModuleItemTreeQuery(module_id)),
                    defs: &defs,
                    type_resolution: &type_resolution,
                    lowered: &lowered,
                },
            )
        }
        nia_body_check::BodyCheckFilter::ReachableFunctions(_)
        | nia_body_check::BodyCheckFilter::All => filtered_inputs,
    };
    BodyCheckWithResolutionInputs {
        body_check,
        inputs: stored_inputs,
        comptime: filtered_comptime_inputs.map(BodyCheckComptimeInputs::into_check),
    }
}

fn body_local_item_signatures(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    lowered: &TypeLowering,
) -> ItemSignatures {
    let defs = db.query_shared(FullModuleDefsQuery(module_id));
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
    function_signatures.extend(traits.functions.clone());
    let mut global_signatures = values.globals;
    global_signatures.extend(functions.globals);
    global_signatures.extend(extension_functions.globals);
    global_signatures.extend(traits.globals);
    let mut comptime_signatures = values.comptimes;
    comptime_signatures.extend(functions.comptimes);
    comptime_signatures.extend(extension_functions.comptimes);
    comptime_signatures.extend(traits.comptimes);
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
        globals: global_signatures,
        comptimes: comptime_signatures,
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
    let active_item_tree = db.query_shared(SignatureItemTreeQuery(module_id, set));
    let symbols = db.context().symbols();
    nia_item_signatures::collect_item_signatures_from_active_item_tree_with_symbols(
        &active_item_tree,
        defs,
        lowered,
        &symbols,
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
        let defs = db.query_shared(FullModuleDefsQuery(module_id));
        let active_item_tree = db.query_shared(FullActiveModuleItemTreeQuery(module_id));
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
                interner: signature_type_interner(
                    db,
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ),
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
                interner: signature_type_interner(
                    db,
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ),
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
                interner: signature_type_interner(
                    db,
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ),
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
                interner: signature_type_interner(
                    db,
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ),
            })
        };
        let executable_array_lengths = |id: nia_ids::GlobalConstExprId| {
            if let Some(array_length_cache) = array_length_cache {
                if !array_length_cache.borrow().contains_key(&id.module_id) {
                    let has_reachable_body_items = has_reachable_executable_body_items(
                        db,
                        id.module_id,
                        reachable_functions,
                        reachable_globals,
                    );
                    let array_lengths = if has_reachable_body_items {
                        with_comptime_input_and_program_signatures(
                            db,
                            id.module_id,
                            program_signatures_override,
                            |input, module| {
                                let mut array_lengths =
                                    nia_comptime_check::compute_module_comptime_array_lengths(
                                        input,
                                    );
                                array_lengths.diagnostics.extend(module.diagnostics.clone());
                                array_lengths
                            },
                        )
                    } else {
                        with_type_signature_comptime_input(
                            db,
                            id.module_id,
                            program_signatures_override,
                            |input, module| {
                                let mut array_lengths =
                                    nia_comptime_check::compute_module_comptime_array_lengths(
                                        input,
                                    );
                                array_lengths.diagnostics.extend(module.diagnostics.clone());
                                array_lengths
                            },
                        )
                    };
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
            let symbols = db.context().symbols();
            nia_layout::compute_layouts_for_roots_with_program_context(
                nia_layout::LayoutComputationInput {
                    defs: &defs,
                    interner: &layout_interner,
                    signatures: &item_signatures,
                    normalized: &type_normalization.normalized,
                    array_lengths: &executable_array_lengths,
                    target: nia_layout::TargetDataLayout::LP64,
                    program: nia_layout::ProgramLayoutContext {
                        symbols: Some(&symbols),
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
        let has_reachable_body_items = has_reachable_executable_body_items(
            db,
            module_id,
            reachable_functions,
            reachable_globals,
        );
        let layouts = if has_reachable_body_items {
            executable_layouts_for_reachable_items(
                db,
                module_id,
                reachable_functions,
                reachable_globals,
                array_length_cache,
                program_signatures_override,
            )
        } else {
            signature_layouts_for_types(db, module_id, program_signatures_override)
        };
        cache.borrow_mut().insert(module_id, layouts.clone());
        Some(layouts)
    }
}

fn has_reachable_executable_body_items(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
) -> bool {
    reachable_functions
        .iter()
        .any(|def_id| def_id.module_id == module_id)
        || reachable_globals
            .iter()
            .any(|def_id| def_id.module_id == module_id && is_runtime_global_def(db, *def_id))
}

fn executable_reachable_body_modules(
    db: &QueryDb<CompilerContext>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
) -> HashSet<ModuleId> {
    reachable_functions
        .iter()
        .map(|def_id| def_id.module_id)
        .chain(
            reachable_globals
                .iter()
                .filter(|def_id| is_runtime_global_def(db, **def_id))
                .map(|def_id| def_id.module_id),
        )
        .collect()
}

fn is_runtime_global_def(db: &QueryDb<CompilerContext>, def_id: GlobalDefId) -> bool {
    db.query(ModuleDefsQuery(def_id.module_id))
        .defs
        .get(def_id.def_id)
        .is_some_and(|def| def.kind == DefKind::Global)
}

fn rooted_layouts_for_checked_module(
    db: &QueryDb<CompilerContext>,
    module: &CheckedModule,
    program_layouts_override: Option<&dyn Fn(ModuleId) -> Option<nia_layout::Layouts>>,
    program_array_lengths_override: Option<&dyn Fn(nia_ids::GlobalConstExprId) -> Option<u64>>,
) -> nia_layout::Layouts {
    if module.executable_type_only {
        return module.layouts.clone();
    }
    let item_signatures = db.query(ItemSignaturesQuery(module.id));
    let roots = checked_module_layout_roots(module);
    let array_lengths = &module.comptime.array_lengths;
    let symbols = db.context().symbols();
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
                symbols: Some(&symbols),
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
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                self.add_global_struct(def_id);
                self.add_global_union(def_id);
                for arg in &args {
                    self.add(*arg);
                }
                for arg in &const_args {
                    self.add(arg.ty);
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
            | Some(TyKind::BuiltinType(_))
            | Some(TyKind::Vector { .. })
            | Some(TyKind::Error)
            | Some(TyKind::ComptimeOnly)
            | Some(TyKind::SelfParam)
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
        generics: &[SymbolId],
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
            .collect::<SymbolMap<_>>();
        for field in fields {
            let field_ty = self.substitute_generics(field.ty, &substitutions);
            self.add(field_ty);
        }
    }

    fn substitute_generics(
        &mut self,
        ty: InternedTyId,
        substitutions: &SymbolMap<InternedTyId>,
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
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.substitute_generics(arg, substitutions))
                    .collect();
                let const_args = const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.substitute_generics(arg.ty, substitutions);
                        arg
                    })
                    .collect();
                self.intern(TyKind::Nominal {
                    def_id,
                    args,
                    const_args,
                })
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.substitute_generics(arg, substitutions))
                    .collect();
                self.intern(TyKind::BuiltinTrait { trait_id, args })
            }
            Some(TyKind::BuiltinType(_)) => ty,
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            }) => {
                let self_ty = self.substitute_generics(self_ty, substitutions);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_generics(arg, substitutions))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.substitute_generics(arg.ty, substitutions);
                        arg
                    })
                    .collect();
                self.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    name,
                })
            }
            Some(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_generics(arg, substitutions))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.substitute_generics(arg.ty, substitutions);
                        arg
                    })
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
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|mut arg| {
                                arg.ty = self.substitute_generics(arg.ty, substitutions);
                                arg
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.substitute_generics(binding.ty, substitutions),
                    })
                    .collect();
                self.intern(TyKind::TraitObject {
                    is_readonly,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_generics(arg, substitutions))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.substitute_generics(arg.ty, substitutions);
                        arg
                    })
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
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|mut arg| {
                                arg.ty = self.substitute_generics(arg.ty, substitutions);
                                arg
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.substitute_generics(binding.ty, substitutions),
                    })
                    .collect();
                self.intern(TyKind::TraitObjectPointee {
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::Primitive(_))
            | Some(TyKind::Vector { .. })
            | Some(TyKind::Error)
            | Some(TyKind::ComptimeOnly)
            | Some(TyKind::SelfParam)
            | None => ty,
        }
    }

    fn substitute_array_len_generics(
        &mut self,
        len: nia_ty::ArrayLenTy,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> nia_ty::ArrayLenTy {
        match len {
            nia_ty::ArrayLenTy::Builtin { builtin, ty } => nia_ty::ArrayLenTy::Builtin {
                builtin,
                ty: self.substitute_generics(ty, substitutions),
            },
            nia_ty::ArrayLenTy::Infer
            | nia_ty::ArrayLenTy::GenericParam(_)
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
        comptime,
    } = body_check;
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

fn executable_signature_checked_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    layouts: nia_layout::Layouts,
    program_signatures: &ProgramExecutableSignatures,
) -> CheckedModule {
    let type_resolution = db.query(SignatureTypeResolutionQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ));
    let type_lowering = db.query(SignatureTypeLoweringQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ));
    let type_normalization = db.query(SignatureTypeNormalizationQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ));
    let (array_lengths, enum_values) = with_type_signature_comptime_input(
        db,
        module_id,
        Some(program_signatures),
        |input, module| {
            let mut array_lengths =
                nia_comptime_check::compute_module_comptime_array_lengths(input);
            array_lengths.diagnostics.extend(module.diagnostics.clone());
            let mut enum_values = nia_comptime_check::compute_module_comptime_enum_values(
                input,
                array_lengths.clone(),
            );
            enum_values.diagnostics.extend(module.diagnostics.clone());
            (array_lengths, enum_values)
        },
    );
    let mut comptime_diagnostics = array_lengths.diagnostics.clone();
    comptime_diagnostics.extend(enum_values.diagnostics.clone());
    CheckedModule {
        id: module_id,
        path: db.query(ModulePathQuery(module_id)),
        defs: db.query(ModuleDefsQuery(module_id)),
        type_resolution,
        type_lowering,
        value_resolution: ValueResolution {
            node_names: HashMap::new(),
            node_qualified_values: HashMap::new(),
            node_builtin_associated_values: HashMap::new(),
            node_variant_enums: HashMap::new(),
            node_qualified_type_prefixes: HashMap::new(),
            diagnostics: Vec::new(),
        },
        local_resolution: nia_local_resolve::LocalResolution {
            locals: nia_local_resolve::LocalMap::default(),
            node_local_defs: HashMap::new(),
            node_uses: HashMap::new(),
            diagnostics: Vec::new(),
        },
        type_normalization: type_normalization.clone(),
        comptime: ComptimeCheck {
            interner: enum_values.interner,
            values: HashMap::new(),
            typed_values: HashMap::new(),
            enum_values: enum_values.values,
            typed_enum_values: enum_values.typed_values,
            array_lengths: array_lengths.values,
            diagnostics: comptime_diagnostics,
        },
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
) -> HashSet<GlobalDefId> {
    let active_item_tree =
        time_module_provider(db, "extend_value_refs.active_item_tree", module_id, || {
            db.query_shared(FullActiveModuleItemTreeQuery(module_id))
        });
    let defs = time_module_provider(db, "extend_value_refs.defs", module_id, || {
        db.query(FullModuleDefsQuery(module_id))
    });
    let signatures = time_module_provider(db, "extend_value_refs.signatures", module_id, || {
        db.query(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ))
    });
    let program_defs = |module_id| Some(db.query_shared(FullModuleDefsQuery(module_id)));
    let public_surfaces =
        time_module_provider(db, "extend_value_refs.public_surfaces", module_id, || {
            db.query(PublicSurfacesQuery)
        });
    let using_scope = time_module_provider(
        db,
        "extend_value_refs.module_using_scope",
        module_id,
        || db.query(ModuleUsingScopeQuery(module_id)),
    );
    let graph = time_module_provider(db, "extend_value_refs.module_graph", module_id, || {
        db.query(ModuleGraphQuery)
    });

    loop {
        let filter = nia_body_check::BodyCheckFilter::ReachableItems {
            functions: &module_functions,
            globals: module_globals,
            already_checked_functions: checked_functions,
            already_checked_globals: None,
        };
        let filtered_active_item_tree =
            time_module_provider(db, "extend_value_refs.filter_item_tree", module_id, || {
                active_item_tree_for_body_check_filter(module_id, &defs, &active_item_tree, filter)
            });
        let values =
            time_module_provider(db, "extend_value_refs.value_resolution", module_id, || {
                nia_value_resolve::resolve_module_values_from_active_item_tree(
                    &filtered_active_item_tree,
                    &defs,
                    nia_value_resolve::ProgramDefsContext {
                        defs: Some(&program_defs),
                        graph: Some(&graph),
                    },
                    &public_surfaces.surfaces,
                    &using_scope,
                )
            });
        let local_refs = LocalExecutableValueRefs {
            module_id,
            defs: &defs,
            values: &values,
            signatures: &signatures.functions,
        };
        let mut changed = false;
        time_module_provider(db, "extend_value_refs.scan_refs", module_id, || {
            changed |= extend_local_executable_functions_from_value_refs(
                &mut module_functions,
                &filtered_active_item_tree,
                &local_refs,
                checked_functions,
            );
        });
        if !changed {
            break;
        }
    }
    module_functions
}

fn extend_local_executable_functions_from_value_refs(
    module_functions: &mut HashSet<GlobalDefId>,
    active_item_tree: &ActiveModuleItemTree,
    refs: &LocalExecutableValueRefs<'_>,
    checked_functions: Option<&HashSet<GlobalDefId>>,
) -> bool {
    let mut keys = HashSet::new();
    collect_active_item_tree_node_keys(active_item_tree, &mut keys);
    let mut changed = false;
    for key in keys {
        if let Some(def_id) =
            refs.values
                .node_names
                .get(&key)
                .and_then(|resolution| match resolution {
                    nia_value_resolve::ValueNameResolution::Def(def_id) => Some(*def_id),
                    nia_value_resolve::ValueNameResolution::External(_)
                    | nia_value_resolve::ValueNameResolution::Module
                    | nia_value_resolve::ValueNameResolution::LocalDeferred
                    | nia_value_resolve::ValueNameResolution::Error => None,
                })
        {
            changed |=
                insert_local_executable_function(module_functions, refs, def_id, checked_functions);
        }
        if let Some(global_id) = refs.values.node_qualified_values.get(&key)
            && global_id.module_id == refs.module_id
        {
            changed |= insert_local_executable_function(
                module_functions,
                refs,
                global_id.def_id,
                checked_functions,
            );
        }
    }
    changed
}

fn insert_local_executable_function(
    module_functions: &mut HashSet<GlobalDefId>,
    refs: &LocalExecutableValueRefs<'_>,
    def_id: DefId,
    checked_functions: Option<&HashSet<GlobalDefId>>,
) -> bool {
    let Some(def) = refs.defs.defs.get(def_id) else {
        return false;
    };
    if !matches!(
        def.kind,
        DefKind::Function | DefKind::Method | DefKind::TraitMethod
    ) {
        return false;
    }
    let Some(signature) = refs.signatures.get(&def_id) else {
        return false;
    };
    if signature.is_comptime || !signature.has_body {
        return false;
    }
    let global_id = GlobalDefId {
        module_id: refs.module_id,
        def_id,
    };
    if checked_functions.is_some_and(|checked| checked.contains(&global_id)) {
        return false;
    }
    module_functions.insert(global_id)
}

fn collect_active_item_tree_node_keys(
    active_item_tree: &ActiveModuleItemTree,
    keys: &mut HashSet<nia_node_id::VersionedNodeKey>,
) {
    struct Collector<'a> {
        keys: &'a mut HashSet<nia_node_id::VersionedNodeKey>,
    }

    impl<'ast> nia_ast_walk::Visitor<'ast> for Collector<'_> {
        fn visit_expr(&mut self, expr: &'ast nia_ast::Expr) {
            self.keys.insert(expr.node_key.clone());
            nia_ast_walk::walk_expr(self, expr);
        }

        fn visit_type(&mut self, ty: &'ast nia_ast::TypeRef) {
            self.keys.insert(ty.node_key.clone());
            nia_ast_walk::walk_type(self, ty);
        }
    }

    let mut collector = Collector { keys };
    for item in &active_item_tree.items {
        nia_ast_walk::Visitor::visit_item(&mut collector, &item.to_ast_item());
    }
}

pub(super) fn provide_checked_module_ids(db: &QueryDb<CompilerContext>) -> Vec<ModuleId> {
    time_provider(db.context().timings(), "checked_module_ids", || {
        db.query(SemanticModuleIdsQuery)
    })
}

pub(super) fn provide_executable_checked_module_set(
    db: &QueryDb<CompilerContext>,
) -> ExecutableCheckedModuleSet {
    time_provider(
        db.context().timings(),
        "executable_checked_module_set",
        || executable_checked_module_set_inner(db),
    )
}

#[cfg(test)]
pub(super) fn provide_executable_checked_modules(
    db: &QueryDb<CompilerContext>,
) -> Vec<CheckedModule> {
    time_provider(db.context().timings(), "executable_checked_modules", || {
        let set = db.query(ExecutableCheckedModuleSetQuery);
        db.context().executable_checked_modules(&set)
    })
}

fn executable_checked_module_set_inner(
    db: &QueryDb<CompilerContext>,
) -> ExecutableCheckedModuleSet {
    let parse_ok = db.query(SemanticModuleIdsQuery);
    let graph = db.query_shared(ModuleGraphQuery);
    let mut program_signatures = None::<ProgramExecutableSignatures>;
    let extension_methods = db.query(ExtensionMethodIndexQuery).methods.clone();
    let caches = ExecutableCheckCaches::default();
    let function_signature = |def_id: GlobalDefId| {
        if let Some(signature) = caches
            .reachability_function_signatures
            .borrow()
            .get(&def_id)
            .cloned()
        {
            return Some(signature);
        }
        let signatures = db.query(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ));
        let lowering = db.query_shared(SignatureTypeLoweringQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ));
        let signature = signatures
            .functions
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramFunctionSignature {
                name: db
                    .query(ModuleDefsQuery(def_id.module_id))
                    .defs
                    .get(def_id.def_id)
                    .map(|def| def.name.clone())
                    .unwrap_or_default(),
                signature,
                interner: lowering.interner.clone(),
            })?;
        let signature = Arc::new(signature);
        caches
            .reachability_function_signatures
            .borrow_mut()
            .insert(def_id, signature.clone());
        Some(signature)
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
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ),
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
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ),
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
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Traits,
            ),
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
                                interner: signature_type_interner(
                                    db,
                                    def_id.module_id,
                                    nia_item_tree::SignatureItemSet::Traits,
                                ),
                            },
                        )
                    })
            })
    };
    let named_function = |module_id, name: SymbolId| {
        let defs = db.query_shared(FullModuleDefsQuery(module_id));
        defs.defs.iter().find_map(|(def_id, def)| {
            (def.kind == DefKind::Function && def.name == name)
                .then_some(GlobalDefId { module_id, def_id })
        })
    };
    let module_functions = |module_id| {
        let defs = db.query_shared(FullModuleDefsQuery(module_id));
        defs.defs
            .iter()
            .filter_map(|(def_id, def)| {
                (def.kind == DefKind::Function).then_some(GlobalDefId { module_id, def_id })
            })
            .collect::<Vec<_>>()
    };
    let mut checked_by_id = HashMap::<ModuleId, ExecutableCheckedModuleState>::new();
    let comptime_module_cache = RefCell::new(HashMap::<ModuleId, ComptimeModuleLowering>::new());
    let mut reachability_state = IncrementalExecutableReachability::default();
    let program_trait_impls = executable_program_trait_impls(db);
    let extension_index = ExecutableExtensionIndex::new(&extension_methods, &program_trait_impls);
    let reachability = loop {
        let reachable_inputs = time_provider(
            db.context().timings(),
            "executable_checked_modules.inputs",
            || reachable_module_inputs(&checked_by_id),
        );
        let mut reachability = time_provider(
            db.context().timings(),
            "executable_checked_modules.reachability_compute",
            || {
                compute_executable_reachability_incremental_with_timings(
                    &mut reachability_state,
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
                    &extension_index,
                    &reachable_inputs,
                    db.context().timings(),
                )
            },
        );
        let mut stale = time_provider(
            db.context().timings(),
            "executable_checked_modules.stale_select",
            || stale_executable_modules(db, &parse_ok, &reachability, &checked_by_id),
        );
        if stale.is_empty() {
            break reachability;
        }
        while let Some(module_id) = stale.pop_front() {
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
            let module_functions = time_module_provider(
                db,
                "executable_checked_modules.extend_local_static_owners",
                module_id,
                || {
                    extend_module_functions_from_local_static_globals(
                        db,
                        module_id,
                        module_functions,
                        &module_globals,
                        already_checked_functions,
                    )
                },
            );
            let module_functions = time_module_provider(
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
                db.context().timings(),
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
                Some(&caches.array_lengths),
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
                    Some(&caches.array_lengths),
                    Some(&*program_signatures),
                );
                let reachable_body_modules = executable_reachable_body_modules(
                    db,
                    &reachability.functions,
                    &reachability.globals,
                );
                time_module_provider(db, "executable_body_check", module_id, || {
                    body_check_with_filter_and_layouts_with_inputs(
                        db,
                        module_id,
                        filter,
                        Some(layouts.clone()),
                        Some(&executable_program_layouts),
                        ExecutableFactMode::executable(program_signatures, &reachable_body_modules),
                        None,
                        seed_interner,
                        Some(&caches.global_initializers),
                        Some(&comptime_module_cache),
                        Some(&caches.body_function_signatures),
                    )
                })
            };
            let checked_this_round = body_check.body_check.checked_functions.clone();
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
            let new_globals_len = module_globals.len();
            let module_path = module.path.clone();
            reachability
                .functions
                .extend(module.body_ir.function_bodies.keys().copied());
            reachability_state.replace_reachability(reachability.clone());
            let flow_check = executable_flow_check(db, module_id, &checked_this_round);
            let mut module = module;
            module.flow_check = flow_check;
            time_module_provider(
                db,
                "executable_checked_modules.state_merge",
                module_id,
                || match checked_by_id.get_mut(&module_id) {
                    Some(state) => state.extend(module, checked_this_round.clone(), module_globals),
                    None => {
                        checked_by_id.insert(
                            module_id,
                            ExecutableCheckedModuleState::new(
                                module,
                                checked_this_round.clone(),
                                module_globals,
                            ),
                        );
                    }
                },
            );
            if let Some(state) = checked_by_id.get(&module_id) {
                print_executable_round_debug(ExecutableRoundDebug {
                    module_id,
                    module_path: &module_path,
                    requested_functions: module_functions.len(),
                    new_functions: checked_this_round.len(),
                    new_globals: new_globals_len,
                    checked_functions_total: state.checked_functions.len(),
                    checked_globals_total: state.checked_globals.len(),
                    reachable_functions_total: reachability.functions.len(),
                    reachable_globals_total: reachability.globals.len(),
                    reachable_modules_total: reachability.modules.len(),
                    type_modules_total: reachability.type_modules.len(),
                });
            }
            let checked_inputs = reachable_module_inputs(&checked_by_id);
            let checked_inputs_by_id = reachable_module_inputs_by_id(&checked_inputs);
            reachability = time_module_provider(
                db,
                "executable_checked_modules.incremental_extend",
                module_id,
                || {
                    extend_incremental_executable_reachability_from_checked_module_with_timings(
                        &mut reachability_state,
                        &parse_ok,
                        nia_executable_reachability::ExecutableSignatureIndex {
                            function: &function_signature,
                            struct_: &struct_signature,
                            union: &union_signature,
                            trait_: &trait_signature,
                            trait_default_method: &trait_default_method,
                        },
                        &extension_index,
                        checked_inputs
                            .iter()
                            .copied()
                            .find(|input| input.module_id == module_id)
                            .expect("just-checked module must have a reachable input"),
                        &checked_this_round,
                        &checked_inputs_by_id,
                        db.context().timings(),
                    )
                },
            );
            for next_module_id in parse_ok.iter().copied() {
                if !reachability.modules.contains(&next_module_id) {
                    continue;
                }
                let is_stale =
                    executable_module_has_pending_body_items(db, next_module_id, &reachability)
                        && executable_module_is_stale(
                            next_module_id,
                            &reachability,
                            &checked_by_id,
                        );
                if !is_stale || stale.contains(&next_module_id) {
                    continue;
                }
                if next_module_id == module_id {
                    stale.push_front(next_module_id);
                } else {
                    stale.push_back(next_module_id);
                }
            }
        }
        reachability_state.replace_reachability(reachability);
    };

    let parse_ok_modules = parse_ok;
    let mut codegen_modules = time_provider(
        db.context().timings(),
        "executable_checked_modules.final.codegen_modules",
        || {
            parse_ok_modules
                .iter()
                .copied()
                .filter(|module_id| reachability.modules.contains(module_id))
                .filter_map(|module_id| checked_by_id.remove(&module_id).map(|state| state.module))
                .collect::<Vec<_>>()
        },
    );
    let codegen_layout_cache = time_provider(
        db.context().timings(),
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
        db.context().timings(),
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
        Some(&caches.array_lengths),
        Some(&*program_signatures),
    );
    let type_only_modules = time_provider(
        db.context().timings(),
        "executable_checked_modules.final.type_only_modules",
        || {
            parse_ok_modules
                .iter()
                .copied()
                .filter(|module_id| reachability.type_modules.contains(module_id))
                .filter(|module_id| !reachability.modules.contains(module_id))
                .map(|module_id| {
                    let layouts = executable_program_layouts(module_id).unwrap_or_else(|| {
                        signature_layouts_for_types(db, module_id, Some(&*program_signatures))
                    });
                    executable_signature_checked_module(db, module_id, layouts, program_signatures)
                })
                .collect::<Vec<_>>()
        },
    );
    codegen_modules.extend(type_only_modules);
    let codegen_array_lengths = time_provider(
        db.context().timings(),
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
                caches
                    .array_lengths
                    .borrow()
                    .get(&id.module_id)
                    .and_then(|array_lengths| array_lengths.values.get(&id).copied())
            })
    };
    codegen_modules = time_provider(
        db.context().timings(),
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
        db.context().timings(),
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
        db.context().timings(),
        "executable_checked_modules.final.store_aggregate_roots",
        || {
            for module in &mut codegen_modules {
                module.executable_reachable_structs = Some(aggregate_roots.structs.clone());
                module.executable_reachable_unions = Some(aggregate_roots.unions.clone());
            }
        },
    );
    db.context()
        .store_executable_checked_modules(codegen_modules)
}

fn extend_module_functions_from_local_static_globals(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    mut module_functions: HashSet<GlobalDefId>,
    module_globals: &HashSet<GlobalDefId>,
    checked_functions: Option<&HashSet<GlobalDefId>>,
) -> HashSet<GlobalDefId> {
    let defs = db.query_shared(FullModuleDefsQuery(module_id));
    for global in module_globals {
        let Some(def) = defs.defs.get(global.def_id) else {
            continue;
        };
        if def.kind != DefKind::Global {
            continue;
        }
        let Some(owner) = def.parent else {
            continue;
        };
        let owner = GlobalDefId {
            module_id,
            def_id: owner,
        };
        if checked_functions.is_some_and(|checked| checked.contains(&owner)) {
            continue;
        }
        module_functions.insert(owner);
    }
    module_functions
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
        let active_item_tree = db.query_shared(FullActiveModuleItemTreeQuery(module_id));
        let type_lowering = db.query_shared(SignatureTypeLoweringQuery(
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

#[cfg(test)]
pub(super) fn provide_monomorphization(
    db: &QueryDb<CompilerContext>,
) -> nia_monomorphize::Monomorphization {
    time_provider(db.context().timings(), "monomorphization", || {
        let checked_modules = checked_modules_for_codegen(db);
        monomorphization_for_checked_modules(db, &checked_modules)
    })
}

fn monomorphization_for_checked_modules(
    db: &QueryDb<CompilerContext>,
    checked_modules: &[CheckedModule],
) -> nia_monomorphize::Monomorphization {
    let runtime = db.query(CompilerRuntimeQuery);
    let executable_signatures;
    let trait_solving_signatures;
    let program_enums_storage;
    let (program_enums, trait_impls) = if runtime == RuntimeModel::FreestandingExecutable {
        executable_signatures = executable_program_signatures_without_functions(db);
        (
            &executable_signatures.enums,
            executable_signatures.trait_impls.as_slice(),
        )
    } else {
        trait_solving_signatures = db.query(ProgramTraitSolvingSignaturesQuery);
        let type_facts = program_signature_facts(db, nia_item_tree::SignatureItemSet::Types);
        program_enums_storage = type_facts
            .iter()
            .flat_map(|facts| {
                facts
                    .enums
                    .iter()
                    .map(|(def_id, signature)| (*def_id, signature.clone()))
            })
            .collect::<HashMap<_, _>>();
        (
            &program_enums_storage,
            trait_solving_signatures.trait_impls.as_slice(),
        )
    };
    let local_signatures = checked_modules
        .iter()
        .map(|module| (module.id, db.query(ItemSignaturesQuery(module.id))))
        .collect::<HashMap<_, _>>();
    let function_bodies = function_bodies_from_checked_modules(db, checked_modules);
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
}

fn checked_modules_for_codegen(db: &QueryDb<CompilerContext>) -> Vec<CheckedModule> {
    if db.query(CompilerRuntimeQuery) == RuntimeModel::FreestandingExecutable {
        materialize_executable_checked_modules(db, db.query(ExecutableCheckedModuleSetQuery))
    } else {
        materialize_checked_modules(db, db.query(CheckedModuleIdsQuery))
    }
}

fn checked_modules_for_diagnostics(db: &QueryDb<CompilerContext>) -> Vec<CheckedModule> {
    if db.query(CompilerRuntimeQuery) == RuntimeModel::FreestandingExecutable {
        materialize_executable_checked_modules(db, db.query(ExecutableCheckedModuleSetQuery))
    } else {
        materialize_checked_modules(db, db.query(CheckedModuleIdsQuery))
    }
}

fn materialize_checked_modules(
    db: &QueryDb<CompilerContext>,
    module_ids: Vec<ModuleId>,
) -> Vec<CheckedModule> {
    db.query_many(module_ids.into_iter().map(CheckedModuleQuery))
}

fn materialize_executable_checked_modules(
    db: &QueryDb<CompilerContext>,
    set: ExecutableCheckedModuleSet,
) -> Vec<CheckedModule> {
    db.context().executable_checked_modules(&set)
}

fn function_bodies_from_checked_modules(
    db: &QueryDb<CompilerContext>,
    checked_modules: &[CheckedModule],
) -> Vec<LoweredFunctionBodies> {
    time_provider(
        db.context().timings(),
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

#[cfg(test)]
pub(super) fn provide_backend_lowering(
    db: &QueryDb<CompilerContext>,
) -> nia_backend_lower::BackendLowering {
    time_provider(db.context().timings(), "backend_lowering", || {
        provide_backend_lowering_inner(db)
    })
}

#[cfg(test)]
fn provide_backend_lowering_inner(
    db: &QueryDb<CompilerContext>,
) -> nia_backend_lower::BackendLowering {
    let checked_modules = checked_modules_for_codegen(db);
    let monomorphization = db.query(MonomorphizationQuery);
    provide_backend_lowering_inner_for_modules(db, &monomorphization, &checked_modules)
}

fn provide_backend_lowering_inner_for_modules(
    db: &QueryDb<CompilerContext>,
    monomorphization: &nia_monomorphize::Monomorphization,
    checked_modules: &[CheckedModule],
) -> nia_backend_lower::BackendLowering {
    let (
        all_visible_extensions,
        active_item_trees,
        item_signatures,
        comptime_array_lengths,
        comptime_enum_values,
        visible_extensions,
        extension_methods,
        function_bodies,
    ) = time_provider(db.context().timings(), "backend_lowering.inputs", || {
        let timings = db.context().timings();
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
                        db.query_shared(FullActiveModuleItemTreeQuery(checked_module.id))
                    })
                    .collect::<Vec<_>>()
            });
        let item_signatures =
            time_provider(timings, "backend_lowering.inputs.item_signatures", || {
                checked_modules
                    .iter()
                    .map(|checked_module| {
                        body_local_item_signatures(
                            db,
                            checked_module.id,
                            &checked_module.type_lowering,
                        )
                    })
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
        let function_bodies = function_bodies_from_checked_modules(db, checked_modules);
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
    });
    let function_lowering_diagnostics =
        function_lowering_diagnostics(checked_modules, &function_bodies);
    if !function_lowering_diagnostics.is_empty() {
        return nia_backend_lower::BackendLowering {
            diagnostics: function_lowering_diagnostics
                .into_iter()
                .map(|program_diagnostic| program_diagnostic.diagnostic)
                .collect(),
            ..empty_backend_lowering(db.query(CompilerOptimizationQuery))
        };
    }
    let indexes = time_provider(db.context().timings(), "backend_lowering.indexes", || {
        build_backend_lowering_indexes(
            &all_visible_extensions,
            checked_modules,
            &comptime_array_lengths,
            &function_bodies,
        )
    });
    let program_defs = |module_id| Some(db.query_shared(FullModuleDefsQuery(module_id)));
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
    let symbols = db.context().symbols();
    let inputs = time_provider(
        db.context().timings(),
        "backend_lowering.module_inputs",
        || {
            build_backend_lowering_module_inputs(BackendLoweringModuleInputsInput {
                symbols: &symbols,
                checked_modules,
                runtime: db.query(CompilerRuntimeQuery),
                active_item_trees: &active_item_trees,
                item_signatures: &item_signatures,
                comptime_array_lengths: &comptime_array_lengths,
                comptime_enum_values: &comptime_enum_values,
                visible_extensions: &visible_extensions,
                function_bodies: &function_bodies,
                extension_methods: &extension_methods.methods,
                program_defs: &program_defs,
                program_signatures,
                indexes: &indexes,
            })
        },
    );
    time_provider(
        db.context().timings(),
        "backend_lowering.lower_backend_program",
        || {
            nia_backend_lower::lower_backend_program_with_timings(
                &inputs,
                monomorphization,
                db.query(CompilerOptimizationQuery),
                db.context().timings(),
            )
        },
    )
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
    let public_surfaces = db.query(PublicSurfacesQuery);
    let public_using_scopes = db.query(PublicUsingScopesQuery);
    for (module_id, diagnostic) in public_surfaces
        .diagnostics
        .iter()
        .chain(public_using_scopes.diagnostics.iter())
    {
        diagnostics.push(ProgramDiagnostic {
            path: db.query(ModulePathQuery(*module_id)),
            diagnostic: diagnostic.clone(),
        });
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
        let extension_validation = db.query(ExtensionProviderValidationFactsQuery(checked.id));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &extension_validation.diagnostics,
        ));
        let extension_provider = db.query(ExtensionProviderModuleFactsQuery(checked.id));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &extension_provider.associated_value_diagnostics,
        ));
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
