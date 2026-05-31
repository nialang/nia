// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_ast::{ExprKind, SwitchArmBody};
use nia_node_id::{NodePosition, SyntaxKind};
use nia_source::{SourceId, SourceRevision, SourceVersion};

#[test]
fn parses_top_level_items() {
    let source = r#"
import .math as math;

pub extern fn printf(fmt: &u8, ...);

pub enum Color: u8 {
    Black,
    White = 2,
}

struct Vec2 {
    x: i32,
    y: i32,
}

extend Vec2 {
    fn len2(&const self) i32 {
        self.x * self.x + self.y * self.y
    }
}

const banner = "nia\0";
extern var a: usize;
type Byte = u8;
fn main() i32 { 0 }
"#;
    let (module, errors) = parse_module(source);
    assert_eq!(errors, Vec::<ParseError>::new());
    assert_eq!(module.items.len(), 9);
    assert!(matches!(module.items[0].kind, ItemKind::Import(_)));
    assert!(matches!(&module.items[1].kind, ItemKind::Function(function) if function.is_extern));
    assert!(matches!(module.items[2].kind, ItemKind::Enum(_)));
    assert!(matches!(module.items[3].kind, ItemKind::Struct(_)));
    assert!(matches!(module.items[4].kind, ItemKind::Extend(_)));
    assert!(matches!(module.items[5].kind, ItemKind::Binding(_)));
    assert!(matches!(&module.items[6].kind, ItemKind::Binding(binding) if binding.is_extern));
    assert!(matches!(module.items[7].kind, ItemKind::TypeAlias(_)));
    assert!(matches!(module.items[8].kind, ItemKind::Function(_)));
}

#[test]
fn parses_ast_from_lossless_syntax_tree() {
    let source = "fn  main() i32 { // retained by syntax\n  0\n}\n";
    let syntax = nia_syntax::parse_source(source, None);
    let (from_source, source_errors) = parse_module(source);
    let (from_syntax, syntax_errors) = parse_module_syntax(&syntax);

    assert_eq!(syntax.full_text(), source);
    assert_eq!(source_errors, syntax_errors);
    assert_eq!(from_source, from_syntax);
}

#[test]
fn parse_errors_from_syntax_carry_red_token_node_keys() {
    let version = SourceVersion {
        id: SourceId(9),
        revision: SourceRevision(3),
    };
    let syntax = nia_syntax::parse_source("fn bad(value) {}", Some(version));
    let (_, errors) = parse_module_syntax(&syntax);

    let error = errors
        .iter()
        .find(|error| error.message.contains("expected `:` after parameter name"))
        .expect("parameter type error");
    let key = error.node_key.as_ref().expect("red token node key");
    assert_eq!(key.source_version(), version);
    assert!(matches!(
        &key.position,
        NodePosition::ChildPath(path) if !path.steps().is_empty()
    ));
}

#[test]
fn parse_module_syntax_records_ast_origins_as_red_child_path_ranges() {
    let version = SourceVersion {
        id: SourceId(10),
        revision: SourceRevision(4),
    };
    let syntax = nia_syntax::parse_source(
        r#"
fn main(a: i32) i32 {
    var x = a;
    x
}
"#,
        Some(version),
    );
    let (module, errors, origins) = parse_module_syntax_with_origins(&syntax);

    assert!(errors.is_empty(), "{errors:?}");
    assert!(!origins.is_empty());
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let expr = function
        .body
        .as_ref()
        .and_then(|body| body.tail.as_ref())
        .expect("tail expression");
    let key = origins
        .get(SyntaxKind::Expr, expr.span)
        .expect("tail expr origin");

    assert_eq!(key.source_version(), version);
    assert_eq!(key.kind, SyntaxKind::Expr);
    assert!(matches!(
        &key.position,
        NodePosition::ChildPathRange { start, end }
            if !start.steps().is_empty() && !end.steps().is_empty()
    ));
}

#[test]
fn reports_parameter_without_explicit_type() {
    let (_, errors) = parse_module(
        r#"
fn bad(value) {}
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected `:` after parameter name")),
        "{errors:?}"
    );
}

