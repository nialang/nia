// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn visible_extensions_do_not_query_nominal_provider_discovery() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module types;
module impls;
using root::types;

pub fun main(value: types::Used) i32 {
    value.len()
}
"#,
    );
    let entry_id = fixture.entry_id();
    fixture.add_child(entry_id, "types", "types.nia", "pub struct Used {}");
    fixture.add_child(
        entry_id,
        "impls",
        "impls.nia",
        r#"
using root::types::Used;

extend Used {
    pub fun len(&self) i32 { 1 }
}
"#,
    );
    let loaded = fixture.freestanding_program();
    let db = query_db(loaded);

    let checked = db.expect_get(CodegenProgramQuery);
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.diagnostic.summary == "unknown method `len`" }),
        "nominal type import unexpectedly exposed an external extension: {:?}",
        checked.diagnostics
    );
    let trace = db.query_trace().expect("query trace");
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "visible_extensions" && dependency.to.name.contains("nominal")
    }));
}
