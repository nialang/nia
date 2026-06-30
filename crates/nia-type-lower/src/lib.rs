// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_ast::{
    ArrayLen, AssocBindingKey, Expr, ExprKind, FunctionItem, Item, ItemKind, Module, TypeArg,
    TypeKind, TypePathSegment, TypeRef, WhereClause,
};
use nia_ast_walk::Visitor;
use nia_defs::DefCollection;
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{ConstExprId, GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNode, ItemTreeNodeKind, ModuleItemTree};
use nia_node_id::NodeSite;
use nia_span::Span;
use nia_ty::{
    ArrayLenTy, AssociatedTypeBindingTy, BuiltinTrait, ConstExprSummary, LayoutBuiltin,
    PrimitiveTy, PrimitiveTypeSpelling, RangeTyKind, TraitId, TyInterner, TyKind,
};
use nia_type_resolve::{TypeNameResolution, TypeResolution};

#[derive(Debug, Clone, PartialEq)]
pub struct TypeLowering {
    pub interner: TyInterner,
    pub type_uses: HashMap<NodeSite, InternedTyId>,
    pub const_exprs: HashMap<GlobalConstExprId, Expr>,
    pub const_expr_summaries: HashMap<GlobalConstExprId, ConstExprSummary>,
    pub diagnostics: Vec<Diagnostic>,
}

