// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn provider_graph_growth_recomputes_query_derived_executable_roots() {
    let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let entry_id = fixture.entry_id();
    let provider_id = fixture.add_shallow_child(
        entry_id,
        "provider",
        "main/provider.nia",
        "pub fn value() i32 { 1 }",
    );
    let loaded = fixture.program();
    let database = CompilerDatabase::new(CompileRequest::new(loaded.clone()));
    assert_eq!(
        database.db.expect_get(ExecutableRootModulesQuery).as_ref(),
        &(entry_id, Vec::new())
    );
    let _ = database.db.expect_get(TypeResolutionQuery(entry_id));

    let mut grown = loaded;
    let mut graph = (*grown.graph).clone();
    assert!(graph.mark_semantic_selected(provider_id));
    grown.graph = graph.into();
    let before_update = database.query_trace();
    database.update(CompileRequest::new(grown));
    assert_eq!(
        database.db.expect_get(ExecutableRootModulesQuery).as_ref(),
        &(entry_id, Vec::new())
    );
    let after_update = database.query_trace();
    assert_query_executions_unchanged(&before_update, &after_update, "type_resolution");
    assert!(
        query_executions(&before_update, "executable_root_modules")
            < query_executions(&after_update, "executable_root_modules")
    );
}

#[test]
fn additive_provider_graph_growth_reuses_existing_executable_facts() {
    let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let entry_id = fixture.entry_id();
    let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));

    let _ = database.executable_provider_demands();
    {
        let session = database
            .db
            .context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned");
        assert!(session.modules.contains_key(&entry_id));
        assert!(
            session
                .caches
                .body_resolution_inputs
                .borrow()
                .contains_key(&entry_id)
        );
    }

    fixture.add_child(
        entry_id,
        "provider",
        "main/provider.nia",
        "pub fn value() i32 { 1 }",
    );
    database.update(CompileRequest::new(fixture.program()));
    let session = database
        .db
        .context()
        .executable_fact_session
        .lock()
        .expect("executable fact session lock poisoned");
    assert!(session.modules.contains_key(&entry_id));
    assert!(
        session
            .caches
            .body_resolution_inputs
            .borrow()
            .contains_key(&entry_id)
    );
}

#[test]
fn provider_changes_discard_affected_executable_fact_caches() {
    let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let entry_id = fixture.entry_id();
    let revision = crate::ProviderFactRevision::new_store();
    let mut program = fixture.program();
    program.provider_fact_revision = revision;
    let database = CompilerDatabase::new(CompileRequest::new(program));
    let _ = database.executable_provider_demands();
    let provider_changes = vec![crate::ProviderDemand {
        source_path: SourcePath::new("main.nia"),
        request: crate::ProviderRequest::Method {
            target_type_name: None,
            method_name: SymbolId::default(),
        },
    }];
    {
        let mut session = database
            .db
            .context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned");
        let state = session
            .modules
            .get_mut(&entry_id)
            .expect("entry executable facts");
        state
            .unowned_provider_demands
            .insert(provider_changes[0].clone());
        state.provider_demands.insert(provider_changes[0].clone());
    }

    fixture.add_child(
        entry_id,
        "provider",
        "main/provider.nia",
        "pub fn value() i32 { 1 }",
    );
    database.update(CompileRequest::new(fixture.program()));
    database.replace_provider_facts(crate::ProviderFactSnapshot::new(
        revision.next(),
        revision,
        provider_changes,
    ));

    {
        let session = database
            .db
            .context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned");
        assert!(session.modules.contains_key(&entry_id));
        assert_eq!(session.applied_provider_fact_revision, Some(revision));
    }
    let worklist = database.db.expect_get(ProviderFactWorklistQuery);
    let mut session = database
        .db
        .context()
        .executable_fact_session
        .lock()
        .expect("executable fact session lock poisoned");
    session.apply_provider_fact_worklist(&worklist, &database.db.context().type_store);
    assert!(!session.modules.contains_key(&entry_id));
    assert!(
        !session
            .caches
            .body_resolution_inputs
            .borrow()
            .contains_key(&entry_id)
    );
    assert_eq!(
        session.applied_provider_fact_revision,
        Some(revision.next())
    );
}

