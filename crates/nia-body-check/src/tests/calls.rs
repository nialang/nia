// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn checks_direct_calls_to_concrete_closure_values() {
    let checked = pipeline(
        r#"
fun main(base: i32) i32 {
    let callback = \[base] value: i32 -> { base + value };
    callback(2)
}

"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn statically_dispatches_function_item_through_callable_generic() {
    let checked = pipeline(
        r#"
fun apply[F](callback: F, value: i32) i32
where F: Fun(i32) i32
{
    callback(value)
}

fun increment(value: i32) i32 {
    value + 1
}

fun main() i32 {
    apply(increment, 2)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn static_callable_generic_infers_unannotated_closure_parameter_and_return() {
    let checked = pipeline(
        r#"
fun apply[Mapper, Output](callback: Mapper, value: i32) Output
where Mapper: Fun(i32) Output
{
    callback(value)
}

fun main() i32 {
    apply(\value -> value + 1, 2)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn static_callable_generic_infers_identity_return_without_result_context() {
    let checked = pipeline(
        r#"
fun apply[Mapper, Output](callback: Mapper) Output
where Mapper: Fun(i32) Output
{
    callback(1)
}

fun main() i32 {
    let result = apply(\value: i32 -> value);
    _ = result;
    0
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn static_callable_generic_infers_output_from_function_item() {
    let checked = pipeline(
        r#"
struct Target {
    value: i32,
}

fun makeTarget(value: i32) Target {
    Target { value }
}

fun apply[Mapper, Output](callback: Mapper, value: i32) Output
where Mapper: Fun(i32) Output
{
    callback(value)
}

fun main() Target {
    apply(makeTarget, 2)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn callable_generic_rejects_runtime_function_pointer() {
    let checked = pipeline(
        r#"
fun apply[F](callback: F, value: i32) i32
where F: Fun(i32) i32
{
    callback(value)
}

fun increment(value: i32) i32 {
    value + 1
}

fun main() i32 {
    let pointer: &fun(i32) i32 = &increment;
    apply(pointer, 2)
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.summary.contains("trait bound not satisfied") }),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn statically_dispatches_capturing_closure_through_callable_generic() {
    let checked = pipeline(
        r#"
fun apply[F](callback: F, value: i32) i32
where F: Fun(i32) i32
{
    callback(value)
}

fun main(base: i32) i32 {
    apply(\[base] value: i32 -> { base + value }, 2)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn explains_non_callable_callee_type_and_next_step() {
    let checked = pipeline(
        r#"
fun main() i32 {
    let value: i32 = 1;
    value()
}
"#,
    );
    let diagnostic = checked
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.summary == "callee is not callable")
        .expect("non-callable callee diagnostic");
    assert!(diagnostic.notes.iter().any(|note| {
        note.contains("callee has type `i32`") && note.contains("only function pointers")
    }));
    assert!(
        diagnostic
            .help
            .iter()
            .any(|help| help.contains("call a function or callable value"))
    );
}

#[test]
fn coerces_function_pointer_to_callable_view() {
    let checked = pipeline(
        r#"
fun add(value: i32) i32 { value + 1 }

fun apply(callback: &Fun(i32) i32, value: i32) i32 {
    callback(value)
}

fun main() i32 {
    apply(&add, 2)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn coerces_runtime_function_pointer_to_callable_view() {
    let checked = pipeline(
        r#"
fun add(value: i32) i32 { value + 1 }

fun main() i32 {
    let pointer: &fun(i32) i32 = &add;
    let callable: &Fun(i32) i32 = pointer;
    callable(2)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_closure_signature_from_callable_context() {
    let checked = pipeline(
        r#"
fun apply(callback: &Fun(i32) i32, value: i32) i32 {
    callback(value)
}

fun main() i32 {
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
fun main() i32 {
    let identity = \value -> { value };
    identity(3)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_let_bound_closure_when_passed_by_readonly_view() {
    let checked = pipeline(
        r#"
fun apply(callback: &Fun(i32) i32, value: i32) i32 {
    callback(value)
}

fun main() i32 {
    let increment = \value -> { value + 1 };
    apply(&increment, 2)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_let_bound_closure_for_generic_callable_view() {
    let checked = pipeline(
        r#"
fun apply[T](callback: &Fun(T) T, value: T) T {
    callback(value)
}

fun main() i32 {
    let identity = \value -> { value };
    apply(&identity, 2)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_let_bound_closure_for_mutable_callable_view() {
    let checked = pipeline(
        r#"
fun applyMut(callback: &mut Fun(i32) i32, value: i32) i32 {
    callback(value)
}

fun main() i32 {
    let mut increment = \value -> { value + 1 };
    applyMut(&mut increment, 2)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_let_bound_closure_for_explicit_generic_callable_view() {
    let checked = pipeline(
        r#"
fun apply[T](callback: &Fun(T) T, value: T) T {
    callback(value)
}

fun main() i32 {
    let identity = \value -> value;
    apply[i32](&identity, 2)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_nested_closure_signatures_across_later_calls() {
    let checked = pipeline(
        r#"
fun apply(callback: &Fun(i32) i32, value: i32) i32 {
    callback(value)
}

fun main() i32 {
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
fun main() i32 {
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
fun apply(callback: &Fun(i32) i32, value: i32) i32 {
    callback(value)
}

fun main() i32 {
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
fun main() i32 {
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
fn reports_both_uses_for_a_conflicting_closure_call() {
    let source = r#"
fun main() i32 {
    let identity = \value -> { value };
    _ = identity(1);
    _ = identity(true);
    0
}
"#;
    let checked = pipeline(source);
    let diagnostic = checked
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.summary == "closure uses have conflicting types")
        .unwrap_or_else(|| {
            panic!(
                "expected the closure conflict diagnostic: {:?}",
                checked.diagnostics
            )
        });
    assert_eq!(diagnostic.labels.len(), 2, "{diagnostic:?}");
    let first_call = source.find("identity(1)").expect("first call");
    let second_call = source.find("identity(true)").expect("second call");
    assert_eq!(diagnostic.primary_span().unwrap().start, second_call);
    assert_eq!(diagnostic.labels[1].span.start, first_call);
}

#[test]
fn reports_both_uses_when_a_closure_conflicts_through_a_generic_callee() {
    let source = r#"
fun apply[T](callback: &Fun(T) T, value: T) T {
    callback(value)
}

fun main() i32 {
    let identity = \value -> { value };
    _ = apply(&identity, 1);
    _ = apply(&identity, true);
    0
}
"#;
    let checked = pipeline(source);
    let diagnostic = checked
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.summary == "closure uses have conflicting types")
        .unwrap_or_else(|| {
            panic!(
                "expected the closure conflict diagnostic: {:?}",
                checked.diagnostics
            )
        });
    assert_eq!(diagnostic.labels.len(), 2, "{diagnostic:?}");
    let first_call = source.find("apply(&identity, 1)").expect("first call");
    let second_call = source.find("apply(&identity, true)").expect("second call");
    assert_eq!(diagnostic.primary_span().unwrap().start, second_call);
    assert_eq!(diagnostic.labels[1].span.start, first_call);
}

#[test]
fn closure_signature_contributes_to_generic_callable_inference() {
    let checked = pipeline(
        r#"
fun apply[T](callback: &Fun(T) T, value: T) T {
    callback(value)
}

fun main() i32 {
    apply(&\value -> { value }, 1)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn generic_callable_inference_accepts_implicit_closure_views() {
    let checked = pipeline(
        r#"
fun accepts[T](callback: &Fun(T) T) i32 {
    1
}

fun main() i32 {
    accepts(\value: i32 -> value)
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn mutable_closure_address_infers_generic_readonly_callable_signature() {
    let checked = pipeline(
        r#"
fun accepts[T](callback: &Fun(T) T) i32 {
    1
}

fun main() i32 {
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
fun accepts[T](callback: &Fun(T, T) T, value: T) T {
    callback(value, value)
}

fun main() bool {
    accepts(&\value: i32 -> { value }, true)
}
"#,
    );

    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("closure parameter count mismatch")
    }));
    assert!(
        !checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("conflicting inferred type for generic parameter")
        }),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn nested_closure_shape_mismatch_does_not_commit_earlier_substitutions() {
    let checked = pipeline(
        r#"
fun accepts[T, U](callback: &Fun(T, (U, U)) T, value: T) T {
    callback(value, (value, value))
}

fun main() bool {
    accepts(&\value: i32, pair: (bool,) -> { value }, true)
}
"#,
    );

    assert!(!checked.diagnostics.is_empty());
    assert!(
        !checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("conflicting inferred type for generic parameter")
        }),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn readonly_closure_address_does_not_infer_mutable_callable_generics() {
    let checked = pipeline(
        r#"
fun accepts[T](callback: &mut Fun(T) T, value: T) T {
    callback(value)
}

fun main() bool {
    accepts(&\value: i32 -> { value }, true)
}
"#,
    );

    assert!(!checked.diagnostics.is_empty());
    assert!(!checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("conflicting inferred type for generic parameter")
    }));
}

#[test]
fn generic_thin_function_pointer_infers_from_readonly_closure_address() {
    let checked = pipeline(
        r#"
fun accepts[T](callback: &fun(T) T) i32 {
    1
}

fun main() i32 {
    accepts(&\value: i32 -> { value })
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn generic_callable_infers_range_bound_from_closure_signature() {
    let checked = pipeline(
        r#"
fun accepts[T](callback: &Fun(T..T) T) i32 {
    1
}

fun main() i32 {
    accepts(&\bounds: usize..usize -> { bounds.end() })
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn generic_callable_infers_trait_object_argument_from_closure_signature() {
    let checked = pipeline(
        r#"
trait Consumer[T] {
    fun consume(&self, value: T) T;
}

fun accepts[T](callback: &Fun(&Consumer[T]) T) i32 {
    1
}

fun main() i32 {
    accepts(&\source: &Consumer[i32] -> {
        _ = source;
        1
    })
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn automatically_addresses_values_before_trait_object_coercion() {
    let checked = pipeline(
        r#"
trait Describe {
    fun value(&self) i32;
}

trait Bump {
    fun bump(&mut self) i32;
}

struct Number {
    value: i32,
}

trait Elements {
    fun count(&self) i32;
}

extend Number : Describe {
    fun value(&self) i32 {
        self.value
    }
}

extend Number : Bump {
    fun bump(&mut self) i32 {
        self.value += 1;
        self.value
    }
}

extend [Number] : Elements {
    fun count(&self) i32 {
        1
    }
}

fun describe(value: &Describe) i32 {
    value.value()
}

fun bump(value: &mut Bump) i32 {
    value.bump()
}

fun describeMany(values: &[&Describe]) i32 {
    values[0].value()
}

fun countElements(values: &Elements) i32 {
    values.count()
}

fun main() i32 {
    let mut number = Number { value: 7 };
    let numbers: [Number; 1] = [number];
    describe(number) + describeMany([number]) + countElements(numbers) + bump(number)
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(
        checked.facts.iter_node_expr_adjustments().any(|(_, plan)| {
            matches!(
                plan.steps.as_slice(),
                [ExprAdjustment::AddressOf(_), ExprAdjustment::TraitObject(_)]
            )
        }),
        "{:?}",
        checked
            .facts
            .iter_node_expr_adjustments()
            .collect::<Vec<_>>()
    );
    assert!(
        checked.facts.iter_node_expr_adjustments().any(|(_, plan)| {
            matches!(
                plan.steps.as_slice(),
                [
                    ExprAdjustment::AddressOf(_),
                    ExprAdjustment::PointerArrayToSlice(_),
                    ExprAdjustment::TraitObject(_)
                ]
            )
        }),
        "{:?}",
        checked
            .facts
            .iter_node_expr_adjustments()
            .collect::<Vec<_>>()
    );
}

#[test]
fn failed_implicit_trait_address_does_not_record_a_partial_adjustment() {
    let checked = pipeline(
        r#"
trait Marker {}

struct Number {}

fun accept(value: &Marker) () {
    _ = value;
}

fun main() () {
    let number = Number {};
    accept(number);
}
"#,
    );

    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("type mismatch in call argument 1")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .facts
            .iter_node_expr_adjustments()
            .all(|(_, plan)| !plan
                .steps
                .iter()
                .any(|step| matches!(step, ExprAdjustment::AddressOf(_)))),
        "{:?}",
        checked
            .facts
            .iter_node_expr_adjustments()
            .collect::<Vec<_>>()
    );
}

#[test]
fn generic_callable_argument_infers_return_from_direct_closure_pointer() {
    let checked = pipeline(
        r#"
extend[Value, Source, Target] Source!Value {
    fun mapError(self, mapper: &Fun(Source) Target) Target!Value {
        match self {
            !value => !value,
            error! => mapper(error)!,
        }
    }
}

fun source() i32!i32 {
    1!
}

fun main() bool!i32 {
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
    fun mapError(self, mapper: &Fun(Source) Target) Target!Value {
        match self {
            !value => !value,
            error! => mapper(error)!,
        }
    }
}

fun source() i32!i32 {
    1!
}

fun main() i32 {
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
fn map_error_infers_unannotated_closure_parameter_from_receiver() {
    let checked = pipeline(
        r#"
enum TargetError {
    Converted(i32),
}

extend[Value, Source, Target] Source!Value {
    fun mapError(self, mapper: &Fun(Source) Target) Target!Value {
        match self {
            !value => !value,
            error! => mapper(error)!,
        }
    }
}

fun map_error(source: i32!i32) TargetError!i32 {
    source.mapError(&\cause -> TargetError::Converted(cause))
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn static_callable_method_infers_map_closure_parameter_and_output() {
    let checked = pipeline(
        r#"
struct Source {
    value: i32,
}

struct Target {
    value: i32,
}

extend Source {
    fun intoTarget(&self) Target {
        Target { value: self.value }
    }
}

extend[Value] ?Value {
    fun map[Mapped, Mapper](self, mapper: Mapper) ?Mapped
    where Mapper: Fun(Value) Mapped
    {
        if self is ?value { ?mapper(value) } else { null }
    }
}

fun main(source: ?Source) ?Target {
    source.map(\value -> value.intoTarget())
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn callable_inference_uses_earlier_value_arguments_for_functions_and_methods() {
    for call in [
        "apply(makeSource(), \\value -> value.intoTarget())",
        "Runner {}.apply(makeSource(), \\value -> value.intoTarget())",
        "Runner::apply(Runner {}, makeSource(), \\value -> value.intoTarget())",
        "applyBorrowed(makeSource(), &\\value -> value.intoTarget())",
        "Runner {}.applyBorrowed(makeSource(), &\\value -> value.intoTarget())",
        "Runner::applyBorrowed(Runner {}, makeSource(), &\\value -> value.intoTarget())",
    ] {
        let checked = pipeline(&format!(
            r#"
struct Source {{ value: i32 }}
struct Target {{ value: i32 }}
struct Runner {{}}

fun makeSource() Source {{ Source {{ value: 42 }} }}

extend Source {{
    fun intoTarget(self) Target {{ Target {{ value: self.value }} }}
}}

fun apply[Input, Output, Mapper](value: Input, callback: Mapper) Output
where Mapper: Fun(Input) Output
{{
    callback(value)
}}

fun applyBorrowed[Input, Output](value: Input, callback: &Fun(Input) Output) Output {{
    callback(value)
}}

extend Runner {{
    fun apply[Input, Output, Mapper](self, value: Input, callback: Mapper) Output
    where Mapper: Fun(Input) Output
    {{
        callback(value)
    }}

    fun applyBorrowed[Input, Output](self, value: Input, callback: &Fun(Input) Output) Output {{
        callback(value)
    }}
}}

fun main() i32 {{
    let result = {call};
    _ = result;
    0
}}
"#,
        ));
        assert!(
            checked.diagnostics.is_empty(),
            "{call}: {:?}",
            checked.diagnostics
        );
    }
}

#[test]
fn generic_method_callable_infers_return_from_nested_method_call() {
    let checked = pipeline(
        r#"
struct Source {
    value: i32,
}

struct Target {
    value: i32,
}

extend Source {
    fun intoTarget(&self) Target {
        Target { value: self.value }
    }
}

extend[Value] ?Value {
    fun map[Mapped](self, mapper: &Fun(Value) Mapped) ?Mapped {
        match self {
            ?value => ?mapper(value),
            null => null,
        }
    }
}

fun main(source: ?Source) i32 {
    let mapped = source.map(&\value -> value.intoTarget());
    _ = mapped;
    0
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn generic_function_callable_infers_return_from_nested_method_call() {
    let checked = pipeline(
        r#"
struct Source {
    value: i32,
}

struct Target {
    value: i32,
}

extend Source {
    fun intoTarget(&self) Target {
        Target { value: self.value }
    }
}

fun map[Value, Mapped](value: ?Value, mapper: &Fun(Value) Mapped) ?Mapped {
    match value {
        ?present => ?mapper(present),
        null => null,
    }
}

fun main(source: ?Source) i32 {
    let mapped = map(source, &\value -> value.intoTarget());
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
    fun orElse(self, fallback: &Fun(Source) Target!Value) Target!Value {
        match self {
            !value => !value,
            error! => fallback(error),
        }
    }
}

fun source() i32!i32 {
    1!
}

fun main() i32 {
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
fn generic_fallible_callable_bound_infers_error_and_receiver_value() {
    let checked = pipeline(
        r#"
enum TargetError {
    Wrapped,
    _,
}

extend[Value, Source] Source!Value {
    fun orElse[Target, Fallback](self, fallback: Fallback) Target!Value
    where Fallback: Fun(Source) Target!Value
    {
        match self {
            !value => !value,
            error! => fallback(error),
        }
    }
}

fun source() i32!i32 {
    1!
}

fun main() i32 {
    let recovered = source().orElse(\error -> {
        if error == 1 {
            TargetError::Wrapped!
        } else {
            TargetError::Wrapped!
        }
    });
    match recovered {
        !value => value,
        error! => if error == TargetError::Wrapped { 1 } else { 0 },
    }
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn generic_function_fallible_callable_bound_contextualizes_error_arms() {
    let checked = pipeline(
        r#"
enum TargetError {
    Wrapped,
    _,
}

fun recover[Value, Source, Target, Fallback](
    input: Source!Value,
    fallback: Fallback,
) Target!Value
where Fallback: Fun(Source) Target!Value
{
    match input {
        !value => !value,
        error! => fallback(error),
    }
}

fun source() i32!i32 {
    1!
}

fun main() i32 {
    let recovered = recover(source(), \error -> {
        if error == 1 {
            TargetError::Wrapped!
        } else {
            TargetError::Wrapped!
        }
    });
    match recovered {
        !value => value,
        error! => if error == TargetError::Wrapped { 1 } else { 0 },
    }
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn explicit_callable_view_infers_error_union_closure_context() {
    let checked = pipeline(
        r#"
enum TargetError {
    Wrapped,
    _,
}

extend[Value, Source, Target] Source!Value {
    fun orElse(self, fallback: &Fun(Source) Target!Value) Target!Value {
        match self {
            !value => !value,
            error! => fallback(error),
        }
    }
}

fun source() i32!i32 {
    1!
}

fun main() i32 {
    let recovered = source().orElse(&\cause -> {
        if cause == 1 {
            TargetError::Wrapped!
        } else {
            TargetError::Wrapped!
        }
    });
    match recovered {
        !value => value,
        error! => if error == TargetError::Wrapped { 1 } else { 0 },
    }
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn explicit_callable_view_supplies_error_context_for_nested_propagation() {
    let checked = pipeline(
        r#"
enum Failure {
    Failed,
}

extend[Value, Source, Target] Source!Value {
    fun orElse(self, fallback: &Fun(Source) Target!Value) Target!Value {
        match self {
            !value => !value,
            error! => fallback(error),
        }
    }
}

fun fallible() Failure!i32 {
    Failure::Failed!
}

fun main() Failure!i32 {
    let input = fallible();
    input.orElse(&\cause -> !(fallible().?))
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn explicit_fallible_callable_view_infers_local_closure_as_one_signature() {
    let checked = pipeline(
        r#"
enum Failure {
    Failed,
    Unexpected,
    _,
}

extend[Value, Source, Target] Source!Value {
    fun orElse(self, fallback: &Fun(Source) Target!Value) Target!Value {
        match self {
            !value => !value,
            error! => fallback(error),
        }
    }
}

fun main() Failure!i32 {
    let fallback = \cause -> {
        if cause == 1 {
            Failure::Failed!
        } else {
            Failure::Unexpected!
        }
    };
    let input: i32!i32 = 1!;
    input.orElse(&fallback)
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn later_callable_context_resolves_generic_result_through_local_try() {
    let checked = pipeline(
        r#"
enum SourceError {
    Failed,
}

fun source[Value]() SourceError!Value {
    SourceError::Failed!
}

extend[Value, Source, Target] Source!Value {
    fun orElse(self, fallback: &Fun(Source) Target!Value) Target!Value {
        match self {
            !value => !value,
            error! => fallback(error),
        }
    }
}

fun main() SourceError!i32 {
    let fallback = \cause -> !(source().?);
    let input: i32!i32 = 1!;
    input.orElse(&fallback)
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn later_callable_context_connects_propagation_to_nested_local_closure() {
    for (operand, result) in [("?i32", "?value"), ("Failure!i32", "!value")] {
        let source = format!(
            r#"
enum Failure {{ Failed }}

fun invoke(callback: &Fun({operand}) {operand}, input: {operand}) {operand} {{
    callback(input)
}}

fun main(input: {operand}) {operand} {{
    let callback = \input -> {{
        let identity = \item -> item;
        let value = identity(input.?);
        {result}
    }};
    invoke(callback, input)
}}
"#
        );
        let checked = pipeline(&source);
        assert!(
            checked.diagnostics.is_empty(),
            "{operand}: {:?}",
            checked.diagnostics
        );
    }
}

#[test]
fn generic_propagation_payload_context_does_not_require_a_closure() {
    let checked = pipeline(
        r#"
enum Failure { Failed }

fun source[Value]() Failure!Value { Failure::Failed! }

fun main() Failure!i64 {
    !(source().?)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn generic_method_result_uses_solved_propagation_payload_context() {
    let checked = pipeline(
        r#"
enum Failure { Failed }

struct Factory {}
extend Factory {
    fun make[Value](&self) Failure!Value { Failure::Failed! }
}

fun invoke(callback: &Fun(i32) Failure!i64) Failure!i64 {
    callback(0)
}

fun main() Failure!i64 {
    let callback = \input -> {
        let factory = Factory {};
        !(factory.make().?)
    };
    invoke(callback)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn later_callable_context_keeps_propagation_error_conversion_independent() {
    let checked = pipeline(
        r#"
enum SourceError { Failed }
enum TargetError { Failed }

fun invoke(callback: &Fun(SourceError!i32) TargetError!i32) TargetError!i32 {
    callback(SourceError::Failed!)
}

fun main() TargetError!i32 {
    let callback = \input -> {
        let identity = \item -> item;
        let value = identity(input.?);
        !value
    };
    invoke(callback)
}
"#,
    );
    let messages = checked
        .diagnostics
        .iter()
        .map(|d| d.summary.as_str())
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("`SourceError` to implement `IntoError[TargetError]`"),
        "{messages:?}"
    );
}

#[test]
fn propagation_does_not_guess_an_unconstrained_generic_source_error() {
    let checked = pipeline(
        r#"
enum Failure { Failed }

fun source[Error, Value](value: Value) Error!Value { !value }

fun main() Failure!i32 {
    !(source(42).?)
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|d| d.summary.contains("cannot infer generic parameter `Error`")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn fallible_closure_context_flows_through_match_and_pattern_branches() {
    for body in [
        "match source() { !value => !value, error! => fallback(error), }",
        "if source() is error! { fallback(error) } else { !0 }",
        "if source() is error! and error == 1 { fallback(error) } else { !0 }",
        "match source() { !value => !value, error! => { let (left, right) = (error, 0); fallback(left + right) }, }",
    ] {
        let source = format!(
            r#"
enum Failure {{ Failed, Other }}
fun source() i32!i32 {{ 1! }}
fun main() Failure!i32 {{
    let fallback = \cause -> {{
        if cause == 1 {{ Failure::Failed! }} else {{ Failure::Other! }}
    }};
    {body}
}}
"#
        );
        let checked = pipeline(&source);
        assert!(
            checked.diagnostics.is_empty(),
            "{body}: {:?}",
            checked.diagnostics
        );
    }
}

#[test]
fn diverging_branches_do_not_constrain_contextual_call_payloads_to_unit() {
    for body in [
        "match source() { ?value => value, null => { return 0; }, }",
        "match source() { null => { return 0; }, ?value => value, }",
        "match source() { ?value => value, null => return 0, }",
        "if source() is ?value { value } else { return 0; }",
        "if source() is ?value and true { value } else { return 0; }",
        "match source() { ?value => value, null => { loop {} }, }",
        "match source() { ?value => value, null => stop(), }",
        "if source() is ?value { value } else { stop() }",
    ] {
        let source = format!(
            r#"
fun source[Value]() ?Value {{ null }}
fun stop() never {{ loop {{}} }}
fun consume(value: i64) i64 {{ value }}
fun main() i32 {{
    let identity = \item -> item;
    let value = {body};
    _ = consume(identity(value));
    0
}}
"#
        );
        let checked = pipeline(&source);
        assert!(
            checked.diagnostics.is_empty(),
            "{body}: {:?}",
            checked.diagnostics
        );
    }
}

#[test]
fn propagation_payload_is_independent_of_the_enclosing_success_type() {
    let checked = pipeline(
        r#"
enum Failure { Failed }
fun source[Value]() Failure!Value { Failure::Failed! }
fun main() Failure!bool {
    let value: i64 = source().?;
    _ = value;
    !true
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn fallible_callable_bound_still_rejects_a_runtime_erased_view() {
    let checked = pipeline(
        r#"
enum Failure { Failed }
fun invoke[Callback](callback: Callback) Failure!i32
where Callback: Fun(i32) Failure!i32
{
    callback(1)
}
fun main() Failure!i32 {
    let callback: &Fun(i32) Failure!i32 = \value -> !value;
    invoke(callback)
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|d| d.summary.contains("trait bound not satisfied")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn propagation_and_operator_constraints_share_a_fixed_point() {
    let checked = pipeline(
        r#"
struct Boxed {}
extend Boxed : Add[i32] {
    type Output = ?i32;
    fun add(self, rhs: i32) ?i32 { ?rhs }
}
fun main() i32 {
    let callback = \input -> {
        let identity = \value -> value;
        let one: i32 = 1;
        let value = identity((input + one).?);
        input + value
    };
    _ = callback(Boxed {});
    0
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn explicit_callable_view_infers_nested_error_union_and_keeps_conversion_boundary() {
    let checked = pipeline(
        r#"
enum SourceError {
    Failed,
}

enum TargetError {
    Wrapped(SourceError),
}

extend[Value, Source, Target] Source!Value {
    fun orElse(self, fallback: &Fun(Source) Target!Value) Target!Value {
        match self {
            !value => !value,
            error! => fallback(error),
        }
    }
}

fun source() SourceError!i32 {
    SourceError::Failed!
}

fun propagate() SourceError!i32 {
    let result = source().orElse(&\cause -> {
        if cause == SourceError::Failed {
            SourceError::Failed!
        } else {
            SourceError::Failed!
        }
    });
    !(result.?)
}

fun main() i32 {
    match propagate() {
        !value => value,
        error! => if error == SourceError::Failed { 1 } else { 0 },
    }
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn explicit_callable_view_does_not_infer_into_error_conversion_as_type_equality() {
    let checked = pipeline(
        r#"
enum SourceError {
    Failed,
}

enum TargetError {
    Wrapped(SourceError),
}

enum UnrelatedError {
    Failed,
}

extend[Value, Source, Target] Source!Value {
    fun orElse(self, fallback: &Fun(Source) Target!Value) Target!Value {
        match self {
            !value => !value,
            error! => fallback(error),
        }
    }
}

fun source() SourceError!i32 {
    SourceError::Failed!
}

fun propagate() TargetError!i32 {
    let result = source().orElse(&\cause -> {
        if cause == SourceError::Failed {
            UnrelatedError::Failed!
        } else {
            UnrelatedError::Failed!
        }
    });
    !(result.?)
}

fun main() i32 {
    0
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
            .any(|message| message.contains("IntoError[TargetError]")),
        "expected IntoError conversion failure at `.?`, got {messages:?}"
    );
    assert!(
        !messages
            .iter()
            .any(|message| message.contains("cannot infer closure parameter")),
        "callable view should contextualize closure parameter: {messages:?}"
    );
}

#[test]
fn callable_bound_propagates_error_union_shape_through_local_call() {
    let checked = pipeline(
        r#"
enum Failure {
    Failed,
}

fun invoke[Callback](callback: Callback) Failure!i32
where Callback: Fun() Failure!i32
{
    let identity = \value -> value;
    !(identity(callback().?))
}

fun main() i32 {
    match invoke(\-> Failure::Failed!) {
        !value => value,
        error! => 1,
    }
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn generic_callable_bound_infers_nominal_constructor_return() {
    let checked = pipeline(
        r#"
enum TargetError {
    Wrapped(i32),
}

extend[Value, Source, Target] Source!Value {
    fun mapError(self, mapper: &Fun(Source) Target) Target!Value {
        match self {
            !value => !value,
            error! => mapper(error)!,
        }
    }
}

fun source() i32!i32 {
    1!
}

fun main() i32 {
    let mapped = source().mapError(\cause -> TargetError::Wrapped(cause));
    match mapped {
        !value => value,
        error! => match error {
            .Wrapped(value) => value,
        },
    }
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn trait_default_method_infers_nominal_error_mapping_from_callable_bound() {
    let checked = pipeline(
        r#"
enum SourceError {
    Failed,
}

enum ReallocError {
    Allocation(SourceError),
}

trait Allocator {
    fun allocate(&mut self) SourceError!i32;

    fun reallocate(&mut self) ReallocError!i32 {
        let value = self.allocate()
            .mapError(\cause -> ReallocError::Allocation(cause))
            .?;
        !value
    }
}

extend[Value, Source, Target] Source!Value {
    fun mapError(self, mapper: &Fun(Source) Target) Target!Value {
        match self {
            !value => !value,
            error! => mapper(error)!,
        }
    }
}

fun main() i32 {
    0
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn closure_callable_error_inference_keeps_into_error_as_conversion_obligation() {
    let checked = pipeline(
        r#"
enum SourceError {
    Failed,
}

enum ReallocError {
    Allocation(SourceError),
}

enum UnrelatedError {
    Failed,
}

trait Allocator {
    fun allocate(&mut self) SourceError!i32;

    fun reallocate(&mut self) ReallocError!i32 {
        let value = self.allocate()
            .mapError(\cause -> UnrelatedError::Failed)
            .?;
        !value
    }
}

extend[Value, Source] Source!Value {
    fun mapError[Target, Mapper](self, mapper: Mapper) Target!Value
    where Mapper: Fun(Source) Target
    {
        match self {
            !value => !value,
            error! => mapper(error)!,
        }
    }
}

fun main() i32 {
    0
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
            .any(|message| message.contains("IntoError[ReallocError]")),
        "expected a direct error-conversion diagnostic, got {messages:?}"
    );
    assert!(
        !messages
            .iter()
            .any(|message| message.contains("cannot infer closure parameter")),
        "the callable bound should still provide the closure's source-error context: {messages:?}"
    );
}

#[test]
fn closure_inference_preserves_optional_propagation_shape() {
    let checked = pipeline(
        r#"
fun main(value: ?i32) ?i32 {
    let unwrapped = value.?;
    let increment = \item: i32 -> item + 1;
    ?increment(unwrapped)
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
    fun orElse(self, fallback: &Fun(Source) Target!Value) Target!Value {
        match self {
            !value => !value,
            error! => fallback(error),
        }
    }
}

fun source() i32!i32 {
    1!
}

fun main() bool!i32 {
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
fun main(base: i32) i32 {
    let callback = \[base] value: i32 -> { base + value };
    let view: &Fun(i32) i32 = &callback;
    let mut mutable_callback = \[base] value: i32 -> { base + value };
    let mutable_view: &mut Fun(i32) i32 = &mut mutable_callback;
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
fn implicitly_converts_closure_values_to_callable_views() {
    let checked = pipeline(
        r#"
fun apply(callback: &Fun(i32) i32, value: i32) i32 {
    callback(value)
}

fun main(base: i32) i32 {
    let add_base = \[base] value: i32 -> base + value;
    apply(add_base, 3) + apply(\value -> value + 1, 4)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let body = checked
        .ir
        .function_bodies
        .values()
        .find(|body| body.stmts.len() == 1)
        .expect("main body");
    let tail = body.tail.as_deref().expect("main tail");
    let nia_body_ir::TypedExprKind::Call { args, .. } = &tail.kind else {
        panic!("expected addition call");
    };
    for call in args {
        let nia_body_ir::TypedExprKind::Call { args, .. } = &call.kind else {
            panic!("expected apply call");
        };
        assert!(matches!(
            args.first().map(|arg| &arg.kind),
            Some(nia_body_ir::TypedExprKind::CallableCoercion { .. })
        ));
    }
}

#[test]
fn implicitly_converts_writable_closure_values_to_mutable_callable_views() {
    let checked = pipeline(
        r#"
fun apply(callback: &mut Fun(i32) i32, value: i32) i32 {
    callback(value)
}

fun main() i32 {
    let mut total = 0;
    let mut accumulate = \[&mut total] value: i32 -> {
        total.* += value;
        total.*
    };
    apply(accumulate, 3) + total
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn implicit_mutable_callable_view_requires_a_writable_closure_place() {
    let checked = pipeline(
        r#"
fun apply(callback: &mut Fun(i32) i32, value: i32) i32 {
    callback(value)
}

fun main() i32 {
    let callback = \value: i32 -> value + 1;
    apply(callback, 2)
}
"#,
    );

    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.summary.contains("expected &mut Fun(i32) i32") })
    );
}

#[test]
fn constructs_readonly_callable_view_from_mutable_closure_state() {
    let checked = pipeline(
        r#"
fun main(base: i32) i32 {
    let mut callback = \[base] value: i32 -> { base + value };
    let view: &Fun(i32) i32 = &mut callback;
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
    for target in ["&Fun(i64) i32", "&Fun(i32) i64"] {
        let checked = pipeline(&format!(
            r#"
fun main(base: i32) i32 {{
    let callback = \[base] value: i32 -> {{ base + value }};
    let view: {target} = &callback;
    0
}}
"#,
        ));
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.summary.contains("type mismatch") })
        );
    }
}

#[test]
fn rejects_readonly_closure_state_for_mutable_callable_view() {
    let checked = pipeline(
        r#"
fun main(base: i32) i32 {
    let callback = \[base] value: i32 -> { base + value };
    let view: &mut Fun(i32) i32 = &callback;
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
fn callable_view_conversion_does_not_reinterpret_a_stored_closure_state_pointer() {
    let checked = pipeline(
        r#"
fun main(base: i32) i32 {
    let callback = \[base] value: i32 -> { base + value };
    let state = &callback;
    let view: &Fun(i32) i32 = state;
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
fun main() i32 {
    let callback = \value: i32 -> { value + 1 };
    let pointer: &fun(i32) i32 = &callback;
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
fun main(base: i32) i32 {
    let callback = \[base] value: i32 -> { base + value };
    let pointer: &fun(i32) i32 = &callback;
    0
}
"#,
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("capturing closures cannot be converted to thin function pointers")
    }));
    let diagnostic = checked
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic
                .summary
                .contains("capturing closures cannot be converted to thin function pointers")
        })
        .expect("capturing closure diagnostic");
    assert!(diagnostic.notes.iter().any(
        |note| note.contains("captures 1 value(s)") && note.contains("no environment storage")
    ));
    assert!(
        diagnostic
            .help
            .iter()
            .any(|help| help.contains("callable view") && help.contains("&Fun(...)"))
    );
}

#[test]
fn rejects_no_capture_closure_function_pointer_signature_mismatch() {
    let checked = pipeline(
        r#"
fun main() i32 {
    let callback = \value: i32 -> { value + 1 };
    let pointer: &fun(i64) i32 = &callback;
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
fun main() i32 {
    let mut callback = \value: i32 -> { value + 1 };
    let pointer: &fun(i32) i32 = &mut callback;
    0
}
"#,
        r#"
fun main() i32 {
    let callback = \value: i32 -> { value + 1 };
    let state = &callback;
    let pointer: &fun(i32) i32 = state;
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
fun id(x: i32) i32 { x }
fun main() i32 {
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
pub fun trap() never;

fun main() () {
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
fn rejects_tracked_caller_function_pointers() {
    let checked = pipeline(
        r#"
@[trackCaller]
fun tracked() i32 { 1 }

fun main() () {
    _ = &tracked;
}

"#,
    );

    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic.summary.contains(
            "tracked-caller functions cannot be used as ordinary function pointers or callable values"
        )),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn records_exact_tracked_callsite_location_in_typed_ir() {
    let checked =
        pipeline("@[trackCaller]\nfun tracked() i32 { 1 }\n\nfun main() i32 {\n    tracked()\n}\n");
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let main = checked
        .ir
        .function_bodies
        .values()
        .find(|body| {
            body.tail.as_deref().is_some_and(|tail| {
                matches!(
                    tail.kind,
                    nia_body_ir::TypedExprKind::Call {
                        callee: nia_body_ir::TypedCallee::Tracked { .. },
                        ..
                    }
                )
            })
        })
        .expect("main body with tracked call");
    let nia_body_ir::TypedExprKind::Call {
        callee: nia_body_ir::TypedCallee::Tracked { location, .. },
        ..
    } = &main.tail.as_deref().expect("main tail").kind
    else {
        unreachable!()
    };
    assert_eq!(location.line, 5);
    assert_eq!(location.column, 5);
}

#[test]
fn checks_direct_call_argument_count_and_types() {
    let checked = pipeline(
        r#"
fun add(a: i32, b: i32) i32 { a + b }

fun main(flag: bool) i32 {
    _ = add(flag, 1);
    _ = add(1);
    0
}
"#,
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("type mismatch in call argument 1")
    }));
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("argument count mismatch"))
    );
    let arity = checked
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.summary.contains("argument count mismatch"))
        .expect("call arity diagnostic");
    assert!(arity.primary_span().is_some_and(|span| span.start > 0));
    assert!(arity.notes.iter().any(|note| {
        note.contains("this call accepts 2 argument(s)") && note.contains("received 1")
    }));
    assert!(
        arity
            .help
            .iter()
            .any(|help| help.contains("add the missing arguments"))
    );
}

#[test]
fn checks_aggregate_literals_from_call_argument_context() {
    let checked = pipeline_with_len_provider(
        r#"
struct Item {
    value: i32,
}

fun take(items: & [Item]) i32 {
    items.len() as i32
}

fun main() i32 {
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
fun id[T](value: T) T { value }
fun pair[T](left: T, right: T) T { left }

fun main(flag: bool) i32 {
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
fun id[T](value: T) T { value }

fun main() i32 {
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
        checked.diagnostics.iter().all(|diagnostic| {
            !diagnostic
                .summary
                .contains("function values are not supported")
        }),
        "function items are valid zero-sized static values: {:?}",
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
    fun make(value: T) Box[T] {
        Self { value }
    }
}

fun id[T](value: T) T { value }

fun main() i32 {
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
extern fun log(value: i32);

fun id[T](value: T) T {
    value
}

fun main() i32 {
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
fun id[T](value: T) T {
    value
}

fun main() i32 {
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
const fun pair[T](first: T, second: T) [T; 2] {
    [first, second]
}

fun main() u32 {
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
fun lane(v: u8x16, i: usize) u8 {
    std::builtin::extract(v, i)
}

fun changed(v: u8x16, i: usize) u8x16 {
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
fun invalid(v: u8x16) () {
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
fn simd_builtins_do_not_reject_error_recovery_types() {
    let checked = pipeline(
        r#"
fun invalid() usize {
    _ = std::builtin::extract(missing, 0usize);
    _ = std::builtin::bitmask(missing);
    _ = std::builtin::splat[MissingVector](1u8);
    _ = std::builtin::ctz[MissingInteger](0usize);
    0
}
"#,
    );

    assert!(
        checked.diagnostics.iter().all(|diagnostic| {
            !diagnostic.summary.contains("SIMD")
                && !diagnostic.summary.contains("integer type argument")
        }),
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| { diagnostic.summary.contains("unknown value `missing`") })
            .count(),
        2,
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| { diagnostic.summary.contains("unknown type `Missing") })
            .count(),
        2,
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn builtins_do_not_reject_structural_error_recovery_types() {
    let checked = pipeline(
        r#"
fun invalid() () {
    _ = std::builtin::extract(&missing_vector, 0usize);
    _ = std::builtin::bitmask([missing_mask]);
    _ = std::builtin::loadUnaligned[i32](&missing_pointer);
    _ = std::builtin::sliceLen(&missing_slice);
}
"#,
    );
    let summaries = checked
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    for name in [
        "missing_vector",
        "missing_mask",
        "missing_pointer",
        "missing_slice",
    ] {
        assert!(
            summaries
                .iter()
                .any(|summary| summary.contains(&format!("unknown value `{name}`"))),
            "missing {name} root: {summaries:?}"
        );
    }
    assert!(
        summaries.iter().all(|summary| {
            !summary.contains("requires a SIMD")
                && !summary.contains("requires a byte pointer")
                && !summary.contains("requires a slice pointer")
        }),
        "{summaries:?}"
    );
}

#[test]
fn simd_builtin_type_argument_rejects_a_resolved_const_value() {
    let checked = pipeline(
        r#"
const LANE: usize = 1;

fun invalid() usize {
    _ = std::builtin::ctz[LANE](0usize);
    0
}
"#,
    );

    assert_eq!(checked.diagnostics.len(), 1, "{:?}", checked.diagnostics);
    assert_eq!(
        checked.diagnostics[0].summary,
        "generic argument resolved as a value; expected a type"
    );
}

#[test]
fn checks_atomic_builtin_ordering_rules() {
    let checked = pipeline(
        r#"
fun main() () {
    let mut value = 0i32;
    _ = std::builtin::atomicLoad[i32](&value, 3usize);
    std::builtin::atomicStore[i32](&mut value, 1i32, 2usize);
    _ = std::builtin::cmpxchgStrong[i32](&mut value, 0i32, 1i32, 1usize, 3usize);
    _ = std::builtin::cmpxchgStrong[i32](&mut value, 0i32, 1i32, 3usize, 2usize);
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
fn atomic_builtins_do_not_reject_error_recovery_types() {
    let checked = pipeline(
        r#"
fun invalid() () {
    let mut value = 0i32;
    _ = std::builtin::atomicLoad[Missing](value, 1usize);
    _ = std::builtin::atomicRmw[Missing](&mut value, 99usize, 1i32, 1usize);
}
"#,
    );

    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("unknown type `Missing`"))
            .count(),
        2,
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("invalid atomic RMW operation `99`")
        }),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("builtin `atomicLoad` requires a pointer argument")
        }),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().all(|diagnostic| {
            !diagnostic.summary.contains("supports only bool")
                && !diagnostic.summary.contains("atomic pointer argument")
        }),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn atomic_builtins_do_not_const_check_unresolved_control_arguments() {
    let checked = pipeline(
        r#"
fun invalid() () {
    let mut value = 0i32;
    _ = std::builtin::atomicLoad[i32](value, missing_order);
    _ = std::builtin::atomicRmw[i32](&mut value, missing_operation, 1i32, 1usize);
}
"#,
    );

    let summaries = checked
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        summaries
            .iter()
            .filter(|summary| summary.contains("unknown value"))
            .count(),
        2,
        "{summaries:?}"
    );
    assert!(
        summaries.iter().all(|summary| {
            !summary.contains("compile-time integer constant")
                && !summary.contains("invalid atomic ordering")
                && !summary.contains("invalid atomic RMW operation")
        }),
        "{summaries:?}"
    );
}

#[test]
fn rejects_non_atomic_builtin_value_types() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
}

fun main() () {
    let mut point = Point { x: 1 };
    _ = std::builtin::atomicLoad[Point](&point, 1usize);
    _ = std::builtin::atomicRmw[Point](&mut point, 1usize, Point { x: 2 }, 1usize);
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
fun make() u8x16 {
    std::builtin::splat[u8x16](7u8)
}

fun invalid() u8 {
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
fun mask(v: u8x16) usize {
    std::builtin::bitmask(v == std::builtin::splat[u8x16](7u8))
}

fun invalid_value(v: u8x16) usize {
    std::builtin::bitmask(v)
}

fun invalid_type_arg(v: boolx16) usize {
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
fun bits(mask: usize) usize {
    std::builtin::ctz[usize](mask) + std::builtin::clz[usize](mask) + std::builtin::popcount[usize](mask)
}

fun invalid_type(value: bool) usize {
    std::builtin::ctz[bool](value)
}

fun missing_type(value: usize) usize {
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
fun load(ptr: &u8) u8x8 {
    std::builtin::loadUnaligned[u8x8](ptr)
}

fun invalid_ptr(ptr: &u16) u8x8 {
    std::builtin::loadUnaligned[u8x8](ptr)
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

#[test]
fn load_unaligned_does_not_reject_error_recovery_pointers() {
    let checked = pipeline(
        r#"
fun invalid() u8x8 {
    std::builtin::loadUnaligned[u8x8](missing)
}
"#,
    );

    assert_eq!(checked.diagnostics.len(), 1, "{:?}", checked.diagnostics);
    assert!(
        checked.diagnostics[0]
            .summary
            .contains("unknown value `missing`"),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn slice_len_does_not_reject_error_recovery_pointers() {
    let checked = pipeline(
        r#"
fun invalid() usize {
    std::builtin::sliceLen(missing)
}
"#,
    );

    assert_eq!(checked.diagnostics.len(), 1, "{:?}", checked.diagnostics);
    assert!(
        checked.diagnostics[0]
            .summary
            .contains("unknown value `missing`"),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn slice_len_still_rejects_non_pointer_arguments() {
    let checked = pipeline(
        r#"
fun invalid() usize {
    std::builtin::sliceLen(1usize)
}
"#,
    );

    assert_eq!(checked.diagnostics.len(), 1, "{:?}", checked.diagnostics);
    assert!(
        checked.diagnostics[0]
            .summary
            .contains("builtin `sliceLen` requires a slice pointer"),
        "{:?}",
        checked.diagnostics
    );
}
