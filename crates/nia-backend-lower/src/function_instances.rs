// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet, VecDeque};

use crate::ModuleLowerer;
use nia_backend_ir::{BackendFunction, BackendFunctionInstance, BackendTraitObjectVtableFunction};
use nia_defs::DefKind;
use nia_function_ir::{
    FunctionBlock, FunctionBody, FunctionCallee, FunctionDeferBody, FunctionExpr, FunctionExprKind,
    FunctionForHeader, FunctionOp, FunctionTerminator,
};
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_ty::TyKind;

type InstanceKey = (GlobalDefId, ModuleId, Vec<InternedTyId>);
type InstanceQueueEntry = (InstanceKey, String);

struct InstanceWorkQueue {
    entries: VecDeque<InstanceQueueEntry>,
    queued: HashSet<InstanceKey>,
}

impl InstanceWorkQueue {
    fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            queued: HashSet::new(),
        }
    }

    fn contains(&self, key: &InstanceKey) -> bool {
        self.queued.contains(key)
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn push(
        &mut self,
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        args: Vec<InternedTyId>,
        symbol: String,
    ) {
        let key = (def_id, arg_module_id, args);
        if self.queued.insert(key.clone()) {
            self.entries.push_back((key, symbol));
        }
    }

    fn pop_front(&mut self) -> Option<(GlobalDefId, ModuleId, Vec<InternedTyId>, String)> {
        let (key, symbol) = self.entries.pop_front()?;
        self.queued.remove(&key);
        Some((key.0, key.1, key.2, symbol))
    }
}

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn lower_function_instances(
        &mut self,
        functions: &[BackendFunction],
    ) -> Vec<BackendFunctionInstance> {
        let mut instances = Vec::new();
        let mut seen = HashSet::<InstanceKey>::new();
        let mut queue = InstanceWorkQueue::new();
        let functions_by_def = functions
            .iter()
            .map(|function| (function.def_id, function))
            .collect::<HashMap<_, _>>();
        for instance in self
            .monomorphization
            .instances
            .iter()
            .filter(|instance| instance.def_id.module_id == self.input.module_id)
        {
            queue.push(
                instance.def_id,
                instance.arg_module_id,
                instance.args.clone(),
                instance.symbol.clone(),
            );
        }
        self.enqueue_function_instances_from_functions(functions, &mut seen, &mut queue);

        self.drain_function_instance_queue(
            &functions_by_def,
            &mut seen,
            &mut queue,
            &mut instances,
        );
        let mut next_vtable_scan_start = 0;
        loop {
            let mut vtables = Vec::new();
            if next_vtable_scan_start == 0 {
                self.collect_trait_object_vtables_from_functions(&mut vtables, functions);
            }
            self.collect_trait_object_vtables_from_function_instances(
                &mut vtables,
                &instances[next_vtable_scan_start..],
            );
            next_vtable_scan_start = instances.len();
            for vtable in &vtables {
                for entry in &vtable.entries {
                    if let BackendTraitObjectVtableFunction::FunctionInstance { def_id, args } =
                        &entry.function
                    {
                        self.enqueue_function_instance(*def_id, args, &seen, &mut queue);
                    }
                }
            }
            if queue.is_empty() {
                break;
            }
            let drained = self.drain_function_instance_queue(
                &functions_by_def,
                &mut seen,
                &mut queue,
                &mut instances,
            );
            if drained == 0 {
                break;
            }
        }
        instances
    }

    fn drain_function_instance_queue(
        &mut self,
        functions_by_def: &HashMap<GlobalDefId, &BackendFunction>,
        seen: &mut HashSet<InstanceKey>,
        queue: &mut InstanceWorkQueue,
        instances: &mut Vec<BackendFunctionInstance>,
    ) -> usize {
        let start_len = instances.len();
        while let Some((def_id, arg_module_id, args, symbol)) = queue.pop_front() {
            if !seen.insert((def_id, arg_module_id, args.clone())) {
                continue;
            }
            let Some(base) = functions_by_def.get(&def_id).copied() else {
                continue;
            };
            let substitutions = self.effective_generic_substitutions(base.def_id, &args);
            let function_body = base.function_body.clone().map(|body| {
                self.instantiate_function_body(def_id, args.len(), body, &substitutions)
            });
            if let Some(body) = &function_body {
                self.enqueue_function_instances_from_body(body, seen, queue);
            }
            instances.push(BackendFunctionInstance {
                def_id,
                name: base.name.clone(),
                arg_module_id,
                args,
                symbol,
                params: self.instantiate_params(base, &substitutions),
                return_type: self.instantiate_ty(base.return_type, &substitutions),
                is_extern: base.is_extern,
                is_variadic: base.is_variadic,
                function_body,
                span: base.span,
            });
        }
        instances.len() - start_len
    }

    fn enqueue_function_instances_from_functions(
        &mut self,
        functions: &[BackendFunction],
        seen: &mut HashSet<InstanceKey>,
        queue: &mut InstanceWorkQueue,
    ) {
        for function in functions {
            if !self
                .effective_generics(function.def_id, &function.generics)
                .is_empty()
            {
                continue;
            }
            if let Some(body) = &function.function_body {
                self.enqueue_function_instances_from_body(body, seen, queue);
            }
        }
    }

    fn enqueue_function_instances_from_body(
        &mut self,
        body: &FunctionBody,
        seen: &mut HashSet<InstanceKey>,
        queue: &mut InstanceWorkQueue,
    ) {
        self.enqueue_function_instances_from_blocks(&body.blocks, seen, queue);
    }

    fn enqueue_function_instances_from_defer_body(
        &mut self,
        body: &FunctionDeferBody,
        seen: &mut HashSet<InstanceKey>,
        queue: &mut InstanceWorkQueue,
    ) {
        self.enqueue_function_instances_from_blocks(&body.blocks, seen, queue);
    }

    fn enqueue_function_instances_from_blocks(
        &mut self,
        blocks: &[FunctionBlock],
        seen: &mut HashSet<InstanceKey>,
        queue: &mut InstanceWorkQueue,
    ) {
        for block in blocks {
            for op in &block.ops {
                self.enqueue_function_instances_from_op(op, seen, queue);
            }
            self.enqueue_function_instances_from_terminator(&block.terminator, seen, queue);
        }
    }

    fn enqueue_function_instances_from_terminator(
        &mut self,
        terminator: &FunctionTerminator,
        seen: &mut HashSet<InstanceKey>,
        queue: &mut InstanceWorkQueue,
    ) {
        match terminator {
            FunctionTerminator::If { cond, .. } => {
                self.enqueue_function_instances_from_expr(cond, seen, queue);
            }
            FunctionTerminator::Switch { target, arms, .. } => {
                self.enqueue_function_instances_from_expr(target, seen, queue);
                for arm in arms {
                    self.enqueue_function_instances_from_expr(&arm.pattern, seen, queue);
                }
            }
            FunctionTerminator::Try { value, .. } => {
                self.enqueue_function_instances_from_expr(value, seen, queue);
            }
            FunctionTerminator::Loop { header, .. } => match header {
                FunctionForHeader::Condition(expr) => {
                    self.enqueue_function_instances_from_expr(expr, seen, queue);
                }
                FunctionForHeader::Infinite => {}
            },
            FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
                if let Some(value) = value {
                    self.enqueue_function_instances_from_expr(value, seen, queue);
                }
            }
            FunctionTerminator::Branch { .. }
            | FunctionTerminator::Next { .. }
            | FunctionTerminator::Error { .. } => {}
        }
    }

    fn enqueue_function_instances_from_op(
        &mut self,
        op: &FunctionOp,
        seen: &mut HashSet<InstanceKey>,
        queue: &mut InstanceWorkQueue,
    ) {
        match op {
            FunctionOp::Binding(binding) => {
                if let Some(value) = &binding.value {
                    self.enqueue_function_instances_from_expr(value, seen, queue);
                }
            }
            FunctionOp::StoreLocal { value, .. } | FunctionOp::Expr(value) => {
                self.enqueue_function_instances_from_expr(value, seen, queue);
            }
            FunctionOp::Defer(body) => {
                self.enqueue_function_instances_from_defer_body(body, seen, queue);
            }
        }
    }

    fn enqueue_function_instances_from_expr(
        &mut self,
        expr: &FunctionExpr,
        seen: &mut HashSet<InstanceKey>,
        queue: &mut InstanceWorkQueue,
    ) {
        match &expr.kind {
            FunctionExprKind::FunctionInstance { def_id, args } => {
                self.enqueue_function_instance(*def_id, args, seen, queue);
            }
            FunctionExprKind::Discard(inner)
            | FunctionExprKind::RangeBound { range: inner, .. }
            | FunctionExprKind::Cast { expr: inner, .. }
            | FunctionExprKind::OptionalSome { expr: inner }
            | FunctionExprKind::ErrorOk { expr: inner }
            | FunctionExprKind::ErrorErr { expr: inner }
            | FunctionExprKind::TaggedUnionTag { expr: inner }
            | FunctionExprKind::TaggedUnionPayload { expr: inner }
            | FunctionExprKind::Try { expr: inner }
            | FunctionExprKind::TraitObjectUpcast { expr: inner, .. }
            | FunctionExprKind::TraitObjectCoercion { expr: inner, .. } => {
                self.enqueue_function_instances_from_expr(inner, seen, queue);
            }
            FunctionExprKind::Range(range) => {
                if let Some(start) = &range.start {
                    self.enqueue_function_instances_from_expr(start, seen, queue);
                }
                if let Some(end) = &range.end {
                    self.enqueue_function_instances_from_expr(end, seen, queue);
                }
            }
            FunctionExprKind::InlineAsm(asm) => {
                for input in &asm.inputs {
                    self.enqueue_function_instances_from_expr(&input.value, seen, queue);
                }
            }
            FunctionExprKind::CStringPointer { array, .. } => {
                self.enqueue_function_instances_from_expr(array, seen, queue);
            }
            FunctionExprKind::ArrayLiteral { elems } => match elems {
                nia_function_ir::FunctionArrayElements::List(elems) => {
                    for elem in elems {
                        self.enqueue_function_instances_from_expr(elem, seen, queue);
                    }
                }
                nia_function_ir::FunctionArrayElements::Repeat { value, .. } => {
                    self.enqueue_function_instances_from_expr(value, seen, queue);
                }
            },
            FunctionExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    self.enqueue_function_instances_from_expr(&field.value, seen, queue);
                }
            }
            FunctionExprKind::UnionLiteral { field, .. } => {
                self.enqueue_function_instances_from_expr(&field.value, seen, queue);
            }
            FunctionExprKind::Unary { expr: inner, .. } => {
                self.enqueue_function_instances_from_expr(inner, seen, queue);
            }
            FunctionExprKind::AddrOf(place) => {
                self.enqueue_function_instances_from_place(place, seen, queue);
            }
            FunctionExprKind::Binary { lhs, rhs, .. } => {
                self.enqueue_function_instances_from_expr(lhs, seen, queue);
                self.enqueue_function_instances_from_expr(rhs, seen, queue);
            }
            FunctionExprKind::Assign { place, rhs, .. } => {
                self.enqueue_function_instances_from_place(place, seen, queue);
                self.enqueue_function_instances_from_expr(rhs, seen, queue);
            }
            FunctionExprKind::Call { callee, args } => {
                self.enqueue_function_instances_from_callee(callee, seen, queue);
                for arg in args {
                    self.enqueue_function_instances_from_expr(arg, seen, queue);
                }
            }
            FunctionExprKind::Field { lhs, .. } => {
                self.enqueue_function_instances_from_expr(lhs, seen, queue);
            }
            FunctionExprKind::Index { lhs, index } => {
                self.enqueue_function_instances_from_expr(lhs, seen, queue);
                self.enqueue_function_instances_from_expr(index, seen, queue);
            }
            FunctionExprKind::Slice { lhs, range, .. } => {
                self.enqueue_function_instances_from_expr(lhs, seen, queue);
                if let Some(start) = &range.start {
                    self.enqueue_function_instances_from_expr(start, seen, queue);
                }
                if let Some(end) = &range.end {
                    self.enqueue_function_instances_from_expr(end, seen, queue);
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
            | FunctionExprKind::Null
            | FunctionExprKind::Local(_)
            | FunctionExprKind::Global(_)
            | FunctionExprKind::Function(_)
            | FunctionExprKind::EnumVariant(_)
            | FunctionExprKind::BuiltinValue(_) => {}
        }
    }

    fn enqueue_function_instances_from_callee(
        &mut self,
        callee: &FunctionCallee,
        seen: &mut HashSet<InstanceKey>,
        queue: &mut InstanceWorkQueue,
    ) {
        match callee {
            FunctionCallee::FunctionInstance { def_id, args }
            | FunctionCallee::Method { def_id, args, .. } => {
                self.enqueue_function_instance(*def_id, args, seen, queue);
            }
            FunctionCallee::TraitMethod {
                method_id,
                args,
                self_ty,
                trait_args,
                ..
            } => {
                let mut instance_args = vec![*self_ty];
                instance_args.extend(trait_args.iter().copied());
                instance_args.extend(args.iter().copied());
                self.enqueue_function_instance(*method_id, &instance_args, seen, queue);
            }
            FunctionCallee::BuiltinPlaceMethod { receiver, .. }
            | FunctionCallee::BuiltinMethod { receiver, .. } => {
                self.enqueue_function_instances_from_expr(receiver, seen, queue);
            }
            FunctionCallee::DynamicTraitMethod { receiver, .. } => {
                self.enqueue_function_instances_from_expr(receiver, seen, queue);
            }
            FunctionCallee::Function(_)
            | FunctionCallee::BuiltinOperator(_)
            | FunctionCallee::FunctionPointer(_) => {}
        }
    }

    fn enqueue_function_instances_from_place(
        &mut self,
        place: &nia_function_ir::FunctionPlace,
        seen: &mut HashSet<InstanceKey>,
        queue: &mut InstanceWorkQueue,
    ) {
        if let nia_function_ir::FunctionPlaceBase::Deref(expr) = &place.base {
            self.enqueue_function_instances_from_expr(expr, seen, queue);
        }
        for elem in &place.elems {
            if let nia_function_ir::FunctionPlaceElem::Index(expr) = elem {
                self.enqueue_function_instances_from_expr(expr, seen, queue);
            }
        }
    }

    fn enqueue_function_instance(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        seen: &HashSet<InstanceKey>,
        queue: &mut InstanceWorkQueue,
    ) {
        if args.is_empty() || def_id.module_id != self.input.module_id {
            return;
        }
        if args
            .iter()
            .any(|arg| self.cached_ty_contains_generic_param(*arg))
        {
            return;
        }
        let arg_module_id = args
            .first()
            .map(|arg| arg.interner_id)
            .unwrap_or(self.input.module_id);
        let key = (def_id, arg_module_id, args.to_vec());
        if seen.contains(&key) || queue.contains(&key) {
            return;
        }
        let Some(def) = self.input.defs.defs.get(def_id.def_id) else {
            return;
        };
        if !matches!(
            def.kind,
            DefKind::Function | DefKind::Method | DefKind::TraitMethod
        ) {
            return;
        }
        let name = def.name.clone();
        let symbol = self.mangle_instance_symbol(def_id, &name, args);
        queue.push(def_id, arg_module_id, key.2, symbol);
    }

    fn cached_ty_contains_generic_param(&mut self, ty: InternedTyId) -> bool {
        let body_interner = &self.input.body_check.ir.interner;
        let extension_interner = self.input.extension_interner;
        contains_generic_param(
            ty,
            &mut |ty| {
                if ty.interner_id == body_interner.interner_id() {
                    return body_interner.get(ty).cloned();
                }
                if let Some(extension_interner) = extension_interner
                    && ty.interner_id == extension_interner.interner_id()
                {
                    return extension_interner.get(ty).cloned();
                }
                None
            },
            Some(&mut self.generic_param_presence),
        )
    }
}

