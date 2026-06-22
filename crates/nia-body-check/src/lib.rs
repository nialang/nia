// SPDX-License-Identifier: GPL-3.0-or-later
use std::fmt;
use std::time::Instant;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

mod aggregates;
mod bir;
mod calls;
mod expr;
mod helpers;
mod literals;
mod places;
mod projection_obligations;
mod static_init;
mod trait_objects;
mod type_support;

pub use nia_ty::import_type_into;

use nia_ast::{BindingStmt, Block, Expr, ExprKind, FunctionItem, Module, Stmt, StmtKind};
use nia_body_ir::BodyIr;
use nia_comptime_check::ComptimeCheck;
use nia_comptime_ir::ResolvedComptimeModule;
use nia_defs::{DefCollection, DefId, DefKind, ExtensionMethods, VisibleExtensionMethods};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalDefId, InternedTyId, LocalId, ModuleId, ReceiverKind};
use nia_item_signatures::{
    EnumSignature, FunctionSignature, ItemSignatures, ProgramComptimeSignature,
    ProgramEnumSignature, ProgramFunctionSignature, ProgramGlobalSignature, ProgramSignatureMaps,
    ProgramStructSignature, ProgramTraitImplSignature, ProgramTraitSignature,
    ProgramTypeAliasSignature, ProgramUnionSignature, StructSignature, UnionSignature,
};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNodeKind, ModuleItemTree};
use nia_layout::Layouts;
use nia_local_resolve::LocalResolution;
use nia_node_id::{NodeKey, NodeOriginTable};
use nia_sema_ir::{
    ArrayToSliceCoercion, BracketSuffixResolution, BuiltinValue, FunctionReference,
    FunctionSemanticFacts, GenericInstantiation, PointerArrayToSliceCoercion, ResolvedCall,
    SemanticFacts, SemanticUseTable, SemanticValueUse, TraitObjectCoercion, TraitObjectUpcast,
};
use nia_source::{SourcePath, SourceVersion};
use nia_span::Span;
use nia_target_config::TargetConfig;
use nia_ty::{PrimitiveTy, TyInterner, TyKind};
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_value_resolve::ValueResolution;

#[derive(Debug, Clone, PartialEq)]
pub struct BodyCheck {
    pub ir: BodyIr,
    pub facts: SemanticFacts,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SwitchInterval {
    start: i128,
    end: i128,
    span: Span,
}

struct RangePatternCheck<'a> {
    span: Span,
    start: &'a Expr,
    end: &'a Expr,
    inclusive: bool,
}

#[derive(Debug, Clone, Default)]
struct PatternCoverage {
    catch_all: Option<Span>,
    optional_null: Option<Span>,
    optional_some: Option<Box<PatternCoverage>>,
    error_ok: Option<Box<PatternCoverage>>,
    error_err: Option<Box<PatternCoverage>>,
}

#[derive(Debug, Clone, Default)]
struct SwitchCoverage {
    catch_all: Option<Span>,
    intervals: Vec<SwitchInterval>,
    enum_variants: HashMap<DefId, Span>,
}

