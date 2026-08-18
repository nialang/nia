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
    LLVMConstVector, LLVMCountParams, LLVMCountStructElementTypes, LLVMFunctionType,
    LLVMGetAllocatedType, LLVMGetBasicBlockParent, LLVMGetBasicBlockTerminator, LLVMGetElementType,
    LLVMGetEnumAttributeKindForName, LLVMGetFirstBasicBlock, LLVMGetFirstInstruction,
    LLVMGetInstructionOpcode, LLVMGetIntTypeWidth, LLVMGetNextBasicBlock, LLVMGetNextInstruction,
    LLVMGetParam, LLVMGetReturnType, LLVMGetTypeKind, LLVMGetUndef, LLVMGetValueName2,
    LLVMGetVectorSize, LLVMGlobalGetValueType, LLVMIsAInstruction, LLVMIsPackedStruct,
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
pub enum LlvmError {
    Error(String),
    Ice(nia_ice::Ice),
}

pub type LlvmResult<T> = Result<T, LlvmError>;

impl LlvmError {
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error(message.into())
    }

    pub fn ice(message: impl Into<String>) -> Self {
        Self::Ice(nia_ice::Ice::new(format!("LLVM: {}", message.into())))
    }

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

pub trait AsTypeRef {
    fn as_type_ref(&self) -> LLVMTypeRef;
}

pub trait BasicType<'ctx>: AsTypeRef + Copy {}

pub trait AsValueRef {
    fn as_value_ref(&self) -> LLVMValueRef;
}

pub trait BasicValue<'ctx>: AsValueRef {
    fn as_basic_value_enum(&self) -> BasicValueEnum<'ctx>;
}

