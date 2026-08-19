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
        /// Result type loaded from memory.
        ty: InternedTyId,
        /// Byte pointer source.
        ptr: Box<TypedExpr>,
    },
    /// Broadcasts one scalar value into a SIMD vector.
    Splat {
        /// Scalar lane value.
        value: Box<TypedExpr>,
    },
    /// Extracts a SIMD lane by integer index.
    ExtractElement {
        /// Vector source.
        vector: Box<TypedExpr>,
        /// Lane index.
        index: Box<TypedExpr>,
    },
    /// Replaces one SIMD lane.
    InsertElement {
        /// Vector source.
        vector: Box<TypedExpr>,
        /// Lane index.
        index: Box<TypedExpr>,
        /// Replacement lane value.
        value: Box<TypedExpr>,
    },
    /// Packs boolean lanes into a target-width integer.
    Bitmask {
        /// Boolean vector source.
        vector: Box<TypedExpr>,
    },
    /// Integer bit-counting intrinsic.
    BitIntrinsic {
        /// Intrinsic operation.
        op: TypedBitIntrinsicOp,
        /// Integer source value.
        value: Box<TypedExpr>,
    },
    /// Converts a `u32` candidate into an optional `char`.
    CharFromU32 {
        /// Candidate scalar value.
        value: Box<TypedExpr>,
    },
    /// Promotes an array into stable static storage.
    StaticArrayPointer {
        /// Promoted allocation identity.
        allocation: PromotedAllocationId,
        /// Array value being promoted.
        array: Box<TypedExpr>,
        /// Whether resulting storage is readonly.
        is_readonly: bool,
    },
    /// Constructs an array value.
    ArrayLiteral {
        /// Explicit or repeated elements.
        elems: TypedArrayElements,
    },
    /// Constructs a nominal struct value.
    StructLiteral {
        /// Struct definition identity.
        def_id: GlobalDefId,
        /// Field initializers in declaration order.
        fields: Vec<TypedFieldInit>,
    },
    /// Constructs a nominal union value with one active field.
    UnionLiteral {
        /// Union definition identity.
        def_id: GlobalDefId,
        /// Selected field initializer.
        field: Box<TypedFieldInit>,
    },
    /// Reconstructs a const-evaluated union byte image with relocations.
    UnionStorageLiteral {
        /// Byte image; `None` denotes uninitialized bytes.
        bytes: Vec<Option<u8>>,
        /// Sorted, non-overlapping promoted-pointer relocations.
        relocations: Vec<TypedUnionRelocation>,
    },
    /// Typed unary operation.
    Unary {
        /// Unary operator.
        op: UnaryOp,
        /// Operand expression.
        expr: Box<TypedExpr>,
    },
    /// Constructs an optional success value.
    OptionalSome {
        /// Payload expression.
        expr: Box<TypedExpr>,
    },
    /// Constructs an error-union success value.
    ErrorOk {
        /// Success payload.
        expr: Box<TypedExpr>,
    },
    /// Constructs an error-union error value.
    ErrorErr {
        /// Error payload.
        expr: Box<TypedExpr>,
    },
    /// Propagates an optional or error-union failure.
    Try {
        /// Value being propagated.
        expr: Box<TypedExpr>,
        /// Optional conversion for an error-union mismatch.
        error_conversion: Option<TypedTryErrorConversion>,
    },
    /// Typed scalar/vector binary operation.
    Binary {
        /// Left operand.
        lhs: Box<TypedExpr>,
        /// Binary operator.
        op: BinaryOp,
        /// Right operand.
        rhs: Box<TypedExpr>,
    },
    /// Writes a value into a typed place.
    Assign {
        /// Destination place.
        place: TypedPlace,
        /// Assignment operator.
        op: AssignOp,
        /// Right-hand value.
        rhs: Box<TypedExpr>,
    },
    /// Evaluates an expression for effects and yields unit.
    Discard(Box<TypedExpr>),
    /// Source-approved cast to a destination type.
    Cast {
        /// Source expression.
        expr: Box<TypedExpr>,
        /// Destination type.
        ty: InternedTyId,
    },
    /// Upcasts a trait object to a supertrait view.
    TraitObjectUpcast {
        /// Source object value.
        expr: Box<TypedExpr>,
        /// Source object type.
        source_ty: InternedTyId,
        /// Target object type.
        target_ty: InternedTyId,
    },
    /// Coerces a data pointer/slice to a trait object.
    TraitObjectCoercion {
        /// Data expression.
        expr: Box<TypedExpr>,
        /// Complete target object type.
        target_ty: InternedTyId,
        /// Concrete receiver type for vtable selection.
        self_ty: InternedTyId,
    },
    /// Builds a callable fat pointer from captured state.
    CallableCoercion {
        /// Captured state pointer.
        state: Box<TypedExpr>,
        /// Closure identity.
        closure_id: nia_ids::ClosureId,
    },
    /// Selects a non-capturing closure function pointer.
    ClosureFunctionPointer {
        /// Closure identity.
        closure_id: nia_ids::ClosureId,
    },
    /// Calls a resolved callee with ABI-ordered arguments.
    Call {
        /// Callee shape selected by semantic resolution.
        callee: TypedCallee,
        /// Arguments in source/ABI order.
        args: Vec<TypedExpr>,
    },
    /// Loads a nominal field.
    Field {
        /// Aggregate source.
        lhs: Box<TypedExpr>,
        /// Field identity.
        field: GlobalDefId,
    },
    /// Projects a tuple field.
    TupleField {
        /// Tuple source.
        lhs: Box<TypedExpr>,
        /// Declaration-order field index.
        index: usize,
    },
    /// Loads an indexed element.
    Index {
        /// Array, pointer, or slice source.
        lhs: Box<TypedExpr>,
        /// Integer index.
        index: Box<TypedExpr>,
    },
    /// Creates a slice view with typed bounds.
    Slice {
        /// Array, pointer, or slice source.
        lhs: Box<TypedExpr>,
        /// Slice bounds.
        range: TypedSliceRange,
        /// Whether resulting view is readonly.
        is_readonly: bool,
    },
    /// Nested typed block expression.
    Block(TypedBody),
    /// Conditional expression with typed branches.
    If {
        /// Boolean condition.
        cond: Box<TypedExpr>,
        /// Success branch body.
        then_branch: TypedBody,
        /// Optional failure branch expression.
        else_branch: Option<Box<TypedExpr>>,
    },
    /// Conditional pattern expression.
    IfPattern(Box<TypedIfPattern>),
    /// Match expression with typed arms.
    Match(Box<TypedMatch>),
}

