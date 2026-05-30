// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_abi_check::check_module_abi;
use nia_body_check::{
    BodyCheckInput, ProgramSignatureMaps, check_module_bodies_with_program_signatures_and_layouts,
};
use nia_defs::{DefKind, VisibleExtensionMethod, VisibleExtensionMethods, collect_module_defs};
use nia_flow_check::check_module_flow;
use nia_function_ir::{FunctionArrayElements, FunctionExprKind, FunctionOp, FunctionTerminator};
use nia_function_lower::lower_function_body;
use nia_item_signatures::collect_item_signatures;
use nia_local_resolve::resolve_module_locals;
use nia_node_id::NodeOriginTable;
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
        defs: &defs,
        values: &values,
        locals: &locals,
        signatures: &signatures,
        interner: &normalization.interner,
        const_exprs: &type_lowering.const_exprs,
        program: nia_comptime_check::ComptimeProgramContext::empty(),
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
    let origins = NodeOriginTable::default();
    let body_check = check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
        source_version: None,
        origins: &origins,
        module: &module,
        defs: &defs,
        values: &values,
        locals: &locals,
        lowered: &type_lowering,
        signatures: &signatures,
        normalization: &normalization,
        comptime: &comptime,
        layouts: &layouts,
        extensions: &extensions,
        extension_interner: None,
        program: nia_body_check::BodyProgramContext::empty(),
        program_signatures: ProgramSignatureMaps {
            functions: &HashMap::new(),
            globals: &HashMap::new(),
            comptimes: &HashMap::new(),
            structs: &HashMap::new(),
            unions: &HashMap::new(),
            enums: &HashMap::new(),
        },
        program_comptime: nia_body_check::ProgramComptimeMaps {
            comptimes: &HashMap::new(),
        },
    });
    assert!(
        body_check.diagnostics.is_empty(),
        "{:?}",
        body_check.diagnostics
    );
    let function_bodies = body_check
        .ir
        .function_bodies
        .iter()
        .map(|(def_id, body)| (*def_id, lower_function_body(body)))
        .collect::<HashMap<_, _>>();

    let input = BackendLowerModuleInput {
        module_id: ModuleId(0),
        module_name: "main".to_string(),
        module: &module,
        defs: &defs,
        values: &values,
        locals: &locals,
        type_lowering: &type_lowering,
        signatures: &signatures,
        type_normalization: &normalization,
        body_check: &body_check,
        extensions: &extensions,
        comptime: &comptime,
        layouts: &layouts,
        function_bodies: &function_bodies,
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
        main.function_body
            .as_ref()
            .expect("main function body")
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
    let body = main.function_body.as_ref().expect("main function body");
    let Some(FunctionOp::Binding(binding)) = body.blocks[0].ops.first() else {
        panic!("expected buffer binding");
    };
    let value = binding.value.as_ref().expect("buffer initializer");
    let FunctionExprKind::ArrayLiteral {
        elems: FunctionArrayElements::Repeat { count, .. },
    } = &value.kind
    else {
        panic!("expected repeat array initializer");
    };
    assert_eq!(*count, 1048576);
}

#[test]
fn lowers_function_body_to_function_ir() {
    let source = r#"
fn main() i32 {
    defer {
    };
    var value = 1;
    return value;
}
"#;
    let lowering = lower_source(source);
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let function_body = main.function_body.as_ref().expect("main function body");
    assert_eq!(function_body.blocks.len(), 2);
    assert!(matches!(
        function_body.blocks[0].ops[0],
        FunctionOp::Defer(_)
    ));
    assert!(matches!(
        function_body.blocks[0].ops[1],
        FunctionOp::Binding(_)
    ));
    let FunctionTerminator::Next { target, .. } = function_body.blocks[0].terminator else {
        panic!("expected first block to continue to return terminator block");
    };
    assert!(matches!(
        function_body
            .block(target)
            .expect("return terminator block")
            .terminator,
        FunctionTerminator::Return { value: Some(_), .. }
    ));
}

