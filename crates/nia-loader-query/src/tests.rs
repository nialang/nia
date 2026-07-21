use super::*;
use crate::provider_facts::{ProviderDemandsQuery, ProviderFactStore};
use crate::queries::{
    LoadedModuleQuery, ModuleDeclarationsQuery, ModuleFacadeFactsQuery, ParsedModuleQuery,
    ProviderSummaryQuery, SourceStatus, SourceStatusQuery, SourceTextQuery, SyntaxModuleQuery,
    provider_summary_query,
};
use nia_compiler_query::{
    CompileRequest, CompilerDatabase, ProviderDemand, RuntimeModel, has_error_diagnostics,
};
use nia_imports::{ModuleGraph, ModuleNode};
use nia_item_tree::ItemTreeNodeKind;
use nia_symbol::{SymbolId, stable_hash};
use nia_symbol_table::SymbolTable;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

static TEMP_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

#[test]
fn source_frontend_query_keys_are_compact_handles() {
    assert_eq!(std::mem::size_of::<ProviderDemandsQuery>(), 0);
    assert_eq!(
        std::mem::size_of::<crate::graph::ModuleGraphRevisionQuery>(),
        16
    );
    assert_eq!(std::mem::size_of::<SourceTextQuery>(), 4);
    assert_eq!(std::mem::size_of::<SourceStatusQuery>(), 4);
    assert_eq!(std::mem::size_of::<LoadedModuleQuery>(), 4);
    assert_eq!(std::mem::size_of::<ParsedModuleQuery>(), 16);
    assert_eq!(std::mem::size_of::<SyntaxModuleQuery>(), 16);
    assert_eq!(std::mem::size_of::<ModuleDeclarationsQuery>(), 16);
    assert_eq!(std::mem::size_of::<ProviderSummaryQuery>(), 16);
    assert_eq!(std::mem::size_of::<ModuleFacadeFactsQuery>(), 16);
}

fn test_loader_context(
    entry_path: SourcePath,
    module_map: ModuleMap,
    sources: SourceDatabase,
) -> LoaderContext {
    LoaderContext {
        entry_path: entry_path.clone(),
        module_map: effective_module_map(&entry_path, module_map),
        sources,
        node_store: nia_node_id::NodeStore::new(),
        symbols: SymbolTable::new(),
        target: TargetConfig::host(),
        entry_runtime: EntryRuntime::None,
        package_roots_with_used_paths: HashSet::new(),
        provider_facts: ProviderFactStore::default(),
    }
}

fn registered_query_db(context: LoaderContext) -> QueryDb<LoaderContext> {
    QueryDb::new_registered(context, crate::loader_query_registry())
}

fn query_executions(trace: &nia_query::QueryTrace, name: &str) -> usize {
    trace
        .queries
        .iter()
        .filter(|query| query.frame.name == name)
        .map(|query| query.stats.executions)
        .sum()
}

#[test]
fn loader_query_registry_covers_all_declared_query_contracts() {
    let descriptors = crate::loader_query_registry().descriptors();

    assert_eq!(descriptors.len(), 13);
    assert!(
        descriptors
            .windows(2)
            .all(|pair| pair[0].name < pair[1].name)
    );
    assert!(descriptors.iter().all(|descriptor| {
        let expected_fingerprint = if descriptor.name == "source_status" {
            nia_query::QueryFingerprintPolicy::StableValue
        } else {
            nia_query::QueryFingerprintPolicy::None
        };
        descriptor.context_type == std::any::type_name::<LoaderContext>()
            && descriptor.provider == nia_query::QueryProviderPolicy::KeyExecute
            && descriptor.fingerprint == expected_fingerprint
            && descriptor.storage == nia_query::QueryStoragePolicy::CacheOwnedArc
    }));
}