impl TypeLowering {
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
                    if let Some(value) = &variant.value {
                        self.visit_expr(value);
                    }
                }
            }
            ItemTreeNodeKind::TypeAlias(alias) => {
                self.visit_where_clause(&alias.where_clause);
                self.visit_type(&alias.ty);
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
    pub defs: Option<&'a dyn Fn(ModuleId) -> Option<DefCollection>>,
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

pub fn lower_module_types(module: &Module, resolved: &TypeResolution) -> TypeLowering {
    let item_tree = ModuleItemTree::from_module(module);
    lower_module_types_from_item_tree(ModuleId(0), &item_tree, resolved)
}

pub fn lower_module_types_with_id(
    module_id: ModuleId,
    module: &Module,
    resolved: &TypeResolution,
) -> TypeLowering {
    let defs = nia_defs::collect_module_defs(module_id, module);
    let mut program_defs = HashMap::new();
    program_defs.insert(module_id, defs);
    let program_defs_by_module = |module_id| program_defs.get(&module_id).cloned();
    let item_tree = ModuleItemTree::from_module(module);
    lower_module_types_from_item_tree_with_defs(
        module_id,
        &item_tree,
        resolved,
        ProgramDefsContext {
            defs: Some(&program_defs_by_module),
        },
    )
}

pub fn lower_module_types_with_defs(
    module_id: ModuleId,
    module: &Module,
    resolved: &TypeResolution,
    program_defs: ProgramDefsContext<'_>,
) -> TypeLowering {
    let item_tree = ModuleItemTree::from_module(module);
    lower_module_types_from_item_tree_with_defs(module_id, &item_tree, resolved, program_defs)
}

pub fn lower_module_types_from_item_tree(
    module_id: ModuleId,
    item_tree: &ModuleItemTree,
    resolved: &TypeResolution,
) -> TypeLowering {
    lower_module_types_from_item_tree_with_defs(
        module_id,
        item_tree,
        resolved,
        ProgramDefsContext::empty(),
    )
}

pub fn lower_module_types_from_active_item_tree(
    module_id: ModuleId,
    item_tree: &ActiveModuleItemTree,
    resolved: &TypeResolution,
    program_defs: ProgramDefsContext<'_>,
) -> TypeLowering {
    lower_module_types_from_items(
        module_id,
        &item_tree.items,
        resolved,
        program_defs,
        TypeLowerMode::All,
    )
}

pub fn lower_module_declaration_types_from_active_item_tree(
    module_id: ModuleId,
    item_tree: &ActiveModuleItemTree,
    resolved: &TypeResolution,
    program_defs: ProgramDefsContext<'_>,
) -> TypeLowering {
    lower_module_types_from_items(
        module_id,
        &item_tree.items,
        resolved,
        program_defs,
        TypeLowerMode::Declarations,
    )
}

pub fn lower_module_types_from_item_tree_with_defs(
    module_id: ModuleId,
    item_tree: &ModuleItemTree,
    resolved: &TypeResolution,
    program_defs: ProgramDefsContext<'_>,
) -> TypeLowering {
    lower_module_types_from_items(
        module_id,
        &item_tree.items,
        resolved,
        program_defs,
        TypeLowerMode::All,
    )
}

fn lower_module_types_from_items(
    module_id: ModuleId,
    items: &[ItemTreeNode],
    resolved: &TypeResolution,
    program_defs: ProgramDefsContext<'_>,
    mode: TypeLowerMode,
) -> TypeLowering {
    let mut lowerer = TypeLowerer {
        module_id,
        resolved,
        program_defs,
        defs_cache: HashMap::new(),
        interner: TyInterner::new(module_id),
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
        interner: lowerer.interner,
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

struct TypeLowerer<'a> {
    module_id: ModuleId,
    resolved: &'a TypeResolution,
    program_defs: ProgramDefsContext<'a>,
    defs_cache: HashMap<ModuleId, Option<DefCollection>>,
    interner: TyInterner,
    type_uses: HashMap<NodeSite, InternedTyId>,
    const_exprs: HashMap<GlobalConstExprId, Expr>,
    const_expr_summaries: HashMap<GlobalConstExprId, ConstExprSummary>,
    diagnostics: Vec<Diagnostic>,
    generic_stack: Vec<Vec<String>>,
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
    names: Vec<String>,
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
    associated_type_bindings: Vec<AssociatedTypeBindingTy>,
}

impl<'ast> Visitor<'ast> for TypeLowerer<'_> {
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
                    let self_ty = lowerer
                        .interner
                        .intern(TyKind::GenericParam("Self".to_string()));
                    lowerer.with_self_type(self_ty, |lowerer| {
                        if let Some(trait_id) = lowerer.local_trait_id(&item.node_key) {
                            let trait_args = item_trait
                                .generics
                                .iter()
                                .map(|generic| {
                                    lowerer
                                        .interner
                                        .intern(TyKind::GenericParam(generic.clone()))
                                })
                                .collect::<Vec<_>>();
                            let associated_types = item_trait
                                .associated_types
                                .iter()
                                .map(|associated_type| associated_type.name.clone())
                                .collect::<Vec<_>>();
                            lowerer.with_associated_type_scope(
                                AssociatedTypeScope {
                                    self_ty,
                                    trait_id: TraitId::Source(trait_id),
                                    trait_args,
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
            }
            ItemKind::TypeAlias(alias) => {
                self.with_generics(&alias.generics, |lowerer| {
                    lowerer.lower_where_clause(&alias.where_clause);
                    lowerer.lower_type_in_context(&alias.ty, TypeContext::Alias);
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

impl TypeLowerer<'_> {
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
                    let self_ty = lowerer
                        .interner
                        .intern(TyKind::GenericParam("Self".to_string()));
                    lowerer.with_self_type(self_ty, |lowerer| {
                        if let Some(trait_id) = lowerer.local_trait_id(&item.node_key) {
                            let trait_args = item_trait
                                .generics
                                .iter()
                                .map(|generic| {
                                    lowerer
                                        .interner
                                        .intern(TyKind::GenericParam(generic.clone()))
                                })
                                .collect::<Vec<_>>();
                            let associated_types = item_trait
                                .associated_types
                                .iter()
                                .map(|associated_type| associated_type.name.clone())
                                .collect::<Vec<_>>();
                            lowerer.with_associated_type_scope(
                                AssociatedTypeScope {
                                    self_ty,
                                    trait_id: TraitId::Source(trait_id),
                                    trait_args,
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
            }
            ItemTreeNodeKind::TypeAlias(alias) => {
                self.with_generics(&alias.generics, |lowerer| {
                    lowerer.lower_where_clause(&alias.where_clause);
                    lowerer.lower_type_in_context(&alias.ty, TypeContext::Alias);
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

impl<'a> TypeLowerer<'a> {
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
            TypeKind::Error => self.interner.error(),
            TypeKind::Infer => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_NORMALIZATION,
                    ty.span,
                    "`_` type inference is not valid in this type lowering context",
                ));
                self.interner.error()
            }
            TypeKind::Void => self.interner.primitive(PrimitiveTy::Void),
            TypeKind::Never => self.interner.primitive(PrimitiveTy::Never),
            TypeKind::Optional { elem } => {
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.interner.intern(TyKind::Optional { elem })
            }
            TypeKind::ErrorUnion { error, value } => {
                let error = self.lower_type_in_context(error, TypeContext::Value);
                let value = self.lower_type_in_context(value, TypeContext::Value);
                self.interner.intern(TyKind::ErrorUnion { error, value })
            }
            TypeKind::SelfType => self.self_type_stack.last().copied().unwrap_or_else(|| {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_NORMALIZATION,
                    ty.span,
                    "`Self` is only valid in traits and extend blocks",
                ));
                self.interner.error()
            }),
            TypeKind::Pointer { is_readonly, elem } => {
                if let Some(trait_object) = self.lower_trait_object_type(*is_readonly, elem) {
                    trait_object
                } else {
                    let elem = self.lower_type_in_context(elem, TypeContext::Value);
                    self.interner.intern(TyKind::Pointer {
                        is_readonly: *is_readonly,
                        elem,
                    })
                }
            }
            TypeKind::VolatilePointer { is_readonly, elem } => {
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.interner.intern(TyKind::VolatilePointer {
                    is_readonly: *is_readonly,
                    elem,
                })
            }
            TypeKind::Slice { is_readonly, elem } => {
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.interner.intern(TyKind::Slice {
                    is_readonly: *is_readonly,
                    elem,
                })
            }
            TypeKind::SlicePointee { elem } => {
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.interner.intern(TyKind::SlicePointee { elem })
            }
            TypeKind::Array { len, elem } => {
                let len = self.lower_array_len(len);
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.interner.intern(TyKind::Array { len, elem })
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
                    None => self.interner.primitive(PrimitiveTy::Void),
                };
                self.interner.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic: *is_variadic,
                })
            }
            TypeKind::Path { segments } => {
                let Some(first) = segments.first() else {
                    return self.interner.error();
                };
                let Some(type_segment) = type_name_segment(segments) else {
                    return self.interner.error();
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
                    Some(TypeNameResolution::GenericParam) => self
                        .interner
                        .intern(TyKind::GenericParam(first.name.clone())),
                    Some(TypeNameResolution::AssociatedType) => {
                        self.lower_scoped_associated_type(ty.span, &first.name, type_segment)
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
                    Some(TypeNameResolution::Error) | None => self.interner.error(),
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
                let Some((trait_id, args)) = self.projection_trait_id(trait_ty) else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_NORMALIZATION,
                        trait_ref.span,
                        "projection trait must resolve to a trait",
                    ));
                    return self.interner.error();
                };
                if !self.trait_id_has_associated_type(trait_id, name) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_NORMALIZATION,
                        ty.span,
                        format!("trait does not define associated type `{name}`"),
                    ));
                    return self.interner.error();
                }
                self.interner.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args: args,
                    name: name.clone(),
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
                return self.interner.error();
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
                    return self.interner.error();
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
            return self.interner.error();
        }
        self.interner.intern(TyKind::Range { kind, bound })
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
                        "comptime value generic arguments are not supported",
                    ));
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
                            let Some((name, _, _)) =
                                self.lower_assoc_binding_key(key, Some(TraitId::Source(def_id)))
                            else {
                                continue;
                            };
                            if !seen_assoc_bindings.insert(self.assoc_binding_seen_key(
                                name,
                                None,
                                &[],
                            )) {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::TYPE_NORMALIZATION,
                                    *span,
                                    format!("duplicate associated type binding `{name}`"),
                                ));
                            }
                            if !self.trait_has_associated_type(def_id, name) {
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
        self.check_type_arg_count(span, def_id, args.len());
        if context == TypeContext::ExtendTarget && self.is_trait_def(def_id) {
            let object_args = self
                .lower_trait_object_args(span, segment, TraitId::Source(def_id))
                .unwrap_or_default();
            return self.interner.intern(TyKind::TraitObjectPointee {
                trait_id: TraitId::Source(def_id),
                trait_args: object_args.trait_args,
                associated_type_bindings: object_args.associated_type_bindings,
            });
        }
        self.interner.intern(TyKind::Nominal { def_id, args })
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
        self.check_type_arg_count(span, def_id, object_args.trait_args.len());
        Some(self.interner.intern(TyKind::TraitObject {
            is_readonly,
            trait_id: TraitId::Source(def_id),
            trait_args: object_args.trait_args,
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
        self.interner.intern(TyKind::TraitObject {
            is_readonly,
            trait_id: TraitId::Builtin(trait_id),
            trait_args: object_args.trait_args,
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
                    object_args
                        .trait_args
                        .push(self.lower_type_in_context(arg_ty, TypeContext::Value));
                }
                TypeArg::Const(expr) => {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_NORMALIZATION,
                        expr.span,
                        "comptime value generic arguments are not supported",
                    ));
                }
                TypeArg::AssocBinding {
                    key,
                    span,
                    ty: binding_ty,
                } => {
                    seen_assoc_binding = true;
                    let binding_ty = self.lower_type_in_context(binding_ty, TypeContext::Value);
                    let Some((name, binding_trait_id, binding_trait_args)) =
                        self.lower_assoc_binding_key(key, Some(trait_id))
                    else {
                        continue;
                    };
                    let seen_key =
                        self.assoc_binding_seen_key(name, binding_trait_id, &binding_trait_args);
                    if !seen_assoc_bindings.insert(seen_key) {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            *span,
                            format!("duplicate associated type binding `{name}`"),
                        ));
                    }
                    let effective_trait = binding_trait_id.unwrap_or(trait_id);
                    if !self.trait_id_has_associated_type(effective_trait, name) {
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
                            name: name.to_string(),
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
    ) -> Option<(&'b str, Option<TraitId>, Vec<InternedTyId>)> {
        match key {
            AssocBindingKey::Name(name) => Some((name.as_str(), None, Vec::new())),
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
                        "associated type binding projection key must be `[Self as Trait]::Name`",
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
                let (trait_id, trait_args) = match self
                    .interner
                    .get(self.normalize_if_known(lowered_trait))
                    .cloned()
                {
                    Some(TyKind::Nominal { def_id, args }) => (TraitId::Source(def_id), args),
                    Some(TyKind::BuiltinTrait { trait_id, args }) => {
                        (TraitId::Builtin(trait_id), args)
                    }
                    Some(TyKind::TraitObject {
                        trait_id,
                        trait_args,
                        ..
                    }) => (trait_id, trait_args),
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
                    return Some((name.as_str(), None, trait_args));
                }
                Some((name.as_str(), Some(trait_id), trait_args))
            }
        }
    }

    fn assoc_binding_seen_key(
        &self,
        name: &str,
        trait_id: Option<TraitId>,
        trait_args: &[InternedTyId],
    ) -> String {
        format!("{trait_id:?}:{trait_args:?}:{name}")
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
            return self.interner.intern(TyKind::TraitObjectPointee {
                trait_id: TraitId::Builtin(trait_id),
                trait_args: object_args.trait_args,
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
                        "comptime value generic arguments are not supported",
                    ));
                }
                TypeArg::AssocBinding {
                    key,
                    span,
                    ty: binding_ty,
                } => {
                    seen_assoc_binding = true;
                    if context == TypeContext::TraitBound {
                        self.lower_type_in_context(binding_ty, TypeContext::Value);
                        let Some((name, binding_trait_id, binding_trait_args)) =
                            self.lower_assoc_binding_key(key, Some(TraitId::Builtin(trait_id)))
                        else {
                            continue;
                        };
                        let seen_key = self.assoc_binding_seen_key(
                            name,
                            binding_trait_id,
                            &binding_trait_args,
                        );
                        if !seen_assoc_bindings.insert(seen_key) {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::TYPE_NORMALIZATION,
                                *span,
                                format!("duplicate associated type binding `{name}`"),
                            ));
                        }
                        let valid = match binding_trait_id {
                            Some(TraitId::Builtin(binding_trait)) => {
                                binding_trait.has_associated_type(name)
                            }
                            Some(TraitId::Source(def_id)) => {
                                self.trait_has_associated_type(def_id, name)
                            }
                            None => trait_id.has_associated_type(name),
                        };
                        if !valid {
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
        self.interner
            .intern(TyKind::BuiltinTrait { trait_id, args })
    }

    fn projection_trait_id(
        &mut self,
        trait_ty: InternedTyId,
    ) -> Option<(TraitId, Vec<InternedTyId>)> {
        match self.interner.get(trait_ty).cloned() {
            Some(TyKind::Nominal { def_id, args }) if self.is_trait_def(def_id) => {
                Some((TraitId::Source(def_id), args))
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                Some((TraitId::Builtin(trait_id), args))
            }
            _ => None,
        }
    }

    fn is_trait_def(&mut self, def_id: GlobalDefId) -> bool {
        self.defs_for_module(def_id.module_id)
            .and_then(|defs| defs.defs.get(def_id.def_id).map(|def| def.kind))
            == Some(nia_defs::DefKind::Trait)
    }

    fn trait_has_associated_type(&mut self, trait_id: GlobalDefId, name: &str) -> bool {
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

    fn trait_id_has_associated_type(&mut self, trait_id: TraitId, name: &str) -> bool {
        match trait_id {
            TraitId::Source(def_id) => self.trait_has_associated_type(def_id, name),
            TraitId::Builtin(trait_id) => trait_id.has_associated_type(name),
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
        let name = def.name.clone();
        if expected != actual {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                span,
                format!(
                    "generic argument count mismatch for `{name}`: expected {expected}, got {actual}"
                ),
            ));
        }
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

    fn with_generics(&mut self, generics: &[String], f: impl FnOnce(&mut Self)) {
        self.generic_stack.push(generics.to_vec());
        f(self);
        self.generic_stack.pop();
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
        name: &str,
        segment: &TypePathSegment,
    ) -> InternedTyId {
        if !segment.args.is_empty() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                span,
                "associated type shorthand cannot take generic arguments",
            ));
            return self.interner.error();
        }
        let Some(scope) = self
            .associated_type_scope_stack
            .iter()
            .rev()
            .find(|scope| scope.names.iter().any(|associated| associated == name))
            .cloned()
        else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                span,
                format!("unknown associated type `{name}`"),
            ));
            return self.interner.error();
        };
        self.interner.intern(TyKind::Projection {
            self_ty: scope.self_ty,
            trait_id: scope.trait_id,
            trait_args: scope.trait_args,
            name: name.to_string(),
        })
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
        let (trait_id, trait_args) = self.projection_trait_id(trait_ty)?;
        let names = match trait_id {
            TraitId::Source(def_id) => self.source_trait_associated_type_names(def_id),
            TraitId::Builtin(_) => Vec::new(),
        };
        Some(AssociatedTypeScope {
            self_ty,
            trait_id,
            trait_args,
            names,
        })
    }

    fn source_trait_associated_type_names(&mut self, trait_id: GlobalDefId) -> Vec<String> {
        let Some(defs) = self.defs_for_module(trait_id.module_id) else {
            return Vec::new();
        };
        defs.defs
            .iter()
            .filter_map(|(_, def)| {
                (def.parent == Some(trait_id.def_id)
                    && def.kind == nia_defs::DefKind::TraitAssociatedType)
                    .then_some(def.name.clone())
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
        let id = GlobalConstExprId {
            module_id: self.module_id,
            const_expr_id: ConstExprId(self.next_const_expr_id),
        };
        self.next_const_expr_id += 1;
        self.const_exprs.insert(id, expr.clone());
        self.const_expr_summaries
            .insert(id, const_expr_summary(expr));
        ArrayLenTy::ConstExpr(id)
    }

    fn is_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(ty),
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
                self.interner.get(self.normalize_if_known(ty)),
                Some(TyKind::GenericParam(_))
            )
    }

    fn types_equivalent(&self, left: InternedTyId, right: InternedTyId) -> bool {
        left == right || self.interner.get(left) == self.interner.get(right)
    }

    fn invalid_value_type_message(&mut self, ty: InternedTyId) -> Option<&'static str> {
        match self.interner.get(ty).cloned() {
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
            .and_then(|defs| defs.as_ref())
    }
}

