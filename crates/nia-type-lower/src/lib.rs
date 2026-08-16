// SPDX-License-Identifier: GPL-3.0-or-later
//! Type syntax lowering, scoped generic context, and diagnostic recovery.
//!
//! The lowering pass records every type site, interns types in the caller's
//! canonical store, and preserves const-expression identities for later phases.

mod context;
mod core;
mod trait_objects;
mod traversal;

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use nia_ast::{
    ArrayLen, AssocBindingKey, Expr, ExprKind, FunctionItem, GenericParam, GenericParamKind, Item,
    ItemKind, Module, PathSegmentKind, TypeArg, TypeKind, TypePathSegment, TypeRef, WhereClause,
};
use nia_ast_walk::Visitor;
use nia_defs::DefCollection;
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{ConstExprId, GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNode, ItemTreeNodeKind, ModuleItemTree};
use nia_node_id::NodeSite;
use nia_span::Span;
use nia_symbol::{
    SymbolId, SymbolText, ToSymbolId, known, symbol_identity_key,
    symbol_text_from_optional_resolver,
};
use nia_ty::{
    ArrayLenTy, AssociatedTypeBindingTy, BuiltinTrait, ConstExprSummary, ConstGenericArg,
    ConstGenericValue, IntConst, LayoutBuiltin, PrimitiveTy, PrimitiveTypeSpelling, RangeTyKind,
    TraitId, TyKind, TypeStore, TypeStoreAppend,
};
use nia_type_resolve::{TypeNameResolution, TypeResolution};

#[derive(Debug, Clone, PartialEq)]
pub struct TypeLowering {
    pub type_uses: HashMap<NodeSite, InternedTyId>,
    pub const_exprs: HashMap<GlobalConstExprId, Expr>,
    pub const_expr_summaries: HashMap<GlobalConstExprId, ConstExprSummary>,
    pub diagnostics: Vec<Diagnostic>,
}

impl TypeLowering {
    pub fn explicit_type_roots(&self) -> Vec<InternedTyId> {
        let mut roots = self.type_uses.values().copied().collect::<Vec<_>>();
        roots.sort_unstable();
        roots.dedup();
        roots
    }

    pub fn ty_for_site(&self, site: &NodeSite) -> Option<InternedTyId> {
        self.type_uses.get(site).copied()
    }

    pub fn ty_for_key(&self, key: &nia_node_id::VersionedNodeKey) -> Option<InternedTyId> {
        self.ty_for_site(key.site())
    }

    pub fn versioned_type_uses_from_active_item_tree(
        &self,
        item_tree: &ActiveModuleItemTree,
    ) -> Vec<(nia_node_id::VersionedNodeKey, InternedTyId)> {
        let mut collector = VersionedTypeUseCollector {
            lowering: self,
            uses: Vec::new(),
        };
        for item in &item_tree.items {
            collector.visit_item_tree_node(item);
        }
        collector.uses
    }
}

struct VersionedTypeUseCollector<'a> {
    lowering: &'a TypeLowering,
    uses: Vec<(nia_node_id::VersionedNodeKey, InternedTyId)>,
}

impl VersionedTypeUseCollector<'_> {
    fn record_type(&mut self, ty: &TypeRef) {
        if let Some(lowered) = self.lowering.ty_for_key(&ty.node_key) {
            self.uses.push((ty.node_key.clone(), lowered));
        }
    }
}

impl<'ast> Visitor<'ast> for VersionedTypeUseCollector<'_> {
    fn visit_type(&mut self, ty: &'ast TypeRef) {
        self.record_type(ty);
        nia_ast_walk::walk_type(self, ty);
    }
}

