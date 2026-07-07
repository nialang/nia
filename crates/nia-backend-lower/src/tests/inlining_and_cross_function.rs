// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

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
        .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
            .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
            .find(|function| function.name == sym("main"))
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
            .find(|function| function.name == sym("main"))
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
                .any(|instance| instance.name == sym("identity")),
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
    let mut value = 1;
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
            .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
    let mut items = [1, 2, 3, 4, 5];
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
        .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
    fn add(&self, rhs: i32) i32;
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    fn add(&self, rhs: i32) i32 {
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
                method_name: nia_symbol::known::ADD,
                trait_args: Vec::new(),
                slot: 0,
                params: vec![i32_ty],
                return_type: i32_ty,
                receiver_kind: nia_ids::ReceiverKind::Ref,
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
        let impl_id = _signatures.trait_impls[0].impl_id;
        extensions.insert(
            impl_id,
            counter_type,
            VisibleExtensionMethod {
                name: sym("add"),
                def_id: add_id,
                impl_id,
                effective_generics: Vec::new(),
                trait_id: Some(nia_ids::TraitId::Source(source_id)),
                trait_args: Vec::new(),
                where_predicates: Vec::new(),
                is_callable: true,
                is_trait_witness: true,
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
        .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
    fn add(&self, rhs: i32) i32;
}

struct Box[T] {
    value: T,
}

extend[T] Box[T] : Source {
    fn add(&self, rhs: i32) i32 {
        rhs
    }
}

fn main() i32 {
    let mut value: Box[i32] = { value: 0 };
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
                method_name: nia_symbol::known::ADD,
                trait_args: Vec::new(),
                slot: 0,
                params: vec![i32_ty],
                return_type: i32_ty,
                receiver_kind: nia_ids::ReceiverKind::Ref,
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
        let impl_id = signatures.trait_impls[0].impl_id;
        extensions.insert(
            impl_id,
            box_pattern,
            VisibleExtensionMethod {
                name: sym("add"),
                def_id: add_id,
                impl_id,
                effective_generics: vec![sym("T")],
                trait_id: Some(nia_ids::TraitId::Source(source_id)),
                trait_args: Vec::new(),
                where_predicates: Vec::new(),
                is_callable: true,
                is_trait_witness: true,
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
        .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
        .find(|function| function.name == sym("main"))
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
    let mut value = 1;
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
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let value = first_terminal_value(main.function_body.as_ref().expect("main body"));

    assert!(matches!(value.kind, FunctionExprKind::Call { .. }));
}
