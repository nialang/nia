// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::ModuleLowerer;
use nia_backend_ir::{BackendFunction, BackendParam};
use nia_defs::VisibleExtensionTarget;
use nia_function_ir::{
    FunctionArrayElements, FunctionAsmInput, FunctionAsmOutput, FunctionBinding, FunctionBody,
    FunctionCallee, FunctionDeferBody, FunctionExpr, FunctionExprKind, FunctionFieldInit,
    FunctionForHeader, FunctionInlineAsm, FunctionLocal, FunctionOp, FunctionPlace,
    FunctionPlaceBase, FunctionPlaceElem, FunctionRange, FunctionSliceRange, FunctionTerminator,
};
use nia_ids::{BuiltinTrait, BuiltinTraitMethod, GlobalDefId, InternedTyId, TraitId};
use nia_ty::{LayoutBuiltin, PrimitiveTy, TyKind};

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn generic_substitutions(
        &self,
        generics: &[String],
        args: &[InternedTyId],
    ) -> HashMap<String, InternedTyId> {
        generics.iter().cloned().zip(args.iter().copied()).collect()
    }

    pub(crate) fn effective_generics(
        &self,
        def_id: GlobalDefId,
        own_generics: &[String],
    ) -> Vec<String> {
        if self
            .input
            .defs
            .defs
            .get(def_id.def_id)
            .is_some_and(|def| def.kind == nia_defs::DefKind::TraitMethod)
        {
            let mut generics = vec!["Self".to_string()];
            generics.extend(
                self.input
                    .defs
                    .defs
                    .get(def_id.def_id)
                    .and_then(|def| def.parent)
                    .and_then(|parent| self.input.defs.defs.get(parent))
                    .map(|parent| parent.generics.clone())
                    .unwrap_or_default(),
            );
            generics.extend(own_generics.iter().cloned());
            return generics;
        }
        let mut generics = self.extension_target_generics(def_id).unwrap_or_else(|| {
            self.input
                .defs
                .defs
                .get(def_id.def_id)
                .and_then(|def| def.parent)
                .and_then(|parent| self.input.defs.defs.get(parent))
                .map(|parent| parent.generics.clone())
                .unwrap_or_default()
        });
        generics.extend(own_generics.iter().cloned());
        generics
    }

    fn extension_target_generics(&self, def_id: GlobalDefId) -> Option<Vec<String>> {
        self.input
            .extensions
            .targets()
            .iter()
            .find(|target| target.methods.iter().any(|method| method.def_id == def_id))
            .map(|target| self.generic_params_in_ty(target.target_ty))
    }

    pub(crate) fn generic_params_in_ty(&self, ty: InternedTyId) -> Vec<String> {
        let mut generics = Vec::new();
        self.collect_generic_params_in_ty(ty, &mut generics);
        generics
    }

    pub(crate) fn collect_generic_params_in_ty(
        &self,
        ty: InternedTyId,
        generics: &mut Vec<String>,
    ) {
        match self.ty_kind(ty) {
            Some(TyKind::GenericParam(name)) => {
                if !generics.contains(name) {
                    generics.push(name.clone());
                }
            }
            Some(TyKind::Pointer { elem, .. } | TyKind::Slice { elem, .. }) => {
                self.collect_generic_params_in_ty(*elem, generics);
            }
            Some(TyKind::Array { elem, .. }) => {
                self.collect_generic_params_in_ty(*elem, generics);
            }
            Some(TyKind::Range { bound, .. }) => {
                if let Some(bound) = bound {
                    self.collect_generic_params_in_ty(*bound, generics);
                }
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                for param in params {
                    self.collect_generic_params_in_ty(*param, generics);
                }
                self.collect_generic_params_in_ty(*return_type, generics);
            }
            Some(TyKind::Nominal { args, .. }) => {
                for arg in args {
                    self.collect_generic_params_in_ty(*arg, generics);
                }
            }
            Some(TyKind::BuiltinTrait { args, .. }) => {
                for arg in args {
                    self.collect_generic_params_in_ty(*arg, generics);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.collect_generic_params_in_ty(*self_ty, generics);
                for arg in trait_args {
                    self.collect_generic_params_in_ty(*arg, generics);
                }
            }
            Some(TyKind::Error | TyKind::Primitive(_)) | None => {}
        }
    }

    pub(crate) fn effective_generic_substitutions(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
    ) -> HashMap<String, InternedTyId> {
        let own_generics = self
            .input
            .defs
            .defs
            .get(def_id.def_id)
            .map(|def| def.generics.as_slice())
            .unwrap_or(&[]);
        let generics = self.effective_generics(def_id, own_generics);
        self.generic_substitutions(&generics, args)
    }

    pub(crate) fn instantiate_params(
        &mut self,
        function: &BackendFunction,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Vec<BackendParam> {
        function
            .params
            .iter()
            .map(|param| BackendParam {
                local_id: param.local_id,
                name: param.name.clone(),
                receiver: param.receiver,
                ty: self.instantiate_ty(param.ty, substitutions),
                span: param.span,
            })
            .collect()
    }

    pub(crate) fn instantiate_function_body(
        &mut self,
        body: FunctionBody,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> FunctionBody {
        let body = FunctionBody {
            span: body.span,
            locals: body
                .locals
                .into_iter()
                .map(|local| FunctionLocal {
                    id: local.id,
                    name: local.name,
                    kind: local.kind,
                    ty: self.instantiate_ty(local.ty, substitutions),
                    span: local.span,
                })
                .collect(),
            scopes: body.scopes,
            blocks: body
                .blocks
                .into_iter()
                .map(|block| nia_function_ir::FunctionBlock {
                    id: block.id,
                    scope: block.scope,
                    span: block.span,
                    ops: block
                        .ops
                        .into_iter()
                        .map(|op| self.instantiate_op(op, substitutions))
                        .collect(),
                    terminator: self.instantiate_terminator(block.terminator, substitutions),
                })
                .collect(),
            entry: body.entry,
            ty: self.instantiate_ty(body.ty, substitutions),
        };
        self.resolve_builtin_operator_calls_in_body(body)
    }

    fn instantiate_defer_body(
        &mut self,
        body: FunctionDeferBody,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> FunctionDeferBody {
        FunctionDeferBody {
            span: body.span,
            scopes: body.scopes,
            blocks: body
                .blocks
                .into_iter()
                .map(|block| nia_function_ir::FunctionBlock {
                    id: block.id,
                    scope: block.scope,
                    span: block.span,
                    ops: block
                        .ops
                        .into_iter()
                        .map(|op| self.instantiate_op(op, substitutions))
                        .collect(),
                    terminator: self.instantiate_terminator(block.terminator, substitutions),
                })
                .collect(),
            entry: body.entry,
        }
    }

    fn instantiate_op(
        &mut self,
        op: FunctionOp,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> FunctionOp {
        match op {
            FunctionOp::Binding(binding) => {
                FunctionOp::Binding(self.instantiate_binding(binding, substitutions))
            }
            FunctionOp::StoreLocal {
                local_id,
                value,
                span,
            } => FunctionOp::StoreLocal {
                local_id,
                value: self.instantiate_expr(value, substitutions),
                span,
            },
            FunctionOp::Expr(expr) => FunctionOp::Expr(self.instantiate_expr(expr, substitutions)),
            FunctionOp::Defer(body) => {
                FunctionOp::Defer(self.instantiate_defer_body(body, substitutions))
            }
        }
    }

    fn instantiate_binding(
        &mut self,
        binding: FunctionBinding,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> FunctionBinding {
        FunctionBinding {
            local_id: binding.local_id,
            name: binding.name,
            ty: self.instantiate_ty(binding.ty, substitutions),
            value: binding
                .value
                .map(|value| self.instantiate_expr(value, substitutions)),
            is_const: binding.is_const,
        }
    }

    fn instantiate_terminator(
        &mut self,
        terminator: FunctionTerminator,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> FunctionTerminator {
        match terminator {
            FunctionTerminator::Error { span } => FunctionTerminator::Error { span },
            FunctionTerminator::Branch { target, span } => {
                FunctionTerminator::Branch { target, span }
            }
            FunctionTerminator::Next { target, span } => FunctionTerminator::Next { target, span },
            FunctionTerminator::If {
                cond,
                then_target,
                else_target,
                span,
            } => FunctionTerminator::If {
                cond: self.instantiate_expr(cond, substitutions),
                then_target,
                else_target,
                span,
            },
            FunctionTerminator::Switch {
                target,
                arms,
                default,
                fallback,
                span,
            } => FunctionTerminator::Switch {
                target: self.instantiate_expr(target, substitutions),
                arms: arms
                    .into_iter()
                    .map(|arm| nia_function_ir::FunctionSwitchArm {
                        pattern: self.instantiate_expr(arm.pattern, substitutions),
                        target: arm.target,
                    })
                    .collect(),
                default,
                fallback,
                span,
            },
            FunctionTerminator::Loop {
                header,
                body,
                continue_target,
                break_target,
                span,
            } => FunctionTerminator::Loop {
                header: self.instantiate_for_header(header, substitutions),
                body,
                continue_target,
                break_target,
                span,
            },
            FunctionTerminator::Return { value, span } => FunctionTerminator::Return {
                value: value.map(|value| self.instantiate_expr(value, substitutions)),
                span,
            },
            FunctionTerminator::Tail { value, span } => FunctionTerminator::Tail {
                value: value.map(|value| self.instantiate_expr(value, substitutions)),
                span,
            },
        }
    }

    fn instantiate_for_header(
        &mut self,
        header: FunctionForHeader,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> FunctionForHeader {
        match header {
            FunctionForHeader::Infinite => FunctionForHeader::Infinite,
            FunctionForHeader::Condition(expr) => {
                FunctionForHeader::Condition(self.instantiate_expr(expr, substitutions))
            }
            FunctionForHeader::CStyle { cond } => FunctionForHeader::CStyle {
                cond: cond.map(|cond| Box::new(self.instantiate_expr(*cond, substitutions))),
            },
        }
    }

    fn instantiate_expr(
        &mut self,
        expr: FunctionExpr,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> FunctionExpr {
        FunctionExpr {
            span: expr.span,
            ty: self.instantiate_ty(expr.ty, substitutions),
            kind: match expr.kind {
                FunctionExprKind::Error => FunctionExprKind::Error,
                FunctionExprKind::Integer(text) => FunctionExprKind::Integer(text),
                FunctionExprKind::Float(text) => FunctionExprKind::Float(text),
                FunctionExprKind::String(scalars) => FunctionExprKind::String(scalars),
                FunctionExprKind::ByteString(bytes) => FunctionExprKind::ByteString(bytes),
                FunctionExprKind::Char(value) => FunctionExprKind::Char(value),
                FunctionExprKind::ByteChar(text) => FunctionExprKind::ByteChar(text),
                FunctionExprKind::Bool(value) => FunctionExprKind::Bool(value),
                FunctionExprKind::Local(local) => FunctionExprKind::Local(local),
                FunctionExprKind::Global(def_id) => FunctionExprKind::Global(def_id),
                FunctionExprKind::Function(def_id) => FunctionExprKind::Function(def_id),
                FunctionExprKind::FunctionInstance { def_id, args } => {
                    FunctionExprKind::FunctionInstance {
                        def_id,
                        args: args
                            .into_iter()
                            .map(|arg| self.instantiate_ty(arg, substitutions))
                            .collect(),
                    }
                }
                FunctionExprKind::EnumVariant(def_id) => FunctionExprKind::EnumVariant(def_id),
                FunctionExprKind::BuiltinValue(value) => FunctionExprKind::BuiltinValue(
                    self.instantiate_builtin_value(value, substitutions),
                ),
                FunctionExprKind::Range(range) => {
                    FunctionExprKind::Range(self.instantiate_range(range, substitutions))
                }
                FunctionExprKind::InlineAsm(asm) => {
                    FunctionExprKind::InlineAsm(FunctionInlineAsm {
                        code: asm.code,
                        inputs: asm
                            .inputs
                            .into_iter()
                            .map(|input| FunctionAsmInput {
                                constraint: input.constraint,
                                value: self.instantiate_expr(input.value, substitutions),
                                span: input.span,
                            })
                            .collect(),
                        outputs: asm
                            .outputs
                            .into_iter()
                            .map(|output| FunctionAsmOutput {
                                constraint: output.constraint,
                                place: self.instantiate_place(output.place, substitutions),
                                span: output.span,
                            })
                            .collect(),
                        clobbers: asm.clobbers,
                        options: asm.options,
                    })
                }
                FunctionExprKind::CStringPointer { array, is_const } => {
                    FunctionExprKind::CStringPointer {
                        array: Box::new(self.instantiate_expr(*array, substitutions)),
                        is_const,
                    }
                }
                FunctionExprKind::ArrayLiteral { elems } => FunctionExprKind::ArrayLiteral {
                    elems: self.instantiate_array_elements(elems, substitutions),
                },
                FunctionExprKind::StructLiteral { def_id, fields } => {
                    FunctionExprKind::StructLiteral {
                        def_id,
                        fields: fields
                            .into_iter()
                            .map(|field| FunctionFieldInit {
                                field: field.field,
                                name: field.name,
                                value: self.instantiate_expr(field.value, substitutions),
                                span: field.span,
                            })
                            .collect(),
                    }
                }
                FunctionExprKind::UnionLiteral { def_id, field } => {
                    FunctionExprKind::UnionLiteral {
                        def_id,
                        field: Box::new(FunctionFieldInit {
                            field: field.field,
                            name: field.name,
                            value: self.instantiate_expr(field.value, substitutions),
                            span: field.span,
                        }),
                    }
                }
                FunctionExprKind::Unary { op, expr } => FunctionExprKind::Unary {
                    op,
                    expr: Box::new(self.instantiate_expr(*expr, substitutions)),
                },
                FunctionExprKind::AddrOf(place) => {
                    FunctionExprKind::AddrOf(self.instantiate_place(place, substitutions))
                }
                FunctionExprKind::Binary { lhs, op, rhs } => FunctionExprKind::Binary {
                    lhs: Box::new(self.instantiate_expr(*lhs, substitutions)),
                    op,
                    rhs: Box::new(self.instantiate_expr(*rhs, substitutions)),
                },
                FunctionExprKind::Assign { place, op, rhs } => FunctionExprKind::Assign {
                    place: self.instantiate_place(place, substitutions),
                    op,
                    rhs: Box::new(self.instantiate_expr(*rhs, substitutions)),
                },
                FunctionExprKind::Discard(expr) => {
                    FunctionExprKind::Discard(Box::new(self.instantiate_expr(*expr, substitutions)))
                }
                FunctionExprKind::Cast { expr, ty } => FunctionExprKind::Cast {
                    expr: Box::new(self.instantiate_expr(*expr, substitutions)),
                    ty: self.instantiate_ty(ty, substitutions),
                },
                FunctionExprKind::Call { callee, args } => {
                    let args = args
                        .into_iter()
                        .map(|arg| self.instantiate_expr(arg, substitutions))
                        .collect::<Vec<_>>();
                    let callee = self.instantiate_callee(callee, substitutions);
                    if let FunctionCallee::BuiltinPlaceMethod {
                        trait_id,
                        method: _,
                        self_ty,
                        trait_args,
                        receiver,
                        ..
                    } = &callee
                        && let Some(native_expr) = self.lower_native_builtin_place_method_call(
                            *trait_id,
                            *self_ty,
                            trait_args,
                            receiver.as_ref().clone(),
                            &args,
                        )
                    {
                        return native_expr;
                    }
                    if let FunctionCallee::BuiltinPlaceMethod {
                        trait_id,
                        method,
                        self_ty,
                        trait_args,
                        receiver,
                    } = callee
                    {
                        if let Some((def_id, target_args)) = self.resolve_builtin_place_method_impl(
                            trait_id,
                            &trait_args,
                            method,
                            self_ty,
                        ) {
                            return FunctionExpr {
                                span: expr.span,
                                ty: expr.ty,
                                kind: FunctionExprKind::Call {
                                    callee: FunctionCallee::Method {
                                        def_id,
                                        args: target_args,
                                        receiver,
                                    },
                                    args,
                                },
                            };
                        }
                        if self.native_builtin_place_method_impl(trait_id, self_ty, &trait_args) {
                            return FunctionExpr {
                                span: expr.span,
                                ty: expr.ty,
                                kind: FunctionExprKind::Call {
                                    callee: FunctionCallee::BuiltinPlaceMethod {
                                        trait_id,
                                        method,
                                        self_ty,
                                        trait_args,
                                        receiver,
                                    },
                                    args,
                                },
                            };
                        }
                        self.diagnostics.push(nia_diagnostic::Diagnostic::error(
                            receiver.span,
                            "no visible implementation found for builtin place method call",
                        ));
                        return FunctionExpr {
                            span: expr.span,
                            ty: expr.ty,
                            kind: FunctionExprKind::Call {
                                callee: FunctionCallee::BuiltinPlaceMethod {
                                    trait_id,
                                    method,
                                    self_ty,
                                    trait_args,
                                    receiver,
                                },
                                args,
                            },
                        };
                    }
                    FunctionExprKind::Call { callee, args }
                }
                FunctionExprKind::Field { lhs, field } => FunctionExprKind::Field {
                    lhs: Box::new(self.instantiate_expr(*lhs, substitutions)),
                    field,
                },
                FunctionExprKind::Index { lhs, index } => FunctionExprKind::Index {
                    lhs: Box::new(self.instantiate_expr(*lhs, substitutions)),
                    index: Box::new(self.instantiate_expr(*index, substitutions)),
                },
                FunctionExprKind::Slice {
                    lhs,
                    range,
                    is_const,
                } => FunctionExprKind::Slice {
                    lhs: Box::new(self.instantiate_expr(*lhs, substitutions)),
                    range: self.instantiate_slice_range(range, substitutions),
                    is_const,
                },
            },
        }
    }

    fn instantiate_callee(
        &mut self,
        callee: FunctionCallee,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> FunctionCallee {
        match callee {
            FunctionCallee::Function(def_id) => FunctionCallee::Function(def_id),
            FunctionCallee::FunctionInstance { def_id, args } => FunctionCallee::FunctionInstance {
                def_id,
                args: args
                    .into_iter()
                    .map(|arg| self.instantiate_ty(arg, substitutions))
                    .collect(),
            },
            FunctionCallee::Method {
                def_id,
                args,
                receiver,
            } => FunctionCallee::Method {
                def_id,
                args: args
                    .into_iter()
                    .map(|arg| self.instantiate_ty(arg, substitutions))
                    .collect(),
                receiver: Box::new(self.instantiate_expr(*receiver, substitutions)),
            },
            FunctionCallee::TraitMethod {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
                receiver,
            } => {
                let self_ty = self.instantiate_ty(self_ty, substitutions);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.instantiate_ty(arg, substitutions))
                    .collect::<Vec<_>>();
                let args = args
                    .into_iter()
                    .map(|arg| self.instantiate_ty(arg, substitutions))
                    .collect::<Vec<_>>();
                let receiver = Box::new(self.instantiate_expr(*receiver, substitutions));
                if let Some((def_id, target_args)) = self.resolve_trait_method_impl(
                    trait_id,
                    &trait_args,
                    method_id,
                    &method_name,
                    self_ty,
                ) {
                    let mut instance_args = target_args;
                    instance_args.extend(args);
                    FunctionCallee::Method {
                        def_id,
                        args: instance_args,
                        receiver,
                    }
                } else if self.trait_method_has_default(method_id) {
                    let mut instance_args = vec![self_ty];
                    instance_args.extend(trait_args.iter().copied());
                    instance_args.extend(args);
                    FunctionCallee::Method {
                        def_id: method_id,
                        args: instance_args,
                        receiver,
                    }
                } else {
                    self.diagnostics.push(nia_diagnostic::Diagnostic::error(
                        receiver.span,
                        "no visible implementation found for trait method call",
                    ));
                    FunctionCallee::TraitMethod {
                        trait_id,
                        method_id,
                        method_name,
                        self_ty,
                        trait_args,
                        args,
                        receiver,
                    }
                }
            }
            FunctionCallee::BuiltinPlaceMethod {
                trait_id,
                method,
                self_ty,
                trait_args,
                receiver,
            } => {
                let self_ty = self.instantiate_ty(self_ty, substitutions);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.instantiate_ty(arg, substitutions))
                    .collect::<Vec<_>>();
                let receiver = Box::new(self.instantiate_expr(*receiver, substitutions));
                FunctionCallee::BuiltinPlaceMethod {
                    trait_id,
                    method,
                    self_ty,
                    trait_args,
                    receiver,
                }
            }
            FunctionCallee::BuiltinOperator(operator) => FunctionCallee::BuiltinOperator(operator),
            FunctionCallee::FunctionPointer(expr) => FunctionCallee::FunctionPointer(Box::new(
                self.instantiate_expr(*expr, substitutions),
            )),
        }
    }

    fn instantiate_builtin_value(
        &mut self,
        value: nia_function_ir::FunctionBuiltinValue,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> nia_function_ir::FunctionBuiltinValue {
        match value {
            nia_function_ir::FunctionBuiltinValue::Layout { builtin, ty } => {
                let ty = self.instantiate_ty(ty, substitutions);
                if let Some(layout) = self.layout_of(ty) {
                    let value = match builtin {
                        LayoutBuiltin::Size => layout.size,
                        LayoutBuiltin::Align => layout.align,
                    };
                    nia_function_ir::FunctionBuiltinValue::Usize(value)
                } else {
                    nia_function_ir::FunctionBuiltinValue::Layout { builtin, ty }
                }
            }
            nia_function_ir::FunctionBuiltinValue::Usize(value) => {
                nia_function_ir::FunctionBuiltinValue::Usize(value)
            }
            nia_function_ir::FunctionBuiltinValue::Int(value) => {
                nia_function_ir::FunctionBuiltinValue::Int(value)
            }
        }
    }

    fn trait_method_has_default(&self, method_id: GlobalDefId) -> bool {
        self.input
            .signatures
            .traits
            .values()
            .flat_map(|signature| signature.methods.iter())
            .any(|method| {
                GlobalDefId {
                    module_id: self.input.module_id,
                    def_id: method.def_id,
                } == method_id
                    && method.has_default
            })
    }

    fn resolve_trait_method_impl(
        &mut self,
        trait_id: GlobalDefId,
        trait_args: &[InternedTyId],
        trait_method_id: GlobalDefId,
        trait_method_name: &str,
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let trait_method_name = self
            .input
            .defs
            .defs
            .get(trait_method_id.def_id)
            .filter(|_| trait_method_id.module_id == self.input.module_id)
            .map(|def| def.name.clone())
            .or_else(|| {
                self.input
                    .extensions
                    .targets()
                    .iter()
                    .flat_map(|target| target.methods.iter())
                    .find(|method| method.def_id == trait_method_id)
                    .map(|method| method.name.clone())
            })
            .unwrap_or_else(|| trait_method_name.to_string());
        let candidates = self
            .input
            .extensions
            .targets()
            .iter()
            .filter_map(|target| {
                self.trait_impl_method_for_target(
                    target,
                    trait_id,
                    &trait_args,
                    &trait_method_name,
                    self_ty,
                )
            })
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [candidate] => Some(candidate.clone()),
            _ => None,
        }
    }

    fn trait_impl_method_for_target(
        &mut self,
        target: &VisibleExtensionTarget,
        trait_id: GlobalDefId,
        trait_args: &[InternedTyId],
        method_name: &str,
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        if !self.extension_type_pattern_matches(target.target_ty, self_ty) {
            return None;
        }
        let method = target.methods.iter().find(|method| {
            let method_trait_args = method
                .trait_args
                .iter()
                .map(|arg| self.import_extension_type(*arg))
                .collect::<Vec<_>>();
            method.name == method_name
                && method.trait_id == Some(TraitId::Source(trait_id))
                && method_trait_args.len() == trait_args.len()
                && method_trait_args
                    .iter()
                    .zip(trait_args)
                    .all(|(actual, expected)| self.types_match(*actual, *expected))
        })?;
        let mut substitutions = HashMap::new();
        self.match_extension_type_pattern(target.target_ty, self_ty, &mut substitutions)
            .then(|| {
                let args = self
                    .generic_params_in_extension_ty(target.target_ty)
                    .iter()
                    .filter_map(|generic| substitutions.get(generic).copied())
                    .collect::<Vec<_>>();
                (method.def_id, args)
            })
    }

    fn resolve_builtin_place_method_impl(
        &mut self,
        trait_id: BuiltinTrait,
        trait_args: &[InternedTyId],
        method: BuiltinTraitMethod,
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let candidates = self
            .input
            .extensions
            .targets()
            .iter()
            .filter_map(|target| {
                self.builtin_place_impl_method_for_target(
                    target,
                    trait_id,
                    &trait_args,
                    method.name(),
                    self_ty,
                )
            })
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [candidate] => Some(candidate.clone()),
            _ => None,
        }
    }

    fn builtin_place_impl_method_for_target(
        &mut self,
        target: &VisibleExtensionTarget,
        trait_id: BuiltinTrait,
        trait_args: &[InternedTyId],
        method_name: &str,
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        if !self.extension_type_pattern_matches(target.target_ty, self_ty) {
            return None;
        }
        let method = target.methods.iter().find(|method| {
            let method_trait_args = method
                .trait_args
                .iter()
                .map(|arg| self.import_extension_type(*arg))
                .collect::<Vec<_>>();
            method.name == method_name
                && method.trait_id == Some(TraitId::Builtin(trait_id))
                && method_trait_args.len() == trait_args.len()
                && method_trait_args
                    .iter()
                    .zip(trait_args)
                    .all(|(actual, expected)| self.types_match(*actual, *expected))
        })?;
        let mut substitutions = HashMap::new();
        self.match_extension_type_pattern(target.target_ty, self_ty, &mut substitutions)
            .then(|| {
                let args = self
                    .generic_params_in_extension_ty(target.target_ty)
                    .iter()
                    .filter_map(|generic| substitutions.get(generic).copied())
                    .collect::<Vec<_>>();
                (method.def_id, args)
            })
    }

    fn lower_native_builtin_place_method_call(
        &mut self,
        trait_id: BuiltinTrait,
        self_ty: InternedTyId,
        trait_args: &[InternedTyId],
        receiver: FunctionExpr,
        args: &[FunctionExpr],
    ) -> Option<FunctionExpr> {
        let receiver_span = receiver.span;
        match (trait_id, trait_args, args) {
            (BuiltinTrait::DerefConst | BuiltinTrait::Deref, [], []) => {
                if !self.native_deref_trait_impl(trait_id, self_ty) {
                    return None;
                }
                let elem = self.pointer_elem_ty(self_ty)?;
                let receiver_ptr = FunctionExpr {
                    span: receiver_span,
                    ty: self_ty,
                    kind: FunctionExprKind::Unary {
                        op: nia_ast::UnaryOp::Deref,
                        expr: Box::new(receiver),
                    },
                };
                Some(FunctionExpr {
                    span: receiver_ptr.span,
                    ty: self.interner.intern(TyKind::Pointer {
                        is_const: matches!(trait_id, BuiltinTrait::DerefConst),
                        elem,
                    }),
                    kind: FunctionExprKind::AddrOf(FunctionPlace {
                        span: receiver_ptr.span,
                        ty: elem,
                        base: FunctionPlaceBase::Deref(Box::new(receiver_ptr)),
                        elems: Vec::new(),
                    }),
                })
            }
            (BuiltinTrait::IndexConst | BuiltinTrait::Index, [index_ty], [index]) => {
                if !self.native_index_trait_impl(trait_id, self_ty, *index_ty) {
                    return None;
                }
                let elem = self.index_elem_ty(self_ty)?;
                let base = FunctionExpr {
                    span: receiver_span,
                    ty: self_ty,
                    kind: FunctionExprKind::Unary {
                        op: nia_ast::UnaryOp::Deref,
                        expr: Box::new(receiver),
                    },
                };
                Some(FunctionExpr {
                    span: index.span,
                    ty: self.interner.intern(TyKind::Pointer {
                        is_const: matches!(trait_id, BuiltinTrait::IndexConst),
                        elem,
                    }),
                    kind: FunctionExprKind::AddrOf(FunctionPlace {
                        span: index.span,
                        ty: elem,
                        base: FunctionPlaceBase::Deref(Box::new(base)),
                        elems: vec![FunctionPlaceElem::Index(Box::new(index.clone()))],
                    }),
                })
            }
            (BuiltinTrait::SliceConst | BuiltinTrait::Slice, [_range_ty], [range]) => {
                if !self.native_slice_trait_impl(trait_id, self_ty) {
                    return None;
                }
                let base = FunctionExpr {
                    span: receiver_span,
                    ty: self_ty,
                    kind: FunctionExprKind::Unary {
                        op: nia_ast::UnaryOp::Deref,
                        expr: Box::new(receiver),
                    },
                };
                Some(FunctionExpr {
                    span: range.span,
                    ty: self.resolve_associated_type_projection(
                        self_ty,
                        TraitId::Builtin(trait_id),
                        trait_args,
                        BuiltinTrait::OUTPUT_ASSOC_TYPE,
                    )?,
                    kind: FunctionExprKind::Slice {
                        lhs: Box::new(base),
                        range: self.range_expr_to_slice_range(range)?,
                        is_const: matches!(trait_id, BuiltinTrait::SliceConst),
                    },
                })
            }
            _ => None,
        }
    }

    fn native_deref_trait_impl(&self, trait_id: BuiltinTrait, self_ty: InternedTyId) -> bool {
        match (trait_id, self.ty_kind(self_ty)) {
            (BuiltinTrait::DerefConst, Some(TyKind::Pointer { elem, .. })) => !self.is_void(*elem),
            (
                BuiltinTrait::Deref,
                Some(TyKind::Pointer {
                    is_const: false,
                    elem,
                }),
            ) => !self.is_void(*elem),
            _ => false,
        }
    }

    fn native_index_trait_impl(
        &self,
        trait_id: BuiltinTrait,
        self_ty: InternedTyId,
        index_ty: InternedTyId,
    ) -> bool {
        if !self.is_integral_ty(index_ty) {
            return false;
        }
        match (trait_id, self.ty_kind(self_ty)) {
            (
                BuiltinTrait::IndexConst,
                Some(TyKind::Array { .. } | TyKind::Pointer { .. } | TyKind::Slice { .. }),
            ) => true,
            (BuiltinTrait::Index, Some(TyKind::Array { .. })) => true,
            (
                BuiltinTrait::Index,
                Some(
                    TyKind::Pointer {
                        is_const: false, ..
                    }
                    | TyKind::Slice {
                        is_const: false, ..
                    },
                ),
            ) => true,
            _ => false,
        }
    }

    fn native_slice_trait_impl(&self, trait_id: BuiltinTrait, self_ty: InternedTyId) -> bool {
        match (trait_id, self.ty_kind(self_ty)) {
            (
                BuiltinTrait::SliceConst,
                Some(TyKind::Array { .. } | TyKind::Pointer { .. } | TyKind::Slice { .. }),
            ) => true,
            (BuiltinTrait::Slice, Some(TyKind::Array { .. })) => true,
            (
                BuiltinTrait::Slice,
                Some(
                    TyKind::Pointer {
                        is_const: false, ..
                    }
                    | TyKind::Slice {
                        is_const: false, ..
                    },
                ),
            ) => true,
            _ => false,
        }
    }

    fn native_builtin_place_method_impl(
        &self,
        trait_id: BuiltinTrait,
        self_ty: InternedTyId,
        trait_args: &[InternedTyId],
    ) -> bool {
        if !trait_args.is_empty() {
            return matches!(
                (trait_id, self.ty_kind(self_ty)),
                (
                    BuiltinTrait::SliceConst,
                    Some(TyKind::Array { .. } | TyKind::Pointer { .. } | TyKind::Slice { .. })
                ) | (BuiltinTrait::Slice, Some(TyKind::Array { .. }))
                    | (
                        BuiltinTrait::Slice,
                        Some(
                            TyKind::Pointer {
                                is_const: false,
                                ..
                            } | TyKind::Slice {
                                is_const: false,
                                ..
                            }
                        )
                    )
            );
        }
        match (trait_id, self.ty_kind(self_ty)) {
            (BuiltinTrait::Len, Some(TyKind::Array { .. } | TyKind::Slice { .. })) => true,
            (BuiltinTrait::GetPtrConst, Some(TyKind::Slice { .. })) => true,
            (
                BuiltinTrait::GetPtr,
                Some(TyKind::Slice {
                    is_const: false, ..
                }),
            ) => true,
            _ => false,
        }
    }

    fn pointer_elem_ty(&self, ty: InternedTyId) -> Option<InternedTyId> {
        match self.ty_kind(ty) {
            Some(TyKind::Pointer { elem, .. }) => Some(*elem),
            _ => None,
        }
    }

    fn index_elem_ty(&self, ty: InternedTyId) -> Option<InternedTyId> {
        match self.ty_kind(ty) {
            Some(TyKind::Array { elem, .. })
            | Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::Slice { elem, .. }) => Some(*elem),
            _ => None,
        }
    }

    fn is_integral_ty(&self, ty: InternedTyId) -> bool {
        matches!(
            self.ty_kind(ty),
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
            ))
        )
    }

    fn is_void(&self, ty: InternedTyId) -> bool {
        matches!(self.ty_kind(ty), Some(TyKind::Primitive(PrimitiveTy::Void)))
    }

    pub(crate) fn generic_params_in_extension_ty(&self, ty: InternedTyId) -> Vec<String> {
        let mut generics = Vec::new();
        self.collect_generic_params_in_extension_ty(ty, &mut generics);
        generics
    }

    fn collect_generic_params_in_extension_ty(&self, ty: InternedTyId, generics: &mut Vec<String>) {
        match self.extension_ty_kind(ty) {
            Some(TyKind::GenericParam(name)) => {
                if !generics.contains(name) {
                    generics.push(name.clone());
                }
            }
            Some(TyKind::Pointer { elem, .. } | TyKind::Slice { elem, .. }) => {
                self.collect_generic_params_in_extension_ty(*elem, generics);
            }
            Some(TyKind::Array { elem, .. }) => {
                self.collect_generic_params_in_extension_ty(*elem, generics);
            }
            Some(TyKind::Range { bound, .. }) => {
                if let Some(bound) = bound {
                    self.collect_generic_params_in_extension_ty(*bound, generics);
                }
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                for param in params {
                    self.collect_generic_params_in_extension_ty(*param, generics);
                }
                self.collect_generic_params_in_extension_ty(*return_type, generics);
            }
            Some(TyKind::Nominal { args, .. }) => {
                for arg in args {
                    self.collect_generic_params_in_extension_ty(*arg, generics);
                }
            }
            Some(TyKind::BuiltinTrait { args, .. }) => {
                for arg in args {
                    self.collect_generic_params_in_extension_ty(*arg, generics);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.collect_generic_params_in_extension_ty(*self_ty, generics);
                for arg in trait_args {
                    self.collect_generic_params_in_extension_ty(*arg, generics);
                }
            }
            Some(TyKind::Error | TyKind::Primitive(_)) | None => {}
        }
    }

    fn instantiate_place(
        &mut self,
        place: FunctionPlace,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> FunctionPlace {
        FunctionPlace {
            span: place.span,
            ty: self.instantiate_ty(place.ty, substitutions),
            base: match place.base {
                FunctionPlaceBase::Local(local_id) => FunctionPlaceBase::Local(local_id),
                FunctionPlaceBase::Global(def_id) => FunctionPlaceBase::Global(def_id),
                FunctionPlaceBase::Deref(expr) => {
                    FunctionPlaceBase::Deref(Box::new(self.instantiate_expr(*expr, substitutions)))
                }
                FunctionPlaceBase::Error => FunctionPlaceBase::Error,
            },
            elems: place
                .elems
                .into_iter()
                .map(|elem| match elem {
                    FunctionPlaceElem::Field(field) => FunctionPlaceElem::Field(field),
                    FunctionPlaceElem::Index(expr) => FunctionPlaceElem::Index(Box::new(
                        self.instantiate_expr(*expr, substitutions),
                    )),
                    FunctionPlaceElem::Error => FunctionPlaceElem::Error,
                })
                .collect(),
        }
    }

    fn instantiate_slice_range(
        &mut self,
        range: FunctionSliceRange,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> FunctionSliceRange {
        FunctionSliceRange {
            start: range
                .start
                .map(|start| Box::new(self.instantiate_expr(*start, substitutions))),
            end: range
                .end
                .map(|end| Box::new(self.instantiate_expr(*end, substitutions))),
            inclusive: range.inclusive,
        }
    }

    fn instantiate_range(
        &mut self,
        range: FunctionRange,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> FunctionRange {
        FunctionRange {
            start: range
                .start
                .map(|start| Box::new(self.instantiate_expr(*start, substitutions))),
            end: range
                .end
                .map(|end| Box::new(self.instantiate_expr(*end, substitutions))),
            inclusive: range.inclusive,
        }
    }

    fn range_expr_to_slice_range(&self, range: &FunctionExpr) -> Option<FunctionSliceRange> {
        match &range.kind {
            FunctionExprKind::Range(range) => Some(FunctionSliceRange {
                start: range.start.clone(),
                end: range.end.clone(),
                inclusive: range.inclusive,
            }),
            _ => None,
        }
    }

    fn instantiate_array_elements(
        &mut self,
        elems: FunctionArrayElements,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> FunctionArrayElements {
        match elems {
            FunctionArrayElements::List(elems) => FunctionArrayElements::List(
                elems
                    .into_iter()
                    .map(|elem| self.instantiate_expr(elem, substitutions))
                    .collect(),
            ),
            FunctionArrayElements::Repeat { value, count } => FunctionArrayElements::Repeat {
                value: Box::new(self.instantiate_expr(*value, substitutions)),
                count,
            },
        }
    }

    pub(crate) fn instantiate_ty(
        &mut self,
        ty: InternedTyId,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> InternedTyId {
        match self.interner.get(ty).cloned() {
            Some(TyKind::GenericParam(name)) => substitutions.get(&name).copied().unwrap_or(ty),
            Some(TyKind::Pointer { is_const, elem }) => {
                let elem = self.instantiate_ty(elem, substitutions);
                self.interner.intern(TyKind::Pointer { is_const, elem })
            }
            Some(TyKind::Slice { is_const, elem }) => {
                let elem = self.instantiate_ty(elem, substitutions);
                self.interner.intern(TyKind::Slice { is_const, elem })
            }
            Some(TyKind::Array { len, elem }) => {
                let elem = self.instantiate_ty(elem, substitutions);
                self.interner.intern(TyKind::Array { len, elem })
            }
            Some(TyKind::Range { kind, bound }) => {
                let bound = bound.map(|bound| self.instantiate_ty(bound, substitutions));
                self.interner.intern(TyKind::Range { kind, bound })
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                let params = params
                    .iter()
                    .copied()
                    .map(|param| self.instantiate_ty(param, substitutions))
                    .collect();
                let return_type = self.instantiate_ty(return_type, substitutions);
                self.interner.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                })
            }
            Some(TyKind::Nominal { def_id, args }) => {
                let args = args
                    .iter()
                    .copied()
                    .map(|arg| self.instantiate_ty(arg, substitutions))
                    .collect::<Vec<_>>();
                self.interner.intern(TyKind::Nominal { def_id, args })
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                let args = args
                    .iter()
                    .copied()
                    .map(|arg| self.instantiate_ty(arg, substitutions))
                    .collect::<Vec<_>>();
                self.interner
                    .intern(TyKind::BuiltinTrait { trait_id, args })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            }) => {
                let self_ty = self.instantiate_ty(self_ty, substitutions);
                let trait_args = trait_args
                    .iter()
                    .copied()
                    .map(|arg| self.instantiate_ty(arg, substitutions))
                    .collect::<Vec<_>>();
                self.resolve_associated_type_projection(self_ty, trait_id, &trait_args, &name)
                    .unwrap_or_else(|| {
                        self.interner.intern(TyKind::Projection {
                            self_ty,
                            trait_id,
                            trait_args,
                            name,
                        })
                    })
            }
            Some(TyKind::Error) | Some(TyKind::Primitive(_)) | None => ty,
        }
    }

    fn resolve_associated_type_projection(
        &mut self,
        self_ty: InternedTyId,
        trait_id: nia_ty::TraitId,
        trait_args: &[InternedTyId],
        name: &str,
    ) -> Option<InternedTyId> {
        for impl_signature in self.input.trait_impls {
            if impl_signature.trait_id != trait_id {
                continue;
            }
            let target_ty =
                self.import_type_from(&impl_signature.interner, impl_signature.target_ty);
            let impl_trait_args = impl_signature
                .trait_args
                .iter()
                .map(|arg| self.import_type_from(&impl_signature.interner, *arg))
                .collect::<Vec<_>>();
            if !self.types_match(target_ty, self_ty)
                || impl_trait_args.len() != trait_args.len()
                || !impl_trait_args
                    .iter()
                    .zip(trait_args)
                    .all(|(actual, expected)| self.types_match(*actual, *expected))
            {
                continue;
            }
            let associated_type = impl_signature
                .associated_types
                .iter()
                .find(|associated_type| associated_type.name == name)?;
            return Some(self.import_type_from(&impl_signature.interner, associated_type.ty));
        }
        None
    }

    fn import_type_from(&mut self, source: &nia_ty::TyInterner, ty: InternedTyId) -> InternedTyId {
        nia_body_check::import_type_into(&mut self.interner, source, ty)
    }

    fn import_extension_type(&mut self, ty: InternedTyId) -> InternedTyId {
        let Some(extension_interner) = self.input.extension_interner else {
            return ty;
        };
        if ty.interner_id == extension_interner.interner_id() {
            nia_body_check::import_type_into(&mut self.interner, extension_interner, ty)
        } else {
            ty
        }
    }

    pub(crate) fn extension_type_pattern_matches(
        &self,
        pattern: InternedTyId,
        actual: InternedTyId,
    ) -> bool {
        self.match_extension_type_pattern(pattern, actual, &mut HashMap::new())
    }

    fn extension_ty_kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.input
            .extension_interner
            .filter(|interner| ty.interner_id == interner.interner_id())
            .and_then(|interner| interner.get(ty))
            .or_else(|| self.ty_kind(ty))
    }

    pub(crate) fn match_extension_type_pattern(
        &self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
    ) -> bool {
        match self.extension_ty_kind(pattern) {
            Some(TyKind::GenericParam(name)) => {
                if let Some(existing) = substitutions.get(name).copied() {
                    self.types_match(existing, actual)
                } else {
                    substitutions.insert(name.clone(), actual);
                    true
                }
            }
            Some(TyKind::Pointer {
                is_const: pattern_const,
                elem: pattern_elem,
            }) => matches!(
                self.ty_kind(actual),
                Some(TyKind::Pointer { is_const, elem })
                    if is_const == pattern_const
                        && self.match_extension_type_pattern(*pattern_elem, *elem, substitutions)
            ),
            Some(TyKind::Slice {
                is_const: pattern_const,
                elem: pattern_elem,
            }) => matches!(
                self.ty_kind(actual),
                Some(TyKind::Slice { is_const, elem })
                    if is_const == pattern_const
                        && self.match_extension_type_pattern(*pattern_elem, *elem, substitutions)
            ),
            Some(TyKind::Array {
                len: pattern_len,
                elem: pattern_elem,
            }) => match self.ty_kind(actual) {
                Some(TyKind::Array { len, elem }) if pattern_len == len => {
                    self.match_extension_type_pattern(*pattern_elem, *elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::Range {
                kind: pattern_kind,
                bound: pattern_bound,
            }) => match self.ty_kind(actual) {
                Some(TyKind::Range { kind, bound }) if pattern_kind == kind => {
                    match (pattern_bound, bound) {
                        (Some(pattern_bound), Some(bound)) => {
                            self.match_extension_type_pattern(*pattern_bound, *bound, substitutions)
                        }
                        (None, None) => true,
                        _ => false,
                    }
                }
                _ => false,
            },
            Some(TyKind::FunctionPointer {
                params: pattern_params,
                return_type: pattern_return,
                is_variadic: pattern_variadic,
            }) => match self.ty_kind(actual) {
                Some(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                }) if pattern_variadic == is_variadic && pattern_params.len() == params.len() => {
                    pattern_params.iter().zip(params).all(|(pattern, actual)| {
                        self.match_extension_type_pattern(*pattern, *actual, substitutions)
                    }) && self.match_extension_type_pattern(
                        *pattern_return,
                        *return_type,
                        substitutions,
                    )
                }
                _ => false,
            },
            Some(TyKind::Nominal {
                def_id: pattern_def,
                args: pattern_args,
            }) => match self.ty_kind(actual) {
                Some(TyKind::Nominal { def_id, args })
                    if pattern_def == def_id && pattern_args.len() == args.len() =>
                {
                    pattern_args.iter().zip(args).all(|(pattern, actual)| {
                        self.match_extension_type_pattern(*pattern, *actual, substitutions)
                    })
                }
                _ => false,
            },
            Some(TyKind::BuiltinTrait {
                trait_id: pattern_trait,
                args: pattern_args,
            }) => match self.ty_kind(actual) {
                Some(TyKind::BuiltinTrait { trait_id, args })
                    if pattern_trait == trait_id && pattern_args.len() == args.len() =>
                {
                    pattern_args.iter().zip(args).all(|(pattern, actual)| {
                        self.match_extension_type_pattern(*pattern, *actual, substitutions)
                    })
                }
                _ => false,
            },
            Some(TyKind::Projection {
                self_ty: pattern_self,
                trait_id: pattern_trait,
                trait_args: pattern_args,
                name: pattern_name,
            }) => match self.ty_kind(actual) {
                Some(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args,
                    name,
                }) if pattern_trait == trait_id
                    && pattern_name == name
                    && pattern_args.len() == trait_args.len() =>
                {
                    self.match_extension_type_pattern(*pattern_self, *self_ty, substitutions)
                        && pattern_args
                            .iter()
                            .zip(trait_args)
                            .all(|(pattern, actual)| {
                                self.match_extension_type_pattern(*pattern, *actual, substitutions)
                            })
                }
                _ => false,
            },
            Some(TyKind::Primitive(_)) | Some(TyKind::Error) | None => {
                self.types_match(pattern, actual)
            }
        }
    }

    pub(crate) fn types_match(&self, left: InternedTyId, right: InternedTyId) -> bool {
        if left == right {
            return true;
        }
        match (self.ty_kind(left), self.ty_kind(right)) {
            (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
            (
                Some(TyKind::Pointer {
                    is_const: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::Pointer {
                    is_const: right_const,
                    elem: right_elem,
                }),
            ) => left_const == right_const && self.types_match(*left_elem, *right_elem),
            (
                Some(TyKind::Nominal {
                    def_id: left_def,
                    args: left_args,
                }),
                Some(TyKind::Nominal {
                    def_id: right_def,
                    args: right_args,
                }),
            ) => {
                left_def == right_def
                    && left_args.len() == right_args.len()
                    && left_args
                        .iter()
                        .zip(right_args)
                        .all(|(left, right)| self.types_match(*left, *right))
            }
            (
                Some(TyKind::BuiltinTrait {
                    trait_id: left_trait,
                    args: left_args,
                }),
                Some(TyKind::BuiltinTrait {
                    trait_id: right_trait,
                    args: right_args,
                }),
            ) => {
                left_trait == right_trait
                    && left_args.len() == right_args.len()
                    && left_args
                        .iter()
                        .zip(right_args)
                        .all(|(left, right)| self.types_match(*left, *right))
            }
            (
                Some(TyKind::Range {
                    kind: left_kind,
                    bound: left_bound,
                }),
                Some(TyKind::Range {
                    kind: right_kind,
                    bound: right_bound,
                }),
            ) => {
                left_kind == right_kind
                    && match (left_bound, right_bound) {
                        (Some(left_bound), Some(right_bound)) => {
                            self.types_match(*left_bound, *right_bound)
                        }
                        (None, None) => true,
                        _ => false,
                    }
            }
            (
                Some(TyKind::Projection {
                    self_ty: left_self,
                    trait_id: left_trait,
                    trait_args: left_args,
                    name: left_name,
                }),
                Some(TyKind::Projection {
                    self_ty: right_self,
                    trait_id: right_trait,
                    trait_args: right_args,
                    name: right_name,
                }),
            ) => {
                left_trait == right_trait
                    && left_name == right_name
                    && left_args.len() == right_args.len()
                    && self.types_match(*left_self, *right_self)
                    && left_args
                        .iter()
                        .zip(right_args)
                        .all(|(left, right)| self.types_match(*left, *right))
            }
            _ => false,
        }
    }
}
