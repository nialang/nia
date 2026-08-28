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

use super::{
    AsValueRef, BasicBlock, BasicValue, Context, InstructionValue, LlvmError, LlvmResult, Module,
    PointerValue, checked_u32_count,
};

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
    fn new(raw: LLVMMetadataRef) -> LlvmResult<Self> {
        let raw = require_metadata(raw, "DIFile")?;
        Ok(Self {
            raw,
            _marker: PhantomData,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DICompileUnit<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DICompileUnit<'ctx> {
    fn new(raw: LLVMMetadataRef) -> LlvmResult<Self> {
        let raw = require_metadata(raw, "DICompileUnit")?;
        Ok(Self {
            raw,
            _marker: PhantomData,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DISubroutineType<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DISubroutineType<'ctx> {
    fn new(raw: LLVMMetadataRef) -> LlvmResult<Self> {
        let raw = require_metadata(raw, "DISubroutineType")?;
        Ok(Self {
            raw,
            _marker: PhantomData,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DISubprogram<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DISubprogram<'ctx> {
    fn new(raw: LLVMMetadataRef) -> LlvmResult<Self> {
        let raw = require_metadata(raw, "DISubprogram")?;
        Ok(Self {
            raw,
            _marker: PhantomData,
        })
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
    fn new(raw: LLVMMetadataRef) -> LlvmResult<Self> {
        let raw = require_metadata(raw, "DIType")?;
        Ok(Self {
            raw,
            _marker: PhantomData,
        })
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
    fn new(raw: LLVMMetadataRef) -> LlvmResult<Self> {
        let raw = require_metadata(raw, "DILocalVariable")?;
        Ok(Self {
            raw,
            _marker: PhantomData,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DIExpression<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DIExpression<'ctx> {
    fn new(raw: LLVMMetadataRef) -> LlvmResult<Self> {
        let raw = require_metadata(raw, "DIExpression")?;
        Ok(Self {
            raw,
            _marker: PhantomData,
        })
    }
}

impl<'ctx> DILocation<'ctx> {
    fn new(raw: LLVMMetadataRef) -> LlvmResult<Self> {
        let raw = require_metadata(raw, "DILocation")?;
        Ok(Self {
            raw,
            _marker: PhantomData,
        })
    }
}

#[derive(Debug)]
pub struct DebugInfoBuilder<'ctx> {
    raw: LLVMDIBuilderRef,
    _marker: PhantomData<&'ctx Context>,
}

fn require_metadata(raw: LLVMMetadataRef, kind: &str) -> LlvmResult<LLVMMetadataRef> {
    if raw.is_null() {
        Err(LlvmError::error(format!(
            "LLVM returned a null {kind} metadata handle"
        )))
    } else {
        Ok(raw)
    }
}

impl<'ctx> DebugInfoBuilder<'ctx> {
    pub(super) fn new(raw: LLVMDIBuilderRef) -> LlvmResult<Self> {
        if raw.is_null() {
            return Err(LlvmError::error("LLVM returned a null debug-info builder"));
        }
        Ok(Self {
            raw,
            _marker: PhantomData,
        })
    }

    pub fn create_file(&self, filename: &str, directory: &str) -> LlvmResult<DIFile<'ctx>> {
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
    ) -> LlvmResult<DICompileUnit<'ctx>> {
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

    pub fn create_unspecified_type(&self, name: &str) -> LlvmResult<DIType<'ctx>> {
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
    ) -> LlvmResult<DIType<'ctx>> {
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
    ) -> LlvmResult<DIType<'ctx>> {
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

    pub fn create_member_type(
        &self,
        input: DIMemberTypeInput<'ctx, '_>,
    ) -> LlvmResult<DIType<'ctx>> {
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

    pub fn create_struct_type(
        &self,
        input: DICompositeTypeInput<'ctx, '_>,
    ) -> LlvmResult<DIType<'ctx>> {
        let element_count = checked_u32_count(
            input.elements.len(),
            "LLVM debug struct type has too many elements",
        )?;
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
                element_count,
                0,
                std::ptr::null_mut(),
                input.unique_id.as_ptr() as *const _,
                input.unique_id.len(),
            )
        };
        DIType::new(raw)
    }

    pub fn create_union_type(
        &self,
        input: DICompositeTypeInput<'ctx, '_>,
    ) -> LlvmResult<DIType<'ctx>> {
        let element_count = checked_u32_count(
            input.elements.len(),
            "LLVM debug union type has too many elements",
        )?;
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
                element_count,
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
    ) -> LlvmResult<DIType<'ctx>> {
        if len < 0 {
            return Err(LlvmError::error(
                "LLVM debug array type length must be non-negative",
            ));
        }
        let subrange = require_metadata(
            unsafe { LLVMDIBuilderGetOrCreateSubrange(self.raw, 0, len) },
            "DI subrange",
        )?;
        let mut subscripts = [subrange];
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
    ) -> LlvmResult<DIType<'ctx>> {
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

    /// Creates a subroutine signature whose first metadata slot is the return
    /// type (`null` for `void`), followed by the parameter types.
    pub fn create_subroutine_type(
        &self,
        file: DIFile<'ctx>,
        return_type: Option<DIType<'ctx>>,
        parameter_types: &[DIType<'ctx>],
    ) -> LlvmResult<DISubroutineType<'ctx>> {
        let type_count = checked_subroutine_type_count(parameter_types.len())?;
        let mut types = Vec::with_capacity(parameter_types.len() + 1);
        types.push(return_type.map_or(std::ptr::null_mut(), |ty| ty.raw));
        types.extend(parameter_types.iter().map(|ty| ty.raw));
        let raw = unsafe {
            LLVMDIBuilderCreateSubroutineType(self.raw, file.raw, types.as_mut_ptr(), type_count, 0)
        };
        DISubroutineType::new(raw)
    }

    pub fn create_function(
        &self,
        input: DIFunctionInput<'ctx, '_>,
    ) -> LlvmResult<DISubprogram<'ctx>> {
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
    ) -> LlvmResult<DILocation<'ctx>> {
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
    ) -> LlvmResult<DILocalVariable<'ctx>> {
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
    ) -> LlvmResult<DILocalVariable<'ctx>> {
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

    pub fn create_expression(&self) -> LlvmResult<DIExpression<'ctx>> {
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
    ) -> LlvmResult<InstructionValue<'ctx>> {
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
        let raw = raw as llvm_sys::prelude::LLVMValueRef;
        if raw.is_null() {
            return Err(LlvmError::error(
                "LLVM returned a null debug declare instruction",
            ));
        }
        Ok(InstructionValue::new(raw))
    }

    pub fn finalize(&self) {
        unsafe { LLVMDIBuilderFinalize(self.raw) };
    }
}

