// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use crate::check_program;

#[test]
fn associated_types_resolve_explicit_projection_in_trait_methods() {
    let root = temp_dir("associated_types_resolve_explicit_projection_in_trait_methods");
    write(
        &root.join("main.nia"),
        r#"
trait Source {
    type Item;

    fn get(& self) [Self as Source]::Item;
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    type Item = i32;

    fn get(& self) i32 {
        self.value
    }
}

fn read[T](value: & T) [T as Source]::Item
where T: Source {
    value.get()
}

fn main() i32 {
    var counter: Counter = { value: 3 };
    read[Counter](& counter)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn concrete_associated_type_projection_is_well_formed_from_impl() {
    let root = temp_dir("concrete_associated_type_projection_is_well_formed_from_impl");
    write(
        &root.join("main.nia"),
        r#"
trait Source {
    type Item;
}

struct Counter {}

extend Counter : Source {
    type Item = i32;
}

fn id(value: [Counter as Source]::Item) [Counter as Source]::Item {
    value
}

fn main() i32 {
    id(7)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn associated_type_shorthand_normalizes_nested_generic_wrapper_calls() {
    let root = temp_dir("associated_type_shorthand_normalizes_nested_generic_wrapper_calls");
    write(
        &root.join("main.nia"),
        r#"
trait Reader {
    type Error;

    fn read(& self) Error!usize;
}

enum IoError: i32 {
    Bad = 1,
    _
}

struct Source {}

extend Source : Reader {
    type Error = IoError;

    fn read(& self) Error!usize {
        !1
    }
}

struct Limit[R] {
    reader: & R,
}

extend[R] Limit[R] : Reader
where R: Reader
{
    type Error = [R as Reader]::Error;

    fn read(& self) Error!usize {
        self.reader.read()
    }
}

fn main() i32 {
    var source: Source = {};
    var limit: Limit[Source] = { reader: &source };
    switch limit.read() {
        !n => n as i32,
        error! => 1,
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn qualified_trait_projection_uses_trait_bound_context() {
    let root = temp_dir("qualified_trait_projection_uses_trait_bound_context");
    write(
        &root.join("io.nia"),
        r#"
pub trait Writer {
    type Error;
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .io;

fn error_of[W](value: [W as io::Writer]::Error) void
where W: io::Writer
{
    _ = value;
}

fn main() i32 {
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        !program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("trait types are not valid as values")),
        "{:?}",
        program.diagnostics
    );
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn qualified_trait_projection_can_be_generic_argument() {
    let root = temp_dir("qualified_trait_projection_can_be_generic_argument");
    write(
        &root.join("io.nia"),
        r#"
pub trait Writer {
    type Error;
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .io;

struct Box[T] {
    value: T,
}

fn boxed_error[W](value: [W as io::Writer]::Error) Box[[W as io::Writer]::Error]
where W: io::Writer
{
    { value: value }
}

fn main() i32 {
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn projected_generic_argument_type_prefix_associated_call_lowers_cleanly() {
    let root = temp_dir("projected_generic_argument_type_prefix_associated_call_lowers_cleanly");
    write(
        &root.join("io.nia"),
        r#"
pub trait Writer {
    type Error;
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .io;

struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn init(value: T) Box[T] {
        { value: value }
    }
}

fn boxed_error[W](value: [W as io::Writer]::Error) Box[[W as io::Writer]::Error]
where W: io::Writer
{
    Box[[W as io::Writer]::Error]::init(value)
}

fn main() i32 {
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn trait_impls_require_associated_type_definitions() {
    let root = temp_dir("trait_impls_require_associated_type_definitions");
    write(
        &root.join("main.nia"),
        r#"
trait Pair {
    type A;
    type B;
}

struct Point {
    x: i32,
}

extend Point : Pair {
    type A = i32;
    type Extra = i32;
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("missing definition for associated type `B`")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("associated type `Extra` is not a member")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn associated_type_definitions_are_restricted_to_trait_impls() {
    let root = temp_dir("associated_type_definitions_are_restricted_to_trait_impls");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: i32,
}

extend Point {
    type Item = i32;

    fn get(& self) i32 {
        self.x
    }
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("associated type definitions are only allowed in trait implementations")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn duplicate_associated_type_members_are_diagnosed() {
    let root = temp_dir("duplicate_associated_type_members_are_diagnosed");
    write(
        &root.join("main.nia"),
        r#"
trait Pair {
    type Item;
    type Item;
}

struct Point {
    x: i32,
}

extend Point : Pair {
    type Item = i32;
    type Item = i64;
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("duplicate trait associated type")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("duplicate associated type definition")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn projection_trait_must_be_a_trait() {
    let root = temp_dir("projection_trait_must_be_a_trait");
    write(
        &root.join("main.nia"),
        r#"
struct NotTrait {
    value: i32,
}

fn bad[T](value: T) [T as NotTrait]::Item {
    value
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("projection trait must resolve to a trait")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn projection_associated_type_must_exist_on_trait() {
    let root = temp_dir("projection_associated_type_must_exist_on_trait");
    write(
        &root.join("main.nia"),
        r#"
trait Source {
    type Item;
}

fn bad[T](value: T) [T as Source]::Missing
where T: Source {
    value
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("trait does not define associated type `Missing`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn impl_method_signature_checks_associated_type_projection() {
    let root = temp_dir("impl_method_signature_checks_associated_type_projection");
    write(
        &root.join("main.nia"),
        r#"
trait Source {
    type Item;

    fn get(& self) [Self as Source]::Item;
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    type Item = i32;

    fn get(& self) bool {
        true
    }
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains(
                "implementation of trait method `get` does not match the trait signature"
            )),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn generic_trait_associated_types_support_multiple_outputs_and_defaults() {
    let root = temp_dir("generic_trait_associated_types_support_multiple_outputs_and_defaults");
    write(
        &root.join("main.nia"),
        r#"
trait Mapper[A, B] {
    type C;
    type D;

    fn map_c(& self, a: A, b: B) [Self as Mapper[A, B]]::C;
    fn map_d(& self, a: A, b: B, fallback: [Self as Mapper[A, B]]::D) [Self as Mapper[A, B]]::D {
        _ = self.map_c(a, b);
        fallback
    }
}

struct Pairer {
    seed: i32,
}

extend Pairer : Mapper[i32, i32] {
    type C = i32;
    type D = i32;

    fn map_c(& self, a: i32, b: i32) i32 {
        self.seed + a + b
    }
}

fn mapped[T](value: & T, fallback: [T as Mapper[i32, i32]]::D) [T as Mapper[i32, i32]]::D
where T: Mapper[i32, i32] {
    value.map_d(1, 2, fallback)
}

fn main() i32 {
    var p: Pairer = { seed: 3 };
    mapped[Pairer](& p, 9)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn associated_type_bindings_normalize_open_projections() {
    let root = temp_dir("associated_type_bindings_normalize_open_projections");
    write(
        &root.join("main.nia"),
        r#"
trait Combines[Rhs] {
    type Output;

    fn add(& self, rhs: Rhs) [Self as Combines[Rhs]]::Output;
}

struct Number {
    value: i32,
}

extend Number : Combines[Number] {
    type Output = Number;

    fn add(& self, rhs: Number) Number {
        { value: self.value + rhs.value }
    }
}

fn add_same[T](a: & T, b: T) T
where T: Combines[T, Output = T] {
    a.add(b)
}

fn main() i32 {
    var one: Number = { value: 1 };
    var two: Number = { value: 2 };
    add_same[Number](& one, two).value
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn associated_type_bound_bindings_are_not_positional_args() {
    let root = temp_dir("associated_type_bound_bindings_are_not_positional_args");
    write(
        &root.join("main.nia"),
        r#"
trait Combines[Rhs] {
    type Output;
}

fn id[T](value: T) T
where T: Combines[T, Output = T] {
    value
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        !program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("generic argument count mismatch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn associated_type_bindings_do_not_forge_unbound_projection_equality() {
    let root = temp_dir("associated_type_bindings_do_not_forge_unbound_projection_equality");
    write(
        &root.join("main.nia"),
        r#"
trait Combines[Rhs] {
    type Output;

    fn add(& self, rhs: Rhs) [Self as Combines[Rhs]]::Output;
}

fn bad[T](a: & T, b: T) T
where T: Combines[T] {
    a.add(b)
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("type mismatch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn associated_type_bindings_are_validated() {
    let root = temp_dir("associated_type_bindings_are_validated");
    write(
        &root.join("main.nia"),
        r#"
trait Combines[Rhs] {
    type Output;
}

struct NotTrait {}

fn unknown[T](value: T) T
where T: Combines[T, Missing = T] {
    value
}

fn duplicate[T](value: T) T
where T: Combines[T, Output = T, Output = i32] {
    value
}

fn non_trait[T](value: T) T
where T: NotTrait[Output = T] {
    value
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("trait does not define associated type `Missing`")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("duplicate associated type binding `Output`")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("associated type bindings require a trait bound")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn associated_type_bindings_handle_multiple_outputs_and_generics() {
    let root = temp_dir("associated_type_bindings_handle_multiple_outputs_and_generics");
    write(
        &root.join("main.nia"),
        r#"
trait Mapper[A, B] {
    type C;
    type D;

    fn map_c(& self, a: A, b: B) [Self as Mapper[A, B]]::C;
    fn map_d(& self, a: A, b: B) [Self as Mapper[A, B]]::D;
}

struct Pairer {
    seed: i32,
}

extend Pairer : Mapper[i32, bool] {
    type C = i32;
    type D = bool;

    fn map_c(& self, a: i32, b: bool) i32 {
        if b { self.seed + a } else { self.seed }
    }

    fn map_d(& self, a: i32, b: bool) bool {
        if b { a > 0 } else { false }
    }
}

fn map_c_i32[T](value: & T) i32
where T: Mapper[i32, bool, C = i32, D = bool] {
    value.map_c(2, true)
}

fn map_d_bool[T](value: & T) bool
where T: Mapper[i32, bool, C = i32, D = bool] {
    value.map_d(2, true)
}

fn main() i32 {
    var p: Pairer = { seed: 3 };
    if map_d_bool[Pairer](& p) { map_c_i32[Pairer](& p) } else { 0 }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn associated_type_resolution_uses_current_impl_identity_not_target_type() {
    let root = temp_dir("associated_type_resolution_uses_current_impl_identity_not_target_type");
    write(
        &root.join("main.nia"),
        r#"
trait Reader {
    type Error;

    fn read(& self) Error!i32;
}

trait Writer {
    type Error;

    fn write(& self) Error!i32;
}

enum ReadError: i32 {
    Bad = 1,
    _
}

enum WriteError: i32 {
    Bad = 2,
    _
}

struct Device {}

extend Device : Writer {
    type Error = WriteError;

    fn write(& self) Error!i32 {
        !2
    }
}

extend Device : Reader {
    type Error = ReadError;

    fn read(& self) Error!i32 {
        !1
    }
}

fn main() i32 {
    var device: Device = {};
    switch device.read() {
        !value => value,
        error! => 0,
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
