// SPDX-License-Identifier: GPL-3.0-or-later
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
                self.visit_where_clause(&item_struct.where_clause);
                for field in &item_struct.fields {
                    self.visit_type(&field.ty);
                }
            }
            ItemTreeNodeKind::Union(item_union) => {
                self.visit_where_clause(&item_union.where_clause);
                for field in &item_union.fields {
                    self.visit_type(&field.ty);
                }
            }
            ItemTreeNodeKind::Trait(item_trait) => {
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

impl<'ast> Visitor<'ast> for TypeLowerer<'_, '_> {
    fn visit_item(&mut self, item: &'ast Item) {
        match &item.kind {
            ItemKind::Struct(item_struct) => {
                self.with_generics(&item_struct.generics, |lowerer| {
                    lowerer.lower_where_clause(&item_struct.where_clause);
                    for field in &item_struct.fields {
                        lowerer.lower_type_in_context(&field.ty, TypeContext::Value);
                    }
                });
            }
            ItemKind::Union(item_union) => {
                self.with_generics(&item_union.generics, |lowerer| {
                    lowerer.lower_where_clause(&item_union.where_clause);
                    for field in &item_union.fields {
                        lowerer.lower_type_in_context(&field.ty, TypeContext::Value);
                    }
                });
            }
            ItemKind::Trait(item_trait) => {
                self.with_generics(&item_trait.generics, |lowerer| {
                    let self_ty = lowerer.append.intern(TyKind::SelfParam);
                    lowerer.with_self_type(self_ty, |lowerer| {
                        if let Some(trait_id) = lowerer.local_trait_id(&item.node_key) {
                            let trait_args = item_trait
                                .generics
                                .iter()
                                .filter(|generic| matches!(generic.kind, GenericParamKind::Type))
                                .map(|generic| {
                                    lowerer.append.intern(TyKind::GenericParam(generic.name))
                                })
                                .collect::<Vec<_>>();
                            let trait_const_args = item_trait
                                .generics
                                .iter()
                                .filter_map(|generic| match generic.kind {
                                    GenericParamKind::Type => None,
                                    GenericParamKind::Const { ref ty } => {
                                        let ty =
                                            lowerer.lower_type_in_context(ty, TypeContext::Value);
                                        Some(ConstGenericArg {
                                            ty,
                                            value: ConstGenericValue::GenericParam(generic.name),
                                        })
                                    }
                                })
                                .collect::<Vec<_>>();
                            let associated_types = item_trait
                                .associated_types
                                .iter()
                                .map(|associated_type| associated_type.name)
                                .collect::<Vec<_>>();
                            lowerer.with_associated_type_scope(
                                AssociatedTypeScope {
                                    self_ty,
                                    trait_id: TraitId::Source(trait_id),
                                    trait_args,
                                    trait_const_args,
                                    names: associated_types,
                                },
                                |lowerer| {
                                    for supertrait in &item_trait.supertraits {
                                        lowerer.lower_type_in_context(
                                            supertrait,
                                            TypeContext::TraitBound,
                                        );
                                    }
                                    lowerer.lower_where_clause(&item_trait.where_clause);
                                    for associated_value in &item_trait.associated_values {
                                        lowerer.lower_type_in_context(
                                            &associated_value.ty,
                                            TypeContext::Value,
                                        );
                                    }
                                    for method in &item_trait.methods {
                                        lowerer.visit_function(&method.function);
                                    }
                                },
                            );
                        } else {
                            for supertrait in &item_trait.supertraits {
                                lowerer.lower_type_in_context(supertrait, TypeContext::TraitBound);
                            }
                            lowerer.lower_where_clause(&item_trait.where_clause);
                            for associated_value in &item_trait.associated_values {
                                lowerer.lower_type_in_context(
                                    &associated_value.ty,
                                    TypeContext::Value,
                                );
                            }
                            for method in &item_trait.methods {
                                lowerer.visit_function(&method.function);
                            }
                        }
                    });
                });
            }
            ItemKind::Extend(extend) => {
                self.with_generics(&extend.generics, |lowerer| {
                    let self_ty =
                        lowerer.lower_type_in_context(&extend.target, TypeContext::ExtendTarget);
                    let trait_scope = extend.trait_ref.as_ref().and_then(|trait_ref| {
                        let trait_ty =
                            lowerer.lower_type_in_context(trait_ref, TypeContext::TraitBound);
                        lowerer.associated_type_scope_for_trait_impl(self_ty, trait_ty)
                    });
                    lowerer.with_self_type(self_ty, |lowerer| {
                        lowerer.lower_where_clause(&extend.where_clause);
                        for associated_type in &extend.associated_types {
                            lowerer.lower_type_in_context(&associated_type.ty, TypeContext::Value);
                        }
                        for associated_value in &extend.associated_values {
                            if let Some(ty) = &associated_value.binding.ty {
                                lowerer.lower_type_in_context(ty, TypeContext::Value);
                            }
                            if lowerer.mode == TypeLowerMode::All
                                && let Some(value) = &associated_value.binding.value
                            {
                                lowerer.visit_expr(value);
                            }
                        }
                        if let Some(trait_scope) = trait_scope {
                            lowerer.with_associated_type_scope(trait_scope, |lowerer| {
                                for method in &extend.methods {
                                    lowerer.visit_function(&method.function);
                                }
                            });
                        } else {
                            for method in &extend.methods {
                                lowerer.visit_function(&method.function);
                            }
                        }
                    });
                });
            }
            ItemKind::Enum(item_enum) => {
                if let Some(backing_type) = &item_enum.backing_type {
                    let ty = self.lower_type_in_context(backing_type, TypeContext::Value);
                    if !self.is_integer(ty) {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            backing_type.span,
                            "enum backing type must be an integer type",
                        ));
                    }
                }
                for variant in &item_enum.variants {
                    match &variant.payload {
                        nia_ast::EnumVariantPayload::Unit => {}
                        nia_ast::EnumVariantPayload::Tuple(fields) => {
                            for field in fields {
                                self.lower_type_in_context(field, TypeContext::Value);
                            }
                        }
                        nia_ast::EnumVariantPayload::Named(fields) => {
                            for field in fields {
                                self.lower_type_in_context(&field.ty, TypeContext::Value);
                            }
                        }
                    }
                }
            }
            ItemKind::TypeAlias(alias) => {
                self.with_generics(&alias.generics, |lowerer| {
                    lowerer.lower_where_clause(&alias.where_clause);
                    if let Some(ty) = &alias.ty {
                        lowerer.lower_type_in_context(ty, TypeContext::Alias);
                    }
                });
            }
            ItemKind::Binding(binding) => {
                if let Some(ty) = &binding.ty {
                    self.lower_type_in_context(ty, TypeContext::Value);
                }
                if self.mode == TypeLowerMode::All
                    && let Some(value) = &binding.value
                {
                    nia_ast_walk::walk_expr(self, value);
                }
            }
            ItemKind::Function(function) => self.visit_function(function),
            ItemKind::Module(_) | ItemKind::Using(_) => {}
        }
    }

    fn visit_function(&mut self, function: &'ast FunctionItem) {
        self.with_generics(&function.generics, |lowerer| {
            lowerer.lower_where_clause(&function.where_clause);
            for param in &function.params {
                if let Some(ty) = &param.ty {
                    lowerer.lower_type_in_context(ty, TypeContext::Value);
                }
            }
            if let Some(return_type) = &function.return_type {
                lowerer.lower_type_in_context(return_type, TypeContext::Return);
            }
            if lowerer.mode == TypeLowerMode::All
                && let Some(body) = &function.body
            {
                lowerer.visit_block(body);
            }
        });
    }

    fn visit_type(&mut self, ty: &'ast TypeRef) {
        self.lower_type_in_context(ty, TypeContext::Value);
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        match &expr.kind {
            ExprKind::BracketSuffix { callee, args } => {
                self.visit_expr(callee);
                for arg in args {
                    if let Some(ty) = &arg.ty {
                        self.lower_type_in_context(ty, TypeContext::Value);
                    } else if let Some(expr) = &arg.expr {
                        self.visit_expr(expr);
                    }
                }
            }
            ExprKind::TypeTarget { ty } => {
                self.visit_type(ty);
            }
            ExprKind::TraitTarget { ty, trait_ref } => {
                self.lower_type_in_context(ty, TypeContext::Value);
                self.lower_type_in_context(trait_ref, TypeContext::TraitBound);
            }
            ExprKind::TypedArrayLiteral { ty, elems } => {
                self.visit_type(ty);
                match elems {
                    nia_ast::ArrayElements::List(elems) => {
                        for elem in elems {
                            self.visit_expr(elem);
                        }
                    }
                    nia_ast::ArrayElements::Repeat { value, count } => {
                        self.visit_expr(value);
                        self.visit_expr(count);
                    }
                }
            }
            ExprKind::TypedStructLiteral { ty, fields } => {
                self.visit_type(ty);
                for field in fields {
                    self.visit_expr(&field.value);
                }
            }
            _ => nia_ast_walk::walk_expr(self, expr),
        }
    }
}

