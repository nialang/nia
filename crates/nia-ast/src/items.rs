// SPDX-License-Identifier: GPL-3.0-or-later
use nia_node_id::VersionedNodeKey;
use nia_span::Span;
use nia_symbol::{SymbolId, symbol_identity_key};

use crate::{Block, Expr, PathSegmentKind, TypeRef, WhereClause};

pub use nia_ids::{ReceiverKind, Visibility};

/// Parsed module containing top-level items.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    /// Items in source order.
    pub items: Vec<Item>,
}

/// Top-level item with source identity and visibility.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Source span covering the item.
    pub span: Span,
    /// Stable syntax identity for the item.
    pub node_key: VersionedNodeKey,
    /// Attributes attached to the item.
    pub attributes: Vec<Attribute>,
    /// Item visibility.
    pub vis: Visibility,
    /// Item-specific payload.
    pub kind: ItemKind,
}

/// Parsed item attribute.
#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    /// Attribute payload.
    pub kind: AttributeKind,
    /// Source span covering the attribute.
    pub span: Span,
}

/// Supported item attribute forms.
#[derive(Debug, Clone, PartialEq)]
pub enum AttributeKind {
    /// Conditional compilation expression.
    If(ConditionExpr),
    /// Metadata path and arguments.
    Meta(AttributeMeta),
}

/// Metadata attribute path and expression arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeMeta {
    /// Attribute path segments.
    pub path: Vec<SymbolId>,
    /// Attribute arguments in source order.
    pub args: Vec<Expr>,
}

/// Conditional attribute expression.
#[derive(Debug, Clone, PartialEq)]
pub struct ConditionExpr {
    /// Source span covering the condition.
    pub span: Span,
    /// Condition expression payload.
    pub kind: ConditionExprKind,
}

/// Kinds of conditional attribute expressions.
#[derive(Debug, Clone, PartialEq)]
pub enum ConditionExprKind {
    /// Boolean literal.
    Bool(bool),
    /// Integer literal text.
    Integer(String),
    /// String literal text.
    String(String),
    /// Symbol identifier.
    Ident(SymbolId),
    /// Unary condition operation.
    Unary {
        /// Unary operator.
        op: ConditionUnaryOp,
        /// Operand condition.
        expr: Box<ConditionExpr>,
    },
    /// Binary condition operation.
    Binary {
        /// Left operand.
        lhs: Box<ConditionExpr>,
        /// Binary operator.
        op: ConditionBinaryOp,
        /// Right operand.
        rhs: Box<ConditionExpr>,
    },
}

/// Unary operator in a conditional attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionUnaryOp {
    /// Logical negation.
    Not,
}

/// Binary operators in a conditional attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionBinaryOp {
    /// Equality comparison.
    Eq,
    /// Inequality comparison.
    Ne,
    /// Logical conjunction.
    And,
    /// Logical disjunction.
    Or,
}

/// Top-level item kinds.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    /// Nested module declaration.
    Module(ModuleItem),
    /// Using/import declaration.
    Using(UsingItem),
    /// Struct declaration.
    Struct(StructItem),
    /// Union declaration.
    Union(UnionItem),
    /// Trait declaration.
    Trait(TraitItem),
    /// Extension declaration.
    Extend(ExtendItem),
    /// Enum declaration.
    Enum(EnumItem),
    /// Type alias declaration.
    TypeAlias(TypeAliasItem),
    /// Function declaration.
    Function(FunctionItem),
    /// Const or static binding declaration.
    Binding(BindingItem),
}

/// Nested module declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleItem {
    /// Module name identity.
    pub name: SymbolId,
}

/// Using/import declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct UsingItem {
    /// Host path segments.
    pub host: Vec<UsingHostSegment>,
    /// Selected names or group.
    pub selector: UsingSelector,
}

/// One segment of a using host path.
#[derive(Debug, Clone, PartialEq)]
pub struct UsingHostSegment {
    /// Path segment kind.
    pub kind: PathSegmentKind,
    /// Source span covering the segment.
    pub span: Span,
}

/// Selection form in a using declaration.
#[derive(Debug, Clone, PartialEq)]
pub enum UsingSelector {
    /// One imported name.
    Single(UsingName),
    /// Grouped imports.
    Group(Vec<UsingGroupItem>),
    /// Wildcard import.
    Wildcard {
        /// Source span of the wildcard token.
        span: Span,
    },
    /// Import of the host's self name.
    SelfName,
}

