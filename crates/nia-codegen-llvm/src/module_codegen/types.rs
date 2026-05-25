// SPDX-License-Identifier: GPL-3.0-or-later
use super::ModuleCodegen;
use nia_backend_ir::{
    BackendField, BackendFunction, BackendFunctionInstance, BackendLayouts, BackendStructInstance,
};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, TyId};
use nia_llvm::{
    types::{BasicMetadataTypeEnum, BasicTypeEnum, FunctionType, StructType},
    values::FunctionValue,
};
use nia_span::Span;
use nia_ty::{ArrayLenTy, PrimitiveTy, TyInterner, TyKind};

impl<'ctx, 'a> ModuleCodegen<'ctx, 'a> {
    pub(super) fn function_type_in(
        &self,
        function: &BackendFunction,
        interner: &TyInterner,
        layouts: &BackendLayouts,
    ) -> Result<FunctionType<'ctx>, Diagnostic> {
        let mut params = Vec::<BasicMetadataTypeEnum<'ctx>>::new();
        for param in &function.params {
            params.push(self.llvm_basic_type_in(param.ty, param.span, interner, layouts)?);
        }
        match interner.get(function.return_type) {
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
        for param in params {
            llvm_params.push(self.llvm_basic_type_in(*param, span, interner, layouts)?);
        }
        match interner.get(return_type) {
            Some(TyKind::Primitive(PrimitiveTy::Void | PrimitiveTy::Never)) => {
                Ok(self.context.void_type().fn_type(&llvm_params, is_variadic))
            }
            _ => Ok(self
                .llvm_basic_type_in(return_type, span, interner, layouts)?
                .fn_type(&llvm_params, is_variadic)),
        }
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
        interner: &TyInterner,
        layouts: &BackendLayouts,
    ) -> Result<BasicTypeEnum<'ctx>, Diagnostic> {
        match interner.get(ty) {
            Some(TyKind::Primitive(primitive)) => self.primitive_type(*primitive, span),
            Some(TyKind::Pointer { .. } | TyKind::FunctionPointer { .. }) => {
                Ok(self.context.ptr_type(Default::default()).into())
            }
            Some(TyKind::Slice { .. }) => Ok(self.slice_type().into()),
            Some(TyKind::Array { len, elem }) => {
                let elem = self.llvm_basic_type_in(*elem, span, interner, layouts)?;
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
                if let Some(item) = self.program.enums.get(def_id).copied() {
                    return self.llvm_basic_type_in(
                        item.backing_type,
                        item.span,
                        interner,
                        layouts,
                    );
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
            ArrayLenTy::ConstExpr(text) => nia_const_eval::eval_array_len_text(text)
                .map_err(|err| self.error(span, format!("invalid array length: {}", err.message))),
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
        let fields = self.struct_fields(def_id, &args, span)?;
        if let Some(index) = fields
            .iter()
            .position(|candidate| candidate.def_id == field)
        {
            return Ok(index as u32);
        }
        Err(self.error(span, "missing struct field index"))
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
            .struct_fields(def_id, &args, span)?
            .iter()
            .find(|candidate| candidate.def_id == field)
        {
            return Ok(candidate.ty);
        }
        Err(self.error(span, "missing struct field type"))
    }

    fn field_base_type(&self, ty: TyId) -> Option<(GlobalDefId, Vec<TyId>)> {
        match self.interner().get(ty) {
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

    pub(crate) fn function(&self, def_id: GlobalDefId) -> Option<FunctionValue<'ctx>> {
        self.functions.get(&def_id).copied()
    }

    pub(crate) fn function_instance_item(
        &self,
        def_id: GlobalDefId,
        args: &[TyId],
    ) -> Option<&'a BackendFunctionInstance> {
        self.program.function_instances.iter().find_map(
            |((candidate_def, candidate_args), item)| {
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
            .find_map(|((candidate_def, candidate_args), value)| {
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
        self.interner().get(left) == self.interner().get(right)
    }

    pub(crate) fn array_elem_ty(&self, ty: TyId, span: Span) -> Result<TyId, Diagnostic> {
        match self.interner().get(ty) {
            Some(TyKind::Array { elem, .. })
            | Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::Slice { elem, .. }) => Ok(*elem),
            _ => Err(self.error(span, "index base is not an array, pointer, or slice")),
        }
    }
}
