// SPDX-License-Identifier: GPL-3.0-or-later
//! Lowering of the target C calling convention for `extern` functions.
//!
//! [`nia_abi_check::c_abi`] decides how each C value travels; this module turns
//! that decision into LLVM function types and parameter attributes. Function
//! code generation uses the same classification for calls and for the
//! prologue and returns of `extern` definitions, so every side of a C boundary
//! agrees on one machine signature.

use super::ModuleCodegen;
use nia_abi_check::c_abi::{
    CAbiType, CAggregate, CArg, CConvention, CExtension, CFunctionAbi, CPass, CRet, CType,
    classify_c_function,
};
use nia_diagnostic::Diagnostic;
use nia_ids::InternedTyId;
use nia_llvm::{
    Attribute, AttributeLoc,
    types::{BasicMetadataTypeEnum, BasicTypeEnum, FunctionType},
    values::FunctionValue,
};
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyKind};

/// One C argument as a call site or definition sees it: its Nia type, which
/// fixes the in-memory form, and the C type it is classified as.
#[derive(Debug, Clone)]
pub(crate) struct CParam {
    pub(crate) ty: InternedTyId,
    pub(crate) c_type: CType,
}

impl<'ctx> ModuleCodegen<'ctx, '_> {
    pub(crate) fn c_convention(&self) -> CConvention {
        CConvention::for_target(self.target)
    }

    /// The C view of a type the ABI checker admitted at a C boundary.
    pub(crate) fn c_type_of(&self, ty: InternedTyId, span: Span) -> Result<CType, Diagnostic> {
        let pointer_size = self.source.layouts.target.pointer_size;
        let int = |bytes, signed| CType::Int { bytes, signed };
        match self.ty_kind(ty) {
            Some(TyKind::Primitive(primitive)) => Ok(match primitive {
                PrimitiveTy::I8 => int(1, true),
                PrimitiveTy::U8 => int(1, false),
                PrimitiveTy::I16 => int(2, true),
                PrimitiveTy::U16 => int(2, false),
                PrimitiveTy::I32 => int(4, true),
                PrimitiveTy::U32 => int(4, false),
                PrimitiveTy::I64 => int(8, true),
                PrimitiveTy::U64 => int(8, false),
                PrimitiveTy::I128 => int(16, true),
                PrimitiveTy::U128 => int(16, false),
                PrimitiveTy::Isize => int(pointer_size, true),
                PrimitiveTy::Usize => int(pointer_size, false),
                PrimitiveTy::F32 => CType::Float,
                PrimitiveTy::F64 => CType::Double,
                PrimitiveTy::Bool | PrimitiveTy::Char | PrimitiveTy::Never => {
                    return Err(self.error(span, "primitive type has no C representation"));
                }
            }),
            Some(
                TyKind::Pointer { .. }
                | TyKind::VolatilePointer { .. }
                | TyKind::FunctionPointer { .. },
            ) => Ok(CType::Pointer),
            Some(TyKind::Array { len, elem }) => Ok(CType::Array {
                elem: Box::new(self.c_type_of(*elem, span)?),
                len: self.array_len(len, span)?,
            }),
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                let layout = self
                    .struct_layout(*def_id, args, const_args)
                    .ok_or_else(|| self.error(span, "C aggregate has no struct layout"))?;
                let declared = self.struct_fields(*def_id, args, const_args, span)?;
                let mut fields = Vec::with_capacity(layout.fields.len());
                for field in &layout.fields {
                    let backend = declared
                        .iter()
                        .find(|candidate| {
                            super::types::layout_field_matches(
                                *def_id,
                                field.def_id,
                                &candidate.def_id,
                            )
                        })
                        .ok_or_else(|| self.error(span, "C aggregate field has no declaration"))?;
                    fields.push((field.offset, self.c_type_of(backend.ty, span)?));
                }
                Ok(CType::Aggregate(CAggregate {
                    size: layout.layout.size,
                    align: layout.layout.align,
                    fields,
                }))
            }
            _ => Err(self.error(span, "type has no C representation")),
        }
    }

    pub(crate) fn c_params(
        &self,
        params: impl IntoIterator<Item = (InternedTyId, Span)>,
    ) -> Result<Vec<CParam>, Diagnostic> {
        params
            .into_iter()
            .map(|(ty, span)| {
                Ok(CParam {
                    ty,
                    c_type: self.c_type_of(ty, span)?,
                })
            })
            .collect()
    }

    pub(crate) fn c_return_type(
        &self,
        return_type: InternedTyId,
        span: Span,
    ) -> Result<Option<CType>, Diagnostic> {
        if self.ty_kind(return_type).is_some_and(TyKind::is_unit) {
            Ok(None)
        } else {
            self.c_type_of(return_type, span).map(Some)
        }
    }

    pub(crate) fn classify_c(&self, params: &[CParam], ret: Option<&CType>) -> CFunctionAbi {
        let c_types = params
            .iter()
            .map(|param| param.c_type.clone())
            .collect::<Vec<_>>();
        classify_c_function(self.c_convention(), &c_types, ret)
    }

    pub(crate) fn c_abi_llvm_type(&self, ty: &CAbiType) -> Result<BasicTypeEnum<'ctx>, Diagnostic> {
        let context = self.context;
        Ok(match ty {
            CAbiType::Int(bits) => context
                .custom_width_int_type(*bits)
                .map_err(Self::diagnostic_from_llvm_error)?
                .into(),
            CAbiType::Float => context.f32_type().into(),
            CAbiType::Double => context.f64_type().into(),
            CAbiType::Pointer => context.ptr_type(Default::default()).into(),
            CAbiType::FloatPair => BasicTypeEnum::from(context.f32_type())
                .vector_type(2)
                .map_err(Self::diagnostic_from_llvm_error)?
                .into(),
            CAbiType::IntVectorPair => BasicTypeEnum::from(context.i64_type())
                .vector_type(2)
                .map_err(Self::diagnostic_from_llvm_error)?
                .into(),
            CAbiType::Array(elem, len) => self
                .c_abi_llvm_type(elem)?
                .array_type(*len)
                .map_err(Self::diagnostic_from_llvm_error)?
                .into(),
            CAbiType::Struct(fields) => {
                let fields = fields
                    .iter()
                    .map(|field| self.c_abi_llvm_type(field))
                    .collect::<Result<Vec<_>, _>>()?;
                context
                    .struct_type(&fields, false)
                    .map_err(Self::diagnostic_from_llvm_error)?
                    .into()
            }
        })
    }

    /// An upper bound on the size and alignment LLVM gives `ty`, used to size
    /// the buffers that reinterpret values between forms.
    pub(crate) fn c_abi_type_extent(&self, ty: &CAbiType) -> (u64, u64) {
        let pointer_size = self.source.layouts.target.pointer_size;
        match ty {
            CAbiType::Int(bits) => {
                let bytes = u64::from(bits.div_ceil(8)).next_power_of_two();
                (bytes, bytes.min(16))
            }
            CAbiType::Float => (4, 4),
            CAbiType::Double | CAbiType::FloatPair => (8, 8),
            CAbiType::Pointer => (pointer_size, pointer_size),
            CAbiType::IntVectorPair => (16, 16),
            CAbiType::Array(elem, len) => {
                let (size, align) = self.c_abi_type_extent(elem);
                (size * u64::from(*len), align)
            }
            CAbiType::Struct(fields) => {
                let mut size = 0u64;
                let mut align = 1u64;
                for field in fields {
                    let (field_size, field_align) = self.c_abi_type_extent(field);
                    size = size.next_multiple_of(field_align) + field_size;
                    align = align.max(field_align);
                }
                (size.next_multiple_of(align), align)
            }
        }
    }

    /// The LLVM type of a value passed in its own representation.
    fn c_natural_llvm_type(
        &self,
        ty: InternedTyId,
        span: Span,
    ) -> Result<BasicTypeEnum<'ctx>, Diagnostic> {
        self.llvm_basic_type_in(ty, span)
    }

    /// The LLVM parameter types an argument occupies.
    pub(crate) fn c_arg_llvm_types(
        &self,
        arg: &CArg,
        ty: InternedTyId,
        span: Span,
    ) -> Result<Vec<BasicTypeEnum<'ctx>>, Diagnostic> {
        Ok(match arg {
            CArg::Ignore => Vec::new(),
            CArg::Direct {
                pass: CPass::Natural,
                ..
            } => vec![self.c_natural_llvm_type(ty, span)?],
            CArg::Direct {
                pass: CPass::Coerced(abi),
                ..
            } => vec![self.c_abi_llvm_type(abi)?],
            CArg::Expand(parts) => parts
                .iter()
                .map(|(_, part)| self.c_abi_llvm_type(part))
                .collect::<Result<_, _>>()?,
            CArg::Byval { .. } | CArg::Indirect { .. } => {
                vec![self.context.ptr_type(Default::default()).into()]
            }
        })
    }

    /// The LLVM function type of a classified C signature. Variadic extras
    /// are not part of it; `params` lists the fixed parameters only.
    pub(crate) fn c_abi_function_type(
        &self,
        abi: &CFunctionAbi,
        params: &[CParam],
        return_type: InternedTyId,
        is_variadic: bool,
        span: Span,
    ) -> Result<FunctionType<'ctx>, Diagnostic> {
        let mut llvm_params = Vec::<BasicMetadataTypeEnum<'ctx>>::new();
        if matches!(abi.ret, CRet::SRet { .. }) {
            llvm_params.push(self.context.ptr_type(Default::default()).into());
        }
        for (arg, param) in abi.args.iter().zip(params) {
            llvm_params.extend(self.c_arg_llvm_types(arg, param.ty, span)?);
        }
        match &abi.ret {
            CRet::Void | CRet::SRet { .. } => self
                .context
                .void_type()
                .fn_type(&llvm_params, is_variadic)
                .map_err(Self::diagnostic_from_llvm_error),
            CRet::Direct {
                pass: CPass::Natural,
                ..
            } => self
                .c_natural_llvm_type(return_type, span)?
                .fn_type(&llvm_params, is_variadic)
                .map_err(Self::diagnostic_from_llvm_error),
            CRet::Direct {
                pass: CPass::Coerced(abi),
                ..
            } => self
                .c_abi_llvm_type(abi)?
                .fn_type(&llvm_params, is_variadic)
                .map_err(Self::diagnostic_from_llvm_error),
        }
    }

    fn named_attribute(&self, name: &str, value: u64) -> Result<Attribute<'ctx>, Diagnostic> {
        self.context
            .create_enum_attribute(Attribute::get_named_enum_kind_id(name), value)
            .map_err(Self::diagnostic_from_llvm_error)
    }

    fn type_attribute(
        &self,
        name: &str,
        ty: BasicTypeEnum<'ctx>,
    ) -> Result<Attribute<'ctx>, Diagnostic> {
        self.context
            .create_type_attribute(Attribute::get_named_enum_kind_id(name), ty)
            .map_err(Self::diagnostic_from_llvm_error)
    }

    fn extension_attribute(
        &self,
        extension: CExtension,
    ) -> Result<Option<Attribute<'ctx>>, Diagnostic> {
        match extension {
            CExtension::None => Ok(None),
            CExtension::Sign => self.named_attribute("signext", 0).map(Some),
            CExtension::Zero => self.named_attribute("zeroext", 0).map(Some),
        }
    }

    /// Attaches the C ABI attributes of an `extern` function to its
    /// declaration or definition.
    pub(super) fn apply_c_declaration_attributes(
        &self,
        function: FunctionValue<'ctx>,
        params: impl IntoIterator<Item = (InternedTyId, Span)>,
        return_type: InternedTyId,
        span: Span,
    ) -> Result<(), Diagnostic> {
        let params = self.c_params(params)?;
        let ret = self.c_return_type(return_type, span)?;
        let abi = self.classify_c(&params, ret.as_ref());
        let arg_types = params
            .iter()
            .map(|param| self.c_natural_llvm_type(param.ty, span))
            .collect::<Result<Vec<_>, _>>()?;
        let result_type = ret
            .is_some()
            .then(|| self.c_natural_llvm_type(return_type, span))
            .transpose()?;
        for (loc, attribute) in self.c_abi_attributes(&abi, &arg_types, result_type, span)? {
            function.add_attribute(loc, attribute);
        }
        Ok(())
    }

    /// The attributes a classified signature places on a declaration or call
    /// site: result extension, `sret`, `byval`, pointer alignment, argument
    /// extension, and stack alignment.
    /// `arg_types` and `return_type` are the in-memory LLVM types of the
    /// arguments and the result, which `byval` and `sret` name.
    pub(crate) fn c_abi_attributes(
        &self,
        abi: &CFunctionAbi,
        arg_types: &[BasicTypeEnum<'ctx>],
        return_type: Option<BasicTypeEnum<'ctx>>,
        span: Span,
    ) -> Result<Vec<(AttributeLoc, Attribute<'ctx>)>, Diagnostic> {
        let mut attributes = Vec::new();
        let mut index = 0u32;
        match &abi.ret {
            CRet::Direct { extension, .. } => {
                if let Some(attribute) = self.extension_attribute(*extension)? {
                    attributes.push((AttributeLoc::Return, attribute));
                }
            }
            CRet::SRet { align } => {
                let ty = return_type.ok_or_else(|| self.error(span, "sret result has no type"))?;
                attributes.push((AttributeLoc::Param(0), self.type_attribute("sret", ty)?));
                attributes.push((
                    AttributeLoc::Param(0),
                    self.named_attribute("align", *align)?,
                ));
                index = 1;
            }
            CRet::Void => {}
        }
        for (arg, arg_type) in abi.args.iter().zip(arg_types) {
            let loc = AttributeLoc::Param(index);
            match arg {
                CArg::Ignore => continue,
                CArg::Direct {
                    extension,
                    align_stack,
                    ..
                } => {
                    if let Some(attribute) = self.extension_attribute(*extension)? {
                        attributes.push((loc, attribute));
                    }
                    if let Some(align) = align_stack {
                        attributes.push((loc, self.named_attribute("alignstack", *align)?));
                    }
                }
                CArg::Expand(parts) => {
                    index += u32::try_from(parts.len())
                        .map_err(|_| self.error(span, "C argument has too many parts"))?;
                    continue;
                }
                CArg::Byval { align } => {
                    attributes.push((loc, self.type_attribute("byval", *arg_type)?));
                    attributes.push((loc, self.named_attribute("align", *align)?));
                }
                CArg::Indirect { align } => {
                    attributes.push((loc, self.named_attribute("align", *align)?));
                }
            }
            index += 1;
        }
        Ok(attributes)
    }
}
