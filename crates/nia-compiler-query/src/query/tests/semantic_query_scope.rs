// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn const_uses_precise_program_context_queries() {
    let fixture = LoadedProgramFixture::new("main.nia", "const VALUE = 1; fn main() i32 { VALUE }");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let _ = db.expect_get(ConstQuery(module_id));
    let trace = db.query_trace();

    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "const" && dependency.to.name == "const_module"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "const" && dependency.to.name == "const_array_lengths"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "const" && dependency.to.name == "const_enum_values"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "const_array_lengths" && dependency.to.name == "const_module"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "const_enum_values" && dependency.to.name == "const_array_lengths"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "const" && dependency.to.name == "program_full_defs_by_id"
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
            .any(|dependency| dependency.to.name == "program_item_signatures")
    );
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "const" && dependency.to.name == "program_trait_solving_signatures"
    }));
    assert!(!depends_on_body_signature_query(&trace, "const"));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "const" && dependency.to.name == "full_module_defs"
    }));
}

#[test]
fn monomorphization_avoids_removed_program_trait_signature_product() {
    let fixture = LoadedProgramFixture::new("main.nia", "pub fn main() i32 { 1 }");
    let db = query_db(fixture.program());

    let _ = db.expect_get(MonomorphizationQuery);
    let trace = db.query_trace();

    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "monomorphization"
            && dependency.to.name == "program_trait_solving_signatures"
    }));
    assert!(!depends_on_body_signature_query(&trace, "monomorphization"));
}

#[test]
fn executable_reachability_uses_lazy_signature_resolvers() {
    let fixture = LoadedProgramFixture::new("main.nia", "pub fn main() i32 { 1 }");
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let _ = db.expect_get(ExecutableCheckedModulesQuery);
    let trace = db.query_trace();

    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "executable_checked_module_facts"
            && dependency.to.name == "program_executable_reachability_signatures"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "executable_checked_module_facts"
            && dependency.to.name == "signature_item_signatures"
    }));
    assert!(!depends_on_body_signature_query(
        &trace,
        "executable_checked_module_facts"
    ));
    assert!(trace_has_dependency(
        &trace,
        "executable_checked_modules",
        "executable_checked_module_facts"
    ));
}

#[test]
fn body_check_without_method_lookup_does_not_build_global_extension_method_index() {
    let mut fixture =
        LoadedProgramFixture::new("main.nia", "module providers; fn main() i32 { 1 }");
    let module_id = fixture.entry_id();
    fixture.add_child(
        module_id,
        "providers",
        "providers.nia",
        "struct S {} extend S { pub fn make() S { {} } }",
    );
    let db = query_db(fixture.program());

    let checked = db.expect_get(BodyCheckQuery(module_id));
    let trace = db.query_trace();

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(
        !trace
            .queries
            .iter()
            .any(|query| { query.frame.name == "extension_method_index" })
    );
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "body_check" && dependency.to.name == "extension_method_index"
    }));
}

#[test]
fn body_check_method_lookup_uses_named_extension_method_query() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "module module1; using self::module1::S; fn main() i32 { let s = S::make(); 1 }",
    );
    let module_id = fixture.entry_id();
    fixture.add_child(
        module_id,
        "module1",
        "module1.nia",
        "pub struct S {} extend S { pub fn make() S { {} } }",
    );
    let db = query_db(fixture.program());

    let checked = db.expect_get(BodyCheckQuery(module_id));
    let trace = db.query_trace();

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "body_check" && dependency.to.name == "extension_methods_named"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "body_check" && dependency.to.name == "extension_method_index"
    }));
}

#[test]
fn const_module_uses_full_active_item_tree_query() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "const fn value() usize { 1 } const VALUE = value();",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let _ = db.expect_get(ConstModuleQuery(module_id));
    let trace = db.query_trace();

    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "const_module"
            && dependency.to.name == "full_active_module_item_tree"
    }));
}

#[test]
fn semantic_use_table_query_combines_value_local_and_type_resolution() {
    let source = "static VALUE: i32 = 1; fn main() i32 { let mut local: i32 = VALUE; local }";
    let fixture = LoadedProgramFixture::new("main.nia", source);
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let table = db.expect_get(SemanticUseTableQuery(module_id));
    let trace = db.query_trace();

    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "semantic_use_table" && dependency.to.name == "value_resolution"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "semantic_use_table" && dependency.to.name == "local_resolution"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "semantic_use_table" && dependency.to.name == "type_lowering"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "semantic_use_table" && dependency.to.name == "module_origins"
    }));
    assert_eq!(table.store_id(), db.context().node_store().id());

    assert!(matches!(
        table
            .node_value_uses
            .values()
            .find(|value_use| matches!(value_use, SemanticValueUse::Global(_))),
        Some(SemanticValueUse::Global(_))
    ));

    assert!(matches!(
        table
            .node_value_uses
            .values()
            .find(|value_use| matches!(value_use, SemanticValueUse::Local(_))),
        Some(SemanticValueUse::Local(_))
    ));

    assert!(!table.node_type_uses.is_empty());
}

#[test]
fn resolution_queries_share_compiler_session_node_owner() {
    let source = "static VALUE: i32 = 1; fn main() i32 { let local: i32 = VALUE; local }";
    let fixture = LoadedProgramFixture::new("main.nia", source);
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());
    let node_store_id = db.context().node_store().id();

    let values = db.expect_get(ValueResolutionQuery(module_id));
    assert_eq!(values.semantic.node_names.store_id(), node_store_id);
    assert_eq!(
        values.semantic.node_qualified_values.store_id(),
        node_store_id
    );
    assert_eq!(
        values.semantic.node_builtin_associated_values.store_id(),
        node_store_id
    );
    assert_eq!(values.semantic.node_variant_enums.store_id(), node_store_id);
    assert_eq!(
        values.semantic.node_qualified_type_prefixes.store_id(),
        node_store_id
    );

    let locals = db.expect_get(LocalResolutionQuery(module_id));
    assert_eq!(locals.semantic.node_local_defs.store_id(), node_store_id);
    assert_eq!(locals.semantic.node_uses.store_id(), node_store_id);

    let types = db.expect_get(TypeResolutionQuery(module_id));
    assert_eq!(
        types.semantic.node_const_generic_names.store_id(),
        node_store_id
    );
}
