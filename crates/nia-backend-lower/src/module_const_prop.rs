// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::{BackendOptimizationChange, ModuleLowerer};
use nia_backend_ir::{BackendFunction, BackendFunctionInstance};
use nia_function_ir::{
    FunctionArrayElements, FunctionBlock, FunctionBody, FunctionCallee, FunctionDeferBody,
    FunctionExpr, FunctionExprKind, FunctionForHeader, FunctionInlineAsm, FunctionLocalKind,
    FunctionOp, FunctionPlace, FunctionPlaceBase, FunctionPlaceElem, FunctionTerminator,
};
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_opt::OptimizationDepth;
use nia_ty::ConstGenericArg;

pub(crate) const PROPAGATE_CROSS_FUNCTION_CONSTANTS_PASS: &str =
    "propagate-cross-function-constants";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FunctionInstanceKey {
    def_id: GlobalDefId,
    arg_module_id: ModuleId,
    self_arg: Option<InternedTyId>,
    args: Vec<InternedTyId>,
    const_args: Vec<ConstGenericArg>,
}

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn propagate_cross_function_constants(
        &mut self,
        functions: &mut [BackendFunction],
        function_instances: &mut [BackendFunctionInstance],
    ) {
        if !cross_function_constant_propagation_enabled(&self.optimization) {
            return;
        }

        let function_constants = functions
            .iter()
            .filter_map(|function| {
                constant_leaf_return(&function.function_body)
                    .map(|value| (function.def_id, value.clone()))
            })
            .collect::<HashMap<_, _>>();
        let instance_constants = function_instances
            .iter()
            .filter_map(|instance| {
                constant_leaf_return(&instance.function_body).map(|value| {
                    (
                        FunctionInstanceKey {
                            def_id: instance.def_id,
                            arg_module_id: instance.arg_module_id,
                            self_arg: instance.self_arg,
                            args: instance.args.clone(),
                            const_args: instance.const_args.clone(),
                        },
                        value.clone(),
                    )
                })
            })
            .collect::<HashMap<_, _>>();
        if function_constants.is_empty() && instance_constants.is_empty() {
            return;
        }

        for function in functions {
            if let Some(body) = &mut function.function_body
                && propagate_cross_function_constants_in_body(
                    body,
                    &function_constants,
                    &instance_constants,
                )
            {
                self.record_cross_function_constant_propagation(function.def_id, false, 0);
            }
        }
        for instance in function_instances {
            if let Some(body) = &mut instance.function_body
                && propagate_cross_function_constants_in_body(
                    body,
                    &function_constants,
                    &instance_constants,
                )
            {
                self.record_cross_function_constant_propagation(
                    instance.def_id,
                    true,
                    instance.args.len(),
                );
            }
        }
    }

    fn record_cross_function_constant_propagation(
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
                pass: PROPAGATE_CROSS_FUNCTION_CONSTANTS_PASS,
                is_instance,
                type_arg_count,
            });
    }
}

pub(crate) fn cross_function_constant_propagation_enabled(
    optimization: &nia_opt::OptimizationPolicy,
) -> bool {
    optimization
        .const_fold
        .at_least(OptimizationDepth::Aggressive)
        && !optimization.prefer_size
}

