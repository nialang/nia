// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

use nia_ids::ModuleId;
use nia_imports::{ModuleMap, SourcePath};
use nia_loader_query::load_program;
use std::{fs, path::Path};

trait TestModulePath {
    fn path(&self) -> &SourcePath;
}

impl TestModulePath for crate::LoadedModule {
    fn path(&self) -> &SourcePath {
        &self.path
    }
}

impl TestModulePath for crate::CheckedModule {
    fn path(&self) -> &SourcePath {
        &self.path
    }
}

fn modules_under<'a, M: TestModulePath + 'a>(
    modules: impl IntoIterator<Item = &'a M>,
    root: &Path,
) -> usize {
    let root = root.to_string_lossy();
    modules
        .into_iter()
        .filter(|module| module.path().as_str().starts_with(root.as_ref()))
        .count()
}

#[test]
fn loads_entry_and_imported_modules_once() {
    let root = temp_dir("loads_entry_and_imported_modules_once");
    write(
        &root.join("main.nia"),
        r#"module math; using entry::math; fn main() i32 { 0 }"#,
    );
    write(
        &root.join("math.nia"),
        r#"fn add(a: i32, b: i32) i32 { a + b }"#,
    );

    let program = load_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert_eq!(modules_under(&program.modules, &root), 2);
    assert!(
        program
            .modules
            .iter()
            .all(|module| module.parse_errors.is_empty())
    );
    assert_eq!(
        program
            .graph
            .get(program.graph.entry())
            .expect("entry module")
            .declarations
            .len(),
        1
    );
    let math = program
        .graph
        .get(program.graph.entry())
        .and_then(|root| root.children.get("math").copied())
        .expect("math child module");
    let math_path = SourcePath::new(root.join("math.nia").to_string_lossy());
    let math_module = program
        .modules
        .iter()
        .find(|module| module.path == math_path)
        .expect("math module");
    assert_eq!(math, math_module.id);
    assert_ne!(math, program.graph.entry());
}

#[test]
fn reports_missing_imported_modules() {
    let root = temp_dir("reports_missing_imported_modules");
    write(
        &root.join("main.nia"),
        r#"module missing;
using entry::missing; fn main() {}"#,
    );

    let program = load_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("failed to read"))
    );
}

