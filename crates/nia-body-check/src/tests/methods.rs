// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn failed_bracket_base_preserves_type_readings_and_independent_value_errors() {
    for (expression, independent_errors) in [
        ("missing[i32](1)", 0),
        ("missing[i32, bool](1)", 0),
        ("&missing[i32]", 0),
        ("missing[need(true)](1)", 1),
        ("missing[i32](need(true))", 1),
        ("&missing[need(true)]", 1),
    ] {
        let source =
            format!("fun need(value: i32) i32 {{ value }}\nfun main() {{ _ = {expression}; }}");
        let checked = pipeline(&source);
        assert_eq!(
            checked.diagnostics.len(),
            1 + independent_errors,
            "{expression}: {:?}",
            checked.diagnostics
        );
        assert_eq!(
            checked
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.summary == "unknown value `missing`")
                .count(),
            1
        );
        assert_eq!(
            checked
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.summary.contains("expected i32, got bool"))
                .count(),
            independent_errors
        );
    }
}

#[test]
fn failed_callable_selection_keeps_independent_argument_errors() {
    let prelude = r#"
struct Pair[A, B] { a: A, b: B }
extend[T] Pair[T, i32] {
    fun rank(self, value: i32) i32 { value }
    fun make(value: i32) i32 { value }
}
extend[U] Pair[i32, U] {
    fun rank(self, value: i32) i32 { value }
    fun make(value: i32) i32 { value }
}
fun need(value: i32) i32 { value }
"#;
    for (call, root) in [
        (
            "Pair[i32, i32]::missing(need(true))",
            "unknown associated function `missing`",
        ),
        (
            "Pair[i32, i32]::missing[i32](need(true))",
            "unknown associated function `missing`",
        ),
        (
            "Pair[i32, i32]::make(need(true))",
            "ambiguous method `make`",
        ),
        (
            "Pair[i32, i32]::make[i32](need(true))",
            "ambiguous method `make`",
        ),
        (
            "pair.rank(need(true))",
            "no matching method overload `rank`",
        ),
        (
            "pair.rank[i32](need(true))",
            "no matching method overload `rank`",
        ),
        ("true.missing(need(true))", "unknown method `missing`"),
    ] {
        let source = format!("{prelude}\nfun main(pair: Pair[i32, i32]) {{ {call}; }}");
        let checked = pipeline(&source);
        let summaries = checked
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.summary.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            summaries
                .iter()
                .filter(|summary| summary.contains(root))
                .count(),
            1,
            "{call}: {summaries:?}"
        );
        assert_eq!(
            summaries
                .iter()
                .filter(|summary| summary
                    .contains("type mismatch in call argument 1: expected i32, got bool"))
                .count(),
            1,
            "{call}: {summaries:?}"
        );
        assert_eq!(
            summaries.len(),
            2,
            "{call}: unexpected fallback diagnostics: {summaries:?}"
        );
    }
}

#[test]
fn callable_field_fallback_checks_arguments_once() {
    for (argument, errors) in [("1", 0), ("true", 1)] {
        let source = format!(
            "struct Callable {{ invoke: &fun(i32) i32 }}\nfun need(value: i32) i32 {{ value }}\nfun main(value: Callable) i32 {{ value.invoke(need({argument})) }}"
        );
        let checked = pipeline(&source);
        assert_eq!(
            checked.diagnostics.len(),
            errors,
            "{:?}",
            checked.diagnostics
        );
        if errors != 0 {
            assert_eq!(
                checked.diagnostics[0].summary,
                "type mismatch in call argument 1: expected i32, got bool"
            );
        }
    }
}

#[test]
fn failed_trait_callable_selection_keeps_independent_argument_errors() {
    let prelude = r#"
trait Left { fun rank(&self, value: i32) i32; }
trait Right { fun rank(&self, value: i32) i32; }
trait Both: Left + Right {}
trait FactoryLeft { fun make(value: i32) i32; }
trait FactoryRight { fun make(value: i32) i32; }
trait Factories: FactoryLeft + FactoryRight {}
fun need(value: i32) i32 { value }
"#;
    for (signature, call, root) in [
        (
            "fun main[T](value: T) where T: Both",
            "value.rank(need(true))",
            "ambiguous trait method `rank`",
        ),
        (
            "fun main[T](value: T) where T: Factories",
            "[T]::make(need(true))",
            "ambiguous trait associated function `make`",
        ),
        (
            "fun main(value: &Both)",
            "value.rank(need(true))",
            "ambiguous dynamic trait method `rank`",
        ),
    ] {
        let source = format!("{prelude}\n{signature} {{ {call}; }}");
        let checked = pipeline(&source);
        let summaries = checked
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.summary.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            summaries
                .iter()
                .filter(|summary| summary.contains(root))
                .count(),
            1,
            "{call}: {summaries:?}"
        );
        assert_eq!(
            summaries
                .iter()
                .filter(|summary| summary
                    .contains("type mismatch in call argument 1: expected i32, got bool"))
                .count(),
            1,
            "{call}: {summaries:?}"
        );
        assert_eq!(
            summaries.len(),
            2,
            "{call}: unexpected fallback diagnostics: {summaries:?}"
        );
        let ambiguity = checked
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.summary == root)
            .expect("ambiguity diagnostic");
        let traits = if call.starts_with("[T]") {
            ["FactoryLeft::make", "FactoryRight::make"]
        } else {
            ["Left::rank", "Right::rank"]
        };
        assert_eq!(ambiguity.related.len(), 2, "{ambiguity:?}");
        for (related, identity) in ambiguity.related.iter().zip(traits) {
            assert!(related.message.contains(identity), "{related:?}");
            assert!(source[related.span.start..related.span.end].contains("fun "));
        }
        assert!(
            ambiguity
                .help
                .iter()
                .any(|help| help.contains("narrow the receiver"))
        );
    }
}

#[test]
fn associated_candidate_signatures_keep_all_ordinary_parameters() {
    let source = r#"
struct Pair[A, B] { a: A, b: B }
extend[T] Pair[T, i32] { fun make(value: i32, flag: bool) i32 { value } }
extend[U] Pair[i32, U] { fun make(value: i32, flag: bool) i32 { value } }
fun main() i32 { Pair[i32, i32]::make(1, true) }
"#;
    let checked = pipeline(source);
    let diagnostic = checked
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.summary == "ambiguous method `make`")
        .expect("ambiguous associated call");
    let note = diagnostic
        .notes
        .iter()
        .find(|note| note.starts_with("candidate methods:"))
        .expect("candidate signatures");
    assert_eq!(
        note.matches("make(i32, bool) -> i32").count(),
        2,
        "{diagnostic:?}"
    );
    assert_eq!(diagnostic.related.len(), 2);
    for related in diagnostic.related.iter() {
        assert!(
            source[related.span.start..related.span.end]
                .contains("fun make(value: i32, flag: bool)")
        );
    }
    assert!(!diagnostic.notes.iter().any(|note| note.contains("omitted")));
}

