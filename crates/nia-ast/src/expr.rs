// SPDX-License-Identifier: GPL-3.0-or-later
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
    pub kind: StmtKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Binding(BindingStmt),
    Using(crate::UsingItem),
    Expr(Expr),
    Return(Option<Expr>),
    Break,
    Continue,
    Defer(Expr),
    ForIn(Box<ForInStmt>),
    While(Box<WhileStmt>),
    Loop(Box<LoopStmt>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct BindingStmt {
    pub name: String,
    pub ty: Option<TypeRef>,
    pub value: Option<Expr>,
    pub is_const: bool,
    pub is_comptime: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForInStmt {
    pub binding: ForBinding,
    pub iter: Expr,
    pub body: Block,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForBinding {
    pub span: Span,
    pub name: String,
    pub ty: Option<TypeRef>,
    pub is_const: bool,
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
    Expr(Expr),
    Range {
        start: Expr,
        end: Expr,
        inclusive: bool,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum SwitchArmBody {
    Expr(Expr),
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
    RefConst,
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
