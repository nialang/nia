// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn executable_full_lowering_reuses_explicit_and_inferred_const_types() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
const explicit: usize = 19usize;
const inferred = 4usize;

pub fn main() usize {
explicit + inferred
}
"#,
    );
    let module_id = fixture.entry_id();
    let loaded = fixture.freestanding_program();
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

    let trace = db.query_trace().expect("query trace");
    let visible_extensions_key = format!("VisibleExtensionsQuery({module_id:?})");
    let visible_extensions = trace
        .queries
        .iter()
        .find(|query| {
            query.frame.name == "visible_extensions"
                && query.frame.key.as_ref() == visible_extensions_key
        })
        .expect("executable facts should build the entry module extension index");
    assert_eq!(
        visible_extensions.stats.cache_hits, 0,
        "plain executable const inputs should not request the extension index again"
    );
}

#[test]
fn executable_filtered_const_loads_extensions_when_resolving_method_calls() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
enum Size { Small }

extend Size {
const fn value(self) usize {
4usize
}
}

const fn len() usize {
Size::Small.value()
}

pub fn main() i32 {
let mut values: [u8; len()] = [0; 4];
values[0] as i32
}
"#,
    );
    let module_id = fixture.entry_id();
    let loaded = fixture.freestanding_program();
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let module = modules
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry module should be executable-reachable");

    assert!(
        module.body_diagnostics.is_empty(),
        "extension-backed executable const inputs should remain valid: {:?}",
        module.body_diagnostics.diagnostics()
    );
    assert!(
        module
            .const_eval
            .array_lengths
            .values()
            .any(|length| *length == 4),
        "extension method result should remain available as an array length"
    );

    let trace = db.query_trace().expect("query trace");
    let visible_extensions_key = format!("VisibleExtensionsQuery({module_id:?})");
    let visible_extensions = trace
        .queries
        .iter()
        .find(|query| {
            query.frame.name == "visible_extensions"
                && query.frame.key.as_ref() == visible_extensions_key
        })
        .expect("extension method resolution should build the entry module extension index");
    assert!(
        visible_extensions.stats.cache_hits > 0,
        "filtered const method resolution should request the extension index: {:?}",
        trace
            .queries
            .iter()
            .filter(|query| query.frame.name == "visible_extensions")
            .collect::<Vec<_>>()
    );
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

pub fn main() i32 {
f1()
}
"#,
    );
    let module_id = fixture.entry_id();
    let loaded = fixture.freestanding_program();
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

pub fn main() i32 {
let mut values: [u8; facade::LEN] = [0; facade::LEN];
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
    let loaded = fixture.freestanding_program();
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
    let mut values: [u8; LEN] = [0; LEN];
    values[0] as usize
}
}

pub fn main() usize {
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
    let loaded = fixture.freestanding_program();
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

#[test]
fn signature_const_resolves_receiver_locals_for_omitted_patterns() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module palette;
using entry::palette;

pub fn main() i32 {
    let values: [u8; palette::SIZE] = [0; palette::SIZE];
    values[0] as i32
}
"#,
    );
    let entry_id = fixture.entry_id();
    fixture.add_child(
        entry_id,
        "palette",
        "palette.nia",
        r#"
pub enum Color { Red, Data(usize) }

extend Color {
    const fn score(self) usize {
        match self {
            .Red => 4usize,
            .Data(value) => value,
        }
    }
}

pub const SIZE: usize = Color::Red.score();
"#,
    );
    let loaded = fixture.freestanding_program();
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let entry = modules
        .iter()
        .find(|module| module.id == entry_id)
        .expect("entry module should be executable-reachable");

    assert!(
        entry.body_diagnostics.is_empty(),
        "signature const evaluation should resolve method receivers: {:?}",
        entry.body_diagnostics
    );
    assert!(
        entry
            .const_eval
            .array_lengths
            .values()
            .any(|length| *length == 4),
        "signature const evaluation should preserve the receiver method result"
    );
}
