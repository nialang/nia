// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_abi_check::check_module_abi;
use nia_body_check::{
    BodyCheckInput, ProgramSignatureMaps, check_module_bodies_with_program_signatures_and_layouts,
};
use nia_defs::{DefKind, VisibleExtensionMethod, VisibleExtensionMethods, collect_module_defs};
use nia_flow_check::check_module_flow;
use nia_item_signatures::collect_item_signatures;
use nia_local_resolve::resolve_module_locals;
use nia_parser::parse_module;
use nia_type_lower::lower_module_types_with_id;
use nia_type_normalize::normalize_module_types;
use nia_type_resolve::resolve_module_types;
use nia_value_resolve::resolve_module_values;
use std::collections::HashMap;

#[test]
fn lowers_checked_program_shape() {
    let source = r#"
const hello = "hello\0";

struct Point {
    x: i32,
    y: i32,
}

extend Point {
    fn make(x: i32, y: i32) Point {
        { x: x, y: y }
    }
}

fn main() i32 {
    var p = Point::make(1, 2);
    p.x
}
"#;
    let (module, errors) = parse_module(source);
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(ModuleId(0), &module);
    let type_resolved = resolve_module_types(&module, &defs);
    let type_lowering = lower_module_types_with_id(ModuleId(0), &module, &type_resolved);
    let signatures = collect_item_signatures(&module, &defs, &type_lowering);
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    let normalization = normalize_module_types(ModuleId(0), &type_lowering.interner, &signatures);
    let const_eval =
        nia_const_eval::eval_module_consts(&module, &defs, &type_lowering, &signatures);
    let layouts = nia_layout::compute_layouts_with_normalized_types(
        &defs,
        &normalization.interner,
        &signatures,
        &normalization.normalized,
        nia_layout::TargetDataLayout::LP64,
    );
    let _abi = check_module_abi(&defs, &type_lowering.interner, &signatures);
    let _flow = check_module_flow(&module, &type_lowering.interner, &signatures);
    let point_id = defs.module_scope.types.get("Point").expect("Point def");
    let make_id = defs
        .defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.kind == DefKind::Method && def.name == "make").then_some(def_id)
        })
        .expect("make def");
    let mut extensions = VisibleExtensionMethods::default();
    extensions.insert(
        GlobalDefId {
            module_id: ModuleId(0),
            def_id: point_id,
        },
        Vec::new(),
        VisibleExtensionMethod {
            name: "make".to_string(),
            def_id: GlobalDefId {
                module_id: ModuleId(0),
                def_id: make_id,
            },
        },
    );
    let body_check = check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
        module: &module,
        defs: &defs,
        all_defs: std::slice::from_ref(&defs),
        values: &values,
        locals: &locals,
        lowered: &type_lowering,
        signatures: &signatures,
        normalization: &normalization,
        layouts: &layouts,
        extensions: &extensions,
        program_signatures: ProgramSignatureMaps {
            functions: &HashMap::new(),
            globals: &HashMap::new(),
            structs: &HashMap::new(),
            unions: &HashMap::new(),
            enums: &HashMap::new(),
        },
    });
    assert!(
        body_check.diagnostics.is_empty(),
        "{:?}",
        body_check.diagnostics
    );

    let input = BackendLowerModuleInput {
        module_id: ModuleId(0),
        module_name: "main".to_string(),
        module: &module,
        defs: &defs,
        all_defs: std::slice::from_ref(&defs),
        values: &values,
        locals: &locals,
        type_lowering: &type_lowering,
        signatures: &signatures,
        body_check: &body_check,
        extensions: &extensions,
        const_eval: &const_eval,
        layouts: &layouts,
    };
    let lowering = lower_backend_program(
        &[input],
        &Monomorphization {
            instances: Vec::new(),
            diagnostics: Vec::new(),
        },
    );
    assert!(
        lowering.diagnostics.is_empty(),
        "{:?}",
        lowering.diagnostics
    );
    assert_eq!(lowering.program.modules.len(), 1);
    assert_eq!(lowering.program.modules[0].globals.len(), 1);
    assert_eq!(lowering.program.modules[0].functions.len(), 2);
}
