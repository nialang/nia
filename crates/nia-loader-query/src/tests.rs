use super::*;
use crate::queries::{LoadedModuleQuery, provider_summary_query};
use nia_compiler_query::RuntimeModel;
use nia_imports::ModuleNode;
use nia_item_tree::ItemTreeNodeKind;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

static TEMP_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[test]
fn query_loader_loads_declared_modules_once() {
    let root = temp_dir("query_loader_loads_declared_modules_once");
    write(&root.join("main.nia"), "module a; module b;");
    write(&root.join("a.nia"), "module b;");
    fs::create_dir_all(root.join("a")).expect("create child dir");
    write(&root.join("a/b.nia"), "");
    write(&root.join("b.nia"), "pub fn value() i32 { 1 }");

    let program = load_program(root.join("main.nia").to_string_lossy().into_owned());

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert_eq!(program.modules.len(), program.graph.modules().count());
    assert_module_loaded(&program, root.join("main.nia").to_string_lossy().as_ref());
    assert_module_loaded(&program, root.join("a.nia").to_string_lossy().as_ref());
    assert_module_loaded(&program, root.join("a/b.nia").to_string_lossy().as_ref());
    assert_module_loaded(&program, root.join("b.nia").to_string_lossy().as_ref());
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

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert_module_loaded(&program, root.join("main.nia").to_string_lossy().as_ref());
    assert_module_loaded(
        &program,
        root.join("present.nia").to_string_lossy().as_ref(),
    );
    let root_module = program
        .graph
        .get(program.graph.entry())
        .expect("entry module");
    assert!(root_module.children.contains_key("present"));
    assert!(!root_module.children.contains_key("missing"));
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

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert_module_loaded(&program, root.join("main.nia").to_string_lossy().as_ref());
    assert_module_loaded(
        &program,
        root.join("present.nia").to_string_lossy().as_ref(),
    );
    let root_module = program
        .graph
        .get(program.graph.entry())
        .expect("entry module");
    assert!(root_module.children.contains_key("present"));
    assert!(!root_module.children.contains_key("missing"));
}

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

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert_eq!(program.runtime, RuntimeModel::Bare);
    assert!(program.graph.package_root("std").is_some());
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

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
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

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let std_module = program
        .graph
        .get(program.graph.package_root("std").expect("std package root"))
        .expect("std module");
    assert_eq!(std_module.path.as_str(), default_std_module_path().as_str());
    assert!(!program.graph.package_facade_active("std"));
    assert_module_not_loaded(&program, "lib/std/build.nia");
    assert_module_not_loaded(&program, "lib/std/process.nia");
}

#[test]
fn query_loader_loads_std_builtin_target_module() {
    let root = temp_dir("query_loader_loads_std_builtin_target_module");
    let main_path = root.join("main.nia");
    write(&main_path, "using std::builtin::target;");

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(program.graph.package_root("builtin").is_none());
    let target_loaded = program
        .modules
        .iter()
        .find(|module| module.path.as_str().ends_with("lib/std/builtin/target.nia"))
        .expect("loaded std::builtin::target module");
    assert!(target_loaded.item_tree.items.iter().any(|item| {
        matches!(
            &item.kind,
            ItemTreeNodeKind::Binding(binding)
                if binding.is_comptime && binding.name == "pointer_width"
        )
    }));
}

