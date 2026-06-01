// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::{BackendOptimizationChange, ModuleLowerer};
use nia_backend_ir::{BackendFunction, BackendFunctionInstance};
use nia_function_ir::{
    FunctionArrayElements, FunctionBlock, FunctionBody, FunctionCallee, FunctionDeferBody,
    FunctionExpr, FunctionExprKind, FunctionForHeader, FunctionInlineAsm, FunctionOp,
    FunctionPlace, FunctionPlaceBase, FunctionPlaceElem, FunctionTerminator,
};
use nia_ids::{GlobalDefId, InternedTyId};
use nia_opt::InlineThreshold;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FunctionInstanceKey {
    def_id: GlobalDefId,
    args: Vec<InternedTyId>,
}

#[derive(Debug, Clone)]
enum InlineCandidate {
    Function {
        function: GlobalDefId,
        value: FunctionExpr,
    },
    Instance {
        function: GlobalDefId,
        type_arg_count: usize,
        value: FunctionExpr,
    },
}

impl InlineCandidate {
    fn value(&self) -> &FunctionExpr {
        match self {
            Self::Function { value, .. } | Self::Instance { value, .. } => value,
        }
    }

    fn function(&self) -> GlobalDefId {
        match self {
            Self::Function { function, .. } | Self::Instance { function, .. } => *function,
        }
    }

    fn is_instance(&self) -> bool {
        matches!(self, Self::Instance { .. })
    }

