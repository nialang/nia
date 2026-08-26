// SPDX-License-Identifier: GPL-3.0-or-later
use super::{FunctionSignature, ModuleCodegen};
use nia_backend_ir::{
    BackendClosureEntry, BackendClosureEntryKey, BackendField, BackendFunction,
    BackendFunctionInstance, BackendStructInstance, BackendUnionInstance,
};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_llvm::{
    types::{BasicMetadataTypeEnum, BasicTypeEnum, FunctionType, StructType},
    values::FunctionValue,
};
use nia_mangle::mangle_symbol_id;
use nia_span::Span;
use nia_ty::{ArrayLenTy, LayoutBuiltin, PrimitiveTy, TyKind, TypeEquivalence};

pub(super) fn const_args_match_semantic(
    left: &[nia_ty::ConstGenericArg],
    right: &[nia_ty::ConstGenericArg],
    mut same_type: impl FnMut(InternedTyId, InternedTyId) -> bool,
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            same_type(left.ty, right.ty)
                && match (&left.value, &right.value) {
                    (
                        nia_ty::ConstGenericValue::Int(left),
                        nia_ty::ConstGenericValue::Int(right),
                    ) => left.bits() == right.bits(),
                    (left, right) => left == right,
                }
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AbiParam {
    Direct(InternedTyId),
    Omit,
    IndirectReadonly(InternedTyId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AbiReturn {
    Direct(InternedTyId),
    Void,
    IndirectOut(InternedTyId),
    Never,
}

impl<'ctx, 'a> ModuleCodegen<'ctx, 'a> {
    pub(crate) fn ty_kind(&self, ty: InternedTyId) -> Option<&'a TyKind> {
        self.program.ty_kind(ty)
    }

    pub(super) fn function_type_in(
        &self,
        function: &BackendFunction,
    ) -> Result<FunctionType<'ctx>, Diagnostic> {
        self.function_signature_type_in(FunctionSignature {
            param_tys: function
                .params
                .iter()
                .map(|param| (param.passing_ty, param.span)),
            return_type: function.return_type,
            is_extern: function.is_extern,
            is_variadic: function.is_variadic,
            span: function.span,
        })
    }

    pub(super) fn function_signature_type_in<P>(
        &self,
        signature: FunctionSignature<P>,
    ) -> Result<FunctionType<'ctx>, Diagnostic>
    where
        P: IntoIterator<Item = (InternedTyId, Span)>,
    {
        if signature.is_extern {
            return self.c_function_type_in(
                signature.param_tys,
                signature.return_type,
                signature.is_variadic,
                signature.span,
            );
        }
        let mut llvm_params = Vec::<BasicMetadataTypeEnum<'ctx>>::new();
        if let AbiReturn::IndirectOut(ty) = self.classify_return_in(signature.return_type) {
            llvm_params.push(self.pointer_abi_type(ty, signature.span)?);
        }
        for param in self.classify_params_in(signature.param_tys.into_iter().map(|(ty, _)| ty)) {
            match param {
                AbiParam::Direct(ty) => {
                    llvm_params.push(self.llvm_basic_type_in(ty, signature.span)?);
                }
                AbiParam::IndirectReadonly(ty) => {
                    llvm_params.push(self.pointer_abi_type(ty, signature.span)?);
                }
                AbiParam::Omit => {}
            }
        }
        match self.classify_return_in(signature.return_type) {
            AbiReturn::Direct(ty) => self
                .llvm_basic_type_in(ty, signature.span)?
                .fn_type(&llvm_params, signature.is_variadic)
                .map_err(Self::diagnostic_from_llvm_error),
            AbiReturn::Void | AbiReturn::IndirectOut(_) | AbiReturn::Never => self
                .context
                .void_type()
                .fn_type(&llvm_params, signature.is_variadic)
                .map_err(Self::diagnostic_from_llvm_error),
        }
    }

    fn c_function_type_in(
        &self,
        param_tys: impl IntoIterator<Item = (InternedTyId, Span)>,
        return_type: InternedTyId,
        is_variadic: bool,
        span: Span,
    ) -> Result<FunctionType<'ctx>, Diagnostic> {
        let mut llvm_params = Vec::<BasicMetadataTypeEnum<'ctx>>::new();
        for (param_ty, param_span) in param_tys {
            llvm_params.push(self.llvm_basic_type_in(param_ty, param_span)?);
        }
        match self.ty_kind(return_type) {
            Some(kind) if kind.is_unit() => self
                .context
                .void_type()
                .fn_type(&llvm_params, is_variadic)
                .map_err(Self::diagnostic_from_llvm_error),
            _ => self
                .llvm_basic_type_in(return_type, span)?
                .fn_type(&llvm_params, is_variadic)
                .map_err(Self::diagnostic_from_llvm_error),
        }
    }

    pub(crate) fn function_pointer_type_in(
        &self,
        params: &[InternedTyId],
        return_type: InternedTyId,
        is_variadic: bool,
        span: Span,
    ) -> Result<FunctionType<'ctx>, Diagnostic> {
        let mut llvm_params = Vec::<BasicMetadataTypeEnum<'ctx>>::new();
        if let AbiReturn::IndirectOut(ty) = self.classify_return_in(return_type) {
            llvm_params.push(self.pointer_abi_type(ty, span)?);
        }
        for param in self.classify_params_in(params.iter().copied()) {
            match param {
                AbiParam::Direct(ty) => {
                    llvm_params.push(self.llvm_basic_type_in(ty, span)?);
                }
                AbiParam::IndirectReadonly(ty) => {
                    llvm_params.push(self.pointer_abi_type(ty, span)?);
                }
                AbiParam::Omit => {}
            }
        }
        match self.classify_return_in(return_type) {
            AbiReturn::Direct(ty) => self
                .llvm_basic_type_in(ty, span)?
                .fn_type(&llvm_params, is_variadic)
                .map_err(Self::diagnostic_from_llvm_error),
            AbiReturn::Void | AbiReturn::IndirectOut(_) | AbiReturn::Never => self
                .context
                .void_type()
                .fn_type(&llvm_params, is_variadic)
                .map_err(Self::diagnostic_from_llvm_error),
        }
    }

    pub(crate) fn callable_entry_function_type_in(
        &self,
        params: &[InternedTyId],
        return_type: InternedTyId,
        span: Span,
    ) -> Result<FunctionType<'ctx>, Diagnostic> {
        let mut llvm_params = Vec::<BasicMetadataTypeEnum<'ctx>>::new();
        if let AbiReturn::IndirectOut(ty) = self.classify_return_in(return_type) {
            llvm_params.push(self.pointer_abi_type(ty, span)?);
        }
        llvm_params.push(self.context.ptr_type(Default::default()).into());
        for param in self.classify_params_in(params.iter().copied()) {
            match param {
                AbiParam::Direct(ty) => {
                    llvm_params.push(self.llvm_basic_type_in(ty, span)?);
                }
                AbiParam::IndirectReadonly(ty) => {
                    llvm_params.push(self.pointer_abi_type(ty, span)?);
                }
                AbiParam::Omit => {}
            }
        }
        match self.classify_return_in(return_type) {
            AbiReturn::Direct(ty) => self
                .llvm_basic_type_in(ty, span)?
                .fn_type(&llvm_params, false)
                .map_err(Self::diagnostic_from_llvm_error),
            AbiReturn::Void | AbiReturn::IndirectOut(_) | AbiReturn::Never => self
                .context
                .void_type()
                .fn_type(&llvm_params, false)
                .map_err(Self::diagnostic_from_llvm_error),
        }
    }

    pub(crate) fn dynamic_trait_method_type(
        &self,
        _object_ty: InternedTyId,
        params: &[InternedTyId],
        return_type: InternedTyId,
        span: Span,
    ) -> Result<FunctionType<'ctx>, Diagnostic> {
        let mut llvm_params = Vec::<BasicMetadataTypeEnum<'ctx>>::new();
        if let AbiReturn::IndirectOut(ty) = self.classify_function_return(return_type) {
            llvm_params.push(self.pointer_abi_type(ty, span)?);
        }
        llvm_params.push(self.context.ptr_type(Default::default()).into());
        for param in self.classify_function_params(params.iter().copied()) {
            match param {
                AbiParam::Direct(ty) => {
                    llvm_params.push(self.llvm_basic_type(ty, span)?);
                }
                AbiParam::IndirectReadonly(ty) => {
                    llvm_params.push(self.pointer_abi_type(ty, span)?);
                }
                AbiParam::Omit => {}
            }
        }
        match self.classify_function_return(return_type) {
            AbiReturn::Direct(ty) => self
                .llvm_basic_type(ty, span)?
                .fn_type(&llvm_params, false)
                .map_err(Self::diagnostic_from_llvm_error),
            AbiReturn::Void | AbiReturn::IndirectOut(_) | AbiReturn::Never => self
                .context
                .void_type()
                .fn_type(&llvm_params, false)
                .map_err(Self::diagnostic_from_llvm_error),
        }
    }

    pub(crate) fn classify_function_params(
        &self,
        params: impl IntoIterator<Item = InternedTyId>,
    ) -> Vec<AbiParam> {
        self.classify_params_in(params)
    }

    pub(crate) fn classify_function_return(&self, ty: InternedTyId) -> AbiReturn {
        self.classify_return_in(ty)
    }

    fn classify_params_in(&self, params: impl IntoIterator<Item = InternedTyId>) -> Vec<AbiParam> {
        params
            .into_iter()
            .map(|ty| self.classify_param_in(ty))
            .collect()
    }

    fn classify_param_in(&self, ty: InternedTyId) -> AbiParam {
        if self.layout_of(ty).is_some_and(|layout| layout.size == 0) {
            return AbiParam::Omit;
        }
        match self.ty_kind(ty) {
            Some(
                TyKind::Primitive(_)
                | TyKind::Vector { .. }
                | TyKind::Pointer { .. }
                | TyKind::VolatilePointer { .. }
                | TyKind::FunctionPointer { .. }
                | TyKind::Slice { .. }
                | TyKind::TraitObject { .. }
                | TyKind::Callable { .. }
                | TyKind::Range { .. },
            ) => AbiParam::Direct(ty),
            Some(TyKind::Nominal { def_id, .. })
                if self
                    .program
                    .enum_layout(*def_id)
                    .is_some_and(|layout| layout.payload_offset.is_none()) =>
            {
                AbiParam::Direct(ty)
            }
            Some(
                TyKind::Tuple(_)
                | TyKind::Array { .. }
                | TyKind::Nominal { .. }
                | TyKind::ClosureState { .. },
            ) => AbiParam::IndirectReadonly(ty),
            Some(TyKind::Optional { .. } | TyKind::ErrorUnion { .. }) => {
                AbiParam::IndirectReadonly(ty)
            }
            Some(
                TyKind::GenericParam(_)
                | TyKind::Opaque
                | TyKind::SelfParam
                | TyKind::BuiltinType(_)
                | TyKind::BuiltinTrait { .. }
                | TyKind::SlicePointee { .. }
                | TyKind::TraitObjectPointee { .. }
                | TyKind::CallablePointee { .. }
                | TyKind::Projection { .. }
                | TyKind::ConstOnly
                | TyKind::Error,
            )
            | None => AbiParam::Direct(ty),
        }
    }

    fn classify_return_in(&self, ty: InternedTyId) -> AbiReturn {
        match self.ty_kind(ty) {
            Some(kind) if kind.is_unit() => return AbiReturn::Void,
            Some(TyKind::Primitive(PrimitiveTy::Never)) => return AbiReturn::Never,
            _ => {}
        }
        if self.layout_of(ty).is_some_and(|layout| layout.size == 0) {
            return AbiReturn::Void;
        }
        match self.ty_kind(ty) {
            Some(
                TyKind::Primitive(_)
                | TyKind::Vector { .. }
                | TyKind::Pointer { .. }
                | TyKind::VolatilePointer { .. }
                | TyKind::FunctionPointer { .. }
                | TyKind::Slice { .. }
                | TyKind::TraitObject { .. }
                | TyKind::Callable { .. }
                | TyKind::Range { .. },
            ) => AbiReturn::Direct(ty),
            Some(TyKind::Nominal { def_id, .. })
                if self
                    .program
                    .enum_layout(*def_id)
                    .is_some_and(|layout| layout.payload_offset.is_none()) =>
            {
                AbiReturn::Direct(ty)
            }
            Some(
                TyKind::Tuple(_)
                | TyKind::Array { .. }
                | TyKind::Nominal { .. }
                | TyKind::ClosureState { .. },
            ) => AbiReturn::IndirectOut(ty),
            Some(TyKind::Optional { .. } | TyKind::ErrorUnion { .. }) => AbiReturn::IndirectOut(ty),
            Some(
                TyKind::GenericParam(_)
                | TyKind::Opaque
                | TyKind::SelfParam
                | TyKind::BuiltinType(_)
                | TyKind::BuiltinTrait { .. }
                | TyKind::SlicePointee { .. }
                | TyKind::TraitObjectPointee { .. }
                | TyKind::CallablePointee { .. }
                | TyKind::Projection { .. }
                | TyKind::ConstOnly
                | TyKind::Error,
            )
            | None => AbiReturn::Direct(ty),
        }
    }

    fn pointer_abi_type(
        &self,
        _ty: InternedTyId,
        _span: Span,
    ) -> Result<BasicTypeEnum<'ctx>, Diagnostic> {
        Ok(self.context.ptr_type(Default::default()).into())
    }

    pub(crate) fn llvm_basic_type(
        &self,
        ty: InternedTyId,
        span: Span,
    ) -> Result<BasicTypeEnum<'ctx>, Diagnostic> {
        self.llvm_basic_type_in(ty, span)
    }

    pub(crate) fn slice_type(&self, span: Span) -> Result<StructType<'ctx>, Diagnostic> {
        let len_ty = self.integer_llvm_type(PrimitiveTy::Usize, span)?;
        self.context
            .struct_type(
                &[
                    self.context.ptr_type(Default::default()).into(),
                    len_ty.into(),
                ],
                false,
            )
            .map_err(Self::diagnostic_from_llvm_error)
    }

    pub(crate) fn trait_object_type(&self) -> Result<StructType<'ctx>, Diagnostic> {
        self.context
            .struct_type(
                &[
                    self.context.ptr_type(Default::default()).into(),
                    self.context.ptr_type(Default::default()).into(),
                ],
                false,
            )
            .map_err(Self::diagnostic_from_llvm_error)
    }

    pub(crate) fn callable_type(&self) -> Result<StructType<'ctx>, Diagnostic> {
        self.context
            .struct_type(
                &[
                    self.context.ptr_type(Default::default()).into(),
                    self.context.ptr_type(Default::default()).into(),
                ],
                false,
            )
            .map_err(Self::diagnostic_from_llvm_error)
    }

    pub(crate) fn llvm_basic_type_in(
        &self,
        ty: InternedTyId,
        span: Span,
    ) -> Result<BasicTypeEnum<'ctx>, Diagnostic> {
        if self.layout_of(ty).is_some_and(|layout| layout.size == 0)
            && !matches!(
                self.ty_kind(ty),
                Some(
                    TyKind::Pointer { .. }
                        | TyKind::VolatilePointer { .. }
                        | TyKind::FunctionPointer { .. }
                )
            )
        {
            return self
                .context
                .struct_type(&[], false)
                .map(Into::into)
                .map_err(Self::diagnostic_from_llvm_error);
        }
        match self.ty_kind(ty) {
            Some(TyKind::Tuple(elems)) => {
                let fields = elems
                    .iter()
                    .map(|elem| self.llvm_basic_type_in(*elem, span))
                    .collect::<Result<Vec<_>, _>>()?;
                self.context
                    .struct_type(&fields, false)
                    .map(Into::into)
                    .map_err(Self::diagnostic_from_llvm_error)
            }
            Some(TyKind::ClosureState { captures, .. }) => {
                let fields = captures
                    .iter()
                    .map(|capture| self.llvm_basic_type_in(*capture, span))
                    .collect::<Result<Vec<_>, _>>()?;
                self.context
                    .struct_type(&fields, false)
                    .map(Into::into)
                    .map_err(Self::diagnostic_from_llvm_error)
            }
            Some(TyKind::Primitive(primitive)) => self.primitive_type(*primitive, span),
            Some(TyKind::Vector { elem, lanes }) => self.vector_type(*elem, *lanes, span),
            Some(
                TyKind::Pointer { .. }
                | TyKind::VolatilePointer { .. }
                | TyKind::FunctionPointer { .. },
            ) => Ok(self.context.ptr_type(Default::default()).into()),
            Some(TyKind::Slice { .. }) => Ok(self.slice_type(span)?.into()),
            Some(TyKind::TraitObject { .. }) => self.trait_object_type().map(Into::into),
            Some(TyKind::Callable { .. }) => self.callable_type().map(Into::into),
            Some(TyKind::Range { kind, bound }) => self.range_type(*kind, *bound, span),
            Some(TyKind::Optional { elem }) => self.optional_type(*elem, span),
            Some(TyKind::ErrorUnion { error, value }) => {
                self.error_union_type(*error, *value, span)
            }
            Some(TyKind::Array { len, elem }) => {
                let elem = self.llvm_basic_type_in(*elem, span)?;
                let len = self.array_len_in(len, span)?;
                if len > u32::MAX as u64 {
                    return Err(
                        self.error(span, format!("array length {len} is too large for LLVM"))
                    );
                }
                elem.array_type(len as u32)
                    .map(Into::into)
                    .map_err(Self::diagnostic_from_llvm_error)
            }
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                if let Some(struct_ty) = self.struct_instance_type(*def_id, args, const_args) {
                    return Ok(struct_ty.into());
                }
                if let Some(struct_ty) = self.structs.get(def_id).copied() {
                    return Ok(struct_ty.into());
                }
                if let Some(union_ty) = self.union_instance_type(*def_id, args, const_args) {
                    return Ok(union_ty.into());
                }
                if let Some(union_ty) = self.unions.get(def_id).copied() {
                    return Ok(union_ty.into());
                }
                if let Some(item) = self.program.enum_item(*def_id) {
                    return self.enum_type(item, span);
                }
                Err(self.error(span, "unknown nominal type during LLVM lowering"))
            }
            Some(
                TyKind::GenericParam(_)
                | TyKind::Opaque
                | TyKind::SelfParam
                | TyKind::BuiltinType(_)
                | TyKind::BuiltinTrait { .. }
                | TyKind::SlicePointee { .. }
                | TyKind::TraitObjectPointee { .. }
                | TyKind::CallablePointee { .. }
                | TyKind::Projection { .. }
                | TyKind::ConstOnly
                | TyKind::Error,
            )
            | None => Err(self.error(span, "type is not concrete enough for LLVM lowering")),
        }
    }

    pub(crate) fn range_type(
        &self,
        kind: nia_ty::RangeTyKind,
        bound: Option<InternedTyId>,
        span: Span,
    ) -> Result<BasicTypeEnum<'ctx>, Diagnostic> {
        let Some(bound) = bound else {
            return self
                .context
                .struct_type(&[], false)
                .map(Into::into)
                .map_err(Self::diagnostic_from_llvm_error);
        };
        let bound_ty = self.llvm_basic_type(bound, span)?;
        let fields = match kind {
            nia_ty::RangeTyKind::Exclusive | nia_ty::RangeTyKind::Inclusive => {
                vec![bound_ty, bound_ty]
            }
            nia_ty::RangeTyKind::From
            | nia_ty::RangeTyKind::To
            | nia_ty::RangeTyKind::ToInclusive => vec![bound_ty],
            nia_ty::RangeTyKind::Full => Vec::new(),
        };
        self.context
            .struct_type(&fields, false)
            .map(Into::into)
            .map_err(Self::diagnostic_from_llvm_error)
    }

    pub(crate) fn optional_type(
        &self,
        elem: InternedTyId,
        span: Span,
    ) -> Result<BasicTypeEnum<'ctx>, Diagnostic> {
        self.tagged_union_type(&[elem], span)
    }

    pub(crate) fn error_union_type(
        &self,
        error: InternedTyId,
        value: InternedTyId,
        span: Span,
    ) -> Result<BasicTypeEnum<'ctx>, Diagnostic> {
        self.tagged_union_type(&[error, value], span)
    }

    fn enum_type(
        &self,
        item: &nia_backend_ir::BackendEnum,
        span: Span,
    ) -> Result<BasicTypeEnum<'ctx>, Diagnostic> {
        let tag = self.llvm_basic_type_in(item.backing_type, span)?;
        let Some(layout) = self.program.enum_layout(item.def_id) else {
            return Err(self.error(span, "missing enum layout during LLVM lowering"));
        };
        let Some(payload_offset) = layout.payload_offset else {
            return Ok(tag);
        };
        let padding = payload_offset
            .checked_sub(layout.tag.size)
            .ok_or_else(|| self.error(span, "enum payload offset precedes its tag"))?;
        if padding > u32::MAX as u64 {
            return Err(self.error(span, "enum payload padding is too large for LLVM"));
        }
        let storage_size = layout
            .layout
            .size
            .checked_sub(payload_offset)
            .ok_or_else(|| self.error(span, "enum payload exceeds its layout size"))?;
        let storage_align = layout
            .variants
            .iter()
            .map(|variant| variant.payload.align)
            .max()
            .unwrap_or(1);
        let padding = self
            .context
            .i8_type()
            .array_type(padding as u32)
            .map(Into::into)
            .map_err(Self::diagnostic_from_llvm_error)?;
        let storage = self.union_storage_type(storage_size, storage_align, span)?;
        self.context
            .struct_type(&[tag, padding, storage], false)
            .map(Into::into)
            .map_err(Self::diagnostic_from_llvm_error)
    }

    fn tagged_union_type(
        &self,
        payloads: &[InternedTyId],
        span: Span,
    ) -> Result<BasicTypeEnum<'ctx>, Diagnostic> {
        let tag_ty: BasicTypeEnum<'ctx> = self.context.i8_type().into();
        let mut max_align = 1u64;
        let mut max_size = 0u64;
        for payload in payloads {
            let layout = self
                .layout_of(*payload)
                .ok_or_else(|| self.error(span, "missing tagged-union payload layout"))?;
            max_align = max_align.max(layout.align);
            max_size = max_size.max(layout.size);
        }
        let storage = self.union_storage_type(max_size, max_align, span)?;
        self.context
            .struct_type(&[tag_ty, storage], false)
            .map(Into::into)
            .map_err(Self::diagnostic_from_llvm_error)
    }

    pub(crate) fn union_storage_type(
        &self,
        size: u64,
        align: u64,
        span: Span,
    ) -> Result<BasicTypeEnum<'ctx>, Diagnostic> {
        if size == 0 {
            return self
                .context
                .struct_type(&[], false)
                .map(Into::into)
                .map_err(Self::diagnostic_from_llvm_error);
        }
        let align_ty = self.union_alignment_type(align, span)?;
        let align_size = align;
        let padding = size
            .checked_sub(align_size)
            .ok_or_else(|| self.error(span, "union storage alignment exceeds its size"))?;
        let mut fields = vec![align_ty];
        if padding > 0 {
            if padding > u32::MAX as u64 {
                return Err(self.error(span, "union storage padding is too large for LLVM"));
            }
            fields.push(
                self.context
                    .i8_type()
                    .array_type(padding as u32)
                    .map(Into::into)
                    .map_err(Self::diagnostic_from_llvm_error)?,
            );
        }
        self.context
            .struct_type(&fields, false)
            .map(Into::into)
            .map_err(Self::diagnostic_from_llvm_error)
    }

    fn primitive_type(
        &self,
        primitive: PrimitiveTy,
        span: Span,
    ) -> Result<BasicTypeEnum<'ctx>, Diagnostic> {
        let ty: BasicTypeEnum<'ctx> = match primitive {
            PrimitiveTy::I8 | PrimitiveTy::U8 => self.context.i8_type().into(),
            PrimitiveTy::I16 | PrimitiveTy::U16 => self.context.i16_type().into(),
            PrimitiveTy::I32 | PrimitiveTy::U32 | PrimitiveTy::Char => {
                self.context.i32_type().into()
            }
            PrimitiveTy::I64 | PrimitiveTy::U64 => self.context.i64_type().into(),
            PrimitiveTy::I128 | PrimitiveTy::U128 => self.context.i128_type().into(),
            PrimitiveTy::Isize | PrimitiveTy::Usize => {
                self.integer_llvm_type(primitive, span)?.into()
            }
            PrimitiveTy::F32 => self.context.f32_type().into(),
            PrimitiveTy::F64 => self.context.f64_type().into(),
            PrimitiveTy::Bool => self.context.bool_type().into(),
            PrimitiveTy::Never => {
                return Err(self.error(span, "`never` is not an LLVM basic type"));
            }
        };
        Ok(ty)
    }

    fn vector_type(
        &self,
        elem: PrimitiveTy,
        lanes: u32,
        span: Span,
    ) -> Result<BasicTypeEnum<'ctx>, Diagnostic> {
        if !elem.is_vector_element() {
            return Err(self.error(
                span,
                format!("`{}` is not a valid SIMD vector element type", elem.name()),
            ));
        }
        if lanes == 0 {
            return Err(self.error(span, "SIMD vector type requires at least one lane"));
        }
        self.primitive_type(elem, span)?
            .vector_type(lanes)
            .map(Into::into)
            .map_err(Self::diagnostic_from_llvm_error)
    }

    pub(super) fn array_len_in(&self, len: &ArrayLenTy, span: Span) -> Result<u64, Diagnostic> {
        match len {
            ArrayLenTy::ConstValue(value) => Ok(*value),
            ArrayLenTy::ConstExpr(id) => self
                .program
                .module(id.module_id)
                .ok_or_else(|| self.error(span, "missing array length owner module"))?
                .const_eval
                .array_lengths
                .get(id)
                .copied()
                .ok_or_else(|| self.error(span, "array length was not evaluated by const")),
            ArrayLenTy::Builtin { builtin, ty } => {
                let Some(layout) = self.layout_of(*ty) else {
                    return Err(self.error(
                        span,
                        format!("missing layout for `@{}` array length", builtin.name()),
                    ));
                };
                match builtin {
                    LayoutBuiltin::Size => Ok(layout.size),
                    LayoutBuiltin::Align => Ok(layout.align),
                }
            }
            ArrayLenTy::GenericParam(name) => Err(self.error(
                span,
                format!(
                    "array length const generic `{}` reached LLVM lowering",
                    mangle_symbol_id(*name)
                ),
            )),
            ArrayLenTy::Infer => {
                Err(self.error(span, "array length inference reached LLVM lowering"))
            }
        }
    }

    pub(crate) fn array_len(&self, len: &ArrayLenTy, span: Span) -> Result<u64, Diagnostic> {
        self.array_len_in(len, span)
    }

    pub(crate) fn field_index(
        &self,
        base_ty: InternedTyId,
        field: GlobalDefId,
        span: Span,
    ) -> Result<u32, Diagnostic> {
        let Some((def_id, args, const_args)) = self.field_base_type(base_ty) else {
            return Err(self.error(span, "field base type is not nominal"));
        };
        if self
            .layout_of(base_ty)
            .is_some_and(|layout| layout.size == 0)
        {
            return Err(self.error(span, "zero-sized aggregate field has no runtime index"));
        }
        if let Some(layout) = self.struct_layout(def_id, &args, &const_args) {
            if let Some(index) = layout
                .fields
                .iter()
                .filter(|field| field.layout.size != 0)
                .position(|candidate| layout_field_matches(def_id, candidate.def_id, &field))
            {
                return u32::try_from(index)
                    .map_err(|_| self.error(span, "aggregate field index is too large for LLVM"));
            }
            return Err(self.error(span, "missing struct field layout index"));
        }
        if self.union_layout(def_id, &args, &const_args).is_some() {
            return Ok(0);
        }
        Err(self.error(span, "missing aggregate field index"))
    }

    pub(crate) fn field_offset(&self, base_ty: InternedTyId, field: GlobalDefId) -> Option<u64> {
        let (def_id, args, const_args) = self.field_base_type(base_ty)?;
        self.struct_layout(def_id, &args, &const_args)
            .or_else(|| self.union_layout(def_id, &args, &const_args))
            .and_then(|layout| {
                layout
                    .fields
                    .iter()
                    .find(|candidate| layout_field_matches(def_id, candidate.def_id, &field))
                    .map(|field| field.offset)
            })
    }

    fn backend_struct_field(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
        field: GlobalDefId,
        span: Span,
    ) -> Result<&BackendField, Diagnostic> {
        self.struct_fields(def_id, args, const_args, span)?
            .iter()
            .find(|candidate| candidate.def_id == field)
            .ok_or_else(|| self.error(span, "missing struct field"))
    }

    pub(crate) fn field_ty(
        &self,
        base_ty: InternedTyId,
        field: GlobalDefId,
        span: Span,
    ) -> Result<InternedTyId, Diagnostic> {
        let Some((def_id, args, const_args)) = self.field_base_type(base_ty) else {
            return Err(self.error(span, "field base type is not nominal"));
        };
        if let Some(candidate) = self
            .aggregate_fields(def_id, &args, &const_args, span)?
            .iter()
            .find(|candidate| candidate.def_id == field)
        {
            return Ok(candidate.ty);
        }
        Err(self.error(span, "missing aggregate field type"))
    }

    fn field_base_type(
        &self,
        ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>, Vec<nia_ty::ConstGenericArg>)> {
        match self.ty_kind(ty) {
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => Some((*def_id, args.clone(), const_args.clone())),
            Some(TyKind::Pointer { elem, .. }) => self.field_base_type(*elem),
            _ => None,
        }
    }

    pub(super) fn struct_fields(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
        span: Span,
    ) -> Result<&[BackendField], Diagnostic> {
        if let Some(instance) = self.struct_instance_item(def_id, args, const_args) {
            return Ok(&instance.fields);
        }
        if let Some(item) = self.program.struct_item(def_id) {
            return Ok(&item.fields);
        }
        Err(self.error(span, "missing struct fields"))
    }

    pub(super) fn union_fields(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
        span: Span,
    ) -> Result<&[BackendField], Diagnostic> {
        if let Some(instance) = self.union_instance_item(def_id, args, const_args) {
            return Ok(&instance.fields);
        }
        if let Some(item) = self.program.union_item(def_id) {
            return Ok(&item.fields);
        }
        Err(self.error(span, "missing union fields"))
    }

    pub(super) fn aggregate_fields(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
        span: Span,
    ) -> Result<&[BackendField], Diagnostic> {
        self.struct_fields(def_id, args, const_args, span)
            .or_else(|_| self.union_fields(def_id, args, const_args, span))
    }

    pub(crate) fn physical_struct_fields(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
        span: Span,
    ) -> Result<Vec<&BackendField>, Diagnostic> {
        let Some(layout) = self.struct_layout(def_id, args, const_args) else {
            return Err(self.error(span, "missing struct layout"));
        };
        layout
            .fields
            .iter()
            .filter(|field| field.layout.size != 0)
            .map(|field| {
                self.backend_struct_field(
                    def_id,
                    args,
                    const_args,
                    GlobalDefId {
                        module_id: def_id.module_id,
                        def_id: field.def_id,
                    },
                    span,
                )
            })
            .collect()
    }

    pub(crate) fn is_union_def(&self, def_id: GlobalDefId) -> bool {
        self.program.has_union(def_id) || self.program.has_union_instances(def_id)
    }

    pub(super) fn union_storage_fields(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
        span: Span,
    ) -> Result<Vec<BasicTypeEnum<'ctx>>, Diagnostic> {
        let Some(layout) = self.union_layout(def_id, args, const_args) else {
            return Err(self.error(span, "missing union layout"));
        };
        if layout.layout.size == 0 {
            return Ok(Vec::new());
        }
        let align_ty = self.union_alignment_type(layout.layout.align, span)?;
        let align_size = layout.layout.align;
        let padding = layout
            .layout
            .size
            .checked_sub(align_size)
            .ok_or_else(|| self.error(span, "union alignment exceeds its layout size"))?;
        let mut fields = vec![align_ty];
        if padding > 0 {
            if padding > u32::MAX as u64 {
                return Err(self.error(span, "union padding is too large for LLVM"));
            }
            fields.push(
                self.context
                    .i8_type()
                    .array_type(padding as u32)
                    .map(Into::into)
                    .map_err(Self::diagnostic_from_llvm_error)?,
            );
        }
        Ok(fields)
    }

    pub(crate) fn union_alignment_type(
        &self,
        align: u64,
        span: Span,
    ) -> Result<BasicTypeEnum<'ctx>, Diagnostic> {
        match align {
            1 => Ok(self.context.i8_type().into()),
            2 => Ok(self.context.i16_type().into()),
            4 => Ok(self.context.i32_type().into()),
            8 => Ok(self.context.i64_type().into()),
            16 => Ok(self.context.i128_type().into()),
            align if align.is_power_of_two() && align <= u64::from(u32::MAX) => {
                BasicTypeEnum::from(self.context.i8_type())
                    .vector_type(align as u32)
                    .map(Into::into)
                    .map_err(Self::diagnostic_from_llvm_error)
            }
            _ => Err(self.error(span, format!("unsupported union alignment {align}"))),
        }
    }

    fn union_layout(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) -> Option<&nia_layout::StructLayout> {
        self.program.module(def_id.module_id)?;
        if args.is_empty() && const_args.is_empty() {
            self.program.union_layout(def_id)
        } else {
            if let Some(layout) = self.program.union_instance_layout(def_id, args, const_args) {
                return Some(layout);
            }
            let key = (def_id, args.to_vec(), const_args.to_vec());
            if let Some(cached_args) = self.union_layout_lookups.borrow().get(&key).cloned() {
                return cached_args
                    .as_deref()
                    .and_then(|args| self.program.union_instance_layout(def_id, args, const_args));
            }
            let matched_args = self
                .program
                .union_instance_layouts(def_id)
                .find_map(|item| {
                    (self.same_type_args(&item.key.args, args)
                        && self.same_const_args(&item.key.const_args, const_args))
                    .then(|| item.key.args.clone())
                });
            self.union_layout_lookups
                .borrow_mut()
                .insert(key, matched_args.clone());
            matched_args
                .as_deref()
                .and_then(|args| self.program.union_instance_layout(def_id, args, const_args))
        }
    }

    pub(crate) fn struct_layout(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) -> Option<&nia_layout::StructLayout> {
        self.program.module(def_id.module_id)?;
        if args.is_empty() && const_args.is_empty() {
            self.program.struct_layout(def_id)
        } else {
            if let Some(layout) = self
                .program
                .struct_instance_layout(def_id, args, const_args)
            {
                return Some(layout);
            }
            let key = (def_id, args.to_vec(), const_args.to_vec());
            if let Some(cached_args) = self.struct_layout_lookups.borrow().get(&key).cloned() {
                return cached_args.as_deref().and_then(|args| {
                    self.program
                        .struct_instance_layout(def_id, args, const_args)
                });
            }
            let matched_args = self
                .program
                .struct_instance_layouts(def_id)
                .find_map(|item| {
                    (self.same_type_args(&item.key.args, args)
                        && self.same_const_args(&item.key.const_args, const_args))
                    .then(|| item.key.args.clone())
                });
            self.struct_layout_lookups
                .borrow_mut()
                .insert(key, matched_args.clone());
            matched_args.as_deref().and_then(|args| {
                self.program
                    .struct_instance_layout(def_id, args, const_args)
            })
        }
    }

    fn struct_instance_type(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) -> Option<StructType<'ctx>> {
        let key = (def_id, args.to_vec(), const_args.to_vec());
        if let Some(cached) = self.struct_instance_type_lookups.borrow().get(&key) {
            return *cached;
        }
        if let Some(ty) = self
            .struct_instances
            .get(&def_id)
            .and_then(|instances| instances.get(&(args.to_vec(), const_args.to_vec())))
            .copied()
        {
            self.struct_instance_type_lookups
                .borrow_mut()
                .insert(key, Some(ty));
            return Some(ty);
        }
        let ty = self
            .struct_instances_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
            .find_map(|(candidate_args, candidate_const_args, ty)| {
                (self.same_type_args(args, candidate_args)
                    && self.same_const_args(const_args, candidate_const_args))
                .then_some(*ty)
            });
        self.struct_instance_type_lookups
            .borrow_mut()
            .insert(key, ty);
        ty
    }

    fn struct_instance_item(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) -> Option<&BackendStructInstance> {
        if let Some(item) = self.program.struct_instance(def_id, args, const_args) {
            return Some(item);
        }
        self.program.struct_instances_for(def_id).find(|item| {
            self.same_type_args(args, &item.args)
                && self.same_const_args(const_args, &item.const_args)
        })
    }

    fn union_instance_type(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) -> Option<StructType<'ctx>> {
        let key = (def_id, args.to_vec(), const_args.to_vec());
        if let Some(cached) = self.union_instance_type_lookups.borrow().get(&key) {
            return *cached;
        }
        if let Some(ty) = self
            .union_instances
            .get(&def_id)
            .and_then(|instances| instances.get(&(args.to_vec(), const_args.to_vec())))
            .copied()
        {
            self.union_instance_type_lookups
                .borrow_mut()
                .insert(key, Some(ty));
            return Some(ty);
        }
        let ty = self
            .union_instances_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
            .find_map(|(candidate_args, candidate_const_args, ty)| {
                (self.same_type_args(args, candidate_args)
                    && self.same_const_args(const_args, candidate_const_args))
                .then_some(*ty)
            });
        self.union_instance_type_lookups
            .borrow_mut()
            .insert(key, ty);
        ty
    }

    fn union_instance_item(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) -> Option<&BackendUnionInstance> {
        if let Some(item) = self.program.union_instance(def_id, args, const_args) {
            return Some(item);
        }
        self.program.union_instances_for(def_id).find(|item| {
            self.same_type_args(args, &item.args)
                && self.same_const_args(const_args, &item.const_args)
        })
    }

    pub(crate) fn function(&self, def_id: GlobalDefId) -> Option<FunctionValue<'ctx>> {
        self.functions.get(&def_id).copied()
    }

    pub(crate) fn closure_entry_item(
        &self,
        key: &BackendClosureEntryKey,
    ) -> Option<&'a BackendClosureEntry> {
        self.source
            .closure_entries
            .iter()
            .find(|entry| entry.key == *key)
    }

    pub(crate) fn closure_entry_value(
        &self,
        key: &BackendClosureEntryKey,
    ) -> Option<FunctionValue<'ctx>> {
        self.closure_entries.get(key).copied()
    }

    pub(crate) fn function_item(&self, def_id: GlobalDefId) -> Option<&'a BackendFunction> {
        self.program.function(def_id)
    }

    pub(crate) fn function_instance_item_with_arg_module(
        &self,
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        self_arg: Option<InternedTyId>,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) -> Option<&'a BackendFunctionInstance> {
        if let Some(item) =
            self.program
                .function_instance(def_id, arg_module_id, self_arg, args, const_args)
        {
            return Some(item);
        }
        let mut matches = self.program.function_instances_for(def_id).filter(|item| {
            item.arg_module_id == arg_module_id
                && self.same_optional_type(self_arg, item.self_arg)
                && self.same_type_args(args, &item.args)
                && self.same_const_args(const_args, &item.const_args)
        });
        let first = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(first)
    }

    pub(crate) fn function_instance_value(
        &self,
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        self_arg: Option<InternedTyId>,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) -> Option<FunctionValue<'ctx>> {
        let key = nia_function_ir::FunctionInstanceKey {
            def_id,
            arg_module_id,
            self_arg,
            args: args.to_vec(),
            const_args: const_args.to_vec(),
        };
        if let Some(cached) = self.function_instance_value_lookups.borrow().get(&key) {
            return *cached;
        }
        if let Some(value) = self
            .function_instances
            .get(&(def_id, arg_module_id))
            .and_then(|instances| instances.get(&(self_arg, args.to_vec(), const_args.to_vec())))
            .copied()
        {
            self.function_instance_value_lookups
                .borrow_mut()
                .insert(key, Some(value));
            return Some(value);
        }
        let value = self
            .function_instances_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
            .find_map(
                |(
                    candidate_arg_module_id,
                    candidate_self_arg,
                    candidate_args,
                    candidate_const_args,
                    value,
                )| {
                    (*candidate_arg_module_id == arg_module_id
                        && self.same_optional_type(self_arg, *candidate_self_arg)
                        && self.same_type_args(args, candidate_args)
                        && self.same_const_args(candidate_const_args, const_args))
                    .then_some(*value)
                },
            );
        self.function_instance_value_lookups
            .borrow_mut()
            .insert(key, value);
        value
    }

    pub(super) fn same_type_args(&self, left: &[InternedTyId], right: &[InternedTyId]) -> bool {
        self.same_type_args_for_equiv(left, right)
    }

    pub(super) fn same_const_args(
        &self,
        left: &[nia_ty::ConstGenericArg],
        right: &[nia_ty::ConstGenericArg],
    ) -> bool {
        const_args_match_semantic(left, right, |left, right| self.same_type(left, right))
    }

    pub(super) fn same_optional_type(
        &self,
        left: Option<InternedTyId>,
        right: Option<InternedTyId>,
    ) -> bool {
        match (left, right) {
            (Some(left), Some(right)) => self.same_type(left, right),
            (None, None) => true,
            _ => false,
        }
    }

    pub(super) fn same_type(&self, left: InternedTyId, right: InternedTyId) -> bool {
        if left == right {
            return true;
        }
        if let Some(cached) = self.same_type_cache.borrow().get(&(left, right)) {
            return *cached;
        }
        let same = self.compute_same_type_for_equiv(left, right);
        let mut cache = self.same_type_cache.borrow_mut();
        cache.insert((left, right), same);
        cache.insert((right, left), same);
        same
    }

    fn same_array_len(&self, left: &ArrayLenTy, right: &ArrayLenTy) -> bool {
        match (left, right) {
            (ArrayLenTy::Infer, ArrayLenTy::Infer) => true,
            (ArrayLenTy::ConstValue(left), ArrayLenTy::ConstValue(right)) => left == right,
            (ArrayLenTy::ConstValue(left), ArrayLenTy::ConstExpr(right))
            | (ArrayLenTy::ConstExpr(right), ArrayLenTy::ConstValue(left)) => self
                .program
                .module(right.module_id)
                .map(|module| &module.const_eval.array_lengths)
                .unwrap_or(&self.source.const_eval.array_lengths)
                .get(right)
                .is_some_and(|right| left == right),
            (ArrayLenTy::ConstExpr(left), ArrayLenTy::ConstExpr(right)) => {
                left == right || {
                    let left = self
                        .program
                        .module(left.module_id)
                        .and_then(|module| module.const_eval.array_lengths.get(left));
                    let right = self
                        .program
                        .module(right.module_id)
                        .and_then(|module| module.const_eval.array_lengths.get(right));
                    left.is_some() && left == right
                }
            }
            (
                ArrayLenTy::Builtin {
                    builtin: left_builtin,
                    ty: left_ty,
                },
                ArrayLenTy::Builtin {
                    builtin: right_builtin,
                    ty: right_ty,
                },
            ) => left_builtin == right_builtin && self.same_type(*left_ty, *right_ty),
            _ => false,
        }
    }

    pub(crate) fn array_elem_ty(
        &self,
        ty: InternedTyId,
        span: Span,
    ) -> Result<InternedTyId, Diagnostic> {
        match self.ty_kind(ty) {
            Some(TyKind::Array { elem, .. })
            | Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::Slice { elem, .. }) => Ok(*elem),
            _ => Err(self.error(span, "index base is not an array, pointer, or slice")),
        }
    }
}

