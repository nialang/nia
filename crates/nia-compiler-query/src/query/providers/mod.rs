// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_defs::DefKind;
use nia_executable_reachability::{
    ExecutableExtensionLookup, ExecutableRootDefs,
    compute_executable_reachability_incremental_with_timings,
    extend_incremental_executable_reachability_from_checked_module_with_timings,
    filter_semantic_facts_for_reachable_items,
};
use nia_program_signatures::{
    ProgramNonFunctionSignatureMaps, ProgramSignatureContext, ProgramSignatureResolvers,
    ProgramTraitMethodIndex,
};
use nia_symbol::{SymbolId, SymbolMap};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

mod body_check_flow;
mod body_executable;
mod body_signature_lookup;
mod codegen;
mod const_eval;
mod executable_reachability;
mod extension_providers;
mod frontend;
mod layout_roots;
mod module_checks;
mod program_flow;
mod program_signatures;
mod semantic_inputs;
mod signature_const;

use self::body_check_flow::*;
pub(in crate::query) use self::body_check_flow::{
    BodyCheckResolutionInputs, BodyCheckWithResolutionInputs,
};
use self::body_executable::*;
pub(in crate::query) use self::body_executable::{
    ExecutableValueRefEdges, provide_executable_function_body, provide_executable_static_init,
    provide_executable_value_ref_edges,
};
use self::body_signature_lookup::*;
use self::codegen::*;
pub(in crate::query) use self::codegen::{
    provide_backend_module_source_item_plan, provide_lowered_function_body,
};
use self::const_eval::*;
pub(in crate::query) use self::executable_reachability::{
    provide_executable_checked_module_facts, provide_executable_provider_demands,
};
use self::extension_providers::*;
use self::frontend::*;
use self::layout_roots::*;
use self::module_checks::*;
use self::program_flow::*;
use self::program_signatures::*;
use self::semantic_inputs::*;
use self::signature_const::{
    provide_signature_const_module, signature_const_array_lengths, signature_const_module_lowering,
    signature_const_values, signature_layouts_for_types, with_type_signature_const_input,
};

pub(super) struct QueryPublicSurfaceLookup<'a> {
    db: &'a QueryDb<CompilerContext>,
}

impl<'a> QueryPublicSurfaceLookup<'a> {
    pub(super) fn new(db: &'a QueryDb<CompilerContext>) -> Self {
        Self { db }
    }
}

impl PublicSurfaceLookup for QueryPublicSurfaceLookup<'_> {
    fn public_surface(&self, module_id: ModuleId) -> Option<Arc<ModulePublicSurface>> {
        self.db
            .get(ModulePublicSurfaceQuery(module_id))
            .as_ref()
            .clone()
    }

    fn public_module(&self, module_id: ModuleId, name: &SymbolId) -> Option<ModuleId> {
        let target = self.db.get(PublicSurfaceModuleQuery(module_id, *name));
        target
            .as_ref()
            .as_ref()
            .and_then(|stable_key| self.db.context().module_id_for_stable_key(stable_key))
    }

    fn public_value(&self, module_id: ModuleId, name: &SymbolId) -> Option<nia_defs::PublicItem> {
        self.db
            .get(PublicSurfaceValueQuery(module_id, *name))
            .as_ref()
            .clone()
    }

    fn public_type(&self, module_id: ModuleId, name: &SymbolId) -> Option<nia_defs::PublicItem> {
        self.db
            .get(PublicSurfaceTypeQuery(module_id, *name))
            .as_ref()
            .clone()
    }
}

pub(super) struct QueryUsingScopeLookup<'a> {
    db: &'a QueryDb<CompilerContext>,
    module_id: ModuleId,
}

impl<'a> QueryUsingScopeLookup<'a> {
    pub(super) fn new(db: &'a QueryDb<CompilerContext>, module_id: ModuleId) -> Self {
        Self { db, module_id }
    }
}

