// SPDX-License-Identifier: GPL-3.0-or-later
//! Structural vocabulary of the lossless syntax tree.
//!
//! Every grammar production that the AST lowering consumes has one dedicated
//! node kind. Tokens keep their lexical kind, trivia keeps its own kinds, and
//! recovery is represented only by zero-width [`SyntaxKind::Missing`] nodes and
//! retained [`SyntaxKind::Error`] regions.

use nia_lexer::TokenKind;
use nia_node_id::SyntaxKind as NodeSyntaxKind;

/// Structural or lexical kind represented by a green tree element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxKind {
    /// Root node covering the complete source.
    SourceFile,
    /// Explicit zero-width construct inserted by recovery.
    Missing,
    /// Malformed source region retained by recovery.
    Error,

    /// Top-level item: attributes, visibility, and one declaration.
    Item,
    /// `@[path(args)]` or `@[if condition]` attribute.
    Attribute,
    /// Parenthesized attribute argument list.
    AttributeArgList,
    /// Binary `and`/`or`/`==`/`!=` conditional-compilation expression.
    ConditionBinary,
    /// `not` conditional-compilation expression.
    ConditionNot,
    /// Parenthesized conditional-compilation expression.
    ConditionParen,
    /// Literal or name conditional-compilation operand.
    ConditionAtom,
    /// `pub`, `pub(super)`, or `pub(pkg)` visibility.
    Visibility,
    /// `module name;` declaration.
    ModuleDecl,
    /// `using ...;` declaration.
    UsingDecl,
    /// Host path and selector of a `using` declaration or nested group entry.
    UsingTree,
    /// Braced `using` selector group.
    UsingGroup,
    /// Single `using` selector with an optional alias.
    UsingName,
    /// Struct declaration.
    StructDecl,
    /// Union declaration.
    UnionDecl,
    /// Enum declaration.
    EnumDecl,
    /// Trait declaration.
    TraitDecl,
    /// Extension declaration.
    ExtendDecl,
    /// Type alias declaration.
    TypeAliasDecl,
    /// Function or method declaration.
    FunctionDecl,
    /// `const` or `static` binding declaration.
    BindingDecl,
    /// Braced named-field list.
    FieldList,
    /// Named aggregate field.
    Field,
    /// Parenthesized positional-field list.
    TupleFieldList,
    /// Positional aggregate field.
    TupleField,
    /// Braced enum variant list.
    VariantList,
    /// Enum variant.
    Variant,
    /// `_` open-enum marker.
    OpenVariant,
    /// `: A + B` trait supertrait list.
    SupertraitList,
    /// Braced trait or extension member list.
    MemberList,
    /// Trait or extension member with attributes and visibility.
    Member,
    /// Associated type declaration or definition.
    AssocTypeDecl,
    /// Trait associated const declaration.
    AssocConstDecl,
    /// `[T, N: usize]` generic parameter list.
    GenericParamList,
    /// Generic type or const parameter.
    GenericParam,
    /// `where` clause.
    WhereClause,
    /// `T: A + B` where predicate.
    WherePredicate,
    /// Parenthesized function parameter list.
    ParamList,
    /// Function, method, or closure parameter.
    Param,

    /// Path type.
    PathType,
    /// Path segment with optional type arguments.
    PathSegment,
    /// `[...]` type argument list.
    TypeArgList,
    /// Type argument.
    TypeArg,
    /// Const expression argument.
    ConstArg,
    /// Path argument whose type or const meaning is decided semantically.
    TypeOrConstArg,
    /// `Name = Type` associated type binding argument.
    AssocBindingArg,
    /// `[T as Trait]::Name` projection type.
    ProjectionType,
    /// `&T` or `&mut T` pointer type.
    PointerType,
    /// `^T` or `^mut T` volatile pointer type.
    VolatilePointerType,
    /// `&[T]` or `&mut [T]` slice type.
    SliceType,
    /// Unsized `[T]` slice pointee type.
    SlicePointeeType,
    /// `[T; N]` array type.
    ArrayType,
    /// Tuple type, including the unit type.
    TupleType,
    /// Parenthesized type.
    ParenType,
    /// Range type.
    RangeType,
    /// `&fn(...) R` function pointer type.
    FnPointerType,
    /// `Fn(...) R` callable interface type.
    CallableType,
    /// `?T` optional type.
    OptionalType,
    /// `E!T` error-union type.
    ErrorUnionType,
    /// `Self` type.
    SelfType,
    /// `opaque` type.
    OpaqueType,
    /// `never` type.
    NeverType,
    /// `_` inferred type.
    InferType,
    /// Unparseable type region retained for recovery.
    ErrorType,

    /// Single-token literal expression.
    LiteralExpr,
    /// Adjacent string or byte-string literal run.
    StringExpr,
    /// Name, `self`, `pkg`, `super`, or `_` expression.
    NameExpr,
    /// Tuple expression, including the unit value.
    TupleExpr,
    /// Parenthesized expression.
    ParenExpr,
    /// Braced block.
    Block,
    /// `if` expression, optionally with pattern conditions.
    IfExpr,
    /// `target is pattern` condition clause.
    IsClause,
    /// `match` expression.
    MatchExpr,
    /// `match` arm.
    MatchArm,
    /// Closure expression.
    ClosureExpr,
    /// Closure capture list.
    CaptureList,
    /// Closure capture.
    Capture,
    /// Closure parameter list.
    ClosureParamList,
    /// Array literal.
    ArrayExpr,
    /// `[T]::` type target.
    TypeTargetExpr,
    /// `[T as Trait]::` trait target.
    TraitTargetExpr,
    /// `Type { ... }` struct literal.
    TypedStructLiteral,
    /// `path::Name { ... }` struct literal.
    QualifiedStructLiteral,
    /// `.{ ... }` aggregate literal.
    OmittedAggregateLiteral,
    /// `.name` member expression.
    OmittedMemberExpr,
    /// Braced field initializer list.
    FieldInitList,
    /// Field initializer.
    FieldInit,
    /// Prefix `-`, `~`, `&`, `&mut`, `?`, or `!` expression.
    PrefixExpr,
    /// `not` expression.
    NotExpr,
    /// `expr as Type` cast expression.
    CastExpr,
    /// Binary operator expression.
    BinaryExpr,
    /// Assignment expression.
    AssignExpr,
    /// Range expression.
    RangeExpr,
    /// Call expression.
    CallExpr,
    /// Parenthesized call argument list.
    ArgList,
    /// `.name` field expression.
    FieldExpr,
    /// `.0` tuple field expression.
    TupleFieldExpr,
    /// `.?` propagation expression.
    TryExpr,
    /// `.*` dereference expression.
    DerefExpr,
    /// Postfix `!` error-payload expression.
    ErrorErrExpr,
    /// `::name` qualified expression.
    QualifiedExpr,
    /// `expr[...]` bracket suffix expression.
    BracketExpr,
    /// Bracket argument list.
    BracketArgList,
    /// Bracket argument.
    BracketArg,
    /// Range inside a bracket suffix.
    SliceRange,
    /// Unparseable expression region retained for recovery.
    RawExpr,

    /// `let` or `const` local binding.
    LetStmt,
    /// Local `static` binding.
    StaticStmt,
    /// Local `using` declaration.
    UsingStmt,
    /// `return` statement.
    ReturnStmt,
    /// `break` statement.
    BreakStmt,
    /// `continue` statement.
    ContinueStmt,
    /// `defer` statement.
    DeferStmt,
    /// `for` loop.
    ForStmt,
    /// `while` loop.
    WhileStmt,
    /// `loop` loop.
    LoopStmt,
    /// Expression statement.
    ExprStmt,

    /// `_` pattern.
    WildcardPattern,
    /// Binding pattern.
    BindPattern,
    /// `mut` pattern prefix.
    MutPattern,
    /// `&p` or `&mut p` pattern.
    PointerPattern,
    /// `?p` optional pattern.
    OptionalPattern,
    /// `null` pattern.
    NullPattern,
    /// `!p` error-union success pattern.
    ErrorOkPattern,
    /// `p!` error-union error pattern.
    ErrorErrPattern,
    /// Tuple pattern.
    TuplePattern,
    /// Parenthesized pattern.
    ParenPattern,
    /// Nominal constructor pattern.
    NominalPattern,
    /// Positional nominal pattern fields.
    NominalTupleFields,
    /// Named nominal pattern fields.
    NominalNamedFields,
    /// Named nominal pattern field.
    NamedPatternField,
    /// Expression pattern.
    ExprPattern,
    /// Closed range pattern.
    RangePattern,

    /// Significant lexical token.
    Token(TokenKind),
    /// Whitespace trivia.
    Whitespace,
    /// Line-comment trivia.
    LineComment,
}

