use super::*;

#[test]
fn query_loader_scopes_structural_methods_through_trait_reexports() {
    let root = temp_dir("query_loader_scopes_structural_methods_through_trait_reexports");
    let main = root.join("main.nia");
    write(&main, "module parse; using root::parse; fun main() () {}");
    fs::create_dir_all(root.join("parse")).expect("create facade directory");
    write(
        &root.join("parse/implementation.nia"),
        r#"
pub trait From[Input] { fun from(input: Input) Self; }
extend[Unit] [Unit] where Unit: Sized {
    pub fun parse[T](&self) T where T: From[&[Unit]] { [T]::from(self) }
}
"#,
    );
    for reexport in [
        "pub using implementation::From;",
        "pub using implementation::{From};",
        "pub using implementation::{From as Conversion};",
    ] {
        write(
            &root.join("parse.nia"),
            &format!("pub module implementation; {reexport}"),
        );
        let program = load_program_with_provider_demand(&main, ModuleMap::default(), None, "parse");
        assert_no_error_diagnostics(&program);
        let provider = program
            .graph
            .modules()
            .find(|module| module.path.as_str().ends_with("parse/implementation.nia"))
            .expect("method provider");
        assert!(
            program
                .graph
                .get(program.graph.entry())
                .expect("entry module")
                .provider_dependencies
                .contains(&provider.id),
            "{reexport}: {:?}",
            program.graph
        );
    }
}

#[test]
fn query_loader_loads_trait_provider_declared_in_package_root() {
    let root = temp_dir("query_loader_loads_trait_provider_declared_in_package_root");
    let main_path = root.join("main.nia");
    let pkg_root = root.join("dep.nia");
    write(
        &main_path,
        "using dep::types; fun main(value: types::Widget) () { _ = value; }",
    );
    fs::create_dir_all(root.join("dep")).expect("create dependency package");
    write(
        &root.join("dep/types.nia"),
        "pub struct Widget { value: usize }",
    );
    write(
        &pkg_root,
        r#"
pub module types;
using root::types;
extend types::Widget : Len {
    const fun len(&self) usize { self.value }
}
"#,
    );
    let mut module_map = ModuleMap::new();
    module_map
        .insert("dep", SourcePath::new(pkg_root.to_string_lossy()))
        .expect("insert dependency package");
    let program = load_program_with_trait_provider_demand_for_package(
        &main_path,
        module_map,
        Some("Widget"),
        Some(sym("dep")),
        "Len",
        &[],
    );
    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, pkg_root.to_string_lossy().as_ref());
}

#[test]
fn query_loader_processes_imports_owned_by_trait_provider() {
    let root = temp_dir("query_loader_processes_imports_owned_by_trait_provider");
    let main_path = root.join("main.nia");
    let pkg_root = root.join("dep.nia");
    write(&main_path, "fun main() () {}");
    fs::create_dir_all(root.join("dep")).expect("create dependency package");
    write(
        &root.join("dep/types.nia"),
        "pub struct Widget { value: usize }",
    );
    write(
        &root.join("dep/providers.nia"),
        r#"
using root::types;

extend types::Widget : Len {
    fun len(&self) usize { self.value }
}
"#,
    );
    write(&pkg_root, "pub module types; pub module providers;");
    let mut module_map = ModuleMap::new();
    module_map
        .insert("dep", SourcePath::new(pkg_root.to_string_lossy()))
        .expect("insert dependency package");

    let program = load_program_with_trait_provider_demand_for_package(
        &main_path,
        module_map,
        Some("Widget"),
        Some(sym("dep")),
        "Len",
        &[],
    );
    assert_no_error_diagnostics(&program);
    assert_module_loaded(
        &program,
        root.join("dep/providers.nia").to_string_lossy().as_ref(),
    );
    assert_module_loaded(
        &program,
        root.join("dep/types.nia").to_string_lossy().as_ref(),
    );
    let entry = program
        .graph
        .get(program.graph.entry())
        .expect("entry module");
    let provider = program
        .graph
        .modules()
        .find(|module| module.path.as_str().ends_with("dep/providers.nia"))
        .expect("trait provider");
    assert!(entry.trait_impl_dependencies.contains(&provider.id));
    assert!(!entry.provider_dependencies.contains(&provider.id));
}

#[test]
fn query_loader_loads_builtin_trait_provider_from_a_non_std_package() {
    let root = temp_dir("query_loader_loads_builtin_trait_provider_from_a_non_std_package");
    let main_path = root.join("main.nia");
    let pkg_root = root.join("pkg.nia");
    write(
        &main_path,
        r#"
using dep::types;

fun main(value: types::Widget) usize {
    value.len()
}

"#,
    );
    write(&pkg_root, "pub module types;\npub module extensions;");
    write(
        &root.join("types.nia"),
        "pub struct Widget { value: usize }",
    );
    fs::create_dir_all(root.join("extensions")).expect("create provider directory");
    write(&root.join("extensions.nia"), "pub module widgets;");
    write(
        &root.join("extensions/widgets.nia"),
        r#"
using root::types;

extend types::Widget : Len {
    fun len(&self) usize { self.value }
}
"#,
    );
    let mut module_map = ModuleMap::new();
    module_map
        .insert("dep", SourcePath::new(pkg_root.to_string_lossy()))
        .expect("insert module map entry");

    let program =
        load_program_with_trait_provider_demand(&main_path, module_map, Some("Widget"), "Len", &[]);

    assert_no_error_diagnostics(&program);
    assert_module_loaded(
        &program,
        root.join("extensions/widgets.nia")
            .to_string_lossy()
            .as_ref(),
    );
}

