// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{BackendOptimizationChange, ModuleLowerer};
use nia_backend_ir::{BackendFunction, BackendFunctionInstance};
use nia_function_ir::{
    FunctionArrayElements, FunctionBlock, FunctionBody, FunctionCallee, FunctionDeferBody,
    FunctionExpr, FunctionExprKind, FunctionForHeader, FunctionInlineAsm, FunctionOp,
    FunctionPlace, FunctionPlaceBase, FunctionPlaceElem, FunctionTerminator,
};
use nia_ids::{GlobalDefId, TraitId};
use nia_opt::NiaOptimizationLevel;

pub(crate) const DEVIRTUALIZE_DIRECT_TRAIT_CALLS_PASS: &str = "devirtualize-direct-trait-calls";

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn devirtualize_direct_trait_calls(
        &mut self,
        functions: &mut [BackendFunction],
        function_instances: &mut [BackendFunctionInstance],
    ) {
        if self.optimization.level != NiaOptimizationLevel::O3 {
            return;
        }

        for function in functions {
            if let Some(body) = &mut function.function_body {
                if self.devirtualize_direct_trait_calls_in_body(body) {
                    self.record_devirtualization(function.def_id, false, 0);
                }
            }
        }
        for instance in function_instances {
            if let Some(body) = &mut instance.function_body {
                if self.devirtualize_direct_trait_calls_in_body(body) {
                    self.record_devirtualization(instance.def_id, true, instance.args.len());
                }
            }
        }
    }

    fn devirtualize_direct_trait_calls_in_body(&mut self, body: &mut FunctionBody) -> bool {
        self.devirtualize_direct_trait_calls_in_blocks(&mut body.blocks)
    }

    fn devirtualize_direct_trait_calls_in_defer_body(
        &mut self,
        body: &mut FunctionDeferBody,
    ) -> bool {
        self.devirtualize_direct_trait_calls_in_blocks(&mut body.blocks)
    }

    fn devirtualize_direct_trait_calls_in_blocks(&mut self, blocks: &mut [FunctionBlock]) -> bool {
        let mut changed = false;
        for block in blocks {
            for op in &mut block.ops {
                changed |= self.devirtualize_direct_trait_calls_in_op(op);
            }
            changed |= self.devirtualize_direct_trait_calls_in_terminator(&mut block.terminator);
        }
        changed
    }

    fn devirtualize_direct_trait_calls_in_op(&mut self, op: &mut FunctionOp) -> bool {
        match op {
            FunctionOp::Binding(binding) => binding
                .value
                .as_mut()
                .is_some_and(|value| self.devirtualize_direct_trait_calls_in_expr(value)),
            FunctionOp::StoreLocal { value, .. } | FunctionOp::Expr(value) => {
                self.devirtualize_direct_trait_calls_in_expr(value)
            }
            FunctionOp::Defer(body) => self.devirtualize_direct_trait_calls_in_defer_body(body),
        }
    }

    fn devirtualize_direct_trait_calls_in_terminator(
        &mut self,
        terminator: &mut FunctionTerminator,
    ) -> bool {
        match terminator {
            FunctionTerminator::If { cond, .. } => {
                self.devirtualize_direct_trait_calls_in_expr(cond)
            }
            FunctionTerminator::Switch { target, arms, .. } => {
                let mut changed = self.devirtualize_direct_trait_calls_in_expr(target);
                for arm in arms {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(&mut arm.pattern);
                }
                changed
            }
            FunctionTerminator::Loop { header, .. } => match header {
                FunctionForHeader::Condition(cond) => {
                    self.devirtualize_direct_trait_calls_in_expr(cond)
                }
                FunctionForHeader::Infinite => false,
            },
            FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
                value
                    .as_mut()
                    .is_some_and(|value| self.devirtualize_direct_trait_calls_in_expr(value))
            }
            FunctionTerminator::Error { .. }
            | FunctionTerminator::Branch { .. }
            | FunctionTerminator::Next { .. } => false,
        }
    }

    fn devirtualize_direct_trait_calls_in_expr(&mut self, expr: &mut FunctionExpr) -> bool {
        let mut changed = false;
        match &mut expr.kind {
            FunctionExprKind::Call { callee, args } => {
                changed |= self.devirtualize_direct_trait_calls_in_callee(callee);
                for arg in args {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(arg);
                }
                if let Some((receiver, def_id)) = self.direct_trait_call_target(callee) {
                    *callee = FunctionCallee::Method {
                        def_id,
                        args: Vec::new(),
                        receiver,
                    };
                    changed = true;
                }
            }
            FunctionExprKind::Range(range) => {
                if let Some(start) = &mut range.start {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(start);
                }
                if let Some(end) = &mut range.end {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(end);
                }
            }
            FunctionExprKind::InlineAsm(asm) => {
                changed |= self.devirtualize_direct_trait_calls_in_inline_asm(asm);
            }
            FunctionExprKind::CStringPointer { array, .. }
            | FunctionExprKind::Unary { expr: array, .. }
            | FunctionExprKind::Discard(array)
            | FunctionExprKind::Cast { expr: array, .. }
            | FunctionExprKind::TraitObjectUpcast { expr: array, .. }
            | FunctionExprKind::TraitObjectCoercion { expr: array, .. } => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(array);
            }
            FunctionExprKind::ArrayLiteral { elems } => match elems {
                FunctionArrayElements::List(elems) => {
                    for elem in elems {
                        changed |= self.devirtualize_direct_trait_calls_in_expr(elem);
                    }
                }
                FunctionArrayElements::Repeat { value, .. } => {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(value);
                }
            },
            FunctionExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(&mut field.value);
                }
            }
            FunctionExprKind::UnionLiteral { field, .. } => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(&mut field.value);
            }
            FunctionExprKind::AddrOf(place) => {
                changed |= self.devirtualize_direct_trait_calls_in_place(place);
            }
            FunctionExprKind::Binary { lhs, rhs, .. } => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(lhs);
                changed |= self.devirtualize_direct_trait_calls_in_expr(rhs);
            }
            FunctionExprKind::Assign { place, rhs, .. } => {
                changed |= self.devirtualize_direct_trait_calls_in_place(place);
                changed |= self.devirtualize_direct_trait_calls_in_expr(rhs);
            }
            FunctionExprKind::Field { lhs, .. } => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(lhs);
            }
            FunctionExprKind::Index { lhs, index } => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(lhs);
                changed |= self.devirtualize_direct_trait_calls_in_expr(index);
            }
            FunctionExprKind::Slice { lhs, range, .. } => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(lhs);
                if let Some(start) = &mut range.start {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(start);
                }
                if let Some(end) = &mut range.end {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(end);
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
            | FunctionExprKind::FunctionInstance { .. }
            | FunctionExprKind::EnumVariant(_)
            | FunctionExprKind::BuiltinValue(_) => {}
        }
        changed
    }

    fn devirtualize_direct_trait_calls_in_callee(&mut self, callee: &mut FunctionCallee) -> bool {
        match callee {
            FunctionCallee::Method { receiver, .. }
            | FunctionCallee::TraitMethod { receiver, .. }
            | FunctionCallee::DynamicTraitMethod { receiver, .. }
            | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
            | FunctionCallee::FunctionPointer(receiver) => {
                self.devirtualize_direct_trait_calls_in_expr(receiver)
            }
            FunctionCallee::Function(_)
            | FunctionCallee::FunctionInstance { .. }
            | FunctionCallee::BuiltinOperator(_) => false,
        }
    }

    fn devirtualize_direct_trait_calls_in_place(&mut self, place: &mut FunctionPlace) -> bool {
        let mut changed = false;
        if let FunctionPlaceBase::Deref(expr) = &mut place.base {
            changed |= self.devirtualize_direct_trait_calls_in_expr(expr);
        }
        for elem in &mut place.elems {
            if let FunctionPlaceElem::Index(expr) = elem {
                changed |= self.devirtualize_direct_trait_calls_in_expr(expr);
            }
        }
        changed
    }

    fn devirtualize_direct_trait_calls_in_inline_asm(
        &mut self,
        asm: &mut FunctionInlineAsm,
    ) -> bool {
        let mut changed = false;
        for input in &mut asm.inputs {
            changed |= self.devirtualize_direct_trait_calls_in_expr(&mut input.value);
        }
        for output in &mut asm.outputs {
            changed |= self.devirtualize_direct_trait_calls_in_place(&mut output.place);
        }
        changed
    }

    fn direct_trait_call_target(
        &mut self,
        callee: &FunctionCallee,
    ) -> Option<(Box<FunctionExpr>, GlobalDefId)> {
        let FunctionCallee::DynamicTraitMethod {
            trait_id: TraitId::Source(trait_def_id),
            method_id,
            method_name,
            trait_args,
            receiver,
            ..
        } = callee
        else {
            return None;
        };
        let FunctionExprKind::TraitObjectCoercion {
            expr,
            target_ty: _,
            self_ty,
        } = &receiver.kind
        else {
            return None;
        };
        let (def_id, args) = self.resolve_trait_method_impl(
            *trait_def_id,
            trait_args,
            *method_id,
            method_name,
            *self_ty,
        )?;
        args.is_empty().then(|| (expr.clone(), def_id))
    }

    fn record_devirtualization(
        &mut self,
        function: GlobalDefId,
        is_instance: bool,
        type_arg_count: usize,
    ) {
        self.optimization_report
            .changed_passes
            .push(BackendOptimizationChange::Function {
                module_id: self.input.module_id,
                function,
                pass: DEVIRTUALIZE_DIRECT_TRAIT_CALLS_PASS,
                is_instance,
                type_arg_count,
            });
    }
}
