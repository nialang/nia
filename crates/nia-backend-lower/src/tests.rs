// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_abi_check::check_module_abi;
use nia_body_check::{BodyCheckInput, check_module_bodies_with_program_signatures_and_layouts};
use nia_defs::{DefKind, VisibleExtensionMethod, VisibleExtensionMethods, collect_module_defs};
use nia_flow_check::check_module_flow;
use nia_function_ir::{
    FunctionArrayElements, FunctionBlockId, FunctionExpr, FunctionExprKind, FunctionOp,
    FunctionTerminator,
};
use nia_function_lower::lower_function_body;
use nia_ids::LocalId;
use nia_item_signatures::{ProgramSignatureMaps, collect_item_signatures};
use nia_local_resolve::resolve_module_locals;
use nia_node_id::NodeOriginTable;
use nia_parser::parse_module;
use nia_static_ir::StaticInit;
use nia_type_lower::{TypeLowering, lower_module_types_with_id};
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
        type_uses: &type_lowering.type_uses,
        normalized: &normalization.normalized,
        const_exprs: &type_lowering.const_exprs,
        program: nia_comptime_check::ComptimeProgramContext::empty(),
    });
    let layouts = nia_layout::compute_layouts_with_normalized_types(
        &defs,
        &normalization.interner,
        &signatures,
        &normalization.normalized,
        &|id| comptime.array_lengths.get(&id).copied(),
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
            trait_id: None,
            trait_args: Vec::new(),
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
            traits: &HashMap::new(),
            trait_impls: &[],
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
        program_enums: &HashMap::new(),
        program_traits: &HashMap::new(),
        trait_impls: &[],
    };
    let lowering = lower_backend_program(
        &[input],
        &Monomorphization {
            instances: Vec::new(),
            diagnostics: Vec::new(),
        },
        nia_opt::OptimizationPolicy::default(),
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
    loop {
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

#[test]
fn instantiates_nested_generic_function_instance_args_in_visible_interner() {
    let source = r#"
fn inner[T](value: T) T {
    value
}

fn outer[T](value: &const T) &const T {
    inner[&const T](value)
}

fn main() i32 {
    var value = 1;
    var ptr = &const value;
    _ = outer[i32](ptr);
    0
}
"#;
    let lowering = lower_source(source);
    let module = &lowering.program.modules[0];
    let i32_ty = module.interner.primitive(nia_ty::PrimitiveTy::I32);
    let i32_ptr = module
        .interner
        .iter()
        .find_map(|(ty_id, ty)| {
            matches!(
                ty,
                nia_ty::TyKind::Pointer {
                    is_const: true,
                    elem,
                } if *elem == i32_ty
            )
            .then_some(ty_id)
        })
        .expect("&const i32 type");
    let instance = module
        .function_instances
        .iter()
        .find(|instance| instance.name == "inner")
        .expect("inner instance");

    assert_eq!(instance.args, vec![i32_ptr]);
    assert_eq!(instance.params[0].ty, i32_ptr);
    assert_eq!(instance.return_type, i32_ptr);
    assert!(module.interner.get(instance.args[0]).is_some());
}

#[test]
fn o1_removes_unreachable_backend_function_blocks() {
    let source = r#"
fn main() i32 {
    defer {
    };
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let mut unreachable = body.blocks[0].clone();
            unreachable.id = FunctionBlockId(999);
            unreachable.ops.clear();
            unreachable.terminator = FunctionTerminator::Return {
                value: None,
                span: unreachable.span,
            };
            body.blocks.push(unreachable);
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.block(FunctionBlockId(999)).is_none());
}

#[test]
fn o0_preserves_unreachable_backend_function_blocks() {
    let source = r#"
fn main() i32 {
    defer {
    };
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let mut unreachable = body.blocks[0].clone();
            unreachable.id = FunctionBlockId(999);
            unreachable.ops.clear();
            unreachable.terminator = FunctionTerminator::Return {
                value: None,
                span: unreachable.span,
            };
            body.blocks.push(unreachable);
        },
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.block(FunctionBlockId(999)).is_some());
}

#[test]
fn o1_merges_empty_backend_function_jump_blocks() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let mut original = body.blocks[0].clone();
            let empty_id = FunctionBlockId(998);
            let original_id = FunctionBlockId(999);
            original.id = original_id;
            body.blocks[0].terminator = FunctionTerminator::Next {
                target: empty_id,
                span: body.blocks[0].span,
            };
            body.blocks.push(nia_function_ir::FunctionBlock {
                id: empty_id,
                scope: original.scope,
                span: original.span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Next {
                    target: original_id,
                    span: original.span,
                },
            });
            body.blocks.push(original);
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.block(FunctionBlockId(998)).is_none());
}

#[test]
fn o0_preserves_empty_backend_function_jump_blocks() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let mut original = body.blocks[0].clone();
            let empty_id = FunctionBlockId(998);
            let original_id = FunctionBlockId(999);
            original.id = original_id;
            body.blocks[0].terminator = FunctionTerminator::Next {
                target: empty_id,
                span: body.blocks[0].span,
            };
            body.blocks.push(nia_function_ir::FunctionBlock {
                id: empty_id,
                scope: original.scope,
                span: original.span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Next {
                    target: original_id,
                    span: original.span,
                },
            });
            body.blocks.push(original);
        },
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.block(FunctionBlockId(998)).is_some());
}

#[test]
fn o1_folds_constant_bool_backend_if_branches() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let mut original = body.blocks[0].clone();
            let then_id = FunctionBlockId(998);
            let else_id = FunctionBlockId(999);
            original.id = then_id;
            body.blocks[0].terminator = FunctionTerminator::If {
                cond: nia_function_ir::FunctionExpr {
                    span: body.blocks[0].span,
                    ty: body.ty,
                    kind: FunctionExprKind::Bool(false),
                },
                then_target: then_id,
                else_target: else_id,
                span: body.blocks[0].span,
            };
            body.blocks.push(original);
            let mut selected = body.blocks[0].clone();
            selected.id = else_id;
            selected.terminator = FunctionTerminator::Return {
                value: None,
                span: selected.span,
            };
            body.blocks.push(selected);
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.block(FunctionBlockId(998)).is_none());
    assert!(body.block(FunctionBlockId(999)).is_some());
}

#[test]
fn o1_simplifies_constant_logical_backend_if_conditions() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let mut original = body.blocks[0].clone();
            let then_id = FunctionBlockId(998);
            let else_id = FunctionBlockId(999);
            original.id = then_id;
            body.blocks[0].terminator = FunctionTerminator::If {
                cond: nia_function_ir::FunctionExpr {
                    span: body.blocks[0].span,
                    ty: body.ty,
                    kind: FunctionExprKind::Binary {
                        lhs: Box::new(nia_function_ir::FunctionExpr {
                            span: body.blocks[0].span,
                            ty: body.ty,
                            kind: FunctionExprKind::Bool(true),
                        }),
                        op: nia_ast::BinaryOp::And,
                        rhs: Box::new(nia_function_ir::FunctionExpr {
                            span: body.blocks[0].span,
                            ty: body.ty,
                            kind: FunctionExprKind::Bool(false),
                        }),
                    },
                },
                then_target: then_id,
                else_target: else_id,
                span: body.blocks[0].span,
            };
            body.blocks.push(original);
            let mut selected = body.blocks[0].clone();
            selected.id = else_id;
            selected.terminator = FunctionTerminator::Return {
                value: None,
                span: selected.span,
            };
            body.blocks.push(selected);
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.block(FunctionBlockId(998)).is_none());
    assert!(body.block(FunctionBlockId(999)).is_some());
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "simplify-constant-logical-exprs",
                    ..
                }
            ))
    );
}

#[test]
fn o0_preserves_constant_bool_backend_if_branches() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let mut original = body.blocks[0].clone();
            let then_id = FunctionBlockId(998);
            let else_id = FunctionBlockId(999);
            original.id = then_id;
            body.blocks[0].terminator = FunctionTerminator::If {
                cond: nia_function_ir::FunctionExpr {
                    span: body.blocks[0].span,
                    ty: body.ty,
                    kind: FunctionExprKind::Bool(false),
                },
                then_target: then_id,
                else_target: else_id,
                span: body.blocks[0].span,
            };
            body.blocks.push(original);
            let mut selected = body.blocks[0].clone();
            selected.id = else_id;
            selected.terminator = FunctionTerminator::Return {
                value: None,
                span: selected.span,
            };
            body.blocks.push(selected);
        },
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.block(FunctionBlockId(998)).is_some());
    assert!(body.block(FunctionBlockId(999)).is_some());
}

#[test]
fn o1_removes_same_type_backend_casts() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let value = first_terminal_value_mut(body);
            let original = value.clone();
            value.kind = FunctionExprKind::Cast {
                expr: Box::new(original),
                ty: value.ty,
            };
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");
    let value = first_terminal_value(body);

    assert!(!matches!(value.kind, FunctionExprKind::Cast { .. }));
}

