// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    ActiveModuleItemTreeFactKind, CheckedModule, CheckedProgram, CheckedProgramAnalysis,
    CodegenPreparation, CodegenProgram, FrontendCheckInputFingerprint, FrontendCheckScope,
    ProgramDiagnostic, ProgramDiagnosticBundles, RuntimeSpec, TimingMode, module_diagnostics,
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
    DefinitionId, ModuleId as StableModuleId, PackageId, StableArrayLength,
    StableAssociatedTypeBinding, StableConstArg, StableConstValue, StableTraitId, StableTypeGraph,
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
    FingerprintDomain::new("nia.compiler.provider-fact-worklist");
const PROVIDER_FACT_REVISION_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.provider-fact-revision");
const PROVIDER_DEMAND_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.provider-demand");
const CHECK_CERTIFICATE_INPUT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.check-certificate-input");
const MODULE_GRAPH_PATH_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-graph-path");
const MODULE_GRAPH_ENTRY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-graph-entry");
const MODULE_GRAPH_PARENT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-graph-parent");
const MODULE_GRAPH_CHILD_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-graph-child");
const MODULE_GRAPH_PROVIDER_DEPENDENCIES_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-graph-provider-dependencies");
const MODULE_PACKAGE_ROOT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-package-root");
const LOADED_MODULES_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.loaded-modules");
const PARSE_OK_MODULE_IDS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.parse-ok-module-ids");
const SEMANTIC_MODULE_IDS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.semantic-module-ids");
const MODULE_SOURCE_PATH_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-source-path");
const MODULE_SOURCE_VERSION_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-source-version");
const PUBLIC_SURFACE_MODULE_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.public-surface-module");
const USING_SCOPE_MODULE_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.using-scope-module");
const PROGRAM_SIGNATURE_MODULE_IDS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.program-signature-module-ids");
const PROGRAM_SIGNATURE_MODULE_ELIGIBILITY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.program-signature-module-eligibility");
const EXTENSION_PROVIDER_MODULE_IDS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.extension-provider-module-ids");
const EXTENSION_PROVIDER_MODULE_ELIGIBILITY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.extension-provider-module-eligibility");
const PROVIDER_SUMMARY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.provider-summary");
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

/// Resolves a stable definition identity into the current compiler
/// session. Implementations are responsible for remapping package/module
/// identities; no physical path lookup is implied by this trait.
pub trait StableDefinitionResolver {
    fn definition_for_identity(&self, definition: &DefinitionId) -> QueryResult<GlobalDefId>;
}

