// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn parses_top_level_items() {
    let source = r#"
module math;

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

static banner = "nia\0";
extern static mut a: usize;
type Byte = u8;
fn main() i32 { 0 }
"#;
    let (module, errors) = parse_module(source);
    assert_eq!(errors, Vec::<ParseError>::new());
    assert_eq!(module.items.len(), 9);
    assert!(matches!(module.items[0].kind, ItemKind::Module(_)));
    assert!(matches!(&module.items[1].kind, ItemKind::Function(function) if function.is_extern));
    assert!(matches!(module.items[2].kind, ItemKind::Enum(_)));
    assert!(matches!(module.items[3].kind, ItemKind::Struct(_)));
    assert!(matches!(module.items[4].kind, ItemKind::Extend(_)));
    assert!(matches!(module.items[5].kind, ItemKind::Binding(_)));
    assert!(matches!(&module.items[6].kind, ItemKind::Binding(binding) if binding.is_extern()));
    assert!(matches!(module.items[7].kind, ItemKind::TypeAlias(_)));
    assert!(matches!(module.items[8].kind, ItemKind::Function(_)));
}

#[test]
fn syntax_tree_is_lossless_and_lowering_is_deterministic() {
    let source = "fn  main() i32 { // retained by syntax\n  0\n}\n";
    let parse = nia_syntax::parse(source, Some(version(1))).expect("grammar tree");
    let store = NodeStore::new();
    let first = lower_module(&parse, &store, nia_symbol_table::SymbolTable::new());
    let second = lower_module(&parse, &store, nia_symbol_table::SymbolTable::new());

    assert_eq!(parse.tree.full_text(), source);
    assert_eq!(first.errors, second.errors);
    assert_eq!(first.module, second.module);
    assert_eq!(first.origins, second.origins);
}

#[test]
fn incremental_edit_sequence_preserves_clean_ast_diagnostics_and_origins() {
    let source =
        "module first;\nfn main[T](x: T) T { value[index]; x }\nfn broken(x) {}\nmodule last;\n";
    let mut current_version = version(1);
    let mut incremental = nia_syntax::parse(source, Some(current_version)).expect("initial parse");
    let store = NodeStore::new();
    let edits = [
        ("index", "Box[N]"),
        ("Box[N]", "Wrap[Box[N]]"),
        ("Wrap[Box[N]]", "Box[N]"),
        ("value", "call"),
        ("broken(x)", "broken(x: i32)"),
        ("module first;", "// leading comment\nmodule first;"),
        ("call[Box[N]]", "call[Box[N], (]"),
        ("call[Box[N], (]", "call[index]"),
        ("module last;", "module before_last;\nmodule last;"),
        ("{ call[index]; x }", "{ if x { x } else { x } }"),
    ];
    for (old, new) in edits {
        let start = incremental.tree.source().find(old).expect("edit target");
        let edit = nia_syntax::TextEdit::replace(Span::new(start, start + old.len()), new);
        current_version.revision = SourceRevision(current_version.revision.0 + 1);
        incremental = incremental
            .reparse(&edit, Some(current_version))
            .expect("incremental parse");
        let clean = nia_syntax::parse(incremental.tree.source(), Some(current_version))
            .expect("clean parse");
        assert_eq!(incremental, clean, "edit {old:?} -> {new:?}");
        let incremental_ast =
            lower_module(&incremental, &store, nia_symbol_table::SymbolTable::new());
        let clean_ast = lower_module(&clean, &store, nia_symbol_table::SymbolTable::new());
        assert_eq!(
            incremental_ast.module, clean_ast.module,
            "edit {old:?} -> {new:?}"
        );
        assert_eq!(
            incremental_ast.errors, clean_ast.errors,
            "edit {old:?} -> {new:?}"
        );
        assert_eq!(
            incremental_ast.origins, clean_ast.origins,
            "edit {old:?} -> {new:?}"
        );
    }
}

#[test]
fn parse_errors_carry_red_token_node_keys() {
    let version = version(3);
    let lowered = lower_with("fn bad(value) {}", Some(version));

    let error = lowered
        .errors
        .iter()
        .find(|error| error.message.contains("expected `:` after parameter name"))
        .expect("parameter type error");
    let key = error.node_key.as_ref().expect("red token node key");
    assert_eq!(key.source_version(), version);
    assert!(matches!(
        key.position(),
        NodePosition::ChildPath(path) if !path.steps().is_empty()
    ));
}

