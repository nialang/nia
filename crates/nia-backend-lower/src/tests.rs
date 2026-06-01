// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_abi_check::check_module_abi;
use nia_body_check::{BodyCheckInput, check_module_bodies_with_program_signatures_and_layouts};
use nia_defs::{DefKind, VisibleExtensionMethod, VisibleExtensionMethods, collect_module_defs};
use nia_flow_check::check_module_flow;
use nia_function_ir::{
    FunctionArrayElements, FunctionBlockId, FunctionExprKind, FunctionOp, FunctionTerminator,
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
    lower_source_with_body_mutation_comptime_mutation_and_optimization(
        source,
        mutate_body,
        |_, _| {},
        optimization,
    )
}

fn lower_source_with_body_mutation_comptime_mutation_and_optimization(
    source: &str,
    mut mutate_body: impl FnMut(&mut nia_function_ir::FunctionBody),
    mutate_comptime: impl FnOnce(&mut nia_comptime_check::ComptimeCheck, &TypeLowering),
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