#[test]
fn o0_preserves_same_type_backend_casts() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let value = first_terminal_value_mut(body);
            let original = value.clone();
            value.kind = FunctionExprKind::Cast {
                expr: Box::new(original),
                ty: value.ty,
            };
        },
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");
    let value = first_terminal_value(body);

    assert!(matches!(value.kind, FunctionExprKind::Cast { .. }));
}

#[test]
fn o2_simplifies_zero_static_initializers() {
    let source = r#"
const zeroes: [4]i32 = [0; 4];

fn main() i32 {
    zeroes[0]
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let zeroes = lowering.program.modules[0]
        .globals
        .iter()
        .find(|global| global.name == "zeroes")
        .expect("zeroes global");

    assert!(matches!(zeroes.init, Some(StaticInit::Zero)));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Global {
                    global,
                    pass: "simplify-static-init",
                    ..
                } if *global == zeroes.def_id
            ))
    );
}

#[test]
fn o2_simplifies_zero_float_static_initializers() {
    let source = r#"
const zeroes: [2]f64 = [0.0f64, 0.0];

fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let zeroes = lowering.program.modules[0]
        .globals
        .iter()
        .find(|global| global.name == "zeroes")
        .expect("zeroes global");

    assert!(matches!(zeroes.init, Some(StaticInit::Zero)));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Global {
                    global,
                    pass: "simplify-static-init",
                    ..
                } if *global == zeroes.def_id
            ))
    );
}

#[test]
fn o2_simplifies_empty_repeat_static_initializers() {
    let source = r#"
const values: [0]i32 = [1; 0];

fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let values = lowering.program.modules[0]
        .globals
        .iter()
        .find(|global| global.name == "values")
        .expect("values global");

    assert!(matches!(values.init, Some(StaticInit::Zero)));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Global {
                    global,
                    pass: "simplify-static-init",
                    ..
                } if *global == values.def_id
            ))
    );
}

#[test]
fn o2_simplifies_repeated_static_array_initializers() {
    let source = r#"
const values: [3]i32 = [7, 7, 7];

fn main() i32 {
    values[0]
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let values = lowering.program.modules[0]
        .globals
        .iter()
        .find(|global| global.name == "values")
        .expect("values global");

    assert!(matches!(
        values.init,
        Some(StaticInit::Repeat {
            ref value,
            count: 3
        }) if matches!(**value, StaticInit::Int(7))
    ));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Global {
                    global,
                    pass: "simplify-static-init",
                    ..
                } if *global == values.def_id
            ))
    );
}

#[test]
fn o2_simplifies_repeated_byte_static_initializers() {
    let source = r#"
const bytes: [3]u8 = b"aaa";

fn main() u8 {
    bytes[0]
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let bytes = lowering.program.modules[0]
        .globals
        .iter()
        .find(|global| global.name == "bytes")
        .expect("bytes global");

    assert!(matches!(
        bytes.init,
        Some(StaticInit::Repeat {
            ref value,
            count: 3
        }) if matches!(**value, StaticInit::Byte(b'a'))
    ));
}

#[test]
fn o2_simplifies_repeated_char_static_initializers() {
    let source = r#"
const text: [3]char = "aaa";

fn main() char {
    text[0]
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let text = lowering.program.modules[0]
        .globals
        .iter()
        .find(|global| global.name == "text")
        .expect("text global");

    assert!(matches!(
        text.init,
        Some(StaticInit::Repeat {
            ref value,
            count: 3
        }) if matches!(**value, StaticInit::Char(value) if value == 'a' as u32)
    ));
}

#[test]
fn size_levels_simplify_static_initializers_for_size() {
    let source = r#"
const zeroes: [4]i32 = [0, 0, 0, 0];
const values: [3]i32 = [7, 7, 7];

fn main() i32 {
    zeroes[0] + values[0]
}
"#;

    for level in [
        nia_opt::NiaOptimizationLevel::Os,
        nia_opt::NiaOptimizationLevel::Oz,
    ] {
        let lowering =
            lower_source_with_body_mutation_and_optimization(source, |_| {}, level.policy());
        let module = &lowering.program.modules[0];
        let zeroes = module
            .globals
            .iter()
            .find(|global| global.name == "zeroes")
            .expect("zeroes global");
        let values = module
            .globals
            .iter()
            .find(|global| global.name == "values")
            .expect("values global");

        assert!(matches!(zeroes.init, Some(StaticInit::Zero)), "{level:?}");
        assert!(
            matches!(
                values.init,
                Some(StaticInit::Repeat {
                    ref value,
                    count: 3
                }) if matches!(**value, StaticInit::Int(7))
            ),
            "{level:?}"
        );
        assert!(
            lowering
                .optimization_report
                .changed_passes
                .iter()
                .filter(|change| matches!(
                    change,
                    BackendOptimizationChange::Global {
                        pass: "simplify-static-init",
                        ..
                    }
                ))
                .count()
                >= 2,
            "{level:?}"
        );
    }
}

#[test]
fn o1_preserves_zero_static_initializers() {
    let source = r#"
const zeroes: [4]i32 = [0; 4];

fn main() i32 {
    zeroes[0]
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let zeroes = lowering.program.modules[0]
        .globals
        .iter()
        .find(|global| global.name == "zeroes")
        .expect("zeroes global");

    assert!(matches!(
        zeroes.init,
        Some(StaticInit::Repeat { .. } | StaticInit::Array(_))
    ));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Global {
                    pass: "simplify-static-init",
                    ..
                }
            ))
    );
}

#[test]
fn o1_preserves_zero_float_static_initializers() {
    let source = r#"
const zeroes: [2]f32 = [0.0f32, 0.0f32];

fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let zeroes = lowering.program.modules[0]
        .globals
        .iter()
        .find(|global| global.name == "zeroes")
        .expect("zeroes global");

    assert!(matches!(zeroes.init, Some(StaticInit::Array(_))));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Global {
                    pass: "simplify-static-init",
                    ..
                }
            ))
    );
}

#[test]
fn o1_preserves_repeated_static_array_initializers() {
    let source = r#"
const values: [3]i32 = [7, 7, 7];

fn main() i32 {
    values[0]
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let values = lowering.program.modules[0]
        .globals
        .iter()
        .find(|global| global.name == "values")
        .expect("values global");

    assert!(matches!(values.init, Some(StaticInit::Array(_))));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Global {
                    pass: "simplify-static-init",
                    ..
                }
            ))
    );
}

#[test]
fn o1_preserves_repeated_string_static_initializers() {
    let source = r#"
const bytes: [3]u8 = b"aaa";
const text: [3]char = "aaa";

fn main() u8 {
    bytes[0]
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let module = &lowering.program.modules[0];
    let bytes = module
        .globals
        .iter()
        .find(|global| global.name == "bytes")
        .expect("bytes global");
    let text = module
        .globals
        .iter()
        .find(|global| global.name == "text")
        .expect("text global");

    assert!(matches!(bytes.init, Some(StaticInit::Bytes(_))));
    assert!(matches!(text.init, Some(StaticInit::Chars(_))));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Global {
                    pass: "simplify-static-init",
                    ..
                }
            ))
    );
}

#[test]
fn o2_removes_unused_private_functions() {
    let source = r#"
fn used(value: i32) i32 {
    var out = value;
    out
}

fn unused() i32 {
    2
}

pub fn exported() i32 {
    3
}

fn main() i32 {
    used(1)
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let module = &lowering.program.modules[0];

    assert!(
        module
            .functions
            .iter()
            .any(|function| function.name == "used")
    );
    assert!(
        module
            .functions
            .iter()
            .any(|function| function.name == "main")
    );
    assert!(
        module
            .functions
            .iter()
            .any(|function| function.name == "exported")
    );
    assert!(
        !module
            .functions
            .iter()
            .any(|function| function.name == "unused")
    );
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "remove-unused-functions",
                    is_instance: false,
                    ..
                }
            ))
    );
}

#[test]
fn o2_does_not_preserve_function_refs_inside_empty_repeat_static_initializers() {
    let source = r#"
const values: [0]i32 = [1; 0];

fn unused() i32 {
    1
}

fn main() i32 {
    0
}
"#;
    let policy = nia_opt::OptimizationPolicy {
        level: nia_opt::NiaOptimizationLevel::O2,
        simplify_cfg: nia_opt::OptimizationDepth::Disabled,
        const_fold: nia_opt::OptimizationDepth::Disabled,
        dead_code_elim: nia_opt::OptimizationDepth::Full,
        local_copy_prop: nia_opt::OptimizationDepth::Disabled,
        inline_threshold: nia_opt::InlineThreshold::Never,
        specialize_generics: nia_opt::SpecializationPolicy::RequiredOnly,
        dedup_monomorphized_instances: true,
        prefer_size: false,
    };
    let lowering = lower_source_with_body_check_mutation_and_optimization(
        source,
        |_| {},
        |_, _, _, _| {},
        |_, _| {},
        |body_check, _, defs, _| {
            let values = global_def_id_by_name(defs, "values");
            let unused = global_def_id_by_name(defs, "unused");
            body_check.ir.global_inits.insert(
                values,
                StaticInit::Repeat {
                    value: Box::new(StaticInit::AddrOfFunction {
                        function: unused,
                        args: Vec::new(),
                    }),
                    count: 0,
                },
            );
        },
        policy,
    );
    let module = &lowering.program.modules[0];

    assert!(
        !module
            .functions
            .iter()
            .any(|function| function.name == "unused")
    );
    let values = module
        .globals
        .iter()
        .find(|global| global.name == "values")
        .expect("values global");
    assert!(matches!(
        values.init,
        Some(StaticInit::Repeat { count: 0, .. })
    ));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "remove-unused-functions",
                    is_instance: false,
                    ..
                }
            ))
    );
}

