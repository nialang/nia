// SPDX-License-Identifier: GPL-3.0-or-later
//! Module-level item trees for conditional selection and signature queries.

use nia_ast::{
    Attribute, AttributeKind, BindingItem, ConditionExpr, ConditionExprKind, EnumItem, EnumVariant,
    ExtendAssociatedType, ExtendAssociatedValue, ExtendItem, ExtendMethod, Field, FunctionItem,
    GenericParam, GenericParamKind, Module, Param, StructItem, TraitAssociatedType,
    TraitAssociatedValue, TraitItem, TraitMethod, TypeAliasItem, UnionItem, UsingGroupItem,
    UsingItem, UsingName, UsingSelector, expr_decl_eq, option_type_ref_decl_eq, type_ref_decl_eq,
    type_refs_decl_eq, where_clause_decl_eq,
};
use nia_span::Span;
use std::{collections::HashSet, ops::Index, sync::Arc};

pub use nia_ast::{Item as ItemTreeNode, ItemKind as ItemTreeNodeKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Consumer-specific subset selected from an active item tree.
pub enum SignatureItemSet {
    /// Items needed to collect ordinary function signatures.
    Functions,
    /// Items needed to collect extension function signatures.
    ExtensionFunctions,
    /// Items needed to collect value signatures.
    Values,
    /// Items needed to collect type signatures.
    Types,
    /// Items needed to collect trait and implementation signatures.
    Traits,
}

#[derive(Debug, Clone, PartialEq)]
/// Module-level items before conditional attributes are evaluated.
pub struct ModuleItemTree {
    /// Items in source order.
    pub items: ItemTreeItems,
}

/// Ordered, immutable handles to shared canonical item payloads.
#[derive(Debug, Clone)]
pub struct ItemTreeItems {
    items: Arc<[Arc<ItemTreeNode>]>,
}

#[derive(Debug, Clone, PartialEq)]
/// Conditionally selected item tree plus inactive source ranges.
pub struct ActiveModuleItemTree {
    /// Active items in source order.
    pub items: ItemTreeItems,
    /// Source ranges excluded by conditional attributes.
    pub inactive_spans: Arc<HashSet<Span>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Failure produced while evaluating an item condition.
pub struct ItemTreeError {
    /// Source range associated with the failure.
    pub span: Span,
    /// User-facing failure description.
    pub message: String,
}

/// Evaluates conditional item attributes for a target/configuration context.
pub trait ConditionResolver {
    /// Resolves one condition or returns a source-located failure.
    fn resolve_condition(&mut self, cond: &ConditionExpr) -> Result<bool, ItemTreeError>;

    /// Resolves one build-profile marker. Older resolvers that only model
    /// target conditions treat profile markers as active.
    fn resolve_profile(&mut self, _profile: nia_ast::ProfileKind) -> Result<bool, ItemTreeError> {
        Ok(true)
    }

    /// Resolves a test-compilation marker. Resolvers that only model target
    /// conditions treat test markers as active.
    fn resolve_test(&mut self) -> Result<bool, ItemTreeError> {
        Ok(true)
    }
}

impl ModuleItemTree {
    /// Projects module-level AST items into an item tree.
    pub fn from_module(module: &Module) -> Self {
        Self {
            items: ItemTreeItems::new(module.items.to_vec()),
        }
    }

    /// Moves module-level AST items into an item tree without cloning payloads.
    pub fn from_owned_module(module: Module) -> Self {
        Self {
            items: ItemTreeItems::new(module.items),
        }
    }

    /// Treats every item as active without evaluating conditional attributes.
    pub fn all_items_active(&self) -> ActiveModuleItemTree {
        ActiveModuleItemTree::from_shared_parts(self.items.clone(), Arc::new(HashSet::new()))
    }

    /// Compares declaration-relevant syntax while ignoring bodies and identities.
    pub fn declaration_eq(&self, other: &Self) -> bool {
        item_tree_nodes_declaration_eq(&self.items, &other.items)
    }

    /// Compares shallow definition shape used by definition-level invalidation.
    pub fn definition_eq(&self, other: &Self) -> bool {
        item_tree_nodes_definition_eq(&self.items, &other.items)
    }

    /// Evaluates conditional attributes and records excluded item ranges.
    pub fn active_items(
        &self,
        resolver: &mut impl ConditionResolver,
    ) -> Result<ActiveModuleItemTree, ItemTreeError> {
        let mut items = Vec::new();
        let mut inactive_spans = HashSet::new();
        collect_active_items(&self.items, resolver, &mut items, &mut inactive_spans)?;
        Ok(ActiveModuleItemTree::from_shared_parts(
            self.items.select(items),
            Arc::new(inactive_spans),
        ))
    }
}

impl ActiveModuleItemTree {
    /// Creates an active tree from selected items and inactive ranges.
    pub fn new(items: Vec<ItemTreeNode>, inactive_spans: HashSet<Span>) -> Self {
        Self {
            items: ItemTreeItems::new(items),
            inactive_spans: Arc::new(inactive_spans),
        }
    }

    /// Creates an active tree from already-shared immutable parts.
    pub fn from_shared_parts(items: ItemTreeItems, inactive_spans: Arc<HashSet<Span>>) -> Self {
        Self {
            items,
            inactive_spans,
        }
    }

    /// Projects active items back into a module AST.
    pub fn to_module(&self) -> Module {
        Module {
            items: self.items.iter().cloned().collect(),
        }
    }

    /// Compares inactive ranges and declaration-relevant item syntax.
    pub fn declaration_eq(&self, other: &Self) -> bool {
        self.inactive_spans == other.inactive_spans
            && item_tree_nodes_declaration_eq(&self.items, &other.items)
    }

    /// Compares inactive ranges and shallow definition shape.
    pub fn definition_eq(&self, other: &Self) -> bool {
        self.inactive_spans == other.inactive_spans
            && item_tree_nodes_definition_eq(&self.items, &other.items)
    }

    /// Filters and trims items for one signature consumer.
    pub fn signature_items(&self, set: SignatureItemSet) -> Self {
        Self::from_shared_parts(
            self.items.project(|item| signature_item(item, set)),
            Arc::clone(&self.inactive_spans),
        )
    }

    /// Filters items to declarations relevant to const signature collection.
    pub fn const_signature_items(&self) -> Self {
        Self::from_shared_parts(
            self.items.project(const_signature_item),
            Arc::clone(&self.inactive_spans),
        )
    }
}

impl PartialEq for ItemTreeItems {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().zip(other.iter()).all(|(lhs, rhs)| lhs == rhs)
    }
}

impl ItemTreeItems {
    fn new(items: Vec<ItemTreeNode>) -> Self {
        Self {
            items: items.into_iter().map(Arc::new).collect::<Vec<_>>().into(),
        }
    }

    /// Returns the number of items in this ordered view.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Reports whether this view contains no items.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Iterates item payloads in source order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &ItemTreeNode> {
        self.items.iter().map(Arc::as_ref)
    }

    /// Clones the immutable payload handles in source order.
    pub fn shared_items(&self) -> impl ExactSizeIterator<Item = Arc<ItemTreeNode>> + '_ {
        self.items.iter().map(Arc::clone)
    }

    /// Builds an ordered view from existing immutable payload handles.
    pub fn from_shared_items(items: Vec<Arc<ItemTreeNode>>) -> Self {
        Self {
            items: items.into(),
        }
    }

    fn select(&self, selected: Vec<Arc<ItemTreeNode>>) -> Self {
        Self {
            items: selected.into(),
        }
    }

    fn project(
        &self,
        mut project: impl FnMut(&Arc<ItemTreeNode>) -> Option<Arc<ItemTreeNode>>,
    ) -> Self {
        Self {
            items: self.items.iter().filter_map(&mut project).collect(),
        }
    }

    #[cfg(test)]
    fn shares_payload(&self, other: &Self, index: usize) -> bool {
        Arc::ptr_eq(&self.items[index], &other.items[index])
    }
}

