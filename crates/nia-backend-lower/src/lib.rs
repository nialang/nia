// SPDX-License-Identifier: GPL-3.0-or-later
mod function_instances;
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

use std::collections::{HashMap, HashSet};

use nia_ast::{Expr, ItemKind, Module};
use nia_backend_ir::{BackendLayouts, BackendModule, BackendProgram, BackendStructInstanceKey};
use nia_body_ir::BodyIr;
use nia_defs::{DefCollection, DefId, DefKind, VisibleExtensionMethods};
use nia_diagnostic::Diagnostic;
use nia_function_ir::FunctionBody;
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId, TraitId};
use nia_item_signatures::{
    ItemSignatures, ProgramEnumSignature, ProgramFunctionSignature, ProgramTraitImplSignature,
    ProgramTraitSignature,
};
use nia_layout::{Layouts, StructLayoutKey};
use nia_local_resolve::LocalResolution;
use nia_mangle::{mangle_instance_symbol, sanitize_symbol_part};
use nia_monomorphize::Monomorphization;
use nia_opt::{InlineThreshold, OptimizationDepth, OptimizationPolicy};
use nia_sema_ir::SemanticFacts;
use nia_span::Span;
use nia_trait_solve::TraitResolution;
use nia_ty::TyKind;
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_value_resolve::ValueResolution;

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
    pub semantic_facts: &'a SemanticFacts,
    pub extensions: &'a VisibleExtensionMethods,
    pub comptime: &'a nia_comptime_check::ComptimeCheck,
    pub layouts: &'a Layouts,
    pub function_bodies: &'a std::collections::HashMap<GlobalDefId, FunctionBody>,
    pub extension_interner: Option<&'a nia_ty::TyInterner>,
    pub program_extensions: &'a std::collections::HashMap<
        ModuleId,
        (&'a VisibleExtensionMethods, &'a nia_ty::TyInterner),
    >,
    pub program_type_interners: &'a std::collections::HashMap<ModuleId, &'a nia_ty::TyInterner>,
    pub program_functions: &'a std::collections::HashMap<GlobalDefId, ProgramFunctionSignature>,
    pub program_enums: &'a std::collections::HashMap<GlobalDefId, ProgramEnumSignature>,
    pub program_traits: &'a std::collections::HashMap<GlobalDefId, ProgramTraitSignature>,
    pub trait_impls: &'a [ProgramTraitImplSignature],
}