#[test]
fn o2_preserves_function_refs_inside_static_initializers() {
    let source = r#"
const values: [2]usize = [0, 0];

fn kept() i32 {
    1
}

fn kept_id[T](value: T) T {
    value
}

fn seed() i32 {
    kept_id[i32](1)
}

fn main() i32 {
    0
}
"#;
    let policy = nia_opt::OptimizationPolicy {
        level: nia_opt::NiaOptimizationLevel::O2,
        simplify_cfg: nia_opt::OptimizationDepth::Disabled,
        const_fold: nia_opt::OptimizationDepth::Disabled,
        dead_code_elim: nia_opt::OptimizationDepth::Full,
        local_copy_prop: nia_opt::OptimizationDepth::Disabled,
        inline_threshold: nia_opt::InlineThreshold::Never,
        specialize_generics: nia_opt::SpecializationPolicy::RequiredOnly,
        dedup_monomorphized_instances: true,
        prefer_size: false,
    };
    let lowering = lower_source_with_body_check_mutation_and_optimization(
        source,
        |_| {},
        |_, _, _, _| {},
        |_, _| {},
        |body_check, _, defs, _| {
            let values = global_def_id_by_name(defs, "values");
            let kept = global_def_id_by_name(defs, "kept");
            let kept_id = global_def_id_by_name(defs, "kept_id");
            let i32_ty = body_check.ir.interner.primitive(nia_ty::PrimitiveTy::I32);
            body_check.ir.global_inits.insert(
                values,
                StaticInit::Array(vec![
                    StaticInit::AddrOfFunction {
                        function: kept,
                        args: Vec::new(),
                    },
                    StaticInit::AddrOfFunction {
                        function: kept_id,
                        args: vec![i32_ty],
                    },
                ]),
            );
        },
        policy,
    );
    let module = &lowering.program.modules[0];

    assert!(
        module
            .functions
            .iter()
            .any(|function| function.name == "kept")
    );
    assert!(
        !module
            .functions
            .iter()
            .any(|function| function.name == "seed")
    );
    assert!(
        module
            .function_instances
            .iter()
            .any(|instance| instance.name == "kept_id")
    );
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "remove-unused-functions",
                    is_instance: false,
                    ..
                }
            ))
    );
}

#[test]
fn o2_preserves_transitively_used_private_functions() {
    let source = r#"
fn leaf(value: i32) i32 {
    var out = value;
    out
}

fn middle() i32 {
    var out = leaf(1);
    out
}

fn unused() i32 {
    2
}

fn main() i32 {
    middle()
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let module = &lowering.program.modules[0];

    assert!(
        module
            .functions
            .iter()
            .any(|function| function.name == "leaf")
    );
    assert!(
        module
            .functions
            .iter()
            .any(|function| function.name == "middle")
    );
    assert!(
        !module
            .functions
            .iter()
            .any(|function| function.name == "unused")
    );
}

#[test]
fn o1_preserves_unused_private_functions() {
    let source = r#"
fn unused() i32 {
    2
}

fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let module = &lowering.program.modules[0];

    assert!(
        module
            .functions
            .iter()
            .any(|function| function.name == "unused")
    );
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "remove-unused-functions",
                    ..
                }
            ))
    );
}

#[test]
fn o2_removes_unused_private_function_instances() {
    let source = r#"
fn unused_id[T](value: T) T {
    value
}

fn unused() i32 {
    unused_id[i32](2)
}

fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let module = &lowering.program.modules[0];

    assert!(
        !module
            .function_instances
            .iter()
            .any(|instance| instance.name == "unused_id")
    );
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "remove-unused-function-instances",
                    is_instance: true,
                    ..
                }
            ))
    );
}

#[test]
fn o2_preserves_used_private_function_instances() {
    let source = r#"
fn id[T](value: T) T {
    var out = value;
    out
}

fn main() i32 {
    id[i32](1)
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let module = &lowering.program.modules[0];

    assert!(
        module
            .function_instances
            .iter()
            .any(|instance| instance.name == "id")
    );
}

#[test]
fn exact_function_instance_keys_are_deduplicated() {
    let source = r#"
fn id[T](value: T) T {
    var out = value;
    out
}

fn main() i32 {
    id[i32](1) + id[i32](2)
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let module = &lowering.program.modules[0];
    let instances = module
        .function_instances
        .iter()
        .filter(|instance| instance.name == "id")
        .collect::<Vec<_>>();

    assert_eq!(instances.len(), 1);
    assert_eq!(instances[0].args.len(), 1);
}

#[test]
fn o2_preserves_transitively_used_private_function_instances() {
    let source = r#"
fn id[T](value: T) T {
    var out = value;
    out
}

fn wrapper[T](value: T) T {
    var out = id[T](value);
    out
}

fn main() i32 {
    wrapper[i32](1)
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let module = &lowering.program.modules[0];

    assert!(
        module
            .function_instances
            .iter()
            .any(|instance| instance.name == "id")
    );
    assert!(
        module
            .function_instances
            .iter()
            .any(|instance| instance.name == "wrapper")
    );
}

#[test]
fn o2_preserves_public_function_instances() {
    let source = r#"
pub fn id[T](value: T) T {
    value
}

fn unused() i32 {
    id[i32](1)
}

fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let module = &lowering.program.modules[0];

    assert!(
        module
            .function_instances
            .iter()
            .any(|instance| instance.name == "id")
    );
}

#[test]
fn o1_inlines_constant_leaf_function_calls() {
    let source = r#"
fn answer() i32 {
    42
}

fn main() i32 {
    answer()
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

    assert!(matches!(value.kind, FunctionExprKind::Integer(ref text) if text == "42"));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "inline-leaf-functions",
                    is_instance: false,
                    ..
                }
            ))
    );
}

#[test]
fn o0_preserves_constant_leaf_function_calls() {
    let source = r#"
fn answer() i32 {
    42
}

fn main() i32 {
    answer()
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

    assert!(matches!(value.kind, FunctionExprKind::Call { .. }));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "inline-leaf-functions",
                    ..
                }
            ))
    );
}

#[test]
fn o1_inlines_constant_leaf_function_instance_calls() {
    let source = r#"
fn one[T]() i32 {
    1
}

fn main() i32 {
    one[i32]()
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

    assert!(matches!(value.kind, FunctionExprKind::Integer(ref text) if text == "1"));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "inline-leaf-functions",
                    is_instance: true,
                    type_arg_count: 1,
                    ..
                }
            ))
    );
}

#[test]
fn o2_inlines_tiny_pure_leaf_function_calls() {
    let source = r#"
fn pair() [2]i32 {
    [1, 2]
}

fn main() [2]i32 {
    pair()
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

    assert!(matches!(value.kind, FunctionExprKind::ArrayLiteral { .. }));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "inline-leaf-functions",
                    is_instance: false,
                    ..
                }
            ))
    );
}

#[test]
fn o1_preserves_tiny_pure_leaf_function_calls() {
    let source = r#"
fn pair() [2]i32 {
    [1, 2]
}

fn main() [2]i32 {
    pair()
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

    assert!(matches!(value.kind, FunctionExprKind::Call { .. }));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "inline-leaf-functions",
                    ..
                }
            ))
    );
}

#[test]
fn size_levels_preserve_non_constant_pure_leaf_function_calls() {
    let source = r#"
fn pair() [2]i32 {
    [1, 2]
}

fn main() [2]i32 {
    pair()
}
"#;

    for level in [
        nia_opt::NiaOptimizationLevel::Os,
        nia_opt::NiaOptimizationLevel::Oz,
    ] {
        let lowering =
            lower_source_with_body_mutation_and_optimization(source, |_| {}, level.policy());
        let main = lowering.program.modules[0]
            .functions
            .iter()
            .find(|function| function.name == "main")
            .expect("main function");
        let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

        assert!(
            matches!(value.kind, FunctionExprKind::Call { .. }),
            "{level:?}"
        );
        assert!(
            lowering
                .optimization_report
                .changed_passes
                .iter()
                .all(|change| !matches!(
                    change,
                    BackendOptimizationChange::Function {
                        pass: "inline-leaf-functions",
                        ..
                    }
                )),
            "{level:?}"
        );
    }
}

