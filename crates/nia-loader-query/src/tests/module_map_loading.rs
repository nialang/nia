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
fn query_loader_discovers_module_map_root_used_only_by_a_pattern() {
    let root = temp_dir("query_loader_discovers_module_map_root_used_only_by_a_pattern");
    write(
        &root.join("main.nia"),
        r#"
fn classify(value: i32) i32 {
    match value {
        dep::EXPECTED => 1,
        _ => 0,
    }
}
"#,
    );
    write(&root.join("dep.nia"), "pub const EXPECTED: i32 = 7;");
    let mut module_map = ModuleMap::new();
    module_map.insert(
        "dep",
        SourcePath::new(root.join("dep.nia").to_string_lossy()),
    );

    let program = load_program_with_map(root.join("main.nia").to_string_lossy(), module_map);

    assert_no_error_diagnostics(&program);
    assert!(
        program
            .modules
            .iter()
            .any(|module| module.path.as_str().ends_with("dep.nia"))
    );
}

#[test]
fn query_loader_discovers_module_map_root_in_const_generic_parameter_type() {
    let root = temp_dir("query_loader_discovers_module_map_root_in_const_generic_parameter_type");
    write(&root.join("main.nia"), "fn inspect[N: dep::Marker]() () {}");
    write(&root.join("dep.nia"), "pub struct Marker {}");
    let mut module_map = ModuleMap::new();
    module_map.insert(
        "dep",
        SourcePath::new(root.join("dep.nia").to_string_lossy()),
    );

    let program = load_program_with_map(root.join("main.nia").to_string_lossy(), module_map);

    assert_no_error_diagnostics(&program);
    assert!(
        program
            .modules
            .iter()
            .any(|module| module.path.as_str().ends_with("dep.nia"))
    );
}

#[test]
fn query_loader_injects_std_from_explicit_toolchain_layout() {
    let root = temp_dir("query_loader_injects_std_from_explicit_toolchain_layout");
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
    assert_eq!(
        std_module.path.as_str(),
        test_toolchain_layout().std_module().to_string_lossy()
    );
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

#[test]
fn query_loader_uses_only_the_relocated_toolchain_resource_tree() {
    let root = temp_dir("query_loader_uses_only_the_relocated_toolchain_resource_tree");
    let relocated_resources = root.join("resources");
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_resources = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .expect("nia-loader-query lives under crates/")
        .join("lib");
    copy_tree(&workspace_resources, &relocated_resources);
    let executable = root.join("bin/nia");
    std::fs::create_dir_all(executable.parent().expect("compiler parent"))
        .expect("create compiler directory");
    write(&executable, "compiler");
    let layout = std::sync::Arc::new(
        nia_toolchain::ToolchainLayout::resolve(nia_toolchain::ToolchainLayoutRequest::explicit(
            &executable,
            &relocated_resources,
        ))
        .expect("relocated toolchain layout"),
    );
    let main = root.join("main.nia");
    write(&main, "using std::builtin::target;");

    let program = crate::load_program(main.to_string_lossy().into_owned(), layout)
        .expect("load through relocated toolchain");

    assert_no_error_diagnostics(&program);
    let relocated_resources =
        std::fs::canonicalize(relocated_resources).expect("canonical relocated resource root");
    let std_modules = program
        .modules
        .iter()
        .filter(|module| module.path.as_str().contains("/std"))
        .collect::<Vec<_>>();
    assert!(!std_modules.is_empty(), "{:?}", program.modules);
    assert!(
        std_modules
            .iter()
            .all(|module| std::path::Path::new(module.path.as_str())
                .starts_with(&relocated_resources)),
        "{:?}",
        std_modules
    );
}

fn copy_tree(source: &std::path::Path, destination: &std::path::Path) {
    std::fs::create_dir_all(destination).expect("create copied resource directory");
    for entry in std::fs::read_dir(source).expect("read resource directory") {
        let entry = entry.expect("read resource entry");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_tree(&source_path, &destination_path);
        } else {
            std::fs::copy(&source_path, &destination_path).expect("copy resource file");
        }
    }
}
