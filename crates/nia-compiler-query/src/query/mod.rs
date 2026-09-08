// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    ActiveModuleItemTreeFactKind, CheckedModule, CheckedProgram, CheckedProgramAnalysis,
    CodegenPreparation, CodegenProgram, FrontendCheckInputFingerprint, FrontendCheckScope,
    ProgramDiagnostic, ProgramDiagnosticBundles, RuntimeModel, TimingMode, module_diagnostics,
};
#[cfg(test)]
use crate::{LoadedModule, LoadedProgram};
use nia_backend_lower::BackendLowerModuleInput;
use nia_const_check::{ConstCheck, ConstModuleLowering};
use nia_defs::{
    DefCollection, ModulePublicSurface, ModuleUsingScope, PublicSurfaceLookup,
    PublicSurfaceModuleFacts, PublicSurfaces, UsingScopeLookup,
};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{DefId, GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
#[cfg(test)]
use nia_imports::ModuleGraph;
use nia_imports::{ModuleGraphLookup, ModuleGraphSnapshot, StableModuleKey};
use nia_item_signatures::{
    ItemSignatures, ProgramConstSignature, ProgramEnumSignature, ProgramFunctionSignature,
    ProgramGlobalSignature, ProgramStructSignature, ProgramTraitSignature,
    ProgramTypeAliasSignature, ProgramUnionSignature, StructSignature, UnionSignature,
};
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_local_resolve::LocalResolution;
use nia_monomorphize::MonomorphizeModuleInput;
use nia_node_id::NodeOriginTable;
use nia_opt::{NiaOptimizationLevel, OptimizationPolicy};
use nia_package_metadata::{
    DefinitionId, InterfaceRecord, InterfaceSection, ModuleId as StableModuleId, ModuleInterface,
    PackageId, PackageManifest, SectionKind, StableConstArg, StableDeclaration, StableTypeGraph,
    StableTypeNode,
};
use nia_parser::ParseError;
use nia_program_signatures::{
    ExtensionMethodIndexModuleInput, ExtensionMethodValidationInput, ExtensionModuleInput,
    ExtensionTraitSignatureIndex, ModuleProgramSignatureFacts, ModuleSignatureInput,
    VisibleExtensionsForModule, VisibleExtensionsInput, VisibleTraitImplsForModule,
    VisibleTypeSignatures, collect_extension_associated_value_index_for_module,
    collect_extension_method_diagnostics_for_module, collect_extension_method_index_for_module,
    collect_nominal_extension_providers_for_module, visible_extensions_for_module,
    visible_trait_impls_for_module,
};
use nia_public_surface::{
    TypeExposureIndex, compute_exported_public_surfaces_with_symbols,
    compute_using_scopes_from_surfaces_with_symbols,
};
use nia_query::{
    FingerprintDomain, QueryDb, QueryError, QueryFingerprint, QueryFingerprintBuilder,
    QueryFingerprintPolicy, QueryFrame, QueryKey, QueryProviderPolicy, QueryResult,
    QueryStoragePolicy, QueryTrace,
};
use nia_source::{SourcePath, SourceVersion};
use nia_span::Span;
use nia_symbol::SymbolId;
use nia_target_config::TargetConfig;
use nia_ty::{ArrayLenTy, TyKind};
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_type_resolve::TypeResolution;
use nia_value_resolve::ValueResolution;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, RwLock},
};

/// Public compiler-facing name for the canonical metadata module identity.
pub type StableModuleIdentity = StableModuleId;

mod backend_lowering;
mod base;
mod checked;
mod checks;
mod context;
mod diagnostics;
mod executable;
mod extension_provider_queries;
mod function_body_queries;
mod program;
mod program_signature_queries;
mod providers;
mod registry;

const PROVIDER_FACT_WORKLIST_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.provider-fact-worklist.v1");
const PROVIDER_FACT_REVISION_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.provider-fact-revision.v1");
const PROVIDER_DEMAND_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.provider-demand.v1");
const CHECK_CERTIFICATE_INPUT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.check-certificate-input.v1");
const MODULE_GRAPH_PATH_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-graph-path.v1");
const MODULE_GRAPH_ENTRY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-graph-entry.v1");
const MODULE_GRAPH_PARENT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-graph-parent.v1");
const MODULE_GRAPH_CHILD_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-graph-child.v1");
const MODULE_PACKAGE_ROOT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-package-root.v1");
const LOADED_MODULES_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.loaded-modules.v1");
const PARSE_OK_MODULE_IDS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.parse-ok-module-ids.v1");
const SEMANTIC_MODULE_IDS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.semantic-module-ids.v1");
const MODULE_SOURCE_PATH_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-source-path.v1");
const MODULE_SOURCE_VERSION_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-source-version.v1");
const PUBLIC_SURFACE_MODULE_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.public-surface-module.v1");
const USING_SCOPE_MODULE_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.using-scope-module.v1");
const PROGRAM_SIGNATURE_MODULE_IDS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.program-signature-module-ids.v1");
const PROGRAM_SIGNATURE_MODULE_ELIGIBILITY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.program-signature-module-eligibility.v1");
const EXTENSION_PROVIDER_MODULE_IDS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.extension-provider-module-ids.v1");
const EXTENSION_PROVIDER_MODULE_ELIGIBILITY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.extension-provider-module-eligibility.v1");
const PROVIDER_SUMMARY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.provider-summary.v1");
const COMPILED_INTERFACE_INDEX_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.compiled-interface-index.v1");
mod resolve;
mod static_init_queries;
mod types;

use backend_lowering::*;
use base::*;
use checked::*;
use checks::*;
use context::*;
use diagnostics::*;
use executable::*;
use extension_provider_queries::*;
use function_body_queries::*;
use program::*;
use program_signature_queries::*;
use providers::*;
use registry::*;
use resolve::*;
use static_init_queries::*;
use types::*;

type ExtensionProviderModuleFactsValue = ExtensionProviderModuleFactsQueryValue;
type ExtensionProviderValidationFactsValue = ExtensionProviderValidationFactsQueryValue;
type ExtensionProviderNominalModuleFactsValue = ExtensionProviderNominalModuleFactsQueryValue;
type ExtensionProviderDiscoveryIndexValue = ExtensionProviderDiscoveryIndexQueryValue;
type ExtensionProviderNominalCandidateModulesValue =
    ExtensionProviderNominalCandidateModulesQueryValue;
type ExtensionProviderNominalModulesForTargetsValue =
    ExtensionProviderNominalModulesForTargetsQueryValue;
type TypeExposureIndexValue = TypeExposureIndex;
type ExtensionMethodIndexValue = ExtensionMethodIndexQueryValue;
type ExtensionMethodsNamedValue = ExtensionMethodsNamedQueryValue;
type ExtensionMethodByIdValue = ExtensionMethodByIdQueryValue;
type ExtensionTraitSignatureIndexValue = ExtensionTraitSignatureIndex;
type VisibleExtensionsValue = VisibleExtensionsForModule;
type VisibleTraitImplsValue = VisibleTraitImplsForModule;

/// Resolves a session-local definition to its relocation-independent package.
///
/// Implementations are owned by the compiler/loader boundary and may consult
/// package manifests or an installed identity index. The resolver must not
/// infer ownership from declaration spelling or physical source paths.
pub trait StableDefinitionPackageResolver {
    fn package_for_definition(&self, def_id: GlobalDefId) -> QueryResult<PackageId>;
}

/// Resolves a session module to its relocation-independent package identity.
pub trait StableModulePackageResolver {
    fn package_for_module(&self, module_id: ModuleId) -> QueryResult<PackageId>;
}

impl<F> StableModulePackageResolver for F
where
    F: Fn(ModuleId) -> QueryResult<PackageId>,
{
    fn package_for_module(&self, module_id: ModuleId) -> QueryResult<PackageId> {
        self(module_id)
    }
}

/// Session-local remap table for stable package module identities.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StableModuleIndex {
    modules: BTreeMap<StableModuleId, ModuleId>,
}

impl StableModuleIndex {
    /// Creates an empty module remap table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts one identity, returning the previous handle when present.
    pub fn insert(&mut self, identity: StableModuleId, module: ModuleId) -> Option<ModuleId> {
        self.modules.insert(identity, module)
    }

    /// Resolves a stable module identity to a current session handle.
    pub fn module(&self, identity: &StableModuleId) -> Option<ModuleId> {
        self.modules.get(identity).copied()
    }

    /// Returns the number of remapped modules.
    pub fn len(&self) -> usize {
        self.modules.len()
    }

    /// Reports whether no modules are installed.
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
}

/// Resolves a published definition identity into the current compiler
/// session. Implementations are responsible for remapping package/module
/// identities; no physical path lookup is implied by this trait.
pub trait StableDefinitionResolver {
    fn definition_for_identity(&self, definition: &DefinitionId) -> QueryResult<GlobalDefId>;
}

/// Session-local remap table for stable package definition identities.
///
/// The table is built from the current module/definition facts and an
/// explicit package resolver. It is the only supported bridge from immutable
/// artifact identities to transient `ModuleId`/`DefId` handles.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StableDefinitionIndex {
    definitions: BTreeMap<DefinitionId, GlobalDefId>,
    modules: StableModuleIndex,
}

impl StableDefinitionIndex {
    pub fn definition(&self, identity: &DefinitionId) -> Option<GlobalDefId> {
        self.definitions.get(identity).copied()
    }

    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    /// Resolves a published module identity to its current session handle.
    pub fn module(&self, identity: &StableModuleId) -> Option<ModuleId> {
        self.modules.module(identity)
    }

    /// Returns the number of remapped public modules.
    pub fn module_len(&self) -> usize {
        self.modules.len()
    }
}

impl StableDefinitionResolver for StableDefinitionIndex {
    fn definition_for_identity(&self, definition: &DefinitionId) -> QueryResult<GlobalDefId> {
        self.definition(definition)
            .ok_or_else(|| QueryError::InvalidInput {
                query: QueryFrame {
                    name: "stable_definition_index",
                    key: "StableDefinitionIndex".to_string(),
                    description: "stable_definition_index".to_string(),
                },
                message: format!(
                    "compiled definition is not present in the current session: {definition:?}"
                ),
            })
    }
}

impl<F> StableDefinitionResolver for F
where
    F: Fn(&DefinitionId) -> QueryResult<GlobalDefId>,
{
    fn definition_for_identity(&self, definition: &DefinitionId) -> QueryResult<GlobalDefId> {
        self(definition)
    }
}

/// Compiler-owned index of validated compiled package interfaces.
///
/// The index deliberately retains stable metadata identities. It does not
/// manufacture session-local module or definition handles; provider
/// installation must perform that remapping explicitly at a later boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledPackageInterfaceIndex {
    packages: BTreeMap<PackageId, nia_package_metadata::CompiledPackageInterface>,
    definitions: BTreeMap<DefinitionId, (PackageId, usize)>,
}

/// Decoded declaration facts for one selected compiled package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledPackageDeclarations {
    package: PackageId,
    declarations: BTreeMap<DefinitionId, StableDeclaration>,
}

/// Validated public interface records belonging to one canonical package
/// module. This is the first artifact-backed module fact consumed by the
/// compiler query graph; it deliberately carries no source or session ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledPackageModuleInterface {
    identity: StableModuleId,
    interface_hash: [u8; 32],
    records: Vec<InterfaceRecord>,
}

/// Checked downstream templates selected from one compiled package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledPackageTemplates {
    package: PackageId,
    records: BTreeMap<DefinitionId, nia_package_metadata::TemplateRecord>,
}

impl CompiledPackageTemplates {
    pub fn package(&self) -> &PackageId {
        &self.package
    }

    pub fn get(&self, definition: &DefinitionId) -> Option<&nia_package_metadata::TemplateRecord> {
        self.records.get(definition)
    }

    pub fn iter(
        &self,
    ) -> impl Iterator<Item = (&DefinitionId, &nia_package_metadata::TemplateRecord)> {
        self.records.iter()
    }
}

impl CompiledPackageModuleInterface {
    pub fn identity(&self) -> &StableModuleId {
        &self.identity
    }

    pub fn interface_hash(&self) -> [u8; 32] {
        self.interface_hash
    }

    pub fn records(&self) -> &[InterfaceRecord] {
        &self.records
    }
}

impl CompiledPackageDeclarations {
    pub fn package(&self) -> &PackageId {
        &self.package
    }

    pub fn declaration(&self, definition: &DefinitionId) -> Option<&StableDeclaration> {
        self.declarations.get(definition)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&DefinitionId, &StableDeclaration)> {
        self.declarations.iter()
    }
}

impl CompiledPackageInterfaceIndex {
    pub fn from_interfaces(
        interfaces: Vec<nia_package_metadata::CompiledPackageInterface>,
    ) -> Result<Self, String> {
        let mut packages = BTreeMap::new();
        let mut definitions = BTreeMap::new();
        for interface in interfaces {
            let package = interface.manifest().package.clone();
            if packages.insert(package.clone(), interface).is_some() {
                return Err("duplicate compiled interface for one package".to_string());
            }
        }
        for (package, interface) in &packages {
            for (index, record) in interface.records().iter().enumerate() {
                if definitions
                    .insert(record.definition.clone(), (package.clone(), index))
                    .is_some()
                {
                    return Err("duplicate compiled definition identity".to_string());
                }
            }
        }
        Ok(Self {
            packages,
            definitions,
        })
    }

