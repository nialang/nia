// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{
    Attribute, AttributeKind, BindingItem, ConditionExpr, EnumItem, EnumVariant,
    ExtendAssociatedType, ExtendAssociatedValue, ExtendItem, ExtendMethod, Field, FunctionItem,
    Item, ItemKind, Module, ModuleItem, Param, StructItem, TraitAssociatedType,
    TraitAssociatedValue, TraitItem, TraitMethod, TypeAliasItem, UnionItem, UsingItem, Visibility,
    option_type_ref_decl_eq, type_ref_decl_eq, type_refs_decl_eq, where_clause_decl_eq,
};
use nia_node_id::VersionedNodeKey;
use nia_span::Span;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignatureItemSet {
    Functions,
    ExtensionFunctions,
    Values,
    Types,
    Traits,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleItemTree {
    pub items: Vec<ItemTreeNode>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemTreeNode {
    pub span: Span,
    pub node_key: VersionedNodeKey,
    pub attributes: Vec<Attribute>,
    pub visibility: Visibility,
    pub kind: ItemTreeNodeKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemTreeNodeKind {
    Module(ModuleItem),
    Using(UsingItem),
    Struct(StructItem),
    Union(UnionItem),
    Trait(TraitItem),
    Extend(ExtendItem),
    Enum(EnumItem),
    TypeAlias(TypeAliasItem),
    Function(FunctionItem),
    Binding(BindingItem),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActiveModuleItemTree {
    pub items: Vec<ItemTreeNode>,
    pub inactive_spans: HashSet<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemTreeError {
    pub span: Span,
    pub message: String,
}

pub trait ConditionResolver {
    fn resolve_condition(&mut self, cond: &ConditionExpr) -> Result<bool, ItemTreeError>;
}

impl ModuleItemTree {
    pub fn from_module(module: &Module) -> Self {
        Self {
            items: module.items.iter().map(lower_item).collect(),
        }
    }

    pub fn active_items_without_comptime(&self) -> Vec<ItemTreeNode> {
        self.items.clone()
    }

    pub fn declaration_eq(&self, other: &Self) -> bool {
        item_tree_nodes_declaration_eq(&self.items, &other.items)
    }

    pub fn definition_eq(&self, other: &Self) -> bool {
        item_tree_nodes_definition_eq(&self.items, &other.items)
    }

    pub fn active_items(
        &self,
        resolver: &mut impl ConditionResolver,
    ) -> Result<ActiveModuleItemTree, ItemTreeError> {
        let mut active = ActiveModuleItemTree {
            items: Vec::new(),
            inactive_spans: HashSet::new(),
        };
        collect_active_items(&self.items, resolver, &mut active)?;
        Ok(active)
    }
}

impl ActiveModuleItemTree {
    pub fn new(items: Vec<ItemTreeNode>, inactive_spans: HashSet<Span>) -> Self {
        Self {
            items,
            inactive_spans,
        }
    }

    pub fn to_module(&self) -> Module {
        Module {
            items: self.items.iter().map(ItemTreeNode::to_ast_item).collect(),
        }
    }

    pub fn declaration_eq(&self, other: &Self) -> bool {
        self.inactive_spans == other.inactive_spans
            && item_tree_nodes_declaration_eq(&self.items, &other.items)
    }

    pub fn definition_eq(&self, other: &Self) -> bool {
        self.inactive_spans == other.inactive_spans
            && item_tree_nodes_definition_eq(&self.items, &other.items)
    }

    pub fn signature_items(&self, set: SignatureItemSet) -> Self {
        Self {
            items: self
                .items
                .iter()
                .filter_map(|item| signature_item(item, set))
                .collect(),
            inactive_spans: self.inactive_spans.clone(),
        }
    }
}

impl ItemTreeNode {
    pub fn to_ast_item(&self) -> Item {
        Item {
            span: self.span,
            node_key: self.node_key.clone(),
            attributes: self.attributes.clone(),
            vis: self.visibility,
            kind: match &self.kind {
                ItemTreeNodeKind::Module(item) => ItemKind::Module(item.clone()),
                ItemTreeNodeKind::Using(item) => ItemKind::Using(item.clone()),
                ItemTreeNodeKind::Struct(item) => ItemKind::Struct(item.clone()),
                ItemTreeNodeKind::Union(item) => ItemKind::Union(item.clone()),
                ItemTreeNodeKind::Trait(item) => ItemKind::Trait(item.clone()),
                ItemTreeNodeKind::Extend(item) => ItemKind::Extend(item.clone()),
                ItemTreeNodeKind::Enum(item) => ItemKind::Enum(item.clone()),
                ItemTreeNodeKind::TypeAlias(item) => ItemKind::TypeAlias(item.clone()),
                ItemTreeNodeKind::Function(item) => ItemKind::Function(item.clone()),
                ItemTreeNodeKind::Binding(item) => ItemKind::Binding(item.clone()),
            },
        }
    }
}

fn item_tree_nodes_declaration_eq(lhs: &[ItemTreeNode], rhs: &[ItemTreeNode]) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs.iter())
            .all(|(lhs, rhs)| item_tree_node_declaration_eq(lhs, rhs))
}

fn item_tree_nodes_definition_eq(lhs: &[ItemTreeNode], rhs: &[ItemTreeNode]) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs.iter())
            .all(|(lhs, rhs)| item_tree_node_definition_eq(lhs, rhs))
}

fn item_tree_node_declaration_eq(lhs: &ItemTreeNode, rhs: &ItemTreeNode) -> bool {
    item_attributes_declaration_eq(&lhs.attributes, &rhs.attributes)
        && lhs.visibility == rhs.visibility
        && item_tree_node_kind_declaration_eq(&lhs.kind, &rhs.kind)
}

fn item_tree_node_definition_eq(lhs: &ItemTreeNode, rhs: &ItemTreeNode) -> bool {
    item_attributes_definition_eq(&lhs.attributes, &rhs.attributes)
        && lhs.visibility == rhs.visibility
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
            lhs.name == rhs.name && lhs.is_comptime == rhs.is_comptime
        }
        _ => false,
    }
}