/// Session-local remap table for stable package definition identities.
///
/// The table is built from the current module/definition facts and an
/// explicit package resolver. It is the only supported bridge from immutable
/// package identities to transient `ModuleId`/`DefId` handles.
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

    /// Iterates every stable definition remapped in this session.
    pub fn iter(&self) -> impl Iterator<Item = (&DefinitionId, &GlobalDefId)> {
        self.definitions.iter()
    }

    /// Resolves a stable module identity to its current session handle.
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
        let resolved = self.definition(definition).or_else(|| {
            (definition.disambiguator == 0)
                .then(|| {
                    self.definitions
                        .iter()
                        .filter(|(candidate, _)| {
                            candidate.module == definition.module
                                && candidate.name == definition.name
                                && candidate.kind == definition.kind
                                && candidate.owner == definition.owner
                        })
                        .map(|(_, resolved)| *resolved)
                        .collect::<Vec<_>>()
                })
                .and_then(|matches| (matches.len() == 1).then_some(matches[0]))
        });
        resolved.ok_or_else(|| QueryError::InvalidInput {
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

/// Resolves one loaded source definition through the query graph.
///
/// This is the query-side counterpart of [`CompilerDatabase::resolve_loaded_definition`].
/// Keeping the remap algorithm here lets provider queries consume stable
/// stable identities without manufacturing a second, name-only lookup path.
pub(in crate::query) fn resolve_loaded_definition_in_query(
    db: &QueryDb<CompilerContext>,
    definition: &DefinitionId,
    package: &PackageId,
) -> QueryResult<GlobalDefId> {
    if &definition.module.package != package {
        return Err(db.invalid_input(
            &ModuleGraphQuery,
            "stable definition belongs to a different package".to_string(),
        ));
    }
    let graph = db.get(ModuleGraphQuery)?;
    let module_id = graph.module_id_for_path(&definition.module.path);
    let Some(module_id) = module_id else {
        return Err(db.invalid_input(
            &ModuleGraphQuery,
            format!(
                "stable definition module is not loaded: {}",
                definition.module.path
            ),
        ));
    };
    let defs = db.get(FullModuleDefsQuery(module_id))?;
    let symbols = db.context().loader_facts().symbols();
    let matches = defs
        .semantic
        .defs
        .iter()
        .filter_map(|(def_id, def)| {
            if def_kind_tag(def.kind) != definition.kind
                || !symbols
                    .resolve(def.name)
                    .is_some_and(|name| name.as_ref() == definition.name.as_str())
            {
                return None;
            }
            let mut owner_chain = Vec::new();
            let mut parent = def.parent;
            while let Some(parent_id) = parent {
                let parent_def = defs.semantic.defs.get(parent_id)?;
                let parent_name = symbols.resolve(parent_def.name)?;
                owner_chain.push((
                    parent_name.to_string(),
                    def_kind_tag(parent_def.kind),
                    parent_id.0,
                ));
                parent = parent_def.parent;
            }
            let mut owner_identity = None;
            for (owner_name, owner_kind, owner_disambiguator) in owner_chain.into_iter().rev() {
                owner_identity = Some(Box::new(DefinitionId {
                    module: definition.module.clone(),
                    name: owner_name,
                    kind: owner_kind,
                    disambiguator: owner_disambiguator,
                    owner: owner_identity,
                }));
            }
            (definition.disambiguator == 0 || definition.disambiguator == def_id.0)
                .then_some((owner_identity, GlobalDefId { module_id, def_id }))
        })
        .filter(|(owner_identity, _)| owner_identity == &definition.owner)
        .map(|(_, resolved)| resolved)
        .collect::<Vec<_>>();
    let [resolved] = matches.as_slice() else {
        return Err(db.invalid_input(
            &ModuleGraphQuery,
            format!(
                "stable definition is missing or ambiguous: {}::{}",
                definition.module.path, definition.name
            ),
        ));
    };
    Ok(*resolved)
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
    /// Definition-root scope used by executable and package code generation.
    pub codegen_scope: crate::CodegenScope,
    /// Canonical identity of the current source package for package-qualified
    /// symbols. Standalone source compilations derive an anonymous identity
    /// from their stable source root.
    pub current_package: Option<PackageId>,
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
            codegen_scope: crate::CodegenScope::Entry,
            current_package: None,
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

    /// Selects whether code generation starts from an entry or the complete
    /// concrete definition inventory of the current package.
    pub fn with_codegen_scope(mut self, scope: crate::CodegenScope) -> Self {
        self.codegen_scope = scope;
        self
    }

    /// Binds current-source linkage to a canonical package identity.
    pub fn with_current_package(mut self, package: Option<PackageId>) -> Self {
        self.current_package = package;
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
    fn rehydrate_stable_trait_id(
        &self,
        trait_id: &StableTraitId,
        resolver: &dyn StableDefinitionResolver,
    ) -> QueryResult<nia_ty::TraitId> {
        Ok(match trait_id {
            StableTraitId::Source(definition) => {
                nia_ty::TraitId::Source(resolver.definition_for_identity(definition)?)
            }
            StableTraitId::Builtin(tag) => {
                nia_ty::TraitId::Builtin(stable_builtin_trait(*tag).ok_or_else(|| {
                    self.db.invalid_input(
                        &ModuleGraphQuery,
                        "unknown stable builtin trait tag".to_string(),
                    )
                })?)
            }
        })
    }

    fn rehydrate_stable_const_args(
        &self,
        arguments: &[StableConstArg],
        types: &[nia_ids::InternedTyId],
    ) -> QueryResult<Vec<nia_ty::ConstGenericArg>> {
        arguments
            .iter()
            .map(|argument| {
                let StableConstArg { ty, value } = argument;
                Ok(nia_ty::ConstGenericArg {
                    ty: types.get(*ty as usize).copied().ok_or_else(|| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            "stable const argument type is outside graph".to_string(),
                        )
                    })?,
                    value: match value {
                        StableConstValue::GenericParam(hash) => {
                            nia_ty::ConstGenericValue::GenericParam(SymbolId::from_stable_hash(
                                *hash,
                            ))
                        }
                        StableConstValue::Integer { bits, signed } => {
                            nia_ty::ConstGenericValue::Int(if *signed {
                                nia_ty::IntConst::signed_bits(*bits)
                            } else {
                                nia_ty::IntConst::unsigned(*bits)
                            })
                        }
                        StableConstValue::Bool(value) => nia_ty::ConstGenericValue::Bool(*value),
                        StableConstValue::Char(value) => nia_ty::ConstGenericValue::Char(*value),
                    },
                })
            })
            .collect()
    }

    fn rehydrate_stable_bindings(
        &self,
        bindings: &[StableAssociatedTypeBinding],
        types: &[nia_ids::InternedTyId],
        resolver: &dyn StableDefinitionResolver,
    ) -> QueryResult<Vec<nia_ty::AssociatedTypeBindingTy>> {
        bindings
            .iter()
            .map(|binding| {
                Ok(nia_ty::AssociatedTypeBindingTy {
                    trait_id: binding
                        .trait_id
                        .as_ref()
                        .map(|trait_id| self.rehydrate_stable_trait_id(trait_id, resolver))
                        .transpose()?,
                    trait_args: binding
                        .trait_arguments
                        .iter()
                        .map(|index| types[usize::try_from(*index).unwrap()])
                        .collect(),
                    trait_const_args: self
                        .rehydrate_stable_const_args(&binding.trait_const_arguments, types)?,
                    name: SymbolId::from_stable_hash(binding.name),
                    ty: types[usize::try_from(binding.ty).unwrap()],
                })
            })
            .collect()
    }

    /// Creates a compiler database and registers the complete query provider graph.
    pub fn new(request: CompileRequest) -> QueryResult<Self> {
        compiler_database_with_providers(request, CompilerQueryProviders::default())
    }

    #[cfg(test)]
    pub(super) fn new_for_test(request: CompileRequest) -> Self {
        Self::new(request)
            .unwrap_or_else(|error| panic!("failed to create compiler database: {error}"))
    }

    /// Returns the shared session that owns this database and its loader facts.
    pub fn query_session(&self) -> nia_query::QuerySession {
        self.db.session()
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
                // The remap index includes private definitions as well: public
                // aggregate layout and enum payloads may depend on them.
                let Some(name) = symbols.resolve(def.name) else {
                    continue;
                };
                let package = resolver.package_for_definition(GlobalDefId {
                    module_id: module.id,
                    def_id,
                })?;
                let mut owner_chain = Vec::new();
                let mut parent = def.parent;
                while let Some(parent_id) = parent {
                    let parent_def = facts.semantic.defs.get(parent_id).ok_or_else(|| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            "definition parent is missing from module facts".to_string(),
                        )
                    })?;
                    let parent_name = symbols.resolve(parent_def.name).ok_or_else(|| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            "definition parent has no resolvable symbol".to_string(),
                        )
                    })?;
                    owner_chain.push((
                        parent_name.to_string(),
                        def_kind_tag(parent_def.kind),
                        parent_id.0,
                    ));
                    parent = parent_def.parent;
                }
                let mut owner_identity = None;
                for (owner_name, owner_kind, owner_disambiguator) in owner_chain.into_iter().rev() {
                    owner_identity = Some(Box::new(DefinitionId {
                        module: StableModuleId {
                            package: package.clone(),
                            path: module_path.clone(),
                        },
                        name: owner_name,
                        kind: owner_kind,
                        disambiguator: owner_disambiguator,
                        owner: owner_identity,
                    }));
                }
                let identity = DefinitionId {
                    module: StableModuleId {
                        package,
                        path: module_path.clone(),
                    },
                    name: name.to_string(),
                    kind: def_kind_tag(def.kind),
                    disambiguator: def_id.0,
                    owner: owner_identity,
                };
                let global = GlobalDefId {
                    module_id: module.id,
                    def_id,
                };
                let module_identity = identity.module.clone();
                if let Some(previous_package) =
                    module_owners.insert(module.id, identity.module.package.clone())
                    && previous_package != identity.module.package
                {
                    return Err(self.db.invalid_input(
                        &ModuleGraphQuery,
                        format!(
                            "stable module identity resolves to multiple packages: module {:?}, packages {:?} and {:?}",
                            module.id, previous_package, identity.module.package
                        ),
                    ));
                }
                if let Some(previous) = modules.insert(module_identity.clone(), module.id)
                    && previous != module.id
                {
                    return Err(self.db.invalid_input(
                        &ModuleGraphQuery,
                        format!(
                            "stable module identity resolves to multiple session modules: {:?}",
                            module_identity
                        ),
                    ));
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
        let types = self.rehydrate_stable_type_graph_nodes(graph, resolver)?;
        Ok(graph
            .roots
            .iter()
            .map(|root| types[usize::try_from(*root).unwrap()])
            .collect())
    }

    /// Rehydrates every node in a stable package graph, preserving its wire
    /// index so typed signature payloads can reference arbitrary nodes.
    fn rehydrate_stable_type_graph_nodes(
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
                    const_args: self.rehydrate_stable_const_args(const_arguments, &types)?,
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
                    len: match length {
                        StableArrayLength::ConstValue(value) => {
                            nia_ty::ArrayLenTy::ConstValue(*value)
                        }
                        StableArrayLength::GenericParam(hash) => {
                            nia_ty::ArrayLenTy::GenericParam(SymbolId::from_stable_hash(*hash))
                        }
                    },
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
                StableTypeNode::Error => append.intern(nia_ty::TyKind::Error),
                StableTypeNode::ConstOnly => append.intern(nia_ty::TyKind::ConstOnly),
                StableTypeNode::Opaque => append.intern(nia_ty::TyKind::Opaque),
                StableTypeNode::VolatilePointer { target, readonly } => {
                    append.intern(nia_ty::TyKind::VolatilePointer {
                        is_readonly: *readonly,
                        elem: types[usize::try_from(*target).unwrap()],
                    })
                }
                StableTypeNode::Slice { target, readonly } => {
                    append.intern(nia_ty::TyKind::Slice {
                        is_readonly: *readonly,
                        elem: types[usize::try_from(*target).unwrap()],
                    })
                }
                StableTypeNode::SlicePointee { target } => {
                    append.intern(nia_ty::TyKind::SlicePointee {
                        elem: types[usize::try_from(*target).unwrap()],
                    })
                }
                StableTypeNode::Vector { element, lanes } => {
                    append.intern(nia_ty::TyKind::Vector {
                        elem: stable_primitive_from_tag(*element).ok_or_else(|| {
                            self.db.invalid_input(
                                &ModuleGraphQuery,
                                "unknown stable vector element tag".to_string(),
                            )
                        })?,
                        lanes: *lanes,
                    })
                }
                StableTypeNode::Range { kind, bound } => append.intern(nia_ty::TyKind::Range {
                    kind: stable_range_kind(*kind).ok_or_else(|| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            "unknown stable range kind".to_string(),
                        )
                    })?,
                    bound: bound.map(|index| types[usize::try_from(index).unwrap()]),
                }),
                StableTypeNode::Optional { element } => append.intern(nia_ty::TyKind::Optional {
                    elem: types[usize::try_from(*element).unwrap()],
                }),
                StableTypeNode::ErrorUnion { error, value } => {
                    append.intern(nia_ty::TyKind::ErrorUnion {
                        error: types[usize::try_from(*error).unwrap()],
                        value: types[usize::try_from(*value).unwrap()],
                    })
                }
                StableTypeNode::Callable {
                    parameters,
                    result,
                    readonly,
                } => append.intern(nia_ty::TyKind::Callable {
                    is_readonly: *readonly,
                    params: parameters
                        .iter()
                        .map(|index| types[usize::try_from(*index).unwrap()])
                        .collect(),
                    return_type: types[usize::try_from(*result).unwrap()],
                }),
                StableTypeNode::CallablePointee { parameters, result } => {
                    append.intern(nia_ty::TyKind::CallablePointee {
                        params: parameters
                            .iter()
                            .map(|index| types[usize::try_from(*index).unwrap()])
                            .collect(),
                        return_type: types[usize::try_from(*result).unwrap()],
                    })
                }
                StableTypeNode::SelfParam => append.intern(nia_ty::TyKind::SelfParam),
                StableTypeNode::BuiltinType(tag) => append.intern(nia_ty::TyKind::BuiltinType(
                    stable_builtin_type(*tag).ok_or_else(|| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            "unknown stable builtin type tag".to_string(),
                        )
                    })?,
                )),
                StableTypeNode::BuiltinTrait {
                    trait_id,
                    arguments,
                } => append.intern(nia_ty::TyKind::BuiltinTrait {
                    trait_id: stable_builtin_trait(*trait_id).ok_or_else(|| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            "unknown stable builtin trait tag".to_string(),
                        )
                    })?,
                    args: arguments
                        .iter()
                        .map(|index| types[usize::try_from(*index).unwrap()])
                        .collect(),
                }),
                StableTypeNode::TraitObject {
                    readonly,
                    trait_id,
                    trait_arguments,
                    trait_const_arguments,
                    associated_type_bindings,
                } => append.intern(nia_ty::TyKind::TraitObject {
                    is_readonly: *readonly,
                    trait_id: self.rehydrate_stable_trait_id(trait_id, resolver)?,
                    trait_args: trait_arguments
                        .iter()
                        .map(|index| types[usize::try_from(*index).unwrap()])
                        .collect(),
                    trait_const_args: self
                        .rehydrate_stable_const_args(trait_const_arguments, &types)?,
                    associated_type_bindings: self.rehydrate_stable_bindings(
                        associated_type_bindings,
                        &types,
                        resolver,
                    )?,
                }),
                StableTypeNode::TraitObjectPointee {
                    trait_id,
                    trait_arguments,
                    trait_const_arguments,
                    associated_type_bindings,
                } => append.intern(nia_ty::TyKind::TraitObjectPointee {
                    trait_id: self.rehydrate_stable_trait_id(trait_id, resolver)?,
                    trait_args: trait_arguments
                        .iter()
                        .map(|index| types[usize::try_from(*index).unwrap()])
                        .collect(),
                    trait_const_args: self
                        .rehydrate_stable_const_args(trait_const_arguments, &types)?,
                    associated_type_bindings: self.rehydrate_stable_bindings(
                        associated_type_bindings,
                        &types,
                        resolver,
                    )?,
                }),
                StableTypeNode::Projection {
                    self_ty,
                    trait_id,
                    trait_arguments,
                    trait_const_arguments,
                    name,
                } => append.intern(nia_ty::TyKind::Projection {
                    self_ty: types[usize::try_from(*self_ty).unwrap()],
                    trait_id: self.rehydrate_stable_trait_id(trait_id, resolver)?,
                    trait_args: trait_arguments
                        .iter()
                        .map(|index| types[usize::try_from(*index).unwrap()])
                        .collect(),
                    trait_const_args: self
                        .rehydrate_stable_const_args(trait_const_arguments, &types)?,
                    name: SymbolId::from_stable_hash(*name),
                }),
                StableTypeNode::ClosureState {
                    owner,
                    ordinal,
                    captures,
                    parameters,
                    result,
                } => append.intern(nia_ty::TyKind::ClosureState {
                    closure_id: nia_ids::ClosureId {
                        owner: resolver.definition_for_identity(owner)?,
                        ordinal: *ordinal,
                    },
                    captures: captures
                        .iter()
                        .map(|index| types[usize::try_from(*index).unwrap()])
                        .collect(),
                    params: parameters
                        .iter()
                        .map(|index| types[usize::try_from(*index).unwrap()])
                        .collect(),
                    return_type: types[usize::try_from(*result).unwrap()],
                }),
            };
            types.push(ty);
        }
        Ok(types)
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
        resolve_loaded_definition_in_query(&self.db, definition, package)
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
                    format!("nominal type belongs to an external package; provide an external definition resolver: {def_id:?}"),
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
    /// to the current package.
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
        // stable ordering. Sort roots by their structural, relocation-
        // independent key before assigning graph indices.
        let mut keyed_roots = roots
            .iter()
            .map(|root| Ok((*root, encoder.canonical_key(*root)?)))
            .collect::<QueryResult<Vec<_>>>()?;
        keyed_roots.sort_by(|left, right| left.1.cmp(&right.1));
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

    /// Resolves a definition to its canonical package, using the supplied
    /// identity as the owner of the current source root.
    pub fn package_for_definition_in_package(
        &self,
        def_id: GlobalDefId,
        current_package: &PackageId,
    ) -> QueryResult<PackageId> {
        let graph = self.db.get(ModuleGraphQuery)?;
        if graph.current_package_root(def_id.module_id) == graph.current_package_root(graph.entry())
        {
            return Ok(current_package.clone());
        }
        if graph.current_package_root(def_id.module_id) == graph.std_package_root() {
            return Ok(PackageId::standard_library());
        }
        if graph.current_package_root(def_id.module_id)
            == graph.package_root(&nia_symbol::known::RUNTIME)
            && let RuntimeSpec::Source(runtime) = self.db.get(CompilerRuntimeQuery)?.as_ref()
        {
            return Ok(runtime.package().clone());
        }
        // Any remaining source root is owned by the current package.
        Ok(current_package.clone())
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
        let (optimization_changed, codegen_scope_changed, current_package_changed) = {
            let mut inputs = self.inputs.write().expect("compiler input lock poisoned");
            let optimization_changed = inputs.optimization != new_inputs.optimization;
            let codegen_scope_changed = inputs.codegen_scope != new_inputs.codegen_scope;
            let current_package_changed = inputs.current_package != new_inputs.current_package;
            *inputs = new_inputs;
            (
                optimization_changed,
                codegen_scope_changed,
                current_package_changed,
            )
        };
        let mut invalidation = CompilerInvalidation::default();
        if graph_changed {
            // The executable fact epoch contains session-local module handles;
            // a graph replacement makes that value and every dependent red.
            invalidation.extend(self.db.invalidate(ExecutableFactEpochQuery)?);
            if handle_generation_changed {
                self.db
                    .session()
                    .invalidate_scope(|frame| frame.name == "loaded_modules")?;
            }
            let loaded_modules = StableModuleSequence::from_source_identities(
                self.db
                    .context()
                    .loader_facts()
                    .loaded_module_source_identities()?,
            );
            invalidation.extend(
                self.db
                    .validate_input(LoadedModulesQuery, &loaded_modules)?,
            );
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
        let inputs_invalidation = self.invalidate_inputs(
            optimization_changed,
            codegen_scope_changed,
            current_package_changed,
        )?;
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
        Ok(invalidation)
    }

    /// Returns a snapshot of query execution and reuse counters.
    pub fn query_trace(&self) -> QueryResult<QueryTrace> {
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

    fn invalidate_inputs(
        &self,
        optimization_changed: bool,
        codegen_scope_changed: bool,
        current_package_changed: bool,
    ) -> QueryResult<CompilerInvalidation> {
        let mut invalidation = CompilerInvalidation::default();
        let provider_worklist = self.db.context().provider_fact_worklist()?;
        invalidation.extend(
            self.db
                .validate_input(ProviderFactWorklistQuery, &provider_worklist)?,
        );
        if optimization_changed {
            invalidation.extend(self.db.invalidate(CompilerOptimizationQuery)?);
        }
        if codegen_scope_changed {
            invalidation.extend(self.db.invalidate(CompilerCodegenScopeQuery)?);
        }
        if current_package_changed {
            invalidation.extend(self.db.invalidate(MonomorphizationQuery)?);
            invalidation.extend(self.db.invalidate(BackendLoweringInputsQuery)?);
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
            nia_ty::TyKind::Error => StableTypeNode::Error,
            nia_ty::TyKind::ConstOnly => StableTypeNode::ConstOnly,
            nia_ty::TyKind::Opaque => StableTypeNode::Opaque,
            nia_ty::TyKind::VolatilePointer { is_readonly, elem } => {
                StableTypeNode::VolatilePointer {
                    target: self.encode(elem)?,
                    readonly: is_readonly,
                }
            }
            nia_ty::TyKind::Slice { is_readonly, elem } => StableTypeNode::Slice {
                target: self.encode(elem)?,
                readonly: is_readonly,
            },
            nia_ty::TyKind::SlicePointee { elem } => StableTypeNode::SlicePointee {
                target: self.encode(elem)?,
            },
            nia_ty::TyKind::Vector { elem, lanes } => StableTypeNode::Vector {
                element: primitive_tag(elem)
                    .ok_or_else(|| self.unsupported("invalid vector lane type"))?,
                lanes,
            },
            nia_ty::TyKind::Range { kind, bound } => StableTypeNode::Range {
                kind: range_kind_tag(kind),
                bound: bound.map(|bound| self.encode(bound)).transpose()?,
            },
            nia_ty::TyKind::Optional { elem } => StableTypeNode::Optional {
                element: self.encode(elem)?,
            },
            nia_ty::TyKind::ErrorUnion { error, value } => StableTypeNode::ErrorUnion {
                error: self.encode(error)?,
                value: self.encode(value)?,
            },
            nia_ty::TyKind::Callable {
                is_readonly,
                params,
                return_type,
            } => StableTypeNode::Callable {
                parameters: params
                    .into_iter()
                    .map(|param| self.encode(param))
                    .collect::<QueryResult<Vec<_>>>()?,
                result: self.encode(return_type)?,
                readonly: is_readonly,
            },
            nia_ty::TyKind::CallablePointee {
                params,
                return_type,
            } => StableTypeNode::CallablePointee {
                parameters: params
                    .into_iter()
                    .map(|param| self.encode(param))
                    .collect::<QueryResult<Vec<_>>>()?,
                result: self.encode(return_type)?,
            },
            nia_ty::TyKind::SelfParam => StableTypeNode::SelfParam,
            nia_ty::TyKind::BuiltinType(builtin) => {
                StableTypeNode::BuiltinType(builtin_type_tag(builtin))
            }
            nia_ty::TyKind::BuiltinTrait { trait_id, args } => StableTypeNode::BuiltinTrait {
                trait_id: builtin_trait_tag(trait_id),
                arguments: args
                    .into_iter()
                    .map(|arg| self.encode(arg))
                    .collect::<QueryResult<Vec<_>>>()?,
            },
            nia_ty::TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            } => StableTypeNode::TraitObject {
                readonly: is_readonly,
                trait_id: self.stable_trait_id(trait_id)?,
                trait_arguments: trait_args
                    .into_iter()
                    .map(|arg| self.encode(arg))
                    .collect::<QueryResult<Vec<_>>>()?,
                trait_const_arguments: trait_const_args
                    .iter()
                    .map(|arg| self.encode_const_argument(arg))
                    .collect::<QueryResult<Vec<_>>>()?,
                associated_type_bindings: associated_type_bindings
                    .iter()
                    .map(|binding| self.stable_binding(binding))
                    .collect::<QueryResult<Vec<_>>>()?,
            },
            nia_ty::TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            } => StableTypeNode::TraitObjectPointee {
                trait_id: self.stable_trait_id(trait_id)?,
                trait_arguments: trait_args
                    .into_iter()
                    .map(|arg| self.encode(arg))
                    .collect::<QueryResult<Vec<_>>>()?,
                trait_const_arguments: trait_const_args
                    .iter()
                    .map(|arg| self.encode_const_argument(arg))
                    .collect::<QueryResult<Vec<_>>>()?,
                associated_type_bindings: associated_type_bindings
                    .iter()
                    .map(|binding| self.stable_binding(binding))
                    .collect::<QueryResult<Vec<_>>>()?,
            },
            nia_ty::TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            } => StableTypeNode::Projection {
                self_ty: self.encode(self_ty)?,
                trait_id: self.stable_trait_id(trait_id)?,
                trait_arguments: trait_args
                    .into_iter()
                    .map(|arg| self.encode(arg))
                    .collect::<QueryResult<Vec<_>>>()?,
                trait_const_arguments: trait_const_args
                    .iter()
                    .map(|arg| self.encode_const_argument(arg))
                    .collect::<QueryResult<Vec<_>>>()?,
                name: name.raw(),
            },
            nia_ty::TyKind::Array { len, elem } => StableTypeNode::Array {
                element: self.encode(elem)?,
                length: self.encode_array_length(&len)?,
            },
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
                if let Some(trait_id) = self.builtin_trait_for_definition(def_id)? {
                    if !const_args.is_empty() {
                        return Err(self.unsupported(
                            "builtin trait applications with const arguments cannot cross package metadata",
                        ));
                    }
                    StableTypeNode::BuiltinTrait {
                        trait_id: builtin_trait_tag(trait_id),
                        arguments: args
                            .into_iter()
                            .map(|argument| self.encode(argument))
                            .collect::<QueryResult<Vec<_>>>()?,
                    }
                } else {
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
            }
            nia_ty::TyKind::GenericParam(name) => StableTypeNode::GenericParam(name.raw()),
            nia_ty::TyKind::ClosureState {
                closure_id,
                captures,
                params,
                return_type,
            } => StableTypeNode::ClosureState {
                owner: self.definition(closure_id.owner)?,
                ordinal: closure_id.ordinal,
                captures: captures
                    .into_iter()
                    .map(|capture| self.encode(capture))
                    .collect::<QueryResult<Vec<_>>>()?,
                parameters: params
                    .into_iter()
                    .map(|param| self.encode(param))
                    .collect::<QueryResult<Vec<_>>>()?,
                result: self.encode(return_type)?,
            },
            unsupported => {
                return Err(self.unsupported(&format!(
                    "type form has no stable package encoding: {unsupported:?}"
                )));
            }
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
            nia_ty::TyKind::Error => key.push(11),
            nia_ty::TyKind::ConstOnly => key.push(12),
            nia_ty::TyKind::Opaque => key.push(13),
            nia_ty::TyKind::VolatilePointer { is_readonly, elem } => {
                key.extend_from_slice(&[14, u8::from(is_readonly)]);
                append_bytes(&mut key, &self.canonical_key(elem)?);
            }
            nia_ty::TyKind::Slice { is_readonly, elem } => {
                key.extend_from_slice(&[15, u8::from(is_readonly)]);
                append_bytes(&mut key, &self.canonical_key(elem)?);
            }
            nia_ty::TyKind::SlicePointee { elem } => {
                key.push(16);
                append_bytes(&mut key, &self.canonical_key(elem)?);
            }
            nia_ty::TyKind::Vector { elem, lanes } => {
                key.push(17);
                key.push(
                    primitive_tag(elem)
                        .ok_or_else(|| self.unsupported("invalid vector lane type"))?,
                );
                key.extend_from_slice(&lanes.to_le_bytes());
            }
            nia_ty::TyKind::Range { kind, bound } => {
                key.push(18);
                key.push(range_kind_tag(kind));
                if let Some(bound) = bound {
                    key.push(1);
                    append_bytes(&mut key, &self.canonical_key(bound)?);
                } else {
                    key.push(0);
                }
            }
            nia_ty::TyKind::Optional { elem } => {
                key.push(19);
                append_bytes(&mut key, &self.canonical_key(elem)?);
            }
            nia_ty::TyKind::ErrorUnion { error, value } => {
                key.push(20);
                append_bytes(&mut key, &self.canonical_key(error)?);
                append_bytes(&mut key, &self.canonical_key(value)?);
            }
            nia_ty::TyKind::Callable {
                is_readonly,
                params,
                return_type,
            } => {
                key.extend_from_slice(&[21, u8::from(is_readonly)]);
                append_len(&mut key, params.len());
                for param in params {
                    append_bytes(&mut key, &self.canonical_key(param)?);
                }
                append_bytes(&mut key, &self.canonical_key(return_type)?);
            }
            nia_ty::TyKind::CallablePointee {
                params,
                return_type,
            } => {
                key.push(22);
                append_len(&mut key, params.len());
                for param in params {
                    append_bytes(&mut key, &self.canonical_key(param)?);
                }
                append_bytes(&mut key, &self.canonical_key(return_type)?);
            }
            nia_ty::TyKind::SelfParam => key.push(23),
            nia_ty::TyKind::BuiltinType(builtin) => {
                key.push(24);
                key.push(builtin_type_tag(builtin));
            }
            nia_ty::TyKind::BuiltinTrait { trait_id, args } => {
                key.push(25);
                key.push(builtin_trait_tag(trait_id));
                append_len(&mut key, args.len());
                for arg in args {
                    append_bytes(&mut key, &self.canonical_key(arg)?);
                }
            }
            nia_ty::TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            } => {
                key.extend_from_slice(&[26, u8::from(is_readonly)]);
                self.append_trait_key(&mut key, trait_id)?;
                append_len(&mut key, trait_args.len());
                for arg in trait_args {
                    append_bytes(&mut key, &self.canonical_key(arg)?);
                }
                append_len(&mut key, trait_const_args.len());
                for arg in trait_const_args {
                    self.append_stable_const_key(&mut key, &arg)?;
                }
                self.append_binding_keys(&mut key, &associated_type_bindings)?;
            }
            nia_ty::TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            } => {
                key.push(27);
                self.append_trait_key(&mut key, trait_id)?;
                append_len(&mut key, trait_args.len());
                for arg in trait_args {
                    append_bytes(&mut key, &self.canonical_key(arg)?);
                }
                append_len(&mut key, trait_const_args.len());
                for arg in trait_const_args {
                    self.append_stable_const_key(&mut key, &arg)?;
                }
                self.append_binding_keys(&mut key, &associated_type_bindings)?;
            }
            nia_ty::TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            } => {
                key.push(28);
                append_bytes(&mut key, &self.canonical_key(self_ty)?);
                self.append_trait_key(&mut key, trait_id)?;
                append_len(&mut key, trait_args.len());
                for arg in trait_args {
                    append_bytes(&mut key, &self.canonical_key(arg)?);
                }
                append_len(&mut key, trait_const_args.len());
                for arg in trait_const_args {
                    self.append_stable_const_key(&mut key, &arg)?;
                }
                key.extend_from_slice(&name.raw().to_le_bytes());
            }
            nia_ty::TyKind::Array { len, elem } => {
                key.push(5);
                match self.encode_array_length(&len)? {
                    StableArrayLength::ConstValue(length) => {
                        key.push(0);
                        key.extend_from_slice(&length.to_le_bytes());
                    }
                    StableArrayLength::GenericParam(hash) => {
                        key.push(1);
                        key.extend_from_slice(&hash.to_le_bytes());
                    }
                }
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
                if let Some(trait_id) = self.builtin_trait_for_definition(def_id)? {
                    if !const_args.is_empty() {
                        return Err(self.unsupported(
                            "builtin trait applications with const arguments cannot cross package metadata",
                        ));
                    }
                    key.push(25);
                    key.push(builtin_trait_tag(trait_id));
                    append_len(&mut key, args.len());
                    for argument in args {
                        append_bytes(&mut key, &self.canonical_key(argument)?);
                    }
                } else {
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
                        self.append_stable_const_key(&mut key, argument)?;
                    }
                }
            }
            nia_ty::TyKind::GenericParam(name) => {
                key.push(9);
                key.extend_from_slice(&name.raw().to_le_bytes());
            }
            nia_ty::TyKind::ClosureState {
                closure_id,
                captures,
                params,
                return_type,
            } => {
                key.push(29);
                append_definition_key(&mut key, &self.definition(closure_id.owner)?);
                key.extend_from_slice(&closure_id.ordinal.to_le_bytes());
                append_len(&mut key, captures.len());
                for capture in captures {
                    append_bytes(&mut key, &self.canonical_key(capture)?);
                }
                append_len(&mut key, params.len());
                for param in params {
                    append_bytes(&mut key, &self.canonical_key(param)?);
                }
                append_bytes(&mut key, &self.canonical_key(return_type)?);
            }
            unsupported => {
                return Err(self.unsupported(&format!(
                    "type form has no stable package encoding: {unsupported:?}"
                )));
            }
        }
        self.key_visiting.remove(&ty);
        self.key_cache.insert(ty, key.clone());
        Ok(key)
    }

    fn append_trait_key(&self, key: &mut Vec<u8>, trait_id: nia_ty::TraitId) -> QueryResult<()> {
        match trait_id {
            nia_ty::TraitId::Source(def_id) => {
                key.push(0);
                append_definition_key(key, &self.definition(def_id)?);
            }
            nia_ty::TraitId::Builtin(builtin) => {
                key.push(1);
                key.push(builtin_trait_tag(builtin));
            }
        }
        Ok(())
    }

    fn append_stable_const_key(
        &mut self,
        key: &mut Vec<u8>,
        argument: &nia_ty::ConstGenericArg,
    ) -> QueryResult<()> {
        append_bytes(key, &self.canonical_key(argument.ty)?);
        match &argument.value {
            nia_ty::ConstGenericValue::GenericParam(name) => {
                key.push(0);
                key.extend_from_slice(&name.raw().to_le_bytes());
            }
            nia_ty::ConstGenericValue::Int(value) => {
                key.push(1);
                key.extend_from_slice(&value.bits().to_le_bytes());
                key.push(u8::from(value.is_signed()));
            }
            nia_ty::ConstGenericValue::Bool(value) => {
                key.push(2);
                key.push(u8::from(*value));
            }
            nia_ty::ConstGenericValue::Char(value) => {
                key.push(3);
                key.extend_from_slice(&u32::from(*value).to_le_bytes());
            }
            nia_ty::ConstGenericValue::ConstExpr(_) => {
                return Err(self.unsupported(
                    "const expression argument has no stable package representation",
                ));
            }
        }
        Ok(())
    }

    fn append_binding_keys(
        &mut self,
        key: &mut Vec<u8>,
        bindings: &[nia_ty::AssociatedTypeBindingTy],
    ) -> QueryResult<()> {
        append_len(key, bindings.len());
        for binding in bindings {
            match binding.trait_id {
                Some(trait_id) => {
                    key.push(1);
                    self.append_trait_key(key, trait_id)?;
                }
                None => key.push(0),
            }
            append_len(key, binding.trait_args.len());
            for arg in &binding.trait_args {
                append_bytes(key, &self.canonical_key(*arg)?);
            }
            append_len(key, binding.trait_const_args.len());
            for arg in &binding.trait_const_args {
                self.append_stable_const_key(key, arg)?;
            }
            key.extend_from_slice(&binding.name.raw().to_le_bytes());
            append_bytes(key, &self.canonical_key(binding.ty)?);
        }
        Ok(())
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
        let mut owner_chain = Vec::new();
        let mut parent = def.parent;
        while let Some(parent_id) = parent {
            let parent_def = defs.semantic.defs.get(parent_id).ok_or_else(|| {
                self.db.invalid_input(
                    &ModuleGraphQuery,
                    "definition parent is missing from module facts".to_string(),
                )
            })?;
            let parent_name = self.symbols.resolve(parent_def.name).ok_or_else(|| {
                self.db.invalid_input(
                    &ModuleGraphQuery,
                    "definition parent has no resolvable symbol".to_string(),
                )
            })?;
            owner_chain.push((
                parent_name.to_string(),
                def_kind_tag(parent_def.kind),
                parent_id.0,
            ));
            parent = parent_def.parent;
        }
        let mut owner_identity = None;
        for (owner_name, owner_kind, owner_disambiguator) in owner_chain.into_iter().rev() {
            owner_identity = Some(Box::new(DefinitionId {
                module: StableModuleId {
                    package: package.clone(),
                    path: module.source_identity().normalized_path().to_owned(),
                },
                name: owner_name,
                kind: owner_kind,
                disambiguator: owner_disambiguator,
                owner: owner_identity,
            }));
        }
        Ok(DefinitionId {
            module: StableModuleId {
                package,
                path: module.source_identity().normalized_path().to_owned(),
            },
            name: name.to_string(),
            kind: def_kind_tag(def.kind),
            disambiguator: def_id.def_id.0,
            owner: owner_identity,
        })
    }

    fn encode_const_argument(
        &mut self,
        argument: &nia_ty::ConstGenericArg,
    ) -> QueryResult<StableConstArg> {
        let value = match &argument.value {
            nia_ty::ConstGenericValue::GenericParam(name) => {
                StableConstValue::GenericParam(name.raw())
            }
            nia_ty::ConstGenericValue::Int(value) => StableConstValue::Integer {
                bits: value.bits(),
                signed: value.is_signed(),
            },
            nia_ty::ConstGenericValue::Bool(value) => StableConstValue::Bool(*value),
            nia_ty::ConstGenericValue::Char(value) => StableConstValue::Char(*value),
            nia_ty::ConstGenericValue::ConstExpr(_) => {
                return Err(self.unsupported(
                    "const expression argument has no stable package representation",
                ));
            }
        };
        Ok(StableConstArg {
            ty: self.encode(argument.ty)?,
            value,
        })
    }

    fn encode_array_length(&self, length: &nia_ty::ArrayLenTy) -> QueryResult<StableArrayLength> {
        match length {
            nia_ty::ArrayLenTy::ConstValue(value) => Ok(StableArrayLength::ConstValue(*value)),
            nia_ty::ArrayLenTy::GenericParam(name) => {
                Ok(StableArrayLength::GenericParam(name.raw()))
            }
            nia_ty::ArrayLenTy::ConstExpr(id) => {
                let values = self.db.get(ConstArrayLengthsQuery(id.module_id))?;
                values
                    .values
                    .get(id)
                    .copied()
                    .map(StableArrayLength::ConstValue)
                    .ok_or_else(|| {
                        self.unsupported(
                            "array length const expression is not evaluated in the stable type graph",
                        )
                    })
            }
            nia_ty::ArrayLenTy::Builtin { .. } | nia_ty::ArrayLenTy::Infer => Err(self
                .unsupported("array length depends on contextual or target-dependent inference")),
        }
    }

    fn stable_trait_id(&self, trait_id: nia_ty::TraitId) -> QueryResult<StableTraitId> {
        Ok(match trait_id {
            nia_ty::TraitId::Source(def_id) => {
                if let Some(builtin) = self.builtin_trait_for_definition(def_id)? {
                    StableTraitId::Builtin(builtin_trait_tag(builtin))
                } else {
                    StableTraitId::Source(self.definition(def_id)?)
                }
            }
            nia_ty::TraitId::Builtin(builtin) => StableTraitId::Builtin(builtin_trait_tag(builtin)),
        })
    }

    fn builtin_trait_for_definition(
        &self,
        def_id: GlobalDefId,
    ) -> QueryResult<Option<nia_ids::BuiltinTrait>> {
        Ok(self
            .db
            .get(ItemSignaturesQuery(def_id.module_id))?
            .semantic
            .traits
            .get(&def_id.def_id)
            .and_then(|signature| signature.builtin))
    }

    fn stable_binding(
        &mut self,
        binding: &nia_ty::AssociatedTypeBindingTy,
    ) -> QueryResult<StableAssociatedTypeBinding> {
        Ok(StableAssociatedTypeBinding {
            trait_id: binding
                .trait_id
                .map(|trait_id| self.stable_trait_id(trait_id))
                .transpose()?,
            trait_arguments: binding
                .trait_args
                .iter()
                .map(|arg| self.encode(*arg))
                .collect::<QueryResult<Vec<_>>>()?,
            trait_const_arguments: binding
                .trait_const_args
                .iter()
                .map(|arg| self.encode_const_argument(arg))
                .collect::<QueryResult<Vec<_>>>()?,
            name: binding.name.raw(),
            ty: self.encode(binding.ty)?,
        })
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

fn stable_range_kind(tag: u8) -> Option<nia_ty::RangeTyKind> {
    Some(match tag {
        0 => nia_ty::RangeTyKind::Exclusive,
        1 => nia_ty::RangeTyKind::Inclusive,
        2 => nia_ty::RangeTyKind::From,
        3 => nia_ty::RangeTyKind::To,
        4 => nia_ty::RangeTyKind::ToInclusive,
        5 => nia_ty::RangeTyKind::Full,
        _ => return None,
    })
}

fn primitive_tag(primitive: nia_ty::PrimitiveTy) -> Option<u8> {
    match primitive_type_node(primitive).ok()? {
        StableTypeNode::Primitive(tag) => Some(tag),
        StableTypeNode::Never => None,
        _ => None,
    }
}

fn range_kind_tag(kind: nia_ty::RangeTyKind) -> u8 {
    match kind {
        nia_ty::RangeTyKind::Exclusive => 0,
        nia_ty::RangeTyKind::Inclusive => 1,
        nia_ty::RangeTyKind::From => 2,
        nia_ty::RangeTyKind::To => 3,
        nia_ty::RangeTyKind::ToInclusive => 4,
        nia_ty::RangeTyKind::Full => 5,
    }
}

fn builtin_type_tag(value: nia_ids::BuiltinType) -> u8 {
    match value {
        nia_ids::BuiltinType::AsmConfig => 0,
        nia_ids::BuiltinType::AsmInputs => 1,
        nia_ids::BuiltinType::AsmOutputs => 2,
    }
}

fn builtin_trait_tag(value: nia_ids::BuiltinTrait) -> u8 {
    u8::try_from(value.stable_tag()).expect("builtin trait stable tag fits in u8")
}

fn stable_builtin_type(tag: u8) -> Option<nia_ids::BuiltinType> {
    Some(match tag {
        0 => nia_ids::BuiltinType::AsmConfig,
        1 => nia_ids::BuiltinType::AsmInputs,
        2 => nia_ids::BuiltinType::AsmOutputs,
        _ => return None,
    })
}

fn stable_builtin_trait(tag: u8) -> Option<nia_ids::BuiltinTrait> {
    nia_ids::BuiltinTrait::from_stable_tag(u32::from(tag))
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
    match &definition.owner {
        Some(owner) => {
            bytes.push(1);
            append_definition_key(bytes, owner);
        }
        None => bytes.push(0),
    }
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
    let mut demands = program
        .modules
        .iter()
        .flat_map(|module| module.provider_demands.iter().cloned())
        .collect::<Vec<_>>();
    demands.extend(program.modules.iter().filter_map(|module| {
        let needs_body_activation = program
            .graph
            .get(module.id)
            .is_some_and(|node| !node.process_used_paths);
        needs_body_activation.then(|| crate::ProviderDemand {
            source_path: module.path.clone(),
            request: crate::ProviderRequest::ModuleBody {
                module_path: module.path.clone(),
            },
        })
    }));
    demands
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
) -> QueryResult<CompilerDatabase> {
    let session = match request.loader_facts.query_session() {
        Some(session) => session,
        None => nia_query::QuerySession::new()?,
    };
    compiler_database_with_providers_in_session(request, providers, session)
}

fn compiler_database_with_providers_in_session(
    request: CompileRequest,
    providers: CompilerQueryProviders,
    session: nia_query::QuerySession,
) -> QueryResult<CompilerDatabase> {
    let timings = request.timings;
    let signature_cache = request.frontend_cache_dir.as_ref().map(|root| {
        Arc::new(crate::signature_cache::PersistentSignatureCache::new(
            root.clone(),
        ))
    });
    let verify_frontend_cache = request.verify_frontend_cache;
    let loader_facts = Arc::clone(&request.loader_facts);
    let observed_graph = loader_facts.module_graph()?;
    if let Some(loader_session) = loader_facts.query_session()
        && !session.ptr_eq(&loader_session)
    {
        return Err(QueryError::internal(
            "compiler and loader facts must share one query session",
        ));
    }
    let node_store = loader_facts.node_store();
    let inputs = Arc::new(RwLock::new(CompilerInputs::new(request)));
    let executable_fact_session = Arc::new(std::sync::Mutex::new(ExecutableFactSession::default()));
    let type_store = Arc::new(nia_ty::TypeStore::new()?);
    let db = QueryDb::new_registered_with_timings_in_session(
        CompilerContext {
            inputs: inputs.clone(),
            observed_graph: std::sync::Mutex::new(observed_graph),
            loader_facts,
            providers,
            executable_fact_session,
            executable_fact_scheduler: std::sync::Mutex::new(()),
            type_store,
            diagnostic_store: nia_diagnostic::DiagnosticStore::new()?,
            node_store,
            signature_cache,
            verify_frontend_cache,
            provider_demand_rounds: std::sync::atomic::AtomicU64::new(0),
        },
        timings,
        compiler_query_registry()?,
        session,
    )?;
    Ok(CompilerDatabase { db, inputs })
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
        QueryError::Internal(ice) => Diagnostic::from(ice),
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
            trait_type_argument_names,
        } => {
            builder.write_u8(1);
            if let Some(target_type_name) = target_type_name {
                builder.write_u8(1);
                builder.write_u64(target_type_name.raw());
            } else {
                builder.write_u8(0);
            }
            builder.write_u64(trait_name.raw());
            builder.write_u64(trait_type_argument_names.len() as u64);
            for argument in trait_type_argument_names {
                if let Some(argument) = argument {
                    builder.write_u8(1);
                    builder.write_u64(argument.raw());
                } else {
                    builder.write_u8(0);
                }
            }
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
            let key = graph
                .stable_key(module_id)
                .unwrap_or_else(|| panic!("Nia ICE: module {module_id:?} has no stable key"));
            identities.push(key.source_identity().clone());
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
            if let Some(module_id) = graph.module_id_for_stable_key(key) {
                module_ids.push(module_id);
                continue;
            }
            // A graph update can preserve the source identity while allocating a
            // fresh stable-key package/relationship. Keep the compatibility path
            // for that transition; the common unchanged-graph case uses the
            // indexed lookup above and avoids repeated loader path clones.
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

    fn codegen_scope(&self) -> crate::CodegenScope {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .codegen_scope
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
    use crate::RuntimeSpec;
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
