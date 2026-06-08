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
fn std_io_file_writer_is_created_from_process_io_capability() {
    let root = temp_dir("std_io_file_writer_is_created_from_process_io_capability");
    write(
        &root.join("main.nia"),
        r#"
import std;

using std::{io, process};

pub fn main(init: process::Init) process::ExitCode!void {
    var buffer: [0]u8 = [];
    var stdout = io::FileWriter::stdout(init.io(), &mut buffer[..]);
    switch stdout.write_all(b"nia\n") {
        !ok => _ = ok,
        error! => return process::ExitCode::init(1)!,
    }
    !{}
}
"#,
    );

    let program = crate::check_freestanding_executable_with_options(
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
import std;

using std::{io, process};

pub fn main(init: process::Init) process::ExitCode!void {
    var buffer: [64]u8 = [0; 64];
    var raw_buffer: [0]u8 = [];
    var raw = io::FileWriter::stdout(init.io(), &mut raw_buffer[..]);
    var stdout = io::BufferedWriter[io::FileWriter]::init(&mut raw, &mut buffer[..]);
    switch stdout.write_all(b"nia\n") {
        !ok => _ = ok,
        error! => return process::ExitCode::init(1)!,
    }
    switch stdout.flush() {
        !ok => _ = ok,
        error! => return process::ExitCode::init(2)!,
    }
    !{}
}
"#,
    );

    let program = crate::check_freestanding_executable_with_options(
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
import std;

using std::{fs, process};

fn reject_file_writer(file: fs::File) process::ExitCode!void {
    switch file.write_all(b"nia\n") {
        !ok => _ = ok,
        error! => return process::ExitCode::init(1)!,
    }
    !{}
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
    );

    let program = crate::check_freestanding_executable_with_options(
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
import std;

using std::{io, process};

pub fn main(init: process::Init) process::ExitCode!void {
    var buffer: [64]u8 = [0; 64];
    var reader = io::FileReader::stdin(init.io(), &mut buffer[..]);
    var bytes: [1]u8 = [0];
    switch reader.read(&mut bytes[..]) {
        !ok => _ = ok,
        error! => return process::ExitCode::init(1)!,
    }
    !{}
}
"#,
    );

    let program = crate::check_freestanding_executable_with_options(
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
import std;

using std::{io, process};

fn main(argc: usize, argv: &&u8, envp: &&u8) void {
    var backend = io::BlockingIo::init();
    let init = process::Init::init(argc, argv, envp, &mut backend);
    let object: &mut io::Io[Error = std::os::Error] = init.io();
    _ = object;
}
"#,
    );

    let program = crate::check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
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
fn imports_public_extension_associated_comptime_values() {
    let root = temp_dir("imports_public_extension_associated_comptime_values");
    write(
        &root.join("main.nia"),
        r#"
import .math;

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
    pub comptime let LIMIT: usize = 123usize;
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
import .math;

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
    pub comptime let LIMIT: usize = if @builtin().target.pointer_width == 64 {
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
import std.array_list;
import std.math;

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
import std.math;

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
import std;

fn main() usize {
    var total = 0usize;
    for i in std::range(0usize..4usize) {
        total += i;
    }
    total
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_facade_range_iterates_half_open_i64_ranges_with_expected_bound_type() {
    let root = temp_dir("std_facade_range_iterates_half_open_i64_ranges_with_expected_bound_type");
    write(
        &root.join("main.nia"),
        r#"
import std;

fn main() i64 {
    var total = 0i64;
    for i in std::range[i64](1..4) {
        total += i;
    }
    total
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_facade_range_iterates_inclusive_i32_ranges() {
    let root = temp_dir("std_facade_range_iterates_inclusive_i32_ranges");
    write(
        &root.join("main.nia"),
        r#"
import std;

fn main() i32 {
    var total = 0i32;
    for i in std::inclusive[i32](2..=4) {
        total += i;
    }
    total
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_facade_range_iterates_inclusive_and_from_usize_ranges() {
    let root = temp_dir("std_facade_range_iterates_inclusive_and_from_usize_ranges");
    write(
        &root.join("main.nia"),
        r#"
import std;

fn main() usize {
    var total = 0usize;
    for i in std::inclusive(2usize..=4usize) {
        total += i;
    }
    var from = std::from(5usize..);
    var count = 0usize;
    for i in from {
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

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn std_facade_exposes_range_constructors() {
    let root = temp_dir("std_facade_exposes_range_constructors");
    write(
        &root.join("main.nia"),
        r#"
import std;

fn main() usize {
    var total = 0usize;
    for i in std::range(1usize..3usize) {
        total += i;
    }
    total
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn reexported_module_and_value_with_same_name_resolve_by_context() {
    let root = temp_dir("reexported_module_and_value_with_same_name_resolve_by_context");
    write(
        &root.join("main.nia"),
        r#"
import .facade;

fn main() usize {
    var iter: facade::range::Range[usize] = facade::range(1usize..4usize);
    var total = 0usize;
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
import .range;

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
            self.current = switch self.current.next() {
                ?next => next,
                null => self.end,
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
fn u128_max_is_not_a_truncated_builtin_associated_value() {
    let root = temp_dir("u128_max_is_not_a_truncated_builtin_associated_value");
    write(
        &root.join("main.nia"),
        r#"
fn main() u128 {
    u128::MAX
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
fn imported_enum_extension_method_function_pointer_uses_nominal_prefix() {
    let root = temp_dir("imported_enum_extension_method_function_pointer_uses_nominal_prefix");
    write(
        &root.join("main.nia"),
        r#"
import .errors;

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
