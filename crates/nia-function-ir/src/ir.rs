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
/// Stable identity of a basic block within one function body.
pub struct FunctionBlockId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Stable identity of a lexical scope within one function body.
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
    /// Together with `state_param`, this must enumerate every
    /// [`FunctionLocalKind::Param`] in `body` exactly once.
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
    /// Source or generated local name.
    pub name: LocalName,
    /// Declares whether the slot is user storage, generated storage, or a parameter.
    pub kind: FunctionLocalKind,
    /// Physical storage type; typed expression views may be more readonly.
    pub ty: InternedTyId,
    /// Source span that introduced the local.
    pub span: Span,
}

/// Storage role used by a function local.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionLocalKind {
    /// ABI parameter initialized by the caller.
    Param,
    /// Mutable source binding.
    MutableBinding,
    /// Immutable source binding.
    ImmutableBinding,
}

#[derive(Debug, Clone, PartialEq)]
/// One lexical scope in the defer-cleanup ownership tree.
pub struct FunctionScope {
    /// Stable scope identity.
    pub id: FunctionScopeId,
    /// Enclosing scope, or `None` for the function root.
    pub parent: Option<FunctionScopeId>,
    /// Source span that introduced the scope.
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
/// A basic block whose operations execute in order before its terminator.
pub struct FunctionBlock {
    /// Stable block identity.
    pub id: FunctionBlockId,
    /// Lexical scope active for exits originating in this block.
    pub scope: FunctionScopeId,
    /// Source span covering the block's control-flow construct.
    pub span: Span,
    /// Ordered side-effecting operations executed before the terminator.
    pub ops: Vec<FunctionOp>,
    /// Final control-flow action for the block.
    pub terminator: FunctionTerminator,
}

/// Operation executed within a basic block.
#[derive(Debug, Clone, PartialEq)]
pub enum FunctionOp {
    /// Declares and optionally initializes a local binding.
    Binding(FunctionBinding),
    /// Initializes or merges a control-flow value into exactly typed local storage.
    ///
    /// This internal write may initialize an immutable source binding; source
    /// mutability governs later assignment expressions, not CFG construction.
    StoreLocal {
        /// Local storage slot being written.
        local_id: LocalId,
        /// Value merged into the slot.
        value: FunctionExpr,
        /// Source span of the write.
        span: Span,
    },
    /// Effect-only bulk memory operation.
    MemoryIntrinsic(Box<FunctionMemoryIntrinsic>),
    /// Effectful expression evaluated for its side effects.
    Expr(FunctionExpr),
    /// Nested deferred CFG.
    Defer(FunctionDeferBody),
}

/// Local binding declaration and optional initializer.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionBinding {
    /// Local table entry initialized by this binding.
    pub local_id: LocalId,
    /// Name retained for diagnostics.
    pub name: LocalName,
    /// Storage type of the binding.
    pub ty: InternedTyId,
    /// Initial value, absent for deferred/uninitialized bindings.
    pub value: Option<FunctionExpr>,
    /// Whether the source binding is immutable.
    pub is_let: bool,
}

/// Nested CFG executed by a `defer` operation.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDeferBody {
    /// Span of the deferred block.
    pub span: Span,
    /// Private lexical scope table.
    pub scopes: Vec<FunctionScope>,
    /// Private basic-block table.
    pub blocks: Vec<FunctionBlock>,
    /// Entry block of the deferred CFG.
    pub entry: FunctionBlockId,
}

#[derive(Debug, Clone, PartialEq)]
/// An effect-only bulk memory operation over slice storage.
///
/// `dest` must be a mutable slice of `elem_ty`. Copy and move operations pair
/// with a slice source of the same element type, while set operations require
/// both `elem_ty` and their byte source to be `u8`.
pub struct FunctionMemoryIntrinsic {
    /// Source span of the intrinsic operation.
    pub span: Span,
    /// Copy, move, or set operation kind.
    pub op: FunctionMemoryIntrinsicOp,
    /// Element type of the destination slice.
    pub elem_ty: InternedTyId,
    /// Mutable destination slice expression.
    pub dest: FunctionExpr,
    /// Slice or byte source according to `op`.
    pub source: FunctionMemoryIntrinsicSource,
}