#[test]
fn o2_inlines_tiny_pure_leaf_function_calls_with_params() {
    let source = r#"
fn identity(value: i32) i32 {
    value
}

fn main() i32 {
    identity(7)
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

    assert!(matches!(value.kind, FunctionExprKind::Integer(ref text) if text == "7"));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "inline-leaf-functions",
                    is_instance: false,
                    ..
                }
            ))
    );
}

#[test]
fn size_levels_inline_single_param_forwarding_wrappers() {
    let source = r#"
fn identity(value: i32) i32 {
    value
}

fn main() i32 {
    identity(7)
}
"#;

    for level in [
        nia_opt::NiaOptimizationLevel::Os,
        nia_opt::NiaOptimizationLevel::Oz,
    ] {
        let lowering =
            lower_source_with_body_mutation_and_optimization(source, |_| {}, level.policy());
        let main = lowering.program.modules[0]
            .functions
            .iter()
            .find(|function| function.name == "main")
            .expect("main function");
        let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

        assert!(
            matches!(value.kind, FunctionExprKind::Integer(ref text) if text == "7"),
            "{level:?}"
        );
        assert!(
            lowering
                .optimization_report
                .changed_passes
                .iter()
                .any(|change| matches!(
                    change,
                    BackendOptimizationChange::Function {
                        pass: "inline-leaf-functions",
                        is_instance: false,
                        ..
                    }
                )),
            "{level:?}"
        );
    }
}

#[test]
fn size_levels_prune_inlined_private_generic_forwarding_instances() {
    let source = r#"
fn identity[T](value: T) T {
    value
}

fn main() i32 {
    identity[i32](7)
}
"#;

    for level in [
        nia_opt::NiaOptimizationLevel::Os,
        nia_opt::NiaOptimizationLevel::Oz,
    ] {
        let lowering =
            lower_source_with_body_mutation_and_optimization(source, |_| {}, level.policy());
        let module = &lowering.program.modules[0];
        let main = module
            .functions
            .iter()
            .find(|function| function.name == "main")
            .expect("main function");
        let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

        assert!(
            matches!(value.kind, FunctionExprKind::Integer(ref text) if text == "7"),
            "{level:?}"
        );
        assert!(
            !module
                .function_instances
                .iter()
                .any(|instance| instance.name == "identity"),
            "{level:?}"
        );
        assert!(
            lowering
                .optimization_report
                .changed_passes
                .iter()
                .any(|change| matches!(
                    change,
                    BackendOptimizationChange::Function {
                        pass: "inline-leaf-functions",
                        is_instance: true,
                        ..
                    }
                )),
            "{level:?}"
        );
        assert!(
            lowering
                .optimization_report
                .changed_passes
                .iter()
                .any(|change| matches!(
                    change,
                    BackendOptimizationChange::Function {
                        pass: "remove-unused-function-instances",
                        is_instance: true,
                        ..
                    }
                )),
            "{level:?}"
        );
    }
}

#[test]
fn size_levels_preserve_multi_param_forwarding_wrappers() {
    let source = r#"
fn first(left: i32, right: i32) i32 {
    left
}

fn effect() i32 {
    var value = 1;
    value
}

fn main() i32 {
    first(7, effect())
}
"#;

    for level in [
        nia_opt::NiaOptimizationLevel::Os,
        nia_opt::NiaOptimizationLevel::Oz,
    ] {
        let lowering =
            lower_source_with_body_mutation_and_optimization(source, |_| {}, level.policy());
        let main = lowering.program.modules[0]
            .functions
            .iter()
            .find(|function| function.name == "main")
            .expect("main function");
        let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

        assert!(
            matches!(value.kind, FunctionExprKind::Call { .. }),
            "{level:?}"
        );
        assert!(
            lowering
                .optimization_report
                .changed_passes
                .iter()
                .all(|change| !matches!(
                    change,
                    BackendOptimizationChange::Function {
                        pass: "inline-leaf-functions",
                        ..
                    }
                )),
            "{level:?}"
        );
    }
}

#[test]
fn o3_inlines_larger_pure_leaf_function_calls_than_o2() {
    let source = r#"
fn values() [5]i32 {
    [1, 2, 3, 4, 5]
}

fn main() [5]i32 {
    values()
}
"#;
    let o2 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let o2_main = o2.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let o2_value = first_terminal_value(o2_main.function_body.as_ref().expect("main body"));

    assert!(matches!(o2_value.kind, FunctionExprKind::Call { .. }));
    assert!(
        o2.optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "inline-leaf-functions",
                    ..
                }
            ))
    );

    let o3 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O3.policy(),
    );
    let o3_main = o3.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let o3_value = first_terminal_value(o3_main.function_body.as_ref().expect("main body"));

    assert!(matches!(
        o3_value.kind,
        FunctionExprKind::ArrayLiteral { .. }
    ));
    assert!(
        o3.optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "inline-leaf-functions",
                    is_instance: false,
                    ..
                }
            ))
    );
}

#[test]
fn o3_inlines_larger_pure_leaf_function_instance_calls_than_o2() {
    let source = r#"
fn values[T]() [5]i32 {
    [1, 2, 3, 4, 5]
}

fn main() [5]i32 {
    values[i32]()
}
"#;
    let o2 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let o2_main = o2.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let o2_value = first_terminal_value(o2_main.function_body.as_ref().expect("main body"));

    assert!(matches!(o2_value.kind, FunctionExprKind::Call { .. }));
    assert!(
        o2.optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "inline-leaf-functions",
                    is_instance: true,
                    ..
                }
            ))
    );

    let o3 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O3.policy(),
    );
    let o3_main = o3.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let o3_value = first_terminal_value(o3_main.function_body.as_ref().expect("main body"));

    assert!(matches!(
        o3_value.kind,
        FunctionExprKind::ArrayLiteral { .. }
    ));
    assert!(
        o3.optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "inline-leaf-functions",
                    is_instance: true,
                    type_arg_count: 1,
                    ..
                }
            ))
    );
}

#[test]
fn o3_propagates_cross_function_constant_returns() {
    let source = r#"
fn answer() i32 {
    42
}

fn main() i32 {
    answer()
}
"#;
    let o2 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let o2_main = o2.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let o2_value = first_terminal_value(o2_main.function_body.as_ref().expect("main body"));

    assert!(matches!(o2_value.kind, FunctionExprKind::Integer(ref text) if text == "42"));
    assert!(
        o2.optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "propagate-cross-function-constants",
                    ..
                }
            ))
    );

    let o3 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O3.policy(),
    );
    let o3_main = o3.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let o3_value = first_terminal_value(o3_main.function_body.as_ref().expect("main body"));

    assert!(matches!(o3_value.kind, FunctionExprKind::Integer(ref text) if text == "42"));
    assert!(
        o3.optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "propagate-cross-function-constants",
                    is_instance: false,
                    ..
                }
            ))
    );
}

#[test]
fn o3_does_not_cross_propagate_parameterized_leaf_returns() {
    let source = r#"
fn answer(value: i32) i32 {
    value
}

fn main() i32 {
    answer(42)
}
"#;
    let o3 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O3.policy(),
    );
    let main = o3.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

    assert!(matches!(value.kind, FunctionExprKind::Integer(ref text) if text == "42"));
    assert!(
        o3.optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "propagate-cross-function-constants",
                    ..
                }
            ))
    );
}

#[test]
fn o3_does_not_cross_propagate_parameterized_instance_returns() {
    let source = r#"
fn answer[T](value: T) T {
    value
}

fn main() i32 {
    answer[i32](42)
}
"#;
    let o3 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O3.policy(),
    );
    let main = o3.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

    assert!(matches!(value.kind, FunctionExprKind::Integer(ref text) if text == "42"));
    assert!(
        o3.optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "propagate-cross-function-constants",
                    ..
                }
            ))
    );
}

#[test]
fn o3_propagates_cross_function_constant_instance_returns() {
    let source = r#"
fn answer[T]() i32 {
    42
}

fn main() i32 {
    answer[i32]()
}
"#;
    let o3 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O3.policy(),
    );
    let main = o3.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

    assert!(matches!(value.kind, FunctionExprKind::Integer(ref text) if text == "42"));
    assert!(
        o3.optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "propagate-cross-function-constants",
                    is_instance: false,
                    type_arg_count: 0,
                    ..
                }
            ))
    );
}

#[test]
fn o3_inlines_pure_leaf_function_calls_through_bindings() {
    let source = r#"
fn values() [5]i32 {
    var items = [1, 2, 3, 4, 5];
    items
}

fn main() [5]i32 {
    values()
}
"#;
    let o2 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let o2_main = o2.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let o2_value = first_terminal_value(o2_main.function_body.as_ref().expect("main body"));

    assert!(matches!(o2_value.kind, FunctionExprKind::Call { .. }));

    let o3 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O3.policy(),
    );
    let o3_main = o3.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let o3_value = first_terminal_value(o3_main.function_body.as_ref().expect("main body"));

    assert!(matches!(
        o3_value.kind,
        FunctionExprKind::ArrayLiteral { .. }
    ));
    assert!(
        o3.optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "inline-leaf-functions",
                    is_instance: false,
                    ..
                }
            ))
    );
}

