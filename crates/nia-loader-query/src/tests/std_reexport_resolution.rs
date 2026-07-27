use super::*;

#[test]
fn query_loader_resolves_std_root_reexport_import_shallowly() {
    let root = temp_dir("query_loader_resolves_std_root_reexport_import_shallowly");
    let main_path = root.join("main.nia");
    write(&main_path, "using std::CStringView; fn main() void {}");

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
    write(&main_path, "using std::CStringView; fn main() void {}");

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
        r#"fn main() void { if ?text = std::CStringView::from_bytes(b"nia\0") { _ = text; } or null {} }"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert!(!program.graph.package_facade_active(&sym("std")));
    assert_module_loaded(&program, "lib/std/cstring.nia");
    assert_module_not_loaded(&program, "lib/std/process.nia");
}
