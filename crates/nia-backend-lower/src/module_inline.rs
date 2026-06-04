// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::{BackendOptimizationChange, ModuleLowerer};
use nia_backend_ir::{BackendFunction, BackendFunctionInstance};
use nia_function_ir::{
    FunctionArrayElements, FunctionBlock, FunctionBody, FunctionCallee, FunctionDeferBody,
    FunctionExpr, FunctionExprKind, FunctionForHeader, FunctionInlineAsm, FunctionLocalKind,
    FunctionOp, FunctionPlace, FunctionPlaceBase, FunctionPlaceElem, FunctionTerminator,
};
use nia_ids::{GlobalDefId, InternedTyId, LocalId};
use nia_opt::{InlineThreshold, SpecializationPolicy};

pub(crate) const INLINE_LEAF_FUNCTIONS_PASS: &str = "inline-leaf-functions";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FunctionInstanceKey {
    def_id: GlobalDefId,
    args: Vec<InternedTyId>,
}

#[derive(Debug, Clone)]
enum InlineCandidate {
    Function {
        function: GlobalDefId,
        body: LeafInlineBody,
        threshold: InlineThreshold,
        allow_forwarding_wrapper: bool,
    },
    Instance {
        function: GlobalDefId,
        type_arg_count: usize,
        body: LeafInlineBody,
        threshold: InlineThreshold,
        allow_forwarding_wrapper: bool,
    },
}

impl InlineCandidate {
    fn body(&self) -> &LeafInlineBody {
        match self {
            Self::Function { body, .. } | Self::Instance { body, .. } => body,
        }
    }

    fn threshold(&self) -> InlineThreshold {
        match self {
            Self::Function { threshold, .. } | Self::Instance { threshold, .. } => *threshold,
        }
    }

