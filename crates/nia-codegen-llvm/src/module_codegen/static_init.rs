// SPDX-License-Identifier: GPL-3.0-or-later
use super::ModuleCodegen;
use crate::literals::{decode_char_literal, parse_float_literal, parse_int_literal};
use nia_backend_ir::{
    BackendLayouts, BuiltinConst, PlaceElem, StaticFieldInit, StaticInit, TypedExpr, TypedExprKind,
};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, TyId};
use nia_llvm::{types::BasicTypeEnum, values::BasicValueEnum};
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyInterner, TyKind};

impl<'ctx, 'a> ModuleCodegen<'ctx, 'a> {
    pub(super) fn static_init_value_in(
        &self,
        ty: TyId,
        init: &StaticInit,
        span: Span,
        interner: &TyInterner,
        layouts: &BackendLayouts,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        match init {
            StaticInit::Zero => Ok(self
                .llvm_basic_type_in(ty, span, interner, layouts)?
                .const_zero()),
            StaticInit::Int(value) => {
                let Some(TyKind::Primitive(primitive)) = interner.get(ty) else {
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
            StaticInit::Bytes(bytes) => Ok(self.context.const_string(bytes, true).into()),
            StaticInit::Float(text) => self.static_float_init_value(ty, text, span),
            StaticInit::Char(text) => self.static_char_init_value_in(interner, ty, text, span),
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
            StaticInit::AddrOfFunction(function) => {
                self.static_addr_of_function_value(ty, *function, span, interner, layouts)
            }
        }
    }

    pub(crate) fn static_init_value_in_current(
        &self,
        ty: TyId,
        init: &StaticInit,
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        self.static_init_value_in(ty, init, span, self.interner(), &self.source.layouts)
    }

    fn static_addr_of_global_value(
        &self,
        ty: TyId,
        global: GlobalDefId,
        path: &[PlaceElem],
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
                    PlaceElem::Field(field) => {
                        indices.push(
                            self.context.i32_type().const_int(
                                self.field_index(current_ty, *field, span)? as u64,
                                false,
                            ),
                        );
                        current_ty = self.field_ty(current_ty, *field, span)?;
                    }
                    PlaceElem::Index(index) => {
                        let const_index = self.const_index_value(index, span)?;
                        indices.push(self.context.i64_type().const_int(const_index, false));
                        current_ty = self.array_elem_ty(current_ty, span)?;
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
            .into_pointer_type();
        Ok(ptr.const_bitcast(target_ptr_ty).into())
    }

    fn static_addr_of_function_value(
        &self,
        ty: TyId,
        function: GlobalDefId,
        span: Span,
        target_interner: &TyInterner,
        target_layouts: &BackendLayouts,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let function = self
            .function(function)
            .ok_or_else(|| self.error(span, "missing function for static address initializer"))?;
        let target_ptr_ty = self
            .llvm_basic_type_in(ty, span, target_interner, target_layouts)?
            .into_pointer_type();
        Ok(function
            .as_global_value()
            .as_pointer_value()
            .const_bitcast(target_ptr_ty)
            .into())
    }

    fn static_float_init_value(
        &self,
        ty: TyId,
        text: &str,
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let value = parse_float_literal(text)
            .ok_or_else(|| self.error(span, format!("invalid float literal `{text}`")))?;
        match self.interner().get(ty) {
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
        interner: &TyInterner,
        ty: TyId,
        text: &str,
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let value = decode_char_literal(text)
            .ok_or_else(|| self.error(span, format!("invalid char literal `{text}`")))?;
        let Some(TyKind::Primitive(primitive)) = interner.get(ty) else {
            return Err(self.error(span, "char static initializer target is not primitive"));
        };
        let int_ty = self.integer_llvm_type(*primitive, span)?;
        Ok(int_ty.const_u128(value as u128).into())
    }

    fn static_array_init_value_in(
        &self,
        interner: &TyInterner,
        layouts: &BackendLayouts,
        ty: TyId,
        elems: &[StaticInit],
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(TyKind::Array { elem, .. }) = interner.get(ty) else {
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
        ty: TyId,
        value: &StaticInit,
        count: u64,
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(TyKind::Array { elem, .. }) = interner.get(ty) else {
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
        ty: TyId,
        fields: &[StaticFieldInit],
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let struct_ty = self
            .llvm_basic_type_in(ty, span, interner, layouts)?
            .into_struct_type();
        let Some(TyKind::Nominal { def_id, args }) = interner.get(ty) else {
            return Err(self.error(span, "struct static initializer target is not nominal"));
        };
        let struct_fields = self.struct_fields(*def_id, args, span)?;
        let values = struct_fields
            .iter()
            .enumerate()
            .map(|(index, field)| {
                let init = fields
                    .get(index)
                    .ok_or_else(|| self.error(field.span, "missing static field initializer"))?;
                self.static_init_value_in(field.ty, &init.value, field.span, interner, layouts)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(struct_ty.const_named_struct(&values).into())
    }

    fn const_index_value(&self, expr: &TypedExpr, span: Span) -> Result<u64, Diagnostic> {
        match &expr.kind {
            TypedExprKind::Integer(text) => parse_int_literal(text)
                .and_then(|value| u64::try_from(value).ok())
                .ok_or_else(|| self.error(span, "static address index is not a valid usize")),
            TypedExprKind::BuiltinValue(BuiltinConst::Usize(value)) => Ok(*value),
            _ => Err(self.error(span, "static address index is not a supported constant")),
        }
    }

    pub(super) fn const_array_from_values_in(
        &self,
        elem_ty: TyId,
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
                        .collect::<Vec<_>>(),
                )
                .into()),
            BasicTypeEnum::FloatType(ty) => Ok(ty
                .const_array(
                    &values
                        .iter()
                        .map(|value| value.into_float_value())
                        .collect::<Vec<_>>(),
                )
                .into()),
            BasicTypeEnum::PointerType(ty) => Ok(ty
                .const_array(
                    &values
                        .iter()
                        .map(|value| value.into_pointer_value())
                        .collect::<Vec<_>>(),
                )
                .into()),
            BasicTypeEnum::StructType(ty) => Ok(ty
                .const_array(
                    &values
                        .iter()
                        .map(|value| value.into_struct_value())
                        .collect::<Vec<_>>(),
                )
                .into()),
            BasicTypeEnum::ArrayType(ty) => Ok(ty
                .const_array(
                    &values
                        .iter()
                        .map(|value| value.into_array_value())
                        .collect::<Vec<_>>(),
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
        elem_ty: TyId,
        values: &[BasicValueEnum<'ctx>],
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        self.const_array_from_values_in(
            elem_ty,
            values,
            span,
            self.interner(),
            &self.source.layouts,
        )
    }
}