#[derive(Debug, Clone, PartialEq)]
/// The source shape paired with a [`FunctionMemoryIntrinsic`] operation.
pub enum FunctionMemoryIntrinsicSource {
    /// Source slice for copy/move.
    Slice(FunctionExpr),
    /// Byte value for set.
    Byte(FunctionExpr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The overlap contract of a [`FunctionMemoryIntrinsic`].
pub enum FunctionMemoryIntrinsicOp {
    /// Permit overlapping source/destination ranges.
    Copy,
    /// Source and destination ranges must not overlap.
    Move,
    /// Fill destination bytes with one `u8` value.
    Set,
}

#[derive(Debug, Clone, PartialEq)]
/// The final control-flow action of a [`FunctionBlock`].
///
/// Backend consumers additionally require every [`FunctionTerminator::Switch`]
/// arm to carry a compile-time integer pattern. Function lowering emits only
/// checked scalar constants and enum tags there; arbitrary value expressions
/// are structurally valid function IR but must not cross the LLVM boundary.
pub enum FunctionTerminator {
    /// Recovery terminator that carries no outgoing edge.
    Error {
        /// Source span of the recovery node.
        span: Span,
    },
    /// Unconditional edge to another block.
    Branch {
        /// Destination block.
        target: FunctionBlockId,
        /// Source span of the edge.
        span: Span,
    },
    /// Fallthrough edge produced by structured lowering.
    Next {
        /// Destination block.
        target: FunctionBlockId,
        /// Source span of the edge.
        span: Span,
    },
    /// Conditional edge selected by a boolean expression.
    If {
        /// Boolean condition.
        cond: FunctionExpr,
        /// Destination when the condition is true.
        then_target: FunctionBlockId,
        /// Destination when the condition is false.
        else_target: FunctionBlockId,
        /// Source span of the branch.
        span: Span,
    },
    /// Integer dispatch with explicit arms and a fallback edge.
    Switch {
        /// Integer-like value being matched.
        target: FunctionExpr,
        /// Constant pattern arms.
        arms: Vec<FunctionSwitchArm>,
        /// Optional explicit default edge.
        default: Option<FunctionBlockId>,
        /// Structural fallback retained for later lowering.
        fallback: FunctionBlockId,
        /// Source span of the switch.
        span: Span,
    },
    /// Transitional propagation node consumed into a CFG terminator by function lowering.
    /// It is invalid at the backend boundary.
    /// Expression-level propagation of an optional or error union.
    Try {
        /// Optional or error-union value being propagated.
        value: FunctionExpr,
        /// Propagation flavor.
        kind: FunctionTryKind,
        // Error conversion is absent for optionals and for identical error
        // payloads. Keep the uncommon second expression out-of-line so every
        // basic block does not inherit the size of two full expression trees.
        /// Optional conversion expression for the propagated error.
        error_conversion: Option<Box<FunctionExpr>>,
        /// Local receiving the successful payload.
        success_local: LocalId,
        /// Destination for successful propagation.
        success_target: FunctionBlockId,
        /// Source span of the propagation edge.
        span: Span,
    },
    /// Loop with explicit body, continue, and break edges.
    Loop {
        /// Loop condition or infinite marker.
        header: FunctionForHeader,
        /// Loop body entry block.
        body: FunctionBlockId,
        /// Continue edge target.
        continue_target: FunctionBlockId,
        /// Break edge target.
        break_target: FunctionBlockId,
        /// Source span of the loop.
        span: Span,
    },
    /// Function return after running exited-scope defers.
    Return {
        /// Optional returned value.
        value: Option<FunctionExpr>,
        /// Source span of the return.
        span: Span,
    },
    /// Tail propagation used by expression lowering.
    Tail {
        /// Optional tail value.
        value: Option<FunctionExpr>,
        /// Source span of the tail edge.
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
/// One constant case and destination of a [`FunctionTerminator::Switch`].
///
/// The pattern has the switch target's integer-like type. Its value must be
/// constant after function lowering, and no two arms may have the same bit
/// pattern at that type's target-dependent width.
pub struct FunctionSwitchArm {
    /// Constant pattern expression.
    pub pattern: FunctionExpr,
    /// Destination block for a matching pattern.
    pub target: FunctionBlockId,
}

/// Propagation flavor for a [`FunctionTerminator::Try`] node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionTryKind {
    /// Propagate an optional null case.
    Optional,
    /// Propagate an error-union error case.
    ErrorUnion,
}

/// ABI discriminants for optional values.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionOptionalTag {
    /// No payload is present.
    Null = 0,
    /// Payload is present.
    Some = 1,
}

impl FunctionOptionalTag {
    /// Returns the ABI discriminant byte.
    pub const fn discriminant(self) -> u8 {
        self as u8
    }
}

/// ABI discriminants for error-union values.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionErrorUnionTag {
    /// Success payload is present.
    Ok = 0,
    /// Error payload is present.
    Err = 1,
}

impl FunctionErrorUnionTag {
    /// Returns the ABI discriminant byte.
    pub const fn discriminant(self) -> u8 {
        self as u8
    }
}

/// Loop header used by a loop terminator.
#[derive(Debug, Clone, PartialEq)]
pub enum FunctionForHeader {
    /// Loop has no condition and repeats until an explicit break.
    Infinite,
    /// Loop repeats while the condition is true.
    Condition(Box<FunctionExpr>),
}

/// Typed expression node in function IR.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionExpr {
    /// Source span of the expression.
    pub span: Span,
    /// Semantic result type.
    pub ty: InternedTyId,
    /// Expression operation and operands.
    pub kind: FunctionExprKind,
}

