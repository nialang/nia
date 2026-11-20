// SPDX-License-Identifier: GPL-3.0-or-later
#[cfg(test)]
use crate::{ActiveModuleItemTreeFactKind, LoadedModule, LoadedProgram};
use crate::{
    CheckedModule, CheckedProgram, CheckedProgramAnalysis, CodegenPreparation, CodegenProgram,
    FrontendCheckInputFingerprint, FrontendCheckScope, ProgramDiagnostic, ProgramDiagnosticBundles,
    RuntimeSpec, TimingMode, module_diagnostics,
};
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
#[cfg(test)]
use nia_node_id::NodeOriginTable;
use nia_opt::{NiaOptimizationLevel, OptimizationPolicy};
use nia_package_metadata::{
    DefinitionId, ModuleId as StableModuleId, PackageId, StableArrayLength,
    StableAssociatedTypeBinding, StableClosureOwnerInstance, StableConstArg, StableConstValue,
    StableTraitId, StableTypeGraph, StableTypeNode,
};
use nia_program_signatures::{
    ExtensionMethodIndexModuleInput, ExtensionMethodValidationInput, ExtensionModuleInput,
    ExtensionTraitSignatureIndex, ModuleProgramSignatureFacts, ModuleSignatureInput,
    VisibleExtensionsForModule, VisibleExtensionsInput, VisibleTraitImplsForModule,
    collect_extension_associated_value_index_for_module,
    collect_extension_method_diagnostics_for_module, collect_extension_method_index_for_module,
    visible_extensions_for_module, visible_trait_impls_for_module,
};
pub use nia_public_surface::LexicalUsingScopes;

/// Source locations and member bindings for a module's trait implementations.
#[derive(Default)]
pub struct SourceTraitImplementations {
    /// Implemented trait and the source span naming it in each implementation.
    pub traits: Vec<(GlobalDefId, Span)>,
    /// Local member definition and the trait member it implements.
    pub members: Vec<(DefId, GlobalDefId)>,
}

/// Source metadata for an applicable receiver method.
pub struct SourceReceiverMethod {
    /// Method spelling.
    pub name: SymbolId,
    /// Physical source and declaration span when the receiver identifies one member.
    pub declaration: Option<(SourcePath, Span)>,
}

/// Compiler-checked receiver and its applicable methods at a source location.
pub struct SourceReceiverMembers {
    /// Type inferred at the receiver expression.
    pub ty: InternedTyId,
    /// Applicable methods with declaration locations where resolvable.
    pub methods: Vec<SourceReceiverMethod>,
}
use nia_public_surface::{
    TypeExposureIndex, compute_exported_public_surfaces_with_symbols,
    compute_using_scopes_from_surfaces_with_symbols,
};
use nia_query::{
    CancellationToken, FingerprintDomain, QueryDb, QueryError, QueryFingerprint,
    QueryFingerprintBuilder, QueryFingerprintPolicy, QueryFrame, QueryKey, QueryProviderPolicy,
    QueryResult, QueryStoragePolicy, QueryTrace,
};
use nia_source::{SourcePath, SourceVersion};
use nia_span::Span;
use nia_symbol::SymbolId;
#[cfg(test)]
use nia_syntax::ParseError;
use nia_target::TargetConfig;
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
mod lexical_usings;
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
use lexical_usings::*;
use program::*;
use program_signature_queries::*;
use providers::*;
use registry::*;
pub use request::*;
use resolve::*;
use settlement::*;
use stable_type_graph::*;
use static_init_queries::*;
use types::*;

