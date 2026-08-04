// SPDX-License-Identifier: GPL-3.0-or-later
mod aggregate;
mod asm;
mod atomic;
mod call;
mod defer;
mod function_body;
mod literals;
mod memory;
mod ops;
mod place;

use std::collections::HashMap;

use crate::module_codegen::{AbiParam, AbiReturn, ModuleCodegen};
use defer::DeferScope;
use nia_ast::BinaryOp;
use nia_backend_ir::{BackendFunction, BackendParam};
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    FunctionBitIntrinsicOp, FunctionBody, FunctionBuiltinValue, FunctionCallee,
    FunctionErrorUnionTag, FunctionExpr, FunctionExprKind, FunctionLocal, FunctionLocalKind,
    FunctionRange, FunctionScopeId,
};
use nia_ids::{GlobalDefId, InternedTyId, LocalId};
use nia_llvm::{
    IntPredicate,
    builder::Builder,
    types::BasicTypeEnum,
    values::{BasicValueEnum, FunctionValue, PointerValue},
};
use nia_span::Span;
use nia_ty::{LayoutBuiltin, PrimitiveTy, TyKind};

pub(super) struct FunctionCodegen<'m, 'ctx, 'a> {
    module: &'m ModuleCodegen<'ctx, 'a>,
    builder: Builder<'ctx>,
    function: FunctionCodegenInput<'a>,
    llvm_function: FunctionValue<'ctx>,
    locals: HashMap<LocalId, PointerValue<'ctx>>,
    local_tys: HashMap<LocalId, InternedTyId>,
    zst_locals: HashMap<LocalId, PointerValue<'ctx>>,
    out_ptr: Option<PointerValue<'ctx>>,
    defer_scopes: Vec<DeferScope>,
    function_defer_scopes: HashMap<FunctionScopeId, usize>,
    active_function_scope: Option<FunctionScopeId>,
}

#[derive(Clone, Copy)]
pub(super) struct FunctionCodegenInput<'a> {
    pub(super) params: &'a [BackendParam],
    pub(super) return_type: InternedTyId,
    pub(super) local_names: &'a HashMap<LocalId, String>,
    pub(super) span: Span,
}