fn checked_subroutine_type_count(parameter_count: usize) -> LlvmResult<u32> {
    let count = parameter_count
        .checked_add(1)
        .ok_or_else(|| LlvmError::error("LLVM debug subroutine type count exceeds usize"))?;
    checked_u32_count(count, "LLVM debug subroutine type has too many parameters")
}

impl<'ctx> Drop for DebugInfoBuilder<'ctx> {
    fn drop(&mut self) {
        unsafe { LLVMDisposeDIBuilder(self.raw) };
    }
}

impl Context {
    /// Returns the debug metadata version understood by the linked LLVM.
    pub fn debug_metadata_version(&self) -> u32 {
        unsafe { LLVMDebugMetadataVersion() }
    }
}

impl<'ctx> Module<'ctx> {
    /// Creates a DIBuilder, surfacing LLVM allocation failure as an error.
    pub fn create_debug_info_builder(&self) -> LlvmResult<DebugInfoBuilder<'ctx>> {
        DebugInfoBuilder::new(unsafe { LLVMCreateDIBuilder(self.raw) })
    }

    /// Adds a module flag whose payload is represented by a basic LLVM value.
    ///
    /// Conversion to metadata is checked before the handle reaches
    /// `LLVMAddModuleFlag`, whose C API cannot report a null payload itself.
    pub fn add_basic_value_flag<V: BasicValue<'ctx>>(
        &self,
        key: &str,
        behavior: ModuleFlagBehavior,
        value: V,
    ) -> LlvmResult<()> {
        let metadata = require_metadata(
            unsafe { LLVMValueAsMetadata(value.as_value_ref()) },
            "module flag",
        )?;
        unsafe {
            LLVMAddModuleFlag(
                self.raw,
                behavior.into(),
                key.as_ptr() as *mut _,
                key.len(),
                metadata,
            )
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn referenced_subroutine_type_list(ir: &str) -> &str {
        let signature = ir
            .lines()
            .find(|line| line.contains("!DISubroutineType(types: !"))
            .expect("subroutine type metadata");
        let list_id = signature
            .split_once("types: ")
            .expect("subroutine type list")
            .1
            .split([',', ')'])
            .next()
            .expect("subroutine type list id");
        ir.lines()
            .find(|line| line.starts_with(&format!("{list_id} = !{{")))
            .and_then(|line| line.split_once(" = ").map(|(_, list)| list))
            .expect("referenced subroutine type list")
    }

    #[test]
    fn rejects_null_debug_info_builder() {
        let error = DebugInfoBuilder::new(std::ptr::null_mut()).expect_err("null DIBuilder");

        assert_eq!(
            error,
            LlvmError::Error("LLVM returned a null debug-info builder".to_string())
        );
    }

    #[test]
    fn rejects_null_metadata_before_typed_handle_construction() {
        let error = DIType::new(std::ptr::null_mut()).expect_err("null metadata");

        assert_eq!(
            error,
            LlvmError::Error("LLVM returned a null DIType metadata handle".to_string())
        );
    }

    #[test]
    fn rejects_null_module_flag_metadata_before_attachment() {
        let error = require_metadata(std::ptr::null_mut(), "module flag")
            .expect_err("null module flag metadata");

        assert_eq!(
            error,
            LlvmError::Error("LLVM returned a null module flag metadata handle".to_string())
        );
    }

    #[test]
    fn creates_array_debug_type_with_checked_subrange() {
        let context = Context::create().unwrap();
        let module = context.create_module("debug-array").unwrap();
        let builder = module.create_debug_info_builder().unwrap();
        let element = builder.create_basic_type("u8", 8, 0x07).unwrap();

        let array = builder
            .create_array_type(element, 32, 8, 4)
            .expect("array debug type should be created");
        assert!(!array.raw.is_null());
    }

    #[test]
    fn rejects_negative_array_debug_length_before_llvm_call() {
        let context = Context::create().unwrap();
        let module = context.create_module("debug-array-negative").unwrap();
        let builder = module.create_debug_info_builder().unwrap();
        let element = builder.create_basic_type("u8", 8, 0x07).unwrap();

        let error = builder
            .create_array_type(element, 32, 8, -1)
            .expect_err("negative debug array length");
        assert_eq!(
            error,
            LlvmError::Error("LLVM debug array type length must be non-negative".to_string())
        );
    }

    #[test]
    fn encodes_void_return_in_subroutine_type_slot_zero() {
        let context = Context::create().unwrap();
        let module = context.create_module("debug-subroutine").unwrap();
        let builder = module.create_debug_info_builder().unwrap();
        let file = builder.create_file("main.nia", ".").unwrap();
        let unit = builder
            .create_compile_unit(file, "nia-test", false)
            .unwrap();
        let subroutine = builder.create_subroutine_type(file, None, &[]).unwrap();
        let function_type = context.void_type().fn_type(&[], false).unwrap();
        let function = module.add_function("main", function_type, None).unwrap();
        builder
            .create_function(DIFunctionInput {
                scope: unit,
                file,
                name: "main",
                linkage_name: "main",
                line: 1,
                scope_line: 1,
                subroutine_type: subroutine,
                is_local_to_unit: false,
                is_optimized: false,
            })
            .map(|subprogram| function.set_subprogram(subprogram))
            .unwrap();
        builder.finalize();

        let ir = module.ir_string().unwrap();
        assert_eq!(referenced_subroutine_type_list(&ir), "!{null}", "{ir}");
    }

    #[test]
    fn encodes_return_and_parameter_subroutine_types_in_order() {
        let context = Context::create().unwrap();
        let module = context.create_module("debug-typed-subroutine").unwrap();
        let builder = module.create_debug_info_builder().unwrap();
        let file = builder.create_file("main.nia", ".").unwrap();
        let unit = builder
            .create_compile_unit(file, "nia-test", false)
            .unwrap();
        let i32_debug = builder.create_basic_type("i32", 32, 0x05).unwrap();
        let subroutine = builder
            .create_subroutine_type(file, Some(i32_debug), &[i32_debug])
            .unwrap();
        let function_type = context
            .i32_type()
            .fn_type(&[context.i32_type().into()], false)
            .unwrap();
        let function = module
            .add_function("identity", function_type, None)
            .unwrap();
        builder
            .create_function(DIFunctionInput {
                scope: unit,
                file,
                name: "identity",
                linkage_name: "identity",
                line: 1,
                scope_line: 1,
                subroutine_type: subroutine,
                is_local_to_unit: false,
                is_optimized: false,
            })
            .map(|subprogram| function.set_subprogram(subprogram))
            .unwrap();
        builder.finalize();

        let ir = module.ir_string().unwrap();
        let list = referenced_subroutine_type_list(&ir);
        let entries = list
            .trim_start_matches("!{")
            .trim_end_matches('}')
            .split(", ")
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 2, "{ir}");
        assert!(entries.iter().all(|entry| entry.starts_with('!')), "{ir}");
        assert_eq!(entries[0], entries[1], "{ir}");
    }

    #[test]
    fn rejects_subroutine_parameter_count_overflow() {
        assert_eq!(
            checked_subroutine_type_count(u32::MAX as usize),
            Err(LlvmError::Error(
                "LLVM debug subroutine type has too many parameters".to_string()
            ))
        );
        if usize::BITS > u32::BITS {
            assert_eq!(
                checked_subroutine_type_count(usize::MAX),
                Err(LlvmError::Error(
                    "LLVM debug subroutine type count exceeds usize".to_string()
                ))
            );
        }
    }
}
