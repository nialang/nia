// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn selects_most_specific_extension_method_target() {
    let checked = pipeline(
        r#"
extend[T] T {
    fn rank(self) i32 {
        1
    }
}

extend i32 {
    fn rank(self) i32 {
        2
    }
}

extend[T] &T {
    fn ptr_rank(self) i32 {
        3
    }
}

extend &i32 {
    fn ptr_rank(self) i32 {
        4
    }
}

fn main(value: &i32) i32 {
    1.rank() + value.ptr_rank()
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
    fn rank(self) i32 {
        1
    }
}

extend[U] Pair[i32, U] {
    fn rank(self) i32 {
        2
    }
}

fn main(pair: Pair[i32, i32]) i32 {
    pair.rank()
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("ambiguous method `rank`")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_assignment_targets_and_const_bindings() {
    let checked = pipeline(
        r#"
const global_const: i32 = 1;
var global_mut: i32 = 0;

struct Cell {
    value: i32,
}

fn main(param: i32, read: &const i32, write: &i32, cell: Cell, read_cell: &const Cell, write_cell: &Cell) i32 {
    const local_const = 1;
    var local_mut = 1;
    local_mut = 2;
    param = 3;
    _ += 1;
    global_mut = 4;
    local_const = 5;
    global_const = 6;
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
            .any(|diagnostic| diagnostic.message.contains("local is const"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("global is const"))
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("pointer is const"))
            .count(),
        1
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("local_mut"))
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
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
    fn get(&const self) T {
        self.value
    }

    fn set(&self, value: T) {
        self.value = value;
    }
}

fn main(ro: &const Box[i32], rw: &Box[i32]) i32 {
    var box: Box[i32] = { value: 1 };
    var x: i32 = box.get();
    var y: i32 = ro.get();
    rw.set(2);
    ro.set(3);
    box.set(true);
    box.get(1);
    x + y
}
"#,
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("receiver cannot be matched through `&const T`")
    }));
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
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
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
    fn inspect(&const self) i32 { self.x }
    fn init(&self) {}
    fn deinit(&self) {}
}

fn main() {
    var p: Point;
    p.init();
    defer p.deinit();
    const origin: Point;
    _ = origin.inspect();
    const n: i32;
    var copied: i32 = n;
    var borrowed: &const i32 = &const n;
    _ = copied;
    _ = borrowed;
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn rejects_mutating_const_uninitialized_bindings() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
}

extend Point {
    fn init(&self) {}
}

fn main() {
    const origin: Point;
    origin.init();
    const n: i32;
    n = 1;
    _ = &n;
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains("receiver is not assignable")
                || diagnostic
                    .message
                    .contains("reference target is not assignable")
        }),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("local is const")),
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
    fn replace[U](&const self, value: U) U {
        value
    }

    fn get(&const self) T {
        self.value
    }
}

fn main(flag: bool) i32 {
    var box: Box[i32] = { value: 1 };
    var x: i32 = box.replace[i32](2);
    var y: bool = box.replace[bool](flag);
    var z: i32 = box.get();
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
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument"))
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .message
                .contains("generic argument count mismatch for method"))
            .count(),
        1
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .message
                .contains("cannot infer generic parameter `U`"))
            .count(),
        1
    );
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
    fn replace[U](&const self, value: U) U {
        value
    }

    fn make[U](value: U) U {
        value
    }

}

extend[T] EmptyBox[T] {
    fn empty() EmptyBox[T] {}
}

fn main() i32 {
    var box: Box[i32] = { value: 1 };
    var a: usize = box.replace(1);
    var b: usize = Box[i32]::make(1);
    var c: EmptyBox[i32] = EmptyBox::empty();
    _ = c;
    a as i32 + b as i32
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_function_pointer_calls() {
    let checked = pipeline(
        r#"
fn main(cb: &const fn(i32, bool) i64, variadic: &const fn(i32, ...) void, flag: bool) i64 {
    var x: i64 = cb(1, flag);
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
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("call argument"))
            .count(),
        2
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("argument count mismatch"))
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
    fn new(x: i32) Point {
        { x: x }
    }

    fn get(&const self) i32 {
        self.x
    }

    fn set(&self, value: i32) {
        self.x = value;
    }
}

fn main() i32 {
    var make: &const fn(i32) Point = &const Point::new;
    var get: &const fn(&const Point) i32 = &const Point::get;
    var set: &const fn(&Point, i32) void = &const Point::set;
    var p = make(1);
    set(&p, 2);
    get(&const p)
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
    fn make(value: T) Box[T] {
        { value: value }
    }

    fn replace[U](&const self, value: U) U {
        value
    }
}

fn main(flag: bool) i32 {
    var make: &const fn(i32) Box[i32] = &const Box[i32]::make;
    var replace: &const fn(&const Box[i32], bool) bool = &const Box[i32]::replace[bool];
    var b = make(1);
    if replace(&const b, flag) { b.value } else { 0 }
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
    fn is_null(self) bool {
        self as usize == 0
    }

    fn zero() usize {
        0usize
    }
}

extend[T] [3]T {
    fn first(self) T {
        self[0]
    }
}

fn main(ptr: &u8, triple: [3]i32) i32 {
    var is_null: &const fn(&u8) bool = &const [&u8]::is_null;
    var zero: &const fn() usize = &const [&u8]::zero;
    if is_null(ptr) {}
    if [&u8]::is_null(ptr) {}
    [[3]i32]::first(triple) + zero() as i32
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_deep_pointer_structural_associated_calls_and_function_pointers() {
    let checked = pipeline(
        r#"
extend &&&&&&const &&i32 {
    fn is_null(self) bool {
        self as usize == 0
    }
}

fn main(ptr: &&&&&&const &&i32) bool {
    var is_null: &const fn(&&&&&&const &&i32) bool = &const [&&&&&&const &&i32]::is_null;
    is_null(ptr) and [&&&&&&const &&i32]::is_null(ptr)
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
    fn make(value: T) Box[T] {
        { value: value }
    }

    fn replace[U](&const self, value: U) U {
        value
    }
}

fn main() {
    var bad_make: &const fn(i32) Box[i32] = &const Box::make;
    var bad_replace: &const fn(&const Box[i32], bool) bool = &const Box[i32]::replace;
    var mutable_ref = &Box[i32]::make;
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("generic function pointer requires explicit type arguments")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("generic argument count mismatch for function pointer")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("function pointers must be formed with `&const`")),
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
    fn new(x: i32) Point {
        { x: x }
    }

    fn get(&const self) i32 {
        self.x
    }
}

fn main(flag: bool) i32 {
    var p = Point::new(1);
    var value: i32 = Point::get(&p);
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
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
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
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("receiver method `get` requires")
    }));
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("qualified access is not a value expression")
    }));
}

#[test]
fn checks_generic_type_prefix_associated_function_calls() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn make(value: T) Box[T] {
        { value: value }
    }

    fn empty() Box[T] {
        { value: 0 }
    }
}

fn main(flag: bool) i32 {
    var a: Box[i32] = Box[i32]::make(1);
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
            .contains("generic argument count mismatch for `Box`")
    }));
}

#[test]
fn checks_lowercase_generic_type_prefix_associated_function_calls() {
    let checked = pipeline(
        r#"
struct box[T] {
    value: T,
}

extend[T] box[T] {
    fn make(value: T) box[T] {
        { value: value }
    }
}

fn main() i32 {
    var a: box[i32] = box[i32]::make(1);
    a.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}
