// SPDX-License-Identifier: GPL-3.0-or-later
use std::cell::RefCell;
use std::fmt;
use std::{
    collections::{HashMap, HashSet, VecDeque},
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

use nia_ast::{
    Attribute, AttributeKind, BindingStmt, Block, Expr, ExprKind, FunctionItem, Module, Stmt,
    StmtKind,
};
use nia_body_ir::BodyIr;
use nia_comptime_check::{
    ComptimeArrayLengths, ComptimeKey, ComptimeTypedFacts, ComptimeValue, ComptimeValues,
    TypedComptimeValue,
};
use nia_comptime_ir::ResolvedComptimeModule;
use nia_defs::{
    DefCollection, DefId, DefKind, ExtensionMethods, VisibleExtensionMethod,
    VisibleExtensionMethods,
};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalDefId, InternedTyId, LocalId, ModuleId, ReceiverKind, Visibility};
use nia_item_signatures::{
    ComptimeSignature, EnumSignature, FunctionSignature, GlobalSignature, ItemSignatures,
    ProgramComptimeSignature, ProgramEnumSignature, ProgramFunctionSignature,
    ProgramGlobalSignature, ProgramStructSignature, ProgramTraitImplSignature,
    ProgramTraitSignature, ProgramTypeAliasSignature, ProgramUnionSignature, StructSignature,
    TraitImplSignature, TraitSignature, TypeAliasSignature, UnionSignature,
};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNode, ItemTreeNodeKind, ModuleItemTree};
use nia_layout::Layouts;
use nia_local_resolve::LocalResolution;
use nia_node_id::{NodeOriginTable, VersionedNodeKey};
use nia_program_signatures::{ProgramSignatureContext, ProgramSignatureLookup};
use nia_sema_ir::{
    AssociatedComptimeProjection, BracketSuffixResolution, BuiltinValue, FunctionReference,
    FunctionSemanticFacts, GenericInstantiation, PointerArrayToSliceCoercion, ResolvedCall,
    SemanticFacts, SemanticUseTable, SemanticValueUse, TraitObjectCoercion, TraitObjectUpcast,
};
use nia_source::{SourcePath, SourceVersion};
use nia_span::Span;
use nia_target_config::TargetConfig;
use nia_ty::{ConstGenericArg, PrimitiveTy, TyInterner, TyKind};
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_value_resolve::ValueResolution;

use crate::projection_obligations::TraitObligation;

#[derive(Debug, Clone, PartialEq)]
pub struct BodyCheck {
    pub ir: BodyIr,
    pub facts: SemanticFacts,
    pub checked_functions: HashSet<GlobalDefId>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy)]
pub struct BodyComptime<'a> {
    pub interner: &'a TyInterner,
    pub values: &'a HashMap<ComptimeKey, ComptimeValue>,
    pub typed_values: &'a HashMap<ComptimeKey, TypedComptimeValue>,
    pub array_lengths: &'a HashMap<nia_ids::GlobalConstExprId, u64>,
}

impl<'a> BodyComptime<'a> {
    pub fn from_phases(
        values: &'a ComptimeValues,
        array_lengths: &'a ComptimeArrayLengths,
        typed_facts: &'a ComptimeTypedFacts,
    ) -> Self {
        Self {
            interner: &typed_facts.interner,
            values: &values.values,
            typed_values: &typed_facts.typed_values,
            array_lengths: &array_lengths.values,
        }
    }
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

#[derive(Clone, Copy)]
pub struct ProgramComptimeMaps<'a> {
    pub values: &'a dyn Fn(ModuleId) -> Option<ComptimeValues>,
    pub array_lengths: &'a dyn Fn(ModuleId) -> Option<ComptimeArrayLengths>,
    pub module: &'a dyn Fn(ModuleId) -> Option<ResolvedComptimeModule>,
}

impl fmt::Debug for ProgramComptimeMaps<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProgramComptimeMaps")
            .field("values", &true)
            .field("array_lengths", &true)
            .field("module", &true)
            .finish()
    }
}

impl ProgramComptimeMaps<'_> {
    pub fn empty() -> Self {
        Self {
            values: &no_program_comptime_values,
            array_lengths: &no_program_comptime_array_lengths,
            module: &no_program_comptime_module,
        }
    }
}

fn no_program_comptime_values(_: ModuleId) -> Option<ComptimeValues> {
    None
}

fn no_program_comptime_array_lengths(_: ModuleId) -> Option<ComptimeArrayLengths> {
    None
}

fn no_program_comptime_module(_: ModuleId) -> Option<ResolvedComptimeModule> {
    None
}

#[derive(Debug, Clone, Copy, Default)]
pub enum BodyCheckFilter<'a> {
    #[default]
    All,
    ReachableFunctions(&'a HashSet<GlobalDefId>),
    ReachableItems {
        functions: &'a HashSet<GlobalDefId>,
        globals: &'a HashSet<GlobalDefId>,
        already_checked_functions: Option<&'a HashSet<GlobalDefId>>,
        already_checked_globals: Option<&'a HashSet<GlobalDefId>>,
    },
}

#[derive(Debug, Clone)]
enum ActiveBodyCheckFilter<'a> {
    All,
    ReachableItems {
        functions: &'a HashSet<GlobalDefId>,
        globals: &'a HashSet<GlobalDefId>,
        already_checked_functions: Option<&'a HashSet<GlobalDefId>>,
        already_checked_globals: Option<&'a HashSet<GlobalDefId>>,
        discovered_functions: HashSet<GlobalDefId>,
    },
}

impl<'a> ActiveBodyCheckFilter<'a> {
    fn from_filter(filter: BodyCheckFilter<'a>) -> Self {
        match filter {
            BodyCheckFilter::All => Self::All,
            BodyCheckFilter::ReachableFunctions(functions) => Self::ReachableItems {
                functions,
                globals: empty_global_def_ids(),
                already_checked_functions: None,
                already_checked_globals: None,
                discovered_functions: HashSet::new(),
            },
            BodyCheckFilter::ReachableItems {
                functions,
                globals,
                already_checked_functions,
                already_checked_globals,
            } => Self::ReachableItems {
                functions,
                globals,
                already_checked_functions,
                already_checked_globals,
                discovered_functions: HashSet::new(),
            },
        }
    }

    fn includes_function(&self, def_id: GlobalDefId) -> bool {
        match self {
            Self::All => true,
            Self::ReachableItems {
                functions,
                already_checked_functions,
                discovered_functions,
                ..
            } => {
                (functions.contains(&def_id) || discovered_functions.contains(&def_id))
                    && already_checked_functions.is_none_or(|checked| !checked.contains(&def_id))
            }
        }
    }

    fn includes_global(&self, def_id: GlobalDefId) -> bool {
        match self {
            Self::All => true,
            Self::ReachableItems {
                globals,
                already_checked_globals,
                ..
            } => {
                globals.contains(&def_id)
                    && already_checked_globals.is_none_or(|checked| !checked.contains(&def_id))
            }
        }
    }

    fn add_function(&mut self, def_id: GlobalDefId) -> bool {
        match self {
            Self::All => false,
            Self::ReachableItems {
                functions,
                already_checked_functions,
                discovered_functions,
                ..
            } => {
                if already_checked_functions.is_some_and(|checked| checked.contains(&def_id)) {
                    return false;
                }
                if functions.contains(&def_id) {
                    return false;
                }
                discovered_functions.insert(def_id)
            }
        }
    }