impl Index<usize> for ItemTreeItems {
    type Output = ItemTreeNode;

    fn index(&self, index: usize) -> &Self::Output {
        &self.items[index]
    }
}

impl From<Vec<ItemTreeNode>> for ItemTreeItems {
    fn from(items: Vec<ItemTreeNode>) -> Self {
        Self::new(items)
    }
}

fn item_tree_nodes_declaration_eq(lhs: &ItemTreeItems, rhs: &ItemTreeItems) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs.iter())
            .all(|(lhs, rhs)| item_tree_node_declaration_eq(lhs, rhs))
}

fn item_tree_nodes_definition_eq(lhs: &ItemTreeItems, rhs: &ItemTreeItems) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs.iter())
            .all(|(lhs, rhs)| item_tree_node_definition_eq(lhs, rhs))
}

fn item_tree_node_declaration_eq(lhs: &ItemTreeNode, rhs: &ItemTreeNode) -> bool {
    item_attributes_declaration_eq(&lhs.attributes, &rhs.attributes)
        && lhs.vis == rhs.vis
        && item_tree_node_kind_declaration_eq(&lhs.kind, &rhs.kind)
}

fn item_tree_node_definition_eq(lhs: &ItemTreeNode, rhs: &ItemTreeNode) -> bool {
    item_attributes_definition_eq(&lhs.attributes, &rhs.attributes)
        && lhs.vis == rhs.vis
        && item_tree_node_kind_definition_eq(&lhs.kind, &rhs.kind)
}

fn item_tree_node_kind_declaration_eq(lhs: &ItemTreeNodeKind, rhs: &ItemTreeNodeKind) -> bool {
    match (lhs, rhs) {
        (ItemTreeNodeKind::Module(lhs), ItemTreeNodeKind::Module(rhs)) => lhs == rhs,
        (ItemTreeNodeKind::Using(lhs), ItemTreeNodeKind::Using(rhs)) => using_decl_eq(lhs, rhs),
        (ItemTreeNodeKind::Struct(lhs), ItemTreeNodeKind::Struct(rhs)) => struct_decl_eq(lhs, rhs),
        (ItemTreeNodeKind::Union(lhs), ItemTreeNodeKind::Union(rhs)) => union_decl_eq(lhs, rhs),
        (ItemTreeNodeKind::Trait(lhs), ItemTreeNodeKind::Trait(rhs)) => trait_decl_eq(lhs, rhs),
        (ItemTreeNodeKind::Extend(lhs), ItemTreeNodeKind::Extend(rhs)) => extend_decl_eq(lhs, rhs),
        (ItemTreeNodeKind::Enum(lhs), ItemTreeNodeKind::Enum(rhs)) => enum_decl_eq(lhs, rhs),
        (ItemTreeNodeKind::TypeAlias(lhs), ItemTreeNodeKind::TypeAlias(rhs)) => {
            type_alias_decl_eq(lhs, rhs)
        }
        (ItemTreeNodeKind::Function(lhs), ItemTreeNodeKind::Function(rhs)) => {
            function_decl_eq(lhs, rhs)
        }
        (ItemTreeNodeKind::Binding(lhs), ItemTreeNodeKind::Binding(rhs)) => {
            binding_decl_eq(lhs, rhs)
        }
        _ => false,
    }
}

