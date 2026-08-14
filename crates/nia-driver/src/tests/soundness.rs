// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn open_projection_does_not_forge_concrete_type() {
    let root = temp_dir("open_projection_does_not_forge_concrete_type");
    write(
        &root.join("main.nia"),
        r#"
trait TypeIs[T] {
    type Is;
}

struct I32Proof {}

extend I32Proof : TypeIs[i32] {
    type Is = i32;
}

fn rewrite[R, RW](value: [RW as TypeIs[R]]::Is) R
where RW: TypeIs[R] {
    value
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
fn open_projection_does_not_forge_pointer_deref() {
    let root = temp_dir("open_projection_does_not_forge_pointer_deref");
    write(
        &root.join("main.nia"),
        r#"
trait RefLike {
    type Ref;
}

struct I32Ref {}

extend I32Ref : RefLike {
    type Ref = & i32;
}

fn read_open[T](value: [T as RefLike]::Ref) i32
where T: RefLike {
    value.*
}

fn read_concrete(value: [I32Ref as RefLike]::Ref) i32 {
    value.*
}

fn main() i32 {
    let mut value: i32 = 5;
    read_concrete(& value)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("Deref")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn supertrait_bound_does_not_infer_parent_associated_type_equality() {
    let root = temp_dir("supertrait_bound_does_not_infer_parent_associated_type_equality");
    write(
        &root.join("main.nia"),
        r#"
trait TypeIs[T] {
    type Is;
}

trait Lift[T] : TypeIs[T] {}

struct I32Lift {}

extend I32Lift : TypeIs[i32] {
    type Is = i32;
}

extend I32Lift : Lift[i32] {}

fn rewrite[R, RW](value: [RW as TypeIs[R]]::Is) R
where RW: Lift[R] {
    value
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
fn generic_supertrait_arguments_remain_distinct_for_projection() {
    let root = temp_dir("generic_supertrait_arguments_remain_distinct_for_projection");
    write(
        &root.join("main.nia"),
        r#"
trait Source[T] {
    type Item;
}

trait SizedSource[T] : Source[T] {}

struct Source64 {}

extend Source64 : Source[i64] {
    type Item = i64;
}

extend Source64 : SizedSource[i64] {}

fn bad[S](value: [S as Source[i32]]::Item) [S as Source[i32]]::Item
where S: SizedSource[i64] {
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
            .contains("trait bound not satisfied")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn cross_module_associated_type_projection_resolves_impl_definition() {
    let root = temp_dir("cross_module_associated_type_projection_resolves_impl_definition");
    write(
        &root.join("traits.nia"),
        r#"
pub trait Source {
    type Item;

    fn get(& self) [Self as Source]::Item;
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module traits;
using entry::traits;

struct Counter {
    value: i32,
}

extend Counter : traits::Source {
    type Item = i32;

    fn get(& self) i32 {
        self.value
    }
}

fn read[T](value: & T) [T as traits::Source]::Item
where T: traits::Source {
    value.get()
}

fn main() i32 {
    let mut counter = Counter { value: 8 };
    read[Counter](& counter)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn open_associated_const_projection_does_not_forge_concrete_array_length() {
    let root = temp_dir("open_associated_const_projection_does_not_forge_concrete_array_length");
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

fn rewrite[T](value: [[T as Shape]::N]i32) [4]i32
where T: Shape
{
    value
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
fn supertrait_associated_const_projection_does_not_forge_concrete_length() {
    let root = temp_dir("supertrait_associated_const_projection_does_not_forge_concrete_length");
    write(
        &root.join("main.nia"),
        r#"
trait Base {
    const N: usize;
}

trait Sub : Base {}

struct Four {}

extend Four : Base {
    const N: usize = 4usize;
}

extend Four : Sub {}

fn rewrite[T](value: [[T as Base]::N]i32) [4]i32
where T: Sub
{
    value
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
fn associated_const_projection_rejects_cross_const_instance_rewrite() {
    let root = temp_dir("associated_const_projection_rejects_cross_const_instance_rewrite");
    write(
        &root.join("main.nia"),
        r#"
trait Slot[N: usize] {
    const Width: usize;
}

struct Store {}

extend Store : Slot[2] {
    const Width: usize = 2usize;
}

extend Store : Slot[4] {
    const Width: usize = 4usize;
}

fn bad(value: [[Store as Slot[2]]::Width]i32) [[Store as Slot[4]]::Width]i32 {
    value
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
fn associated_const_fake_refs_do_not_runtime_materialize_projection_values() {
    let root = temp_dir("associated_const_fake_refs_do_not_runtime_materialize_projection_values");
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

const fn width[T]() usize
where T: Shape
{
    let value: usize = [T as Shape]::N;
    value
}

const WIDTH: usize = [Four as Shape]::N;

struct Buffer[T, N: usize] {
    values: [N]T,
}

fn make[T](value: T) Buffer[T, WIDTH] {
    Buffer[T, WIDTH] { values: [value; width[Four]()] }
}

fn main() i32 {
    let buffer = make[i32](3);
    buffer.values[3]
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn associated_const_projection_requires_trait_bound_for_open_target() {
    let root = temp_dir("associated_const_projection_requires_trait_bound_for_open_target");
    write(
        &root.join("main.nia"),
        r#"
trait Shape {
    const N: usize;
}

fn bad[T]() usize {
    [T as Shape]::N
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("trait bound not satisfied")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn open_associated_type_and_const_projection_do_not_forge_concrete_array_type() {
    let root =
        temp_dir("open_associated_type_and_const_projection_do_not_forge_concrete_array_type");
    write(
        &root.join("main.nia"),
        r#"
trait Packet {
    type Elem;
    const Len: usize;
}

struct I32x4 {}

extend I32x4 : Packet {
    type Elem = i32;
    const Len: usize = 4usize;
}

fn rewrite[P](value: [[P as Packet]::Len][P as Packet]::Elem) [4]i32
where P: Packet
{
    value
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
fn supertrait_associated_const_const_args_remain_distinct() {
    let root = temp_dir("supertrait_associated_const_const_args_remain_distinct");
    write(
        &root.join("main.nia"),
        r#"
trait Base[N: usize] {
    const Width: usize;
}

trait Sub[N: usize] : Base[N] {}

struct Store {}

extend Store : Base[8] {
    const Width: usize = 8usize;
}

extend Store : Sub[8] {}

fn bad[T](value: [[T as Base[4]]::Width]i32) [4]i32
where T: Sub[8]
{
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
            .contains("trait bound not satisfied")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn associated_const_bool_const_args_do_not_cross_rewrite_instances() {
    let root = temp_dir("associated_const_bool_const_args_do_not_cross_rewrite_instances");
    write(
        &root.join("main.nia"),
        r#"
trait Mode[Enabled: bool] {
    const Width: usize;
}

struct Store {}

extend Store : Mode[true] {
    const Width: usize = 1usize;
}

extend Store : Mode[false] {
    const Width: usize = 2usize;
}

fn bad(value: [[Store as Mode[true]]::Width]i32) [[Store as Mode[false]]::Width]i32 {
    value
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
fn imported_generic_impl_associated_const_substitutes_instance_const_args() {
    let root = temp_dir("imported_generic_impl_associated_const_substitutes_instance_const_args");
    write(
        &root.join("api.nia"),
        r#"
pub trait HasLen {
    const Len: usize;
}
"#,
    );
    write(
        &root.join("types.nia"),
        r#"
pub struct Buf[N: usize] {
    values: [N]u8,
}
"#,
    );
    write(
        &root.join("impls.nia"),
        r#"
using entry::api;
using entry::types;

extend[N: usize] types::Buf[N] : api::HasLen {
    pub const Len: usize = N;
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module api;
module types;
module impls;

using entry::api;
using entry::types;
using entry::impls;

const fn len[T]() usize
where T: api::HasLen
{
    [T as api::HasLen]::Len
}

const FOUR: usize = len[types::Buf[4]]();
const SEVEN: usize = len[types::Buf[7]]();

fn main() usize {
    let four: [FOUR]u8 = [1u8, 2u8, 3u8, 4u8];
    let seven: [SEVEN]u8 = [1u8, 2u8, 3u8, 4u8, 5u8, 6u8, 7u8];
    four[3] as usize + seven[6] as usize
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
