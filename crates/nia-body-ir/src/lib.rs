// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{AssignOp, BinaryOp, UnaryOp};
use nia_ids::{GlobalDefId, InternedTyId, LocalId};
use nia_node_id::NodeKey;
use nia_span::Span;
use nia_static_ir::StaticInit;
use nia_ty::TyInterner;

#[derive(Debug, Clone, PartialEq)]
pub struct BodyIr {
    pub interner: TyInterner,
    pub function_bodies: HashMap<GlobalDefId, TypedBody>,
    pub global_inits: HashMap<GlobalDefId, StaticInit>,
    pub expr_types: HashMap<Span, InternedTyId>,
    pub bracket_suffix_resolutions: HashMap<Span, BracketSuffixResolution>,
    pub array_to_slice_coercions: HashMap<Span, ArrayToSliceCoercion>,
    pub c_string_pointer_coercions: HashMap<Span, CStringPointerCoercion>,
    pub local_types: HashMap<LocalId, InternedTyId>,
    pub builtin_values: HashMap<Span, BuiltinValue>,
    pub resolved_calls: HashMap<Span, ResolvedCall>,
    pub function_references: HashMap<Span, FunctionReference>,
    pub generic_instantiations: Vec<GenericInstantiation>,
    pub node_expr_types: HashMap<NodeKey, InternedTyId>,
    pub node_bracket_suffix_resolutions: HashMap<NodeKey, BracketSuffixResolution>,
    pub node_array_to_slice_coercions: HashMap<NodeKey, ArrayToSliceCoercion>,
    pub node_c_string_pointer_coercions: HashMap<NodeKey, CStringPointerCoercion>,
    pub node_builtin_values: HashMap<NodeKey, BuiltinValue>,
    pub node_resolved_calls: HashMap<NodeKey, ResolvedCall>,
    pub node_function_references: HashMap<NodeKey, FunctionReference>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinValue {
    Usize(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BracketSuffixResolution {
    Index,
    GenericCall,
    TypePrefixInstantiation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrayToSliceCoercion {
    pub array_ty: InternedTyId,
    pub slice_ty: InternedTyId,
    pub is_const: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CStringPointerCoercion {
    pub array_ty: InternedTyId,
    pub pointer_ty: InternedTyId,
    pub is_const: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GenericInstantiation {
    pub def_id: GlobalDefId,
    pub args: Vec<InternedTyId>,
    pub generics: Vec<String>,
    pub span: Span,
    pub source_def_id: Option<GlobalDefId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedCall {
    Function(GlobalDefId),
    FunctionInstance {
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
    },
    Method {
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
    },
    TraitMethod {
        trait_id: GlobalDefId,
        method_id: GlobalDefId,
        method_name: String,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        args: Vec<InternedTyId>,
    },
    FunctionPointer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionReference {
    pub def_id: GlobalDefId,
    pub args: Vec<InternedTyId>,
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
    For(Box<TypedFor>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedBinding {
    pub local_id: LocalId,
    pub name: String,
    pub ty: InternedTyId,
    pub value: Option<TypedExpr>,
    pub is_const: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedFor {
    pub header: TypedForHeader,
    pub body: TypedBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedForHeader {
    Infinite,
    Condition(TypedExpr),
    CStyle {
        init: Option<Box<TypedForInit>>,
        cond: Option<Box<TypedExpr>>,
        step: Option<Box<TypedExpr>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedForInit {
    Binding(TypedBinding),
    Expr(TypedExpr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedSwitch {
    pub target: TypedExpr,
    pub arms: Vec<TypedSwitchArm>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedSwitchArm {
    pub pattern: TypedSwitchPattern,
    pub body: TypedSwitchArmBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedSwitchPattern {
    Default,
    Expr(TypedExpr),
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
    Local(LocalId),
    Global(GlobalDefId),
    Function(GlobalDefId),
    FunctionInstance {
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
    },
    EnumVariant(GlobalDefId),
    BuiltinValue(BuiltinConst),
    Len(Box<TypedExpr>),
    Ptr(Box<TypedExpr>),
    InlineAsm(TypedInlineAsm),
    CStringPointer {
        array: Box<TypedExpr>,
        is_const: bool,
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
        is_const: bool,
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
pub enum BuiltinConst {
    Usize(u64),
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
    pub field: GlobalDefId,
    pub name: String,
    pub value: TypedExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedCallee {
    Function(GlobalDefId),
    FunctionInstance {
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
    },
    Method {
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
        receiver: Box<TypedExpr>,
    },
    TraitMethod {
        trait_id: GlobalDefId,
        method_id: GlobalDefId,
        method_name: String,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        args: Vec<InternedTyId>,
        receiver: Box<TypedExpr>,
    },
    FunctionPointer(Box<TypedExpr>),
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
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlaceElem {
    Field(GlobalDefId),
    Index(Box<TypedExpr>),
}