#[test]
fn query_loader_loads_facade_reexport_sources_by_used_name() {
    let root = temp_dir("query_loader_loads_facade_reexport_sources_by_used_name");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::collections;

fn main(values: collections::ArrayList[i32]) void {
_ = values;
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert_module_loaded(&program, "lib/std/collections.nia");
    let collections_node = program
        .graph
        .modules()
        .find(|module| module.path.as_str().ends_with("lib/std/collections.nia"))
        .expect("std collections node");
    assert!(
        !collections_node.process_used_paths,
        "collections facade should stay shallow: {collections_node:?}"
    );
    assert_module_loaded(&program, "lib/std/collections/array_list.nia");
    assert_module_loaded(&program, "lib/std/collections/array_list/list.nia");
    assert_module_not_loaded(&program, "lib/std/collections/hash_map.nia");
    assert_module_not_loaded(&program, "lib/std/collections/hash_map/map.nia");
}

#[test]
fn query_loader_keeps_facade_used_paths_shallow_for_reexported_value() {
    let root = temp_dir("query_loader_keeps_facade_used_paths_shallow_for_reexported_value");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::builtin::size;

comptime word_size: usize = size[usize]();
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let builtin = module_by_suffix(&program, "lib/std/builtin.nia");
    let layout = module_by_suffix(&program, "lib/std/builtin/layout.nia");
    assert!(!builtin.process_used_paths);
    assert!(layout.process_used_paths);
    assert_module_not_loaded(&program, "lib/std/builtin/atomic.nia");
    assert_module_not_loaded(&program, "lib/std/builtin/ops.nia");
}

#[test]
fn query_loader_loads_reexported_std_type_module_dependencies() {
    let root = temp_dir("query_loader_loads_reexported_std_type_module_dependencies");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::CStringView;

fn main() void {
_ = CStringView::from_ptr(&0u8);
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let cstring = module_by_suffix(&program, "lib/std/cstring.nia");
    let std_root = program.graph.package_root("std").expect("std package root");
    let std_root = program.graph.get(std_root).expect("std root module");
    let fmt = module_by_suffix(&program, "lib/std/fmt.nia");
    let fmt_core = module_by_suffix(&program, "lib/std/fmt/core.nia");
    assert!(cstring.process_used_paths);
    assert!(
        std_root
            .declarations
            .iter()
            .any(|declaration| declaration.name == "fmt"),
        "std root should record the fmt module declaration: {std_root:?}"
    );
    assert!(
        !fmt.process_used_paths,
        "fmt facade should stay shallow while selected exports load their source modules: {fmt:?}"
    );
    assert!(fmt_core.process_used_paths);
    assert_module_loaded(&program, "lib/std/cstring.nia");
    assert_module_loaded(&program, "lib/std/fmt.nia");
    assert_module_loaded(&program, "lib/std/fmt/core.nia");
    assert_module_not_loaded(&program, "lib/std/build.nia");
    assert_module_not_loaded(&program, "lib/std/process.nia");
}

#[test]
fn query_loader_injects_freestanding_entry_runtime_through_std_start_facade() {
    let root = temp_dir("query_loader_injects_freestanding_entry_runtime_through_std_start_facade");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        "using std::process; pub fn main(init: process::Init) process::ExitCode!void { _ = init; !{} }",
    );

    let program = load_program_with_map_and_entry_runtime(
        main_path.to_string_lossy().into_owned(),
        ModuleMap::default(),
        EntryRuntime::Freestanding,
    );

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
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
    let std_root = program.graph.package_root("std").expect("std package root");
    let std = program.graph.get(std_root).expect("std entry module");
    let start_declaration = std
        .declarations
        .iter()
        .find(|declaration| declaration.name == "start")
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
        "using std::process; pub fn main(init: process::Init) process::ExitCode!void { _ = init; !{} }",
    );

    let program = load_program_with_map_and_entry_runtime(
        main_path.to_string_lossy().into_owned(),
        ModuleMap::default(),
        EntryRuntime::Freestanding,
    );

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(!program.graph.package_facade_active("std"));
    let process = module_by_suffix(&program, "lib/std/process.nia");
    assert!(
        !process.process_used_paths,
        "process facade should stay shallow while selected exports load their source modules: {process:?}"
    );
    assert!(
        process
            .declarations
            .iter()
            .any(|declaration| declaration.name == "types"),
        "process should record the selected types child: {process:?}"
    );
    let process_types = module_by_suffix(&program, "lib/std/process/types.nia");
    let process_init = module_by_suffix(&program, "lib/std/process/init.nia");
    let process_args = module_by_suffix(&program, "lib/std/process/args.nia");
    let process_env = module_by_suffix(&program, "lib/std/process/env.nia");
    let slice = module_by_suffix(&program, "lib/std/slice.nia");
    let iter = module_by_suffix(&program, "lib/std/iter.nia");
    assert!(process_types.process_used_paths);
    assert!(process_init.process_used_paths);
    assert!(process_args.process_used_paths);
    assert!(process_env.process_used_paths);
    assert!(slice.process_used_paths);
    assert!(iter.process_used_paths);
    assert_module_loaded(&program, "lib/std/process/init.nia");
    assert_module_loaded(&program, "lib/std/process/args.nia");
    assert_module_loaded(&program, "lib/std/process/env.nia");
    assert_module_loaded(&program, "lib/std/process/types.nia");
    assert_module_not_loaded(&program, "lib/std/process/command.nia");
    assert_module_loaded(&program, "lib/std/start/freestanding/linux/x86_64.nia");
    assert_module_not_loaded(&program, "lib/std/build/core.nia");
    assert_module_not_loaded(&program, "lib/std/atomic.nia");
    assert_module_not_loaded(&program, "lib/std/debug.nia");
}

