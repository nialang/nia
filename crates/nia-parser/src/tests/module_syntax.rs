// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

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
    fn len2(&self) i32 {
        self.x * self.x + self.y * self.y
    }
}

let banner = "nia\0";
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