    /// Returns all selected package interfaces in stable package order.
    pub fn packages(
        &self,
    ) -> impl Iterator<Item = (&PackageId, &nia_package_metadata::CompiledPackageInterface)> {
        self.packages.iter()
    }

    /// Looks up one package interface by stable package identity.
    pub fn package(
        &self,
        package: &PackageId,
    ) -> Option<&nia_package_metadata::CompiledPackageInterface> {
        self.packages.get(package)
    }

    /// Resolves one stable definition without loading dependency source.
    pub fn definition(
        &self,
        definition: &DefinitionId,
    ) -> Option<&nia_package_metadata::InterfaceRecord> {
        let (package, index) = self.definitions.get(definition)?;
        self.packages.get(package)?.records().get(*index)
    }

    /// Returns one package's declarations for a stable module path.
    pub fn module_records(
        &self,
        package: &PackageId,
        module: &str,
    ) -> Option<Vec<&nia_package_metadata::InterfaceRecord>> {
        Some(self.packages.get(package)?.module_records(module).collect())
    }

    /// Looks up one package-qualified module manifest record.
    pub fn module(
        &self,
        identity: &nia_package_metadata::ModuleId,
    ) -> Option<&nia_package_metadata::ModuleInterface> {
        self.packages.get(&identity.package)?.module(identity)
    }
}

impl<F> StableDefinitionPackageResolver for F
where
    F: Fn(GlobalDefId) -> QueryResult<PackageId>,
{
    fn package_for_definition(&self, def_id: GlobalDefId) -> QueryResult<PackageId> {
        self(def_id)
    }
}
type ExtensionSignatureModuleInputValue = ExtensionSignatureModuleInputQueryValue;
type ExtensionTraitSolvingModuleFactsValue = ExtensionTraitSolvingModuleFactsQueryValue;
type ExtensionTraitImplsForTraitValue = ExtensionTraitImplsForTraitQueryValue;
type ModuleProgramSignatureFactsValue = ModuleProgramSignatureFacts;
type ModuleAbiSignatureFactsValue = ModuleAbiSignatureFactsQueryValue;
type PublicSurfacesValue = PublicSurfacesQueryValue;
type PublicUsingScopesValue = PublicUsingScopesQueryValue;

/// Loader facts and session-stable policies used to create or update a compiler database.
#[derive(Clone)]
pub struct CompileRequest {
    loader_facts: Arc<dyn crate::LoaderFactProvider>,
    /// Optimization level contributing to executable query products.
    pub optimization: NiaOptimizationLevel,
    /// Compiler timing collection policy.
    pub timings: TimingMode,
    frontend_cache_dir: Option<PathBuf>,
    verify_frontend_cache: bool,
}

impl CompileRequest {
    /// Creates a request from a loader fact provider with caching disabled.
    pub fn new(loader_facts: impl crate::LoaderFactProvider + 'static) -> Self {
        let loader_facts: Arc<dyn crate::LoaderFactProvider> = Arc::new(loader_facts);
        Self {
            loader_facts,
            optimization: NiaOptimizationLevel::default(),
            timings: TimingMode::Off,
            frontend_cache_dir: None,
            verify_frontend_cache: false,
        }
    }

    /// Selects the executable optimization level.
    pub fn with_optimization(mut self, optimization: NiaOptimizationLevel) -> Self {
        self.optimization = optimization;
        self
    }

    /// Selects compiler timing collection.
    pub fn with_timings(mut self, timings: TimingMode) -> Self {
        self.timings = timings;
        self
    }

    /// Selects the persistent frontend cache root for this query session.
    pub fn with_frontend_cache_dir(mut self, frontend_cache_dir: Option<PathBuf>) -> Self {
        self.frontend_cache_dir = frontend_cache_dir;
        self
    }

    /// Enables recomputation and comparison of otherwise reusable frontend entries.
    pub fn with_frontend_cache_verification(mut self, verify: bool) -> Self {
        self.verify_frontend_cache = verify;
        self
    }

    #[cfg(test)]
    fn with_loader_facts(mut self, loader_facts: impl crate::LoaderFactProvider + 'static) -> Self {
        self.loader_facts = Arc::new(loader_facts);
        self
    }
}

/// Incremental compiler database bound to one loader/query session.
#[derive(Clone)]
pub struct CompilerDatabase {
    db: QueryDb<CompilerContext>,
    inputs: Arc<RwLock<CompilerInputs>>,
}

impl CompilerDatabase {
    /// Creates a compiler database and registers the complete query provider graph.
    pub fn new(request: CompileRequest) -> Self {
        compiler_database_with_providers(request, CompilerQueryProviders::default())
    }

    /// Returns the shared session that owns this database and its loader facts.
    pub fn query_session(&self) -> nia_query::QuerySession {
        self.db.session()
    }

    /// Returns the loader-selected package interfaces without materializing
    /// dependency source text or creating session-local handles.
    pub fn compiled_package_interfaces(
        &self,
    ) -> QueryResult<Vec<nia_package_metadata::CompiledPackageInterface>> {
        self.db
            .context()
            .loader_facts()
            .compiled_package_interfaces()
    }

    /// Returns selected artifact module identities without source loading.
    pub fn compiled_package_module_identities(
        &self,
    ) -> QueryResult<Vec<nia_package_metadata::ModuleId>> {
        self.db
            .context()
            .loader_facts()
            .compiled_package_module_identities()
    }

    /// Publishes one source-free module interface fact for every selected
    /// compiled package module. Publication is tied to the validated package
    /// interface index, so replacing or retiring an artifact invalidates all
    /// module facts atomically.
    pub fn install_compiled_package_module_interfaces(
        &self,
    ) -> QueryResult<Vec<nia_package_metadata::ModuleId>> {
        let index = self.compiled_package_interface_index()?;
        let mut identities = Vec::new();
        for (package, interface) in index.packages() {
            for identity in interface.module_identities() {
                let module = interface.module(&identity).ok_or_else(|| {
                    self.db.invalid_input(
                        &CompiledPackageInterfaceIndexQuery,
                        format!(
                            "compiled module identity is absent from package manifest: {identity:?}"
                        ),
                    )
                })?;
                let records = interface
                    .module_records(&identity.path)
                    .cloned()
                    .collect::<Vec<_>>();
                let expected_hash = nia_package_metadata::interface_module_hash(
                    &InterfaceSection {
                        records: records.clone(),
                    },
                    &identity.path,
                )
                .map_err(|error| {
                    self.db
                        .invalid_input(&CompiledPackageInterfaceIndexQuery, error.to_string())
                })?;
                if expected_hash != module.interface_hash || identity.package != *package {
                    return Err(self.db.invalid_input(
                        &CompiledPackageInterfaceIndexQuery,
                        format!("compiled module interface hash or package identity mismatch: {identity:?}"),
                    ));
                }
                if !self
                    .db
                    .can_publish_owned(CompiledPackageModuleInterfaceQuery(identity.clone()))
                {
                    identities.push(identity);
                    continue;
                }
                self.db.publish_owned(
                    CompiledPackageModuleInterfaceQuery(identity.clone()),
                    CompiledPackageModuleInterface {
                        identity: identity.clone(),
                        interface_hash: module.interface_hash,
                        records,
                    },
                    &CompiledPackageInterfaceIndexQuery,
                );
                identities.push(identity);
            }
        }
        Ok(identities)
    }

    /// Consumes one source-free compiled module interface fact.
    pub fn compiled_package_module_interface(
        &self,
        identity: nia_package_metadata::ModuleId,
    ) -> QueryResult<CompiledPackageModuleInterface> {
        self.db
            .get_owned(CompiledPackageModuleInterfaceQuery(identity))
    }

    /// Publishes validated generic/const templates for selected packages.
    pub fn install_compiled_package_templates(&self) -> QueryResult<Vec<PackageId>> {
        let index = self.compiled_package_interface_index()?;
        let mut installed = Vec::new();
        for (package, interface) in index.packages() {
            let mut records = BTreeMap::new();
            if let Some(templates) = interface.templates() {
                for record in &templates.records {
                    if record.definition.module.package != *package
                        || records
                            .insert(record.definition.clone(), record.clone())
                            .is_some()
                    {
                        return Err(self.db.invalid_input(
                            &CompiledPackageInterfaceIndexQuery,
                            format!(
                                "compiled template identity is inconsistent: {:?}",
                                record.definition
                            ),
                        ));
                    }
                }
            }
            if !self
                .db
                .can_publish_owned(CompiledPackageTemplatesQuery(package.clone()))
            {
                installed.push(package.clone());
                continue;
            }
            self.db.publish_owned(
                CompiledPackageTemplatesQuery(package.clone()),
                CompiledPackageTemplates {
                    package: package.clone(),
                    records,
                },
                &CompiledPackageInterfaceIndexQuery,
            );
            installed.push(package.clone());
        }
        Ok(installed)
    }

    /// Consumes the checked template inventory for one package.
    pub fn compiled_package_templates(
        &self,
        package: PackageId,
    ) -> QueryResult<CompiledPackageTemplates> {
        self.db.get_owned(CompiledPackageTemplatesQuery(package))
    }

    /// Returns the query-tracked index of selected compiled interfaces.
    pub fn compiled_package_interface_index(&self) -> QueryResult<CompiledPackageInterfaceIndex> {
        self.db
            .get(CompiledPackageInterfaceIndexQuery)
            .map(Arc::unwrap_or_clone)
    }

    /// Builds the session remap table for all loaded definitions. Package
    /// ownership is supplied by the caller; it must not be inferred from
    /// source paths or declaration names.
    pub fn stable_definition_index(
        &self,
        resolver: &dyn StableDefinitionPackageResolver,
    ) -> QueryResult<StableDefinitionIndex> {
        let graph = self.db.get(ModuleGraphQuery)?;
        let symbols = self.db.context().loader_facts().symbols();
        let mut definitions = BTreeMap::new();
        let mut modules = StableModuleIndex::new();
        let mut module_owners = HashMap::<ModuleId, PackageId>::new();
        for module in graph.modules() {
            let Some(stable_key) = graph.stable_key(module.id) else {
                continue;
            };
            let module_path = stable_key.source_identity().normalized_path().to_owned();
            let facts = self.db.get(FullModuleDefsQuery(module.id))?;
            for (def_id, def) in facts.semantic.defs.iter() {
                // Artifact identities describe the public package surface;
                // private members and nested definitions are not valid
                // cross-package remap targets and must not force the package
                // resolver to classify them.
                if def.parent.is_some() || def.visibility != nia_defs::Visibility::Public {
                    continue;
                }
                let Some(name) = symbols.resolve(def.name) else {
                    continue;
                };
                let identity = DefinitionId {
                    module: StableModuleId {
                        package: resolver.package_for_definition(GlobalDefId {
                            module_id: module.id,
                            def_id,
                        })?,
                        path: module_path.clone(),
                    },
                    name: name.to_string(),
                    kind: def_kind_tag(def.kind),
                };
                let global = GlobalDefId {
                    module_id: module.id,
                    def_id,
                };
                let module_identity = identity.module.clone();
                if let Some(previous_package) =
                    module_owners.insert(module.id, identity.module.package.clone())
                {
                    if previous_package != identity.module.package {
                        return Err(self.db.invalid_input(
                            &ModuleGraphQuery,
                            format!(
                                "stable module identity resolves to multiple packages: module {:?}, packages {:?} and {:?}",
                                module.id, previous_package, identity.module.package
                            ),
                        ));
                    }
                }
                if let Some(previous) = modules.insert(module_identity.clone(), module.id) {
                    if previous != module.id {
                        return Err(self.db.invalid_input(
                            &ModuleGraphQuery,
                            format!(
                                "stable module identity resolves to multiple session modules: {:?}",
                                module_identity
                            ),
                        ));
                    }
                }
                if definitions.insert(identity, global).is_some() {
                    return Err(self.db.invalid_input(
                        &ModuleGraphQuery,
                        "duplicate stable definition identity in current session".to_string(),
                    ));
                }
            }
        }
        Ok(StableDefinitionIndex {
            definitions,
            modules,
        })
    }

    /// Builds a stable package/module remap table for every loaded module.
    ///
    /// Package ownership is supplied explicitly by the loader boundary. The
    /// compiler never infers it from source paths, package-root symbols, or
    /// declaration names. Duplicate stable identities are rejected instead of
    /// silently selecting one session handle.
    pub fn stable_module_index(
        &self,
        resolver: &dyn StableModulePackageResolver,
    ) -> QueryResult<StableModuleIndex> {
        let graph = self.db.get(ModuleGraphQuery)?;
        let mut index = StableModuleIndex::new();
        for module in graph.modules() {
            let Some(stable_key) = graph.stable_key(module.id) else {
                continue;
            };
            let identity = StableModuleId {
                package: resolver.package_for_module(module.id)?,
                path: stable_key.source_identity().normalized_path().to_owned(),
            };
            if let Some(previous) = index.insert(identity.clone(), module.id)
                && previous != module.id
            {
                return Err(self.db.invalid_input(
                    &ModuleGraphQuery,
                    format!(
                        "stable module identity resolves to multiple session modules: {:?}",
                        identity
                    ),
                ));
            }
        }
        Ok(index)
    }

