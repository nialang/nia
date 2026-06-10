// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{
    Attribute, BindingItem, ComptimeIfItem, ComptimeIfItemElse, EnumItem, ExtendItem, FunctionItem,
    Item, ItemKind, Module, ModuleItem, StructItem, TraitItem, TypeAliasItem, UnionItem, UsingItem,
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
    ComptimeIf(ComptimeIfNode),
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
pub struct ComptimeIfNode {
    pub span: Span,
    pub cond: nia_ast::Expr,
    pub then_items: Vec<ItemTreeNode>,
    pub else_branch: Option<ComptimeIfElse>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComptimeBranch {
    Then,
    Else,
    None,
}

pub trait ComptimeBranchResolver {
    fn resolve_comptime_if(
        &mut self,
        span: Span,
        cond: &nia_ast::Expr,
    ) -> Result<ComptimeBranch, ItemTreeError>;
}

#[derive(Debug, Clone, PartialEq)]
pub enum ComptimeIfElse {
    If(Box<ComptimeIfNode>),
    Items(Vec<ItemTreeNode>),
}

impl ModuleItemTree {
    pub fn from_module(module: &Module) -> Self {
        Self {
            items: module.items.iter().map(lower_item).collect(),
        }
    }

    pub fn active_items_without_comptime(&self) -> Vec<ItemTreeNode> {
        let mut items = Vec::new();
        flatten_non_comptime_items(&self.items, &mut items);
        items
    }