impl SyntaxKind {
    /// Reports whether this node is a type production.
    pub fn is_type(&self) -> bool {
        matches!(
            self,
            Self::PathType
                | Self::ProjectionType
                | Self::PointerType
                | Self::VolatilePointerType
                | Self::SliceType
                | Self::SlicePointeeType
                | Self::ArrayType
                | Self::TupleType
                | Self::ParenType
                | Self::RangeType
                | Self::FnPointerType
                | Self::CallableType
                | Self::OptionalType
                | Self::ErrorUnionType
                | Self::SelfType
                | Self::OpaqueType
                | Self::NeverType
                | Self::InferType
                | Self::ErrorType
        )
    }

    /// Reports whether this node is an expression production.
    pub fn is_expr(&self) -> bool {
        matches!(
            self,
            Self::LiteralExpr
                | Self::StringExpr
                | Self::NameExpr
                | Self::TupleExpr
                | Self::ParenExpr
                | Self::Block
                | Self::IfExpr
                | Self::MatchExpr
                | Self::ClosureExpr
                | Self::ArrayExpr
                | Self::TypeTargetExpr
                | Self::TraitTargetExpr
                | Self::TypedStructLiteral
                | Self::QualifiedStructLiteral
                | Self::OmittedAggregateLiteral
                | Self::OmittedMemberExpr
                | Self::PrefixExpr
                | Self::NotExpr
                | Self::CastExpr
                | Self::BinaryExpr
                | Self::AssignExpr
                | Self::RangeExpr
                | Self::CallExpr
                | Self::FieldExpr
                | Self::TupleFieldExpr
                | Self::TryExpr
                | Self::DerefExpr
                | Self::ErrorErrExpr
                | Self::QualifiedExpr
                | Self::BracketExpr
                | Self::RawExpr
        )
    }

