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
fn parses_unit_tuple_and_named_enum_variants() {
    let (module, errors) = parse_module(
        r#"
enum Event: u16 {
    Closed,
    Data(Bytes),
    Move(i32, i32) = 7,
    Resize { width: i32, height: i32 },
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Enum(item_enum) = &module.items[0].kind else {
        panic!("expected enum");
    };
    assert!(matches!(
        item_enum.variants[0].payload,
        EnumVariantPayload::Unit
    ));
    assert!(matches!(
        &item_enum.variants[1].payload,
        EnumVariantPayload::Tuple(fields) if fields.len() == 1
    ));
    assert!(matches!(
        &item_enum.variants[2].payload,
        EnumVariantPayload::Tuple(fields) if fields.len() == 2
    ));
    assert!(item_enum.variants[2].value.is_some());
    assert!(matches!(
        &item_enum.variants[3].payload,
        EnumVariantPayload::Named(fields)
            if fields.len() == 2
                && fields[0].name == sym("width")
                && fields[1].name == sym("height")
    ));
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
extern static errno: i32;
extern static mut global_counter: usize;

fn main() {
    let mut p: CPoint;
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
        matches!(&module.items[3].kind, ItemKind::Binding(binding) if binding.is_extern() && !binding.is_mutable() && binding.value.is_none())
    );
    assert!(
        matches!(&module.items[4].kind, ItemKind::Binding(binding) if binding.is_extern() && binding.value.is_none())
    );
    let ItemKind::Function(function) = &module.items[5].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("body");
    assert!(matches!(&body.stmts[0].kind, StmtKind::Binding(binding) if binding.value.is_none()));
    assert!(
        matches!(&body.stmts[1].kind, StmtKind::Binding(binding) if !binding.is_mutable() && binding.value.is_none())
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
    @[note(config.target.os)]
    flags: u16,
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");

    assert_eq!(module.items[0].attributes.len(), 1);
    let attr = &module.items[0].attributes[0];
    let AttributeKind::Meta(meta) = &attr.kind else {
        panic!("expected metadata attribute");
    };
    assert_eq!(meta.path, vec![sym("link_name")]);
    assert_eq!(meta.args.len(), 1);
    assert!(matches!(meta.args[0].kind, ExprKind::String(_)));
    assert!(matches!(module.items[0].vis, Visibility::Public));

    assert_eq!(module.items[1].attributes.len(), 1);
    let attr = &module.items[1].attributes[0];
    let AttributeKind::Meta(meta) = &attr.kind else {
        panic!("expected metadata attribute");
    };
    assert_eq!(meta.path, vec![sym("layout"), sym("version")]);
    assert_eq!(meta.args.len(), 2);
    assert!(matches!(meta.args[0].kind, ExprKind::Integer(_)));
    assert!(matches!(meta.args[1].kind, ExprKind::Bool(true)));

    let ItemKind::Struct(item_struct) = &module.items[1].kind else {
        panic!("expected struct");
    };
    assert_eq!(item_struct.fields.len(), 2);
    assert_eq!(item_struct.fields[0].attributes.len(), 1);
    assert!(matches!(
        &item_struct.fields[0].attributes[0].kind,
        AttributeKind::Meta(meta) if meta.path == [sym("offset")]
    ));
    assert_eq!(item_struct.fields[1].attributes.len(), 1);
    assert!(matches!(
        &item_struct.fields[1].attributes[0].kind,
        AttributeKind::Meta(meta)
            if meta.path == [sym("note")] && matches!(meta.args[0].kind, ExprKind::Field { .. })
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
fn rejects_extern_binding_without_static() {
    let (_module, errors) = parse_module("extern errno: i32;");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("expected `struct`, `union`, `fn`, or `static` after `extern`")),
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
fn rejects_top_level_let_without_stalling_recovery() {
    let (module, errors) = parse_module("let VALUE = 1; fn main() i32 { 0 }");

    assert!(
        errors.iter().any(|error| error
            .message
            .contains("top-level storage declarations use `static`")),
        "{errors:?}"
    );
    assert_eq!(module.items.len(), 1);
    assert!(matches!(module.items[0].kind, ItemKind::Function(_)));
}

#[test]
fn item_recovery_makes_progress_across_invalid_top_level_fragments() {
    let (module, errors) = parse_module(
        r#"
let OLD = 1;
answer: i32 = 42;
extern errno: i32;
static kept: i32 = 1;
fn main() i32 { kept }
"#,
    );

    assert!(errors.len() >= 3, "{errors:?}");
    assert_eq!(module.items.len(), 2);
    assert!(matches!(module.items[0].kind, ItemKind::Binding(_)));
    assert!(matches!(module.items[1].kind, ItemKind::Function(_)));
}

#[test]
fn parses_nested_using_group_items() {
    let (module, errors) = parse_module(
        r#"
using entry::math;
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
                && host_name(&host[0]) == Some(sym("Operator"))
                && matches!(selector.as_ref(), UsingSelector::Wildcard { .. }))
    );
}

#[test]
fn parses_module_self_using() {
    let (module, errors) = parse_module(
        r#"
using entry::math;
pub using math;
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Using(using) = &module.items[1].kind else {
        panic!("expected using");
    };
    assert_eq!(using.host.len(), 1);
    assert_eq!(host_name(&using.host[0]), Some(sym("math")));
    assert!(matches!(using.selector, UsingSelector::SelfName));
}

#[test]
fn parses_package_root_using() {
    let (module, errors) = parse_module("using pkg::math;");
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Using(using) = &module.items[0].kind else {
        panic!("expected using");
    };
    assert_eq!(using.host.len(), 1);
    assert!(matches!(using.host[0].kind, PathSegmentKind::Package));
    let UsingSelector::Single(name) = &using.selector else {
        panic!("expected single using selector");
    };
    assert_eq!(name.name, sym("math"));
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
            && host_name(&host[0]) == Some(sym("A"))
            && matches!(selector.as_ref(), UsingSelector::Single(name) if name.name == sym("foo")))
    );
    assert!(
        matches!(&items[2], UsingGroupItem::Nested { host, selector }
        if host.len() == 2
            && host_name(&host[0]) == Some(sym("C"))
            && host_name(&host[1]) == Some(sym("SomeEnum"))
            && matches!(selector.as_ref(), UsingSelector::Single(name) if name.name == sym("DDD")))
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
    assert_eq!(host_name(&using.host[0]), Some(sym("A")));
    assert_eq!(host_name(&using.host[1]), Some(sym("B")));
    let UsingSelector::Group(items) = &using.selector else {
        panic!("expected using group");
    };
    assert_eq!(items.len(), 3);
    assert!(
        matches!(&items[0], UsingGroupItem::Nested { host, selector }
        if host.len() == 1
            && host_name(&host[0]) == Some(sym("C"))
            && matches!(selector.as_ref(), UsingSelector::Single(name) if name.name == sym("foo")))
    );
    assert!(
        matches!(&items[1], UsingGroupItem::Nested { host, selector }
        if host.len() == 2
            && host_name(&host[0]) == Some(sym("D"))
            && host_name(&host[1]) == Some(sym("E"))
            && matches!(selector.as_ref(), UsingSelector::Group(group) if group.len() == 2))
    );
    assert!(
        matches!(&items[2], UsingGroupItem::Nested { host, selector }
        if host.len() == 2
            && host_name(&host[0]) == Some(sym("H"))
            && host_name(&host[1]) == Some(sym("Color"))
            && matches!(selector.as_ref(), UsingSelector::Wildcard { .. }))
    );
}

#[test]
fn rejects_extern_before_pub_modifier_order() {
    let (_, errors) = parse_module("extern pub fn add(a: i32, b: i32) i32;");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("expected `struct`, `union`, `fn`, or `static` after `extern`")),
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
    assert_eq!(nia_ast::generic_param_names(&item.generics), vec![sym("T")]);
    assert_eq!(item.fields.len(), 1);
    let ItemKind::Extend(extend) = &module.items[1].kind else {
        panic!("expected extend");
    };
    assert_eq!(
        nia_ast::generic_param_names(&extend.generics),
        vec![sym("T")]
    );
    assert_eq!(extend.methods.len(), 1);
}

#[test]
fn parses_const_generic_params() {
    let (module, errors) = parse_module(
        r#"
struct Buffer[T, N: usize] {
    data: [T; N],
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Struct(item) = &module.items[0].kind else {
        panic!("expected struct");
    };
    assert_eq!(
        nia_ast::generic_param_names(&item.generics),
        vec![sym("T"), sym("N")]
    );
    assert!(item.generics[0].is_type());
    assert!(item.generics[1].is_const());
}

#[test]
fn parses_extend_associated_const_values() {
    let (module, errors) = parse_module(
        r#"
extend usize {
    pub const MAX: usize = 18446744073709551615usize;
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Extend(extend) = &module.items[0].kind else {
        panic!("expected extend");
    };
    assert_eq!(extend.associated_values.len(), 1);
    assert_eq!(extend.associated_values[0].binding.name, sym("MAX"));
    assert!(extend.associated_values[0].binding.is_const());
    assert!(!extend.associated_values[0].binding.is_mutable());
}

#[test]
fn rejects_mutable_extend_associated_const_values() {
    let (_module, errors) = parse_module(
        r#"
extend usize {
    const mut shadow: usize = 1usize;
}
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("const bindings cannot be mutable")),
        "{errors:?}"
    );
}

#[test]
fn bodyless_extend_associated_const_values_require_builtin_extend() {
    let (module, errors) = parse_module(
        r#"
@[builtin("usize")]
extend usize {
    pub const MAX: usize;
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Extend(extend) = &module.items[0].kind else {
        panic!("expected extend");
    };
    assert_eq!(extend.associated_values.len(), 1);
    assert_eq!(extend.associated_values[0].binding.name, sym("MAX"));
    assert!(extend.associated_values[0].binding.value.is_none());

    let (_, errors) = parse_module(
        r#"
extend usize {
    pub const MAX: usize;
}
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("requires an initializer")),
        "{errors:?}"
    );
}

#[test]
fn bodyless_type_aliases_require_builtin_attribute() {
    let (module, errors) = parse_module(
        r#"
@[builtin("AsmConfig")]
pub type AsmConfig;
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::TypeAlias(alias) = &module.items[0].kind else {
        panic!("expected type alias");
    };
    assert_eq!(alias.name, sym("AsmConfig"));
    assert!(alias.ty.is_none());

    let (_, errors) = parse_module(
        r#"
type Token;
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected `=` in type alias")),
        "{errors:?}"
    );
}
