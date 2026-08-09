// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn const_declaration_filter_checks_only_const_roots_without_bodies() {
    let checked = pipeline_const_declarations(
        r#"
fn invalidRuntimeCallee() usize {
    false
}

fn invalidOrdinary() usize {
    false
}

const fn invalidConst() usize {
    false
}

const fn callsRuntime() usize {
    invalidRuntimeCallee()
}
"#,
    );

    assert_eq!(checked.checked_functions.len(), 2);
    assert!(checked.ir.function_bodies.is_empty());
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .summary
                .contains("type mismatch in function body"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .summary
                .contains("const expression can only call `const fn`"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn const_declaration_filter_accepts_slice_pointer_methods() {
    let checked = pipeline_const_declarations(
        r#"
const fn pointer(values: [2]usize) usize {
    let mut slice = &values[..];
    let ptr = slice.ptr();
    0
}

"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn const_declaration_filter_rejects_runtime_only_iteration_witnesses() {
    let checked = pipeline_const_declarations(
        r#"
struct Counter {
    current: usize,
    end: usize,
}

extend Counter : Iterator {
    type Item = usize;

    fn next(&mut self) ?usize {
        null
    }
}

const fn total(iter: Counter) usize {
    let mut result: usize = 0;
    for value in iter {
        result += value;
    }
    result
}

const fn first(iter: Counter) usize {
    let mut values = iter;
    switch values.next() {
        ?value => value,
        null => 0,
    }
}
"#,
    );

    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains(
                "`Iterator::next` trait witness used by const for-in must be declared `const fn`"
            )),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("const expression can only call `const fn`")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn const_declaration_filter_accepts_scalar_union_value_operations() {
    let checked = pipeline_const_declarations(
        r#"
union Bits {
    integer: usize,
    flag: bool,
}

const fn inspect() usize {
    let bits: Bits = { integer: 1 };
    bits.integer
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn const_declaration_filter_accepts_scalar_array_and_struct_union_fields() {
    let checked = pipeline_const_declarations(
        r#"
struct Header {
    marker: u8,
    value: u16,
}

union Payload {
    bytes: [2]u8,
    integer: u16,
    header: Header,
    nested: Nested,
}

union Nested {
    bytes: [2]u8,
    integer: u16,
}

const fn inspect() u16 {
    let payload: Payload = { integer: 1 };
    payload.integer
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn const_declaration_filter_accepts_nested_pointer_fields_with_relocations() {
    let checked = pipeline_const_declarations(
        r#"
struct Header {
    value: &u16,
}

union Payload {
    header: Header,
    integer: u16,
}

const fn inspect() u16 {
    let payload: Payload = { integer: 1 };
    payload.integer
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn const_declaration_filter_rejects_function_pointer_values_and_calls() {
    let checked = pipeline_const_declarations(
        r#"
const fn increment(value: usize) usize {
    value + 1
}

const fn storesFunctionPointer() usize {
    let callback = & increment;
    1
}

const fn callsFunctionPointer() usize {
    let callback = & increment;
    callback(1)
}
"#,
    );

    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .summary
                .contains("function pointer values are not available during const evaluation"))
            .count(),
        2,
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("indirect function calls are not available during const evaluation")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_return_tail_and_local_binding_types() {
    let checked = pipeline(
        r#"
fn add(a: i32, b: i32) i32 {
    let mut sum: i32 = a + b;
    sum
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_union_literals_and_field_access() {
    let checked = pipeline(
        r#"
union Bits[T] {
    i: i64,
    value: T,
}

fn main() i32 {
    let mut bits: Bits[i32] = { value: 10 };
    bits.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let bad_empty = pipeline(
        r#"
union Bits {
    i: i32,
}

fn main() i32 {
    let mut bits: Bits = {};
    0
}
"#,
    );
    assert!(
        bad_empty
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("exactly one field")),
        "{:?}",
        bad_empty.diagnostics
    );

    let bad_multi = pipeline(
        r#"
union Bits {
    i: i32,
    f: f32,
}

fn main() i32 {
    let mut bits: Bits = { i: 1, f: 2.0 };
    0
}
"#,
    );
    assert!(
        bad_multi
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("exactly one field")),
        "{:?}",
        bad_multi.diagnostics
    );
}

#[test]
fn checks_unit_values_empty_structs_and_opaque_pointers() {
    let checked = pipeline(
        r#"
struct Empty {}

fn take_opaque(p: &opaque) {}
fn take_read_opaque(p: &opaque) {}

fn main() {
    let mut unit: () = ();
    let mut empty: Empty = {};
    let mut value: i32 = 1;
    take_opaque(&value as &opaque);
    take_read_opaque(&value as &opaque);
    unit
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let bad_implicit = pipeline(
        r#"
fn main() {
    let mut value: i32 = 1;
    let mut ptr: &opaque = &value;
}
"#,
    );
    assert!(
        bad_implicit
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("binding initializer")),
        "{:?}",
        bad_implicit.diagnostics
    );

    let bad_deref = pipeline(
        r#"
fn main() i32 {
    let mut value: i32 = 1;
    let mut ptr: &opaque = &value as &opaque;
    ptr.*
}
"#,
    );
    assert!(
        bad_deref
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("cannot dereference `&opaque`")),
        "{:?}",
        bad_deref.diagnostics
    );
}

#[test]
fn checks_volatile_pointer_deref_and_readonly_writes() {
    let checked = pipeline(
        r#"
fn read_reg(reg: ^u32) u32 {
    reg.*
}

fn write_reg(reg: ^mut u32, value: u32) () {
    reg.* = value;
}

fn cast_reg(addr: usize) ^mut u32 {
    addr as ^mut u32
}

fn readonly_from_mut(reg: ^mut u32) ^u32 {
    reg
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let bad = pipeline(
        r#"
fn write_readonly(reg: ^u32, value: u32) () {
    reg.* = value;
}
"#,
    );
    assert!(
        bad.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("pointer is read-only")),
        "{:?}",
        bad.diagnostics
    );
}

#[test]
fn accepts_explicit_return_without_tail_expression() {
    let checked = pipeline(
        r#"
extern fn printf(fmt: &u8, ...);

fn main() i32 {
    let mut hello = b"hello, world!\n\0";
    printf(&hello[0]);
    return 0;
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn reports_mismatched_return_and_binding_types() {
    let checked = pipeline(
        r#"
fn bad(flag: bool) i32 {
    let mut x: bool = 1;
    flag
}
"#,
    );
    assert!(checked.diagnostics.len() >= 2, "{:?}", checked.diagnostics);
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("binding initializer"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("function body"))
    );
}

#[test]
fn checks_optional_and_error_union_construction_and_propagation() {
    let checked = pipeline(
        r#"
fn maybe(flag: bool) ?i32 {
    if flag {
        return ?1;
    }
    null
}

fn ok(value: i32) i32!i32 {
    !value
}

fn fail(error: i32) i32!i32 {
    error!
}

fn use_error(value: i32!i32) i32!i32 {
    let mut unwrapped = value.?;
    !unwrapped
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn error_propagation_uses_unique_into_error_conversion() {
    let checked = pipeline(
        r#"
trait IntoError[Target] {
    fn into_error(self) Target;
}

enum SourceError: i32 {
    Failed = 1,
    _,
}

enum TargetError: i32 {
    Converted = 2,
    Unknown = 3,
    _,
}

extend SourceError : IntoError[TargetError] {
    fn into_error(self) TargetError {
        switch self {
            SourceError::Failed => TargetError::Converted,
            _ => TargetError::Unknown,
        }
    }
}

fn propagate(value: SourceError!i32) TargetError!i32 {
    !(value.?)
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(checked.facts.iter_node_resolved_calls().any(|(_, call)| {
        matches!(
            call,
            nia_sema_ir::ResolvedCall::TraitMethod { method_name, .. }
                if *method_name == nia_symbol::known::INTO_ERROR
        )
    }));
}

#[test]
fn error_propagation_rejects_missing_into_error_conversion() {
    let checked = pipeline(
        r#"
trait IntoError[Target] {
    fn into_error(self) Target;
}

enum SourceError: i32 {
    Failed = 1,
    _,
}

enum TargetError: i32 {
    Converted = 2,
    _,
}

fn propagate(value: SourceError!i32) TargetError!i32 {
    !(value.?)
}
"#,
    );

    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("to implement `IntoError[")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn error_propagation_does_not_chain_into_error_conversions() {
    let checked = pipeline(
        r#"
trait IntoError[Target] {
    fn into_error(self) Target;
}

enum SourceError: i32 { Value = 1, _ }
enum MiddleError: i32 { Value = 2, _ }
enum TargetError: i32 { Value = 3, _ }

extend SourceError : IntoError[MiddleError] {
    fn into_error(self) MiddleError { MiddleError::Value }
}

extend MiddleError : IntoError[TargetError] {
    fn into_error(self) TargetError { TargetError::Value }
}

fn propagate(value: SourceError!i32) TargetError!i32 {
    !(value.?)
}
"#,
    );

    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("to implement `IntoError[")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn error_propagation_rejects_ambiguous_into_error_conversion() {
    let checked = pipeline(
        r#"
trait IntoError[Target] {
    fn into_error(self) Target;
}

struct Pair[A, B] {
    first: A,
    second: B,
}

struct TargetError {}

extend[T] Pair[T, i32] : IntoError[TargetError] {
    fn into_error(self) TargetError { {} }
}

extend[U] Pair[i32, U] : IntoError[TargetError] {
    fn into_error(self) TargetError { {} }
}

fn propagate(value: Pair[i32, i32]!i32) TargetError!i32 {
    !(value.?)
}
"#,
    );

    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("ambiguous error propagation conversion")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn error_propagation_ignores_malformed_into_error_protocol() {
    let checked = pipeline(
        r#"
trait IntoError[Target] {
    fn into_error(self, context: i32) Target;
}

struct SourceError {}
struct TargetError {}

extend SourceError : IntoError[TargetError] {
    fn into_error(self, context: i32) TargetError {
        _ = context;
        {}
    }
}

fn propagate(value: SourceError!i32) TargetError!i32 {
    !(value.?)
}
"#,
    );

    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("to implement `IntoError[")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn const_error_propagation_rejects_automatic_into_error_conversion() {
    let checked = pipeline_const_declarations(
        r#"
trait IntoError[Target] {
    const fn into_error(self) Target;
}

enum SourceError: i32 {
    Failed = 1,
    _,
}

enum TargetError: i32 {
    Converted = 2,
    _,
}

extend SourceError : IntoError[TargetError] {
    const fn into_error(self) TargetError {
        TargetError::Converted
    }
}

const fn propagate(value: SourceError!i32) TargetError!i32 {
    !(value.?)
}
"#,
    );

    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("automatic `IntoError` conversion is not available during const evaluation")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn rejects_error_union_construction_without_expected_type() {
    let checked = pipeline(
        r#"
fn bad() i32 {
    let mut a = !10i32;
    let mut b = 20i32!;
    a + b
}
"#,
    );

    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("requires an expected error union type")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_optional_and_error_union_if_patterns() {
    let checked = pipeline(
        r#"
fn unwrap_optional(value: ?i32) i32 {
    switch value {
        ?x => {
            x
        },
        null => {
            0
        },
    }
}

fn unwrap_error(value: i32!i32) i32 {
    switch value {
        !x => {
            x
        },
        e! => {
            e
        },
    }
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let missing = pipeline(
        r#"
fn bad(value: ?i32) i32 {
    let unwrapped: i32 = if value is ?x {
        x
    };
    unwrapped
}
"#,
    );
    assert!(
        missing
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("non-exhaustive if pattern")),
        "{:?}",
        missing.diagnostics
    );

    let wrong_target = pipeline(
        r#"
fn bad(value: i32) i32 {
    switch value {
        ?x => {
            x
        },
        _ => {
            0
        },
    }
}
"#,
    );
    assert!(
        wrong_target
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("requires an optional target")),
        "{:?}",
        wrong_target.diagnostics
    );
}
