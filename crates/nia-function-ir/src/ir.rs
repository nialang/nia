// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{AssignOp, BinaryOp, UnaryOp};
use nia_ids::{BuiltinTraitMethod, InternedTyId, LayoutBuiltin, LocalId};
use nia_span::Span;
use nia_ty::BuiltinTrait;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionBlockId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionScopeId(pub u32);

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionBody {
    pub span: Span,
    pub locals: Vec<FunctionLocal>,
    pub scopes: Vec<FunctionScope>,
    pub blocks: Vec<FunctionBlock>,
    pub entry: FunctionBlockId,
    pub ty: InternedTyId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionLocal {
    pub id: LocalId,
    pub name: String,
    pub kind: FunctionLocalKind,
    pub ty: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionLocalKind {
    Param,
    Binding,
    ConstBinding,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionScope {
    pub id: FunctionScopeId,
    pub parent: Option<FunctionScopeId>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionBlock {
    pub id: FunctionBlockId,
    pub scope: FunctionScopeId,
    pub span: Span,
    pub ops: Vec<FunctionOp>,
    pub terminator: FunctionTerminator,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionOp {
    Binding(FunctionBinding),
    StoreLocal {
        local_id: LocalId,
        value: FunctionExpr,
        span: Span,
    },
    Expr(FunctionExpr),
    Defer(FunctionDeferBody),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionBinding {
    pub local_id: LocalId,
    pub name: String,
    pub ty: InternedTyId,
    pub value: Option<FunctionExpr>,
    pub is_const: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDeferBody {
    pub span: Span,
    pub scopes: Vec<FunctionScope>,
    pub blocks: Vec<FunctionBlock>,
    pub entry: FunctionBlockId,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionTerminator {
    Error {
        span: Span,
    },
    Branch {
        target: FunctionBlockId,
        span: Span,
    },
    Next {
        target: FunctionBlockId,
        span: Span,
    },
    If {
        cond: FunctionExpr,
        then_target: FunctionBlockId,
        else_target: FunctionBlockId,
        span: Span,
    },
    Switch {
        target: FunctionExpr,
        arms: Vec<FunctionSwitchArm>,
        default: Option<FunctionBlockId>,
        fallback: FunctionBlockId,
        span: Span,
    },
    Loop {
        header: FunctionForHeader,
        body: FunctionBlockId,
        continue_target: FunctionBlockId,
        break_target: FunctionBlockId,
        span: Span,
    },
    Return {
        value: Option<FunctionExpr>,
        span: Span,
    },
    Tail {
        value: Option<FunctionExpr>,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionSwitchArm {
    pub pattern: FunctionExpr,
    pub target: FunctionBlockId,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionForHeader {
    Infinite,
    Condition(FunctionExpr),
    CStyle { cond: Option<Box<FunctionExpr>> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionExpr {
    pub span: Span,
    pub ty: InternedTyId,
    pub kind: FunctionExprKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionExprKind {
    Error,
    Integer(String),
    Float(String),
    String(Vec<u32>),
    ByteString(Vec<u8>),
    Char(u32),
    ByteChar(String),
    Bool(bool),
    Local(LocalId),
    Global(nia_ids::GlobalDefId),
    Function(nia_ids::GlobalDefId),
    FunctionInstance {
        def_id: nia_ids::GlobalDefId,
        args: Vec<InternedTyId>,
    },
    EnumVariant(nia_ids::GlobalDefId),
    BuiltinValue(FunctionBuiltinValue),
    Range(FunctionRange),
    InlineAsm(FunctionInlineAsm),
    CStringPointer {
        array: Box<FunctionExpr>,
        is_const: bool,
    },
    ArrayLiteral {
        elems: FunctionArrayElements,
    },
    StructLiteral {
        def_id: nia_ids::GlobalDefId,
        fields: Vec<FunctionFieldInit>,
    },
    UnionLiteral {
        def_id: nia_ids::GlobalDefId,
        field: Box<FunctionFieldInit>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<FunctionExpr>,
    },
    AddrOf(FunctionPlace),
    Binary {
        lhs: Box<FunctionExpr>,
        op: BinaryOp,
        rhs: Box<FunctionExpr>,
    },
    Assign {
        place: FunctionPlace,
        op: AssignOp,
        rhs: Box<FunctionExpr>,
    },
    Discard(Box<FunctionExpr>),
    Cast {
        expr: Box<FunctionExpr>,
        ty: InternedTyId,
    },
    TraitObjectUpcast {
        expr: Box<FunctionExpr>,
        target_ty: InternedTyId,
    },
    Call {
        callee: FunctionCallee,
        args: Vec<FunctionExpr>,
    },
    Field {
        lhs: Box<FunctionExpr>,
        field: nia_ids::GlobalDefId,
    },
    Index {
        lhs: Box<FunctionExpr>,
        index: Box<FunctionExpr>,
    },
    Slice {
        lhs: Box<FunctionExpr>,
        range: FunctionSliceRange,
        is_const: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionSliceRange {
    pub start: Option<Box<FunctionExpr>>,
    pub end: Option<Box<FunctionExpr>>,
    pub inclusive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionRange {
    pub start: Option<Box<FunctionExpr>>,
    pub end: Option<Box<FunctionExpr>>,
    pub inclusive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionInlineAsm {
    pub code: String,
    pub inputs: Vec<FunctionAsmInput>,
    pub outputs: Vec<FunctionAsmOutput>,
    pub clobbers: Vec<String>,
    pub options: Vec<FunctionAsmOption>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunctionBuiltinValue {
    Usize(u64),
    Layout {
        builtin: LayoutBuiltin,
        ty: InternedTyId,
    },
    Int(i128),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionAsmOption {
    Volatile,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionAsmInput {
    pub constraint: String,
    pub value: FunctionExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionAsmOutput {
    pub constraint: String,
    pub place: FunctionPlace,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionArrayElements {
    List(Vec<FunctionExpr>),
    Repeat {
        value: Box<FunctionExpr>,
        count: u64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionFieldInit {
    pub field: Option<nia_ids::GlobalDefId>,
    pub name: String,
    pub value: FunctionExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionCallee {
    Function(nia_ids::GlobalDefId),
    FunctionInstance {
        def_id: nia_ids::GlobalDefId,
        args: Vec<InternedTyId>,
    },
    Method {
        def_id: nia_ids::GlobalDefId,
        args: Vec<InternedTyId>,
        receiver: Box<FunctionExpr>,
    },
    TraitMethod {
        trait_id: nia_ids::GlobalDefId,
        method_id: nia_ids::GlobalDefId,
        method_name: String,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        args: Vec<InternedTyId>,
        receiver: Box<FunctionExpr>,
    },
    BuiltinPlaceMethod {
        trait_id: BuiltinTrait,
        method: BuiltinTraitMethod,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        receiver: Box<FunctionExpr>,
    },
    BuiltinOperator(FunctionBuiltinOperator),
    FunctionPointer(Box<FunctionExpr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FunctionBuiltinOperator {
    pub trait_id: BuiltinTrait,
    pub op: FunctionBuiltinOperatorOp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionBuiltinOperatorOp {
    Unary(UnaryOp),
    Binary(BinaryOp),
}

impl FunctionBuiltinOperatorOp {
    pub fn method(self) -> Option<BuiltinTraitMethod> {
        match self {
            Self::Unary(op) => match op {
                UnaryOp::Neg => Some(BuiltinTraitMethod::Neg),
                UnaryOp::Not => Some(BuiltinTraitMethod::Not),
                UnaryOp::BitNot => Some(BuiltinTraitMethod::BitNot),
                UnaryOp::RefConst | UnaryOp::Ref | UnaryOp::Deref => None,
            },
            Self::Binary(op) => match op {
                BinaryOp::Add => Some(BuiltinTraitMethod::Add),
                BinaryOp::Sub => Some(BuiltinTraitMethod::Sub),
                BinaryOp::Mul => Some(BuiltinTraitMethod::Mul),
                BinaryOp::Div => Some(BuiltinTraitMethod::Div),
                BinaryOp::Rem => Some(BuiltinTraitMethod::Rem),
                BinaryOp::BitAnd => Some(BuiltinTraitMethod::BitAnd),
                BinaryOp::BitOr => Some(BuiltinTraitMethod::BitOr),
                BinaryOp::BitXor => Some(BuiltinTraitMethod::BitXor),
                BinaryOp::Shl => Some(BuiltinTraitMethod::Shl),
                BinaryOp::Shr => Some(BuiltinTraitMethod::Shr),
                BinaryOp::Eq => Some(BuiltinTraitMethod::Eq),
                BinaryOp::Ne => Some(BuiltinTraitMethod::Ne),
                BinaryOp::Lt => Some(BuiltinTraitMethod::Lt),
                BinaryOp::Le => Some(BuiltinTraitMethod::Le),
                BinaryOp::Gt => Some(BuiltinTraitMethod::Gt),
                BinaryOp::Ge => Some(BuiltinTraitMethod::Ge),
                BinaryOp::And | BinaryOp::Or => None,
            },
        }
    }
}

impl FunctionBuiltinOperator {
    pub fn method(self) -> Option<BuiltinTraitMethod> {
        self.op
            .method()
            .filter(|method| method.trait_id() == self.trait_id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionPlace {
    pub span: Span,
    pub ty: InternedTyId,
    pub base: FunctionPlaceBase,
    pub elems: Vec<FunctionPlaceElem>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionPlaceBase {
    Local(LocalId),
    Global(nia_ids::GlobalDefId),
    Deref(Box<FunctionExpr>),
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionPlaceElem {
    Field(nia_ids::GlobalDefId),
    Index(Box<FunctionExpr>),
    Error,
}

impl FunctionTerminator {
    pub fn successors(&self) -> Vec<FunctionBlockId> {
        match self {
            FunctionTerminator::Error { .. } => Vec::new(),
            FunctionTerminator::Branch { target, .. } | FunctionTerminator::Next { target, .. } => {
                vec![*target]
            }
            FunctionTerminator::If {
                then_target,
                else_target,
                ..
            } => vec![*then_target, *else_target],
            FunctionTerminator::Switch {
                arms,
                default,
                fallback,
                ..
            } => arms
                .iter()
                .map(|arm| arm.target)
                .chain(default.or(Some(*fallback)))
                .collect(),
            FunctionTerminator::Loop {
                body, break_target, ..
            } => vec![*body, *break_target],
            FunctionTerminator::Return { .. } | FunctionTerminator::Tail { .. } => Vec::new(),
        }
    }
}

impl FunctionBody {
    pub fn block(&self, id: FunctionBlockId) -> Option<&FunctionBlock> {
        self.blocks.iter().find(|block| block.id == id)
    }

    pub fn scope(&self, id: FunctionScopeId) -> Option<&FunctionScope> {
        self.scopes.iter().find(|scope| scope.id == id)
    }

    pub fn edge_exited_scopes(
        &self,
        from: FunctionBlockId,
        to: FunctionBlockId,
    ) -> Option<Vec<FunctionScopeId>> {
        let from = self.block(from)?.scope;
        let to = self.block(to)?.scope;
        self.exited_scopes_between(from, Some(to))
    }

    pub fn return_exited_scopes(&self, from: FunctionBlockId) -> Option<Vec<FunctionScopeId>> {
        let from = self.block(from)?.scope;
        self.exited_scopes_between(from, None)
    }

    pub fn exited_scopes_between(
        &self,
        from: FunctionScopeId,
        to: Option<FunctionScopeId>,
    ) -> Option<Vec<FunctionScopeId>> {
        let from_chain = self.scope_chain_to_root(from)?;
        let to_chain = match to {
            Some(scope) => self.scope_chain_to_root(scope)?,
            None => Vec::new(),
        };
        let lca = from_chain
            .iter()
            .find(|scope| to_chain.contains(scope))
            .copied();
        Some(
            from_chain
                .into_iter()
                .take_while(|scope| Some(*scope) != lca)
                .collect(),
        )
    }

    fn scope_chain_to_root(&self, scope: FunctionScopeId) -> Option<Vec<FunctionScopeId>> {
        let mut chain = Vec::new();
        let mut current = Some(scope);
        while let Some(scope) = current {
            chain.push(scope);
            current = self.scope(scope)?.parent;
        }
        Some(chain)
    }
}

impl FunctionDeferBody {
    pub fn block(&self, id: FunctionBlockId) -> Option<&FunctionBlock> {
        self.blocks.iter().find(|block| block.id == id)
    }

    pub fn scope(&self, id: FunctionScopeId) -> Option<&FunctionScope> {
        self.scopes.iter().find(|scope| scope.id == id)
    }

    pub fn edge_exited_scopes(
        &self,
        from: FunctionBlockId,
        to: FunctionBlockId,
    ) -> Option<Vec<FunctionScopeId>> {
        let from = self.block(from)?.scope;
        let to = self.block(to)?.scope;
        self.exited_scopes_between(from, Some(to))
    }

    pub fn return_exited_scopes(&self, from: FunctionBlockId) -> Option<Vec<FunctionScopeId>> {
        let from = self.block(from)?.scope;
        self.exited_scopes_between(from, None)
    }

    pub fn exited_scopes_between(
        &self,
        from: FunctionScopeId,
        to: Option<FunctionScopeId>,
    ) -> Option<Vec<FunctionScopeId>> {
        let from_chain = self.scope_chain_to_root(from)?;
        let to_chain = match to {
            Some(scope) => self.scope_chain_to_root(scope)?,
            None => Vec::new(),
        };
        let lca = from_chain
            .iter()
            .find(|scope| to_chain.contains(scope))
            .copied();
        Some(
            from_chain
                .into_iter()
                .take_while(|scope| Some(*scope) != lca)
                .collect(),
        )
    }

    fn scope_chain_to_root(&self, scope: FunctionScopeId) -> Option<Vec<FunctionScopeId>> {
        let mut chain = Vec::new();
        let mut current = Some(scope);
        while let Some(scope) = current {
            chain.push(scope);
            current = self.scope(scope)?.parent;
        }
        Some(chain)
    }
}
