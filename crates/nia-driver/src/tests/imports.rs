// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use crate::{check_program, load_program};

use nia_ids::ModuleId;

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
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("failed to read"))
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
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("function body"))
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
fn imported_runtime_module_can_call_public_items_through_root() {
    let root = temp_dir("imported_runtime_module_can_call_public_items_through_root");
    write(
        &root.join("main.nia"),
        r#"
import .runtime;

pub fn app_main() i32 {
    42
}

fn main() i32 {
    runtime::call_app_main()
}
"#,
    );
    write(
        &root.join("runtime.nia"),
        r#"
import root;

pub fn call_app_main() i32 {
    root::app_main()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn freestanding_start_can_call_public_root_main() {
    let root = temp_dir("freestanding_start_can_call_public_root_main");
    write(
        &root.join("main.nia"),
        r#"
import std.process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
    );

    let program = crate::check_freestanding_executable_with_map_and_options(
        root.join("main.nia").to_string_lossy().into_owned(),
        nia_imports::ModuleMap::default(),
        nia_opt::NiaOptimizationLevel::default(),
    );
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
            .summary
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
    pub fn len2(& self) i32 {
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

fn call(p: & math::Point, f: &fn(& math::Point) i32) i32 {
    f(p)
}

fn main(p: math::Point) i32 {
    call(& p, & math::Point::len2)
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
    pub fn len2(& self) i32 {
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
    pub fn len2(& self) i32 {
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
    assert_eq!(main.type_resolution.node_qualified_type_names.len(), 2);
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
    assert_eq!(main.value_resolution.node_qualified_values.len(), 1);
}

#[test]
fn extends_imported_type_in_current_module() {
    let root = temp_dir("extends_imported_type_in_current_module");
    write(
        &root.join("main.nia"),
        r#"
import .math;

extend math::Point {
    fn len2(& self) i32 {
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
    pub fn len2(& self) i32 {
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
    pub fn len2(& self) i32 {
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

extern fn read_readonly() &u8;

fn main(mut_ptr: &mut u8) i32 {
    var readonly_ptr = read_readonly();
    if mut_ptr.is_null() {
        return 1;
    }
    if readonly_ptr.is_null() {
        return 2;
    }
    0
}
"#,
    );
    write(
        &root.join("ptr.nia"),
        r#"
extend[T] &mut T {
    pub fn is_null(self) bool {
        self as usize == 0
    }
}

extend[T] &T {
    pub fn is_null(self) bool {
        self as usize == 0
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_extension_method_where_clause_constrains_candidates() {
    let root = temp_dir("imported_extension_method_where_clause_constrains_candidates");
    write(
        &root.join("main.nia"),
        r#"
import .containers;

fn main(boxed: containers::Box[bool]) i32 {
    boxed.tag()
}
"#,
    );
    write(
        &root.join("containers.nia"),
        r#"
trait Marker {}

extend i32 : Marker {}

pub struct Box[T] {
    value: T,
}

extend[T] Box[T]
where T: Marker {
    pub fn tag(& self) i32 {
        _ = self;
        1
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("unknown struct field `tag`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn imported_extension_method_where_clause_allows_satisfied_candidates() {
    let root = temp_dir("imported_extension_method_where_clause_allows_satisfied_candidates");
    write(
        &root.join("main.nia"),
        r#"
import .containers;

fn main(boxed: containers::Box[i32]) i32 {
    boxed.tag()
}
"#,
    );
    write(
        &root.join("containers.nia"),
        r#"
trait Marker {}

extend i32 : Marker {}

pub struct Box[T] {
    value: T,
}

extend[T] Box[T]
where T: Marker {
    pub fn tag(& self) i32 {
        _ = self;
        1
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

extern fn read_readonly() &u8;

fn main(mut_ptr: &mut u8) i32 {
    var readonly_ptr = read_readonly();
    if mut_ptr.is_null() {
        return 1;
    }
    if readonly_ptr.is_null() {
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
extend[T] &mut T {
    pub fn is_null(self) bool {
        self as usize == 0
    }
}

extend[T] &T {
    pub fn is_null(self) bool {
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

extern fn read_readonly() &u8;

fn main(mut_ptr: &mut u8) i32 {
    var readonly_ptr = read_readonly();
    if mut_ptr.is_null() {
        return 1;
    }
    if readonly_ptr.is_null() {
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
extend[T] &mut T {
    pub fn is_null(self) bool {
        self as usize == 0
    }
}

extend[T] &T {
    pub fn is_null(self) bool {
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
        assert!(
            program
                .backend_lowering
                .program
                .modules
                .iter()
                .any(|module| module.interner.get(instance.args[0]).is_some()),
            "{instance:?}"
        );
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
    fn len2(& self) i32 {
        4
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .diagnostic
            .summary
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
            .summary
            .contains("type `math::Point` is private")
    }));
    assert!(program.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .diagnostic
            .summary
            .contains("value `math::add` is private")
    }));
}
