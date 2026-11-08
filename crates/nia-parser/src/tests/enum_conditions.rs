use crate::parse_module;

#[test]
fn parses_omitted_enum_variant_before_if_pattern_body() {
    let (_module, errors) = parse_module(
        r#"
enum Kind {
    Directory,
    File,
}

fun main(kind: Kind) i32 {
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

fun main(kind: Kind) i32 {
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

#[test]
fn qualified_generic_constructor_paths_remain_structural_patterns() {
    use nia_ast::{ExprKind, ItemKind, NominalPatternFields, PatternKind};
    for path in [
        "Packet[u8]::Read",
        "model::Packet[u8]::Read",
        "root::model::Packet[u8]::Read",
    ] {
        let source = format!(
            "fun inspect(value: u8) i32 {{ match value {{ {path}(Inner::Source(123))! => 1, _ => 0, }} }}"
        );
        let (module, errors) = parse_module(&source);
        assert!(errors.is_empty(), "{path}: {errors:?}");
        let ItemKind::Function(function) = &module.items[0].kind else {
            panic!("function");
        };
        let ExprKind::Match(matched) = &function.body.as_ref().unwrap().tail.as_ref().unwrap().kind
        else {
            panic!("match");
        };
        let PatternKind::ErrorErr(inner) = &matched.arms[0].patterns[0].kind else {
            panic!("error pattern");
        };
        let PatternKind::Nominal {
            fields: NominalPatternFields::Tuple(fields),
            ..
        } = &inner.kind
        else {
            panic!("{path} must be a constructor pattern: {inner:?}");
        };
        assert!(
            matches!(fields[0].kind, PatternKind::Nominal { .. }),
            "{fields:?}"
        );
    }
}
