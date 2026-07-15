// SPDX-License-Identifier: GPL-3.0-or-later
use nia_node_id::VersionedNodeKey;
use nia_span::Span;
use nia_symbol::{SymbolId, symbol_identity_key};

use crate::{Block, Expr, PathSegmentKind, TypeRef, WhereClause};

pub use nia_ids::{ReceiverKind, Visibility};

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub span: Span,
    pub node_key: VersionedNodeKey,
    pub attributes: Vec<Attribute>,
    pub vis: Visibility,
    pub kind: ItemKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    pub kind: AttributeKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AttributeKind {
    If(ConditionExpr),
    Meta(AttributeMeta),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttributeMeta {
    pub path: Vec<SymbolId>,
    pub args: Vec<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConditionExpr {
    pub span: Span,
    pub kind: ConditionExprKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConditionExprKind {
    Bool(bool),
    Integer(String),
    String(String),
    Ident(SymbolId),
    Unary {
        op: ConditionUnaryOp,
        expr: Box<ConditionExpr>,
    },
    Binary {
        lhs: Box<ConditionExpr>,
        op: ConditionBinaryOp,
        rhs: Box<ConditionExpr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionUnaryOp {
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionBinaryOp {
    Eq,
    Ne,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
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
pub struct ModuleItem {
    pub name: SymbolId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsingItem {
    pub host: Vec<UsingHostSegment>,
    pub selector: UsingSelector,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsingHostSegment {
    pub kind: PathSegmentKind,
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
    pub name: SymbolId,
    pub name_span: Span,
    pub alias: Option<SymbolId>,
    pub alias_span: Option<Span>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructItem {
    pub name: SymbolId,
    pub generics: Vec<GenericParam>,
    pub where_clause: WhereClause,
    pub fields: Vec<Field>,
    pub is_extern: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnionItem {
    pub name: SymbolId,
    pub generics: Vec<GenericParam>,
    pub where_clause: WhereClause,
    pub fields: Vec<Field>,
    pub is_extern: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitItem {
    pub name: SymbolId,
    pub generics: Vec<GenericParam>,
    pub supertraits: Vec<TypeRef>,
    pub where_clause: WhereClause,
    pub associated_types: Vec<TraitAssociatedType>,
    pub associated_values: Vec<TraitAssociatedValue>,
    pub methods: Vec<TraitMethod>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitAssociatedType {
    pub name: SymbolId,
    pub span: Span,
    pub node_key: VersionedNodeKey,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitAssociatedValue {
    pub name: SymbolId,
    pub ty: TypeRef,
    pub span: Span,
    pub node_key: VersionedNodeKey,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitMethod {
    pub function: FunctionItem,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtendItem {
    pub generics: Vec<GenericParam>,
    pub target: TypeRef,
    pub trait_ref: Option<TypeRef>,
    pub where_clause: WhereClause,
    pub associated_types: Vec<ExtendAssociatedType>,
    pub associated_values: Vec<ExtendAssociatedValue>,
    pub methods: Vec<ExtendMethod>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtendAssociatedType {
    pub name: SymbolId,
    pub ty: TypeRef,
    pub span: Span,
    pub node_key: VersionedNodeKey,
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
    pub name: SymbolId,
    pub ty: TypeRef,
    pub attributes: Vec<Attribute>,
    pub span: Span,
    pub node_key: VersionedNodeKey,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumItem {
    pub name: SymbolId,
    pub backing_type: Option<TypeRef>,
    pub is_open: bool,
    pub variants: Vec<EnumVariant>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariant {
    pub name: SymbolId,
    pub value: Option<Expr>,
    pub span: Span,
    pub node_key: VersionedNodeKey,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeAliasItem {
    pub name: SymbolId,
    pub generics: Vec<GenericParam>,
    pub where_clause: WhereClause,
    pub ty: Option<TypeRef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionItem {
    pub name: SymbolId,
    pub generics: Vec<GenericParam>,
    pub where_clause: WhereClause,
    pub params: Vec<Param>,
    pub return_type: Option<TypeRef>,
    pub body: Option<Block>,
    pub is_extern: bool,
    pub is_const: bool,
    pub is_variadic: bool,
    pub span: Span,
    pub node_key: VersionedNodeKey,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenericParam {
    pub name: SymbolId,
    pub name_span: Span,
    pub kind: GenericParamKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GenericParamKind {
    Type,
    Const { ty: TypeRef },
}

impl GenericParam {
    pub fn type_param(name: SymbolId, name_span: Span) -> Self {
        Self {
            name,
            name_span,
            kind: GenericParamKind::Type,
        }
    }

    pub fn const_param(name: SymbolId, name_span: Span, ty: TypeRef) -> Self {
        Self {
            name,
            name_span,
            kind: GenericParamKind::Const { ty },
        }
    }

    pub fn is_type(&self) -> bool {
        matches!(self.kind, GenericParamKind::Type)
    }

    pub fn is_const(&self) -> bool {
        matches!(self.kind, GenericParamKind::Const { .. })
    }
}

pub fn generic_param_names(generics: &[GenericParam]) -> Vec<SymbolId> {
    generics.iter().map(|generic| generic.name).collect()
}

pub fn generic_param_identities(generics: &[GenericParam]) -> Vec<String> {
    generics.iter().map(generic_param_identity).collect()
}

pub fn generic_param_identity(generic: &GenericParam) -> String {
    match &generic.kind {
        GenericParamKind::Type => symbol_identity_key(generic.name),
        GenericParamKind::Const { ty } => {
            format!(
                "{}:{}",
                symbol_identity_key(generic.name),
                crate::type_ref_identity(ty)
            )
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub receiver: Option<ReceiverKind>,
    pub name: Option<SymbolId>,
    pub ty: Option<TypeRef>,
    pub span: Span,
    pub node_key: VersionedNodeKey,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BindingItem {
    pub name: SymbolId,
    pub ty: Option<TypeRef>,
    pub value: Option<Expr>,
    pub kind: ItemBindingKind,
    pub node_key: VersionedNodeKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemBindingKind {
    Const,
    Static { is_mutable: bool, is_extern: bool },
}

impl BindingItem {
    pub fn is_const(&self) -> bool {
        matches!(self.kind, ItemBindingKind::Const)
    }

    pub fn is_mutable(&self) -> bool {
        matches!(
            self.kind,
            ItemBindingKind::Static {
                is_mutable: true,
                ..
            }
        )
    }

    pub fn is_extern(&self) -> bool {
        matches!(
            self.kind,
            ItemBindingKind::Static {
                is_extern: true,
                ..
            }
        )
    }
}
