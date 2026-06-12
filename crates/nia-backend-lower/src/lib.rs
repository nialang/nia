// SPDX-License-Identifier: GPL-3.0-or-later
mod function_instances;
mod function_refs;
mod instantiate;
mod items;
mod module_const_prop;
mod module_dce;
mod module_devirt;
mod module_inline;
mod operator_dispatch;
mod opt;
mod struct_instances;
mod trait_object_vtables;

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

use nia_ast::{Expr, ItemKind, Module, Visibility};
use nia_backend_ir::{
    BackendFunction, BackendFunctionInstance, BackendLayouts, BackendModule, BackendProgram,
    BackendStructInstanceKey, BackendTraitObjectVtable, BackendTraitObjectVtableFunction,
    BackendTraitObjectVtableKey,
};
use nia_body_ir::BodyIr;
use nia_defs::{DefCollection, DefId, DefKind, ExtensionMethods, VisibleExtensionMethods};
use nia_diagnostic::Diagnostic;
use nia_function_ir::FunctionBody;
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId, TraitId};
use nia_item_signatures::{
    ItemSignatures, ProgramEnumSignature, ProgramFunctionSignature, ProgramStructSignature,
    ProgramTraitImplSignature, ProgramTraitSignature, ProgramUnionSignature,
    WherePredicateSignature,
};
use nia_layout::{Layouts, ProgramLayoutContext, StructLayoutKey};
use nia_local_resolve::LocalResolution;
use nia_mangle::{mangle_instance_symbol, sanitize_symbol_part};
use nia_monomorphize::Monomorphization;
use nia_node_id::NodeKey;
use nia_opt::{InlineThreshold, OptimizationDepth, OptimizationPolicy};
use nia_sema_ir::SemanticFacts;
use nia_trait_solve::TraitResolution;
use nia_ty::TyKind;
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_value_resolve::ValueResolution;

use crate::function_refs::{FunctionInstanceKey, FunctionInstanceRef, FunctionRefs};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendTimingMode {
    #[default]
    Off,
    Detail,
}

impl BackendTimingMode {
    fn enabled(self) -> bool {
        matches!(self, Self::Detail)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendLowerModuleInput<'a> {
    pub module_id: ModuleId,
    pub module_name: String,
    pub module: &'a Module,
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
    pub comptime: &'a nia_comptime_check::ComptimeCheck,
    pub layouts: &'a Layouts,
    pub function_bodies: &'a std::collections::HashMap<GlobalDefId, FunctionBody>,
    pub roots: BackendFunctionRoots,
    pub program_function_bodies: &'a std::collections::HashMap<GlobalDefId, FunctionBody>,
    pub extension_interner: Option<&'a nia_ty::TyInterner>,
    pub program_extension_methods: &'a ExtensionMethods,
    pub program_extensions: &'a std::collections::HashMap<
        ModuleId,
        (&'a VisibleExtensionMethods, &'a nia_ty::TyInterner),
    >,
    pub program_defs: &'a std::collections::HashMap<ModuleId, DefCollection>,
    pub program_type_interners: &'a std::collections::HashMap<ModuleId, &'a nia_ty::TyInterner>,
    pub program_functions: &'a std::collections::HashMap<GlobalDefId, ProgramFunctionSignature>,
    pub program_structs: &'a std::collections::HashMap<GlobalDefId, ProgramStructSignature>,
    pub program_unions: &'a std::collections::HashMap<GlobalDefId, ProgramUnionSignature>,
    pub program_enums: &'a std::collections::HashMap<GlobalDefId, ProgramEnumSignature>,
    pub program_traits: &'a std::collections::HashMap<GlobalDefId, ProgramTraitSignature>,
    pub trait_impls: &'a [ProgramTraitImplSignature],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendFunctionRoots {
    #[default]
    Public,
    FunctionBodies,
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
        BackendTimingMode::Off,
    )
}

