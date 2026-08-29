// SPDX-License-Identifier: GPL-3.0-or-later
//! Program-wide planning and module-owned backend IR lowering.
//!
//! Planning discovers reachable templates and concrete instances before module
//! finalization. Cross-module references are routed back to a unique owner;
//! finalization may run per module, but collection restores source-module order.

mod closure_entries;
mod function_instances;
mod indexes;
mod input;
mod instantiate;
mod instantiation_context;
mod items;
mod layout_extender;
mod lowerer_helpers;
mod lowerer_materialize;
mod lowerer_membership;
mod lowerer_setup;
mod module_const_prop;
mod module_dce;
mod module_devirt;
mod module_inline;
mod operator_dispatch;
mod opt;
mod struct_instances;
mod trait_context;
mod trait_object_vtables;
mod type_context;

pub(crate) use indexes::*;

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Deref;
use std::sync::Arc;

use nia_ast::{BindingItem, Block, Expr, StmtKind, Visibility, generic_param_names};
use nia_backend_ir::{
    BackendClosureEntry, BackendClosureEntryOwner, BackendFunction, BackendFunctionInstance,
    BackendGlobal, BackendGlobalInstance, BackendGlobalInstanceKey, BackendLayouts, BackendModule,
    BackendModuleReadiness, BackendModuleStore, BackendProgram, BackendStruct,
    BackendStructInstance, BackendStructInstanceKey, BackendTraitObjectVtable,
    BackendTraitObjectVtableFunction, BackendTraitObjectVtableKey, BackendUnion,
    BackendUnionInstance,
};
use nia_defs::{DefCollection, DefId, DefKind, ExtensionMethods, VisibleExtensionMethods};
use nia_diagnostic::{Diagnostic, codes};
use nia_function_ir::{
    FunctionBody, FunctionBodyRefs, FunctionInstanceKey, FunctionInstanceRef, GlobalInstanceKey,
    GlobalInstanceRef,
};
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId, TraitId};
use nia_item_signatures::{
    ItemSignatures, ProgramEnumSignature, ProgramFunctionSignature, ProgramStructSignature,
    ProgramTraitImplIndex, ProgramTraitImplSignature, ProgramTraitSignature, ProgramUnionSignature,
    WherePredicateSignature,
};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNodeKind};
use nia_layout::{Layouts, StructLayoutKey};
use nia_local_resolve::LocalResolution;
use nia_mangle::{MangleModuleId, MangleResolvers, mangle_instance_symbol_id, mangle_symbol_id};
use nia_node_id::VersionedNodeKey;
use nia_opt::{InlineThreshold, OptimizationDepth, OptimizationPolicy};
use nia_sema_ir::SemanticFacts;
use nia_symbol::{SymbolId, SymbolText, known, stable_hash, symbol_text_or_unresolved};
use nia_ty::{ArrayLenTy, ConstGenericArg, ConstGenericValue, TyKind, TypeEquivalence};
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_value_resolve::ValueResolution;

#[derive(Debug, PartialEq)]
/// Complete validated input to backend code generation.
///
/// `program`, `owner_directory`, and `codegen_partitions` are one inseparable
/// snapshot: every emitted definition has exactly one module owner, and the
/// partition plan was derived from that exact program. Diagnostics may be
/// non-empty; callers must not pass such a lowering to codegen.
pub struct BackendLowering {
    /// Final module-owned backend IR.
    pub program: BackendProgram,
    /// Canonical owner of every definition and generated backend item.
    pub owner_directory: Arc<nia_backend_ir::BackendModuleOwnerDirectory>,
    /// Deterministic codegen partitioning derived from `program`.
    pub codegen_partitions: nia_backend_ir::CodegenPartitionPlan,
    /// Optimization policy used while producing the program.
    pub optimization: OptimizationPolicy,
    /// Enabled passes and transformations that changed IR.
    pub optimization_report: BackendOptimizationReport,
    /// Internal diagnostics that make this lowering unsuitable for codegen.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, PartialEq)]
/// Program-wide item discovery result before module-local finalization.
///
/// Planning resolves cross-module ownership and materializes the initial item
/// fixed point. [`BackendItemPlan::into_module_plans`] then separates immutable
/// program metadata from independently finalizable module plans.
pub struct BackendItemPlan {
    modules: Vec<BackendModuleItemPlan>,
    optimization: OptimizationPolicy,
    optimization_report: BackendOptimizationReport,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, PartialEq)]
/// One module's planned backend items, ready for module-local finalization.
pub struct BackendModuleItemPlan {
    module: BackendModule,
}

#[derive(Debug, PartialEq)]
/// Program-wide state retained while module plans are finalized independently.
pub struct BackendItemPlanFinalization {
    optimization: OptimizationPolicy,
    optimization_report: BackendOptimizationReport,
    diagnostics: Vec<Diagnostic>,
    owner_directory: Arc<nia_backend_ir::BackendModuleOwnerDirectory>,
}

/// Shared read-only context for independently finalizing module item plans.
///
/// The context rebuilds program indexes once and may be shared by parallel
/// module tasks. Each task must pair an input and plan with the same module
/// owner and source-order position.
pub struct BackendProgramFinalizationContext<S = std::sync::Arc<nia_ty::TypeStore>> {
    type_store: S,
    optimization: OptimizationPolicy,
    shared: BackendLowerShared,
    timing: bool,
}

#[derive(Debug, PartialEq)]
/// Finalized module plus source-order metadata and module-owned reports.
pub struct BackendModuleFinalization {
    position: usize,
    module: BackendModule,
    optimization_report: BackendOptimizationReport,
    diagnostics: Vec<Diagnostic>,
}

/// Publishes finalized modules while restoring deterministic program order.
///
/// Module tasks may complete in any order. The collector publishes each module
/// to [`BackendModuleStore`] immediately for downstream readiness consumers,
/// but retains diagnostics and optimization changes by original source-module
/// position so observable output is deterministic.
pub struct BackendModuleFinalizationCollector {
    finalization: BackendItemPlanFinalization,
    module_order: Vec<ModuleId>,
    modules: Arc<BackendModuleStore>,
    optimization_reports: Vec<Option<BackendOptimizationReport>>,
    diagnostics: Vec<Option<Vec<Diagnostic>>>,
}

