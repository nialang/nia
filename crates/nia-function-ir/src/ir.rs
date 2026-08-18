// SPDX-License-Identifier: GPL-3.0-or-later
//! Function-level control-flow IR shared by lowering, optimization, and codegen.
//!
//! A [`FunctionBody`] owns flat tables of locals, lexical scopes, and basic
//! blocks. References use stable ids rather than vector indices so optimization
//! passes may remove or reorder blocks without rewriting unrelated data. Use
//! [`crate::validate_function_body`] at every producer/consumer boundary: the
//! structs deliberately remain easy to transform, so their cross-table
//! invariants are enforced by validation rather than private constructors.
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
/// A validated function CFG and its flat local/scope metadata tables.
pub struct FunctionBody {
    /// Span used when a body-wide invariant has no narrower source location.
    pub span: Span,
    /// Storage declarations keyed by [`FunctionLocal::id`].
    pub locals: Vec<FunctionLocal>,
    /// Lexical ownership tree used to schedule defer cleanup.
    pub scopes: Vec<FunctionScope>,
    /// Basic blocks keyed by [`FunctionBlock::id`].
    pub blocks: Vec<FunctionBlock>,
    /// First block executed when entering the function.
    pub entry: FunctionBlockId,
    /// Type of the lowered body expression, which may be `Never` even when the
    /// enclosing function signature has a concrete return type.
    pub ty: InternedTyId,
}

/// A generated entry body for one concrete closure-state type.
///
/// `state_param` is the first ABI parameter and always has type
/// `&ClosureState`. Captured locals are rewritten to projections through that
/// pointer and never leak from the containing source function.
#[derive(Debug, Clone, PartialEq)]
/// One closure entry point embedded in its enclosing function owner.
pub struct FunctionClosureEntry {
    /// Stable identity shared with closure construction and callable adapters.
    pub closure_id: ClosureId,
    /// Aggregate capture-state type addressed by `state_param`.
    pub state_ty: InternedTyId,
    /// Local receiving the generated readonly state pointer.
    pub state_param: LocalId,
    /// User-visible parameters in ABI order, excluding the state pointer.
    pub params: Vec<LocalId>,
    /// Declared result type used for closure call ABI classification.
    pub return_type: InternedTyId,
    /// Closure-local CFG; its flat local table also contains the generated parameters.
    pub body: FunctionBody,
}