#[test]
fn package_root_names_current_mapped_package() {
    let root = temp_dir("package_root_names_current_mapped_package");
    let dep_root = root.join("dep.nia");
    write(
        &root.join("main.nia"),
        r#"
using dep::api;

fn main() i32 {
    api::answer()
}
"#,
    );
    write(&dep_root, "pub module api; pub module helper;");
    fs::create_dir_all(root.join("dep")).expect("create dep dir");
    write(
        &root.join("dep/api.nia"),
        r#"
using pkg::helper;

pub fn answer() i32 {
    helper::answer()
}
"#,
    );
    write(&root.join("dep/helper.nia"), "pub fn answer() i32 { 42 }");
    let mut module_map = ModuleMap::new();
    module_map.insert("dep", SourcePath::new(dep_root.to_string_lossy()));

    let program = check_program_with_map(
        root.join("main.nia").to_string_lossy().into_owned(),
        module_map,
    );

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn entry_inside_mapped_package_still_names_entry_module() {
    let root = temp_dir("entry_inside_mapped_package_still_names_entry_module");
    let dep_root = root.join("dep.nia");
    write(
        &root.join("main.nia"),
        r#"
using dep::api;

pub fn main_value() i32 { 31 }

fn main() i32 {
    api::answer()
}
"#,
    );
    write(&dep_root, "pub module api; pub fn main_value() i32 { 7 }");
    fs::create_dir_all(root.join("dep")).expect("create dep dir");
    write(
        &root.join("dep/api.nia"),
        r#"
using entry;

pub fn answer() i32 {
    entry::main_value()
}
"#,
    );
    let mut module_map = ModuleMap::new();
    module_map.insert("dep", SourcePath::new(dep_root.to_string_lossy()));

    let program = check_program_with_map(
        root.join("main.nia").to_string_lossy().into_owned(),
        module_map,
    );

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn checks_each_loaded_module() {
    let root = temp_dir("checks_each_loaded_module");
    write(
        &root.join("main.nia"),
        r#"module math;
using entry::math; fn main() i32 { 0 }"#,
    );
    write(&root.join("math.nia"), r#"fn bad() i32 { true }"#);

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_eq!(modules_under(&program.modules, &root), 2);
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("function body"))
    );
}

#[test]
fn std_io_file_writer_is_created_from_process_io_capability() {
    let root = temp_dir("std_io_file_writer_is_created_from_process_io_capability");
    write(
        &root.join("main.nia"),
        r#"
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let mut buffer: [0]u8 = [];
    let mut stdout = io::FileWriter::stdout(init.io(), &mut buffer[..]);
    if !ok = stdout.write_all(&b"nia\n") {
        _ = ok;
    } or error! {
        return (1 as process::ExitCode)!;
    }
    !{}
}
"#,
    );

    let program = check_freestanding_executable_with_options(
        root.join("main.nia").to_string_lossy().into_owned(),
        crate::NiaOptimizationLevel::default(),
    );
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_io_buffered_file_writer_flushes_explicitly_through_process_io() {
    let root = temp_dir("std_io_buffered_file_writer_flushes_explicitly_through_process_io");
    write(
        &root.join("main.nia"),
        r#"
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let mut buffer: [64]u8 = [0; 64];
    let mut raw_buffer: [0]u8 = [];
    let mut raw = io::FileWriter::stdout(init.io(), &mut raw_buffer[..]);
    let mut stdout = io::BufferedWriter[io::FileWriter]::init(&mut raw, &mut buffer[..]);
    if !ok = stdout.write_all(&b"nia\n") {
        _ = ok;
    } or error! {
        return (1 as process::ExitCode)!;
    }
    if !ok = stdout.flush() {
        _ = ok;
    } or error! {
        return (2 as process::ExitCode)!;
    }
    !{}
}
"#,
    );

    let program = check_freestanding_executable_with_options(
        root.join("main.nia").to_string_lossy().into_owned(),
        crate::NiaOptimizationLevel::default(),
    );
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_fs_file_is_not_a_writer_without_process_io_capability() {
    let root = temp_dir("std_fs_file_is_not_a_writer_without_process_io_capability");
    write(
        &root.join("main.nia"),
        r#"
using std::fs;
using std::process;

fn reject_file_writer(file: fs::File) process::ExitCode!void {
    if !ok = file.write_all(&b"nia\n") {
        _ = ok;
    } or error! {
        return (1 as process::ExitCode)!;
    }
    !{}
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
    );

    let program = check_freestanding_executable_with_options(
        root.join("main.nia").to_string_lossy().into_owned(),
        crate::NiaOptimizationLevel::default(),
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("unknown struct field `write_all`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn std_io_file_reader_is_created_from_process_io_capability() {
    let root = temp_dir("std_io_file_reader_is_created_from_process_io_capability");
    write(
        &root.join("main.nia"),
        r#"
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let mut buffer: [64]u8 = [0; 64];
    let mut reader = io::FileReader::stdin(init.io(), &mut buffer[..]);
    let mut bytes: [1]u8 = [0];
    if !ok = reader.read(&mut bytes[..]) {
        _ = ok;
    } or error! {
        return (1 as process::ExitCode)!;
    }
    !{}
}
"#,
    );

    let program = check_freestanding_executable_with_options(
        root.join("main.nia").to_string_lossy().into_owned(),
        crate::NiaOptimizationLevel::default(),
    );
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_blocking_io_coerces_to_io_trait_object_with_error_binding() {
    let root = temp_dir("std_blocking_io_coerces_to_io_trait_object_with_error_binding");
    write(
        &root.join("main.nia"),
        r#"
using std::io;
using std::os;
using std::process;

fn main(argc: usize, argv: &&u8, envp: &&u8) void {
    let mut backend = io::BlockingIo::init();
    let init = process::Init::init(argc, argv, envp, &mut backend);
    let object: &mut io::Io[Error = os::Error] = init.io();
    _ = object;
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn using_cycles_do_not_load_or_cycle_modules() {
    let root = temp_dir("using_cycles_do_not_load_or_cycle_modules");
    write(
        &root.join("main.nia"),
        r#"module b;
module a;
using entry::a; fn main() {}"#,
    );
    write(&root.join("a.nia"), r#"using entry::b;"#);
    write(&root.join("b.nia"), r#"using entry::a;"#);

    let program = load_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_module_exposes_target_comptime_values() {
    let root = temp_dir("builtin_module_exposes_target_comptime_values");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::target;

fn main() usize {
    if target::pointer_width == 64usize or target::pointer_width == 32usize {
        target::os.len()
    } else {
        0usize
    }
}
"#,
    );

    let checked = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(
        checked
            .modules
            .iter()
            .any(|module| module.path.as_str().ends_with("lib/std/builtin/target.nia"))
    );
}

#[test]
fn child_module_can_call_public_entry_items_through_entry_namespace() {
    let root = temp_dir("child_module_can_call_public_entry_items_through_entry_namespace");
    write(
        &root.join("main.nia"),
        r#"
module runtime;
using entry::runtime;

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
using entry;

pub fn call_app_main() i32 {
    entry::app_main()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn freestanding_start_can_call_public_entry_main() {
    let root = temp_dir("freestanding_start_can_call_public_entry_main");
    write(
        &root.join("main.nia"),
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
    );

    let program = check_freestanding_executable_with_map_and_options(
        root.join("main.nia").to_string_lossy().into_owned(),
        nia_imports::ModuleMap::default(),
        nia_opt::NiaOptimizationLevel::default(),
    );
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn resolves_imported_extension_method_function_pointers() {
    let root = temp_dir("resolves_imported_extension_method_function_pointers");
    write(
        &root.join("main.nia"),
        r#"
module math;
using entry::math;

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
fn complex_using_graph_does_not_create_load_cycle() {
    let root = temp_dir("complex_using_graph_does_not_create_load_cycle");
    write(
        &root.join("main.nia"),
        r#"
module ops;
module math;
module helpers;
module core;
module app;
using entry::core;
using entry::app;

fn main(p: core::Point) i32 {
    app::score(p)
}
"#,
    );
    write(
        &root.join("core.nia"),
        r#"
using entry::ops;

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
        &root.join("app.nia"),
        r#"
using entry::core;
using entry::math;
using entry::ops;

pub fn score(p: core::Point) i32 {
    p.len2() + math::via_helpers() + ops::from_cycle()
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
using entry::helpers;

pub fn via_helpers() i32 {
    helpers::call_core()
}
"#,
    );
    write(
        &root.join("helpers.nia"),
        r#"
using entry::core;
using entry::ops;

pub fn call_core() i32 {
    core::base() + ops::from_cycle()
}
"#,
    );
    write(
        &root.join("ops.nia"),
        r#"
using entry::core;
using entry::helpers;

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
module math;
using entry::math as math_a;
using entry::math as math_b;
fn main() {}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"fn add(a: i32, b: i32) i32 { a + b }"#,
    );

    let program = load_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert_eq!(modules_under(&program.modules, &root), 2);
}

#[test]
fn resolves_qualified_imported_type_paths() {
    let root = temp_dir("resolves_qualified_imported_type_paths");
    write(
        &root.join("main.nia"),
        r#"
module math;
using entry::math;
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
module math;
using entry::math;
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
module math;
using entry::math;

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
module math;
module point_ext;
using entry::math;
using entry::point_ext;

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
using entry::math;

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
fn imports_public_extension_associated_comptime_values() {
    let root = temp_dir("imports_public_extension_associated_comptime_values");
    write(
        &root.join("main.nia"),
        r#"
module math;
using entry::math;

fn main() usize {
    math::Marker::LIMIT
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
pub struct Marker {}

extend Marker {
    pub comptime LIMIT: usize = 123usize;
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imports_public_extension_associated_comptime_values_with_builtin_initializer() {
    let root =
        temp_dir("imports_public_extension_associated_comptime_values_with_builtin_initializer");
    write(
        &root.join("main.nia"),
        r#"
module math;
using entry::math;

fn main() usize {
    math::Marker::LIMIT
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
pub struct Marker {}

extend Marker {
    pub comptime LIMIT: usize = if 64usize == 64 {
        18446744073709551615usize
    } else {
        4294967295usize
    };
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_math_usize_max_remains_available_with_array_list_imported() {
    let root = temp_dir("std_math_usize_max_remains_available_with_array_list_imported");
    write(
        &root.join("main.nia"),
        r#"
using std::collections;
using std::math;

fn main() usize {
    usize::MAX
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_math_usize_max_can_be_compared() {
    let root = temp_dir("std_math_usize_max_can_be_compared");
    write(
        &root.join("main.nia"),
        r#"
using std::math;

fn main() bool {
    1usize != usize::MAX
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_facade_range_iterates_half_open_usize_ranges() {
    let root = temp_dir("std_facade_range_iterates_half_open_usize_ranges");
    write(
        &root.join("main.nia"),
        r#"
using std;
using std::*;

fn main() usize {
    let mut total = 0usize;
    for i in 0usize..4usize {
        total += i;
    }
    total
}
"#,
    );

    let program = check_entry_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_facade_for_in_iterates_range_literals() {
    let root = temp_dir("std_facade_for_in_iterates_range_literals");
    write(
        &root.join("main.nia"),
        r#"
using std;
using std::*;

fn main() usize {
    let mut total = 0usize;
    let mut naked_count = 0usize;
    for i in 0..10 {
        naked_count += 1usize;
    }
    total += naked_count;
    for i in 0usize..4usize {
        total += i;
    }
    for i in 2usize..=4usize {
        total += i;
    }
    let mut count = 0usize;
    for i in 5usize.. {
        total += i;
        count += 1usize;
        if count == 3usize {
            break;
        }
    }
    total
}
"#,
    );

    let program = check_entry_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_facade_iterator_adaptors_work_on_range_literals() {
    let root = temp_dir("std_facade_iterator_adaptors_work_on_range_literals");
    write(
        &root.join("main.nia"),
        r#"
using std;
using std::*;

fn main() usize {
    let mut total = 0usize;
    for i in (0usize..10usize).iter().take(3usize) {
        total += i;
    }
    for i in (1usize..=3usize).iter().rev() {
        total = total * 10usize + i;
    }
    total + (20usize..25usize).iter().count()
}
"#,
    );

    let program = check_entry_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_facade_range_iterates_half_open_i64_ranges_with_expected_bound_type() {
    let root = temp_dir("std_facade_range_iterates_half_open_i64_ranges_with_expected_bound_type");
    write(
        &root.join("main.nia"),
        r#"
using std;
using std::*;

fn main() i64 {
    let mut total = 0i64;
    for i in 1i64..4i64 {
        total += i;
    }
    total
}
"#,
    );

    let program = check_entry_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_facade_range_iterates_inclusive_i32_ranges() {
    let root = temp_dir("std_facade_range_iterates_inclusive_i32_ranges");
    write(
        &root.join("main.nia"),
        r#"
using std;
using std::*;

fn main() i32 {
    let mut total = 0i32;
    for i in 2i32..=4i32 {
        total += i;
    }
    total
}
"#,
    );

    let program = check_entry_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_facade_range_iterates_inclusive_and_from_usize_ranges() {
    let root = temp_dir("std_facade_range_iterates_inclusive_and_from_usize_ranges");
    write(
        &root.join("main.nia"),
        r#"
using std;
using std::*;

fn main() usize {
    let mut total = 0usize;
    for i in 2usize..=4usize {
        total += i;
    }
    let mut count = 0usize;
    for i in 5usize.. {
        total += i;
        count += 1usize;
        if count == 3usize {
            break;
        }
    }
    total
}
"#,
    );

    let program = check_entry_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_facade_exposes_iter_range_types_under_iter_namespace() {
    let root = temp_dir("std_facade_exposes_iter_range_types_under_iter_namespace");
    write(
        &root.join("main.nia"),
        r#"
using std;
using std::*;

fn main() usize {
    let mut iter: std::iter::Range[usize] = (1usize..3usize).iter();
    let mut total = 0usize;
    for i in iter {
        total += i;
    }
    total
}
"#,
    );

    let program = check_entry_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_facade_does_not_reexport_range_module_namespace() {
    let root = temp_dir("std_facade_does_not_reexport_range_module_namespace");
    write(
        &root.join("main.nia"),
        r#"
using std;

fn main() void {
    let mut iter: std::range::Range[usize] = (1usize..3usize).iter();
    _ = iter;
}
"#,
    );

    let program = check_entry_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("unknown namespace `range`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn std_facade_exposes_array_list_type_directly() {
    let root = temp_dir("std_facade_exposes_array_list_type_directly");
    write(
        &root.join("main.nia"),
        r#"
using std;

fn main() usize {
    let mut list = std::ArrayList[i32]::init();
    list.len()
}
"#,
    );

    let program = check_entry_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_facade_type_can_be_imported_directly() {
    let root = temp_dir("std_facade_type_can_be_imported_directly");
    write(
        &root.join("main.nia"),
        r#"
using std::CStringView;

fn main() void {
    if ?text = CStringView::from_bytes(&b"nia\0") {
        _ = text;
    } or null {}
}
"#,
    );

    let program = check_entry_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_facade_type_can_be_named_by_package_root_path() {
    let root = temp_dir("std_facade_type_can_be_named_by_package_root_path");
    write(
        &root.join("main.nia"),
        r#"
fn main() void {
    if ?text = std::CStringView::from_bytes(&b"nia\0") {
        _ = text;
    } or null {}
}
"#,
    );

    let program = check_entry_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_type_alias_can_be_used_as_associated_call_prefix() {
    let root = temp_dir("imported_type_alias_can_be_used_as_associated_call_prefix");
    write(
        &root.join("main.nia"),
        r#"
using std::collections::ArrayList;

fn main() usize {
    let mut list = ArrayList[i32]::init();
    list.len()
}
"#,
    );

    let program = check_entry_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_facade_does_not_reexport_standard_library_module_namespaces() {
    let root = temp_dir("std_facade_does_not_reexport_standard_library_module_namespaces");
    write(
        &root.join("main.nia"),
        r#"
using std;

fn main() usize {
    let writer = io::DiscardingWriter::init();
    writer.len()
}
"#,
    );

    let program = check_entry_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("qualified access is not a value expression")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn std_facade_does_not_reexport_array_list_module_namespace() {
    let root = temp_dir("std_facade_does_not_reexport_array_list_module_namespace");
    write(
        &root.join("main.nia"),
        r#"
using std;

fn main() usize {
    let mut list = std::array_list::ArrayList[i32]::init();
    list.len()
}
"#,
    );

    let program = check_entry_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("unknown namespace `array_list`")
            || diagnostic
                .diagnostic
                .summary
                .contains("unknown value `array_list`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn std_root_facade_does_not_reexport_collection_detail_types() {
    let root = temp_dir("std_root_facade_does_not_reexport_collection_detail_types");
    write(
        &root.join("main.nia"),
        r#"
using std;

struct Key {}

extend Key : std::HashMapContext[i32] {
    fn hash(&self, key: &i32) u64 {
        _ = key;
        0u64
    }

    fn equal(&self, left: &i32, right: &i32) bool {
        left.* == right.*
    }
}

fn main() void {}
"#,
    );

    let program = check_entry_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("unknown type `HashMapContext`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn std_os_does_not_expose_platform_implementation_modules() {
    let root = temp_dir("std_os_does_not_expose_platform_implementation_modules");
    write(
        &root.join("main.nia"),
        r#"
using std::os::linux::stat;

fn main() void {}
"#,
    );

    let program = check_entry_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("unknown namespace `linux`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn std_facades_do_not_expose_package_private_implementation_modules() {
    let root = temp_dir("std_facades_do_not_expose_package_private_implementation_modules");
    let private_paths = [
        "std::io::traits",
        "std::collections::array_list",
        "std::collections::hash_map::map",
        "std::fs::convert",
        "std::hash::impls",
    ];
    let mut source = String::new();
    for path in private_paths {
        source.push_str(&format!("using {path};\n"));
    }
    source.push_str("\nfn main() void {}\n");
    write(&root.join("main.nia"), &source);

    let program = check_entry_program(root.join("main.nia").to_string_lossy().into_owned());
    let hidden_namespace_diagnostics = program
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.diagnostic.summary.contains("unknown namespace")
                || diagnostic
                    .diagnostic
                    .summary
                    .contains("could not be resolved")
        })
        .count();
    assert!(
        hidden_namespace_diagnostics >= private_paths.len(),
        "private std facade namespaces should not be visible: {:?}",
        program.diagnostics
    );
}

#[test]
fn std_hash_facade_exposes_builtin_hash_impls() {
    let root = temp_dir("std_hash_facade_exposes_builtin_hash_impls");
    write(
        &root.join("main.nia"),
        r#"
using std::hash;

fn main() u64 {
    let mut hasher = hash::Wyhash::init(1u64);
    42usize.hash(&mut hasher);
    true.hash(&mut hasher);
    hasher.finish()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_hash_facade_enables_default_hash_map_key_impls() {
    let root = temp_dir("std_hash_facade_enables_default_hash_map_key_impls");
    write(
        &root.join("main.nia"),
        r#"
using std;
using std::collections;
using std::hash;
using std::mem;

fn main() mem::Error!usize {
    let mut buffer: [4096]u8 = [0; 4096];
    let mut allocator = mem::FixedBufferAllocator::init(&mut buffer[..]);
    let mut map = collections::HashMapWithContext[i32, i32, collections::DefaultHashMapContext]::init_seed(1u64);
    defer map.deinit(&mut allocator).?;
    _ = map.put(&mut allocator, 1, 2).?;
    !map.len()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn hidden_trait_impl_does_not_satisfy_generic_obligation() {
    let root = temp_dir("hidden_trait_impl_does_not_satisfy_generic_obligation");
    write(
        &root.join("main.nia"),
        r#"
module api;
module types;
module impls;

using entry::api;
using entry::types;

fn need[T](value: T) i32
where T: api::Show
{
    value.show()
}

fn main(value: types::Box) i32 {
    need[types::Box](value)
}
"#,
    );
    write(
        &root.join("api.nia"),
        r#"
pub trait Show {
    fn show(&self) i32;
}
"#,
    );
    write(&root.join("types.nia"), r#"pub struct Box { value: i32 }"#);
    write(
        &root.join("impls.nia"),
        r#"
using entry::api;
using entry::types;

extend types::Box : api::Show {
    fn show(&self) i32 {
        self.value
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("trait bound not satisfied: Box: Show")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn imported_trait_impl_satisfies_generic_obligation() {
    let root = temp_dir("imported_trait_impl_satisfies_generic_obligation");
    write(
        &root.join("main.nia"),
        r#"
module api;
module types;
module impls;

using entry::api;
using entry::types;
using entry::impls;

fn need[T](value: T) i32
where T: api::Show
{
    value.show()
}

fn main(value: types::Box) i32 {
    need[types::Box](value)
}
"#,
    );
    write(
        &root.join("api.nia"),
        r#"
pub trait Show {
    fn show(&self) i32;
}
"#,
    );
    write(&root.join("types.nia"), r#"pub struct Box { value: i32 }"#);
    write(
        &root.join("impls.nia"),
        r#"
using entry::api;
using entry::types;

extend types::Box : api::Show {
    pub fn show(&self) i32 {
        self.value
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_root_facade_enables_default_hash_map_key_impls() {
    let root = temp_dir("std_root_facade_enables_default_hash_map_key_impls");
    write(
        &root.join("main.nia"),
        r#"
using std;
using std::collections;
using std::mem;

fn main() mem::Error!usize {
    let mut buffer: [4096]u8 = [0; 4096];
    let mut allocator = mem::FixedBufferAllocator::init(&mut buffer[..]);
    let mut map = collections::HashMapWithContext[i32, i32, collections::DefaultHashMapContext]::init_seed(1u64);
    defer map.deinit(&mut allocator).?;
    _ = map.put(&mut allocator, 1, 2).?;
    !map.len()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_package_private_implementation_module_is_not_resolved_from_root_package() {
    let root =
        temp_dir("std_package_private_implementation_module_is_not_resolved_from_root_package");
    write(
        &root.join("main.nia"),
        r#"
using std::io::traits;

fn main() void {}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("could not be resolved")
            || diagnostic.diagnostic.summary.contains("unknown namespace")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn std_start_injected_for_executables_is_not_public_api() {
    let root = temp_dir("std_start_injected_for_executables_is_not_public_api");
    write(
        &root.join("main.nia"),
        r#"
using std::process;
using std::start;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
    );

    let program = check_freestanding_executable_with_options(
        root.join("main.nia").to_string_lossy().into_owned(),
        crate::NiaOptimizationLevel::default(),
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("could not be resolved")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn reexported_module_and_value_with_same_name_resolve_by_context() {
    let root = temp_dir("reexported_module_and_value_with_same_name_resolve_by_context");
    write(
        &root.join("main.nia"),
        r#"
module range;
module facade;
using entry::facade;

fn main() usize {
    let mut iter: facade::range::Range[usize] = facade::range(1usize..4usize);
    let mut total = 0usize;
    for i in iter {
        total += i;
    }
    total
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
using entry::range;

pub using range;
pub using range::range;
"#,
    );
    write(
        &root.join("range.nia"),
        r#"
pub trait Step {
    fn next(self) ?Self;
}

pub struct Range[T] {
    current: T,
    end: T,
}

extend[T] Range[T] {
    pub fn init(current: T, end: T) Range[T] {
        { current: current, end: end }
    }
}

extend[T] Range[T] : Iterator
where T: Step + Ord[T]
{
    type Item = T;

    pub fn next(&mut self) ?T {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current = if ?next = self.current.next() {
                next
            } or null {
                self.end
            };
            ?value
        }
    }
}

pub fn range[T](bounds: T..T) Range[T]
where T: Step + Ord[T]
{
    Range[T]::init(bounds.start(), bounds.end())
}

extend usize : Step {
    pub fn next(self) ?usize {
        ?(self + 1usize)
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn primitive_integer_limits_are_builtin_associated_values() {
    let root = temp_dir("primitive_integer_limits_are_builtin_associated_values");
    write(
        &root.join("main.nia"),
        r#"
fn main() bool {
    usize::MIN == 0usize
        and usize::MAX > 0usize
        and isize::MIN < 0isize
        and i32::MAX == 2147483647i32
        and i32::MIN < 0i32
        and u128::MAX > 0u128
        and i128::MIN < 0i128
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn lowercase_primitive_integer_limits_are_not_builtin_associated_values() {
    let root = temp_dir("lowercase_primitive_integer_limits_are_not_builtin_associated_values");
    write(
        &root.join("main.nia"),
        r#"
fn main() usize {
    usize::max
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("qualified access is not a value expression")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn u128_max_is_a_builtin_associated_value() {
    let root = temp_dir("u128_max_is_a_builtin_associated_value");
    write(
        &root.join("main.nia"),
        r#"
fn main() u128 {
    u128::MAX
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
module point_ext;
module math;
module facade;
using entry::math;
using entry::facade;

fn main(p: math::Point) i32 {
    p.len2()
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"pub struct Point { x: i32, y: i32 }"#,
    );
    write(&root.join("facade.nia"), r#"using entry::point_ext;"#);
    write(
        &root.join("point_ext.nia"),
        r#"
using entry::math;

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
fn facade_reexported_type_exposes_public_inherent_extension_methods() {
    let root = temp_dir("facade_reexported_type_exposes_public_inherent_extension_methods");
    write(
        &root.join("main.nia"),
        r#"
module facade;
module impls;
module types;
using entry::facade;

fn main(p: facade::Point) i32 {
    p.len2()
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"pub using entry::types::Point;"#,
    );
    write(
        &root.join("types.nia"),
        r#"pub struct Point { x: i32, y: i32 }"#,
    );
    write(
        &root.join("impls.nia"),
        r#"
using entry::types;

extend types::Point {
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
fn executable_facade_reexport_does_not_body_check_unused_reexport_extension_providers() {
    let root = temp_dir(
        "executable_facade_reexport_does_not_body_check_unused_reexport_extension_providers",
    );
    write(
        &root.join("main.nia"),
        r#"
module facade;
module impls;
module types;
using entry::facade;

fn consume(used: facade::Used) i32 {
    _ = used;
    1
}

fn main() i32 {
    consume(facade::Used {})
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
pub using entry::types::{Used, Unused};
"#,
    );
    write(
        &root.join("types.nia"),
        r#"
pub struct Used {}
pub struct Unused {}
"#,
    );
    write(
        &root.join("impls.nia"),
        r#"
using entry::types;

extend types::Unused {
    pub fn expensive_or_invalid(&self) i32 {
        missing_symbol
    }
}
"#,
    );

    let program = check_entry_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(
        program
            .modules
            .iter()
            .all(|module| { !module.path.as_str().ends_with("impls.nia") }),
        "unused facade re-export extension provider should not enter executable checking: {:?}",
        program
            .modules
            .iter()
            .map(|module| module.path.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn facade_reexported_generic_alias_exposes_target_public_inherent_extension_methods() {
    let root = temp_dir(
        "facade_reexported_generic_alias_exposes_target_public_inherent_extension_methods",
    );
    write(
        &root.join("main.nia"),
        r#"
module facade;
module impls;
module types;
using entry::facade;

fn main() usize {
    let mut bag = facade::Bag[i32]::init();
    bag.len()
}
"#,
    );
    write(&root.join("facade.nia"), r#"pub using entry::types::Bag;"#);
    write(
        &root.join("types.nia"),
        r#"
pub struct RawBag[T] {
    len: usize,
}

pub type Bag[T] = RawBag[T];
"#,
    );
    write(
        &root.join("impls.nia"),
        r#"
using entry::types;

extend[T] types::RawBag[T] {
    pub fn init() types::RawBag[T] {
        { len: 0usize }
    }

    pub fn len(&self) usize {
        self.len
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
module ptr;
using entry::ptr;

extern fn read_readonly() &u8;

fn main(mut_ptr: &mut u8) i32 {
    let mut readonly_ptr = read_readonly();
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
fn imported_enum_extension_method_function_pointer_uses_nominal_prefix() {
    let root = temp_dir("imported_enum_extension_method_function_pointer_uses_nominal_prefix");
    write(
        &root.join("main.nia"),
        r#"
module errors;
using entry::errors;

fn main() i32 {
    let code: &fn(errors::Error) i32 = &errors::Error::code;
    code(errors::Error::Invalid)
}
"#,
    );
    write(
        &root.join("errors.nia"),
        r#"
pub enum Error: i32 {
    Invalid = 22,
    _,
}

extend Error {
    pub fn code(self) i32 {
        self as i32
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
module containers;
using entry::containers;

fn main(boxed: containers::Box[bool]) i32 {
    boxed.tag()
}
"#,
    );
    write(
        &root.join("containers.nia"),
        r#"
pub trait Marker {}

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
module containers;
module impls;
using entry::containers;
using entry::impls;

fn need[T](value: T) i32
where T: containers::Marker
{
    value.mark()
}

fn main(value: i32) i32 {
    need[i32](value)
}
"#,
    );
    write(
        &root.join("containers.nia"),
        r#"
pub trait Marker {
    fn mark(self) i32;
}
"#,
    );
    write(
        &root.join("impls.nia"),
        r#"
using entry::containers;

extend i32 : containers::Marker {
    fn mark(self) i32 {
        self
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
module ptr;
module share;
using entry::share;

extern fn read_readonly() &u8;

fn main(mut_ptr: &mut u8) i32 {
    let mut readonly_ptr = read_readonly();
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
    write(&root.join("share.nia"), r#"using entry::ptr;"#);
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
module ptr;
module share;
using entry::share;

extern fn read_readonly() &u8;

fn main(mut_ptr: &mut u8) i32 {
    let mut readonly_ptr = read_readonly();
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
    write(&root.join("share.nia"), r#"using entry::ptr;"#);
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

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
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
module ptr;
using entry::ptr;

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
type RawPtr[T] = &T;

extend[T] RawPtr[T] {
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
module point_ext;
module math;
module facade;
using entry::math;
using entry::facade;

fn main(p: math::Point) i32 {
    p.len2()
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"pub struct Point { x: i32, y: i32 }"#,
    );
    write(&root.join("facade.nia"), r#"using entry::point_ext;"#);
    write(
        &root.join("point_ext.nia"),
        r#"
using entry::math;

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
module math;
using entry::math;
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

#[test]
fn self_import_loads_child_module_by_stem_path() {
    let root = temp_dir("self_import_loads_child_module_by_stem_path");
    write(
        &root.join("main.nia"),
        r#"
module hash_map;
using entry::hash_map;

fn main() i32 {
    hash_map::score()
}
"#,
    );
    write(
        &root.join("hash_map.nia"),
        r#"
module probe;
using self::probe;

pub fn score() i32 {
    probe::value()
}
"#,
    );
    std::fs::create_dir_all(root.join("hash_map")).expect("create child module dir");
    write(
        &root.join("hash_map/probe.nia"),
        r#"
pub(super) fn value() i32 {
    7
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn super_import_loads_sibling_under_parent_module_stem() {
    let root = temp_dir("super_import_loads_sibling_under_parent_module_stem");
    write(
        &root.join("main.nia"),
        r#"
module hash_map;
using entry::hash_map;

fn main() i32 {
    hash_map::score()
}
"#,
    );
    write(
        &root.join("hash_map.nia"),
        r#"
module probe;
module iter;
using self::iter;

pub fn score() i32 {
    iter::score()
}
"#,
    );
    std::fs::create_dir_all(root.join("hash_map")).expect("create child module dir");
    write(
        &root.join("hash_map/iter.nia"),
        r#"
using super::probe;

pub(super) fn score() i32 {
    probe::value()
}
"#,
    );
    write(
        &root.join("hash_map/probe.nia"),
        r#"
pub(super) fn value() i32 {
    11
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn pub_super_items_are_hidden_outside_parent_subtree() {
    let root = temp_dir("pub_super_items_are_hidden_outside_parent_subtree");
    write(
        &root.join("main.nia"),
        r#"
module hash_map;
using entry::hash_map::probe;

fn main() i32 {
    probe::value()
}
"#,
    );
    std::fs::create_dir_all(root.join("hash_map")).expect("create child module dir");
    write(&root.join("hash_map.nia"), "pub module probe;");
    write(
        &root.join("hash_map/probe.nia"),
        r#"
pub(super) fn value() i32 {
    13
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("private")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn using_can_import_pub_super_items_visible_from_parent_subtree() {
    let root = temp_dir("using_can_import_pub_super_items_visible_from_parent_subtree");
    write(
        &root.join("main.nia"),
        r#"
module command;
using entry::command;

fn main() i32 {
    command::score()
}
"#,
    );
    write(
        &root.join("command.nia"),
        r#"
module types;
pub module cli;
using self::cli;

pub fn score() i32 {
    cli::score()
}
"#,
    );
    std::fs::create_dir_all(root.join("command")).expect("create command dir");
    write(
        &root.join("command/types.nia"),
        r#"
pub(super) struct ElfHeader {
    value: i32,
}

pub(super) fn make_header() ElfHeader {
    { value: 42 }
}

pub(super) fn header_score(header: ElfHeader) i32 {
    header.value
}
"#,
    );
    write(
        &root.join("command/cli.nia"),
        r#"
using super::types::{ElfHeader, make_header, header_score};

pub(super) fn score() i32 {
    let header: ElfHeader = make_header();
    header_score(header)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn using_can_import_pub_package_items_inside_package() {
    let root = temp_dir("using_can_import_pub_package_items_inside_package");
    let dep_root = root.join("dep.nia");
    write(
        &root.join("main.nia"),
        r#"
using dep::api;

fn main() i32 {
    api::inside()
}
"#,
    );
    write(&dep_root, "pub module api; pub module types;");
    std::fs::create_dir_all(root.join("dep")).expect("create dep dir");
    write(
        &root.join("dep/types.nia"),
        r#"
pub(pkg) struct Token {
    value: i32,
}

pub(pkg) fn make_token() Token {
    { value: 31 }
}

pub(pkg) fn token_value(token: Token) i32 {
    token.value
}
"#,
    );
    write(
        &root.join("dep/api.nia"),
        r#"
using pkg::types::{Token, make_token, token_value};

pub fn inside() i32 {
    let token: Token = make_token();
    token_value(token)
}
"#,
    );
    let mut module_map = ModuleMap::new();
    module_map.insert("dep", SourcePath::new(dep_root.to_string_lossy()));

    let program = check_program_with_map(
        root.join("main.nia").to_string_lossy().into_owned(),
        module_map,
    );
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn using_super_after_first_segment_reports_specific_diagnostic() {
    let root = temp_dir("using_super_after_first_segment_reports_specific_diagnostic");
    write(
        &root.join("main.nia"),
        r#"
module command;
module output;
using entry::command::cli;

fn main() void {}
"#,
    );
    write(&root.join("output.nia"), "pub fn write() void {}");
    write(&root.join("command.nia"), "pub module cli;");
    std::fs::create_dir_all(root.join("command")).expect("create command dir");
    write(
        &root.join("command/cli.nia"),
        r#"
using super::super::output;

pub fn run() void {}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("`super` can only be used as the first path segment")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn failed_explicit_using_suppresses_dependent_name_cascades() {
    let root = temp_dir("failed_explicit_using_suppresses_dependent_name_cascades");
    write(
        &root.join("main.nia"),
        r#"
module api;
using entry::api::{MissingType, missing_value};

fn main() void {
    let value: MissingType = missing_value();
    _ = value;
}
"#,
    );
    write(&root.join("api.nia"), "");

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("could not be resolved")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        !program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("unknown type `MissingType`")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        !program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary == "name is unresolved"),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn pub_package_extension_methods_are_visible_only_inside_package() {
    let root = temp_dir("pub_package_extension_methods_are_visible_only_inside_package");
    let dep_root = root.join("dep.nia");
    write(
        &root.join("main.nia"),
        r#"
using dep::api;

fn main() i32 {
    api::inside()
}
"#,
    );
    write(&dep_root, "pub module api; pub module model;");
    std::fs::create_dir_all(root.join("dep")).expect("create dep dir");
    write(
        &root.join("dep/model.nia"),
        r#"
pub struct Value {
    data: i32,
}

extend Value {
    pub fn init(data: i32) Value {
        { data: data }
    }

    pub(pkg) fn package_score(&self) i32 {
        self.data + 1
    }
}
"#,
    );
    write(
        &root.join("dep/api.nia"),
        r#"
using pkg::model;

pub fn inside() i32 {
    model::Value::init(41).package_score()
}
"#,
    );
    let mut module_map = ModuleMap::new();
    module_map.insert("dep", SourcePath::new(dep_root.to_string_lossy()));

    let program = check_program_with_map(
        root.join("main.nia").to_string_lossy().into_owned(),
        module_map,
    );
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);

    write(
        &root.join("main.nia"),
        r#"
using dep::model;

fn main() i32 {
    model::Value::init(41).package_score()
}
"#,
    );
    let mut module_map = ModuleMap::new();
    module_map.insert("dep", SourcePath::new(dep_root.to_string_lossy()));
    let program = check_program_with_map(
        root.join("main.nia").to_string_lossy().into_owned(),
        module_map,
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("unknown struct field")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn pub_package_extension_associated_values_are_visible_only_inside_package() {
    let root = temp_dir("pub_package_extension_associated_values_are_visible_only_inside_package");
    let dep_root = root.join("dep.nia");
    write(
        &root.join("main.nia"),
        r#"
using dep::api;

fn main() usize {
    api::inside()
}
"#,
    );
    write(&dep_root, "pub module api; pub module model;");
    std::fs::create_dir_all(root.join("dep")).expect("create dep dir");
    write(
        &root.join("dep/model.nia"),
        r#"
pub struct Marker {}

extend Marker {
    pub(pkg) comptime LIMIT: usize = 123usize;
}
"#,
    );
    write(
        &root.join("dep/api.nia"),
        r#"
using pkg::model;

pub fn inside() usize {
    model::Marker::LIMIT
}
"#,
    );
    let mut module_map = ModuleMap::new();
    module_map.insert("dep", SourcePath::new(dep_root.to_string_lossy()));

    let program = check_program_with_map(
        root.join("main.nia").to_string_lossy().into_owned(),
        module_map,
    );
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);

    write(
        &root.join("main.nia"),
        r#"
using dep::model;

fn main() usize {
    model::Marker::LIMIT
}
"#,
    );
    let mut module_map = ModuleMap::new();
    module_map.insert("dep", SourcePath::new(dep_root.to_string_lossy()));
    let program = check_program_with_map(
        root.join("main.nia").to_string_lossy().into_owned(),
        module_map,
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("qualified access is not a value expression")),
        "{:?}",
        program.diagnostics
    );
}
