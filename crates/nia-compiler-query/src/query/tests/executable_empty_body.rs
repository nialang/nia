// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn executable_function_body_produces_factless_empty_body() {
    let fixture = LoadedProgramFixture::new("main.nia", "pub fn main() () {}");
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let facts = db.expect_get(ExecutableCheckedModuleFactsQuery);
    let module = facts.modules.first().expect("entry module facts");
    let def_id = module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.name == sym("main")).then_some(GlobalDefId {
                module_id: module.id,
                def_id,
            })
        })
        .expect("main function definition");
    assert!(facts.runtime_functions.contains(&def_id));
    assert!(
        !module.semantic_facts.function_facts.contains_key(&def_id),
        "an empty body should not need a synthetic semantic-facts entry"
    );

    let body = db.expect_get(ExecutableFunctionBodyQuery(def_id));
    let body = body.as_ref().as_ref().expect("empty checked body product");
    assert!(body.stmts.is_empty());
    assert!(body.tail.is_none());

    let checked = db.expect_get(ExecutableCheckedModulesQuery);
    let aggregate_body = checked
        .iter()
        .find(|module| module.id == def_id.module_id)
        .and_then(|module| module.body_ir.function_bodies.get(&def_id))
        .expect("aggregate empty checked body");
    assert!(Arc::ptr_eq(body, aggregate_body));

    let codegen = db.expect_get(CodegenProgramQuery);
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    assert!(
        codegen
            .backend_lowering
            .program
            .modules
            .iter()
            .flat_map(|module| &module.functions)
            .any(|function| function.def_id == def_id)
    );
}
