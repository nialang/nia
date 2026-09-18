use crate::parse_module;

#[test]
fn parses_omitted_enum_variant_before_if_pattern_body() {
    let (_module, errors) = parse_module(
        r#"
enum Kind {
    Directory,
    File,
}

fn main(kind: Kind) i32 {
    if kind is .Directory {
        1
    } else {
        0
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn parses_omitted_enum_variant_before_block_in_equality() {
    let (_module, errors) = parse_module(
        r#"
enum Kind {
    Directory,
    File,
}

fn main(kind: Kind) i32 {
    if kind == .Directory {
        1
    } else {
        0
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
}