#[derive(Debug, Clone, PartialEq)]
/// A storage slot in the body's flat local table.
pub struct FunctionLocal {
    /// Stable key referenced by expressions, places, and ABI parameter mappings.
    pub id: LocalId,
    pub name: LocalName,
    /// Declares whether the slot is user storage, generated storage, or a parameter.
    pub kind: FunctionLocalKind,
    /// Physical storage type; typed expression views may be more readonly.
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
/// One lexical scope in the defer-cleanup ownership tree.
pub struct FunctionScope {
    pub id: FunctionScopeId,
    /// Enclosing scope, or `None` for the function root.
    pub parent: Option<FunctionScopeId>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
/// A basic block whose operations execute in order before its terminator.
pub struct FunctionBlock {
    pub id: FunctionBlockId,
    /// Lexical scope active for exits originating in this block.
    pub scope: FunctionScopeId,
    pub span: Span,
    pub ops: Vec<FunctionOp>,
    pub terminator: FunctionTerminator,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionOp {
    Binding(FunctionBinding),
    /// Initializes or merges a control-flow value into exactly typed local storage.
    ///
    /// This internal write may initialize an immutable source binding; source
    /// mutability governs later assignment expressions, not CFG construction.
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
/// An effect-only bulk memory operation over slice storage.
///
/// `dest` must be a mutable slice of `elem_ty`. Copy and move operations pair
/// with a slice source of the same element type, while set operations require
/// both `elem_ty` and their byte source to be `u8`.
pub struct FunctionMemoryIntrinsic {
    pub span: Span,
    pub op: FunctionMemoryIntrinsicOp,
    pub elem_ty: InternedTyId,
    pub dest: FunctionExpr,
    pub source: FunctionMemoryIntrinsicSource,
}

#[derive(Debug, Clone, PartialEq)]
/// The source shape paired with a [`FunctionMemoryIntrinsic`] operation.
pub enum FunctionMemoryIntrinsicSource {
    Slice(FunctionExpr),
    Byte(FunctionExpr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The overlap contract of a [`FunctionMemoryIntrinsic`].
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
    /// Transitional propagation node consumed into a CFG terminator by function lowering.
    /// It is invalid at the backend boundary.
    Try {
        value: FunctionExpr,
        kind: FunctionTryKind,
        // Error conversion is absent for optionals and for identical error
        // payloads. Keep the uncommon second expression out-of-line so every
        // basic block does not inherit the size of two full expression trees.
        error_conversion: Option<Box<FunctionExpr>>,
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
    /// An integer literal whose target type is resolved before LLVM lowering.
    Integer(String),
    /// An `f32` or `f64` literal in its source spelling.
    Float(String),
    /// A Unicode scalar array literal with a compiler-known array type.
    String(Vec<u32>),
    /// A byte array literal with a compiler-known array type.
    ByteString(Vec<u8>),
    /// A Unicode scalar literal, represented as the `char` primitive.
    Char(u32),
    /// A byte character literal, represented as `u8`.
    ByteChar(String),
    /// A boolean literal.
    Bool(bool),
    /// The null/failure discriminant of an Optional or ErrorUnion.
    Null,
    /// A use of local storage through the expression's current typed view.
    ///
    /// `FunctionExpr::ty` need not equal the local table type: address-taking
    /// and coercion lowering can reinterpret the same storage through a pointer
    /// or slice view. Operations that write a local (`Binding` and `StoreLocal`)
    /// retain the stronger storage-type contract.
    Local(LocalId),
    /// Loads a global through its declared storage type or a readonly-qualified view.
    Global(nia_ids::GlobalDefId),
    ConstGeneric(ConstGenericArg),
    /// Loads one concrete generic global instance through its published storage type.
    GlobalInstance {
        def_id: nia_ids::GlobalDefId,
        arg_module_id: nia_ids::ModuleId,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
    },
    /// Materializes a non-generic function as its exact source-level function pointer type.
    Function(nia_ids::GlobalDefId),
    /// Materializes one concrete generic function instance as its exact function pointer type.
    FunctionInstance {
        def_id: nia_ids::GlobalDefId,
        arg_module_id: nia_ids::ModuleId,
        self_arg: Option<InternedTyId>,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
    },
    /// Constructs an enum variant, storing payload fields in declaration order.
    EnumVariant {
        variant: nia_ids::GlobalDefId,
        fields: Vec<FunctionExpr>,
    },
    /// Produces a variant's backing integer tag for pattern comparisons.
    EnumVariantTag(nia_ids::GlobalDefId),
    /// Extracts an enum tag from either its aggregate representation or backing integer.
    EnumTag {
        value: Box<FunctionExpr>,
    },
    /// Loads one payload field from a value known to have the selected variant.
    EnumPayloadField {
        value: Box<FunctionExpr>,
        variant: nia_ids::GlobalDefId,
        field: usize,
    },
    BuiltinValue(FunctionBuiltinValue),
    Trap,
    /// Constructs a range whose bound presence and types follow its `RangeTyKind`.
    Range(FunctionRange),
    /// Extracts one statically present bound from a range value.
    RangeBound {
        range: Box<FunctionExpr>,
        bound: FunctionRangeBound,
    },
    InlineAsm(FunctionInlineAsm),
    Atomic(FunctionAtomic),
    /// Loads `ty` at byte alignment from a readonly or mutable `u8` pointer.
    LoadUnaligned {
        ty: InternedTyId,
        ptr: Box<FunctionExpr>,
    },
    /// Broadcasts one scalar lane into the result SIMD vector type.
    Splat {
        value: Box<FunctionExpr>,
    },
    /// Extracts one lane using an integer index; the result is the lane type.
    ExtractElement {
        vector: Box<FunctionExpr>,
        index: Box<FunctionExpr>,
    },
    /// Replaces one lane and returns the same SIMD vector type.
    InsertElement {
        vector: Box<FunctionExpr>,
        index: Box<FunctionExpr>,
        value: Box<FunctionExpr>,
    },
    /// Packs bool lanes, up to the target `usize` width, into a `usize` result.
    Bitmask {
        vector: Box<FunctionExpr>,
    },
    /// Applies an LLVM integer bit-counting intrinsic without changing type.
    BitIntrinsic {
        op: FunctionBitIntrinsicOp,
        value: Box<FunctionExpr>,
    },
    /// Converts a `u32` scalar to `Optional[char]` after validity checks.
    CharFromU32 {
        value: Box<FunctionExpr>,
    },
    /// Materializes promoted array storage and returns a pointer to the complete array value.
    ///
    /// The result pointer element is the type of `array`; `is_readonly` mirrors
    /// the result pointer qualifier and records whether the promotion is frozen.
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
    /// Initializes every declared field of one exact nominal struct instance.
    StructLiteral {
        def_id: nia_ids::GlobalDefId,
        fields: Vec<FunctionFieldInit>,
    },
    /// Initializes the selected storage member of one exact nominal union instance.
    UnionLiteral {
        def_id: nia_ids::GlobalDefId,
        field: Box<FunctionFieldInit>,
    },
    /// Reconstructs the byte representation of a const-evaluated union.
    ///
    /// Relocations are sorted, non-overlapping target-pointer ranges within
    /// `bytes`; each pointee becomes separately promoted storage whose address
    /// replaces the covered initialized bytes. Function IR validation enforces
    /// these structural rules before backend target-width validation.
    UnionStorageLiteral {
        bytes: Vec<Option<u8>>,
        relocations: Vec<FunctionUnionRelocation>,
    },
    /// A typed unary operation; references to ordinary places are lowered to
    /// [`FunctionExprKind::AddrOf`] before this backend boundary.
    Unary {
        op: UnaryOp,
        expr: Box<FunctionExpr>,
    },
    /// Constructs an `Optional` value with the `Some` discriminant.
    OptionalSome {
        expr: Box<FunctionExpr>,
    },
    /// Constructs an `ErrorUnion` value carrying its success payload.
    ErrorOk {
        expr: Box<FunctionExpr>,
    },
    /// Constructs an `ErrorUnion` value carrying its error payload.
    ErrorErr {
        expr: Box<FunctionExpr>,
    },
    /// Extracts the one-byte discriminant from an optional or error union.
    TaggedUnionTag {
        expr: Box<FunctionExpr>,
    },
    /// Extracts the active payload from an optional or error union.
    TaggedUnionPayload {
        expr: Box<FunctionExpr>,
    },
    Try {
        expr: Box<FunctionExpr>,
    },
    AddrOf(FunctionPlace),
    /// A typed scalar/vector operation whose result is either the operand type
    /// or a bool mask for comparisons.
    Binary {
        lhs: Box<FunctionExpr>,
        op: BinaryOp,
        rhs: Box<FunctionExpr>,
    },
    /// Writes `rhs` into an exactly typed, writable place and evaluates to unit.
    ///
    /// Compound forms apply the corresponding builtin binary operation to the
    /// current place value and `rhs`; that operation must produce `place.ty`.
    Assign {
        place: FunctionPlace,
        op: AssignOp,
        rhs: Box<FunctionExpr>,
    },
    /// Evaluates its operand for effects and produces unit.
    Discard(Box<FunctionExpr>),
    /// Converts between source-approved numeric, enum, and pointer categories.
    Cast {
        expr: Box<FunctionExpr>,
        ty: InternedTyId,
    },
    /// Repoints an existing trait object at a supertrait vtable.
    TraitObjectUpcast {
        expr: Box<FunctionExpr>,
        source_ty: InternedTyId,
        target_ty: InternedTyId,
    },
    /// Builds a trait object from a pointer/slice data view and a concrete vtable.
    TraitObjectCoercion {
        expr: Box<FunctionExpr>,
        target_ty: InternedTyId,
        self_ty: InternedTyId,
    },
    /// Pairs a closure-state pointer with its generated entry as a callable fat pointer.
    ///
    /// The state, closure identity, callable signature, and owner-qualified
    /// backend entry must describe the same closure instance.
    CallableCoercion {
        state: Box<FunctionExpr>,
        closure_id: ClosureId,
    },
    /// Selects the generated adapter for a non-capturing closure.
    ClosureFunctionPointer {
        closure_id: ClosureId,
    },
    Call {
        callee: FunctionCallee,
        args: Vec<FunctionExpr>,
    },
    /// Loads a field from a nominal aggregate or a pointer to one.
    Field {
        lhs: Box<FunctionExpr>,
        field: nia_ids::GlobalDefId,
    },
    /// Loads one element from an array, pointer, or slice base.
    Index {
        lhs: Box<FunctionExpr>,
        index: Box<FunctionExpr>,
    },
    /// Creates a fat slice view over an array, pointer, or existing slice.
    Slice {
        lhs: Box<FunctionExpr>,
        range: FunctionSliceRange,
        is_readonly: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
/// A pointer relocation embedded in a const-evaluated union byte image.
pub struct FunctionUnionRelocation {
    /// First byte replaced by the promoted pointee address.
    pub offset: usize,
    /// Number of replaced bytes; backend validation requires target pointer size.
    pub width: usize,
    /// Stable identity of the separately materialized promoted allocation.
    pub allocation: PromotedAllocationId,
    /// Constant value used to initialize that allocation.
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
    /// Optional lower bound; omitted bounds start at zero.
    pub start: Option<Box<FunctionExpr>>,
    /// Optional upper bound; omitted bounds use the source length.
    pub end: Option<Box<FunctionExpr>>,
    /// Whether the upper bound is inclusive and must be incremented.
    pub inclusive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionRange {
    /// Present only for range kinds with a lower bound.
    pub start: Option<Box<FunctionExpr>>,
    /// Present only for range kinds with an upper bound.
    pub end: Option<Box<FunctionExpr>>,
    /// Mirrors the inclusive range kind selected in the expression type.
    pub inclusive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionRangeBound {
    Start,
    End,
}

#[derive(Debug, Clone, PartialEq)]
/// LLVM-style inline assembly after source constraint checking.
pub struct FunctionInlineAsm {
    /// Assembly template passed verbatim to the target backend.
    pub code: String,
    /// Input constraints and values in template operand order.
    pub inputs: Vec<FunctionAsmInput>,
    /// Output constraints and writable destination places in operand order.
    pub outputs: Vec<FunctionAsmOutput>,
    /// Target register or state clobbers appended to the constraint list.
    pub clobbers: Vec<String>,
    /// Behavioral flags that affect optimization and side-effect classification.
    pub options: Vec<FunctionAsmOption>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunctionBuiltinValue {
    /// A target-width `usize` constant.
    Usize(u64),
    /// The target layout size or alignment of a runtime-representable type.
    Layout {
        builtin: LayoutBuiltin,
        ty: InternedTyId,
    },
    /// The byte offset of a declared field in its aggregate type.
    FieldOffset {
        ty: InternedTyId,
        field: nia_ids::GlobalDefId,
    },
    /// A target-typed integer bit pattern produced by constant evaluation.
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
/// One inline-assembly input after type checking.
pub struct FunctionAsmInput {
    /// Backend constraint corresponding to this operand.
    pub constraint: String,
    /// Scalar value passed directly through LLVM's inline-assembly call boundary.
    pub value: FunctionExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
/// One inline-assembly output stored into a writable place after execution.
pub struct FunctionAsmOutput {
    /// Backend output constraint corresponding to this operand.
    pub constraint: String,
    /// Exact writable scalar storage updated with the corresponding result.
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
        /// Required to select the correct const-generic extension impl.
        trait_const_args: Vec<ConstGenericArg>,
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
        /// Required to select the correct const-generic extension impl.
        trait_const_args: Vec<ConstGenericArg>,
        args: Vec<InternedTyId>,
    },
    DynamicTraitMethod {
        object_ty: InternedTyId,
        trait_id: TraitId,
        method_id: nia_ids::GlobalDefId,
        method_name: SymbolId,
        trait_args: Vec<InternedTyId>,
        /// Retains the complete trait-object identity for vtable dispatch.
        trait_const_args: Vec<ConstGenericArg>,
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
/// A typed addressable path rooted in local, global, or dereferenced storage.
///
/// `ty` is the final selected value type after applying `elems`; it may be a
/// readonly-qualified view of mutable storage. Backend consumers must derive
/// the path from the base type rather than trusting this summary type alone.
pub struct FunctionPlace {
    pub span: Span,
    pub ty: InternedTyId,
    /// Storage root from which projection begins.
    pub base: FunctionPlaceBase,
    /// Declaration-ordered projections applied from left to right.
    pub elems: Vec<FunctionPlaceElem>,
}

#[derive(Debug, Clone, PartialEq)]
/// Root storage for a [`FunctionPlace`].
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
/// One typed projection in an addressable storage path.
pub enum FunctionPlaceElem {
    Field(nia_ids::GlobalDefId),
    TupleField(usize),
    Index(Box<FunctionExpr>),
    Error,
}

impl FunctionTerminator {
    /// Returns blocks that control can enter immediately after this terminator.
    ///
    /// This is a CFG operation, not a complete reference walk. In particular,
    /// `Loop::continue_target` is metadata used by lowered `continue` edges, and
    /// a `Switch` fallback is inactive when an explicit default exists. Use
    /// [`Self::referenced_blocks`] when validating or rewriting stored block ids.
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

    /// Returns every block id stored by this terminator.
    ///
    /// Unlike [`Self::successors`], this includes inactive structural metadata:
    /// the switch fallback even when a default is present, and a loop's
    /// `continue_target`. Optimizers must keep these references valid because a
    /// later transformation can make them operational again.
    pub fn referenced_blocks(&self) -> Vec<FunctionBlockId> {
        match self {
            FunctionTerminator::Error { .. }
            | FunctionTerminator::Return { .. }
            | FunctionTerminator::Tail { .. } => Vec::new(),
            FunctionTerminator::Branch { target, .. } | FunctionTerminator::Next { target, .. } => {
                vec![*target]
            }
            FunctionTerminator::Try { success_target, .. } => vec![*success_target],
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
            } => {
                let mut targets = arms.iter().map(|arm| arm.target).collect::<Vec<_>>();
                targets.extend(default.iter().copied());
                targets.push(*fallback);
                targets
            }
            FunctionTerminator::Loop {
                body,
                continue_target,
                break_target,
                ..
            } => vec![*body, *continue_target, *break_target],
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

/// Computes the lexical scopes unwound by a control-flow edge.
///
/// Scope chains are stored child-to-root. Their first common element is the
/// lowest common ancestor, so the prefix before it is exactly the sequence of
/// scopes whose defers must run, innermost first. A return has no destination
/// scope and therefore unwinds the complete source chain.
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