#[test]
fn provider_fact_snapshot_deduplicates_demands() {
    let revision = crate::ProviderFactRevision::new_store();
    let demand = crate::ProviderDemand {
        source_path: SourcePath::new("main.nia"),
        request: crate::ProviderRequest::Method {
            target_type_name: None,
            method_name: SymbolId::default(),
        },
    };
    let facts = crate::ProviderFactSnapshot::new(revision, revision, [demand.clone(), demand]);
    assert_eq!(facts.demands().len(), 1);
}

#[test]
fn check_certificate_input_covers_stable_graph_and_provider_demands() {
    let mut public_graph = LoadedProgramFixture::new("main.nia", "module child;");
    let public_entry = public_graph.entry_id();
    public_graph.add_child_with_visibility(
        public_entry,
        "child",
        nia_ids::Visibility::Public,
        "child.nia",
        "pub fn value() i32 { 1 }",
    );
    let mut private_graph = LoadedProgramFixture::new("main.nia", "module child;");
    let private_entry = private_graph.entry_id();
    private_graph.add_child_with_visibility(
        private_entry,
        "child",
        nia_ids::Visibility::Private,
        "child.nia",
        "pub fn value() i32 { 1 }",
    );
    let program_sources =
        crate::frontend_program_source_fingerprint(public_graph.graph.modules().map(|module| {
            (
                &module.stable_key,
                crate::source_content_fingerprint("same exact source"),
                17,
            )
        }));
    let revision = crate::ProviderFactRevision::new_store();
    let empty = crate::ProviderFactSnapshot::empty(revision);
    let public = check_certificate_input_fingerprint(
        program_sources,
        &public_graph.graph.clone().into(),
        &empty,
    );
    let private = check_certificate_input_fingerprint(
        program_sources,
        &private_graph.graph.clone().into(),
        &empty,
    );
    assert_ne!(public, private);

    let demanded = crate::ProviderFactSnapshot::new(
        revision,
        revision,
        [crate::ProviderDemand {
            source_path: SourcePath::new("child.nia"),
            request: crate::ProviderRequest::ModuleBody {
                module_path: SourcePath::new("child.nia"),
            },
        }],
    );
    assert_ne!(
        public,
        check_certificate_input_fingerprint(program_sources, &public_graph.graph.into(), &demanded,)
    );
}

#[test]
fn compiler_inputs_preserve_provider_fact_revision() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let mut program = fixture.program();
    let revision = crate::ProviderFactRevision::new_store().next();
    program.provider_fact_revision = revision;
    let database = CompilerDatabase::new(CompileRequest::new(program));

    assert_eq!(
        database.provider_fact_revision().expect("revision"),
        revision
    );
}

#[test]
fn executable_products_depend_on_incremental_worklists() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let revision = crate::ProviderFactRevision::new_store();
    let mut program = fixture.program();
    program.provider_fact_revision = revision;
    let database = CompilerDatabase::new(CompileRequest::new(program));
    assert_eq!(std::mem::size_of::<ProviderFactWorklistQuery>(), 0);
    assert_eq!(std::mem::size_of::<BodyActivationWorklistQuery>(), 0);
    assert_eq!(std::mem::size_of::<ExecutableFactEpochQuery>(), 0);

    let _ = database.executable_provider_demands();
    let modules = database.db.expect_get(ExecutableCheckedModulesQuery);
    assert!(!modules.is_empty());
    assert_eq!(
        database.provider_fact_revision().expect("revision"),
        revision
    );
    assert_eq!(
        database
            .db
            .context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned")
            .applied_provider_fact_revision,
        Some(revision)
    );

    let dependencies = &database.query_trace().dependencies;
    for product in [
        "executable_provider_demands",
        "executable_checked_module_facts",
    ] {
        for worklist in ["body_activation_worklist", "provider_fact_worklist"] {
            assert!(dependencies.iter().any(|dependency| {
                dependency.from.name == product && dependency.to.name == worklist
            }));
        }
        assert!(dependencies.iter().any(|dependency| {
            dependency.from.name == product && dependency.to.name == "executable_fact_epoch"
        }));
    }
    assert!(dependencies.iter().any(|dependency| {
        dependency.from.name == "executable_checked_modules"
            && dependency.to.name == "executable_checked_module_facts"
    }));
    assert!(dependencies.iter().any(|dependency| {
        dependency.from.name == "provider_fact_revision"
            && dependency.to.name == "provider_fact_worklist"
    }));
}

