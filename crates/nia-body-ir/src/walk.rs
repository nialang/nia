// SPDX-License-Identifier: GPL-3.0-or-later
//! Structural walks over typed body IR.

use crate::{
    PlaceBase, PlaceElem, TypedArrayElements, TypedAtomic, TypedBody, TypedCallee, TypedExpr,
    TypedExprKind, TypedMatchArmBody, TypedMemoryIntrinsicSource, TypedPattern, TypedPatternKind,
    TypedPlace, TypedStmt, TypedStmtKind,
};

/// Visits every lexical body belonging to one typed function in preorder.
///
/// Nested bodies are not limited to statement blocks: match arms, patterns,
/// call arguments, place indices, inline assembly operands, and all other
/// expression containers are traversed. Closure bodies are function boundaries
/// and are deliberately excluded, although their capture expressions still
/// belong to the enclosing function and are traversed. Consumers that build
/// flat per-function tables should use this walk instead of maintaining a
/// partial list of expression forms that happen to contain bodies today.
pub fn walk_typed_function_bodies<'a>(body: &'a TypedBody, visit: &mut impl FnMut(&'a TypedBody)) {
    visit(body);
    for stmt in &body.stmts {
        walk_stmt(stmt, visit);
    }
    if let Some(tail) = &body.tail {
        walk_expr(tail, visit);
    }
}

fn walk_stmt<'a>(stmt: &'a TypedStmt, visit: &mut impl FnMut(&'a TypedBody)) {
    match &stmt.kind {
        TypedStmtKind::Binding(binding) => {
            if let Some(value) = &binding.value {
                walk_expr(value, visit);
            }
        }
        TypedStmtKind::PatternBinding(binding) => {
            walk_pattern(&binding.pattern, visit);
            walk_expr(&binding.value, visit);
        }
        TypedStmtKind::Expr(expr)
        | TypedStmtKind::Return(Some(expr))
        | TypedStmtKind::Defer(expr) => walk_expr(expr, visit),
        TypedStmtKind::ForIn(for_in) => {
            walk_pattern(&for_in.pattern, visit);
            walk_expr(&for_in.iter, visit);
            walk_typed_function_bodies(&for_in.body, visit);
        }
        TypedStmtKind::While(while_stmt) => {
            walk_expr(&while_stmt.cond, visit);
            walk_typed_function_bodies(&while_stmt.body, visit);
        }
        TypedStmtKind::Loop(loop_stmt) => walk_typed_function_bodies(&loop_stmt.body, visit),
        TypedStmtKind::Return(None) | TypedStmtKind::Break | TypedStmtKind::Continue => {}
    }
}

