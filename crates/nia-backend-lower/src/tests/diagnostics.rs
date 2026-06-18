// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn unresolved_array_lengths_in_backend_symbols_are_diagnostic_not_panic() {
    let source = r#"
comptime let N: usize = 3;

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
            .summary
            .contains("was not evaluated before backend symbol generation"),
        "{:?}",
        lowering.diagnostics
    );
}
