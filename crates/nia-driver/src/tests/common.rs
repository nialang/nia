// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::BinaryOp;
use nia_body_ir::{TypedBody, TypedCallee, TypedExpr, TypedExprKind, TypedStmtKind};
use nia_function_ir::{
    FunctionCallee, FunctionExpr, FunctionExprKind, FunctionMemoryIntrinsicSource, FunctionOp,
    FunctionTerminator,
};
use nia_imports::ModuleMap;
use nia_opt::NiaOptimizationLevel;
use nia_symbol::{SymbolId, stable_hash};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

static TEMP_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(super) fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

pub(super) fn temp_dir(name: &str) -> PathBuf {
    loop {
        let id = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "nia-driver-{name}-{}-{:?}-{id}",
            std::process::id(),
            std::thread::current().id()
        ));
        match fs::create_dir(&dir) {
            Ok(()) => return dir,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => panic!("create temp dir {}: {error}", dir.display()),
        }
    }
}

pub(super) fn write(path: &Path, source: &str) {
    fs::write(path, source).expect("write source file");
}

pub(super) fn assert_no_error_diagnostics(diagnostics: &[crate::ProgramDiagnostic]) {
    assert!(
        !nia_compiler_query::has_error_diagnostics(diagnostics),
        "{diagnostics:?}"
    );
}

pub(super) fn test_toolchain_layout() -> Arc<crate::ToolchainLayout> {
    static LAYOUT: OnceLock<Arc<crate::ToolchainLayout>> = OnceLock::new();
    Arc::clone(LAYOUT.get_or_init(|| {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir
            .parent()
            .and_then(Path::parent)
            .expect("nia-driver lives under crates/");
        Arc::new(
            crate::ToolchainLayout::resolve(crate::ToolchainLayoutRequest::explicit(
                std::env::current_exe().expect("test executable path"),
                workspace_root.join("lib"),
            ))
            .expect("development toolchain layout"),
        )
    }))
}

pub(super) fn test_driver() -> crate::Driver {
    crate::Driver::new(test_toolchain_layout())
}

pub(super) fn check_program(
    entry_path: impl Into<String>,
) -> nia_compiler_query::CheckedProgramAnalysis {
    check_program_with_options(entry_path, NiaOptimizationLevel::default())
}

pub(super) fn check_program_with_options(
    entry_path: impl Into<String>,
    optimization: NiaOptimizationLevel,
) -> nia_compiler_query::CheckedProgramAnalysis {
    test_driver()
        .analyze_all_modules(crate::CheckRequest::new(entry_path).with_optimization(optimization))
}

pub(super) fn check_entry_program(
    entry_path: impl Into<String>,
) -> nia_compiler_query::CheckedProgramAnalysis {
    test_driver().analyze_entry_program(crate::CheckRequest::new(entry_path))
}

pub(super) fn codegen_program(entry_path: impl Into<String>) -> crate::CodegenProgram {
    codegen_program_with_options(entry_path, NiaOptimizationLevel::default())
}

pub(super) fn codegen_program_with_options(
    entry_path: impl Into<String>,
    optimization: NiaOptimizationLevel,
) -> crate::CodegenProgram {
    codegen_program_from_output(
        test_driver().codegen(crate::CheckRequest::new(entry_path).with_optimization(optimization)),
    )
}

pub(super) fn check_program_with_map(
    entry_path: impl Into<String>,
    module_map: ModuleMap,
) -> nia_compiler_query::CheckedProgramAnalysis {
    test_driver()
        .analyze_all_modules(crate::CheckRequest::new(entry_path).with_module_map(module_map))
}

pub(super) fn check_freestanding_executable_with_options(
    entry_path: impl Into<String>,
    optimization: NiaOptimizationLevel,
) -> nia_compiler_query::CheckedProgramAnalysis {
    test_driver().analyze_all_modules(
        crate::CheckRequest::new(entry_path)
            .with_optimization(optimization)
            .with_runtime(crate::Runtime::Freestanding),
    )
}

pub(super) fn check_freestanding_executable_with_map_and_options(
    entry_path: impl Into<String>,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
) -> nia_compiler_query::CheckedProgramAnalysis {
    test_driver().analyze_all_modules(
        crate::CheckRequest::new(entry_path)
            .with_module_map(module_map)
            .with_optimization(optimization)
            .with_runtime(crate::Runtime::Freestanding),
    )
}

pub(super) fn checked_program_from_output(
    output: crate::DriverOutput<crate::CheckedProgram>,
) -> crate::CheckedProgram {
    match output.result {
        Ok(program) | Err(crate::DriverError::CheckDiagnostics(program)) => program,
        Err(error) => panic!("unexpected driver error: {error:?}"),
    }
}