    fn initial_functions(
        &self,
        available: &HashMap<GlobalDefId, FunctionItemRef<'_>>,
    ) -> Vec<GlobalDefId> {
        match self {
            Self::All => available.keys().copied().collect(),
            Self::ReachableItems {
                functions,
                already_checked_functions,
                ..
            } => functions
                .iter()
                .copied()
                .filter(|def_id| {
                    already_checked_functions.is_none_or(|checked| !checked.contains(def_id))
                })
                .filter(|def_id| available.contains_key(def_id))
                .collect(),
        }
    }
}

fn empty_global_def_ids() -> &'static HashSet<GlobalDefId> {
    static EMPTY: std::sync::OnceLock<HashSet<GlobalDefId>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(HashSet::new)
}

#[derive(Clone, Copy)]
pub struct BodyProgramContext<'a> {
    pub defs: Option<&'a dyn Fn(ModuleId) -> Option<DefCollection>>,
    pub type_normalizations: Option<&'a dyn Fn(ModuleId) -> Option<TypeNormalization>>,
    pub extension_type_normalizations: Option<&'a dyn Fn(ModuleId) -> Option<TypeNormalization>>,
    pub signatures: Option<&'a dyn Fn(ModuleId) -> Option<ItemSignatures>>,
    pub layouts: Option<&'a dyn Fn(ModuleId) -> Option<Layouts>>,
    pub visible_extensions: Option<&'a dyn Fn(ModuleId) -> Option<VisibleExtensionMethods>>,
}

impl<'a> BodyProgramContext<'a> {
    pub fn empty() -> Self {
        Self {
            defs: None,
            type_normalizations: None,
            extension_type_normalizations: None,
            signatures: None,
            layouts: None,
            visible_extensions: None,
        }
    }
}

enum ModuleDefs<'a> {
    Borrowed(&'a DefCollection),
    Owned(DefCollection),
}

impl ModuleDefs<'_> {
    fn as_ref(&self) -> &DefCollection {
        match self {
            ModuleDefs::Borrowed(defs) => defs,
            ModuleDefs::Owned(defs) => defs,
        }
    }
}

impl fmt::Debug for BodyProgramContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BodyProgramContext")
            .field("defs", &self.defs.is_some())
            .field("type_normalizations", &self.type_normalizations.is_some())
            .field(
                "extension_type_normalizations",
                &self.extension_type_normalizations.is_some(),
            )
            .field("signatures", &self.signatures.is_some())
            .field("layouts", &self.layouts.is_some())
            .finish()
    }
}

#[derive(Clone)]
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
    pub signatures: BodyLocalSignatures<'a>,
    pub comptime_signatures: &'a ItemSignatures,
    pub normalization: &'a TypeNormalization,
    pub seed_interner: Option<TyInterner>,
    pub target: &'a TargetConfig,
    pub comptime: BodyComptime<'a>,
    pub comptime_module: &'a ResolvedComptimeModule,
    pub layouts: &'a Layouts,
    pub extensions: &'a VisibleExtensionMethods,
    pub program_extension_methods: &'a ExtensionMethods,
    pub extension_interner: Option<&'a TyInterner>,
    pub program: BodyProgramContext<'a>,
    pub program_signatures: ProgramSignatureContext<'a>,
    pub function_scope: FunctionCheckScope,
    pub program_comptime: ProgramComptimeMaps<'a>,
    pub filter: BodyCheckFilter<'a>,
}

#[derive(Debug, Clone, Copy)]
pub struct BodyLocalSignatures<'a> {
    pub functions: &'a HashMap<DefId, FunctionSignature>,
    pub globals: &'a HashMap<DefId, GlobalSignature>,
    pub comptimes: &'a HashMap<DefId, ComptimeSignature>,
    pub structs: &'a HashMap<DefId, StructSignature>,
    pub unions: &'a HashMap<DefId, UnionSignature>,
    pub enums: &'a HashMap<DefId, EnumSignature>,
    pub type_aliases: &'a HashMap<DefId, TypeAliasSignature>,
    pub traits: &'a HashMap<DefId, TraitSignature>,
    pub trait_impls: &'a [TraitImplSignature],
}

impl<'a> BodyLocalSignatures<'a> {
    pub fn from_item_signatures(signatures: &'a ItemSignatures) -> Self {
        Self {
            functions: &signatures.functions,
            globals: &signatures.globals,
            comptimes: &signatures.comptimes,
            structs: &signatures.structs,
            unions: &signatures.unions,
            enums: &signatures.enums,
            type_aliases: &signatures.type_aliases,
            traits: &signatures.traits,
            trait_impls: &signatures.trait_impls,
        }
    }
}

