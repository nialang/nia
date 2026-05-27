// SPDX-License-Identifier: GPL-3.0-or-later
use super::ModuleCodegen;
use nia_backend_ir::{
    BackendField, BackendFunction, BackendFunctionInstance, BackendLayouts, BackendStructInstance,
    BackendStructInstanceKey, BackendUnionInstance,
};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, ModuleId, TyId};
use nia_llvm::{
    types::{BasicMetadataTypeEnum, BasicTypeEnum, FunctionType, StructType},
    values::FunctionValue,
};
use nia_span::Span;
use nia_ty::{ArrayLenTy, PrimitiveTy, TyInterner, TyKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AbiParam {
    Direct(TyId),
    Omit,
    IndirectReadonly(TyId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AbiReturn {
    Direct(TyId),
    Void,
    IndirectOut(TyId),
    Never,
}

impl<'ctx, 'a> ModuleCodegen<'ctx, 'a> {
    fn module_interner(&self, module_id: ModuleId) -> Option<&'a TyInterner> {
        self.program
            .module(module_id)
            .map(|module| &module.interner)
    }

    pub(crate) fn ty_kind(&self, ty: TyId) -> Option<&'a TyKind> {
        self.module_interner(ty.module_id)?.get(ty)
    }

    pub(super) fn function_type_in(
        &self,
        function: &BackendFunction,
        interner: &TyInterner,
        layouts: &BackendLayouts,
    ) -> Result<FunctionType<'ctx>, Diagnostic> {
        if function.is_extern {
            return self.c_function_type_in(function, interner, layouts);
        }
        let mut params = Vec::<BasicMetadataTypeEnum<'ctx>>::new();
        if let AbiReturn::IndirectOut(ty) =
            self.classify_return_in(function.return_type, interner, layouts)
        {
            params.push(self.pointer_abi_type(ty, function.span, interner, layouts)?);
        }
        for param in self.classify_params_in(
            function.params.iter().map(|param| param.ty),
            interner,
            layouts,
        ) {
            match param {
                AbiParam::Direct(ty) => {
                    params.push(self.llvm_basic_type_in(ty, function.span, interner, layouts)?);
                }
                AbiParam::IndirectReadonly(ty) => {
                    params.push(self.pointer_abi_type(ty, function.span, interner, layouts)?);
                }
                AbiParam::Omit => {}
            }
        }
        match self.classify_return_in(function.return_type, interner, layouts) {
            AbiReturn::Direct(ty) => Ok(self
                .llvm_basic_type_in(ty, function.span, interner, layouts)?
                .fn_type(&params, function.is_variadic)),
            AbiReturn::Void | AbiReturn::IndirectOut(_) | AbiReturn::Never => Ok(self
                .context
                .void_type()
                .fn_type(&params, function.is_variadic)),
        }
    }

    fn c_function_type_in(
        &self,
        function: &BackendFunction,
        interner: &TyInterner,
        layouts: &BackendLayouts,
    ) -> Result<FunctionType<'ctx>, Diagnostic> {
        let mut params = Vec::<BasicMetadataTypeEnum<'ctx>>::new();
        for param in &function.params {
            params.push(self.llvm_basic_type_in(param.ty, param.span, interner, layouts)?);
        }
        match self.ty_kind(function.return_type) {
            Some(TyKind::Primitive(PrimitiveTy::Void | PrimitiveTy::Never)) => Ok(self
                .context
                .void_type()
                .fn_type(&params, function.is_variadic)),
            _ => Ok(self
                .llvm_basic_type_in(function.return_type, function.span, interner, layouts)?
                .fn_type(&params, function.is_variadic)),
        }
    }

    pub(crate) fn function_pointer_type_in(
        &self,
        interner: &TyInterner,
        layouts: &BackendLayouts,
        params: &[TyId],
        return_type: TyId,
        is_variadic: bool,
        span: Span,
    ) -> Result<FunctionType<'ctx>, Diagnostic> {
        let mut llvm_params = Vec::<BasicMetadataTypeEnum<'ctx>>::new();
        if let AbiReturn::IndirectOut(ty) = self.classify_return_in(return_type, interner, layouts)
        {
            llvm_params.push(self.pointer_abi_type(ty, span, interner, layouts)?);
        }
        for param in self.classify_params_in(params.iter().copied(), interner, layouts) {
            match param {
                AbiParam::Direct(ty) => {
                    llvm_params.push(self.llvm_basic_type_in(ty, span, interner, layouts)?);
                }
                AbiParam::IndirectReadonly(ty) => {
                    llvm_params.push(self.pointer_abi_type(ty, span, interner, layouts)?);
                }
                AbiParam::Omit => {}
            }
        }
        match self.classify_return_in(return_type, interner, layouts) {
            AbiReturn::Direct(ty) => Ok(self
                .llvm_basic_type_in(ty, span, interner, layouts)?
                .fn_type(&llvm_params, is_variadic)),
            AbiReturn::Void | AbiReturn::IndirectOut(_) | AbiReturn::Never => {
                Ok(self.context.void_type().fn_type(&llvm_params, is_variadic))
            }
        }
    }

    pub(crate) fn classify_function_params(&self, params: &[TyId]) -> Vec<AbiParam> {
        self.classify_params_in(
            params.iter().copied(),
            self.interner(),
            &self.source.layouts,
        )
    }

    pub(crate) fn classify_function_return(&self, ty: TyId) -> AbiReturn {
        self.classify_return_in(ty, self.interner(), &self.source.layouts)
    }

    fn classify_params_in(
        &self,
        params: impl IntoIterator<Item = TyId>,
        interner: &TyInterner,
        layouts: &BackendLayouts,
    ) -> Vec<AbiParam> {
        params
            .into_iter()
            .map(|ty| self.classify_param_in(ty, interner, layouts))
            .collect()
    }

    fn classify_param_in(
        &self,
        ty: TyId,
        _interner: &TyInterner,
        layouts: &BackendLayouts,
    ) -> AbiParam {
        if self
            .layout_of_in(ty, layouts)
            .is_some_and(|layout| layout.size == 0)
        {
            return AbiParam::Omit;
        }
        match self.ty_kind(ty) {
            Some(
                TyKind::Primitive(_)
                | TyKind::Pointer { .. }
                | TyKind::FunctionPointer { .. }
                | TyKind::Slice { .. },
            ) => AbiParam::Direct(ty),
            Some(TyKind::Nominal { def_id, .. }) if self.program.enums.contains_key(def_id) => {
                AbiParam::Direct(ty)
            }
            Some(TyKind::Array { .. } | TyKind::Nominal { .. }) => AbiParam::IndirectReadonly(ty),
            Some(TyKind::GenericParam(_) | TyKind::Error) | None => AbiParam::Direct(ty),
        }
    }

    fn classify_return_in(
        &self,
        ty: TyId,
        _interner: &TyInterner,
        layouts: &BackendLayouts,
    ) -> AbiReturn {
        match self.ty_kind(ty) {
            Some(TyKind::Primitive(PrimitiveTy::Never)) => return AbiReturn::Never,
            Some(TyKind::Primitive(PrimitiveTy::Void)) => return AbiReturn::Void,
            _ => {}
        }
        if self
            .layout_of_in(ty, layouts)
            .is_some_and(|layout| layout.size == 0)
        {
            return AbiReturn::Void;
        }
        match self.ty_kind(ty) {
            Some(
                TyKind::Primitive(_)
                | TyKind::Pointer { .. }
                | TyKind::FunctionPointer { .. }
                | TyKind::Slice { .. },
            ) => AbiReturn::Direct(ty),
            Some(TyKind::Nominal { def_id, .. }) if self.program.enums.contains_key(def_id) => {
                AbiReturn::Direct(ty)
            }
            Some(TyKind::Array { .. } | TyKind::Nominal { .. }) => AbiReturn::IndirectOut(ty),
            Some(TyKind::GenericParam(_) | TyKind::Error) | None => AbiReturn::Direct(ty),
        }
    }

    fn pointer_abi_type(
        &self,
        _ty: TyId,
        _span: Span,
        _interner: &TyInterner,
        _layouts: &BackendLayouts,
    ) -> Result<BasicTypeEnum<'ctx>, Diagnostic> {
        Ok(self.context.ptr_type(Default::default()).into())
    }

    pub(crate) fn llvm_basic_type(
        &self,
        ty: TyId,
        span: Span,
    ) -> Result<BasicTypeEnum<'ctx>, Diagnostic> {
        self.llvm_basic_type_in(ty, span, self.interner(), &self.source.layouts)
    }

    pub(crate) fn slice_type(&self) -> StructType<'ctx> {
        self.context.struct_type(
            &[
                self.context.ptr_type(Default::default()).into(),
                self.context.i64_type().into(),
            ],
            false,
        )
    }

    pub(crate) fn llvm_basic_type_in(
        &self,
        ty: TyId,
        span: Span,
        _interner: &TyInterner,
        layouts: &BackendLayouts,
    ) -> Result<BasicTypeEnum<'ctx>, Diagnostic> {
        if self
            .layout_of_in(ty, layouts)
            .is_some_and(|layout| layout.size == 0)
            && !matches!(
                self.ty_kind(ty),
                Some(TyKind::Pointer { .. } | TyKind::FunctionPointer { .. })
            )
        {
            return Ok(self.context.struct_type(&[], false).into());
        }
        match self.ty_kind(ty) {
            Some(TyKind::Primitive(primitive)) => self.primitive_type(*primitive, span),
            Some(TyKind::Pointer { .. } | TyKind::FunctionPointer { .. }) => {
                Ok(self.context.ptr_type(Default::default()).into())
            }
            Some(TyKind::Slice { .. }) => Ok(self.slice_type().into()),
            Some(TyKind::Array { len, elem }) => {
                let elem = self.llvm_basic_type_in(*elem, span, self.interner(), layouts)?;
                let len = self.array_len_in(len, span, layouts)?;
                if len > u32::MAX as u64 {
                    return Err(
                        self.error(span, format!("array length {len} is too large for LLVM"))
                    );
                }
                Ok(elem.array_type(len as u32).into())
            }
            Some(TyKind::Nominal { def_id, args }) => {
                if let Some(struct_ty) = self.struct_instance_type(*def_id, args) {
                    return Ok(struct_ty.into());
                }
                if let Some(struct_ty) = self.structs.get(def_id).copied() {
                    return Ok(struct_ty.into());
                }
                if let Some(union_ty) = self.union_instance_type(*def_id, args) {
                    return Ok(union_ty.into());
                }
                if let Some(union_ty) = self.unions.get(def_id).copied() {
                    return Ok(union_ty.into());
                }
                if let Some(item) = self.program.enums.get(def_id).copied() {
                    return self.llvm_basic_type(item.backing_type, item.span);
                }
                Err(self.error(span, "unknown nominal type during LLVM lowering"))
            }
            Some(TyKind::GenericParam(_) | TyKind::Error) | None => {
                Err(self.error(span, "type is not concrete enough for LLVM lowering"))
            }
        }
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
            PrimitiveTy::Isize | PrimitiveTy::Usize => self.context.i64_type().into(),
            PrimitiveTy::F32 => self.context.f32_type().into(),
            PrimitiveTy::F64 => self.context.f64_type().into(),
            PrimitiveTy::Bool => self.context.bool_type().into(),
            PrimitiveTy::Void | PrimitiveTy::Never => {
                return Err(self.error(span, "`void` and `!` are not LLVM basic types"));
            }
        };
        Ok(ty)
    }

    fn array_len_in(
        &self,
        len: &ArrayLenTy,
        span: Span,
        layouts: &BackendLayouts,
    ) -> Result<u64, Diagnostic> {
        match len {
            ArrayLenTy::ConstExpr { text, span } => self
                .source
                .comptime
                .array_lengths
                .get(span)
                .copied()
                .or_else(|| nia_comptime_engine::eval_array_len_text(text).ok())
                .ok_or_else(|| {
                    self.error(
                        *span,
                        format!("array length `{text}` was not evaluated by comptime"),
                    )
                }),
            ArrayLenTy::Builtin { name, ty } => {
                let Some(layout) = layouts
                    .types
                    .iter()
                    .find_map(|(layout_ty, layout)| (*layout_ty == *ty).then_some(layout))
                else {
                    return Err(
                        self.error(span, format!("missing layout for `@{name}` array length"))
                    );
                };
                match name.as_str() {
                    "size" => Ok(layout.size),
                    "align" => Ok(layout.align),
                    _ => {
                        Err(self.error(span, format!("unsupported array length builtin `@{name}`")))
                    }
                }
            }
            ArrayLenTy::Infer => {
                Err(self.error(span, "array length inference reached LLVM lowering"))
            }
        }
    }

    pub(crate) fn array_len(&self, len: &ArrayLenTy, span: Span) -> Result<u64, Diagnostic> {
        self.array_len_in(len, span, &self.source.layouts)
    }

    pub(crate) fn field_index(
        &self,
        base_ty: TyId,
        field: GlobalDefId,
        span: Span,
    ) -> Result<u32, Diagnostic> {
        let Some((def_id, args)) = self.field_base_type(base_ty) else {
            return Err(self.error(span, "field base type is not nominal"));
        };
        if self
            .layout_of(base_ty)
            .is_some_and(|layout| layout.size == 0)
        {
            return Err(self.error(span, "zero-sized aggregate field has no runtime index"));
        }
        if let Some(layout) = self.struct_layout(def_id, &args) {
            if let Some(index) = layout
                .fields
                .iter()
                .filter(|field| field.layout.size != 0)
                .position(|candidate| candidate.def_id == field.def_id)
            {
                return Ok(index as u32);
            }
            return Err(self.error(span, "missing struct field layout index"));
        }
        if self.union_layout(def_id, &args).is_some() {
            return Ok(0);
        }
        Err(self.error(span, "missing aggregate field index"))
    }

    fn backend_struct_field(
        &self,
        def_id: GlobalDefId,
        args: &[TyId],
        field: GlobalDefId,
        span: Span,
    ) -> Result<&BackendField, Diagnostic> {
        self.struct_fields(def_id, args, span)?
            .iter()
            .find(|candidate| candidate.def_id == field)
            .ok_or_else(|| self.error(span, "missing struct field"))
    }

    fn layout_of_in(&self, ty: TyId, layouts: &BackendLayouts) -> Option<nia_layout::TypeLayout> {
        layouts
            .types
            .iter()
            .find_map(|(candidate, layout)| (*candidate == ty).then_some(layout.clone()))
    }

    pub(crate) fn field_ty(
        &self,
        base_ty: TyId,
        field: GlobalDefId,
        span: Span,
    ) -> Result<TyId, Diagnostic> {
        let Some((def_id, args)) = self.field_base_type(base_ty) else {
            return Err(self.error(span, "field base type is not nominal"));
        };
        if let Some(candidate) = self
            .aggregate_fields(def_id, &args, span)?
            .iter()
            .find(|candidate| candidate.def_id == field)
        {
            return Ok(candidate.ty);
        }
        Err(self.error(span, "missing aggregate field type"))
    }

    fn field_base_type(&self, ty: TyId) -> Option<(GlobalDefId, Vec<TyId>)> {
        match self.ty_kind(ty) {
            Some(TyKind::Nominal { def_id, args }) => Some((*def_id, args.clone())),
            Some(TyKind::Pointer { elem, .. }) => self.field_base_type(*elem),
            _ => None,
        }
    }

    pub(super) fn struct_fields(
        &self,
        def_id: GlobalDefId,
        args: &[TyId],
        span: Span,
    ) -> Result<&[BackendField], Diagnostic> {
        if let Some(instance) = self.struct_instance_item(def_id, args) {
            return Ok(&instance.fields);
        }
        if let Some(item) = self.program.structs.get(&def_id) {
            return Ok(&item.fields);
        }
        Err(self.error(span, "missing struct fields"))
    }

    pub(super) fn union_fields(
        &self,
        def_id: GlobalDefId,
        args: &[TyId],
        span: Span,
    ) -> Result<&[BackendField], Diagnostic> {
        if let Some(instance) = self.union_instance_item(def_id, args) {
            return Ok(&instance.fields);
        }
        if let Some(item) = self.program.unions.get(&def_id) {
            return Ok(&item.fields);
        }
        Err(self.error(span, "missing union fields"))
    }

    pub(super) fn aggregate_fields(
        &self,
        def_id: GlobalDefId,
        args: &[TyId],
        span: Span,
    ) -> Result<&[BackendField], Diagnostic> {
        self.struct_fields(def_id, args, span)
            .or_else(|_| self.union_fields(def_id, args, span))
    }

    pub(super) fn physical_struct_fields(
        &self,
        def_id: GlobalDefId,
        args: &[TyId],
        span: Span,
    ) -> Result<Vec<&BackendField>, Diagnostic> {
        let Some(layout) = self.struct_layout(def_id, args) else {
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
        self.program.unions.contains_key(&def_id)
            || self
                .program
                .union_instances
                .keys()
                .any(|(candidate, _)| *candidate == def_id)
    }

    pub(super) fn union_storage_fields(
        &self,
        def_id: GlobalDefId,
        args: &[TyId],
        span: Span,
    ) -> Result<Vec<BasicTypeEnum<'ctx>>, Diagnostic> {
        let Some(layout) = self.union_layout(def_id, args) else {
            return Err(self.error(span, "missing union layout"));
        };
        let align_ty = self.union_alignment_type(layout.layout.align, span)?;
        let align_size = layout.layout.align;
        let padding = layout.layout.size.saturating_sub(align_size);
        let mut fields = vec![align_ty];
        if padding > 0 {
            if padding > u32::MAX as u64 {
                return Err(self.error(span, "union padding is too large for LLVM"));
            }
            fields.push(self.context.i8_type().array_type(padding as u32).into());
        }
        Ok(fields)
    }

    fn union_alignment_type(
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
            _ => Err(self.error(span, format!("unsupported union alignment {align}"))),
        }
    }

    fn union_layout(
        &self,
        def_id: GlobalDefId,
        args: &[TyId],
    ) -> Option<&nia_layout::StructLayout> {
        if args.is_empty() {
            self.source
                .layouts
                .unions
                .iter()
                .find_map(|(candidate, layout)| (*candidate == def_id).then_some(layout))
        } else {
            self.source
                .layouts
                .union_instances
                .iter()
                .find_map(|(key, layout)| {
                    (key.def_id == def_id && self.same_type_args(&key.args, args)).then_some(layout)
                })
        }
    }

    fn struct_layout(
        &self,
        def_id: GlobalDefId,
        args: &[TyId],
    ) -> Option<&nia_layout::StructLayout> {
        let owner = self.program.module(def_id.module_id)?;
        if args.is_empty() {
            owner
                .layouts
                .structs
                .iter()
                .find_map(|(candidate, layout)| (*candidate == def_id).then_some(layout))
        } else {
            owner
                .layouts
                .struct_instances
                .iter()
                .find_map(|(key, layout)| {
                    (*key
                        == BackendStructInstanceKey {
                            def_id,
                            args: args.to_vec(),
                        })
                    .then_some(layout)
                })
        }
    }

    fn struct_instance_type(&self, def_id: GlobalDefId, args: &[TyId]) -> Option<StructType<'ctx>> {
        self.struct_instances
            .iter()
            .find_map(|((candidate_def, candidate_args), ty)| {
                (*candidate_def == def_id && self.same_type_args(args, candidate_args))
                    .then_some(*ty)
            })
    }

    fn struct_instance_item(
        &self,
        def_id: GlobalDefId,
        args: &[TyId],
    ) -> Option<&BackendStructInstance> {
        self.program
            .struct_instances
            .iter()
            .find_map(|((candidate_def, candidate_args), item)| {
                (*candidate_def == def_id && self.same_type_args(args, candidate_args))
                    .then_some(*item)
            })
    }

    fn union_instance_type(&self, def_id: GlobalDefId, args: &[TyId]) -> Option<StructType<'ctx>> {
        self.union_instances
            .iter()
            .find_map(|((candidate_def, candidate_args), ty)| {
                (*candidate_def == def_id && self.same_type_args(args, candidate_args))
                    .then_some(*ty)
            })
    }

    fn union_instance_item(
        &self,
        def_id: GlobalDefId,
        args: &[TyId],
    ) -> Option<&BackendUnionInstance> {
        self.program
            .union_instances
            .iter()
            .find_map(|((candidate_def, candidate_args), item)| {
                (*candidate_def == def_id && self.same_type_args(args, candidate_args))
                    .then_some(*item)
            })
    }

    pub(crate) fn function(&self, def_id: GlobalDefId) -> Option<FunctionValue<'ctx>> {
        self.functions.get(&def_id).copied()
    }

    pub(crate) fn function_item(&self, def_id: GlobalDefId) -> Option<&'a BackendFunction> {
        self.program.functions.get(&def_id).copied()
    }

    pub(crate) fn function_instance_item(
        &self,
        def_id: GlobalDefId,
        args: &[TyId],
    ) -> Option<&'a BackendFunctionInstance> {
        self.program.function_instances.iter().find_map(
            |((candidate_def, _, candidate_args), item)| {
                (*candidate_def == def_id && self.same_type_args(args, candidate_args))
                    .then_some(*item)
            },
        )
    }

    pub(crate) fn function_instance_value(
        &self,
        def_id: GlobalDefId,
        args: &[TyId],
    ) -> Option<FunctionValue<'ctx>> {
        self.function_instances
            .iter()
            .find_map(|((candidate_def, _, candidate_args), value)| {
                (*candidate_def == def_id && self.same_type_args(args, candidate_args))
                    .then_some(*value)
            })
    }

    fn same_type_args(&self, left: &[TyId], right: &[TyId]) -> bool {
        left.len() == right.len()
            && left
                .iter()
                .zip(right)
                .all(|(left, right)| self.same_type(*left, *right))
    }

    fn same_type(&self, left: TyId, right: TyId) -> bool {
        if left == right {
            return true;
        }
        match (self.ty_kind(left), self.ty_kind(right)) {
            (Some(TyKind::Error), Some(TyKind::Error)) => true,
            (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
            (Some(TyKind::GenericParam(left)), Some(TyKind::GenericParam(right))) => left == right,
            (
                Some(TyKind::Pointer {
                    is_const: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::Pointer {
                    is_const: right_const,
                    elem: right_elem,
                }),
            )
            | (
                Some(TyKind::Slice {
                    is_const: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::Slice {
                    is_const: right_const,
                    elem: right_elem,
                }),
            ) => left_const == right_const && self.same_type(*left_elem, *right_elem),
            (
                Some(TyKind::Array {
                    len: left_len,
                    elem: left_elem,
                }),
                Some(TyKind::Array {
                    len: right_len,
                    elem: right_elem,
                }),
            ) => {
                self.same_array_len(left_len, right_len) && self.same_type(*left_elem, *right_elem)
            }
            (
                Some(TyKind::FunctionPointer {
                    params: left_params,
                    return_type: left_return,
                    is_variadic: left_variadic,
                }),
                Some(TyKind::FunctionPointer {
                    params: right_params,
                    return_type: right_return,
                    is_variadic: right_variadic,
                }),
            ) => {
                left_variadic == right_variadic
                    && self.same_type_args(left_params, right_params)
                    && self.same_type(*left_return, *right_return)
            }
            (
                Some(TyKind::Nominal {
                    def_id: left_def,
                    args: left_args,
                }),
                Some(TyKind::Nominal {
                    def_id: right_def,
                    args: right_args,
                }),
            ) => left_def == right_def && self.same_type_args(left_args, right_args),
            _ => false,
        }
    }

    fn same_array_len(&self, left: &ArrayLenTy, right: &ArrayLenTy) -> bool {
        match (left, right) {
            (ArrayLenTy::Infer, ArrayLenTy::Infer) => true,
            (
                ArrayLenTy::ConstExpr { text: left, .. },
                ArrayLenTy::ConstExpr { text: right, .. },
            ) => left == right,
            (
                ArrayLenTy::Builtin {
                    name: left_name,
                    ty: left_ty,
                },
                ArrayLenTy::Builtin {
                    name: right_name,
                    ty: right_ty,
                },
            ) => left_name == right_name && self.same_type(*left_ty, *right_ty),
            _ => false,
        }
    }

    pub(crate) fn array_elem_ty(&self, ty: TyId, span: Span) -> Result<TyId, Diagnostic> {
        match self.ty_kind(ty) {
            Some(TyKind::Array { elem, .. })
            | Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::Slice { elem, .. }) => Ok(*elem),
            _ => Err(self.error(span, "index base is not an array, pointer, or slice")),
        }
    }
}