#[test]
fn query_loader_loads_facade_trait_impl_provider_for_used_trait_method() {
    let root = temp_dir("query_loader_loads_facade_trait_impl_provider_for_used_trait_method");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::hash;

fn main() u64 {
let mut hasher = hash::Wyhash::init(1u64);
42usize.hash(&mut hasher);
hasher.finish()
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert_module_loaded(&program, "lib/std/hash.nia");
    assert_module_loaded(&program, "lib/std/hash/impls.nia");
    assert_module_loaded(&program, "lib/std/hash/wyhash.nia");
}

#[test]
fn query_loader_loads_reexported_type_inherent_provider_chain() {
    let root = temp_dir("query_loader_loads_reexported_type_inherent_provider_chain");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::fs;
using std::io;
using std::os;

fn main(file: fs::File, state: &mut io::Io[Error = os::Error], buffer: &mut [u8]) fs::Error!io::FileWriter {
file.writer(state, buffer)
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert_module_loaded(&program, "lib/std/io/file_adapter.nia");
    assert_module_loaded(&program, "lib/std/fs/file.nia");
    assert_module_loaded(&program, "lib/std/fs/types.nia");
}

#[test]
fn query_loader_loads_package_private_provider_for_reexported_build_type() {
    let root = temp_dir("query_loader_loads_package_private_provider_for_reexported_build_type");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::build;
using std::fs;
using std::mem;
using std::process;

fn main(init: process::Init, allocator: &mut mem::Allocator) build::Build {
let path = fs::PathView::init(&"");
build::Build::init(init, allocator, path, path, path, path, 1usize)
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert_module_loaded(&program, "lib/std/build.nia");
    assert_module_loaded(&program, "lib/std/build/core.nia");
    assert_module_loaded(&program, "lib/std/build/types.nia");
    assert_module_loaded(&program, "lib/std/fmt/template.nia");
    assert_module_loaded(&program, "lib/std/io/file_adapter.nia");
}

#[test]
fn query_loader_loads_package_private_provider_for_custom_reexported_type() {
    let root = temp_dir("query_loader_loads_package_private_provider_for_custom_reexported_type");
    let main_path = root.join("main.nia");
    let pkg_root = root.join("pkg.nia");
    write(
        &main_path,
        r#"
using dep::facade;

fn main(value: facade::Widget) i32 {
value.score()
}
"#,
    );
    write(&pkg_root, "pub module facade;");
    fs::create_dir_all(root.join("pkg").join("facade")).expect("create package dir");
    write(
        &root.join("pkg/facade.nia"),
        r#"
pub(pkg) module providers;
pub(pkg) module types;

pub using types::Widget;
"#,
    );
    write(
        &root.join("pkg/facade/types.nia"),
        r#"pub struct Widget { value: i32 }"#,
    );
    write(
        &root.join("pkg/facade/providers.nia"),
        r#"
using self::types;

extend types::Widget {
pub fn score(&self) i32 {
    self.value
}
}
"#,
    );
    let mut module_map = ModuleMap::new();
    module_map.insert("dep", SourcePath::new(pkg_root.to_string_lossy()));

    let program = load_program_with_map(main_path.to_string_lossy().into_owned(), module_map);

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert_module_loaded(
        &program,
        root.join("pkg/facade.nia").to_string_lossy().as_ref(),
    );
    assert_module_loaded(
        &program,
        root.join("pkg/facade/types.nia").to_string_lossy().as_ref(),
    );
    assert_module_loaded(
        &program,
        root.join("pkg/facade/providers.nia")
            .to_string_lossy()
            .as_ref(),
    );
}

#[test]
fn query_loader_resolves_std_root_reexport_import_shallowly() {
    let root = temp_dir("query_loader_resolves_std_root_reexport_import_shallowly");
    let main_path = root.join("main.nia");
    write(&main_path, "using std::CStringView; fn main() void {}");

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(!program.graph.package_facade_active("std"));
    assert_module_loaded(&program, "lib/std/cstring.nia");
    assert_module_not_loaded(&program, "lib/std/process.nia");
}

#[test]
fn query_loader_resolves_std_single_value_reexport_import_shallowly() {
    let root = temp_dir("query_loader_resolves_std_single_value_reexport_import_shallowly");
    let main_path = root.join("main.nia");
    write(&main_path, "using std::CStringView; fn main() void {}");

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(!program.graph.package_facade_active("std"));
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

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(!program.graph.package_facade_active("std"));
    assert_module_loaded(&program, "lib/std/cstring.nia");
    assert_module_not_loaded(&program, "lib/std/process.nia");
}

#[test]
fn query_loader_keeps_local_modules_from_activating_same_named_package() {
    let root = temp_dir("query_loader_keeps_local_modules_from_activating_same_named_package");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
module std;

fn main(value: std::fmt::Value) void {
_ = value;
}
"#,
    );
    write(&root.join("std.nia"), "pub module fmt;");
    fs::create_dir_all(root.join("std")).expect("create std dir");
    write(&root.join("std/fmt.nia"), "pub struct Value {}");

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(!program.graph.package_facade_active("std"));
    assert!(program.graph.package_root("std").is_none());
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

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert_module_loaded(&program, root.join("main.nia").to_string_lossy().as_ref());
    assert_module_loaded(&program, root.join("defs.nia").to_string_lossy().as_ref());
    let root_module = program
        .graph
        .get(program.graph.entry())
        .expect("entry module");
    let defs_module = program
        .graph
        .get(root_module.children["defs"])
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

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(program.modules.iter().any(|module| {
        module.path.as_str() == "main.nia"
            && module.item_tree.items.iter().any(|item| {
                matches!(
                    &item.kind,
                    ItemTreeNodeKind::Module(module_item) if module_item.name == "defs"
                )
            })
    }));
    assert!(program.modules.iter().any(|module| {
        module.path.as_str() == "defs.nia"
            && module.item_tree.items.iter().any(|item| {
                matches!(
                    &item.kind,
                    ItemTreeNodeKind::Function(function) if function.name == "value"
                )
            })
    }));
}

#[test]
fn query_trace_records_source_frontend_dependencies() {
    let root = temp_dir("query_trace_records_source_frontend_dependencies");
    let main_path = root.join("main.nia");
    write(&main_path, "fn main() i32 { 0 }");
    let main_path = main_path.to_string_lossy().into_owned();

    let trace = load_program_trace(main_path.clone(), ModuleMap::default());

    assert!(trace.dependencies.iter().any(|dependency| {
        dependency
            .from
            .description
            .starts_with(&format!("parsed_module({main_path})@"))
            && dependency
                .to
                .description
                .starts_with(&format!("syntax_module({main_path})@"))
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency
            .from
            .description
            .starts_with(&format!("syntax_module({main_path})@"))
            && dependency.to.description == format!("source_text({main_path})")
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency
            .from
            .description
            .starts_with(&format!("module_declarations({main_path})@"))
            && dependency
                .to
                .description
                .starts_with(&format!("parsed_module({main_path})@"))
    }));
}

#[test]
fn provider_summary_is_cached_per_module_source_version() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let provider = SourcePath::new("provider.nia");
    sources.set_source(main.clone(), "fn main() void {}");
    sources.set_source(
        provider.clone(),
        r#"
struct Widget { value: i32 }

extend Widget {
    pub fn score(&self) i32 {
        self.value
    }
}
"#,
    );
    let db = QueryDb::new(LoaderContext {
        entry_path: main.clone(),
        module_map: effective_module_map(&main, ModuleMap::default()),
        sources,
        target: TargetConfig::host(),
        entry_runtime: EntryRuntime::None,
        package_root_used_paths: false,
    });
    let first = db.query(provider_summary_query(&db, provider.clone()));
    let second = db.query(provider_summary_query(&db, provider.clone()));
    assert!(first.defines_inherent_associated_item("Widget", "score"));
    assert_eq!(first, second);

    let trace = db.query_trace();
    let query = trace
        .queries
        .iter()
        .find(|query| query.frame.name == "provider_summary")
        .expect("provider summary query should be recorded");
    assert_eq!(query.stats.executions, 1, "{query:?}");
    assert_eq!(query.stats.cache_hits, 1, "{query:?}");
}

#[test]
fn invalidates_source_dependents_after_in_memory_text_change() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    sources.set_source(main.clone(), "fn main() i32 { 0 }");
    let db = QueryDb::new(LoaderContext {
        entry_path: main.clone(),
        module_map: effective_module_map(&main, ModuleMap::default()),
        sources: sources.clone(),
        target: TargetConfig::host(),
        entry_runtime: EntryRuntime::None,
        package_root_used_paths: false,
    });

    let first = db.query(LoadedProgramQuery);
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let first_module = first
        .modules
        .iter()
        .find(|module| module.path == main)
        .expect("loaded main module");
    let first_version = first_module.source_version;
    let first_item_tree = first_module.item_tree.clone();

    sources.set_source(main.clone(), "fn main() i32 { 1 }");
    let invalidation = db.invalidate(SourceTextQuery(main.clone()));
    let invalidated = invalidation
        .invalidated
        .iter()
        .map(|frame| frame.description.as_str())
        .collect::<Vec<_>>();
    assert!(
        invalidated.contains(&"source_text(main.nia)"),
        "{invalidated:?}"
    );
    assert!(
        invalidated
            .iter()
            .any(|description| description.starts_with("parsed_module(main.nia)@")),
        "{invalidated:?}"
    );
    assert!(
        invalidated.contains(&"loaded_program::LoadedProgramQuery"),
        "{invalidated:?}"
    );

    let second = db.query(LoadedProgramQuery);
    assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
    let second_module = second
        .modules
        .iter()
        .find(|module| module.path == main)
        .expect("reloaded main module");
    assert_ne!(second_module.source_version, first_version);
    assert_ne!(second_module.item_tree, first_item_tree);
}