#[test]
fn o3_devirtualizes_direct_trait_object_calls() {
    let source = r#"
trait Source {
    fn add(&const self, rhs: i32) i32;
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    fn add(&const self, rhs: i32) i32 {
        rhs
    }
}

fn main() i32 {
    0
}
"#;
    let counter = std::cell::Cell::new(None);
    let source_trait = std::cell::Cell::new(None);
    let add_method = std::cell::Cell::new(None);
    let counter_ty = std::cell::Cell::new(None);
    let source_object_ty = std::cell::Cell::new(None);
    let i32_ty = std::cell::Cell::new(None);
    let build_dynamic_call = |body: &mut nia_function_ir::FunctionBody| {
        let Some(counter) = counter.get() else {
            return;
        };
        let Some(source_trait) = source_trait.get() else {
            return;
        };
        let Some(add_method) = add_method.get() else {
            return;
        };
        let Some(counter_ty) = counter_ty.get() else {
            return;
        };
        let Some(source_object_ty) = source_object_ty.get() else {
            return;
        };
        let Some(i32_ty) = i32_ty.get() else {
            return;
        };
        let value = first_terminal_value_mut(body);
        let span = value.span;
        value.kind = FunctionExprKind::Call {
            callee: nia_function_ir::FunctionCallee::DynamicTraitMethod {
                object_ty: source_object_ty,
                trait_id: nia_ids::TraitId::Source(source_trait),
                method_id: add_method,
                method_name: "add".to_string(),
                trait_args: Vec::new(),
                slot: 0,
                params: vec![i32_ty],
                return_type: i32_ty,
                receiver: Box::new(FunctionExpr {
                    span,
                    ty: source_object_ty,
                    kind: FunctionExprKind::TraitObjectCoercion {
                        expr: Box::new(FunctionExpr {
                            span,
                            ty: counter_ty,
                            kind: FunctionExprKind::StructLiteral {
                                def_id: counter,
                                fields: Vec::new(),
                            },
                        }),
                        target_ty: source_object_ty,
                        self_ty: counter_ty,
                    },
                }),
            },
            args: vec![FunctionExpr {
                span,
                ty: i32_ty,
                kind: FunctionExprKind::Integer("4".to_string()),
            }],
        };
    };
    let setup_extensions = |extensions: &mut VisibleExtensionMethods,
                            defs: &nia_defs::DefCollection,
                            type_lowering: &TypeLowering,
                            _signatures: &ItemSignatures| {
        let counter_id = global_def_id_by_name(defs, "Counter");
        let source_id = global_def_id_by_name(defs, "Source");
        let add_id = global_def_id_by_name(defs, "add");
        let counter_type = nominal_type_by_def(&type_lowering.interner, counter_id);
        let source_object = nominal_type_by_def(&type_lowering.interner, source_id);
        counter.set(Some(counter_id));
        source_trait.set(Some(source_id));
        add_method.set(Some(add_id));
        counter_ty.set(Some(counter_type));
        source_object_ty.set(Some(source_object));
        i32_ty.set(Some(
            type_lowering.interner.primitive(nia_ty::PrimitiveTy::I32),
        ));
        extensions.insert(
            counter_type,
            VisibleExtensionMethod {
                name: "add".to_string(),
                def_id: add_id,
                trait_id: Some(nia_ids::TraitId::Source(source_id)),
                trait_args: Vec::new(),
            },
        );
    };
    let o2 = lower_source_with_body_mutation_extensions_comptime_mutation_and_optimization(
        source,
        build_dynamic_call,
        setup_extensions,
        |_, _| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let o2_main = o2.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let o2_value = first_terminal_value(o2_main.function_body.as_ref().expect("main body"));

    assert!(matches!(
        o2_value.kind,
        FunctionExprKind::Call {
            callee: nia_function_ir::FunctionCallee::DynamicTraitMethod { .. },
            ..
        }
    ));

    let o3 = lower_source_with_body_mutation_extensions_comptime_mutation_and_optimization(
        source,
        build_dynamic_call,
        setup_extensions,
        |_, _| {},
        nia_opt::NiaOptimizationLevel::O3.policy(),
    );
    let o3_main = o3.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let o3_value = first_terminal_value(o3_main.function_body.as_ref().expect("main body"));

    assert!(matches!(
        o3_value.kind,
        FunctionExprKind::Call {
            callee: nia_function_ir::FunctionCallee::Method { .. },
            ..
        }
    ));
    assert!(
        o3.optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "devirtualize-direct-trait-calls",
                    is_instance: false,
                    ..
                }
            ))
    );
}

#[test]
fn o3_preserves_direct_trait_object_calls_to_generic_impl_instances() {
    let source = r#"
trait Source {
    fn add(&const self, rhs: i32) i32;
}

struct Box[T] {
    value: T,
}

extend[T] Box[T] : Source {
    fn add(&const self, rhs: i32) i32 {
        rhs
    }
}

fn main() i32 {
    var value: Box[i32] = { value: 0 };
    _ = value;
    0
}
"#;
    let box_def = std::cell::Cell::new(None);
    let source_trait = std::cell::Cell::new(None);
    let add_method = std::cell::Cell::new(None);
    let box_i32_ty = std::cell::Cell::new(None);
    let source_object_ty = std::cell::Cell::new(None);
    let i32_ty = std::cell::Cell::new(None);
    let build_dynamic_call = |body: &mut nia_function_ir::FunctionBody| {
        let value = first_terminal_value_mut(body);
        if !matches!(value.kind, FunctionExprKind::Integer(ref text) if text == "0") {
            return;
        }
        let Some(box_def) = box_def.get() else {
            return;
        };
        let Some(source_trait) = source_trait.get() else {
            return;
        };
        let Some(add_method) = add_method.get() else {
            return;
        };
        let Some(box_i32_ty) = box_i32_ty.get() else {
            return;
        };
        let Some(source_object_ty) = source_object_ty.get() else {
            return;
        };
        let Some(i32_ty) = i32_ty.get() else {
            return;
        };
        let span = value.span;
        value.kind = FunctionExprKind::Call {
            callee: nia_function_ir::FunctionCallee::DynamicTraitMethod {
                object_ty: source_object_ty,
                trait_id: nia_ids::TraitId::Source(source_trait),
                method_id: add_method,
                method_name: "add".to_string(),
                trait_args: Vec::new(),
                slot: 0,
                params: vec![i32_ty],
                return_type: i32_ty,
                receiver: Box::new(FunctionExpr {
                    span,
                    ty: source_object_ty,
                    kind: FunctionExprKind::TraitObjectCoercion {
                        expr: Box::new(FunctionExpr {
                            span,
                            ty: box_i32_ty,
                            kind: FunctionExprKind::StructLiteral {
                                def_id: box_def,
                                fields: Vec::new(),
                            },
                        }),
                        target_ty: source_object_ty,
                        self_ty: box_i32_ty,
                    },
                }),
            },
            args: vec![FunctionExpr {
                span,
                ty: i32_ty,
                kind: FunctionExprKind::Integer("4".to_string()),
            }],
        };
    };
    let setup_extensions = |extensions: &mut VisibleExtensionMethods,
                            defs: &nia_defs::DefCollection,
                            type_lowering: &TypeLowering,
                            signatures: &ItemSignatures| {
        let box_id = global_def_id_by_name(defs, "Box");
        let source_id = global_def_id_by_name(defs, "Source");
        let add_id = global_def_id_by_name(defs, "add");
        let i32_type = type_lowering.interner.primitive(nia_ty::PrimitiveTy::I32);
        let box_pattern = signatures
            .trait_impls
            .iter()
            .find_map(|signature| {
                matches!(
                    type_lowering.interner.get(signature.target_ty),
                    Some(nia_ty::TyKind::Nominal { def_id, .. }) if *def_id == box_id
                )
                .then_some(signature.target_ty)
            })
            .expect("Box[T] trait impl target");
        let box_i32_type =
            nominal_type_by_def_with_args(&type_lowering.interner, box_id, &[i32_type]);
        let source_object = nominal_type_by_def(&type_lowering.interner, source_id);
        box_def.set(Some(box_id));
        source_trait.set(Some(source_id));
        add_method.set(Some(add_id));
        box_i32_ty.set(Some(box_i32_type));
        source_object_ty.set(Some(source_object));
        i32_ty.set(Some(i32_type));
        extensions.insert(
            box_pattern,
            VisibleExtensionMethod {
                name: "add".to_string(),
                def_id: add_id,
                trait_id: Some(nia_ids::TraitId::Source(source_id)),
                trait_args: Vec::new(),
            },
        );
    };
    let o3 = lower_source_with_body_mutation_extensions_comptime_mutation_and_optimization(
        source,
        build_dynamic_call,
        setup_extensions,
        |_, _| {},
        nia_opt::NiaOptimizationLevel::O3.policy(),
    );
    let main = o3.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

    assert!(matches!(
        value.kind,
        FunctionExprKind::Call {
            callee: nia_function_ir::FunctionCallee::DynamicTraitMethod { .. },
            ..
        }
    ));
    assert!(
        o3.optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "devirtualize-direct-trait-calls",
                    ..
                }
            ))
    );
}