fn item_tree_node_kind_definition_eq(lhs: &ItemTreeNodeKind, rhs: &ItemTreeNodeKind) -> bool {
    match (lhs, rhs) {
        (ItemTreeNodeKind::Module(lhs), ItemTreeNodeKind::Module(rhs)) => lhs == rhs,
        (ItemTreeNodeKind::Using(lhs), ItemTreeNodeKind::Using(rhs)) => using_decl_eq(lhs, rhs),
        (ItemTreeNodeKind::Struct(lhs), ItemTreeNodeKind::Struct(rhs)) => {
            lhs.name == rhs.name
                && lhs.is_tuple == rhs.is_tuple
                && lhs.fields.len() == rhs.fields.len()
                && lhs
                    .fields
                    .iter()
                    .zip(rhs.fields.iter())
                    .all(|(lhs, rhs)| lhs.name == rhs.name)
        }
        (ItemTreeNodeKind::Union(lhs), ItemTreeNodeKind::Union(rhs)) => {
            lhs.name == rhs.name
                && lhs.fields.len() == rhs.fields.len()
                && lhs
                    .fields
                    .iter()
                    .zip(rhs.fields.iter())
                    .all(|(lhs, rhs)| lhs.name == rhs.name)
        }
        (ItemTreeNodeKind::Trait(lhs), ItemTreeNodeKind::Trait(rhs)) => {
            lhs.name == rhs.name
                && lhs.associated_types.len() == rhs.associated_types.len()
                && lhs
                    .associated_types
                    .iter()
                    .zip(rhs.associated_types.iter())
                    .all(|(lhs, rhs)| lhs.name == rhs.name)
                && lhs.associated_values.len() == rhs.associated_values.len()
                && lhs
                    .associated_values
                    .iter()
                    .zip(rhs.associated_values.iter())
                    .all(|(lhs, rhs)| lhs.name == rhs.name)
                && lhs.methods.len() == rhs.methods.len()
                && lhs
                    .methods
                    .iter()
                    .zip(rhs.methods.iter())
                    .all(|(lhs, rhs)| {
                        lhs.function.name == rhs.function.name
                            && lhs.function.params.len() == rhs.function.params.len()
                            && lhs
                                .function
                                .params
                                .iter()
                                .zip(rhs.function.params.iter())
                                .all(|(lhs, rhs)| {
                                    lhs.name == rhs.name && lhs.receiver == rhs.receiver
                                })
                    })
        }
        (ItemTreeNodeKind::Extend(lhs), ItemTreeNodeKind::Extend(rhs)) => {
            lhs.associated_types.len() == rhs.associated_types.len()
                && lhs
                    .associated_types
                    .iter()
                    .zip(rhs.associated_types.iter())
                    .all(|(lhs, rhs)| lhs.name == rhs.name)
                && lhs.associated_values.len() == rhs.associated_values.len()
                && lhs
                    .associated_values
                    .iter()
                    .zip(rhs.associated_values.iter())
                    .all(|(lhs, rhs)| lhs.vis == rhs.vis && lhs.binding.name == rhs.binding.name)
                && lhs.methods.len() == rhs.methods.len()
                && lhs
                    .methods
                    .iter()
                    .zip(rhs.methods.iter())
                    .all(|(lhs, rhs)| {
                        lhs.vis == rhs.vis
                            && lhs.function.name == rhs.function.name
                            && lhs.function.params.len() == rhs.function.params.len()
                            && lhs
                                .function
                                .params
                                .iter()
                                .zip(rhs.function.params.iter())
                                .all(|(lhs, rhs)| {
                                    lhs.name == rhs.name && lhs.receiver == rhs.receiver
                                })
                    })
        }
        (ItemTreeNodeKind::Enum(lhs), ItemTreeNodeKind::Enum(rhs)) => {
            lhs.name == rhs.name
                && lhs.variants.len() == rhs.variants.len()
                && lhs
                    .variants
                    .iter()
                    .zip(rhs.variants.iter())
                    .all(|(lhs, rhs)| lhs.name == rhs.name)
        }
        (ItemTreeNodeKind::TypeAlias(lhs), ItemTreeNodeKind::TypeAlias(rhs)) => {
            lhs.name == rhs.name
        }
        (ItemTreeNodeKind::Function(lhs), ItemTreeNodeKind::Function(rhs)) => {
            lhs.name == rhs.name
                && lhs.params.len() == rhs.params.len()
                && lhs
                    .params
                    .iter()
                    .zip(rhs.params.iter())
                    .all(|(lhs, rhs)| lhs.name == rhs.name && lhs.receiver == rhs.receiver)
        }
        (ItemTreeNodeKind::Binding(lhs), ItemTreeNodeKind::Binding(rhs)) => {
            lhs.name == rhs.name && lhs.is_const() == rhs.is_const()
        }
        _ => false,
    }
}

fn item_attributes_declaration_eq(lhs: &[Attribute], rhs: &[Attribute]) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs.iter())
            .all(|(lhs, rhs)| attribute_kind_declaration_eq(&lhs.kind, &rhs.kind))
}

fn attribute_kind_declaration_eq(lhs: &AttributeKind, rhs: &AttributeKind) -> bool {
    match (lhs, rhs) {
        (AttributeKind::If(lhs), AttributeKind::If(rhs)) => condition_declaration_eq(lhs, rhs),
        (AttributeKind::Profile(lhs), AttributeKind::Profile(rhs)) => lhs == rhs,
        (AttributeKind::Test, AttributeKind::Test) => true,
        (AttributeKind::Meta(lhs), AttributeKind::Meta(rhs)) => {
            lhs.path == rhs.path
                && lhs.args.len() == rhs.args.len()
                && lhs
                    .args
                    .iter()
                    .zip(rhs.args.iter())
                    .all(|(lhs, rhs)| expr_decl_eq(lhs, rhs))
        }
        _ => false,
    }
}

fn condition_declaration_eq(lhs: &ConditionExpr, rhs: &ConditionExpr) -> bool {
    match (&lhs.kind, &rhs.kind) {
        (ConditionExprKind::Bool(lhs), ConditionExprKind::Bool(rhs)) => lhs == rhs,
        (ConditionExprKind::Integer(lhs), ConditionExprKind::Integer(rhs))
        | (ConditionExprKind::String(lhs), ConditionExprKind::String(rhs)) => lhs == rhs,
        (ConditionExprKind::Ident(lhs), ConditionExprKind::Ident(rhs)) => lhs == rhs,
        (
            ConditionExprKind::Unary {
                op: lhs_op,
                expr: lhs_expr,
            },
            ConditionExprKind::Unary {
                op: rhs_op,
                expr: rhs_expr,
            },
        ) => lhs_op == rhs_op && condition_declaration_eq(lhs_expr, rhs_expr),
        (
            ConditionExprKind::Binary {
                lhs: lhs_lhs,
                op: lhs_op,
                rhs: lhs_rhs,
            },
            ConditionExprKind::Binary {
                lhs: rhs_lhs,
                op: rhs_op,
                rhs: rhs_rhs,
            },
        ) => {
            lhs_op == rhs_op
                && condition_declaration_eq(lhs_lhs, rhs_lhs)
                && condition_declaration_eq(lhs_rhs, rhs_rhs)
        }
        _ => false,
    }
}