fn item_attributes_declaration_eq(lhs: &[Attribute], rhs: &[Attribute]) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs.iter())
            .all(|(lhs, rhs)| lhs.kind == rhs.kind)
}

fn item_attributes_definition_eq(lhs: &[Attribute], rhs: &[Attribute]) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs.iter())
            .all(|(lhs, rhs)| lhs.kind == rhs.kind)
}

fn signature_item(item: &ItemTreeNode, set: SignatureItemSet) -> Option<ItemTreeNode> {
    let kind = match (&item.kind, set) {
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
        | (ItemTreeNodeKind::Extend(_), SignatureItemSet::Types) => return None,
        (ItemTreeNodeKind::Module(item), _) => ItemTreeNodeKind::Module(item.clone()),
        (ItemTreeNodeKind::Using(item), _) => ItemTreeNodeKind::Using(item.clone()),
        (
            ItemTreeNodeKind::Struct(item),
            SignatureItemSet::Functions
            | SignatureItemSet::ExtensionFunctions
            | SignatureItemSet::Types
            | SignatureItemSet::Traits,
        ) => ItemTreeNodeKind::Struct(item.clone()),
        (
            ItemTreeNodeKind::Union(item),
            SignatureItemSet::Functions
            | SignatureItemSet::ExtensionFunctions
            | SignatureItemSet::Types
            | SignatureItemSet::Traits,
        ) => ItemTreeNodeKind::Union(item.clone()),
        (
            ItemTreeNodeKind::Enum(item),
            SignatureItemSet::Functions
            | SignatureItemSet::ExtensionFunctions
            | SignatureItemSet::Types
            | SignatureItemSet::Traits,
        ) => ItemTreeNodeKind::Enum(item.clone()),
        (
            ItemTreeNodeKind::TypeAlias(item),
            SignatureItemSet::Functions
            | SignatureItemSet::ExtensionFunctions
            | SignatureItemSet::Types
            | SignatureItemSet::Traits,
        ) => ItemTreeNodeKind::TypeAlias(item.clone()),
        (ItemTreeNodeKind::Function(item), SignatureItemSet::Functions) => {
            ItemTreeNodeKind::Function(item.clone())
        }
        (ItemTreeNodeKind::Binding(item), SignatureItemSet::Values) => {
            ItemTreeNodeKind::Binding(item.clone())
        }
        (
            ItemTreeNodeKind::Trait(item),
            SignatureItemSet::Functions
            | SignatureItemSet::ExtensionFunctions
            | SignatureItemSet::Traits,
        ) => ItemTreeNodeKind::Trait(item.clone()),
        (
            ItemTreeNodeKind::Extend(item),
            SignatureItemSet::Functions | SignatureItemSet::ExtensionFunctions,
        ) => {
            let mut item = item.clone();
            item.associated_values.clear();
            ItemTreeNodeKind::Extend(item)
        }
        (ItemTreeNodeKind::Extend(item), SignatureItemSet::Values) => {
            if item.associated_values.is_empty() {
                return None;
            }
            let mut item = item.clone();
            item.methods.clear();
            item.associated_types.clear();
            ItemTreeNodeKind::Extend(item)
        }
        (ItemTreeNodeKind::Extend(item), SignatureItemSet::Traits) => {
            ItemTreeNodeKind::Extend(item.clone())
        }
    };
    Some(ItemTreeNode {
        span: item.span,
        node_key: item.node_key.clone(),
        attributes: item.attributes.clone(),
        visibility: item.visibility,
        kind,
    })
}

