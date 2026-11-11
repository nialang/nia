// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

fn persisted_plan_path(loader: &LoaderDatabase) -> PathBuf {
    let context = loader.db.context();
    let directory = context
        .frontend_cache
        .as_ref()
        .expect("frontend cache")
        .provider_demand_plan_directory(context.provider_demand_plan_key.expect("plan key"));
    let paths = fs::read_dir(directory)
        .expect("plan directory")
        .map(|entry| entry.expect("plan entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "fpd"))
        .collect::<Vec<_>>();
    assert_eq!(paths.len(), 1, "expected one persisted plan");
    paths.into_iter().next().unwrap()
}

fn provider_plan_toolchain(root: &Path) -> Arc<nia_toolchain::ToolchainLayout> {
    let resources = root.join("resources");
    let executable = root.join("bin/nia");
    fs::create_dir_all(executable.parent().expect("compiler parent"))
        .expect("create compiler directory");
    fs::create_dir_all(resources.join("std")).expect("create resource directories");
    fs::create_dir_all(resources.join("runtime")).expect("create runtime directory");
    write(&executable, "compiler");
    write(
        &resources.join("toolchain.meta"),
        include_str!("../../../../lib/toolchain.meta"),
    );
    for name in nia_compat::TOOLCHAIN_PACKAGES {
        write(
            &resources.join(name).join("nia.toml"),
            &nia_compat::toolchain_package_manifest(name),
        );
    }
    write(&resources.join("std/root.nia"), "pub module unicode;");
    write(
        &resources.join("runtime/root.nia"),
        "pub(root) module start;",
    );
    write(&resources.join("runtime/start.nia"), "");
    write(
        &resources.join("std/unicode.nia"),
        "extend char { pub fun encoded_len(self) i32 { _ = self; 1 } }",
    );
    Arc::new(
        nia_toolchain::ToolchainLayout::resolve(nia_toolchain::ToolchainLayoutRequest::explicit(
            executable, resources,
        ))
        .expect("test toolchain layout"),
    )
}

/// Unrelated packages deliberately use the same relocatable identities. Even
/// identical entry bytes do not identify a plan: their children may differ.
#[test]
fn shared_plans_preserve_concurrent_source_snapshots_and_relocation() {
    let root = temp_dir("provider_plan_source_snapshots");
    let cache_root = root.join("cache");
    let request = |tree: &str, value: i32| {
        let directory = root.join(tree);
        let main = directory.join("main.nia");
        write(&main, "module helper; fun main() i32 { 0 }");
        write(
            &directory.join("helper.nia"),
            &format!("const VALUE = {value};"),
        );
        LoadRequest::from_source_path(SourcePath::with_identity(
            main.to_string_lossy(),
            "build-package:root:/main.nia",
        ))
        .with_toolchain_layout(test_toolchain_layout())
        .with_frontend_cache_dir(Some(cache_root.clone()))
    };
    let first = LoaderDatabase::new_for_test(request("first", 1));
    let second = LoaderDatabase::new_for_test(request("second", 2));
    assert_eq!(
        first.db.context().provider_demand_plan_key,
        second.db.context().provider_demand_plan_key
    );
    let install_demand = |loader: &LoaderDatabase| {
        let entry = loader.db.context().entry_path.clone();
        let helper = SourcePath::with_identity(
            Path::new(entry.as_str())
                .with_file_name("helper.nia")
                .to_string_lossy(),
            "build-package:root:/helper.nia",
        );
        loader.load_program().expect("load tree");
        loader
            .update_provider_demands([ProviderDemand {
                source_path: entry,
                request: nia_compiler_query::ProviderRequest::ModuleBody {
                    module_path: helper,
                },
            }])
            .expect("activate helper body");
    };
    install_demand(&first);
    install_demand(&second);
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        for loader in [&first, &second] {
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                nia_compiler_query::LoaderFactProvider::settle_provider_demands(loader)
                    .expect("publish concurrent snapshot");
            });
        }
    });
    for (tree, value) in [
        ("first", 1),
        ("second", 2),
        ("relocated", 1),
        ("first", 1),
        ("second", 2),
    ] {
        let warm = LoaderDatabase::new_for_test(request(tree, value));
        warm.load_program().expect("restore tree");
        let facts =
            nia_compiler_query::LoaderFactProvider::provider_facts(&warm).expect("provider facts");
        let demand = facts
            .demands()
            .iter()
            .find(|demand| {
                matches!(
                    demand.request,
                    nia_compiler_query::ProviderRequest::ModuleBody { .. }
                )
            })
            .expect("each snapshot must remain reusable after loading another tree");
        let nia_compiler_query::ProviderRequest::ModuleBody { module_path } = &demand.request
        else {
            unreachable!()
        };
        assert_eq!(
            normalized_test_path(module_path.as_str()),
            normalized_test_path(root.join(tree).join("helper.nia").to_string_lossy())
        );
    }
}

