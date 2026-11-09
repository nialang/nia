// SPDX-License-Identifier: GPL-3.0-or-later
//! Calls into and definitions of C-ABI functions.
//!
//! A C value has two forms: the in-memory form Nia code works with, and the
//! machine form the target's C calling convention passes it in (see
//! [`nia_abi_check::c_abi`]). This module moves values between the two at
//! every C boundary: arguments and results of calls, and parameters and
//! returns of `extern` definitions.
//!
//! Immediate values cross as SSA values. Memory values cross through their
//! storage: a coerced or expanded value is read from or written to memory in
//! its machine parts, and only a value the C ABI passes in its natural LLVM
//! type is loaded or stored whole.

use super::FunctionCodegen;
use crate::module_codegen::{CParam, ModuleCodegen};
use nia_abi_check::c_abi::{CAbiType, CArg, CFunctionAbi, CPass, CRet, CType};
use nia_diagnostic::Diagnostic;
use nia_ids::InternedTyId;
use nia_llvm::{
    types::{BasicTypeEnum, FunctionType},
    values::{BasicMetadataValueEnum, BasicValueEnum, CallSiteValue, FunctionValue, PointerValue},
};
use nia_span::Span;

/// The function a C call invokes.
pub(super) enum CCallTarget<'ctx> {
    Direct(FunctionValue<'ctx>),
    Indirect {
        function_type: FunctionType<'ctx>,
        pointer: PointerValue<'ctx>,
    },
}