    /// Rehydrates a stable package type graph into this database's canonical
    /// type store using an explicit definition identity resolver.
    pub fn rehydrate_stable_type_graph(
        &self,
        graph: &StableTypeGraph,
        resolver: &dyn StableDefinitionResolver,
    ) -> QueryResult<Vec<InternedTyId>> {
        graph
            .validate()
            .map_err(|error| self.db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
        let entry = self.db.get(ModuleGraphQuery)?.entry();
        let append = self.db.context().type_store.append_for_module(entry);
        let mut types = Vec::with_capacity(graph.nodes.len());
        for node in &graph.nodes {
            let ty = match node {
                StableTypeNode::Primitive(tag) => stable_primitive_from_tag(*tag)
                    .map(|primitive| append.primitive(primitive))
                    .ok_or_else(|| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            "unknown stable primitive tag".to_string(),
                        )
                    })?,
                StableTypeNode::Named(definition) => append.intern(nia_ty::TyKind::Nominal {
                    def_id: resolver.definition_for_identity(definition)?,
                    args: Vec::new(),
                    const_args: Vec::new(),
                }),
                StableTypeNode::NamedApplied {
                    definition,
                    arguments,
                    const_arguments,
                } => append.intern(nia_ty::TyKind::Nominal {
                    def_id: resolver.definition_for_identity(definition)?,
                    args: arguments
                        .iter()
                        .map(|index| types[usize::try_from(*index).unwrap()])
                        .collect(),
                    const_args: const_arguments
                        .iter()
                        .map(|argument| match argument {
                            StableConstArg::GenericParam(hash) => Ok(nia_ty::ConstGenericArg {
                                ty: append.primitive(nia_ty::PrimitiveTy::Usize),
                                value: nia_ty::ConstGenericValue::GenericParam(
                                    SymbolId::from_stable_hash(*hash),
                                ),
                            }),
                            StableConstArg::Integer { bits, signed } => {
                                Ok(nia_ty::ConstGenericArg {
                                    ty: append.primitive(if *signed {
                                        nia_ty::PrimitiveTy::I128
                                    } else {
                                        nia_ty::PrimitiveTy::U128
                                    }),
                                    value: nia_ty::ConstGenericValue::Int(if *signed {
                                        nia_ty::IntConst::signed_bits(*bits)
                                    } else {
                                        nia_ty::IntConst::unsigned(*bits)
                                    }),
                                })
                            }
                            StableConstArg::Bool(value) => Ok(nia_ty::ConstGenericArg {
                                ty: append.primitive(nia_ty::PrimitiveTy::Bool),
                                value: nia_ty::ConstGenericValue::Bool(*value),
                            }),
                            StableConstArg::Char(value) => Ok(nia_ty::ConstGenericArg {
                                ty: append.primitive(nia_ty::PrimitiveTy::Char),
                                value: nia_ty::ConstGenericValue::Char(*value),
                            }),
                        })
                        .collect::<QueryResult<Vec<_>>>()?,
                }),
                StableTypeNode::Unit => append.intern(nia_ty::TyKind::Tuple(Vec::new())),
                StableTypeNode::Never => {
                    append.intern(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::Never))
                }
                StableTypeNode::Tuple(elements) => append.intern(nia_ty::TyKind::Tuple(
                    elements
                        .iter()
                        .map(|index| types[usize::try_from(*index).unwrap()])
                        .collect(),
                )),
                StableTypeNode::Array { element, length } => append.intern(nia_ty::TyKind::Array {
                    elem: types[usize::try_from(*element).unwrap()],
                    len: nia_ty::ArrayLenTy::ConstValue(*length),
                }),
                StableTypeNode::Function { parameters, result } => {
                    append.intern(nia_ty::TyKind::FunctionPointer {
                        params: parameters
                            .iter()
                            .map(|index| types[usize::try_from(*index).unwrap()])
                            .collect(),
                        return_type: types[usize::try_from(*result).unwrap()],
                        is_variadic: false,
                    })
                }
                StableTypeNode::Reference { target, mutable } => {
                    append.intern(nia_ty::TyKind::Pointer {
                        is_readonly: !*mutable,
                        elem: types[usize::try_from(*target).unwrap()],
                    })
                }
                StableTypeNode::Pointer { target, readonly } => {
                    append.intern(nia_ty::TyKind::Pointer {
                        is_readonly: *readonly,
                        elem: types[usize::try_from(*target).unwrap()],
                    })
                }
                StableTypeNode::GenericParam(hash) => append.intern(nia_ty::TyKind::GenericParam(
                    SymbolId::from_stable_hash(*hash),
                )),
            };
            types.push(ty);
        }
        Ok(graph
            .roots
            .iter()
            .map(|root| types[usize::try_from(*root).unwrap()])
            .collect())
    }

    /// Rehydrates every selected compiled interface into the current type
    /// store and returns per-definition signature roots. This is the common
    /// semantic input boundary for artifact-backed providers.
    pub fn rehydrate_compiled_interface_type_roots(
        &self,
        resolver: &dyn StableDefinitionResolver,
    ) -> QueryResult<BTreeMap<DefinitionId, Vec<InternedTyId>>> {
        let index = self.compiled_package_interface_index()?;
        let mut result = BTreeMap::new();
        for (_, interface) in index.packages() {
            let types = interface
                .type_graph()
                .map(|graph| self.rehydrate_stable_type_graph(graph, resolver))
                .transpose()?
                .unwrap_or_default();
            for record in interface.records() {
                let roots = record
                    .type_roots
                    .iter()
                    .map(|root| {
                        types.get(*root as usize).copied().ok_or_else(|| {
                            self.db.invalid_input(
                                &CompiledPackageInterfaceIndexQuery,
                                "compiled interface type root is outside its graph".to_string(),
                            )
                        })
                    })
                    .collect::<QueryResult<Vec<_>>>()?;
                if result.insert(record.definition.clone(), roots).is_some() {
                    return Err(self.db.invalid_input(
                        &CompiledPackageInterfaceIndexQuery,
                        "duplicate compiled definition during type-root rehydration".to_string(),
                    ));
                }
            }
        }
        Ok(result)
    }

    /// Publishes rehydrated compiled-package roots into the typed query graph.
    /// The interface index is recorded as the predecessor, so invalidating or
    /// replacing loader-selected artifacts retires the owned payload.
    pub fn publish_compiled_package_type_roots(
        &self,
        package: PackageId,
        roots: BTreeMap<DefinitionId, Vec<InternedTyId>>,
    ) -> QueryResult<()> {
        let index = self.compiled_package_interface_index()?;
        if index.package(&package).is_none() {
            return Err(self.db.invalid_input(
                &CompiledPackageInterfaceIndexQuery,
                format!(
                    "cannot publish compiled type roots for an unselected package: {package:?}"
                ),
            ));
        }
        if roots
            .keys()
            .any(|definition| definition.module.package != package)
        {
            return Err(self.db.invalid_input(
                &CompiledPackageInterfaceIndexQuery,
                "compiled type-root payload contains a definition from another package".to_string(),
            ));
        }
        let key = CompiledPackageTypeRootsQuery(package);
        if self.db.can_publish_owned(key.clone()) {
            self.db
                .publish_owned(key, roots, &CompiledPackageInterfaceIndexQuery);
        }
        Ok(())
    }

    /// Publishes the validated declaration inventory for one selected package
    /// into the typed query graph.
    pub fn publish_compiled_package_declarations(&self, package: PackageId) -> QueryResult<()> {
        let index = self.compiled_package_interface_index()?;
        let Some(interface) = index.package(&package) else {
            return Err(self.db.invalid_input(
                &CompiledPackageInterfaceIndexQuery,
                format!(
                    "cannot publish compiled declarations for an unselected package: {package:?}"
                ),
            ));
        };
        let mut declarations = BTreeMap::new();
        for record in interface.records() {
            let declaration = nia_package_metadata::decode_declaration(&record.declaration)
                .map_err(|error| {
                    self.db
                        .invalid_input(&CompiledPackageInterfaceIndexQuery, error.to_string())
                })?;
            if declaration.kind != record.definition.kind || declaration.visibility != 3 {
                return Err(self.db.invalid_input(
                    &CompiledPackageInterfaceIndexQuery,
                    "compiled declaration identity or public visibility is inconsistent"
                        .to_string(),
                ));
            }
            declarations.insert(record.definition.clone(), declaration);
        }
        let key = CompiledPackageDeclarationsQuery(package.clone());
        if self.db.can_publish_owned(key.clone()) {
            self.db.publish_owned(
                key,
                CompiledPackageDeclarations {
                    package,
                    declarations,
                },
                &CompiledPackageInterfaceIndexQuery,
            );
        }
        Ok(())
    }

    /// Consumes the declaration inventory published for one package.
    pub fn compiled_package_declarations(
        &self,
        package: &PackageId,
    ) -> QueryResult<CompiledPackageDeclarations> {
        self.db
            .get_owned(CompiledPackageDeclarationsQuery(package.clone()))
    }

    /// Installs all loader-selected compiled interface roots into their
    /// package query slots. Definitions are grouped by the stable package
    /// identity carried by the artifact; no package is inferred from the
    /// current source graph.
    pub fn install_compiled_interface_type_roots(
        &self,
        resolver: &dyn StableDefinitionResolver,
    ) -> QueryResult<Vec<PackageId>> {
        let roots = self.rehydrate_compiled_interface_type_roots(resolver)?;
        let mut grouped: BTreeMap<PackageId, BTreeMap<DefinitionId, Vec<InternedTyId>>> =
            BTreeMap::new();
        let selected = self.compiled_package_interface_index()?;
        for (package, _) in selected.packages() {
            grouped.entry(package.clone()).or_default();
        }
        for (definition, type_roots) in roots {
            grouped
                .entry(definition.module.package.clone())
                .or_default()
                .insert(definition, type_roots);
        }
        let mut packages = grouped.keys().cloned().collect::<Vec<_>>();
        for (package, roots) in grouped {
            self.publish_compiled_package_type_roots(package, roots)?;
        }
        let index = self.compiled_package_interface_index()?;
        for (package, _) in index.packages() {
            if !packages.contains(package) {
                packages.push(package.clone());
            }
            self.publish_compiled_package_declarations(package.clone())?;
        }
        packages.sort();
        Ok(packages)
    }

    /// Consumes one artifact-backed package root product from the query graph.
    pub fn compiled_package_type_roots(
        &self,
        package: &PackageId,
    ) -> QueryResult<BTreeMap<DefinitionId, Vec<InternedTyId>>> {
        self.db
            .get_owned(CompiledPackageTypeRootsQuery(package.clone()))
    }

    /// Resolves a stable definition identity against the currently loaded
    /// source graph, validating module, name, and declaration kind together.
    ///
    /// This helper intentionally only resolves definitions present in the
    /// current graph. External package identities must be supplied by a
    /// compiled-interface installation layer rather than guessed from paths.
    pub fn resolve_loaded_definition(
        &self,
        definition: &DefinitionId,
        package: &PackageId,
    ) -> QueryResult<GlobalDefId> {
        if &definition.module.package != package {
            return Err(self.db.invalid_input(
                &ModuleGraphQuery,
                "stable definition belongs to a different package".to_string(),
            ));
        }
        let graph = self.db.get(ModuleGraphQuery)?;
        let Some(module_id) = graph.module_id_for_path(&definition.module.path) else {
            return Err(self.db.invalid_input(
                &ModuleGraphQuery,
                format!(
                    "stable definition module is not loaded: {}",
                    definition.module.path
                ),
            ));
        };
        let defs = self.db.get(FullModuleDefsQuery(module_id))?;
        let symbols = self.db.context().loader_facts().symbols();
        let matches = defs
            .semantic
            .defs
            .iter()
            .filter(|(_def_id, def)| {
                def.parent.is_none()
                    && def_kind_tag(def.kind) == definition.kind
                    && symbols
                        .resolve(def.name)
                        .is_some_and(|name| name.as_ref() == definition.name.as_str())
            })
            .map(|(def_id, _)| GlobalDefId { module_id, def_id })
            .collect::<Vec<_>>();
        let [resolved] = matches.as_slice() else {
            return Err(self.db.invalid_input(
                &ModuleGraphQuery,
                format!(
                    "stable definition is missing or ambiguous: {}::{}",
                    definition.module.path, definition.name
                ),
            ));
        };
        Ok(*resolved)
    }

    /// Converts session-owned type roots into a package-stable type graph.
    ///
    /// Only forms with a complete cross-package representation are accepted.
    /// Unsupported forms deliberately fail rather than serializing a
    /// session-local handle or silently weakening the interface contract.
    pub fn stable_type_graph_for_roots(
        &self,
        package: PackageId,
        roots: &[nia_ids::InternedTyId],
    ) -> QueryResult<StableTypeGraph> {
        self.stable_type_graph_for_roots_with_resolver(roots, &|def_id: GlobalDefId| {
            let graph = self.db.get(ModuleGraphQuery)?;
            let Some(entry_root) = graph.current_package_root(graph.entry()) else {
                return Err(self.db.invalid_input(
                    &ModuleGraphQuery,
                    "entry module has no package root; provide an external definition resolver"
                        .to_string(),
                ));
            };
            if graph.current_package_root(def_id.module_id) != Some(entry_root) {
                return Err(self.db.invalid_input(
                    &ModuleGraphQuery,
                    "nominal type belongs to an external package; provide an external definition resolver"
                        .to_string(),
                ));
            }
            Ok(package.clone())
        })
    }

    /// Converts session-owned type roots into a package-stable type graph using
    /// an explicit definition-to-package resolver.
    ///
    /// The resolver is required for every nominal definition, including
    /// definitions from the current package. This keeps external identities
    /// explicit and prevents dependency types from being silently attributed
    /// to the package being published.
    pub fn stable_type_graph_for_roots_with_resolver(
        &self,
        roots: &[nia_ids::InternedTyId],
        resolver: &dyn StableDefinitionPackageResolver,
    ) -> QueryResult<StableTypeGraph> {
        self.stable_type_graph_for_roots_with_resolver_and_indexes(roots, resolver)
            .map(|(graph, _)| graph)
    }

    fn stable_type_graph_for_roots_with_resolver_and_indexes(
        &self,
        roots: &[nia_ids::InternedTyId],
        resolver: &dyn StableDefinitionPackageResolver,
    ) -> QueryResult<(StableTypeGraph, HashMap<nia_ids::InternedTyId, u32>)> {
        let graph = self.db.get(ModuleGraphQuery)?;
        let symbols = self.db.context().loader_facts().symbols();
        let mut encoder = StableTypeGraphEncoder {
            db: &self.db,
            graph: &graph,
            symbols: &symbols,
            resolver,
            indexes: HashMap::new(),
            visiting: HashSet::new(),
            key_cache: HashMap::new(),
            key_visiting: HashSet::new(),
            canonical_indexes: HashMap::new(),
            nodes: Vec::new(),
        };
        // Session-local type handles are intentionally not part of the
        // published ordering. Sort roots by their structural, relocation-
        // independent key before assigning graph indices.
        let mut keyed_roots = roots
            .iter()
            .map(|root| Ok((*root, encoder.canonical_key(*root)?)))
            .collect::<QueryResult<Vec<_>>>()?;
        keyed_roots.sort_by(|left, right| left.1.cmp(&right.1));
        keyed_roots.dedup_by(|left, right| left.1 == right.1);
        let ordered_roots = keyed_roots
            .into_iter()
            .map(|(root, _)| root)
            .collect::<Vec<_>>();
        let roots = ordered_roots
            .iter()
            .map(|root| encoder.encode(*root))
            .collect::<QueryResult<Vec<_>>>()?;
        let mut roots = roots;
        roots.sort_unstable();
        roots.dedup();
        let graph = StableTypeGraph {
            nodes: encoder.nodes,
            roots,
        };
        graph
            .validate()
            .map_err(|error| self.db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
        Ok((graph, encoder.indexes))
    }

    /// Publishes the target-independent public declaration inventory for one package.
    ///
    /// This is deliberately an explicit publication API: normal compilation
    /// continues to consume the tracked source queries. The returned section is
    /// canonical and can be embedded in a [`nia_package_metadata::PackageArtifact`].
    pub fn package_interface_section(&self, package: PackageId) -> QueryResult<InterfaceSection> {
        let package_for_resolver = package.clone();
        let resolver = |def_id: GlobalDefId| {
            let graph = self.db.get(ModuleGraphQuery)?;
            let Some(entry_root) = graph.current_package_root(graph.entry()) else {
                return Err(self.db.invalid_input(
                    &ModuleGraphQuery,
                    "entry module has no package root; provide an external definition resolver"
                        .to_string(),
                ));
            };
            if graph.current_package_root(def_id.module_id) != Some(entry_root) {
                return Err(self.db.invalid_input(
                    &ModuleGraphQuery,
                    "nominal type belongs to an external package; provide an external definition resolver"
                        .to_string(),
                ));
            }
            Ok(package_for_resolver.clone())
        };
        self.package_interface_and_type_graph(package, &resolver)
            .map(|(interface, _)| interface)
    }

    /// Publishes an interface using an explicit resolver for every nominal
    /// definition. This is the required API when signatures reference
    /// dependency packages.
    pub fn package_interface_section_with_resolver(
        &self,
        package: PackageId,
        resolver: &dyn StableDefinitionPackageResolver,
    ) -> QueryResult<InterfaceSection> {
        self.package_interface_and_type_graph(package, resolver)
            .map(|(interface, _)| interface)
    }

    fn package_interface_and_type_graph(
        &self,
        package: PackageId,
        resolver: &dyn StableDefinitionPackageResolver,
    ) -> QueryResult<(InterfaceSection, StableTypeGraph)> {
        let graph = self.db.get(ModuleGraphQuery)?;
        let symbols = self.db.context().loader_facts().symbols();
        let mut pending = Vec::new();
        for module in graph.modules() {
            let Some(stable_key) = graph.stable_key(module.id) else {
                continue;
            };
            let module_path = stable_key.source_identity().normalized_path().to_owned();
            let defs = self.db.get(FullModuleDefsQuery(module.id))?;
            for (def_id, def) in defs.semantic.defs.iter() {
                if def.parent.is_some() || def.visibility != nia_defs::Visibility::Public {
                    continue;
                }
                let Some(name) = symbols.resolve(def.name) else {
                    continue;
                };
                let definition = DefinitionId {
                    module: StableModuleId {
                        package: package.clone(),
                        path: module_path.clone(),
                    },
                    name: name.to_string(),
                    kind: def_kind_tag(def.kind),
                };
                let roots = self
                    .db
                    .get(ItemSignaturesQuery(module.id))?
                    .semantic
                    .type_roots_for_definition(def_id)
                    .unwrap_or_default();
                pending.push((
                    InterfaceRecord {
                        definition,
                        declaration: declaration_signature(def),
                        type_roots: Vec::new(),
                    },
                    roots,
                ));
            }
        }
        let mut all_roots = pending
            .iter()
            .flat_map(|(_, roots)| roots.iter().copied())
            .collect::<Vec<_>>();
        all_roots.sort_unstable();
        all_roots.dedup();
        let (type_graph, indexes) =
            self.stable_type_graph_for_roots_with_resolver_and_indexes(&all_roots, resolver)?;
        let mut records = pending
            .into_iter()
            .map(|(mut record, roots)| {
                record.type_roots = roots
                    .into_iter()
                    .map(|root| {
                        indexes.get(&root).copied().ok_or_else(|| {
                            self.db.invalid_input(
                                &ModuleGraphQuery,
                                "signature root missing from published type graph".to_string(),
                            )
                        })
                    })
                    .collect::<QueryResult<Vec<_>>>()?;
                Ok(record)
            })
            .collect::<QueryResult<Vec<_>>>()?;
        records.sort_by(|left, right| left.definition.cmp(&right.definition));
        let section = InterfaceSection { records };
        section
            .validate()
            .map_err(|error| self.db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
        section
            .validate_type_roots(&type_graph)
            .map_err(|error| self.db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
        Ok((section, type_graph))
    }

    /// Produces one complete, target-independent package artifact.
    pub fn publish_package_artifact(
        &self,
        package: PackageId,
    ) -> QueryResult<crate::PackageArtifactPublication> {
        let package_for_resolver = package.clone();
        let resolver = |def_id: GlobalDefId| {
            let graph = self.db.get(ModuleGraphQuery)?;
            let Some(entry_root) = graph.current_package_root(graph.entry()) else {
                return Err(self.db.invalid_input(
                    &ModuleGraphQuery,
                    "entry module has no package root; provide an external definition resolver"
                        .to_string(),
                ));
            };
            if graph.current_package_root(def_id.module_id) != Some(entry_root) {
                return Err(self.db.invalid_input(
                    &ModuleGraphQuery,
                    "nominal type belongs to an external package; provide an external definition resolver"
                        .to_string(),
                ));
            }
            Ok(package_for_resolver.clone())
        };
        self.publish_package_artifact_with_resolver(package, &resolver)
    }

    /// Produces a package artifact with an explicit nominal-definition
    /// resolver, allowing public signatures to refer to dependencies.
    pub fn publish_package_artifact_with_resolver(
        &self,
        package: PackageId,
        resolver: &dyn StableDefinitionPackageResolver,
    ) -> QueryResult<crate::PackageArtifactPublication> {
        let (interface, type_graph) =
            self.package_interface_and_type_graph(package.clone(), resolver)?;
        let interface_bytes = nia_package_metadata::encode_interface(&interface)
            .map_err(|error| self.db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
        let graph = self.db.get(ModuleGraphQuery)?;
        let mut paths = graph
            .modules()
            .filter_map(|module| {
                graph
                    .stable_key(module.id)
                    .map(|key| key.source_identity().normalized_path().to_owned())
            })
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        let modules = paths
            .into_iter()
            .map(|path| {
                let module_section = InterfaceSection {
                    records: interface
                        .records
                        .iter()
                        .filter(|record| record.definition.module.path == path)
                        .cloned()
                        .collect(),
                };
                let bytes = nia_package_metadata::encode_interface(&module_section)
                    .map_err(|error| self.db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
                Ok(ModuleInterface {
                    path,
                    interface_hash: nia_package_metadata::section_hash(&bytes),
                })
            })
            .collect::<QueryResult<Vec<_>>>()?;
        let manifest = PackageManifest {
            modules,
            ..PackageManifest::current(package)
        };
        let type_graph_bytes = nia_package_metadata::encode_type_graph(&type_graph)
            .map_err(|error| self.db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
        let bytes = nia_package_metadata::encode_artifact(
            &manifest,
            &[
                (SectionKind::Interface, interface_bytes.as_slice()),
                (SectionKind::TypeGraph, type_graph_bytes.as_slice()),
            ],
        )
        .map_err(|error| self.db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
        nia_package_metadata::PackageArtifact::open(bytes.clone())
            .map_err(|error| self.db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
        Ok(crate::PackageArtifactPublication { manifest, bytes })
    }

    /// Checks every loaded module after settling provider-demand fixed points.
    pub fn check_program(&self) -> QueryResult<CheckedProgram> {
        self.check_report(FrontendCheckScope::AllModules)
    }

    #[doc(hidden)]
    pub fn analyze_program(&self) -> QueryResult<CheckedProgramAnalysis> {
        self.settle_provider_worklist(false, Self::check_program_once, checked_provider_demands)
    }

    fn check_program_once(&self) -> QueryResult<CheckedProgramAnalysis> {
        self.db.get(CheckedProgramQuery).map(Arc::unwrap_or_clone)
    }

    /// Checks the entry-reachable program scope.
    pub fn entry_check_program(&self) -> QueryResult<CheckedProgram> {
        self.check_report(FrontendCheckScope::Entry)
    }

    #[doc(hidden)]
    pub fn analyze_entry_program(&self) -> QueryResult<CheckedProgramAnalysis> {
        self.settle_provider_worklist(
            true,
            Self::entry_check_program_once,
            checked_provider_demands,
        )
    }

    fn entry_check_program_once(&self) -> QueryResult<CheckedProgramAnalysis> {
        self.db
            .get(EntryCheckedProgramQuery)
            .map(Arc::unwrap_or_clone)
    }

    fn check_report(&self, scope: FrontendCheckScope) -> QueryResult<CheckedProgram> {
        let certificate_context = self.check_certificate_context(scope)?;
        let cached = certificate_context.and_then(|context| {
            let cache = self.db.context().signature_cache.as_ref()?;
            let lookup = cache.load_check_certificate(context.identity()).ok()?;
            Some((context, lookup))
        });
        if !self.db.context().verify_frontend_cache
            && let Some((_, crate::signature_cache::CheckCertificateLookup::Hit(certificate))) =
                cached.as_ref()
        {
            emit_check_certificate_reuse(self.db.context().timings(), true);
            self.db.context().loader_facts().settle_provider_demands()?;
            self.db
                .context()
                .provider_demand_rounds
                .store(0, std::sync::atomic::Ordering::Relaxed);
            return Ok(CheckedProgram {
                graph: self.current_graph()?,
                optimization: self.current_optimization(),
                diagnostics: certificate.diagnostics.clone(),
                checked_body_count: certificate.checked_body_count,
                reachable_body_count: certificate.reachable_body_count,
            });
        }
        emit_check_certificate_reuse(self.db.context().timings(), false);
        let report = match scope {
            FrontendCheckScope::AllModules => self.analyze_program()?.into_report(),
            FrontendCheckScope::Entry => self.analyze_entry_program()?.into_report(),
        };
        let Some(cache) = self.db.context().signature_cache.as_ref() else {
            return Ok(report);
        };
        let context = self.check_certificate_context(scope)?;
        if self.db.context().verify_frontend_cache
            && let Some((cached_context, crate::signature_cache::CheckCertificateLookup::Hit(_))) =
                cached.as_ref()
            && context
                .as_ref()
                .is_none_or(|context| context.key() != cached_context.key())
        {
            cache.remove_check_certificate(cached_context.key());
        }
        let Some(context) = context else {
            return Ok(report);
        };
        let certificate = crate::signature_cache::CachedCheckCertificate {
            checked_body_count: report.checked_body_count(),
            reachable_body_count: report.reachable_body_count(),
            diagnostics: report.diagnostics.clone(),
        };
        let _ = cache.publish_check_certificate(
            context.identity(),
            certificate,
            self.db.context().verify_frontend_cache,
        );
        Ok(report)
    }

    fn check_certificate_context(
        &self,
        scope: FrontendCheckScope,
    ) -> QueryResult<Option<CheckCertificateContext>> {
        let program_sources = self.db.get(FrontendProgramSourcesQuery)?;
        let Some(program_sources) = program_sources.as_ref().as_ref() else {
            return Ok(None);
        };
        let graph = self.db.get(ModuleGraphQuery)?;
        let Some(entry) = graph.stable_key(graph.entry()).cloned() else {
            return Ok(None);
        };
        let provider_facts = self.db.context().provider_fact_worklist()?;
        let input = check_certificate_input_fingerprint(
            program_sources.fingerprint,
            &graph,
            &provider_facts,
        );
        Ok(Some(CheckCertificateContext {
            namespace: self.db.context().frontend_cache_namespace(),
            entry,
            input,
            scope,
            source_lengths: program_sources
                .by_module
                .values()
                .map(|source| {
                    (
                        source.module.source_identity().normalized_path().to_owned(),
                        source.len,
                    )
                })
                .collect(),
        }))
    }

    fn executable_provider_demands(&self) -> QueryResult<Vec<crate::ProviderDemand>> {
        self.db
            .get(ExecutableProviderDemandsQuery)
            .map(Arc::unwrap_or_clone)
    }

    /// Returns the loader provider-fact revision observed by the query graph.
    pub fn provider_fact_revision(&self) -> QueryResult<crate::ProviderFactRevision> {
        self.db
            .get(ProviderFactRevisionQuery)
            .map(|revision| *revision)
    }

    /// Produces the complete checked and backend-lowered program.
    pub fn codegen_program(&self) -> QueryResult<CodegenProgram> {
        self.settle_provider_worklist(true, Self::codegen_program_once, codegen_provider_demands)
    }

    /// Produces checked executable products before backend module finalization.
    pub fn codegen_preparation(&self) -> QueryResult<CodegenPreparation> {
        self.settle_provider_worklist(
            true,
            Self::codegen_preparation_once,
            codegen_preparation_provider_demands,
        )
    }

    /// Exposes completion-order backend finalization to `consume` within its executor scope.
    pub fn with_backend_finalization_schedule<R>(
        &self,
        consume: impl for<'borrow, 'stream, 'executor> FnOnce(
            Result<
                crate::BackendFinalizationSchedule<'borrow, 'stream, 'executor>,
                nia_backend_lower::BackendLowering,
            >,
        ) -> R,
    ) -> QueryResult<R> {
        providers::with_backend_finalization_schedule(&self.db, consume)
    }

    fn codegen_preparation_once(&self) -> QueryResult<CodegenPreparation> {
        self.db
            .get(CodegenPreparationQuery)
            .map(Arc::unwrap_or_clone)
    }

    fn codegen_program_once(&self) -> QueryResult<CodegenProgram> {
        self.db.get(CodegenProgramQuery).map(Arc::unwrap_or_clone)
    }

    fn settle_provider_worklist<T>(
        &self,
        discover_executable_providers: bool,
        compile: impl Fn(&Self) -> QueryResult<T>,
        provider_demands: impl Fn(&T) -> Vec<crate::ProviderDemand>,
    ) -> QueryResult<T> {
        let mut skip_executable_discovery = false;
        let mut rounds = 0_u64;
        loop {
            rounds += 1;
            if discover_executable_providers && !skip_executable_discovery {
                let demands = self.executable_provider_demands()?;
                emit_provider_demand_batch(self.db.context().timings(), rounds, &demands);
                if let crate::ProviderGraphUpdate::Changed {
                    invalidates_resolved_body_facts,
                } = self
                    .db
                    .context()
                    .loader_facts()
                    .update_provider_demands(demands)?
                {
                    emit_provider_graph_change(
                        self.db.context().timings(),
                        rounds,
                        invalidates_resolved_body_facts,
                    );
                    skip_executable_discovery = !invalidates_resolved_body_facts;
                    continue;
                }
            }
            let output = compile(self)?;
            match self
                .db
                .context()
                .loader_facts()
                .update_provider_demands(provider_demands(&output))?
            {
                crate::ProviderGraphUpdate::Changed {
                    invalidates_resolved_body_facts,
                } => {
                    skip_executable_discovery =
                        discover_executable_providers && !invalidates_resolved_body_facts;
                }
                crate::ProviderGraphUpdate::Stable => {
                    self.db.context().loader_facts().settle_provider_demands()?;
                    self.db
                        .context()
                        .provider_demand_rounds
                        .store(rounds, std::sync::atomic::Ordering::Relaxed);
                    return Ok(output);
                }
            }
        }
    }

    /// Returns the number of fixed-point rounds used by the last top-level compilation.
    pub fn provider_demand_rounds(&self) -> u64 {
        self.db
            .context()
            .provider_demand_rounds
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Replaces session-compatible inputs and returns the resulting invalidation set.
    ///
    /// The loader session, frontend cache root, and verification policy cannot
    /// change in place because they own persisted and in-memory query identity.
    pub fn update(&self, request: CompileRequest) -> QueryResult<CompilerInvalidation> {
        let loader_session = request.loader_facts.query_session().unwrap_or_else(|| {
            panic!("Nia ICE: compiler updates require a tracked loader fact provider")
        });
        assert!(
            self.db.session().ptr_eq(&loader_session),
            "Nia ICE: compiler update loader facts belong to a different query session"
        );
        assert_eq!(
            request.frontend_cache_dir.as_deref(),
            self.db
                .context()
                .signature_cache
                .as_ref()
                .map(|cache| cache.root()),
            "Nia ICE: compiler frontend cache root cannot change within a query session"
        );
        assert_eq!(
            request.verify_frontend_cache,
            self.db.context().verify_frontend_cache,
            "Nia ICE: compiler frontend cache verification cannot change within a query session"
        );
        let new_graph = request.loader_facts.module_graph()?;
        let new_compiled_interface_fingerprint =
            compiled_interface_fingerprint(request.loader_facts.compiled_package_interfaces()?)?;
        let compiled_interfaces_changed = {
            let observed = self
                .db
                .context()
                .observed_compiled_interfaces
                .lock()
                .expect("compiler compiled-interface observation lock poisoned");
            *observed != Some(new_compiled_interface_fingerprint)
        };
        let graph_changed = {
            let observed = self
                .db
                .context()
                .observed_graph
                .lock()
                .expect("compiler graph observation lock poisoned");
            *observed != new_graph
        };
        let handle_generation_changed = {
            let observed = self
                .db
                .context()
                .observed_graph
                .lock()
                .expect("compiler graph observation lock poisoned");
            observed.modules().any(|old| {
                let Some(key) = observed.stable_key(old.id) else {
                    return false;
                };
                new_graph
                    .modules()
                    .find(|module| new_graph.stable_key(module.id) == Some(key))
                    .is_none_or(|new| new.id != old.id)
            })
        };
        let new_inputs = CompilerInputs::new(request);
        let optimization_changed = {
            let mut inputs = self.inputs.write().expect("compiler input lock poisoned");
            let optimization_changed = inputs.optimization != new_inputs.optimization;
            *inputs = new_inputs;
            optimization_changed
        };
        let mut invalidation = CompilerInvalidation::default();
        if graph_changed {
            // The executable fact epoch contains session-local module handles;
            // a graph replacement makes that value and every dependent red.
            invalidation.extend(self.db.invalidate(ExecutableFactEpochQuery));
            if handle_generation_changed {
                self.db
                    .session()
                    .invalidate_scope(|frame| frame.name == "loaded_modules");
            }
            let loaded_modules = StableModuleSequence::from_source_identities(
                self.db
                    .context()
                    .loader_facts()
                    .loaded_module_source_identities()?,
            );
            invalidation.extend(self.db.validate_input(LoadedModulesQuery, &loaded_modules));
            if handle_generation_changed {
                *self
                    .db
                    .context()
                    .executable_fact_session
                    .lock()
                    .expect("executable fact session lock poisoned") =
                    ExecutableFactSession::default();
            }
        }
        if compiled_interfaces_changed {
            invalidation.extend(self.db.invalidate(CompiledPackageInterfaceIndexQuery));
            *self
                .db
                .context()
                .observed_compiled_interfaces
                .lock()
                .expect("compiler compiled-interface observation lock poisoned") =
                Some(new_compiled_interface_fingerprint);
        }
        let inputs_invalidation = self.invalidate_inputs(optimization_changed)?;
        invalidation
            .invalidated
            .extend(inputs_invalidation.invalidated);
        if graph_changed {
            *self
                .db
                .context()
                .observed_graph
                .lock()
                .expect("compiler graph observation lock poisoned") = new_graph;
        }
        // The compiled-interface index is an input-derived semantic query.
        // Retire it only when the loaded module graph changes; option-only or
        // content-identical updates must preserve the query graph and reuse
        // the existing index.
        if graph_changed && !compiled_interfaces_changed {
            invalidation.extend(self.db.invalidate(CompiledPackageInterfaceIndexQuery));
        }
        Ok(invalidation)
    }

    /// Returns a snapshot of query execution and reuse counters.
    pub fn query_trace(&self) -> QueryTrace {
        self.db.query_trace()
    }

    fn current_graph(&self) -> QueryResult<ModuleGraphSnapshot> {
        self.db.context().loader_facts.module_graph()
    }

    fn current_optimization(&self) -> OptimizationPolicy {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .optimization
    }

    fn invalidate_inputs(&self, optimization_changed: bool) -> QueryResult<CompilerInvalidation> {
        let mut invalidation = CompilerInvalidation::default();
        let provider_worklist = self.db.context().provider_fact_worklist()?;
        invalidation.extend(
            self.db
                .validate_input(ProviderFactWorklistQuery, &provider_worklist),
        );
        if optimization_changed {
            invalidation.extend(self.db.invalidate(CompilerOptimizationQuery));
        }
        Ok(invalidation)
    }
}

fn stable_primitive_from_tag(tag: u8) -> Option<nia_ty::PrimitiveTy> {
    use nia_ty::PrimitiveTy;
    Some(match tag {
        1 => PrimitiveTy::I8,
        2 => PrimitiveTy::I16,
        3 => PrimitiveTy::I32,
        4 => PrimitiveTy::I64,
        5 => PrimitiveTy::I128,
        6 => PrimitiveTy::Isize,
        7 => PrimitiveTy::U8,
        8 => PrimitiveTy::U16,
        9 => PrimitiveTy::U32,
        10 => PrimitiveTy::U64,
        11 => PrimitiveTy::U128,
        12 => PrimitiveTy::Usize,
        13 => PrimitiveTy::F32,
        14 => PrimitiveTy::F64,
        15 => PrimitiveTy::Bool,
        16 => PrimitiveTy::Char,
        _ => return None,
    })
}

struct StableTypeGraphEncoder<'db> {
    db: &'db QueryDb<CompilerContext>,
    graph: &'db ModuleGraphSnapshot,
    symbols: &'db nia_symbol_table::SymbolTable,
    resolver: &'db dyn StableDefinitionPackageResolver,
    indexes: HashMap<nia_ids::InternedTyId, u32>,
    visiting: HashSet<nia_ids::InternedTyId>,
    key_cache: HashMap<nia_ids::InternedTyId, Vec<u8>>,
    key_visiting: HashSet<nia_ids::InternedTyId>,
    canonical_indexes: HashMap<Vec<u8>, u32>,
    nodes: Vec<StableTypeNode>,
}

impl StableTypeGraphEncoder<'_> {
    fn encode(&mut self, ty: nia_ids::InternedTyId) -> QueryResult<u32> {
        if let Some(index) = self.indexes.get(&ty) {
            return Ok(*index);
        }
        let key = self.canonical_key(ty)?;
        if !self.visiting.insert(ty) {
            return Err(self.db.invalid_input(
                &ModuleGraphQuery,
                "recursive session type graph has no stable package representation".to_string(),
            ));
        }
        let kind = self
            .db
            .context()
            .type_store
            .get(ty)
            .cloned()
            .ok_or_else(|| {
                self.db.invalid_input(
                    &ModuleGraphQuery,
                    "type root belongs to a different compiler session".to_string(),
                )
            })?;
        let node = match kind {
            nia_ty::TyKind::Primitive(primitive) => primitive_type_node(primitive)?,
            nia_ty::TyKind::Tuple(elements) if elements.is_empty() => StableTypeNode::Unit,
            nia_ty::TyKind::Tuple(elements) => StableTypeNode::Tuple(
                elements
                    .into_iter()
                    .map(|element| self.encode(element))
                    .collect::<QueryResult<Vec<_>>>()?,
            ),
            nia_ty::TyKind::Pointer {
                is_readonly, elem, ..
            } => StableTypeNode::Pointer {
                target: self.encode(elem)?,
                readonly: is_readonly,
            },
            nia_ty::TyKind::Array { len, elem } => {
                let nia_ty::ArrayLenTy::ConstValue(length) = len else {
                    return Err(self.unsupported("array length is not a stable constant"));
                };
                StableTypeNode::Array {
                    element: self.encode(elem)?,
                    length,
                }
            }
            nia_ty::TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic: false,
            } => StableTypeNode::Function {
                parameters: params
                    .into_iter()
                    .map(|parameter| self.encode(parameter))
                    .collect::<QueryResult<Vec<_>>>()?,
                result: self.encode(return_type)?,
            },
            nia_ty::TyKind::Nominal {
                def_id,
                args,
                const_args,
            } => {
                let definition = self.definition(def_id)?;
                if args.is_empty() && const_args.is_empty() {
                    StableTypeNode::Named(definition)
                } else {
                    StableTypeNode::NamedApplied {
                        definition,
                        arguments: args
                            .into_iter()
                            .map(|argument| self.encode(argument))
                            .collect::<QueryResult<Vec<_>>>()?,
                        const_arguments: const_args
                            .iter()
                            .map(|argument| self.encode_const_argument(argument))
                            .collect::<QueryResult<Vec<_>>>()?,
                    }
                }
            }
            nia_ty::TyKind::GenericParam(name) => StableTypeNode::GenericParam(name.raw()),
            _ => return Err(self.unsupported("type form has no stable package encoding")),
        };
        self.visiting.remove(&ty);
        if let Some(index) = self.canonical_indexes.get(&key) {
            self.indexes.insert(ty, *index);
            return Ok(*index);
        }
        let index = u32::try_from(self.nodes.len()).map_err(|_| {
            self.db.invalid_input(
                &ModuleGraphQuery,
                "stable type graph exceeds u32 nodes".to_string(),
            )
        })?;
        self.nodes.push(node);
        self.indexes.insert(ty, index);
        self.canonical_indexes.insert(key, index);
        Ok(index)
    }

    /// Computes a relocation-independent structural key.  This key is used
    /// solely for graph ordering/deduplication; it never contains session
    /// handles such as `InternedTyId` or `DefId`.
    fn canonical_key(&mut self, ty: nia_ids::InternedTyId) -> QueryResult<Vec<u8>> {
        if let Some(key) = self.key_cache.get(&ty) {
            return Ok(key.clone());
        }
        if !self.key_visiting.insert(ty) {
            return Err(self.db.invalid_input(
                &ModuleGraphQuery,
                "recursive session type graph has no stable package representation".to_string(),
            ));
        }
        let kind = self
            .db
            .context()
            .type_store
            .get(ty)
            .cloned()
            .ok_or_else(|| {
                self.db.invalid_input(
                    &ModuleGraphQuery,
                    "type root belongs to a different compiler session".to_string(),
                )
            })?;
        let mut key = Vec::new();
        match kind {
            nia_ty::TyKind::Primitive(primitive) => {
                let stable = primitive_type_node(primitive)?;
                match stable {
                    StableTypeNode::Primitive(tag) => key.extend_from_slice(&[1, tag]),
                    StableTypeNode::Never => key.push(10),
                    _ => return Err(self.unsupported("primitive has no stable package tag")),
                }
            }
            nia_ty::TyKind::Tuple(elements) if elements.is_empty() => key.push(2),
            nia_ty::TyKind::Tuple(elements) => {
                key.push(3);
                append_len(&mut key, elements.len());
                for element in elements {
                    append_bytes(&mut key, &self.canonical_key(element)?);
                }
            }
            nia_ty::TyKind::Pointer { is_readonly, elem } => {
                key.extend_from_slice(&[4, u8::from(is_readonly)]);
                append_bytes(&mut key, &self.canonical_key(elem)?);
            }
            nia_ty::TyKind::Array { len, elem } => {
                let nia_ty::ArrayLenTy::ConstValue(length) = len else {
                    return Err(self.unsupported("array length is not a stable constant"));
                };
                key.push(5);
                key.extend_from_slice(&length.to_le_bytes());
                append_bytes(&mut key, &self.canonical_key(elem)?);
            }
            nia_ty::TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic: false,
            } => {
                key.push(6);
                append_len(&mut key, params.len());
                for parameter in params {
                    append_bytes(&mut key, &self.canonical_key(parameter)?);
                }
                append_bytes(&mut key, &self.canonical_key(return_type)?);
            }
            nia_ty::TyKind::Nominal {
                def_id,
                args,
                const_args,
            } => {
                let definition = self.definition(def_id)?;
                key.push(if args.is_empty() && const_args.is_empty() {
                    7
                } else {
                    8
                });
                append_definition_key(&mut key, &definition);
                append_len(&mut key, args.len());
                for argument in args {
                    append_bytes(&mut key, &self.canonical_key(argument)?);
                }
                append_len(&mut key, const_args.len());
                for argument in &const_args {
                    append_const_arg_key(&mut key, argument);
                }
            }
            nia_ty::TyKind::GenericParam(name) => {
                key.push(9);
                key.extend_from_slice(&name.raw().to_le_bytes());
            }
            _ => return Err(self.unsupported("type form has no stable package encoding")),
        }
        self.key_visiting.remove(&ty);
        self.key_cache.insert(ty, key.clone());
        Ok(key)
    }

    fn definition(&self, def_id: nia_ids::GlobalDefId) -> QueryResult<DefinitionId> {
        let module = self.graph.stable_key(def_id.module_id).ok_or_else(|| {
            self.db.invalid_input(
                &ModuleGraphQuery,
                "nominal type has no stable module identity".to_string(),
            )
        })?;
        let defs = self.db.get(FullModuleDefsQuery(def_id.module_id))?;
        let def = defs.semantic.defs.get(def_id.def_id).ok_or_else(|| {
            self.db.invalid_input(
                &ModuleGraphQuery,
                "nominal type has no definition facts".to_string(),
            )
        })?;
        let name = self.symbols.resolve(def.name).ok_or_else(|| {
            self.db.invalid_input(
                &ModuleGraphQuery,
                "nominal type name is absent from the session symbol table".to_string(),
            )
        })?;
        let package = self.resolver.package_for_definition(def_id)?;
        Ok(DefinitionId {
            module: StableModuleId {
                package,
                path: module.source_identity().normalized_path().to_owned(),
            },
            name: name.to_string(),
            kind: def_kind_tag(def.kind),
        })
    }

    fn encode_const_argument(
        &mut self,
        argument: &nia_ty::ConstGenericArg,
    ) -> QueryResult<StableConstArg> {
        match &argument.value {
            nia_ty::ConstGenericValue::GenericParam(name) => {
                Ok(StableConstArg::GenericParam(name.raw()))
            }
            nia_ty::ConstGenericValue::Int(value) => Ok(StableConstArg::Integer {
                bits: value.bits(),
                signed: value.is_signed(),
            }),
            nia_ty::ConstGenericValue::Bool(value) => Ok(StableConstArg::Bool(*value)),
            nia_ty::ConstGenericValue::Char(value) => Ok(StableConstArg::Char(*value)),
            nia_ty::ConstGenericValue::ConstExpr(_) => {
                Err(self
                    .unsupported("const expression argument has no stable package representation"))
            }
        }
    }

    fn unsupported(&self, detail: &str) -> QueryError {
        self.db.invalid_input(
            &ModuleGraphQuery,
            format!("{detail}; package type graph publication is not implemented for this form"),
        )
    }
}

fn primitive_type_node(primitive: nia_ty::PrimitiveTy) -> QueryResult<StableTypeNode> {
    use nia_ty::PrimitiveTy;
    Ok(match primitive {
        PrimitiveTy::Never => StableTypeNode::Never,
        PrimitiveTy::I8 => StableTypeNode::Primitive(1),
        PrimitiveTy::I16 => StableTypeNode::Primitive(2),
        PrimitiveTy::I32 => StableTypeNode::Primitive(3),
        PrimitiveTy::I64 => StableTypeNode::Primitive(4),
        PrimitiveTy::I128 => StableTypeNode::Primitive(5),
        PrimitiveTy::Isize => StableTypeNode::Primitive(6),
        PrimitiveTy::U8 => StableTypeNode::Primitive(7),
        PrimitiveTy::U16 => StableTypeNode::Primitive(8),
        PrimitiveTy::U32 => StableTypeNode::Primitive(9),
        PrimitiveTy::U64 => StableTypeNode::Primitive(10),
        PrimitiveTy::U128 => StableTypeNode::Primitive(11),
        PrimitiveTy::Usize => StableTypeNode::Primitive(12),
        PrimitiveTy::F32 => StableTypeNode::Primitive(13),
        PrimitiveTy::F64 => StableTypeNode::Primitive(14),
        PrimitiveTy::Bool => StableTypeNode::Primitive(15),
        PrimitiveTy::Char => StableTypeNode::Primitive(16),
    })
}

fn append_len(bytes: &mut Vec<u8>, len: usize) {
    bytes.extend_from_slice(
        &(u64::try_from(len).expect("type graph length exceeds u64")).to_le_bytes(),
    );
}

fn append_bytes(bytes: &mut Vec<u8>, value: &[u8]) {
    append_len(bytes, value.len());
    bytes.extend_from_slice(value);
}

fn append_definition_key(bytes: &mut Vec<u8>, definition: &DefinitionId) {
    append_bytes(bytes, definition.module.package.namespace.as_bytes());
    append_bytes(bytes, definition.module.package.name.as_bytes());
    append_bytes(bytes, definition.module.package.version.as_bytes());
    append_bytes(bytes, definition.module.path.as_bytes());
    append_bytes(bytes, definition.name.as_bytes());
    bytes.push(definition.kind);
}

fn append_const_arg_key(bytes: &mut Vec<u8>, argument: &nia_ty::ConstGenericArg) {
    match &argument.value {
        nia_ty::ConstGenericValue::GenericParam(name) => {
            bytes.push(1);
            bytes.extend_from_slice(&name.raw().to_le_bytes());
        }
        nia_ty::ConstGenericValue::Int(value) => {
            bytes.push(2);
            bytes.extend_from_slice(&value.bits().to_le_bytes());
            bytes.push(u8::from(value.is_signed()));
        }
        nia_ty::ConstGenericValue::Bool(value) => bytes.extend_from_slice(&[3, u8::from(*value)]),
        nia_ty::ConstGenericValue::Char(value) => {
            bytes.push(4);
            bytes.extend_from_slice(&u32::from(*value).to_le_bytes());
        }
        nia_ty::ConstGenericValue::ConstExpr(_) => bytes.push(5),
    }
}

fn declaration_signature(def: &nia_defs::Def) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(24 + def.generics.len() * 8);
    bytes.extend_from_slice(b"NIADECL01");
    bytes.push(def_kind_tag(def.kind));
    bytes.push(match def.visibility {
        nia_defs::Visibility::Private => 0,
        nia_defs::Visibility::PublicSuper => 1,
        nia_defs::Visibility::PublicPkg => 2,
        nia_defs::Visibility::Public => 3,
    });
    bytes.extend_from_slice(
        &u32::try_from(def.generics.len())
            .expect("definition generic count exceeds u32")
            .to_le_bytes(),
    );
    for generic in &def.generics {
        bytes.extend_from_slice(&generic.raw().to_le_bytes());
    }
    bytes
}

fn def_kind_tag(kind: nia_defs::DefKind) -> u8 {
    use nia_defs::DefKind;
    match kind {
        DefKind::Module => 1,
        DefKind::Function => 2,
        DefKind::Global => 3,
        DefKind::Const => 4,
        DefKind::Struct => 5,
        DefKind::StructField => 6,
        DefKind::Union => 7,
        DefKind::UnionField => 8,
        DefKind::Trait => 9,
        DefKind::TraitAssociatedType => 10,
        DefKind::TraitMethod => 11,
        DefKind::Method => 12,
        DefKind::Enum => 13,
        DefKind::EnumVariant => 14,
        DefKind::EnumVariantField => 15,
        DefKind::TypeAlias => 16,
    }
}

fn emit_provider_graph_change(
    timings: TimingMode,
    round: u64,
    invalidates_resolved_body_facts: bool,
) {
    if !timings.enabled() {
        return;
    }
    let prefix = format!("compiler.executable_provider_demands.round_{round}");
    nia_timing::emit_counter(format!("{prefix}.graph_changed"), 1);
    nia_timing::emit_counter(
        format!("{prefix}.invalidates_body_facts"),
        u64::from(invalidates_resolved_body_facts),
    );
}

fn emit_provider_demand_batch(timings: TimingMode, round: u64, demands: &[crate::ProviderDemand]) {
    if !timings.enabled() {
        return;
    }
    let mut methods = 0_u64;
    let mut trait_impls = 0_u64;
    let mut module_semantics = 0_u64;
    let mut module_bodies = 0_u64;
    for demand in demands {
        match demand.request {
            crate::ProviderRequest::Method { .. } => methods += 1,
            crate::ProviderRequest::TraitImpl { .. } => trait_impls += 1,
            crate::ProviderRequest::ModuleSemantic { .. } => module_semantics += 1,
            crate::ProviderRequest::ModuleBody { .. } => module_bodies += 1,
        }
    }
    let prefix = format!("compiler.executable_provider_demands.round_{round}");
    nia_timing::emit_counter(format!("{prefix}.total"), demands.len() as u64);
    nia_timing::emit_counter(format!("{prefix}.methods"), methods);
    nia_timing::emit_counter(format!("{prefix}.trait_impls"), trait_impls);
    nia_timing::emit_counter(format!("{prefix}.module_semantics"), module_semantics);
    nia_timing::emit_counter(format!("{prefix}.module_bodies"), module_bodies);
}

fn emit_check_certificate_reuse(timings: TimingMode, hit: bool) {
    if !timings.enabled() {
        return;
    }
    nia_timing::emit_counter("compiler.check_certificate_hits", u64::from(hit));
    nia_timing::emit_counter("compiler.check_certificate_misses", u64::from(!hit));
}

impl std::fmt::Debug for CompilerDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        f.debug_struct("CompilerDatabase")
            .field("optimization", &inputs.optimization)
            .finish_non_exhaustive()
    }
}