/// Entry in a grouped using selector.
#[derive(Debug, Clone, PartialEq)]
pub enum UsingGroupItem {
    /// Direct imported name.
    Name(UsingName),
    /// Nested host and selector.
    Nested {
        /// Nested host path.
        host: Vec<UsingHostSegment>,
        /// Nested selector.
        selector: Box<UsingSelector>,
    },
}

/// Imported name with an optional alias.
#[derive(Debug, Clone, PartialEq)]
pub struct UsingName {
    /// Imported symbol identity.
    pub name: SymbolId,
    /// Span of the imported name.
    pub name_span: Span,
    /// Optional local alias.
    pub alias: Option<SymbolId>,
    /// Span of the alias.
    pub alias_span: Option<Span>,
}

/// Struct declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct StructItem {
    /// Struct name identity.
    pub name: SymbolId,
    /// Generic parameters.
    pub generics: Vec<GenericParam>,
    /// Where-clause predicates.
    pub where_clause: WhereClause,
    /// Declared fields.
    pub fields: Vec<Field>,
    /// Positional fields use tuple construction and projection syntax.
    pub is_tuple: bool,
    /// Whether the declaration uses external ABI layout.
    pub is_extern: bool,
}

/// Union declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct UnionItem {
    /// Union name identity.
    pub name: SymbolId,
    /// Generic parameters.
    pub generics: Vec<GenericParam>,
    /// Where-clause predicates.
    pub where_clause: WhereClause,
    /// Declared fields.
    pub fields: Vec<Field>,
    /// Whether the declaration uses external ABI layout.
    pub is_extern: bool,
}

/// Trait declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitItem {
    /// Trait name identity.
    pub name: SymbolId,
    /// Generic parameters.
    pub generics: Vec<GenericParam>,
    /// Supertrait type references.
    pub supertraits: Vec<TypeRef>,
    /// Where-clause predicates.
    pub where_clause: WhereClause,
    /// Associated type declarations.
    pub associated_types: Vec<TraitAssociatedType>,
    /// Associated value declarations.
    pub associated_values: Vec<TraitAssociatedValue>,
    /// Trait methods.
    pub methods: Vec<TraitMethod>,
}

/// Associated type declaration in a trait.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitAssociatedType {
    /// Associated type name.
    pub name: SymbolId,
    /// Source span.
    pub span: Span,
    /// Stable syntax identity.
    pub node_key: VersionedNodeKey,
}

/// Associated value declaration in a trait.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitAssociatedValue {
    /// Associated value name.
    pub name: SymbolId,
    /// Declared value type.
    pub ty: TypeRef,
    /// Source span.
    pub span: Span,
    /// Stable syntax identity.
    pub node_key: VersionedNodeKey,
}

/// Trait method declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitMethod {
    /// Function signature and body.
    pub function: FunctionItem,
}

/// Trait or inherent extension declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtendItem {
    /// Generic parameters.
    pub generics: Vec<GenericParam>,
    /// Extended target type.
    pub target: TypeRef,
    /// Optional implemented trait.
    pub trait_ref: Option<TypeRef>,
    /// Where-clause predicates.
    pub where_clause: WhereClause,
    /// Associated type definitions.
    pub associated_types: Vec<ExtendAssociatedType>,
    /// Associated value definitions.
    pub associated_values: Vec<ExtendAssociatedValue>,
    /// Extension methods.
    pub methods: Vec<ExtendMethod>,
}

/// Associated type definition in an extension.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtendAssociatedType {
    /// Associated type name.
    pub name: SymbolId,
    /// Defined type.
    pub ty: TypeRef,
    /// Source span.
    pub span: Span,
    /// Stable syntax identity.
    pub node_key: VersionedNodeKey,
}

/// Associated value definition in an extension.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtendAssociatedValue {
    /// Value visibility.
    pub vis: Visibility,
    /// Value binding declaration.
    pub binding: BindingItem,
    /// Source span.
    pub span: Span,
}

/// Method definition in an extension.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtendMethod {
    /// Method visibility.
    pub vis: Visibility,
    /// Method function declaration.
    pub function: FunctionItem,
}