pub fn lower_backend_program(
    modules: &[BackendLowerModuleInput<'_>],
    monomorphization: &Monomorphization,
    optimization: OptimizationPolicy,
) -> BackendLowering {
    let mut diagnostics = Vec::new();
    let mut optimization_report = BackendOptimizationReport {
        enabled_module_passes: enabled_module_passes(&optimization),
        enabled_function_passes: opt::enabled_function_passes(&optimization),
        enabled_global_passes: enabled_global_passes(&optimization),
        changed_passes: Vec::new(),
    };
    let lowered_modules = modules
        .iter()
        .map(|input| {
            let mut lowerer = ModuleLowerer::new(input, monomorphization, optimization);
            let module = lowerer.lower_module();
            diagnostics.extend(lowerer.diagnostics);
            optimization_report
                .changed_passes
                .extend(lowerer.optimization_report.changed_passes);
            module
        })
        .collect();
    BackendLowering {
        program: BackendProgram {
            modules: lowered_modules,
        },
        optimization,
        optimization_report,
        diagnostics,
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
    pub(crate) monomorphization: &'a Monomorphization,
    pub(crate) optimization: OptimizationPolicy,
    pub(crate) interner: nia_ty::TyInterner,
    pub(crate) diagnostics: Vec<Diagnostic>,
    optimization_report: BackendOptimizationReport,
    missing_array_len_diagnostics: HashSet<GlobalConstExprId>,
    extension_targets_by_method: HashMap<GlobalDefId, InternedTyId>,
    extension_trait_method_candidates:
        HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>>,
    instance_extension_trait_method_candidates: Option<(
        ModuleId,
        HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>>,
    )>,
    instance_extension_interner: Option<&'a nia_ty::TyInterner>,
    struct_layout_instances_by_def: HashMap<DefId, Vec<StructLayoutKey>>,
    union_layout_instances_by_def: HashMap<DefId, Vec<StructLayoutKey>>,
    builtin_trait_resolutions: HashMap<BuiltinTraitGoalKey, TraitResolution>,
    trait_methods_with_defaults: HashSet<GlobalDefId>,
    type_instantiations: HashMap<TypeInstantiationKey, InternedTyId>,
    type_substitutions: Vec<HashMap<String, InternedTyId>>,
    type_substitution_ids: HashMap<TypeSubstitutionKey, TypeSubstitutionId>,
    effective_generics: HashMap<GlobalDefId, Vec<String>>,
    extension_ty_generics: HashMap<InternedTyId, Vec<String>>,
    generic_param_presence: HashMap<InternedTyId, bool>,
    def_names: HashMap<GlobalDefId, String>,
    method_names_by_def: HashMap<GlobalDefId, String>,
    trait_object_vtables: trait_object_vtables::TraitObjectVtableCache,
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
    source_interner: nia_ty::TyInterner,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TypeInstantiationKey {
    ty: InternedTyId,
    substitutions: TypeSubstitutionId,
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
    ) -> Self {
        Self {
            input,
            monomorphization,
            optimization,
            interner: input.body_ir.interner.clone(),
            diagnostics: Vec::new(),
            optimization_report: BackendOptimizationReport::default(),
            missing_array_len_diagnostics: HashSet::new(),
            extension_targets_by_method: index_extension_targets_by_method(input.extensions),
            extension_trait_method_candidates: index_extension_trait_method_candidates(
                input.extensions,
                input.extension_interner.unwrap_or(&input.body_ir.interner),
            ),
            instance_extension_trait_method_candidates: None,
            instance_extension_interner: None,
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
            extension_ty_generics: HashMap::new(),
            generic_param_presence: HashMap::new(),
            def_names: HashMap::new(),
            method_names_by_def: index_method_names_by_def(input),
            trait_object_vtables: trait_object_vtables::TraitObjectVtableCache::default(),
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
        let mut trait_object_vtables = Vec::new();

        for item in &self.input.module.items {
            match &item.kind {
                ItemKind::Struct(item_struct) => {
                    if item_struct.generics.is_empty()
                        && let Some(item) = self.lower_struct(item.span, item_struct)
                    {
                        structs.push(item);
                    }
                    struct_instances.extend(self.lower_struct_instances(item.span, item_struct));
                }
                ItemKind::Union(item_union) => {
                    if item_union.generics.is_empty()
                        && let Some(item) = self.lower_union(item.span, item_union)
                    {
                        unions.push(item);
                    }
                    union_instances.extend(self.lower_union_instances(item.span, item_union));
                }
                ItemKind::Trait(item_trait) => {
                    for method in &item_trait.methods {
                        if method.function.body.is_some()
                            && let Some(function) =
                                self.lower_function(method.function.span, &method.function)
                        {
                            function_templates.push(function);
                        }
                    }
                }
                ItemKind::Extend(extend) => {
                    let extend_target_is_generic = self.extend_target_has_generics(extend);
                    for method in &extend.methods {
                        if let Some(function) =
                            self.lower_function(method.function.span, &method.function)
                        {
                            if !extend_target_is_generic && function.generics.is_empty() {
                                functions.push(function.clone());
                            }
                            function_templates.push(function);
                        }
                    }
                }
                ItemKind::Enum(item_enum) => {
                    if let Some(item) = self.lower_enum(item.span, item_enum) {
                        enums.push(item);
                    }
                }
                ItemKind::Function(function) => {
                    if let Some(function) = self.lower_function(item.span, function) {
                        if function.generics.is_empty() {
                            functions.push(function.clone());
                        }
                        function_templates.push(function);
                    }
                }
                ItemKind::Binding(binding) => {
                    if binding.is_comptime {
                        continue;
                    }
                    if let Some(global) = self.lower_global(item.span, binding) {
                        globals.push(global);
                    }
                }
                ItemKind::Import(_)
                | ItemKind::Using(_)
                | ItemKind::ComptimeIf(_)
                | ItemKind::TypeAlias(_) => {}
            }
        }

        let mut function_instances = self.lower_function_instances(&function_templates);
        self.collect_trait_object_vtables(
            &mut trait_object_vtables,
            &functions,
            &function_instances,
        );
        self.devirtualize_direct_trait_calls(&mut functions, &mut function_instances);
        self.propagate_cross_function_constants(&mut functions, &mut function_instances);
        self.inline_leaf_functions(&mut functions, &mut function_instances);
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
        self.extend_backend_layouts_for_instances(&mut backend_layouts);

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

    fn extend_backend_layouts_for_instances(&self, layouts: &mut BackendLayouts) {
        let computed = nia_layout::compute_layouts_with_normalized_types(
            self.input.defs,
            &self.interner,
            self.input.signatures,
            &self.input.type_normalization.normalized,
            &|id| self.input.comptime.array_lengths.get(&id).copied(),
            self.input.layouts.target,
        );
        append_missing_layout_instances(
            self.input.module_id,
            &mut layouts.struct_instances,
            computed.struct_instances,
        );
        append_missing_layout_instances(
            self.input.module_id,
            &mut layouts.union_instances,
            computed.union_instances,
        );
    }

    fn extend_target_has_generics(&self, extend: &nia_ast::ExtendItem) -> bool {
        let Some(ty) = self
            .input
            .type_lowering
            .node_type_uses
            .get(&extend.target.node_key)
            .copied()
        else {
            return !extend.generics.is_empty();
        };
        !self.generic_params_in_ty(ty).is_empty()
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

    fn def_id_for_span(&mut self, span: Span, expected: DefKind) -> Option<DefId> {
        let def_id = self.input.defs.def_spans.get(span)?;
        let def = self.input.defs.defs.get(def_id)?;
        if def.kind == expected {
            Some(def_id)
        } else {
            None
        }
    }

    fn def_id_for_span_any_function(&mut self, span: Span) -> Option<DefId> {
        let def_id = self.input.defs.def_spans.get(span)?;
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
                let name = defs
                    .get(def_id.def_id)
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
                    diagnostics.push(Diagnostic::error(
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
        self.input.body_ir.interner.error()
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
        None
    }
}

fn index_extension_targets_by_method(
    extensions: &VisibleExtensionMethods,
) -> HashMap<GlobalDefId, InternedTyId> {
    let mut targets_by_method = HashMap::new();
    for target in extensions.targets() {
        for method in &target.methods {
            targets_by_method.insert(method.def_id, target.target_ty);
        }
    }
    targets_by_method
}

fn index_extension_trait_method_candidates(
    extensions: &VisibleExtensionMethods,
    source_interner: &nia_ty::TyInterner,
) -> HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>> {
    let mut candidates: HashMap<ExtensionTraitMethodKey, Vec<ExtensionTraitMethodCandidate>> =
        HashMap::new();
    for target in extensions.targets() {
        for method in &target.methods {
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
                    source_interner: source_interner.clone(),
                });
        }
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
    module_id: ModuleId,
    output: &mut Vec<(BackendStructInstanceKey, nia_layout::StructLayout)>,
    computed: HashMap<StructLayoutKey, nia_layout::StructLayout>,
) {
    let mut existing = output
        .iter()
        .map(|(key, _)| key.clone())
        .collect::<HashSet<_>>();
    for (key, layout) in computed {
        let key = BackendStructInstanceKey::from_module_key(module_id, &key);
        if existing.insert(key.clone()) {
            output.push((key, layout));
        }
    }
}
#[cfg(test)]
mod tests;
