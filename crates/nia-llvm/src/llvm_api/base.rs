// SPDX-License-Identifier: GPL-3.0-or-later
//! Base LLVM wrapper types.
//!
//! This file defines typed handles for LLVM values, types, blocks, modules, and
//! attributes. Handles are non-owning unless their wrapper implements `Drop`;
//! callers must ensure they belong to the live LLVM context/module that created
//! them.
//!
//! Safety model: most wrappers are transparent handles around LLVM C API
//! pointers. They do not prove lifetime relationships in Rust's type system;
//! higher-level code must keep the originating `Context`/`Module` alive and must
//! use the typed constructors instead of fabricating raw handles.

use llvm_sys::LLVMAttributeFunctionIndex;
use llvm_sys::core::{
    LLVMAddAttributeAtIndex, LLVMAddIncoming, LLVMArrayType2, LLVMConstArray2, LLVMConstBitCast,
    LLVMConstGEP2, LLVMConstInBoundsGEP2, LLVMConstInt, LLVMConstIntOfArbitraryPrecision,
    LLVMConstIntToPtr, LLVMConstNamedStruct, LLVMConstNull, LLVMConstPointerNull, LLVMConstReal,
    LLVMConstVector, LLVMCountParamTypes, LLVMCountParams, LLVMCountStructElementTypes,
    LLVMFunctionType, LLVMGetAllocatedType, LLVMGetBasicBlockParent, LLVMGetBasicBlockTerminator,
    LLVMGetCmpXchgFailureOrdering, LLVMGetElementType, LLVMGetEnumAttributeKindForName,
    LLVMGetFirstBasicBlock, LLVMGetFirstInstruction, LLVMGetInstructionOpcode,
    LLVMGetInstructionParent, LLVMGetIntTypeWidth, LLVMGetNextBasicBlock, LLVMGetNextInstruction,
    LLVMGetParam, LLVMGetParamTypes, LLVMGetPointerAddressSpace, LLVMGetReturnType,
    LLVMGetTypeKind, LLVMGetUndef, LLVMGetValueName2, LLVMGetVectorSize, LLVMGlobalGetValueType,
    LLVMIsAInstruction, LLVMIsLiteralStruct, LLVMIsOpaqueStruct, LLVMIsPackedStruct,
    LLVMSetAlignment, LLVMSetGlobalConstant, LLVMSetInitializer, LLVMSetLinkage, LLVMSetOrdering,
    LLVMSetSection, LLVMSetVolatile, LLVMSetWeak, LLVMStructGetTypeAtIndex, LLVMStructSetBody,
    LLVMTypeOf, LLVMVectorType,
};
use llvm_sys::debuginfo::LLVMSetSubprogram;
use llvm_sys::prelude::{LLVMAttributeRef, LLVMBasicBlockRef, LLVMTypeRef, LLVMValueRef};
use llvm_sys::{
    LLVMAtomicOrdering, LLVMAtomicRMWBinOp, LLVMInlineAsmDialect, LLVMIntPredicate, LLVMLinkage,
    LLVMOpcode, LLVMRealPredicate, LLVMTypeKind,
};
use std::ffi::CString;
use std::marker::PhantomData;
use std::slice;

use super::{Context, DISubprogram};

#[derive(Debug, Clone, PartialEq, Eq)]
/// Failure reported by the typed LLVM boundary.
pub enum LlvmError {
    /// Recoverable LLVM C API failure text.
    Error(String),
    /// Compiler invariant failure detected while preparing an LLVM call.
    Ice(nia_ice::Ice),
}

/// Result returned by fallible LLVM wrapper operations.
pub type LlvmResult<T> = Result<T, LlvmError>;

impl LlvmError {
    /// Creates a recoverable LLVM API error.
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error(message.into())
    }

    /// Creates an internal compiler error owned by the LLVM subsystem.
    pub fn ice(message: impl Into<String>) -> Self {
        Self::Ice(nia_ice::Ice::new(format!("LLVM: {}", message.into())))
    }

    /// Converts this wrapper failure to the compiler's diagnostic format.
    pub fn diagnostic(&self) -> nia_diagnostic::Diagnostic {
        match self {
            Self::Error(message) => nia_diagnostic::Diagnostic::internal_error(
                nia_diagnostic::codes::INTERNAL_LLVM_API,
                "LLVM API returned an error",
            )
            .primary_fallback(nia_span::Span::default(), "while calling LLVM")
            .debug("llvm_message", message)
            .finish(),
            Self::Ice(ice) => ice.diagnostic(),
        }
    }
}

impl From<LlvmError> for nia_diagnostic::Diagnostic {
    fn from(error: LlvmError) -> Self {
        error.diagnostic()
    }
}

pub(super) fn to_c_string(input: &str) -> LlvmResult<CString> {
    CString::new(input)
        .map_err(|_| LlvmError::ice(format!("string contains interior NUL byte: {input:?}")))
}

pub(super) fn bool_to_llvm(value: bool) -> i32 {
    if value { 1 } else { 0 }
}

pub(super) fn checked_u32_count(count: usize, message: &str) -> LlvmResult<u32> {
    u32::try_from(count).map_err(|_| LlvmError::error(message))
}

pub(super) fn require_value(raw: LLVMValueRef, operation: &str) -> LlvmResult<LLVMValueRef> {
    if raw.is_null() {
        return Err(LlvmError::error(format!(
            "LLVM returned a null {operation} value"
        )));
    }
    Ok(raw)
}

pub(super) fn validate_alignment(bytes: u32) -> LlvmResult<()> {
    if bytes == 0 || !bytes.is_power_of_two() {
        return Err(LlvmError::error(
            "LLVM alignment must be a non-zero power of two",
        ));
    }
    Ok(())
}

/// Provides a borrowed raw LLVM type handle.
pub trait AsTypeRef {
    /// Returns the underlying non-owning type handle.
    fn as_type_ref(&self) -> LLVMTypeRef;
}

/// Marker for types accepted as first-class LLVM value types.
pub trait BasicType<'ctx>: AsTypeRef + Copy {}

/// Provides a borrowed raw LLVM value handle.
pub trait AsValueRef {
    /// Returns the underlying non-owning value handle.
    fn as_value_ref(&self) -> LLVMValueRef;
}

/// A first-class LLVM value convertible to the shared value enum.
pub trait BasicValue<'ctx>: AsValueRef {
    /// Converts this typed handle without changing LLVM ownership.
    fn as_basic_value_enum(&self) -> BasicValueEnum<'ctx>;
}

fn validate_constant_elements<T: AsValueRef>(
    values: &[T],
    element_type: LLVMTypeRef,
) -> LlvmResult<()> {
    if element_type.is_null() {
        return Err(LlvmError::error(
            "LLVM returned a null constant array element type",
        ));
    }
    for (index, value) in values.iter().enumerate() {
        let value_type = unsafe { LLVMTypeOf(value.as_value_ref()) };
        if value_type.is_null() {
            return Err(LlvmError::error(format!(
                "LLVM returned a null type for constant array element {index}"
            )));
        }
        if value_type != element_type {
            return Err(LlvmError::error(format!(
                "constant array element {index} type does not match the element type"
            )));
        }
    }
    Ok(())
}

