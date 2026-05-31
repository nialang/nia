// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_ids::ModuleId;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[test]
fn loads_root_and_imported_modules_once() {
    let root = temp_dir("loads_root_and_imported_modules_once");
    write(
        &root.join("main.nia"),
        r#"import .math; fn main() i32 { 0 }"#,
    );
    write(
        &root.join("math.nia"),
        r#"fn add(a: i32, b: i32) i32 { a + b }"#,
    );

    let program = load_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert_eq!(program.modules.len(), 2);
    assert!(
        program
            .modules
            .iter()
            .all(|module| module.parse_errors.is_empty())
    );
    assert_eq!(
        program
            .graph
            .get(program.graph.root())
            .expect("root module")
            .imports
            .len(),
        1
    );
    let math = program
        .imports
        .get(program.graph.root(), "math")
        .expect("math import alias");
    assert_eq!(math.target, ModuleId(1));
}

#[test]
fn reports_missing_imported_modules() {
    let root = temp_dir("reports_missing_imported_modules");
    write(&root.join("main.nia"), r#"import .missing; fn main() {}"#);

    let program = load_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.message.contains("failed to read"))
    );
}

#[test]
fn checks_each_loaded_module() {
    let root = temp_dir("checks_each_loaded_module");
    write(
        &root.join("main.nia"),
        r#"import .math; fn main() i32 { 0 }"#,
    );
    write(&root.join("math.nia"), r#"fn bad() i32 { true }"#);

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_eq!(program.modules.len(), 2);
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.message.contains("function body"))
    );
}