fn checked_provider_demands(program: &CheckedProgramAnalysis) -> Vec<crate::ProviderDemand> {
    program
        .modules
        .iter()
        .flat_map(|module| module.provider_demands.iter().cloned())
        .collect()
}

fn codegen_provider_demands(program: &CodegenProgram) -> Vec<crate::ProviderDemand> {
    program
        .modules
        .iter()
        .flat_map(|module| module.provider_demands.iter().cloned())
        .collect()
}

fn codegen_preparation_provider_demands(
    preparation: &CodegenPreparation,
) -> Vec<crate::ProviderDemand> {
    preparation
        .modules
        .iter()
        .flat_map(|module| module.provider_demands.iter().cloned())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompilerInvalidation {
    pub invalidated: Vec<QueryFrame>,
}

impl CompilerInvalidation {
    fn extend(&mut self, invalidation: nia_query::QueryInvalidation) {
        for frame in invalidation.invalidated {
            if !self.invalidated.contains(&frame) {
                self.invalidated.push(frame);
            }
        }
    }
}

fn compiler_database_with_providers(
    request: CompileRequest,
    providers: CompilerQueryProviders,
) -> CompilerDatabase {
    let session = request.loader_facts.query_session().unwrap_or_default();
    compiler_database_with_providers_in_session(request, providers, session)
}

fn compiler_database_with_providers_in_session(
    request: CompileRequest,
    providers: CompilerQueryProviders,
    session: nia_query::QuerySession,
) -> CompilerDatabase {
    let timings = request.timings;
    let signature_cache = request.frontend_cache_dir.as_ref().map(|root| {
        Arc::new(crate::signature_cache::PersistentSignatureCache::new(
            root.clone(),
        ))
    });
    let verify_frontend_cache = request.verify_frontend_cache;
    let loader_facts = Arc::clone(&request.loader_facts);
    let observed_graph = loader_facts
        .module_graph()
        .expect("initial compiler module graph");
    let observed_compiled_interfaces = compiled_interface_fingerprint(
        loader_facts
            .compiled_package_interfaces()
            .expect("initial compiled package interfaces"),
    )
    .expect("initial compiled package interface fingerprint");
    if let Some(loader_session) = loader_facts.query_session() {
        assert!(
            session.ptr_eq(&loader_session),
            "Nia ICE: compiler and loader facts must share one query session"
        );
    }
    let node_store = loader_facts.node_store();
    let inputs = Arc::new(RwLock::new(CompilerInputs::new(request)));
    let executable_fact_session = Arc::new(std::sync::Mutex::new(ExecutableFactSession::default()));
    let type_store = Arc::new(nia_ty::TypeStore::new());
    let db = QueryDb::new_registered_with_timings_in_session(
        CompilerContext {
            inputs: inputs.clone(),
            observed_graph: std::sync::Mutex::new(observed_graph),
            observed_compiled_interfaces: std::sync::Mutex::new(Some(observed_compiled_interfaces)),
            loader_facts,
            providers,
            executable_fact_session,
            executable_fact_scheduler: std::sync::Mutex::new(()),
            type_store,
            diagnostic_store: nia_diagnostic::DiagnosticStore::new(),
            node_store,
            signature_cache,
            verify_frontend_cache,
            provider_demand_rounds: std::sync::atomic::AtomicU64::new(0),
        },
        timings,
        compiler_query_registry(),
        session,
    );
    CompilerDatabase { db, inputs }
}

pub(crate) fn query_error_diagnostic(err: QueryError) -> Diagnostic {
    match err {
        QueryError::Cycle { cycle } => {
            let mut message = String::from("query cycle detected");
            for frame in cycle {
                message.push_str("\n  ");
                message.push_str(&frame.description);
            }
            Diagnostic::internal_error(codes::QUERY_ENGINE, message)
                .primary_fallback(Span::default(), "query cycle has no source span")
                .finish()
        }
        QueryError::InvalidInput { query, message } => {
            let message = format!("invalid query input for {}: {message}", query.description);
            Diagnostic::internal_error(codes::QUERY_ENGINE, message)
                .primary_fallback(Span::default(), "query input has no source span")
                .finish()
        }
    }
}

fn stable_module_sequence(
    db: &QueryDb<CompilerContext>,
    module_ids: impl IntoIterator<Item = ModuleId>,
) -> QueryResult<StableModuleSequence> {
    db.context().stable_module_sequence(module_ids)
}

fn resolve_stable_module_sequence_from_current_inputs(
    db: &QueryDb<CompilerContext>,
    sequence: &StableModuleSequence,
) -> QueryResult<Vec<ModuleId>> {
    db.context().resolve_stable_module_sequence(sequence)
}

fn resolve_stable_module_sequence(
    db: &QueryDb<CompilerContext>,
    sequence: &StableModuleSequence,
) -> QueryResult<Vec<ModuleId>> {
    let _graph = db.get(ModuleGraphQuery)?;
    db.context().resolve_stable_module_sequence(sequence)
}

fn provider_fact_worklist_fingerprint(worklist: &crate::ProviderFactSnapshot) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(PROVIDER_FACT_WORKLIST_DOMAIN);
    builder.write_fingerprint(provider_fact_revision_fingerprint(worklist.revision()));
    builder.write_fingerprint(provider_fact_revision_fingerprint(
        worklist.reset_revision(),
    ));
    let mut changes = worklist
        .demands()
        .iter()
        .map(provider_demand_fingerprint)
        .collect::<Vec<_>>();
    changes.sort_unstable();
    builder.write_u64(changes.len() as u64);
    for change in changes {
        builder.write_fingerprint(change);
    }
    builder.finish()
}