/// One evaluated argument of a C call.
pub(super) enum CArgument<'ctx> {
    /// An immediate value.
    Value(BasicValueEnum<'ctx>),
    /// A memory value in a temporary that this call owns.
    Stored(PointerValue<'ctx>),
}

/// The C signature of an `extern` definition, fixed for its whole body.
pub(super) struct CDefinitionAbi {
    pub(super) abi: CFunctionAbi,
    pub(super) params: Vec<CParam>,
}

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
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

    /// Reads the value of in-memory type `ty` at `value` as the machine type
    /// `abi`.
    fn coerce_to_c(
        &mut self,
        value: PointerValue<'ctx>,
        ty: InternedTyId,
        abi: &CAbiType,
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let buffer = self.reinterpret_buffer(ty, abi, span)?;
        self.copy_value(buffer, false, value, false, ty, span)?;
        let abi_ty = self.module.c_abi_llvm_type(abi)?;
        self.builder
            .build_load(abi_ty, buffer, "c.abi.arg")
            .map_err(|_| self.error(span, "failed to load C argument"))
    }

    /// Writes a machine value of type `abi` to `dest` as in-memory type `ty`.
    fn coerce_from_c(
        &mut self,
        value: BasicValueEnum<'ctx>,
        abi: &CAbiType,
        ty: InternedTyId,
        dest: PointerValue<'ctx>,
        span: Span,
    ) -> Result<(), Diagnostic> {
        let buffer = self.reinterpret_buffer(ty, abi, span)?;
        self.builder
            .build_store(buffer, value)
            .map_err(|_| self.error(span, "failed to store C value"))?;
        self.copy_value(dest, false, buffer, false, ty, span)
    }

    /// Storage holding the value at `value` aligned to at least `align`:
    /// `value` itself when it is aligned enough, or else a copy.
    fn aligned_c_storage(
        &mut self,
        value: PointerValue<'ctx>,
        ty: InternedTyId,
        align: u64,
        span: Span,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        let (_, natural_align) = self.natural_extent(ty, span)?;
        if align <= natural_align {
            return Ok(value);
        }
        let natural = self.module.llvm_basic_type(ty, span)?;
        let copy = self.entry_alloca(natural, align, "c.abi.copy", span)?;
        self.copy_value(copy, false, value, false, ty, span)?;
        Ok(copy)
    }

    /// Loads a memory value that the C ABI passes whole in its natural type.
    fn load_natural(
        &self,
        value: PointerValue<'ctx>,
        ty: InternedTyId,
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let natural = self.module.llvm_basic_type(ty, span)?;
        self.builder
            .build_load(natural, value, "c.abi.natural")
            .map_err(|_| self.error(span, "failed to load C value"))
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

    /// Evaluates the arguments of a C call. `fixed` lists the declared
    /// parameter types; an argument past it keeps its own type.
    pub(super) fn emit_c_arguments(
        &mut self,
        fixed: &[InternedTyId],
        args: &[nia_function_ir::FunctionExpr],
    ) -> Result<Vec<(CArgument<'ctx>, InternedTyId)>, Diagnostic> {
        let mut values = Vec::with_capacity(args.len());
        for (index, arg) in args.iter().enumerate() {
            let ty = fixed.get(index).copied().unwrap_or(arg.ty);
            let value = if self.is_memory_value(arg.ty) {
                let copy = self.temporary(arg.ty, "c.arg", arg.span)?;
                self.emit_into(arg, copy)?;
                CArgument::Stored(copy)
            } else {
                CArgument::Value(self.emit_expr(arg)?)
            };
            values.push((value, ty));
        }
        Ok(values)
    }

    /// Emits a call through the target C calling convention. `fixed` lists
    /// the declared parameter types; arguments past it are variadic extras.
    /// A memory result is written to `result`; an immediate one is returned.
    pub(super) fn emit_c_call(
        &mut self,
        span: Span,
        target: CCallTarget<'ctx>,
        fixed: &[InternedTyId],
        return_type: InternedTyId,
        args: Vec<(CArgument<'ctx>, InternedTyId)>,
        result: Option<PointerValue<'ctx>>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, Diagnostic> {
        self.emit_c_call_with_site(span, target, fixed, return_type, args, result)
            .map(|(_, value)| value)
    }

    pub(super) fn emit_c_call_with_site(
        &mut self,
        span: Span,
        target: CCallTarget<'ctx>,
        fixed: &[InternedTyId],
        return_type: InternedTyId,
        args: Vec<(CArgument<'ctx>, InternedTyId)>,
        result: Option<PointerValue<'ctx>>,
    ) -> Result<(CallSiteValue<'ctx>, Option<BasicValueEnum<'ctx>>), Diagnostic> {
        let mut params = Vec::with_capacity(args.len());
        let mut values = Vec::with_capacity(args.len());
        for (index, (value, ty)) in args.into_iter().enumerate() {
            let c_type = self.module.c_type_of(ty, span)?;
            let (value, c_type) = if index < fixed.len() {
                (value, c_type)
            } else {
                let value = match value {
                    CArgument::Value(value) => {
                        CArgument::Value(self.promote_variadic(value, &c_type, span)?)
                    }
                    stored => stored,
                };
                (value, c_type.promoted_variadic())
            };
            params.push(CParam { ty, c_type });
            values.push(value);
        }
        let ret = self.module.c_return_type(return_type, span)?;
        let abi = self.module.classify_c(&params, ret.as_ref());
        let memory_result = ret.is_some() && self.is_memory_value(return_type);
        if memory_result && result.is_none() {
            return Err(self.error(span, "C call result has no destination"));
        }
        // The C ABI may return an immediate value, such as a Windows `i128`,
        // in memory; it then lands in a slot and is loaded from there.
        let spilled_result = match abi.ret {
            CRet::SRet { .. }
            | CRet::Direct {
                pass: CPass::Coerced(_),
                ..
            } if ret.is_some() && !memory_result => {
                Some(self.temporary(return_type, "c.result", span)?)
            }
            _ => None,
        };
        let result = spilled_result.or(result);

        let mut llvm_args = Vec::<BasicMetadataValueEnum<'ctx>>::new();
        let mut arg_types = Vec::with_capacity(values.len());
        let result_type = ret
            .is_some()
            .then(|| self.module.llvm_basic_type(return_type, span))
            .transpose()?;
        let sret = match (&abi.ret, result) {
            (CRet::SRet { align }, Some(result)) => {
                let slot = self.aligned_c_storage(result, return_type, *align, span)?;
                llvm_args.push(slot.into());
                Some(slot)
            }
            (CRet::SRet { .. }, None) => {
                return Err(self.error(span, "C call result has no destination"));
            }
            _ => None,
        };
        for ((arg, param), value) in abi.args.iter().zip(&params).zip(values) {
            arg_types.push(match &value {
                CArgument::Value(value) => value.get_type()?,
                CArgument::Stored(_) => self.module.llvm_basic_type(param.ty, span)?,
            });
            let passed_whole = matches!(
                arg,
                CArg::Ignore
                    | CArg::Direct {
                        pass: CPass::Natural,
                        ..
                    }
            );
            let value = match value {
                CArgument::Value(value) if !passed_whole => {
                    let spill = self.temporary(param.ty, "c.arg", span)?;
                    self.builder
                        .build_store(spill, value)
                        .map_err(|_| self.error(span, "failed to store C argument"))?;
                    CArgument::Stored(spill)
                }
                value => value,
            };
            match (arg, value) {
                (CArg::Ignore, _) => {}
                (
                    CArg::Direct {
                        pass: CPass::Natural,
                        ..
                    },
                    CArgument::Value(value),
                ) => llvm_args.push(value),
                (
                    CArg::Direct {
                        pass: CPass::Natural,
                        ..
                    },
                    CArgument::Stored(value),
                ) => llvm_args.push(self.load_natural(value, param.ty, span)?),
                (
                    CArg::Direct {
                        pass: CPass::Coerced(abi),
                        ..
                    },
                    CArgument::Stored(value),
                ) => llvm_args.push(self.coerce_to_c(value, param.ty, abi, span)?),
                (CArg::Expand(parts), CArgument::Stored(value)) => {
                    for (offset, part) in parts {
                        let address = self.byte_offset(value, *offset, span)?;
                        let part_ty = self.module.c_abi_llvm_type(part)?;
                        let part = self
                            .builder
                            .build_load(part_ty, address, "c.abi.part")
                            .map_err(|_| self.error(span, "failed to load C argument part"))?;
                        llvm_args.push(part);
                    }
                }
                (CArg::Byval { align } | CArg::Indirect { align }, CArgument::Stored(value)) => {
                    let storage = self.aligned_c_storage(value, param.ty, *align, span)?;
                    llvm_args.push(storage.into());
                }
                (_, CArgument::Value(_)) => {
                    return Err(self.error(span, "C argument lost its storage"));
                }
            }
        }

        let call = match target {
            CCallTarget::Direct(function) => {
                self.builder.build_call(function, &llvm_args, "c.call")
            }
            CCallTarget::Indirect {
                function_type,
                pointer,
            } => self
                .builder
                .build_indirect_call(function_type, pointer, &llvm_args, "c.call"),
        }
        .map_err(|_| self.error(span, "failed to build C call"))?;
        for (loc, attribute) in self
            .module
            .c_abi_attributes(&abi, &arg_types, result_type, span)?
        {
            call.add_attribute(loc, attribute);
        }

        let immediate = match abi.ret {
            CRet::Void => None,
            CRet::SRet { .. } => {
                let (Some(slot), Some(result)) = (sret, result) else {
                    return Err(self.error(span, "C call lost its result storage"));
                };
                if slot != result {
                    self.copy_value(result, false, slot, false, return_type, span)?;
                }
                None
            }
            CRet::Direct { pass, .. } => {
                let value = call
                    .try_as_basic_value()
                    .basic()
                    .ok_or_else(|| self.error(span, "C call did not produce a value"))??;
                match (pass, result) {
                    (CPass::Natural, Some(result)) if memory_result => {
                        self.builder
                            .build_store(result, value)
                            .map_err(|_| self.error(span, "failed to store C result"))?;
                        None
                    }
                    (CPass::Natural, _) => Some(value),
                    (CPass::Coerced(abi), Some(result)) => {
                        self.coerce_from_c(value, &abi, return_type, result, span)?;
                        None
                    }
                    (CPass::Coerced(_), None) => {
                        return Err(self.error(span, "C call result has no destination"));
                    }
                }
            }
        };
        match spilled_result {
            Some(slot) => Ok((call, Some(self.load_natural(slot, return_type, span)?))),
            None => Ok((call, immediate)),
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
            match arg {
                CArg::Ignore => {}
                CArg::Direct { pass, .. } => {
                    let value = self.c_param(index, span)?;
                    index += 1;
                    let Some(storage) = storage else {
                        continue;
                    };
                    match pass {
                        CPass::Natural => {
                            self.builder
                                .build_store(storage, value)
                                .map_err(|_| self.error(span, "failed to store C parameter"))?;
                        }
                        CPass::Coerced(abi) => {
                            self.coerce_from_c(value, abi, c_param.ty, storage, span)?
                        }
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
                        self.copy_value(storage, false, pointer, false, c_param.ty, span)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Returns the immediate `value` from an `extern` definition in its
    /// machine form.
    pub(super) fn emit_c_return(
        &mut self,
        definition: &CRet,
        span: Span,
        value: BasicValueEnum<'ctx>,
    ) -> Result<(), Diagnostic> {
        match definition {
            CRet::Direct {
                pass: CPass::Natural,
                ..
            } => {
                self.builder
                    .build_return(Some(&value))
                    .map_err(|_| self.error(span, "failed to build C return"))?;
            }
            CRet::Void => {
                self.builder
                    .build_return(None)
                    .map_err(|_| self.error(span, "failed to build C return"))?;
            }
            CRet::SRet { .. }
            | CRet::Direct {
                pass: CPass::Coerced(_),
                ..
            } => {
                let slot = self.return_slot(span)?;
                self.builder
                    .build_store(slot, value)
                    .map_err(|_| self.error(span, "failed to store C result"))?;
                return self.emit_c_memory_return(definition, span, slot);
            }
        }
        Ok(())
    }

    /// Returns the memory result at `slot` from an `extern` definition in
    /// its machine form.
    pub(super) fn emit_c_memory_return(
        &mut self,
        definition: &CRet,
        span: Span,
        slot: PointerValue<'ctx>,
    ) -> Result<(), Diagnostic> {
        let ty = self.function.return_type;
        match definition {
            CRet::Void | CRet::SRet { .. } => {
                self.builder
                    .build_return(None)
                    .map_err(|_| self.error(span, "failed to build C return"))?;
            }
            CRet::Direct { pass, .. } => {
                let value = match pass {
                    CPass::Natural => self.load_natural(slot, ty, span)?,
                    CPass::Coerced(abi) => self.coerce_to_c(slot, ty, abi, span)?,
                };
                self.builder
                    .build_return(Some(&value))
                    .map_err(|_| self.error(span, "failed to build C return"))?;
            }
        }
        Ok(())
    }
}