/// Marker for LLVM aggregate values accepted by extract/insert operations.
pub trait AggregateValue<'ctx>: BasicValue<'ctx> {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
/// Numeric LLVM address space.
pub struct AddressSpace(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Assembly syntax accepted by LLVM inline assembly.
pub enum InlineAsmDialect {
    /// AT&T syntax.
    ATT,
    /// Intel syntax.
    Intel,
}

impl From<InlineAsmDialect> for LLVMInlineAsmDialect {
    fn from(value: InlineAsmDialect) -> Self {
        match value {
            InlineAsmDialect::ATT => LLVMInlineAsmDialect::LLVMInlineAsmDialectATT,
            InlineAsmDialect::Intel => LLVMInlineAsmDialect::LLVMInlineAsmDialectIntel,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Integer comparison predicate with explicit signedness.
pub enum IntPredicate {
    /// Equal.
    EQ,
    /// Not equal.
    NE,
    /// Unsigned greater than.
    UGT,
    /// Unsigned greater than or equal.
    UGE,
    /// Unsigned less than.
    ULT,
    /// Unsigned less than or equal.
    ULE,
    /// Signed greater than.
    SGT,
    /// Signed greater than or equal.
    SGE,
    /// Signed less than.
    SLT,
    /// Signed less than or equal.
    SLE,
}

impl From<IntPredicate> for LLVMIntPredicate {
    fn from(value: IntPredicate) -> Self {
        match value {
            IntPredicate::EQ => LLVMIntPredicate::LLVMIntEQ,
            IntPredicate::NE => LLVMIntPredicate::LLVMIntNE,
            IntPredicate::UGT => LLVMIntPredicate::LLVMIntUGT,
            IntPredicate::UGE => LLVMIntPredicate::LLVMIntUGE,
            IntPredicate::ULT => LLVMIntPredicate::LLVMIntULT,
            IntPredicate::ULE => LLVMIntPredicate::LLVMIntULE,
            IntPredicate::SGT => LLVMIntPredicate::LLVMIntSGT,
            IntPredicate::SGE => LLVMIntPredicate::LLVMIntSGE,
            IntPredicate::SLT => LLVMIntPredicate::LLVMIntSLT,
            IntPredicate::SLE => LLVMIntPredicate::LLVMIntSLE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Ordered floating-point comparison predicate.
pub enum FloatPredicate {
    /// Ordered equal.
    OEQ,
    /// Ordered greater than.
    OGT,
    /// Ordered greater than or equal.
    OGE,
    /// Ordered less than.
    OLT,
    /// Ordered less than or equal.
    OLE,
    /// Ordered not equal.
    ONE,
}

impl From<FloatPredicate> for LLVMRealPredicate {
    fn from(value: FloatPredicate) -> Self {
        match value {
            FloatPredicate::OEQ => LLVMRealPredicate::LLVMRealOEQ,
            FloatPredicate::OGT => LLVMRealPredicate::LLVMRealOGT,
            FloatPredicate::OGE => LLVMRealPredicate::LLVMRealOGE,
            FloatPredicate::OLT => LLVMRealPredicate::LLVMRealOLT,
            FloatPredicate::OLE => LLVMRealPredicate::LLVMRealOLE,
            FloatPredicate::ONE => LLVMRealPredicate::LLVMRealONE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// LLVM atomic memory ordering.
pub enum AtomicOrdering {
    /// Operation is not atomic.
    NotAtomic,
    /// Unordered atomic operation.
    Unordered,
    /// Monotonic ordering.
    Monotonic,
    /// Acquire ordering.
    Acquire,
    /// Release ordering.
    Release,
    /// Combined acquire/release ordering.
    AcquireRelease,
    /// Sequentially consistent ordering.
    SequentiallyConsistent,
}

impl From<AtomicOrdering> for LLVMAtomicOrdering {
    fn from(value: AtomicOrdering) -> Self {
        match value {
            AtomicOrdering::NotAtomic => LLVMAtomicOrdering::LLVMAtomicOrderingNotAtomic,
            AtomicOrdering::Unordered => LLVMAtomicOrdering::LLVMAtomicOrderingUnordered,
            AtomicOrdering::Monotonic => LLVMAtomicOrdering::LLVMAtomicOrderingMonotonic,
            AtomicOrdering::Acquire => LLVMAtomicOrdering::LLVMAtomicOrderingAcquire,
            AtomicOrdering::Release => LLVMAtomicOrdering::LLVMAtomicOrderingRelease,
            AtomicOrdering::AcquireRelease => LLVMAtomicOrdering::LLVMAtomicOrderingAcquireRelease,
            AtomicOrdering::SequentiallyConsistent => {
                LLVMAtomicOrdering::LLVMAtomicOrderingSequentiallyConsistent
            }
        }
    }
}

fn atomic_ordering_from_llvm(value: LLVMAtomicOrdering) -> AtomicOrdering {
    match value {
        LLVMAtomicOrdering::LLVMAtomicOrderingNotAtomic => AtomicOrdering::NotAtomic,
        LLVMAtomicOrdering::LLVMAtomicOrderingUnordered => AtomicOrdering::Unordered,
        LLVMAtomicOrdering::LLVMAtomicOrderingMonotonic => AtomicOrdering::Monotonic,
        LLVMAtomicOrdering::LLVMAtomicOrderingAcquire => AtomicOrdering::Acquire,
        LLVMAtomicOrdering::LLVMAtomicOrderingRelease => AtomicOrdering::Release,
        LLVMAtomicOrdering::LLVMAtomicOrderingAcquireRelease => AtomicOrdering::AcquireRelease,
        LLVMAtomicOrdering::LLVMAtomicOrderingSequentiallyConsistent => {
            AtomicOrdering::SequentiallyConsistent
        }
    }
}

fn cmpxchg_failure_order_allowed(success: AtomicOrdering, failure: AtomicOrdering) -> bool {
    match success {
        AtomicOrdering::Monotonic => failure == AtomicOrdering::Monotonic,
        AtomicOrdering::Acquire => {
            matches!(failure, AtomicOrdering::Monotonic | AtomicOrdering::Acquire)
        }
        AtomicOrdering::Release => failure == AtomicOrdering::Monotonic,
        AtomicOrdering::AcquireRelease => {
            matches!(failure, AtomicOrdering::Monotonic | AtomicOrdering::Acquire)
        }
        AtomicOrdering::SequentiallyConsistent => matches!(
            failure,
            AtomicOrdering::Monotonic
                | AtomicOrdering::Acquire
                | AtomicOrdering::SequentiallyConsistent
        ),
        AtomicOrdering::NotAtomic | AtomicOrdering::Unordered => false,
    }
}

fn validate_instruction_ordering(
    opcode: LLVMOpcode,
    ordering: AtomicOrdering,
    cmpxchg_failure: Option<AtomicOrdering>,
) -> LlvmResult<()> {
    let allowed = match opcode {
        LLVMOpcode::LLVMLoad => matches!(
            ordering,
            AtomicOrdering::NotAtomic
                | AtomicOrdering::Unordered
                | AtomicOrdering::Monotonic
                | AtomicOrdering::Acquire
                | AtomicOrdering::SequentiallyConsistent
        ),
        LLVMOpcode::LLVMStore => matches!(
            ordering,
            AtomicOrdering::NotAtomic
                | AtomicOrdering::Unordered
                | AtomicOrdering::Monotonic
                | AtomicOrdering::Release
                | AtomicOrdering::SequentiallyConsistent
        ),
        LLVMOpcode::LLVMAtomicRMW | LLVMOpcode::LLVMAtomicCmpXchg => matches!(
            ordering,
            AtomicOrdering::Monotonic
                | AtomicOrdering::Acquire
                | AtomicOrdering::Release
                | AtomicOrdering::AcquireRelease
                | AtomicOrdering::SequentiallyConsistent
        ),
        _ => false,
    };
    if !allowed {
        return Err(LlvmError::error(
            "LLVM atomic ordering is not valid for this instruction opcode",
        ));
    }
    if opcode == LLVMOpcode::LLVMAtomicCmpXchg {
        let failure = cmpxchg_failure.ok_or_else(|| {
            LlvmError::error("LLVM compare-exchange failure ordering is unavailable")
        })?;
        if !cmpxchg_failure_order_allowed(ordering, failure) {
            return Err(LlvmError::error(
                "LLVM compare-exchange success ordering is incompatible with failure ordering",
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Read-modify-write operation performed atomically by LLVM.
pub enum AtomicRMWBinOp {
    /// Exchange.
    Xchg,
    /// Addition.
    Add,
    /// Subtraction.
    Sub,
    /// Bitwise AND.
    And,
    /// Bitwise NAND.
    Nand,
    /// Bitwise OR.
    Or,
    /// Bitwise XOR.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// LLVM code-generation optimization level.
pub enum OptimizationLevel {
    /// Disable optimization.
    None,
    /// Optimize cheaply.
    Less,
    /// Use LLVM's default optimization level.
    Default,
    /// Use LLVM's aggressive optimization level.
    Aggressive,
}

impl From<AtomicRMWBinOp> for LLVMAtomicRMWBinOp {
    fn from(value: AtomicRMWBinOp) -> Self {
        match value {
            AtomicRMWBinOp::Xchg => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpXchg,
            AtomicRMWBinOp::Add => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpAdd,
            AtomicRMWBinOp::Sub => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpSub,
            AtomicRMWBinOp::And => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpAnd,
            AtomicRMWBinOp::Nand => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpNand,
            AtomicRMWBinOp::Or => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpOr,
            AtomicRMWBinOp::Xor => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpXor,
            AtomicRMWBinOp::Max => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpMax,
            AtomicRMWBinOp::Min => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpMin,
            AtomicRMWBinOp::UMax => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpUMax,
            AtomicRMWBinOp::UMin => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpUMin,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Linkage supported by Nia's module emitter.
pub enum Linkage {
    /// Externally visible definition or declaration.
    External,
    /// Appending linkage used for concatenated globals.
    Appending,
    /// ODR link-once definition.
    LinkOnceOdr,
    /// Weak ODR definition.
    WeakOdr,
    /// Module-local definition.
    Internal,
}

impl From<Linkage> for LLVMLinkage {
    fn from(value: Linkage) -> Self {
        match value {
            Linkage::External => LLVMLinkage::LLVMExternalLinkage,
            Linkage::Appending => LLVMLinkage::LLVMAppendingLinkage,
            Linkage::LinkOnceOdr => LLVMLinkage::LLVMLinkOnceODRLinkage,
            Linkage::WeakOdr => LLVMLinkage::LLVMWeakODRLinkage,
            Linkage::Internal => LLVMLinkage::LLVMInternalLinkage,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Location at which an LLVM attribute is attached.
pub enum AttributeLoc {
    /// Function-level attribute index.
    Function,
}

#[derive(Debug, Clone, Copy)]
/// Non-owning LLVM attribute handle tied to its originating context.
pub struct Attribute {
    pub(super) raw: LLVMAttributeRef,
}

impl Attribute {
    pub(super) fn new(raw: LLVMAttributeRef) -> LlvmResult<Self> {
        if raw.is_null() {
            Err(LlvmError::error("LLVM returned a null attribute"))
        } else {
            Ok(Self { raw })
        }
    }

    /// Resolves LLVM's numeric id for a named enum attribute.
    pub fn get_named_enum_kind_id(name: &str) -> u32 {
        unsafe { LLVMGetEnumAttributeKindForName(name.as_ptr() as *const _, name.len()) }
    }
}

macro_rules! impl_type_wrapper {
    ($doc:literal, $name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[doc = $doc]
        pub struct $name<'ctx> {
            pub(super) raw: LLVMTypeRef,
            pub(super) _marker: PhantomData<&'ctx Context>,
        }

        impl<'ctx> $name<'ctx> {
            pub(super) fn new(raw: LLVMTypeRef) -> Self {
                assert!(!raw.is_null());
                Self {
                    raw,
                    _marker: PhantomData,
                }
            }
        }

        impl<'ctx> AsTypeRef for $name<'ctx> {
            fn as_type_ref(&self) -> LLVMTypeRef {
                self.raw
            }
        }
    };
}

impl_type_wrapper!("Non-owning LLVM void type handle.", VoidType);
impl_type_wrapper!("Non-owning LLVM integer type handle.", IntType);
impl_type_wrapper!("Non-owning LLVM floating-point type handle.", FloatType);
impl_type_wrapper!("Non-owning LLVM opaque pointer type handle.", PointerType);
impl_type_wrapper!("Non-owning LLVM struct type handle.", StructType);
impl_type_wrapper!("Non-owning LLVM fixed-size array type handle.", ArrayType);
impl_type_wrapper!(
    "Non-owning LLVM fixed-width vector type handle.",
    VectorType
);
impl_type_wrapper!(
    "Non-owning LLVM scalable-vector type handle.",
    ScalableVectorType
);
impl_type_wrapper!("Non-owning LLVM function signature handle.", FunctionType);

impl<'ctx> VoidType<'ctx> {
    /// Creates a function signature returning `void`.
    pub fn fn_type(
        self,
        params: &[BasicMetadataTypeEnum<'ctx>],
        variadic: bool,
    ) -> LlvmResult<FunctionType<'ctx>> {
        let param_count =
            checked_u32_count(params.len(), "LLVM function type has too many parameters")?;
        let mut params = params
            .iter()
            .map(|param| param.as_type_ref())
            .collect::<Vec<_>>();
        let raw = unsafe {
            LLVMFunctionType(
                self.as_type_ref(),
                params.as_mut_ptr(),
                param_count,
                bool_to_llvm(variadic),
            )
        };
        if raw.is_null() {
            return Err(LlvmError::error("LLVM returned a null function type"));
        }
        Ok(FunctionType::new(raw))
    }
}

macro_rules! impl_basic_type_methods {
    ($name:ident) => {
        impl<'ctx> $name<'ctx> {
            /// Creates a function signature returning this type.
            pub fn fn_type(
                self,
                params: &[BasicMetadataTypeEnum<'ctx>],
                variadic: bool,
            ) -> LlvmResult<FunctionType<'ctx>> {
                let param_count =
                    checked_u32_count(params.len(), "LLVM function type has too many parameters")?;
                let mut params = params
                    .iter()
                    .map(|param| param.as_type_ref())
                    .collect::<Vec<_>>();
                let raw = unsafe {
                    LLVMFunctionType(
                        self.as_type_ref(),
                        params.as_mut_ptr(),
                        param_count,
                        bool_to_llvm(variadic),
                    )
                };
                if raw.is_null() {
                    return Err(LlvmError::error("LLVM returned a null function type"));
                }
                Ok(FunctionType::new(raw))
            }

            /// Creates a fixed-size array of this element type.
            pub fn array_type(self, len: u32) -> LlvmResult<ArrayType<'ctx>> {
                let raw = unsafe { LLVMArrayType2(self.as_type_ref(), len as u64) };
                if raw.is_null() {
                    return Err(LlvmError::error("LLVM returned a null array type"));
                }
                Ok(ArrayType::new(raw))
            }
        }
    };
}

impl_basic_type_methods!(IntType);
impl_basic_type_methods!(FloatType);
impl_basic_type_methods!(PointerType);
impl_basic_type_methods!(StructType);
impl_basic_type_methods!(ArrayType);
impl_basic_type_methods!(VectorType);
impl_basic_type_methods!(ScalableVectorType);

impl<'ctx> IntType<'ctx> {
    /// Returns this integer type's bit width.
    pub fn bit_width(self) -> u32 {
        unsafe { LLVMGetIntTypeWidth(self.as_type_ref()) }
    }

    /// Creates an integer constant from its low 64 bits.
    pub fn const_int(self, value: u64, sign_extend: bool) -> IntValue<'ctx> {
        IntValue::new(unsafe { LLVMConstInt(self.as_type_ref(), value, bool_to_llvm(sign_extend)) })
    }

    /// Creates an unsigned integer constant up to 128 bits wide.
    pub fn const_u128(self, value: u128) -> IntValue<'ctx> {
        if self.bit_width() <= 64 {
            return self.const_int(value as u64, false);
        }

        let words = [value as u64, (value >> 64) as u64];
        IntValue::new(unsafe {
            LLVMConstIntOfArbitraryPrecision(self.as_type_ref(), words.len() as u32, words.as_ptr())
        })
    }

    /// Creates an array constant from integer elements of this type.
    pub fn const_array(self, values: &[IntValue<'ctx>]) -> LlvmResult<ArrayValue<'ctx>> {
        validate_constant_elements(values, self.as_type_ref())?;
        let mut values = values
            .iter()
            .map(|value| value.as_value_ref())
            .collect::<Vec<_>>();
        let raw = unsafe {
            LLVMConstArray2(self.as_type_ref(), values.as_mut_ptr(), values.len() as u64)
        };
        Ok(ArrayValue::new(require_value(
            raw,
            "integer constant array",
        )?))
    }

    /// Creates the zero constant.
    pub fn const_zero(self) -> LlvmResult<IntValue<'ctx>> {
        Ok(IntValue::new(require_value(
            unsafe { LLVMConstInt(self.as_type_ref(), 0, 0) },
            "integer zero constant",
        )?))
    }

    /// Creates an undefined value of this type.
    pub fn get_undef(self) -> LlvmResult<IntValue<'ctx>> {
        Ok(IntValue::new(require_value(
            unsafe { LLVMGetUndef(self.as_type_ref()) },
            "undefined integer value",
        )?))
    }
}

impl<'ctx> IntValue<'ctx> {
    /// Constant-folds a bitcast to another integer type.
    pub fn const_bitcast(self, target_ty: IntType<'ctx>) -> LlvmResult<IntValue<'ctx>> {
        if self.get_type()?.bit_width() != target_ty.bit_width() {
            return Err(LlvmError::error(
                "integer constant bitcast requires equal source and target widths",
            ));
        }
        let raw = unsafe { LLVMConstBitCast(self.as_value_ref(), target_ty.as_type_ref()) };
        if raw.is_null() {
            return Err(LlvmError::error("LLVM returned a null integer bitcast"));
        }
        Ok(IntValue::new(raw))
    }
}

impl<'ctx> FloatType<'ctx> {
    /// Creates a floating-point constant, rounded to this type's precision.
    pub fn const_float(self, value: f64) -> FloatValue<'ctx> {
        FloatValue::new(unsafe { LLVMConstReal(self.as_type_ref(), value) })
    }

    /// Creates an array constant from floating-point elements of this type.
    pub fn const_array(self, values: &[FloatValue<'ctx>]) -> LlvmResult<ArrayValue<'ctx>> {
        validate_constant_elements(values, self.as_type_ref())?;
        let mut values = values
            .iter()
            .map(|value| value.as_value_ref())
            .collect::<Vec<_>>();
        let raw = unsafe {
            LLVMConstArray2(self.as_type_ref(), values.as_mut_ptr(), values.len() as u64)
        };
        Ok(ArrayValue::new(require_value(
            raw,
            "floating constant array",
        )?))
    }

    /// Creates positive floating-point zero.
    pub fn const_zero(self) -> LlvmResult<FloatValue<'ctx>> {
        Ok(FloatValue::new(require_value(
            unsafe { LLVMConstReal(self.as_type_ref(), 0.0) },
            "floating-point zero constant",
        )?))
    }

    /// Creates an undefined value of this type.
    pub fn get_undef(self) -> LlvmResult<FloatValue<'ctx>> {
        Ok(FloatValue::new(require_value(
            unsafe { LLVMGetUndef(self.as_type_ref()) },
            "undefined floating-point value",
        )?))
    }
}

impl<'ctx> PointerType<'ctx> {
    /// Returns this opaque pointer type's numeric address space.
    pub fn address_space(self) -> AddressSpace {
        AddressSpace(unsafe { LLVMGetPointerAddressSpace(self.as_type_ref()) })
    }

    /// Creates a null pointer constant.
    pub fn const_zero(self) -> LlvmResult<PointerValue<'ctx>> {
        self.const_null()
    }

    /// Creates a null pointer constant.
    pub fn const_null(self) -> LlvmResult<PointerValue<'ctx>> {
        Ok(PointerValue::new(require_value(
            unsafe { LLVMConstPointerNull(self.as_type_ref()) },
            "null pointer constant",
        )?))
    }

    /// Constant-folds an integer-to-pointer conversion.
    pub fn const_int_to_ptr(self, value: IntValue<'ctx>) -> LlvmResult<PointerValue<'ctx>> {
        Ok(PointerValue::new(require_value(
            unsafe { LLVMConstIntToPtr(value.as_value_ref(), self.as_type_ref()) },
            "integer-to-pointer constant",
        )?))
    }

    /// Creates an undefined pointer value.
    pub fn get_undef(self) -> LlvmResult<PointerValue<'ctx>> {
        Ok(PointerValue::new(require_value(
            unsafe { LLVMGetUndef(self.as_type_ref()) },
            "undefined pointer value",
        )?))
    }

    /// Creates an array constant from pointer elements of this type.
    pub fn const_array(self, values: &[PointerValue<'ctx>]) -> LlvmResult<ArrayValue<'ctx>> {
        validate_constant_elements(values, self.as_type_ref())?;
        let mut values = values
            .iter()
            .map(|value| value.as_value_ref())
            .collect::<Vec<_>>();
        let raw = unsafe {
            LLVMConstArray2(self.as_type_ref(), values.as_mut_ptr(), values.len() as u64)
        };
        Ok(ArrayValue::new(require_value(
            raw,
            "pointer constant array",
        )?))
    }
}

impl<'ctx> StructType<'ctx> {
    /// Converts this typed handle to the shared basic-type enum.
    pub fn as_basic_type_enum(self) -> BasicTypeEnum<'ctx> {
        BasicTypeEnum::StructType(self)
    }

    /// Defines the physical fields of a named opaque struct type.
    pub fn set_body(self, fields: &[BasicTypeEnum<'ctx>], packed: bool) -> LlvmResult<()> {
        let raw = self.as_type_ref();
        if unsafe { LLVMIsLiteralStruct(raw) } != 0 {
            return Err(LlvmError::error(
                "LLVM struct body requires a named opaque struct",
            ));
        }
        if unsafe { LLVMIsOpaqueStruct(raw) } == 0 {
            return Err(LlvmError::error(
                "LLVM struct body can only be assigned once to an opaque struct",
            ));
        }
        let field_count = u32::try_from(fields.len())
            .map_err(|_| LlvmError::error("LLVM struct body has too many fields"))?;
        let mut fields = fields
            .iter()
            .map(|field| field.as_type_ref())
            .collect::<Vec<_>>();
        unsafe {
            LLVMStructSetBody(
                self.as_type_ref(),
                fields.as_mut_ptr(),
                field_count,
                bool_to_llvm(packed),
            )
        };
        Ok(())
    }

    /// Returns the number of physical struct fields.
    pub fn count_fields(self) -> u32 {
        unsafe { LLVMCountStructElementTypes(self.as_type_ref()) }
    }

    /// Reports whether field alignment padding is disabled.
    pub fn is_packed(self) -> bool {
        unsafe { LLVMIsPackedStruct(self.as_type_ref()) != 0 }
    }

    /// Returns a field type, or `None` when `index` is out of bounds.
    pub fn get_field_type_at_index(self, index: u32) -> Option<LlvmResult<BasicTypeEnum<'ctx>>> {
        if index >= self.count_fields() {
            None
        } else {
            Some(BasicTypeEnum::new(unsafe {
                LLVMStructGetTypeAtIndex(self.as_type_ref(), index)
            }))
        }
    }

    /// Creates a constant using this named struct's physical field order.
    pub fn const_named_struct(
        self,
        values: &[BasicValueEnum<'ctx>],
    ) -> LlvmResult<StructValue<'ctx>> {
        let field_count = self.count_fields();
        if values.len() != field_count as usize {
            return Err(LlvmError::error(format!(
                "struct constant has {} values, expected {field_count}",
                values.len()
            )));
        }
        for (index, value) in values.iter().enumerate() {
            let field_ty = self
                .get_field_type_at_index(
                    u32::try_from(index)
                        .map_err(|_| LlvmError::error("struct field index exceeds LLVM width"))?,
                )
                .ok_or_else(|| LlvmError::error("struct field disappeared during inspection"))??;
            let value_ty = value.get_type()?;
            if value_ty != field_ty {
                return Err(LlvmError::error(format!(
                    "struct constant field {index} type does not match struct field type"
                )));
            }
        }
        let mut values = values
            .iter()
            .map(|value| value.as_value_ref())
            .collect::<Vec<_>>();
        let raw =
            unsafe { LLVMConstNamedStruct(self.as_type_ref(), values.as_mut_ptr(), field_count) };
        Ok(StructValue::new(require_value(
            raw,
            "named struct constant",
        )?))
    }

    /// Creates a recursively zero-initialized struct constant.
    pub fn const_zero(self) -> LlvmResult<StructValue<'ctx>> {
        Ok(StructValue::new(require_value(
            unsafe { LLVMConstNull(self.as_type_ref()) },
            "struct zero constant",
        )?))
    }

    /// Creates an undefined struct value.
    pub fn get_undef(self) -> LlvmResult<StructValue<'ctx>> {
        Ok(StructValue::new(require_value(
            unsafe { LLVMGetUndef(self.as_type_ref()) },
            "undefined struct value",
        )?))
    }

    /// Creates an array constant from struct elements of this type.
    pub fn const_array(self, values: &[StructValue<'ctx>]) -> LlvmResult<ArrayValue<'ctx>> {
        validate_constant_elements(values, self.as_type_ref())?;
        let mut values = values
            .iter()
            .map(|value| value.as_value_ref())
            .collect::<Vec<_>>();
        let raw = unsafe {
            LLVMConstArray2(self.as_type_ref(), values.as_mut_ptr(), values.len() as u64)
        };
        Ok(ArrayValue::new(require_value(
            raw,
            "struct constant array",
        )?))
    }
}

impl<'ctx> ArrayType<'ctx> {
    /// Returns the fixed array length reported by LLVM.
    pub fn len(self) -> u64 {
        unsafe { llvm_sys::core::LLVMGetArrayLength2(self.as_type_ref()) }
    }

    /// Returns the array element type.
    pub fn get_element_type(self) -> LlvmResult<BasicTypeEnum<'ctx>> {
        BasicTypeEnum::new(unsafe { LLVMGetElementType(self.as_type_ref()) })
    }

    /// Reports whether the array has zero elements.
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// Creates a recursively zero-initialized array constant.
    pub fn const_zero(self) -> LlvmResult<ArrayValue<'ctx>> {
        Ok(ArrayValue::new(require_value(
            unsafe { LLVMConstNull(self.as_type_ref()) },
            "array zero constant",
        )?))
    }

    /// Creates an undefined array value.
    pub fn get_undef(self) -> LlvmResult<ArrayValue<'ctx>> {
        Ok(ArrayValue::new(require_value(
            unsafe { LLVMGetUndef(self.as_type_ref()) },
            "undefined array value",
        )?))
    }

    /// Creates a nested array constant from elements of this array type.
    pub fn const_array(self, values: &[ArrayValue<'ctx>]) -> LlvmResult<ArrayValue<'ctx>> {
        let elem_ty = unsafe { LLVMGetElementType(self.as_type_ref()) };
        validate_constant_elements(values, elem_ty)?;
        let mut values = values
            .iter()
            .map(|value| value.as_value_ref())
            .collect::<Vec<_>>();
        let raw = unsafe { LLVMConstArray2(elem_ty, values.as_mut_ptr(), values.len() as u64) };
        Ok(ArrayValue::new(require_value(raw, "array constant array")?))
    }
}

impl<'ctx> VectorType<'ctx> {
    /// Returns the fixed vector lane count.
    pub fn len(self) -> u32 {
        unsafe { LLVMGetVectorSize(self.as_type_ref()) }
    }

    /// Reports whether the vector has zero lanes.
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// Returns the vector lane type.
    pub fn get_element_type(self) -> LlvmResult<BasicTypeEnum<'ctx>> {
        BasicTypeEnum::new(unsafe { LLVMGetElementType(self.as_type_ref()) })
    }

    /// Creates a vector constant after checking lane count and lane types.
    pub fn const_vector(self, values: &[BasicValueEnum<'ctx>]) -> LlvmResult<VectorValue<'ctx>> {
        if values.len() != self.len() as usize {
            return Err(LlvmError::error(
                "constant vector lane count does not match type",
            ));
        }
        let elem_ty = self.get_element_type()?;
        for (index, value) in values.iter().enumerate() {
            if value.get_type()? != elem_ty {
                return Err(LlvmError::error(format!(
                    "constant vector lane {index} type does not match type"
                )));
            }
        }
        let mut values = values
            .iter()
            .map(|value| value.as_value_ref())
            .collect::<Vec<_>>();
        let raw = unsafe { LLVMConstVector(values.as_mut_ptr(), self.len()) };
        Ok(VectorValue::new(require_value(raw, "vector constant")?))
    }

    /// Creates a zero-initialized vector constant.
    pub fn const_zero(self) -> LlvmResult<BasicValueEnum<'ctx>> {
        BasicValueEnum::new(unsafe { LLVMConstNull(self.as_type_ref()) })
    }

    /// Creates an undefined vector value.
    pub fn get_undef(self) -> LlvmResult<BasicValueEnum<'ctx>> {
        BasicValueEnum::new(unsafe { LLVMGetUndef(self.as_type_ref()) })
    }
}

impl<'ctx> ScalableVectorType<'ctx> {
    /// Creates a zero-initialized scalable-vector constant.
    pub fn const_zero(self) -> LlvmResult<BasicValueEnum<'ctx>> {
        BasicValueEnum::new(unsafe { LLVMConstNull(self.as_type_ref()) })
    }

    /// Creates an undefined scalable-vector value.
    pub fn get_undef(self) -> LlvmResult<BasicValueEnum<'ctx>> {
        BasicValueEnum::new(unsafe { LLVMGetUndef(self.as_type_ref()) })
    }
}

impl<'ctx> FunctionType<'ctx> {
    /// Returns `None` for void signatures or the typed return value type.
    pub fn get_return_type(self) -> LlvmResult<Option<BasicTypeEnum<'ctx>>> {
        classify_return_type(unsafe { LLVMGetReturnType(self.as_type_ref()) })
    }

    /// Returns the fixed parameters in this signature.
    pub fn get_param_types(self) -> LlvmResult<Vec<BasicTypeEnum<'ctx>>> {
        let count = unsafe { LLVMCountParamTypes(self.as_type_ref()) } as usize;
        let mut raw_types = vec![std::ptr::null_mut(); count];
        unsafe { LLVMGetParamTypes(self.as_type_ref(), raw_types.as_mut_ptr()) };
        raw_types.into_iter().map(BasicTypeEnum::new).collect()
    }

    /// Reports whether this signature accepts arguments beyond its fixed list.
    pub fn is_variadic(self) -> bool {
        unsafe { llvm_sys::core::LLVMIsFunctionVarArg(self.as_type_ref()) != 0 }
    }
}

/// Classifies the raw return type before querying its kind. LLVM's C API uses
/// a null type handle as its failure sentinel, and `LLVMGetTypeKind` requires a
/// live handle, so the check must happen at this boundary.
fn classify_return_type<'ctx>(raw: LLVMTypeRef) -> LlvmResult<Option<BasicTypeEnum<'ctx>>> {
    if raw.is_null() {
        return Err(LlvmError::error(
            "LLVM returned a null function return type",
        ));
    }
    if unsafe { LLVMGetTypeKind(raw) } == LLVMTypeKind::LLVMVoidTypeKind {
        Ok(None)
    } else {
        BasicTypeEnum::new(raw).map(Some)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Any first-class, non-void LLVM type supported by Nia codegen.
pub enum BasicTypeEnum<'ctx> {
    /// Fixed-size array type.
    ArrayType(ArrayType<'ctx>),
    /// Floating-point type.
    FloatType(FloatType<'ctx>),
    /// Integer type.
    IntType(IntType<'ctx>),
    /// Opaque pointer type.
    PointerType(PointerType<'ctx>),
    /// Struct type.
    StructType(StructType<'ctx>),
    /// Fixed-width vector type.
    VectorType(VectorType<'ctx>),
    /// Scalable-vector type.
    ScalableVectorType(ScalableVectorType<'ctx>),
}

impl<'ctx> BasicTypeEnum<'ctx> {
    pub(super) fn new(raw: LLVMTypeRef) -> LlvmResult<Self> {
        // LLVM type inspection requires a live handle. Check before calling
        // `LLVMGetTypeKind`; otherwise an upstream null return would cross the
        // FFI boundary despite this constructor exposing a fallible API.
        if raw.is_null() {
            return Err(LlvmError::error("LLVM returned a null basic type"));
        }
        match unsafe { LLVMGetTypeKind(raw) } {
            LLVMTypeKind::LLVMArrayTypeKind => Ok(Self::ArrayType(ArrayType::new(raw))),
            LLVMTypeKind::LLVMFloatTypeKind | LLVMTypeKind::LLVMDoubleTypeKind => {
                Ok(Self::FloatType(FloatType::new(raw)))
            }
            LLVMTypeKind::LLVMIntegerTypeKind => Ok(Self::IntType(IntType::new(raw))),
            LLVMTypeKind::LLVMPointerTypeKind => Ok(Self::PointerType(PointerType::new(raw))),
            LLVMTypeKind::LLVMStructTypeKind => Ok(Self::StructType(StructType::new(raw))),
            LLVMTypeKind::LLVMVectorTypeKind => Ok(Self::VectorType(VectorType::new(raw))),
            LLVMTypeKind::LLVMScalableVectorTypeKind => {
                Ok(Self::ScalableVectorType(ScalableVectorType::new(raw)))
            }
            other => Err(LlvmError::ice(format!(
                "unsupported LLVM basic type kind: {other:?}"
            ))),
        }
    }

    /// Creates the type's canonical zero value.
    pub fn const_zero(self) -> LlvmResult<BasicValueEnum<'ctx>> {
        Ok(match self {
            Self::ArrayType(t) => BasicValueEnum::new(unsafe { LLVMConstNull(t.as_type_ref()) })?,
            Self::FloatType(t) => t.const_zero()?.into(),
            Self::IntType(t) => t.const_zero()?.into(),
            Self::PointerType(t) => t.const_zero()?.into(),
            Self::StructType(t) => BasicValueEnum::new(unsafe { LLVMConstNull(t.as_type_ref()) })?,
            Self::VectorType(t) => t.const_zero()?,
            Self::ScalableVectorType(t) => t.const_zero()?,
        })
    }

    /// Creates a fixed-size array of this element type.
    pub fn array_type(self, len: u32) -> LlvmResult<ArrayType<'ctx>> {
        let raw = unsafe { LLVMArrayType2(self.as_type_ref(), len as u64) };
        if raw.is_null() {
            return Err(LlvmError::error("LLVM returned a null array type"));
        }
        Ok(ArrayType::new(raw))
    }

    /// Creates a function signature returning this type.
    pub fn fn_type(
        self,
        params: &[BasicMetadataTypeEnum<'ctx>],
        variadic: bool,
    ) -> LlvmResult<FunctionType<'ctx>> {
        let param_count =
            checked_u32_count(params.len(), "LLVM function type has too many parameters")?;
        let mut params = params
            .iter()
            .map(|param| param.as_type_ref())
            .collect::<Vec<_>>();
        let raw = unsafe {
            LLVMFunctionType(
                self.as_type_ref(),
                params.as_mut_ptr(),
                param_count,
                bool_to_llvm(variadic),
            )
        };
        if raw.is_null() {
            return Err(LlvmError::error("LLVM returned a null function type"));
        }
        Ok(FunctionType::new(raw))
    }

    /// Creates a fixed-width vector of this lane type.
    pub fn vector_type(self, len: u32) -> LlvmResult<VectorType<'ctx>> {
        if len == 0 {
            return Err(LlvmError::error(
                "LLVM fixed vector type requires at least one lane",
            ));
        }
        let raw = unsafe { LLVMVectorType(self.as_type_ref(), len) };
        if raw.is_null() {
            return Err(LlvmError::error("LLVM returned a null vector type"));
        }
        Ok(VectorType::new(raw))
    }

    /// Reports whether this enum contains a pointer type.
    pub fn is_pointer_type(self) -> bool {
        matches!(self, Self::PointerType(_))
    }

    /// Extracts an array type or reports an invariant failure.
    pub fn into_array_type(self) -> LlvmResult<ArrayType<'ctx>> {
        match self {
            Self::ArrayType(value) => Ok(value),
            _ => Err(LlvmError::ice("expected array type")),
        }
    }

    /// Extracts a floating-point type or reports an invariant failure.
    pub fn into_float_type(self) -> LlvmResult<FloatType<'ctx>> {
        match self {
            Self::FloatType(value) => Ok(value),
            _ => Err(LlvmError::ice("expected float type")),
        }
    }

    /// Extracts an integer type or reports an invariant failure.
    pub fn into_int_type(self) -> LlvmResult<IntType<'ctx>> {
        match self {
            Self::IntType(value) => Ok(value),
            _ => Err(LlvmError::ice("expected int type")),
        }
    }

    /// Extracts a pointer type or reports an invariant failure.
    pub fn into_pointer_type(self) -> LlvmResult<PointerType<'ctx>> {
        match self {
            Self::PointerType(value) => Ok(value),
            _ => Err(LlvmError::ice("expected pointer type")),
        }
    }

    /// Extracts a struct type or reports an invariant failure.
    pub fn into_struct_type(self) -> LlvmResult<StructType<'ctx>> {
        match self {
            Self::StructType(value) => Ok(value),
            _ => Err(LlvmError::ice("expected struct type")),
        }
    }

    /// Extracts a fixed-width vector type or reports an invariant failure.
    pub fn into_vector_type(self) -> LlvmResult<VectorType<'ctx>> {
        match self {
            Self::VectorType(value) => Ok(value),
            _ => Err(LlvmError::ice("expected vector type")),
        }
    }
}

impl<'ctx> AsTypeRef for BasicTypeEnum<'ctx> {
    fn as_type_ref(&self) -> LLVMTypeRef {
        match self {
            Self::ArrayType(value) => value.as_type_ref(),
            Self::FloatType(value) => value.as_type_ref(),
            Self::IntType(value) => value.as_type_ref(),
            Self::PointerType(value) => value.as_type_ref(),
            Self::StructType(value) => value.as_type_ref(),
            Self::VectorType(value) => value.as_type_ref(),
            Self::ScalableVectorType(value) => value.as_type_ref(),
        }
    }
}

impl<'ctx> BasicType<'ctx> for BasicTypeEnum<'ctx> {}
impl<'ctx> BasicType<'ctx> for IntType<'ctx> {}
impl<'ctx> BasicType<'ctx> for FloatType<'ctx> {}
impl<'ctx> BasicType<'ctx> for PointerType<'ctx> {}
impl<'ctx> BasicType<'ctx> for StructType<'ctx> {}
impl<'ctx> BasicType<'ctx> for ArrayType<'ctx> {}
impl<'ctx> BasicType<'ctx> for VectorType<'ctx> {}
impl<'ctx> BasicType<'ctx> for ScalableVectorType<'ctx> {}

impl<'ctx> From<IntType<'ctx>> for BasicTypeEnum<'ctx> {
    fn from(value: IntType<'ctx>) -> Self {
        Self::IntType(value)
    }
}

impl<'ctx> From<FloatType<'ctx>> for BasicTypeEnum<'ctx> {
    fn from(value: FloatType<'ctx>) -> Self {
        Self::FloatType(value)
    }
}

impl<'ctx> From<PointerType<'ctx>> for BasicTypeEnum<'ctx> {
    fn from(value: PointerType<'ctx>) -> Self {
        Self::PointerType(value)
    }
}

impl<'ctx> From<StructType<'ctx>> for BasicTypeEnum<'ctx> {
    fn from(value: StructType<'ctx>) -> Self {
        Self::StructType(value)
    }
}

impl<'ctx> From<ArrayType<'ctx>> for BasicTypeEnum<'ctx> {
    fn from(value: ArrayType<'ctx>) -> Self {
        Self::ArrayType(value)
    }
}

impl<'ctx> From<VectorType<'ctx>> for BasicTypeEnum<'ctx> {
    fn from(value: VectorType<'ctx>) -> Self {
        Self::VectorType(value)
    }
}

impl<'ctx> From<ScalableVectorType<'ctx>> for BasicTypeEnum<'ctx> {
    fn from(value: ScalableVectorType<'ctx>) -> Self {
        Self::ScalableVectorType(value)
    }
}

/// LLVM metadata-argument type supported by this wrapper.
pub type BasicMetadataTypeEnum<'ctx> = BasicTypeEnum<'ctx>;

macro_rules! impl_value_wrapper {
    ($doc:literal, $name:ident, $basic_method:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[doc = $doc]
        pub struct $name<'ctx> {
            pub(super) raw: LLVMValueRef,
            pub(super) _marker: PhantomData<&'ctx Context>,
        }

        impl<'ctx> $name<'ctx> {
            pub(super) fn new(raw: LLVMValueRef) -> Self {
                assert!(!raw.is_null());
                Self {
                    raw,
                    _marker: PhantomData,
                }
            }
        }

        impl<'ctx> AsValueRef for $name<'ctx> {
            fn as_value_ref(&self) -> LLVMValueRef {
                self.raw
            }
        }

        impl<'ctx> BasicValue<'ctx> for $name<'ctx> {
            fn as_basic_value_enum(&self) -> BasicValueEnum<'ctx> {
                BasicValueEnum::$basic_method(*self)
            }
        }
    };
}

impl_value_wrapper!("Non-owning LLVM integer value handle.", IntValue, IntValue);
impl_value_wrapper!(
    "Non-owning LLVM floating-point value handle.",
    FloatValue,
    FloatValue
);
impl_value_wrapper!(
    "Non-owning LLVM pointer value handle.",
    PointerValue,
    PointerValue
);
impl_value_wrapper!(
    "Non-owning LLVM struct value handle.",
    StructValue,
    StructValue
);
impl_value_wrapper!(
    "Non-owning LLVM array value handle.",
    ArrayValue,
    ArrayValue
);
impl_value_wrapper!(
    "Non-owning LLVM fixed-width vector value handle.",
    VectorValue,
    VectorValue
);
impl_value_wrapper!(
    "Non-owning LLVM scalable-vector value handle.",
    ScalableVectorValue,
    ScalableVectorValue
);

impl<'ctx> AggregateValue<'ctx> for StructValue<'ctx> {}
impl<'ctx> AggregateValue<'ctx> for ArrayValue<'ctx> {}

impl<'ctx> IntValue<'ctx> {
    /// Returns this value's integer type.
    pub fn get_type(self) -> LlvmResult<IntType<'ctx>> {
        BasicTypeEnum::new(unsafe { LLVMTypeOf(self.raw) })?.into_int_type()
    }
}