fn constant_leaf_return(body: &Option<FunctionBody>) -> Option<&FunctionExpr> {
    let body = body.as_ref()?;
    if body
        .locals
        .iter()
        .any(|local| local.kind == FunctionLocalKind::Param)
    {
        return None;
    }
    let [block] = body.blocks.as_slice() else {
        return None;
    };
    if !block.ops.is_empty() {
        return None;
    }
    let value = leaf_return_value(block)?;
    is_cross_function_constant(value).then_some(value)
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

fn is_cross_function_constant(expr: &FunctionExpr) -> bool {
    matches!(
        expr.kind,
        FunctionExprKind::Integer(_)
            | FunctionExprKind::Float(_)
            | FunctionExprKind::String(_)
            | FunctionExprKind::ByteString(_)
            | FunctionExprKind::Char(_)
            | FunctionExprKind::ByteChar(_)
            | FunctionExprKind::Bool(_)
            | FunctionExprKind::ConstGeneric(_)
            | FunctionExprKind::BuiltinValue(_)
    )
}

fn propagate_cross_function_constants_in_body(
    body: &mut FunctionBody,
    function_constants: &HashMap<GlobalDefId, FunctionExpr>,
    instance_constants: &HashMap<FunctionInstanceKey, FunctionExpr>,
) -> bool {
    propagate_cross_function_constants_in_blocks(
        &mut body.blocks,
        function_constants,
        instance_constants,
    )
}

fn propagate_cross_function_constants_in_defer_body(
    body: &mut FunctionDeferBody,
    function_constants: &HashMap<GlobalDefId, FunctionExpr>,
    instance_constants: &HashMap<FunctionInstanceKey, FunctionExpr>,
) -> bool {
    propagate_cross_function_constants_in_blocks(
        &mut body.blocks,
        function_constants,
        instance_constants,
    )
}

fn propagate_cross_function_constants_in_blocks(
    blocks: &mut [FunctionBlock],
    function_constants: &HashMap<GlobalDefId, FunctionExpr>,
    instance_constants: &HashMap<FunctionInstanceKey, FunctionExpr>,
) -> bool {
    let mut changed = false;
    for block in blocks {
        for op in &mut block.ops {
            changed |= propagate_cross_function_constants_in_op(
                op,
                function_constants,
                instance_constants,
            );
        }
        changed |= propagate_cross_function_constants_in_terminator(
            &mut block.terminator,
            function_constants,
            instance_constants,
        );
    }
    changed
}

fn propagate_cross_function_constants_in_op(
    op: &mut FunctionOp,
    function_constants: &HashMap<GlobalDefId, FunctionExpr>,
    instance_constants: &HashMap<FunctionInstanceKey, FunctionExpr>,
) -> bool {
    match op {
        FunctionOp::Binding(binding) => binding.value.as_mut().is_some_and(|value| {
            propagate_cross_function_constants_in_expr(
                value,
                function_constants,
                instance_constants,
            )
        }),
        FunctionOp::StoreLocal { value, .. } | FunctionOp::Expr(value) => {
            propagate_cross_function_constants_in_expr(
                value,
                function_constants,
                instance_constants,
            )
        }
        FunctionOp::MemoryIntrinsic(memory) => {
            let mut changed = propagate_cross_function_constants_in_expr(
                &mut memory.dest,
                function_constants,
                instance_constants,
            );
            changed |= match &mut memory.source {
                nia_function_ir::FunctionMemoryIntrinsicSource::Slice(source)
                | nia_function_ir::FunctionMemoryIntrinsicSource::Byte(source) => {
                    propagate_cross_function_constants_in_expr(
                        source,
                        function_constants,
                        instance_constants,
                    )
                }
            };
            changed
        }
        FunctionOp::Defer(body) => propagate_cross_function_constants_in_defer_body(
            body,
            function_constants,
            instance_constants,
        ),
    }
}

fn propagate_cross_function_constants_in_terminator(
    terminator: &mut FunctionTerminator,
    function_constants: &HashMap<GlobalDefId, FunctionExpr>,
    instance_constants: &HashMap<FunctionInstanceKey, FunctionExpr>,
) -> bool {
    match terminator {
        FunctionTerminator::If { cond, .. } => {
            propagate_cross_function_constants_in_expr(cond, function_constants, instance_constants)
        }
        FunctionTerminator::Switch { target, arms, .. } => {
            let mut changed = propagate_cross_function_constants_in_expr(
                target,
                function_constants,
                instance_constants,
            );
            for arm in arms {
                changed |= propagate_cross_function_constants_in_expr(
                    &mut arm.pattern,
                    function_constants,
                    instance_constants,
                );
            }
            changed
        }
        FunctionTerminator::Try {
            value,
            error_conversion,
            ..
        } => {
            let mut changed = propagate_cross_function_constants_in_expr(
                value,
                function_constants,
                instance_constants,
            );
            if let Some(conversion) = error_conversion {
                changed |= propagate_cross_function_constants_in_expr(
                    conversion,
                    function_constants,
                    instance_constants,
                );
            }
            changed
        }
        FunctionTerminator::Loop { header, .. } => match header {
            FunctionForHeader::Condition(cond) => propagate_cross_function_constants_in_expr(
                cond,
                function_constants,
                instance_constants,
            ),
            FunctionForHeader::Infinite => false,
        },
        FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
            value.as_mut().is_some_and(|value| {
                propagate_cross_function_constants_in_expr(
                    value,
                    function_constants,
                    instance_constants,
                )
            })
        }
        FunctionTerminator::Error { .. }
        | FunctionTerminator::Branch { .. }
        | FunctionTerminator::Next { .. } => false,
    }
}

