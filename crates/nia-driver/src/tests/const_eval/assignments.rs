// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn const_function_rejects_assignment_to_immutable_local() {
    let root = temp_dir("const_function_rejects_assignment_to_immutable_local");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    let i: usize = 2;
    i = 5;
    i
}

const n: usize = width();

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cannot assign to immutable const local `i`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_assignment_value_type_mismatch() {
    let root = temp_dir("const_function_rejects_assignment_value_type_mismatch");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    let mut i: usize = 2;
    i = true;
    i
}

const n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("primitive type usize")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_int_assignment_to_bool() {
    let root = temp_dir("const_function_rejects_int_assignment_to_bool");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    let mut value: bool = true;
    value = 1usize;
    1usize
}

const n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("primitive type bool")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_array_assignment_shape_mismatch() {
    let root = temp_dir("const_function_rejects_array_assignment_shape_mismatch");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    let mut values: [2]usize = [1usize, 2usize];
    values = true;
    values.len()
}

const n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("expected array type")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_array_assignment_length_mismatch() {
    let root = temp_dir("const_function_rejects_array_assignment_length_mismatch");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    let mut values: [2]usize = [1usize, 2usize];
    values = [1usize; 3usize];
    values.len()
}

const n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("expected length 2")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_optional_assignment_shape_mismatch() {
    let root = temp_dir("const_function_rejects_optional_assignment_shape_mismatch");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    let mut value: ?usize = ?1usize;
    value = true;
    1usize
}

const n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("expected optional type")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_error_union_assignment_shape_mismatch() {
    let root = temp_dir("const_function_rejects_error_union_assignment_shape_mismatch");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    let mut value: usize!usize = !1usize;
    value = true;
    1usize
}

const n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("expected error union type")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_struct_assignment_missing_field() {
    let root = temp_dir("const_function_rejects_struct_assignment_missing_field");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    let mut config = {width: 4usize, enabled: true};
    config = {width: 8usize};
    config.width
}

