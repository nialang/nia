// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn checks_direct_calls_to_concrete_closure_values() {
    let checked = pipeline(
        r#"
fn main(base: i32) i32 {
    let callback = \[base] value: i32 -> { base + value };
    callback(2)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let body = checked
        .ir
        .function_bodies
        .values()
        .next()
        .expect("main body");
    let tail = body.tail.as_deref().expect("main tail");
    let nia_body_ir::TypedExprKind::Call { callee, args } = &tail.kind else {
        panic!("expected closure call");
    };
    assert!(matches!(callee, nia_body_ir::TypedCallee::Closure(_)));
    assert_eq!(args.len(), 1);
}

#[test]
fn infers_closure_signature_from_callable_context() {
    let checked = pipeline(
        r#"
fn apply(callback: &Fn(i32) i32, value: i32) i32 {
    callback(value)
}

fn main() i32 {
    apply(&\value -> { value + 1 }, 2)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_local_closure_signature_from_a_later_call() {
    let checked = pipeline(
        r#"
fn main() i32 {
    let identity = \value -> { value };
    identity(3)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_nested_closure_signatures_across_later_calls() {
    let checked = pipeline(
        r#"
fn apply(callback: &Fn(i32) i32, value: i32) i32 {
    callback(value)
}

fn main() i32 {
    apply(&\left -> {
        apply(&\right -> { right + 1 }, left)
    }, 2)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn reports_unresolved_closure_parameter_without_constraints() {
    let checked = pipeline(
        r#"
fn main() i32 {
    let identity = \value -> { value };
    0
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("cannot infer closure parameter")
        }),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn rejects_explicit_closure_parameter_that_conflicts_with_callable_context() {
    let checked = pipeline(
        r#"
fn apply(callback: &Fn(i32) i32, value: i32) i32 {
    callback(value)
}

fn main() i32 {
    apply(&\value: i64 -> { value }, 2)
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("type mismatch in closure parameter")
        }),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn keeps_inferred_closures_monotype_when_called_with_conflicting_types() {
    let checked = pipeline(
        r#"
fn main() i32 {
    let identity = \value -> { value };
    _ = identity(1);
    _ = identity(true);
    0
}
"#,
    );
    assert!(
        !checked.diagnostics.is_empty(),
        "expected a monotype conflict"
    );
}

#[test]
fn closure_signature_contributes_to_generic_callable_inference() {
    let checked = pipeline(
        r#"
fn apply[T](callback: &Fn(T) T, value: T) T {
    callback(value)
}

fn main() i32 {
    apply(&\value -> { value }, 1)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn mutable_closure_address_infers_generic_readonly_callable_signature() {
    let checked = pipeline(
        r#"
fn accepts[T](callback: &Fn(T) T) i32 {
    1
}

fn main() i32 {
    let mut callback = \value: i32 -> { value };
    accepts(&mut callback)
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn mismatched_closure_arity_does_not_partially_infer_generics() {
    let checked = pipeline(
        r#"
fn accepts[T](callback: &Fn(T, T) T, value: T) T {
    callback(value, value)
}

fn main() bool {
    accepts(&\value: i32 -> { value }, true)
}
"#,
    );

    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("closure parameter count mismatch")
    }));
    assert!(!checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("conflicting inferred type for generic parameter")
    }));
}

#[test]
fn generic_callable_argument_infers_return_from_direct_closure_pointer() {
    let checked = pipeline(
        r#"
extend[Value, Source, Target] Source!Value {
    fn mapError(self, mapper: &Fn(Source) Target) Target!Value {
        match self {
            !value => !value,
            error! => mapper(error)!,
        }
    }
}

fn source() i32!i32 {
    1!
}

fn main() bool!i32 {
    source().mapError(&\error: i32 -> { error == 1 })
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn generic_callable_argument_infers_nominal_return_without_result_context() {
    let checked = pipeline(
        r#"
enum TargetError: i32 {
    Mapped = 1,
    _,
}

extend[Value, Source, Target] Source!Value {
    fn mapError(self, mapper: &Fn(Source) Target) Target!Value {
        match self {
            !value => !value,
            error! => mapper(error)!,
        }
    }
}

fn source() i32!i32 {
    1!
}

fn main() i32 {
    let mapped = source().mapError(&\error: i32 -> {
        _ = error;
        TargetError::Mapped
    });
    _ = mapped;
    0
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn fallible_callable_argument_infers_nested_error_union_return() {
    let checked = pipeline(
        r#"
extend[Value, Source, Target] Source!Value {
    fn orElse(self, fallback: &Fn(Source) Target!Value) Target!Value {
        match self {
            !value => !value,
            error! => fallback(error),
        }
    }
}

fn source() i32!i32 {
    1!
}

fn main() i32 {
    let recovered = source().orElse(&\error: i32 -> {
        if error == 1 {
            !42
        } else {
            true!
        }
    });
    match recovered {
        !value => value,
        error! => if error { 1 } else { 0 },
    }
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn fallible_error_recovery_rejects_a_different_success_type() {
    let checked = pipeline(
        r#"
extend[Value, Source, Target] Source!Value {
    fn orElse(self, fallback: &Fn(Source) Target!Value) Target!Value {
        match self {
            !value => !value,
            error! => fallback(error),
        }
    }
}

fn source() i32!i32 {
    1!
}

fn main() bool!i32 {
    source().orElse(&\error: i32 -> {
        if error == 1 {
            !42i64
        } else {
            true!
        }
    })
}
"#,
    );

    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("type mismatch in error-union success value")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn constructs_and_calls_readonly_and_mutable_callable_views() {
    let checked = pipeline(
        r#"
fn main(base: i32) i32 {
    let callback = \[base] value: i32 -> { base + value };
    let view: &Fn(i32) i32 = &callback;
    let mut mutable_callback = \[base] value: i32 -> { base + value };
    let mutable_view: &mut Fn(i32) i32 = &mut mutable_callback;
    view(1) + mutable_view(2)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let body = checked
        .ir
        .function_bodies
        .values()
        .next()
        .expect("main body");
    for binding in [&body.stmts[1], &body.stmts[3]] {
        let nia_body_ir::TypedStmtKind::Binding(binding) = &binding.kind else {
            panic!("expected callable view binding");
        };
        assert!(matches!(
            binding.value.as_ref().map(|value| &value.kind),
            Some(nia_body_ir::TypedExprKind::CallableCoercion { .. })
        ));
    }
    let tail = body.tail.as_deref().expect("main tail");
    let nia_body_ir::TypedExprKind::Call { callee, args } = &tail.kind else {
        panic!("expected outer operator call");
    };
    let nia_body_ir::TypedCallee::BuiltinOperator(_) = callee else {
        panic!("expected addition operator");
    };
    assert_eq!(args.len(), 2);
    for call in args {
        let nia_body_ir::TypedExprKind::Call { callee, .. } = &call.kind else {
            panic!("expected callable view call");
        };
        assert!(matches!(callee, nia_body_ir::TypedCallee::Callable(_)));
    }
}

#[test]
fn constructs_readonly_callable_view_from_mutable_closure_state() {
    let checked = pipeline(
        r#"
fn main(base: i32) i32 {
    let mut callback = \[base] value: i32 -> { base + value };
    let view: &Fn(i32) i32 = &mut callback;
    view(1)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let body = checked
        .ir
        .function_bodies
        .values()
        .next()
        .expect("main body");
    let nia_body_ir::TypedStmtKind::Binding(binding) = &body.stmts[1].kind else {
        panic!("expected callable view binding");
    };
    assert!(matches!(
        binding.value.as_ref().map(|value| &value.kind),
        Some(nia_body_ir::TypedExprKind::CallableCoercion { .. })
    ));
}

#[test]
fn rejects_callable_views_with_mismatched_signatures() {
    for target in ["&Fn(i64) i32", "&Fn(i32) i64"] {
        let checked = pipeline(&format!(
            r#"
fn main(base: i32) i32 {{
    let callback = \[base] value: i32 -> {{ base + value }};
    let view: {target} = &callback;
    0
}}
"#,
        ));
        assert!(checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("type mismatch in binding initializer")
        }));
    }
}

#[test]
fn rejects_readonly_closure_state_for_mutable_callable_view() {
    let checked = pipeline(
        r#"
fn main(base: i32) i32 {
    let callback = \[base] value: i32 -> { base + value };
    let view: &mut Fn(i32) i32 = &callback;
    0
}
"#,
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("type mismatch in binding initializer")
    }));
}

#[test]
fn callable_view_construction_requires_explicit_closure_address_syntax() {
    let checked = pipeline(
        r#"
fn main(base: i32) i32 {
    let callback = \[base] value: i32 -> { base + value };
    let state = &callback;
    let view: &Fn(i32) i32 = state;
    0
}
"#,
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("type mismatch in binding initializer")
    }));
}

#[test]
fn decays_no_capture_closure_to_thin_function_pointer() {
    let checked = pipeline(
        r#"
fn main() i32 {
    let callback = \value: i32 -> { value + 1 };
    let pointer: &fn(i32) i32 = &callback;
    pointer(2)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let body = checked
        .ir
        .function_bodies
        .values()
        .next()
        .expect("main body");
    let nia_body_ir::TypedStmtKind::Binding(binding) = &body.stmts[1].kind else {
        panic!("expected function pointer binding");
    };
    assert!(matches!(
        binding.value.as_ref().map(|value| &value.kind),
        Some(nia_body_ir::TypedExprKind::ClosureFunctionPointer { .. })
    ));
    assert!(matches!(
        body.tail.as_deref().map(|tail| &tail.kind),
        Some(nia_body_ir::TypedExprKind::Call {
            callee: nia_body_ir::TypedCallee::FunctionPointer(_),
            ..
        })
    ));
}

#[test]
fn rejects_capturing_closure_to_thin_function_pointer_with_dedicated_diagnostic() {
    let checked = pipeline(
        r#"
fn main(base: i32) i32 {
    let callback = \[base] value: i32 -> { base + value };
    let pointer: &fn(i32) i32 = &callback;
    0
}
"#,
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("capturing closures cannot be converted to thin function pointers")
    }));
}

#[test]
fn rejects_no_capture_closure_function_pointer_signature_mismatch() {
    let checked = pipeline(
        r#"
fn main() i32 {
    let callback = \value: i32 -> { value + 1 };
    let pointer: &fn(i64) i32 = &callback;
    0
}
"#,
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("type mismatch in binding initializer")
    }));
}

#[test]
fn thin_function_pointer_decay_requires_direct_readonly_closure_address() {
    for source in [
        r#"
fn main() i32 {
    let mut callback = \value: i32 -> { value + 1 };
    let pointer: &fn(i32) i32 = &mut callback;
    0
}
"#,
        r#"
fn main() i32 {
    let callback = \value: i32 -> { value + 1 };
    let state = &callback;
    let pointer: &fn(i32) i32 = state;
    0
}
"#,
    ] {
        let checked = pipeline(source);
        assert!(checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("type mismatch in binding initializer")
        }));
    }
}

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
fn rejects_builtin_function_pointers() {
    let checked = pipeline(
        r#"
@[builtin("trap")]
pub fn trap() never;

fn main() () {
    _ = &trap;
}
"#,
    );

    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("builtin function `trap` cannot be used as a function pointer")
        }),
        "{:?}",
        checked.diagnostics
    );
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
    let checked = pipeline_with_len_provider(
        r#"
struct Item {
    value: i32,
}

fn take(items: & [Item]) i32 {
    items.len() as i32
}

fn main() i32 {
    take(&[
        Item { value: 1 },
        Item { value: 2 },
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
    let mut xs: [i32; 3] = [10, 20, 30];
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
        Self { value }
    }
}

fn id[T](value: T) T { value }

fn main() i32 {
    let mut xs: [i32; 3] = [10, 20, 30];
    let mut i32: usize = 1;
    let mut indexed = xs[i32];
    let mut called: i32 = id[i32](indexed);
    let mut boxed = Box[i32]::make(called);
    indexed + boxed.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let counts = checked.facts.iter_node_bracket_suffix_resolutions().fold(
        (0usize, 0usize, 0usize),
        |(indexes, generic_calls, type_prefixes), (_, resolution)| match resolution {
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
            .iter_node_bracket_suffix_resolutions()
            .any(|(_, resolution)| matches!(resolution, BracketSuffixResolution::GenericCall)),
        "{:?}",
        checked
            .facts
            .iter_node_bracket_suffix_resolutions()
            .collect::<Vec<_>>()
    );
    assert!(
        checked
            .facts
            .iter_node_resolved_calls()
            .any(|(_, call)| matches!(call, nia_sema_ir::ResolvedCall::FunctionInstance { .. })),
        "{:?}",
        checked.facts.iter_node_resolved_calls().collect::<Vec<_>>()
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
            .iter_node_bracket_suffix_resolutions()
            .filter(|(_, resolution)| matches!(resolution, BracketSuffixResolution::GenericCall))
            .count(),
        2,
        "{:?}",
        checked
            .facts
            .iter_node_bracket_suffix_resolutions()
            .collect::<Vec<_>>()
    );
    assert_eq!(
        checked
            .facts
            .iter_node_resolved_calls()
            .filter(|(_, call)| matches!(call, nia_sema_ir::ResolvedCall::FunctionInstance { .. }))
            .count(),
        2,
        "{:?}",
        checked.facts.iter_node_resolved_calls().collect::<Vec<_>>()
    );
}

#[test]
fn records_runtime_generic_const_call_before_indexing_its_result() {
    let checked = pipeline(
        r#"
const fn pair[T](first: T, second: T) [T; 2] {
    [first, second]
}

fn main() u32 {
    pair[u32](1, 2)[1]
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert_eq!(
        checked
            .facts
            .iter_node_resolved_calls()
            .filter(|(_, call)| matches!(call, nia_sema_ir::ResolvedCall::FunctionInstance { .. }))
            .count(),
        1,
        "{:?}",
        checked.facts.iter_node_resolved_calls().collect::<Vec<_>>()
    );
}

#[test]
fn checks_simd_lane_builtins() {
    let checked = pipeline(
        r#"
fn lane(v: u8x16, i: usize) u8 {
    std::builtin::extract(v, i)
}

fn changed(v: u8x16, i: usize) u8x16 {
    std::builtin::insert(v, i, 9u8)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn rejects_invalid_simd_lane_builtins() {
    let checked = pipeline(
        r#"
fn invalid(v: u8x16) () {
    _ = std::builtin::extract[u8](v, 0usize);
    _ = std::builtin::extract(1u8, 0usize);
    _ = std::builtin::insert(v, 0usize, true);
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("builtin `extract` does not take a type argument")),
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
fn main() () {
    let mut value = 0i32;
    _ = std::builtin::atomic_load[i32](&value, 3usize);
    std::builtin::atomic_store[i32](&mut value, 1i32, 2usize);
    _ = std::builtin::cmpxchg_strong[i32](&mut value, 0i32, 1i32, 1usize, 3usize);
    _ = std::builtin::cmpxchg_strong[i32](&mut value, 0i32, 1i32, 3usize, 2usize);
    std::builtin::fence(1usize);
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
    assert_eq!(
        messages
            .iter()
            .filter(|message| message
                .contains("failure ordering cannot be stronger than or incomparable"))
            .count(),
        1,
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

fn main() () {
    let mut point = Point { x: 1 };
    _ = std::builtin::atomic_load[Point](&point, 1usize);
    _ = std::builtin::atomic_rmw[Point](&mut point, 1usize, Point { x: 2 }, 1usize);
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
    std::builtin::splat[u8x16](7u8)
}

fn invalid() u8 {
    std::builtin::splat[u8](1u8)
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
    std::builtin::bitmask(v == std::builtin::splat[u8x16](7u8))
}

fn invalid_value(v: u8x16) usize {
    std::builtin::bitmask(v)
}

fn invalid_type_arg(v: boolx16) usize {
    std::builtin::bitmask[usize](v)
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
            .contains("builtin `bitmask` does not take a type argument")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_bit_intrinsic_builtins() {
    let checked = pipeline(
        r#"
fn bits(mask: usize) usize {
    std::builtin::ctz[usize](mask) + std::builtin::clz[usize](mask) + std::builtin::popcount[usize](mask)
}

fn invalid_type(value: bool) usize {
    std::builtin::ctz[bool](value)
}

fn missing_type(value: usize) usize {
    std::builtin::popcount(value)
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
            .contains("builtin `popcount` requires exactly one type argument")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_load_unaligned_builtin() {
    let checked = pipeline(
        r#"
fn load(ptr: &u8) u8x8 {
    std::builtin::load_unaligned[u8x8](ptr)
}

fn invalid_ptr(ptr: &u16) u8x8 {
    std::builtin::load_unaligned[u8x8](ptr)
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
