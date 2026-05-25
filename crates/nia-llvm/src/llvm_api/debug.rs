// SPDX-License-Identifier: GPL-3.0-or-later
//! LLVM debug-info wrappers.
//!
//! These types wrap DIBuilder metadata creation for compile units, files,
//! scopes, subprograms, variables, composite types, and debug locations while
//! keeping raw metadata handles behind typed values.

use llvm_sys::LLVMModuleFlagBehavior;
use llvm_sys::core::{LLVMAddModuleFlag, LLVMValueAsMetadata};
use llvm_sys::debuginfo::{
    LLVMCreateDIBuilder, LLVMDIBuilderCreateArrayType, LLVMDIBuilderCreateAutoVariable,
    LLVMDIBuilderCreateBasicType, LLVMDIBuilderCreateCompileUnit, LLVMDIBuilderCreateDebugLocation,
    LLVMDIBuilderCreateExpression, LLVMDIBuilderCreateFile, LLVMDIBuilderCreateFunction,
    LLVMDIBuilderCreateMemberType, LLVMDIBuilderCreateParameterVariable,
    LLVMDIBuilderCreatePointerType, LLVMDIBuilderCreateReplaceableCompositeType,
    LLVMDIBuilderCreateStructType, LLVMDIBuilderCreateSubroutineType, LLVMDIBuilderCreateUnionType,
    LLVMDIBuilderCreateUnspecifiedType, LLVMDIBuilderFinalize, LLVMDIBuilderGetOrCreateSubrange,
    LLVMDIBuilderInsertDeclareRecordAtEnd as LLVMDIBuilderInsertDeclareAtEnd, LLVMDIFlagZero,
    LLVMDWARFEmissionKind, LLVMDWARFSourceLanguage, LLVMDWARFTypeEncoding,
    LLVMDebugMetadataVersion, LLVMDisposeDIBuilder, LLVMMetadataReplaceAllUsesWith,
};
use llvm_sys::prelude::{LLVMDIBuilderRef, LLVMMetadataRef};
use std::marker::PhantomData;