fn propagate_cross_function_constants_in_expr(
    expr: &mut FunctionExpr,
    function_constants: &HashMap<GlobalDefId, FunctionExpr>,
    instance_constants: &HashMap<FunctionInstanceKey, FunctionExpr>,
) -> bool {
    let mut changed = false;
    match &mut expr.kind {
        FunctionExprKind::Call { callee, args } => {
            changed |= propagate_cross_function_constants_in_callee(
                callee,
                function_constants,
                instance_constants,
            );
            for arg in args.iter_mut() {
                changed |= propagate_cross_function_constants_in_expr(
                    arg,
                    function_constants,
                    instance_constants,
                );
            }
            if args.is_empty()
                && let Some(value) = cross_function_constant_for_callee(
                    callee,
                    function_constants,
                    instance_constants,
                )
            {
                *expr = value.clone();
                changed = true;
            }
        }
        FunctionExprKind::Range(range) => {
            if let Some(start) = &mut range.start {
                changed |= propagate_cross_function_constants_in_expr(
                    start,
                    function_constants,
                    instance_constants,
                );
            }
            if let Some(end) = &mut range.end {
                changed |= propagate_cross_function_constants_in_expr(
                    end,
                    function_constants,
                    instance_constants,
                );
            }
        }
        FunctionExprKind::InlineAsm(asm) => {
            changed |= propagate_cross_function_constants_in_inline_asm(
                asm,
                function_constants,
                instance_constants,
            );
        }
        FunctionExprKind::Atomic(atomic) => {
            changed |= propagate_cross_function_constants_in_atomic(
                atomic,
                function_constants,
                instance_constants,
            );
        }
        FunctionExprKind::StaticArrayPointer { array, .. }
        | FunctionExprKind::RangeBound { range: array, .. }
        | FunctionExprKind::Unary { expr: array, .. }
        | FunctionExprKind::OptionalSome { expr: array }
        | FunctionExprKind::ErrorOk { expr: array }
        | FunctionExprKind::ErrorErr { expr: array }
        | FunctionExprKind::TaggedUnionTag { expr: array }
        | FunctionExprKind::TaggedUnionPayload { expr: array }
        | FunctionExprKind::Try { expr: array }
        | FunctionExprKind::LoadUnaligned { ptr: array, .. }
        | FunctionExprKind::Splat { value: array }
        | FunctionExprKind::Bitmask { vector: array }
        | FunctionExprKind::BitIntrinsic { value: array, .. }
        | FunctionExprKind::CharFromU32 { value: array }
        | FunctionExprKind::Discard(array)
        | FunctionExprKind::Cast { expr: array, .. }
        | FunctionExprKind::TraitObjectUpcast { expr: array, .. }
        | FunctionExprKind::TraitObjectCoercion { expr: array, .. } => {
            changed |= propagate_cross_function_constants_in_expr(
                array,
                function_constants,
                instance_constants,
            );
        }
        FunctionExprKind::ArrayLiteral { elems } => match elems {
            FunctionArrayElements::List(elems) => {
                for elem in elems {
                    changed |= propagate_cross_function_constants_in_expr(
                        elem,
                        function_constants,
                        instance_constants,
                    );
                }
            }
            FunctionArrayElements::Repeat { value, .. } => {
                changed |= propagate_cross_function_constants_in_expr(
                    value,
                    function_constants,
                    instance_constants,
                );
            }
        },
        FunctionExprKind::Tuple(elems) => {
            for elem in elems {
                changed |= propagate_cross_function_constants_in_expr(
                    elem,
                    function_constants,
                    instance_constants,
                );
            }
        }
        FunctionExprKind::TupleField { value, .. } => {
            changed |= propagate_cross_function_constants_in_expr(
                value,
                function_constants,
                instance_constants,
            );
        }
        FunctionExprKind::StructLiteral { fields, .. } => {
            for field in fields {
                changed |= propagate_cross_function_constants_in_expr(
                    &mut field.value,
                    function_constants,
                    instance_constants,
                );
            }
        }
        FunctionExprKind::UnionLiteral { field, .. } => {
            changed |= propagate_cross_function_constants_in_expr(
                &mut field.value,
                function_constants,
                instance_constants,
            );
        }
        FunctionExprKind::AddrOf(place) => {
            changed |= propagate_cross_function_constants_in_place(
                place,
                function_constants,
                instance_constants,
            );
        }
        FunctionExprKind::Binary { lhs, rhs, .. } => {
            changed |= propagate_cross_function_constants_in_expr(
                lhs,
                function_constants,
                instance_constants,
            );
            changed |= propagate_cross_function_constants_in_expr(
                rhs,
                function_constants,
                instance_constants,
            );
        }
        FunctionExprKind::ExtractElement { vector, index } => {
            changed |= propagate_cross_function_constants_in_expr(
                vector,
                function_constants,
                instance_constants,
            );
            changed |= propagate_cross_function_constants_in_expr(
                index,
                function_constants,
                instance_constants,
            );
        }
        FunctionExprKind::InsertElement {
            vector,
            index,
            value,
        } => {
            changed |= propagate_cross_function_constants_in_expr(
                vector,
                function_constants,
                instance_constants,
            );
            changed |= propagate_cross_function_constants_in_expr(
                index,
                function_constants,
                instance_constants,
            );
            changed |= propagate_cross_function_constants_in_expr(
                value,
                function_constants,
                instance_constants,
            );
        }
        FunctionExprKind::Assign { place, rhs, .. } => {
            changed |= propagate_cross_function_constants_in_place(
                place,
                function_constants,
                instance_constants,
            );
            changed |= propagate_cross_function_constants_in_expr(
                rhs,
                function_constants,
                instance_constants,
            );
        }
        FunctionExprKind::Field { lhs, .. } => {
            changed |= propagate_cross_function_constants_in_expr(
                lhs,
                function_constants,
                instance_constants,
            );
        }
        FunctionExprKind::Index { lhs, index } => {
            changed |= propagate_cross_function_constants_in_expr(
                lhs,
                function_constants,
                instance_constants,
            );
            changed |= propagate_cross_function_constants_in_expr(
                index,
                function_constants,
                instance_constants,
            );
        }
        FunctionExprKind::Slice { lhs, range, .. } => {
            changed |= propagate_cross_function_constants_in_expr(
                lhs,
                function_constants,
                instance_constants,
            );
            if let Some(start) = &mut range.start {
                changed |= propagate_cross_function_constants_in_expr(
                    start,
                    function_constants,
                    instance_constants,
                );
            }
            if let Some(end) = &mut range.end {
                changed |= propagate_cross_function_constants_in_expr(
                    end,
                    function_constants,
                    instance_constants,
                );
            }
        }
        FunctionExprKind::Error => {
            crate::input::unreachable_invalid_function_ir("FunctionExprKind::Error")
        }
        FunctionExprKind::EnumVariant { fields, .. } => {
            for field in fields {
                changed |= propagate_cross_function_constants_in_expr(
                    field,
                    function_constants,
                    instance_constants,
                );
            }
        }
        FunctionExprKind::EnumTag { value } | FunctionExprKind::EnumPayloadField { value, .. } => {
            changed |= propagate_cross_function_constants_in_expr(
                value,
                function_constants,
                instance_constants,
            );
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
        | FunctionExprKind::GlobalInstance { .. }
        | FunctionExprKind::Function(_)
        | FunctionExprKind::FunctionInstance { .. }
        | FunctionExprKind::EnumVariantTag(_)
        | FunctionExprKind::BuiltinValue(_) => {}
        FunctionExprKind::UnionStorageLiteral { relocations, .. } => {
            for relocation in relocations {
                changed |= propagate_cross_function_constants_in_expr(
                    &mut relocation.pointee,
                    function_constants,
                    instance_constants,
                );
            }
        }
    }
    changed
}

fn propagate_cross_function_constants_in_atomic(
    atomic: &mut nia_function_ir::FunctionAtomic,
    function_constants: &HashMap<GlobalDefId, FunctionExpr>,
    instance_constants: &HashMap<FunctionInstanceKey, FunctionExpr>,
) -> bool {
    match atomic {
        nia_function_ir::FunctionAtomic::Load { ptr, .. } => {
            propagate_cross_function_constants_in_expr(ptr, function_constants, instance_constants)
        }
        nia_function_ir::FunctionAtomic::Store { ptr, value, .. }
        | nia_function_ir::FunctionAtomic::Rmw { ptr, value, .. } => {
            propagate_cross_function_constants_in_expr(ptr, function_constants, instance_constants)
                | propagate_cross_function_constants_in_expr(
                    value,
                    function_constants,
                    instance_constants,
                )
        }
        nia_function_ir::FunctionAtomic::Cmpxchg {
            ptr,
            expected,
            desired,
            ..
        } => {
            propagate_cross_function_constants_in_expr(ptr, function_constants, instance_constants)
                | propagate_cross_function_constants_in_expr(
                    expected,
                    function_constants,
                    instance_constants,
                )
                | propagate_cross_function_constants_in_expr(
                    desired,
                    function_constants,
                    instance_constants,
                )
        }
        nia_function_ir::FunctionAtomic::Fence { .. } => false,
    }
}

fn propagate_cross_function_constants_in_callee(
    callee: &mut FunctionCallee,
    function_constants: &HashMap<GlobalDefId, FunctionExpr>,
    instance_constants: &HashMap<FunctionInstanceKey, FunctionExpr>,
) -> bool {
    match callee {
        FunctionCallee::Method { receiver, .. }
        | FunctionCallee::TraitMethod { receiver, .. }
        | FunctionCallee::DynamicTraitMethod { receiver, .. }
        | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
        | FunctionCallee::BuiltinMethod { receiver, .. }
        | FunctionCallee::FunctionPointer(receiver) => propagate_cross_function_constants_in_expr(
            receiver,
            function_constants,
            instance_constants,
        ),
        FunctionCallee::Function(_)
        | FunctionCallee::FunctionInstance { .. }
        | FunctionCallee::TraitAssociatedFunction { .. }
        | FunctionCallee::BuiltinOperator(_) => false,
    }
}

fn propagate_cross_function_constants_in_place(
    place: &mut FunctionPlace,
    function_constants: &HashMap<GlobalDefId, FunctionExpr>,
    instance_constants: &HashMap<FunctionInstanceKey, FunctionExpr>,
) -> bool {
    let mut changed = false;
    match &mut place.base {
        FunctionPlaceBase::Deref(expr) => {
            changed |= propagate_cross_function_constants_in_expr(
                expr,
                function_constants,
                instance_constants,
            );
        }
        FunctionPlaceBase::Local(_)
        | FunctionPlaceBase::Global(_)
        | FunctionPlaceBase::GlobalInstance { .. } => {}
        FunctionPlaceBase::Error => {
            crate::input::unreachable_invalid_function_ir("FunctionPlaceBase::Error")
        }
    }
    for elem in &mut place.elems {
        if let FunctionPlaceElem::Index(expr) = elem {
            changed |= propagate_cross_function_constants_in_expr(
                expr,
                function_constants,
                instance_constants,
            );
        }
    }
    changed
}

fn propagate_cross_function_constants_in_inline_asm(
    asm: &mut FunctionInlineAsm,
    function_constants: &HashMap<GlobalDefId, FunctionExpr>,
    instance_constants: &HashMap<FunctionInstanceKey, FunctionExpr>,
) -> bool {
    let mut changed = false;
    for input in &mut asm.inputs {
        changed |= propagate_cross_function_constants_in_expr(
            &mut input.value,
            function_constants,
            instance_constants,
        );
    }
    for output in &mut asm.outputs {
        changed |= propagate_cross_function_constants_in_place(
            &mut output.place,
            function_constants,
            instance_constants,
        );
    }
    changed
}

fn cross_function_constant_for_callee<'a>(
    callee: &FunctionCallee,
    function_constants: &'a HashMap<GlobalDefId, FunctionExpr>,
    instance_constants: &'a HashMap<FunctionInstanceKey, FunctionExpr>,
) -> Option<&'a FunctionExpr> {
    match callee {
        FunctionCallee::Function(def_id) => function_constants.get(def_id),
        FunctionCallee::FunctionInstance {
            def_id,
            arg_module_id,
            self_arg,
            args,
            const_args,
        } => instance_constants.get(&FunctionInstanceKey {
            def_id: *def_id,
            arg_module_id: *arg_module_id,
            self_arg: *self_arg,
            args: args.clone(),
            const_args: const_args.clone(),
        }),
        FunctionCallee::Method { .. }
        | FunctionCallee::TraitMethod { .. }
        | FunctionCallee::TraitAssociatedFunction { .. }
        | FunctionCallee::DynamicTraitMethod { .. }
        | FunctionCallee::BuiltinPlaceMethod { .. }
        | FunctionCallee::BuiltinMethod { .. }
        | FunctionCallee::BuiltinOperator(_)
        | FunctionCallee::FunctionPointer(_) => None,
    }
}