fn provider_fact_revision_fingerprint(revision: crate::ProviderFactRevision) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(PROVIDER_FACT_REVISION_DOMAIN);
    for part in revision.fingerprint_parts() {
        builder.write_u64(part);
    }
    builder.finish()
}

fn provider_demand_fingerprint(demand: &crate::ProviderDemand) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(PROVIDER_DEMAND_DOMAIN);
    builder.write_str(demand.source_path.identity().normalized_path());
    match &demand.request {
        crate::ProviderRequest::Method {
            target_type_name,
            method_name,
        } => {
            builder.write_u8(0);
            if let Some(target_type_name) = target_type_name {
                builder.write_u8(1);
                builder.write_u64(target_type_name.raw());
            } else {
                builder.write_u8(0);
            }
            builder.write_u64(method_name.raw());
        }
        crate::ProviderRequest::TraitImpl {
            target_type_name,
            trait_name,
        } => {
            builder.write_u8(1);
            if let Some(target_type_name) = target_type_name {
                builder.write_u8(1);
                builder.write_u64(target_type_name.raw());
            } else {
                builder.write_u8(0);
            }
            builder.write_u64(trait_name.raw());
        }
        crate::ProviderRequest::ModuleSemantic { module_path } => {
            builder.write_u8(2);
            builder.write_str(module_path.identity().normalized_path());
        }
        crate::ProviderRequest::ModuleBody { module_path } => {
            builder.write_u8(3);
            builder.write_str(module_path.identity().normalized_path());
        }
    }
    builder.finish()
}

