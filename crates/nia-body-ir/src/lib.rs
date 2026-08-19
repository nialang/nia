// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed semantic body IR consumed by function lowering and body walkers.
//!
//! The tables retain source spans and semantic types while preserving enough
//! structure for pattern, closure, defer, and aggregate lowering. Consumers
//! should treat the typed nodes as validated output of body checking rather
//! than as a recovery-friendly syntax tree.
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

mod walk;

pub use walk::walk_typed_function_bodies;

/// Module-owned typed bodies and static initializers.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyIr {
    /// Function definitions keyed by their global identity.
    pub function_bodies: HashMap<GlobalDefId, Arc<TypedBody>>,
    /// Static/global initializers keyed by global identity.
    pub global_inits: HashMap<GlobalDefId, Arc<StaticInit>>,
}

/// Typed statement body with locals, statements, and an optional tail value.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedBody {
    /// Source span covering the body.
    pub span: Span,
    /// Local storage declarations visible in the body.
    pub locals: Vec<TypedLocal>,
    /// Statements in source order.
    pub stmts: Vec<TypedStmt>,
    /// Optional final expression value.
    pub tail: Option<Box<TypedExpr>>,
    /// Semantic result type of the body.
    pub ty: InternedTyId,
}

/// Typed local declaration used by body lowering.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedLocal {
    /// Stable local identity.
    pub id: LocalId,
    /// Source or generated local name.
    pub name: LocalName,
    /// Binding/storage role.
    pub kind: TypedLocalKind,
    /// Semantic storage type.
    pub ty: InternedTyId,
    /// Source declaration span.
    pub span: Span,
}

/// Storage role of a typed local.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedLocalKind {
    /// ABI parameter.
    Param,
    /// Mutable source binding.
    MutableBinding,
    /// Immutable source binding.
    ImmutableBinding,
}

/// One typed statement and its source span.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedStmt {
    /// Source span of the statement.
    pub span: Span,
    /// Statement operation.
    pub kind: TypedStmtKind,
}

/// Statement operation after semantic checking.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedStmtKind {
    /// Local binding declaration.
    Binding(TypedBinding),
    /// Pattern destructuring assignment.
    PatternBinding(Box<TypedPatternBinding>),
    /// Effect or value expression statement.
    Expr(TypedExpr),
    /// Function return with an optional value.
    Return(Option<TypedExpr>),
    /// Break from the nearest loop.
    Break,
    /// Continue at the nearest loop header.
    Continue,
    /// Deferred effect body.
    Defer(TypedExpr),
    /// Iterator-driven loop.
    ForIn(Box<TypedForIn>),
    /// Conditional loop.
    While(Box<TypedWhile>),
    /// Unconditional loop.
    Loop(Box<TypedLoop>),
}

/// Pattern binding statement.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedPatternBinding {
    /// Pattern receiving the value.
    pub pattern: TypedPattern,
    /// Value being destructured.
    pub value: TypedExpr,
}

/// Typed local binding declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedBinding {
    /// Local storage identity.
    pub local_id: LocalId,
    /// Source or generated name.
    pub name: LocalName,
    /// Binding storage type.
    pub ty: InternedTyId,
    /// Optional initializer expression.
    pub value: Option<TypedExpr>,
    /// Whether subsequent assignment is permitted.
    pub is_mutable: bool,
}

/// Iterator-driven `for` loop after method resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedForIn {
    /// Pattern receiving each item.
    pub pattern: TypedPattern,
    /// Item type yielded by the iterator.
    pub item_ty: InternedTyId,
    /// Boolean type used by iterator predicates.
    pub bool_ty: InternedTyId,
    /// Receiver type used to resolve iteration methods.
    pub iterable_self_ty: InternedTyId,
    /// Concrete iterator type.
    pub iterator_ty: InternedTyId,
    /// Iterable expression.
    pub iter: TypedExpr,
    /// Loop body.
    pub body: TypedBody,
}

/// Conditional `while` loop.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedWhile {
    /// Boolean condition.
    pub cond: TypedExpr,
    /// Loop body.
    pub body: TypedBody,
}

/// Unconditional loop body.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedLoop {
    /// Loop body.
    pub body: TypedBody,
}

/// Typed match expression or statement arms.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedMatch {
    /// Value being matched.
    pub target: TypedExpr,
    /// Boolean type used by lowered pattern tests.
    pub bool_ty: InternedTyId,
    /// Arms in source order.
    pub arms: Vec<TypedMatchArm>,
}

/// One typed match arm.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedMatchArm {
    /// Patterns accepted by this arm.
    pub patterns: Vec<TypedPattern>,
    /// Arm body representation.
    pub body: TypedMatchArmBody,
    /// Source span of the arm.
    pub span: Span,
}

/// Typed `if` pattern construct.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedIfPattern {
    /// Value tested by the pattern.
    pub target: TypedExpr,
    /// Boolean type used by the test.
    pub bool_ty: InternedTyId,
    /// Pattern being tested.
    pub pattern: TypedPattern,
    /// Branch taken on success.
    pub then_branch: TypedBody,
    /// Optional expression on failure.
    pub else_branch: Option<Box<TypedExpr>>,
}

/// Typed pattern with a semantic type.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedPattern {
    /// Semantic type matched by the pattern.
    pub ty: InternedTyId,
    /// Source span of the pattern.
    pub span: Span,
    /// Pattern operation.
    pub kind: TypedPatternKind,
}

