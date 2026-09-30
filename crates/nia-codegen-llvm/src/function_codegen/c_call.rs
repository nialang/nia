// SPDX-License-Identifier: GPL-3.0-or-later
//! Calls into and definitions of C-ABI functions.
//!
//! A C value has two forms: the in-memory form Nia code works with, and the
//! machine form the target's C calling convention passes it in (see
//! [`nia_abi_check::c_abi`]). This module moves values between the two at
//! every C boundary: arguments and results of calls, and parameters and
//! returns of `extern` definitions.

use super::FunctionCodegen;
use crate::module_codegen::{CParam, ModuleCodegen};
use nia_abi_check::c_abi::{CAbiType, CArg, CFunctionAbi, CPass, CRet, CType};
use nia_diagnostic::Diagnostic;
use nia_ids::InternedTyId;
use nia_llvm::{
    types::BasicTypeEnum,
    values::{BasicMetadataValueEnum, BasicValueEnum, FunctionValue, PointerValue},
};
use nia_span::Span;

/// The function a C call invokes.
pub(super) enum CCallTarget<'ctx> {
    Direct(FunctionValue<'ctx>),
}

/// The C signature of an `extern` definition, fixed for its whole body.
pub(super) struct CDefinitionAbi {
    pub(super) abi: CFunctionAbi,
    pub(super) params: Vec<CParam>,
}

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    /// A stack slot in the entry block, so that a call in a loop does not grow
    /// the frame on every iteration.
    fn entry_alloca(
        &self,
        ty: BasicTypeEnum<'ctx>,
        align: u64,
        name: &str,
        span: Span,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        let entry = self
            .llvm_function
            .get_first_basic_block()
            .ok_or_else(|| self.error(span, "function has no physical entry block"))?;
        let builder = self
            .module
            .context
            .create_builder()
            .map_err(ModuleCodegen::diagnostic_from_llvm_error)?;
        match entry.get_first_instruction() {
            Some(first) => builder.position_before(&first),
            None => builder.position_at_end(entry),
        }
        let slot = builder
            .build_alloca(ty, name)
            .map_err(|_| self.error(span, "failed to allocate C ABI storage"))?;
        let align = u32::try_from(align)
            .map_err(|_| self.error(span, "C ABI storage alignment is too large"))?;
        BasicValueEnum::from(slot)
            .as_instruction_value()
            .ok_or_else(|| self.error(span, "C ABI storage is not an instruction"))?
            .set_alignment(align)
            .map_err(|_| self.error(span, "failed to align C ABI storage"))?;
        Ok(slot)
    }

    fn natural_extent(&self, ty: InternedTyId, span: Span) -> Result<(u64, u64), Diagnostic> {
        self.module
            .layout_of(ty)
            .map(|layout| (layout.size, layout.align))
            .ok_or_else(|| self.error(span, "C value has no layout"))
    }

    /// Storage large and aligned enough for both `ty` and `abi`.
    fn reinterpret_buffer(
        &self,
        ty: InternedTyId,
        abi: &CAbiType,
        span: Span,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        let (size, align) = self.natural_extent(ty, span)?;
        let (abi_size, abi_align) = self.module.c_abi_type_extent(abi);
        let bytes = u32::try_from(size.max(abi_size))
            .map_err(|_| self.error(span, "C ABI value is too large"))?;
        let storage = BasicTypeEnum::from(self.module.context.i8_type())
            .array_type(bytes)
            .map_err(ModuleCodegen::diagnostic_from_llvm_error)?;
        self.entry_alloca(storage.into(), align.max(abi_align), "c.abi.coerce", span)
    }

    /// Reads a value of in-memory type `ty` as the machine type `abi`.
    fn coerce_to_c(
        &mut self,
        value: BasicValueEnum<'ctx>,
        ty: InternedTyId,
        abi: &CAbiType,
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let buffer = self.reinterpret_buffer(ty, abi, span)?;
        self.builder
            .build_store(buffer, value)
            .map_err(|_| self.error(span, "failed to store C argument"))?;
        let abi_ty = self.module.c_abi_llvm_type(abi)?;
        self.builder
            .build_load(abi_ty, buffer, "c.abi.arg")
            .map_err(|_| self.error(span, "failed to load C argument"))
    }

    /// Reads a machine value of type `abi` as the in-memory type `ty`.
    fn coerce_from_c(
        &mut self,
        value: BasicValueEnum<'ctx>,
        abi: &CAbiType,
        ty: InternedTyId,
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let buffer = self.reinterpret_buffer(ty, abi, span)?;
        self.builder
            .build_store(buffer, value)
            .map_err(|_| self.error(span, "failed to store C value"))?;
        let natural = self.module.llvm_basic_type(ty, span)?;
        self.builder
            .build_load(natural, buffer, "c.abi.value")
            .map_err(|_| self.error(span, "failed to load C value"))
    }

    /// Copies `value` to fresh storage aligned to at least `align`.
    fn spill_c_value(
        &mut self,
        value: BasicValueEnum<'ctx>,
        ty: InternedTyId,
        align: u64,
        span: Span,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        let (_, natural_align) = self.natural_extent(ty, span)?;
        let natural = self.module.llvm_basic_type(ty, span)?;
        let slot = self.entry_alloca(natural, natural_align.max(align), "c.abi.copy", span)?;
        self.builder
            .build_store(slot, value)
            .map_err(|_| self.error(span, "failed to copy C argument"))?;
        Ok(slot)
    }

    fn byte_offset(
        &self,
        base: PointerValue<'ctx>,
        offset: u64,
        span: Span,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        if offset == 0 {
            return Ok(base);
        }
        let index = self
            .module
            .context
            .i64_type()
            .const_int(offset, false)
            .map_err(ModuleCodegen::diagnostic_from_llvm_error)?;
        // SAFETY: every expanded part lies inside the aggregate at `base`.
        unsafe {
            self.builder
                .build_gep(self.module.context.i8_type(), base, &[index], "c.abi.part")
        }
        .map_err(|_| self.error(span, "failed to address C argument part"))
    }

    fn expand_parts_align(&self, parts: &[(u64, CAbiType)]) -> u64 {
        parts
            .iter()
            .map(|(_, part)| self.module.c_abi_type_extent(part).1)
            .max()
            .unwrap_or(1)
    }

    /// Widens a variadic extra to its promoted type: small integers to
    /// `int`, `float` to `double`.
    fn promote_variadic(
        &mut self,
        value: BasicValueEnum<'ctx>,
        c_type: &CType,
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        match c_type {
            CType::Int { bytes, signed } if *bytes < 4 => {
                let int = self.module.context.i32_type();
                let value = value.into_int_value()?;
                let widened = if *signed {
                    self.builder.build_int_s_extend(value, int, "c.vararg.int")
                } else {
                    self.builder.build_int_z_extend(value, int, "c.vararg.int")
                };
                Ok(widened
                    .map_err(|_| self.error(span, "failed to promote variadic integer"))?
                    .into())
            }
            CType::Float => Ok(self
                .builder
                .build_float_cast(
                    value.into_float_value()?,
                    self.module.context.f64_type(),
                    "c.vararg.double",
                )
                .map_err(|_| self.error(span, "failed to promote variadic float"))?
                .into()),
            _ => Ok(value),
        }
    }

    /// Emits a call through the target C calling convention. `fixed` lists
    /// the declared parameter types; arguments past it are variadic extras.
    pub(super) fn emit_c_call(
        &mut self,
        span: Span,
        target: CCallTarget<'ctx>,
        fixed: &[InternedTyId],
        return_type: InternedTyId,
        args: Vec<(BasicValueEnum<'ctx>, InternedTyId)>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, Diagnostic> {
        let mut params = Vec::with_capacity(args.len());
        let mut values = Vec::with_capacity(args.len());
        for (index, (value, ty)) in args.into_iter().enumerate() {
            let c_type = self.module.c_type_of(ty, span)?;
            let (value, c_type) = if index < fixed.len() {
                (value, c_type)
            } else {
                let promoted = self.promote_variadic(value, &c_type, span)?;
                (promoted, c_type.promoted_variadic())
            };
            params.push(CParam { ty, c_type });
            values.push(value);
        }
        let ret = self.module.c_return_type(return_type, span)?;
        let abi = self.module.classify_c(&params, ret.as_ref());

        let mut llvm_args = Vec::<BasicMetadataValueEnum<'ctx>>::new();
        let mut arg_types = Vec::with_capacity(values.len());
        let result_type = ret
            .is_some()
            .then(|| self.module.llvm_basic_type(return_type, span))
            .transpose()?;
        let result_slot = match (&abi.ret, result_type) {
            (CRet::SRet { align }, Some(ty)) => {
                let (_, natural_align) = self.natural_extent(return_type, span)?;
                let slot = self.entry_alloca(ty, natural_align.max(*align), "c.abi.sret", span)?;
                llvm_args.push(slot.into());
                Some(slot)
            }
            _ => None,
        };
        for ((arg, param), value) in abi.args.iter().zip(&params).zip(values) {
            arg_types.push(value.get_type()?);
            match arg {
                CArg::Ignore => {}
                CArg::Direct {
                    pass: CPass::Natural,
                    ..
                } => llvm_args.push(value),
                CArg::Direct {
                    pass: CPass::Coerced(abi),
                    ..
                } => llvm_args.push(self.coerce_to_c(value, param.ty, abi, span)?),
                CArg::Expand(parts) => {
                    let align = self.expand_parts_align(parts);
                    let copy = self.spill_c_value(value, param.ty, align, span)?;
                    for (offset, part) in parts {
                        let address = self.byte_offset(copy, *offset, span)?;
                        let part_ty = self.module.c_abi_llvm_type(part)?;
                        let part = self
                            .builder
                            .build_load(part_ty, address, "c.abi.part")
                            .map_err(|_| self.error(span, "failed to load C argument part"))?;
                        llvm_args.push(part);
                    }
                }
                CArg::Byval { align } | CArg::Indirect { align } => {
                    let copy = self.spill_c_value(value, param.ty, *align, span)?;
                    llvm_args.push(copy.into());
                }
            }
        }

        let call = match target {
            CCallTarget::Direct(function) => {
                self.builder.build_call(function, &llvm_args, "c.call")
            }
        }
        .map_err(|_| self.error(span, "failed to build C call"))?;
        for (loc, attribute) in self
            .module
            .c_abi_attributes(&abi, &arg_types, result_type, span)?
        {
            call.add_attribute(loc, attribute);
        }

        match abi.ret {
            CRet::Void => Ok(None),
            CRet::SRet { .. } => {
                let (Some(slot), Some(ty)) = (result_slot, result_type) else {
                    return Err(self.error(span, "C call lost its result storage"));
                };
                self.builder
                    .build_load(ty, slot, "c.result")
                    .map(Some)
                    .map_err(|_| self.error(span, "failed to load C result"))
            }
            CRet::Direct { pass, .. } => {
                let value = call
                    .try_as_basic_value()
                    .basic()
                    .ok_or_else(|| self.error(span, "C call did not produce a value"))??;
                match pass {
                    CPass::Natural => Ok(Some(value)),
                    CPass::Coerced(abi) => {
                        self.coerce_from_c(value, &abi, return_type, span).map(Some)
                    }
                }
            }
        }
    }

    /// The C signature of this function when it is an `extern` definition.
    pub(super) fn c_definition_abi(&self) -> Result<Option<CDefinitionAbi>, Diagnostic> {
        if !self.function.is_extern {
            return Ok(None);
        }
        let span = self.function.span;
        let params = self.module.c_params(
            self.function
                .params
                .iter()
                .map(|param| (param.passing_ty, param.span)),
        )?;
        let ret = self.module.c_return_type(self.function.return_type, span)?;
        let abi = self.module.classify_c(&params, ret.as_ref());
        Ok(Some(CDefinitionAbi { abi, params }))
    }

    fn c_param(&self, index: usize, span: Span) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let index = u32::try_from(index)
            .map_err(|_| self.error(span, "LLVM parameter index is too large"))?;
        self.llvm_function
            .get_nth_param(index)
            .ok_or_else(|| self.error(span, "missing LLVM function parameter"))?
            .map_err(ModuleCodegen::diagnostic_from_llvm_error)
    }

    /// Moves the machine arguments of an `extern` definition into the
    /// in-memory storage of its parameters.
    pub(super) fn store_c_params(&mut self, definition: &CDefinitionAbi) -> Result<(), Diagnostic> {
        let mut index = usize::from(matches!(definition.abi.ret, CRet::SRet { .. }));
        let params = self.function.params;
        for ((param, arg), c_param) in params
            .iter()
            .zip(&definition.abi.args)
            .zip(&definition.params)
        {
            let span = param.span;
            let storage = param
                .local_id
                .and_then(|local| self.locals.get(&local).copied());
            let natural = || self.module.llvm_basic_type(c_param.ty, span);
            match arg {
                CArg::Ignore => {}
                CArg::Direct { pass, .. } => {
                    let value = self.c_param(index, span)?;
                    index += 1;
                    if let Some(storage) = storage {
                        let value = match pass {
                            CPass::Natural => value,
                            CPass::Coerced(abi) => {
                                self.coerce_from_c(value, abi, c_param.ty, span)?
                            }
                        };
                        self.builder
                            .build_store(storage, value)
                            .map_err(|_| self.error(span, "failed to store C parameter"))?;
                    }
                }
                CArg::Expand(parts) => {
                    for (offset, _) in parts {
                        let value = self.c_param(index, span)?;
                        index += 1;
                        if let Some(storage) = storage {
                            let address = self.byte_offset(storage, *offset, span)?;
                            self.builder.build_store(address, value).map_err(|_| {
                                self.error(span, "failed to store C parameter part")
                            })?;
                        }
                    }
                }
                CArg::Byval { .. } | CArg::Indirect { .. } => {
                    let pointer = self.c_param(index, span)?.into_pointer_value()?;
                    index += 1;
                    if let Some(storage) = storage {
                        let value = self
                            .builder
                            .build_load(natural()?, pointer, "c.param")
                            .map_err(|_| self.error(span, "failed to load C parameter"))?;
                        self.builder
                            .build_store(storage, value)
                            .map_err(|_| self.error(span, "failed to store C parameter"))?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Returns `value` from an `extern` definition in its machine form.
    pub(super) fn emit_c_return(
        &mut self,
        definition: &CRet,
        span: Span,
        value: BasicValueEnum<'ctx>,
    ) -> Result<(), Diagnostic> {
        match definition {
            CRet::Void => {
                self.builder
                    .build_return(None)
                    .map_err(|_| self.error(span, "failed to build C return"))?;
            }
            CRet::SRet { .. } => {
                let out = self
                    .out_ptr
                    .ok_or_else(|| self.error(span, "missing C result pointer"))?;
                self.builder
                    .build_store(out, value)
                    .map_err(|_| self.error(span, "failed to store C result"))?;
                self.builder
                    .build_return(None)
                    .map_err(|_| self.error(span, "failed to build C return"))?;
            }
            CRet::Direct { pass, .. } => {
                let value = match pass {
                    CPass::Natural => value,
                    CPass::Coerced(abi) => {
                        self.coerce_to_c(value, self.function.return_type, abi, span)?
                    }
                };
                self.builder
                    .build_return(Some(&value))
                    .map_err(|_| self.error(span, "failed to build C return"))?;
            }
        }
        Ok(())
    }
}
