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

#[cfg(test)]
mod tests {
    use super::walk_typed_function_bodies;
    use crate::{
        AtomicOrder, MemoryIntrinsicOp, PlaceBase, PlaceElem, TypedAsmInput, TypedAsmOutput,
        TypedAtomic, TypedCallee, TypedClosureCapture, TypedExpr, TypedExprKind, TypedForIn,
        TypedIfPattern, TypedInlineAsm, TypedLoop, TypedMatch, TypedMatchArm, TypedMatchArmBody,
        TypedMemoryIntrinsic, TypedMemoryIntrinsicSource, TypedPattern, TypedPatternKind,
        TypedPlace, TypedStmt, TypedStmtKind, TypedUnionRelocation, TypedWhile,
    };
    use nia_ids::{
        ClosureId, DefId, GlobalDefId, InternedTyId, LocalId, ModuleId, ModuleIdAllocator,
    };
    use nia_span::Span;
    use nia_ty::{PrimitiveTy, TypeStore};

    struct Fixture {
        _types: TypeStore,
        module_id: ModuleId,
        ty: InternedTyId,
    }

    impl Fixture {
        fn new() -> Self {
            let types = TypeStore::new();
            let module_id = ModuleIdAllocator::new().allocate();
            let ty = types
                .append_for_module(module_id)
                .primitive(PrimitiveTy::Bool);
            Self {
                _types: types,
                module_id,
                ty,
            }
        }

        fn body(&self, marker: usize) -> crate::TypedBody {
            crate::TypedBody {
                span: Span::new(marker, marker + 1),
                locals: Vec::new(),
                stmts: Vec::new(),
                tail: None,
                ty: self.ty,
            }
        }

        fn expr(&self, kind: TypedExprKind) -> TypedExpr {
            TypedExpr {
                span: Span::default(),
                ty: self.ty,
                kind,
            }
        }

        fn block(&self, marker: usize) -> TypedExpr {
            self.expr(TypedExprKind::Block(self.body(marker)))
        }

        fn pattern(&self, kind: TypedPatternKind) -> TypedPattern {
            TypedPattern {
                ty: self.ty,
                span: Span::default(),
                kind,
            }
        }

        fn visited_markers(&self, root: &crate::TypedBody) -> Vec<usize> {
            let mut markers = Vec::new();
            walk_typed_function_bodies(root, &mut |body| markers.push(body.span.start));
            markers
        }
    }

    #[test]
    fn visits_loop_and_conditional_bodies_in_preorder() {
        let fixture = Fixture::new();
        let mut then_branch = fixture.body(40);
        then_branch.tail = Some(Box::new(fixture.block(41)));

        let mut root = fixture.body(0);
        root.stmts = vec![
            TypedStmt {
                span: Span::default(),
                kind: TypedStmtKind::ForIn(Box::new(TypedForIn {
                    pattern: fixture.pattern(TypedPatternKind::Wildcard),
                    item_ty: fixture.ty,
                    bool_ty: fixture.ty,
                    iterable_self_ty: fixture.ty,
                    iterator_ty: fixture.ty,
                    iter: fixture.expr(TypedExprKind::Bool(true)),
                    body: fixture.body(10),
                })),
            },
            TypedStmt {
                span: Span::default(),
                kind: TypedStmtKind::While(Box::new(TypedWhile {
                    cond: fixture.expr(TypedExprKind::Bool(true)),
                    body: fixture.body(20),
                })),
            },
            TypedStmt {
                span: Span::default(),
                kind: TypedStmtKind::Loop(Box::new(TypedLoop {
                    body: fixture.body(30),
                })),
            },
        ];
        root.tail = Some(Box::new(fixture.expr(TypedExprKind::If {
            cond: Box::new(fixture.expr(TypedExprKind::Bool(true))),
            then_branch,
            else_branch: Some(Box::new(fixture.block(42))),
        })));

        assert_eq!(
            fixture.visited_markers(&root),
            vec![0, 10, 20, 30, 40, 41, 42]
        );
    }

