//! Discovers every callable body reachable through typed function IR.

use std::collections::HashMap;

use nia_body_ir::{
    PlaceBase, PlaceElem, TypedArrayElements, TypedAtomic, TypedBody, TypedCallee, TypedExpr,
    TypedExprKind, TypedMatchArmBody, TypedMemoryIntrinsicSource, TypedPlace, TypedStmtKind,
};

use super::{CallableBody, CallableKey};

/// Adds nested closures to the same analysis graph as their owning function.
///
/// Discovery runs before summary iteration. Missing a typed IR edge here would
/// silently turn a known closure invocation into a conservative unknown call,
/// changing both escape summaries and diagnostics.
pub(super) fn collect_body_closures<'a>(
    body: &'a TypedBody,
    callables: &mut HashMap<CallableKey, CallableBody<'a>>,
) {
    for stmt in &body.stmts {
        collect_stmt_closures(stmt, callables);
    }
    if let Some(tail) = &body.tail {
        collect_expr_closures(tail, callables);
    }
}

fn collect_stmt_closures<'a>(
    stmt: &'a nia_body_ir::TypedStmt,
    callables: &mut HashMap<CallableKey, CallableBody<'a>>,
) {
    match &stmt.kind {
        TypedStmtKind::Binding(binding) => {
            if let Some(value) = &binding.value {
                collect_expr_closures(value, callables);
            }
        }
        TypedStmtKind::PatternBinding(binding) => {
            collect_pattern_closures(&binding.pattern, callables);
            collect_expr_closures(&binding.value, callables);
        }
        TypedStmtKind::Expr(expr)
        | TypedStmtKind::Return(Some(expr))
        | TypedStmtKind::Defer(expr) => collect_expr_closures(expr, callables),
        TypedStmtKind::ForIn(for_in) => {
            collect_pattern_closures(&for_in.pattern, callables);
            collect_expr_closures(&for_in.iter, callables);
            collect_body_closures(&for_in.body, callables);
        }
        TypedStmtKind::While(while_stmt) => {
            collect_expr_closures(&while_stmt.cond, callables);
            collect_body_closures(&while_stmt.body, callables);
        }
        TypedStmtKind::Loop(loop_stmt) => collect_body_closures(&loop_stmt.body, callables),
        TypedStmtKind::Return(None) | TypedStmtKind::Break | TypedStmtKind::Continue => {}
    }
}

