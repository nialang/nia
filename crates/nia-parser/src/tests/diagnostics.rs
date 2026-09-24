// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn reports_lexer_errors_through_parser() {
    let (_module, errors) = parse_module(r#"fn main() { let mut x = "\q"; }"#);
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("invalid escape in string literal"))
    );
}

#[test]
fn reports_misplaced_numeric_separators_through_parser() {
    let (_module, errors) = parse_module("fn main() { let value = 1e_2; }");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("invalid numeric literal")),
        "{errors:?}"
    );
}

#[test]
fn rejects_string_module_name() {
    let (_module, errors) = parse_module(r#"module "math";"#);
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected module name")),
        "{errors:?}"
    );
}

#[test]
fn rejects_removed_comptime_keyword() {
    let (_module, errors) = parse_module("comptime width: usize = 4;");
    assert!(
        !errors.is_empty(),
        "legacy `comptime` syntax must not parse"
    );
}

#[test]
fn rejects_deep_relative_using_prefix() {
    let (_module, errors) = parse_module("using super..math;");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected `;` after using")),
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
            .filter(|error| error.message.contains("must be written as `&fn(...)`"))
            .count(),
        1,
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
fn expression_recovery_keeps_later_statements_and_items() {
    let (module, errors) = parse_module(
        r#"
fn main() () {
    ;
    retained();
}

fn retained() () {}
fn later() () {}
"#,
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].message.contains("expected expression"),
        "{errors:?}"
    );

    let ItemKind::Function(main) = &module.items[0].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("expected main body");
    assert_eq!(body.stmts.len(), 1);

    let ItemKind::Function(retained) = &module.items[1].kind else {
        panic!("expected retained function");
    };
    assert_eq!(retained.name, sym("retained"));
    let ItemKind::Function(later) = &module.items[2].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn failed_control_flow_recovery_keeps_later_statements_and_items() {
    let (module, errors) = parse_module(
        r#"
fn main() () {
    if + {
        discarded();
    }
    retained();
}

fn later() () {}
"#,
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].message.contains("expected expression"),
        "{errors:?}"
    );

    let ItemKind::Function(main) = &module.items[0].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("expected main body");
    assert_eq!(body.stmts.len(), 1);
    let StmtKind::Expr(expr) = &body.stmts[0].kind else {
        panic!("expected retained expression statement");
    };
    assert!(matches!(expr.kind, ExprKind::Call { .. }));

    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn failed_return_expression_recovery_keeps_later_statements_and_items() {
    let (module, errors) = parse_module(
        r#"
fn main() () {
    return + {
        discarded();
    };
    retained();
}

fn later() () {}
"#,
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].message.contains("expected expression"),
        "{errors:?}"
    );

    let ItemKind::Function(main) = &module.items[0].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("expected main body");
    assert_eq!(body.stmts.len(), 1);
    let StmtKind::Expr(expr) = &body.stmts[0].kind else {
        panic!("expected retained expression statement");
    };
    assert!(matches!(expr.kind, ExprKind::Call { .. }));

    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn reports_expected_token_and_actionable_help() {
    let (_module, errors) = parse_module("fn main() { let value = 1 }");
    let error = errors
        .iter()
        .find(|error| error.message.contains("expected `;` after binding"))
        .expect("missing semicolon diagnostic");
    assert!(error.message.contains("found `}`"), "{error:?}");
    assert_eq!(error.kind, ParseErrorKind::MissingSemicolon);

    let diagnostic = error.to_diagnostic();
    assert_eq!(diagnostic.code.as_str(), "E0101");
    assert_eq!(diagnostic.primary_span(), Some(error.span));
    assert_eq!(
        diagnostic.help.as_slice(),
        ["add the missing `;` to terminate this declaration or statement"]
    );
}

#[test]
fn member_recovery_keeps_later_fields_after_a_missing_type() {
    let (module, errors) =
        parse_module("struct Recovered { missing: , retained: i32, }\nfn later() {}");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].message.contains("expected type"), "{errors:?}");
    let ItemKind::Struct(item) = &module.items[0].kind else {
        panic!("expected recovered struct item");
    };
    assert_eq!(item.fields.len(), 1);
    assert_eq!(item.fields[0].name, sym("retained"));
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected later function item");
    };
    assert_eq!(function.name, sym("later"));
}