#[test]
fn trait_candidate_reports_sort_before_applying_the_display_limit() {
    for (signature, call, receiver) in [
        (
            "fun main[T](value: T) where T: All",
            "value.rank()",
            "&self",
        ),
        ("fun main(value: &All)", "value.rank()", "&self"),
        ("fun main[T](value: T) where T: All", "[T]::rank()", ""),
    ] {
        let mut notes = Vec::new();
        for reverse in [false, true] {
            let order = if reverse {
                (0..10).rev().collect::<Vec<_>>()
            } else {
                (0..10).collect()
            };
            let traits = order
                .iter()
                .map(|index| format!("trait Candidate{index} {{ fun rank({receiver}) i32; }}\n"))
                .collect::<String>();
            let bounds = order
                .iter()
                .map(|index| format!("Candidate{index}"))
                .collect::<Vec<_>>()
                .join(" + ");
            let source = format!("{traits}trait All: {bounds} {{}}\n{signature} {{ _ = {call}; }}");
            let checked = pipeline(&source);
            assert_eq!(checked.diagnostics.len(), 1, "{:?}", checked.diagnostics);
            let diagnostic = &checked.diagnostics[0];
            assert_eq!(diagnostic.related.len(), 8, "{diagnostic:?}");
            for (index, related) in diagnostic.related.iter().enumerate() {
                assert!(
                    related.message.contains(&format!("Candidate{index}::rank")),
                    "{related:?}"
                );
            }
            assert!(
                diagnostic
                    .notes
                    .iter()
                    .any(|note| note == "2 additional candidate(s) omitted")
            );
            notes.push(diagnostic.notes.clone());
        }
        assert_eq!(notes[0], notes[1]);
    }
}

