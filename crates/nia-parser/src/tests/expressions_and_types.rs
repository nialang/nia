// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn parses_unit_and_tuple_types_and_values() {
    let (module, errors) = parse_module(
        r#"
fn values(unit: (), single: (i32,), pair: (i32, bool), grouped: (i32)) () {
    let a = ();
    let b = (1,);
    let c = (1, true);
    let d = (1);
    ()
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");

    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    assert!(matches!(
        function.params[0].ty.as_ref().map(|ty| &ty.kind),
        Some(TypeKind::Tuple { elems }) if elems.is_empty()
    ));
    assert!(matches!(
        function.params[1].ty.as_ref().map(|ty| &ty.kind),
        Some(TypeKind::Tuple { elems }) if elems.len() == 1
    ));
    assert!(matches!(
        function.params[2].ty.as_ref().map(|ty| &ty.kind),
        Some(TypeKind::Tuple { elems }) if elems.len() == 2
    ));
    assert!(matches!(
        function.params[3].ty.as_ref().map(|ty| &ty.kind),
        Some(TypeKind::Path { .. })
    ));
    assert!(matches!(
        function.return_type.as_ref().map(|ty| &ty.kind),
        Some(TypeKind::Tuple { elems }) if elems.is_empty()
    ));

    let body = function.body.as_ref().expect("expected body");
    for (index, expected_len) in [0, 1, 2].into_iter().enumerate() {
        let StmtKind::Binding(binding) = &body.stmts[index].kind else {
            panic!("expected binding");
        };
        assert!(matches!(
            binding.value.as_ref().map(|value| &value.kind),
            Some(ExprKind::Tuple(elems)) if elems.len() == expected_len
        ));
    }
    let StmtKind::Binding(grouped) = &body.stmts[3].kind else {
        panic!("expected binding");
    };
    assert!(matches!(
        grouped.value.as_ref().map(|value| &value.kind),
        Some(ExprKind::Integer(_))
    ));
    assert!(matches!(
        body.tail.as_ref().map(|value| &value.kind),
        Some(ExprKind::Tuple(elems)) if elems.is_empty()
    ));
}

#[test]
fn parses_tuple_projections_separately_from_named_fields() {
    let (module, errors) = parse_module(
        r#"
struct Pair {
    left: (i32, bool),
}

fn project(pair: Pair) bool {
    let first = (1, true).0;
    pair.left.0.1
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");

    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let StmtKind::Binding(first) = &body.stmts[0].kind else {
        panic!("expected binding");
    };
    assert!(matches!(
        first.value.as_ref().map(|value| &value.kind),
        Some(ExprKind::TupleField { index: 0, .. })
    ));

    let tail = body.tail.as_ref().expect("expected tail");
    let ExprKind::TupleField { lhs, index: 1 } = &tail.kind else {
        panic!("expected outer tuple projection");
    };
    let ExprKind::TupleField { lhs, index: 0 } = &lhs.kind else {
        panic!("expected nested tuple projection");
    };
    assert!(matches!(lhs.kind, ExprKind::Field { .. }));
}

#[test]
fn parses_explicit_capture_closures() {
    let (module, errors) = parse_module(
        r#"
fn make(base: i32) () {
    let offset = 2;
    let add = \[base, offset] value: i32 -> {
        base + offset + value
    };
    let stateless = \value: i32 -> { value };
    ()
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");

    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let StmtKind::Binding(binding) = &body.stmts[1].kind else {
        panic!("expected closure binding");
    };
    let ExprKind::Closure {
        captures,
        params,
        body: closure_body,
    } = &binding.value.as_ref().expect("expected closure value").kind
    else {
        panic!("expected closure expression");
    };
    assert_eq!(captures.len(), 2);
    assert_eq!(params.len(), 1);
    assert!(matches!(closure_body.kind, ExprKind::Block(_)));

    let StmtKind::Binding(stateless_binding) = &body.stmts[2].kind else {
        panic!("expected stateless closure binding");
    };
    let ExprKind::Closure { captures, .. } = &stateless_binding
        .value
        .as_ref()
        .expect("expected closure value")
        .kind
    else {
        panic!("expected closure expression");
    };
    assert!(captures.is_empty());
}

#[test]
fn parses_closure_parameters_without_type_annotations() {
    let (module, errors) = parse_module(
        r#"
fn main() () {
    let callback = \left, right -> { left + right };
    ()
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");

    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let StmtKind::Binding(binding) = &body.stmts[0].kind else {
        panic!("expected closure binding");
    };
    let ExprKind::Closure { params, .. } = &binding.value.as_ref().expect("closure").kind else {
        panic!("expected closure");
    };
    assert_eq!(params.len(), 2);
    assert!(params.iter().all(|param| param.ty.is_none()));
}

#[test]
fn parses_closure_capture_modes_and_expression_body() {
    let (module, errors) = parse_module(
        r#"
fn main(x: i32, y: i32, z: i32) () {
    let callback = \[x, &y, &mut z] a, b -> x + y.* + z.* + a + b;
    ()
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");

    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let StmtKind::Binding(binding) = &body.stmts[0].kind else {
        panic!("expected closure binding");
    };
    let ExprKind::Closure { captures, body, .. } = &binding.value.as_ref().expect("closure").kind
    else {
        panic!("expected closure");
    };
    assert!(matches!(captures[0].value.kind, ExprKind::Ident(_)));
    assert!(matches!(
        captures[1].value.kind,
        ExprKind::Unary {
            op: UnaryOp::RefReadOnly,
            ..
        }
    ));
    assert!(matches!(
        captures[2].value.kind,
        ExprKind::Unary {
            op: UnaryOp::Ref,
            ..
        }
    ));
    assert!(!matches!(body.kind, ExprKind::Block(_)));
}

#[test]
fn closure_capture_entries_must_be_names() {
    let (_module, errors) = parse_module(
        r#"
fn main(x: i32) () {
    let callback = \[captured = x] -> captured;
    ()
}
"#,
    );
    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("expected `]` after closure captures")
    }));
}

#[test]
fn rejects_empty_closure_capture_list() {
    let (_module, errors) = parse_module(
        r#"
fn main() () {
    let callback = \[] value -> value;
    ()
}
"#,
    );
    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("empty closure capture list must be omitted")
    }));
}

#[test]
fn rejects_non_identifier_closure_parameters() {
    let (_module, errors) = parse_module(
        r#"
fn main() () {
    let callback = \value: i32, ... -> value;
    ()
}
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected closure parameter name"))
    );
}

#[test]
fn rejects_non_canonical_tuple_projection_fields() {
    for (field, expected) in [
        ("0x1", "tuple field must be a decimal integer"),
        ("1_0", "tuple field must be a decimal integer"),
        ("01", "tuple field must not contain leading zeroes"),
        (
            "99999999999999999999999999999999999999999999999999",
            "tuple field index is too large",
        ),
    ] {
        let source = format!(
            r#"
fn project(pair: (i32, bool)) i32 {{
    pair.{field}
}}
"#
        );
        let (_, errors) = parse_module(&source);
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "missing `{expected}` for `.{field}` in {errors:?}"
        );
    }
}

#[test]
fn reports_malformed_tuple_types_values_and_patterns() {
    for (source, expected) in [
        (
            "fn malformed() () { let value = 1 as (i32, bool u8); }",
            "expected `)` after tuple type",
        ),
        (
            "fn value() () { let pair = (1, true false); }",
            "expected `)` after tuple",
        ),
        (
            "fn bind() () { let (first, second third) = (1, 2); }",
            "expected binding pattern",
        ),
    ] {
        let (_, errors) = parse_module(source);
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "missing `{expected}` in {errors:?}"
        );
    }
}

#[test]
fn parses_opaque_only_as_a_distinct_type_syntax() {
    let (module, errors) = parse_module(
        r#"
extern fn use_pointer(value: &opaque) &mut opaque;
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    assert!(matches!(
        function.params[0].ty.as_ref().map(|ty| &ty.kind),
        Some(TypeKind::Pointer { elem, .. }) if matches!(elem.kind, TypeKind::Opaque)
    ));
    assert!(matches!(
        function.return_type.as_ref().map(|ty| &ty.kind),
        Some(TypeKind::Pointer { elem, .. }) if matches!(elem.kind, TypeKind::Opaque)
    ));
}

#[test]
fn qualified_enum_value_before_if_block_is_not_a_struct_literal() {
    let (module, errors) = parse_module(
        r#"
enum Mode {
    Active,
}

fn main(mode: Mode) i32 {
    if mode == Mode::Active {
        1
    } else {
        0
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    assert!(matches!(
        body.tail.as_ref().map(|expr| &expr.kind),
        Some(ExprKind::If { .. })
    ));
}

#[test]
fn parses_function_body_statements_and_expressions() {
    let (module, errors) = parse_module(
        r#"
fn main() i32 {
    let mut x = 1 + 2 * 3 << 1;
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
    callback: &fn(i32, ...) (),
}

fn make() Header {
    let mut data: [_]u8 = [0; 8];
    let mut more: [_]u8 = [1, 2, 3];
    let mut header: Header = { bytes: data, callback: cb };
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
fn parses_callable_interface_types() {
    let (module, errors) = parse_module(
        r#"
type Callback = Fn(i32, bool) i32;
type CallbackRef = &Fn(i32) i32;
type CallbackMut = &mut Fn(i32) i32;
type UnitCallback = Fn();
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");

    let aliases = module
        .items
        .iter()
        .map(|item| match &item.kind {
            ItemKind::TypeAlias(alias) => alias.ty.as_ref().expect("alias target"),
            _ => panic!("expected type alias"),
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        &aliases[0].kind,
        TypeKind::Callable {
            params,
            return_type: Some(_),
        } if params.len() == 2
    ));
    assert!(matches!(
        &aliases[1].kind,
        TypeKind::Pointer {
            is_readonly: true,
            elem,
        } if matches!(elem.kind, TypeKind::Callable { .. })
    ));
    assert!(matches!(
        &aliases[2].kind,
        TypeKind::Pointer {
            is_readonly: false,
            elem,
        } if matches!(elem.kind, TypeKind::Callable { .. })
    ));
    assert!(matches!(
        &aliases[3].kind,
        TypeKind::Callable {
            params,
            return_type: None,
        } if params.is_empty()
    ));
}

#[test]
fn rejects_variadic_callable_interface_types() {
    let (_, errors) = parse_module("type Callback = Fn(i32, ...);");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("callable interface types cannot be variadic")),
        "{errors:?}"
    );
}

#[test]
fn parses_casts_builtins_and_struct_literals() {
    let (module, errors) = parse_module(
        r#"
struct Pair[T] {
    value: T,
}

fn make(ptr: &u8, xs: &[_]i32) Pair[i32] {
    let mut size = std::builtin::size[Pair[i32]]();
    let mut offset = std::builtin::offset[Pair[i32]]("value");
    let mut addr = ptr as usize;
    let mut first = xs[0];
    let mut value = ptr.*;
    { value: (addr + size + offset) as i32 }
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
    assert!(matches!(callee.kind, ExprKind::BracketSuffix { .. }));
    let StmtKind::Binding(offset) = &body.stmts[1].kind else {
        panic!("expected binding");
    };
    let Some(ExprKind::Call { callee, args }) = offset.value.as_ref().map(|value| &value.kind)
    else {
        panic!("expected offset builtin call");
    };
    assert_eq!(args.len(), 1);
    assert!(matches!(callee.kind, ExprKind::BracketSuffix { .. }));
    let StmtKind::Binding(addr) = &body.stmts[2].kind else {
        panic!("expected binding");
    };
    assert!(matches!(
        addr.value.as_ref().map(|value| &value.kind),
        Some(ExprKind::Cast { .. })
    ));
    let StmtKind::Binding(first) = &body.stmts[3].kind else {
        panic!("expected binding");
    };
    assert!(matches!(
        first.value.as_ref().map(|value| &value.kind),
        Some(ExprKind::BracketSuffix { .. })
    ));
    let StmtKind::Binding(value) = &body.stmts[4].kind else {
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
fn rejects_removed_at_prefixed_builtin_call_syntax() {
    let (_, errors) = parse_module(
        r#"
fn size() usize {
    @size[usize]()
}
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected expression")),
        "{errors:?}"
    );
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
    let mut p = Point{x: 1, y: 2};
    let mut xs = [_]i32[1, 2, 3];
    let mut boxes = [_]Box[i32][Box { value: 1 }];
    let mut matrix = [2][2]Box[i32][
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
fn take(xs: &[i32], ys: &mut [i32]) usize {
    let mut a = &xs[..];
    let mut b = &xs[0..2];
    let mut c = &xs[0..=2];
    let mut d = &xs[1..];
    let mut e = &xs[..3];
    let mut f = &xs[..=4];
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
        Some(TypeKind::Slice {
            is_readonly: true,
            ..
        })
    ));
    assert!(matches!(
        function.params[1].ty.as_ref().map(|ty| &ty.kind),
        Some(TypeKind::Slice {
            is_readonly: false,
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
fn parses_slice_pointee_type_and_extend_target() {
    let (module, errors) = parse_module(
        r#"
fn take(xs: [i32]) () {}

extend[T] [T] {
    fn len2(& self) usize {
        self.len()
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    assert!(matches!(
        function.params[0].ty.as_ref().map(|ty| &ty.kind),
        Some(TypeKind::SlicePointee { .. })
    ));
    let ItemKind::Extend(extend) = &module.items[1].kind else {
        panic!("expected extend");
    };
    assert!(matches!(extend.target.kind, TypeKind::SlicePointee { .. }));
}

#[test]
fn parses_concrete_slice_pointee_extend_targets() {
    let (module, errors) = parse_module(
        r#"
trait Format {}

extend [char] : Format {}
extend [u8] {
    fn len2(& self) usize {
        self.len()
    }
}
extend [&[char]] {
    fn partCount(&self) usize {
        self.len()
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Extend(char_extend) = &module.items[1].kind else {
        panic!("expected char extend");
    };
    assert!(char_extend.generics.is_empty());
    assert!(matches!(
        char_extend.target.kind,
        TypeKind::SlicePointee { .. }
    ));
    let ItemKind::Extend(byte_extend) = &module.items[2].kind else {
        panic!("expected byte extend");
    };
    assert!(byte_extend.generics.is_empty());
    assert!(matches!(
        byte_extend.target.kind,
        TypeKind::SlicePointee { .. }
    ));
    let ItemKind::Extend(text_parts_extend) = &module.items[3].kind else {
        panic!("expected extend");
    };
    assert!(text_parts_extend.generics.is_empty());
    assert!(matches!(
        &text_parts_extend.target.kind,
        TypeKind::SlicePointee { elem }
            if matches!(
                &elem.kind,
                TypeKind::Slice {
                    is_readonly: true,
                    ..
                }
            )
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
fn take(xs: &&[u8]) {}
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
fn distinguishes_slice_and_array_pointer_types() {
    let (module, errors) = parse_module(
        r#"
fn take(slice: &[u8], array: &[3]u8, inferred: &mut [_]u8) {}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    assert!(matches!(
        function.params[0].ty.as_ref().map(|ty| &ty.kind),
        Some(TypeKind::Slice {
            is_readonly: true,
            ..
        })
    ));
    let Some(TypeKind::Pointer {
        is_readonly: true,
        elem,
    }) = function.params[1].ty.as_ref().map(|ty| &ty.kind)
    else {
        panic!("expected readonly array pointer");
    };
    assert!(matches!(
        elem.kind,
        TypeKind::Array {
            len: ArrayLen::Expr(_),
            ..
        }
    ));
    let Some(TypeKind::Pointer {
        is_readonly: false,
        elem,
    }) = function.params[2].ty.as_ref().map(|ty| &ty.kind)
    else {
        panic!("expected writable inferred-array pointer");
    };
    assert!(matches!(
        elem.kind,
        TypeKind::Array {
            len: ArrayLen::Infer,
            ..
        }
    ));
}

#[test]
fn parses_volatile_pointer_types() {
    let (module, errors) = parse_module(
        r#"
fn use_regs(read: ^u32, write: ^mut u32, maybe: ?^mut i32) {}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    assert!(matches!(
        function.params[0].ty.as_ref().map(|ty| &ty.kind),
        Some(TypeKind::VolatilePointer {
            is_readonly: true,
            ..
        })
    ));
    assert!(matches!(
        function.params[1].ty.as_ref().map(|ty| &ty.kind),
        Some(TypeKind::VolatilePointer {
            is_readonly: false,
            ..
        })
    ));
    let Some(TypeKind::Optional { elem }) = function.params[2].ty.as_ref().map(|ty| &ty.kind)
    else {
        panic!("expected optional volatile pointer");
    };
    assert!(matches!(
        elem.kind,
        TypeKind::VolatilePointer {
            is_readonly: false,
            ..
        }
    ));
}

#[test]
fn parses_optional_and_error_union_syntax() {
    let (module, errors) = parse_module(
        r#"
fn maybe(x: bool, err: i32) i32!i32 {
    let mut a: ?i32 = ?10i32;
    let mut b: i32!i32 = !20i32;
    let mut c: i32!i32 = err!;
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
fn parses_optional_error_union_as_error_union_with_optional_error_type() {
    let (module, errors) = parse_module(
        r#"
fn read() ?Error!i32 {}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let Some(return_type) = function.return_type.as_ref() else {
        panic!("expected return type");
    };
    let TypeKind::ErrorUnion { error, .. } = &return_type.kind else {
        panic!("expected outer error union");
    };
    assert!(matches!(error.kind, TypeKind::Optional { .. }));
}

#[test]
fn parses_error_union_optional_as_error_union_of_optional() {
    let (module, errors) = parse_module(
        r#"
fn read() Error!?i32 {}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let Some(return_type) = function.return_type.as_ref() else {
        panic!("expected return type");
    };
    let TypeKind::ErrorUnion { value, .. } = &return_type.kind else {
        panic!("expected outer error union");
    };
    assert!(matches!(value.kind, TypeKind::Optional { .. }));
}

#[test]
fn parenthesized_types_override_optional_error_union_precedence() {
    let (module, errors) = parse_module(
        r#"
fn optional_error() (?Error)!i32 {}
fn optional_value() Error!(?i32) {}
fn optional_of_error() ?(Error!i32) {}
fn nested_optional_of_error() ??(Error!i32) {}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");

    let ItemKind::Function(optional_error) = &module.items[0].kind else {
        panic!("expected function");
    };
    let return_type = optional_error
        .return_type
        .as_ref()
        .expect("expected return type");
    let TypeKind::ErrorUnion { error, .. } = &return_type.kind else {
        panic!("expected outer error union");
    };
    assert!(matches!(error.kind, TypeKind::Optional { .. }));

    let ItemKind::Function(optional_value) = &module.items[1].kind else {
        panic!("expected function");
    };
    let return_type = optional_value
        .return_type
        .as_ref()
        .expect("expected return type");
    let TypeKind::ErrorUnion { value, .. } = &return_type.kind else {
        panic!("expected outer error union");
    };
    assert!(matches!(value.kind, TypeKind::Optional { .. }));

    let ItemKind::Function(optional_of_error) = &module.items[2].kind else {
        panic!("expected function");
    };
    let return_type = optional_of_error
        .return_type
        .as_ref()
        .expect("expected return type");
    let TypeKind::Optional { elem } = &return_type.kind else {
        panic!("expected outer optional");
    };
    assert!(matches!(elem.kind, TypeKind::ErrorUnion { .. }));

    let ItemKind::Function(nested_optional_of_error) = &module.items[3].kind else {
        panic!("expected function");
    };
    let return_type = nested_optional_of_error
        .return_type
        .as_ref()
        .expect("expected return type");
    let TypeKind::Optional { elem } = &return_type.kind else {
        panic!("expected outer optional");
    };
    let TypeKind::Optional { elem } = &elem.kind else {
        panic!("expected nested optional");
    };
    assert!(matches!(elem.kind, TypeKind::ErrorUnion { .. }));
}

#[test]
fn parses_optional_and_error_union_if_patterns() {
    let (module, errors) = parse_module(
        r#"
fn optional(value: ?i32) i32 {
    if value is ?x {
        x
    } else {
        0
    }
}

fn error_union(value: i32!i32) i32 {
    switch value {
        !x => {
            x
        },
        e! => {
            e
        },
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
    let ExprKind::IfPattern(if_pattern) = &tail.kind else {
        panic!("expected if pattern");
    };
    assert!(matches!(
        &if_pattern.pattern.kind,
        PatternKind::OptionalSome(inner)
            if matches!(&inner.kind, PatternKind::Bind { name, .. } if *name == sym("x"))
    ));
    assert!(if_pattern.else_branch.is_some());
}