pub(crate) fn contains_generic_param(
    ty: InternedTyId,
    ty_kind: &mut impl FnMut(InternedTyId) -> Option<TyKind>,
    mut cache: Option<&mut HashMap<InternedTyId, bool>>,
) -> bool {
    if let Some(cache) = cache.as_deref()
        && let Some(cached) = cache.get(&ty)
    {
        return *cached;
    }
    let contains = match ty_kind(ty) {
        Some(TyKind::GenericParam(_)) => true,
        Some(TyKind::Pointer { elem, .. } | TyKind::Slice { elem, .. }) => {
            contains_generic_param(elem, ty_kind, cache.as_deref_mut())
        }
        Some(TyKind::Array { elem, .. }) => {
            contains_generic_param(elem, ty_kind, cache.as_deref_mut())
        }
        Some(TyKind::Range { bound, .. }) => {
            bound.is_some_and(|bound| contains_generic_param(bound, ty_kind, cache.as_deref_mut()))
        }
        Some(TyKind::FunctionPointer {
            params,
            return_type,
            ..
        }) => {
            params
                .iter()
                .any(|param| contains_generic_param(*param, ty_kind, cache.as_deref_mut()))
                || contains_generic_param(return_type, ty_kind, cache.as_deref_mut())
        }
        Some(TyKind::Optional { elem }) => {
            contains_generic_param(elem, ty_kind, cache.as_deref_mut())
        }
        Some(TyKind::ErrorUnion { error, value }) => {
            contains_generic_param(error, ty_kind, cache.as_deref_mut())
                || contains_generic_param(value, ty_kind, cache.as_deref_mut())
        }
        Some(TyKind::Nominal { args, .. } | TyKind::BuiltinTrait { args, .. }) => args
            .iter()
            .any(|arg| contains_generic_param(*arg, ty_kind, cache.as_deref_mut())),
        Some(TyKind::TraitObject {
            trait_args,
            associated_type_bindings,
            ..
        }) => {
            trait_args
                .iter()
                .any(|arg| contains_generic_param(*arg, ty_kind, cache.as_deref_mut()))
                || associated_type_bindings.iter().any(|binding| {
                    binding
                        .trait_args
                        .iter()
                        .any(|arg| contains_generic_param(*arg, ty_kind, cache.as_deref_mut()))
                        || contains_generic_param(binding.ty, ty_kind, cache.as_deref_mut())
                })
        }
        Some(TyKind::Projection {
            self_ty,
            trait_args,
            ..
        }) => {
            contains_generic_param(self_ty, ty_kind, cache.as_deref_mut())
                || trait_args
                    .iter()
                    .any(|arg| contains_generic_param(*arg, ty_kind, cache.as_deref_mut()))
        }
        Some(TyKind::Primitive(_) | TyKind::Error) | None => false,
    };
    if let Some(cache) = cache {
        cache.insert(ty, contains);
    }
    contains
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ids::{ModuleId, TyInternerIndex};
    use nia_ty::PrimitiveTy;

    #[test]
    fn generic_param_presence_cache_reuses_recursive_results() {
        let generic = test_ty(0);
        let pointer = test_ty(1);
        let mut calls = 0;
        let mut cache = HashMap::new();

        let first = contains_generic_param(
            pointer,
            &mut |ty| {
                calls += 1;
                match ty.index.index() {
                    0 => Some(TyKind::GenericParam("T".to_string())),
                    1 => Some(TyKind::Pointer {
                        is_const: true,
                        elem: generic,
                    }),
                    _ => None,
                }
            },
            Some(&mut cache),
        );
        let first_calls = calls;
        let second = contains_generic_param(
            pointer,
            &mut |_| {
                calls += 1;
                None
            },
            Some(&mut cache),
        );

        assert!(first);
        assert!(second);
        assert_eq!(first_calls, 2);
        assert_eq!(calls, first_calls);
    }

    #[test]
    fn generic_param_presence_cache_reuses_negative_results() {
        let int = test_ty(0);
        let slice = test_ty(1);
        let mut calls = 0;
        let mut cache = HashMap::new();

        let first = contains_generic_param(
            slice,
            &mut |ty| {
                calls += 1;
                match ty.index.index() {
                    0 => Some(TyKind::Primitive(PrimitiveTy::I32)),
                    1 => Some(TyKind::Slice {
                        is_const: false,
                        elem: int,
                    }),
                    _ => None,
                }
            },
            Some(&mut cache),
        );
        let first_calls = calls;
        let second = contains_generic_param(
            slice,
            &mut |_| {
                calls += 1;
                None
            },
            Some(&mut cache),
        );

        assert!(!first);
        assert!(!second);
        assert_eq!(first_calls, 2);
        assert_eq!(calls, first_calls);
    }

    fn test_ty(index: u32) -> InternedTyId {
        InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(index))
    }
}