use super::{AsValueRef, BasicBlock, BasicValue, Context, InstructionValue, Module, PointerValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleFlagBehavior {
    Warning,
}

impl From<ModuleFlagBehavior> for LLVMModuleFlagBehavior {
    fn from(value: ModuleFlagBehavior) -> Self {
        match value {
            ModuleFlagBehavior::Warning => LLVMModuleFlagBehavior::LLVMModuleFlagBehaviorWarning,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DIFile<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DIFile<'ctx> {
    fn new(raw: LLVMMetadataRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DICompileUnit<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DICompileUnit<'ctx> {
    fn new(raw: LLVMMetadataRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DISubroutineType<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DISubroutineType<'ctx> {
    fn new(raw: LLVMMetadataRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DISubprogram<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DISubprogram<'ctx> {
    fn new(raw: LLVMMetadataRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DILocation<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DIType<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DIType<'ctx> {
    fn new(raw: LLVMMetadataRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }
}

pub struct DIMemberTypeInput<'ctx, 'a> {
    pub scope: DICompileUnit<'ctx>,
    pub name: &'a str,
    pub file: DIFile<'ctx>,
    pub size_in_bits: u64,
    pub align_in_bits: u32,
    pub offset_in_bits: u64,
    pub ty: DIType<'ctx>,
}

pub struct DICompositeTypeInput<'ctx, 'a> {
    pub scope: DICompileUnit<'ctx>,
    pub name: &'a str,
    pub file: DIFile<'ctx>,
    pub size_in_bits: u64,
    pub align_in_bits: u32,
    pub elements: &'a [DIType<'ctx>],
    pub unique_id: &'a str,
}

pub struct DIReplaceableCompositeTypeInput<'ctx, 'a> {
    pub tag: u32,
    pub scope: DICompileUnit<'ctx>,
    pub name: &'a str,
    pub file: DIFile<'ctx>,
    pub size_in_bits: u64,
    pub align_in_bits: u32,
    pub unique_id: &'a str,
}

pub struct DIFunctionInput<'ctx, 'a> {
    pub scope: DICompileUnit<'ctx>,
    pub file: DIFile<'ctx>,
    pub name: &'a str,
    pub linkage_name: &'a str,
    pub line: u32,
    pub scope_line: u32,
    pub subroutine_type: DISubroutineType<'ctx>,
    pub is_local_to_unit: bool,
    pub is_optimized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DILocalVariable<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DILocalVariable<'ctx> {
    fn new(raw: LLVMMetadataRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DIExpression<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DIExpression<'ctx> {
    fn new(raw: LLVMMetadataRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }
}

impl<'ctx> DILocation<'ctx> {
    fn new(raw: LLVMMetadataRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }
}

#[derive(Debug)]
pub struct DebugInfoBuilder<'ctx> {
    raw: LLVMDIBuilderRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DebugInfoBuilder<'ctx> {
    pub(super) fn new(raw: LLVMDIBuilderRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    pub fn create_file(&self, filename: &str, directory: &str) -> DIFile<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreateFile(
                self.raw,
                filename.as_ptr() as *const _,
                filename.len(),
                directory.as_ptr() as *const _,
                directory.len(),
            )
        };
        DIFile::new(raw)
    }

    pub fn create_compile_unit(
        &self,
        file: DIFile<'ctx>,
        producer: &str,
        is_optimized: bool,
    ) -> DICompileUnit<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreateCompileUnit(
                self.raw,
                LLVMDWARFSourceLanguage::LLVMDWARFSourceLanguageC,
                file.raw,
                producer.as_ptr() as *const _,
                producer.len(),
                if is_optimized { 1 } else { 0 },
                std::ptr::null(),
                0,
                0,
                std::ptr::null(),
                0,
                LLVMDWARFEmissionKind::LLVMDWARFEmissionKindFull,
                0,
                0,
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
            )
        };
        DICompileUnit::new(raw)
    }

    pub fn create_unspecified_type(&self, name: &str) -> DIType<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreateUnspecifiedType(self.raw, name.as_ptr() as *const _, name.len())
        };
        DIType::new(raw)
    }

    pub fn create_basic_type(
        &self,
        name: &str,
        size_in_bits: u64,
        encoding: LLVMDWARFTypeEncoding,
    ) -> DIType<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreateBasicType(
                self.raw,
                name.as_ptr() as *const _,
                name.len(),
                size_in_bits,
                encoding,
                0,
            )
        };
        DIType::new(raw)
    }

    pub fn create_pointer_type(
        &self,
        pointee: DIType<'ctx>,
        size_in_bits: u64,
        align_in_bits: u32,
        name: &str,
    ) -> DIType<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreatePointerType(
                self.raw,
                pointee.raw,
                size_in_bits,
                align_in_bits,
                0,
                name.as_ptr() as *const _,
                name.len(),
            )
        };
        DIType::new(raw)
    }

    pub fn create_member_type(&self, input: DIMemberTypeInput<'ctx, '_>) -> DIType<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreateMemberType(
                self.raw,
                input.scope.raw,
                input.name.as_ptr() as *const _,
                input.name.len(),
                input.file.raw,
                0,
                input.size_in_bits,
                input.align_in_bits,
                input.offset_in_bits,
                LLVMDIFlagZero,
                input.ty.raw,
            )
        };
        DIType::new(raw)
    }

    pub fn create_struct_type(&self, input: DICompositeTypeInput<'ctx, '_>) -> DIType<'ctx> {
        let mut elements = input
            .elements
            .iter()
            .map(|elem| elem.raw)
            .collect::<Vec<_>>();
        let raw = unsafe {
            LLVMDIBuilderCreateStructType(
                self.raw,
                input.scope.raw,
                input.name.as_ptr() as *const _,
                input.name.len(),
                input.file.raw,
                0,
                input.size_in_bits,
                input.align_in_bits,
                LLVMDIFlagZero,
                std::ptr::null_mut(),
                elements.as_mut_ptr(),
                elements.len() as u32,
                0,
                std::ptr::null_mut(),
                input.unique_id.as_ptr() as *const _,
                input.unique_id.len(),
            )
        };
        DIType::new(raw)
    }

    pub fn create_union_type(&self, input: DICompositeTypeInput<'ctx, '_>) -> DIType<'ctx> {
        let mut elements = input
            .elements
            .iter()
            .map(|elem| elem.raw)
            .collect::<Vec<_>>();
        let raw = unsafe {
            LLVMDIBuilderCreateUnionType(
                self.raw,
                input.scope.raw,
                input.name.as_ptr() as *const _,
                input.name.len(),
                input.file.raw,
                0,
                input.size_in_bits,
                input.align_in_bits,
                LLVMDIFlagZero,
                elements.as_mut_ptr(),
                elements.len() as u32,
                0,
                input.unique_id.as_ptr() as *const _,
                input.unique_id.len(),
            )
        };
        DIType::new(raw)
    }

    pub fn create_array_type(
        &self,
        elem: DIType<'ctx>,
        size_in_bits: u64,
        align_in_bits: u32,
        len: i64,
    ) -> DIType<'ctx> {
        let mut subscripts = [unsafe { LLVMDIBuilderGetOrCreateSubrange(self.raw, 0, len) }];
        let raw = unsafe {
            LLVMDIBuilderCreateArrayType(
                self.raw,
                size_in_bits,
                align_in_bits,
                elem.raw,
                subscripts.as_mut_ptr(),
                subscripts.len() as u32,
            )
        };
        DIType::new(raw)
    }

    pub fn create_replaceable_composite_type(
        &self,
        input: DIReplaceableCompositeTypeInput<'ctx, '_>,
    ) -> DIType<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreateReplaceableCompositeType(
                self.raw,
                input.tag,
                input.name.as_ptr() as *const _,
                input.name.len(),
                input.scope.raw,
                input.file.raw,
                0,
                0,
                input.size_in_bits,
                input.align_in_bits,
                LLVMDIFlagZero,
                input.unique_id.as_ptr() as *const _,
                input.unique_id.len(),
            )
        };
        DIType::new(raw)
    }

    pub fn replace_all_uses_with(&self, from: DIType<'ctx>, to: DIType<'ctx>) {
        unsafe { LLVMMetadataReplaceAllUsesWith(from.raw, to.raw) };
    }

    pub fn create_subroutine_type(&self, file: DIFile<'ctx>) -> DISubroutineType<'ctx> {
        let mut tys = [std::ptr::null_mut()];
        let raw = unsafe {
            LLVMDIBuilderCreateSubroutineType(
                self.raw,
                file.raw,
                tys.as_mut_ptr(),
                tys.len() as u32,
                0,
            )
        };
        DISubroutineType::new(raw)
    }

    pub fn create_function(&self, input: DIFunctionInput<'ctx, '_>) -> DISubprogram<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreateFunction(
                self.raw,
                input.scope.raw,
                input.name.as_ptr() as *const _,
                input.name.len(),
                input.linkage_name.as_ptr() as *const _,
                input.linkage_name.len(),
                input.file.raw,
                input.line,
                input.subroutine_type.raw,
                if input.is_local_to_unit { 1 } else { 0 },
                1,
                input.scope_line,
                0,
                if input.is_optimized { 1 } else { 0 },
            )
        };
        DISubprogram::new(raw)
    }

    pub fn create_debug_location(
        &self,
        context: &'ctx Context,
        line: u32,
        column: u32,
        scope: DISubprogram<'ctx>,
    ) -> DILocation<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreateDebugLocation(
                context.raw,
                line,
                column,
                scope.raw,
                std::ptr::null_mut(),
            )
        };
        DILocation::new(raw)
    }

    pub fn create_parameter_variable(
        &self,
        scope: DISubprogram<'ctx>,
        name: &str,
        arg_no: u32,
        file: DIFile<'ctx>,
        line: u32,
        ty: DIType<'ctx>,
    ) -> DILocalVariable<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreateParameterVariable(
                self.raw,
                scope.raw,
                name.as_ptr() as *const _,
                name.len(),
                arg_no,
                file.raw,
                line,
                ty.raw,
                1,
                0,
            )
        };
        DILocalVariable::new(raw)
    }

    pub fn create_auto_variable(
        &self,
        scope: DISubprogram<'ctx>,
        name: &str,
        file: DIFile<'ctx>,
        line: u32,
        ty: DIType<'ctx>,
        align_in_bits: u32,
    ) -> DILocalVariable<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreateAutoVariable(
                self.raw,
                scope.raw,
                name.as_ptr() as *const _,
                name.len(),
                file.raw,
                line,
                ty.raw,
                1,
                0,
                align_in_bits,
            )
        };
        DILocalVariable::new(raw)
    }

    pub fn create_expression(&self) -> DIExpression<'ctx> {
        let raw = unsafe { LLVMDIBuilderCreateExpression(self.raw, std::ptr::null_mut(), 0) };
        DIExpression::new(raw)
    }

    pub fn insert_declare_at_end(
        &self,
        storage: PointerValue<'ctx>,
        variable: DILocalVariable<'ctx>,
        expr: DIExpression<'ctx>,
        location: DILocation<'ctx>,
        block: BasicBlock<'ctx>,
    ) -> InstructionValue<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderInsertDeclareAtEnd(
                self.raw,
                storage.as_value_ref(),
                variable.raw,
                expr.raw,
                location.raw,
                block.raw,
            )
        };
        InstructionValue::new(raw as _)
    }

    pub fn finalize(&self) {
        unsafe { LLVMDIBuilderFinalize(self.raw) };
    }
}

impl<'ctx> Drop for DebugInfoBuilder<'ctx> {
    fn drop(&mut self) {
        unsafe { LLVMDisposeDIBuilder(self.raw) };
    }
}

impl Context {
    pub fn debug_metadata_version(&self) -> u32 {
        unsafe { LLVMDebugMetadataVersion() }
    }
}

impl<'ctx> Module<'ctx> {
    pub fn create_debug_info_builder(&self) -> DebugInfoBuilder<'ctx> {
        DebugInfoBuilder::new(unsafe { LLVMCreateDIBuilder(self.raw) })
    }

    pub fn add_basic_value_flag<V: BasicValue<'ctx>>(
        &self,
        key: &str,
        behavior: ModuleFlagBehavior,
        value: V,
    ) {
        let metadata = unsafe { LLVMValueAsMetadata(value.as_value_ref()) };
        unsafe {
            LLVMAddModuleFlag(
                self.raw,
                behavior.into(),
                key.as_ptr() as *mut _,
                key.len(),
                metadata,
            )
        };
    }
}
