// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn o2_simplifies_zero_static_initializers() {
    let source = r#"
static zeroes: [i32; 4] = [0; 4];

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
        .find(|global| global.name == sym("zeroes"))
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
static zeroes: [f64; 2] = [0.0f64, 0.0];

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
        .find(|global| global.name == sym("zeroes"))
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
static values: [i32; 0] = [1; 0];

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
        .find(|global| global.name == sym("values"))
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
static values: [i32; 3] = [7, 7, 7];

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
        .find(|global| global.name == sym("values"))
        .expect("values global");

    assert!(matches!(
        values.init,
        Some(StaticInit::Repeat {
            ref value,
            count: 3
        }) if matches!(**value, StaticInit::Int(value) if value.as_i128() == Some(7))
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
static bytes: [u8; 3] = b"aaa";

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
        .find(|global| global.name == sym("bytes"))
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
static text: [char; 3] = "aaa";

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
        .find(|global| global.name == sym("text"))
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
static zeroes: [i32; 4] = [0, 0, 0, 0];
static values: [i32; 3] = [7, 7, 7];

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
            .find(|global| global.name == sym("zeroes"))
            .expect("zeroes global");
        let values = module
            .globals
            .iter()
            .find(|global| global.name == sym("values"))
            .expect("values global");

        assert!(matches!(zeroes.init, Some(StaticInit::Zero)), "{level:?}");
        assert!(
            matches!(
                values.init,
                Some(StaticInit::Repeat {
                    ref value,
                    count: 3
                }) if matches!(**value, StaticInit::Int(value) if value.as_i128() == Some(7))
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
static zeroes: [i32; 4] = [0; 4];

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
        .find(|global| global.name == sym("zeroes"))
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
static zeroes: [f32; 2] = [0.0f32, 0.0f32];

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
        .find(|global| global.name == sym("zeroes"))
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
static values: [i32; 3] = [7, 7, 7];

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
        .find(|global| global.name == sym("values"))
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
static bytes: [u8; 3] = b"aaa";
static text: [char; 3] = "aaa";

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
        .find(|global| global.name == sym("bytes"))
        .expect("bytes global");
    let text = module
        .globals
        .iter()
        .find(|global| global.name == sym("text"))
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
fn lowers_global_string_literals_as_static_arrays() {
    let source = r#"
static bytes = b"ok";
static text = "hi";

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
        .find(|global| global.name == sym("bytes"))
        .expect("bytes global");
    let text = module
        .globals
        .iter()
        .find(|global| global.name == sym("text"))
        .expect("text global");

    assert!(matches!(bytes.init, Some(StaticInit::Bytes(_))));
    assert!(matches!(text.init, Some(StaticInit::Chars(_))));
}
