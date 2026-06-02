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
            .any(|diagnostic| diagnostic.message.contains("struct literal field"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unknown struct field"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("duplicate struct field"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("missing struct field"))
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

fn main(pair: Pair[i32], ptr: &const Pair[i32]) i32 {
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
            .any(|diagnostic| diagnostic.message.contains("unknown struct field"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("function body"))
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
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
extern fn abort() !;
extern fn printf(fmt: &const u8, ...);

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
            .filter(|diagnostic| diagnostic.message.contains("non-void expression result"))
            .count(),
        1
    );
}

#[test]
fn checks_new_loop_expression_type_edges() {
    let checked = pipeline(
        r#"
fn main(flag: bool) i32 {
    var i = 0;
    while flag {
        _ = i;
        break;
    }

    while i {
        break;
    }

    for n in 0..3 {
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
            .filter(|diagnostic| diagnostic.message.contains("while condition"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("non-void expression result")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn rejects_for_in_ranges_without_start_bound() {
    let checked = pipeline(
        r#"
fn main() i32 {
    for n in ..3 {
        _ = n;
    }
    for n in ..=3 {
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
            .filter(|diagnostic| diagnostic
                .message
                .contains("for-in range iterator requires a start bound"))
            .count(),
        2,
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn accepts_for_in_range_values() {
    let checked = pipeline(
        r#"
fn main() i32 {
    var r = 0..3;
    for n in r {
        _ = n;
    }
    0
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn accepts_for_in_range_iter_method() {
    let checked = pipeline(
        r#"
fn main() i32 {
    var r = 0..3;
    for n in r.iter() {
        _ = n;
    }
    0
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn rejects_range_iter_method_without_start_bound() {
    let checked = pipeline(
        r#"
fn main() i32 {
    var r = ..3;
    for n in r.iter() {
        _ = n;
    }
    0
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("range.iter() requires a start bound")
        }),
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
                .message
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
    print: &const fn(&i32)
}

fn print_i32(value: &i32) {}

const vtable: Vtable = { print: &const print_i32 };

fn main() i32 {
    var x = 1;
    vtable.print(&x);
    0
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}