#[test]
fn o2_inlines_tiny_pure_leaf_function_instance_calls_with_params() {
    let source = r#"
fn identity[T](value: T) T {
    value
}

fn main() i32 {
    identity[i32](7)
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

    assert!(matches!(value.kind, FunctionExprKind::Integer(ref text) if text == "7"));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "inline-leaf-functions",
                    is_instance: true,
                    type_arg_count: 1,
                    ..
                }
            ))
    );
}

#[test]
fn o1_preserves_forwarding_function_instance_calls_with_params() {
    let source = r#"
fn identity[T](value: T) T {
    value
}

fn main() i32 {
    identity[i32](7)
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

    assert!(matches!(value.kind, FunctionExprKind::Call { .. }));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "inline-leaf-functions",
                    is_instance: true,
                    ..
                }
            ))
    );
}

#[test]
fn size_aware_specialization_preserves_non_constant_leaf_function_instance_calls() {
    let source = r#"
fn pair[T]() [2]i32 {
    [1, 2]
}

fn main() [2]i32 {
    pair[i32]()
}
"#;
    let mut policy = nia_opt::NiaOptimizationLevel::O2.policy();
    policy.specialize_generics = nia_opt::SpecializationPolicy::SizeAware;

    let lowering = lower_source_with_body_mutation_and_optimization(source, |_| {}, policy);
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

    assert!(matches!(value.kind, FunctionExprKind::Call { .. }));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "inline-leaf-functions",
                    is_instance: true,
                    ..
                }
            ))
    );
}

#[test]
fn o1_preserves_tiny_pure_leaf_function_calls_with_params() {
    let source = r#"
fn identity(value: i32) i32 {
    value
}

fn main() i32 {
    identity(7)
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

    assert!(matches!(value.kind, FunctionExprKind::Call { .. }));
}

#[test]
fn o2_preserves_leaf_function_calls_with_effectful_args() {
    let source = r#"
fn identity(value: i32) i32 {
    value
}

fn effect() i32 {
    var value = 1;
    value
}

fn main() i32 {
    identity(effect())
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

    assert!(matches!(value.kind, FunctionExprKind::Call { .. }));
}

#[test]
fn o1_removes_noop_backend_local_stores() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let span = body.blocks[0].span;
            let ty = body.ty;
            body.blocks[0].ops.push(FunctionOp::StoreLocal {
                local_id: LocalId(0),
                value: nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                },
                span,
            });
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.blocks.iter().all(|block| block.ops.is_empty()));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "remove-noop-local-stores",
                    is_instance: false,
                    type_arg_count: 0,
                    ..
                }
            ))
    );
}

#[test]
fn o0_preserves_noop_backend_local_stores() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let span = body.blocks[0].span;
            let ty = body.ty;
            body.blocks[0].ops.push(FunctionOp::StoreLocal {
                local_id: LocalId(0),
                value: nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                },
                span,
            });
        },
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.blocks.iter().any(|block| !block.ops.is_empty()));
    assert!(lowering.optimization_report.changed_passes.is_empty());
}

#[test]
fn optimization_report_lists_enabled_pass_inventory_by_scope() {
    let source = r#"
const zeroes: [4]i32 = [0; 4];

fn answer() i32 {
    42
}

fn main() i32 {
    answer() + zeroes[0]
}
"#;

    let o0 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    assert!(o0.optimization_report.enabled_module_passes.is_empty());
    assert!(o0.optimization_report.enabled_function_passes.is_empty());
    assert!(o0.optimization_report.enabled_global_passes.is_empty());

    let o1 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    assert_eq!(
        o1.optimization_report.enabled_module_passes,
        vec!["inline-leaf-functions"]
    );
    assert!(
        o1.optimization_report
            .enabled_function_passes
            .contains(&"remove-unused-temp-bindings")
    );
    assert!(o1.optimization_report.enabled_global_passes.is_empty());

    let o2 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    assert!(
        o2.optimization_report
            .enabled_module_passes
            .contains(&"remove-unused-functions")
    );
    assert!(
        o2.optimization_report
            .enabled_function_passes
            .contains(&"remove-unused-local-bindings")
    );
    assert_eq!(
        o2.optimization_report.enabled_global_passes,
        vec!["simplify-static-init"]
    );

    let o3 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O3.policy(),
    );
    assert!(
        o3.optimization_report
            .enabled_module_passes
            .contains(&"devirtualize-direct-trait-calls")
    );
    assert!(
        o3.optimization_report
            .enabled_module_passes
            .contains(&"propagate-cross-function-constants")
    );
    assert!(
        o3.optimization_report
            .enabled_function_passes
            .contains(&"propagate-local-constants")
    );

    for level in [
        nia_opt::NiaOptimizationLevel::Os,
        nia_opt::NiaOptimizationLevel::Oz,
    ] {
        let lowering =
            lower_source_with_body_mutation_and_optimization(source, |_| {}, level.policy());
        assert!(
            lowering
                .optimization_report
                .enabled_module_passes
                .contains(&"inline-leaf-functions"),
            "{level:?}"
        );
        assert_eq!(
            lowering.optimization_report.enabled_global_passes,
            vec!["simplify-static-init"],
            "{level:?}"
        );
    }
}

#[test]
fn o1_removes_pure_backend_expr_ops() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let span = body.blocks[0].span;
            let ty = body.ty;
            body.blocks[0]
                .ops
                .push(FunctionOp::Expr(nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Binary {
                        lhs: Box::new(nia_function_ir::FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Local(LocalId(0)),
                        }),
                        op: nia_ast::BinaryOp::Add,
                        rhs: Box::new(nia_function_ir::FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Integer("1".to_string()),
                        }),
                    },
                }));
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.blocks.iter().all(|block| block.ops.is_empty()));
}

#[test]
fn o0_preserves_pure_backend_expr_ops() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let span = body.blocks[0].span;
            let ty = body.ty;
            body.blocks[0]
                .ops
                .push(FunctionOp::Expr(nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Binary {
                        lhs: Box::new(nia_function_ir::FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Local(LocalId(0)),
                        }),
                        op: nia_ast::BinaryOp::Add,
                        rhs: Box::new(nia_function_ir::FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Integer("1".to_string()),
                        }),
                    },
                }));
        },
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.blocks.iter().any(|block| !block.ops.is_empty()));
}

#[test]
fn o2_removes_unused_backend_local_bindings() {
    let source = r#"
fn main() i32 {
    var unused = 1;
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.locals.iter().all(|local| local.name != "unused"));
    assert!(body.blocks.iter().all(|block| {
        block.ops.iter().all(|op| {
            !matches!(
                op,
                FunctionOp::Binding(binding) if binding.name == "unused"
            )
        })
    }));
}

#[test]
fn o1_preserves_unused_backend_local_bindings() {
    let source = r#"
fn main() i32 {
    var unused = 1;
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.locals.iter().any(|local| local.name == "unused"));
    assert!(body.blocks.iter().any(|block| {
        block.ops.iter().any(|op| {
            matches!(
                op,
                FunctionOp::Binding(binding) if binding.name == "unused"
            )
        })
    }));
}

#[test]
fn o1_removes_unused_backend_temp_bindings() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let span = body.span;
            let ty = body.ty;
            let temp = LocalId(999);
            body.locals.push(nia_function_ir::FunctionLocal {
                id: temp,
                name: "fir.tmp.999".to_string(),
                kind: nia_function_ir::FunctionLocalKind::Binding,
                ty,
                span,
            });
            body.blocks[0]
                .ops
                .push(FunctionOp::Binding(nia_function_ir::FunctionBinding {
                    local_id: temp,
                    name: "fir.tmp.999".to_string(),
                    ty,
                    value: Some(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Integer("1".to_string()),
                    }),
                    is_const: false,
                }));
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.locals.iter().all(|local| local.name != "fir.tmp.999"));
    assert!(body.blocks.iter().all(|block| {
        block.ops.iter().all(|op| {
            !matches!(
                op,
                FunctionOp::Binding(binding) if binding.name == "fir.tmp.999"
            )
        })
    }));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| {
                matches!(
                    change,
                    BackendOptimizationChange::Function {
                        pass: "remove-unused-temp-bindings",
                        ..
                    }
                )
            })
    );
}

#[test]
fn o1_removes_pure_zst_local_runtime_ops() {
    let source = r#"
struct Empty {}

fn main() i32 {
    var local: Empty = {};
    local = {};
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.blocks.iter().all(|block| {
        block.ops.iter().all(|op| {
            !matches!(
                op,
                FunctionOp::Binding(binding) if binding.name == "local"
            )
        })
    }));
    assert!(body.blocks.iter().all(|block| {
        block
            .ops
            .iter()
            .all(|op| !matches!(op, FunctionOp::StoreLocal { .. }))
    }));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| {
                matches!(
                    change,
                    BackendOptimizationChange::Function {
                        pass: "remove-zst-local-runtime-ops",
                        ..
                    }
                )
            })
    );
}

