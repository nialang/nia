use super::*;

#[test]
fn query_loader_uses_package_module_map() {
    let root = temp_dir("query_loader_uses_package_module_map");
    write(&root.join("main.nia"), "using std::io;");
    write(&root.join("std.nia"), "");
    fs::create_dir_all(root.join("std")).expect("create std dir");
    write(&root.join("std.nia"), "pub module io;");
    write(&root.join("std/io.nia"), "pub fn value() i32 { 1 }");
    let mut module_map = ModuleMap::new();
    module_map.insert(
        "std",
        SourcePath::new(root.join("std.nia").to_string_lossy()),
    );

    let program = load_program_with_map(
        root.join("main.nia").to_string_lossy().into_owned(),
        module_map,
    );

    assert_no_error_diagnostics(&program);
    assert_eq!(program.runtime, RuntimeModel::Bare);
    assert!(program.graph.package_root(&sym("std")).is_some());
    assert!(
        program.modules.iter().any(
            |module| module.path.as_str() == root.join("std/io.nia").to_string_lossy().as_ref()
        )
    );
}

#[test]
fn query_loader_processes_module_map_root_when_selected_as_value_host() {
    let root = temp_dir("query_loader_processes_module_map_root_when_selected_as_value_host");
    write(
        &root.join("main.nia"),
        r#"
using dep;

fn main() i32 {
    dep::build()
}
"#,
    );
    write(
        &root.join("dep.nia"),
        r#"
pub fn build() i32 {
    1
}
"#,
    );
    let mut module_map = ModuleMap::new();
    module_map.insert(
        "dep",
        SourcePath::new(root.join("dep.nia").to_string_lossy()),
    );

    let program = load_program_with_map(root.join("main.nia").to_string_lossy(), module_map);

    assert_no_error_diagnostics(&program);
    let dep = module_by_suffix(&program, "dep.nia");
    assert!(
        dep.process_used_paths,
        "selected module-map package root must be semantic: {dep:?}"
    );
}

#[test]
fn query_loader_injects_default_std_module_map_to_toolchain_lib() {
    let root = temp_dir("query_loader_injects_default_std_module_map_to_toolchain_lib");
    let main_path = root.join("main.nia");
    write(&main_path, "using std;");

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    let std_module = program
        .graph
        .get(
            program
                .graph
                .package_root(&sym("std"))
                .expect("std package root"),
        )
        .expect("std module");
    assert_eq!(std_module.path.as_str(), default_std_module_path().as_str());
    assert!(!program.graph.package_facade_active(&sym("std")));
    assert_module_not_loaded(&program, "lib/std/build.nia");
    assert_module_not_loaded(&program, "lib/std/process.nia");
}

#[test]
fn query_loader_loads_std_builtin_target_module() {
    let root = temp_dir("query_loader_loads_std_builtin_target_module");
    let main_path = root.join("main.nia");
    write(&main_path, "using std::builtin::target;");

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert!(program.graph.package_root(&sym("builtin")).is_none());
    let target_loaded = program
        .modules
        .iter()
        .find(|module| module.path.as_str().ends_with("lib/std/builtin/target.nia"))
        .expect("loaded std::builtin::target module");
    assert!(target_loaded.item_tree.items.iter().any(|item| {
        matches!(
            &item.kind,
            ItemTreeNodeKind::Binding(binding)
                if binding.is_const() && binding.name == sym("pointer_width")
        )
    }));
}
