// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet, VecDeque};

use crate::{BackendOptimizationChange, ModuleLowerer};
use nia_ast::Visibility;
use nia_backend_ir::{
    BackendFunction, BackendFunctionInstance, BackendGlobal, BackendTraitObjectVtable,
    BackendTraitObjectVtableFunction,
};
use nia_defs::DefKind;
use nia_function_ir::{
    FunctionArrayElements, FunctionBlock, FunctionBody, FunctionCallee, FunctionDeferBody,
    FunctionExpr, FunctionExprKind, FunctionForHeader, FunctionInlineAsm, FunctionOp,
    FunctionPlace, FunctionPlaceBase, FunctionPlaceElem, FunctionTerminator,
};
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_opt::OptimizationDepth;
use nia_static_ir::StaticInit;

pub(crate) const REMOVE_UNUSED_FUNCTIONS_PASS: &str = "remove-unused-functions";
pub(crate) const REMOVE_UNUSED_FUNCTION_INSTANCES_PASS: &str = "remove-unused-function-instances";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FunctionInstanceRef {
    def_id: GlobalDefId,
    arg_module_id: ModuleId,
    args: Vec<InternedTyId>,
}

#[derive(Debug, Default)]
struct FunctionRefs {
    functions: HashSet<GlobalDefId>,
    instances: HashSet<FunctionInstanceRef>,
}

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn remove_unused_private_functions(
        &mut self,
        functions: &mut Vec<BackendFunction>,
        function_instances: &mut Vec<BackendFunctionInstance>,
        globals: &[BackendGlobal],
        trait_object_vtables: &[BackendTraitObjectVtable],
    ) {
        if !self
            .optimization
            .dead_code_elim
            .at_least(OptimizationDepth::Full)
        {
            return;
        }

        let removable_functions = functions
            .iter()
            .filter(|function| self.is_removable_private_function(function))
            .map(|function| function.def_id)
            .collect::<HashSet<_>>();
        let removable_instances = function_instances
            .iter()
            .filter(|instance| self.is_removable_private_function_instance(instance))
            .map(FunctionInstanceRef::from)
            .collect::<HashSet<_>>();
        if removable_functions.is_empty() && removable_instances.is_empty() {
            return;
        }

        let mut refs = FunctionRefs::default();
        for function in functions.iter() {
            if !removable_functions.contains(&function.def_id) {
                collect_function_refs_from_optional_body(
                    self.input.module_id,
                    &function.function_body,
                    &mut refs,
                );
            }
        }
        for instance in function_instances.iter() {
            if !removable_instances.contains(&FunctionInstanceRef::from(instance)) {
                collect_function_refs_from_optional_body(
                    self.input.module_id,
                    &instance.function_body,
                    &mut refs,
                );
            }
        }
        for global in globals {
            if let Some(init) = &global.init {
                collect_function_refs_from_static_init(self.input.module_id, init, &mut refs);
            }
        }
        for vtable in trait_object_vtables {
            for entry in &vtable.entries {
                match &entry.function {
                    BackendTraitObjectVtableFunction::Function(function) => {
                        refs.functions.insert(*function);
                    }
                    BackendTraitObjectVtableFunction::FunctionInstance {
                        def_id,
                        arg_module_id,
                        args,
                    } => {
                        refs.instances.insert(FunctionInstanceRef {
                            def_id: *def_id,
                            arg_module_id: *arg_module_id,
                            args: args.clone(),
                        });
                    }
                }
            }
        }
        collect_transitive_refs(functions, function_instances, &mut refs);

        let mut removed_functions = Vec::new();
        functions.retain(|function| {
            let remove = removable_functions.contains(&function.def_id)
                && !refs.functions.contains(&function.def_id);
            if remove {
                removed_functions.push(function.def_id);
            }
            !remove
        });
        for function in removed_functions {
            self.optimization_report
                .changed_passes
                .push(BackendOptimizationChange::Function {
                    module_id: self.input.module_id,
                    function,
                    pass: REMOVE_UNUSED_FUNCTIONS_PASS,
                    is_instance: false,
                    type_arg_count: 0,
                });
        }

        let mut removed_instances = Vec::new();
        function_instances.retain(|instance| {
            let key = FunctionInstanceRef::from(instance);
            let remove = removable_instances.contains(&key) && !refs.instances.contains(&key);
            if remove {
                removed_instances.push((instance.def_id, instance.args.len()));
            }
            !remove
        });
        for (function, type_arg_count) in removed_instances {
            self.optimization_report
                .changed_passes
                .push(BackendOptimizationChange::Function {
                    module_id: self.input.module_id,
                    function,
                    pass: REMOVE_UNUSED_FUNCTION_INSTANCES_PASS,
                    is_instance: true,
                    type_arg_count,
                });
        }
    }

    fn is_removable_private_function(&self, function: &BackendFunction) -> bool {
        if function.is_extern
            || function.name == "main"
            || function.def_id.module_id != self.input.module_id
        {
            return false;
        }
        let Some(def) = self.input.defs.defs.get(function.def_id.def_id) else {
            return false;
        };
        matches!(def.kind, DefKind::Function) && def.visibility != Visibility::Public
    }

    fn is_removable_private_function_instance(&self, instance: &BackendFunctionInstance) -> bool {
        if instance.is_extern || instance.def_id.module_id != self.input.module_id {
            return false;
        }
        let Some(def) = self.input.defs.defs.get(instance.def_id.def_id) else {
            return false;
        };
        matches!(def.kind, DefKind::Function) && def.visibility != Visibility::Public
    }
}