#[test]
fn o0_preserves_pure_zst_local_runtime_ops() {
    let source = r#"
struct Empty {}

fn main() i32 {
    var local: Empty = {};
    local = {};
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.blocks.iter().any(|block| {
        block.ops.iter().any(|op| {
            matches!(
                op,
                FunctionOp::Binding(binding) if binding.name == "local"
            )
        })
    }));
    assert!(body.blocks.iter().any(|block| {
        block.ops.iter().any(|op| {
            matches!(
                op,
                FunctionOp::Expr(FunctionExpr {
                    kind: FunctionExprKind::Assign { .. },
                    ..
                })
            )
        })
    }));
    assert!(lowering.optimization_report.changed_passes.is_empty());
}

#[test]
fn o1_preserves_effects_from_zst_local_runtime_ops() {
    let source = r#"
struct Wrap {
    value: void,
}

extern fn log(value: i32);

fn effect(value: i32) void {
    log(value);
}

fn main() i32 {
    var local: Wrap = { value: effect(1) };
    local = { value: effect(2) };
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");
    let effectful_exprs = body
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .filter(|op| zst_struct_effect_expr(op))
        .count();

    assert_eq!(effectful_exprs, 2);
    assert!(body.blocks.iter().all(|block| {
        block.ops.iter().all(|op| {
            !matches!(
                op,
                FunctionOp::Binding(binding) if binding.name == "local"
            )
        })
    }));
    assert!(body.blocks.iter().all(|block| {
        block
            .ops
            .iter()
            .all(|op| !matches!(op, FunctionOp::StoreLocal { .. }))
    }));
}

fn zst_struct_effect_expr(op: &FunctionOp) -> bool {
    match op {
        FunctionOp::Expr(FunctionExpr {
            kind: FunctionExprKind::StructLiteral { .. },
            ..
        }) => true,
        FunctionOp::Expr(FunctionExpr {
            kind: FunctionExprKind::Discard(inner),
            ..
        }) => matches!(inner.kind, FunctionExprKind::StructLiteral { .. }),
        _ => false,
    }
}

#[test]
fn o0_preserves_zst_local_runtime_ops() {
    let source = r#"
struct Empty {}

fn main() i32 {
    var local: Empty = {};
    local = {};
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.blocks.iter().any(|block| {
        block.ops.iter().any(|op| {
            matches!(
                op,
                FunctionOp::Binding(binding) if binding.name == "local"
            )
        })
    }));
    assert!(body.blocks.iter().any(|block| {
        block.ops.iter().any(|op| {
            matches!(
                op,
                FunctionOp::Expr(FunctionExpr {
                    kind: FunctionExprKind::Assign { .. },
                    ..
                })
            )
        })
    }));
}

#[test]
fn o2_propagates_backend_local_copies() {
    let source = r#"
fn main() i32 {
    var source = 1;
    var copy = source;
    copy
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");
    let source_id = body
        .locals
        .iter()
        .find(|local| local.name == "source")
        .expect("source local")
        .id;
    let value = first_terminal_value(body);
    assert!(matches!(value.kind, FunctionExprKind::Local(id) if id == source_id));
    assert!(body.locals.iter().all(|local| local.name != "copy"));
}

#[test]
fn o1_preserves_backend_local_copies() {
    let source = r#"
fn main() i32 {
    var source = 1;
    var copy = source;
    copy
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");
    let copy_id = body
        .locals
        .iter()
        .find(|local| local.name == "copy")
        .expect("copy local")
        .id;
    let value = first_terminal_value(body);

    assert!(matches!(value.kind, FunctionExprKind::Local(id) if id == copy_id));
    assert!(body.locals.iter().any(|local| local.name == "copy"));
}

#[test]
fn o3_propagates_backend_local_constants() {
    let source = r#"
fn main() i32 {
    var value = 42;
    value
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O3.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");
    let value = first_terminal_value(body);

    assert!(matches!(
        &value.kind,
        FunctionExprKind::Integer(value) if value == "42"
    ));
    assert!(body.locals.iter().all(|local| local.name != "value"));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "propagate-local-constants",
                    ..
                }
            ))
    );
}

#[test]
fn o2_preserves_backend_local_constants() {
    let source = r#"
fn main() i32 {
    var value = 42;
    value
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");
    let value_id = body
        .locals
        .iter()
        .find(|local| local.name == "value")
        .expect("value local")
        .id;
    let value = first_terminal_value(body);

    assert!(matches!(value.kind, FunctionExprKind::Local(id) if id == value_id));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "propagate-local-constants",
                    ..
                }
            ))
    );
}

#[test]
fn o2_folds_constant_backend_switches() {
    let source = r#"
fn main() i32 {
    switch 2 {
        1 => 10,
        2 => 20,
        _ => 30,
    }
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(
        body.blocks
            .iter()
            .all(|block| { !matches!(block.terminator, FunctionTerminator::Switch { .. }) })
    );
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "fold-constant-switches",
                    ..
                }
            ))
    );
}

#[test]
fn o1_preserves_constant_backend_switches() {
    let source = r#"
fn main() i32 {
    switch 2 {
        1 => 10,
        2 => 20,
        _ => 30,
    }
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(
        body.blocks
            .iter()
            .any(|block| matches!(block.terminator, FunctionTerminator::Switch { .. }))
    );
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "fold-constant-switches",
                    ..
                }
            ))
    );
}

#[test]
fn o2_removes_overwritten_backend_local_stores() {
    let source = r#"
fn main() i32 {
    var target = 0;
    target
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let target_id = body
                .locals
                .iter()
                .find(|local| local.name == "target")
                .expect("target local")
                .id;
            let span = body.blocks[0].span;
            let ty = body.ty;
            body.blocks[0].ops.push(FunctionOp::StoreLocal {
                local_id: target_id,
                value: nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("1".to_string()),
                },
                span,
            });
            body.blocks[0].ops.push(FunctionOp::StoreLocal {
                local_id: target_id,
                value: nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("2".to_string()),
                },
                span,
            });
        },
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert_eq!(
        body.blocks
            .iter()
            .flat_map(|block| &block.ops)
            .filter(|op| matches!(op, FunctionOp::StoreLocal { .. }))
            .count(),
        1
    );
}

#[test]
fn o1_preserves_overwritten_backend_local_stores() {
    let source = r#"
fn main() i32 {
    var target = 0;
    target
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let target_id = body
                .locals
                .iter()
                .find(|local| local.name == "target")
                .expect("target local")
                .id;
            let span = body.blocks[0].span;
            let ty = body.ty;
            body.blocks[0].ops.push(FunctionOp::StoreLocal {
                local_id: target_id,
                value: nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("1".to_string()),
                },
                span,
            });
            body.blocks[0].ops.push(FunctionOp::StoreLocal {
                local_id: target_id,
                value: nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("2".to_string()),
                },
                span,
            });
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert_eq!(
        body.blocks
            .iter()
            .flat_map(|block| &block.ops)
            .filter(|op| matches!(op, FunctionOp::StoreLocal { .. }))
            .count(),
        2
    );
}

#[test]
fn o2_removes_never_read_backend_local_stores() {
    let source = r#"
fn main() i32 {
    var target = 0;
    1
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let target_id = body
                .locals
                .iter()
                .find(|local| local.name == "target")
                .expect("target local")
                .id;
            let span = body.blocks[0].span;
            let ty = body.ty;
            body.blocks[0].ops.push(FunctionOp::StoreLocal {
                local_id: target_id,
                value: nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("2".to_string()),
                },
                span,
            });
        },
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.blocks.iter().all(|block| {
        block
            .ops
            .iter()
            .all(|op| !matches!(op, FunctionOp::StoreLocal { .. }))
    }));
}

#[test]
fn o1_preserves_never_read_backend_local_stores() {
    let source = r#"
fn main() i32 {
    var target = 0;
    1
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let target_id = body
                .locals
                .iter()
                .find(|local| local.name == "target")
                .expect("target local")
                .id;
            let span = body.blocks[0].span;
            let ty = body.ty;
            body.blocks[0].ops.push(FunctionOp::StoreLocal {
                local_id: target_id,
                value: nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("2".to_string()),
                },
                span,
            });
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.blocks.iter().any(|block| {
        block
            .ops
            .iter()
            .any(|op| matches!(op, FunctionOp::StoreLocal { .. }))
    }));
}