#[test]
fn source_status_tracks_missing_and_present_revisions() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let source_id = sources.id_for_path(&main);
    let db = registered_query_db(test_loader_context(
        main.clone(),
        ModuleMap::default(),
        sources.clone(),
    ));

    let missing = db.get(SourceStatusQuery(source_id));
    assert_eq!(*missing, SourceStatus::Missing);
    let file = sources.set_source(main, "fn main() i32 { 0 }");
    db.invalidate(SourceTextQuery(source_id));
    let present = db.get(SourceStatusQuery(source_id));

    assert!(!Arc::ptr_eq(&missing, &present));
    assert_eq!(*present, SourceStatus::Present(file.version()));
}

#[test]
fn compiler_loader_roots_record_cross_database_dependencies() {
    let sources = SourceDatabase::new();
    sources.set_source(SourcePath::new("main.nia"), "fn main() i32 { 0 }");
    let loader = LoaderDatabase::new(LoadRequest::new("main.nia").with_sources(sources));
    let compiler = CompilerDatabase::new_in_session(
        CompileRequest::new(loader.clone()),
        loader.query_session(),
    );

    let checked = compiler.check_program();

    assert!(!has_error_diagnostics(&checked.diagnostics));
    let trace = compiler.query_trace();
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "loaded_modules" && dependency.to.name == "module_graph"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "program_load_diagnostics"
            && dependency.to.name == "load_diagnostics"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_path" && dependency.to.name == "module_graph"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_source_version" && dependency.to.name == "source_status"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_provider_summary"
            && dependency.to.name == "provider_summary"
    }));
}

fn assert_no_error_diagnostics(program: &nia_compiler_query::LoadedProgram) {
    assert!(
        !has_error_diagnostics(&program.diagnostics),
        "{:?}",
        program.diagnostics
    );
}

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

    let missing = database.load_program();
    assert!(has_error_diagnostics(&missing.diagnostics));
    let missing_entry = missing.graph.entry();
    write(&defs, "pub fn value() i32 { 1 }");

    database.invalidate_source(defs.to_string_lossy().into_owned());
    let present = database.load_program();

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

    assert_no_error_diagnostics(&program);
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

