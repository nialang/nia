// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn executable_checked_program_uses_query_backed_extension_method_lookup() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "trait Show { fn show(self) i32; } extend i32 : Show { fn show(self) i32 { self } } pub fn main() i32 { 1.show() }",
    );
    fixture
        .add_freestanding_runtime("using entry; pub extern fn _start() () { _ = entry::main(); }");
    let mut loaded = fixture.program();
    loaded.runtime = test_freestanding_runtime();
    let db = query_db(loaded);

    let checked = db.expect_get(CodegenProgramQuery);
    let trace = db.query_trace().expect("query trace");

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "executable_checked_module_facts"
            && dependency.to.name == "extension_trait_impls_for_trait"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "executable_checked_module_facts"
            && dependency.to.name == "program_trait_solving_signatures"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "executable_checked_module_facts"
            && dependency.to.name == "extension_provider_module_facts"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "executable_checked_module_facts"
            && dependency.to.name == "extension_method_index"
    }));
    assert!(trace_has_dependency(
        &trace,
        "executable_checked_modules",
        "executable_checked_module_facts"
    ));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "checked_program"
            && dependency.to.name == "extension_provider_validation_facts"
    }));
}

#[test]
fn freestanding_runtime_source_is_the_only_user_entry_root() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "pub fn main() i32 { 7 } pub fn mymain() i32 { 11 }",
    );
    fixture.add_freestanding_runtime(
        "using entry; pub extern fn _start() () { _ = entry::mymain(); }",
    );
    let entry_id = fixture.entry_id();
    let mut loaded = fixture.program();
    loaded.runtime = test_freestanding_runtime();
    let db = query_db(loaded);

    let facts = db.expect_get(ExecutableCheckedModuleFactsQuery);
    let entry = facts
        .modules
        .iter()
        .find(|module| module.id == entry_id)
        .expect("runtime call must make the entry module reachable");
    let function = |name: &str| {
        entry
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Function && def.name == sym(name)).then_some(
                    GlobalDefId {
                        module_id: entry_id,
                        def_id,
                    },
                )
            })
            .expect("entry function definition")
    };

    assert!(facts.runtime_functions.contains(&function("mymain")));
    assert!(!facts.runtime_functions.contains(&function("main")));
}

#[test]
fn freestanding_runtime_helpers_follow_ordinary_source_reachability() {
    let fixture = LoadedProgramFixture::new("main.nia", "pub fn main() i32 { 7 }");
    let loaded = fixture.freestanding_program_with_runtime(
        r#"
using entry;

fn used() i32 { entry::main() }
fn unused() i32 { 99 }
pub extern fn _start() () { _ = &used; }
"#,
    );
    let db = query_db(loaded);

    let facts = db.expect_get(ExecutableCheckedModuleFactsQuery);
    let runtime = facts
        .modules
        .iter()
        .find(|module| {
            module
                .defs
                .defs
                .iter()
                .any(|(_, def)| def.name == sym("_start"))
        })
        .expect("runtime start module");
    let function = |name: &str| {
        runtime
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Function && def.name == sym(name)).then_some(
                    GlobalDefId {
                        module_id: runtime.id,
                        def_id,
                    },
                )
            })
            .expect("runtime function definition")
    };

    assert!(facts.runtime_functions.contains(&function("_start")));
    assert!(facts.runtime_functions.contains(&function("used")));
    assert!(!facts.runtime_functions.contains(&function("unused")));
}

#[test]
fn bare_entry_checked_program_uses_rooted_diagnostics_without_freestanding_start() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "extend ! { fn nope(self) () {} } pub fn main() i32 { 1 }",
    );
    let db = query_db(fixture.program());

    let checked = db.expect_get(EntryCheckedProgramQuery);
    let trace = db.query_trace().expect("query trace");

    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("extend target must be an extendable value type")),
        "{:?}",
        checked.diagnostics
    );
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "entry_checked_program"
            && dependency.to.name == "executable_checked_modules"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "entry_checked_program"
            && dependency.to.name == "extension_provider_validation_facts"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "entry_checked_program"
            && dependency.to.name == "extension_method_index"
    }));
}

#[test]
fn entry_check_validates_const_fn_capability_at_declaration() {
    for source in [
        r#"
fn plain(v: u32) u32 { v }
const fn viaPlain(v: u32) u32 { plain(v) }
pub fn main() i32 { 0 }
"#,
        r#"
fn plain(v: u32) u32 { v }
const fn viaPlain(v: u32) u32 { plain(v) }
pub fn main() i32 { _ = viaPlain(1u32); 0 }
"#,
        r#"
fn plain(v: u32) u32 { v }
const fn viaPlain(v: u32) u32 { plain(v) }
const W: u32 = viaPlain(1u32);
pub fn main() i32 { _ = W; 0 }
"#,
    ] {
        let fixture = LoadedProgramFixture::new("main.nia", source);
        let checked = query_db(fixture.program()).expect_get(EntryCheckedProgramQuery);
        let capability_errors = checked
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic
                    .diagnostic
                    .summary
                    .contains("const expression can only call `const fn`")
            })
            .count();

        assert_eq!(capability_errors, 1, "{:?}", checked.diagnostics);
    }
}

#[test]
fn freestanding_entry_checked_program_uses_executable_reachability() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "extend ! { fn nope(self) () {} } pub fn main() i32 { 1 }",
    );
    fixture
        .add_freestanding_runtime("using entry; pub extern fn _start() () { _ = entry::main(); }");
    let mut loaded = fixture.program();
    loaded.runtime = test_freestanding_runtime();
    let db = query_db(loaded);

    let checked = db.expect_get(EntryCheckedProgramQuery);
    let trace = db.query_trace().expect("query trace");

    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("extend target must be an extendable value type")),
        "{:?}",
        checked.diagnostics
    );
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "entry_checked_program"
            && dependency.to.name == "executable_checked_modules"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "entry_checked_program"
            && dependency.to.name == "extension_provider_validation_facts"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "entry_checked_program"
            && dependency.to.name == "extension_method_index"
    }));
}

#[test]
fn executable_reachability_keeps_matched_trait_impl_method_bodies() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module parse;
using entry::parse;

pub fn main() i32 {
(parse::Input {}).parse[i32]()
}
"#,
    );
    let entry_id = fixture.entry_id();
    let parse_id = fixture.add_child(
        entry_id,
        "parse",
        "parse.nia",
        r#"
pub struct Input {}

pub trait From[Input] {
fn from(input: Input) Self;
}

extend Input {
pub fn parse[T](self) T
where T: From[Input] {
[T]::from(self)
}
}

extend i32 : From[Input] {
fn from(input: Input) i32 {
    _ = input;
    42
}
}
"#,
    );
    fixture
        .add_freestanding_runtime("using entry; pub extern fn _start() () { _ = entry::main(); }");
    let mut loaded = fixture.program();
    loaded.runtime = test_freestanding_runtime();
    let db = query_db(loaded);

    let checked = db.expect_get(ExecutableCheckedModulesQuery);
    let parse_module = checked
        .iter()
        .find(|module| module.id == parse_id)
        .expect("parse module should be executable-reachable");
    let from_method = parse_module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.name == sym("from") && def.kind == nia_defs::DefKind::Method).then_some(
                GlobalDefId {
                    module_id: parse_id,
                    def_id,
                },
            )
        })
        .expect("impl from method should be defined");

    assert!(
        parse_module
            .body_ir
            .function_bodies
            .contains_key(&from_method),
        "matched trait impl method body should be retained for executable codegen"
    );
}