impl<S> BackendProgramFinalizationContext<S>
where
    S: Deref<Target = nia_ty::TypeStore>,
{
    /// Builds the shared indexes used by every module finalization task.
    pub fn new(
        modules: &[BackendLowerModuleInput<'_>],
        type_store: S,
        optimization: OptimizationPolicy,
        timings: nia_timing::TimingMode,
    ) -> Self {
        let timing = timings.detail();
        let shared = time_backend_stage(timing, "backend_lower.final_shared_indexes", || {
            BackendLowerShared::new(modules)
        });
        Self {
            type_store,
            optimization,
            shared,
            timing,
        }
    }

    /// Finalizes one module plan without mutating any other module's plan.
    ///
    /// `position` is opaque source-order metadata for the collector. `input`
    /// and `module_plan` must have the same module owner.
    pub fn finalize_module(
        &self,
        position: usize,
        input: &BackendLowerModuleInput<'_>,
        module_plan: BackendModuleItemPlan,
    ) -> BackendModuleFinalization {
        assert_eq!(
            input.module_id, module_plan.module.id,
            "Nia ICE: backend module plan owner must match finalization input"
        );
        let mut lowerer = ModuleLowerer::new(
            input,
            &self.type_store,
            self.optimization,
            &self.shared,
            self.timing,
        );
        let mut module = module_plan.module;
        lowerer.finish_module(&mut module);
        BackendModuleFinalization {
            position,
            module,
            optimization_report: lowerer.optimization_report,
            diagnostics: lowerer.diagnostics,
        }
    }
}

impl BackendModuleItemPlan {
    /// Returns the planned module for inspection before consuming the plan.
    pub fn module(&self) -> &BackendModule {
        &self.module
    }
}

impl BackendModuleFinalizationCollector {
    /// Creates a collector for the exact source-module order of the plan.
    pub fn new(finalization: BackendItemPlanFinalization, module_order: &[ModuleId]) -> Self {
        let module_count = module_order.len();
        Self {
            finalization,
            module_order: module_order.to_vec(),
            modules: Arc::new(BackendModuleStore::new(module_order.iter().copied())),
            optimization_reports: (0..module_count).map(|_| None).collect(),
            diagnostics: (0..module_count).map(|_| None).collect(),
        }
    }

    /// Returns the live store into which finalized modules are published.
    pub fn module_store(&self) -> Arc<BackendModuleStore> {
        Arc::clone(&self.modules)
    }

    /// Returns the owner directory fixed during program-wide planning.
    pub fn owner_directory(&self) -> Arc<nia_backend_ir::BackendModuleOwnerDirectory> {
        Arc::clone(&self.finalization.owner_directory)
    }

    /// Takes the store's readiness stream for incremental codegen.
    ///
    /// The stream is unique; taking it more than once is rejected by the store.
    pub fn take_readiness(&self) -> BackendModuleReadiness {
        self.modules.take_readiness()
    }

    /// Publishes one completed module task at its original source position.
    ///
    /// Each position must be pushed exactly once and must match both the task's
    /// recorded position and the module owner in `module_order`.
    pub fn push(&mut self, position: usize, module_finalization: BackendModuleFinalization) {
        assert_eq!(
            module_finalization.position, position,
            "Nia ICE: backend module finalization completion position must match its task"
        );
        let expected_module = self.module_order.get(position).unwrap_or_else(|| {
            panic!("Nia ICE: backend module finalization position is out of bounds")
        });
        assert_eq!(
            module_finalization.module.id, *expected_module,
            "Nia ICE: backend module finalization owner must match module order"
        );
        // Publication can arrive in completion order, while reports and diagnostics remain keyed
        // by source-module position so parallel finalization cannot perturb observable ordering.
        self.finalization
            .owner_directory
            .validate_finalized_module(&module_finalization.module);
        self.modules.publish(module_finalization.module);
        self.optimization_reports[position] = Some(module_finalization.optimization_report);
        self.diagnostics[position] = Some(module_finalization.diagnostics);
    }

    /// Joins all module results into one complete backend lowering.
    ///
    /// Every source position must have been pushed. Reports and diagnostics are
    /// joined in source order even when publication happened in completion
    /// order.
    pub fn finish(self) -> BackendLowering {
        let BackendItemPlanFinalization {
            optimization,
            mut optimization_report,
            mut diagnostics,
            owner_directory,
        } = self.finalization;
        for (position, (report, module_diagnostics)) in self
            .optimization_reports
            .into_iter()
            .zip(self.diagnostics)
            .enumerate()
        {
            let report = report.unwrap_or_else(|| {
                panic!("Nia ICE: backend module finalization report {position} was not collected")
            });
            let module_diagnostics = module_diagnostics.unwrap_or_else(|| {
                panic!(
                    "Nia ICE: backend module finalization diagnostics {position} were not collected"
                )
            });
            optimization_report
                .changed_passes
                .extend(report.changed_passes);
            diagnostics.extend(module_diagnostics);
        }
        let program = BackendProgram::from_module_store(self.modules);
        let codegen_partitions = program.codegen_partition_plan();
        BackendLowering {
            program,
            owner_directory,
            codegen_partitions,
            optimization,
            optimization_report,
            diagnostics,
        }
    }
}

impl BackendItemPlan {
    /// Creates an empty plan representing failure before item planning began.
    pub fn from_diagnostics(
        optimization: OptimizationPolicy,
        diagnostics: Vec<Diagnostic>,
    ) -> Self {
        Self {
            modules: Vec::new(),
            optimization,
            optimization_report: BackendOptimizationReport {
                enabled_module_passes: enabled_module_passes(&optimization),
                enabled_function_passes: opt::enabled_function_passes(&optimization),
                enabled_global_passes: enabled_global_passes(&optimization),
                changed_passes: Vec::new(),
            },
            diagnostics,
        }
    }

    /// Returns module plans in the same order as the lowering inputs.
    pub fn modules(&self) -> &[BackendModuleItemPlan] {
        &self.modules
    }

    /// Splits program-wide state from module-local work.
    ///
    /// The returned module vector preserves input order. Its plans may be
    /// finalized concurrently, then joined with a
    /// [`BackendModuleFinalizationCollector`].
    pub fn into_module_plans(self) -> (BackendItemPlanFinalization, Vec<BackendModuleItemPlan>) {
        let owner_directory = Arc::new(nia_backend_ir::BackendModuleOwnerDirectory::from_modules(
            self.modules.iter().map(BackendModuleItemPlan::module),
        ));
        (
            BackendItemPlanFinalization {
                optimization: self.optimization,
                optimization_report: self.optimization_report,
                diagnostics: self.diagnostics,
                owner_directory,
            },
            self.modules,
        )
    }

    /// Returns the optimization policy captured by planning.
    pub fn optimization(&self) -> OptimizationPolicy {
        self.optimization
    }

    /// Returns planning-time optimization evidence.
    pub fn optimization_report(&self) -> &BackendOptimizationReport {
        &self.optimization_report
    }

    /// Returns diagnostics accumulated before module finalization.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Requested concrete function instance discovered by an earlier compiler phase.
pub struct BackendFunctionInstancePlan {
    /// Generic function or method definition to instantiate.
    pub def_id: GlobalDefId,
    /// Module in whose type context the arguments were resolved.
    pub arg_module_id: ModuleId,
    /// Concrete `Self` argument for method instances.
    pub self_arg: Option<InternedTyId>,
    /// Concrete type arguments in effective generic parameter order.
    pub args: Vec<InternedTyId>,
    /// Concrete const arguments in effective const-generic parameter order.
    pub const_args: Vec<nia_ty::ConstGenericArg>,
    /// Use site that caused the instance to become reachable.
    pub span: nia_span::Span,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
/// Audit trail for backend optimization policy and observed transformations.
pub struct BackendOptimizationReport {
    /// Module pass names enabled by policy, whether or not they changed IR.
    pub enabled_module_passes: Vec<&'static str>,
    /// Function pass names enabled by policy, whether or not they changed IR.
    pub enabled_function_passes: Vec<&'static str>,
    /// Global/static pass names enabled by policy, whether or not they changed IR.
    pub enabled_global_passes: Vec<&'static str>,
    /// Concrete owner and pass for every transformation that changed IR.
    pub changed_passes: Vec<BackendOptimizationChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// One backend owner changed by an optimization pass.
pub enum BackendOptimizationChange {
    /// A source function or concrete function instance changed.
    Function {
        /// Module that owns the emitted function.
        module_id: ModuleId,
        /// Source definition behind the emitted function.
        function: GlobalDefId,
        /// Stable pass name.
        pass: &'static str,
        /// Whether the changed function is a monomorphized instance.
        is_instance: bool,
        /// Number of concrete type arguments for an instance.
        type_arg_count: usize,
    },
    /// A global or static initializer changed.
    Global {
        /// Module that owns the emitted global.
        module_id: ModuleId,
        /// Source definition of the global.
        global: GlobalDefId,
        /// Stable pass name.
        pass: &'static str,
    },
}

/// Program-wide semantic and IR facts required by module-owned lowering.
///
/// Implementations must expose one coherent compiler snapshot. Ids returned by
/// list methods must resolve through their corresponding lookup method, and
/// normalized types and signatures must belong to the same [`nia_ty::TypeStore`]
/// supplied to lowering.
pub trait BackendProgramFacts: Sync {
    /// Returns source identities for every module that may own generated items.
    fn source_identities(&self) -> &HashMap<ModuleId, nia_source::SourceIdentity>;
    /// Returns evaluated array lengths owned by `module_id`.
    fn const_array_lengths(&self, module_id: ModuleId) -> Option<&HashMap<GlobalConstExprId, u64>>;
    /// Returns all source definitions with available checked function bodies.
    fn function_body_ids(&self) -> &[GlobalDefId];
    /// Looks up the checked function IR for a source definition.
    fn function_body(&self, def_id: GlobalDefId) -> Option<&FunctionBody>;
    /// Returns closure entry bodies owned by a source function.
    fn closure_entries(&self, _def_id: GlobalDefId) -> &[nia_function_ir::FunctionClosureEntry] {
        &[]
    }
    /// Returns all definitions with available checked static initializers.
    fn static_init_ids(&self) -> &[GlobalDefId];
    /// Looks up the checked static initializer for a global definition.
    fn static_init(&self, def_id: GlobalDefId) -> Option<&nia_static_ir::StaticInit>;
    /// Returns program-wide extension-method ownership.
    fn extension_methods(&self) -> &ExtensionMethods;
    /// Returns extension methods visible from `module_id`.
    fn extensions(&self, module_id: ModuleId) -> Option<&VisibleExtensionMethods>;
    /// Returns definitions owned by `module_id`.
    fn defs(&self, module_id: ModuleId) -> Option<&DefCollection>;
    /// Returns the canonical normalized form of a program type when available.
    fn normalized_type(&self, ty: InternedTyId) -> Option<InternedTyId>;
    /// Normalizes a type using aliases and projections visible from `module_id`.
    fn normalized_type_from_module(
        &self,
        module_id: ModuleId,
        ty: InternedTyId,
    ) -> Option<InternedTyId>;
    /// Returns program-wide function signatures keyed by source definition.
    fn functions(&self) -> &HashMap<GlobalDefId, ProgramFunctionSignature>;
    /// Returns program-wide struct signatures keyed by source definition.
    fn structs(&self) -> &HashMap<GlobalDefId, ProgramStructSignature>;
    /// Returns program-wide union signatures keyed by source definition.
    fn unions(&self) -> &HashMap<GlobalDefId, ProgramUnionSignature>;
    /// Returns program-wide enum signatures keyed by source definition.
    fn enums(&self) -> &HashMap<GlobalDefId, ProgramEnumSignature>;
    /// Returns program-wide trait signatures keyed by source definition.
    fn traits(&self) -> &HashMap<GlobalDefId, ProgramTraitSignature>;
    /// Returns program-wide type-alias signatures keyed by source definition.
    fn type_aliases(&self)
    -> &HashMap<GlobalDefId, nia_item_signatures::ProgramTypeAliasSignature>;
    /// Returns all program trait implementation signatures.
    fn trait_impls(&self) -> &[ProgramTraitImplSignature];
    /// Returns the canonical lookup index for trait implementations.
    fn trait_impl_index(&self) -> &ProgramTraitImplIndex;
}

#[derive(Clone)]
/// Complete checked input for lowering one source module.
///
/// All borrowed products must belong to the same module revision. Program-wide
/// reachability slices, when present, must be sorted because membership checks
/// use binary search.
pub struct BackendLowerModuleInput<'a> {
    /// Stable module identity shared by all local products.
    pub module_id: ModuleId,
    /// Path-independent source identity used for deterministic ownership.
    pub source_identity: nia_source::SourceIdentity,
    /// Human-readable module name used in emitted backend metadata.
    pub module_name: String,
    /// Symbol text resolver for names and diagnostics.
    pub symbols: &'a (dyn SymbolText + Sync),
    /// Active syntax items after configuration filtering.
    pub active_item_tree: &'a ActiveModuleItemTree,
    /// Definition ownership for the module revision.
    pub defs: &'a DefCollection,
    /// Resolved value paths.
    pub values: &'a ValueResolution,
    /// Resolved local bindings and parameter identities.
    pub locals: &'a LocalResolution,
    /// Lowered source types and const expressions.
    pub type_lowering: &'a TypeLowering,
    /// Checked item signatures for the module.
    pub signatures: &'a ItemSignatures,
    /// Canonical normalized types for this module.
    pub type_normalization: &'a TypeNormalization,
    /// Body-check types, calls, and other semantic facts.
    pub semantic_facts: &'a SemanticFacts,
    /// Extension methods visible from this module.
    pub extensions: &'a VisibleExtensionMethods,
    /// Evaluated array lengths owned by this module.
    pub const_array_lengths: &'a HashMap<GlobalConstExprId, u64>,
    /// Evaluated enum discriminants owned by this module.
    pub const_enum_values: &'a HashMap<DefId, nia_const_check::ConstValue>,
    /// Layouts computed for the checked type snapshot.
    pub layouts: &'a Layouts,
    /// Policy for choosing initial function roots.
    pub roots: BackendFunctionRoots,
    /// Sorted executable function reachability, when available.
    pub reachable_functions: Option<&'a [GlobalDefId]>,
    /// Sorted executable global reachability, when available.
    pub reachable_globals: Option<&'a [GlobalDefId]>,
    /// Sorted executable struct reachability, when available.
    pub reachable_structs: Option<&'a [GlobalDefId]>,
    /// Sorted executable union reachability, when available.
    pub reachable_unions: Option<&'a [GlobalDefId]>,
    /// Concrete function instances requested before backend discovery.
    pub function_instance_plan: &'a [BackendFunctionInstancePlan],
    /// Coherent program-wide facts used for cross-module materialization.
    pub program: &'a dyn BackendProgramFacts,
}

impl std::fmt::Debug for BackendLowerModuleInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackendLowerModuleInput")
            .field("module_id", &self.module_id)
            .field("module_name", &self.module_name)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
/// Policy for selecting the initial function set before reachability expansion.
pub enum BackendFunctionRoots {
    /// Lower externs, entry symbols, and non-private source functions.
    #[default]
    Public,
    /// Lower externs and functions present in executable reachability facts.
    EntryPoints,
    /// Lower every available non-generic checked body plus extern declarations.
    FunctionBodies,
    /// Do not select source functions as initial roots.
    NoFunctions,
}

/// Plans and finalizes a backend program with timing disabled.
pub fn lower_backend_program(
    modules: &[BackendLowerModuleInput<'_>],
    type_store: &nia_ty::TypeStore,
    optimization: OptimizationPolicy,
) -> BackendLowering {
    lower_backend_program_with_timings(
        modules,
        type_store,
        optimization,
        nia_timing::TimingMode::Off,
    )
}

/// Plans and finalizes a backend program in one sequential convenience call.
///
/// Use [`plan_backend_program_with_timings`] and the public finalization types
/// when module finalization and LLVM readiness should overlap.
pub fn lower_backend_program_with_timings(
    modules: &[BackendLowerModuleInput<'_>],
    type_store: &nia_ty::TypeStore,
    optimization: OptimizationPolicy,
    timings: nia_timing::TimingMode,
) -> BackendLowering {
    let plan = plan_backend_program_with_timings(modules, type_store, optimization, timings);
    let (finalization, module_plans) = plan.into_module_plans();
    finalize_backend_module_item_plans_with_timings(
        modules,
        type_store,
        finalization,
        module_plans,
        timings,
    )
}

/// Discovers program-wide backend items with timing disabled.
pub fn plan_backend_program(
    modules: &[BackendLowerModuleInput<'_>],
    type_store: &nia_ty::TypeStore,
    optimization: OptimizationPolicy,
) -> BackendItemPlan {
    plan_backend_program_with_timings(
        modules,
        type_store,
        optimization,
        nia_timing::TimingMode::Off,
    )
}

/// Discovers reachable items, resolves unique owners, and creates module plans.
///
/// This phase is program-wide because function instances, aggregate instances,
/// vtables, and referenced foreign items can be discovered from several source
/// modules. Equal generated definitions receive one deterministic owner before
/// module plans are allowed to finalize independently.
pub fn plan_backend_program_with_timings(
    modules: &[BackendLowerModuleInput<'_>],
    type_store: &nia_ty::TypeStore,
    optimization: OptimizationPolicy,
    timings: nia_timing::TimingMode,
) -> BackendItemPlan {
    let timing = timings.detail();
    let mut diagnostics = input::validate_backend_lowering_inputs(modules);
    let mut optimization_report = BackendOptimizationReport {
        enabled_module_passes: enabled_module_passes(&optimization),
        enabled_function_passes: opt::enabled_function_passes(&optimization),
        enabled_global_passes: enabled_global_passes(&optimization),
        changed_passes: Vec::new(),
    };
    if !diagnostics.is_empty() {
        return BackendItemPlan::from_diagnostics(optimization, diagnostics);
    }
    let shared = time_backend_stage(timing, "backend_lower.shared_indexes", || {
        BackendLowerShared::new(modules)
    });
    let mut lowerers = time_backend_stage(timing, "backend_lower.new_lowerers", || {
        modules
            .iter()
            .map(|input| ModuleLowerer::new(input, type_store, optimization, &shared, timing))
            .collect::<Vec<_>>()
    });
    let mut lowered_modules = Vec::new();
    let mut pending_foreign_items = PendingForeignBackendItems::default();
    time_backend_stage(timing, "backend_lower.initial_modules", || {
        for lowerer in &mut lowerers {
            let module = lowerer.lower_initial_module();
            if timing {
                nia_timing::emit_query_note(
                    format!("backend_lower.module[{:?}]", module.id),
                    format!(
                        "functions={} instances={} structs={} unions={}",
                        module.functions.len(),
                        module.function_instances.len(),
                        module.struct_instances.len(),
                        module.union_instances.len()
                    ),
                );
            }
            pending_foreign_items.extend_from_lowerer(lowerer);
            diagnostics.extend(std::mem::take(&mut lowerer.diagnostics));
            optimization_report.changed_passes.extend(std::mem::take(
                &mut lowerer.optimization_report.changed_passes,
            ));
            lowered_modules.push(module);
        }
    });
    let module_indices = lowered_modules
        .iter()
        .enumerate()
        .map(|(index, module)| (module.id, index))
        .collect::<HashMap<_, _>>();
    assert_eq!(
        module_indices.len(),
        lowered_modules.len(),
        "Nia ICE: backend module plan contains duplicate module owners"
    );
    time_backend_stage(timing, "backend_lower.foreign_items", || {
        while !pending_foreign_items.is_empty() {
            let (plan, owner_diagnostics) =
                pending_foreign_items.drain_plan(&module_indices, lowerers.len());
            diagnostics.extend(owner_diagnostics);
            for (owner_index, refs) in plan.functions_by_owner.into_iter().enumerate() {
                if refs.is_empty() {
                    continue;
                }
                {
                    let lowerer = &mut lowerers[owner_index];
                    lowerer.lower_additional_functions(refs, &mut lowered_modules[owner_index]);
                }
                pending_foreign_items.extend_from_lowerer(&mut lowerers[owner_index]);
                diagnostics.extend(std::mem::take(&mut lowerers[owner_index].diagnostics));
                optimization_report.changed_passes.extend(std::mem::take(
                    &mut lowerers[owner_index].optimization_report.changed_passes,
                ));
            }

            for (owner_index, refs) in plan.function_instances_by_owner.into_iter().enumerate() {
                if refs.is_empty() {
                    continue;
                }
                lowerers[owner_index].lower_additional_function_instances_into_module(
                    refs,
                    &mut lowered_modules[owner_index],
                );
                pending_foreign_items.extend_from_lowerer(&mut lowerers[owner_index]);
                diagnostics.extend(std::mem::take(&mut lowerers[owner_index].diagnostics));
                optimization_report.changed_passes.extend(std::mem::take(
                    &mut lowerers[owner_index].optimization_report.changed_passes,
                ));
            }

            for (owner_index, refs) in plan.global_instances_by_owner.into_iter().enumerate() {
                if refs.is_empty() {
                    continue;
                }
                {
                    let lowerer = &mut lowerers[owner_index];
                    lowerer
                        .lower_additional_global_instances(refs, &mut lowered_modules[owner_index]);
                }
                pending_foreign_items.extend_from_lowerer(&mut lowerers[owner_index]);
                diagnostics.extend(std::mem::take(&mut lowerers[owner_index].diagnostics));
                optimization_report.changed_passes.extend(std::mem::take(
                    &mut lowerers[owner_index].optimization_report.changed_passes,
                ));
            }
        }
    });

    assert_eq!(
        lowerers.len(),
        lowered_modules.len(),
        "Nia ICE: backend lowerers must match materialized modules"
    );
    time_backend_stage(timing, "backend_lower.definition_membership", || {
        for (lowerer, module) in lowerers.iter_mut().zip(&mut lowered_modules) {
            lowerer.complete_definition_membership(module);
            diagnostics.extend(std::mem::take(&mut lowerer.diagnostics));
            optimization_report.changed_passes.extend(std::mem::take(
                &mut lowerer.optimization_report.changed_passes,
            ));
        }
    });
    diagnostics.extend(assign_unique_aggregate_instance_owners(
        &mut lowered_modules,
        type_store,
    ));
    diagnostics.extend(assign_unique_vtable_owners(
        &mut lowered_modules,
        type_store,
    ));

    BackendItemPlan {
        modules: lowered_modules
            .into_iter()
            .map(|module| BackendModuleItemPlan { module })
            .collect(),
        optimization,
        optimization_report,
        diagnostics,
    }
}

fn assign_unique_aggregate_instance_owners(
    modules: &mut [BackendModule],
    type_store: &nia_ty::TypeStore,
) -> Vec<Diagnostic> {
    // Generic instances may be discovered from several modules. Equal definitions are assigned
    // to the lexicographically earliest normalized source path, giving stable ownership without
    // depending on traversal or worker completion order.
    let array_lengths = backend_array_lengths(modules);
    let mut diagnostics = Vec::new();
    let mut struct_owners = Vec::<(BackendStructInstanceKey, (usize, usize))>::new();
    for (module_index, module) in modules.iter().enumerate() {
        for (item_index, item) in module.struct_instances.iter().enumerate() {
            let key = BackendStructInstanceKey {
                def_id: item.def_id,
                args: item.args.clone(),
                const_args: item.const_args.clone(),
            };
            let Some(owner_position) = struct_owners.iter().position(|(candidate, _)| {
                backend_struct_instance_keys_match(type_store, &array_lengths, candidate, &key)
            }) else {
                struct_owners.push((key, (module_index, item_index)));
                continue;
            };
            let (_, (owner_module_index, owner_item_index)) = struct_owners[owner_position];
            let owner_module = &modules[owner_module_index];
            let owner = &owner_module.struct_instances[owner_item_index];
            if !backend_struct_instance_payloads_match(type_store, &array_lengths, owner, item) {
                diagnostics.push(Diagnostic::internal_error_at(
                    codes::INVALID_BACKEND_IR,
                    item.span,
                    format!(
                        "conflicting backend struct instance definitions in modules {:?} and {:?}",
                        owner_module.id, module.id
                    ),
                ));
            }
            if module.source_identity.normalized_path()
                < owner_module.source_identity.normalized_path()
            {
                struct_owners[owner_position] = (key, (module_index, item_index));
            }
        }
    }
    for (module_index, module) in modules.iter_mut().enumerate() {
        let instances = std::mem::take(&mut module.struct_instances);
        module.struct_instances = instances
            .into_iter()
            .enumerate()
            .filter_map(|(item_index, item)| {
                let key = BackendStructInstanceKey {
                    def_id: item.def_id,
                    args: item.args.clone(),
                    const_args: item.const_args.clone(),
                };
                struct_owners
                    .iter()
                    .any(|(candidate, (owner_module, owner_index))| {
                        *owner_module == module_index
                            && *owner_index == item_index
                            && backend_struct_instance_keys_match(
                                type_store,
                                &array_lengths,
                                candidate,
                                &key,
                            )
                    })
                    .then_some(item)
            })
            .collect();
    }

    let mut union_owners = Vec::<(BackendStructInstanceKey, (usize, usize))>::new();
    for (module_index, module) in modules.iter().enumerate() {
        for (item_index, item) in module.union_instances.iter().enumerate() {
            let key = BackendStructInstanceKey {
                def_id: item.def_id,
                args: item.args.clone(),
                const_args: item.const_args.clone(),
            };
            let Some(owner_position) = union_owners.iter().position(|(candidate, _)| {
                backend_struct_instance_keys_match(type_store, &array_lengths, candidate, &key)
            }) else {
                union_owners.push((key, (module_index, item_index)));
                continue;
            };
            let (_, (owner_module_index, owner_item_index)) = union_owners[owner_position];
            let owner_module = &modules[owner_module_index];
            let owner = &owner_module.union_instances[owner_item_index];
            if !backend_union_instance_payloads_match(type_store, &array_lengths, owner, item) {
                diagnostics.push(Diagnostic::internal_error_at(
                    codes::INVALID_BACKEND_IR,
                    item.span,
                    format!(
                        "conflicting backend union instance definitions in modules {:?} and {:?}",
                        owner_module.id, module.id
                    ),
                ));
            }
            if module.source_identity.normalized_path()
                < owner_module.source_identity.normalized_path()
            {
                union_owners[owner_position] = (key, (module_index, item_index));
            }
        }
    }
    for (module_index, module) in modules.iter_mut().enumerate() {
        let instances = std::mem::take(&mut module.union_instances);
        module.union_instances = instances
            .into_iter()
            .enumerate()
            .filter_map(|(item_index, item)| {
                let key = BackendStructInstanceKey {
                    def_id: item.def_id,
                    args: item.args.clone(),
                    const_args: item.const_args.clone(),
                };
                union_owners
                    .iter()
                    .any(|(candidate, (owner_module, owner_index))| {
                        *owner_module == module_index
                            && *owner_index == item_index
                            && backend_struct_instance_keys_match(
                                type_store,
                                &array_lengths,
                                candidate,
                                &key,
                            )
                    })
                    .then_some(item)
            })
            .collect();
    }
    diagnostics
}

fn backend_array_lengths(modules: &[BackendModule]) -> HashMap<GlobalConstExprId, u64> {
    modules
        .iter()
        .flat_map(|module| {
            module
                .const_eval
                .array_lengths
                .iter()
                .map(|(id, value)| (*id, *value))
        })
        .collect()
}

fn backend_struct_instance_keys_match(
    type_store: &nia_ty::TypeStore,
    array_lengths: &HashMap<GlobalConstExprId, u64>,
    left: &BackendStructInstanceKey,
    right: &BackendStructInstanceKey,
) -> bool {
    let equivalence = BackendVtableTypeEquivalence {
        type_store,
        array_lengths,
    };
    left.def_id == right.def_id
        && equivalence.same_type_args_for_equiv(&left.args, &right.args)
        && equivalence.same_const_generic_args_for_equiv(&left.const_args, &right.const_args)
}

fn backend_fields_match(
    type_store: &nia_ty::TypeStore,
    array_lengths: &HashMap<GlobalConstExprId, u64>,
    left: &[nia_backend_ir::BackendField],
    right: &[nia_backend_ir::BackendField],
) -> bool {
    let equivalence = BackendVtableTypeEquivalence {
        type_store,
        array_lengths,
    };
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.def_id == right.def_id
                && left.name == right.name
                && equivalence.same_type_for_equiv(left.ty, right.ty)
        })
}

fn backend_struct_instance_payloads_match(
    type_store: &nia_ty::TypeStore,
    array_lengths: &HashMap<GlobalConstExprId, u64>,
    left: &BackendStructInstance,
    right: &BackendStructInstance,
) -> bool {
    left.def_id == right.def_id
        && left.name == right.name
        && left.symbol == right.symbol
        && left.is_extern == right.is_extern
        && backend_struct_instance_keys_match(
            type_store,
            array_lengths,
            &BackendStructInstanceKey {
                def_id: left.def_id,
                args: left.args.clone(),
                const_args: left.const_args.clone(),
            },
            &BackendStructInstanceKey {
                def_id: right.def_id,
                args: right.args.clone(),
                const_args: right.const_args.clone(),
            },
        )
        && backend_fields_match(type_store, array_lengths, &left.fields, &right.fields)
}

fn backend_union_instance_payloads_match(
    type_store: &nia_ty::TypeStore,
    array_lengths: &HashMap<GlobalConstExprId, u64>,
    left: &BackendUnionInstance,
    right: &BackendUnionInstance,
) -> bool {
    left.def_id == right.def_id
        && left.name == right.name
        && left.symbol == right.symbol
        && left.is_extern == right.is_extern
        && backend_struct_instance_keys_match(
            type_store,
            array_lengths,
            &BackendStructInstanceKey {
                def_id: left.def_id,
                args: left.args.clone(),
                const_args: left.const_args.clone(),
            },
            &BackendStructInstanceKey {
                def_id: right.def_id,
                args: right.args.clone(),
                const_args: right.const_args.clone(),
            },
        )
        && backend_fields_match(type_store, array_lengths, &left.fields, &right.fields)
}

struct BackendVtableTypeEquivalence<'a> {
    type_store: &'a nia_ty::TypeStore,
    array_lengths: &'a HashMap<GlobalConstExprId, u64>,
}

impl TypeEquivalence for BackendVtableTypeEquivalence<'_> {
    fn ty_kind_for_equiv(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.type_store.get(ty)
    }

    fn same_array_len_for_equiv(
        &self,
        left: &nia_ty::ArrayLenTy,
        right: &nia_ty::ArrayLenTy,
    ) -> bool {
        match (left, right) {
            (
                nia_ty::ArrayLenTy::Builtin {
                    builtin: left_builtin,
                    ty: left_ty,
                },
                nia_ty::ArrayLenTy::Builtin {
                    builtin: right_builtin,
                    ty: right_ty,
                },
            ) => left_builtin == right_builtin && self.same_type_for_equiv(*left_ty, *right_ty),
            (nia_ty::ArrayLenTy::ConstValue(left), nia_ty::ArrayLenTy::ConstExpr(right))
            | (nia_ty::ArrayLenTy::ConstExpr(right), nia_ty::ArrayLenTy::ConstValue(left)) => self
                .array_lengths
                .get(right)
                .is_some_and(|right| left == right),
            (nia_ty::ArrayLenTy::ConstExpr(left), nia_ty::ArrayLenTy::ConstExpr(right)) => {
                left == right
                    || self
                        .array_lengths
                        .get(left)
                        .zip(self.array_lengths.get(right))
                        .is_some_and(|(left, right)| left == right)
            }
            _ => left == right,
        }
    }

    fn same_type_for_equiv(&self, left: InternedTyId, right: InternedTyId) -> bool {
        left == right || self.compute_same_type_for_equiv(left, right)
    }

    fn same_const_generic_args_for_equiv(
        &self,
        left: &[ConstGenericArg],
        right: &[ConstGenericArg],
    ) -> bool {
        left.len() == right.len()
            && left.iter().zip(right).all(|(left, right)| {
                self.same_type_for_equiv(left.ty, right.ty)
                    && match (&left.value, &right.value) {
                        (ConstGenericValue::Int(left), ConstGenericValue::Int(right)) => {
                            left.bits() == right.bits()
                        }
                        (ConstGenericValue::Int(left), ConstGenericValue::ConstExpr(right))
                        | (ConstGenericValue::ConstExpr(right), ConstGenericValue::Int(left)) => {
                            self.array_lengths
                                .get(right)
                                .is_some_and(|right| left.bits() == u128::from(*right))
                        }
                        (
                            ConstGenericValue::ConstExpr(left),
                            ConstGenericValue::ConstExpr(right),
                        ) => {
                            left == right
                                || self
                                    .array_lengths
                                    .get(left)
                                    .zip(self.array_lengths.get(right))
                                    .is_some_and(|(left, right)| left == right)
                        }
                        (left, right) => left == right,
                    }
            })
    }
}

fn assign_unique_vtable_owners(
    modules: &mut [BackendModule],
    type_store: &nia_ty::TypeStore,
) -> Vec<Diagnostic> {
    // Vtable keys carry complete type payloads. Match them through the same
    // structural equivalence used for payload validation so semantically equal
    // rebuilt const representations share one owner.
    let array_lengths = backend_array_lengths(modules);
    let mut diagnostics = Vec::new();
    let mut owners = Vec::<(BackendTraitObjectVtableKey, (usize, usize))>::new();
    for (module_index, module) in modules.iter().enumerate() {
        for (vtable_index, vtable) in module.trait_object_vtables.iter().enumerate() {
            let Some(owner_position) = owners.iter().position(|(key, _)| {
                backend_vtable_keys_match(type_store, &array_lengths, key, &vtable.key)
            }) else {
                owners.push((vtable.key.clone(), (module_index, vtable_index)));
                continue;
            };
            let (_, (owner_module_index, owner_vtable_index)) = owners[owner_position];
            let owner_module = &modules[owner_module_index];
            let owner = &owner_module.trait_object_vtables[owner_vtable_index];
            if !backend_vtable_payloads_match(type_store, &array_lengths, owner, vtable) {
                diagnostics.push(Diagnostic::internal_error_at(
                    codes::INVALID_BACKEND_IR,
                    vtable.span,
                    format!(
                        "conflicting backend trait-object vtable definitions in modules {:?} and {:?}",
                        owner_module.id, module.id
                    ),
                ));
            }
            if module.source_identity.normalized_path()
                < owner_module.source_identity.normalized_path()
            {
                owners[owner_position] = (vtable.key.clone(), (module_index, vtable_index));
            }
        }
    }
    for (module_index, module) in modules.iter_mut().enumerate() {
        let vtables = std::mem::take(&mut module.trait_object_vtables);
        module.trait_object_vtables = vtables
            .into_iter()
            .enumerate()
            .filter_map(|(vtable_index, vtable)| {
                owners
                    .iter()
                    .any(|(key, (owner_module, owner_index))| {
                        *owner_module == module_index
                            && *owner_index == vtable_index
                            && backend_vtable_keys_match(
                                type_store,
                                &array_lengths,
                                key,
                                &vtable.key,
                            )
                    })
                    .then_some(vtable)
            })
            .collect();
    }
    diagnostics
}

fn backend_vtable_keys_match(
    type_store: &nia_ty::TypeStore,
    array_lengths: &HashMap<GlobalConstExprId, u64>,
    left: &BackendTraitObjectVtableKey,
    right: &BackendTraitObjectVtableKey,
) -> bool {
    let equivalence = BackendVtableTypeEquivalence {
        type_store,
        array_lengths,
    };
    equivalence.same_type_for_equiv(left.self_ty, right.self_ty)
        && equivalence.same_type_for_equiv(left.object_ty, right.object_ty)
}

fn backend_vtable_payloads_match(
    type_store: &nia_ty::TypeStore,
    array_lengths: &HashMap<GlobalConstExprId, u64>,
    left: &BackendTraitObjectVtable,
    right: &BackendTraitObjectVtable,
) -> bool {
    let equivalence = BackendVtableTypeEquivalence {
        type_store,
        array_lengths,
    };
    left.trait_id == right.trait_id
        && equivalence.same_type_args_for_equiv(&left.trait_args, &right.trait_args)
        && equivalence
            .same_const_generic_args_for_equiv(&left.trait_const_args, &right.trait_const_args)
        && left.entries.len() == right.entries.len()
        && left
            .entries
            .iter()
            .zip(&right.entries)
            .all(|(left, right)| {
                left.trait_id == right.trait_id
                    && equivalence.same_type_args_for_equiv(&left.trait_args, &right.trait_args)
                    && equivalence.same_const_generic_args_for_equiv(
                        &left.trait_const_args,
                        &right.trait_const_args,
                    )
                    && left.method_id == right.method_id
                    && left.method_name == right.method_name
                    && left.slot == right.slot
                    && backend_vtable_functions_match(&equivalence, &left.function, &right.function)
            })
}

fn backend_vtable_functions_match(
    equivalence: &impl TypeEquivalence,
    left: &BackendTraitObjectVtableFunction,
    right: &BackendTraitObjectVtableFunction,
) -> bool {
    match (left, right) {
        (
            BackendTraitObjectVtableFunction::Function(left),
            BackendTraitObjectVtableFunction::Function(right),
        ) => left == right,
        (
            BackendTraitObjectVtableFunction::FunctionInstance {
                def_id: left_def,
                arg_module_id: left_module,
                self_arg: left_self,
                args: left_args,
                const_args: left_const_args,
            },
            BackendTraitObjectVtableFunction::FunctionInstance {
                def_id: right_def,
                arg_module_id: right_module,
                self_arg: right_self,
                args: right_args,
                const_args: right_const_args,
            },
        ) => {
            left_def == right_def
                && left_module == right_module
                && match (left_self, right_self) {
                    (None, None) => true,
                    (Some(left), Some(right)) => equivalence.same_type_for_equiv(*left, *right),
                    _ => false,
                }
                && equivalence.same_type_args_for_equiv(left_args, right_args)
                && equivalence.same_const_generic_args_for_equiv(left_const_args, right_const_args)
        }
        _ => false,
    }
}

/// Finalizes module plans and joins them with timing disabled.
pub fn finalize_backend_module_item_plans(
    modules: &[BackendLowerModuleInput<'_>],
    type_store: &nia_ty::TypeStore,
    finalization: BackendItemPlanFinalization,
    module_plans: Vec<BackendModuleItemPlan>,
) -> BackendLowering {
    finalize_backend_module_item_plans_with_timings(
        modules,
        type_store,
        finalization,
        module_plans,
        nia_timing::TimingMode::Off,
    )
}

/// Finalizes and joins module plans produced by the matching planning call.
///
/// `modules` and `module_plans` must have equal length and identical module
/// order. If planning already produced diagnostics, finalization is skipped and
/// the partial planned program is returned for diagnostics only.
pub fn finalize_backend_module_item_plans_with_timings(
    modules: &[BackendLowerModuleInput<'_>],
    type_store: &nia_ty::TypeStore,
    finalization: BackendItemPlanFinalization,
    module_plans: Vec<BackendModuleItemPlan>,
    timings: nia_timing::TimingMode,
) -> BackendLowering {
    let optimization = finalization.optimization;
    if !finalization.diagnostics.is_empty() {
        let program = BackendProgram::new(
            module_plans
                .into_iter()
                .map(|module_plan| module_plan.module)
                .collect(),
        );
        let codegen_partitions = program.codegen_partition_plan();
        return BackendLowering {
            program,
            owner_directory: finalization.owner_directory,
            codegen_partitions,
            optimization,
            optimization_report: finalization.optimization_report,
            diagnostics: finalization.diagnostics,
        };
    }
    assert_eq!(
        modules.len(),
        module_plans.len(),
        "Nia ICE: backend item plan must match finalization inputs"
    );
    for (input, module_plan) in modules.iter().zip(&module_plans) {
        assert_eq!(
            input.module_id, module_plan.module.id,
            "Nia ICE: backend item plan owner order must match finalization inputs"
        );
    }

    let finalization_context =
        BackendProgramFinalizationContext::new(modules, type_store, optimization, timings);
    let module_finalizations =
        time_backend_stage(timings.detail(), "backend_lower.final_modules", || {
            modules
                .iter()
                .zip(module_plans)
                .enumerate()
                .map(|(position, (input, module_plan))| {
                    finalization_context.finalize_module(position, input, module_plan)
                })
                .collect::<Vec<_>>()
        });
    let module_order = modules
        .iter()
        .map(|input| input.module_id)
        .collect::<Vec<_>>();
    let mut collector = BackendModuleFinalizationCollector::new(finalization, &module_order);
    for module_finalization in module_finalizations {
        let position = module_finalization.position;
        collector.push(position, module_finalization);
    }
    collector.finish()
}

fn time_backend_stage<T>(enabled: bool, name: &str, f: impl FnOnce() -> T) -> T {
    nia_timing::time_detail(enabled, name, f)
}

fn enabled_module_passes(optimization: &OptimizationPolicy) -> Vec<&'static str> {
    let mut passes = Vec::new();
    if optimization.level == nia_opt::NiaOptimizationLevel::O3 {
        passes.push(module_devirt::DEVIRTUALIZE_DIRECT_TRAIT_CALLS_PASS);
    }
    if module_const_prop::cross_function_constant_propagation_enabled(optimization) {
        passes.push(module_const_prop::PROPAGATE_CROSS_FUNCTION_CONSTANTS_PASS);
    }
    if !matches!(optimization.inline_threshold, InlineThreshold::Never) {
        passes.push(module_inline::INLINE_LEAF_FUNCTIONS_PASS);
    }
    if optimization
        .dead_code_elim
        .at_least(OptimizationDepth::Full)
    {
        passes.push(module_dce::REMOVE_UNUSED_FUNCTIONS_PASS);
        passes.push(module_dce::REMOVE_UNUSED_FUNCTION_INSTANCES_PASS);
    }
    passes
}

fn enabled_global_passes(optimization: &OptimizationPolicy) -> Vec<&'static str> {
    if static_init_simplification_enabled(optimization) {
        vec![items::SIMPLIFY_STATIC_INIT_PASS]
    } else {
        Vec::new()
    }
}

pub(crate) fn static_init_simplification_enabled(optimization: &OptimizationPolicy) -> bool {
    optimization.const_fold.at_least(OptimizationDepth::Full) || optimization.prefer_size
}

pub(crate) struct ModuleLowerer<'a> {
    pub(crate) input: &'a BackendLowerModuleInput<'a>,
    pub(crate) type_store: &'a nia_ty::TypeStore,
    shared: &'a BackendLowerShared,
    pub(crate) optimization: OptimizationPolicy,
    pub(crate) type_context: type_context::BackendTypeContext<'a>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    optimization_report: BackendOptimizationReport,
    missing_array_len_diagnostics: HashSet<GlobalConstExprId>,
    missing_source_identity_diagnostics: HashSet<ModuleId>,
    missing_type_diagnostics: HashSet<InternedTyId>,
    extension_generics_by_method: HashMap<GlobalDefId, Vec<SymbolId>>,
    extension_method_sources_by_def: HashMap<GlobalDefId, ExtensionMethodSource>,
    trait_context: trait_context::BackendTraitContext,
    instantiation: instantiation_context::BackendInstantiationContext,
    foreign_function_refs: Vec<GlobalDefId>,
    foreign_function_instance_refs: Vec<FunctionInstanceRef>,
    foreign_global_instance_refs: Vec<GlobalInstanceRef>,
    struct_layout_instances_by_def: HashMap<DefId, Vec<StructLayoutKey>>,
    union_layout_instances_by_def: HashMap<DefId, Vec<StructLayoutKey>>,
    effective_generics: HashMap<GlobalDefId, Vec<SymbolId>>,
    def_names: HashMap<GlobalDefId, String>,
    function_sources: HashMap<GlobalDefId, BackendFunctionSource<'a>>,
    aggregate_sources: HashMap<GlobalDefId, BackendAggregateSource<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BuiltinTraitGoalKey {
    self_ty: InternedTyId,
    trait_id: nia_ids::BuiltinTrait,
    trait_args: Vec<InternedTyId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ExtensionTraitMethodKey {
    trait_id: TraitId,
    method_name: SymbolId,
    trait_arg_count: usize,
    /// Const-argument count is part of the lookup bucket; values are checked
    /// by trait selection after the cheap arity filter.
    trait_const_arg_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExtensionTraitMethodCandidate {
    module_id: ModuleId,
    target_ty: InternedTyId,
    method_def_id: GlobalDefId,
    trait_args: Vec<InternedTyId>,
    /// Concrete or generic const arguments of the implemented trait.
    trait_const_args: Vec<nia_ty::ConstGenericArg>,
    where_predicates: Vec<WherePredicateSignature>,
    effective_generics: Vec<SymbolId>,
}

#[derive(Debug, Clone)]
pub(crate) struct ExtensionMethodSource {
    module_id: ModuleId,
    target_ty: InternedTyId,
    where_predicates: Vec<WherePredicateSignature>,
}

pub(crate) struct BackendLowerShared {
    source_identities: HashMap<ModuleId, nia_source::SourceIdentity>,
    program_extension_generics_by_method: HashMap<GlobalDefId, Vec<SymbolId>>,
    program_extension_method_sources_by_def: HashMap<GlobalDefId, ExtensionMethodSource>,
    program_trait_impls_by_method: HashMap<GlobalDefId, usize>,
    program_extension_trait_method_candidates:
        HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>>,
    program_trait_methods_with_defaults: HashSet<GlobalDefId>,
    program_method_symbols_by_def: HashMap<GlobalDefId, SymbolId>,
}

impl BackendLowerShared {
    fn new(modules: &[BackendLowerModuleInput<'_>]) -> Self {
        let first = modules.first();
        Self {
            source_identities: first
                .map(|input| input.program.source_identities().clone())
                .unwrap_or_default(),
            program_extension_generics_by_method: first
                .map(|input| index_extension_generics_by_method(input.program.extension_methods()))
                .unwrap_or_default(),
            program_extension_method_sources_by_def: first
                .map(index_program_extension_method_sources_by_def)
                .unwrap_or_default(),
            program_trait_impls_by_method: first
                .map(index_program_trait_impls_by_method)
                .unwrap_or_default(),
            program_extension_trait_method_candidates:
                index_program_extension_trait_method_candidates(first),
            program_trait_methods_with_defaults: first
                .map(index_program_trait_methods_with_defaults)
                .unwrap_or_default(),
            program_method_symbols_by_def: first
                .map(index_program_method_symbols_by_def)
                .unwrap_or_default(),
        }
    }
}

#[derive(Clone, Copy)]
struct BackendFunctionSource<'a> {
    span: nia_span::Span,
    function: &'a nia_ast::FunctionItem,
}

#[derive(Clone, Copy)]
enum BackendAggregateSource<'a> {
    Struct {
        node_key: &'a VersionedNodeKey,
        span: nia_span::Span,
        item: &'a nia_ast::StructItem,
    },
    Union {
        node_key: &'a VersionedNodeKey,
        span: nia_span::Span,
        item: &'a nia_ast::UnionItem,
    },
}

#[derive(Default)]
struct PendingForeignBackendItems {
    functions: VecDeque<GlobalDefId>,
    function_instances: VecDeque<FunctionInstanceRef>,
    global_instances: VecDeque<GlobalInstanceRef>,
    queued_functions: HashSet<GlobalDefId>,
    queued_function_instances: HashSet<FunctionInstanceKey>,
    queued_global_instances: HashSet<GlobalInstanceKey>,
}

struct ForeignBackendItemPlan {
    functions_by_owner: Vec<Vec<GlobalDefId>>,
    function_instances_by_owner: Vec<Vec<FunctionInstanceRef>>,
    global_instances_by_owner: Vec<Vec<GlobalInstanceRef>>,
}

impl PendingForeignBackendItems {
    fn extend_from_lowerer(&mut self, lowerer: &mut ModuleLowerer<'_>) {
        self.functions
            .extend(std::mem::take(&mut lowerer.foreign_function_refs));
        self.function_instances
            .extend(std::mem::take(&mut lowerer.foreign_function_instance_refs));
        self.global_instances
            .extend(std::mem::take(&mut lowerer.foreign_global_instance_refs));
    }

    fn is_empty(&self) -> bool {
        self.functions.is_empty()
            && self.function_instances.is_empty()
            && self.global_instances.is_empty()
    }

    fn drain_plan(
        &mut self,
        module_indices: &HashMap<ModuleId, usize>,
        module_count: usize,
    ) -> (ForeignBackendItemPlan, Vec<Diagnostic>) {
        let mut plan = ForeignBackendItemPlan {
            functions_by_owner: (0..module_count).map(|_| Vec::new()).collect(),
            function_instances_by_owner: (0..module_count).map(|_| Vec::new()).collect(),
            global_instances_by_owner: (0..module_count).map(|_| Vec::new()).collect(),
        };
        let mut diagnostics = Vec::new();
        while let Some(function) = self.functions.pop_front() {
            if !self.queued_functions.insert(function) {
                continue;
            }
            let Some(owner_index) = foreign_item_owner_index(
                module_indices,
                function.module_id,
                "source function",
                &mut diagnostics,
            ) else {
                continue;
            };
            plan.functions_by_owner[owner_index].push(function);
        }
        for functions in &mut plan.functions_by_owner {
            functions.sort_unstable();
        }
        while let Some(instance) = self.function_instances.pop_front() {
            let key = instance.key();
            if !self.queued_function_instances.insert(key.clone()) {
                continue;
            }
            let Some(owner_index) = foreign_item_owner_index(
                module_indices,
                instance.def_id.module_id,
                "function instance",
                &mut diagnostics,
            ) else {
                continue;
            };
            plan.function_instances_by_owner[owner_index].push(instance);
        }
        while let Some(instance) = self.global_instances.pop_front() {
            if !self.queued_global_instances.insert(instance.key()) {
                continue;
            }
            let Some(owner_index) = foreign_item_owner_index(
                module_indices,
                instance.def_id.module_id,
                "global instance",
                &mut diagnostics,
            ) else {
                continue;
            };
            plan.global_instances_by_owner[owner_index].push(instance);
        }
        (plan, diagnostics)
    }
}

fn foreign_item_owner_index(
    module_indices: &HashMap<ModuleId, usize>,
    owner: ModuleId,
    item_kind: &'static str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let Some(index) = module_indices.get(&owner).copied() else {
        diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            nia_span::Span::default(),
            format!("foreign backend {item_kind} owner {owner:?} is not in the module plan"),
        ));
        return None;
    };
    Some(index)
}

#[derive(Default)]
struct ReachabilityWorklist {
    pending_functions: VecDeque<GlobalDefId>,
    queued_functions: HashSet<GlobalDefId>,
    pending_instances: Vec<FunctionInstanceRef>,
    queued_instances: HashSet<FunctionInstanceKey>,
    pending_global_instances: Vec<GlobalInstanceRef>,
    queued_global_instances: HashSet<GlobalInstanceKey>,
}

struct FunctionInstanceMaterialization {
    instances: Vec<BackendFunctionInstance>,
    closure_entries: Vec<BackendClosureEntry>,
    discovery: BackendItemDiscovery,
}

struct GlobalInstanceMaterialization {
    instances: Vec<BackendGlobalInstance>,
    discovery: BackendItemDiscovery,
}

fn backend_function_instance_key(instance: &BackendFunctionInstance) -> FunctionInstanceKey {
    FunctionInstanceKey {
        def_id: instance.def_id,
        arg_module_id: instance.arg_module_id,
        self_arg: instance.self_arg,
        args: instance.args.clone(),
        const_args: instance.const_args.clone(),
    }
}

#[derive(Default)]
struct BackendItemDiscovery {
    refs: FunctionBodyRefs,
    trait_object_vtables: Vec<BackendTraitObjectVtable>,
}

impl BackendItemDiscovery {
    fn extend(&mut self, other: Self) {
        self.refs.extend(other.refs);
        self.trait_object_vtables.extend(other.trait_object_vtables);
    }
}

#[derive(Default)]
struct ReachableAggregateRoots {
    seen_tys: HashSet<InternedTyId>,
    seen_structs: HashSet<GlobalDefId>,
    structs: Vec<GlobalDefId>,
    seen_unions: HashSet<GlobalDefId>,
    unions: Vec<GlobalDefId>,
}

struct ReachableAggregateInputs<'a> {
    globals: &'a [BackendGlobal],
    functions: &'a [BackendFunction],
    function_instances: &'a [BackendFunctionInstance],
    closure_entries: &'a [BackendClosureEntry],
    struct_instances: &'a [nia_backend_ir::BackendStructInstance],
    union_instances: &'a [nia_backend_ir::BackendUnionInstance],
    trait_object_vtables: &'a [BackendTraitObjectVtable],
}

