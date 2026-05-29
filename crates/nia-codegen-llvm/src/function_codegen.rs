// SPDX-License-Identifier: GPL-3.0-or-later
mod aggregate;
mod asm;
mod call;
mod control;
mod defer;
mod literals;
mod ops;
mod place;

use std::collections::HashMap;

use crate::module_codegen::{AbiParam, AbiReturn, ModuleCodegen};
use defer::DeferScope;
use nia_ast::BinaryOp;
use nia_backend_ir::BackendFunction;
use nia_body_ir::{BuiltinConst, TypedExpr, TypedExprKind, TypedLocalKind};
use nia_control_ir::{ControlBody, ControlScopeId};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId, LocalId};
use nia_llvm::{
    builder::Builder,
    values::{BasicValueEnum, FunctionValue, PointerValue},
};
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyKind};

pub(super) struct FunctionCodegen<'m, 'ctx, 'a> {
    module: &'m ModuleCodegen<'ctx, 'a>,
    builder: Builder<'ctx>,
    function: &'a BackendFunction,
    llvm_function: FunctionValue<'ctx>,
    locals: HashMap<LocalId, PointerValue<'ctx>>,
    local_tys: HashMap<LocalId, InternedTyId>,
    out_ptr: Option<PointerValue<'ctx>>,
    defer_scopes: Vec<DeferScope>,
    control_defer_scopes: HashMap<ControlScopeId, usize>,
    active_control_scope: Option<ControlScopeId>,
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
            control_defer_scopes: HashMap::new(),
            active_control_scope: None,
        }
    }

    fn alloc_control_locals(&mut self, body: &ControlBody) -> Result<(), Diagnostic> {
        self.alloc_local_list(&body.locals)
    }

    fn alloc_local_list(&mut self, locals: &[nia_body_ir::TypedLocal]) -> Result<(), Diagnostic> {
        for local in locals {
            if matches!(
                local.kind,
                TypedLocalKind::Param | TypedLocalKind::Binding | TypedLocalKind::ConstBinding
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

    fn emit_expr(&mut self, expr: &TypedExpr) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        match &expr.kind {
            TypedExprKind::Integer(text) => self.emit_integer_literal(expr.ty, expr.span, text),
            TypedExprKind::Float(text) => self.emit_float_literal(expr.ty, expr.span, text),
            TypedExprKind::String(scalars) => self.emit_string_literal(expr.ty, expr.span, scalars),
            TypedExprKind::ByteString(bytes) => {
                self.emit_byte_string_literal(expr.ty, expr.span, bytes)
            }
            TypedExprKind::Char(value) => self.emit_char_literal(expr.ty, expr.span, *value),
            TypedExprKind::ByteChar(text) => self.emit_byte_char_literal(expr.ty, expr.span, text),
            TypedExprKind::Bool(value) => Ok(self
                .module
                .context
                .bool_type()
                .const_int(u64::from(*value), false)
                .into()),
            TypedExprKind::Local(local_id) => {
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
            TypedExprKind::Global(def_id) => self
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
            TypedExprKind::BuiltinValue(BuiltinConst::Usize(value)) => Ok(self
                .module
                .context
                .i64_type()
                .const_int(*value, false)
                .into()),
            TypedExprKind::BuiltinValue(BuiltinConst::Int(value)) => {
                let ty = self
                    .module
                    .llvm_basic_type(expr.ty, expr.span)?
                    .into_int_type()?;
                Ok(ty.const_u128(*value as u128).into())
            }
            TypedExprKind::Len(inner) => self.emit_len(expr.span, inner),
            TypedExprKind::Ptr(inner) => self.emit_ptr(expr.span, inner),
            TypedExprKind::InlineAsm(asm) => {
                self.emit_inline_asm(asm)?;
                Err(self.error(expr.span, "inline assembly does not produce a value"))
            }
            TypedExprKind::CStringPointer { array, .. } => {
                self.emit_c_string_pointer(expr.span, array)
            }
            TypedExprKind::ArrayLiteral { elems } => self.emit_array_literal(expr, elems),
            TypedExprKind::StructLiteral { def_id, fields } => {
                self.emit_struct_literal(expr, *def_id, fields)
            }
            TypedExprKind::UnionLiteral { def_id, field } => {
                self.emit_union_literal(expr, *def_id, field)
            }
            TypedExprKind::EnumVariant(def_id) => self.emit_enum_variant(expr, *def_id),
            TypedExprKind::Binary { lhs, op, rhs } => {
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    return self.emit_short_circuit(expr.span, lhs, *op, rhs);
                }
                let lhs = self.emit_expr(lhs)?;
                let rhs = self.emit_expr(rhs)?;
                self.emit_binary(expr.span, expr.ty, lhs, *op, rhs)
            }
            TypedExprKind::Unary { op, expr: inner } => {
                self.emit_unary(expr.span, expr.ty, *op, inner)
            }
            TypedExprKind::Cast { expr: inner, ty } => {
                let value = self.emit_expr(inner)?;
                self.emit_cast(expr.span, inner.ty, *ty, value)
            }
            TypedExprKind::Call { callee, args } => self.emit_call(expr, callee, args),
            TypedExprKind::Slice { lhs, range, .. } => self.emit_slice(expr.span, lhs, range),
            TypedExprKind::Field { .. } | TypedExprKind::Index { .. } => {
                let ptr = self.emit_addr_of(expr)?;
                let ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
                self.builder
                    .build_load(ty, ptr, "loadtmp")
                    .map_err(|_| self.error(expr.span, "failed to load place"))
            }
            TypedExprKind::Block(_) | TypedExprKind::If { .. } | TypedExprKind::Switch(_) => {
                Err(self.error(
                    expr.span,
                    "control expression was not lowered to control IR",
                ))
            }
            TypedExprKind::Function(_) | TypedExprKind::FunctionInstance { .. } => Err(self.error(
                expr.span,
                "function item cannot be emitted as a runtime value",
            )),
            TypedExprKind::Assign { .. } => {
                Err(self.error(expr.span, "assignment expression cannot be used as a value"))
            }
            TypedExprKind::Discard(_) => {
                Err(self.error(expr.span, "discard expression cannot be used as a value"))
            }
            TypedExprKind::Error => Err(self.error(expr.span, "cannot emit erroneous expression")),
        }
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