#[test]
fn provider_demand_plan_remaps_logical_toolchain_paths_after_relocation() {
    let root = temp_dir("provider_demand_plan_remaps_relocated_toolchain");
    let cache_root = root.join("cache");
    let main_path = root.join("main.nia");
    write(&main_path, "using std::unicode;");
    let first_toolchain = provider_plan_toolchain(&root.join("first"));
    let second_toolchain = provider_plan_toolchain(&root.join("second"));
    let request = |toolchain: Arc<nia_toolchain::ToolchainLayout>| {
        LoadRequest::new(main_path.to_string_lossy().into_owned())
            .with_frontend_cache_dir(Some(cache_root.clone()))
            .with_toolchain_layout(toolchain)
    };

    let cold = LoaderDatabase::new_for_test(request(Arc::clone(&first_toolchain)));
    let program = cold.load_program().expect("cold program load");
    let first_unicode = program
        .modules
        .iter()
        .find(|module| module.path.identity().normalized_path() == "toolchain:/std/unicode.nia")
        .expect("cold unicode module")
        .path
        .clone();
    let demand = ProviderDemand {
        source_path: first_unicode.clone(),
        request: nia_compiler_query::ProviderRequest::ModuleBody {
            module_path: first_unicode,
        },
    };
    cold.update_provider_demands([demand])
        .expect("cold provider update");
    nia_compiler_query::LoaderFactProvider::settle_provider_demands(&cold)
        .expect("settle cold provider demands");

    let warm = LoaderDatabase::new_for_test(request(Arc::clone(&second_toolchain)));
    warm.load_program().expect("warm relocated program load");
    let warm_facts =
        nia_compiler_query::LoaderFactProvider::provider_facts(&warm).expect("warm provider facts");
    let restored = warm_facts
        .demands()
        .iter()
        .find(|demand| {
            matches!(
                demand.request,
                nia_compiler_query::ProviderRequest::ModuleBody { .. }
            )
        })
        .expect("restored module-body demand");
    let expected = second_toolchain.resource_root().join("std/unicode.nia");
    assert_eq!(
        normalized_test_path(restored.source_path.as_str()),
        normalized_test_path(expected.to_string_lossy())
    );
    let nia_compiler_query::ProviderRequest::ModuleBody { module_path } = &restored.request else {
        unreachable!();
    };
    assert_eq!(
        normalized_test_path(module_path.as_str()),
        normalized_test_path(expected.to_string_lossy())
    );
    assert!(
        !restored
            .source_path
            .as_str()
            .starts_with(first_toolchain.resource_root().to_string_lossy().as_ref())
    );
}

/// Loads `main.nia` with `inputs`, records a demand for the std module it
/// uses, settles it, and returns whether a fresh loader with the same cache
/// then restores that demand from the persisted plan.
fn plan_round_trip(
    root: &Path,
    cold: impl Fn(LoadRequest) -> LoadRequest,
    warm: impl Fn(LoadRequest) -> LoadRequest,
) -> bool {
    let cache_root = root.join("cache");
    let main_path = root.join("main.nia");
    write(&main_path, "using std::unicode;");
    let toolchain = provider_plan_toolchain(&root.join("toolchain"));
    let request = || {
        LoadRequest::new(main_path.to_string_lossy().into_owned())
            .with_frontend_cache_dir(Some(cache_root.clone()))
            .with_toolchain_layout(Arc::clone(&toolchain))
    };
    let loader = LoaderDatabase::new_for_test(cold(request()));
    let program = loader.load_program().expect("cold program load");
    let unicode = program
        .modules
        .iter()
        .find(|module| module.path.identity().normalized_path() == "toolchain:/std/unicode.nia")
        .expect("unicode module")
        .path
        .clone();
    loader
        .update_provider_demands([ProviderDemand {
            source_path: unicode.clone(),
            request: nia_compiler_query::ProviderRequest::ModuleBody {
                module_path: unicode,
            },
        }])
        .expect("cold provider update");
    nia_compiler_query::LoaderFactProvider::settle_provider_demands(&loader)
        .expect("settle cold provider demands");

    let restored = LoaderDatabase::new_for_test(warm(request()));
    restored.load_program().expect("warm program load");
    nia_compiler_query::LoaderFactProvider::provider_facts(&restored)
        .expect("warm provider facts")
        .demands()
        .iter()
        .any(|demand| {
            matches!(
                demand.request,
                nia_compiler_query::ProviderRequest::ModuleBody { .. }
            )
        })
}

