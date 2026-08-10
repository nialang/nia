// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{AssignOp, BinaryOp, UnaryOp};
use nia_ids::{BuiltinTraitMethod, ClosureId, InternedTyId, LayoutBuiltin, LocalId, ReceiverKind};
pub use nia_ir_names::{GeneratedLocalName, LocalName, PromotedAllocationId};
use nia_span::Span;
use nia_symbol::SymbolId;
use nia_ty::{ArrayLenTy, BuiltinTrait, ConstGenericArg, IntConst, TraitId};

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

/// A generated entry body for one concrete closure-state type.
///
/// `state_param` is the first ABI parameter and always has type
/// `&ClosureState`. Captured locals are rewritten to projections through that
/// pointer and never leak from the containing source function.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionClosureEntry {
    pub closure_id: ClosureId,
    pub state_ty: InternedTyId,
    pub state_param: LocalId,
    pub params: Vec<LocalId>,
    pub return_type: InternedTyId,
    pub body: FunctionBody,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionLocal {
    pub id: LocalId,
    pub name: LocalName,
    pub kind: FunctionLocalKind,
    pub ty: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionLocalKind {
    Param,
    MutableBinding,
    ImmutableBinding,
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
    MemoryIntrinsic(Box<FunctionMemoryIntrinsic>),
    Expr(FunctionExpr),
    Defer(FunctionDeferBody),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionBinding {
    pub local_id: LocalId,
    pub name: LocalName,
    pub ty: InternedTyId,
    pub value: Option<FunctionExpr>,
    pub is_let: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDeferBody {
    pub span: Span,
    pub scopes: Vec<FunctionScope>,
    pub blocks: Vec<FunctionBlock>,
    pub entry: FunctionBlockId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionMemoryIntrinsic {
    pub span: Span,
    pub op: FunctionMemoryIntrinsicOp,
    pub elem_ty: InternedTyId,
    pub dest: FunctionExpr,
    pub source: FunctionMemoryIntrinsicSource,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionMemoryIntrinsicSource {
    Slice(FunctionExpr),
    Byte(FunctionExpr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionMemoryIntrinsicOp {
    Copy,
    Move,
    Set,
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
    Try {
        value: FunctionExpr,
        kind: FunctionTryKind,
        error_conversion: Option<FunctionExpr>,
        success_local: LocalId,
        success_target: FunctionBlockId,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionTryKind {
    Optional,
    ErrorUnion,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionOptionalTag {
    Null = 0,
    Some = 1,
}

impl FunctionOptionalTag {
    pub const fn discriminant(self) -> u8 {
        self as u8
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionErrorUnionTag {
    Ok = 0,
    Err = 1,
}

impl FunctionErrorUnionTag {
    pub const fn discriminant(self) -> u8 {
        self as u8
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionForHeader {
    Infinite,
    Condition(Box<FunctionExpr>),
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
    Null,
    Local(LocalId),
    Global(nia_ids::GlobalDefId),
    ConstGeneric(ConstGenericArg),
    GlobalInstance {
        def_id: nia_ids::GlobalDefId,
        arg_module_id: nia_ids::ModuleId,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
    },
    Function(nia_ids::GlobalDefId),
    FunctionInstance {
        def_id: nia_ids::GlobalDefId,
        arg_module_id: nia_ids::ModuleId,
        self_arg: Option<InternedTyId>,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
    },
    EnumVariant {
        variant: nia_ids::GlobalDefId,
        fields: Vec<FunctionExpr>,
    },
    EnumVariantTag(nia_ids::GlobalDefId),
    EnumTag {
        value: Box<FunctionExpr>,
    },
    EnumPayloadField {
        value: Box<FunctionExpr>,
        variant: nia_ids::GlobalDefId,
        field: usize,
    },
    BuiltinValue(FunctionBuiltinValue),
    Trap,
    Range(FunctionRange),
    RangeBound {
        range: Box<FunctionExpr>,
        bound: FunctionRangeBound,
    },
    InlineAsm(FunctionInlineAsm),
    Atomic(FunctionAtomic),
    LoadUnaligned {
        ty: InternedTyId,
        ptr: Box<FunctionExpr>,
    },
    Splat {
        value: Box<FunctionExpr>,
    },
    ExtractElement {
        vector: Box<FunctionExpr>,
        index: Box<FunctionExpr>,
    },
    InsertElement {
        vector: Box<FunctionExpr>,
        index: Box<FunctionExpr>,
        value: Box<FunctionExpr>,
    },
    Bitmask {
        vector: Box<FunctionExpr>,
    },
    BitIntrinsic {
        op: FunctionBitIntrinsicOp,
        value: Box<FunctionExpr>,
    },
    CharFromU32 {
        value: Box<FunctionExpr>,
    },
    StaticArrayPointer {
        allocation: PromotedAllocationId,
        array: Box<FunctionExpr>,
        is_readonly: bool,
    },
    ArrayLiteral {
        elems: FunctionArrayElements,
    },
    Tuple(Vec<FunctionExpr>),
    TupleField {
        value: Box<FunctionExpr>,
        index: usize,
    },
    StructLiteral {
        def_id: nia_ids::GlobalDefId,
        fields: Vec<FunctionFieldInit>,
    },
    UnionLiteral {
        def_id: nia_ids::GlobalDefId,
        field: Box<FunctionFieldInit>,
    },
    UnionStorageLiteral {
        bytes: Vec<Option<u8>>,
        relocations: Vec<FunctionUnionRelocation>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<FunctionExpr>,
    },
    OptionalSome {
        expr: Box<FunctionExpr>,
    },
    ErrorOk {
        expr: Box<FunctionExpr>,
    },
    ErrorErr {
        expr: Box<FunctionExpr>,
    },
    TaggedUnionTag {
        expr: Box<FunctionExpr>,
    },
    TaggedUnionPayload {
        expr: Box<FunctionExpr>,
    },
    Try {
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
        source_ty: InternedTyId,
        target_ty: InternedTyId,
    },
    TraitObjectCoercion {
        expr: Box<FunctionExpr>,
        target_ty: InternedTyId,
        self_ty: InternedTyId,
    },
    CallableCoercion {
        state: Box<FunctionExpr>,
        closure_id: ClosureId,
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
        is_readonly: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionUnionRelocation {
    pub offset: usize,
    pub width: usize,
    pub allocation: PromotedAllocationId,
    pub pointee: Box<FunctionExpr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionBitIntrinsicOp {
    Ctz,
    Clz,
    Popcount,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionRangeBound {
    Start,
    End,
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
    FieldOffset {
        ty: InternedTyId,
        field: nia_ids::GlobalDefId,
    },
    Int(IntConst),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionAsmOption {
    Volatile,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionAtomic {
    Load {
        ty: InternedTyId,
        ptr: Box<FunctionExpr>,
        order: AtomicOrder,
    },
    Store {
        ty: InternedTyId,
        ptr: Box<FunctionExpr>,
        value: Box<FunctionExpr>,
        order: AtomicOrder,
    },
    Rmw {
        ty: InternedTyId,
        ptr: Box<FunctionExpr>,
        op: AtomicRmwOp,
        value: Box<FunctionExpr>,
        order: AtomicOrder,
    },
    Cmpxchg {
        ty: InternedTyId,
        ptr: Box<FunctionExpr>,
        expected: Box<FunctionExpr>,
        desired: Box<FunctionExpr>,
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
        count: ArrayLenTy,
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
    ClosureEntry {
        closure_id: ClosureId,
        state: Box<FunctionExpr>,
    },
    Function(nia_ids::GlobalDefId),
    FunctionInstance {
        def_id: nia_ids::GlobalDefId,
        arg_module_id: nia_ids::ModuleId,
        self_arg: Option<InternedTyId>,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
    },
    Method {
        def_id: nia_ids::GlobalDefId,
        arg_module_id: nia_ids::ModuleId,
        self_arg: Option<InternedTyId>,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
        receiver_kind: ReceiverKind,
        receiver: Box<FunctionExpr>,
    },
    TraitMethod {
        trait_id: nia_ids::GlobalDefId,
        method_id: nia_ids::GlobalDefId,
        method_name: SymbolId,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        args: Vec<InternedTyId>,
        receiver_kind: ReceiverKind,
        receiver: Box<FunctionExpr>,
    },
    TraitAssociatedFunction {
        trait_id: nia_ids::GlobalDefId,
        method_id: nia_ids::GlobalDefId,
        method_name: SymbolId,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        args: Vec<InternedTyId>,
    },
    DynamicTraitMethod {
        object_ty: InternedTyId,
        trait_id: TraitId,
        method_id: nia_ids::GlobalDefId,
        method_name: SymbolId,
        trait_args: Vec<InternedTyId>,
        slot: usize,
        params: Vec<InternedTyId>,
        return_type: InternedTyId,
        receiver_kind: ReceiverKind,
        receiver: Box<FunctionExpr>,
    },
    BuiltinMethod {
        method: FunctionBuiltinMethod,
        self_ty: InternedTyId,
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
    Callable(Box<FunctionExpr>),
    FunctionPointer(Box<FunctionExpr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FunctionBuiltinOperator {
    pub trait_id: BuiltinTrait,
    pub op: FunctionBuiltinOperatorOp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionBuiltinMethod {
    SliceLen,
    SlicePtr,
    SlicePtrMut,
    Start,
    End,
    Iter,
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
                UnaryOp::RefReadOnly | UnaryOp::Ref | UnaryOp::Deref => None,
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
    GlobalInstance {
        def_id: nia_ids::GlobalDefId,
        arg_module_id: nia_ids::ModuleId,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
    },
    Deref(Box<FunctionExpr>),
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionPlaceElem {
    Field(nia_ids::GlobalDefId),
    TupleField(usize),
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
            FunctionTerminator::Try { success_target, .. } => vec![*success_target],
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
        exited_scopes_between(&self.scopes, from, Some(to))
    }

    pub fn return_exited_scopes(&self, from: FunctionBlockId) -> Option<Vec<FunctionScopeId>> {
        let from = self.block(from)?.scope;
        exited_scopes_between(&self.scopes, from, None)
    }

    pub fn exited_scopes_between(
        &self,
        from: FunctionScopeId,
        to: Option<FunctionScopeId>,
    ) -> Option<Vec<FunctionScopeId>> {
        exited_scopes_between(&self.scopes, from, to)
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
        exited_scopes_between(&self.scopes, from, Some(to))
    }

    pub fn return_exited_scopes(&self, from: FunctionBlockId) -> Option<Vec<FunctionScopeId>> {
        let from = self.block(from)?.scope;
        exited_scopes_between(&self.scopes, from, None)
    }

    pub fn exited_scopes_between(
        &self,
        from: FunctionScopeId,
        to: Option<FunctionScopeId>,
    ) -> Option<Vec<FunctionScopeId>> {
        exited_scopes_between(&self.scopes, from, to)
    }
}

fn exited_scopes_between(
    scopes: &[FunctionScope],
    from: FunctionScopeId,
    to: Option<FunctionScopeId>,
) -> Option<Vec<FunctionScopeId>> {
    let from_chain = scope_chain_to_root(scopes, from)?;
    let to_chain = match to {
        Some(scope) => scope_chain_to_root(scopes, scope)?,
        None => Vec::new(),
    };
    let lca = from_chain
        .iter()
        .find(|scope| to_chain.contains(scope))
        .copied();
    // Defer emission treats scope exit as stack unwinding: leave the source scope first,
    // then its parents, stopping before the lowest common ancestor shared with the target.
    Some(
        from_chain
            .into_iter()
            .take_while(|scope| Some(*scope) != lca)
            .collect(),
    )
}

fn scope_chain_to_root(
    scopes: &[FunctionScope],
    scope: FunctionScopeId,
) -> Option<Vec<FunctionScopeId>> {
    let mut chain = Vec::new();
    let mut current = Some(scope);
    while let Some(scope) = current {
        chain.push(scope);
        current = scopes
            .iter()
            .find(|candidate| candidate.id == scope)?
            .parent;
    }
    Some(chain)
}
