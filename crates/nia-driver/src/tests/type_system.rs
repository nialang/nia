// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use crate::check_program;
use nia_ids::ModuleId;

#[test]
fn checks_bodies_against_normalized_type_aliases() {
    let root = temp_dir("checks_bodies_against_normalized_type_aliases");
    write(
        &root.join("main.nia"),
        r#"
type Byte = u8;
fn id(x: Byte) u8 {
    x
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn checks_bodies_against_normalized_generic_type_aliases() {
    let root = temp_dir("checks_bodies_against_normalized_generic_type_aliases");
    write(
        &root.join("main.nia"),
        r#"
type Ptr[T] = &T;
fn id(p: Ptr[u8]) &u8 {
    p
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn supports_alias_to_pointer_extension_methods_without_void_cascades() {
    let root = temp_dir("supports_alias_to_pointer_extension_methods_without_void_cascades");
    write(
        &root.join("main.nia"),
        r#"
type Ptr[T] = &T;

extend[T] Ptr[T] {
    fn is_null(self) bool {
        self as usize == 0
    }
}

fn main(ptr: Ptr[i32]) void {
    if ptr.is_null() {}
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(
        !program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("cannot cast void to usize")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        !program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("cannot cast <error type> to usize")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn supports_structural_extension_methods() {
    let root = temp_dir("supports_structural_extension_methods");
    write(
        &root.join("main.nia"),
        r#"
type Ptr[T] = &T;

extend i32 {
    fn is_zero(self) bool { self == 0 }
}

extend void {
    fn unit(self) i32 { 1 }
}

extend[T] Ptr[T] {
    fn is_null(self) bool { self as usize == 0 }
}

extend[T] &const [T] {
    fn size(self) usize { self.len() }
}

extend[T] [3]T {
    fn first(self) T { self[0] }
}

extend &const fn(i32) i32 {
    fn apply(self, value: i32) i32 { self(value) }
}

fn inc(value: i32) i32 { value + 1 }

fn main(ptr: &i32, xs: &const [i32], triple: [3]i32) i32 {
    if 0.is_zero() {}
    if ptr.is_null() {}
    {}.unit() + xs.size() as i32 + triple.first() + (&const inc).apply(1)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn supports_extension_method_specialization() {
    let root = temp_dir("supports_extension_method_specialization");
    write(
        &root.join("main.nia"),
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

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn reports_ambiguous_extension_method_specialization() {
    let root = temp_dir("reports_ambiguous_extension_method_specialization");
    write(
        &root.join("main.nia"),
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

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("ambiguous method `rank`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn supports_structural_extension_associated_functions() {
    let root = temp_dir("supports_structural_extension_associated_functions");
    write(
        &root.join("main.nia"),
        r#"
extend ! {
    fn nope(self) void {}
}

extend i32 {
    fn make() i32 { 0 }
}

fn main() i32 {
    [i32]::make()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("extend target must be an extendable value type")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        !program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("associated functions are not supported")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn reports_generic_type_argument_count_mismatches() {
    let root = temp_dir("reports_generic_type_argument_count_mismatches");
    write(
        &root.join("main.nia"),
        r#"
import .math;
struct Point {}
struct Box[T] { value: T }
type Pair[T, U] = T;
fn missing_arg(a: Box) {}
fn extra_arg(a: Box[i32, bool]) {}
fn alias_missing_arg(a: Pair[i32]) {}
fn non_generic_arg(a: Point[i32]) {}
fn qualified_missing_arg(a: math::RemoteBox) {}
fn qualified_extra_arg(a: math::RemoteBox[i32, bool]) {}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
pub struct RemoteBox[T] {
    value: T,
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    let mismatch_count = program
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic
                .diagnostic
                .message
                .contains("generic argument count mismatch")
        })
        .count();
    assert_eq!(mismatch_count, 6, "{:?}", program.diagnostics);
}

#[test]
fn checks_qualified_explicit_generic_function_calls() {
    let root = temp_dir("checks_qualified_explicit_generic_function_calls");
    write(
        &root.join("main.nia"),
        r#"
import .math;
fn main(flag: bool) i32 {
    var x: i32 = math::id[i32](1);
    _ = math::id[i32](flag);
    x
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
pub fn id[T](value: T) T {
    value
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        !program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .message
                .contains("binding initializer")
        }),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.diagnostic.message.contains("call argument") })
    );
}

#[test]
fn checks_qualified_inferred_generic_function_calls() {
    let root = temp_dir("checks_qualified_inferred_generic_function_calls");
    write(
        &root.join("main.nia"),
        r#"
import .math;
fn main(flag: bool) i32 {
    var x: i32 = math::id(1);
    _ = math::choose(1, flag);
    x
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
pub fn id[T](value: T) T {
    value
}

pub fn choose[T](left: T, right: T) T {
    left
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        !program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .message
                .contains("binding initializer")
        }),
        "{:?}",
        program.diagnostics
    );
    assert!(program.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .diagnostic
            .message
            .contains("conflicting inferred type for generic parameter `T`")
    }));
}

#[test]
fn collects_unique_monomorphized_instances() {
    let root = temp_dir("collects_unique_monomorphized_instances");
    write(
        &root.join("main.nia"),
        r#"
fn id[T](value: T) T {
    value
}

fn main() i32 {
    var a: i32 = id(1);
    var b: i32 = id(2);
    a + b
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert_eq!(program.monomorphization.instances.len(), 1);
}

#[test]
fn lowers_monomorphized_function_instances_with_readable_symbols() {
    let root = temp_dir("lowers_monomorphized_function_instances_with_readable_symbols");
    write(
        &root.join("main.nia"),
        r#"
fn id[T](value: T) T {
    value
}

fn main() i32 {
    id(1)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let module = &program.backend_lowering.program.modules[0];
    assert_eq!(module.function_instances.len(), 1);
    let symbol = &module.function_instances[0].symbol;
    assert!(symbol.starts_with("nia__m0__d"), "{symbol}");
    assert!(symbol.contains("__id__inst__"), "{symbol}");
    assert!(symbol.contains("i32"), "{symbol}");
}

#[test]
fn reports_recursive_generic_instantiations() {
    let root = temp_dir("reports_recursive_generic_instantiations");
    write(
        &root.join("main.nia"),
        r#"
fn recurse[T](value: T) T {
    recurse[T](value)
}

fn main() i32 {
    recurse(1)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .diagnostic
            .message
            .contains("recursive generic instantiation")
    }));
}

#[test]
fn reports_indirect_recursive_generic_instantiations() {
    let root = temp_dir("reports_indirect_recursive_generic_instantiations");
    write(
        &root.join("main.nia"),
        r#"
fn a[T](value: T) T {
    b[T](value)
}

fn b[T](value: T) T {
    a[T](value)
}

fn main() i32 {
    a(1)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .diagnostic
            .message
            .contains("recursive generic instantiation")
    }));
}

#[test]
fn computes_layouts_in_check_pipeline() {
    let root = temp_dir("computes_layouts_in_check_pipeline");
    write(
        &root.join("main.nia"),
        r#"
struct Pair {
    a: u8,
    b: i32,
}

fn main(p: Pair) {}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let main = program
        .modules
        .iter()
        .find(|module| module.id == ModuleId(0))
        .expect("main module");
    let pair_id = main.defs.module_scope.types.get("Pair").expect("Pair def");
    let pair_layout = main.layouts.structs.get(&pair_id).expect("Pair layout");
    assert_eq!(
        pair_layout.layout,
        nia_layout::TypeLayout { size: 8, align: 4 }
    );
    assert_eq!(pair_layout.fields[0].offset, 0);
    assert_eq!(pair_layout.fields[1].offset, 4);
}

#[test]
fn checks_cross_module_struct_literals() {
    let root = temp_dir("checks_cross_module_struct_literals");
    write(
        &root.join("main.nia"),
        r#"
import .geom;

fn main() i32 {
    var p: geom::Point = { x: 40, y: 2 };
    p.x + p.y
}
"#,
    );
    write(
        &root.join("geom.nia"),
        r#"
pub struct Point {
    x: i32,
    y: i32,
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