fn item_attributes_definition_eq(lhs: &[Attribute], rhs: &[Attribute]) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs.iter())
            .all(|(lhs, rhs)| attribute_kind_declaration_eq(&lhs.kind, &rhs.kind))
}

fn signature_item(item: &Arc<ItemTreeNode>, set: SignatureItemSet) -> Option<Arc<ItemTreeNode>> {
    match (&item.kind, set) {
        (ItemTreeNodeKind::Struct(_), SignatureItemSet::Values)
        | (ItemTreeNodeKind::Union(_), SignatureItemSet::Values)
        | (ItemTreeNodeKind::Enum(_), SignatureItemSet::Values)
        | (ItemTreeNodeKind::TypeAlias(_), SignatureItemSet::Values)
        | (
            ItemTreeNodeKind::Function(_),
            SignatureItemSet::ExtensionFunctions
            | SignatureItemSet::Values
            | SignatureItemSet::Types
            | SignatureItemSet::Traits,
        )
        | (
            ItemTreeNodeKind::Binding(_),
            SignatureItemSet::Functions
            | SignatureItemSet::ExtensionFunctions
            | SignatureItemSet::Types
            | SignatureItemSet::Traits,
        )
        | (ItemTreeNodeKind::Trait(_), SignatureItemSet::Values | SignatureItemSet::Types)
        | (ItemTreeNodeKind::Extend(_), SignatureItemSet::Types) => None,
        (
            ItemTreeNodeKind::Extend(extend),
            SignatureItemSet::Functions | SignatureItemSet::ExtensionFunctions,
        ) => {
            if extend.associated_values.is_empty() {
                return Some(Arc::clone(item));
            }
            Some(replace_item_kind(
                item,
                ItemTreeNodeKind::Extend(project_extend(
                    extend,
                    extend.associated_types.clone(),
                    Vec::new(),
                    extend.methods.clone(),
                )),
            ))
        }
        (ItemTreeNodeKind::Extend(extend), SignatureItemSet::Values) => {
            if extend.associated_values.is_empty() {
                return None;
            }
            if extend.methods.is_empty() && extend.associated_types.is_empty() {
                return Some(Arc::clone(item));
            }
            Some(replace_item_kind(
                item,
                ItemTreeNodeKind::Extend(project_extend(
                    extend,
                    Vec::new(),
                    extend.associated_values.clone(),
                    Vec::new(),
                )),
            ))
        }
        _ => Some(Arc::clone(item)),
    }
}

fn const_signature_item(item: &Arc<ItemTreeNode>) -> Option<Arc<ItemTreeNode>> {
    match &item.kind {
        ItemTreeNodeKind::Struct(_)
        | ItemTreeNodeKind::Union(_)
        | ItemTreeNodeKind::Enum(_)
        | ItemTreeNodeKind::TypeAlias(_)
        | ItemTreeNodeKind::Module(_)
        | ItemTreeNodeKind::Using(_) => Some(Arc::clone(item)),
        ItemTreeNodeKind::Binding(binding) if binding.is_const() => Some(Arc::clone(item)),
        ItemTreeNodeKind::Function(function) if function.is_const => Some(Arc::clone(item)),
        ItemTreeNodeKind::Extend(extend) => {
            if extend
                .associated_values
                .iter()
                .all(|associated_value| associated_value.binding.is_const())
                && extend.methods.iter().all(|method| method.function.is_const)
            {
                return Some(Arc::clone(item));
            }
            let associated_values = extend
                .associated_values
                .iter()
                .filter(|associated_value| associated_value.binding.is_const())
                .cloned()
                .collect::<Vec<_>>();
            let methods = extend
                .methods
                .iter()
                .filter(|method| method.function.is_const)
                .cloned()
                .collect::<Vec<_>>();
            if extend.associated_types.is_empty()
                && associated_values.is_empty()
                && methods.is_empty()
            {
                return None;
            }
            Some(replace_item_kind(
                item,
                ItemTreeNodeKind::Extend(project_extend(
                    extend,
                    extend.associated_types.clone(),
                    associated_values,
                    methods,
                )),
            ))
        }
        ItemTreeNodeKind::Trait(_)
        | ItemTreeNodeKind::Function(_)
        | ItemTreeNodeKind::Binding(_) => None,
    }
}

fn project_extend(
    extend: &ExtendItem,
    associated_types: Vec<ExtendAssociatedType>,
    associated_values: Vec<ExtendAssociatedValue>,
    methods: Vec<ExtendMethod>,
) -> ExtendItem {
    ExtendItem {
        generics: extend.generics.clone(),
        target: extend.target.clone(),
        trait_ref: extend.trait_ref.clone(),
        where_clause: extend.where_clause.clone(),
        associated_types,
        associated_values,
        methods,
    }
}

fn replace_item_kind(item: &ItemTreeNode, kind: ItemTreeNodeKind) -> Arc<ItemTreeNode> {
    Arc::new(nia_ast::Item {
        span: item.span,
        node_key: item.node_key.clone(),
        attributes: item.attributes.clone(),
        vis: item.vis,
        kind,
    })
}

fn using_decl_eq(lhs: &UsingItem, rhs: &UsingItem) -> bool {
    lhs.host.len() == rhs.host.len()
        && lhs
            .host
            .iter()
            .zip(rhs.host.iter())
            .all(|(lhs, rhs)| lhs.kind == rhs.kind)
        && using_selector_decl_eq(&lhs.selector, &rhs.selector)
}

fn using_selector_decl_eq(lhs: &UsingSelector, rhs: &UsingSelector) -> bool {
    match (lhs, rhs) {
        (UsingSelector::Single(lhs), UsingSelector::Single(rhs)) => using_name_decl_eq(lhs, rhs),
        (UsingSelector::Group(lhs), UsingSelector::Group(rhs)) => {
            lhs.len() == rhs.len()
                && lhs
                    .iter()
                    .zip(rhs.iter())
                    .all(|(lhs, rhs)| using_group_item_decl_eq(lhs, rhs))
        }
        (UsingSelector::Wildcard { .. }, UsingSelector::Wildcard { .. })
        | (UsingSelector::SelfName, UsingSelector::SelfName) => true,
        _ => false,
    }
}