#[derive(Debug, Clone, Copy)]
pub struct ProgramComptimeMaps<'a> {
    pub comptimes: &'a HashMap<ModuleId, ComptimeCheck>,
    pub modules: &'a HashMap<ModuleId, ResolvedComptimeModule>,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum BodyCheckFilter<'a> {
    #[default]
    All,
    ReachableFunctions(&'a HashSet<GlobalDefId>),
}

impl BodyCheckFilter<'_> {
    fn includes(self, def_id: GlobalDefId) -> bool {
        match self {
            Self::All => true,
            Self::ReachableFunctions(functions) => functions.contains(&def_id),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BodyTimingMode {
    #[default]
    Off,
    Detail,
}

impl BodyTimingMode {
    fn enabled(self) -> bool {
        matches!(self, Self::Detail)
    }
}

#[derive(Clone, Copy)]
pub struct BodyProgramContext<'a> {
    pub defs: Option<&'a HashMap<ModuleId, DefCollection>>,
    pub type_lowerings: Option<&'a HashMap<ModuleId, TypeLowering>>,
    pub type_normalizations: Option<&'a HashMap<ModuleId, TypeNormalization>>,
    pub signatures: Option<&'a HashMap<ModuleId, ItemSignatures>>,
    pub layouts: Option<&'a dyn Fn(ModuleId) -> Option<Layouts>>,
}

impl<'a> BodyProgramContext<'a> {
    pub fn empty() -> Self {
        Self {
            defs: None,
            type_lowerings: None,
            type_normalizations: None,
            signatures: None,
            layouts: None,
        }
    }
}

impl fmt::Debug for BodyProgramContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BodyProgramContext")
            .field("defs", &self.defs.is_some())
            .field("type_lowerings", &self.type_lowerings.is_some())
            .field("type_normalizations", &self.type_normalizations.is_some())
            .field("signatures", &self.signatures.is_some())
            .field("layouts", &self.layouts.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BodyCheckInput<'a> {
    pub source_version: Option<SourceVersion>,
    pub source_path: &'a SourcePath,
    pub origins: &'a NodeOriginTable,
    pub active_item_tree: &'a ActiveModuleItemTree,
    pub defs: &'a DefCollection,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub semantic_uses: &'a SemanticUseTable,
    pub lowered: &'a TypeLowering,
    pub signatures: &'a ItemSignatures,
    pub normalization: &'a TypeNormalization,
    pub target: &'a TargetConfig,
    pub comptime: &'a ComptimeCheck,
    pub comptime_module: &'a ResolvedComptimeModule,
    pub layouts: &'a Layouts,
    pub extensions: &'a VisibleExtensionMethods,
    pub program_extension_methods: &'a ExtensionMethods,
    pub extension_interner: Option<&'a TyInterner>,
    pub program: BodyProgramContext<'a>,
    pub program_signatures: ProgramSignatureMaps<'a>,
    pub program_comptime: ProgramComptimeMaps<'a>,
    pub filter: BodyCheckFilter<'a>,
}

#[derive(Debug, Clone, Copy)]
pub struct BodyCheckWithProgramSignaturesInput<'a> {
    pub source_version: Option<SourceVersion>,
    pub source_path: &'a SourcePath,
    pub origins: &'a NodeOriginTable,
    pub active_item_tree: &'a ActiveModuleItemTree,
    pub defs: &'a DefCollection,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub semantic_uses: &'a SemanticUseTable,
    pub lowered: &'a TypeLowering,
    pub signatures: &'a ItemSignatures,
    pub normalization: &'a TypeNormalization,
    pub target: &'a TargetConfig,
    pub comptime: &'a ComptimeCheck,
    pub comptime_module: &'a ResolvedComptimeModule,
    pub extensions: &'a VisibleExtensionMethods,
    pub program_extension_methods: &'a ExtensionMethods,
    pub program: BodyProgramContext<'a>,
    pub program_signatures: ProgramSignatureMaps<'a>,
}

#[derive(Debug, Clone)]
struct ResolvedFunctionSignature {
    def_id: GlobalDefId,
    signature: FunctionSignature,
}

pub fn check_module_bodies(
    module: &Module,
    defs: &DefCollection,
    values: &ValueResolution,
    locals: &LocalResolution,
    lowered: &TypeLowering,
    signatures: &ItemSignatures,
) -> BodyCheck {
    let empty_normalization = TypeNormalization {
        interner: lowered.interner.clone(),
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let layouts = nia_layout::compute_layouts(
        defs,
        &lowered.interner,
        signatures,
        nia_layout::TargetDataLayout::LP64,
    );
    let empty_functions = HashMap::new();
    let empty_globals = HashMap::new();
    let empty_comptimes = HashMap::new();
    let empty_program_comptime = HashMap::new();
    let empty_program_comptime_modules = HashMap::new();
    let empty_comptime_module = ResolvedComptimeModule::default();
    let empty_structs = HashMap::new();
    let empty_unions = HashMap::new();
    let empty_enums = HashMap::new();
    let empty_traits = HashMap::new();
    let empty_type_aliases = HashMap::new();
    let empty_trait_impls = Vec::new();
    let empty_extensions = VisibleExtensionMethods::default();
    let empty_program_extension_methods = ExtensionMethods::default();
    let empty_comptime = ComptimeCheck::default();
    let target = TargetConfig::host();
    let source_path = SourcePath::new("main.nia");
    let semantic_uses = semantic_use_table_for_body_input(defs.module_id, values, locals, lowered);
    let item_tree = ModuleItemTree::from_module(module);
    let active_item_tree = ActiveModuleItemTree::new(
        item_tree.active_items_without_comptime(),
        Default::default(),
    );
    let mut checked = check_module_bodies_with_layouts(BodyCheckInput {
        source_version: None,
        source_path: &source_path,
        origins: &NodeOriginTable::default(),
        active_item_tree: &active_item_tree,
        defs,
        values,
        locals,
        semantic_uses: &semantic_uses,
        lowered,
        signatures,
        normalization: &empty_normalization,
        target: &target,
        comptime: &empty_comptime,
        comptime_module: &empty_comptime_module,
        layouts: &layouts,
        extensions: &empty_extensions,
        program_extension_methods: &empty_program_extension_methods,
        extension_interner: None,
        program: BodyProgramContext::empty(),
        program_signatures: ProgramSignatureMaps {
            functions: &empty_functions,
            globals: &empty_globals,
            comptimes: &empty_comptimes,
            structs: &empty_structs,
            unions: &empty_unions,
            enums: &empty_enums,
            traits: &empty_traits,
            type_aliases: &empty_type_aliases,
            trait_impls: &empty_trait_impls,
        },
        program_comptime: ProgramComptimeMaps {
            comptimes: &empty_program_comptime,
            modules: &empty_program_comptime_modules,
        },
        filter: BodyCheckFilter::All,
    });
    checked.diagnostics.extend(layouts.diagnostics);
    checked
}

pub fn check_module_bodies_with_layouts(input: BodyCheckInput<'_>) -> BodyCheck {
    check_module_bodies_with_program_signatures_and_layouts(input)
}

pub fn check_module_bodies_with_program_signatures(
    input: BodyCheckWithProgramSignaturesInput<'_>,
) -> BodyCheck {
    let layouts = nia_layout::compute_layouts_with_normalized_types(
        input.defs,
        &input.normalization.interner,
        input.signatures,
        &input.normalization.normalized,
        &|id| input.comptime.array_lengths.get(&id).copied(),
        nia_layout::TargetDataLayout::LP64,
    );
    let mut checked = check_module_bodies_with_layouts(BodyCheckInput {
        source_version: input.source_version,
        source_path: input.source_path,
        origins: input.origins,
        active_item_tree: input.active_item_tree,
        defs: input.defs,
        values: input.values,
        locals: input.locals,
        semantic_uses: input.semantic_uses,
        lowered: input.lowered,
        signatures: input.signatures,
        normalization: input.normalization,
        target: input.target,
        comptime: input.comptime,
        comptime_module: input.comptime_module,
        layouts: &layouts,
        extensions: input.extensions,
        program_extension_methods: input.program_extension_methods,
        extension_interner: None,
        program: input.program,
        program_signatures: input.program_signatures,
        program_comptime: ProgramComptimeMaps {
            comptimes: &HashMap::new(),
            modules: &HashMap::new(),
        },
        filter: BodyCheckFilter::All,
    });
    checked.diagnostics.extend(layouts.diagnostics);
    checked
}

fn semantic_use_table_for_body_input(
    module_id: ModuleId,
    values: &ValueResolution,
    locals: &LocalResolution,
    lowered: &TypeLowering,
) -> SemanticUseTable {
    let mut builder = SemanticUseTable::builder();
    for (key, local_use) in &locals.node_uses {
        if let nia_local_resolve::LocalUse::Local(local_id) = local_use {
            builder.insert_node_local_value_use(key.clone(), *local_id);
        }
    }
    builder.extend_node_global_value_uses(
        values
            .node_qualified_values
            .iter()
            .map(|(key, global_id)| (key.clone(), *global_id)),
    );
    for (key, resolution) in &values.node_names {
        match resolution {
            nia_value_resolve::ValueNameResolution::Def(def_id) => {
                builder.insert_node_global_value_use(
                    key.clone(),
                    GlobalDefId {
                        module_id,
                        def_id: *def_id,
                    },
                );
            }
            nia_value_resolve::ValueNameResolution::External(global_id) => {
                builder.insert_node_global_value_use(key.clone(), *global_id);
            }
            nia_value_resolve::ValueNameResolution::Module
            | nia_value_resolve::ValueNameResolution::LocalDeferred
            | nia_value_resolve::ValueNameResolution::Error => {}
        }
    }
    builder.extend_node_local_defs(
        locals
            .node_local_defs
            .iter()
            .map(|(key, local_id)| (key.clone(), *local_id)),
    );
    builder.extend_node_type_uses(
        lowered
            .node_type_uses
            .iter()
            .map(|(key, ty)| (key.clone(), *ty)),
    );
    builder.finish()
}

pub fn check_module_bodies_with_program_signatures_and_layouts(
    input: BodyCheckInput<'_>,
) -> BodyCheck {
    check_module_bodies_with_program_signatures_and_layouts_with_timings(input, BodyTimingMode::Off)
}

pub fn check_module_bodies_with_program_signatures_and_layouts_with_timings(
    input: BodyCheckInput<'_>,
    timings: BodyTimingMode,
) -> BodyCheck {
    let timing = timings.enabled();
    let module_id = input.defs.module_id;
    let mut interner = input
        .extension_interner
        .cloned()
        .unwrap_or_else(|| input.normalization.interner.clone());
    let extension_methods_by_id = BodyChecker::extension_method_lookup(
        module_id,
        input.signatures,
        input.extensions,
        input.program_extension_methods,
        &mut interner,
        &input.lowered.interner,
        input.normalization,
        input.program.type_normalizations,
    );
    let void_ty = interner.primitive(PrimitiveTy::Void);
    let mut checker = time_body_stage(timing, "body_check.init", module_id, || BodyChecker {
        active_item_tree: input.active_item_tree,
        defs: input.defs,
        program: input.program,
        values: input.values,
        locals: input.locals,
        semantic_uses: input.semantic_uses,
        interner,
        type_lowering: input.lowered,
        node_type_uses: &input.lowered.node_type_uses,
        signatures: input.signatures,
        normalization: input.normalization,
        target: input.target,
        comptime: input.comptime,
        comptime_module: input.comptime_module,
        layouts: input.layouts,
        extensions: input.extensions,
        program_extension_methods: input.program_extension_methods,
        program_functions: input.program_signatures.functions,
        program_globals: input.program_signatures.globals,
        program_comptimes: input.program_signatures.comptimes,
        program_structs: input.program_signatures.structs,
        program_unions: input.program_signatures.unions,
        program_enums: input.program_signatures.enums,
        program_traits: input.program_signatures.traits,
        program_type_aliases: input.program_signatures.type_aliases,
        program_trait_impls: input.program_signatures.trait_impls,
        program_comptime: input.program_comptime.comptimes,
        program_comptime_modules: input.program_comptime.modules,
        source_path: input.source_path,
        extension_methods_by_id,
        node_expr_types: HashMap::new(),
        node_bracket_suffix_resolutions: HashMap::new(),
        node_array_to_slice_coercions: HashMap::new(),
        node_pointer_array_to_slice_coercions: HashMap::new(),
        node_trait_object_coercions: HashMap::new(),
        node_trait_object_upcasts: HashMap::new(),
        node_builtin_values: HashMap::new(),
        node_array_repeat_counts: HashMap::new(),
        node_switch_pattern_values: HashMap::new(),
        node_resolved_calls: HashMap::new(),
        node_function_references: HashMap::new(),
        generic_instantiations: Vec::new(),
        function_facts: HashMap::new(),
        function_bodies: HashMap::new(),
        global_inits: HashMap::new(),
        local_types: HashMap::new(),
        global_types: HashMap::new(),
        comptime_types: HashMap::new(),
        method_receiver_kinds: HashMap::new(),
        traits_by_method_name: HashMap::new(),
        trait_impls_by_trait: HashMap::new(),
        diagnostics: Vec::new(),
        timing,
        timing_module_id: module_id,
        current_return: void_ty,
        current_def_id: None,
        current_param_locals: Vec::new(),
        comptime_context_depth: 0,
        comptime_call_locals: Vec::new(),
        body_filter: input.filter,
    });
    time_body_stage(timing, "body_check.seed_global_types", module_id, || {
        checker.seed_global_types();
    });
    time_body_stage(timing, "body_check.check_module", module_id, || {
        checker.check_module(input.active_item_tree, timing, module_id);
    });
    time_body_stage(timing, "body_check.finish", module_id, || BodyCheck {
        ir: BodyIr {
            interner: checker.interner,
            function_bodies: checker.function_bodies,
            global_inits: checker.global_inits,
        },
        facts: SemanticFacts {
            local_types: checker.local_types,
            global_types: checker
                .global_types
                .into_iter()
                .map(|(def_id, ty)| (GlobalDefId { module_id, def_id }, ty))
                .collect(),
            generic_instantiations: checker.generic_instantiations,
            function_facts: checker.function_facts,
            node_expr_types: checker.node_expr_types,
            node_bracket_suffix_resolutions: checker.node_bracket_suffix_resolutions,
            node_array_to_slice_coercions: checker.node_array_to_slice_coercions,
            node_pointer_array_to_slice_coercions: checker.node_pointer_array_to_slice_coercions,
            node_trait_object_coercions: checker.node_trait_object_coercions,
            node_trait_object_upcasts: checker.node_trait_object_upcasts,
            node_builtin_values: checker.node_builtin_values,
            node_builtin_associated_values: input
                .semantic_uses
                .node_builtin_associated_values
                .clone(),
            node_array_repeat_counts: checker.node_array_repeat_counts,
            node_switch_pattern_values: checker.node_switch_pattern_values,
            node_resolved_calls: checker.node_resolved_calls,
            node_function_references: checker.node_function_references,
        },
        diagnostics: checker.diagnostics,
    })
}

fn time_body_stage<T>(enabled: bool, name: &str, module_id: ModuleId, f: impl FnOnce() -> T) -> T {
    if !enabled {
        return f();
    }
    let start = Instant::now();
    let result = f();
    eprintln!(
        "query timing {name}[{module_id:?}]: {:.3}s",
        start.elapsed().as_secs_f64()
    );
    result
}

fn time_body_stage_if_slow<T>(
    enabled: bool,
    name: &str,
    module_id: ModuleId,
    detail: &str,
    threshold_seconds: f64,
    f: impl FnOnce() -> T,
) -> T {
    if !enabled {
        return f();
    }
    let start = Instant::now();
    let result = f();
    let elapsed = start.elapsed().as_secs_f64();
    if elapsed >= threshold_seconds {
        eprintln!("query timing {name}[{module_id:?} {detail}]: {elapsed:.3}s");
    }
    result
}

struct BodyChecker<'a> {
    active_item_tree: &'a ActiveModuleItemTree,
    defs: &'a DefCollection,
    program: BodyProgramContext<'a>,
    values: &'a ValueResolution,
    locals: &'a LocalResolution,
    semantic_uses: &'a SemanticUseTable,
    interner: TyInterner,
    type_lowering: &'a TypeLowering,
    node_type_uses: &'a HashMap<NodeKey, InternedTyId>,
    signatures: &'a ItemSignatures,
    normalization: &'a TypeNormalization,
    target: &'a TargetConfig,
    comptime: &'a ComptimeCheck,
    comptime_module: &'a ResolvedComptimeModule,
    layouts: &'a Layouts,
    extensions: &'a VisibleExtensionMethods,
    program_extension_methods: &'a ExtensionMethods,
    program_functions: &'a HashMap<GlobalDefId, ProgramFunctionSignature>,
    program_globals: &'a HashMap<GlobalDefId, ProgramGlobalSignature>,
    program_comptimes: &'a HashMap<GlobalDefId, ProgramComptimeSignature>,
    program_structs: &'a HashMap<GlobalDefId, ProgramStructSignature>,
    program_unions: &'a HashMap<GlobalDefId, ProgramUnionSignature>,
    program_enums: &'a HashMap<GlobalDefId, ProgramEnumSignature>,
    program_traits: &'a HashMap<GlobalDefId, ProgramTraitSignature>,
    program_type_aliases: &'a HashMap<GlobalDefId, ProgramTypeAliasSignature>,
    program_trait_impls: &'a [ProgramTraitImplSignature],
    program_comptime: &'a HashMap<ModuleId, ComptimeCheck>,
    program_comptime_modules: &'a HashMap<ModuleId, ResolvedComptimeModule>,
    source_path: &'a SourcePath,
    extension_methods_by_id: Arc<HashMap<GlobalDefId, ExtensionMethodLookup>>,
    node_expr_types: HashMap<NodeKey, InternedTyId>,
    node_bracket_suffix_resolutions: HashMap<NodeKey, BracketSuffixResolution>,
    node_array_to_slice_coercions: HashMap<NodeKey, ArrayToSliceCoercion>,
    node_pointer_array_to_slice_coercions: HashMap<NodeKey, PointerArrayToSliceCoercion>,
    node_trait_object_coercions: HashMap<NodeKey, TraitObjectCoercion>,
    node_trait_object_upcasts: HashMap<NodeKey, TraitObjectUpcast>,
    node_builtin_values: HashMap<NodeKey, BuiltinValue>,
    node_array_repeat_counts: HashMap<NodeKey, u64>,
    node_switch_pattern_values: HashMap<NodeKey, i128>,
    node_resolved_calls: HashMap<NodeKey, ResolvedCall>,
    node_function_references: HashMap<NodeKey, FunctionReference>,
    generic_instantiations: Vec<GenericInstantiation>,
    function_facts: HashMap<GlobalDefId, FunctionSemanticFacts>,
    function_bodies: HashMap<GlobalDefId, nia_body_ir::TypedBody>,
    global_inits: HashMap<GlobalDefId, nia_static_ir::StaticInit>,
    local_types: HashMap<LocalId, InternedTyId>,
    global_types: HashMap<DefId, InternedTyId>,
    comptime_types: HashMap<DefId, InternedTyId>,
    method_receiver_kinds: HashMap<GlobalDefId, Option<ReceiverKind>>,
    traits_by_method_name: HashMap<String, Vec<GlobalDefId>>,
    trait_impls_by_trait: HashMap<nia_ty::TraitId, Vec<usize>>,
    diagnostics: Vec<Diagnostic>,
    timing: bool,
    timing_module_id: ModuleId,
    current_return: InternedTyId,
    current_def_id: Option<GlobalDefId>,
    current_param_locals: Vec<LocalId>,
    comptime_context_depth: usize,
    comptime_call_locals: Vec<ComptimeCallFrame>,
    body_filter: BodyCheckFilter<'a>,
}

#[derive(Debug, Clone)]
struct ExtensionMethodLookup {
    target_ty: InternedTyId,
    impl_generics: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct ComptimeCallFrame {
    module_id: Option<ModuleId>,
    function_id: Option<GlobalDefId>,
    locals: HashMap<LocalId, nia_comptime_check::ComptimeValue>,
    local_types: HashMap<LocalId, nia_comptime_check::ComptimeValueType>,
    mutable_locals: HashSet<LocalId>,
    type_substitutions: HashMap<String, InternedTyId>,
}

#[derive(Debug, Clone, PartialEq)]
struct ReceiverBase {
    def_id: GlobalDefId,
    args: Vec<InternedTyId>,
    from_pointer: bool,
    has_readonly_pointer: bool,
}

impl<'a> BodyChecker<'a> {
    fn extension_method_lookup(
        module_id: ModuleId,
        signatures: &ItemSignatures,
        extensions: &VisibleExtensionMethods,
        program_extensions: &ExtensionMethods,
        interner: &mut TyInterner,
        local_type_interner: &TyInterner,
        local_normalization: &TypeNormalization,
        program_normalizations: Option<&HashMap<ModuleId, TypeNormalization>>,
    ) -> Arc<HashMap<GlobalDefId, ExtensionMethodLookup>> {
        let mut methods = HashMap::new();
        for impl_signature in &signatures.trait_impls {
            let target_ty = local_normalization.normalize(impl_signature.target_ty);
            let target_ty = nia_ty::import_type_into(interner, local_type_interner, target_ty);
            for method in &impl_signature.methods {
                methods.insert(
                    GlobalDefId {
                        module_id,
                        def_id: method.def_id,
                    },
                    ExtensionMethodLookup {
                        target_ty,
                        impl_generics: impl_signature.generics.clone(),
                    },
                );
            }
        }
        for target in extensions.targets() {
            for method in &target.methods {
                methods
                    .entry(method.def_id)
                    .or_insert_with(|| ExtensionMethodLookup {
                        target_ty: target.target_ty,
                        impl_generics: method.impl_generics.clone(),
                    });
            }
        }
        for method in program_extensions.all_methods() {
            if methods.contains_key(&method.def_id) {
                continue;
            }
            let target_ty = program_normalizations
                .and_then(|normalizations| normalizations.get(&method.def_id.module_id))
                .map(|normalization| {
                    let target_ty = normalization.normalize(method.target_ty);
                    nia_ty::import_type_into(interner, &normalization.interner, target_ty)
                })
                .unwrap_or(method.target_ty);
            methods.insert(
                method.def_id,
                ExtensionMethodLookup {
                    target_ty,
                    impl_generics: method.impl_generics.clone(),
                },
            );
        }
        Arc::new(methods)
    }

    fn record_expr_node_type(&mut self, expr: &Expr, ty: InternedTyId) {
        let ty = self.import_type_to_working_interner(ty);
        let ty = self.normalize_projection(ty);
        self.node_expr_types.insert(expr.node_key.clone(), ty);
        if let Some(facts) = self.current_function_facts() {
            facts.node_expr_types.insert(expr.node_key.clone(), ty);
        }
    }

    fn record_bracket_suffix_node_resolution(
        &mut self,
        expr: &Expr,
        resolution: BracketSuffixResolution,
    ) {
        self.node_bracket_suffix_resolutions
            .insert(expr.node_key.clone(), resolution);
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_bracket_suffix_resolutions
                .insert(expr.node_key.clone(), resolution);
        }
    }

    fn record_resolved_node_call(&mut self, _span: Span, key: &NodeKey, call: ResolvedCall) {
        self.node_resolved_calls.insert(key.clone(), call.clone());
        if let Some(facts) = self.current_function_facts() {
            facts.node_resolved_calls.insert(key.clone(), call);
        }
    }

    fn record_array_to_slice_node_coercion(&mut self, expr: &Expr, coercion: ArrayToSliceCoercion) {
        self.node_array_to_slice_coercions
            .insert(expr.node_key.clone(), coercion);
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_array_to_slice_coercions
                .insert(expr.node_key.clone(), coercion);
        }
    }

    fn record_pointer_array_to_slice_node_coercion(
        &mut self,
        expr: &Expr,
        coercion: PointerArrayToSliceCoercion,
    ) {
        self.node_pointer_array_to_slice_coercions
            .insert(expr.node_key.clone(), coercion);
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_pointer_array_to_slice_coercions
                .insert(expr.node_key.clone(), coercion);
        }
    }

    fn record_trait_object_node_coercion(&mut self, expr: &Expr, coercion: TraitObjectCoercion) {
        self.node_trait_object_coercions
            .insert(expr.node_key.clone(), coercion);
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_trait_object_coercions
                .insert(expr.node_key.clone(), coercion);
        }
    }

    fn record_trait_object_node_upcast(&mut self, expr: &Expr, upcast: TraitObjectUpcast) {
        self.node_trait_object_upcasts
            .insert(expr.node_key.clone(), upcast);
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_trait_object_upcasts
                .insert(expr.node_key.clone(), upcast);
        }
    }

    fn record_builtin_node_value(&mut self, expr: &Expr, value: BuiltinValue) {
        self.node_builtin_values
            .insert(expr.node_key.clone(), value.clone());
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_builtin_values
                .insert(expr.node_key.clone(), value);
        }
    }

    fn record_function_node_reference(
        &mut self,
        _span: Span,
        key: &NodeKey,
        reference: FunctionReference,
    ) {
        self.node_function_references
            .insert(key.clone(), reference.clone());
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_function_references
                .insert(key.clone(), reference);
        }
    }

    fn record_array_repeat_count(&mut self, expr: &Expr, value: u64) {
        self.node_array_repeat_counts
            .insert(expr.node_key.clone(), value);
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_array_repeat_counts
                .insert(expr.node_key.clone(), value);
        }
    }

    fn record_switch_pattern_value(&mut self, expr: &Expr, value: i128) {
        self.node_switch_pattern_values
            .insert(expr.node_key.clone(), value);
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_switch_pattern_values
                .insert(expr.node_key.clone(), value);
        }
    }

    fn record_local_type(&mut self, local_id: LocalId, ty: InternedTyId) {
        let ty = self.import_type_to_working_interner(ty);
        let ty = self.normalize_aliases_in_type(ty);
        self.local_types.insert(local_id, ty);
        if let Some(facts) = self.current_function_facts() {
            facts.local_types.insert(local_id, ty);
        }
    }

    fn import_type_to_working_interner(&mut self, ty: InternedTyId) -> InternedTyId {
        if ty.interner_id == self.interner.interner_id() {
            ty
        } else if ty.interner_id == self.normalization.interner.interner_id() {
            nia_ty::import_type_into(&mut self.interner, &self.normalization.interner, ty)
        } else {
            ty
        }
    }

    fn current_function_facts(&mut self) -> Option<&mut FunctionSemanticFacts> {
        self.current_def_id
            .map(|def_id| self.function_facts.entry(def_id).or_default())
    }

    fn expr_ty(&mut self, expr: &Expr) -> Option<InternedTyId> {
        if let Some(ty) = self.node_expr_types.get(&expr.node_key).copied() {
            return Some(ty);
        }
        if let Some(nia_local_resolve::LocalUse::Local(local_id)) = self.local_use(expr)
            && let Some(ty) = self.local_types.get(&local_id).copied()
        {
            return Some(ty);
        }
        let ty = self.node_type_uses.get(&expr.node_key).copied()?;
        Some(self.import_type_to_working_interner(ty))
    }

    fn bracket_suffix_resolution(&self, expr: &Expr) -> Option<BracketSuffixResolution> {
        self.node_bracket_suffix_resolutions
            .get(&expr.node_key)
            .copied()
    }

    fn resolved_call(&self, expr: &Expr) -> Option<ResolvedCall> {
        self.node_resolved_calls.get(&expr.node_key).cloned()
    }

    fn function_reference(&self, expr: &Expr) -> Option<&FunctionReference> {
        self.node_function_references.get(&expr.node_key)
    }

    fn builtin_value(&self, expr: &Expr) -> Option<&BuiltinValue> {
        self.node_builtin_values.get(&expr.node_key)
    }

    fn local_def(&self, key: &NodeKey) -> Option<LocalId> {
        self.locals.node_local_defs.get(key).copied()
    }

    fn local_use(&self, expr: &Expr) -> Option<nia_local_resolve::LocalUse> {
        self.locals.node_uses.get(&expr.node_key).copied()
    }

    fn value_name(&self, expr: &Expr) -> Option<nia_value_resolve::ValueNameResolution> {
        self.values.node_names.get(&expr.node_key).copied()
    }

    fn qualified_value(&self, expr: &Expr) -> Option<GlobalDefId> {
        let global_id = self
            .values
            .node_qualified_values
            .get(&expr.node_key)
            .copied()?;
        match self.semantic_uses.node_value_use(&expr.node_key) {
            Some(SemanticValueUse::Global(value_use)) if value_use == global_id => Some(global_id),
            _ => None,
        }
    }

    fn variant_enum(&self, expr: &Expr) -> Option<GlobalDefId> {
        self.values.node_variant_enums.get(&expr.node_key).copied()
    }

    fn qualified_type_prefix(&self, expr: &Expr) -> Option<GlobalDefId> {
        self.values
            .node_qualified_type_prefixes
            .get(&expr.node_key)
            .copied()
    }

    fn builtin_resolution(&self, expr: &Expr) -> Option<nia_value_resolve::BuiltinResolution> {
        self.values.node_builtins.get(&expr.node_key).copied()
    }

    fn with_comptime_context<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.comptime_context_depth += 1;
        let result = f(self);
        self.comptime_context_depth -= 1;
        result
    }

    fn in_comptime_context(&self) -> bool {
        self.comptime_context_depth > 0
    }

    fn defs_for_module(&self, module_id: ModuleId) -> Option<&DefCollection> {
        if module_id == self.defs.module_id {
            Some(self.defs)
        } else {
            self.program.defs?.get(&module_id)
        }
    }
}