impl<'a> From<&'a BackendFunction> for FunctionCodegenInput<'a> {
    fn from(function: &'a BackendFunction) -> Self {
        Self {
            params: &function.params,
            return_type: function.return_type,
            local_names: &function.local_names,
            span: function.span,
        }
    }
}

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn new(
        module: &'m ModuleCodegen<'ctx, 'a>,
        function: impl Into<FunctionCodegenInput<'a>>,
        llvm_function: FunctionValue<'ctx>,
    ) -> Self {
        Self {
            module,
            builder: module.context.create_builder(),
            function: function.into(),
            llvm_function,
            locals: HashMap::new(),
            local_tys: HashMap::new(),
            zst_locals: HashMap::new(),
            out_ptr: None,
            defer_scopes: Vec::new(),
            function_defer_scopes: HashMap::new(),
            active_function_scope: None,
        }
    }

    fn alloc_function_locals(&mut self, body: &FunctionBody) -> Result<(), Diagnostic> {
        self.alloc_local_list(&body.locals)
    }

    fn local_addr(
        &mut self,
        local_id: LocalId,
        span: Span,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        if let Some(ptr) = self.locals.get(&local_id).copied() {
            return Ok(ptr);
        }
        let Some(ty) = self.local_tys.get(&local_id).copied() else {
            return Err(self.error(span, "missing local storage"));
        };
        if !self.is_zero_sized(ty) {
            return Err(self.error(span, "missing local storage"));
        }
        if let Some(ptr) = self.zst_locals.get(&local_id).copied() {
            return Ok(ptr);
        }
        let ptr = self
            .builder
            .build_alloca(self.module.context.i8_type(), "zst.local")
            .map_err(|_| self.error(span, "failed to build zero-sized local address"))?;
        self.zst_locals.insert(local_id, ptr);
        Ok(ptr)
    }

    fn alloc_local_list(&mut self, locals: &[FunctionLocal]) -> Result<(), Diagnostic> {
        for local in locals {
            if matches!(
                local.kind,
                FunctionLocalKind::Param
                    | FunctionLocalKind::MutableBinding
                    | FunctionLocalKind::ImmutableBinding
            ) {
                if self.is_zero_sized(local.ty) {
                    self.local_tys.insert(local.id, local.ty);
                    continue;
                }
                let ty = self.module.llvm_basic_type(local.ty, local.span)?;
                let name = self.local_storage_name(local);
                let ptr = self
                    .builder
                    .build_alloca(ty, &name)
                    .map_err(|_| self.error(local.span, "failed to build local alloca"))?;
                self.locals.insert(local.id, ptr);
                self.local_tys.insert(local.id, local.ty);
            }
        }
        Ok(())
    }

    fn local_storage_name(&self, local: &FunctionLocal) -> String {
        self.function
            .local_names
            .get(&local.id)
            .cloned()
            .unwrap_or_else(|| format!("local.{}", local.id.0))
    }

    fn store_params(&mut self) -> Result<(), Diagnostic> {
        let classifications = self
            .module
            .classify_function_params(self.function.params.iter().map(|param| param.passing_ty));
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
            FunctionExprKind::ConstGeneric(_) => {
                Err(self.error(expr.span, "const generic value reached LLVM codegen"))
            }
            FunctionExprKind::Null => self.emit_optional_null(expr),
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
                    let Some(global_info) = self.module.program.global(*def_id) else {
                        return Err(self.error(expr.span, "missing global metadata"));
                    };
                    let ty = self.module.llvm_basic_type_in(global_info.ty, expr.span)?;
                    self.builder
                        .build_load(ty, global.as_pointer_value(), "loadglobal")
                        .map_err(|_| self.error(expr.span, "failed to load global"))
                }),
            FunctionExprKind::GlobalInstance {
                def_id,
                arg_module_id,
                args,
                const_args,
            } => self
                .module
                .global_instances
                .get(&(*def_id, *arg_module_id, args.clone(), const_args.clone()))
                .copied()
                .ok_or_else(|| self.error(expr.span, "missing global instance value"))
                .and_then(|global| {
                    if self.is_zero_sized(expr.ty) {
                        return Err(self
                            .error(expr.span, "zero-sized global instance has no runtime value"));
                    }
                    let Some(global_info) = self.module.program.global_instance(
                        *def_id,
                        *arg_module_id,
                        args,
                        const_args,
                    ) else {
                        return Err(self.error(expr.span, "missing global instance metadata"));
                    };
                    let ty = self.module.llvm_basic_type_in(global_info.ty, expr.span)?;
                    self.builder
                        .build_load(ty, global.as_pointer_value(), "loadglobal.instance")
                        .map_err(|_| self.error(expr.span, "failed to load global instance"))
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
            FunctionExprKind::BuiltinValue(FunctionBuiltinValue::FieldOffset { ty, field }) => {
                let Some(offset) = self.module.field_offset(*ty, *field) else {
                    return Err(self.error(expr.span, "field offset builtin has no known offset"));
                };
                Ok(self
                    .module
                    .context
                    .i64_type()
                    .const_int(offset, false)
                    .into())
            }
            FunctionExprKind::BuiltinValue(FunctionBuiltinValue::Int(value)) => {
                let ty = self
                    .module
                    .llvm_basic_type(expr.ty, expr.span)?
                    .into_int_type()?;
                Ok(ty.const_u128(value.bits()).into())
            }
            FunctionExprKind::Range(range) => self.emit_range(expr.span, expr.ty, range),
            FunctionExprKind::RangeBound { range, bound } => {
                self.emit_range_bound(expr.span, range, *bound)
            }
            FunctionExprKind::Trap => {
                self.emit_trap(expr.span)?;
                Err(self.error(expr.span, "trap does not produce a value"))
            }
            FunctionExprKind::InlineAsm(asm) => {
                self.emit_inline_asm(asm)?;
                Err(self.error(expr.span, "inline assembly does not produce a value"))
            }
            FunctionExprKind::Atomic(atomic) => self.emit_atomic_value(expr, atomic),
            FunctionExprKind::LoadUnaligned { ty, ptr } => {
                self.emit_load_unaligned(expr.span, *ty, ptr)
            }
            FunctionExprKind::Splat { value } => self.emit_splat(expr, value),
            FunctionExprKind::ExtractElement { vector, index } => {
                self.emit_extract_element(expr, vector, index)
            }
            FunctionExprKind::InsertElement {
                vector,
                index,
                value,
            } => self.emit_insert_element(expr, vector, index, value),
            FunctionExprKind::Bitmask { vector } => self.emit_bitmask(expr, vector),
            FunctionExprKind::BitIntrinsic { op, value } => {
                self.emit_bit_intrinsic(expr, *op, value)
            }
            FunctionExprKind::CharFromU32 { value } => self.emit_char_from_u32(expr, value),
            FunctionExprKind::StaticArrayPointer { array, .. } => {
                self.emit_static_array_pointer(expr.span, array)
            }
            FunctionExprKind::ArrayLiteral { elems } => self.emit_array_literal(expr, elems),
            FunctionExprKind::StructLiteral { def_id, fields } => {
                self.emit_struct_literal(expr, *def_id, fields)
            }
            FunctionExprKind::UnionLiteral { def_id, field } => {
                self.emit_union_literal(expr, *def_id, field)
            }
            FunctionExprKind::OptionalSome { expr: inner } => self.emit_optional_some(expr, inner),
            FunctionExprKind::ErrorOk { expr: inner } => {
                self.emit_error_union_value(expr, FunctionErrorUnionTag::Ok, inner)
            }
            FunctionExprKind::ErrorErr { expr: inner } => {
                self.emit_error_union_value(expr, FunctionErrorUnionTag::Err, inner)
            }
            FunctionExprKind::TaggedUnionTag { expr: inner } => {
                let aggregate = self.emit_tagged_union_value(expr.span, inner)?;
                self.builder
                    .build_extract_value(aggregate, 0, "tagged.tag")
                    .map_err(|_| self.error(expr.span, "failed to extract tagged union tag"))
            }
            FunctionExprKind::TaggedUnionPayload { expr: inner } => {
                self.emit_tagged_union_payload(expr.span, inner, expr.ty)
            }
            FunctionExprKind::Try { .. } => Err(self.error(
                expr.span,
                "`.?` propagation requires control-flow lowering before LLVM codegen",
            )),
            FunctionExprKind::EnumVariant { variant, fields } => {
                self.emit_enum_variant(expr, *variant, fields)
            }
            FunctionExprKind::EnumVariantTag(variant) => self.emit_enum_variant_tag(expr, *variant),
            FunctionExprKind::EnumTag { value } => self.emit_enum_tag(expr, value),
            FunctionExprKind::EnumPayloadField {
                value,
                variant,
                field,
            } => self.emit_enum_payload_field(expr, value, *variant, *field),
            FunctionExprKind::Binary { lhs, op, rhs } => {
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    return self.emit_short_circuit(expr.span, lhs, *op, rhs);
                }
                let operand_ty = lhs.ty;
                let rhs_ty = rhs.ty;
                let lhs = self.emit_expr(lhs)?;
                let rhs = self.emit_expr(rhs)?;
                self.emit_binary(expr.span, operand_ty, lhs, *op, rhs_ty, rhs)
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

    fn emit_splat(
        &mut self,
        expr: &FunctionExpr,
        value: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let lanes = match self.module.ty_kind(expr.ty) {
            Some(TyKind::Vector { lanes, .. }) => *lanes,
            _ => {
                return Err(self.error(
                    expr.span,
                    "std::builtin::splat result type is not a SIMD vector",
                ));
            }
        };
        let vector_ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
        let zero = vector_ty
            .const_zero()
            .map_err(|_| self.error(expr.span, "failed to create zero vector"))?
            .into_vector_value()?;
        let lane = self.emit_expr(value)?;
        let index = self.module.context.i32_type().const_int(0, false);
        let inserted = self
            .builder
            .build_insert_element(zero, lane, index, "splat.insert")
            .map_err(|_| self.error(expr.span, "failed to insert splat lane"))?
            .into_vector_value()?;
        let mask = BasicTypeEnum::from(self.module.context.i32_type())
            .vector_type(lanes)
            .const_zero()
            .map_err(|_| self.error(expr.span, "failed to create splat mask"))?
            .into_vector_value()?;
        self.builder
            .build_shuffle_vector(inserted, inserted, mask, "splat")
            .map_err(|_| self.error(expr.span, "failed to build splat vector"))
    }

    fn emit_load_unaligned(
        &mut self,
        span: Span,
        ty: InternedTyId,
        ptr: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let llvm_ty = self.module.llvm_basic_type(ty, span)?;
        let ptr = self.emit_expr(ptr)?.into_pointer_value()?;
        self.builder
            .build_aligned_load(llvm_ty, ptr, 1, "load.unaligned")
            .map_err(|_| self.error(span, "failed to build unaligned load"))
    }

    fn emit_extract_element(
        &mut self,
        expr: &FunctionExpr,
        vector: &FunctionExpr,
        index: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let vector = self.emit_expr(vector)?.into_vector_value()?;
        let index = self.emit_expr(index)?.into_int_value()?;
        self.builder
            .build_extract_element(vector, index, "extract")
            .map_err(|_| self.error(expr.span, "failed to extract vector lane"))
    }

    fn emit_insert_element(
        &mut self,
        expr: &FunctionExpr,
        vector: &FunctionExpr,
        index: &FunctionExpr,
        value: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let vector = self.emit_expr(vector)?.into_vector_value()?;
        let index = self.emit_expr(index)?.into_int_value()?;
        let value = self.emit_expr(value)?;
        self.builder
            .build_insert_element(vector, value, index, "insert")
            .map_err(|_| self.error(expr.span, "failed to insert vector lane"))
    }

    fn emit_bitmask(
        &mut self,
        expr: &FunctionExpr,
        vector: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let lanes = match self.module.ty_kind(vector.ty) {
            Some(TyKind::Vector {
                elem: PrimitiveTy::Bool,
                lanes,
            }) => *lanes,
            _ => {
                return Err(self.error(
                    expr.span,
                    "std::builtin::bitmask argument must be a SIMD bool vector",
                ));
            }
        };
        if lanes > 64 {
            return Err(self.error(
                expr.span,
                "std::builtin::bitmask supports at most 64 SIMD mask lanes",
            ));
        }
        let vector = self.emit_expr(vector)?;
        let packed_ty = self.module.context.custom_width_int_type(lanes);
        let packed = self
            .builder
            .build_bit_cast(vector, packed_ty, "bitmask.pack")
            .map_err(|_| self.error(expr.span, "failed to pack SIMD mask"))?
            .into_int_value()?;
        if lanes == 64 {
            return Ok(packed.into());
        }
        self.builder
            .build_int_z_extend(packed, self.module.context.i64_type(), "bitmask")
            .map(Into::into)
            .map_err(|_| self.error(expr.span, "failed to widen SIMD mask"))
    }

    fn emit_bit_intrinsic(
        &mut self,
        expr: &FunctionExpr,
        op: FunctionBitIntrinsicOp,
        value: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let value = self.emit_expr(value)?;
        let ty = value.get_type()?;
        let intrinsic_name = match op {
            FunctionBitIntrinsicOp::Ctz => "llvm.cttz",
            FunctionBitIntrinsicOp::Clz => "llvm.ctlz",
            FunctionBitIntrinsicOp::Popcount => "llvm.ctpop",
        };
        let intrinsic = nia_llvm::intrinsics::Intrinsic::find(intrinsic_name)
            .and_then(|intrinsic| intrinsic.get_declaration(&self.module.module, &[ty]))
            .ok_or_else(|| self.error(expr.span, "failed to declare bit intrinsic"))?;
        let call = match op {
            FunctionBitIntrinsicOp::Ctz | FunctionBitIntrinsicOp::Clz => {
                let zero_is_poison = self.module.context.bool_type().const_int(0, false);
                self.builder
                    .build_call(intrinsic, &[value, zero_is_poison.into()], "bitintr")
            }
            FunctionBitIntrinsicOp::Popcount => {
                self.builder.build_call(intrinsic, &[value], "bitintr")
            }
        }
        .map_err(|_| self.error(expr.span, "failed to build bit intrinsic call"))?;
        call.try_as_basic_value()
            .unwrap_basic()
            .map_err(|_| self.error(expr.span, "bit intrinsic did not produce a value"))
    }

    fn emit_char_from_u32(
        &mut self,
        expr: &FunctionExpr,
        value: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let value = self.emit_expr(value)?.into_int_value()?;
        let i32_ty = self.module.context.i32_type();
        let max = i32_ty.const_int(0x10ffff, false);
        let surrogate_start = i32_ty.const_int(0xd800, false);
        let surrogate_end = i32_ty.const_int(0xdfff, false);
        let in_range = self
            .builder
            .build_int_compare(IntPredicate::ULE, value, max, "char.range")
            .map_err(|_| self.error(expr.span, "failed to check char range"))?;
        let before_surrogates = self
            .builder
            .build_int_compare(
                IntPredicate::ULT,
                value,
                surrogate_start,
                "char.before_surrogate",
            )
            .map_err(|_| self.error(expr.span, "failed to check char surrogate range"))?;
        let after_surrogates = self
            .builder
            .build_int_compare(
                IntPredicate::UGT,
                value,
                surrogate_end,
                "char.after_surrogate",
            )
            .map_err(|_| self.error(expr.span, "failed to check char surrogate range"))?;
        let not_surrogate = self
            .builder
            .build_or(before_surrogates, after_surrogates, "char.not_surrogate")
            .map_err(|_| self.error(expr.span, "failed to combine char validity"))?;
        let valid = self
            .builder
            .build_and(in_range, not_surrogate, "char.valid")
            .map_err(|_| self.error(expr.span, "failed to combine char validity"))?;
        let optional_ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
        let out = self
            .builder
            .build_alloca(optional_ty, "char.optional")
            .map_err(|_| self.error(expr.span, "failed to allocate char optional"))?;
        let tag_ptr = self
            .builder
            .build_struct_gep(optional_ty, out, 0, "char.optional.tag")
            .map_err(|_| self.error(expr.span, "failed to build char optional tag"))?;
        let tag = self
            .builder
            .build_select(
                valid.into(),
                self.module
                    .context
                    .i8_type()
                    .const_int(
                        nia_function_ir::FunctionOptionalTag::Some
                            .discriminant()
                            .into(),
                        false,
                    )
                    .into(),
                self.module
                    .context
                    .i8_type()
                    .const_int(
                        nia_function_ir::FunctionOptionalTag::Null
                            .discriminant()
                            .into(),
                        false,
                    )
                    .into(),
                "char.optional.tag",
            )
            .map_err(|_| self.error(expr.span, "failed to select char optional tag"))?;
        self.builder
            .build_store(tag_ptr, tag)
            .map_err(|_| self.error(expr.span, "failed to store char optional tag"))?;
        let payload_ptr = self
            .builder
            .build_struct_gep(optional_ty, out, 1, "char.optional.payload")
            .map_err(|_| self.error(expr.span, "failed to build char optional payload"))?;
        self.builder
            .build_store(payload_ptr, value)
            .map_err(|_| self.error(expr.span, "failed to store char optional payload"))?;
        self.builder
            .build_load(optional_ty, out, "char.optional")
            .map_err(|_| self.error(expr.span, "failed to load char optional"))
    }

    fn emit_trait_object_coercion(
        &mut self,
        span: Span,
        inner: &FunctionExpr,
        self_ty: InternedTyId,
        target_ty: InternedTyId,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let object_ptr = self.emit_trait_object_data_ptr(span, inner)?;
        self.emit_trait_object_from_data_ptr(span, self_ty, target_ty, object_ptr)
    }

    pub(super) fn emit_trait_object_from_data_ptr(
        &mut self,
        span: Span,
        self_ty: InternedTyId,
        target_ty: InternedTyId,
        object_ptr: PointerValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
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

    fn emit_trait_object_data_ptr(
        &mut self,
        span: Span,
        inner: &FunctionExpr,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        match self.module.ty_kind(inner.ty) {
            Some(TyKind::Pointer { .. }) => Ok(self.emit_expr(inner)?.into_pointer_value()?),
            Some(TyKind::Slice { .. }) => {
                let ty = self.module.llvm_basic_type(inner.ty, span)?;
                let ptr = self
                    .builder
                    .build_alloca(ty, "traitobj.self")
                    .map_err(|_| self.error(span, "failed to allocate trait object self"))?;
                let value = self.emit_expr(inner)?;
                self.builder
                    .build_store(ptr, value)
                    .map_err(|_| self.error(span, "failed to store trait object self"))?;
                Ok(ptr)
            }
            _ => Err(self.error(span, "trait object source is not representable")),
        }
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
                .into_struct_value()
                .map_err(|_| self.error(start_span, "range value is not a struct"))?;
            index += 1;
        }
        if let Some(end) = &range.end {
            let end_span = end.span;
            let end = self.emit_expr(end)?;
            value = self
                .builder
                .build_insert_value(value, end, index, "range.end")
                .map_err(|_| self.error(end_span, "failed to insert range end"))?
                .into_struct_value()
                .map_err(|_| self.error(end_span, "range value is not a struct"))?;
        }
        Ok(value.into())
    }

    fn emit_range_bound(
        &mut self,
        span: Span,
        range: &FunctionExpr,
        bound: nia_function_ir::FunctionRangeBound,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let Some(TyKind::Range { kind, .. }) = self.module.ty_kind(range.ty) else {
            return Err(self.error(span, "range bound base type is not a range"));
        };
        let index = match (kind, bound) {
            (
                nia_ty::RangeTyKind::Exclusive | nia_ty::RangeTyKind::Inclusive,
                nia_function_ir::FunctionRangeBound::Start,
            )
            | (nia_ty::RangeTyKind::From, nia_function_ir::FunctionRangeBound::Start)
            | (
                nia_ty::RangeTyKind::To | nia_ty::RangeTyKind::ToInclusive,
                nia_function_ir::FunctionRangeBound::End,
            ) => 0,
            (
                nia_ty::RangeTyKind::Exclusive | nia_ty::RangeTyKind::Inclusive,
                nia_function_ir::FunctionRangeBound::End,
            ) => 1,
            _ => {
                return Err(self.error(span, "range type does not contain requested bound"));
            }
        };
        let range = self
            .emit_expr(range)?
            .into_struct_value()
            .map_err(|_| self.error(span, "range bound base value is not a struct"))?;
        self.builder
            .build_extract_value(range, index, "range.bound")
            .map_err(|_| self.error(span, "failed to extract range bound"))
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

    fn is_zero_sized_local(&self, local_id: LocalId) -> bool {
        self.local_tys
            .get(&local_id)
            .copied()
            .is_some_and(|ty| self.is_zero_sized(ty))
    }

    fn error_union_error_ty(&self, ty: InternedTyId) -> Option<InternedTyId> {
        match self.module.ty_kind(ty) {
            Some(TyKind::ErrorUnion { error, .. }) => Some(*error),
            _ => None,
        }
    }

    fn emit_tagged_union_payload(
        &mut self,
        span: Span,
        tagged: &FunctionExpr,
        payload_ty: InternedTyId,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let tagged_value = self.emit_tagged_union_value(span, tagged)?.into();
        self.load_tagged_union_payload_from_value(span, tagged_value, tagged.ty, payload_ty)
    }

    fn load_tagged_union_payload_from_value(
        &mut self,
        span: Span,
        tagged_value: BasicValueEnum<'ctx>,
        tagged_ty: InternedTyId,
        payload_ty: InternedTyId,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let tagged_ty = self.module.llvm_basic_type(tagged_ty, span)?;
        let tagged_ptr = self
            .builder
            .build_alloca(tagged_ty, "tagged.payload.copy")
            .map_err(|_| self.error(span, "failed to allocate tagged union payload copy"))?;
        self.builder
            .build_store(tagged_ptr, tagged_value)
            .map_err(|_| self.error(span, "failed to store tagged union payload copy"))?;
        let payload_ptr = self
            .builder
            .build_struct_gep(tagged_ty, tagged_ptr, 1, "tagged.payload.ptr")
            .map_err(|_| self.error(span, "failed to build tagged union payload address"))?;
        let payload_llvm_ty = self.module.llvm_basic_type(payload_ty, span)?;
        self.builder
            .build_load(payload_llvm_ty, payload_ptr, "tagged.payload")
            .map_err(|_| self.error(span, "failed to load tagged union payload"))
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
            )) | Some(TyKind::Vector {
                elem: PrimitiveTy::I8
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
                    | PrimitiveTy::Bool,
                ..
            })
        )
    }

    fn is_float(&self, ty: InternedTyId) -> bool {
        matches!(
            self.module.ty_kind(ty),
            Some(TyKind::Primitive(PrimitiveTy::F32 | PrimitiveTy::F64))
                | Some(TyKind::Vector {
                    elem: PrimitiveTy::F32 | PrimitiveTy::F64,
                    ..
                })
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
            Some(TyKind::Nominal { def_id, .. }) if self.module.program.has_enum(*def_id)
        )
    }

    fn integer_bits(&self, ty: InternedTyId, span: Span) -> Result<u32, Diagnostic> {
        if let Some(TyKind::Nominal { def_id, .. }) = self.module.ty_kind(ty)
            && let Some(item) = self.module.program.enum_item(*def_id)
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
            )) | Some(TyKind::Vector {
                elem: PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize,
                ..
            })
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

    fn emit_trap(&mut self, span: Span) -> Result<(), Diagnostic> {
        if self.current_block_has_terminator() {
            return Ok(());
        }
        let intrinsic = nia_llvm::intrinsics::Intrinsic::find("llvm.trap")
            .and_then(|intrinsic| intrinsic.get_declaration(&self.module.module, &[]))
            .ok_or_else(|| self.error(span, "failed to declare trap intrinsic"))?;
        self.builder
            .build_call(intrinsic, &[], "trap")
            .map_err(|_| self.error(span, "failed to build trap call"))?;
        if !self.current_block_has_terminator() {
            self.builder
                .build_unreachable()
                .map_err(|_| self.error(span, "failed to build trap terminator"))?;
        }
        Ok(())
    }

    fn error(&self, span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic::user_error_at(nia_diagnostic::codes::LLVM_CODEGEN, span, message)
    }
}