fn using_group_item_decl_eq(lhs: &UsingGroupItem, rhs: &UsingGroupItem) -> bool {
    match (lhs, rhs) {
        (UsingGroupItem::Name(lhs), UsingGroupItem::Name(rhs)) => using_name_decl_eq(lhs, rhs),
        (
            UsingGroupItem::Nested {
                host: lhs_host,
                selector: lhs_selector,
            },
            UsingGroupItem::Nested {
                host: rhs_host,
                selector: rhs_selector,
            },
        ) => {
            lhs_host.len() == rhs_host.len()
                && lhs_host
                    .iter()
                    .zip(rhs_host.iter())
                    .all(|(lhs, rhs)| lhs.kind == rhs.kind)
                && using_selector_decl_eq(lhs_selector, rhs_selector)
        }
        _ => false,
    }
}

fn using_name_decl_eq(lhs: &UsingName, rhs: &UsingName) -> bool {
    lhs.name == rhs.name && lhs.alias == rhs.alias
}

fn generic_params_decl_eq(lhs: &[GenericParam], rhs: &[GenericParam]) -> bool {
    lhs.len() == rhs.len()
        && lhs.iter().zip(rhs.iter()).all(|(lhs, rhs)| {
            lhs.name == rhs.name
                && match (&lhs.kind, &rhs.kind) {
                    (GenericParamKind::Type, GenericParamKind::Type) => true,
                    (GenericParamKind::Const { ty: lhs }, GenericParamKind::Const { ty: rhs }) => {
                        type_ref_decl_eq(lhs, rhs)
                    }
                    _ => false,
                }
        })
}

fn struct_decl_eq(lhs: &StructItem, rhs: &StructItem) -> bool {
    lhs.name == rhs.name
        && generic_params_decl_eq(&lhs.generics, &rhs.generics)
        && where_clause_decl_eq(&lhs.where_clause, &rhs.where_clause)
        && fields_decl_eq(&lhs.fields, &rhs.fields)
        && lhs.is_tuple == rhs.is_tuple
        && lhs.is_extern == rhs.is_extern
}

fn union_decl_eq(lhs: &UnionItem, rhs: &UnionItem) -> bool {
    lhs.name == rhs.name
        && generic_params_decl_eq(&lhs.generics, &rhs.generics)
        && where_clause_decl_eq(&lhs.where_clause, &rhs.where_clause)
        && fields_decl_eq(&lhs.fields, &rhs.fields)
        && lhs.is_extern == rhs.is_extern
}

fn fields_decl_eq(lhs: &[Field], rhs: &[Field]) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs.iter())
            .all(|(lhs, rhs)| field_decl_eq(lhs, rhs))
}

fn field_decl_eq(lhs: &Field, rhs: &Field) -> bool {
    lhs.name == rhs.name
        && type_ref_decl_eq(&lhs.ty, &rhs.ty)
        && item_attributes_declaration_eq(&lhs.attributes, &rhs.attributes)
}

fn trait_decl_eq(lhs: &TraitItem, rhs: &TraitItem) -> bool {
    lhs.name == rhs.name
        && generic_params_decl_eq(&lhs.generics, &rhs.generics)
        && type_refs_decl_eq(&lhs.supertraits, &rhs.supertraits)
        && where_clause_decl_eq(&lhs.where_clause, &rhs.where_clause)
        && trait_associated_types_decl_eq(&lhs.associated_types, &rhs.associated_types)
        && trait_associated_values_decl_eq(&lhs.associated_values, &rhs.associated_values)
        && lhs.methods.len() == rhs.methods.len()
        && lhs
            .methods
            .iter()
            .zip(rhs.methods.iter())
            .all(|(lhs, rhs)| trait_method_decl_eq(lhs, rhs))
}

fn trait_associated_types_decl_eq(
    lhs: &[TraitAssociatedType],
    rhs: &[TraitAssociatedType],
) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs.iter())
            .all(|(lhs, rhs)| lhs.name == rhs.name)
}

fn trait_associated_values_decl_eq(
    lhs: &[TraitAssociatedValue],
    rhs: &[TraitAssociatedValue],
) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs.iter())
            .all(|(lhs, rhs)| lhs.name == rhs.name && type_ref_decl_eq(&lhs.ty, &rhs.ty))
}

fn trait_method_decl_eq(lhs: &TraitMethod, rhs: &TraitMethod) -> bool {
    function_decl_eq(&lhs.function, &rhs.function)
}

fn extend_decl_eq(lhs: &ExtendItem, rhs: &ExtendItem) -> bool {
    generic_params_decl_eq(&lhs.generics, &rhs.generics)
        && type_ref_decl_eq(&lhs.target, &rhs.target)
        && option_type_ref_decl_eq(lhs.trait_ref.as_ref(), rhs.trait_ref.as_ref())
        && where_clause_decl_eq(&lhs.where_clause, &rhs.where_clause)
        && extend_associated_types_decl_eq(&lhs.associated_types, &rhs.associated_types)
        && extend_associated_values_decl_eq(&lhs.associated_values, &rhs.associated_values)
        && lhs.methods.len() == rhs.methods.len()
        && lhs
            .methods
            .iter()
            .zip(rhs.methods.iter())
            .all(|(lhs, rhs)| extend_method_decl_eq(lhs, rhs))
}

fn extend_associated_types_decl_eq(
    lhs: &[ExtendAssociatedType],
    rhs: &[ExtendAssociatedType],
) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs.iter())
            .all(|(lhs, rhs)| lhs.name == rhs.name && type_ref_decl_eq(&lhs.ty, &rhs.ty))
}

fn extend_associated_values_decl_eq(
    lhs: &[ExtendAssociatedValue],
    rhs: &[ExtendAssociatedValue],
) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs.iter())
            .all(|(lhs, rhs)| lhs.vis == rhs.vis && binding_decl_eq(&lhs.binding, &rhs.binding))
}