impl<'ctx> FloatValue<'ctx> {
    /// Returns this value's floating-point type.
    pub fn get_type(self) -> LlvmResult<FloatType<'ctx>> {
        BasicTypeEnum::new(unsafe { LLVMTypeOf(self.raw) })?.into_float_type()
    }
}

impl<'ctx> PointerValue<'ctx> {
    /// Returns this value's opaque pointer type.
    pub fn get_type(self) -> LlvmResult<PointerType<'ctx>> {
        BasicTypeEnum::new(unsafe { LLVMTypeOf(self.raw) })?.into_pointer_type()
    }

    /// Constant-folds a pointer bitcast.
    pub fn const_bitcast(self, target_ty: PointerType<'ctx>) -> LlvmResult<PointerValue<'ctx>> {
        if self.get_type()?.address_space() != target_ty.address_space() {
            return Err(LlvmError::error(
                "pointer constant bitcast requires equal address spaces",
            ));
        }
        let raw = unsafe { LLVMConstBitCast(self.as_value_ref(), target_ty.as_type_ref()) };
        if raw.is_null() {
            return Err(LlvmError::error("LLVM returned a null pointer bitcast"));
        }
        Ok(PointerValue::new(raw))
    }

    /// Builds a constant GEP expression for this pointer.
    ///
    /// # Safety
    ///
    /// `pointee_ty` must describe the type currently pointed to by `self`, and
    /// every index must be valid for that aggregate layout according to LLVM's
    /// `LLVMConstGEP2` contract.
    pub unsafe fn const_gep<T: AsTypeRef>(
        self,
        pointee_ty: T,
        indices: &[IntValue<'ctx>],
    ) -> LlvmResult<PointerValue<'ctx>> {
        let index_count = checked_u32_count(indices.len(), "LLVM GEP has too many indices")?;
        let mut indices = indices
            .iter()
            .map(|index| index.as_value_ref())
            .collect::<Vec<_>>();
        let raw = unsafe {
            LLVMConstGEP2(
                pointee_ty.as_type_ref(),
                self.as_value_ref(),
                indices.as_mut_ptr(),
                index_count,
            )
        };
        if raw.is_null() {
            return Err(LlvmError::error("LLVM returned a null constant GEP"));
        }
        Ok(PointerValue::new(raw))
    }

    /// Builds a constant in-bounds GEP expression for this pointer.
    ///
    /// # Safety
    ///
    /// This has the same requirements as [`PointerValue::const_gep`]. In
    /// addition, the resulting address must stay within the allocation or one
    /// byte past it, as required by LLVM's in-bounds GEP semantics.
    pub unsafe fn const_in_bounds_gep<T: AsTypeRef>(
        self,
        pointee_ty: T,
        indices: &[IntValue<'ctx>],
    ) -> LlvmResult<PointerValue<'ctx>> {
        let index_count = checked_u32_count(indices.len(), "LLVM GEP has too many indices")?;
        let mut indices = indices
            .iter()
            .map(|index| index.as_value_ref())
            .collect::<Vec<_>>();
        let raw = unsafe {
            LLVMConstInBoundsGEP2(
                pointee_ty.as_type_ref(),
                self.as_value_ref(),
                indices.as_mut_ptr(),
                index_count,
            )
        };
        if raw.is_null() {
            return Err(LlvmError::error(
                "LLVM returned a null constant in-bounds GEP",
            ));
        }
        Ok(PointerValue::new(raw))
    }
}

