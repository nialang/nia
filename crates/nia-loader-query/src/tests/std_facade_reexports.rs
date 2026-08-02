use super::*;

#[test]
fn query_loader_loads_facade_reexport_sources_by_used_name() {
    let root = temp_dir("query_loader_loads_facade_reexport_sources_by_used_name");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::collections;

fn main(values: collections::ArrayList[i32]) void {
_ = values;
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/collections.nia");
    let collections_node = program
        .graph
        .modules()
        .find(|module| module.path.as_str().ends_with("lib/std/collections.nia"))
        .expect("std collections node");
    assert!(
        !collections_node.process_used_paths,
        "collections facade should stay shallow: {collections_node:?}"
    );
    assert_module_loaded(&program, "lib/std/collections/array_list.nia");
    assert_module_loaded(&program, "lib/std/collections/array_list/list.nia");
    assert_module_not_loaded(&program, "lib/std/collections/hash_map.nia");
    assert_module_not_loaded(&program, "lib/std/collections/hash_map/map.nia");
}

#[test]
fn query_loader_keeps_facade_used_paths_shallow_for_reexported_value() {
    let root = temp_dir("query_loader_keeps_facade_used_paths_shallow_for_reexported_value");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::builtin::size;

const word_size: usize = size[usize]();
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    let builtin = module_by_suffix(&program, "lib/std/builtin.nia");
    let layout = module_by_suffix(&program, "lib/std/builtin/layout.nia");
    assert!(!builtin.process_used_paths);
    assert!(layout.process_used_paths);
    assert_module_not_loaded(&program, "lib/std/builtin/atomic.nia");
    assert_module_not_loaded(&program, "lib/std/builtin/ops.nia");
}

#[test]
fn query_loader_loads_reexported_std_type_module_dependencies() {
    let root = temp_dir("query_loader_loads_reexported_std_type_module_dependencies");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::CStringView;

fn main() void {
_ = CStringView::fromPtrUnchecked(&0u8);
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    let cstring = module_by_suffix(&program, "lib/std/cstring.nia");
    let std_root = program
        .graph
        .package_root(&sym("std"))
        .expect("std package root");
    let std_root = program.graph.get(std_root).expect("std root module");
    let fmt = module_by_suffix(&program, "lib/std/fmt.nia");
    let fmt_core = module_by_suffix(&program, "lib/std/fmt/core.nia");
    assert!(cstring.process_used_paths);
    assert!(
        std_root
            .declarations
            .iter()
            .any(|declaration| declaration.name == sym("fmt")),
        "std root should record the fmt module declaration: {std_root:?}"
    );
    assert!(
        !fmt.process_used_paths,
        "fmt facade should stay shallow while selected exports load their source modules: {fmt:?}"
    );
    assert!(fmt_core.process_used_paths);
    assert_module_loaded(&program, "lib/std/cstring.nia");
    assert_module_loaded(&program, "lib/std/fmt.nia");
    assert_module_loaded(&program, "lib/std/fmt/core.nia");
    assert_module_not_loaded(&program, "lib/std/build.nia");
    assert_module_not_loaded(&program, "lib/std/process.nia");
}
