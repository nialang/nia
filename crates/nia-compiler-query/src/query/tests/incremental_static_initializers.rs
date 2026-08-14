// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn body_edit_keeps_unrelated_lowered_function_product_green() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "fn helper() i32 { 1 } fn main() i32 { helper() }",
    );
    let module_id = fixture.entry_id();
    let database = fixture.database();

    let first_codegen = database.codegen_program();
    assert!(
        first_codegen.diagnostics.is_empty(),
        "{:?}",
        first_codegen.diagnostics
    );
    let checked = database.db.expect_get(ExecutableCheckedModulesQuery);
    let module = checked
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module should be checked");
    let function = |name| {
        module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == sym(name)).then_some(GlobalDefId { module_id, def_id })
            })
            .unwrap_or_else(|| panic!("missing function `{name}`"))
    };
    let helper = function("helper");
    let main = function("main");
    let fact_modules = database.db.expect_get(ExecutableCheckedModuleFactsQuery);
    let fact_module = fact_modules
        .modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module facts should exist");
    assert!(
        fact_module.body_ir.function_bodies.is_empty(),
        "the executable facts aggregate must not produce checked bodies"
    );
    assert!(fact_modules.runtime_functions.contains(&helper));
    assert!(fact_modules.runtime_functions.contains(&main));
    let checked_helper = module
        .body_ir
        .function_bodies
        .get(&helper)
        .expect("helper should have a checked body");
    let checked_helper_product = database.db.expect_get(ExecutableFunctionBodyQuery(helper));
    let checked_helper_product = checked_helper_product
        .as_ref()
        .as_ref()
        .expect("helper checked-body product");
    assert!(Arc::ptr_eq(checked_helper, checked_helper_product));
    let first_helper = database.db.expect_get(LoweredFunctionBodyQuery(helper));
    let first_main = database.db.expect_get(LoweredFunctionBodyQuery(main));

    fixture.update_module_source(
        module_id,
        "fn helper() i32 { 2 } fn main() i32 { helper() }",
        SourceRevision(1),
    );
    database.update(CompileRequest::new(fixture.program()));
    let second_codegen = database.codegen_program();
    assert!(
        second_codegen.diagnostics.is_empty(),
        "{:?}",
        second_codegen.diagnostics
    );
    let second_helper = database.db.expect_get(LoweredFunctionBodyQuery(helper));
    let second_main = database.db.expect_get(LoweredFunctionBodyQuery(main));
    let trace = database.query_trace();

    assert!(!Arc::ptr_eq(&first_helper, &second_helper));
    assert!(Arc::ptr_eq(&first_main, &second_main));
    assert_eq!(query_executions(&trace, "executable_function_body"), 4);
    assert_eq!(query_executions(&trace, "lowered_function_body"), 3);
    assert!(query_green_validations(&trace, "lowered_function_body") >= 1);
}

#[test]
fn global_edit_preserves_unrelated_static_init_semantic_value() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
static first: [u8; 4] = [1, 2, 3, 4];
static second: [u8; 4] = [5, 6, 7, 8];

fn main() u8 {
first[0] + second[0]
}
"#,
    );
    let module_id = fixture.entry_id();
    let database = fixture.database();

    let first_codegen = database.codegen_program();
    assert!(
        first_codegen.diagnostics.is_empty(),
        "{:?}",
        first_codegen.diagnostics
    );
    let checked = database.db.expect_get(ExecutableCheckedModulesQuery);
    let module = checked
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module should be checked");
    let global = |name| {
        module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Global && def.name == sym(name))
                    .then_some(GlobalDefId { module_id, def_id })
            })
            .unwrap_or_else(|| panic!("missing global `{name}`"))
    };
    let first = global("first");
    let second = global("second");
    let fact_modules = database.db.expect_get(ExecutableCheckedModuleFactsQuery);
    let fact_module = fact_modules
        .modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module facts should exist");
    assert!(
        fact_module.body_ir.global_inits.is_empty(),
        "the executable facts aggregate must not produce static initializer payloads"
    );
    assert!(fact_modules.runtime_globals.contains(&first));
    assert!(fact_modules.runtime_globals.contains(&second));
    let aggregate_first = module
        .body_ir
        .global_inits
        .get(&first)
        .expect("first should have a static initializer");
    let first_item = database.db.expect_get(ExecutableStaticInitQuery(first));
    let first_payload = first_item
        .as_ref()
        .as_ref()
        .expect("first static initializer product");
    assert!(Arc::ptr_eq(aggregate_first, first_payload));
    let first_second = database.db.expect_get(ExecutableStaticInitQuery(second));

    fixture.update_module_source(
        module_id,
        r#"
static first: [u8; 4] = [9, 2, 3, 4];
static second: [u8; 4] = [5, 6, 7, 8];

fn main() u8 {
first[0] + second[0]
}
"#,
        SourceRevision(1),
    );
    database.update(CompileRequest::new(fixture.program()));
    let second_codegen = database.codegen_program();
    assert!(
        second_codegen.diagnostics.is_empty(),
        "{:?}",
        second_codegen.diagnostics
    );
    let second_first = database.db.expect_get(ExecutableStaticInitQuery(first));
    let second_second = database.db.expect_get(ExecutableStaticInitQuery(second));
    let checked = database.db.expect_get(ExecutableCheckedModulesQuery);
    let module = checked
        .iter()
        .find(|module| module.id == module_id)
        .expect("updated entry module should be checked");
    let aggregate_second = module
        .body_ir
        .global_inits
        .get(&second)
        .expect("second should retain a static initializer");
    let second_payload = second_second
        .as_ref()
        .as_ref()
        .expect("updated second static initializer product");
    let trace = database.query_trace();

    assert!(!Arc::ptr_eq(&first_item, &second_first));
    assert_eq!(first_second.as_ref(), second_second.as_ref());
    assert!(Arc::ptr_eq(aggregate_second, second_payload));
    assert_eq!(query_executions(&trace, "executable_static_init"), 4);
}