impl<'ctx> StructValue<'ctx> {
    /// Returns this value's struct type.
    pub fn get_type(self) -> LlvmResult<StructType<'ctx>> {
        BasicTypeEnum::new(unsafe { LLVMTypeOf(self.raw) })?.into_struct_type()
    }

    /// Views the value as an instruction when it was produced by one.
    pub fn as_instruction(self) -> Option<InstructionValue<'ctx>> {
        let value = unsafe { LLVMIsAInstruction(self.raw) };
        if value.is_null() {
            None
        } else {
            Some(InstructionValue::new(value))
        }
    }
}

impl<'ctx> ArrayValue<'ctx> {
    /// Returns this value's fixed-size array type.
    pub fn get_type(self) -> LlvmResult<ArrayType<'ctx>> {
        BasicTypeEnum::new(unsafe { LLVMTypeOf(self.raw) })?.into_array_type()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Any first-class LLVM value supported by Nia codegen.
pub enum BasicValueEnum<'ctx> {
    /// Fixed-size array value.
    ArrayValue(ArrayValue<'ctx>),
    /// Floating-point value.
    FloatValue(FloatValue<'ctx>),
    /// Integer value.
    IntValue(IntValue<'ctx>),
    /// Pointer value.
    PointerValue(PointerValue<'ctx>),
    /// Struct value.
    StructValue(StructValue<'ctx>),
    /// Fixed-width vector value.
    VectorValue(VectorValue<'ctx>),
    /// Scalable-vector value.
    ScalableVectorValue(ScalableVectorValue<'ctx>),
}

impl<'ctx> BasicValueEnum<'ctx> {
    pub(super) fn new(raw: LLVMValueRef) -> LlvmResult<Self> {
        // `LLVMTypeOf` does not accept null. Keep null-result handling at this
        // shared conversion boundary so every builder returning an enum value
        // reports an error instead of invoking LLVM with an invalid handle.
        if raw.is_null() {
            return Err(LlvmError::error("LLVM returned a null basic value"));
        }
        match BasicTypeEnum::new(unsafe { LLVMTypeOf(raw) })? {
            BasicTypeEnum::ArrayType(_) => Ok(Self::ArrayValue(ArrayValue::new(raw))),
            BasicTypeEnum::FloatType(_) => Ok(Self::FloatValue(FloatValue::new(raw))),
            BasicTypeEnum::IntType(_) => Ok(Self::IntValue(IntValue::new(raw))),
            BasicTypeEnum::PointerType(_) => Ok(Self::PointerValue(PointerValue::new(raw))),
            BasicTypeEnum::StructType(_) => Ok(Self::StructValue(StructValue::new(raw))),
            BasicTypeEnum::VectorType(_) => Ok(Self::VectorValue(VectorValue::new(raw))),
            BasicTypeEnum::ScalableVectorType(_) => {
                Ok(Self::ScalableVectorValue(ScalableVectorValue::new(raw)))
            }
        }
    }

    /// Returns the value's typed LLVM type.
    pub fn get_type(self) -> LlvmResult<BasicTypeEnum<'ctx>> {
        BasicTypeEnum::new(unsafe { LLVMTypeOf(self.as_value_ref()) })
    }

    /// Reports whether this is an integer value.
    pub fn is_int_value(self) -> bool {
        matches!(self, Self::IntValue(_))
    }

    /// Reports whether this is a floating-point value.
    pub fn is_float_value(self) -> bool {
        matches!(self, Self::FloatValue(_))
    }

    /// Reports whether this is a pointer value.
    pub fn is_pointer_value(self) -> bool {
        matches!(self, Self::PointerValue(_))
    }

    /// Reports whether this is a struct value.
    pub fn is_struct_value(self) -> bool {
        matches!(self, Self::StructValue(_))
    }

    /// Reports whether this is a fixed or scalable vector value.
    pub fn is_vector_value(self) -> bool {
        matches!(self, Self::VectorValue(_) | Self::ScalableVectorValue(_))
    }

    /// Extracts an array value or reports an invariant failure.
    pub fn into_array_value(self) -> LlvmResult<ArrayValue<'ctx>> {
        match self {
            Self::ArrayValue(value) => Ok(value),
            _ => Err(LlvmError::ice("expected array value")),
        }
    }

    /// Extracts a fixed-width vector value or reports an invariant failure.
    pub fn into_vector_value(self) -> LlvmResult<VectorValue<'ctx>> {
        match self {
            Self::VectorValue(value) => Ok(value),
            _ => Err(LlvmError::ice("expected vector value")),
        }
    }

    /// Extracts a floating-point value or reports an invariant failure.
    pub fn into_float_value(self) -> LlvmResult<FloatValue<'ctx>> {
        match self {
            Self::FloatValue(value) => Ok(value),
            _ => Err(LlvmError::ice("expected float value")),
        }
    }

    /// Extracts an integer value or reports an invariant failure.
    pub fn into_int_value(self) -> LlvmResult<IntValue<'ctx>> {
        match self {
            Self::IntValue(value) => Ok(value),
            _ => Err(LlvmError::ice("expected int value")),
        }
    }

    /// Extracts a pointer value or reports an invariant failure.
    pub fn into_pointer_value(self) -> LlvmResult<PointerValue<'ctx>> {
        match self {
            Self::PointerValue(value) => Ok(value),
            _ => Err(LlvmError::ice("expected pointer value")),
        }
    }

    /// Extracts a struct value or reports an invariant failure.
    pub fn into_struct_value(self) -> LlvmResult<StructValue<'ctx>> {
        match self {
            Self::StructValue(value) => Ok(value),
            _ => Err(LlvmError::ice("expected struct value")),
        }
    }