#[test]
fn narrowing_trait_bounds_resolves_reported_ambiguities() {
    let checked = pipeline(
        r#"
trait Left { fun rank(&self) i32; }
trait Factory { fun make(value: i32) i32; }
fun bounded[T](value: T) i32 where T: Left { value.rank() }
fun dynamic(value: &Left) i32 { value.rank() }
fun associated[T](value: T) i32 where T: Factory { _ = value; [T]::make(1) }
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn trait_candidate_identities_distinguish_const_arguments() {
    let checked = pipeline(
        r#"
trait SizedRank[N: usize] { fun rank(&self) i32; }
trait Both: SizedRank[1] + SizedRank[2] {}
fun main[T](value: T) i32 where T: Both { value.rank() }
"#,
    );
    assert_eq!(checked.diagnostics.len(), 1, "{:?}", checked.diagnostics);
    let diagnostic = &checked.diagnostics[0];
    assert_eq!(diagnostic.summary, "ambiguous trait method `rank`");
    assert_eq!(diagnostic.related.len(), 2, "{diagnostic:?}");
    for (related, value) in diagnostic.related.iter().zip([1, 2]) {
        assert!(
            related
                .message
                .contains(&format!("const arguments: {value}")),
            "{related:?}"
        );
    }
}

#[test]
fn selects_most_specific_extension_method_target() {
    let checked = pipeline(
        r#"
extend[T] T {
    fun rank(self) i32 {
        1
    }
}

extend i32 {
    fun rank(self) i32 {
        2
    }
}

extend[T] &T {
    fun ptr_rank(self) i32 {
        3
    }
}

extend &i32 {
    fun ptr_rank(self) i32 {
        4
    }
}

fun main(value: &i32) i32 {
    1.rank() + value.ptr_rank()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn selects_fixed_array_length_over_const_generic_extension() {
    let checked = pipeline(
        r#"
extend[T, N: usize] [T; N] {
    fun rank(&self) i32 {
        1
    }
}

extend[T] [T; 2] {
    fun rank(&self) i32 {
        2
    }
}

fun main(values: &[i32; 2]) i32 {
    values.rank()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_extension_type_from_array_layout_builtin_operand() {
    let checked = pipeline(
        r#"
extend[T] [u8; std::builtin::size[T]()] where T: Sized {
    fun marker(&self) usize { 1usize }
}

fun main(value: &[u8; std::builtin::size[i32]()]) usize {
    value.marker()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn selects_concrete_array_layout_operand_over_generic_extension() {
    let checked = pipeline(
        r#"
extend[T] [u8; std::builtin::size[T]()] where T: Sized {
    fun rank(&self) i32 { 1 }
}

extend [u8; std::builtin::size[i32]()] {
    fun rank(&self) i32 { 2 }
}

fun main(value: &[u8; std::builtin::size[i32]()]) i32 {
    value.rank()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn selects_repeated_type_parameter_over_independent_parameters() {
    let checked = pipeline(
        r#"
struct Pair[A, B] {
    left: A,
    right: B,
}

extend[A, B] Pair[A, B] {
    fun rank(self) i32 {
        1
    }
}

extend[T] Pair[T, T] {
    fun rank(self) i32 {
        2
    }
}

fun main(value: Pair[i32, i32]) i32 {
    value.rank()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn method_overload_selection_uses_argument_types() {
    let checked = pipeline(
        r#"
struct Box {}

extend Box {
    fun pick[T](self, value: T) T {
        value
    }
}

extend Box {
    fun pick(self, value: i32) i32 {
        value
    }
}

fun main(box: Box) bool {
    box.pick(true)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_let_bound_closure_from_method_callable_parameter() {
    let checked = pipeline(
        r#"
struct Box { value: i32 }

extend Box {
    fun apply(self, callback: &Fun(i32) i32) i32 {
        callback(self.value)
    }
}

fun main() i32 {
    let increment = \value -> value + 1;
    Box { value: 1 }.apply(&increment)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_let_bound_closure_from_generic_method_callable_parameter() {
    let checked = pipeline(
        r#"
struct Box {}

extend Box {
    fun apply[T](self, callback: &Fun(T) T, value: T) T {
        callback(value)
    }
}

fun main() i32 {
    let identity = \value -> value;
    Box {}.apply(&identity, 2)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn closure_inference_uses_ordinary_iter_method_before_builtin_fallback() {
    let checked = pipeline(
        r#"
struct Collection {}
struct Cursor {}

extend Collection {
    fun iter(self) Cursor { Cursor {} }
}

extend Cursor {
    fun apply[T](self, callback: &Fun(T) T, value: T) T {
        callback(value)
    }
}

fun main() i32 {
    let increment = \value -> value + 1;
    Collection {}.iter().apply(&increment, 2)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn method_argument_matching_traverses_all_trait_object_associated_bindings() {
    let checked = pipeline(
        r#"
trait Left {
    type Item;
}

trait Right {
    type Item;
}

trait Pair : Left + Right {}

struct Box {}

extend Box {
    fun pick[T, U](
        self,
        other: &Pair[[Self as Left]::Item = T, [Self as Right]::Item = U],
    ) i32 {
        1
    }
}

fun main(box: Box, value: &Pair[
    [Self as Left]::Item = i32,
    [Self as Right]::Item = bool,
]) i32 {
    box.pick(value)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn method_closure_inference_preserves_mutable_callable_borrow() {
    let checked = pipeline(
        r#"
struct Box {}

extend Box {
    fun apply[T](self, callback: &mut Fun(T) T, value: T) T {
        callback(value)
    }
}

fun main(box: Box) bool {
    box.apply(&\value: i32 -> { value }, true)
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
fn invalid_method_overload_call_reports_no_match() {
    let checked = pipeline(
        r#"
struct Box {}

extend Box {
    fun pick(self, value: i32) i32 {
        value
    }
}

extend Box {
    fun pick(self, value: usize) usize {
        value
    }
}

fun main(box: Box) bool {
    box.pick(true)
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("no matching method overload `pick`")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().all(|diagnostic| !diagnostic
            .summary
            .contains("ambiguous method")
            && !diagnostic.summary.contains("unknown struct field")),
        "{:?}",
        checked.diagnostics
    );
    let diagnostic = checked
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic
                .summary
                .contains("no matching method overload `pick`")
        })
        .expect("method overload diagnostic");
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| note.contains("receiver has type `Box`"))
    );
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| note.contains("candidate methods"))
    );
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| { note.contains("pick(i32)") && note.contains("pick(usize)") })
    );
    assert!(
        diagnostic
            .help
            .iter()
            .any(|help| { help.contains("change the receiver or argument types") })
    );
}

#[test]
fn self_parameter_specializes_generic_method_parameter() {
    let checked = pipeline(
        r#"
struct Box {}

extend Box {
    fun merge[T](self, other: T) bool {
        true
    }
}

extend Box {
    fun merge(self, other: Self) i32 {
        1
    }
}

fun main(box: Box, other: Box) i32 {
    let result = box.merge(other);
    result
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn trait_method_selection_uses_argument_types() {
    let checked = pipeline(
        r#"
trait PickBool {
    fun pick(self, value: bool) bool;
}

trait PickInt {
    fun pick(self, value: i32) bool;
}

struct Box {}

extend Box : PickBool {
    fun pick(self, value: bool) bool {
        value
    }
}

extend Box : PickInt {
    fun pick(self, value: i32) bool {
        value != 0
    }
}

fun main(box: Box) bool {
    box.pick(true)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn trait_method_callable_bound_contextualizes_closure_arguments() {
    let checked = pipeline(
        r#"
trait Apply {
    fun apply[Callback](&self, callback: Callback) i32
    where Callback: Fun(i32) i32;
}

struct Worker {}

extend Worker : Apply {
    fun apply[Callback](&self, callback: Callback) i32
    where Callback: Fun(i32) i32
    {
        callback(4)
    }
}

fun main() i32 {
    let worker = Worker {};
    worker.apply(\value -> value + 3)
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn attributes_provider_demand_to_calling_function() {
    let checked = pipeline_without_visible_extensions(
        r#"
struct Value {}

fun main(value: Value) i32 {
    value.missing()
}
"#,
    );
    let owned_provider_demands = checked
        .provider_demands_by_function
        .values()
        .flat_map(|demands| demands.iter().cloned())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(&owned_provider_demands, checked.provider_demands.as_ref());
    assert_eq!(checked.provider_demands_by_function.len(), 1);
    assert_eq!(checked.diagnostic_owners.len(), checked.diagnostics.len());
    assert!(checked.diagnostic_owners.iter().all(Option::is_some));
}

#[test]
fn error_receivers_do_not_emit_provider_demands() {
    for receiver in ["missing", "&missing", "(missing, 0)", "[missing]"] {
        for method in ["absent", "absent[i32]"] {
            let checked = pipeline_without_visible_extensions(&format!(
                "fun need(value: i32) i32 {{ value }}\nfun main() () {{ ({receiver}).{method}(need(true)); }}"
            ));
            assert_eq!(
                checked.diagnostics.len(),
                2,
                "{receiver}: {:?}",
                checked.diagnostics
            );
            assert!(
                checked.diagnostics.iter().all(|diagnostic| diagnostic
                    .summary
                    .contains("unknown value `missing`")
                    || diagnostic.summary.contains("expected i32, got bool")),
                "{receiver}: {:?}",
                checked.diagnostics
            );
            assert!(
                checked
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.summary.contains("unknown value `missing`")),
                "{:?}",
                checked.diagnostics
            );
            assert!(
                checked
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.summary.contains("expected i32, got bool")),
                "{:?}",
                checked.diagnostics
            );
            assert!(
                checked.provider_demands.is_empty(),
                "{receiver}: {:?}",
                checked.provider_demands
            );
        }
    }
}

#[test]
fn method_candidate_expected_context_does_not_reject_nested_calls() {
    let checked = pipeline(
        r#"
struct Buffer {
    data: &[i32],
}

extend Buffer {
    fun as_slice(&self) &[i32] {
        self.data
    }
}

fun main(buffer: Buffer) i32 {
    buffer.as_slice()[0]
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn ref_method_receiver_lowering_preserves_pointer_typed_self_lhs() {
    let checked = pipeline(
        r#"
struct Counter {
    value: i32,
}

extend Counter {
    fun bump(&mut self) () {
        self.value += 1;
    }

    fun outer(&mut self) () {
        self.bump();
    }
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let receiver = checked
        .ir
        .function_bodies
        .values()
        .filter_map(|body| match body.stmts.as_slice() {
            [
                nia_body_ir::TypedStmt {
                    kind:
                        nia_body_ir::TypedStmtKind::Expr(nia_body_ir::TypedExpr {
                            kind: nia_body_ir::TypedExprKind::Call { callee, .. },
                            ..
                        }),
                    ..
                },
            ] => match callee {
                nia_body_ir::TypedCallee::Method {
                    receiver_kind,
                    receiver,
                    ..
                } if *receiver_kind == ReceiverKind::Ref => Some(receiver),
                _ => None,
            },
            _ => None,
        })
        .next()
        .expect("outer self.bump() receiver");
    assert!(
        matches!(
            checked.type_store.get(receiver.ty),
            Some(TyKind::Pointer {
                is_readonly: false,
                ..
            })
        ),
        "expected ref method receiver typed as pointer, got {:?}",
        checked.type_store.get(receiver.ty)
    );
}

#[test]
fn mutable_receiver_uses_mutable_pointee_without_reassigning_pointer_binding() {
    let checked = pipeline(
        r#"
struct Counter {
    value: i32,
}

extend Counter {
    fun bump(&mut self) () {
        self.value += 1;
    }
}

fun main() i32 {
    let mut counter = Counter { value: 0 };
    let pointer = &mut counter;
    pointer.bump();
    counter.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn mutable_slice_can_call_readonly_pointee_extension_method() {
    let checked = pipeline(
        r#"
extend[T] [T]
where T: Sized
{
    fun inspect(&self) bool {
        true
    }
}

fun main(values: &mut [i32]) bool {
    values.inspect()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn pointer_arrays_can_call_slice_extension_methods() {
    let checked = pipeline_with_len_provider(
        r#"
trait SliceMetric {
    fun metric(&self) i32;
}

extend[T] [T]
where T: Sized
{
    fun itemCount(&self) usize {
        self.len()
    }

    fun replaceFirst(&mut self, value: T) () {
        self[0] = value;
    }

    fun choose[U](&self, value: U) U {
        value
    }

    fun storageKind(&self) i32 {
        1
    }
}

extend[T] [T] : SliceMetric
where T: Sized
{
    fun metric(&self) i32 {
        self.len() as i32
    }
}

extend[T] [T; 2]
where T: Sized
{
    fun storageKind(&self) i32 {
        2
    }
}

fun main() i32 {
    let literalCount = (&"nia").itemCount();
    let choices: [i32; 2] = [1, 2];
    let selected = (&choices).choose[i32](7);
    let metric = (&choices).metric();
    let storageKind = (&choices).storageKind();
    let mut values: [i32; 2] = [3, 4];
    (&mut values).replaceFirst(9);
    literalCount as i32 + selected + metric + storageKind + values[0]
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert_eq!(pointer_array_to_slice_adjustment_count(&checked.facts), 4);
}

#[test]
fn associated_function_and_field_access_use_extension_owner_type() {
    let checked = pipeline(
        r#"
struct CStr {
    ptr: &u8,
}

extend CStr {
    fun from_ptr(ptr: &u8) CStr {
        Self { ptr }
    }

    fun from_bytes(bytes: &[u8]) ?CStr {
        ?CStr::from_ptr(bytes.ptr())
    }

    fun raw_ptr(&self) &u8 {
        self.ptr
    }
}

fun main(bytes: &[u8]) ?&u8 {
    ?CStr::from_bytes(bytes).?.raw_ptr()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn reports_ambiguous_extension_method_specializations() {
    let checked = pipeline(
        r#"
struct Pair[A, B] {
    a: A,
    b: B,
}

extend[T] Pair[T, i32] {
    fun rank(self) i32 {
        1
    }
}

extend[U] Pair[i32, U] {
    fun rank(self) i32 {
        2
    }
}

fun main(pair: Pair[i32, i32]) i32 {
    pair.rank()
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("ambiguous method `rank`")),
        "{:?}",
        checked.diagnostics
    );
    let diagnostic = checked
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.summary.contains("ambiguous method `rank`"))
        .expect("ambiguous method diagnostic");
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| note.contains("equally specific"))
    );
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| note.contains("candidate methods"))
    );
    assert!(diagnostic.help.iter().any(|help| {
        help.contains("more specific") || help.contains("explicit method/type qualification")
    }));
    assert!(
        checked
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.summary.contains("unknown struct field `rank`")),
        "method ambiguity must not fall back to a missing-field diagnostic: {:?}",
        checked.diagnostics
    );
    assert!(
        checked.facts.iter_node_resolved_calls().next().is_none(),
        "failed probes must not publish resolved calls"
    );
    assert!(
        checked.provider_demands.is_empty(),
        "failed probes must not publish provider demands: {:?}",
        checked.provider_demands
    );
}

#[test]
fn checks_assignment_targets_and_let_bindings() {
    let checked = pipeline(
        r#"
static global_let: i32 = 1;
static mut global_mut: i32 = 0;

struct Cell {
    value: i32,
}

fun main(param: i32, read: & i32, write: &mut i32, cell: Cell, read_cell: & Cell, write_cell: &mut Cell) i32 {
    let local_let = 1;
    let mut local_mut = 1;
    local_mut = 2;
    param = 3;
    _ += 1;
    global_mut = 4;
    local_let = 5;
    global_let = 6;
    read.* = 7;
    write.* = 8;
    cell.value = 9;
    read_cell.value = 10;
    write_cell.value = 11;
    0
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("local is let"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("static is immutable"))
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("pointer is read-only"))
            .count(),
        1
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("local_mut"))
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("`_` discard only supports plain assignment")
    }));
}

#[test]
fn checks_method_calls_and_receiver_matching() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fun get(& self) T {
        self.value
    }

    fun set(&mut self, value: T) {
        self.value = value;
    }
}

fun main(ro: & Box[i32], rw: &mut Box[i32]) i32 {
    let mut box = Box[i32] { value: 1 };
    let mut x: i32 = box.get();
    let mut y: i32 = ro.get();
    rw.set(2);
    ro.set(3);
    box.set(true);
    box.get(1);
    x + y
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("receiver cannot be matched through read-only `&T`")
                || diagnostic
                    .summary
                    .contains("receiver is not assignable: local is let")
                || diagnostic.summary.contains(
                    "type mismatch in receiver argument: expected Box[i32], got &Box[i32]",
                )
        }),
        "{:?}",
        checked.diagnostics
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
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("binding initializer"))
    );
}

#[test]
fn builtin_trait_method_failures_explain_the_missing_obligation() {
    let checked = pipeline(
        r#"
fun main(flag: bool) bool {
    flag.add(flag)
}
"#,
    );
    let diagnostic = checked
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.summary == "trait bound not satisfied: bool: Add[bool]")
        .expect("builtin trait method obligation diagnostic");
    assert!(diagnostic.primary_span().is_some_and(|span| span.start > 0));
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| { note.contains("visible implementation") && note.contains("Add[bool]") })
    );
    assert!(diagnostic.help.iter().any(|help| {
        help.contains("provide an implementation") && help.contains("matching implementation")
    }));
}

#[test]
fn lowers_current_module_extension_self_fields_without_visible_extension_seed() {
    let checked = pipeline_without_visible_extensions(
        r#"
struct Init {
    argc: usize,
    argv: &&u8,
}

extend Init {
    pub fun argc(&self) usize {
        self.argc
    }

    pub fun argv(&self) &&u8 {
        self.argv
    }
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(
        checked
            .ir
            .function_bodies
            .values()
            .all(|body| !typed_body_has_error_expr(body)),
        "{:?}",
        checked.ir.function_bodies
    );
}

#[test]
fn resolves_associated_functions_through_type_aliases() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

type Alias[T] = Box[T];

extend[T] Box[T] {
    fun init(value: T) Box[T] {
        Self { value }
    }
}

fun main() i32 {
    let box = Alias[i32]::init(42);
    box.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

fn typed_body_has_error_expr(body: &nia_body_ir::TypedBody) -> bool {
    body.stmts.iter().any(typed_stmt_has_error_expr)
        || body.tail.as_deref().is_some_and(typed_expr_has_error_expr)
}

fn typed_stmt_has_error_expr(stmt: &nia_body_ir::TypedStmt) -> bool {
    match &stmt.kind {
        nia_body_ir::TypedStmtKind::Binding(binding) => binding
            .value
            .as_ref()
            .is_some_and(typed_expr_has_error_expr),
        nia_body_ir::TypedStmtKind::PatternBinding(binding) => {
            typed_expr_has_error_expr(&binding.value)
        }
        nia_body_ir::TypedStmtKind::Expr(expr)
        | nia_body_ir::TypedStmtKind::Defer(expr)
        | nia_body_ir::TypedStmtKind::Return(Some(expr)) => typed_expr_has_error_expr(expr),
        nia_body_ir::TypedStmtKind::ForIn(for_in) => {
            typed_expr_has_error_expr(&for_in.iter) || typed_body_has_error_expr(&for_in.body)
        }
        nia_body_ir::TypedStmtKind::While(while_stmt) => {
            typed_expr_has_error_expr(&while_stmt.cond)
                || typed_body_has_error_expr(&while_stmt.body)
        }
        nia_body_ir::TypedStmtKind::Loop(loop_stmt) => typed_body_has_error_expr(&loop_stmt.body),
        nia_body_ir::TypedStmtKind::Return(None)
        | nia_body_ir::TypedStmtKind::Break
        | nia_body_ir::TypedStmtKind::Continue => false,
    }
}

fn typed_expr_has_error_expr(expr: &nia_body_ir::TypedExpr) -> bool {
    match &expr.kind {
        nia_body_ir::TypedExprKind::Error => true,
        nia_body_ir::TypedExprKind::Unary { expr, .. }
        | nia_body_ir::TypedExprKind::OptionalSome { expr }
        | nia_body_ir::TypedExprKind::ErrorOk { expr }
        | nia_body_ir::TypedExprKind::ErrorErr { expr }
        | nia_body_ir::TypedExprKind::Try { expr, .. }
        | nia_body_ir::TypedExprKind::Discard(expr)
        | nia_body_ir::TypedExprKind::Cast { expr, .. } => typed_expr_has_error_expr(expr),
        nia_body_ir::TypedExprKind::Binary { lhs, rhs, .. } => {
            typed_expr_has_error_expr(lhs) || typed_expr_has_error_expr(rhs)
        }
        nia_body_ir::TypedExprKind::Call { args, .. } => args.iter().any(typed_expr_has_error_expr),
        nia_body_ir::TypedExprKind::Field { lhs, .. } => typed_expr_has_error_expr(lhs),
        nia_body_ir::TypedExprKind::Block(body) => typed_body_has_error_expr(body),
        _ => false,
    }
}

#[test]
fn accepts_local_binding_declarations() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
    y: i32,
}

extend Point {
    fun inspect(& self) i32 { self.x }
    fun init(&self) {}
    fun deinit(&self) {}
}

fun main() {
    let mut p: Point;
    p.init();
    defer p.deinit();
    let origin: Point;
    _ = origin.inspect();
    let n: i32;
    let mut copied: i32 = n;
    let mut borrowed: & i32 = & n;
    _ = copied;
    _ = borrowed;
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn rejects_mutating_let_uninitialized_bindings() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
}

extend Point {
    fun init(&mut self) {}
}

fun main() {
    let origin: Point;
    origin.init();
    let n: i32;
    n = 1;
    _ = &n;
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic.summary.contains("receiver is not assignable")
                || diagnostic
                    .summary
                    .contains("reference target is not assignable")
                || diagnostic
                    .summary
                    .contains("assignment target is not assignable: local is let")
        }),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("local is let")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_explicit_generic_method_calls() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fun replace[U](& self, value: U) U {
        value
    }

    fun get(& self) T {
        self.value
    }
}

fun main(flag: bool) i32 {
    let mut box = Box[i32] { value: 1 };
    let mut x: i32 = box.replace[i32](2);
    let mut y: bool = box.replace[bool](flag);
    let mut z: i32 = box.get();
    _ = box.replace[i32](flag);
    _ = box.replace();
    _ = box.get[i32]();
    x + z
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
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .summary
                .contains("generic argument count mismatch for method"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
    let generic_count = checked
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic
                .summary
                .contains("generic argument count mismatch for method")
        })
        .expect("method generic argument count diagnostic");
    assert!(
        generic_count
            .primary_span()
            .is_some_and(|span| span.start > 0),
        "method generic argument diagnostics must point at the call, not Span::default(): {generic_count:?}"
    );
    assert!(generic_count.notes.iter().any(|note| {
        note.contains("method declares 0 generic parameter(s)") && note.contains("call supplies 1")
    }));
    assert!(
        generic_count
            .help
            .iter()
            .any(|help| help.contains("remove the extra type or const arguments"))
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .summary
                .contains("cannot infer generic parameter `U`"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn preserves_const_generic_method_arguments_in_resolved_calls() {
    let checked = pipeline(
        r#"
struct Counter {}

extend Counter {
    fun value[N: usize](& self) usize {
        N
    }
}

fun main() usize {
    let counter = Counter {};
    counter.value[3]()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let method = checked
        .facts
        .iter_node_resolved_calls()
        .find_map(|(_, call)| match call {
            nia_sema_ir::ResolvedCall::Method { const_args, .. } if !const_args.is_empty() => {
                Some(const_args.clone())
            }
            _ => None,
        })
        .expect("const-generic method call");
    assert_eq!(method.len(), 1);
    assert_eq!(
        method[0].value,
        nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(3))
    );
}

#[test]
fn infers_const_generic_method_arguments_from_array_values() {
    let checked = pipeline(
        r#"
struct Buffer {}

extend Buffer {
    fun take[N: usize](&self, value: [i32; N]) usize {
        N
    }
}

fun main() usize {
    let buffer = Buffer {};
    buffer.take([1, 2, 3])
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_method_generics_through_implicit_pointer_adjustments() {
    let checked = pipeline(
        r#"
struct Reader {}

extend Reader {
    fun read[T](&self, value: &T) T where T: Sized { value.* }
    fun write[T](&self, value: &mut T) T where T: Sized { value.* }
}

fun main() i32 {
    let mut value = 7;
    Reader {}.read(value) + Reader {}.write(value)
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn method_const_inference_shapes_later_callable_arguments() {
    let checked = pipeline(
        r#"
struct Runner {}

extend Runner {
    fun apply[N: usize, Output, Mapper](self, value: [i32; N], callback: Mapper) Output
    where Mapper: Fun([i32; N]) Output
    {
        callback(value)
    }
}

fun main() i32 {
    Runner {}.apply([1, 2, 3], \value -> value[0])
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn preserves_const_generic_extension_target_in_function_pointers() {
    let checked = pipeline(
        r#"
struct Buffer[N: usize] {
    value: i32,
}

extend[N: usize] Buffer[N] {
    fun rank(& self) i32 {
        self.value
    }
}

fun apply(buffer: & Buffer[3], callback: &fun(& Buffer[3]) i32) i32 {
    callback(buffer)
}

fun main() i32 {
    let buffer = Buffer[3] { value: 42 };
    apply(& buffer, & Buffer[3]::rank)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_method_generics_from_expected_return_type() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

struct EmptyBox[T] {}

extend[T] Box[T] {
    fun replace[U](& self, value: U) U {
        value
    }

    fun make[U](value: U) U {
        value
    }

}

extend[T] EmptyBox[T] {
    fun empty() EmptyBox[T] {}
}

fun main() i32 {
    let mut box = Box[i32] { value: 1 };
    let mut a: usize = box.replace(1);
    let mut b: usize = Box[i32]::make(1);
    let mut c: EmptyBox[i32] = EmptyBox::empty();
    _ = c;
    a as i32 + b as i32
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_trait_method_impl_generics_from_method_arguments() {
    let checked = pipeline(
        r#"
trait Writer {}

trait Hash[H]
where H: Writer
{
    fun hash(&self, writer: &mut H) ();
}

struct H {}

extend H {
    fun init() H {
        {}
    }
}

extend H : Writer {}

extend[H] u32 : Hash[H]
where H: Writer
{
    fun hash(&self, writer: &mut H) () {
        _ = self;
        _ = writer;
    }
}

fun main() () {
    let mut hasher = H::init();
    (7u32).hash(&mut hasher);
}
"#,
    );
    assert!(
        checked.diagnostics.iter().all(|diagnostic| !diagnostic
            .summary
            .contains("conflicting inferred type")
            && !diagnostic
                .summary
                .contains("type mismatch in call argument")
            && !diagnostic
                .summary
                .contains("cannot infer generic parameter")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_function_pointer_calls() {
    let checked = pipeline(
        r#"
fun main(cb: &fun(i32, bool) i64, variadic: &fun(i32, ...) (), flag: bool) i64 {
    let mut x: i64 = cb(1, flag);
    _ = cb(flag, flag);
    _ = cb(1);
    variadic(flag, 1);
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
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("call argument"))
            .count(),
        2
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("argument count mismatch"))
            .count(),
        1
    );
}

#[test]
fn checks_associated_method_function_pointers() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
}

extend Point {
    fun new(x: i32) Point {
        Self { x }
    }

    fun get(& self) i32 {
        self.x
    }

    fun set(&self, value: i32) {
        self.x = value;
    }
}

fun main() i32 {
    let make: &fun(i32) Point = & Point::new;
    let get: &fun(& Point) i32 = & Point::get;
    let set: &fun(&Point, i32) () = & Point::set;
    let mut p = make(1);
    set(&p, 2);
    get(& p)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_generic_associated_method_function_pointers() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fun make(value: T) Box[T] {
        Self { value }
    }

    fun replace[U](& self, value: U) U {
        value
    }
}

fun main(flag: bool) i32 {
    let make: &fun(i32) Box[i32] = & Box[i32]::make;
    let replace: &fun(& Box[i32], bool) bool = & Box[i32]::replace[bool];
    let mut b = make(1);
    if replace(& b, flag) { b.value } else { 0 }
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn ambiguous_associated_function_pointer_keeps_only_candidate_diagnostic() {
    let checked = pipeline(
        r#"
struct Pair[A, B] {
    a: A,
    b: B,
}

extend[T] Pair[T, i32] {
    fun rank() i32 { 1 }
}

extend[U] Pair[i32, U] {
    fun rank() i32 { 2 }
}

fun main() () {
    let rank: &fun() i32 = &Pair[i32, i32]::rank;
    _ = rank;
}
"#,
    );
    assert_eq!(
        checked.diagnostics.len(),
        1,
        "an ambiguous function reference must not fall back to ordinary value checking: {:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics[0]
            .summary
            .contains("ambiguous method `rank`"),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn automatically_addresses_array_method_receivers() {
    let checked = pipeline_with_len_provider(
        r#"
extend[T] [T]
where T: Sized
{
    fun count(&self) usize { self.len() }

    fun replaceFirst(&mut self, value: T) () {
        self[0] = value;
    }
}

fun main() usize {
    let mut values: [i32; 2] = [1, 2];
    values.replaceFirst(9);
    values.count()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_structural_associated_calls_and_function_pointers() {
    let checked = pipeline(
        r#"
extend[T] &T {
    fun is_null(self) bool {
        self as usize == 0
    }

    fun zero() usize {
        0usize
    }

    fun marker(self) i32 {
        3
    }
}

extend[T] [T; 3] {
    fun first(self) T {
        self[0]
    }
}

fun main(ptr: &u8, triple: [i32; 3], value: i32) i32 {
    let is_null: &fun(&u8) bool = & [&u8]::is_null;
    let zero: &fun() usize = & [&u8]::zero;
    if is_null(ptr) {}
    if [&u8]::is_null(ptr) {}
    [[i32; 3]]::first(triple) + zero() as i32 + value.marker()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_deep_pointer_structural_associated_calls_and_function_pointers() {
    let checked = pipeline(
        r#"
extend &&&&&& &&i32 {
    fun is_null(self) bool {
        self as usize == 0
    }
}

fun main(ptr: &&&&&& &&i32) bool {
    let is_null: &fun(&&&&&& &&i32) bool = & [&&&&&& &&i32]::is_null;
    is_null(ptr) and [&&&&&& &&i32]::is_null(ptr)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_associated_method_function_pointer_errors() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fun make(value: T) Box[T] {
        Self { value }
    }

    fun replace[U](& self, value: U) U {
        value
    }
}

fun main() {
    let make: &fun(i32) Box[i32] = & Box::make;
    let bad_replace: &fun(& Box[i32], bool) bool = & Box[i32]::replace;
}
"#,
    );
    assert!(
        checked.diagnostics.iter().all(|diagnostic| !diagnostic
            .summary
            .contains("generic argument count mismatch for `Box`")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("generic argument count mismatch for function pointer")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_associated_function_calls() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
}

extend Point {
    fun new(x: i32) Point {
        Self { x }
    }

    fun get(& self) i32 {
        self.x
    }
}

fun main(flag: bool) i32 {
    let mut p = Point::new(1);
    let mut value: i32 = Point::get(&p);
    _ = Point::new(flag);
    _ = Point::new();
    _ = Point::get();
    p::get();
    value
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
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("argument count mismatch"))
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("receiver method `get` requires")
    }));
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("qualified access is not a value expression")
    }));
}

#[test]
fn explains_unknown_associated_function_target_and_visibility() {
    let checked = pipeline(
        r#"
struct Point {}

fun main() () {
    Point::missing()
}
"#,
    );
    let diagnostic = checked
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.summary == "unknown associated function `missing`")
        .expect("unknown associated function diagnostic");
    assert!(diagnostic.notes.iter().any(|note| {
        note.contains("target type is `Point`") && note.contains("no visible associated function")
    }));
    assert!(diagnostic.help.iter().any(|help| {
        help.contains("check the type qualification") && help.contains("import the module")
    }));
}

#[test]
fn checks_unqualified_associated_function_calls_inside_extension_methods() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
}

extend Point {
    fun helper() i32 {
        1
    }

    fun value(&self) i32 {
        helper()
    }
}

fun main() i32 {
    let point = Point { x: 0 };
    point.value()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_unqualified_associated_function_calls_inside_generic_extension_methods() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fun wrap(value: T) Box[T] {
        Self { value }
    }

    fun copy(&self) Box[T] {
        wrap(self.value)
    }
}

fun main() i32 {
    let boxed = Box[i32] { value: 7 };
    boxed.copy().value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn module_function_takes_precedence_over_extension_helper_in_method_body() {
    let checked = pipeline(
        r#"
struct S {}

fun helper() i32 {
    10
}

extend S {
    fun helper() i32 {
        1
    }

    fun method(&self) i32 {
        helper()
    }
}

fun main() i32 {
    let value = S {};
    value.method()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn local_callable_takes_precedence_over_extension_helper_in_method_body() {
    let checked = pipeline(
        r#"
struct S {}

fun chosen() i32 {
    10
}

extend S {
    fun helper() i32 {
        1
    }

    fun method(&self) i32 {
        let helper = &chosen;
        helper()
    }
}

fun main() i32 {
    let value = S {};
    value.method()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_generic_type_prefix_associated_function_calls() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fun make(value: T) Box[T] {
        Self { value }
    }

    fun empty() Box[T] {
        Self { value: 0 }
    }
}

fun main(flag: bool) i32 {
    let mut a: Box[i32] = Box[i32]::make(1);
    _ = Box[i32]::make(flag);
    _ = Box[i32, bool]::make(1);
    _ = Box::empty();
    a.value
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
            .contains("generic argument count mismatch for `Box`")
    }));
}

#[test]
fn infers_const_generic_nominal_prefix_from_expected_return() {
    let checked = pipeline(
        r#"
struct Box[T, N: usize] {
    value: T,
}

extend[T, N: usize] Box[T, N] {
    fun make(value: T) Box[T, N] {
        Self { value }
    }
}

fun main() i32 {
    let value: Box[i32, 3] = Box::make(42);
    value.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_lowercase_generic_type_prefix_associated_function_calls() {
    let checked = pipeline(
        r#"
struct box[T] {
    value: T,
}

extend[T] box[T] {
    fun make(value: T) box[T] {
        Self { value }
    }
}

fun main() i32 {
    let mut a: box[i32] = box[i32]::make(1);
    a.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn receiver_method_calls_take_precedence_over_function_pointer_fields() {
    let checked = pipeline(
        r#"
type RunFn = &fun() i32;

struct MethodBox {
    run: RunFn,
}

struct FieldBox {
    run: RunFn,
}

fun field_run() i32 {
    1
}

extend MethodBox {
    fun run(self) i32 {
        _ = self;
        2
    }
}

fun main() i32 {
    let method_box = MethodBox { run: &field_run };
    let field_box = FieldBox { run: &field_run };
    method_box.run() + field_box.run()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let mut saw_method = false;
    let mut saw_function_pointer = false;
    for (_, call) in checked.facts.iter_node_resolved_calls() {
        match call {
            nia_sema_ir::ResolvedCall::Method { .. } => saw_method = true,
            nia_sema_ir::ResolvedCall::FunctionPointer => saw_function_pointer = true,
            _ => {}
        }
    }
    assert!(
        saw_method,
        "{:?}",
        checked.facts.iter_node_resolved_calls().collect::<Vec<_>>()
    );
    assert!(
        saw_function_pointer,
        "{:?}",
        checked.facts.iter_node_resolved_calls().collect::<Vec<_>>()
    );
}

#[test]
fn propagates_expected_type_through_contextual_receiver_chain() {
    let checked = pipeline(
        r#"
struct Foo {
    value: i32,
}

extend Foo {
    fun init() Foo {
        Foo { value: 1 }
    }

    fun withFoo(self) Foo {
        self
    }
}

fun main() i32 {
    let foo: Foo = .init().withFoo();
    foo.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_cross_type_receiver_from_method_result() {
    let checked = pipeline(
        r#"
struct Builder {
    value: i32,
}

struct Product {
    value: i32,
}

extend Builder {
    fun init() Builder {
        Builder { value: 2 }
    }

    fun finish(self) Product {
        Product { value: self.value }
    }
}

fun main() i32 {
    let product: Product = .init().finish();
    product.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn propagates_expected_type_through_generic_middle_call() {
    let checked = pipeline(
        r#"
struct Source {}
struct Collected[T] {}
struct Product[T] {}

extend Source {
    fun init() Source {}

    fun collect[T](self) Collected[T] {
        _ = self;
        {}
    }
}

extend[T] Collected[T] {
    fun finish(self) Product[T] {
        _ = self;
        {}
    }
}

fun main() () {
    let product: Product[i32] = .init().collect().finish();
    _ = product;
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn reports_ambiguous_contextual_receiver_solutions() {
    let checked = pipeline(
        r#"
struct Left {}
struct Right {}
struct Product {}

extend Left {
    fun init() Left {}
    fun finish(self) Product {}
}

extend Right {
    fun init() Right {}
    fun finish(self) Product {}
}

fun main() () {
    let product: Product = .init().finish();
    _ = product;
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("ambiguous method `finish` while inferring receiver type")
        }),
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("while inferring receiver type"))
            .count(),
        1,
        "contextual ambiguity should produce one primary diagnostic: {:?}",
        checked.diagnostics
    );
}

#[test]
fn infers_generic_contextual_receiver_target() {
    let checked = pipeline(
        r#"
struct Builder[T] {
    value: T,
}

struct Product[T] {
    value: T,
}

extend[T] Builder[T] {
    fun init(value: T) Builder[T] {
        Builder[T] { value }
    }

    fun finish(self) Product[T] {
        Product[T] { value: self.value }
    }
}

fun main() i32 {
    let product: Product[i32] = .init(4).finish();
    product.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_const_generic_contextual_receiver_target() {
    let checked = pipeline(
        r#"
struct Builder[N: usize] {}
struct Product[N: usize] {}

extend[N: usize] Builder[N] {
    fun init() Builder[N] {}
    fun finish(self) Product[N] {}
}

fun main() () {
    let product: Product[3] = .init().finish();
    _ = product;
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn propagates_context_through_explicit_method_generics() {
    let checked = pipeline(
        r#"
struct Source {}
struct Middle[T] { value: T }
struct Product[T] { value: T }

extend Source {
    fun init() Source {}

    fun collect[T](self, value: T) Middle[T] {
        _ = self;
        Middle[T] { value }
    }
}

extend[T] Middle[T] {
    fun finish(self) Product[T] {
        Product[T] { value: self.value }
    }
}

fun main() i32 {
    let product: Product[i32] = .init().collect[i32](5).finish();
    product.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn propagates_context_through_long_cross_type_chain() {
    let checked = pipeline(
        r#"
struct Start {}
struct First {}
struct Second {}
struct Product {}

extend Start {
    fun init() Start {}
    fun first(self) First {}
}

extend First {
    fun second(self) Second {}
}

extend Second {
    fun finish(self) Product {}
}

fun main() () {
    let product: Product = .init().first().second().finish();
    _ = product;
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn contextual_receiver_search_rejects_argument_mismatches() {
    let checked = pipeline(
        r#"
struct Left {}
struct Right {}
struct Product {}

extend Left {
    fun init() Left {}
    fun finish(self, value: i32) Product { _ = value; {} }
}

extend Right {
    fun init() Right {}
    fun finish(self, value: bool) Product { _ = value; {} }
}

fun main() () {
    let product: Product = .init().finish(true);
    _ = product;
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn contextual_receiver_inference_requires_expected_type() {
    let checked = pipeline(
        r#"
struct Builder {}
struct Product {}

extend Builder {
    fun init() Builder {}
    fun finish(self) Product {}
}

fun main() () {
    let product = .init().finish();
    _ = product;
}
"#,
    );
    assert!(
        !checked.diagnostics.is_empty(),
        "context-free chain must be rejected"
    );
}

#[test]
fn infers_contextual_receiver_for_trait_method() {
    let checked = pipeline(
        r#"
trait Finish {
    fun finish(self) Product {
        _ = self;
        {}
    }
}

struct Builder {}
struct Product {}

extend Builder {
    fun init() Builder {}
}

extend Builder : Finish {}

fun main() () {
    let product: Product = .init().finish();
    _ = product;
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn known_local_receiver_does_not_trigger_contextual_ambiguity() {
    let checked = pipeline(
        r#"
struct Left {}
struct Right {}
struct Product {}

extend Left {
    fun init() Left {}
    fun finish(self) Product {}
}

extend Right {
    fun finish(self) Product {}
}

fun main() () {
    let builder: Left = .init();
    let product: Product = builder.finish();
    _ = product;
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_generic_trait_impl_contextual_receiver() {
    let checked = pipeline(
        r#"
trait Finish[T] {
    fun finish(self) Product[T];
}

struct Builder[T] { value: T }
struct Product[T] { value: T }

extend[T] Builder[T] {
    fun init(value: T) Builder[T] {
        Builder[T] { value }
    }
}

extend[T] Builder[T] : Finish[T] {
    fun finish(self) Product[T] {
        Product[T] { value: self.value }
    }
}

fun main() i32 {
    let product: Product[i32] = .init(6).finish();
    product.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn propagates_context_through_explicit_trait_method_generics() {
    let checked = pipeline(
        r#"
trait Finish {
    fun finish[T](self, value: T) Product[T] {
        _ = self;
        Product[T] { value }
    }
}

struct Builder {}
struct Product[T] { value: T }

extend Builder {
    fun init() Builder {}
}

extend Builder : Finish {}

fun main() i32 {
    let product: Product[i32] = .init().finish[i32](7);
    product.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn contextual_receiver_keeps_normal_overload_specificity() {
    let checked = pipeline(
        r#"
struct Builder {}
struct Product {}

extend Builder {
    fun init() Builder {}

    fun finish[T](self, value: T) Product {
        _ = value;
        {}
    }
}

extend Builder {
    fun finish(self, value: bool) Product {
        _ = value;
        {}
    }
}

fun main() () {
    let product: Product = .init().finish(true);
    _ = product;
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn contextual_receiver_does_not_infer_through_result_coercion() {
    let checked = pipeline(
        r#"
struct Builder {}
struct Product {}

static mut product: Product = {};

extend Builder {
    fun init() Builder {}

    fun finish(self) &mut Product {
        _ = self;
        &mut product
    }
}

fun main() () {
    let value: &Product = .init().finish();
    _ = value;
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("omitted member requires a call or enum expected type")),
        "result coercions must not drive reverse receiver inference: {:?}",
        checked.diagnostics
    );
}
