// SPDX-License-Identifier: GPL-3.0-or-later
use nia_backend_ir::{TypedCallee, TypedExpr, TypedExprKind};
use nia_diagnostic::Diagnostic;
use nia_llvm::values::BasicValueEnum;
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
        let call = self.emit_call_raw(expr, callee, args)?;
        call.try_as_basic_value()
            .basic()
            .ok_or_else(|| self.error(expr.span, "void call cannot be used as a value"))
    }

    pub(super) fn emit_call_raw(
        &mut self,
        expr: &TypedExpr,
        callee: &TypedCallee,
        args: &[TypedExpr],
    ) -> Result<nia_llvm::values::CallSiteValue<'ctx>, Diagnostic> {
        match callee {
            TypedCallee::Function(def_id) => {
                let Some(function) = self.module.function(*def_id) else {
                    return Err(self.error(expr.span, "missing callee function"));
                };
                let mut llvm_args = Vec::new();
                for arg in args {
                    llvm_args.push(self.emit_expr(arg)?);
                }
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
                let mut llvm_args = Vec::new();
                for arg in args {
                    llvm_args.push(self.emit_expr(arg)?);
                }
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
                let function = if type_args.is_empty() {
                    self.module.function(*def_id)
                } else {
                    self.module
                        .function_instance_item(*def_id, type_args)
                        .and_then(|instance| {
                            self.module
                                .function_instance_value(instance.def_id, &instance.args)
                        })
                };
                let Some(function) = function else {
                    return Err(self.error(expr.span, "missing method function"));
                };
                let mut llvm_args = Vec::with_capacity(args.len() + 1);
                llvm_args.push(self.emit_method_receiver(receiver)?);
                for arg in args {
                    llvm_args.push(self.emit_expr(arg)?);
                }
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
                let mut llvm_args = Vec::new();
                for arg in args {
                    llvm_args.push(self.emit_expr(arg)?);
                }
                self.builder
                    .build_indirect_call(function_type, function_pointer, &llvm_args, "calltmp")
                    .map_err(|_| self.error(expr.span, "failed to build indirect call"))
            }
        }
    }

    fn emit_method_receiver(
        &mut self,
        receiver: &TypedExpr,
    ) -> Result<BasicValueEnum<'ctx>, Diagnostic> {
        match self.module.interner().get(receiver.ty) {
            Some(TyKind::Pointer { .. }) => self.emit_expr(receiver),
            _ => Ok(self.emit_addr_of(receiver)?.into()),
        }
    }
}
