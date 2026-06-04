// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

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