impl UsingScopeLookup for QueryUsingScopeLookup<'_> {
    fn using_module(&self, name: &SymbolId) -> Option<ModuleId> {
        let target = self.db.get(UsingScopeModuleQuery(self.module_id, *name));
        target
            .as_ref()
            .as_ref()
            .and_then(|stable_key| self.db.context().module_id_for_stable_key(stable_key))
    }

    fn using_value(&self, name: &SymbolId) -> Option<nia_defs::UsingEntry> {
        self.db
            .get(UsingScopeValueQuery(self.module_id, *name))
            .as_ref()
            .clone()
    }

    fn using_type(&self, name: &SymbolId) -> Option<nia_defs::UsingEntry> {
        self.db
            .get(UsingScopeTypeQuery(self.module_id, *name))
            .as_ref()
            .clone()
    }

    fn has_unresolved_using_name(&self, name: &SymbolId) -> bool {
        *self
            .db
            .get(UsingScopeUnresolvedQuery(self.module_id, *name))
    }
}

pub(super) struct QueryModuleGraphLookup<'a> {
    db: &'a QueryDb<CompilerContext>,
}

impl<'a> QueryModuleGraphLookup<'a> {
    pub(super) fn new(db: &'a QueryDb<CompilerContext>) -> Self {
        Self { db }
    }
}

impl ModuleGraphLookup for QueryModuleGraphLookup<'_> {
    fn entry_module(&self) -> ModuleId {
        let stable_key = self.db.get(ModuleGraphEntryQuery);
        self.db
            .context()
            .module_id_for_stable_key(&stable_key)
            .expect("compiler entry stable key must resolve in current module graph")
    }

    fn package_root_module(&self, package: &SymbolId) -> Option<ModuleId> {
        let root = self.db.get(ModulePackageRootQuery(*package));
        root.as_ref()
            .as_ref()
            .and_then(|stable_key| self.db.context().module_id_for_stable_key(stable_key))
    }

    fn module_path(&self, module_id: ModuleId) -> Option<nia_imports::ModulePath> {
        self.db
            .get(ModuleGraphPathQuery(module_id))
            .as_ref()
            .clone()
    }

    fn parent_module(&self, module_id: ModuleId) -> Option<ModuleId> {
        let parent = self.db.get(ModuleGraphParentQuery(module_id));
        parent
            .as_ref()
            .as_ref()
            .and_then(|stable_key| self.db.context().module_id_for_stable_key(stable_key))
    }

    fn child_declaration(
        &self,
        module_id: ModuleId,
        name: &SymbolId,
    ) -> Option<(ModuleId, nia_ids::Visibility)> {
        let child = self.db.get(ModuleGraphChildQuery(module_id, *name));
        child
            .as_ref()
            .as_ref()
            .and_then(|(stable_key, visibility)| {
                self.db
                    .context()
                    .module_id_for_stable_key(stable_key)
                    .map(|module_id| (module_id, *visibility))
            })
    }
}

