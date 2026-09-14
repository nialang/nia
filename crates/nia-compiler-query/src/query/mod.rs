// SPDX-License-Identifier: GPL-3.0-or-later
use crate::ctfe_template_codec::{
    collect_resolved_const_function_relocations, decode_resolved_const_function,
    encode_resolved_const_function,
};
use crate::template_body_codec::{
    TemplateBodyDecodeContext, TemplateBodyEncodeContext,
    collect_checked_closure_entry_relocations, collect_checked_function_body_relocations,
    decode_checked_closure_entries, decode_checked_function_body, encode_checked_closure_entries,
    encode_checked_function_body,
};
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
    DefCollection, ModulePublicSurface, ModuleUsingScope, PublicSource, PublicSurfaceLookup,
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
    PackageId, PackageManifest, PublicSurfaceExport, PublicSurfaceModule, PublicSurfaceSection,
    SectionKind, StableArrayLength, StableAssociatedTypeBinding, StableConstArg, StableConstValue,
    StableDeclaration, StableTraitId, StableTypeGraph, StableTypeNode,
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
use nia_symbol::{SymbolId, SymbolText};
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

    /// Iterates every stable definition remapped in this session. The
    /// publication boundary uses this inverse view to encode checked bodies
    /// without reconstructing identities from source names.
    pub fn iter(&self) -> impl Iterator<Item = (&DefinitionId, &GlobalDefId)> {
        self.definitions.iter()
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

/// Resolves one loaded artifact/source definition through the query graph.
///
/// This is the query-side counterpart of [`CompilerDatabase::resolve_loaded_definition`].
/// Keeping the remap algorithm here lets provider queries consume stable
/// artifact identities without manufacturing a second, name-only lookup path.
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
    let mut module_id = None;
    for module in graph.modules() {
        if let Some(identity) = db
            .context()
            .loader_facts()
            .compiled_package_module_identity(module.id)?
        {
            if identity.package == *package && identity.path == definition.module.path {
                module_id = Some(module.id);
                break;
            }
        }
    }
    let module_id = module_id.or_else(|| graph.module_id_for_path(&definition.module.path));
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

/// Rehydrated package type graph retaining the canonical wire-node indexes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledPackageTypeGraph {
    package: PackageId,
    types: Vec<InternedTyId>,
}

impl CompiledPackageTypeGraph {
    pub fn package(&self) -> &PackageId {
        &self.package
    }

    pub fn get(&self, index: u32) -> Option<InternedTyId> {
        self.types.get(index as usize).copied()
    }

    pub fn len(&self) -> usize {
        self.types.len()
    }
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
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledPackageTemplates {
    package: PackageId,
    records: BTreeMap<DefinitionId, CompiledTemplate>,
}

/// One validated, session-remapped checked template body.
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledTemplate {
    pub definition: GlobalDefId,
    pub body: Option<nia_function_ir::FunctionBody>,
    pub closure_entries: Vec<nia_function_ir::FunctionClosureEntry>,
    pub ctfe_body: Option<nia_const_ir::ResolvedConstFunction>,
    pub summary: nia_package_metadata::TemplateSummary,
}

struct TemplateDecodeContext {
    types: Vec<InternedTyId>,
    definitions: Vec<GlobalDefId>,
    modules: Vec<ModuleId>,
}

struct TemplateEncodeContext {
    types: HashMap<InternedTyId, u32>,
    definitions: HashMap<GlobalDefId, u32>,
    modules: HashMap<ModuleId, u32>,
}

struct PublishedTemplateBody {
    definition: DefinitionId,
    global: GlobalDefId,
    body: nia_function_ir::FunctionBody,
    closure_entries: Vec<nia_function_ir::FunctionClosureEntry>,
    ctfe_body: Option<nia_const_ir::ResolvedConstFunction>,
    parameter_count: u32,
}

impl TemplateBodyEncodeContext for TemplateEncodeContext {
    fn type_index(&self, ty: InternedTyId) -> Option<u32> {
        self.types.get(&ty).copied()
    }

    fn definition_index(&self, definition: GlobalDefId) -> Option<u32> {
        self.definitions.get(&definition).copied()
    }

    fn module_index(&self, module: ModuleId) -> Option<u32> {
        self.modules.get(&module).copied()
    }
}

impl TemplateBodyDecodeContext for TemplateDecodeContext {
    fn type_at(&self, index: u32) -> Option<InternedTyId> {
        self.types.get(index as usize).copied()
    }

    fn definition_at(&self, index: u32) -> Option<GlobalDefId> {
        self.definitions.get(index as usize).copied()
    }

    fn module_at(&self, index: u32) -> Option<ModuleId> {
        self.modules.get(index as usize).copied()
    }
}

/// Target-independent signature facts selected from one compiled package.
/// Records retain canonical identities and stable type-graph roots; consumers
/// must remap those roots before constructing session-local handles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledPackageSignatures {
    package: PackageId,
    records: BTreeMap<DefinitionId, nia_package_metadata::SignatureRecord>,
    traits: BTreeMap<DefinitionId, nia_package_metadata::SignatureTraitRecord>,
    extensions: Vec<nia_package_metadata::SignatureExtensionRecord>,
}

impl CompiledPackageSignatures {
    pub fn package(&self) -> &PackageId {
        &self.package
    }

    pub fn get(&self, definition: &DefinitionId) -> Option<&nia_package_metadata::SignatureRecord> {
        self.records.get(definition)
    }

    /// Returns the complete typed declaration payload when the published
    /// package carries one. Consumers must remap its stable roots before
    /// constructing session-local signatures.
    pub fn payload(
        &self,
        definition: &DefinitionId,
    ) -> Option<&nia_package_metadata::SignaturePayload> {
        self.records.get(definition)?.payload.as_ref()
    }

    pub fn iter(
        &self,
    ) -> impl Iterator<Item = (&DefinitionId, &nia_package_metadata::SignatureRecord)> {
        self.records.iter()
    }

    pub fn trait_record(
        &self,
        definition: &DefinitionId,
    ) -> Option<&nia_package_metadata::SignatureTraitRecord> {
        self.traits.get(definition)
    }

    pub fn traits(
        &self,
    ) -> impl Iterator<Item = (&DefinitionId, &nia_package_metadata::SignatureTraitRecord)> {
        self.traits.iter()
    }

    pub fn extensions(&self) -> &[nia_package_metadata::SignatureExtensionRecord] {
        &self.extensions
    }

    /// Returns extension records implementing a given stable trait root.
    pub fn extensions_for_trait_root(
        &self,
        trait_root: u32,
    ) -> impl Iterator<Item = &nia_package_metadata::SignatureExtensionRecord> {
        self.extensions
            .iter()
            .filter(move |record| record.trait_root == Some(trait_root))
    }
}

/// Target-specific native products selected from one compiled package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledPackageNative {
    package: PackageId,
    variant: nia_package_metadata::NativeVariant,
}

impl CompiledPackageNative {
    pub fn package(&self) -> &PackageId {
        &self.package
    }

    pub fn variant(&self) -> &nia_package_metadata::NativeVariant {
        &self.variant
    }
}

impl CompiledPackageTemplates {
    pub fn package(&self) -> &PackageId {
        &self.package
    }

    pub fn get(&self, definition: &DefinitionId) -> Option<&CompiledTemplate> {
        self.records.get(definition)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&DefinitionId, &CompiledTemplate)> {
        self.records.iter()
    }