fn type_name_segment(segments: &[TypePathSegment]) -> Option<&TypePathSegment> {
    segments.last()
}

impl TypeLowerer<'_> {
    fn lower_primitive_type(&mut self, primitive: PrimitiveTypeSpelling) -> InternedTyId {
        match primitive {
            PrimitiveTypeSpelling::Scalar(primitive) => self.interner.primitive(primitive),
            PrimitiveTypeSpelling::Vector { elem, lanes } => {
                self.interner.intern(TyKind::Vector { elem, lanes })
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
    if root == "std" && builtin_segment == "builtin" {
        LayoutBuiltin::from_name(name).map(|builtin| (builtin, type_arg))
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

fn const_expr_summary(expr: &Expr) -> ConstExprSummary {
    ConstExprSummary {
        span: expr.span,
        literal_array_len: literal_array_len_expr_value(expr),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs, collect_module_defs_from_active_item_tree};
    use nia_item_tree::ModuleItemTree;
    use nia_parser::parse_module;
    use nia_type_resolve::{
        ProgramDefsContext as TypeResolveProgramDefsContext, resolve_module_types,
        resolve_module_types_from_active_item_tree,
    };
    use std::collections::HashMap;

    #[test]
    fn lowers_primitive_pointer_array_function_and_nominal_types() {
        let (module, errors) = parse_module(
            r#"
struct Box[T] {
    value: T,
}

fn make(ptr: &u8, cb: &fn(i32) void) [4]Box[i32] {
    let mut tmp: [_]i32 = [1, 2, 3];
    [{ value: 0 }; 4]
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &resolved);
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        assert!(
            lowered
                .interner
                .get(lowered.interner.error())
                .is_some_and(|ty| matches!(ty, TyKind::Error))
        );
        assert!(
            lowered
                .interner
                .get(lowered.interner.primitive(PrimitiveTy::I8))
                .is_some_and(|_| lowered.interner.len() > 1)
        );
        assert!(
            lowered
                .type_uses
                .values()
                .any(|ty_id| matches!(lowered.interner.get(*ty_id), Some(TyKind::Nominal { .. })))
        );
        assert!(
            lowered
                .type_uses
                .values()
                .any(|ty_id| matches!(lowered.interner.get(*ty_id), Some(TyKind::Array { .. })))
        );
        assert!(
            lowered
                .type_uses
                .values()
                .any(|ty_id| matches!(lowered.interner.get(*ty_id), Some(TyKind::Pointer { .. })))
        );
    }

    #[test]
    fn lowers_trait_associated_type_shorthand_to_projection() {
        let (module, errors) = parse_module(
            r#"
trait Writer {
    type Error;

    fn write(& self) Error!void;
}

enum BufferError: i32 {
    Bad = 1,
    _,
}

struct Sink {}

extend Sink : Writer {
    type Error = BufferError;

    fn write(& self) Error!void {
        _ = self;
        !{}
    }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &resolved);
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        let shorthand_projections = lowered
            .type_uses
            .values()
            .filter(|ty_id| {
                matches!(
                    lowered.interner.get(**ty_id),
                    Some(TyKind::Projection { name, .. }) if name == "Error"
                )
            })
            .count();
        assert!(shorthand_projections >= 2, "{:?}", lowered.type_uses);
    }

    #[test]
    fn lowers_slice_extend_target_to_slice_pointee() {
        let (module, errors) = parse_module(
            r#"
extend[T] [T] {
    fn len2(& self) usize {
        self.len()
    }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &resolved);
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        let nia_ast::ItemKind::Extend(extend) = &module.items[0].kind else {
            panic!("expected extend");
        };
        let target_ty = lowered
            .ty_for_key(&extend.target.node_key)
            .expect("expected lowered extend target");
        assert!(matches!(
            lowered.interner.get(target_ty),
            Some(TyKind::SlicePointee { .. })
        ));
    }

    #[test]
    fn rejects_comptime_value_generic_type_arguments() {
        let (module, errors) = parse_module(
            r#"
struct Box[T] {
    value: T,
}

fn make() Box[4] {
    { value: 0 }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types(&module, &resolved);
        assert!(
            lowered
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("comptime value generic"))
        );
    }

    #[test]
    fn reports_generic_type_argument_count_mismatches() {
        let (module, errors) = parse_module(
            r#"
struct Point {}
struct Box[T] { value: T }
type Pair[T, U] = T;
fn missing_arg(a: Box) {}
fn extra_arg(a: Box[i32, bool]) {}
fn alias_missing_arg(a: Pair[i32]) {}
fn non_generic_arg(a: Point[i32]) {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
        let resolved = resolve_module_types(&module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        let program_defs = HashMap::from([(ModuleId(0), defs.clone())]);
        let program_defs_by_module = |module_id| program_defs.get(&module_id).cloned();
        let lowered = lower_module_types_with_defs(
            ModuleId(0),
            &module,
            &resolved,
            ProgramDefsContext {
                defs: Some(&program_defs_by_module),
            },
        );
        let mismatch_count = lowered
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic
                    .summary
                    .contains("generic argument count mismatch")
            })
            .count();
        assert_eq!(mismatch_count, 4, "{:?}", lowered.diagnostics);
    }

    #[test]
    fn accepts_void_value_types_but_rejects_never_value_types_and_enum_backing_types() {
        let (module, errors) = parse_module(
            r#"
enum Bad: bool {
    A,
}

struct BadFields {
    field: void,
    array: [1]void,
    never_field: never,
}

fn bad_param(x: void) void {}
fn bad_never_param(x: never) void {}
fn good_return() void {}
fn good_never_return() never {}

static mut global_void: void;
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types(&module, &resolved);
        assert!(
            lowered.diagnostics.iter().any(|diagnostic| diagnostic
                .summary
                .contains("enum backing type must be an integer type")),
            "{:?}",
            lowered.diagnostics
        );
        assert_eq!(
            lowered
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.summary.contains("`never` is not valid"))
                .count(),
            2,
            "{:?}",
            lowered.diagnostics
        );
    }

    #[test]
    fn lowers_trait_object_pointer_types() {
        let (module, errors) = parse_module(
            r#"
trait Source[T] {
    type Item;
}

fn read(source: &Source[i32, Item = i32]) void {}
fn write(source: &mut Source[i32, Item = i32]) void {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        let program_defs = HashMap::from([(ModuleId(0), defs.clone())]);
        let program_defs_by_module = |module_id| program_defs.get(&module_id).cloned();
        let lowered = lower_module_types_with_defs(
            ModuleId(0),
            &module,
            &resolved,
            ProgramDefsContext {
                defs: Some(&program_defs_by_module),
            },
        );
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        let trait_objects = lowered
            .type_uses
            .values()
            .filter_map(|ty| match lowered.interner.get(*ty) {
                Some(TyKind::TraitObject {
                    is_readonly,
                    trait_args,
                    associated_type_bindings,
                    ..
                }) => Some((
                    *is_readonly,
                    trait_args.len(),
                    associated_type_bindings.len(),
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(trait_objects.contains(&(true, 1, 1)), "{trait_objects:?}");
        assert!(trait_objects.contains(&(false, 1, 1)), "{trait_objects:?}");
    }

    #[test]
    fn validates_trait_object_associated_type_bindings() {
        let (module, errors) = parse_module(
            r#"
trait Source {
    type Item;
}

fn unknown(source: &Source[Missing = i32]) void {}
fn duplicate(source: &Source[Item = i32, Item = bool]) void {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        let program_defs = HashMap::from([(ModuleId(0), defs.clone())]);
        let program_defs_by_module = |module_id| program_defs.get(&module_id).cloned();
        let lowered = lower_module_types_with_defs(
            ModuleId(0),
            &module,
            &resolved,
            ProgramDefsContext {
                defs: Some(&program_defs_by_module),
            },
        );
        assert!(
            lowered.diagnostics.iter().any(|diagnostic| diagnostic
                .summary
                .contains("trait does not define associated type `Missing`")),
            "{:?}",
            lowered.diagnostics
        );
        assert!(
            lowered.diagnostics.iter().any(|diagnostic| diagnostic
                .summary
                .contains("duplicate associated type binding `Item`")),
            "{:?}",
            lowered.diagnostics
        );
    }

    #[test]
    fn rejects_bare_trait_as_value_type() {
        let (module, errors) = parse_module(
            r#"
trait Show {}

fn bad(value: Show) void {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        let program_defs = HashMap::from([(ModuleId(0), defs.clone())]);
        let program_defs_by_module = |module_id| program_defs.get(&module_id).cloned();
        let lowered = lower_module_types_with_defs(
            ModuleId(0),
            &module,
            &resolved,
            ProgramDefsContext {
                defs: Some(&program_defs_by_module),
            },
        );
        assert!(
            lowered
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("trait types are not valid")),
            "{:?}",
            lowered.diagnostics
        );
    }

    #[test]
    fn lowers_types_from_active_item_tree_only() {
        let (module, errors) = parse_module(
            r#"
@[if false]
fn skipped(value: MissingType) void {}
@[if true]
fn selected(value: i32) void {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let tree = ModuleItemTree::from_module(&module);
        let active = tree.active_items(&mut BoolResolver(false)).unwrap();
        let defs = collect_module_defs_from_active_item_tree(ModuleId(0), &active);
        assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
        let resolved = resolve_module_types_from_active_item_tree(
            &active,
            &defs,
            TypeResolveProgramDefsContext::empty(),
            &nia_defs::PublicSurfaces::default(),
            &nia_defs::ModuleUsingScope::default(),
        );
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        let lowered = lower_module_types_from_active_item_tree(
            ModuleId(0),
            &active,
            &resolved,
            ProgramDefsContext::empty(),
        );
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        assert!(lowered.type_uses.values().any(|ty| matches!(
            lowered.interner.get(*ty),
            Some(TyKind::Primitive(PrimitiveTy::I32))
        )));
    }

    struct BoolResolver(bool);

    impl nia_item_tree::ConditionResolver for BoolResolver {
        fn resolve_condition(
            &mut self,
            cond: &nia_ast::ConditionExpr,
        ) -> Result<bool, nia_item_tree::ItemTreeError> {
            match &cond.kind {
                nia_ast::ConditionExprKind::Bool(value) => Ok(*value),
                _ => Ok(self.0),
            }
        }
    }
}