/// Expression operation in function IR.
#[derive(Debug, Clone, PartialEq)]
pub enum FunctionExprKind {
    /// Recovery expression; rejected before backend lowering.
    Error,
    /// An integer literal whose valid spelling and target range are resolved before LLVM lowering.
    Integer(String),
    /// A finite `f32` or `f64` literal in its source spelling.
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
    /// Loads a const-generic argument value.
    ConstGeneric(ConstGenericArg),
    /// Loads one concrete generic global instance through its published storage type.
    GlobalInstance {
        /// Defining global identity.
        def_id: nia_ids::GlobalDefId,
        /// Module supplying generic argument resolution context.
        arg_module_id: nia_ids::ModuleId,
        /// Type arguments in canonical order.
        args: Vec<InternedTyId>,
        /// Const arguments paired with `args`.
        const_args: Vec<ConstGenericArg>,
    },
    /// Materializes a non-generic function as its exact source-level function pointer type.
    Function(nia_ids::GlobalDefId),
    /// Materializes one concrete generic function instance as its exact function pointer type.
    FunctionInstance {
        /// Defining function identity.
        def_id: nia_ids::GlobalDefId,
        /// Module supplying generic argument resolution context.
        arg_module_id: nia_ids::ModuleId,
        /// Optional receiver substitution.
        self_arg: Option<InternedTyId>,
        /// Type arguments in canonical order.
        args: Vec<InternedTyId>,
        /// Const arguments paired with `args`.
        const_args: Vec<ConstGenericArg>,
    },
    /// Constructs an enum variant, storing payload fields in declaration order.
    EnumVariant {
        /// Variant definition identity.
        variant: nia_ids::GlobalDefId,
        /// Payload fields in declaration order.
        fields: Vec<FunctionExpr>,
    },
    /// Produces a variant's backing integer tag for pattern comparisons.
    EnumVariantTag(nia_ids::GlobalDefId),
    /// Extracts an enum tag from either its aggregate representation or backing integer.
    EnumTag {
        /// Enum value whose discriminant is loaded.
        value: Box<FunctionExpr>,
    },
    /// Loads one payload field from a value known to have the selected variant.
    EnumPayloadField {
        /// Enum value known to carry the selected variant.
        value: Box<FunctionExpr>,
        /// Selected variant identity.
        variant: nia_ids::GlobalDefId,
        /// Declaration-order payload field index.
        field: usize,
    },
    /// Target-independent builtin value construction.
    BuiltinValue(FunctionBuiltinValue),
    /// Deliberate trap expression.
    Trap,
    /// Constructs a range whose bound presence and types follow its `RangeTyKind`.
    Range(FunctionRange),
    /// Extracts one statically present bound from a range value.
    RangeBound {
        /// Range value being projected.
        range: Box<FunctionExpr>,
        /// Bound selected from the range.
        bound: FunctionRangeBound,
    },
    /// Checked inline assembly operation.
    InlineAsm(FunctionInlineAsm),
    /// Checked atomic operation.
    Atomic(FunctionAtomic),
    /// Loads `ty` at byte alignment from a readonly or mutable `u8` pointer.
    LoadUnaligned {
        /// Result type loaded from memory.
        ty: InternedTyId,
        /// Byte pointer source.
        ptr: Box<FunctionExpr>,
    },
    /// Broadcasts one scalar lane into the result SIMD vector type.
    Splat {
        /// Scalar lane to broadcast.
        value: Box<FunctionExpr>,
    },
    /// Extracts one lane using an integer index; the result is the lane type.
    ExtractElement {
        /// SIMD vector source.
        vector: Box<FunctionExpr>,
        /// Integer lane index.
        index: Box<FunctionExpr>,
    },
    /// Replaces one lane and returns the same SIMD vector type.
    InsertElement {
        /// SIMD vector source.
        vector: Box<FunctionExpr>,
        /// Integer lane index.
        index: Box<FunctionExpr>,
        /// Replacement lane value.
        value: Box<FunctionExpr>,
    },
    /// Packs bool lanes, up to the target `usize` width, into a `usize` result.
    Bitmask {
        /// Boolean SIMD vector source.
        vector: Box<FunctionExpr>,
    },
    /// Applies an LLVM integer bit-counting intrinsic without changing type.
    BitIntrinsic {
        /// Bit-counting operation.
        op: FunctionBitIntrinsicOp,
        /// Integer value being inspected.
        value: Box<FunctionExpr>,
    },
    /// Converts a `u32` scalar to `Optional[char]` after validity checks.
    CharFromU32 {
        /// Unicode scalar candidate.
        value: Box<FunctionExpr>,
    },
    /// Materializes promoted array storage and returns a pointer to the complete array value.
    ///
    /// The result pointer element is the type of `array`; `is_readonly` mirrors
    /// the result pointer qualifier and records whether the promotion is frozen.
    StaticArrayPointer {
        /// Stable promoted allocation identity.
        allocation: PromotedAllocationId,
        /// Complete array value to promote.
        array: Box<FunctionExpr>,
        /// Whether the resulting pointer is readonly.
        is_readonly: bool,
    },
    /// Constructs an array value.
    ArrayLiteral {
        /// Explicit or repeated element representation.
        elems: FunctionArrayElements,
    },
    /// Constructs a tuple value.
    Tuple(Vec<FunctionExpr>),
    /// Projects one tuple field.
    TupleField {
        /// Tuple value being projected.
        value: Box<FunctionExpr>,
        /// Declaration-order field index.
        index: usize,
    },
    /// Initializes every declared field of one exact nominal struct instance.
    StructLiteral {
        /// Nominal struct identity.
        def_id: nia_ids::GlobalDefId,
        /// Field initializers in declaration or canonical order.
        fields: Vec<FunctionFieldInit>,
    },
    /// Initializes the selected storage member of one exact nominal union instance.
    UnionLiteral {
        /// Nominal union identity.
        def_id: nia_ids::GlobalDefId,
        /// Exactly one selected field initializer.
        field: Box<FunctionFieldInit>,
    },
    /// Reconstructs the byte representation of a const-evaluated union.
    ///
    /// Relocations are sorted, non-overlapping target-pointer ranges within
    /// `bytes`; each pointee becomes separately promoted storage whose address
    /// replaces the covered initialized bytes. Function IR validation enforces
    /// these structural rules before backend target-width validation.
    UnionStorageLiteral {
        /// Byte image, with `None` for uninitialized bytes.
        bytes: Vec<Option<u8>>,
        /// Sorted non-overlapping pointer relocations.
        relocations: Vec<FunctionUnionRelocation>,
    },
    /// A typed unary operation; references to ordinary places are lowered to
    /// [`FunctionExprKind::AddrOf`] before this backend boundary.
    Unary {
        /// Unary operator.
        op: UnaryOp,
        /// Operand expression.
        expr: Box<FunctionExpr>,
    },
    /// Constructs an `Optional` value with the `Some` discriminant.
    OptionalSome {
        /// Payload expression.
        expr: Box<FunctionExpr>,
    },
    /// Constructs an `ErrorUnion` value carrying its success payload.
    ErrorOk {
        /// Success payload expression.
        expr: Box<FunctionExpr>,
    },
    /// Constructs an `ErrorUnion` value carrying its error payload.
    ErrorErr {
        /// Error payload expression.
        expr: Box<FunctionExpr>,
    },
    /// Extracts the one-byte discriminant from an optional or error union.
    TaggedUnionTag {
        /// Optional or error-union value.
        expr: Box<FunctionExpr>,
    },
    /// Extracts the active payload from an optional or error union.
    TaggedUnionPayload {
        /// Optional or error-union value.
        expr: Box<FunctionExpr>,
    },
    /// Expression-level propagation of an optional or error union.
    Try {
        /// Value whose failure branch is propagated.
        expr: Box<FunctionExpr>,
    },
    /// Takes the address of a typed place.
    AddrOf(FunctionPlace),
    /// A typed scalar/vector operation whose result is either the operand type
    /// or a bool mask for comparisons.
    Binary {
        /// Left operand.
        lhs: Box<FunctionExpr>,
        /// Binary operator.
        op: BinaryOp,
        /// Right operand.
        rhs: Box<FunctionExpr>,
    },
    /// Writes `rhs` into an exactly typed, writable place and evaluates to unit.
    ///
    /// Compound forms apply the corresponding builtin binary operation to the
    /// current place value and `rhs`; that operation must produce `place.ty`.
    Assign {
        /// Writable destination place.
        place: FunctionPlace,
        /// Assignment operator.
        op: AssignOp,
        /// Right-hand value.
        rhs: Box<FunctionExpr>,
    },
    /// Evaluates its operand for effects and produces unit.
    Discard(Box<FunctionExpr>),
    /// Converts between source-approved numeric, enum, and pointer categories.
    Cast {
        /// Source expression.
        expr: Box<FunctionExpr>,
        /// Destination type.
        ty: InternedTyId,
    },
    /// Repoints an existing trait object at a supertrait vtable.
    TraitObjectUpcast {
        /// Existing trait-object value.
        expr: Box<FunctionExpr>,
        /// Source object type.
        source_ty: InternedTyId,
        /// Target supertrait object type.
        target_ty: InternedTyId,
    },
    /// Builds a trait object from a pointer/slice data view and a concrete vtable.
    TraitObjectCoercion {
        /// Data pointer or slice expression.
        expr: Box<FunctionExpr>,
        /// Complete target object type.
        target_ty: InternedTyId,
        /// Concrete receiver type used for vtable selection.
        self_ty: InternedTyId,
    },
    /// Pairs a closure-state pointer with its generated entry as a callable fat pointer.
    ///
    /// The state, closure identity, callable signature, and owner-qualified
    /// backend entry must describe the same closure instance.
    CallableCoercion {
        /// Captured state pointer.
        state: Box<FunctionExpr>,
        /// Source closure identity.
        closure_id: ClosureId,
    },
    /// Selects the generated adapter for a non-capturing closure.
    ClosureFunctionPointer {
        /// Source closure identity.
        closure_id: ClosureId,
    },
    /// Calls a resolved callee with evaluated arguments.
    Call {
        /// Selected callee shape.
        callee: FunctionCallee,
        /// Arguments in ABI order.
        args: Vec<FunctionExpr>,
    },
    /// Loads a field from a nominal aggregate or a pointer to one.
    Field {
        /// Aggregate or aggregate-pointer source.
        lhs: Box<FunctionExpr>,
        /// Selected field identity.
        field: nia_ids::GlobalDefId,
    },
    /// Loads one element from an array, pointer, or slice base.
    Index {
        /// Array, pointer, or slice source.
        lhs: Box<FunctionExpr>,
        /// Integer index expression.
        index: Box<FunctionExpr>,
    },
    /// Creates a fat slice view over an array, pointer, or existing slice.
    Slice {
        /// Array, pointer, or slice source.
        lhs: Box<FunctionExpr>,
        /// Range bounds.
        range: FunctionSliceRange,
        /// Whether the resulting data view is readonly.
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

/// Integer bit-counting intrinsic selected by a function expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionBitIntrinsicOp {
    /// Count trailing zero bits.
    Ctz,
    /// Count leading zero bits.
    Clz,
    /// Count set bits.
    Popcount,
}

/// Slice bounds attached to a slice expression.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionSliceRange {
    /// Optional lower bound; omitted bounds start at zero.
    pub start: Option<Box<FunctionExpr>>,
    /// Optional upper bound; omitted bounds use the source length.
    pub end: Option<Box<FunctionExpr>>,
    /// Whether the upper bound is inclusive and must be incremented.
    pub inclusive: bool,
}