#[test]
fn tuple_payload_recovery_keeps_later_types_and_declarations() {
    let (module, errors) =
        parse_module("struct Pair(, i32); enum Value { Broken(, bool), Kept } fn later() {}");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected type"))
            .count(),
        2,
        "{errors:?}"
    );

    let ItemKind::Struct(pair) = &module.items[0].kind else {
        panic!("expected recovered tuple struct");
    };
    assert_eq!(pair.fields.len(), 1);

    let ItemKind::Enum(value) = &module.items[1].kind else {
        panic!("expected recovered enum");
    };
    assert_eq!(value.variants.len(), 2);
    let nia_ast::EnumVariantPayload::Tuple(payload) = &value.variants[0].payload else {
        panic!("expected tuple payload");
    };
    assert_eq!(payload.len(), 1);
    assert_eq!(value.variants[1].name, sym("Kept"));

    let ItemKind::Function(later) = &module.items[2].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn tuple_payload_delimiter_recovery_keeps_later_types() {
    let (module, errors) = parse_module(
        "struct Pair(i32 bool)\nenum Value { Broken(i32 bool), Kept }\nfn later() () {}",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error
                .message
                .contains("expected `,` or `)` after tuple field"))
            .count(),
        2,
        "{errors:?}"
    );
    let ItemKind::Struct(pair) = &module.items[0].kind else {
        panic!("expected tuple struct");
    };
    assert_eq!(pair.fields.len(), 2);
    let ItemKind::Enum(value) = &module.items[1].kind else {
        panic!("expected enum");
    };
    let EnumVariantPayload::Tuple(payload) = &value.variants[0].payload else {
        panic!("expected tuple payload");
    };
    assert_eq!(payload.len(), 2);
    assert_eq!(value.variants[1].name, sym("Kept"));
    let ItemKind::Function(later) = &module.items[2].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn enum_variant_name_recovery_keeps_later_variants_and_declarations() {
    let (module, errors) = parse_module("enum Recovered { , Kept, Also }\nfn later() () {}");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].message.contains("expected enum variant"),
        "{errors:?}"
    );

    let ItemKind::Enum(recovered) = &module.items[0].kind else {
        panic!("expected recovered enum");
    };
    assert_eq!(recovered.variants.len(), 2);
    assert_eq!(recovered.variants[0].name, sym("Kept"));
    assert_eq!(recovered.variants[1].name, sym("Also"));

    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn enum_variant_delimiter_recovery_keeps_later_variants() {
    let (module, errors) = parse_module("enum Recovered { First Second, Third }\nfn later() () {}");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error
                .message
                .contains("expected `,` or `}` after enum variant"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Enum(recovered) = &module.items[0].kind else {
        panic!("expected recovered enum");
    };
    assert_eq!(
        recovered
            .variants
            .iter()
            .map(|variant| variant.name)
            .collect::<Vec<_>>(),
        vec![sym("First"), sym("Second"), sym("Third")]
    );
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn field_delimiter_recovery_keeps_later_fields_and_items() {
    let (module, errors) = parse_module(
        "struct Recovered { first: i32 second: bool, third: u8 }\nenum Payload { Item { first: i32 second: bool, third: u8 } }\nfn later() () {}",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected `,` or `}` after field"))
            .count(),
        2,
        "{errors:?}"
    );
    let ItemKind::Struct(recovered) = &module.items[0].kind else {
        panic!("expected recovered struct");
    };
    assert_eq!(
        recovered
            .fields
            .iter()
            .map(|field| field.name)
            .collect::<Vec<_>>(),
        vec![sym("first"), sym("second"), sym("third")]
    );
    let ItemKind::Enum(payload) = &module.items[1].kind else {
        panic!("expected recovered enum");
    };
    let EnumVariantPayload::Named(fields) = &payload.variants[0].payload else {
        panic!("expected named enum payload");
    };
    assert_eq!(
        fields.iter().map(|field| field.name).collect::<Vec<_>>(),
        vec![sym("first"), sym("second"), sym("third")]
    );
    let ItemKind::Function(later) = &module.items[2].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn tuple_type_recovery_keeps_later_elements_and_the_function() {
    let (module, errors) = parse_module("fn retained(value: (, i32,, bool)) () {}\nfn later() {}");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected type"))
            .count(),
        2,
        "{errors:?}"
    );
    let ItemKind::Function(retained) = &module.items[0].kind else {
        panic!("expected retained function");
    };
    let Some(ty) = retained.params[0].ty.as_ref() else {
        panic!("expected recovered parameter type");
    };
    let TypeKind::Tuple { elems } = &ty.kind else {
        panic!("expected recovered tuple type");
    };
    assert_eq!(elems.len(), 2);

    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn tuple_type_recovery_keeps_elements_after_nested_invalid_first_type() {
    let (module, errors) =
        parse_module("fn retained(value: (@ (bad, value), bool)) () {}\nfn later() () {}");
    assert!(!errors.is_empty(), "invalid first type must be diagnosed");
    let ItemKind::Function(retained) = &module.items[0].kind else {
        panic!("expected retained function");
    };
    let Some(ty) = retained.params[0].ty.as_ref() else {
        panic!("expected recovered parameter type");
    };
    let TypeKind::Tuple { elems } = &ty.kind else {
        panic!("expected recovered tuple type");
    };
    assert_eq!(elems.len(), 1, "{errors:?}");
    assert!(matches!(elems[0].kind, TypeKind::Path { .. }));
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn tuple_expression_and_pattern_recovery_keeps_later_statements() {
    let (module, errors) = parse_module(
        "fn main() () { let value = (, 1,, true); let (, retained,, last) = value; retained; }",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected expression")
                || error.message.contains("expected binding pattern"))
            .count(),
        4,
        "{errors:?}"
    );
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    assert_eq!(body.stmts.len(), 3);
}

#[test]
fn tuple_expression_recovery_ignores_nested_element_delimiters() {
    let (module, errors) = parse_module(
        "fn main() () { let tuple = (@ (bad, value), true); later(); } fn later() () {}",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected expression"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(main) = &module.items[0].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("expected body");
    let StmtKind::Binding(binding) = &body.stmts[0].kind else {
        panic!("expected binding: {body:?}");
    };
    let ExprKind::Tuple(elems) = &binding.value.as_ref().expect("tuple").kind else {
        panic!("expected tuple expression");
    };
    assert_eq!(elems.len(), 1, "{errors:?}");
    assert_eq!(body.stmts.len(), 2, "{errors:?}");
}

#[test]
fn tuple_expression_delimiter_recovery_keeps_later_elements_and_statements() {
    let (module, errors) =
        parse_module("fn main() () { let value = (true false); later(); } fn later() () {}");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0]
            .message
            .contains("expected `,` or `)` after tuple element"),
        "{errors:?}"
    );
    let ItemKind::Function(main) = &module.items[0].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("expected body");
    assert_eq!(body.stmts.len(), 2, "{errors:?}");
    let StmtKind::Binding(binding) = &body.stmts[0].kind else {
        panic!("expected tuple binding");
    };
    let ExprKind::Tuple(elems) = &binding.value.as_ref().expect("tuple initializer").kind else {
        panic!("expected tuple expression");
    };
    assert_eq!(elems.len(), 2);
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn pattern_delimiter_recovery_keeps_later_patterns_and_arms() {
    let (module, errors) = parse_module(
        "struct Pair(bool, bool)\nstruct Point { x: bool, y: bool }\nfn main(value: (bool, bool)) () { match value { (true false) => (), Pair(true false) => (), Point { x: true y: false } => (), _ => (), } }",
    );
    assert_eq!(errors.len(), 3, "{errors:?}");
    assert!(
        errors.iter().take(2).all(|error| error
            .message
            .contains("expected `,` or `)` after tuple pattern")),
        "{errors:?}"
    );
    assert!(
        errors[2]
            .message
            .contains("expected `,` or `}` after nominal pattern field"),
        "{errors:?}"
    );
    let ItemKind::Function(main) = &module.items[2].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("expected body");
    assert!(body.stmts.is_empty(), "{errors:?}");
    let Some(tail) = &body.tail else {
        panic!("expected match tail");
    };
    let ExprKind::Match(matched) = &tail.kind else {
        panic!("expected match expression");
    };
    assert_eq!(matched.arms.len(), 4, "{errors:?}");
}

#[test]
fn call_argument_recovery_keeps_later_arguments_and_statements() {
    let (module, errors) = parse_module(
        "fn main() () { consume(, 1,, true); later(); } fn later() () {} fn consume(a: i32, b: bool) () {}",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected expression"))
            .count(),
        2,
        "{errors:?}"
    );
    let ItemKind::Function(main) = &module.items[0].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("expected body");
    assert_eq!(body.stmts.len(), 2);
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn call_argument_delimiter_recovery_keeps_later_arguments_and_statements() {
    let (module, errors) = parse_module(
        "fn main() () { consume(true false); later(); } fn consume(first: bool, second: bool) () {} fn later() () {}",
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0]
            .message
            .contains("expected `,` or `)` after call argument"),
        "{errors:?}"
    );
    let ItemKind::Function(main) = &module.items[0].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("expected body");
    assert_eq!(body.stmts.len(), 2, "{errors:?}");
    let StmtKind::Expr(call) = &body.stmts[0].kind else {
        panic!("expected call statement");
    };
    let ExprKind::Call { args, .. } = &call.kind else {
        panic!("expected call expression");
    };
    assert_eq!(args.len(), 2);
    let ItemKind::Function(later) = &module.items[2].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn array_and_struct_literal_recovery_keeps_later_members() {
    let (module, errors) = parse_module(
        "struct Point { x: i32, y: bool }\nfn main() () { let array = [, 1,, true]; let point = Point { x: , y: true }; later(); } fn later() () {}",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected expression"))
            .count(),
        3,
        "{errors:?}"
    );
    let ItemKind::Function(main) = &module.items[1].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("expected body");
    assert_eq!(body.stmts.len(), 3);
    let ItemKind::Function(later) = &module.items[2].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn array_literal_recovery_ignores_nested_element_delimiters() {
    let (module, errors) = parse_module(
        "fn main() () { let array = [@ (bad, value), true]; later(); } fn later() () {}",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected expression"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(main) = &module.items[0].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("expected body");
    let StmtKind::Binding(binding) = &body.stmts[0].kind else {
        panic!("expected binding: {body:?}");
    };
    let ExprKind::ArrayLiteral {
        elems: nia_ast::ArrayElements::List(elems),
    } = &binding.value.as_ref().expect("array").kind
    else {
        panic!("expected array literal");
    };
    assert_eq!(elems.len(), 1, "{errors:?}");
    assert_eq!(body.stmts.len(), 2, "{errors:?}");
}

#[test]
fn aggregate_literal_delimiter_recovery_keeps_later_members_and_statements() {
    let (module, errors) = parse_module(
        "struct Point { x: i32, y: bool }\nfn main() () { let array = [true false]; let point = Point { x: 1i32 y: true }; later(); } fn later() () {}",
    );
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(
        errors[0].message.contains("expected `,` or `]`"),
        "{errors:?}"
    );
    assert!(
        errors[1].message.contains("expected `,` or `}`"),
        "{errors:?}"
    );
    let ItemKind::Function(main) = &module.items[1].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("expected body");
    assert_eq!(body.stmts.len(), 3, "{errors:?}");
    let StmtKind::Binding(array) = &body.stmts[0].kind else {
        panic!("expected array binding");
    };
    let ExprKind::ArrayLiteral {
        elems: nia_ast::ArrayElements::List(elems),
    } = &array.value.as_ref().expect("array initializer").kind
    else {
        panic!("expected array literal");
    };
    assert_eq!(elems.len(), 2);
    let StmtKind::Binding(point) = &body.stmts[1].kind else {
        panic!("expected point binding");
    };
    let ExprKind::TypedStructLiteral { fields, .. } = &point.value.as_ref().expect("point").kind
    else {
        panic!("expected typed struct literal");
    };
    assert_eq!(fields.len(), 2);
    let ItemKind::Function(later) = &module.items[2].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn aggregate_literal_recovery_ignores_nested_member_delimiters() {
    let (module, errors) = parse_module(
        "struct Point { x: i32, y: bool }\nfn main() () { let point = Point { x: @ (bad, value), y: true }; later(); } fn later() () {}",
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected")),
        "{errors:?}"
    );
    let ItemKind::Function(main) = &module.items[1].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("expected body");
    assert_eq!(body.stmts.len(), 2, "{errors:?}");
    let StmtKind::Binding(point) = &body.stmts[0].kind else {
        panic!("expected point binding");
    };
    let ExprKind::TypedStructLiteral { fields, .. } = &point.value.as_ref().expect("point").kind
    else {
        panic!("expected typed struct literal");
    };
    assert_eq!(fields.len(), 1, "{errors:?}");
    assert_eq!(fields[0].name, sym("y"));
}

#[test]
fn callable_type_recovery_keeps_later_parameters_and_the_function() {
    let (module, errors) = parse_module(
        "fn retained(pointer: &fn(, i32), callable: Fn(, bool)) () {}\nfn later() () {}",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected type"))
            .count(),
        2,
        "{errors:?}"
    );
    let ItemKind::Function(retained) = &module.items[0].kind else {
        panic!("expected retained function");
    };
    assert_eq!(retained.params.len(), 2);
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn generic_parameter_recovery_keeps_later_parameters_and_the_function() {
    let (module, errors) = parse_module("fn retained[, Kept]() () {}\nfn later() () {}");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected generic parameter"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(retained) = &module.items[0].kind else {
        panic!("expected retained function");
    };
    assert_eq!(retained.generics.len(), 1);
    assert_eq!(retained.generics[0].name, sym("Kept"));
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn where_predicate_recovery_keeps_later_predicates_and_the_function() {
    let (module, errors) = parse_module("fn retained[T]() () where : T, T: T {}\nfn later() () {}");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected type"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(retained) = &module.items[0].kind else {
        panic!("expected retained function");
    };
    assert_eq!(retained.where_clause.predicates.len(), 1);
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn where_predicate_delimiter_recovery_keeps_later_predicates() {
    let (module, errors) =
        parse_module("fn retained[T]() () where T: Copy U: Clone, V: Send {}\nfn later() () {}");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected `,` after where predicate"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(retained) = &module.items[0].kind else {
        panic!("expected retained function");
    };
    assert_eq!(retained.where_clause.predicates.len(), 3);
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn associated_type_argument_recovery_keeps_later_arguments_and_the_function() {
    let (module, errors) =
        parse_module("fn retained(value: Wrapper[Item = , i32]) () {}\nfn later() () {}");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error
                .message
                .contains("expected associated type binding value"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(retained) = &module.items[0].kind else {
        panic!("expected retained function");
    };
    let Some(ty) = retained.params[0].ty.as_ref() else {
        panic!("expected recovered parameter type");
    };
    let TypeKind::Path { segments } = &ty.kind else {
        panic!("expected path type");
    };
    assert_eq!(segments[0].args.len(), 1);
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn generic_parameter_delimiter_recovery_keeps_the_function() {
    let (module, errors) = parse_module("fn retained[T U]() () {}\nfn later() () {}");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error
                .message
                .contains("expected `,` or `]` after generic parameter"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(retained) = &module.items[0].kind else {
        panic!("expected retained function");
    };
    assert_eq!(retained.generics.len(), 2);
    assert_eq!(
        nia_ast::generic_param_names(&retained.generics),
        vec![sym("T"), sym("U")]
    );
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn generic_parameter_delimiter_recovery_keeps_later_parameters() {
    let (module, errors) =
        parse_module("fn retained[T U, N: i32 M: bool]() () {}\nfn later() () {}");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error
                .message
                .contains("expected `,` or `]` after generic parameter"))
            .count(),
        2,
        "{errors:?}"
    );
    let ItemKind::Function(retained) = &module.items[0].kind else {
        panic!("expected retained function");
    };
    assert_eq!(
        nia_ast::generic_param_names(&retained.generics),
        vec![sym("T"), sym("U"), sym("N"), sym("M")]
    );
    assert!(retained.generics[2].is_const());
    assert!(retained.generics[3].is_const());
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn using_group_recovery_keeps_later_members_and_items() {
    let (module, errors) = parse_module("using {kept, , later as alias};\nfn after() () {}");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected name in `using`"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Using(using) = &module.items[0].kind else {
        panic!("expected using item");
    };
    let nia_ast::UsingSelector::Group(items) = &using.selector else {
        panic!("expected using group");
    };
    assert_eq!(items.len(), 2);
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(function.name, sym("after"));
}

#[test]
fn using_group_delimiter_recovery_keeps_later_members_and_items() {
    let (module, errors) = parse_module("using {kept later};\nfn after() () {}");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0]
            .message
            .contains("expected `,` or `}` after using selector"),
        "{errors:?}"
    );
    let ItemKind::Using(using) = &module.items[0].kind else {
        panic!("expected using item");
    };
    let nia_ast::UsingSelector::Group(items) = &using.selector else {
        panic!("expected using group");
    };
    assert_eq!(items.len(), 2, "{errors:?}");
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(function.name, sym("after"));
}

#[test]
fn using_group_recovery_ignores_nested_member_delimiters() {
    let (module, errors) = parse_module("using {(bad, value), kept};\nfn after() () {}");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected name in `using`"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Using(using) = &module.items[0].kind else {
        panic!("expected using item");
    };
    let nia_ast::UsingSelector::Group(items) = &using.selector else {
        panic!("expected using group");
    };
    assert_eq!(items.len(), 1, "{errors:?}");
    let ItemKind::Function(after) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(after.name, sym("after"));
}

#[test]
fn attribute_argument_recovery_keeps_later_arguments_and_item() {
    let (module, errors) = parse_module("@[custom(, true)]\nfn retained() () {}\nfn later() () {}");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected expression"))
            .count(),
        1,
        "{errors:?}"
    );
    let item = &module.items[0];
    let ItemKind::Function(_) = &item.kind else {
        panic!("expected retained function");
    };
    assert_eq!(item.attributes.len(), 1);
    let nia_ast::AttributeKind::Meta(meta) = &item.attributes[0].kind else {
        panic!("expected metadata attribute");
    };
    assert_eq!(meta.args.len(), 1);
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn attribute_argument_delimiter_recovery_keeps_later_arguments_and_item() {
    let (module, errors) =
        parse_module("@[custom(true false)]\nfn retained() () {}\nfn later() () {}");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0]
            .message
            .contains("expected `,` or `)` after attribute argument"),
        "{errors:?}"
    );
    let item = &module.items[0];
    let ItemKind::Function(_) = &item.kind else {
        panic!("expected retained function");
    };
    let nia_ast::AttributeKind::Meta(meta) = &item.attributes[0].kind else {
        panic!("expected metadata attribute");
    };
    assert_eq!(meta.args.len(), 2);
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn attribute_argument_recovery_ignores_nested_argument_delimiters() {
    let (module, errors) =
        parse_module("@[custom(@ [bad, value], true)]\nfn retained() () {}\nfn later() () {}");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected expression"))
            .count(),
        1,
        "{errors:?}"
    );
    let item = &module.items[0];
    let nia_ast::AttributeKind::Meta(meta) = &item.attributes[0].kind else {
        panic!("expected metadata attribute");
    };
    assert_eq!(meta.args.len(), 1, "{errors:?}");
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn closure_capture_recovery_keeps_later_captures_and_items() {
    let (module, errors) = parse_module(
        "fn retained() () { let closure = \\[kept, , later] value -> kept; }\nfn after() () {}",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected capture name"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(retained) = &module.items[0].kind else {
        panic!("expected retained function");
    };
    let body = retained.body.as_ref().expect("expected retained body");
    let StmtKind::Binding(binding) = &body.stmts[0].kind else {
        panic!("expected closure binding");
    };
    let ExprKind::Closure { captures, .. } = &binding.value.as_ref().expect("closure").kind else {
        panic!("expected closure expression");
    };
    assert_eq!(captures.len(), 2);
    let ItemKind::Function(after) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(after.name, sym("after"));
}

#[test]
fn closure_capture_delimiter_recovery_keeps_later_captures() {
    let (module, errors) =
        parse_module("fn retained() () { let closure = \\[kept later] value -> kept; }");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error
                .message
                .contains("expected `,` or `]` after closure capture"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let StmtKind::Binding(binding) = &body.stmts[0].kind else {
        panic!("expected closure binding");
    };
    let ExprKind::Closure { captures, .. } = &binding.value.as_ref().expect("closure").kind else {
        panic!("expected closure expression");
    };
    assert_eq!(captures.len(), 2);
}

#[test]
fn bracket_argument_recovery_keeps_later_arguments_and_items() {
    let (module, errors) = parse_module(
        "struct Wrapper[T, U] {}\nfn retained(value: Wrapper[i32, , bool]) () {}\nfn later() () {}",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected expression"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(retained) = &module.items[1].kind else {
        panic!("expected retained function");
    };
    let ty = retained.params[0].ty.as_ref().expect("parameter type");
    let TypeKind::Path { segments } = &ty.kind else {
        panic!("expected wrapper path");
    };
    assert_eq!(segments[0].args.len(), 2);
    let ItemKind::Function(later) = &module.items[2].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn bracket_argument_delimiter_recovery_keeps_later_arguments_and_items() {
    let (module, errors) =
        parse_module("fn main() () { let value = target[true false]; later(); } fn later() () {}");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0]
            .message
            .contains("expected `,` or `]` after bracket argument"),
        "{errors:?}"
    );
    let ItemKind::Function(main) = &module.items[0].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("expected body");
    assert_eq!(body.stmts.len(), 2, "{errors:?}");
    let StmtKind::Binding(binding) = &body.stmts[0].kind else {
        panic!("expected bracket binding");
    };
    let ExprKind::BracketSuffix { args, .. } = &binding.value.as_ref().expect("value").kind else {
        panic!("expected bracket suffix");
    };
    assert_eq!(args.len(), 2);
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn bracket_argument_recovery_ignores_nested_argument_delimiters() {
    let (module, errors) = parse_module(
        "fn main() () { let value = target[@ (bad, value), true]; later(); } fn later() () {}",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected bracket argument"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(main) = &module.items[0].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("expected body");
    let StmtKind::Binding(binding) = &body.stmts[0].kind else {
        panic!("expected binding: {body:?}");
    };
    let ExprKind::BracketSuffix { args, .. } = &binding.value.as_ref().expect("value").kind else {
        panic!("expected bracket suffix");
    };
    assert_eq!(args.len(), 1, "{errors:?}");
    assert!(args[0].expr.is_some(), "{errors:?}");
    assert_eq!(body.stmts.len(), 2, "{errors:?}");
}

#[test]
fn match_pattern_recovery_keeps_later_patterns_and_statements() {
    let (module, errors) = parse_module(
        "fn main(value: bool) () { match value { , true => (), false => (), } later(); }\nfn later() () {}",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected expression"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(main) = &module.items[0].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("main body");
    assert_eq!(body.stmts.len(), 2);
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn match_pattern_delimiter_recovery_keeps_later_patterns_and_arms() {
    let (module, errors) =
        parse_module("fn main(value: bool) () { match value { true false => (), _ => (), } }");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0]
            .message
            .contains("expected `,` or `=>` after match pattern"),
        "{errors:?}"
    );
    let ItemKind::Function(main) = &module.items[0].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("expected body");
    let Some(tail) = &body.tail else {
        panic!("expected match tail");
    };
    let ExprKind::Match(matched) = &tail.kind else {
        panic!("expected match expression");
    };
    assert_eq!(matched.arms.len(), 2, "{errors:?}");
}

#[test]
fn match_arm_body_recovery_keeps_later_arms_and_statements() {
    let (module, errors) = parse_module(
        "fn main(value: bool) () { match value { true => , false => (), } later(); }\nfn later() () {}",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected expression"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(main) = &module.items[0].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("main body");
    assert_eq!(body.stmts.len(), 2);
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn match_tuple_pattern_recovery_keeps_later_fields() {
    let (module, errors) = parse_module(
        "fn main(value: (bool, bool)) () { match value { (true, , false) => (), _ => (), } }",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected expression"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(main) = &module.items[0].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("main body");
    let Some(tail) = &body.tail else {
        panic!("expected match tail");
    };
    let ExprKind::Match(matched) = &tail.kind else {
        panic!("expected match expression");
    };
    assert_eq!(matched.arms.len(), 2);
    let PatternKind::Tuple(fields) = &matched.arms[0].patterns[0].kind else {
        panic!("expected tuple pattern");
    };
    assert_eq!(fields.len(), 2);
    assert!(matches!(
        matched.arms[1].patterns[0].kind,
        PatternKind::Wildcard
    ));
}

#[test]
fn nominal_tuple_pattern_recovery_keeps_later_fields() {
    let (module, errors) = parse_module(
        "fn main(value: bool) () { match value { Pair(true, , false) => (), _ => (), } }",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected expression"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(main) = &module.items[0].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("main body");
    let Some(tail) = &body.tail else {
        panic!("expected match tail");
    };
    let ExprKind::Match(matched) = &tail.kind else {
        panic!("expected match expression");
    };
    assert_eq!(matched.arms.len(), 2);
    let PatternKind::Nominal {
        fields: nia_ast::NominalPatternFields::Tuple(fields),
        ..
    } = &matched.arms[0].patterns[0].kind
    else {
        panic!("expected nominal tuple pattern");
    };
    assert_eq!(fields.len(), 2);
    assert!(matches!(
        matched.arms[1].patterns[0].kind,
        PatternKind::Wildcard
    ));
}

#[test]
fn nominal_named_pattern_recovery_keeps_later_fields() {
    let (module, errors) = parse_module(
        "fn main(value: bool) () { match value { Point { x: , y: true } => (), _ => (), } }",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected expression"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(main) = &module.items[0].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("main body");
    let Some(tail) = &body.tail else {
        panic!("expected match tail");
    };
    let ExprKind::Match(matched) = &tail.kind else {
        panic!("expected match expression");
    };
    assert_eq!(matched.arms.len(), 2);
    let PatternKind::Nominal {
        fields: nia_ast::NominalPatternFields::Named { fields, .. },
        ..
    } = &matched.arms[0].patterns[0].kind
    else {
        panic!("expected nominal named pattern");
    };
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, sym("y"));
    assert!(matches!(
        matched.arms[1].patterns[0].kind,
        PatternKind::Wildcard
    ));
}

#[test]
fn tuple_pattern_recovery_keeps_fields_after_nested_invalid_first_pattern() {
    let (module, errors) = parse_module(
        "fn main(value: (bool, bool)) () { match value { (@ (bad, value), true) => (), _ => (), } later(); } fn later() () {}",
    );
    assert!(
        !errors.is_empty(),
        "invalid first pattern must be diagnosed"
    );
    let ItemKind::Function(main) = &module.items[0].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("main body");
    assert_eq!(body.stmts.len(), 2, "{errors:?}");
    let StmtKind::Expr(match_stmt) = &body.stmts[0].kind else {
        panic!("expected match statement");
    };
    let ExprKind::Match(matched) = &match_stmt.kind else {
        panic!("expected match expression");
    };
    assert_eq!(matched.arms.len(), 2, "{errors:?}");
    let PatternKind::Tuple(fields) = &matched.arms[0].patterns[0].kind else {
        panic!("expected tuple pattern");
    };
    assert_eq!(fields.len(), 1, "{errors:?}");
    assert!(matches!(fields[0].kind, PatternKind::Expr(_)));
    let StmtKind::Expr(later_stmt) = &body.stmts[1].kind else {
        panic!("expected retained later statement");
    };
    assert!(matches!(later_stmt.kind, ExprKind::Call { .. }));
}

#[test]
fn irrefutable_tuple_pattern_recovery_keeps_fields_after_nested_invalid_first_pattern() {
    let (module, errors) = parse_module(
        "fn main(source: (bool, bool)) () { let (@ (bad, value), retained) = source; later(); } fn later() () {}",
    );
    assert!(
        !errors.is_empty(),
        "invalid first pattern must be diagnosed"
    );
    let ItemKind::Function(main) = &module.items[0].kind else {
        panic!("expected main function");
    };
    let body = main.body.as_ref().expect("main body");
    assert_eq!(body.stmts.len(), 2, "{errors:?}");
    let StmtKind::Binding(binding) = &body.stmts[0].kind else {
        panic!("expected binding statement");
    };
    let PatternKind::Tuple(fields) = &binding.pattern.kind else {
        panic!("expected tuple binding pattern");
    };
    assert_eq!(fields.len(), 1, "{errors:?}");
    assert!(matches!(fields[0].kind, PatternKind::Bind { .. }));
}

#[test]
fn type_argument_delimiter_recovery_keeps_the_function() {
    let (module, errors) =
        parse_module("fn retained(value: Wrapper[i32 bool]) () {}\nfn later() () {}");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error
                .message
                .contains("expected `,` or `]` after type argument"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(retained) = &module.items[0].kind else {
        panic!("expected retained function");
    };
    let Some(TypeRef {
        kind: TypeKind::Path { segments },
        ..
    }) = retained.params[0].ty.as_ref()
    else {
        panic!("expected path parameter type");
    };
    assert_eq!(segments[0].args.len(), 2);
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn type_argument_recovery_keeps_later_arguments_after_nested_tokens() {
    let (module, errors) = parse_module(
        "struct Wrapper[T, U] {}\nfn retained(value: Wrapper[@ (bad, value), bool]) () {}\nfn later() () {}",
    );
    assert!(!errors.is_empty(), "{errors:?}");
    let ItemKind::Function(retained) = &module.items[1].kind else {
        panic!("expected retained function");
    };
    let Some(TypeRef {
        kind: TypeKind::Path { segments },
        ..
    }) = retained.params[0].ty.as_ref()
    else {
        panic!("expected path parameter type");
    };
    assert_eq!(segments[0].args.len(), 1, "{errors:?}");
    let ItemKind::Function(later) = &module.items[2].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn function_parameter_delimiter_recovery_keeps_the_function() {
    let (module, errors) =
        parse_module("fn retained(first: i32 second: bool) () {}\nfn later() () {}");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error
                .message
                .contains("expected `,` or `)` after parameter"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(retained) = &module.items[0].kind else {
        panic!("expected retained function");
    };
    assert_eq!(retained.params.len(), 2);
    assert_eq!(retained.params[1].name, Some(sym("second")));
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn trait_member_parameter_recovery_keeps_later_members_and_items() {
    let (module, errors) = parse_module(
        "trait Retained { fn bad(first: i32 second: bool) (); fn good(value: i32) (); }\nfn later() () {}",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error
                .message
                .contains("expected `,` or `)` after parameter"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Trait(retained) = &module.items[0].kind else {
        panic!("expected retained trait");
    };
    assert_eq!(retained.methods.len(), 2);
    assert_eq!(retained.methods[0].function.params.len(), 2);
    assert_eq!(
        retained.methods[0].function.params[1].name,
        Some(sym("second"))
    );
    assert_eq!(retained.methods[1].function.name, sym("good"));
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn extend_member_parameter_recovery_keeps_later_members_and_items() {
    let (module, errors) = parse_module(
        "struct Value {}\nextend Value { fn bad(first: i32 second: bool) Value { Value {} } fn good(value: i32) Value { Value {} } }\nfn later() () {}",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error
                .message
                .contains("expected `,` or `)` after parameter"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Extend(retained) = &module.items[1].kind else {
        panic!("expected retained extension");
    };
    assert_eq!(retained.methods.len(), 2);
    assert_eq!(retained.methods[0].function.params.len(), 2);
    assert_eq!(
        retained.methods[0].function.params[1].name,
        Some(sym("second"))
    );
    assert_eq!(retained.methods[1].function.name, sym("good"));
    let ItemKind::Function(later) = &module.items[2].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn closure_parameter_delimiter_recovery_keeps_the_function() {
    let (module, errors) = parse_module(
        "fn retained() () { let value = \\first: i32 second: bool -> second; }\nfn later() () {}",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error
                .message
                .contains("expected `,` or `->` after closure parameter"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Function(retained) = &module.items[0].kind else {
        panic!("expected retained function");
    };
    let body = retained.body.as_ref().expect("expected retained body");
    let StmtKind::Binding(binding) = &body.stmts[0].kind else {
        panic!("expected closure binding");
    };
    let ExprKind::Closure { params, .. } = &binding.value.as_ref().expect("closure").kind else {
        panic!("expected closure expression");
    };
    assert_eq!(params.len(), 2);
    assert_eq!(params[1].name, Some(sym("second")));
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn closure_parameter_name_recovery_keeps_later_parameters() {
    let (module, errors) = parse_module(
        r#"fn retained() () { let value = \, second: bool -> second; }
fn later() () {}"#,
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0]
            .message
            .contains("expected closure parameter name"),
        "{errors:?}"
    );

    let ItemKind::Function(retained) = &module.items[0].kind else {
        panic!("expected retained function");
    };
    let body = retained.body.as_ref().expect("expected retained body");
    let StmtKind::Binding(binding) = &body.stmts[0].kind else {
        panic!("expected closure binding");
    };
    let ExprKind::Closure { params, .. } = &binding.value.as_ref().expect("closure").kind else {
        panic!("expected closure expression");
    };
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, Some(sym("second")));

    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn type_parameter_delimiter_recovery_keeps_the_function() {
    let (module, errors) = parse_module(
        "fn retained(pointer: &fn(i32 bool), callable: Fn(i32 bool), tuple: (i32 bool)) () {}\nfn later() () {}",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error
                .message
                .contains("expected `,` or `)` after type parameter"))
            .count(),
        3,
        "{errors:?}"
    );
    let ItemKind::Function(retained) = &module.items[0].kind else {
        panic!("expected retained function");
    };
    assert_eq!(retained.params.len(), 3);
    let ItemKind::Function(later) = &module.items[1].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn supertrait_delimiter_recovery_keeps_later_traits() {
    let (module, errors) = parse_module(
        "trait Base {}
         trait Other {}
         trait Derived: Base Other {}
         fn later() () {}",
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("expected `+` after supertrait"))
            .count(),
        1,
        "{errors:?}"
    );
    let ItemKind::Trait(derived) = &module.items[2].kind else {
        panic!("expected derived trait");
    };
    assert_eq!(derived.supertraits.len(), 2);
    let ItemKind::Trait(other) = &module.items[1].kind else {
        panic!("expected other trait");
    };
    assert_eq!(other.name, sym("Other"));
    let ItemKind::Function(later) = &module.items[3].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn item_recovery_ignores_nested_statement_delimiters() {
    let (module, errors) = parse_module("extern @ (bad; value);\nfn later() () {}");
    assert_eq!(errors.len(), 1, "{errors:?}");
    let ItemKind::Function(later) = &module.items[0].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}

#[test]
fn classifies_parse_errors_by_grammar_rule_not_message_text() {
    let cases = [
        (
            "struct { x: i32 }",
            "expected struct name",
            ParseErrorKind::ExpectedName,
        ),
        (
            "type = i32;",
            "expected type alias name",
            ParseErrorKind::ExpectedName,
        ),
        (
            "fn f(x: ) {}",
            "expected type",
            ParseErrorKind::ExpectedType,
        ),
        (
            "fn f() { let = 1; }",
            "expected binding pattern",
            ParseErrorKind::ExpectedBindingPattern,
        ),
        (
            "fn f() i32 { 1 + }",
            "expected expression",
            ParseErrorKind::ExpectedExpression,
        ),
        (
            "fn f() { g(1; }",
            "expected `)`",
            ParseErrorKind::MissingClosingDelimiter,
        ),
        ("fn f() { 1 $ }", "lexical error", ParseErrorKind::Lexical),
    ];
    for (source, message, kind) in cases {
        let (_module, errors) = parse_module(source);
        let error = errors
            .iter()
            .find(|error| error.message.contains(message))
            .unwrap_or_else(|| panic!("missing `{message}` for {source:?}: {errors:?}"));
        assert_eq!(error.kind, kind, "{source:?}: {error:?}");
        let help = error.to_diagnostic().help.first().cloned();
        assert_eq!(help.as_deref(), kind.help(), "{source:?}");
    }
}

#[test]
fn grammar_errors_without_a_rule_hint_carry_no_help() {
    let (_module, errors) = parse_module("const mut X: i32 = 1;");
    let error = errors
        .iter()
        .find(|error| error.message == "const bindings cannot be mutable")
        .expect("mutable const diagnostic");
    assert_eq!(error.kind, ParseErrorKind::Grammar);
    assert!(error.to_diagnostic().help.is_empty(), "{error:?}");
}

#[test]
fn rejects_prefix_deref_syntax() {
    let (_module, errors) = parse_module(
        r#"
fn main(ptr: &i32) i32 {
    *ptr
}
"#,
    );
    assert!(!errors.is_empty(), "{errors:?}");
}

#[test]
fn rejects_removed_multi_arm_if_pattern_syntax() {
    let (_module, errors) = parse_module(
        r#"
fn main(value: ?i32) () {
    if ?item = value {
        _ = item;
    } or null {}
}
"#,
    );
    assert!(!errors.is_empty(), "removed syntax parsed successfully");
}

#[test]
fn parser_makes_progress_on_generated_invalid_inputs() {
    const TOKENS: &[&str] = &[
        "let",
        "static",
        "mut",
        "const",
        "extern",
        "pub",
        "fn",
        "struct",
        "union",
        "trait",
        "extend",
        "enum",
        "type",
        "using",
        "if",
        "is",
        "or",
        "else",
        "for",
        "while",
        "loop",
        "return",
        "break",
        "continue",
        "defer",
        "module",
        "pkg",
        "self",
        "Self",
        "=",
        ":",
        ";",
        ",",
        ".",
        "::",
        "(",
        ")",
        "{",
        "}",
        "[",
        "]",
        "&",
        "*",
        "?",
        "!",
        "+",
        "-",
        "/",
        "..",
        "...",
        "\"unterminated",
        "\"bad\\q\"",
        "123",
        "name",
    ];
    const PREFIXES: &[&str] = &[
        "",
        "fn anchor() i32 { ",
        "struct S { ",
        "extend S { ",
        "trait T { ",
        "enum E { ",
    ];
    const SUFFIXES: &[&str] = &[
        "",
        " fn main() i32 { 0 }",
        " } fn main() i32 { 0 }",
        "; fn main() i32 { 0 }",
    ];

    let mut seed = 0x9E37_79B9u32;
    let mut cases = Vec::new();
    for case_index in 0..256 {
        let mut source = String::new();
        source.push_str(PREFIXES[case_index % PREFIXES.len()]);
        let token_count = 4 + (case_index % 18);
        for _ in 0..token_count {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let token = TOKENS[(seed as usize) % TOKENS.len()];
            source.push_str(token);
            source.push(' ');
        }
        source.push_str(SUFFIXES[(case_index / PREFIXES.len()) % SUFFIXES.len()]);
        cases.push(source);
    }

    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for source in cases {
            let _ = parse_module(&source);
        }
        sender.send(()).expect("send parser fuzz completion");
    });

    receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("parser did not make progress on generated invalid inputs");
}

#[test]
fn associated_member_recovery_keeps_later_members_and_items() {
    let (module, errors) = parse_module(
        "trait Retained { type Missing fn good(value: i32) (); const ALSO: bool; }\n\
         extend Retained { type Missing fn good(value: i32) Retained { Retained {} } const ALSO: bool; }\n\
         fn later() () {}",
    );
    assert!(
        !errors.is_empty(),
        "malformed associated members must report errors"
    );

    let ItemKind::Trait(trait_item) = &module.items[0].kind else {
        panic!("expected retained trait");
    };
    assert_eq!(trait_item.methods.len(), 1);
    assert_eq!(trait_item.methods[0].function.name, sym("good"));
    assert_eq!(trait_item.associated_values.len(), 1);
    assert_eq!(trait_item.associated_values[0].name, sym("ALSO"));

    let ItemKind::Extend(extend_item) = &module.items[1].kind else {
        panic!("expected retained extension");
    };
    assert_eq!(extend_item.methods.len(), 1);
    assert_eq!(extend_item.methods[0].function.name, sym("good"));

    let ItemKind::Function(later) = &module.items[2].kind else {
        panic!("expected later function");
    };
    assert_eq!(later.name, sym("later"));
}
