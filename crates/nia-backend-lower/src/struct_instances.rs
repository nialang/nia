// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

use crate::ModuleLowerer;
use nia_backend_ir::{
    BackendField, BackendFunction, BackendFunctionInstance, BackendStructInstance,
    BackendUnionInstance, TypedArrayElements, TypedBody, TypedExpr, TypedExprKind, TypedForHeader,
    TypedForInit, TypedStmt, TypedStmtKind, TypedSwitchArmBody, TypedSwitchPattern,
};
use nia_defs::{DefId, DefKind};
use nia_ids::{GlobalDefId, TyId};
use nia_mangle::mangle_instance_symbol;
use nia_span::Span;
use nia_ty::TyKind;

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn lower_struct_instances(
        &mut self,
        span: Span,
        item: &nia_ast::StructItem,
    ) -> Vec<BackendStructInstance> {
        let Some(def_id) = self.def_id_for_span(span, DefKind::Struct) else {
            return Vec::new();
        };
        let Some(signature) = self.input.signatures.structs.get(&def_id) else {
            return Vec::new();
        };
        if signature.generics.is_empty() {
            return Vec::new();
        }
        self.input
            .layouts
            .struct_instances
            .iter()
            .filter(|(key, _)| key.def_id == def_id)
            .map(|(key, _)| {
                let substitutions = self.generic_substitutions(&signature.generics, &key.args);
                BackendStructInstance {
                    def_id: self.global_def_id(def_id),
                    name: item.name.clone(),
                    args: key.args.clone(),
                    symbol: mangle_instance_symbol(
                        self.global_def_id(def_id),
                        &item.name,
                        &key.args,
                        &self.interner,
                        |def_id| self.def_name(def_id.def_id),
                    ),
                    fields: signature
                        .fields
                        .iter()
                        .map(|field| BackendField {
                            def_id: self.global_def_id(field.def_id),
                            name: field.name.clone(),
                            ty: self.instantiate_ty(field.ty, &substitutions),
                            span: field.span,
                        })
                        .collect(),
                    is_extern: signature.is_extern,
                    span,
                }
            })
            .collect()
    }

    pub(crate) fn lower_union_instances(
        &mut self,
        span: Span,
        item: &nia_ast::UnionItem,
    ) -> Vec<BackendUnionInstance> {
        let Some(def_id) = self.def_id_for_span(span, DefKind::Union) else {
            return Vec::new();
        };
        let Some(signature) = self.input.signatures.unions.get(&def_id) else {
            return Vec::new();
        };
        if signature.generics.is_empty() {
            return Vec::new();
        }
        self.input
            .layouts
            .union_instances
            .iter()
            .filter(|(key, _)| key.def_id == def_id)
            .map(|(key, _)| {
                let substitutions = self.generic_substitutions(&signature.generics, &key.args);
                BackendUnionInstance {
                    def_id: self.global_def_id(def_id),
                    name: item.name.clone(),
                    args: key.args.clone(),
                    symbol: mangle_instance_symbol(
                        self.global_def_id(def_id),
                        &item.name,
                        &key.args,
                        &self.interner,
                        |def_id| self.def_name(def_id.def_id),
                    ),
                    fields: signature
                        .fields
                        .iter()
                        .map(|field| BackendField {
                            def_id: self.global_def_id(field.def_id),
                            name: field.name.clone(),
                            ty: self.instantiate_ty(field.ty, &substitutions),
                            span: field.span,
                        })
                        .collect(),
                    is_extern: signature.is_extern,
                    span,
                }
            })
            .collect()
    }

    pub(crate) fn extend_struct_instances_from_functions(
        &mut self,
        struct_instances: &mut Vec<BackendStructInstance>,
        union_instances: &mut Vec<BackendUnionInstance>,
        functions: &[BackendFunction],
        function_instances: &[BackendFunctionInstance],
    ) {
        let mut seen = struct_instances
            .iter()
            .map(|item| (item.def_id, item.args.clone()))
            .collect::<HashSet<_>>();
        let mut seen_unions = union_instances
            .iter()
            .map(|item| (item.def_id, item.args.clone()))
            .collect::<HashSet<_>>();
        for function in functions {
            self.collect_struct_instance_ty(function.return_type, &mut seen, struct_instances);
            self.collect_union_instance_ty(function.return_type, &mut seen_unions, union_instances);
            for param in &function.params {
                self.collect_struct_instance_ty(param.ty, &mut seen, struct_instances);
                self.collect_union_instance_ty(param.ty, &mut seen_unions, union_instances);
            }
            if let Some(body) = &function.body {
                self.collect_struct_instances_body(body, &mut seen, struct_instances);
                self.collect_union_instances_body(body, &mut seen_unions, union_instances);
            }
        }
        for function in function_instances {
            self.collect_struct_instance_ty(function.return_type, &mut seen, struct_instances);
            self.collect_union_instance_ty(function.return_type, &mut seen_unions, union_instances);
            for param in &function.params {
                self.collect_struct_instance_ty(param.ty, &mut seen, struct_instances);
                self.collect_union_instance_ty(param.ty, &mut seen_unions, union_instances);
            }
            if let Some(body) = &function.body {
                self.collect_struct_instances_body(body, &mut seen, struct_instances);
                self.collect_union_instances_body(body, &mut seen_unions, union_instances);
            }
        }
    }

    fn collect_struct_instances_body(
        &mut self,
        body: &TypedBody,
        seen: &mut HashSet<(GlobalDefId, Vec<TyId>)>,
        out: &mut Vec<BackendStructInstance>,
    ) {
        self.collect_struct_instance_ty(body.ty, seen, out);
        for local in &body.locals {
            self.collect_struct_instance_ty(local.ty, seen, out);
        }
        for stmt in &body.stmts {
            self.collect_struct_instances_stmt(stmt, seen, out);
        }
        if let Some(tail) = &body.tail {
            self.collect_struct_instances_expr(tail, seen, out);
        }
    }

    fn collect_struct_instances_stmt(
        &mut self,
        stmt: &TypedStmt,
        seen: &mut HashSet<(GlobalDefId, Vec<TyId>)>,
        out: &mut Vec<BackendStructInstance>,
    ) {
        match &stmt.kind {
            TypedStmtKind::Binding(binding) => {
                self.collect_struct_instance_ty(binding.ty, seen, out);
                if let Some(value) = &binding.value {
                    self.collect_struct_instances_expr(value, seen, out);
                }
            }
            TypedStmtKind::Expr(expr) | TypedStmtKind::Defer(expr) => {
                self.collect_struct_instances_expr(expr, seen, out);
            }
            TypedStmtKind::Return(Some(expr)) => {
                self.collect_struct_instances_expr(expr, seen, out)
            }
            TypedStmtKind::Return(None) | TypedStmtKind::Break | TypedStmtKind::Continue => {}
            TypedStmtKind::For(for_stmt) => {
                match &for_stmt.header {
                    TypedForHeader::Infinite => {}
                    TypedForHeader::Condition(expr) => {
                        self.collect_struct_instances_expr(expr, seen, out);
                    }
                    TypedForHeader::CStyle { init, cond, step } => {
                        if let Some(init) = init {
                            match &**init {
                                TypedForInit::Binding(binding) => {
                                    self.collect_struct_instance_ty(binding.ty, seen, out);
                                    if let Some(value) = &binding.value {
                                        self.collect_struct_instances_expr(value, seen, out);
                                    }
                                }
                                TypedForInit::Expr(expr) => {
                                    self.collect_struct_instances_expr(expr, seen, out);
                                }
                            }
                        }
                        if let Some(cond) = cond {
                            self.collect_struct_instances_expr(cond, seen, out);
                        }
                        if let Some(step) = step {
                            self.collect_struct_instances_expr(step, seen, out);
                        }
                    }
                }
                self.collect_struct_instances_body(&for_stmt.body, seen, out);
            }
            TypedStmtKind::Switch(switch) => {
                self.collect_struct_instances_expr(&switch.target, seen, out);
                for arm in &switch.arms {
                    if let TypedSwitchPattern::Expr(expr) = &arm.pattern {
                        self.collect_struct_instances_expr(expr, seen, out);
                    }
                    match &arm.body {
                        TypedSwitchArmBody::Expr(expr) => {
                            self.collect_struct_instances_expr(expr, seen, out);
                        }
                        TypedSwitchArmBody::Stmt(stmt) => {
                            self.collect_struct_instances_stmt(stmt, seen, out);
                        }
                        TypedSwitchArmBody::Block(body) => {
                            self.collect_struct_instances_body(body, seen, out);
                        }
                    }
                }
            }
        }
    }

    fn collect_struct_instances_expr(
        &mut self,
        expr: &TypedExpr,
        seen: &mut HashSet<(GlobalDefId, Vec<TyId>)>,
        out: &mut Vec<BackendStructInstance>,
    ) {
        self.collect_struct_instance_ty(expr.ty, seen, out);
        match &expr.kind {
            TypedExprKind::ArrayLiteral { elems } => match elems {
                TypedArrayElements::List(elems) => {
                    for elem in elems {
                        self.collect_struct_instances_expr(elem, seen, out);
                    }
                }
                TypedArrayElements::Repeat { value, .. } => {
                    self.collect_struct_instances_expr(value, seen, out);
                }
            },
            TypedExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    self.collect_struct_instances_expr(&field.value, seen, out);
                }
            }
            TypedExprKind::UnionLiteral { field, .. } => {
                self.collect_struct_instances_expr(&field.value, seen, out);
            }
            TypedExprKind::Unary { expr, .. } | TypedExprKind::Cast { expr, .. } => {
                self.collect_struct_instances_expr(expr, seen, out);
            }
            TypedExprKind::Binary { lhs, rhs, .. } => {
                self.collect_struct_instances_expr(lhs, seen, out);
                self.collect_struct_instances_expr(rhs, seen, out);
            }
            TypedExprKind::Assign { place, rhs, .. } => {
                self.collect_struct_instance_ty(place.ty, seen, out);
                self.collect_struct_instances_expr(rhs, seen, out);
            }
            TypedExprKind::Len(inner) | TypedExprKind::Ptr(inner) => {
                self.collect_struct_instances_expr(inner, seen, out);
            }
            TypedExprKind::InlineAsm(asm) => {
                for input in &asm.inputs {
                    self.collect_struct_instances_expr(&input.value, seen, out);
                }
                for output in &asm.outputs {
                    self.collect_struct_instance_ty(output.place.ty, seen, out);
                }
            }
            TypedExprKind::Call { args, .. } => {
                for arg in args {
                    self.collect_struct_instances_expr(arg, seen, out);
                }
            }
            TypedExprKind::Field { lhs, .. } | TypedExprKind::Index { lhs, .. } => {
                self.collect_struct_instances_expr(lhs, seen, out);
            }
            TypedExprKind::Slice { lhs, range, .. } => {
                self.collect_struct_instances_expr(lhs, seen, out);
                if let Some(start) = &range.start {
                    self.collect_struct_instances_expr(start, seen, out);
                }
                if let Some(end) = &range.end {
                    self.collect_struct_instances_expr(end, seen, out);
                }
            }
            TypedExprKind::Block(body) => self.collect_struct_instances_body(body, seen, out),
            TypedExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.collect_struct_instances_expr(cond, seen, out);
                self.collect_struct_instances_body(then_branch, seen, out);
                if let Some(else_branch) = else_branch {
                    self.collect_struct_instances_expr(else_branch, seen, out);
                }
            }
            TypedExprKind::Error
            | TypedExprKind::Integer(_)
            | TypedExprKind::Float(_)
            | TypedExprKind::String(_)
            | TypedExprKind::ByteString(_)
            | TypedExprKind::Char(_)
            | TypedExprKind::ByteChar(_)
            | TypedExprKind::Bool(_)
            | TypedExprKind::Local(_)
            | TypedExprKind::Global(_)
            | TypedExprKind::Function(_)
            | TypedExprKind::FunctionInstance { .. }
            | TypedExprKind::EnumVariant(_)
            | TypedExprKind::BuiltinValue(_) => {}
        }
    }

    fn collect_struct_instance_ty(
        &mut self,
        ty: TyId,
        seen: &mut HashSet<(GlobalDefId, Vec<TyId>)>,
        out: &mut Vec<BackendStructInstance>,
    ) {
        match self.interner.get(ty).cloned() {
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::Array { elem, .. }) => {
                self.collect_struct_instance_ty(elem, seen, out);
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                for param in params {
                    self.collect_struct_instance_ty(param, seen, out);
                }
                self.collect_struct_instance_ty(return_type, seen, out);
            }
            Some(TyKind::Nominal { def_id, args }) => {
                for arg in &args {
                    self.collect_struct_instance_ty(*arg, seen, out);
                }
                if def_id.module_id == self.input.module_id
                    && !args.is_empty()
                    && seen.insert((def_id, args.clone()))
                    && let Some(item) = self.lower_struct_instance(def_id.def_id, args)
                {
                    out.push(item);
                }
            }
            Some(TyKind::Error | TyKind::GenericParam(_) | TyKind::Primitive(_)) | None => {}
        }
    }

    fn lower_struct_instance(
        &mut self,
        def_id: DefId,
        args: Vec<TyId>,
    ) -> Option<BackendStructInstance> {
        let signature = self.input.signatures.structs.get(&def_id)?.clone();
        if signature.generics.is_empty() || signature.generics.len() != args.len() {
            return None;
        }
        let def = self.input.defs.defs.get(def_id)?;
        let substitutions = self.generic_substitutions(&signature.generics, &args);
        Some(BackendStructInstance {
            def_id: self.global_def_id(def_id),
            name: def.name.clone(),
            args: args.clone(),
            symbol: mangle_instance_symbol(
                self.global_def_id(def_id),
                &def.name,
                &args,
                &self.interner,
                |def_id| self.def_name(def_id.def_id),
            ),
            fields: signature
                .fields
                .iter()
                .map(|field| BackendField {
                    def_id: self.global_def_id(field.def_id),
                    name: field.name.clone(),
                    ty: self.instantiate_ty(field.ty, &substitutions),
                    span: field.span,
                })
                .collect(),
            is_extern: signature.is_extern,
            span: signature.span,
        })
    }

    fn collect_union_instances_body(
        &mut self,
        body: &TypedBody,
        seen: &mut HashSet<(GlobalDefId, Vec<TyId>)>,
        out: &mut Vec<BackendUnionInstance>,
    ) {
        self.collect_union_instance_ty(body.ty, seen, out);
        for local in &body.locals {
            self.collect_union_instance_ty(local.ty, seen, out);
        }
        for stmt in &body.stmts {
            self.collect_union_instances_stmt(stmt, seen, out);
        }
        if let Some(tail) = &body.tail {
            self.collect_union_instances_expr(tail, seen, out);
        }
    }

    fn collect_union_instances_stmt(
        &mut self,
        stmt: &TypedStmt,
        seen: &mut HashSet<(GlobalDefId, Vec<TyId>)>,
        out: &mut Vec<BackendUnionInstance>,
    ) {
        match &stmt.kind {
            TypedStmtKind::Binding(binding) => {
                self.collect_union_instance_ty(binding.ty, seen, out);
                if let Some(value) = &binding.value {
                    self.collect_union_instances_expr(value, seen, out);
                }
            }
            TypedStmtKind::Expr(expr) | TypedStmtKind::Defer(expr) => {
                self.collect_union_instances_expr(expr, seen, out);
            }
            TypedStmtKind::Return(Some(expr)) => self.collect_union_instances_expr(expr, seen, out),
            TypedStmtKind::Return(None) | TypedStmtKind::Break | TypedStmtKind::Continue => {}
            TypedStmtKind::For(for_stmt) => {
                match &for_stmt.header {
                    TypedForHeader::Infinite => {}
                    TypedForHeader::Condition(expr) => {
                        self.collect_union_instances_expr(expr, seen, out);
                    }
                    TypedForHeader::CStyle { init, cond, step } => {
                        if let Some(init) = init {
                            match &**init {
                                TypedForInit::Binding(binding) => {
                                    self.collect_union_instance_ty(binding.ty, seen, out);
                                    if let Some(value) = &binding.value {
                                        self.collect_union_instances_expr(value, seen, out);
                                    }
                                }
                                TypedForInit::Expr(expr) => {
                                    self.collect_union_instances_expr(expr, seen, out);
                                }
                            }
                        }
                        if let Some(cond) = cond {
                            self.collect_union_instances_expr(cond, seen, out);
                        }
                        if let Some(step) = step {
                            self.collect_union_instances_expr(step, seen, out);
                        }
                    }
                }
                self.collect_union_instances_body(&for_stmt.body, seen, out);
            }
            TypedStmtKind::Switch(switch) => {
                self.collect_union_instances_expr(&switch.target, seen, out);
                for arm in &switch.arms {
                    if let TypedSwitchPattern::Expr(expr) = &arm.pattern {
                        self.collect_union_instances_expr(expr, seen, out);
                    }
                    match &arm.body {
                        TypedSwitchArmBody::Expr(expr) => {
                            self.collect_union_instances_expr(expr, seen, out);
                        }
                        TypedSwitchArmBody::Stmt(stmt) => {
                            self.collect_union_instances_stmt(stmt, seen, out);
                        }
                        TypedSwitchArmBody::Block(body) => {
                            self.collect_union_instances_body(body, seen, out);
                        }
                    }
                }
            }
        }
    }

    fn collect_union_instances_expr(
        &mut self,
        expr: &TypedExpr,
        seen: &mut HashSet<(GlobalDefId, Vec<TyId>)>,
        out: &mut Vec<BackendUnionInstance>,
    ) {
        self.collect_union_instance_ty(expr.ty, seen, out);
        match &expr.kind {
            TypedExprKind::ArrayLiteral { elems } => match elems {
                TypedArrayElements::List(elems) => {
                    for elem in elems {
                        self.collect_union_instances_expr(elem, seen, out);
                    }
                }
                TypedArrayElements::Repeat { value, .. } => {
                    self.collect_union_instances_expr(value, seen, out);
                }
            },
            TypedExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    self.collect_union_instances_expr(&field.value, seen, out);
                }
            }
            TypedExprKind::UnionLiteral { field, .. } => {
                self.collect_union_instances_expr(&field.value, seen, out);
            }
            TypedExprKind::Unary { expr, .. } | TypedExprKind::Cast { expr, .. } => {
                self.collect_union_instances_expr(expr, seen, out);
            }
            TypedExprKind::Binary { lhs, rhs, .. } => {
                self.collect_union_instances_expr(lhs, seen, out);
                self.collect_union_instances_expr(rhs, seen, out);
            }
            TypedExprKind::Assign { place, rhs, .. } => {
                self.collect_union_instance_ty(place.ty, seen, out);
                self.collect_union_instances_expr(rhs, seen, out);
            }
            TypedExprKind::Len(inner) | TypedExprKind::Ptr(inner) => {
                self.collect_union_instances_expr(inner, seen, out);
            }
            TypedExprKind::InlineAsm(asm) => {
                for input in &asm.inputs {
                    self.collect_union_instances_expr(&input.value, seen, out);
                }
                for output in &asm.outputs {
                    self.collect_union_instance_ty(output.place.ty, seen, out);
                }
            }
            TypedExprKind::Call { args, .. } => {
                for arg in args {
                    self.collect_union_instances_expr(arg, seen, out);
                }
            }
            TypedExprKind::Field { lhs, .. } | TypedExprKind::Index { lhs, .. } => {
                self.collect_union_instances_expr(lhs, seen, out);
            }
            TypedExprKind::Slice { lhs, range, .. } => {
                self.collect_union_instances_expr(lhs, seen, out);
                if let Some(start) = &range.start {
                    self.collect_union_instances_expr(start, seen, out);
                }
                if let Some(end) = &range.end {
                    self.collect_union_instances_expr(end, seen, out);
                }
            }
            TypedExprKind::Block(body) => self.collect_union_instances_body(body, seen, out),
            TypedExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.collect_union_instances_expr(cond, seen, out);
                self.collect_union_instances_body(then_branch, seen, out);
                if let Some(else_branch) = else_branch {
                    self.collect_union_instances_expr(else_branch, seen, out);
                }
            }
            TypedExprKind::Error
            | TypedExprKind::Integer(_)
            | TypedExprKind::Float(_)
            | TypedExprKind::String(_)
            | TypedExprKind::ByteString(_)
            | TypedExprKind::Char(_)
            | TypedExprKind::ByteChar(_)
            | TypedExprKind::Bool(_)
            | TypedExprKind::Local(_)
            | TypedExprKind::Global(_)
            | TypedExprKind::Function(_)
            | TypedExprKind::FunctionInstance { .. }
            | TypedExprKind::EnumVariant(_)
            | TypedExprKind::BuiltinValue(_) => {}
        }
    }

    fn collect_union_instance_ty(
        &mut self,
        ty: TyId,
        seen: &mut HashSet<(GlobalDefId, Vec<TyId>)>,
        out: &mut Vec<BackendUnionInstance>,
    ) {
        match self.interner.get(ty).cloned() {
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::Array { elem, .. }) => {
                self.collect_union_instance_ty(elem, seen, out);
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                for param in params {
                    self.collect_union_instance_ty(param, seen, out);
                }
                self.collect_union_instance_ty(return_type, seen, out);
            }
            Some(TyKind::Nominal { def_id, args }) => {
                for arg in &args {
                    self.collect_union_instance_ty(*arg, seen, out);
                }
                if def_id.module_id == self.input.module_id
                    && !args.is_empty()
                    && seen.insert((def_id, args.clone()))
                    && let Some(item) = self.lower_union_instance(def_id.def_id, args)
                {
                    out.push(item);
                }
            }
            Some(TyKind::Error | TyKind::GenericParam(_) | TyKind::Primitive(_)) | None => {}
        }
    }

    fn lower_union_instance(
        &mut self,
        def_id: DefId,
        args: Vec<TyId>,
    ) -> Option<BackendUnionInstance> {
        let signature = self.input.signatures.unions.get(&def_id)?.clone();
        if signature.generics.is_empty() || signature.generics.len() != args.len() {
            return None;
        }
        let def = self.input.defs.defs.get(def_id)?;
        let substitutions = self.generic_substitutions(&signature.generics, &args);
        Some(BackendUnionInstance {
            def_id: self.global_def_id(def_id),
            name: def.name.clone(),
            args: args.clone(),
            symbol: mangle_instance_symbol(
                self.global_def_id(def_id),
                &def.name,
                &args,
                &self.interner,
                |def_id| self.def_name(def_id.def_id),
            ),
            fields: signature
                .fields
                .iter()
                .map(|field| BackendField {
                    def_id: self.global_def_id(field.def_id),
                    name: field.name.clone(),
                    ty: self.instantiate_ty(field.ty, &substitutions),
                    span: field.span,
                })
                .collect(),
            is_extern: signature.is_extern,
            span: signature.span,
        })
    }
}
