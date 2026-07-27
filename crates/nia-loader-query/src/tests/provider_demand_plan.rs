// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn persistent_provider_demand_plan_restores_current_symbols_and_full_snapshot() {
    let root = temp_dir("persistent_provider_demand_plan_restores_full_snapshot");
    let cache_root = root.join("cache");
    let main_path = root.join("main.nia");
    write(&main_path, "fn main() i32 { 0 }");
    let entry = SourcePath::new(main_path.to_string_lossy());
    let request = || {
        LoadRequest::new(main_path.to_string_lossy().into_owned())
            .with_frontend_cache_dir(Some(cache_root.clone()))
    };

    let cold = LoaderDatabase::new(request());
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
    let demands = HashSet::from([
        ProviderDemand {
            source_path: entry.clone(),
            request: nia_compiler_query::ProviderRequest::Method {
                target_type_name: Some(target),
                method_name: method,
            },
        },
        ProviderDemand {
            source_path: entry.clone(),
            request: nia_compiler_query::ProviderRequest::TraitImpl { trait_name },
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
    let plan_path = cold
        .db
        .context()
        .frontend_cache
        .as_ref()
        .expect("frontend cache")
        .provider_demand_plan_path(
            cold.db
                .context()
                .provider_demand_plan_key
                .expect("provider plan key"),
        );
    assert!(plan_path.is_file());

    let warm = LoaderDatabase::new(request());
    assert_eq!(
        nia_compiler_query::LoaderFactProvider::provider_facts(&warm)
            .expect("warm provider facts")
            .demands(),
        &demands
    );
    warm.load_program().expect("warm program load");
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

    write(&main_path, "fn main() i32 { 1 }");
    let invalidated = LoaderDatabase::new(request());
    assert!(
        nia_compiler_query::LoaderFactProvider::provider_facts(&invalidated)
            .expect("invalidated provider facts")
            .demands()
            .is_empty()
    );
    assert!(
        !plan_path.exists(),
        "stale provider plan must be physically retired"
    );
}

#[test]
fn provider_demand_plan_verification_replaces_semantically_wrong_artifact() {
    let root = temp_dir("provider_demand_plan_verification_replaces_wrong_artifact");
    let cache_root = root.join("cache");
    let main_path = root.join("main.nia");
    write(&main_path, "fn main() i32 { 0 }");
    let entry = SourcePath::new(main_path.to_string_lossy());
    let request = |verify| {
        LoadRequest::new(main_path.to_string_lossy().into_owned())
            .with_frontend_cache_dir(Some(cache_root.clone()))
            .with_frontend_cache_verification(verify)
    };

    let seeded = LoaderDatabase::new(request(false));
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

    let verified_loader = LoaderDatabase::new(request(true));
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
    );
    let checked = compiler.check_program().expect("verified compiler check");
    assert!(!has_error_diagnostics(&checked.diagnostics));
    assert!(
        !nia_compiler_query::LoaderFactProvider::provider_facts(&verified_loader)
            .expect("verified provider facts")
            .demands()
            .contains(&fake)
    );

    let warm = LoaderDatabase::new(request(false));
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
    write(&main_path, "fn main() i32 { 0 }");
    let request = || {
        LoadRequest::new(main_path.to_string_lossy().into_owned())
            .with_frontend_cache_dir(Some(cache_root.clone()))
    };
    let seeded = LoaderDatabase::new(request());
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
    let context = seeded.db.context();
    let plan_path = context
        .frontend_cache
        .as_ref()
        .expect("frontend cache")
        .provider_demand_plan_path(
            context
                .provider_demand_plan_key
                .expect("provider demand plan key"),
        );
    assert!(plan_path.is_file());
    fs::write(&plan_path, b"corrupt").expect("corrupt provider plan");

    let loaded = LoaderDatabase::new(request());
    assert!(
        nia_compiler_query::LoaderFactProvider::provider_facts(&loaded)
            .expect("loaded provider facts")
            .demands()
            .is_empty()
    );
    assert!(!plan_path.exists());
}
