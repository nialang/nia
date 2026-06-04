// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

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
extern let errno: i32;
extern var global_counter: usize;

fn main() {
    var p: CPoint;
    let origin: CPoint;
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
        matches!(&module.items[3].kind, ItemKind::Binding(binding) if binding.is_extern && binding.is_let && binding.value.is_none())
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
        matches!(&body.stmts[1].kind, StmtKind::Binding(binding) if binding.is_let && binding.value.is_none())
    );
}

#[test]
fn parses_item_and_field_attributes() {
    let (module, errors) = parse_module(
        r#"
@[link_name("runtime_start")]
pub extern fn start(argc: i32) i32;

@[layout.version(1, true)]
struct Header {
    @[offset(0)]
    magic: u32,
    @[note(@builtin().target.os)]
    flags: u16,
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");

    assert_eq!(module.items[0].attributes.len(), 1);
    let attr = &module.items[0].attributes[0];
    assert_eq!(attr.path, vec!["link_name"]);
    assert_eq!(attr.args.len(), 1);
    assert!(matches!(attr.args[0].kind, ExprKind::String(_)));
    assert!(matches!(module.items[0].vis, Visibility::Public));

    assert_eq!(module.items[1].attributes.len(), 1);
    let attr = &module.items[1].attributes[0];
    assert_eq!(attr.path, vec!["layout", "version"]);
    assert_eq!(attr.args.len(), 2);
    assert!(matches!(attr.args[0].kind, ExprKind::Integer(_)));
    assert!(matches!(attr.args[1].kind, ExprKind::Bool(true)));

    let ItemKind::Struct(item_struct) = &module.items[1].kind else {
        panic!("expected struct");
    };
    assert_eq!(item_struct.fields.len(), 2);
    assert_eq!(item_struct.fields[0].attributes.len(), 1);
    assert_eq!(item_struct.fields[0].attributes[0].path, vec!["offset"]);
    assert_eq!(item_struct.fields[1].attributes.len(), 1);
    assert_eq!(item_struct.fields[1].attributes[0].path, vec!["note"]);
    assert!(matches!(
        &item_struct.fields[1].attributes[0].args[0].kind,
        ExprKind::Field { .. }
    ));
}

#[test]
fn rejects_bare_at_item_attribute_without_brackets() {
    let (_, errors) = parse_module(
        r#"
@link_name("runtime_start")
extern fn start() i32;
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected item")),
        "{errors:?}"
    );
}

#[test]
fn rejects_extern_binding_without_let_or_var() {
    let (_module, errors) = parse_module("extern errno: i32;");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("expected `struct`, `union`, `fn`, `let`, or `var` after `extern`")),
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
            .contains("expected `struct`, `union`, `fn`, `let`, or `var` after `extern`")),
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
    fn get(&self) T { self.value }
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
