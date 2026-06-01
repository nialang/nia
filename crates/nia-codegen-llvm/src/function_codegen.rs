// SPDX-License-Identifier: GPL-3.0-or-later
mod aggregate;
mod asm;
mod call;
mod defer;
mod function_body;
mod literals;
mod ops;
mod place;

use std::collections::HashMap;

use crate::module_codegen::{AbiParam, AbiReturn, ModuleCodegen};
use defer::DeferScope;
use nia_ast::BinaryOp;
use nia_backend_ir::BackendFunction;
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    FunctionBody, FunctionBuiltinValue, FunctionExpr, FunctionExprKind, FunctionLocal,
    FunctionLocalKind, FunctionRange, FunctionScopeId,
};
use nia_ids::{GlobalDefId, InternedTyId, LocalId};
use nia_llvm::{
    builder::Builder,
    values::{BasicValueEnum, FunctionValue, PointerValue},
};
use nia_span::Span;
use nia_ty::{LayoutBuiltin, PrimitiveTy, TyKind};

pub(super) struct FunctionCodegen<'m, 'ctx, 'a> {
    module: &'m ModuleCodegen<'ctx, 'a>,
    builder: Builder<'ctx>,
    function: &'a BackendFunction,
    llvm_function: FunctionValue<'ctx>,
    locals: HashMap<LocalId, PointerValue<'ctx>>,
    local_tys: HashMap<LocalId, InternedTyId>,
    out_ptr: Option<PointerValue<'ctx>>,
    defer_scopes: Vec<DeferScope>,
    function_defer_scopes: HashMap<FunctionScopeId, usize>,
    active_function_scope: Option<FunctionScopeId>,
}

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn new(
        module: &'m ModuleCodegen<'ctx, 'a>,
        function: &'a BackendFunction,
        llvm_function: FunctionValue<'ctx>,
    ) -> Self {
        Self {
            module,
            builder: module.context.create_builder(),
            function,
            llvm_function,
            locals: HashMap::new(),
            local_tys: HashMap::new(),
            out_ptr: None,
            defer_scopes: Vec::new(),
            function_defer_scopes: HashMap::new(),
            active_function_scope: None,
        }
    }

    fn alloc_function_locals(&mut self, body: &FunctionBody) -> Result<(), Diagnostic> {
        self.alloc_local_list(&body.locals)
    }

    fn alloc_local_list(&mut self, locals: &[FunctionLocal]) -> Result<(), Diagnostic> {
        for local in locals {
            if matches!(
                local.kind,
                FunctionLocalKind::Param
                    | FunctionLocalKind::Binding
                    | FunctionLocalKind::ConstBinding
            ) {
                if self.is_zero_sized(local.ty) {
                    self.local_tys.insert(local.id, local.ty);
                    continue;
                }
                let ty = self.module.llvm_basic_type(local.ty, local.span)?;
                let ptr = self
                    .builder
                    .build_alloca(ty, &local.name)
                    .map_err(|_| self.error(local.span, "failed to build local alloca"))?;
                self.locals.insert(local.id, ptr);
                self.local_tys.insert(local.id, local.ty);
            }
        }
        Ok(())
    }

    fn store_params(&mut self) -> Result<(), Diagnostic> {
        let classifications = self.module.classify_function_params(
            &self
                .function
                .params
                .iter()
                .map(|param| param.ty)
                .collect::<Vec<_>>(),
        );
        let mut llvm_index = usize::from(matches!(
            self.module
                .classify_function_return(self.function.return_type),
            AbiReturn::IndirectOut(_)
        ));
        for (param, classification) in self.function.params.iter().zip(classifications) {
            let Some(local_id) = param.local_id else {
                if !matches!(classification, AbiParam::Omit) {
                    llvm_index += 1;
                }
                continue;
            };
            let Some(ptr) = self.locals.get(&local_id).copied() else {
                if !matches!(classification, AbiParam::Omit) {
                    llvm_index += 1;
                }
                continue;
            };
            match classification {
                AbiParam::Direct(_) => {
                    let Some(value) = self.llvm_function.get_nth_param(llvm_index as u32) else {
                        return Err(self.error(param.span, "missing LLVM function parameter"));
                    };
                    let value = value?;
                    self.builder.build_store(ptr, value).map_err(|_| {
                        self.error(param.span, "failed to store function parameter")
                    })?;
                    llvm_index += 1;
                }
                AbiParam::IndirectReadonly(ty) => {
                    let Some(value) = self.llvm_function.get_nth_param(llvm_index as u32) else {
                        return Err(self.error(param.span, "missing LLVM function parameter"));
                    };
                    let value = value?;
                    let loaded_ty = self.module.llvm_basic_type(ty, param.span)?;
                    let loaded = self
                        .builder
                        .build_load(loaded_ty, value.into_pointer_value()?, "param.copy")
                        .map_err(|_| self.error(param.span, "failed to load indirect parameter"))?;
                    self.builder.build_store(ptr, loaded).map_err(|_| {
                        self.error(param.span, "failed to store function parameter")
                    })?;
                    llvm_index += 1;
                }
                AbiParam::Omit => {}
            };
        }
        Ok(())
    }

    fn function_out_ptr(&self) -> Result<Option<PointerValue<'ctx>>, Diagnostic> {
        if !matches!(
            self.module
                .classify_function_return(self.function.return_type),
            AbiReturn::IndirectOut(_)
        ) {
            return Ok(None);
        }
        let Some(value) = self.llvm_function.get_nth_param(0) else {
            return Err(self.error(self.function.span, "missing aggregate return pointer"));
        };
        Ok(Some(value?.into_pointer_value()?))
    }

    pub(super) fn emit_return_value(
        &mut self,
        span: Span,
        value: BasicValueEnum<'ctx>,
    ) -> Result<(), Diagnostic> {
        match self
            .module
            .classify_function_return(self.function.return_type)
        {
            AbiReturn::Direct(_) => self
                .builder
                .build_return(Some(&value))
                .map_err(|_| self.error(span, "failed to build return"))
                .map(|_| ())?,
            AbiReturn::IndirectOut(_) => {
                let Some(out_ptr) = self.out_ptr else {
                    return Err(self.error(span, "missing aggregate return pointer"));
                };
                self.builder
                    .build_store(out_ptr, value)
                    .map_err(|_| self.error(span, "failed to store aggregate return"))?;
                self.builder
                    .build_return(None)
                    .map_err(|_| self.error(span, "failed to build aggregate return"))?;
            }
            AbiReturn::Void => self
                .builder
                .build_return(None)
                .map_err(|_| self.error(span, "failed to build void return"))
                .map(|_| ())?,
            AbiReturn::Never => self
                .builder
                .build_unreachable()
                .map_err(|_| self.error(span, "failed to build never return"))
                .map(|_| ())?,
        }
        Ok(())
    }

    fn emit_expr(&mut self, expr: &FunctionExpr) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        match &expr.kind {
            FunctionExprKind::Integer(text) => self.emit_integer_literal(expr.ty, expr.span, text),
            FunctionExprKind::Float(text) => self.emit_float_literal(expr.ty, expr.span, text),
            FunctionExprKind::String(scalars) => {
                self.emit_string_literal(expr.ty, expr.span, scalars)
            }
            FunctionExprKind::ByteString(bytes) => {
                self.emit_byte_string_literal(expr.ty, expr.span, bytes)
            }
            FunctionExprKind::Char(value) => self.emit_char_literal(expr.ty, expr.span, *value),
            FunctionExprKind::ByteChar(text) => {
                self.emit_byte_char_literal(expr.ty, expr.span, text)
            }
            FunctionExprKind::Bool(value) => Ok(self
                .module
                .context
                .bool_type()
                .const_int(u64::from(*value), false)
                .into()),
            FunctionExprKind::Local(local_id) => {
                if self.is_zero_sized(expr.ty) {
                    return Err(self.error(expr.span, "zero-sized local has no runtime value"));
                }
                let Some(ptr) = self.locals.get(local_id).copied() else {
                    return Err(self.error(expr.span, "missing local storage"));
                };
                let ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
                self.builder
                    .build_load(ty, ptr, "loadtmp")
                    .map_err(|_| self.error(expr.span, "failed to load local"))
            }
            FunctionExprKind::Global(def_id) => self
                .module
                .globals
                .get(def_id)
                .copied()
                .ok_or_else(|| self.error(expr.span, "missing global value"))
                .and_then(|global| {
                    if self.is_zero_sized(expr.ty) {
                        return Err(self.error(expr.span, "zero-sized global has no runtime value"));
                    }
                    let Some(global_info) = self.module.program.globals.get(def_id).copied() else {
                        return Err(self.error(expr.span, "missing global metadata"));
                    };
                    let Some(owner) = self.module.program.module(def_id.module_id) else {
                        return Err(self.error(expr.span, "missing global owner module"));
                    };
                    let ty = self.module.llvm_basic_type_in(
                        global_info.ty,
                        expr.span,
                        &owner.interner,
                        &owner.layouts,
                    )?;
                    self.builder
                        .build_load(ty, global.as_pointer_value(), "loadglobal")
                        .map_err(|_| self.error(expr.span, "failed to load global"))
                }),
            FunctionExprKind::BuiltinValue(FunctionBuiltinValue::Usize(value)) => Ok(self
                .module
                .context
                .i64_type()
                .const_int(*value, false)
                .into()),
            FunctionExprKind::BuiltinValue(FunctionBuiltinValue::Layout { builtin, ty }) => {
                let Some(layout) = self.module.layout_of(*ty) else {
                    return Err(self.error(expr.span, "layout builtin type has no known layout"));
                };
                let value = match builtin {
                    LayoutBuiltin::Size => layout.size,
                    LayoutBuiltin::Align => layout.align,
                };
                Ok(self
                    .module
                    .context
                    .i64_type()
                    .const_int(value, false)
                    .into())
            }
            FunctionExprKind::BuiltinValue(FunctionBuiltinValue::Int(value)) => {
                let ty = self
                    .module
                    .llvm_basic_type(expr.ty, expr.span)?
                    .into_int_type()?;
                Ok(ty.const_u128(*value as u128).into())
            }
            FunctionExprKind::Range(range) => self.emit_range(expr.span, expr.ty, range),
            FunctionExprKind::InlineAsm(asm) => {
                self.emit_inline_asm(asm)?;
                Err(self.error(expr.span, "inline assembly does not produce a value"))
            }
            FunctionExprKind::CStringPointer { array, .. } => {
                self.emit_c_string_pointer(expr.span, array)
            }
            FunctionExprKind::ArrayLiteral { elems } => self.emit_array_literal(expr, elems),
            FunctionExprKind::StructLiteral { def_id, fields } => {
                self.emit_struct_literal(expr, *def_id, fields)
            }
            FunctionExprKind::UnionLiteral { def_id, field } => {
                self.emit_union_literal(expr, *def_id, field)
            }
            FunctionExprKind::EnumVariant(def_id) => self.emit_enum_variant(expr, *def_id),
            FunctionExprKind::Binary { lhs, op, rhs } => {
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    return self.emit_short_circuit(expr.span, lhs, *op, rhs);
                }
                let lhs = self.emit_expr(lhs)?;
                let rhs = self.emit_expr(rhs)?;
                self.emit_binary(expr.span, expr.ty, lhs, *op, rhs)
            }
            FunctionExprKind::Unary { op, expr: inner } => {
                self.emit_unary(expr.span, expr.ty, *op, inner)
            }
            FunctionExprKind::AddrOf(place) => Ok(self.emit_typed_place_addr(place)?.into()),
            FunctionExprKind::Cast { expr: inner, ty } => {
                let value = self.emit_expr(inner)?;
                self.emit_cast(expr.span, inner.ty, *ty, value)
            }
            FunctionExprKind::TraitObjectUpcast {
                expr: inner,
                source_ty,
                target_ty,
            } => self.emit_trait_object_upcast(expr.span, inner, *source_ty, *target_ty),
            FunctionExprKind::TraitObjectCoercion {
                expr: inner,
                target_ty,
                self_ty,
            } => self.emit_trait_object_coercion(expr.span, inner, *self_ty, *target_ty),
            FunctionExprKind::Call { callee, args } => self.emit_call(expr, callee, args),
            FunctionExprKind::Slice { lhs, range, .. } => self.emit_slice(expr.span, lhs, range),
            FunctionExprKind::Field { .. } | FunctionExprKind::Index { .. } => {
                let ptr = self.emit_addr_of(expr)?;
                let ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
                self.builder
                    .build_load(ty, ptr, "loadtmp")
                    .map_err(|_| self.error(expr.span, "failed to load place"))
            }
            FunctionExprKind::Function(_) | FunctionExprKind::FunctionInstance { .. } => Err(self
                .error(
                    expr.span,
                    "function item cannot be emitted as a runtime value",
                )),
            FunctionExprKind::Assign { .. } => {
                Err(self.error(expr.span, "assignment expression cannot be used as a value"))
            }
            FunctionExprKind::Discard(_) => {
                Err(self.error(expr.span, "discard expression cannot be used as a value"))
            }
            FunctionExprKind::Error => {
                Err(self.error(expr.span, "cannot emit erroneous expression"))
            }
        }
    }

    fn emit_trait_object_coercion(
        &mut self,
        span: Span,
        inner: &FunctionExpr,
        self_ty: InternedTyId,
        target_ty: InternedTyId,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let object_ptr = self.emit_expr(inner)?.into_pointer_value()?;
        let Some(vtable) = self.module.trait_object_vtable(self_ty, target_ty) else {
            return Err(self.error(span, "missing trait object vtable"));
        };
        let metadata = vtable.as_pointer_value();
        let trait_object_ty = self.module.trait_object_type();
        let result = trait_object_ty.get_undef();
        let result = self
            .builder
            .build_insert_value(result, object_ptr, 0, "traitobj.ptr")
            .map_err(|_| self.error(span, "failed to build trait object"))?
            .into_struct_value()
            .map_err(|_| self.error(span, "failed to build trait object"))?;
        let result = self
            .builder
            .build_insert_value(result, metadata, 1, "traitobj.vtable")
            .map_err(|_| self.error(span, "failed to build trait object"))?;
        Ok(result)
    }

    fn emit_trait_object_upcast(
        &mut self,
        span: Span,
        inner: &FunctionExpr,
        source_ty: InternedTyId,
        target_ty: InternedTyId,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let value = self.emit_expr(inner)?;
        let value = value
            .into_struct_value()
            .map_err(|_| self.error(span, "trait object upcast source is not a trait object"))?;
        let trait_object_ty = self.module.trait_object_type();
        let object_ptr = self
            .builder
            .build_extract_value(value, 0, "traitobj.ptr")
            .map_err(|_| self.error(span, "failed to extract trait object pointer"))?;
        let mut metadata = self
            .builder
            .build_extract_value(value, 1, "traitobj.metadata")
            .map_err(|_| self.error(span, "failed to extract trait object metadata"))?;
        let offset = self
            .module
            .trait_object_upcast_slot_offset(source_ty, target_ty);
        if offset > 0 {
            let ptr_ty = self.module.context.ptr_type(Default::default());
            let zero = self.module.context.i64_type().const_int(0, false);
            let offset_index = self
                .module
                .context
                .i64_type()
                .const_int(offset as u64, false);
            metadata = unsafe {
                self.builder
                    .build_gep(
                        ptr_ty.array_type((offset + 1) as u32),
                        metadata.into_pointer_value()?,
                        &[zero, offset_index],
                        "traitobj.upcast.metadata.offset",
                    )
                    .map_err(|_| self.error(span, "failed to offset trait object metadata"))?
                    .into()
            };
        }
        let result = trait_object_ty.get_undef();
        let result = self
            .builder
            .build_insert_value(result, object_ptr, 0, "traitobj.upcast.ptr")
            .map_err(|_| self.error(span, "failed to build trait object upcast"))?
            .into_struct_value()
            .map_err(|_| self.error(span, "failed to build trait object upcast"))?;
        let result = self
            .builder
            .build_insert_value(result, metadata, 1, "traitobj.upcast.metadata")
            .map_err(|_| self.error(span, "failed to build trait object upcast"))?;
        Ok(result)
    }

    fn emit_range(
        &mut self,
        span: Span,
        ty: InternedTyId,
        range: &FunctionRange,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(TyKind::Range { kind, bound }) = self.module.ty_kind(ty) else {
            return Err(self.error(span, "range expression target type is not a range"));
        };
        let Some(bound) = bound else {
            return self
                .module
                .llvm_basic_type(ty, span)?
                .const_zero()
                .map_err(Into::into);
        };
        let mut value = self
            .module
            .range_type(*kind, Some(*bound), span)?
            .into_struct_type()?
            .get_undef();
        let mut index = 0u32;
        if let Some(start) = &range.start {
            let start_span = start.span;
            let start = self.emit_expr(start)?;
            value = self
                .builder
                .build_insert_value(value, start, index, "range.start")
                .map_err(|_| self.error(start_span, "failed to insert range start"))?
                .into_struct_value()?;
            index += 1;
        }
        if let Some(end) = &range.end {
            let end_span = end.span;
            let end = self.emit_expr(end)?;
            value = self
                .builder
                .build_insert_value(value, end, index, "range.end")
                .map_err(|_| self.error(end_span, "failed to insert range end"))?
                .into_struct_value()?;
        }
        Ok(value.into())
    }

    fn field_index(
        &self,
        base_ty: InternedTyId,
        field: GlobalDefId,
        span: Span,
    ) -> Result<u32, Diagnostic> {
        self.module.field_index(base_ty, field, span)
    }

    fn field_ty(
        &self,
        base_ty: InternedTyId,
        field: GlobalDefId,
        span: Span,
    ) -> Result<InternedTyId, Diagnostic> {
        self.module.field_ty(base_ty, field, span)
    }

    fn array_elem_ty(&self, ty: InternedTyId, span: Span) -> Result<InternedTyId, Diagnostic> {
        self.module.array_elem_ty(ty, span)
    }

    fn same_llvm_type(
        &self,
        lhs: InternedTyId,
        rhs: InternedTyId,
        span: Span,
    ) -> Result<bool, Diagnostic> {
        Ok(self.module.llvm_basic_type(lhs, span)? == self.module.llvm_basic_type(rhs, span)?)
    }

    fn is_zero_sized(&self, ty: InternedTyId) -> bool {
        self.module
            .layout_of(ty)
            .is_some_and(|layout| layout.size == 0)
    }

    fn is_integer_like(&self, ty: InternedTyId) -> bool {
        self.is_integer(ty) || self.is_enum(ty)
    }

    fn is_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.module.ty_kind(ty),
            Some(TyKind::Primitive(
                PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize
                    | PrimitiveTy::U8
                    | PrimitiveTy::U16
                    | PrimitiveTy::U32
                    | PrimitiveTy::U64
                    | PrimitiveTy::U128
                    | PrimitiveTy::Usize
                    | PrimitiveTy::Bool
                    | PrimitiveTy::Char
            ))
        )
    }

    fn is_float(&self, ty: InternedTyId) -> bool {
        matches!(
            self.module.ty_kind(ty),
            Some(TyKind::Primitive(PrimitiveTy::F32 | PrimitiveTy::F64))
        )
    }

    fn is_pointer_like(&self, ty: InternedTyId) -> bool {
        matches!(
            self.module.ty_kind(ty),
            Some(TyKind::Pointer { .. } | TyKind::FunctionPointer { .. })
        )
    }

    fn is_pointer_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.module.ty_kind(ty),
            Some(TyKind::Primitive(PrimitiveTy::Usize | PrimitiveTy::Isize))
        )
    }

    fn is_enum(&self, ty: InternedTyId) -> bool {
        matches!(
            self.module.ty_kind(ty),
            Some(TyKind::Nominal { def_id, .. }) if self.module.program.enums.contains_key(def_id)
        )
    }

    fn integer_bits(&self, ty: InternedTyId, span: Span) -> Result<u32, Diagnostic> {
        if let Some(TyKind::Nominal { def_id, .. }) = self.module.ty_kind(ty)
            && let Some(item) = self.module.program.enums.get(def_id).copied()
        {
            return self.integer_bits(item.backing_type, span);
        }
        let Some(TyKind::Primitive(primitive)) = self.module.ty_kind(ty) else {
            return Err(self.error(span, "expected integer type"));
        };
        Ok(match primitive {
            PrimitiveTy::Bool => 1,
            PrimitiveTy::I8 | PrimitiveTy::U8 => 8,
            PrimitiveTy::I16 | PrimitiveTy::U16 => 16,
            PrimitiveTy::I32 | PrimitiveTy::U32 | PrimitiveTy::Char => 32,
            PrimitiveTy::I64 | PrimitiveTy::U64 | PrimitiveTy::Isize | PrimitiveTy::Usize => 64,
            PrimitiveTy::I128 | PrimitiveTy::U128 => 128,
            PrimitiveTy::F32 | PrimitiveTy::F64 | PrimitiveTy::Void | PrimitiveTy::Never => {
                return Err(self.error(span, "expected integer type"));
            }
        })
    }

    fn integer_llvm_type(
        &self,
        primitive: PrimitiveTy,
        span: Span,
    ) -> Result<nia_llvm::types::IntType<'ctx>, Diagnostic> {
        match primitive {
            PrimitiveTy::I8 | PrimitiveTy::U8 => Ok(self.module.context.i8_type()),
            PrimitiveTy::I16 | PrimitiveTy::U16 => Ok(self.module.context.i16_type()),
            PrimitiveTy::I32 | PrimitiveTy::U32 | PrimitiveTy::Char => {
                Ok(self.module.context.i32_type())
            }
            PrimitiveTy::I64 | PrimitiveTy::U64 | PrimitiveTy::Isize | PrimitiveTy::Usize => {
                Ok(self.module.context.i64_type())
            }
            PrimitiveTy::I128 | PrimitiveTy::U128 => Ok(self.module.context.i128_type()),
            PrimitiveTy::Bool => Ok(self.module.context.bool_type()),
            PrimitiveTy::F32 | PrimitiveTy::F64 | PrimitiveTy::Void | PrimitiveTy::Never => {
                Err(self.error(span, "expected integer primitive type"))
            }
        }
    }

    fn is_signed_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.module.ty_kind(ty),
            Some(TyKind::Primitive(
                PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize
                    | PrimitiveTy::Char
            ))
        )
    }

    fn is_void(&self, ty: InternedTyId) -> bool {
        matches!(
            self.module.ty_kind(ty),
            Some(TyKind::Primitive(PrimitiveTy::Void))
        )
    }

    fn is_never(&self, ty: InternedTyId) -> bool {
        matches!(
            self.module.ty_kind(ty),
            Some(TyKind::Primitive(PrimitiveTy::Never))
        )
    }

    fn current_block_has_terminator(&self) -> bool {
        self.builder
            .get_insert_block()
            .is_some_and(|block| block.get_terminator().is_some())
    }

    fn error(&self, span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(span, message)
    }
}
