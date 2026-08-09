use super::*;

#[test]
fn broad_and_narrow_std_imports_select_the_same_module_work() {
    let root = temp_dir("broad_and_narrow_std_imports_select_the_same_module_work");
    let broad_path = root.join("broad.nia");
    let narrow_path = root.join("narrow.nia");
    write(
        &broad_path,
        r#"
using std::collections;

fn consume(values: collections::ArrayList[i32]) usize {
    values.len()
}
"#,
    );
    write(
        &narrow_path,
        r#"
using std::collections::ArrayList;

fn consume(values: ArrayList[i32]) usize {
    values.len()
}
"#,
    );

    let broad = load_program(broad_path.to_string_lossy().into_owned());
    let narrow = load_program(narrow_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&broad);
    assert_no_error_diagnostics(&narrow);
    assert_eq!(
        toolchain_module_activation(&broad),
        toolchain_module_activation(&narrow),
        "public import spelling must not change selected std work"
    );
}

#[test]
fn query_loader_keeps_unused_explicit_std_imports_shallow() {
    let root = temp_dir("query_loader_keeps_unused_explicit_std_imports_shallow");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::build;
using std::fs;
using std::io;
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    !()
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    let warnings = program
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.is_warning())
        .collect::<Vec<_>>();
    assert_eq!(warnings.len(), 4, "{warnings:?}");
    assert!(
        warnings
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("build")),
        "{warnings:?}"
    );
    assert!(
        warnings
            .iter()
            .all(|diagnostic| !diagnostic.diagnostic.summary.contains("process")),
        "{warnings:?}"
    );
    let build = module_by_suffix(&program, "lib/std/build.nia");
    let fs = module_by_suffix(&program, "lib/std/fs.nia");
    let io = module_by_suffix(&program, "lib/std/io.nia");
    let mem = module_by_suffix(&program, "lib/std/mem.nia");
    let process = module_by_suffix(&program, "lib/std/process.nia");
    assert!(!build.process_used_paths, "{build:?}");
    assert!(!fs.process_used_paths, "{fs:?}");
    assert!(!io.process_used_paths, "{io:?}");
    assert!(!mem.process_used_paths, "{mem:?}");
    assert!(!process.process_used_paths, "{process:?}");
    assert_module_loaded(&program, "lib/std/process/types.nia");
    assert_module_not_loaded(&program, "lib/std/build/core.nia");
    assert_module_not_loaded(&program, "lib/std/fs/file.nia");
    assert_module_not_loaded(&program, "lib/std/io/file_adapter.nia");
    assert_module_not_loaded(&program, "lib/std/mem/general_purpose_allocator.nia");
    assert_module_not_loaded(&program, "lib/std/process/command.nia");
}

fn toolchain_module_activation(program: &LoadedProgram) -> Vec<(String, bool, bool, bool)> {
    let mut modules = program
        .graph
        .modules()
        .filter_map(|module| {
            let identity = module.path.identity();
            identity
                .normalized_path()
                .starts_with("toolchain:/")
                .then(|| {
                    (
                        identity.normalized_path().to_string(),
                        module.semantic_selected,
                        module.process_used_paths,
                        module.process_declared_children,
                    )
                })
        })
        .collect::<Vec<_>>();
    modules.sort_unstable();
    modules
}

#[test]
fn query_loader_counts_package_facade_scope_as_import_usage() {
    let root = temp_dir("query_loader_counts_package_facade_scope_as_import_usage");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std;

fn main() i32 {
    let mut sum = 0;
    for i in 1usize..4usize {
        sum += i as i32;
    }
    sum
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert!(
        program
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.is_warning()),
        "{:?}",
        program.diagnostics
    );
    assert_module_loaded(&program, "lib/std/iter.nia");
}

#[test]
fn query_loader_does_not_warn_for_used_narrow_explicit_import() {
    let root = temp_dir("query_loader_does_not_warn_for_used_narrow_explicit_import");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    !()
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert!(
        program
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.is_warning())
            .count()
            == 0,
        "{:?}",
        program.diagnostics
    );
    assert_module_loaded(&program, "lib/std/process/types.nia");
    assert_module_not_loaded(&program, "lib/std/process/command.nia");
}