#[test]
fn invalidates_module_graph_after_module_declaration_text_change() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    sources.set_source(main.clone(), "");
    sources.set_source(SourcePath::new("defs.nia"), "pub fn value() i32 { 1 }");
    let db = QueryDb::new(LoaderContext {
        entry_path: main.clone(),
        module_map: effective_module_map(&main, ModuleMap::default()),
        sources: sources.clone(),
        target: TargetConfig::host(),
        entry_runtime: EntryRuntime::None,
        package_root_used_paths: false,
    });

    let first = db.query(LoadedProgramQuery);
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_module_loaded(&first, "main.nia");
    assert_module_not_loaded(&first, "defs.nia");

    sources.set_source(main.clone(), "module defs;");
    db.invalidate(SourceTextQuery(main));

    let second = db.query(LoadedProgramQuery);
    assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
    assert!(
        second
            .modules
            .iter()
            .any(|module| module.path.as_str() == "defs.nia")
    );
}

#[test]
fn loaded_module_query_reports_paths_outside_module_graph() {
    let db = QueryDb::new(LoaderContext {
        entry_path: SourcePath::new("main.nia"),
        module_map: ModuleMap::default(),
        sources: SourceDatabase::new(),
        target: TargetConfig::host(),
        entry_runtime: EntryRuntime::None,
        package_root_used_paths: false,
    });

    let err = db
        .try_query(LoadedModuleQuery(SourcePath::new("missing.nia")))
        .expect_err("missing module path should be an invalid query input");

    assert!(matches!(err, nia_query::QueryError::InvalidInput { .. }));
    assert!(
        err.to_string()
            .contains("missing module id for `missing.nia`"),
        "{err}"
    );
}

