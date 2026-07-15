// SPDX-License-Identifier: GPL-3.0-or-later
mod function_instances;
mod function_refs;
mod input;
mod instantiate;
mod instantiation_context;
mod items;
mod layout_extender;
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

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use nia_ast::{BindingItem, Block, Expr, StmtKind, Visibility, generic_param_names};
use nia_backend_ir::{
    BackendFunction, BackendFunctionInstance, BackendGlobal, BackendGlobalInstance,
    BackendGlobalInstanceKey, BackendLayouts, BackendModule, BackendProgram, BackendStruct,
    BackendTraitObjectVtable, BackendTraitObjectVtableFunction, BackendTraitObjectVtableKey,
    BackendUnion,
};
use nia_body_ir::BodyIr;
use nia_defs::{DefCollection, DefId, DefKind, ExtensionMethods, VisibleExtensionMethods};
use nia_diagnostic::Diagnostic;
use nia_function_ir::FunctionBody;
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId, TraitId, TyInternerId};
use nia_item_signatures::{
    ItemSignatures, ProgramEnumSignature, ProgramFunctionSignature, ProgramStructSignature,
    ProgramTraitImplIndex, ProgramTraitImplSignature, ProgramTraitSignature, ProgramUnionSignature,
    WherePredicateSignature,
};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNodeKind};
use nia_layout::{Layouts, StructLayoutKey};
use nia_local_resolve::LocalResolution;
use nia_mangle::{mangle_instance_symbol_id, mangle_symbol_id};
use nia_monomorphize::Monomorphization;
use nia_node_id::VersionedNodeKey;
use nia_opt::{InlineThreshold, OptimizationDepth, OptimizationPolicy};
use nia_sema_ir::SemanticFacts;
use nia_symbol::{SymbolId, SymbolText, known, symbol_text_or_unresolved};
use nia_ty::TyKind;
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_value_resolve::ValueResolution;

use crate::function_refs::{
    FunctionInstanceKey, FunctionInstanceRef, FunctionRefs, GlobalInstanceKey, GlobalInstanceRef,
};

#[derive(Debug, Clone, PartialEq)]
pub struct BackendLowering {
    pub program: BackendProgram,
    pub optimization: OptimizationPolicy,
    pub optimization_report: BackendOptimizationReport,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BackendOptimizationReport {
    pub enabled_module_passes: Vec<&'static str>,
    pub enabled_function_passes: Vec<&'static str>,
    pub enabled_global_passes: Vec<&'static str>,
    pub changed_passes: Vec<BackendOptimizationChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendOptimizationChange {
    Function {
        module_id: ModuleId,
        function: GlobalDefId,
        pass: &'static str,
        is_instance: bool,
        type_arg_count: usize,
    },
    Global {
        module_id: ModuleId,
        global: GlobalDefId,
        pass: &'static str,
    },
}

#[derive(Clone)]
pub struct BackendLowerModuleInput<'a> {
    pub module_id: ModuleId,
    pub module_name: String,
    pub symbols: &'a dyn SymbolText,
    pub active_item_tree: &'a ActiveModuleItemTree,
    pub defs: &'a DefCollection,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub type_lowering: &'a TypeLowering,
    pub signatures: &'a ItemSignatures,
    pub type_normalization: &'a TypeNormalization,
    pub body_ir: &'a BodyIr,
    pub function_interner: &'a nia_ty::TyInterner,
    pub semantic_facts: &'a SemanticFacts,
    pub extensions: &'a VisibleExtensionMethods,
    pub const_array_lengths: &'a nia_const_check::ConstArrayLengths,
    pub const_enum_values: &'a nia_const_check::ConstEnumValues,
    pub program_const:
        &'a std::collections::HashMap<ModuleId, &'a nia_const_check::ConstArrayLengths>,
    pub layouts: &'a Layouts,
    pub function_bodies: &'a std::collections::HashMap<GlobalDefId, FunctionBody>,
    pub roots: BackendFunctionRoots,
    pub reachable_globals: Option<&'a std::collections::HashSet<GlobalDefId>>,
    pub reachable_structs: Option<&'a std::collections::HashSet<GlobalDefId>>,
    pub reachable_unions: Option<&'a std::collections::HashSet<GlobalDefId>>,
    pub program_function_bodies: &'a std::collections::HashMap<GlobalDefId, FunctionBody>,
    pub extension_interner: Option<&'a nia_ty::TyInterner>,
    pub program_extension_methods: &'a ExtensionMethods,
    pub program_extensions: &'a std::collections::HashMap<
        ModuleId,
        (&'a VisibleExtensionMethods, &'a nia_ty::TyInterner),
    >,
    pub program_defs: &'a dyn Fn(ModuleId) -> Option<Arc<DefCollection>>,
    pub program_function_body_interners: &'a ProgramFunctionBodyInterners<'a>,
    pub program_type_normalizations:
        &'a std::collections::HashMap<ModuleId, nia_type_normalize::TypeNormalization>,
    pub program_functions: &'a std::collections::HashMap<GlobalDefId, ProgramFunctionSignature>,
    pub program_structs: &'a std::collections::HashMap<GlobalDefId, ProgramStructSignature>,
    pub program_unions: &'a std::collections::HashMap<GlobalDefId, ProgramUnionSignature>,
    pub program_enums: &'a std::collections::HashMap<GlobalDefId, ProgramEnumSignature>,
    pub program_traits: &'a std::collections::HashMap<GlobalDefId, ProgramTraitSignature>,
    pub program_type_aliases:
        &'a std::collections::HashMap<GlobalDefId, nia_item_signatures::ProgramTypeAliasSignature>,
    pub trait_impls: &'a [ProgramTraitImplSignature],
    pub trait_impl_index: &'a ProgramTraitImplIndex,
}

impl std::fmt::Debug for BackendLowerModuleInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackendLowerModuleInput")
            .field("module_id", &self.module_id)
            .field("module_name", &self.module_name)
            .field("program_defs", &true)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ProgramFunctionBodyInterners<'a> {
    by_module: HashMap<ModuleId, &'a nia_ty::TyInterner>,
}

impl<'a> ProgramFunctionBodyInterners<'a> {
    pub fn from_modules(
        modules: impl IntoIterator<Item = (ModuleId, &'a nia_ty::TyInterner)>,
    ) -> Self {
        Self {
            by_module: modules.into_iter().collect(),
        }
    }

    pub fn for_module(&self, module_id: ModuleId) -> Option<&'a nia_ty::TyInterner> {
        self.by_module.get(&module_id).copied()
    }

