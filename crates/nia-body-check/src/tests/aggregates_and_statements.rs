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
    var bad: Pair = { left: true, left: 1, extra: 1 };
    var inferred: Pair = { left: 1, right: false };
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
fn checks_struct_field_access() {
    let checked = pipeline(
        r#"
struct Pair[T] {
    left: T,
    right: bool,
}

fn main(pair: Pair[i32], ptr: & Pair[i32]) i32 {
    var x: i32 = pair.left;
    var y: bool = ptr.right;
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
    var p = Point { x: 1, y: 2 };
    var p_ptr = &(Point { x: 3, y: 4 });
    var literal_ptr = &10i32;
    var call_ptr = &make();
    var slice = &([_]i32[1, 2, 3])[..];
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
    _ = printf(c"ok\n");
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
            .filter(|diagnostic| diagnostic.summary.contains("non-void expression result"))
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
    var i = 0;
    while flag {
        _ = i;
        break;
    }

    while i {
        break;
    }

    var iter = Counter { current: 0, end: 3 };
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
            .all(|diagnostic| !diagnostic.summary.contains("non-void expression result")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn rejects_for_in_non_iterator_ranges() {
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
            .filter(|diagnostic| diagnostic.summary.contains("for-in expects an Iterator"))
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
    var iter = Counter { current: 0, end: 3 };
    var total = 0;
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
    var total = 0usize;
    var iter = Counter { current: 0usize, end: len };
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
    var iter = Once { value: value, done: false };
    for &n in iter {
        _ = n;
    }
    0
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
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
    var iter = Counter {};
    for &n in iter {
        _ = n;
    }
    0
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("for pattern requires iterator item")),
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
        switch value() {
            0 => cleanup(),
            _ => cleanup(),
        }
    };
    defer {
        switch value() {
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
                .contains("`defer` expression must have type `void`"))
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

let vtable: Vtable = { print: & print_i32 };

fn main() i32 {
    var x = 1;
    vtable.print(&x);
    0
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}