    /// Views this value as an instruction when it was produced by one.
    pub fn as_instruction_value(self) -> Option<InstructionValue<'ctx>> {
        let value = unsafe { LLVMIsAInstruction(self.as_value_ref()) };
        if value.is_null() {
            None
        } else {
            Some(InstructionValue::new(value))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_null_basic_type_before_llvm_inspection() {
        let error = BasicTypeEnum::new(std::ptr::null_mut()).expect_err("null type");

        assert_eq!(
            error,
            LlvmError::Error("LLVM returned a null basic type".to_string())
        );
    }

    #[test]
    fn rejects_null_function_return_type_before_kind_inspection() {
        let error =
            classify_return_type::<'static>(std::ptr::null_mut()).expect_err("null return type");

        assert_eq!(
            error,
            LlvmError::Error("LLVM returned a null function return type".to_string())
        );
    }

    #[test]
    fn rejects_null_function_value_type_before_wrapper_construction() {
        let error =
            require_function_type(std::ptr::null_mut()).expect_err("null function value type");

        assert_eq!(
            error,
            LlvmError::Error("LLVM returned a null function type".to_string())
        );
    }

    #[test]
    fn rejects_null_basic_value_before_llvm_inspection() {
        let error = BasicValueEnum::new(std::ptr::null_mut()).expect_err("null value");

        assert_eq!(
            error,
            LlvmError::Error("LLVM returned a null basic value".to_string())
        );
    }

    #[test]
    fn rejects_null_constant_value_before_wrapper_construction() {
        let error =
            require_value(std::ptr::null_mut(), "constant array").expect_err("null constant value");

        assert_eq!(
            error,
            LlvmError::Error("LLVM returned a null constant array value".to_string())
        );
    }

    #[test]
    fn rejects_struct_constant_field_count_mismatch() {
        let context = Context::create().unwrap();
        let struct_ty = context
            .struct_type(&[context.i32_type().into()], false)
            .unwrap();

        let error = struct_ty
            .const_named_struct(&[])
            .expect_err("struct constant field count mismatch");

        assert!(matches!(
            error,
            LlvmError::Error(message) if message.contains("struct constant has 0 values, expected 1")
        ));
    }

    #[test]
    fn rejects_struct_constant_field_type_mismatch() {
        let context = Context::create().unwrap();
        let struct_ty = context
            .struct_type(&[context.i32_type().into()], false)
            .unwrap();

        let error = struct_ty
            .const_named_struct(&[context.i64_type().const_zero().unwrap().into()])
            .expect_err("struct constant field type mismatch");

        assert!(matches!(
            error,
            LlvmError::Error(message) if message.contains("struct constant field 0 type does not match")
        ));
    }

    #[test]
    fn rejects_constant_array_element_type_mismatch() {
        let context = Context::create().unwrap();

        let error = context
            .i32_type()
            .const_array(&[context.i64_type().const_zero().unwrap()])
            .expect_err("constant array element type mismatch");

        assert!(matches!(
            error,
            LlvmError::Error(message) if message.contains("constant array element 0 type does not match")
        ));
    }

    #[test]
    fn rejects_setting_body_on_literal_struct() {
        let context = Context::create().unwrap();
        let struct_ty = context.struct_type(&[], false).unwrap();

        let error = struct_ty
            .set_body(&[context.i32_type().into()], false)
            .expect_err("literal struct body assignment");

        assert!(matches!(
            error,
            LlvmError::Error(message) if message.contains("requires a named opaque struct")
        ));
    }

    #[test]
    fn rejects_setting_body_after_opaque_struct_is_defined() {
        let context = Context::create().unwrap();
        let struct_ty = context.opaque_struct_type("defined").unwrap();
        struct_ty
            .set_body(&[context.i32_type().into()], false)
            .unwrap();

        let error = struct_ty
            .set_body(&[context.i64_type().into()], false)
            .expect_err("redefining opaque struct body");

        assert!(matches!(
            error,
            LlvmError::Error(message) if message.contains("can only be assigned once")
        ));
    }

    #[test]
    fn reports_constant_vector_lane_count_as_api_error() {
        let context = Context::create().unwrap();
        let vector_ty = BasicTypeEnum::from(context.i32_type())
            .vector_type(2)
            .unwrap();

        let error = vector_ty
            .const_vector(&[context.i32_type().const_zero().unwrap().into()])
            .expect_err("constant vector lane count mismatch");

        assert!(matches!(
            error,
            LlvmError::Error(message) if message.contains("constant vector lane count does not match")
        ));
    }

    #[test]
    fn reports_constant_vector_lane_type_as_api_error() {
        let context = Context::create().unwrap();
        let vector_ty = BasicTypeEnum::from(context.i32_type())
            .vector_type(1)
            .unwrap();

        let error = vector_ty
            .const_vector(&[context.i64_type().const_zero().unwrap().into()])
            .expect_err("constant vector lane type mismatch");

        assert!(matches!(
            error,
            LlvmError::Error(message) if message.contains("constant vector lane 0 type does not match")
        ));
    }

    #[test]
    fn rejects_zero_lane_fixed_vector_types() {
        let context = Context::create().unwrap();

        let error = BasicTypeEnum::from(context.i32_type())
            .vector_type(0)
            .expect_err("zero-lane fixed vector type");

        assert_eq!(
            error,
            LlvmError::Error("LLVM fixed vector type requires at least one lane".to_string())
        );
    }

    #[test]
    fn accepts_zero_length_fixed_array_types() {
        let context = Context::create().unwrap();

        let array = BasicTypeEnum::from(context.i8_type())
            .array_type(0)
            .expect("zero-length LLVM arrays are valid");

        let _ = array.const_zero().unwrap();
    }

    #[test]
    fn shared_zero_constant_path_checks_aggregate_handles() {
        let context = Context::create().unwrap();
        let array = context.i8_type().array_type(0).unwrap();
        let struct_ty = context.struct_type(&[], false).unwrap();

        assert!(BasicTypeEnum::ArrayType(array).const_zero().is_ok());
        assert!(BasicTypeEnum::StructType(struct_ty).const_zero().is_ok());
        assert!(array.const_zero().is_ok());
        assert!(array.get_undef().is_ok());
        assert!(struct_ty.const_zero().is_ok());
        assert!(struct_ty.get_undef().is_ok());
    }

    #[test]
    fn typed_value_type_queries_use_checked_conversion() {
        let context = Context::create().unwrap();
        let struct_ty = context.struct_type(&[], false).unwrap();
        let array_ty = context.i8_type().array_type(0).unwrap();

        assert_eq!(
            context.i32_type().const_zero().unwrap().get_type().unwrap(),
            context.i32_type()
        );
        assert_eq!(
            context.f32_type().const_zero().unwrap().get_type().unwrap(),
            context.f32_type()
        );
        assert_eq!(
            context
                .ptr_type(AddressSpace(2))
                .const_null()
                .unwrap()
                .get_type()
                .unwrap(),
            context.ptr_type(AddressSpace(2))
        );
        assert_eq!(
            struct_ty.const_zero().unwrap().get_type().unwrap(),
            struct_ty
        );
        assert_eq!(array_ty.const_zero().unwrap().get_type().unwrap(), array_ty);
    }

    #[test]
    fn typed_scalar_undef_queries_use_checked_conversion() {
        let context = Context::create().unwrap();

        assert!(context.i32_type().get_undef().is_ok());
        assert!(context.f32_type().get_undef().is_ok());
        assert!(context.ptr_type(AddressSpace(3)).get_undef().is_ok());
    }

    #[test]
    fn typed_scalar_zero_queries_use_checked_conversion() {
        let context = Context::create().unwrap();

        assert!(context.i32_type().const_zero().is_ok());
        assert!(context.f32_type().const_zero().is_ok());
        assert!(context.ptr_type(AddressSpace(3)).const_null().is_ok());
    }

    #[test]
    fn typed_integer_to_pointer_constant_uses_checked_conversion() {
        let context = Context::create().unwrap();
        let value = context
            .ptr_type(AddressSpace(1))
            .const_int_to_ptr(context.i64_type().const_zero().unwrap());

        assert!(value.is_ok());
    }

    #[test]
    fn rejects_integer_constant_bitcast_width_mismatch() {
        let context = Context::create().unwrap();

        let error = context
            .i32_type()
            .const_zero()
            .unwrap()
            .const_bitcast(context.i64_type())
            .expect_err("integer bitcast width mismatch");

        assert_eq!(
            error,
            LlvmError::Error(
                "integer constant bitcast requires equal source and target widths".to_string()
            )
        );
    }

    #[test]
    fn rejects_pointer_constant_bitcast_address_space_mismatch() {
        let context = Context::create().unwrap();
        let source = context.ptr_type(AddressSpace(1)).const_null().unwrap();

        let error = source
            .const_bitcast(context.ptr_type(AddressSpace(0)))
            .expect_err("pointer bitcast address-space mismatch");

        assert_eq!(
            error,
            LlvmError::Error("pointer constant bitcast requires equal address spaces".to_string())
        );
    }

    #[test]
    fn rejects_function_parameter_count_width_overflow() {
        let count = u32::MAX as usize;
        assert_eq!(
            checked_u32_count(count, "LLVM function type has too many parameters").unwrap(),
            u32::MAX
        );
        if usize::BITS > u32::BITS {
            let error = checked_u32_count(count + 1, "LLVM function type has too many parameters")
                .expect_err("parameter count overflow");
            assert_eq!(
                error,
                LlvmError::Error("LLVM function type has too many parameters".to_string())
            );
        }
    }

    #[test]
    fn rejects_gep_index_count_width_overflow() {
        let count = u32::MAX as usize;
        assert_eq!(
            checked_u32_count(count, "LLVM GEP has too many indices").unwrap(),
            u32::MAX
        );
        if usize::BITS > u32::BITS {
            let error = checked_u32_count(count + 1, "LLVM GEP has too many indices")
                .expect_err("GEP index count overflow");
            assert_eq!(
                error,
                LlvmError::Error("LLVM GEP has too many indices".to_string())
            );
        }
    }

    #[test]
    fn rejects_null_call_result_type_before_kind_inspection() {
        let result =
            classify_call_site_result::<'static>(std::ptr::null_mut(), std::ptr::null_mut())
                .basic()
                .expect("null call result type should be an error");
        let error = result.expect_err("null call result type");

        assert_eq!(
            error,
            LlvmError::Error("LLVM returned a null call result type".to_string())
        );
    }

    #[test]
    fn rejects_null_attribute_before_attachment() {
        let error = Attribute::new(std::ptr::null_mut()).expect_err("null attribute");

        assert_eq!(
            error,
            LlvmError::Error("LLVM returned a null attribute".to_string())
        );
    }
}

impl<'ctx> AsValueRef for BasicValueEnum<'ctx> {
    fn as_value_ref(&self) -> LLVMValueRef {
        match self {
            Self::ArrayValue(value) => value.as_value_ref(),
            Self::FloatValue(value) => value.as_value_ref(),
            Self::IntValue(value) => value.as_value_ref(),
            Self::PointerValue(value) => value.as_value_ref(),
            Self::StructValue(value) => value.as_value_ref(),
            Self::VectorValue(value) => value.as_value_ref(),
            Self::ScalableVectorValue(value) => value.as_value_ref(),
        }
    }
}

impl<'ctx> BasicValue<'ctx> for BasicValueEnum<'ctx> {
    fn as_basic_value_enum(&self) -> BasicValueEnum<'ctx> {
        *self
    }
}

