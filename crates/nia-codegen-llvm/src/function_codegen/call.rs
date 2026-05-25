// SPDX-License-Identifier: GPL-3.0-or-later
use crate::module_codegen::{AbiParam, AbiReturn};
use nia_backend_ir::{TypedCallee, TypedExpr, TypedExprKind};
use nia_diagnostic::Diagnostic;
use nia_llvm::values::{BasicValueEnum, CallSiteValue};
use nia_span::Span;
use nia_ty::TyKind;

use super::FunctionCodegen;

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn emit_function_pointer(
        &mut self,
        span: Span,
        expr: &TypedExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        match &expr.kind {
            TypedExprKind::Function(def_id) => {
                let Some(function) = self.module.function(*def_id) else {
                    return Err(self.error(span, "missing function item"));
                };
                Ok(function.as_global_value().as_pointer_value().into())
            }
            TypedExprKind::FunctionInstance { def_id, args } => {
                let Some(instance) = self.module.function_instance_item(*def_id, args) else {
                    return Err(self.error(span, "missing function instance item"));
                };
                let Some(function) = self
                    .module
                    .function_instance_value(instance.def_id, &instance.args)
                else {
                    return Err(self.error(span, "missing function instance value"));
                };
                Ok(function.as_global_value().as_pointer_value().into())
            }
            _ => Err(self.error(span, "expression is not a function item")),
        }
    }

    pub(super) fn emit_call(
        &mut self,
        expr: &TypedExpr,
        callee: &TypedCallee,
        args: &[TypedExpr],
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        match self.module.classify_function_return(expr.ty) {
            AbiReturn::IndirectOut(ty) => {
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
            AbiReturn::Direct(_) => {
                let call = self.emit_call_raw_with_out(expr, callee, args, None)?;
                call.try_as_basic_value()
                    .basic()
                    .ok_or_else(|| self.error(expr.span, "call did not produce a value"))
            }
            AbiReturn::Void | AbiReturn::Never => {
                let _ = self.emit_call_raw_with_out(expr, callee, args, None)?;
                Err(self.error(expr.span, "void call cannot be used as a value"))
            }
        }
    }

    pub(super) fn emit_call_raw(
        &mut self,
        expr: &TypedExpr,
        callee: &TypedCallee,
        args: &[TypedExpr],
    ) -> Result<nia_llvm::values::CallSiteValue<'ctx>, Diagnostic> {
        self.emit_call_raw_with_out(expr, callee, args, None)
    }

    fn emit_call_raw_with_out(
        &mut self,
        expr: &TypedExpr,
        callee: &TypedCallee,
        args: &[TypedExpr],
        out_ptr: Option<nia_llvm::values::PointerValue<'ctx>>,
    ) -> Result<CallSiteValue<'ctx>, Diagnostic> {
        match callee {
            TypedCallee::Function(def_id) => {
                let Some(function) = self.module.function(*def_id) else {
                    return Err(self.error(expr.span, "missing callee function"));
                };
                let Some(function_item) = self.module.function_item(*def_id) else {
                    return Err(self.error(expr.span, "missing callee function metadata"));
                };
                let llvm_args = if function_item.is_extern {
                    self.emit_c_call_args(args)?
                } else {
                    let call_args = args.iter().collect::<Vec<_>>();
                    self.emit_call_args(expr.span, &call_args, out_ptr)?
                };
                self.builder
                    .build_call(function, &llvm_args, "calltmp")
                    .map_err(|_| self.error(expr.span, "failed to build call"))
            }
            TypedCallee::FunctionInstance {
                def_id,
                args: type_args,
            } => {
                let Some(instance) = self.module.function_instance_item(*def_id, type_args) else {
                    return Err(self.error(expr.span, "missing callee function instance"));
                };
                let llvm_args = if instance.is_extern {
                    self.emit_c_call_args(args)?
                } else {
                    let call_args = args.iter().collect::<Vec<_>>();
                    self.emit_call_args(expr.span, &call_args, out_ptr)?
                };
                self.builder
                    .build_call(
                        self.module
                            .function_instance_value(instance.def_id, &instance.args)
                            .ok_or_else(|| {
                                self.error(expr.span, "missing callee function instance")
                            })?,
                        &llvm_args,
                        "calltmp",
                    )
                    .map_err(|_| self.error(expr.span, "failed to build call"))
            }
            TypedCallee::Method {
                def_id,
                args: type_args,
                receiver,
            } => {
                let (function, is_extern) = if type_args.is_empty() {
                    (
                        self.module.function(*def_id),
                        self.module
                            .function_item(*def_id)
                            .is_some_and(|item| item.is_extern),
                    )
                } else {
                    let instance = self.module.function_instance_item(*def_id, type_args);
                    (
                        instance.and_then(|instance| {
                            self.module
                                .function_instance_value(instance.def_id, &instance.args)
                        }),
                        instance.is_some_and(|instance| instance.is_extern),
                    )
                };
                let Some(function) = function else {
                    return Err(self.error(expr.span, "missing method function"));
                };
                let mut call_args = Vec::with_capacity(args.len() + 1);
                call_args.push(receiver.as_ref());
                call_args.extend(args.iter());
                let llvm_args = if is_extern {
                    self.emit_c_call_args_refs(&call_args)?
                } else {
                    self.emit_call_args(expr.span, &call_args, out_ptr)?
                };
                self.builder
                    .build_call(function, &llvm_args, "calltmp")
                    .map_err(|_| self.error(expr.span, "failed to build method call"))
            }
            TypedCallee::FunctionPointer(callee) => {
                let Some(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                }) = self.module.interner().get(callee.ty)
                else {
                    return Err(self.error(callee.span, "callee is not a function pointer"));
                };
                let function_type = self.module.function_pointer_type_in(
                    self.module.interner(),
                    &self.module.source.layouts,
                    params,
                    *return_type,
                    *is_variadic,
                    callee.span,
                )?;
                let function_pointer = self.emit_expr(callee)?.into_pointer_value();
                let call_args = args.iter().collect::<Vec<_>>();
                let llvm_args = self.emit_call_args(expr.span, &call_args, out_ptr)?;
                self.builder
                    .build_indirect_call(function_type, function_pointer, &llvm_args, "calltmp")
                    .map_err(|_| self.error(expr.span, "failed to build indirect call"))
            }
        }
    }

    fn emit_call_args(
        &mut self,
        span: Span,
        args: &[&TypedExpr],
        out_ptr: Option<nia_llvm::values::PointerValue<'ctx>>,
    ) -> Result<Vec<BasicValueEnum<'ctx>>, Diagnostic> {
        let mut llvm_args = Vec::new();
        if let Some(out_ptr) = out_ptr {
            llvm_args.push(out_ptr.into());
        }
        let arg_tys = args.iter().map(|arg| arg.ty).collect::<Vec<_>>();
        for (arg, classification) in args
            .iter()
            .zip(self.module.classify_function_params(&arg_tys))
        {
            match classification {
                AbiParam::Direct(_) => llvm_args.push(self.emit_expr(arg)?),
                AbiParam::IndirectReadonly(_) => {
                    llvm_args.push(self.emit_arg_address(span, arg)?.into())
                }
                AbiParam::Omit => self.emit_zero_sized_expr(arg)?,
            }
        }
        Ok(llvm_args)
    }

    fn emit_c_call_args(
        &mut self,
        args: &[TypedExpr],
    ) -> Result<Vec<BasicValueEnum<'ctx>>, Diagnostic> {
        let args = args.iter().collect::<Vec<_>>();
        self.emit_c_call_args_refs(&args)
    }

    fn emit_c_call_args_refs(
        &mut self,
        args: &[&TypedExpr],
    ) -> Result<Vec<BasicValueEnum<'ctx>>, Diagnostic> {
        args.iter().map(|arg| self.emit_expr(arg)).collect()
    }

    fn emit_arg_address(
        &mut self,
        span: Span,
        arg: &TypedExpr,
    ) -> Result<nia_llvm::values::PointerValue<'ctx>, Diagnostic> {
        match &arg.kind {
            TypedExprKind::Global(_)
            | TypedExprKind::Local(_)
            | TypedExprKind::Unary {
                op: nia_ast::UnaryOp::Deref,
                ..
            }
            | TypedExprKind::Field { .. }
            | TypedExprKind::Index { .. } => self.emit_addr_of(arg),
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
}