fn temp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let id = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.push(format!(
        "nia_loader_query_{name}_{}_{:?}_{id}",
        std::process::id(),
        std::thread::current().id()
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write(path: &Path, source: &str) {
    fs::write(path, source).expect("write source");
}

fn assert_module_loaded(program: &LoadedProgram, suffix: &str) {
    assert!(
        program
            .modules
            .iter()
            .any(|module| module.path.as_str().ends_with(suffix)),
        "missing module {suffix}: {:?}",
        program
            .modules
            .iter()
            .map(|module| module.path.as_str())
            .collect::<Vec<_>>()
    );
}

fn module_by_suffix<'a>(program: &'a LoadedProgram, suffix: &str) -> &'a ModuleNode {
    program
        .graph
        .modules()
        .find(|module| module.path.as_str().ends_with(suffix))
        .unwrap_or_else(|| {
            panic!(
                "missing module {suffix}: {:?}",
                program
                    .modules
                    .iter()
                    .map(|module| module.path.as_str())
                    .collect::<Vec<_>>()
            )
        })
}

fn assert_module_not_loaded(program: &LoadedProgram, suffix: &str) {
    assert!(
        !program
            .modules
            .iter()
            .any(|module| module.path.as_str().ends_with(suffix)),
        "unexpected module {suffix}: {:?}",
        program
            .modules
            .iter()
            .map(|module| module.path.as_str())
            .collect::<Vec<_>>()
    );
}
