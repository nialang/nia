use super::*;

#[test]
fn query_loader_loads_len_prelude_provider_on_demand() {
    let root = temp_dir("query_loader_loads_len_prelude_provider_on_demand");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
fn main(values: &[u8]) usize {
    values.len()
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/builtin.nia");
    assert_module_loaded(&program, "lib/std/builtin/place.nia");
    assert_module_not_loaded(&program, "lib/std/builtin/atomic.nia");
}

#[test]
fn query_loader_does_not_load_len_provider_without_len_demand() {
    let root = temp_dir("query_loader_does_not_load_len_provider_without_len_demand");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
fn main() usize {
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
fn query_loader_loads_implicit_builtin_trait_provider_from_facade() {
    let root = temp_dir("query_loader_loads_implicit_builtin_trait_provider_from_facade");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std;

fn main() () {
    for _ in 1usize..4usize {}
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/iter.nia");
    assert_module_loaded(&program, "lib/std/iter/range.nia");
    assert_module_not_loaded(&program, "lib/std/process/command.nia");
}

#[test]
fn query_loader_loads_iterator_provider_for_for_in_iterator_values() {
    let root = temp_dir("query_loader_loads_iterator_provider_for_for_in_iterator_values");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::process;

fn main(init: process::Init) () {
    for _ in init.env().iter() {}
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/process/env.nia");
}

#[test]
fn query_loader_keeps_implicit_trait_search_on_the_processed_collection_branch() {
    let root =
        temp_dir("query_loader_keeps_implicit_trait_search_on_the_processed_collection_branch");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::collections;

fn main(values: collections::ArrayList[i32]) usize {
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

fn main() u64 {
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

fn main(file: fs::File, buffer: &mut [u8]) fs::Error!io::FileWriter {
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

fn main(bytes: &[u8]) PathError!NativePathView {
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

fn main(path: PathView) PathError!RelativePathView {
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

fn main(dir: &fs::Dir, path: fs::RelativePathView) () {
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
    assert_module_loaded(&program, "lib/std/os/linux/fd.nia");
    assert_module_loaded(&program, "lib/std/os/linux/stat.nia");
    assert_module_loaded(&program, "lib/std/os/linux/types.nia");
}

#[test]
fn query_loader_forwards_provider_requests_to_the_selected_reexport_source() {
    let root = temp_dir("query_loader_forwards_provider_requests_to_the_selected_reexport_source");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::collections::ArrayList;

fn main() usize {
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
        "following the selected ArrayList provider chain must not load the HashMap subtree"
    );
}
