// SPDX-License-Identifier: GPL-3.0-or-later
use std::cell::RefCell;
use std::fmt;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
    sync::Arc,
};

mod aggregates;
mod bir;
mod calls;
mod expr;
mod extension_lookup;
mod filter;
mod helpers;
mod inputs;
mod literals;
mod orchestration;
mod patterns;
mod pipeline;
mod places;
mod products;
mod projection_obligations;
mod provider;
mod semantic_facts;
mod signature_scope;
mod statements;
mod static_init;
mod symbols;
mod trait_objects;
mod type_support;

use nia_ast::{
    Attribute, AttributeKind, BindingStmt, Block, Expr, ExprKind, FunctionItem, Module, Stmt,
    StmtKind,
};
use nia_ast_walk::Visitor;
use nia_body_ir::BodyIr;
use nia_const_check::{
    ConstArrayLengths, ConstKey, ConstTypedFacts, ConstValue, ConstValues, TypedConstValue,
};
use nia_const_ir::ResolvedConstModule;
use nia_defs::{
    DefCollection, DefId, DefKind, ExtensionMethod, ExtensionMethods, VisibleExtensionMethod,
    VisibleExtensionMethods,
};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{
    BuiltinTraitMethod, GlobalDefId, InternedTyId, LocalId, ModuleId, ReceiverKind, Visibility,
};
use nia_item_signatures::{
    ConstSignature, EnumSignature, FunctionSignature, GlobalSignature, ItemSignatures,
    ProgramConstSignature, ProgramEnumSignature, ProgramFunctionSignature, ProgramGlobalSignature,
    ProgramStructSignature, ProgramTraitImplIndex, ProgramTraitImplSignature,
    ProgramTraitSignature, ProgramTypeAliasSignature, ProgramUnionSignature, StructSignature,
    TraitImplSignature, TraitSignature, TypeAliasSignature, UnionSignature,
};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNode, ItemTreeNodeKind, ModuleItemTree};
use nia_layout::Layouts;
use nia_local_resolve::LocalResolution;
use nia_mangle::mangle_symbol_id;
use nia_node_id::{NodeOriginTable, VersionedNodeKey};
use nia_program_signatures::{ProgramSignatureContext, ProgramSignatureLookup};
use nia_sema_ir::{
    AssociatedConstProjection, BracketSuffixResolution, BuiltinValue, FunctionReference,
    FunctionSemanticFactsBuilder, GenericInstantiation, PointerArrayToSliceCoercion, ResolvedCall,
    SemanticFacts, SemanticFactsBuilder, SemanticTraitMethodRef, SemanticUseTable,
    SemanticValueUse, TraitObjectCoercion, TraitObjectUpcast,
};
use nia_source::{SourcePath, SourceVersion};
use nia_span::Span;
use nia_symbol::{SymbolId, SymbolMap, ToSymbolId, known};
use nia_symbol_table::SymbolTable;
use nia_target_config::TargetConfig;
use nia_ty::{ConstGenericArg, PrimitiveTy, TyKind, TypeStoreAppend};
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_value_resolve::ValueResolution;

use crate::projection_obligations::TraitObligation;
use extension_lookup::{BodyVisibleExtensionSource, ExtensionMethodLookup};
use filter::ActiveBodyCheckFilter;
use signature_scope::ProgramSignatureScope;

pub use inputs::{
    BodyCheckFilter, BodyCheckInput, BodyCheckSeed, BodyCheckWithProgramSignaturesInput, BodyConst,
    BodyLocalSignatures, BodyProgramContext, BodyVisibleExtensions, FunctionCheckScope,
    ProgramConstMaps,
};
pub use orchestration::{
    check_module_bodies, check_module_bodies_with_layouts,
    check_module_bodies_with_program_signatures,
    check_module_bodies_with_program_signatures_and_layouts,
    check_module_bodies_with_program_signatures_and_layouts_with_timings,
};
use orchestration::{time_body_stage, time_body_stage_if_slow};
pub use products::{BodyCheck, BodyCheckProduct, PrecheckedBodyCheck};
pub use provider::{
    ProviderDemand, ProviderFactRevision, ProviderFactRevisionTransition, ProviderRequest,
};

struct BodyTypeCx<'a> {
    store: &'a nia_ty::TypeStore,
    append: TypeStoreAppend,
}

impl<'a> BodyTypeCx<'a> {
    fn new(store: &'a nia_ty::TypeStore, module_id: ModuleId) -> Self {
        Self {
            store,
            append: store.append_for_module(module_id),
        }
    }

    fn get(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.store.get(ty)
    }

    fn intern(&self, kind: TyKind) -> InternedTyId {
        self.append.intern(kind)
    }

    fn primitive(&self, primitive: PrimitiveTy) -> InternedTyId {
        self.intern(TyKind::Primitive(primitive))
    }

    fn error(&self) -> InternedTyId {
        self.intern(TyKind::Error)
    }

    fn store_id(&self) -> nia_ids::TypeStoreId {
        self.store.id()
    }
}

