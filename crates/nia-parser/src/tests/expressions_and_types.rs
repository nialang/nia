// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn parses_function_body_statements_and_expressions() {
    let (module, errors) = parse_module(
        r#"
fn main() i32 {
    var x = 1 + 2 * 3 << 1;
    defer cleanup();
    if x > 3 {
        x = x + 1;
    }
    for i in 0..3 {
        x += i;
        x >>= 1;
    }
    switch x {
        0 => return 1,
        _ => return 0,
    }
    x
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    assert_eq!(body.stmts.len(), 5);
    assert!(matches!(body.stmts[0].kind, StmtKind::Binding(_)));
    assert!(matches!(body.stmts[1].kind, StmtKind::Defer(_)));
    assert!(matches!(body.stmts[2].kind, StmtKind::Expr(_)));
    assert!(matches!(body.stmts[3].kind, StmtKind::ForIn(_)));
    let StmtKind::Expr(expr) = &body.stmts[4].kind else {
        panic!("expected switch");
    };
    let ExprKind::Switch(switch) = &expr.kind else {
        panic!("expected switch expression");
    };
    assert!(matches!(
        &switch.arms[0].body,
        SwitchArmBody::Stmt(boxed) if matches!(boxed.as_ref(), Stmt {
            kind: StmtKind::Return(_),
            ..
        })
    ));
    assert!(body.tail.is_some());
}

#[test]
fn parses_structured_types_and_aggregate_literals() {
    let (module, errors) = parse_module(
        r#"
struct Header {
    bytes: [_]u8,
    callback: &const fn(i32, ...) void,
}

fn make() Header {
    var data: [_]u8 = [0; 8];
    var more: [_]u8 = [1, 2, 3];
    var header: Header = { bytes: data, callback: cb };
    header
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Struct(header) = &module.items[0].kind else {
        panic!("expected struct");
    };
    assert!(matches!(
        header.fields[0].ty.kind,
        TypeKind::Array {
            len: ArrayLen::Infer,
            ..
        }
    ));
    assert!(matches!(
        header.fields[1].ty.kind,
        TypeKind::Pointer { .. } | TypeKind::FunctionPointer { .. }
    ));
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let StmtKind::Binding(binding) = &body.stmts[0].kind else {
        panic!("expected binding");
    };
    assert!(matches!(
        binding.value.as_ref().map(|value| &value.kind),
        Some(ExprKind::ArrayLiteral { .. })
    ));
    let StmtKind::Binding(more) = &body.stmts[1].kind else {
        panic!("expected binding");
    };
    assert!(matches!(
        more.value.as_ref().map(|value| &value.kind),
        Some(ExprKind::ArrayLiteral { .. })
    ));
    let StmtKind::Binding(header) = &body.stmts[2].kind else {
        panic!("expected binding");
    };
    assert!(matches!(
        header.value.as_ref().map(|value| &value.kind),
        Some(ExprKind::StructLiteral { .. })
    ));
    let tail = body.tail.as_ref().expect("expected tail");
    assert!(matches!(tail.kind, ExprKind::Ident(_)));
}

#[test]
fn parses_casts_builtins_and_struct_literals() {
    let (module, errors) = parse_module(
        r#"
struct Pair[T] {
    value: T,
}

fn make(ptr: &const u8, xs: &[_]i32) Pair[i32] {
    var size = @size[Pair[i32]]();
    var addr = ptr as usize;
    var first = xs[0];
    var value = ptr.*;
    { value: (addr + size) as i32 }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let StmtKind::Binding(size) = &body.stmts[0].kind else {
        panic!("expected binding");
    };
    let Some(ExprKind::Call { callee, args }) = size.value.as_ref().map(|value| &value.kind) else {
        panic!("expected builtin call");
    };
    assert!(args.is_empty());
    assert!(matches!(
        callee.kind,
        ExprKind::Builtin {
            type_arg: Some(_),
            ..
        }
    ));
    let StmtKind::Binding(addr) = &body.stmts[1].kind else {
        panic!("expected binding");
    };
    assert!(matches!(
        addr.value.as_ref().map(|value| &value.kind),
        Some(ExprKind::Cast { .. })
    ));
    let StmtKind::Binding(first) = &body.stmts[2].kind else {
        panic!("expected binding");
    };
    assert!(matches!(
        first.value.as_ref().map(|value| &value.kind),
        Some(ExprKind::BracketSuffix { .. })
    ));
    let StmtKind::Binding(value) = &body.stmts[3].kind else {
        panic!("expected binding");
    };
    assert!(matches!(
        value.value.as_ref().map(|value| &value.kind),
        Some(ExprKind::Unary {
            op: UnaryOp::Deref,
            ..
        })
    ));
    let tail = body.tail.as_ref().expect("expected tail");
    assert!(matches!(tail.kind, ExprKind::StructLiteral { .. }));
}

#[test]
fn parses_typed_aggregate_literals() {
    let (module, errors) = parse_module(
        r#"
struct Box[T] {
    value: T,
}

struct Point {
    x: i32,
    y: i32,
}

fn make() i32 {
    var p = Point{x: 1, y: 2};
    var xs = [_]i32[1, 2, 3];
    var boxes = [_]Box[i32][Box { value: 1 }];
    var matrix = [2][2]Box[i32][
        [Box[i32] { value: 1 }, Box[i32] { value: 2 }],
        [Box[i32] { value: 3 }, Box[i32] { value: 4 }],
    ];
    p.x + xs[0]
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[2].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let StmtKind::Binding(point) = &body.stmts[0].kind else {
        panic!("expected point binding");
    };
    assert!(matches!(
        point.value.as_ref().map(|value| &value.kind),
        Some(ExprKind::TypedStructLiteral { .. })
    ));
    let StmtKind::Binding(array) = &body.stmts[1].kind else {
        panic!("expected array binding");
    };
    assert!(matches!(
        array.value.as_ref().map(|value| &value.kind),
        Some(ExprKind::TypedArrayLiteral { .. })
    ));
    let StmtKind::Binding(generic_array) = &body.stmts[2].kind else {
        panic!("expected generic array binding");
    };
    let Some(ExprKind::TypedArrayLiteral { ty, .. }) =
        generic_array.value.as_ref().map(|value| &value.kind)
    else {
        panic!("expected typed generic array literal");
    };
    let TypeKind::Array { elem, .. } = &ty.kind else {
        panic!("expected array type prefix");
    };
    let TypeKind::Path { segments } = &elem.kind else {
        panic!("expected generic element path");
    };
    assert_eq!(segments[0].args.len(), 1);
    let StmtKind::Binding(matrix) = &body.stmts[3].kind else {
        panic!("expected matrix binding");
    };
    let Some(ExprKind::TypedArrayLiteral { ty, .. }) =
        matrix.value.as_ref().map(|value| &value.kind)
    else {
        panic!("expected typed matrix array literal");
    };
    let TypeKind::Array { elem, .. } = &ty.kind else {
        panic!("expected outer array type prefix");
    };
    let TypeKind::Array { elem, .. } = &elem.kind else {
        panic!("expected inner array type prefix");
    };
    let TypeKind::Path { segments } = &elem.kind else {
        panic!("expected generic matrix element path");
    };
    assert_eq!(segments[0].args.len(), 1);
}

#[test]
fn parses_slice_types_and_ranges() {
    let (module, errors) = parse_module(
        r#"
fn take(xs: &const [i32], ys: &[i32]) usize {
    var a = &const xs[..];
    var b = &const xs[0..2];
    var c = &const xs[0..=2];
    var d = &const xs[1..];
    var e = &const xs[..3];
    var f = &const xs[..=4];
    a.len() + b.len() + c.len() + d.len() + e.len() + f.len()
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    assert!(matches!(
        function.params[0].ty.as_ref().map(|ty| &ty.kind),
        Some(TypeKind::Slice { is_const: true, .. })
    ));
    assert!(matches!(
        function.params[1].ty.as_ref().map(|ty| &ty.kind),
        Some(TypeKind::Slice {
            is_const: false,
            ..
        })
    ));
    let body = function.body.as_ref().expect("expected body");
    let StmtKind::Binding(binding) = &body.stmts[0].kind else {
        panic!("expected binding");
    };
    let Some(ExprKind::Unary { expr, .. }) = binding.value.as_ref().map(|value| &value.kind) else {
        panic!("expected slice borrow");
    };
    assert!(matches!(
        expr.kind,
        ExprKind::Index {
            index: nia_ast::IndexArg::Range(_),
            ..
        }
    ));
}

#[test]
fn parses_bit_not_unary_operator() {
    let (module, errors) = parse_module(
        r#"
fn main(x: u32) u32 {
    ~x
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    assert!(matches!(
        body.tail.as_ref().map(|expr| &expr.kind),
        Some(ExprKind::Unary {
            op: UnaryOp::BitNot,
            ..
        })
    ));
}

#[test]
fn parses_nested_reference_slice_types() {
    let (module, errors) = parse_module(
        r#"
fn take(xs: &const &const [u8]) {}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let Some(ty) = function.params[0].ty.as_ref() else {
        panic!("expected parameter type");
    };
    let TypeKind::Pointer { elem, .. } = &ty.kind else {
        panic!("expected outer pointer");
    };
    assert!(matches!(elem.kind, TypeKind::Slice { .. }));
}

#[test]
fn parses_optional_and_error_union_syntax() {
    let (module, errors) = parse_module(
        r#"
fn maybe(x: bool, err: i32) i32!i32 {
    var a: ?i32 = ?10i32;
    var b: i32!i32 = !20i32;
    var c: i32!i32 = err!;
    if not x {
        return err!;
    }
    b.?
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    assert!(matches!(
        function.return_type.as_ref().map(|ty| &ty.kind),
        Some(TypeKind::ErrorUnion { .. })
    ));
    let body = function.body.as_ref().expect("expected body");
    let StmtKind::Binding(optional) = &body.stmts[0].kind else {
        panic!("expected optional binding");
    };
    assert!(matches!(
        optional.ty.as_ref().map(|ty| &ty.kind),
        Some(TypeKind::Optional { .. })
    ));
    assert!(matches!(
        optional.value.as_ref().map(|expr| &expr.kind),
        Some(ExprKind::OptionalSome { .. })
    ));
    let StmtKind::Binding(ok) = &body.stmts[1].kind else {
        panic!("expected error success binding");
    };
    assert!(matches!(
        ok.value.as_ref().map(|expr| &expr.kind),
        Some(ExprKind::ErrorOk { .. })
    ));
    let StmtKind::Binding(err_binding) = &body.stmts[2].kind else {
        panic!("expected error value binding");
    };
    assert!(matches!(
        err_binding.value.as_ref().map(|expr| &expr.kind),
        Some(ExprKind::ErrorErr { .. })
    ));
    let StmtKind::Expr(if_expr) = &body.stmts[3].kind else {
        panic!("expected if statement");
    };
    let ExprKind::If { cond, .. } = &if_expr.kind else {
        panic!("expected if");
    };
    assert!(matches!(
        cond.kind,
        ExprKind::Unary {
            op: UnaryOp::Not,
            ..
        }
    ));
    assert!(matches!(
        body.tail.as_ref().map(|expr| &expr.kind),
        Some(ExprKind::Try { .. })
    ));
}

#[test]
fn parses_optional_and_error_union_switch_patterns() {
    let (module, errors) = parse_module(
        r#"
fn optional(value: ?i32) i32 {
    switch value {
        ?x => x,
        null => 0,
    }
}

fn error_union(value: i32!i32) i32 {
    switch value {
        !x => x,
        e! => e,
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let Some(tail) = &body.tail else {
        panic!("expected tail");
    };
    let ExprKind::Switch(switch) = &tail.kind else {
        panic!("expected switch");
    };
    assert!(matches!(
        switch.arms[0].patterns.as_slice(),
        [SwitchPattern::OptionalSome { name, .. }] if name == "x"
    ));
    assert!(matches!(
        switch.arms[1].patterns.as_slice(),
        [SwitchPattern::OptionalNull { .. }]
    ));
}
