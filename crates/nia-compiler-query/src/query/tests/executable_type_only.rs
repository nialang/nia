// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn executable_type_only_modules_keep_signature_const_enum_values() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module types;
using entry::types;

fn main(value: types::Mode) i32 {
0
}
"#,
    );
    let entry_id = fixture.entry_id();
    let types_id = fixture.add_child(
        entry_id,
        "types",
        "types.nia",
        r#"
pub enum Mode: i32 {
A = 1,
B = 1 + 2,
}

pub fn unused_bad() i32 {
missing_symbol
}
"#,
    );
    let types_description = format!("{types_id:?}");
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let type_module = modules
        .iter()
        .find(|module| module.id == types_id)
        .expect("type owner module should be present for backend type lookup");
    assert!(
        type_module.executable_type_only,
        "enum owner module should stay type-only"
    );
    let b = type_module
        .defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.kind == nia_defs::DefKind::EnumVariant && def.name == sym("B")).then_some(def_id)
        })
        .expect("enum variant B");
    assert!(
        matches!(
            type_module.const_eval.enum_values.get(&b),
            Some(nia_const_check::ConstValue::Int(value)) if value.bits() == 3
        ),
        "type-only signature const should evaluate enum discriminants: {:?}",
        type_module.const_eval.enum_values
    );

    let trace = db.query_trace();
    for full_query in ["type_resolution", "type_lowering", "value_resolution"] {
        assert!(
            !trace.queries.iter().any(|query| {
                query.frame.name == full_query
                    && query.frame.description.contains(&types_description)
                    && query.stats.executions > 0
            }),
            "type-only enum module should not execute {full_query}: {:?}",
            trace
                .queries
                .iter()
                .filter(|query| query.frame.name == full_query)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn executable_type_only_modules_keep_signature_const_array_lengths() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module types;
using entry::types;

fn main(value: types::Packet) i32 {
0
}
"#,
    );
    let entry_id = fixture.entry_id();
    let types_id = fixture.add_child(
        entry_id,
        "types",
        "types.nia",
        r#"
const N: usize = 4;

pub struct Packet {
data: [N]u8,
}

pub fn unused_bad() i32 {
missing_symbol
}
"#,
    );
    let types_description = format!("{types_id:?}");
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let modules = db.expect_get(ExecutableCheckedModulesQuery);
    let type_module = modules
        .iter()
        .find(|module| module.id == types_id)
        .expect("type owner module should be present for backend type lookup");
    assert!(
        type_module.executable_type_only,
        "array owner module should stay type-only"
    );
    assert!(
        type_module
            .const_eval
            .array_lengths
            .values()
            .any(|len| *len == 4),
        "type-only signature const should evaluate array length constants: {:?}",
        type_module.const_eval.array_lengths
    );

    let trace = db.query_trace();
    for full_query in ["type_resolution", "type_lowering", "value_resolution"] {
        assert!(
            !trace.queries.iter().any(|query| {
                query.frame.name == full_query
                    && query.frame.description.contains(&types_description)
                    && query.stats.executions > 0
            }),
            "type-only array module should not execute {full_query}: {:?}",
            trace
                .queries
                .iter()
                .filter(|query| query.frame.name == full_query)
                .collect::<Vec<_>>()
        );
    }
}
