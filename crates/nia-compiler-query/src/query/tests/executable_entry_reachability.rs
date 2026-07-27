// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn executable_checked_program_uses_query_backed_extension_method_lookup() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "trait Show { fn show(self) i32; } extend i32 : Show { fn show(self) i32 { self } } pub fn main() i32 { 1.show() }",
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let checked = db.expect_get(CodegenProgramQuery);
    let trace = db.query_trace();

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
fn bare_entry_checked_program_uses_rooted_diagnostics_without_freestanding_start() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "extend ! { fn nope(self) void {} } pub fn main() i32 { 1 }",
    );
    let db = query_db(fixture.program());

    let checked = db.expect_get(EntryCheckedProgramQuery);
    let trace = db.query_trace();

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
fn freestanding_entry_checked_program_uses_executable_reachability() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "extend ! { fn nope(self) void {} } pub fn main() i32 { 1 }",
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let checked = db.expect_get(EntryCheckedProgramQuery);
    let trace = db.query_trace();

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
parse::parse[i32, parse::Input](parse::Input {})
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

pub trait ParseFrom[Input] {
fn parse_from(input: Input) Self;
}

pub fn parse[T, Input](input: Input) T
where T: ParseFrom[Input]
{
[T]::parse_from(input)
}

extend i32 : ParseFrom[Input] {
fn parse_from(input: Input) i32 {
    _ = input;
    42
}
}
"#,
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let checked = db.expect_get(ExecutableCheckedModulesQuery);
    let parse_module = checked
        .iter()
        .find(|module| module.id == parse_id)
        .expect("parse module should be executable-reachable");
    let parse_from = parse_module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.name == sym("parse_from") && def.kind == nia_defs::DefKind::Method).then_some(
                GlobalDefId {
                    module_id: parse_id,
                    def_id,
                },
            )
        })
        .expect("impl parse_from method should be defined");

    assert!(
        parse_module
            .body_ir
            .function_bodies
            .contains_key(&parse_from),
        "matched trait impl method body should be retained for executable codegen"
    );
}
