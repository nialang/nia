// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn executable_full_lowering_reuses_explicit_and_inferred_const_types() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
const explicit: usize = 19usize;
const inferred = 4usize;

fn main() usize {
explicit + inferred
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
        module.body_diagnostics.is_empty(),
        "prechecked const types must remain available during full body lowering: {:?}",
        module.body_diagnostics
    );
    assert_eq!(module.semantic_facts.const_types.len(), 2);
    assert_eq!(module.body_ir.function_bodies.len(), 1);
}

#[test]
fn executable_body_check_follows_same_module_call_closure() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn f3() i32 {
3
}

fn f2() i32 {
f3()
}

fn f1() i32 {
f2()
}

fn main() i32 {
f1()
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
    assert_eq!(
        module.body_ir.function_bodies.len(),
        4,
        "same-module executable body check should retain the whole call closure"
    );
    assert!(
        module.body_diagnostics.is_empty(),
        "same-module executable call closure should check without diagnostics: {:?}",
        module.body_diagnostics
    );
}

#[test]
fn executable_filtered_const_resolves_forwarded_array_len_values() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module facade;
using entry::facade;

fn main() i32 {
let mut values: [facade::LEN]u8 = [0; facade::LEN];
values[0] as i32
}
"#,
    );
    let entry_id = fixture.entry_id();
    let facade_id = fixture.add_child(
        entry_id,
        "facade",
        "facade.nia",
        r#"
module raw;
using self::raw;

pub const LEN: usize = raw::LEN;
"#,
    );
    fixture.add_child(
        facade_id,
        "raw",
        "facade/raw.nia",
        r#"
pub const LEN: usize = 4usize;
"#,
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let entry = modules
        .iter()
        .find(|module| module.id == entry_id)
        .expect("entry module should be executable-reachable");

    assert!(
        entry.body_diagnostics.is_empty(),
        "filtered executable body checking should resolve forwarded const array lengths: {:?}",
        entry.body_diagnostics
    );
    assert!(
        entry
            .const_eval
            .array_lengths
            .values()
            .any(|length| *length == 4),
        "filtered executable const should evaluate forwarded array length"
    );
}

#[test]
fn executable_filtered_const_resolves_local_forwarded_array_len_in_method_body() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module raw;
using entry::raw;

const LEN: usize = raw::LEN;

struct Box {}

extend Box {
fn value(&self) usize {
    let mut values: [LEN]u8 = [_]u8[0; LEN];
    values[0] as usize
}
}

fn main() usize {
let box = Box {};
box.value()
}
"#,
    );
    let entry_id = fixture.entry_id();
    fixture.add_child(
        entry_id,
        "raw",
        "raw.nia",
        r#"
pub const LEN: usize = 4usize;
"#,
    );
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let entry = modules
        .iter()
        .find(|module| module.id == entry_id)
        .expect("entry module should be executable-reachable");

    assert!(
        entry.body_diagnostics.is_empty(),
        "filtered executable body checking should resolve local forwarded array lengths used in method bodies: {:?}",
        entry.body_diagnostics
    );
    assert!(
        entry
            .const_eval
            .array_lengths
            .values()
            .any(|length| *length == 4),
        "filtered executable const should evaluate local forwarded method-body array length"
    );
}