#[derive(Debug, Clone)]
struct ResolvedStructSignature {
    signature: StructSignature,
}

#[derive(Debug, Clone)]
struct ResolvedUnionSignature {
    signature: UnionSignature,
}

#[derive(Debug, Clone)]
struct ResolvedEnumSignature {
    signature: EnumSignature,
}

impl<'a> BodyChecker<'a> {
    fn check_module(
        &mut self,
        active_item_tree: &ActiveModuleItemTree,
        timing: bool,
        module_id: ModuleId,
    ) {
        time_body_stage(timing, "body_check.bindings", module_id, || {
            for item in &active_item_tree.items {
                if let ItemTreeNodeKind::Binding(binding) = &item.kind {
                    if binding.is_comptime {
                        self.check_comptime_binding(item.span, binding);
                    } else {
                        self.check_global_binding(item.span, binding);
                    }
                }
            }
        });
        time_body_stage(timing, "body_check.functions", module_id, || {
            for item in &active_item_tree.items {
                if let ItemTreeNodeKind::Function(function) = &item.kind {
                    time_body_stage_if_slow(
                        timing,
                        "body_check.function",
                        module_id,
                        &function.name,
                        0.050,
                        || {
                            self.check_function_item(item.span, function);
                        },
                    );
                }
            }
        });
        time_body_stage(timing, "body_check.trait_defaults", module_id, || {
            for item in &active_item_tree.items {
                if let ItemTreeNodeKind::Trait(item_trait) = &item.kind {
                    for method in &item_trait.methods {
                        time_body_stage_if_slow(
                            timing,
                            "body_check.trait_method",
                            module_id,
                            &method.function.name,
                            0.050,
                            || {
                                self.check_trait_function_def(
                                    method.function.span,
                                    &method.function,
                                );
                            },
                        );
                    }
                }
            }
        });
        time_body_stage(timing, "body_check.extends", module_id, || {
            for item in &active_item_tree.items {
                if let ItemTreeNodeKind::Extend(extend) = &item.kind {
                    for associated_value in &extend.associated_values {
                        self.check_comptime_binding(
                            associated_value.span,
                            &associated_value.binding,
                        );
                    }
                    for method in &extend.methods {
                        time_body_stage_if_slow(
                            timing,
                            "body_check.extend_method",
                            module_id,
                            &method.function.name,
                            0.010,
                            || {
                                self.check_function_def(method.function.span, &method.function);
                            },
                        );
                    }
                }
            }
        });
    }