pub fn lower_backend_program_with_timings(
    modules: &[BackendLowerModuleInput<'_>],
    monomorphization: &Monomorphization,
    optimization: OptimizationPolicy,
    timings: BackendTimingMode,
) -> BackendLowering {
    let timing = timings.enabled();
    let mut diagnostics = Vec::new();
    let mut optimization_report = BackendOptimizationReport {
        enabled_module_passes: enabled_module_passes(&optimization),
        enabled_function_passes: opt::enabled_function_passes(&optimization),
        enabled_global_passes: enabled_global_passes(&optimization),
        changed_passes: Vec::new(),
    };
    let shared = time_backend_stage(timing, "backend_lower.shared_indexes", || {
        BackendLowerShared::new(modules, monomorphization)
    });
    let mut lowerers = time_backend_stage(timing, "backend_lower.new_lowerers", || {
        modules
            .iter()
            .map(|input| ModuleLowerer::new(input, monomorphization, optimization, &shared))
            .collect::<Vec<_>>()
    });
    let mut lowered_modules = Vec::new();
    let mut pending_foreign_instances = VecDeque::new();
    time_backend_stage(timing, "backend_lower.initial_modules", || {
        for lowerer in &mut lowerers {
            let module = lowerer.lower_module();
            if timing {
                eprintln!(
                    "query timing backend_lower.module[{:?}]: functions={} instances={} structs={} unions={}",
                    module.id,
                    module.functions.len(),
                    module.function_instances.len(),
                    module.struct_instances.len(),
                    module.union_instances.len()
                );
            }
            pending_foreign_instances
                .extend(std::mem::take(&mut lowerer.foreign_function_instance_refs));
            diagnostics.extend(std::mem::take(&mut lowerer.diagnostics));
            optimization_report.changed_passes.extend(std::mem::take(
                &mut lowerer.optimization_report.changed_passes,
            ));
            lowered_modules.push(module);
        }
    });
    time_backend_stage(timing, "backend_lower.refresh_interners", || {
        refresh_known_backend_type_interners(&mut lowerers);
    });

    let module_indices = lowered_modules
        .iter()
        .enumerate()
        .map(|(index, module)| (module.id, index))
        .collect::<HashMap<_, _>>();
    let mut queued_foreign_instances = HashSet::new();
    time_backend_stage(timing, "backend_lower.foreign_instances", || {
        while !pending_foreign_instances.is_empty() {
            let mut batches = (0..lowerers.len()).map(|_| Vec::new()).collect::<Vec<_>>();
            while let Some(instance) = pending_foreign_instances.pop_front() {
                if !queued_foreign_instances.insert(instance.key()) {
                    continue;
                }
                let Some(owner_index) = module_indices.get(&instance.def_id.module_id).copied()
                else {
                    continue;
                };
                batches[owner_index].push(instance);
            }

            for (owner_index, refs) in batches.into_iter().enumerate() {
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
                    refresh_known_backend_type_interner_from_source(&mut lowerers, owner_index);
                }
                pending_foreign_instances.extend(std::mem::take(
                    &mut lowerers[owner_index].foreign_function_instance_refs,
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
    if !enabled {
        return f();
    }
    let start = Instant::now();
    let result = f();
    eprintln!("query timing {name}: {:.3}s", start.elapsed().as_secs_f64());
    result
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
    module.interner = lowerer.interner.clone();
}

fn enqueue_function_ref(
    pending: &mut VecDeque<GlobalDefId>,
    queued: &mut HashSet<GlobalDefId>,
    def_id: GlobalDefId,
) {
    if queued.insert(def_id) {
        pending.push_back(def_id);
    }
}

fn enqueue_function_refs(
    refs: FunctionRefs,
    pending_functions: &mut VecDeque<GlobalDefId>,
    queued_functions: &mut HashSet<GlobalDefId>,
    pending_instances: &mut Vec<FunctionInstanceRef>,
    queued_instances: &mut HashSet<FunctionInstanceKey>,
) {
    for function in refs.functions {
        enqueue_function_ref(pending_functions, queued_functions, function);
    }
    enqueue_function_instance_refs(refs.instances, pending_instances, queued_instances);
}

fn enqueue_function_instance_refs(
    refs: impl IntoIterator<Item = FunctionInstanceRef>,
    pending_instances: &mut Vec<FunctionInstanceRef>,
    queued_instances: &mut HashSet<FunctionInstanceKey>,
) {
    for instance in refs {
        if queued_instances.insert(instance.key()) {
            pending_instances.push(instance);
        }
    }
}

fn enqueue_trait_object_vtable_refs(
    vtable: &BackendTraitObjectVtable,
    pending_functions: &mut VecDeque<GlobalDefId>,
    queued_functions: &mut HashSet<GlobalDefId>,
    pending_instances: &mut Vec<FunctionInstanceRef>,
    queued_instances: &mut HashSet<FunctionInstanceKey>,
) {
    for entry in &vtable.entries {
        match &entry.function {
            BackendTraitObjectVtableFunction::Function(function) => {
                enqueue_function_ref(pending_functions, queued_functions, *function);
            }
            BackendTraitObjectVtableFunction::FunctionInstance {
                def_id,
                arg_module_id,
                args,
            } => enqueue_function_instance_refs(
                [FunctionInstanceRef {
                    def_id: *def_id,
                    arg_module_id: *arg_module_id,
                    args: args.clone(),
                    span: vtable.span,
                }],
                pending_instances,
                queued_instances,
            ),
        }
    }
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
    pub(crate) interner: nia_ty::TyInterner,
    pub(crate) diagnostics: Vec<Diagnostic>,
    optimization_report: BackendOptimizationReport,
    missing_array_len_diagnostics: HashSet<GlobalConstExprId>,
    extension_generics_by_method: HashMap<GlobalDefId, Vec<String>>,
    trait_impls_by_method: HashMap<GlobalDefId, usize>,
    extension_trait_method_candidates:
        HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>>,
    instance_extension_trait_method_candidates: Option<(
        ModuleId,
        HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>>,
    )>,
    instance_extension_interner: Option<&'a nia_ty::TyInterner>,
    dynamic_type_interners: HashMap<ModuleId, Vec<nia_ty::TyInterner>>,
    current_instantiated_function: Option<GlobalDefId>,
    current_instantiation_module_id: Option<ModuleId>,
    current_instantiated_body_interner: Option<&'a nia_ty::TyInterner>,
    current_type_substitutions: Option<TypeSubstitutionId>,
    foreign_function_instance_refs: Vec<function_refs::FunctionInstanceRef>,
    struct_layout_instances_by_def: HashMap<DefId, Vec<StructLayoutKey>>,
    union_layout_instances_by_def: HashMap<DefId, Vec<StructLayoutKey>>,
    builtin_trait_resolutions: HashMap<BuiltinTraitGoalKey, TraitResolution>,
    trait_methods_with_defaults: HashSet<GlobalDefId>,
    type_instantiations: HashMap<TypeInstantiationKey, InternedTyId>,
    type_substitutions: Vec<HashMap<String, InternedTyId>>,
    type_substitution_ids: HashMap<TypeSubstitutionKey, TypeSubstitutionId>,
    effective_generics: HashMap<GlobalDefId, Vec<String>>,
    def_names: HashMap<GlobalDefId, String>,
    method_names_by_def: HashMap<GlobalDefId, String>,
    trait_object_vtables: trait_object_vtables::TraitObjectVtableCache,
    function_sources: HashMap<GlobalDefId, BackendFunctionSource<'a>>,
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
    method_name: String,
    trait_arg_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExtensionTraitMethodCandidate {
    target_ty: InternedTyId,
    method_def_id: GlobalDefId,
    trait_args: Vec<InternedTyId>,
    where_predicates: Vec<WherePredicateSignature>,
    impl_generics: Vec<String>,
    source_interner: nia_ty::TyInterner,
}

pub(crate) struct BackendLowerShared {
    program_extension_trait_method_candidates:
        HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>>,
    known_type_interners: HashMap<ModuleId, Vec<nia_ty::TyInterner>>,
}

impl BackendLowerShared {
    fn new(modules: &[BackendLowerModuleInput<'_>], monomorphization: &Monomorphization) -> Self {
        Self {
            program_extension_trait_method_candidates:
                index_program_extension_trait_method_candidates(modules),
            known_type_interners: index_shared_known_type_interners(modules, monomorphization),
        }
    }
}

#[derive(Clone, Copy)]
struct BackendFunctionSource<'a> {
    span: nia_span::Span,
    function: &'a nia_ast::FunctionItem,
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
    substitutions: Vec<(String, InternedTyId)>,
}

impl<'a> ModuleLowerer<'a> {
    fn new(
        input: &'a BackendLowerModuleInput<'a>,
        monomorphization: &'a Monomorphization,
        optimization: OptimizationPolicy,
        shared: &'a BackendLowerShared,
    ) -> Self {
        Self {
            input,
            shared,
            monomorphization,
            optimization,
            interner: input.function_interner.clone(),
            diagnostics: Vec::new(),
            optimization_report: BackendOptimizationReport::default(),
            missing_array_len_diagnostics: HashSet::new(),
            extension_generics_by_method: index_extension_generics_by_method(input.extensions),
            trait_impls_by_method: index_trait_impls_by_method(input),
            extension_trait_method_candidates: index_extension_trait_method_candidates(
                input.extensions,
                input.extension_interner.unwrap_or(input.function_interner),
            ),
            instance_extension_trait_method_candidates: None,
            instance_extension_interner: None,
            dynamic_type_interners: HashMap::new(),
            current_instantiated_function: None,
            current_instantiation_module_id: None,
            current_instantiated_body_interner: None,
            current_type_substitutions: None,
            foreign_function_instance_refs: Vec::new(),
            struct_layout_instances_by_def: index_layout_instances_by_def(
                input.layouts.struct_instances.keys(),
            ),
            union_layout_instances_by_def: index_layout_instances_by_def(
                input.layouts.union_instances.keys(),
            ),
            builtin_trait_resolutions: HashMap::new(),
            trait_methods_with_defaults: index_trait_methods_with_defaults(input),
            type_instantiations: HashMap::new(),
            type_substitutions: Vec::new(),
            type_substitution_ids: HashMap::new(),
            effective_generics: HashMap::new(),
            def_names: HashMap::new(),
            method_names_by_def: index_method_names_by_def(input),
            trait_object_vtables: trait_object_vtables::TraitObjectVtableCache::default(),
            function_sources: HashMap::new(),
        }
    }

    fn lower_module(&mut self) -> BackendModule {
        let mut structs = Vec::new();
        let mut unions = Vec::new();
        let mut struct_instances = Vec::new();
        let mut union_instances = Vec::new();
        let mut enums = Vec::new();
        let mut globals = Vec::new();
        let mut functions = Vec::new();
        let mut function_templates = Vec::new();
        let mut pending_functions = VecDeque::new();
        let mut queued_functions = HashSet::new();
        let mut pending_instances = Vec::new();
        let mut queued_instances = HashSet::new();
        let mut trait_object_vtables = Vec::new();

        for item in &self.input.module.items {
            match &item.kind {
                ItemKind::Struct(item_struct) => {
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
                ItemKind::Union(item_union) => {
                    if item_union.generics.is_empty()
                        && let Some(item) = self.lower_union(&item.node_key, item.span, item_union)
                    {
                        unions.push(item);
                    }
                    union_instances.extend(self.lower_union_instances(
                        &item.node_key,
                        item.span,
                        item_union,
                    ));
                }
                ItemKind::Trait(item_trait) => {
                    for method in &item_trait.methods {
                        self.index_function_source(
                            method.function.span,
                            &method.function,
                            &mut pending_functions,
                            &mut queued_functions,
                        );
                    }
                }
                ItemKind::Extend(extend) => {
                    for method in &extend.methods {
                        let def_id = self.index_function_source(
                            method.function.span,
                            &method.function,
                            &mut pending_functions,
                            &mut queued_functions,
                        );
                        if extend.trait_ref.is_some()
                            && let Some(def_id) = def_id
                            && self.is_eager_trait_impl_method_root(def_id)
                        {
                            enqueue_function_ref(
                                &mut pending_functions,
                                &mut queued_functions,
                                def_id,
                            );
                        }
                    }
                }
                ItemKind::Enum(item_enum) => {
                    if let Some(item) = self.lower_enum(&item.node_key, item.span, item_enum) {
                        enums.push(item);
                    }
                }
                ItemKind::Function(function) => {
                    self.index_function_source(
                        item.span,
                        function,
                        &mut pending_functions,
                        &mut queued_functions,
                    );
                }
                ItemKind::Binding(binding) => {
                    if binding.is_comptime {
                        continue;
                    }
                    if let Some(global) = self.lower_global(&binding.node_key, item.span, binding) {
                        if let Some(init) = &global.init {
                            let mut refs = FunctionRefs::default();
                            function_refs::collect_function_refs_from_static_init(
                                self.input.module_id,
                                init,
                                &mut refs,
                            );
                            enqueue_function_refs(
                                refs,
                                &mut pending_functions,
                                &mut queued_functions,
                                &mut pending_instances,
                                &mut queued_instances,
                            );
                        }
                        globals.push(global);
                    }
                }
                ItemKind::Module(_)
                | ItemKind::Using(_)
                | ItemKind::ComptimeIf(_)
                | ItemKind::TypeAlias(_) => {}
            }
        }

        pending_instances.extend(self.initial_monomorphized_function_instance_refs());
        self.lower_reachable_function_closure(
            &mut functions,
            &mut pending_functions,
            &mut queued_functions,
            &mut pending_instances,
            &mut queued_instances,
            &mut trait_object_vtables,
        );
        let mut function_instances = Vec::new();
        self.lower_reachable_instances_and_vtables(
            &mut functions,
            &mut function_templates,
            &mut function_instances,
            &mut pending_functions,
            &mut queued_functions,
            &mut pending_instances,
            &mut queued_instances,
            &mut trait_object_vtables,
        );
        self.devirtualize_direct_trait_calls(&mut functions, &mut function_instances);
        self.propagate_cross_function_constants(&mut functions, &mut function_instances);
        self.inline_leaf_functions(&mut functions, &mut function_instances);
        self.complete_reachable_backend_items(
            &mut functions,
            &mut function_templates,
            &mut function_instances,
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
            interner: self.interner.clone(),
            comptime: self.input.comptime.clone(),
            layouts: backend_layouts,
            structs,
            unions,
            struct_instances,
            union_instances,
            enums,
            globals,
            functions,
            function_instances,
            trait_object_vtables,
            generic_instantiations: self
                .input
                .semantic_facts
                .generic_instantiations
                .iter()
                .map(|inst| nia_backend_ir::BackendGenericInstantiation {
                    def_id: inst.def_id,
                    arg_module_id: self.input.module_id,
                    args: inst.args.clone(),
                    span: inst.span,
                    source_def_id: inst.source_def_id,
                })
                .collect(),
        }
    }

    fn is_eager_trait_impl_method_root(&self, def_id: GlobalDefId) -> bool {
        match self.input.roots {
            BackendFunctionRoots::Public => true,
            BackendFunctionRoots::FunctionBodies => {
                self.input.function_bodies.contains_key(&def_id)
            }
        }
    }

    fn index_function_source(
        &mut self,
        span: nia_span::Span,
        function: &'a nia_ast::FunctionItem,
        pending: &mut VecDeque<GlobalDefId>,
        queued: &mut HashSet<GlobalDefId>,
    ) -> Option<GlobalDefId> {
        let Some(def_id) = self.def_id_for_node_any_function(&function.node_key) else {
            return None;
        };
        let global_def_id = self.global_def_id(def_id);
        self.function_sources
            .insert(global_def_id, BackendFunctionSource { span, function });
        if self.is_backend_function_root(global_def_id, function) {
            enqueue_function_ref(pending, queued, global_def_id);
        }
        Some(global_def_id)
    }

    fn is_backend_function_root(
        &self,
        def_id: GlobalDefId,
        function: &nia_ast::FunctionItem,
    ) -> bool {
        if self.input.roots == BackendFunctionRoots::FunctionBodies {
            return function.is_extern || self.input.function_bodies.contains_key(&def_id);
        }
        if function.is_comptime
            || function.is_extern
            || function.name == "main"
            || function.name == "_start"
        {
            return true;
        }
        let Some(def) = self.input.defs.defs.get(def_id.def_id) else {
            return false;
        };
        def.visibility != Visibility::Private
    }

    fn lower_reachable_function_closure(
        &mut self,
        functions: &mut Vec<BackendFunction>,
        pending_functions: &mut VecDeque<GlobalDefId>,
        queued_functions: &mut HashSet<GlobalDefId>,
        pending_instances: &mut Vec<FunctionInstanceRef>,
        queued_instances: &mut HashSet<FunctionInstanceKey>,
        trait_object_vtables: &mut Vec<BackendTraitObjectVtable>,
    ) -> bool {
        let mut changed = false;
        let mut lowered = functions
            .iter()
            .map(|function| function.def_id)
            .collect::<HashSet<_>>();
        while let Some(def_id) = pending_functions.pop_front() {
            if def_id.module_id != self.input.module_id || lowered.contains(&def_id) {
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
                enqueue_function_refs(
                    refs,
                    pending_functions,
                    queued_functions,
                    pending_instances,
                    queued_instances,
                );
                functions.push(function);
                changed = true;
            }
        }
        changed |= self.collect_new_trait_object_vtables(
            trait_object_vtables,
            functions,
            &[],
            pending_functions,
            queued_functions,
            pending_instances,
            queued_instances,
        );
        changed
    }

    fn lower_reachable_instances_and_vtables(
        &mut self,
        functions: &mut Vec<BackendFunction>,
        function_templates: &mut Vec<BackendFunction>,
        function_instances: &mut Vec<BackendFunctionInstance>,
        pending_functions: &mut VecDeque<GlobalDefId>,
        queued_functions: &mut HashSet<GlobalDefId>,
        pending_instances: &mut Vec<FunctionInstanceRef>,
        queued_instances: &mut HashSet<FunctionInstanceKey>,
        trait_object_vtables: &mut Vec<BackendTraitObjectVtable>,
    ) {
        loop {
            let mut changed = self.lower_reachable_function_closure(
                functions,
                pending_functions,
                queued_functions,
                pending_instances,
                queued_instances,
                trait_object_vtables,
            );
            if !pending_instances.is_empty() {
                self.lower_pending_instance_templates(function_templates, pending_instances);
                let refs = std::mem::take(pending_instances);
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
                    enqueue_function_refs(
                        refs,
                        pending_functions,
                        queued_functions,
                        pending_instances,
                        queued_instances,
                    );
                }
                changed |= !additional.is_empty();
                function_instances.extend(additional);
            }
            changed |= self.collect_new_trait_object_vtables(
                trait_object_vtables,
                functions,
                function_instances,
                pending_functions,
                queued_functions,
                pending_instances,
                queued_instances,
            );
            if !changed && pending_functions.is_empty() && pending_instances.is_empty() {
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
            if instance.def_id.module_id != self.input.module_id || !known.insert(instance.def_id) {
                continue;
            }
            let Some(source) = self.function_sources.get(&instance.def_id).copied() else {
                continue;
            };
            if let Some(function) = self.lower_function(source.span, source.function) {
                function_templates.push(function);
            }
        }
    }

    fn lower_additional_reachable_functions_from_instances(&mut self, module: &mut BackendModule) {
        let mut function_templates = Vec::new();
        self.complete_reachable_backend_items(
            &mut module.functions,
            &mut function_templates,
            &mut module.function_instances,
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
        module.interner = self.interner.clone();
    }

    fn complete_reachable_backend_items(
        &mut self,
        functions: &mut Vec<BackendFunction>,
        function_templates: &mut Vec<BackendFunction>,
        function_instances: &mut Vec<BackendFunctionInstance>,
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
            for vtable in trait_object_vtables.iter() {
                for entry in &vtable.entries {
                    match &entry.function {
                        BackendTraitObjectVtableFunction::Function(function) => {
                            refs.functions.insert(*function);
                        }
                        BackendTraitObjectVtableFunction::FunctionInstance {
                            def_id,
                            arg_module_id,
                            args,
                        } => refs.instances.push(FunctionInstanceRef {
                            def_id: *def_id,
                            arg_module_id: *arg_module_id,
                            args: args.clone(),
                            span: vtable.span,
                        }),
                    }
                }
            }

            let mut pending_functions = VecDeque::new();
            let mut queued_functions = functions
                .iter()
                .map(|function| function.def_id)
                .collect::<HashSet<_>>();
            let mut pending_instances = Vec::new();
            let mut queued_instances = function_instances
                .iter()
                .map(FunctionInstanceKey::from)
                .collect::<HashSet<_>>();
            enqueue_function_refs(
                refs,
                &mut pending_functions,
                &mut queued_functions,
                &mut pending_instances,
                &mut queued_instances,
            );
            let before = (
                functions.len(),
                function_instances.len(),
                trait_object_vtables.len(),
            );
            self.lower_reachable_instances_and_vtables(
                functions,
                function_templates,
                function_instances,
                &mut pending_functions,
                &mut queued_functions,
                &mut pending_instances,
                &mut queued_instances,
                trait_object_vtables,
            );
            if before
                == (
                    functions.len(),
                    function_instances.len(),
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
        pending_functions: &mut VecDeque<GlobalDefId>,
        queued_functions: &mut HashSet<GlobalDefId>,
        pending_instances: &mut Vec<FunctionInstanceRef>,
        queued_instances: &mut HashSet<FunctionInstanceKey>,
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
            enqueue_trait_object_vtable_refs(
                &vtable,
                pending_functions,
                queued_functions,
                pending_instances,
                queued_instances,
            );
            trait_object_vtables.push(vtable);
            changed = true;
        }
        changed
    }

    fn extend_backend_layouts_for_instances(
        &self,
        layouts: &mut BackendLayouts,
        struct_instances: &[nia_backend_ir::BackendStructInstance],
        union_instances: &[nia_backend_ir::BackendUnionInstance],
    ) {
        let computed = nia_layout::compute_layouts_with_normalized_types(
            self.input.defs,
            &self.interner,
            self.input.signatures,
            &self.input.type_normalization.normalized,
            &|id| self.input.comptime.array_lengths.get(&id).copied(),
            self.input.layouts.target,
        );
        append_missing_layout_instances(
            &mut layouts.struct_instances,
            computed.struct_instances,
            self.input.module_id,
        );
        append_missing_layout_instances(
            &mut layouts.union_instances,
            computed.union_instances,
            self.input.module_id,
        );
        self.append_foreign_instance_layouts(layouts, struct_instances, union_instances);
    }

    fn append_foreign_instance_layouts(
        &self,
        layouts: &mut BackendLayouts,
        struct_instances: &[nia_backend_ir::BackendStructInstance],
        union_instances: &[nia_backend_ir::BackendUnionInstance],
    ) {
        let array_lengths = |id| self.input.comptime.array_lengths.get(&id).copied();
        let program = ProgramLayoutContext {
            layouts: None,
            array_lengths: Some(&array_lengths),
            structs: Some(self.input.program_structs),
            unions: Some(self.input.program_unions),
        };
        let layout_input = nia_layout::LayoutComputationInput {
            defs: self.input.defs,
            interner: &self.interner,
            signatures: self.input.signatures,
            normalized: &self.input.type_normalization.normalized,
            array_lengths: &array_lengths,
            target: self.input.layouts.target,
            program,
        };

        let mut seen_structs = layouts
            .struct_instances
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<HashSet<_>>();
        for instance in struct_instances {
            if instance.def_id.module_id == self.input.module_id {
                continue;
            }
            let key = BackendStructInstanceKey {
                def_id: instance.def_id,
                args: instance.args.clone(),
            };
            if !seen_structs.insert(key.clone()) {
                continue;
            }
            if let Some(layout) = nia_layout::compute_struct_instance_layout_with_program_context(
                layout_input,
                nia_layout::InstanceLayoutRequest {
                    def_id: instance.def_id,
                    args: &instance.args,
                },
            ) {
                layouts.struct_instances.push((key, layout));
            }
        }

        let mut seen_unions = layouts
            .union_instances
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<HashSet<_>>();
        for instance in union_instances {
            if instance.def_id.module_id == self.input.module_id {
                continue;
            }
            let key = BackendStructInstanceKey {
                def_id: instance.def_id,
                args: instance.args.clone(),
            };
            if !seen_unions.insert(key.clone()) {
                continue;
            }
            if let Some(layout) = nia_layout::compute_union_instance_layout_with_program_context(
                layout_input,
                nia_layout::InstanceLayoutRequest {
                    def_id: instance.def_id,
                    args: &instance.args,
                },
            ) {
                layouts.union_instances.push((key, layout));
            }
        }
    }

    fn expr_ty(&self, expr: &Expr) -> Option<InternedTyId> {
        self.input
            .semantic_facts
            .node_expr_types
            .get(&expr.node_key)
            .copied()
    }

    pub(crate) fn receiver_kind_for_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<nia_ast::ReceiverKind> {
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

    fn def_id_for_node(&mut self, node_key: &NodeKey, expected: DefKind) -> Option<DefId> {
        let def_id = self.input.defs.def_nodes.get(node_key)?;
        let def = self.input.defs.defs.get(def_id)?;
        if def.kind == expected {
            Some(def_id)
        } else {
            None
        }
    }

    fn def_id_for_node_any_function(&mut self, node_key: &NodeKey) -> Option<DefId> {
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
        name: &str,
        args: &[InternedTyId],
    ) -> String {
        let defs = &self.input.defs.defs;
        let const_exprs = &self.input.type_lowering.const_exprs;
        let comptime = self.input.comptime;
        let missing_array_len_diagnostics = &mut self.missing_array_len_diagnostics;
        let diagnostics = &mut self.diagnostics;
        let def_names = &mut self.def_names;
        mangle_instance_symbol(
            def_id,
            name,
            args,
            &self.interner,
            |def_id| {
                if let Some(name) = def_names.get(&def_id) {
                    return name.clone();
                }
                let name = self
                    .input
                    .program_defs
                    .get(&def_id.module_id)
                    .and_then(|defs| defs.defs.get(def_id.def_id))
                    .or_else(|| defs.get(def_id.def_id))
                    .map(|def| sanitize_symbol_part(&def.name))
                    .unwrap_or_else(|| format!("def{}", def_id.def_id.0));
                def_names.insert(def_id, name.clone());
                name
            },
            |id| {
                let value = comptime.array_lengths.get(&id).copied();
                if value.is_none() && missing_array_len_diagnostics.insert(id) {
                    let span = const_exprs
                        .get(&id)
                        .map(|expr| expr.span)
                        .unwrap_or_default();
                    diagnostics.push(Diagnostic::user_error_at(
                        "E0601",
                        span,
                        format!(
                            "array length {id:?} was not evaluated before backend symbol generation"
                        ),
                    ));
                }
                value
            },
        )
    }

    pub(crate) fn def_name(&self, def_id: GlobalDefId) -> String {
        self.input
            .program_defs
            .get(&def_id.module_id)
            .and_then(|defs| defs.defs.get(def_id.def_id))
            .or_else(|| {
                (def_id.module_id == self.input.module_id)
                    .then(|| self.input.defs.defs.get(def_id.def_id))
                    .flatten()
            })
            .map(|def| def.name.clone())
            .unwrap_or_else(|| format!("def{}", def_id.def_id.0))
    }

    pub(crate) fn layout_of(&self, ty: InternedTyId) -> Option<nia_layout::TypeLayout> {
        let ty = self.input.type_normalization.normalize(ty);
        if let Some(layout) = self.input.layouts.types.get(&ty).cloned() {
            return Some(layout);
        }
        let Some(TyKind::Nominal { def_id, args }) = self.ty_kind(ty) else {
            return None;
        };
        if def_id.module_id != self.input.module_id {
            return None;
        }
        self.input.layouts.nominal_type_layout(*def_id, args)
    }

    fn error_ty(&self) -> InternedTyId {
        self.input.function_interner.error()
    }

    pub(crate) fn ty_kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        if ty.interner_id == self.interner.interner_id() {
            return self.interner.get(ty);
        }
        if let Some(extension_interner) = self.input.extension_interner
            && ty.interner_id == extension_interner.interner_id()
        {
            return extension_interner.get(ty);
        }
        self.known_interner_containing_ty(ty)
            .and_then(|interner| interner.get(ty))
    }

    pub(crate) fn known_interner_containing_ty(
        &self,
        ty: InternedTyId,
    ) -> Option<&nia_ty::TyInterner> {
        let mut error_candidate = None;
        let dynamic_interners = self
            .dynamic_type_interners
            .get(&ty.interner_id)
            .into_iter()
            .flat_map(|interners| interners.iter().rev());
        let shared_interners = self
            .shared
            .known_type_interners
            .get(&ty.interner_id)
            .into_iter()
            .flat_map(|interners| interners.iter().rev());
        for interner in dynamic_interners.chain(shared_interners) {
            match interner.get(ty) {
                Some(TyKind::Error) => {
                    error_candidate.get_or_insert(interner);
                }
                Some(_) => return Some(interner),
                None => {}
            }
        }
        error_candidate
    }

    fn remember_type_interner(&mut self, interner: &nia_ty::TyInterner) {
        insert_known_type_interner(&mut self.dynamic_type_interners, interner);
    }
}

fn refresh_known_backend_type_interners(lowerers: &mut [ModuleLowerer<'_>]) {
    for index in 0..lowerers.len() {
        refresh_known_backend_type_interner_from_source(lowerers, index);
    }
}

fn refresh_known_backend_type_interner_from_source(
    lowerers: &mut [ModuleLowerer<'_>],
    source_index: usize,
) {
    let Some(interner) = lowerers
        .get(source_index)
        .map(|lowerer| lowerer.interner.clone())
    else {
        return;
    };
    for lowerer in lowerers {
        lowerer.remember_type_interner(&interner);
    }
}

fn index_extension_generics_by_method(
    extensions: &VisibleExtensionMethods,
) -> HashMap<GlobalDefId, Vec<String>> {
    let mut generics_by_method = HashMap::new();
    for target in extensions.targets() {
        for method in &target.methods {
            generics_by_method.insert(method.def_id, method.impl_generics.clone());
        }
    }
    generics_by_method
}

fn index_shared_known_type_interners(
    modules: &[BackendLowerModuleInput<'_>],
    monomorphization: &Monomorphization,
) -> HashMap<ModuleId, Vec<nia_ty::TyInterner>> {
    let mut interners = HashMap::new();
    for input in modules {
        insert_known_type_interner(&mut interners, &input.body_ir.interner);
        insert_known_type_interner(&mut interners, input.function_interner);
        if let Some(interner) = input.extension_interner {
            insert_known_type_interner(&mut interners, interner);
        }
        for interner in input.program_type_interners.values() {
            insert_known_type_interner(&mut interners, interner);
        }
        for (_, interner) in input.program_extensions.values() {
            insert_known_type_interner(&mut interners, interner);
        }
    }
    for interner in monomorphization.type_interners.values() {
        insert_known_type_interner(&mut interners, interner);
    }
    interners
}

fn insert_known_type_interner(
    interners: &mut HashMap<ModuleId, Vec<nia_ty::TyInterner>>,
    interner: &nia_ty::TyInterner,
) {
    let candidates = interners.entry(interner.interner_id()).or_default();
    if !candidates.iter().any(|candidate| candidate == interner) {
        candidates.push(interner.clone());
    }
}

fn index_trait_impls_by_method(input: &BackendLowerModuleInput<'_>) -> HashMap<GlobalDefId, usize> {
    let impls = input
        .trait_impls
        .iter()
        .enumerate()
        .map(|(program_index, impl_signature)| {
            (
                (impl_signature.module_id, impl_signature.local_index),
                program_index,
            )
        })
        .collect::<HashMap<_, _>>();
    let mut impls_by_method = HashMap::new();
    for target in input.extensions.targets() {
        for method in &target.methods {
            let Some(program_index) = impls.get(&(input.module_id, method.impl_index)).copied()
            else {
                continue;
            };
            impls_by_method.insert(method.def_id, program_index);
        }
    }
    for method in input.program_extension_methods.all_methods() {
        let Some(program_index) = impls
            .get(&(method.def_id.module_id, method.impl_index))
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
                    method_name: method.name.clone(),
                    trait_arg_count: method.trait_args.len(),
                })
                .or_default()
                .push(ExtensionTraitMethodCandidate {
                    target_ty: target.target_ty,
                    method_def_id: method.def_id,
                    trait_args: method.trait_args.clone(),
                    where_predicates: method.where_predicates.clone(),
                    impl_generics: method.impl_generics.clone(),
                    source_interner: source_interner.clone(),
                });
        }
    }
    candidates
}

fn index_program_extension_trait_method_candidates(
    modules: &[BackendLowerModuleInput<'_>],
) -> HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>> {
    let Some(input) = modules.first() else {
        return HashMap::new();
    };
    let impls = input
        .trait_impls
        .iter()
        .map(|impl_signature| {
            (
                (impl_signature.module_id, impl_signature.local_index),
                impl_signature,
            )
        })
        .collect::<HashMap<_, _>>();
    let mut candidates: HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>> =
        HashMap::new();
    for method in input.program_extension_methods.all_methods() {
        let Some(trait_id) = method.trait_id else {
            continue;
        };
        let Some(impl_signature) = impls.get(&(method.def_id.module_id, method.impl_index)) else {
            continue;
        };
        candidates
            .entry(ExtensionTraitMethodKey {
                trait_id,
                method_name: method.name.clone(),
                trait_arg_count: method.trait_args.len(),
            })
            .or_default()
            .push(ExtensionTraitMethodCandidate {
                target_ty: method.target_ty,
                method_def_id: method.def_id,
                trait_args: method.trait_args.clone(),
                where_predicates: method.where_predicates.clone(),
                impl_generics: method.impl_generics.clone(),
                source_interner: impl_signature.interner.clone(),
            });
    }
    candidates
}

fn index_trait_methods_with_defaults(input: &BackendLowerModuleInput<'_>) -> HashSet<GlobalDefId> {
    let mut methods = input
        .signatures
        .traits
        .values()
        .flat_map(|signature| signature.methods.iter())
        .filter(|method| method.has_default)
        .map(|method| GlobalDefId {
            module_id: input.module_id,
            def_id: method.def_id,
        })
        .collect::<HashSet<_>>();
    methods.extend(
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
            }),
    );
    methods
}

fn index_method_names_by_def(input: &BackendLowerModuleInput<'_>) -> HashMap<GlobalDefId, String> {
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
                def.name.clone(),
            )
        })
        .collect::<HashMap<_, _>>();
    for target in input.extensions.targets() {
        for method in &target.methods {
            names
                .entry(method.def_id)
                .or_insert_with(|| method.name.clone());
        }
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

fn append_missing_layout_instances(
    output: &mut Vec<(BackendStructInstanceKey, nia_layout::StructLayout)>,
    computed: HashMap<StructLayoutKey, nia_layout::StructLayout>,
    default_module_id: ModuleId,
) {
    let mut existing = output
        .iter()
        .map(|(key, _)| key.clone())
        .collect::<HashSet<_>>();
    for (key, layout) in computed {
        let key = BackendStructInstanceKey::from_module_key(default_module_id, &key);
        if existing.insert(key.clone()) {
            output.push((key, layout));
        }
    }
}
#[cfg(test)]
mod tests;