impl<'ctx> From<IntValue<'ctx>> for BasicValueEnum<'ctx> {
    fn from(value: IntValue<'ctx>) -> Self {
        Self::IntValue(value)
    }
}

impl<'ctx> From<FloatValue<'ctx>> for BasicValueEnum<'ctx> {
    fn from(value: FloatValue<'ctx>) -> Self {
        Self::FloatValue(value)
    }
}

impl<'ctx> From<PointerValue<'ctx>> for BasicValueEnum<'ctx> {
    fn from(value: PointerValue<'ctx>) -> Self {
        Self::PointerValue(value)
    }
}

impl<'ctx> From<StructValue<'ctx>> for BasicValueEnum<'ctx> {
    fn from(value: StructValue<'ctx>) -> Self {
        Self::StructValue(value)
    }
}

impl<'ctx> From<ArrayValue<'ctx>> for BasicValueEnum<'ctx> {
    fn from(value: ArrayValue<'ctx>) -> Self {
        Self::ArrayValue(value)
    }
}

impl<'ctx> From<VectorValue<'ctx>> for BasicValueEnum<'ctx> {
    fn from(value: VectorValue<'ctx>) -> Self {
        Self::VectorValue(value)
    }
}

impl<'ctx> From<ScalableVectorValue<'ctx>> for BasicValueEnum<'ctx> {
    fn from(value: ScalableVectorValue<'ctx>) -> Self {
        Self::ScalableVectorValue(value)
    }
}