    fn seed_global_types(&mut self) {
        for (def_id, signature) in &self.signatures.globals {
            if let Some(ty) = signature.explicit_type {
                self.global_types.insert(*def_id, ty);
            }
        }
        for (def_id, signature) in &self.signatures.comptimes {
            if let Some(ty) = signature.explicit_type {
                self.comptime_types.insert(*def_id, ty);
            }
        }
    }

    fn check_comptime_binding(&mut self, item_span: Span, binding: &nia_ast::BindingItem) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, item_span, DefKind::Comptime)
        else {
            return;
        };
        let Some(value) = &binding.value else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                item_span,
                "comptime binding requires an initializer",
            ));
            return;
        };
        let comptime_ty = match binding.ty.as_ref() {
            Some(ty) => {
                let explicit = self.ty_for_type(ty);
                let value_ty = self
                    .comptime_initializer_runtime_type(value, Some(explicit))
                    .unwrap_or_else(|| {
                        self.with_comptime_context(|this| {
                            this.check_expr_with_expected(value, Some(explicit))
                        })
                    });
                if !self.is_comptime_only_ty(value_ty) && !self.types_match(explicit, value_ty) {
                    self.expect_expr_type(value, explicit, value_ty, "comptime initializer");
                }
                self.materialize_inferred_array_type(explicit, value_ty)
                    .unwrap_or(explicit)
            }
            None => {
                if let Some(ty) = self.comptime_initializer_runtime_type(value, None) {
                    ty
                } else if matches!(value.kind, ExprKind::ArrayLiteral { .. }) {
                    self.with_comptime_context(|this| this.infer_array_literal_expr(value))
                } else {
                    self.with_comptime_context(|this| this.check_expr(value))
                }
            }
        };
        self.comptime_types.insert(def_id, comptime_ty);
    }

    fn comptime_initializer_runtime_type(
        &mut self,
        value: &Expr,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        if !is_embed_builtin_call(value) {
            return None;
        }
        let comptime_expr = self.lower_comptime_expr(value).ok()?;
        let ty = self.comptime_expr_type_for_ir_with_expected(&comptime_expr, expected)?;
        match ty {
            nia_comptime_check::ComptimeValueType::Runtime(ty) => Some(ty),
            _ => None,
        }
    }

    fn check_global_binding(&mut self, item_span: Span, binding: &nia_ast::BindingItem) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, item_span, DefKind::Global)
        else {
            return;
        };
        let Some(value) = &binding.value else {
            let Some(signature) = self.signatures.globals.get(&def_id) else {
                return;
            };
            if let Some(ty) = signature.explicit_type {
                self.global_types.insert(def_id, ty);
            } else {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    item_span,
                    "global declaration requires an explicit type",
                ));
            }
            return;
        };
        let global_ty = match binding.ty.as_ref() {
            Some(ty) => {
                let explicit = self.ty_for_type(ty);
                let value_ty = self.check_expr_with_expected(value, Some(explicit));
                if self.is_comptime_only_ty(value_ty) {
                    self.reject_runtime_comptime_only_value(value.span, "global initializer");
                    self.error()
                } else {
                    self.expect_expr_type(value, explicit, value_ty, "global initializer");
                    self.materialize_inferred_array_type(explicit, value_ty)
                        .unwrap_or(explicit)
                }
            }
            None => {
                let value_ty = if matches!(value.kind, ExprKind::ArrayLiteral { .. }) {
                    self.infer_array_literal_expr(value)
                } else {
                    self.check_expr(value)
                };
                if self.is_comptime_only_ty(value_ty) {
                    self.reject_runtime_comptime_only_value(value.span, "global initializer");
                    self.error()
                } else {
                    value_ty
                }
            }
        };
        self.global_types.insert(def_id, global_ty);
        if global_ty != self.error() {
            let init = self.lower_global_static_init(value, global_ty);
            self.global_inits.insert(self.global_def_id(def_id), init);
        }
    }

    fn check_function_item(&mut self, _item_span: Span, function: &FunctionItem) {
        let Some(def_id) =
            self.def_id_for_node(&function.node_key, function.span, DefKind::Function)
        else {
            return;
        };
        if !self.body_filter.includes(self.global_def_id(def_id)) {
            return;
        }
        self.check_function(def_id, function);
    }

    fn check_function_def(&mut self, _span: Span, function: &FunctionItem) {
        let Some(def_id) = self.def_id_for_node(&function.node_key, function.span, DefKind::Method)
        else {
            return;
        };
        if !self.body_filter.includes(self.global_def_id(def_id)) {
            return;
        }
        self.check_function(def_id, function);
    }

    fn check_trait_function_def(&mut self, _span: Span, function: &FunctionItem) {
        let Some(def_id) =
            self.def_id_for_node(&function.node_key, function.span, DefKind::TraitMethod)
        else {
            return;
        };
        if !self.body_filter.includes(self.global_def_id(def_id)) {
            return;
        }
        self.check_function(def_id, function);
    }

    fn check_function(&mut self, def_id: DefId, function: &FunctionItem) {
        let global_def_id = self.global_def_id(def_id);
        if !self.program_functions.is_empty()
            && !self.program_functions.contains_key(&global_def_id)
        {
            return;
        }
        let signature = if let Some(program_signature) = self.program_functions.get(&global_def_id)
        {
            self.import_program_function_signature(&program_signature.clone())
        } else {
            let Some(raw_signature) = self.signatures.functions.get(&def_id).cloned() else {
                return;
            };
            self.import_local_function_signature(&raw_signature)
        };
        time_body_stage_if_slow(
            self.timing,
            "body_check.function.projection_obligations",
            self.timing_module_id,
            &function.name,
            0.020,
            || {
                self.check_function_signature_projection_obligations(def_id, &signature);
            },
        );
        let previous_return = self.current_return;
        let previous_def_id = self.current_def_id;
        let previous_param_locals = std::mem::take(&mut self.current_param_locals);
        self.current_return = signature.return_type;
        self.current_def_id = Some(global_def_id);
        let self_ty = self.method_self_type(def_id, &signature);
        time_body_stage_if_slow(
            self.timing,
            "body_check.function.object_safe",
            self.timing_module_id,
            &function.name,
            0.020,
            || {
                self.check_object_safe_types_in_signature(&signature);
            },
        );
        time_body_stage_if_slow(
            self.timing,
            "body_check.function.seed_params",
            self.timing_module_id,
            &function.name,
            0.020,
            || {
                self.seed_param_types(&signature, function, self_ty);
            },
        );
        if signature.is_comptime {
            self.current_return = previous_return;
            self.current_def_id = previous_def_id;
            self.current_param_locals = previous_param_locals;
            return;
        }
        if let Some(body) = &function.body {
            let expected_tail =
                (!self.is_void(signature.return_type)).then_some(signature.return_type);
            time_body_stage_if_slow(
                self.timing,
                "body_check.function.check_block",
                self.timing_module_id,
                &function.name,
                0.020,
                || {
                    let body_ty = self.check_block_with_expected(body, expected_tail);
                    if let Some(tail) = body.tail.as_deref() {
                        if !self.is_void(signature.return_type) {
                            self.expect_expr_type(
                                tail,
                                signature.return_type,
                                body_ty,
                                "function body",
                            );
                        }
                    } else if self.is_void(signature.return_type) {
                        self.expect_type(
                            body.span,
                            signature.return_type,
                            body_ty,
                            "function body",
                        );
                    }
                },
            );
            let body = time_body_stage_if_slow(
                self.timing,
                "body_check.function.lower_body",
                self.timing_module_id,
                &function.name,
                0.020,
                || self.lower_body(body),
            );
            self.function_bodies
                .insert(self.global_def_id(def_id), body);
        }
        self.current_return = previous_return;
        self.current_def_id = previous_def_id;
        self.current_param_locals = previous_param_locals;
    }

    fn check_object_safe_types_in_signature(&mut self, signature: &FunctionSignature) {
        for param in &signature.params {
            self.check_object_safe_type(param.span, param.ty);
        }
        self.check_object_safe_type(signature.span, signature.return_type);
    }

    fn seed_param_types(
        &mut self,
        signature: &FunctionSignature,
        function: &FunctionItem,
        self_ty: Option<InternedTyId>,
    ) {
        for (param, param_sig) in function.params.iter().zip(&signature.params) {
            if let Some(local_id) = self.local_def(&param.node_key) {
                let ty = if param_sig.receiver.is_some() {
                    self_ty.unwrap_or_else(|| self.error())
                } else {
                    param_sig.ty
                };
                self.record_local_type(local_id, ty);
                self.current_param_locals.push(local_id);
            }
        }
    }

    fn check_block(&mut self, block: &Block) -> InternedTyId {
        self.check_block_with_expected(block, None)
    }

    fn check_block_with_expected(
        &mut self,
        block: &Block,
        expected_tail: Option<InternedTyId>,
    ) -> InternedTyId {
        if block.stmts.is_empty()
            && block.tail.is_none()
            && let Some(expected) = expected_tail
            && let Some(TyKind::Nominal { def_id, args }) = self.interner.get(expected)
        {
            let def_id = *def_id;
            let args = args.clone();
            if self.is_union_def(def_id) {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    block.span,
                    "union literal requires exactly one field, got 0",
                ));
                return expected;
            }
            if self.is_empty_struct_type(def_id, &args) {
                return expected;
            }
        }
        for stmt in &block.stmts {
            self.check_stmt(stmt);
        }
        if let Some(tail) = &block.tail {
            self.check_expr_with_expected(tail, expected_tail)
        } else if self.block_ends_with_never_stmt(block) {
            self.never()
        } else {
            self.void()
        }
    }

    fn block_ends_with_never_stmt(&mut self, block: &Block) -> bool {
        let Some(stmt) = block.stmts.last() else {
            return false;
        };
        match &stmt.kind {
            StmtKind::Return(_) | StmtKind::Break | StmtKind::Continue => true,
            StmtKind::Expr(expr) => self.expr_ty(expr).is_some_and(|ty| self.is_never(ty)),
            StmtKind::Binding(_)
            | StmtKind::Using(_)
            | StmtKind::Defer(_)
            | StmtKind::ForIn(_)
            | StmtKind::While(_)
            | StmtKind::Loop(_) => false,
        }
    }

    fn is_empty_struct_type(&mut self, def_id: GlobalDefId, args: &[InternedTyId]) -> bool {
        let Some(resolved) = self.resolved_struct_signature(def_id) else {
            return false;
        };
        resolved.signature.generics.len() == args.len() && resolved.signature.fields.is_empty()
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Binding(binding) => {
                self.check_local_binding(stmt, binding);
            }
            StmtKind::Using(_) => {
                // Block-scope `using` is a no-op for body type-checking.
            }
            StmtKind::Expr(expr) => {
                let expr_ty = self.check_expr(expr);
                if !self.is_void(expr_ty) && !self.is_never(expr_ty) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        expr.span,
                        "non-void expression result is discarded; assign it to `_` explicitly",
                    ));
                }
            }
            StmtKind::Defer(expr) => {
                let expr_ty = self.check_expr(expr);
                if !self.is_void(expr_ty) && !self.is_never(expr_ty) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        expr.span,
                        "`defer` expression must have type `void`",
                    ));
                }
            }
            StmtKind::Return(value) => {
                let value_ty = match value {
                    Some(value) => self.check_expr_with_expected(value, Some(self.current_return)),
                    None => self.void(),
                };
                if let Some(value) = value {
                    self.expect_expr_type(value, self.current_return, value_ty, "return");
                    self.record_expr_node_type(value, self.current_return);
                } else {
                    self.expect_type(stmt.span, self.current_return, value_ty, "return");
                }
            }
            StmtKind::Break | StmtKind::Continue => {}
            StmtKind::ForIn(for_stmt) => {
                let iter_ty = self.check_expr(&for_stmt.iter);
                let item_ty = self.for_iterator_item_type(&for_stmt.iter, iter_ty);
                let binding_ty = self.check_binding_pattern(
                    for_stmt.pattern.kind,
                    for_stmt.pattern.span,
                    item_ty,
                );
                if for_stmt.pattern.name().is_some()
                    && let Some(local_id) = self.local_def(&for_stmt.pattern.node_key)
                {
                    self.record_local_type(local_id, binding_ty);
                }
                self.check_block(&for_stmt.body);
            }
            StmtKind::While(while_stmt) => {
                let cond_ty = self.check_expr(&while_stmt.cond);
                self.expect_type(
                    while_stmt.cond.span,
                    self.bool(),
                    cond_ty,
                    "while condition",
                );
                self.check_block(&while_stmt.body);
            }
            StmtKind::Loop(loop_stmt) => {
                self.check_block(&loop_stmt.body);
            }
        }
    }

    fn for_iterator_item_type(&mut self, iter: &Expr, iter_ty: InternedTyId) -> InternedTyId {
        if !self.current_context_proves_trait_obligation(
            iter_ty,
            nia_ty::TraitId::Builtin(nia_ty::BuiltinTrait::Iterator),
            Vec::new(),
        ) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                iter.span,
                format!(
                    "for-in expects an Iterator, found `{}`",
                    self.ty_name(iter_ty)
                ),
            ));
            return self.error();
        }
        self.iterator_item_projection(iter_ty)
    }

    fn lower_for_iterator_item_type(&mut self, iter_ty: InternedTyId) -> InternedTyId {
        if !self.current_context_proves_trait_obligation(
            iter_ty,
            nia_ty::TraitId::Builtin(nia_ty::BuiltinTrait::Iterator),
            Vec::new(),
        ) {
            return self.error();
        }
        self.iterator_item_projection(iter_ty)
    }

    fn iterator_item_projection(&mut self, iter_ty: InternedTyId) -> InternedTyId {
        let item = self.interner.intern(TyKind::Projection {
            self_ty: iter_ty,
            trait_id: nia_ty::TraitId::Builtin(nia_ty::BuiltinTrait::Iterator),
            trait_args: Vec::new(),
            name: nia_ty::BuiltinTrait::ITEM_ASSOC_TYPE.to_string(),
        });
        self.normalize_projection(item)
    }

    fn check_binding_pattern(
        &mut self,
        kind: nia_ast::BindingPatternKind,
        span: Span,
        value_ty: InternedTyId,
    ) -> InternedTyId {
        match kind {
            nia_ast::BindingPatternKind::Value => value_ty,
            nia_ast::BindingPatternKind::Pointer | nia_ast::BindingPatternKind::MutPointer => {
                let expected_readonly = matches!(kind, nia_ast::BindingPatternKind::Pointer);
                match self
                    .interner
                    .get(self.normalization.normalize(value_ty))
                    .cloned()
                {
                    Some(TyKind::Pointer { is_readonly, elem })
                        if is_readonly == expected_readonly =>
                    {
                        elem
                    }
                    Some(TyKind::Pointer { .. }) => {
                        let expected = if expected_readonly {
                            "`&x`"
                        } else {
                            "`&mut x`"
                        };
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            span,
                            format!("binding pattern {expected} does not match value type"),
                        ));
                        self.error()
                    }
                    _ => {
                        let expected = if expected_readonly {
                            "read-only pointer"
                        } else {
                            "mutable pointer"
                        };
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            span,
                            format!("binding pattern requires value to be a {expected}"),
                        ));
                        self.error()
                    }
                }
            }
        }
    }

    fn binding_pattern_input_ty(
        &mut self,
        kind: nia_ast::BindingPatternKind,
        binding_ty: InternedTyId,
    ) -> InternedTyId {
        match kind {
            nia_ast::BindingPatternKind::Value => binding_ty,
            nia_ast::BindingPatternKind::Pointer => self.interner.intern(TyKind::Pointer {
                is_readonly: true,
                elem: binding_ty,
            }),
            nia_ast::BindingPatternKind::MutPointer => self.interner.intern(TyKind::Pointer {
                is_readonly: false,
                elem: binding_ty,
            }),
        }
    }

    fn materialize_explicit_binding_pattern_ty(
        &mut self,
        kind: nia_ast::BindingPatternKind,
        explicit_binding: InternedTyId,
        value_ty: InternedTyId,
    ) -> InternedTyId {
        match kind {
            nia_ast::BindingPatternKind::Value => self
                .materialize_inferred_array_type(explicit_binding, value_ty)
                .unwrap_or(explicit_binding),
            nia_ast::BindingPatternKind::Pointer | nia_ast::BindingPatternKind::MutPointer => {
                let value_elem = match self.interner.get(self.normalization.normalize(value_ty)) {
                    Some(TyKind::Pointer { elem, .. }) => Some(*elem),
                    _ => None,
                };
                value_elem
                    .and_then(|elem| self.materialize_inferred_array_type(explicit_binding, elem))
                    .unwrap_or(explicit_binding)
            }
        }
    }

    fn local_binding_pattern_key<'b>(
        &self,
        stmt: &'b Stmt,
        binding: &'b BindingStmt,
    ) -> &'b NodeKey {
        if matches!(binding.pattern_kind, nia_ast::BindingPatternKind::Value) {
            &stmt.node_key
        } else {
            &binding.pattern_node_key
        }
    }

    fn check_local_binding(&mut self, stmt: &Stmt, binding: &BindingStmt) {
        let span = stmt.span;
        if binding.is_comptime && binding.value.is_none() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "comptime binding requires an initializer",
            ));
        }
        if !matches!(binding.pattern_kind, nia_ast::BindingPatternKind::Value)
            && binding.value.is_none()
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                binding.pattern_span,
                "binding pattern requires an initializer",
            ));
            return self.record_error_local_binding(self.local_binding_pattern_key(stmt, binding));
        }
        let binding_ty = match (&binding.ty, &binding.value) {
            (Some(ty), Some(value)) => {
                let explicit_binding = self.ty_for_type(ty);
                let explicit_input =
                    self.binding_pattern_input_ty(binding.pattern_kind, explicit_binding);
                let value_ty = if binding.is_comptime {
                    self.with_comptime_context(|this| {
                        this.check_expr_with_expected(value, Some(explicit_input))
                    })
                } else {
                    self.check_expr_with_expected(value, Some(explicit_input))
                };
                if binding.is_comptime && self.is_comptime_only_ty(value_ty) {
                    // The initializer is validated by nia-comptime-check and has no runtime value.
                } else if self.is_comptime_only_ty(value_ty) {
                    self.reject_runtime_comptime_only_value(value.span, "binding initializer");
                    return self
                        .record_error_local_binding(self.local_binding_pattern_key(stmt, binding));
                } else {
                    self.expect_expr_type(value, explicit_input, value_ty, "binding initializer");
                }
                self.materialize_explicit_binding_pattern_ty(
                    binding.pattern_kind,
                    explicit_binding,
                    value_ty,
                )
            }
            (Some(ty), None) => {
                let explicit = self.ty_for_type(ty);
                if matches!(binding.pattern_kind, nia_ast::BindingPatternKind::Value) {
                    explicit
                } else {
                    self.error()
                }
            }
            (None, Some(value)) => {
                let value_ty = if matches!(value.kind, ExprKind::ArrayLiteral { .. }) {
                    if binding.is_comptime {
                        self.with_comptime_context(|this| this.infer_array_literal_expr(value))
                    } else {
                        self.infer_array_literal_expr(value)
                    }
                } else {
                    if binding.is_comptime {
                        self.with_comptime_context(|this| this.check_expr(value))
                    } else {
                        self.check_expr(value)
                    }
                };
                if !binding.is_comptime && self.is_comptime_only_ty(value_ty) {
                    self.reject_runtime_comptime_only_value(value.span, "binding initializer");
                    self.error()
                } else {
                    self.check_binding_pattern(binding.pattern_kind, binding.pattern_span, value_ty)
                }
            }
            (None, None) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    "binding declaration requires an explicit type",
                ));
                self.error()
            }
        };
        if let Some(local_id) = self.local_def(self.local_binding_pattern_key(stmt, binding)) {
            self.record_local_type(local_id, binding_ty);
        }
    }

    fn reject_runtime_comptime_only_value(&mut self, span: Span, context: &str) {
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            span,
            format!("{context} cannot use comptime-only value"),
        ));
    }

    fn record_error_local_binding(&mut self, key: &NodeKey) {
        if let Some(local_id) = self.local_def(key) {
            self.record_local_type(local_id, self.error());
        }
    }

    pub(crate) fn check_switch_expr(
        &mut self,
        switch: &nia_ast::SwitchStmt,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let target_ty = self.check_expr(&switch.target);
        self.check_switch_target_is_value_dispatch(switch.target.span, target_ty);
        let mut coverage = SwitchCoverage::default();
        let mut result_ty = expected;

        for arm in &switch.arms {
            if coverage.catch_all.is_some() {
                self.diagnostics.push(Diagnostic::user_error_at(codes::TYPE_CHECK,
                    arm.span,
                    "switch arm is unreachable because a previous pattern matches all remaining values",
                ));
            }
            for pattern in &arm.patterns {
                if matches!(&pattern.kind, nia_ast::SwitchPatternKind::Wildcard)
                    && arm.patterns.len() != 1
                {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        arm.span,
                        "`_` default must be the only pattern in a switch arm",
                    ));
                }
                self.check_switch_pattern(pattern, target_ty, &mut coverage);
            }
            let arm_ty = self.check_switch_arm_body(&arm.body, result_ty);
            if let Some(expected) = result_ty {
                self.expect_switch_arm_type(&arm.body, expected, arm_ty);
            } else if !self.is_never(arm_ty) {
                result_ty = Some(arm_ty);
            }
        }

        self.check_pattern_switch_exhaustive(switch.target.span, target_ty, &coverage);
        result_ty.unwrap_or_else(|| self.void())
    }

    fn check_switch_target_is_value_dispatch(&mut self, span: Span, target_ty: InternedTyId) {
        match self.interner.get(self.normalization.normalize(target_ty)) {
            Some(TyKind::Optional { .. }) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    "switch does not destructure optional values; use `if let` or `if var`",
                ));
            }
            Some(TyKind::ErrorUnion { .. }) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    "switch does not destructure error-union values; use `if let` or `if var`",
                ));
            }
            _ => {}
        }
    }

    pub(crate) fn check_if_pattern_expr(
        &mut self,
        if_pattern: &nia_ast::IfPatternExpr,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        let target_ty = self.check_expr(&if_pattern.target);
        let mut coverage = PatternCoverage::default();
        let mut result_ty = expected;
        for arm in &if_pattern.arms {
            if coverage.catch_all.is_some() {
                self.diagnostics.push(Diagnostic::user_error_at(codes::TYPE_CHECK,
                    arm.span,
                    "if pattern arm is unreachable because a previous pattern matches all remaining values",
                ));
            }
            self.check_pattern(&arm.pattern, target_ty, Some(&mut coverage), "if pattern");
            let arm_ty = self.check_block_with_expected(&arm.body, result_ty);
            if let Some(expected) = result_ty {
                self.expect_block_tail_type(&arm.body, expected, arm_ty, "if pattern branches");
            } else if !self.is_never(arm_ty) {
                result_ty = Some(arm_ty);
            }
        }

        let Some(else_branch) = &if_pattern.else_branch else {
            if self.pattern_coverage_covers_type(target_ty, &coverage) {
                return result_ty.unwrap_or_else(|| self.void());
            }
            if expected.is_some_and(|expected| !self.is_void(expected)) {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    if_pattern.target.span,
                    "non-exhaustive if pattern requires an `else` branch",
                ));
            }
            return self.void();
        };
        let else_ty = self.check_expr_with_expected(else_branch, result_ty);
        if let Some(expected) = result_ty {
            self.expect_expr_or_block_tail_type(
                else_branch,
                expected,
                else_ty,
                "if pattern branches",
            );
            expected
        } else {
            else_ty
        }
    }

    fn check_switch_pattern(
        &mut self,
        pattern: &nia_ast::SwitchPattern,
        target_ty: InternedTyId,
        coverage: &mut SwitchCoverage,
    ) {
        match &pattern.kind {
            nia_ast::SwitchPatternKind::Wildcard => {
                coverage.catch_all = Some(pattern.span);
            }
            nia_ast::SwitchPatternKind::Expr(expr) => {
                if let Some(previous) = coverage.catch_all {
                    self.report_pattern_overlap(pattern.span, previous);
                }
                self.check_switch_expr_pattern(
                    expr,
                    target_ty,
                    self.enum_global_def_id(target_ty),
                    "switch pattern",
                    &mut coverage.enum_variants,
                    &mut coverage.intervals,
                );
            }
            nia_ast::SwitchPatternKind::Range {
                start,
                end,
                inclusive,
            } => {
                if let Some(previous) = coverage.catch_all {
                    self.report_pattern_overlap(pattern.span, previous);
                }
                self.check_switch_range_pattern(
                    RangePatternCheck {
                        span: pattern.span,
                        start,
                        end,
                        inclusive: *inclusive,
                    },
                    target_ty,
                    "switch pattern",
                    &mut coverage.intervals,
                );
            }
        }
    }

    fn check_pattern(
        &mut self,
        pattern: &nia_ast::Pattern,
        target_ty: InternedTyId,
        coverage: Option<&mut PatternCoverage>,
        context: &str,
    ) {
        match &pattern.kind {
            nia_ast::PatternKind::Wildcard => {
                if let Some(coverage) = coverage {
                    coverage.catch_all = Some(pattern.span);
                }
            }
            nia_ast::PatternKind::Bind { node_key, .. } => {
                if let Some(local_id) = self.local_def(node_key) {
                    self.record_local_type(local_id, target_ty);
                }
                if let Some(coverage) = coverage {
                    coverage.catch_all = Some(pattern.span);
                }
            }
            nia_ast::PatternKind::OptionalSome(inner) => {
                let elem_ty = match self.interner.get(self.normalization.normalize(target_ty)) {
                    Some(TyKind::Optional { elem }) => *elem,
                    _ => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            pattern.span,
                            format!(
                                "`?` pattern requires an optional target, found `{}`",
                                self.ty_name(target_ty)
                            ),
                        ));
                        self.error()
                    }
                };
                let child_coverage = coverage.map(|coverage| {
                    if let Some(previous) = coverage.catch_all {
                        self.report_pattern_overlap(pattern.span, previous);
                    }
                    coverage
                        .optional_some
                        .get_or_insert_with(|| Box::new(PatternCoverage::default()))
                        .as_mut()
                });
                self.check_pattern(inner, elem_ty, child_coverage, context);
            }
            nia_ast::PatternKind::OptionalNull => {
                if !matches!(
                    self.interner.get(self.normalization.normalize(target_ty)),
                    Some(TyKind::Optional { .. })
                ) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        pattern.span,
                        format!(
                            "`null` pattern requires an optional target, found `{}`",
                            self.ty_name(target_ty)
                        ),
                    ));
                }
                if let Some(coverage) = coverage {
                    if let Some(previous) = coverage.catch_all.or(coverage.optional_null) {
                        self.report_pattern_overlap(pattern.span, previous);
                    }
                    coverage.optional_null = Some(pattern.span);
                }
            }
            nia_ast::PatternKind::ErrorOk(inner) => {
                let value_ty = match self.interner.get(self.normalization.normalize(target_ty)) {
                    Some(TyKind::ErrorUnion { value, .. }) => *value,
                    _ => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            pattern.span,
                            format!(
                                "`!` pattern requires an error union target, found `{}`",
                                self.ty_name(target_ty)
                            ),
                        ));
                        self.error()
                    }
                };
                let child_coverage = coverage.map(|coverage| {
                    if let Some(previous) = coverage.catch_all {
                        self.report_pattern_overlap(pattern.span, previous);
                    }
                    coverage
                        .error_ok
                        .get_or_insert_with(|| Box::new(PatternCoverage::default()))
                        .as_mut()
                });
                self.check_pattern(inner, value_ty, child_coverage, context);
            }
            nia_ast::PatternKind::ErrorErr(inner) => {
                let error_ty = match self.interner.get(self.normalization.normalize(target_ty)) {
                    Some(TyKind::ErrorUnion { error, .. }) => *error,
                    _ => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            pattern.span,
                            format!(
                                "`pattern!` requires an error union target, found `{}`",
                                self.ty_name(target_ty)
                            ),
                        ));
                        self.error()
                    }
                };
                let child_coverage = coverage.map(|coverage| {
                    if let Some(previous) = coverage.catch_all {
                        self.report_pattern_overlap(pattern.span, previous);
                    }
                    coverage
                        .error_err
                        .get_or_insert_with(|| Box::new(PatternCoverage::default()))
                        .as_mut()
                });
                self.check_pattern(inner, error_ty, child_coverage, context);
            }
            nia_ast::PatternKind::Expr(expr) => {
                let pattern_ty = self.check_expr_with_expected(expr, Some(target_ty));
                self.expect_expr_type(expr, target_ty, pattern_ty, context);
            }
            nia_ast::PatternKind::Range {
                start,
                end,
                inclusive,
            } => {
                self.check_if_pattern_range(
                    RangePatternCheck {
                        span: pattern.span,
                        start,
                        end,
                        inclusive: *inclusive,
                    },
                    target_ty,
                    context,
                );
            }
        }
    }

    fn check_if_pattern_range(
        &mut self,
        pattern: RangePatternCheck<'_>,
        target_ty: InternedTyId,
        context: &str,
    ) {
        if !self.is_integer(target_ty) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                pattern.span,
                format!("{context} range requires an integer target"),
            ));
        }
        let start_ty = self.check_expr_with_expected(pattern.start, Some(target_ty));
        self.expect_expr_type(pattern.start, target_ty, start_ty, context);
        let end_ty = self.check_expr_with_expected(pattern.end, Some(target_ty));
        self.expect_expr_type(pattern.end, target_ty, end_ty, context);
    }

    fn report_pattern_overlap(&mut self, span: Span, previous: Span) {
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            span,
            format!("pattern overlaps previous pattern at {previous:?}"),
        ));
    }

    fn check_pattern_switch_exhaustive(
        &mut self,
        span: Span,
        target_ty: InternedTyId,
        coverage: &SwitchCoverage,
    ) {
        if self.switch_coverage_covers_type(target_ty, coverage) {
            return;
        }
        match self.interner.get(self.normalization.normalize(target_ty)) {
            Some(TyKind::Optional { .. }) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    "switch over optional values is not supported; use `if let` or `if var`",
                ));
            }
            Some(TyKind::ErrorUnion { .. }) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    "switch over error-union values is not supported; use `if let` or `if var`",
                ));
            }
            _ => {
                if let Some(enum_id) = self.enum_global_def_id(target_ty) {
                    let covered = coverage.enum_variants.keys().copied().collect();
                    self.check_enum_switch_exhaustive(
                        span,
                        enum_id,
                        coverage.catch_all.is_some(),
                        &covered,
                    );
                }
            }
        }
    }

    fn switch_coverage_covers_type(
        &mut self,
        target_ty: InternedTyId,
        coverage: &SwitchCoverage,
    ) -> bool {
        if coverage.catch_all.is_some() {
            return true;
        }
        if self.is_bool(target_ty) {
            return self.pattern_intervals_cover_bool(&coverage.intervals);
        }
        self.enum_global_def_id(target_ty)
            .is_some_and(|enum_id| self.switch_coverage_covers_enum(enum_id, coverage))
    }

    fn switch_coverage_covers_enum(
        &mut self,
        enum_id: GlobalDefId,
        coverage: &SwitchCoverage,
    ) -> bool {
        let Some(resolved) = self.resolved_enum_signature(enum_id) else {
            return false;
        };
        !resolved.signature.is_open
            && resolved
                .signature
                .variants
                .iter()
                .all(|variant| coverage.enum_variants.contains_key(&variant.def_id))
    }

    fn pattern_coverage_covers_type(
        &mut self,
        target_ty: InternedTyId,
        coverage: &PatternCoverage,
    ) -> bool {
        if coverage.catch_all.is_some() {
            return true;
        }
        let normalized = self.normalization.normalize(target_ty);
        match self.interner.get(normalized).cloned() {
            Some(TyKind::Optional { elem }) => {
                coverage.optional_null.is_some()
                    && if let Some(coverage) = coverage.optional_some.as_deref() {
                        self.pattern_coverage_covers_type(elem, coverage)
                    } else {
                        false
                    }
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let ok_covered = if let Some(coverage) = coverage.error_ok.as_deref() {
                    self.pattern_coverage_covers_type(value, coverage)
                } else {
                    false
                };
                let err_covered = if let Some(coverage) = coverage.error_err.as_deref() {
                    self.pattern_coverage_covers_type(error, coverage)
                } else {
                    false
                };
                ok_covered && err_covered
            }
            _ => false,
        }
    }

    fn pattern_intervals_cover_bool(&self, intervals: &[SwitchInterval]) -> bool {
        let covers = |tag: i128| {
            intervals
                .iter()
                .any(|interval| interval.start <= tag && tag <= interval.end)
        };
        covers(0) && covers(1)
    }

    fn check_switch_expr_pattern(
        &mut self,
        pattern: &Expr,
        target_ty: InternedTyId,
        enum_id: Option<GlobalDefId>,
        context: &str,
        covered_enum_variants: &mut HashMap<DefId, Span>,
        covered_intervals: &mut Vec<SwitchInterval>,
    ) {
        let pattern_ty = self.check_expr_with_expected(pattern, Some(target_ty));
        if self.is_open_enum(target_ty)
            && self.check_integer_literal_enum_backing_range(pattern, target_ty, context)
        {
            self.record_expr_node_type(pattern, target_ty);
        } else {
            self.expect_expr_type(pattern, target_ty, pattern_ty, context);
        }
        if let Some(expected_enum) = enum_id
            && let Some((variant_enum, variant_id)) = self.enum_variant_info(pattern)
            && variant_enum == expected_enum
        {
            if let Some(previous) = covered_enum_variants.insert(variant_id, pattern.span) {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    pattern.span,
                    format!("{context} overlaps previous pattern at {previous:?}"),
                ));
            }
            return;
        }
        if self.is_integer(target_ty) || self.is_bool(target_ty) {
            let Some(value) = self.switch_pattern_int_value(pattern) else {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    pattern.span,
                    format!("{context} must be a compile-time integer constant"),
                ));
                return;
            };
            self.check_switch_interval_overlap(
                SwitchInterval {
                    start: value,
                    end: value,
                    span: pattern.span,
                },
                covered_intervals,
            );
        }
    }

    fn check_switch_range_pattern(
        &mut self,
        pattern: RangePatternCheck<'_>,
        target_ty: InternedTyId,
        context: &str,
        covered_intervals: &mut Vec<SwitchInterval>,
    ) {
        if !self.is_integer(target_ty) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                pattern.span,
                format!("{context} range requires an integer target"),
            ));
        }
        let start_ty = self.check_expr_with_expected(pattern.start, Some(target_ty));
        self.expect_expr_type(pattern.start, target_ty, start_ty, context);
        let end_ty = self.check_expr_with_expected(pattern.end, Some(target_ty));
        self.expect_expr_type(pattern.end, target_ty, end_ty, context);
        let Some(start_value) = self.switch_pattern_int_value(pattern.start) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                pattern.start.span,
                format!("{context} range start must be a compile-time integer constant"),
            ));
            return;
        };
        let Some(end_value) = self.switch_pattern_int_value(pattern.end) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                pattern.end.span,
                format!("{context} range end must be a compile-time integer constant"),
            ));
            return;
        };
        let Some(end_inclusive) = (if pattern.inclusive {
            Some(end_value)
        } else {
            end_value.checked_sub(1)
        }) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                pattern.span,
                format!("{context} range endpoint is out of range"),
            ));
            return;
        };
        if start_value > end_inclusive {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                pattern.span,
                format!("{context} range is empty"),
            ));
            return;
        }
        self.check_switch_interval_overlap(
            SwitchInterval {
                start: start_value,
                end: end_inclusive,
                span: pattern.span,
            },
            covered_intervals,
        );
    }

    fn switch_pattern_int_value(&mut self, expr: &Expr) -> Option<i128> {
        let value = if let ExprKind::Bool(value) = expr.kind {
            if value { 1 } else { 0 }
        } else {
            match self
                .with_comptime_context(|this| {
                    let expr = this.lower_comptime_expr(expr).map_err(|err| {
                        nia_comptime_engine::ComptimeError {
                            span: err.span,
                            message: err.message,
                        }
                    })?;
                    nia_comptime_engine::eval_resolved_comptime_expr(&expr, this)
                })
                .ok()?
            {
                nia_comptime_engine::ComptimeValue::Int(value) => value.as_i128()?,
                _ => return None,
            }
        };
        self.record_switch_pattern_value(expr, value);
        Some(value)
    }

    fn check_switch_interval_overlap(
        &mut self,
        interval: SwitchInterval,
        covered_intervals: &mut Vec<SwitchInterval>,
    ) {
        if let Some(previous) = covered_intervals
            .iter()
            .find(|previous| interval.start <= previous.end && previous.start <= interval.end)
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                interval.span,
                format!(
                    "switch pattern overlaps previous pattern at {:?}",
                    previous.span
                ),
            ));
        }
        covered_intervals.push(interval);
    }

    fn is_bool(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(self.normalization.normalize(ty)),
            Some(TyKind::Primitive(PrimitiveTy::Bool))
        )
    }

    fn check_switch_arm_body(
        &mut self,
        body: &nia_ast::SwitchArmBody,
        expected: Option<InternedTyId>,
    ) -> InternedTyId {
        match body {
            nia_ast::SwitchArmBody::Expr(expr) => self.check_expr_with_expected(expr, expected),
            nia_ast::SwitchArmBody::Stmt(stmt) => {
                self.check_stmt(stmt);
                if matches!(
                    stmt.kind,
                    StmtKind::Return(_) | StmtKind::Break | StmtKind::Continue
                ) {
                    self.never()
                } else {
                    self.void()
                }
            }
            nia_ast::SwitchArmBody::Block(block) => self.check_block_with_expected(block, expected),
        }
    }

    fn expect_switch_arm_type(
        &mut self,
        body: &nia_ast::SwitchArmBody,
        expected: InternedTyId,
        actual: InternedTyId,
    ) {
        if self.is_never(actual) {
            return;
        }
        match body {
            nia_ast::SwitchArmBody::Expr(expr) => {
                self.expect_expr_type(expr, expected, actual, "switch arms");
            }
            nia_ast::SwitchArmBody::Block(block) => {
                self.expect_block_tail_type(block, expected, actual, "switch arms");
            }
            nia_ast::SwitchArmBody::Stmt(stmt) => {
                self.expect_type(stmt.span, expected, actual, "switch arms");
            }
        }
    }
}

fn is_embed_builtin_call(expr: &Expr) -> bool {
    let ExprKind::Call { callee, .. } = &expr.kind else {
        return false;
    };
    matches!(
        &callee.kind,
        ExprKind::Builtin { name, .. } if name == "embed"
    )
}

pub(crate) fn generic_inst_base(expr: &Expr) -> &Expr {
    match &expr.kind {
        ExprKind::BracketSuffix { callee, .. } => callee,
        _ => expr,
    }
}

#[cfg(test)]
mod tests;
