// SPDX-License-Identifier: GPL-3.0-or-later
use super::ModuleCodegen;
use crate::literals::parse_float_literal;
use nia_backend_ir::BackendLayouts;
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId};
use nia_llvm::{types::BasicTypeEnum, values::BasicValueEnum};
use nia_span::Span;
use nia_static_ir::{StaticAddressElem, StaticFieldInit, StaticInit};
use nia_ty::{PrimitiveTy, TyInterner, TyKind};

impl<'ctx, 'a> ModuleCodegen<'ctx, 'a> {
    pub(super) fn static_init_value_in(
        &self,
        ty: InternedTyId,
        init: &StaticInit,
        span: Span,
        interner: &TyInterner,
        layouts: &BackendLayouts,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        match init {
            StaticInit::Zero => Ok(self
                .llvm_basic_type_in(ty, span, interner, layouts)?
                .const_zero()?),
            StaticInit::Int(value) => {
                let Some(TyKind::Primitive(primitive)) = self.ty_kind(ty) else {
                    return Err(
                        self.error(span, "integer static initializer target is not primitive")
                    );
                };
                let int_ty = self.integer_llvm_type(*primitive, span)?;
                Ok(int_ty.const_u128(*value as u128).into())
            }
            StaticInit::Bool(value) => Ok(self
                .context
                .bool_type()
                .const_int(u64::from(*value), false)
                .into()),
            StaticInit::Byte(value) => Ok(self
                .context
                .i8_type()
                .const_int(*value as u64, false)
                .into()),
            StaticInit::Chars(scalars) => {
                self.static_chars_init_value_in(interner, ty, scalars, span)
            }
            StaticInit::Bytes(bytes) => Ok(self.context.const_string(bytes, true).into()),
            StaticInit::Float(text) => self.static_float_init_value(ty, text, span),
            StaticInit::Char(value) => self.static_char_init_value_in(interner, ty, *value, span),
            StaticInit::Array(elems) => {
                self.static_array_init_value_in(interner, layouts, ty, elems, span)
            }
            StaticInit::Repeat { value, count } => {
                self.static_repeat_init_value_in(interner, layouts, ty, value, *count, span)
            }
            StaticInit::Struct(fields) => {
                self.static_struct_init_value_in(interner, layouts, ty, fields, span)
            }
            StaticInit::NullPtr => match self.llvm_basic_type_in(ty, span, interner, layouts)? {
                BasicTypeEnum::PointerType(ptr_ty) => Ok(ptr_ty.const_null().into()),
                _ => Err(self.error(
                    span,
                    "null pointer static initializer target is not pointer",
                )),
            },
            StaticInit::AddrOfGlobal { global, path } => {
                self.static_addr_of_global_value(ty, *global, path, span, interner, layouts)
            }
            StaticInit::AddrOfFunction { function, args } => {
                self.static_addr_of_function_value(ty, *function, args, span, interner, layouts)
            }
        }
    }

    pub(crate) fn static_init_value_in_current(
        &self,
        ty: InternedTyId,
        init: &StaticInit,
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        self.static_init_value_in(ty, init, span, self.interner(), &self.source.layouts)
    }