fn check_certificate_input_fingerprint(
    program_sources: crate::FrontendProgramSourceFingerprint,
    graph: &nia_imports::ModuleGraphSnapshot,
    provider_facts: &crate::ProviderFactSnapshot,
) -> FrontendCheckInputFingerprint {
    let mut builder = QueryFingerprintBuilder::new(CHECK_CERTIFICATE_INPUT_DOMAIN);
    builder.write_fingerprint(QueryFingerprint::from_parts(program_sources.parts()));
    let mut modules = graph.modules().collect::<Vec<_>>();
    modules.sort_unstable_by(|left, right| {
        left.stable_key
            .source_identity()
            .normalized_path()
            .cmp(right.stable_key.source_identity().normalized_path())
    });
    builder.write_u64(modules.len() as u64);
    for module in modules {
        builder.write_str(module.stable_key.source_identity().normalized_path());
        builder.write_u64(module.module_path.package.raw());
        builder.write_u64(module.module_path.segments.len() as u64);
        for segment in &module.module_path.segments {
            builder.write_u64(segment.raw());
        }
        if let Some(parent) = module.parent {
            builder.write_u8(1);
            builder.write_str(
                graph
                    .stable_key(parent)
                    .expect("module graph parent must have stable identity")
                    .source_identity()
                    .normalized_path(),
            );
        } else {
            builder.write_u8(0);
        }
        builder.write_u8(u8::from(graph.is_executable_root_module(module.id)));
        let mut declarations = module
            .declarations
            .iter()
            .map(|declaration| {
                (
                    declaration.name.raw(),
                    visibility_tag(declaration.visibility),
                    graph
                        .stable_key(declaration.target)
                        .expect("module declaration target must have stable identity")
                        .source_identity()
                        .normalized_path()
                        .to_owned(),
                )
            })
            .collect::<Vec<_>>();
        declarations.sort_unstable();
        builder.write_u64(declarations.len() as u64);
        for (name, visibility, target) in declarations {
            builder.write_u64(name);
            builder.write_u8(visibility);
            builder.write_str(&target);
        }
    }
    let mut demands = provider_facts
        .demands()
        .iter()
        .map(provider_demand_fingerprint)
        .collect::<Vec<_>>();
    demands.sort_unstable();
    builder.write_u64(demands.len() as u64);
    for demand in demands {
        builder.write_fingerprint(demand);
    }
    FrontendCheckInputFingerprint::from_parts(builder.finish().parts())
}

