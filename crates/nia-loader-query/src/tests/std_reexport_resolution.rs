use super::*;

#[test]
fn reexport_header_lookup_loads_every_matching_namespace_route() {
    let root = temp_dir("reexport_header_lookup_loads_every_matching_namespace_route");
    let main = root.join("main.nia");
    write(&main, "using dep::api::Thing; fun main() () {}");
    write(
        &root.join("root.nia"),
        "pub module api; pub module types; pub module values;",
    );
    write(
        &root.join("api.nia"),
        "pub using root::types::Thing; pub using root::values::Thing;",
    );
    write(&root.join("types.nia"), "pub struct Thing {}");
    write(&root.join("values.nia"), "pub fun Thing() i32 { 42 }");
    let mut module_map = ModuleMap::new();
    module_map
        .insert(
            "dep",
            SourcePath::new(root.join("root.nia").to_string_lossy()),
        )
        .expect("map dependency");

    let program = load_program_with_map(main.to_string_lossy().into_owned(), module_map);

    assert_no_error_diagnostics(&program);
    for module in ["types.nia", "values.nia"] {
        let node = module_by_suffix(&program, root.join(module).to_string_lossy().as_ref());
        assert!(!node.process_used_paths, "{node:?}");
        assert!(!node.semantic_selected, "{node:?}");
    }
}

#[test]
fn unused_reexport_selections_follow_original_names_without_activating_bodies() {
    for (label, relay) in [
        ("single", "pub using root::facade::Renamed as Final;"),
        ("group", "pub using root::{facade::{Renamed as Final}};"),
        ("wildcard", "pub using root::facade::*;"),
    ] {
        let root = temp_dir(&format!("unused_reexport_selection_{label}"));
        let main = root.join("main.nia");
        let selected = if label == "wildcard" {
            "Renamed"
        } else {
            "Final"
        };
        write(
            &main,
            &format!("using dep::relay::{{{selected}}}; fun main() () {{}}"),
        );
        write(
            &root.join("root.nia"),
            "pub module relay; pub module facade; pub module source; pub module unrelated;",
        );
        write(&root.join("relay.nia"), relay);
        write(
            &root.join("facade.nia"),
            "pub(root) using root::source::Original as Renamed;",
        );
        write(
            &root.join("source.nia"),
            "using root::unrelated; pub struct Original {} fun helper() i32 { unrelated::value() }",
        );
        write(&root.join("unrelated.nia"), "pub fun value() i32 { 42 }");
        let mut module_map = ModuleMap::new();
        module_map
            .insert(
                "dep",
                SourcePath::new(root.join("root.nia").to_string_lossy()),
            )
            .expect("map dependency");

        let program = load_program_with_map(main.to_string_lossy().into_owned(), module_map);

        assert_no_error_diagnostics(&program);
        for module in ["relay.nia", "facade.nia", "source.nia"] {
            let node = module_by_suffix(&program, root.join(module).to_string_lossy().as_ref());
            assert!(!node.process_used_paths, "{label}: {node:?}");
            assert!(!node.semantic_selected, "{label}: {node:?}");
        }
        assert_module_not_loaded(
            &program,
            root.join("unrelated.nia").to_string_lossy().as_ref(),
        );
    }
}

#[test]
fn query_loader_resolves_std_root_reexport_import_shallowly() {
    let root = temp_dir("query_loader_resolves_std_root_reexport_import_shallowly");
    let main_path = root.join("main.nia");
    write(&main_path, "using std::CStringView; fun main() () {}");

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert!(!program.graph.package_facade_active(&sym("std")));
    assert_module_loaded(&program, "lib/std/cstring.nia");
    assert_module_not_loaded(&program, "lib/std/process.nia");
}

#[test]
fn query_loader_resolves_std_single_value_reexport_import_shallowly() {
    let root = temp_dir("query_loader_resolves_std_single_value_reexport_import_shallowly");
    let main_path = root.join("main.nia");
    write(&main_path, "using std::CStringView; fun main() () {}");

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert!(!program.graph.package_facade_active(&sym("std")));
    assert_module_loaded(&program, "lib/std/cstring.nia");
    assert_module_not_loaded(&program, "lib/std/process.nia");
}

#[test]
fn query_loader_resolves_std_qualified_root_reexport_shallowly() {
    let root = temp_dir("query_loader_resolves_std_qualified_root_reexport_shallowly");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"fun main() () { match std::CStringView::fromBytes(b"nia\0") {
    !text => { _ = text; },
    error! => {},
} }"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert!(!program.graph.package_facade_active(&sym("std")));
    assert_module_loaded(&program, "lib/std/cstring.nia");
    assert_module_not_loaded(&program, "lib/std/process.nia");
}
