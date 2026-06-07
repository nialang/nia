// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::BinaryOp;
use nia_body_ir::{
    TypedBody, TypedCallee, TypedExpr, TypedExprKind, TypedForIterator, TypedStmtKind,
};
use nia_function_ir::{
    FunctionCallee, FunctionExpr, FunctionExprKind, FunctionMemoryIntrinsicSource, FunctionOp,
    FunctionTerminator,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

static TEMP_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(super) fn temp_dir(name: &str) -> PathBuf {
    let id = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "nia-driver-{name}-{}-{:?}-{id}",
        std::process::id(),
        std::thread::current().id()
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

pub(super) fn write(path: &Path, source: &str) {
    fs::write(path, source).expect("write source file");
}

pub(super) fn function_body_contains_builtin_eq(body: &nia_function_ir::FunctionBody) -> bool {
    body.blocks.iter().any(|block| {
        block.ops.iter().any(function_op_contains_builtin_eq)
            || function_terminator_contains_builtin_eq(&block.terminator)
    })
}

fn function_op_contains_builtin_eq(op: &FunctionOp) -> bool {
    match op {
        FunctionOp::Binding(binding) => binding
            .value
            .as_ref()
            .is_some_and(function_expr_contains_builtin_eq),
        FunctionOp::StoreLocal { value, .. } | FunctionOp::Expr(value) => {
            function_expr_contains_builtin_eq(value)
        }
        FunctionOp::MemoryIntrinsic(memory) => {
            function_expr_contains_builtin_eq(&memory.dest)
                || match &memory.source {
                    FunctionMemoryIntrinsicSource::Slice(source)
                    | FunctionMemoryIntrinsicSource::Byte(source) => {
                        function_expr_contains_builtin_eq(source)
                    }
                }
        }
        FunctionOp::Defer(body) => body.blocks.iter().any(|block| {
            block.ops.iter().any(function_op_contains_builtin_eq)
                || function_terminator_contains_builtin_eq(&block.terminator)
        }),
    }
}

fn function_terminator_contains_builtin_eq(terminator: &FunctionTerminator) -> bool {
    match terminator {
        FunctionTerminator::If { cond, .. } | FunctionTerminator::Switch { target: cond, .. } => {
            function_expr_contains_builtin_eq(cond)
        }
        FunctionTerminator::Try { value, .. } => function_expr_contains_builtin_eq(value),
        FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => value
            .as_ref()
            .is_some_and(function_expr_contains_builtin_eq),
        FunctionTerminator::Error { .. }
        | FunctionTerminator::Branch { .. }
        | FunctionTerminator::Next { .. }
        | FunctionTerminator::Loop { .. } => false,
    }
}

fn function_expr_contains_builtin_eq(expr: &FunctionExpr) -> bool {
    match &expr.kind {
        FunctionExprKind::Call {
            callee: FunctionCallee::BuiltinOperator(operator),
            args,
        } => {
            (operator.trait_id == nia_ty::BuiltinTrait::Eq
                && operator.op == nia_function_ir::FunctionBuiltinOperatorOp::Binary(BinaryOp::Eq))
                || args.iter().any(function_expr_contains_builtin_eq)
        }
        FunctionExprKind::Call { args, .. } => args.iter().any(function_expr_contains_builtin_eq),
        FunctionExprKind::Unary { expr, .. }
        | FunctionExprKind::OptionalSome { expr }
        | FunctionExprKind::ErrorOk { expr }
        | FunctionExprKind::ErrorErr { expr }
        | FunctionExprKind::TaggedUnionTag { expr }
        | FunctionExprKind::TaggedUnionPayload { expr }
        | FunctionExprKind::Try { expr }
        | FunctionExprKind::Discard(expr)
        | FunctionExprKind::Cast { expr, .. }
        | FunctionExprKind::TraitObjectUpcast { expr, .. }
        | FunctionExprKind::TraitObjectCoercion { expr, .. }
        | FunctionExprKind::RangeBound { range: expr, .. }
        | FunctionExprKind::CStringPointer { array: expr, .. } => {
            function_expr_contains_builtin_eq(expr)
        }
        FunctionExprKind::Binary { lhs, rhs, .. } | FunctionExprKind::Index { lhs, index: rhs } => {
            function_expr_contains_builtin_eq(lhs) || function_expr_contains_builtin_eq(rhs)
        }
        FunctionExprKind::Assign { place, rhs, .. } => {
            function_place_contains_builtin_eq(place) || function_expr_contains_builtin_eq(rhs)
        }
        FunctionExprKind::AddrOf(place) => function_place_contains_builtin_eq(place),
        FunctionExprKind::Field { lhs, .. } => function_expr_contains_builtin_eq(lhs),
        FunctionExprKind::Slice { lhs, range, .. } => {
            function_expr_contains_builtin_eq(lhs)
                || range
                    .start
                    .as_deref()
                    .is_some_and(function_expr_contains_builtin_eq)
                || range
                    .end
                    .as_deref()
                    .is_some_and(function_expr_contains_builtin_eq)
        }
        FunctionExprKind::Range(range) => {
            range
                .start
                .as_deref()
                .is_some_and(function_expr_contains_builtin_eq)
                || range
                    .end
                    .as_deref()
                    .is_some_and(function_expr_contains_builtin_eq)
        }
        FunctionExprKind::ArrayLiteral { elems } => match elems {
            nia_function_ir::FunctionArrayElements::List(elems) => {
                elems.iter().any(function_expr_contains_builtin_eq)
            }
            nia_function_ir::FunctionArrayElements::Repeat { value, .. } => {
                function_expr_contains_builtin_eq(value)
            }
        },
        FunctionExprKind::StructLiteral { fields, .. } => fields
            .iter()
            .any(|field| function_expr_contains_builtin_eq(&field.value)),
        FunctionExprKind::UnionLiteral { field, .. } => {
            function_expr_contains_builtin_eq(&field.value)
        }
        FunctionExprKind::InlineAsm(asm) => asm
            .inputs
            .iter()
            .any(|input| function_expr_contains_builtin_eq(&input.value)),
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
        | FunctionExprKind::BuiltinValue(_) => false,
    }
}

pub(super) fn body_contains_dynamic_trait_callee(body: &TypedBody) -> bool {
    body.stmts.iter().any(|stmt| match &stmt.kind {
        TypedStmtKind::Binding(binding) => binding
            .value
            .as_ref()
            .is_some_and(expr_contains_dynamic_trait_callee),
        TypedStmtKind::Expr(expr)
        | TypedStmtKind::Return(Some(expr))
        | TypedStmtKind::Defer(expr) => expr_contains_dynamic_trait_callee(expr),
        TypedStmtKind::ForIn(for_stmt) => {
            for_iterator_contains_dynamic_trait_callee(&for_stmt.iter)
                || body_contains_dynamic_trait_callee(&for_stmt.body)
        }
        TypedStmtKind::While(while_stmt) => {
            expr_contains_dynamic_trait_callee(&while_stmt.cond)
                || body_contains_dynamic_trait_callee(&while_stmt.body)
        }
        TypedStmtKind::Loop(loop_stmt) => body_contains_dynamic_trait_callee(&loop_stmt.body),
        _ => false,
    }) || body
        .tail
        .as_ref()
        .is_some_and(|tail| expr_contains_dynamic_trait_callee(tail))
}

fn for_iterator_contains_dynamic_trait_callee(iter: &TypedForIterator) -> bool {
    match iter {
        TypedForIterator::Range(range) => expr_contains_dynamic_trait_callee(&range.expr),
        TypedForIterator::Expr(expr) => expr_contains_dynamic_trait_callee(expr),
    }
}

fn expr_contains_dynamic_trait_callee(expr: &TypedExpr) -> bool {
    match &expr.kind {
        TypedExprKind::Call {
            callee: TypedCallee::DynamicTraitMethod { .. },
            ..
        } => true,
        TypedExprKind::Call { args, .. } => args.iter().any(expr_contains_dynamic_trait_callee),
        TypedExprKind::Unary { expr, .. }
        | TypedExprKind::Discard(expr)
        | TypedExprKind::Cast { expr, .. }
        | TypedExprKind::TraitObjectUpcast { expr, .. }
        | TypedExprKind::TraitObjectCoercion { expr, .. }
        | TypedExprKind::CStringPointer { array: expr, .. } => {
            expr_contains_dynamic_trait_callee(expr)
        }
        TypedExprKind::Binary { lhs, rhs, .. } | TypedExprKind::Index { lhs, index: rhs } => {
            expr_contains_dynamic_trait_callee(lhs) || expr_contains_dynamic_trait_callee(rhs)
        }
        TypedExprKind::Assign { rhs, .. } => expr_contains_dynamic_trait_callee(rhs),
        TypedExprKind::Field { lhs, .. } | TypedExprKind::Slice { lhs, .. } => {
            expr_contains_dynamic_trait_callee(lhs)
        }
        TypedExprKind::Block(body) => body_contains_dynamic_trait_callee(body),
        TypedExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            expr_contains_dynamic_trait_callee(cond)
                || body_contains_dynamic_trait_callee(then_branch)
                || else_branch
                    .as_ref()
                    .is_some_and(|else_branch| expr_contains_dynamic_trait_callee(else_branch))
        }
        _ => false,
    }
}

fn function_place_contains_builtin_eq(place: &nia_function_ir::FunctionPlace) -> bool {
    let base_contains = match &place.base {
        nia_function_ir::FunctionPlaceBase::Deref(expr) => function_expr_contains_builtin_eq(expr),
        nia_function_ir::FunctionPlaceBase::Local(_)
        | nia_function_ir::FunctionPlaceBase::Global(_)
        | nia_function_ir::FunctionPlaceBase::Error => false,
    };
    base_contains
        || place.elems.iter().any(|elem| match elem {
            nia_function_ir::FunctionPlaceElem::Index(expr) => {
                function_expr_contains_builtin_eq(expr)
            }
            nia_function_ir::FunctionPlaceElem::Field(_)
            | nia_function_ir::FunctionPlaceElem::Error => false,
        })
}
