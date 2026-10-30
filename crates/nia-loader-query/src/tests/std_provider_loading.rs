use super::*;

#[test]
fn query_loader_loads_slice_order_provider_for_sorting_demand() {
    let root = temp_dir("query_loader_loads_slice_order_provider_for_sorting_demand");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::slice;

fun sort(values: &mut [i32]) () {
    values.sort();
}
"#,
    );

    let program =
        load_program_with_provider_demand(&main_path, ModuleMap::default(), Some("[i32]"), "sort");

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/slice.nia");
    assert_module_loaded(&program, "lib/std/slice/order.nia");
    let ordering = module_by_suffix(&program, "lib/std/slice/order.nia");
    assert!(ordering.semantic_selected, "{ordering:?}");
    assert!(ordering.process_used_paths, "{ordering:?}");
}

#[test]
fn query_loader_does_not_load_slice_order_provider_without_ordering_demand() {
    let root = temp_dir("query_loader_does_not_load_slice_order_provider_without_ordering_demand");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::slice;

fun main(values: &[i32]) ?&i32 {
    values.first()
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/slice.nia");
    assert_module_not_loaded(&program, "lib/std/slice/order.nia");
}

#[test]
fn query_loader_loads_len_trait_provider_on_explicit_demand() {
    let root = temp_dir("query_loader_loads_len_trait_provider_on_explicit_demand");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
fun main(values: &[u8]) usize {
    values.len()
}
"#,
    );

    let program = load_program_with_trait_provider_demand(
        &main_path,
        ModuleMap::default(),
        Some("[u8]"),
        "Len",
        &[],
    );

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/builtin.nia");
    assert_module_loaded(&program, "lib/std/builtin/place.nia");
    assert_module_not_loaded(&program, "lib/std/builtin/atomic.nia");
}

#[test]
fn query_loader_loads_len_impl_from_explicit_trait_import() {
    let root = temp_dir("query_loader_loads_len_impl_from_explicit_trait_import");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::builtin::Len;

struct Window {}
extend Window : Len {
    const fun len(&self) usize { 0usize }
}
fun main() () {}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/builtin.nia");
    assert_module_loaded(&program, "lib/std/builtin/place.nia");
    assert_module_not_loaded(&program, "lib/std/builtin/atomic.nia");
    assert_module_not_loaded(&program, "lib/std/iter/range.nia");
}

#[test]
fn query_loader_does_not_predeclare_len_without_an_import() {
    let root = temp_dir("query_loader_does_not_predeclare_len_without_an_import");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
struct Window {}
extend Window : Len {
    const fun len(&self) usize { 0usize }
}
fun main() () {}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_module_not_loaded(&program, "lib/std/builtin.nia");
}

#[test]
fn query_loader_does_not_load_len_provider_without_len_demand() {
    let root = temp_dir("query_loader_does_not_load_len_provider_without_len_demand");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
fun main() usize {
    1usize
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_module_not_loaded(&program, "lib/std/builtin.nia");
    assert_module_not_loaded(&program, "lib/std/builtin/place.nia");
}

#[test]
fn query_loader_loads_builtin_trait_provider_from_facade() {
    let root = temp_dir("query_loader_loads_builtin_trait_provider_from_facade");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std;

fun main() () {
    for _ in 1usize..4usize {}
}
"#,
    );

    let program = load_program_with_trait_provider_demand(
        &main_path,
        ModuleMap::default(),
        None,
        "Iterable",
        &[],
    );

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/iter.nia");
    assert_module_loaded(&program, "lib/std/iter/range.nia");
    assert_module_not_loaded(&program, "lib/std/process/command.nia");
}

#[test]
fn query_loader_loads_range_provider_for_range_methods_without_iter_import() {
    let root = temp_dir("query_loader_loads_range_provider_for_range_methods_without_iter_import");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
fun main() () {
    let range = (1..4).iter();
    _ = range;
}
"#,
    );

    let program = load_program_with_trait_provider_demand(
        &main_path,
        ModuleMap::default(),
        Some("Range"),
        "Iterator",
        &[],
    );

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/iter.nia");
    assert_module_loaded(&program, "lib/std/iter/range.nia");
    assert_module_not_loaded(&program, "lib/std/process/command.nia");
}

