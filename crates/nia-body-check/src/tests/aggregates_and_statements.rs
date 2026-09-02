// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn checks_struct_literal_fields() {
    let checked = pipeline(
        r#"
struct Pair {
    left: i32,
    right: bool,
}

fn main() i32 {
    let mut bad = Pair { left: true, left: 1, extra: 1 };
    let mut inferred = Pair { left: 1, right: false };
    0
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("struct literal field"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("unknown struct field"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("duplicate struct field"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("missing struct field"))
    );
}

#[test]
fn fills_omitted_struct_fields_from_declaration_defaults() {
    let checked = pipeline(
        r#"
struct Config {
    required: i32,
    port: i32 = 8080,
    workers: i32 = { let base = 2; base + 2 },
}

fn main() i32 {
    let explicit = Config { required: 1, port: 9000 };
    let contextual: Config = .{ required: 2 };
    explicit.port + explicit.workers + contextual.port + contextual.workers
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let body = checked
        .check
        .ir
        .function_bodies
        .values()
        .next()
        .expect("main body");
    let struct_field_counts = body
        .stmts
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            nia_body_ir::TypedStmtKind::Binding(binding) => binding.value.as_ref(),
            _ => None,
        })
        .filter_map(|expr| match &expr.kind {
            nia_body_ir::TypedExprKind::StructLiteral { fields, .. } => Some(fields.len()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(struct_field_counts, [3, 3]);
}

#[test]
fn still_rejects_omitted_required_struct_fields() {
    let checked = pipeline(
        r#"
struct Config {
    required: i32,
    optional: i32 = 1,
}

fn main() i32 {
    let bad = Config {};
    0
}
"#,
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic.summary.contains("missing struct field `required`")
    }));
}

#[test]
fn checks_unused_struct_field_defaults_at_the_declaration() {
    let checked = pipeline(
        r#"
struct Bad {
    value: bool = 1,
}

fn main() {}
"#,
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("struct field default value")
    }));
}

#[test]
fn rejects_defaults_outside_ordinary_structs() {
    let checked = pipeline(
        r#"
extern struct External { value: i32 = 1 }
union Storage { value: i32 = 1 }
enum Event { Data { value: i32 = 1 } }

fn main() {}
"#,
    );
    for expected in [
        "extern struct fields cannot have default values",
        "union fields cannot have default values",
        "enum payload fields cannot have default values",
    ] {
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains(expected)),
            "missing `{expected}` in {:?}",
            checked.diagnostics
        );
    }
}

#[test]
fn checks_struct_field_access() {
    let checked = pipeline(
        r#"
struct Pair[T] {
    left: T,
    right: bool,
}

fn main(pair: Pair[i32], ptr: & Pair[i32]) i32 {
    let mut x: i32 = pair.left;
    let mut y: bool = ptr.right;
    _ = pair.missing;
    pair.right
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("unknown struct field"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("function body"))
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("binding initializer"))
    );
}

#[test]
fn accepts_typed_literals_and_rvalue_reference_targets() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
    y: i32,
}

extend Point {
    fn sum(&self) i32 {
        self.x + self.y
    }
}

fn main() i32 {
    let mut p = Point { x: 1, y: 2 };
    let mut p_ptr = &(Point { x: 3, y: 4 });
    let mut literal_ptr = &10i32;
    let mut call_ptr = &make();
    let mut slice = &([1, 2, 3])[..];
    p.sum() + Point { x: 5, y: 6 }.sum() + p_ptr.x + literal_ptr.* + call_ptr.* + slice[0]
}