impl ReachableAggregateRoots {
    fn add_backend_function(
        &mut self,
        lowerer: &mut ModuleLowerer<'_>,
        function: &BackendFunction,
    ) {
        self.add_ty(lowerer, function.return_type);
        for param in &function.params {
            self.add_ty(lowerer, param.passing_ty);
            self.add_ty(lowerer, param.local_ty);
        }
        if let Some(body) = &function.function_body {
            self.add_function_body(lowerer, body);
        }
    }

    fn add_backend_function_instance(
        &mut self,
        lowerer: &mut ModuleLowerer<'_>,
        function: &BackendFunctionInstance,
    ) {
        self.add_ty(lowerer, function.return_type);
        for arg in &function.args {
            self.add_ty(lowerer, *arg);
        }
        for param in &function.params {
            self.add_ty(lowerer, param.passing_ty);
            self.add_ty(lowerer, param.local_ty);
        }
        if let Some(body) = &function.function_body {
            self.add_function_body(lowerer, body);
        }
    }

    fn add_backend_closure_entry(
        &mut self,
        lowerer: &mut ModuleLowerer<'_>,
        entry: &BackendClosureEntry,
    ) {
        self.add_ty(lowerer, entry.abi.state_type);
        self.add_ty(lowerer, entry.abi.state_pointer_type);
        self.add_ty(lowerer, entry.abi.return_type);
        for param in &entry.abi.params {
            self.add_ty(lowerer, *param);
        }
        self.add_function_body(lowerer, &entry.function_body);
    }

