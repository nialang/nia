// SPDX-License-Identifier: GPL-3.0-or-later
use nia_node_id::NodeKey;
use nia_span::Span;

use crate::{Block, Expr, TypeRef, WhereClause};

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub span: Span,
    pub node_key: NodeKey,
    pub attributes: Vec<Attribute>,
    pub vis: Visibility,
    pub kind: ItemKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    pub path: Vec<String>,
    pub args: Vec<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility {
    #[default]
    Private,
    PublicSuper,
    PublicPackage,
    Public,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    Module(ModuleItem),
    Using(UsingItem),
    ComptimeIf(ComptimeIfItem),
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
pub struct ComptimeIfItem {
    pub cond: Expr,
    pub then_items: Vec<Item>,
    pub else_branch: Option<ComptimeIfItemElse>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ComptimeIfItemElse {
    If(Box<ComptimeIfItem>),
    Items(Vec<Item>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleItem {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsingItem {
    pub host: Vec<UsingHostSegment>,
    pub selector: UsingSelector,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsingHostSegment {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UsingSelector {
    Single(UsingName),
    Group(Vec<UsingGroupItem>),
    Wildcard { span: Span },
    SelfName,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UsingGroupItem {
    Name(UsingName),
    Nested {
        host: Vec<UsingHostSegment>,
        selector: Box<UsingSelector>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsingName {
    pub name: String,
    pub name_span: Span,
    pub alias: Option<String>,
    pub alias_span: Option<Span>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructItem {
    pub name: String,
    pub generics: Vec<String>,
    pub where_clause: WhereClause,
    pub fields: Vec<Field>,
    pub is_extern: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnionItem {
    pub name: String,
    pub generics: Vec<String>,
    pub where_clause: WhereClause,
    pub fields: Vec<Field>,
    pub is_extern: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitItem {
    pub name: String,
    pub generics: Vec<String>,
    pub supertraits: Vec<TypeRef>,
    pub where_clause: WhereClause,
    pub associated_types: Vec<TraitAssociatedType>,
    pub methods: Vec<TraitMethod>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitAssociatedType {
    pub name: String,
    pub span: Span,
    pub node_key: NodeKey,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitMethod {
    pub function: FunctionItem,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtendItem {
    pub generics: Vec<String>,
    pub target: TypeRef,
    pub trait_ref: Option<TypeRef>,
    pub where_clause: WhereClause,
    pub associated_types: Vec<ExtendAssociatedType>,
    pub associated_values: Vec<ExtendAssociatedValue>,
    pub methods: Vec<ExtendMethod>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtendAssociatedType {
    pub name: String,
    pub ty: TypeRef,
    pub span: Span,
    pub node_key: NodeKey,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtendAssociatedValue {
    pub vis: Visibility,
    pub binding: BindingItem,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtendMethod {
    pub vis: Visibility,
    pub function: FunctionItem,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: TypeRef,
    pub attributes: Vec<Attribute>,
    pub span: Span,
    pub node_key: NodeKey,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumItem {
    pub name: String,
    pub backing_type: Option<TypeRef>,
    pub is_open: bool,
    pub variants: Vec<EnumVariant>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariant {
    pub name: String,
    pub value: Option<Expr>,
    pub span: Span,
    pub node_key: NodeKey,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeAliasItem {
    pub name: String,
    pub generics: Vec<String>,
    pub where_clause: WhereClause,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionItem {
    pub name: String,
    pub generics: Vec<String>,
    pub where_clause: WhereClause,
    pub params: Vec<Param>,
    pub return_type: Option<TypeRef>,
    pub body: Option<Block>,
    pub is_extern: bool,
    pub is_comptime: bool,
    pub is_variadic: bool,
    pub span: Span,
    pub node_key: NodeKey,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub receiver: Option<ReceiverKind>,
    pub name: Option<String>,
    pub ty: Option<TypeRef>,
    pub span: Span,
    pub node_key: NodeKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiverKind {
    RefReadOnly,
    Ref,
    Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BindingItem {
    pub name: String,
    pub ty: Option<TypeRef>,
    pub value: Option<Expr>,
    pub is_let: bool,
    pub is_comptime: bool,
    pub is_extern: bool,
    pub node_key: NodeKey,
}
