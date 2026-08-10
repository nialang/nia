// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

use crate::{ModuleLowerer, function_instances::contains_generic_param};
use nia_backend_ir::{
    BackendField, BackendFunction, BackendFunctionInstance, BackendStructInstance,
    BackendUnionInstance,
};
use nia_defs::DefKind;
use nia_function_ir::{
    FunctionArrayElements, FunctionBody, FunctionCallee, FunctionDeferBody, FunctionExpr,
    FunctionExprKind, FunctionForHeader, FunctionMemoryIntrinsicSource, FunctionOp, FunctionPlace,
    FunctionPlaceBase, FunctionPlaceElem, FunctionTerminator,
};
use nia_ids::{GlobalDefId, InternedTyId};
use nia_node_id::VersionedNodeKey;
use nia_span::Span;
use nia_ty::{ConstGenericArg, TyKind};

type AggregateInstanceKey = (GlobalDefId, Vec<InternedTyId>, Vec<ConstGenericArg>);

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn lower_struct_instances(
        &mut self,
        node_key: &VersionedNodeKey,
        span: Span,
        item: &nia_ast::StructItem,
    ) -> Vec<BackendStructInstance> {
        let Some(def_id) = self.def_id_for_node(node_key, DefKind::Struct) else {
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
        let mut instances = Vec::new();
        for key in keys {
            if self.instance_args_contain_generic_param(&key.args) {
                continue;
            }
            let global_def_id = self.global_def_id(def_id);
            let (substitutions, const_substitutions) = self
                .generic_substitutions_and_consts_for_def(
                    global_def_id,
                    &key.args,
                    &key.const_args,
                );
            let substitution_id =
                self.intern_type_and_const_substitutions(&substitutions, &const_substitutions);
            instances.push(BackendStructInstance {
                def_id: global_def_id,
                name: item.name,
                args: key.args.clone(),
                const_args: key.const_args.clone(),
                symbol: self.mangle_instance_symbol(
                    self.global_def_id(def_id),
                    item.name,
                    None,
                    &key.args,
                    &key.const_args,
                ),
                fields: signature
                    .fields
                    .iter()
                    .map(|field| BackendField {
                        def_id: self.global_def_id(field.def_id),
                        name: field.name,
                        ty: self.instantiate_ty_with_id(field.ty, substitution_id),
                        span: field.span,
                    })
                    .collect(),
                is_extern: signature.is_extern,
                span,
            });
        }
        instances
    }

    pub(crate) fn lower_union_instances(
        &mut self,
        node_key: &VersionedNodeKey,
        span: Span,
        item: &nia_ast::UnionItem,
    ) -> Vec<BackendUnionInstance> {
        let Some(def_id) = self.def_id_for_node(node_key, DefKind::Union) else {
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
        let mut instances = Vec::new();
        for key in keys {
            if self.instance_args_contain_generic_param(&key.args) {
                continue;
            }
            let global_def_id = self.global_def_id(def_id);
            let (substitutions, const_substitutions) = self
                .generic_substitutions_and_consts_for_def(
                    global_def_id,
                    &key.args,
                    &key.const_args,
                );
            let substitution_id =
                self.intern_type_and_const_substitutions(&substitutions, &const_substitutions);
            instances.push(BackendUnionInstance {
                def_id: global_def_id,
                name: item.name,
                args: key.args.clone(),
                const_args: key.const_args.clone(),
                symbol: self.mangle_instance_symbol(
                    self.global_def_id(def_id),
                    item.name,
                    None,
                    &key.args,
                    &key.const_args,
                ),
                fields: signature
                    .fields
                    .iter()
                    .map(|field| BackendField {
                        def_id: self.global_def_id(field.def_id),
                        name: field.name,
                        ty: self.instantiate_ty_with_id(field.ty, substitution_id),
                        span: field.span,
                    })
                    .collect(),
                is_extern: signature.is_extern,
                span,
            });
        }
        instances
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
            .map(|item| (item.def_id, item.args.clone(), item.const_args.clone()))
            .collect::<HashSet<_>>();
        let mut seen_unions = union_instances
            .iter()
            .map(|item| (item.def_id, item.args.clone(), item.const_args.clone()))
            .collect::<HashSet<_>>();
        for function in functions {
            self.collect_struct_instance_ty(function.return_type, &mut seen, struct_instances);
            self.collect_union_instance_ty(function.return_type, &mut seen_unions, union_instances);
            for param in &function.params {
                self.collect_struct_instance_ty(param.passing_ty, &mut seen, struct_instances);
                self.collect_union_instance_ty(param.passing_ty, &mut seen_unions, union_instances);
                self.collect_struct_instance_ty(param.local_ty, &mut seen, struct_instances);
                self.collect_union_instance_ty(param.local_ty, &mut seen_unions, union_instances);
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
                self.collect_struct_instance_ty(param.passing_ty, &mut seen, struct_instances);
                self.collect_union_instance_ty(param.passing_ty, &mut seen_unions, union_instances);
                self.collect_struct_instance_ty(param.local_ty, &mut seen, struct_instances);
                self.collect_union_instance_ty(param.local_ty, &mut seen_unions, union_instances);
            }
            if let Some(body) = &function.function_body {
                self.collect_struct_instances_body(body, &mut seen, struct_instances);
                self.collect_union_instances_body(body, &mut seen_unions, union_instances);
            }
        }
        let mut struct_index = 0usize;
        let mut union_index = 0usize;
        while struct_index < struct_instances.len() || union_index < union_instances.len() {
            while struct_index < struct_instances.len() {
                let fields = struct_instances[struct_index].fields.clone();
                for field in fields {
                    self.collect_struct_instance_ty(field.ty, &mut seen, struct_instances);
                    self.collect_union_instance_ty(field.ty, &mut seen_unions, union_instances);
                }
                struct_index += 1;
            }
            while union_index < union_instances.len() {
                let fields = union_instances[union_index].fields.clone();
                for field in fields {
                    self.collect_struct_instance_ty(field.ty, &mut seen, struct_instances);
                    self.collect_union_instance_ty(field.ty, &mut seen_unions, union_instances);
                }
                union_index += 1;
            }
        }
    }

    fn collect_struct_instances_body(
        &mut self,
        body: &FunctionBody,
        seen: &mut HashSet<AggregateInstanceKey>,
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
        seen: &mut HashSet<AggregateInstanceKey>,
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
        seen: &mut HashSet<AggregateInstanceKey>,
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
            FunctionOp::MemoryIntrinsic(memory) => {
                self.collect_struct_instance_ty(memory.elem_ty, seen, out);
                self.collect_struct_instances_expr(&memory.dest, seen, out);
                match &memory.source {
                    FunctionMemoryIntrinsicSource::Slice(source)
                    | FunctionMemoryIntrinsicSource::Byte(source) => {
                        self.collect_struct_instances_expr(source, seen, out);
                    }
                }
            }
            FunctionOp::Defer(body) => {
                self.collect_struct_instances_defer_body(body, seen, out);
            }
        }
    }

    fn collect_struct_instances_terminator(
        &mut self,
        terminator: &FunctionTerminator,
        seen: &mut HashSet<AggregateInstanceKey>,
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
            FunctionTerminator::Try {
                value,
                error_conversion,
                ..
            } => {
                self.collect_struct_instances_expr(value, seen, out);
                if let Some(conversion) = error_conversion {
                    self.collect_struct_instances_expr(conversion, seen, out);
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
        seen: &mut HashSet<AggregateInstanceKey>,
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
        seen: &mut HashSet<AggregateInstanceKey>,
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
            FunctionExprKind::Tuple(elems) => {
                for elem in elems {
                    self.collect_struct_instances_expr(elem, seen, out);
                }
            }
            FunctionExprKind::TupleField { value, .. } => {
                self.collect_struct_instances_expr(value, seen, out)
            }
            FunctionExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    self.collect_struct_instances_expr(&field.value, seen, out);
                }
            }
            FunctionExprKind::UnionLiteral { field, .. } => {
                self.collect_struct_instances_expr(&field.value, seen, out);
            }
            FunctionExprKind::Unary { expr, .. }
            | FunctionExprKind::OptionalSome { expr }
            | FunctionExprKind::ErrorOk { expr }
            | FunctionExprKind::ErrorErr { expr }
            | FunctionExprKind::TaggedUnionTag { expr }
            | FunctionExprKind::TaggedUnionPayload { expr }
            | FunctionExprKind::Try { expr }
            | FunctionExprKind::LoadUnaligned { ptr: expr, .. }
            | FunctionExprKind::Splat { value: expr }
            | FunctionExprKind::Bitmask { vector: expr }
            | FunctionExprKind::BitIntrinsic { value: expr, .. }
            | FunctionExprKind::CharFromU32 { value: expr }
            | FunctionExprKind::Cast { expr, .. }
            | FunctionExprKind::TraitObjectUpcast { expr, .. }
            | FunctionExprKind::TraitObjectCoercion { expr, .. }
            | FunctionExprKind::RangeBound { range: expr, .. } => {
                self.collect_struct_instances_expr(expr, seen, out);
            }
            FunctionExprKind::AddrOf(place) => {
                self.collect_struct_instances_place(place, seen, out);
            }
            FunctionExprKind::Binary { lhs, rhs, .. } => {
                self.collect_struct_instances_expr(lhs, seen, out);
                self.collect_struct_instances_expr(rhs, seen, out);
            }
            FunctionExprKind::ExtractElement { vector, index } => {
                self.collect_struct_instances_expr(vector, seen, out);
                self.collect_struct_instances_expr(index, seen, out);
            }
            FunctionExprKind::InsertElement {
                vector,
                index,
                value,
            } => {
                self.collect_struct_instances_expr(vector, seen, out);
                self.collect_struct_instances_expr(index, seen, out);
                self.collect_struct_instances_expr(value, seen, out);
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
            FunctionExprKind::StaticArrayPointer { array, .. } => {
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
            FunctionExprKind::Atomic(atomic) => {
                self.collect_struct_instances_atomic(atomic, seen, out);
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
            FunctionExprKind::GlobalInstance { args, .. } => {
                for arg in args {
                    self.collect_struct_instance_ty(*arg, seen, out);
                }
            }
            FunctionExprKind::Error => {
                crate::input::unreachable_invalid_function_ir("FunctionExprKind::Error")
            }
            FunctionExprKind::EnumVariant { fields, .. } => {
                for field in fields {
                    self.collect_struct_instances_expr(field, seen, out);
                }
            }
            FunctionExprKind::EnumTag { value }
            | FunctionExprKind::EnumPayloadField { value, .. } => {
                self.collect_struct_instances_expr(value, seen, out);
            }
            FunctionExprKind::Trap
            | FunctionExprKind::Integer(_)
            | FunctionExprKind::Float(_)
            | FunctionExprKind::String(_)
            | FunctionExprKind::ByteString(_)
            | FunctionExprKind::Char(_)
            | FunctionExprKind::ByteChar(_)
            | FunctionExprKind::Bool(_)
            | FunctionExprKind::Null
            | FunctionExprKind::ConstGeneric(_)
            | FunctionExprKind::Local(_)
            | FunctionExprKind::Global(_)
            | FunctionExprKind::Function(_)
            | FunctionExprKind::EnumVariantTag(_)
            | FunctionExprKind::BuiltinValue(_) => {}
            FunctionExprKind::UnionStorageLiteral { relocations, .. } => {
                for relocation in relocations {
                    self.collect_struct_instances_expr(&relocation.pointee, seen, out);
                }
            }
        }
    }

    fn collect_struct_instances_atomic(
        &mut self,
        atomic: &nia_function_ir::FunctionAtomic,
        seen: &mut HashSet<AggregateInstanceKey>,
        out: &mut Vec<BackendStructInstance>,
    ) {
        match atomic {
            nia_function_ir::FunctionAtomic::Load { ty, ptr, .. } => {
                self.collect_struct_instance_ty(*ty, seen, out);
                self.collect_struct_instances_expr(ptr, seen, out);
            }
            nia_function_ir::FunctionAtomic::Store { ty, ptr, value, .. }
            | nia_function_ir::FunctionAtomic::Rmw { ty, ptr, value, .. } => {
                self.collect_struct_instance_ty(*ty, seen, out);
                self.collect_struct_instances_expr(ptr, seen, out);
                self.collect_struct_instances_expr(value, seen, out);
            }
            nia_function_ir::FunctionAtomic::Cmpxchg {
                ty,
                ptr,
                expected,
                desired,
                ..
            } => {
                self.collect_struct_instance_ty(*ty, seen, out);
                self.collect_struct_instances_expr(ptr, seen, out);
                self.collect_struct_instances_expr(expected, seen, out);
                self.collect_struct_instances_expr(desired, seen, out);
            }
            nia_function_ir::FunctionAtomic::Fence { .. } => {}
        }
    }

    fn collect_struct_instances_callee(
        &mut self,
        callee: &FunctionCallee,
        seen: &mut HashSet<AggregateInstanceKey>,
        out: &mut Vec<BackendStructInstance>,
    ) {
        match callee {
            FunctionCallee::ClosureEntry { state, .. } => {
                self.collect_struct_instances_expr(state, seen, out);
            }
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
            FunctionCallee::TraitAssociatedFunction {
                self_ty,
                trait_args,
                args,
                ..
            } => {
                self.collect_struct_instance_ty(*self_ty, seen, out);
                for arg in trait_args {
                    self.collect_struct_instance_ty(*arg, seen, out);
                }
                for arg in args {
                    self.collect_struct_instance_ty(*arg, seen, out);
                }
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
            FunctionCallee::BuiltinMethod {
                self_ty, receiver, ..
            } => {
                self.collect_struct_instance_ty(*self_ty, seen, out);
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
        seen: &mut HashSet<AggregateInstanceKey>,
        out: &mut Vec<BackendStructInstance>,
    ) {
        self.collect_struct_instance_ty(place.ty, seen, out);
        match &place.base {
            FunctionPlaceBase::Deref(expr) => {
                self.collect_struct_instances_expr(expr, seen, out);
            }
            FunctionPlaceBase::Local(_) | FunctionPlaceBase::Global(_) => {}
            FunctionPlaceBase::GlobalInstance { args, .. } => {
                for arg in args {
                    self.collect_struct_instance_ty(*arg, seen, out);
                }
            }
            FunctionPlaceBase::Error => {
                crate::input::unreachable_invalid_function_ir("FunctionPlaceBase::Error")
            }
        }
        for elem in &place.elems {
            match elem {
                FunctionPlaceElem::Field(_) | FunctionPlaceElem::TupleField(_) => {}
                FunctionPlaceElem::Error => {
                    crate::input::unreachable_invalid_function_ir("FunctionPlaceElem::Error")
                }
                FunctionPlaceElem::Index(expr) => {
                    self.collect_struct_instances_expr(expr, seen, out);
                }
            }
        }
    }

    fn collect_struct_instance_ty(
        &mut self,
        ty: InternedTyId,
        seen: &mut HashSet<AggregateInstanceKey>,
        out: &mut Vec<BackendStructInstance>,
    ) {
        match self.ty_kind(ty).cloned() {
            Some(TyKind::Tuple(elems)) => {
                for elem in elems {
                    self.collect_struct_instance_ty(elem, seen, out);
                }
            }
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::VolatilePointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem })
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
            })
            | Some(TyKind::Callable {
                params,
                return_type,
                ..
            })
            | Some(TyKind::CallablePointee {
                params,
                return_type,
            }) => {
                for param in params {
                    self.collect_struct_instance_ty(param, seen, out);
                }
                self.collect_struct_instance_ty(return_type, seen, out);
            }
            Some(TyKind::Optional { elem }) => self.collect_struct_instance_ty(elem, seen, out),
            Some(TyKind::ErrorUnion { error, value }) => {
                self.collect_struct_instance_ty(error, seen, out);
                self.collect_struct_instance_ty(value, seen, out);
            }
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                for arg in &args {
                    self.collect_struct_instance_ty(*arg, seen, out);
                }
                if seen.insert((def_id, args.clone(), const_args.clone())) {
                    if !args.is_empty() || !const_args.is_empty() {
                        if let Some(item) = self.lower_struct_instance(def_id, args, const_args) {
                            out.push(item);
                        }
                    } else {
                        for field_ty in self.struct_field_tys(def_id) {
                            self.collect_struct_instance_ty(field_ty, seen, out);
                        }
                    }
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
            })
            | Some(TyKind::TraitObjectPointee {
                trait_args,
                associated_type_bindings,
                ..
            }) => {
                for arg in trait_args {
                    self.collect_struct_instance_ty(arg, seen, out);
                }
                for binding in associated_type_bindings {
                    for arg in binding.trait_args {
                        self.collect_struct_instance_ty(arg, seen, out);
                    }
                    self.collect_struct_instance_ty(binding.ty, seen, out);
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
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::Opaque
                | TyKind::GenericParam(_)
                | TyKind::SelfParam
                | TyKind::BuiltinType(_)
                | TyKind::Primitive(_)
                | TyKind::Vector { .. }
                | TyKind::ClosureState { .. },
            )
            | None => {}
        }
    }

    fn lower_struct_instance(
        &mut self,
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
    ) -> Option<BackendStructInstance> {
        if self.instance_args_contain_generic_param(&args) {
            return None;
        }
        if def_id.module_id != self.input.module_id {
            return self.lower_foreign_struct_instance(def_id, args, const_args);
        }
        let signature = self.input.signatures.structs.get(&def_id.def_id)?.clone();
        if signature.generics.is_empty()
            || signature.generics.len() != args.len() + const_args.len()
        {
            return None;
        }
        let def = self.input.defs.defs.get(def_id.def_id)?;
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, &args, &const_args);
        let substitution_id =
            self.intern_type_and_const_substitutions(&substitutions, &const_substitutions);
        Some(BackendStructInstance {
            def_id,
            name: def.name,
            args: args.clone(),
            const_args: const_args.clone(),
            symbol: self.mangle_instance_symbol(def_id, def.name, None, &args, &const_args),
            fields: signature
                .fields
                .iter()
                .map(|field| BackendField {
                    def_id: GlobalDefId {
                        module_id: self.input.module_id,
                        def_id: field.def_id,
                    },
                    name: field.name,
                    ty: self.instantiate_ty_with_id(field.ty, substitution_id),
                    span: field.span,
                })
                .collect(),
            is_extern: signature.is_extern,
            span: signature.span,
        })
    }

    pub(crate) fn struct_field_tys(&mut self, def_id: GlobalDefId) -> Vec<InternedTyId> {
        if def_id.module_id != self.input.module_id {
            let Some(program_signature) = self.input.program.structs().get(&def_id) else {
                return Vec::new();
            };
            return program_signature
                .signature
                .fields
                .iter()
                .map(|field| self.normalized_type_from_module(def_id.module_id, field.ty))
                .collect();
        }
        let Some(signature) = self.input.signatures.structs.get(&def_id.def_id) else {
            return Vec::new();
        };
        signature.fields.iter().map(|field| field.ty).collect()
    }

    fn lower_foreign_struct_instance(
        &mut self,
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
    ) -> Option<BackendStructInstance> {
        if self.instance_args_contain_generic_param(&args) {
            return None;
        }
        let program_signature = self.input.program.structs().get(&def_id)?.clone();
        let signature = program_signature.signature;
        if signature.generics.is_empty()
            || signature.generics.len() != args.len() + const_args.len()
        {
            return None;
        }
        let symbol_name = self.def_symbol_name(def_id)?;
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, &args, &const_args);
        let substitution_id =
            self.intern_type_and_const_substitutions(&substitutions, &const_substitutions);
        Some(BackendStructInstance {
            def_id,
            name: symbol_name,
            args: args.clone(),
            const_args: const_args.clone(),
            symbol: self.mangle_instance_symbol(def_id, symbol_name, None, &args, &const_args),
            fields: signature
                .fields
                .iter()
                .map(|field| {
                    let ty = self.normalized_type_from_module(def_id.module_id, field.ty);
                    BackendField {
                        def_id: GlobalDefId {
                            module_id: def_id.module_id,
                            def_id: field.def_id,
                        },
                        name: field.name,
                        ty: self.instantiate_ty_with_id(ty, substitution_id),
                        span: field.span,
                    }
                })
                .collect(),
            is_extern: signature.is_extern,
            span: signature.span,
        })
    }

    fn collect_union_instances_body(
        &mut self,
        body: &FunctionBody,
        seen: &mut HashSet<AggregateInstanceKey>,
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
        seen: &mut HashSet<AggregateInstanceKey>,
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
        seen: &mut HashSet<AggregateInstanceKey>,
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
            FunctionOp::MemoryIntrinsic(memory) => {
                self.collect_union_instance_ty(memory.elem_ty, seen, out);
                self.collect_union_instances_expr(&memory.dest, seen, out);
                match &memory.source {
                    FunctionMemoryIntrinsicSource::Slice(source)
                    | FunctionMemoryIntrinsicSource::Byte(source) => {
                        self.collect_union_instances_expr(source, seen, out);
                    }
                }
            }
            FunctionOp::Defer(body) => {
                self.collect_union_instances_defer_body(body, seen, out);
            }
        }
    }

    fn collect_union_instances_terminator(
        &mut self,
        terminator: &FunctionTerminator,
        seen: &mut HashSet<AggregateInstanceKey>,
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
            FunctionTerminator::Try {
                value,
                error_conversion,
                ..
            } => {
                self.collect_union_instances_expr(value, seen, out);
                if let Some(conversion) = error_conversion {
                    self.collect_union_instances_expr(conversion, seen, out);
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
        seen: &mut HashSet<AggregateInstanceKey>,
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
        seen: &mut HashSet<AggregateInstanceKey>,
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
            FunctionExprKind::Tuple(elems) => {
                for elem in elems {
                    self.collect_union_instances_expr(elem, seen, out);
                }
            }
            FunctionExprKind::TupleField { value, .. } => {
                self.collect_union_instances_expr(value, seen, out)
            }
            FunctionExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    self.collect_union_instances_expr(&field.value, seen, out);
                }
            }
            FunctionExprKind::UnionLiteral { field, .. } => {
                self.collect_union_instances_expr(&field.value, seen, out);
            }
            FunctionExprKind::Unary { expr, .. }
            | FunctionExprKind::OptionalSome { expr }
            | FunctionExprKind::ErrorOk { expr }
            | FunctionExprKind::ErrorErr { expr }
            | FunctionExprKind::TaggedUnionTag { expr }
            | FunctionExprKind::TaggedUnionPayload { expr }
            | FunctionExprKind::Try { expr }
            | FunctionExprKind::LoadUnaligned { ptr: expr, .. }
            | FunctionExprKind::Splat { value: expr }
            | FunctionExprKind::Bitmask { vector: expr }
            | FunctionExprKind::BitIntrinsic { value: expr, .. }
            | FunctionExprKind::CharFromU32 { value: expr }
            | FunctionExprKind::Cast { expr, .. }
            | FunctionExprKind::TraitObjectUpcast { expr, .. }
            | FunctionExprKind::TraitObjectCoercion { expr, .. }
            | FunctionExprKind::RangeBound { range: expr, .. } => {
                self.collect_union_instances_expr(expr, seen, out);
            }
            FunctionExprKind::AddrOf(place) => {
                self.collect_union_instances_place(place, seen, out);
            }
            FunctionExprKind::Binary { lhs, rhs, .. } => {
                self.collect_union_instances_expr(lhs, seen, out);
                self.collect_union_instances_expr(rhs, seen, out);
            }
            FunctionExprKind::ExtractElement { vector, index } => {
                self.collect_union_instances_expr(vector, seen, out);
                self.collect_union_instances_expr(index, seen, out);
            }
            FunctionExprKind::InsertElement {
                vector,
                index,
                value,
            } => {
                self.collect_union_instances_expr(vector, seen, out);
                self.collect_union_instances_expr(index, seen, out);
                self.collect_union_instances_expr(value, seen, out);
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
            FunctionExprKind::StaticArrayPointer { array, .. } => {
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
            FunctionExprKind::Atomic(atomic) => {
                self.collect_union_instances_atomic(atomic, seen, out);
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
            FunctionExprKind::GlobalInstance { args, .. } => {
                for arg in args {
                    self.collect_union_instance_ty(*arg, seen, out);
                }
            }
            FunctionExprKind::Error => {
                crate::input::unreachable_invalid_function_ir("FunctionExprKind::Error")
            }
            FunctionExprKind::EnumVariant { fields, .. } => {
                for field in fields {
                    self.collect_union_instances_expr(field, seen, out);
                }
            }
            FunctionExprKind::EnumTag { value }
            | FunctionExprKind::EnumPayloadField { value, .. } => {
                self.collect_union_instances_expr(value, seen, out);
            }
            FunctionExprKind::Trap
            | FunctionExprKind::Integer(_)
            | FunctionExprKind::Float(_)
            | FunctionExprKind::String(_)
            | FunctionExprKind::ByteString(_)
            | FunctionExprKind::Char(_)
            | FunctionExprKind::ByteChar(_)
            | FunctionExprKind::Bool(_)
            | FunctionExprKind::Null
            | FunctionExprKind::ConstGeneric(_)
            | FunctionExprKind::Local(_)
            | FunctionExprKind::Global(_)
            | FunctionExprKind::Function(_)
            | FunctionExprKind::EnumVariantTag(_)
            | FunctionExprKind::BuiltinValue(_) => {}
            FunctionExprKind::UnionStorageLiteral { relocations, .. } => {
                for relocation in relocations {
                    self.collect_union_instances_expr(&relocation.pointee, seen, out);
                }
            }
        }
    }

    fn collect_union_instances_atomic(
        &mut self,
        atomic: &nia_function_ir::FunctionAtomic,
        seen: &mut HashSet<AggregateInstanceKey>,
        out: &mut Vec<BackendUnionInstance>,
    ) {
        match atomic {
            nia_function_ir::FunctionAtomic::Load { ty, ptr, .. } => {
                self.collect_union_instance_ty(*ty, seen, out);
                self.collect_union_instances_expr(ptr, seen, out);
            }
            nia_function_ir::FunctionAtomic::Store { ty, ptr, value, .. }
            | nia_function_ir::FunctionAtomic::Rmw { ty, ptr, value, .. } => {
                self.collect_union_instance_ty(*ty, seen, out);
                self.collect_union_instances_expr(ptr, seen, out);
                self.collect_union_instances_expr(value, seen, out);
            }
            nia_function_ir::FunctionAtomic::Cmpxchg {
                ty,
                ptr,
                expected,
                desired,
                ..
            } => {
                self.collect_union_instance_ty(*ty, seen, out);
                self.collect_union_instances_expr(ptr, seen, out);
                self.collect_union_instances_expr(expected, seen, out);
                self.collect_union_instances_expr(desired, seen, out);
            }
            nia_function_ir::FunctionAtomic::Fence { .. } => {}
        }
    }

    fn collect_union_instances_callee(
        &mut self,
        callee: &FunctionCallee,
        seen: &mut HashSet<AggregateInstanceKey>,
        out: &mut Vec<BackendUnionInstance>,
    ) {
        match callee {
            FunctionCallee::ClosureEntry { state, .. } => {
                self.collect_union_instances_expr(state, seen, out);
            }
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
            FunctionCallee::TraitAssociatedFunction {
                self_ty,
                trait_args,
                args,
                ..
            } => {
                self.collect_union_instance_ty(*self_ty, seen, out);
                for arg in trait_args {
                    self.collect_union_instance_ty(*arg, seen, out);
                }
                for arg in args {
                    self.collect_union_instance_ty(*arg, seen, out);
                }
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
            FunctionCallee::BuiltinMethod {
                self_ty, receiver, ..
            } => {
                self.collect_union_instance_ty(*self_ty, seen, out);
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
        seen: &mut HashSet<AggregateInstanceKey>,
        out: &mut Vec<BackendUnionInstance>,
    ) {
        self.collect_union_instance_ty(place.ty, seen, out);
        match &place.base {
            FunctionPlaceBase::Deref(expr) => {
                self.collect_union_instances_expr(expr, seen, out);
            }
            FunctionPlaceBase::Local(_) | FunctionPlaceBase::Global(_) => {}
            FunctionPlaceBase::GlobalInstance { args, .. } => {
                for arg in args {
                    self.collect_union_instance_ty(*arg, seen, out);
                }
            }
            FunctionPlaceBase::Error => {
                crate::input::unreachable_invalid_function_ir("FunctionPlaceBase::Error")
            }
        }
        for elem in &place.elems {
            match elem {
                FunctionPlaceElem::Field(_) | FunctionPlaceElem::TupleField(_) => {}
                FunctionPlaceElem::Error => {
                    crate::input::unreachable_invalid_function_ir("FunctionPlaceElem::Error")
                }
                FunctionPlaceElem::Index(expr) => {
                    self.collect_union_instances_expr(expr, seen, out);
                }
            }
        }
    }

    fn collect_union_instance_ty(
        &mut self,
        ty: InternedTyId,
        seen: &mut HashSet<AggregateInstanceKey>,
        out: &mut Vec<BackendUnionInstance>,
    ) {
        match self.ty_kind(ty).cloned() {
            Some(TyKind::Tuple(elems)) => {
                for elem in elems {
                    self.collect_union_instance_ty(elem, seen, out);
                }
            }
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::VolatilePointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem })
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
            })
            | Some(TyKind::Callable {
                params,
                return_type,
                ..
            })
            | Some(TyKind::CallablePointee {
                params,
                return_type,
            }) => {
                for param in params {
                    self.collect_union_instance_ty(param, seen, out);
                }
                self.collect_union_instance_ty(return_type, seen, out);
            }
            Some(TyKind::Optional { elem }) => self.collect_union_instance_ty(elem, seen, out),
            Some(TyKind::ErrorUnion { error, value }) => {
                self.collect_union_instance_ty(error, seen, out);
                self.collect_union_instance_ty(value, seen, out);
            }
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                for arg in &args {
                    self.collect_union_instance_ty(*arg, seen, out);
                }
                if seen.insert((def_id, args.clone(), const_args.clone())) {
                    if !args.is_empty() || !const_args.is_empty() {
                        if let Some(item) = self.lower_union_instance(def_id, args, const_args) {
                            out.push(item);
                        }
                    } else {
                        for field_ty in self.union_field_tys(def_id) {
                            self.collect_union_instance_ty(field_ty, seen, out);
                        }
                    }
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
            })
            | Some(TyKind::TraitObjectPointee {
                trait_args,
                associated_type_bindings,
                ..
            }) => {
                for arg in trait_args {
                    self.collect_union_instance_ty(arg, seen, out);
                }
                for binding in associated_type_bindings {
                    for arg in binding.trait_args {
                        self.collect_union_instance_ty(arg, seen, out);
                    }
                    self.collect_union_instance_ty(binding.ty, seen, out);
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
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::Opaque
                | TyKind::GenericParam(_)
                | TyKind::SelfParam
                | TyKind::BuiltinType(_)
                | TyKind::Primitive(_)
                | TyKind::Vector { .. }
                | TyKind::ClosureState { .. },
            )
            | None => {}
        }
    }

    fn lower_union_instance(
        &mut self,
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
    ) -> Option<BackendUnionInstance> {
        if self.instance_args_contain_generic_param(&args) {
            return None;
        }
        if def_id.module_id != self.input.module_id {
            return self.lower_foreign_union_instance(def_id, args, const_args);
        }
        let signature = self.input.signatures.unions.get(&def_id.def_id)?.clone();
        if signature.generics.is_empty()
            || signature.generics.len() != args.len() + const_args.len()
        {
            return None;
        }
        let def = self.input.defs.defs.get(def_id.def_id)?;
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, &args, &const_args);
        let substitution_id =
            self.intern_type_and_const_substitutions(&substitutions, &const_substitutions);
        Some(BackendUnionInstance {
            def_id,
            name: def.name,
            args: args.clone(),
            const_args: const_args.clone(),
            symbol: self.mangle_instance_symbol(def_id, def.name, None, &args, &const_args),
            fields: signature
                .fields
                .iter()
                .map(|field| BackendField {
                    def_id: GlobalDefId {
                        module_id: self.input.module_id,
                        def_id: field.def_id,
                    },
                    name: field.name,
                    ty: self.instantiate_ty_with_id(field.ty, substitution_id),
                    span: field.span,
                })
                .collect(),
            is_extern: signature.is_extern,
            span: signature.span,
        })
    }

    pub(crate) fn union_field_tys(&mut self, def_id: GlobalDefId) -> Vec<InternedTyId> {
        if def_id.module_id != self.input.module_id {
            let Some(program_signature) = self.input.program.unions().get(&def_id) else {
                return Vec::new();
            };
            return program_signature
                .signature
                .fields
                .iter()
                .map(|field| self.normalized_type_from_module(def_id.module_id, field.ty))
                .collect();
        }
        let Some(signature) = self.input.signatures.unions.get(&def_id.def_id) else {
            return Vec::new();
        };
        signature.fields.iter().map(|field| field.ty).collect()
    }

    fn lower_foreign_union_instance(
        &mut self,
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
    ) -> Option<BackendUnionInstance> {
        if self.instance_args_contain_generic_param(&args) {
            return None;
        }
        let program_signature = self.input.program.unions().get(&def_id)?.clone();
        let signature = program_signature.signature;
        if signature.generics.is_empty()
            || signature.generics.len() != args.len() + const_args.len()
        {
            return None;
        }
        let symbol_name = self.def_symbol_name(def_id)?;
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, &args, &const_args);
        let substitution_id =
            self.intern_type_and_const_substitutions(&substitutions, &const_substitutions);
        Some(BackendUnionInstance {
            def_id,
            name: symbol_name,
            args: args.clone(),
            const_args: const_args.clone(),
            symbol: self.mangle_instance_symbol(def_id, symbol_name, None, &args, &const_args),
            fields: signature
                .fields
                .iter()
                .map(|field| {
                    let ty = self.normalized_type_from_module(def_id.module_id, field.ty);
                    BackendField {
                        def_id: GlobalDefId {
                            module_id: def_id.module_id,
                            def_id: field.def_id,
                        },
                        name: field.name,
                        ty: self.instantiate_ty_with_id(ty, substitution_id),
                        span: field.span,
                    }
                })
                .collect(),
            is_extern: signature.is_extern,
            span: signature.span,
        })
    }

    fn instance_args_contain_generic_param(&mut self, args: &[InternedTyId]) -> bool {
        let mut ty_kind = |ty: InternedTyId| self.ty_kind(ty).cloned();
        args.iter()
            .any(|arg| contains_generic_param(*arg, &mut ty_kind, None))
    }
}