    fn add_function_body(&mut self, lowerer: &mut ModuleLowerer<'_>, body: &FunctionBody) {
        self.add_ty(lowerer, body.ty);
        for local in &body.locals {
            self.add_ty(lowerer, local.ty);
        }
    }

    fn add_static_init(
        &mut self,
        lowerer: &mut ModuleLowerer<'_>,
        init: &nia_static_ir::StaticInit,
    ) {
        match init {
            nia_static_ir::StaticInit::Array(elems)
            | nia_static_ir::StaticInit::Tuple(elems)
            | nia_static_ir::StaticInit::Vector(elems) => {
                for elem in elems {
                    self.add_static_init(lowerer, elem);
                }
            }
            nia_static_ir::StaticInit::Repeat { value, .. } => {
                self.add_static_init(lowerer, value);
            }
            nia_static_ir::StaticInit::Struct(fields) => {
                for field in fields {
                    self.add_static_init(lowerer, &field.value);
                }
            }
            nia_static_ir::StaticInit::AddrOfFunction {
                args, const_args, ..
            } => {
                for arg in args {
                    self.add_ty(lowerer, *arg);
                }
                for arg in const_args {
                    self.add_ty(lowerer, arg.ty);
                }
            }
            nia_static_ir::StaticInit::Zero
            | nia_static_ir::StaticInit::Int(_)
            | nia_static_ir::StaticInit::Float(_)
            | nia_static_ir::StaticInit::Bool(_)
            | nia_static_ir::StaticInit::Char(_)
            | nia_static_ir::StaticInit::Byte(_)
            | nia_static_ir::StaticInit::Chars(_)
            | nia_static_ir::StaticInit::Bytes(_)
            | nia_static_ir::StaticInit::NullPtr
            | nia_static_ir::StaticInit::AddrOfGlobal { .. } => {}
        }
    }

