use super::*;

#[test]
fn query_loader_injects_freestanding_entry_runtime_through_std_start_facade() {
    let root = temp_dir("query_loader_injects_freestanding_entry_runtime_through_std_start_facade");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        "using std::process; pub fn main(init: process::Init) process::ExitCode!() { _ = init; !() }",
    );

    let program = load_program_with_map_and_entry_runtime(
        main_path.to_string_lossy().into_owned(),
        ModuleMap::default(),
        EntryRuntime::Freestanding,
    );

    assert_no_error_diagnostics(&program);
    assert_eq!(program.runtime, RuntimeModel::FreestandingExecutable);
    assert!(
        program
            .modules
            .iter()
            .any(|module| module.path.as_str().ends_with("lib/std/start.nia")),
        "{:?}",
        program
            .modules
            .iter()
            .map(|module| module.path.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        program.modules.iter().any(|module| module
            .path
            .as_str()
            .ends_with("lib/std/start/freestanding/linux/x86_64.nia")),
        "{:?}",
        program
            .modules
            .iter()
            .map(|module| module.path.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        program
            .graph
            .modules()
            .any(|module| module.path.as_str().ends_with("lib/std/start.nia"))
    );
    let std_root = program
        .graph
        .package_root(&sym("std"))
        .expect("std package root");
    let std = program.graph.get(std_root).expect("std entry module");
    let start_declaration = std
        .declarations
        .iter()
        .find(|declaration| declaration.name == sym("start"))
        .expect("injected std start declaration");
    assert_eq!(
        start_declaration.visibility,
        nia_imports::Visibility::PublicPkg
    );
}

#[test]
fn query_loader_loads_std_package_root_children_on_demand() {
    let root = temp_dir("query_loader_loads_std_package_root_children_on_demand");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        "using std::process; pub fn main(init: process::Init) process::ExitCode!() { _ = init; !() }",
    );

    let program = load_program_with_map_and_entry_runtime(
        main_path.to_string_lossy().into_owned(),
        ModuleMap::default(),
        EntryRuntime::Freestanding,
    );

    assert_no_error_diagnostics(&program);
    assert!(!program.graph.package_facade_active(&sym("std")));
    let process = module_by_suffix(&program, "lib/std/process.nia");
    assert!(
        !process.process_used_paths,
        "process facade should stay shallow while selected exports load their source modules: {process:?}"
    );
    assert!(
        process
            .declarations
            .iter()
            .any(|declaration| declaration.name == sym("types")),
        "process should record the selected types child: {process:?}"
    );
    let process_types = module_by_suffix(&program, "lib/std/process/types.nia");
    assert!(process_types.process_used_paths);
    assert_module_loaded(&program, "lib/std/process/types.nia");
    assert_module_not_loaded(&program, "lib/std/process/command.nia");
    assert_module_loaded(&program, "lib/std/start/freestanding/linux/x86_64.nia");
    assert_module_not_loaded(&program, "lib/std/build/core.nia");
    assert_module_not_loaded(&program, "lib/std/atomic.nia");
    assert_module_not_loaded(&program, "lib/std/debug.nia");
}
