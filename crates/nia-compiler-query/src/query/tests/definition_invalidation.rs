// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn direct_module_defs_invalidation_stops_at_snapshot_boundary() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "pub struct S { value: i32 } fn main() i32 { 0 }",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let _ = db.expect_get(TypeResolutionQuery(module_id));
    let invalidation = db.invalidate(ModuleDefsQuery(module_id));
    let invalidated = invalidation
        .invalidated
        .iter()
        .map(|frame| frame.name)
        .collect::<Vec<_>>();

    assert!(invalidated.contains(&"module_defs"), "{invalidated:?}");
    assert!(!invalidated.contains(&"public_surfaces"), "{invalidated:?}");
    assert!(
        !invalidated.contains(&"public_using_scopes"),
        "{invalidated:?}"
    );
    assert!(
        !invalidated.contains(&"module_using_scope"),
        "{invalidated:?}"
    );
    assert!(!invalidated.contains(&"type_resolution"), "{invalidated:?}");

    let _ = db.expect_get(TypeResolutionQuery(module_id));
}

#[test]
fn invalidates_module_defs_after_item_tree_changes() {
    let fixture = LoadedProgramFixture::new("main.nia", "pub struct S { value: i32 }");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let _ = db.expect_get(ModuleDefsQuery(module_id));
    let invalidation = db.invalidate(ModuleItemTreeInputQuery(module_id));
    let invalidated = invalidation
        .invalidated
        .iter()
        .map(|frame| frame.name)
        .collect::<Vec<_>>();

    assert!(
        invalidated.contains(&"module_item_tree_input"),
        "{invalidated:?}"
    );
    assert!(invalidated.contains(&"module_item_tree"), "{invalidated:?}");
    assert!(
        invalidated.contains(&"active_module_item_tree"),
        "{invalidated:?}"
    );
    assert!(invalidated.contains(&"module_defs"), "{invalidated:?}");
}