    fn add_ty(&mut self, lowerer: &mut ModuleLowerer<'_>, ty: InternedTyId) {
        if !self.seen_tys.insert(ty) {
            return;
        }
        match lowerer.ty_kind(ty).cloned() {
            Some(TyKind::Tuple(elems)) => {
                for elem in elems {
                    self.add_ty(lowerer, elem);
                }
            }
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::VolatilePointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem })
            | Some(TyKind::Optional { elem }) => self.add_ty(lowerer, elem),
            Some(TyKind::Array { len, elem }) => {
                if let ArrayLenTy::Builtin { ty, .. } = len {
                    self.add_ty(lowerer, ty);
                }
                self.add_ty(lowerer, elem);
            }
            Some(TyKind::Range { bound, .. }) => {
                if let Some(bound) = bound {
                    self.add_ty(lowerer, bound);
                }
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            })
            | Some(TyKind::Callable {
                params,
                return_type,
                ..
            })
            | Some(TyKind::CallablePointee {
                params,
                return_type,
            }) => {
                for param in params {
                    self.add_ty(lowerer, param);
                }
                self.add_ty(lowerer, return_type);
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                self.add_ty(lowerer, error);
                self.add_ty(lowerer, value);
            }
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                self.add_struct(def_id);
                self.add_union(def_id);
                for arg in args {
                    self.add_ty(lowerer, arg);
                }
                for arg in const_args {
                    self.add_ty(lowerer, arg.ty);
                }
                for field_ty in lowerer.struct_field_tys(def_id) {
                    self.add_ty(lowerer, field_ty);
                }
                for field_ty in lowerer.union_field_tys(def_id) {
                    self.add_ty(lowerer, field_ty);
                }
            }
            Some(TyKind::BuiltinTrait { args, .. }) => {
                for arg in args {
                    self.add_ty(lowerer, arg);
                }
            }
            Some(TyKind::TraitObject {
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            }) => {
                for arg in trait_args {
                    self.add_ty(lowerer, arg);
                }
                for arg in trait_const_args {
                    self.add_ty(lowerer, arg.ty);
                }
                for binding in associated_type_bindings {
                    for arg in binding.trait_args {
                        self.add_ty(lowerer, arg);
                    }
                    for arg in binding.trait_const_args {
                        self.add_ty(lowerer, arg.ty);
                    }
                    self.add_ty(lowerer, binding.ty);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                trait_const_args,
                ..
            }) => {
                self.add_ty(lowerer, self_ty);
                for arg in trait_args {
                    self.add_ty(lowerer, arg);
                }
                for arg in trait_const_args {
                    self.add_ty(lowerer, arg.ty);
                }
            }
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::Opaque
                | TyKind::GenericParam(_)
                | TyKind::SelfParam
                | TyKind::BuiltinType(_)
                | TyKind::Primitive(_)
                | TyKind::Vector { .. }
                | TyKind::ClosureState { .. },
            )
            | None => {}
        }
    }

    fn add_struct(&mut self, def_id: GlobalDefId) {
        if self.seen_structs.insert(def_id) {
            self.structs.push(def_id);
        }
    }

    fn add_union(&mut self, def_id: GlobalDefId) {
        if self.seen_unions.insert(def_id) {
            self.unions.push(def_id);
        }
    }
}