/// Bounds attached to a range value.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionRange {
    /// Present only for range kinds with a lower bound.
    pub start: Option<Box<FunctionExpr>>,
    /// Present only for range kinds with an upper bound.
    pub end: Option<Box<FunctionExpr>>,
    /// Mirrors the inclusive range kind selected in the expression type.
    pub inclusive: bool,
}

/// Bound selector for range projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionRangeBound {
    /// Lower bound of a range.
    Start,
    /// Upper bound of a range.
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

/// Target-independent builtin value query or constant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunctionBuiltinValue {
    /// A target-width `usize` constant.
    Usize(u64),
    /// The target layout size or alignment of a runtime-representable type.
    Layout {
        /// Layout query kind.
        builtin: LayoutBuiltin,
        /// Type being queried.
        ty: InternedTyId,
    },
    /// The byte offset of a declared field in its aggregate type.
    FieldOffset {
        /// Aggregate type containing the field.
        ty: InternedTyId,
        /// Field whose offset is requested.
        field: nia_ids::GlobalDefId,
    },
    /// A target-typed integer bit pattern produced by constant evaluation.
    Int(IntConst),
}

/// Inline assembly option affecting optimizer assumptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionAsmOption {
    /// Assembly may not be removed or reordered as a pure operation.
    Volatile,
}

