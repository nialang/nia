use super::*;

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
    let reset = database.load_program().expect("reset program load");

    assert_ne!(reset.graph.entry(), provider_entry);
    assert_module_not_loaded(&reset, "pkg/facade/providers.nia");
}