fn callee_is_extern(codegen: &FunctionCodegen<'_, '_, '_>, callee: &FunctionCallee) -> bool {
    match callee {
        FunctionCallee::Function(def_id) => codegen
            .module
            .function_item(*def_id)
            .is_some_and(|function| function.is_extern),
        FunctionCallee::FunctionInstance {
            def_id,
            arg_module_id,
            self_arg,
            args,
            const_args,
        } => codegen
            .module
            .function_instance_item_with_arg_module(
                *def_id,
                *arg_module_id,
                *self_arg,
                args,
                const_args,
            )
            .is_some_and(|function| function.is_extern),
        FunctionCallee::Method {
            def_id,
            arg_module_id,
            self_arg,
            args,
            ..
        } => {
            if self_arg.is_none() && args.is_empty() {
                codegen
                    .module
                    .function_item(*def_id)
                    .is_some_and(|function| function.is_extern)
            } else {
                codegen
                    .module
                    .function_instance_item_with_arg_module(
                        *def_id,
                        *arg_module_id,
                        *self_arg,
                        args,
                        &[],
                    )
                    .is_some_and(|function| function.is_extern)
            }
        }
        FunctionCallee::FunctionPointer(_) => false,
        FunctionCallee::DynamicTraitMethod { .. } => false,
        FunctionCallee::TraitMethod { .. }
        | FunctionCallee::TraitAssociatedFunction { .. }
        | FunctionCallee::BuiltinPlaceMethod { .. }
        | FunctionCallee::BuiltinMethod { .. }
        | FunctionCallee::BuiltinOperator(_) => false,
    }
}