/// A plan was once looked up under the default profile and mode but published
/// under the load's own, so a test-mode or release load read its own plan as
/// corrupt and deleted it on every build.
#[test]
fn provider_demand_plans_restore_under_every_loading_input() {
    let level = nia_target::CpuFeatures::for_level(
        TargetConfig::host().expect("tests run on a maintained host"),
        nia_target::CpuLevel::Baseline,
    )
    .expect("named level");
    type Inputs = fn(LoadRequest) -> LoadRequest;
    let cases: [(&str, Inputs); 4] = [
        ("default", |request| request),
        ("test mode", |request| {
            request.with_compilation_mode(CompilationMode::Test)
        }),
        ("release", |request| {
            request.with_profile(BuildProfile::Release)
        }),
        ("test release", |request| {
            request
                .with_profile(BuildProfile::Release)
                .with_compilation_mode(CompilationMode::Test)
        }),
    ];
    for (name, inputs) in cases {
        let root = temp_dir(&format!("provider_plan_inputs_{}", name.replace(' ', "_")));
        assert!(plan_round_trip(&root, inputs, inputs), "{name}");
    }
    let root = temp_dir("provider_plan_inputs_cpu");
    let with_level = move |request: LoadRequest| request.with_cpu_features(level);
    assert!(
        plan_round_trip(&root, with_level, with_level),
        "explicit CPU features"
    );
}

/// Pruning depends on the mode, so a plan from one mode describes another
/// mode's program wrongly; loads that differ only in mode never share one.
#[test]
fn provider_demand_plans_are_not_shared_across_modes() {
    let root = temp_dir("provider_plan_modes");
    assert!(!plan_round_trip(
        &root,
        |request| request.with_compilation_mode(CompilationMode::Test),
        |request| request,
    ));
}

#[test]
fn provider_demand_plan_remaps_runtime_start_after_toolchain_relocation() {
    let root = temp_dir("provider_demand_plan_remaps_relocated_runtime");
    let cache_root = root.join("cache");
    let main_path = root.join("main.nia");
    write(&main_path, "fun main() i32 { 0 }");
    let first_toolchain = provider_plan_toolchain(&root.join("first"));
    let second_toolchain = provider_plan_toolchain(&root.join("second"));
    let request = |toolchain: Arc<nia_toolchain::ToolchainLayout>| {
        let runtime = RuntimeSpec::freestanding(&toolchain, toolchain.artifact_target())
            .expect("host freestanding runtime");
        LoadRequest::new(main_path.to_string_lossy().into_owned())
            .with_runtime(runtime)
            .with_frontend_cache_dir(Some(cache_root.clone()))
            .with_toolchain_layout(toolchain)
    };

    let cold = LoaderDatabase::new_for_test(request(Arc::clone(&first_toolchain)));
    let program = cold.load_program().expect("cold program load");
    let first_start = program
        .modules
        .iter()
        .find(|module| module.path.identity().normalized_path() == "toolchain:/runtime/start.nia")
        .expect("cold runtime start module")
        .path
        .clone();
    cold.update_provider_demands([ProviderDemand {
        source_path: first_start.clone(),
        request: nia_compiler_query::ProviderRequest::ModuleBody {
            module_path: first_start,
        },
    }])
    .expect("cold runtime provider update");
    nia_compiler_query::LoaderFactProvider::settle_provider_demands(&cold)
        .expect("settle cold runtime provider demands");

    let warm = LoaderDatabase::new_for_test(request(Arc::clone(&second_toolchain)));
    warm.load_program().expect("warm relocated program load");
    let warm_facts =
        nia_compiler_query::LoaderFactProvider::provider_facts(&warm).expect("warm provider facts");
    let restored = warm_facts
        .demands()
        .iter()
        .find(|demand| {
            matches!(
                demand.request,
                nia_compiler_query::ProviderRequest::ModuleBody { .. }
            ) && demand.source_path.identity().normalized_path() == "toolchain:/runtime/start.nia"
        })
        .expect("restored runtime module-body demand");
    let expected = second_toolchain
        .freestanding_runtime_package()
        .root_file()
        .with_file_name("start.nia");
    assert_eq!(
        normalized_test_path(restored.source_path.as_str()),
        normalized_test_path(expected.to_string_lossy())
    );
    let nia_compiler_query::ProviderRequest::ModuleBody { module_path } = &restored.request else {
        unreachable!();
    };
    assert_eq!(
        normalized_test_path(module_path.as_str()),
        normalized_test_path(expected.to_string_lossy())
    );
}

