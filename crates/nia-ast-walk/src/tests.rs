// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{ExprKind, Pattern, TypeRef};
use nia_parser::parse_module;

use super::*;

struct StructuralCollector<'source> {
    source: &'source str,
    patterns: Vec<String>,
    integer_exprs: Vec<String>,
    types: Vec<String>,
}

impl<'ast> Visitor<'ast> for StructuralCollector<'_> {
    fn visit_pattern(&mut self, pattern: &'ast Pattern) {
        self.patterns
            .push(self.text(pattern.span.start, pattern.span.end));
        walk_pattern(self, pattern);
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        if let ExprKind::Integer(value) = &expr.kind {
            self.integer_exprs.push(value.clone());
        }
        walk_expr(self, expr);
    }

    fn visit_type(&mut self, ty: &'ast TypeRef) {
        self.types.push(self.text(ty.span.start, ty.span.end));
        walk_type(self, ty);
    }
}

impl StructuralCollector<'_> {
    fn text(&self, start: usize, end: usize) -> String {
        self.source[start..end].to_owned()
    }
}

fn parse(source: &str) -> Module {
    let (module, errors) = parse_module(source);
    assert!(errors.is_empty(), "{errors:?}");
    module
}

#[test]
fn visits_patterns_from_every_pattern_owning_ast_node() {
    let source = r#"
struct Point { x: i32, y: i32 }

fn inspect(points: [Point; 1], point: Point) i32 {
    let Point { x: 101, y: 102..=103 } = point;
    for Point { x: 201, y: 202 } in points {}
    let selected = if point is Point { x: 301, y: 302 } { 1 } else { 0 };
    match point {
        Point { x: 401, y: 402 } => selected,
    }
}
"#;
    let module = parse(source);
    let mut collector = StructuralCollector {
        source,
        patterns: Vec::new(),
        integer_exprs: Vec::new(),
        types: Vec::new(),
    };
    walk_module(&mut collector, &module);

    for pattern in [
        "Point { x: 101, y: 102..=103 }",
        "Point { x: 201, y: 202 }",
        "Point { x: 301, y: 302 }",
        "Point { x: 401, y: 402 }",
    ] {
        assert!(collector.patterns.iter().any(|visited| visited == pattern));
    }
    for value in [
        "101", "102", "103", "201", "202", "301", "302", "401", "402",
    ] {
        assert!(
            collector
                .integer_exprs
                .iter()
                .any(|visited| visited == value)
        );
    }
}

#[test]
fn visits_const_generic_types_for_every_generic_owner() {
    let source = r#"
struct Record[N: kinds::Record] {}
union Storage[N: kinds::Storage] {}
trait Inspect[N: kinds::Inspect] {}
extend[N: kinds::Extension] Record[1] {}
type Alias[N: kinds::Alias] = Record[1];
fn run[N: kinds::Function]() () {}
"#;
    let module = parse(source);
    let mut collector = StructuralCollector {
        source,
        patterns: Vec::new(),
        integer_exprs: Vec::new(),
        types: Vec::new(),
    };
    walk_module(&mut collector, &module);

    for ty in [
        "kinds::Record",
        "kinds::Storage",
        "kinds::Inspect",
        "kinds::Extension",
        "kinds::Alias",
        "kinds::Function",
    ] {
        assert_eq!(
            collector
                .types
                .iter()
                .filter(|visited| *visited == ty)
                .count(),
            1
        );
    }
}

#[test]
fn finds_static_bindings_nested_in_expression_blocks_and_closures() {
    let source = r#"
fn collect() () {
    static root: i32 = 0;
    let root_value = 0;
    let closure = \[root_value] -> { static nested: i32 = 1; };
    if true { static branch: i32 = 2; }
    match true {
        true => { static arm: i32 = 3; }
    }
}
"#;
    let module = parse(source);
    let function = match &module.items[0].kind {
        nia_ast::ItemKind::Function(function) => function,
        other => panic!("expected function, got {other:?}"),
    };
    let body = function.body.as_ref().expect("function body");
    let mut declarations = Vec::new();
    walk_static_bindings(body, &mut |stmt| {
        if matches!(stmt.kind, nia_ast::StmtKind::Static(_)) {
            declarations.push(source[stmt.span.start..stmt.span.end].to_owned());
        }
    });

    assert_eq!(
        declarations,
        [
            "static root: i32 = 0;",
            "static nested: i32 = 1;",
            "static branch: i32 = 2;",
            "static arm: i32 = 3;",
        ]
    );
}