#[derive(Clone)]
pub(super) struct CompilerQueryProviders {
    pub(super) checked_program: fn(&QueryDb<CompilerContext>) -> CheckedProgram,
    pub(super) entry_checked_program: fn(&QueryDb<CompilerContext>) -> CheckedProgram,
    pub(super) codegen_program: fn(&QueryDb<CompilerContext>) -> CodegenProgram,
    pub(super) parse_ok_module_ids: fn(&QueryDb<CompilerContext>) -> StableModuleSequence,
    pub(super) semantic_module_ids: fn(&QueryDb<CompilerContext>) -> StableModuleSequence,
    pub(super) module_item_tree: fn(&QueryDb<CompilerContext>, ModuleId) -> ModuleItemTree,
    pub(super) active_module_item_tree:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ActiveModuleItemTree,
    pub(super) full_module_item_tree: fn(&QueryDb<CompilerContext>, ModuleId) -> ModuleItemTree,
    pub(super) full_active_module_item_tree:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ActiveModuleItemTree,
    pub(super) module_defs: fn(&QueryDb<CompilerContext>, ModuleId) -> DefCollection,
    pub(super) full_module_defs: fn(&QueryDb<CompilerContext>, ModuleId) -> DefCollection,
    pub(super) public_surfaces: fn(&QueryDb<CompilerContext>) -> PublicSurfacesValue,
    pub(super) module_public_surface:
        fn(&QueryDb<CompilerContext>, ModuleId) -> Option<Arc<ModulePublicSurface>>,
    pub(super) public_using_scopes: fn(&QueryDb<CompilerContext>) -> PublicUsingScopesValue,
    pub(super) module_using_scope: fn(&QueryDb<CompilerContext>, ModuleId) -> ModuleUsingScope,
    pub(super) type_exposure_index: fn(&QueryDb<CompilerContext>) -> TypeExposureIndexValue,
    pub(super) type_resolution: fn(&QueryDb<CompilerContext>, ModuleId) -> TypeResolution,
    pub(super) declaration_type_resolution:
        fn(&QueryDb<CompilerContext>, ModuleId) -> TypeResolution,
    pub(super) signature_type_resolution:
        fn(&QueryDb<CompilerContext>, ModuleId, nia_item_tree::SignatureItemSet) -> TypeResolution,
    pub(super) signature_const_type_resolution:
        fn(&QueryDb<CompilerContext>, ModuleId) -> TypeResolution,
    pub(super) type_lowering: fn(&QueryDb<CompilerContext>, ModuleId) -> TypeLowering,
    pub(super) declaration_type_lowering: fn(&QueryDb<CompilerContext>, ModuleId) -> TypeLowering,
    pub(super) signature_type_lowering:
        fn(&QueryDb<CompilerContext>, ModuleId, nia_item_tree::SignatureItemSet) -> TypeLowering,
    pub(super) signature_const_type_lowering:
        fn(&QueryDb<CompilerContext>, ModuleId) -> TypeLowering,
    pub(super) item_signatures: fn(&QueryDb<CompilerContext>, ModuleId) -> ItemSignatures,
    pub(super) signature_item_signatures:
        fn(&QueryDb<CompilerContext>, ModuleId, nia_item_tree::SignatureItemSet) -> ItemSignatures,
    pub(super) signature_const_item_signatures:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ItemSignatures,
    pub(super) type_normalization: fn(&QueryDb<CompilerContext>, ModuleId) -> TypeNormalization,
    pub(super) layout_type_normalization:
        fn(&QueryDb<CompilerContext>, ModuleId) -> TypeNormalization,
    pub(super) signature_type_normalization: fn(
        &QueryDb<CompilerContext>,
        ModuleId,
        nia_item_tree::SignatureItemSet,
    ) -> TypeNormalization,
    pub(super) signature_const_type_normalization:
        fn(&QueryDb<CompilerContext>, ModuleId) -> TypeNormalization,
    pub(super) signature_const_module:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ConstModuleLowering,
    pub(super) program_signature_module_ids:
        fn(&QueryDb<CompilerContext>, nia_item_tree::SignatureItemSet) -> StableModuleSequence,
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
    pub(super) extension_provider_module_ids: fn(&QueryDb<CompilerContext>) -> StableModuleSequence,
    pub(super) extension_provider_module_eligibility:
        fn(&QueryDb<CompilerContext>, ModuleId) -> bool,
    pub(super) extension_signature_module_input:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ExtensionSignatureModuleInputValue,
    pub(super) extension_trait_solving_module_facts:
        fn(&QueryDb<CompilerContext>, ModuleId) -> ExtensionTraitSolvingModuleFactsValue,
    pub(super) extension_trait_impls_for_trait:
        fn(&QueryDb<CompilerContext>, nia_ty::TraitId) -> ExtensionTraitImplsForTraitValue,
    pub(super) program_trait_method_index: fn(&QueryDb<CompilerContext>) -> ProgramTraitMethodIndex,
    pub(super) program_abi_signatures: fn(&QueryDb<CompilerContext>) -> ProgramAbiSignaturesValue,
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
    pub(super) extension_methods_named:
        fn(&QueryDb<CompilerContext>, SymbolId) -> ExtensionMethodsNamedValue,
    pub(super) extension_method_by_id:
        fn(&QueryDb<CompilerContext>, GlobalDefId) -> ExtensionMethodByIdValue,
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
    pub(super) const_module: fn(&QueryDb<CompilerContext>, ModuleId) -> ConstModuleLowering,
    pub(super) const_eval: fn(&QueryDb<CompilerContext>, ModuleId) -> ConstCheck,
    pub(super) const_array_lengths:
        fn(&QueryDb<CompilerContext>, ModuleId) -> nia_const_check::ConstArrayLengths,
    pub(super) const_enum_values:
        fn(&QueryDb<CompilerContext>, ModuleId) -> nia_const_check::ConstEnumValues,
    pub(super) const_values:
        fn(&QueryDb<CompilerContext>, ModuleId) -> nia_const_check::ConstValues,
    pub(super) const_typed_facts:
        fn(&QueryDb<CompilerContext>, ModuleId) -> nia_const_check::ConstTypedFacts,
    pub(super) layouts: fn(&QueryDb<CompilerContext>, ModuleId) -> nia_layout::Layouts,
    pub(super) signature_layouts: fn(&QueryDb<CompilerContext>, ModuleId) -> nia_layout::Layouts,
    pub(super) abi_check: fn(&QueryDb<CompilerContext>, ModuleId) -> nia_abi_check::AbiCheck,
    pub(super) static_check:
        fn(&QueryDb<CompilerContext>, ModuleId) -> nia_static_check::StaticCheck,
    pub(super) flow_check: fn(&QueryDb<CompilerContext>, ModuleId) -> nia_flow_check::FlowCheck,
    pub(super) body_check: fn(&QueryDb<CompilerContext>, ModuleId) -> nia_body_check::BodyCheck,
    pub(super) checked_module: fn(&QueryDb<CompilerContext>, ModuleId) -> CheckedModule,
    pub(super) checked_module_ids: fn(&QueryDb<CompilerContext>) -> Vec<ModuleId>,
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
            parse_ok_module_ids: provide_parse_ok_module_ids,
            semantic_module_ids: provide_semantic_module_ids,
            module_item_tree: provide_module_item_tree,
            active_module_item_tree: provide_active_module_item_tree,
            full_module_item_tree: provide_full_module_item_tree,
            full_active_module_item_tree: provide_full_active_module_item_tree,
            module_defs: provide_module_defs,
            full_module_defs: provide_full_module_defs,
            public_surfaces: provide_public_surfaces,
            module_public_surface: provide_module_public_surface,
            public_using_scopes: provide_public_using_scopes,
            module_using_scope: provide_module_using_scope,
            type_exposure_index: provide_type_exposure_index,
            type_resolution: provide_type_resolution,
            declaration_type_resolution: provide_declaration_type_resolution,
            signature_type_resolution: provide_signature_type_resolution,
            signature_const_type_resolution: provide_signature_const_type_resolution,
            type_lowering: provide_type_lowering,
            declaration_type_lowering: provide_declaration_type_lowering,
            signature_type_lowering: provide_signature_type_lowering,
            signature_const_type_lowering: provide_signature_const_type_lowering,
            item_signatures: provide_item_signatures,
            signature_item_signatures: provide_signature_item_signatures,
            signature_const_item_signatures: provide_signature_const_item_signatures,
            type_normalization: provide_type_normalization,
            layout_type_normalization: provide_layout_type_normalization,
            signature_type_normalization: provide_signature_type_normalization,
            signature_const_type_normalization: provide_signature_const_type_normalization,
            signature_const_module: provide_signature_const_module,
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
            extension_trait_impls_for_trait: provide_extension_trait_impls_for_trait,
            program_trait_method_index: provide_program_trait_method_index,
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
            extension_methods_named: provide_extension_methods_named,
            extension_method_by_id: provide_extension_method_by_id,
            extension_trait_signature_index: provide_extension_trait_signature_index,
            visible_extensions: provide_visible_extensions,
            visible_trait_impls: provide_visible_trait_impls,
            value_resolution: provide_value_resolution,
            local_resolution: provide_local_resolution,
            semantic_use_table: provide_semantic_use_table,
            const_module: provide_const_module,
            const_eval: provide_const,
            const_array_lengths: provide_const_array_lengths,
            const_enum_values: provide_const_enum_values,
            const_values: provide_const_values,
            const_typed_facts: provide_const_typed_facts,
            layouts: provide_layouts,
            signature_layouts: provide_signature_layouts,
            abi_check: provide_abi_check,
            static_check: provide_static_check,
            flow_check: provide_flow_check,
            body_check: provide_body_check,
            checked_module: provide_checked_module,
            checked_module_ids: provide_checked_module_ids,
            monomorphization: provide_monomorphization,
            backend_lowering: provide_backend_lowering,
        }
    }
}

fn empty_monomorphization() -> nia_monomorphize::Monomorphization {
    nia_monomorphize::Monomorphization {
        instances: Vec::new(),
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
