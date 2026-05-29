// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_abi_check::check_module_abi;
use nia_body_check::{
    BodyCheckInput, ProgramSignatureMaps, check_module_bodies_with_program_signatures_and_layouts,
};
use nia_body_ir::{TypedArrayElements, TypedExprKind, TypedStmtKind};
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
const hello = c"hello";

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
    let comptime = nia_comptime_check::check_module_comptime(nia_comptime_check::ComptimeInput {
        module: &module,
        all_modules: std::slice::from_ref(&module),
        defs: &defs,
        all_defs: std::slice::from_ref(&defs),
        values: &values,
        locals: &locals,
        signatures: &signatures,
        interner: &normalization.interner,
        const_exprs: &type_lowering.const_exprs,
    });
    let layouts = nia_layout::compute_layouts_with_normalized_types(
        &defs,
        &normalization.interner,
        &signatures,
        &normalization.normalized,
        &comptime,
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
    let point_ty = normalization
        .interner
        .iter()
        .find_map(|(ty_id, ty)| {
            matches!(
                ty,
                nia_ty::TyKind::Nominal {
                    def_id,
                    args
                } if def_id.module_id == ModuleId(0) && def_id.def_id == point_id && args.is_empty()
            )
            .then_some(ty_id)
        })
        .expect("Point type");
    extensions.insert(
        point_ty,
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
        comptime: &comptime,
        layouts: &layouts,
        extensions: &extensions,
        extension_interner: None,
        program_signatures: ProgramSignatureMaps {
            functions: &HashMap::new(),
            globals: &HashMap::new(),
            comptimes: &HashMap::new(),
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
        all_modules: std::slice::from_ref(&module),
        defs: &defs,
        all_defs: std::slice::from_ref(&defs),
        values: &values,
        locals: &locals,
        type_lowering: &type_lowering,
        signatures: &signatures,
        type_normalization: &normalization,
        body_check: &body_check,
        extensions: &extensions,
        comptime: &comptime,
        layouts: &layouts,
        extension_interner: None,
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

#[test]
fn comptime_bindings_do_not_lower_to_storage() {
    let source = r#"
comptime answer: i32 = 40 + 2;

fn main() i32 {
    comptime local: i32 = answer;
    local
}
"#;
    let lowering = lower_source(source);
    let module = &lowering.program.modules[0];
    assert!(module.globals.is_empty());
    let main = module
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    assert!(
        main.body
            .as_ref()
            .expect("main body")
            .locals
            .iter()
            .all(|local| local.name != "local")
    );
}

#[test]
fn lowers_large_array_repeat_count_from_comptime_binding() {
    let source = r#"
comptime N: usize = 1048576;

fn main() i32 {
    var buffer: [N]u8 = [0u8; N];
    0
}
"#;
    let lowering = lower_source(source);
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.body.as_ref().expect("main body");
    let Some(TypedStmtKind::Binding(binding)) = body.stmts.first().map(|stmt| &stmt.kind) else {
        panic!("expected buffer binding");
    };
    let value = binding.value.as_ref().expect("buffer initializer");
    let TypedExprKind::ArrayLiteral {
        elems: TypedArrayElements::Repeat { count, .. },
    } = &value.kind
    else {
        panic!("expected repeat array initializer");
    };
    assert_eq!(*count, 1048576);
}

fn lower_source(source: &str) -> BackendLowering {
    let (module, errors) = parse_module(source);
    assert!(errors.is_empty(), "{errors:?}");
    let defs = collect_module_defs(ModuleId(0), &module);
    let type_resolved = resolve_module_types(&module, &defs);
    let type_lowering = lower_module_types_with_id(ModuleId(0), &module, &type_resolved);
    let signatures = collect_item_signatures(&module, &defs, &type_lowering);
    let values = resolve_module_values(&module, &defs);
    let locals = resolve_module_locals(&module, &defs, &values);
    let normalization = normalize_module_types(ModuleId(0), &type_lowering.interner, &signatures);
    let comptime = nia_comptime_check::check_module_comptime(nia_comptime_check::ComptimeInput {
        module: &module,
        all_modules: std::slice::from_ref(&module),
        defs: &defs,
        all_defs: std::slice::from_ref(&defs),
        values: &values,
        locals: &locals,
        signatures: &signatures,
        interner: &normalization.interner,
        const_exprs: &type_lowering.const_exprs,
    });
    let layouts = nia_layout::compute_layouts_with_normalized_types(
        &defs,
        &normalization.interner,
        &signatures,
        &normalization.normalized,
        &comptime,
        nia_layout::TargetDataLayout::LP64,
    );
    let extensions = VisibleExtensionMethods::default();
    let body_check = check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
        module: &module,
        defs: &defs,
        all_defs: std::slice::from_ref(&defs),
        values: &values,
        locals: &locals,
        lowered: &type_lowering,
        signatures: &signatures,
        normalization: &normalization,
        comptime: &comptime,
        layouts: &layouts,
        extensions: &extensions,
        extension_interner: None,
        program_signatures: ProgramSignatureMaps {
            functions: &HashMap::new(),
            globals: &HashMap::new(),
            comptimes: &HashMap::new(),
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
        all_modules: std::slice::from_ref(&module),
        defs: &defs,
        all_defs: std::slice::from_ref(&defs),
        values: &values,
        locals: &locals,
        type_lowering: &type_lowering,
        signatures: &signatures,
        type_normalization: &normalization,
        body_check: &body_check,
        extensions: &extensions,
        comptime: &comptime,
        layouts: &layouts,
        extension_interner: None,
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
    lowering
}
