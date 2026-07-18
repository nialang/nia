// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn invalid_function_ir_is_rejected_before_backend_lowering() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let value = first_terminal_value_mut(body);
            value.kind = FunctionExprKind::Error;
        },
        nia_opt::OptimizationPolicy::default(),
    );

    assert!(
        lowering.program.modules.is_empty(),
        "{:?}",
        lowering.program
    );
    assert_eq!(lowering.diagnostics.len(), 1, "{:?}", lowering.diagnostics);
    assert_eq!(lowering.diagnostics[0].code.as_str(), "I0201");
    assert!(
        lowering.diagnostics[0]
            .summary
            .contains("invalid function IR passed to backend lowering"),
        "{:?}",
        lowering.diagnostics
    );
    assert!(
        lowering.diagnostics[0]
            .primary_message()
            .is_some_and(|message| message.contains("error expression escaped")),
        "{:?}",
        lowering.diagnostics
    );
}

#[test]
fn unresolved_array_lengths_in_backend_symbols_are_diagnostic_not_panic() {
    let source = r#"
const N: usize = 3;

struct Box[T] {
    value: T,
}

fn main(value: Box[[N]u8]) void {}
"#;
    let lowering = lower_source_with_const_mutation(source, |const_eval, type_lowering| {
        for id in type_lowering.const_exprs.keys() {
            Arc::make_mut(&mut const_eval.array_lengths).remove(id);
        }
    });

    let module = &lowering.program.modules[0];
    let instance = module
        .struct_instances
        .iter()
        .find(|instance| instance.name == sym("Box"))
        .expect("Box instance");

    assert!(
        instance.symbol.contains("len_unresolved__m0__c0"),
        "{}",
        instance.symbol
    );
    assert_eq!(lowering.diagnostics.len(), 1);
    assert!(
        lowering.diagnostics[0]
            .summary
            .contains("was not evaluated before backend symbol generation"),
        "{:?}",
        lowering.diagnostics
    );
}
