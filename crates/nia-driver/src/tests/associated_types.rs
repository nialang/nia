// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

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
    let mut counter = Counter { value: 3 };
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
fn trait_goal_args_normalize_associated_type_projections() {
    let root = temp_dir("trait_goal_args_normalize_associated_type_projections");
    write(
        &root.join("main.nia"),
        r#"
trait Source {
    type Item;
}

trait Back[Item] {
    fn back(& self) Item;
}

struct Box[T] {
    value: T,
}

extend[T] Box[T] : Source {
    type Item = T;
}

extend[T] Box[T] : Back[T] {
    fn back(& self) T {
        self.value
    }
}

fn read[B](value: & B) [B as Source]::Item
where B: Source + Back[[B as Source]::Item]
{
    value.back()
}

fn main() i32 {
    let box = Box[i32] { value: 7 };
    read[Box[i32]](&box)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn subtrait_methods_can_return_inherited_associated_type_projection() {
    let root = temp_dir("subtrait_methods_can_return_inherited_associated_type_projection");
    write(
        &root.join("main.nia"),
        r#"
trait Source {
    type Item;

    fn next(&mut self) ?[Self as Source]::Item;
}

trait Back : Source {
    fn next_back(&mut self) ?[Self as Source]::Item;
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    type Item = i32;

    fn next(&mut self) ?i32 {
        ?self.value
    }
}

extend Counter : Back {
    fn next_back(&mut self) ?i32 {
        ?self.value
    }
}

fn read_back[B](value: &mut B) ?[B as Source]::Item
where B: Back
{
    value.next_back()
}

fn main() i32 {
    let mut counter = Counter { value: 7 };
    switch read_back[Counter](&mut counter) {
        ?value => {
            value
        },
        null => {
            0
        },
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_subtrait_methods_can_return_inherited_associated_type_projection() {
    let root = temp_dir("generic_subtrait_methods_can_return_inherited_associated_type_projection");
    write(
        &root.join("main.nia"),
        r#"
trait Source {
    type Item;
}

trait Back : Source {
    fn next_back(&mut self) ?[Self as Source]::Item;
}

struct Box[T] {
    value: T,
}

extend[T] Box[T] : Source {
    type Item = T;
}

extend[T] Box[T] : Back {
    fn next_back(&mut self) ?T {
        ?self.value
    }
}

fn main() i32 {
    let mut value = Box[i32] { value: 7 };
    switch value.next_back() {
        ?item => {
            item
        },
        null => {
            0
        },
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn source_subtrait_methods_can_return_builtin_associated_type_projection() {
    let root = temp_dir("source_subtrait_methods_can_return_builtin_associated_type_projection");
    write(
        &root.join("main.nia"),
        r#"
trait Back : Iterator {
    fn next_back(&mut self) ?[Self as Iterator]::Item;
}

struct Box[T] {
    value: T,
}

extend[T] Box[T] : Iterator {
    type Item = T;

    fn next(&mut self) ?T {
        ?self.value
    }
}

extend[T] Box[T] : Back {
    fn next_back(&mut self) ?T {
        self.next()
    }
}

fn main() i32 {
    let mut value = Box[i32] { value: 7 };
    switch value.next_back() {
        ?item => {
            item
        },
        null => {
            0
        },
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_wrapper_impl_can_return_where_bound_builtin_associated_projection() {
    let root =
        temp_dir("generic_wrapper_impl_can_return_where_bound_builtin_associated_projection");
    write(
        &root.join("main.nia"),
        r#"
trait Back : Iterator {
    fn next_back(&mut self) ?[Self as Iterator]::Item;
}

struct Rev[I]
where I: Back
{
    iter: I,
}

extend[I] Rev[I] : Iterator
where I: Back
{
    type Item = [I as Iterator]::Item;

    fn next(&mut self) ?[I as Iterator]::Item {
        self.iter.next_back()
    }
}

extend[I] Rev[I] : Back
where I: Back
{
    fn next_back(&mut self) ?[I as Iterator]::Item {
        self.iter.next()
    }
}

fn main() i32 { 0 }
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
    let mut source = Source {};
    let mut limit = Limit[Source] { reader: &source };
    switch limit.read() {
        !n => {
            n as i32
        },
        error! => {
            1
        },
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
module io;
using entry::io;

fn error_of[W](value: [W as io::Writer]::Error) ()
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
module io;
using entry::io;

struct Box[T] {
    value: T,
}

fn boxed_error[W](value: [W as io::Writer]::Error) Box[[W as io::Writer]::Error]
where W: io::Writer
{
    Box[[W as io::Writer]::Error] { value }
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
module io;
using entry::io;

struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn init(value: T) Box[T] {
        Self { value }
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
fn trait_impls_require_associated_const_definitions() {
    let root = temp_dir("trait_impls_require_associated_const_definitions");
    write(
        &root.join("main.nia"),
        r#"
trait Simd {
    type Lane;
    const Lanes: usize;
}

struct Vec4 {}

extend Vec4 : Simd {
    type Lane = u8;
    const Extra: usize = 4usize;
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("missing definition for associated const `Lanes`")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("associated const `Extra` is not a member")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn trait_impls_accept_associated_const_definitions() {
    let root = temp_dir("trait_impls_accept_associated_const_definitions");
    write(
        &root.join("main.nia"),
        r#"
trait Simd {
    type Lane;
    const Lanes: usize;
}

struct Vec4 {}

extend Vec4 : Simd {
    type Lane = u8;
    const Lanes: usize = 4usize;
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
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
    let mut p = Pairer { seed: 3 };
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
        Number { value: self.value + rhs.value }
    }
}

fn add_same[T](a: & T, b: T) T
where T: Combines[T, Output = T] {
    a.add(b)
}

fn main() i32 {
    let mut one = Number { value: 1 };
    let mut two = Number { value: 2 };
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
    let mut p = Pairer { seed: 3 };
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
    let mut device = Device {};
    switch device.read() {
        !value => {
            value
        },
        error! => {
            0
        },
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn trait_associated_const_projection_checks_as_value() {
    let root = temp_dir("trait_associated_const_projection_checks_as_value");
    write(
        &root.join("main.nia"),
        r#"
trait Simd {
    const Lanes: usize;
}

struct Vec4 {}

extend Vec4 : Simd {
    const Lanes: usize = 4usize;
}

fn lanes[T]() usize
where T: Simd
{
    [T as Simd]::Lanes
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
fn trait_impl_associated_const_type_must_match_requirement() {
    let root = temp_dir("trait_impl_associated_const_type_must_match_requirement");
    write(
        &root.join("main.nia"),
        r#"
trait Simd {
    const Lanes: usize;
}

struct Vec4 {}

extend Vec4 : Simd {
    const Lanes: bool = true;
}

fn main() i32 {
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("does not match the trait requirement")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn imported_trait_associated_const_projection_checks_as_value() {
    let root = temp_dir("imported_trait_associated_const_projection_checks_as_value");
    write(
        &root.join("simd.nia"),
        r#"
pub trait Simd {
    const Lanes: usize;
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module simd;
using entry::simd;

struct Vec4 {}

extend Vec4 : simd::Simd {
    const Lanes: usize = 4usize;
}

fn lanes[T]() usize
where T: simd::Simd
{
    [T as simd::Simd]::Lanes
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
fn trait_associated_const_projection_drives_array_lengths() {
    let root = temp_dir("trait_associated_const_projection_drives_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
trait Shape {
    const N: usize;
}

struct Four {}

extend Four : Shape {
    const N: usize = 4usize;
}

fn fourth(values: [[Four as Shape]::N]u8) u8 {
    values[3]
}

fn main() i32 {
    fourth([1u8, 2u8, 3u8, 4u8]) as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_impl_associated_const_projection_uses_instance_const_args() {
    let root = temp_dir("generic_impl_associated_const_projection_uses_instance_const_args");
    write(
        &root.join("main.nia"),
        r#"
trait HasLen {
    const Len: usize;
}

struct Buf[N: usize] {
    values: [N]u8,
}

extend[N: usize] Buf[N] : HasLen {
    const Len: usize = N;
}

const fn len[T]() usize
where T: HasLen
{
    [T as HasLen]::Len
}

const A: usize = len[Buf[4]]();
const B: usize = len[Buf[8]]();

fn main() usize {
    A * 10usize + B
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn supertrait_associated_const_projection_resolves_through_subtrait_bound() {
    let root = temp_dir("supertrait_associated_const_projection_resolves_through_subtrait_bound");
    write(
        &root.join("main.nia"),
        r#"
trait Base {
    const N: usize;
}

trait Sub : Base {}

struct Value {}

extend Value : Base {
    const N: usize = 6usize;
}

extend Value : Sub {}

const fn n[T]() usize
where T: Sub
{
    [T as Base]::N
}

const WIDTH: usize = n[Value]();

fn main() usize {
    let values: [WIDTH]usize = [1, 2, 3, 4, 5, 6];
    values[5]
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn trait_const_generic_associated_const_projection_substitutes_const_args() {
    let root = temp_dir("trait_const_generic_associated_const_projection_substitutes_const_args");
    write(
        &root.join("main.nia"),
        r#"
trait Slot[N: usize] {
    const Width: usize;
}

struct Store {}

extend Store : Slot[3] {
    const Width: usize = 3usize;
}

const fn width_store() usize
{
    [Store as Slot[3]]::Width
}

fn generic_width[T, N: usize]() usize
where T: Slot[N]
{
    [T as Slot[N]]::Width
}

const WIDTH: usize = width_store();

fn main() usize {
    let values: [WIDTH]usize = [1, 2, 3];
    values[2] + generic_width[Store, 3]()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_simd_trait_projects_lane_type() {
    let root = temp_dir("builtin_simd_trait_projects_lane_type");
    write(
        &root.join("main.nia"),
        r#"
fn concrete(value: [u8x16 as Simd]::Lane) u8 {
    value
}

fn lanes() usize {
    [u8x16 as Simd]::Lanes
}

fn lane_array(values: [[u8x16 as Simd]::Lanes]u8) u8 {
    values[15]
}

fn generic[V](value: [V as Simd]::Lane) ()
where V: Simd
{
    _ = value;
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
fn builtin_simd_mask_implies_simd_lane_projection() {
    let root = temp_dir("builtin_simd_mask_implies_simd_lane_projection");
    write(
        &root.join("main.nia"),
        r#"
fn generic[V](value: [V as Simd]::Lane) ()
where V: SimdMask
{
    _ = value;
}

fn main() i32 {
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