/// Pattern operation after type checking.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedPatternKind {
    /// Matches any value.
    Wildcard,
    /// Binds the matched value to a local.
    Bind {
        /// Bound local identity.
        local_id: LocalId,
        /// Bound local name.
        name: LocalName,
    },
    /// Readonly pointer pattern.
    Pointer(Box<TypedPattern>),
    /// Mutable pointer pattern.
    MutPointer(Box<TypedPattern>),
    /// Optional payload pattern.
    OptionalSome(Box<TypedPattern>),
    /// Optional null pattern.
    OptionalNull,
    /// Error-union success payload pattern.
    ErrorOk(Box<TypedPattern>),
    /// Error-union error payload pattern.
    ErrorErr(Box<TypedPattern>),
    /// Tuple destructuring pattern.
    Tuple(Vec<TypedPattern>),
    /// Nominal struct or enum pattern.
    Nominal {
        /// Constructor identity and field mapping.
        constructor: TypedNominalPatternConstructor,
        /// Field patterns in declaration order.
        fields: Vec<TypedPattern>,
    },
    /// Expression equality pattern.
    Expr(TypedExpr),
    /// Checked integer constant pattern.
    CheckedInt {
        /// Target-width integer value.
        value: i128,
    },
    /// Runtime range pattern.
    Range {
        /// Lower bound.
        start: Box<TypedExpr>,
        /// Upper bound.
        end: Box<TypedExpr>,
        /// Whether the upper bound is inclusive.
        inclusive: bool,
    },
    /// Compile-time checked integer range.
    CheckedIntRange {
        /// Lower bound.
        start: i128,
        /// Upper bound.
        end: i128,
        /// Whether the upper bound is inclusive.
        inclusive: bool,
    },
}

/// Constructor metadata for a nominal pattern.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedNominalPatternConstructor {
    /// Struct field pattern.
    Struct {
        /// Field identities in declaration order.
        field_defs: Vec<GlobalDefId>,
    },
    /// Enum variant pattern.
    EnumVariant {
        /// Variant identity.
        variant: GlobalDefId,
        /// Integer backing type for its discriminant.
        backing_type: InternedTyId,
    },
}

/// Body shape attached to one match arm.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedMatchArmBody {
    // Match bodies are stored per arm. Box the full expression so statement
    // and block arms do not inherit `TypedExpr`'s substantially larger size.
    /// Value expression arm.
    Expr(Box<TypedExpr>),
    /// Single statement arm.
    Stmt(Box<TypedStmt>),
    /// Nested block arm.
    Block(Box<TypedBody>),
}

/// Typed expression node with semantic result type.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedExpr {
    /// Source span of the expression.
    pub span: Span,
    /// Semantic result type.
    pub ty: InternedTyId,
    /// Expression operation.
    pub kind: TypedExprKind,
}

/// Expression operation after semantic checking.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedExprKind {
    /// Recovery expression rejected by lowering.
    Error,
    /// Integer literal in source spelling.
    Integer(String),
    /// Floating-point literal in source spelling.
    Float(String),
    /// Unicode scalar array literal.
    String(Vec<u32>),
    /// Byte array literal.
    ByteString(Vec<u8>),
    /// Unicode scalar literal.
    Char(u32),
    /// Byte character literal.
    ByteChar(String),
    /// Boolean literal.
    Bool(bool),
    /// Optional/error-union null value.
    Null,
    /// Local value reference.
    Local(LocalId),
    /// Global value reference.
    Global(GlobalDefId),
    /// Const-generic value reference.
    ConstGeneric(nia_ty::ConstGenericArg),
    /// Monomorphic function reference.
    Function(GlobalDefId),
    /// Concrete generic function reference.
    FunctionInstance {
        /// Function definition identity.
        def_id: GlobalDefId,
        /// Module supplying generic argument context.
        arg_module_id: nia_ids::ModuleId,
        /// Type arguments in canonical order.
        args: Vec<InternedTyId>,
        /// Const arguments paired with `args`.
        const_args: Vec<ConstGenericArg>,
    },
    /// Enum variant construction.
    EnumVariant {
        /// Variant identity.
        variant: GlobalDefId,
        /// Payload fields in declaration order.
        fields: Vec<TypedExpr>,
    },
    /// Tuple construction.
    Tuple(Vec<TypedExpr>),
    /// Closure construction with captures and body.
    Closure {
        /// Source closure identity.
        closure_id: nia_ids::ClosureId,
        /// Captured values in capture order.
        captures: Vec<TypedClosureCapture>,
        /// User parameter locals.
        params: Vec<LocalId>,
        /// Closure body.
        body: TypedBody,
    },
    /// Target-independent builtin value.
    BuiltinValue(BuiltinConst),
    /// Deliberate trap expression.
    Trap,
    /// Range construction.
    Range(TypedRange),
    /// Inline assembly expression.
    InlineAsm(TypedInlineAsm),
    /// Bulk memory intrinsic.
    MemoryIntrinsic(TypedMemoryIntrinsic),
    /// Atomic operation.
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
    CallableCoercion {
        state: Box<TypedExpr>,
        closure_id: nia_ids::ClosureId,
    },
    ClosureFunctionPointer {
        closure_id: nia_ids::ClosureId,
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
    Match(Box<TypedMatch>),
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
        /// Forwarded to backend dispatch together with the type arguments.
        trait_const_args: Vec<ConstGenericArg>,
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
        /// Forwarded to backend dispatch together with the type arguments.
        trait_const_args: Vec<ConstGenericArg>,
        args: Vec<InternedTyId>,
    },
    DynamicTraitMethod {
        object_ty: InternedTyId,
        trait_id: TraitId,
        method_id: GlobalDefId,
        method_name: SymbolId,
        trait_args: Vec<InternedTyId>,
        /// Identifies the concrete trait-object instantiation.
        trait_const_args: Vec<ConstGenericArg>,
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
    Callable(Box<TypedExpr>),
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