    #[test]
    fn visits_pattern_expressions_and_every_match_arm_body_form() {
        let fixture = Fixture::new();
        let match_expr = fixture.expr(TypedExprKind::Match(Box::new(TypedMatch {
            target: fixture.block(60),
            bool_ty: fixture.ty,
            arms: vec![
                TypedMatchArm {
                    patterns: vec![fixture.pattern(TypedPatternKind::Expr(fixture.block(70)))],
                    body: TypedMatchArmBody::Expr(Box::new(fixture.block(80))),
                    span: Span::default(),
                },
                TypedMatchArm {
                    patterns: vec![fixture.pattern(TypedPatternKind::Range {
                        start: Box::new(fixture.block(71)),
                        end: Box::new(fixture.block(72)),
                        inclusive: true,
                    })],
                    body: TypedMatchArmBody::Stmt(Box::new(TypedStmt {
                        span: Span::default(),
                        kind: TypedStmtKind::Loop(Box::new(TypedLoop {
                            body: fixture.body(90),
                        })),
                    })),
                    span: Span::default(),
                },
                TypedMatchArm {
                    patterns: vec![fixture.pattern(TypedPatternKind::Wildcard)],
                    body: TypedMatchArmBody::Block(Box::new(fixture.body(100))),
                    span: Span::default(),
                },
            ],
        })));
        let mut root = fixture.body(0);
        root.tail = Some(Box::new(fixture.expr(TypedExprKind::IfPattern(Box::new(
            TypedIfPattern {
                target: fixture.block(10),
                bool_ty: fixture.ty,
                pattern: fixture.pattern(TypedPatternKind::Tuple(vec![
                    fixture.pattern(TypedPatternKind::Expr(fixture.block(20))),
                    fixture.pattern(TypedPatternKind::Range {
                        start: Box::new(fixture.block(30)),
                        end: Box::new(fixture.block(40)),
                        inclusive: false,
                    }),
                ])),
                then_branch: fixture.body(50),
                else_branch: Some(Box::new(match_expr)),
            },
        )))));

        assert_eq!(
            fixture.visited_markers(&root),
            vec![0, 10, 20, 30, 40, 50, 60, 70, 80, 71, 72, 90, 100]
        );
    }

    #[test]
    fn visits_closure_captures_but_stops_at_the_closure_body() {
        let fixture = Fixture::new();
        let owner = GlobalDefId {
            module_id: fixture.module_id,
            def_id: DefId(0),
        };
        let mut root = fixture.body(0);
        root.tail = Some(Box::new(fixture.expr(TypedExprKind::Closure {
            closure_id: ClosureId { owner, ordinal: 0 },
            captures: vec![TypedClosureCapture {
                local_id: LocalId(0),
                value: fixture.block(10),
            }],
            params: Vec::new(),
            body: fixture.body(99),
        })));

        assert_eq!(fixture.visited_markers(&root), vec![0, 10]);
    }

    #[test]
    fn visits_bodies_hidden_in_places_callees_and_effect_operands() {
        let fixture = Fixture::new();
        let place = |base, elems| TypedPlace {
            span: Span::default(),
            ty: fixture.ty,
            base,
            elems,
        };
        let mut root = fixture.body(0);
        root.tail = Some(Box::new(fixture.expr(TypedExprKind::Tuple(vec![
            fixture.expr(TypedExprKind::Assign {
                place: place(
                    PlaceBase::Deref(Box::new(fixture.block(10))),
                    vec![PlaceElem::Index(Box::new(fixture.block(11)))],
                ),
                op: nia_ast::AssignOp::Assign,
                rhs: Box::new(fixture.expr(TypedExprKind::Bool(true))),
            }),
            fixture.expr(TypedExprKind::Call {
                callee: TypedCallee::FunctionPointer(Box::new(fixture.block(20))),
                args: Vec::new(),
            }),
            fixture.expr(TypedExprKind::InlineAsm(TypedInlineAsm {
                code: String::new(),
                inputs: vec![TypedAsmInput {
                    constraint: String::new(),
                    value: fixture.block(30),
                    span: Span::default(),
                }],
                outputs: vec![TypedAsmOutput {
                    constraint: String::new(),
                    place: place(
                        PlaceBase::Deref(Box::new(fixture.block(31))),
                        vec![PlaceElem::Index(Box::new(fixture.block(32)))],
                    ),
                    span: Span::default(),
                }],
                clobbers: Vec::new(),
                options: Vec::new(),
            })),
            fixture.expr(TypedExprKind::UnionStorageLiteral {
                bytes: Vec::new(),
                relocations: vec![TypedUnionRelocation {
                    offset: 0,
                    width: 0,
                    allocation: crate::PromotedAllocationId::new(
                        fixture.module_id,
                        Span::default(),
                    ),
                    pointee: Box::new(fixture.block(40)),
                }],
            }),
            fixture.expr(TypedExprKind::MemoryIntrinsic(TypedMemoryIntrinsic {
                op: MemoryIntrinsicOp::Copy,
                elem_ty: fixture.ty,
                dest: Box::new(fixture.block(50)),
                source: TypedMemoryIntrinsicSource::Slice(Box::new(fixture.block(51))),
            })),
            fixture.expr(TypedExprKind::Atomic(TypedAtomic::Cmpxchg {
                ty: fixture.ty,
                ptr: Box::new(fixture.block(60)),
                expected: Box::new(fixture.block(61)),
                desired: Box::new(fixture.block(62)),
                success: AtomicOrder::SeqCst,
                failure: AtomicOrder::Acquire,
                weak: false,
            })),
        ]))));

        assert_eq!(
            fixture.visited_markers(&root),
            vec![0, 10, 11, 20, 30, 31, 32, 40, 50, 51, 60, 61, 62]
        );
    }
}
