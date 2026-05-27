// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::ModuleLowerer;
use nia_backend_ir::{
    BackendFunction, BackendParam, PlaceElem, TypedArrayElements, TypedAsmInput, TypedAsmOutput,
    TypedBinding, TypedBody, TypedCallee, TypedExpr, TypedExprKind, TypedFieldInit, TypedFor,
    TypedForHeader, TypedForInit, TypedInlineAsm, TypedLocal, TypedPlace, TypedSliceRange,
    TypedStmt, TypedStmtKind, TypedSwitch, TypedSwitchArm, TypedSwitchArmBody, TypedSwitchPattern,
};
use nia_ids::{GlobalDefId, TyId};
use nia_ty::TyKind;

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn generic_substitutions(
        &self,
        generics: &[String],
        args: &[TyId],
    ) -> HashMap<String, TyId> {
        generics.iter().cloned().zip(args.iter().copied()).collect()
    }

    pub(crate) fn effective_generics(
        &self,
        def_id: GlobalDefId,
        own_generics: &[String],
    ) -> Vec<String> {
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

    pub(crate) fn generic_params_in_ty(&self, ty: TyId) -> Vec<String> {
        let mut generics = Vec::new();
        self.collect_generic_params_in_ty(ty, &mut generics);
        generics
    }

    pub(crate) fn collect_generic_params_in_ty(&self, ty: TyId, generics: &mut Vec<String>) {
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
        args: &[TyId],
    ) -> HashMap<String, TyId> {
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
        substitutions: &HashMap<String, TyId>,
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

    pub(crate) fn instantiate_body(
        &mut self,
        body: TypedBody,
        substitutions: &HashMap<String, TyId>,
    ) -> TypedBody {
        TypedBody {
            span: body.span,
            locals: body
                .locals
                .into_iter()
                .map(|local| TypedLocal {
                    id: local.id,
                    name: local.name,
                    kind: local.kind,
                    ty: self.instantiate_ty(local.ty, substitutions),
                    span: local.span,
                })
                .collect(),
            stmts: body
                .stmts
                .into_iter()
                .map(|stmt| self.instantiate_stmt(stmt, substitutions))
                .collect(),
            tail: body
                .tail
                .map(|tail| Box::new(self.instantiate_expr(*tail, substitutions))),
            ty: self.instantiate_ty(body.ty, substitutions),
        }
    }

    fn instantiate_stmt(
        &mut self,
        stmt: TypedStmt,
        substitutions: &HashMap<String, TyId>,
    ) -> TypedStmt {
        TypedStmt {
            span: stmt.span,
            kind: match stmt.kind {
                TypedStmtKind::Binding(binding) => {
                    TypedStmtKind::Binding(self.instantiate_binding(binding, substitutions))
                }
                TypedStmtKind::Expr(expr) => {
                    TypedStmtKind::Expr(self.instantiate_expr(expr, substitutions))
                }
                TypedStmtKind::Return(value) => TypedStmtKind::Return(
                    value.map(|expr| self.instantiate_expr(expr, substitutions)),
                ),
                TypedStmtKind::Break => TypedStmtKind::Break,
                TypedStmtKind::Continue => TypedStmtKind::Continue,
                TypedStmtKind::Defer(expr) => {
                    TypedStmtKind::Defer(self.instantiate_expr(expr, substitutions))
                }
                TypedStmtKind::For(for_stmt) => {
                    TypedStmtKind::For(Box::new(self.instantiate_for(*for_stmt, substitutions)))
                }
                TypedStmtKind::Switch(switch) => {
                    TypedStmtKind::Switch(self.instantiate_switch(switch, substitutions))
                }
            },
        }
    }

    fn instantiate_binding(
        &mut self,
        binding: TypedBinding,
        substitutions: &HashMap<String, TyId>,
    ) -> TypedBinding {
        TypedBinding {
            local_id: binding.local_id,
            name: binding.name,
            ty: self.instantiate_ty(binding.ty, substitutions),
            value: binding
                .value
                .map(|value| self.instantiate_expr(value, substitutions)),
            is_const: binding.is_const,
        }
    }

    fn instantiate_for(
        &mut self,
        for_stmt: TypedFor,
        substitutions: &HashMap<String, TyId>,
    ) -> TypedFor {
        TypedFor {
            header: match for_stmt.header {
                TypedForHeader::Infinite => TypedForHeader::Infinite,
                TypedForHeader::Condition(expr) => {
                    TypedForHeader::Condition(self.instantiate_expr(expr, substitutions))
                }
                TypedForHeader::CStyle { init, cond, step } => TypedForHeader::CStyle {
                    init: init.map(|init| {
                        Box::new(match *init {
                            TypedForInit::Binding(binding) => TypedForInit::Binding(
                                self.instantiate_binding(binding, substitutions),
                            ),
                            TypedForInit::Expr(expr) => {
                                TypedForInit::Expr(self.instantiate_expr(expr, substitutions))
                            }
                        })
                    }),
                    cond: cond.map(|expr| Box::new(self.instantiate_expr(*expr, substitutions))),
                    step: step.map(|expr| Box::new(self.instantiate_expr(*expr, substitutions))),
                },
            },
            body: self.instantiate_body(for_stmt.body, substitutions),
        }
    }

    fn instantiate_switch(
        &mut self,
        switch: TypedSwitch,
        substitutions: &HashMap<String, TyId>,
    ) -> TypedSwitch {
        TypedSwitch {
            target: self.instantiate_expr(switch.target, substitutions),
            arms: switch
                .arms
                .into_iter()
                .map(|arm| TypedSwitchArm {
                    pattern: match arm.pattern {
                        TypedSwitchPattern::Default => TypedSwitchPattern::Default,
                        TypedSwitchPattern::Expr(expr) => {
                            TypedSwitchPattern::Expr(self.instantiate_expr(expr, substitutions))
                        }
                    },
                    body: match arm.body {
                        TypedSwitchArmBody::Expr(expr) => {
                            TypedSwitchArmBody::Expr(self.instantiate_expr(expr, substitutions))
                        }
                        TypedSwitchArmBody::Stmt(stmt) => TypedSwitchArmBody::Stmt(Box::new(
                            self.instantiate_stmt(*stmt, substitutions),
                        )),
                        TypedSwitchArmBody::Block(block) => TypedSwitchArmBody::Block(Box::new(
                            self.instantiate_body(*block, substitutions),
                        )),
                    },
                    span: arm.span,
                })
                .collect(),
        }
    }

    fn instantiate_expr(
        &mut self,
        expr: TypedExpr,
        substitutions: &HashMap<String, TyId>,
    ) -> TypedExpr {
        TypedExpr {
            span: expr.span,
            ty: self.instantiate_ty(expr.ty, substitutions),
            kind: match expr.kind {
                TypedExprKind::Error => TypedExprKind::Error,
                TypedExprKind::Integer(text) => TypedExprKind::Integer(text),
                TypedExprKind::Float(text) => TypedExprKind::Float(text),
                TypedExprKind::String(scalars) => TypedExprKind::String(scalars),
                TypedExprKind::ByteString(bytes) => TypedExprKind::ByteString(bytes),
                TypedExprKind::Char(value) => TypedExprKind::Char(value),
                TypedExprKind::ByteChar(text) => TypedExprKind::ByteChar(text),
                TypedExprKind::Bool(value) => TypedExprKind::Bool(value),
                TypedExprKind::Local(local) => TypedExprKind::Local(local),
                TypedExprKind::Global(def_id) => TypedExprKind::Global(def_id),
                TypedExprKind::Function(def_id) => TypedExprKind::Function(def_id),
                TypedExprKind::FunctionInstance { def_id, args } => {
                    TypedExprKind::FunctionInstance {
                        def_id,
                        args: args
                            .into_iter()
                            .map(|arg| self.instantiate_ty(arg, substitutions))
                            .collect(),
                    }
                }
                TypedExprKind::EnumVariant(def_id) => TypedExprKind::EnumVariant(def_id),
                TypedExprKind::BuiltinValue(value) => TypedExprKind::BuiltinValue(value),
                TypedExprKind::Len(inner) => {
                    TypedExprKind::Len(Box::new(self.instantiate_expr(*inner, substitutions)))
                }
                TypedExprKind::Ptr(inner) => {
                    TypedExprKind::Ptr(Box::new(self.instantiate_expr(*inner, substitutions)))
                }
                TypedExprKind::InlineAsm(asm) => TypedExprKind::InlineAsm(TypedInlineAsm {
                    code: asm.code,
                    inputs: asm
                        .inputs
                        .into_iter()
                        .map(|input| TypedAsmInput {
                            constraint: input.constraint,
                            value: self.instantiate_expr(input.value, substitutions),
                            span: input.span,
                        })
                        .collect(),
                    outputs: asm
                        .outputs
                        .into_iter()
                        .map(|output| TypedAsmOutput {
                            constraint: output.constraint,
                            place: self.instantiate_place(output.place, substitutions),
                            span: output.span,
                        })
                        .collect(),
                    clobbers: asm.clobbers,
                    options: asm.options,
                }),
                TypedExprKind::ArrayLiteral { elems } => TypedExprKind::ArrayLiteral {
                    elems: self.instantiate_array_elements(elems, substitutions),
                },
                TypedExprKind::StructLiteral { def_id, fields } => TypedExprKind::StructLiteral {
                    def_id,
                    fields: fields
                        .into_iter()
                        .map(|field| TypedFieldInit {
                            field: field.field,
                            name: field.name,
                            value: self.instantiate_expr(field.value, substitutions),
                            span: field.span,
                        })
                        .collect(),
                },
                TypedExprKind::UnionLiteral { def_id, field } => TypedExprKind::UnionLiteral {
                    def_id,
                    field: Box::new(TypedFieldInit {
                        field: field.field,
                        name: field.name,
                        value: self.instantiate_expr(field.value, substitutions),
                        span: field.span,
                    }),
                },
                TypedExprKind::Unary { op, expr } => TypedExprKind::Unary {
                    op,
                    expr: Box::new(self.instantiate_expr(*expr, substitutions)),
                },
                TypedExprKind::Binary { lhs, op, rhs } => TypedExprKind::Binary {
                    lhs: Box::new(self.instantiate_expr(*lhs, substitutions)),
                    op,
                    rhs: Box::new(self.instantiate_expr(*rhs, substitutions)),
                },
                TypedExprKind::Assign { place, op, rhs } => TypedExprKind::Assign {
                    place: self.instantiate_place(place, substitutions),
                    op,
                    rhs: Box::new(self.instantiate_expr(*rhs, substitutions)),
                },
                TypedExprKind::Cast { expr, ty } => TypedExprKind::Cast {
                    expr: Box::new(self.instantiate_expr(*expr, substitutions)),
                    ty: self.instantiate_ty(ty, substitutions),
                },
                TypedExprKind::Call { callee, args } => TypedExprKind::Call {
                    callee: self.instantiate_callee(callee, substitutions),
                    args: args
                        .into_iter()
                        .map(|arg| self.instantiate_expr(arg, substitutions))
                        .collect(),
                },
                TypedExprKind::Field { lhs, field } => TypedExprKind::Field {
                    lhs: Box::new(self.instantiate_expr(*lhs, substitutions)),
                    field,
                },
                TypedExprKind::Index { lhs, index } => TypedExprKind::Index {
                    lhs: Box::new(self.instantiate_expr(*lhs, substitutions)),
                    index: Box::new(self.instantiate_expr(*index, substitutions)),
                },
                TypedExprKind::Slice {
                    lhs,
                    range,
                    is_const,
                } => TypedExprKind::Slice {
                    lhs: Box::new(self.instantiate_expr(*lhs, substitutions)),
                    range: self.instantiate_slice_range(range, substitutions),
                    is_const,
                },
                TypedExprKind::Block(block) => {
                    TypedExprKind::Block(self.instantiate_body(block, substitutions))
                }
                TypedExprKind::If {
                    cond,
                    then_branch,
                    else_branch,
                } => TypedExprKind::If {
                    cond: Box::new(self.instantiate_expr(*cond, substitutions)),
                    then_branch: self.instantiate_body(then_branch, substitutions),
                    else_branch: else_branch
                        .map(|expr| Box::new(self.instantiate_expr(*expr, substitutions))),
                },
            },
        }
    }

    fn instantiate_callee(
        &mut self,
        callee: TypedCallee,
        substitutions: &HashMap<String, TyId>,
    ) -> TypedCallee {
        match callee {
            TypedCallee::Function(def_id) => TypedCallee::Function(def_id),
            TypedCallee::FunctionInstance { def_id, args } => TypedCallee::FunctionInstance {
                def_id,
                args: args
                    .into_iter()
                    .map(|arg| self.instantiate_ty(arg, substitutions))
                    .collect(),
            },
            TypedCallee::Method {
                def_id,
                args,
                receiver,
            } => TypedCallee::Method {
                def_id,
                args: args
                    .into_iter()
                    .map(|arg| self.instantiate_ty(arg, substitutions))
                    .collect(),
                receiver: Box::new(self.instantiate_expr(*receiver, substitutions)),
            },
            TypedCallee::FunctionPointer(expr) => {
                TypedCallee::FunctionPointer(Box::new(self.instantiate_expr(*expr, substitutions)))
            }
        }
    }

    fn instantiate_place(
        &mut self,
        place: TypedPlace,
        substitutions: &HashMap<String, TyId>,
    ) -> TypedPlace {
        TypedPlace {
            span: place.span,
            ty: self.instantiate_ty(place.ty, substitutions),
            base: place.base,
            elems: place
                .elems
                .into_iter()
                .map(|elem| match elem {
                    PlaceElem::Field(field) => PlaceElem::Field(field),
                    PlaceElem::Index(expr) => {
                        PlaceElem::Index(Box::new(self.instantiate_expr(*expr, substitutions)))
                    }
                })
                .collect(),
        }
    }

    fn instantiate_slice_range(
        &mut self,
        range: TypedSliceRange,
        substitutions: &HashMap<String, TyId>,
    ) -> TypedSliceRange {
        TypedSliceRange {
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
        elems: TypedArrayElements,
        substitutions: &HashMap<String, TyId>,
    ) -> TypedArrayElements {
        match elems {
            TypedArrayElements::List(elems) => TypedArrayElements::List(
                elems
                    .into_iter()
                    .map(|elem| self.instantiate_expr(elem, substitutions))
                    .collect(),
            ),
            TypedArrayElements::Repeat { value, count } => TypedArrayElements::Repeat {
                value: Box::new(self.instantiate_expr(*value, substitutions)),
                count,
            },
        }
    }

    pub(crate) fn instantiate_ty(
        &mut self,
        ty: TyId,
        substitutions: &HashMap<String, TyId>,
    ) -> TyId {
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
}
