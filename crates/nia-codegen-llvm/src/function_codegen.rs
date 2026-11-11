// SPDX-License-Identifier: GPL-3.0-or-later
mod aggregate;
mod asm;
mod atomic;
mod c_call;
mod call;
mod defer;
mod function_body;
mod literals;
mod memory;
mod ops;
mod place;
mod value;

use std::collections::{HashMap, HashSet};

use crate::module_codegen::{
    AbiParam, AbiReturn, ModuleCodegen, checked_vtable_index, checked_vtable_slot_array_len,
};
use defer::DeferScope;
use nia_ast::BinaryOp;
use nia_backend_ir::{BackendClosureEntryOwner, BackendFunction, BackendParam};
use nia_diagnostic::Diagnostic;
use nia_function_ir::{
    FunctionBody, FunctionBuiltinValue, FunctionCallee, FunctionDeferBody, FunctionExpr,
    FunctionExprKind, FunctionLocal, FunctionLocalKind, FunctionOp, FunctionRange, FunctionScopeId,
    FunctionTerminator, shuffle_lane_index,
};
use nia_ids::{GlobalDefId, InternedTyId, IntrinsicOp, LocalId};
use nia_llvm::{
    IntPredicate,
    basic_block::BasicBlock,
    builder::Builder,
    types::BasicTypeEnum,
    values::{BasicValueEnum, FunctionValue, IntValue, PointerValue},
};
use nia_span::Span;
use nia_ty::{ConstGenericArg, LayoutBuiltin, PrimitiveTy, TyKind};

pub(super) struct FunctionCodegen<'m, 'ctx, 'a> {
    module: &'m ModuleCodegen<'ctx, 'a>,
    builder: Builder<'ctx>,
    function: FunctionCodegenInput<'a>,
    llvm_function: FunctionValue<'ctx>,
    locals: HashMap<LocalId, PointerValue<'ctx>>,
    local_tys: HashMap<LocalId, InternedTyId>,
    zst_locals: HashMap<LocalId, PointerValue<'ctx>>,
    out_ptr: Option<PointerValue<'ctx>>,
    /// Frame storage for a memory result that is not returned indirectly.
    return_slot: Option<PointerValue<'ctx>>,
    caller_location: Option<PointerValue<'ctx>>,
    defer_scopes: Vec<DeferScope>,
    function_defer_scopes: HashMap<FunctionScopeId, usize>,
    active_function_scope: Option<FunctionScopeId>,
    function_return_storage: Option<PointerValue<'ctx>>,
    function_return_cleanup_blocks:
        HashMap<(FunctionScopeId, usize, BasicBlock<'ctx>), BasicBlock<'ctx>>,
    function_return_cleanup_return: Option<BasicBlock<'ctx>>,
    pending_function_return_cleanups: Vec<FunctionReturnCleanup<'ctx>>,
    c_definition: Option<std::rc::Rc<c_call::CDefinitionAbi>>,
}

struct FunctionReturnCleanup<'ctx> {
    entry: BasicBlock<'ctx>,
    body: Option<FunctionDeferBody>,
    next: Option<BasicBlock<'ctx>>,
    span: Span,
}

#[derive(Clone)]
pub(super) struct FunctionCodegenInput<'a> {
    pub(super) params: &'a [BackendParam],
    pub(super) return_type: InternedTyId,
    pub(super) local_names: &'a HashMap<LocalId, String>,
    pub(super) span: Span,
    pub(super) closure_owner: BackendClosureEntryOwner,
    pub(super) tracks_caller: bool,
    // Definitions with C linkage take and return values through the C ABI.
    pub(super) is_extern: bool,
}

