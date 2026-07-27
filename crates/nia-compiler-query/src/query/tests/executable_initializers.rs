// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn executable_checked_modules_include_reachable_global_initializers() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
static used: i32 = 1;

fn main() i32 {
used
}
"#,
    );
    let module_id = fixture.entry_id();
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module should be executable-reachable");
    let used = module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.kind == nia_defs::DefKind::Global && def.name == sym("used"))
                .then_some(GlobalDefId { module_id, def_id })
        })
        .expect("used global");

    assert!(
        module.body_ir.global_inits.contains_key(&used),
        "reachable global initializers must be retained for executable codegen"
    );
}

#[test]
fn executable_checked_modules_include_reachable_local_static_initializers() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn option_arg() &u8 {
static text = b"-O2\0";
&text[0]
}

fn main() i32 {
_ = option_arg();
0
}
"#,
    );
    let module_id = fixture.entry_id();
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module should be executable-reachable");
    let text = module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.kind == nia_defs::DefKind::Global && def.name == sym("text"))
                .then_some(GlobalDefId { module_id, def_id })
        })
        .expect("local static global");

    assert!(
        module.body_ir.global_inits.contains_key(&text),
        "reachable local static initializers must be retained for executable codegen"
    );
}

#[test]
fn executable_checked_modules_include_reachable_extension_method_local_static_initializers() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
enum Mode: i32 {
O2 = 2,
}

extend Mode {
fn argv(self) &u8 {
    static o2 = b"-O2\0";
    switch self {
        Mode::O2 => &o2[0],
        _ => &o2[0],
    }
}
}

fn main() i32 {
_ = Mode::O2.argv();
0
}
"#,
    );
    let module_id = fixture.entry_id();
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module should be executable-reachable");
    let o2 = module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.kind == nia_defs::DefKind::Global && def.name == sym("o2"))
                .then_some(GlobalDefId { module_id, def_id })
        })
        .expect("local static global");

    assert!(
        module.body_ir.global_inits.contains_key(&o2),
        "reachable extension method local static initializers must be retained for executable codegen"
    );
}

#[test]
fn executable_checked_modules_include_cross_module_extension_method_local_static_initializers() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
using helper::Mode;

fn main() i32 {
_ = Mode::O2.argv();
0
}
"#,
    );
    let entry_id = fixture.entry_id();
    let helper_id = fixture.add_child(
        entry_id,
        "helper",
        "helper.nia",
        r#"
pub enum Mode: i32 {
O2 = 2,
}

extend Mode {
pub fn argv(self) &u8 {
    static o2 = b"-O2\0";
    switch self {
        Mode::O2 => &o2[0],
        _ => &o2[0],
    }
}
}
"#,
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == helper_id)
        .expect("helper module should be executable-reachable");
    let o2 = module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.kind == nia_defs::DefKind::Global && def.name == sym("o2")).then_some(
                GlobalDefId {
                    module_id: helper_id,
                    def_id,
                },
            )
        })
        .expect("local static global");

    assert!(
        module.body_ir.global_inits.contains_key(&o2),
        "reachable cross-module extension method local static initializers must be retained for executable codegen"
    );
    assert!(
        module
            .executable_reachable_globals
            .as_ref()
            .is_some_and(|globals| globals.contains(&o2)),
        "reachable local static should be recorded in executable_reachable_globals: {:?}",
        module.executable_reachable_globals
    );

    let backend = db.expect_get(BackendLoweringQuery);
    let backend_module = backend
        .semantic
        .program
        .modules
        .iter()
        .find(|module| module.id == helper_id)
        .expect("helper backend module");
    assert!(
        backend_module
            .globals
            .iter()
            .any(|global| global.def_id == o2 && global.init.is_some()),
        "reachable cross-module extension method local static must lower as a backend global"
    );
}

#[test]
fn executable_checked_modules_do_not_flow_check_unreachable_functions() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn unused() i32 {
}

fn main() i32 {
0
}
"#,
    );
    let module_id = fixture.entry_id();
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module should be executable-reachable");

    assert!(
        module.flow_check.diagnostics.is_empty(),
        "unreachable function flow diagnostics should not block executable checking: {:?}",
        module.flow_check.diagnostics
    );
}

#[test]
fn executable_checked_modules_do_not_body_check_unreachable_loaded_modules() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
pub module unused;

fn main() i32 {
0
}
"#,
    );
    let entry_id = fixture.entry_id();
    let unused_id = fixture.add_child(
        entry_id,
        "unused",
        "unused.nia",
        r#"
pub fn expensive_or_invalid() i32 {
missing_symbol
}
"#,
    );
    let unused_description = format!("{unused_id:?}");
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let trace = db.query_trace();

    assert!(
        modules.iter().all(|module| module.id != unused_id),
        "unreachable module should not be kept for executable codegen"
    );
    assert!(
        !trace.queries.iter().any(|query| {
            query.frame.name == "body_check"
                && query.frame.description.contains(&unused_description)
                && query.stats.executions > 0
        }),
        "unreachable module should not be body-checked: {:?}",
        trace
            .queries
            .iter()
            .filter(|query| query.frame.name == "body_check")
            .collect::<Vec<_>>()
    );
}
