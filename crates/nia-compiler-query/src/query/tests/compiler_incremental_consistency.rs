// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn semantic_provider_activation_preserves_resolved_caller_facts() {
    let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let entry_id = fixture.entry_id();
    let revision = crate::ProviderFactRevision::new_store();
    let mut program = fixture.program();
    program.provider_fact_revision = revision;
    let database = CompilerDatabase::new(CompileRequest::new(program));
    let _ = database.executable_provider_demands();
    fixture.add_child(
        entry_id,
        "provider",
        "main/provider.nia",
        "pub fn value() i32 { 1 }",
    );
    let provider_change = crate::ProviderDemand {
        source_path: SourcePath::new("main.nia"),
        request: crate::ProviderRequest::ModuleSemantic {
            module_path: SourcePath::new("main/provider.nia"),
        },
    };
    let checked_function = {
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
        let checked_function = *state
            .checked_functions
            .iter()
            .next()
            .expect("checked entry function");
        state
            .provider_demands_by_function
            .entry(checked_function)
            .or_default()
            .insert(provider_change.clone());
        state.provider_demands.insert(provider_change.clone());
        checked_function
    };

    database.update(CompileRequest::new(fixture.program()));
    database.replace_provider_facts(crate::ProviderFactSnapshot::new(
        revision.next(),
        revision,
        [provider_change],
    ));
    let _ = database.executable_provider_demands();

    let session = database
        .db
        .context()
        .executable_fact_session
        .lock()
        .expect("executable fact session lock poisoned");
    let state = session
        .modules
        .get(&entry_id)
        .expect("preserved entry executable facts");
    assert!(state.checked_functions.contains(&checked_function));
    assert_eq!(
        session.applied_provider_fact_revision,
        Some(revision.next())
    );
    assert!(
        session
            .caches
            .body_resolution_inputs
            .borrow()
            .contains_key(&entry_id)
    );
}

#[test]
fn method_provider_change_removes_only_affected_function_diagnostics() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "struct Value {} fn helper() i32 { 1 } fn main(value: Value) i32 { value.missing() }",
    );
    let entry_id = fixture.entry_id();
    let revision = crate::ProviderFactRevision::new_store();
    let mut program = fixture.program();
    program.provider_fact_revision = revision;
    let database = CompilerDatabase::new(CompileRequest::new(program));
    let provider_changes = database
        .executable_provider_demands()
        .expect("test executable provider demands")
        .into_iter()
        .filter(|demand| matches!(demand.request, crate::ProviderRequest::Method { .. }))
        .collect::<Vec<_>>();
    assert!(!provider_changes.is_empty());
    let (affected_function, unaffected_function) = {
        let session = database
            .db
            .context()
            .executable_fact_session
            .lock()
            .expect("executable fact session lock poisoned");
        let state = session.modules.get(&entry_id).expect("entry facts");
        assert!(!state.diagnostics.is_empty());
        let affected = *state
            .provider_demands_by_function
            .iter()
            .find(|(_, demands)| {
                demands
                    .iter()
                    .any(|demand| provider_changes.contains(demand))
            })
            .map(|(function, _)| function)
            .expect("function-owned method demand");
        let unaffected = *state
            .checked_functions
            .iter()
            .find(|function| **function != affected)
            .expect("unaffected helper function");
        (affected, unaffected)
    };

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
    let worklist = database.db.expect_get(ProviderFactWorklistQuery);

    let mut session = database
        .db
        .context()
        .executable_fact_session
        .lock()
        .expect("executable fact session lock poisoned");
    session.apply_provider_fact_worklist(&worklist, &database.db.context().type_store);
    let state = session
        .modules
        .get(&entry_id)
        .expect("partially retained entry facts");
    assert!(!state.checked_functions.contains(&affected_function));
    assert!(state.checked_functions.contains(&unaffected_function));
    assert!(state.diagnostics.is_empty(), "{:?}", state.diagnostics);
    assert_eq!(state.diagnostic_owners.len(), state.diagnostics.len());
    assert!(
        session
            .caches
            .body_resolution_inputs
            .borrow()
            .contains_key(&entry_id)
    );
}

#[test]
fn randomized_incremental_checks_match_clean_recomputation() {
    #[derive(Debug, PartialEq)]
    struct ObservableCheck {
        diagnostics: Vec<ProgramDiagnostic>,
        modules: Vec<(String, usize, usize, usize, usize)>,
    }

    fn observable(program: CheckedProgramAnalysis) -> ObservableCheck {
        ObservableCheck {
            diagnostics: program.diagnostics,
            modules: program
                .modules
                .iter()
                .map(|module| {
                    (
                        module.path.as_str().to_owned(),
                        module.defs.defs.iter().count(),
                        module.body_ir.function_bodies.len(),
                        module.semantic_facts.function_facts.len(),
                        module.provider_demands.len(),
                    )
                })
                .collect(),
        }
    }

    let sources = [
        "fn main() i32 { 0 }",
        "fn main() i32 { 1 }",
        "fn helper() i32 { 2 } fn main() i32 { helper() }",
        "struct Value { field: i32 } fn main() i32 { let value = Value { field: 3 }; value.field }",
        "fn main() i32 { true }",
        "fn main() i32 { let value: i32 = 4; value }",
        "const answer: i32 = 5; fn main() i32 { answer }",
        "fn main() i32 { missing() }",
    ];
    let mut fixture = LoadedProgramFixture::new("main.nia", sources[0]);
    let module_id = fixture.entry_id();
    let incremental = fixture.database();
    let mut random = 0x9e37_79b9_u32;

    for revision in 1..=24_u64 {
        random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let source = sources[(random as usize) % sources.len()];
        fixture.update_module_source(module_id, source, SourceRevision(revision));
        incremental.update(CompileRequest::new(fixture.program()));
        let incremental_output = observable(incremental.analyze_program());

        let clean_fixture = LoadedProgramFixture::new("main.nia", source);
        let clean_output = observable(clean_fixture.database().analyze_program());

        assert_eq!(
            incremental_output, clean_output,
            "incremental/clean mismatch at revision {revision} for `{source}`"
        );
    }
}

#[test]
fn source_identity_change_invalidates_loaded_module_list() {
    let mut fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let module_id = fixture.entry_id();
    let database = fixture.database();
    let _ = database.check_program();

    fixture.update_module_path(module_id, "other.nia");
    database.update(CompileRequest::new(fixture.program()));
    let loaded = database.db.expect_get(LoadedModulesQuery);
    assert_eq!(
        resolve_stable_module_sequence(&database.db, &loaded).expect("renamed module sequence"),
        vec![module_id]
    );
    assert_eq!(
        database.db.expect_get(ModulePathQuery(module_id)).as_str(),
        "other.nia"
    );
}