impl<'a> From<&'a BackendFunction> for FunctionCodegenInput<'a> {
    fn from(function: &'a BackendFunction) -> Self {
        Self {
            params: &function.params,
            return_type: function.return_type,
            local_names: &function.local_names,
            span: function.span,
            closure_owner: BackendClosureEntryOwner::Source(function.def_id),
            tracks_caller: function
                .attributes
                .contains(&nia_backend_ir::BackendFunctionAttribute::TrackCaller),
            is_extern: function.linkage.is_extern(),
        }
    }
}

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn new(
        module: &'m ModuleCodegen<'ctx, 'a>,
        function: impl Into<FunctionCodegenInput<'a>>,
        llvm_function: FunctionValue<'ctx>,
    ) -> Result<Self, Diagnostic> {
        let function = function.into();
        let builder = module
            .context
            .create_builder()
            .map_err(ModuleCodegen::diagnostic_from_llvm_error)?;
        let mut codegen = Self {
            module,
            builder,
            function,
            llvm_function,
            locals: HashMap::new(),
            local_tys: HashMap::new(),
            zst_locals: HashMap::new(),
            out_ptr: None,
            return_slot: None,
            caller_location: None,
            defer_scopes: Vec::new(),
            function_defer_scopes: HashMap::new(),
            active_function_scope: None,
            function_return_storage: None,
            function_return_cleanup_blocks: HashMap::new(),
            function_return_cleanup_return: None,
            pending_function_return_cleanups: Vec::new(),
            c_definition: None,
        };
        codegen.c_definition = codegen.c_definition_abi()?.map(std::rc::Rc::new);
        Ok(codegen)
    }

    fn alloc_function_locals(&mut self, body: &FunctionBody) -> Result<(), Diagnostic> {
        let mut preassigned = self.indirect_param_locals();
        if let Some(local) = self.returned_local(body) {
            let slot = self.return_slot(body.span)?;
            self.locals.insert(local, slot);
            preassigned.insert(local);
        }
        self.alloc_local_list(&body.locals, &preassigned)
    }

    /// The local whose storage can be the function's result slot: a binding
    /// of the result type that every value-returning exit returns.
    ///
    /// The value is then built where the caller wants it instead of being
    /// copied there at the return, which matters for large results. Callers
    /// always pass fresh storage, so writing it before the return cannot be
    /// observed. Deferred code and `.?` are the other writers and readers of
    /// the result slot: deferred code may change the local after the return
    /// has taken its value, and a propagated failure is written into the slot
    /// while the local is live. Functions with either keep separate storage.
    fn returned_local(&self, body: &FunctionBody) -> Option<LocalId> {
        let return_type = self.function.return_type;
        if !self.is_memory_value(return_type) {
            return None;
        }
        let mut returned = None;
        for block in &body.blocks {
            if block
                .ops
                .iter()
                .any(|op| matches!(op, FunctionOp::Defer(_)))
            {
                return None;
            }
            let value = match &block.terminator {
                FunctionTerminator::Try { .. } => return None,
                FunctionTerminator::Return { value, .. }
                | FunctionTerminator::Tail { value, .. } => value.as_ref(),
                _ => None,
            };
            let Some(value) = value else {
                continue;
            };
            let FunctionExprKind::Local(local) = value.kind else {
                return None;
            };
            if returned.is_some_and(|returned| returned != local) {
                return None;
            }
            returned = Some(local);
        }
        let local = returned?;
        body.locals
            .iter()
            .find(|candidate| candidate.id == local)
            .filter(|candidate| {
                matches!(
                    candidate.kind,
                    FunctionLocalKind::MutableBinding | FunctionLocalKind::ImmutableBinding
                ) && self.module.same_type(candidate.ty, return_type)
            })
            .map(|candidate| candidate.id)
    }

    /// Parameters passed indirectly: the caller's copy is their storage.
    fn indirect_param_locals(&self) -> HashSet<LocalId> {
        if self.c_definition.is_some() {
            return HashSet::new();
        }
        let classifications = self
            .module
            .classify_function_params(self.function.params.iter().map(|param| param.passing_ty));
        self.function
            .params
            .iter()
            .zip(classifications)
            .filter(|(_, classification)| matches!(classification, AbiParam::Indirect(_)))
            .filter_map(|(param, _)| param.local_id)
            .collect()
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
        let align = self.module.layout_of(ty).map_or(1, |layout| layout.align);
        let ptr = self.entry_alloca(
            self.module.context.i8_type().into(),
            align,
            "zst.local",
            span,
        )?;
        self.zst_locals.insert(local_id, ptr);
        Ok(ptr)
    }

    /// Allocates frame storage for every local except those in
    /// `preassigned`, whose storage is already known.
    fn alloc_local_list(
        &mut self,
        locals: &[FunctionLocal],
        preassigned: &HashSet<LocalId>,
    ) -> Result<(), Diagnostic> {
        for local in locals {
            if matches!(
                local.kind,
                FunctionLocalKind::Param
                    | FunctionLocalKind::MutableBinding
                    | FunctionLocalKind::ImmutableBinding
            ) {
                self.local_tys.insert(local.id, local.ty);
                if self.is_zero_sized(local.ty) || preassigned.contains(&local.id) {
                    continue;
                }
                let name = self.local_storage_name(local);
                let ptr = self.temporary(local.ty, &name, local.span)?;
                self.locals.insert(local.id, ptr);
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
        if let Some(definition) = self.c_definition.clone() {
            return self.store_c_params(&definition);
        }
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
            if let AbiParam::Indirect(_) = classification {
                let index = u32::try_from(llvm_index)
                    .map_err(|_| self.error(param.span, "LLVM parameter index is too large"))?;
                let Some(value) = self.llvm_function.get_nth_param(index) else {
                    return Err(self.error(param.span, "missing LLVM function parameter"));
                };
                self.locals.insert(local_id, value?.into_pointer_value()?);
                llvm_index += 1;
                continue;
            }
            let Some(ptr) = self.locals.get(&local_id).copied() else {
                if !matches!(classification, AbiParam::Omit) {
                    llvm_index += 1;
                }
                continue;
            };
            match classification {
                AbiParam::Direct(_) => {
                    let index = u32::try_from(llvm_index)
                        .map_err(|_| self.error(param.span, "LLVM parameter index is too large"))?;
                    let Some(value) = self.llvm_function.get_nth_param(index) else {
                        return Err(self.error(param.span, "missing LLVM function parameter"));
                    };
                    let value = value?;
                    self.builder.build_store(ptr, value).map_err(|_| {
                        self.error(param.span, "failed to store function parameter")
                    })?;
                    llvm_index += 1;
                }
                AbiParam::Indirect(_) | AbiParam::Omit => {}
            };
        }
        Ok(())
    }

    fn function_out_ptr(&self) -> Result<Option<PointerValue<'ctx>>, Diagnostic> {
        let returns_indirect = match &self.c_definition {
            Some(definition) => {
                matches!(definition.abi.ret, nia_abi_check::c_abi::CRet::SRet { .. })
            }
            None => matches!(
                self.module
                    .classify_function_return(self.function.return_type),
                AbiReturn::IndirectOut(_)
            ),
        };
        if !returns_indirect {
            return Ok(None);
        }
        let Some(value) = self.llvm_function.get_nth_param(0) else {
            return Err(self.error(self.function.span, "missing aggregate return pointer"));
        };
        Ok(Some(value?.into_pointer_value()?))
    }

    fn function_caller_location(&self) -> Result<Option<PointerValue<'ctx>>, Diagnostic> {
        if !self.function.tracks_caller {
            return Ok(None);
        }
        if self.c_definition.is_some() {
            return Err(self.error(
                self.function.span,
                "a C-ABI definition cannot receive a caller location",
            ));
        }
        let direct_params = self
            .module
            .classify_function_params(self.function.params.iter().map(|param| param.passing_ty))
            .into_iter()
            .filter(|param| !matches!(param, AbiParam::Omit))
            .count();
        let index = usize::from(matches!(
            self.module
                .classify_function_return(self.function.return_type),
            AbiReturn::IndirectOut(_)
        )) + direct_params;
        let index = u32::try_from(index).map_err(|_| {
            self.error(
                self.function.span,
                "caller-location parameter index is too large",
            )
        })?;
        let value = self
            .llvm_function
            .get_nth_param(index)
            .ok_or_else(|| self.error(self.function.span, "missing caller-location parameter"))?
            .map_err(ModuleCodegen::diagnostic_from_llvm_error)?;
        Ok(Some(value.into_pointer_value()?))
    }

    /// Where the function's memory-represented result is built: the caller's
    /// destination when the result is returned indirectly, or else frame
    /// storage that the return reads.
    fn return_slot(&mut self, span: Span) -> Result<PointerValue<'ctx>, Diagnostic> {
        if let Some(slot) = self.out_ptr.or(self.return_slot) {
            return Ok(slot);
        }
        let slot = self.temporary(self.function.return_type, "return.slot", span)?;
        self.return_slot = Some(slot);
        Ok(slot)
    }

    /// Returns the memory-represented result in [`Self::return_slot`].
    fn emit_memory_return(&mut self, span: Span) -> Result<(), Diagnostic> {
        if let Some(definition) = self.c_definition.clone() {
            let slot = self.return_slot(span)?;
            return self.emit_c_memory_return(&definition.abi.ret, span, slot);
        }
        self.builder
            .build_return(None)
            .map_err(|_| self.error(span, "failed to build aggregate return"))?;
        Ok(())
    }

    /// Returns an immediate-represented result.
    pub(super) fn emit_return_value(
        &mut self,
        span: Span,
        value: BasicValueEnum<'ctx>,
    ) -> Result<(), Diagnostic> {
        if let Some(definition) = self.c_definition.clone() {
            return self.emit_c_return(&definition.abi.ret, span, value);
        }
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
                let destination = self
                    .out_ptr
                    .ok_or_else(|| self.error(span, "indirect result has no destination"))?;
                self.builder
                    .build_store(destination, value)
                    .map_err(|_| self.error(span, "failed to store indirect result"))?;
                self.builder
                    .build_return(None)
                    .map_err(|_| self.error(span, "failed to return indirect result"))?;
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

    /// The SSA value of an immediate-represented expression. Memory values
    /// have no such form; see [`value`].
    fn emit_expr(&mut self, expr: &FunctionExpr) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        if self.is_memory_value(expr.ty) {
            return Err(self.error(
                expr.span,
                "memory-represented value reached an immediate context",
            ));
        }
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
                .const_int(u64::from(*value), false)?
                .into()),
            FunctionExprKind::ConstGeneric(_) => {
                Err(self.error(expr.span, "const generic value reached LLVM codegen"))
            }
            FunctionExprKind::AssociatedConstProjection(_) => Err(self.error(
                expr.span,
                "unresolved associated const projection reached LLVM codegen",
            )),
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
                .usize_llvm_type(expr.span)?
                .const_int(*value, false)?
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
                    .usize_llvm_type(expr.span)?
                    .const_int(value, false)?
                    .into())
            }
            FunctionExprKind::BuiltinValue(FunctionBuiltinValue::FieldOffset { ty, field }) => {
                let Some(offset) = self.module.field_offset(*ty, *field) else {
                    return Err(self.error(expr.span, "field offset builtin has no known offset"));
                };
                Ok(self
                    .module
                    .usize_llvm_type(expr.span)?
                    .const_int(offset, false)?
                    .into())
            }
            FunctionExprKind::BuiltinValue(FunctionBuiltinValue::Int(value)) => {
                let ty = self
                    .module
                    .llvm_basic_type(expr.ty, expr.span)?
                    .into_int_type()?;
                Ok(ty.const_u128(value.bits())?.into())
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
            FunctionExprKind::Intrinsic { op, args } => self.emit_intrinsic(expr, *op, args),
            FunctionExprKind::StaticArrayPointer {
                allocation, array, ..
            } => self.emit_static_array_pointer(expr.span, *allocation, array),
            FunctionExprKind::CallerLocation(_)
            | FunctionExprKind::Null
            | FunctionExprKind::CharFromU32 { .. }
            | FunctionExprKind::ArrayLiteral { .. }
            | FunctionExprKind::Tuple(_)
            | FunctionExprKind::StructLiteral { .. }
            | FunctionExprKind::UnionLiteral { .. }
            | FunctionExprKind::UnionStorageLiteral { .. }
            | FunctionExprKind::OptionalSome { .. }
            | FunctionExprKind::ErrorOk { .. }
            | FunctionExprKind::ErrorErr { .. } => Err(self.error(
                expr.span,
                "a value that lives in memory reached an immediate context",
            )),
            FunctionExprKind::TupleField { value, index } => {
                let ptr = self.emit_tuple_field_addr(expr.span, value, *index)?;
                self.load_immediate(expr, ptr)
            }
            FunctionExprKind::TaggedUnionTag { expr: inner } => {
                let tagged = self.emit_memory_operand(inner)?;
                self.load_tagged_union_tag(expr.span, inner.ty, tagged, "tagged.tag")
                    .map(Into::into)
            }
            FunctionExprKind::TaggedUnionPayload { expr: inner } => {
                let tagged = self.emit_memory_operand(inner)?;
                let ptr = self.tagged_union_payload_addr(expr.span, inner.ty, tagged)?;
                self.load_immediate(expr, ptr)
            }
            FunctionExprKind::Try { .. } => Err(self.error(
                expr.span,
                "`.?` propagation requires control-flow lowering before LLVM codegen",
            )),
            FunctionExprKind::EnumVariant { variant, .. } => self.emit_enum_variant(expr, *variant),
            FunctionExprKind::EnumVariantTag(variant) => self.emit_enum_variant_tag(expr, *variant),
            FunctionExprKind::EnumTag { value } => self.emit_enum_tag(expr, value),
            FunctionExprKind::EnumPayloadField {
                value,
                variant,
                field,
            } => {
                let ptr = self.emit_enum_payload_field_addr(expr, value, *variant, *field)?;
                self.load_immediate(expr, ptr)
            }
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
            FunctionExprKind::CallableCoercion { closure_id, state } => {
                self.emit_callable_coercion(expr.span, *closure_id, state)
            }
            FunctionExprKind::FunctionCallable { function } => {
                self.emit_function_callable(expr.span, function)
            }
            FunctionExprKind::ClosureFunctionPointer { closure_id } => {
                let Some((key, _entry)) = self.closure_entry_for_call(*closure_id, None) else {
                    return Err(self.error(expr.span, "missing generated closure entry ABI"));
                };
                Ok(self
                    .module
                    .closure_function_pointer_adapter(&key, expr.ty, expr.span)?
                    .as_global_value()
                    .as_pointer_value()
                    .into())
            }
            FunctionExprKind::EnumConstructor(variant) => Ok(self
                .module
                .enum_constructor_function(*variant, expr.ty, expr.span)?
                .as_global_value()
                .as_pointer_value()
                .into()),
            FunctionExprKind::Call { callee, args } => self.emit_call(expr, callee, args),
            FunctionExprKind::Slice { lhs, range, .. } => {
                self.emit_slice(expr.span, expr.ty, lhs, range)
            }
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

    fn emit_intrinsic(
        &mut self,
        expr: &FunctionExpr,
        op: IntrinsicOp,
        args: &[FunctionExpr],
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        match (op, args) {
            (IntrinsicOp::LoadUnaligned, [ptr]) => self.emit_load_unaligned(expr, ptr),
            (IntrinsicOp::Splat, [value]) => self.emit_splat(expr, value),
            (IntrinsicOp::ExtractElement, [vector, index]) => {
                self.emit_extract_element(expr, vector, index)
            }
            (IntrinsicOp::InsertElement, [vector, index, value]) => {
                self.emit_insert_element(expr, vector, index, value)
            }
            (IntrinsicOp::Bitmask, [vector]) => self.emit_bitmask(expr, vector),
            (IntrinsicOp::Ctz, [value]) => self.emit_bit_intrinsic(expr, "llvm.cttz", true, value),
            (IntrinsicOp::Clz, [value]) => self.emit_bit_intrinsic(expr, "llvm.ctlz", true, value),
            (IntrinsicOp::Popcount, [value]) => {
                self.emit_bit_intrinsic(expr, "llvm.ctpop", false, value)
            }
            (IntrinsicOp::SaturatingAdd, [lhs, rhs]) => self.emit_saturating(expr, "add", lhs, rhs),
            (IntrinsicOp::SaturatingSub, [lhs, rhs]) => self.emit_saturating(expr, "sub", lhs, rhs),
            (IntrinsicOp::Shuffle, [first, second, indices @ ..]) if !indices.is_empty() => {
                self.emit_shuffle(expr, first, second, indices)
            }
            (IntrinsicOp::Lookup16, [table, indices]) => self.emit_lookup16(expr, table, indices),
            // Every operation is named so that a new one is a compile error
            // here until it is emitted.
            (
                IntrinsicOp::LoadUnaligned
                | IntrinsicOp::Splat
                | IntrinsicOp::ExtractElement
                | IntrinsicOp::InsertElement
                | IntrinsicOp::Bitmask
                | IntrinsicOp::Ctz
                | IntrinsicOp::Clz
                | IntrinsicOp::Popcount
                | IntrinsicOp::SaturatingAdd
                | IntrinsicOp::SaturatingSub
                | IntrinsicOp::Shuffle
                | IntrinsicOp::Lookup16,
                _,
            ) => Err(self.error(
                expr.span,
                format!(
                    "intrinsic `{}` takes {}, found {}",
                    op.name(),
                    op.arity(),
                    args.len()
                ),
            )),
        }
    }

    /// Emits `lookup16`. AArch64's `tbl` has exactly its semantics, an index
    /// of 16 or more giving 0, so it is called directly. Elsewhere the lookup
    /// is a lane-by-lane gather on the low four index bits, then a select
    /// that zeroes out-of-range lanes; x86 code generation turns that into
    /// `pshufb` and a mask whenever the CPU has SSSE3.
    fn emit_lookup16(
        &mut self,
        expr: &FunctionExpr,
        table: &FunctionExpr,
        indices: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let table = self.emit_expr(table)?.into_vector_value()?;
        let indices = self.emit_expr(indices)?.into_vector_value()?;
        let bytes_ty = BasicValueEnum::from(table).get_type()?;
        if self.module.target.arch() == nia_target::Arch::Aarch64 {
            let intrinsic = nia_llvm::intrinsics::Intrinsic::find("llvm.aarch64.neon.tbl1")
                .and_then(|intrinsic| intrinsic.get_declaration(&self.module.module, &[bytes_ty]))
                .ok_or_else(|| self.error(expr.span, "failed to declare table lookup"))?;
            let call = self
                .builder
                .build_call(intrinsic, &[table.into(), indices.into()], "lookup16")
                .map_err(|_| self.error(expr.span, "failed to build table lookup"))?;
            return call
                .try_as_basic_value()
                .unwrap_basic()
                .map_err(|_| self.error(expr.span, "table lookup did not produce a value"));
        }
        let i8_ty = self.module.context.i8_type();
        let splat = |this: &Self, value: u64| -> Result<BasicValueEnum<'ctx>, Diagnostic> {
            let lanes = vec![BasicValueEnum::from(i8_ty.const_int(value, false)?); 16];
            BasicTypeEnum::from(i8_ty)
                .vector_type(16)
                .and_then(|ty| ty.const_vector(&lanes))
                .map(Into::into)
                .map_err(|_| this.error(expr.span, "failed to create lookup constant"))
        };
        let in_range = self
            .builder
            .build_basic_int_compare(
                nia_llvm::IntPredicate::ULT,
                indices.into(),
                splat(self, 16)?,
                "lookup16.in_range",
            )
            .map_err(|_| self.error(expr.span, "failed to compare lookup indices"))?;
        let low_bits = self
            .builder
            .build_basic_and(indices.into(), splat(self, 15)?, "lookup16.index")
            .map_err(|_| self.error(expr.span, "failed to mask lookup indices"))?
            .into_vector_value()?;
        let zero = bytes_ty
            .const_zero()
            .map_err(|_| self.error(expr.span, "failed to create zero vector"))?;
        let i32_ty = self.module.context.i32_type();
        let mut gathered = zero.into_vector_value()?;
        for lane in 0..16 {
            let lane = i32_ty.const_int(lane, false)?;
            let index = self
                .builder
                .build_extract_element(low_bits, lane, "lookup16.lane_index")
                .map_err(|_| self.error(expr.span, "failed to read lookup index"))?
                .into_int_value()?;
            let byte = self
                .builder
                .build_extract_element(table, index, "lookup16.byte")
                .map_err(|_| self.error(expr.span, "failed to read table byte"))?;
            gathered = self
                .builder
                .build_insert_element(gathered, byte, lane, "lookup16.gather")
                .map_err(|_| self.error(expr.span, "failed to gather table bytes"))?
                .into_vector_value()?;
        }
        self.builder
            .build_select(in_range, gathered.into(), zero, "lookup16")
            .map_err(|_| self.error(expr.span, "failed to zero out-of-range lookups"))
    }

    /// Emits a `shufflevector`. Every index is a constant below twice the
    /// operand lane count; the backend validator has checked both.
    fn emit_shuffle(
        &mut self,
        expr: &FunctionExpr,
        first: &FunctionExpr,
        second: &FunctionExpr,
        indices: &[FunctionExpr],
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let first = self.emit_expr(first)?.into_vector_value()?;
        let second = self.emit_expr(second)?.into_vector_value()?;
        let i32_ty = self.module.context.i32_type();
        let mask = indices
            .iter()
            .map(|index| {
                let lane = shuffle_lane_index(index)
                    .and_then(|lane| u32::try_from(lane).ok())
                    .ok_or_else(|| {
                        self.error(index.span, "shuffle lane index is not a constant")
                    })?;
                Ok(BasicValueEnum::from(
                    i32_ty.const_int(u64::from(lane), false)?,
                ))
            })
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        let mask = BasicTypeEnum::from(i32_ty)
            .vector_type(
                u32::try_from(mask.len())
                    .map_err(|_| self.error(expr.span, "shuffle has too many result lanes"))?,
            )
            .map_err(|error| {
                self.error(
                    expr.span,
                    format!("failed to create shuffle mask type: {error:?}"),
                )
            })?
            .const_vector(&mask)
            .map_err(|_| self.error(expr.span, "failed to create shuffle mask"))?;
        self.builder
            .build_shuffle_vector(first, second, mask, "shuffle")
            .map_err(|_| self.error(expr.span, "failed to build shuffle"))
    }

    /// Emits `llvm.{s,u}{operation}.sat` on the result type; for a vector it
    /// works lane by lane.
    fn emit_saturating(
        &mut self,
        expr: &FunctionExpr,
        operation: &str,
        lhs: &FunctionExpr,
        rhs: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let lane = match self.module.ty_kind(expr.ty) {
            Some(TyKind::Primitive(primitive)) => *primitive,
            Some(TyKind::Vector { elem, .. }) => *elem,
            _ => {
                return Err(self.error(
                    expr.span,
                    "saturating arithmetic result is not an integer or integer vector",
                ));
            }
        };
        let sign = if lane.is_signed_integer() { 's' } else { 'u' };
        let lhs = self.emit_expr(lhs)?;
        let rhs = self.emit_expr(rhs)?;
        let ty = lhs.get_type()?;
        let intrinsic_name = format!("llvm.{sign}{operation}.sat");
        let intrinsic = nia_llvm::intrinsics::Intrinsic::find(&intrinsic_name)
            .and_then(|intrinsic| intrinsic.get_declaration(&self.module.module, &[ty]))
            .ok_or_else(|| self.error(expr.span, "failed to declare saturating intrinsic"))?;
        let call = self
            .builder
            .build_call(intrinsic, &[lhs, rhs], "sat")
            .map_err(|_| self.error(expr.span, "failed to build saturating intrinsic call"))?;
        call.try_as_basic_value()
            .unwrap_basic()
            .map_err(|_| self.error(expr.span, "saturating intrinsic did not produce a value"))
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
        let index = self.module.context.i32_type().const_int(0, false)?;
        let inserted = self
            .builder
            .build_insert_element(zero, lane, index, "splat.insert")
            .map_err(|_| self.error(expr.span, "failed to insert splat lane"))?
            .into_vector_value()?;
        let mask = BasicTypeEnum::from(self.module.context.i32_type())
            .vector_type(lanes)
            .map_err(|error| {
                self.error(
                    expr.span,
                    format!("failed to create splat mask type: {error:?}"),
                )
            })?
            .const_zero()
            .map_err(|_| self.error(expr.span, "failed to create splat mask"))?
            .into_vector_value()?;
        self.builder
            .build_shuffle_vector(inserted, inserted, mask, "splat")
            .map_err(|_| self.error(expr.span, "failed to build splat vector"))
    }

    fn emit_load_unaligned(
        &mut self,
        expr: &FunctionExpr,
        ptr: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let llvm_ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
        let ptr = self.emit_expr(ptr)?.into_pointer_value()?;
        self.builder
            .build_aligned_load(llvm_ty, ptr, 1, "load.unaligned")
            .map_err(|_| self.error(expr.span, "failed to build unaligned load"))
    }

    fn emit_extract_element(
        &mut self,
        expr: &FunctionExpr,
        vector: &FunctionExpr,
        index: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let lanes = match self.module.ty_kind(vector.ty) {
            Some(TyKind::Vector { lanes, .. }) => *lanes,
            _ => return Err(self.error(expr.span, "extract argument is not a SIMD vector")),
        };
        let vector = self.emit_expr(vector)?.into_vector_value()?;
        let index = self.emit_expr(index)?.into_int_value()?;
        self.emit_vector_index_bounds_check(expr.span, index, lanes, "extract")?;
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
        let lanes = match self.module.ty_kind(vector.ty) {
            Some(TyKind::Vector { lanes, .. }) => *lanes,
            _ => return Err(self.error(expr.span, "insert argument is not a SIMD vector")),
        };
        let vector = self.emit_expr(vector)?.into_vector_value()?;
        let index = self.emit_expr(index)?.into_int_value()?;
        let value = self.emit_expr(value)?;
        self.emit_vector_index_bounds_check(expr.span, index, lanes, "insert")?;
        self.builder
            .build_insert_element(vector, value, index, "insert")
            .map_err(|_| self.error(expr.span, "failed to insert vector lane"))
    }

    fn emit_vector_index_bounds_check(
        &mut self,
        span: Span,
        index: nia_llvm::values::IntValue<'ctx>,
        lanes: u32,
        name: &str,
    ) -> Result<(), Diagnostic> {
        let limit = index.get_type()?.const_int(u64::from(lanes), false)?;
        let in_bounds = self
            .builder
            .build_basic_int_compare(
                nia_llvm::IntPredicate::ULT,
                index.into(),
                limit.into(),
                &format!("{name}.in_bounds"),
            )
            .map_err(|_| self.error(span, "failed to compare SIMD lane index"))?
            .into_int_value()?;
        let operation = self
            .module
            .context
            .append_basic_block(self.llvm_function, &format!("{name}.operation"))?;
        let trap = self
            .module
            .context
            .append_basic_block(self.llvm_function, &format!("{name}.trap"))?;
        self.builder
            .build_conditional_branch(in_bounds, operation, trap)
            .map_err(|_| self.error(span, "failed to branch on SIMD lane bounds"))?;
        self.builder.position_at_end(trap);
        self.emit_trap(span)?;
        self.builder.position_at_end(operation);
        Ok(())
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
        let result_ty = self.module.usize_llvm_type(expr.span)?;
        if lanes > result_ty.bit_width() {
            return Err(self.error(
                expr.span,
                "std::builtin::bitmask mask exceeds the target usize width",
            ));
        }
        let vector = self.emit_expr(vector)?;
        let packed_ty = self
            .module
            .context
            .custom_width_int_type(lanes)
            .map_err(|error| {
                self.error(
                    expr.span,
                    format!("failed to create bitmask type: {error:?}"),
                )
            })?;
        let packed = self
            .builder
            .build_bit_cast(vector, packed_ty, "bitmask.pack")
            .map_err(|_| self.error(expr.span, "failed to pack SIMD mask"))?
            .into_int_value()?;
        if lanes == result_ty.bit_width() {
            return Ok(packed.into());
        }
        self.builder
            .build_int_z_extend(packed, result_ty, "bitmask")
            .map(Into::into)
            .map_err(|_| self.error(expr.span, "failed to widen SIMD mask"))
    }

    /// Calls an LLVM bit-counting intrinsic. `takes_zero_flag` intrinsics
    /// (`cttz`, `ctlz`) get `is_zero_poison = false`, so a zero input
    /// counts every bit, as the language defines.
    fn emit_bit_intrinsic(
        &mut self,
        expr: &FunctionExpr,
        intrinsic_name: &str,
        takes_zero_flag: bool,
        value: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let value = self.emit_expr(value)?;
        let ty = value.get_type()?;
        let intrinsic = nia_llvm::intrinsics::Intrinsic::find(intrinsic_name)
            .and_then(|intrinsic| intrinsic.get_declaration(&self.module.module, &[ty]))
            .ok_or_else(|| self.error(expr.span, "failed to declare bit intrinsic"))?;
        let call = if takes_zero_flag {
            let zero_is_poison = self.module.context.bool_type().const_int(0, false)?;
            self.builder
                .build_call(intrinsic, &[value, zero_is_poison.into()], "bitintr")
        } else {
            self.builder.build_call(intrinsic, &[value], "bitintr")
        }
        .map_err(|_| self.error(expr.span, "failed to build bit intrinsic call"))?;
        call.try_as_basic_value()
            .unwrap_basic()
            .map_err(|_| self.error(expr.span, "bit intrinsic did not produce a value"))
    }

    fn emit_char_from_u32_into(
        &mut self,
        expr: &FunctionExpr,
        value: &FunctionExpr,
        out: PointerValue<'ctx>,
    ) -> Result<(), Diagnostic> {
        let value = self.emit_expr(value)?.into_int_value()?;
        let i32_ty = self.module.context.i32_type();
        let max = i32_ty.const_int(0x10ffff, false)?;
        let surrogate_start = i32_ty.const_int(0xd800, false)?;
        let surrogate_end = i32_ty.const_int(0xdfff, false)?;
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
        let tag_ptr = unsafe {
            self.builder
                .build_struct_gep(optional_ty, out, 0, "char.optional.tag")
        }
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
                    )?
                    .into(),
                self.module
                    .context
                    .i8_type()
                    .const_int(
                        nia_function_ir::FunctionOptionalTag::Null
                            .discriminant()
                            .into(),
                        false,
                    )?
                    .into(),
                "char.optional.tag",
            )
            .map_err(|_| self.error(expr.span, "failed to select char optional tag"))?;
        self.builder
            .build_store(tag_ptr, tag)
            .map_err(|_| self.error(expr.span, "failed to store char optional tag"))?;
        let payload_ptr = unsafe {
            self.builder
                .build_struct_gep(optional_ty, out, 1, "char.optional.payload")
        }
        .map_err(|_| self.error(expr.span, "failed to build char optional payload"))?;
        self.builder
            .build_store(payload_ptr, value)
            .map_err(|_| self.error(expr.span, "failed to store char optional payload"))?;
        Ok(())
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
        let trait_object_ty = self.module.trait_object_type()?;
        let result = trait_object_ty.get_undef()?;
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

    fn emit_callable_coercion(
        &mut self,
        span: Span,
        closure_id: nia_ids::ClosureId,
        state: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let owner_instance = match self.module.ty_kind(state.ty) {
            Some(TyKind::Pointer { elem, .. }) => match self.module.ty_kind(*elem) {
                Some(TyKind::ClosureState { owner_instance, .. }) => owner_instance.as_ref(),
                _ => None,
            },
            _ => None,
        };
        let Some((key, _entry)) = self.closure_entry_for_call(closure_id, owner_instance) else {
            return Err(self.error(span, "missing generated closure entry function"));
        };
        let Some(function) = self.module.closure_entry_value(&key) else {
            return Err(self.error(span, "missing generated closure entry function"));
        };
        let state = self.emit_expr(state)?.into_pointer_value()?;
        let result = self.module.callable_type()?.get_undef()?;
        let result = self
            .builder
            .build_insert_value(result, state, 0, "callable.state")
            .map_err(|_| self.error(span, "failed to build callable state view"))?
            .into_struct_value()
            .map_err(|_| self.error(span, "failed to build callable state view"))?;
        self.builder
            .build_insert_value(
                result,
                function.as_global_value().as_pointer_value(),
                1,
                "callable.entry",
            )
            .map_err(|_| self.error(span, "failed to build callable entry view"))
    }

    fn emit_function_callable(
        &mut self,
        span: Span,
        function: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let function_value = match &function.kind {
            FunctionExprKind::Unary { expr: inner, .. } => self
                .emit_function_pointer(span, inner)?
                .into_pointer_value()?,
            _ => self.emit_expr(function)?.into_pointer_value()?,
        };
        let adapter = self
            .module
            .function_pointer_callable_adapter(function.ty, span)?;
        let result = self.module.callable_type()?.get_undef()?;
        let result = self
            .builder
            .build_insert_value(result, function_value, 0, "callable.context")
            .map_err(|_| self.error(span, "failed to build function callable"))?
            .into_struct_value()
            .map_err(|_| self.error(span, "failed to build function callable"))?;
        self.builder
            .build_insert_value(
                result,
                adapter.as_global_value().as_pointer_value(),
                1,
                "callable.entry",
            )
            .map_err(|_| self.error(span, "failed to build function callable"))
    }

    fn emit_trait_object_data_ptr(
        &mut self,
        span: Span,
        inner: &FunctionExpr,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        match self.module.ty_kind(inner.ty) {
            Some(TyKind::Pointer { .. }) => Ok(self.emit_expr(inner)?.into_pointer_value()?),
            Some(TyKind::Slice { .. }) => {
                let ptr = self.temporary(inner.ty, "traitobj.self", span)?;
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
        let trait_object_ty = self.module.trait_object_type()?;
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
            .trait_object_upcast_slot_offset(source_ty, target_ty, span)?;
        if offset > 0 {
            let array_len = checked_vtable_slot_array_len(offset).ok_or_else(|| {
                self.error(span, "trait-object metadata offset is too large for LLVM")
            })?;
            let offset_index = checked_vtable_index(offset).ok_or_else(|| {
                self.error(
                    span,
                    "trait-object metadata offset cannot be represented by LLVM",
                )
            })?;
            let ptr_ty = self.module.context.ptr_type(Default::default());
            let zero = self.module.context.i64_type().const_int(0, false)?;
            let offset_index = self
                .module
                .context
                .i64_type()
                .const_int(offset_index, false)?;
            metadata = unsafe {
                self.builder
                    .build_gep(
                        ptr_ty.array_type(array_len).map_err(|error| {
                            self.error(
                                span,
                                format!("failed to create metadata array type: {error:?}"),
                            )
                        })?,
                        metadata.into_pointer_value()?,
                        &[zero, offset_index],
                        "traitobj.upcast.metadata.offset",
                    )
                    .map_err(|_| self.error(span, "failed to offset trait object metadata"))?
                    .into()
            };
        }
        let result = trait_object_ty.get_undef()?;
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
            .get_undef()?;
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

    /// Loads an immediate-represented `expr` stored at `ptr`.
    fn load_immediate(
        &self,
        expr: &FunctionExpr,
        ptr: PointerValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let ty = self.module.llvm_basic_type(expr.ty, expr.span)?;
        self.builder
            .build_load(ty, ptr, "loadtmp")
            .map_err(|_| self.error(expr.span, "failed to load value"))
    }

    /// The address of tuple field `index` of `value`.
    fn emit_tuple_field_addr(
        &mut self,
        span: Span,
        value: &FunctionExpr,
        index: usize,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        let tuple = self.emit_memory_operand(value)?;
        let tuple_ty = self.module.llvm_basic_type(value.ty, span)?;
        let index = checked_tuple_field_index(index)
            .ok_or_else(|| self.error(span, "tuple field index is too large for LLVM"))?;
        unsafe {
            self.builder
                .build_struct_gep(tuple_ty, tuple, index, "tuple.field.ptr")
        }
        .map_err(|_| self.error(span, "failed to address tuple field"))
    }

    /// The address of the payload of the optional or error union at `ptr`.
    fn tagged_union_payload_addr(
        &self,
        span: Span,
        tagged_ty: InternedTyId,
        ptr: PointerValue<'ctx>,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        let ty = self.module.llvm_basic_type(tagged_ty, span)?;
        unsafe {
            self.builder
                .build_struct_gep(ty, ptr, 1, "tagged.payload.ptr")
        }
        .map_err(|_| self.error(span, "failed to address tagged union payload"))
    }

    fn load_tagged_union_tag(
        &self,
        span: Span,
        tagged_ty: InternedTyId,
        ptr: PointerValue<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>, Diagnostic> {
        let ty = self.module.llvm_basic_type(tagged_ty, span)?;
        let tag_ptr = unsafe { self.builder.build_struct_gep(ty, ptr, 0, name) }
            .map_err(|_| self.error(span, "failed to address tagged union tag"))?;
        self.builder
            .build_load(self.module.context.i8_type(), tag_ptr, name)
            .map_err(|_| self.error(span, "failed to load tagged union tag"))?
            .into_int_value()
            .map_err(|_| self.error(span, "tagged union tag is not an integer"))
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
            Some(
                TyKind::Pointer { .. }
                    | TyKind::VolatilePointer { .. }
                    | TyKind::FunctionPointer { .. },
            )
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
            PrimitiveTy::I64 | PrimitiveTy::U64 => 64,
            PrimitiveTy::Isize | PrimitiveTy::Usize => self
                .module
                .source
                .layouts
                .target
                .pointer_size
                .checked_mul(8)
                .and_then(|bits| u32::try_from(bits).ok())
                .filter(|bits| matches!(bits, 8 | 16 | 32 | 64 | 128))
                .ok_or_else(|| {
                    self.error(span, "target pointer width is not an LLVM integer width")
                })?,
            PrimitiveTy::I128 | PrimitiveTy::U128 => 128,
            PrimitiveTy::F32 | PrimitiveTy::F64 | PrimitiveTy::Never => {
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
            PrimitiveTy::I64 | PrimitiveTy::U64 => Ok(self.module.context.i64_type()),
            PrimitiveTy::Isize | PrimitiveTy::Usize => self.module.usize_llvm_type(span),
            PrimitiveTy::I128 | PrimitiveTy::U128 => Ok(self.module.context.i128_type()),
            PrimitiveTy::Bool => Ok(self.module.context.bool_type()),
            PrimitiveTy::F32 | PrimitiveTy::F64 | PrimitiveTy::Never => {
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

    fn is_unit(&self, ty: InternedTyId) -> bool {
        self.module.ty_kind(ty).is_some_and(TyKind::is_unit)
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

    fn emit_const_count_loop(
        &mut self,
        span: Span,
        count: u64,
        name: &str,
        mut emit_body: impl FnMut(&mut Self, nia_llvm::values::IntValue<'ctx>) -> Result<(), Diagnostic>,
    ) -> Result<(), Diagnostic> {
        if count == 0 {
            return Ok(());
        }
        let preheader = self
            .builder
            .get_insert_block()
            .ok_or_else(|| self.error(span, "missing current block for counted loop"))?;
        let header = self
            .module
            .context
            .append_basic_block(self.llvm_function, &format!("{name}.loop"))
            .map_err(|_| self.error(span, "failed to create counted loop header"))?;
        let body = self
            .module
            .context
            .append_basic_block(self.llvm_function, &format!("{name}.body"))
            .map_err(|_| self.error(span, "failed to create counted loop body"))?;
        let after = self
            .module
            .context
            .append_basic_block(self.llvm_function, &format!("{name}.end"))
            .map_err(|_| self.error(span, "failed to create counted loop end"))?;
        let usize_ty = self.module.usize_llvm_type(span)?;
        let zero = usize_ty.const_int(0, false)?;
        let one = usize_ty.const_int(1, false)?;
        let count = usize_ty.const_int(count, false)?;
        self.builder
            .build_unconditional_branch(header)
            .map_err(|_| self.error(span, "failed to enter counted loop"))?;

        self.builder.position_at_end(header);
        let index = self
            .builder
            .build_phi(usize_ty, &format!("{name}.index"))
            .map_err(|_| self.error(span, "failed to build counted loop index"))?;
        index
            .add_incoming(&[(&zero, preheader)])
            .map_err(|_| self.error(span, "failed to initialize counted loop index"))?;
        let index_value = index
            .as_basic_value()
            .and_then(|value| value.into_int_value())
            .map_err(|_| self.error(span, "failed to read counted loop index"))?;
        let keep_going = self
            .builder
            .build_int_compare(
                IntPredicate::ULT,
                index_value,
                count,
                &format!("{name}.more"),
            )
            .map_err(|_| self.error(span, "failed to compare counted loop index"))?;
        self.builder
            .build_conditional_branch(keep_going, body, after)
            .map_err(|_| self.error(span, "failed to branch counted loop"))?;

        self.builder.position_at_end(body);
        emit_body(self, index_value)?;
        if !self.current_block_has_terminator() {
            let body_end = self
                .builder
                .get_insert_block()
                .ok_or_else(|| self.error(span, "missing counted loop body block"))?;
            let next = self
                .builder
                .build_int_add(index_value, one, &format!("{name}.next"))
                .map_err(|_| self.error(span, "failed to increment counted loop index"))?;
            self.builder
                .build_unconditional_branch(header)
                .map_err(|_| self.error(span, "failed to continue counted loop"))?;
            index
                .add_incoming(&[(&next, body_end)])
                .map_err(|_| self.error(span, "failed to extend counted loop index"))?;
        }
        self.builder.position_at_end(after);
        Ok(())
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
        let message = message.into();
        Diagnostic::internal_error(nia_diagnostic::codes::INTERNAL_LLVM_API, message.clone())
            .primary(span, message)
            .note(
                "LLVM code generation reached an operation whose preconditions were not satisfied after semantic and backend validation",
            )
            .note("this indicates a compiler defect rather than a source-level type or name error")
            .finish()
    }
}

fn callee_is_extern(codegen: &FunctionCodegen<'_, '_, '_>, callee: &FunctionCallee) -> bool {
    match callee {
        FunctionCallee::Tracked { callee, .. } => callee_is_extern(codegen, callee),
        FunctionCallee::ClosureEntry { .. } => false,
        FunctionCallee::Function(def_id) => codegen
            .module
            .function_item(*def_id)
            .is_some_and(|function| function.linkage.is_extern()),
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
            .is_some_and(|function| function.linkage.is_extern()),
        FunctionCallee::Method {
            def_id,
            arg_module_id,
            self_arg,
            args,
            const_args,
            ..
        } => {
            if !method_requires_instance_metadata(*self_arg, args, const_args) {
                codegen
                    .module
                    .function_item(*def_id)
                    .is_some_and(|function| function.linkage.is_extern())
            } else {
                codegen
                    .module
                    .function_instance_item_with_arg_module(
                        *def_id,
                        *arg_module_id,
                        *self_arg,
                        args,
                        const_args,
                    )
                    .is_some_and(|function| function.linkage.is_extern())
            }
        }
        FunctionCallee::Callable(_) | FunctionCallee::FunctionPointer(_) => false,
        FunctionCallee::DynamicTraitMethod { .. } => false,
        FunctionCallee::TraitMethod { .. }
        | FunctionCallee::TraitAssociatedFunction { .. }
        | FunctionCallee::BuiltinTraitMethodCall { .. }
        | FunctionCallee::BuiltinMethod { .. }
        | FunctionCallee::BuiltinOperator(_) => false,
    }
}

pub(super) fn method_requires_instance_metadata(
    self_arg: Option<InternedTyId>,
    args: &[InternedTyId],
    const_args: &[ConstGenericArg],
) -> bool {
    self_arg.is_some() || !args.is_empty() || !const_args.is_empty()
}

fn checked_tuple_field_index(index: usize) -> Option<u32> {
    u32::try_from(index).ok()
}

fn checked_vector_lane_count(lanes: u32) -> Option<usize> {
    usize::try_from(lanes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ids::ModuleIdAllocator;
    use nia_ty::{ConstGenericValue, IntConst, TypeStore};

    trait TestTypeStoreAppend {
        fn test_primitive(&self, primitive: PrimitiveTy) -> InternedTyId;
    }

    impl TestTypeStoreAppend for nia_ty::TypeStoreAppend {
        fn test_primitive(&self, primitive: PrimitiveTy) -> InternedTyId {
            self.primitive(primitive)
                .expect("intern primitive function codegen test type")
        }
    }

    #[test]
    fn const_generic_method_calls_require_instance_metadata() {
        let module_id = ModuleIdAllocator::new()
            .expect("create module ID allocator")
            .allocate()
            .expect("allocate module ID");
        let types = TypeStore::new().expect("create type store");
        let usize_ty = types
            .append_for_module(module_id)
            .test_primitive(PrimitiveTy::Usize);
        let const_arg = ConstGenericArg {
            ty: usize_ty,
            value: ConstGenericValue::Int(IntConst::unsigned(4)),
        };

        assert!(method_requires_instance_metadata(None, &[], &[const_arg]));
        assert!(!method_requires_instance_metadata(None, &[], &[]));
    }

    #[test]
    fn tuple_field_index_conversion_rejects_llvm_width_overflow() {
        let max = u32::MAX as usize;
        assert_eq!(checked_tuple_field_index(max), Some(u32::MAX));
        if usize::BITS > u32::BITS {
            assert_eq!(checked_tuple_field_index(max + 1), None);
        }
    }

    #[test]
    fn vector_lane_count_conversion_is_checked() {
        assert_eq!(checked_vector_lane_count(1), Some(1));
        if usize::BITS < u32::BITS {
            assert_eq!(checked_vector_lane_count(u32::MAX), None);
        }
    }
}
