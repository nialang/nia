// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::ModuleLowerer;
use nia_backend_ir::{BackendFunction, BackendParam};
use nia_defs::VisibleExtensionTarget;
use nia_function_ir::{
    FunctionArrayElements, FunctionAsmInput, FunctionAsmOutput, FunctionBinding, FunctionBody,
    FunctionCallee, FunctionDeferBody, FunctionExpr, FunctionExprKind, FunctionFieldInit,
    FunctionForHeader, FunctionInlineAsm, FunctionLocal, FunctionOp, FunctionPlace,
    FunctionPlaceBase, FunctionPlaceElem, FunctionSliceRange, FunctionTerminator,
};
use nia_ids::{GlobalDefId, InternedTyId};
use nia_ty::TyKind;

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
        FunctionBody {
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
        }
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
                FunctionExprKind::BuiltinValue(value) => FunctionExprKind::BuiltinValue(value),
                FunctionExprKind::Len(inner) => {
                    FunctionExprKind::Len(Box::new(self.instantiate_expr(*inner, substitutions)))
                }
                FunctionExprKind::Ptr(inner) => {
                    FunctionExprKind::Ptr(Box::new(self.instantiate_expr(*inner, substitutions)))
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
                FunctionExprKind::Call { callee, args } => FunctionExprKind::Call {
                    callee: self.instantiate_callee(callee, substitutions),
                    args: args
                        .into_iter()
                        .map(|arg| self.instantiate_expr(arg, substitutions))
                        .collect(),
                },
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
                self_ty,
                args,
                receiver,
            } => {
                let self_ty = self.instantiate_ty(self_ty, substitutions);
                let args = args
                    .into_iter()
                    .map(|arg| self.instantiate_ty(arg, substitutions))
                    .collect::<Vec<_>>();
                let receiver = Box::new(self.instantiate_expr(*receiver, substitutions));
                if let Some((def_id, target_args)) =
                    self.resolve_trait_method_impl(trait_id, method_id, self_ty)
                {
                    let mut instance_args = target_args;
                    instance_args.extend(args);
                    FunctionCallee::Method {
                        def_id,
                        args: instance_args,
                        receiver,
                    }
                } else if self.trait_method_has_default(method_id) {
                    let mut instance_args = vec![self_ty];
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
                        self_ty,
                        args,
                        receiver,
                    }
                }
            }
            FunctionCallee::FunctionPointer(expr) => FunctionCallee::FunctionPointer(Box::new(
                self.instantiate_expr(*expr, substitutions),
            )),
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
        trait_method_id: GlobalDefId,
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let trait_method_name = self
            .input
            .defs
            .defs
            .get(trait_method_id.def_id)
            .map(|def| def.name.clone())
            .or_else(|| {
                self.input
                    .extensions
                    .targets()
                    .iter()
                    .flat_map(|target| target.methods.iter())
                    .find(|method| method.def_id == trait_method_id)
                    .map(|method| method.name.clone())
            })?;
        let candidates = self
            .input
            .extensions
            .targets()
            .iter()
            .filter_map(|target| {
                self.trait_impl_method_for_target(target, trait_id, &trait_method_name, self_ty)
            })
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [candidate] => Some(candidate.clone()),
            _ => None,
        }
    }

    fn trait_impl_method_for_target(
        &self,
        target: &VisibleExtensionTarget,
        trait_id: GlobalDefId,
        method_name: &str,
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        if !self.type_pattern_matches(target.target_ty, self_ty) {
            return None;
        }
        let method = target
            .methods
            .iter()
            .find(|method| method.name == method_name && method.trait_id == Some(trait_id))?;
        let mut substitutions = HashMap::new();
        self.match_type_pattern(target.target_ty, self_ty, &mut substitutions)
            .then(|| {
                let args = self
                    .generic_params_in_ty(target.target_ty)
                    .iter()
                    .filter_map(|generic| substitutions.get(generic).copied())
                    .collect();
                (method.def_id, args)
            })
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
            },
            elems: place
                .elems
                .into_iter()
                .map(|elem| match elem {
                    FunctionPlaceElem::Field(field) => FunctionPlaceElem::Field(field),
                    FunctionPlaceElem::Index(expr) => FunctionPlaceElem::Index(Box::new(
                        self.instantiate_expr(*expr, substitutions),
                    )),
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
                    .collect();
                self.interner.intern(TyKind::Nominal { def_id, args })
            }
            Some(TyKind::Error) | Some(TyKind::Primitive(_)) | None => ty,
        }
    }

    pub(crate) fn type_pattern_matches(&self, pattern: InternedTyId, actual: InternedTyId) -> bool {
        self.match_type_pattern(pattern, actual, &mut HashMap::new())
    }

    pub(crate) fn match_type_pattern(
        &self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
    ) -> bool {
        match self.ty_kind(pattern) {
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
                        && self.match_type_pattern(*pattern_elem, *elem, substitutions)
            ),
            Some(TyKind::Slice {
                is_const: pattern_const,
                elem: pattern_elem,
            }) => matches!(
                self.ty_kind(actual),
                Some(TyKind::Slice { is_const, elem })
                    if is_const == pattern_const
                        && self.match_type_pattern(*pattern_elem, *elem, substitutions)
            ),
            Some(TyKind::Array {
                len: pattern_len,
                elem: pattern_elem,
            }) => match self.ty_kind(actual) {
                Some(TyKind::Array { len, elem }) if pattern_len == len => {
                    self.match_type_pattern(*pattern_elem, *elem, substitutions)
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
                        self.match_type_pattern(*pattern, *actual, substitutions)
                    }) && self.match_type_pattern(*pattern_return, *return_type, substitutions)
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
                        self.match_type_pattern(*pattern, *actual, substitutions)
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
            _ => false,
        }
    }
}