#[test]
fn query_loader_does_not_load_iterator_provider_without_builtin_trait_demand() {
    let root =
        temp_dir("query_loader_does_not_load_iterator_provider_without_builtin_trait_demand");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
fun main() () {
    let range = 1usize..4usize;
    _ = range;
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_module_not_loaded(&program, "lib/std/iter.nia");
    assert_module_not_loaded(&program, "lib/std/iter/range.nia");
}

#[test]
fn query_loader_loads_exit_code_conversion_provider_on_demand() {
    let root = temp_dir("query_loader_loads_exit_code_conversion_provider_on_demand");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::process;
using std::fs;
"#,
    );

    let program = load_program_with_trait_provider_demand(
        &main_path,
        ModuleMap::default(),
        Some("PathError"),
        "IntoError",
        &[Some("ExitCode")],
    );

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/process/exit_code.nia");
    assert_module_not_loaded(&program, "lib/std/process/command.nia");
}

#[test]
fn query_loader_loads_provider_through_explicit_facade_reexport() {
    let root = temp_dir("query_loader_loads_provider_through_explicit_facade_reexport");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::process::ExitCode;
using std::fs::PathError;

fun main(error: PathError) ExitCode!() {
    error!
}
"#,
    );

    let program = load_program_with_trait_provider_demand(
        &main_path,
        ModuleMap::default(),
        Some("PathError"),
        "IntoError",
        &[Some("ExitCode")],
    );

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/process/exit_code.nia");
    assert_module_not_loaded(&program, "lib/std/process/command.nia");
}

#[test]
fn query_loader_filters_trait_providers_by_nominal_type_argument() {
    let root = temp_dir("query_loader_filters_trait_providers_by_nominal_type_argument");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::collections;
using std::process;
"#,
    );

    let program = super::load_program_with_trait_provider_demand_at_module(
        &main_path,
        ModuleMap::default(),
        Some("Error"),
        "IntoError",
        &[Some("ExitCode")],
        vec![sym("process"), sym("types")],
    );

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/process/exit_code.nia");
    assert_module_not_loaded(&program, "lib/std/collections/hash_map/map.nia");
}

#[test]
fn query_loader_loads_iterator_provider_for_for_in_iterator_values() {
    let root = temp_dir("query_loader_loads_iterator_provider_for_for_in_iterator_values");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::process;

fun main(init: process::Init) () {
    for _ in init.env().iter() {}
}
"#,
    );

    let program = load_program_with_trait_provider_demand(
        &main_path,
        ModuleMap::default(),
        Some("EnvIter"),
        "Iterator",
        &[],
    );

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/process/env.nia");
}

#[test]
fn query_loader_keeps_trait_search_on_the_processed_collection_branch() {
    let root = temp_dir("query_loader_keeps_trait_search_on_the_processed_collection_branch");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::collections;

fun main(values: collections::ArrayList[i32]) usize {
    for value in values {
        _ = value;
    }
    values.len()
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/collections/array_list/list.nia");
    assert_module_not_loaded(&program, "lib/std/collections/hash_map.nia");
}

#[test]
fn query_loader_loads_facade_trait_impl_provider_for_used_trait_method() {
    let root = temp_dir("query_loader_loads_facade_trait_impl_provider_for_used_trait_method");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::hash;

fun main() u64 {
let mut hasher = hash::Wyhash::init(1u64);
42usize.hash(&mut hasher);
hasher.finish()
}
"#,
    );

    let program =
        load_program_with_provider_demand(&main_path, ModuleMap::default(), Some("usize"), "hash");

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/hash.nia");
    assert_module_loaded(&program, "lib/std/hash/impls.nia");
    assert_module_loaded(&program, "lib/std/hash/wyhash.nia");
}