impl VersionedTypeUseCollector<'_> {
    fn visit_item_tree_node(&mut self, item: &ItemTreeNode) {
        match &item.kind {
            ItemTreeNodeKind::Module(_) | ItemTreeNodeKind::Using(_) => {}
            ItemTreeNodeKind::Struct(item_struct) => {
                nia_ast_walk::walk_generic_params(self, &item_struct.generics);
                self.visit_where_clause(&item_struct.where_clause);
                for field in &item_struct.fields {
                    self.visit_type(&field.ty);
                }
            }
            ItemTreeNodeKind::Union(item_union) => {
                nia_ast_walk::walk_generic_params(self, &item_union.generics);
                self.visit_where_clause(&item_union.where_clause);
                for field in &item_union.fields {
                    self.visit_type(&field.ty);
                }
            }
            ItemTreeNodeKind::Trait(item_trait) => {
                nia_ast_walk::walk_generic_params(self, &item_trait.generics);
                for supertrait in &item_trait.supertraits {
                    self.visit_type(supertrait);
                }
                self.visit_where_clause(&item_trait.where_clause);
                for associated_value in &item_trait.associated_values {
                    self.visit_type(&associated_value.ty);
                }
                for method in &item_trait.methods {
                    self.visit_function(&method.function);
                }
            }
            ItemTreeNodeKind::Extend(extend) => {
                nia_ast_walk::walk_generic_params(self, &extend.generics);
                self.visit_type(&extend.target);
                if let Some(trait_ref) = &extend.trait_ref {
                    self.visit_type(trait_ref);
                }
                self.visit_where_clause(&extend.where_clause);
                for associated_type in &extend.associated_types {
                    self.visit_type(&associated_type.ty);
                }
                for associated_value in &extend.associated_values {
                    if let Some(ty) = &associated_value.binding.ty {
                        self.visit_type(ty);
                    }
                    if let Some(value) = &associated_value.binding.value {
                        self.visit_expr(value);
                    }
                }
                for method in &extend.methods {
                    self.visit_function(&method.function);
                }
            }
            ItemTreeNodeKind::Enum(item_enum) => {
                if let Some(backing_type) = &item_enum.backing_type {
                    self.visit_type(backing_type);
                }
                for variant in &item_enum.variants {
                    match &variant.payload {
                        nia_ast::EnumVariantPayload::Unit => {}
                        nia_ast::EnumVariantPayload::Tuple(fields) => {
                            for field in fields {
                                self.visit_type(field);
                            }
                        }
                        nia_ast::EnumVariantPayload::Named(fields) => {
                            for field in fields {
                                self.visit_type(&field.ty);
                            }
                        }
                    }
                    if let Some(value) = &variant.value {
                        self.visit_expr(value);
                    }
                }
            }
            ItemTreeNodeKind::TypeAlias(alias) => {
                nia_ast_walk::walk_generic_params(self, &alias.generics);
                self.visit_where_clause(&alias.where_clause);
                if let Some(ty) = &alias.ty {
                    self.visit_type(ty);
                }
            }
            ItemTreeNodeKind::Binding(binding) => {
                if let Some(ty) = &binding.ty {
                    self.visit_type(ty);
                }
                if let Some(value) = &binding.value {
                    self.visit_expr(value);
                }
            }
            ItemTreeNodeKind::Function(function) => self.visit_function(function),
        }
    }

    fn visit_where_clause(&mut self, clause: &WhereClause) {
        for predicate in &clause.predicates {
            self.visit_type(&predicate.ty);
            for bound in &predicate.bounds {
                self.visit_type(bound);
            }
        }
    }

    fn visit_function(&mut self, function: &FunctionItem) {
        nia_ast_walk::walk_generic_params(self, &function.generics);
        self.visit_where_clause(&function.where_clause);
        for param in &function.params {
            if let Some(ty) = &param.ty {
                self.visit_type(ty);
            }
        }
        if let Some(return_type) = &function.return_type {
            self.visit_type(return_type);
        }
        if let Some(body) = &function.body {
            self.visit_block(body);
        }
    }
}

#[derive(Clone, Copy)]
pub struct ProgramDefsContext<'a> {
    pub defs: Option<&'a dyn Fn(ModuleId) -> Option<Arc<DefCollection>>>,
}

impl<'a> ProgramDefsContext<'a> {
    pub fn empty() -> Self {
        Self { defs: None }
    }
}

impl std::fmt::Debug for ProgramDefsContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgramDefsContext")
            .field("defs", &self.defs.is_some())
            .finish()
    }
}

#[derive(Clone, Copy)]
pub struct TypeLoweringContext<'a> {
    pub type_store: &'a nia_ty::TypeStore,
    pub program_defs: ProgramDefsContext<'a>,
    pub symbols: Option<&'a dyn SymbolText>,
}

impl<'a> TypeLoweringContext<'a> {
    pub fn empty(type_store: &'a nia_ty::TypeStore) -> Self {
        Self {
            type_store,
            program_defs: ProgramDefsContext::empty(),
            symbols: None,
        }
    }

    pub fn from_program_defs(
        type_store: &'a nia_ty::TypeStore,
        program_defs: ProgramDefsContext<'a>,
    ) -> Self {
        Self {
            type_store,
            program_defs,
            symbols: None,
        }
    }

    pub fn with_symbols(mut self, symbols: &'a dyn SymbolText) -> Self {
        self.symbols = Some(symbols);
        self
    }
}

impl std::fmt::Debug for TypeLoweringContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypeLoweringContext")
            .field("type_store", &self.type_store.id())
            .field("program_defs", &self.program_defs)
            .field("symbols", &self.symbols.is_some())
            .finish()
    }
}

pub fn lower_module_types_with_context(
    module_id: ModuleId,
    module: &Module,
    resolved: &TypeResolution,
    context: TypeLoweringContext<'_>,
) -> TypeLowering {
    let item_tree = ModuleItemTree::from_module(module);
    lower_module_types_from_item_tree_with_context(module_id, &item_tree, resolved, context)
}