/// Layout products store field slots with a module-local [`DefId`]. Reattach
/// the aggregate owner before comparing that slot with a program-wide field
/// identity so equal local numbers from different modules cannot alias.
fn layout_field_matches(
    aggregate: GlobalDefId,
    local_field: nia_ids::DefId,
    requested: &GlobalDefId,
) -> bool {
    requested.module_id == aggregate.module_id && requested.def_id == local_field
}

impl TypeEquivalence for ModuleCodegen<'_, '_> {
    fn ty_kind_for_equiv(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.ty_kind(ty)
    }

    fn same_array_len_for_equiv(&self, left: &ArrayLenTy, right: &ArrayLenTy) -> bool {
        self.same_array_len(left, right)
    }

    fn same_type_for_equiv(&self, left: InternedTyId, right: InternedTyId) -> bool {
        self.same_type(left, right)
    }
}

#[cfg(test)]
mod tests {
    use super::layout_field_matches;
    use nia_ids::{DefId, GlobalDefId, ModuleIdAllocator};

    #[test]
    fn layout_field_matching_requires_the_aggregate_module_owner() {
        let mut modules = ModuleIdAllocator::new();
        let aggregate_module = modules.allocate();
        let foreign_module = modules.allocate();
        let aggregate = GlobalDefId {
            module_id: aggregate_module,
            def_id: DefId(10),
        };
        let local_field = DefId(3);

        assert!(layout_field_matches(
            aggregate,
            local_field,
            &GlobalDefId {
                module_id: aggregate_module,
                def_id: local_field,
            }
        ));
        assert!(!layout_field_matches(
            aggregate,
            local_field,
            &GlobalDefId {
                module_id: foreign_module,
                def_id: local_field,
            }
        ));
    }
}
