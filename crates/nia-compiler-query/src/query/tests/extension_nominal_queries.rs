// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn visible_extensions_use_signature_type_normalization_and_nominal_provider_queries() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module facade;
using entry::facade;

fn main(value: facade::Used) i32 {
    value.len()
}
"#,
    );
    let entry_id = fixture.entry_id();
    let facade_id = fixture.add_child(
        entry_id,
        "facade",
        "facade.nia",
        r#"
module impls;
module types;

pub using self::types::Used;
"#,
    );
    let impls_id = fixture.add_child_with_visibility(
        facade_id,
        "impls",
        nia_ids::Visibility::Private,
        "facade/impls.nia",
        r#"
using entry::facade::types::Used;

extend Used {
    pub fn len(&self) i32 {
        1
    }
}
"#,
    );
    fixture.add_child(facade_id, "types", "facade/types.nia", "pub struct Used {}");
    let impls_description = format!("{impls_id:?}");
    let mut loaded = fixture.program();
    loaded.runtime = RuntimeModel::FreestandingExecutable;
    let db = query_db(loaded);

    let _ = db.expect_get(VisibleExtensionsQuery(entry_id));
    let trace = db.query_trace();

    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "visible_extensions"
            && dependency.to.name == "signature_type_normalization"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "visible_extensions"
            && dependency.to.name == "extension_provider_nominal_modules_for_targets"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_provider_nominal_modules_for_targets"
            && dependency.to.name == "extension_provider_nominal_target_names"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_provider_nominal_modules_for_targets"
            && dependency.to.name == "type_exposure_index"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_provider_nominal_modules_for_targets"
            && dependency.to.name == "extension_provider_nominal_candidate_modules"
    }));
    assert!(
        !trace.dependencies.iter().any(|dependency| {
            dependency.to.name == "extension_provider_nominal_candidate_index"
        })
    );
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_provider_nominal_modules_for_targets"
            && dependency.to.name == "extension_provider_nominal_module_facts"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.to.name == "extension_provider_nominal_conservative_target_index"
    }));
    assert!(
        !trace
            .dependencies
            .iter()
            .any(|dependency| dependency.to.name == "extension_provider_nominal_index")
    );
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "visible_extensions"
            && dependency.to.name == "extension_provider_nominal_modules"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_provider_nominal_modules"
            && dependency.to.name == "extension_provider_module_facts"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "visible_extensions"
            && dependency.to.name == "extension_provider_module_facts"
            && dependency.to.description.contains(&impls_description)
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "visible_extensions"
            && dependency.to.name == "extension_method_index"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "visible_extensions" && dependency.to.name == "program_defs_by_id"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "visible_extensions" && dependency.to.name == "module_defs"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "visible_extensions"
            && dependency.to.name == "program_full_defs_by_id"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "visible_extensions"
            && dependency.to.name == "program_type_normalizations"
    }));
    assert!(
        !trace
            .dependencies
            .iter()
            .any(|dependency| dependency.to.name == "program_visible_type_signatures")
    );
    assert!(!depends_on_body_signature_query(
        &trace,
        "visible_extensions"
    ));
}