pub(super) fn codegen_program_from_output(
    output: crate::DriverOutput<crate::CodegenProgram>,
) -> crate::CodegenProgram {
    match output.result {
        Ok(program) => program,
        Err(crate::DriverError::CodegenProgramDiagnostics(program)) => *program,
        Err(error) => panic!("unexpected driver error: {error:?}"),
    }
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
        FunctionTerminator::Try {
            value,
            error_conversion,
            ..
        } => {
            function_expr_contains_builtin_eq(value)
                || error_conversion
                    .as_deref()
                    .is_some_and(function_expr_contains_builtin_eq)
        }
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
        FunctionExprKind::FunctionCallable { function } => {
            function_expr_contains_builtin_eq(function)
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
        | FunctionExprKind::TraitObjectUpcast { expr, .. }
        | FunctionExprKind::TraitObjectCoercion { expr, .. }
        | FunctionExprKind::CallableCoercion { state: expr, .. }
        | FunctionExprKind::RangeBound { range: expr, .. }
        | FunctionExprKind::LoadUnaligned { ptr: expr, .. }
        | FunctionExprKind::Splat { value: expr }
        | FunctionExprKind::Bitmask { vector: expr }
        | FunctionExprKind::BitIntrinsic { value: expr, .. }
        | FunctionExprKind::CharFromU32 { value: expr }
        | FunctionExprKind::StaticArrayPointer { array: expr, .. } => {
            function_expr_contains_builtin_eq(expr)
        }
        FunctionExprKind::Binary { lhs, rhs, .. } | FunctionExprKind::Index { lhs, index: rhs } => {
            function_expr_contains_builtin_eq(lhs) || function_expr_contains_builtin_eq(rhs)
        }
        FunctionExprKind::ExtractElement { vector, index } => {
            function_expr_contains_builtin_eq(vector) || function_expr_contains_builtin_eq(index)
        }
        FunctionExprKind::InsertElement {
            vector,
            index,
            value,
        } => {
            function_expr_contains_builtin_eq(vector)
                || function_expr_contains_builtin_eq(index)
                || function_expr_contains_builtin_eq(value)
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
        FunctionExprKind::Tuple(elems) => elems.iter().any(function_expr_contains_builtin_eq),
        FunctionExprKind::TupleField { value, .. } => function_expr_contains_builtin_eq(value),
        FunctionExprKind::StructLiteral { fields, .. } => fields
            .iter()
            .any(|field| function_expr_contains_builtin_eq(&field.value)),
        FunctionExprKind::EnumVariant { fields, .. } => {
            fields.iter().any(function_expr_contains_builtin_eq)
        }
        FunctionExprKind::EnumTag { value } | FunctionExprKind::EnumPayloadField { value, .. } => {
            function_expr_contains_builtin_eq(value)
        }
        FunctionExprKind::UnionLiteral { field, .. } => {
            function_expr_contains_builtin_eq(&field.value)
        }
        FunctionExprKind::InlineAsm(asm) => asm
            .inputs
            .iter()
            .any(|input| function_expr_contains_builtin_eq(&input.value)),
        FunctionExprKind::Atomic(atomic) => match atomic {
            nia_function_ir::FunctionAtomic::Load { ptr, .. } => {
                function_expr_contains_builtin_eq(ptr)
            }
            nia_function_ir::FunctionAtomic::Store { ptr, value, .. }
            | nia_function_ir::FunctionAtomic::Rmw { ptr, value, .. } => {
                function_expr_contains_builtin_eq(ptr) || function_expr_contains_builtin_eq(value)
            }
            nia_function_ir::FunctionAtomic::Cmpxchg {
                ptr,
                expected,
                desired,
                ..
            } => {
                function_expr_contains_builtin_eq(ptr)
                    || function_expr_contains_builtin_eq(expected)
                    || function_expr_contains_builtin_eq(desired)
            }
            nia_function_ir::FunctionAtomic::Fence { .. } => false,
        },
        FunctionExprKind::Error
        | FunctionExprKind::Trap
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
        | FunctionExprKind::EnumConstructor(_)
        | FunctionExprKind::FunctionInstance { .. }
        | FunctionExprKind::CallerLocation(_)
        | FunctionExprKind::ClosureFunctionPointer { .. }
        | FunctionExprKind::EnumVariantTag(_)
        | FunctionExprKind::BuiltinValue(_) => false,
        FunctionExprKind::UnionStorageLiteral { relocations, .. } => relocations
            .iter()
            .any(|relocation| function_expr_contains_builtin_eq(&relocation.pointee)),
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
            expr_contains_dynamic_trait_callee(&for_stmt.iter)
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
        | TypedExprKind::StaticArrayPointer { array: expr, .. } => {
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
        | nia_function_ir::FunctionPlaceBase::GlobalInstance { .. }
        | nia_function_ir::FunctionPlaceBase::Error => false,
    };
    base_contains
        || place.elems.iter().any(|elem| match elem {
            nia_function_ir::FunctionPlaceElem::Index(expr) => {
                function_expr_contains_builtin_eq(expr)
            }
            nia_function_ir::FunctionPlaceElem::Field(_)
            | nia_function_ir::FunctionPlaceElem::TupleField(_)
            | nia_function_ir::FunctionPlaceElem::Error => false,
        })
}