#[test]
fn executable_products_serialize_the_shared_fact_session() {
    let fixture = LoadedProgramFixture::new("main.nia", "pub fn main() i32 { 0 }");
    let mut program = fixture.program();
    program.runtime = RuntimeModel::FreestandingExecutable;
    let revision = program.provider_fact_revision;
    let db = query_db(program);

    let (_demands, modules) = std::thread::scope(|scope| {
        let demands = scope.spawn(|| db.expect_get(ExecutableProviderDemandsQuery));
        let modules = scope.spawn(|| db.expect_get(ExecutableCheckedModulesQuery));
        (
            demands.join().expect("provider demand query thread"),
            modules.join().expect("checked modules query thread"),
        )
    });

    assert!(!modules.is_empty());
    assert_eq!(
        db.context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned")
            .applied_provider_fact_revision,
        Some(revision)
    );
}

#[test]
fn provider_worklist_fingerprint_is_deterministic_and_order_independent() {
    let revision = crate::ProviderFactRevision::new_store().next();
    let method = crate::ProviderDemand {
        source_path: SourcePath::new("main.nia"),
        request: crate::ProviderRequest::Method {
            target_type_name: Some(sym("Thing")),
            method_name: sym("run"),
        },
    };
    let trait_impl = crate::ProviderDemand {
        source_path: SourcePath::new("provider.nia"),
        request: crate::ProviderRequest::TraitImpl {
            trait_name: sym("Display"),
        },
    };
    let first_provider =
        crate::ProviderFactSnapshot::new(revision, revision, [method.clone(), trait_impl.clone()]);
    let mut reversed_changes = HashSet::new();
    reversed_changes.insert(trait_impl);
    reversed_changes.insert(method);
    let second_provider = crate::ProviderFactSnapshot::new(revision, revision, reversed_changes);
    assert_eq!(
        provider_fact_worklist_fingerprint(&first_provider),
        provider_fact_worklist_fingerprint(&second_provider)
    );
}

#[test]
fn executable_fact_epoch_defers_full_reset_to_query_boundary() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let database = fixture.database();
    let first_epoch = database.db.expect_get(ExecutableFactEpochQuery);
    let _ = database.db.expect_get(ExecutableCheckedModulesQuery);
    let _ = database.executable_provider_demands();
    let sentinel = crate::ProviderDemand {
        source_path: SourcePath::new("main.nia"),
        request: crate::ProviderRequest::Method {
            target_type_name: None,
            method_name: SymbolId::default(),
        },
    };
    {
        let mut session = database
            .db
            .context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned");
        assert_eq!(session.epoch.as_ref(), Some(first_epoch.as_ref()));
        session.applied_provider_changes.insert(sentinel.clone());
    }

    let mut reset = fixture.program();
    reset.runtime = RuntimeModel::FreestandingExecutable;
    database.update(CompileRequest::new(reset));
    {
        let session = database
            .db
            .context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned");
        assert_eq!(session.epoch.as_ref(), Some(first_epoch.as_ref()));
        assert!(session.applied_provider_changes.contains(&sentinel));
    }

    let _ = database.executable_provider_demands();
    let latest_epoch = database.db.expect_get(ExecutableFactEpochQuery);
    let session = database
        .db
        .context()
        .executable_fact_session
        .lock()
        .expect("executable fact session lock poisoned");
    assert_ne!(first_epoch.as_ref(), latest_epoch.as_ref());
    assert_eq!(session.epoch.as_ref(), Some(latest_epoch.as_ref()));
    assert!(!session.applied_provider_changes.contains(&sentinel));
}