#[test]
fn lowering_records_ast_origins_as_red_child_path_ranges() {
    let version = version(4);
    let LoweredModule {
        module,
        errors,
        origins,
    } = lower_with(
        r#"
fn main(a: i32) i32 {
    let mut x = a;
    x
}

"#,
        Some(version),
    );

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
        .locator(SyntaxKind::Expr, expr.span)
        .expect("tail expr origin");

    assert_eq!(key.source_version(), version);
    assert_eq!(key.kind(), SyntaxKind::Expr);
    assert!(matches!(
        key.position(),
        NodePosition::ChildPathRange { start, end }
            if !start.steps().is_empty() && !end.steps().is_empty()
    ));
}

#[test]
fn unversioned_syntax_uses_one_isolated_origin_identity_per_parse() {
    let LoweredModule {
        module,
        errors,
        origins,
    } = lower_with("fn main() i32 { 0 }", None);

    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let return_type = function.return_type.as_ref().expect("return type");
    let key = origins
        .locator(SyntaxKind::Type, return_type.span)
        .expect("return type origin");
    let item_key = origins
        .locator(SyntaxKind::Item, module.items[0].span)
        .expect("function item origin");
    assert_eq!(key.source_version().revision, SourceRevision::INITIAL);
    assert_eq!(key.source_version().id, item_key.source_version().id);

    let LoweredModule {
        module: second_module,
        errors: second_errors,
        origins: second_origins,
    } = lower_with("fn main() i32 { 0 }", None);
    assert!(second_errors.is_empty(), "{second_errors:?}");
    let ItemKind::Function(second_function) = &second_module.items[0].kind else {
        panic!("expected function");
    };
    let second_return_type = second_function.return_type.as_ref().expect("return type");
    let second_key = second_origins
        .locator(SyntaxKind::Type, second_return_type.span)
        .expect("return type origin");
    assert_ne!(key.source_version().id, second_key.source_version().id);
}

#[test]
fn speculative_readings_publish_only_retained_origins() {
    let source = "fn main() i32 { value[index]; 0 }";
    let LoweredModule {
        module,
        errors,
        origins,
    } = lower_with(source, Some(version(1)));

    assert!(errors.is_empty(), "{errors:?}");
    let span_of = |name: &str| {
        let start = source.find(name).expect("fixture expression name");
        nia_span::Span::new(start, start + name.len())
    };
    // `value` is only ever tried as a struct-literal type and then discarded.
    assert!(
        origins
            .locator(SyntaxKind::Type, span_of("value"))
            .is_none(),
        "discarded speculative type reading leaked an origin"
    );
    // The bracket argument keeps both readings, so both are published.
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("function body");
    let StmtKind::Expr(expr) = &body.stmts[0].kind else {
        panic!("expected expression statement");
    };
    let ExprKind::BracketSuffix { args, .. } = &expr.kind else {
        panic!("expected bracket suffix");
    };
    let ty = args[0].ty.as_ref().expect("retained type reading");
    assert_eq!(
        origins.locator(SyntaxKind::Type, span_of("index")),
        Some(ty.node_key.clone())
    );
    assert!(
        origins
            .locator(SyntaxKind::Expr, span_of("index"))
            .is_some()
    );
}

#[test]
fn failed_item_recovery_does_not_publish_partial_ast_origins() {
    let source = "fn broken(value: Input) Output {";
    let LoweredModule {
        module,
        errors,
        origins,
    } = lower_with(source, Some(version(1)));

    assert!(module.items.is_empty());
    assert!(!errors.is_empty());
    for name in ["Input", "Output"] {
        let start = source.find(name).expect("fixture type name");
        assert!(
            origins
                .locator(
                    SyntaxKind::Type,
                    nia_span::Span::new(start, start + name.len())
                )
                .is_none(),
            "failed item leaked the partial `{name}` type origin"
        );
    }
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
    assert_eq!(item_enum.variants[0].name, sym("A"));
    assert_eq!(item_enum.variants[1].name, sym("B"));
}