fn collect_expr_closures<'a>(
    expr: &'a TypedExpr,
    callables: &mut HashMap<CallableKey, CallableBody<'a>>,
) {
    match &expr.kind {
        TypedExprKind::Closure {
            closure_id,
            captures,
            params,
            body,
        } => {
            callables.insert(
                CallableKey::Closure(*closure_id),
                CallableBody {
                    captures: captures.iter().map(|capture| capture.local_id).collect(),
                    params: params.clone(),
                    body,
                },
            );
            for capture in captures {
                collect_expr_closures(&capture.value, callables);
            }
            collect_body_closures(body, callables);
        }
        TypedExprKind::EnumVariant { fields, .. } | TypedExprKind::Tuple(fields) => {
            for field in fields {
                collect_expr_closures(field, callables);
            }
        }
        TypedExprKind::Range(range) => {
            for bound in range.start.iter().chain(&range.end) {
                collect_expr_closures(bound, callables);
            }
        }
        TypedExprKind::InlineAsm(asm) => {
            for input in &asm.inputs {
                collect_expr_closures(&input.value, callables);
            }
            for output in &asm.outputs {
                collect_place_closures(&output.place, callables);
            }
        }
        TypedExprKind::MemoryIntrinsic(intrinsic) => {
            collect_expr_closures(&intrinsic.dest, callables);
            match &intrinsic.source {
                TypedMemoryIntrinsicSource::Slice(source)
                | TypedMemoryIntrinsicSource::Byte(source) => {
                    collect_expr_closures(source, callables)
                }
            }
        }
        TypedExprKind::Atomic(atomic) => match atomic {
            TypedAtomic::Load { ptr, .. } => collect_expr_closures(ptr, callables),
            TypedAtomic::Store { ptr, value, .. } | TypedAtomic::Rmw { ptr, value, .. } => {
                collect_expr_closures(ptr, callables);
                collect_expr_closures(value, callables);
            }
            TypedAtomic::Cmpxchg {
                ptr,
                expected,
                desired,
                ..
            } => {
                collect_expr_closures(ptr, callables);
                collect_expr_closures(expected, callables);
                collect_expr_closures(desired, callables);
            }
            TypedAtomic::Fence { .. } => {}
        },
        TypedExprKind::LoadUnaligned { ptr, .. }
        | TypedExprKind::Splat { value: ptr }
        | TypedExprKind::BitIntrinsic { value: ptr, .. }
        | TypedExprKind::CharFromU32 { value: ptr }
        | TypedExprKind::StaticArrayPointer { array: ptr, .. }
        | TypedExprKind::OptionalSome { expr: ptr }
        | TypedExprKind::ErrorOk { expr: ptr }
        | TypedExprKind::ErrorErr { expr: ptr }
        | TypedExprKind::Try { expr: ptr, .. }
        | TypedExprKind::Discard(ptr)
        | TypedExprKind::Cast { expr: ptr, .. }
        | TypedExprKind::TraitObjectUpcast { expr: ptr, .. }
        | TypedExprKind::TraitObjectCoercion { expr: ptr, .. }
        | TypedExprKind::CallableCoercion { state: ptr, .. }
        | TypedExprKind::Unary { expr: ptr, .. }
        | TypedExprKind::Field { lhs: ptr, .. }
        | TypedExprKind::TupleField { lhs: ptr, .. } => collect_expr_closures(ptr, callables),
        TypedExprKind::ExtractElement { vector, index }
        | TypedExprKind::Binary {
            lhs: vector,
            rhs: index,
            ..
        }
        | TypedExprKind::Index { lhs: vector, index } => {
            collect_expr_closures(vector, callables);
            collect_expr_closures(index, callables);
        }
        TypedExprKind::InsertElement {
            vector,
            index,
            value,
        } => {
            collect_expr_closures(vector, callables);
            collect_expr_closures(index, callables);
            collect_expr_closures(value, callables);
        }
        TypedExprKind::Bitmask { vector } => collect_expr_closures(vector, callables),
        TypedExprKind::ArrayLiteral { elems } => match elems {
            TypedArrayElements::List(elems) => {
                for elem in elems {
                    collect_expr_closures(elem, callables);
                }
            }
            TypedArrayElements::Repeat { value, .. } => collect_expr_closures(value, callables),
        },
        TypedExprKind::StructLiteral { fields, .. } => {
            for field in fields {
                collect_expr_closures(&field.value, callables);
            }
        }
        TypedExprKind::UnionLiteral { field, .. } => collect_expr_closures(&field.value, callables),
        TypedExprKind::UnionStorageLiteral { relocations, .. } => {
            for relocation in relocations {
                collect_expr_closures(&relocation.pointee, callables);
            }
        }
        TypedExprKind::Assign { place, rhs, .. } => {
            collect_place_closures(place, callables);
            collect_expr_closures(rhs, callables);
        }
        TypedExprKind::Call { callee, args } => {
            collect_callee_closures(callee, callables);
            for arg in args {
                collect_expr_closures(arg, callables);
            }
        }
        TypedExprKind::Slice { lhs, range, .. } => {
            collect_expr_closures(lhs, callables);
            for bound in range.start.iter().chain(&range.end) {
                collect_expr_closures(bound, callables);
            }
        }
        TypedExprKind::Block(body) => collect_body_closures(body, callables),
        TypedExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_closures(cond, callables);
            collect_body_closures(then_branch, callables);
            if let Some(branch) = else_branch {
                collect_expr_closures(branch, callables);
            }
        }
        TypedExprKind::IfPattern(pattern) => {
            collect_expr_closures(&pattern.target, callables);
            collect_pattern_closures(&pattern.pattern, callables);
            collect_body_closures(&pattern.then_branch, callables);
            if let Some(branch) = &pattern.else_branch {
                collect_expr_closures(branch, callables);
            }
        }
        TypedExprKind::Match(matched) => {
            collect_expr_closures(&matched.target, callables);
            for arm in &matched.arms {
                for pattern in &arm.patterns {
                    collect_pattern_closures(pattern, callables);
                }
                match &arm.body {
                    TypedMatchArmBody::Expr(expr) => collect_expr_closures(expr, callables),
                    TypedMatchArmBody::Stmt(stmt) => collect_stmt_closures(stmt, callables),
                    TypedMatchArmBody::Block(body) => collect_body_closures(body, callables),
                }
            }
        }
        TypedExprKind::Error
        | TypedExprKind::Integer(_)
        | TypedExprKind::Float(_)
        | TypedExprKind::String(_)
        | TypedExprKind::ByteString(_)
        | TypedExprKind::Char(_)
        | TypedExprKind::ByteChar(_)
        | TypedExprKind::Bool(_)
        | TypedExprKind::Null
        | TypedExprKind::Local(_)
        | TypedExprKind::Global(_)
        | TypedExprKind::ConstGeneric(_)
        | TypedExprKind::Function(_)
        | TypedExprKind::FunctionInstance { .. }
        | TypedExprKind::BuiltinValue(_)
        | TypedExprKind::Trap
        | TypedExprKind::ClosureFunctionPointer { .. } => {}
    }
}

