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
    assert_eq!(retained.generics.len(), 1);
    let ItemKind::Function(later) = &module.items[1].kind else {
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