impl ReachabilityWorklist {
    fn enqueue_function(&mut self, def_id: GlobalDefId) {
        if self.queued_functions.insert(def_id) {
            self.pending_functions.push_back(def_id);
        }
    }

    fn enqueue_refs(&mut self, refs: FunctionBodyRefs) {
        for function in refs.functions {
            self.enqueue_function(function);
        }
        self.enqueue_instances(refs.function_instances);
        self.enqueue_global_instances(refs.global_instances);
    }

    fn enqueue_instances(&mut self, refs: impl IntoIterator<Item = FunctionInstanceRef>) {
        for instance in refs {
            if self.queued_instances.insert(instance.key()) {
                self.pending_instances.push(instance);
            }
        }
    }

    fn enqueue_global_instances(&mut self, refs: impl IntoIterator<Item = GlobalInstanceRef>) {
        for instance in refs {
            if self.queued_global_instances.insert(instance.key()) {
                self.pending_global_instances.push(instance);
            }
        }
    }

    fn enqueue_vtable_refs(&mut self, vtable: &BackendTraitObjectVtable) {
        for entry in &vtable.entries {
            match &entry.function {
                BackendTraitObjectVtableFunction::Function(function) => {
                    self.enqueue_function(*function);
                }
                BackendTraitObjectVtableFunction::FunctionInstance {
                    def_id,
                    arg_module_id,
                    self_arg,
                    args,
                    const_args,
                } => self.enqueue_instances([FunctionInstanceRef {
                    def_id: *def_id,
                    arg_module_id: *arg_module_id,
                    self_arg: *self_arg,
                    args: args.clone(),
                    const_args: const_args.clone(),
                    span: vtable.span,
                }]),
            }
        }
    }
}

fn program_def(input: &BackendLowerModuleInput<'_>, def_id: GlobalDefId) -> Option<nia_defs::Def> {
    if def_id.module_id == input.module_id {
        return input.defs.defs.get(def_id.def_id).cloned();
    }
    input
        .program
        .defs(def_id.module_id)?
        .defs
        .get(def_id.def_id)
        .cloned()
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TypeInstantiationKey {
    ty: InternedTyId,
    substitutions: TypeSubstitutionId,
    current_function: Option<GlobalDefId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TypeSubstitutionId(pub(crate) usize);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TypeSubstitutionKey {
    self_arg: Option<InternedTyId>,
    substitutions: Vec<(SymbolId, InternedTyId)>,
    const_substitutions: Vec<(SymbolId, nia_ty::ConstGenericArg)>,
}

#[cfg(test)]
mod tests;