const n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("const struct value is missing field `enabled`")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_nominal_struct_assignment_missing_field() {
    let root = temp_dir("const_function_rejects_nominal_struct_assignment_missing_field");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

const fn width() usize {
    let mut p: Point = Point{x: 1usize, y: 2usize};
    p = Point{x: 3usize};
    p.x
}

const n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("const struct value is missing field `y`")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_struct_assignment_extra_field() {
    let root = temp_dir("const_function_rejects_struct_assignment_extra_field");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    let mut config = {width: 4usize};
    config = {width: 8usize, enabled: true};
    config.width
}

const n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("const struct value has extra field `enabled`")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_nominal_struct_assignment_extra_field() {
    let root = temp_dir("const_function_rejects_nominal_struct_assignment_extra_field");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

const fn width() usize {
    let mut p: Point = Point{x: 1usize, y: 2usize};
    p = Point{x: 3usize, y: 4usize, z: 5usize};
    p.x
}

const n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("const struct value has extra field `z`")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_mutates_struct_fields() {
    let root = temp_dir("const_function_mutates_struct_fields");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

const fn width() usize {
    let mut p: Point = Point{x: 2, y: 3};
    p.x += 4;
    p.y = p.x + p.y;
    p.y
}

const n: usize = width();

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_mutates_array_indexes() {
    let root = temp_dir("const_function_mutates_array_indexes");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    let mut values: [4]usize = [1, 2, 3, 4];
    let mut i: usize = 0;
    while i < 4 {
        let value = i;
        values[i] += value;
        i += 1;
    }
    values[0] + values[1] + values[2] + values[3]
}

const n: usize = width();

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_compound_assignment_evaluates_index_once() {
    let root = temp_dir("const_compound_assignment_evaluates_index_once");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    let mut values: [2]usize = [4, 9];
    let mut calls = 0usize;
    values[{
        calls += 1;
        0usize
    }] += 1;
    values[0] * 100 + values[1] * 10 + calls
}

const n: usize = width();

fn main() i32 {
    let values: [591]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_for_in_rejects_range_iter_method() {
    let root = temp_dir("const_for_in_rejects_range_iter_method");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    let mut total: usize = 0;
    for value in (0usize..4usize).iter() {
        total += value;
    }
    total
}

const n: usize = width();

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const expression can only call `const fn`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_mutates_nested_aggregate_paths() {
    let root = temp_dir("const_function_mutates_nested_aggregate_paths");
    write(
        &root.join("main.nia"),
        r#"
struct Pair {
    values: [2]usize,
}

struct Config {
    pairs: [2]Pair,
}

const fn width() usize {
    let mut config: Config = Config{
        pairs: [
            Pair{values: [1, 2]},
            Pair{values: [3, 4]},
        ],
    };
    config.pairs[1].values[0] = 8;
    config.pairs[0].values[1] += config.pairs[1].values[0];
    config.pairs[0].values[1]
}

const n: usize = width();

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_rejects_field_assignment_to_immutable_root() {
    let root = temp_dir("const_function_rejects_field_assignment_to_immutable_root");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
}

const fn width() usize {
    let p: Point = Point{x: 1};
    p.x = 2;
    p.x
}

const n: usize = width();

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cannot assign to immutable const local `p`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_field_assignment_value_type_mismatch() {
    let root = temp_dir("const_function_rejects_field_assignment_value_type_mismatch");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
}

const fn width() usize {
    let mut p: Point = Point{x: 1};
    p.x = true;
    p.x
}

const n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("primitive type usize")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_if_statement_flows_return_and_else_if() {
    let root = temp_dir("const_function_if_statement_flows_return_and_else_if");
    write(
        &root.join("main.nia"),
        r#"
const fn width(bits: usize) usize {
    if bits == 16 {
        return 2;
    } else if bits == 32 {
        return 4;
    }
    return 8;
}

const n: usize = width(32);

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_for_in_arrays_require_iterator() {
    let root = temp_dir("const_function_for_in_arrays_require_iterator");
    write(
        &root.join("main.nia"),
        r#"
const fn width(values: [4]usize) usize {
    let mut total: usize = 0;
    for value in values {
        total += value;
    }
    total
}

const n: usize = width([1, 2, 3, 4]);

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("for-in expects an Iterable")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_for_in_ranges_require_iterator() {
    let root = temp_dir("const_function_for_in_ranges_require_iterator");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    let mut total: usize = 0;
    for value in 0usize..=5usize {
        if value == 2usize {
            continue;
        }
        if value == 5usize {
            break;
        }
        total += value;
    }
    total
}

const n: usize = width();

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("for-in expects an Iterable")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_for_in_ranges_without_iterator() {
    let root = temp_dir("const_function_rejects_for_in_ranges_without_iterator");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    let mut total: usize = 0;
    for value in ..5usize {
        total += value;
    }
    total
}

const n: usize = width();

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("for-in expects an Iterable")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_for_in_executes_iterator_trait() {
    let root = temp_dir("const_function_for_in_executes_iterator_trait");
    write(
        &root.join("main.nia"),
        r#"
struct Counter {
    current: usize,
    end: usize,
}

extend Counter : Iterator {
    type Item = usize;

    const fn next(&mut self) ?usize {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

extend Counter {
    const fn next(&mut self) ?usize {
        null
    }
}

const fn width() usize {
    let mut total: usize = 0;
    let mut iter = Counter{current: 0, end: 4};
    for value in iter {
        if value == 1 {
            continue;
        }
        if value == 3 {
            break;
        }
        total += value;
    }
    total
}

const n: usize = width();

fn main() i32 {
    let values: [2]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_for_in_executes_iterable_trait() {
    let root = temp_dir("const_function_for_in_executes_iterable_trait");
    write(
        &root.join("main.nia"),
        r#"
struct Counter {
    current: usize,
    end: usize,
}

extend Counter : Iterator {
    type Item = usize;

    const fn next(&mut self) ?usize {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

struct Bounds {
    start: usize,
    end: usize,
}

extend Bounds : Iterable {
    type Item = usize;
    type Iter = Counter;

    const fn iter(&self) Counter {
        Counter { current: self.start, end: self.end }
    }
}

const fn width() usize {
    let mut total = 0usize;
    let bounds = Bounds { start: 2, end: 5 };
    let mut direct = bounds.iter();
    switch direct.next() {
        ?value => {
            total += value;
        },
        null => {},
    }
    for value in bounds {
        total += value;
    }
    total
}

const n: usize = width();

fn main() i32 {
    let values: [11]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_generic_const_iterable_executes_trait_witnesses() {
    let root = temp_dir("imported_generic_const_iterable_executes_trait_witnesses");
    write(
        &root.join("main.nia"),
        r#"
module pair;
using entry::pair;

const fn total(values: pair::Pair[usize]) usize {
    let mut result: usize = 0;
    for value in values {
        result += value;
    }
    result
}

const n: usize = total(pair::pair(4, 6));

fn main() i32 {
    let values: [10]i32 = [0; n];
    values.len() as i32
}
"#,
    );
    write(
        &root.join("pair.nia"),
        r#"
pub struct Pair[T] {
    first: T,
    second: T,
}

pub struct PairIter[T] {
    first: T,
    second: T,
    index: usize,
}

extend[T] PairIter[T] : Iterator {
    type Item = T;

    pub const fn next(&mut self) ?T {
        switch self.index {
            0usize => {
                self.index += 1;
                ?self.first
            },
            1usize => {
                self.index += 1;
                ?self.second
            },
            _ => null,
        }
    }
}

extend[T] Pair[T] : Iterable {
    type Item = T;
    type Iter = PairIter[T];

    pub const fn iter(&self) PairIter[T] {
        PairIter[T] { first: self.first, second: self.second, index: 0 }
    }
}

pub const fn pair[T](first: T, second: T) Pair[T] {
    Pair[T] { first: first, second: second }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_const_iterator_witness_requires_visible_extension_module() {
    let root = temp_dir("imported_const_iterator_witness_requires_visible_extension_module");
    write(
        &root.join("main.nia"),
        r#"
module counter;

const fn total(iter: counter::Counter) usize {
    let mut result: usize = 0;
    for value in iter {
        result += value;
    }
    result
}

fn main() i32 { 0 }
"#,
    );
    write(
        &root.join("counter.nia"),
        r#"
pub struct Counter {
    current: usize,
    end: usize,
}

extend Counter : Iterator {
    type Item = usize;

    pub const fn next(&mut self) ?usize {
        null
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("for-in expects an Iterable")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_imported_runtime_only_iterator_witness() {
    let root = temp_dir("unused_const_function_rejects_imported_runtime_only_iterator_witness");
    write(
        &root.join("main.nia"),
        r#"
module counter;
using entry::counter;

const fn total(iter: counter::Counter) usize {
    let mut result: usize = 0;
    for value in iter {
        result += value;
    }
    result
}

const fn first(iter: counter::DirectCounter) usize {
    let mut values = iter;
    switch values.next() {
        ?value => value,
        null => 0,
    }
}

fn main() i32 { 0 }
"#,
    );
    write(
        &root.join("counter.nia"),
        r#"
pub struct Counter {
    current: usize,
    end: usize,
}

pub struct DirectCounter {
    current: usize,
    end: usize,
}

extend Counter : Iterator {
    type Item = usize;

    pub fn next(&mut self) ?usize {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

extend DirectCounter : Iterator {
    type Item = usize;

    pub fn next(&mut self) ?usize {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

extend Counter {
    pub const fn next(&mut self) ?usize {
        null
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains(
                "`Iterator::next` trait witness used by const for-in must be declared `const fn`"
            )),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("`Iterator::next` trait witness used during const evaluation must be declared `const fn`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_imported_runtime_only_iterable_witness() {
    let root = temp_dir("unused_const_function_rejects_imported_runtime_only_iterable_witness");
    write(
        &root.join("main.nia"),
        r#"
module bounds;
using entry::bounds;

const fn total(values: bounds::Bounds) usize {
    let mut result: usize = 0;
    for value in values {
        result += value;
    }
    result
}

const fn iterator(values: bounds::Bounds) bounds::Counter {
    values.iter()
}

fn main() i32 { 0 }
"#,
    );
    write(
        &root.join("bounds.nia"),
        r#"
pub struct Counter {
    current: usize,
    end: usize,
}

extend Counter : Iterator {
    type Item = usize;

    pub const fn next(&mut self) ?usize {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

pub struct Bounds {
    start: usize,
    end: usize,
}

extend Bounds : Iterable {
    type Item = usize;
    type Iter = Counter;

    pub fn iter(&self) Counter {
        Counter { current: self.start, end: self.end }
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains(
                "`Iterable::iter` trait witness used by const for-in must be declared `const fn`"
            )),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("`Iterable::iter` trait witness used during const evaluation must be declared `const fn`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_mutable_receiver_writes_back_nested_place_once() {
    let root = temp_dir("const_mutable_receiver_writes_back_nested_place_once");
    write(
        &root.join("main.nia"),
        r#"
struct Counter {
    value: usize,
}

extend Counter {
    const fn bump(&mut self) usize {
        self.value += 1;
        self.value
    }
}

struct State {
    counters: [2]Counter,
}

const fn width() usize {
    let mut state = State {
        counters: [Counter { value: 2 }, Counter { value: 5 }],
    };
    let mut index = 0usize;
    let bumped = state.counters[{
        index += 1;
        0usize
    }].bump();
    state.counters[0].value * 1000 + state.counters[1].value * 100 + index * 10 + bumped
}

const n: usize = width();

fn main() i32 {
    let values: [3513]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_rejects_escaped_loop_control_flow() {
    let root = temp_dir("const_function_rejects_escaped_loop_control_flow");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    break;
}

const n: usize = width();

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const loop control flow escaped its loop")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_infinite_loop_statements() {
    let root = temp_dir("const_function_rejects_infinite_loop_statements");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    loop {
        continue;
    }
    return 1;
}

const n: usize = width();

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const loop exceeded")),
        "{:?}",
        program.diagnostics
    );
}