type ExtensionProviderModuleFactsValue = ExtensionProviderModuleFactsQueryValue;
type ExtensionProviderValidationFactsValue = ExtensionProviderValidationFactsQueryValue;
type ExtensionProviderDiscoveryIndexValue = ExtensionProviderDiscoveryIndexQueryValue;
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

    /// Returns source spans for the semantic node identities in a loaded module.
    pub fn source_origins(&self, module: ModuleId) -> QueryResult<nia_node_id::NodeOriginTable> {
        self.db.context().module_origins(&self.db, module)
    }

    /// Returns the ranges excluded by the current target, profile, and compilation mode.
    pub fn source_inactive_spans(&self, module: ModuleId) -> QueryResult<Arc<HashSet<Span>>> {
        let items = self.db.get(FullActiveModuleItemTreeQuery(module))?;
        Ok(Arc::clone(&items.inactive_spans))
    }

    /// Returns the effective import scope at a source position.
    pub fn source_imports(
        &self,
        module: ModuleId,
        position: usize,
    ) -> QueryResult<ModuleUsingScope> {
        Ok(self
            .source_import_scopes(module)?
            .effective_scope(Span::new(position, position))
            .clone())
    }

    /// Returns module and block import scopes from one source snapshot.
    pub fn source_import_scopes(&self, module: ModuleId) -> QueryResult<LexicalUsingScopes> {
        let errors = self.db.get(ModuleParseErrorsQuery(module))?;
        if !errors.is_empty() {
            let current = self.source_definitions(module)?;
            let mut defs = shared_public_surface_defs_by_module(&self.db)?;
            defs.push((*current).clone());
            let graph = self.current_graph()?;
            let symbols = self.db.context().symbols();
            let surfaces = compute_exported_public_surfaces_with_symbols(&defs, &graph, &symbols);
            let mut resolved = compute_using_scopes_from_surfaces_with_symbols(
                &defs,
                &graph,
                &surfaces.surfaces,
                &symbols,
            );
            let scope = resolved.using_scopes.remove(&module).unwrap_or_default();
            let items = self.db.get(FullActiveModuleItemTreeQuery(module))?;
            let (lexical, _) = nia_public_surface::compute_lexical_using_scopes_with_symbols(
                &items,
                &current,
                &defs,
                &graph,
                &surfaces.surfaces,
                &scope,
                &symbols,
            );
            return Ok(lexical);
        }
        let lexical = self.db.get(ModuleLexicalUsingScopesQuery(module))?;
        Ok(lexical.scopes.clone())
    }

    /// Returns the exported namespaces of a module.
    pub fn source_exports(
        &self,
        module: ModuleId,
    ) -> QueryResult<Option<Arc<ModulePublicSurface>>> {
        Ok(self
            .db
            .get(ModulePublicSurfaceQuery(module))?
            .as_ref()
            .clone())
    }

    /// Returns the spelling of a name in the current session.
    pub fn source_symbol_text(&self, name: SymbolId) -> Option<Arc<str>> {
        self.db.context().symbols().resolve(name)
    }

    /// Returns the active declarations, including those in a module with body errors.
    pub fn source_definitions(&self, module: ModuleId) -> QueryResult<Arc<DefCollection>> {
        full_module_defs_semantic(&self.db, module)
    }

    /// Lists inherent members available through a nominal type's qualified path.
    pub fn source_associated_members(
        &self,
        module: ModuleId,
        owner: GlobalDefId,
    ) -> QueryResult<Vec<(SymbolId, GlobalDefId)>> {
        let extensions = self.db.get(VisibleExtensionsQuery(module))?;
        let types = self.type_store();
        let mut members = Vec::new();
        for target in extensions.methods.targets() {
            if !matches!(types.get(target.target_ty), Some(TyKind::Nominal { def_id, .. }) if *def_id == owner)
            {
                continue;
            }
            members.extend(
                target
                    .methods
                    .iter()
                    .filter(|method| method.trait_id.is_none() && method.is_callable)
                    .map(|method| (method.name, method.def_id)),
            );
            members.extend(
                target
                    .associated_values
                    .iter()
                    .filter(|value| value.is_accessible)
                    .map(|value| (value.name, value.def_id)),
            );
        }
        // The compiler also resolves inherent methods declared by the type's
        // own module when only the type, or an alias for it, was imported.
        if owner.module_id != module {
            let owner_extensions = self.db.get(VisibleExtensionsQuery(owner.module_id))?;
            let defs = self.source_definitions(owner.module_id)?;
            let graph = self.current_graph()?;
            for target in owner_extensions.methods.targets().iter().filter(|target| {
                matches!(types.get(target.target_ty), Some(TyKind::Nominal { def_id, .. }) if *def_id == owner)
            }) {
                members.extend(target.methods.iter().filter_map(|method| {
                    if method.trait_id.is_some() || method.def_id.module_id != owner.module_id {
                        return None;
                    }
                    let def = defs.defs.get(method.def_id.def_id)?;
                    nia_imports::visibility_allows(def.visibility, &graph, owner.module_id, module)
                        .then_some((method.name, method.def_id))
                }));
            }
            members.sort_unstable();
            members.dedup();
        }
        Ok(members)
    }

    /// Resolves a module's implementations to their source traits and members.
    pub fn source_trait_implementations(
        &self,
        module: &CheckedModule,
    ) -> QueryResult<SourceTraitImplementations> {
        let signatures = self.db.get(ItemSignaturesQuery(module.id))?;
        let types = self.type_store();
        let mut bindings = SourceTraitImplementations::default();
        for implementation in &signatures.semantic.trait_impls {
            let Some(trait_ty) = implementation.trait_ty else {
                continue;
            };
            let Some(TyKind::Nominal {
                def_id: trait_id, ..
            }) = types.get(module.type_normalization.normalize(trait_ty))
            else {
                continue;
            };
            bindings.traits.push((
                *trait_id,
                implementation.trait_span.unwrap_or(implementation.span),
            ));
            let defs = self.source_definitions(trait_id.module_id)?;
            let Some(members) = defs.scopes.struct_members.get(&trait_id.def_id) else {
                continue;
            };
            bindings
                .members
                .extend(implementation.methods.iter().filter_map(|method| {
                    Some((
                        method.def_id,
                        GlobalDefId {
                            module_id: trait_id.module_id,
                            def_id: members.methods.get(&method.name)?,
                        },
                    ))
                }));
            bindings
                .members
                .extend(implementation.associated_values.iter().filter_map(|value| {
                    Some((
                        value.def_id,
                        GlobalDefId {
                            module_id: trait_id.module_id,
                            def_id: members.values.get(&value.name)?,
                        },
                    ))
                }));
            bindings
                .members
                .extend(
                    implementation
                        .associated_types
                        .iter()
                        .filter_map(|associated| {
                            let (local, _) = module.defs.defs.iter().find(|(_, def)| {
                                def.kind == nia_defs::DefKind::TraitAssociatedType
                                    && def.name == associated.name
                                    && def.span == associated.span
                            })?;
                            Some((
                                local,
                                GlobalDefId {
                                    module_id: trait_id.module_id,
                                    def_id: members.fields.get(&associated.name)?,
                                },
                            ))
                        }),
                );
        }
        Ok(bindings)
    }

    /// Resolves a receiver type and its methods in an isolated editor database.
    pub fn source_receiver_members(
        &self,
        module: ModuleId,
        receiver: nia_span::Span,
        names: &[SymbolId],
        cancel: Option<&CancellationToken>,
    ) -> QueryResult<Option<SourceReceiverMembers>> {
        let (receiver, _) = settlement::settle_provider_worklist(
            self,
            false,
            |database| {
                editor_receiver_methods(
                    &database.db,
                    module,
                    receiver,
                    nia_body_check::EditorMemberLookup::Receiver(names),
                )
            },
            |(_, demands)| demands.clone(),
            cancel,
        )?;
        let Some(receiver) = receiver else {
            return Ok(None);
        };
        let graph = self.current_graph()?;
        let methods = receiver
            .methods
            .into_iter()
            .map(|candidate| {
                if let Some(cancel) = cancel {
                    cancel.check()?;
                }
                let declaration = match candidate.declaration {
                    Some(id) => {
                        let definitions = self.source_definitions(id.module_id)?;
                        graph
                            .get(id.module_id)
                            .zip(definitions.defs.get(id.def_id))
                            .map(|(module, def)| (module.path.clone(), def.span))
                    }
                    None => None,
                };
                Ok(SourceReceiverMethod {
                    name: candidate.name,
                    declaration,
                })
            })
            .collect::<QueryResult<_>>()?;
        Ok(Some(SourceReceiverMembers {
            ty: receiver.ty,
            methods,
        }))
    }

    /// Returns the enum inferred at an omitted variant expression or pattern.
    pub fn source_omitted_enum(
        &self,
        module: ModuleId,
        member: Span,
        cancel: Option<&CancellationToken>,
    ) -> QueryResult<Option<GlobalDefId>> {
        let (result, _) = settlement::settle_provider_worklist(
            self,
            false,
            |database| {
                editor_receiver_methods(
                    &database.db,
                    module,
                    member,
                    nia_body_check::EditorMemberLookup::OmittedVariant,
                )
            },
            |(_, demands)| demands.clone(),
            cancel,
        )?;
        Ok(
            result.and_then(|result| match self.type_store().get(result.ty) {
                Some(TyKind::Nominal { def_id, .. }) => Some(*def_id),
                _ => None,
            }),
        )
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
        self.analyze_entry_program_with_cancellation(None)
    }

    /// Checks an entry and observes cancellation at provider settlement boundaries.
    pub fn analyze_entry_program_with_cancellation(
        &self,
        cancel: Option<&CancellationToken>,
    ) -> QueryResult<CheckedProgramAnalysis> {
        settlement::settle_provider_worklist(
            self,
            true,
            Self::entry_check_program_once,
            checked_provider_demands,
            cancel,
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
            let graph = self.current_graph()?;
            return Ok(CheckedProgram {
                diagnostic_sources: diagnostic_sources(&self.db, &graph, &certificate.diagnostics)?,
                graph,
                optimization: self.current_optimization(),
                diagnostics: certificate.diagnostics.clone(),
                suppressed_downstream: certificate.suppressed_downstream,
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
            suppressed_downstream: report.suppressed_downstream,
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
            None,
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
        let context = self.db.context();
        let loader = context.loader_facts();
        let before = if timings.enabled() {
            let facts = loader.provider_facts()?;
            let unique = demands.iter().collect::<HashSet<_>>();
            let known = unique
                .iter()
                .filter(|d| facts.demands().contains(**d))
                .count();
            Some((facts, unique.len(), known))
        } else {
            None
        };
        let update = loader.update_provider_demands(demands)?;
        if matches!(update, crate::ProviderGraphUpdate::Changed { .. }) {
            // The next edit may remove a module first loaded during this round.
            let graph = loader.module_graph()?;
            *context.observed_graph.lock() = graph;
        }
        if let Some((before, unique, known)) = before {
            let after = loader.provider_facts()?;
            let added = after
                .demands()
                .difference(before.demands())
                .collect::<Vec<_>>();
            emit_provider_demand_update(timings, round, phase, unique as u64, known as u64, &added);
        }
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
            return Ok(PackageId::toolchain("std"));
        }
        if graph.current_package_root(def_id.module_id) == graph.runtime_package_root()
            && let RuntimeSpec::Source(runtime) = self.db.get(CompilerRuntimeQuery)?.as_ref()
        {
            return Ok(runtime.package().id());
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

pub(crate) fn query_error_diagnostic(err: QueryError) -> Diagnostic {
    match err {
        QueryError::Canceled => {
            Diagnostic::internal_error(codes::QUERY_ENGINE, err.to_string()).finish()
        }
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
            || const_args.iter().any(|arg| {
                let mut missing = false;
                arg.visit_referenced_types(|ty| missing |= type_store.get(ty).is_none());
                missing
            })
        {
            return false;
        }
        self_arg == instance.self_arg && args == instance.args && const_args == instance.const_args
    }
}