#[test]
fn unresolved_array_lengths_in_backend_symbols_are_diagnostic_not_panic() {
    let source = r#"
comptime N: usize = 3;

struct Box[T] {
    value: T,
}

fn main(value: Box[[N]u8]) void {}
"#;
    let lowering = lower_source_with_comptime_mutation(source, |comptime, type_lowering| {
        for id in type_lowering.const_exprs.keys() {
            comptime.array_lengths.remove(id);
        }
    });

    let module = &lowering.program.modules[0];
    let instance = module
        .struct_instances
        .iter()
        .find(|instance| instance.name == "Box")
        .expect("Box instance");

    assert!(
        instance.symbol.contains("len_unresolved__m0__c0"),
        "{}",
        instance.symbol
    );
    assert_eq!(lowering.diagnostics.len(), 1);
    assert!(
        lowering.diagnostics[0]
            .message
            .contains("was not evaluated before backend symbol generation"),
        "{:?}",
        lowering.diagnostics
    );
}

#[test]
fn append_missing_layout_instances_uses_existing_keys_as_set() {
    let key_existing = nia_layout::StructLayoutKey {
        def_id: DefId(1),
        args: Vec::new(),
    };
    let key_new = nia_layout::StructLayoutKey {
        def_id: DefId(2),
        args: Vec::new(),
    };
    let existing_layout = nia_layout::StructLayout {
        layout: nia_layout::TypeLayout { size: 4, align: 4 },
        fields: Vec::new(),
    };
    let duplicate_layout = nia_layout::StructLayout {
        layout: nia_layout::TypeLayout { size: 8, align: 8 },
        fields: Vec::new(),
    };
    let new_layout = nia_layout::StructLayout {
        layout: nia_layout::TypeLayout { size: 16, align: 8 },
        fields: Vec::new(),
    };
    let module_id = ModuleId(0);
    let mut output = vec![(
        BackendStructInstanceKey::from_module_key(module_id, &key_existing),
        existing_layout.clone(),
    )];
    let computed = HashMap::from([
        (key_existing.clone(), duplicate_layout),
        (key_new.clone(), new_layout.clone()),
    ]);

    append_missing_layout_instances(module_id, &mut output, computed);

    assert_eq!(output.len(), 2);
    assert_eq!(output[0].1, existing_layout);
    assert!(output.iter().any(|(key, layout)| key
        == &BackendStructInstanceKey::from_module_key(module_id, &key_new)
        && layout == &new_layout));
}

fn lower_source(source: &str) -> BackendLowering {
    let lowering = lower_source_with_comptime_mutation(source, |_, _| {});
    assert!(
        lowering.diagnostics.is_empty(),
        "{:?}",
        lowering.diagnostics
    );
    lowering
}

fn lower_source_with_comptime_mutation(
    source: &str,
    mutate_comptime: impl FnOnce(&mut nia_comptime_check::ComptimeCheck, &TypeLowering),
) -> BackendLowering {
    lower_source_with_body_mutation_comptime_mutation_and_optimization(
        source,
        |_| {},
        mutate_comptime,
        nia_opt::OptimizationPolicy::default(),
    )
}

fn lower_source_with_body_mutation_and_optimization(
    source: &str,
    mutate_body: impl FnMut(&mut nia_function_ir::FunctionBody),
    optimization: nia_opt::OptimizationPolicy,
) -> BackendLowering {
    lower_source_with_body_mutation_extensions_comptime_mutation_and_optimization(
        source,
        mutate_body,
        |_, _, _, _| {},
        |_, _| {},
        optimization,
    )
}

fn lower_source_with_body_mutation_comptime_mutation_and_optimization(
    source: &str,
    mutate_body: impl FnMut(&mut nia_function_ir::FunctionBody),
    mutate_comptime: impl FnOnce(&mut nia_comptime_check::ComptimeCheck, &TypeLowering),
    optimization: nia_opt::OptimizationPolicy,
) -> BackendLowering {
    lower_source_with_body_mutation_extensions_comptime_mutation_and_optimization(
        source,
        mutate_body,
        |_, _, _, _| {},
        mutate_comptime,
        optimization,
    )
}

fn lower_source_with_body_mutation_extensions_comptime_mutation_and_optimization(
    source: &str,
    mutate_body: impl FnMut(&mut nia_function_ir::FunctionBody),
    mutate_extensions: impl FnOnce(
        &mut VisibleExtensionMethods,
        &nia_defs::DefCollection,
        &TypeLowering,
        &ItemSignatures,
    ),
    mutate_comptime: impl FnOnce(&mut nia_comptime_check::ComptimeCheck, &TypeLowering),
    optimization: nia_opt::OptimizationPolicy,
) -> BackendLowering {
    lower_source_with_body_check_mutation_and_optimization(
        source,
        mutate_body,
        mutate_extensions,
        mutate_comptime,
        |_, _, _, _| {},
        optimization,
    )
}

fn lower_source_with_body_check_mutation_and_optimization(
    source: &str,
    mut mutate_body: impl FnMut(&mut nia_function_ir::FunctionBody),
    mutate_extensions: impl FnOnce(
        &mut VisibleExtensionMethods,
        &nia_defs::DefCollection,
        &TypeLowering,
        &ItemSignatures,
    ),
    mutate_comptime: impl FnOnce(&mut nia_comptime_check::ComptimeCheck, &TypeLowering),
    mutate_body_check: impl FnOnce(
        &mut nia_body_check::BodyCheck,
        &nia_ast::Module,
        &nia_defs::DefCollection,
        &ItemSignatures,
    ),
    optimization: nia_opt::OptimizationPolicy,
) -> BackendLowering {
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
        type_uses: &type_lowering.type_uses,
        normalized: &normalization.normalized,
        const_exprs: &type_lowering.const_exprs,
        program: nia_comptime_check::ComptimeProgramContext::empty(),
    });
    let layouts = nia_layout::compute_layouts_with_normalized_types(
        &defs,
        &normalization.interner,
        &signatures,
        &normalization.normalized,
        &|id| comptime.array_lengths.get(&id).copied(),
        nia_layout::TargetDataLayout::LP64,
    );
    let mut extensions = VisibleExtensionMethods::default();
    mutate_extensions(&mut extensions, &defs, &type_lowering, &signatures);
    let origins = NodeOriginTable::default();
    let mut body_check = check_module_bodies_with_program_signatures_and_layouts(BodyCheckInput {
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
            traits: &HashMap::new(),
            trait_impls: &[],
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
    mutate_body_check(&mut body_check, &module, &defs, &signatures);
    let function_bodies = body_check
        .ir
        .function_bodies
        .iter()
        .map(|(def_id, body)| {
            let mut body = lower_function_body(body);
            mutate_body(&mut body);
            (*def_id, body)
        })
        .collect::<HashMap<_, _>>();
    let monomorphization =
        nia_monomorphize::collect_monomorphizations(&[nia_monomorphize::MonomorphizeModuleInput {
            module_id: ModuleId(0),
            defs: &defs,
            interner: &body_check.ir.interner,
            comptime: &comptime,
            const_exprs: &type_lowering.const_exprs,
            instantiations: &body_check.ir.generic_instantiations,
        }]);
    assert!(
        monomorphization.diagnostics.is_empty(),
        "{:?}",
        monomorphization.diagnostics
    );
    let mut comptime = comptime;
    mutate_comptime(&mut comptime, &type_lowering);

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
        program_enums: &HashMap::new(),
        program_traits: &HashMap::new(),
        trait_impls: &[],
    };
    lower_backend_program(&[input], &monomorphization, optimization)
}

fn global_def_id_by_name(defs: &nia_defs::DefCollection, name: &str) -> GlobalDefId {
    defs.defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.name == name).then_some(GlobalDefId {
                module_id: defs.module_id,
                def_id,
            })
        })
        .unwrap_or_else(|| panic!("missing def `{name}`"))
}

fn nominal_type_by_def(interner: &nia_ty::TyInterner, target: GlobalDefId) -> InternedTyId {
    nominal_type_by_def_with_args(interner, target, &[])
}

fn nominal_type_by_def_with_args(
    interner: &nia_ty::TyInterner,
    target: GlobalDefId,
    target_args: &[InternedTyId],
) -> InternedTyId {
    interner
        .iter()
        .find_map(|(ty, kind)| {
            matches!(
                kind,
                nia_ty::TyKind::Nominal {
                    def_id,
                    args
                } if *def_id == target && args == target_args
            )
            .then_some(ty)
        })
        .unwrap_or_else(|| panic!("missing nominal type {target:?} with args {target_args:?}"))
}

fn first_terminal_value(body: &nia_function_ir::FunctionBody) -> &nia_function_ir::FunctionExpr {
    body.blocks
        .iter()
        .find_map(|block| match &block.terminator {
            FunctionTerminator::Return {
                value: Some(value), ..
            }
            | FunctionTerminator::Tail {
                value: Some(value), ..
            } => Some(value),
            _ => None,
        })
        .expect("terminal value")
}

fn first_terminal_value_mut(
    body: &mut nia_function_ir::FunctionBody,
) -> &mut nia_function_ir::FunctionExpr {
    body.blocks
        .iter_mut()
        .find_map(|block| match &mut block.terminator {
            FunctionTerminator::Return {
                value: Some(value), ..
            }
            | FunctionTerminator::Tail {
                value: Some(value), ..
            } => Some(value),
            _ => None,
        })
        .expect("terminal value")
}