fn make() i32 {
    7
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn rejects_implicit_discard_of_non_void_expression_statements() {
    let checked = pipeline(
        r#"
fn value() i32 { 1 }
fn effect() {}
extern fn abort() never;
extern fn printf(fmt: & u8, ...);

fn main() i32 {
    value();
    _ = value();
    _ = effect();
    let fmt = b"ok\n\0";
    _ = printf(&fmt[0]);
    effect();
    abort();
    0
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("non-unit expression result"))
            .count(),
        1
    );
}

#[test]
fn checks_new_loop_expression_type_edges() {
    let checked = pipeline(
        r#"
struct Counter {
    current: i32,
    end: i32,
}

extend Counter : Iterator {
    type Item = i32;

    fn next(&mut self) ?i32 {
        null
    }
}

fn main(flag: bool) i32 {
    let mut i = 0;
    while flag {
        _ = i;
        break;
    }

    while i {
        break;
    }

    let mut iter = Counter { current: 0, end: 3 };
    for n in iter {
        _ = n;
    }

    loop {
        _ = i;
        break;
    }

    i
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("while condition"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.summary.contains("non-() expression result")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn rejects_for_in_non_iterable_ranges_without_visible_impls() {
    let checked = pipeline(
        r#"
fn main() i32 {
    for n in 0..3 {
        _ = n;
    }
    for n in ..3 {
        _ = n;
    }
    0
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("for-in expects an Iterable"))
            .count(),
        2,
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn accepts_for_in_iterator_values() {
    let checked = pipeline(
        r#"
struct Counter {
    current: i32,
    end: i32,
}

extend Counter : Iterator {
    type Item = i32;

    fn next(&mut self) ?i32 {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

fn main() i32 {
    let mut iter = Counter { current: 0, end: 3 };
    let mut total = 0;
    for n in iter {
        total += n;
    }
    total
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn iterator_item_type_guides_for_binding_type() {
    let checked = pipeline(
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

fn main(len: usize) usize {
    let mut total = 0usize;
    let mut iter = Counter { current: 0usize, end: len };
    for n in iter {
        total += n;
    }
    total
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn accepts_for_in_pointer_item_pattern() {
    let checked = pipeline(
        r#"
struct Once {
    value: &i32,
    done: bool,
}

extend Once : Iterator {
    type Item = &i32;

    fn next(&mut self) ?&i32 {
        if self.done {
            null
        } else {
            self.done = true;
            ?self.value
        }
    }
}

fn main(value: &i32) i32 {
    let mut iter = Once { value: value, done: false };
    let mut total = 0;
    for &n in iter {
        total += n;
    }
    total
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn borrowed_slice_iterable_providers_guide_for_pointer_patterns() {
    let checked = pipeline(
        r#"
struct ReadIter[T]
where T: Sized
{}

extend[T] ReadIter[T] : Iterator
where T: Sized
{
    type Item = &T;

    fn next(&mut self) ?&T {
        null
    }
}

extend[T] &[T] : Iterable
where T: Sized
{
    type Item = &T;
    type Iter = ReadIter[T];

    fn iter(&self) ReadIter[T] {
        {}
    }
}

extend[T] &mut [T] : Iterable
where T: Sized
{
    type Item = &T;
    type Iter = ReadIter[T];

    fn iter(&self) ReadIter[T] {
        {}
    }
}

fn sum(values: &[i32]) i32 {
    let mut total = 0;
    for &value in values {
        total += value;
    }
    total
}

fn sumMutView(values: &mut [i32]) i32 {
    let mut total = 0;
    for &value in values {
        total += value;
    }
    total
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn accepts_local_pointer_binding_patterns() {
    let checked = pipeline(
        r#"
fn main(ptr: &i32, mut_ptr: &mut i32) i32 {
    let &x = ptr;
    let mut &mut y: &mut i32 = mut_ptr;
    x + y
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn pointer_binding_annotations_describe_the_pattern_input() {
    let checked = pipeline(
        r#"
fn main(pair: &(i32, bool), writable: &mut (i32, i32)) i32 {
    let &(value, enabled): &(i32, bool) = pair;
    let mut &mut (left, right): &mut (i32, i32) = writable;
    let (extra, active): (i32, bool) = (1, true);
    left += right;
    if enabled and active { value + left + extra } else { right }
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn rejects_pointer_binding_annotation_for_the_destructured_value() {
    let checked = pipeline(
        r#"
fn main(ptr: &i32) i32 {
    let &value: i32 = ptr;
    value
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
fn rejects_for_in_pointer_pattern_for_value_items() {
    let checked = pipeline(
        r#"
struct Counter {}

extend Counter : Iterator {
    type Item = i32;

    fn next(&mut self) ?i32 {
        null
    }
}

fn main() i32 {
    let mut iter = Counter {};
    for &n in iter {
        _ = n;
    }
    0
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("for pattern requires value")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_defer_expression_type_edges() {
    let checked = pipeline(
        r#"
fn value() i32 { 1 }
fn cleanup() {}

fn main(flag: bool) {
    defer cleanup();
    defer _ = value();
    defer if flag {
        cleanup();
    } else {
        cleanup();
    };
    defer if flag {
        value()
    } else {
        2
    };
    defer {
        match value() {
            0 => cleanup(),
            _ => cleanup(),
        }
    };
    defer {
        match value() {
            0 => value(),
            _ => value(),
        }
    };
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .summary
                .contains("`defer` expression must have type `()`"))
            .count(),
        2,
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_function_pointer_fields_as_void_calls() {
    let checked = pipeline(
        r#"
struct Vtable {
    print: &fn(&i32)
}

fn print_i32(value: &i32) {}

static vtable: Vtable = Vtable { print: & print_i32 };

fn main() i32 {
    let mut x = 1;
    vtable.print(&x);
    0
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}
