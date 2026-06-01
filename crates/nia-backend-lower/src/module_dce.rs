// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

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
use nia_ids::GlobalDefId;
use nia_opt::OptimizationDepth;
use nia_static_ir::StaticInit;

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn remove_unused_private_functions(
        &mut self,
        functions: &mut Vec<BackendFunction>,
        function_instances: &[BackendFunctionInstance],
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

        let removable = functions
            .iter()
            .filter(|function| self.is_removable_private_function(function))
            .map(|function| function.def_id)
            .collect::<HashSet<_>>();
        if removable.is_empty() {
            return;
        }

        let mut referenced = HashSet::new();
        for function in functions.iter() {
            if !removable.contains(&function.def_id) {
                collect_function_refs_from_optional_body(&function.function_body, &mut referenced);
            }
        }
        for instance in function_instances {
            collect_function_refs_from_optional_body(&instance.function_body, &mut referenced);
        }
        for global in globals {
            if let Some(init) = &global.init {
                collect_function_refs_from_static_init(init, &mut referenced);
            }
        }
        for vtable in trait_object_vtables {
            for entry in &vtable.entries {
                match &entry.function {
                    BackendTraitObjectVtableFunction::Function(function) => {
                        referenced.insert(*function);
                    }
                    BackendTraitObjectVtableFunction::FunctionInstance { def_id, .. } => {
                        referenced.insert(*def_id);
                    }
                }
            }
        }

        let mut removed = Vec::new();
        functions.retain(|function| {
            let remove =
                removable.contains(&function.def_id) && !referenced.contains(&function.def_id);
            if remove {
                removed.push(function.def_id);
            }
            !remove
        });
        for function in removed {
            self.optimization_report
                .changed_passes
                .push(BackendOptimizationChange::Function {
                    module_id: self.input.module_id,
                    function,
                    pass: "remove-unused-functions",
                    is_instance: false,
                    type_arg_count: 0,
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
}

fn collect_function_refs_from_optional_body(
    body: &Option<FunctionBody>,
    refs: &mut HashSet<GlobalDefId>,
) {
    if let Some(body) = body {
        collect_function_refs_from_body(body, refs);
    }
}

fn collect_function_refs_from_body(body: &FunctionBody, refs: &mut HashSet<GlobalDefId>) {
    for block in &body.blocks {
        collect_function_refs_from_block(block, refs);
    }
}

fn collect_function_refs_from_defer_body(
    body: &FunctionDeferBody,
    refs: &mut HashSet<GlobalDefId>,
) {
    for block in &body.blocks {
        collect_function_refs_from_block(block, refs);
    }
}

fn collect_function_refs_from_block(block: &FunctionBlock, refs: &mut HashSet<GlobalDefId>) {
    for op in &block.ops {
        collect_function_refs_from_op(op, refs);
    }
    collect_function_refs_from_terminator(&block.terminator, refs);
}

fn collect_function_refs_from_op(op: &FunctionOp, refs: &mut HashSet<GlobalDefId>) {
    match op {
        FunctionOp::Binding(binding) => {
            if let Some(value) = &binding.value {
                collect_function_refs_from_expr(value, refs);
            }
        }
        FunctionOp::StoreLocal { value, .. } | FunctionOp::Expr(value) => {
            collect_function_refs_from_expr(value, refs);
        }
        FunctionOp::Defer(body) => collect_function_refs_from_defer_body(body, refs),
    }
}

fn collect_function_refs_from_terminator(
    terminator: &FunctionTerminator,
    refs: &mut HashSet<GlobalDefId>,
) {
    match terminator {
        FunctionTerminator::If { cond, .. } => collect_function_refs_from_expr(cond, refs),
        FunctionTerminator::Switch { target, arms, .. } => {
            collect_function_refs_from_expr(target, refs);
            for arm in arms {
                collect_function_refs_from_expr(&arm.pattern, refs);
            }
        }
        FunctionTerminator::Loop { header, .. } => match header {
            FunctionForHeader::Condition(expr) => collect_function_refs_from_expr(expr, refs),
            FunctionForHeader::CStyle { cond } => {
                if let Some(cond) = cond {
                    collect_function_refs_from_expr(cond, refs);
                }
            }
            FunctionForHeader::Infinite => {}
        },
        FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
            if let Some(value) = value {
                collect_function_refs_from_expr(value, refs);
            }
        }
        FunctionTerminator::Error { .. }
        | FunctionTerminator::Branch { .. }
        | FunctionTerminator::Next { .. } => {}
    }
}

fn collect_function_refs_from_expr(expr: &FunctionExpr, refs: &mut HashSet<GlobalDefId>) {
    match &expr.kind {
        FunctionExprKind::Function(def_id) | FunctionExprKind::FunctionInstance { def_id, .. } => {
            refs.insert(*def_id);
        }
        FunctionExprKind::Range(range) => {
            if let Some(start) = &range.start {
                collect_function_refs_from_expr(start, refs);
            }
            if let Some(end) = &range.end {
                collect_function_refs_from_expr(end, refs);
            }
        }
        FunctionExprKind::InlineAsm(asm) => collect_function_refs_from_inline_asm(asm, refs),
        FunctionExprKind::CStringPointer { array, .. }
        | FunctionExprKind::Unary { expr: array, .. }
        | FunctionExprKind::Discard(array)
        | FunctionExprKind::Cast { expr: array, .. }
        | FunctionExprKind::TraitObjectUpcast { expr: array, .. }
        | FunctionExprKind::TraitObjectCoercion { expr: array, .. } => {
            collect_function_refs_from_expr(array, refs);
        }
        FunctionExprKind::ArrayLiteral { elems } => match elems {
            FunctionArrayElements::List(elems) => {
                for elem in elems {
                    collect_function_refs_from_expr(elem, refs);
                }
            }
            FunctionArrayElements::Repeat { value, .. } => {
                collect_function_refs_from_expr(value, refs)
            }
        },
        FunctionExprKind::StructLiteral { fields, .. } => {
            for field in fields {
                collect_function_refs_from_expr(&field.value, refs);
            }
        }
        FunctionExprKind::UnionLiteral { field, .. } => {
            collect_function_refs_from_expr(&field.value, refs);
        }
        FunctionExprKind::AddrOf(place) => collect_function_refs_from_place(place, refs),
        FunctionExprKind::Binary { lhs, rhs, .. } => {
            collect_function_refs_from_expr(lhs, refs);
            collect_function_refs_from_expr(rhs, refs);
        }
        FunctionExprKind::Assign { place, rhs, .. } => {
            collect_function_refs_from_place(place, refs);
            collect_function_refs_from_expr(rhs, refs);
        }
        FunctionExprKind::Call { callee, args } => {
            collect_function_refs_from_callee(callee, refs);
            for arg in args {
                collect_function_refs_from_expr(arg, refs);
            }
        }
        FunctionExprKind::Field { lhs, .. } => collect_function_refs_from_expr(lhs, refs),
        FunctionExprKind::Index { lhs, index } => {
            collect_function_refs_from_expr(lhs, refs);
            collect_function_refs_from_expr(index, refs);
        }
        FunctionExprKind::Slice { lhs, range, .. } => {
            collect_function_refs_from_expr(lhs, refs);
            if let Some(start) = &range.start {
                collect_function_refs_from_expr(start, refs);
            }
            if let Some(end) = &range.end {
                collect_function_refs_from_expr(end, refs);
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
        | FunctionExprKind::EnumVariant(_)
        | FunctionExprKind::BuiltinValue(_) => {}
    }
}

fn collect_function_refs_from_callee(callee: &FunctionCallee, refs: &mut HashSet<GlobalDefId>) {
    match callee {
        FunctionCallee::Function(def_id)
        | FunctionCallee::FunctionInstance { def_id, .. }
        | FunctionCallee::Method { def_id, .. } => {
            refs.insert(*def_id);
        }
        FunctionCallee::TraitMethod {
            method_id,
            receiver,
            ..
        } => {
            refs.insert(*method_id);
            collect_function_refs_from_expr(receiver, refs);
        }
        FunctionCallee::DynamicTraitMethod { receiver, .. }
        | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
        | FunctionCallee::FunctionPointer(receiver) => {
            collect_function_refs_from_expr(receiver, refs);
        }
        FunctionCallee::BuiltinOperator(_) => {}
    }
}

fn collect_function_refs_from_place(place: &FunctionPlace, refs: &mut HashSet<GlobalDefId>) {
    match &place.base {
        FunctionPlaceBase::Deref(expr) => collect_function_refs_from_expr(expr, refs),
        FunctionPlaceBase::Local(_) | FunctionPlaceBase::Global(_) | FunctionPlaceBase::Error => {}
    }
    for elem in &place.elems {
        match elem {
            FunctionPlaceElem::Index(expr) => collect_function_refs_from_expr(expr, refs),
            FunctionPlaceElem::Field(_) | FunctionPlaceElem::Error => {}
        }
    }
}

fn collect_function_refs_from_inline_asm(asm: &FunctionInlineAsm, refs: &mut HashSet<GlobalDefId>) {
    for input in &asm.inputs {
        collect_function_refs_from_expr(&input.value, refs);
    }
    for output in &asm.outputs {
        collect_function_refs_from_place(&output.place, refs);
    }
}

fn collect_function_refs_from_static_init(init: &StaticInit, refs: &mut HashSet<GlobalDefId>) {
    match init {
        StaticInit::Array(elems) => {
            for elem in elems {
                collect_function_refs_from_static_init(elem, refs);
            }
        }
        StaticInit::Repeat { value, .. } => collect_function_refs_from_static_init(value, refs),
        StaticInit::Struct(fields) => {
            for field in fields {
                collect_function_refs_from_static_init(&field.value, refs);
            }
        }
        StaticInit::AddrOfGlobal { .. } => {}
        StaticInit::AddrOfFunction { function, .. } => {
            refs.insert(*function);
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