impl From<&BackendFunctionInstance> for FunctionInstanceRef {
    fn from(instance: &BackendFunctionInstance) -> Self {
        Self {
            def_id: instance.def_id,
            arg_module_id: instance.arg_module_id,
            args: instance.args.clone(),
        }
    }
}

fn collect_transitive_refs(
    functions: &[BackendFunction],
    instances: &[BackendFunctionInstance],
    refs: &mut FunctionRefs,
) {
    let functions_by_id = functions
        .iter()
        .map(|function| (function.def_id, function))
        .collect::<HashMap<_, _>>();
    let instances_by_ref = instances
        .iter()
        .map(|instance| (FunctionInstanceRef::from(instance), instance))
        .collect::<HashMap<_, _>>();
    let mut visited_functions = HashSet::new();
    let mut visited_instances = HashSet::new();
    let mut pending_functions = refs.functions.iter().copied().collect::<VecDeque<_>>();
    let mut pending_instances = refs.instances.iter().cloned().collect::<VecDeque<_>>();

    while !pending_functions.is_empty() || !pending_instances.is_empty() {
        while let Some(function_id) = pending_functions.pop_front() {
            if !visited_functions.insert(function_id) {
                continue;
            }
            let Some(function) = functions_by_id.get(&function_id) else {
                continue;
            };
            let mut discovered = FunctionRefs::default();
            collect_function_refs_from_optional_body(
                function.def_id.module_id,
                &function.function_body,
                &mut discovered,
            );
            enqueue_new_refs(
                refs,
                discovered,
                &mut pending_functions,
                &mut pending_instances,
            );
        }

        while let Some(instance_ref) = pending_instances.pop_front() {
            if !visited_instances.insert(instance_ref.clone()) {
                continue;
            }
            let Some(instance) = instances_by_ref.get(&instance_ref) else {
                continue;
            };
            let mut discovered = FunctionRefs::default();
            collect_function_refs_from_optional_body(
                instance.arg_module_id,
                &instance.function_body,
                &mut discovered,
            );
            enqueue_new_refs(
                refs,
                discovered,
                &mut pending_functions,
                &mut pending_instances,
            );
        }
    }
}

fn enqueue_new_refs(
    refs: &mut FunctionRefs,
    discovered: FunctionRefs,
    pending_functions: &mut VecDeque<GlobalDefId>,
    pending_instances: &mut VecDeque<FunctionInstanceRef>,
) {
    for function in discovered.functions {
        if refs.functions.insert(function) {
            pending_functions.push_back(function);
        }
    }
    for instance in discovered.instances {
        if refs.instances.insert(instance.clone()) {
            pending_instances.push_back(instance);
        }
    }
}

fn collect_function_refs_from_optional_body(
    module_id: ModuleId,
    body: &Option<FunctionBody>,
    refs: &mut FunctionRefs,
) {
    if let Some(body) = body {
        collect_function_refs_from_body(module_id, body, refs);
    }
}

fn collect_function_refs_from_body(
    module_id: ModuleId,
    body: &FunctionBody,
    refs: &mut FunctionRefs,
) {
    for block in &body.blocks {
        collect_function_refs_from_block(module_id, block, refs);
    }
}

fn collect_function_refs_from_defer_body(
    module_id: ModuleId,
    body: &FunctionDeferBody,
    refs: &mut FunctionRefs,
) {
    for block in &body.blocks {
        collect_function_refs_from_block(module_id, block, refs);
    }
}

fn collect_function_refs_from_block(
    module_id: ModuleId,
    block: &FunctionBlock,
    refs: &mut FunctionRefs,
) {
    for op in &block.ops {
        collect_function_refs_from_op(module_id, op, refs);
    }
    collect_function_refs_from_terminator(module_id, &block.terminator, refs);
}