#[test]
fn static_init_ref_summary_drives_reachability_without_aggregate_payload() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
struct Helper {}

extend Helper {
fn value() i32 {
    7
}
}

static callback: &fn() i32 = &Helper::value;

fn main() i32 {
callback()
}
"#,
    );
    let db = query_db(fixture.program());

    let codegen = db.expect_get(CodegenProgramQuery);
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let facts = db.expect_get(ExecutableCheckedModuleFactsQuery);
    let module = facts.modules.first().expect("entry module facts");
    assert!(module.body_ir.global_inits.is_empty());
    let def_id = |name| {
        module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == sym(name)).then_some(GlobalDefId {
                    module_id: module.id,
                    def_id,
                })
            })
            .unwrap_or_else(|| panic!("missing definition `{name}`"))
    };
    let helper = def_id("value");
    let callback = def_id("callback");

    assert!(facts.runtime_functions.contains(&helper));
    assert!(facts.runtime_globals.contains(&callback));
    let init = db.expect_get(ExecutableStaticInitQuery(callback));
    assert!(
        matches!(
            init.as_ref().as_deref(),
            Some(nia_static_ir::StaticInit::AddrOfFunction { function, .. })
                if *function == helper
        ),
        "{init:?}"
    );
    assert!(
        codegen
            .backend_lowering
            .program
            .modules
            .iter()
            .flat_map(|module| &module.functions)
            .any(|function| function.def_id == helper),
        "the static reference summary must keep helper reachable"
    );
}

#[test]
fn local_static_item_uses_owner_function_facts_for_associated_function_reference() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
struct Helper {}

extend Helper {
fn value() i32 {
    7
}
}

fn invoke() i32 {
static callback: &fn() i32 = &Helper::value;
callback()
}

fn main() i32 {
invoke()
}
"#,
    );
    let db = query_db(fixture.program());

    let codegen = db.expect_get(CodegenProgramQuery);
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let facts = db.expect_get(ExecutableCheckedModuleFactsQuery);
    let module = facts.modules.first().expect("entry module facts");
    assert!(module.body_ir.global_inits.is_empty());
    let def_id = |name, kind| {
        module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == sym(name) && def.kind == kind).then_some(GlobalDefId {
                    module_id: module.id,
                    def_id,
                })
            })
            .unwrap_or_else(|| panic!("missing {kind:?} definition `{name}`"))
    };
    let helper = def_id("value", nia_defs::DefKind::Method);
    let callback = def_id("callback", nia_defs::DefKind::Global);

    assert!(facts.runtime_functions.contains(&helper));
    assert!(facts.runtime_globals.contains(&callback));
    let init = db.expect_get(ExecutableStaticInitQuery(callback));
    assert!(
        matches!(
            init.as_ref().as_deref(),
            Some(nia_static_ir::StaticInit::AddrOfFunction { function, .. })
                if *function == helper
        ),
        "{init:?}"
    );
    assert!(
        codegen
            .backend_lowering
            .program
            .modules
            .iter()
            .flat_map(|module| &module.functions)
            .any(|function| function.def_id == helper),
        "the local static reference summary must keep helper reachable"
    );
}