/// Checked atomic operation lowered to the target memory model.
#[derive(Debug, Clone, PartialEq)]
pub enum FunctionAtomic {
    /// Atomic load.
    Load {
        /// Loaded value type.
        ty: InternedTyId,
        /// Data pointer.
        ptr: Box<FunctionExpr>,
        /// Memory ordering.
        order: AtomicOrder,
    },
    /// Atomic store.
    Store {
        /// Stored value type.
        ty: InternedTyId,
        /// Data pointer.
        ptr: Box<FunctionExpr>,
        /// Value to store.
        value: Box<FunctionExpr>,
        /// Memory ordering.
        order: AtomicOrder,
    },
    /// Atomic read-modify-write operation.
    Rmw {
        /// Updated value type.
        ty: InternedTyId,
        /// Data pointer.
        ptr: Box<FunctionExpr>,
        /// RMW operation.
        op: AtomicRmwOp,
        /// Operand value.
        value: Box<FunctionExpr>,
        /// Memory ordering.
        order: AtomicOrder,
    },
    /// Atomic compare-and-exchange operation.
    Cmpxchg {
        /// Compared value type.
        ty: InternedTyId,
        /// Data pointer.
        ptr: Box<FunctionExpr>,
        /// Expected old value.
        expected: Box<FunctionExpr>,
        /// Desired replacement value.
        desired: Box<FunctionExpr>,
        /// Ordering on success.
        success: AtomicOrder,
        /// Ordering on failure.
        failure: AtomicOrder,
        /// Whether spurious failure is permitted.
        weak: bool,
    },
    /// Atomic fence.
    Fence {
        /// Fence ordering.
        order: AtomicOrder,
    },
}

