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