#[test]
fn query_loader_does_not_search_a_third_package_for_trait_implementations() {
    let root = temp_dir("query_loader_does_not_search_a_third_package_for_trait_implementations");
    let main_path = root.join("main.nia");
    let dep_root = root.join("dep.nia");
    let extra_root = root.join("extra.nia");
    write(
        &main_path,
        r#"
using dep::types;
using extra::facade;

fun main(value: types::Widget) usize {
    _ = value;
    0usize
}
"#,
    );
    write(&dep_root, "pub module types;");
    fs::create_dir_all(root.join("dep")).expect("create target package directory");
    write(
        &root.join("dep/types.nia"),
        "pub struct Widget { value: usize }",
    );
    write(&extra_root, "pub module facade;");
    fs::create_dir_all(root.join("extra/facade")).expect("create unrelated provider directory");
    write(&root.join("extra/facade.nia"), "pub module providers;");
    write(
        &root.join("extra/facade/providers.nia"),
        r#"
using dep::types;

extend types::Widget : Len {
    fun len(&self) usize { self.value }
}
"#,
    );
    let mut module_map = ModuleMap::new();
    module_map
        .insert("dep", SourcePath::new(dep_root.to_string_lossy()))
        .expect("insert target package");
    module_map
        .insert("extra", SourcePath::new(extra_root.to_string_lossy()))
        .expect("insert unrelated package");

    let program = load_program_with_trait_provider_demand_for_package(
        &main_path,
        module_map,
        Some("Widget"),
        Some(sym("dep")),
        "Len",
        &[],
    );

    assert_no_error_diagnostics(&program);
    assert_module_loaded(
        &program,
        root.join("extra/facade.nia").to_string_lossy().as_ref(),
    );
    assert_module_not_loaded(
        &program,
        root.join("extra/facade/providers.nia")
            .to_string_lossy()
            .as_ref(),
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

fun main(init: process::Init, allocator: &mut mem::Allocator) build::Error!build::Build {
let path = fs::PathView::init(&"");
let text: &[char] = &"";
let target = build::TargetView::init(text, text, text, text, text, text, 64u32);
let mut initialization = build::Build::init(allocator, path, path, path, path, path, target, target, build::OptimizationMode::O0, 1u32, null);
initialization.finish()
}
"#,
    );

    let program =
        load_program_with_provider_demand(&main_path, ModuleMap::default(), Some("Build"), "init");

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/build.nia");
    assert_module_loaded(&program, "lib/std/build/lifecycle.nia");
    assert_module_loaded(&program, "lib/std/build/types.nia");
    assert_module_not_loaded(&program, "lib/std/build/core.nia");
    assert_module_not_loaded(&program, "lib/std/build/plan.nia");
}

#[test]
fn query_loader_loads_package_private_provider_for_custom_reexported_type() {
    let root = temp_dir("query_loader_loads_package_private_provider_for_custom_reexported_type");
    let main_path = root.join("main.nia");
    let pkg_root = root.join("pkg.nia");
    let main_source = r#"
using dep::facade;

fun main(value: facade::Widget) i32 {
value.score()
}
"#;
    write(&main_path, main_source);
    write(&pkg_root, "pub module facade;");
    fs::create_dir_all(root.join("facade")).expect("create package dir");
    write(
        &root.join("facade.nia"),
        r#"
pub(root) module providers;
pub(root) module types;

using self::providers;
pub using types::Widget;
"#,
    );
    write(
        &root.join("facade/types.nia"),
        r#"pub struct Widget { value: i32 }"#,
    );
    write(
        &root.join("facade/providers.nia"),
        r#"
using self::types;

extend types::Widget {
pub fun score(&self) i32 {
    self.value
}
}
"#,
    );
    let mut module_map = ModuleMap::new();
    module_map
        .insert("dep", SourcePath::new(pkg_root.to_string_lossy()))
        .expect("insert module map entry");
    let source_path = SourcePath::new(main_path.to_string_lossy());
    let database = LoaderDatabase::new_for_test(
        test_request(main_path.to_string_lossy().into_owned()).with_module_map(module_map),
    );
    let initial = database.load_program().expect("initial program load");
    let initial_module_ids = initial
        .graph
        .modules()
        .map(|node| (node.path.identity(), node.id))
        .collect::<HashMap<_, _>>();
    let update = database
        .update_provider_demands([ProviderDemand {
            source_path,
            request: nia_compiler_query::ProviderRequest::Method {
                target_type_name: Some(sym("Widget")),
                method_name: sym("score"),
            },
        }])
        .expect("provider graph update");
    let ProviderGraphUpdate::Changed { .. } = update else {
        panic!("provider demand should grow the module graph");
    };
    let program = database.load_program().expect("provider program load");
    assert_eq!(
        program.provider_fact_revision,
        nia_compiler_query::LoaderFactProvider::provider_facts(&database)
            .expect("provider facts")
            .revision()
    );

    assert_no_error_diagnostics(&program);
    for (identity, initial_id) in initial_module_ids {
        assert_eq!(
            program.graph.module_id_for_source_identity(&identity),
            Some(initial_id),
            "provider graph growth changed the module id for {}",
            identity.normalized_path()
        );
    }
    assert_module_loaded(&program, root.join("facade.nia").to_string_lossy().as_ref());
    assert_module_loaded(
        &program,
        root.join("facade/types.nia").to_string_lossy().as_ref(),
    );
    assert_module_loaded(
        &program,
        root.join("facade/providers.nia").to_string_lossy().as_ref(),
    );
    let provider_entry = program.graph.entry();

    database
        .set_source(main_path.to_string_lossy().into_owned(), main_source)
        .expect("replace source");
    let reset = database.load_program().expect("reset program load");

    assert_ne!(reset.graph.entry(), provider_entry);
    assert_module_not_loaded(&reset, "facade/providers.nia");
}
