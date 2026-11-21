// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn final_function_body_reuses_unchanged_inputs_across_edits() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "pub module lib; pub module values; pub fun main() i32 { lib::read() + 1 }",
    );
    let entry = fixture.entry_id();
    let lib = fixture.add_child(
        entry,
        "lib",
        "lib.nia",
        "using root::values; pub fun read() i32 { values::K }",
    );
    let values = fixture.add_child(entry, "values", "values.nia", "pub const K: i32 = 7;");
    let database = fixture.database();
    let body = || {
        let facts = database.db.expect_get(ExecutableCheckedModuleFactsQuery);
        facts
            .function_bodies
            .iter()
            .find(|(id, _)| id.module_id == lib)
            .unwrap()
            .1
            .clone()
    };
    let initial = body();
    fixture.update_module_source(
        entry,
        "pub module lib; pub module values; pub fun main() i32 { 2 + lib::read() }",
        SourceRevision(1),
    );
    database.update(CompileRequest::new(fixture.program()));
    let edited = body();
    assert!(Arc::ptr_eq(&initial, &edited));
    assert!(
        !database
            .db
            .query_trace()
            .unwrap()
            .queries
            .iter()
            .any(|query| query.frame.name == "executable_provider_demands")
    );
    fixture.update_module_source(values, "pub const K: i32 = 9;", SourceRevision(1));
    database.update(CompileRequest::new(fixture.program()));
    let changed = body();
    assert_ne!(*edited, *changed);
}

#[test]
fn final_static_only_module_retains_its_const_lowering_on_cache_hits() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "pub module data; pub fun main() i32 { data::VALUE }",
    );
    let entry = fixture.entry_id();
    let data = fixture.add_child(entry, "data", "data.nia", "pub static VALUE: i32 = 42;");
    let database = fixture.database();
    let initializer = || {
        let facts = database.db.expect_get(ExecutableCheckedModuleFactsQuery);
        assert!(facts.const_modules.contains_key(&data));
        let global = *facts
            .runtime_globals
            .iter()
            .find(|id| id.module_id == data)
            .unwrap();
        database
            .db
            .expect_get(ExecutableStaticInitQuery(global))
            .as_ref()
            .clone()
            .unwrap()
    };
    let before = initializer();
    fixture.update_module_source(
        entry,
        "pub module data; pub fun main() i32 { data::VALUE + 1 }",
        SourceRevision(1),
    );
    database.update(CompileRequest::new(fixture.program()));
    assert_eq!(before, initializer());
}

#[test]
fn executable_checked_modules_reuse_filtered_const_inputs() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
const fun len() usize {
4
}

fun unused() i32 {
missing_symbol
}

pub fun main() i32 {
let mut values: [i32; len()] = [0; len()];
values[0]
}
"#,
    );
    let module_id = fixture.entry_id();
    let loaded = fixture.freestanding_program();
    let db = query_db(loaded);

    let facts = db.expect_get(ExecutableCheckedModuleFactsQuery);
    assert!(facts.const_modules.contains_key(&module_id));
    assert!(
        facts
            .runtime_functions
            .iter()
            .chain(&facts.runtime_globals)
            .all(|def_id| facts.const_modules.contains_key(&def_id.module_id))
    );
    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module should be executable-reachable");
    let const_len = module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.kind == nia_defs::DefKind::Function && def.name == sym("len"))
                .then_some(GlobalDefId { module_id, def_id })
        })
        .expect("const len function");
    let main = module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.kind == nia_defs::DefKind::Function && def.name == sym("main"))
                .then_some(GlobalDefId { module_id, def_id })
        })
        .expect("main function");
    let value_ref_edges = db.expect_get(ExecutableValueRefEdgesQuery(main));
    assert!(
        !value_ref_edges.functions.contains(&const_len),
        "array length and repeat-count calls must not enter raw runtime value-ref edges: {:?}",
        value_ref_edges.functions
    );
    let trace = db.query_trace().expect("query trace");

    assert!(
        module.body_diagnostics.is_empty(),
        "reachable const functions must remain available to executable body checking: {:?}",
        module.body_diagnostics
    );
    assert!(
        module
            .const_eval
            .array_lengths
            .values()
            .any(|length| *length == 4),
        "filtered executable const phases should retain reachable array lengths"
    );
    assert!(
        !facts.runtime_functions.contains(&const_len),
        "a const-only function must not enter executable reachability: {:?}",
        facts.runtime_functions
    );
    assert!(
        !module.body_ir.function_bodies.contains_key(&const_len),
        "a function used only by const evaluation must not become a runtime body root"
    );
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "executable_body_check"
            && matches!(
                dependency.to.name,
                "const_values" | "const_array_lengths" | "const_typed_facts"
            )
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "executable_body_check"
            && dependency.to.name == "program_trait_solving_signatures"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "executable_checked_modules"
            && matches!(dependency.to.name, "const" | "const_enum_values")
    }));
}

#[test]
fn runtime_function_reference_roots_only_its_const_target() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
const fun referenced(value: i32) i32 {
value * 2 + 1
}

const fun unused(value: i32) i32 {
value - 100
}

pub fun main() i32 {
let callback = & referenced;
callback(12)
}
"#,
    );
    let module_id = fixture.entry_id();
    let loaded = fixture.freestanding_program();
    let db = query_db(loaded);

    let facts = db.expect_get(ExecutableCheckedModuleFactsQuery);
    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module should be executable-reachable");
    let function = |name| {
        module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Function && def.name == sym(name))
                    .then_some(GlobalDefId { module_id, def_id })
            })
            .unwrap_or_else(|| panic!("missing function `{name}`"))
    };
    let main = function("main");
    let referenced = function("referenced");
    let unused = function("unused");

    assert!(
        module.body_diagnostics.is_empty(),
        "runtime function references should type-check: {:?}",
        module.body_diagnostics
    );
    assert!(facts.runtime_functions.contains(&main));
    assert!(facts.runtime_functions.contains(&referenced));
    assert!(!facts.runtime_functions.contains(&unused));
    assert!(module.body_ir.function_bodies.contains_key(&referenced));
    assert!(!module.body_ir.function_bodies.contains_key(&unused));
}

#[test]
fn executable_checked_modules_do_not_body_check_modules_for_generic_metadata_only() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module helper;
using root::helper;

pub fun main() i32 {
helper::id[i32](1)
}
"#,
    );
    let entry_id = fixture.entry_id();
    let helper_id = fixture.add_child(
        entry_id,
        "helper",
        "helper.nia",
        r#"
pub fun id[T](value: T) T {
value
}

fun unused_bad() i32 {
missing_symbol
}
"#,
    );
    let loaded = fixture.freestanding_program();
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let helper_module = modules
        .iter()
        .find(|module| module.id == helper_id)
        .expect("called generic function owner should be executable-reachable");
    let unused_bad = helper_module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.kind == nia_defs::DefKind::Function && def.name == sym("unused_bad")).then_some(
                GlobalDefId {
                    module_id: helper_id,
                    def_id,
                },
            )
        })
        .expect("unused function");

    assert!(
        helper_module.body_diagnostics.is_empty(),
        "unused function in a generic callee module should not be body-checked: {:?}",
        helper_module.body_diagnostics
    );
    assert!(
        !helper_module
            .body_ir
            .function_bodies
            .contains_key(&unused_bad),
        "reachable generic metadata should not retain unrelated function bodies"
    );
}