pub fn lower_module_types_from_active_item_tree_with_context(
    module_id: ModuleId,
    item_tree: &ActiveModuleItemTree,
    resolved: &TypeResolution,
    context: TypeLoweringContext<'_>,
) -> TypeLowering {
    lower_module_types_from_items(
        module_id,
        &item_tree.items,
        resolved,
        context,
        TypeLowerMode::All,
    )
}

pub fn lower_module_declaration_types_from_active_item_tree_with_context(
    module_id: ModuleId,
    item_tree: &ActiveModuleItemTree,
    resolved: &TypeResolution,
    context: TypeLoweringContext<'_>,
) -> TypeLowering {
    lower_module_types_from_items(
        module_id,
        &item_tree.items,
        resolved,
        context,
        TypeLowerMode::Declarations,
    )
}

pub fn lower_module_types_from_item_tree_with_context(
    module_id: ModuleId,
    item_tree: &ModuleItemTree,
    resolved: &TypeResolution,
    context: TypeLoweringContext<'_>,
) -> TypeLowering {
    lower_module_types_from_items(
        module_id,
        &item_tree.items,
        resolved,
        context,
        TypeLowerMode::All,
    )
}

fn lower_module_types_from_items(
    module_id: ModuleId,
    items: &[ItemTreeNode],
    resolved: &TypeResolution,
    context: TypeLoweringContext<'_>,
    mode: TypeLowerMode,
) -> TypeLowering {
    let mut lowerer = TypeLowerer {
        module_id,
        resolved,
        program_defs: context.program_defs,
        symbols: context.symbols,
        defs_cache: HashMap::new(),
        type_store: context.type_store,
        append: context.type_store.append_for_module(module_id),
        type_uses: HashMap::new(),
        const_exprs: HashMap::new(),
        const_expr_summaries: HashMap::new(),
        diagnostics: Vec::new(),
        generic_stack: Vec::new(),
        self_type_stack: Vec::new(),
        associated_type_scope_stack: Vec::new(),
        next_const_expr_id: 0,
        mode,
    };
    for item in items {
        lowerer.visit_item_tree_node(item);
    }
    TypeLowering {
        type_uses: lowerer.type_uses,
        const_exprs: lowerer.const_exprs,
        const_expr_summaries: lowerer.const_expr_summaries,
        diagnostics: lowerer.diagnostics,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeLowerMode {
    All,
    Declarations,
}

struct TypeLowerer<'a, 'store> {
    module_id: ModuleId,
    resolved: &'a TypeResolution,
    program_defs: ProgramDefsContext<'a>,
    symbols: Option<&'a dyn SymbolText>,
    defs_cache: HashMap<ModuleId, Option<Arc<DefCollection>>>,
    type_store: &'store TypeStore,
    append: TypeStoreAppend,
    type_uses: HashMap<NodeSite, InternedTyId>,
    const_exprs: HashMap<GlobalConstExprId, Expr>,
    const_expr_summaries: HashMap<GlobalConstExprId, ConstExprSummary>,
    diagnostics: Vec<Diagnostic>,
    generic_stack: Vec<Vec<GenericParam>>,
    self_type_stack: Vec<InternedTyId>,
    associated_type_scope_stack: Vec<AssociatedTypeScope>,
    next_const_expr_id: u32,
    mode: TypeLowerMode,
}

#[derive(Debug, Clone)]
struct AssociatedTypeScope {
    self_ty: InternedTyId,
    trait_id: TraitId,
    trait_args: Vec<InternedTyId>,
    trait_const_args: Vec<ConstGenericArg>,
    names: Vec<SymbolId>,
}

struct LoweredAssocBindingKey<'a> {
    name: &'a SymbolId,
    trait_id: Option<TraitId>,
    trait_args: Vec<InternedTyId>,
    trait_const_args: Vec<ConstGenericArg>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeContext {
    Value,
    Pointee,
    Return,
    Alias,
    SizeQuery,
    TraitBound,
    ExtendTarget,
}

#[derive(Debug, Clone, Default)]
struct TraitObjectArgs {
    // Trait object syntax accepts both positional trait arguments and
    // `Assoc = Ty` bindings in the same bracket list. Keeping them separated
    // here prevents later phases from depending on parser ordering details.
    trait_args: Vec<InternedTyId>,
    trait_const_args: Vec<ConstGenericArg>,
    associated_type_bindings: Vec<AssociatedTypeBindingTy>,
}

fn type_name_segment(segments: &[TypePathSegment]) -> Option<&TypePathSegment> {
    segments.last()
}

fn type_path_segment_name(segment: &TypePathSegment) -> Option<&SymbolId> {
    match &segment.kind {
        PathSegmentKind::Name(name) => Some(name),
        PathSegmentKind::Package | PathSegmentKind::Super | PathSegmentKind::SelfValue => None,
    }
}

fn builtin_trait_has_associated_type(trait_id: BuiltinTrait, name: &SymbolId) -> bool {
    trait_id
        .associated_types()
        .iter()
        .any(|associated_type| associated_type.symbol_id() == *name)
}

fn layout_builtin_for_symbol(name: SymbolId) -> Option<LayoutBuiltin> {
    match name {
        known::SIZE => Some(LayoutBuiltin::Size),
        known::ALIGN => Some(LayoutBuiltin::Align),
        _ => None,
    }
}

impl TypeLowerer<'_, '_> {
    fn lower_primitive_type(&mut self, primitive: PrimitiveTypeSpelling) -> InternedTyId {
        match primitive {
            PrimitiveTypeSpelling::Scalar(primitive) => {
                self.append.intern(TyKind::Primitive(primitive))
            }
            PrimitiveTypeSpelling::Vector { elem, lanes } => {
                self.append.intern(TyKind::Vector { elem, lanes })
            }
        }
    }
}

fn layout_builtin_array_len(expr: &Expr) -> Option<(LayoutBuiltin, &TypeRef)> {
    let ExprKind::Call { callee, args } = &expr.kind else {
        return layout_builtin_type_arg(expr);
    };
    if args.is_empty() {
        layout_builtin_type_arg(callee)
    } else {
        None
    }
}

fn layout_builtin_type_arg(expr: &Expr) -> Option<(LayoutBuiltin, &TypeRef)> {
    let ExprKind::BracketSuffix { callee, args } = &expr.kind else {
        return None;
    };
    let [arg] = args.as_slice() else {
        return None;
    };
    let type_arg = arg.ty.as_ref()?;
    let ExprKind::Qualified { lhs, name } = &callee.kind else {
        return None;
    };
    let ExprKind::Qualified {
        lhs: std_expr,
        name: builtin_segment,
    } = &lhs.kind
    else {
        return None;
    };
    let ExprKind::Ident(root) = &std_expr.kind else {
        return None;
    };
    if *root == known::std() && *builtin_segment == known::builtin() {
        layout_builtin_for_symbol(*name).map(|builtin| (builtin, type_arg))
    } else {
        None
    }
}

fn literal_array_len_expr_value(expr: &Expr) -> Option<u64> {
    let ExprKind::Integer(text) = &expr.kind else {
        return None;
    };
    let value = nia_literals::eval_int_literal(text).ok()?;
    u64::try_from(value).ok()
}

fn parse_integer_const_generic(text: &str) -> Option<IntConst> {
    nia_literals::eval_int_literal(text)
        .ok()
        .and_then(|value| u128::try_from(value).ok())
        .map(IntConst::unsigned)
}

fn expr_from_type_path(
    span: Span,
    node_key: nia_node_id::VersionedNodeKey,
    segments: &[TypePathSegment],
) -> Option<Expr> {
    let mut iter = segments.iter();
    let first = iter.next()?;
    let first_kind = match first.kind {
        PathSegmentKind::Name(name) => ExprKind::Ident(name),
        PathSegmentKind::Package | PathSegmentKind::Super | PathSegmentKind::SelfValue => {
            ExprKind::PathRoot(first.kind)
        }
    };
    let mut expr = Expr {
        span,
        node_key: node_key.clone(),
        kind: first_kind,
    };
    for segment in iter {
        let name = *type_path_segment_name(segment)?;
        expr = Expr {
            span,
            node_key: node_key.clone(),
            kind: ExprKind::Qualified {
                lhs: Box::new(expr),
                name,
            },
        };
    }
    Some(expr)
}

fn const_expr_summary(expr: &Expr) -> ConstExprSummary {
    ConstExprSummary {
        span: expr.span,
        literal_array_len: literal_array_len_expr_value(expr),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{collect_module_defs, collect_module_defs_from_active_item_tree};
    use nia_ids::ModuleIdAllocator;
    use nia_item_tree::ModuleItemTree;
    use nia_parser::{parse_module, parse_module_with_symbols};
    use nia_symbol_table::SymbolTable;
    use nia_type_resolve::{
        ProgramDefsContext as TypeResolveProgramDefsContext, resolve_module_types,
        resolve_module_types_from_active_item_tree,
    };
    use std::collections::HashMap;

    include!("tests/type_lower/test_support.rs");

    #[path = "type_lower/core_lowering.rs"]
    mod core_lowering;

    #[path = "type_lower/generic_diagnostics.rs"]
    mod generic_diagnostics;

    #[path = "type_lower/trait_objects.rs"]
    mod trait_objects;

    #[path = "type_lower/active_item_tree.rs"]
    mod active_item_tree;
}
