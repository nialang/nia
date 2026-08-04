// SPDX-License-Identifier: GPL-3.0-or-later
use crate::module_codegen::{AbiParam, AbiReturn};
use nia_diagnostic::Diagnostic;
use nia_function_ir::{FunctionBuiltinOperatorOp, FunctionCallee, FunctionExpr, FunctionExprKind};
use nia_ids::{InternedTyId, ReceiverKind};
use nia_llvm::values::{BasicValueEnum, CallSiteValue};
use nia_span::Span;
use nia_ty::{BuiltinTrait, TyKind, TypeEquivalence};

use super::{FunctionCodegen, callee_is_extern};

struct DynamicTraitMethodCall<'a, 'ctx> {
    expr: &'a FunctionExpr,
    // These fields mirror the body-check candidate. Keeping them together is
    // important because the vtable address calculation depends on the object
    // type, resolved trait, concrete method, and slot all matching.
    object_ty: nia_ids::InternedTyId,
    trait_id: nia_ty::TraitId,
    method_id: nia_ids::GlobalDefId,
    slot: usize,
    params: &'a [nia_ids::InternedTyId],
    return_type: nia_ids::InternedTyId,
    receiver: &'a FunctionExpr,
    args: &'a [FunctionExpr],
    out_ptr: Option<nia_llvm::values::PointerValue<'ctx>>,
}

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn emit_function_pointer(
        &mut self,
        span: Span,
        expr: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        match &expr.kind {
            FunctionExprKind::Function(def_id) => {
                let Some(function) = self.module.function(*def_id) else {
                    return Err(self.error(span, "missing function item"));
                };
                Ok(function.as_global_value().as_pointer_value().into())
            }
            FunctionExprKind::FunctionInstance {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
            } => {
                let Some(instance) = self.module.function_instance_item_with_arg_module(
                    *def_id,
                    *arg_module_id,
                    *self_arg,
                    args,
                    const_args,
                ) else {
                    return Err(self.error(span, "missing function instance item"));
                };
                let Some(function) = self.module.function_instance_value(
                    instance.def_id,
                    instance.arg_module_id,
                    instance.self_arg,
                    &instance.args,
                    &instance.const_args,
                ) else {
                    return Err(self.error(span, "missing function instance value"));
                };
                Ok(function.as_global_value().as_pointer_value().into())
            }
            _ => Err(self.error(span, "expression is not a function item")),
        }
    }

    pub(super) fn emit_call(
        &mut self,
        expr: &FunctionExpr,
        callee: &FunctionCallee,
        args: &[FunctionExpr],
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        if let FunctionCallee::BuiltinMethod {
            method,
            self_ty,
            receiver,
        } = callee
        {
            return match method {
                nia_function_ir::FunctionBuiltinMethod::Len => {
                    self.emit_builtin_len_method(expr.span, *self_ty, receiver)
                }
                nia_function_ir::FunctionBuiltinMethod::Start => self.emit_range_bound(
                    expr.span,
                    receiver,
                    nia_function_ir::FunctionRangeBound::Start,
                ),
                nia_function_ir::FunctionBuiltinMethod::End => self.emit_range_bound(
                    expr.span,
                    receiver,
                    nia_function_ir::FunctionRangeBound::End,
                ),
                nia_function_ir::FunctionBuiltinMethod::Iter => Err(self.error(
                    expr.span,
                    "`iter` builtin method must be resolved before LLVM codegen",
                )),
            };
        }
        if let FunctionCallee::BuiltinPlaceMethod {
            trait_id,
            self_ty,
            receiver,
            ..
        } = callee
        {
            return match trait_id {
                BuiltinTrait::Ptr | BuiltinTrait::PtrMut => {
                    self.emit_builtin_ptr_method(expr.span, *self_ty, receiver)
                }
                _ => Err(self.error(
                    expr.span,
                    "unsupported builtin place method reached LLVM codegen",
                )),
            };
        }
        if let FunctionCallee::BuiltinOperator(operator) = callee {
            return match operator.op {
                FunctionBuiltinOperatorOp::Unary(op) => {
                    let [inner] = args else {
                        return Err(self.error(
                            expr.span,
                            "builtin unary operator reached LLVM codegen with invalid arity",
                        ));
                    };
                    self.emit_unary(expr.span, expr.ty, op, inner)
                }
                FunctionBuiltinOperatorOp::Binary(op) => {
                    let [lhs, rhs] = args else {
                        return Err(self.error(
                            expr.span,
                            "builtin binary operator reached LLVM codegen with invalid arity",
                        ));
                    };
                    let operand_ty = lhs.ty;
                    let rhs_ty = rhs.ty;
                    let lhs = self.emit_expr(lhs)?;
                    let rhs = self.emit_expr(rhs)?;
                    self.emit_binary(expr.span, operand_ty, lhs, op, rhs_ty, rhs)
                }
            };
        }
        match self.module.classify_function_return(expr.ty) {
            AbiReturn::IndirectOut(ty) if !callee_is_extern(self, callee) => {
                let result_ty = self.module.llvm_basic_type(ty, expr.span)?;
                let result_ptr = self
                    .builder
                    .build_alloca(result_ty, "call.out")
                    .map_err(|_| self.error(expr.span, "failed to allocate call result"))?;
                let _ = self.emit_call_raw_with_out(expr, callee, args, Some(result_ptr))?;
                self.builder
                    .build_load(result_ty, result_ptr, "call.result")
                    .map_err(|_| self.error(expr.span, "failed to load call result"))
            }
            AbiReturn::Direct(_) | AbiReturn::IndirectOut(_) => {
                let call = self.emit_call_raw_with_out(expr, callee, args, None)?;
                call.try_as_basic_value()
                    .basic()
                    .ok_or_else(|| self.error(expr.span, "call did not produce a value"))?
                    .map_err(Into::into)
            }
            AbiReturn::Void | AbiReturn::Never => {
                let _ = self.emit_call_raw_with_out(expr, callee, args, None)?;
                Err(self.error(expr.span, "void call cannot be used as a value"))
            }
        }
    }

    fn emit_builtin_len_method(
        &mut self,
        span: Span,
        self_ty: nia_ids::InternedTyId,
        receiver: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        match self.module.ty_kind(self_ty) {
            Some(TyKind::Array { len, .. }) => {
                let len = self.module.array_len(len, span)?;
                Ok(self.module.context.i64_type().const_int(len, false).into())
            }
            Some(TyKind::Slice { .. }) => {
                let slice = self.load_builtin_method_receiver_value(span, self_ty, receiver)?;
                let slice = slice
                    .into_struct_value()
                    .map_err(|_| self.error(span, "`len` receiver is not a slice value"))?;
                self.extract_slice_len(span, slice).map(Into::into)
            }
            _ => Err(self.error(span, "`len` requires an array or slice")),
        }
    }

    fn emit_builtin_ptr_method(
        &mut self,
        span: Span,
        self_ty: nia_ids::InternedTyId,
        receiver: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        match self.module.ty_kind(self_ty) {
            Some(TyKind::Slice { .. }) => {
                let slice = self.load_builtin_method_receiver_value(span, self_ty, receiver)?;
                let slice = slice.into_struct_value().map_err(|_| {
                    self.error(span, "`PtrMut.ptr_mut` receiver is not a slice value")
                })?;
                self.extract_slice_ptr(span, slice).map(Into::into)
            }
            _ => Err(self.error(span, "`PtrMut.ptr_mut` requires a slice")),
        }
    }

    fn load_builtin_method_receiver_value(
        &mut self,
        span: Span,
        self_ty: nia_ids::InternedTyId,
        receiver: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        let receiver_value = self.emit_expr(receiver)?;
        if matches!(self.module.ty_kind(self_ty), Some(TyKind::Slice { .. }))
            && matches!(receiver.kind, FunctionExprKind::AddrOf(_))
        {
            return self
                .builder
                .build_load(
                    self.module.llvm_basic_type(self_ty, span)?,
                    receiver_value.into_pointer_value()?,
                    "builtin.receiver",
                )
                .map_err(|_| self.error(span, "failed to load builtin method receiver"));
        }
        if matches!(
            self.module.ty_kind(receiver.ty),
            Some(TyKind::Pointer { .. })
        ) {
            return self
                .builder
                .build_load(
                    self.module.llvm_basic_type(self_ty, span)?,
                    receiver_value.into_pointer_value()?,
                    "builtin.receiver",
                )
                .map_err(|_| self.error(span, "failed to load builtin method receiver"));
        }
        Ok(receiver_value)
    }

    pub(super) fn emit_call_raw(
        &mut self,
        expr: &FunctionExpr,
        callee: &FunctionCallee,
        args: &[FunctionExpr],
    ) -> Result<nia_llvm::values::CallSiteValue<'ctx>, Diagnostic> {
        self.emit_call_raw_with_out(expr, callee, args, None)
    }

    pub(super) fn emit_call_raw_with_out(
        &mut self,
        expr: &FunctionExpr,
        callee: &FunctionCallee,
        args: &[FunctionExpr],
        out_ptr: Option<nia_llvm::values::PointerValue<'ctx>>,
    ) -> Result<CallSiteValue<'ctx>, Diagnostic> {
        match callee {
            FunctionCallee::Function(def_id) => {
                let Some(function) = self.module.function(*def_id) else {
                    return Err(self.error(expr.span, "missing callee function"));
                };
                let Some(function_item) = self.module.function_item(*def_id) else {
                    return Err(self.error(expr.span, "missing callee function metadata"));
                };
                let llvm_args = if function_item.is_extern {
                    self.emit_c_call_args(args)?
                } else {
                    let param_tys = function_item.params.iter().map(|param| param.passing_ty);
                    self.emit_call_args(expr.span, args, param_tys, out_ptr)?
                };
                self.builder
                    .build_call(function, &llvm_args, "calltmp")
                    .map_err(|_| self.error(expr.span, "failed to build call"))
            }
            FunctionCallee::FunctionInstance {
                def_id,
                arg_module_id,
                self_arg,
                args: type_args,
                const_args,
            } => {
                let Some(instance) = self.module.function_instance_item_with_arg_module(
                    *def_id,
                    *arg_module_id,
                    *self_arg,
                    type_args,
                    const_args,
                ) else {
                    return Err(self.error(expr.span, "missing callee function instance"));
                };
                let llvm_args = if instance.is_extern {
                    self.emit_c_call_args(args)?
                } else {
                    let param_tys = instance.params.iter().map(|param| param.passing_ty);
                    self.emit_call_args(expr.span, args, param_tys, out_ptr)?
                };
                self.builder
                    .build_call(
                        self.module
                            .function_instance_value(
                                instance.def_id,
                                instance.arg_module_id,
                                instance.self_arg,
                                &instance.args,
                                &instance.const_args,
                            )
                            .ok_or_else(|| {
                                self.error(expr.span, "missing callee function instance")
                            })?,
                        &llvm_args,
                        "calltmp",
                    )
                    .map_err(|_| self.error(expr.span, "failed to build call"))
            }
            FunctionCallee::Method {
                def_id,
                arg_module_id,
                self_arg,
                args: type_args,
                receiver_kind,
                receiver,
            } => {
                let (function, is_extern, param_tys) = if self_arg.is_none() && type_args.is_empty()
                {
                    let item = self.module.function_item(*def_id);
                    (
                        self.module.function(*def_id),
                        item.is_some_and(|item| item.is_extern),
                        item.map(|item| {
                            item.params
                                .iter()
                                .map(|param| param.passing_ty)
                                .collect::<Vec<_>>()
                        }),
                    )
                } else {
                    let instance = self.module.function_instance_item_with_arg_module(
                        *def_id,
                        *arg_module_id,
                        *self_arg,
                        type_args,
                        &[],
                    );
                    (
                        instance.and_then(|instance| {
                            self.module.function_instance_value(
                                instance.def_id,
                                instance.arg_module_id,
                                instance.self_arg,
                                &instance.args,
                                &instance.const_args,
                            )
                        }),
                        instance.is_some_and(|instance| instance.is_extern),
                        instance.map(|instance| {
                            instance
                                .params
                                .iter()
                                .map(|param| param.passing_ty)
                                .collect::<Vec<_>>()
                        }),
                    )
                };
                let Some(function) = function else {
                    return Err(self.error(
                        expr.span,
                        format!(
                            "missing method function for def {:?} in arg module {:?} with args {:?}",
                            def_id, arg_module_id, type_args
                        ),
                    ));
                };
                let Some(param_tys) = param_tys else {
                    return Err(self.error(expr.span, "missing method metadata"));
                };
                let llvm_args = if is_extern {
                    let mut call_args = Vec::with_capacity(args.len() + 1);
                    call_args.push(receiver.as_ref());
                    call_args.extend(args.iter());
                    self.emit_c_call_args_refs(&call_args)?
                } else {
                    let mut llvm_args = Vec::new();
                    if let Some(out_ptr) = out_ptr {
                        llvm_args.push(out_ptr.into());
                    }
                    let receiver_ty = param_tys.first().copied().ok_or_else(|| {
                        self.error(expr.span, "method metadata is missing receiver parameter")
                    })?;
                    match self
                        .module
                        .classify_function_params(std::iter::once(receiver_ty))
                        .into_iter()
                        .next()
                        .unwrap_or(AbiParam::Omit)
                    {
                        AbiParam::Direct(_) => llvm_args.push(self.emit_method_receiver_arg(
                            *receiver_kind,
                            receiver_ty,
                            receiver,
                        )?),
                        AbiParam::IndirectReadonly(_) => {
                            llvm_args.push(self.emit_arg_address(expr.span, receiver)?.into())
                        }
                        AbiParam::Omit => match receiver_kind {
                            ReceiverKind::Value => self.emit_effect_expr(receiver)?,
                            ReceiverKind::RefReadOnly | ReceiverKind::Ref => {
                                return Err(self.error(
                                    expr.span,
                                    "by-reference method receiver was omitted by ABI classification",
                                ));
                            }
                        },
                    }
                    llvm_args.extend(self.emit_call_args(
                        expr.span,
                        args,
                        param_tys.into_iter().skip(1),
                        None,
                    )?);
                    llvm_args
                };
                self.builder
                    .build_call(function, &llvm_args, "calltmp")
                    .map_err(|_| self.error(expr.span, "failed to build method call"))
            }
            FunctionCallee::TraitMethod { .. } => Err(self.error(
                expr.span,
                "unresolved trait method call reached LLVM codegen",
            )),
            FunctionCallee::TraitAssociatedFunction { .. } => Err(self.error(
                expr.span,
                "unresolved trait associated function call reached LLVM codegen",
            )),
            FunctionCallee::DynamicTraitMethod {
                object_ty,
                trait_id,
                method_id,
                slot,
                params,
                return_type,
                receiver,
                ..
            } => self.emit_dynamic_trait_method_call(DynamicTraitMethodCall {
                expr,
                object_ty: *object_ty,
                trait_id: *trait_id,
                method_id: *method_id,
                slot: *slot,
                params,
                return_type: *return_type,
                receiver,
                args,
                out_ptr,
            }),
            FunctionCallee::BuiltinPlaceMethod { .. } => Err(self.error(
                expr.span,
                "unresolved builtin place method call reached LLVM codegen",
            )),
            FunctionCallee::BuiltinMethod { .. } => {
                Err(self.error(expr.span, "builtin method cannot be emitted as a raw call"))
            }
            FunctionCallee::BuiltinOperator(_) => Err(self.error(
                expr.span,
                "builtin operator cannot be emitted as a raw call",
            )),
            FunctionCallee::FunctionPointer(callee) => {
                let Some(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                }) = self.module.ty_kind(callee.ty)
                else {
                    return Err(self.error(callee.span, "callee is not a function pointer"));
                };
                let function_type = self.module.function_pointer_type_in(
                    params,
                    *return_type,
                    *is_variadic,
                    callee.span,
                )?;
                let function_pointer = self.emit_expr(callee)?.into_pointer_value()?;
                let llvm_args =
                    self.emit_call_args(expr.span, args, params.iter().copied(), out_ptr)?;
                self.builder
                    .build_indirect_call(function_type, function_pointer, &llvm_args, "calltmp")
                    .map_err(|_| self.error(expr.span, "failed to build indirect call"))
            }
        }
    }

    fn emit_dynamic_trait_method_call(
        &mut self,
        call: DynamicTraitMethodCall<'_, 'ctx>,
    ) -> Result<CallSiteValue<'ctx>, Diagnostic> {
        let receiver_value = self
            .emit_expr(call.receiver)?
            .into_struct_value()
            .map_err(|_| {
                self.error(
                    call.expr.span,
                    "dynamic trait receiver is not a trait object",
                )
            })?;
        let object_ptr = self
            .builder
            .build_extract_value(receiver_value, 0, "traitobj.ptr")
            .map_err(|_| self.error(call.expr.span, "failed to extract trait object pointer"))?;
        let metadata = self
            .builder
            .build_extract_value(receiver_value, 1, "traitobj.vtable")
            .map_err(|_| self.error(call.expr.span, "failed to extract trait object vtable"))?
            .into_pointer_value()?;
        let slot = self.module.trait_object_method_slot(
            call.object_ty,
            call.trait_id,
            call.method_id,
            call.slot,
        );
        let ptr_ty = self.module.context.ptr_type(Default::default());
        let zero = self.module.context.i64_type().const_int(0, false);
        let slot_index = self.module.context.i64_type().const_int(slot as u64, false);
        let entry_ptr = unsafe {
            self.builder
                .build_gep(
                    ptr_ty.array_type((slot + 1) as u32),
                    metadata,
                    &[zero, slot_index],
                    "vtable.slot",
                )
                .map_err(|_| self.error(call.expr.span, "failed to load vtable slot"))?
        };
        let function_pointer = self
            .builder
            .build_load(ptr_ty, entry_ptr, "vtable.fn")
            .map_err(|_| self.error(call.expr.span, "failed to load vtable function"))?
            .into_pointer_value()?;
        let function_type = self.module.dynamic_trait_method_type(
            call.object_ty,
            call.params,
            call.return_type,
            call.expr.span,
        )?;
        let mut llvm_args = Vec::new();
        if let Some(out_ptr) = call.out_ptr {
            llvm_args.push(out_ptr.into());
        }
        llvm_args.push(object_ptr);
        llvm_args.extend(self.emit_call_args(
            call.expr.span,
            call.args,
            call.params.iter().copied(),
            None,
        )?);
        self.builder
            .build_indirect_call(function_type, function_pointer, &llvm_args, "calltmp")
            .map_err(|_| self.error(call.expr.span, "failed to build dynamic trait call"))
    }

    fn emit_call_args(
        &mut self,
        span: Span,
        args: &[FunctionExpr],
        param_tys: impl IntoIterator<Item = InternedTyId>,
        out_ptr: Option<nia_llvm::values::PointerValue<'ctx>>,
    ) -> Result<Vec<BasicValueEnum<'ctx>>, Diagnostic> {
        let mut llvm_args = Vec::new();
        if let Some(out_ptr) = out_ptr {
            llvm_args.push(out_ptr.into());
        }
        for (arg, classification) in args
            .iter()
            .zip(self.module.classify_function_params(param_tys))
        {
            match classification {
                AbiParam::Direct(ty)
                    if matches!(self.module.ty_kind(ty), Some(TyKind::Pointer { .. }))
                        && !matches!(self.module.ty_kind(arg.ty), Some(TyKind::Pointer { .. })) =>
                {
                    llvm_args.push(self.emit_arg_address(span, arg)?.into())
                }
                AbiParam::Direct(_) => llvm_args.push(self.emit_expr(arg)?),
                AbiParam::IndirectReadonly(_) => {
                    llvm_args.push(self.emit_arg_address(span, arg)?.into())
                }
                AbiParam::Omit => self.emit_effect_expr(arg)?,
            }
        }
        Ok(llvm_args)
    }

    fn emit_method_receiver_arg(
        &mut self,
        receiver_kind: ReceiverKind,
        passing_ty: InternedTyId,
        receiver: &FunctionExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        match receiver_kind {
            ReceiverKind::Value => self.emit_expr(receiver),
            ReceiverKind::RefReadOnly | ReceiverKind::Ref => {
                if receiver.ty == passing_ty
                    && matches!(
                        self.module.ty_kind(receiver.ty),
                        Some(
                            TyKind::Pointer { .. }
                                | TyKind::VolatilePointer { .. }
                                | TyKind::FunctionPointer { .. }
                                | TyKind::Slice { .. }
                                | TyKind::TraitObject { .. }
                        )
                    )
                {
                    return self.emit_expr(receiver);
                }
                if self.is_fat_receiver_passing_ty(passing_ty) {
                    let value = self.emit_expr(receiver)?;
                    if self.receiver_points_to_passing_ty(receiver.ty, passing_ty) {
                        let ty = self.module.llvm_basic_type(passing_ty, receiver.span)?;
                        return self
                            .builder
                            .build_load(ty, value.into_pointer_value()?, "loadreceiver")
                            .map_err(|_| {
                                self.error(receiver.span, "failed to load fat method receiver")
                            });
                    }
                    return Ok(value);
                }
                if matches!(
                    self.module.ty_kind(receiver.ty),
                    Some(TyKind::Pointer { .. })
                ) && self.pointer_receiver_matches_passing_ty(receiver.ty, passing_ty)
                {
                    return self.emit_expr(receiver);
                }
                if is_addressable_receiver(receiver) {
                    return Ok(self.emit_addr_of(receiver)?.into());
                }
                Ok(self.emit_arg_address(receiver.span, receiver)?.into())
            }
        }
    }

    fn receiver_points_to_passing_ty(
        &self,
        receiver_ty: InternedTyId,
        passing_ty: InternedTyId,
    ) -> bool {
        let Some(TyKind::Pointer { elem, .. }) = self.module.ty_kind(receiver_ty) else {
            return false;
        };
        self.module.same_type_for_equiv(*elem, passing_ty)
    }

    fn pointer_receiver_matches_passing_ty(
        &self,
        receiver_ty: InternedTyId,
        passing_ty: InternedTyId,
    ) -> bool {
        if self.module.same_type_for_equiv(receiver_ty, passing_ty) {
            return true;
        }
        let (
            Some(TyKind::Pointer {
                is_readonly: false,
                elem: receiver_elem,
            }),
            Some(TyKind::Pointer {
                is_readonly: true,
                elem: passing_elem,
            }),
        ) = (
            self.module.ty_kind(receiver_ty),
            self.module.ty_kind(passing_ty),
        )
        else {
            return false;
        };
        self.module
            .same_type_for_equiv(*receiver_elem, *passing_elem)
    }

    fn is_fat_receiver_passing_ty(&self, ty: InternedTyId) -> bool {
        matches!(
            self.module.ty_kind(ty),
            Some(TyKind::Slice { .. } | TyKind::TraitObject { .. })
        )
    }

    fn emit_c_call_args(
        &mut self,
        args: &[FunctionExpr],
    ) -> Result<Vec<BasicValueEnum<'ctx>>, Diagnostic> {
        args.iter().map(|arg| self.emit_expr(arg)).collect()
    }

    fn emit_c_call_args_refs(
        &mut self,
        args: &[&FunctionExpr],
    ) -> Result<Vec<BasicValueEnum<'ctx>>, Diagnostic> {
        args.iter().map(|arg| self.emit_expr(arg)).collect()
    }

    fn emit_arg_address(
        &mut self,
        span: Span,
        arg: &FunctionExpr,
    ) -> Result<nia_llvm::values::PointerValue<'ctx>, Diagnostic> {
        match &arg.kind {
            FunctionExprKind::Local(local_id) if self.is_zero_sized(arg.ty) => {
                self.local_addr(*local_id, span)
            }
            _ if self.is_zero_sized(arg.ty) => self
                .builder
                .build_alloca(self.module.context.i8_type(), "zst.arg")
                .map_err(|_| self.error(span, "failed to allocate zero-sized argument")),
            FunctionExprKind::AddrOf(place) => self.emit_typed_place_addr(place),
            FunctionExprKind::ArrayLiteral { elems } => {
                let ty = self.module.llvm_basic_type(arg.ty, span)?;
                let ptr = self
                    .builder
                    .build_alloca(ty, "arg.copy")
                    .map_err(|_| self.error(span, "failed to allocate indirect argument"))?;
                self.emit_array_literal_into(arg, elems, ptr)?;
                Ok(ptr)
            }
            FunctionExprKind::StructLiteral { fields, .. } => {
                let ty = self.module.llvm_basic_type(arg.ty, span)?;
                let ptr = self
                    .builder
                    .build_alloca(ty, "arg.copy")
                    .map_err(|_| self.error(span, "failed to allocate indirect argument"))?;
                self.emit_struct_literal_into(arg, fields, ptr)?;
                Ok(ptr)
            }
            FunctionExprKind::UnionLiteral { field, .. } => {
                let ty = self.module.llvm_basic_type(arg.ty, span)?;
                let ptr = self
                    .builder
                    .build_alloca(ty, "arg.copy")
                    .map_err(|_| self.error(span, "failed to allocate indirect argument"))?;
                self.emit_union_literal_into(arg, field, ptr)?;
                Ok(ptr)
            }
            FunctionExprKind::Call { callee, args } if self.call_returns_indirect_out(arg) => {
                let ty = self.module.llvm_basic_type(arg.ty, span)?;
                let ptr = self
                    .builder
                    .build_alloca(ty, "arg.copy")
                    .map_err(|_| self.error(span, "failed to allocate indirect argument"))?;
                let _ = self.emit_call_raw_with_out(arg, callee, args, Some(ptr))?;
                Ok(ptr)
            }
            _ => {
                let ty = self.module.llvm_basic_type(arg.ty, span)?;
                let ptr = self
                    .builder
                    .build_alloca(ty, "arg.copy")
                    .map_err(|_| self.error(span, "failed to allocate indirect argument"))?;
                let value = self.emit_expr(arg)?;
                self.builder
                    .build_store(ptr, value)
                    .map_err(|_| self.error(span, "failed to store indirect argument"))?;
                Ok(ptr)
            }
        }
    }

    fn call_returns_indirect_out(&self, expr: &FunctionExpr) -> bool {
        matches!(
            self.module.classify_function_return(expr.ty),
            AbiReturn::IndirectOut(_)
        )
    }
}

fn is_addressable_receiver(receiver: &FunctionExpr) -> bool {
    matches!(
        receiver.kind,
        FunctionExprKind::AddrOf(_)
            | FunctionExprKind::Global(_)
            | FunctionExprKind::Local(_)
            | FunctionExprKind::Unary {
                op: nia_ast::UnaryOp::Deref,
                ..
            }
            | FunctionExprKind::Field { .. }
            | FunctionExprKind::Index { .. }
            | FunctionExprKind::StaticArrayPointer { .. }
    )
}