impl TypeLowerer<'_, '_> {
    fn visit_item_tree_node(&mut self, item: &ItemTreeNode) {
        match &item.kind {
            ItemTreeNodeKind::Struct(item_struct) => {
                self.with_generics(&item_struct.generics, |lowerer| {
                    lowerer.lower_where_clause(&item_struct.where_clause);
                    for field in &item_struct.fields {
                        lowerer.lower_type_in_context(&field.ty, TypeContext::Value);
                    }
                });
            }
            ItemTreeNodeKind::Union(item_union) => {
                self.with_generics(&item_union.generics, |lowerer| {
                    lowerer.lower_where_clause(&item_union.where_clause);
                    for field in &item_union.fields {
                        lowerer.lower_type_in_context(&field.ty, TypeContext::Value);
                    }
                });
            }
            ItemTreeNodeKind::Trait(item_trait) => {
                self.with_generics(&item_trait.generics, |lowerer| {
                    let self_ty = lowerer.append.intern(TyKind::SelfParam);
                    lowerer.with_self_type(self_ty, |lowerer| {
                        if let Some(trait_id) = lowerer.local_trait_id(&item.node_key) {
                            let trait_args = item_trait
                                .generics
                                .iter()
                                .filter(|generic| matches!(generic.kind, GenericParamKind::Type))
                                .map(|generic| {
                                    lowerer.append.intern(TyKind::GenericParam(generic.name))
                                })
                                .collect::<Vec<_>>();
                            let trait_const_args = item_trait
                                .generics
                                .iter()
                                .filter_map(|generic| match generic.kind {
                                    GenericParamKind::Type => None,
                                    GenericParamKind::Const { ref ty } => {
                                        let ty =
                                            lowerer.lower_type_in_context(ty, TypeContext::Value);
                                        Some(ConstGenericArg {
                                            ty,
                                            value: ConstGenericValue::GenericParam(generic.name),
                                        })
                                    }
                                })
                                .collect::<Vec<_>>();
                            let associated_types = item_trait
                                .associated_types
                                .iter()
                                .map(|associated_type| associated_type.name)
                                .collect::<Vec<_>>();
                            lowerer.with_associated_type_scope(
                                AssociatedTypeScope {
                                    self_ty,
                                    trait_id: TraitId::Source(trait_id),
                                    trait_args,
                                    trait_const_args,
                                    names: associated_types,
                                },
                                |lowerer| {
                                    for supertrait in &item_trait.supertraits {
                                        lowerer.lower_type_in_context(
                                            supertrait,
                                            TypeContext::TraitBound,
                                        );
                                    }
                                    lowerer.lower_where_clause(&item_trait.where_clause);
                                    for associated_value in &item_trait.associated_values {
                                        lowerer.lower_type_in_context(
                                            &associated_value.ty,
                                            TypeContext::Value,
                                        );
                                    }
                                    for method in &item_trait.methods {
                                        lowerer.visit_function(&method.function);
                                    }
                                },
                            );
                        } else {
                            for supertrait in &item_trait.supertraits {
                                lowerer.lower_type_in_context(supertrait, TypeContext::TraitBound);
                            }
                            lowerer.lower_where_clause(&item_trait.where_clause);
                            for associated_value in &item_trait.associated_values {
                                lowerer.lower_type_in_context(
                                    &associated_value.ty,
                                    TypeContext::Value,
                                );
                            }
                            for method in &item_trait.methods {
                                lowerer.visit_function(&method.function);
                            }
                        }
                    });
                });
            }
            ItemTreeNodeKind::Extend(extend) => {
                self.with_generics(&extend.generics, |lowerer| {
                    let self_ty =
                        lowerer.lower_type_in_context(&extend.target, TypeContext::ExtendTarget);
                    let trait_scope = extend.trait_ref.as_ref().and_then(|trait_ref| {
                        let trait_ty =
                            lowerer.lower_type_in_context(trait_ref, TypeContext::TraitBound);
                        lowerer.associated_type_scope_for_trait_impl(self_ty, trait_ty)
                    });
                    lowerer.with_self_type(self_ty, |lowerer| {
                        lowerer.lower_where_clause(&extend.where_clause);
                        for associated_type in &extend.associated_types {
                            lowerer.lower_type_in_context(&associated_type.ty, TypeContext::Value);
                        }
                        for associated_value in &extend.associated_values {
                            if let Some(ty) = &associated_value.binding.ty {
                                lowerer.lower_type_in_context(ty, TypeContext::Value);
                            }
                            if lowerer.mode == TypeLowerMode::All
                                && let Some(value) = &associated_value.binding.value
                            {
                                lowerer.visit_expr(value);
                            }
                        }
                        if let Some(trait_scope) = trait_scope {
                            lowerer.with_associated_type_scope(trait_scope, |lowerer| {
                                for method in &extend.methods {
                                    lowerer.visit_function(&method.function);
                                }
                            });
                        } else {
                            for method in &extend.methods {
                                lowerer.visit_function(&method.function);
                            }
                        }
                    });
                });
            }
            ItemTreeNodeKind::Enum(item_enum) => {
                if let Some(backing_type) = &item_enum.backing_type {
                    let ty = self.lower_type_in_context(backing_type, TypeContext::Value);
                    if !self.is_integer(ty) {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            backing_type.span,
                            "enum backing type must be an integer type",
                        ));
                    }
                }
                for variant in &item_enum.variants {
                    match &variant.payload {
                        nia_ast::EnumVariantPayload::Unit => {}
                        nia_ast::EnumVariantPayload::Tuple(fields) => {
                            for field in fields {
                                self.lower_type_in_context(field, TypeContext::Value);
                            }
                        }
                        nia_ast::EnumVariantPayload::Named(fields) => {
                            for field in fields {
                                self.lower_type_in_context(&field.ty, TypeContext::Value);
                            }
                        }
                    }
                }
            }
            ItemTreeNodeKind::TypeAlias(alias) => {
                self.with_generics(&alias.generics, |lowerer| {
                    lowerer.lower_where_clause(&alias.where_clause);
                    if let Some(ty) = &alias.ty {
                        lowerer.lower_type_in_context(ty, TypeContext::Alias);
                    }
                });
            }
            ItemTreeNodeKind::Binding(binding) => {
                if let Some(ty) = &binding.ty {
                    self.lower_type_in_context(ty, TypeContext::Value);
                }
                if self.mode == TypeLowerMode::All
                    && let Some(value) = &binding.value
                {
                    nia_ast_walk::walk_expr(self, value);
                }
            }
            ItemTreeNodeKind::Function(function) => self.visit_function(function),
            ItemTreeNodeKind::Module(_) | ItemTreeNodeKind::Using(_) => {}
        }
    }
}