fn extend_method_decl_eq(lhs: &ExtendMethod, rhs: &ExtendMethod) -> bool {
    lhs.vis == rhs.vis && function_decl_eq(&lhs.function, &rhs.function)
}

fn enum_decl_eq(lhs: &EnumItem, rhs: &EnumItem) -> bool {
    lhs.name == rhs.name
        && option_type_ref_decl_eq(lhs.backing_type.as_ref(), rhs.backing_type.as_ref())
        && lhs.is_open == rhs.is_open
        && enum_variants_decl_eq(&lhs.variants, &rhs.variants)
}

fn enum_variants_decl_eq(lhs: &[EnumVariant], rhs: &[EnumVariant]) -> bool {
    lhs.len() == rhs.len()
        && lhs.iter().zip(rhs.iter()).all(|(lhs, rhs)| {
            lhs.name == rhs.name
                && enum_variant_payload_decl_eq(&lhs.payload, &rhs.payload)
                && match (&lhs.value, &rhs.value) {
                    (Some(lhs), Some(rhs)) => expr_decl_eq(lhs, rhs),
                    (None, None) => true,
                    _ => false,
                }
        })
}

fn enum_variant_payload_decl_eq(
    lhs: &nia_ast::EnumVariantPayload,
    rhs: &nia_ast::EnumVariantPayload,
) -> bool {
    match (lhs, rhs) {
        (nia_ast::EnumVariantPayload::Unit, nia_ast::EnumVariantPayload::Unit) => true,
        (nia_ast::EnumVariantPayload::Tuple(lhs), nia_ast::EnumVariantPayload::Tuple(rhs)) => {
            type_refs_decl_eq(lhs, rhs)
        }
        (nia_ast::EnumVariantPayload::Named(lhs), nia_ast::EnumVariantPayload::Named(rhs)) => {
            fields_decl_eq(lhs, rhs)
        }
        _ => false,
    }
}

fn type_alias_decl_eq(lhs: &TypeAliasItem, rhs: &TypeAliasItem) -> bool {
    lhs.name == rhs.name
        && generic_params_decl_eq(&lhs.generics, &rhs.generics)
        && where_clause_decl_eq(&lhs.where_clause, &rhs.where_clause)
        && match (&lhs.ty, &rhs.ty) {
            (Some(lhs), Some(rhs)) => type_ref_decl_eq(lhs, rhs),
            (None, None) => true,
            _ => false,
        }
}

fn function_decl_eq(lhs: &FunctionItem, rhs: &FunctionItem) -> bool {
    lhs.name == rhs.name
        && generic_params_decl_eq(&lhs.generics, &rhs.generics)
        && where_clause_decl_eq(&lhs.where_clause, &rhs.where_clause)
        && params_decl_eq(&lhs.params, &rhs.params)
        && option_type_ref_decl_eq(lhs.return_type.as_ref(), rhs.return_type.as_ref())
        && lhs.is_extern == rhs.is_extern
        && lhs.is_const == rhs.is_const
        && lhs.is_variadic == rhs.is_variadic
}

fn params_decl_eq(lhs: &[Param], rhs: &[Param]) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs.iter())
            .all(|(lhs, rhs)| param_decl_eq(lhs, rhs))
}

fn param_decl_eq(lhs: &Param, rhs: &Param) -> bool {
    lhs.receiver == rhs.receiver
        && lhs.name == rhs.name
        && option_type_ref_decl_eq(lhs.ty.as_ref(), rhs.ty.as_ref())
}

fn binding_decl_eq(lhs: &BindingItem, rhs: &BindingItem) -> bool {
    lhs.name == rhs.name
        && option_type_ref_decl_eq(lhs.ty.as_ref(), rhs.ty.as_ref())
        && lhs.is_mutable() == rhs.is_mutable()
        && lhs.is_const() == rhs.is_const()
        && lhs.is_extern() == rhs.is_extern()
}

/// Lowers module-level AST items into the query-facing item tree.
pub fn lower_module_items(module: &Module) -> ModuleItemTree {
    ModuleItemTree::from_module(module)
}

fn collect_active_items(
    items: &ItemTreeItems,
    resolver: &mut impl ConditionResolver,
    active_items: &mut Vec<Arc<ItemTreeNode>>,
    inactive_spans: &mut HashSet<Span>,
) -> Result<(), ItemTreeError> {
    for item in items.items.iter() {
        if item_is_active(item, resolver)? {
            active_items.push(Arc::clone(item));
        } else {
            inactive_spans.insert(item.span);
        }
    }
    Ok(())
}