/// LLVM metadata-argument value supported by this wrapper.
pub type BasicMetadataValueEnum<'ctx> = BasicValueEnum<'ctx>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Non-owning handle to an LLVM function value.
pub struct FunctionValue<'ctx> {
    pub(super) raw: LLVMValueRef,
    pub(super) _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> FunctionValue<'ctx> {
    pub(super) fn new(raw: LLVMValueRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    /// Returns parameter `index`, or `None` when it is outside the signature.
    pub fn get_nth_param(self, index: u32) -> Option<LlvmResult<BasicValueEnum<'ctx>>> {
        let count = unsafe { LLVMCountParams(self.raw) };
        if index >= count {
            None
        } else {
            Some(BasicValueEnum::new(unsafe {
                LLVMGetParam(self.raw, index)
            }))
        }
    }

    /// Returns the function's first basic block.
    pub fn get_first_basic_block(self) -> Option<BasicBlock<'ctx>> {
        let block = unsafe { LLVMGetFirstBasicBlock(self.raw) };
        if block.is_null() {
            None
        } else {
            Some(BasicBlock::new(block))
        }
    }

    /// Returns the function signature type.
    pub fn get_type(self) -> LlvmResult<FunctionType<'ctx>> {
        require_function_type(unsafe { LLVMGlobalGetValueType(self.raw) })
    }

    /// Attaches `attribute` at the selected function location.
    pub fn add_attribute(self, loc: AttributeLoc, attribute: Attribute) {
        let index = match loc {
            AttributeLoc::Function => LLVMAttributeFunctionIndex,
        };
        unsafe { LLVMAddAttributeAtIndex(self.raw, index, attribute.raw) };
    }

    /// Views the function through LLVM's global-value API.
    pub fn as_global_value(self) -> GlobalValue<'ctx> {
        GlobalValue::new(self.raw)
    }

    /// Returns the function name, using lossy UTF-8 conversion if necessary.
    pub fn name(self) -> String {
        let mut len = 0;
        let ptr = unsafe { LLVMGetValueName2(self.raw, &mut len) };
        if ptr.is_null() || len == 0 {
            return String::new();
        }
        let bytes = unsafe { slice::from_raw_parts(ptr as *const u8, len) };
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// Associates debug subprogram metadata with this function.
    pub fn set_subprogram(self, subprogram: DISubprogram<'ctx>) {
        unsafe { LLVMSetSubprogram(self.raw, subprogram.raw) };
    }
}

fn require_function_type<'ctx>(raw: LLVMTypeRef) -> LlvmResult<FunctionType<'ctx>> {
    if raw.is_null() {
        return Err(LlvmError::error("LLVM returned a null function type"));
    }
    Ok(FunctionType::new(raw))
}

impl<'ctx> AsValueRef for FunctionValue<'ctx> {
    fn as_value_ref(&self) -> LLVMValueRef {
        self.raw
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Non-owning handle to an LLVM global value.
pub struct GlobalValue<'ctx> {
    pub(super) raw: LLVMValueRef,
    pub(super) _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> GlobalValue<'ctx> {
    pub(super) fn new(raw: LLVMValueRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    /// Views this global as its pointer value.
    pub fn as_pointer_value(self) -> PointerValue<'ctx> {
        PointerValue::new(self.raw)
    }

    /// Sets the global's initializer.
    pub fn set_initializer<V: BasicValue<'ctx>>(self, value: &V) -> LlvmResult<()> {
        let global_type = BasicTypeEnum::new(unsafe { LLVMGlobalGetValueType(self.raw) })?;
        let value_type = BasicTypeEnum::new(unsafe { LLVMTypeOf(value.as_value_ref()) })?;
        if value_type != global_type {
            return Err(LlvmError::error(
                "global initializer type does not match the global type",
            ));
        }
        unsafe { LLVMSetInitializer(self.raw, value.as_value_ref()) };
        Ok(())
    }

    /// Marks whether writes to the global are forbidden after initialization.
    pub fn set_constant(self, constant: bool) {
        unsafe { LLVMSetGlobalConstant(self.raw, bool_to_llvm(constant)) };
    }