#[derive(Clone, Copy)]
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
    pub comptime: BodyComptime<'a>,
    pub comptime_module: &'a ResolvedComptimeModule,
    pub extensions: &'a VisibleExtensionMethods,
    pub program_extension_methods: &'a ExtensionMethods,
    pub program: BodyProgramContext<'a>,
    pub program_signatures: ProgramSignatureContext<'a>,
    pub function_scope: FunctionCheckScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionCheckScope {
    LocalModule,
    ProgramSignatures,
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
    let empty_comptime_module = ResolvedComptimeModule::default();
    let empty_extensions = VisibleExtensionMethods::default();
    let empty_program_extension_methods = ExtensionMethods::default();
    let empty_comptime_values = HashMap::new();
    let empty_typed_comptime_values = HashMap::new();
    let empty_array_lengths = HashMap::new();
    let empty_comptime = BodyComptime {
        interner: &lowered.interner,
        values: &empty_comptime_values,
        typed_values: &empty_typed_comptime_values,
        array_lengths: &empty_array_lengths,
    };
    let target = TargetConfig::host();
    let source_path = SourcePath::new("main.nia");
    let item_tree = ModuleItemTree::from_module(module);
    let active_item_tree = ActiveModuleItemTree::new(
        item_tree.active_items_without_comptime(),
        Default::default(),
    );
    let semantic_uses = semantic_use_table_for_body_input(
        defs.module_id,
        values,
        locals,
        lowered,
        &active_item_tree,
    );
    let input = BodyCheckInput {
        source_version: None,
        source_path: &source_path,
        origins: &NodeOriginTable::default(),
        active_item_tree: &active_item_tree,
        defs,
        values,
        locals,
        semantic_uses: &semantic_uses,
        lowered,
        signatures: BodyLocalSignatures::from_item_signatures(signatures),
        comptime_signatures: signatures,
        normalization: &empty_normalization,
        seed_interner: None,
        target: &target,
        comptime: empty_comptime,
        comptime_module: &empty_comptime_module,
        layouts: &layouts,
        extensions: &empty_extensions,
        program_extension_methods: &empty_program_extension_methods,
        extension_interner: None,
        program: BodyProgramContext::empty(),
        program_signatures: ProgramSignatureContext::empty(),
        program_comptime: ProgramComptimeMaps::empty(),
        function_scope: FunctionCheckScope::LocalModule,
        filter: BodyCheckFilter::All,
    };
    let mut checked = check_module_bodies_with_program_signatures_and_layouts_with_timings(
        input,
        nia_timing::TimingMode::Off,
    );
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
        signatures: BodyLocalSignatures::from_item_signatures(input.signatures),
        comptime_signatures: input.signatures,
        normalization: input.normalization,
        seed_interner: None,
        target: input.target,
        comptime: input.comptime,
        comptime_module: input.comptime_module,
        layouts: &layouts,
        extensions: input.extensions,
        program_extension_methods: input.program_extension_methods,
        extension_interner: None,
        program: input.program,
        program_signatures: input.program_signatures,
        function_scope: input.function_scope,
        program_comptime: ProgramComptimeMaps::empty(),
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
    active_item_tree: &ActiveModuleItemTree,
) -> SemanticUseTable {
    let mut builder = SemanticUseTable::builder();
    for (key, local_use) in &locals.node_uses {
        match local_use {
            nia_local_resolve::LocalUse::Local(local_id) => {
                builder.insert_node_local_value_use(key.clone(), *local_id);
            }
            nia_local_resolve::LocalUse::Static(global_id) => {
                builder.insert_node_global_value_use(key.clone(), *global_id);
            }
            nia_local_resolve::LocalUse::ModuleValue
            | nia_local_resolve::LocalUse::Module
            | nia_local_resolve::LocalUse::TypePrefix
            | nia_local_resolve::LocalUse::Unresolved => {}
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
    builder
        .extend_node_type_uses(lowered.versioned_type_uses_from_active_item_tree(active_item_tree));
    builder.finish()
}

pub fn check_module_bodies_with_program_signatures_and_layouts(
    input: BodyCheckInput<'_>,
) -> BodyCheck {
    check_module_bodies_with_program_signatures_and_layouts_with_timings(
        input,
        nia_timing::TimingMode::Off,
    )
}

pub fn check_module_bodies_with_program_signatures_and_layouts_with_timings(
    input: BodyCheckInput<'_>,
    timings: nia_timing::TimingMode,
) -> BodyCheck {
    let timing = timings.detail();
    let module_id = input.defs.module_id;
    let mut interner = input
        .seed_interner
        .clone()
        .unwrap_or_else(|| input.normalization.interner.clone());
    let extension_methods_by_id = time_body_stage(
        timing,
        "body_check.extension_method_lookup",
        module_id,
        || {
            BodyChecker::extension_method_lookup(
                module_id,
                input.defs,
                input.signatures,
                input.extensions,
                input.program_extension_methods,
                &mut interner,
                &input.lowered.interner,
                input.normalization,
                input.extension_interner,
                input.program.extension_type_normalizations,
            )
        },
    );
    let extensions = time_body_stage(
        timing,
        "body_check.import_visible_extensions",
        module_id,
        || {
            import_visible_extensions_into_working_interner(
                input.extensions,
                input.extension_interner,
                &mut interner,
            )
        },
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
        signatures: input.signatures,
        comptime_signatures: input.comptime_signatures,
        normalization: input.normalization,
        target: input.target,
        comptime: input.comptime,
        comptime_module: input.comptime_module,
        layouts: input.layouts,
        extensions,
        program_extension_methods: input.program_extension_methods,
        program_signature_scope: match input.function_scope {
            FunctionCheckScope::LocalModule => ProgramSignatureScope::LocalModule,
            FunctionCheckScope::ProgramSignatures => {
                ProgramSignatureScope::Program(input.program_signatures.lookup)
            }
        },
        program_trait_impls: input.program_signatures.trait_impls,
        program_comptime_values: input.program_comptime.values,
        program_comptime_array_lengths: input.program_comptime.array_lengths,
        program_comptime_module: input.program_comptime.module,
        source_path: input.source_path,
        extension_methods_by_id,
        extension_method_lookup_cache: HashMap::new(),
        callable_extension_methods_by_name: HashMap::new(),
        node_expr_types: HashMap::new(),
        node_bracket_suffix_resolutions: HashMap::new(),
        node_pointer_array_to_slice_coercions: HashMap::new(),
        node_trait_object_coercions: HashMap::new(),
        node_trait_object_upcasts: HashMap::new(),
        node_builtin_values: HashMap::new(),
        node_associated_comptime_projections: HashMap::new(),
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
        def_trait_obligations_cache: HashMap::new(),
        trait_obligation_resolution_cache: HashMap::new(),
        program_type_normalizations: RefCell::new(HashMap::new()),
        diagnostics: Vec::new(),
        timing,
        timing_module_id: module_id,
        current_return: void_ty,
        current_def_id: None,
        current_param_locals: Vec::new(),
        comptime_context_depth: 0,
        comptime_call_locals: Vec::new(),
        body_filter: ActiveBodyCheckFilter::from_filter(input.filter),
        checked_functions: HashSet::new(),
        pending_functions: VecDeque::new(),
        profile: nia_timing::TimingAccumulator::default(),
    });
    time_body_stage(timing, "body_check.seed_global_types", module_id, || {
        checker.seed_global_types();
    });
    time_body_stage(timing, "body_check.check_module", module_id, || {
        checker.check_module(input.active_item_tree, timing, module_id);
    });
    checker.print_profile();
    time_body_stage(timing, "body_check.finish", module_id, || BodyCheck {
        ir: BodyIr {
            interner: checker.interner,
            function_bodies: checker.function_bodies,
            global_inits: checker.global_inits,
        },
        facts: SemanticFacts {
            global_types: checker
                .global_types
                .into_iter()
                .map(|(def_id, ty)| (GlobalDefId { module_id, def_id }, ty))
                .collect(),
            generic_instantiations: checker.generic_instantiations,
            function_facts: checker.function_facts,
            node_expr_types: checker.node_expr_types,
            node_bracket_suffix_resolutions: checker.node_bracket_suffix_resolutions,
            node_pointer_array_to_slice_coercions: checker.node_pointer_array_to_slice_coercions,
            node_trait_object_coercions: checker.node_trait_object_coercions,
            node_trait_object_upcasts: checker.node_trait_object_upcasts,
            node_builtin_values: checker.node_builtin_values,
            node_builtin_associated_values: input
                .semantic_uses
                .node_builtin_associated_values
                .clone(),
            node_associated_comptime_projections: checker.node_associated_comptime_projections,
            node_array_repeat_counts: checker.node_array_repeat_counts,
            node_switch_pattern_values: checker.node_switch_pattern_values,
            node_resolved_calls: checker.node_resolved_calls,
            node_function_references: checker.node_function_references,
        },
        checked_functions: checker.checked_functions,
        diagnostics: checker.diagnostics,
    })
}

fn time_body_stage<T>(enabled: bool, name: &str, module_id: ModuleId, f: impl FnOnce() -> T) -> T {
    if !enabled {
        return f();
    }
    nia_timing::time_query(
        nia_timing::TimingMode::Detail,
        &format!("{name}[{module_id:?}]"),
        f,
    )
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
    nia_timing::time_query_if_slow(
        nia_timing::TimingMode::Detail,
        &format!("{name}[{module_id:?} {detail}]"),
        std::time::Duration::from_secs_f64(threshold_seconds),
        f,
    )
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
    signatures: BodyLocalSignatures<'a>,
    comptime_signatures: &'a ItemSignatures,
    normalization: &'a TypeNormalization,
    target: &'a TargetConfig,
    comptime: BodyComptime<'a>,
    comptime_module: &'a ResolvedComptimeModule,
    layouts: &'a Layouts,
    extensions: VisibleExtensionMethods,
    program_extension_methods: &'a ExtensionMethods,
    program_signature_scope: ProgramSignatureScope<'a>,
    program_trait_impls: &'a [ProgramTraitImplSignature],
    program_comptime_values: &'a dyn Fn(ModuleId) -> Option<ComptimeValues>,
    program_comptime_array_lengths: &'a dyn Fn(ModuleId) -> Option<ComptimeArrayLengths>,
    program_comptime_module: &'a dyn Fn(ModuleId) -> Option<ResolvedComptimeModule>,
    source_path: &'a SourcePath,
    extension_methods_by_id: Arc<HashMap<GlobalDefId, ExtensionMethodLookup>>,
    extension_method_lookup_cache: HashMap<GlobalDefId, ExtensionMethodLookup>,
    callable_extension_methods_by_name: HashMap<String, CallableExtensionMethods>,
    node_expr_types: HashMap<VersionedNodeKey, InternedTyId>,
    node_bracket_suffix_resolutions: HashMap<VersionedNodeKey, BracketSuffixResolution>,
    node_pointer_array_to_slice_coercions: HashMap<VersionedNodeKey, PointerArrayToSliceCoercion>,
    node_trait_object_coercions: HashMap<VersionedNodeKey, TraitObjectCoercion>,
    node_trait_object_upcasts: HashMap<VersionedNodeKey, TraitObjectUpcast>,
    node_builtin_values: HashMap<VersionedNodeKey, BuiltinValue>,
    node_associated_comptime_projections: HashMap<VersionedNodeKey, AssociatedComptimeProjection>,
    node_array_repeat_counts: HashMap<VersionedNodeKey, u64>,
    node_switch_pattern_values: HashMap<VersionedNodeKey, i128>,
    node_resolved_calls: HashMap<VersionedNodeKey, ResolvedCall>,
    node_function_references: HashMap<VersionedNodeKey, FunctionReference>,
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
    def_trait_obligations_cache: HashMap<DefId, Vec<TraitObligation>>,
    trait_obligation_resolution_cache:
        HashMap<TraitObligationResolutionKey, nia_trait_solve::TraitResolution>,
    program_type_normalizations: RefCell<HashMap<ModuleId, TypeNormalization>>,
    diagnostics: Vec<Diagnostic>,
    timing: bool,
    timing_module_id: ModuleId,
    current_return: InternedTyId,
    current_def_id: Option<GlobalDefId>,
    current_param_locals: Vec<LocalId>,
    comptime_context_depth: usize,
    comptime_call_locals: Vec<ComptimeCallFrame>,
    body_filter: ActiveBodyCheckFilter<'a>,
    checked_functions: HashSet<GlobalDefId>,
    pending_functions: VecDeque<GlobalDefId>,
    profile: nia_timing::TimingAccumulator,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TraitObligationResolutionKey {
    current_def_id: Option<GlobalDefId>,
    self_ty: InternedTyId,
    trait_id: nia_ty::TraitId,
    trait_args: Vec<InternedTyId>,
    trait_const_args: Vec<ConstGenericArg>,
}

#[derive(Clone, Copy)]
enum ProgramSignatureScope<'a> {
    LocalModule,
    Program(&'a dyn ProgramSignatureLookup),
}

impl<'a> ProgramSignatureScope<'a> {
    fn function(&self, def_id: GlobalDefId) -> Option<ProgramFunctionSignature> {
        match self {
            ProgramSignatureScope::LocalModule => None,
            ProgramSignatureScope::Program(program) => program.function(def_id),
        }
    }

    fn includes_function(&self, def_id: GlobalDefId) -> bool {
        match self {
            ProgramSignatureScope::LocalModule => true,
            ProgramSignatureScope::Program(program) => program.has_function(def_id),
        }
    }

    fn global(&self, def_id: GlobalDefId) -> Option<ProgramGlobalSignature> {
        match self {
            ProgramSignatureScope::LocalModule => None,
            ProgramSignatureScope::Program(program) => program.global(def_id),
        }
    }

    fn comptime(&self, def_id: GlobalDefId) -> Option<ProgramComptimeSignature> {
        match self {
            ProgramSignatureScope::LocalModule => None,
            ProgramSignatureScope::Program(program) => program.comptime(def_id),
        }
    }

    fn struct_(&self, def_id: GlobalDefId) -> Option<ProgramStructSignature> {
        match self {
            ProgramSignatureScope::LocalModule => None,
            ProgramSignatureScope::Program(program) => program.struct_(def_id),
        }
    }

    fn union(&self, def_id: GlobalDefId) -> Option<ProgramUnionSignature> {
        match self {
            ProgramSignatureScope::LocalModule => None,
            ProgramSignatureScope::Program(program) => program.union(def_id),
        }
    }

    fn enum_(&self, def_id: GlobalDefId) -> Option<ProgramEnumSignature> {
        match self {
            ProgramSignatureScope::LocalModule => None,
            ProgramSignatureScope::Program(program) => program.enum_(def_id),
        }
    }

    fn trait_(&self, def_id: GlobalDefId) -> Option<ProgramTraitSignature> {
        match self {
            ProgramSignatureScope::LocalModule => None,
            ProgramSignatureScope::Program(program) => program.trait_(def_id),
        }
    }

    fn type_alias(&self, def_id: GlobalDefId) -> Option<ProgramTypeAliasSignature> {
        match self {
            ProgramSignatureScope::LocalModule => None,
            ProgramSignatureScope::Program(program) => program.type_alias(def_id),
        }
    }

    fn has_enum(&self, def_id: GlobalDefId) -> bool {
        match self {
            ProgramSignatureScope::LocalModule => false,
            ProgramSignatureScope::Program(program) => program.has_enum(def_id),
        }
    }

    fn has_union(&self, def_id: GlobalDefId) -> bool {
        match self {
            ProgramSignatureScope::LocalModule => false,
            ProgramSignatureScope::Program(program) => program.has_union(def_id),
        }
    }

    fn trait_ids_with_method_named(&self, name: &str) -> Vec<GlobalDefId> {
        match self {
            ProgramSignatureScope::LocalModule => Vec::new(),
            ProgramSignatureScope::Program(program) => program.trait_ids_with_method_named(name),
        }
    }

    fn trait_owning_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<(GlobalDefId, ProgramTraitSignature)> {
        match self {
            ProgramSignatureScope::LocalModule => None,
            ProgramSignatureScope::Program(program) => program.trait_owning_method(method_id),
        }
    }
}

impl<'a> BodyChecker<'a> {
    fn program_is_enum(&self, def_id: GlobalDefId) -> bool {
        self.program_signature_scope.has_enum(def_id)
    }
}

#[derive(Debug, Clone)]
struct ExtensionMethodLookup {
    target_ty: InternedTyId,
    impl_id: nia_ids::TraitImplId,
    effective_generics: Vec<String>,
    where_predicates: Vec<nia_defs::WherePredicateSignature>,
}

#[derive(Debug, Clone, Default)]
struct ComptimeCallFrame {
    module_id: Option<ModuleId>,
    function_id: Option<GlobalDefId>,
    locals: HashMap<LocalId, nia_comptime_check::ComptimeValue>,
    local_types: HashMap<LocalId, nia_comptime_check::ComptimeValueType>,
    mutable_locals: HashSet<LocalId>,
    type_substitutions: HashMap<String, InternedTyId>,
    const_substitutions: HashMap<String, ConstGenericArg>,
}

struct FunctionItemRef<'a> {
    item_span: Span,
    kind: DefKind,
    function: &'a FunctionItem,
}

#[derive(Debug, Clone, PartialEq)]
struct ReceiverBase {
    def_id: GlobalDefId,
    args: Vec<InternedTyId>,
    const_args: Vec<ConstGenericArg>,
    from_pointer: bool,
    has_readonly_pointer: bool,
}

#[derive(Debug, Clone)]
struct CallableExtensionMethod {
    target_ty: InternedTyId,
    method: VisibleExtensionMethod,
}

#[derive(Debug, Clone, Default)]
struct CallableExtensionMethods {
    methods: Vec<CallableExtensionMethod>,
    unbased_methods: Vec<usize>,
    methods_by_base: HashMap<GlobalDefId, Vec<usize>>,
}

fn import_visible_extensions_into_working_interner(
    extensions: &VisibleExtensionMethods,
    source: Option<&TyInterner>,
    target: &mut TyInterner,
) -> VisibleExtensionMethods {
    let Some(source) = source else {
        return extensions.clone();
    };
    let mut imported = VisibleExtensionMethods::default();
    for extension_target in extensions.targets() {
        let target_ty = nia_ty::import_type_into(target, source, extension_target.target_ty);
        for method in &extension_target.methods {
            let mut method = method.clone();
            method.trait_args = method
                .trait_args
                .iter()
                .map(|arg| nia_ty::import_type_into(target, source, *arg))
                .collect();
            method.where_predicates =
                import_where_predicates_between_interners(target, source, &method.where_predicates);
            imported.insert(extension_target.impl_id, target_ty, method);
        }
        for value in &extension_target.associated_values {
            imported.insert_associated_value(extension_target.impl_id, target_ty, value.clone());
        }
    }
    for (module_id, impl_id) in extensions.trait_witness_impls() {
        imported.insert_trait_witness_impl(module_id, impl_id);
    }
    imported
}

fn import_where_predicates_between_interners(
    target: &mut TyInterner,
    source: &TyInterner,
    predicates: &[nia_defs::WherePredicateSignature],
) -> Vec<nia_defs::WherePredicateSignature> {
    predicates
        .iter()
        .map(|predicate| nia_defs::WherePredicateSignature {
            ty: nia_ty::import_type_into(target, source, predicate.ty),
            bounds: predicate
                .bounds
                .iter()
                .map(|bound| nia_defs::WhereBoundSignature {
                    trait_ty: nia_ty::import_type_into(target, source, bound.trait_ty),
                    associated_type_bindings: bound
                        .associated_type_bindings
                        .iter()
                        .map(|binding| nia_defs::AssociatedTypeBindingSignature {
                            name: binding.name.clone(),
                            ty: nia_ty::import_type_into(target, source, binding.ty),
                            span: binding.span,
                        })
                        .collect(),
                    span: bound.span,
                })
                .collect(),
            span: predicate.span,
        })
        .collect()
}

impl<'a> BodyChecker<'a> {
    fn extension_method_lookup(
        module_id: ModuleId,
        defs: &DefCollection,
        signatures: BodyLocalSignatures<'_>,
        extensions: &VisibleExtensionMethods,
        _program_extensions: &ExtensionMethods,
        interner: &mut TyInterner,
        local_type_interner: &TyInterner,
        local_normalization: &TypeNormalization,
        extension_interner: Option<&TyInterner>,
        _program_normalizations: Option<&dyn Fn(ModuleId) -> Option<TypeNormalization>>,
    ) -> Arc<HashMap<GlobalDefId, ExtensionMethodLookup>> {
        let mut methods = HashMap::new();
        for impl_signature in signatures.trait_impls {
            if impl_signature.builtin.is_some() {
                continue;
            }
            let target_ty = local_normalization.normalize(impl_signature.target_ty);
            let target_ty = nia_ty::import_type_into(interner, local_type_interner, target_ty);
            for method in &impl_signature.methods {
                let mut effective_generics = impl_signature.generics.clone();
                if let Some(def) = defs.defs.get(method.def_id) {
                    effective_generics.extend(def.generics.iter().cloned());
                }
                methods.insert(
                    GlobalDefId {
                        module_id,
                        def_id: method.def_id,
                    },
                    ExtensionMethodLookup {
                        target_ty,
                        impl_id: impl_signature.impl_id,
                        effective_generics,
                        where_predicates: impl_signature.where_predicates.clone(),
                    },
                );
            }
        }
        for target in extensions.targets() {
            let target_ty = extension_interner
                .map(|source| nia_ty::import_type_into(interner, source, target.target_ty))
                .unwrap_or(target.target_ty);
            for method in &target.methods {
                let mut method = method.clone();
                if let Some(source) = extension_interner {
                    method.trait_args = method
                        .trait_args
                        .iter()
                        .map(|arg| nia_ty::import_type_into(interner, source, *arg))
                        .collect();
                    method.where_predicates = import_where_predicates_between_interners(
                        interner,
                        source,
                        &method.where_predicates,
                    );
                }
                methods
                    .entry(method.def_id)
                    .or_insert_with(|| ExtensionMethodLookup {
                        target_ty,
                        impl_id: method.impl_id,
                        effective_generics: method.effective_generics.clone(),
                        where_predicates: method.where_predicates.clone(),
                    });
            }
        }
        Arc::new(methods)
    }

    fn extension_method_lookup_for_id(
        &self,
        method_id: GlobalDefId,
    ) -> Option<&ExtensionMethodLookup> {
        self.extension_method_lookup_cache
            .get(&method_id)
            .or_else(|| self.extension_methods_by_id.get(&method_id))
    }

    fn import_program_extension_method_lookup(
        &mut self,
        method: &nia_defs::ExtensionMethod,
    ) -> Option<ExtensionMethodLookup> {
        if method.def_id.module_id != self.defs.module_id && method.visibility != Visibility::Public
        {
            return None;
        }
        let program_normalizations = self.program.extension_type_normalizations?;
        let normalization = program_normalizations(method.def_id.module_id)?;
        let source_interner = &normalization.interner;
        let target_ty = normalization.normalize(method.target_ty);
        let target_ty = nia_ty::import_type_into(&mut self.interner, source_interner, target_ty);
        let where_predicates = import_where_predicates_between_interners(
            &mut self.interner,
            source_interner,
            &method.where_predicates,
        );
        Some(ExtensionMethodLookup {
            target_ty,
            impl_id: method.impl_id,
            effective_generics: method.effective_generics.clone(),
            where_predicates,
        })
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

    fn record_resolved_node_call(
        &mut self,
        _span: Span,
        key: &VersionedNodeKey,
        call: ResolvedCall,
    ) {
        self.enqueue_same_module_resolved_call(&call);
        self.node_resolved_calls.insert(key.clone(), call.clone());
        if let Some(facts) = self.current_function_facts() {
            facts.node_resolved_calls.insert(key.clone(), call);
        }
    }

    fn enqueue_same_module_resolved_call(&mut self, call: &ResolvedCall) {
        let def_id = match call {
            ResolvedCall::Function(def_id)
            | ResolvedCall::FunctionInstance { def_id, .. }
            | ResolvedCall::Method { def_id, .. } => *def_id,
            ResolvedCall::TraitMethod { method_id, .. }
            | ResolvedCall::TraitAssociatedFunction { method_id, .. } => *method_id,
            ResolvedCall::DynamicTraitMethod { .. }
            | ResolvedCall::BuiltinFunction { .. }
            | ResolvedCall::BuiltinTraitMethod { .. }
            | ResolvedCall::BuiltinMethod { .. }
            | ResolvedCall::BuiltinPlaceMethod { .. }
            | ResolvedCall::FunctionPointer => return,
        };
        if Some(def_id.module_id) != self.current_def_id.map(|current| current.module_id) {
            return;
        }
        if self.checked_functions.contains(&def_id) {
            return;
        }
        if self.body_filter.add_function(def_id) {
            self.pending_functions.push_back(def_id);
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

    fn record_associated_comptime_projection(
        &mut self,
        expr: &Expr,
        projection: AssociatedComptimeProjection,
    ) {
        self.node_associated_comptime_projections
            .insert(expr.node_key.clone(), projection.clone());
        if let Some(facts) = self.current_function_facts() {
            facts
                .node_associated_comptime_projections
                .insert(expr.node_key.clone(), projection);
        }
    }

    fn record_function_node_reference(
        &mut self,
        _span: Span,
        key: &VersionedNodeKey,
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
        if self.interner.get(ty).is_some() {
            return ty;
        }
        if let Some(source) = self.interner_containing_ty(ty) {
            nia_ty::import_type_into(&mut self.interner, &source, ty)
        } else {
            ty
        }
    }

    fn import_lowered_type_to_working_interner(&mut self, ty: InternedTyId) -> InternedTyId {
        if self.type_lowering.interner.get(ty).is_some() {
            return nia_ty::import_type_into(&mut self.interner, &self.type_lowering.interner, ty);
        }
        self.import_type_to_working_interner(ty)
    }

    fn interner_containing_ty(&self, ty: InternedTyId) -> Option<TyInterner> {
        if self.normalization.interner.get(ty).is_some() {
            return Some(self.normalization.interner.clone());
        }
        if self.comptime.interner.get(ty).is_some() {
            return Some(self.comptime.interner.clone());
        }
        if self.interner.get(ty).is_some() {
            return Some(self.interner.clone());
        }
        let module_id = ty.owner().module_id();
        if !self
            .program_type_normalizations
            .borrow()
            .contains_key(&module_id)
        {
            let normalization = (self.program.type_normalizations?)(module_id)?;
            self.program_type_normalizations
                .borrow_mut()
                .insert(module_id, normalization);
        }
        let normalizations = self.program_type_normalizations.borrow();
        let interner = &normalizations.get(&module_id)?.interner;
        (ty.interner_id == interner.interner_id() && interner.get(ty).is_some())
            .then(|| interner.clone())
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
        let ty = self.type_lowering.ty_for_key(&expr.node_key)?;
        Some(self.import_lowered_type_to_working_interner(ty))
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

    fn local_def(&self, key: &VersionedNodeKey) -> Option<LocalId> {
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

    fn with_comptime_context<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.comptime_context_depth += 1;
        let result = f(self);
        self.comptime_context_depth -= 1;
        result
    }

    fn in_comptime_context(&self) -> bool {
        self.comptime_context_depth > 0
    }

    fn defs_for_module(&self, module_id: ModuleId) -> Option<ModuleDefs<'_>> {
        if module_id == self.defs.module_id {
            Some(ModuleDefs::Borrowed(self.defs))
        } else {
            Some(ModuleDefs::Owned((self.program.defs?)(module_id)?))
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
    pub(crate) fn profile_stage<T>(
        &mut self,
        name: &'static str,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        if !self.timing {
            return f(self);
        }
        let mut profile = std::mem::take(&mut self.profile);
        let result = profile.time(name, || f(self));
        self.profile = profile;
        result
    }

    fn print_profile(&self) {
        if !self.timing || self.profile.is_empty() {
            return;
        }
        self.profile
            .emit_query_timings(|name| format!("{name}[{:?}]", self.timing_module_id));
    }

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
            let function_items = self.function_items_by_id(active_item_tree);
            self.check_reachable_functions(&function_items, timing, module_id);
        });
        time_body_stage(timing, "body_check.extends", module_id, || {
            for item in &active_item_tree.items {
                if let ItemTreeNodeKind::Extend(extend) = &item.kind {
                    if extend.generics.is_empty() {
                        for associated_value in &extend.associated_values {
                            if associated_value.binding.value.is_none() {
                                continue;
                            }
                            self.check_reachable_comptime_binding(
                                associated_value.span,
                                &associated_value.binding,
                            );
                        }
                    }
                }
            }
        });
    }

    fn function_items_by_id<'ast>(
        &mut self,
        active_item_tree: &'ast ActiveModuleItemTree,
    ) -> HashMap<GlobalDefId, FunctionItemRef<'ast>> {
        let mut items = HashMap::new();
        for item in &active_item_tree.items {
            self.collect_function_items_by_id(item, &mut items);
        }
        items
    }

    fn collect_function_items_by_id<'ast>(
        &mut self,
        item: &'ast ItemTreeNode,
        items: &mut HashMap<GlobalDefId, FunctionItemRef<'ast>>,
    ) {
        match &item.kind {
            ItemTreeNodeKind::Function(function) => {
                self.insert_function_item(item.span, DefKind::Function, function, items);
            }
            ItemTreeNodeKind::Trait(item_trait) => {
                for method in &item_trait.methods {
                    self.insert_function_item(
                        method.function.span,
                        DefKind::TraitMethod,
                        &method.function,
                        items,
                    );
                }
            }
            ItemTreeNodeKind::Extend(extend) => {
                if has_builtin_attribute(&item.attributes) {
                    return;
                }
                for method in &extend.methods {
                    self.insert_function_item(
                        method.function.span,
                        DefKind::Method,
                        &method.function,
                        items,
                    );
                }
            }
            ItemTreeNodeKind::Module(_)
            | ItemTreeNodeKind::Using(_)
            | ItemTreeNodeKind::Struct(_)
            | ItemTreeNodeKind::Union(_)
            | ItemTreeNodeKind::Enum(_)
            | ItemTreeNodeKind::Binding(_)
            | ItemTreeNodeKind::TypeAlias(_) => {}
        }
    }

    fn insert_function_item<'ast>(
        &mut self,
        item_span: Span,
        kind: DefKind,
        function: &'ast FunctionItem,
        items: &mut HashMap<GlobalDefId, FunctionItemRef<'ast>>,
    ) {
        let Some(def_id) = self.def_id_for_node(&function.node_key, function.span, kind) else {
            return;
        };
        let global_def_id = self.global_def_id(def_id);
        items.insert(
            global_def_id,
            FunctionItemRef {
                item_span,
                kind,
                function,
            },
        );
    }

    fn check_reachable_functions<'ast>(
        &mut self,
        function_items: &HashMap<GlobalDefId, FunctionItemRef<'ast>>,
        timing: bool,
        module_id: ModuleId,
    ) {
        let initial = self.body_filter.initial_functions(function_items);
        for def_id in initial {
            self.check_reachable_function_by_id(def_id, function_items, timing, module_id);
        }
        while let Some(def_id) = self.pending_functions.pop_front() {
            self.check_reachable_function_by_id(def_id, function_items, timing, module_id);
        }
    }

    fn check_reachable_function_by_id<'ast>(
        &mut self,
        def_id: GlobalDefId,
        function_items: &HashMap<GlobalDefId, FunctionItemRef<'ast>>,
        timing: bool,
        module_id: ModuleId,
    ) {
        if !self.body_filter.includes_function(def_id) || !self.checked_functions.insert(def_id) {
            return;
        }
        let Some(item) = function_items.get(&def_id) else {
            return;
        };
        let stage = match item.kind {
            DefKind::Function => "body_check.function",
            DefKind::TraitMethod => "body_check.trait_method",
            DefKind::Method => "body_check.extend_method",
            _ => "body_check.function",
        };
        let threshold = if item.kind == DefKind::Method {
            0.010
        } else {
            0.050
        };
        time_body_stage_if_slow(
            timing,
            stage,
            module_id,
            &item.function.name,
            threshold,
            || {
                self.check_function_with_kind(item.item_span, item.kind, item.function);
            },
        );
    }

    fn seed_global_types(&mut self) {
        for (def_id, signature) in self.signatures.globals {
            if let Some(ty) = signature.explicit_type {
                self.global_types.insert(*def_id, ty);
            }
        }
        for (def_id, signature) in self.signatures.comptimes {
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
            if self
                .signatures
                .comptimes
                .get(&def_id)
                .is_some_and(|signature| signature.builtin.is_some())
            {
                return;
            }
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

    fn check_reachable_comptime_binding(
        &mut self,
        item_span: Span,
        binding: &nia_ast::BindingItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, item_span, DefKind::Comptime)
        else {
            return;
        };
        if !self.body_filter.includes_global(self.global_def_id(def_id)) {
            return;
        }
        self.check_comptime_binding(item_span, binding);
    }

    fn comptime_initializer_runtime_type(
        &mut self,
        value: &Expr,
        expected: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        let comptime_expr = self.lower_comptime_expr(value).ok()?;
        let ty = self.comptime_expr_type_for_ir_with_expected(&comptime_expr, expected)?;
        match ty {
            nia_comptime_check::ComptimeValueType::Runtime(ty) => Some(ty),
            _ => None,
        }
    }

    fn check_global_binding(&mut self, item_span: Span, binding: &nia_ast::BindingItem) {
        self.check_global_binding_inner(item_span, binding, true);
    }

    fn check_global_binding_inner(
        &mut self,
        item_span: Span,
        binding: &nia_ast::BindingItem,
        filter_reachable_globals: bool,
    ) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, item_span, DefKind::Global)
        else {
            return;
        };
        if filter_reachable_globals && !self.body_filter.includes_global(self.global_def_id(def_id))
        {
            return;
        }
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
        if !self
            .body_filter
            .includes_function(self.global_def_id(def_id))
        {
            return;
        }
        self.check_function(def_id, function);
    }

    fn check_function_with_kind(
        &mut self,
        item_span: Span,
        kind: DefKind,
        function: &FunctionItem,
    ) {
        match kind {
            DefKind::Function => self.check_function_item(item_span, function),
            DefKind::Method => self.check_function_def(item_span, function),
            DefKind::TraitMethod => self.check_trait_function_def(item_span, function),
            _ => {}
        }
    }

    fn check_function_def(&mut self, _span: Span, function: &FunctionItem) {
        let Some(def_id) = self.def_id_for_node(&function.node_key, function.span, DefKind::Method)
        else {
            return;
        };
        if !self
            .body_filter
            .includes_function(self.global_def_id(def_id))
        {
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
        if !self
            .body_filter
            .includes_function(self.global_def_id(def_id))
        {
            return;
        }
        self.check_function(def_id, function);
    }

    fn check_function(&mut self, def_id: DefId, function: &FunctionItem) {
        let global_def_id = self.global_def_id(def_id);
        if !self
            .program_signature_scope
            .includes_function(global_def_id)
        {
            return;
        }
        let signature = self.profile_stage("body_check.profile.function.signature", |this| {
            if let Some(program_signature) = this.program_signature_scope.function(global_def_id) {
                Some(this.import_program_function_signature(&program_signature))
            } else {
                let raw_signature = this.signatures.functions.get(&def_id).cloned()?;
                Some(this.import_local_function_signature(&raw_signature))
            }
        });
        let Some(signature) = signature else {
            return;
        };
        time_body_stage_if_slow(
            self.timing,
            "body_check.function.projection_obligations",
            self.timing_module_id,
            &function.name,
            0.020,
            || {
                self.profile_stage(
                    "body_check.profile.function.projection_obligations",
                    |this| {
                        this.check_function_signature_projection_obligations(def_id, &signature);
                    },
                );
            },
        );
        let previous_return = self.current_return;
        let previous_def_id = self.current_def_id;
        let previous_param_locals = std::mem::take(&mut self.current_param_locals);
        let previous_local_types = std::mem::take(&mut self.local_types);
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
                self.profile_stage("body_check.profile.function.object_safe", |this| {
                    this.check_object_safe_types_in_signature(&signature);
                });
            },
        );
        time_body_stage_if_slow(
            self.timing,
            "body_check.function.seed_params",
            self.timing_module_id,
            &function.name,
            0.020,
            || {
                self.profile_stage("body_check.profile.function.seed_params", |this| {
                    this.seed_param_types(&signature, function, self_ty);
                });
            },
        );
        if signature.is_comptime {
            self.current_return = previous_return;
            self.current_def_id = previous_def_id;
            self.current_param_locals = previous_param_locals;
            self.local_types = previous_local_types;
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
                    self.profile_stage("body_check.profile.function.check_block", |this| {
                        let body_ty = this.check_block_with_expected(body, expected_tail);
                        if let Some(tail) = body.tail.as_deref() {
                            if !this.is_void(signature.return_type) {
                                this.expect_expr_type(
                                    tail,
                                    signature.return_type,
                                    body_ty,
                                    "function body",
                                );
                            }
                        } else if this.is_void(signature.return_type) {
                            this.expect_type(
                                body.span,
                                signature.return_type,
                                body_ty,
                                "function body",
                            );
                        }
                    });
                },
            );
            let body = time_body_stage_if_slow(
                self.timing,
                "body_check.function.lower_body",
                self.timing_module_id,
                &function.name,
                0.020,
                || {
                    self.profile_stage("body_check.profile.function.lower_body", |this| {
                        this.lower_body(body)
                    })
                },
            );
            self.function_bodies
                .insert(self.global_def_id(def_id), body);
        }
        self.current_return = previous_return;
        self.current_def_id = previous_def_id;
        self.current_param_locals = previous_param_locals;
        self.local_types = previous_local_types;
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
            && let Some(TyKind::Nominal { def_id, args, .. }) = self.interner.get(expected)
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
            | StmtKind::Static(_)
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
            StmtKind::Static(binding) => {
                self.check_global_binding_inner(stmt.span, binding, false);
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
                let iterable_ty = self.check_expr(&for_stmt.iter);
                let (item_ty, _iterator_ty) = self.for_iterable_parts(&for_stmt.iter, iterable_ty);
                self.check_irrefutable_pattern(&for_stmt.pattern, item_ty, "for pattern");
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

    fn for_iterable_parts(
        &mut self,
        iter: &Expr,
        iterable_ty: InternedTyId,
    ) -> (InternedTyId, InternedTyId) {
        if !self.current_context_proves_trait_obligation(
            iterable_ty,
            nia_ty::TraitId::Builtin(nia_ty::BuiltinTrait::Iterable),
            Vec::new(),
        ) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                iter.span,
                format!(
                    "for-in expects an Iterable, found `{}`",
                    self.ty_name(iterable_ty)
                ),
            ));
            return (self.error(), self.error());
        }
        let item_ty = self.iterable_item_projection(iterable_ty);
        let iterator_ty = self.iterable_iter_projection(iterable_ty);
        self.check_for_iterator(iter.span, iterator_ty, item_ty);
        (item_ty, iterator_ty)
    }

    fn check_for_iterator(
        &mut self,
        span: Span,
        iterator_ty: InternedTyId,
        iterable_item_ty: InternedTyId,
    ) {
        if !self.current_context_proves_trait_obligation(
            iterator_ty,
            nia_ty::TraitId::Builtin(nia_ty::BuiltinTrait::Iterator),
            Vec::new(),
        ) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!(
                    "for-in Iterable iterator must implement Iterator, found `{}`",
                    self.ty_name(iterator_ty)
                ),
            ));
            return;
        }
        let iterator_item_ty = self.iterator_item_projection(iterator_ty);
        self.expect_type(
            span,
            iterable_item_ty,
            iterator_item_ty,
            "for iterable item",
        );
    }

    fn lower_for_iterable_parts(
        &mut self,
        iterable_ty: InternedTyId,
    ) -> (InternedTyId, InternedTyId) {
        if !self.current_context_proves_trait_obligation(
            iterable_ty,
            nia_ty::TraitId::Builtin(nia_ty::BuiltinTrait::Iterable),
            Vec::new(),
        ) {
            return (self.error(), self.error());
        }
        (
            self.iterable_item_projection(iterable_ty),
            self.iterable_iter_projection(iterable_ty),
        )
    }

    fn iterable_item_projection(&mut self, iterable_ty: InternedTyId) -> InternedTyId {
        let item = self.interner.intern(TyKind::Projection {
            self_ty: iterable_ty,
            trait_id: nia_ty::TraitId::Builtin(nia_ty::BuiltinTrait::Iterable),
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            name: nia_ty::BuiltinTrait::ITEM_ASSOC_TYPE.to_string(),
        });
        self.normalize_projection(item)
    }

    fn iterable_iter_projection(&mut self, iterable_ty: InternedTyId) -> InternedTyId {
        let iter = self.interner.intern(TyKind::Projection {
            self_ty: iterable_ty,
            trait_id: nia_ty::TraitId::Builtin(nia_ty::BuiltinTrait::Iterable),
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            name: nia_ty::BuiltinTrait::ITER_ASSOC_TYPE.to_string(),
        });
        self.normalize_projection(iter)
    }

    fn iterator_item_projection(&mut self, iter_ty: InternedTyId) -> InternedTyId {
        let item = self.interner.intern(TyKind::Projection {
            self_ty: iter_ty,
            trait_id: nia_ty::TraitId::Builtin(nia_ty::BuiltinTrait::Iterator),
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            name: nia_ty::BuiltinTrait::ITEM_ASSOC_TYPE.to_string(),
        });
        self.normalize_projection(item)
    }

    fn check_irrefutable_pattern(
        &mut self,
        pattern: &nia_ast::Pattern,
        value_ty: InternedTyId,
        context: &str,
    ) -> InternedTyId {
        match &pattern.kind {
            nia_ast::PatternKind::Wildcard => value_ty,
            nia_ast::PatternKind::Bind { node_key, .. } => {
                if let Some(local_id) = self.local_def(node_key) {
                    self.record_local_type(local_id, value_ty);
                }
                value_ty
            }
            nia_ast::PatternKind::Pointer(inner) | nia_ast::PatternKind::MutPointer(inner) => {
                let expected_readonly = matches!(pattern.kind, nia_ast::PatternKind::Pointer(_));
                let elem_ty = match self
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
                            pattern.span,
                            format!("{context} {expected} does not match value type"),
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
                            pattern.span,
                            format!("{context} requires value to be a {expected}"),
                        ));
                        self.error()
                    }
                };
                self.check_irrefutable_pattern(inner, elem_ty, context)
            }
            nia_ast::PatternKind::OptionalSome(_)
            | nia_ast::PatternKind::OptionalNull
            | nia_ast::PatternKind::ErrorOk(_)
            | nia_ast::PatternKind::ErrorErr(_)
            | nia_ast::PatternKind::Expr(_)
            | nia_ast::PatternKind::Range { .. } => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    pattern.span,
                    format!("{context} must be irrefutable"),
                ));
                self.error()
            }
        }
    }

    fn pattern_input_ty(
        &mut self,
        pattern: &nia_ast::Pattern,
        binding_ty: InternedTyId,
    ) -> InternedTyId {
        match &pattern.kind {
            nia_ast::PatternKind::Pointer(inner) => {
                let elem = self.pattern_input_ty(inner, binding_ty);
                self.interner.intern(TyKind::Pointer {
                    is_readonly: true,
                    elem,
                })
            }
            nia_ast::PatternKind::MutPointer(inner) => {
                let elem = self.pattern_input_ty(inner, binding_ty);
                self.interner.intern(TyKind::Pointer {
                    is_readonly: false,
                    elem,
                })
            }
            _ => binding_ty,
        }
    }

    fn materialize_explicit_pattern_ty(
        &mut self,
        pattern: &nia_ast::Pattern,
        explicit_binding: InternedTyId,
        value_ty: InternedTyId,
    ) -> InternedTyId {
        match &pattern.kind {
            nia_ast::PatternKind::Pointer(inner) | nia_ast::PatternKind::MutPointer(inner) => {
                let value_elem = match self.interner.get(self.normalization.normalize(value_ty)) {
                    Some(TyKind::Pointer { elem, .. }) => Some(*elem),
                    _ => None,
                };
                value_elem
                    .map(|elem| self.materialize_explicit_pattern_ty(inner, explicit_binding, elem))
                    .unwrap_or(explicit_binding)
            }
            _ => self
                .materialize_inferred_array_type(explicit_binding, value_ty)
                .unwrap_or(explicit_binding),
        }
    }

    fn single_pattern_binding_key<'b>(
        &self,
        pattern: &'b nia_ast::Pattern,
    ) -> Option<&'b VersionedNodeKey> {
        match &pattern.kind {
            nia_ast::PatternKind::Bind { node_key, .. } => Some(node_key),
            nia_ast::PatternKind::Pointer(inner) | nia_ast::PatternKind::MutPointer(inner) => {
                self.single_pattern_binding_key(inner)
            }
            _ => None,
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
        let Some(binding_key) = self.single_pattern_binding_key(&binding.pattern) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                binding.pattern.span,
                "binding requires a single binding pattern",
            ));
            return;
        };
        if !matches!(binding.pattern.kind, nia_ast::PatternKind::Bind { .. })
            && binding.value.is_none()
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                binding.pattern.span,
                "binding pattern requires an initializer",
            ));
            return self.record_error_local_binding(binding_key);
        }
        let binding_ty = match (&binding.ty, &binding.value) {
            (Some(ty), Some(value)) => {
                let explicit_binding = self.ty_for_type(ty);
                let explicit_input = self.pattern_input_ty(&binding.pattern, explicit_binding);
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
                    return self.record_error_local_binding(binding_key);
                } else {
                    self.expect_expr_type(value, explicit_input, value_ty, "binding initializer");
                }
                self.materialize_explicit_pattern_ty(&binding.pattern, explicit_binding, value_ty)
            }
            (Some(ty), None) => {
                let explicit = self.ty_for_type(ty);
                if matches!(binding.pattern.kind, nia_ast::PatternKind::Bind { .. }) {
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
                    self.check_irrefutable_pattern(&binding.pattern, value_ty, "binding pattern")
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
        if let Some(local_id) = self.local_def(binding_key) {
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

    fn record_error_local_binding(&mut self, key: &VersionedNodeKey) {
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
                    "switch does not destructure optional values; use `if let`",
                ));
            }
            Some(TyKind::ErrorUnion { .. }) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    "switch does not destructure error-union values; use `if let`",
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
            nia_ast::PatternKind::Pointer(inner) | nia_ast::PatternKind::MutPointer(inner) => {
                let expected_readonly = matches!(pattern.kind, nia_ast::PatternKind::Pointer(_));
                let elem_ty = match self.interner.get(self.normalization.normalize(target_ty)) {
                    Some(TyKind::Pointer { is_readonly, elem })
                        if *is_readonly == expected_readonly =>
                    {
                        *elem
                    }
                    Some(TyKind::Pointer { .. }) => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            pattern.span,
                            format!("{context} pointer mutability does not match target"),
                        ));
                        self.error()
                    }
                    _ => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            pattern.span,
                            format!("{context} pointer pattern requires a pointer target"),
                        ));
                        self.error()
                    }
                };
                self.check_pattern(inner, elem_ty, coverage, context);
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
                    "switch over optional values is not supported; use `if let`",
                ));
            }
            Some(TyKind::ErrorUnion { .. }) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    "switch over error-union values is not supported; use `if let`",
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

pub(crate) fn generic_inst_base(expr: &Expr) -> &Expr {
    match &expr.kind {
        ExprKind::BracketSuffix { callee, .. } => callee,
        _ => expr,
    }
}

fn has_builtin_attribute(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        matches!(
            &attribute.kind,
            AttributeKind::Meta(meta) if meta.path == ["builtin"]
        )
    })
}

#[cfg(test)]
mod tests;