    fn allow_forwarding_wrapper(&self) -> bool {
        match self {
            Self::Function {
                allow_forwarding_wrapper,
                ..
            }
            | Self::Instance {
                allow_forwarding_wrapper,
                ..
            } => *allow_forwarding_wrapper,
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

#[derive(Debug, Clone)]
struct LeafInlineBody {
    params: Vec<LocalId>,
    value: FunctionExpr,
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
        let instance_inline_threshold = generic_instance_inline_threshold(
            self.optimization.inline_threshold,
            self.optimization.specialize_generics,
        );
        let allow_forwarding_wrapper = self.optimization.prefer_size;
        let function_candidates = functions
            .iter()
            .filter_map(|function| {
                leaf_inline_return(
                    &function.function_body,
                    inline_threshold,
                    allow_forwarding_wrapper,
                )
                .map(|body| {
                    (
                        function.def_id,
                        InlineCandidate::Function {
                            function: function.def_id,
                            body,
                            threshold: inline_threshold,
                            allow_forwarding_wrapper,
                        },
                    )
                })
            })
            .collect::<HashMap<_, _>>();
        let instance_candidates = function_instances
            .iter()
            .filter_map(|instance| {
                leaf_inline_return(
                    &instance.function_body,
                    instance_inline_threshold,
                    allow_forwarding_wrapper,
                )
                .map(|body| {
                    (
                        FunctionInstanceKey {
                            def_id: instance.def_id,
                            args: instance.args.clone(),
                        },
                        InlineCandidate::Instance {
                            function: instance.def_id,
                            type_arg_count: instance.args.len(),
                            body,
                            threshold: instance_inline_threshold,
                            allow_forwarding_wrapper,
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
            FunctionTerminator::Try { value, .. } => {
                self.inline_leaf_calls_in_expr(value, function_candidates, instance_candidates);
            }
            FunctionTerminator::Loop { header, .. } => match header {
                FunctionForHeader::Condition(expr) => {
                    self.inline_leaf_calls_in_expr(expr, function_candidates, instance_candidates);
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
                if let Some(candidate) =
                    inline_candidate_for_callee(callee, function_candidates, instance_candidates)
                    && let Some(value) = inline_candidate_value(candidate, args)
                {
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
            | FunctionExprKind::Null
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
            | FunctionCallee::BuiltinMethod { receiver, .. }
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
                pass: INLINE_LEAF_FUNCTIONS_PASS,
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
        | FunctionCallee::BuiltinMethod { .. }
        | FunctionCallee::BuiltinOperator(_)
        | FunctionCallee::FunctionPointer(_) => None,
    }
}

fn inline_candidate_value(
    candidate: &InlineCandidate,
    args: &[FunctionExpr],
) -> Option<FunctionExpr> {
    let body = candidate.body();
    let threshold = candidate.threshold();
    if body.params.len() != args.len() {
        return None;
    }
    if body.params.is_empty() {
        return Some(body.value.clone());
    }
    if let Some(value) =
        size_safe_forwarded_param_value(candidate.allow_forwarding_wrapper(), threshold, body, args)
    {
        return Some(value);
    }
    if !matches!(
        threshold,
        InlineThreshold::Normal | InlineThreshold::Aggressive
    ) {
        return None;
    }

    let budget = inline_expr_budget(threshold);
    let substitutions = body
        .params
        .iter()
        .copied()
        .zip(args.iter().cloned())
        .collect::<HashMap<_, _>>();
    if substitutions
        .values()
        .any(|arg| small_pure_arg_inline_expr_cost(arg, budget).is_none())
    {
        return None;
    }

    let mut value = body.value.clone();
    substitute_inline_params(&mut value, &substitutions)?;
    small_pure_arg_inline_expr_cost(&value, budget).map(|_| value)
}

fn generic_instance_inline_threshold(
    inline_threshold: InlineThreshold,
    specialization: SpecializationPolicy,
) -> InlineThreshold {
    match specialization {
        SpecializationPolicy::Aggressive | SpecializationPolicy::Normal => inline_threshold,
        SpecializationPolicy::SizeAware => InlineThreshold::Size,
        SpecializationPolicy::RequiredOnly => InlineThreshold::Minimal,
    }
}

fn leaf_inline_return(
    body: &Option<FunctionBody>,
    threshold: InlineThreshold,
    allow_forwarding_wrapper: bool,
) -> Option<LeafInlineBody> {
    let body = body.as_ref()?;
    let (block, value) = leaf_return_shape(body, threshold)?;
    let params = body
        .locals
        .iter()
        .filter(|local| local.kind == FunctionLocalKind::Param)
        .map(|local| local.id)
        .collect::<Vec<_>>();

    if !block.ops.is_empty() {
        return matches!(threshold, InlineThreshold::Aggressive)
            .then(|| aggressive_leaf_inline_return_from_bindings(block, value, params))
            .flatten();
    }

    leaf_inline_return_value(value, params, threshold, allow_forwarding_wrapper)
}

fn leaf_return_shape(
    body: &FunctionBody,
    threshold: InlineThreshold,
) -> Option<(&FunctionBlock, &FunctionExpr)> {
    if let [block] = body.blocks.as_slice() {
        return leaf_return_value(block).map(|value| (block, value));
    }

    if !matches!(threshold, InlineThreshold::Aggressive) {
        return None;
    }

    if body.blocks.len() != 2 {
        return None;
    }
    let entry = body.blocks.iter().find(|block| block.id == body.entry)?;
    let FunctionTerminator::Next { target, .. } = entry.terminator else {
        return None;
    };
    let return_block = body.blocks.iter().find(|block| block.id == target)?;
    if !return_block.ops.is_empty() {
        return None;
    }
    leaf_return_value(return_block).map(|value| (entry, value))
}

fn leaf_return_value(block: &FunctionBlock) -> Option<&FunctionExpr> {
    match &block.terminator {
        FunctionTerminator::Return {
            value: Some(value), ..
        }
        | FunctionTerminator::Tail {
            value: Some(value), ..
        } => Some(value),
        _ => None,
    }
}

fn leaf_inline_return_value(
    value: &FunctionExpr,
    params: Vec<LocalId>,
    threshold: InlineThreshold,
    allow_forwarding_wrapper: bool,
) -> Option<LeafInlineBody> {
    if params.is_empty() {
        return inline_expr_allowed(value, threshold).then(|| LeafInlineBody {
            params,
            value: value.clone(),
        });
    }
    if size_safe_forwarded_param_return(allow_forwarding_wrapper, threshold, value, &params) {
        return Some(LeafInlineBody {
            params,
            value: value.clone(),
        });
    }
    if !matches!(
        threshold,
        InlineThreshold::Normal | InlineThreshold::Aggressive
    ) {
        return None;
    }
    small_pure_param_inline_expr_cost(value, inline_expr_budget(threshold), &params).map(|_| {
        LeafInlineBody {
            params,
            value: value.clone(),
        }
    })
}

fn aggressive_leaf_inline_return_from_bindings(
    block: &FunctionBlock,
    value: &FunctionExpr,
    params: Vec<LocalId>,
) -> Option<LeafInlineBody> {
    let budget = inline_expr_budget(InlineThreshold::Aggressive);
    let mut substitutions = HashMap::<LocalId, FunctionExpr>::new();
    for op in &block.ops {
        let FunctionOp::Binding(binding) = op else {
            return None;
        };
        let mut value = binding.value.clone()?;
        substitute_known_locals(&mut value, &substitutions)?;
        small_pure_param_inline_expr_cost(&value, budget, &params)?;
        substitutions.insert(binding.local_id, value);
    }

    let mut value = value.clone();
    substitute_known_locals(&mut value, &substitutions)?;
    small_pure_param_inline_expr_cost(&value, budget, &params)
        .map(|_| LeafInlineBody { params, value })
}

fn size_safe_forwarded_param_value(
    allow_forwarding_wrapper: bool,
    threshold: InlineThreshold,
    body: &LeafInlineBody,
    args: &[FunctionExpr],
) -> Option<FunctionExpr> {
    if !size_safe_forwarded_param_return(
        allow_forwarding_wrapper,
        threshold,
        &body.value,
        &body.params,
    ) {
        return None;
    }
    let [arg] = args else {
        return None;
    };
    Some(arg.clone())
}

fn size_safe_forwarded_param_return(
    allow_forwarding_wrapper: bool,
    threshold: InlineThreshold,
    value: &FunctionExpr,
    params: &[LocalId],
) -> bool {
    allow_forwarding_wrapper
        && matches!(threshold, InlineThreshold::Minimal | InlineThreshold::Size)
        && matches!(params, [param] if matches!(value.kind, FunctionExprKind::Local(local_id) if local_id == *param))
}

fn inline_expr_allowed(expr: &FunctionExpr, threshold: InlineThreshold) -> bool {
    match threshold {
        InlineThreshold::Never => false,
        InlineThreshold::Minimal | InlineThreshold::Size | InlineThreshold::Small => {
            is_constant_inline_expr(expr)
        }
        InlineThreshold::Normal | InlineThreshold::Aggressive => {
            small_pure_inline_expr_cost(expr, inline_expr_budget(threshold)).is_some()
        }
    }
}

fn inline_expr_budget(threshold: InlineThreshold) -> usize {
    match threshold {
        InlineThreshold::Aggressive => 8,
        _ => 4,
    }
}

fn substitute_inline_params(
    expr: &mut FunctionExpr,
    substitutions: &HashMap<LocalId, FunctionExpr>,
) -> Option<()> {
    substitute_inline_locals(expr, substitutions, true)
}

fn substitute_known_locals(
    expr: &mut FunctionExpr,
    substitutions: &HashMap<LocalId, FunctionExpr>,
) -> Option<()> {
    substitute_inline_locals(expr, substitutions, false)
}

fn substitute_inline_locals(
    expr: &mut FunctionExpr,
    substitutions: &HashMap<LocalId, FunctionExpr>,
    require_local_match: bool,
) -> Option<()> {
    match &mut expr.kind {
        FunctionExprKind::Local(local_id) => {
            if let Some(value) = substitutions.get(local_id) {
                *expr = value.clone();
            } else if require_local_match {
                return None;
            }
        }
        FunctionExprKind::Range(range) => {
            if let Some(start) = &mut range.start {
                substitute_inline_locals(start, substitutions, require_local_match)?;
            }
            if let Some(end) = &mut range.end {
                substitute_inline_locals(end, substitutions, require_local_match)?;
            }
        }
        FunctionExprKind::ArrayLiteral { elems } => match elems {
            FunctionArrayElements::List(elems) => {
                for elem in elems {
                    substitute_inline_locals(elem, substitutions, require_local_match)?;
                }
            }
            FunctionArrayElements::Repeat { value, .. } => {
                substitute_inline_locals(value, substitutions, require_local_match)?;
            }
        },
        FunctionExprKind::StructLiteral { fields, .. } => {
            for field in fields {
                substitute_inline_locals(&mut field.value, substitutions, require_local_match)?;
            }
        }
        FunctionExprKind::UnionLiteral { field, .. } => {
            substitute_inline_locals(&mut field.value, substitutions, require_local_match)?;
        }
        FunctionExprKind::Unary { expr, .. }
        | FunctionExprKind::OptionalSome { expr }
        | FunctionExprKind::ErrorOk { expr }
        | FunctionExprKind::ErrorErr { expr }
        | FunctionExprKind::TaggedUnionTag { expr }
        | FunctionExprKind::TaggedUnionPayload { expr }
        | FunctionExprKind::Try { expr }
        | FunctionExprKind::Discard(expr)
        | FunctionExprKind::Cast { expr, .. }
        | FunctionExprKind::RangeBound { range: expr, .. } => {
            substitute_inline_locals(expr, substitutions, require_local_match)?;
        }
        FunctionExprKind::Binary { lhs, rhs, .. } => {
            substitute_inline_locals(lhs, substitutions, require_local_match)?;
            substitute_inline_locals(rhs, substitutions, require_local_match)?;
        }
        FunctionExprKind::Field { lhs, .. } => {
            substitute_inline_locals(lhs, substitutions, require_local_match)?;
        }
        FunctionExprKind::Index { lhs, index } => {
            substitute_inline_locals(lhs, substitutions, require_local_match)?;
            substitute_inline_locals(index, substitutions, require_local_match)?;
        }
        FunctionExprKind::Slice { lhs, range, .. } => {
            substitute_inline_locals(lhs, substitutions, require_local_match)?;
            if let Some(start) = &mut range.start {
                substitute_inline_locals(start, substitutions, require_local_match)?;
            }
            if let Some(end) = &mut range.end {
                substitute_inline_locals(end, substitutions, require_local_match)?;
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
        | FunctionExprKind::Global(_)
        | FunctionExprKind::Function(_)
        | FunctionExprKind::FunctionInstance { .. }
        | FunctionExprKind::EnumVariant(_)
        | FunctionExprKind::BuiltinValue(_) => {}
        FunctionExprKind::InlineAsm(_)
        | FunctionExprKind::CStringPointer { .. }
        | FunctionExprKind::AddrOf(_)
        | FunctionExprKind::Assign { .. }
        | FunctionExprKind::TraitObjectUpcast { .. }
        | FunctionExprKind::TraitObjectCoercion { .. }
        | FunctionExprKind::Call { .. } => return None,
    }
    Some(())
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
    small_pure_inline_expr_cost_with_local(expr, budget, |_| false)
}

fn small_pure_param_inline_expr_cost(
    expr: &FunctionExpr,
    budget: usize,
    params: &[LocalId],
) -> Option<usize> {
    small_pure_inline_expr_cost_with_local(expr, budget, |local_id| params.contains(&local_id))
}

fn small_pure_arg_inline_expr_cost(expr: &FunctionExpr, budget: usize) -> Option<usize> {
    small_pure_inline_expr_cost_with_local(expr, budget, |_| true)
}

fn small_pure_inline_expr_cost_with_local(
    expr: &FunctionExpr,
    budget: usize,
    local_allowed: impl Fn(LocalId) -> bool + Copy,
) -> Option<usize> {
    let cost = match &expr.kind {
        FunctionExprKind::Integer(_)
        | FunctionExprKind::Float(_)
        | FunctionExprKind::String(_)
        | FunctionExprKind::ByteString(_)
        | FunctionExprKind::Char(_)
        | FunctionExprKind::ByteChar(_)
        | FunctionExprKind::Bool(_)
        | FunctionExprKind::Null
        | FunctionExprKind::Global(_)
        | FunctionExprKind::Function(_)
        | FunctionExprKind::FunctionInstance { .. }
        | FunctionExprKind::EnumVariant(_)
        | FunctionExprKind::BuiltinValue(_) => 1,
        FunctionExprKind::Local(local_id) if local_allowed(*local_id) => 1,
        FunctionExprKind::Range(range) => {
            1 + optional_inline_expr_cost(range.start.as_deref(), budget, local_allowed)?
                + optional_inline_expr_cost(range.end.as_deref(), budget, local_allowed)?
        }
        FunctionExprKind::ArrayLiteral { elems } => match elems {
            FunctionArrayElements::List(elems) => {
                1 + elems
                    .iter()
                    .map(|elem| small_pure_inline_expr_cost_with_local(elem, budget, local_allowed))
                    .sum::<Option<usize>>()?
            }
            FunctionArrayElements::Repeat { value, .. } => {
                1 + small_pure_inline_expr_cost_with_local(value, budget, local_allowed)?
            }
        },
        FunctionExprKind::StructLiteral { fields, .. } => {
            1 + fields
                .iter()
                .map(|field| {
                    small_pure_inline_expr_cost_with_local(&field.value, budget, local_allowed)
                })
                .sum::<Option<usize>>()?
        }
        FunctionExprKind::UnionLiteral { field, .. } => {
            1 + small_pure_inline_expr_cost_with_local(&field.value, budget, local_allowed)?
        }
        FunctionExprKind::OptionalSome { expr }
        | FunctionExprKind::ErrorOk { expr }
        | FunctionExprKind::ErrorErr { expr }
        | FunctionExprKind::TaggedUnionTag { expr }
        | FunctionExprKind::TaggedUnionPayload { expr } => {
            1 + small_pure_inline_expr_cost_with_local(expr, budget, local_allowed)?
        }
        FunctionExprKind::Unary { expr, .. }
        | FunctionExprKind::Discard(expr)
        | FunctionExprKind::Cast { expr, .. }
        | FunctionExprKind::RangeBound { range: expr, .. } => {
            1 + small_pure_inline_expr_cost_with_local(expr, budget, local_allowed)?
        }
        FunctionExprKind::Binary { lhs, rhs, .. } => {
            1 + small_pure_inline_expr_cost_with_local(lhs, budget, local_allowed)?
                + small_pure_inline_expr_cost_with_local(rhs, budget, local_allowed)?
        }
        FunctionExprKind::Field { lhs, .. } => {
            1 + small_pure_inline_expr_cost_with_local(lhs, budget, local_allowed)?
        }
        FunctionExprKind::Index { lhs, index } => {
            1 + small_pure_inline_expr_cost_with_local(lhs, budget, local_allowed)?
                + small_pure_inline_expr_cost_with_local(index, budget, local_allowed)?
        }
        FunctionExprKind::Slice { lhs, range, .. } => {
            1 + small_pure_inline_expr_cost_with_local(lhs, budget, local_allowed)?
                + optional_inline_expr_cost(range.start.as_deref(), budget, local_allowed)?
                + optional_inline_expr_cost(range.end.as_deref(), budget, local_allowed)?
        }
        FunctionExprKind::Error
        | FunctionExprKind::Local(_)
        | FunctionExprKind::InlineAsm(_)
        | FunctionExprKind::CStringPointer { .. }
        | FunctionExprKind::AddrOf(_)
        | FunctionExprKind::Assign { .. }
        | FunctionExprKind::Try { .. }
        | FunctionExprKind::TraitObjectUpcast { .. }
        | FunctionExprKind::TraitObjectCoercion { .. }
        | FunctionExprKind::Call { .. } => return None,
    };
    (cost <= budget).then_some(cost)
}

fn optional_inline_expr_cost(
    expr: Option<&FunctionExpr>,
    budget: usize,
    local_allowed: impl Fn(LocalId) -> bool + Copy,
) -> Option<usize> {
    expr.map_or(Some(0), |expr| {
        small_pure_inline_expr_cost_with_local(expr, budget, local_allowed)
    })
}