    pub fn values(&self) -> impl Iterator<Item = &'a nia_ty::TyInterner> + '_ {
        self.by_module.values().copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendFunctionRoots {
    #[default]
    Public,
    EntryPoints,
    FunctionBodies,
    NoFunctions,
}

pub fn lower_backend_program(
    modules: &[BackendLowerModuleInput<'_>],
    monomorphization: &Monomorphization,
    optimization: OptimizationPolicy,
) -> BackendLowering {
    lower_backend_program_with_timings(
        modules,
        monomorphization,
        optimization,
        nia_timing::TimingMode::Off,
    )
}

pub fn lower_backend_program_with_timings(
    modules: &[BackendLowerModuleInput<'_>],
    monomorphization: &Monomorphization,
    optimization: OptimizationPolicy,
    timings: nia_timing::TimingMode,
) -> BackendLowering {
    let timing = timings.detail();
    let mut diagnostics = input::validate_backend_lowering_inputs(modules);
    let mut optimization_report = BackendOptimizationReport {
        enabled_module_passes: enabled_module_passes(&optimization),
        enabled_function_passes: opt::enabled_function_passes(&optimization),
        enabled_global_passes: enabled_global_passes(&optimization),
        changed_passes: Vec::new(),
    };
    if !diagnostics.is_empty() {
        return BackendLowering {
            program: BackendProgram {
                modules: Vec::new(),
            },
            optimization,
            optimization_report,
            diagnostics,
        };
    }
    let shared = time_backend_stage(timing, "backend_lower.shared_indexes", || {
        BackendLowerShared::new(modules)
    });
    let mut lowerers = time_backend_stage(timing, "backend_lower.new_lowerers", || {
        modules
            .iter()
            .map(|input| ModuleLowerer::new(input, monomorphization, optimization, &shared, timing))
            .collect::<Vec<_>>()
    });
    let mut lowered_modules = Vec::new();
    let mut pending_foreign_instances = VecDeque::new();
    let mut pending_foreign_global_instances = VecDeque::new();
    time_backend_stage(timing, "backend_lower.initial_modules", || {
        for lowerer in &mut lowerers {
            let module = lowerer.lower_module();
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
            pending_foreign_instances
                .extend(std::mem::take(&mut lowerer.foreign_function_instance_refs));
            pending_foreign_global_instances
                .extend(std::mem::take(&mut lowerer.foreign_global_instance_refs));
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
    let mut pending_foreign_functions = lowerers
        .iter_mut()
        .flat_map(|lowerer| std::mem::take(&mut lowerer.foreign_function_refs))
        .collect::<VecDeque<_>>();
    let mut queued_foreign_functions = HashSet::new();
    let mut queued_foreign_instances = HashSet::new();
    let mut queued_foreign_global_instances = HashSet::new();
    time_backend_stage(timing, "backend_lower.foreign_instances", || {
        while !pending_foreign_functions.is_empty()
            || !pending_foreign_instances.is_empty()
            || !pending_foreign_global_instances.is_empty()
        {
            let mut function_batches = (0..lowerers.len()).map(|_| Vec::new()).collect::<Vec<_>>();
            while let Some(function) = pending_foreign_functions.pop_front() {
                if !queued_foreign_functions.insert(function) {
                    continue;
                }
                let Some(owner_index) = module_indices.get(&function.module_id).copied() else {
                    continue;
                };
                function_batches[owner_index].push(function);
            }
            for (owner_index, refs) in function_batches.into_iter().enumerate() {
                if refs.is_empty() {
                    continue;
                }
                {
                    let lowerer = &mut lowerers[owner_index];
                    lowerer.lower_additional_functions(refs, &mut lowered_modules[owner_index]);
                }
                pending_foreign_functions.extend(std::mem::take(
                    &mut lowerers[owner_index].foreign_function_refs,
                ));
                pending_foreign_instances.extend(std::mem::take(
                    &mut lowerers[owner_index].foreign_function_instance_refs,
                ));
                pending_foreign_global_instances.extend(std::mem::take(
                    &mut lowerers[owner_index].foreign_global_instance_refs,
                ));
                diagnostics.extend(std::mem::take(&mut lowerers[owner_index].diagnostics));
                optimization_report.changed_passes.extend(std::mem::take(
                    &mut lowerers[owner_index].optimization_report.changed_passes,
                ));
            }

            let mut instance_batches = (0..lowerers.len()).map(|_| Vec::new()).collect::<Vec<_>>();
            while let Some(instance) = pending_foreign_instances.pop_front() {
                if !queued_foreign_instances.insert(instance.key()) {
                    continue;
                }
                let Some(owner_index) = module_indices.get(&instance.def_id.module_id).copied()
                else {
                    continue;
                };
                instance_batches[owner_index].push(instance);
            }

            for (owner_index, refs) in instance_batches.into_iter().enumerate() {
                if refs.is_empty() {
                    continue;
                }
                let additional = {
                    let lowerer = &mut lowerers[owner_index];
                    lowerer.lower_additional_function_instances(
                        refs,
                        &lowered_modules[owner_index].functions,
                        &lowered_modules[owner_index].function_instances,
                    )
                };
                if !additional.is_empty() {
                    append_function_instances(
                        &mut lowerers[owner_index],
                        &mut lowered_modules[owner_index],
                        additional,
                    );
                    lowerers[owner_index].lower_additional_reachable_functions_from_instances(
                        &mut lowered_modules[owner_index],
                    );
                }
                pending_foreign_functions.extend(std::mem::take(
                    &mut lowerers[owner_index].foreign_function_refs,
                ));
                pending_foreign_instances.extend(std::mem::take(
                    &mut lowerers[owner_index].foreign_function_instance_refs,
                ));
                pending_foreign_global_instances.extend(std::mem::take(
                    &mut lowerers[owner_index].foreign_global_instance_refs,
                ));
                diagnostics.extend(std::mem::take(&mut lowerers[owner_index].diagnostics));
                optimization_report.changed_passes.extend(std::mem::take(
                    &mut lowerers[owner_index].optimization_report.changed_passes,
                ));
            }

            let mut global_instance_batches =
                (0..lowerers.len()).map(|_| Vec::new()).collect::<Vec<_>>();
            while let Some(instance) = pending_foreign_global_instances.pop_front() {
                if !queued_foreign_global_instances.insert(instance.key()) {
                    continue;
                }
                let Some(owner_index) = module_indices.get(&instance.def_id.module_id).copied()
                else {
                    continue;
                };
                global_instance_batches[owner_index].push(instance);
            }

            for (owner_index, refs) in global_instance_batches.into_iter().enumerate() {
                if refs.is_empty() {
                    continue;
                }
                {
                    let lowerer = &mut lowerers[owner_index];
                    lowerer
                        .lower_additional_global_instances(refs, &mut lowered_modules[owner_index]);
                }
                pending_foreign_functions.extend(std::mem::take(
                    &mut lowerers[owner_index].foreign_function_refs,
                ));
                pending_foreign_instances.extend(std::mem::take(
                    &mut lowerers[owner_index].foreign_function_instance_refs,
                ));
                pending_foreign_global_instances.extend(std::mem::take(
                    &mut lowerers[owner_index].foreign_global_instance_refs,
                ));
                diagnostics.extend(std::mem::take(&mut lowerers[owner_index].diagnostics));
                optimization_report.changed_passes.extend(std::mem::take(
                    &mut lowerers[owner_index].optimization_report.changed_passes,
                ));
            }
        }
    });

    BackendLowering {
        program: BackendProgram {
            modules: lowered_modules,
        },
        optimization,
        optimization_report,
        diagnostics,
    }
}

fn time_backend_stage<T>(enabled: bool, name: &str, f: impl FnOnce() -> T) -> T {
    nia_timing::time_detail(enabled, name, f)
}

fn append_function_instances(
    lowerer: &mut ModuleLowerer<'_>,
    module: &mut BackendModule,
    instances: Vec<BackendFunctionInstance>,
) {
    module.function_instances.extend(instances);
    lowerer.extend_struct_instances_from_functions(
        &mut module.struct_instances,
        &mut module.union_instances,
        &module.functions,
        &module.function_instances,
    );
    let mut backend_layouts =
        BackendLayouts::from_module_layouts(lowerer.input.module_id, lowerer.input.layouts);
    lowerer.extend_backend_layouts_for_instances(
        &mut backend_layouts,
        &module.struct_instances,
        &module.union_instances,
    );
    module.layouts = backend_layouts;
    module.interner = lowerer.type_context.interner.clone();
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
    shared: &'a BackendLowerShared,
    pub(crate) monomorphization: &'a Monomorphization,
    pub(crate) optimization: OptimizationPolicy,
    pub(crate) type_context: type_context::BackendTypeContext<'a, 'a>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    optimization_report: BackendOptimizationReport,
    missing_array_len_diagnostics: HashSet<GlobalConstExprId>,
    extension_generics_by_method: HashMap<GlobalDefId, Vec<SymbolId>>,
    extension_method_sources_by_def: HashMap<GlobalDefId, ExtensionMethodSource>,
    trait_context: trait_context::BackendTraitContext,
    instantiation: instantiation_context::BackendInstantiationContext<'a>,
    foreign_function_refs: Vec<GlobalDefId>,
    foreign_function_instance_refs: Vec<function_refs::FunctionInstanceRef>,
    foreign_global_instance_refs: Vec<function_refs::GlobalInstanceRef>,
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
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExtensionTraitMethodCandidate {
    target_ty: InternedTyId,
    method_def_id: GlobalDefId,
    trait_args: Vec<InternedTyId>,
    where_predicates: Vec<WherePredicateSignature>,
    effective_generics: Vec<SymbolId>,
    interner: Arc<nia_ty::TyInterner>,
}

#[derive(Debug, Clone)]
pub(crate) struct ExtensionMethodSource {
    target_ty: InternedTyId,
    where_predicates: Vec<WherePredicateSignature>,
    interner: nia_ty::TyInterner,
}

pub(crate) struct BackendLowerShared {
    program_extension_generics_by_method: HashMap<GlobalDefId, Vec<SymbolId>>,
    program_extension_method_sources_by_def: HashMap<GlobalDefId, ExtensionMethodSource>,
    program_trait_impls_by_method: HashMap<GlobalDefId, usize>,
    program_extension_trait_method_candidates:
        HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>>,
    program_trait_methods_with_defaults: HashSet<GlobalDefId>,
    program_method_symbols_by_def: HashMap<GlobalDefId, SymbolId>,
    input_type_interners: HashMap<TyInternerId, nia_ty::TyInterner>,
}

impl BackendLowerShared {
    fn new(modules: &[BackendLowerModuleInput<'_>]) -> Self {
        let first = modules.first();
        Self {
            program_extension_generics_by_method: first
                .map(|input| index_extension_generics_by_method(input.program_extension_methods))
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
            input_type_interners: index_input_type_interner_snapshots(modules),
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
struct ReachabilityWorklist {
    pending_functions: VecDeque<GlobalDefId>,
    queued_functions: HashSet<GlobalDefId>,
    pending_instances: Vec<FunctionInstanceRef>,
    queued_instances: HashSet<FunctionInstanceKey>,
    pending_global_instances: Vec<GlobalInstanceRef>,
    queued_global_instances: HashSet<GlobalInstanceKey>,
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
            nia_static_ir::StaticInit::Array(elems) => {
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
            nia_static_ir::StaticInit::AddrOfFunction { args, .. } => {
                for arg in args {
                    self.add_ty(lowerer, *arg);
                }
            }
            nia_static_ir::StaticInit::StaticArrayPointer {
                array_ty,
                array_init,
            } => {
                self.add_ty(lowerer, *array_ty);
                self.add_static_init(lowerer, array_init);
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
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::VolatilePointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem })
            | Some(TyKind::Optional { elem })
            | Some(TyKind::Array { elem, .. }) => self.add_ty(lowerer, elem),
            Some(TyKind::Range { bound, .. }) => {
                if let Some(bound) = bound {
                    self.add_ty(lowerer, bound);
                }
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
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
            Some(TyKind::Nominal { def_id, args, .. }) => {
                self.add_struct(def_id);
                self.add_union(def_id);
                for arg in args {
                    self.add_ty(lowerer, arg);
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
                associated_type_bindings,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_args,
                associated_type_bindings,
                ..
            }) => {
                for arg in trait_args {
                    self.add_ty(lowerer, arg);
                }
                for binding in associated_type_bindings {
                    for arg in binding.trait_args {
                        self.add_ty(lowerer, arg);
                    }
                    self.add_ty(lowerer, binding.ty);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.add_ty(lowerer, self_ty);
                for arg in trait_args {
                    self.add_ty(lowerer, arg);
                }
            }
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::GenericParam(_)
                | TyKind::SelfParam
                | TyKind::BuiltinType(_)
                | TyKind::Primitive(_)
                | TyKind::Vector { .. },
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

    fn enqueue_refs(&mut self, refs: FunctionRefs) {
        for function in refs.functions {
            self.enqueue_function(function);
        }
        self.enqueue_instances(refs.instances);
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
                    arg_interner: None,
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
    (input.program_defs)(def_id.module_id)?
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

impl<'a> ModuleLowerer<'a> {
    fn new(
        input: &'a BackendLowerModuleInput<'a>,
        monomorphization: &'a Monomorphization,
        optimization: OptimizationPolicy,
        shared: &'a BackendLowerShared,
        timing: bool,
    ) -> Self {
        let type_context =
            time_backend_stage(timing, "backend_lower.new_lowerer.type_context", || {
                type_context::BackendTypeContext::new(input, shared)
            });
        let extension_generics_by_method = time_backend_stage(
            timing,
            "backend_lower.new_lowerer.local_extension_generics",
            || index_local_extension_generics_by_method(input.extensions),
        );
        let extension_method_sources_by_def = time_backend_stage(
            timing,
            "backend_lower.new_lowerer.local_extension_sources",
            || index_local_extension_method_sources_by_def(input),
        );
        let trait_context =
            time_backend_stage(timing, "backend_lower.new_lowerer.trait_context", || {
                trait_context::BackendTraitContext::new(input)
            });
        let struct_layout_instances_by_def = time_backend_stage(
            timing,
            "backend_lower.new_lowerer.struct_layout_instances",
            || index_layout_instances_by_def(input.layouts.struct_instances.keys()),
        );
        let union_layout_instances_by_def = time_backend_stage(
            timing,
            "backend_lower.new_lowerer.union_layout_instances",
            || index_layout_instances_by_def(input.layouts.union_instances.keys()),
        );
        Self {
            input,
            shared,
            monomorphization,
            optimization,
            type_context,
            diagnostics: Vec::new(),
            optimization_report: BackendOptimizationReport::default(),
            missing_array_len_diagnostics: HashSet::new(),
            extension_generics_by_method,
            extension_method_sources_by_def,
            trait_context,
            instantiation: instantiation_context::BackendInstantiationContext::default(),
            foreign_function_refs: Vec::new(),
            foreign_function_instance_refs: Vec::new(),
            foreign_global_instance_refs: Vec::new(),
            struct_layout_instances_by_def,
            union_layout_instances_by_def,
            effective_generics: HashMap::new(),
            def_names: HashMap::new(),
            function_sources: HashMap::new(),
            aggregate_sources: HashMap::new(),
        }
    }

    pub(crate) fn trait_impl_index_for_method(&self, def_id: GlobalDefId) -> Option<usize> {
        self.trait_context
            .trait_impls_by_method
            .get(&def_id)
            .copied()
            .or_else(|| {
                self.shared
                    .program_trait_impls_by_method
                    .get(&def_id)
                    .copied()
            })
    }

    pub(crate) fn extension_method_source(
        &self,
        def_id: GlobalDefId,
    ) -> Option<&ExtensionMethodSource> {
        self.extension_method_sources_by_def
            .get(&def_id)
            .or_else(|| {
                self.shared
                    .program_extension_method_sources_by_def
                    .get(&def_id)
            })
    }

    pub(crate) fn method_symbol_for_def(&self, def_id: GlobalDefId) -> Option<SymbolId> {
        self.trait_context
            .method_symbols_by_def
            .get(&def_id)
            .or_else(|| self.shared.program_method_symbols_by_def.get(&def_id))
            .copied()
    }

    fn lower_module(&mut self) -> BackendModule {
        let mut structs = Vec::new();
        let mut unions = Vec::new();
        let mut struct_instances = Vec::new();
        let mut union_instances = Vec::new();
        let mut enums = Vec::new();
        let mut globals = Vec::new();
        let mut global_instances = Vec::new();
        let mut functions = Vec::new();
        let mut function_templates = Vec::new();
        let mut worklist = ReachabilityWorklist::default();
        let mut trait_object_vtables = Vec::new();

        for item in &self.input.active_item_tree.items {
            match &item.kind {
                ItemTreeNodeKind::Struct(item_struct) => {
                    self.index_aggregate_source(&item.node_key, item.span, item_struct);
                    if !matches!(
                        self.input.roots,
                        BackendFunctionRoots::EntryPoints | BackendFunctionRoots::NoFunctions
                    ) {
                        if item_struct.generics.is_empty()
                            && let Some(item) =
                                self.lower_struct(&item.node_key, item.span, item_struct)
                        {
                            structs.push(item);
                        }
                        struct_instances.extend(self.lower_struct_instances(
                            &item.node_key,
                            item.span,
                            item_struct,
                        ));
                    }
                }
                ItemTreeNodeKind::Union(item_union) => {
                    self.index_union_source(&item.node_key, item.span, item_union);
                    if !matches!(
                        self.input.roots,
                        BackendFunctionRoots::EntryPoints | BackendFunctionRoots::NoFunctions
                    ) {
                        if item_union.generics.is_empty()
                            && let Some(item) =
                                self.lower_union(&item.node_key, item.span, item_union)
                        {
                            unions.push(item);
                        }
                        union_instances.extend(self.lower_union_instances(
                            &item.node_key,
                            item.span,
                            item_union,
                        ));
                    }
                }
                ItemTreeNodeKind::Trait(item_trait) => {
                    for method in &item_trait.methods {
                        if method.function.body.is_none() {
                            continue;
                        }
                        self.index_function_source(
                            method.function.span,
                            &method.function,
                            &mut worklist,
                        );
                        self.lower_function_local_static_globals(
                            &method.function,
                            &mut globals,
                            &mut worklist,
                        );
                    }
                }
                ItemTreeNodeKind::Extend(extend) => {
                    for method in &extend.methods {
                        if method.function.body.is_none() {
                            continue;
                        }
                        self.index_function_source(
                            method.function.span,
                            &method.function,
                            &mut worklist,
                        );
                        self.lower_function_local_static_globals(
                            &method.function,
                            &mut globals,
                            &mut worklist,
                        );
                    }
                }
                ItemTreeNodeKind::Enum(item_enum) => {
                    if let Some(item) = self.lower_enum(&item.node_key, item.span, item_enum) {
                        enums.push(item);
                    }
                }
                ItemTreeNodeKind::Function(function) => {
                    self.index_function_source(item.span, function, &mut worklist);
                    self.lower_function_local_static_globals(function, &mut globals, &mut worklist);
                }
                ItemTreeNodeKind::Binding(binding) => {
                    if binding.is_const() {
                        continue;
                    }
                    self.lower_static_global_binding(
                        item.span,
                        binding,
                        &mut globals,
                        &mut worklist,
                    );
                }
                ItemTreeNodeKind::Module(_)
                | ItemTreeNodeKind::Using(_)
                | ItemTreeNodeKind::TypeAlias(_) => {}
            }
        }

        worklist.enqueue_instances(self.initial_monomorphized_function_instance_refs());
        self.lower_reachable_function_closure(
            &mut functions,
            &mut worklist,
            &mut trait_object_vtables,
        );
        self.lower_missing_body_ir_static_globals(&mut globals, &mut worklist);
        let mut function_instances = Vec::new();
        self.lower_reachable_instances_and_vtables(
            &mut functions,
            &mut function_templates,
            &mut function_instances,
            &mut global_instances,
            &mut worklist,
            &mut trait_object_vtables,
        );
        self.devirtualize_direct_trait_calls(&mut functions, &mut function_instances);
        self.propagate_cross_function_constants(&mut functions, &mut function_instances);
        self.inline_leaf_functions(&mut functions, &mut function_instances);
        self.complete_reachable_backend_items(
            &mut functions,
            &mut function_templates,
            &mut function_instances,
            &mut global_instances,
            &mut trait_object_vtables,
        );
        self.remove_unused_private_functions(
            &mut functions,
            &mut function_instances,
            &globals,
            &trait_object_vtables,
        );
        self.extend_struct_instances_from_functions(
            &mut struct_instances,
            &mut union_instances,
            &functions,
            &function_instances,
        );
        self.complete_reachable_aggregates(
            &mut structs,
            &mut unions,
            ReachableAggregateInputs {
                globals: &globals,
                functions: &functions,
                function_instances: &function_instances,
                struct_instances: &struct_instances,
                union_instances: &union_instances,
                trait_object_vtables: &trait_object_vtables,
            },
        );

        let mut backend_layouts =
            BackendLayouts::from_module_layouts(self.input.module_id, self.input.layouts);
        self.extend_backend_layouts_for_instances(
            &mut backend_layouts,
            &struct_instances,
            &union_instances,
        );

        BackendModule {
            id: self.input.module_id,
            name: self.input.module_name.clone(),
            interner: self.type_context.interner.clone(),
            const_eval: nia_backend_ir::BackendConstFacts {
                array_lengths: self.input.const_array_lengths.values.clone(),
            },
            layouts: backend_layouts,
            structs,
            unions,
            struct_instances,
            union_instances,
            enums,
            globals,
            global_instances,
            functions,
            function_instances,
            trait_object_vtables,
            generic_instantiations: self
                .input
                .semantic_facts
                .iter_generic_instantiations()
                .map(|inst| nia_backend_ir::BackendGenericInstantiation {
                    def_id: inst.def_id,
                    arg_module_id: self.input.module_id,
                    self_arg: inst.self_arg,
                    args: inst.args.clone(),
                    const_args: inst.const_args.clone(),
                    span: inst.span,
                    source_def_id: inst.source_def_id,
                })
                .collect(),
        }
    }

    fn lower_function_local_static_globals(
        &mut self,
        function: &nia_ast::FunctionItem,
        globals: &mut Vec<BackendGlobal>,
        worklist: &mut ReachabilityWorklist,
    ) {
        let Some(body) = &function.body else {
            return;
        };
        let owner_has_effective_generics = self
            .def_id_for_node_any_function(&function.node_key)
            .map(|def_id| {
                let global_def_id = self.global_def_id(def_id);
                !self
                    .effective_generics(global_def_id, &generic_param_names(&function.generics))
                    .is_empty()
            })
            .unwrap_or(false);
        self.lower_block_static_globals(body, owner_has_effective_generics, globals, worklist);
    }

    fn lower_block_static_globals(
        &mut self,
        block: &Block,
        owner_has_effective_generics: bool,
        globals: &mut Vec<BackendGlobal>,
        worklist: &mut ReachabilityWorklist,
    ) {
        for stmt in &block.stmts {
            match &stmt.kind {
                StmtKind::Static(binding) => {
                    self.lower_local_static_global_binding(
                        stmt.span,
                        binding,
                        owner_has_effective_generics,
                        globals,
                        worklist,
                    );
                }
                StmtKind::ForIn(for_stmt) => {
                    self.lower_block_static_globals(
                        &for_stmt.body,
                        owner_has_effective_generics,
                        globals,
                        worklist,
                    );
                }
                StmtKind::While(while_stmt) => {
                    self.lower_block_static_globals(
                        &while_stmt.body,
                        owner_has_effective_generics,
                        globals,
                        worklist,
                    );
                }
                StmtKind::Loop(loop_stmt) => {
                    self.lower_block_static_globals(
                        &loop_stmt.body,
                        owner_has_effective_generics,
                        globals,
                        worklist,
                    );
                }
                StmtKind::Binding(_)
                | StmtKind::Using(_)
                | StmtKind::Expr(_)
                | StmtKind::Return(_)
                | StmtKind::Break
                | StmtKind::Continue
                | StmtKind::Defer(_) => {}
            }
        }
    }

    fn lower_local_static_global_binding(
        &mut self,
        span: nia_span::Span,
        binding: &BindingItem,
        owner_has_effective_generics: bool,
        globals: &mut Vec<BackendGlobal>,
        worklist: &mut ReachabilityWorklist,
    ) {
        let Some(global_def_id) = self
            .def_id_for_node(&binding.node_key, DefKind::Global)
            .map(|def_id| self.global_def_id(def_id))
        else {
            return;
        };
        if owner_has_effective_generics
            && !self.input.body_ir.global_inits.contains_key(&global_def_id)
        {
            return;
        }
        self.lower_static_global_binding(span, binding, globals, worklist);
    }

    fn lower_missing_body_ir_static_globals(
        &mut self,
        globals: &mut Vec<BackendGlobal>,
        worklist: &mut ReachabilityWorklist,
    ) {
        let mut seen = globals
            .iter()
            .map(|global| global.def_id)
            .collect::<HashSet<_>>();
        let mut pending = self
            .input
            .body_ir
            .global_inits
            .keys()
            .copied()
            .collect::<Vec<_>>();
        pending.sort_by_key(|def_id| def_id.def_id);
        for global_def_id in pending {
            if global_def_id.module_id != self.input.module_id || !seen.insert(global_def_id) {
                continue;
            }
            let Some(global) = self.lower_global_from_body_ir(global_def_id) else {
                continue;
            };
            if let Some(init) = &global.init {
                let mut refs = FunctionRefs::default();
                function_refs::collect_function_refs_from_static_init(
                    self.input.module_id,
                    init,
                    &mut refs,
                );
                worklist.enqueue_refs(refs);
            }
            globals.push(global);
        }
    }

    fn lower_static_global_binding(
        &mut self,
        span: nia_span::Span,
        binding: &BindingItem,
        globals: &mut Vec<BackendGlobal>,
        worklist: &mut ReachabilityWorklist,
    ) {
        let Some(global_def_id) = self
            .def_id_for_node(&binding.node_key, DefKind::Global)
            .map(|def_id| self.global_def_id(def_id))
        else {
            return;
        };
        if !self.is_backend_global_reachable(global_def_id) {
            return;
        }
        if let Some(global) = self.lower_global(&binding.node_key, span, binding) {
            if let Some(init) = &global.init {
                let mut refs = FunctionRefs::default();
                function_refs::collect_function_refs_from_static_init(
                    self.input.module_id,
                    init,
                    &mut refs,
                );
                worklist.enqueue_refs(refs);
            }
            globals.push(global);
        }
    }

    fn index_function_source(
        &mut self,
        span: nia_span::Span,
        function: &'a nia_ast::FunctionItem,
        worklist: &mut ReachabilityWorklist,
    ) -> Option<GlobalDefId> {
        let def_id = self.def_id_for_node_any_function(&function.node_key)?;
        let global_def_id = self.global_def_id(def_id);
        self.function_sources
            .insert(global_def_id, BackendFunctionSource { span, function });
        if self.is_backend_function_root(global_def_id, function) {
            worklist.enqueue_function(global_def_id);
        }
        Some(global_def_id)
    }

    fn index_aggregate_source(
        &mut self,
        node_key: &'a VersionedNodeKey,
        span: nia_span::Span,
        item: &'a nia_ast::StructItem,
    ) -> Option<GlobalDefId> {
        let def_id = self.def_id_for_node(node_key, DefKind::Struct)?;
        let global_def_id = self.global_def_id(def_id);
        self.aggregate_sources.insert(
            global_def_id,
            BackendAggregateSource::Struct {
                node_key,
                span,
                item,
            },
        );
        Some(global_def_id)
    }

    fn index_union_source(
        &mut self,
        node_key: &'a VersionedNodeKey,
        span: nia_span::Span,
        item: &'a nia_ast::UnionItem,
    ) -> Option<GlobalDefId> {
        let def_id = self.def_id_for_node(node_key, DefKind::Union)?;
        let global_def_id = self.global_def_id(def_id);
        self.aggregate_sources.insert(
            global_def_id,
            BackendAggregateSource::Union {
                node_key,
                span,
                item,
            },
        );
        Some(global_def_id)
    }

    fn is_backend_function_root(
        &mut self,
        def_id: GlobalDefId,
        function: &nia_ast::FunctionItem,
    ) -> bool {
        if self.input.roots == BackendFunctionRoots::NoFunctions {
            return false;
        }
        let Some(def) = self.input.defs.defs.get(def_id.def_id) else {
            return false;
        };
        if def.kind == DefKind::TraitMethod {
            return false;
        }
        if self.input.roots == BackendFunctionRoots::FunctionBodies {
            return function.is_extern
                || (self.input.function_bodies.contains_key(&def_id)
                    && !self
                        .has_effective_generics(def_id, &generic_param_names(&function.generics)));
        }
        if function.is_const
            || function.is_extern
            || def.name == known::MAIN
            || def.name == known::START_ENTRY
        {
            return true;
        }
        if self.input.roots == BackendFunctionRoots::EntryPoints {
            return false;
        }
        def.visibility != Visibility::Private
    }

    fn is_backend_global_reachable(&self, def_id: GlobalDefId) -> bool {
        if self.input.roots == BackendFunctionRoots::NoFunctions {
            return false;
        }
        match self.input.reachable_globals {
            Some(globals) if self.input.roots == BackendFunctionRoots::EntryPoints => {
                globals.contains(&def_id)
            }
            _ => true,
        }
    }

    fn is_backend_struct_reachable(&self, def_id: GlobalDefId) -> bool {
        match self.input.reachable_structs {
            Some(structs)
                if matches!(
                    self.input.roots,
                    BackendFunctionRoots::EntryPoints | BackendFunctionRoots::NoFunctions
                ) =>
            {
                structs.contains(&def_id)
            }
            _ => true,
        }
    }

    fn is_backend_union_reachable(&self, def_id: GlobalDefId) -> bool {
        match self.input.reachable_unions {
            Some(unions)
                if matches!(
                    self.input.roots,
                    BackendFunctionRoots::EntryPoints | BackendFunctionRoots::NoFunctions
                ) =>
            {
                unions.contains(&def_id)
            }
            _ => true,
        }
    }

    fn complete_reachable_aggregates(
        &mut self,
        structs: &mut Vec<BackendStruct>,
        unions: &mut Vec<BackendUnion>,
        input: ReachableAggregateInputs<'_>,
    ) {
        if !matches!(
            self.input.roots,
            BackendFunctionRoots::EntryPoints | BackendFunctionRoots::NoFunctions
        ) {
            return;
        }
        let mut roots = ReachableAggregateRoots::default();
        for global in input.globals {
            roots.add_ty(self, global.ty);
            if let Some(init) = &global.init {
                roots.add_static_init(self, init);
            }
        }
        for function in input.functions {
            roots.add_backend_function(self, function);
        }
        for instance in input.function_instances {
            roots.add_backend_function_instance(self, instance);
        }
        for instance in input.struct_instances {
            roots.add_struct(instance.def_id);
            for arg in &instance.args {
                roots.add_ty(self, *arg);
            }
            for field in &instance.fields {
                roots.add_ty(self, field.ty);
            }
        }
        for instance in input.union_instances {
            roots.add_union(instance.def_id);
            for arg in &instance.args {
                roots.add_ty(self, *arg);
            }
            for field in &instance.fields {
                roots.add_ty(self, field.ty);
            }
        }
        for vtable in input.trait_object_vtables {
            roots.add_ty(self, vtable.key.self_ty);
            roots.add_ty(self, vtable.key.object_ty);
            for arg in &vtable.trait_args {
                roots.add_ty(self, *arg);
            }
            for entry in &vtable.entries {
                if let BackendTraitObjectVtableFunction::FunctionInstance { args, .. } =
                    &entry.function
                {
                    for arg in args {
                        roots.add_ty(self, *arg);
                    }
                }
            }
        }
        if let Some(reachable_structs) = self.input.reachable_structs {
            for def_id in reachable_structs {
                roots.add_struct(*def_id);
            }
        }
        if let Some(reachable_unions) = self.input.reachable_unions {
            for def_id in reachable_unions {
                roots.add_union(*def_id);
            }
        }

        let mut seen_structs = structs
            .iter()
            .map(|item| item.def_id)
            .collect::<HashSet<_>>();
        for def_id in roots.structs {
            if def_id.module_id != self.input.module_id
                || !self.is_backend_struct_reachable(def_id)
                || !seen_structs.insert(def_id)
            {
                continue;
            }
            let Some(BackendAggregateSource::Struct {
                node_key,
                span,
                item,
            }) = self.aggregate_sources.get(&def_id).copied()
            else {
                continue;
            };
            if item.generics.is_empty()
                && let Some(item) = self.lower_struct(node_key, span, item)
            {
                structs.push(item);
            }
        }

        let mut seen_unions = unions
            .iter()
            .map(|item| item.def_id)
            .collect::<HashSet<_>>();
        for def_id in roots.unions {
            if def_id.module_id != self.input.module_id
                || !self.is_backend_union_reachable(def_id)
                || !seen_unions.insert(def_id)
            {
                continue;
            }
            let Some(BackendAggregateSource::Union {
                node_key,
                span,
                item,
            }) = self.aggregate_sources.get(&def_id).copied()
            else {
                continue;
            };
            if item.generics.is_empty()
                && let Some(item) = self.lower_union(node_key, span, item)
            {
                unions.push(item);
            }
        }
    }

    fn has_effective_generics(&mut self, def_id: GlobalDefId, own_generics: &[SymbolId]) -> bool {
        !self.effective_generics(def_id, own_generics).is_empty()
    }

    fn lower_reachable_function_closure(
        &mut self,
        functions: &mut Vec<BackendFunction>,
        worklist: &mut ReachabilityWorklist,
        trait_object_vtables: &mut Vec<BackendTraitObjectVtable>,
    ) -> bool {
        let mut changed = false;
        let mut lowered = functions
            .iter()
            .map(|function| function.def_id)
            .collect::<HashSet<_>>();
        while let Some(def_id) = worklist.pending_functions.pop_front() {
            if def_id.module_id != self.input.module_id {
                self.foreign_function_refs.push(def_id);
                continue;
            }
            if lowered.contains(&def_id) {
                continue;
            }
            if self
                .input
                .defs
                .defs
                .get(def_id.def_id)
                .is_some_and(|def| def.kind == DefKind::TraitMethod)
            {
                continue;
            }
            let Some(source) = self.function_sources.get(&def_id).copied() else {
                continue;
            };
            let Some(function) = self.lower_function(source.span, source.function) else {
                continue;
            };
            lowered.insert(def_id);
            if function.generics.is_empty() {
                let mut refs = FunctionRefs::default();
                function_refs::collect_function_refs_from_optional_body(
                    self.input.module_id,
                    &function.function_body,
                    &mut refs,
                );
                worklist.enqueue_refs(refs);
                functions.push(function);
                changed = true;
            }
        }
        changed |=
            self.collect_new_trait_object_vtables(trait_object_vtables, functions, &[], worklist);
        changed
    }

    fn lower_additional_functions(&mut self, refs: Vec<GlobalDefId>, module: &mut BackendModule) {
        let mut worklist = ReachabilityWorklist {
            pending_functions: VecDeque::new(),
            queued_functions: module
                .functions
                .iter()
                .map(|function| function.def_id)
                .collect::<HashSet<_>>(),
            pending_instances: Vec::new(),
            queued_instances: module
                .function_instances
                .iter()
                .map(FunctionInstanceKey::from)
                .collect::<HashSet<_>>(),
            pending_global_instances: Vec::new(),
            queued_global_instances: module
                .global_instances
                .iter()
                .map(|instance| GlobalInstanceKey {
                    def_id: instance.def_id,
                    arg_module_id: instance.arg_module_id,
                    args: instance.args.clone(),
                    const_args: instance.const_args.clone(),
                })
                .collect::<HashSet<_>>(),
        };
        for def_id in refs {
            worklist.enqueue_function(def_id);
        }
        let mut function_templates = Vec::new();
        self.complete_reachable_backend_items(
            &mut module.functions,
            &mut function_templates,
            &mut module.function_instances,
            &mut module.global_instances,
            &mut module.trait_object_vtables,
        );
        self.lower_reachable_instances_and_vtables(
            &mut module.functions,
            &mut function_templates,
            &mut module.function_instances,
            &mut module.global_instances,
            &mut worklist,
            &mut module.trait_object_vtables,
        );
        self.extend_struct_instances_from_functions(
            &mut module.struct_instances,
            &mut module.union_instances,
            &module.functions,
            &module.function_instances,
        );
        let mut backend_layouts =
            BackendLayouts::from_module_layouts(self.input.module_id, self.input.layouts);
        self.extend_backend_layouts_for_instances(
            &mut backend_layouts,
            &module.struct_instances,
            &module.union_instances,
        );
        module.layouts = backend_layouts;
        module.interner = self.type_context.interner.clone();
    }

    fn lower_additional_global_instances(
        &mut self,
        refs: Vec<GlobalInstanceRef>,
        module: &mut BackendModule,
    ) {
        let additional = self.lower_global_instances_from_refs(refs, &module.global_instances);
        if additional.is_empty() {
            return;
        }
        module.global_instances.extend(additional);
        let mut backend_layouts =
            BackendLayouts::from_module_layouts(self.input.module_id, self.input.layouts);
        self.extend_backend_layouts_for_instances(
            &mut backend_layouts,
            &module.struct_instances,
            &module.union_instances,
        );
        module.layouts = backend_layouts;
        module.interner = self.type_context.interner.clone();
    }

    fn lower_reachable_instances_and_vtables(
        &mut self,
        functions: &mut Vec<BackendFunction>,
        function_templates: &mut Vec<BackendFunction>,
        function_instances: &mut Vec<BackendFunctionInstance>,
        global_instances: &mut Vec<BackendGlobalInstance>,
        worklist: &mut ReachabilityWorklist,
        trait_object_vtables: &mut Vec<BackendTraitObjectVtable>,
    ) {
        loop {
            let mut changed =
                self.lower_reachable_function_closure(functions, worklist, trait_object_vtables);
            if !worklist.pending_instances.is_empty() {
                self.lower_pending_instance_templates(
                    function_templates,
                    &worklist.pending_instances,
                );
                let refs = std::mem::take(&mut worklist.pending_instances);
                let additional = self.lower_additional_function_instances(
                    refs,
                    function_templates,
                    function_instances,
                );
                for instance in &additional {
                    let mut refs = FunctionRefs::default();
                    function_refs::collect_function_refs_from_optional_body(
                        instance.arg_module_id,
                        &instance.function_body,
                        &mut refs,
                    );
                    worklist.enqueue_refs(refs);
                }
                changed |= !additional.is_empty();
                function_instances.extend(additional);
            }
            if !worklist.pending_global_instances.is_empty() {
                let refs = std::mem::take(&mut worklist.pending_global_instances);
                let additional =
                    self.lower_global_instances_from_refs(refs, global_instances.as_slice());
                for instance in &additional {
                    if let Some(init) = &instance.init {
                        let mut refs = FunctionRefs::default();
                        function_refs::collect_function_refs_from_static_init(
                            instance.arg_module_id,
                            init,
                            &mut refs,
                        );
                        worklist.enqueue_refs(refs);
                    }
                }
                changed |= !additional.is_empty();
                global_instances.extend(additional);
            }
            changed |= self.collect_new_trait_object_vtables(
                trait_object_vtables,
                functions,
                function_instances,
                worklist,
            );
            if !changed
                && worklist.pending_functions.is_empty()
                && worklist.pending_instances.is_empty()
                && worklist.pending_global_instances.is_empty()
            {
                break;
            }
        }
    }

    fn lower_pending_instance_templates(
        &mut self,
        function_templates: &mut Vec<BackendFunction>,
        pending_instances: &[FunctionInstanceRef],
    ) {
        let mut known = function_templates
            .iter()
            .map(|function| function.def_id)
            .collect::<HashSet<_>>();
        for instance in pending_instances {
            if !known.insert(instance.def_id) {
                continue;
            }
            let function = if instance.def_id.module_id == self.input.module_id {
                self.function_sources
                    .get(&instance.def_id)
                    .copied()
                    .and_then(|source| self.lower_function(source.span, source.function))
            } else {
                self.backend_function_template_for_program_def(instance.def_id)
            };
            if let Some(function) = function {
                function_templates.push(function);
            }
        }
    }

    fn lower_global_instances_from_refs(
        &mut self,
        refs: Vec<GlobalInstanceRef>,
        existing: &[BackendGlobalInstance],
    ) -> Vec<BackendGlobalInstance> {
        let mut instances = Vec::new();
        let mut seen = existing
            .iter()
            .map(|instance| BackendGlobalInstanceKey {
                def_id: instance.def_id,
                arg_module_id: instance.arg_module_id,
                args: self.canonicalize_instance_args(&instance.args),
                const_args: instance.const_args.clone(),
            })
            .collect::<HashSet<_>>();
        for instance in refs {
            if instance.def_id.module_id != self.input.module_id {
                self.foreign_global_instance_refs
                    .push(self.with_current_arg_interner_global(instance));
                continue;
            }
            let args = self.canonicalize_global_instance_ref_args(&instance);
            let const_args = instance.const_args.clone();
            let key = BackendGlobalInstanceKey {
                def_id: instance.def_id,
                arg_module_id: instance.arg_module_id,
                args: args.clone(),
                const_args: const_args.clone(),
            };
            if !seen.insert(key) {
                continue;
            }
            if args.iter().any(|arg| {
                self.cached_ty_contains_generic_param(*arg)
                    || self.cached_ty_contains_unresolved_projection(*arg)
                    || self.cached_ty_contains_error(*arg)
            }) {
                continue;
            }
            let Some(global) = self.lower_planned_global_instance(
                instance.def_id,
                instance.arg_module_id,
                args,
                const_args,
            ) else {
                continue;
            };
            instances.push(global);
        }
        instances
    }

    fn lower_planned_global_instance(
        &mut self,
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        args: Vec<InternedTyId>,
        const_args: Vec<nia_ty::ConstGenericArg>,
    ) -> Option<BackendGlobalInstance> {
        let signature = self.input.signatures.globals.get(&def_id.def_id)?;
        if signature.is_extern {
            return None;
        }
        let def = self.input.defs.defs.get(def_id.def_id)?;
        let owner = def.parent?;
        let owner_def_id = GlobalDefId {
            module_id: def_id.module_id,
            def_id: owner,
        };
        let owner_generics = if owner_def_id.module_id == self.input.module_id {
            self.input
                .signatures
                .functions
                .get(&owner)
                .map(|signature| signature.generics.as_slice())?
        } else {
            self.input
                .program_functions
                .get(&owner_def_id)
                .map(|signature| signature.signature.generics.as_slice())?
        };
        let effective_generics = self
            .effective_generics(owner_def_id, owner_generics)
            .to_vec();
        let imported_args = args
            .iter()
            .map(|arg| self.import_instance_arg_type(*arg))
            .collect::<Vec<_>>();
        let substitutions =
            ModuleLowerer::generic_substitutions(&effective_generics, &imported_args);
        let substitutions = self.intern_type_substitutions(&substitutions);
        let ty = self
            .input
            .semantic_facts
            .global_types
            .get(&def_id)
            .copied()
            .or(signature.explicit_type)
            .map(|ty| self.instantiate_ty_with_id(ty, substitutions))?;
        let init = self
            .input
            .body_ir
            .global_inits
            .get(&def_id)
            .cloned()
            .map(|init| self.instantiate_static_init(init, substitutions))
            .map(|init| self.optimize_static_init(def_id, init));
        Some(BackendGlobalInstance {
            def_id,
            name: def.name,
            arg_module_id,
            args: args.clone(),
            const_args: const_args.clone(),
            symbol: self.mangle_instance_symbol(def_id, def.name, None, &args, &const_args),
            ty,
            is_let: !signature.is_mutable,
            init,
            span: def.span,
        })
    }

    fn instantiate_static_init(
        &mut self,
        init: nia_static_ir::StaticInit,
        substitutions: TypeSubstitutionId,
    ) -> nia_static_ir::StaticInit {
        match init {
            nia_static_ir::StaticInit::Array(elems) => nia_static_ir::StaticInit::Array(
                elems
                    .into_iter()
                    .map(|elem| self.instantiate_static_init(elem, substitutions))
                    .collect(),
            ),
            nia_static_ir::StaticInit::Repeat { value, count } => {
                nia_static_ir::StaticInit::Repeat {
                    value: Box::new(self.instantiate_static_init(*value, substitutions)),
                    count,
                }
            }
            nia_static_ir::StaticInit::Struct(fields) => nia_static_ir::StaticInit::Struct(
                fields
                    .into_iter()
                    .map(|field| nia_static_ir::StaticFieldInit {
                        field: field.field,
                        value: self.instantiate_static_init(field.value, substitutions),
                    })
                    .collect(),
            ),
            nia_static_ir::StaticInit::AddrOfFunction { function, args } => {
                nia_static_ir::StaticInit::AddrOfFunction {
                    function,
                    args: args
                        .into_iter()
                        .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
                        .collect(),
                }
            }
            nia_static_ir::StaticInit::StaticArrayPointer {
                array_ty,
                array_init,
            } => nia_static_ir::StaticInit::StaticArrayPointer {
                array_ty: self.instantiate_ty_with_id(array_ty, substitutions),
                array_init: Box::new(self.instantiate_static_init(*array_init, substitutions)),
            },
            nia_static_ir::StaticInit::Zero
            | nia_static_ir::StaticInit::Int(_)
            | nia_static_ir::StaticInit::Float(_)
            | nia_static_ir::StaticInit::Bool(_)
            | nia_static_ir::StaticInit::Char(_)
            | nia_static_ir::StaticInit::Byte(_)
            | nia_static_ir::StaticInit::Chars(_)
            | nia_static_ir::StaticInit::Bytes(_)
            | nia_static_ir::StaticInit::NullPtr
            | nia_static_ir::StaticInit::AddrOfGlobal { .. } => init,
        }
    }

    fn lower_additional_reachable_functions_from_instances(&mut self, module: &mut BackendModule) {
        let mut function_templates = Vec::new();
        self.complete_reachable_backend_items(
            &mut module.functions,
            &mut function_templates,
            &mut module.function_instances,
            &mut module.global_instances,
            &mut module.trait_object_vtables,
        );
        self.extend_struct_instances_from_functions(
            &mut module.struct_instances,
            &mut module.union_instances,
            &module.functions,
            &module.function_instances,
        );
        let mut backend_layouts =
            BackendLayouts::from_module_layouts(self.input.module_id, self.input.layouts);
        self.extend_backend_layouts_for_instances(
            &mut backend_layouts,
            &module.struct_instances,
            &module.union_instances,
        );
        module.layouts = backend_layouts;
        module.interner = self.type_context.interner.clone();
    }

    fn complete_reachable_backend_items(
        &mut self,
        functions: &mut Vec<BackendFunction>,
        function_templates: &mut Vec<BackendFunction>,
        function_instances: &mut Vec<BackendFunctionInstance>,
        global_instances: &mut Vec<BackendGlobalInstance>,
        trait_object_vtables: &mut Vec<BackendTraitObjectVtable>,
    ) {
        loop {
            let mut refs = FunctionRefs::default();
            for function in functions.iter() {
                function_refs::collect_function_refs_from_optional_body(
                    self.input.module_id,
                    &function.function_body,
                    &mut refs,
                );
            }
            for instance in function_instances.iter() {
                function_refs::collect_function_refs_from_optional_body(
                    instance.arg_module_id,
                    &instance.function_body,
                    &mut refs,
                );
            }
            for instance in global_instances.iter() {
                if let Some(init) = &instance.init {
                    function_refs::collect_function_refs_from_static_init(
                        instance.arg_module_id,
                        init,
                        &mut refs,
                    );
                }
            }
            for vtable in trait_object_vtables.iter() {
                for entry in &vtable.entries {
                    match &entry.function {
                        BackendTraitObjectVtableFunction::Function(function) => {
                            refs.functions.insert(*function);
                        }
                        BackendTraitObjectVtableFunction::FunctionInstance {
                            def_id,
                            arg_module_id,
                            self_arg,
                            args,
                            const_args,
                        } => refs.instances.push(FunctionInstanceRef {
                            def_id: *def_id,
                            arg_module_id: *arg_module_id,
                            self_arg: *self_arg,
                            args: args.clone(),
                            const_args: const_args.clone(),
                            arg_interner: None,
                            span: vtable.span,
                        }),
                    }
                }
            }

            let mut worklist = ReachabilityWorklist {
                pending_functions: VecDeque::new(),
                queued_functions: functions
                    .iter()
                    .map(|function| function.def_id)
                    .collect::<HashSet<_>>(),
                pending_instances: Vec::new(),
                queued_instances: function_instances
                    .iter()
                    .map(FunctionInstanceKey::from)
                    .collect::<HashSet<_>>(),
                pending_global_instances: Vec::new(),
                queued_global_instances: global_instances
                    .iter()
                    .map(|instance| GlobalInstanceKey {
                        def_id: instance.def_id,
                        arg_module_id: instance.arg_module_id,
                        args: instance.args.clone(),
                        const_args: instance.const_args.clone(),
                    })
                    .collect::<HashSet<_>>(),
            };
            worklist.enqueue_refs(refs);
            let before = (
                functions.len(),
                function_instances.len(),
                global_instances.len(),
                trait_object_vtables.len(),
            );
            self.lower_reachable_instances_and_vtables(
                functions,
                function_templates,
                function_instances,
                global_instances,
                &mut worklist,
                trait_object_vtables,
            );
            if before
                == (
                    functions.len(),
                    function_instances.len(),
                    global_instances.len(),
                    trait_object_vtables.len(),
                )
            {
                break;
            }
        }
    }

    fn collect_new_trait_object_vtables(
        &mut self,
        trait_object_vtables: &mut Vec<BackendTraitObjectVtable>,
        functions: &[BackendFunction],
        function_instances: &[BackendFunctionInstance],
        worklist: &mut ReachabilityWorklist,
    ) -> bool {
        let mut discovered = Vec::new();
        self.collect_trait_object_vtables(&mut discovered, functions, function_instances);
        let mut seen = trait_object_vtables
            .iter()
            .map(|vtable| vtable.key.clone())
            .collect::<HashSet<BackendTraitObjectVtableKey>>();
        let mut changed = false;
        for vtable in discovered {
            if !seen.insert(vtable.key.clone()) {
                continue;
            }
            worklist.enqueue_vtable_refs(&vtable);
            trait_object_vtables.push(vtable);
            changed = true;
        }
        changed
    }

    fn extend_backend_layouts_for_instances(
        &mut self,
        layouts: &mut BackendLayouts,
        struct_instances: &[nia_backend_ir::BackendStructInstance],
        union_instances: &[nia_backend_ir::BackendUnionInstance],
    ) {
        layout_extender::BackendLayoutExtender::new(self.input, &mut self.type_context.interner)
            .extend_for_instances(layouts, struct_instances, union_instances);
    }

    fn expr_ty(&self, expr: &Expr) -> Option<InternedTyId> {
        self.input.semantic_facts.node_expr_type(&expr.node_key)
    }

    pub(crate) fn receiver_kind_for_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<nia_ids::ReceiverKind> {
        if method_id.module_id == self.input.module_id
            && let Some(signature) = self.input.signatures.functions.get(&method_id.def_id)
        {
            return signature.params.first().and_then(|param| param.receiver);
        }
        self.input
            .program_functions
            .get(&method_id)
            .and_then(|signature| signature.signature.params.first())
            .and_then(|param| param.receiver)
    }

    fn def_id_for_node(&mut self, node_key: &VersionedNodeKey, expected: DefKind) -> Option<DefId> {
        let def_id = self.input.defs.def_nodes.get(node_key)?;
        let def = self.input.defs.defs.get(def_id)?;
        if def.kind == expected {
            Some(def_id)
        } else {
            None
        }
    }

    fn def_id_for_node_any_function(&mut self, node_key: &VersionedNodeKey) -> Option<DefId> {
        let def_id = self.input.defs.def_nodes.get(node_key)?;
        let def = self.input.defs.defs.get(def_id)?;
        matches!(
            def.kind,
            DefKind::Function | DefKind::Method | DefKind::TraitMethod
        )
        .then_some(def_id)
    }

    fn global_def_id(&self, def_id: DefId) -> GlobalDefId {
        GlobalDefId {
            module_id: self.input.module_id,
            def_id,
        }
    }

    pub(crate) fn mangle_instance_symbol(
        &mut self,
        def_id: GlobalDefId,
        name: SymbolId,
        self_arg: Option<InternedTyId>,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) -> String {
        let defs = &self.input.defs.defs;
        let input = self.input;
        let const_expr_summaries = &self.input.type_lowering.const_expr_summaries;
        let const_array_lengths = self.input.const_array_lengths;
        let self_arg = self_arg.map(|ty| self.import_instance_arg_type(ty));
        let missing_array_len_diagnostics = &mut self.missing_array_len_diagnostics;
        let diagnostics = &mut self.diagnostics;
        let def_names = &mut self.def_names;
        let mut args = args.to_vec();
        if let Some(self_arg) = self_arg {
            args.insert(0, self_arg);
        }
        let mut symbol = mangle_instance_symbol_id(
            def_id,
            name,
            &args,
            const_args,
            &self.type_context.interner,
            |def_id| {
                if let Some(name) = def_names.get(&def_id) {
                    return name.clone();
                }
                let name = program_def(input, def_id)
                    .or_else(|| defs.get(def_id.def_id).cloned())
                    .map(|def| mangle_symbol_id(def.name))
                    .unwrap_or_else(|| format!("def{}", def_id.def_id.0));
                def_names.insert(def_id, name.clone());
                name
            },
            |id| {
                let value = const_array_lengths.values.get(&id).copied();
                if value.is_none() && missing_array_len_diagnostics.insert(id) {
                    let span = const_expr_summaries
                        .get(&id)
                        .map(|summary| summary.span)
                        .unwrap_or_default();
                    diagnostics.push(Diagnostic::user_error_at(
                        nia_diagnostic::codes::LLVM_CODEGEN,
                        span,
                        format!(
                            "array length {id:?} was not evaluated before backend symbol generation"
                        ),
                    ));
                }
                value
            },
        );
        if self_arg.is_some() {
            symbol = symbol.replacen("__inst__t_", "__inst__t_self_", 1);
        }
        symbol
    }

    pub(crate) fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_or_unresolved(self.input.symbols, symbol)
    }

    pub(crate) fn local_name(&self, name: nia_function_ir::LocalName) -> String {
        match name {
            nia_function_ir::LocalName::SelfValue => "self".to_string(),
            nia_function_ir::LocalName::Named(symbol) => self.symbol_name(symbol),
            nia_function_ir::LocalName::Generated(
                nia_function_ir::GeneratedLocalName::ForIterable,
            ) => "__for_iterable".to_string(),
            nia_function_ir::LocalName::Generated(
                nia_function_ir::GeneratedLocalName::ForIterator,
            ) => "__for_iter".to_string(),
            nia_function_ir::LocalName::Generated(nia_function_ir::GeneratedLocalName::ForNext) => {
                "__for_next".to_string()
            }
            nia_function_ir::LocalName::Temporary(id) => format!("fir.tmp.{id}"),
            nia_function_ir::LocalName::Anonymous => "_".to_string(),
        }
    }

    pub(crate) fn function_local_names(
        &self,
        body: &nia_function_ir::FunctionBody,
    ) -> HashMap<nia_ids::LocalId, String> {
        body.locals
            .iter()
            .map(|local| (local.id, self.local_name(local.name)))
            .collect()
    }

    pub(crate) fn def_symbol_name(&self, def_id: GlobalDefId) -> Option<SymbolId> {
        program_def(self.input, def_id)
            .or_else(|| self.input.defs.defs.get(def_id.def_id).cloned())
            .map(|def| def.name)
    }

    pub(crate) fn layout_of(&self, ty: InternedTyId) -> Option<nia_layout::TypeLayout> {
        self.type_context.layout_of(ty)
    }

    pub(crate) fn field_offset(
        &self,
        ty: InternedTyId,
        field: nia_ids::GlobalDefId,
    ) -> Option<u64> {
        self.type_context.field_offset(ty, field)
    }

    fn error_ty(&self) -> InternedTyId {
        self.input.function_interner.error()
    }

    pub(crate) fn ty_kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.type_context.ty_kind(ty)
    }

    pub(crate) fn active_interner_for_type(&self, ty: InternedTyId) -> &nia_ty::TyInterner {
        self.type_context.active_interner_for_type(ty)
    }
}

fn index_extension_generics_by_method(
    extensions: &ExtensionMethods,
) -> HashMap<GlobalDefId, Vec<SymbolId>> {
    let mut generics_by_method = HashMap::new();
    for method in extensions.all_methods() {
        generics_by_method.insert(method.def_id, method.effective_generics.clone());
    }
    generics_by_method
}

fn index_local_extension_generics_by_method(
    extensions: &VisibleExtensionMethods,
) -> HashMap<GlobalDefId, Vec<SymbolId>> {
    let mut generics_by_method = HashMap::new();
    for target in extensions.targets() {
        for method in &target.methods {
            generics_by_method.insert(method.def_id, method.effective_generics.clone());
        }
    }
    generics_by_method
}

fn index_local_extension_method_sources_by_def(
    input: &BackendLowerModuleInput<'_>,
) -> HashMap<GlobalDefId, ExtensionMethodSource> {
    let mut sources = HashMap::new();
    if let Some(interner) = input.extension_interner {
        for target in input.extensions.targets() {
            for method in &target.methods {
                sources.insert(
                    method.def_id,
                    ExtensionMethodSource {
                        target_ty: target.target_ty,
                        where_predicates: method.where_predicates.clone(),
                        interner: interner.clone(),
                    },
                );
            }
        }
    }
    sources
}

fn index_program_extension_method_sources_by_def(
    input: &BackendLowerModuleInput<'_>,
) -> HashMap<GlobalDefId, ExtensionMethodSource> {
    let mut sources = HashMap::new();
    for method in input.program_extension_methods.all_methods() {
        let Some(type_normalization) = input
            .program_type_normalizations
            .get(&method.def_id.module_id)
        else {
            continue;
        };
        sources.insert(
            method.def_id,
            ExtensionMethodSource {
                target_ty: method.target_ty,
                where_predicates: method.where_predicates.clone(),
                interner: type_normalization.interner.clone(),
            },
        );
    }
    sources
}

fn index_input_type_interner_snapshots(
    modules: &[BackendLowerModuleInput<'_>],
) -> HashMap<TyInternerId, nia_ty::TyInterner> {
    let mut interners = HashMap::new();
    for input in modules {
        insert_input_type_interner_snapshot(&mut interners, "body_ir", &input.body_ir.interner);
        insert_input_type_interner_snapshot(&mut interners, "function", input.function_interner);
        for interner in input.program_function_body_interners.values() {
            insert_input_type_interner_snapshot(&mut interners, "program_function_body", interner);
        }
    }
    interners
}

fn insert_input_type_interner_snapshot(
    interners: &mut HashMap<TyInternerId, nia_ty::TyInterner>,
    source: &'static str,
    interner: &nia_ty::TyInterner,
) {
    let interner_id = interner.interner_id();
    if let Some(existing) = interners.get(&interner_id) {
        if existing.is_prefix_of(interner) {
            interners.insert(interner_id, interner.clone());
        } else if !interner.is_prefix_of(existing) {
            panic!(
                "conflicting type interner snapshots share id {:?} from {}",
                interner_id, source
            );
        }
    } else {
        interners.insert(interner_id, interner.clone());
    }
}

fn index_local_trait_impls_by_method(
    input: &BackendLowerModuleInput<'_>,
) -> HashMap<GlobalDefId, usize> {
    let impls = input
        .trait_impls
        .iter()
        .enumerate()
        .map(|(program_index, impl_signature)| {
            (
                (impl_signature.module_id, impl_signature.impl_id),
                program_index,
            )
        })
        .collect::<HashMap<_, _>>();
    let mut impls_by_method = HashMap::new();
    for target in input.extensions.targets() {
        for method in &target.methods {
            let Some(program_index) = impls.get(&(input.module_id, method.impl_id)).copied() else {
                continue;
            };
            impls_by_method.insert(method.def_id, program_index);
        }
    }
    impls_by_method
}

fn index_program_trait_impls_by_method(
    input: &BackendLowerModuleInput<'_>,
) -> HashMap<GlobalDefId, usize> {
    let impls = input
        .trait_impls
        .iter()
        .enumerate()
        .map(|(program_index, impl_signature)| {
            (
                (impl_signature.module_id, impl_signature.impl_id),
                program_index,
            )
        })
        .collect::<HashMap<_, _>>();
    let mut impls_by_method = HashMap::new();
    for method in input.program_extension_methods.all_methods() {
        let Some(program_index) = impls
            .get(&(method.def_id.module_id, method.impl_id))
            .copied()
        else {
            continue;
        };
        impls_by_method.insert(method.def_id, program_index);
    }
    impls_by_method
}

fn index_extension_trait_method_candidates(
    extensions: &VisibleExtensionMethods,
    source_interner: &nia_ty::TyInterner,
) -> HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>> {
    let mut candidates: HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>> =
        HashMap::new();
    let source_interner = Arc::new(source_interner.clone());
    for target in extensions.targets() {
        for method in &target.methods {
            if !method.is_trait_witness {
                continue;
            }
            let Some(trait_id) = method.trait_id else {
                continue;
            };
            candidates
                .entry(ExtensionTraitMethodKey {
                    trait_id,
                    method_name: method.name,
                    trait_arg_count: method.trait_args.len(),
                })
                .or_default()
                .push(ExtensionTraitMethodCandidate {
                    target_ty: target.target_ty,
                    method_def_id: method.def_id,
                    trait_args: method.trait_args.clone(),
                    where_predicates: method.where_predicates.clone(),
                    effective_generics: method.effective_generics.clone(),
                    interner: source_interner.clone(),
                });
        }
    }
    candidates
}

fn index_program_extension_trait_method_candidates(
    input: Option<&BackendLowerModuleInput<'_>>,
) -> HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>> {
    let Some(input) = input else {
        return HashMap::new();
    };
    let impls = input
        .trait_impls
        .iter()
        .map(|impl_signature| {
            (
                (impl_signature.module_id, impl_signature.impl_id),
                impl_signature,
            )
        })
        .collect::<HashMap<_, _>>();
    let mut candidates: HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>> =
        HashMap::new();
    let mut interner_cache = CandidateInternerCache::default();
    for method in input.program_extension_methods.all_methods() {
        let Some(trait_id) = method.trait_id else {
            continue;
        };
        let Some(impl_signature) = impls.get(&(method.def_id.module_id, method.impl_id)) else {
            continue;
        };
        let candidate = ExtensionTraitMethodCandidate {
            target_ty: impl_signature.target_ty,
            method_def_id: method.def_id,
            trait_args: impl_signature.trait_args.clone(),
            where_predicates: impl_signature.where_predicates.clone(),
            effective_generics: impl_signature.generics.clone(),
            interner: interner_cache.intern(&impl_signature.interner),
        };
        candidates
            .entry(ExtensionTraitMethodKey {
                trait_id,
                method_name: method.name,
                trait_arg_count: method.trait_args.len(),
            })
            .or_default()
            .push(candidate);
    }
    for bucket in candidates.values_mut() {
        let mut seen = HashSet::new();
        bucket.retain(|candidate| {
            if seen.contains(&candidate.method_def_id) {
                false
            } else {
                seen.insert(candidate.method_def_id);
                true
            }
        });
    }
    candidates
}

#[derive(Default)]
struct CandidateInternerCache {
    interners: Vec<Arc<nia_ty::TyInterner>>,
}

impl CandidateInternerCache {
    fn intern(&mut self, interner: &nia_ty::TyInterner) -> Arc<nia_ty::TyInterner> {
        if let Some(existing) = self
            .interners
            .iter()
            .find(|existing| existing.as_ref() == interner)
        {
            return existing.clone();
        }
        let interner = Arc::new(interner.clone());
        self.interners.push(interner.clone());
        interner
    }
}

fn index_local_trait_methods_with_defaults(
    input: &BackendLowerModuleInput<'_>,
) -> HashSet<GlobalDefId> {
    input
        .signatures
        .traits
        .values()
        .flat_map(|signature| signature.methods.iter())
        .filter(|method| method.has_default)
        .map(|method| GlobalDefId {
            module_id: input.module_id,
            def_id: method.def_id,
        })
        .collect::<HashSet<_>>()
}

fn index_program_trait_methods_with_defaults(
    input: &BackendLowerModuleInput<'_>,
) -> HashSet<GlobalDefId> {
    input
        .program_traits
        .iter()
        .flat_map(|(trait_id, signature)| {
            signature
                .signature
                .methods
                .iter()
                .filter(|method| method.has_default)
                .map(|method| GlobalDefId {
                    module_id: trait_id.module_id,
                    def_id: method.def_id,
                })
        })
        .collect()
}

fn index_local_method_symbols_by_def(
    input: &BackendLowerModuleInput<'_>,
) -> HashMap<GlobalDefId, SymbolId> {
    let mut names = input
        .defs
        .defs
        .iter()
        .map(|(def_id, def)| {
            (
                GlobalDefId {
                    module_id: input.module_id,
                    def_id,
                },
                def.name,
            )
        })
        .collect::<HashMap<_, _>>();
    for target in input.extensions.targets() {
        for method in &target.methods {
            names.entry(method.def_id).or_insert(method.name);
        }
    }
    names
}

fn index_program_method_symbols_by_def(
    input: &BackendLowerModuleInput<'_>,
) -> HashMap<GlobalDefId, SymbolId> {
    let mut names = HashMap::new();
    for (trait_id, signature) in input.program_traits {
        for method in &signature.signature.methods {
            names.insert(
                GlobalDefId {
                    module_id: trait_id.module_id,
                    def_id: method.def_id,
                },
                method.name,
            );
        }
    }
    for method in input.program_extension_methods.all_methods() {
        names.insert(method.def_id, method.name);
    }
    names
}

fn index_layout_instances_by_def<'a>(
    keys: impl IntoIterator<Item = &'a StructLayoutKey>,
) -> HashMap<DefId, Vec<StructLayoutKey>> {
    let mut instances_by_def = HashMap::new();
    for key in keys {
        instances_by_def
            .entry(key.def_id)
            .or_insert_with(Vec::new)
            .push(key.clone());
    }
    instances_by_def
}

#[cfg(test)]
mod tests;