const word_size: usize = size[usize]();
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
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

    assert_no_error_diagnostics(&program);
    let cstring = module_by_suffix(&program, "lib/std/cstring.nia");
    let std_root = program
        .graph
        .package_root(&sym("std"))
        .expect("std package root");
    let std_root = program.graph.get(std_root).expect("std root module");
    let fmt = module_by_suffix(&program, "lib/std/fmt.nia");
    let fmt_core = module_by_suffix(&program, "lib/std/fmt/core.nia");
    assert!(cstring.process_used_paths);
    assert!(
        std_root
            .declarations
            .iter()
            .any(|declaration| declaration.name == sym("fmt")),
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
        "using std::process; pub fn main(init: process::Init) process::ExitCode!void { _ = init; !{} }",
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

#[test]
fn query_loader_loads_implicit_builtin_trait_provider_from_facade() {
    let root = temp_dir("query_loader_loads_implicit_builtin_trait_provider_from_facade");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std;

fn main() void {
    for _ in 1usize..4usize {}
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/iter.nia");
    assert_module_loaded(&program, "lib/std/iter/range.nia");
    assert_module_not_loaded(&program, "lib/std/process/command.nia");
}

#[test]
fn query_loader_loads_iterator_provider_for_for_in_iterator_values() {
    let root = temp_dir("query_loader_loads_iterator_provider_for_for_in_iterator_values");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::process;

fn main(init: process::Init) void {
    for _ in init.env().iter() {}
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/process/env.nia");
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

    let program =
        load_program_with_provider_demand(&main_path, ModuleMap::default(), Some("usize"), "hash");

    assert_no_error_diagnostics(&program);
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

    let program =
        load_program_with_provider_demand(&main_path, ModuleMap::default(), Some("File"), "writer");

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/io/file_adapter.nia");
    assert_module_loaded(&program, "lib/std/fs/types.nia");
}

#[test]
fn query_loader_forwards_provider_requests_to_the_selected_reexport_source() {
    let root = temp_dir("query_loader_forwards_provider_requests_to_the_selected_reexport_source");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::collections::ArrayList;

fn main() usize {
let list = ArrayList[i32]::init();
_ = list;
0
}
"#,
    );

    let program = load_program_with_provider_demand(
        &main_path,
        ModuleMap::default(),
        Some("ArrayList"),
        "init",
    );

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/collections.nia");
    assert_module_loaded(&program, "lib/std/collections/array_list.nia");
    assert_module_loaded(&program, "lib/std/collections/array_list/list.nia");
    assert!(
        !program
            .modules
            .iter()
            .any(|module| module.path.as_str().contains("/collections/hash_map")),
        "following the selected ArrayList provider chain must not load the HashMap subtree"
    );
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

    let program =
        load_program_with_provider_demand(&main_path, ModuleMap::default(), Some("Build"), "init");

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/build.nia");
    assert_module_loaded(&program, "lib/std/build/core.nia");
    assert_module_loaded(&program, "lib/std/build/types.nia");
}

#[test]
fn query_loader_loads_package_private_provider_for_custom_reexported_type() {
    let root = temp_dir("query_loader_loads_package_private_provider_for_custom_reexported_type");
    let main_path = root.join("main.nia");
    let pkg_root = root.join("pkg.nia");
    let main_source = r#"
using dep::facade;

fn main(value: facade::Widget) i32 {
value.score()
}
"#;
    write(&main_path, main_source);
    write(&pkg_root, "pub module facade;");
    fs::create_dir_all(root.join("pkg").join("facade")).expect("create package dir");
    write(
        &root.join("pkg/facade.nia"),
        r#"
pub(pkg) module providers;
pub(pkg) module types;

using self::providers;
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

    let source_path = SourcePath::new(main_path.to_string_lossy());
    let database = LoaderDatabase::new(
        LoadRequest::new(main_path.to_string_lossy().into_owned()).with_module_map(module_map),
    );
    let initial = database.load_program();
    let initial_module_ids = initial
        .graph
        .modules()
        .map(|node| (node.path.identity(), node.id))
        .collect::<HashMap<_, _>>();
    let update = database.update_provider_demands([ProviderDemand {
        source_path,
        request: nia_compiler_query::ProviderRequest::Method {
            target_type_name: Some(sym("Widget")),
            method_name: sym("score"),
        },
    }]);
    let ProviderDemandUpdate::GraphChanged { revision, .. } = update else {
        panic!("provider demand should grow the module graph");
    };
    let program = database.load_program();
    assert_eq!(program.provider_fact_revision, revision);

    assert_no_error_diagnostics(&program);
    for (identity, initial_id) in initial_module_ids {
        assert_eq!(
            program.graph.module_id_for_source_identity(&identity),
            Some(initial_id),
            "provider graph growth changed the module id for {}",
            identity.normalized_path()
        );
    }
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
    let provider_entry = program.graph.entry();

    database.set_source(main_path.to_string_lossy().into_owned(), main_source);
    let reset = database.load_program();

    assert_ne!(reset.graph.entry(), provider_entry);
    assert_module_not_loaded(&reset, "pkg/facade/providers.nia");
}

#[test]
fn provider_demand_update_distinguishes_stable_graphs_and_known_demands() {
    let root = temp_dir("provider_demand_update_distinguishes_stable_graphs_and_known_demands");
    let main_path = root.join("main.nia");
    write(&main_path, "fn main() void {}");
    let database = LoaderDatabase::new(LoadRequest::new(main_path.to_string_lossy().into_owned()));
    let demand = ProviderDemand {
        source_path: SourcePath::new(main_path.to_string_lossy()),
        request: nia_compiler_query::ProviderRequest::Method {
            target_type_name: None,
            method_name: sym("missing"),
        },
    };

    let ProviderDemandUpdate::GraphUnchanged { new_demands, .. } =
        database.update_provider_demands([demand.clone()])
    else {
        panic!("an unmatched provider demand should leave the graph unchanged");
    };
    assert_eq!(new_demands, HashSet::from([demand.clone()]));
    assert_eq!(
        database.update_provider_demands([demand]),
        ProviderDemandUpdate::NoNewDemands
    );
    assert!(
        database
            .query_trace()
            .queries
            .iter()
            .all(|query| query.frame.name != "loaded_program"),
        "a stable graph should not rebuild the aggregate loaded program"
    );
    let trace = database.query_trace();
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_graph" && dependency.to.name == "provider_demands"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_graph_revision"
            && dependency.to.name == "module_graph_revision"
    }));
    let graph_query = trace
        .queries
        .iter()
        .find(|query| query.frame.name == "module_graph")
        .expect("module graph query trace");
    assert_eq!(graph_query.stats.executions, 2, "{graph_query:?}");
}