    fn type_arg_count(&self) -> usize {
        match self {
            Self::Function { .. } => 0,
            Self::Instance { type_arg_count, .. } => *type_arg_count,
        }
    }
}

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn inline_leaf_functions(
        &mut self,
        functions: &mut [BackendFunction],
        function_instances: &mut [BackendFunctionInstance],
    ) {
        if matches!(self.optimization.inline_threshold, InlineThreshold::Never) {
            return;
        }

        let inline_threshold = self.optimization.inline_threshold;
        let function_candidates = functions
            .iter()
            .filter_map(|function| {
                leaf_inline_return(&function.function_body, inline_threshold).map(|value| {
                    (
                        function.def_id,
                        InlineCandidate::Function {
                            function: function.def_id,
                            value,
                        },
                    )
                })
            })
            .collect::<HashMap<_, _>>();
        let instance_candidates = function_instances
            .iter()
            .filter_map(|instance| {
                leaf_inline_return(&instance.function_body, inline_threshold).map(|value| {
                    (
                        FunctionInstanceKey {
                            def_id: instance.def_id,
                            args: instance.args.clone(),
                        },
                        InlineCandidate::Instance {
                            function: instance.def_id,
                            type_arg_count: instance.args.len(),
                            value,
                        },
                    )
                })
            })
            .collect::<HashMap<_, _>>();
        if function_candidates.is_empty() && instance_candidates.is_empty() {
            return;
        }

        for function in functions {
            if let Some(body) = &mut function.function_body {
                self.inline_leaf_calls_in_body(body, &function_candidates, &instance_candidates);
            }
        }
        for instance in function_instances {
            if let Some(body) = &mut instance.function_body {
                self.inline_leaf_calls_in_body(body, &function_candidates, &instance_candidates);
            }
        }
    }

    fn inline_leaf_calls_in_body(
        &mut self,
        body: &mut FunctionBody,
        function_candidates: &HashMap<GlobalDefId, InlineCandidate>,
        instance_candidates: &HashMap<FunctionInstanceKey, InlineCandidate>,
    ) {
        for block in &mut body.blocks {
            self.inline_leaf_calls_in_block(block, function_candidates, instance_candidates);
        }
    }

    fn inline_leaf_calls_in_defer_body(
        &mut self,
        body: &mut FunctionDeferBody,
        function_candidates: &HashMap<GlobalDefId, InlineCandidate>,
        instance_candidates: &HashMap<FunctionInstanceKey, InlineCandidate>,
    ) {
        for block in &mut body.blocks {
            self.inline_leaf_calls_in_block(block, function_candidates, instance_candidates);
        }
    }

    fn inline_leaf_calls_in_block(
        &mut self,
        block: &mut FunctionBlock,
        function_candidates: &HashMap<GlobalDefId, InlineCandidate>,
        instance_candidates: &HashMap<FunctionInstanceKey, InlineCandidate>,
    ) {
        for op in &mut block.ops {
            self.inline_leaf_calls_in_op(op, function_candidates, instance_candidates);
        }
        self.inline_leaf_calls_in_terminator(
            &mut block.terminator,
            function_candidates,
            instance_candidates,
        );
    }

    fn inline_leaf_calls_in_op(
        &mut self,
        op: &mut FunctionOp,
        function_candidates: &HashMap<GlobalDefId, InlineCandidate>,
        instance_candidates: &HashMap<FunctionInstanceKey, InlineCandidate>,
    ) {
        match op {
            FunctionOp::Binding(binding) => {
                if let Some(value) = &mut binding.value {
                    self.inline_leaf_calls_in_expr(value, function_candidates, instance_candidates);
                }
            }
            FunctionOp::StoreLocal { value, .. } | FunctionOp::Expr(value) => {
                self.inline_leaf_calls_in_expr(value, function_candidates, instance_candidates);
            }
            FunctionOp::Defer(body) => {
                self.inline_leaf_calls_in_defer_body(body, function_candidates, instance_candidates)
            }
        }
    }

    fn inline_leaf_calls_in_terminator(
        &mut self,
        terminator: &mut FunctionTerminator,
        function_candidates: &HashMap<GlobalDefId, InlineCandidate>,
        instance_candidates: &HashMap<FunctionInstanceKey, InlineCandidate>,
    ) {
        match terminator {
            FunctionTerminator::If { cond, .. } => {
                self.inline_leaf_calls_in_expr(cond, function_candidates, instance_candidates);
            }
            FunctionTerminator::Switch { target, arms, .. } => {
                self.inline_leaf_calls_in_expr(target, function_candidates, instance_candidates);
                for arm in arms {
                    self.inline_leaf_calls_in_expr(
                        &mut arm.pattern,
                        function_candidates,
                        instance_candidates,
                    );
                }
            }
            FunctionTerminator::Loop { header, .. } => match header {
                FunctionForHeader::Condition(expr) => {
                    self.inline_leaf_calls_in_expr(expr, function_candidates, instance_candidates);
                }
                FunctionForHeader::CStyle { cond } => {
                    if let Some(cond) = cond {
                        self.inline_leaf_calls_in_expr(
                            cond,
                            function_candidates,
                            instance_candidates,
                        );
                    }
                }
                FunctionForHeader::Infinite => {}
            },
            FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
                if let Some(value) = value {
                    self.inline_leaf_calls_in_expr(value, function_candidates, instance_candidates);
                }
            }
            FunctionTerminator::Error { .. }
            | FunctionTerminator::Branch { .. }
            | FunctionTerminator::Next { .. } => {}
        }
    }

    fn inline_leaf_calls_in_expr(
        &mut self,
        expr: &mut FunctionExpr,
        function_candidates: &HashMap<GlobalDefId, InlineCandidate>,
        instance_candidates: &HashMap<FunctionInstanceKey, InlineCandidate>,
    ) {
        match &mut expr.kind {
            FunctionExprKind::Call { callee, args } => {
                self.inline_leaf_calls_in_callee(callee, function_candidates, instance_candidates);
                for arg in args.iter_mut() {
                    self.inline_leaf_calls_in_expr(arg, function_candidates, instance_candidates);
                }
                if args.is_empty()
                    && let Some(candidate) = inline_candidate_for_callee(
                        callee,
                        function_candidates,
                        instance_candidates,
                    )
                {
                    let value = candidate.value().clone();
                    self.record_inline(candidate);
                    *expr = value;
                }
            }
            FunctionExprKind::Range(range) => {
                if let Some(start) = &mut range.start {
                    self.inline_leaf_calls_in_expr(start, function_candidates, instance_candidates);
                }
                if let Some(end) = &mut range.end {
                    self.inline_leaf_calls_in_expr(end, function_candidates, instance_candidates);
                }
            }
            FunctionExprKind::InlineAsm(asm) => {
                self.inline_leaf_calls_in_inline_asm(asm, function_candidates, instance_candidates);
            }
            FunctionExprKind::CStringPointer { array, .. }
            | FunctionExprKind::Unary { expr: array, .. }
            | FunctionExprKind::Discard(array)
            | FunctionExprKind::Cast { expr: array, .. }
            | FunctionExprKind::TraitObjectUpcast { expr: array, .. }
            | FunctionExprKind::TraitObjectCoercion { expr: array, .. } => {
                self.inline_leaf_calls_in_expr(array, function_candidates, instance_candidates);
            }
            FunctionExprKind::ArrayLiteral { elems } => match elems {
                FunctionArrayElements::List(elems) => {
                    for elem in elems {
                        self.inline_leaf_calls_in_expr(
                            elem,
                            function_candidates,
                            instance_candidates,
                        );
                    }
                }
                FunctionArrayElements::Repeat { value, .. } => {
                    self.inline_leaf_calls_in_expr(value, function_candidates, instance_candidates);
                }
            },
            FunctionExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    self.inline_leaf_calls_in_expr(
                        &mut field.value,
                        function_candidates,
                        instance_candidates,
                    );
                }
            }
            FunctionExprKind::UnionLiteral { field, .. } => {
                self.inline_leaf_calls_in_expr(
                    &mut field.value,
                    function_candidates,
                    instance_candidates,
                );
            }
            FunctionExprKind::AddrOf(place) => {
                self.inline_leaf_calls_in_place(place, function_candidates, instance_candidates);
            }
            FunctionExprKind::Binary { lhs, rhs, .. } => {
                self.inline_leaf_calls_in_expr(lhs, function_candidates, instance_candidates);
                self.inline_leaf_calls_in_expr(rhs, function_candidates, instance_candidates);
            }
            FunctionExprKind::Assign { place, rhs, .. } => {
                self.inline_leaf_calls_in_place(place, function_candidates, instance_candidates);
                self.inline_leaf_calls_in_expr(rhs, function_candidates, instance_candidates);
            }
            FunctionExprKind::Field { lhs, .. } => {
                self.inline_leaf_calls_in_expr(lhs, function_candidates, instance_candidates);
            }
            FunctionExprKind::Index { lhs, index } => {
                self.inline_leaf_calls_in_expr(lhs, function_candidates, instance_candidates);
                self.inline_leaf_calls_in_expr(index, function_candidates, instance_candidates);
            }
            FunctionExprKind::Slice { lhs, range, .. } => {
                self.inline_leaf_calls_in_expr(lhs, function_candidates, instance_candidates);
                if let Some(start) = &mut range.start {
                    self.inline_leaf_calls_in_expr(start, function_candidates, instance_candidates);
                }
                if let Some(end) = &mut range.end {
                    self.inline_leaf_calls_in_expr(end, function_candidates, instance_candidates);
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
    }

    fn inline_leaf_calls_in_callee(
        &mut self,
        callee: &mut FunctionCallee,
        function_candidates: &HashMap<GlobalDefId, InlineCandidate>,
        instance_candidates: &HashMap<FunctionInstanceKey, InlineCandidate>,
    ) {
        match callee {
            FunctionCallee::Method { receiver, .. }
            | FunctionCallee::TraitMethod { receiver, .. }
            | FunctionCallee::DynamicTraitMethod { receiver, .. }
            | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
            | FunctionCallee::FunctionPointer(receiver) => {
                self.inline_leaf_calls_in_expr(receiver, function_candidates, instance_candidates);
            }
            FunctionCallee::Function(_)
            | FunctionCallee::FunctionInstance { .. }
            | FunctionCallee::BuiltinOperator(_) => {}
        }
    }

    fn inline_leaf_calls_in_place(
        &mut self,
        place: &mut FunctionPlace,
        function_candidates: &HashMap<GlobalDefId, InlineCandidate>,
        instance_candidates: &HashMap<FunctionInstanceKey, InlineCandidate>,
    ) {
        if let FunctionPlaceBase::Deref(expr) = &mut place.base {
            self.inline_leaf_calls_in_expr(expr, function_candidates, instance_candidates);
        }
        for elem in &mut place.elems {
            if let FunctionPlaceElem::Index(expr) = elem {
                self.inline_leaf_calls_in_expr(expr, function_candidates, instance_candidates);
            }
        }
    }

    fn inline_leaf_calls_in_inline_asm(
        &mut self,
        asm: &mut FunctionInlineAsm,
        function_candidates: &HashMap<GlobalDefId, InlineCandidate>,
        instance_candidates: &HashMap<FunctionInstanceKey, InlineCandidate>,
    ) {
        for input in &mut asm.inputs {
            self.inline_leaf_calls_in_expr(
                &mut input.value,
                function_candidates,
                instance_candidates,
            );
        }
        for output in &mut asm.outputs {
            self.inline_leaf_calls_in_place(
                &mut output.place,
                function_candidates,
                instance_candidates,
            );
        }
    }

    fn record_inline(&mut self, candidate: &InlineCandidate) {
        self.optimization_report
            .changed_passes
            .push(BackendOptimizationChange::Function {
                module_id: self.input.module_id,
                function: candidate.function(),
                pass: "inline-leaf-functions",
                is_instance: candidate.is_instance(),
                type_arg_count: candidate.type_arg_count(),
            });
    }
}