/// Memory ordering accepted by function IR atomic operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicOrder {
    /// Unordered operation.
    Unordered,
    /// Monotonic operation.
    Monotonic,
    /// Acquire ordering.
    Acquire,
    /// Release ordering.
    Release,
    /// Acquire-release ordering.
    AcqRel,
    /// Sequentially consistent ordering.
    SeqCst,
}

/// Read-modify-write operation selected for an atomic update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicRmwOp {
    /// Exchange old and new values.
    Xchg,
    /// Add operand.
    Add,
    /// Subtract operand.
    Sub,
    /// Bitwise and.
    And,
    /// Bitwise nand.
    Nand,
    /// Bitwise or.
    Or,
    /// Bitwise xor.
    Xor,
    /// Signed maximum.
    Max,
    /// Signed minimum.
    Min,
    /// Unsigned maximum.
    UMax,
    /// Unsigned minimum.
    UMin,
}

#[derive(Debug, Clone, PartialEq)]
/// One inline-assembly input after type checking.
pub struct FunctionAsmInput {
    /// Backend constraint corresponding to this operand.
    pub constraint: String,
    /// Scalar value passed directly through LLVM's inline-assembly call boundary.
    pub value: FunctionExpr,
    /// Source span of the input operand.
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
/// One inline-assembly output stored into a writable place after execution.
pub struct FunctionAsmOutput {
    /// Backend output constraint corresponding to this operand.
    pub constraint: String,
    /// Exact writable scalar storage updated with the corresponding result.
    pub place: FunctionPlace,
    /// Source span of the output operand.
    pub span: Span,
}

/// Array literal represented as explicit elements or a repeated value.
#[derive(Debug, Clone, PartialEq)]
pub enum FunctionArrayElements {
    /// Explicit elements in source order.
    List(Vec<FunctionExpr>),
    /// One value repeated a const-evaluated number of times.
    Repeat {
        /// Repeated element value.
        value: Box<FunctionExpr>,
        /// Number of repetitions.
        count: ArrayLenTy,
    },
}

/// One named or positional aggregate field initializer.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionFieldInit {
    /// Declared field identity, absent for positional syntax.
    pub field: Option<nia_ids::GlobalDefId>,
    /// Source field name or generated positional label.
    pub name: String,
    /// Initializer expression.
    pub value: FunctionExpr,
    /// Source span of the initializer.
    pub span: Span,
}

