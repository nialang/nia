use super::*;

#[test]
fn query_loader_keeps_unused_explicit_std_imports_shallow() {
    let root = temp_dir("query_loader_keeps_unused_explicit_std_imports_shallow");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::collections;
using std::fs;
using std::io;
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
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
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("collections")),
        "{warnings:?}"
    );
    assert!(
        warnings
            .iter()
            .all(|diagnostic| !diagnostic.diagnostic.summary.contains("process")),
        "{warnings:?}"
    );
    let collections = module_by_suffix(&program, "lib/std/collections.nia");
    let fs = module_by_suffix(&program, "lib/std/fs.nia");
    let io = module_by_suffix(&program, "lib/std/io.nia");
    let mem = module_by_suffix(&program, "lib/std/mem.nia");
    let process = module_by_suffix(&program, "lib/std/process.nia");
    assert!(!collections.process_used_paths, "{collections:?}");
    assert!(!fs.process_used_paths, "{fs:?}");
    assert!(!io.process_used_paths, "{io:?}");
    assert!(!mem.process_used_paths, "{mem:?}");
    assert!(!process.process_used_paths, "{process:?}");
    assert_module_loaded(&program, "lib/std/process/types.nia");
    assert_module_not_loaded(&program, "lib/std/collections/hash_map.nia");
    assert_module_not_loaded(&program, "lib/std/collections/array_list.nia");
    assert_module_not_loaded(&program, "lib/std/fs/file.nia");
    assert_module_not_loaded(&program, "lib/std/io/file_adapter.nia");
    assert_module_not_loaded(&program, "lib/std/mem/general_purpose_allocator.nia");
    assert_module_not_loaded(&program, "lib/std/process/command.nia");
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

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
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