fn item_is_active(
    item: &ItemTreeNode,
    resolver: &mut impl ConditionResolver,
) -> Result<bool, ItemTreeError> {
    let mut profile = None;
    let mut has_test = false;
    for attribute in &item.attributes {
        match attribute.kind {
            AttributeKind::Profile(current) => {
                if profile.is_some() {
                    return Err(ItemTreeError {
                        span: attribute.span,
                        message: "an item cannot use more than one build profile".to_string(),
                    });
                }
                profile = Some(current);
            }
            AttributeKind::Test => {
                if has_test {
                    return Err(ItemTreeError {
                        span: attribute.span,
                        message: "an item cannot use `@[test]` more than once".to_string(),
                    });
                }
                has_test = true;
            }
            AttributeKind::If(_) | AttributeKind::Meta(_) => {}
        }
    }
    for attribute in &item.attributes {
        match &attribute.kind {
            AttributeKind::If(cond) => {
                if !resolver.resolve_condition(cond)? {
                    return Ok(false);
                }
            }
            AttributeKind::Profile(profile) => {
                if !resolver.resolve_profile(*profile)? {
                    return Ok(false);
                }
            }
            AttributeKind::Test => {
                if !resolver.resolve_test()? {
                    return Ok(false);
                }
            }
            AttributeKind::Meta(_) => {}
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_parser::parse_module;
    use nia_source::{SourceId, SourceRevision, SourceVersion};
    use nia_symbol::{SymbolId, stable_hash};

    fn sym(text: &str) -> SymbolId {
        SymbolId::from_stable_hash(stable_hash(text))
    }

    #[test]
    fn keeps_conditional_attributes_as_item_attributes() {
        let (module, errors) = parse_module(
            r#"
@[if os == "linux"]
fn selected() i32 { 1 }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let tree = lower_module_items(&module);
        assert_eq!(tree.items.len(), 1);
        assert_eq!(tree.items[0].attributes.len(), 1);
        assert!(matches!(
            tree.items[0].attributes[0].kind,
            AttributeKind::If(_)
        ));
    }

    #[test]
    fn all_items_active_shares_module_item_storage() {
        let (module, errors) = parse_module("fn main() i32 { 0 }");
        assert!(errors.is_empty(), "{errors:?}");
        let tree = lower_module_items(&module);

        let active = tree.all_items_active();

        assert!(tree.items.shares_payload(&active.items, 0));
        assert!(active.inactive_spans.is_empty());
    }

    #[test]
    fn signature_projection_shares_inactive_span_storage() {
        let (module, errors) = parse_module(
            r#"
@[if false]
fn skipped() i32 { 0 }
fn selected() i32 { 1 }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let tree = lower_module_items(&module);
        let active = tree.active_items(&mut BoolResolver).unwrap();

        let signature = active.signature_items(SignatureItemSet::Functions);

        assert!(Arc::ptr_eq(
            &active.inactive_spans,
            &signature.inactive_spans
        ));
    }

    #[test]
    fn ordinary_signature_projection_shares_item_payload() {
        let (module, errors) = parse_module("fn selected() i32 { 1 }");
        assert!(errors.is_empty(), "{errors:?}");
        let active = lower_module_items(&module).all_items_active();

        let signature = active.signature_items(SignatureItemSet::Functions);

        assert!(active.items.shares_payload(&signature.items, 0));
    }

    #[test]
    fn trimmed_extend_signature_replaces_only_its_payload() {
        let (module, errors) = parse_module(
            r#"
fn selected() i32 { 1 }
extend i32 {
    fn value(self) i32 { self }
    const answer: i32 = 42;
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let active = lower_module_items(&module).all_items_active();

        let signature = active.signature_items(SignatureItemSet::Functions);

        assert!(active.items.shares_payload(&signature.items, 0));
        assert!(!active.items.shares_payload(&signature.items, 1));
    }

    #[test]
    fn already_projected_extend_signature_shares_its_payload() {
        let (module, errors) = parse_module(
            r#"
extend i32 {
    fn value(self) i32 { self }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let active = lower_module_items(&module).all_items_active();

        let signature = active.signature_items(SignatureItemSet::Functions);

        assert!(active.items.shares_payload(&signature.items, 0));
    }

    #[test]
    fn preserves_item_attributes_in_tree_nodes_and_ast_projection() {
        let (module, errors) = parse_module(
            r#"
@[linkName("runtime_start")]
pub extern fn start(argc: i32) i32;
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let tree = lower_module_items(&module);
        assert_eq!(tree.items.len(), 1);
        assert_eq!(tree.items[0].attributes.len(), 1);
        assert!(matches!(
            &tree.items[0].attributes[0].kind,
            AttributeKind::Meta(meta) if meta.path == [sym("linkName")]
        ));

        let projected = tree.all_items_active().to_module();
        assert_eq!(projected.items[0].attributes.len(), 1);
        assert!(matches!(
            &projected.items[0].attributes[0].kind,
            AttributeKind::Meta(meta) if meta.path == [sym("linkName")]
        ));
    }

    #[test]
    fn resolves_active_items_through_condition_resolver() {
        let (module, errors) = parse_module(
            r#"
fn always() i32 { 1 }
@[if false]
fn skipped() i32 { 0 }
@[if true]
fn selected() i32 { 2 }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let tree = lower_module_items(&module);
        let active = tree.active_items(&mut BoolResolver).unwrap();
        let names = active
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemTreeNodeKind::Function(function) => Some(function.name),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(names, [sym("always"), sym("selected")]);
        assert!(!active.inactive_spans.is_empty());
    }

    #[test]
    fn function_body_changes_do_not_change_item_tree_declaration_shape() {
        let (before, before_errors) = parse_module("pub fn main() i32 { 0 }");
        let (after, after_errors) = parse_module("pub fn main() i32 { 1 }");
        assert!(before_errors.is_empty(), "{before_errors:?}");
        assert!(after_errors.is_empty(), "{after_errors:?}");

        let before_tree = lower_module_items(&before);
        let after_tree = lower_module_items(&after);

        assert_ne!(before_tree, after_tree);
        assert!(before_tree.declaration_eq(&after_tree));
    }

    #[test]
    fn function_signature_changes_do_not_change_definition_shape() {
        let (before, before_errors) = parse_module("pub fn main() i32 { 0 }");
        let (after, after_errors) = parse_module("pub fn main() u8 { 0 }");
        assert!(before_errors.is_empty(), "{before_errors:?}");
        assert!(after_errors.is_empty(), "{after_errors:?}");

        let before_tree = lower_module_items(&before);
        let after_tree = lower_module_items(&after);

        assert_ne!(before_tree, after_tree);
        assert!(!before_tree.declaration_eq(&after_tree));
        assert!(before_tree.definition_eq(&after_tree));
    }

    #[test]
    fn definition_shape_tracks_named_children() {
        let (before, before_errors) = parse_module("struct Pair { a: i32 }");
        let (after, after_errors) = parse_module("struct Pair { b: i32 }");
        assert!(before_errors.is_empty(), "{before_errors:?}");
        assert!(after_errors.is_empty(), "{after_errors:?}");

        let before_tree = lower_module_items(&before);
        let after_tree = lower_module_items(&after);

        assert!(!before_tree.definition_eq(&after_tree));
    }

    #[test]
    fn signature_item_sets_track_their_own_declarations() {
        let (before, before_errors) = parse_module(
            "pub struct S { value: i32 } static VALUE: i32 = 1; fn helper() i32 { 1 }",
        );
        let (after, after_errors) =
            parse_module("pub struct S { value: i32 } static VALUE: i32 = 1; fn helper() u8 { 1 }");
        assert!(before_errors.is_empty(), "{before_errors:?}");
        assert!(after_errors.is_empty(), "{after_errors:?}");

        let before_active = lower_module_items(&before).all_items_active();
        let after_active = lower_module_items(&after).all_items_active();

        assert!(
            !before_active
                .signature_items(SignatureItemSet::Functions)
                .declaration_eq(&after_active.signature_items(SignatureItemSet::Functions))
        );
        assert!(
            before_active
                .signature_items(SignatureItemSet::ExtensionFunctions)
                .declaration_eq(
                    &after_active.signature_items(SignatureItemSet::ExtensionFunctions)
                )
        );
        assert!(
            before_active
                .signature_items(SignatureItemSet::Values)
                .declaration_eq(&after_active.signature_items(SignatureItemSet::Values))
        );
        assert!(
            before_active
                .signature_items(SignatureItemSet::Types)
                .declaration_eq(&after_active.signature_items(SignatureItemSet::Types))
        );
        assert!(
            before_active
                .signature_items(SignatureItemSet::Traits)
                .declaration_eq(&after_active.signature_items(SignatureItemSet::Traits))
        );
    }

    #[test]
    fn source_revision_changes_do_not_change_item_tree_declaration_shape() {
        let source = "pub fn main(value: i32) i32 { value }";
        let source_id = SourceId::isolated();
        let before_tree = parse_versioned_item_tree(source, source_id, SourceRevision::INITIAL);
        let after_tree = parse_versioned_item_tree(source, source_id, SourceRevision(1));

        assert_ne!(before_tree, after_tree);
        assert!(before_tree.declaration_eq(&after_tree));
    }

    #[test]
    fn body_change_with_new_source_revision_keeps_function_declaration_shape() {
        let source_id = SourceId::isolated();
        let before_tree = parse_versioned_item_tree(
            "pub fn main() i32 { 0 }",
            source_id,
            SourceRevision::INITIAL,
        );
        let after_tree =
            parse_versioned_item_tree("pub fn main() i32 { 1 }", source_id, SourceRevision(1));

        assert_ne!(before_tree, after_tree);
        assert!(before_tree.declaration_eq(&after_tree));
    }

    #[test]
    fn attribute_layout_and_source_revision_do_not_change_declaration_shape() {
        let source_id = SourceId::isolated();
        let before_tree = parse_versioned_item_tree(
            "@[if true]\n@[linkName(\"main\")]\npub fn main() i32 { 0 }",
            source_id,
            SourceRevision::INITIAL,
        );
        let after_tree = parse_versioned_item_tree(
            "\n\n@[if true]\n@[linkName(\"main\")]\npub fn main() i32 { 1 }",
            source_id,
            SourceRevision(1),
        );

        assert_ne!(before_tree, after_tree);
        assert!(before_tree.declaration_eq(&after_tree));
    }

    #[test]
    fn using_spans_and_source_revision_do_not_change_declaration_shape() {
        let source_id = SourceId::isolated();
        let before_tree = parse_versioned_item_tree(
            "using math::{add, sub as minus, Operator::*};",
            source_id,
            SourceRevision::INITIAL,
        );
        let after_tree = parse_versioned_item_tree(
            "\n\nusing math::{ add, sub as minus, Operator::* };",
            source_id,
            SourceRevision(1),
        );

        assert_ne!(before_tree, after_tree);
        assert!(before_tree.declaration_eq(&after_tree));
        assert!(before_tree.definition_eq(&after_tree));
    }

    #[test]
    fn enum_discriminant_spans_and_revision_do_not_change_declaration_shape() {
        let source_id = SourceId::isolated();
        let before_tree = parse_versioned_item_tree(
            "enum Mode: u8 { Zero = 0, One = 1 + 0 }",
            source_id,
            SourceRevision::INITIAL,
        );
        let after_tree = parse_versioned_item_tree(
            "\n\nenum Mode: u8 { Zero = 0, One = 1 + 0 }",
            source_id,
            SourceRevision(1),
        );

        assert_ne!(before_tree, after_tree);
        assert!(before_tree.declaration_eq(&after_tree));
    }

    #[test]
    fn type_formatting_changes_do_not_change_item_tree_declaration_shape() {
        let (before, before_errors) = parse_module(
            r#"
struct Box[T] { value: T }
extend[T] &Box[T] {
    fn get(self) T { self.value }
}
fn main(items: &[Box[i32]]) &Box[i32] { &items[0] }
"#,
        );
        let (after, after_errors) = parse_module(
            r#"
struct Box[ T ] { value: T }
extend[ T ] & Box[ T ] {
    fn get(self) T { self.value }
}
fn main(items: & [ Box[ i32 ] ]) & Box[ i32 ] { &items[0] }
"#,
        );
        assert!(before_errors.is_empty(), "{before_errors:?}");
        assert!(after_errors.is_empty(), "{after_errors:?}");

        let before_tree = lower_module_items(&before);
        let after_tree = lower_module_items(&after);

        assert_ne!(before_tree, after_tree);
        assert!(before_tree.declaration_eq(&after_tree));
    }

    fn parse_versioned_item_tree(
        source: &str,
        source_id: SourceId,
        revision: SourceRevision,
    ) -> ModuleItemTree {
        let version = SourceVersion {
            id: source_id,
            revision,
        };
        let syntax = nia_syntax::parse_source(source, Some(version));
        let (module, errors) = nia_parser::parse_module_syntax(&syntax);
        assert!(errors.is_empty(), "{errors:?}");
        lower_module_items(&module)
    }

    struct BoolResolver;

    impl ConditionResolver for BoolResolver {
        fn resolve_condition(&mut self, cond: &ConditionExpr) -> Result<bool, ItemTreeError> {
            match cond.kind {
                nia_ast::ConditionExprKind::Bool(value) => Ok(value),
                _ => Err(ItemTreeError {
                    span: cond.span,
                    message: "expected bool test condition".to_string(),
                }),
            }
        }
    }
}
