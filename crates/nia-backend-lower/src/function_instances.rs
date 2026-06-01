// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet, VecDeque};

use crate::ModuleLowerer;
use nia_backend_ir::{BackendFunction, BackendFunctionInstance, BackendTraitObjectVtableFunction};
use nia_defs::DefKind;
use nia_function_ir::{FunctionBody, FunctionCallee, FunctionExpr, FunctionExprKind, FunctionOp};
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
        loop {
            let mut vtables = Vec::new();
            self.collect_trait_object_vtables(&mut vtables, functions, &instances);
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
            self.drain_function_instance_queue(
                &functions_by_def,
                &mut seen,
                &mut queue,
                &mut instances,
            );
        }
        instances
    }

    fn drain_function_instance_queue(
        &mut self,
        functions_by_def: &HashMap<GlobalDefId, &BackendFunction>,
        seen: &mut HashSet<InstanceKey>,
        queue: &mut InstanceWorkQueue,
        instances: &mut Vec<BackendFunctionInstance>,
    ) {
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
        for block in &body.blocks {
            for op in &block.ops {
                self.enqueue_function_instances_from_op(op, seen, queue);
            }
            match &block.terminator {
                nia_function_ir::FunctionTerminator::If { cond, .. } => {
                    self.enqueue_function_instances_from_expr(cond, seen, queue);
                }
                nia_function_ir::FunctionTerminator::Switch { target, arms, .. } => {
                    self.enqueue_function_instances_from_expr(target, seen, queue);
                    for arm in arms {
                        self.enqueue_function_instances_from_expr(&arm.pattern, seen, queue);
                    }
                }
                nia_function_ir::FunctionTerminator::Loop { header, .. } => match header {
                    nia_function_ir::FunctionForHeader::Condition(expr) => {
                        self.enqueue_function_instances_from_expr(expr, seen, queue);
                    }
                    nia_function_ir::FunctionForHeader::CStyle { cond } => {
                        if let Some(cond) = cond {
                            self.enqueue_function_instances_from_expr(cond, seen, queue);
                        }
                    }
                    nia_function_ir::FunctionForHeader::Infinite => {}
                },
                nia_function_ir::FunctionTerminator::Return { value, .. }
                | nia_function_ir::FunctionTerminator::Tail { value, .. } => {
                    if let Some(value) = value {
                        self.enqueue_function_instances_from_expr(value, seen, queue);
                    }
                }
                nia_function_ir::FunctionTerminator::Branch { .. }
                | nia_function_ir::FunctionTerminator::Next { .. }
                | nia_function_ir::FunctionTerminator::Error { .. } => {}
            }
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
                for block in &body.blocks {
                    for op in &block.ops {
                        self.enqueue_function_instances_from_op(op, seen, queue);
                    }
                }
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
            | FunctionExprKind::Cast { expr: inner, .. }
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
            FunctionCallee::BuiltinPlaceMethod { receiver, .. } => {
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
        if args.iter().any(|arg| self.ty_contains_generic_param(*arg)) {
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

    fn ty_contains_generic_param(&self, ty: InternedTyId) -> bool {
        match self.ty_kind(ty) {
            Some(TyKind::GenericParam(_)) => true,
            Some(TyKind::Pointer { elem, .. } | TyKind::Slice { elem, .. }) => {
                self.ty_contains_generic_param(*elem)
            }
            Some(TyKind::Array { elem, .. }) => self.ty_contains_generic_param(*elem),
            Some(TyKind::Range { bound, .. }) => {
                bound.is_some_and(|bound| self.ty_contains_generic_param(bound))
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                params
                    .iter()
                    .any(|param| self.ty_contains_generic_param(*param))
                    || self.ty_contains_generic_param(*return_type)
            }
            Some(TyKind::Nominal { args, .. } | TyKind::BuiltinTrait { args, .. }) => {
                args.iter().any(|arg| self.ty_contains_generic_param(*arg))
            }
            Some(TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            }) => {
                trait_args
                    .iter()
                    .any(|arg| self.ty_contains_generic_param(*arg))
                    || associated_type_bindings
                        .iter()
                        .any(|(_, ty)| self.ty_contains_generic_param(*ty))
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.ty_contains_generic_param(*self_ty)
                    || trait_args
                        .iter()
                        .any(|arg| self.ty_contains_generic_param(*arg))
            }
            Some(TyKind::Primitive(_) | TyKind::Error) | None => false,
        }
    }
}