fn walk_expr<'a>(expr: &'a TypedExpr, visit: &mut impl FnMut(&'a TypedBody)) {
    match &expr.kind {
        TypedExprKind::Closure { captures, .. } => {
            for capture in captures {
                walk_expr(&capture.value, visit);
            }
        }
        TypedExprKind::EnumVariant { fields, .. } | TypedExprKind::Tuple(fields) => {
            for field in fields {
                walk_expr(field, visit);
            }
        }
        TypedExprKind::Range(range) => {
            for bound in range.start.iter().chain(&range.end) {
                walk_expr(bound, visit);
            }
        }
        TypedExprKind::InlineAsm(asm) => {
            for input in &asm.inputs {
                walk_expr(&input.value, visit);
            }
            for output in &asm.outputs {
                walk_place(&output.place, visit);
            }
        }
        TypedExprKind::MemoryIntrinsic(intrinsic) => {
            walk_expr(&intrinsic.dest, visit);
            match &intrinsic.source {
                TypedMemoryIntrinsicSource::Slice(source)
                | TypedMemoryIntrinsicSource::Byte(source) => walk_expr(source, visit),
            }
        }
        TypedExprKind::Atomic(atomic) => match atomic {
            TypedAtomic::Load { ptr, .. } => walk_expr(ptr, visit),
            TypedAtomic::Store { ptr, value, .. } | TypedAtomic::Rmw { ptr, value, .. } => {
                walk_expr(ptr, visit);
                walk_expr(value, visit);
            }
            TypedAtomic::Cmpxchg {
                ptr,
                expected,
                desired,
                ..
            } => {
                walk_expr(ptr, visit);
                walk_expr(expected, visit);
                walk_expr(desired, visit);
            }
            TypedAtomic::Fence { .. } => {}
        },
        TypedExprKind::LoadUnaligned { ptr: inner, .. }
        | TypedExprKind::Splat { value: inner }
        | TypedExprKind::Bitmask { vector: inner }
        | TypedExprKind::BitIntrinsic { value: inner, .. }
        | TypedExprKind::CharFromU32 { value: inner }
        | TypedExprKind::StaticArrayPointer { array: inner, .. }
        | TypedExprKind::OptionalSome { expr: inner }
        | TypedExprKind::ErrorOk { expr: inner }
        | TypedExprKind::ErrorErr { expr: inner }
        | TypedExprKind::Try { expr: inner, .. }
        | TypedExprKind::Discard(inner)
        | TypedExprKind::Cast { expr: inner, .. }
        | TypedExprKind::TraitObjectUpcast { expr: inner, .. }
        | TypedExprKind::TraitObjectCoercion { expr: inner, .. }
        | TypedExprKind::CallableCoercion { state: inner, .. }
        | TypedExprKind::Unary { expr: inner, .. }
        | TypedExprKind::Field { lhs: inner, .. }
        | TypedExprKind::TupleField { lhs: inner, .. } => walk_expr(inner, visit),
        TypedExprKind::ExtractElement { vector, index }
        | TypedExprKind::Binary {
            lhs: vector,
            rhs: index,
            ..
        }
        | TypedExprKind::Index { lhs: vector, index } => {
            walk_expr(vector, visit);
            walk_expr(index, visit);
        }
        TypedExprKind::InsertElement {
            vector,
            index,
            value,
        } => {
            walk_expr(vector, visit);
            walk_expr(index, visit);
            walk_expr(value, visit);
        }
        TypedExprKind::ArrayLiteral { elems } => match elems {
            TypedArrayElements::List(elems) => {
                for elem in elems {
                    walk_expr(elem, visit);
                }
            }
            TypedArrayElements::Repeat { value, .. } => walk_expr(value, visit),
        },
        TypedExprKind::StructLiteral { fields, .. } => {
            for field in fields {
                walk_expr(&field.value, visit);
            }
        }
        TypedExprKind::UnionLiteral { field, .. } => walk_expr(&field.value, visit),
        TypedExprKind::UnionStorageLiteral { relocations, .. } => {
            for relocation in relocations {
                walk_expr(&relocation.pointee, visit);
            }
        }
        TypedExprKind::Assign { place, rhs, .. } => {
            walk_place(place, visit);
            walk_expr(rhs, visit);
        }
        TypedExprKind::Call { callee, args } => {
            walk_callee(callee, visit);
            for arg in args {
                walk_expr(arg, visit);
            }
        }
        TypedExprKind::Slice { lhs, range, .. } => {
            walk_expr(lhs, visit);
            for bound in range.start.iter().chain(&range.end) {
                walk_expr(bound, visit);
            }
        }
        TypedExprKind::Block(body) => walk_typed_function_bodies(body, visit),
        TypedExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            walk_expr(cond, visit);
            walk_typed_function_bodies(then_branch, visit);
            if let Some(branch) = else_branch {
                walk_expr(branch, visit);
            }
        }
        TypedExprKind::IfPattern(pattern) => {
            walk_expr(&pattern.target, visit);
            walk_pattern(&pattern.pattern, visit);
            walk_typed_function_bodies(&pattern.then_branch, visit);
            if let Some(branch) = &pattern.else_branch {
                walk_expr(branch, visit);
            }
        }
        TypedExprKind::Match(matched) => {
            walk_expr(&matched.target, visit);
            for arm in &matched.arms {
                for pattern in &arm.patterns {
                    walk_pattern(pattern, visit);
                }
                match &arm.body {
                    TypedMatchArmBody::Expr(expr) => walk_expr(expr, visit),
                    TypedMatchArmBody::Stmt(stmt) => walk_stmt(stmt, visit),
                    TypedMatchArmBody::Block(body) => walk_typed_function_bodies(body, visit),
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

fn walk_pattern<'a>(pattern: &'a TypedPattern, visit: &mut impl FnMut(&'a TypedBody)) {
    match &pattern.kind {
        TypedPatternKind::Pointer(inner)
        | TypedPatternKind::MutPointer(inner)
        | TypedPatternKind::OptionalSome(inner)
        | TypedPatternKind::ErrorOk(inner)
        | TypedPatternKind::ErrorErr(inner) => walk_pattern(inner, visit),
        TypedPatternKind::Tuple(patterns)
        | TypedPatternKind::Nominal {
            fields: patterns, ..
        } => {
            for pattern in patterns {
                walk_pattern(pattern, visit);
            }
        }
        TypedPatternKind::Expr(expr) => walk_expr(expr, visit),
        TypedPatternKind::Range { start, end, .. } => {
            walk_expr(start, visit);
            walk_expr(end, visit);
        }
        TypedPatternKind::Wildcard
        | TypedPatternKind::Bind { .. }
        | TypedPatternKind::OptionalNull
        | TypedPatternKind::CheckedInt { .. }
        | TypedPatternKind::CheckedIntRange { .. } => {}
    }
}

fn walk_place<'a>(place: &'a TypedPlace, visit: &mut impl FnMut(&'a TypedBody)) {
    if let PlaceBase::Deref(expr) = &place.base {
        walk_expr(expr, visit);
    }
    for elem in &place.elems {
        if let PlaceElem::Index(expr) = elem {
            walk_expr(expr, visit);
        }
    }
}

fn walk_callee<'a>(callee: &'a TypedCallee, visit: &mut impl FnMut(&'a TypedBody)) {
    match callee {
        TypedCallee::Closure(expr)
        | TypedCallee::Callable(expr)
        | TypedCallee::FunctionPointer(expr) => walk_expr(expr, visit),
        TypedCallee::Method { receiver, .. }
        | TypedCallee::TraitMethod { receiver, .. }
        | TypedCallee::DynamicTraitMethod { receiver, .. }
        | TypedCallee::BuiltinMethod { receiver, .. }
        | TypedCallee::BuiltinPlaceMethod(crate::BuiltinPlaceMethod { receiver, .. }) => {
            walk_expr(receiver, visit)
        }
        TypedCallee::Function(_)
        | TypedCallee::FunctionInstance { .. }
        | TypedCallee::TraitAssociatedFunction { .. }
        | TypedCallee::BuiltinOperator(_) => {}
    }
}
