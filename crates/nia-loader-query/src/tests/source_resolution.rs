use super::*;

#[test]
fn query_loader_keeps_local_modules_from_activating_same_named_package() {
    let root = temp_dir("query_loader_keeps_local_modules_from_activating_same_named_package");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
module std;

fn main(value: std::fmt::Value) () {
_ = value;
}
"#,
    );
    write(&root.join("std.nia"), "pub module fmt;");
    fs::create_dir_all(root.join("std")).expect("create std dir");
    write(&root.join("std/fmt.nia"), "pub struct Value {}");

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert!(!program.graph.package_facade_active(&sym("std")));
    assert!(program.graph.package_root(&sym("std")).is_none());
    assert!(
        program
            .modules
            .iter()
            .any(|module| module.path.as_str() == root.join("std/fmt.nia").to_string_lossy())
    );
    assert_module_not_loaded(&program, "lib/std/builtin.nia");
    assert_module_not_loaded(&program, "lib/std/fmt.nia");
}

#[test]
fn query_loader_resolves_root_children_relative_to_entry_file() {
    let root = temp_dir("query_loader_resolves_root_children_relative_to_entry_file");
    let main_path = root.join("main.nia");
    write(&main_path, "module defs;");
    write(&root.join("defs.nia"), "pub fn value() i32 { 1 }");

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, root.join("main.nia").to_string_lossy().as_ref());
    assert_module_loaded(&program, root.join("defs.nia").to_string_lossy().as_ref());
    let root_module = program
        .graph
        .get(program.graph.entry())
        .expect("entry module");
    let defs_module = program
        .graph
        .get(root_module.children[&sym("defs")])
        .expect("defs module");
    assert_eq!(
        defs_module.path.as_str(),
        root.join("defs.nia").to_string_lossy().as_ref()
    );
}

#[test]
fn query_loader_accepts_in_memory_sources() {
    let sources = SourceDatabase::new();
    sources.set_source(SourcePath::new("main.nia"), "module defs;");
    sources.set_source(SourcePath::new("defs.nia"), "pub fn value() i32 { 1 }");

    let program = load_program_from_sources("main.nia", ModuleMap::default(), sources);

    assert_no_error_diagnostics(&program);
    assert!(program.modules.iter().any(|module| {
        module.path.as_str() == "main.nia"
            && module.item_tree.items.iter().any(|item| {
                matches!(
                    &item.kind,
                    ItemTreeNodeKind::Module(module_item) if module_item.name == sym("defs")
                )
            })
    }));
    assert!(program.modules.iter().any(|module| {
        module.path.as_str() == "defs.nia"
            && module.item_tree.items.iter().any(|item| {
                matches!(
                    &item.kind,
                    ItemTreeNodeKind::Function(function) if function.name == sym("value")
                )
            })
    }));
}
