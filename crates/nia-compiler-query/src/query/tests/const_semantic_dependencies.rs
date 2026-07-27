// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn body_check_uses_const_semantic_modules_not_ast_module_map() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "const N: usize = 4; fn main() i32 { let mut values: [N]i32 = [0; N]; values.len() as i32 }",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let _ = db.expect_get(BodyCheckQuery(module_id));
    let trace = db.query_trace();

    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "body_check" && dependency.to.name == "const_module"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "body_check" && dependency.to.name == "const_values"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "body_check" && dependency.to.name == "const_array_lengths"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "body_check" && dependency.to.name == "full_active_module_item_tree"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "body_check" && dependency.to.name == "const"
    }));
    assert!(
        !trace
            .dependencies
            .iter()
            .any(|dependency| dependency.to.name == "program_const_modules")
    );
    assert!(
        !trace
            .dependencies
            .iter()
            .any(|dependency| dependency.to.name == "program_modules_by_id")
    );
    assert!(
        !trace
            .dependencies
            .iter()
            .any(|dependency| dependency.to.name == "program_item_signatures")
    );
}