    /// Decodes the canonical semantic summary for one published template.
    /// Summary decoding is kept at the compiler boundary so downstream
    /// analyses never reinterpret opaque artifact bytes.
    pub fn summary(
        &self,
        definition: &DefinitionId,
    ) -> QueryResult<Option<nia_package_metadata::TemplateSummary>> {
        let Some(record) = self.records.get(definition) else {
            return Ok(None);
        };
        Ok(Some(record.summary.clone()))
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

    /// Looks up one package's artifact-backed signature section.
    pub fn signatures(
        &self,
        package: &PackageId,
    ) -> Option<&nia_package_metadata::SignatureSection> {
        self.packages.get(package)?.signatures()
    }

    /// Looks up a canonical trait declaration from a selected package.
    pub fn trait_signature(
        &self,
        definition: &DefinitionId,
    ) -> Option<&nia_package_metadata::SignatureTraitRecord> {
        let package = &definition.module.package;
        self.packages
            .get(package)?
            .signatures()?
            .traits
            .iter()
            .find(|record| record.definition == *definition)
    }

    /// Returns extension records published by a selected package.
    pub fn extension_signatures(
        &self,
        package: &PackageId,
    ) -> Option<&[nia_package_metadata::SignatureExtensionRecord]> {
        self.packages
            .get(package)?
            .signatures()
            .map(|section| section.extensions.as_slice())
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
    /// Definition-root scope used by executable and package code generation.
    pub codegen_scope: crate::CodegenScope,
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
                Ok(match argument {
                    StableConstArg { ty, value } => nia_ty::ConstGenericArg {
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
                            StableConstValue::Bool(value) => {
                                nia_ty::ConstGenericValue::Bool(*value)
                            }
                            StableConstValue::Char(value) => {
                                nia_ty::ConstGenericValue::Char(*value)
                            }
                        },
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
        let mut prepared = Vec::new();
        for (package, interface) in index.packages() {
            let mut records = BTreeMap::new();
            if let Some(templates) = interface.templates() {
                for record in &templates.records {
                    if record.definition.module.package != *package
                        || !interface
                            .records()
                            .iter()
                            .any(|item| item.definition == record.definition)
                    {
                        return Err(self.db.invalid_input(
                            &CompiledPackageInterfaceIndexQuery,
                            format!(
                                "compiled template identity is inconsistent: {:?}",
                                record.definition
                            ),
                        ));
                    }
                    let type_graph = self.compiled_package_type_graph(package).map_err(|_| {
                        self.db.invalid_input(
                            &CompiledPackageInterfaceIndexQuery,
                            "compiled template requires a rehydrated package type graph",
                        )
                    })?;
                    let mut context = TemplateDecodeContext {
                        types: Vec::with_capacity(record.type_roots.len()),
                        definitions: Vec::with_capacity(record.referenced_definitions.len()),
                        modules: Vec::with_capacity(record.referenced_modules.len()),
                    };
                    for root in &record.type_roots {
                        context.types.push(type_graph.get(*root).ok_or_else(|| {
                            self.db.invalid_input(
                                &CompiledPackageInterfaceIndexQuery,
                                "compiled template type root is outside its graph",
                            )
                        })?);
                    }
                    for definition in &record.referenced_definitions {
                        context.definitions.push(
                            self.resolve_template_definition(definition, package, interface)?,
                        );
                    }
                    for module in &record.referenced_modules {
                        context
                            .modules
                            .push(self.resolve_compiled_module_identity(module)?);
                    }
                    let owner =
                        self.resolve_template_definition(&record.definition, package, interface)?;
                    let body = (!record.body.is_empty())
                        .then(|| decode_checked_function_body(&record.body, &context))
                        .transpose()
                        .map_err(|error| {
                            self.db.invalid_input(
                                &CompiledPackageInterfaceIndexQuery,
                                format!("invalid checked template body: {error}"),
                            )
                        })?;
                    let closure_entries = (!record.closure_entries.is_empty())
                        .then(|| decode_checked_closure_entries(&record.closure_entries, &context))
                        .transpose()
                        .map_err(|error| {
                            self.db.invalid_input(
                                &CompiledPackageInterfaceIndexQuery,
                                format!("invalid checked closure entries: {error}"),
                            )
                        })?
                        .unwrap_or_default();
                    let ctfe_body = (!record.ctfe_body.is_empty())
                        .then(|| decode_resolved_const_function(&record.ctfe_body, &context))
                        .transpose()
                        .map_err(|error| {
                            self.db.invalid_input(
                                &CompiledPackageInterfaceIndexQuery,
                                format!("invalid CTFE template body: {error}"),
                            )
                        })?;
                    let is_const = interface.signatures().is_some_and(|signatures| {
                        signatures.records.iter().any(|signature| {
                            signature.definition == record.definition
                                && signature.flags & nia_package_metadata::SIGNATURE_FLAG_CONST != 0
                        })
                    });
                    if is_const && ctfe_body.is_none() {
                        return Err(self.db.invalid_input(
                            &CompiledPackageInterfaceIndexQuery,
                            format!(
                                "compiled const template has no CTFE body: {:?}",
                                record.definition
                            ),
                        ));
                    }
                    let summary = nia_package_metadata::decode_template_summary(&record.summary)
                        .map_err(|error| {
                            self.db.invalid_input(
                                &CompiledPackageInterfaceIndexQuery,
                                format!("invalid checked template summary: {error}"),
                            )
                        })?;
                    if records
                        .insert(
                            record.definition.clone(),
                            CompiledTemplate {
                                definition: owner,
                                body,
                                closure_entries,
                                ctfe_body,
                                summary,
                            },
                        )
                        .is_some()
                    {
                        return Err(self.db.invalid_input(
                            &CompiledPackageInterfaceIndexQuery,
                            "duplicate compiled template identity",
                        ));
                    }
                }
            }
            prepared.push((package.clone(), records));
        }
        for (package, records) in prepared {
            if self
                .db
                .can_publish_shared(CompiledPackageTemplatesQuery(package.clone()))
            {
                self.db.publish_shared(
                    CompiledPackageTemplatesQuery(package.clone()),
                    CompiledPackageTemplates {
                        package: package.clone(),
                        records,
                    },
                    &CompiledPackageInterfaceIndexQuery,
                );
            }
            installed.push(package.clone());
        }
        Ok(installed)
    }

    /// Consumes the checked template inventory for one package.
    pub fn compiled_package_templates(
        &self,
        package: PackageId,
    ) -> QueryResult<CompiledPackageTemplates> {
        Ok(self
            .db
            .get(CompiledPackageTemplatesQuery(package))?
            .as_ref()
            .clone())
    }

    /// Publishes validated target-independent signatures for selected packages.
    /// The product is predecessor-bound to the compiled interface index and is
    /// intentionally separate from source-backed `ItemSignaturesQuery`.
    pub fn install_compiled_package_signatures(&self) -> QueryResult<Vec<PackageId>> {
        let index = self.compiled_package_interface_index()?;
        let mut installed = Vec::new();
        for (package, interface) in index.packages() {
            let mut records = BTreeMap::new();
            let mut traits = BTreeMap::new();
            let mut extensions = Vec::new();
            if let Some(signatures) = interface.signatures() {
                for record in &signatures.records {
                    if record.definition.module.package != *package
                        || !interface
                            .records()
                            .iter()
                            .any(|item| item.definition == record.definition)
                        || records
                            .insert(record.definition.clone(), record.clone())
                            .is_some()
                    {
                        return Err(self.db.invalid_input(
                            &CompiledPackageInterfaceIndexQuery,
                            format!(
                                "compiled signature identity is inconsistent: {:?}",
                                record.definition
                            ),
                        ));
                    }
                }
                for record in &signatures.traits {
                    if record.definition.module.package != *package
                        || !interface
                            .records()
                            .iter()
                            .any(|item| item.definition == record.definition)
                        || traits
                            .insert(record.definition.clone(), record.clone())
                            .is_some()
                    {
                        return Err(self.db.invalid_input(
                            &CompiledPackageInterfaceIndexQuery,
                            "compiled trait signature identity is inconsistent",
                        ));
                    }
                }
                for extension in &signatures.extensions {
                    if extension.members.iter().any(|member| {
                        member.definition.module.package != *package
                            || !interface
                                .records()
                                .iter()
                                .any(|item| item.definition == member.definition)
                    }) || extension.module.package != *package
                        || extensions.iter().any(
                            |existing: &nia_package_metadata::SignatureExtensionRecord| {
                                existing.module == extension.module
                                    && existing.impl_id == extension.impl_id
                            },
                        )
                    {
                        return Err(self.db.invalid_input(
                            &CompiledPackageInterfaceIndexQuery,
                            "compiled extension signature identity is inconsistent",
                        ));
                    }
                    extensions.push(extension.clone());
                }
            }
            let key = CompiledPackageSignaturesQuery(package.clone());
            if self.db.can_publish_shared(key.clone()) {
                self.db.publish_shared(
                    key,
                    CompiledPackageSignatures {
                        package: package.clone(),
                        records,
                        traits,
                        extensions,
                    },
                    &CompiledPackageInterfaceIndexQuery,
                );
            }
            installed.push(package.clone());
        }
        Ok(installed)
    }

    /// Consumes one package's artifact-backed signature facts.
    pub fn compiled_package_signatures(
        &self,
        package: PackageId,
    ) -> QueryResult<CompiledPackageSignatures> {
        self.db
            .get(CompiledPackageSignaturesQuery(package))
            .map(|signatures| signatures.as_ref().clone())
    }

    /// Rehydrates artifact signature roots into the current type store. This
    /// is the semantic bridge used by downstream providers; it validates that
    /// every generic/where/trait/extension root points into the selected
    /// package graph before exposing session-local type handles.
    pub fn rehydrate_compiled_signature_roots(
        &self,
        resolver: &dyn StableDefinitionResolver,
    ) -> QueryResult<BTreeMap<DefinitionId, Vec<InternedTyId>>> {
        let index = self.compiled_package_interface_index()?;
        let mut result = BTreeMap::new();
        for (package, interface) in index.packages() {
            let types = interface
                .type_graph()
                .map(|graph| self.rehydrate_stable_type_graph(graph, resolver))
                .transpose()?
                .unwrap_or_default();
            let signatures = self.compiled_package_signatures(package.clone())?;
            for (definition, record) in signatures.iter() {
                let roots = record
                    .type_roots
                    .iter()
                    .map(|root| {
                        types.get(*root as usize).copied().ok_or_else(|| {
                            self.db.invalid_input(
                                &CompiledPackageInterfaceIndexQuery,
                                "compiled signature root is outside its graph",
                            )
                        })
                    })
                    .collect::<QueryResult<Vec<_>>>()?;
                for param in &record.generic_params {
                    if let Some(root) = param.type_root {
                        if types.get(root as usize).is_none() {
                            return Err(self.db.invalid_input(
                                &CompiledPackageInterfaceIndexQuery,
                                "compiled generic parameter root is outside its graph",
                            ));
                        }
                    }
                }
                for predicate in &record.where_predicates {
                    for root in std::iter::once(predicate.type_root).chain(
                        predicate.bounds.iter().flat_map(|bound| {
                            std::iter::once(bound.trait_root).chain(
                                bound
                                    .associated_type_bindings
                                    .iter()
                                    .map(|binding| binding.type_root),
                            )
                        }),
                    ) {
                        if types.get(root as usize).is_none() {
                            return Err(self.db.invalid_input(
                                &CompiledPackageInterfaceIndexQuery,
                                "compiled where predicate root is outside its graph",
                            ));
                        }
                    }
                }
                if result.insert(definition.clone(), roots).is_some() {
                    return Err(self.db.invalid_input(
                        &CompiledPackageInterfaceIndexQuery,
                        "duplicate compiled signature during root rehydration",
                    ));
                }
            }
        }
        Ok(result)
    }

    /// Returns decoded closure summaries for every installed template. Stable
    /// definition identities remain intact until an explicit remap step.
    pub fn compiled_template_summaries(
        &self,
    ) -> QueryResult<BTreeMap<DefinitionId, nia_closure_check::ImportedClosureEscapeSummary>> {
        let mut summaries = BTreeMap::new();
        let index = self.compiled_package_interface_index()?;
        for (package, _) in index.packages() {
            let templates = self.compiled_package_templates(package.clone())?;
            for (definition, _) in templates.iter() {
                let summary = templates.summary(definition)?.ok_or_else(|| {
                    self.db.invalid_input(
                        &CompiledPackageInterfaceIndexQuery,
                        "installed template is missing its semantic summary".to_string(),
                    )
                })?;
                let imported =
                    nia_closure_check::ImportedClosureEscapeSummary::from_parameter_sets(
                        summary.returned_parameters,
                        summary.escaping_parameters,
                        summary.returned_captured_address_parameters,
                        summary.escaping_captured_address_parameters,
                    )
                    .map_err(|message| {
                        self.db
                            .invalid_input(&CompiledPackageInterfaceIndexQuery, message)
                    })?;
                summaries.insert(definition.clone(), imported);
            }
        }
        Ok(summaries)
    }

    /// Remaps all installed template summaries into current-session function
    /// identities for interprocedural analyses such as closure escape checking.
    pub fn compiled_template_summaries_for_session(
        &self,
        resolver: &dyn StableDefinitionResolver,
    ) -> QueryResult<HashMap<GlobalDefId, nia_closure_check::ImportedClosureEscapeSummary>> {
        self.compiled_template_summaries()?
            .into_iter()
            .map(|(identity, summary)| {
                resolver
                    .definition_for_identity(&identity)
                    .map(|definition| (definition, summary))
            })
            .collect()
    }

    /// Publishes the target-specific native section for one selected package.
    pub fn install_compiled_package_native(&self) -> QueryResult<Vec<PackageId>> {
        let index = self.compiled_package_interface_index()?;
        let _observation = self.db.get(CompiledPackageNativeObservationQuery)?;
        let mut installed = Vec::new();
        for (package, interface) in index.packages() {
            let Some(section) = interface.native() else {
                continue;
            };
            let Some(variant) = section.variant(native_optimization_tag(self)) else {
                continue;
            };
            let key = CompiledPackageNativeQuery(package.clone());
            if self.db.can_publish_owned(key.clone()) {
                self.db.publish_owned(
                    key,
                    CompiledPackageNative {
                        package: package.clone(),
                        variant: variant.clone(),
                    },
                    &CompiledPackageNativeObservationQuery,
                );
            }
            installed.push(package.clone());
        }
        Ok(installed)
    }

    /// Consumes one target-specific native package product.
    pub fn compiled_package_native(
        &self,
        package: PackageId,
    ) -> QueryResult<CompiledPackageNative> {
        self.db.get_owned(CompiledPackageNativeQuery(package))
    }

    /// Returns all target-compatible native products selected from compiled
    /// package artifacts. Products are query-owned and therefore participate
    /// in normal invalidation when an artifact is replaced.
    pub fn compiled_package_native_products(&self) -> QueryResult<Vec<CompiledPackageNative>> {
        let index = self.compiled_package_interface_index()?;
        let mut products = Vec::new();
        for (package, _) in index.packages() {
            if let Ok(product) = self.compiled_package_native(package.clone()) {
                products.push(product);
            }
        }
        Ok(products)
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

    /// Rehydrates every selected compiled interface into the current type
    /// store and returns per-definition signature roots. This is the common
    /// semantic input boundary for artifact-backed providers.
    pub fn rehydrate_compiled_interface_type_roots(
        &self,
        resolver: &dyn StableDefinitionResolver,
    ) -> QueryResult<BTreeMap<DefinitionId, Vec<InternedTyId>>> {
        let index = self.compiled_package_interface_index()?;
        let mut result = BTreeMap::new();
        for (package, interface) in index.packages() {
            let types = interface
                .type_graph()
                .map(|graph| self.rehydrate_stable_type_graph_nodes(graph, resolver))
                .transpose()?
                .unwrap_or_default();
            self.publish_compiled_package_type_graph(package.clone(), types.clone())?;
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

    pub fn publish_compiled_package_type_graph(
        &self,
        package: PackageId,
        types: Vec<InternedTyId>,
    ) -> QueryResult<()> {
        let index = self.compiled_package_interface_index()?;
        if index.package(&package).is_none() {
            return Err(self.db.invalid_input(
                &CompiledPackageInterfaceIndexQuery,
                format!(
                    "cannot publish compiled type graph for an unselected package: {package:?}"
                ),
            ));
        }
        let key = CompiledPackageTypeGraphQuery(package.clone());
        if self.db.can_publish_shared(key.clone()) {
            self.db.publish_shared(
                key,
                CompiledPackageTypeGraph { package, types },
                &CompiledPackageInterfaceIndexQuery,
            );
        }
        Ok(())
    }

    pub fn compiled_package_type_graph(
        &self,
        package: &PackageId,
    ) -> QueryResult<CompiledPackageTypeGraph> {
        self.db
            .get(CompiledPackageTypeGraphQuery(package.clone()))
            .map(|graph| graph.as_ref().clone())
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
            if declaration.kind != record.definition.kind {
                return Err(self.db.invalid_input(
                    &CompiledPackageInterfaceIndexQuery,
                    "compiled declaration identity is inconsistent".to_string(),
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

    /// Installs decoded declaration inventories for every selected package.
    /// The interface index remains the sole predecessor, so replacement or
    /// retirement of an artifact invalidates all published declaration facts.
    pub fn install_compiled_package_declarations(&self) -> QueryResult<Vec<PackageId>> {
        let index = self.compiled_package_interface_index()?;
        let mut installed = Vec::new();
        for (package, _) in index.packages() {
            self.publish_compiled_package_declarations(package.clone())?;
            installed.push(package.clone());
        }
        Ok(installed)
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
        resolve_loaded_definition_in_query(&self.db, definition, package)
    }

    fn resolve_compiled_module_identity(&self, identity: &StableModuleId) -> QueryResult<ModuleId> {
        let graph = self.db.get(ModuleGraphQuery)?;
        for module in graph.modules() {
            if self
                .db
                .context()
                .loader_facts()
                .compiled_package_module_identity(module.id)?
                .as_ref()
                == Some(identity)
            {
                return Ok(module.id);
            }
        }
        Err(self.db.invalid_input(
            &CompiledPackageInterfaceIndexQuery,
            format!("compiled template module is not loaded: {identity:?}"),
        ))
    }

    fn resolve_template_definition(
        &self,
        identity: &DefinitionId,
        package: &PackageId,
        interface: &nia_package_metadata::CompiledPackageInterface,
    ) -> QueryResult<GlobalDefId> {
        if &identity.module.package != package {
            return Err(self.db.invalid_input(
                &CompiledPackageInterfaceIndexQuery,
                "compiled template relocation belongs to a different package",
            ));
        }
        if let Ok(module_id) = self.resolve_compiled_module_identity(&identity.module) {
            if interface.definition(identity).is_none() {
                return Err(self.db.invalid_input(
                    &CompiledPackageInterfaceIndexQuery,
                    format!("compiled template definition is absent from interface: {identity:?}"),
                ));
            }
            return Ok(GlobalDefId {
                module_id,
                def_id: DefId(identity.disambiguator),
            });
        }
        self.resolve_loaded_definition(identity, package)
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
                if let Some(identity) = self
                    .db
                    .context()
                    .loader_facts()
                    .compiled_package_module_identity(def_id.module_id)?
                {
                    return Ok(identity.package);
                }
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
    fn checked_template_bodies(
        &self,
        interface: &InterfaceSection,
        resolver: &dyn StableDefinitionPackageResolver,
    ) -> QueryResult<Vec<PublishedTemplateBody>> {
        let stable_index = self.stable_definition_index(resolver)?;
        let mut bodies = Vec::new();
        for record in &interface.records {
            if record.definition.kind != def_kind_tag(nia_defs::DefKind::Function)
                && record.definition.kind != def_kind_tag(nia_defs::DefKind::Method)
                && record.definition.kind != def_kind_tag(nia_defs::DefKind::TraitMethod)
            {
                continue;
            }
            let global = stable_index.definition_for_identity(&record.definition)?;
            let signatures = self.db.get(ItemSignaturesQuery(global.module_id))?;
            let Some(signature) = signatures.semantic.functions.get(&global.def_id) else {
                continue;
            };
            let defs = full_module_defs_semantic(&self.db, global.module_id)?;
            let effective_generic_params = providers::codegen::effective_function_generic_params(
                &signatures.semantic,
                &defs,
                global.def_id,
            );
            let specializes_self = defs
                .defs
                .get(global.def_id)
                .is_some_and(|def| def.kind == nia_defs::DefKind::TraitMethod);
            // Extern/builtin const declarations are callable by the evaluator
            // but have no source body to publish. Trait defaults are templates
            // even without explicit generics because every implementation
            // specializes their implicit `Self` parameter.
            if !signature.has_body
                || (effective_generic_params.is_empty() && !signature.is_const && !specializes_self)
            {
                continue;
            }
            let checked = self.db.get(CheckedModuleQuery(global.module_id))?;
            let const_templates;
            let typed = match checked.body_ir.function_bodies.get(&global) {
                Some(body) => body,
                None if signature.is_const => {
                    const_templates =
                        providers::body_check_const_templates(&self.db, global.module_id)?;
                    const_templates
                        .ir
                        .function_bodies
                        .get(&global)
                        .ok_or_else(|| {
                            self.db.invalid_input(
                                &ModuleGraphQuery,
                                format!(
                                    "published const template has no checked runtime body: {:?}",
                                    record.definition
                                ),
                            )
                        })?
                }
                None => {
                    return Err(self.db.invalid_input(
                        &ModuleGraphQuery,
                        format!(
                            "published template has no checked body: {:?}",
                            record.definition
                        ),
                    ));
                }
            };
            let lowered = nia_function_lower::lower_function_body(
                global.module_id,
                typed,
                nia_function_lower::FunctionTypeContext::for_module(
                    &self.db.context().type_store,
                    global.module_id,
                ),
            )
            .map_err(|diagnostic| {
                self.db.invalid_input(
                    &ModuleGraphQuery,
                    format!(
                        "failed to lower published template {:?}: {}",
                        record.definition, diagnostic.message
                    ),
                )
            })?;
            let ctfe_body = if signature.is_const {
                Some(
                    self.db
                        .get(ConstModuleQuery(global.module_id))?
                        .module
                        .functions()
                        .get(&global)
                        .cloned()
                        .ok_or_else(|| {
                            self.db.invalid_input(
                                &ModuleGraphQuery,
                                format!(
                                    "published const template has no resolved CTFE body: {:?}",
                                    record.definition
                                ),
                            )
                        })?,
                )
            } else {
                None
            };
            bodies.push(PublishedTemplateBody {
                definition: record.definition.clone(),
                global,
                body: lowered.body,
                closure_entries: lowered.closure_entries,
                ctfe_body,
                parameter_count: u32::try_from(signature.params.len()).map_err(|_| {
                    self.db.invalid_input(
                        &ModuleGraphQuery,
                        "template parameter count overflows u32".to_string(),
                    )
                })?,
            });
        }
        Ok(bodies)
    }

    /// Publishes the canonical checked template inventory for one package.
    fn package_template_section_with_resolver(
        &self,
        package: PackageId,
        interface: &InterfaceSection,
        type_indexes: &HashMap<InternedTyId, u32>,
        resolver: &dyn StableDefinitionPackageResolver,
    ) -> QueryResult<nia_package_metadata::TemplateSection> {
        let graph = self.db.get(ModuleGraphQuery)?;
        let stable_index = self.stable_definition_index(resolver)?;
        let entry_root = graph.current_package_root(graph.entry());
        let std_root = graph.std_package_root();
        let runtime_root = graph.package_root(&nia_symbol::known::RUNTIME);
        let runtime_package = match self.db.get(CompilerRuntimeQuery)?.as_ref() {
            RuntimeSpec::Source(runtime) => Some(runtime.package().clone()),
            RuntimeSpec::Bare => None,
        };
        let mut module_identities = HashMap::new();
        for module in graph.modules() {
            let Some(key) = graph.stable_key(module.id) else {
                continue;
            };
            let is_std_module = package == PackageId::standard_library()
                && std_root.is_some_and(|root| {
                    module.id == root || graph.current_package_root(module.id) == Some(root)
                });
            let is_runtime_module = runtime_root.is_some_and(|root| {
                module.id == root || graph.current_package_root(module.id) == Some(root)
            });
            let owner = if graph.current_package_root(module.id) == entry_root || is_std_module {
                package.clone()
            } else if is_runtime_module {
                runtime_package.clone().ok_or_else(|| {
                    self.db.invalid_input(
                        &ModuleGraphQuery,
                        "runtime module present without a source runtime package".to_string(),
                    )
                })?
            } else {
                self.db
                    .context()
                    .loader_facts()
                    .compiled_package_module_identity(module.id)?
                    .map(|identity| identity.package)
                    .unwrap_or_else(|| package.clone())
            };
            module_identities.insert(
                module.id,
                StableModuleId {
                    package: owner,
                    path: key.source_identity().normalized_path().to_owned(),
                },
            );
        }
        let bodies = self
            .checked_template_bodies(interface, resolver)?
            .into_iter()
            .filter(|body| body.definition.module.package == package)
            .collect::<Vec<_>>();
        let checked_modules = graph
            .modules()
            .filter(|module| graph.current_package_root(module.id) == entry_root)
            .map(|module| self.db.get(CheckedModuleQuery(module.id)))
            .collect::<QueryResult<Vec<_>>>()?;
        let closure_check = providers::closure_safety_check(&self.db, &checked_modules)?;
        let mut records = Vec::with_capacity(bodies.len());
        for template in bodies {
            let mut relocations = collect_checked_function_body_relocations(&template.body)
                .map_err(|e| {
                    self.db.invalid_input(
                        &ModuleGraphQuery,
                        format!("failed to collect template relocations: {e}"),
                    )
                })?;
            let closure_relocations = collect_checked_closure_entry_relocations(
                &template.closure_entries,
            )
            .map_err(|e| {
                self.db.invalid_input(
                    &ModuleGraphQuery,
                    format!("failed to collect template closure relocations: {e}"),
                )
            })?;
            relocations.types.extend(closure_relocations.types);
            relocations
                .definitions
                .extend(closure_relocations.definitions);
            relocations.modules.extend(closure_relocations.modules);
            if let Some(ctfe_body) = &template.ctfe_body {
                let ctfe_relocations = collect_resolved_const_function_relocations(ctfe_body)
                    .map_err(|e| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            format!("failed to collect CTFE template relocations: {e}"),
                        )
                    })?;
                relocations.types.extend(ctfe_relocations.types);
                relocations.definitions.extend(ctfe_relocations.definitions);
                relocations.modules.extend(ctfe_relocations.modules);
            }
            let mut definitions = relocations
                .definitions
                .iter()
                .copied()
                .map(|global| {
                    stable_index
                        .iter()
                        .find_map(|(identity, candidate)| {
                            (*candidate == global).then_some(identity.clone())
                        })
                        .ok_or_else(|| {
                            self.db.invalid_input(
                                &ModuleGraphQuery,
                                format!("template references unpublished definition: {global:?}"),
                            )
                        })
                })
                .collect::<QueryResult<Vec<_>>>()?;
            definitions.sort();
            definitions.dedup();
            let mut modules = relocations
                .modules
                .iter()
                .copied()
                .map(|module| {
                    module_identities.get(&module).cloned().ok_or_else(|| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            format!("template references unpublished module: {module:?}"),
                        )
                    })
                })
                .collect::<QueryResult<Vec<_>>>()?;
            modules.sort();
            modules.dedup();
            let mut type_pairs = relocations
                .types
                .iter()
                .copied()
                .map(|ty| {
                    type_indexes
                        .get(&ty)
                        .copied()
                        .map(|index| (ty, index))
                        .ok_or_else(|| {
                            self.db.invalid_input(
                                &ModuleGraphQuery,
                                "template type is absent from published graph".to_string(),
                            )
                        })
                })
                .collect::<QueryResult<Vec<_>>>()?;
            type_pairs.sort_by_key(|(ty, index)| (*index, *ty));
            type_pairs.dedup_by_key(|(ty, _)| *ty);
            type_pairs.sort_by_key(|(_, index)| *index);
            let mut type_roots = type_pairs
                .iter()
                .map(|(_, index)| *index)
                .collect::<Vec<_>>();
            type_roots.dedup();
            let mut context = TemplateEncodeContext {
                types: HashMap::new(),
                definitions: HashMap::new(),
                modules: HashMap::new(),
            };
            for (ty, graph_index) in type_pairs.iter().copied() {
                let index = type_roots.binary_search(&graph_index).map_err(|_| {
                    self.db.invalid_input(
                        &ModuleGraphQuery,
                        "template type relocation root ordering mismatch".to_string(),
                    )
                })?;
                context.types.insert(
                    ty,
                    u32::try_from(index).map_err(|_| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            "template type index overflows u32".to_string(),
                        )
                    })?,
                );
            }
            for global in relocations.definitions.iter().copied() {
                let identity = stable_index
                    .iter()
                    .find_map(|(identity, candidate)| (*candidate == global).then_some(identity))
                    .ok_or_else(|| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            format!("template references unpublished definition: {global:?}"),
                        )
                    })?;
                let index = definitions.binary_search(identity).map_err(|_| {
                    self.db.invalid_input(
                        &ModuleGraphQuery,
                        "template definition relocation ordering mismatch".to_string(),
                    )
                })?;
                context.definitions.insert(
                    global,
                    u32::try_from(index).map_err(|_| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            "template definition index overflows u32".to_string(),
                        )
                    })?,
                );
            }
            for module in relocations.modules.iter().copied() {
                let identity = module_identities.get(&module).ok_or_else(|| {
                    self.db.invalid_input(
                        &ModuleGraphQuery,
                        format!("template references unpublished module: {module:?}"),
                    )
                })?;
                let index = modules.binary_search(identity).map_err(|_| {
                    self.db.invalid_input(
                        &ModuleGraphQuery,
                        "template module relocation ordering mismatch".to_string(),
                    )
                })?;
                context.modules.insert(
                    module,
                    u32::try_from(index).map_err(|_| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            "template module index overflows u32".to_string(),
                        )
                    })?,
                );
            }
            let body = encode_checked_function_body(&template.body, &context).map_err(|e| {
                self.db.invalid_input(
                    &ModuleGraphQuery,
                    format!("failed to encode template body: {e}"),
                )
            })?;
            let closure_entries =
                encode_checked_closure_entries(&template.closure_entries, &context).map_err(
                    |e| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            format!("failed to encode template closure entries: {e}"),
                        )
                    },
                )?;
            let ctfe_body = template
                .ctfe_body
                .as_ref()
                .map(|body| encode_resolved_const_function(body, &context))
                .transpose()
                .map_err(|e| {
                    self.db.invalid_input(
                        &ModuleGraphQuery,
                        format!("failed to encode CTFE template body: {e}"),
                    )
                })?
                .unwrap_or_default();
            let summary = closure_check
                .summaries
                .get(&template.global)
                .cloned()
                .unwrap_or_default();
            let summary = nia_package_metadata::TemplateSummary {
                returned_parameters: summary
                    .returned_parameters
                    .into_iter()
                    .map(|v| v as u32)
                    .collect(),
                escaping_parameters: summary
                    .escaping_parameters
                    .into_iter()
                    .map(|v| v as u32)
                    .collect(),
                returned_captured_address_parameters: summary
                    .returned_captured_address_parameters
                    .into_iter()
                    .map(|v| v as u32)
                    .collect(),
                escaping_captured_address_parameters: summary
                    .escaping_captured_address_parameters
                    .into_iter()
                    .map(|v| v as u32)
                    .collect(),
            };
            let summary = nia_package_metadata::encode_template_summary(&summary)
                .map_err(|e| self.db.invalid_input(&ModuleGraphQuery, e.to_string()))?;
            records.push(nia_package_metadata::TemplateRecord {
                definition: template.definition,
                parameter_count: template.parameter_count,
                referenced_definitions: definitions,
                referenced_modules: modules,
                type_roots,
                body,
                closure_entries,
                ctfe_body,
                summary,
            });
        }
        records.sort_by(|left, right| left.definition.cmp(&right.definition));
        let section = nia_package_metadata::TemplateSection { records };
        section
            .validate()
            .map_err(|e| self.db.invalid_input(&ModuleGraphQuery, e.to_string()))?;
        Ok(section)
    }

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
                if let Some(identity) = self
                    .db
                    .context()
                    .loader_facts()
                    .compiled_package_module_identity(def_id.module_id)?
                {
                    return Ok(identity.package);
                }
                return Err(self.db.invalid_input(
                    &ModuleGraphQuery,
                    "nominal type belongs to an external package; provide an external definition resolver"
                        .to_string(),
                ));
            }
            Ok(package_for_resolver.clone())
        };
        self.package_interface_and_type_graph(package, &resolver)
            .map(|(interface, _, _)| interface)
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
            .map(|(interface, _, _)| interface)
    }

    /// Derives the stable signature inventory from the same source query
    /// products used to build the package interface. This keeps publication
    /// deterministic while preserving the session boundary: only canonical
    /// identities, flags, and stable type-graph indexes cross into metadata.
    pub fn package_signature_section_with_resolver(
        &self,
        package: PackageId,
        resolver: &dyn StableDefinitionPackageResolver,
    ) -> QueryResult<nia_package_metadata::SignatureSection> {
        let (interface, _, indexes) =
            self.package_interface_and_type_graph(package.clone(), resolver)?;
        self.signature_section_from_interface(package, resolver, interface, &indexes)
    }

    fn append_interface_definition_chain(
        &self,
        definition: &DefinitionId,
        stable_index: &StableDefinitionIndex,
        published_definitions: &mut std::collections::BTreeSet<DefinitionId>,
        pending: &mut Vec<(InterfaceRecord, Vec<InternedTyId>)>,
        all_roots: &mut Vec<InternedTyId>,
    ) -> QueryResult<bool> {
        let mut chain = Vec::new();
        let mut current = Some(definition.clone());
        while let Some(identity) = current {
            current = identity.owner.as_deref().cloned();
            chain.push(identity);
        }
        chain.reverse();

        let mut changed = false;
        for identity in chain {
            if !published_definitions.insert(identity.clone()) {
                continue;
            }
            let global = stable_index.definition(&identity).ok_or_else(|| {
                self.db.invalid_input(
                    &ModuleGraphQuery,
                    format!("interface definition is absent from definition index: {identity:?}"),
                )
            })?;
            let defs = self.db.get(FullModuleDefsQuery(global.module_id))?;
            let def = defs.semantic.defs.get(global.def_id).ok_or_else(|| {
                self.db.invalid_input(
                    &ModuleGraphQuery,
                    "interface definition is missing from module facts".to_string(),
                )
            })?;
            let roots = self
                .db
                .get(ItemSignaturesQuery(global.module_id))?
                .semantic
                .type_roots_for_definition(global.def_id)
                .unwrap_or_default();
            all_roots.extend(roots.iter().copied());
            pending.push((
                InterfaceRecord {
                    definition: identity,
                    declaration: declaration_signature(def),
                    type_roots: Vec::new(),
                },
                roots,
            ));
            changed = true;
        }
        Ok(changed)
    }

    fn close_interface_signature_members(
        &self,
        stable_index: &StableDefinitionIndex,
        published_definitions: &mut std::collections::BTreeSet<DefinitionId>,
        pending: &mut Vec<(InterfaceRecord, Vec<InternedTyId>)>,
        all_roots: &mut Vec<InternedTyId>,
    ) -> QueryResult<bool> {
        let mut changed_any = false;
        loop {
            let mut required = Vec::new();
            for definition in published_definitions.iter() {
                let Some(global) = stable_index.definition(definition) else {
                    continue;
                };
                let defs = self.db.get(FullModuleDefsQuery(global.module_id))?;
                let Some(owner) = defs.semantic.defs.get(global.def_id) else {
                    continue;
                };
                for (child_id, child) in defs.semantic.defs.iter() {
                    if child.parent == Some(global.def_id)
                        && signature_owner_requires_child(owner.kind, child.kind)
                    {
                        let child_global = GlobalDefId {
                            module_id: global.module_id,
                            def_id: child_id,
                        };
                        let identity = stable_index
                            .iter()
                            .find_map(|(identity, candidate)| {
                                (*candidate == child_global).then_some(identity.clone())
                            })
                            .ok_or_else(|| {
                                self.db.invalid_input(
                                    &ModuleGraphQuery,
                                    format!(
                                        "signature member is absent from definition index: {child_global:?}"
                                    ),
                                )
                            })?;
                        if !published_definitions.contains(&identity) {
                            required.push(identity);
                        }
                    }
                }
            }
            required.sort();
            required.dedup();
            if required.is_empty() {
                break;
            }
            for definition in required {
                changed_any |= self.append_interface_definition_chain(
                    &definition,
                    stable_index,
                    published_definitions,
                    pending,
                    all_roots,
                )?;
            }
        }
        Ok(changed_any)
    }

    fn signature_section_from_interface(
        &self,
        package: PackageId,
        resolver: &dyn StableDefinitionPackageResolver,
        interface: InterfaceSection,
        type_indexes: &HashMap<nia_ids::InternedTyId, u32>,
    ) -> QueryResult<nia_package_metadata::SignatureSection> {
        let definition_index = self.stable_definition_index(resolver)?;
        let mut records = Vec::with_capacity(interface.records.len());
        let mut trait_records = Vec::new();
        let mut extension_records = Vec::new();
        let mut stable_by_global = HashMap::new();
        let mut signature_definitions = std::collections::BTreeSet::new();
        let mut members =
            BTreeMap::<DefinitionId, Vec<nia_package_metadata::SignatureMember>>::new();
        for item in &interface.records {
            let global = definition_index.definition_for_identity(&item.definition)?;
            stable_by_global.insert(global, item.definition.clone());
        }
        for item in &interface.records {
            let global = definition_index.definition_for_identity(&item.definition)?;
            let kind = item.definition.kind;
            let declaration = nia_package_metadata::decode_declaration(&item.declaration)
                .map_err(|error| self.db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
            let facts = self.db.get(ItemSignaturesQuery(global.module_id))?;
            let defs = self.db.get(FullModuleDefsQuery(global.module_id))?;
            let flags = signature_flags_for_definition(global.def_id, kind, &facts.semantic);
            let (generic_params, where_predicates) = signature_generic_and_where_facts(
                global.def_id,
                &facts.semantic,
                &defs.semantic,
                &self.db.context().loader_facts().symbols(),
                type_indexes,
            )?;
            let payload = signature_payload_for_definition(
                global.def_id,
                global.module_id,
                kind,
                &facts.semantic,
                &stable_by_global,
                type_indexes,
                &self.db.context().loader_facts().symbols(),
                &self.db,
            )?;
            if nia_package_metadata::signature_kind_requires_payload(kind) && payload.is_none() {
                if declaration.visibility != 3 {
                    continue;
                }
                return Err(self.db.invalid_input(
                    &ModuleGraphQuery,
                    format!(
                        "semantic definition has no complete signature payload: {:?}",
                        item.definition
                    ),
                ));
            }
            if let Some(owner) = item.definition.owner.as_deref() {
                members.entry(owner.clone()).or_default().push(
                    nia_package_metadata::SignatureMember {
                        definition: item.definition.clone(),
                        name: item.definition.name.clone(),
                        kind,
                        flags,
                        type_roots: item.type_roots.clone(),
                    },
                );
            }
            records.push((
                item.definition.clone(),
                kind,
                flags,
                item.type_roots.clone(),
                generic_params,
                where_predicates,
                payload,
            ));
            signature_definitions.insert(item.definition.clone());
        }
        for item in interface
            .records
            .iter()
            .filter(|item| item.definition.kind == 9)
        {
            if !signature_definitions.contains(&item.definition) {
                continue;
            }
            let global = definition_index.definition_for_identity(&item.definition)?;
            let facts = self.db.get(ItemSignaturesQuery(global.module_id))?;
            let Some(trait_signature) = facts.semantic.traits.get(&global.def_id) else {
                continue;
            };
            let supertraits = trait_signature
                .supertraits
                .iter()
                .map(|bound| {
                    signature_supertrait_to_wire(
                        global.def_id,
                        bound,
                        &self.db.context().loader_facts().symbols(),
                        type_indexes,
                    )
                })
                .collect::<QueryResult<Vec<_>>>()?;
            let method_order = trait_signature
                .methods
                .iter()
                .map(|method| {
                    let global = GlobalDefId {
                        module_id: global.module_id,
                        def_id: method.def_id,
                    };
                    stable_by_global.get(&global).cloned().ok_or_else(|| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            format!("published trait method has no stable identity: {global:?}"),
                        )
                    })
                })
                .collect::<QueryResult<Vec<_>>>()?;
            trait_records.push(nia_package_metadata::SignatureTraitRecord {
                definition: item.definition.clone(),
                supertraits,
                members: members.get(&item.definition).cloned().unwrap_or_default(),
                method_order,
                builtin: trait_signature
                    .builtin
                    .map(nia_ids::BuiltinTrait::stable_tag),
            });
        }
        let module_graph = self.db.get(ModuleGraphQuery)?;
        let entry_root = module_graph.current_package_root(module_graph.entry());
        for module in module_graph.modules() {
            if entry_root.is_some() && module_graph.current_package_root(module.id) != entry_root {
                continue;
            }
            let facts = self.db.get(ItemSignaturesQuery(module.id))?;
            for implementation in &facts.semantic.trait_impls {
                let target_root = match type_indexes.get(&implementation.target_ty) {
                    Some(root) => *root,
                    None => continue,
                };
                let trait_root = implementation
                    .trait_ty
                    .map(|ty| type_indexes.get(&ty).copied())
                    .flatten();
                let generic_params = signature_generic_params_to_wire(
                    DefId(implementation.impl_id.0),
                    &implementation.generic_params,
                    &self.db.context().loader_facts().symbols(),
                    type_indexes,
                )?;
                let where_predicates = signature_where_predicates_to_wire(
                    DefId(implementation.impl_id.0),
                    &implementation.where_predicates,
                    &self.db.context().loader_facts().symbols(),
                    type_indexes,
                )?;
                let mut extension_members = Vec::new();
                for method in &implementation.methods {
                    if let Some(definition) = stable_by_global.get(&GlobalDefId {
                        module_id: module.id,
                        def_id: method.def_id,
                    }) {
                        if let Some(item) = interface.records.iter().find(|item| {
                            &item.definition == definition
                                && signature_definitions.contains(&item.definition)
                        }) {
                            extension_members.push(nia_package_metadata::SignatureMember {
                                definition: definition.clone(),
                                name: definition.name.clone(),
                                kind: definition.kind,
                                flags: signature_flags_for_definition(
                                    method.def_id,
                                    definition.kind,
                                    &facts.semantic,
                                ),
                                type_roots: item.type_roots.clone(),
                            });
                        }
                    }
                }
                for value in &implementation.associated_values {
                    if let Some(definition) = stable_by_global.get(&GlobalDefId {
                        module_id: module.id,
                        def_id: value.def_id,
                    }) {
                        if let Some(item) = interface.records.iter().find(|item| {
                            &item.definition == definition
                                && signature_definitions.contains(&item.definition)
                        }) {
                            extension_members.push(nia_package_metadata::SignatureMember {
                                definition: definition.clone(),
                                name: definition.name.clone(),
                                kind: definition.kind,
                                flags: 0,
                                type_roots: item.type_roots.clone(),
                            });
                        }
                    }
                }
                let associated_types = implementation
                    .associated_types
                    .iter()
                    .map(|associated| {
                        let name = self
                            .db
                            .context()
                            .loader_facts()
                            .symbols()
                            .symbol_text(associated.name)
                            .ok_or_else(|| {
                                self.db.invalid_input(
                                    &ModuleGraphQuery,
                                    "associated type has no stable symbol text",
                                )
                            })?
                            .to_string();
                        let type_root =
                            type_indexes.get(&associated.ty).copied().ok_or_else(|| {
                                self.db.invalid_input(
                                    &ModuleGraphQuery,
                                    "associated type missing from published graph",
                                )
                            })?;
                        Ok(nia_package_metadata::SignatureAssociatedType { name, type_root })
                    })
                    .collect::<QueryResult<Vec<_>>>()?;
                extension_records.push(nia_package_metadata::SignatureExtensionRecord {
                    module: nia_package_metadata::ModuleId {
                        package: package.clone(),
                        path: module_graph
                            .stable_key(module.id)
                            .ok_or_else(|| {
                                self.db.invalid_input(
                                    &ModuleGraphQuery,
                                    "extension module has no stable identity",
                                )
                            })?
                            .source_identity()
                            .normalized_path()
                            .to_owned(),
                    },
                    impl_id: implementation.impl_id.0,
                    target_root,
                    trait_root,
                    generic_params,
                    where_predicates,
                    members: extension_members,
                    associated_types,
                    builtin: implementation.builtin.clone(),
                });
            }
        }
        let records = records
            .into_iter()
            .map(
                |(
                    definition,
                    kind,
                    flags,
                    type_roots,
                    generic_params,
                    where_predicates,
                    payload,
                )| {
                    nia_package_metadata::SignatureRecord {
                        members: members.remove(&definition).unwrap_or_default(),
                        definition,
                        kind,
                        flags,
                        type_roots,
                        generic_params,
                        where_predicates,
                        payload,
                    }
                },
            )
            .collect();
        let section = nia_package_metadata::SignatureSection {
            records,
            traits: trait_records,
            extensions: extension_records,
        };
        let mut section = section;
        section
            .records
            .sort_by(|left, right| left.definition.cmp(&right.definition));
        section
            .traits
            .sort_by(|left, right| left.definition.cmp(&right.definition));
        section.extensions.sort_by(|left, right| {
            left.module
                .cmp(&right.module)
                .then_with(|| left.impl_id.cmp(&right.impl_id))
        });
        section.validate().map_err(|error| {
            self.db
                .invalid_input(&ModuleGraphQuery, format!("signature validate: {error}"))
        })?;
        Ok(section)
    }

    fn package_interface_and_type_graph(
        &self,
        package: PackageId,
        resolver: &dyn StableDefinitionPackageResolver,
    ) -> QueryResult<(
        InterfaceSection,
        StableTypeGraph,
        HashMap<nia_ids::InternedTyId, u32>,
    )> {
        let graph = self.db.get(ModuleGraphQuery)?;
        let symbols = self.db.context().loader_facts().symbols();
        let entry_package_root = graph.current_package_root(graph.entry());
        let mut pending = Vec::new();
        let mut module_signature_roots = Vec::new();
        let mut required_impl_members = std::collections::HashSet::new();
        for module in graph.modules() {
            if entry_package_root.is_some()
                && graph.current_package_root(module.id) != entry_package_root
            {
                continue;
            }
            let signatures = self.db.get(ItemSignaturesQuery(module.id))?;
            for implementation in &signatures.semantic.trait_impls {
                if implementation.trait_ty.is_none() {
                    continue;
                }
                required_impl_members.extend(implementation.methods.iter().map(|member| {
                    GlobalDefId {
                        module_id: module.id,
                        def_id: member.def_id,
                    }
                }));
                required_impl_members.extend(implementation.associated_values.iter().map(
                    |member| GlobalDefId {
                        module_id: module.id,
                        def_id: member.def_id,
                    },
                ));
            }
        }
        for module in graph.modules() {
            if entry_package_root.is_some()
                && graph.current_package_root(module.id) != entry_package_root
            {
                continue;
            }
            let Some(stable_key) = graph.stable_key(module.id) else {
                continue;
            };
            let module_path = stable_key.source_identity().normalized_path().to_owned();
            let defs = self.db.get(FullModuleDefsQuery(module.id))?;
            module_signature_roots.extend(
                self.db
                    .get(ItemSignaturesQuery(module.id))?
                    .semantic
                    .type_roots(),
            );
            for (def_id, def) in defs.semantic.defs.iter() {
                let global = GlobalDefId {
                    module_id: module.id,
                    def_id,
                };
                let mut include = def.visibility == nia_defs::Visibility::Public
                    || required_impl_members.contains(&global);
                let mut parent = def.parent;
                while !include {
                    let Some(parent_id) = parent else { break };
                    let Some(parent_def) = defs.semantic.defs.get(parent_id) else {
                        return Err(self.db.invalid_input(
                            &ModuleGraphQuery,
                            "definition parent is missing from module facts".to_string(),
                        ));
                    };
                    include = parent_def.visibility == nia_defs::Visibility::Public;
                    parent = parent_def.parent;
                }
                if !include {
                    continue;
                }
                let owner = resolver.package_for_definition(global)?;
                if owner != package {
                    continue;
                }
                let Some(name) = symbols.resolve(def.name) else {
                    continue;
                };
                let mut owner_chain = Vec::new();
                let mut parent = def.parent;
                while let Some(parent_id) = parent {
                    let parent_def = defs.semantic.defs.get(parent_id).ok_or_else(|| {
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
                            package: owner.clone(),
                            path: module_path.clone(),
                        },
                        name: owner_name,
                        kind: owner_kind,
                        disambiguator: owner_disambiguator,
                        owner: owner_identity,
                    }));
                }
                let definition = DefinitionId {
                    module: StableModuleId {
                        package: owner.clone(),
                        path: module_path.clone(),
                    },
                    name: name.to_string(),
                    kind: def_kind_tag(def.kind),
                    disambiguator: def_id.0,
                    owner: owner_identity,
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
            .chain(module_signature_roots)
            .collect::<Vec<_>>();
        let pending_interface = InterfaceSection {
            records: pending.iter().map(|(record, _)| record.clone()).collect(),
        };
        let template_bodies = self.checked_template_bodies(&pending_interface, resolver)?;
        let stable_index = self.stable_definition_index(resolver)?;
        let mut published_definitions = pending
            .iter()
            .map(|(record, _)| record.definition.clone())
            .collect::<std::collections::BTreeSet<_>>();
        for template in &template_bodies {
            let mut relocations = collect_checked_function_body_relocations(&template.body)
                .map_err(|e| {
                    self.db.invalid_input(
                        &ModuleGraphQuery,
                        format!("failed to collect template type roots: {e}"),
                    )
                })?;
            let closure_relocations = collect_checked_closure_entry_relocations(
                &template.closure_entries,
            )
            .map_err(|e| {
                self.db.invalid_input(
                    &ModuleGraphQuery,
                    format!("failed to collect template closure roots: {e}"),
                )
            })?;
            relocations.types.extend(closure_relocations.types);
            relocations
                .definitions
                .extend(closure_relocations.definitions);
            relocations.modules.extend(closure_relocations.modules);
            all_roots.extend(relocations.types);
            let mut referenced = relocations.definitions;
            if let Some(ctfe_body) = &template.ctfe_body {
                let relocations =
                    collect_resolved_const_function_relocations(ctfe_body).map_err(|e| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            format!("failed to collect CTFE template type roots: {e}"),
                        )
                    })?;
                all_roots.extend(relocations.types);
                referenced.extend(relocations.definitions);
            }
            for global in referenced {
                let Some((identity, _)) = stable_index.iter().find(|(identity, candidate)| {
                    **candidate == global && identity.module.package == package
                }) else {
                    return Err(self.db.invalid_input(
                        &ModuleGraphQuery,
                        format!("template references unpublished definition: {global:?}"),
                    ));
                };
                let mut identity = Some(identity.clone());
                while let Some(definition) = identity {
                    if !published_definitions.insert(definition.clone()) {
                        identity = definition.owner.as_deref().cloned();
                        continue;
                    }
                    let Some((_, global)) = stable_index
                        .iter()
                        .find(|(candidate, _)| **candidate == definition)
                    else {
                        return Err(self.db.invalid_input(
                            &ModuleGraphQuery,
                            format!(
                                "template owner is absent from definition index: {definition:?}"
                            ),
                        ));
                    };
                    let defs = self.db.get(FullModuleDefsQuery(global.module_id))?;
                    let def = defs.semantic.defs.get(global.def_id).ok_or_else(|| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            "template definition is missing from module facts".to_string(),
                        )
                    })?;
                    let roots = self
                        .db
                        .get(ItemSignaturesQuery(global.module_id))?
                        .semantic
                        .type_roots_for_definition(global.def_id)
                        .unwrap_or_default();
                    pending.push((
                        InterfaceRecord {
                            definition: definition.clone(),
                            declaration: declaration_signature(def),
                            type_roots: Vec::new(),
                        },
                        roots,
                    ));
                    identity = definition.owner.as_deref().cloned();
                }
            }
        }
        // Adding private definitions referenced by public templates can make
        // additional generic bodies eligible for publication. Include their
        // relocations in the same canonical graph before encoding templates.
        let complete_interface = InterfaceSection {
            records: pending.iter().map(|(record, _)| record.clone()).collect(),
        };
        for template in self.checked_template_bodies(&complete_interface, resolver)? {
            if published_definitions.insert(template.definition.clone()) {
                let Some((_, global)) = stable_index
                    .iter()
                    .find(|(candidate, _)| **candidate == template.definition)
                else {
                    return Err(self.db.invalid_input(
                        &ModuleGraphQuery,
                        format!(
                            "template owner is absent from definition index: {:?}",
                            template.definition
                        ),
                    ));
                };
                let defs = self.db.get(FullModuleDefsQuery(global.module_id))?;
                let def = defs.semantic.defs.get(global.def_id).ok_or_else(|| {
                    self.db.invalid_input(
                        &ModuleGraphQuery,
                        "template owner is missing from module facts".to_string(),
                    )
                })?;
                let roots = self
                    .db
                    .get(ItemSignaturesQuery(global.module_id))?
                    .semantic
                    .type_roots_for_definition(global.def_id)
                    .unwrap_or_default();
                pending.push((
                    InterfaceRecord {
                        definition: template.definition.clone(),
                        declaration: declaration_signature(def),
                        type_roots: Vec::new(),
                    },
                    roots,
                ));
            }
            let mut relocations = collect_checked_function_body_relocations(&template.body)
                .map_err(|e| {
                    self.db.invalid_input(
                        &ModuleGraphQuery,
                        format!("failed to collect private template type roots: {e}"),
                    )
                })?;
            let closure_relocations = collect_checked_closure_entry_relocations(
                &template.closure_entries,
            )
            .map_err(|e| {
                self.db.invalid_input(
                    &ModuleGraphQuery,
                    format!("failed to collect private template closure roots: {e}"),
                )
            })?;
            relocations.types.extend(closure_relocations.types);
            relocations
                .definitions
                .extend(closure_relocations.definitions);
            relocations.modules.extend(closure_relocations.modules);
            all_roots.extend(relocations.types);
            for global in relocations.definitions {
                let Some((identity, _)) = stable_index.iter().find(|(identity, candidate)| {
                    **candidate == global && identity.module.package == package
                }) else {
                    return Err(self.db.invalid_input(
                        &ModuleGraphQuery,
                        format!("template references unpublished definition: {global:?}"),
                    ));
                };
                if published_definitions.insert(identity.clone()) {
                    let defs = self.db.get(FullModuleDefsQuery(global.module_id))?;
                    let def = defs.semantic.defs.get(global.def_id).ok_or_else(|| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            "template definition is missing from module facts".to_string(),
                        )
                    })?;
                    let roots = self
                        .db
                        .get(ItemSignaturesQuery(global.module_id))?
                        .semantic
                        .type_roots_for_definition(global.def_id)
                        .unwrap_or_default();
                    pending.push((
                        InterfaceRecord {
                            definition: identity.clone(),
                            declaration: declaration_signature(def),
                            type_roots: Vec::new(),
                        },
                        roots,
                    ));
                }
            }
            if let Some(ctfe_body) = &template.ctfe_body {
                let relocations =
                    collect_resolved_const_function_relocations(ctfe_body).map_err(|e| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            format!("failed to collect private CTFE template type roots: {e}"),
                        )
                    })?;
                all_roots.extend(relocations.types);
                for global in relocations.definitions {
                    let Some((identity, _)) = stable_index.iter().find(|(identity, candidate)| {
                        **candidate == global && identity.module.package == package
                    }) else {
                        return Err(self.db.invalid_input(
                            &ModuleGraphQuery,
                            format!("CTFE template references unpublished definition: {global:?}"),
                        ));
                    };
                    if published_definitions.insert(identity.clone()) {
                        let defs = self.db.get(FullModuleDefsQuery(global.module_id))?;
                        let def = defs.semantic.defs.get(global.def_id).ok_or_else(|| {
                            self.db.invalid_input(
                                &ModuleGraphQuery,
                                "CTFE template definition is missing from module facts".to_string(),
                            )
                        })?;
                        let roots = self
                            .db
                            .get(ItemSignaturesQuery(global.module_id))?
                            .semantic
                            .type_roots_for_definition(global.def_id)
                            .unwrap_or_default();
                        pending.push((
                            InterfaceRecord {
                                definition: identity.clone(),
                                declaration: declaration_signature(def),
                                type_roots: Vec::new(),
                            },
                            roots,
                        ));
                    }
                }
            }
        }
        // Private generic templates can reference further private generic
        // templates through arbitrarily deep call chains. Close their
        // declaration ownership transitively instead of relying on a fixed
        // number of publication passes.
        loop {
            let interface_len_before = pending.len();
            let complete_interface = InterfaceSection {
                records: pending.iter().map(|(record, _)| record.clone()).collect(),
            };
            for template in self.checked_template_bodies(&complete_interface, resolver)? {
                let mut relocations = collect_checked_function_body_relocations(&template.body)
                    .map_err(|error| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            format!("failed to collect template closure roots: {error}"),
                        )
                    })?;
                let closure_relocations = collect_checked_closure_entry_relocations(
                    &template.closure_entries,
                )
                .map_err(|error| {
                    self.db.invalid_input(
                        &ModuleGraphQuery,
                        format!("failed to collect template closure roots: {error}"),
                    )
                })?;
                relocations.types.extend(closure_relocations.types);
                relocations
                    .definitions
                    .extend(closure_relocations.definitions);
                relocations.modules.extend(closure_relocations.modules);
                if let Some(ctfe_body) = &template.ctfe_body {
                    let ctfe = collect_resolved_const_function_relocations(ctfe_body).map_err(
                        |error| {
                            self.db.invalid_input(
                                &ModuleGraphQuery,
                                format!("failed to collect CTFE template closure roots: {error}"),
                            )
                        },
                    )?;
                    relocations.types.extend(ctfe.types);
                    relocations.definitions.extend(ctfe.definitions);
                }
                all_roots.extend(relocations.types);
                for global in relocations.definitions {
                    let Some((identity, _)) = stable_index.iter().find(|(identity, candidate)| {
                        **candidate == global && identity.module.package == package
                    }) else {
                        return Err(self.db.invalid_input(
                            &ModuleGraphQuery,
                            format!("template references unpublished definition: {global:?}"),
                        ));
                    };
                    let mut identity = Some(identity.clone());
                    while let Some(definition) = identity {
                        if !published_definitions.insert(definition.clone()) {
                            identity = definition.owner.as_deref().cloned();
                            continue;
                        }
                        let Some((_, global)) = stable_index
                            .iter()
                            .find(|(candidate, _)| **candidate == definition)
                        else {
                            return Err(self.db.invalid_input(
                                &ModuleGraphQuery,
                                format!(
                                    "template owner is absent from definition index: {definition:?}"
                                ),
                            ));
                        };
                        let defs = self.db.get(FullModuleDefsQuery(global.module_id))?;
                        let def = defs.semantic.defs.get(global.def_id).ok_or_else(|| {
                            self.db.invalid_input(
                                &ModuleGraphQuery,
                                "template definition is missing from module facts".to_string(),
                            )
                        })?;
                        let roots = self
                            .db
                            .get(ItemSignaturesQuery(global.module_id))?
                            .semantic
                            .type_roots_for_definition(global.def_id)
                            .unwrap_or_default();
                        pending.push((
                            InterfaceRecord {
                                definition: definition.clone(),
                                declaration: declaration_signature(def),
                                type_roots: Vec::new(),
                            },
                            roots,
                        ));
                        identity = definition.owner.as_deref().cloned();
                    }
                }
            }
            if pending.len() == interface_len_before {
                break;
            }
        }
        // Public extension records carry roots which are not necessarily
        // attached to a public item declaration (notably associated types and
        // their target/trait projections). Include those roots in the same
        // canonical graph before signature publication.
        for module in graph.modules() {
            if entry_package_root.is_some()
                && graph.current_package_root(module.id) != entry_package_root
            {
                continue;
            }
            let facts = self.db.get(ItemSignaturesQuery(module.id))?;
            for implementation in &facts.semantic.trait_impls {
                all_roots.push(implementation.target_ty);
                if let Some(trait_ty) = implementation.trait_ty {
                    all_roots.push(trait_ty);
                }
                for parameter in &implementation.generic_params {
                    if let nia_item_signatures::GenericParamSignatureKind::Const { ty } =
                        &parameter.kind
                    {
                        all_roots.push(*ty);
                    }
                }
                for predicate in &implementation.where_predicates {
                    all_roots.push(predicate.ty);
                    for bound in &predicate.bounds {
                        all_roots.push(bound.trait_ty);
                        all_roots.extend(
                            bound
                                .associated_type_bindings
                                .iter()
                                .map(|binding| binding.ty),
                        );
                    }
                }
                all_roots.extend(
                    implementation
                        .associated_types
                        .iter()
                        .map(|associated| associated.ty),
                );
            }
        }
        self.close_interface_signature_members(
            &stable_index,
            &mut published_definitions,
            &mut pending,
            &mut all_roots,
        )?;

        // Definition signatures and the stable type graph form one closure.
        // A signature member can introduce a new nominal type, and a nominal
        // type discovered in the graph requires its complete owned signature.
        // Rebuild until neither side contributes new definitions or roots.
        let (type_graph, indexes) = loop {
            all_roots.sort_unstable();
            all_roots.dedup();
            let (type_graph, indexes) = self
                .stable_type_graph_for_roots_with_resolver_and_indexes(&all_roots, resolver)
                .map_err(|error| {
                    self.db
                        .invalid_input(&ModuleGraphQuery, format!("stable graph: {error}"))
                })?;
            let mut graph_definitions = stable_type_graph_definitions(&type_graph);
            graph_definitions.sort();
            graph_definitions.dedup();
            let mut changed = false;
            for definition in graph_definitions {
                if definition.module.package != package {
                    continue;
                }
                changed |= self.append_interface_definition_chain(
                    &definition,
                    &stable_index,
                    &mut published_definitions,
                    &mut pending,
                    &mut all_roots,
                )?;
            }
            changed |= self.close_interface_signature_members(
                &stable_index,
                &mut published_definitions,
                &mut pending,
                &mut all_roots,
            )?;
            if !changed {
                break (type_graph, indexes);
            }
        };
        let mut records = pending
            .into_iter()
            .map(|(mut record, roots)| {
                let mut type_roots = roots
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
                type_roots.sort_unstable();
                type_roots.dedup();
                record.type_roots = type_roots;
                Ok(record)
            })
            .collect::<QueryResult<Vec<_>>>()?;
        records.sort_by(|left, right| left.definition.cmp(&right.definition));
        let section = InterfaceSection { records };
        section.validate().map_err(|error| {
            self.db
                .invalid_input(&ModuleGraphQuery, format!("interface validate: {error}"))
        })?;
        section.validate_type_roots(&type_graph).map_err(|error| {
            self.db
                .invalid_input(&ModuleGraphQuery, format!("interface roots: {error}"))
        })?;
        Ok((section, type_graph, indexes))
    }

    /// Publishes the fully resolved export surface for one package. Export
    /// targets are encoded as stable identities, so consumers never need to
    /// load dependency source or replay `using` resolution.
    pub fn package_public_surface_section_with_resolver(
        &self,
        package: PackageId,
        resolver: &dyn StableDefinitionPackageResolver,
    ) -> QueryResult<PublicSurfaceSection> {
        let parse_ok = self.db.get(ParseOkModuleIdsQuery)?;
        let module_ids = self
            .db
            .context()
            .resolve_stable_module_sequence(&parse_ok)?;
        let defs = module_ids
            .into_iter()
            .map(|module_id| {
                Ok(self
                    .db
                    .get(PublicSurfaceModuleFactsQuery(module_id))?
                    .materialize_for_public_surface(module_id))
            })
            .collect::<QueryResult<Vec<_>>>()?;
        let graph = self.db.get(ModuleGraphQuery)?;
        let symbols = self.db.context().loader_facts().symbols();
        let exports = compute_exported_public_surfaces_with_symbols(&defs, &graph, &symbols);
        let stable_target = |module_id: ModuleId, def_id: DefId| -> QueryResult<DefinitionId> {
            let key = graph.stable_key(module_id).ok_or_else(|| {
                self.db.invalid_input(
                    &ModuleGraphQuery,
                    "public export target has no stable module identity".to_string(),
                )
            })?;
            let owner = resolver.package_for_definition(GlobalDefId { module_id, def_id })?;
            let module = StableModuleId {
                package: owner,
                path: key.source_identity().normalized_path().to_owned(),
            };
            let module_defs = self.db.get(FullModuleDefsQuery(module_id))?;
            let def = module_defs.semantic.defs.get(def_id).ok_or_else(|| {
                self.db.invalid_input(
                    &ModuleGraphQuery,
                    "public export target definition is missing".to_string(),
                )
            })?;
            let name = symbols.resolve(def.name).ok_or_else(|| {
                self.db.invalid_input(
                    &ModuleGraphQuery,
                    "public export target has no symbol text".to_string(),
                )
            })?;
            let mut owner_chain = Vec::new();
            let mut parent = def.parent;
            while let Some(parent_id) = parent {
                let parent_def = module_defs.semantic.defs.get(parent_id).ok_or_else(|| {
                    self.db.invalid_input(
                        &ModuleGraphQuery,
                        "public export parent definition is missing".to_string(),
                    )
                })?;
                let parent_name = symbols.resolve(parent_def.name).ok_or_else(|| {
                    self.db.invalid_input(
                        &ModuleGraphQuery,
                        "public export parent has no symbol text".to_string(),
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
            for (name, kind, disambiguator) in owner_chain.into_iter().rev() {
                owner_identity = Some(Box::new(DefinitionId {
                    module: module.clone(),
                    name,
                    kind,
                    disambiguator,
                    owner: owner_identity,
                }));
            }
            Ok(DefinitionId {
                module,
                name: name.to_string(),
                kind: def_kind_tag(def.kind),
                disambiguator: def_id.0,
                owner: owner_identity,
            })
        };
        let mut modules = Vec::new();
        for (module_id, surface) in exports.surfaces.iter() {
            let path = graph
                .stable_key(*module_id)
                .ok_or_else(|| {
                    self.db.invalid_input(
                        &ModuleGraphQuery,
                        "public surface module has no stable identity".to_string(),
                    )
                })?
                .source_identity()
                .normalized_path()
                .to_owned();
            let mut child_modules = surface
                .modules
                .iter()
                .map(|(name, child)| {
                    let text = symbols.resolve(*name).ok_or_else(|| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            "public module export has no symbol text".to_string(),
                        )
                    })?;
                    let child_path = graph
                        .stable_key(*child)
                        .ok_or_else(|| {
                            self.db.invalid_input(
                                &ModuleGraphQuery,
                                "public child module has no stable identity".to_string(),
                            )
                        })?
                        .source_identity()
                        .normalized_path()
                        .to_owned();
                    Ok((
                        text.to_string(),
                        StableModuleId {
                            package: package.clone(),
                            path: child_path,
                        },
                    ))
                })
                .collect::<QueryResult<Vec<_>>>()?;
            child_modules.sort_by(|a, b| a.0.cmp(&b.0));
            let mut surface_exports = Vec::new();
            for (name, item, namespace) in surface
                .values
                .iter()
                .map(|(name, item)| (name, item, 0u8))
                .chain(surface.types.iter().map(|(name, item)| (name, item, 1u8)))
            {
                let spelling = symbols.resolve(*name).ok_or_else(|| {
                    self.db.invalid_input(
                        &ModuleGraphQuery,
                        "public export has no symbol text".to_string(),
                    )
                })?;
                let target = stable_target(item.target_module, item.target_def_id)?;
                let parent_enum = item
                    .parent_enum
                    .map(|global| stable_target(global.module_id, global.def_id))
                    .transpose()?;
                let source = match item.source {
                    PublicSource::Direct => 0,
                    PublicSource::PubUsing { .. } => 1,
                };
                surface_exports.push(PublicSurfaceExport {
                    name: spelling.to_string(),
                    namespace,
                    target,
                    parent_enum,
                    source,
                });
            }
            surface_exports.sort_by(|a, b| {
                (a.name.as_str(), a.namespace, &a.target).cmp(&(
                    b.name.as_str(),
                    b.namespace,
                    &b.target,
                ))
            });
            modules.push(PublicSurfaceModule {
                path,
                modules: child_modules,
                exports: surface_exports,
            });
        }
        modules.sort_by(|a, b| a.path.cmp(&b.path));
        let section = PublicSurfaceSection { package, modules };
        section
            .validate()
            .map_err(|e| self.db.invalid_input(&ModuleGraphQuery, e.to_string()))?;
        Ok(section)
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
                if let Some(identity) = self
                    .db
                    .context()
                    .loader_facts()
                    .compiled_package_module_identity(def_id.module_id)?
                {
                    return Ok(identity.package);
                }
                return Err(self.db.invalid_input(
                    &ModuleGraphQuery,
                    format!("nominal type belongs to an external package; provide an external definition resolver: {def_id:?}"),
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
        self.publish_package_artifact_with_resolver_and_products_and_signatures(
            package, resolver, None, None, None,
        )
    }

    /// Publishes a package artifact and optionally embeds a validated
    /// target/profile-specific native product. Native bytes are supplied by
    /// the driver after code generation; interface publication remains
    /// target-independent and keeps its own invalidation boundary.
    pub fn publish_package_artifact_with_resolver_and_native(
        &self,
        package: PackageId,
        resolver: &dyn StableDefinitionPackageResolver,
        native: Option<nia_package_metadata::NativeSection>,
    ) -> QueryResult<crate::PackageArtifactPublication> {
        self.publish_package_artifact_with_resolver_and_products_and_signatures(
            package, resolver, None, native, None,
        )
    }

    /// Convenience native publication for a package with no external nominal
    /// definition references.
    pub fn publish_package_artifact_with_native(
        &self,
        package: PackageId,
        native: nia_package_metadata::NativeSection,
    ) -> QueryResult<crate::PackageArtifactPublication> {
        let package_for_resolver = package.clone();
        let resolver = |def_id: GlobalDefId| {
            let graph = self.db.get(ModuleGraphQuery)?;
            let Some(entry_root) = graph.current_package_root(graph.entry()) else {
                return Err(self.db.invalid_input(
                    &ModuleGraphQuery,
                    "entry module has no package root".to_string(),
                ));
            };
            if graph.current_package_root(def_id.module_id) != Some(entry_root) {
                if let Some(identity) = self
                    .db
                    .context()
                    .loader_facts()
                    .compiled_package_module_identity(def_id.module_id)?
                {
                    return Ok(identity.package);
                }
                return Err(self.db.invalid_input(
                    &ModuleGraphQuery,
                    "nominal type belongs to an external package".to_string(),
                ));
            }
            Ok(package_for_resolver.clone())
        };
        self.publish_package_artifact_with_resolver_and_native(package, &resolver, Some(native))
    }

    /// Publishes a package artifact with explicitly supplied checked templates
    /// and target-native objects. Products are validated by their canonical
    /// metadata codecs before the container is emitted.
    pub fn publish_package_artifact_with_resolver_and_products(
        &self,
        package: PackageId,
        resolver: &dyn StableDefinitionPackageResolver,
        templates: Option<nia_package_metadata::TemplateSection>,
        native: Option<nia_package_metadata::NativeSection>,
    ) -> QueryResult<crate::PackageArtifactPublication> {
        self.publish_package_artifact_with_resolver_and_products_and_signatures(
            package, resolver, templates, native, None,
        )
    }

    /// Publishes a package artifact with explicit signature, template, and
    /// native products. Signature records are validated against the package's
    /// public interface before encoding.
    pub fn publish_package_artifact_with_resolver_and_products_and_signatures(
        &self,
        package: PackageId,
        resolver: &dyn StableDefinitionPackageResolver,
        templates: Option<nia_package_metadata::TemplateSection>,
        native: Option<nia_package_metadata::NativeSection>,
        signatures: Option<nia_package_metadata::SignatureSection>,
    ) -> QueryResult<crate::PackageArtifactPublication> {
        let (interface, type_graph, indexes) = self
            .package_interface_and_type_graph(package.clone(), resolver)
            .map_err(|error| {
                self.db
                    .invalid_input(&ModuleGraphQuery, format!("interface: {error}"))
            })?;
        let signatures = match signatures {
            Some(signatures) => Some(signatures),
            None => Some(
                self.signature_section_from_interface(
                    package.clone(),
                    resolver,
                    interface.clone(),
                    &indexes,
                )
                .map_err(|error| {
                    self.db
                        .invalid_input(&ModuleGraphQuery, format!("signatures: {error}"))
                })?,
            ),
        };
        let public_surface = self
            .package_public_surface_section_with_resolver(package.clone(), resolver)
            .map_err(|error| {
                self.db
                    .invalid_input(&ModuleGraphQuery, format!("surface: {error}"))
            })?;
        let interface_bytes = nia_package_metadata::encode_interface(&interface)
            .map_err(|error| self.db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
        let graph = self.db.get(ModuleGraphQuery)?;
        let entry_package_root = graph.current_package_root(graph.entry());
        let mut paths = graph
            .modules()
            .filter(|module| {
                entry_package_root.is_none()
                    || graph.current_package_root(module.id) == entry_package_root
            })
            .filter_map(|module| {
                graph
                    .stable_key(module.id)
                    .map(|key| key.source_identity().normalized_path().to_owned())
            })
            .chain(
                interface
                    .records
                    .iter()
                    .map(|record| record.definition.module.path.clone()),
            )
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
        let mut dependencies = self
            .db
            .context()
            .loader_facts()
            .compiled_package_interfaces()?
            .into_iter()
            .filter(|interface| interface.manifest().package != package)
            .map(|interface| {
                let bytes = nia_package_metadata::encode_interface(&InterfaceSection {
                    records: interface.records().to_vec(),
                })
                .map_err(|error| self.db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
                Ok(nia_package_metadata::PackageDependency {
                    package: interface.manifest().package.clone(),
                    interface_hash: nia_package_metadata::section_hash(&bytes),
                })
            })
            .collect::<QueryResult<Vec<_>>>()?;
        // Source-backed dependencies (not yet published as selected
        // compiled interfaces) still need to be represented in the manifest.
        // Public-surface re-exports are the canonical graph evidence for such
        // packages; omitting them makes an otherwise valid artifact fail
        // source-free manifest validation.
        // Public re-exports identify package dependencies. A dependency hash
        // is only meaningful when it comes from that package's canonical
        // interface artifact; never encode an all-zero sentinel that would
        // make an unverified source dependency look like a valid product.
        let mut surface_dependencies = std::collections::BTreeSet::<PackageId>::new();
        for module in &public_surface.modules {
            for export in &module.exports {
                let target_package = export.target.module.package.clone();
                if target_package != package {
                    surface_dependencies.insert(target_package);
                }
                if let Some(parent) = &export.parent_enum
                    && parent.module.package != package
                {
                    surface_dependencies.insert(parent.module.package.clone());
                }
            }
        }
        for dep in surface_dependencies {
            // The dependency hash must describe the dependency's canonical
            // interface, not the consumer's re-export projection. Resolve it
            // from a selected artifact when available; otherwise this package
            // cannot claim a verifiable binary dependency and publication
            // fails rather than emitting a sentinel hash.
            let Some(interface) = self
                .db
                .context()
                .loader_facts()
                .compiled_package_interfaces()?
                .into_iter()
                .find(|interface| interface.manifest().package == dep)
            else {
                return Err(self.db.invalid_input(
                    &ModuleGraphQuery,
                    format!(
                        "public surface references source-backed dependency without compiled interface: {dep:?}"
                    ),
                ));
            };
            let bytes = nia_package_metadata::encode_interface(&InterfaceSection {
                records: interface.records().to_vec(),
            })
            .map_err(|error| self.db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
            dependencies.push(nia_package_metadata::PackageDependency {
                package: dep,
                interface_hash: nia_package_metadata::section_hash(&bytes),
            });
        }
        dependencies.sort_by(|left, right| left.package.cmp(&right.package));
        dependencies.dedup_by(|left, right| left.package == right.package);
        let compilation_target = self.db.context().loader_facts().target();
        let manifest_target = nia_package_metadata::CompilationTarget {
            arch: compilation_target.arch,
            vendor: compilation_target.vendor,
            os: compilation_target.os,
            env: compilation_target.env,
            abi: compilation_target.abi,
            endian: compilation_target.endian,
            pointer_width: compilation_target.pointer_width,
        };
        let manifest_profile = match self.db.context().loader_facts().profile() {
            nia_target_config::BuildProfile::Debug => 0,
            nia_target_config::BuildProfile::Release => 1,
        };
        let manifest_mode = match self.db.context().loader_facts().compilation_mode() {
            nia_target_config::CompilationMode::Normal => 0,
            nia_target_config::CompilationMode::Test => 1,
        };
        let manifest = PackageManifest {
            modules,
            dependencies,
            ..PackageManifest::current(
                package.clone(),
                manifest_target,
                manifest_profile,
                manifest_mode,
            )
        };
        let type_graph_bytes =
            nia_package_metadata::encode_type_graph(&type_graph).map_err(|error| {
                self.db
                    .invalid_input(&ModuleGraphQuery, format!("type graph: {error}"))
            })?;
        let public_surface_bytes = nia_package_metadata::encode_public_surface(&public_surface)
            .map_err(|error| {
                self.db
                    .invalid_input(&ModuleGraphQuery, format!("surface bytes: {error}"))
            })?;
        let native_bytes = native
            .as_ref()
            .map(nia_package_metadata::encode_native)
            .transpose()
            .map_err(|error| self.db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
        let templates = match templates {
            Some(templates) => Some(templates),
            None => Some(
                self.package_template_section_with_resolver(
                    package.clone(),
                    &interface,
                    &indexes,
                    resolver,
                )
                .map_err(|error| {
                    self.db
                        .invalid_input(&ModuleGraphQuery, format!("templates: {error}"))
                })?,
            ),
        };
        let template_bytes = templates
            .as_ref()
            .map(|section| {
                if section.records.iter().any(|record| {
                    record.definition.module.package != package
                        || !interface
                            .records
                            .iter()
                            .any(|item| item.definition == record.definition)
                }) {
                    return Err(nia_package_metadata::MetadataError::InvalidManifest);
                }
                nia_package_metadata::encode_templates(section)
            })
            .transpose()
            .map_err(|error| self.db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
        let signature_bytes = signatures
            .as_ref()
            .map(|section| {
                if section.records.iter().any(|record| {
                    record.definition.module.package != package
                        || record.kind != record.definition.kind
                        || !interface
                            .records
                            .iter()
                            .any(|item| item.definition == record.definition)
                }) {
                    return Err(nia_package_metadata::MetadataError::InvalidManifest);
                }
                section.validate_type_roots(&type_graph)?;
                nia_package_metadata::encode_signatures(section)
            })
            .transpose()
            .map_err(|error| self.db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
        let mut sections = vec![
            (SectionKind::Interface, interface_bytes.as_slice()),
            (SectionKind::TypeGraph, type_graph_bytes.as_slice()),
            (SectionKind::PublicSurface, public_surface_bytes.as_slice()),
        ];
        if let Some(bytes) = native_bytes.as_ref() {
            sections.push((SectionKind::Native, bytes.as_slice()));
        }
        if let Some(bytes) = template_bytes.as_ref() {
            sections.push((SectionKind::Templates, bytes.as_slice()));
        }
        if let Some(bytes) = signature_bytes.as_ref() {
            sections.push((SectionKind::Signatures, bytes.as_slice()));
        }
        let bytes =
            nia_package_metadata::encode_artifact(&manifest, &sections).map_err(|error| {
                self.db
                    .invalid_input(&ModuleGraphQuery, format!("artifact: {error}"))
            })?;
        nia_package_metadata::PackageArtifact::open(bytes.clone()).map_err(|error| {
            self.db
                .invalid_input(&ModuleGraphQuery, format!("artifact open: {error}"))
        })?;
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

    /// Returns the canonical source identities owned by the entry package.
    ///
    /// Artifact-backed dependency modules are excluded even when their
    /// physical paths overlap a local source tree. Drivers use this inventory
    /// to bind package-native objects to the same module ownership snapshot as
    /// semantic publication.
    pub fn current_package_source_identities(
        &self,
    ) -> QueryResult<Vec<nia_source::SourceIdentity>> {
        let graph = self.db.get(ModuleGraphQuery)?;
        let package_root = graph.current_package_root(graph.entry());
        let mut identities = Vec::new();
        for module in graph.modules() {
            if graph.current_package_root(module.id) != package_root
                || self
                    .db
                    .context()
                    .loader_facts()
                    .compiled_package_module_identity(module.id)?
                    .is_some()
            {
                continue;
            }
            if let Some(key) = graph.stable_key(module.id) {
                identities.push(key.source_identity().clone());
            }
        }
        identities.sort();
        identities.dedup();
        Ok(identities)
    }

    /// Returns the canonical source identities owned by the selected runtime
    /// package. Runtime source remains part of the ordinary module graph; this
    /// inventory only gives artifact publication a stable ownership boundary.
    pub fn runtime_source_identities(&self) -> QueryResult<Vec<nia_source::SourceIdentity>> {
        let runtime = self.db.get(CompilerRuntimeQuery)?;
        let Some(runtime) = runtime.source() else {
            return Ok(Vec::new());
        };
        let graph = self.db.get(ModuleGraphQuery)?;
        let root_path = nia_source::SourcePath::with_identity(
            runtime.package_root().to_string_lossy().into_owned(),
            runtime.package_root_identity(),
        );
        let root_identity = nia_source::SourceIdentity::from_path(&root_path);
        let Some(runtime_root) = graph.module_id_for_source_identity(&root_identity) else {
            return Err(self.db.invalid_input(
                &ModuleGraphQuery,
                "selected runtime package root is absent from the module graph".to_string(),
            ));
        };
        let mut identities = graph
            .modules()
            .filter(|module| graph.current_package_root(module.id) == Some(runtime_root))
            .filter_map(|module| graph.stable_key(module.id))
            .map(|key| key.source_identity().clone())
            .collect::<Vec<_>>();
        identities.sort();
        identities.dedup();
        Ok(identities)
    }

    /// Resolves a source codegen identity to the canonical compiled-package
    /// module that supplied it. Local and runtime source modules return none.
    pub fn compiled_package_module_for_source_identity(
        &self,
        identity: &nia_source::SourceIdentity,
    ) -> QueryResult<Option<nia_package_metadata::ModuleId>> {
        let graph = self.db.get(ModuleGraphQuery)?;
        let Some(module_id) = graph.module_id_for_source_identity(identity) else {
            return Ok(None);
        };
        self.db
            .context()
            .loader_facts()
            .compiled_package_module_identity(module_id)
    }

    /// Resolves a definition to its canonical package while publishing the
    /// supplied package as the owner of the current source root.
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
        if let Some(identity) = self
            .db
            .context()
            .loader_facts()
            .compiled_package_module_identity(def_id.module_id)?
        {
            return Ok(identity.package);
        }
        if graph.current_package_root(def_id.module_id)
            == graph.package_root(&nia_symbol::known::RUNTIME)
            && let RuntimeSpec::Source(runtime) = self.db.get(CompilerRuntimeQuery)?.as_ref()
        {
            return Ok(runtime.package().clone());
        }
        // Any remaining source root in a package-publication graph is owned by
        // the package being published. External package roots are represented
        // by validated compiled-module identities and were handled above.
        return Ok(current_package.clone());
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
        let new_compiled_native_fingerprint = compiled_native_observation_fingerprint(
            request.loader_facts.compiled_package_interfaces()?,
        )?;
        let compiled_interfaces_changed = {
            let observed = self
                .db
                .context()
                .observed_compiled_interfaces
                .lock()
                .expect("compiler compiled-interface observation lock poisoned");
            *observed != Some(new_compiled_interface_fingerprint)
        };
        let compiled_native_changed = {
            let observed = self
                .db
                .context()
                .observed_compiled_native
                .lock()
                .expect("compiler native observation lock poisoned");
            *observed != Some(new_compiled_native_fingerprint)
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
        let (optimization_changed, codegen_scope_changed) = {
            let mut inputs = self.inputs.write().expect("compiler input lock poisoned");
            let optimization_changed = inputs.optimization != new_inputs.optimization;
            let codegen_scope_changed = inputs.codegen_scope != new_inputs.codegen_scope;
            *inputs = new_inputs;
            (optimization_changed, codegen_scope_changed)
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
        if compiled_native_changed {
            invalidation.extend(self.db.invalidate(CompiledPackageNativeObservationQuery));
            *self
                .db
                .context()
                .observed_compiled_native
                .lock()
                .expect("compiler native observation lock poisoned") =
                Some(new_compiled_native_fingerprint);
        }
        let inputs_invalidation =
            self.invalidate_inputs(optimization_changed, codegen_scope_changed)?;
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

    fn invalidate_inputs(
        &self,
        optimization_changed: bool,
        codegen_scope_changed: bool,
    ) -> QueryResult<CompilerInvalidation> {
        let mut invalidation = CompilerInvalidation::default();
        let provider_worklist = self.db.context().provider_fact_worklist()?;
        invalidation.extend(
            self.db
                .validate_input(ProviderFactWorklistQuery, &provider_worklist),
        );
        if optimization_changed {
            invalidation.extend(self.db.invalidate(CompilerOptimizationQuery));
        }
        if codegen_scope_changed {
            invalidation.extend(self.db.invalidate(CompilerCodegenScopeQuery));
        }
        Ok(invalidation)
    }
}

fn native_optimization_tag(database: &CompilerDatabase) -> u8 {
    match database.current_optimization().level {
        NiaOptimizationLevel::O0 => 0,
        NiaOptimizationLevel::O1 => 1,
        NiaOptimizationLevel::O2 => 2,
        NiaOptimizationLevel::O3 => 3,
        NiaOptimizationLevel::Os => 4,
        NiaOptimizationLevel::Oz => 5,
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
                            "array length const expression is not evaluated in the published package",
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

fn signature_owner_requires_child(owner: nia_defs::DefKind, child: nia_defs::DefKind) -> bool {
    use nia_defs::DefKind;
    matches!(
        (owner, child),
        (DefKind::Struct, DefKind::StructField)
            | (DefKind::Union, DefKind::UnionField)
            | (DefKind::Enum, DefKind::EnumVariant)
            | (DefKind::EnumVariant, DefKind::EnumVariantField)
            | (
                DefKind::Trait,
                DefKind::TraitAssociatedType | DefKind::TraitMethod
            )
    )
}

fn stable_trait_definition(
    trait_id: &nia_package_metadata::StableTraitId,
) -> Option<nia_package_metadata::DefinitionId> {
    match trait_id {
        nia_package_metadata::StableTraitId::Source(definition) => Some(definition.clone()),
        nia_package_metadata::StableTraitId::Builtin(_) => None,
    }
}

fn stable_type_graph_definitions(graph: &StableTypeGraph) -> Vec<DefinitionId> {
    let mut definitions = Vec::new();
    for node in &graph.nodes {
        match node {
            nia_package_metadata::StableTypeNode::Named(definition)
            | nia_package_metadata::StableTypeNode::NamedApplied { definition, .. } => {
                definitions.push(definition.clone());
            }
            nia_package_metadata::StableTypeNode::TraitObject {
                trait_id,
                associated_type_bindings,
                ..
            }
            | nia_package_metadata::StableTypeNode::TraitObjectPointee {
                trait_id,
                associated_type_bindings,
                ..
            } => {
                definitions.extend(stable_trait_definition(trait_id));
                definitions.extend(
                    associated_type_bindings
                        .iter()
                        .filter_map(|binding| stable_trait_definition(binding.trait_id.as_ref()?)),
                );
            }
            nia_package_metadata::StableTypeNode::Projection { trait_id, .. } => {
                definitions.extend(stable_trait_definition(trait_id));
            }
            nia_package_metadata::StableTypeNode::ClosureState { owner, .. } => {
                definitions.push(owner.clone());
            }
            _ => {}
        }
    }
    definitions
}

fn signature_flags_for_definition(
    def_id: DefId,
    kind: u8,
    signatures: &nia_item_signatures::ItemSignatures,
) -> u32 {
    match kind {
        2 | 12 => signatures
            .functions
            .get(&def_id)
            .map_or(0, function_signature_flags),
        11 => signatures
            .traits
            .values()
            .flat_map(|signature| &signature.methods)
            .find(|method| method.def_id == def_id)
            .map_or(0, |method| function_signature_flags(&method.signature)),
        5 => signatures.structs.get(&def_id).map_or(0, |signature| {
            let mut flags = 0;
            if signature.is_extern {
                flags |= nia_package_metadata::SIGNATURE_FLAG_EXTERN;
            }
            if signature.is_tuple {
                flags |= nia_package_metadata::SIGNATURE_FLAG_TUPLE;
            }
            flags
        }),
        7 => signatures.unions.get(&def_id).map_or(0, |signature| {
            u32::from(signature.is_extern) * nia_package_metadata::SIGNATURE_FLAG_EXTERN
        }),
        13 => signatures.enums.get(&def_id).map_or(0, |signature| {
            u32::from(signature.is_open) * nia_package_metadata::SIGNATURE_FLAG_OPEN
        }),
        3 => signatures.globals.get(&def_id).map_or(0, |signature| {
            let mut flags = 0;
            if signature.is_extern {
                flags |= nia_package_metadata::SIGNATURE_FLAG_EXTERN;
            }
            if signature.is_mutable {
                flags |= nia_package_metadata::SIGNATURE_FLAG_MUTABLE;
            }
            flags
        }),
        4 => nia_package_metadata::SIGNATURE_FLAG_CONST,
        _ => 0,
    }
}

fn function_signature_flags(signature: &nia_item_signatures::FunctionSignature) -> u32 {
    let mut flags = 0;
    if signature.has_body {
        flags |= nia_package_metadata::SIGNATURE_FLAG_HAS_BODY;
    }
    if signature.is_extern {
        flags |= nia_package_metadata::SIGNATURE_FLAG_EXTERN;
    }
    if signature.is_const {
        flags |= nia_package_metadata::SIGNATURE_FLAG_CONST;
    }
    if signature.is_variadic {
        flags |= nia_package_metadata::SIGNATURE_FLAG_VARIADIC;
    }
    flags
}

fn signature_payload_for_definition(
    def_id: DefId,
    module_id: ModuleId,
    kind: u8,
    signatures: &nia_item_signatures::ItemSignatures,
    stable_by_global: &HashMap<GlobalDefId, DefinitionId>,
    indexes: &HashMap<InternedTyId, u32>,
    symbols: &dyn nia_symbol::SymbolText,
    db: &QueryDb<CompilerContext>,
) -> QueryResult<Option<nia_package_metadata::SignaturePayload>> {
    let root = |ty: InternedTyId| {
        indexes.get(&ty).copied().ok_or_else(|| {
            db.invalid_input(
                &ModuleGraphQuery,
                "signature payload type missing from published graph",
            )
        })
    };
    let field = |field: &nia_item_signatures::FieldSignature| {
        let definition = stable_by_global
            .get(&GlobalDefId {
                module_id,
                def_id: field.def_id,
            })
            .cloned()
            .ok_or_else(|| {
                db.invalid_input(
                    &ModuleGraphQuery,
                    "signature payload field is missing from interface identities",
                )
            })?;
        Ok(nia_package_metadata::SignatureField {
            name: symbols
                .symbol_text(field.name)
                .ok_or_else(|| db.invalid_input(&ModuleGraphQuery, "field has no stable name"))?
                .to_string(),
            type_root: root(field.ty)?,
            definition,
        })
    };
    let has_definition =
        |def_id: DefId| stable_by_global.contains_key(&GlobalDefId { module_id, def_id });
    let payload =
        match kind {
            2 | 11 | 12 => signatures.functions.get(&def_id).map(|signature| {
                let params = signature
                    .params
                    .iter()
                    .map(|param| {
                        Ok(nia_package_metadata::SignatureParameter {
                            name: param.name.and_then(|name| {
                                symbols.symbol_text(name).as_deref().map(str::to_owned)
                            }),
                            receiver: match param.receiver {
                                None => 0,
                                Some(nia_ids::ReceiverKind::RefReadOnly) => 1,
                                Some(nia_ids::ReceiverKind::Ref) => 2,
                                Some(nia_ids::ReceiverKind::Value) => 3,
                            },
                            type_root: root(param.ty)?,
                        })
                    })
                    .collect::<QueryResult<Vec<_>>>()?;
                let attributes = signature
                    .attributes
                    .iter()
                    .map(|attribute| -> QueryResult<u32> {
                        match attribute {
                            nia_item_signatures::FunctionAttribute::Naked => Ok(1),
                            nia_item_signatures::FunctionAttribute::TrackCaller => Ok(2),
                            nia_item_signatures::FunctionAttribute::Builtin(builtin) => {
                                Ok(0x100 | builtin.stable_tag())
                            }
                        }
                    })
                    .collect::<QueryResult<Vec<_>>>()?;
                Ok(nia_package_metadata::SignaturePayload::Function {
                    params,
                    return_type: root(signature.return_type)?,
                    attributes,
                })
            }),
            5 => signatures.structs.get(&def_id).and_then(|signature| {
                signature
                    .fields
                    .iter()
                    .all(|field| has_definition(field.def_id))
                    .then(|| {
                        Ok(nia_package_metadata::SignaturePayload::Aggregate {
                            fields: signature
                                .fields
                                .iter()
                                .map(field)
                                .collect::<QueryResult<Vec<_>>>()?,
                        })
                    })
            }),
            7 => signatures.unions.get(&def_id).and_then(|signature| {
                signature
                    .fields
                    .iter()
                    .all(|field| has_definition(field.def_id))
                    .then(|| {
                        Ok(nia_package_metadata::SignaturePayload::Aggregate {
                            fields: signature
                                .fields
                                .iter()
                                .map(field)
                                .collect::<QueryResult<Vec<_>>>()?,
                        })
                    })
            }),
            13 => {
                signatures.enums.get(&def_id).and_then(|signature| {
                    signature
                        .variants
                        .iter()
                        .all(|variant| {
                            has_definition(variant.def_id)
                                && match &variant.payload {
                                    nia_item_signatures::EnumVariantPayloadSignature::Named(
                                        fields,
                                    ) => fields.iter().all(|field| has_definition(field.def_id)),
                                    _ => true,
                                }
                        })
                        .then(|| {
                            let variants =
                                signature
                                    .variants
                                    .iter()
                                    .map(|variant| {
                                        let definition = stable_by_global
                                            .get(&GlobalDefId {
                                                module_id,
                                                def_id: variant.def_id,
                                            })
                                            .cloned()
                                            .ok_or_else(|| {
                                                db.invalid_input(
                                &ModuleGraphQuery,
                                "enum variant is missing from interface identities",
                            )
                                            })?;
                                        let payload = match &variant.payload {
                        nia_item_signatures::EnumVariantPayloadSignature::Unit => {
                            nia_package_metadata::SignatureVariantPayload::Unit
                        }
                        nia_item_signatures::EnumVariantPayloadSignature::Tuple(types) => {
                            nia_package_metadata::SignatureVariantPayload::Tuple(
                                types.iter().map(|ty| root(*ty)).collect::<QueryResult<Vec<_>>>()?,
                            )
                        }
                        nia_item_signatures::EnumVariantPayloadSignature::Named(fields) => {
                            nia_package_metadata::SignatureVariantPayload::Named(
                                fields.iter().map(field).collect::<QueryResult<Vec<_>>>()?,
                            )
                        }
                    };
                                        Ok(nia_package_metadata::SignatureVariant {
                                            definition,
                                            name: symbols
                                                .symbol_text(variant.name)
                                                .ok_or_else(|| {
                                                    db.invalid_input(
                                                        &ModuleGraphQuery,
                                                        "variant has no stable name",
                                                    )
                                                })?
                                                .to_string(),
                                            payload,
                                        })
                                    })
                                    .collect::<QueryResult<Vec<_>>>()?;
                            Ok(nia_package_metadata::SignaturePayload::Enum {
                                backing_type: root(signature.backing_type)?,
                                variants,
                            })
                        })
                })
            }
            16 => signatures.type_aliases.get(&def_id).map(|signature| {
                Ok(nia_package_metadata::SignaturePayload::TypeAlias {
                    target: root(signature.target)?,
                })
            }),
            3 => signatures.globals.get(&def_id).map(|signature| {
                Ok(nia_package_metadata::SignaturePayload::Value {
                    explicit_type: signature.explicit_type.map(root).transpose()?,
                    builtin: None,
                })
            }),
            4 => signatures
                .consts
                .get(&def_id)
                .map(|signature| {
                    Ok(nia_package_metadata::SignaturePayload::Value {
                        explicit_type: signature.explicit_type.map(root).transpose()?,
                        builtin: signature
                            .builtin
                            .map(nia_ids::BuiltinConstValue::stable_tag),
                    })
                })
                .or_else(|| {
                    signatures.traits.values().find_map(|signature| {
                        signature
                            .associated_values
                            .iter()
                            .find(|value| value.def_id == def_id)
                            .map(|value| {
                                Ok(nia_package_metadata::SignaturePayload::Value {
                                    explicit_type: Some(root(value.ty)?),
                                    builtin: None,
                                })
                            })
                    })
                }),
            _ => None,
        };
    payload.transpose()
}

fn signature_generic_and_where_facts(
    def_id: DefId,
    signatures: &nia_item_signatures::ItemSignatures,
    defs: &nia_defs::DefCollection,
    symbols: &dyn nia_symbol::SymbolText,
    indexes: &HashMap<InternedTyId, u32>,
) -> QueryResult<(
    Vec<nia_package_metadata::SignatureGenericParam>,
    Vec<nia_package_metadata::SignatureWherePredicate>,
)> {
    let (params, predicates) = if let Some(signature) = signatures.functions.get(&def_id) {
        let params =
            providers::codegen::effective_function_generic_params(signatures, defs, def_id);
        let mut predicates = Vec::new();
        if let Some(parent) = defs.defs.get(def_id).and_then(|def| def.parent)
            && let Some(parent) = signatures.traits.get(&parent)
        {
            predicates.extend(parent.where_predicates.iter().cloned());
        }
        if let Some(extension) = signatures.trait_impls.iter().find(|extension| {
            extension
                .methods
                .iter()
                .any(|method| method.def_id == def_id)
        }) {
            predicates.extend(extension.where_predicates.iter().cloned());
        }
        predicates.extend(signature.where_predicates.iter().cloned());
        (params, predicates)
    } else if let Some(signature) = signatures.structs.get(&def_id) {
        (
            signature.generic_params.clone(),
            signature.where_predicates.clone(),
        )
    } else if let Some(signature) = signatures.unions.get(&def_id) {
        (
            signature.generic_params.clone(),
            signature.where_predicates.clone(),
        )
    } else if let Some(signature) = signatures.traits.get(&def_id) {
        (
            signature.generic_params.clone(),
            signature.where_predicates.clone(),
        )
    } else if let Some(signature) = signatures.type_aliases.get(&def_id) {
        (signature.generic_params.clone(), Vec::new())
    } else {
        (Vec::new(), Vec::new())
    };
    Ok((
        signature_generic_params_to_wire(def_id, &params, symbols, indexes)?,
        signature_where_predicates_to_wire(def_id, &predicates, symbols, indexes)?,
    ))
}

fn signature_generic_params_to_wire(
    def_id: DefId,
    params: &[nia_item_signatures::GenericParamSignature],
    symbols: &dyn nia_symbol::SymbolText,
    indexes: &HashMap<InternedTyId, u32>,
) -> QueryResult<Vec<nia_package_metadata::SignatureGenericParam>> {
    params
        .iter()
        .map(|param| {
            let name = symbols
                .symbol_text(param.name)
                .ok_or_else(|| {
                    signature_publication_error(
                        def_id,
                        "generic parameter has no stable symbol text",
                    )
                })?
                .to_string();
            let type_root = match param.kind {
                nia_item_signatures::GenericParamSignatureKind::Type => None,
                nia_item_signatures::GenericParamSignatureKind::Const { ty } => {
                    Some(*indexes.get(&ty).ok_or_else(|| {
                        signature_publication_error(
                            def_id,
                            "generic parameter type missing from published graph",
                        )
                    })?)
                }
            };
            Ok(nia_package_metadata::SignatureGenericParam {
                name,
                kind: u8::from(matches!(
                    param.kind,
                    nia_item_signatures::GenericParamSignatureKind::Const { .. }
                )),
                type_root,
            })
        })
        .collect()
}

fn signature_where_predicates_to_wire(
    def_id: DefId,
    predicates: &[nia_defs::WherePredicateSignature],
    symbols: &dyn nia_symbol::SymbolText,
    indexes: &HashMap<InternedTyId, u32>,
) -> QueryResult<Vec<nia_package_metadata::SignatureWherePredicate>> {
    predicates
        .iter()
        .map(|predicate| {
            Ok(nia_package_metadata::SignatureWherePredicate {
                type_root: *indexes.get(&predicate.ty).ok_or_else(|| {
                    signature_publication_error(
                        def_id,
                        "where predicate type missing from published graph",
                    )
                })?,
                bounds: predicate
                    .bounds
                    .iter()
                    .map(|bound| signature_where_bound_to_wire(def_id, bound, symbols, indexes))
                    .collect::<QueryResult<Vec<_>>>()?,
            })
        })
        .collect()
}

fn signature_where_bound_to_wire(
    def_id: DefId,
    bound: &nia_defs::WhereBoundSignature,
    symbols: &dyn nia_symbol::SymbolText,
    indexes: &HashMap<InternedTyId, u32>,
) -> QueryResult<nia_package_metadata::SignatureWhereBound> {
    Ok(nia_package_metadata::SignatureWhereBound {
        trait_root: *indexes.get(&bound.trait_ty).ok_or_else(|| {
            signature_publication_error(def_id, "where bound trait missing from published graph")
        })?,
        associated_type_bindings: bound
            .associated_type_bindings
            .iter()
            .map(|binding| {
                Ok(nia_package_metadata::SignatureAssociatedTypeBinding {
                    name: symbols
                        .symbol_text(binding.name)
                        .ok_or_else(|| {
                            signature_publication_error(
                                def_id,
                                "associated type has no stable symbol text",
                            )
                        })?
                        .to_string(),
                    type_root: *indexes.get(&binding.ty).ok_or_else(|| {
                        signature_publication_error(
                            def_id,
                            "associated type binding missing from published graph",
                        )
                    })?,
                })
            })
            .collect::<QueryResult<Vec<_>>>()?,
    })
}

fn signature_supertrait_to_wire(
    def_id: DefId,
    supertrait: &nia_item_signatures::TraitSupertraitSignature,
    symbols: &dyn nia_symbol::SymbolText,
    indexes: &HashMap<InternedTyId, u32>,
) -> QueryResult<nia_package_metadata::SignatureWhereBound> {
    Ok(nia_package_metadata::SignatureWhereBound {
        trait_root: *indexes.get(&supertrait.ty).ok_or_else(|| {
            signature_publication_error(def_id, "supertrait type missing from published graph")
        })?,
        associated_type_bindings: supertrait
            .associated_type_bindings
            .iter()
            .map(|binding| {
                Ok(nia_package_metadata::SignatureAssociatedTypeBinding {
                    name: symbols
                        .symbol_text(binding.name)
                        .ok_or_else(|| {
                            signature_publication_error(
                                def_id,
                                "supertrait associated type has no stable symbol text",
                            )
                        })?
                        .to_string(),
                    type_root: *indexes.get(&binding.ty).ok_or_else(|| {
                        signature_publication_error(
                            def_id,
                            "supertrait associated type missing from published graph",
                        )
                    })?,
                })
            })
            .collect::<QueryResult<Vec<_>>>()?,
    })
}

fn signature_publication_error(def_id: DefId, message: &str) -> QueryError {
    QueryError::InvalidInput {
        query: QueryFrame {
            name: "signature_publication",
            key: format!("{def_id:?}"),
            description: "signature_publication".into(),
        },
        message: message.into(),
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
    let observed_compiled_native = compiled_native_observation_fingerprint(
        loader_facts
            .compiled_package_interfaces()
            .expect("initial compiled package interfaces"),
    )
    .expect("initial compiled native observation fingerprint");
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
            observed_compiled_native: std::sync::Mutex::new(Some(observed_compiled_native)),
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
        builder.write_u64(interface.manifest().dependencies.len() as u64);
        for dependency in &interface.manifest().dependencies {
            builder.write_str(&dependency.package.namespace);
            builder.write_str(&dependency.package.name);
            builder.write_str(&dependency.package.version);
            builder.write_bytes(&dependency.interface_hash);
        }
        builder.write_u64(interface.manifest().modules.len() as u64);
        for module in &interface.manifest().modules {
            builder.write_str(&module.path);
            builder.write_bytes(&module.interface_hash);
        }
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
            let bytes = nia_package_metadata::encode_templates(templates).ok()?;
            builder.write_bytes(&bytes);
        } else {
            builder.write_u8(0);
        }
        if let Some(surface) = interface.public_surface() {
            builder.write_u8(1);
            let bytes = nia_package_metadata::encode_public_surface(surface).ok()?;
            builder.write_bytes(&bytes);
        } else {
            builder.write_u8(0);
        }
        if let Some(signatures) = interface.signatures() {
            builder.write_u8(1);
            let bytes = nia_package_metadata::encode_signatures(signatures).ok()?;
            builder.write_bytes(&bytes);
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

fn compiled_native_observation_fingerprint(
    interfaces: Vec<nia_package_metadata::CompiledPackageInterface>,
) -> QueryResult<QueryFingerprint> {
    let mut builder = QueryFingerprintBuilder::new(FingerprintDomain::new(
        "nia.compiler.compiled-native-observation.v1",
    ));
    for interface in interfaces {
        let package = &interface.manifest().package;
        builder.write_str(&package.namespace);
        builder.write_str(&package.name);
        builder.write_str(&package.version);
        if let Some(native) = interface.native() {
            let bytes = nia_package_metadata::encode_native(native).map_err(|error| {
                QueryError::InvalidInput {
                    query: QueryFrame {
                        name: "compiled_package_native_observation",
                        key: "compiled_package_native_observation".into(),
                        description: "compiled_package_native_observation".into(),
                    },
                    message: error.to_string(),
                }
            })?;
            builder.write_bytes(&bytes);
        }
    }
    Ok(builder.finish())
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
