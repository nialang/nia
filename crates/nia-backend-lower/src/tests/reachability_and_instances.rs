// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn o2_skips_lowering_unused_private_functions() {
    let source = r#"
fn used(value: i32) i32 {
    let mut out = value;
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
            .any(|function| function.name == sym("used"))
    );
    assert!(
        module
            .functions
            .iter()
            .any(|function| function.name == sym("main"))
    );
    assert!(
        module
            .functions
            .iter()
            .any(|function| function.name == sym("exported"))
    );
    assert!(
        !module
            .functions
            .iter()
            .any(|function| function.name == sym("unused"))
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
fn o2_does_not_preserve_function_refs_inside_empty_repeat_static_initializers() {
    let source = r#"
static values: [0]i32 = [1; 0];

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
        |_, _, _, _, _| {},
        |_, _| {},
        |body_check, _, defs, _, _| {
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
            .any(|function| function.name == sym("unused"))
    );
    let values = module
        .globals
        .iter()
        .find(|global| global.name == sym("values"))
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
fn o2_preserves_function_refs_inside_static_initializers() {
    let source = r#"
static values: [2]usize = [0, 0];

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
        |_, _, _, _, _| {},
        |_, _| {},
        |body_check, _, defs, _, interner| {
            let values = global_def_id_by_name(defs, "values");
            let kept = global_def_id_by_name(defs, "kept");
            let kept_id = global_def_id_by_name(defs, "kept_id");
            let i32_ty = interner.primitive(nia_ty::PrimitiveTy::I32);
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
            .any(|function| function.name == sym("kept"))
    );
    assert!(
        !module
            .functions
            .iter()
            .any(|function| function.name == sym("seed"))
    );
    assert!(
        module
            .function_instances
            .iter()
            .any(|instance| instance.name == sym("kept_id"))
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
fn o2_preserves_transitively_used_private_functions() {
    let source = r#"
fn leaf(value: i32) i32 {
    let mut out = value;
    out
}

fn middle() i32 {
    let mut out = leaf(1);
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
            .any(|function| function.name == sym("leaf"))
    );
    assert!(
        module
            .functions
            .iter()
            .any(|function| function.name == sym("middle"))
    );
    assert!(
        !module
            .functions
            .iter()
            .any(|function| function.name == sym("unused"))
    );
}

#[test]
fn o1_skips_lowering_unused_private_functions() {
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
        !module
            .functions
            .iter()
            .any(|function| function.name == sym("unused"))
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
            .any(|instance| instance.name == sym("unused_id"))
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
    let mut out = value;
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
            .any(|instance| instance.name == sym("id"))
    );
}

#[test]
fn exact_function_instance_keys_are_deduplicated() {
    let source = r#"
fn id[T](value: T) T {
    let mut out = value;
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
        .filter(|instance| instance.name == sym("id"))
        .collect::<Vec<_>>();

    assert_eq!(instances.len(), 1);
    assert_eq!(instances[0].args.len(), 1);
}

#[test]
fn o2_preserves_transitively_used_private_function_instances() {
    let source = r#"
fn id[T](value: T) T {
    let mut out = value;
    out
}

fn wrapper[T](value: T) T {
    let mut out = id[T](value);
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
            .any(|instance| instance.name == sym("id"))
    );
    assert!(
        module
            .function_instances
            .iter()
            .any(|instance| instance.name == sym("wrapper"))
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
            .any(|instance| instance.name == sym("id"))
    );
}