/// Struct, union, or enum field declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// Field name identity.
    pub name: SymbolId,
    /// Field type syntax.
    pub ty: TypeRef,
    /// Field attributes.
    pub attributes: Vec<Attribute>,
    /// Source span.
    pub span: Span,
    /// Stable syntax identity.
    pub node_key: VersionedNodeKey,
}

/// Enum declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumItem {
    /// Enum name identity.
    pub name: SymbolId,
    /// Optional explicit backing type.
    pub backing_type: Option<TypeRef>,
    /// Whether downstream extensions may add variants.
    pub is_open: bool,
    /// Declared variants.
    pub variants: Vec<EnumVariant>,
}

/// One enum variant declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariant {
    /// Variant name identity.
    pub name: SymbolId,
    /// Variant payload shape.
    pub payload: EnumVariantPayload,
    /// Optional explicit discriminant expression.
    pub value: Option<Expr>,
    /// Source span.
    pub span: Span,
    /// Stable syntax identity.
    pub node_key: VersionedNodeKey,
}

/// Payload shape of an enum variant.
#[derive(Debug, Clone, PartialEq)]
pub enum EnumVariantPayload {
    /// Variant with no payload.
    Unit,
    /// Positional payload types.
    Tuple(Vec<TypeRef>),
    /// Named payload fields.
    Named(Vec<Field>),
}

/// Type alias declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeAliasItem {
    /// Alias name identity.
    pub name: SymbolId,
    /// Generic parameters.
    pub generics: Vec<GenericParam>,
    /// Where-clause predicates.
    pub where_clause: WhereClause,
    /// Optional aliased type for declaration-only builtin aliases.
    pub ty: Option<TypeRef>,
}

/// Function, method, or callable declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionItem {
    /// Function name identity.
    pub name: SymbolId,
    /// Generic parameters.
    pub generics: Vec<GenericParam>,
    /// Where-clause predicates.
    pub where_clause: WhereClause,
    /// Function parameters.
    pub params: Vec<Param>,
    /// Optional return type; omission denotes the language default.
    pub return_type: Option<TypeRef>,
    /// Optional body for declaration-only functions.
    pub body: Option<Block>,
    /// Whether the function has external linkage.
    pub is_extern: bool,
    /// Whether the function is const-evaluable.
    pub is_const: bool,
    /// Whether the function accepts variadic arguments.
    pub is_variadic: bool,
    /// Source span.
    pub span: Span,
    /// Stable syntax identity.
    pub node_key: VersionedNodeKey,
}

/// Generic type or const parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct GenericParam {
    /// Parameter name identity.
    pub name: SymbolId,
    /// Span of the parameter name.
    pub name_span: Span,
    /// Type or const parameter payload.
    pub kind: GenericParamKind,
}

/// Kind of generic parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum GenericParamKind {
    /// Generic type parameter.
    Type,
    /// Generic const parameter with an explicit type.
    Const {
        /// Declared const parameter type.
        ty: TypeRef,
    },
}

impl GenericParam {
    /// Creates a generic type parameter.
    pub fn type_param(name: SymbolId, name_span: Span) -> Self {
        Self {
            name,
            name_span,
            kind: GenericParamKind::Type,
        }
    }

    /// Creates a generic const parameter.
    pub fn const_param(name: SymbolId, name_span: Span, ty: TypeRef) -> Self {
        Self {
            name,
            name_span,
            kind: GenericParamKind::Const { ty },
        }
    }

    /// Reports whether this is a type parameter.
    pub fn is_type(&self) -> bool {
        matches!(self.kind, GenericParamKind::Type)
    }

    /// Reports whether this is a const parameter.
    pub fn is_const(&self) -> bool {
        matches!(self.kind, GenericParamKind::Const { .. })
    }
}

/// Returns generic parameter names in declaration order.
pub fn generic_param_names(generics: &[GenericParam]) -> Vec<SymbolId> {
    generics.iter().map(|generic| generic.name).collect()
}

/// Returns stable identities for generic parameters in declaration order.
pub fn generic_param_identities(generics: &[GenericParam]) -> Vec<String> {
    generics.iter().map(generic_param_identity).collect()
}

/// Returns a stable identity including a const parameter's type syntax.
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

