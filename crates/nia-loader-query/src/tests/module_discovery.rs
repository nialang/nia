use super::*;

#[test]
fn query_loader_loads_declared_modules_once() {
    let root = temp_dir("query_loader_loads_declared_modules_once");
    write(&root.join("main.nia"), "module a; module b;");
    write(&root.join("a.nia"), "module b;");
    fs::create_dir_all(root.join("a")).expect("create child dir");
    write(&root.join("a/b.nia"), "");
    write(&root.join("b.nia"), "pub fun value() i32 { 1 }");

    let program = load_program(root.join("main.nia").to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_eq!(program.modules.len(), program.graph.modules().count());
    assert_module_loaded(&program, root.join("main.nia").to_string_lossy().as_ref());
    assert_module_loaded(&program, root.join("a.nia").to_string_lossy().as_ref());
    assert_module_loaded(&program, root.join("a/b.nia").to_string_lossy().as_ref());
    assert_module_loaded(&program, root.join("b.nia").to_string_lossy().as_ref());
}

#[test]
fn query_loader_roots_the_tree_at_the_entry_file_whatever_lies_beside_it() {
    let root = temp_dir("query_loader_roots_the_tree_at_the_entry_file");
    let main = root.join("main.nia");
    // A file named like the removed package-root convention is ordinary.
    write(&root.join("pkg.nia"), "pub module unrelated;");
    write(&root.join("config.nia"), "pub const answer: i32 = 42;");
    write(
        &main,
        "module config; using root::config; fun main() i32 { config::answer }",
    );

    let program = load_program(main.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    let entry = program.graph.entry();
    assert_eq!(program.graph.current_package_root(entry), Some(entry));
    assert!(
        program
            .graph
            .modules()
            .all(|module| !module.path.as_str().ends_with("pkg.nia")),
        "pkg.nia must not join the program tree"
    );
}

#[test]
fn query_loader_discovers_root_qualified_expression_paths() {
    let root = temp_dir("query_loader_discovers_root_qualified_expression_paths");
    let main = root.join("main.nia");
    write(&root.join("config.nia"), "pub const answer: i32 = 42;");
    write(
        &main,
        "module config; fun main() i32 { root::config::answer }",
    );

    let program = load_program(main.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, root.join("config.nia").to_string_lossy().as_ref());
}

#[test]
fn query_loader_discovers_root_qualified_type_paths() {
    let root = temp_dir("query_loader_discovers_root_qualified_type_paths");
    let main = root.join("main.nia");
    write(&root.join("config.nia"), "pub struct Config {}");
    write(
        &main,
        "module config; fun consume(value: root::config::Config) () { _ = value; }",
    );

    let program = load_program(main.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, root.join("config.nia").to_string_lossy().as_ref());
}

#[test]
fn query_loader_reports_missing_source() {
    let root = temp_dir("query_loader_reports_missing_source");
    write(&root.join("main.nia"), "module missing;");

    let program = load_program(root.join("main.nia").to_string_lossy().into_owned());

    let [diagnostic] = program.diagnostics.as_slice() else {
        panic!("expected one load error: {:?}", program.diagnostics);
    };
    assert!(
        diagnostic.path.as_str().ends_with("main.nia"),
        "{diagnostic:?}"
    );
    assert_eq!(
        diagnostic.diagnostic.summary,
        "module `missing` has no readable source file"
    );
    assert!(
        diagnostic
            .diagnostic
            .notes
            .iter()
            .any(|note| note.contains("failed to read")),
        "{diagnostic:?}"
    );
    assert!(
        program
            .modules
            .iter()
            .all(|module| !module.path.as_str().ends_with("missing.nia")),
        "an unreadable source is not a loaded module"
    );
}

#[test]
fn source_existence_change_rebuilds_missing_module_graph() {
    let root = temp_dir("source_existence_change_rebuilds_missing_module_graph");
    let main = root.join("main.nia");
    let defs = root.join("defs.nia");
    write(&main, "module defs;");
    let database = LoaderDatabase::new_for_test(test_request(main.to_string_lossy().into_owned()));

    let missing = database
        .load_program()
        .expect("missing-module program load");
    assert!(has_error_diagnostics(&missing.diagnostics));
    let missing_entry = missing.graph.entry();
    write(&defs, "pub fun value() i32 { 1 }");

    database
        .invalidate_source(defs.to_string_lossy().into_owned())
        .expect("invalidate source");
    let present = database
        .load_program()
        .expect("present-module program load");

    assert_no_error_diagnostics(&present);
    assert_ne!(present.graph.entry(), missing_entry);
    assert_module_loaded(&present, defs.to_string_lossy().as_ref());
    let defs_module = present
        .modules
        .iter()
        .find(|module| {
            normalized_test_path(module.path.as_str())
                == normalized_test_path(defs.to_string_lossy())
        })
        .expect("present defs module");
    assert_eq!(
        defs_module.source_version.revision,
        nia_source::SourceRevision::INITIAL
    );
}

#[test]
fn conditional_attribute_prunes_unselected_modules_before_graph_loading() {
    let root = temp_dir("conditional_attribute_prunes_unselected_modules_before_graph_loading");
    write(
        &root.join("main.nia"),
        r#"
@[if false]
module missing;
@[if true]
module present;
"#,
    );
    write(&root.join("present.nia"), "pub fun value() i32 { 1 }");

    let program = load_program(root.join("main.nia").to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, root.join("main.nia").to_string_lossy().as_ref());
    assert_module_loaded(
        &program,
        root.join("present.nia").to_string_lossy().as_ref(),
    );
    let root_module = program
        .graph
        .get(program.graph.entry())
        .expect("entry module");
    assert!(root_module.children.contains_key(&sym("present")));
    assert!(!root_module.children.contains_key(&sym("missing")));
}

#[test]
fn conditional_attribute_uses_target_fields_for_module_pruning() {
    let root = temp_dir("conditional_attribute_uses_target_fields_for_module_pruning");
    write(
        &root.join("main.nia"),
        r#"
@[if os == "definitely-not-the-host-os"]
module missing;
@[if os != "definitely-not-the-host-os"]
module present;
"#,
    );
    write(&root.join("present.nia"), "pub fun value() i32 { 1 }");

    let program = load_program(root.join("main.nia").to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, root.join("main.nia").to_string_lossy().as_ref());
    assert_module_loaded(
        &program,
        root.join("present.nia").to_string_lossy().as_ref(),
    );
    let root_module = program
        .graph
        .get(program.graph.entry())
        .expect("entry module");
    assert!(root_module.children.contains_key(&sym("present")));
    assert!(!root_module.children.contains_key(&sym("missing")));
}