struct BodyChecker<'a> {
    type_store: &'a nia_ty::TypeStore,
    active_item_tree: &'a ActiveModuleItemTree,
    defs: &'a DefCollection,
    program: BodyProgramContext<'a>,
    values: &'a ValueResolution,
    locals: &'a LocalResolution,
    semantic_uses: &'a SemanticUseTable,
    interner: BodyTypeCx<'a>,
    type_lowering: &'a TypeLowering,
    signatures: BodyLocalSignatures<'a>,
    const_signatures: &'a ItemSignatures,
    normalization: &'a TypeNormalization,
    target: &'a TargetConfig,
    const_eval: BodyConst<'a>,
    const_module: &'a ResolvedConstModule,
    layouts: &'a Layouts,
    extensions: BodyVisibleExtensionSource<'a>,
    program_extension_methods: &'a ExtensionMethods,
    program_signature_scope: ProgramSignatureScope<'a>,
    program_trait_impls: &'a [ProgramTraitImplSignature],
    program_trait_impl_index: Option<&'a ProgramTraitImplIndex>,
    program_const_values: &'a dyn Fn(ModuleId) -> Option<Arc<ConstValues>>,
    program_const_array_lengths: &'a dyn Fn(ModuleId) -> Option<Arc<ConstArrayLengths>>,
    program_const_module: &'a dyn Fn(ModuleId) -> Option<Arc<ResolvedConstModule>>,
    source_path: &'a SourcePath,
    symbols: &'a SymbolTable,
    extension_methods_by_id: Arc<HashMap<GlobalDefId, ExtensionMethodLookup>>,
    extension_method_lookup_cache: HashMap<GlobalDefId, ExtensionMethodLookup>,
    callable_extension_methods_by_name: SymbolMap<CallableExtensionMethods>,
    provider_demands: Rc<RefCell<HashSet<ProviderDemand>>>,
    provider_demands_by_function: Rc<RefCell<HashMap<GlobalDefId, HashSet<ProviderDemand>>>>,
    node_expr_types: HashMap<VersionedNodeKey, InternedTyId>,
    node_bracket_suffix_resolutions: HashMap<VersionedNodeKey, BracketSuffixResolution>,
    node_pointer_array_to_slice_coercions: HashMap<VersionedNodeKey, PointerArrayToSliceCoercion>,
    node_trait_object_coercions: HashMap<VersionedNodeKey, TraitObjectCoercion>,
    node_trait_object_upcasts: HashMap<VersionedNodeKey, TraitObjectUpcast>,
    node_builtin_values: HashMap<VersionedNodeKey, BuiltinValue>,
    node_associated_const_projections: HashMap<VersionedNodeKey, AssociatedConstProjection>,
    node_array_repeat_counts: HashMap<VersionedNodeKey, u64>,
    node_switch_pattern_values: HashMap<VersionedNodeKey, i128>,
    node_resolved_calls: HashMap<VersionedNodeKey, ResolvedCall>,
    node_function_references: HashMap<VersionedNodeKey, FunctionReference>,
    generic_instantiations: Vec<GenericInstantiation>,
    function_facts: HashMap<GlobalDefId, FunctionSemanticFactsBuilder>,
    function_bodies: HashMap<GlobalDefId, Arc<nia_body_ir::TypedBody>>,
    global_inits: HashMap<GlobalDefId, Arc<nia_static_ir::StaticInit>>,
    static_init_refs: HashMap<GlobalDefId, nia_static_ir::StaticInitRefs>,
    local_types: HashMap<LocalId, InternedTyId>,
    global_types: HashMap<DefId, InternedTyId>,
    const_types: HashMap<DefId, InternedTyId>,
    method_receiver_kinds: HashMap<GlobalDefId, Option<ReceiverKind>>,
    traits_by_method_name: SymbolMap<Vec<GlobalDefId>>,
    trait_impls_by_trait: HashMap<nia_ty::TraitId, Vec<usize>>,
    def_trait_obligations_cache: HashMap<DefId, Vec<TraitObligation>>,
    trait_obligation_resolution_cache:
        HashMap<TraitObligationResolutionKey, nia_trait_solve::TraitResolution>,
    type_match_cache: HashMap<(InternedTyId, InternedTyId), bool>,
    diagnostics: Vec<Diagnostic>,
    diagnostic_owners: Vec<Option<GlobalDefId>>,
    timing: bool,
    timing_module_id: ModuleId,
    current_return: InternedTyId,
    current_def_id: Option<GlobalDefId>,
    current_param_locals: Vec<LocalId>,
    const_context_depth: usize,
    const_call_locals: Vec<ConstCallFrame>,
    body_filter: ActiveBodyCheckFilter<'a>,
    product: BodyCheckProduct,
    checked_functions: HashSet<GlobalDefId>,
    pending_functions: VecDeque<GlobalDefId>,
    profile: nia_timing::TimingAccumulator,
}

struct CheckedStaticInitVisitor<'checker, 'context> {
    checker: &'checker mut BodyChecker<'context>,
}

impl<'ast> Visitor<'ast> for CheckedStaticInitVisitor<'_, '_> {
    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        if let StmtKind::Static(binding) = &stmt.kind {
            self.checker.lower_checked_static_init(stmt.span, binding);
        }
        nia_ast_walk::walk_stmt(self, stmt);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TraitObligationResolutionKey {
    current_def_id: Option<GlobalDefId>,
    self_ty: InternedTyId,
    trait_id: nia_ty::TraitId,
    trait_args: Vec<InternedTyId>,
    trait_const_args: Vec<ConstGenericArg>,
}

#[derive(Debug, Clone, Default)]
struct ConstCallFrame {
    module_id: Option<ModuleId>,
    function_id: Option<GlobalDefId>,
    locals: HashMap<LocalId, nia_const_check::ConstValue>,
    local_types: HashMap<LocalId, nia_const_check::ConstValueType>,
    mutable_locals: HashSet<LocalId>,
    type_substitutions: SymbolMap<InternedTyId>,
    const_substitutions: SymbolMap<ConstGenericArg>,
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

pub(crate) fn generic_inst_base(expr: &Expr) -> &Expr {
    match &expr.kind {
        ExprKind::BracketSuffix { callee, .. } => callee,
        _ => expr,
    }
}

#[cfg(test)]
mod tests;
