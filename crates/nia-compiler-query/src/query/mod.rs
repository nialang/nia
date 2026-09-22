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
use parking_lot::{Mutex, RwLock};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

/// Public compiler-facing name for the canonical metadata module identity.
pub type StableModuleIdentity = StableModuleId;

mod backend_lowering;
mod base;
mod checked;
mod checks;
mod context;
mod database;
mod diagnostics;
mod executable;
mod extension_provider_queries;
mod fingerprints;
mod frontend_cache_publication;
mod function_body_queries;
mod identity;
mod invalidation;
mod program;
mod program_signature_queries;
mod providers;
mod registry;
mod request;
mod settlement;

mod resolve;
mod stable_type_graph;
mod static_init_queries;
mod types;

use backend_lowering::*;
use base::*;
use checked::*;
use checks::*;
use context::*;
pub use database::*;
use diagnostics::*;
use executable::*;
use extension_provider_queries::*;
use fingerprints::*;
use frontend_cache_publication::*;
use function_body_queries::*;
pub use identity::*;
use program::*;
use program_signature_queries::*;
use providers::*;
use registry::*;
pub use request::*;
use resolve::*;
use stable_type_graph::*;
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

type ExtensionSignatureModuleInputValue = ExtensionSignatureModuleInputQueryValue;
type ExtensionTraitSolvingModuleFactsValue = ExtensionTraitSolvingModuleFactsQueryValue;
type ExtensionTraitImplsForTraitValue = ExtensionTraitImplsForTraitQueryValue;
type ModuleProgramSignatureFactsValue = ModuleProgramSignatureFacts;
type ModuleAbiSignatureFactsValue = ModuleAbiSignatureFactsQueryValue;
type PublicSurfacesValue = PublicSurfacesQueryValue;
type PublicUsingScopesValue = PublicUsingScopesQueryValue;

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

    fn rehydrated_type(&self, types: &[InternedTyId], index: u32) -> QueryResult<InternedTyId> {
        types.get(index as usize).copied().ok_or_else(|| {
            self.db.invalid_input(
                &ModuleGraphQuery,
                format!("stable type index {index} is outside the decoded graph"),
            )
        })
    }

    fn rehydrated_types(
        &self,
        types: &[InternedTyId],
        indexes: &[u32],
    ) -> QueryResult<Vec<InternedTyId>> {
        indexes
            .iter()
            .map(|index| self.rehydrated_type(types, *index))
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
                    trait_args: self.rehydrated_types(types, &binding.trait_arguments)?,
                    trait_const_args: self
                        .rehydrate_stable_const_args(&binding.trait_const_arguments, types)?,
                    name: SymbolId::from_stable_hash(binding.name),
                    ty: self.rehydrated_type(types, binding.ty)?,
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

    /// Returns the session-owned canonical type store for observability.
    pub fn type_store(&self) -> std::sync::Arc<nia_ty::TypeStore> {
        std::sync::Arc::clone(&self.db.context().type_store)
    }

    /// Returns the current loader-owned module graph snapshot.
    ///
    /// This is intentionally a read-only observability boundary. The loader
    /// remains the sole owner of graph mutation and provider-demand updates.
    pub fn module_graph(&self) -> QueryResult<ModuleGraphSnapshot> {
        self.current_graph()
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
        self.rehydrated_types(&types, &graph.roots)
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
                    args: self.rehydrated_types(&types, arguments)?,
                    const_args: self.rehydrate_stable_const_args(const_arguments, &types)?,
                }),
                StableTypeNode::Unit => append.intern(nia_ty::TyKind::Tuple(Vec::new())),
                StableTypeNode::Never => {
                    append.intern(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::Never))
                }
                StableTypeNode::Tuple(elements) => append.intern(nia_ty::TyKind::Tuple(
                    self.rehydrated_types(&types, elements)?,
                )),
                StableTypeNode::Array { element, length } => append.intern(nia_ty::TyKind::Array {
                    elem: self.rehydrated_type(&types, *element)?,
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
                        params: self.rehydrated_types(&types, parameters)?,
                        return_type: self.rehydrated_type(&types, *result)?,
                        is_variadic: false,
                    })
                }
                StableTypeNode::Reference { target, mutable } => {
                    append.intern(nia_ty::TyKind::Pointer {
                        is_readonly: !*mutable,
                        elem: self.rehydrated_type(&types, *target)?,
                    })
                }
                StableTypeNode::Pointer { target, readonly } => {
                    append.intern(nia_ty::TyKind::Pointer {
                        is_readonly: *readonly,
                        elem: self.rehydrated_type(&types, *target)?,
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
                        elem: self.rehydrated_type(&types, *target)?,
                    })
                }
                StableTypeNode::Slice { target, readonly } => {
                    append.intern(nia_ty::TyKind::Slice {
                        is_readonly: *readonly,
                        elem: self.rehydrated_type(&types, *target)?,
                    })
                }
                StableTypeNode::SlicePointee { target } => {
                    append.intern(nia_ty::TyKind::SlicePointee {
                        elem: self.rehydrated_type(&types, *target)?,
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
                    bound: bound
                        .map(|index| self.rehydrated_type(&types, index))
                        .transpose()?,
                }),
                StableTypeNode::Optional { element } => append.intern(nia_ty::TyKind::Optional {
                    elem: self.rehydrated_type(&types, *element)?,
                }),
                StableTypeNode::ErrorUnion { error, value } => {
                    append.intern(nia_ty::TyKind::ErrorUnion {
                        error: self.rehydrated_type(&types, *error)?,
                        value: self.rehydrated_type(&types, *value)?,
                    })
                }
                StableTypeNode::Callable {
                    parameters,
                    result,
                    readonly,
                } => append.intern(nia_ty::TyKind::Callable {
                    is_readonly: *readonly,
                    params: self.rehydrated_types(&types, parameters)?,
                    return_type: self.rehydrated_type(&types, *result)?,
                }),
                StableTypeNode::CallablePointee { parameters, result } => {
                    append.intern(nia_ty::TyKind::CallablePointee {
                        params: self.rehydrated_types(&types, parameters)?,
                        return_type: self.rehydrated_type(&types, *result)?,
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
                    args: self.rehydrated_types(&types, arguments)?,
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
                    trait_args: self.rehydrated_types(&types, trait_arguments)?,
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
                    trait_args: self.rehydrated_types(&types, trait_arguments)?,
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
                    self_ty: self.rehydrated_type(&types, *self_ty)?,
                    trait_id: self.rehydrate_stable_trait_id(trait_id, resolver)?,
                    trait_args: self.rehydrated_types(&types, trait_arguments)?,
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
                    captures: self.rehydrated_types(&types, captures)?,
                    params: self.rehydrated_types(&types, parameters)?,
                    return_type: self.rehydrated_type(&types, *result)?,
                }),
            }?;
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
        )?;
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
        ) -> QueryResult<R>,
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
        settlement::settle_provider_worklist(
            self,
            discover_executable_providers,
            compile,
            provider_demands,
        )
    }

    fn refresh_frontend_program_sources_snapshot(&self) -> QueryResult<()> {
        if self.db.context().signature_cache.is_none() {
            return Ok(());
        }
        if let Some(program_sources) = self.db.get(FrontendProgramSourcesQuery)?.as_ref().as_ref() {
            self.db
                .context()
                .observe_frontend_program_sources(program_sources);
        }
        Ok(())
    }

    fn update_provider_demands_with_telemetry(
        &self,
        round: u64,
        phase: &'static str,
        demands: Vec<crate::ProviderDemand>,
    ) -> QueryResult<crate::ProviderGraphUpdate> {
        let timings = self.db.context().timings();
        if !timings.enabled() {
            return self
                .db
                .context()
                .loader_facts()
                .update_provider_demands(demands);
        }
        let before = self.db.context().loader_facts().provider_facts()?;
        let unique_demands = demands.iter().cloned().collect::<HashSet<_>>();
        let known = unique_demands.intersection(before.demands()).count() as u64;
        let update = self
            .db
            .context()
            .loader_facts()
            .update_provider_demands(demands)?;
        let after = self.db.context().loader_facts().provider_facts()?;
        let added = after
            .demands()
            .difference(before.demands())
            .collect::<Vec<_>>();
        emit_provider_demand_update(
            timings,
            round,
            phase,
            unique_demands.len() as u64,
            known,
            &added,
        );
        Ok(update)
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

    /// Returns a snapshot of query execution and reuse counters.
    pub fn query_trace(&self) -> QueryResult<QueryTrace> {
        self.db.query_trace()
    }

    fn current_graph(&self) -> QueryResult<ModuleGraphSnapshot> {
        self.db.context().loader_facts.module_graph()
    }

    fn current_optimization(&self) -> OptimizationPolicy {
        self.inputs.read().optimization
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

fn emit_provider_demand_update(
    timings: TimingMode,
    round: u64,
    phase: &str,
    unique: u64,
    known: u64,
    added: &[&crate::ProviderDemand],
) {
    if !timings.enabled() {
        return;
    }
    let prefix = format!("compiler.executable_provider_demands.round_{round}.{phase}");
    nia_timing::emit_counter(format!("{prefix}.unique"), unique);
    nia_timing::emit_counter(format!("{prefix}.known"), known);
    nia_timing::emit_counter(format!("{prefix}.new"), added.len() as u64);
    let mut methods = 0_u64;
    let mut trait_impls = 0_u64;
    let mut module_semantics = 0_u64;
    let mut module_bodies = 0_u64;
    for demand in added {
        match demand.request {
            crate::ProviderRequest::Method { .. } => methods += 1,
            crate::ProviderRequest::TraitImpl { .. } => trait_impls += 1,
            crate::ProviderRequest::ModuleSemantic { .. } => module_semantics += 1,
            crate::ProviderRequest::ModuleBody { .. } => module_bodies += 1,
        }
    }
    nia_timing::emit_counter(format!("{prefix}.new_methods"), methods);
    nia_timing::emit_counter(format!("{prefix}.new_trait_impls"), trait_impls);
    nia_timing::emit_counter(format!("{prefix}.new_module_semantics"), module_semantics);
    nia_timing::emit_counter(format!("{prefix}.new_module_bodies"), module_bodies);
}

fn emit_check_certificate_reuse(timings: TimingMode, hit: bool) {
    if !timings.enabled() {
        return;
    }
    nia_timing::emit_counter("compiler.check_certificate_hits", u64::from(hit));
    nia_timing::emit_counter("compiler.check_certificate_misses", u64::from(!hit));
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

    fn symbols(&self) -> nia_symbol_table::SymbolTable {
        self.loader_facts().symbols()
    }

    fn provider_fact_worklist(&self) -> QueryResult<crate::ProviderFactSnapshot> {
        self.loader_facts().provider_facts()
    }

    fn optimization(&self) -> OptimizationPolicy {
        self.inputs.read().optimization
    }

    fn codegen_scope(&self) -> crate::CodegenScope {
        self.inputs.read().codegen_scope
    }

    fn timings(&self) -> TimingMode {
        self.inputs.read().timings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RuntimeSpec;
    use nia_sema_ir::SemanticValueUse;
    use nia_source::{SourceId, SourceIdentity, SourceRevision};

    trait TestTypeStoreAppend {
        fn test_intern(&self, kind: nia_ty::TyKind) -> nia_ids::InternedTyId;
        fn test_primitive(&self, primitive: nia_ty::PrimitiveTy) -> nia_ids::InternedTyId;
    }

    impl TestTypeStoreAppend for nia_ty::TypeStoreAppend {
        fn test_intern(&self, kind: nia_ty::TyKind) -> nia_ids::InternedTyId {
            self.intern(kind).expect("intern compiler-query test type")
        }

        fn test_primitive(&self, primitive: nia_ty::PrimitiveTy) -> nia_ids::InternedTyId {
            self.primitive(primitive)
                .expect("intern primitive compiler-query test type")
        }
    }

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
