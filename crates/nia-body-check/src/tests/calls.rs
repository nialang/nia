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
            .any(|diagnostic| diagnostic.summary.contains("call argument"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("argument count mismatch"))
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
    take(&[
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
    let mut x: i32 = id[i32](1);
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
            .any(|diagnostic| diagnostic.summary.contains("binding initializer"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("call argument"))
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("generic argument count mismatch")
    }));
}

#[test]
fn disambiguates_bracket_suffix_between_index_and_generic_instantiation() {
    let checked = pipeline(
        r#"
fn id[T](value: T) T { value }

fn main() i32 {
    let mut xs: [3]i32 = [10, 20, 30];
    let mut i32: usize = 1;
    let mut indexed = xs[i32];
    let mut called: i32 = id[i32](indexed);
    let mut ptr = & id[i32];
    let mut bad_value = id[i32];
    indexed + ptr(1) + called
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("index")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
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
    let mut xs: [3]i32 = [10, 20, 30];
    let mut i32: usize = 1;
    let mut indexed = xs[i32];
    let mut called: i32 = id[i32](indexed);
    let mut boxed = Box[i32]::make(called);
    indexed + boxed.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let counts = checked.facts.node_bracket_suffix_resolutions.values().fold(
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

#[test]
fn records_generic_calls_inside_defer_blocks() {
    let checked = pipeline(
        r#"
extern fn log(value: i32);

fn id[T](value: T) T {
    value
}

fn main() i32 {
    defer {
        log(id[i32](7))
    };
    0
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(
        checked
            .facts
            .node_bracket_suffix_resolutions
            .values()
            .any(|resolution| matches!(resolution, BracketSuffixResolution::GenericCall)),
        "{:?}",
        checked.facts.node_bracket_suffix_resolutions
    );
    assert!(
        checked
            .facts
            .node_resolved_calls
            .values()
            .any(|call| matches!(call, nia_sema_ir::ResolvedCall::FunctionInstance { .. })),
        "{:?}",
        checked.facts.node_resolved_calls
    );
}

#[test]
fn records_generic_calls_inside_binary_operator_operands() {
    let checked = pipeline(
        r#"
fn id[T](value: T) T {
    value
}

fn main() i32 {
    id[i32](1) + id[i32](2)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert_eq!(
        checked
            .facts
            .node_bracket_suffix_resolutions
            .values()
            .filter(|resolution| matches!(resolution, BracketSuffixResolution::GenericCall))
            .count(),
        2,
        "{:?}",
        checked.facts.node_bracket_suffix_resolutions
    );
    assert_eq!(
        checked
            .facts
            .node_resolved_calls
            .values()
            .filter(|call| matches!(call, nia_sema_ir::ResolvedCall::FunctionInstance { .. }))
            .count(),
        2,
        "{:?}",
        checked.facts.node_resolved_calls
    );
}

#[test]
fn checks_simd_lane_builtins() {
    let checked = pipeline(
        r#"
fn lane(v: u8x16, i: usize) u8 {
    @extract(v, i)
}

fn changed(v: u8x16, i: usize) u8x16 {
    @insert(v, i, 9u8)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn rejects_invalid_simd_lane_builtins() {
    let checked = pipeline(
        r#"
fn invalid(v: u8x16) void {
    _ = @extract[u8](v, 0usize);
    _ = @extract(1u8, 0usize);
    _ = @insert(v, 0usize, true);
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("builtin `@extract` does not take a type argument")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("requires a SIMD vector argument")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("SIMD lane value")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_atomic_builtin_ordering_rules() {
    let checked = pipeline(
        r#"
fn main() void {
    let mut value = 0i32;
    _ = @atomic_load[i32](&value, 3usize);
    @atomic_store[i32](&mut value, 1i32, 2usize);
    _ = @cmpxchg_strong[i32](&mut value, 0i32, 1i32, 1usize, 3usize);
    @fence(1usize);
}
"#,
    );
    let messages = checked
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("atomic ordering `Release` is invalid for atomic load")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        messages.iter().any(
            |message| message.contains("atomic ordering `Acquire` is invalid for atomic store")
        ),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        messages.iter().any(|message| {
            message.contains("atomic ordering `Release` is invalid for cmpxchg failure")
        }),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        messages.iter().any(|message| {
            message.contains("atomic ordering `Monotonic` is invalid for atomic fence")
        }),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn rejects_non_atomic_builtin_value_types() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
}

fn main() void {
    let mut point: Point = { x: 1 };
    _ = @atomic_load[Point](&point, 1usize);
    let mut pair = [1i32, 2i32];
    _ = @atomic_rmw[[2]i32](&mut pair, 1usize, [3i32, 4i32], 1usize);
}
"#,
    );
    let count = checked
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.summary.contains(
                "supports only bool, integer, enum, and pointer types up to the native pointer width",
            )
        })
        .count();
    assert_eq!(count, 2, "{:?}", checked.diagnostics);
}

#[test]
fn checks_splat_builtin() {
    let checked = pipeline(
        r#"
fn make() u8x16 {
    @splat[u8x16](7u8)
}

fn invalid() u8 {
    @splat[u8](1u8)
}
"#,
    );

    assert_eq!(checked.diagnostics.len(), 1, "{:?}", checked.diagnostics);
    assert!(
        checked.diagnostics[0]
            .summary
            .contains("requires a SIMD vector type"),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_simd_bitmask_builtin() {
    let checked = pipeline(
        r#"
fn mask(v: u8x16) usize {
    @bitmask(v == @splat[u8x16](7u8))
}

fn invalid_value(v: u8x16) usize {
    @bitmask(v)
}

fn invalid_type_arg(v: boolx16) usize {
    @bitmask[usize](v)
}
"#,
    );

    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("bool SIMD mask vector")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("builtin `@bitmask` does not take a type argument")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_bit_intrinsic_builtins() {
    let checked = pipeline(
        r#"
fn bits(mask: usize) usize {
    @ctz[usize](mask) + @clz[usize](mask) + @popcount[usize](mask)
}

fn invalid_type(value: bool) usize {
    @ctz[bool](value)
}

fn missing_type(value: usize) usize {
    @popcount(value)
}
"#,
    );

    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("got bool")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("builtin `@popcount` requires an integer type argument")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_load_unaligned_builtin() {
    let checked = pipeline(
        r#"
fn load(ptr: &u8) u8x8 {
    @load_unaligned[u8x8](ptr)
}

fn invalid_ptr(ptr: &u16) u8x8 {
    @load_unaligned[u8x8](ptr)
}
"#,
    );

    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("byte pointer argument")),
        "{:?}",
        checked.diagnostics
    );
}