#[test]
fn provider_revision_update_invalidates_executable_products() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let revision = crate::ProviderFactRevision::new_store();
    let mut program = fixture.program();
    program.provider_fact_revision = revision;
    let database = CompilerDatabase::new(CompileRequest::new(program));
    let _ = database.executable_provider_demands();
    let first_set = database.db.expect_get(ExecutableCheckedModulesQuery);
    assert_eq!(
        database.provider_fact_revision().expect("revision"),
        revision
    );

    let invalidation = database.replace_provider_facts(crate::ProviderFactSnapshot::new(
        revision.next(),
        revision,
        std::iter::empty(),
    ));
    let invalidated = invalidation
        .invalidated
        .iter()
        .map(|frame| frame.name)
        .collect::<Vec<_>>();

    for name in [
        "provider_fact_revision",
        "provider_fact_worklist",
        "executable_provider_demands",
        "executable_checked_modules",
    ] {
        assert!(invalidated.contains(&name), "{invalidated:?}");
    }
    assert!(
        !invalidated.contains(&"body_activation_worklist"),
        "{invalidated:?}"
    );
    assert_eq!(
        database.provider_fact_revision().expect("updated revision"),
        revision.next()
    );
    let revision_query = database
        .query_trace()
        .queries
        .into_iter()
        .find(|query| query.frame.name == "provider_fact_revision")
        .expect("provider fact revision query trace");
    assert_eq!(revision_query.stats.validations, 1);
    assert_eq!(revision_query.stats.green_validations, 0);
    let second_set = database.db.expect_get(ExecutableCheckedModulesQuery);
    assert!(!Arc::ptr_eq(&first_set, &second_set));
    assert!(!second_set.is_empty());
}

#[test]
fn provider_worklist_accumulates_until_consumed() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let revision = crate::ProviderFactRevision::new_store();
    let mut program = fixture.program();
    program.provider_fact_revision = revision;
    let database = CompilerDatabase::new(CompileRequest::new(program));
    let first_demand = crate::ProviderDemand {
        source_path: SourcePath::new("main.nia"),
        request: crate::ProviderRequest::Method {
            target_type_name: None,
            method_name: SymbolId::default(),
        },
    };
    let second_demand = crate::ProviderDemand {
        source_path: SourcePath::new("main.nia"),
        request: crate::ProviderRequest::TraitImpl {
            trait_name: SymbolId::default(),
        },
    };
    let first_revision = revision.next();
    let second_revision = first_revision.next();

    database.replace_provider_facts(crate::ProviderFactSnapshot::new(
        first_revision,
        revision,
        [first_demand.clone()],
    ));
    database.replace_provider_facts(crate::ProviderFactSnapshot::new(
        second_revision,
        revision,
        [first_demand.clone(), second_demand.clone()],
    ));

    let worklist = database.db.expect_get(ProviderFactWorklistQuery);
    let expected_changes = HashSet::from([first_demand, second_demand]);
    assert_eq!(worklist.revision(), second_revision);
    assert_eq!(worklist.demands(), &expected_changes);

    let mut session = ExecutableFactSession::default();
    session.apply_provider_fact_worklist(&worklist, &database.db.context().type_store);
    assert_eq!(
        session.applied_provider_fact_revision,
        Some(second_revision)
    );
    assert_eq!(session.applied_provider_changes, expected_changes);

    let reset_revision = second_revision.next();
    database.replace_provider_facts(crate::ProviderFactSnapshot::new(
        reset_revision,
        reset_revision,
        std::iter::empty(),
    ));
    let reset = database.db.expect_get(ProviderFactWorklistQuery);
    assert_eq!(reset.revision(), reset_revision);
    assert!(reset.demands().is_empty());
}