#[test]
fn persistent_provider_demand_plan_restores_current_symbols_and_full_snapshot() {
    let root = temp_dir("persistent_provider_demand_plan_restores_full_snapshot");
    let cache_root = root.join("cache");
    let main_path = root.join("main.nia");
    write(&main_path, "fun main() i32 { 0 }");
    let entry = SourcePath::new(main_path.to_string_lossy());
    let request = || {
        test_request(main_path.to_string_lossy().into_owned())
            .with_frontend_cache_dir(Some(cache_root.clone()))
    };

    let cold = LoaderDatabase::new_for_test(request());
    let method = cold
        .db
        .context()
        .symbols
        .intern("cached_method")
        .expect("intern method");
    let target = cold
        .db
        .context()
        .symbols
        .intern("CachedTarget")
        .expect("intern target");
    let trait_name = cold
        .db
        .context()
        .symbols
        .intern("CachedTrait")
        .expect("intern trait");
    let trait_argument = cold
        .db
        .context()
        .symbols
        .intern("CachedArgument")
        .expect("intern trait argument");
    let demands = HashSet::from([
        ProviderDemand {
            source_path: entry.clone(),
            request: nia_compiler_query::ProviderRequest::Method {
                target_type_name: Some(target),
                target_package: nia_symbol::known::STD,
                target_module_segments: Vec::new(),
                method_name: method,
            },
        },
        ProviderDemand {
            source_path: entry.clone(),
            request: nia_compiler_query::ProviderRequest::TraitImpl {
                target_type_name: Some(target),
                target_package: None,
                target_module_segments: Vec::new(),
                trait_name,
                trait_package: None,
                trait_type_argument_names: vec![Some(trait_argument), None],
            },
        },
        ProviderDemand {
            source_path: entry.clone(),
            request: nia_compiler_query::ProviderRequest::ModuleSemantic {
                module_path: entry.clone(),
            },
        },
        ProviderDemand {
            source_path: entry.clone(),
            request: nia_compiler_query::ProviderRequest::ModuleBody {
                module_path: entry.clone(),
            },
        },
    ]);
    cold.load_program().expect("cold program load");
    cold.update_provider_demands(demands.iter().cloned())
        .expect("initial provider update");
    assert_eq!(
        cold.update_provider_demands(demands.iter().cloned())
            .expect("stable provider update"),
        ProviderGraphUpdate::Stable
    );
    nia_compiler_query::LoaderFactProvider::settle_provider_demands(&cold)
        .expect("settle provider demands");
    let plan_path = persisted_plan_path(&cold);
    assert!(plan_path.is_file());

    let warm = LoaderDatabase::new_for_test(request());
    warm.load_program().expect("warm program load");
    assert_eq!(
        nia_compiler_query::LoaderFactProvider::provider_facts(&warm)
            .expect("warm provider facts")
            .demands(),
        &demands
    );
    assert_eq!(
        warm.db.context().symbols.resolve(method).as_deref(),
        Some("cached_method")
    );
    assert_eq!(
        warm.db.context().symbols.resolve(target).as_deref(),
        Some("CachedTarget")
    );
    assert_eq!(
        warm.db.context().symbols.resolve(trait_name).as_deref(),
        Some("CachedTrait")
    );

    write(&main_path, "fun main() i32 { 1 }");
    let invalidated = LoaderDatabase::new_for_test(request());
    assert!(
        nia_compiler_query::LoaderFactProvider::provider_facts(&invalidated)
            .expect("invalidated provider facts")
            .demands()
            .is_empty()
    );
    assert!(
        plan_path.is_file(),
        "a snapshot mismatch must not retire another tree's reusable plan"
    );
    write(&main_path, "fun main() i32 { 0 }");
    let restored = LoaderDatabase::new_for_test(request());
    restored.load_program().expect("restore original snapshot");
    assert_eq!(
        nia_compiler_query::LoaderFactProvider::provider_facts(&restored)
            .expect("original provider facts")
            .demands(),
        &demands
    );
}