#[test]
fn parses_open_enum_marker() {
    let (module, errors) = parse_module(
        r#"
enum Flag {
    A,
    B,
    _,
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Enum(item_enum) = &module.items[0].kind else {
        panic!("expected enum");
    };
    assert!(item_enum.is_open);
    assert_eq!(item_enum.variants.len(), 2);
    assert_eq!(item_enum.variants[0].name, "A");
    assert_eq!(item_enum.variants[1].name, "B");
}

#[test]
fn parses_multiline_string_literal() {
    let (module, errors) = parse_module(
        r#"
const script =
    \\mov rax, 60
    \\syscall
;
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Binding(binding) = &module.items[0].kind else {
        panic!("expected binding");
    };
    assert!(
        matches!(binding.value.as_ref().map(|value| &value.kind), Some(ExprKind::String(literal)) if literal.parts[0].contains("syscall"))
    );
}

#[test]
fn parses_adjacent_quoted_string_literals_as_one_literal() {
    let (module, errors) = parse_module(
        r#"
const text = "hello" "" ", " "world" "" "!" "\n" "done";
const bytes = b"" b"n" b"" b"i" b"" b"a" b"" b"\0";
const cstr = c"" c"hello" c"" c", " c"" c"world" c"" c"!";
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Binding(text) = &module.items[0].kind else {
        panic!("expected binding");
    };
    assert!(matches!(
        text.value.as_ref().map(|value| &value.kind),
        Some(ExprKind::String(literal)) if literal.parts.len() == 8
    ));
    let ItemKind::Binding(bytes) = &module.items[1].kind else {
        panic!("expected binding");
    };
    assert!(matches!(
        bytes.value.as_ref().map(|value| &value.kind),
        Some(ExprKind::ByteString(literal)) if literal.parts.len() == 8
    ));
    let ItemKind::Binding(cstr) = &module.items[2].kind else {
        panic!("expected binding");
    };
    assert!(matches!(
        cstr.value.as_ref().map(|value| &value.kind),
        Some(ExprKind::CString(literal)) if literal.parts.len() == 8
    ));
}

#[test]
fn rejects_adjacent_string_literals_with_different_prefixes() {
    let (_module, errors) = parse_module(
        r#"
const a = "hello" b"world";
const b = b"hello" c"world";
const c = "hello" c"world";
"#,
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error
                .message
                .contains("adjacent string literals must use the same literal prefix"))
            .count(),
        3,
        "{errors:?}"
    );
}

#[test]
fn does_not_concatenate_multiline_string_literals() {
    let (_module, errors) = parse_module(
        r#"
const text =
    \\hello
    "world";
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected `;` after binding")),
        "{errors:?}"
    );
}

#[test]
fn rejects_non_terminal_open_enum_marker() {
    let (_, errors) = parse_module(
        r#"
enum Flag {
    A,
    _,
    B,
}
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("open enum marker must be last")),
        "{errors:?}"
    );
}

#[test]
fn parses_extern_items_and_binding_declarations() {
    let (module, errors) = parse_module(
        r#"
pub extern fn printf(fmt: &u8, ...);
pub extern fn add(a: i32, b: i32) i32 {
    a + b
}
extern struct CPoint {
    x: i32,
    y: i32,
}
extern const errno: i32;
extern var global_counter: usize;

fn main() {
    var p: CPoint;
    const origin: CPoint;
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert!(
        matches!(&module.items[0].kind, ItemKind::Function(function) if function.is_extern && function.body.is_none())
    );
    assert!(
        matches!(&module.items[1].kind, ItemKind::Function(function) if function.is_extern && function.body.is_some())
    );
    assert!(
        matches!(&module.items[2].kind, ItemKind::Struct(item_struct) if item_struct.is_extern)
    );
    assert!(
        matches!(&module.items[3].kind, ItemKind::Binding(binding) if binding.is_extern && binding.is_const && binding.value.is_none())
    );
    assert!(
        matches!(&module.items[4].kind, ItemKind::Binding(binding) if binding.is_extern && binding.value.is_none())
    );
    let ItemKind::Function(function) = &module.items[5].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("body");
    assert!(matches!(&body.stmts[0].kind, StmtKind::Binding(binding) if binding.value.is_none()));
    assert!(
        matches!(&body.stmts[1].kind, StmtKind::Binding(binding) if binding.is_const && binding.value.is_none())
    );
}

#[test]
fn rejects_extern_binding_without_var_or_const() {
    let (_module, errors) = parse_module("extern errno: i32;");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("expected `struct`, `union`, `fn`, `var`, or `const` after `extern`")),
        "{errors:?}"
    );
}

#[test]
fn rejects_bare_binding_syntax() {
    let (_, top_level_errors) = parse_module("answer: i32 = 42;");
    assert!(
        top_level_errors
            .iter()
            .any(|error| error.message.contains("expected item")),
        "{top_level_errors:?}"
    );

    let (_, local_errors) = parse_module(
        r#"
fn main() i32 {
    answer: i32 = 42;
    answer
}
"#,
    );
    assert!(
        local_errors
            .iter()
            .any(|error| error.message.contains("expected `;` after expression")),
        "{local_errors:?}"
    );
}

#[test]
fn parses_nested_using_group_items() {
    let (module, errors) = parse_module(
        r#"
import .math;
using math::{add, sub as minus, Operator::*};
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Using(using) = &module.items[1].kind else {
        panic!("expected using");
    };
    let UsingSelector::Group(items) = &using.selector else {
        panic!("expected using group");
    };
    assert_eq!(items.len(), 3);
    assert!(matches!(items[0], UsingGroupItem::Name(_)));
    assert!(matches!(items[1], UsingGroupItem::Name(_)));
    assert!(
        matches!(&items[2], UsingGroupItem::Nested { host, selector }
            if host.len() == 1
                && host[0].name == "Operator"
                && matches!(selector.as_ref(), UsingSelector::Wildcard { .. }))
    );
}

#[test]
fn parses_module_self_using() {
    let (module, errors) = parse_module(
        r#"
import .math;
pub using math;
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Using(using) = &module.items[1].kind else {
        panic!("expected using");
    };
    assert_eq!(using.host.len(), 1);
    assert_eq!(using.host[0].name, "math");
    assert!(matches!(using.selector, UsingSelector::SelfName));
}

#[test]
fn parses_root_using_group_with_module_and_deep_paths() {
    let (module, errors) = parse_module(
        r#"
using {A, A::foo, C::SomeEnum::DDD};
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Using(using) = &module.items[0].kind else {
        panic!("expected using");
    };
    assert!(using.host.is_empty());
    let UsingSelector::Group(items) = &using.selector else {
        panic!("expected root using group");
    };
    assert_eq!(items.len(), 3);
    assert!(matches!(items[0], UsingGroupItem::Name(_)));
    assert!(
        matches!(&items[1], UsingGroupItem::Nested { host, selector }
        if host.len() == 1
            && host[0].name == "A"
            && matches!(selector.as_ref(), UsingSelector::Single(name) if name.name == "foo"))
    );
    assert!(
        matches!(&items[2], UsingGroupItem::Nested { host, selector }
        if host.len() == 2
            && host[0].name == "C"
            && host[1].name == "SomeEnum"
            && matches!(selector.as_ref(), UsingSelector::Single(name) if name.name == "DDD"))
    );
}

#[test]
fn parses_deep_nested_using_group_selectors() {
    let (module, errors) = parse_module(
        r#"
using A::B::{C::foo, D::E::{F::goo, G}, H::Color::*};
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Using(using) = &module.items[0].kind else {
        panic!("expected using");
    };
    assert_eq!(using.host.len(), 2);
    assert_eq!(using.host[0].name, "A");
    assert_eq!(using.host[1].name, "B");
    let UsingSelector::Group(items) = &using.selector else {
        panic!("expected using group");
    };
    assert_eq!(items.len(), 3);
    assert!(
        matches!(&items[0], UsingGroupItem::Nested { host, selector }
        if host.len() == 1
            && host[0].name == "C"
            && matches!(selector.as_ref(), UsingSelector::Single(name) if name.name == "foo"))
    );
    assert!(
        matches!(&items[1], UsingGroupItem::Nested { host, selector }
        if host.len() == 2
            && host[0].name == "D"
            && host[1].name == "E"
            && matches!(selector.as_ref(), UsingSelector::Group(group) if group.len() == 2))
    );
    assert!(
        matches!(&items[2], UsingGroupItem::Nested { host, selector }
        if host.len() == 2
            && host[0].name == "H"
            && host[1].name == "Color"
            && matches!(selector.as_ref(), UsingSelector::Wildcard { .. }))
    );
}

#[test]
fn rejects_extern_before_pub_modifier_order() {
    let (_, errors) = parse_module("extern pub fn add(a: i32, b: i32) i32;");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("expected `struct`, `union`, `fn`, `var`, or `const` after `extern`")),
        "{errors:?}"
    );
}

#[test]
fn parses_extend_methods_and_struct_fields() {
    let (module, errors) = parse_module(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn get(&const self) T { self.value }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Struct(item) = &module.items[0].kind else {
        panic!("expected struct");
    };
    assert_eq!(item.generics, vec!["T"]);
    assert_eq!(item.fields.len(), 1);
    let ItemKind::Extend(extend) = &module.items[1].kind else {
        panic!("expected extend");
    };
    assert_eq!(extend.generics, vec!["T"]);
    assert_eq!(extend.methods.len(), 1);
}

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
    for var i = 0; i < 3; i += 1 {
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
    assert!(matches!(body.stmts[3].kind, StmtKind::For(_)));
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
    @len(a) + @len(b) + @len(c) + @len(d) + @len(e) + @len(f)
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
fn parses_explicit_generic_function_instantiation() {
    let (module, errors) = parse_module(
        r#"
fn id[T](value: T) T { value }
fn main() i32 {
    id[i32](1)
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    let ExprKind::Call { callee, .. } = &tail.kind else {
        panic!("expected call");
    };
    assert!(matches!(callee.kind, ExprKind::BracketSuffix { .. }));
}

#[test]
fn parses_generic_type_prefix_associated_call() {
    let (module, errors) = parse_module(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn make(value: T) Box[T] {
        { value: value }
    }
}

fn main() Box[i32] {
    Box[i32]::make(1)
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[2].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    let ExprKind::Call { callee, .. } = &tail.kind else {
        panic!("expected call");
    };
    let ExprKind::Qualified { lhs, .. } = &callee.kind else {
        panic!("expected qualified callee");
    };
    assert!(matches!(lhs.kind, ExprKind::BracketSuffix { .. }));
}

#[test]
fn parses_structural_type_target_associated_call() {
    let (module, errors) = parse_module(
        r#"
extend[T] &T {
    fn is_null(self) bool {
        self as usize == 0
    }
}

fn main(ptr: &u8) bool {
    [&u8]::is_null(ptr)
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    let ExprKind::Call { callee, .. } = &tail.kind else {
        panic!("expected call");
    };
    let ExprKind::Qualified { lhs, .. } = &callee.kind else {
        panic!("expected qualified callee");
    };
    assert!(matches!(lhs.kind, ExprKind::TypeTarget { .. }));
}

#[test]
fn parses_deep_pointer_structural_type_target_associated_call() {
    let (module, errors) = parse_module(
        r#"
extend &&&&&&const &&i32 {
    fn null(self) bool {
        self as usize == 0
    }
}

fn main(ptr: &&&&&&const &&i32) bool {
    [&&&&&&const &&i32]::null(ptr)
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    let ExprKind::Call { callee, .. } = &tail.kind else {
        panic!("expected call");
    };
    let ExprKind::Qualified { lhs, .. } = &callee.kind else {
        panic!("expected qualified callee");
    };
    assert!(matches!(lhs.kind, ExprKind::TypeTarget { .. }));
}

#[test]
fn parses_array_structural_type_target_associated_call() {
    let (module, errors) = parse_module(
        r#"
extend[T] [3]T {
    fn first(self) T {
        self[0]
    }
}

fn main(triple: [3]i32) i32 {
    [[3]i32]::first(triple)
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    let ExprKind::Call { callee, .. } = &tail.kind else {
        panic!("expected call");
    };
    let ExprKind::Qualified { lhs, .. } = &callee.kind else {
        panic!("expected qualified callee");
    };
    assert!(matches!(lhs.kind, ExprKind::TypeTarget { .. }));
}

#[test]
fn parses_array_structural_type_target_in_binary_expr() {
    let (module, errors) = parse_module(
        r#"
extend[T] [3]T {
    fn first(self) T {
        self[0]
    }
}

fn zero() i32 {
    0
}

fn main(triple: [3]i32) i32 {
    [[3]i32]::first(triple) + zero() as i32
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[2].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    assert!(matches!(tail.kind, ExprKind::Binary { .. }));
}

#[test]
fn parses_explicit_associated_type_projection() {
    let (module, errors) = parse_module(
        r#"
trait Source {
    type Item;

    fn get(&const self) [Self as Source]::Item;
}

fn read[T](value: &const T) [T as Source]::Item
where T: Source {
    value.get()
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Trait(item_trait) = &module.items[0].kind else {
        panic!("expected trait");
    };
    let return_ty = item_trait.methods[0]
        .function
        .return_type
        .as_ref()
        .expect("expected return type");
    assert!(matches!(return_ty.kind, TypeKind::Projection { .. }));
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let return_ty = function.return_type.as_ref().expect("expected return type");
    assert!(matches!(return_ty.kind, TypeKind::Projection { .. }));
}

#[test]
fn parses_generic_trait_associated_type_projection() {
    let (module, errors) = parse_module(
        r#"
trait Add[Rhs] {
    type Output;

    fn add(&const self, rhs: Rhs) [Self as Add[Rhs]]::Output;
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Trait(item_trait) = &module.items[0].kind else {
        panic!("expected trait");
    };
    let return_ty = item_trait.methods[0]
        .function
        .return_type
        .as_ref()
        .expect("expected return type");
    let TypeKind::Projection {
        trait_ref, name, ..
    } = &return_ty.kind
    else {
        panic!("expected projection");
    };
    assert_eq!(name, "Output");
    let TypeKind::Path { segments } = &trait_ref.kind else {
        panic!("expected trait path");
    };
    assert_eq!(segments[0].name, "Add");
    assert_eq!(segments[0].args.len(), 1);
}

#[test]
fn parses_structural_type_targets_after_if_statements() {
    let (module, errors) = parse_module(
        r#"
extend[T] &T {
    fn null(self) bool {
        self as usize == 0
    }
}

extend[T] [3]T {
    fn first(self) T {
        self[0]
    }
}

fn zero() usize {
    0usize
}

fn main(ptr: &u8, triple: [3]i32) i32 {
    if [&u8]::null(ptr) {}
    [[3]i32]::first(triple) + zero() as i32
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[3].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    assert!(matches!(tail.kind, ExprKind::Binary { .. }));
}

#[test]
fn parses_index_before_field_as_index_not_generic_instantiation() {
    let (module, errors) = parse_module(
        r#"
fn main(items: &[i32], i: usize) i32 {
    items[i].value
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    let ExprKind::Field { lhs, .. } = &tail.kind else {
        panic!("expected field");
    };
    assert!(matches!(lhs.kind, ExprKind::BracketSuffix { .. }));
}

#[test]
fn parses_lowercase_generic_associated_call_with_colon_colon() {
    let (module, errors) = parse_module(
        r#"
struct box[T] {
    value: T,
}

extend[T] box[T] {
    fn make(value: T) box[T] {
        { value: value }
    }
}

fn main() box[i32] {
    box[i32]::make(1)
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[2].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    let ExprKind::Call { callee, .. } = &tail.kind else {
        panic!("expected call");
    };
    let ExprKind::Qualified { lhs, .. } = &callee.kind else {
        panic!("expected qualified callee");
    };
    assert!(matches!(lhs.kind, ExprKind::BracketSuffix { .. }));
}

#[test]
fn reports_lexer_errors_through_parser() {
    let (_module, errors) = parse_module(r#"fn main() { var x = "\q"; }"#);
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("InvalidStringEscape"))
    );
}

#[test]
fn rejects_string_import_as_invalid_import_path() {
    let (_module, errors) = parse_module(r#"import "math";"#);
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("expected module path after `import`")),
        "{errors:?}"
    );
}

#[test]
fn rejects_deep_relative_import_prefix() {
    let (_module, errors) = parse_module("import ...math;");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("relative import supports only `.` or `..`")),
        "{errors:?}"
    );
}

#[test]
fn reports_bare_fn_type_with_function_pointer_hint() {
    let (_module, errors) = parse_module(
        r#"
struct Vtable {
    print: fn(&u8),
    write: &fn(&u8),
}
"#,
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error
                .message
                .contains("must be written as `&const fn(...)`"))
            .count(),
        2,
        "{errors:?}"
    );
}

#[test]
fn reports_missing_semicolon_between_expression_statements() {
    let (_module, errors) = parse_module(
        r#"
fn main() {
    effect()
    other();
}
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected `;` after expression")),
        "{errors:?}"
    );
}

#[test]
fn parses_union_items() {
    let (module, errors) = parse_module(
        r#"
pub extern union Bits[T] {
    i: i64,
    value: T,
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Union(item) = &module.items[0].kind else {
        panic!("expected union item");
    };
    assert_eq!(item.name, "Bits");
    assert_eq!(item.generics, ["T"]);
    assert_eq!(item.fields.len(), 2);
    assert!(item.is_extern);
}

#[test]
fn parses_c_style_for_header_with_parenthesized_block_expressions() {
    let (module, errors) = parse_module(
        r#"
fn main() {
    var i = 0;
    for ({
        var a = 1;
        var b = 3;
        {
            var c = 0;
            c + a + b
        }
    }); ({
        var d = 0;
        d < 4;
        true
    }); i += 1 {
        _ = i;
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let StmtKind::For(for_stmt) = &body.stmts[1].kind else {
        panic!("expected for statement");
    };
    let ForHeader::CStyle { init, cond, step } = &for_stmt.header else {
        panic!("expected C-style for header");
    };
    assert!(matches!(
        init.as_deref(),
        Some(ForInit::Expr(expr)) if matches!(expr.kind, ExprKind::Block(_))
    ));
    assert!(matches!(
        cond.as_deref().map(|expr| &expr.kind),
        Some(ExprKind::Block(_))
    ));
    assert!(matches!(
        step.as_deref().map(|expr| &expr.kind),
        Some(ExprKind::Assign { .. })
    ));
}

#[test]
fn reports_ambiguous_block_as_first_for_header_expression() {
    let (_module, errors) = parse_module(
        r#"
fn main() {
    var i = 0;
    for {
        var a = 1;
        a
    }; true; i += 1 {
        _ = i;
    }
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
fn parses_defer_with_complex_expression_forms() {
    let (module, errors) = parse_module(
        r#"
fn cleanup() {}

fn main(flag: bool) {
    defer cleanup();
    defer if flag {
        cleanup();
    } else {
        cleanup();
    };
    defer {
        var state = 1;
        switch state {
            0 => cleanup(),
            _ => cleanup(),
        }
    };
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    assert_eq!(
        body.stmts
            .iter()
            .filter(|stmt| matches!(stmt.kind, StmtKind::Defer(_)))
            .count(),
        3
    );
    assert!(matches!(
        &body.stmts[1].kind,
        StmtKind::Defer(expr) if matches!(expr.kind, ExprKind::If { .. })
    ));
    assert!(matches!(
        &body.stmts[2].kind,
        StmtKind::Defer(expr) if matches!(expr.kind, ExprKind::Block(_))
    ));
}

#[test]
fn parses_switch_arm_expression_statement_and_block_bodies() {
    let (module, errors) = parse_module(
        r#"
fn cleanup() {}

fn main(state: i32) i32 {
    for {
        switch state {
            0 => cleanup(),
            1 => defer cleanup(),
            2 => {
                defer cleanup();
                20
            },
            _ => break,
        }
    }
    0
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let StmtKind::For(for_stmt) = &body.stmts[0].kind else {
        panic!("expected for statement");
    };
    let expr = for_stmt.body.tail.as_ref().expect("expected switch tail");
    let ExprKind::Switch(switch) = &expr.kind else {
        panic!("expected switch expression");
    };
    assert!(matches!(switch.arms[0].body, SwitchArmBody::Expr(_)));
    assert!(matches!(
        &switch.arms[1].body,
        SwitchArmBody::Stmt(stmt) if matches!(stmt.kind, StmtKind::Defer(_))
    ));
    assert!(matches!(switch.arms[2].body, SwitchArmBody::Block(_)));
    assert!(matches!(
        &switch.arms[3].body,
        SwitchArmBody::Stmt(stmt) if matches!(stmt.kind, StmtKind::Break)
    ));
}

#[test]
fn parses_trait_impl_where_and_self_type() {
    let (module, errors) = parse_module(
        r#"
trait Show {
    fn show(&const self) i32;
    fn clone_self(&const self) Self {
        self.*
    }
}

struct Box[T] where T: Show {
    value: T,
}

extend Box[i32] : Show where i32: Show {
    fn show(&const self) i32 {
        self.value
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert!(matches!(module.items[0].kind, ItemKind::Trait(_)));
    let ItemKind::Struct(item_struct) = &module.items[1].kind else {
        panic!("expected struct");
    };
    assert_eq!(item_struct.where_clause.predicates.len(), 1);
    let ItemKind::Extend(extend) = &module.items[2].kind else {
        panic!("expected extend");
    };
    assert!(extend.trait_ref.is_some());
    assert_eq!(extend.where_clause.predicates.len(), 1);
}
