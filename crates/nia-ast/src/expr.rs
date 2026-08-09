// SPDX-License-Identifier: GPL-3.0-or-later
use nia_node_id::VersionedNodeKey;
use nia_span::Span;
use nia_symbol::SymbolId;

use crate::{PathSegmentKind, TypeRef};

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub span: Span,
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub span: Span,
    pub node_key: VersionedNodeKey,
    pub attributes: Vec<crate::Attribute>,
    pub kind: StmtKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Binding(Box<BindingStmt>),
    Static(Box<crate::BindingItem>),
    Using(crate::UsingItem),
    Expr(Box<Expr>),
    Return(Option<Box<Expr>>),
    Break,
    Continue,
    Defer(Box<Expr>),
    ForIn(Box<ForInStmt>),
    While(Box<WhileStmt>),
    Loop(Box<LoopStmt>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct BindingStmt {
    pub pattern: Pattern,
    pub ty: Option<TypeRef>,
    pub value: Option<Expr>,
    pub kind: LocalBindingKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalBindingKind {
    Let { is_mutable: bool },
    Const,
}

impl BindingStmt {
    pub fn is_const(&self) -> bool {
        matches!(self.kind, LocalBindingKind::Const)
    }

    pub fn is_mutable(&self) -> bool {
        matches!(self.kind, LocalBindingKind::Let { is_mutable: true })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForInStmt {
    pub pattern: Pattern,
    pub iter: Expr,
    pub body: Block,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhileStmt {
    pub cond: Expr,
    pub body: Block,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoopStmt {
    pub body: Block,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IfPatternExpr {
    pub target: Expr,
    pub pattern: Pattern,
    pub then_branch: Block,
    pub else_branch: Option<Box<Expr>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwitchStmt {
    pub target: Expr,
    pub arms: Vec<SwitchArm>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwitchArm {
    pub patterns: Vec<Pattern>,
    pub body: SwitchArmBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub span: Span,
    pub kind: PatternKind,
}

impl Pattern {
    pub fn contains_binding(&self) -> bool {
        match &self.kind {
            PatternKind::Bind { .. } => true,
            PatternKind::Pointer(pattern) | PatternKind::MutPointer(pattern) => {
                pattern.contains_binding()
            }
            PatternKind::OptionalSome(pattern)
            | PatternKind::ErrorOk(pattern)
            | PatternKind::ErrorErr(pattern) => pattern.contains_binding(),
            PatternKind::Tuple(patterns) => patterns.iter().any(Pattern::contains_binding),
            PatternKind::EnumVariant { fields, .. } => fields.contains_binding(),
            PatternKind::Wildcard
            | PatternKind::OptionalNull
            | PatternKind::Expr(_)
            | PatternKind::Range { .. } => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternKind {
    Wildcard,
    Bind {
        name: SymbolId,
        node_key: VersionedNodeKey,
        is_mutable: bool,
    },
    Pointer(Box<Pattern>),
    MutPointer(Box<Pattern>),
    OptionalSome(Box<Pattern>),
    OptionalNull,
    ErrorOk(Box<Pattern>),
    ErrorErr(Box<Pattern>),
    Tuple(Vec<Pattern>),
    EnumVariant {
        variant: Box<Expr>,
        fields: EnumVariantPatternFields,
    },
    Expr(Box<Expr>),
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        inclusive: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum EnumVariantPatternFields {
    Tuple(Vec<Pattern>),
    Named(Vec<NamedPatternField>),
}

impl EnumVariantPatternFields {
    fn contains_binding(&self) -> bool {
        match self {
            Self::Tuple(fields) => fields.iter().any(Pattern::contains_binding),
            Self::Named(fields) => fields.iter().any(|field| field.pattern.contains_binding()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamedPatternField {
    pub name: SymbolId,
    pub pattern: Pattern,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SwitchArmBody {
    Expr(Box<Expr>),
    Stmt(Box<Stmt>),
    Block(Box<Block>),
}

impl SwitchArmBody {
    pub fn span(&self) -> Span {
        match self {
            Self::Expr(expr) => expr.span,
            Self::Stmt(stmt) => stmt.span,
            Self::Block(block) => block.span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub span: Span,
    pub node_key: VersionedNodeKey,
    pub kind: ExprKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringLiteral {
    pub parts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Error,
    Integer(String),
    Float(String),
    String(StringLiteral),
    ByteString(StringLiteral),
    Char(String),
    ByteChar(String),
    Raw(String),
    Bool(bool),
    Null,
    Ident(SymbolId),
    SelfValue,
    PathRoot(PathSegmentKind),
    Underscore,
    TypeTarget {
        ty: TypeRef,
    },
    TraitTarget {
        ty: TypeRef,
        trait_ref: TypeRef,
    },
    BracketSuffix {
        callee: Box<Expr>,
        args: Vec<BracketArg>,
    },
    Tuple(Vec<Expr>),
    /// Anonymous closure value with explicit captures.
    ///
    /// The capture expressions are evaluated at the closure site and their
    /// names are visible only in the closure body. The concrete state type is
    /// intentionally anonymous; dynamic callable views are a later semantic
    /// coercion rather than a second source spelling.
    Closure {
        captures: Vec<ClosureCapture>,
        params: Vec<crate::Param>,
        return_type: Option<TypeRef>,
        body: Block,
    },
    ArrayLiteral {
        elems: ArrayElements,
    },
    StructLiteral {
        fields: Vec<FieldInit>,
    },
    TypedArrayLiteral {
        ty: TypeRef,
        elems: ArrayElements,
    },
    TypedStructLiteral {
        ty: TypeRef,
        fields: Vec<FieldInit>,
    },
    QualifiedStructLiteral {
        target: Box<Expr>,
        fields: Vec<FieldInit>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    OptionalSome {
        expr: Box<Expr>,
    },
    ErrorOk {
        expr: Box<Expr>,
    },
    ErrorErr {
        expr: Box<Expr>,
    },
    Try {
        expr: Box<Expr>,
    },
    Binary {
        lhs: Box<Expr>,
        op: BinaryOp,
        rhs: Box<Expr>,
    },
    Assign {
        lhs: Box<Expr>,
        op: AssignOp,
        rhs: Box<Expr>,
    },
    Cast {
        expr: Box<Expr>,
        ty: TypeRef,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Qualified {
        lhs: Box<Expr>,
        name: SymbolId,
    },
    Field {
        lhs: Box<Expr>,
        name: SymbolId,
    },
    TupleField {
        lhs: Box<Expr>,
        index: usize,
    },
    Index {
        lhs: Box<Expr>,
        index: IndexArg,
    },
    Range(SliceRange),
    Block(Block),
    If {
        cond: Box<Expr>,
        then_branch: Block,
        else_branch: Option<Box<Expr>>,
    },
    IfPattern(Box<IfPatternExpr>),
    Switch(Box<SwitchStmt>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct BracketArg {
    pub span: Span,
    pub expr: Option<Expr>,
    pub ty: Option<TypeRef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClosureCapture {
    pub name: SymbolId,
    pub value: Expr,
    pub span: Span,
    pub node_key: VersionedNodeKey,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IndexArg {
    Expr(Box<Expr>),
    Range(SliceRange),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SliceRange {
    pub start: Option<Box<Expr>>,
    pub end: Option<Box<Expr>>,
    pub inclusive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArrayElements {
    List(Vec<Expr>),
    Repeat { value: Box<Expr>, count: Box<Expr> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldInit {
    pub name: SymbolId,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
    RefReadOnly,
    Ref,
    Deref,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Mul,
    Div,
    Rem,
    Add,
    Sub,
    Shl,
    Shr,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    BitAnd,
    BitXor,
    BitOr,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    Add,
    Sub,
    Shl,
    Shr,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitXor,
    BitOr,
}