#[test]
fn query_loader_loads_reexported_type_inherent_provider_chain() {
    let root = temp_dir("query_loader_loads_reexported_type_inherent_provider_chain");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::fs;
using std::io;

fun main(file: fs::File, buffer: &mut [u8]) fs::Error!io::FileWriter {
file.writer(buffer)
}
"#,
    );

    let program =
        load_program_with_provider_demand(&main_path, ModuleMap::default(), Some("File"), "writer");

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/io/file_adapter.nia");
    assert_module_loaded(&program, "lib/std/fs/types.nia");
}

#[test]
fn query_loader_loads_native_path_validation_without_file_operations() {
    let root = temp_dir("query_loader_loads_native_path_validation_without_file_operations");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::fs::{NativePathView, PathError};

fun main(bytes: &[u8]) PathError!NativePathView {
NativePathView::fromBytes(bytes)
}
"#,
    );

    let program = load_program_with_provider_demand(
        &main_path,
        ModuleMap::default(),
        Some("NativePathView"),
        "fromBytes",
    );

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/fs.nia");
    assert_module_loaded(&program, "lib/std/fs/types.nia");
    assert_module_loaded(&program, "lib/std/fs/path.nia");
    assert_module_not_loaded(&program, "lib/std/fs/file.nia");
}

#[test]
fn query_loader_loads_relative_path_validation_without_file_operations() {
    let root = temp_dir("query_loader_loads_relative_path_validation_without_file_operations");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::fs::{PathError, PathView, RelativePathView};

fun main(path: PathView) PathError!RelativePathView {
path.relative()
}
"#,
    );

    let program = load_program_with_provider_demand(
        &main_path,
        ModuleMap::default(),
        Some("PathView"),
        "relative",
    );

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/fs.nia");
    assert_module_loaded(&program, "lib/std/fs/types.nia");
    assert_module_loaded(&program, "lib/std/fs/path.nia");
    assert_module_not_loaded(&program, "lib/std/fs/file.nia");
}

#[test]
fn query_loader_loads_contained_metadata_provider_chain() {
    let root = temp_dir("query_loader_loads_contained_metadata_provider_chain");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::fs;

fun main(dir: &fs::Dir, path: fs::RelativePathView) () {
    _ = dir.metadata(path, fs::MetadataOptions::init());
}
"#,
    );

    let program = load_program_with_provider_demand(
        &main_path,
        ModuleMap::default(),
        Some("Dir"),
        "metadata",
    );

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/fs/file.nia");
    if cfg!(windows) {
        assert_module_loaded(&program, "lib/std/os/windows.nia");
        assert_module_not_loaded(&program, "lib/std/os/linux/fd.nia");
    } else {
        assert_module_loaded(&program, "lib/std/os/linux/fd.nia");
        assert_module_loaded(&program, "lib/std/os/linux/stat.nia");
        assert_module_loaded(&program, "lib/std/os/linux/types.nia");
    }
}

#[test]
fn query_loader_forwards_provider_requests_to_the_selected_reexport_source() {
    let root = temp_dir("query_loader_forwards_provider_requests_to_the_selected_reexport_source");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::collections::ArrayList;

fun main() usize {
let list = ArrayList[i32]::init();
_ = list;
0
}
"#,
    );

    let program = load_program_with_provider_demand(
        &main_path,
        ModuleMap::default(),
        Some("ArrayList"),
        "init",
    );

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/collections.nia");
    assert_module_loaded(&program, "lib/std/collections/array_list.nia");
    assert_module_loaded(&program, "lib/std/collections/array_list/list.nia");
    assert!(
        !program
            .modules
            .iter()
            .any(|module| module.path.as_str().contains("/collections/hash_map")),
        "following the selected ArrayList provider chain must not load the HashMap subtree: {:?}",
        program
            .modules
            .iter()
            .map(|module| module.path.as_str())
            .collect::<Vec<_>>()
    );
}