#[test]
fn semantic_provider_demand_remaps_across_graph_owners() {
    let mut initial = ModuleGraph::new(SourcePath::new("main.nia"));
    let initial_provider = initial
        .intern_declared_child_with_processing(
            initial.entry(),
            &sym("provider"),
            nia_ast::Visibility::Private,
            nia_span::Span::default(),
            false,
            false,
        )
        .expect("initial provider module");
    let provider_path = initial.get(initial_provider).unwrap().path.clone();
    let mut rebuilt = ModuleGraph::new(SourcePath::new("main.nia"));
    let rebuilt_provider = rebuilt
        .intern_declared_child_with_processing(
            rebuilt.entry(),
            &sym("provider"),
            nia_ast::Visibility::Private,
            nia_span::Span::default(),
            false,
            false,
        )
        .expect("rebuilt provider module");

    assert_ne!(rebuilt_provider, initial_provider);
    assert!(!rebuilt.get(rebuilt_provider).unwrap().semantic_selected);
    crate::graph::mark_semantic_provider_module(&mut rebuilt, &provider_path);
    assert!(rebuilt.get(rebuilt_provider).unwrap().semantic_selected);
}

#[test]
fn query_loader_does_not_load_a_declared_provider_without_an_explicit_using_edge() {
    let root =
        temp_dir("query_loader_does_not_load_a_declared_provider_without_an_explicit_using_edge");
    let main_path = root.join("main.nia");
    let pkg_root = root.join("pkg.nia");
    write(
        &main_path,
        r#"
using dep::facade;

fn main(value: facade::Widget) i32 {
_ = value;
0
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
    let provider_path = root.join("pkg/facade/providers.nia");
    let mut module_map = ModuleMap::new();
    module_map.insert("dep", SourcePath::new(pkg_root.to_string_lossy()));

    let program = load_program_with_map(main_path.to_string_lossy().into_owned(), module_map);

    assert_no_error_diagnostics(&program);
    assert!(
        !program
            .modules
            .iter()
            .any(|module| module.path.as_str() == provider_path.to_string_lossy()),
        "declaring a provider child must not make it visible without an explicit using edge"
    );
}

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

#[test]
fn query_trace_records_source_frontend_dependencies() {
    let root = temp_dir("query_trace_records_source_frontend_dependencies");
    let main_path = root.join("main.nia");
    write(&main_path, "fn main() i32 { 0 }");
    let main_path = main_path.to_string_lossy().into_owned();

    let trace = load_program_trace(main_path, ModuleMap::default());

    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "parsed_module" && dependency.to.name == "syntax_module"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "syntax_module" && dependency.to.name == "source_text"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_declarations" && dependency.to.name == "parsed_module"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_graph_revision" && dependency.to.name == "source_status"
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
    let db = registered_query_db(test_loader_context(
        main.clone(),
        ModuleMap::default(),
        sources,
    ));
    let first = db.get(provider_summary_query(&db, &provider));
    let second = db.get(provider_summary_query(&db, &provider));
    assert!(first.defines_inherent_associated_item(&sym("Widget"), &sym("score")));
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
fn facade_facts_are_cached_for_reexport_and_provider_loading() {
    let root = temp_dir("facade_facts_are_cached_for_reexport_and_provider_loading");
    let main = root.join("main.nia");
    let pkg_root = root.join("pkg.nia");
    write(
        &main,
        r#"
using dep::facade;

fn first(value: facade::Widget) i32 {
    value.score()
}

fn second(value: facade::Widget) i32 {
    value.score()
}
"#,
    );
    write(&pkg_root, "pub module facade;");
    fs::create_dir_all(root.join("pkg").join("facade")).expect("create facade dir");
    write(
        &root.join("pkg/facade.nia"),
        r#"
pub(pkg) module providers;
pub(pkg) module types;

using self::providers;
pub using types::Widget;
"#,
    );
    write(
        &root.join("pkg/facade/types.nia"),
        "pub struct Widget { value: i32 }",
    );
    write(
        &root.join("pkg/facade/providers.nia"),
        r#"
using self::types;

extend types::Widget {
    pub fn score(&self) i32 { self.value }
}
"#,
    );
    let mut module_map = ModuleMap::new();
    module_map.insert("dep", SourcePath::new(pkg_root.to_string_lossy()));

    let entry_path = SourcePath::new(main.to_string_lossy());
    let db = registered_query_db(test_loader_context(
        entry_path.clone(),
        module_map,
        SourceDatabase::new(),
    ));
    db.context().provider_facts.insert_new([ProviderDemand {
        source_path: entry_path,
        request: nia_compiler_query::ProviderRequest::Method {
            target_type_name: Some(sym("Widget")),
            method_name: sym("score"),
        },
    }]);
    let program = db.get(LoadedProgramQuery);

    assert_no_error_diagnostics(&program);
    assert_module_loaded(
        &program,
        root.join("pkg/facade/providers.nia")
            .to_string_lossy()
            .as_ref(),
    );

    let trace = db.query_trace();
    let query = trace
        .queries
        .iter()
        .find(|query| query.frame.name == "module_facade_facts")
        .expect("facade facts query should be recorded for custom package facade");
    assert_eq!(query.stats.executions, 1, "{query:?}");
    assert!(
        query.stats.cache_hits >= 1,
        "reexport and provider loading should reuse facade facts: {query:?}"
    );
}

#[test]
fn invalidates_source_dependents_after_in_memory_text_change() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    sources.set_source(main.clone(), "fn main() i32 { 0 }");
    let db = registered_query_db(test_loader_context(
        main.clone(),
        ModuleMap::default(),
        sources.clone(),
    ));

    let first = db.get(LoadedProgramQuery);
    assert_no_error_diagnostics(&first);
    let first_module = first
        .modules
        .iter()
        .find(|module| module.path == main)
        .expect("loaded main module");
    let first_version = first_module.source_version;
    let first_item_tree = first_module.item_tree.clone();
    let first_item_span = first_module.item_tree.items[0].span;
    let first_node_id = first_module
        .origins
        .node_id(nia_node_id::SyntaxKind::Item, first_item_span)
        .expect("first revision item node id");
    let first_locator = db
        .context()
        .node_store
        .locator(first_node_id)
        .expect("first revision item locator");
    assert_eq!(
        first_module.origins.store_id(),
        db.context().node_store.id()
    );
    assert_eq!(first_locator.source_version(), first_version);

    let source_id = sources.id_for_path(&main);
    sources.set_source(main.clone(), "fn main() i32 { 1 }");
    let invalidation = db.invalidate(SourceTextQuery(source_id));
    let invalidated = invalidation
        .invalidated
        .iter()
        .map(|frame| frame.description.as_str())
        .collect::<Vec<_>>();
    let source_description = format!("source_text({source_id:?})");
    assert!(
        invalidated.contains(&source_description.as_str()),
        "{invalidated:?}"
    );
    assert!(
        invalidated
            .iter()
            .any(|description| description.starts_with("parsed_module(SourceVersion")),
        "{invalidated:?}"
    );
    assert!(
        invalidated.contains(&"loaded_program::LoadedProgramQuery"),
        "{invalidated:?}"
    );

    let second = db.get(LoadedProgramQuery);
    assert_no_error_diagnostics(&second);
    let second_module = second
        .modules
        .iter()
        .find(|module| module.path == main)
        .expect("reloaded main module");
    let second_node_id = second_module
        .origins
        .node_id(
            nia_node_id::SyntaxKind::Item,
            second_module.item_tree.items[0].span,
        )
        .expect("second revision item node id");
    assert_ne!(second_module.source_version, first_version);
    assert_ne!(second_module.item_tree, first_item_tree);
    assert_ne!(second_node_id, first_node_id);
    assert_eq!(
        second_module.origins.store_id(),
        db.context().node_store.id()
    );
    assert_eq!(
        db.context().node_store.locator(first_node_id),
        Some(first_locator)
    );
}

#[test]
fn invalidates_module_graph_after_module_declaration_text_change() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    sources.set_source(main.clone(), "");
    sources.set_source(SourcePath::new("defs.nia"), "pub fn value() i32 { 1 }");
    let db = registered_query_db(test_loader_context(
        main.clone(),
        ModuleMap::default(),
        sources.clone(),
    ));

    let first = db.get(LoadedProgramQuery);
    assert_no_error_diagnostics(&first);
    assert_module_loaded(&first, "main.nia");
    assert_module_not_loaded(&first, "defs.nia");
    let first_entry = first.graph.entry();

    let source_id = sources.id_for_path(&main);
    sources.set_source(main, "module defs;");
    db.invalidate(SourceTextQuery(source_id));

    let second = db.get(LoadedProgramQuery);
    assert_no_error_diagnostics(&second);
    assert_ne!(second.graph.entry(), first_entry);
    assert!(
        second
            .modules
            .iter()
            .any(|module| module.path.as_str() == "defs.nia")
    );
}

#[test]
fn loader_source_update_replaces_graph_only_at_query_boundary() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    sources.set_source(main.clone(), "");
    sources.set_source(SourcePath::new("defs.nia"), "pub fn value() i32 { 1 }");
    let database = LoaderDatabase::new(LoadRequest::new(main.as_str()).with_sources(sources));
    let first = database.load_program();
    let executions_before_update = query_executions(&database.query_trace(), "module_graph");

    database.set_source(main.as_str(), "module defs;");

    assert_eq!(
        query_executions(&database.query_trace(), "module_graph"),
        executions_before_update
    );
    let second = database.load_program();
    assert_ne!(second.graph.entry(), first.graph.entry());
    assert_module_loaded(&second, "defs.nia");
    assert!(query_executions(&database.query_trace(), "module_graph") > executions_before_update);
}

