// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

const FORMAL_MATRIX: &str = include_str!("../../../../docs/formal/value_inference_matrix.json");

#[test]
fn value_construction_compositions_check_in_runtime_and_const_contexts() {
    let mut expressions = [
        ("integer_default", "i32", "1"),
        ("integer_contextual_widening", "i64", "1"),
        ("integer_suffix", "u8", "2u8"),
        ("integer_negative", "i32", "-1"),
        ("integer_radix", "i32", "0b1010"),
        ("integer_hexadecimal", "u8", "0xffu8"),
        ("integer_octal", "usize", "0o755usize"),
        ("integer_separators", "u32", "1_000u32"),
        ("integer_min_i32", "i32", "-2147483648"),
        ("float_default", "f64", "1.5"),
        ("float_suffix", "f32", "1.5f32"),
        ("float_exponent", "f64", "1.25e2"),
        ("character", "char", "'a'"),
        ("escaped_character", "char", "'\\n'"),
        ("unicode_character", "char", "'中'"),
        ("byte_character", "u8", "b'a'"),
        ("nul_byte_character", "u8", "b'\\0'"),
        ("string", "[char; 2]", "\"ab\""),
        ("adjacent_string", "[char; 2]", "\"a\" \"b\""),
        ("byte_string", "[u8; 2]", "b\"ab\""),
        ("utf8_byte_string", "[u8; 3]", "b\"中\""),
        ("string_auto_address_slice", "&[char]", "\"ab\""),
        ("byte_string_auto_address_slice", "&[u8]", "b\"ab\""),
        ("boolean", "bool", "true"),
        ("global_address", "&i32", "&api::ONE"),
        ("unit_tuple", "()", "()"),
        ("tuple_first", "(i32, bool)", "(1, false)"),
        ("tuple_second", "(bool, i32)", "(false, 1)"),
        ("tuple_third", "(u8, bool, i32)", "(1u8, false, 2)"),
        (
            "tuple_of_array_and_optional",
            "([i32; 2], ?i32)",
            "([1, 2], ?3)",
        ),
        ("array_list", "[i32; 2]", "[1, 2]"),
        ("array_repeat", "[i32; 2]", "[1; 2]"),
        ("array_empty", "[i32; 0]", "[]"),
        ("array_empty_repeat", "[i32; 0]", "[1; 0]"),
        (
            "array_of_tuples",
            "[(i32, bool); 2]",
            "[(1, false), (2, true)]",
        ),
        ("nested_empty_array", "[?i32; 2]", "[null, null]"),
        ("optional_some", "?i32", "?1"),
        ("optional_null", "?i32", "null"),
        ("error_ok", "u8!i32", "!1"),
        ("error_error", "u8!i32", "2u8!"),
        ("block_tail", "i32", "{ let value = 1; value }"),
        ("if_then_and_else", "i32", "if true { 1 } else { 2 }"),
        ("match_arms", "i32", "match true { true => 1, false => 2 }"),
        (
            "match_block_arms",
            "i32",
            "match true { true => { 1 }, false => { 2 } }",
        ),
        (
            "if_pattern_value",
            "i32",
            "if ?1 is ?value { value } else { 0 }",
        ),
        (
            "if_pattern_chain_value",
            "i32",
            "if true and ?1 is ?value and value > 0 { value } else { 0 }",
        ),
        ("operator_context", "i64", "1 + 2i64"),
        ("comparison_result", "bool", "1 < 2i64"),
        ("cast_result", "i64", "1 as i64"),
        (
            "typed_struct",
            "api::Cell[i32]",
            "api::Cell[i32] { value: 1 }",
        ),
        ("empty_struct", "api::Empty", "api::Empty {}"),
        ("qualified_struct", "api::Plain", "api::Plain { value: 1 }"),
        ("omitted_struct", "api::Cell[i32]", ".{ value: 1 }"),
        (
            "array_of_generic_aggregates",
            "[api::Cell[i32]; 2]",
            "[api::Cell[i32] { value: 1 }, .{ value: 2 }]",
        ),
        ("aliased_omitted_struct", "LocalCell[i32]", ".{ value: 1 }"),
        (
            "typed_union",
            "api::Slot[i32]",
            "api::Slot[i32] { value: 1 }",
        ),
        (
            "qualified_union",
            "api::PlainSlot",
            "api::PlainSlot { value: 1 }",
        ),
        ("omitted_union", "api::Slot[i32]", ".{ value: 1 }"),
        (
            "tuple_struct",
            "api::TupleCell[i32]",
            "api::TupleCell[i32](1)",
        ),
        ("enum_unit_qualified", "api::Event", "api::Event::Empty"),
        ("enum_unit_omitted", "api::Event", ".Empty"),
        ("enum_tuple_qualified", "api::Event", "api::Event::Data(1)"),
        ("enum_tuple_omitted", "api::Event", ".Data(1)"),
        (
            "enum_named_qualified",
            "api::Event",
            "api::Event::Record { value: 1 }",
        ),
        ("enum_named_omitted", "api::Event", ".Record { value: 1 }"),
        ("generic_identity", "i32", "api::identity(1)"),
        (
            "generic_identity_of_omitted_struct",
            "api::Cell[i32]",
            "api::identity[api::Cell[i32]](.{ value: 1 })",
        ),
        (
            "cross_module_nested_omitted_generic",
            "api::Deep[i32]",
            "api::identity[api::Deep[i32]](.{ value: .{ value: api::identity(1) } })",
        ),
        (
            "generic_identity_of_omitted_union",
            "api::Slot[i32]",
            "api::identity[api::Slot[i32]](.{ value: api::identity(1) })",
        ),
        (
            "generic_identity_of_omitted_enum",
            "api::Event",
            "api::identity[api::Event](.Data(api::identity(1)))",
        ),
        (
            "external_omitted_struct",
            "api::Plain",
            ".{ value: api::identity(1) }",
        ),
        ("nested_optional_generic", "?i32", "?api::identity(1)"),
        ("nested_array_generic", "[i32; 2]", "[api::identity(1), 2]"),
        (
            "nested_nominal_generic",
            "api::Box[i32]",
            "api::Box[i32] { value: api::identity(1) }",
        ),
        (
            "nested_nominal_omission",
            "api::Deep[i32]",
            "api::Deep[i32] { value: .{ value: api::identity(1) } }",
        ),
        (
            "tuple_of_omitted_aggregate_and_array",
            "(api::Plain, [i32; 2])",
            "(.{ value: 3 }, [4, 5])",
        ),
        ("nested_error_generic", "u8!i32", "!api::identity(1)"),
        (
            "nested_optional_error_union",
            "?(u8!i32)",
            "?(!api::identity(1))",
        ),
        (
            "branch_generic_results",
            "i32",
            "if true { api::identity(1) } else { api::identity(2) }",
        ),
        (
            "deep_cross_constructor_nesting",
            "api::Cell[(api::Plain, [i32!i32; 2])]",
            "api::Cell[(api::Plain, [i32!i32; 2])] { value: (.{ value: 1 }, [!2, !3]) }",
        ),
        (
            "deep_cross_module_omission",
            "api::Deep[?i32]",
            "api::Deep[?i32] { value: .{ value: ?api::identity(1) } }",
        ),
    ]
    .into_iter()
    .map(|(name, ty, expression)| (name.to_string(), ty.to_string(), expression.to_string()))
    .collect::<Vec<_>>();
    for ty in [
        "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize",
    ] {
        expressions.push((
            format!("integer_context_{ty}"),
            ty.to_string(),
            "1".to_string(),
        ));
        expressions.push((
            format!("integer_suffix_{ty}"),
            ty.to_string(),
            format!("1{ty}"),
        ));
        if ty.starts_with('i') {
            expressions.push((
                format!("negative_context_{ty}"),
                ty.to_string(),
                "-1".to_string(),
            ));
        }
    }
    for ty in ["f32", "f64"] {
        expressions.push((
            format!("float_context_{ty}"),
            ty.to_string(),
            "1.5".to_string(),
        ));
        expressions.push((
            format!("float_suffix_{ty}"),
            ty.to_string(),
            format!("1.5{ty}"),
        ));
    }
    let matrix: serde_json::Value =
        serde_json::from_str(FORMAL_MATRIX).expect("formal inference matrix is valid JSON");
    let manifest_cases = matrix["compiler_expression_cases"]
        .as_array()
        .expect("formal matrix lists compiler expression cases")
        .iter()
        .map(|case| case.as_str().expect("case names are strings"))
        .collect::<Vec<_>>();
    let test_cases = expressions
        .iter()
        .map(|(name, _, _)| name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        test_cases, manifest_cases,
        "compiler cases and the formal manifest must cover exactly the same matrix"
    );
    let manifest_constructors = matrix["constructor_forms"]
        .as_array()
        .expect("formal matrix lists constructor forms")
        .iter()
        .map(|constructor| constructor.as_str().expect("constructor names are strings"))
        .collect::<Vec<_>>();
    let coverage = matrix["constructor_coverage"]
        .as_array()
        .expect("formal matrix maps constructors to compiler cases")
        .iter()
        .map(|entry| entry.as_str().expect("coverage entries are strings"))
        .collect::<Vec<_>>();
    let coverage_constructors = coverage
        .iter()
        .map(|entry| {
            let (constructor, case) = entry
                .split_once('=')
                .expect("coverage entries use constructor=compiler_case syntax");
            assert!(
                test_cases.contains(&case),
                "constructor {constructor:?} points to missing compiler case {case:?}"
            );
            constructor
        })
        .collect::<Vec<_>>();
    assert_eq!(
        coverage_constructors, manifest_constructors,
        "every formal constructor form must map to a compiler case in manifest order"
    );

    let bindings = expressions
        .iter()
        .enumerate()
        .map(|(index, (_, ty, expression))| {
            format!("    let value_{index}: {ty} = {expression};\n")
        })
        .collect::<String>();
    let root = temp_dir("value_construction_compositions");
    let main_source = format!(
        r#"
module api;
using root::api;

type LocalCell[T] = api::Cell[T];

const fun verify_const() i32 {{
{bindings}    0
}}

fun verify_runtime() i32 {{
{bindings}    0
}}

const checked: i32 = verify_const();

fun main() i32 {{
    checked + verify_runtime()
}}
"#
    );
    write(&root.join("main.nia"), &main_source);
    write(
        &root.join("api.nia"),
        r#"
pub struct Cell[T] { value: T }
pub struct Box[T] { value: T }
pub struct Deep[T] { value: Box[T] }
pub union Slot[T] { value: T, bytes: [u8; 4] }
pub struct Plain { value: i32 }
pub struct Empty {}
pub union PlainSlot { value: i32, bytes: [u8; 4] }
pub struct TupleCell[T](T)
pub enum Event { Empty, Data(i32), Record { value: i32 } }

pub const fun identity[T](value: T) T { value }
pub const ONE: i32 = 1;
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    let errors = program
        .diagnostics
        .iter()
        .map(|diagnostic| {
            let span = diagnostic
                .diagnostic
                .labels
                .first()
                .map_or(0, |label| label.span.start);
            let expression = main_source[..span.min(main_source.len())]
                .rfind("let value_")
                .and_then(|start| {
                    let digits = main_source[start + "let value_".len()..]
                        .chars()
                        .take_while(char::is_ascii_digit)
                        .collect::<String>();
                    digits.parse::<usize>().ok()
                })
                .and_then(|index| expressions.get(index))
                .map(|(name, _, expression)| format!("{name}: {expression}"))
                .unwrap_or_else(|| format!("source offset {span}"));
            format!("{expression}: {}", diagnostic.diagnostic.summary)
        })
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "value-construction diagnostics: {errors:#?}"
    );

    let emitted = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    let emit_errors = emitted
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert!(
        emit_errors.is_empty(),
        "value-construction emit diagnostics: {emit_errors:#?}"
    );
}

#[test]
fn incompatible_value_construction_contexts_are_rejected() {
    let root = temp_dir("incompatible_value_construction_contexts");
    write(
        &root.join("main.nia"),
        r#"
struct Point { value: i32 }

fun invalid() i32 {
    let wrong_scalar: bool = 1;
    let wrong_suffix: u16 = 1u8;
    let wrong_length: [i32; 3] = [1, 2];
    let wrong_optional: ?bool = ?1;
    let null_without_context = null;
    let omitted_without_context = .{ value: 1 };
    let wrong_branch: i32 = if true { 1 } else { false };
    0
}

fun main() i32 { invalid() }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    let summaries = program
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    for expected in [
        "type mismatch in binding initializer",
        "type mismatch in optional value",
        "omitted constructor requires an expected nominal type",
        "requires an expected optional type",
    ] {
        assert!(
            summaries.iter().any(|summary| summary.contains(expected)),
            "missing rejection {expected:?}; got {summaries:#?}"
        );
    }
    assert!(
        summaries.len() >= 7,
        "expected every invalid constructor to be rejected: {summaries:#?}"
    );
}