fn using_decl_eq(lhs: &UsingItem, rhs: &UsingItem) -> bool {
    lhs == rhs
}

fn struct_decl_eq(lhs: &StructItem, rhs: &StructItem) -> bool {
    lhs.name == rhs.name
        && lhs.generics == rhs.generics
        && where_clause_decl_eq(&lhs.where_clause, &rhs.where_clause)
        && fields_decl_eq(&lhs.fields, &rhs.fields)
        && lhs.is_extern == rhs.is_extern
}

fn union_decl_eq(lhs: &UnionItem, rhs: &UnionItem) -> bool {
    lhs.name == rhs.name
        && lhs.generics == rhs.generics
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
        && lhs.generics == rhs.generics
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
    lhs.generics == rhs.generics
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
        && lhs
            .iter()
            .zip(rhs.iter())
            .all(|(lhs, rhs)| lhs.name == rhs.name && lhs.value == rhs.value)
}

fn type_alias_decl_eq(lhs: &TypeAliasItem, rhs: &TypeAliasItem) -> bool {
    lhs.name == rhs.name
        && lhs.generics == rhs.generics
        && where_clause_decl_eq(&lhs.where_clause, &rhs.where_clause)
        && match (&lhs.ty, &rhs.ty) {
            (Some(lhs), Some(rhs)) => type_ref_decl_eq(lhs, rhs),
            (None, None) => true,
            _ => false,
        }
}

fn function_decl_eq(lhs: &FunctionItem, rhs: &FunctionItem) -> bool {
    lhs.name == rhs.name
        && lhs.generics == rhs.generics
        && where_clause_decl_eq(&lhs.where_clause, &rhs.where_clause)
        && params_decl_eq(&lhs.params, &rhs.params)
        && option_type_ref_decl_eq(lhs.return_type.as_ref(), rhs.return_type.as_ref())
        && lhs.is_extern == rhs.is_extern
        && lhs.is_comptime == rhs.is_comptime
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
        && lhs.is_mutable == rhs.is_mutable
        && lhs.is_comptime == rhs.is_comptime
        && lhs.is_extern == rhs.is_extern
}

pub fn lower_module_items(module: &Module) -> ModuleItemTree {
    ModuleItemTree::from_module(module)
}