#[test]
fn provider_demand_plan_verification_replaces_semantically_wrong_artifact() {
    let root = temp_dir("provider_demand_plan_verification_replaces_wrong_artifact");
    let cache_root = root.join("cache");
    let main_path = root.join("main.nia");
    write(&main_path, "fun main() i32 { 0 }");
    let entry = SourcePath::new(main_path.to_string_lossy());
    let request = |verify| {
        test_request(main_path.to_string_lossy().into_owned())
            .with_frontend_cache_dir(Some(cache_root.clone()))
            .with_frontend_cache_verification(verify)
    };

    let seeded = LoaderDatabase::new_for_test(request(false));
    let fake_method = seeded
        .db
        .context()
        .symbols
        .intern("not_actually_demanded")
        .expect("intern fake method");
    let fake = ProviderDemand {
        source_path: entry,
        request: nia_compiler_query::ProviderRequest::Method {
            target_type_name: None,
            target_package: nia_symbol::known::STD,
            target_module_segments: Vec::new(),
            method_name: fake_method,
        },
    };
    seeded.load_program().expect("seed program load");
    assert_eq!(
        seeded
            .update_provider_demands([fake.clone()])
            .expect("seed provider update"),
        ProviderGraphUpdate::Stable
    );
    nia_compiler_query::LoaderFactProvider::settle_provider_demands(&seeded)
        .expect("settle provider demands");

    let verified_loader = LoaderDatabase::new_for_test(request(true));
    assert!(
        nia_compiler_query::LoaderFactProvider::provider_facts(&verified_loader)
            .expect("verified provider facts")
            .demands()
            .is_empty(),
        "verification must not inject the candidate plan"
    );
    let compiler = CompilerDatabase::new(
        CompileRequest::new(verified_loader.clone())
            .with_frontend_cache_dir(Some(cache_root.clone()))
            .with_frontend_cache_verification(true),
    )
    .expect("create compiler database");
    let checked = compiler.check_program().expect("verified compiler check");
    assert!(!has_error_diagnostics(&checked.diagnostics));
    assert!(
        !nia_compiler_query::LoaderFactProvider::provider_facts(&verified_loader)
            .expect("verified provider facts")
            .demands()
            .contains(&fake)
    );

    let warm = LoaderDatabase::new_for_test(request(false));
    assert!(
        !nia_compiler_query::LoaderFactProvider::provider_facts(&warm)
            .expect("warm provider facts")
            .demands()
            .contains(&fake),
        "verification must replace the wrong artifact instead of retaining it"
    );
}

#[test]
fn corrupt_provider_demand_plan_is_physically_retired() {
    let root = temp_dir("corrupt_provider_demand_plan_is_retired");
    let cache_root = root.join("cache");
    let main_path = root.join("main.nia");
    write(&main_path, "fun main() i32 { 0 }");
    let request = || {
        test_request(main_path.to_string_lossy().into_owned())
            .with_frontend_cache_dir(Some(cache_root.clone()))
    };
    let seeded = LoaderDatabase::new_for_test(request());
    let method_name = seeded
        .db
        .context()
        .symbols
        .intern("cached_method")
        .expect("intern method");
    let demand = ProviderDemand {
        source_path: SourcePath::new(main_path.to_string_lossy()),
        request: nia_compiler_query::ProviderRequest::Method {
            target_type_name: None,
            target_package: nia_symbol::known::STD,
            target_module_segments: Vec::new(),
            method_name,
        },
    };
    seeded.load_program().expect("seed program load");
    assert_eq!(
        seeded
            .update_provider_demands([demand])
            .expect("seed provider update"),
        ProviderGraphUpdate::Stable
    );
    nia_compiler_query::LoaderFactProvider::settle_provider_demands(&seeded)
        .expect("settle provider demands");
    let plan_path = persisted_plan_path(&seeded);
    assert!(plan_path.is_file());
    fs::write(&plan_path, b"corrupt").expect("corrupt provider plan");

    let loaded = LoaderDatabase::new_for_test(request());
    assert!(
        nia_compiler_query::LoaderFactProvider::provider_facts(&loaded)
            .expect("loaded provider facts")
            .demands()
            .is_empty()
    );
    assert!(!plan_path.exists());
}