    fn static_addr_of_global_value(
        &self,
        ty: InternedTyId,
        global: GlobalDefId,
        path: &[StaticAddressElem],
        span: Span,
        target_interner: &TyInterner,
        target_layouts: &BackendLayouts,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(global_value) = self.globals.get(&global).copied() else {
            return Err(self.error(span, "missing global for static address initializer"));
        };
        let Some(owner) = self.program.module(global.module_id) else {
            return Err(self.error(span, "missing global owner module"));
        };
        let Some(global_ty) = self.program.globals.get(&global).map(|item| item.ty) else {
            return Err(self.error(span, "missing global type for static address initializer"));
        };
        let mut ptr = global_value.as_pointer_value();
        let mut current_ty = global_ty;
        if !path.is_empty() {
            let mut indices = vec![self.context.i64_type().const_int(0, false)];
            for elem in path {
                match elem {
                    StaticAddressElem::Field(field) => {
                        indices.push(
                            self.context.i32_type().const_int(
                                self.field_index(current_ty, *field, span)? as u64,
                                false,
                            ),
                        );
                        current_ty = self.field_ty(current_ty, *field, span)?;
                    }
                    StaticAddressElem::Index(index) => {
                        indices.push(self.context.i64_type().const_int(*index, false));
                        current_ty = self.array_elem_ty(current_ty, span)?;
                    }
                    StaticAddressElem::Error => {
                        return Err(self.error(span, "invalid static address path"));
                    }
                }
            }
            let pointee_ty =
                self.llvm_basic_type_in(global_ty, span, &owner.interner, &owner.layouts)?;
            unsafe {
                ptr = ptr.const_in_bounds_gep(pointee_ty, &indices);
            }
        }
        let target_ptr_ty = self
            .llvm_basic_type_in(ty, span, target_interner, target_layouts)?
            .into_pointer_type()?;
        Ok(ptr.const_bitcast(target_ptr_ty).into())
    }

    fn static_addr_of_function_value(
        &self,
        ty: InternedTyId,
        function: GlobalDefId,
        args: &[InternedTyId],
        span: Span,
        target_interner: &TyInterner,
        target_layouts: &BackendLayouts,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let function = if args.is_empty() {
            self.function(function)
        } else {
            self.function_instance_value(function, args)
        }
        .ok_or_else(|| self.error(span, "missing function for static address initializer"))?;
        let target_ptr_ty = self
            .llvm_basic_type_in(ty, span, target_interner, target_layouts)?
            .into_pointer_type()?;
        Ok(function
            .as_global_value()
            .as_pointer_value()
            .const_bitcast(target_ptr_ty)
            .into())
    }

    fn static_float_init_value(
        &self,
        ty: InternedTyId,
        text: &str,
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let value = parse_float_literal(text)
            .ok_or_else(|| self.error(span, format!("invalid float literal `{text}`")))?;
        match self.ty_kind(ty) {
            Some(TyKind::Primitive(PrimitiveTy::F32)) => {
                Ok(self.context.f32_type().const_float(value).into())
            }
            Some(TyKind::Primitive(PrimitiveTy::F64)) => {
                Ok(self.context.f64_type().const_float(value).into())
            }
            _ => Err(self.error(span, "float static initializer target is not float")),
        }
    }

    fn static_char_init_value_in(
        &self,
        _interner: &TyInterner,
        ty: InternedTyId,
        value: u32,
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(TyKind::Primitive(primitive)) = self.ty_kind(ty) else {
            return Err(self.error(span, "char static initializer target is not primitive"));
        };
        let int_ty = self.integer_llvm_type(*primitive, span)?;
        Ok(int_ty.const_u128(value as u128).into())
    }

    fn static_chars_init_value_in(
        &self,
        interner: &TyInterner,
        ty: InternedTyId,
        scalars: &[u32],
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(TyKind::Array { elem, .. }) = interner.get(ty) else {
            return Err(self.error(
                span,
                "char string static initializer target is not an array",
            ));
        };
        let values = scalars
            .iter()
            .map(|scalar| self.static_char_init_value_in(interner, *elem, *scalar, span))
            .collect::<Result<Vec<_>, _>>()?;
        self.const_array_from_values_in(*elem, &values, span, interner, self.layouts_for(*elem))
    }

    fn static_array_init_value_in(
        &self,
        interner: &TyInterner,
        layouts: &BackendLayouts,
        ty: InternedTyId,
        elems: &[StaticInit],
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(TyKind::Array { elem, .. }) = self.ty_kind(ty) else {
            return Err(self.error(span, "array static initializer target is not array"));
        };
        let values = elems
            .iter()
            .map(|elem_init| self.static_init_value_in(*elem, elem_init, span, interner, layouts))
            .collect::<Result<Vec<_>, _>>()?;
        self.const_array_from_values_in(*elem, &values, span, interner, layouts)
    }

