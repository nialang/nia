// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_defs::{DefId, DefKind};
use nia_executable_reachability::{
    ExecutableExtensionLookup, ExecutableRootDefs, IncrementalExecutableReachability,
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

mod body_check_flow;
mod body_executable;
mod body_signature_lookup;
mod codegen;
mod comptime;
mod executable_reachability;
mod extension_providers;
mod frontend;
mod layout_roots;
mod module_checks;
mod program_flow;
mod program_signatures;
mod semantic_inputs;
mod signature_comptime;

use self::body_check_flow::*;
pub(in crate::query) use self::body_check_flow::{
    BodyCheckResolutionInputs, BodyCheckWithResolutionInputs,
};
use self::body_executable::*;
use self::body_signature_lookup::*;
use self::codegen::*;
use self::comptime::*;
pub(in crate::query) use self::executable_reachability::provide_executable_checked_module_set;
#[cfg(test)]
pub(in crate::query) use self::executable_reachability::provide_executable_checked_modules;
use self::extension_providers::*;
use self::frontend::*;
use self::layout_roots::*;
use self::module_checks::*;
use self::program_flow::*;
use self::program_signatures::*;
use self::semantic_inputs::*;
use self::signature_comptime::{
    provide_signature_comptime_module, signature_comptime_array_lengths,
    signature_comptime_module_lowering, signature_comptime_values, signature_layouts_for_types,
    with_type_signature_comptime_input,
};

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
    pub(super) extension_trait_impls_for_trait:
        fn(&QueryDb<CompilerContext>, nia_ty::TraitId) -> ExtensionTraitImplsForTraitValue,
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
            extension_trait_impls_for_trait: provide_extension_trait_impls_for_trait,
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
            extension_methods_named: provide_extension_methods_named,
            extension_method_by_id: provide_extension_method_by_id,
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