    /// Reports whether this node is a statement production.
    pub fn is_stmt(&self) -> bool {
        matches!(
            self,
            Self::LetStmt
                | Self::StaticStmt
                | Self::UsingStmt
                | Self::ReturnStmt
                | Self::BreakStmt
                | Self::ContinueStmt
                | Self::DeferStmt
                | Self::ForStmt
                | Self::WhileStmt
                | Self::LoopStmt
                | Self::ExprStmt
        )
    }

    /// Reports whether this node is a pattern production.
    pub fn is_pattern(&self) -> bool {
        matches!(
            self,
            Self::WildcardPattern
                | Self::BindPattern
                | Self::MutPattern
                | Self::PointerPattern
                | Self::OptionalPattern
                | Self::NullPattern
                | Self::ErrorOkPattern
                | Self::ErrorErrPattern
                | Self::TuplePattern
                | Self::ParenPattern
                | Self::NominalPattern
                | Self::ExprPattern
                | Self::RangePattern
        )
    }

    /// Reports whether this node is an item declaration production.
    pub fn is_declaration(&self) -> bool {
        matches!(
            self,
            Self::ModuleDecl
                | Self::UsingDecl
                | Self::StructDecl
                | Self::UnionDecl
                | Self::EnumDecl
                | Self::TraitDecl
                | Self::ExtendDecl
                | Self::TypeAliasDecl
                | Self::FunctionDecl
                | Self::BindingDecl
        )
    }

    /// Returns the lexical kind of a significant token element.
    pub fn token_kind(&self) -> Option<&TokenKind> {
        match self {
            Self::Token(kind) => Some(kind),
            _ => None,
        }
    }

    /// Reports whether this element is trivia.
    pub fn is_trivia(&self) -> bool {
        matches!(self, Self::Whitespace | Self::LineComment)
    }

    /// Maps this element to the coarse node-identity category.
    pub fn node_key_kind(&self) -> NodeSyntaxKind {
        if self.is_type() {
            NodeSyntaxKind::Type
        } else if self.is_expr() {
            NodeSyntaxKind::Expr
        } else if self.is_stmt() {
            NodeSyntaxKind::Stmt
        } else if self.is_pattern() {
            NodeSyntaxKind::Pattern
        } else if self.is_declaration() || matches!(self, Self::Item | Self::Attribute) {
            NodeSyntaxKind::Item
        } else {
            match self {
                Self::SourceFile => NodeSyntaxKind::Module,
                Self::Param => NodeSyntaxKind::Param,
                Self::Token(_) | Self::Whitespace | Self::LineComment => NodeSyntaxKind::Token,
                _ => NodeSyntaxKind::Syntax,
            }
        }
    }
}