fn inline_candidate_for_callee<'a>(
    callee: &FunctionCallee,
    function_candidates: &'a HashMap<GlobalDefId, InlineCandidate>,
    instance_candidates: &'a HashMap<FunctionInstanceKey, InlineCandidate>,
) -> Option<&'a InlineCandidate> {
    match callee {
        FunctionCallee::Function(def_id) => function_candidates.get(def_id),
        FunctionCallee::FunctionInstance { def_id, args } => {
            instance_candidates.get(&FunctionInstanceKey {
                def_id: *def_id,
                args: args.clone(),
            })
        }
        FunctionCallee::Method { .. }
        | FunctionCallee::TraitMethod { .. }
        | FunctionCallee::DynamicTraitMethod { .. }
        | FunctionCallee::BuiltinPlaceMethod { .. }
        | FunctionCallee::BuiltinOperator(_)
        | FunctionCallee::FunctionPointer(_) => None,
    }
}

fn leaf_inline_return(
    body: &Option<FunctionBody>,
    threshold: InlineThreshold,
) -> Option<FunctionExpr> {
    let body = body.as_ref()?;
    let [block] = body.blocks.as_slice() else {
        return None;
    };
    if !block.ops.is_empty() {
        return None;
    }
    let value = match &block.terminator {
        FunctionTerminator::Return {
            value: Some(value), ..
        }
        | FunctionTerminator::Tail {
            value: Some(value), ..
        } => value,
        _ => return None,
    };
    inline_expr_allowed(value, threshold).then(|| value.clone())
}