pub trait AggregateValue<'ctx>: BasicValue<'ctx> {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct AddressSpace(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineAsmDialect {
    ATT,
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
pub enum IntPredicate {
    EQ,
    NE,
    UGT,
    UGE,
    ULT,
    ULE,
    SGT,
    SGE,
    SLT,
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
pub enum FloatPredicate {
    OEQ,
    OGT,
    OGE,
    OLT,
    OLE,
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
pub enum AtomicOrdering {
    NotAtomic,
    Unordered,
    Monotonic,
    Acquire,
    Release,
    AcquireRelease,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicRMWBinOp {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizationLevel {
    None,
    Less,
    Default,
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
pub enum Linkage {
    External,
    Appending,
    LinkOnceOdr,
    WeakOdr,
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
pub enum AttributeLoc {
    Function,
}

#[derive(Debug, Clone, Copy)]
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

    pub fn get_named_enum_kind_id(name: &str) -> u32 {
        unsafe { LLVMGetEnumAttributeKindForName(name.as_ptr() as *const _, name.len()) }
    }
}

macro_rules! impl_type_wrapper {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

impl_type_wrapper!(VoidType);
impl_type_wrapper!(IntType);
impl_type_wrapper!(FloatType);
impl_type_wrapper!(PointerType);
impl_type_wrapper!(StructType);
impl_type_wrapper!(ArrayType);
impl_type_wrapper!(VectorType);
impl_type_wrapper!(ScalableVectorType);
impl_type_wrapper!(FunctionType);

impl<'ctx> VoidType<'ctx> {
    pub fn fn_type(
        self,
        params: &[BasicMetadataTypeEnum<'ctx>],
        variadic: bool,
    ) -> FunctionType<'ctx> {
        let mut params = params
            .iter()
            .map(|param| param.as_type_ref())
            .collect::<Vec<_>>();
        FunctionType::new(unsafe {
            LLVMFunctionType(
                self.as_type_ref(),
                params.as_mut_ptr(),
                params.len() as u32,
                bool_to_llvm(variadic),
            )
        })
    }
}

macro_rules! impl_basic_type_methods {
    ($name:ident) => {
        impl<'ctx> $name<'ctx> {
            pub fn fn_type(
                self,
                params: &[BasicMetadataTypeEnum<'ctx>],
                variadic: bool,
            ) -> FunctionType<'ctx> {
                let mut params = params
                    .iter()
                    .map(|param| param.as_type_ref())
                    .collect::<Vec<_>>();
                FunctionType::new(unsafe {
                    LLVMFunctionType(
                        self.as_type_ref(),
                        params.as_mut_ptr(),
                        params.len() as u32,
                        bool_to_llvm(variadic),
                    )
                })
            }

            pub fn array_type(self, len: u32) -> ArrayType<'ctx> {
                ArrayType::new(unsafe { LLVMArrayType2(self.as_type_ref(), len as u64) })
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
    pub fn bit_width(self) -> u32 {
        unsafe { LLVMGetIntTypeWidth(self.as_type_ref()) }
    }

    pub fn const_int(self, value: u64, sign_extend: bool) -> IntValue<'ctx> {
        IntValue::new(unsafe { LLVMConstInt(self.as_type_ref(), value, bool_to_llvm(sign_extend)) })
    }

    pub fn const_u128(self, value: u128) -> IntValue<'ctx> {
        if self.bit_width() <= 64 {
            return self.const_int(value as u64, false);
        }

        let words = [value as u64, (value >> 64) as u64];
        IntValue::new(unsafe {
            LLVMConstIntOfArbitraryPrecision(self.as_type_ref(), words.len() as u32, words.as_ptr())
        })
    }

    pub fn const_array(self, values: &[IntValue<'ctx>]) -> ArrayValue<'ctx> {
        let mut values = values
            .iter()
            .map(|value| value.as_value_ref())
            .collect::<Vec<_>>();
        ArrayValue::new(unsafe {
            LLVMConstArray2(self.as_type_ref(), values.as_mut_ptr(), values.len() as u64)
        })
    }

    pub fn const_zero(self) -> IntValue<'ctx> {
        self.const_int(0, false)
    }

    pub fn get_undef(self) -> IntValue<'ctx> {
        IntValue::new(unsafe { LLVMGetUndef(self.as_type_ref()) })
    }
}

impl<'ctx> IntValue<'ctx> {
    pub fn const_bitcast(self, target_ty: IntType<'ctx>) -> IntValue<'ctx> {
        IntValue::new(unsafe { LLVMConstBitCast(self.as_value_ref(), target_ty.as_type_ref()) })
    }
}

impl<'ctx> FloatType<'ctx> {
    pub fn const_float(self, value: f64) -> FloatValue<'ctx> {
        FloatValue::new(unsafe { LLVMConstReal(self.as_type_ref(), value) })
    }

    pub fn const_array(self, values: &[FloatValue<'ctx>]) -> ArrayValue<'ctx> {
        let mut values = values
            .iter()
            .map(|value| value.as_value_ref())
            .collect::<Vec<_>>();
        ArrayValue::new(unsafe {
            LLVMConstArray2(self.as_type_ref(), values.as_mut_ptr(), values.len() as u64)
        })
    }

    pub fn const_zero(self) -> FloatValue<'ctx> {
        self.const_float(0.0)
    }

    pub fn get_undef(self) -> FloatValue<'ctx> {
        FloatValue::new(unsafe { LLVMGetUndef(self.as_type_ref()) })
    }
}

impl<'ctx> PointerType<'ctx> {
    pub fn const_zero(self) -> PointerValue<'ctx> {
        self.const_null()
    }

    pub fn const_null(self) -> PointerValue<'ctx> {
        PointerValue::new(unsafe { LLVMConstPointerNull(self.as_type_ref()) })
    }

    pub fn const_int_to_ptr(self, value: IntValue<'ctx>) -> PointerValue<'ctx> {
        PointerValue::new(unsafe { LLVMConstIntToPtr(value.as_value_ref(), self.as_type_ref()) })
    }

    pub fn get_undef(self) -> PointerValue<'ctx> {
        PointerValue::new(unsafe { LLVMGetUndef(self.as_type_ref()) })
    }

    pub fn const_array(self, values: &[PointerValue<'ctx>]) -> ArrayValue<'ctx> {
        let mut values = values
            .iter()
            .map(|value| value.as_value_ref())
            .collect::<Vec<_>>();
        ArrayValue::new(unsafe {
            LLVMConstArray2(self.as_type_ref(), values.as_mut_ptr(), values.len() as u64)
        })
    }
}

impl<'ctx> StructType<'ctx> {
    pub fn as_basic_type_enum(self) -> BasicTypeEnum<'ctx> {
        BasicTypeEnum::StructType(self)
    }

    pub fn set_body(self, fields: &[BasicTypeEnum<'ctx>], packed: bool) {
        let mut fields = fields
            .iter()
            .map(|field| field.as_type_ref())
            .collect::<Vec<_>>();
        unsafe {
            LLVMStructSetBody(
                self.as_type_ref(),
                fields.as_mut_ptr(),
                fields.len() as u32,
                bool_to_llvm(packed),
            )
        };
    }

    pub fn count_fields(self) -> u32 {
        unsafe { LLVMCountStructElementTypes(self.as_type_ref()) }
    }

    pub fn is_packed(self) -> bool {
        unsafe { LLVMIsPackedStruct(self.as_type_ref()) != 0 }
    }

    pub fn get_field_type_at_index(self, index: u32) -> Option<LlvmResult<BasicTypeEnum<'ctx>>> {
        if index >= self.count_fields() {
            None
        } else {
            Some(BasicTypeEnum::new(unsafe {
                LLVMStructGetTypeAtIndex(self.as_type_ref(), index)
            }))
        }
    }

    pub fn const_named_struct(self, values: &[BasicValueEnum<'ctx>]) -> StructValue<'ctx> {
        let mut values = values
            .iter()
            .map(|value| value.as_value_ref())
            .collect::<Vec<_>>();
        StructValue::new(unsafe {
            LLVMConstNamedStruct(self.as_type_ref(), values.as_mut_ptr(), values.len() as u32)
        })
    }

    pub fn const_zero(self) -> StructValue<'ctx> {
        StructValue::new(unsafe { LLVMConstNull(self.as_type_ref()) })
    }

    pub fn get_undef(self) -> StructValue<'ctx> {
        StructValue::new(unsafe { LLVMGetUndef(self.as_type_ref()) })
    }

    pub fn const_array(self, values: &[StructValue<'ctx>]) -> ArrayValue<'ctx> {
        let mut values = values
            .iter()
            .map(|value| value.as_value_ref())
            .collect::<Vec<_>>();
        ArrayValue::new(unsafe {
            LLVMConstArray2(self.as_type_ref(), values.as_mut_ptr(), values.len() as u64)
        })
    }
}

impl<'ctx> ArrayType<'ctx> {
    pub fn len(self) -> u32 {
        unsafe { llvm_sys::core::LLVMGetArrayLength2(self.as_type_ref()) as u32 }
    }

    pub fn get_element_type(self) -> LlvmResult<BasicTypeEnum<'ctx>> {
        BasicTypeEnum::new(unsafe { LLVMGetElementType(self.as_type_ref()) })
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub fn const_zero(self) -> ArrayValue<'ctx> {
        ArrayValue::new(unsafe { LLVMConstNull(self.as_type_ref()) })
    }

    pub fn get_undef(self) -> ArrayValue<'ctx> {
        ArrayValue::new(unsafe { LLVMGetUndef(self.as_type_ref()) })
    }

    pub fn const_array(self, values: &[ArrayValue<'ctx>]) -> ArrayValue<'ctx> {
        let elem_ty = unsafe { LLVMGetElementType(self.as_type_ref()) };
        let mut values = values
            .iter()
            .map(|value| value.as_value_ref())
            .collect::<Vec<_>>();
        ArrayValue::new(unsafe {
            LLVMConstArray2(elem_ty, values.as_mut_ptr(), values.len() as u64)
        })
    }
}

impl<'ctx> VectorType<'ctx> {
    pub fn len(self) -> u32 {
        unsafe { LLVMGetVectorSize(self.as_type_ref()) }
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub fn get_element_type(self) -> LlvmResult<BasicTypeEnum<'ctx>> {
        BasicTypeEnum::new(unsafe { LLVMGetElementType(self.as_type_ref()) })
    }

    pub fn const_vector(self, values: &[BasicValueEnum<'ctx>]) -> LlvmResult<VectorValue<'ctx>> {
        if values.len() != self.len() as usize {
            return Err(LlvmError::ice(
                "constant vector lane count does not match type",
            ));
        }
        let elem_ty = self.get_element_type()?;
        if values
            .iter()
            .any(|value| !value.get_type().is_ok_and(|ty| ty == elem_ty))
        {
            return Err(LlvmError::ice(
                "constant vector lane type does not match type",
            ));
        }
        let mut values = values
            .iter()
            .map(|value| value.as_value_ref())
            .collect::<Vec<_>>();
        Ok(VectorValue::new(unsafe {
            LLVMConstVector(values.as_mut_ptr(), values.len() as u32)
        }))
    }

    pub fn const_zero(self) -> LlvmResult<BasicValueEnum<'ctx>> {
        BasicValueEnum::new(unsafe { LLVMConstNull(self.as_type_ref()) })
    }

    pub fn get_undef(self) -> LlvmResult<BasicValueEnum<'ctx>> {
        BasicValueEnum::new(unsafe { LLVMGetUndef(self.as_type_ref()) })
    }
}

impl<'ctx> ScalableVectorType<'ctx> {
    pub fn const_zero(self) -> LlvmResult<BasicValueEnum<'ctx>> {
        BasicValueEnum::new(unsafe { LLVMConstNull(self.as_type_ref()) })
    }

    pub fn get_undef(self) -> LlvmResult<BasicValueEnum<'ctx>> {
        BasicValueEnum::new(unsafe { LLVMGetUndef(self.as_type_ref()) })
    }
}

impl<'ctx> FunctionType<'ctx> {
    pub fn get_return_type(self) -> LlvmResult<Option<BasicTypeEnum<'ctx>>> {
        classify_return_type(unsafe { LLVMGetReturnType(self.as_type_ref()) })
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
pub enum BasicTypeEnum<'ctx> {
    ArrayType(ArrayType<'ctx>),
    FloatType(FloatType<'ctx>),
    IntType(IntType<'ctx>),
    PointerType(PointerType<'ctx>),
    StructType(StructType<'ctx>),
    VectorType(VectorType<'ctx>),
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

    pub fn const_zero(self) -> LlvmResult<BasicValueEnum<'ctx>> {
        Ok(match self {
            Self::ArrayType(t) => t.const_zero().into(),
            Self::FloatType(t) => t.const_zero().into(),
            Self::IntType(t) => t.const_zero().into(),
            Self::PointerType(t) => t.const_zero().into(),
            Self::StructType(t) => t.const_zero().into(),
            Self::VectorType(t) => t.const_zero()?,
            Self::ScalableVectorType(t) => t.const_zero()?,
        })
    }

    pub fn array_type(self, len: u32) -> ArrayType<'ctx> {
        ArrayType::new(unsafe { LLVMArrayType2(self.as_type_ref(), len as u64) })
    }

    pub fn fn_type(
        self,
        params: &[BasicMetadataTypeEnum<'ctx>],
        variadic: bool,
    ) -> FunctionType<'ctx> {
        let mut params = params
            .iter()
            .map(|param| param.as_type_ref())
            .collect::<Vec<_>>();
        FunctionType::new(unsafe {
            LLVMFunctionType(
                self.as_type_ref(),
                params.as_mut_ptr(),
                params.len() as u32,
                bool_to_llvm(variadic),
            )
        })
    }

    pub fn vector_type(self, len: u32) -> VectorType<'ctx> {
        VectorType::new(unsafe { LLVMVectorType(self.as_type_ref(), len) })
    }

    pub fn is_pointer_type(self) -> bool {
        matches!(self, Self::PointerType(_))
    }

    pub fn into_array_type(self) -> LlvmResult<ArrayType<'ctx>> {
        match self {
            Self::ArrayType(value) => Ok(value),
            _ => Err(LlvmError::ice("expected array type")),
        }
    }

    pub fn into_float_type(self) -> LlvmResult<FloatType<'ctx>> {
        match self {
            Self::FloatType(value) => Ok(value),
            _ => Err(LlvmError::ice("expected float type")),
        }
    }

    pub fn into_int_type(self) -> LlvmResult<IntType<'ctx>> {
        match self {
            Self::IntType(value) => Ok(value),
            _ => Err(LlvmError::ice("expected int type")),
        }
    }

    pub fn into_pointer_type(self) -> LlvmResult<PointerType<'ctx>> {
        match self {
            Self::PointerType(value) => Ok(value),
            _ => Err(LlvmError::ice("expected pointer type")),
        }
    }

    pub fn into_struct_type(self) -> LlvmResult<StructType<'ctx>> {
        match self {
            Self::StructType(value) => Ok(value),
            _ => Err(LlvmError::ice("expected struct type")),
        }
    }

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

pub type BasicMetadataTypeEnum<'ctx> = BasicTypeEnum<'ctx>;

macro_rules! impl_value_wrapper {
    ($name:ident, $basic_method:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

impl_value_wrapper!(IntValue, IntValue);
impl_value_wrapper!(FloatValue, FloatValue);
impl_value_wrapper!(PointerValue, PointerValue);
impl_value_wrapper!(StructValue, StructValue);
impl_value_wrapper!(ArrayValue, ArrayValue);
impl_value_wrapper!(VectorValue, VectorValue);
impl_value_wrapper!(ScalableVectorValue, ScalableVectorValue);

impl<'ctx> AggregateValue<'ctx> for StructValue<'ctx> {}
impl<'ctx> AggregateValue<'ctx> for ArrayValue<'ctx> {}

impl<'ctx> IntValue<'ctx> {
    pub fn get_type(self) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMTypeOf(self.raw) })
    }
}

impl<'ctx> FloatValue<'ctx> {
    pub fn get_type(self) -> FloatType<'ctx> {
        FloatType::new(unsafe { LLVMTypeOf(self.raw) })
    }
}

impl<'ctx> PointerValue<'ctx> {
    pub fn get_type(self) -> PointerType<'ctx> {
        PointerType::new(unsafe { LLVMTypeOf(self.raw) })
    }

    pub fn const_bitcast(self, target_ty: PointerType<'ctx>) -> PointerValue<'ctx> {
        PointerValue::new(unsafe { LLVMConstBitCast(self.as_value_ref(), target_ty.as_type_ref()) })
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
    ) -> PointerValue<'ctx> {
        let mut indices = indices
            .iter()
            .map(|index| index.as_value_ref())
            .collect::<Vec<_>>();
        PointerValue::new(unsafe {
            LLVMConstGEP2(
                pointee_ty.as_type_ref(),
                self.as_value_ref(),
                indices.as_mut_ptr(),
                indices.len() as u32,
            )
        })
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
    ) -> PointerValue<'ctx> {
        let mut indices = indices
            .iter()
            .map(|index| index.as_value_ref())
            .collect::<Vec<_>>();
        PointerValue::new(unsafe {
            LLVMConstInBoundsGEP2(
                pointee_ty.as_type_ref(),
                self.as_value_ref(),
                indices.as_mut_ptr(),
                indices.len() as u32,
            )
        })
    }
}

impl<'ctx> StructValue<'ctx> {
    pub fn get_type(self) -> StructType<'ctx> {
        StructType::new(unsafe { LLVMTypeOf(self.raw) })
    }

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
    pub fn get_type(self) -> ArrayType<'ctx> {
        ArrayType::new(unsafe { LLVMTypeOf(self.raw) })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BasicValueEnum<'ctx> {
    ArrayValue(ArrayValue<'ctx>),
    FloatValue(FloatValue<'ctx>),
    IntValue(IntValue<'ctx>),
    PointerValue(PointerValue<'ctx>),
    StructValue(StructValue<'ctx>),
    VectorValue(VectorValue<'ctx>),
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

    pub fn get_type(self) -> LlvmResult<BasicTypeEnum<'ctx>> {
        BasicTypeEnum::new(unsafe { LLVMTypeOf(self.as_value_ref()) })
    }

    pub fn is_int_value(self) -> bool {
        matches!(self, Self::IntValue(_))
    }

    pub fn is_float_value(self) -> bool {
        matches!(self, Self::FloatValue(_))
    }

    pub fn is_pointer_value(self) -> bool {
        matches!(self, Self::PointerValue(_))
    }

    pub fn is_struct_value(self) -> bool {
        matches!(self, Self::StructValue(_))
    }

    pub fn is_vector_value(self) -> bool {
        matches!(self, Self::VectorValue(_) | Self::ScalableVectorValue(_))
    }

    pub fn into_array_value(self) -> LlvmResult<ArrayValue<'ctx>> {
        match self {
            Self::ArrayValue(value) => Ok(value),
            _ => Err(LlvmError::ice("expected array value")),
        }
    }

    pub fn into_vector_value(self) -> LlvmResult<VectorValue<'ctx>> {
        match self {
            Self::VectorValue(value) => Ok(value),
            _ => Err(LlvmError::ice("expected vector value")),
        }
    }

    pub fn into_float_value(self) -> LlvmResult<FloatValue<'ctx>> {
        match self {
            Self::FloatValue(value) => Ok(value),
            _ => Err(LlvmError::ice("expected float value")),
        }
    }

    pub fn into_int_value(self) -> LlvmResult<IntValue<'ctx>> {
        match self {
            Self::IntValue(value) => Ok(value),
            _ => Err(LlvmError::ice("expected int value")),
        }
    }

    pub fn into_pointer_value(self) -> LlvmResult<PointerValue<'ctx>> {
        match self {
            Self::PointerValue(value) => Ok(value),
            _ => Err(LlvmError::ice("expected pointer value")),
        }
    }

    pub fn into_struct_value(self) -> LlvmResult<StructValue<'ctx>> {
        match self {
            Self::StructValue(value) => Ok(value),
            _ => Err(LlvmError::ice("expected struct value")),
        }
    }

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
    fn rejects_null_basic_value_before_llvm_inspection() {
        let error = BasicValueEnum::new(std::ptr::null_mut()).expect_err("null value");

        assert_eq!(
            error,
            LlvmError::Error("LLVM returned a null basic value".to_string())
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

pub type BasicMetadataValueEnum<'ctx> = BasicValueEnum<'ctx>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

    pub fn get_first_basic_block(self) -> Option<BasicBlock<'ctx>> {
        let block = unsafe { LLVMGetFirstBasicBlock(self.raw) };
        if block.is_null() {
            None
        } else {
            Some(BasicBlock::new(block))
        }
    }

    pub fn get_type(self) -> FunctionType<'ctx> {
        FunctionType::new(unsafe { LLVMGlobalGetValueType(self.raw) })
    }

    pub fn add_attribute(self, loc: AttributeLoc, attribute: Attribute) {
        let index = match loc {
            AttributeLoc::Function => LLVMAttributeFunctionIndex,
        };
        unsafe { LLVMAddAttributeAtIndex(self.raw, index, attribute.raw) };
    }

    pub fn as_global_value(self) -> GlobalValue<'ctx> {
        GlobalValue::new(self.raw)
    }