#[test]
fn lowers_loop_break_and_continue_to_function_ir_branches() {
    let source = r#"
fn main() i32 {
    for {
        continue;
        break;
    }
    0
}
"#;
    let lowering = lower_source(source);
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let function_body = main.function_body.as_ref().expect("main function body");
    let FunctionTerminator::Next { target, .. } = function_body.blocks[0].terminator else {
        panic!("expected entry branch to loop header");
    };
    let FunctionTerminator::Loop {
        body,
        continue_target,
        ..
    } = function_body.block(target).expect("loop header").terminator
    else {
        panic!("expected loop terminator");
    };
    let body = function_body
        .blocks
        .iter()
        .find(|block| block.id == body)
        .expect("loop body block");
    assert_eq!(body.terminator.successors(), vec![continue_target]);
}

#[test]
fn instantiates_generic_function_instances_in_function_ir() {
    let source = r#"
fn id[T](value: T) T {
    value
}

fn main() i32 {
    id[i32](42)
}
"#;
    let lowering = lower_source(source);
    let module = &lowering.program.modules[0];
    let instance = module
        .function_instances
        .iter()
        .find(|instance| instance.name == "id")
        .expect("id instance");
    let body = instance
        .function_body
        .as_ref()
        .expect("id instance function body");
    let i32_ty = module.interner.primitive(nia_ty::PrimitiveTy::I32);

    assert_eq!(instance.params[0].ty, i32_ty);
    assert_eq!(instance.return_type, i32_ty);
    assert_eq!(body.ty, i32_ty);
    assert!(body.locals.iter().all(|local| local.ty == i32_ty));
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
        defs: &defs,
        values: &values,
        locals: &locals,
        signatures: &signatures,
        interner: &normalization.interner,
        const_exprs: &type_lowering.const_exprs,
        program: nia_comptime_check::ComptimeProgramContext::empty(),
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
    let origins = NodeOriginTable::default();
    let body_check = check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
        source_version: None,
        origins: &origins,
        module: &module,
        defs: &defs,
        values: &values,
        locals: &locals,
        lowered: &type_lowering,
        signatures: &signatures,
        normalization: &normalization,
        comptime: &comptime,
        layouts: &layouts,
        extensions: &extensions,
        extension_interner: None,
        program: nia_body_check::BodyProgramContext::empty(),
        program_signatures: ProgramSignatureMaps {
            functions: &HashMap::new(),
            globals: &HashMap::new(),
            comptimes: &HashMap::new(),
            structs: &HashMap::new(),
            unions: &HashMap::new(),
            enums: &HashMap::new(),
        },
        program_comptime: nia_body_check::ProgramComptimeMaps {
            comptimes: &HashMap::new(),
        },
    });
    assert!(
        body_check.diagnostics.is_empty(),
        "{:?}",
        body_check.diagnostics
    );
    let function_bodies = body_check
        .ir
        .function_bodies
        .iter()
        .map(|(def_id, body)| (*def_id, lower_function_body(body)))
        .collect::<HashMap<_, _>>();
    let monomorphization =
        nia_monomorphize::collect_monomorphizations(&[nia_monomorphize::MonomorphizeModuleInput {
            module_id: ModuleId(0),
            defs: &defs,
            interner: &body_check.ir.interner,
            comptime: &comptime,
            instantiations: &body_check.ir.generic_instantiations,
        }]);
    assert!(
        monomorphization.diagnostics.is_empty(),
        "{:?}",
        monomorphization.diagnostics
    );

    let input = BackendLowerModuleInput {
        module_id: ModuleId(0),
        module_name: "main".to_string(),
        module: &module,
        defs: &defs,
        values: &values,
        locals: &locals,
        type_lowering: &type_lowering,
        signatures: &signatures,
        type_normalization: &normalization,
        body_check: &body_check,
        extensions: &extensions,
        comptime: &comptime,
        layouts: &layouts,
        function_bodies: &function_bodies,
        extension_interner: None,
    };
    let lowering = lower_backend_program(&[input], &monomorphization);
    assert!(
        lowering.diagnostics.is_empty(),
        "{:?}",
        lowering.diagnostics
    );
    lowering
}
