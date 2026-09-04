use super::*;

#[test]
fn query_loader_loads_declared_modules_once() {
    let root = temp_dir("query_loader_loads_declared_modules_once");
    write(&root.join("main.nia"), "module a; module b;");
    write(&root.join("a.nia"), "module b;");
    fs::create_dir_all(root.join("a")).expect("create child dir");
    write(&root.join("a/b.nia"), "");
    write(&root.join("b.nia"), "pub fn value() i32 { 1 }");

    let program = load_program(root.join("main.nia").to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_eq!(program.modules.len(), program.graph.modules().count());
    assert_module_loaded(&program, root.join("main.nia").to_string_lossy().as_ref());
    assert_module_loaded(&program, root.join("a.nia").to_string_lossy().as_ref());
    assert_module_loaded(&program, root.join("a/b.nia").to_string_lossy().as_ref());
    assert_module_loaded(&program, root.join("b.nia").to_string_lossy().as_ref());
}

#[test]
fn query_loader_keeps_package_root_separate_from_entry_module() {
    let root = temp_dir("query_loader_keeps_package_root_separate_from_entry_module");
    let main = root.join("main.nia");
    let package = root.join("pkg.nia");
    write(&package, "pub module config;");
    write(&root.join("config.nia"), "pub const answer: i32 = 42;");
    write(&main, "using pkg::config; fn main() i32 { config::answer }");

    let program = super::load_program_request(
        LoadRequest::new(main.to_string_lossy().into_owned())
            .with_package_root(SourcePath::new(package.to_string_lossy())),
    )
    .expect("package-root program load");

    assert_no_error_diagnostics(&program);
    let entry = program.graph.entry();
    let package_root = program
        .graph
        .current_package_root(entry)
        .expect("entry package root");
    assert_ne!(entry, package_root);
    assert_eq!(
        program.graph.get(entry).expect("entry module").path,
        SourcePath::new(main.to_string_lossy())
    );
    assert_eq!(
        program.graph.get(package_root).expect("package root").path,
        SourcePath::new(package.to_string_lossy())
    );
}

#[test]
fn query_loader_reports_missing_source() {
    let root = temp_dir("query_loader_reports_missing_source");
    write(&root.join("main.nia"), "module missing;");

    let program = load_program(root.join("main.nia").to_string_lossy().into_owned());

    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.diagnostic.summary.contains("failed to read") })
    );
}

#[test]
fn source_existence_change_rebuilds_missing_module_graph() {
    let root = temp_dir("source_existence_change_rebuilds_missing_module_graph");
    let main = root.join("main.nia");
    let defs = root.join("defs.nia");
    write(&main, "module defs;");
    let database = LoaderDatabase::new(LoadRequest::new(main.to_string_lossy().into_owned()));

    let missing = database
        .load_program()
        .expect("missing-module program load");
    assert!(has_error_diagnostics(&missing.diagnostics));
    let missing_entry = missing.graph.entry();
    write(&defs, "pub fn value() i32 { 1 }");

    database.invalidate_source(defs.to_string_lossy().into_owned());
    let present = database
        .load_program()
        .expect("present-module program load");

    assert_no_error_diagnostics(&present);
    assert_ne!(present.graph.entry(), missing_entry);
    assert_module_loaded(&present, defs.to_string_lossy().as_ref());
    let defs_module = present
        .modules
        .iter()
        .find(|module| module.path.as_str() == defs.to_string_lossy())
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
    write(&root.join("present.nia"), "pub fn value() i32 { 1 }");

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
    write(&root.join("present.nia"), "pub fn value() i32 { 1 }");

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