    pub fn name(self) -> String {
        let mut len = 0;
        let ptr = unsafe { LLVMGetValueName2(self.raw, &mut len) };
        if ptr.is_null() || len == 0 {
            return String::new();
        }
        let bytes = unsafe { slice::from_raw_parts(ptr as *const u8, len) };
        String::from_utf8_lossy(bytes).into_owned()
    }

    pub fn set_subprogram(self, subprogram: DISubprogram<'ctx>) {
        unsafe { LLVMSetSubprogram(self.raw, subprogram.raw) };
    }
}

impl<'ctx> AsValueRef for FunctionValue<'ctx> {
    fn as_value_ref(&self) -> LLVMValueRef {
        self.raw
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

    pub fn as_pointer_value(self) -> PointerValue<'ctx> {
        PointerValue::new(self.raw)
    }

    pub fn set_initializer<V: BasicValue<'ctx>>(self, value: &V) {
        unsafe { LLVMSetInitializer(self.raw, value.as_value_ref()) };
    }

    pub fn set_constant(self, constant: bool) {
        unsafe { LLVMSetGlobalConstant(self.raw, bool_to_llvm(constant)) };
    }

    pub fn set_linkage(self, linkage: Linkage) {
        unsafe { LLVMSetLinkage(self.raw, linkage.into()) };
    }

    pub fn set_section(self, section: Option<&str>) -> LlvmResult<()> {
        let section = section.unwrap_or("");
        let section = to_c_string(section)?;
        unsafe { LLVMSetSection(self.raw, section.as_ptr()) };
        Ok(())
    }

    pub fn set_alignment(self, bytes: u32) {
        unsafe { LLVMSetAlignment(self.raw, bytes) };
    }
}