/// Callee shape selected after method, trait, and closure resolution.
#[derive(Debug, Clone, PartialEq)]
pub enum FunctionCallee {
    /// Generated closure entry plus its captured state pointer.
    ClosureEntry {
        /// Source closure identity.
        closure_id: ClosureId,
        /// Captured state expression.
        state: Box<FunctionExpr>,
    },
    /// Monomorphic function definition.
    Function(nia_ids::GlobalDefId),
    /// Concrete generic function instance.
    FunctionInstance {
        /// Defining function identity.
        def_id: nia_ids::GlobalDefId,
        /// Module supplying generic argument context.
        arg_module_id: nia_ids::ModuleId,
        /// Optional receiver substitution.
        self_arg: Option<InternedTyId>,
        /// Type arguments in canonical order.
        args: Vec<InternedTyId>,
        /// Const arguments paired with `args`.
        const_args: Vec<ConstGenericArg>,
    },
    /// Method call with a statically selected function body.
    Method {
        /// Defining method identity.
        def_id: nia_ids::GlobalDefId,
        /// Module supplying generic argument context.
        arg_module_id: nia_ids::ModuleId,
        /// Optional receiver substitution.
        self_arg: Option<InternedTyId>,
        /// Type arguments in canonical order.
        args: Vec<InternedTyId>,
        /// Const arguments paired with `args`.
        const_args: Vec<ConstGenericArg>,
        /// Receiver passing mode.
        receiver_kind: ReceiverKind,
        /// Receiver expression.
        receiver: Box<FunctionExpr>,
    },
    /// Trait method call through a statically selected implementation.
    TraitMethod {
        /// Trait identity containing the method.
        trait_id: nia_ids::GlobalDefId,
        /// Selected method identity.
        method_id: nia_ids::GlobalDefId,
        /// Method name for diagnostics.
        method_name: SymbolId,
        /// Concrete receiver type.
        self_ty: InternedTyId,
        /// Trait type arguments.
        trait_args: Vec<InternedTyId>,
        /// Required to select the correct const-generic extension impl.
        trait_const_args: Vec<ConstGenericArg>,
        /// Method type arguments.
        args: Vec<InternedTyId>,
        /// Receiver passing mode.
        receiver_kind: ReceiverKind,
        /// Receiver expression.
        receiver: Box<FunctionExpr>,
    },
    /// Trait-associated function call without a receiver.
    TraitAssociatedFunction {
        /// Trait identity containing the associated function.
        trait_id: nia_ids::GlobalDefId,
        /// Selected method identity.
        method_id: nia_ids::GlobalDefId,
        /// Method name for diagnostics.
        method_name: SymbolId,
        /// Concrete receiver type.
        self_ty: InternedTyId,
        /// Trait type arguments.
        trait_args: Vec<InternedTyId>,
        /// Required to select the correct const-generic extension impl.
        trait_const_args: Vec<ConstGenericArg>,
        /// Method type arguments.
        args: Vec<InternedTyId>,
    },
    /// Dynamic dispatch through a trait-object vtable slot.
    DynamicTraitMethod {
        /// Erased object type used for vtable lookup.
        object_ty: InternedTyId,
        /// Trait segment containing the selected slot.
        trait_id: TraitId,
        /// Declared method identity.
        method_id: nia_ids::GlobalDefId,
        /// Method name for diagnostics.
        method_name: SymbolId,
        /// Trait type arguments.
        trait_args: Vec<InternedTyId>,
        /// Retains the complete trait-object identity for vtable dispatch.
        trait_const_args: Vec<ConstGenericArg>,
        /// Vtable slot index.
        slot: usize,
        /// ABI parameter types.
        params: Vec<InternedTyId>,
        /// ABI return type.
        return_type: InternedTyId,
        /// Receiver passing mode.
        receiver_kind: ReceiverKind,
        /// Dynamic receiver expression.
        receiver: Box<FunctionExpr>,
    },
    /// Builtin method call.
    BuiltinMethod {
        /// Builtin method kind.
        method: FunctionBuiltinMethod,
        /// Receiver type.
        self_ty: InternedTyId,
        /// Receiver expression.
        receiver: Box<FunctionExpr>,
    },
    /// Builtin method requiring an addressable receiver.
    BuiltinPlaceMethod {
        /// Builtin trait containing the method.
        trait_id: BuiltinTrait,
        /// Builtin method identity.
        method: BuiltinTraitMethod,
        /// Receiver type.
        self_ty: InternedTyId,
        /// Trait type arguments.
        trait_args: Vec<InternedTyId>,
        /// Receiver place expression.
        receiver: Box<FunctionExpr>,
    },
    /// Operator dispatched through a builtin trait.
    BuiltinOperator(FunctionBuiltinOperator),
    /// Callable fat-pointer expression.
    Callable(Box<FunctionExpr>),
    /// Function-pointer expression.
    FunctionPointer(Box<FunctionExpr>),
}

/// Builtin operator trait and operation pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FunctionBuiltinOperator {
    /// Builtin trait implementing the operator.
    pub trait_id: BuiltinTrait,
    /// Unary or binary operator kind.
    pub op: FunctionBuiltinOperatorOp,
}

/// Builtin methods exposed on slices and range values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionBuiltinMethod {
    /// Slice element count.
    SliceLen,
    /// Readonly slice data pointer.
    SlicePtr,
    /// Mutable slice data pointer.
    SlicePtrMut,
    /// Range lower bound.
    Start,
    /// Range upper bound.
    End,
    /// Range iterator adapter.
    Iter,
}

/// Operator form used to select a builtin trait method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionBuiltinOperatorOp {
    /// Unary operator.
    Unary(UnaryOp),
    /// Binary operator.
    Binary(BinaryOp),
}

