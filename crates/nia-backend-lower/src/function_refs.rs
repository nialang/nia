// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

use nia_function_ir::{
    FunctionArrayElements, FunctionBlock, FunctionBody, FunctionCallee, FunctionDeferBody,
    FunctionExpr, FunctionExprKind, FunctionForHeader, FunctionInlineAsm, FunctionOp,
    FunctionPlace, FunctionPlaceBase, FunctionPlaceElem, FunctionTerminator,
};
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_span::Span;
use nia_static_ir::StaticInit;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FunctionInstanceRef {
    pub(crate) def_id: GlobalDefId,
    pub(crate) arg_module_id: ModuleId,
    pub(crate) args: Vec<InternedTyId>,
    pub(crate) span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct FunctionInstanceKey {
    pub(crate) def_id: GlobalDefId,
    pub(crate) arg_module_id: ModuleId,
    pub(crate) args: Vec<InternedTyId>,
}

impl FunctionInstanceRef {
    pub(crate) fn key(&self) -> FunctionInstanceKey {
        FunctionInstanceKey {
            def_id: self.def_id,
            arg_module_id: self.arg_module_id,
            args: self.args.clone(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct FunctionRefs {
    pub(crate) functions: HashSet<GlobalDefId>,
    pub(crate) instances: Vec<FunctionInstanceRef>,
}

pub(crate) fn collect_function_refs_from_optional_body(
    module_id: ModuleId,
    body: &Option<FunctionBody>,
    refs: &mut FunctionRefs,
) {
    if let Some(body) = body {
        collect_function_refs_from_body(module_id, body, refs);
    }
}

pub(crate) fn collect_function_refs_from_body(
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
        FunctionOp::MemoryIntrinsic(memory) => {
            collect_function_refs_from_expr(module_id, &memory.dest, refs);
            match &memory.source {
                nia_function_ir::FunctionMemoryIntrinsicSource::Slice(source)
                | nia_function_ir::FunctionMemoryIntrinsicSource::Byte(source) => {
                    collect_function_refs_from_expr(module_id, source, refs);
                }
            }
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
        FunctionExprKind::FunctionInstance {
            def_id,
            arg_module_id,
            args,
        } => {
            refs.instances.push(FunctionInstanceRef {
                def_id: *def_id,
                arg_module_id: *arg_module_id,
                args: args.clone(),
                span: expr.span,
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
        FunctionExprKind::Atomic(atomic) => {
            collect_function_refs_from_atomic(module_id, atomic, refs)
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
        FunctionExprKind::ExtractElement { vector, index } => {
            collect_function_refs_from_expr(module_id, vector, refs);
            collect_function_refs_from_expr(module_id, index, refs);
        }
        FunctionExprKind::InsertElement {
            vector,
            index,
            value,
        } => {
            collect_function_refs_from_expr(module_id, vector, refs);
            collect_function_refs_from_expr(module_id, index, refs);
            collect_function_refs_from_expr(module_id, value, refs);
        }
        FunctionExprKind::Assign { place, rhs, .. } => {
            collect_function_refs_from_place(module_id, place, refs);
            collect_function_refs_from_expr(module_id, rhs, refs);
        }
        FunctionExprKind::Call { callee, args } => {
            collect_function_refs_from_callee(module_id, expr.span, callee, refs);
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
        FunctionExprKind::Error => {
            crate::input::unreachable_invalid_function_ir("FunctionExprKind::Error")
        }
        FunctionExprKind::Integer(_)
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
        | FunctionExprKind::BuiltinValue(_)
        | FunctionExprKind::Trap => {}
    }
}

fn collect_function_refs_from_atomic(
    module_id: ModuleId,
    atomic: &nia_function_ir::FunctionAtomic,
    refs: &mut FunctionRefs,
) {
    match atomic {
        nia_function_ir::FunctionAtomic::Load { ptr, .. } => {
            collect_function_refs_from_expr(module_id, ptr, refs)
        }
        nia_function_ir::FunctionAtomic::Store { ptr, value, .. }
        | nia_function_ir::FunctionAtomic::Rmw { ptr, value, .. } => {
            collect_function_refs_from_expr(module_id, ptr, refs);
            collect_function_refs_from_expr(module_id, value, refs);
        }
        nia_function_ir::FunctionAtomic::Cmpxchg {
            ptr,
            expected,
            desired,
            ..
        } => {
            collect_function_refs_from_expr(module_id, ptr, refs);
            collect_function_refs_from_expr(module_id, expected, refs);
            collect_function_refs_from_expr(module_id, desired, refs);
        }
        nia_function_ir::FunctionAtomic::Fence { .. } => {}
    }
}

fn collect_function_refs_from_callee(
    module_id: ModuleId,
    span: Span,
    callee: &FunctionCallee,
    refs: &mut FunctionRefs,
) {
    match callee {
        FunctionCallee::Function(def_id) => {
            refs.functions.insert(*def_id);
        }
        FunctionCallee::FunctionInstance {
            def_id,
            arg_module_id,
            args,
        } => {
            refs.instances.push(FunctionInstanceRef {
                def_id: *def_id,
                arg_module_id: *arg_module_id,
                args: args.clone(),
                span,
            });
        }
        FunctionCallee::Method {
            def_id,
            arg_module_id,
            args,
            receiver,
            ..
        } => {
            if args.is_empty() {
                refs.functions.insert(*def_id);
            } else {
                refs.instances.push(FunctionInstanceRef {
                    def_id: *def_id,
                    arg_module_id: *arg_module_id,
                    args: args.clone(),
                    span,
                });
            }
            collect_function_refs_from_expr(module_id, receiver, refs);
        }
        FunctionCallee::TraitMethod { receiver, .. } => {
            collect_function_refs_from_expr(module_id, receiver, refs);
        }
        FunctionCallee::TraitAssociatedFunction { .. } => {}
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
        FunctionPlaceBase::Local(_) | FunctionPlaceBase::Global(_) => {}
        FunctionPlaceBase::Error => {
            crate::input::unreachable_invalid_function_ir("FunctionPlaceBase::Error")
        }
    }
    for elem in &place.elems {
        match elem {
            FunctionPlaceElem::Index(expr) => {
                collect_function_refs_from_expr(module_id, expr, refs)
            }
            FunctionPlaceElem::Field(_) => {}
            FunctionPlaceElem::Error => {
                crate::input::unreachable_invalid_function_ir("FunctionPlaceElem::Error")
            }
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

pub(crate) fn collect_function_refs_from_static_init(
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
        StaticInit::StaticArrayPointer { array_init, .. } => {
            collect_function_refs_from_static_init(module_id, array_init, refs);
        }
        StaticInit::AddrOfGlobal { .. } => {}
        StaticInit::AddrOfFunction { function, args } => {
            if args.is_empty() {
                refs.functions.insert(*function);
            } else {
                refs.instances.push(FunctionInstanceRef {
                    def_id: *function,
                    arg_module_id: module_id,
                    args: args.clone(),
                    span: Span::default(),
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