#[test]
fn provider_worklist_reset_watermark_survives_skipped_revisions() {
    let initial_revision = crate::ProviderFactRevision::new_store();
    let reset_revision = initial_revision.next();
    let current_revision = reset_revision.next();
    let stale = crate::ProviderDemand {
        source_path: SourcePath::new("stale.nia"),
        request: crate::ProviderRequest::TraitImpl {
            trait_name: sym("Stale"),
        },
    };
    let current = crate::ProviderDemand {
        source_path: SourcePath::new("current.nia"),
        request: crate::ProviderRequest::TraitImpl {
            trait_name: sym("Current"),
        },
    };
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let database = fixture.database();
    let mut session = ExecutableFactSession {
        applied_provider_fact_revision: Some(initial_revision),
        applied_provider_changes: HashSet::from([stale.clone()]),
        ..ExecutableFactSession::default()
    };

    session.apply_provider_fact_worklist(
        &crate::ProviderFactSnapshot::new(current_revision, reset_revision, [current.clone()]),
        &database.db.context().type_store,
    );

    assert_eq!(
        session.applied_provider_fact_revision,
        Some(current_revision)
    );
    assert_eq!(session.applied_provider_changes, HashSet::from([current]));
    assert!(!session.applied_provider_changes.contains(&stale));
}

#[test]
fn body_activation_worklist_accumulates_until_consumed() {
    let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let entry_id = fixture.entry_id();
    let first_module = fixture.add_shallow_child(
        entry_id,
        "first",
        "main/first.nia",
        "pub fn first() i32 { 1 }",
    );
    let second_module = fixture.add_shallow_child(
        entry_id,
        "second",
        "main/second.nia",
        "pub fn second() i32 { 2 }",
    );
    assert!(fixture.graph.mark_semantic_selected(first_module));
    assert!(fixture.graph.mark_semantic_selected(second_module));
    let database = fixture.database();
    let _ = database.executable_provider_demands();

    assert!(fixture.graph.mark_process_used_paths(first_module));
    database.update(CompileRequest::new(fixture.program()));

    assert!(fixture.graph.mark_process_used_paths(second_module));
    database.update(CompileRequest::new(fixture.program()));

    let worklist = database.db.expect_get(BodyActivationWorklistQuery);
    let expected = HashMap::from([
        (
            fixture
                .graph
                .stable_key(entry_id)
                .expect("entry stable key")
                .clone(),
            entry_id,
        ),
        (
            fixture
                .graph
                .stable_key(first_module)
                .expect("first stable key")
                .clone(),
            first_module,
        ),
        (
            fixture
                .graph
                .stable_key(second_module)
                .expect("second stable key")
                .clone(),
            second_module,
        ),
    ]);
    assert_eq!(worklist.modules.as_ref(), &expected);

    let _ = database.executable_provider_demands();
    let session = database
        .db
        .context()
        .executable_fact_session
        .lock()
        .expect("executable fact session lock poisoned");
    assert_eq!(
        session.applied_body_activations,
        expected.keys().cloned().collect()
    );
}

#[test]
fn content_identical_input_replacement_keeps_executable_facts_green() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let revision = crate::ProviderFactRevision::new_store();
    let mut program = fixture.program();
    program.provider_fact_revision = revision;
    let database = CompilerDatabase::new(CompileRequest::new(program));
    let first_set = database.db.expect_get(ExecutableCheckedModulesQuery);
    let _ = database.executable_provider_demands();
    let before_update = database.query_trace();

    let invalidation = database
        .update(CompileRequest::new(fixture.program()).with_timings(crate::TimingMode::Summary));

    assert!(
        invalidation.invalidated.is_empty(),
        "{:?}",
        invalidation.invalidated
    );
    let second_set = database.db.expect_get(ExecutableCheckedModulesQuery);
    let _ = database.executable_provider_demands();
    assert!(Arc::ptr_eq(&first_set, &second_set));
    assert!(!second_set.is_empty());
    let after_reuse = database.query_trace();
    for name in [
        "body_activation_worklist",
        "executable_checked_modules",
        "executable_fact_epoch",
        "executable_provider_demands",
        "provider_fact_worklist",
    ] {
        assert_query_executions_unchanged(&before_update, &after_reuse, name);
    }
}
