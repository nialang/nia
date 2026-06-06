// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{AssignOp, BinaryOp, ReceiverKind, UnaryOp};
use nia_ids::{BuiltinTraitMethod, GlobalDefId, InternedTyId, LayoutBuiltin, LocalId};
pub use nia_sema_ir::{
    ArrayToSliceCoercion, BracketSuffixResolution, BuiltinMethod, BuiltinOperatorOp, BuiltinValue,
    CStringPointerCoercion, ComptimeIfSelection, FunctionReference, GenericInstantiation,
    ResolvedCall, TraitObjectCoercion, TraitObjectUpcast,
};
use nia_span::Span;
use nia_static_ir::StaticInit;
use nia_ty::{BuiltinTrait, TraitId, TyInterner};

#[derive(Debug, Clone, PartialEq)]
pub struct BodyIr {
    pub interner: TyInterner,
    pub function_bodies: HashMap<GlobalDefId, TypedBody>,
    pub global_inits: HashMap<GlobalDefId, StaticInit>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedBody {
    pub span: Span,
    pub locals: Vec<TypedLocal>,
    pub stmts: Vec<TypedStmt>,
    pub tail: Option<Box<TypedExpr>>,
    pub ty: InternedTyId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedLocal {
    pub id: LocalId,
    pub name: String,
    pub kind: TypedLocalKind,
    pub ty: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedLocalKind {
    Param,
    Binding,
    ConstBinding,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedStmt {
    pub span: Span,
    pub kind: TypedStmtKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedStmtKind {
    Binding(TypedBinding),
    Expr(TypedExpr),
    Return(Option<TypedExpr>),
    Break,
    Continue,
    Defer(TypedExpr),
    ForIn(Box<TypedForIn>),
    While(Box<TypedWhile>),
    Loop(Box<TypedLoop>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedBinding {
    pub local_id: LocalId,
    pub name: String,
    pub ty: InternedTyId,
    pub value: Option<TypedExpr>,
    pub is_let: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedForIn {
    pub local_id: LocalId,
    pub name: String,
    pub ty: InternedTyId,
    pub iter: TypedForIterator,
    pub body: TypedBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedForIterator {
    Range(TypedRangeIterator),
    Expr(TypedExpr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedRangeIterator {
    pub span: Span,
    pub ty: InternedTyId,
    pub expr: TypedExpr,
    pub kind: TypedRangeIteratorKind,
    pub has_end: bool,
    pub inclusive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedRangeIteratorKind {
    Exclusive,
    Inclusive,
    From,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedWhile {
    pub cond: TypedExpr,
    pub body: TypedBody,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedLoop {
    pub body: TypedBody,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedSwitch {
    pub target: TypedExpr,
    pub bool_ty: InternedTyId,
    pub arms: Vec<TypedSwitchArm>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedSwitchArm {
    pub patterns: Vec<TypedSwitchPattern>,
    pub body: TypedSwitchArmBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedSwitchPattern {
    Default,
    OptionalSome {
        local_id: LocalId,
        name: String,
        ty: InternedTyId,
        span: Span,
    },
    OptionalNull {
        span: Span,
    },
    ErrorOk {
        local_id: LocalId,
        name: String,
        ty: InternedTyId,
        span: Span,
    },
    ErrorErr {
        local_id: LocalId,
        name: String,
        ty: InternedTyId,
        span: Span,
    },
    Expr(TypedExpr),
    CheckedInt {
        value: i128,
        ty: InternedTyId,
        span: Span,
    },
    Range {
        start: Box<TypedExpr>,
        end: Box<TypedExpr>,
        inclusive: bool,
        span: Span,
    },
    CheckedIntRange {
        start: i128,
        end: i128,
        inclusive: bool,
        ty: InternedTyId,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedSwitchArmBody {
    Expr(TypedExpr),
    Stmt(Box<TypedStmt>),
    Block(Box<TypedBody>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedExpr {
    pub span: Span,
    pub ty: InternedTyId,
    pub kind: TypedExprKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedExprKind {
    Error,
    Integer(String),
    Float(String),
    String(Vec<u32>),
    ByteString(Vec<u8>),
    Char(u32),
    ByteChar(String),
    Bool(bool),
    Null,
    Local(LocalId),
    Global(GlobalDefId),
    Function(GlobalDefId),
    FunctionInstance {
        def_id: GlobalDefId,
        arg_module_id: nia_ids::ModuleId,
        args: Vec<InternedTyId>,
    },
    EnumVariant(GlobalDefId),
    BuiltinValue(BuiltinConst),
    Range(TypedRange),
    InlineAsm(TypedInlineAsm),
    CStringPointer {
        array: Box<TypedExpr>,
        is_readonly: bool,
    },
    ArrayLiteral {
        elems: TypedArrayElements,
    },
    StructLiteral {
        def_id: GlobalDefId,
        fields: Vec<TypedFieldInit>,
    },
    UnionLiteral {
        def_id: GlobalDefId,
        field: Box<TypedFieldInit>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<TypedExpr>,
    },
    OptionalSome {
        expr: Box<TypedExpr>,
    },
    ErrorOk {
        expr: Box<TypedExpr>,
    },
    ErrorErr {
        expr: Box<TypedExpr>,
    },
    Try {
        expr: Box<TypedExpr>,
    },
    Binary {
        lhs: Box<TypedExpr>,
        op: BinaryOp,
        rhs: Box<TypedExpr>,
    },
    Assign {
        place: TypedPlace,
        op: AssignOp,
        rhs: Box<TypedExpr>,
    },
    Discard(Box<TypedExpr>),
    Cast {
        expr: Box<TypedExpr>,
        ty: InternedTyId,
    },
    TraitObjectUpcast {
        expr: Box<TypedExpr>,
        source_ty: InternedTyId,
        target_ty: InternedTyId,
    },
    TraitObjectCoercion {
        expr: Box<TypedExpr>,
        target_ty: InternedTyId,
        self_ty: InternedTyId,
    },
    Call {
        callee: TypedCallee,
        args: Vec<TypedExpr>,
    },
    Field {
        lhs: Box<TypedExpr>,
        field: GlobalDefId,
    },
    Index {
        lhs: Box<TypedExpr>,
        index: Box<TypedExpr>,
    },
    Slice {
        lhs: Box<TypedExpr>,
        range: TypedSliceRange,
        is_readonly: bool,
    },
    Block(TypedBody),
    If {
        cond: Box<TypedExpr>,
        then_branch: TypedBody,
        else_branch: Option<Box<TypedExpr>>,
    },
    Switch(Box<TypedSwitch>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedSliceRange {
    pub start: Option<Box<TypedExpr>>,
    pub end: Option<Box<TypedExpr>>,
    pub inclusive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedRange {
    pub start: Option<Box<TypedExpr>>,
    pub end: Option<Box<TypedExpr>>,
    pub inclusive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BuiltinConst {
    Usize(u64),
    Layout {
        builtin: LayoutBuiltin,
        ty: InternedTyId,
    },
    Int(i128),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedInlineAsm {
    pub code: String,
    pub inputs: Vec<TypedAsmInput>,
    pub outputs: Vec<TypedAsmOutput>,
    pub clobbers: Vec<String>,
    pub options: Vec<AsmOption>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedAsmInput {
    pub constraint: String,
    pub value: TypedExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedAsmOutput {
    pub constraint: String,
    pub place: TypedPlace,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsmOption {
    Volatile,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedArrayElements {
    List(Vec<TypedExpr>),
    Repeat { value: Box<TypedExpr>, count: u64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedFieldInit {
    pub field: Option<GlobalDefId>,
    pub name: String,
    pub value: TypedExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedCallee {
    Function(GlobalDefId),
    FunctionInstance {
        def_id: GlobalDefId,
        arg_module_id: nia_ids::ModuleId,
        args: Vec<InternedTyId>,
    },
    Method {
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
        receiver_kind: ReceiverKind,
        receiver: Box<TypedExpr>,
    },
    TraitMethod {
        trait_id: GlobalDefId,
        method_id: GlobalDefId,
        method_name: String,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        args: Vec<InternedTyId>,
        receiver_kind: ReceiverKind,
        receiver: Box<TypedExpr>,
    },
    DynamicTraitMethod {
        object_ty: InternedTyId,
        trait_id: TraitId,
        method_id: GlobalDefId,
        method_name: String,
        trait_args: Vec<InternedTyId>,
        slot: usize,
        params: Vec<InternedTyId>,
        return_type: InternedTyId,
        receiver_kind: ReceiverKind,
        receiver: Box<TypedExpr>,
    },
    BuiltinMethod {
        method: BuiltinMethod,
        self_ty: InternedTyId,
        receiver: Box<TypedExpr>,
    },
    BuiltinOperator(BuiltinOperator),
    BuiltinPlaceMethod(BuiltinPlaceMethod),
    FunctionPointer(Box<TypedExpr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinOperator {
    pub trait_id: BuiltinTrait,
    pub op: BuiltinOperatorOp,
}

impl BuiltinOperator {
    pub fn method(self) -> Option<BuiltinTraitMethod> {
        self.op
            .method()
            .filter(|method| method.trait_id() == self.trait_id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltinPlaceMethod {
    pub trait_id: BuiltinTrait,
    pub method: BuiltinTraitMethod,
    pub self_ty: InternedTyId,
    pub trait_args: Vec<InternedTyId>,
    pub receiver: Box<TypedExpr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedPlace {
    pub span: Span,
    pub ty: InternedTyId,
    pub base: PlaceBase,
    pub elems: Vec<PlaceElem>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlaceBase {
    Local(LocalId),
    Global(GlobalDefId),
    Deref(Box<TypedExpr>),
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlaceElem {
    Field(GlobalDefId),
    Index(Box<TypedExpr>),
    Error,
}
