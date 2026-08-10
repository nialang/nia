// SPDX-License-Identifier: GPL-3.0-or-later
use std::{collections::HashMap, sync::Arc};

use nia_ast::{AssignOp, BinaryOp, UnaryOp};
use nia_ids::{
    BuiltinTraitMethod, GlobalDefId, InternedTyId, LayoutBuiltin, LocalId, ReceiverKind,
};
pub use nia_ir_names::{GeneratedLocalName, LocalName, PromotedAllocationId};
pub use nia_sema_ir::{
    BracketSuffixResolution, BuiltinMethod, BuiltinOperatorOp, BuiltinValue, FunctionReference,
    GenericInstantiation, PointerArrayToSliceCoercion, ResolvedCall, TraitObjectCoercion,
    TraitObjectUpcast,
};
use nia_span::Span;
use nia_static_ir::StaticInit;
use nia_symbol::SymbolId;
use nia_ty::IntConst;
use nia_ty::{ArrayLenTy, BuiltinTrait, ConstGenericArg, TraitId};

#[derive(Debug, Clone, PartialEq)]
pub struct BodyIr {
    pub function_bodies: HashMap<GlobalDefId, Arc<TypedBody>>,
    pub global_inits: HashMap<GlobalDefId, Arc<StaticInit>>,
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
    pub name: LocalName,
    pub kind: TypedLocalKind,
    pub ty: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedLocalKind {
    Param,
    MutableBinding,
    ImmutableBinding,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedStmt {
    pub span: Span,
    pub kind: TypedStmtKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedStmtKind {
    Binding(TypedBinding),
    PatternBinding(Box<TypedPatternBinding>),
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
pub struct TypedPatternBinding {
    pub pattern: TypedPattern,
    pub value: TypedExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedBinding {
    pub local_id: LocalId,
    pub name: LocalName,
    pub ty: InternedTyId,
    pub value: Option<TypedExpr>,
    pub is_mutable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedForIn {
    pub pattern: TypedPattern,
    pub item_ty: InternedTyId,
    pub bool_ty: InternedTyId,
    pub iterable_self_ty: InternedTyId,
    pub iterator_ty: InternedTyId,
    pub iter: TypedExpr,
    pub body: TypedBody,
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
    pub patterns: Vec<TypedPattern>,
    pub body: TypedSwitchArmBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedIfPattern {
    pub target: TypedExpr,
    pub bool_ty: InternedTyId,
    pub pattern: TypedPattern,
    pub then_branch: TypedBody,
    pub else_branch: Option<Box<TypedExpr>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedPattern {
    pub ty: InternedTyId,
    pub span: Span,
    pub kind: TypedPatternKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedPatternKind {
    Wildcard,
    Bind {
        local_id: LocalId,
        name: LocalName,
    },
    Pointer(Box<TypedPattern>),
    MutPointer(Box<TypedPattern>),
    OptionalSome(Box<TypedPattern>),
    OptionalNull,
    ErrorOk(Box<TypedPattern>),
    ErrorErr(Box<TypedPattern>),
    Tuple(Vec<TypedPattern>),
    EnumVariant {
        variant: GlobalDefId,
        backing_type: InternedTyId,
        fields: Vec<TypedPattern>,
    },
    Expr(TypedExpr),
    CheckedInt {
        value: i128,
    },
    Range {
        start: Box<TypedExpr>,
        end: Box<TypedExpr>,
        inclusive: bool,
    },
    CheckedIntRange {
        start: i128,
        end: i128,
        inclusive: bool,
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
    ConstGeneric(nia_ty::ConstGenericArg),
    Function(GlobalDefId),
    FunctionInstance {
        def_id: GlobalDefId,
        arg_module_id: nia_ids::ModuleId,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
    },
    EnumVariant {
        variant: GlobalDefId,
        fields: Vec<TypedExpr>,
    },
    Tuple(Vec<TypedExpr>),
    Closure {
        closure_id: nia_ids::ClosureId,
        captures: Vec<TypedClosureCapture>,
        params: Vec<LocalId>,
        body: TypedBody,
    },
    BuiltinValue(BuiltinConst),
    Trap,
    Range(TypedRange),
    InlineAsm(TypedInlineAsm),
    MemoryIntrinsic(TypedMemoryIntrinsic),
    Atomic(TypedAtomic),
    LoadUnaligned {
        ty: InternedTyId,
        ptr: Box<TypedExpr>,
    },
    Splat {
        value: Box<TypedExpr>,
    },
    ExtractElement {
        vector: Box<TypedExpr>,
        index: Box<TypedExpr>,
    },
    InsertElement {
        vector: Box<TypedExpr>,
        index: Box<TypedExpr>,
        value: Box<TypedExpr>,
    },
    Bitmask {
        vector: Box<TypedExpr>,
    },
    BitIntrinsic {
        op: TypedBitIntrinsicOp,
        value: Box<TypedExpr>,
    },
    CharFromU32 {
        value: Box<TypedExpr>,
    },
    StaticArrayPointer {
        allocation: PromotedAllocationId,
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
    UnionStorageLiteral {
        bytes: Vec<Option<u8>>,
        relocations: Vec<TypedUnionRelocation>,
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
        error_conversion: Option<TypedTryErrorConversion>,
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
    TupleField {
        lhs: Box<TypedExpr>,
        index: usize,
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
    IfPattern(Box<TypedIfPattern>),
    Switch(Box<TypedSwitch>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedClosureCapture {
    pub local_id: LocalId,
    pub value: TypedExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedTryErrorConversion {
    pub trait_id: GlobalDefId,
    pub method_id: GlobalDefId,
    pub method_name: SymbolId,
    pub source_ty: InternedTyId,
    pub target_ty: InternedTyId,
    pub trait_args: Vec<InternedTyId>,
    pub receiver_kind: ReceiverKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedUnionRelocation {
    pub offset: usize,
    pub width: usize,
    pub allocation: PromotedAllocationId,
    pub pointee: Box<TypedExpr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedBitIntrinsicOp {
    Ctz,
    Clz,
    Popcount,
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
    FieldOffset {
        ty: InternedTyId,
        field: GlobalDefId,
    },
    Int(IntConst),
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
pub struct TypedMemoryIntrinsic {
    pub op: MemoryIntrinsicOp,
    pub elem_ty: InternedTyId,
    pub dest: Box<TypedExpr>,
    pub source: TypedMemoryIntrinsicSource,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedMemoryIntrinsicSource {
    Slice(Box<TypedExpr>),
    Byte(Box<TypedExpr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryIntrinsicOp {
    Copy,
    Move,
    Set,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedAtomic {
    Load {
        ty: InternedTyId,
        ptr: Box<TypedExpr>,
        order: AtomicOrder,
    },
    Store {
        ty: InternedTyId,
        ptr: Box<TypedExpr>,
        value: Box<TypedExpr>,
        order: AtomicOrder,
    },
    Rmw {
        ty: InternedTyId,
        ptr: Box<TypedExpr>,
        op: AtomicRmwOp,
        value: Box<TypedExpr>,
        order: AtomicOrder,
    },
    Cmpxchg {
        ty: InternedTyId,
        ptr: Box<TypedExpr>,
        expected: Box<TypedExpr>,
        desired: Box<TypedExpr>,
        success: AtomicOrder,
        failure: AtomicOrder,
        weak: bool,
    },
    Fence {
        order: AtomicOrder,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicOrder {
    Unordered,
    Monotonic,
    Acquire,
    Release,
    AcqRel,
    SeqCst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicRmwOp {
    Xchg,
    Add,
    Sub,
    And,
    Nand,
    Or,
    Xor,
    Max,
    Min,
    UMax,
    UMin,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedArrayElements {
    List(Vec<TypedExpr>),
    Repeat {
        value: Box<TypedExpr>,
        count: ArrayLenTy,
    },
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
    Closure(Box<TypedExpr>),
    Function(GlobalDefId),
    FunctionInstance {
        def_id: GlobalDefId,
        arg_module_id: nia_ids::ModuleId,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
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
        method_name: SymbolId,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        args: Vec<InternedTyId>,
        receiver_kind: ReceiverKind,
        receiver: Box<TypedExpr>,
    },
    TraitAssociatedFunction {
        trait_id: GlobalDefId,
        method_id: GlobalDefId,
        method_name: SymbolId,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        args: Vec<InternedTyId>,
    },
    DynamicTraitMethod {
        object_ty: InternedTyId,
        trait_id: TraitId,
        method_id: GlobalDefId,
        method_name: SymbolId,
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
    TupleField(usize),
    Index(Box<TypedExpr>),
    Error,
}
