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
            .any(|error| error.message.contains("InvalidStringEscape"))
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
fn main(value: ?i32) void {
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