#[test]
fn loaded_module_query_reports_paths_outside_module_graph() {
    let sources = SourceDatabase::new();
    let db = registered_query_db(test_loader_context(
        SourcePath::new("main.nia"),
        ModuleMap::default(),
        sources.clone(),
    ));
    let missing = SourcePath::new("missing.nia");
    let missing_id = sources.id_for_path(&missing);

    let err = db
        .try_get(LoadedModuleQuery(missing_id))
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

fn load_program_with_provider_demand(
    entry_path: &Path,
    module_map: ModuleMap,
    target_type_name: Option<&str>,
    method_name: &str,
) -> LoadedProgram {
    let source_path = SourcePath::new(entry_path.to_string_lossy());
    let database = LoaderDatabase::new(
        LoadRequest::new(entry_path.to_string_lossy().into_owned()).with_module_map(module_map),
    );
    let update = database.update_provider_demands([ProviderDemand {
        source_path,
        request: nia_compiler_query::ProviderRequest::Method {
            target_type_name: target_type_name.map(sym),
            method_name: sym(method_name),
        },
    }]);
    match update {
        ProviderDemandUpdate::GraphChanged { .. } => database.load_program(),
        ProviderDemandUpdate::GraphUnchanged { .. } | ProviderDemandUpdate::NoNewDemands => {
            database.load_program()
        }
    }
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
