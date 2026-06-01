// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{AssignOp, BinaryOp, UnaryOp};
use nia_ids::{BuiltinTraitMethod, GlobalDefId, InternedTyId, LayoutBuiltin, LocalId};
use nia_node_id::NodeKey;
use nia_span::Span;
use nia_static_ir::StaticInit;
use nia_ty::{BuiltinTrait, TraitId, TyInterner};

#[derive(Debug, Clone, PartialEq)]
pub struct BodyIr {
    pub interner: TyInterner,
    pub function_bodies: HashMap<GlobalDefId, TypedBody>,
    pub global_inits: HashMap<GlobalDefId, StaticInit>,
    pub expr_types: HashMap<Span, InternedTyId>,
    pub bracket_suffix_resolutions: HashMap<Span, BracketSuffixResolution>,
    pub array_to_slice_coercions: HashMap<Span, ArrayToSliceCoercion>,
    pub c_string_pointer_coercions: HashMap<Span, CStringPointerCoercion>,
    pub trait_object_coercions: HashMap<Span, TraitObjectCoercion>,
    pub trait_object_upcasts: HashMap<Span, TraitObjectUpcast>,
    pub local_types: HashMap<LocalId, InternedTyId>,
    pub builtin_values: HashMap<Span, BuiltinValue>,
    pub resolved_calls: HashMap<Span, ResolvedCall>,
    pub function_references: HashMap<Span, FunctionReference>,
    pub generic_instantiations: Vec<GenericInstantiation>,
    pub node_expr_types: HashMap<NodeKey, InternedTyId>,
    pub node_bracket_suffix_resolutions: HashMap<NodeKey, BracketSuffixResolution>,
    pub node_array_to_slice_coercions: HashMap<NodeKey, ArrayToSliceCoercion>,
    pub node_c_string_pointer_coercions: HashMap<NodeKey, CStringPointerCoercion>,
    pub node_trait_object_coercions: HashMap<NodeKey, TraitObjectCoercion>,
    pub node_trait_object_upcasts: HashMap<NodeKey, TraitObjectUpcast>,
    pub node_builtin_values: HashMap<NodeKey, BuiltinValue>,
    pub node_resolved_calls: HashMap<NodeKey, ResolvedCall>,
    pub node_function_references: HashMap<NodeKey, FunctionReference>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuiltinValue {
    Usize(u64),
    Layout {
        builtin: LayoutBuiltin,
        ty: InternedTyId,
    },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraitObjectCoercion {
    pub source_ty: InternedTyId,
    pub target_ty: InternedTyId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraitObjectUpcast {
    pub source_ty: InternedTyId,
    pub target_ty: InternedTyId,
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
    DynamicTraitMethod {
        object_ty: InternedTyId,
        trait_id: TraitId,
        method_id: GlobalDefId,
        method_name: String,
        trait_args: Vec<InternedTyId>,
        slot: usize,
        params: Vec<InternedTyId>,
        return_type: InternedTyId,
    },
    BuiltinTraitMethod {
        trait_id: BuiltinTrait,
        op: BuiltinOperatorOp,
    },
    BuiltinPlaceMethod {
        trait_id: BuiltinTrait,
        method: BuiltinTraitMethod,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
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
    Range(TypedRange),
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
    DynamicTraitMethod {
        object_ty: InternedTyId,
        trait_id: TraitId,
        method_id: GlobalDefId,
        method_name: String,
        trait_args: Vec<InternedTyId>,
        slot: usize,
        params: Vec<InternedTyId>,
        return_type: InternedTyId,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinOperatorOp {
    Unary(UnaryOp),
    Binary(BinaryOp),
}

impl BuiltinOperatorOp {
    pub fn trait_id(self) -> Option<BuiltinTrait> {
        match self {
            Self::Unary(op) => match op {
                UnaryOp::Neg => Some(BuiltinTrait::Neg),
                UnaryOp::Not => Some(BuiltinTrait::Not),
                UnaryOp::BitNot => Some(BuiltinTrait::BitNot),
                UnaryOp::RefConst | UnaryOp::Ref | UnaryOp::Deref => None,
            },
            Self::Binary(op) => match op {
                BinaryOp::Add => Some(BuiltinTrait::Add),
                BinaryOp::Sub => Some(BuiltinTrait::Sub),
                BinaryOp::Mul => Some(BuiltinTrait::Mul),
                BinaryOp::Div => Some(BuiltinTrait::Div),
                BinaryOp::Rem => Some(BuiltinTrait::Rem),
                BinaryOp::BitAnd => Some(BuiltinTrait::BitAnd),
                BinaryOp::BitOr => Some(BuiltinTrait::BitOr),
                BinaryOp::BitXor => Some(BuiltinTrait::BitXor),
                BinaryOp::Shl => Some(BuiltinTrait::Shl),
                BinaryOp::Shr => Some(BuiltinTrait::Shr),
                BinaryOp::Eq | BinaryOp::Ne => Some(BuiltinTrait::Eq),
                BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                    Some(BuiltinTrait::Ord)
                }
                BinaryOp::And | BinaryOp::Or => None,
            },
        }
    }

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

    pub fn from_method(method: BuiltinTraitMethod) -> Option<Self> {
        match method {
            BuiltinTraitMethod::Add => Some(Self::Binary(BinaryOp::Add)),
            BuiltinTraitMethod::Sub => Some(Self::Binary(BinaryOp::Sub)),
            BuiltinTraitMethod::Mul => Some(Self::Binary(BinaryOp::Mul)),
            BuiltinTraitMethod::Div => Some(Self::Binary(BinaryOp::Div)),
            BuiltinTraitMethod::Rem => Some(Self::Binary(BinaryOp::Rem)),
            BuiltinTraitMethod::Neg => Some(Self::Unary(UnaryOp::Neg)),
            BuiltinTraitMethod::Not => Some(Self::Unary(UnaryOp::Not)),
            BuiltinTraitMethod::BitNot => Some(Self::Unary(UnaryOp::BitNot)),
            BuiltinTraitMethod::BitAnd => Some(Self::Binary(BinaryOp::BitAnd)),
            BuiltinTraitMethod::BitOr => Some(Self::Binary(BinaryOp::BitOr)),
            BuiltinTraitMethod::BitXor => Some(Self::Binary(BinaryOp::BitXor)),
            BuiltinTraitMethod::Shl => Some(Self::Binary(BinaryOp::Shl)),
            BuiltinTraitMethod::Shr => Some(Self::Binary(BinaryOp::Shr)),
            BuiltinTraitMethod::Eq => Some(Self::Binary(BinaryOp::Eq)),
            BuiltinTraitMethod::Ne => Some(Self::Binary(BinaryOp::Ne)),
            BuiltinTraitMethod::Lt => Some(Self::Binary(BinaryOp::Lt)),
            BuiltinTraitMethod::Le => Some(Self::Binary(BinaryOp::Le)),
            BuiltinTraitMethod::Gt => Some(Self::Binary(BinaryOp::Gt)),
            BuiltinTraitMethod::Ge => Some(Self::Binary(BinaryOp::Ge)),
            BuiltinTraitMethod::DerefConst
            | BuiltinTraitMethod::Deref
            | BuiltinTraitMethod::IndexConst
            | BuiltinTraitMethod::Index
            | BuiltinTraitMethod::SliceConst
            | BuiltinTraitMethod::Slice
            | BuiltinTraitMethod::Len
            | BuiltinTraitMethod::GetPtrConst
            | BuiltinTraitMethod::GetPtr => None,
        }
    }
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
