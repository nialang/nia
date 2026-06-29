// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn parses_multiline_string_literal() {
    let (module, errors) = parse_module(
        r#"
static script =
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
static text = "hello" "" ", " "world" "" "!" "\n" "done";
static bytes = b"" b"n" b"" b"i" b"" b"a" b"" b"\0";
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
}

#[test]
fn rejects_adjacent_string_literals_with_different_prefixes() {
    let (_module, errors) = parse_module(
        r#"
static a = "hello" b"world";
"#,
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error
                .message
                .contains("adjacent string literals must use the same literal prefix"))
            .count(),
        1,
        "{errors:?}"
    );
}

#[test]
fn does_not_concatenate_multiline_string_literals() {
    let (_module, errors) = parse_module(
        r#"
static text =
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