fn visibility_tag(visibility: nia_ids::Visibility) -> u8 {
    match visibility {
        nia_ids::Visibility::Private => 0,
        nia_ids::Visibility::PublicSuper => 1,
        nia_ids::Visibility::PublicPkg => 2,
        nia_ids::Visibility::Public => 3,
    }
}

fn module_graph_path_fingerprint(path: &Option<nia_imports::ModulePath>) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(MODULE_GRAPH_PATH_DOMAIN);
    let Some(path) = path else {
        builder.write_u8(0);
        return builder.finish();
    };
    builder.write_u8(1);
    builder.write_u64(path.package.raw());
    builder.write_u64(path.segments.len() as u64);
    for segment in &path.segments {
        builder.write_u64(segment.raw());
    }
    builder.finish()
}

fn stable_module_key_fingerprint(
    domain: FingerprintDomain,
    key: &StableModuleKey,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    write_stable_module_key(&mut builder, key);
    builder.finish()
}

fn optional_stable_module_key_fingerprint(
    domain: FingerprintDomain,
    key: Option<&StableModuleKey>,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    if let Some(key) = key {
        builder.write_u8(1);
        write_stable_module_key(&mut builder, key);
    } else {
        builder.write_u8(0);
    }
    builder.finish()
}

fn module_graph_child_fingerprint(
    child: &Option<(StableModuleKey, nia_ids::Visibility)>,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(MODULE_GRAPH_CHILD_DOMAIN);
    let Some((key, visibility)) = child else {
        builder.write_u8(0);
        return builder.finish();
    };
    builder.write_u8(1);
    write_stable_module_key(&mut builder, key);
    builder.write_u8(match visibility {
        nia_ids::Visibility::Private => 0,
        nia_ids::Visibility::PublicSuper => 1,
        nia_ids::Visibility::PublicPkg => 2,
        nia_ids::Visibility::Public => 3,
    });
    builder.finish()
}

fn write_stable_module_key(builder: &mut QueryFingerprintBuilder, key: &StableModuleKey) {
    builder.write_str(key.source_identity().normalized_path());
}

fn stable_module_sequence_fingerprint(
    domain: FingerprintDomain,
    sequence: &StableModuleSequence,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_u64(sequence.keys.len() as u64);
    for key in &sequence.keys {
        write_stable_module_key(&mut builder, key);
    }
    builder.finish()
}

fn source_path_fingerprint(domain: FingerprintDomain, path: &SourcePath) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_str(path.as_str());
    builder.finish()
}

fn source_version_fingerprint(
    domain: FingerprintDomain,
    version: SourceVersion,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_u64(u64::from(version.id.0));
    builder.write_u64(version.revision.0);
    builder.finish()
}

fn provider_summary_fingerprint(
    summary: &nia_provider_summary::ProviderSummary,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(PROVIDER_SUMMARY_DOMAIN);
    builder.write_u64(summary.providers().len() as u64);
    for provider in summary.providers() {
        provider_type_ref_fingerprint(&mut builder, &provider.target.ty);
        if let Some(trait_ref) = &provider.trait_ref {
            builder.write_u8(1);
            provider_type_ref_fingerprint(&mut builder, trait_ref);
        } else {
            builder.write_u8(0);
        }
        builder.write_u64(provider.associated_methods.len() as u64);
        for method in &provider.associated_methods {
            builder.write_u64(method.raw());
        }
        builder.write_u64(provider.associated_values.len() as u64);
        for value in &provider.associated_values {
            builder.write_u64(value.raw());
        }
    }
    builder.finish()
}

fn provider_type_ref_fingerprint(
    builder: &mut QueryFingerprintBuilder,
    type_ref: &nia_provider_summary::ProviderTypeRef,
) {
    if let Some(last_name) = type_ref.last_name {
        builder.write_u8(1);
        builder.write_u64(last_name.raw());
    } else {
        builder.write_u8(0);
    }
    builder.write_u8(u8::from(type_ref.is_generic_or_structural_target));
    builder.write_u8(u8::from(type_ref.semantic_is_conservative));
}