impl<'ctx> AsValueRef for GlobalValue<'ctx> {
    fn as_value_ref(&self) -> LLVMValueRef {
        self.raw
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

    pub fn get_parent(self) -> Option<FunctionValue<'ctx>> {
        let value = unsafe { LLVMGetBasicBlockParent(self.raw) };
        if value.is_null() {
            None
        } else {
            Some(FunctionValue::new(value))
        }
    }

    pub fn get_terminator(self) -> Option<InstructionValue<'ctx>> {
        let value = unsafe { LLVMGetBasicBlockTerminator(self.raw) };
        if value.is_null() {
            None
        } else {
            Some(InstructionValue::new(value))
        }
    }

    pub fn get_first_instruction(self) -> Option<InstructionValue<'ctx>> {
        let value = unsafe { LLVMGetFirstInstruction(self.raw) };
        if value.is_null() {
            None
        } else {
            Some(InstructionValue::new(value))
        }
    }

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

    pub fn set_atomic_ordering(self, ordering: AtomicOrdering) {
        unsafe { LLVMSetOrdering(self.raw, ordering.into()) };
    }

    pub fn set_alignment(self, bytes: u32) {
        unsafe { LLVMSetAlignment(self.raw, bytes) };
    }

    pub fn set_volatile(self, is_volatile: bool) {
        unsafe { LLVMSetVolatile(self.raw, bool_to_llvm(is_volatile)) };
    }

    pub fn set_weak(self, is_weak: bool) {
        unsafe { LLVMSetWeak(self.raw, bool_to_llvm(is_weak)) };
    }

    pub fn get_next_instruction(self) -> Option<InstructionValue<'ctx>> {
        let value = unsafe { LLVMGetNextInstruction(self.raw) };
        if value.is_null() {
            None
        } else {
            Some(InstructionValue::new(value))
        }
    }

    pub fn get_opcode(self) -> LLVMOpcode {
        unsafe { LLVMGetInstructionOpcode(self.raw) }
    }

    pub fn get_allocated_type(self) -> LlvmResult<BasicTypeEnum<'ctx>> {
        BasicTypeEnum::new(unsafe { LLVMGetAllocatedType(self.raw) })
    }

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

    pub fn add_incoming(self, incoming: &[(&dyn BasicValue<'ctx>, BasicBlock<'ctx>)]) {
        let mut values = incoming
            .iter()
            .map(|(value, _)| value.as_value_ref())
            .collect::<Vec<_>>();
        let mut blocks = incoming
            .iter()
            .map(|(_, block)| block.raw)
            .collect::<Vec<_>>();
        unsafe {
            LLVMAddIncoming(
                self.raw,
                values.as_mut_ptr(),
                blocks.as_mut_ptr(),
                incoming.len() as u32,
            )
        };
    }

    pub fn as_basic_value(self) -> LlvmResult<BasicValueEnum<'ctx>> {
        BasicValueEnum::new(self.raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

    pub fn try_as_basic_value(self) -> CallSiteTryAsValue<'ctx> {
        let ty = unsafe { LLVMTypeOf(self.raw) };
        let value = if unsafe { LLVMGetTypeKind(ty) } == LLVMTypeKind::LLVMVoidTypeKind {
            None
        } else {
            Some(BasicValueEnum::new(self.raw))
        };
        CallSiteTryAsValue { value }
    }
}

pub struct CallSiteTryAsValue<'ctx> {
    value: Option<LlvmResult<BasicValueEnum<'ctx>>>,
}

impl<'ctx> CallSiteTryAsValue<'ctx> {
    pub fn basic(self) -> Option<LlvmResult<BasicValueEnum<'ctx>>> {
        self.value
    }

    pub fn unwrap_basic(self) -> LlvmResult<BasicValueEnum<'ctx>> {
        self.value
            .ok_or_else(|| LlvmError::ice("expected non-void call result"))?
    }
}
