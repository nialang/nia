// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

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