    /// Sets the global's linkage.
    pub fn set_linkage(self, linkage: Linkage) {
        unsafe { LLVMSetLinkage(self.raw, linkage.into()) };
    }

    /// Places the global in `section`, or clears the section when absent.
    pub fn set_section(self, section: Option<&str>) -> LlvmResult<()> {
        let section = section.unwrap_or("");
        let section = to_c_string(section)?;
        unsafe { LLVMSetSection(self.raw, section.as_ptr()) };
        Ok(())
    }

    /// Sets the global's required byte alignment.
    pub fn set_alignment(self, bytes: u32) -> LlvmResult<()> {
        validate_alignment(bytes)?;
        unsafe { LLVMSetAlignment(self.raw, bytes) };
        Ok(())
    }
}

impl<'ctx> AsValueRef for GlobalValue<'ctx> {
    fn as_value_ref(&self) -> LLVMValueRef {
        self.raw
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Non-owning LLVM basic-block handle.
pub struct BasicBlock<'ctx> {
    pub(super) raw: LLVMBasicBlockRef,
    pub(super) _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> BasicBlock<'ctx> {
    pub(super) fn new(raw: LLVMBasicBlockRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    /// Returns the function that owns this block.
    pub fn get_parent(self) -> Option<FunctionValue<'ctx>> {
        let value = unsafe { LLVMGetBasicBlockParent(self.raw) };
        if value.is_null() {
            None
        } else {
            Some(FunctionValue::new(value))
        }
    }

    /// Returns the block terminator when one has been emitted.
    pub fn get_terminator(self) -> Option<InstructionValue<'ctx>> {
        let value = unsafe { LLVMGetBasicBlockTerminator(self.raw) };
        if value.is_null() {
            None
        } else {
            Some(InstructionValue::new(value))
        }
    }

    /// Returns the first instruction in this block.
    pub fn get_first_instruction(self) -> Option<InstructionValue<'ctx>> {
        let value = unsafe { LLVMGetFirstInstruction(self.raw) };
        if value.is_null() {
            None
        } else {
            Some(InstructionValue::new(value))
        }
    }

    /// Returns the next block in the parent function's order.
    pub fn get_next_basic_block(self) -> Option<BasicBlock<'ctx>> {
        let value = unsafe { LLVMGetNextBasicBlock(self.raw) };
        if value.is_null() {
            None
        } else {
            Some(BasicBlock::new(value))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Non-owning handle to an LLVM instruction.
pub struct InstructionValue<'ctx> {
    pub(super) raw: LLVMValueRef,
    pub(super) _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> InstructionValue<'ctx> {
    pub(super) fn new(raw: LLVMValueRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    /// Sets the instruction's atomic ordering after validating opcode-specific
    /// direction and compare-exchange success/failure constraints.
    pub fn set_atomic_ordering(self, ordering: AtomicOrdering) -> LlvmResult<()> {
        let opcode = self.get_opcode();
        let failure = (opcode == LLVMOpcode::LLVMAtomicCmpXchg)
            .then(|| atomic_ordering_from_llvm(unsafe { LLVMGetCmpXchgFailureOrdering(self.raw) }));
        validate_instruction_ordering(opcode, ordering, failure)?;
        unsafe { LLVMSetOrdering(self.raw, ordering.into()) };
        Ok(())
    }

    /// Sets the instruction's required byte alignment.
    pub fn set_alignment(self, bytes: u32) -> LlvmResult<()> {
        validate_alignment(bytes)?;
        match self.get_opcode() {
            LLVMOpcode::LLVMAlloca
            | LLVMOpcode::LLVMLoad
            | LLVMOpcode::LLVMStore
            | LLVMOpcode::LLVMAtomicRMW
            | LLVMOpcode::LLVMAtomicCmpXchg => {}
            _ => {
                return Err(LlvmError::error(
                    "LLVM alignment requires a memory instruction",
                ));
            }
        }
        unsafe { LLVMSetAlignment(self.raw, bytes) };
        Ok(())
    }

    /// Marks a load, store, atomic RMW, or compare-exchange instruction
    /// volatile or non-volatile.
    pub fn set_volatile(self, is_volatile: bool) -> LlvmResult<()> {
        match self.get_opcode() {
            LLVMOpcode::LLVMLoad
            | LLVMOpcode::LLVMStore
            | LLVMOpcode::LLVMAtomicRMW
            | LLVMOpcode::LLVMAtomicCmpXchg => {}
            _ => {
                return Err(LlvmError::error(
                    "LLVM volatile flag requires a memory access instruction",
                ));
            }
        }
        unsafe { LLVMSetVolatile(self.raw, bool_to_llvm(is_volatile)) };
        Ok(())
    }

    /// Sets the weak flag on an atomic compare-and-exchange instruction.
    pub fn set_weak(self, is_weak: bool) -> LlvmResult<()> {
        if self.get_opcode() != LLVMOpcode::LLVMAtomicCmpXchg {
            return Err(LlvmError::error(
                "LLVM weak flag requires an atomic compare-exchange instruction",
            ));
        }
        unsafe { LLVMSetWeak(self.raw, bool_to_llvm(is_weak)) };
        Ok(())
    }

    /// Returns the next instruction in the owning block.
    pub fn get_next_instruction(self) -> Option<InstructionValue<'ctx>> {
        let value = unsafe { LLVMGetNextInstruction(self.raw) };
        if value.is_null() {
            None
        } else {
            Some(InstructionValue::new(value))
        }
    }

    /// Returns LLVM's opcode for this instruction.
    pub fn get_opcode(self) -> LLVMOpcode {
        unsafe { LLVMGetInstructionOpcode(self.raw) }
    }

    /// Returns the allocation type of an `alloca` instruction.
    pub fn get_allocated_type(self) -> LlvmResult<BasicTypeEnum<'ctx>> {
        BasicTypeEnum::new(unsafe { LLVMGetAllocatedType(self.raw) })
    }

    /// Returns the instruction name, using lossy UTF-8 conversion if needed.
    pub fn name(self) -> String {
        let mut len = 0;
        let ptr = unsafe { LLVMGetValueName2(self.raw, &mut len) };
        if ptr.is_null() || len == 0 {
            return String::new();
        }
        let bytes = unsafe { slice::from_raw_parts(ptr as *const u8, len) };
        String::from_utf8_lossy(bytes).into_owned()
    }
}

impl<'ctx> AsValueRef for InstructionValue<'ctx> {
    fn as_value_ref(&self) -> LLVMValueRef {
        self.raw
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Non-owning handle to an LLVM phi instruction.
pub struct PhiValue<'ctx> {
    pub(super) raw: LLVMValueRef,
    pub(super) _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> PhiValue<'ctx> {
    pub(super) fn new(raw: LLVMValueRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    /// Adds value/block pairs to this phi instruction.
    pub fn add_incoming(
        self,
        incoming: &[(&dyn BasicValue<'ctx>, BasicBlock<'ctx>)],
    ) -> LlvmResult<()> {
        let phi_block = unsafe { LLVMGetInstructionParent(self.raw) };
        if phi_block.is_null() {
            return Err(LlvmError::error("phi instruction has no parent block"));
        }
        let phi_block = BasicBlock::new(phi_block);
        let phi_function = phi_block
            .get_parent()
            .ok_or_else(|| LlvmError::error("phi block has no parent function"))?;
        let phi_type = BasicTypeEnum::new(unsafe { LLVMTypeOf(self.raw) })?;
        for (value, block) in incoming {
            let value_type = BasicTypeEnum::new(unsafe { LLVMTypeOf(value.as_value_ref()) })?;
            if value_type != phi_type {
                return Err(LlvmError::error(
                    "phi incoming value type does not match the phi type",
                ));
            }
            let block_function = block
                .get_parent()
                .ok_or_else(|| LlvmError::error("phi incoming block has no parent function"))?;
            if block_function != phi_function {
                return Err(LlvmError::error(
                    "phi incoming block must belong to the phi's function",
                ));
            }
        }
        let mut values = incoming
            .iter()
            .map(|(value, _)| value.as_value_ref())
            .collect::<Vec<_>>();
        let mut blocks = incoming
            .iter()
            .map(|(_, block)| block.raw)
            .collect::<Vec<_>>();
        let incoming_count = u32::try_from(incoming.len())
            .map_err(|_| LlvmError::error("LLVM phi has too many incoming values"))?;
        unsafe {
            LLVMAddIncoming(
                self.raw,
                values.as_mut_ptr(),
                blocks.as_mut_ptr(),
                incoming_count,
            )
        };
        Ok(())
    }

    /// Converts the phi instruction to its first-class result value.
    pub fn as_basic_value(self) -> LlvmResult<BasicValueEnum<'ctx>> {
        BasicValueEnum::new(self.raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Non-owning handle to an LLVM call instruction.
pub struct CallSiteValue<'ctx> {
    pub(super) raw: LLVMValueRef,
    pub(super) _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> CallSiteValue<'ctx> {
    pub(super) fn new(raw: LLVMValueRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    /// Classifies this call as void or as a fallible first-class value.
    pub fn try_as_basic_value(self) -> CallSiteTryAsValue<'ctx> {
        let ty = unsafe { LLVMTypeOf(self.raw) };
        classify_call_site_result(self.raw, ty)
    }
}

/// Deferred classification of a call as void or first-class value.
///
/// LLVM represents both forms with a call instruction. This wrapper preserves
/// the distinction while allowing malformed non-void results to carry an
/// [`LlvmError`] instead of panicking during typed conversion.
pub struct CallSiteTryAsValue<'ctx> {
    value: Option<LlvmResult<BasicValueEnum<'ctx>>>,
}

impl<'ctx> CallSiteTryAsValue<'ctx> {
    /// Returns `None` for void calls or the typed non-void result.
    pub fn basic(self) -> Option<LlvmResult<BasicValueEnum<'ctx>>> {
        self.value
    }

    /// Requires a non-void call result, reporting an ICE for void calls.
    pub fn unwrap_basic(self) -> LlvmResult<BasicValueEnum<'ctx>> {
        self.value
            .ok_or_else(|| LlvmError::ice("expected non-void call result"))?
    }
}

/// Classifies a call result only after confirming that LLVM returned a type.
/// A non-void result is converted through [`BasicValueEnum::new`], which adds
/// the corresponding value/type checks for the typed wrapper.
fn classify_call_site_result<'ctx>(
    value: LLVMValueRef,
    ty: LLVMTypeRef,
) -> CallSiteTryAsValue<'ctx> {
    let value = if ty.is_null() {
        Some(Err(LlvmError::error(
            "LLVM returned a null call result type",
        )))
    } else if unsafe { LLVMGetTypeKind(ty) } == LLVMTypeKind::LLVMVoidTypeKind {
        None
    } else {
        Some(BasicValueEnum::new(value))
    };
    CallSiteTryAsValue { value }
}