#[test]
fn loads_cyclic_imports_without_cycle_diagnostic() {
    let root = temp_dir("loads_cyclic_imports_without_cycle_diagnostic");
    write(&root.join("main.nia"), r#"import .a; fn main() {}"#);
    write(&root.join("a.nia"), r#"import .b;"#);
    write(&root.join("b.nia"), r#"import .a;"#);

    let program = load_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert_eq!(program.modules.len(), 3);
}

#[test]
fn resolves_public_items_inside_cyclic_imports() {
    let root = temp_dir("resolves_public_items_inside_cyclic_imports");
    write(
        &root.join("main.nia"),
        r#"import .a; fn main() i32 { a::from_b() }"#,
    );
    write(
        &root.join("a.nia"),
        r#"
import .b;

pub fn from_b() i32 {
    b::value()
}
"#,
    );
    write(
        &root.join("b.nia"),
        r#"
import .a;

pub fn value() i32 {
    42
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn rejects_private_items_inside_cyclic_imports() {
    let root = temp_dir("rejects_private_items_inside_cyclic_imports");
    write(
        &root.join("main.nia"),
        r#"import .a; fn main() i32 { a::from_b() }"#,
    );
    write(
        &root.join("a.nia"),
        r#"
import .b;

pub fn from_b() i32 {
    b::value()
}
"#,
    );
    write(
        &root.join("b.nia"),
        r#"
import .a;

fn value() i32 {
    42
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .diagnostic
            .message
            .contains("value `b::value` is private")
    }));
}

#[test]
fn resolves_public_extension_methods_inside_cyclic_imports() {
    let root = temp_dir("resolves_public_extension_methods_inside_cyclic_imports");
    write(
        &root.join("main.nia"),
        r#"
import .a;

fn main(p: a::Point) i32 {
    p.len2()
}
"#,
    );
    write(
        &root.join("a.nia"),
        r#"
import .b;

pub struct Point {
    x: i32,
    y: i32,
}
"#,
    );
    write(
        &root.join("b.nia"),
        r#"
import .a;

extend a::Point {
    pub fn len2(&const self) i32 {
        4
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn resolves_imported_extension_method_function_pointers() {
    let root = temp_dir("resolves_imported_extension_method_function_pointers");
    write(
        &root.join("main.nia"),
        r#"
import .math;

fn call(p: &const math::Point, f: &const fn(&const math::Point) i32) i32 {
    f(p)
}

fn main(p: math::Point) i32 {
    call(&const p, &const math::Point::len2)
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
pub struct Point {
    x: i32,
    y: i32,
}

extend Point {
    pub fn len2(&const self) i32 {
        self.x * self.x + self.y * self.y
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn resolves_items_and_extensions_through_complex_cyclic_import_graph() {
    let root = temp_dir("resolves_items_and_extensions_through_complex_cyclic_import_graph");
    write(
        &root.join("main.nia"),
        r#"
import .core;
import .entry;

fn main(p: core::Point) i32 {
    entry::score(p)
}
"#,
    );
    write(
        &root.join("core.nia"),
        r#"
import .ops;

pub struct Point {
    x: i32,
    y: i32,
}

pub fn base() i32 {
    10
}
"#,
    );
    write(
        &root.join("entry.nia"),
        r#"
import .core;
import .math;
import .ops;

pub fn score(p: core::Point) i32 {
    p.len2() + math::via_helpers() + ops::from_cycle()
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
import .helpers;

pub fn via_helpers() i32 {
    helpers::call_core()
}
"#,
    );
    write(
        &root.join("helpers.nia"),
        r#"
import .core;
import .ops;

pub fn call_core() i32 {
    core::base() + ops::from_cycle()
}
"#,
    );
    write(
        &root.join("ops.nia"),
        r#"
import .core;
import .helpers;

extend core::Point {
    pub fn len2(&const self) i32 {
        helpers::call_core()
    }
}

pub fn from_cycle() i32 {
    core::base()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn deduplicates_imported_paths() {
    let root = temp_dir("deduplicates_imported_paths");
    write(
        &root.join("main.nia"),
        r#"
import .math as math_a;
import .math as math_b;
fn main() {}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"fn add(a: i32, b: i32) i32 { a + b }"#,
    );

    let program = load_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert_eq!(program.modules.len(), 2);
}

#[test]
fn resolves_qualified_imported_type_paths() {
    let root = temp_dir("resolves_qualified_imported_type_paths");
    write(
        &root.join("main.nia"),
        r#"
import .math;
fn origin(p: math::Point) math::Point { p }
"#,
    );
    write(&root.join("math.nia"), r#"pub struct Point { x: i32 }"#);

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let main = program
        .modules
        .iter()
        .find(|module| module.id == ModuleId(0))
        .expect("main module");
    assert_eq!(main.type_resolution.qualified_type_names.len(), 2);
}

#[test]
fn resolves_qualified_imported_function_calls() {
    let root = temp_dir("resolves_qualified_imported_function_calls");
    write(
        &root.join("main.nia"),
        r#"
import .math;
fn main() i32 {
    math::add(40, 2)
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"pub fn add(a: i32, b: i32) i32 { a + b }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let main = program
        .modules
        .iter()
        .find(|module| module.id == ModuleId(0))
        .expect("main module");
    assert_eq!(main.value_resolution.qualified_values.len(), 1);
}

#[test]
fn extends_imported_type_in_current_module() {
    let root = temp_dir("extends_imported_type_in_current_module");
    write(
        &root.join("main.nia"),
        r#"
import .math;

extend math::Point {
    fn len2(&const self) i32 {
        4
    }
}

fn main(p: math::Point) i32 {
    p.len2()
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"pub struct Point { x: i32, y: i32 }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imports_public_extension_methods() {
    let root = temp_dir("imports_public_extension_methods");
    write(
        &root.join("main.nia"),
        r#"
import .math;
import .point_ext;

fn main(p: math::Point) i32 {
    p.len2()
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"pub struct Point { x: i32, y: i32 }"#,
    );
    write(
        &root.join("point_ext.nia"),
        r#"
import .math;

extend math::Point {
    pub fn len2(&const self) i32 {
        4
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imports_public_extension_methods_through_import_closure() {
    let root = temp_dir("imports_public_extension_methods_through_import_closure");
    write(
        &root.join("main.nia"),
        r#"
import .math;
import .facade;

fn main(p: math::Point) i32 {
    p.len2()
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"pub struct Point { x: i32, y: i32 }"#,
    );
    write(&root.join("facade.nia"), r#"import .point_ext;"#);
    write(
        &root.join("point_ext.nia"),
        r#"
import .math;

extend math::Point {
    pub fn len2(&const self) i32 {
        4
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imports_generic_structural_extension_methods() {
    let root = temp_dir("imports_generic_structural_extension_methods");
    write(
        &root.join("main.nia"),
        r#"
import .ptr;

extern fn read_const() &const u8;

fn main(mut_ptr: &u8) i32 {
    var const_ptr = read_const();
    if mut_ptr.null() {
        return 1;
    }
    if const_ptr.null() {
        return 2;
    }
    0
}
"#,
    );
    write(
        &root.join("ptr.nia"),
        r#"
extend[T] &const T {
    pub fn null(self) bool {
        self as usize == 0
    }
}

extend[T] &T {
    pub fn null(self) bool {
        self as usize == 0
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imports_generic_structural_extension_methods_through_import_closure() {
    let root = temp_dir("imports_generic_structural_extension_methods_through_import_closure");
    write(
        &root.join("main.nia"),
        r#"
import .share;

extern fn read_const() &const u8;

fn main(mut_ptr: &u8) i32 {
    var const_ptr = read_const();
    if mut_ptr.null() {
        return 1;
    }
    if const_ptr.null() {
        return 2;
    }
    0
}
"#,
    );
    write(&root.join("share.nia"), r#"import .ptr;"#);
    write(
        &root.join("ptr.nia"),
        r#"
extend[T] &const T {
    pub fn null(self) bool {
        self as usize == 0
    }
}

extend[T] &T {
    pub fn null(self) bool {
        self as usize == 0
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn lowers_imported_generic_structural_extension_instances() {
    let root = temp_dir("lowers_imported_generic_structural_extension_instances");
    write(
        &root.join("main.nia"),
        r#"
import .share;

extern fn read_const() &const u8;

fn main(mut_ptr: &u8) i32 {
    var const_ptr = read_const();
    if mut_ptr.null() {
        return 1;
    }
    if const_ptr.null() {
        return 2;
    }
    0
}
"#,
    );
    write(&root.join("share.nia"), r#"import .ptr;"#);
    write(
        &root.join("ptr.nia"),
        r#"
extend[T] &const T {
    pub fn null(self) bool {
        self as usize == 0
    }
}

extend[T] &T {
    pub fn null(self) bool {
        self as usize == 0
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);

    let instances = program
        .backend_lowering
        .program
        .modules
        .iter()
        .flat_map(|module| module.function_instances.iter())
        .collect::<Vec<_>>();
    assert_eq!(instances.len(), 2, "{instances:?}");
    for instance in instances {
        assert_eq!(instance.args.len(), 1, "{instance:?}");
        assert_eq!(instance.arg_module_id, instance.args[0].interner_id);
    }
}

#[test]
fn imports_generic_structural_extension_methods_with_alias_targets() {
    let root = temp_dir("imports_generic_structural_extension_methods_with_alias_targets");
    write(
        &root.join("main.nia"),
        r#"
import .ptr;

fn main(ptr: &u8) i32 {
    if ptr.alias_null() {
        return 1;
    }
    0
}
"#,
    );
    write(
        &root.join("ptr.nia"),
        r#"
type Ptr[T] = &T;

extend[T] Ptr[T] {
    pub fn alias_null(self) bool {
        self as usize == 0
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn does_not_import_private_extension_methods_through_import_closure() {
    let root = temp_dir("does_not_import_private_extension_methods_through_import_closure");
    write(
        &root.join("main.nia"),
        r#"
import .math;
import .facade;

fn main(p: math::Point) i32 {
    p.len2()
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"pub struct Point { x: i32, y: i32 }"#,
    );
    write(&root.join("facade.nia"), r#"import .point_ext;"#);
    write(
        &root.join("point_ext.nia"),
        r#"
import .math;

extend math::Point {
    fn len2(&const self) i32 {
        4
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .diagnostic
            .message
            .contains("unknown struct field `len2`")
    }));
}

#[test]
fn rejects_private_cross_module_items() {
    let root = temp_dir("rejects_private_cross_module_items");
    write(
        &root.join("main.nia"),
        r#"
import .math;
fn take(p: math::Point) i32 {
    math::add(1, 2)
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
struct Point { x: i32 }
fn add(a: i32, b: i32) i32 { a + b }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .diagnostic
            .message
            .contains("type `math::Point` is private")
    }));
    assert!(program.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .diagnostic
            .message
            .contains("value `math::add` is private")
    }));
}

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
    fn size(self) usize { @len(self) }
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

#[test]
fn using_brings_imported_function_into_scope() {
    let root = temp_dir("using_brings_imported_function_into_scope");
    write(
        &root.join("main.nia"),
        r#"
import .math;
using math::add;

fn main() i32 {
    add(40, 2)
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"pub fn add(a: i32, b: i32) i32 { a + b }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn using_supports_group_and_rename() {
    let root = temp_dir("using_supports_group_and_rename");
    write(
        &root.join("main.nia"),
        r#"
import .math;
using math::{add, sub as minus};

fn main() i32 {
    add(40, minus(4, 2))
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
pub fn add(a: i32, b: i32) i32 { a + b }
pub fn sub(a: i32, b: i32) i32 { a - b }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn using_group_supports_nested_enum_wildcard() {
    let root = temp_dir("using_group_supports_nested_enum_wildcard");
    write(
        &root.join("main.nia"),
        r#"
import .math;
using math::{add, sub as minus, Operator::*};

fn main(flag: bool) math::Operator {
    var n = add(40, minus(4, 2));
    if flag { Add } else { Sub }
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
pub enum Operator: u8 { Add, Sub }
pub fn add(a: i32, b: i32) i32 { a + b }
pub fn sub(a: i32, b: i32) i32 { a - b }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn using_wildcard_imports_public_surface() {
    let root = temp_dir("using_wildcard_imports_public_surface");
    write(
        &root.join("main.nia"),
        r#"
import .math;
using math::*;

fn main(p: Point) i32 {
    add(p.x, p.y)
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
pub struct Point { x: i32, y: i32 }
pub fn add(a: i32, b: i32) i32 { a + b }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn using_wildcard_imports_pub_using_reexports() {
    let root = temp_dir("using_wildcard_imports_pub_using_reexports");
    write(
        &root.join("main.nia"),
        r#"
import .facade;
using facade::*;

fn main(p: Point) i32 {
    add(p.x, p.y)
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
import .impl;
pub using impl::*;
"#,
    );
    write(
        &root.join("impl.nia"),
        r#"
pub struct Point { x: i32, y: i32 }
pub fn add(a: i32, b: i32) i32 { a + b }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn pub_using_module_namespace_is_visible_downstream() {
    let root = temp_dir("pub_using_module_namespace_is_visible_downstream");
    write(
        &root.join("main.nia"),
        r#"
import .facade;

fn main() i32 {
    facade::impl::add(40, 2)
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
import .impl;
pub using impl;
"#,
    );
    write(
        &root.join("impl.nia"),
        r#"pub fn add(a: i32, b: i32) i32 { a + b }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn using_wildcard_brings_reexported_module_namespace_into_scope() {
    let root = temp_dir("using_wildcard_brings_reexported_module_namespace_into_scope");
    write(
        &root.join("main.nia"),
        r#"
import .facade;
using facade::*;

fn main() i32 {
    impl::add(40, 2)
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
import .impl;
pub using impl;
"#,
    );
    write(
        &root.join("impl.nia"),
        r#"pub fn add(a: i32, b: i32) i32 { a + b }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn pub_using_reexports_for_downstream_modules() {
    let root = temp_dir("pub_using_reexports_for_downstream_modules");
    write(
        &root.join("main.nia"),
        r#"
import .facade;

fn main() i32 {
    facade::add(40, 2)
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
import .impl;
pub using impl::add;
"#,
    );
    write(
        &root.join("impl.nia"),
        r#"pub fn add(a: i32, b: i32) i32 { a + b }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn pub_using_group_supports_nested_enum_wildcard() {
    let root = temp_dir("pub_using_group_supports_nested_enum_wildcard");
    write(
        &root.join("main.nia"),
        r#"
import .facade;

fn main(flag: bool) facade::Operator {
    var n = facade::add(40, facade::minus(4, 2));
    if flag { facade::Add } else { facade::Sub }
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
import .math;
pub using math::{Operator, add, sub as minus, Operator::*};
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
pub enum Operator: u8 { Add, Sub }
pub fn add(a: i32, b: i32) i32 { a + b }
pub fn sub(a: i32, b: i32) i32 { a - b }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn pub_using_root_group_reexports_modules_items_and_variants() {
    let root = temp_dir("pub_using_root_group_reexports_modules_items_and_variants");
    write(
        &root.join("main.nia"),
        r#"
import .facade;

fn main(flag: bool) facade::palette::Color {
    var n = facade::add(40, 2);
    if flag { facade::Red } else { facade::DDD }
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
import .math;
import .palette;
pub using {math, math::add, palette, palette::Color::{Red, DDD}};
"#,
    );
    write(
        &root.join("math.nia"),
        r#"pub fn add(a: i32, b: i32) i32 { a + b }"#,
    );
    write(
        &root.join("palette.nia"),
        r#"pub enum Color: u8 { Red, DDD }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn using_supports_deep_nested_module_groups() {
    let root = temp_dir("using_supports_deep_nested_module_groups");
    write(
        &root.join("main.nia"),
        r#"
import .rootmod;
using rootmod::a::{b::c::foo, d::e::{f::goo, g}, h::Color::*};

fn main(flag: bool) rootmod::a::h::Color {
    var n = foo(40, 2) + goo(1) + g(2);
    if flag { Red } else { Blue }
}
"#,
    );
    write(
        &root.join("rootmod.nia"),
        r#"
import .a;
pub using a;
"#,
    );
    write(
        &root.join("a.nia"),
        r#"
import .b;
import .d;
import .h;
pub using {b, d, h};
"#,
    );
    write(
        &root.join("b.nia"),
        r#"
import .c;
pub using c;
"#,
    );
    write(
        &root.join("c.nia"),
        r#"pub fn foo(a: i32, b: i32) i32 { a + b }"#,
    );
    write(
        &root.join("d.nia"),
        r#"
import .e;
pub using e;
"#,
    );
    write(
        &root.join("e.nia"),
        r#"
import .f;
pub using f;
pub fn g(a: i32) i32 { a + 3 }
"#,
    );
    write(&root.join("f.nia"), r#"pub fn goo(a: i32) i32 { a + 4 }"#);
    write(&root.join("h.nia"), r#"pub enum Color: u8 { Red, Blue }"#);

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn using_unknown_name_reports_diagnostic() {
    let root = temp_dir("using_unknown_name_reports_diagnostic");
    write(
        &root.join("main.nia"),
        r#"
import .math;
using math::missing;

fn main() i32 { 0 }
"#,
    );
    write(
        &root.join("math.nia"),
        r#"pub fn add(a: i32, b: i32) i32 { a + b }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .message
                .contains("could not be resolved")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn pub_import_is_rejected() {
    let root = temp_dir("pub_import_is_rejected");
    write(
        &root.join("main.nia"),
        r#"
pub import .math;
fn main() i32 { 0 }
"#,
    );
    write(
        &root.join("math.nia"),
        r#"pub fn add(a: i32, b: i32) i32 { a + b }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("`pub` cannot be applied")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn using_local_enum_variant_brings_bare_name() {
    let root = temp_dir("using_local_enum_variant_brings_bare_name");
    write(
        &root.join("main.nia"),
        r#"
enum Color: u8 { Red, Black }
using Color::Red;

fn main() Color { Red }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn using_local_enum_wildcard_brings_all_variants() {
    let root = temp_dir("using_local_enum_wildcard_brings_all_variants");
    write(
        &root.join("main.nia"),
        r#"
enum Color: u8 { Red, Black, Green }
using Color::*;

fn pick(flag: bool) Color {
    if flag { Red } else { Black }
}

fn main() Color { pick(true) }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn using_cross_module_enum_variant_three_segments() {
    let root = temp_dir("using_cross_module_enum_variant_three_segments");
    write(
        &root.join("main.nia"),
        r#"
import .palette;
using palette::Color::{Red, Black as Dark};

fn main() palette::Color {
    var c: palette::Color = Red;
    Dark
}
"#,
    );
    write(
        &root.join("palette.nia"),
        r#"
pub enum Color: u8 { Red, Black, Green }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn pub_using_enum_variant_reexports_for_downstream() {
    let root = temp_dir("pub_using_enum_variant_reexports_for_downstream");
    write(
        &root.join("main.nia"),
        r#"
import .facade;

fn main() facade::Color {
    facade::Red
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
import .palette;
pub using palette::Color;
pub using palette::Color::Red;
"#,
    );
    write(
        &root.join("palette.nia"),
        r#"
pub enum Color: u8 { Red, Black, Green }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn using_unknown_enum_variant_reports_diagnostic() {
    let root = temp_dir("using_unknown_enum_variant_reports_diagnostic");
    write(
        &root.join("main.nia"),
        r#"
enum Color: u8 { Red, Black }
using Color::Purple;

fn main() Color { Red }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("unknown enum variant")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn qualified_cross_module_enum_variant_access() {
    let root = temp_dir("qualified_cross_module_enum_variant_access");
    write(
        &root.join("main.nia"),
        r#"
import .palette;

fn main() palette::Color {
    var c: palette::Color = palette::Color::Red;
    palette::Color::Black
}
"#,
    );
    write(
        &root.join("palette.nia"),
        r#"pub enum Color: u8 { Red, Black, Green }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn using_imported_type_supports_enum_variants_and_associated_functions() {
    let root = temp_dir("using_imported_type_supports_enum_variants_and_associated_functions");
    write(
        &root.join("main.nia"),
        r#"
import .defs;

using defs::{Box, Mode};

fn main() i32 {
    var box = Box::make(Mode::A);
    box.mode as u8 as i32
}
"#,
    );
    write(
        &root.join("defs.nia"),
        r#"
pub enum Mode: u8 {
    A,
    B,
}

pub struct Box {
    mode: Mode,
}

extend Box {
    pub fn make(mode: Mode) Box {
        { mode: mode }
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn switch_exhaustive_over_cross_module_enum() {
    let root = temp_dir("switch_exhaustive_over_cross_module_enum");
    write(
        &root.join("main.nia"),
        r#"
import .palette;

fn pick(c: palette::Color) i32 {
    switch c {
        palette::Color::Red => return 0,
        palette::Color::Black => return 1,
        palette::Color::Green => return 2,
    }
    -1
}

fn main() i32 { pick(palette::Color::Red) }
"#,
    );
    write(
        &root.join("palette.nia"),
        r#"pub enum Color: u8 { Red, Black, Green }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn switch_over_cross_module_enum_reports_missing_variants() {
    let root = temp_dir("switch_over_cross_module_enum_reports_missing_variants");
    write(
        &root.join("main.nia"),
        r#"
import .palette;

fn pick(c: palette::Color) i32 {
    switch c {
        palette::Color::Red => return 0,
    }
    -1
}

fn main() i32 { pick(palette::Color::Red) }
"#,
    );
    write(
        &root.join("palette.nia"),
        r#"pub enum Color: u8 { Red, Black, Green }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("non-exhaustive enum switch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn uses_cross_module_public_union() {
    let root = temp_dir("uses_cross_module_public_union");
    write(
        &root.join("main.nia"),
        r#"
import .bits;

fn main() i32 {
    var value: bits::Bits = { i: 7 };
    value.i
}
"#,
    );
    write(
        &root.join("bits.nia"),
        r#"pub union Bits { i: i32, f: f32 }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn rejects_cross_module_nia_types_at_extern_abi_boundaries() {
    let root = temp_dir("rejects_cross_module_nia_types_at_extern_abi_boundaries");
    write(
        &root.join("main.nia"),
        r#"
import .types;

extern fn bad_struct(point: types::Point);
extern fn bad_union(bits: types::Bits);
extern fn bad_enum(color: types::Color);
"#,
    );
    write(
        &root.join("types.nia"),
        r#"
pub struct Point { x: i32 }
pub union Bits { i: i32 }
pub enum Color: u8 { Red }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    for expected in [
        "normal Nia struct by value",
        "union by value",
        "enum directly",
    ] {
        assert!(
            program
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.diagnostic.message.contains(expected)),
            "{expected}: {:?}",
            program.diagnostics
        );
    }
}

#[test]
fn public_comptime_values_are_visible_through_import_closure() {
    let root = temp_dir("public_comptime_values_are_visible_through_import_closure");
    write(
        &root.join("main.nia"),
        r#"
import .facade;

fn main() i32 {
    facade::answer
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
import .defs;
pub using defs::answer;
"#,
    );
    write(
        &root.join("defs.nia"),
        r#"
pub comptime answer: i32 = 42;
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let main_module = program
        .backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module");
    assert!(main_module.globals.is_empty());
}

#[test]
fn comptime_values_drive_array_lengths() {
    let root = temp_dir("comptime_values_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
pub comptime width: usize = 2 + 2;

fn main() i32 {
    comptime local_width: usize = width;
    var values: [local_width]i32 = [1, 2, 3, 4];
    values[3]
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_comptime_values_drive_array_lengths() {
    let root = temp_dir("imported_comptime_values_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
import .config;

fn main() i32 {
    var values: [config::width]i32 = [1, 2, 3, 4];
    values[config::width - 1]
}
"#,
    );
    write(
        &root.join("config.nia"),
        r#"
pub comptime width: usize = 4;
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_struct_field_array_length_accepts_literal_repeat_count() {
    let root = temp_dir("imported_struct_field_array_length_accepts_literal_repeat_count");
    write(
        &root.join("defs.nia"),
        r#"
pub comptime N: usize = 4;

pub struct Item {
    value: u32,
}

pub struct Boxed {
    items: [N]Item,
}

extend Item {
    pub fn zero() Item {
        { value: 0 }
    }
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .defs;
using defs::*;

fn make() Boxed {
    {
        items: [Item::zero(); 4],
    }
}

fn main() i32 {
    var x = make();
    x.items[0].value as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_struct_field_array_length_accepts_imported_repeat_count() {
    let root = temp_dir("imported_struct_field_array_length_accepts_imported_repeat_count");
    write(
        &root.join("defs.nia"),
        r#"
pub comptime N: usize = 4;

pub struct Item {
    value: u32,
}

pub struct Boxed {
    items: [N]Item,
}

extend Item {
    pub fn zero() Item {
        { value: 0 }
    }
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .defs;
using defs::*;

fn make() Boxed {
    {
        items: [Item::zero(); defs::N],
    }
}

fn main() i32 {
    var x = make();
    x.items[0].value as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_dependency_cycles_are_diagnosed() {
    let root = temp_dir("comptime_dependency_cycles_are_diagnosed");
    write(
        &root.join("main.nia"),
        r#"
comptime a: i32 = b;
comptime b: i32 = a;

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("cyclic comptime dependency")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_rejects_runtime_local_dependency() {
    let root = temp_dir("comptime_rejects_runtime_local_dependency");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    var runtime = 4;
    comptime n: usize = runtime;
    var values: [n]i32 = [1, 2, 3, 4];
    values[0]
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("comptime expression can only use comptime bindings")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn trait_impl_methods_are_checked_against_trait_requirements() {
    let root = temp_dir("trait_impl_methods_are_checked_against_trait_requirements");
    write(
        &root.join("main.nia"),
        r#"
trait Show {
    fn show(&const self) i32;
}

struct Point {
    x: i32,
}

extend Point : Show {
    fn show(&const self) i32 {
        self.x
    }
}

fn main() i32 {
    var point: Point = { x: 7 };
    point.show()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn trait_impl_rejects_extra_missing_and_mismatched_methods() {
    let root = temp_dir("trait_impl_rejects_extra_missing_and_mismatched_methods");
    write(
        &root.join("main.nia"),
        r#"
trait Show {
    fn show(&const self) i32;
    fn size(&const self) i32;
}

struct Point {
    x: i32,
}

extend Point : Show {
    fn show(&self) i32 {
        self.x
    }

    fn debug(&const self) i32 {
        self.x
    }
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
            .message
            .contains("does not match the trait signature")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("is not a member of implemented trait")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("missing implementation for trait method `size`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn trait_impl_substitutes_self_in_required_signatures() {
    let root = temp_dir("trait_impl_substitutes_self_in_required_signatures");
    write(
        &root.join("main.nia"),
        r#"
trait Eq {
    fn eq(&const self, other: &const Self) bool;
}

struct Point {
    x: i32,
}

extend Point : Eq {
    fn eq(&const self, other: &const Point) bool {
        self.x == other.x
    }
}

fn main() bool {
    var a: Point = { x: 1 };
    var b: Point = { x: 1 };
    a.eq(&const b)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn cross_module_trait_impls_are_checked() {
    let root = temp_dir("cross_module_trait_impls_are_checked");
    write(
        &root.join("main.nia"),
        r#"
import .traits;

struct Point {
    x: i32,
}

extend Point : traits::Show {
    fn show(&const self) i32 {
        self.x
    }

    fn debug(&const self) i32 {
        self.x
    }
}

fn main() i32 {
    0
}
"#,
    );
    write(
        &root.join("traits.nia"),
        r#"
pub trait Show {
    fn show(&const self) i32;
    fn size(&const self) i32;
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("is not a member of implemented trait")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("missing implementation for trait method `size`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn associated_types_resolve_explicit_projection_in_trait_methods() {
    let root = temp_dir("associated_types_resolve_explicit_projection_in_trait_methods");
    write(
        &root.join("main.nia"),
        r#"
trait Source {
    type Item;

    fn get(&const self) [Self as Source]::Item;
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    type Item = i32;

    fn get(&const self) i32 {
        self.value
    }
}

fn read[T](value: &const T) [T as Source]::Item
where T: Source {
    value.get()
}

fn main() i32 {
    var counter: Counter = { value: 3 };
    read[Counter](&const counter)
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
            .message
            .contains("missing definition for associated type `B`")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
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

    fn get(&const self) i32 {
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
            .message
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
            .message
            .contains("duplicate trait associated type")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
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
            .message
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
            .message
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

    fn get(&const self) [Self as Source]::Item;
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    type Item = i32;

    fn get(&const self) bool {
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
            .any(|diagnostic| diagnostic.diagnostic.message.contains(
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

    fn map_c(&const self, a: A, b: B) [Self as Mapper[A, B]]::C;
    fn map_d(&const self, a: A, b: B, fallback: [Self as Mapper[A, B]]::D) [Self as Mapper[A, B]]::D {
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

    fn map_c(&const self, a: i32, b: i32) i32 {
        self.seed + a + b
    }
}

fn mapped[T](value: &const T, fallback: [T as Mapper[i32, i32]]::D) [T as Mapper[i32, i32]]::D
where T: Mapper[i32, i32] {
    value.map_d(1, 2, fallback)
}

fn main() i32 {
    var p: Pairer = { seed: 3 };
    mapped[Pairer](&const p, 9)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn cross_module_associated_type_projection_resolves_impl_definition() {
    let root = temp_dir("cross_module_associated_type_projection_resolves_impl_definition");
    write(
        &root.join("traits.nia"),
        r#"
pub trait Source {
    type Item;

    fn get(&const self) [Self as Source]::Item;
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .traits;

struct Counter {
    value: i32,
}

extend Counter : traits::Source {
    type Item = i32;

    fn get(&const self) i32 {
        self.value
    }
}

fn read[T](value: &const T) [T as traits::Source]::Item
where T: traits::Source {
    value.get()
}

fn main() i32 {
    var counter: Counter = { value: 8 };
    read[Counter](&const counter)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_where_bound_trait_methods_dispatch_to_impl_instances() {
    let root = temp_dir("generic_where_bound_trait_methods_dispatch_to_impl_instances");
    write(
        &root.join("main.nia"),
        r#"
trait Eq {
    fn eq(&const self, other: &const Self) bool;
}

struct Point {
    x: i32,
}

extend Point : Eq {
    fn eq(&const self, other: &const Point) bool {
        self.x == other.x
    }
}

fn same[T](a: &const T, b: &const T) bool
where T: Eq {
    a.eq(b)
}

fn main() bool {
    var a: Point = { x: 1 };
    var b: Point = { x: 1 };
    same[Point](&const a, &const b)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn trait_default_methods_are_used_when_impl_omits_method() {
    let root = temp_dir("trait_default_methods_are_used_when_impl_omits_method");
    write(
        &root.join("main.nia"),
        r#"
trait Eq {
    fn eq(&const self, other: &const Self) bool;

    fn ne(&const self, other: &const Self) bool {
        !self.eq(other)
    }
}

struct Point {
    x: i32,
}

extend Point : Eq {
    fn eq(&const self, other: &const Point) bool {
        self.x == other.x
    }
}

fn different[T](a: &const T, b: &const T) bool
where T: Eq {
    a.ne(b)
}

fn main() bool {
    var a: Point = { x: 1 };
    var b: Point = { x: 2 };
    different[Point](&const a, &const b)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nia-driver-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write(path: &Path, source: &str) {
    fs::write(path, source).expect("write source file");
}