impl<'a> TypeLowerer<'a, '_> {
    fn lower_type_in_context(&mut self, ty: &TypeRef, context: TypeContext) -> InternedTyId {
        let lowered = self.lower_type(ty, context);
        self.type_uses.insert(ty.node_key.site().clone(), lowered);
        if context == TypeContext::Value
            && let Some(message) = self.invalid_value_type_message(lowered)
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                ty.span,
                message,
            ));
        }
        lowered
    }

    fn lower_type(&mut self, ty: &TypeRef, context: TypeContext) -> InternedTyId {
        match &ty.kind {
            TypeKind::Error => self.append.intern(TyKind::Error),
            TypeKind::Infer => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_NORMALIZATION,
                    ty.span,
                    "`_` type inference is not valid in this type lowering context",
                ));
                self.append.intern(TyKind::Error)
            }
            TypeKind::Void => self.append.intern(TyKind::Primitive(PrimitiveTy::Void)),
            TypeKind::Never => self.append.intern(TyKind::Primitive(PrimitiveTy::Never)),
            TypeKind::Optional { elem } => {
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.append.intern(TyKind::Optional { elem })
            }
            TypeKind::ErrorUnion { error, value } => {
                let error = self.lower_type_in_context(error, TypeContext::Value);
                let value = self.lower_type_in_context(value, TypeContext::Value);
                self.append.intern(TyKind::ErrorUnion { error, value })
            }
            TypeKind::SelfType => self.self_type_stack.last().copied().unwrap_or_else(|| {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_NORMALIZATION,
                    ty.span,
                    "`Self` is only valid in traits and extend blocks",
                ));
                self.append.intern(TyKind::Error)
            }),
            TypeKind::Pointer { is_readonly, elem } => {
                if let Some(trait_object) = self.lower_trait_object_type(*is_readonly, elem) {
                    trait_object
                } else {
                    let elem = self.lower_type_in_context(elem, TypeContext::Value);
                    self.append.intern(TyKind::Pointer {
                        is_readonly: *is_readonly,
                        elem,
                    })
                }
            }
            TypeKind::VolatilePointer { is_readonly, elem } => {
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.append.intern(TyKind::VolatilePointer {
                    is_readonly: *is_readonly,
                    elem,
                })
            }
            TypeKind::Slice { is_readonly, elem } => {
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.append.intern(TyKind::Slice {
                    is_readonly: *is_readonly,
                    elem,
                })
            }
            TypeKind::SlicePointee { elem } => {
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.append.intern(TyKind::SlicePointee { elem })
            }
            TypeKind::Array { len, elem } => {
                let len = self.lower_array_len(len);
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.append.intern(TyKind::Array { len, elem })
            }
            TypeKind::Range {
                start,
                end,
                inclusive,
            } => self.lower_range_type(ty.span, start.as_deref(), end.as_deref(), *inclusive),
            TypeKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            } => {
                let params = params
                    .iter()
                    .map(|param| self.lower_type_in_context(param, TypeContext::Value))
                    .collect();
                let return_type = match return_type {
                    Some(return_type) => {
                        self.lower_type_in_context(return_type, TypeContext::Return)
                    }
                    None => self.append.intern(TyKind::Primitive(PrimitiveTy::Void)),
                };
                self.append.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic: *is_variadic,
                })
            }
            TypeKind::Path { segments } => {
                let Some(first) = segments.first() else {
                    return self.append.intern(TyKind::Error);
                };
                let Some(type_segment) = type_name_segment(segments) else {
                    return self.append.intern(TyKind::Error);
                };
                match self
                    .resolved
                    .node_type_names
                    .get(ty.node_key.site())
                    .copied()
                {
                    Some(TypeNameResolution::Primitive(primitive)) => {
                        self.lower_primitive_type(primitive)
                    }
                    Some(TypeNameResolution::BuiltinTrait(trait_id)) => self
                        .lower_builtin_trait_or_extend_target_type(
                            ty.span,
                            type_segment,
                            trait_id,
                            context,
                        ),
                    Some(TypeNameResolution::GenericParam) => {
                        let Some(name) = type_path_segment_name(first) else {
                            return self.append.intern(TyKind::Error);
                        };
                        self.append.intern(TyKind::GenericParam(*name))
                    }
                    Some(TypeNameResolution::AssociatedType) => {
                        let Some(name) = type_path_segment_name(first) else {
                            return self.append.intern(TyKind::Error);
                        };
                        self.lower_scoped_associated_type(ty.span, name, type_segment)
                    }
                    Some(TypeNameResolution::Def(def_id)) => {
                        let def_id = self
                            .resolved
                            .node_qualified_type_names
                            .get(ty.node_key.site())
                            .copied()
                            .unwrap_or(GlobalDefId {
                                module_id: self.module_id,
                                def_id,
                            });
                        self.lower_path_type(ty.span, type_segment, def_id, context)
                    }
                    Some(TypeNameResolution::External(global_id)) => {
                        self.lower_path_type(ty.span, type_segment, global_id, context)
                    }
                    Some(TypeNameResolution::Error) | None => self.append.intern(TyKind::Error),
                }
            }
            TypeKind::Projection {
                ty,
                trait_ref,
                name,
            } => {
                let self_ty = self.lower_type_in_context(ty, TypeContext::Value);
                let trait_ty = self.lower_type_in_context(trait_ref, TypeContext::TraitBound);
                let trait_ty = self.normalize_if_known(trait_ty);
                let Some((trait_id, args, const_args)) = self.projection_trait_id(trait_ty) else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_NORMALIZATION,
                        trait_ref.span,
                        "projection trait must resolve to a trait",
                    ));
                    return self.append.intern(TyKind::Error);
                };
                if !self.trait_id_has_associated_type(trait_id, name) {
                    let name = self.symbol_name(*name);
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_NORMALIZATION,
                        ty.span,
                        format!("trait does not define associated type `{name}`"),
                    ));
                    return self.append.intern(TyKind::Error);
                }
                self.append.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args: args,
                    trait_const_args: const_args,
                    name: *name,
                })
            }
        }
    }

    fn lower_range_type(
        &mut self,
        span: Span,
        start: Option<&TypeRef>,
        end: Option<&TypeRef>,
        inclusive: bool,
    ) -> InternedTyId {
        let start_ty = start.map(|ty| self.lower_type_in_context(ty, TypeContext::Value));
        let end_ty = end.map(|ty| self.lower_type_in_context(ty, TypeContext::Value));
        let kind = match (start_ty, end_ty, inclusive) {
            (Some(_), Some(_), false) => RangeTyKind::Exclusive,
            (Some(_), Some(_), true) => RangeTyKind::Inclusive,
            (Some(_), None, false) => RangeTyKind::From,
            (None, Some(_), false) => RangeTyKind::To,
            (None, Some(_), true) => RangeTyKind::ToInclusive,
            (None, None, false) => RangeTyKind::Full,
            (Some(_), None, true) | (None, None, true) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_NORMALIZATION,
                    span,
                    "inclusive range type requires an end bound",
                ));
                return self.append.intern(TyKind::Error);
            }
        };
        let bound = match (start_ty, end_ty) {
            (Some(start_ty), Some(end_ty)) => {
                if !self.types_equivalent(start_ty, end_ty) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_NORMALIZATION,
                        span,
                        "range type bounds must have the same type",
                    ));
                    return self.append.intern(TyKind::Error);
                }
                Some(start_ty)
            }
            (Some(bound), None) | (None, Some(bound)) => Some(bound),
            (None, None) => None,
        };
        if let Some(bound) = bound
            && !self.can_be_integer(bound)
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                span,
                "range bound type must be an integer type",
            ));
            return self.append.intern(TyKind::Error);
        }
        self.append.intern(TyKind::Range { kind, bound })
    }

    fn normalize_if_known(&self, ty: InternedTyId) -> InternedTyId {
        ty
    }

    fn lower_path_type(
        &mut self,
        span: Span,
        segment: &TypePathSegment,
        def_id: GlobalDefId,
        context: TypeContext,
    ) -> InternedTyId {
        let mut args = Vec::new();
        let mut const_args = Vec::new();
        let mut seen_assoc_bindings = HashSet::new();
        let mut seen_assoc_binding = false;
        let generic_params = self.generic_params_for_def(def_id).unwrap_or_default();
        let mut positional_index = 0usize;
        for arg in &segment.args {
            match arg {
                TypeArg::Type(arg_ty) => {
                    if seen_assoc_binding {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            arg_ty.span,
                            "positional type arguments must precede associated type bindings",
                        ));
                    }
                    match generic_params
                        .get(positional_index)
                        .map(|generic| &generic.kind)
                    {
                        Some(GenericParamKind::Const { ty }) => {
                            let Some(value) = self.lower_const_generic_value_from_type_ref(arg_ty)
                            else {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::TYPE_NORMALIZATION,
                                    arg_ty.span,
                                    "expected const generic argument",
                                ));
                                positional_index += 1;
                                continue;
                            };
                            let ty = self.lower_type_in_context(ty, TypeContext::Value);
                            const_args.push(ConstGenericArg { ty, value });
                        }
                        _ => args.push(self.lower_type_or_const_type_arg(arg_ty)),
                    }
                    positional_index += 1;
                }
                TypeArg::Const(expr) => {
                    match generic_params
                        .get(positional_index)
                        .map(|generic| &generic.kind)
                    {
                        Some(GenericParamKind::Const { ty }) => {
                            let Some(value) = self.lower_const_generic_value_from_expr(expr) else {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::TYPE_NORMALIZATION,
                                    expr.span,
                                    "unsupported const generic argument",
                                ));
                                positional_index += 1;
                                continue;
                            };
                            let ty = self.lower_type_in_context(ty, TypeContext::Value);
                            const_args.push(ConstGenericArg { ty, value });
                        }
                        _ => {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::TYPE_NORMALIZATION,
                                expr.span,
                                "const value generic argument supplied for type parameter",
                            ));
                        }
                    }
                    positional_index += 1;
                }
                TypeArg::TypeOrConst { ty: arg_ty, expr } => {
                    if seen_assoc_binding {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            arg_ty.span,
                            "positional type arguments must precede associated type bindings",
                        ));
                    }
                    match generic_params
                        .get(positional_index)
                        .map(|generic| &generic.kind)
                    {
                        Some(GenericParamKind::Const { ty }) => {
                            let Some(value) = self.lower_const_generic_value_from_expr(expr) else {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::TYPE_NORMALIZATION,
                                    expr.span,
                                    "unsupported const generic argument",
                                ));
                                positional_index += 1;
                                continue;
                            };
                            let ty = self.lower_type_in_context(ty, TypeContext::Value);
                            const_args.push(ConstGenericArg { ty, value });
                        }
                        _ => args.push(self.lower_type_in_context(arg_ty, TypeContext::Value)),
                    }
                    positional_index += 1;
                }
                TypeArg::AssocBinding {
                    key,
                    span,
                    ty: binding_ty,
                } => {
                    seen_assoc_binding = true;
                    if context == TypeContext::TraitBound {
                        self.lower_type_in_context(binding_ty, TypeContext::Value);
                        if !self.is_trait_def(def_id) {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::TYPE_NORMALIZATION,
                                *span,
                                "associated type bindings require a trait bound",
                            ));
                        } else {
                            let Some(LoweredAssocBindingKey { name, .. }) =
                                self.lower_assoc_binding_key(key, Some(TraitId::Source(def_id)))
                            else {
                                continue;
                            };
                            if !seen_assoc_bindings.insert(self.assoc_binding_seen_key(
                                name,
                                None,
                                &[],
                                &[],
                            )) {
                                let name = self.symbol_name(*name);
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::TYPE_NORMALIZATION,
                                    *span,
                                    format!("duplicate associated type binding `{name}`"),
                                ));
                            }
                            if !self.trait_has_associated_type(def_id, name) {
                                let name = self.symbol_name(*name);
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::TYPE_NORMALIZATION,
                                    *span,
                                    format!("trait does not define associated type `{name}`"),
                                ));
                            }
                        }
                    } else {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            *span,
                            "associated type bindings are only valid in trait bounds",
                        ));
                    }
                }
            }
        }
        self.check_type_arg_count(span, def_id, positional_index);
        if context == TypeContext::ExtendTarget && self.is_trait_def(def_id) {
            let object_args = self
                .lower_trait_object_args(span, segment, TraitId::Source(def_id))
                .unwrap_or_default();
            return self.append.intern(TyKind::TraitObjectPointee {
                trait_id: TraitId::Source(def_id),
                trait_args: object_args.trait_args,
                trait_const_args: object_args.trait_const_args,
                associated_type_bindings: object_args.associated_type_bindings,
            });
        }
        self.append.intern(TyKind::Nominal {
            def_id,
            args,
            const_args,
        })
    }

    fn lower_type_or_const_type_arg(&mut self, ty: &TypeRef) -> InternedTyId {
        if matches!(ty.kind, TypeKind::Path { .. })
            && !self
                .resolved
                .node_type_names
                .contains_key(ty.node_key.site())
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                ty.span,
                "expected type generic argument",
            ));
            return self.append.intern(TyKind::Error);
        }
        self.lower_type_in_context(ty, TypeContext::Value)
    }

    fn lower_trait_object_type(&mut self, is_readonly: bool, ty: &TypeRef) -> Option<InternedTyId> {
        let TypeKind::Path { segments } = &ty.kind else {
            return None;
        };
        let type_segment = type_name_segment(segments)?;
        match self
            .resolved
            .node_type_names
            .get(ty.node_key.site())
            .copied()
        {
            Some(TypeNameResolution::BuiltinTrait(trait_id)) => {
                Some(self.lower_builtin_trait_object(ty.span, is_readonly, type_segment, trait_id))
            }
            Some(TypeNameResolution::Def(def_id)) => {
                let def_id = self
                    .resolved
                    .node_qualified_type_names
                    .get(ty.node_key.site())
                    .copied()
                    .unwrap_or(GlobalDefId {
                        module_id: self.module_id,
                        def_id,
                    });
                self.lower_source_trait_object(ty.span, is_readonly, type_segment, def_id)
            }
            Some(TypeNameResolution::External(def_id)) => {
                self.lower_source_trait_object(ty.span, is_readonly, type_segment, def_id)
            }
            _ => None,
        }
    }

    fn lower_source_trait_object(
        &mut self,
        span: Span,
        is_readonly: bool,
        segment: &TypePathSegment,
        def_id: GlobalDefId,
    ) -> Option<InternedTyId> {
        if !self.is_trait_def(def_id) {
            return None;
        }
        let object_args = self.lower_trait_object_args(span, segment, TraitId::Source(def_id))?;
        self.check_type_arg_count(
            span,
            def_id,
            object_args.trait_args.len() + object_args.trait_const_args.len(),
        );
        Some(self.append.intern(TyKind::TraitObject {
            is_readonly,
            trait_id: TraitId::Source(def_id),
            trait_args: object_args.trait_args,
            trait_const_args: object_args.trait_const_args,
            associated_type_bindings: object_args.associated_type_bindings,
        }))
    }

    fn lower_builtin_trait_object(
        &mut self,
        span: Span,
        is_readonly: bool,
        segment: &TypePathSegment,
        trait_id: BuiltinTrait,
    ) -> InternedTyId {
        let object_args = self
            .lower_trait_object_args(span, segment, TraitId::Builtin(trait_id))
            .unwrap_or_default();
        self.check_builtin_trait_arg_count(span, trait_id, object_args.trait_args.len());
        self.append.intern(TyKind::TraitObject {
            is_readonly,
            trait_id: TraitId::Builtin(trait_id),
            trait_args: object_args.trait_args,
            trait_const_args: object_args.trait_const_args,
            associated_type_bindings: object_args.associated_type_bindings,
        })
    }

    fn lower_trait_object_args(
        &mut self,
        _span: Span,
        segment: &TypePathSegment,
        trait_id: TraitId,
    ) -> Option<TraitObjectArgs> {
        let mut object_args = TraitObjectArgs::default();
        let mut seen_assoc_bindings = HashSet::new();
        let mut seen_assoc_binding = false;
        let generic_params = match trait_id {
            TraitId::Source(def_id) => self.generic_params_for_def(def_id).unwrap_or_default(),
            TraitId::Builtin(_) => Vec::new(),
        };
        let mut positional_index = 0usize;
        for arg in &segment.args {
            match arg {
                TypeArg::Type(arg_ty) => {
                    if seen_assoc_binding {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            arg_ty.span,
                            "positional type arguments must precede associated type bindings",
                        ));
                    }
                    match generic_params
                        .get(positional_index)
                        .map(|generic| &generic.kind)
                    {
                        Some(GenericParamKind::Const { ty }) => {
                            let Some(value) = self.lower_const_generic_value_from_type_ref(arg_ty)
                            else {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::TYPE_NORMALIZATION,
                                    arg_ty.span,
                                    "expected const generic argument",
                                ));
                                positional_index += 1;
                                continue;
                            };
                            let ty = self.lower_type_in_context(ty, TypeContext::Value);
                            object_args
                                .trait_const_args
                                .push(ConstGenericArg { ty, value });
                        }
                        _ => object_args
                            .trait_args
                            .push(self.lower_type_in_context(arg_ty, TypeContext::Value)),
                    }
                    positional_index += 1;
                }
                TypeArg::Const(expr) => {
                    match generic_params
                        .get(positional_index)
                        .map(|generic| &generic.kind)
                    {
                        Some(GenericParamKind::Const { ty }) => {
                            let Some(value) = self.lower_const_generic_value_from_expr(expr) else {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::TYPE_NORMALIZATION,
                                    expr.span,
                                    "expected const generic argument",
                                ));
                                positional_index += 1;
                                continue;
                            };
                            let ty = self.lower_type_in_context(ty, TypeContext::Value);
                            object_args
                                .trait_const_args
                                .push(ConstGenericArg { ty, value });
                        }
                        _ => {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::TYPE_NORMALIZATION,
                                expr.span,
                                "const value generic argument supplied for type parameter",
                            ));
                        }
                    }
                    positional_index += 1;
                }
                TypeArg::TypeOrConst { ty: arg_ty, expr } => {
                    if seen_assoc_binding {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            arg_ty.span,
                            "positional type arguments must precede associated type bindings",
                        ));
                    }
                    match generic_params
                        .get(positional_index)
                        .map(|generic| &generic.kind)
                    {
                        Some(GenericParamKind::Const { ty }) => {
                            let Some(value) = self.lower_const_generic_value_from_expr(expr) else {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::TYPE_NORMALIZATION,
                                    expr.span,
                                    "expected const generic argument",
                                ));
                                positional_index += 1;
                                continue;
                            };
                            let ty = self.lower_type_in_context(ty, TypeContext::Value);
                            object_args
                                .trait_const_args
                                .push(ConstGenericArg { ty, value });
                        }
                        _ => object_args
                            .trait_args
                            .push(self.lower_type_or_const_type_arg(arg_ty)),
                    }
                    positional_index += 1;
                }
                TypeArg::AssocBinding {
                    key,
                    span,
                    ty: binding_ty,
                } => {
                    seen_assoc_binding = true;
                    let binding_ty = self.lower_type_in_context(binding_ty, TypeContext::Value);
                    let Some(LoweredAssocBindingKey {
                        name,
                        trait_id: binding_trait_id,
                        trait_args: binding_trait_args,
                        trait_const_args: binding_trait_const_args,
                    }) = self.lower_assoc_binding_key(key, Some(trait_id))
                    else {
                        continue;
                    };
                    let seen_key = self.assoc_binding_seen_key(
                        name,
                        binding_trait_id,
                        &binding_trait_args,
                        &binding_trait_const_args,
                    );
                    if !seen_assoc_bindings.insert(seen_key) {
                        let name = self.symbol_name(*name);
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            *span,
                            format!("duplicate associated type binding `{name}`"),
                        ));
                    }
                    let effective_trait = binding_trait_id.unwrap_or(trait_id);
                    if !self.trait_id_has_associated_type(effective_trait, name) {
                        let name = self.symbol_name(*name);
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            *span,
                            format!("trait does not define associated type `{name}`"),
                        ));
                    }
                    object_args
                        .associated_type_bindings
                        .push(AssociatedTypeBindingTy {
                            trait_id: binding_trait_id,
                            trait_args: binding_trait_args,
                            trait_const_args: binding_trait_const_args,
                            name: *name,
                            ty: binding_ty,
                        });
                }
            }
        }
        Some(object_args)
    }

    fn lower_assoc_binding_key<'b>(
        &mut self,
        key: &'b AssocBindingKey,
        target_trait: Option<TraitId>,
    ) -> Option<LoweredAssocBindingKey<'b>> {
        match key {
            AssocBindingKey::Name(name) => Some(LoweredAssocBindingKey {
                name,
                trait_id: None,
                trait_args: Vec::new(),
                trait_const_args: Vec::new(),
            }),
            AssocBindingKey::Projection(projection) => {
                let TypeKind::Projection {
                    ty,
                    trait_ref,
                    name,
                } = &projection.kind
                else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_NORMALIZATION,
                        projection.span,
                        "associated type binding projection key must be `[Self as Trait]::SymbolId`",
                    ));
                    return None;
                };
                if !matches!(ty.kind, TypeKind::SelfType) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_NORMALIZATION,
                        ty.span,
                        "associated type binding projection must project from `Self`",
                    ));
                }
                let lowered_trait = self.lower_type_in_context(trait_ref, TypeContext::TraitBound);
                let (trait_id, trait_args, trait_const_args) = match self
                    .type_store
                    .get(self.normalize_if_known(lowered_trait))
                    .cloned()
                {
                    Some(TyKind::Nominal {
                        def_id,
                        args,
                        const_args,
                    }) => (TraitId::Source(def_id), args, const_args),
                    Some(TyKind::BuiltinTrait { trait_id, args }) => {
                        (TraitId::Builtin(trait_id), args, Vec::new())
                    }
                    Some(TyKind::TraitObject {
                        trait_id,
                        trait_args,
                        trait_const_args,
                        ..
                    }) => (trait_id, trait_args, trait_const_args),
                    _ => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            trait_ref.span,
                            "associated type binding projection trait must resolve to a trait",
                        ));
                        return None;
                    }
                };
                if let Some(target_trait) = target_trait
                    && trait_id == target_trait
                {
                    return Some(LoweredAssocBindingKey {
                        name,
                        trait_id: None,
                        trait_args,
                        trait_const_args,
                    });
                }
                Some(LoweredAssocBindingKey {
                    name,
                    trait_id: Some(trait_id),
                    trait_args,
                    trait_const_args,
                })
            }
        }
    }

    fn assoc_binding_seen_key(
        &self,
        name: &SymbolId,
        trait_id: Option<TraitId>,
        trait_args: &[InternedTyId],
        trait_const_args: &[ConstGenericArg],
    ) -> String {
        format!(
            "{trait_id:?}:{trait_args:?}:{trait_const_args:?}:{}",
            symbol_identity_key(*name)
        )
    }

    fn lower_builtin_trait_or_extend_target_type(
        &mut self,
        span: Span,
        segment: &TypePathSegment,
        trait_id: BuiltinTrait,
        context: TypeContext,
    ) -> InternedTyId {
        if context == TypeContext::ExtendTarget {
            let object_args = self
                .lower_trait_object_args(span, segment, TraitId::Builtin(trait_id))
                .unwrap_or_default();
            self.check_builtin_trait_arg_count(span, trait_id, object_args.trait_args.len());
            return self.append.intern(TyKind::TraitObjectPointee {
                trait_id: TraitId::Builtin(trait_id),
                trait_args: object_args.trait_args,
                trait_const_args: object_args.trait_const_args,
                associated_type_bindings: object_args.associated_type_bindings,
            });
        }
        let mut args = Vec::new();
        let mut seen_assoc_bindings = HashSet::new();
        let mut seen_assoc_binding = false;
        for arg in &segment.args {
            match arg {
                TypeArg::Type(arg_ty) => {
                    if seen_assoc_binding {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            arg_ty.span,
                            "positional type arguments must precede associated type bindings",
                        ));
                    }
                    args.push(self.lower_type_in_context(arg_ty, TypeContext::Value));
                }
                TypeArg::Const(expr) => {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_NORMALIZATION,
                        expr.span,
                        "const value generic arguments are not supported",
                    ));
                }
                TypeArg::TypeOrConst { ty, .. } => {
                    if seen_assoc_binding {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            ty.span,
                            "positional type arguments must precede associated type bindings",
                        ));
                    }
                    args.push(self.lower_type_or_const_type_arg(ty));
                }
                TypeArg::AssocBinding {
                    key,
                    span,
                    ty: binding_ty,
                } => {
                    seen_assoc_binding = true;
                    if context == TypeContext::TraitBound {
                        self.lower_type_in_context(binding_ty, TypeContext::Value);
                        let Some(LoweredAssocBindingKey {
                            name,
                            trait_id: binding_trait_id,
                            trait_args: binding_trait_args,
                            trait_const_args: binding_trait_const_args,
                        }) = self.lower_assoc_binding_key(key, Some(TraitId::Builtin(trait_id)))
                        else {
                            continue;
                        };
                        let seen_key = self.assoc_binding_seen_key(
                            name,
                            binding_trait_id,
                            &binding_trait_args,
                            &binding_trait_const_args,
                        );
                        if !seen_assoc_bindings.insert(seen_key) {
                            let name = self.symbol_name(*name);
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::TYPE_NORMALIZATION,
                                *span,
                                format!("duplicate associated type binding `{name}`"),
                            ));
                        }
                        let valid = match binding_trait_id {
                            Some(TraitId::Builtin(binding_trait)) => {
                                builtin_trait_has_associated_type(binding_trait, name)
                            }
                            Some(TraitId::Source(def_id)) => {
                                self.trait_has_associated_type(def_id, name)
                            }
                            None => builtin_trait_has_associated_type(trait_id, name),
                        };
                        if !valid {
                            let name = self.symbol_name(*name);
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::TYPE_NORMALIZATION,
                                *span,
                                format!("trait does not define associated type `{name}`"),
                            ));
                        }
                    } else {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            *span,
                            "associated type bindings are only valid in trait bounds",
                        ));
                    }
                }
            }
        }
        self.check_builtin_trait_arg_count(span, trait_id, args.len());
        self.append.intern(TyKind::BuiltinTrait { trait_id, args })
    }

    fn projection_trait_id(
        &mut self,
        trait_ty: InternedTyId,
    ) -> Option<(TraitId, Vec<InternedTyId>, Vec<ConstGenericArg>)> {
        match self.type_store.get(trait_ty).cloned() {
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) if self.is_trait_def(def_id) => Some((TraitId::Source(def_id), args, const_args)),
            Some(TyKind::TraitObject {
                trait_id,
                trait_args,
                trait_const_args,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                ..
            }) => Some((trait_id, trait_args, trait_const_args)),
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                Some((TraitId::Builtin(trait_id), args, Vec::new()))
            }
            _ => None,
        }
    }

    fn is_trait_def(&mut self, def_id: GlobalDefId) -> bool {
        self.defs_for_module(def_id.module_id)
            .and_then(|defs| defs.defs.get(def_id.def_id).map(|def| def.kind))
            == Some(nia_defs::DefKind::Trait)
    }

    fn trait_has_associated_type(&mut self, trait_id: GlobalDefId, name: &SymbolId) -> bool {
        let Some(defs) = self.defs_for_module(trait_id.module_id) else {
            return true;
        };
        let Some(members) = defs.scopes.struct_members.get(&trait_id.def_id) else {
            return true;
        };
        members.fields.get(name).is_some_and(|def_id| {
            defs.defs
                .get(def_id)
                .is_some_and(|def| def.kind == nia_defs::DefKind::TraitAssociatedType)
        })
    }

    fn trait_id_has_associated_type(&mut self, trait_id: TraitId, name: &SymbolId) -> bool {
        match trait_id {
            TraitId::Source(def_id) => self.trait_has_associated_type(def_id, name),
            TraitId::Builtin(trait_id) => builtin_trait_has_associated_type(trait_id, name),
        }
    }

    fn check_type_arg_count(&mut self, span: Span, def_id: GlobalDefId, actual: usize) {
        let Some(defs) = self.defs_for_module(def_id.module_id) else {
            return;
        };
        let Some(def) = defs.defs.get(def_id.def_id) else {
            return;
        };
        let expected = def.generics.len();
        let name = def.name;
        if expected != actual {
            let name = self.symbol_name(name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                span,
                format!(
                    "generic argument count mismatch for `{name}`: expected {expected}, got {actual}"
                ),
            ));
        }
    }

    fn generic_params_for_def(&mut self, def_id: GlobalDefId) -> Option<Vec<GenericParam>> {
        self.defs_for_module(def_id.module_id).and_then(|defs| {
            defs.defs
                .get(def_id.def_id)
                .map(|def| def.generic_params.clone())
        })
    }

    fn const_generic_value_from_type_ref(&self, ty: &TypeRef) -> Option<ConstGenericValue> {
        let TypeKind::Path { segments } = &ty.kind else {
            return None;
        };
        if segments.len() == 1 && segments[0].args.is_empty() {
            let name = type_path_segment_name(&segments[0])?;
            if self.is_const_generic_param(name) {
                return Some(ConstGenericValue::GenericParam(*name));
            }
        }
        None
    }

    fn lower_const_generic_value_from_type_ref(
        &mut self,
        ty: &TypeRef,
    ) -> Option<ConstGenericValue> {
        if let Some(value) = self.const_generic_value_from_type_ref(ty) {
            return Some(value);
        }
        let TypeKind::Path { segments } = &ty.kind else {
            return None;
        };
        if segments.iter().any(|segment| !segment.args.is_empty()) {
            return None;
        }
        let expr = expr_from_type_path(ty.span, ty.node_key.clone(), segments)?;
        self.lower_const_generic_value_from_expr(&expr)
    }

    fn lower_const_generic_value_from_expr(&mut self, expr: &Expr) -> Option<ConstGenericValue> {
        if let ExprKind::Ident(name) = &expr.kind
            && self.is_const_generic_param(name)
        {
            return Some(ConstGenericValue::GenericParam(*name));
        }
        if let ExprKind::Bool(value) = &expr.kind {
            return Some(ConstGenericValue::Bool(*value));
        }
        if let ExprKind::Integer(text) = &expr.kind {
            return parse_integer_const_generic(text).map(ConstGenericValue::Int);
        }
        Some(ConstGenericValue::ConstExpr(
            self.register_const_expr_value(expr),
        ))
    }

    fn check_builtin_trait_arg_count(&mut self, span: Span, trait_id: BuiltinTrait, actual: usize) {
        let expected = trait_id.generic_count();
        if expected != actual {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                span,
                format!(
                    "generic argument count mismatch for `{}`: expected {expected}, got {actual}",
                    trait_id.name()
                ),
            ));
        }
    }

    fn with_generics(&mut self, generics: &[GenericParam], f: impl FnOnce(&mut Self)) {
        for generic in generics {
            if let GenericParamKind::Const { ty } = &generic.kind {
                self.lower_type_in_context(ty, TypeContext::Value);
            }
        }
        self.generic_stack.push(generics.to_vec());
        f(self);
        self.generic_stack.pop();
    }

    fn is_const_generic_param(&self, name: &SymbolId) -> bool {
        self.generic_stack.iter().rev().any(|generics| {
            generics.iter().any(|generic| {
                &generic.name == name && matches!(generic.kind, GenericParamKind::Const { .. })
            })
        })
    }

    fn with_self_type(&mut self, self_ty: InternedTyId, f: impl FnOnce(&mut Self)) {
        self.self_type_stack.push(self_ty);
        f(self);
        self.self_type_stack.pop();
    }

    fn with_associated_type_scope(
        &mut self,
        scope: AssociatedTypeScope,
        f: impl FnOnce(&mut Self),
    ) {
        self.associated_type_scope_stack.push(scope);
        f(self);
        self.associated_type_scope_stack.pop();
    }

    fn lower_scoped_associated_type(
        &mut self,
        span: Span,
        name: &SymbolId,
        segment: &TypePathSegment,
    ) -> InternedTyId {
        if !segment.args.is_empty() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                span,
                "associated type shorthand cannot take generic arguments",
            ));
            return self.append.intern(TyKind::Error);
        }
        let Some(scope) = self
            .associated_type_scope_stack
            .iter()
            .rev()
            .find(|scope| scope.names.iter().any(|associated| associated == name))
            .cloned()
        else {
            let name = self.symbol_name(*name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                span,
                format!("unknown associated type `{name}`"),
            ));
            return self.append.intern(TyKind::Error);
        };
        self.append.intern(TyKind::Projection {
            self_ty: scope.self_ty,
            trait_id: scope.trait_id,
            trait_args: scope.trait_args,
            trait_const_args: scope.trait_const_args,
            name: *name,
        })
    }

    fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_from_optional_resolver(self.symbols, symbol)
    }

    fn local_trait_id(&mut self, node_key: &nia_node_id::VersionedNodeKey) -> Option<GlobalDefId> {
        let defs = self.defs_for_module(self.module_id)?;
        let def_id = defs.def_nodes.get(node_key)?;
        Some(GlobalDefId {
            module_id: self.module_id,
            def_id,
        })
    }

    fn associated_type_scope_for_trait_impl(
        &mut self,
        self_ty: InternedTyId,
        trait_ty: InternedTyId,
    ) -> Option<AssociatedTypeScope> {
        let trait_ty = self.normalize_if_known(trait_ty);
        let (trait_id, trait_args, trait_const_args) = self.projection_trait_id(trait_ty)?;
        let names = match trait_id {
            TraitId::Source(def_id) => self.source_trait_associated_type_names(def_id),
            TraitId::Builtin(_) => Vec::new(),
        };
        Some(AssociatedTypeScope {
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            names,
        })
    }

    fn source_trait_associated_type_names(&mut self, trait_id: GlobalDefId) -> Vec<SymbolId> {
        let Some(defs) = self.defs_for_module(trait_id.module_id) else {
            return Vec::new();
        };
        defs.defs
            .iter()
            .filter_map(|(_, def)| {
                (def.parent == Some(trait_id.def_id)
                    && def.kind == nia_defs::DefKind::TraitAssociatedType)
                    .then_some(def.name)
            })
            .collect()
    }

    fn lower_where_clause(&mut self, clause: &WhereClause) {
        for predicate in &clause.predicates {
            self.lower_type_in_context(&predicate.ty, TypeContext::Value);
            for bound in &predicate.bounds {
                self.lower_trait_bound(bound);
            }
        }
    }

    fn lower_trait_bound(&mut self, bound: &nia_ast::TypeRef) {
        self.lower_type_in_context(bound, TypeContext::TraitBound);
    }

    fn lower_array_len(&mut self, len: &ArrayLen) -> ArrayLenTy {
        match len {
            ArrayLen::Infer => ArrayLenTy::Infer,
            ArrayLen::Expr(expr) => self.lower_array_len_expr(expr),
        }
    }

    fn lower_array_len_expr(&mut self, expr: &Expr) -> ArrayLenTy {
        if let ExprKind::Ident(name) = &expr.kind
            && self.is_const_generic_param(name)
        {
            return ArrayLenTy::GenericParam(*name);
        }
        if let Some((builtin, type_arg)) = layout_builtin_array_len(expr) {
            ArrayLenTy::Builtin {
                builtin,
                ty: self.lower_type_in_context(type_arg, TypeContext::SizeQuery),
            }
        } else {
            self.register_const_array_len(expr)
        }
    }

    fn register_const_array_len(&mut self, expr: &Expr) -> ArrayLenTy {
        let id = self.register_const_expr_value(expr);
        ArrayLenTy::ConstExpr(id)
    }

    fn register_const_expr_value(&mut self, expr: &Expr) -> GlobalConstExprId {
        self.visit_expr(expr);
        let id = GlobalConstExprId {
            module_id: self.module_id,
            const_expr_id: ConstExprId(self.next_const_expr_id),
        };
        self.next_const_expr_id += 1;
        self.const_exprs.insert(id, expr.clone());
        self.const_expr_summaries
            .insert(id, const_expr_summary(expr));
        id
    }

    fn is_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.type_store.get(ty),
            Some(TyKind::Primitive(
                PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize
                    | PrimitiveTy::U8
                    | PrimitiveTy::U16
                    | PrimitiveTy::U32
                    | PrimitiveTy::U64
                    | PrimitiveTy::U128
                    | PrimitiveTy::Usize
            ))
        )
    }

    fn can_be_integer(&self, ty: InternedTyId) -> bool {
        self.is_integer(ty)
            || matches!(
                self.type_store.get(self.normalize_if_known(ty)),
                Some(TyKind::GenericParam(_))
            )
    }

    fn types_equivalent(&self, left: InternedTyId, right: InternedTyId) -> bool {
        left == right || self.type_store.get(left) == self.type_store.get(right)
    }

    fn invalid_value_type_message(&mut self, ty: InternedTyId) -> Option<&'static str> {
        match self.type_store.get(ty).cloned() {
            Some(TyKind::Primitive(PrimitiveTy::Never)) => {
                Some("`never` is not valid as a value, field, parameter, or array element type")
            }
            Some(TyKind::SlicePointee { .. }) => Some(
                "slice pointee types are unsized and not valid as values, fields, parameters, or array elements; use `&[T]` or `&mut [T]` for a slice value",
            ),
            Some(TyKind::TraitObjectPointee { .. }) => Some(
                "trait object pointee types are unsized and not valid as values, fields, parameters, or array elements; use `&Trait[...]` or `&mut Trait[...]` for a trait object",
            ),
            Some(TyKind::BuiltinTrait { .. }) => Some(
                "trait types are not valid as values, fields, parameters, or array elements; use `&Trait[...]` or `&mut Trait[...]` for a trait object",
            ),
            Some(TyKind::Nominal { def_id, .. }) if self.is_trait_def(def_id) => Some(
                "trait types are not valid as values, fields, parameters, or array elements; use `&Trait[...]` or `&mut Trait[...]` for a trait object",
            ),
            _ => None,
        }
    }

    fn defs_for_module(&mut self, module_id: ModuleId) -> Option<&DefCollection> {
        if !self.defs_cache.contains_key(&module_id) {
            self.defs_cache
                .insert(module_id, (self.program_defs.defs?)(module_id));
        }
        self.defs_cache
            .get(&module_id)
            .and_then(|defs| defs.as_deref())
    }
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