    fn static_repeat_init_value_in(
        &self,
        interner: &TyInterner,
        layouts: &BackendLayouts,
        ty: InternedTyId,
        value: &StaticInit,
        count: u64,
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(TyKind::Array { elem, .. }) = self.ty_kind(ty) else {
            return Err(self.error(span, "repeat static initializer target is not array"));
        };
        let value = self.static_init_value_in(*elem, value, span, interner, layouts)?;
        let values = std::iter::repeat_n(value, count as usize).collect::<Vec<_>>();
        self.const_array_from_values_in(*elem, &values, span, interner, layouts)
    }

    fn static_struct_init_value_in(
        &self,
        interner: &TyInterner,
        layouts: &BackendLayouts,
        ty: InternedTyId,
        fields: &[StaticFieldInit],
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let struct_ty = self
            .llvm_basic_type_in(ty, span, interner, layouts)?
            .into_struct_type()?;
        let Some(TyKind::Nominal { def_id, args }) = self.ty_kind(ty) else {
            return Err(self.error(span, "struct static initializer target is not nominal"));
        };
        if self.is_union_def(*def_id) {
            let Some(init) = fields.first() else {
                return Err(self.error(span, "missing union static field initializer"));
            };
            let Some(field_id) = init.field else {
                return Err(self.error(span, "invalid union static field initializer"));
            };
            let field_ty = self.field_ty(ty, field_id, span)?;
            let value =
                self.static_init_value_in(field_ty, &init.value, span, interner, layouts)?;
            let mut values = vec![value];
            for index in 1..struct_ty.count_fields() {
                let Some(field_ty) = struct_ty.get_field_type_at_index(index) else {
                    continue;
                };
                values.push(field_ty?.const_zero()?);
            }
            return Ok(struct_ty.const_named_struct(&values).into());
        }
        let struct_fields = self.physical_struct_fields(*def_id, args, span)?;
        let values = struct_fields
            .iter()
            .map(|field| {
                let init = fields
                    .iter()
                    .find(|init| init.field == Some(field.def_id))
                    .ok_or_else(|| self.error(field.span, "missing static field initializer"))?;
                self.static_init_value_in(field.ty, &init.value, field.span, interner, layouts)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(struct_ty.const_named_struct(&values).into())
    }

    pub(super) fn const_array_from_values_in(
        &self,
        elem_ty: InternedTyId,
        values: &[BasicValueEnum<'ctx>],
        span: Span,
        interner: &TyInterner,
        layouts: &BackendLayouts,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        match self.llvm_basic_type_in(elem_ty, span, interner, layouts)? {
            BasicTypeEnum::IntType(ty) => Ok(ty
                .const_array(
                    &values
                        .iter()
                        .map(|value| value.into_int_value())
                        .collect::<Result<Vec<_>, _>>()?,
                )
                .into()),
            BasicTypeEnum::FloatType(ty) => Ok(ty
                .const_array(
                    &values
                        .iter()
                        .map(|value| value.into_float_value())
                        .collect::<Result<Vec<_>, _>>()?,
                )
                .into()),
            BasicTypeEnum::PointerType(ty) => Ok(ty
                .const_array(
                    &values
                        .iter()
                        .map(|value| value.into_pointer_value())
                        .collect::<Result<Vec<_>, _>>()?,
                )
                .into()),
            BasicTypeEnum::StructType(ty) => Ok(ty
                .const_array(
                    &values
                        .iter()
                        .map(|value| value.into_struct_value())
                        .collect::<Result<Vec<_>, _>>()?,
                )
                .into()),
            BasicTypeEnum::ArrayType(ty) => Ok(ty
                .const_array(
                    &values
                        .iter()
                        .map(|value| value.into_array_value())
                        .collect::<Result<Vec<_>, _>>()?,
                )
                .into()),
            _ => Err(self.error(
                span,
                "array static initializer element type is not supported",
            )),
        }
    }

    pub(crate) fn const_array_from_values_in_current(
        &self,
        elem_ty: InternedTyId,
        values: &[BasicValueEnum<'ctx>],
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        self.const_array_from_values_in(
            elem_ty,
            values,
            span,
            self.interner(),
            self.layouts_for(elem_ty),
        )
    }
}
