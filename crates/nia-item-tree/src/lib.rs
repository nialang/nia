// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{
    Attribute, AttributeKind, BindingItem, ConditionExpr, EnumItem, ExtendItem, FunctionItem, Item,
    ItemKind, Module, ModuleItem, StructItem, TraitItem, TypeAliasItem, UnionItem, UsingItem,
    Visibility,
};
use nia_node_id::NodeKey;
use nia_span::Span;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleItemTree {
    pub items: Vec<ItemTreeNode>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemTreeNode {
    pub span: Span,
    pub node_key: NodeKey,
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

fn item_tree_node_declaration_eq(lhs: &ItemTreeNode, rhs: &ItemTreeNode) -> bool {
    lhs.node_key == rhs.node_key
        && item_tree_span_declaration_eq(lhs, rhs)
        && lhs.attributes == rhs.attributes
        && lhs.visibility == rhs.visibility
        && item_tree_node_kind_declaration_eq(&lhs.kind, &rhs.kind)
}

fn item_tree_node_kind_declaration_eq(lhs: &ItemTreeNodeKind, rhs: &ItemTreeNodeKind) -> bool {
    match (lhs, rhs) {
        (ItemTreeNodeKind::Module(lhs), ItemTreeNodeKind::Module(rhs)) => lhs == rhs,
        (ItemTreeNodeKind::Using(lhs), ItemTreeNodeKind::Using(rhs)) => lhs == rhs,
        (ItemTreeNodeKind::Struct(lhs), ItemTreeNodeKind::Struct(rhs)) => lhs == rhs,
        (ItemTreeNodeKind::Union(lhs), ItemTreeNodeKind::Union(rhs)) => lhs == rhs,
        (ItemTreeNodeKind::Trait(lhs), ItemTreeNodeKind::Trait(rhs)) => trait_decl_eq(lhs, rhs),
        (ItemTreeNodeKind::Extend(lhs), ItemTreeNodeKind::Extend(rhs)) => extend_decl_eq(lhs, rhs),
        (ItemTreeNodeKind::Enum(lhs), ItemTreeNodeKind::Enum(rhs)) => lhs == rhs,
        (ItemTreeNodeKind::TypeAlias(lhs), ItemTreeNodeKind::TypeAlias(rhs)) => lhs == rhs,
        (ItemTreeNodeKind::Function(lhs), ItemTreeNodeKind::Function(rhs)) => {
            function_decl_eq(lhs, rhs)
        }
        (ItemTreeNodeKind::Binding(lhs), ItemTreeNodeKind::Binding(rhs)) => lhs == rhs,
        _ => false,
    }
}

fn item_tree_span_declaration_eq(lhs: &ItemTreeNode, rhs: &ItemTreeNode) -> bool {
    if matches!(
        (&lhs.kind, &rhs.kind),
        (ItemTreeNodeKind::Function(_), ItemTreeNodeKind::Function(_))
            | (ItemTreeNodeKind::Trait(_), ItemTreeNodeKind::Trait(_))
            | (ItemTreeNodeKind::Extend(_), ItemTreeNodeKind::Extend(_))
    ) {
        true
    } else {
        lhs.span == rhs.span
    }
}

fn trait_decl_eq(lhs: &TraitItem, rhs: &TraitItem) -> bool {
    lhs.name == rhs.name
        && lhs.generics == rhs.generics
        && lhs.supertraits == rhs.supertraits
        && lhs.where_clause == rhs.where_clause
        && lhs.associated_types == rhs.associated_types
        && lhs.methods.len() == rhs.methods.len()
        && lhs
            .methods
            .iter()
            .zip(rhs.methods.iter())
            .all(|(lhs, rhs)| function_decl_eq(&lhs.function, &rhs.function))
}

fn extend_decl_eq(lhs: &ExtendItem, rhs: &ExtendItem) -> bool {
    lhs.generics == rhs.generics
        && lhs.target == rhs.target
        && lhs.trait_ref == rhs.trait_ref
        && lhs.where_clause == rhs.where_clause
        && lhs.associated_types == rhs.associated_types
        && lhs.associated_values == rhs.associated_values
        && lhs.methods.len() == rhs.methods.len()
        && lhs
            .methods
            .iter()
            .zip(rhs.methods.iter())
            .all(|(lhs, rhs)| lhs.vis == rhs.vis && function_decl_eq(&lhs.function, &rhs.function))
}

fn function_decl_eq(lhs: &FunctionItem, rhs: &FunctionItem) -> bool {
    lhs.name == rhs.name
        && lhs.generics == rhs.generics
        && lhs.where_clause == rhs.where_clause
        && lhs.params == rhs.params
        && lhs.return_type == rhs.return_type
        && lhs.is_extern == rhs.is_extern
        && lhs.is_comptime == rhs.is_comptime
        && lhs.is_variadic == rhs.is_variadic
        && lhs.node_key == rhs.node_key
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