fn bool_query_fingerprint(domain: FingerprintDomain, value: bool) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_u8(u8::from(value));
    builder.finish()
}

fn compiled_interface_index_fingerprint(
    index: &CompiledPackageInterfaceIndex,
) -> Option<QueryFingerprint> {
    let mut builder = QueryFingerprintBuilder::new(COMPILED_INTERFACE_INDEX_DOMAIN);
    builder.write_u64(index.packages.len() as u64);
    for (package, interface) in &index.packages {
        builder.write_str(&package.namespace);
        builder.write_str(&package.name);
        builder.write_str(&package.version);
        builder.write_u64(interface.records().len() as u64);
        for record in interface.records() {
            builder.write_str(&record.definition.module.path);
            builder.write_str(&record.definition.name);
            builder.write_u8(record.definition.kind);
            builder.write_bytes(&record.declaration);
            builder.write_u64(record.type_roots.len() as u64);
            for root in &record.type_roots {
                builder.write_u64(u64::from(*root));
            }
        }
        if let Some(graph) = interface.type_graph() {
            builder.write_u8(1);
            let graph_bytes = nia_package_metadata::encode_type_graph(graph).ok()?;
            builder.write_bytes(&graph_bytes);
        } else {
            builder.write_u8(0);
        }
        if let Some(templates) = interface.templates() {
            builder.write_u8(1);
            builder.write_u64(templates.records.len() as u64);
            for record in &templates.records {
                builder.write_str(&record.definition.module.path);
                builder.write_str(&record.definition.name);
                builder.write_u8(record.definition.kind);
                builder.write_bytes(&record.body);
                builder.write_bytes(&record.summary);
            }
        } else {
            builder.write_u8(0);
        }
    }
    Some(builder.finish())
}

fn compiled_interface_fingerprint(
    interfaces: Vec<nia_package_metadata::CompiledPackageInterface>,
) -> QueryResult<QueryFingerprint> {
    let index = CompiledPackageInterfaceIndex::from_interfaces(interfaces).map_err(|error| {
        QueryError::InvalidInput {
            query: QueryFrame {
                name: "compiled_package_interface_index",
                key: "compiled_package_interface_index".to_string(),
                description: "compiled_package_interface_index".to_string(),
            },
            message: error,
        }
    })?;
    compiled_interface_index_fingerprint(&index).ok_or_else(|| QueryError::InvalidInput {
        query: QueryFrame {
            name: "compiled_package_interface_index",
            key: "compiled_package_interface_index".to_string(),
            description: "compiled_package_interface_index".to_string(),
        },
        message: "failed to encode compiled interface fingerprint".to_string(),
    })
}

impl CompilerContext {
    fn loader_facts(&self) -> &dyn crate::LoaderFactProvider {
        self.loader_facts.as_ref()
    }

    fn type_store(&self) -> &nia_ty::TypeStore {
        &self.type_store
    }

    fn node_store(&self) -> &nia_node_id::NodeStore {
        &self.node_store
    }

    fn frontend_program_sources(
        &self,
        db: &QueryDb<CompilerContext>,
    ) -> QueryResult<Option<FrontendProgramSources>> {
        let modules = db.get(LoadedModulesQuery)?;
        let module_ids = resolve_stable_module_sequence_from_current_inputs(db, &modules)?;
        let mut by_module = HashMap::new();
        let mut module_by_path = HashMap::new();
        let mut path_by_module = HashMap::new();
        let mut fingerprint_inputs = Vec::new();
        for module_id in module_ids {
            let path = db.get(ModulePathQuery(module_id))?;
            let version = *db.get(ModuleSourceVersionQuery(module_id))?;
            let Some((source, len)) = self.loader_facts.module_source_fingerprint(module_id)?
            else {
                return Ok(None);
            };
            let module = StableModuleKey::from_source_identity(path.identity());
            let normalized_path = module.source_identity().normalized_path().to_string();
            if module_by_path
                .insert(normalized_path.clone(), module_id)
                .is_some()
                || path_by_module.insert(module_id, normalized_path).is_some()
            {
                return Ok(None);
            }
            fingerprint_inputs.push((module.clone(), source, len));
            by_module.insert(
                module_id,
                FrontendProgramSource {
                    module,
                    version,
                    len,
                },
            );
        }
        let fingerprint = crate::frontend_program_source_fingerprint(
            fingerprint_inputs
                .iter()
                .map(|(module, source, len)| (module, *source, *len)),
        );
        Ok(Some(FrontendProgramSources {
            fingerprint,
            by_module,
            module_by_path,
            path_by_module,
        }))
    }

    fn stable_module_sequence(
        &self,
        module_ids: impl IntoIterator<Item = ModuleId>,
    ) -> QueryResult<StableModuleSequence> {
        let graph = self.loader_facts.module_graph()?;
        let mut identities = Vec::new();
        for module_id in module_ids {
            graph
                .get(module_id)
                .unwrap_or_else(|| panic!("Nia ICE: module {module_id:?} is not loaded"));
            let path = self
                .loader_facts
                .module_path(module_id)?
                .unwrap_or_else(|| panic!("Nia ICE: module {module_id:?} has no source path"));
            identities.push(path.identity());
        }
        Ok(StableModuleSequence::from_source_identities(identities))
    }

    fn resolve_stable_module_sequence(
        &self,
        sequence: &StableModuleSequence,
    ) -> QueryResult<Vec<ModuleId>> {
        let graph = self.loader_facts.module_graph()?;
        let mut module_ids = Vec::with_capacity(sequence.keys.len());
        for key in &sequence.keys {
            let mut current = None;
            for module in graph.modules() {
                if self
                    .loader_facts
                    .module_path(module.id)?
                    .is_some_and(|path| path.identity() == *key.source_identity())
                {
                    current = Some(module.id);
                    break;
                }
            }
            module_ids.push(current.unwrap_or_else(|| {
                panic!(
                    "Nia ICE: stable loaded module `{}` is missing from current loader facts",
                    key.source_identity().normalized_path()
                )
            }));
        }
        Ok(module_ids)
    }

    fn module_path(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<SourcePath> {
        self.loader_facts().module_path(module_id)?.ok_or_else(|| {
            db.invalid_input(
                &ModulePathQuery(module_id),
                format!("missing loaded module {module_id:?}"),
            )
        })
    }

    fn module_source_version(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<SourceVersion> {
        self.loader_facts()
            .module_source_version(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &ModuleSourceVersionQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn module_origins(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<NodeOriginTable> {
        self.loader_facts()
            .module_origins(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &ModuleOriginsQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn module_parse_errors(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<Vec<ParseError>> {
        self.loader_facts()
            .module_parse_errors(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &ModuleParseErrorsQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ModuleItemTree> {
        self.loader_facts()
            .module_item_tree(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &ModuleItemTreeInputQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn declaration_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ModuleItemTree> {
        self.loader_facts()
            .module_item_tree(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &DeclarationModuleItemTreeInputQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn full_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ModuleItemTree> {
        self.loader_facts()
            .module_item_tree(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &FullModuleItemTreeInputQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn active_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ActiveModuleItemTree> {
        self.loader_facts()
            .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::Full)?
            .ok_or_else(|| {
                db.invalid_input(
                    &ActiveModuleItemTreeInputQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn declaration_active_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ActiveModuleItemTree> {
        self.loader_facts()
            .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::Full)?
            .ok_or_else(|| {
                db.invalid_input(
                    &DeclarationActiveModuleItemTreeInputQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn full_active_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ActiveModuleItemTree> {
        self.loader_facts()
            .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::Full)?
            .ok_or_else(|| {
                db.invalid_input(
                    &FullActiveModuleItemTreeInputQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn signature_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
        set: nia_item_tree::SignatureItemSet,
    ) -> QueryResult<ActiveModuleItemTree> {
        self.loader_facts()
            .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::Signature(set))?
            .ok_or_else(|| {
                db.invalid_input(
                    &SignatureItemTreeQuery(module_id, set),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn signature_const_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ActiveModuleItemTree> {
        self.loader_facts()
            .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::ConstSignature)?
            .ok_or_else(|| {
                db.invalid_input(
                    &SignatureConstItemTreeQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn module_provider_summary(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<nia_provider_summary::ProviderSummary> {
        self.loader_facts()
            .module_provider_summary(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &ExtensionProviderSummaryQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    fn module_id_for_stable_key(
        &self,
        stable_key: &StableModuleKey,
    ) -> QueryResult<Option<ModuleId>> {
        Ok(self
            .loader_facts
            .module_graph()?
            .module_id_for_stable_key(stable_key))
    }

    fn symbols(&self) -> nia_symbol_table::SymbolTable {
        self.loader_facts().symbols()
    }

    fn provider_fact_worklist(&self) -> QueryResult<crate::ProviderFactSnapshot> {
        self.loader_facts().provider_facts()
    }

    fn optimization(&self) -> OptimizationPolicy {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .optimization
    }

    fn timings(&self) -> TimingMode {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .timings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RuntimeModel;
    use nia_sema_ir::SemanticValueUse;
    use nia_source::{SourceId, SourceIdentity, SourceRevision};
    #[path = "backend_closure.rs"]
    mod backend_closure;
    #[path = "backend_orchestration.rs"]
    mod backend_orchestration;
    #[path = "backend_stage.rs"]
    mod backend_stage;
    #[path = "body_invalidation.rs"]
    mod body_invalidation;
    #[path = "checked_products.rs"]
    mod checked_products;
    #[path = "closure_safety.rs"]
    mod closure_safety;
    #[path = "compiler_contracts.rs"]
    mod compiler_contracts;
    #[path = "compiler_incremental_consistency.rs"]
    mod compiler_incremental_consistency;
    #[path = "const_semantic_dependencies.rs"]
    mod const_semantic_dependencies;
    #[path = "database.rs"]
    mod database;
    #[path = "definition_invalidation.rs"]
    mod definition_invalidation;
    #[path = "executable_const.rs"]
    mod executable_const;
    #[path = "executable_const_metadata.rs"]
    mod executable_const_metadata;
    #[path = "executable_empty_body.rs"]
    mod executable_empty_body;
    #[path = "executable_entry_reachability.rs"]
    mod executable_entry_reachability;
    #[path = "executable_filtering.rs"]
    mod executable_filtering;
    #[path = "executable_generic_reachability.rs"]
    mod executable_generic_reachability;
    #[path = "executable_initializers.rs"]
    mod executable_initializers;
    #[path = "executable_type_only.rs"]
    mod executable_type_only;
    #[path = "executable_value_refs.rs"]
    mod executable_value_refs;
    #[path = "extension_dependencies.rs"]
    mod extension_dependencies;
    #[path = "extension_nominal_queries.rs"]
    mod extension_nominal_queries;
    #[path = "extension_provider_refresh.rs"]
    mod extension_provider_refresh;
    #[path = "fixture.rs"]
    mod fixture;
    #[path = "frontend_cache.rs"]
    mod frontend_cache;
    #[path = "frontend_invalidation.rs"]
    mod frontend_invalidation;
    #[path = "frontend_membership.rs"]
    mod frontend_membership;
    #[path = "frontend_products.rs"]
    mod frontend_products;
    #[path = "incremental_extension_body.rs"]
    mod incremental_extension_body;
    #[path = "incremental_static_initializers.rs"]
    mod incremental_static_initializers;
    #[path = "loader_facts.rs"]
    mod loader_facts;
    #[path = "persistent_cache.rs"]
    mod persistent_cache;
    #[path = "provider_incremental.rs"]
    mod provider_incremental;
    #[path = "query_handle_reuse.rs"]
    mod query_handle_reuse;
    #[path = "semantic_diagnostics.rs"]
    mod semantic_diagnostics;
    #[path = "semantic_query_dependencies.rs"]
    mod semantic_query_dependencies;
    #[path = "semantic_query_scope.rs"]
    mod semantic_query_scope;
    #[path = "signature_query_dependencies.rs"]
    mod signature_query_dependencies;
    #[path = "support.rs"]
    mod support;
    #[path = "type_store_session.rs"]
    mod type_store_session;
    #[path = "visible_extensions.rs"]
    mod visible_extensions;
    use database::CompilerDatabase;
    use fixture::*;
    use frontend_cache::*;
    use loader_facts::*;
    use support::*;

    struct VtableFunctionInstanceRef<'a> {
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        self_arg: Option<InternedTyId>,
        args: &'a [InternedTyId],
        const_args: &'a [nia_ty::ConstGenericArg],
    }

    fn backend_function_instance_matches_vtable_ref(
        type_store: &nia_ty::TypeStore,
        vtable: VtableFunctionInstanceRef<'_>,
        instance: &nia_backend_ir::BackendFunctionInstance,
    ) -> bool {
        let VtableFunctionInstanceRef {
            def_id,
            arg_module_id,
            self_arg,
            args,
            const_args,
        } = vtable;
        if instance.def_id != def_id || instance.arg_module_id != arg_module_id {
            return false;
        }
        if self_arg.is_some_and(|ty| type_store.get(ty).is_none())
            || args.iter().any(|ty| type_store.get(*ty).is_none())
            || const_args
                .iter()
                .any(|arg| type_store.get(arg.ty).is_none())
        {
            return false;
        }
        self_arg == instance.self_arg && args == instance.args && const_args == instance.const_args
    }
}
