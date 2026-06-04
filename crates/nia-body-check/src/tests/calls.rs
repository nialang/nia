// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn checks_simple_calls_to_module_functions() {
    let checked = pipeline(
        r#"
fn id(x: i32) i32 { x }
fn main() i32 {
    id(1)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_direct_call_argument_count_and_types() {
    let checked = pipeline(
        r#"
fn add(a: i32, b: i32) i32 { a + b }

fn main(flag: bool) i32 {
    _ = add(flag, 1);
    _ = add(1);
    0
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("argument count mismatch"))
    );
}

#[test]
fn checks_aggregate_literals_from_call_argument_context() {
    let checked = pipeline(
        r#"
struct Item {
    value: i32,
}

fn take(items: & [Item]) i32 {
    items.len() as i32
}

fn main() i32 {
    take([
        { value: 1 },
        { value: 2 },
    ])
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_explicit_generic_function_calls() {
    let checked = pipeline(
        r#"
fn id[T](value: T) T { value }
fn pair[T](left: T, right: T) T { left }

fn main(flag: bool) i32 {
    var x: i32 = id[i32](1);
    _ = id[i32](flag);
    _ = id[i32, bool](1);
    _ = pair[bool](true, false);
    x
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument"))
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("generic argument count mismatch")
    }));
}

#[test]
fn disambiguates_bracket_suffix_between_index_and_generic_instantiation() {
    let checked = pipeline(
        r#"
fn id[T](value: T) T { value }

fn main() i32 {
    var xs: [3]i32 = [10, 20, 30];
    var i32: usize = 1;
    var indexed = xs[i32];
    var called: i32 = id[i32](indexed);
    var ptr = & id[i32];
    var bad_value = id[i32];
    indexed + ptr(1) + called
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("index")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("function values are not supported")
        }),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn records_bracket_suffix_resolution_kinds() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn make(value: T) Box[T] {
        { value: value }
    }
}

fn id[T](value: T) T { value }

fn main() i32 {
    var xs: [3]i32 = [10, 20, 30];
    var i32: usize = 1;
    var indexed = xs[i32];
    var called: i32 = id[i32](indexed);
    var boxed = Box[i32]::make(called);
    indexed + boxed.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let counts = checked.facts.bracket_suffix_resolutions.values().fold(
        (0usize, 0usize, 0usize),
        |(indexes, generic_calls, type_prefixes), resolution| match resolution {
            BracketSuffixResolution::Index => (indexes + 1, generic_calls, type_prefixes),
            BracketSuffixResolution::GenericCall => (indexes, generic_calls + 1, type_prefixes),
            BracketSuffixResolution::TypePrefixInstantiation => {
                (indexes, generic_calls, type_prefixes + 1)
            }
        },
    );
    assert_eq!(counts, (1, 1, 1));
}