/// Function or method parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    /// Receiver mode for a `self` parameter.
    pub receiver: Option<ReceiverKind>,
    /// Optional parameter name.
    pub name: Option<SymbolId>,
    /// Optional parameter type during parse recovery.
    pub ty: Option<TypeRef>,
    /// Source span.
    pub span: Span,
    /// Stable syntax identity.
    pub node_key: VersionedNodeKey,
}

/// Top-level or associated const/static binding.
#[derive(Debug, Clone, PartialEq)]
pub struct BindingItem {
    /// Binding name identity.
    pub name: SymbolId,
    /// Optional explicit binding type.
    pub ty: Option<TypeRef>,
    /// Optional initializer expression.
    pub value: Option<Expr>,
    /// Const or static binding mode.
    pub kind: ItemBindingKind,
    /// Stable syntax identity.
    pub node_key: VersionedNodeKey,
}

/// Kind and mutability/linkage of an item binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemBindingKind {
    /// Immutable compile-time constant.
    Const,
    /// Static storage binding.
    Static {
        /// Whether writes are permitted.
        is_mutable: bool,
        /// Whether storage is defined externally.
        is_extern: bool,
    },
}

impl BindingItem {
    /// Reports whether this binding is a const item.
    pub fn is_const(&self) -> bool {
        matches!(self.kind, ItemBindingKind::Const)
    }

    /// Reports whether this static binding is mutable.
    pub fn is_mutable(&self) -> bool {
        matches!(
            self.kind,
            ItemBindingKind::Static {
                is_mutable: true,
                ..
            }
        )
    }

    /// Reports whether this static binding has external storage.
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

#[cfg(test)]
mod tests {
    use super::*;
    use nia_node_id::{SyntaxKind, VersionedNodeKey};
    use nia_source::{SourceId, SourceRevision, SourceVersion};

    fn type_ref(kind: crate::TypeKind, span: Span) -> TypeRef {
        TypeRef {
            span,
            node_key: VersionedNodeKey::span(
                SourceVersion {
                    id: SourceId(1),
                    revision: SourceRevision::INITIAL,
                },
                SyntaxKind::Type,
                span,
            ),
            text: String::new(),
            kind,
        }
    }

    fn binding(kind: ItemBindingKind) -> BindingItem {
        BindingItem {
            name: SymbolId::from_stable_hash(3),
            ty: None,
            value: None,
            kind,
            node_key: VersionedNodeKey::span(
                SourceVersion {
                    id: SourceId(1),
                    revision: SourceRevision::INITIAL,
                },
                SyntaxKind::Item,
                Span::new(0, 1),
            ),
        }
    }

    #[test]
    fn generic_parameter_helpers_preserve_kind_order_and_const_type() {
        let type_name = SymbolId::from_stable_hash(1);
        let const_name = SymbolId::from_stable_hash(2);
        let generics = vec![
            GenericParam::type_param(type_name, Span::new(0, 1)),
            GenericParam::const_param(
                const_name,
                Span::new(2, 3),
                type_ref(crate::TypeKind::SelfType, Span::new(5, 9)),
            ),
        ];

        assert!(generics[0].is_type());
        assert!(!generics[0].is_const());
        assert!(generics[1].is_const());
        assert!(!generics[1].is_type());
        assert_eq!(generic_param_names(&generics), vec![type_name, const_name]);
        assert_eq!(
            generic_param_identities(&generics),
            vec![
                symbol_identity_key(type_name),
                format!("{}:self", symbol_identity_key(const_name)),
            ]
        );
    }

    #[test]
    fn item_binding_helpers_distinguish_const_and_static_flags() {
        let constant = binding(ItemBindingKind::Const);
        assert!(constant.is_const());
        assert!(!constant.is_mutable());
        assert!(!constant.is_extern());

        let immutable_static = binding(ItemBindingKind::Static {
            is_mutable: false,
            is_extern: false,
        });
        assert!(!immutable_static.is_const());
        assert!(!immutable_static.is_mutable());
        assert!(!immutable_static.is_extern());

        let external_mutable_static = binding(ItemBindingKind::Static {
            is_mutable: true,
            is_extern: true,
        });
        assert!(!external_mutable_static.is_const());
        assert!(external_mutable_static.is_mutable());
        assert!(external_mutable_static.is_extern());
    }
}