impl FunctionBuiltinOperatorOp {
    /// Maps an operator to its builtin trait method, if one exists.
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
    /// Returns the method only when it belongs to this operator's trait.
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
    /// Source span of the place expression.
    pub span: Span,
    /// Final projected type.
    pub ty: InternedTyId,
    /// Storage root from which projection begins.
    pub base: FunctionPlaceBase,
    /// Declaration-ordered projections applied from left to right.
    pub elems: Vec<FunctionPlaceElem>,
}

#[derive(Debug, Clone, PartialEq)]
/// Root storage for a [`FunctionPlace`].
pub enum FunctionPlaceBase {
    /// Local storage root.
    Local(LocalId),
    /// Non-generic global storage root.
    Global(nia_ids::GlobalDefId),
    /// Generic global instance storage root.
    GlobalInstance {
        /// Defining global identity.
        def_id: nia_ids::GlobalDefId,
        /// Module supplying generic argument context.
        arg_module_id: nia_ids::ModuleId,
        /// Type arguments in canonical order.
        args: Vec<InternedTyId>,
        /// Const arguments paired with `args`.
        const_args: Vec<ConstGenericArg>,
    },
    /// Dereferenced pointer expression.
    Deref(Box<FunctionExpr>),
    /// Recovery base rejected by validation.
    Error,
}

#[derive(Debug, Clone, PartialEq)]
/// One typed projection in an addressable storage path.
pub enum FunctionPlaceElem {
    /// Named aggregate field projection.
    Field(nia_ids::GlobalDefId),
    /// Tuple field projection.
    TupleField(usize),
    /// Indexed array/pointer/slice projection.
    Index(Box<FunctionExpr>),
    /// Recovery projection rejected by validation.
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
    /// Looks up a block by its stable id.
    pub fn block(&self, id: FunctionBlockId) -> Option<&FunctionBlock> {
        self.blocks.iter().find(|block| block.id == id)
    }

    /// Looks up a lexical scope by its stable id.
    pub fn scope(&self, id: FunctionScopeId) -> Option<&FunctionScope> {
        self.scopes.iter().find(|scope| scope.id == id)
    }

    /// Returns scopes exited by an edge between two blocks.
    pub fn edge_exited_scopes(
        &self,
        from: FunctionBlockId,
        to: FunctionBlockId,
    ) -> Option<Vec<FunctionScopeId>> {
        let from = self.block(from)?.scope;
        let to = self.block(to)?.scope;
        exited_scopes_between(&self.scopes, from, Some(to))
    }

    /// Returns scopes unwound by returning from a block.
    pub fn return_exited_scopes(&self, from: FunctionBlockId) -> Option<Vec<FunctionScopeId>> {
        let from = self.block(from)?.scope;
        exited_scopes_between(&self.scopes, from, None)
    }

    /// Computes scopes exited between explicit source and destination scopes.
    pub fn exited_scopes_between(
        &self,
        from: FunctionScopeId,
        to: Option<FunctionScopeId>,
    ) -> Option<Vec<FunctionScopeId>> {
        exited_scopes_between(&self.scopes, from, to)
    }
}

impl FunctionDeferBody {
    /// Looks up a deferred-body block by stable id.
    pub fn block(&self, id: FunctionBlockId) -> Option<&FunctionBlock> {
        self.blocks.iter().find(|block| block.id == id)
    }

    /// Looks up a deferred-body scope by stable id.
    pub fn scope(&self, id: FunctionScopeId) -> Option<&FunctionScope> {
        self.scopes.iter().find(|scope| scope.id == id)
    }

    /// Returns scopes exited by a deferred-body edge.
    pub fn edge_exited_scopes(
        &self,
        from: FunctionBlockId,
        to: FunctionBlockId,
    ) -> Option<Vec<FunctionScopeId>> {
        let from = self.block(from)?.scope;
        let to = self.block(to)?.scope;
        exited_scopes_between(&self.scopes, from, Some(to))
    }

    /// Returns scopes unwound by returning from a deferred body block.
    pub fn return_exited_scopes(&self, from: FunctionBlockId) -> Option<Vec<FunctionScopeId>> {
        let from = self.block(from)?.scope;
        exited_scopes_between(&self.scopes, from, None)
    }

    /// Computes scopes exited between deferred-body scopes.
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
    // A real edge may enter a child scope or leave to an ancestor, but it
    // cannot jump between unrelated roots. Returning `None` for that shape
    // lets lowering surface the malformed CFG before codegen invents a defer
    // unwinding sequence for an unrelated destination.
    let lca = match to {
        Some(_) => Some(
            from_chain
                .iter()
                .find(|scope| to_chain.contains(scope))
                .copied()?,
        ),
        None => None,
    };
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