fn collect_function_refs_from_op(module_id: ModuleId, op: &FunctionOp, refs: &mut FunctionRefs) {
    match op {
        FunctionOp::Binding(binding) => {
            if let Some(value) = &binding.value {
                collect_function_refs_from_expr(module_id, value, refs);
            }
        }
        FunctionOp::StoreLocal { value, .. } | FunctionOp::Expr(value) => {
            collect_function_refs_from_expr(module_id, value, refs);
        }
        FunctionOp::Defer(body) => collect_function_refs_from_defer_body(module_id, body, refs),
    }
}

fn collect_function_refs_from_terminator(
    module_id: ModuleId,
    terminator: &FunctionTerminator,
    refs: &mut FunctionRefs,
) {
    match terminator {
        FunctionTerminator::If { cond, .. } => {
            collect_function_refs_from_expr(module_id, cond, refs)
        }
        FunctionTerminator::Switch { target, arms, .. } => {
            collect_function_refs_from_expr(module_id, target, refs);
            for arm in arms {
                collect_function_refs_from_expr(module_id, &arm.pattern, refs);
            }
        }
        FunctionTerminator::Try { value, .. } => {
            collect_function_refs_from_expr(module_id, value, refs)
        }
        FunctionTerminator::Loop { header, .. } => match header {
            FunctionForHeader::Condition(expr) => {
                collect_function_refs_from_expr(module_id, expr, refs)
            }
            FunctionForHeader::Infinite => {}
        },
        FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
            if let Some(value) = value {
                collect_function_refs_from_expr(module_id, value, refs);
            }
        }
        FunctionTerminator::Error { .. }
        | FunctionTerminator::Branch { .. }
        | FunctionTerminator::Next { .. } => {}
    }
}