/// One captured local and its lowered value.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedClosureCapture {
    /// Captured local identity.
    pub local_id: LocalId,
    /// Value captured for the closure state.
    pub value: TypedExpr,
}

/// Error conversion metadata attached to a try expression.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedTryErrorConversion {
    /// Trait supplying the conversion method.
    pub trait_id: GlobalDefId,
    /// Conversion method identity.
    pub method_id: GlobalDefId,
    /// Method name for diagnostics.
    pub method_name: SymbolId,
    /// Source error type.
    pub source_ty: InternedTyId,
    /// Converted target error type.
    pub target_ty: InternedTyId,
    /// Trait type arguments.
    pub trait_args: Vec<InternedTyId>,
    /// Receiver passing mode.
    pub receiver_kind: ReceiverKind,
}

/// Pointer relocation embedded in a union byte image.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedUnionRelocation {
    /// First byte replaced by a promoted pointer.
    pub offset: usize,
    /// Number of bytes covered by the relocation.
    pub width: usize,
    /// Promoted allocation identity.
    pub allocation: PromotedAllocationId,
    /// Value used to initialize the promoted allocation.
    pub pointee: Box<TypedExpr>,
}

/// Integer bit-counting intrinsic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedBitIntrinsicOp {
    /// Count trailing zeros.
    Ctz,
    /// Count leading zeros.
    Clz,
    /// Count set bits.
    Popcount,
}

/// Bounds attached to a typed slice expression.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedSliceRange {
    /// Optional lower bound.
    pub start: Option<Box<TypedExpr>>,
    /// Optional upper bound.
    pub end: Option<Box<TypedExpr>>,
    /// Whether upper bound is inclusive.
    pub inclusive: bool,
}

/// Bounds attached to a typed range value.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedRange {
    /// Optional lower bound.
    pub start: Option<Box<TypedExpr>>,
    /// Optional upper bound.
    pub end: Option<Box<TypedExpr>>,
    /// Whether upper bound is inclusive.
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
