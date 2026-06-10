// SPDX-License-Identifier: GPL-3.0-or-later
use nia_node_id::NodeKey;
use nia_span::Span;

use crate::TypeRef;

#[derive(Debug, Clone, PartialEq)]
pub struct ExprStub {
    pub span: Span,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub span: Span,
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub span: Span,
    pub node_key: NodeKey,
    pub kind: StmtKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Binding(Box<BindingStmt>),
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
    pub name: String,
    pub pattern_kind: ForPatternKind,
    pub pattern_span: Span,
    pub pattern_node_key: NodeKey,
    pub ty: Option<TypeRef>,
    pub value: Option<Expr>,
    pub is_let: bool,
    pub is_comptime: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForInStmt {
    pub pattern: ForPattern,
    pub iter: Expr,
    pub body: Block,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForPattern {
    pub span: Span,
    pub node_key: NodeKey,
    pub name: Option<String>,
    pub kind: ForPatternKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForPatternKind {
    Value,
    Pointer,
    MutPointer,
}

impl ForPattern {
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
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
pub struct ComptimeIfExpr {
    pub cond: Box<Expr>,
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
    pub patterns: Vec<SwitchPattern>,
    pub body: SwitchArmBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SwitchPattern {
    Default,
    OptionalSome {
        name: String,
        span: Span,
        node_key: NodeKey,
    },
    OptionalNull {
        span: Span,
    },
    ErrorOk {
        name: String,
        span: Span,
        node_key: NodeKey,
    },
    ErrorErr {
        name: String,
        span: Span,
        node_key: NodeKey,
    },
    Expr(Box<Expr>),
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        inclusive: bool,
        span: Span,
    },
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
    pub node_key: NodeKey,
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
    CString(StringLiteral),
    Char(String),
    ByteChar(String),
    Raw(String),
    Bool(bool),
    Null,
    Ident(String),
    Underscore,
    Builtin {
        name: String,
        type_arg: Option<TypeRef>,
    },
    TypeTarget {
        ty: TypeRef,
    },
    BracketSuffix {
        callee: Box<Expr>,
        args: Vec<BracketArg>,
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
        name: String,
    },
    Field {
        lhs: Box<Expr>,
        name: String,
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
    ComptimeIf(Box<ComptimeIfExpr>),
    Switch(Box<SwitchStmt>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct BracketArg {
    pub span: Span,
    pub expr: Option<Expr>,
    pub ty: Option<TypeRef>,
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
    pub name: String,
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
