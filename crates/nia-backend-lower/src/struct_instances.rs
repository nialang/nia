// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

use crate::ModuleLowerer;
use nia_backend_ir::{
    BackendField, BackendFunction, BackendFunctionInstance, BackendStructInstance,
    BackendUnionInstance,
};
use nia_defs::{DefId, DefKind};
use nia_function_ir::{
    FunctionArrayElements, FunctionBody, FunctionCallee, FunctionDeferBody, FunctionExpr,
    FunctionExprKind, FunctionForHeader, FunctionOp, FunctionPlace, FunctionPlaceBase,
    FunctionPlaceElem, FunctionTerminator,
};
use nia_ids::{GlobalDefId, InternedTyId};
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
        let keys = self
            .struct_layout_instances_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        keys.into_iter()
            .map(|key| {
                let substitutions =
                    ModuleLowerer::generic_substitutions(&signature.generics, &key.args);
                BackendStructInstance {
                    def_id: self.global_def_id(def_id),
                    name: item.name.clone(),
                    args: key.args.clone(),
                    symbol: self.mangle_instance_symbol(
                        self.global_def_id(def_id),
                        &item.name,
                        &key.args,
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
        let keys = self
            .union_layout_instances_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        keys.into_iter()
            .map(|key| {
                let substitutions =
                    ModuleLowerer::generic_substitutions(&signature.generics, &key.args);
                BackendUnionInstance {
                    def_id: self.global_def_id(def_id),
                    name: item.name.clone(),
                    args: key.args.clone(),
                    symbol: self.mangle_instance_symbol(
                        self.global_def_id(def_id),
                        &item.name,
                        &key.args,
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
            if let Some(body) = &function.function_body {
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
            if let Some(body) = &function.function_body {
                self.collect_struct_instances_body(body, &mut seen, struct_instances);
                self.collect_union_instances_body(body, &mut seen_unions, union_instances);
            }
        }
    }

    fn collect_struct_instances_body(
        &mut self,
        body: &FunctionBody,
        seen: &mut HashSet<(GlobalDefId, Vec<InternedTyId>)>,
        out: &mut Vec<BackendStructInstance>,
    ) {
        self.collect_struct_instance_ty(body.ty, seen, out);
        for local in &body.locals {
            self.collect_struct_instance_ty(local.ty, seen, out);
        }
        for block in &body.blocks {
            for op in &block.ops {
                self.collect_struct_instances_op(op, seen, out);
            }
            self.collect_struct_instances_terminator(&block.terminator, seen, out);
        }
    }

    fn collect_struct_instances_defer_body(
        &mut self,
        body: &FunctionDeferBody,
        seen: &mut HashSet<(GlobalDefId, Vec<InternedTyId>)>,
        out: &mut Vec<BackendStructInstance>,
    ) {
        for block in &body.blocks {
            for op in &block.ops {
                self.collect_struct_instances_op(op, seen, out);
            }
            self.collect_struct_instances_terminator(&block.terminator, seen, out);
        }
    }

    fn collect_struct_instances_op(
        &mut self,
        op: &FunctionOp,
        seen: &mut HashSet<(GlobalDefId, Vec<InternedTyId>)>,
        out: &mut Vec<BackendStructInstance>,
    ) {
        match op {
            FunctionOp::Binding(binding) => {
                self.collect_struct_instance_ty(binding.ty, seen, out);
                if let Some(value) = &binding.value {
                    self.collect_struct_instances_expr(value, seen, out);
                }
            }
            FunctionOp::StoreLocal { value, .. } | FunctionOp::Expr(value) => {
                self.collect_struct_instances_expr(value, seen, out);
            }
            FunctionOp::Defer(body) => {
                self.collect_struct_instances_defer_body(body, seen, out);
            }
        }
    }

    fn collect_struct_instances_terminator(
        &mut self,
        terminator: &FunctionTerminator,
        seen: &mut HashSet<(GlobalDefId, Vec<InternedTyId>)>,
        out: &mut Vec<BackendStructInstance>,
    ) {
        match terminator {
            FunctionTerminator::If { cond, .. } => {
                self.collect_struct_instances_expr(cond, seen, out);
            }
            FunctionTerminator::Switch { target, arms, .. } => {
                self.collect_struct_instances_expr(target, seen, out);
                for arm in arms {
                    self.collect_struct_instances_expr(&arm.pattern, seen, out);
                }
            }
            FunctionTerminator::Loop { header, .. } => {
                self.collect_struct_instances_for_header(header, seen, out);
            }
            FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
                if let Some(expr) = value {
                    self.collect_struct_instances_expr(expr, seen, out);
                }
            }
            FunctionTerminator::Branch { .. }
            | FunctionTerminator::Next { .. }
            | FunctionTerminator::Error { .. } => {}
        }
    }

    fn collect_struct_instances_for_header(
        &mut self,
        header: &FunctionForHeader,
        seen: &mut HashSet<(GlobalDefId, Vec<InternedTyId>)>,
        out: &mut Vec<BackendStructInstance>,
    ) {
        match header {
            FunctionForHeader::Infinite => {}
            FunctionForHeader::Condition(expr) => {
                self.collect_struct_instances_expr(expr, seen, out);
            }
        }
    }

    fn collect_struct_instances_expr(
        &mut self,
        expr: &FunctionExpr,
        seen: &mut HashSet<(GlobalDefId, Vec<InternedTyId>)>,
        out: &mut Vec<BackendStructInstance>,
    ) {
        self.collect_struct_instance_ty(expr.ty, seen, out);
        match &expr.kind {
            FunctionExprKind::ArrayLiteral { elems } => match elems {
                FunctionArrayElements::List(elems) => {
                    for elem in elems {
                        self.collect_struct_instances_expr(elem, seen, out);
                    }
                }
                FunctionArrayElements::Repeat { value, .. } => {
                    self.collect_struct_instances_expr(value, seen, out);
                }
            },
            FunctionExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    self.collect_struct_instances_expr(&field.value, seen, out);
                }
            }
            FunctionExprKind::UnionLiteral { field, .. } => {
                self.collect_struct_instances_expr(&field.value, seen, out);
            }
            FunctionExprKind::Unary { expr, .. }
            | FunctionExprKind::Cast { expr, .. }
            | FunctionExprKind::TraitObjectUpcast { expr, .. }
            | FunctionExprKind::TraitObjectCoercion { expr, .. } => {
                self.collect_struct_instances_expr(expr, seen, out);
            }
            FunctionExprKind::AddrOf(place) => {
                self.collect_struct_instances_place(place, seen, out);
            }
            FunctionExprKind::Binary { lhs, rhs, .. } => {
                self.collect_struct_instances_expr(lhs, seen, out);
                self.collect_struct_instances_expr(rhs, seen, out);
            }
            FunctionExprKind::Assign { place, rhs, .. } => {
                self.collect_struct_instances_place(place, seen, out);
                self.collect_struct_instances_expr(rhs, seen, out);
            }
            FunctionExprKind::Discard(expr) => {
                self.collect_struct_instances_expr(expr, seen, out);
            }
            FunctionExprKind::Range(range) => {
                if let Some(start) = &range.start {
                    self.collect_struct_instances_expr(start, seen, out);
                }
                if let Some(end) = &range.end {
                    self.collect_struct_instances_expr(end, seen, out);
                }
            }
            FunctionExprKind::CStringPointer { array, .. } => {
                self.collect_struct_instances_expr(array, seen, out);
            }
            FunctionExprKind::InlineAsm(asm) => {
                for input in &asm.inputs {
                    self.collect_struct_instances_expr(&input.value, seen, out);
                }
                for output in &asm.outputs {
                    self.collect_struct_instances_place(&output.place, seen, out);
                }
            }
            FunctionExprKind::Call { callee, args } => {
                self.collect_struct_instances_callee(callee, seen, out);
                for arg in args {
                    self.collect_struct_instances_expr(arg, seen, out);
                }
            }
            FunctionExprKind::Field { lhs, .. } => {
                self.collect_struct_instances_expr(lhs, seen, out);
            }
            FunctionExprKind::Index { lhs, index } => {
                self.collect_struct_instances_expr(lhs, seen, out);
                self.collect_struct_instances_expr(index, seen, out);
            }
            FunctionExprKind::Slice { lhs, range, .. } => {
                self.collect_struct_instances_expr(lhs, seen, out);
                if let Some(start) = &range.start {
                    self.collect_struct_instances_expr(start, seen, out);
                }
                if let Some(end) = &range.end {
                    self.collect_struct_instances_expr(end, seen, out);
                }
            }
            FunctionExprKind::FunctionInstance { args, .. } => {
                for arg in args {
                    self.collect_struct_instance_ty(*arg, seen, out);
                }
            }
            FunctionExprKind::Error
            | FunctionExprKind::Integer(_)
            | FunctionExprKind::Float(_)
            | FunctionExprKind::String(_)
            | FunctionExprKind::ByteString(_)
            | FunctionExprKind::Char(_)
            | FunctionExprKind::ByteChar(_)
            | FunctionExprKind::Bool(_)
            | FunctionExprKind::Local(_)
            | FunctionExprKind::Global(_)
            | FunctionExprKind::Function(_)
            | FunctionExprKind::EnumVariant(_)
            | FunctionExprKind::BuiltinValue(_) => {}
        }
    }

    fn collect_struct_instances_callee(
        &mut self,
        callee: &FunctionCallee,
        seen: &mut HashSet<(GlobalDefId, Vec<InternedTyId>)>,
        out: &mut Vec<BackendStructInstance>,
    ) {
        match callee {
            FunctionCallee::Function(_) => {}
            FunctionCallee::FunctionInstance { args, .. } => {
                for arg in args {
                    self.collect_struct_instance_ty(*arg, seen, out);
                }
            }
            FunctionCallee::Method { args, receiver, .. } => {
                for arg in args {
                    self.collect_struct_instance_ty(*arg, seen, out);
                }
                self.collect_struct_instances_expr(receiver, seen, out);
            }
            FunctionCallee::TraitMethod {
                self_ty,
                trait_args,
                args,
                receiver,
                ..
            } => {
                self.collect_struct_instance_ty(*self_ty, seen, out);
                for arg in trait_args {
                    self.collect_struct_instance_ty(*arg, seen, out);
                }
                for arg in args {
                    self.collect_struct_instance_ty(*arg, seen, out);
                }
                self.collect_struct_instances_expr(receiver, seen, out);
            }
            FunctionCallee::BuiltinPlaceMethod {
                self_ty,
                trait_args,
                receiver,
                ..
            } => {
                self.collect_struct_instance_ty(*self_ty, seen, out);
                for arg in trait_args {
                    self.collect_struct_instance_ty(*arg, seen, out);
                }
                self.collect_struct_instances_expr(receiver, seen, out);
            }
            FunctionCallee::DynamicTraitMethod {
                object_ty,
                trait_args,
                receiver,
                ..
            } => {
                self.collect_struct_instance_ty(*object_ty, seen, out);
                for arg in trait_args {
                    self.collect_struct_instance_ty(*arg, seen, out);
                }
                self.collect_struct_instances_expr(receiver, seen, out);
            }
            FunctionCallee::BuiltinOperator(_) => {}
            FunctionCallee::FunctionPointer(expr) => {
                self.collect_struct_instances_expr(expr, seen, out);
            }
        }
    }

    fn collect_struct_instances_place(
        &mut self,
        place: &FunctionPlace,
        seen: &mut HashSet<(GlobalDefId, Vec<InternedTyId>)>,
        out: &mut Vec<BackendStructInstance>,
    ) {
        self.collect_struct_instance_ty(place.ty, seen, out);
        if let FunctionPlaceBase::Deref(expr) = &place.base {
            self.collect_struct_instances_expr(expr, seen, out);
        }
        for elem in &place.elems {
            match elem {
                FunctionPlaceElem::Field(_) | FunctionPlaceElem::Error => {}
                FunctionPlaceElem::Index(expr) => {
                    self.collect_struct_instances_expr(expr, seen, out);
                }
            }
        }
    }

    fn collect_struct_instance_ty(
        &mut self,
        ty: InternedTyId,
        seen: &mut HashSet<(GlobalDefId, Vec<InternedTyId>)>,
        out: &mut Vec<BackendStructInstance>,
    ) {
        match self.interner.get(ty).cloned() {
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::Array { elem, .. }) => {
                self.collect_struct_instance_ty(elem, seen, out);
            }
            Some(TyKind::Range { bound, .. }) => {
                if let Some(bound) = bound {
                    self.collect_struct_instance_ty(bound, seen, out);
                }
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
            Some(TyKind::BuiltinTrait { args, .. }) => {
                for arg in args {
                    self.collect_struct_instance_ty(arg, seen, out);
                }
            }
            Some(TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            }) => {
                for arg in trait_args {
                    self.collect_struct_instance_ty(arg, seen, out);
                }
                for (_, ty) in associated_type_bindings {
                    self.collect_struct_instance_ty(ty, seen, out);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.collect_struct_instance_ty(self_ty, seen, out);
                for arg in trait_args {
                    self.collect_struct_instance_ty(arg, seen, out);
                }
            }
            Some(TyKind::Error | TyKind::GenericParam(_) | TyKind::Primitive(_)) | None => {}
        }
    }

    fn lower_struct_instance(
        &mut self,
        def_id: DefId,
        args: Vec<InternedTyId>,
    ) -> Option<BackendStructInstance> {
        let signature = self.input.signatures.structs.get(&def_id)?.clone();
        if signature.generics.is_empty() || signature.generics.len() != args.len() {
            return None;
        }
        let def = self.input.defs.defs.get(def_id)?;
        let substitutions = ModuleLowerer::generic_substitutions(&signature.generics, &args);
        Some(BackendStructInstance {
            def_id: self.global_def_id(def_id),
            name: def.name.clone(),
            args: args.clone(),
            symbol: self.mangle_instance_symbol(self.global_def_id(def_id), &def.name, &args),
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
        body: &FunctionBody,
        seen: &mut HashSet<(GlobalDefId, Vec<InternedTyId>)>,
        out: &mut Vec<BackendUnionInstance>,
    ) {
        self.collect_union_instance_ty(body.ty, seen, out);
        for local in &body.locals {
            self.collect_union_instance_ty(local.ty, seen, out);
        }
        for block in &body.blocks {
            for op in &block.ops {
                self.collect_union_instances_op(op, seen, out);
            }
            self.collect_union_instances_terminator(&block.terminator, seen, out);
        }
    }

    fn collect_union_instances_defer_body(
        &mut self,
        body: &FunctionDeferBody,
        seen: &mut HashSet<(GlobalDefId, Vec<InternedTyId>)>,
        out: &mut Vec<BackendUnionInstance>,
    ) {
        for block in &body.blocks {
            for op in &block.ops {
                self.collect_union_instances_op(op, seen, out);
            }
            self.collect_union_instances_terminator(&block.terminator, seen, out);
        }
    }

    fn collect_union_instances_op(
        &mut self,
        op: &FunctionOp,
        seen: &mut HashSet<(GlobalDefId, Vec<InternedTyId>)>,
        out: &mut Vec<BackendUnionInstance>,
    ) {
        match op {
            FunctionOp::Binding(binding) => {
                self.collect_union_instance_ty(binding.ty, seen, out);
                if let Some(value) = &binding.value {
                    self.collect_union_instances_expr(value, seen, out);
                }
            }
            FunctionOp::StoreLocal { value, .. } | FunctionOp::Expr(value) => {
                self.collect_union_instances_expr(value, seen, out);
            }
            FunctionOp::Defer(body) => {
                self.collect_union_instances_defer_body(body, seen, out);
            }
        }
    }

    fn collect_union_instances_terminator(
        &mut self,
        terminator: &FunctionTerminator,
        seen: &mut HashSet<(GlobalDefId, Vec<InternedTyId>)>,
        out: &mut Vec<BackendUnionInstance>,
    ) {
        match terminator {
            FunctionTerminator::If { cond, .. } => {
                self.collect_union_instances_expr(cond, seen, out);
            }
            FunctionTerminator::Switch { target, arms, .. } => {
                self.collect_union_instances_expr(target, seen, out);
                for arm in arms {
                    self.collect_union_instances_expr(&arm.pattern, seen, out);
                }
            }
            FunctionTerminator::Loop { header, .. } => {
                self.collect_union_instances_for_header(header, seen, out);
            }
            FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
                if let Some(expr) = value {
                    self.collect_union_instances_expr(expr, seen, out);
                }
            }
            FunctionTerminator::Branch { .. }
            | FunctionTerminator::Next { .. }
            | FunctionTerminator::Error { .. } => {}
        }
    }

    fn collect_union_instances_for_header(
        &mut self,
        header: &FunctionForHeader,
        seen: &mut HashSet<(GlobalDefId, Vec<InternedTyId>)>,
        out: &mut Vec<BackendUnionInstance>,
    ) {
        match header {
            FunctionForHeader::Infinite => {}
            FunctionForHeader::Condition(expr) => {
                self.collect_union_instances_expr(expr, seen, out);
            }
        }
    }

    fn collect_union_instances_expr(
        &mut self,
        expr: &FunctionExpr,
        seen: &mut HashSet<(GlobalDefId, Vec<InternedTyId>)>,
        out: &mut Vec<BackendUnionInstance>,
    ) {
        self.collect_union_instance_ty(expr.ty, seen, out);
        match &expr.kind {
            FunctionExprKind::ArrayLiteral { elems } => match elems {
                FunctionArrayElements::List(elems) => {
                    for elem in elems {
                        self.collect_union_instances_expr(elem, seen, out);
                    }
                }
                FunctionArrayElements::Repeat { value, .. } => {
                    self.collect_union_instances_expr(value, seen, out);
                }
            },
            FunctionExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    self.collect_union_instances_expr(&field.value, seen, out);
                }
            }
            FunctionExprKind::UnionLiteral { field, .. } => {
                self.collect_union_instances_expr(&field.value, seen, out);
            }
            FunctionExprKind::Unary { expr, .. }
            | FunctionExprKind::Cast { expr, .. }
            | FunctionExprKind::TraitObjectUpcast { expr, .. }
            | FunctionExprKind::TraitObjectCoercion { expr, .. } => {
                self.collect_union_instances_expr(expr, seen, out);
            }
            FunctionExprKind::AddrOf(place) => {
                self.collect_union_instances_place(place, seen, out);
            }
            FunctionExprKind::Binary { lhs, rhs, .. } => {
                self.collect_union_instances_expr(lhs, seen, out);
                self.collect_union_instances_expr(rhs, seen, out);
            }
            FunctionExprKind::Assign { place, rhs, .. } => {
                self.collect_union_instances_place(place, seen, out);
                self.collect_union_instances_expr(rhs, seen, out);
            }
            FunctionExprKind::Discard(expr) => {
                self.collect_union_instances_expr(expr, seen, out);
            }
            FunctionExprKind::Range(range) => {
                if let Some(start) = &range.start {
                    self.collect_union_instances_expr(start, seen, out);
                }
                if let Some(end) = &range.end {
                    self.collect_union_instances_expr(end, seen, out);
                }
            }
            FunctionExprKind::CStringPointer { array, .. } => {
                self.collect_union_instances_expr(array, seen, out);
            }
            FunctionExprKind::InlineAsm(asm) => {
                for input in &asm.inputs {
                    self.collect_union_instances_expr(&input.value, seen, out);
                }
                for output in &asm.outputs {
                    self.collect_union_instances_place(&output.place, seen, out);
                }
            }
            FunctionExprKind::Call { callee, args } => {
                self.collect_union_instances_callee(callee, seen, out);
                for arg in args {
                    self.collect_union_instances_expr(arg, seen, out);
                }
            }
            FunctionExprKind::Field { lhs, .. } => {
                self.collect_union_instances_expr(lhs, seen, out);
            }
            FunctionExprKind::Index { lhs, index } => {
                self.collect_union_instances_expr(lhs, seen, out);
                self.collect_union_instances_expr(index, seen, out);
            }
            FunctionExprKind::Slice { lhs, range, .. } => {
                self.collect_union_instances_expr(lhs, seen, out);
                if let Some(start) = &range.start {
                    self.collect_union_instances_expr(start, seen, out);
                }
                if let Some(end) = &range.end {
                    self.collect_union_instances_expr(end, seen, out);
                }
            }
            FunctionExprKind::FunctionInstance { args, .. } => {
                for arg in args {
                    self.collect_union_instance_ty(*arg, seen, out);
                }
            }
            FunctionExprKind::Error
            | FunctionExprKind::Integer(_)
            | FunctionExprKind::Float(_)
            | FunctionExprKind::String(_)
            | FunctionExprKind::ByteString(_)
            | FunctionExprKind::Char(_)
            | FunctionExprKind::ByteChar(_)
            | FunctionExprKind::Bool(_)
            | FunctionExprKind::Local(_)
            | FunctionExprKind::Global(_)
            | FunctionExprKind::Function(_)
            | FunctionExprKind::EnumVariant(_)
            | FunctionExprKind::BuiltinValue(_) => {}
        }
    }

    fn collect_union_instances_callee(
        &mut self,
        callee: &FunctionCallee,
        seen: &mut HashSet<(GlobalDefId, Vec<InternedTyId>)>,
        out: &mut Vec<BackendUnionInstance>,
    ) {
        match callee {
            FunctionCallee::Function(_) => {}
            FunctionCallee::FunctionInstance { args, .. } => {
                for arg in args {
                    self.collect_union_instance_ty(*arg, seen, out);
                }
            }
            FunctionCallee::Method { args, receiver, .. } => {
                for arg in args {
                    self.collect_union_instance_ty(*arg, seen, out);
                }
                self.collect_union_instances_expr(receiver, seen, out);
            }
            FunctionCallee::TraitMethod {
                self_ty,
                trait_args,
                args,
                receiver,
                ..
            } => {
                self.collect_union_instance_ty(*self_ty, seen, out);
                for arg in trait_args {
                    self.collect_union_instance_ty(*arg, seen, out);
                }
                for arg in args {
                    self.collect_union_instance_ty(*arg, seen, out);
                }
                self.collect_union_instances_expr(receiver, seen, out);
            }
            FunctionCallee::BuiltinPlaceMethod {
                self_ty,
                trait_args,
                receiver,
                ..
            } => {
                self.collect_union_instance_ty(*self_ty, seen, out);
                for arg in trait_args {
                    self.collect_union_instance_ty(*arg, seen, out);
                }
                self.collect_union_instances_expr(receiver, seen, out);
            }
            FunctionCallee::DynamicTraitMethod {
                object_ty,
                trait_args,
                receiver,
                ..
            } => {
                self.collect_union_instance_ty(*object_ty, seen, out);
                for arg in trait_args {
                    self.collect_union_instance_ty(*arg, seen, out);
                }
                self.collect_union_instances_expr(receiver, seen, out);
            }
            FunctionCallee::BuiltinOperator(_) => {}
            FunctionCallee::FunctionPointer(expr) => {
                self.collect_union_instances_expr(expr, seen, out);
            }
        }
    }

    fn collect_union_instances_place(
        &mut self,
        place: &FunctionPlace,
        seen: &mut HashSet<(GlobalDefId, Vec<InternedTyId>)>,
        out: &mut Vec<BackendUnionInstance>,
    ) {
        self.collect_union_instance_ty(place.ty, seen, out);
        if let FunctionPlaceBase::Deref(expr) = &place.base {
            self.collect_union_instances_expr(expr, seen, out);
        }
        for elem in &place.elems {
            match elem {
                FunctionPlaceElem::Field(_) | FunctionPlaceElem::Error => {}
                FunctionPlaceElem::Index(expr) => {
                    self.collect_union_instances_expr(expr, seen, out);
                }
            }
        }
    }

    fn collect_union_instance_ty(
        &mut self,
        ty: InternedTyId,
        seen: &mut HashSet<(GlobalDefId, Vec<InternedTyId>)>,
        out: &mut Vec<BackendUnionInstance>,
    ) {
        match self.interner.get(ty).cloned() {
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::Array { elem, .. }) => {
                self.collect_union_instance_ty(elem, seen, out);
            }
            Some(TyKind::Range { bound, .. }) => {
                if let Some(bound) = bound {
                    self.collect_union_instance_ty(bound, seen, out);
                }
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
            Some(TyKind::BuiltinTrait { args, .. }) => {
                for arg in args {
                    self.collect_union_instance_ty(arg, seen, out);
                }
            }
            Some(TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            }) => {
                for arg in trait_args {
                    self.collect_union_instance_ty(arg, seen, out);
                }
                for (_, ty) in associated_type_bindings {
                    self.collect_union_instance_ty(ty, seen, out);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.collect_union_instance_ty(self_ty, seen, out);
                for arg in trait_args {
                    self.collect_union_instance_ty(arg, seen, out);
                }
            }
            Some(TyKind::Error | TyKind::GenericParam(_) | TyKind::Primitive(_)) | None => {}
        }
    }

    fn lower_union_instance(
        &mut self,
        def_id: DefId,
        args: Vec<InternedTyId>,
    ) -> Option<BackendUnionInstance> {
        let signature = self.input.signatures.unions.get(&def_id)?.clone();
        if signature.generics.is_empty() || signature.generics.len() != args.len() {
            return None;
        }
        let def = self.input.defs.defs.get(def_id)?;
        let substitutions = ModuleLowerer::generic_substitutions(&signature.generics, &args);
        Some(BackendUnionInstance {
            def_id: self.global_def_id(def_id),
            name: def.name.clone(),
            args: args.clone(),
            symbol: self.mangle_instance_symbol(self.global_def_id(def_id), &def.name, &args),
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