fn collect_pattern_closures<'a>(
    pattern: &'a nia_body_ir::TypedPattern,
    callables: &mut HashMap<CallableKey, CallableBody<'a>>,
) {
    match &pattern.kind {
        nia_body_ir::TypedPatternKind::Pointer(inner)
        | nia_body_ir::TypedPatternKind::MutPointer(inner)
        | nia_body_ir::TypedPatternKind::OptionalSome(inner)
        | nia_body_ir::TypedPatternKind::ErrorOk(inner)
        | nia_body_ir::TypedPatternKind::ErrorErr(inner) => {
            collect_pattern_closures(inner, callables)
        }
        nia_body_ir::TypedPatternKind::Tuple(patterns)
        | nia_body_ir::TypedPatternKind::Nominal {
            fields: patterns, ..
        } => {
            for pattern in patterns {
                collect_pattern_closures(pattern, callables);
            }
        }
        nia_body_ir::TypedPatternKind::Expr(expr) => collect_expr_closures(expr, callables),
        nia_body_ir::TypedPatternKind::Range { start, end, .. } => {
            collect_expr_closures(start, callables);
            collect_expr_closures(end, callables);
        }
        nia_body_ir::TypedPatternKind::Wildcard
        | nia_body_ir::TypedPatternKind::Bind { .. }
        | nia_body_ir::TypedPatternKind::OptionalNull
        | nia_body_ir::TypedPatternKind::CheckedInt { .. }
        | nia_body_ir::TypedPatternKind::CheckedIntRange { .. } => {}
    }
}

fn collect_place_closures<'a>(
    place: &'a TypedPlace,
    callables: &mut HashMap<CallableKey, CallableBody<'a>>,
) {
    if let PlaceBase::Deref(expr) = &place.base {
        collect_expr_closures(expr, callables);
    }
    for elem in &place.elems {
        if let PlaceElem::Index(expr) = elem {
            collect_expr_closures(expr, callables);
        }
    }
}

fn collect_callee_closures<'a>(
    callee: &'a TypedCallee,
    callables: &mut HashMap<CallableKey, CallableBody<'a>>,
) {
    match callee {
        TypedCallee::Closure(expr)
        | TypedCallee::Callable(expr)
        | TypedCallee::FunctionPointer(expr) => collect_expr_closures(expr, callables),
        TypedCallee::Method { receiver, .. }
        | TypedCallee::TraitMethod { receiver, .. }
        | TypedCallee::DynamicTraitMethod { receiver, .. }
        | TypedCallee::BuiltinMethod { receiver, .. }
        | TypedCallee::BuiltinPlaceMethod(nia_body_ir::BuiltinPlaceMethod { receiver, .. }) => {
            collect_expr_closures(receiver, callables)
        }
        TypedCallee::Function(_)
        | TypedCallee::FunctionInstance { .. }
        | TypedCallee::TraitAssociatedFunction { .. }
        | TypedCallee::BuiltinOperator(_) => {}
    }
}