fn inline_expr_allowed(expr: &FunctionExpr, threshold: InlineThreshold) -> bool {
    match threshold {
        InlineThreshold::Never => false,
        InlineThreshold::Minimal | InlineThreshold::Size | InlineThreshold::Small => {
            is_constant_inline_expr(expr)
        }
        InlineThreshold::Normal => small_pure_inline_expr_cost(expr, 4).is_some(),
        InlineThreshold::Aggressive => small_pure_inline_expr_cost(expr, 8).is_some(),
    }
}

fn is_constant_inline_expr(expr: &FunctionExpr) -> bool {
    matches!(
        expr.kind,
        FunctionExprKind::Integer(_)
            | FunctionExprKind::Float(_)
            | FunctionExprKind::String(_)
            | FunctionExprKind::ByteString(_)
            | FunctionExprKind::Char(_)
            | FunctionExprKind::ByteChar(_)
            | FunctionExprKind::Bool(_)
            | FunctionExprKind::BuiltinValue(_)
    )
}

fn small_pure_inline_expr_cost(expr: &FunctionExpr, budget: usize) -> Option<usize> {
    let cost = match &expr.kind {
        FunctionExprKind::Integer(_)
        | FunctionExprKind::Float(_)
        | FunctionExprKind::String(_)
        | FunctionExprKind::ByteString(_)
        | FunctionExprKind::Char(_)
        | FunctionExprKind::ByteChar(_)
        | FunctionExprKind::Bool(_)
        | FunctionExprKind::Global(_)
        | FunctionExprKind::Function(_)
        | FunctionExprKind::FunctionInstance { .. }
        | FunctionExprKind::EnumVariant(_)
        | FunctionExprKind::BuiltinValue(_) => 1,
        FunctionExprKind::Range(range) => {
            1 + optional_inline_expr_cost(range.start.as_deref(), budget)?
                + optional_inline_expr_cost(range.end.as_deref(), budget)?
        }
        FunctionExprKind::ArrayLiteral { elems } => match elems {
            FunctionArrayElements::List(elems) => {
                1 + elems
                    .iter()
                    .map(|elem| small_pure_inline_expr_cost(elem, budget))
                    .sum::<Option<usize>>()?
            }
            FunctionArrayElements::Repeat { value, .. } => {
                1 + small_pure_inline_expr_cost(value, budget)?
            }
        },
        FunctionExprKind::StructLiteral { fields, .. } => {
            1 + fields
                .iter()
                .map(|field| small_pure_inline_expr_cost(&field.value, budget))
                .sum::<Option<usize>>()?
        }
        FunctionExprKind::UnionLiteral { field, .. } => {
            1 + small_pure_inline_expr_cost(&field.value, budget)?
        }
        FunctionExprKind::Unary { expr, .. }
        | FunctionExprKind::Discard(expr)
        | FunctionExprKind::Cast { expr, .. } => 1 + small_pure_inline_expr_cost(expr, budget)?,
        FunctionExprKind::Binary { lhs, rhs, .. } => {
            1 + small_pure_inline_expr_cost(lhs, budget)?
                + small_pure_inline_expr_cost(rhs, budget)?
        }
        FunctionExprKind::Field { lhs, .. } => 1 + small_pure_inline_expr_cost(lhs, budget)?,
        FunctionExprKind::Index { lhs, index } => {
            1 + small_pure_inline_expr_cost(lhs, budget)?
                + small_pure_inline_expr_cost(index, budget)?
        }
        FunctionExprKind::Slice { lhs, range, .. } => {
            1 + small_pure_inline_expr_cost(lhs, budget)?
                + optional_inline_expr_cost(range.start.as_deref(), budget)?
                + optional_inline_expr_cost(range.end.as_deref(), budget)?
        }
        FunctionExprKind::Error
        | FunctionExprKind::Local(_)
        | FunctionExprKind::InlineAsm(_)
        | FunctionExprKind::CStringPointer { .. }
        | FunctionExprKind::AddrOf(_)
        | FunctionExprKind::Assign { .. }
        | FunctionExprKind::TraitObjectUpcast { .. }
        | FunctionExprKind::TraitObjectCoercion { .. }
        | FunctionExprKind::Call { .. } => return None,
    };
    (cost <= budget).then_some(cost)
}

fn optional_inline_expr_cost(expr: Option<&FunctionExpr>, budget: usize) -> Option<usize> {
    expr.map_or(Some(0), |expr| small_pure_inline_expr_cost(expr, budget))
}