    pub fn active_items(
        &self,
        resolver: &mut impl ComptimeBranchResolver,
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
                ItemTreeNodeKind::ComptimeIf(item) => ItemKind::ComptimeIf(item.to_ast_item()),
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

impl ComptimeIfNode {
    fn to_ast_item(&self) -> ComptimeIfItem {
        ComptimeIfItem {
            cond: self.cond.clone(),
            then_items: self
                .then_items
                .iter()
                .map(ItemTreeNode::to_ast_item)
                .collect(),
            else_branch: self.else_branch.as_ref().map(ComptimeIfElse::to_ast_item),
        }
    }
}

impl ComptimeIfElse {
    fn to_ast_item(&self) -> ComptimeIfItemElse {
        match self {
            Self::If(item) => ComptimeIfItemElse::If(Box::new(item.to_ast_item())),
            Self::Items(items) => {
                ComptimeIfItemElse::Items(items.iter().map(ItemTreeNode::to_ast_item).collect())
            }
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
            ItemKind::ComptimeIf(comptime_if) => {
                ItemTreeNodeKind::ComptimeIf(lower_comptime_if(comptime_if))
            }
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

fn lower_comptime_if(comptime_if: &ComptimeIfItem) -> ComptimeIfNode {
    ComptimeIfNode {
        span: comptime_if.cond.span,
        cond: comptime_if.cond.clone(),
        then_items: comptime_if.then_items.iter().map(lower_item).collect(),
        else_branch: comptime_if.else_branch.as_ref().map(lower_comptime_if_else),
    }
}

fn lower_comptime_if_else(else_branch: &ComptimeIfItemElse) -> ComptimeIfElse {
    match else_branch {
        ComptimeIfItemElse::If(comptime_if) => {
            ComptimeIfElse::If(Box::new(lower_comptime_if(comptime_if)))
        }
        ComptimeIfItemElse::Items(items) => {
            ComptimeIfElse::Items(items.iter().map(lower_item).collect())
        }
    }
}

fn flatten_non_comptime_items(items: &[ItemTreeNode], out: &mut Vec<ItemTreeNode>) {
    for item in items {
        if !matches!(item.kind, ItemTreeNodeKind::ComptimeIf(_)) {
            out.push(item.clone());
        }
    }
}

fn collect_active_items(
    items: &[ItemTreeNode],
    resolver: &mut impl ComptimeBranchResolver,
    active: &mut ActiveModuleItemTree,
) -> Result<(), ItemTreeError> {
    for item in items {
        match &item.kind {
            ItemTreeNodeKind::ComptimeIf(comptime_if) => {
                collect_active_comptime_if(comptime_if, resolver, active)?;
            }
            _ => active.items.push(item.clone()),
        }
    }
    Ok(())
}

fn collect_active_comptime_if(
    comptime_if: &ComptimeIfNode,
    resolver: &mut impl ComptimeBranchResolver,
    active: &mut ActiveModuleItemTree,
) -> Result<(), ItemTreeError> {
    match resolver.resolve_comptime_if(comptime_if.span, &comptime_if.cond)? {
        ComptimeBranch::Then => {
            mark_inactive_else(&comptime_if.else_branch, active);
            collect_active_items(&comptime_if.then_items, resolver, active)
        }
        ComptimeBranch::Else => {
            mark_inactive_items(&comptime_if.then_items, active);
            match &comptime_if.else_branch {
                Some(ComptimeIfElse::If(nested)) => {
                    collect_active_comptime_if(nested, resolver, active)
                }
                Some(ComptimeIfElse::Items(items)) => collect_active_items(items, resolver, active),
                None => Ok(()),
            }
        }
        ComptimeBranch::None => {
            mark_inactive_items(&comptime_if.then_items, active);
            mark_inactive_else(&comptime_if.else_branch, active);
            Ok(())
        }
    }
}

fn mark_inactive_else(else_branch: &Option<ComptimeIfElse>, active: &mut ActiveModuleItemTree) {
    match else_branch {
        Some(ComptimeIfElse::If(nested)) => mark_inactive_comptime_if(nested, active),
        Some(ComptimeIfElse::Items(items)) => mark_inactive_items(items, active),
        None => {}
    }
}

fn mark_inactive_comptime_if(comptime_if: &ComptimeIfNode, active: &mut ActiveModuleItemTree) {
    active.inactive_spans.insert(comptime_if.span);
    mark_inactive_items(&comptime_if.then_items, active);
    mark_inactive_else(&comptime_if.else_branch, active);
}

fn mark_inactive_items(items: &[ItemTreeNode], active: &mut ActiveModuleItemTree) {
    for item in items {
        active.inactive_spans.insert(item.span);
        if let ItemTreeNodeKind::ComptimeIf(comptime_if) = &item.kind {
            mark_inactive_comptime_if(comptime_if, active);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_parser::parse_module;

    #[test]
    fn keeps_comptime_if_as_tree_node() {
        let (module, errors) = parse_module(
            r#"
comptime if true {
    fn selected() i32 { 1 }
} else {
    fn other() i32 { 0 }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let tree = lower_module_items(&module);
        assert_eq!(tree.items.len(), 1);
        let ItemTreeNodeKind::ComptimeIf(node) = &tree.items[0].kind else {
            panic!("expected comptime if node");
        };
        assert_eq!(node.then_items.len(), 1);
        assert!(matches!(node.else_branch, Some(ComptimeIfElse::Items(_))));
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
        assert_eq!(tree.items[0].attributes[0].path, vec!["link_name"]);

        let projected = ActiveModuleItemTree::new(tree.items.clone(), HashSet::new()).to_module();
        assert_eq!(projected.items[0].attributes.len(), 1);
        assert_eq!(projected.items[0].attributes[0].path, vec!["link_name"]);
    }

    #[test]
    fn resolves_active_items_through_resolver() {
        let (module, errors) = parse_module(
            r#"
fn always() i32 { 1 }
comptime if false {
    fn skipped() i32 { 0 }
} else {
    fn selected() i32 { 2 }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let tree = lower_module_items(&module);
        let active = tree.active_items(&mut BoolResolver(false)).unwrap();
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

    struct BoolResolver(bool);

    impl ComptimeBranchResolver for BoolResolver {
        fn resolve_comptime_if(
            &mut self,
            span: Span,
            _cond: &nia_ast::Expr,
        ) -> Result<ComptimeBranch, ItemTreeError> {
            let _ = span;
            Ok(if self.0 {
                ComptimeBranch::Then
            } else {
                ComptimeBranch::Else
            })
        }
    }
}