fn collect_function_refs_from_expr(
    module_id: ModuleId,
    expr: &FunctionExpr,
    refs: &mut FunctionRefs,
) {
    match &expr.kind {
        FunctionExprKind::Function(def_id) => {
            refs.functions.insert(*def_id);
        }
        FunctionExprKind::FunctionInstance { def_id, args } => {
            refs.instances.insert(FunctionInstanceRef {
                def_id: *def_id,
                arg_module_id: module_id,
                args: args.clone(),
            });
        }
        FunctionExprKind::Range(range) => {
            if let Some(start) = &range.start {
                collect_function_refs_from_expr(module_id, start, refs);
            }
            if let Some(end) = &range.end {
                collect_function_refs_from_expr(module_id, end, refs);
            }
        }
        FunctionExprKind::InlineAsm(asm) => {
            collect_function_refs_from_inline_asm(module_id, asm, refs)
        }
        FunctionExprKind::CStringPointer { array, .. }
        | FunctionExprKind::RangeBound { range: array, .. }
        | FunctionExprKind::Unary { expr: array, .. }
        | FunctionExprKind::OptionalSome { expr: array }
        | FunctionExprKind::ErrorOk { expr: array }
        | FunctionExprKind::ErrorErr { expr: array }
        | FunctionExprKind::TaggedUnionTag { expr: array }
        | FunctionExprKind::TaggedUnionPayload { expr: array }
        | FunctionExprKind::Try { expr: array }
        | FunctionExprKind::Discard(array)
        | FunctionExprKind::Cast { expr: array, .. }
        | FunctionExprKind::TraitObjectUpcast { expr: array, .. }
        | FunctionExprKind::TraitObjectCoercion { expr: array, .. } => {
            collect_function_refs_from_expr(module_id, array, refs);
        }
        FunctionExprKind::ArrayLiteral { elems } => match elems {
            FunctionArrayElements::List(elems) => {
                for elem in elems {
                    collect_function_refs_from_expr(module_id, elem, refs);
                }
            }
            FunctionArrayElements::Repeat { value, .. } => {
                collect_function_refs_from_expr(module_id, value, refs)
            }
        },
        FunctionExprKind::StructLiteral { fields, .. } => {
            for field in fields {
                collect_function_refs_from_expr(module_id, &field.value, refs);
            }
        }
        FunctionExprKind::UnionLiteral { field, .. } => {
            collect_function_refs_from_expr(module_id, &field.value, refs);
        }
        FunctionExprKind::AddrOf(place) => collect_function_refs_from_place(module_id, place, refs),
        FunctionExprKind::Binary { lhs, rhs, .. } => {
            collect_function_refs_from_expr(module_id, lhs, refs);
            collect_function_refs_from_expr(module_id, rhs, refs);
        }
        FunctionExprKind::Assign { place, rhs, .. } => {
            collect_function_refs_from_place(module_id, place, refs);
            collect_function_refs_from_expr(module_id, rhs, refs);
        }
        FunctionExprKind::Call { callee, args } => {
            collect_function_refs_from_callee(module_id, callee, refs);
            for arg in args {
                collect_function_refs_from_expr(module_id, arg, refs);
            }
        }
        FunctionExprKind::Field { lhs, .. } => {
            collect_function_refs_from_expr(module_id, lhs, refs)
        }
        FunctionExprKind::Index { lhs, index } => {
            collect_function_refs_from_expr(module_id, lhs, refs);
            collect_function_refs_from_expr(module_id, index, refs);
        }
        FunctionExprKind::Slice { lhs, range, .. } => {
            collect_function_refs_from_expr(module_id, lhs, refs);
            if let Some(start) = &range.start {
                collect_function_refs_from_expr(module_id, start, refs);
            }
            if let Some(end) = &range.end {
                collect_function_refs_from_expr(module_id, end, refs);
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
        | FunctionExprKind::EnumVariant(_)
        | FunctionExprKind::BuiltinValue(_) => {}
    }
}

fn collect_function_refs_from_callee(
    module_id: ModuleId,
    callee: &FunctionCallee,
    refs: &mut FunctionRefs,
) {
    match callee {
        FunctionCallee::Function(def_id) => {
            refs.functions.insert(*def_id);
        }
        FunctionCallee::FunctionInstance { def_id, args }
        | FunctionCallee::Method { def_id, args, .. } => {
            refs.instances.insert(FunctionInstanceRef {
                def_id: *def_id,
                arg_module_id: module_id,
                args: args.clone(),
            });
        }
        FunctionCallee::TraitMethod {
            method_id,
            receiver,
            self_ty,
            trait_args,
            args,
            ..
        } => {
            let mut instance_args = vec![*self_ty];
            instance_args.extend(trait_args.iter().copied());
            instance_args.extend(args.iter().copied());
            refs.instances.insert(FunctionInstanceRef {
                def_id: *method_id,
                arg_module_id: module_id,
                args: instance_args,
            });
            collect_function_refs_from_expr(module_id, receiver, refs);
        }
        FunctionCallee::DynamicTraitMethod { receiver, .. }
        | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
        | FunctionCallee::BuiltinMethod { receiver, .. }
        | FunctionCallee::FunctionPointer(receiver) => {
            collect_function_refs_from_expr(module_id, receiver, refs);
        }
        FunctionCallee::BuiltinOperator(_) => {}
    }
}

fn collect_function_refs_from_place(
    module_id: ModuleId,
    place: &FunctionPlace,
    refs: &mut FunctionRefs,
) {
    match &place.base {
        FunctionPlaceBase::Deref(expr) => collect_function_refs_from_expr(module_id, expr, refs),
        FunctionPlaceBase::Local(_) | FunctionPlaceBase::Global(_) | FunctionPlaceBase::Error => {}
    }
    for elem in &place.elems {
        match elem {
            FunctionPlaceElem::Index(expr) => {
                collect_function_refs_from_expr(module_id, expr, refs)
            }
            FunctionPlaceElem::Field(_) | FunctionPlaceElem::Error => {}
        }
    }
}

fn collect_function_refs_from_inline_asm(
    module_id: ModuleId,
    asm: &FunctionInlineAsm,
    refs: &mut FunctionRefs,
) {
    for input in &asm.inputs {
        collect_function_refs_from_expr(module_id, &input.value, refs);
    }
    for output in &asm.outputs {
        collect_function_refs_from_place(module_id, &output.place, refs);
    }
}

fn collect_function_refs_from_static_init(
    module_id: ModuleId,
    init: &StaticInit,
    refs: &mut FunctionRefs,
) {
    match init {
        StaticInit::Array(elems) => {
            for elem in elems {
                collect_function_refs_from_static_init(module_id, elem, refs);
            }
        }
        StaticInit::Repeat { value, count } => {
            if *count != 0 {
                collect_function_refs_from_static_init(module_id, value, refs);
            }
        }
        StaticInit::Struct(fields) => {
            for field in fields {
                collect_function_refs_from_static_init(module_id, &field.value, refs);
            }
        }
        StaticInit::AddrOfGlobal { .. } => {}
        StaticInit::AddrOfFunction { function, args } => {
            if args.is_empty() {
                refs.functions.insert(*function);
            } else {
                refs.instances.insert(FunctionInstanceRef {
                    def_id: *function,
                    arg_module_id: module_id,
                    args: args.clone(),
                });
            }
        }
        StaticInit::Zero
        | StaticInit::Int(_)
        | StaticInit::Float(_)
        | StaticInit::Bool(_)
        | StaticInit::Char(_)
        | StaticInit::Byte(_)
        | StaticInit::Chars(_)
        | StaticInit::Bytes(_)
        | StaticInit::NullPtr => {}
    }
}
