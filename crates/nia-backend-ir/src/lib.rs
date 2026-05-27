// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{AssignOp, BinaryOp, ReceiverKind, UnaryOp};
use nia_comptime_check::ComptimeCheck;
use nia_ids::{GlobalDefId, LocalId, ModuleId, TyId};
use nia_layout::{Layouts, StructLayout, StructLayoutKey, TypeLayout};
use nia_span::Span;
use nia_ty::TyInterner;

#[derive(Debug, Clone, PartialEq)]
pub struct BackendProgram {
    pub modules: Vec<BackendModule>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendModule {
    pub id: ModuleId,
    pub name: String,
    pub interner: TyInterner,
    pub comptime: ComptimeCheck,
    pub layouts: BackendLayouts,
    pub structs: Vec<BackendStruct>,
    pub unions: Vec<BackendUnion>,
    pub struct_instances: Vec<BackendStructInstance>,
    pub union_instances: Vec<BackendUnionInstance>,
    pub enums: Vec<BackendEnum>,
    pub globals: Vec<BackendGlobal>,
    pub functions: Vec<BackendFunction>,
    pub function_instances: Vec<BackendFunctionInstance>,
    pub generic_instantiations: Vec<BackendGenericInstantiation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendLayouts {
    pub types: Vec<(TyId, TypeLayout)>,
    pub structs: Vec<(GlobalDefId, StructLayout)>,
    pub unions: Vec<(GlobalDefId, StructLayout)>,
    pub struct_instances: Vec<(BackendStructInstanceKey, StructLayout)>,
    pub union_instances: Vec<(BackendStructInstanceKey, StructLayout)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BackendStructInstanceKey {
    pub def_id: GlobalDefId,
    pub args: Vec<TyId>,
}

impl BackendLayouts {
    pub fn from_module_layouts(module_id: ModuleId, layouts: &Layouts) -> Self {
        Self {
            types: layouts
                .types
                .iter()
                .map(|(ty, layout)| (*ty, layout.clone()))
                .collect(),
            structs: layouts
                .structs
                .iter()
                .map(|(def_id, layout)| {
                    (
                        GlobalDefId {
                            module_id,
                            def_id: *def_id,
                        },
                        layout.clone(),
                    )
                })
                .collect(),
            unions: layouts
                .unions
                .iter()
                .map(|(def_id, layout)| {
                    (
                        GlobalDefId {
                            module_id,
                            def_id: *def_id,
                        },
                        layout.clone(),
                    )
                })
                .collect(),
            struct_instances: layouts
                .struct_instances
                .iter()
                .map(|(key, layout)| {
                    (
                        BackendStructInstanceKey::from_module_key(module_id, key),
                        layout.clone(),
                    )
                })
                .collect(),
            union_instances: layouts
                .union_instances
                .iter()
                .map(|(key, layout)| {
                    (
                        BackendStructInstanceKey::from_module_key(module_id, key),
                        layout.clone(),
                    )
                })
                .collect(),
        }
    }
}

impl BackendStructInstanceKey {
    pub fn from_module_key(module_id: ModuleId, key: &StructLayoutKey) -> Self {
        Self {
            def_id: GlobalDefId {
                module_id,
                def_id: key.def_id,
            },
            args: key.args.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendStruct {
    pub def_id: GlobalDefId,
    pub name: String,
    pub generics: Vec<String>,
    pub fields: Vec<BackendField>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendUnion {
    pub def_id: GlobalDefId,
    pub name: String,
    pub generics: Vec<String>,
    pub fields: Vec<BackendField>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendStructInstance {
    pub def_id: GlobalDefId,
    pub name: String,
    pub args: Vec<TyId>,
    pub symbol: String,
    pub fields: Vec<BackendField>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendUnionInstance {
    pub def_id: GlobalDefId,
    pub name: String,
    pub args: Vec<TyId>,
    pub symbol: String,
    pub fields: Vec<BackendField>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendField {
    pub def_id: GlobalDefId,
    pub name: String,
    pub ty: TyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendEnum {
    pub def_id: GlobalDefId,
    pub name: String,
    pub backing_type: TyId,
    pub variants: Vec<BackendEnumVariant>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendEnumVariant {
    pub def_id: GlobalDefId,
    pub name: String,
    pub value: Option<i128>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendGlobal {
    pub def_id: GlobalDefId,
    pub name: String,
    pub ty: TyId,
    pub is_const: bool,
    pub is_extern: bool,
    pub init: Option<StaticInit>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StaticInit {
    Zero,
    Int(i128),
    Float(String),
    Bool(bool),
    Char(String),
    Byte(u8),
    Bytes(Vec<u8>),
    Array(Vec<StaticInit>),
    Repeat {
        value: Box<StaticInit>,
        count: u64,
    },
    Struct(Vec<StaticFieldInit>),
    NullPtr,
    AddrOfGlobal {
        global: GlobalDefId,
        path: Vec<PlaceElem>,
    },
    AddrOfFunction(GlobalDefId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StaticFieldInit {
    pub field: GlobalDefId,
    pub value: StaticInit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendFunction {
    pub def_id: GlobalDefId,
    pub name: String,
    pub generics: Vec<String>,
    pub params: Vec<BackendParam>,
    pub return_type: TyId,
    pub is_extern: bool,
    pub is_variadic: bool,
    pub body: Option<TypedBody>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendFunctionInstance {
    pub def_id: GlobalDefId,
    pub name: String,
    pub arg_module_id: ModuleId,
    pub args: Vec<TyId>,
    pub symbol: String,
    pub params: Vec<BackendParam>,
    pub return_type: TyId,
    pub is_extern: bool,
    pub is_variadic: bool,
    pub body: Option<TypedBody>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendGenericInstantiation {
    pub def_id: GlobalDefId,
    pub arg_module_id: ModuleId,
    pub args: Vec<TyId>,
    pub span: Span,
    pub source_def_id: Option<GlobalDefId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendParam {
    pub local_id: Option<LocalId>,
    pub name: Option<String>,
    pub receiver: Option<ReceiverKind>,
    pub ty: TyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedBody {
    pub span: Span,
    pub locals: Vec<TypedLocal>,
    pub stmts: Vec<TypedStmt>,
    pub tail: Option<Box<TypedExpr>>,
    pub ty: TyId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedLocal {
    pub id: LocalId,
    pub name: String,
    pub kind: TypedLocalKind,
    pub ty: TyId,
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
    Switch(TypedSwitch),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedBinding {
    pub local_id: LocalId,
    pub name: String,
    pub ty: TyId,
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
    pub ty: TyId,
    pub kind: TypedExprKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedExprKind {
    Error,
    Integer(String),
    Float(String),
    String(Vec<u8>),
    Char(String),
    ByteChar(String),
    Bool(bool),
    Local(LocalId),
    Global(GlobalDefId),
    Function(GlobalDefId),
    FunctionInstance {
        def_id: GlobalDefId,
        args: Vec<TyId>,
    },
    EnumVariant(GlobalDefId),
    BuiltinValue(BuiltinConst),
    Len(Box<TypedExpr>),
    Ptr(Box<TypedExpr>),
    InlineAsm(TypedInlineAsm),
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
    Cast {
        expr: Box<TypedExpr>,
        ty: TyId,
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
        args: Vec<TyId>,
    },
    Method {
        def_id: GlobalDefId,
        args: Vec<TyId>,
        receiver: Box<TypedExpr>,
    },
    FunctionPointer(Box<TypedExpr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedPlace {
    pub span: Span,
    pub ty: TyId,
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