fn lower_item(item: &Item) -> ItemTreeNode {
    ItemTreeNode {
        span: item.span,
        node_key: item.node_key.clone(),
        attributes: item.attributes.clone(),
        visibility: item.vis,
        kind: match &item.kind {
            ItemKind::Module(module) => ItemTreeNodeKind::Module(module.clone()),
            ItemKind::Using(using) => ItemTreeNodeKind::Using(using.clone()),
            ItemKind::Struct(item_struct) => ItemTreeNodeKind::Struct(item_struct.clone()),
            ItemKind::Union(item_union) => ItemTreeNodeKind::Union(item_union.clone()),
            ItemKind::Trait(item_trait) => ItemTreeNodeKind::Trait(item_trait.clone()),
            ItemKind::Extend(extend) => ItemTreeNodeKind::Extend(extend.clone()),
            ItemKind::Enum(item_enum) => ItemTreeNodeKind::Enum(item_enum.clone()),
            ItemKind::TypeAlias(alias) => ItemTreeNodeKind::TypeAlias(alias.clone()),
            ItemKind::Function(function) => ItemTreeNodeKind::Function(function.clone()),
            ItemKind::Binding(binding) => ItemTreeNodeKind::Binding(binding.clone()),
        },
    }
}

fn collect_active_items(
    items: &[ItemTreeNode],
    resolver: &mut impl ConditionResolver,
    active: &mut ActiveModuleItemTree,
) -> Result<(), ItemTreeError> {
    for item in items {
        if item_is_active(item, resolver)? {
            active.items.push(item.clone());
        } else {
            active.inactive_spans.insert(item.span);
        }
    }
    Ok(())
}

fn item_is_active(
    item: &ItemTreeNode,
    resolver: &mut impl ConditionResolver,
) -> Result<bool, ItemTreeError> {
    for attribute in &item.attributes {
        let AttributeKind::If(cond) = &attribute.kind else {
            continue;
        };
        if !resolver.resolve_condition(cond)? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_parser::parse_module;
    use nia_source::{SourceId, SourceRevision, SourceVersion};

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
    fn preserves_item_attributes_in_tree_nodes_and_ast_projection() {
        let (module, errors) = parse_module(
            r#"
@[link_name("runtime_start")]
pub extern fn start(argc: i32) i32;
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let tree = lower_module_items(&module);
        assert_eq!(tree.items.len(), 1);
        assert_eq!(tree.items[0].attributes.len(), 1);
        assert!(matches!(
            &tree.items[0].attributes[0].kind,
            AttributeKind::Meta(meta) if meta.path == ["link_name"]
        ));

        let projected = ActiveModuleItemTree::new(tree.items.clone(), HashSet::new()).to_module();
        assert_eq!(projected.items[0].attributes.len(), 1);
        assert!(matches!(
            &projected.items[0].attributes[0].kind,
            AttributeKind::Meta(meta) if meta.path == ["link_name"]
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
                ItemTreeNodeKind::Function(function) => Some(function.name.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(names, ["always", "selected"]);
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

        let before_active = ActiveModuleItemTree::new(
            lower_module_items(&before).items,
            std::collections::HashSet::new(),
        );
        let after_active = ActiveModuleItemTree::new(
            lower_module_items(&after).items,
            std::collections::HashSet::new(),
        );

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
        let before_tree = parse_versioned_item_tree(source, SourceRevision::INITIAL);
        let after_tree = parse_versioned_item_tree(source, SourceRevision(1));

        assert_ne!(before_tree, after_tree);
        assert!(before_tree.declaration_eq(&after_tree));
    }

    #[test]
    fn body_change_with_new_source_revision_keeps_function_declaration_shape() {
        let before_tree =
            parse_versioned_item_tree("pub fn main() i32 { 0 }", SourceRevision::INITIAL);
        let after_tree = parse_versioned_item_tree("pub fn main() i32 { 1 }", SourceRevision(1));

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
struct Box[T] { value: T }
extend[T] & Box[ T ] {
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

    fn parse_versioned_item_tree(source: &str, revision: SourceRevision) -> ModuleItemTree {
        let version = SourceVersion {
            id: SourceId(0),
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
